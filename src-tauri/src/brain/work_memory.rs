//! Shared work-memory controls and bounded, query-based recall. No model or user-data access.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MemoryPolicy {
    pub use_memories: bool,
    pub generate_memories: bool,
    pub revision: u64,
}
impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            use_memories: true,
            generate_memories: true,
            revision: 0,
        }
    }
}
impl MemoryPolicy {
    pub fn accepts_generation(&self, input_revision: u64) -> bool {
        self.generate_memories && self.revision == input_revision
    }
}

/// Ignore pleasantries while retaining research, failures, terminal work and short corrections.
pub fn should_extract(user: &str, assistant: &str, has_tool: bool) -> bool {
    if assistant.trim().is_empty() || user.trim().is_empty() {
        return false;
    }
    has_tool
        || user.chars().count() + assistant.chars().count() >= 80
        || [
            "记住",
            "以后",
            "不要",
            "纠正",
            "改为",
            "偏好",
            "不是",
            "应该",
            "请用",
            "改用",
            "别再",
            "今後",
            "覚えて",
            "remember",
            "prefer",
            "correction",
            "always",
            "never",
        ]
        .iter()
        .any(|term| user.to_lowercase().contains(term))
}

fn terms(text: &str) -> std::collections::BTreeSet<String> {
    let lower = text.to_lowercase();
    let mut result = std::collections::BTreeSet::new();
    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if token.chars().count() >= 2 {
            result.insert(token.to_string());
        }
        let chars: Vec<_> = token.chars().collect();
        for pair in chars.windows(2) {
            if pair.iter().all(|c| ('\u{3400}'..='\u{9fff}').contains(c)) {
                result.insert(pair.iter().collect());
            }
        }
    }
    result
}

/// Return ranked original sections, including provenance markers, rather than invented summaries.
pub fn recall(raw: &str, query: &str, budget: usize) -> String {
    let query = terms(query);
    if query.is_empty() {
        return String::new();
    }
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut section_line = 1;
    let mut fence: Option<&str> = None;
    for (index, part) in raw.split_inclusive('\n').enumerate() {
        let marker = if part.trim_start().starts_with("```") {
            Some("```")
        } else if part.trim_start().starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(marker) = marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
        }
        if fence.is_none() && part.starts_with("## ") && !current.is_empty() {
            sections.push((section_line, std::mem::take(&mut current)));
            section_line = index + 1;
        }
        current.push_str(part);
    }
    if !current.is_empty() {
        sections.push((section_line, current));
    }
    let mut ranked: Vec<_> = sections
        .into_iter()
        .filter_map(|(line, text)| {
            if text.contains("<!-- superseded:")
                || text.contains("[已被后续条目替代，勿作为当前约定]")
                || text.starts_with("## 整理来源材料")
            {
                return None;
            }
            let score = terms(&text).intersection(&query).count();
            (score > 0).then_some((score, line, text))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut output = String::new();
    for (_, line, text) in ranked.into_iter().take(4) {
        let block = format!("\n[原文起始行 {line}]\n{text}\n");
        let remaining = budget.saturating_sub(output.chars().count());
        if remaining < 100 {
            break;
        }
        output.extend(block.chars().take(remaining));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_and_independent_controls() {
        assert_eq!(
            serde_json::from_str::<MemoryPolicy>("{}").unwrap(),
            MemoryPolicy::default()
        );
        let p: MemoryPolicy =
            serde_json::from_str(r#"{"use_memories":false,"generate_memories":true}"#).unwrap();
        assert!(!p.use_memories && p.generate_memories);
    }
    #[test]
    fn revoked_jobs_do_not_revive_when_generation_is_enabled_again() {
        let mut p = MemoryPolicy::default();
        assert!(p.accepts_generation(0));
        p.generate_memories = false;
        p.revision = 1;
        assert!(!p.accepts_generation(0));
        p.generate_memories = true;
        p.revision = 2;
        assert!(!p.accepts_generation(0));
        assert!(p.accepts_generation(2));
    }
    #[test]
    fn non_editing_work_and_corrections_are_eligible_but_greetings_are_not() {
        assert!(should_extract("研究结果", "已有核验的资料", true));
        assert!(should_extract("排查失败原因", "终端执行失败", true));
        assert!(should_extract("以后使用中文", "好的", false));
        assert!(should_extract("不是 yarn，应该用 pnpm", "好的", false));
        assert!(!should_extract("你好", "你好", false));
        assert!(!should_extract("记住这个", "", false));
    }
    #[test]
    fn ranked_recall_keeps_sources_and_budget() {
        let raw = "# Memory\n## 无关\n喜欢茶\n## Rust\n<!-- memory-entry: memory_entries/abc.json -->\n编译失败要检查 Rust 工具链\n## 部署\n只手动部署\n";
        let recalled = recall(raw, "Rust 编译失败怎么解决", 220);
        assert!(recalled.contains("工具链") && recalled.contains("memory_entries/abc"));
        assert!(!recalled.contains("喜欢茶"));
        assert!(recalled.chars().count() <= 220);
        assert!(recall(raw, "毫不相关", 220).is_empty());
    }
}
