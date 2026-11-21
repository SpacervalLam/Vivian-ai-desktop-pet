//! Route-level schema negotiation and stream admission, independent of desktop UI.
use super::base::{scope_provider_call, BaseProvider, ProviderCallOptions, StreamEvent};
use crate::error::{VivianError, VivianResult};
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn with_candidate_schema<T, F, Fut>(
    provider: &dyn BaseProvider,
    broken: &RwLock<HashSet<String>>,
    persist: impl Fn(&HashSet<String>),
    schema: Option<Value>,
    mut operation: F,
) -> VivianResult<T>
where
    F: FnMut(Option<Value>) -> Fut,
    Fut: std::future::Future<Output = VivianResult<T>>,
{
    let identity = provider.provider_identity();
    let schema_requested = schema.is_some();
    let mut schema = schema.filter(|_| !broken.read().contains(&identity));
    loop {
        let options = ProviderCallOptions {
            json_schema: schema.clone().map(Arc::new),
            disable_json_schema: schema.is_none(),
            // Schema downgrade must not hit an answer cached with strict enabled.
            response_cache_allowed: schema_requested.then_some(false),
            ..Default::default()
        };
        let result = scope_provider_call(options, operation(schema.clone())).await;
        if schema.is_some() && result.as_ref().err().is_some_and(is_strict_error) {
            // Invalid user schemas are not evidence that the model lacks
            // structured-output support. Only remember explicit rejection.
            if result.as_ref().err().is_some_and(is_schema_unsupported) {
                let mut broken = broken.write();
                if broken.insert(identity.clone()) {
                    persist(&broken);
                }
            }
            schema = None;
            continue;
        }
        return result;
    }
}

pub async fn prime_stream(
    mut source: mpsc::Receiver<StreamEvent>,
) -> VivianResult<mpsc::Receiver<StreamEvent>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut pending = Vec::new();
    loop {
        let event = tokio::time::timeout_at(deadline, source.recv())
            .await
            .map_err(|_| VivianError::Timeout("Provider first-event timeout".into()))?
            .ok_or_else(|| VivianError::Provider("Provider returned an empty stream".into()))?;
        if let StreamEvent::Error { message } = event {
            return Err(VivianError::Provider(message));
        }
        if matches!(event, StreamEvent::Done { .. }) {
            return Err(VivianError::Provider(
                "Provider returned an empty stream".into(),
            ));
        }
        let useful = matches!(&event,
            StreamEvent::Text { content } | StreamEvent::Thinking { content } if !content.is_empty()
        ) || matches!(&event, StreamEvent::ToolCallDelta { .. });
        pending.push(event);
        if pending.len() > 64 {
            return Err(VivianError::Provider(
                "Provider returned too many events before output".into(),
            ));
        }
        if useful {
            break;
        }
    }
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        for event in pending {
            if tx.send(event).await.is_err() {
                return;
            }
        }
        loop {
            let event = tokio::select! {
                _ = tx.closed() => break,
                event = source.recv() => event,
            };
            let Some(event) = event else {
                break;
            };
            let terminal_error = matches!(event, StreamEvent::Error { .. });
            if tx.send(event).await.is_err() || terminal_error {
                break;
            }
        }
    });
    Ok(rx)
}

fn is_schema_unsupported(err: &VivianError) -> bool {
    let message = err.to_string().to_lowercase();
    [
        "unsupported",
        "not supported",
        "does not support",
        "not allowed",
        "unknown parameter",
        "unknown field",
        "unrecognized",
        "不支持",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn is_strict_error(err: &VivianError) -> bool {
    let msg = err.to_string();
    // 必须是 400 错误
    if !match err {
        VivianError::ProviderHttp { status, .. } => *status == 400,
        _ => msg.contains("400"), // Compatibility with plugin and WebSocket errors.
    } {
        return false;
    }
    // 检查 schema 相关关键词（覆盖 OpenAI / 豆包 / Gemini 的错误信息）
    const SCHEMA_KEYWORDS: &[&str] = &[
        "json_schema",
        "response_format",
        "responseschema",
        "response_schema",
        "structured output",
        "structured_output",
        "invalid schema",
        "schema validation",
        "strict",
        "$ref",
        "$defs",
    ];
    let lower = msg.to_lowercase();
    SCHEMA_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Mock;
    #[async_trait::async_trait]
    impl BaseProvider for Mock {
        async fn call_chat(
            &self,
            _: Vec<crate::types::response::ChatMessage>,
        ) -> VivianResult<String> {
            unreachable!()
        }
        async fn call_stream_chat(
            &self,
            _: Vec<crate::types::response::ChatMessage>,
            _: Option<Value>,
        ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
            unreachable!()
        }
        fn get_model(&self) -> &str {
            "actual-fallback"
        }
        fn get_circuit_breaker_stats(&self) -> Value {
            json!({})
        }
    }

    #[tokio::test]
    async fn unsupported_schema_is_candidate_local_and_outer_schema_is_cleared() {
        let broken = RwLock::new(HashSet::new());
        let mut attempts = 0;
        scope_provider_call(
            ProviderCallOptions {
                json_schema: Some(Arc::new(json!({"type":"object"}))),
                ..Default::default()
            },
            with_candidate_schema(
                &Mock,
                &broken,
                |_| {},
                Some(json!({"type":"object"})),
                |_| {
                    attempts += 1;
                    let attempt = attempts;
                    async move {
                        if attempt == 1 {
                            assert!(ProviderCallOptions::current_json_schema().is_some());
                            Err(VivianError::ProviderHttp {
                                status: 400,
                                message: "unsupported responseSchema".into(),
                                retry_after_secs: None,
                            })
                        } else {
                            assert!(ProviderCallOptions::current_json_schema().is_none());
                            Ok(())
                        }
                    }
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(
            *broken.read(),
            HashSet::from(["actual-fallback".to_string()])
        );
    }

    #[tokio::test]
    async fn invalid_schema_is_not_cached_as_missing_model_capability() {
        let broken = RwLock::new(HashSet::new());
        let mut attempts = 0;
        with_candidate_schema(
            &Mock,
            &broken,
            |_| panic!("must not persist invalid schema"),
            Some(json!({"type":"object"})),
            |_| {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err(VivianError::ProviderHttp {
                        status: 400,
                        message: "invalid schema: missing required field".into(),
                        retry_after_secs: None,
                    })
                } else {
                    Ok(())
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
        assert!(broken.read().is_empty());
    }

    #[tokio::test]
    async fn stream_commits_only_useful_output_and_replays_leading_usage_once() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(StreamEvent::Usage {
            input_tokens: 2,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        })
        .await
        .unwrap();
        tx.send(StreamEvent::Text {
            content: "你好😀".into(),
        })
        .await
        .unwrap();
        tx.send(StreamEvent::Error {
            message: "late failure".into(),
        })
        .await
        .unwrap();
        drop(tx);
        let mut rx = prime_stream(rx).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamEvent::Usage { .. })));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Text { content }) if content == "你好😀")
        );
        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn early_error_and_empty_done_never_commit_a_route() {
        for event in [
            StreamEvent::Error {
                message: "early failure".into(),
            },
            StreamEvent::Done {
                finish_reason: None,
            },
        ] {
            let (tx, rx) = mpsc::channel(1);
            tx.send(event).await.unwrap();
            drop(tx);
            assert!(prime_stream(rx).await.is_err());
        }
    }
}
