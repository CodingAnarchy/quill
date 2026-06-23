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
use crate::ai::model_select::{self, Accel};
use crate::ai::{AiError, Backend, Request, Response, Usage};

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
    /// Detect, select, fetch, and load the right Qwen for this machine (#85):
    /// probe RAM + accelerator, recommend a variant, download it (and its
    /// tokenizer) via `hf-hub` — cached outside the project, resumable — and load
    /// it. The recommendation is returned so a caller can surface or override it.
    pub fn recommended() -> Result<(Self, model_select::Recommendation), AiError> {
        let (ram_gb, accel) = detect_hardware();
        let rec = model_select::recommend(ram_gb, accel);
        let model = rec.model.ok_or_else(|| AiError::NotConfigured(rec.reasoning.clone()))?;

        let hub = hf_hub::api::sync::Api::new().map_err(|e| api(format!("hf-hub: {e}")))?;
        let gguf = hub
            .model(model.gguf_repo.to_string())
            .get(model.gguf_file)
            .map_err(|e| api(format!("download {}: {e}", model.gguf_file)))?;
        let tokenizer = hub
            .model(model.tokenizer_repo.to_string())
            .get("tokenizer.json")
            .map_err(|e| api(format!("download tokenizer: {e}")))?;

        let backend = Self::load(gguf.to_str().unwrap_or_default(), tokenizer.to_str().unwrap_or_default())?;
        Ok((backend, rec))
    }

    /// Load a quantized Qwen from a `GGUF` file plus its `tokenizer.json`. Uses
    /// Metal when candle was built with it (macOS), else CPU.
    pub fn load(gguf_path: &str, tokenizer_path: &str) -> Result<Self, AiError> {
        let device = model_select::best_device();
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
    /// viable prefix of the schema. Considers the **whole** vocabulary in
    /// descending logit order — the schema can force a low-probability token (a
    /// required key the model didn't want to emit), so a top-K cap would wrongly
    /// stop. Testing halts at the first viable token, which is usually near the
    /// top, so the deep scan only happens at genuinely forced positions.
    fn pick_constrained(
        &self,
        constraint: &mut Constraint,
        logits: &[f32],
        generated: &[u32],
    ) -> Result<Option<u32>, AiError> {
        let ranked = rank_desc(logits);
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

/// Probe the machine for model selection: total RAM (GB, rounded down) and
/// whether candle has a usable GPU (Metal/CUDA) here.
fn detect_hardware() -> (u32, Accel) {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let ram_gb = (sys.total_memory() / 1024 / 1024 / 1024) as u32; // bytes → GiB
    let accel = if Device::new_metal(0).is_ok() || Device::new_cuda(0).is_ok() {
        Accel::Gpu
    } else {
        Accel::Cpu
    };
    (ram_gb, accel)
}

fn argmax(logits: &[f32]) -> u32 {
    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i as u32)
        .unwrap_or(0)
}

/// All vocabulary indices ordered by logit, highest first.
fn rank_desc(logits: &[f32]) -> Vec<u32> {
    let mut idx: Vec<u32> = (0..logits.len() as u32).collect();
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
    fn rank_desc_orders_by_logit() {
        let logits = [0.1, 0.9, 0.3, 0.7, 0.2];
        assert_eq!(rank_desc(&logits), vec![1, 3, 2, 4, 0]);
        assert_eq!(argmax(&logits), 1);
    }

    #[test]
    fn detect_hardware_reports_some_memory() {
        let (ram_gb, _accel) = detect_hardware();
        assert!(ram_gb >= 1, "should detect at least 1 GB of RAM");
    }

    /// End-to-end of the #85 acquisition path: download a model via `hf-hub` and
    /// generate schema-valid JSON. Forces the smallest model (~400 MB) so it's a
    /// modest download. Ignored (needs network + disk); run with:
    /// `cargo test --features local auto_download -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn auto_download_then_generate() {
        let m = &model_select::MODELS[0]; // 0.5B — keep the download small
        let hub = hf_hub::api::sync::Api::new().expect("hf-hub");
        let gguf = hub.model(m.gguf_repo.into()).get(m.gguf_file).expect("download gguf");
        let tok = hub.model(m.tokenizer_repo.into()).get("tokenizer.json").expect("download tokenizer");
        let backend =
            LocalBackend::load(gguf.to_str().unwrap(), tok.to_str().unwrap()).expect("load");

        let schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "kind": {"enum": ["person", "place"]}},
            "required": ["name", "kind"]
        });
        let req = Request {
            system: Some("Return JSON describing the subject.".into()),
            cached_context: String::new(),
            user: "The city of Aldermoor.".into(),
            model: "auto".into(),
            max_tokens: 256,
            json_schema: Some(schema),
        };
        let resp = backend.complete(&req).expect("completion");
        println!("AUTO OUTPUT: {}", resp.text);
        let parsed: serde_json::Value = serde_json::from_str(&resp.text).expect("valid JSON");
        assert!(parsed["name"].is_string());
        assert!(["person", "place"].contains(&parsed["kind"].as_str().unwrap()));
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
