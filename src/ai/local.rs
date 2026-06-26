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
use crate::ai::tool_loop::{self, Converse, Turn};
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
        let backend = Self::for_model(model)?;
        Ok((backend, rec))
    }

    /// Download (cached, resumable via `hf-hub`) and load a specific Qwen — the
    /// model the router picked from the author's configured set (#377). Shared
    /// with [`recommended`], which chooses the model from the hardware first.
    pub fn for_model(model: &model_select::Model) -> Result<Self, AiError> {
        let hub = hf_hub::api::sync::Api::new().map_err(|e| internal(format!("hf-hub: {e}")))?;
        let gguf = hub
            .model(model.gguf_repo.to_string())
            .get(model.gguf_file)
            .map_err(|e| internal(format!("download {}: {e}", model.gguf_file)))?;
        let tokenizer = hub
            .model(model.tokenizer_repo.to_string())
            .get("tokenizer.json")
            .map_err(|e| internal(format!("download tokenizer: {e}")))?;
        Self::load(gguf.to_str().unwrap_or_default(), tokenizer.to_str().unwrap_or_default())
    }

    /// Load a quantized Qwen from a `GGUF` file plus its `tokenizer.json`. Uses
    /// Metal when candle was built with it (macOS), else CPU.
    pub fn load(gguf_path: &str, tokenizer_path: &str) -> Result<Self, AiError> {
        let device = model_select::best_device();
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| internal(format!("tokenizer: {e}")))?;
        let mut file = std::fs::File::open(gguf_path).map_err(|e| internal(format!("open gguf: {e}")))?;
        let content = gguf_file::Content::read(&mut file).map_err(|e| internal(format!("read gguf: {e}")))?;
        let model = Qwen2::from_gguf(content, &mut file, &device).map_err(|e| internal(format!("load gguf: {e}")))?;
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
            .map_err(|e| internal(format!("encode: {e}")))?
            .get_ids()
            .to_vec();

        let mut model = self.model.lock().map_err(|_| internal("model lock poisoned".into()))?;
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
            None => self.tokenizer.decode(&generated, true).map_err(|e| internal(format!("decode: {e}")))?,
        };
        Ok(Response { text, usage: Usage { output_tokens: generated.len() as u32, ..Usage::default() } })
    }

    fn as_converse(&self) -> Option<&dyn Converse> {
        Some(self)
    }
}

/// Assistant tool-use on the local backend (#310). The local Qwen has no native
/// tool-calling, so each turn is one *constrained* choice — a JSON `{action,
/// message}` over the registry-derived tool names plus `respond` — which
/// constrained decoding ([`crate::ai::constrain`]) guarantees is valid and
/// on-list. So the same [`tool_loop`] drives the assistant offline, and the
/// prose-boundary guardrail (#308) holds identically: the model can only choose a
/// vetted tool name (none writes prose), and the execution-layer allowlist
/// re-checks it. Tool-use quality scales with the chosen model; whole-manuscript
/// context needs an extended-context model (#378).
impl Converse for LocalBackend {
    fn respond(&self, body: &serde_json::Value) -> Result<Turn, AiError> {
        let names = tool_loop::tool_names(body);
        let messages = body["messages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let transcript = tool_loop::flatten_conversation(messages);
        let tool_list = names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
        let user = format!(
            "{transcript}\nDecide the next step and reply as JSON {{\"action\", \"message\"}}. \
             Set action to \"{}\" and put your reply in message to answer the author, or set \
             action to one of [{tool_list}] to run that tool. You cannot write or edit prose.",
            tool_loop::RESPOND,
        );
        let req = Request {
            system: body["system"].as_str().map(str::to_string),
            cached_context: String::new(),
            user,
            model: body["model"].as_str().unwrap_or("auto").to_string(),
            max_tokens: body["max_tokens"].as_u64().unwrap_or(1024) as u32,
            json_schema: Some(tool_loop::choice_schema(&names)),
        };
        let resp = self.complete(&req)?;
        tool_loop::parse_choice_turn(&resp.text)
    }
}

impl LocalBackend {
    /// One forward pass; returns the last position's logits as `f32`.
    fn step(&self, model: &mut Qwen2, tokens: &[u32], pos: usize) -> Result<Vec<f32>, AiError> {
        let input = Tensor::new(tokens, &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| internal(format!("input tensor: {e}")))?;
        model
            .forward(&input, pos)
            .and_then(|l| l.squeeze(0))
            .and_then(|l| l.to_dtype(candle_core::DType::F32))
            .and_then(|l| l.to_vec1::<f32>())
            .map_err(|e| internal(format!("forward: {e}")))
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
        let prev = self.tokenizer.decode(generated, true).map_err(|e| internal(format!("decode: {e}")))?;
        for id in ranked {
            // The piece this token adds = decode(generated+id) minus decode(generated).
            let mut ids = generated.to_vec();
            ids.push(id);
            let whole = self.tokenizer.decode(&ids, true).map_err(|e| internal(format!("decode: {e}")))?;
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

fn internal(msg: String) -> AiError {
    AiError::Internal(msg)
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

    /// Assistant tool-use on the local backend (#310): given a loop body with
    /// tools, `respond` returns a parsed [`Turn`] whose tool call (if any) is one
    /// of the offered tools — constrained decoding can't emit an off-list action.
    /// Ignored (needs a model on disk); run with:
    /// `QUILL_LOCAL_GGUF=… QUILL_LOCAL_TOKENIZER=… \
    ///  cargo test --features local local_assistant -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn local_assistant_tool_use_picks_a_valid_action() {
        let gguf = std::env::var("QUILL_LOCAL_GGUF").expect("QUILL_LOCAL_GGUF");
        let tok = std::env::var("QUILL_LOCAL_TOKENIZER").expect("QUILL_LOCAL_TOKENIZER");
        let backend = LocalBackend::load(&gguf, &tok).expect("load model");

        let tools = vec![
            json!({ "name": "save", "description": "Save the chapter", "input_schema": { "type": "object", "properties": {} } }),
            json!({ "name": "report:progress", "description": "Open the progress report", "input_schema": { "type": "object", "properties": {} } }),
        ];
        let body = tool_loop::build_body(
            "auto",
            Some("You are the authorial assistant. You cannot write or edit prose."),
            &[json!({ "role": "user", "content": [{ "type": "text", "text": "Save my work." }] })],
            &tools,
            256,
        );

        let turn = backend.respond(&body).expect("a parsed turn");
        match turn {
            Turn::ToolCalls(calls) => {
                println!("LOCAL ASSISTANT CALLED: {:?}", calls.iter().map(|c| &c.name).collect::<Vec<_>>());
                for c in &calls {
                    assert!(["save", "report:progress"].contains(&c.name.as_str()), "off-list: {}", c.name);
                }
            }
            Turn::Final(text) => println!("LOCAL ASSISTANT REPLIED: {text}"),
        }
    }
}
