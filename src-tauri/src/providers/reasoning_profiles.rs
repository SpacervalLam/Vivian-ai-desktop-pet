//! Plugin-owned reasoning mappings and user overrides. No network calls here.
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use super::reasoning::{ReasoningMode, ReasoningPreference};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningBudget {
    pub path: String,
    pub min: u32,
    pub max: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningProfile {
    pub model_pattern: String,
    pub provider_types: Vec<String>,
    pub source: String,
    pub verified_at: String,
    pub enabled: Value,
    #[serde(default)]
    pub disabled: Option<Value>,
    #[serde(default)]
    pub efforts: BTreeMap<String, Value>,
    #[serde(default)]
    pub budget: Option<ReasoningBudget>,
    /// Only these reasoning paths may be removed/replaced by this profile.
    pub managed_paths: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct RequestCustomization {
    pub profile: Option<ReasoningProfile>,
    pub overrides: Option<Value>,
}

// The escape hatch is deliberately reasoning-only: never overwrite credentials,
// model, messages, tools, or stream framing. null removes a field.
pub fn validate_patch(patch: &Value) -> Result<(), String> {
    let obj = patch.as_object().ok_or("思考参数覆盖必须是 JSON 对象")?;
    if serde_json::to_vec(patch).map_err(|e| e.to_string())?.len() > 16_384 {
        return Err("思考参数覆盖不能超过 16 KB".into());
    }
    for (key, value) in obj {
        match key.as_str() {
            "reasoning" | "reasoning_effort" | "thinking" | "enable_thinking" | "thinking_budget" => {}
            "generationConfig" => validate_nested(value, &["thinkingConfig"] )?,
            "output_config" => validate_nested(value, &["effort"] )?,
            _ => return Err(format!("不允许覆盖 {key}；这里只支持思考控制参数")),
        }
    }
    Ok(())
}
fn validate_nested(value: &Value, keys: &[&str]) -> Result<(), String> {
    // Null at a container would delete unrelated generation settings.
    let obj = value.as_object().ok_or("容器覆盖必须是对象；删除思考字段请将该字段设为 null")?;
    if obj.keys().any(|key| !keys.contains(&key.as_str())) { return Err("包含非思考配置字段".into()); }
    Ok(())
}
pub fn merge_patch(target: &mut Value, patch: &Value) {
    if let Some(obj) = patch.as_object() {
        if !target.is_object() { *target = json!({}); }
        let target = target.as_object_mut().unwrap();
        for (key, value) in obj {
            if value.is_null() { target.remove(key); }
            else { merge_patch(target.entry(key.clone()).or_insert(Value::Null), value); }
        }
    } else { *target = patch.clone(); }
}
fn set_path(target: &mut Value, path: &str, value: Value) {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let mut patch = value;
    for part in parts.iter().rev() { let mut obj = serde_json::Map::new(); obj.insert((*part).to_string(), patch); patch = Value::Object(obj); }
    merge_patch(target, &patch);
}
fn allowed_path(path: &str) -> bool {
    matches!(path, "/reasoning" | "/reasoning_effort" | "/thinking" | "/enable_thinking" | "/thinking_budget" | "/generationConfig/thinkingConfig" | "/output_config/effort")
}
impl ReasoningProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.model_pattern.is_empty() || self.model_pattern.len() > 256 || self.provider_types.is_empty() {
            return Err("思考能力需要模型匹配规则与协议类型".into());
        }
        if !self.source.starts_with("https://") || self.verified_at.is_empty() { return Err("思考能力需要官方来源和核对日期".into()); }
        if self.managed_paths.is_empty() || self.managed_paths.iter().any(|p| !allowed_path(p)) { return Err("思考能力包含不允许的管理路径".into()); }
        validate_patch(&self.enabled)?;
        if let Some(patch) = &self.disabled { validate_patch(patch)?; }
        for patch in self.efforts.values() { validate_patch(patch)?; }
        if let Some(b) = &self.budget {
            if b.min > b.max || !matches!(b.path.as_str(), "/thinking/budget_tokens" | "/generationConfig/thinkingConfig/thinkingBudget" | "/thinking_budget") { return Err("思考预算范围或路径无效".into()); }
        }
        // Anchored wildcard syntax, not user-supplied regular expressions.
        if self.model_pattern.matches('*').count() > 1 || (self.model_pattern.contains('*') && !self.model_pattern.ends_with('*')) { return Err("模型匹配只支持末尾 *".into()); }
        Ok(())
    }
    pub fn matches(&self, provider: &str, model: &str) -> bool {
        if !self.provider_types.iter().any(|p| p == provider) { return false; }
        let pattern = self.model_pattern.to_ascii_lowercase(); let model = model.to_ascii_lowercase();
        match pattern.strip_suffix('*') { Some(prefix) => model.starts_with(prefix), None => model == pattern }
    }
    pub fn apply(&self, body: &mut Value, pref: ReasoningPreference) {
        for path in &self.managed_paths { set_path(body, path, Value::Null); }
        match pref.mode {
            ReasoningMode::Auto => {} // Explicitly follow the server default.
            ReasoningMode::Off => if let Some(patch) = &self.disabled { merge_patch(body, patch); },
            ReasoningMode::On => {
                merge_patch(body, &self.enabled);
                if let Some(effort) = pref.effort {
                    if let Some(patch) = self.efforts.get(effort.as_str()) { merge_patch(body, patch); }
                }
                if let (Some(budget), Some(tokens)) = (&self.budget, pref.budget_tokens) {
                    set_path(body, &budget.path, json!(tokens.clamp(budget.min, budget.max)));
                }
            }
        }
    }
}
pub fn resolve(provider: &str, model: &str) -> Option<ReasoningProfile> {
    crate::plugins::load_provider_presets().into_iter().flat_map(|p| p.reasoning_profiles)
        .find(|p| p.validate().is_ok() && p.matches(provider, model))
}
impl RequestCustomization {
    pub fn apply(&self, body: &mut Value, pref: ReasoningPreference) {
        if let Some(profile) = &self.profile { profile.apply(body, pref); }
        if let Some(overrides) = &self.overrides { merge_patch(body, overrides); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_and_explicit_off_preserve_unrelated_fields() {
        let profile: ReasoningProfile = serde_json::from_value(json!({
            "modelPattern":"test-*","providerTypes":["gemini"],"source":"https://example.test/docs","verifiedAt":"2026-10-02",
            "enabled":{"generationConfig":{"thinkingConfig":{"thinkingLevel":"medium"}}},
            "disabled":{"generationConfig":{"thinkingConfig":{"thinkingBudget":0}}},
            "efforts":{"low":{"generationConfig":{"thinkingConfig":{"thinkingLevel":"low"}}}},
            "managedPaths":["/generationConfig/thinkingConfig"]
        })).unwrap();
        profile.validate().unwrap();
        assert!(profile.matches("gemini","test-new")); assert!(!profile.matches("openai","test-new"));
        let original = json!({"contents":[1],"generationConfig":{"temperature":1,"thinkingConfig":{"thinkingBudget":99}}});
        let mut body = original.clone(); profile.apply(&mut body, ReasoningPreference::AUTO);
        assert!(body["generationConfig"].get("thinkingConfig").is_none()); assert_eq!(body["contents"],json!([1]));
        profile.apply(&mut body, ReasoningPreference {mode:ReasoningMode::Off,effort:None,budget_tokens:None});
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"],0);
        let c = RequestCustomization {profile:Some(profile),overrides:Some(json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"high"}}}))};
        c.apply(&mut body, ReasoningPreference::on(Some(super::super::reasoning::ReasoningEffort::Low)));
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingLevel"],"high");
        assert!(body["generationConfig"]["thinkingConfig"].get("thinkingBudget").is_none());
        assert_eq!(body["generationConfig"]["temperature"],1);
        for invalid in [json!({"model":"evil"}),json!({"generationConfig":{"temperature":2}}),json!({"generationConfig":null}),json!([])] { assert!(validate_patch(&invalid).is_err()); }
    }
    #[test]
    fn budget_clamps_and_unknown_models_keep_manual_overrides() {
        let profile: ReasoningProfile = serde_json::from_value(json!({
            "modelPattern":"budget-*","providerTypes":["anthropic"],"source":"https://example.test/docs","verifiedAt":"2026-10-02",
            "enabled":{"thinking":{"type":"enabled"}},"disabled":{"thinking":{"type":"disabled"}},
            "budget":{"path":"/thinking/budget_tokens","min":1024,"max":4096},"managedPaths":["/thinking"]
        })).unwrap();
        profile.validate().unwrap();
        let mut body=json!({"max_tokens":8000,"messages":[{"role":"user","content":"hello"}]});
        profile.apply(&mut body,ReasoningPreference{mode:ReasoningMode::On,effort:None,budget_tokens:Some(20)});
        assert_eq!(body["thinking"]["budget_tokens"],1024);
        profile.apply(&mut body,ReasoningPreference{mode:ReasoningMode::On,effort:None,budget_tokens:Some(9000)});
        assert_eq!(body["thinking"]["budget_tokens"],4096);
        RequestCustomization{profile:None,overrides:Some(json!({"enable_thinking":false}))}.apply(&mut body,ReasoningPreference::AUTO);
        assert_eq!(body["enable_thinking"],false); assert_eq!(body["max_tokens"],8000);
    }
    #[test]
    fn builtin_profiles_are_valid_and_protocol_specific() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../plugins/llm-providers/providers.json")).unwrap();
        let profiles:Vec<ReasoningProfile> = rows.iter().flat_map(|row|row["reasoningProfiles"].as_array().into_iter().flatten()).map(|v|serde_json::from_value(v.clone()).unwrap()).collect();
        for profile in &profiles {profile.validate().unwrap();}
        for (protocol,path) in [("openai","/reasoning/effort"),("chat_completions","/reasoning_effort"),("anthropic","/output_config/effort")] {
            let p=profiles.iter().find(|p|p.matches(protocol,"deepseek-flash")).unwrap();
            let mut body=json!({});p.apply(&mut body,ReasoningPreference::on(Some(super::super::reasoning::ReasoningEffort::Low)));
            assert_eq!(body.pointer(path),Some(&json!("low")));
        }
        let p=profiles.iter().find(|p|p.matches("gemini","gemini-3.8-flash")).unwrap();assert!(p.disabled.is_none());
    }

}
