//! Plugin-owned reasoning mappings and user overrides. No network calls here.
use super::reasoning::{ReasoningMode, ReasoningPreference};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
    #[serde(default)]
    pub sampling: Option<SamplingProfile>,
}
#[derive(Debug, Clone, Default)]
pub struct RequestCustomization {
    pub profile: Option<ReasoningProfile>,
    pub overrides: Option<Value>,
    pub provider_type: String,
}

/// Wire field placement belongs to the protocol adapter, never model-name UI logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingProfile {
    pub temperature_path: Option<String>,
    pub max_tokens_path: Option<String>,
}
const TEMPERATURE_PATHS: &[&str] = &[
    "/temperature",
    "/generationConfig/temperature",
    "/parameter/chat/temperature",
];
const TOKEN_PATHS: &[&str] = &[
    "/max_tokens",
    "/max_completion_tokens",
    "/max_output_tokens",
    "/generationConfig/maxOutputTokens",
    "/parameter/chat/max_tokens",
];
impl SamplingProfile {
    pub fn protocol(provider: &str) -> Self {
        let (temperature, tokens) = match provider {
            "gemini" => (
                Some("/generationConfig/temperature"),
                Some("/generationConfig/maxOutputTokens"),
            ),
            "spark" => (
                Some("/parameter/chat/temperature"),
                Some("/parameter/chat/max_tokens"),
            ),
            "openai_agents" => (None, None),
            "openai" | "openai_responses" => (Some("/temperature"), Some("/max_output_tokens")),
            _ => (Some("/temperature"), Some("/max_tokens")),
        };
        Self {
            temperature_path: temperature.map(str::to_owned),
            max_tokens_path: tokens.map(str::to_owned),
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self
            .temperature_path
            .as_deref()
            .is_some_and(|p| !safe_parameter_path(p))
            || self
                .max_tokens_path
                .as_deref()
                .is_some_and(|p| !safe_parameter_path(p))
        {
            return Err("采样字段路径无效或指向受保护载荷".into());
        }
        Ok(())
    }
    pub fn apply(&self, body: &mut Value) {
        for (paths, destination) in [
            (TEMPERATURE_PATHS, &self.temperature_path),
            (TOKEN_PATHS, &self.max_tokens_path),
        ] {
            let value = destination
                .as_ref()
                .and_then(|p| body.pointer(p))
                .cloned()
                .or_else(|| paths.iter().find_map(|p| body.pointer(p).cloned()));
            for path in paths {
                set_path(body, path, Value::Null);
            }
            if let (Some(path), Some(value)) = (destination, value) {
                set_path(body, path, value);
            }
        }
    }
}

/// Declared adapter fields may vary; transport and conversation payloads stay protected.
fn safe_parameter_path(path: &str) -> bool {
    if !path.starts_with('/') || path.len() > 256 {
        return false;
    }
    let parts: Vec<_> = path[1..].split('/').collect();
    if parts.len() > 8 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !matches!(
                part.to_ascii_lowercase().as_str(),
                "model"
                    | "messages"
                    | "input"
                    | "contents"
                    | "tools"
                    | "tool_choice"
                    | "stream"
                    | "instructions"
                    | "system"
                    | "headers"
                    | "authorization"
                    | "api_key"
                    | "apikey"
            )
    })
}

pub fn validate_adapter_patch(
    patch: &Value,
    profile: Option<&ReasoningProfile>,
) -> Result<(), String> {
    if !patch.is_object() || serde_json::to_vec(patch).map_err(|e| e.to_string())?.len() > 16_384 {
        return Err("请求参数覆盖必须是 16 KB 以内的 JSON 对象".into());
    }
    if validate_patch(patch).is_ok() {
        return Ok(());
    }
    let Some(profile) = profile else {
        return validate_patch(patch);
    };
    let mut remaining = patch.clone();
    for path in
        profile
            .managed_paths
            .iter()
            .map(String::as_str)
            .chain(profile.sampling.iter().flat_map(|s| {
                [s.temperature_path.as_deref(), s.max_tokens_path.as_deref()]
                    .into_iter()
                    .flatten()
            }))
    {
        if !safe_parameter_path(path) {
            return Err("适配字段路径不安全".into());
        }
        set_path(&mut remaining, path, Value::Null);
    }
    fn prune(value: &mut Value) {
        if let Some(obj) = value.as_object_mut() {
            for child in obj.values_mut() {
                prune(child);
            }
            obj.retain(|_, value| !value.as_object().is_some_and(|v| v.is_empty()));
        }
    }
    prune(&mut remaining);
    validate_patch(&remaining)
}

pub fn apply_sampling_switches(
    body: &mut Value,
    temperature: bool,
    tokens: bool,
    sampling: &SamplingProfile,
) {
    if !temperature {
        for path in TEMPERATURE_PATHS
            .iter()
            .copied()
            .chain(sampling.temperature_path.as_deref())
        {
            set_path(body, path, Value::Null);
        }
    }
    if !tokens {
        for path in TOKEN_PATHS
            .iter()
            .copied()
            .chain(sampling.max_tokens_path.as_deref())
        {
            set_path(body, path, Value::Null);
        }
    }
}

// Request customization never overwrites credentials,
// model, messages, tools, or stream framing. null removes a field.
pub fn validate_patch(patch: &Value) -> Result<(), String> {
    let obj = patch.as_object().ok_or("思考参数覆盖必须是 JSON 对象")?;
    if serde_json::to_vec(patch).map_err(|e| e.to_string())?.len() > 16_384 {
        return Err("思考参数覆盖不能超过 16 KB".into());
    }
    for (key, value) in obj {
        match key.as_str() {
            "reasoning"
            | "reasoning_effort"
            | "thinking"
            | "enable_thinking"
            | "thinking_budget"
            | "temperature"
            | "max_tokens"
            | "max_completion_tokens"
            | "max_output_tokens"
            | "top_p"
            | "presence_penalty"
            | "frequency_penalty" => {}
            "generationConfig" => validate_nested(
                value,
                &[
                    "thinkingConfig",
                    "temperature",
                    "maxOutputTokens",
                    "topP",
                    "presencePenalty",
                    "frequencyPenalty",
                ],
            )?,
            "parameter" => {
                validate_nested(value, &["chat"])?;
                validate_nested(&value["chat"], &["temperature", "max_tokens"])?;
            }
            "output_config" => validate_nested(value, &["effort"])?,
            _ => return Err(format!("不允许覆盖 {key}；这里只支持思考与采样请求参数")),
        }
    }
    Ok(())
}
fn validate_nested(value: &Value, keys: &[&str]) -> Result<(), String> {
    // Null at a container would delete unrelated generation settings.
    let obj = value
        .as_object()
        .ok_or("容器覆盖必须是对象；删除思考字段请将该字段设为 null")?;
    if obj.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err("包含非请求参数字段".into());
    }
    Ok(())
}
pub fn merge_patch(target: &mut Value, patch: &Value) {
    if let Some(obj) = patch.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        let target = target.as_object_mut().unwrap();
        for (key, value) in obj {
            if value.is_null() {
                target.remove(key);
            } else {
                merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    } else {
        *target = patch.clone();
    }
}
fn set_path(target: &mut Value, path: &str, value: Value) {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if value.is_null() {
        let mut parent = target;
        for part in &parts[..parts.len() - 1] {
            let Some(next) = parent.get_mut(*part) else {
                return;
            };
            parent = next;
        }
        if let Some(obj) = parent.as_object_mut() {
            obj.remove(parts[parts.len() - 1]);
        }
        return;
    }
    let mut patch = value;
    for part in parts.iter().rev() {
        let mut obj = serde_json::Map::new();
        obj.insert((*part).to_string(), patch);
        patch = Value::Object(obj);
    }
    merge_patch(target, &patch);
}
fn allowed_path(path: &str) -> bool {
    matches!(
        path,
        "/reasoning"
            | "/reasoning_effort"
            | "/thinking"
            | "/enable_thinking"
            | "/thinking_budget"
            | "/generationConfig/thinkingConfig"
            | "/output_config/effort"
    )
}
impl ReasoningProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.model_pattern.is_empty()
            || self.model_pattern.len() > 256
            || self.provider_types.is_empty()
        {
            return Err("思考能力需要模型匹配规则与协议类型".into());
        }
        if !self.source.starts_with("https://") || self.verified_at.is_empty() {
            return Err("思考能力需要官方来源和核对日期".into());
        }
        if self.managed_paths.is_empty()
            || self
                .managed_paths
                .iter()
                .any(|p| !allowed_path(p) && !safe_parameter_path(p))
        {
            return Err("思考能力包含不允许的管理路径".into());
        }
        if let Some(sampling) = &self.sampling {
            sampling.validate()?;
        }
        validate_adapter_patch(&self.enabled, Some(self))?;
        if let Some(patch) = &self.disabled {
            validate_adapter_patch(patch, Some(self))?;
        }
        for patch in self.efforts.values() {
            validate_adapter_patch(patch, Some(self))?;
        }
        if let Some(b) = &self.budget {
            if b.min > b.max
                || !matches!(
                    b.path.as_str(),
                    "/thinking/budget_tokens"
                        | "/generationConfig/thinkingConfig/thinkingBudget"
                        | "/thinking_budget"
                )
            {
                return Err("思考预算范围或路径无效".into());
            }
        }
        // Anchored wildcard syntax, not user-supplied regular expressions.
        if self.model_pattern.matches('*').count() > 1
            || (self.model_pattern.contains('*') && !self.model_pattern.ends_with('*'))
        {
            return Err("模型匹配只支持末尾 *".into());
        }
        Ok(())
    }
    pub fn matches(&self, provider: &str, model: &str) -> bool {
        if !self.provider_types.iter().any(|p| p == provider) {
            return false;
        }
        let pattern = self.model_pattern.to_ascii_lowercase();
        let model = model.to_ascii_lowercase();
        match pattern.strip_suffix('*') {
            Some(prefix) => model.starts_with(prefix),
            None => model == pattern,
        }
    }
    pub fn apply(&self, body: &mut Value, pref: ReasoningPreference) {
        for path in &self.managed_paths {
            set_path(body, path, Value::Null);
        }
        match pref.mode {
            ReasoningMode::Auto => {} // Explicitly follow the server default.
            ReasoningMode::Off => {
                if let Some(patch) = &self.disabled {
                    merge_patch(body, patch);
                }
            }
            ReasoningMode::On => {
                merge_patch(body, &self.enabled);
                if let Some(effort) = pref.effort {
                    if let Some(patch) = self.efforts.get(effort.as_str()) {
                        merge_patch(body, patch);
                    }
                }
                if let (Some(budget), Some(tokens)) = (&self.budget, pref.budget_tokens) {
                    set_path(
                        body,
                        &budget.path,
                        json!(tokens.clamp(budget.min, budget.max)),
                    );
                }
            }
        }
    }
}
pub fn resolve(provider: &str, model: &str) -> Option<ReasoningProfile> {
    crate::plugins::load_provider_presets()
        .into_iter()
        .flat_map(|p| p.reasoning_profiles)
        .find(|p| p.validate().is_ok() && p.matches(provider, model))
}
impl RequestCustomization {
    pub fn sampling(&self) -> SamplingProfile {
        self.profile
            .as_ref()
            .and_then(|p| p.sampling.clone())
            .unwrap_or_else(|| SamplingProfile::protocol(&self.provider_type))
    }
    pub fn apply(&self, body: &mut Value, pref: ReasoningPreference, model: &str) {
        if let Some(profile) = &self.profile {
            profile.apply(body, pref);
        } else {
            apply_legacy_adapter(body, pref, &self.provider_type, model);
        }
        if matches!(
            self.provider_type.as_str(),
            "openai"
                | "openai_responses"
                | "openai_agents"
                | "chat_completions"
                | "anthropic"
                | "gemini"
                | "spark"
                | "doubao"
                | "zhipu"
        ) || self.profile.as_ref().is_some_and(|p| p.sampling.is_some())
        {
            self.sampling().apply(body);
        }
        if let Some(overrides) = &self.overrides {
            merge_patch(body, overrides);
        }
    }
}

/// Compatibility metadata lives here until a verified plugin profile replaces it.
/// Protocol implementations do not resolve model families themselves.
pub fn apply_legacy_adapter(
    body: &mut Value,
    pref: ReasoningPreference,
    protocol: &str,
    model: &str,
) {
    use super::reasoning::{self, ReasoningEffort, ReasoningMode};
    if protocol == "gemini" {
        let config = if !matches!(pref.mode, ReasoningMode::Auto) {
            Some(match pref.mode {
                ReasoningMode::Off if !model.starts_with("gemini-3.8") => {
                    json!({"thinkingBudget":0})
                }
                ReasoningMode::Off => json!({"thinkingLevel":"low"}),
                _ => json!({"thinkingLevel":match pref.effort.unwrap_or(ReasoningEffort::Medium) {
                ReasoningEffort::Minimal | ReasoningEffort::Low => "low", ReasoningEffort::Medium => "medium", _ => "high" }}),
            })
        } else {
            None
        };
        if let Some(config) = config {
            set_path(body, "/generationConfig/thinkingConfig", config);
        }
        return;
    }
    let cap = reasoning::resolve_reasoning_capability(model);
    match protocol {
        "openai" | "openai_responses" | "openai_agents" => {
            reasoning::apply_responses_reasoning(body, pref, &cap)
        }
        "chat_completions" | "anthropic" | "doubao" | "zhipu" => {
            let has_tools = body
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty());
            reasoning::apply_reasoning_preference(body, pref, &cap, has_tools);
        }
        _ => {}
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
        assert!(profile.matches("gemini", "test-new"));
        assert!(!profile.matches("openai", "test-new"));
        let original = json!({"contents":[1],"generationConfig":{"temperature":1,"thinkingConfig":{"thinkingBudget":99}}});
        let mut body = original.clone();
        profile.apply(&mut body, ReasoningPreference::AUTO);
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
        assert_eq!(body["contents"], json!([1]));
        profile.apply(
            &mut body,
            ReasoningPreference {
                mode: ReasoningMode::Off,
                effort: None,
                budget_tokens: None,
            },
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        let c = RequestCustomization {
            provider_type: "gemini".into(),
            profile: Some(profile),
            overrides: Some(
                json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"high"}}}),
            ),
        };
        c.apply(
            &mut body,
            ReasoningPreference::on(Some(super::super::reasoning::ReasoningEffort::Low)),
            "test-new",
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "high"
        );
        assert!(body["generationConfig"]["thinkingConfig"]
            .get("thinkingBudget")
            .is_none());
        assert_eq!(body["generationConfig"]["temperature"], 1);
        for invalid in [
            json!({"model":"evil"}),
            json!({"generationConfig":{"contents":[]}}),
            json!({"generationConfig":null}),
            json!([]),
        ] {
            assert!(validate_patch(&invalid).is_err());
        }
    }
    #[test]
    fn budget_clamps_and_unknown_models_keep_manual_overrides() {
        let profile: ReasoningProfile = serde_json::from_value(json!({
            "modelPattern":"budget-*","providerTypes":["anthropic"],"source":"https://example.test/docs","verifiedAt":"2026-10-02",
            "enabled":{"thinking":{"type":"enabled"}},"disabled":{"thinking":{"type":"disabled"}},
            "budget":{"path":"/thinking/budget_tokens","min":1024,"max":4096},"managedPaths":["/thinking"]
        })).unwrap();
        profile.validate().unwrap();
        let mut body = json!({"max_tokens":8000,"messages":[{"role":"user","content":"hello"}]});
        profile.apply(
            &mut body,
            ReasoningPreference {
                mode: ReasoningMode::On,
                effort: None,
                budget_tokens: Some(20),
            },
        );
        assert_eq!(body["thinking"]["budget_tokens"], 1024);
        profile.apply(
            &mut body,
            ReasoningPreference {
                mode: ReasoningMode::On,
                effort: None,
                budget_tokens: Some(9000),
            },
        );
        assert_eq!(body["thinking"]["budget_tokens"], 4096);
        RequestCustomization {
            provider_type: "anthropic".into(),
            profile: None,
            overrides: Some(json!({"enable_thinking":false})),
        }
        .apply(&mut body, ReasoningPreference::AUTO, "unknown");
        assert_eq!(body["enable_thinking"], false);
        assert_eq!(body["max_tokens"], 8000);
    }
    #[test]
    fn sampling_paths_are_declared_and_user_patch_wins_without_touching_payload() {
        let mut body =
            json!({"messages":[{"content":"temperature"}], "temperature":0.7, "max_tokens":4000});
        let adapter = RequestCustomization {
            provider_type: "gemini".into(),
            profile: None,
            overrides: Some(json!({"generationConfig":{"temperature":1.2}})),
        };
        adapter.apply(&mut body, ReasoningPreference::AUTO, "unknown");
        assert_eq!(
            body.pointer("/generationConfig/temperature"),
            Some(&json!(1.2))
        );
        assert_eq!(
            body.pointer("/generationConfig/maxOutputTokens"),
            Some(&json!(4000))
        );
        assert!(body.get("temperature").is_none());
        assert!(body.get("parameter").is_none());
        assert_eq!(body["messages"][0]["content"], "temperature");
        validate_patch(&json!({"temperature":0.5,"max_output_tokens":6000})).unwrap();
        assert!(validate_patch(&json!({"generationConfig":{"responseSchema":{}}})).is_err());
        let mut empty = json!({});
        SamplingProfile::protocol("gemini").apply(&mut empty);
        assert_eq!(empty, json!({}));
    }

    #[test]
    fn declared_custom_fields_support_user_overrides_and_send_switches() {
        let profile: ReasoningProfile = serde_json::from_value(json!({
            "modelPattern":"future-*","providerTypes":["future_protocol"],"source":"https://example.test/docs","verifiedAt":"2026-10-03",
            "enabled":{"ponder":{"mode":"enabled"}},"disabled":{"ponder":{"mode":"disabled"}},
            "efforts":{"high":{"ponder":{"strength":3}}},"managedPaths":["/ponder"],
            "sampling":{"temperaturePath":"/options/heat","maxTokensPath":"/options/output_limit"}
        })).unwrap();
        profile.validate().unwrap();
        let patch = json!({"options":{"heat":0.9}, "ponder":{"strength":5}});
        validate_adapter_patch(&patch, Some(&profile)).unwrap();
        assert!(validate_adapter_patch(&json!({"model":"other"}), Some(&profile)).is_err());
        assert!(validate_adapter_patch(
            &json!({"ponder":{"data":"x".repeat(17000)}}),
            Some(&profile)
        )
        .is_err());
        let adapter = RequestCustomization {
            provider_type: "future_protocol".into(),
            profile: Some(profile),
            overrides: Some(patch),
        };
        let mut body = json!({"messages":["real input"],"temperature":0.7,"max_tokens":4000,
            "tools":[{"parameters":{"properties":{"temperature":{"type":"number"}}}}]});
        adapter.apply(
            &mut body,
            ReasoningPreference::on(Some(super::super::reasoning::ReasoningEffort::High)),
            "future-model",
        );
        assert_eq!(body.pointer("/options/heat"), Some(&json!(0.9)));
        assert_eq!(body.pointer("/options/output_limit"), Some(&json!(4000)));
        assert_eq!(body.pointer("/ponder/strength"), Some(&json!(5)));
        apply_sampling_switches(&mut body, false, false, &adapter.sampling());
        assert!(body.pointer("/options/heat").is_none());
        assert!(body.pointer("/options/output_limit").is_none());
        assert_eq!(body["messages"], json!(["real input"]));
        assert_eq!(
            body["tools"][0]["parameters"]["properties"]["temperature"]["type"],
            "number"
        );
        assert!(!safe_parameter_path("/messages/temperature"));
        assert!(!safe_parameter_path("/api_key"));
        let mut custom = json!({"generationConfig":{"temperature":1,"maxOutputTokens":99}});
        RequestCustomization {
            provider_type: "unregistered_custom".into(),
            ..Default::default()
        }
        .apply(&mut custom, ReasoningPreference::AUTO, "unknown");
        assert_eq!(
            custom,
            json!({"generationConfig":{"temperature":1,"maxOutputTokens":99}})
        );
    }

    #[test]
    fn builtin_profiles_are_valid_and_protocol_specific() {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../plugins/llm-providers/providers.json"))
                .unwrap();
        let profiles: Vec<ReasoningProfile> = rows
            .iter()
            .flat_map(|row| row["reasoningProfiles"].as_array().into_iter().flatten())
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect();
        for profile in &profiles {
            profile.validate().unwrap();
        }
        for (protocol, path) in [
            ("openai", "/reasoning/effort"),
            ("chat_completions", "/reasoning_effort"),
            ("anthropic", "/output_config/effort"),
        ] {
            let p = profiles
                .iter()
                .find(|p| p.matches(protocol, "deepseek-flash"))
                .unwrap();
            let mut body = json!({});
            p.apply(
                &mut body,
                ReasoningPreference::on(Some(super::super::reasoning::ReasoningEffort::Low)),
            );
            assert_eq!(body.pointer(path), Some(&json!("low")));
        }
        let p = profiles
            .iter()
            .find(|p| p.matches("gemini", "gemini-3.8-flash"))
            .unwrap();
        assert!(p.disabled.is_none());
    }
}
