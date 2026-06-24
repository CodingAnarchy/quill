//! Shared `ureq` transport for the HTTP backends (`claude.rs`, `fal.rs`): a
//! POST/GET that never treats an HTTP status as an error (the APIs return JSON
//! on errors too, so we read the body on any status to surface the server's own
//! message) and maps failures onto the uniform [`AiError`] taxonomy — a
//! transport failure to `Network { status: None, .. }`, an unreadable body to
//! `ApiMalformed`. The caller checks the status and turns an error response into
//! its own `Network { status: Some(..), .. }`. (#319)

use serde_json::Value;

use super::AiError;

/// A request header as a `(name, value)` pair.
pub(crate) type Header<'a> = (&'a str, &'a str);

/// POST `body` as JSON to `url` with `headers`; returns the HTTP status and the
/// parsed JSON response.
pub(crate) fn post_json(url: &str, headers: &[Header], body: &Value) -> Result<(u16, Value), AiError> {
    let mut res = with_headers(ureq::post(url).config().http_status_as_error(false).build(), headers)
        .send_json(body)
        .map_err(transport)?;
    let status = res.status().as_u16();
    let value = res.body_mut().read_json().map_err(malformed)?;
    Ok((status, value))
}

/// GET `url` as JSON with `headers`; returns the HTTP status and the parsed body.
pub(crate) fn get_json(url: &str, headers: &[Header]) -> Result<(u16, Value), AiError> {
    let mut res = with_headers(ureq::get(url).config().http_status_as_error(false).build(), headers)
        .call()
        .map_err(transport)?;
    let status = res.status().as_u16();
    let value = res.body_mut().read_json().map_err(malformed)?;
    Ok((status, value))
}

/// GET `url` as raw text with `headers` (e.g. JSONL batch results, not a JSON
/// document).
pub(crate) fn get_text(url: &str, headers: &[Header]) -> Result<String, AiError> {
    with_headers(ureq::get(url).config().http_status_as_error(false).build(), headers)
        .call()
        .map_err(transport)?
        .body_mut()
        .read_to_string()
        .map_err(malformed)
}

/// Apply the auth/content headers to a request builder (generic over the body
/// marker so it serves both POST and GET).
fn with_headers<B>(mut req: ureq::RequestBuilder<B>, headers: &[Header]) -> ureq::RequestBuilder<B> {
    for &(name, value) in headers {
        req = req.header(name, value);
    }
    req
}

/// A transport failure (the call never completed): no HTTP status to report.
fn transport(e: ureq::Error) -> AiError {
    AiError::Network { status: None, message: e.to_string() }
}

/// A body we couldn't read or parse as expected.
fn malformed(e: impl std::fmt::Display) -> AiError {
    AiError::ApiMalformed(e.to_string())
}
