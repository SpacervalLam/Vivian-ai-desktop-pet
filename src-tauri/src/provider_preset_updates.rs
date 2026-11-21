//! Contracts for the settings-driven provider preset verification queue.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const MODE: &str = "provider_presets";
pub const TOOLS: &[&str] = &[
    "use_skill",
    "list_provider_presets",
    "manage_provider_preset",
    "web_search",
    "read_content",
    "read_spilled_result",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTarget {
    pub id: String,
    pub label: String,
    pub label_key: Option<String>,
    pub model_count: usize,
    pub verified_at: Option<String>,
    pub verified_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Queued,
    Running,
    Updated,
    Verified,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateItem {
    pub id: String,
    pub label: String,
    pub status: ItemStatus,
    pub message: Option<String>,
    pub session_id: Option<String>,
    pub source: Option<String>,
    pub added: usize,
    pub removed: usize,
    pub model_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJob {
    pub id: String,
    #[serde(default)]
    pub revision: u64,
    pub running: bool,
    pub stop_requested: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub items: Vec<UpdateItem>,
}

impl UpdateJob {
    /// A process restart cannot resume an in-flight model request implicitly.
    pub fn recover(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        self.revision += 1;
        self.stop_requested = true;
        for item in &mut self.items {
            if matches!(item.status, ItemStatus::Queued | ItemStatus::Running) {
                item.status = ItemStatus::Stopped;
                item.message = Some("应用已重启，请重新发起核对".into());
            }
        }
    }
}

pub fn select_targets(all: &[UpdateTarget], ids: &[String]) -> Result<Vec<UpdateTarget>, String> {
    if ids.is_empty() {
        return Err("请选择至少一个厂商".into());
    }
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .map(|id| {
            all.iter()
                .find(|target| target.id == *id)
                .cloned()
                .ok_or_else(|| format!("未找到厂商预设：{id}"))
        })
        .collect()
}

pub fn check_write(target: &str, action: &str, kind: &str, id: &str) -> Result<(), String> {
    if action != "upsert" || kind != "llm" || id != target {
        return Err(format!(
            "本次核对只允许更新 LLM 预设 {target}，不能删除预设或修改其他厂商"
        ));
    }
    Ok(())
}

/// Official domain families for shipped vendors. Other vendors are restricted
/// to the exact public hosts already present in their trusted preset metadata.
pub fn official_roots(id: &str) -> &'static [&'static str] {
    match id {
        "openai" => &["openai.com"],
        "anthropic" => &["anthropic.com", "claude.com"],
        "gemini" => &["google.dev", "google.com"],
        "deepseek" => &["deepseek.com"],
        "qwen" => &["aliyun.com"],
        "glm" => &["bigmodel.cn"],
        "moonshot" => &["moonshot.cn", "moonshot.ai", "kimi.ai"],
        "doubao" => &["volcengine.com"],
        "minimax" => &["minimaxi.com", "minimax.io"],
        "mimo" => &["xiaomimimo.com"],
        "grok" => &["x.ai"],
        "openrouter" => &["openrouter.ai"],
        "groq" => &["groq.com"],
        "ollama" => &["ollama.com"],
        "mistral" => &["mistral.ai"],
        "together" => &["together.ai"],
        "wenxin" => &["baidu.com", "baidubce.com"],
        "hunyuan" => &["tencent.com", "tencentcloudapi.com"],
        _ => &[],
    }
}

pub fn host_matches(host: &str, roots: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    roots.iter().any(|root| {
        let root = root.trim().to_ascii_lowercase();
        !root.is_empty() && (host == root || host.ends_with(&format!(".{root}")))
    })
}

/// Omitted optional fields keep their current values. Verification cannot rename
/// a vendor or replace presentation/credential requirements as a side effect.
pub fn merge_candidate(current: &Value, input: &Value) -> Result<Value, String> {
    let mut merged = current.clone();
    let target = merged.as_object_mut().ok_or("预设必须为对象")?;
    let input = input.as_object().ok_or("预设参数必须为对象")?;
    for key in [
        "providerType",
        "endpoint",
        "defaultModel",
        "mainModels",
        "contextWindow",
        "suggestedMaxTokens",
        "reasoningProfiles",
        "protocols",
        "verifiedSource",
    ] {
        if let Some(value) = input.get(key) {
            target.insert(key.into(), value.clone());
        }
    }
    let models = merged["mainModels"]
        .as_array()
        .ok_or("mainModels 必须为模型 ID 数组")?;
    if models.is_empty()
        || models
            .iter()
            .any(|model| model.as_str().is_none_or(|id| id.trim().is_empty()))
    {
        return Err("核对结果必须保留至少一个已核实的模型 ID".into());
    }
    let default = merged["defaultModel"].as_str().unwrap_or("");
    if !models.iter().any(|model| model.as_str() == Some(default)) {
        return Err("默认模型必须属于已核实的模型列表".into());
    }
    Ok(merged)
}

pub fn model_delta(before: &Value, after: &Value) -> (usize, usize) {
    let models = |row: &Value| {
        row["mainModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
    };
    let before = models(before);
    let after = models(after);
    (
        after.difference(&before).count(),
        before.difference(&after).count(),
    )
}

pub fn content_changed(before: &Value, after: &Value) -> bool {
    let clean = |row: &Value| {
        let mut row = row.clone();
        if let Some(object) = row.as_object_mut() {
            object.remove("verifiedAt");
            object.remove("verifiedSource");
        }
        row
    };
    clean(before) != clean(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn update_scope_rejects_deletion_embedding_and_other_vendors() {
        assert!(check_write("a", "upsert", "llm", "a").is_ok());
        for (action, kind, id) in [
            ("delete", "llm", "a"),
            ("upsert", "embedding", "a"),
            ("upsert", "llm", "b"),
        ] {
            assert!(check_write("a", action, kind, id).is_err());
        }
    }
    #[test]
    fn official_hosts_require_a_domain_boundary() {
        let roots = vec!["openai.com".into()];
        assert!(host_matches("developers.openai.com", &roots));
        assert!(!host_matches("fakeopenai.com", &roots));
        assert!(!host_matches("openai.com.attacker.test", &roots));
    }
    #[test]
    fn partial_updates_preserve_other_fields_and_require_a_real_default() {
        let current = json!({"id":"a","labelKey":"a.label","defaultModel":"old","mainModels":["old"],"needsSecret":true,"reasoningProfiles":[{"source":"retained"}]});
        let next = merge_candidate(
            &current,
            &json!({"mainModels":["new"],"defaultModel":"new","labelKey":"bad","id":"b"}),
        )
        .unwrap();
        assert_eq!(next["id"], "a");
        assert_eq!(next["labelKey"], "a.label");
        assert_eq!(next["reasoningProfiles"], current["reasoningProfiles"]);
        assert_eq!(model_delta(&current, &next), (1, 1));
        assert!(merge_candidate(&current, &json!({"mainModels":[]})).is_err());
        assert!(merge_candidate(&current, &json!({"defaultModel":"missing"})).is_err());
        let mut verified = current.clone();
        verified["verifiedAt"] = json!("2026-10-27");
        assert!(!content_changed(&current, &verified));
    }
    #[test]
    fn target_selection_deduplicates_and_does_not_expand_unknown_ids() {
        let all = vec![UpdateTarget {
            id: "a".into(),
            label: "A".into(),
            label_key: None,
            model_count: 1,
            verified_at: None,
            verified_source: None,
        }];
        assert_eq!(
            select_targets(&all, &["a".into(), "a".into()])
                .unwrap()
                .len(),
            1
        );
        assert!(select_targets(&all, &[]).is_err());
        assert!(select_targets(&all, &["b".into()]).is_err());
    }
    #[test]
    fn interrupted_job_recovery_retains_finished_results() {
        let mut job = UpdateJob {
            id: "job".into(),
            revision: 1,
            running: true,
            stop_requested: false,
            started_at: "now".into(),
            finished_at: None,
            items: vec![
                UpdateItem {
                    id: "a".into(),
                    label: "A".into(),
                    status: ItemStatus::Updated,
                    message: None,
                    session_id: None,
                    source: None,
                    added: 1,
                    removed: 0,
                    model_count: 2,
                },
                UpdateItem {
                    id: "b".into(),
                    label: "B".into(),
                    status: ItemStatus::Running,
                    message: None,
                    session_id: None,
                    source: None,
                    added: 0,
                    removed: 0,
                    model_count: 1,
                },
            ],
        };
        job.recover();
        assert!(!job.running);
        assert_eq!(job.items[0].status, ItemStatus::Updated);
        assert_eq!(job.items[1].status, ItemStatus::Stopped);
    }
}
