//! 鲁棒 JSON 解析三件套。
//!
//! 所有解析阶段均带降级策略，自动去除 `<thinking>` 等推理标签与工具调用 markup，
//! 支持 code block 提取与平衡括号匹配。

use std::sync::atomic::{AtomicU32, Ordering};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::brain::tool_leak_filter::{strip_tool_call_markup, ToolLeakFilter};
use crate::error::{VivianError, VivianResult};

// ============================================================================
// 常量与预编译正则
// ============================================================================

/// 推理标签模式（编译一次，全局复用）
static THINKING_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?is)<think>.*?</think>").unwrap(),
        Regex::new(r"(?is)<thinking>.*?</thinking>").unwrap(),
        Regex::new(r"(?is)<reason>.*?</reason>").unwrap(),
        Regex::new(r"(?is)<analysis>.*?</analysis>").unwrap(),
        // 思考过程标记（部分模型的内部标记）
        Regex::new(r"(?is)\[think\].*?\[/think\]").unwrap(),
        Regex::new(r"(?is)\[thinking\].*?\[/thinking\]").unwrap(),
    ]
});

/// Code block 模式：```json ... ``` 或 ``` ... ```
static CODE_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)```(?:json)?\s*\n?(.*?)\n?\s*```").unwrap());

/// 未加引号的键名修复：`{key:` / `,key:` → `{ "key":` / `, "key":`
///
/// 注意：Rust `regex` 不支持 lookbehind，故改为捕获前导字符并重新插入。
static UNQUOTED_KEY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([,{])\s*(\w+)\s*:").unwrap());

// ============================================================================
// JsonParser：6 阶段鲁棒解析
// ============================================================================

/// 鲁棒 JSON 解析器。
///
/// 提供 6 阶段降级策略：
/// 1. 直接解析
/// 2. 移除推理标签后解析
/// 3. 提取 code block 后解析
/// 4. 查找 JSON 边界（平衡括号匹配）后解析
/// 5. 引号修复后解析
/// 6. 组合策略（code block + 边界 + 引号修复）
pub struct JsonParser;

impl JsonParser {
    /// 解析文本，返回所有提取到的 JSON 对象。
    ///
    /// 会先扫描文本中所有顶层平衡的 `{...}` 对象，再对每个候选尝试直接解析
    /// 与引号修复解析；若一无所获，则回退到 6 阶段单对象鲁棒解析。
    pub fn parse(text: &str) -> VivianResult<Vec<Value>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        // 阶段 2 预处理：移除推理标签
        let cleaned = remove_thinking_tags(text);
        let working: &str = if cleaned.trim().is_empty() {
            text
        } else {
            &cleaned
        };

        // 阶段 3：尝试 code block 提取
        let code_block = extract_code_block(working);
        let source: &str = code_block.as_deref().unwrap_or(working);

        let mut results: Vec<Value> = Vec::new();

        // 扫描所有顶层 JSON 对象边界
        for candidate in find_all_object_boundaries(source) {
            if let Ok(val) = serde_json::from_str::<Value>(&candidate) {
                results.push(val);
                continue;
            }
            // 阶段 5：引号修复后重试
            let fixed = fix_json_quotes(&candidate);
            if let Ok(val) = serde_json::from_str::<Value>(&fixed) {
                results.push(val);
            }
        }

        // 若未找到任何对象，回退到 6 阶段单对象鲁棒解析（可能命中数组）
        if results.is_empty() {
            if let Some(val) = robust_parse_single(text) {
                results.push(val);
            }
        }

        Ok(results)
    }

    /// 解析文本，返回第一个 JSON 对象。
    pub fn parse_single(text: &str) -> VivianResult<Value> {
        let mut values = Self::parse(text)?;
        if values.is_empty() {
            return Err(VivianError::Serialization(format!(
                "未能从文本中解析出任何 JSON 对象，原文前 200 字符: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(values.remove(0))
    }

    /// 从文本中提取指定字段。
    pub fn extract_field(text: &str, field_name: &str) -> Option<Value> {
        Self::parse_single(text)
            .ok()
            .and_then(|v| v.get(field_name).cloned())
    }

    /// 从可能是 JSON 的文本中安全提取 text 字段。
    ///
    /// 策略：
    /// 1. 先尝试鲁棒 JSON 解析，提取 text/content/response_text/output 字段
    /// 2. 如果解析失败或没有这些字段，返回 None（调用方决定是否使用原文）
    pub fn extract_text(text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }

        // 快速路径：明显不是 JSON（不以 { 或 [ 开头）
        let starts_with_json = trimmed.starts_with('{') || trimmed.starts_with('[');
        if !starts_with_json {
            return None;
        }

        Self::parse(text).ok().and_then(|values| {
            values.into_iter().find_map(|v| json_value_extract_text(&v))
        })
    }
}

/// 从 JSON Value 中提取文本字段（内部辅助）
fn json_value_extract_text(input: &Value) -> Option<String> {
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
    if let Some(arr) = input.as_array() {
        for item in arr {
            if let Some(s) = json_value_extract_text(item) {
                return Some(s);
            }
        }
    }
    None
}

/// 移除所有推理/思考标签与工具调用 markup。
fn remove_thinking_tags(text: &str) -> String {
    let mut result = text.to_string();
    for pattern in THINKING_PATTERNS.iter() {
        result = pattern.replace_all(&result, "").to_string();
    }
    // 移除工具调用 markup 泄露（<tool_call> / <function> 等）
    result = strip_tool_call_markup(&result);
    result.trim().to_string()
}

/// 从文本中提取 code block 中的 JSON。
fn extract_code_block(text: &str) -> Option<String> {
    CODE_BLOCK_RE.captures(text).map(|caps| {
        caps.get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default()
    })
}

/// 查找第一个完整 JSON 边界（平衡括号匹配）。
///
/// 从第一个 `{` 或 `[` 开始，找到匹配的结束位置。
fn find_json_boundaries(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();

    // 找最早的 `{` 或 `[`
    let start = chars
        .iter()
        .position(|&c| c == '{' || c == '[')?;

    let open = chars[start];
    let close = if open == '{' { '}' } else { ']' };

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for i in start..chars.len() {
        let c = chars[i];

        if escape_next {
            escape_next = false;
            continue;
        }

        if c == '\\' {
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(chars[start..=i].iter().collect());
            }
        }
    }

    None
}

/// 扫描文本中所有顶层平衡的 `{...}` 对象。
///
/// 仅匹配 `{}`，遇到嵌套时
/// 通过栈记录起始位置，栈空即得到一个顶层对象。
fn find_all_object_boundaries(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut results: Vec<String> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_string = false;
    let mut escape_next = false;

    for (i, &c) in chars.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        if c == '{' {
            stack.push(i);
        } else if c == '}' {
            if let Some(start) = stack.pop() {
                if stack.is_empty() {
                    results.push(chars[start..=i].iter().collect());
                }
            }
        }
    }

    results
}

/// 修复 JSON 中的引号问题（单引号 → 双引号 + 未加引号键名）。
fn fix_json_quotes(text: &str) -> String {
    let single_fixed = fix_single_quotes(text);
    fix_unquoted_keys(&single_fixed)
}

/// 将 JSON 结构中的单引号替换为双引号。
///
/// 自第一个 `{` 或 `[` 起，所有单引号都替换为双引号。
fn fix_single_quotes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_json = false;

    for c in text.chars() {
        if c == '{' || c == '[' {
            in_json = true;
        }
        if in_json && c == '\'' {
            result.push('"');
        } else {
            result.push(c);
        }
    }

    result
}

/// 为未加引号的键名添加引号。
fn fix_unquoted_keys(text: &str) -> String {
    UNQUOTED_KEY_RE
        .replace_all(text, r#"$1 "$2":"#)
        .to_string()
}

/// 6 阶段单对象鲁棒解析（内部使用，失败返回 `None`）。
fn robust_parse_single(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }

    // 阶段 1：直接解析
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        return Some(val);
    }

    // 阶段 2：移除推理标签
    let cleaned = remove_thinking_tags(text);
    if cleaned != text {
        if let Ok(val) = serde_json::from_str::<Value>(&cleaned) {
            return Some(val);
        }
    }
    let working = if cleaned.is_empty() { text } else { &cleaned };

    // 阶段 3：提取 code block
    let code_block = extract_code_block(working);
    if let Some(ref cb) = code_block {
        if let Ok(val) = serde_json::from_str::<Value>(cb) {
            return Some(val);
        }
    }

    // 阶段 4：查找 JSON 边界
    if let Some(bounded) = find_json_boundaries(working) {
        if let Ok(val) = serde_json::from_str::<Value>(&bounded) {
            return Some(val);
        }
        // 在 bounded 上尝试引号修复
        let fixed = fix_json_quotes(&bounded);
        if let Ok(val) = serde_json::from_str::<Value>(&fixed) {
            return Some(val);
        }
    }

    // 阶段 5：引号修复（全文本）
    let fixed = fix_json_quotes(working);
    if let Ok(val) = serde_json::from_str::<Value>(&fixed) {
        return Some(val);
    }

    // 阶段 6：组合策略（code block + 边界 + 引号修复）
    if let Some(cb) = code_block {
        if let Some(bounded_code) = find_json_boundaries(&cb) {
            let fixed_bounded = fix_json_quotes(&bounded_code);
            if let Ok(val) = serde_json::from_str::<Value>(&fixed_bounded) {
                return Some(val);
            }
        }
    }

    tracing::warn!(
        original_head = %text.chars().take(200).collect::<String>(),
        "[RobustJSON] 所有解析阶段均失败"
    );
    None
}

// ============================================================================
// StreamingJsonParser：状态机流式解析
// ============================================================================

/// 流式解析事件。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// `text` 字段新内容块（实时增量）。
    TextChunk(String),
    /// 检测到 expression 或 motion 字段值（在 text 之前到达，用于提前触发 Live2D 动画）。
    Meta(MetaEvent),
    /// 完整 JSON 解析完成。
    Complete(Value),
    /// 解析错误。
    Error(String),
}

/// 元事件：LLM JSON 中 expression/motion 字段的实时值。
#[derive(Debug, Clone)]
pub struct MetaEvent {
    pub expression: String,
    pub motion: String,
}

/// 解析器主状态（8 状态枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserState {
    /// 等待 JSON 开始。
    Idle,
    /// 字符串值/键中。
    InString,
    /// 字符串转义中（刚看到反斜杠）。
    InStringEscape,
    /// 数字字面量中。
    InNumber,
    /// 对象中。
    InObject,
    /// 数组中。
    InArray,
    /// 值结束之后（期待 `,` / `}` / `]`）。
    AfterValue,
    /// 顶层 JSON 解析完成。
    Done,
}

/// 字段上下文子状态（6 子状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldContext {
    /// 无特殊字段。
    None,
    /// 在 `text` 字段值中。
    InTextField,
    /// 在 `tool_calls` 字段中。
    InToolCalls,
    /// 在 `importance` 字段中。
    InImportance,
    /// `text` 值结束之后。
    AfterTextValue,
    /// `tool_calls` 结束之后。
    AfterToolCallsEnd,
}

/// 流式 JSON 解析器。
///
/// 在流式输出中优先识别 `text` 字段并实时提取，同时完整收集其他 JSON 字段，
/// 支持嵌套对象与顶级数组。
pub struct StreamingJsonParser {
    /// 当前主状态。
    state: ParserState,
    /// 当前字段上下文子状态。
    field_context: FieldContext,
    /// 当前键名（读取键时累积，读取值时引用）。
    current_key: String,
    /// 是否正在读取键名（InString 时区分键/值）。
    parsing_key: bool,
    /// 键名已读完、正等待值（已看到冒号）。
    awaiting_value: bool,
    /// 进入转义前是否处于 text 字段。
    was_in_text_field: bool,
    /// text 字段实时累积内容。
    text_accumulator: String,
    /// 完整 JSON 文本累积（用于最终 `serde_json::from_str`）。
    object_buffer: String,
    /// `{}` 嵌套深度。
    brace_depth: i32,
    /// `[]` 嵌套深度。
    bracket_depth: i32,
    /// 顶层是否为数组。
    top_is_array: bool,
    /// 本次 feed 产生的事件。
    events: Vec<StreamEvent>,
    /// 最终解析结果。
    result_value: Option<Value>,
    /// 是否已完成。
    is_complete: bool,
    /// 工具调用 markup 泄露过滤器（流式过滤 text 字段中的 tool_call 标签）。
    tool_leak_filter: ToolLeakFilter,
}

impl StreamingJsonParser {
    /// 创建新的流式解析器。
    pub fn new() -> Self {
        Self {
            state: ParserState::Idle,
            field_context: FieldContext::None,
            current_key: String::new(),
            parsing_key: false,
            awaiting_value: false,
            was_in_text_field: false,
            text_accumulator: String::new(),
            object_buffer: String::new(),
            brace_depth: 0,
            bracket_depth: 0,
            top_is_array: false,
            events: Vec::new(),
            result_value: None,
            is_complete: false,
            tool_leak_filter: ToolLeakFilter::new(),
        }
    }

    /// 重置解析器到初始状态。
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// 处理一个新的字符块，返回本次产生的事件。
    pub fn feed(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.events.clear();
        for c in chunk.chars() {
            self.process_char(c);
        }
        std::mem::take(&mut self.events)
    }

    /// 获取当前 text 字段累积内容。
    pub fn text_content(&self) -> &str {
        &self.text_accumulator
    }

    /// 获取最终解析结果（若已完成）。
    pub fn result(&self) -> Option<&Value> {
        self.result_value.as_ref()
    }

    /// 是否已完成解析。
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// 处理单个字符。
    fn process_char(&mut self, c: char) {
        // Idle 状态：仅识别 JSON 起始字符
        if self.state == ParserState::Idle {
            if c == '{' {
                self.object_buffer.push(c);
                self.state = ParserState::InObject;
                self.brace_depth = 1;
            } else if c == '[' {
                self.object_buffer.push(c);
                self.state = ParserState::InArray;
                self.bracket_depth = 1;
                self.top_is_array = true;
            }
            // 其他字符忽略
            return;
        }

        // 非 Idle：始终累积到 object_buffer（保证最终完整解析）
        self.object_buffer.push(c);

        // 转义字符处理（优先级最高）
        if self.state == ParserState::InStringEscape {
            if self.was_in_text_field {
                let unescaped = unescape_char(c);
                self.text_accumulator.push_str(&unescaped);
                let filtered = self.tool_leak_filter.feed(&unescaped);
                if !filtered.is_empty() {
                    self.events.push(StreamEvent::TextChunk(filtered));
                }
            }
            self.state = ParserState::InString;
            return;
        }

        if c == '\\' && self.state == ParserState::InString {
            self.was_in_text_field = self.field_context == FieldContext::InTextField;
            self.state = ParserState::InStringEscape;
            return;
        }

        match self.state {
            ParserState::Idle => unreachable!(),
            ParserState::Done => {
                // 完成后忽略多余字符
            }
            ParserState::InArray => {
                if c == ']' {
                    self.bracket_depth -= 1;
                    if self.bracket_depth == 0 {
                        self.finalize_array();
                    }
                } else if c == '[' {
                    self.bracket_depth += 1;
                } else if c == '{' {
                    self.brace_depth = 1;
                    self.state = ParserState::InObject;
                    self.awaiting_value = false;
                    self.parsing_key = false;
                }
            }
            ParserState::InObject => {
                if c == '}' {
                    self.brace_depth -= 1;
                    if self.brace_depth == 0 {
                        if self.top_is_array {
                            // 数组内的对象结束，回到数组状态
                            self.state = ParserState::InArray;
                            self.awaiting_value = false;
                        } else {
                            self.finalize_object();
                        }
                    }
                } else if c == '"' {
                    // 开始读取键或字符串值
                    if self.awaiting_value {
                        // 读取字符串值
                        self.parsing_key = false;
                        self.field_context = self.field_context_for_key();
                        self.state = ParserState::InString;
                    } else {
                        // 读取键名
                        self.current_key.clear();
                        self.parsing_key = true;
                        self.state = ParserState::InString;
                    }
                } else if c == ':' {
                    self.awaiting_value = true;
                } else if c == ',' {
                    self.awaiting_value = false;
                    self.current_key.clear();
                    self.field_context = FieldContext::None;
                } else if c == '{' {
                    // 值为嵌套对象
                    self.brace_depth += 1;
                } else if c == '[' {
                    // 值为数组
                    self.bracket_depth += 1;
                    if self.current_key == "tool_calls" {
                        self.field_context = FieldContext::InToolCalls;
                    }
                } else if c == ']' {
                    self.bracket_depth -= 1;
                    if self.field_context == FieldContext::InToolCalls {
                        self.field_context = FieldContext::AfterToolCallsEnd;
                    }
                } else if c.is_ascii_digit() || c == '-' || c == '+' {
                    if self.awaiting_value {
                        self.state = ParserState::InNumber;
                        self.awaiting_value = false;
                    }
                }
                // 空白字符忽略
            }
            ParserState::InString => {
                if c == '"' {
                    if self.parsing_key {
                        // 键名读取完成
                        self.awaiting_value = true;
                        self.parsing_key = false;
                        self.state = ParserState::InObject;
                    } else {
                        // 字符串值读取完成
                        self.finish_string();
                        self.state = ParserState::AfterValue;
                    }
                } else if self.parsing_key {
                    self.current_key.push(c);
                } else if self.field_context == FieldContext::InTextField {
                    // text 字段实时提取（经 ToolLeakFilter 过滤工具调用 markup）
                    self.text_accumulator.push(c);
                    let filtered = self.tool_leak_filter.feed(&c.to_string());
                    if !filtered.is_empty() {
                        self.events.push(StreamEvent::TextChunk(filtered));
                    }
                }
            }
            ParserState::InStringEscape => {
                unreachable!();
            }
            ParserState::InNumber => {
                if c == ',' || c == '}' || c == ']' {
                    // 数字结束
                    if c == '}' {
                        self.brace_depth -= 1;
                        if self.brace_depth == 0 {
                            if self.top_is_array {
                                self.state = ParserState::InArray;
                            } else {
                                self.finalize_object();
                            }
                        } else {
                            self.state = ParserState::InObject;
                        }
                    } else if c == ']' {
                        self.bracket_depth -= 1;
                        if self.bracket_depth == 0 && self.top_is_array {
                            self.finalize_array();
                        } else {
                            self.state = ParserState::InArray;
                        }
                    } else {
                        // 逗号
                        self.awaiting_value = false;
                        self.current_key.clear();
                        self.field_context = FieldContext::None;
                        self.state = if self.bracket_depth > 0 && self.brace_depth == 0 {
                            ParserState::InArray
                        } else {
                            ParserState::InObject
                        };
                    }
                }
                // 其他数字字符继续累积（已进入 object_buffer）
            }
            ParserState::AfterValue => {
                if c == ',' {
                    self.awaiting_value = false;
                    self.current_key.clear();
                    self.field_context = FieldContext::None;
                    self.state = if self.brace_depth > 0 {
                        ParserState::InObject
                    } else {
                        ParserState::InArray
                    };
                } else if c == '}' {
                    self.brace_depth -= 1;
                    if self.brace_depth == 0 {
                        if self.top_is_array {
                            self.state = ParserState::InArray;
                        } else {
                            self.finalize_object();
                        }
                    } else {
                        self.state = ParserState::InObject;
                    }
                } else if c == ']' {
                    self.bracket_depth -= 1;
                    if self.bracket_depth == 0 && self.top_is_array && self.brace_depth == 0 {
                        // 处理数组内对象后的 ]
                        self.state = ParserState::InArray;
                    } else {
                        self.state = ParserState::InArray;
                    }
                }
                // 空白字符忽略
            }
        }
    }

    /// 根据当前键名确定字段上下文。
    fn field_context_for_key(&self) -> FieldContext {
        match self.current_key.as_str() {
            "text" => FieldContext::InTextField,
            "tool_calls" => FieldContext::InToolCalls,
            "importance" | "importance_user" | "importance_ai" => FieldContext::InImportance,
            _ => FieldContext::None,
        }
    }

    /// 完成字符串值。
    fn finish_string(&mut self) {
        match self.current_key.as_str() {
            "expression" => {
                // 在 text 之前检测到 expression 字段 → 发射 Meta 事件
                if !self.text_accumulator.is_empty() {
                    // text 已开始流式，expression 来晚了，不发射 meta
                } else {
                    let val = self.object_buffer
                        .rfind('"')
                        .and_then(|end| {
                            let before = &self.object_buffer[..end];
                            before.rfind('"').map(|start| &before[start+1..])
                        })
                        .unwrap_or("")
                        .to_string();
                    if !val.is_empty() && val != "default" && val != "" {
                        // 暂存 expression，等 motion 也到了再发射，或直接发射
                        let meta = MetaEvent {
                            expression: val,
                            motion: String::new(),
                        };
                        self.events.push(StreamEvent::Meta(meta));
                    }
                }
            }
            "motion" => {
                // 在 text 之前检测到 motion 字段 → 发射 Meta 事件
                if !self.text_accumulator.is_empty() {
                    // text 已开始，不发射 meta
                } else {
                    let val = self.object_buffer
                        .rfind('"')
                        .and_then(|end| {
                            let before = &self.object_buffer[..end];
                            before.rfind('"').map(|start| &before[start+1..])
                        })
                        .unwrap_or("")
                        .to_string();
                    if !val.is_empty() && val != "idle" {
                        let meta = MetaEvent {
                            expression: String::new(),
                            motion: val,
                        };
                        self.events.push(StreamEvent::Meta(meta));
                    }
                }
            }
            "text" => {
                if self.field_context == FieldContext::InTextField {
                    self.field_context = FieldContext::AfterTextValue;
                }
            }
            _ => {}
        }
        self.awaiting_value = false;
        self.current_key.clear();
    }

    /// 完成对象解析。
    fn finalize_object(&mut self) {
        // 排空工具调用泄露过滤器（可能残留被缓冲的文本）
        let residual = self.tool_leak_filter.flush();
        if !residual.is_empty() {
            self.events.push(StreamEvent::TextChunk(residual));
        }
        match serde_json::from_str::<Value>(&self.object_buffer) {
            Ok(val) => {
                self.result_value = Some(val.clone());
                self.is_complete = true;
                self.state = ParserState::Done;
                self.events.push(StreamEvent::Complete(val));
            }
            Err(e) => {
                tracing::debug!(error = %e, "[StreamingJson] 对象解析失败");
                self.events.push(StreamEvent::Error(e.to_string()));
                self.state = ParserState::Idle;
            }
        }
        self.brace_depth = 0;
    }

    /// 完成数组解析。
    fn finalize_array(&mut self) {
        match serde_json::from_str::<Value>(&self.object_buffer) {
            Ok(val) => {
                self.result_value = Some(val.clone());
                self.is_complete = true;
                self.state = ParserState::Done;
                self.events.push(StreamEvent::Complete(val));
            }
            Err(e) => {
                tracing::debug!(error = %e, "[StreamingJson] 数组解析失败");
                self.events.push(StreamEvent::Error(e.to_string()));
                self.state = ParserState::Idle;
            }
        }
        self.bracket_depth = 0;
        self.brace_depth = 0;
        self.top_is_array = false;
    }
}

impl Default for StreamingJsonParser {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON 转义字符还原。
fn unescape_char(c: char) -> String {
    match c {
        'n' => "\n".to_string(),
        't' => "\t".to_string(),
        'r' => "\r".to_string(),
        'b' => "\u{0008}".to_string(),
        'f' => "\u{000C}".to_string(),
        '\\' => "\\".to_string(),
        '"' => "\"".to_string(),
        '\'' => "'".to_string(),
        '/' => "/".to_string(),
        _ => c.to_string(),
    }
}

// ============================================================================
// StreamingResponseHandler：流式响应处理器
// ============================================================================

/// 流式响应处理器。
///
/// 包装 [`StreamingJsonParser`]，将 `text` 字段增量、工具调用与完成事件
/// 分发到回调。
pub struct StreamingResponseHandler {
    parser: StreamingJsonParser,
    on_text_update: Box<dyn Fn(&str) + Send + Sync>,
    on_tool_call: Option<Box<dyn Fn(&Value) + Send + Sync>>,
    on_complete: Option<Box<dyn Fn(&Value) + Send + Sync>>,
    /// 累积的完整 text。
    full_text: String,
}

impl StreamingResponseHandler {
    /// 创建处理器，`on_text_update` 在 text 字段更新时以完整累积文本调用。
    pub fn new<F>(on_text_update: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        Self {
            parser: StreamingJsonParser::new(),
            on_text_update: Box::new(on_text_update),
            on_tool_call: None,
            on_complete: None,
            full_text: String::new(),
        }
    }

    /// 设置工具调用回调。
    pub fn with_tool_call<F>(mut self, cb: F) -> Self
    where
        F: Fn(&Value) + Send + Sync + 'static,
    {
        self.on_tool_call = Some(Box::new(cb));
        self
    }

    /// 设置完成回调。
    pub fn with_complete<F>(mut self, cb: F) -> Self
    where
        F: Fn(&Value) + Send + Sync + 'static,
    {
        self.on_complete = Some(Box::new(cb));
        self
    }

    /// 处理一个字符块。
    pub fn feed(&mut self, chunk: &str) {
        let events = self.parser.feed(chunk);
        for event in events {
            match event {
                StreamEvent::TextChunk(s) => {
                    self.full_text.push_str(&s);
                    (self.on_text_update)(&self.full_text);
                }
                StreamEvent::Meta(_) => {
                    // Meta 事件（expression/motion 实时检测）由上层 StreamingResponseHandler 的调用方处理
                }
                StreamEvent::Complete(v) => {
                    if let Some(tc) = &self.on_tool_call {
                        if let Some(obj) = v.as_object() {
                            if obj.contains_key("tool") {
                                tc(&v);
                            }
                        }
                    }
                    if let Some(cc) = &self.on_complete {
                        cc(&v);
                    }
                }
                StreamEvent::Error(_) => {}
            }
        }
    }

    /// 获取完整 JSON 结果。
    pub fn get_full_json(&self) -> Option<&Value> {
        self.parser.result()
    }

    /// 获取累积的完整 text。
    pub fn full_text(&self) -> &str {
        &self.full_text
    }
}

// ============================================================================
// JsonProcessor：LLM 响应提取
// ============================================================================

/// LLM 响应处理结果。
///
/// 主调用仅返回核心对话字段，表情/动作/心理状态/评分等由反思调用填充。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProcessedResponse {
    /// 文本回复。
    pub text: String,
    /// 意图。
    pub intent: String,
    /// 响应模式（仅跨角色对话生效）— speak/non_verbal/internal/ignore
    ///
    /// LLM 在 JSON 中返回，未返回时默认 "speak"。
    /// 主对话路径下永远视为 "speak"（即便 LLM 返回非 speak 值）。
    pub response_mode: String,
    /// 微信渠道语音消息标志：为 true 时前端不显示文本，合成 TTS 后以语音气泡发出
    #[serde(default)]
    pub voice_message: bool,
    /// 提取到的工具调用列表（不参与 LLM Schema 约束，工具调用走原生 FC 通道）。
    #[schemars(skip)]
    pub tool_calls: Vec<Value>,
}

/// JSON 处理器，负责 LLM 响应的 JSON 提取与解析。
pub struct JsonProcessor {
    /// 连续 JSON 解析失败计数器。
    json_parsing_failures: AtomicU32,
}

impl JsonProcessor {
    /// 创建新的处理器。
    pub fn new() -> Self {
        Self {
            json_parsing_failures: AtomicU32::new(0),
        }
    }

    /// 处理 LLM 响应，提取工具调用与文本回复。
    pub fn process_response(&self, text: &str) -> ProcessedResponse {
        // 清理 code block 标记
        let mut cleaned = text.trim().to_string();
        cleaned = Regex::new(r"(?m)^```json\s*").unwrap()
            .replace_all(&cleaned, "")
            .to_string();
        cleaned = Regex::new(r"(?m)^```\s*").unwrap()
            .replace_all(&cleaned, "")
            .to_string();
        cleaned = Regex::new(r"(?m)```$").unwrap()
            .replace_all(&cleaned, "")
            .to_string();

        // 提取所有 JSON 对象
        let json_objects = extract_all_json_objects(&cleaned);

        // 找出所有 JSON 对象的位置（用于提取工具调用前的纯文本）
        let json_positions = find_json_positions(&cleaned);
        let text_before_tool = if let Some(&(start, _)) = json_positions.first() {
            cleaned[..start].trim().to_string()
        } else {
            String::new()
        };

        // 识别工具调用与文本回复
        let mut tool_call: Option<Value> = None;
        let mut text_response: Option<Value> = None;
        let mut tool_calls: Vec<Value> = Vec::new();

        for obj in &json_objects {
            if let Some(map) = obj.as_object() {
                if map.contains_key("tool") && map.contains_key("arguments") {
                    if tool_call.is_none() {
                        tool_call = Some(obj.clone());
                    }
                    tool_calls.push(obj.clone());
                }
                if map.contains_key("text") {
                    if text_response.is_none() {
                        text_response = Some(obj.clone());
                    }
                }
            }
        }

        // 所有分支均会将 is_json_valid 置为 true（回退策略总会包装为有效文本）
        #[allow(unused_assignments)]
        let mut is_json_valid = false;
        let mut data: Value;

        if let Some(tc) = &tool_call {
            if !text_before_tool.is_empty() {
                // 工具调用前有纯文本
                tracing::info!("[JSONProcessor] 发现工具调用，使用工具前的文本");
                data = serde_json::json!({
                    "text": text_before_tool,
                });
                is_json_valid = true;
            } else if let Some(tr) = &text_response {
                tracing::info!("[JSONProcessor] 同时发现工具调用和文本回复，优先返回文本回复");
                data = tr.clone();
                is_json_valid = true;
            } else {
                tracing::info!("[JSONProcessor] 发现工具调用");
                data = tc.clone();
                is_json_valid = true;
            }
        } else if let Some(tr) = &text_response {
            tracing::info!("[JSONProcessor] 发现文本回复");
            data = tr.clone();
            is_json_valid = true;
        } else {
            // 检查是否是纯文本回复
            let has_brace = cleaned.contains('{') || cleaned.contains('}');
            let has_bracket = cleaned.contains('[') || cleaned.contains(']');

            if !has_brace && !has_bracket {
                // 纯文本回复，直接包装
                tracing::info!("[JSONProcessor] 纯文本回复");
                data = serde_json::json!({
                    "text": strip_markdown(&cleaned),
                });
                is_json_valid = true;
            } else {
                // 尝试解析 JSON
                match serde_json::from_str::<Value>(&cleaned) {
                    Ok(val) => {
                        data = val;
                        is_json_valid = true;
                    }
                    Err(_) => {
                        // 尝试正则提取数组
                        let array_re = Regex::new(r"\[[\s\S]*\]").unwrap();
                        if let Some(m) = array_re.find(&cleaned) {
                            match serde_json::from_str::<Value>(m.as_str()) {
                                Ok(val) => {
                                    data = val;
                                    is_json_valid = true;
                                }
                                Err(_) => {
                                    // 尝试正则提取对象
                                    let obj_re = Regex::new(r"\{[\s\S]*\}").unwrap();
                                    if let Some(m2) = obj_re.find(&cleaned) {
                                        match serde_json::from_str::<Value>(m2.as_str()) {
                                            Ok(val) => {
                                                data = val;
                                                is_json_valid = true;
                                            }
                                            Err(_) => {
                                                tracing::info!(
                                                    "[JSONProcessor] 没有找到有效的JSON，当作纯文本处理"
                                                );
                                                data = serde_json::json!({
                                                    "text": strip_markdown(&cleaned),
                                                    "motion": "idle",
                                                    "expression": "",
                                                });
                                                is_json_valid = true;
                                            }
                                        }
                                    } else {
                                        data = serde_json::json!({
                                            "text": strip_markdown(&cleaned),
                                            "motion": "idle",
                                            "expression": "",
                                        });
                                        is_json_valid = true;
                                    }
                                }
                            }
                        } else {
                            let obj_re = Regex::new(r"\{[\s\S]*\}").unwrap();
                            if let Some(m2) = obj_re.find(&cleaned) {
                                match serde_json::from_str::<Value>(m2.as_str()) {
                                    Ok(val) => {
                                        data = val;
                                        is_json_valid = true;
                                    }
                                    Err(_) => {
                                        data = serde_json::json!({
                                            "text": strip_markdown(&cleaned),
                                            "motion": "idle",
                                            "expression": "",
                                        });
                                        is_json_valid = true;
                                    }
                                }
                            } else {
                                data = serde_json::json!({
                                    "text": strip_markdown(&cleaned),
                                    "motion": "idle",
                                    "expression": "",
                                });
                                is_json_valid = true;
                            }
                        }
                    }
                }
            }
        }

        // 更新连续失败计数
        if is_json_valid {
            self.json_parsing_failures.store(0, Ordering::Relaxed);
        } else {
            let n = self.json_parsing_failures.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(failures = n, "JSON 解析失败，连续失败次数: {}", n);
        }

        if !is_json_valid {
            data = serde_json::json!({
                "text": strip_markdown(&cleaned),
            });
        }

        // 若为列表，取第一个对象元素
        if let Some(arr) = data.as_array() {
            data = arr
                .first()
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
        }

        let map = data.as_object();
        let get_str = |key: &str, default: &str| -> String {
            map.and_then(|m| m.get(key))
                .and_then(|v| v.as_str())
                .unwrap_or(default)
                .to_string()
        };

        let text = map
            .and_then(|m| m.get("text"))
            .and_then(|v| v.as_str())
            .or_else(|| {
                map.and_then(|m| m.get("content"))
                    .and_then(|v| v.as_str())
            })
            .or_else(|| {
                map.and_then(|m| m.get("output"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();

        let voice_message = map
            .and_then(|m| m.get("voice_message"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        ProcessedResponse {
            text,
            intent: get_str("intent", "reply"),
            response_mode: get_str("response_mode", "speak"),
            voice_message,
            tool_calls,
        }
    }
}

/// 解析 LLM 返回的 appraisal 字段（6 项认知评估）
pub fn parse_appraisal(v: &Value) -> Option<crate::psychology::Appraisal> {
    let obj = v.as_object()?;
    Some(crate::psychology::Appraisal {
        threat: obj.get("threat").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        rejection: obj.get("rejection").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        control: obj.get("control").and_then(|v| v.as_f64()).unwrap_or(0.5).clamp(0.0, 1.0),
        fairness: obj.get("fairness").and_then(|v| v.as_f64()).unwrap_or(0.5).clamp(0.0, 1.0),
        novelty: obj.get("novelty").and_then(|v| v.as_f64()).unwrap_or(0.3).clamp(0.0, 1.0),
        significance: obj.get("significance").and_then(|v| v.as_f64()).unwrap_or(0.5).clamp(0.0, 1.0),
    })
}

/// 解析 LLM 返回的 emotion_update 字段（7 项情绪增量）
pub fn parse_emotion_deltas(v: &Value) -> Option<crate::psychology::EmotionDeltas> {
    let obj = v.as_object()?;
    Some(crate::psychology::EmotionDeltas {
        joy: obj.get("joy").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        sadness: obj.get("sadness").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        anger: obj.get("anger").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        fear: obj.get("fear").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        closeness: obj.get("closeness").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        loneliness: obj.get("loneliness").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
        curiosity: obj.get("curiosity").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-0.3, 0.3),
    })
}

/// 解析 LLM 返回的 behavior_drive 字段（8 项行为驱动）
pub fn parse_behavior_drive(v: &Value) -> Option<crate::psychology::BehaviorDrive> {
    let obj = v.as_object()?;
    use crate::psychology::behavior_drive::DriveSource;
    let source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "llm" => DriveSource::Llm,
            _ => DriveSource::Rule,
        })
        .unwrap_or(DriveSource::Llm);
    Some(crate::psychology::BehaviorDrive {
        approach: obj.get("approach").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        avoid: obj.get("avoid").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        explore: obj.get("explore").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        express: obj.get("express").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        rest: obj.get("rest").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        observe: obj.get("observe").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        play: obj.get("play").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        help: obj.get("help").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0),
        source,
    })
}

impl Default for JsonProcessor {
    fn default() -> Self {
        Self::new()
    }
}

/// 从文本中提取所有有效的 JSON 对象。
fn extract_all_json_objects(text: &str) -> Vec<Value> {
    let mut results = Vec::new();
    for candidate in find_all_object_boundaries(text) {
        if let Ok(val) = serde_json::from_str::<Value>(&candidate) {
            if val.is_object() {
                results.push(val);
            }
        }
    }
    results
}

/// 查找文本中所有顶层 `{...}` 对象的 `(start, end)` 字节位置区间。
fn find_json_positions(text: &str) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let bytes = text.as_bytes();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_string = false;
    let mut escape_next = false;

    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if escape_next {
            escape_next = false;
            continue;
        }
        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if c == '{' {
            stack.push(i);
        } else if c == '}' {
            if let Some(start) = stack.pop() {
                if stack.is_empty() {
                    positions.push((start, i + 1));
                }
            }
        }
    }

    positions
}

/// 当 LLM 违反 JSON 契约、输出纯文本/Markdown 时，剥离常见 Markdown 标记，
/// 让展示层至少不出现 `**`/`##`/`- ` 等格式符号。
fn strip_markdown(text: &str) -> String {
    let s = text.trim();
    if s.is_empty() {
        return String::new();
    }

    static BOLD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
    static ITALIC_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*(.+?)\*").unwrap());
    static INLINE_CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static HEADING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#{1,6}\s+").unwrap());
    static LIST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^[\s]*[-*+]\s+").unwrap());
    static NUM_LIST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^[\s]*\d+\.\s+").unwrap());
    static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap());

    let s = BOLD_RE.replace_all(&s, "$1").to_string();
    let s = ITALIC_RE.replace_all(&s, "$1").to_string();
    let s = INLINE_CODE_RE.replace_all(&s, "$1").to_string();
    let s = LINK_RE.replace_all(&s, "$1").to_string();
    let s = HEADING_RE.replace_all(&s, "").to_string();
    let s = LIST_RE.replace_all(&s, "").to_string();
    let s = NUM_LIST_RE.replace_all(&s, "").to_string();

    s.trim().to_string()
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_object() {
        let text = r#"{"name": "薇薇安", "age": 18}"#;
        let vals = JsonParser::parse(text).unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0]["name"], "薇薇安");
    }

    #[test]
    fn test_strip_markdown_bold_and_heading() {
        let raw = "我能帮你做的事情可多了！\n\n**📝 文字创作类**\n- 写文章\n- 润色";
        let stripped = strip_markdown(raw);
        assert!(!stripped.contains("**"));
        assert!(!stripped.contains("- "));
        assert!(stripped.contains("文字创作类"));
        assert!(stripped.contains("写文章"));
    }

    #[test]
    fn test_strip_markdown_plain_text_passthrough() {
        let raw = "哼…算你厉害嘛";
        assert_eq!(strip_markdown(raw), "哼…算你厉害嘛");
    }

    #[test]
    fn test_strip_markdown_empty() {
        assert_eq!(strip_markdown(""), "");
        assert_eq!(strip_markdown("   "), "");
    }



    #[test]
    fn test_parse_multiple_objects() {
        let text = r#"前置文本 {"a": 1} 中间文本 {"b": 2}"#;
        let vals = JsonParser::parse(text).unwrap();
        assert_eq!(vals.len(), 2);
        assert_eq!(vals[0]["a"], 1);
        assert_eq!(vals[1]["b"], 2);
    }

    #[test]
    fn test_parse_with_thinking_tags() {
        let text = r#"<think>一些思考</think>{"text": "你好"}"#;
        let val = JsonParser::parse_single(text).unwrap();
        assert_eq!(val["text"], "你好");
    }

    #[test]
    fn test_parse_with_code_block() {
        let text = "```json\n{\"key\": \"value\"}\n```";
        let val = JsonParser::parse_single(text).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_parse_single_quotes() {
        let text = r#"{'key': 'value'}"#;
        let val = JsonParser::parse_single(text).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_parse_unquoted_keys() {
        let text = r#"{key: "value"}"#;
        let val = JsonParser::parse_single(text).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_streaming_text_extraction() {
        let mut parser = StreamingJsonParser::new();
        let chunk1 = r#"{"text": "你好"#;
        let chunk2 = r#", 世界"}"#;

        let events1 = parser.feed(chunk1);
        let text1: String = events1
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextChunk(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text1, "你好");

        let events2 = parser.feed(chunk2);
        let text2: String = events2
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextChunk(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text2, ", 世界");

        assert!(parser.is_complete());
        assert_eq!(parser.text_content(), "你好, 世界");
    }

    #[test]
    fn test_streaming_complete() {
        let mut parser = StreamingJsonParser::new();
        parser.feed(r#"{"text": "hi", "motion": "idle"}"#);
        assert!(parser.is_complete());
        let result = parser.result().unwrap();
        assert_eq!(result["text"], "hi");
        assert_eq!(result["motion"], "idle");
    }

    #[test]
    fn test_streaming_array() {
        let mut parser = StreamingJsonParser::new();
        parser.feed(r#"[{"text": "a"}, {"text": "b"}]"#);
        assert!(parser.is_complete());
        let result = parser.result().unwrap();
        assert!(result.is_array());
        assert_eq!(result[0]["text"], "a");
    }

    #[test]
    fn test_streaming_escape() {
        let mut parser = StreamingJsonParser::new();
        parser.feed(r#"{"text": "行1\n行2"}"#);
        assert!(parser.is_complete());
        assert_eq!(parser.text_content(), "行1\n行2");
    }

    #[test]
    fn test_processor_text_response() {
        let processor = JsonProcessor::new();
        let result = processor.process_response(r#"{"text": "你好", "intent": "reply"}"#);
        assert_eq!(result.text, "你好");
        assert_eq!(result.intent, "reply");
    }

    #[test]
    fn test_processor_tool_call() {
        let processor = JsonProcessor::new();
        let result = processor.process_response(
            r#"前面的文本 {"tool": "search", "arguments": {"q": "test"}}"#,
        );
        assert_eq!(result.text, "前面的文本");
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0]["tool"], "search");
    }

    #[test]
    fn test_processor_plain_text() {
        let processor = JsonProcessor::new();
        let result = processor.process_response("这只是一段纯文本回复");
        assert_eq!(result.text, "这只是一段纯文本回复");
    }

    #[test]
    fn test_response_handler() {
        let mut handler = StreamingResponseHandler::new(|_text| {
            // text 更新回调
        });
        handler = handler.with_tool_call(|_| {});
        handler.feed(r#"{"text": "hello"}"#);
        assert_eq!(handler.full_text(), "hello");
        assert!(handler.get_full_json().is_some());
    }
}
