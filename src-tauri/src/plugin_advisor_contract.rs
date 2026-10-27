//! Persistent capability gaps and reviewed plugin suggestions. No execution or network I/O.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PREVIEW_TOOLS: &[&str] = &["create_tool", "create_plugin", "install_plugin"];
pub fn needs_concrete_preview(name: &str) -> bool {
    PREVIEW_TOOLS.contains(&name)
}
pub const DAY: i64 = 86_400;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub id: String,
    pub char_id: String,
    pub need: String,
    pub evidence: Vec<String>,
    pub created_at: i64,
    pub last_seen: i64,
    pub last_search: i64,
    pub search_error: Option<String>,
    pub resolved: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Discovered,
    Reviewed,
    Build,
    Installed,
    Loaded,
    Verified,
    Failed,
    Dismissed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub gap_id: String,
    pub char_id: String,
    pub name: String,
    pub source: String,
    pub revision: Option<String>,
    pub reason: String,
    pub status: Status,
    pub error: Option<String>,
    pub preview: Option<Value>,
    pub cache_path: Option<String>,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub tested_tools: Vec<String>,
    pub notified: bool,
    #[serde(default)]
    pub notification_id: Option<String>,
    #[serde(default)]
    pub loaded_fingerprint: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    pub automatic: bool,
    pub last_auto_search: i64,
    pub gaps: Vec<Gap>,
    pub suggestions: Vec<Suggestion>,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            automatic: true,
            last_auto_search: 0,
            gaps: vec![],
            suggestions: vec![],
        }
    }
}
impl Ledger {
    pub fn record(
        &mut self,
        char_id: &str,
        need: &str,
        evidence: &str,
        now: i64,
    ) -> Result<String, String> {
        let need = need.trim();
        if char_id.is_empty()
            || !(3..=120).contains(&need.chars().count())
            || need.chars().any(char::is_control)
        {
            return Err("能力描述需为 3–120 个字符，不含控制字符".into());
        }
        if let Some(gap) = self.gaps.iter_mut().find(|g| {
            g.char_id == char_id && g.need.to_lowercase() == need.to_lowercase() && !g.resolved
        }) {
            if !gap.evidence.iter().any(|e| e == evidence) && gap.evidence.len() < 8 {
                gap.evidence.push(evidence.into());
            }
            gap.last_seen = now;
            return Ok(gap.id.clone());
        }
        if self.gaps.len() >= 64 {
            self.gaps.retain(|g| !g.resolved);
            self.suggestions
                .retain(|s| self.gaps.iter().any(|g| g.id == s.gap_id));
        }
        if self.gaps.len() >= 64 {
            return Err("能力缺口记录已满，请先关闭已完成的记录".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.gaps.push(Gap {
            id: id.clone(),
            char_id: char_id.into(),
            need: need.into(),
            evidence: vec![evidence.into()],
            created_at: now,
            last_seen: now,
            last_search: 0,
            search_error: None,
            resolved: false,
        });
        Ok(id)
    }
    pub fn next_auto(&self, char_id: &str, now: i64) -> Option<Gap> {
        if !self.automatic || now - self.last_auto_search < DAY {
            return None;
        }
        self.gaps
            .iter()
            .find(|g| {
                g.char_id == char_id
                    && !g.resolved
                    && g.evidence.len() >= 2
                    && (g.last_search == 0 || now - g.last_search >= 7 * DAY)
                    && !self.suggestions.iter().any(|s| s.gap_id == g.id)
            })
            .cloned()
    }
    pub fn insert(&mut self, candidate: Suggestion) -> bool {
        if self.suggestions.iter().any(|s| {
            s.char_id == candidate.char_id
                && s.gap_id == candidate.gap_id
                && s.source == candidate.source
        }) {
            return false;
        }
        if self.suggestions.len() >= 96 {
            return false;
        }
        self.suggestions.push(candidate);
        true
    }
    pub fn dismiss(&mut self, id: &str, char_id: &str) -> Result<(), String> {
        let item = self
            .suggestions
            .iter_mut()
            .find(|s| s.id == id && s.char_id == char_id)
            .ok_or("建议不存在")?;
        item.status = Status::Dismissed;
        item.notified = true;
        Ok(())
    }
}

/// Recommendations use an explicit Vivian contract, never a package's marketing name.
pub fn validate_manifest(raw: &Value) -> Result<(), String> {
    if raw["api"].as_u64() != Some(1) {
        return Err("候选必须明确声明 Vivian 插件 api: 1；DSH/npm 包不能直接安装".into());
    }
    let name = raw["name"].as_str().unwrap_or("");
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("插件名称无效".into());
    }
    if raw["version"].as_str().is_none_or(|v| v.trim().is_empty()) {
        return Err("插件缺少版本".into());
    }
    if ["skills", "tools", "js", "mcp_servers"]
        .iter()
        .any(|key| raw[*key].as_array().is_some_and(|a| !a.is_empty()))
        || ["providers", "embeddings", "protocols"]
            .iter()
            .any(|key| raw[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
    {
        Ok(())
    } else {
        Err("候选没有声明 Vivian 可加载的贡献点".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_advisor_counts_independent_evidence_and_limits_background_search() {
        let mut ledger = Ledger::default();
        let now = 10 * DAY;
        let id = ledger
            .record("vivian", "export_calendar", "session-a", now)
            .unwrap();
        ledger
            .record("vivian", "export_calendar", "session-a", now)
            .unwrap();
        assert!(ledger.next_auto("vivian", now).is_none());
        ledger
            .record("vivian", "export_calendar", "session-b", now)
            .unwrap();
        assert_eq!(ledger.next_auto("vivian", now).unwrap().id, id);
        assert!(ledger.next_auto("nana", now).is_none());
        ledger.last_auto_search = now;
        assert!(ledger.next_auto("vivian", now + 1).is_none());
        ledger.automatic = false;
        assert!(ledger.next_auto("vivian", now + 2 * DAY).is_none());
    }
    #[test]
    fn plugin_advisor_does_not_accept_foreign_or_empty_plugin_contracts() {
        assert!(
            validate_manifest(&serde_json::json!({"name":"dsh-package","main":"index.js"}))
                .is_err()
        );
        assert!(validate_manifest(
            &serde_json::json!({"api":2,"name":"x","version":"1","skills":["a.md"]})
        )
        .is_err());
        assert!(validate_manifest(&serde_json::json!({"api":1,"name":"x","version":"1"})).is_err());
        assert!(validate_manifest(
            &serde_json::json!({"api":1,"name":"x","version":"1","tools":["tools/*.json"]})
        )
        .is_ok());
    }
    #[test]
    fn plugin_advisor_every_executable_package_action_requires_its_own_preview() {
        for name in ["create_tool", "create_plugin", "install_plugin"] {
            assert!(needs_concrete_preview(name));
        }
        assert!(!needs_concrete_preview("plugin_advisor"));
    }
    #[test]
    fn plugin_advisor_dismissals_survive_restart_and_block_recommendation_duplicates() {
        let mut data = Ledger::default();
        let gap = data
            .record("vivian", "calendar export", "one", 10 * DAY)
            .unwrap();
        let item = Suggestion {
            id: "suggestion".into(),
            gap_id: gap.clone(),
            char_id: "vivian".into(),
            name: "calendar".into(),
            source: "https://github.com/a/b".into(),
            revision: None,
            reason: "fixture".into(),
            status: Status::Discovered,
            error: None,
            preview: None,
            cache_path: None,
            session_id: None,
            created_at: 10 * DAY,
            tested_tools: vec![],
            notified: false,
            notification_id: None,
            loaded_fingerprint: None,
        };
        assert!(data.insert(item.clone()));
        assert!(data.dismiss("suggestion", "nana").is_err());
        data.dismiss("suggestion", "vivian").unwrap();
        let mut reloaded: Ledger =
            serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        assert_eq!(reloaded.suggestions[0].status, Status::Dismissed);
        assert!(!reloaded.insert(item));
        reloaded
            .record("vivian", "calendar export", "two", 11 * DAY)
            .unwrap();
        assert!(reloaded.next_auto("vivian", 20 * DAY).is_none());
    }
}
