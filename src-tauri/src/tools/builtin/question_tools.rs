//! ask_user 工具 — 让模型在工具调用中向用户提问并等待自由文本回答。
//!
//! 模型调用 `ask_user` → 后端广播 `chat:question` 事件（携带 question_id + 问题）→
//! 前端弹输入框 → 用户回答经 `respond_question` 命令回传 → 工具返回答案文本。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::super::question::global_question_registry;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 全局 AppHandle（由 lib.rs setup 注入，用于 emit 事件给前端）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// ask_user 工具：模型主动向用户提问并等待回答。
pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their free-text answer. Use when you need clarification, a decision, or information only the user can provide (e.g. which option to choose, a preference, a value). Returns the user's answer text. If the user does not respond in time, returns a timeout notice."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "向用户提问并等待其自由文本回答。当你需要澄清、决策或只有用户能提供的信息时使用（如选哪个选项、偏好、某个值）。返回用户的回答文本；若用户超时未答则返回超时提示。",
            "ja" => "ユーザーに質問し、自由テキストの回答を待つ。明確化、決定、ユーザーだけが知る情報（選択肢、好み、値など）が必要な場合に使用する。ユーザーの回答テキストを返す。時間内に回答がない場合はタイムアウト通知を返す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "The question to ask the user"},
                "hint": {"type": "string", "description": "Optional answer format hint, e.g. 'a number', 'a file path', 'yes or no'"}
            },
            "required": ["prompt"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "向用户提出的问题"},
                    "hint": {"type": "string", "description": "可选的答案格式提示，如“一个数字”“文件路径”“是或否”"}
                },
                "required": ["prompt"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "ユーザーへの質問"},
                    "hint": {"type": "string", "description": "任意の回答形式ヒント（例：「数字」「ファイルパス」「はい/いいえ」）"}
                },
                "required": ["prompt"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("prompt 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let hint = args
            .get("hint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let char_id = context.char_id.clone();

        let registry = global_question_registry();
        let (question_id, rx) = registry.create_question(prompt.clone(), hint.clone(), char_id.clone());

        // 广播给前端：携带 question_id / prompt / hint / char_id，前端据此弹输入框
        if let Some(handle) = APP_HANDLE.read().as_ref() {
            let _ = handle.emit(
                "chat:question",
                json!({
                    "question_id": question_id,
                    "prompt": prompt,
                    "hint": hint,
                    "char_id": char_id,
                }),
            );
        } else {
            tracing::warn!("[ask_user] AppHandle 未注入，无法广播提问，返回超时");
            return ToolResult::standard_error(
                "无法向用户提问（后端未初始化）",
                Some("QuestionUnavailable"),
                None,
            );
        }

        // 等待回答（TTL 由注册表清理，oneshot 超时兜底）
        match tokio::time::timeout(std::time::Duration::from_secs(10 * 60), rx).await {
            Ok(Ok(answer)) => ToolResult::standard_success(
                &format!("用户回答：{}", answer),
                Some(json!({ "answer": answer })),
            ),
            Ok(Err(_)) => ToolResult::standard_error(
                "用户未回答（问题已取消或超时）",
                Some("QuestionCancelled"),
                None,
            ),
            Err(_) => ToolResult::standard_error(
                "等待用户回答超时",
                Some("QuestionTimeout"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 始终全量加载（核心交互工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "ask user question clarification"
    }
}

// 保持 Arc 导入使用（供将来若需要返回 Arc<Self> 时复用）
#[allow(dead_code)]
fn _keep_arc(_: Arc<AskUserTool>) {}
