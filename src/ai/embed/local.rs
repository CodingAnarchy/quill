//! The candle text-embedding backend (#831), behind `#[cfg(feature = "local")]`
//! so its heavy deps stay out of a default build — the same gating the generative
//! [`crate::ai::local`] backend uses. A small BERT-family sentence-transformers
//! encoder (`all-MiniLM-L6-v2`) loaded from safetensors, mean-pooled and
//! L2-normalized to one vector per passage. It only ever reads text to vectorize
//! it, so it's inside the AI boundary by construction.
//!
//! Split out of `embed/mod.rs` to keep that module under the ~1,000-line
//! convention; the retrieval core (chunking, ranking, echo pairing) stays in the
//! parent so the hermetic tests exercise it without candle.

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::Tokenizer;

use super::Embedder;
use crate::ai::model_select::{self, EmbedModel};
use crate::ai::AiError;

/// A loaded local embedding model — the encoder, its tokenizer, and the device.
pub struct LocalEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    tag: String,
}

impl LocalEmbedder {
    /// Load an [`EmbedModel`] from the `hf-hub` cache (downloading if absent,
    /// resumable), matching the generative backend's `for_model` path. The
    /// repo carries `model.safetensors`, `config.json`, and `tokenizer.json` —
    /// the sentence-transformers layout candle's `bert` reads.
    pub fn for_model(model: &EmbedModel) -> Result<Self, AiError> {
        let hub = hf_hub::api::sync::ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|e| download_error(model, e))?;
        let repo = hub.model(model.repo.to_string());
        let weights = repo
            .get(model.weights_file)
            .map_err(|e| download_error(model, e))?;
        let config = repo
            .get("config.json")
            .map_err(|e| download_error(model, e))?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .map_err(|e| download_error(model, e))?;
        Self::load(
            weights.to_str().unwrap_or_default(),
            config.to_str().unwrap_or_default(),
            tokenizer_path.to_str().unwrap_or_default(),
            model.id,
        )
    }

    /// Load a BERT embedding model from a safetensors weights file, its
    /// `config.json`, and its `tokenizer.json`. Uses the best device candle
    /// offers (Metal on Apple Silicon, else CPU).
    pub fn load(
        weights_path: &str,
        config_path: &str,
        tokenizer_path: &str,
        tag: &str,
    ) -> Result<Self, AiError> {
        let device = model_select::best_device();
        let config: Config = serde_json::from_str(
            &std::fs::read_to_string(config_path)
                .map_err(|e| internal(format!("read config: {e}")))?,
        )
        .map_err(|e| internal(format!("parse config: {e}")))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| internal(format!("tokenizer: {e}")))?;
        // SAFETY: mmap of a read-only weights file we just fetched; candle's
        // documented load path (the same one the bert example uses).
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)
                .map_err(|e| internal(format!("load weights: {e}")))?
        };
        let model =
            BertModel::load(vb, &config).map_err(|e| internal(format!("build model: {e}")))?;
        Ok(Self {
            model,
            tokenizer,
            device,
            tag: tag.to_string(),
        })
    }

    /// Embed one text into a mean-pooled sentence vector.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>, AiError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| internal(format!("encode: {e}")))?;
        let ids = encoding.get_ids().to_vec();
        let n = ids.len();
        let token_ids = Tensor::new(ids.as_slice(), &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| internal(format!("input tensor: {e}")))?;
        let token_type_ids = token_ids
            .zeros_like()
            .map_err(|e| internal(format!("type ids: {e}")))?;
        let out = self
            .model
            .forward(&token_ids, &token_type_ids, None)
            .map_err(|e| internal(format!("forward: {e}")))?;
        // Mean-pool over the token axis → one sentence vector. `out` is
        // (1, n_tokens, hidden); sum over tokens and divide by the count.
        let pooled = out
            .sum(1)
            .and_then(|t| t / n.max(1) as f64)
            .and_then(|t| t.squeeze(0))
            .and_then(|t| t.to_dtype(candle_core::DType::F32))
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|e| internal(format!("pool: {e}")))?;
        Ok(pooled)
    }
}

impl Embedder for LocalEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn model_tag(&self) -> String {
        self.tag.clone()
    }
}

const fn internal(msg: String) -> AiError {
    AiError::Internal(msg)
}

/// An actionable error when an embedding model can't be fetched (#726), naming
/// the model + repo + file so a genuine failure points at the concrete resource.
fn download_error(model: &EmbedModel, cause: impl std::fmt::Display) -> AiError {
    AiError::NotConfigured(format!(
        "couldn't obtain the embedding model {} (~{} MB, {}/{}) — download it while \
         online, or Ask falls back to sending the whole book. ({cause})",
        model.id, model.size_mb, model.repo, model.weights_file,
    ))
}
