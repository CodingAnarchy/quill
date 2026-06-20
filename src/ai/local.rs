//! Local-model backend (#124): runs a quantized Qwen GGUF in-process with candle
//! (Metal on Apple Silicon, CPU elsewhere), no daemon. Behind the `local` cargo
//! feature so its heavy deps stay out of default builds.
//!
//! Structured output is enforced by [`crate::ai::constrain`]: when a request
//! carries a `json_schema`, the decode loop only ever accepts a token that keeps
//! the output a viable prefix of a schema-valid value, so the reply is guaranteed
//! parseable — the same guarantee the Claude backend gets from the API. Without a
//! schema (e.g. Q&A) it decodes greedily.
//!
//! Wiring this `LocalBackend` into settings / backend-switching is #87; this
//! module is the engine it will select — hence the `dead_code` allowance until
//! that wiring lands. The end-to-end path is covered by the `#[ignore]`d
//! integration test below (it needs a model on disk).
#![allow(dead_code)]

use std::sync::Mutex;

use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use tokenizers::Tokenizer;

use crate::ai::constrain::Constraint;
use crate::ai::{AiError, Backend, Request, Response, Usage};

/// How many top-logit candidates the constrained loop tests per step before
/// giving up — the schema-valid token is almost always in the first few.
const TOP_K: usize = 64;

/// Qwen end-of-turn / end-of-text tokens (stop signals for unconstrained decode).
const EOS_TOKENS: [u32; 2] = [151645, 151643]; // <|im_end|>, <|endoftext|>

/// A loaded local model. `forward` mutates the model's KV cache, so it sits behind
/// a `Mutex` to satisfy the `&self` [`Backend`] trait.
pub struct LocalBackend {
    model: Mutex<Qwen2>,
    tokenizer: Tokenizer,
    device: Device,
}

impl LocalBackend {
    /// Load a quantized Qwen from a `GGUF` file plus its `tokenizer.json`. Uses
    /// Metal when candle was built with it (macOS), else CPU.
    pub fn load(gguf_path: &str, tokenizer_path: &str) -> Result<Self, AiError> {
        let device = Device::new_metal(0).or_else(|_| Device::new_cuda(0)).unwrap_or(Device::Cpu);
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| api(format!("tokenizer: {e}")))?;
        let mut file = std::fs::File::open(gguf_path).map_err(|e| api(format!("open gguf: {e}")))?;
        let content = gguf_file::Content::read(&mut file).map_err(|e| api(format!("read gguf: {e}")))?;
        let model = Qwen2::from_gguf(content, &mut file, &device).map_err(|e| api(format!("load gguf: {e}")))?;
        Ok(Self { model: Mutex::new(model), tokenizer, device })
    }

    /// Build the Qwen chat-template prompt for a request (manuscript folded into
    /// the user turn, since the local backend has no separate cached block).
    fn prompt(req: &Request) -> String {
        let system = req.system.as_deref().unwrap_or("You are a helpful assistant.");
        let user = if req.cached_context.is_empty() {
            req.user.clone()
        } else {
            format!("{}\n\n{}", req.cached_context, req.user)
        };
        format!(
            "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"
        )
    }
}

impl Backend for LocalBackend {
    fn complete(&self, req: &Request) -> Result<Response, AiError> {
        let prompt = Self::prompt(req);
        let prompt_ids = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| api(format!("encode: {e}")))?
            .get_ids()
            .to_vec();

        let mut model = self.model.lock().map_err(|_| api("model lock poisoned".into()))?;
        let mut constraint = req.json_schema.as_ref().map(Constraint::new);

        // Prompt pass → logits for the first generated token.
        let mut logits = self.step(&mut model, &prompt_ids, 0)?;

        let mut generated: Vec<u32> = Vec::new();
        for _ in 0..req.max_tokens {
            let next = match &mut constraint {
                Some(c) => match self.pick_constrained(c, &logits, &generated)? {
                    Some(id) => id,
                    None => break, // nothing keeps the output valid → stop
                },
                None => argmax(&logits),
            };

            if constraint.is_none() && EOS_TOKENS.contains(&next) {
                break;
            }
            generated.push(next);
            if constraint.as_ref().is_some_and(Constraint::is_complete) {
                break; // a complete schema-valid value
            }
            logits = self.step(&mut model, &[next], prompt_ids.len() + generated.len() - 1)?;
        }

        let text = match &constraint {
            // The constraint accumulated the exact accepted text already.
            Some(c) => c.text().to_string(),
            None => self.tokenizer.decode(&generated, true).map_err(|e| api(format!("decode: {e}")))?,
        };
        Ok(Response { text, usage: Usage { output_tokens: generated.len() as u32, ..Usage::default() } })
    }
}

impl LocalBackend {
    /// One forward pass; returns the last position's logits as `f32`.
    fn step(&self, model: &mut Qwen2, tokens: &[u32], pos: usize) -> Result<Vec<f32>, AiError> {
        let input = Tensor::new(tokens, &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| api(format!("input tensor: {e}")))?;
        model
            .forward(&input, pos)
            .and_then(|l| l.squeeze(0))
            .and_then(|l| l.to_dtype(candle_core::DType::F32))
            .and_then(|l| l.to_vec1::<f32>())
            .map_err(|e| api(format!("forward: {e}")))
    }

    /// Pick the highest-logit token whose detokenized piece keeps the output a
    /// viable prefix of the schema. Tests only the top-`TOP_K` candidates.
    fn pick_constrained(
        &self,
        constraint: &mut Constraint,
        logits: &[f32],
        generated: &[u32],
    ) -> Result<Option<u32>, AiError> {
        let ranked = top_k(logits, TOP_K);
        let prev = self.tokenizer.decode(generated, true).map_err(|e| api(format!("decode: {e}")))?;
        for id in ranked {
            // The piece this token adds = decode(generated+id) minus decode(generated).
            let mut ids = generated.to_vec();
            ids.push(id);
            let whole = self.tokenizer.decode(&ids, true).map_err(|e| api(format!("decode: {e}")))?;
            let Some(piece) = whole.strip_prefix(&prev) else {
                continue; // detokenization wasn't a clean extension — skip this candidate
            };
            if constraint.pick(std::iter::once((id, piece))).is_some() {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
}

fn argmax(logits: &[f32]) -> u32 {
    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i as u32)
        .unwrap_or(0)
}

/// The indices of the `k` highest logits, highest first.
fn top_k(logits: &[f32], k: usize) -> Vec<u32> {
    let k = k.min(logits.len());
    let mut idx: Vec<u32> = (0..logits.len() as u32).collect();
    if k < logits.len() {
        idx.select_nth_unstable_by(k, |&a, &b| logits[b as usize].total_cmp(&logits[a as usize]));
        idx.truncate(k);
    }
    idx.sort_unstable_by(|&a, &b| logits[b as usize].total_cmp(&logits[a as usize]));
    idx
}

fn api(msg: String) -> AiError {
    AiError::Api(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn top_k_returns_highest_first() {
        let logits = [0.1, 0.9, 0.3, 0.7, 0.2];
        assert_eq!(top_k(&logits, 3), vec![1, 3, 2]);
        assert_eq!(argmax(&logits), 1);
    }

    /// End-to-end: load a real Qwen GGUF and generate schema-valid JSON. Ignored
    /// because it needs a model on disk — set the paths and run:
    /// `QUILL_LOCAL_GGUF=…/qwen.gguf QUILL_LOCAL_TOKENIZER=…/tokenizer.json \
    ///  cargo test --features local local_backend -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn local_backend_produces_schema_valid_json() {
        let gguf = std::env::var("QUILL_LOCAL_GGUF").expect("QUILL_LOCAL_GGUF");
        let tok = std::env::var("QUILL_LOCAL_TOKENIZER").expect("QUILL_LOCAL_TOKENIZER");
        let backend = LocalBackend::load(&gguf, &tok).expect("load model");

        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "role": {"enum": ["protagonist", "antagonist", "minor"]},
                "traits": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["name", "role", "traits"]
        });
        let req = Request {
            system: Some("Extract the protagonist from the passage as JSON.".into()),
            cached_context: String::new(),
            user: "Mara Vance had spent thirty winters in the lightsteel mines, her hands scarred, \
                   her temper short, but her loyalty to the miners absolute."
                .into(),
            model: "local-qwen".into(),
            max_tokens: 200,
            json_schema: Some(schema.clone()),
        };

        let resp = backend.complete(&req).expect("completion");
        println!("LOCAL OUTPUT: {}", resp.text);
        // The whole point: the reply parses and matches the schema shape.
        let parsed: serde_json::Value = serde_json::from_str(&resp.text).expect("valid JSON");
        assert!(parsed["name"].is_string());
        assert!(["protagonist", "antagonist", "minor"].contains(&parsed["role"].as_str().unwrap()));
        assert!(parsed["traits"].is_array());
    }
}
