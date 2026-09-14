//! Vivian 通用 LLM 输出 JSON Schema
//!
//! 通过 `schemars::schema_for!` 从 `ProcessedResponse` 自动生成，保证 schema
//! 与 Rust 结构体字段同步。通过各 provider 的 Structured Outputs / JSON Mode
//! 能力下发约束，让 LLM 按结构化 JSON 返回。
//!
//! 设计原则:
//! - schema 字段对齐 `ProcessedResponse`，新增/修改字段自动反映到 schema
//! - `tool_calls` / `control_actions` 用 `#[schemars(skip)]` 跳过 —— 工具调用
//!   交给原生 Function Calling 通道，不进 schema
//! - 主调用 schema 仅约束 `text` / `intent` / `response_mode` 三个字段，
//!   其余结构化字段（表情/动作/心理状态/长期记忆等）由反思调用独立产出
//!
//! 各 provider 注入方式:
//! - OpenAI Responses: `text.format.type=json_schema` (strict=true)
//! - 豆包 Responses: `response_format.type=json_schema` (strict=true)
//! - Gemini: `generationConfig.responseSchema`
//! - Anthropic: 包装成 `emit_response` 伪工具的 `input_schema`
//! - OpenAI 兼容(DeepSeek/Qwen): 仅 `response_format.type=json_object`，
//!   schema 在后端 `validate_vivian_response` 校验
//! - Spark/Wenxin: 不传 schema，纯 prompt 约束 + JsonParser 兜底

use serde_json::{json, Value};
use once_cell::sync::Lazy;

use crate::brain::json_parser::ProcessedResponse;

/// Vivian 通用 LLM 输出 Schema（静态单例，全局共享）
///
/// 由 `schemars::schema_for!(ProcessedResponse)` 自动生成，对齐 Rust 结构体。
/// 调用方通过 `vivian_response_schema()` 获取，传给 `LLMRequest::with_json_schema`。
pub static VIVIAN_RESPONSE_SCHEMA: Lazy<Value> = Lazy::new(build_vivian_response_schema);

/// 获取 Vivian 通用响应 Schema（克隆）
pub fn vivian_response_schema() -> Value {
    VIVIAN_RESPONSE_SCHEMA.clone()
}

/// 从 `ProcessedResponse` 自动生成 JSON Schema
///
/// 使用 schemars 0.8 的 `schema_for!` 宏，返回 `RootSchema` 后取其 `.schema`
/// 字段（去掉 `$schema` / `title` 顶层元数据，OpenAI strict mode 要求）。
fn build_vivian_response_schema() -> Value {
    let root = schemars::schema_for!(ProcessedResponse);
    // RootSchema.schema 是 Schema 对象，序列化为 Value 即为标准 JSON Schema
    serde_json::to_value(&root.schema).unwrap_or_else(|_| {
        json!({
            "type": "object",
            "description": "Vivian LLM response (schema generation failed)"
        })
    })
}

/// Anthropic `emit_response` 伪工具定义
///
/// Claude 没有 `response_format` / JSON Schema 约束能力，所有结构化输出
/// 必须包装成 `tool_use` 块。本函数返回一个"伪工具"定义，其 `input_schema`
/// 就是 Vivian 通用响应 Schema，强制 LLM 通过 `tool_use` 通道返回结构化字段。
///
/// 调用方（AnthropicProvider）应把它追加到 `tools` 数组中，并设置
/// `tool_choice={"type":"tool","name":"emit_response"}` 强制 LLM 必须调用此工具。
pub fn emit_response_tool_definition() -> Value {
    json!({
        "name": "emit_response",
        "description": "输出本轮对话的结构化响应字段（必须调用）",
        "input_schema": vivian_response_schema()
    })
}

/// 判断 tool_use 是否为 emit_response 伪工具调用
pub fn is_emit_response_call(tool_name: &str) -> bool {
    tool_name == "emit_response"
}

/// 校验 LLM 返回的 JSON 是否符合 Vivian Schema
///
/// 用于不支持 Structured Outputs 的 provider（DeepSeek/Qwen 等），
/// 在后端校验 LLM 返回的 JSON Mode 输出。
///
/// 校验失败时返回错误列表，调用方据此决定是否回退到 JsonParser 兜底。
pub fn validate_vivian_response(value: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            errors.push("响应不是 JSON 对象".to_string());
            return Err(errors);
        }
    };

    // 必填字段检查（对齐 ProcessedResponse 非 Option 字段）
    let required = [
        "text",
        "intent",
        "response_mode",
    ];
    for key in &required {
        if !obj.contains_key(*key) {
            errors.push(format!("缺少必填字段: {}", key));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
