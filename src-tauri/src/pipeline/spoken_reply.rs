//! One presentation text for delivery, dialogue history and memory.
//! Only transport/rendering syntax may be removed here; never compose dialogue.

pub(crate) const PRIVATE_OPERATION: &str = "[PRIVATE OPERATION GUIDANCE — NOT SPOKEN CONTEXT]";

pub(crate) fn context(mut messages: Vec<crate::types::response::ChatMessage>) -> Vec<crate::types::response::ChatMessage> {
    messages.retain(|message| !(message.role == "tool" || message.tool_calls.is_some()
        || (message.role == "user" && serde_json::from_str::<serde_json::Value>(&message.content)
            .is_ok_and(|v| v["source"] == "tool_protocol_record"))
        || (message.role == "system" && (message.content.starts_with("[INTERNAL STATUS — NOT A USER REQUEST]")
            || message.content.starts_with("<agent_status>") || message.content.starts_with(PRIVATE_OPERATION)))));
    messages
}

pub(crate) fn text(raw: &str) -> String {
    let prose = super::steps::memory::MemorySavingRunnable::strip_json_if_any(raw);
    let prose = crate::utils::protocol_text::strip_protocol_text(&prose);
    let controls = crate::speech::tts::parse_tts_controls(&prose);
    crate::utils::strip_markdown_syntax(&controls.text).trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn formatting_cleanup_preserves_the_authored_words_and_is_idempotent() {
        let raw = r#"{"text":"**Master**，换好了。"}"#;
        let clean = super::text(raw);
        assert_eq!(clean, "Master，换好了。");
        assert_eq!(super::text(&clean), clean);
        assert!(super::text("").is_empty());
    }
}
