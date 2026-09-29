//! Text delivery to the app's private ChatWindow, never to external WeChat.
use std::sync::Arc;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use crate::state::AppState;
use crate::tools::types::{Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult, PermissionResult, ToolRiskTier};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));
pub fn set_app_handle(handle: AppHandle) { *APP_HANDLE.write() = Some(handle); }

/// Both proactive paths and explicit tool sends use the same history/event/banner contract.
pub fn deliver_chat_message(app: &AppHandle, char_id: &str, content: &str) -> Result<(), String> {
    let content = content.trim();
    if content.is_empty() { return Err("消息内容不能为空".into()); }
    let state = app.try_state::<Arc<AppState>>().ok_or("应用状态尚未就绪")?;
    let character = state.get_character(Some(char_id))?;
    let mut message = crate::types::response::ChatMessage::assistant(content);
    message.meta = Some(crate::messages::MessageMeta::assistant().with_channel("wechat"));
    character.brain.dialogue.add_message(message);
    let manager = &crate::conversation::CONVERSATION_MANAGER;
    if manager.is_user_session_closed(char_id) {
        manager.force_new_session("user", char_id, content);
    }
    manager.set_user_channel(char_id, "wechat");
    // Hidden/suspended windows recover from history. Never report the saved message as unsent
    // on an event error, which would invite duplicate retries.
    if let Err(e) = app.emit("chat:assistant_message", chat_payload(char_id, content)) {
        tracing::warn!("ChatWindow event failed; message retained in history: {e}");
    }
    let visible = app.get_webview_window("chat")
        .and_then(|win| win.is_visible().ok()).unwrap_or(false);
    if !visible {
        let preview: String = content.chars().take(60).collect();
        crate::commands::window::emit_message_banner(app, json!({
            "character_id": char_id, "preview": preview, "kind": "proactive",
            "timestamp": chrono::Local::now().timestamp() as f64,
        }));
        crate::remote::push_toast("proactive", "智能体消息", &preview, char_id, json!({"kind":"proactive"}));
    }
    Ok(())
}

fn chat_payload(char_id: &str, content: &str) -> Value {
    json!({"character_id":char_id,"content":content,"channel":"wechat",
        "timestamp":chrono::Local::now().to_rfc3339()})
}

pub struct SendChatMessageTool;
#[async_trait]
impl Tool for SendChatMessageTool {
    fn name(&self) -> &str { "send_chat_message" }
    fn description(&self) -> &str {
        "Send a private text to this user's in-app 微信 / ChatWindow as yourself. Use when the user asks to send something to 微信, leave it in private chat, or for an independent new topic. Never switch an ongoing face-to-face topic to text unless the user explicitly requests it. Compose in natural texting language. This really writes and delivers the message even if ChatWindow is closed. Not external WeChat, not another person. Do not call for ordinary replies already being sent in the current chat, and do not repeat the sent text in your spoken reply."
    }
    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "以当前角色身份向用户的应用内微信（ChatWindow）发送私聊文字。用户说‘发我微信’‘放私聊里’‘给我留条消息’，或开启独立新话题时使用。同一面对面话题禁止自行切换私聊，除非用户明确要求；消息应按线上聊天语气重新组织。窗口关闭也能收到。不是向外部微信或其他联系人发送。当前微信对话的普通回复直接回答即可，不要重复调用；发送成功后不要再在桌宠气泡复述全文。",
            _ => self.description(),
        }
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"content":{"type":"string","description":"The complete message to send to the user's private ChatWindow."}},"required":["content"],"additionalProperties":false})
    }
    async fn validate_input(&self, input: &Value, _: &ToolUseContext) -> ValidationResult {
        match message_content(input) {
            Ok(_) => ValidationResult::success(None),
            Err(error) => ValidationResult::failure(error, 2),
        }
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult { PermissionResult::allow() }
    fn risk(&self) -> ToolRiskTier { ToolRiskTier::Safe }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let content = match message_content(&args) {
            Ok(content) => content,
            Err(error) => return ToolResult::standard_error(error, None, None),
        };
        // Never silently send as another character when context is missing.
        if ctx.char_id.is_empty() {
            return ToolResult::standard_error("缺少当前角色，消息未发送", None, None);
        }
        let Some(app) = APP_HANDLE.read().clone() else {
            return ToolResult::standard_error("应用尚未就绪，消息未发送", None, None);
        };
        match deliver_chat_message(&app, &ctx.char_id, content) {
            Ok(()) => ToolResult::standard_success("已发送到应用内微信私聊。无需复述全文。", Some(json!({"sent":true,"channel":"wechat","character_id":ctx.char_id}))),
            Err(error) => ToolResult::standard_error(&error, None, None),
        }
    }
    fn is_read_only(&self) -> bool { false }
    fn category(&self) -> ToolCategory { ToolCategory::Pet }
    fn always_load(&self) -> bool { true }
    fn search_hint(&self) -> &str { "发我微信 私聊 留言 聊天窗口 send text private chat wechat ChatWindow" }
}

fn message_content(input: &Value) -> Result<&str, &'static str> {
    let content = input.get("content").and_then(Value::as_str).unwrap_or("").trim();
    if content.is_empty() { return Err("消息内容不能为空"); }
    if content.chars().count() > 8000 { return Err("消息过长，请分成有意义的短消息"); }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_payload_has_explicit_character_channel_and_time() {
        let payload = chat_payload("nana", "留给你稍后看");
        assert_eq!(payload["channel"], "wechat");
        assert_eq!(payload["character_id"], "nana");
        assert_eq!(payload["content"], "留给你稍后看");
        assert!(payload["timestamp"].as_str().is_some());
    }
    #[test]
    fn reject_empty_or_non_text_messages() {
        assert!(message_content(&json!({"content":"  "})).is_err());
        assert!(message_content(&json!({"content":12})).is_err());
        assert!(message_content(&json!({"content":"x".repeat(8001)})).is_err());
        assert_eq!(message_content(&json!({"content":" 可以晚点看 "})).unwrap(), "可以晚点看");
    }
}
