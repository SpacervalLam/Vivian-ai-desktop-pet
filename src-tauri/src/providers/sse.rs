//! SSE 流读取辅助 —— 把字节流解析为逐行 JSON 事件
//!
//! 支持两种 SSE 风格：
//! - 标准 `data: <json>` 行（可选 `[DONE]` 哨兵）
//! - 事件名路由（OpenAI Responses 风格：每行 JSON 带 `type` 字段，
//!   无 `data:` 前缀，直接是 JSON 行）

use futures::StreamExt;
use serde_json::Value;

/// 一个已解析的 SSE 事件
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// 原始 data 行内容（去掉 `data:` 前缀并 trim）
    pub data: String,
    /// 解析后的 JSON（失败为 None，调用方可按原始 data 判断哨兵）
    pub json: Option<Value>,
}

/// 逐行读取字节流，产出 SSE 事件。
///
/// `event_path` 为行前缀（默认 `data:`，`event_path=None` 表示整行即 JSON）。
/// `done_sentinel` 匹配到后返回 `Done` 提前结束（None 表示无哨兵，读到 EOF 结束）。
pub async fn read_sse<B, E>(
    stream: &mut (impl futures::Stream<Item = Result<B, E>> + Unpin),
    event_path: Option<&str>,
    done_sentinel: Option<&str>,
    mut on_event: impl FnMut(SseEvent) -> bool,
) -> Result<(), String>
where
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut buffer = String::new();
    let prefix = event_path.unwrap_or("data:");
    let has_prefix = !prefix.is_empty();

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(error) => return Err(error.to_string()),
        };
        buffer.push_str(&String::from_utf8_lossy(chunk.as_ref()));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_string();
            buffer = buffer[pos + 1..].to_string();
            if line.is_empty() {
                continue;
            }

            let data = if has_prefix {
                match line.strip_prefix(prefix) {
                    Some(rest) => rest.trim().to_string(),
                    None => continue,
                }
            } else {
                line.trim().to_string()
            };

            if data.is_empty() {
                continue;
            }

            // 哨兵判断
            if let Some(sentinel) = done_sentinel {
                if data == sentinel {
                    return Ok(());
                }
            }

            let json = serde_json::from_str::<Value>(&data).ok();
            let event = SseEvent { data, json };
            if !on_event(event) {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_prefix_and_trim() {
        // 只验证事件解析逻辑（read_sse 为异步，这里验证 on_event 数据流靠集成测试）
        let data = "data: {\"a\": 1}";
        let prefix = "data:";
        let rest = data.strip_prefix(prefix).unwrap().trim();
        assert_eq!(rest, "{\"a\": 1}");
        let v: Value = serde_json::from_str(rest).unwrap();
        assert_eq!(v, json!({"a": 1}));
    }
}
