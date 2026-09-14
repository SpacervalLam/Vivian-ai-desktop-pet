//! 原生消息序列化器 —— `ProtocolSpec.request.message_format` 选择的转换层
//!
//! 把统一 `ChatMessage[]` 序列化为各协议的消息数组 / 内容块。设计边界：
//! 角色映射、工具调用重建（Gemini `functionResponse.name`）、Anthropic system
//! 提取、多模态 parts 属于复杂逻辑，保留在 Rust 实现，不写进协议 JSON。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::providers::spec::MessageFormat;
use crate::types::response::ChatMessage;

/// 按消息格式序列化（返回消息数组字段值：messages / input / contents 数组）。
///
/// `instructions` 为系统指令（Chat Completions 风格注入为首条 system 消息；
/// 其余格式由调用方按 `instructions_field` 注入，不在此处理）。
pub fn serialize(
    format: MessageFormat,
    messages: &[ChatMessage],
    instructions: &Option<String>,
) -> Value {
    match format {
        MessageFormat::ChatCompletions => Value::Array(chat_completions(messages, instructions)),
        MessageFormat::ResponsesInput => Value::Array(responses_input(messages)),
        MessageFormat::GeminiContents => gemini_contents(messages),
        MessageFormat::AnthropicMessages => Value::Array(anthropic_messages(messages)),
    }
}

/// Chat Completions `messages[]`（chat_completions.rs::build_messages 等价）
pub fn chat_completions(messages: &[ChatMessage], instructions: &Option<String>) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();
    if let Some(instr) = instructions {
        if !instr.is_empty() {
            result.push(json!({"role": "system", "content": instr}));
        }
    }
    for m in messages {
        match m.role.as_str() {
            "system" => {
                result.push(json!({"role": "system", "content": m.content}));
            }
            "assistant" => {
                let mut msg = json!({"role": "assistant", "content": m.content});
                if let Some(tcs) = &m.tool_calls {
                    let tc_arr: Vec<Value> = tcs
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                }
                            })
                        })
                        .collect();
                    msg["tool_calls"] = Value::Array(tc_arr);
                }
                if let Some(reasoning) = &m.reasoning {
                    if !reasoning.is_empty() {
                        msg["reasoning_content"] = json!(reasoning);
                    }
                }
                result.push(msg);
            }
            "tool" => {
                result.push(json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                }));
            }
            _ => {
                result.push(user_like_with_images(m, "image_url", "image_url", "url", Some("detail")));
            }
        }
    }
    result
}

/// Responses API `input[]`（openai_compat / openai_responses build_input_from_chat 等价）
pub fn responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    let mut input: Vec<Value> = Vec::new();
    for m in messages {
        match m.role.as_str() {
            "system" => {
                input.push(json!({"role": "system", "content": m.content}));
            }
            "assistant" => {
                if !m.content.is_empty() {
                    input.push(json!({"role": "assistant", "content": m.content}));
                }
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        let args_str = serde_json::to_string(&tc.arguments)
                            .unwrap_or_else(|_| "{}".to_string());
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": args_str,
                        }));
                    }
                }
            }
            "tool" => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content,
                }));
            }
            _ => {
                input.push(user_like_with_images(m, "input_text", "input_image", "image_url", None));
            }
        }
    }
    input
}

/// Gemini `contents[]`（gemini.rs::build_contents_from_chat 等价）
pub fn gemini_contents(messages: &[ChatMessage]) -> Value {
    let mut tool_call_names: HashMap<String, String> = HashMap::new();
    for m in messages {
        if m.role == "assistant" {
            if let Some(tcs) = &m.tool_calls {
                for tc in tcs {
                    tool_call_names.insert(tc.id.clone(), tc.name.clone());
                }
            }
        }
    }

    let contents: Vec<Value> = messages
        .iter()
        .map(|m| {
            if m.role == "assistant" {
                if let Some(tcs) = &m.tool_calls {
                    let mut parts: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(json!({"text": m.content}));
                    }
                    for tc in tcs {
                        parts.push(json!({
                            "functionCall": {
                                "name": tc.name,
                                "args": tc.arguments,
                            }
                        }));
                    }
                    json!({"role": "model", "parts": parts})
                } else {
                    json!({"role": "model", "parts": [{"text": m.content}]})
                }
            } else if m.role == "tool" {
                let name = m
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| tool_call_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                let response = serde_json::from_str::<Value>(&m.content)
                    .unwrap_or_else(|_| json!({"result": m.content}));
                json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": response,
                        }
                    }]
                })
            } else {
                let role = &m.role;
                if let Some(imgs) = &m.images {
                    if !imgs.is_empty() {
                        let mut parts: Vec<Value> = Vec::new();
                        if !m.content.is_empty() {
                            parts.push(json!({"text": m.content}));
                        }
                        for img in imgs {
                            if !img.data.is_empty() {
                                parts.push(json!({
                                    "inline_data": {
                                        "mime_type": img.media_type,
                                        "data": img.data,
                                    }
                                }));
                            } else if let Some(u) = &img.url {
                                parts.push(json!({
                                    "file_data": {
                                        "mime_type": img.media_type,
                                        "file_uri": u,
                                    }
                                }));
                            }
                        }
                        json!({"role": role, "parts": parts})
                    } else {
                        json!({"role": role, "parts": [{"text": m.content}]})
                    }
                } else {
                    json!({"role": role, "parts": [{"text": m.content}]})
                }
            }
        })
        .collect();
    json!(contents)
}

/// Anthropic Messages（anthropic.rs::convert_messages 等价；system 由调用方提取）
///
/// 返回 messages 数组；system 文本单独通过 [`anthropic_system`] 提取。
pub fn anthropic_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut converted: Vec<Value> = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role.as_str() {
            "system" => {
                // Anthropic 的 system 在单独字段，这里跳过（由 anthropic_system 提取）
            }
            "assistant" => {
                if let Some(tc) = &m.tool_calls {
                    let mut content_arr: Vec<Value> = Vec::new();
                    if let Some(r) = &m.reasoning {
                        if !r.is_empty() {
                            content_arr.push(json!({
                                "type": "thinking",
                                "thinking": r,
                            }));
                        }
                    }
                    if !m.content.is_empty() {
                        content_arr.push(json!({"type": "text", "text": m.content}));
                    }
                    for c in tc {
                        content_arr.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": c.arguments,
                        }));
                    }
                    converted.push(json!({"role": "assistant", "content": content_arr}));
                } else {
                    let mut content_arr: Vec<Value> = Vec::new();
                    if let Some(r) = &m.reasoning {
                        if !r.is_empty() {
                            content_arr.push(json!({
                                "type": "thinking",
                                "thinking": r,
                            }));
                        }
                    }
                    if content_arr.is_empty() {
                        converted.push(json!({"role": "assistant", "content": m.content}));
                    } else {
                        if !m.content.is_empty() {
                            content_arr.push(json!({"type": "text", "text": m.content}));
                        }
                        converted.push(json!({"role": "assistant", "content": content_arr}));
                    }
                }
            }
            "tool" => {
                let tool_use_id = m.tool_call_id.clone().unwrap_or_default();
                converted.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": m.content,
                    }]
                }));
            }
            "user" => {
                if let Some(imgs) = &m.images {
                    if !imgs.is_empty() {
                        let mut content_arr: Vec<Value> =
                            vec![json!({"type": "text", "text": m.content})];
                        for img in imgs {
                            if !img.data.is_empty() {
                                content_arr.push(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": img.media_type,
                                        "data": img.data,
                                    }
                                }));
                            } else if let Some(u) = &img.url {
                                content_arr.push(json!({
                                    "type": "image",
                                    "source": {"type": "url", "url": u}
                                }));
                            }
                        }
                        converted.push(json!({"role": "user", "content": content_arr}));
                        continue;
                    }
                }
                converted.push(json!({"role": "user", "content": m.content}));
            }
            _ => converted.push(json!({"role": "user", "content": m.content})),
        }
    }
    converted
}

/// 提取 Anthropic 的 system 文本（多条 system 拼接）
pub fn anthropic_system(messages: &[ChatMessage]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for m in messages {
        if m.role == "system" && !m.content.is_empty() {
            parts.push(m.content.clone());
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// user 角色 + 多模态 parts 的通用包装
///
/// - `text_part_key`: Chat Completions 用 `"text"`，Responses 用 `"input_text"`
/// - `image_part_key`: `"image_url"`（OpenAI 族）或 `"input_image"`（Responses）
/// - `image_url_field`: image 的 url 字段名
/// - `detail_field`: 可选的 detail 采样字段名
fn user_like_with_images(
    m: &ChatMessage,
    text_part_key: &str,
    image_part_key: &str,
    image_url_field: &str,
    detail_field: Option<&str>,
) -> Value {
    if let Some(imgs) = &m.images {
        if imgs.is_empty() {
            return json!({"role": m.role, "content": m.content});
        }
        let mut content_arr: Vec<Value> = vec![json!({text_part_key: m.content})];
        for img in imgs {
            let image_url = if !img.data.is_empty() {
                format!("data:{};base64,{}", img.media_type, img.data)
            } else if let Some(u) = &img.url {
                u.clone()
            } else {
                continue;
            };
            let mut part = json!({
                image_part_key: {image_url_field: image_url},
            });
            if let Some(detail) = &img.detail {
                if let Some(df) = detail_field {
                    part[image_part_key][df] = json!(detail);
                }
            }
            content_arr.push(part);
        }
        json!({"role": m.role, "content": content_arr})
    } else {
        json!({"role": m.role, "content": m.content})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            timestamp: None,
            meta: None,
        }
    }

    #[test]
    fn chat_completions_system_first() {
        let msgs = vec![
            text_msg("system", "sys"),
            text_msg("user", "hi"),
        ];
        let out = chat_completions(&msgs, &Some("instr".into()));
        assert_eq!(out[0]["content"], json!("instr"));
        assert_eq!(out[1]["role"], json!("system"));
        assert_eq!(out[2]["role"], json!("user"));
    }

    #[test]
    fn responses_assistant_tool_calls_split() {
        let mut m = text_msg("assistant", "");
        m.tool_calls = Some(vec![crate::types::response::MessageToolCall {
            id: "call_1".into(),
            name: "get_weather".into(),
            arguments: json!({"city": "bj"}),
        }]);
        let out = responses_input(&[m]);
        assert_eq!(out[0]["type"], json!("function_call"));
        assert_eq!(out[0]["name"], json!("get_weather"));
        assert_eq!(out[0]["arguments"], json!("{\"city\":\"bj\"}"));
    }

    #[test]
    fn gemini_tool_response_name_mapping() {
        let mut assistant = text_msg("assistant", "");
        assistant.tool_calls = Some(vec![crate::types::response::MessageToolCall {
            id: "t1".into(),
            name: "get_weather".into(),
            arguments: json!({}),
        }]);
        let tool = text_msg("tool", "{\"temp\": 20}");
        let mut tool = tool;
        tool.tool_call_id = Some("t1".into());
        let contents = gemini_contents(&[assistant, tool]);
        let arr = contents.as_array().unwrap();
        assert_eq!(arr[0]["parts"][0]["functionCall"]["name"], json!("get_weather"));
        assert_eq!(arr[1]["parts"][0]["functionResponse"]["name"], json!("get_weather"));
        assert_eq!(arr[1]["role"], json!("user"));
    }

    #[test]
    fn anthropic_tool_result_user_role() {
        let tool = text_msg("tool", "result");
        let mut tool = tool;
        tool.tool_call_id = Some("tu1".into());
        let out = anthropic_messages(&[tool]);
        assert_eq!(out[0]["role"], json!("user"));
        assert_eq!(out[0]["content"][0]["type"], json!("tool_result"));
        assert_eq!(out[0]["content"][0]["tool_use_id"], json!("tu1"));
    }

    #[test]
    fn anthropic_system_extracted() {
        let msgs = vec![text_msg("system", "a"), text_msg("user", "b"), text_msg("system", "c")];
        assert_eq!(anthropic_system(&msgs), Some("a\n\nc".to_string()));
        assert!(anthropic_messages(&msgs).iter().all(|m| m["role"] != "system"));
    }
}
