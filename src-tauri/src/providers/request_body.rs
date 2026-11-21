//! Full request customization. The reserved envelope keeps legacy parameter patches compatible.
use serde_json::{json, Value};

const LIMIT: usize = 256 * 1024;
const REF: &str = "$requestRef";

pub fn is_full(config: &Value) -> bool {
    config.get("$request").is_some()
}

pub fn validate(
    config: &Value,
    profile: Option<&super::reasoning_profiles::ReasoningProfile>,
) -> Result<(), String> {
    if !is_full(config) {
        return super::reasoning_profiles::validate_adapter_patch(config, profile);
    }
    if serde_json::to_vec(config).map_err(|e| e.to_string())?.len() > LIMIT {
        return Err("请求体配置不能超过 256 KB".into());
    }
    let obj = config.as_object().ok_or("请求配置必须是 JSON 对象")?;
    if obj.len() != 1 {
        return Err("$request 不能与旧版参数覆盖混用".into());
    }
    let request = config["$request"]
        .as_object()
        .ok_or("$request 必须是对象")?;
    if request
        .keys()
        .any(|k| !matches!(k.as_str(), "mode" | "body"))
    {
        return Err("$request 仅支持 mode 和 body".into());
    }
    if !matches!(
        request.get("mode").and_then(Value::as_str),
        Some("merge" | "replace")
    ) {
        return Err("请求体模式必须为 merge 或 replace".into());
    }
    let body = request
        .get("body")
        .filter(|v| v.is_object())
        .ok_or("请求体必须是 JSON 对象")?;
    if body.get(REF).is_some_and(|v| v != "") {
        return Err("请求体根节点仅允许用空 $requestRef 引用完整原始请求体".into());
    }
    validate_refs(body, 0)
}

fn validate_refs(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("请求体模板嵌套不能超过 64 层".into());
    }
    match value {
        Value::Object(obj) => {
            if let Some(reference) = obj.get(REF) {
                if obj.len() != 1 {
                    return Err("$requestRef 必须单独作为一个对象".into());
                }
                let path = reference
                    .as_str()
                    .ok_or("$requestRef 必须是 JSON Pointer 字符串")?;
                if !path.is_empty() && !path.starts_with('/') {
                    return Err("$requestRef 必须为空字符串或以 / 开头".into());
                }
                // Reject malformed JSON Pointer escapes rather than silently dropping fields.
                let bytes = path.as_bytes();
                for (i, b) in bytes.iter().enumerate() {
                    if *b == b'~' && !matches!(bytes.get(i + 1), Some(b'0' | b'1')) {
                        return Err("JSON Pointer 的 ~ 必须转义为 ~0 或 ~1".into());
                    }
                }
            } else {
                for child in obj.values() {
                    validate_refs(child, depth + 1)?;
                }
            }
        }
        Value::Array(arr) => {
            for child in arr {
                validate_refs(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn render(template: &Value, base: &Value) -> Value {
    match template {
        Value::Object(obj) if obj.len() == 1 && obj.contains_key(REF) => base
            .pointer(obj[REF].as_str().unwrap_or(""))
            .cloned()
            .unwrap_or(Value::Null),
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), render(v, base)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| render(v, base)).collect()),
        _ => template.clone(),
    }
}

/// Full customization runs last: explicit JSON wins over form fields and sampling switches.
pub fn apply(body: &mut Value, config: Option<&Value>) {
    let Some(config) = config.filter(|c| is_full(c)) else {
        return;
    };
    let rendered = render(&config["$request"]["body"], body);
    if config["$request"]["mode"] == "replace" {
        *body = rendered;
    } else {
        super::reasoning_profiles::merge_patch(body, &rendered);
    }
}

/// Redact sensitive credentials from example previews.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| {
                    let key = k.to_ascii_lowercase().replace('-', "_");
                    let sensitive = matches!(
                        key.as_str(),
                        "api_key"
                            | "apikey"
                            | "api_secret"
                            | "secret"
                            | "secret_key"
                            | "access_token"
                            | "refresh_token"
                            | "authorization"
                            | "password"
                            | "credential"
                            | "credentials"
                    );
                    (
                        k.clone(),
                        if sensitive {
                            json!("[REDACTED]")
                        } else {
                            redact(v)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(redact).collect()),
        _ => value.clone(),
    }
}
/// Representative protocol body for offline previews.
pub fn example(provider: &str, model: &str, temperature: f64, max_tokens: u32) -> Value {
    let messages = json!([{"role":"system","content":"You are a helpful assistant."},{"role":"user","content":"Hello"}]);
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => {
            json!({"model":model,"system":"You are a helpful assistant.","messages":[{"role":"user","content":"Hello"}],"temperature":temperature,"max_tokens":max_tokens,"stream":true})
        }
        "gemini" | "google" => {
            json!({"systemInstruction":{"parts":[{"text":"You are a helpful assistant."}]},"contents":[{"role":"user","parts":[{"text":"Hello"}]}],"generationConfig":{"temperature":temperature,"maxOutputTokens":max_tokens}})
        }
        "openai" | "openai_compat" | "openai-compat" | "openai_responses" | "openai-responses"
        | "responses_api" | "responses-api" | "doubao" | "doubao_responses"
        | "doubao-responses" | "responses" => {
            json!({"model":model,"input":messages,"temperature":temperature,"max_output_tokens":max_tokens,"stream":true})
        }
        "openai_agents" | "openai-agents" | "agents_api" | "agents-api" => {
            json!({"agent":{"model":model,"instructions":"You are a helpful assistant.","tools":[],"multi_agent":{"enabled":false}},"environment":{"type":"none"},"input":[{"role":"user","content":[{"type":"input_text","text":"Hello"}]}],"stream":true})
        }
        "spark" | "xfyun" | "iflytek" => {
            json!({"header":{"app_id":"EXAMPLE_APP_ID","uid":"vivian"},"parameter":{"chat":{"domain":model,"temperature":temperature,"max_tokens":max_tokens,"audience":"public"}},"payload":{"message":{"text":[{"role":"user","content":"Hello"}]}}})
        }
        "wenxin" | "ernie" | "baidu" => {
            json!({"system":"You are a helpful assistant.","messages":[{"role":"user","content":"Hello"}],"temperature":temperature,"max_output_tokens":max_tokens,"stream":true})
        }
        _ => {
            json!({"model":model,"messages":messages,"temperature":temperature,"max_tokens":max_tokens,"stream":true})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_merge_can_change_messages_model_tools_and_remove_fields() {
        let config = json!({"$request":{"mode":"merge","body":{"model":"custom","messages":[{"role":"user","content":"override"}],"tools":[],"temperature":0.7,"stream":null}}});
        validate(&config, None).unwrap();
        let mut body = json!({"model":"old","messages":[],"stream":true,"max_tokens":10});
        apply(&mut body, Some(&config));
        assert_eq!(body["model"], "custom");
        assert_eq!(body["messages"][0]["content"], "override");
        assert!(body.get("stream").is_none());
        assert_eq!(body["max_tokens"], 10);
        assert_eq!(body["temperature"], 0.7);
    }
    #[test]
    fn replacement_preserves_dynamic_values_and_literal_nulls() {
        let config = json!({"$request":{"mode":"replace","body":{"model":{"$requestRef":"/model"},"input":{"$requestRef":"/messages"},"tools":{"$requestRef":"/tools"},"literal":null,"schema":{"$ref":"#/$defs/foo"}}}});
        validate(&config, None).unwrap();
        let mut body = json!({"model":"live","messages":[{"role":"user","content":"current"}],"temperature":0.3});
        apply(&mut body, Some(&config));
        assert_eq!(body["model"], "live");
        assert_eq!(body["input"][0]["content"], "current");
        assert!(body.get("temperature").is_none());
        assert!(body["tools"].is_null());
        assert!(body.get("literal").unwrap().is_null());
        assert_eq!(body["schema"]["$ref"], "#/$defs/foo");
    }
    #[test]
    fn validates_envelopes_pointers_and_legacy_patches() {
        assert!(validate(&json!({"$request":{"mode":"unknown","body":{}}}), None).is_err());
        assert!(validate(&json!({"$request":{"mode":"replace","body":[]}}), None).is_err());
        assert!(validate(
            &json!({"$request":{"mode":"merge","body":{"x":{"$requestRef":"/bad~2"}}}}),
            None
        )
        .is_err());
        let mut body = json!({"a/b":{"~name":42}});
        let cfg =
            json!({"$request":{"mode":"replace","body":{"x":{"$requestRef":"/a~1b/~0name"}}}});
        validate(&cfg, None).unwrap();
        apply(&mut body, Some(&cfg));
        assert_eq!(body["x"], 42);
        validate(&json!({"temperature":0.5}), None).unwrap();
        assert!(validate(&json!({"messages":[]}), None).is_err());
    }
    #[test]
    fn preview_redacts_credentials_without_removing_messages() {
        let body = json!({"api_key":"secret","messages":[{"content":"hello"}],"nested":{"Authorization":"bearer secret"},"max_tokens":200});

        let result = json!({"body":redact(&body)});
        assert_eq!(result["body"]["api_key"], "[REDACTED]");
        assert_eq!(result["body"]["nested"]["Authorization"], "[REDACTED]");
        assert_eq!(result["body"]["messages"][0]["content"], "hello");
        assert_eq!(result["body"]["max_tokens"], 200);
    }
    #[test]
    fn finalization_keeps_legacy_switches_but_full_customization_wins() {
        use super::super::{
            reasoning::ReasoningPreference, reasoning_profiles::RequestCustomization,
        };
        let base = json!({"model":"test","messages":[],"temperature":0.2,"max_tokens":42});
        let mut adapter = RequestCustomization {
            provider_type: "chat_completions".into(),
            profile: None,
            overrides: Some(json!({"temperature":0.8,"max_tokens":500})),
        };
        let legacy = adapter.finalize(
            base.clone(),
            ReasoningPreference::AUTO,
            "unknown",
            false,
            false,
        );
        assert!(legacy.get("temperature").is_none());
        assert!(legacy.get("max_tokens").is_none());
        adapter.overrides = Some(
            json!({"$request":{"mode":"merge","body":{"temperature":0.8,"max_tokens":500,"extra":{"custom":true}}}}),
        );
        let full = adapter.finalize(
            base.clone(),
            ReasoningPreference::AUTO,
            "unknown",
            false,
            false,
        );
        assert_eq!(full["temperature"], 0.8);
        assert_eq!(full["max_tokens"], 500);
        assert_eq!(full["extra"]["custom"], true);
        assert!(full.get("$request").is_none());
        let request = reqwest::Client::new()
            .post("http://localhost/test")
            .json(&full)
            .build()
            .unwrap();
        let wire: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(wire, full);
    }
    #[test]
    fn agents_customization_targets_full_envelope_and_keeps_legacy_agent_fields() {
        use super::super::{
            reasoning::ReasoningPreference, reasoning_profiles::RequestCustomization,
        };
        let base = example("openai_agents", "test", 0.7, 2048);
        let mut adapter = RequestCustomization {
            provider_type: "openai_agents".into(),
            profile: None,
            overrides: Some(json!({"reasoning":{"effort":"low"}})),
        };
        let legacy = adapter.finalize(
            base.clone(),
            ReasoningPreference::AUTO,
            "unknown",
            true,
            true,
        );
        assert_eq!(legacy["agent"]["reasoning"]["effort"], "low");
        assert!(legacy.get("reasoning").is_none());
        adapter.overrides = Some(
            json!({"$request":{"mode":"merge","body":{"agent":{"model":"new"},"input":[{"role":"user","content":"custom"}]}}}),
        );
        let full = adapter.finalize(base, ReasoningPreference::AUTO, "unknown", true, true);
        assert_eq!(full["agent"]["model"], "new");
        assert_eq!(full["input"][0]["content"], "custom");
    }
    #[test]
    fn root_reference_and_custom_schema_are_not_modified() {
        let mut body = json!({"model":"current","tools":[{"parameters":{"$ref":"#/$defs/x"}}]});
        let original = body.clone();
        let config = json!({"$request":{"mode":"replace","body":{"$requestRef":""}}});
        validate(&config, None).unwrap();
        apply(&mut body, Some(&config));
        assert_eq!(body, original);
        assert!(validate(
            &json!({"$request":{"mode":"replace","body":{"$requestRef":"/tools"}}}),
            None
        )
        .is_err());
    }
}
