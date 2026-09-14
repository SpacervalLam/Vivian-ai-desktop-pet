//! Pipeline 输出解析器 —— 将 LLM 响应解析为结构化数据 / PipelineState 字段。
//!
//! - [`BaseOutputParser`] trait：解析器接口（同时实现 [`Runnable`]）
//! - [`JsonOutputParser`]：包装 [`JsonParser`] / [`JsonProcessor`]，提取 JSON
//! - [`SchemaOutputParser<T>`]：泛型 schema 解析（Rust 用 `serde::DeserializeOwned` 替代 Pydantic）
//! - [`StateFieldParser`]：将解析结果填充到 [`PipelineState`] 字段
//!   （motion / expression / tool_calls / importance / intent / long_term_memory）
//! - [`StreamingOutputParser`]：基于 [`StreamingJsonParser`] 的流式增量解析
//!
//! 错误恢复策略（解析失败时使用默认值，不中断流程）：
//! - `JsonOutputParser` 解析失败时返回 `Value::Null` 或包装原文的 `{"text": raw}`
//! - `StateFieldParser` 仅更新能解析的字段，未命中字段保持原值
//! - `StreamingOutputParser` 单块失败时发出 `error` 事件，不中断后续 feed

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::brain::json_parser::{
    JsonParser, JsonProcessor, ProcessedResponse, StreamingJsonParser,
    StreamEvent as JsonStreamEvent,
};
use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig, StreamEvent};
use crate::pipeline::state::PipelineState;

// ============================================================================
// 工具函数
// ============================================================================

/// 从 `Value` 中提取待解析的文本。
///
/// 支持三种输入形态：
/// - 字符串：直接返回
/// - JSON 对象：依次尝试 `text` / `response_text` / `content` / `output` 字段
/// - 其他：返回 `None`（调用方应原样返回输入）
pub fn extract_text_from_value(input: &Value) -> Option<String> {
    if let Some(s) = input.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = input.as_object() {
        for key in &["text", "response_text", "content", "output"] {
            if let Some(Value::String(s)) = obj.get(*key) {
                if !s.is_empty() {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

/// 解析失败时的默认值 —— 包装原文为 `{"text": raw}`。
pub fn default_wrapped(raw: &str) -> Value {
    json!({
        "text": raw.trim(),
        "motion": "idle",
        "expression": "star_eyes",
    })
}

// ============================================================================
// BaseOutputParser trait
// ============================================================================

/// 输出解析器 trait。
///
/// 解析器本身是 [`Runnable`]，可直接插入管道（`LLM | Parser`）。
/// 子类型需实现 `parse` / `aparse`；`ainvoke` 由 blanket 实现提供，
/// 自动从 `Value` 中提取文本并调用 `aparse`。
#[async_trait]
pub trait BaseOutputParser: Send + Sync {
    /// 解析器名称（用于日志 / 流事件）
    fn name(&self) -> &str {
        "OutputParser"
    }

    /// 同步解析 —— 将 LLM 输出文本解析为结构化 `Value`。
    fn parse(&self, text: &str) -> VivianResult<Value>;

    /// 异步解析 —— 默认委托给 `parse`。
    async fn aparse(&self, text: &str) -> VivianResult<Value> {
        self.parse(text)
    }

    /// 从 `Value` 提取文本后解析。
    ///
    /// 输入非字符串 / 不含 text 字段时，原样返回输入（不报错）。
    async fn parse_value(&self, input: Value) -> VivianResult<Value> {
        match extract_text_from_value(&input) {
            Some(text) => self.aparse(&text).await,
            None => Ok(input),
        }
    }
}

// ============================================================================
// JsonOutputParser
// ============================================================================

/// JSON 输出解析器。
///
/// 优先使用 [`JsonProcessor`]（带工具调用提取与重要性钳制）；
/// 未注入时回退到 [`JsonParser::parse_single`]（6 阶段鲁棒解析）。
///
/// 解析失败时返回 `default_wrapped(raw)`，不传播错误（graceful degradation）。
pub struct JsonOutputParser {
    /// 可选的 JsonProcessor（带 LLM 响应处理逻辑）
    json_processor: Option<Arc<JsonProcessor>>,
    /// 解析器名称
    name: String,
}

impl JsonOutputParser {
    pub fn new() -> Self {
        Self {
            json_processor: None,
            name: "JsonOutputParser".to_string(),
        }
    }

    pub fn with_processor(processor: Arc<JsonProcessor>) -> Self {
        Self {
            json_processor: Some(processor),
            name: "JsonOutputParser".to_string(),
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl Default for JsonOutputParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseOutputParser for JsonOutputParser {
    fn name(&self) -> &str {
        &self.name
    }

    fn parse(&self, text: &str) -> VivianResult<Value> {
        if let Some(processor) = &self.json_processor {
            // 使用 JsonProcessor：返回 ProcessedResponse 序列化结果
            let processed = processor.process_response(text);
            return serde_json::to_value(processed)
                .map_err(|e| VivianError::Serialization(e.to_string()));
        }
        // 回退：JsonParser 单对象解析
        match JsonParser::parse_single(text) {
            Ok(v) => Ok(v),
            Err(_) => {
                tracing::warn!("[JsonOutputParser] JSON 解析失败，返回默认包装");
                Ok(default_wrapped(text))
            }
        }
    }

    async fn aparse(&self, text: &str) -> VivianResult<Value> {
        // Rust 的 serde 解析足够快，直接调用 parse
        self.parse(text)
    }
}

#[async_trait]
impl Runnable for JsonOutputParser {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        self.parse_value(input).await
    }

    async fn astream_events(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Vec<StreamEvent>> {
        let run_id = config
            .as_ref()
            .map(|c| c.run_id.clone())
            .unwrap_or_default();
        let metadata = if run_id.is_empty() {
            Value::Null
        } else {
            json!({ "run_id": run_id })
        };
        let parsed = self.parse_value(input).await?;
        Ok(vec![StreamEvent::new("text_done", parsed.to_string()).with_metadata(metadata)])
    }
}

// ============================================================================
// SchemaOutputParser<T> —— PydanticOutputParser 的 Rust 等价物
// ============================================================================

/// 泛型 schema 解析器。
///
/// Rust 用 `serde::DeserializeOwned` 替代 Pydantic：
/// - 先用 [`JsonParser::parse_single`] 提取 JSON
/// - 再用 `serde_json::from_value::<T>` 反序列化为目标类型
///
/// 解析失败时返回 `Err`（调用方可通过 `parse_or_default` 降级）。
pub struct SchemaOutputParser<T: DeserializeOwned + Serialize> {
    json_processor: Option<Arc<JsonProcessor>>,
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned + Serialize> SchemaOutputParser<T> {
    pub fn new() -> Self {
        Self {
            json_processor: None,
            _phantom: PhantomData,
        }
    }

    pub fn with_processor(processor: Arc<JsonProcessor>) -> Self {
        Self {
            json_processor: Some(processor),
            _phantom: PhantomData,
        }
    }

    /// 解析为目标类型 `T`。
    pub fn parse_typed(&self, text: &str) -> VivianResult<T> {
        let value = if let Some(p) = &self.json_processor {
            let processed = p.process_response(text);
            serde_json::to_value(processed)
                .map_err(|e| VivianError::Serialization(e.to_string()))?
        } else {
            JsonParser::parse_single(text)?
        };
        serde_json::from_value::<T>(value).map_err(|e| {
            VivianError::Serialization(format!("Schema 反序列化失败: {}", e))
        })
    }
}

impl<T: DeserializeOwned + Serialize + Send + Sync> Default for SchemaOutputParser<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: DeserializeOwned + Serialize + Send + Sync + 'static> BaseOutputParser
    for SchemaOutputParser<T>
{
    fn name(&self) -> &str {
        "SchemaOutputParser"
    }

    fn parse(&self, text: &str) -> VivianResult<Value> {
        // BaseOutputParser 要求返回 Value；这里先 parse_typed 再转回 Value
        let typed = self.parse_typed(text)?;
        serde_json::to_value(typed).map_err(|e| VivianError::Serialization(e.to_string()))
    }
}

#[async_trait]
impl<T: DeserializeOwned + Serialize + Send + Sync + 'static> Runnable
    for SchemaOutputParser<T>
{
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        self.parse_value(input).await
    }
}

// ============================================================================
// StateFieldParser —— 将解析结果填充到 PipelineState 字段
// ============================================================================

/// PipelineState 字段填充器。
///
/// 从 LLM 响应 JSON 中提取以下字段并写入 [`PipelineState`]：
/// - `text` / `motion` / `expression`
/// - `importance_user` / `importance_ai`
/// - `intent`（reply / short_reply / no_reply）
/// - `tool_calls`
/// - `long_term_memory`
///
/// 特殊处理：
/// - `intent=no_reply` 时把 `text` 置空（不展示回复）
/// - 字段缺失时保持原值（不覆盖）
/// - 解析失败时使用 `response_text` 兜底
pub struct StateFieldParser {
    /// 可选的 JsonProcessor（注入后启用完整 ProcessedResponse 路径）
    json_processor: Option<Arc<JsonProcessor>>,
}

impl StateFieldParser {
    pub fn new() -> Self {
        Self {
            json_processor: None,
        }
    }

    pub fn with_processor(processor: Arc<JsonProcessor>) -> Self {
        Self {
            json_processor: Some(processor),
        }
    }

    /// 从 `ProcessedResponse` 提取字段到 `PipelineState`。
    pub fn apply_processed(state: &mut PipelineState, processed: &ProcessedResponse) {
        state.text = processed.text.clone();

        // intent 仅接受三种合法值
        let raw_intent = processed.intent.as_str();
        if matches!(raw_intent, "reply" | "short_reply" | "no_reply") {
            state.intent = raw_intent.to_string();
        }

        // no_reply → 清空 text（不展示回复）
        if state.intent == "no_reply" {
            state.text = String::new();
            tracing::debug!("[StateFieldParser] intent=no_reply，清空 text");
        }

        // tool_calls 同步（仅当 state 中为空时才覆盖）
        if !processed.tool_calls.is_empty() && state.tool_calls.is_empty() {
            state.tool_calls = processed.tool_calls.clone();
        }
    }

    /// 从 `Value`（JSON 对象）提取字段到 `PipelineState`。
    ///
    /// 与 `apply_processed` 逻辑对齐，但直接操作 JSON（不依赖 JsonProcessor）。
    pub fn apply_value(state: &mut PipelineState, value: &Value) {
        let get_str = |key: &str| -> Option<String> {
            value.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
        };
        let get_f64 = |key: &str| -> Option<f64> {
            value.get(key).and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            })
        };

        if let Some(text) = get_str("text") {
            state.text = text;
        }
        if let Some(iu) = get_f64("importance_user") {
            state.importance_user = iu.clamp(0.0, 1.0);
        }
        if let Some(ia) = get_f64("importance_ai") {
            state.importance_ai = ia.clamp(0.0, 1.0);
        }
        if let Some(ltm) = get_str("long_term_memory") {
            state.long_term_memory = ltm;
        }
        if let Some(intent) = get_str("intent") {
            if matches!(intent.as_str(), "reply" | "short_reply" | "no_reply") {
                state.intent = intent;
            }
        }
        // no_reply → 清空 text
        if state.intent == "no_reply" {
            state.text = String::new();
        }
        // tool_calls
        if let Some(tc) = value.get("tool_calls").and_then(|v| v.as_array()) {
            if state.tool_calls.is_empty() {
                state.tool_calls = tc.clone();
            }
        }
    }

    /// 从文本解析并填充 PipelineState。
    ///
    /// 优先使用 JsonProcessor（若注入）；否则用 JsonParser 提取 JSON 后用 apply_value。
    /// 解析失败时使用 `response_text` 兜底（写入 `state.text`）。
    pub fn parse_into_state(&self, state: &mut PipelineState, text: &str) {
        if let Some(processor) = &self.json_processor {
            let processed = processor.process_response(text);
            Self::apply_processed(state, &processed);
            return;
        }

        match JsonParser::parse_single(text) {
            Ok(value) => {
                Self::apply_value(state, &value);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[StateFieldParser] JSON 解析失败，使用 response_text 兜底"
                );
                // 兜底：把原文作为 text，motion 默认 idle
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    state.text = trimmed.to_string();
                }
            }
        }
    }
}

impl Default for StateFieldParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseOutputParser for StateFieldParser {
    fn name(&self) -> &str {
        "StateFieldParser"
    }

    fn parse(&self, text: &str) -> VivianResult<Value> {
        let mut state = PipelineState::default();
        state.response_text = text.to_string();
        self.parse_into_state(&mut state, text);
        Ok(state.to_json())
    }
}

#[async_trait]
impl Runnable for StateFieldParser {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input.clone());
        let text = extract_text_from_value(&input)
            .or_else(|| {
                if !state.response_text.is_empty() {
                    Some(state.response_text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        if !text.is_empty() {
            self.parse_into_state(&mut state, &text);
        }
        Ok(state.to_json())
    }
}

// ============================================================================
// StreamingOutputParser —— 基于 StreamingJsonParser 的流式增量解析
// ============================================================================

/// 流式输出解析器。
///
/// 包装 [`StreamingJsonParser`]，将 LLM 流式输出的字符块增量解析为：
/// - `text_delta` 事件：`text` 字段的实时增量
/// - `text_done` 事件：完整 JSON 解析完成
/// - `error` 事件：单块解析错误（不中断后续 feed）
///
/// 用法：
/// ```ignore
/// let parser = StreamingOutputParser::new();
/// for chunk in stream {
///     let events = parser.feed(chunk);
///     for ev in events {
///         match ev.event.as_str() {
///             "text_delta" => print!("{}", ev.data),
///             "text_done" => break,
///             _ => {}
///         }
///     }
/// }
/// ```
pub struct StreamingOutputParser {
    inner: Mutex<StreamingJsonParser>,
    /// 累积的完整 text（用于最终校验）
    full_text: Mutex<String>,
}

impl StreamingOutputParser {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StreamingJsonParser::new()),
            full_text: Mutex::new(String::new()),
        }
    }

    /// 重置解析器到初始状态。
    pub fn reset(&self) {
        self.inner.lock().reset();
        self.full_text.lock().clear();
    }

    /// 是否已完成完整 JSON 解析。
    pub fn is_complete(&self) -> bool {
        self.inner.lock().is_complete()
    }

    /// 获取当前累积的 text 字段内容。
    pub fn text_content(&self) -> String {
        self.inner.lock().text_content().to_string()
    }

    /// 获取完整 JSON 解析结果（若已完成）。
    pub fn result(&self) -> Option<Value> {
        self.inner.lock().result().cloned()
    }

    /// 处理一个字符块，返回本次产生的 [`StreamEvent`] 列表。
    ///
    /// 将 [`JsonStreamEvent`] 转换为 pipeline 层的 [`StreamEvent`]：
    /// - `TextChunk(s)` → `text_delta` 事件
    /// - `Complete(v)` → `text_done` 事件
    /// - `Error(msg)` → `error` 事件
    pub fn feed(&self, chunk: &str) -> Vec<StreamEvent> {
        let json_events = self.inner.lock().feed(chunk);
        let mut out: Vec<StreamEvent> = Vec::with_capacity(json_events.len());

        for ev in json_events {
            match ev {
                JsonStreamEvent::TextChunk(s) => {
                    self.full_text.lock().push_str(&s);
                    out.push(StreamEvent::new("text_delta", s));
                }
                JsonStreamEvent::Meta(meta) => {
                    out.push(StreamEvent::new("meta", format!("{}|{}", meta.expression, meta.motion)));
                }
                JsonStreamEvent::Complete(v) => {
                    out.push(StreamEvent::new("text_done", v.to_string()));
                }
                JsonStreamEvent::Error(msg) => {
                    out.push(StreamEvent::new("error", msg));
                }
            }
        }
        out
    }

    /// 一次性流式解析完整文本（便捷方法）。
    ///
    /// 等价于把整个文本作为单个 chunk feed，返回所有事件。
    pub fn parse_stream(&self, text: &str) -> Vec<StreamEvent> {
        self.reset();
        self.feed(text)
    }
}

impl Default for StreamingOutputParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for StreamingOutputParser {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let text = match extract_text_from_value(&input) {
            Some(t) => t,
            None => return Ok(input),
        };
        self.parse_stream(&text);
        if let Some(v) = self.result() {
            Ok(v)
        } else {
            Ok(default_wrapped(&text))
        }
    }

    /// 流式 transform：将输入文本按字符分块 feed 到解析器，
    /// 把每个 `StreamEvent` 转换为带 `type` 字段的 `Value` 推送到 `output`。
    ///
    /// 推送的 chunk 格式：
    /// - `{"type": "text_delta", "data": "..."}`
    /// - `{"type": "meta", "expression": "...", "motion": "..."}`
    /// - `{"type": "text_done", "data": <parsed_value>}`
    /// - `{"type": "error", "data": "..."}`
    async fn atransform(
        &self,
        input: Value,
        output: tokio::sync::mpsc::Sender<Value>,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<()> {
        let text = match extract_text_from_value(&input) {
            Some(t) => t,
            None => {
                let _ = output.send(input).await;
                return Ok(());
            }
        };
        self.reset();
        // 按字符分块 feed，模拟流式输入
        const CHUNK_SIZE: usize = 8;
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(CHUNK_SIZE) {
            let s: String = chunk.iter().collect();
            let events = self.feed(&s);
            for ev in events {
                let value = match ev.event.as_str() {
                    "text_delta" => json!({ "type": "text_delta", "data": ev.data }),
                    "meta" => {
                        let parts: Vec<&str> = ev.data.splitn(2, '|').collect();
                        let expression = parts.first().copied().unwrap_or("").to_string();
                        let motion = parts.get(1).copied().unwrap_or("").to_string();
                        json!({
                            "type": "meta",
                            "expression": expression,
                            "motion": motion,
                        })
                    }
                    "text_done" => {
                        let parsed = serde_json::from_str::<Value>(&ev.data)
                            .unwrap_or_else(|_| json!({ "text": ev.data }));
                        json!({ "type": "text_done", "data": parsed })
                    }
                    "error" => json!({ "type": "error", "data": ev.data }),
                    _ => json!({ "type": ev.event, "data": ev.data }),
                };
                let _ = output.send(value).await;
            }
        }
        Ok(())
    }

    async fn astream_events(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Vec<StreamEvent>> {
        let text = match extract_text_from_value(&input) {
            Some(t) => t,
            None => return Ok(vec![StreamEvent::new("error", "输入非文本")]),
        };
        Ok(self.parse_stream(&text))
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn test_extract_text_from_value_string() {
        let v = json!("hello world");
        assert_eq!(
            extract_text_from_value(&v),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn test_extract_text_from_value_object() {
        let v = json!({"text": "hi", "motion": "idle"});
        assert_eq!(extract_text_from_value(&v), Some("hi".to_string()));
    }

    #[test]
    fn test_extract_text_from_value_object_response_text() {
        let v = json!({"response_text": "fallback"});
        assert_eq!(extract_text_from_value(&v), Some("fallback".to_string()));
    }

    #[test]
    fn test_extract_text_from_value_number_returns_none() {
        let v = json!(42);
        assert_eq!(extract_text_from_value(&v), None);
    }

    #[test]
    fn test_default_wrapped_trims_and_adds_defaults() {
        let v = default_wrapped("  hello  ");
        assert_eq!(v["text"], "hello");
        assert_eq!(v["motion"], "idle");
        assert_eq!(v["expression"], "star_eyes");
    }

    #[tokio::test]
    async fn test_json_output_parser_parses_object() {
        let parser = JsonOutputParser::new();
        let text = r#"{"text": "你好", "motion": "idle"}"#;
        let result = parser.parse(text).unwrap();
        assert_eq!(result["text"], "你好");
        assert_eq!(result["motion"], "idle");
    }

    #[tokio::test]
    async fn test_json_output_parser_falls_back_on_invalid_json() {
        let parser = JsonOutputParser::new();
        let result = parser.parse("纯文本回复").unwrap();
        // 应回退到 default_wrapped
        assert_eq!(result["text"], "纯文本回复");
        assert_eq!(result["motion"], "idle");
    }

    #[tokio::test]
    async fn test_json_output_parser_ainvoke_extracts_text_from_object() {
        let parser = JsonOutputParser::new();
        let input = json!({"text": r#"{"reply":"hi"}"#});
        let result = parser.ainvoke(input, None).await.unwrap();
        assert_eq!(result["reply"], "hi");
    }

    #[tokio::test]
    async fn test_json_output_parser_ainvoke_returns_input_for_non_string() {
        let parser = JsonOutputParser::new();
        let input = json!(42);
        let result = parser.ainvoke(input.clone(), None).await.unwrap();
        assert_eq!(result, input);
    }

    #[tokio::test]
    async fn test_json_output_parser_with_processor() {
        let processor = Arc::new(JsonProcessor::new());
        let parser = JsonOutputParser::with_processor(processor);
        let text = r#"{"text": "你好", "motion": "idle", "importance_user": 0.8}"#;
        let result = parser.parse(text).unwrap();
        // ProcessedResponse 序列化后应包含这些字段
        assert_eq!(result["text"], "你好");
        assert_eq!(result["motion"], "idle");
        assert_eq!(result["importance_user"], 0.8);
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct MySchema {
        text: String,
        motion: String,
    }

    #[test]
    fn test_schema_output_parser_parses_typed() {
        let parser: SchemaOutputParser<MySchema> = SchemaOutputParser::new();
        let text = r#"{"text": "你好", "motion": "idle"}"#;
        let result = parser.parse_typed(text).unwrap();
        assert_eq!(result, MySchema {
            text: "你好".to_string(),
            motion: "idle".to_string(),
        });
    }

    #[test]
    fn test_schema_output_parser_fails_on_invalid() {
        let parser: SchemaOutputParser<MySchema> = SchemaOutputParser::new();
        let text = "纯文本";
        let result = parser.parse_typed(text);
        assert!(result.is_err());
    }

    #[test]
    fn test_state_field_parser_apply_value_extracts_all_fields() {
        let mut state = PipelineState::default();
        let value = json!({
            "text": "你好呀",
            "motion": "idle",
            "expression": "star_eyes",
            "importance_user": 0.7,
            "importance_ai": 0.4,
            "long_term_memory": "用户打招呼",
            "intent": "reply",
            "tool_calls": [{"tool": "search", "arguments": {}}]
        });
        StateFieldParser::apply_value(&mut state, &value);
        assert_eq!(state.text, "你好呀");
        assert_eq!(state.motion, "idle");
        assert_eq!(state.expression, "star_eyes");
        assert_eq!(state.importance_user, 0.7);
        assert_eq!(state.importance_ai, 0.4);
        assert_eq!(state.long_term_memory, "用户打招呼");
        assert_eq!(state.intent, "reply");
        assert_eq!(state.tool_calls.len(), 1);
    }

    #[test]
    fn test_state_field_parser_no_reply_clears_text() {
        let mut state = PipelineState::default();
        state.text = "本应被清空".to_string();
        let value = json!({"text": "本应被清空", "intent": "no_reply"});
        StateFieldParser::apply_value(&mut state, &value);
        assert_eq!(state.intent, "no_reply");
        assert!(state.text.is_empty());
    }

    #[test]
    fn test_state_field_parser_invalid_intent_ignored() {
        let mut state = PipelineState::default();
        state.intent = "reply".to_string();
        let value = json!({"intent": "invalid_value"});
        StateFieldParser::apply_value(&mut state, &value);
        // 非法 intent 值不应覆盖
        assert_eq!(state.intent, "reply");
    }

    #[test]
    fn test_state_field_parser_clamps_importance() {
        let mut state = PipelineState::default();
        let value = json!({"importance_user": 1.5, "importance_ai": -0.3});
        StateFieldParser::apply_value(&mut state, &value);
        assert_eq!(state.importance_user, 1.0);
        assert_eq!(state.importance_ai, 0.0);
    }

    #[test]
    fn test_state_field_parser_parse_into_state_fallback_on_invalid_json() {
        let parser = StateFieldParser::new();
        let mut state = PipelineState::default();
        parser.parse_into_state(&mut state, "纯文本兜底");
        assert_eq!(state.text, "纯文本兜底");
        assert_eq!(state.motion, "idle");
    }

    #[tokio::test]
    async fn test_state_field_parser_ainvoke_uses_response_text() {
        let parser = StateFieldParser::new();
        let mut state = PipelineState::default();
        state.response_text = r#"{"text":"来自response_text","motion":"nod"}"#.to_string();
        let input = state.to_json();
        let result = parser.ainvoke(input, None).await.unwrap();
        let final_state = PipelineState::from_json(result);
        assert_eq!(final_state.text, "来自response_text");
        assert_eq!(final_state.motion, "nod");
    }

    #[test]
    fn test_streaming_output_parser_text_delta() {
        let parser = StreamingOutputParser::new();
        let chunk1 = r#"{"text": "你好"#;
        let chunk2 = r#", 世界"}"#;

        let events1 = parser.feed(chunk1);
        let deltas1: String = events1
            .iter()
            .filter(|e| e.event == "text_delta")
            .map(|e| e.data.clone())
            .collect();
        assert_eq!(deltas1, "你好");

        let events2 = parser.feed(chunk2);
        let deltas2: String = events2
            .iter()
            .filter(|e| e.event == "text_delta")
            .map(|e| e.data.clone())
            .collect();
        assert_eq!(deltas2, ", 世界");

        assert!(parser.is_complete());
        assert_eq!(parser.text_content(), "你好, 世界");
    }

    #[test]
    fn test_streaming_output_parser_complete_event() {
        let parser = StreamingOutputParser::new();
        let events = parser.feed(r#"{"text":"hi","motion":"idle"}"#);
        let has_done = events.iter().any(|e| e.event == "text_done");
        assert!(has_done);
        assert!(parser.is_complete());
    }

    #[test]
    fn test_streaming_output_parser_reset() {
        let parser = StreamingOutputParser::new();
        parser.feed(r#"{"text":"hi"}"#);
        assert!(parser.is_complete());
        parser.reset();
        assert!(!parser.is_complete());
        assert!(parser.text_content().is_empty());
    }

    #[test]
    fn test_streaming_output_parser_parse_stream_one_shot() {
        let parser = StreamingOutputParser::new();
        let events = parser.parse_stream(r#"{"text":"一次性","motion":"idle"}"#);
        assert!(events.iter().any(|e| e.event == "text_done"));
        assert!(parser.is_complete());
        assert_eq!(parser.text_content(), "一次性");
    }

    #[tokio::test]
    async fn test_streaming_output_parser_runnable_ainvoke() {
        let parser = StreamingOutputParser::new();
        let input = json!(r#"{"text":"你好","motion":"idle"}"#);
        let result = parser.ainvoke(input, None).await.unwrap();
        assert_eq!(result["text"], "你好");
        assert_eq!(result["motion"], "idle");
    }

    #[tokio::test]
    async fn test_streaming_output_parser_runnable_atransform() {
        use tokio::sync::mpsc;

        let parser = StreamingOutputParser::new();
        let input = json!(r#"{"text":"hi","motion":"nod"}"#);
        let (tx, mut rx) = mpsc::channel::<Value>(32);
        parser.atransform(input, tx, None).await.unwrap();

        let mut got_delta = false;
        let mut got_done = false;
        while let Some(v) = rx.recv().await {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("text_delta") => {
                    assert_eq!(v["data"], "hi");
                    got_delta = true;
                }
                Some("text_done") => {
                    assert_eq!(v["data"]["text"], "hi");
                    assert_eq!(v["data"]["motion"], "nod");
                    got_done = true;
                }
                _ => {}
            }
        }
        assert!(got_delta, "应产生 text_delta 事件");
        assert!(got_done, "应产生 text_done 事件");
    }
}
