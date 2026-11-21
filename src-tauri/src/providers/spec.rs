//! 声明式 LLM 协议规范（ProtocolSpec）
//!
//! 插件通过 `protocols/*.json` 贡献一个协议规格，核心的 `DeclarativeProvider`
//! 按此规格构造请求、解析响应与流式事件。新协议（如 Gemini Interactions API）
//! 若复用已知消息格式，可纯靠 JSON 接入，无需改 Rust。
//!
//! 设计边界：`messageFormat` 选择原生 Rust 序列化器（角色映射 / 工具调用重建 /
//! 多模态 parts），不把消息序列化写进 JSON；`request.body` 提供静态骨架 + 占位符
//! 插值，`search` / `jsonSchema` / `reasoning` 等注入以"模式枚举 + 字段名"声明。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::spec_path::PathExpr;

/// 消息序列化格式（选择原生 Rust 序列化器）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageFormat {
    /// OpenAI Chat Completions `messages[]`
    ChatCompletions,
    /// OpenAI Responses API `input[]`（OpenAI / DeepSeek / Qwen / Kimi / 豆包等兼容）
    ResponsesInput,
    /// Anthropic Messages（system 单独字段 + content blocks）
    AnthropicMessages,
    /// Gemini `contents[]`（generateContent / Interactions API 共用）
    GeminiContents,
}

/// 传输层（当前仅 REST HTTP；WebSocket 如讯飞星火保留原生 adapter）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTransport {
    pub method: Option<String>,
    /// 固定路径（如 `/v1/chat/completions`），相对 base_url
    #[serde(default)]
    pub path: Option<String>,
    /// 路径模板（`{model}` 占位，如 `/v1beta/models/{model}:generateContent`）
    #[serde(default)]
    pub path_template: Option<String>,
    /// 静态 query 参数（如 `alt=sse`）
    #[serde(default)]
    pub query_params: Vec<QueryParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParam {
    pub key: String,
    pub value: String,
}

/// 鉴权方式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProtocolAuth {
    /// `Authorization: <scheme> <key>`（或自定义 header）
    Header {
        header_name: Option<String>,
        scheme: Option<String>,
        value_from: AuthValueFrom,
    },
    /// `?<param>=<key>`（Gemini 风格）
    Query { param_name: Option<String>, value_from: AuthValueFrom },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthValueFrom {
    ApiKey,
    ApiSecret,
}

/// 联网搜索注入方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    None,
    /// 顶层 `{field}: true`（DeepSeek / Qwen / 豆包 / 通用）
    BooleanTrue,
    /// 顶层 `web_search_options={"search_context_size":"high"}`（GPT-4o）
    WebSearchOptions,
    /// `tools=[{"type":"web_search",...}]`（智谱 GLM）
    ToolsWebSearch,
    /// `tools=[{"type":"builtin_function","function":{"name":"$web_search"}}]`（Kimi）
    KimiWebSearch,
    /// `tools=[{"google_search":{}}]`（Gemini grounding）
    GeminiGoogleSearch,
}

/// JSON Schema / JSON Mode 注入方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonSchemaMode {
    #[default]
    None,
    /// `response_format={"type":"json_object"}`
    JsonObject,
    /// `response_format={"type":"json_schema","json_schema":{...}}`
    JsonSchema,
    /// `text.format={type:json_schema,...}`（Responses）
    ResponsesTextFormat,
    /// `generationConfig.responseSchema`（Gemini）
    GeminiResponseSchema,
}

/// 推理偏好注入风格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStyle {
    #[default]
    None,
    /// `apply_reasoning_preference`（Chat Completions 族：thinking / reasoning_effort / enable_thinking）
    ChatCompletions,
    /// `apply_responses_reasoning`（Responses：reasoning.effort）
    Responses,
}

/// 提示缓存策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    #[default]
    None,
    /// 顶层 `prompt_cache_key`（Kimi / Moonshot）
    PromptCacheKey,
    /// Anthropic system 块 `cache_control.ephemeral`
    CacheControl,
}

/// 工具 schema 格式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolsFormat {
    /// Chat Completions 嵌套 `{"type":"function","function":{...}}`
    #[default]
    ChatCompletions,
    /// Responses 扁平 `{"type":"function","name":...,...}`
    Responses,
    /// Anthropic `{"name":...,"input_schema":...}`
    Anthropic,
    /// Gemini `[{"function_declarations":[{...}]}]`
    GeminiDeclarations,
}

/// 非流式响应提取
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolResponse {
    /// 正文文本路径（`|join` 可拼多块）
    pub content: Option<String>,
    /// 推理文本路径（Anthropic thinking / reasoning_content）
    pub reasoning: Option<String>,
    /// finish_reason 路径
    pub finish_reason: Option<String>,
    /// usage 对象路径（非流式）
    pub usage: Option<String>,
    /// 是否对正文做 thinking 剥离（strip_thinking_segments）
    #[serde(default)]
    pub strip_thinking: bool,
}

/// 流式 chunk 字段提取
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolChunk {
    pub text: Option<String>,
    pub reasoning: Option<String>,
    /// 工具调用增量（若协议按事件分批给出 id/name/arguments，使用下述分路径）
    pub tool_index: Option<String>,
    pub tool_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_args_delta: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<String>,
}

/// 流式配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolStreaming {
    /// SSE 模式
    #[serde(default = "default_stream_mode")]
    pub mode: StreamMode,
    /// SSE 行前缀（默认 `data:`）
    #[serde(default)]
    pub event_path: Option<String>,
    /// 结束哨兵（如 `[DONE]`；None 表示无哨兵，流结束或遇到完成事件即止）
    #[serde(default)]
    pub done_sentinel: Option<String>,
    /// 每个 SSE 事件是否按其 `type` 字段路由（OpenAI Responses 风格事件名）
    #[serde(default)]
    pub event_type_field: Option<String>,
    /// 完成事件 type 值（如 `response.completed`）
    #[serde(default)]
    pub completed_event: Option<String>,
    /// 失败事件 type 值（如 `response.failed`）
    #[serde(default)]
    pub failed_event: Option<String>,
    pub chunk: ProtocolChunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    Sse,
    SseNoDone,
}

fn default_stream_mode() -> StreamMode {
    StreamMode::Sse
}

/// 能力开关（驱动 BaseProvider 的 capability 方法）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProtocolCapabilities {
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_structured_output: bool,
    #[serde(default)]
    pub supports_json_mode: bool,
    #[serde(default)]
    pub supports_thinking: bool,
    #[serde(default)]
    pub supports_vision: bool,
}

/// 前端 schema 驱动配置字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFieldDef {
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub label_key: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub help_key: Option<String>,
    #[serde(default)]
    pub placeholder_key: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub options: Vec<CustomFieldOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFieldOption {
    pub value: Value,
    #[serde(default)]
    pub label_key: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// 完整协议规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolSpec {
    pub id: String,
    pub provider_type: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub label_key: Option<String>,
    #[serde(default)]
    pub label: Option<String>,

    #[serde(default)]
    pub auth: Option<ProtocolAuth>,
    #[serde(default)]
    pub transport: Option<ProtocolTransport>,
    pub request: ProtocolRequest,
    #[serde(default)]
    pub response: ProtocolResponse,
    #[serde(default)]
    pub streaming: Option<ProtocolStreaming>,
    #[serde(default)]
    pub capabilities: ProtocolCapabilities,
    #[serde(default)]
    pub custom_fields: Vec<CustomFieldDef>,
}

/// 请求规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRequest {
    pub message_format: MessageFormat,
    /// 请求体骨架：叶子字符串为字面量或 `{placeholder}` 占位符
    /// （model / messages / temperature / max_tokens / stream / tools / instructions）
    #[serde(default)]
    pub body: Option<Value>,
    /// 消息数组字段名（默认由 message_format 决定：messages/input/contents）
    #[serde(default)]
    pub messages_field: Option<String>,
    /// max_tokens 字段名（默认 max_tokens；Responses 为 max_output_tokens）
    #[serde(default)]
    pub max_tokens_field: Option<String>,
    /// stream 布尔字段名（默认 stream）
    #[serde(default)]
    pub stream_field: Option<String>,
    /// 系统指令注入字段：Responses 用顶层 `instructions`；Anthropic 用 `system`；
    /// Chat Completions 无需（system 作为首条消息）。None 表示不注入。
    #[serde(default)]
    pub instructions_field: Option<String>,
    #[serde(default)]
    pub search: ProtocolSearch,
    #[serde(default)]
    pub json_schema: ProtocolJsonSchema,
    #[serde(default)]
    pub reasoning_style: ReasoningStyle,
    #[serde(default)]
    pub cache: ProtocolCache,
    /// 工具 schema 格式
    #[serde(default)]
    pub tools_format: ToolsFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolSearch {
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolJsonSchema {
    #[serde(default)]
    pub mode: JsonSchemaMode,
    #[serde(default)]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolCache {
    #[serde(default)]
    pub mode: CacheMode,
    #[serde(default)]
    pub field: Option<String>,
}

impl ProtocolSpec {
    /// 校验 + 预编译路径表达式。返回错误时 spec 不注册。
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("protocol spec 缺少 id".into());
        }
        if self.provider_type.is_empty() {
            return Err(format!("protocol spec `{}` 缺少 providerType", self.id));
        }
        if self.capabilities.supports_streaming && self.streaming.is_none() {
            return Err(format!(
                "protocol spec `{}` 声明 supportsStreaming 但缺少 streaming 配置",
                self.id
            ));
        }
        if self.capabilities.supports_structured_output
            && self.request.json_schema.mode == JsonSchemaMode::None
        {
            return Err(format!(
                "protocol spec `{}` 声明 structured output 但 jsonSchema.mode=none",
                self.id
            ));
        }
        if self.capabilities.supports_json_mode
            && self.request.json_schema.mode == JsonSchemaMode::None
        {
            return Err(format!(
                "protocol spec `{}` 声明 JSON mode 但 jsonSchema.mode=none",
                self.id
            ));
        }
        if self.capabilities.supports_tools && self.capabilities.supports_streaming {
            let chunk = &self.streaming.as_ref().expect("已校验").chunk;
            if chunk.tool_name.is_none()
                && chunk.tool_id.is_none()
                && chunk.tool_args_delta.is_none()
            {
                return Err(format!(
                    "protocol spec `{}` 声明流式工具能力但未配置工具增量路径",
                    self.id
                ));
            }
        }
        if let Some(method) = self
            .transport
            .as_ref()
            .and_then(|transport| transport.method.as_deref())
        {
            method.parse::<reqwest::Method>().map_err(|e| {
                format!("protocol spec `{}` 的 HTTP method 无效: {}", self.id, e)
            })?;
        }
        // 预编译所有路径表达式
        for (field, raw) in [
            ("response.content", &self.response.content),
            ("response.reasoning", &self.response.reasoning),
            ("response.finishReason", &self.response.finish_reason),
            ("response.usage", &self.response.usage),
        ] {
            if let Some(p) = raw {
                PathExpr::parse(p).map_err(|e| format!("spec `{}` 的 {} 无效: {}", self.id, field, e))?;
            }
        }
        if let Some(s) = &self.streaming {
            for (field, raw) in [
                ("streaming.chunk.text", &s.chunk.text),
                ("streaming.chunk.reasoning", &s.chunk.reasoning),
                ("streaming.chunk.toolIndex", &s.chunk.tool_index),
                ("streaming.chunk.toolId", &s.chunk.tool_id),
                ("streaming.chunk.toolName", &s.chunk.tool_name),
                ("streaming.chunk.toolArgsDelta", &s.chunk.tool_args_delta),
                ("streaming.chunk.finishReason", &s.chunk.finish_reason),
                ("streaming.chunk.usage", &s.chunk.usage),
            ] {
                if let Some(p) = raw {
                    PathExpr::parse(p)
                        .map_err(|e| format!("spec `{}` 的 {} 无效: {}", self.id, field, e))?;
                }
            }
        }
        Ok(())
    }
}

/// 编译后的（含预编译路径）spec —— 注册表实际存储的对象
#[derive(Clone)]
pub struct CompiledSpec {
    pub spec: ProtocolSpec,
    pub resp_content: Option<PathExpr>,
    pub resp_reasoning: Option<PathExpr>,
    pub resp_finish: Option<PathExpr>,
    pub resp_usage: Option<PathExpr>,
    pub chunk: CompiledChunk,
}

#[derive(Clone, Default)]
pub struct CompiledChunk {
    pub text: Option<PathExpr>,
    pub reasoning: Option<PathExpr>,
    pub tool_index: Option<PathExpr>,
    pub tool_id: Option<PathExpr>,
    pub tool_name: Option<PathExpr>,
    pub tool_args_delta: Option<PathExpr>,
    pub finish_reason: Option<PathExpr>,
    pub usage: Option<PathExpr>,
}

impl CompiledSpec {
    pub fn compile(spec: ProtocolSpec) -> Result<Self, String> {
        spec.validate()?;
        let parse = |p: &Option<String>| -> Result<Option<PathExpr>, String> {
            p.as_deref()
                .map(PathExpr::parse)
                .transpose()
                .map_err(|e| format!("spec `{}`: {}", spec.id, e))
        };
        let resp_content = parse(&spec.response.content)?;
        let resp_reasoning = parse(&spec.response.reasoning)?;
        let resp_finish = parse(&spec.response.finish_reason)?;
        let resp_usage = parse(&spec.response.usage)?;

        let mut chunk = CompiledChunk::default();
        if let Some(s) = &spec.streaming {
            chunk.text = parse(&s.chunk.text)?;
            chunk.reasoning = parse(&s.chunk.reasoning)?;
            chunk.tool_index = parse(&s.chunk.tool_index)?;
            chunk.tool_id = parse(&s.chunk.tool_id)?;
            chunk.tool_name = parse(&s.chunk.tool_name)?;
            chunk.tool_args_delta = parse(&s.chunk.tool_args_delta)?;
            chunk.finish_reason = parse(&s.chunk.finish_reason)?;
            chunk.usage = parse(&s.chunk.usage)?;
        }
        Ok(CompiledSpec {
            spec,
            resp_content,
            resp_reasoning,
            resp_finish,
            resp_usage,
            chunk,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_spec() -> ProtocolSpec {
        serde_json::from_value(json!({
            "id": "t",
            "provider_type": "chat_completions",
            "request": { "message_format": "chat_completions" }
        }))
        .unwrap()
    }

    #[test]
    fn minimal_spec_parses() {
        let s = base_spec();
        assert_eq!(s.provider_type, "chat_completions");
        assert!(s.validate().is_ok());
    }

    #[test]
    fn missing_id_errors() {
        let mut s = base_spec();
        s.id = String::new();
        assert!(s.validate().is_err());
    }

    #[test]
    fn bad_path_errors() {
        let mut s = base_spec();
        s.response.content = Some("$.a[".into());
        assert!(s.validate().is_err());
    }

    #[test]
    fn auth_parses_variants() {
        let header: ProtocolAuth =
            serde_json::from_value(json!({"type": "header", "scheme": "Bearer", "value_from": "api_key"}))
                .unwrap();
        assert!(matches!(header, ProtocolAuth::Header { .. }));
        let query: ProtocolAuth =
            serde_json::from_value(json!({"type": "query", "param_name": "key", "value_from": "api_key"}))
                .unwrap();
        assert!(matches!(query, ProtocolAuth::Query { .. }));
    }
}
