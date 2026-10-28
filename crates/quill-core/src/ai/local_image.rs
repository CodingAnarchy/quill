//! Local image backend (#240): a Stable-Diffusion v1.5 text-to-image pipeline run
//! in-process with candle (Metal on Apple Silicon, CPU/CUDA elsewhere), behind the
//! `local` cargo feature so its heavy deps stay out of default builds. No key, no
//! per-image cost, nothing leaves the machine.
//!
//! It implements the same [`ImageBackend`] trait as the remote fal.ai backend, so
//! the portrait (#238) and scene (#239) passes run through it unchanged. The model
//! weights download once via `hf-hub` (cached outside the project); a real run
//! needs them on disk and a capable machine, so the end-to-end path is covered by
//! the `#[ignore]`d test below rather than in normal CI.

use candle_core::{DType, Device, Tensor};
use candle_transformers::models::stable_diffusion::clip::ClipTextTransformer;
use candle_transformers::models::stable_diffusion::unet_2d::UNet2DConditionModel;
use candle_transformers::models::stable_diffusion::vae::AutoEncoderKL;
use candle_transformers::models::stable_diffusion::{self, StableDiffusionConfig};
use hf_hub::api::sync::ApiBuilder;
use image::ImageEncoder;
use tokenizers::Tokenizer;

use crate::ai::image::{GeneratedImage, ImageBackend, ImageRequest};
use crate::ai::AiError;

/// The CLIP tokenizer repo (shared across SD versions).
const TOKENIZER_REPO: &str = "openai/clip-vit-base-patch32";
/// The Stable-Diffusion v1.5 weights repo (diffusers `.safetensors` layout). The
/// author can point at another v1.5-compatible repo; the files below are standard.
const SD_REPO: &str = "stable-diffusion-v1-5/stable-diffusion-v1-5";
const CLIP_WEIGHTS: &str = "text_encoder/model.safetensors";
const UNET_WEIGHTS: &str = "unet/diffusion_pytorch_model.safetensors";
const VAE_WEIGHTS: &str = "vae/diffusion_pytorch_model.safetensors";

/// Denoising steps, classifier-free-guidance scale, and the VAE latent scaling —
/// the standard SD v1.5 values.
const STEPS: usize = 28;
const GUIDANCE: f64 = 7.5;
const VAE_SCALE: f64 = 0.18215;

/// A loaded local Stable-Diffusion pipeline. Every component is used through a
/// shared reference (`&self`), so no interior mutability is needed (the scheduler
/// is rebuilt per request).
pub struct LocalImageBackend {
    device: Device,
    config: StableDiffusionConfig,
    clip: ClipTextTransformer,
    unet: UNet2DConditionModel,
    vae: AutoEncoderKL,
    tokenizer: Tokenizer,
    pad_id: u32,
}

impl LocalImageBackend {
    /// Download (cached) and load the SD v1.5 weights for this machine.
    pub fn load() -> Result<Self, AiError> {
        let device = crate::ai::model_select::best_device();
        // SD v1.5 at 512² with standard steps/guidance. Hardware-aware model and
        // resolution selection is deferred to the local-model epic (#83/#85).
        let config = StableDiffusionConfig::v1_5(None, Some(512), Some(512));

        // Disable hf-hub's indicatif progress bar (#727): Quill owns the
        // alternate screen, so the library's raw stderr writes would corrupt the
        // TUI. Progress is surfaced through Quill's own UI instead.
        let hub = ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|e| internal(format!("hf-hub: {e}")))?;
        let fetch = |repo: &str, file: &str| -> Result<std::path::PathBuf, AiError> {
            hub.model(repo.to_string())
                .get(file)
                .map_err(|e| internal(format!("download {file}: {e}")))
        };
        let tokenizer_path = fetch(TOKENIZER_REPO, "tokenizer.json")?;
        let clip_path = fetch(SD_REPO, CLIP_WEIGHTS)?;
        let unet_path = fetch(SD_REPO, UNET_WEIGHTS)?;
        let vae_path = fetch(SD_REPO, VAE_WEIGHTS)?;

        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| internal(e.to_string()))?;
        let pad_id = pad_token(&tokenizer, &config)?;
        let clip =
            stable_diffusion::build_clip_transformer(&config.clip, clip_path, &device, DType::F32)
                .map_err(weights)?;
        let unet = config
            .build_unet(unet_path, &device, 4, false, DType::F32)
            .map_err(weights)?;
        let vae = config
            .build_vae(vae_path, &device, DType::F32)
            .map_err(weights)?;

        Ok(Self {
            device,
            config,
            clip,
            unet,
            vae,
            tokenizer,
            pad_id,
        })
    }

    /// The conditioned + unconditioned CLIP embeddings, stacked for classifier-free
    /// guidance: row 0 unconditioned (`""`), row 1 the prompt.
    fn text_embeddings(&self, prompt: &str) -> Result<Tensor, AiError> {
        let uncond = self
            .clip
            .forward_with_mask(&self.tokens("")?, usize::MAX)
            .map_err(weights)?;
        let cond = self
            .clip
            .forward_with_mask(&self.tokens(prompt)?, usize::MAX)
            .map_err(weights)?;
        Tensor::cat(&[&uncond, &cond], 0).map_err(weights)
    }

    /// Tokenize `text` to a `(1, max_position_embeddings)` id tensor, padded.
    fn tokens(&self, text: &str) -> Result<Tensor, AiError> {
        let mut ids = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| internal(e.to_string()))?
            .get_ids()
            .to_vec();
        ids.resize(self.config.clip.max_position_embeddings, self.pad_id);
        Tensor::new(ids.as_slice(), &self.device)
            .map_err(weights)?
            .unsqueeze(0)
            .map_err(weights)
    }
}

impl ImageBackend for LocalImageBackend {
    fn generate(&self, req: &ImageRequest) -> Result<GeneratedImage, AiError> {
        let text = self.text_embeddings(&req.prompt)?;
        let mut scheduler = self.config.build_scheduler(STEPS).map_err(weights)?;
        let (h, w) = (self.config.height, self.config.width);

        let mut latents = Tensor::randn(0f32, 1f32, (1, 4, h / 8, w / 8), &self.device)
            .map_err(weights)?
            .affine(scheduler.init_noise_sigma(), 0.0)
            .map_err(weights)?;

        for &t in scheduler.timesteps().to_vec().iter() {
            // Classifier-free guidance: run the conditioned and unconditioned
            // latents together, then push away from the unconditioned prediction.
            let input = Tensor::cat(&[&latents, &latents], 0).map_err(weights)?;
            let input = scheduler.scale_model_input(input, t).map_err(weights)?;
            let noise = self
                .unet
                .forward(&input, t as f64, &text)
                .map_err(weights)?;
            let chunks = noise.chunk(2, 0).map_err(weights)?;
            let (uncond, cond) = (&chunks[0], &chunks[1]);
            let guided = uncond
                .add(
                    &cond
                        .sub(uncond)
                        .map_err(weights)?
                        .affine(GUIDANCE, 0.0)
                        .map_err(weights)?,
                )
                .map_err(weights)?;
            latents = scheduler.step(&guided, t, &latents).map_err(weights)?;
        }

        // Decode the latents to an image and normalize [-1, 1] → [0, 255] u8.
        let image = self
            .vae
            .decode(&latents.affine(1.0 / VAE_SCALE, 0.0).map_err(weights)?)
            .map_err(weights)?;
        let image = image
            .affine(0.5, 0.5)
            .map_err(weights)?
            .clamp(0f32, 1f32)
            .map_err(weights)?
            .affine(255.0, 0.0)
            .map_err(weights)?
            .to_dtype(DType::U8)
            .map_err(weights)?
            .squeeze(0)
            .map_err(weights)?;

        Ok(GeneratedImage {
            bytes: encode_png(&image)?,
            ext: "png".into(),
        })
    }
}

/// The padding token id: the CLIP config's `pad_with`, else end-of-text.
fn pad_token(tokenizer: &Tokenizer, config: &StableDiffusionConfig) -> Result<u32, AiError> {
    let vocab = tokenizer.get_vocab(true);
    let token = config.clip.pad_with.as_deref().unwrap_or("<|endoftext|>");
    vocab
        .get(token)
        .copied()
        .ok_or_else(|| internal(format!("tokenizer has no '{token}' token")))
}

/// Encode a `(3, H, W)` u8 tensor to PNG bytes.
fn encode_png(image: &Tensor) -> Result<Vec<u8>, AiError> {
    let (_c, h, w) = image.dims3().map_err(weights)?;
    let pixels = image
        .permute((1, 2, 0))
        .map_err(weights)?
        .flatten_all()
        .map_err(weights)?;
    let pixels = pixels.to_vec1::<u8>().map_err(weights)?;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .map_err(|e| internal(e.to_string()))?;
    Ok(bytes)
}

const fn internal(msg: String) -> AiError {
    AiError::Internal(msg)
}

// by-value to serve as a `.map_err(weights)` fn argument at ~30 call sites
#[allow(clippy::needless_pass_by_value)]
fn weights(e: candle_core::Error) -> AiError {
    AiError::Internal(format!("local image: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end generation — needs the weights on disk and a capable machine, so
    /// it's ignored by default (run with `cargo test --features local -- --ignored`).
    #[test]
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
    fn generates_an_image() {
        let backend = LocalImageBackend::load().expect("load SD weights");
        let img = backend
            .generate(&ImageRequest {
                prompt: "a lighthouse at dawn".into(),
                model: String::new(),
            })
            .expect("generate");
        assert_eq!(img.ext, "png");
        assert!(!img.bytes.is_empty());
    }
}
