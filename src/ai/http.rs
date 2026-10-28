//! Shared `ureq` transport for the HTTP backends (`claude.rs`, `fal.rs`): a
//! POST/GET that never treats an HTTP status as an error (the APIs return JSON
//! on errors too, so we read the body on any status to surface the server's own
//! message) and maps failures onto the uniform [`AiError`] taxonomy — a
//! transport failure to `Network { status: None, .. }`, an unreadable body to
//! `ApiMalformed`. The caller checks the status and turns an error response into
//! its own `Network { status: Some(..), .. }`. (#319)
//!
//! In test builds the three entry points are replaced by stubs that refuse the
//! request, so the suite can never reach a live provider API — the no-network
//! half of the hermeticity guarantee (#528). Tests drive the backends through
//! injected results / mock backends instead of the wire.

#[cfg(not(any(test, feature = "test-support")))]
use std::thread;
#[cfg(not(any(test, feature = "test-support")))]
use std::time::Duration;

use serde_json::Value;

use super::{retryable_status, AiError};

/// A request header as a `(name, value)` pair.
pub type Header<'a> = (&'a str, &'a str);

/// How many times a single idempotent request is attempted before its last
/// failure is surfaced — one initial try plus retries. Kept small: a
/// whole-manuscript pass is expensive, so we ride out a blip, not a sustained
/// outage. (#697)
const MAX_ATTEMPTS: u32 = 4;

/// Base backoff before the first retry; each subsequent wait doubles it
/// (250ms → 500ms → 1s). Not applied in test builds (see [`backoff`]).
#[cfg(not(any(test, feature = "test-support")))]
const BASE_BACKOFF: Duration = Duration::from_millis(250);

/// Retry an *idempotent single request* through bounded exponential backoff,
/// retrying only on transient failures and surfacing the last result otherwise.
/// `op` runs the request; `transient` reports whether an `Ok` value is itself a
/// retryable outcome — the JSON paths read the body on any status and return it
/// inside `Ok`, so a 429/529 has to be caught here, not just a transport `Err`.
///
/// Retries stop as soon as a result is non-transient (a success or a permanent
/// error) or the attempt cap is reached; the final result is returned unchanged,
/// so success behavior and the surfaced-error contract are untouched. This is
/// only for the single-request path — the batch submit is deliberately *not*
/// wrapped, since re-submitting would start a second, separately-billed batch
/// (see `claude.rs`). (#697)
pub fn retry<T>(
    transient: impl Fn(&T) -> bool,
    mut op: impl FnMut() -> Result<T, AiError>,
) -> Result<T, AiError> {
    let mut attempt = 0;
    loop {
        let result = op();
        let is_transient = match &result {
            Ok(value) => transient(value),
            Err(e) => e.retryable(),
        };
        attempt += 1;
        if !is_transient || attempt >= MAX_ATTEMPTS {
            return result;
        }
        backoff(attempt);
    }
}

/// Whether a `(status, _)` transport result carries a transient HTTP status —
/// the `Ok`-side predicate for [`retry`] on the JSON request paths, which read
/// the body on any status (so a 429/529 arrives as `Ok`, not `Err`). (#697)
pub const fn transient_status<V>((status, _): &(u16, V)) -> bool {
    retryable_status(*status)
}

/// Wait before retry number `attempt` (1-based): `BASE_BACKOFF` doubled per prior
/// retry. In test builds this is a no-op so the suite never sleeps (#528, #531).
#[cfg(not(any(test, feature = "test-support")))]
fn backoff(attempt: u32) {
    let factor = 1u32 << (attempt - 1);
    thread::sleep(BASE_BACKOFF * factor);
}

#[cfg(any(test, feature = "test-support"))]
const fn backoff(_attempt: u32) {}

/// POST `body` as JSON to `url` with `headers`; returns the HTTP status and the
/// parsed JSON response.
#[cfg(not(any(test, feature = "test-support")))]
pub fn post_json(url: &str, headers: &[Header], body: &Value) -> Result<(u16, Value), AiError> {
    let mut res = with_headers(
        ureq::post(url).config().http_status_as_error(false).build(),
        headers,
    )
    .send_json(body)
    .map_err(|e| transport(&e))?;
    let status = res.status().as_u16();
    let value = res.body_mut().read_json().map_err(malformed)?;
    Ok((status, value))
}

/// GET `url` as JSON with `headers`; returns the HTTP status and the parsed body.
#[cfg(not(any(test, feature = "test-support")))]
pub fn get_json(url: &str, headers: &[Header]) -> Result<(u16, Value), AiError> {
    let mut res = with_headers(
        ureq::get(url).config().http_status_as_error(false).build(),
        headers,
    )
    .call()
    .map_err(|e| transport(&e))?;
    let status = res.status().as_u16();
    let value = res.body_mut().read_json().map_err(malformed)?;
    Ok((status, value))
}

/// GET `url` as raw text with `headers` (e.g. JSONL batch results, not a JSON
/// document).
#[cfg(not(any(test, feature = "test-support")))]
pub fn get_text(url: &str, headers: &[Header]) -> Result<String, AiError> {
    with_headers(
        ureq::get(url).config().http_status_as_error(false).build(),
        headers,
    )
    .call()
    .map_err(|e| transport(&e))?
    .body_mut()
    .read_to_string()
    .map_err(malformed)
}

/// Apply the auth/content headers to a request builder (generic over the body
/// marker so it serves both POST and GET).
#[cfg(not(any(test, feature = "test-support")))]
fn with_headers<B>(
    mut req: ureq::RequestBuilder<B>,
    headers: &[Header],
) -> ureq::RequestBuilder<B> {
    for &(name, value) in headers {
        req = req.header(name, value);
    }
    req
}

/// A transport failure (the call never completed): no HTTP status to report.
#[cfg(not(any(test, feature = "test-support")))]
fn transport(e: &ureq::Error) -> AiError {
    AiError::Network {
        status: None,
        message: e.to_string(),
    }
}

/// A body we couldn't read or parse as expected.
#[cfg(not(any(test, feature = "test-support")))]
fn malformed(e: impl std::fmt::Display) -> AiError {
    AiError::ApiMalformed(e.to_string())
}

// --- Test stubs: every outbound request is refused before it leaves the machine,
// so the suite can never reach a live provider API (#528). ---------------------

#[cfg(any(test, feature = "test-support"))]
pub fn post_json(_url: &str, _headers: &[Header], _body: &Value) -> Result<(u16, Value), AiError> {
    Err(offline())
}

#[cfg(any(test, feature = "test-support"))]
pub fn get_json(_url: &str, _headers: &[Header]) -> Result<(u16, Value), AiError> {
    Err(offline())
}

#[cfg(any(test, feature = "test-support"))]
pub fn get_text(_url: &str, _headers: &[Header]) -> Result<String, AiError> {
    Err(offline())
}

#[cfg(any(test, feature = "test-support"))]
fn offline() -> AiError {
    AiError::Network {
        status: None,
        message: "outbound network is disabled in tests (#528)".into(),
    }
}

// The retry policy is pure and deterministic — no wire, no sleeps (backoff is a
// no-op under `cfg(test)`), so these are hermetic (#528, #531): they drive the
// classification and the bounded loop directly, not the transport. (#697)
#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;

    use super::{retry, transient_status, MAX_ATTEMPTS};
    use crate::ai::{retryable_status, AiError};

    // --- Classification: transient vs. permanent. ---

    #[test]
    fn rate_limit_and_overloaded_are_transient() {
        assert!(retryable_status(429));
        assert!(retryable_status(529));
    }

    #[test]
    fn other_statuses_are_permanent() {
        for status in [200, 400, 401, 403, 404, 500, 503] {
            assert!(!retryable_status(status), "{status} should be permanent");
        }
    }

    #[test]
    fn a_dropped_connection_is_retryable() {
        let dropped = AiError::Network {
            status: None,
            message: "connection reset".into(),
        };
        assert!(dropped.retryable());
    }

    #[test]
    fn a_transient_status_error_is_retryable_but_others_are_not() {
        let overloaded = AiError::Network {
            status: Some(529),
            message: "overloaded".into(),
        };
        assert!(overloaded.retryable());

        let bad_request = AiError::Network {
            status: Some(400),
            message: "bad request".into(),
        };
        assert!(!bad_request.retryable());
    }

    #[test]
    fn non_network_errors_are_never_retryable() {
        assert!(!AiError::NotConfigured("no key".into()).retryable());
        assert!(!AiError::ApiMalformed("no field".into()).retryable());
        assert!(!AiError::ModelNonCompliance("bad schema".into()).retryable());
        assert!(!AiError::Internal("poisoned lock".into()).retryable());
    }

    // --- The bounded retry loop. ---

    /// A transient error retries up to the cap, then surfaces the last failure.
    #[test]
    fn retries_a_transient_error_then_gives_up() {
        let calls = Cell::new(0);
        let result: Result<(u16, ()), AiError> = retry(transient_status, || {
            calls.set(calls.get() + 1);
            Err(AiError::Network {
                status: Some(529),
                message: "overloaded".into(),
            })
        });
        assert_eq!(calls.get(), MAX_ATTEMPTS, "should exhaust every attempt");
        assert!(matches!(
            result,
            Err(AiError::Network {
                status: Some(529),
                ..
            })
        ));
    }

    /// A permanent error is surfaced on the first try — no retries.
    #[test]
    fn does_not_retry_a_permanent_error() {
        let calls = Cell::new(0);
        let result: Result<(u16, ()), AiError> = retry(transient_status, || {
            calls.set(calls.get() + 1);
            Err(AiError::Network {
                status: Some(400),
                message: "bad request".into(),
            })
        });
        assert_eq!(calls.get(), 1, "a permanent error is not retried");
        assert!(result.is_err());
    }

    /// A transient *status inside `Ok`* (the JSON paths read the body on any
    /// status) is retried too, via the `transient_status` predicate.
    #[test]
    fn retries_a_transient_status_returned_in_ok() {
        let calls = Cell::new(0);
        let result = retry(transient_status, || {
            calls.set(calls.get() + 1);
            Ok((429, json!({"error": {"message": "rate limited"}})))
        });
        assert_eq!(calls.get(), MAX_ATTEMPTS);
        assert!(matches!(result, Ok((429, _))));
    }

    /// A successful (2xx) result returns immediately, unchanged.
    #[test]
    fn a_success_is_not_retried() {
        let calls = Cell::new(0);
        let result = retry(transient_status, || {
            calls.set(calls.get() + 1);
            Ok((200, json!({"ok": true})))
        });
        assert_eq!(calls.get(), 1);
        assert!(matches!(result, Ok((200, _))));
    }

    /// Recovery: a transient blip followed by success returns the success.
    #[test]
    fn recovers_after_a_transient_blip() {
        let calls = Cell::new(0);
        let result = retry(transient_status, || {
            calls.set(calls.get() + 1);
            if calls.get() < 2 {
                Ok((529, json!({})))
            } else {
                Ok((200, json!({"ok": true})))
            }
        });
        assert_eq!(calls.get(), 2, "one retry, then success");
        assert!(matches!(result, Ok((200, _))));
    }
}
