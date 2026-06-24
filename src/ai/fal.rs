//! The fal.ai image backend (#238): generates an image via fal.ai's synchronous
//! REST endpoint over HTTPS with `ureq` (runs on a worker thread), then downloads
//! the returned image bytes. Key-gated; the model travels on each request. See
//! `docs/ai-features.md`.

use serde_json::{json, Value};

use super::http;
use super::image::{self, GeneratedImage, ImageBackend, ImageRequest};
use super::{is_ok, AiError};

/// fal.ai's synchronous run endpoint: `POST {BASE}/{model}` with the inputs,
/// returns the result (including the image URLs) directly.
const BASE: &str = "https://fal.run";

/// Generated images can be a few MB; lift `ureq`'s default read cap for the
/// download.
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// The fal.ai backend. Holds only the API key; the model travels on each request.
pub struct FalBackend {
    key: String,
}

impl FalBackend {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl ImageBackend for FalBackend {
    fn generate(&self, req: &ImageRequest) -> Result<GeneratedImage, AiError> {
        let (status, value) = self.post(&run_url(&req.model), &body(req))?;
        if !is_ok(status) {
            return Err(api_error(status, &value));
        }
        let (url, content_type) = first_image(&value)?;
        let bytes = self.download(&url)?;
        Ok(GeneratedImage { bytes, ext: image::extension_for(&content_type).to_string() })
    }
}

impl FalBackend {
    /// POST `body` to `url` with the fal auth header; returns the HTTP status and
    /// the parsed JSON body (fal returns JSON on errors too).
    fn post(&self, url: &str, body: &Value) -> Result<(u16, Value), AiError> {
        let auth = format!("Key {}", self.key);
        http::post_json(url, &[("authorization", &auth), ("content-type", "application/json")], body)
    }

    /// Download the generated image bytes from its (fal-hosted) URL. Bespoke (not
    /// in `http`) because it reads raw bytes under a size cap, not JSON.
    fn download(&self, url: &str) -> Result<Vec<u8>, AiError> {
        let mut res = ureq::get(url)
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|e| AiError::Network { status: None, message: e.to_string() })?;
        let status = res.status().as_u16();
        if !is_ok(status) {
            return Err(AiError::Network { status: Some(status), message: "image download failed".into() });
        }
        res.body_mut()
            .with_config()
            .limit(MAX_IMAGE_BYTES)
            .read_to_vec()
            .map_err(|e| AiError::Network { status: None, message: e.to_string() })
    }
}

/// The run URL for a model id.
fn run_url(model: &str) -> String {
    format!("{BASE}/{model}")
}

/// The request body — fal's image models take a `prompt` input.
fn body(req: &ImageRequest) -> Value {
    json!({ "prompt": req.prompt })
}

/// The first image's URL and content type from a fal result
/// (`{ "images": [ { "url": ..., "content_type": ... } ] }`). The content type is
/// optional (defaults to PNG); the URL is required.
fn first_image(value: &Value) -> Result<(String, String), AiError> {
    let image = value
        .pointer("/images/0")
        .ok_or_else(|| AiError::ApiMalformed("fal response had no image".into()))?;
    let url = image
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::ApiMalformed("fal image had no url".into()))?;
    let content_type =
        image.get("content_type").and_then(Value::as_str).unwrap_or("image/png");
    Ok((url.to_string(), content_type.to_string()))
}

/// The `Network` error for a fal error body (it reports a `detail` message);
/// `Display` prepends the status.
fn api_error(status: u16, value: &Value) -> AiError {
    let msg = value
        .pointer("/detail")
        .and_then(Value::as_str)
        .or_else(|| value.get("detail").and_then(|d| d.pointer("/0/msg")).and_then(Value::as_str))
        .unwrap_or("unknown error");
    AiError::Network { status: Some(status), message: msg.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_url_joins_the_model() {
        assert_eq!(run_url("fal-ai/flux/schnell"), "https://fal.run/fal-ai/flux/schnell");
    }

    #[test]
    fn body_carries_the_prompt() {
        let req = ImageRequest { prompt: "a portrait of Mara".into(), model: "m".into() };
        assert_eq!(body(&req)["prompt"], "a portrait of Mara");
    }

    #[test]
    fn first_image_reads_url_and_content_type() {
        let v = json!({ "images": [ { "url": "https://cdn.fal/img.jpg", "content_type": "image/jpeg" } ] });
        assert_eq!(first_image(&v).unwrap(), ("https://cdn.fal/img.jpg".into(), "image/jpeg".into()));
    }

    #[test]
    fn first_image_defaults_the_content_type() {
        let v = json!({ "images": [ { "url": "https://cdn.fal/img" } ] });
        assert_eq!(first_image(&v).unwrap().1, "image/png");
    }

    #[test]
    fn first_image_errors_when_absent() {
        assert!(matches!(first_image(&json!({ "images": [] })), Err(AiError::ApiMalformed(_))));
    }

    #[test]
    fn api_error_reads_the_detail_message() {
        let e = api_error(401, &json!({ "detail": "Unauthorized" }));
        assert!(e.to_string().contains("401") && e.to_string().contains("Unauthorized"));
    }
}
