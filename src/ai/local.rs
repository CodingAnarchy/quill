//! Local-model backend (#124): runs a quantized Qwen3 dense GGUF in-process with
//! candle (Metal on Apple Silicon, CPU elsewhere), no daemon. Behind the `local`
//! cargo feature so its heavy deps stay out of default builds.
//!
//! The on-device family is **Qwen3 dense** (#789), loaded through candle 0.11's
//! `quantized_qwen3` path (drop-in `from_gguf`/`forward`, verified on Metal in
//! #735). Qwen3 is a hybrid *reasoning* model: by default it prefixes its answer
//! with a `<think>…</think>` block. That's fine for the constrained path (the
//! schema rejects `<think>` from the first token, so it can never appear), but on
//! the **unconstrained** path it would surface raw chain-of-thought as prose — so
//! we disable it with Qwen3's `/no_think` soft switch in the prompt and strip any
//! residual `<think>…</think>` span before returning ([`strip_think`]).
//!
//! Structured output is enforced by [`crate::ai::constrain`]: when a request
//! carries a `json_schema`, the decode loop only ever accepts a token that keeps
//! the output a viable prefix of a schema-valid value, so the reply is guaranteed
//! parseable — the same guarantee the Claude backend gets from the API. Without a
//! schema (e.g. Q&A) it decodes greedily.
//!
//! The whole module is `local`-gated and every item is reachable in that build
//! (the backend's own paths plus the `#[ignore]`d integration tests), so it needs
//! no `dead_code` allowance (#406).

use std::sync::Mutex;

use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::models::quantized_qwen3::ModelWeights as Qwen3;
use tokenizers::Tokenizer;

use crate::ai::constrain::Constraint;
use crate::ai::model_select::{self, Accel, Hardware};
use crate::ai::tool_loop::{self, Converse, Turn};
use crate::ai::{AiError, Backend, Request, Response, Usage};

/// Qwen end-of-turn / end-of-text tokens (stop signals for unconstrained decode).
const EOS_TOKENS: [u32; 2] = [151645, 151643]; // <|im_end|>, <|endoftext|>

/// A loaded local model. `forward` mutates the model's KV cache, so it sits behind
/// a `Mutex` to satisfy the `&self` [`Backend`] trait.
pub struct LocalBackend {
    model: Mutex<Qwen3>,
    tokenizer: Tokenizer,
    device: Device,
}

impl LocalBackend {
    /// Detect, select, fetch, and load the right Qwen for this machine (#85):
    /// probe RAM + accelerator, recommend a variant, download it (and its
    /// tokenizer) via `hf-hub` — cached outside the project, resumable — and load
    /// it. The recommendation is returned so a caller can surface or override it.
    pub fn recommended() -> Result<(Self, model_select::Recommendation), AiError> {
        let rec = model_select::recommend(detect_hardware());
        let model = rec
            .model
            .ok_or_else(|| AiError::NotConfigured(rec.reasoning.clone()))?;
        let backend = Self::for_model(model)?;
        Ok((backend, rec))
    }

    /// The Qwen this machine would load when no model set is configured — the
    /// hardware recommendation, resolved *without downloading anything* (#725). The
    /// router uses it to ask "is a usable local model actually on disk?" before it
    /// routes a pass on-device, so an AI pass never triggers an implicit fetch.
    /// `None` when the machine is too small for even the smallest model.
    pub fn recommended_model() -> Option<&'static model_select::Model> {
        model_select::recommend(detect_hardware()).model
    }

    /// Download (cached, resumable via `hf-hub`) and load a specific Qwen — the
    /// model the router picked from the author's configured set (#377). Shared
    /// with [`recommended`], which chooses the model from the hardware first.
    pub fn for_model(model: &model_select::Model) -> Result<Self, AiError> {
        let hub = build_hub().map_err(|e| download_error(model, e))?;
        let gguf = hub
            .model(model.gguf_repo.to_string())
            .get(model.gguf_file)
            .map_err(|e| download_error(model, e))?;
        let tokenizer = hub
            .model(model.tokenizer_repo.to_string())
            .get("tokenizer.json")
            .map_err(|e| download_error(model, e))?;
        Self::load(
            gguf.to_str().unwrap_or_default(),
            tokenizer.to_str().unwrap_or_default(),
        )
    }

    /// Load a quantized Qwen from a `GGUF` file plus its `tokenizer.json`. Uses
    /// Metal when candle was built with it (macOS), else CPU.
    pub fn load(gguf_path: &str, tokenizer_path: &str) -> Result<Self, AiError> {
        let device = model_select::best_device();
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| internal(format!("tokenizer: {e}")))?;
        let mut file =
            std::fs::File::open(gguf_path).map_err(|e| internal(format!("open gguf: {e}")))?;
        let content =
            gguf_file::Content::read(&mut file).map_err(|e| internal(format!("read gguf: {e}")))?;
        let model = Qwen3::from_gguf(content, &mut file, &device)
            .map_err(|e| internal(format!("load gguf: {e}")))?;
        Ok(Self {
            model: Mutex::new(model),
            tokenizer,
            device,
        })
    }

    /// Build the Qwen3 chat-template prompt for a request (manuscript folded into
    /// the user turn, since the local backend has no separate cached block).
    ///
    /// Qwen3 is a hybrid reasoning model that emits a `<think>…</think>` block by
    /// default. On the **unconstrained** path (`no_think == true`, i.e. no schema
    /// is active) we append Qwen3's `/no_think` soft switch to the system prompt so
    /// the model skips reasoning and answers directly — the documented way to turn
    /// thinking off without a custom chat template. The constrained path leaves the
    /// prompt untouched: the schema already rejects `<think>` from the first token,
    /// so the reasoning block can never appear, and we don't want to perturb the
    /// structured decode.
    fn prompt(req: &Request, no_think: bool) -> String {
        let base = req
            .system
            .as_deref()
            .unwrap_or("You are a helpful assistant.");
        let system = if no_think {
            format!("{base}\n/no_think")
        } else {
            base.to_string()
        };
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

/// Strip a leading Qwen3 `<think>…</think>` reasoning block (and any surrounding
/// whitespace) from an unconstrained answer (#789). `/no_think` normally keeps the
/// model from emitting one at all — Qwen3 then produces an empty `<think></think>`
/// or none — but this is the belt-and-suspenders that guarantees no chain-of-thought
/// ever reaches the author as prose, whatever the model does. Only ever applied to
/// the unconstrained text path; the constrained path can't produce a `<think>` tag
/// (the schema rejects it) and its accumulated text is returned verbatim.
fn strip_think(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return rest[end + "</think>".len()..].trim_start().to_string();
        }
        // An unterminated <think> means the whole reply was reasoning — drop it.
        return String::new();
    }
    text.trim_start().to_string()
}

impl Backend for LocalBackend {
    fn complete(&self, req: &Request) -> Result<Response, AiError> {
        // Unconstrained (no schema) → disable Qwen3's <think> reasoning (#789).
        let no_think = req.json_schema.is_none();
        let prompt = Self::prompt(req, no_think);
        let prompt_ids = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| internal(format!("encode: {e}")))?
            .get_ids()
            .to_vec();

        let mut model = self
            .model
            .lock()
            .map_err(|_| internal("model lock poisoned".into()))?;
        let mut constraint = req.json_schema.as_ref().map(Constraint::new);

        // Each request is independent, but candle's Qwen3 KV cache is stateful and
        // persists across `forward` calls — so it must be reset here, or the prompt
        // pass (which starts at index_pos 0) would attend over stale keys from the
        // previous completion and candle would fault on the mismatched cache length
        // (#789). The old single-shot qwen2 test never issued a second completion on
        // one backend, so this latent reuse bug only surfaced with the family switch.
        model.clear_kv_cache();

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
            // The constraint accumulated the exact accepted text already; it can't
            // contain a <think> tag (the schema rejects it), so return it verbatim.
            Some(c) => c.text().to_string(),
            // Unconstrained: strip any residual Qwen3 <think>…</think> reasoning so
            // the author never sees chain-of-thought as prose (#789).
            None => strip_think(
                &self
                    .tokenizer
                    .decode(&generated, true)
                    .map_err(|e| internal(format!("decode: {e}")))?,
            ),
        };
        Ok(Response {
            text,
            usage: Usage {
                output_tokens: generated.len() as u32,
                ..Usage::default()
            },
        })
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
/// re-checks it. Tool-use quality scales with the chosen model; a pass only routes
/// on-device when it fits the model's window (Qwen3's native 40960, #378/#791) —
/// a bigger pass than that goes to the cloud, since Qwen3 has no ~1M-context GGUF.
impl Converse for LocalBackend {
    fn respond(&self, body: &serde_json::Value) -> Result<Turn, AiError> {
        let names = tool_loop::tool_names(body);
        let messages = body["messages"]
            .as_array()
            .map_or([].as_slice(), Vec::as_slice);
        let transcript = tool_loop::flatten_conversation(messages);
        let tool_list = names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let user = format!(
            "{transcript}\nDecide the next step and reply as JSON {{\"action\", \"argument\", \"message\"}}. \
             Set action to \"{}\" and put your reply in message to answer the author, or set action to \
             one of [{tool_list}] to run that tool. When a tool needs input (e.g. a question for \
             \"ai:ask\"), put it in argument. You cannot write or edit prose.",
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
    fn step(&self, model: &mut Qwen3, tokens: &[u32], pos: usize) -> Result<Vec<f32>, AiError> {
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
        let prev = self
            .tokenizer
            .decode(generated, true)
            .map_err(|e| internal(format!("decode: {e}")))?;
        for id in ranked {
            // The piece this token adds = decode(generated+id) minus decode(generated).
            let mut ids = generated.to_vec();
            ids.push(id);
            let whole = self
                .tokenizer
                .decode(&ids, true)
                .map_err(|e| internal(format!("decode: {e}")))?;
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

/// GiB in bytes, for turning byte counts into whole GB.
const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

/// Probe the machine for memory-aware model selection (#728): total system RAM,
/// which accelerator a model will run on, and — crucially — the memory pool that
/// accelerator actually draws from, since that (not total RAM) is what bounds the
/// pick. On Apple Silicon that's Metal's `recommendedMaxWorkingSetSize` (unified
/// memory, capped below total RAM); on a discrete GPU it's VRAM. `Device::new_*`
/// only succeeds when candle was built with that backend, so a build without the
/// CUDA backend never reports `Cuda` — matching what it can load.
fn detect_hardware() -> Hardware {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let total_ram_gb = (sys.total_memory() / BYTES_PER_GB) as u32;

    // Prefer Metal (Apple Silicon), then CUDA — the same order `best_device` loads
    // in — and read each accelerator's own memory pool where we can.
    if let Ok(dev) = Device::new_metal(0) {
        return Hardware {
            accel: Accel::Metal,
            total_ram_gb,
            device_mem_gb: metal_working_set_gb(&dev),
        };
    }
    if let Ok(dev) = Device::new_cuda(0) {
        return Hardware {
            accel: Accel::Cuda,
            total_ram_gb,
            device_mem_gb: cuda_vram_gb(&dev),
        };
    }
    Hardware {
        accel: Accel::Cpu,
        total_ram_gb,
        device_mem_gb: None,
    }
}

/// Metal's usable working-set budget in GB — Apple's `recommendedMaxWorkingSetSize`,
/// the largest single allocation Metal advises, which on Apple Silicon is a
/// fraction (~⅔–¾) of unified RAM. Read straight off the underlying `metal::Device`
/// candle exposes. `None` if unavailable, so the picker falls back to a fraction of
/// total RAM.
#[cfg(target_os = "macos")]
fn metal_working_set_gb(dev: &Device) -> Option<f64> {
    let Device::Metal(m) = dev else { return None };
    let bytes = m.metal_device().recommended_max_working_set_size();
    (bytes > 0).then(|| bytes as f64 / BYTES_PER_GB as f64)
}

/// Off macOS candle's Metal backend can't exist, so `Device::new_metal` never
/// succeeds and this is never reached; provided so the call site compiles.
#[cfg(not(target_os = "macos"))]
fn metal_working_set_gb(_dev: &Device) -> Option<f64> {
    None
}

/// VRAM in GB for a discrete CUDA GPU. Quill never builds candle with its CUDA
/// backend, so `Device::new_cuda` only ever succeeds on a candle configured with
/// it; querying VRAM needs candle's re-exported cudarc, which is present only in
/// that build. Here we return `None`, so the picker uses a 0 VRAM budget (→ Claude)
/// — the safe answer for a CUDA box we can't size. A candle-CUDA build would swap
/// this for a `cudarc::driver::result::mem_get_info()` probe of total VRAM.
const fn cuda_vram_gb(_dev: &Device) -> Option<f64> {
    None
}

fn argmax(logits: &[f32]) -> u32 {
    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i as u32)
}

/// All vocabulary indices ordered by logit, highest first.
fn rank_desc(logits: &[f32]) -> Vec<u32> {
    let mut idx: Vec<u32> = (0..logits.len() as u32).collect();
    idx.sort_unstable_by(|&a, &b| logits[b as usize].total_cmp(&logits[a as usize]));
    idx
}

const fn internal(msg: String) -> AiError {
    AiError::Internal(msg)
}

/// Build the `hf-hub` API with its `indicatif` progress bar disabled (#727).
/// Quill owns the crossterm alternate screen, so any raw progress the library
/// writes to stderr lands on top of the TUI and corrupts it; download progress
/// is surfaced through Quill's own UI instead. Matches `Api::new()`'s cache and
/// token defaults (`ApiBuilder::new()` reads the token from the default cache),
/// changing only the progress flag.
fn build_hub() -> Result<hf_hub::api::sync::Api, hf_hub::api::sync::ApiError> {
    hf_hub::api::sync::ApiBuilder::new()
        .with_progress(false)
        .build()
}

/// An actionable error when a local model can't be fetched from HuggingFace
/// (#725): name the model and how to fix it (download it deliberately, or route
/// to Claude), rather than surfacing a bare `hf-hub` string. The underlying cause
/// is appended so a genuine failure (offline, disk full) is still diagnosable.
fn download_error(model: &model_select::Model, cause: impl std::fmt::Display) -> AiError {
    AiError::NotConfigured(format!(
        "couldn't obtain the local model Qwen3-{} (~{:.1} GB) — download it from \
         \"Local AI models\" (Ctrl-K) while online, or set `backend = \"claude\"` / adjust \
         `[ai] routing`. ({cause})",
        model.id(),
        model.size_gb,
    ))
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
        let hw = detect_hardware();
        assert!(hw.total_ram_gb >= 1, "should detect at least 1 GB of RAM");
        // A probed GPU pool, when present, is positive; CPU carries none.
        if let Some(mem) = hw.device_mem_gb {
            assert!(mem > 0.0, "probed device memory should be positive");
        }
    }

    #[test]
    fn strip_think_removes_qwen3_reasoning_from_unconstrained_prose() {
        // A leading <think>…</think> block is dropped, the answer kept (#789).
        assert_eq!(
            strip_think("<think>\nlet me reason...\n</think>\n\nAldermoor is a port."),
            "Aldermoor is a port."
        );
        // The empty block Qwen3 emits under /no_think is stripped to just the answer.
        assert_eq!(
            strip_think("<think>\n\n</think>\n\nA quiet town."),
            "A quiet town."
        );
        // No reasoning block → returned as-is (only leading whitespace trimmed).
        assert_eq!(strip_think("  Just an answer."), "Just an answer.");
        // An unterminated block means the whole reply was reasoning → nothing to show.
        assert_eq!(strip_think("<think>\nstill thinking, cut off"), "");
    }

    /// End-to-end of the #85 acquisition path: download a Qwen3 dense model via
    /// `hf-hub` and generate schema-valid JSON, then confirm the unconstrained path
    /// answers *without* leaking `<think>` reasoning (#789). Forces the smallest
    /// model (Qwen3-0.6B Q8_0, ~640 MB) so it's a modest download. Ignored (needs
    /// network + disk + Metal); run with:
    /// `cargo test --features local auto_download -- --ignored --nocapture`
    #[test]
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
    fn auto_download_then_generate() {
        let m = &model_select::MODELS[0]; // 0.6B Q8_0 — keep the download small
        let hub = build_hub().expect("hf-hub");
        let gguf = hub
            .model(m.gguf_repo.into())
            .get(m.gguf_file)
            .expect("download gguf");
        let tok = hub
            .model(m.tokenizer_repo.into())
            .get("tokenizer.json")
            .expect("download tokenizer");
        let backend =
            LocalBackend::load(gguf.to_str().unwrap(), tok.to_str().unwrap()).expect("load");

        // 1) The CONSTRAINED-JSON path must still yield schema-valid JSON — the
        //    constraint rejects <think> from the first token, so it's unaffected.
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
        println!("AUTO CONSTRAINED OUTPUT: {}", resp.text);
        let parsed: serde_json::Value = serde_json::from_str(&resp.text).expect("valid JSON");
        assert!(parsed["name"].is_string());
        assert!(["person", "place"].contains(&parsed["kind"].as_str().unwrap()));
        assert!(
            !resp.text.contains("<think>"),
            "the constrained path never emits a reasoning tag: {}",
            resp.text
        );

        // 2) The UNCONSTRAINED path must answer without any <think> leaking through
        //    (/no_think + strip_think). We don't judge the prose, only that no raw
        //    chain-of-thought tag survives.
        let qa = Request {
            system: Some("Answer in one short sentence.".into()),
            cached_context: String::new(),
            user: "What is the capital of France?".into(),
            model: "auto".into(),
            max_tokens: 256,
            json_schema: None,
        };
        let ans = backend.complete(&qa).expect("completion");
        println!("AUTO UNCONSTRAINED OUTPUT: {}", ans.text);
        assert!(
            !ans.text.contains("<think>") && !ans.text.contains("</think>"),
            "no reasoning block should reach the author: {}",
            ans.text
        );
    }

    /// End-to-end: load a real Qwen GGUF and generate schema-valid JSON. Ignored
    /// because it needs a model on disk — set the paths and run:
    /// `QUILL_LOCAL_GGUF=…/qwen.gguf QUILL_LOCAL_TOKENIZER=…/tokenizer.json \
    ///  cargo test --features local local_backend -- --ignored --nocapture`
    #[test]
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
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
            user:
                "Mara Vance had spent thirty winters in the lightsteel mines, her hands scarred, \
                   her temper short, but her loyalty to the miners absolute."
                    .into(),
            model: "local-qwen".into(),
            max_tokens: 200,
            json_schema: Some(schema),
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
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
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
                println!(
                    "LOCAL ASSISTANT CALLED: {:?}",
                    calls.iter().map(|c| &c.name).collect::<Vec<_>>()
                );
                for c in &calls {
                    assert!(
                        ["save", "report:progress"].contains(&c.name.as_str()),
                        "off-list: {}",
                        c.name
                    );
                }
            }
            Turn::Final(text) => println!("LOCAL ASSISTANT REPLIED: {text}"),
        }
    }
}
