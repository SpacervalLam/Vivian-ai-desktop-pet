//! Model-call adapter: observes the final router path without changing routing.
use super::coding_event_log::{self as audit, Context, Span};
use crate::{error::VivianResult, providers::base::{LLMRequest, StreamEvent}};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub fn request_info(request: &LLMRequest) -> Option<(Context, Value)> {
    let mut context = audit::current()?;
    context.request_id = Some(request.audit_request_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()));
    Some((context, json!({"task_type":request.task_type,"reasoning":request.reasoning,
        "message_count":request.messages.len(),"tool_count":request.tools.len(),"max_tokens":request.max_tokens_override})))
}
pub async fn response<T>(info: Option<(Context, Value)>, future: impl std::future::Future<Output = VivianResult<T>>,
    summary: impl FnOnce(&T) -> Value) -> VivianResult<T> {
    let Some((context, mut payload)) = info else { return future.await; };
    payload["stream"] = json!(false);
    let mut span = Span::start(Some(context.clone()), None, "model_request", payload);
    let result = audit::scope(context, future).await;
    match &result {
        Ok(value) => span.finish(json!({"status":"complete","response":summary(value)})),
        Err(error) => span.finish(json!({"status":"error","error":error.to_string()})),
    }
    result
}
pub async fn stream(info: Option<(Context, Value)>, future: impl std::future::Future<Output = VivianResult<mpsc::Receiver<StreamEvent>>>) -> VivianResult<mpsc::Receiver<StreamEvent>> {
    let Some((context, mut payload)) = info else { return future.await; };
    payload["stream"] = json!(true);
    let mut span = Span::start(Some(context.clone()), None, "model_request", payload);
    let mut source = match audit::scope(context, future).await {
        Ok(source) => source,
        Err(error) => { span.finish(json!({"status":"error","error":error.to_string()})); return Err(error); }
    };
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        let mut usage = Value::Null;
        let mut chars = 0;
        let mut calls = Vec::new();
        let mut call_count = 0usize;
        loop {
            let event = tokio::select! { biased; _ = tx.closed() => return, event = source.recv() => event };
            let Some(event) = event else { span.finish(json!({"status":"error","error":"stream ended before Done","usage":usage})); return; };
            let terminal = match &event {
                StreamEvent::Usage { .. } => { usage = serde_json::to_value(&event).unwrap_or(Value::Null); None },
                StreamEvent::Text { content } => { chars += content.chars().count(); None },
                StreamEvent::ToolCallDelta { id:Some(id), name, .. } => {
                    call_count += 1;
                    if calls.len() < 64 { calls.push(json!({"call_id":id,"name":name})); } None
                },
                StreamEvent::Done { finish_reason } => Some(json!({"status":"complete","finish_reason":finish_reason,"usage":usage,"response_chars":chars,
                    "tool_calls":calls,"tool_calls_truncated":call_count > calls.len()})),
                StreamEvent::Error { message } => Some(json!({"status":"error","error":message,"usage":usage})),
                _ => None,
            };
            if tx.send(event).await.is_err() { return; }
            if let Some(payload) = terminal { span.finish(payload); return; }
        }
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(root: &std::path::Path) -> Context {
        Context {session_id:"model-audit-test".into(),turn_id:Some("turn".into()),request_id:Some("request".into()),root:Some(root.into()),..Default::default()}
    }
    async fn finished(root: &std::path::Path, kind: &str) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let page = audit::read_page(root, "model-audit-test", 0, 100).unwrap();
                if page["events"].as_array().unwrap().iter().any(|e|e["kind"] == kind) { return page; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap()
    }
    #[tokio::test]
    async fn audit_response_keeps_actual_route_and_request_correlations() {
        let dir = tempfile::tempdir().unwrap(); let ctx = context(dir.path());
        let result = response(Some((ctx, json!({}))), async {
            audit::current_record("model_route_failed",json!({"model":"first","will_fallback":true}));
            audit::current_record("model_route_selected",json!({"model":"fallback"}));
            Ok::<_,crate::error::VivianError>("answer".to_string())
        }, |s|json!({"chars":s.len()})).await.unwrap();
        assert_eq!(result,"answer");
        let page = finished(dir.path(),"model_request_finished").await;
        assert_eq!(page["events"][2]["payload"]["model"],"fallback");
        for event in page["events"].as_array().unwrap() {
            assert_eq!(event["request_id"],"request"); assert_eq!(event["turn_id"],"turn");
        }
    }
    #[tokio::test]
    async fn audit_stream_preserves_events_usage_and_completion() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(8);
        let input = vec![StreamEvent::Usage {input_tokens:3,output_tokens:4,cache_read_tokens:5,cache_write_tokens:0},
            StreamEvent::Text {content:"你好".into()}, StreamEvent::Done {finish_reason:Some("stop".into())}];
        for event in &input { tx.send(event.clone()).await.unwrap(); } drop(tx);
        let mut output = stream(Some((context(dir.path()),json!({}))),async {Ok(rx)}).await.unwrap();
        let mut actual = Vec::new(); while let Some(event) = output.recv().await { actual.push(event); }
        assert_eq!(serde_json::to_value(actual).unwrap(),serde_json::to_value(input).unwrap());
        let page = finished(dir.path(),"model_request_finished").await;
        assert_eq!(page["events"][1]["payload"]["usage"]["cache_read_tokens"],5);
        assert_eq!(page["events"][1]["payload"]["status"],"complete");
    }
    #[tokio::test]
    async fn audit_stream_cancellation_drops_upstream_without_replaying() {
        let dir = tempfile::tempdir().unwrap(); let (tx,rx) = mpsc::channel(8);
        let output = stream(Some((context(dir.path()),json!({}))),async {Ok(rx)}).await.unwrap();
        drop(output);
        tokio::time::timeout(std::time::Duration::from_secs(2),tx.closed()).await.unwrap();
        let page = finished(dir.path(),"model_request_interrupted").await;
        assert!(!page["events"].as_array().unwrap().iter().any(|e|e["kind"] == "model_request_finished"));
    }
    #[tokio::test]
    async fn audit_failed_disk_and_failed_model_keep_original_results() {
        let dir = tempfile::tempdir().unwrap();let blocked = dir.path().join("file");std::fs::write(&blocked,"snapshot").unwrap();
        let result = response(Some((context(&blocked),json!({}))),async {Ok(42)}, |v|json!(v)).await;
        assert_eq!(result.unwrap(),42);
        let result: VivianResult<String> = response(Some((context(dir.path()),json!({}))),async {Err(crate::error::VivianError::Other("upstream failed".into()))},|s|json!(s)).await;
        assert!(result.unwrap_err().to_string().contains("upstream failed"));
        let page = finished(dir.path(),"model_request_finished").await;
        assert_eq!(page["events"][1]["payload"]["status"],"error");
    }
}
