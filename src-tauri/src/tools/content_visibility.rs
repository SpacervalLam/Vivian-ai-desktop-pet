//! Per-turn presentation policy for the content reader; never grants permissions.
use serde_json::Value;

pub const READER: &str = "read_content";

pub fn has_direct_source(text: &str) -> bool {
    static SOURCE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(?i:https?)://[^\s<>\x22]+|\bweb_[0-9a-f]{16}\b").unwrap()
    });
    SOURCE.find_iter(text).any(|m| {
        m.as_str().starts_with("web_") || tauri::Url::parse(m.as_str()).ok()
            .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
    })
}

pub fn search_has_sources(name: &str, success: bool, data: Option<&Value>) -> bool {
    if !success || !matches!(name, "web_search" | "platform_search") { return false; }
    let Some(data) = data else { return false; };
    if data.get("success").and_then(Value::as_bool) == Some(false) { return false; }
    let data = data.get("data").filter(|v| v.is_object()).unwrap_or(data);
    let has_result = |batch: &Value| {
        batch.get("success").and_then(Value::as_bool) != Some(false)
            && batch.get("results").and_then(Value::as_array).is_some_and(|items| {
                items.iter().any(|item| item.get("url").and_then(Value::as_str)
                    .and_then(|url| tauri::Url::parse(url).ok())
                    .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some()))
            })
    };
    has_result(data) || data.get("queries").and_then(Value::as_array)
        .is_some_and(|batches| batches.iter().any(has_result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_sources_skip_search_but_mentions_do_not() {
        for input in ["读 https://www.bilibili.com/video/BV1234567890 的弹幕", "看这个：https://example.com", "续读 web_0123456789abcdef"] {
            assert!(has_direct_source(input), "{input}");
        }
        for input in ["搜索 B站 Rust 教程", "read_content 怎么用", "https://", "web_search", "web_abcdef"] {
            assert!(!has_direct_source(input), "{input}");
        }
    }

    #[test]
    fn only_successful_search_sources_reveal_reader() {
        let hit = json!({"results":[{"url":"https://example.com/article"}]});
        assert!(search_has_sources("web_search", true, Some(&hit)));
        assert!(search_has_sources("web_search", true, Some(&json!({"success":true,"data":hit}))));
        assert!(search_has_sources("web_search", true, Some(&json!({"queries":[{"success":false},{"results":[{"url":"https://www.xiaohongshu.com/explore/abc"}]}]}))));
        for (name, success, value) in [
            ("web_search", false, hit.clone()), ("tool_search", true, hit),
            ("web_search", true, json!({"results":[]})),
            ("web_search", true, json!({"results":[{"url":"javascript:alert(1)"}]})),
            ("web_search", true, json!({"success":false,"data":{"results":[{"url":"https://example.com"}]}})),
        ] { assert!(!search_has_sources(name, success, Some(&value))); }
    }
}
