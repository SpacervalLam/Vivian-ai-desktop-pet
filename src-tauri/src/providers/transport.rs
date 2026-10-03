//! Protocol-independent URL and HTTP error handling.
//! Adapted to Vivian's client architecture from cc-switch's adapter/transport separation.

use crate::error::{VivianError, VivianResult};
use crate::providers::base::ProviderBase;
use crate::resilience::{classify_error, ErrorCategory};
use futures::StreamExt;

/// Exactly one circuit admission and health outcome per logical request.
/// Drop releases a half-open probe on cancellation or a caller/configuration error.
pub struct RequestGuard {
    breaker: std::sync::Arc<parking_lot::RwLock<crate::resilience::CircuitBreaker>>,
    probe_ticket: Option<std::time::Instant>,
    completed: bool,
}

impl RequestGuard {
    pub fn begin(base: &ProviderBase) -> VivianResult<Self> {
        let mut breaker = base.circuit_breaker.write();
        if !breaker.allow_request() {
            return Err(VivianError::CircuitBreaker(breaker.name.clone()));
        }
        Ok(Self {
            breaker: base.circuit_breaker.clone(),
            probe_ticket: breaker.probe_ticket(),
            completed: false,
        })
    }

    pub fn success(mut self) {
        let mut breaker = self.breaker.write();
        if self.probe_ticket.is_none() || breaker.probe_ticket() == self.probe_ticket {
            breaker.record_success();
        }
        self.completed = true;
    }

    pub async fn response(
        self,
        response: Result<reqwest::Response, reqwest::Error>,
    ) -> VivianResult<(reqwest::Response, Self)> {
        match response {
            Ok(response) if response.status().is_success() => Ok((response, self)),
            response => {
                let error = match response {
                    Ok(response) => http_error(response).await,
                    Err(error) => VivianError::Network(error.without_url().to_string()),
                };
                self.failure(&error);
                Err(error)
            }
        }
    }

    pub fn failure(mut self, error: &VivianError) {
        let mut breaker = self.breaker.write();
        if self.probe_ticket.is_some() && breaker.probe_ticket() != self.probe_ticket {
            self.completed = true;
            return;
        }
        if !caller_error(error) {
            breaker.record_failure();
        } else {
            breaker.release_probe(self.probe_ticket);
        }
        self.completed = true;
    }
}

/// Headers are not a successful generation. Track the complete stream and release
/// half-open admission immediately when the receiver is cancelled.
pub fn track_stream(
    mut source: tokio::sync::mpsc::Receiver<super::base::StreamEvent>,
    guard: RequestGuard,
) -> tokio::sync::mpsc::Receiver<super::base::StreamEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        let mut useful = false;
        loop {
            let event = tokio::select! {
                _ = tx.closed() => return,
                event = source.recv() => event,
            };
            let Some(event) = event else {
                if useful {
                    guard.success();
                } else {
                    let error = VivianError::Provider("Provider returned an empty stream".into());
                    guard.failure(&error);
                    let _ = tx
                        .send(super::base::StreamEvent::Error {
                            message: error.to_string(),
                        })
                        .await;
                }
                return;
            };
            if let super::base::StreamEvent::Error { message } = &event {
                let error = VivianError::Provider(message.clone());
                guard.failure(&error);
                let _ = tx.send(event).await;
                return;
            }
            useful |= matches!(&event, super::base::StreamEvent::Text { content }
                | super::base::StreamEvent::Thinking { content } if !content.is_empty())
                || matches!(&event, super::base::StreamEvent::ToolCallDelta { .. });
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    rx
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.breaker.write().release_probe(self.probe_ticket);
        }
    }
}

pub async fn with_retry<T, F, Fut>(base: &ProviderBase, mut operation: F) -> VivianResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = VivianResult<T>>,
{
    let guard = RequestGuard::begin(base)?;
    let mut backoff = std::time::Duration::from_millis(500);
    for attempt in 0..3 {
        match operation().await {
            Ok(value) => {
                guard.success();
                return Ok(value);
            }
            Err(error) => {
                if attempt == 2 || classify_error(&error) == ErrorCategory::Permanent {
                    guard.failure(&error);
                    return Err(error);
                }
                tracing::debug!(model = %base.model, attempt = attempt + 1, code = error.code(), "Retrying provider request");
                tokio::time::sleep(retry_delay(Some(&error), backoff)).await;
                backoff *= 2;
            }
        }
    }
    unreachable!("retry loop always returns")
}

/// Preserve gateway prefixes, query parameters and explicit full API URLs.
pub fn api_endpoint(base: &str, suffix: &str, root_version: Option<&str>) -> String {
    let base = base.trim();
    let Ok(mut url) = reqwest::Url::parse(base) else {
        // Factory validation reports malformed configuration before use.
        return format!(
            "{}/{}",
            base.trim_end_matches('/'),
            suffix.trim_start_matches('/')
        );
    };
    let path = url.path().trim_end_matches('/');
    let suffix = suffix.trim_matches('/');
    let path = if path.ends_with(&format!("/{suffix}")) {
        path.to_string()
    } else if path.is_empty() {
        format!(
            "{}/{}",
            root_version.unwrap_or("").trim_end_matches('/'),
            suffix
        )
    } else if suffix == "v1/messages" && path.ends_with("/messages") {
        path.to_string()
    } else {
        let base_parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let suffix_parts: Vec<&str> = suffix.split('/').collect();
        let overlap = (1..=base_parts.len().min(suffix_parts.len()))
            .rev()
            .find(|&n| base_parts[base_parts.len() - n..] == suffix_parts[..n])
            .unwrap_or(0);
        format!("{path}/{}", suffix_parts[overlap..].join("/"))
    };
    url.set_path(&path);
    url.set_fragment(None);
    url.to_string()
}

pub fn validate_endpoint(endpoint: &str, websocket: bool) -> VivianResult<()> {
    let url = reqwest::Url::parse(endpoint.trim())
        .map_err(|_| VivianError::Config("Provider endpoint 必须是完整的 URL".into()))?;
    let valid_scheme = matches!(url.scheme(), "http" | "https")
        || (websocket && matches!(url.scheme(), "ws" | "wss"));
    if !valid_scheme || url.host_str().is_none() {
        return Err(VivianError::Config(
            "Provider endpoint 必须使用 HTTP(S) 或受支持的 WebSocket 协议".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(VivianError::Config(
            "Provider endpoint 不能嵌入用户名或密码，请使用鉴权字段".into(),
        ));
    }
    Ok(())
}

/// Read a bounded diagnostic body, retaining structured vendor codes for classification.
pub async fn http_error(response: reqwest::Response) -> VivianError {
    let status = response.status().as_u16();
    let retry_after_secs = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.trim().parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|date| (date.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
            })
        });
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, stream.next()).await {
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = 16 * 1024 - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() == 16 * 1024 {
            break;
        }
    }
    let message = if body.is_empty() {
        "Upstream returned an empty error response".to_string()
    } else {
        String::from_utf8_lossy(&body).into_owned()
    };
    VivianError::ProviderHttp {
        status,
        message,
        retry_after_secs,
    }
}

pub fn retry_delay(
    error: Option<&VivianError>,
    backoff: std::time::Duration,
) -> std::time::Duration {
    let requested = match error {
        Some(VivianError::ProviderHttp {
            retry_after_secs: Some(seconds),
            ..
        }) => std::time::Duration::from_secs(*seconds),
        _ => backoff,
    };
    requested
        .max(backoff)
        .min(std::time::Duration::from_secs(30))
}

pub fn validate_json(value: &serde_json::Value) -> VivianResult<()> {
    let has_error = value.get("error").is_some_and(|error| !error.is_null())
        || value
            .get("error_code")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|code| code != 0)
        || value["status"].as_str() == Some("failed");
    if has_error {
        // Preserve the vendor code and message even when a gateway returned HTTP 200.
        return Err(VivianError::Provider(value.to_string()));
    }
    Ok(())
}

/// A different provider can repair credentials/availability/model failures, but
/// cannot repair malformed local configuration or an invalid client request.
pub fn may_failover(error: &VivianError) -> bool {
    !caller_error(error)
}

/// Retryability and health impact are separate: invalid credentials or an unpaid
/// upstream should fail immediately, count once, and allow another provider.
fn caller_error(error: &VivianError) -> bool {
    use crate::resilience::{classify_llm_error_from_str, LlmErrorKind};
    match error {
        VivianError::Config(_) | VivianError::Serialization(_) | VivianError::Json(_) => true,
        VivianError::ProviderHttp {
            status: 400 | 422,
            message,
            ..
        } => !matches!(
            classify_llm_error_from_str(message),
            LlmErrorKind::InvalidApiKey
                | LlmErrorKind::InsufficientBalance
                | LlmErrorKind::QuotaExceeded
                | LlmErrorKind::ModelNotFound
                | LlmErrorKind::RegionNotSupported
                | LlmErrorKind::PermissionDenied
        ),
        VivianError::Provider(message) => matches!(
            classify_llm_error_from_str(message),
            LlmErrorKind::BadRequest
                | LlmErrorKind::ContextLengthExceeded
                | LlmErrorKind::ContentPolicy
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_urls_and_gateway_prefixes_preserve_query_parameters() {
        assert_eq!(
            api_endpoint(
                "https://host.test/gateway/v1/responses/?token=x",
                "responses",
                None
            ),
            "https://host.test/gateway/v1/responses?token=x"
        );
        assert_eq!(
            api_endpoint("https://host.test/api/v3", "responses", None),
            "https://host.test/api/v3/responses"
        );
        assert_eq!(
            api_endpoint("https://host.test/v1?region=a", "v1/messages", None),
            "https://host.test/v1/messages?region=a"
        );
        assert_eq!(
            api_endpoint("https://host.test", "chat/completions", Some("/v1")),
            "https://host.test/v1/chat/completions"
        );
        assert_eq!(
            api_endpoint(
                "https://host.test/gateway/v1?region=a",
                "/v1/chat/completions",
                None
            ),
            "https://host.test/gateway/v1/chat/completions?region=a"
        );
        assert_eq!(
            api_endpoint("https://host.test/proxy/messages", "v1/messages", None),
            "https://host.test/proxy/messages"
        );
    }

    #[test]
    fn invalid_endpoint_configuration_is_rejected_early() {
        assert!(validate_endpoint("localhost:11434", false).is_err());
        assert!(validate_endpoint("file:///tmp/model", false).is_err());
        assert!(validate_endpoint("https://user:key@host.test", false).is_err());
        assert!(validate_endpoint("http://127.0.0.1:11434/v1", false).is_ok());
        assert!(validate_endpoint("wss://host.test/v3", true).is_ok());
    }

    fn base() -> ProviderBase {
        ProviderBase::new(
            "test-key".into(),
            "http://127.0.0.1/v1".into(),
            "transport-test".into(),
            0.7,
            100,
        )
    }

    #[test]
    fn http_status_not_body_digits_controls_retry_and_retry_after_is_bounded() {
        let error = VivianError::ProviderHttp {
            status: 429,
            message: "request 400 was throttled".into(),
            retry_after_secs: Some(90),
        };
        assert_eq!(classify_error(&error), ErrorCategory::RateLimit);
        assert_eq!(
            retry_delay(Some(&error), std::time::Duration::from_millis(500)),
            std::time::Duration::from_secs(30)
        );
        assert!(!may_failover(&VivianError::ProviderHttp {
            status: 400,
            message: "invalid request".into(),
            retry_after_secs: None
        }));
        assert!(may_failover(&VivianError::ProviderHttp {
            status: 401,
            message: "invalid key".into(),
            retry_after_secs: None
        }));
        assert!(validate_json(&serde_json::json!({"error":{"code":"invalid_api_key"}})).is_err());
        assert!(validate_json(&serde_json::json!({"status":"incomplete","output":[]})).is_ok());
    }

    #[tokio::test]
    async fn retries_count_one_outcome_and_invalid_requests_are_health_neutral() {
        let base = base();
        let mut attempts = 0;
        let value = with_retry(&base, || {
            attempts += 1;
            let result = if attempts == 1 {
                Err(VivianError::ProviderHttp {
                    status: 503,
                    message: "unavailable".into(),
                    retry_after_secs: None,
                })
            } else {
                Ok("recovered")
            };
            std::future::ready(result)
        })
        .await
        .unwrap();
        assert_eq!(value, "recovered");
        assert_eq!(attempts, 2);
        assert_eq!(base.circuit_breaker.read().recent_results.len(), 1);
        assert_eq!(base.circuit_breaker.read().failure_count, 0);
        let mut attempts = 0;
        let result = with_retry(&base, || {
            attempts += 1;
            std::future::ready(Err::<(), _>(VivianError::ProviderHttp {
                status: 400,
                message: "bad schema".into(),
                retry_after_secs: None,
            }))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
        assert_eq!(base.circuit_breaker.read().recent_results.len(), 1);
    }

    #[tokio::test]
    async fn cancelled_half_open_stream_releases_probe_without_waiting_for_upstream() {
        let base = base();
        base.circuit_breaker.write().state = crate::resilience::CircuitState::HalfOpen;
        let guard = RequestGuard::begin(&base).unwrap();
        assert!(RequestGuard::begin(&base).is_err());
        let (tx, source) = tokio::sync::mpsc::channel(1);
        let rx = track_stream(source, guard);
        drop(rx);
        tokio::time::timeout(std::time::Duration::from_secs(1), tx.closed())
            .await
            .unwrap();
        let guard = RequestGuard::begin(&base).unwrap();
        assert_eq!(base.circuit_breaker.read().failure_count, 0);
        drop(guard);
    }

    #[tokio::test]
    async fn error_after_headers_is_not_recorded_as_success() {
        let base = base();
        let guard = RequestGuard::begin(&base).unwrap();
        let (tx, source) = tokio::sync::mpsc::channel(1);
        tx.send(super::super::base::StreamEvent::Error {
            message: "connection reset".into(),
        })
        .await
        .unwrap();
        drop(tx);
        let mut rx = track_stream(source, guard);
        assert!(matches!(
            rx.recv().await,
            Some(super::super::base::StreamEvent::Error { .. })
        ));
        assert!(rx.recv().await.is_none());
        assert_eq!(base.circuit_breaker.read().success_count, 0);
        assert_eq!(base.circuit_breaker.read().failure_count, 1);
    }

    #[test]
    fn obsolete_probe_result_cannot_overwrite_a_new_probe() {
        let base = base();
        base.circuit_breaker.write().state = crate::resilience::CircuitState::HalfOpen;
        let old = RequestGuard::begin(&base).unwrap();
        let ticket = base.circuit_breaker.read().probe_ticket();
        base.circuit_breaker.write().release_probe(ticket);
        let current = RequestGuard::begin(&base).unwrap();
        old.success();
        assert_eq!(
            base.circuit_breaker.read().state,
            crate::resilience::CircuitState::HalfOpen
        );
        assert!(RequestGuard::begin(&base).is_err());
        current.success();
        assert_eq!(
            base.circuit_breaker.read().state,
            crate::resilience::CircuitState::Closed
        );
    }
}
