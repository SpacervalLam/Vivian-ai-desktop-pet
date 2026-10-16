//! Wire types and lossless MCP results, shared by both transports.
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn tool_name(server: &str, remote: &str) -> String {
    let prefix = format!("mcp__{server}__");
    if prefix.len() + remote.len() <= 64 && remote.chars().all(|c|c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return format!("{prefix}{remote}");
    }
    let suffix = format!("{:x}", Sha256::digest(remote.as_bytes()));
    let room = 64usize.saturating_sub(prefix.len() + 9);
    let readable: String = remote.chars().map(|c|if c.is_ascii_alphanumeric() || c == '_' || c == '-' {c} else {'_'}).take(room).collect();
    format!("{prefix}{readable}_{}", &suffix[..8])
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct McpToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema", alias = "input_schema")]
    pub input_schema: Option<Value>,
}

pub(super) fn result_payload(result: Value) -> Value {
    let output = result.get("content").and_then(Value::as_array).map(|blocks| {
        blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>().join("\n")
    }).unwrap_or_default();
    let mut payload = result;
    if !payload.is_object() { payload = json!({"raw": payload}); }
    payload["output"] = if output.is_empty() {
        Value::String(serde_json::to_string(&payload).unwrap_or_default())
    } else { Value::String(output) };
    payload
}

/// SSE events can span arbitrary network chunks and multiple data lines.
pub(super) fn take_sse_messages(buffer: &mut Vec<u8>) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    loop {
        let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
        let crlf = buffer.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4));
        let boundary = match (lf, crlf) { (Some(a), Some(b)) => Some(if a.0 < b.0 {a} else {b}), (a, b) => a.or(b) };
        let Some((end, separator)) = boundary else { break };
        let event: Vec<_> = buffer.drain(..end + separator).collect();
        let text = std::str::from_utf8(&event).map_err(|_| "MCP SSE is not UTF-8")?;
        let data = text.lines().filter_map(|l| l.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s)))
            .collect::<Vec<_>>().join("\n");
        if !data.is_empty() {
            messages.push(serde_json::from_str(&data).map_err(|e| format!("Invalid MCP SSE message: {e}"))?);
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_long_and_punctuated_tool_names_stay_provider_compatible() {
        let name = tool_name("01234567890123456789012345678901", "get.current-project-configuration-long-name");
        assert!(name.len() <= 64);
        assert!(name.chars().all(|c|c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert_ne!(tool_name("a", "x.y"), tool_name("a", "x_y"));
        assert_eq!(tool_name("fs", "read"), "mcp__fs__read");
    }
    #[test]
    fn extension_standard_schema_keeps_required_fields() {
        let t: McpToolInfo = serde_json::from_value(json!({"name":"read", "inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}})).unwrap();
        assert_eq!(t.input_schema.unwrap()["required"], json!(["path"]));
    }
    #[test]
    fn extension_mixed_results_and_errors_are_lossless() {
        let original = json!({"isError":true,"content":[{"type":"text","text":"failed"},{"type":"image","data":"abc","mimeType":"image/png"}],"structuredContent":{"code":7}});
        let payload = result_payload(original.clone());
        assert_eq!(payload["content"], original["content"]);
        assert_eq!(payload["structuredContent"], original["structuredContent"]);
        assert_eq!(payload["isError"], true);
        assert_eq!(payload["output"], "failed");
    }
    #[test]
    fn extension_sse_handles_partial_utf8_crlf_and_multiple_events() {
        let mut buffer = b": keepalive\r\n\r\ndata: {\"id\":1,\"result\":{}}\r".to_vec();
        assert!(take_sse_messages(&mut buffer).unwrap().is_empty());
        buffer.extend_from_slice(b"\n\r\ndata: {\"id\":2,\"result\":{}}\n\n");
        let messages = take_sse_messages(&mut buffer).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], 1);
        assert!(buffer.is_empty());
    }
}
