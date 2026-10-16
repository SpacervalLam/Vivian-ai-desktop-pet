//! Tauri adapter for durable, per-attempt work-agent request observations.
use super::coding_workflow::{self, WorkflowRequestDetail, WorkflowRequestSummary, WorkflowToolCall};
use crate::providers::base::LLMRequest;
use crate::utils::path::get_user_data_dir;
use serde_json::{json, Value};
use tauri::Emitter;

pub struct Capture {
    app: tauri::AppHandle,
    session_id: String,
    detail: WorkflowRequestDetail,
    started: std::time::Instant,
    settled: bool,
}

impl Capture {
    pub fn start(app: &tauri::AppHandle, session_id: &str, turn_message_index: usize, turn: usize,
        step: usize, attempt: usize, purpose: &str, model: String, request: &LLMRequest) -> Self {
        let messages = serde_json::to_value(&request.messages).unwrap_or(Value::Null);
        let system = messages.as_array().into_iter().flatten()
            .filter(|m| m["role"] == "system").cloned().collect();
        let capture = Self {
            app: app.clone(), session_id: session_id.into(), started: std::time::Instant::now(), settled: false,
            detail: WorkflowRequestDetail {
                summary: WorkflowRequestSummary {
                    id: uuid::Uuid::new_v4().to_string(), turn_message_index, turn, step, attempt,
                    purpose: purpose.into(), model, started_at: chrono::Utc::now().timestamp_millis(),
                    completed_at: None, duration_ms: None, status: "running".into(),
                    response_preview: String::new(), tool_calls: vec![], usage: None, error: None,
                },
                capture_stage: "agent_input".into(), system, messages,
                tools: serde_json::to_value(&request.tools).unwrap_or(Value::Null),
                reasoning: serde_json::to_value(request.reasoning).unwrap_or(Value::Null),
                response: Value::Null,
            },
        };
        capture.publish();
        capture
    }

    fn publish(&self) {
        let root = get_user_data_dir().join("coding_workflow");
        let result = if self.detail.summary.status == "running" {
            coding_workflow::save(&root, &self.session_id, &self.detail)
        } else {
            coding_workflow::settle(&root, &self.session_id, &self.detail)
        };
        if let Err(error) = result {
            tracing::error!("Workflow snapshot: {error}");
            let _ = self.app.emit("coding:workflow_error", json!({"session_id":self.session_id,"message":error}));
        }
        let _ = self.app.emit("coding:workflow", json!({"session_id":self.session_id,"request":self.detail.summary}));
    }

    pub fn finish(&mut self, response: Value, usage: Option<Value>, error: Option<String>) {
        if self.settled { return; }
        self.settled = true;
        self.detail.summary.completed_at = Some(chrono::Utc::now().timestamp_millis());
        self.detail.summary.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        self.detail.summary.status = if error.is_some() { "error" } else { "complete" }.into();
        self.detail.summary.error = error;
        self.detail.summary.usage = usage;
        self.detail.summary.response_preview = response["content"].as_str().unwrap_or("").chars().take(400).collect();
        self.detail.summary.tool_calls = response["tool_calls"].as_array().into_iter().flatten().map(|call| WorkflowToolCall {
            id: call["id"].as_str().unwrap_or("").into(), name: call["name"].as_str().unwrap_or("").into(),
        }).collect();
        self.detail.response = response;
        self.publish();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if !self.settled {
            self.detail.summary.status = "interrupted".into();
            self.detail.summary.completed_at = Some(chrono::Utc::now().timestamp_millis());
            self.detail.summary.duration_ms = Some(self.started.elapsed().as_millis() as u64);
            self.publish();
        }
    }
}
