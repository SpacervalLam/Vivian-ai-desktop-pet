//! 消息系统 — 结构化消息类型 + 系统提示消息模板
//!
//! - 内容块：`TextContentBlock` / `ImageContentBlock` / `ToolCallBlock` /
//!   `ToolResultBlock` / `ReasoningContentBlock`
//! - 消息类型层级：`BaseMessage` / `SystemMessage` / `HumanMessage` /
//!   `AIMessage` / `ToolMessage`
//! - 工厂函数与转换工具
//!
//! 额外提供（任务要求）：
//! - 系统提示消息模板库（启动问候 / 错误提示 / 操作确认）
//! - 多语言支持（与 `i18n` 模块协作）
//!
//! 与 `types::response::ChatMessage`（router/brain 使用的简化消息）互转：
//! `ChatMessage` 是面向 LLM provider 的纯文本载体，本模块提供更丰富的多模态结构。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ────────────────────────────────────────────────────────────────
// 镜像消息系统 — 内容来源标记
// ────────────────────────────────────────────────────────────────

/// 消息来源类型
///
/// 标记每条消息的真实来源，让记忆系统据此决定是否纳入记忆。
/// - `User` / `Assistant`：正常对话路径，进入记忆
/// - `Tool`：工具执行结果，不抽取为用户事实
/// - `InnerMonologue`：Vivian 的内心独白，不进入对话记忆
/// - `Mirror`：外部控制器注入的内容（插件/游戏/Agent 回调），默认不进入记忆
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    #[default]
    User,
    Assistant,
    Tool,
    InnerMonologue,
    Mirror,
}

impl MessageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageSource::User => "user",
            MessageSource::Assistant => "assistant",
            MessageSource::Tool => "tool",
            MessageSource::InnerMonologue => "inner_monologue",
            MessageSource::Mirror => "mirror",
        }
    }

    /// 是否为可进入记忆的来源（用户真实发言或 LLM 生成的回复）
    pub fn is_memory_eligible(self) -> bool {
        matches!(self, MessageSource::User | MessageSource::Assistant)
    }
}

/// 消息元数据 — 标记内容来源与记忆策略
///
/// 外部控制器在聊天气泡位置注入内容时，标记为非 LLM 生成，
/// memory 系统据此决定是否纳入记忆。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageMeta {
    /// 内容来源
    #[serde(default)]
    pub source: MessageSource,
    /// 是否禁止进入记忆系统（true 则 AutoExtractor / UserFactStore 跳过）
    #[serde(default)]
    pub is_memory_disabled: bool,
    /// 镜像消息的外部控制器标签（如 "game" / "plugin"），仅 source=Mirror 时有意义
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_kind: Option<String>,
    /// 消息渠道（"wechat" 聊天面板 / "direct" 直接说话），影响 LLM 回复风格
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// 内容类型标记（"file" / "image"），由后端写入，不可被前端伪造。
    /// 用于区分文件/图片消息与纯文本，区别于用户手动输入的 `[文件：...]` 前缀。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl MessageMeta {
    pub fn new(source: MessageSource) -> Self {
        let is_memory_disabled = !source.is_memory_eligible();
        Self {
            source,
            is_memory_disabled,
            mirror_kind: None,
            channel: None,
            kind: None,
        }
    }

    pub fn with_mirror_kind(mut self, kind: impl Into<String>) -> Self {
        self.mirror_kind = Some(kind.into());
        self
    }

    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// 便捷构造：用户消息
    pub fn user() -> Self {
        Self::new(MessageSource::User)
    }

    /// 便捷构造：助手消息
    pub fn assistant() -> Self {
        Self::new(MessageSource::Assistant)
    }

    /// 便捷构造：工具结果消息（默认不进记忆）
    pub fn tool() -> Self {
        Self::new(MessageSource::Tool)
    }

    /// 便捷构造：内心独白消息（默认不进记忆）
    pub fn inner_monologue() -> Self {
        Self::new(MessageSource::InnerMonologue)
    }

    /// 便捷构造：镜像消息（默认不进记忆）
    pub fn mirror(kind: impl Into<String>) -> Self {
        Self::new(MessageSource::Mirror).with_mirror_kind(kind)
    }
}

// ────────────────────────────────────────────────────────────────
// 内容块定义
// ────────────────────────────────────────────────────────────────

/// 纯文本内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContentBlock {
    #[serde(default = "default_text_type")]
    pub r#type: String,
    pub text: String,
}

fn default_text_type() -> String {
    "text".to_string()
}

impl TextContentBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            r#type: "text".to_string(),
            text: text.into(),
        }
    }
}

/// 图片内容块 — 支持 URL 和 base64 data URI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContentBlock {
    #[serde(default = "default_image_type")]
    pub r#type: String,
    pub image_url: String,
    #[serde(default = "default_detail")]
    pub detail: String,
}

fn default_image_type() -> String {
    "image_url".to_string()
}

fn default_detail() -> String {
    "auto".to_string()
}

impl ImageContentBlock {
    pub fn new(image_url: impl Into<String>) -> Self {
        Self {
            r#type: "image_url".to_string(),
            image_url: image_url.into(),
            detail: "auto".to_string(),
        }
    }
}

/// 工具调用请求块 — LLM 决定调用工具
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallBlock {
    #[serde(default = "default_tool_call_type")]
    pub r#type: String,
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

fn default_tool_call_type() -> String {
    "tool_call".to_string()
}

impl ToolCallBlock {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            r#type: "tool_call".to_string(),
            id: short_uuid(),
            name: name.into(),
            arguments,
        }
    }
}

/// 工具执行结果块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBlock {
    #[serde(default = "default_tool_result_type")]
    pub r#type: String,
    pub tool_call_id: String,
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_tool_result_type() -> String {
    "tool_result".to_string()
}

impl ToolResultBlock {
    pub fn new(tool_call_id: impl Into<String>, content: Value) -> Self {
        Self {
            r#type: "tool_result".to_string(),
            tool_call_id: tool_call_id.into(),
            content,
            error: None,
        }
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

/// 推理过程块（思维链 / CoT）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningContentBlock {
    #[serde(default = "default_reasoning_type")]
    pub r#type: String,
    pub reasoning: String,
}

fn default_reasoning_type() -> String {
    "reasoning".to_string()
}

impl ReasoningContentBlock {
    pub fn new(reasoning: impl Into<String>) -> Self {
        Self {
            r#type: "reasoning".to_string(),
            reasoning: reasoning.into(),
        }
    }
}

/// 内容块联合（用枚举封装以便序列化/反序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text(TextContentBlock),
    #[serde(rename = "image_url")]
    Image(ImageContentBlock),
    #[serde(rename = "tool_call")]
    ToolCall(ToolCallBlock),
    #[serde(rename = "tool_result")]
    ToolResult(ToolResultBlock),
    #[serde(rename = "reasoning")]
    Reasoning(ReasoningContentBlock),
}

impl ContentBlock {
    /// 转换为 LLM API 兼容的 JSON 表示
    pub fn to_api_json(&self) -> Value {
        match self {
            ContentBlock::Text(b) => json!({"type": "text", "text": b.text}),
            ContentBlock::Image(b) => json!({
                "type": "image_url",
                "image_url": {"url": b.image_url, "detail": b.detail}
            }),
            ContentBlock::ToolCall(b) => json!({
                "type": "tool_call",
                "id": b.id,
                "name": b.name,
                "arguments": b.arguments,
            }),
            ContentBlock::ToolResult(b) => json!({
                "type": "tool_result",
                "tool_call_id": b.tool_call_id,
                "content": b.content,
                "error": b.error,
            }),
            ContentBlock::Reasoning(b) => json!({
                "type": "reasoning",
                "reasoning": b.reasoning,
            }),
        }
    }

    /// 提取纯文本表示（用于日志 / 历史回放）
    pub fn to_text(&self) -> String {
        match self {
            ContentBlock::Text(b) => b.text.clone(),
            ContentBlock::Reasoning(b) => format!("[推理: {}]", b.reasoning),
            ContentBlock::ToolCall(b) => {
                format!("[调用工具: {}({})]", b.name, b.arguments)
            }
            ContentBlock::ToolResult(b) => {
                let s = b.content.to_string();
                let truncated = if s.len() > 200 {
                    format!("{}...", &s[..200])
                } else {
                    s
                };
                format!("[工具结果: {}]", truncated)
            }
            ContentBlock::Image(b) => format!("[图片: {}]", b.image_url),
        }
    }
}

// ────────────────────────────────────────────────────────────────
// 消息基类与子类
// ────────────────────────────────────────────────────────────────

/// 消息基类 — 支持 content_blocks 多模态内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseMessage {
    pub role: String,
    /// 纯文本内容（向后兼容）
    #[serde(default)]
    pub content: String,
    /// 结构化内容块列表（多模态支持）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_blocks: Vec<ContentBlock>,
    /// 消息唯一 ID
    pub id: String,
    /// 创建时间戳（Unix 秒）
    pub timestamp: f64,
    /// 附加元数据
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, Value>,
}

impl BaseMessage {
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: String::new(),
            content_blocks: Vec::new(),
            id: short_uuid(),
            timestamp: now_ts(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content = text.into();
        self
    }

    pub fn with_blocks(mut self, blocks: Vec<ContentBlock>) -> Self {
        self.content_blocks = blocks;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// 获取纯文本表示 — 遍历 content_blocks 提取文字
    pub fn get_text(&self) -> String {
        if !self.content.is_empty() {
            return self.content.clone();
        }
        let texts: Vec<String> = self
            .content_blocks
            .iter()
            .map(|b| b.to_text())
            .collect();
        texts.join("\n")
    }

    /// 转换为 LLM API 兼容的消息格式
    pub fn to_api_format(&self) -> Value {
        if !self.content_blocks.is_empty() {
            let api_content: Vec<Value> = self
                .content_blocks
                .iter()
                .map(|b| b.to_api_json())
                .collect();
            json!({"role": self.role, "content": api_content})
        } else {
            json!({"role": self.role, "content": self.get_text()})
        }
    }
}

/// 系统消息 — 系统指令 / 上下文注入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage(pub BaseMessage);

impl SystemMessage {
    pub fn new(text: impl Into<String>) -> Self {
        let mut base = BaseMessage::new("system");
        base.content = text.into();
        Self(base)
    }
    pub fn base(&self) -> &BaseMessage {
        &self.0
    }
    pub fn into_base(self) -> BaseMessage {
        self.0
    }
}

/// 用户消息 — 支持多模态（文本 + 图片）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanMessage(pub BaseMessage);

impl HumanMessage {
    pub fn new(text: impl Into<String>) -> Self {
        let mut base = BaseMessage::new("user");
        base.content = text.into();
        Self(base)
    }

    pub fn with_images(mut self, images: Vec<String>) -> Self {
        let mut blocks = vec![ContentBlock::Text(TextContentBlock::new(self.0.content.clone()))];
        for img in images {
            blocks.push(ContentBlock::Image(ImageContentBlock::new(img)));
        }
        self.0.content_blocks = blocks;
        self
    }

    pub fn base(&self) -> &BaseMessage {
        &self.0
    }
    pub fn into_base(self) -> BaseMessage {
        self.0
    }
}

/// AI 回复 — 支持 tool_calls + reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIMessage {
    pub base: BaseMessage,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl AIMessage {
    pub fn new(text: impl Into<String>) -> Self {
        let mut base = BaseMessage::new("assistant");
        base.content = text.into();
        Self {
            base,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }

    pub fn with_tool_calls(mut self, calls: Vec<ToolCallBlock>) -> Self {
        self.tool_calls = calls;
        self
    }

    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning_content = Some(reasoning.into());
        self
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// 获取工具调用列表（API 兼容格式）
    pub fn get_tool_calls_api(&self) -> Vec<Value> {
        self.tool_calls
            .iter()
            .map(|tc| {
                json!({
                    "id": if tc.id.is_empty() { short_uuid() } else { tc.id.clone() },
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    }
                })
            })
            .collect()
    }
}

/// 工具消息 — 工具执行结果反馈给 LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMessage {
    pub base: BaseMessage,
    pub tool_call_id: String,
    pub tool_call_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolMessage {
    pub fn new(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        let mut base = BaseMessage::new("tool");
        base.content = content.into();
        Self {
            base,
            tool_call_id: tool_call_id.into(),
            tool_call_name: String::new(),
            error: None,
            is_error: false,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.tool_call_name = name.into();
        self
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        let err = error.into();
        self.is_error = !err.is_empty();
        self.error = if self.is_error { Some(err) } else { None };
        self
    }

    pub fn to_api_format(&self) -> Value {
        json!({
            "role": "tool",
            "tool_call_id": self.tool_call_id,
            "content": self.base.content,
        })
    }
}

// ────────────────────────────────────────────────────────────────
// 工厂函数
// ────────────────────────────────────────────────────────────────

/// 创建用户消息（可能含图片）
pub fn create_human_message(text: &str, images: Option<Vec<String>>) -> HumanMessage {
    let msg = HumanMessage::new(text);
    match images {
        Some(imgs) if !imgs.is_empty() => msg.with_images(imgs),
        _ => msg,
    }
}

/// 创建系统消息
pub fn create_system_message(text: &str) -> SystemMessage {
    SystemMessage::new(text)
}

/// 创建 AI 回复消息
pub fn create_ai_message(
    text: &str,
    tool_calls: Option<Vec<ToolCallBlock>>,
    reasoning: Option<&str>,
) -> AIMessage {
    let mut msg = AIMessage::new(text);
    if let Some(calls) = tool_calls {
        msg = msg.with_tool_calls(calls);
    }
    if let Some(r) = reasoning {
        msg = msg.with_reasoning(r);
    }
    msg
}

/// 创建工具执行结果消息
pub fn create_tool_message(
    content: &str,
    tool_call_id: &str,
    tool_name: &str,
    error: Option<&str>,
) -> ToolMessage {
    let mut msg = ToolMessage::new(content, tool_call_id).with_name(tool_name);
    if let Some(err) = error {
        msg = msg.with_error(err);
    }
    msg
}

/// 将消息列表转换为 LLM API 兼容格式
pub fn messages_to_api_format(messages: &[BaseMessage]) -> Vec<Value> {
    messages.iter().map(|m| m.to_api_format()).collect()
}

// ────────────────────────────────────────────────────────────────
// 系统提示消息模板（任务要求 — 多语言支持）
// ────────────────────────────────────────────────────────────────

/// 系统提示消息键（与 i18n 模块的 key 对齐）
pub mod templates {
    pub const STARTUP_GREETING_FIRST: &str = "messages.startup.greeting_first";
    pub const STARTUP_GREETING_RETURN: &str = "messages.startup.greeting_return";
    pub const ERROR_GENERIC: &str = "messages.error.generic";
    pub const ERROR_NETWORK: &str = "messages.error.network";
    pub const ERROR_LLM: &str = "messages.error.llm";
    pub const ERROR_TOOL: &str = "messages.error.tool";
    pub const OP_CONFIRM: &str = "messages.op.confirm";
    pub const OP_SAVED: &str = "messages.op.saved";
    pub const OP_CLEARED: &str = "messages.op.cleared";
    pub const OP_DELETED: &str = "messages.op.deleted";
    pub const OP_CANCELLED: &str = "messages.op.cancelled";
}

/// 默认中文模板（与 i18n 模块协作时可作为 fallback）
pub fn default_template(key: &str) -> Option<&'static str> {
    match key {
        templates::STARTUP_GREETING_FIRST => Some("你好呀~我是 Vivian，很高兴认识你！"),
        templates::STARTUP_GREETING_RETURN => Some("欢迎回来~今天过得怎么样？"),
        templates::ERROR_GENERIC => Some("出错了：{error}"),
        templates::ERROR_NETWORK => Some("网络连接异常，请稍后再试。{detail}"),
        templates::ERROR_LLM => Some("AI 思考时遇到问题：{detail}"),
        templates::ERROR_TOOL => Some("工具执行失败：{tool} - {detail}"),
        templates::OP_CONFIRM => Some("确认要执行此操作吗？"),
        templates::OP_SAVED => Some("已保存。"),
        templates::OP_CLEARED => Some("已清空。"),
        templates::OP_DELETED => Some("已删除。"),
        templates::OP_CANCELLED => Some("已取消。"),
        _ => None,
    }
}

/// 渲染系统提示消息（简单 `{name}` 占位符替换）
///
/// 与 `i18n::I18n::t` 协作：先查 i18n，未命中则回退到内置默认模板。
pub fn render(template_key: &str, i18n: Option<&crate::i18n::I18n>, params: &HashMap<String, String>) -> String {
    let raw = i18n
        .and_then(|i| {
            let t = i.t(template_key);
            if t == template_key {
                None
            } else {
                Some(t)
            }
        })
        .or_else(|| default_template(template_key).map(|s| s.to_string()))
        .unwrap_or_else(|| template_key.to_string());

    let mut result = raw.clone();
    for (k, v) in params {
        let placeholder = format!("{{{}}}", k);
        result = result.replace(&placeholder, v);
    }
    result
}

// ────────────────────────────────────────────────────────────────
// 工具函数
// ────────────────────────────────────────────────────────────────

fn short_uuid() -> String {
    uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("00000000")
        .to_string()
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ────────────────────────────────────────────────────────────────
// 与现有 ChatMessage 互转
// ────────────────────────────────────────────────────────────────

impl From<crate::types::response::ChatMessage> for BaseMessage {
    fn from(cm: crate::types::response::ChatMessage) -> Self {
        let mut base = BaseMessage::new(cm.role);
        base.content = cm.content;
        if let Some(ts) = cm.timestamp {
            base.timestamp = ts.timestamp() as f64;
        }
        base
    }
}

impl From<BaseMessage> for crate::types::response::ChatMessage {
    fn from(bm: BaseMessage) -> Self {
        let timestamp = chrono::DateTime::from_timestamp(bm.timestamp as i64, 0)
            .map(|dt| dt.with_timezone(&chrono::Local));
        let content = bm.get_text();
        Self {
            role: bm.role,
            content,
            timestamp,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            reasoning: None,
            meta: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_block() {
        let block = TextContentBlock::new("hello");
        assert_eq!(block.text, "hello");
    }

    #[test]
    fn test_human_message() {
        let msg = HumanMessage::new("你好");
        assert_eq!(msg.0.role, "user");
        assert_eq!(msg.0.content, "你好");
    }

    #[test]
    fn test_human_message_with_images() {
        let msg = HumanMessage::new("看这张图").with_images(vec!["https://example.com/x.png".to_string()]);
        assert_eq!(msg.0.content_blocks.len(), 2);
        assert!(matches!(msg.0.content_blocks[0], ContentBlock::Text(_)));
        assert!(matches!(msg.0.content_blocks[1], ContentBlock::Image(_)));
    }

    #[test]
    fn test_ai_message_tool_calls() {
        let call = ToolCallBlock::new("open_app", json!({"name": "notepad"}));
        let msg = AIMessage::new("好的").with_tool_calls(vec![call]);
        assert!(msg.has_tool_calls());
        let api = msg.get_tool_calls_api();
        assert_eq!(api.len(), 1);
        assert_eq!(api[0]["type"], "function");
    }

    #[test]
    fn test_to_api_format_multimodal() {
        let msg = HumanMessage::new("看图")
            .with_images(vec!["https://example.com/x.png".to_string()]);
        let api = msg.0.to_api_format();
        assert_eq!(api["role"], "user");
        assert!(api["content"].is_array());
        assert_eq!(api["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_render_default_template() {
        let mut params = HashMap::new();
        params.insert("error".to_string(), "网络断了".to_string());
        let s = render(templates::ERROR_GENERIC, None, &params);
        assert!(s.contains("网络断了"));
    }

    #[test]
    fn test_render_unknown_key() {
        let s = render("nonexistent.key", None, &HashMap::new());
        assert_eq!(s, "nonexistent.key");
    }

    #[test]
    fn test_chat_message_roundtrip() {
        let cm = crate::types::response::ChatMessage::user("hello");
        let bm: BaseMessage = cm.into();
        assert_eq!(bm.role, "user");
        assert_eq!(bm.content, "hello");
        let cm2: crate::types::response::ChatMessage = bm.into();
        assert_eq!(cm2.role, "user");
        assert_eq!(cm2.content, "hello");
    }

    // ── 镜像消息系统测试 ──

    #[test]
    fn test_message_source_memory_eligible() {
        assert!(MessageSource::User.is_memory_eligible());
        assert!(MessageSource::Assistant.is_memory_eligible());
        assert!(!MessageSource::Tool.is_memory_eligible());
        assert!(!MessageSource::InnerMonologue.is_memory_eligible());
        assert!(!MessageSource::Mirror.is_memory_eligible());
    }

    #[test]
    fn test_message_source_default_is_user() {
        let s = MessageSource::default();
        assert_eq!(s, MessageSource::User);
    }

    #[test]
    fn test_message_meta_user_not_disabled() {
        let m = MessageMeta::user();
        assert!(!m.is_memory_disabled);
        assert_eq!(m.source, MessageSource::User);
    }

    #[test]
    fn test_message_meta_assistant_not_disabled() {
        let m = MessageMeta::assistant();
        assert!(!m.is_memory_disabled);
    }

    #[test]
    fn test_message_meta_tool_disabled() {
        let m = MessageMeta::tool();
        assert!(m.is_memory_disabled);
        assert_eq!(m.source, MessageSource::Tool);
    }

    #[test]
    fn test_message_meta_inner_monologue_disabled() {
        let m = MessageMeta::inner_monologue();
        assert!(m.is_memory_disabled);
    }

    #[test]
    fn test_message_meta_mirror_with_kind() {
        let m = MessageMeta::mirror("game");
        assert!(m.is_memory_disabled);
        assert_eq!(m.source, MessageSource::Mirror);
        assert_eq!(m.mirror_kind.as_deref(), Some("game"));
    }

    #[test]
    fn test_chat_message_is_memory_disabled_default() {
        let cm = crate::types::response::ChatMessage::user("hello");
        assert!(!cm.is_memory_disabled());
    }

    #[test]
    fn test_chat_message_is_memory_disabled_tool_result() {
        let cm = crate::types::response::ChatMessage::tool_result("result", "call_1");
        assert!(cm.is_memory_disabled());
    }

    #[test]
    fn test_chat_message_with_source_mirror() {
        let cm = crate::types::response::ChatMessage::assistant("系统通知")
            .with_source(MessageSource::Mirror);
        assert!(cm.is_memory_disabled());
    }

    #[test]
    fn test_chat_message_with_source_user() {
        let cm = crate::types::response::ChatMessage::user("hi")
            .with_source(MessageSource::User);
        assert!(!cm.is_memory_disabled());
    }

    #[test]
    fn test_message_meta_serialization_roundtrip() {
        let m = MessageMeta::mirror("plugin");
        let json = serde_json::to_string(&m).unwrap();
        let m2: MessageMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.source, MessageSource::Mirror);
        assert!(m2.is_memory_disabled);
        assert_eq!(m2.mirror_kind.as_deref(), Some("plugin"));
    }
}
