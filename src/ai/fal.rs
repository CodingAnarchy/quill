//! The fal.ai image backend (#238): generates an image via fal.ai's synchronous
//! REST endpoint over HTTPS with `ureq` (runs on a worker thread), then downloads
//! the returned image bytes. Key-gated; the model travels on each request. See
//! `docs/ai-features.md`.

use serde_json::{json, Value};

use super::image::{ImageBackend, ImageRequest};
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
    fn generate(&self, req: &ImageRequest) -> Result<Vec<u8>, AiError> {
        let (status, value) = self.post(&run_url(&req.model), &body(req))?;
        if !is_ok(status) {
            return Err(api_error(status, &value));
        }
        let url = image_url(&value)?;
        self.download(&url)
    }
}

impl FalBackend {
    /// POST `body` to `url` with the fal auth header; returns the HTTP status and
    /// the parsed JSON body (fal returns JSON on errors too).
    fn post(&self, url: &str, body: &Value) -> Result<(u16, Value), AiError> {
        let mut res = ureq::post(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("authorization", &format!("Key {}", self.key))
            .header("content-type", "application/json")
            .send_json(body)
            .map_err(|e| AiError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        let value = res.body_mut().read_json().map_err(|e| AiError::Api(e.to_string()))?;
        Ok((status, value))
    }

    /// Download the generated image bytes from its (fal-hosted) URL.
    fn download(&self, url: &str) -> Result<Vec<u8>, AiError> {
        let mut res = ureq::get(url)
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|e| AiError::Network(e.to_string()))?;
        if !is_ok(res.status().as_u16()) {
            return Err(AiError::Network(format!("image download failed: {}", res.status())));
        }
        res.body_mut()
            .with_config()
            .limit(MAX_IMAGE_BYTES)
            .read_to_vec()
            .map_err(|e| AiError::Network(e.to_string()))
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

/// The first image URL from a fal result (`{ "images": [ { "url": ... } ] }`).
fn image_url(value: &Value) -> Result<String, AiError> {
    value
        .pointer("/images/0/url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AiError::Api("fal response had no image url".into()))
}

/// An `AiError::Api` from a fal error body (it reports a `detail` message).
fn api_error(status: u16, value: &Value) -> AiError {
    let msg = value
        .pointer("/detail")
        .and_then(Value::as_str)
        .or_else(|| value.get("detail").and_then(|d| d.pointer("/0/msg")).and_then(Value::as_str))
        .unwrap_or("unknown error");
    AiError::Api(format!("{status}: {msg}"))
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
    fn image_url_reads_the_first_image() {
        let v = json!({ "images": [ { "url": "https://cdn.fal/img.png" } ] });
        assert_eq!(image_url(&v).unwrap(), "https://cdn.fal/img.png");
    }

    #[test]
    fn image_url_errors_when_absent() {
        assert!(matches!(image_url(&json!({ "images": [] })), Err(AiError::Api(_))));
    }

    #[test]
    fn api_error_reads_the_detail_message() {
        let e = api_error(401, &json!({ "detail": "Unauthorized" }));
        assert!(e.to_string().contains("401") && e.to_string().contains("Unauthorized"));
    }
}
