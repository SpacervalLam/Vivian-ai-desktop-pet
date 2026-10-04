//! Normalize delivery metadata without turning malformed protocol output into speech.
use serde_json::{json, Value};

/// Older responses may omit audience metadata; an explicit conflict must never be rerouted.
pub fn matches_listener(data: &Value, planned: &str) -> bool {
    match data.get("listener") {
        None => true,
        Some(value) => value.as_str() == Some(planned),
    }
}

pub fn normalize(raw: &str, planned_channel: &str) -> Result<Option<Value>, &'static str> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("empty_stream");
    }
    if text.eq_ignore_ascii_case("DONT_NOTIFY") {
        return Ok(None);
    }
    if matches!(text, "direct" | "wechat" | "bubble" | "chat_window") {
        return Err("planning_output_in_dialogue");
    }
    let structured = text.starts_with('{') || text.starts_with('[') || text.starts_with("```");
    let mut data = if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end < start { return Err("invalid_json"); }
        serde_json::from_str::<Value>(&text[start..=end]).map_err(|_| "invalid_json")?
    } else if structured {
        return Err("incomplete_json");
    } else {
        // Some roleplay models write prose despite the JSON instruction. Preserve the
        // prose in the medium chosen before composition; do not invent a transport.
        json!({"text": text, "expression": "neutral"})
    };
    if data.get("notify").and_then(Value::as_bool) == Some(false)
        || data
            .get("notify")
            .and_then(Value::as_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("DONT_NOTIFY"))
    {
        return Ok(None);
    }
    let prose = data
        .get("text")
        .and_then(Value::as_str)
        .ok_or("missing_text")?
        .trim();
    if prose.eq_ignore_ascii_case("DONT_NOTIFY") {
        return Ok(None);
    }
    if prose.is_empty() {
        return Err("empty_text");
    }
    if let Some(channel) = data.get("delivery_channel") {
        let channel = match channel.as_str() {
            Some("direct" | "bubble") => "bubble",
            Some("wechat" | "chat_window") => "chat_window",
            _ => return Err("invalid_channel"),
        };
        if channel != planned_channel {
            return Err("channel_mismatch");
        }
    }
    data["delivery_channel"] = json!(planned_channel);
    Ok(Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_is_locked_independently_of_transport() {
        assert!(matches_listener(&json!({"text":"还在忙呀？"}), "user"));
        assert!(matches_listener(&json!({"listener":"user"}), "user"));
        assert!(matches_listener(&json!({"listener":"nana"}), "nana"));
        assert!(!matches_listener(&json!({"listener":"nana"}), "user"));
        assert!(!matches_listener(&json!({"listener":"user"}), "nana"));
        assert!(!matches_listener(&json!({"listener":null}), "user"));
    }
    #[test]
    fn prose_and_omitted_metadata_keep_the_planned_medium() {
        assert_eq!(
            normalize("你回来啦。", "chat_window").unwrap().unwrap()["delivery_channel"],
            "chat_window"
        );
        assert_eq!(
            normalize(r#"{"text":"你好"}"#, "chat_window")
                .unwrap()
                .unwrap()["delivery_channel"],
            "chat_window"
        );
        assert_eq!(
            normalize(r#"{"text":"你好","delivery_channel":"direct"}"#, "bubble")
                .unwrap()
                .unwrap()["text"],
            "你好"
        );
    }
    #[test]
    fn malformed_output_and_transport_switches_are_not_delivered() {
        for text in [
            "}oops{",
            "{\"text\":",
            "```json\n{",
            r#"{"expression":"happy"}"#,
            r#"{"text":" "}"#,
        ] {
            assert!(normalize(text, "bubble").is_err());
        }
        assert_eq!(
            normalize(r#"{"text":"你好","delivery_channel":"wechat"}"#, "bubble").unwrap_err(),
            "channel_mismatch"
        );
        assert!(normalize("DONT_NOTIFY", "bubble").unwrap().is_none());
        assert_eq!(normalize("direct", "bubble").unwrap_err(), "planning_output_in_dialogue");
        assert!(normalize(r#"{"notify":false,"text":"不要发送"}"#, "bubble")
            .unwrap()
            .is_none());
    }
}
