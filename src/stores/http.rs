//! Shared HTTP retry + typed-error boundary for store API clients.
//!
//! `send_with_retry` performs a bounded exponential-backoff retry for
//! idempotent store API calls on transient failures (network errors, 408,
//! 429, 5xx). Non-retryable responses are mapped onto typed
//! [`ShipperError`] variants at this boundary, so callers can keep using
//! `anyhow::Result` without leaking raw `reqwest` shapes or ad-hoc
//! `anyhow!("API error: ...")` strings.
//!
//! Non-goals: upload-session resumption, `Retry-After` parsing, global
//! rate-limiting. Those land in later phases if store behaviour demands
//! them. Endpoints that are NOT safe to retry (bundle upload, edit
//! commit) bypass this helper and go through
//! [`map_status_to_error`] / [`map_upload_failure`] directly.

use crate::error::ShipperError;
use std::time::Duration;

/// How many attempts (including the first) `send_with_retry` will perform.
const MAX_ATTEMPTS: u32 = 3;
/// Initial backoff; doubled after each attempt, capped at `MAX_BACKOFF`.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetryDecision {
    /// Terminal success (2xx).
    Success,
    /// Transient failure; retry after backoff.
    Retry,
    /// Non-retryable failure; caller maps status+body to `ShipperError`.
    Fail,
}

/// Classify an HTTP status code for retry purposes.
///
/// - `2xx`       → `Success`
/// - `408`, `429`, `5xx` → `Retry`
/// - everything else     → `Fail`
pub(crate) fn classify_status(status: u16) -> RetryDecision {
    match status {
        200..=299 => RetryDecision::Success,
        408 | 429 => RetryDecision::Retry,
        500..=599 => RetryDecision::Retry,
        _ => RetryDecision::Fail,
    }
}

/// Map a terminal (non-retryable) status+body to a typed API error.
///
/// `401`/`403` become [`ShipperError::AuthError`]; other statuses become
/// [`ShipperError::ApiError`]. `op` is a short operation label
/// (e.g. `"create edit"`) that prefixes the error message.
pub(crate) fn map_status_to_error(status: u16, body: String, op: &'static str) -> ShipperError {
    let msg = compose_message(op, &body);
    match status {
        401 | 403 => ShipperError::AuthError(msg),
        _ => ShipperError::ApiError {
            status,
            message: msg,
        },
    }
}

/// Like [`map_status_to_error`] but routes non-auth failures to
/// [`ShipperError::UploadFailed`]. Use at upload-endpoint boundaries.
pub(crate) fn map_upload_failure(status: u16, body: String, op: &'static str) -> ShipperError {
    let msg = compose_message(op, &body);
    match status {
        401 | 403 => ShipperError::AuthError(msg),
        _ => ShipperError::UploadFailed(format!("[{}] {}", status, msg)),
    }
}

fn compose_message(op: &'static str, body: &str) -> String {
    if body.is_empty() {
        op.to_string()
    } else {
        format!("{op}: {body}")
    }
}

/// Send a request with retry + typed error mapping.
///
/// `make_request` is invoked once per attempt and MUST build an
/// idempotent request — repeating it must have no observable side effect
/// beyond the first successful send. Uploads and commits are NOT
/// idempotent and should bypass this helper.
///
/// On terminal failure the last observed error is returned. Transient
/// failures are logged at `debug` so retry attempts show up under
/// `--verbose` / `RUST_LOG=debug`.
pub(crate) async fn send_with_retry<F>(
    make_request: F,
    op: &'static str,
) -> anyhow::Result<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut backoff = INITIAL_BACKOFF;
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..MAX_ATTEMPTS {
        match make_request().send().await {
            Ok(res) => {
                let status = res.status().as_u16();
                match classify_status(status) {
                    RetryDecision::Success => return Ok(res),
                    RetryDecision::Retry => {
                        let body = res.text().await.unwrap_or_default();
                        tracing::debug!(
                            op = op,
                            status = status,
                            attempt = attempt + 1,
                            max = MAX_ATTEMPTS,
                            "transient failure, retrying"
                        );
                        last_err = Some(map_status_to_error(status, body, op).into());
                    }
                    RetryDecision::Fail => {
                        let body = res.text().await.unwrap_or_default();
                        return Err(map_status_to_error(status, body, op).into());
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    op = op,
                    attempt = attempt + 1,
                    max = MAX_ATTEMPTS,
                    error = %e,
                    "network error, retrying"
                );
                last_err = Some(anyhow::Error::new(e).context(format!("{op}: network error")));
            }
        }

        if attempt + 1 < MAX_ATTEMPTS {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{op}: retries exhausted")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_success_range() {
        assert_eq!(classify_status(200), RetryDecision::Success);
        assert_eq!(classify_status(201), RetryDecision::Success);
        assert_eq!(classify_status(204), RetryDecision::Success);
    }

    #[test]
    fn classify_retryable_transient() {
        assert_eq!(classify_status(408), RetryDecision::Retry);
        assert_eq!(classify_status(429), RetryDecision::Retry);
        assert_eq!(classify_status(500), RetryDecision::Retry);
        assert_eq!(classify_status(502), RetryDecision::Retry);
        assert_eq!(classify_status(503), RetryDecision::Retry);
        assert_eq!(classify_status(599), RetryDecision::Retry);
    }

    #[test]
    fn classify_client_failures_are_terminal() {
        assert_eq!(classify_status(400), RetryDecision::Fail);
        assert_eq!(classify_status(401), RetryDecision::Fail);
        assert_eq!(classify_status(403), RetryDecision::Fail);
        assert_eq!(classify_status(404), RetryDecision::Fail);
        assert_eq!(classify_status(409), RetryDecision::Fail);
        assert_eq!(classify_status(422), RetryDecision::Fail);
    }

    #[test]
    fn map_auth_statuses_route_to_auth_error() {
        let e = map_status_to_error(401, "bad jwt".to_string(), "fetch builds");
        assert!(matches!(e, ShipperError::AuthError(_)));
        assert!(e.to_string().contains("fetch builds"));
        assert!(e.to_string().contains("bad jwt"));

        let e = map_status_to_error(403, String::new(), "create edit");
        assert!(matches!(e, ShipperError::AuthError(_)));
        assert!(e.to_string().contains("create edit"));
    }

    #[test]
    fn map_other_statuses_route_to_api_error() {
        let e = map_status_to_error(404, "not found".to_string(), "fetch builds");
        match e {
            ShipperError::ApiError { status, message } => {
                assert_eq!(status, 404);
                assert!(message.contains("fetch builds"));
                assert!(message.contains("not found"));
            }
            other => panic!("expected ApiError, got {other:?}"),
        }
    }

    #[test]
    fn upload_failure_routes_auth_vs_upload() {
        let e = map_upload_failure(401, "bad token".to_string(), "upload bundle");
        assert!(matches!(e, ShipperError::AuthError(_)));

        let e = map_upload_failure(500, "oops".to_string(), "upload bundle");
        match e {
            ShipperError::UploadFailed(msg) => {
                assert!(msg.contains("500"));
                assert!(msg.contains("upload bundle"));
                assert!(msg.contains("oops"));
            }
            other => panic!("expected UploadFailed, got {other:?}"),
        }
    }
}
