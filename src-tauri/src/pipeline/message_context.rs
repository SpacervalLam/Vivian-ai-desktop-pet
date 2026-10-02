//! Shared role-preserving context assembly for chat and proactive generation.
use crate::types::response::ChatMessage;
use super::prompt_modules::{section_heading, STATIC_OPEN, STATIC_CLOSE, SYSTEM_PROMPT_DYNAMIC_BOUNDARY};

pub struct PromptContext<'a> {
    pub system: &'a str,
    pub dynamic: Option<&'a str>,
}

pub fn split_prompt_context<'a>(prompt: &'a str, user_input: &str, language: &str) -> PromptContext<'a> {
    let Some(after_open) = prompt.trim_start().strip_prefix(STATIC_OPEN) else {
        return PromptContext { system: prompt.trim(), dynamic: None };
    };
    let Some((system, tail)) = after_open.split_once(STATIC_CLOSE) else {
        return PromptContext { system: prompt.trim(), dynamic: None };
    };
    let Some(dynamic) = tail.trim_start().strip_prefix(SYSTEM_PROMPT_DYNAMIC_BOUNDARY) else {
        return PromptContext { system: prompt.trim(), dynamic: None };
    };
    // Remove only the exact terminal user section, never a heading quoted in memory or input.
    let marker = format!("{}\n", section_heading("user_input", language));
    let dynamic = if !user_input.is_empty() {
        dynamic.trim_end().strip_suffix(user_input.trim_end())
            .and_then(|prefix| prefix.strip_suffix(marker.as_str()))
            .unwrap_or(dynamic)
    } else { dynamic };
    let dynamic = dynamic.trim();
    PromptContext { system: system.trim(), dynamic: (!dynamic.is_empty()).then_some(dynamic) }
}

pub fn ensure_speaker_prefix(content: &str) -> String {
    let (_, speaker, _) = crate::cross_character::parse_any_speaker_prefix(content);
    if speaker.is_some() { content.to_owned() } else { format!("[User says to me] {}", content) }
}

pub fn append_history(messages: &mut Vec<ChatMessage>, history: &[ChatMessage]) {
    messages.extend(history.iter().map(|message| {
        if message.role == "user" {
            ChatMessage { content: ensure_speaker_prefix(&message.content), ..message.clone() }
        } else { let mut message = message.clone();
            if let Some(sticker)=message.meta.as_ref().and_then(|m|m.sticker.as_ref()){message.content.push_str(&format!("\n[Sent sticker: {}]",serde_json::json!({"label":sticker.label,"meaning":sticker.meaning})));}
            message
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips_only_terminal_input_in_each_language() {
        for lang in ["zh", "en", "ja"] {
            let input = "a quoted # User Input\ninside the real input";
            let note = format!("Memory quotes {}\nan old question", section_heading("user_input", lang));
            let prompt = format!("{STATIC_OPEN}persona{STATIC_CLOSE}\n{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}\n{note}\n{}\n{input}", section_heading("user_input", lang));
            let split = split_prompt_context(&prompt, input, lang);
            assert_eq!(split.system, "persona");
            assert_eq!(split.dynamic, Some(note.as_str()));
        }
    }
    #[test]
    fn real_builder_input_is_not_duplicated_into_runtime_note() {
        use crate::pipeline::prompt_modules::{PromptBuilder, PromptParts};
        for language in ["zh", "en", "ja"] {
            let input = "unique_current_input\nwith trailing whitespace  \n";
            let parts = PromptParts {
                language: language.into(),
                user_input: input.into(),
                character_block: Some("unique_persona_identity".into()),
                ..Default::default()
            };
            let prompt = PromptBuilder::build_prompt(&parts);
            let context = split_prompt_context(&prompt, input, language);
            assert!(context.system.contains("unique_persona_identity"));
            let dynamic = context.dynamic.unwrap();
            assert!(dynamic.contains("RUNTIME DATA BOUNDARY"));
            assert!(!dynamic.contains("unique_current_input"));
            assert!(!dynamic.contains("unique_persona_identity"));
        }
    }
    #[test]
    fn malformed_markers_fall_back_without_panicking() {
        for prompt in ["legacy", "<static>unfinished", "<static>x</static>missing boundary"] {
            assert_eq!(split_prompt_context(prompt, "", "zh").system, prompt);
        }
    }
    #[test]
    fn history_keeps_roles_and_existing_speakers() {
        let history = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];
        let mut messages = Vec::new();
        append_history(&mut messages, &history);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "[User says to me] hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "hi");
    }
}
