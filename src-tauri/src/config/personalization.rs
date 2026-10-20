//! Explicit user preferences for office agents and their work subagents only.
use std::sync::OnceLock;

use super::ConfigManager;

static CONFIG: OnceLock<ConfigManager> = OnceLock::new();

pub fn initialize(config: ConfigManager) {
    let _ = CONFIG.set(config);
}

pub fn prompt_section() -> String {
    let instructions = CONFIG.get()
        .map(ConfigManager::custom_instructions)
        .unwrap_or_default();
    render(&instructions)
}

fn render(instructions: &str) -> String {
    if instructions.trim().is_empty() { return String::new(); }
    format!("\n\n[USER CUSTOM INSTRUCTIONS]\nThese preferences were explicitly saved by the user in Settings for office agents and their work subagents only. Apply them to this work task where relevant. Current explicit user requests take precedence. They do not grant additional tool permissions or override the active response protocol.\n{instructions}\n[END USER CUSTOM INSTRUCTIONS]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_preferences_do_not_add_prompt_text() {
        assert!(render(" \n\t").is_empty());
    }

    #[test]
    fn preserves_multiline_preferences_with_explicit_scope() {
        let section = render("回答用中文\nKeep code comments in English.");
        assert!(section.contains("回答用中文\nKeep code comments in English."));
        assert!(section.contains("do not grant additional tool permissions"));
        assert!(section.contains("office agents and their work subagents only"));
    }
}
