//! 供应商预设更新工具 - 智能体在运行时修正 LLM 供应商预设数据
//!
//! 调用场景（核对技能 verify-provider-presets 的结构化落点）：
//! - 联网核对某供应商官方 API 文档后，把过期/错误的预设写回插件数据
//!   （端点迁移、模型退役、上下文窗口更新、协议变体增减）
//! - 新增供应商预设（新 id 即追加，设置 → LLM 页厂商卡片即时可见）
//!
//! 设计要点（相对手工编辑 providers.json 的优势）：
//! - **原子与校验**：id/provider_type 入口校验 + serde 结构化写入，
//!   不可能写出解析失败的数据（手工编辑 JSON 拼错会静默退回内置兜底）
//! - **时间可信**：verifiedAt 由本地系统时间写入，不采用模型给出的日期
//! - **版本自护**：自动递增插件 version，修改不会被下次启动播种覆盖
//! - 生效方式：写的是按需读盘的插件数据文件，重开设置窗口即生效，无需重启

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::plugins::{
    EmbeddingModelPresetData, EmbeddingProviderPresetData, ProviderPresetData, ProviderProtocolData,
};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};


/// 从工具入参解析一条预设行（camelCase 对齐 ProviderPresetData）。
fn parse_preset(input: &Value) -> Result<ProviderPresetData, String> {
    let id = input
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err("id 不能为空".into());
    }
    let protocols = input
        .get("protocols")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| ProviderProtocolData {
                    provider_type: p
                        .get("providerType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    label_key: p
                        .get("labelKey")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    label: p.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    endpoint: p
                        .get("endpoint")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect::<Vec<_>>()
        });

    Ok(ProviderPresetData {
        reasoning_profiles: input.get("reasoningProfiles").map(|v| serde_json::from_value(v.clone())).transpose().map_err(|e| format!("思考能力格式无效: {e}"))?.unwrap_or_default(),
        id,
        label_key: input
            .get("labelKey")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        label: input.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()),
        provider_type: input
            .get("providerType")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        endpoint: input
            .get("endpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        default_model: input
            .get("defaultModel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        main_models: input
            .get("mainModels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        context_window: input.get("contextWindow").and_then(|v| v.as_u64()).map(|v| v as u32),
        suggested_max_tokens: input
            .get("suggestedMaxTokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        needs_secret: input.get("needsSecret").and_then(|v| v.as_bool()),
        needs_app_id: input.get("needsAppId").and_then(|v| v.as_bool()),
        console_url: input
            .get("consoleUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        protocols,
        // 两个元数据字段不接收模型输入：verified_at 由后端系统时间写入，
        // verified_source 经独立参数 verifiedSource 传入（官方文档入口）
        verified_at: None,
        verified_source: None,
    })
}


/// 读取当前实际生效的 LLM 与嵌入预设，供智能体核对前先盘点完整行。
pub struct ListProviderPresetsTool;

impl ListProviderPresetsTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for ListProviderPresetsTool {
    fn name(&self) -> &str { "list_provider_presets" }
    fn description(&self) -> &str {
        "List the currently effective LLM and cloud embedding provider presets contributed by trusted plugins. Use this before updating or deleting a preset so you preserve complete rows and stable ids. Never returns API keys."
    }
    fn description_in(&self, lang: &str) -> &str {
        if lang == "zh" { "列出受信任插件当前实际生效的 LLM 与云端嵌入供应商预设。更新或删除前先调用，以保留完整字段并确认稳定 id；结果不包含 API Key。" } else { self.description() }
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"kind":{"type":"string","enum":["all","llm","embedding"],"description":"Preset kind; default all"}},"additionalProperties":false})
    }
    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("kind").and_then(Value::as_str).unwrap_or("all") {
            "all" | "llm" | "embedding" => ValidationResult::success(None),
            _ => ValidationResult::failure("kind 仅支持 all、llm 或 embedding", 2),
        }
    }
    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }
    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        crate::plugins::ensure_builtin_plugins();
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("all");
        let llm = if kind == "all" || kind == "llm" { crate::plugins::load_provider_presets() } else { Vec::new() };
        let embedding = if kind == "all" || kind == "embedding" { crate::plugins::load_embedding_provider_presets() } else { Vec::new() };
        ToolResult::standard_success(
            &format!("当前有 {} 条 LLM 预设、{} 条嵌入预设", llm.len(), embedding.len()),
            Some(json!({"llm": llm, "embedding": embedding})),
        )
    }
    fn is_read_only(&self) -> bool { true }
    fn category(&self) -> ToolCategory { ToolCategory::System }
    fn should_defer(&self) -> bool { true }
    fn search_hint(&self) -> &str { "list provider presets 供应商预设 列出 盘点 embedding llm" }
}

/// 预设变更动作解析（旧 `update_provider_preset` 没有 action / kind）。
///
/// 旧更新工具的参数形状（id + providerType/endpoint/mainModels…）→ upsert/llm；
/// 嵌入列的 model/dimension/provider 形状 → upsert/embedding。显式 action、kind 优先。
pub(crate) fn effective_provider_preset_action(args: &Value) -> String {
    if let Some(action) = args
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|action| !action.is_empty())
    {
        return action.to_ascii_lowercase();
    }
    let llm_shape = [
        "providerType",
        "defaultModel",
        "mainModels",
        "protocols",
        "reasoningProfiles",
    ]
    .iter()
    .any(|key| args.get(*key).is_some());
    let embedding_shape = ["model", "dimension", "provider"]
        .iter()
        .any(|key| args.get(*key).is_some());
    if llm_shape || embedding_shape || args.get("endpoint").is_some() {
        "upsert".to_string()
    } else {
        String::new()
    }
}

pub(crate) fn effective_provider_preset_kind(args: &Value) -> String {
    if let Some(kind) = args
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|kind| matches!(*kind, "llm" | "embedding"))
    {
        return kind.to_string();
    }
    let embedding_shape = ["model", "dimension", "provider"]
        .iter()
        .any(|key| args.get(*key).is_some());
    if embedding_shape {
        "embedding".to_string()
    } else {
        "llm".to_string()
    }
}

/// 云端嵌入预设的结构化 upsert，以及两类 Provider 预设的安全删除入口。
pub struct ManageProviderPresetTool;

impl ManageProviderPresetTool {
    pub fn new() -> Self { Self }
}

/// 把工具入参解析成**厂商级**嵌入预设（`models` 只含本次要写的那一个模型）。
///
/// 工具语义是「新增/更新某厂商的某个嵌入模型」：厂商行已存在时由
/// `upsert_embedding_model_preset` 合并，不会覆盖同厂商的其他模型。
fn parse_embedding_preset(input: &Value) -> Result<EmbeddingProviderPresetData, String> {
    let get = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let model = get("model");
    let dimension = input.get("dimension").and_then(Value::as_u64).unwrap_or(0) as usize;
    if model.is_empty() || dimension == 0 {
        return Err("embedding upsert 需要完整的 model/dimension".into());
    }
    let preset = EmbeddingProviderPresetData {
        id: get("id"),
        provider: get("provider"),
        endpoint: get("endpoint"),
        models: vec![EmbeddingModelPresetData {
            model,
            dimension,
            max_batch: input
                .get("maxBatch")
                .and_then(Value::as_u64)
                .map(|v| v as usize),
            dimensions: input.get("dimensions").and_then(Value::as_array).map(|rows| {
                rows.iter()
                    .filter_map(Value::as_u64)
                    .map(|v| v as usize)
                    .collect()
            }),
            note: input.get("note").and_then(Value::as_str).map(str::to_string),
        }],
        region: input.get("region").and_then(Value::as_str).map(str::to_string),
        console_url: input.get("consoleUrl").and_then(Value::as_str).map(str::to_string),
        dimension_param: input
            .get("dimensionParam")
            .and_then(Value::as_str)
            .map(str::to_string),
        needs_api_key: input
            .get("needsApiKey")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        recommended_for: input.get("recommendedFor").and_then(Value::as_str).map(str::to_string),
        verified_at: None,
        verified_source: input.get("verifiedSource").and_then(Value::as_str).map(str::to_string),
    };
    if preset.id.is_empty() || preset.provider.is_empty() || preset.endpoint.is_empty() {
        return Err("embedding upsert 需要完整的 id/provider/endpoint/model/dimension".into());
    }
    Ok(preset)
}

#[async_trait]
impl Tool for ManageProviderPresetTool {
    fn name(&self) -> &str { "manage_provider_preset" }
    fn description(&self) -> &str {
        "Manage provider preset data in the built-in llm-providers plugin: upsert one complete LLM or \
         cloud embedding preset (a new id appends a new provider), or delete either kind by stable id. \
         Verify current official docs first and call list_provider_presets before replacing/deleting. \
         For LLM upsert the row REPLACES the existing preset with the same id entirely, so always pass the \
         COMPLETE row including every field you want to keep; the id is indexed by the credential cache and \
         must NOT change. verifiedAt is stamped by the system clock automatically (never trust your own \
         memory for today's date); pass verifiedSource with the official doc URL you relied on. \
         Changes take effect after reopening the settings window, no restart needed. \
         This changes preset metadata only, never API keys or the active runtime selection."
    }
    fn description_in(&self, lang: &str) -> &str {
        if lang == "zh" {
            "管理内置 llm-providers 插件中的供应商预设：可完整新增/更新 LLM 或云端嵌入预设（新 id 即新增厂商），也可按稳定 id 删除。\
             操作前必须核对当前官方文档并先调用 list_provider_presets。LLM 预设按 id 整行替换——必须传完整行（含所有要保留的字段，漏传即丢失）；\
             id 是凭据缓存索引的稳定标识，不可变更。verifiedAt 由系统时钟自动写入（不要凭记忆报日期）；verifiedSource 传核对依据的官方文档 URL。\
             改完重开设置窗口生效，无需重启。只修改候选预设，不读取或修改 API Key，也不会擅自切换当前运行配置。"
        } else {
            self.description()
        }
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["upsert","delete"]},
                "kind":{"type":"string","enum":["llm","embedding"]},
                "id":{"type":"string","description":"Stable preset id (e.g. deepseek); indexed by the credential cache, never rename an existing one"},
                "labelKey":{"type":"string","description":"i18n key for the display name (reuse existing config.preset_* keys)"},
                "label":{"type":"string","description":"Direct display name (providers without an i18n key; takes precedence over labelKey)"},
                "providerType":{"type":"string","description":"Default protocol: openai (Responses) / chat_completions / anthropic / gemini / zhipu / wenxin / doubao"},
                "endpoint":{"type":"string","description":"Base URL of the default protocol, exactly as in the official docs"},
                "defaultModel":{"type":"string","description":"Recommended flagship/default model ID; must belong to mainModels"},
                "mainModels":{"type":"array","items":{"type":"string"},"description":"Currently valid model IDs from the official list (remove retired ones)"},
                "contextWindow":{"type":"integer","description":"Context window in tokens (input + output combined)"}, "suggestedMaxTokens":{"type":"integer","description":"Suggested per-request output cap; be conservative (exceeding it throws a 400)"},
                "reasoningProfiles":{"type":"array","items":{"type":"object"},"description":"Verified model/protocol reasoning mappings; preserve unchanged profiles when updating."},
                "needsSecret":{"type":"boolean","description":"Whether an api_secret is required (OAuth/HMAC providers like wenxin)"},
                "needsAppId":{"type":"boolean","description":"Whether an app_id is required"},
                "consoleUrl":{"type":"string","description":"API key console / getting-started page URL"},
                "protocols":{"type":"array","items":{"type":"object","properties":{"providerType":{"type":"string"},"labelKey":{"type":"string"},"label":{"type":"string"},"endpoint":{"type":"string"}},"required":["providerType","endpoint"]},"description":"Protocol variants (>=2 shows the protocol switcher)"},
                "provider":{"type":"string","description":"Embedding vendor display name"}, "model":{"type":"string","description":"Embedding model ID"},
                "dimension":{"type":"integer","minimum":1,"description":"Embedding vector dimension"}, "recommendedFor":{"type":"string"},
                "region":{"type":"string","description":"地域说明，如 中国大陆 / 国际"},
                "dimensionParam":{"type":"string","enum":["dimensions","output_dimension"],"description":"请求体里下发维度的参数名；服务端维度固定或兼容层不支持时留空"},
                "maxBatch":{"type":"integer","minimum":1,"description":"单请求 input 数组条数上限"},
                "dimensions":{"type":"array","items":{"type":"integer"},"description":"可切换的维度取值"},
                "needsApiKey":{"type":"boolean","description":"本地服务可填任意占位 Key 时传 false"},
                "note":{"type":"string"},
                "verifiedSource":{"type":"string","description":"Official documentation URL used for verification"}
            },
            "required":["action","kind","id"],
            "additionalProperties":false
        })
    }
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let action = effective_provider_preset_action(input);
        let kind = effective_provider_preset_kind(input);
        let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if let Err(error) = crate::commands::provider_presets::guard_write(ctx, &action, &kind, id) {
            return ValidationResult::failure(&error, 2);
        }
        if !matches!(action.as_str(), "upsert" | "delete") || id.is_empty() {
            return ValidationResult::failure("action/kind/id 不合法", 2);
        }
        if action == "upsert" {
            let result = if kind == "llm" {
                parse_preset(input).map(|_| ())
            } else {
                parse_embedding_preset(input).map(|_| ())
            };
            if let Err(e) = result { return ValidationResult::failure(&e, 2); }
        }
        ValidationResult::success(None)
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        if effective_provider_preset_action(input) == "delete" {
            let kind = effective_provider_preset_kind(input);
            let id = input.get("id").and_then(Value::as_str).unwrap_or("");
            PermissionResult::ask(&format!("将删除 {kind} 供应商预设「{id}」（不会删除 API Key），是否继续？"))
        } else {
            PermissionResult::allow()
        }
    }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let action = effective_provider_preset_action(&args);
        let kind = effective_provider_preset_kind(&args);
        let id = args.get("id").and_then(Value::as_str).unwrap_or("");
        if let Err(error) = crate::commands::provider_presets::guard_write(ctx, &action, &kind, id) {
            return ToolResult::standard_error(&error,None,None);
        }
        if action == "delete" {
            return match crate::plugins::remove_provider_preset(&kind, id) {
                Ok((id, version)) => ToolResult::standard_success(
                    &format!("{kind} 供应商预设 {id} 已删除（插件版本 {version}）"),
                    Some(json!({"action":"delete","kind":kind,"id":id,"version":version})),
                ),
                Err(e) => ToolResult::standard_error("删除供应商预设失败", Some(&e), None),
            };
        }
        if kind == "llm" {
            let verified_source = args.get("verifiedSource").and_then(Value::as_str).map(str::to_string);
            let result = crate::commands::provider_presets::upsert_scoped(ctx,&args).and_then(|scoped|match scoped {
                Some(result) => Ok(result),
                None => parse_preset(&args).and_then(|mut p| { p.verified_source = verified_source; crate::plugins::upsert_provider_preset(p) }),
            });
            match result {
                Ok((row, version, is_new)) => ToolResult::standard_success(
                    &format!("LLM 预设 {} 已{}（插件版本 {version}）", row.id, if is_new {"新增"} else {"更新"}),
                    Some(json!({"action":"upsert","kind":"llm","id":row.id,"isNew":is_new,"version":version})),
                ),
                Err(e) => ToolResult::standard_error("更新 LLM 供应商预设失败", Some(&e), None),
            }
        } else {
            match parse_embedding_preset(&args).and_then(crate::plugins::upsert_embedding_model_preset) {
                Ok((row, version, is_new)) => ToolResult::standard_success(
                    &format!(
                        "嵌入预设 {}/{} 已{}（插件版本 {version}）",
                        row.id,
                        row.models.first().map(|m| m.model.as_str()).unwrap_or(""),
                        if is_new { "新增" } else { "更新" }
                    ),
                    Some(json!({"action":"upsert","kind":"embedding","id":row.id,"isNew":is_new,"version":version})),
                ),
                Err(e) => ToolResult::standard_error("更新嵌入供应商预设失败", Some(&e), None),
            }
        }
    }
    fn is_read_only(&self) -> bool { false }
    fn category(&self) -> ToolCategory { ToolCategory::System }
    fn should_defer(&self) -> bool { true }
    fn risk(&self) -> ToolRiskTier { ToolRiskTier::FsWrite }
    fn search_hint(&self) -> &str { "manage update provider preset 供应商 预设 厂商 嵌入 新增 更新 删除 verify 核对" }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Passing a partial LLM row (missing fields you meant to keep) — the row is replaced entirely",
            "Renaming an existing preset id (credential cache indexes by id)",
            "Guessing today's date for verification metadata (system clock stamps it)",
            "Using it before actually checking the official docs (verification-first workflow)",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_embedding_preset() {
        let p = parse_embedding_preset(&json!({
            "id": "vendor",
            "provider": "Vendor",
            "endpoint": "https://api.example.com/v1",
            "model": "embed-v2",
            "dimension": 1024,
            "maxBatch": 32,
            "dimensions": [512, 1024],
            "dimensionParam": "dimensions",
            "region": "国际",
            "recommendedFor": "multilingual",
            "verifiedSource": "https://docs.example.com/embeddings"
        }))
        .expect("完整嵌入预设应可解析");
        // 工具只写一个模型，厂商行由 upsert_embedding_model_preset 合并
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].model, "embed-v2");
        assert_eq!(p.models[0].dimension, 1024);
        assert_eq!(p.models[0].max_batch, Some(32));
        assert_eq!(p.models[0].dimensions, Some(vec![512, 1024]));
        assert_eq!(p.dimension_param.as_deref(), Some("dimensions"));
        assert_eq!(p.region.as_deref(), Some("国际"));
        assert!(p.verified_at.is_none());
    }

    #[test]
    fn legacy_update_arguments_infer_upsert_and_llm() {
        // 旧 update_provider_preset 形状：没有 action / kind，只有整行 LLM 字段
        let legacy_llm = json!({
            "id": "deepseek",
            "providerType": "openai",
            "endpoint": "https://api.deepseek.com/v1",
            "mainModels": ["deepseek-chat"]
        });
        assert_eq!(effective_provider_preset_action(&legacy_llm), "upsert");
        assert_eq!(effective_provider_preset_kind(&legacy_llm), "llm");

        // 嵌入列形状：model / dimension / provider
        let embedding = json!({
            "id": "vendor",
            "provider": "Vendor",
            "model": "embed-v2",
            "dimension": 1024
        });
        assert_eq!(effective_provider_preset_action(&embedding), "upsert");
        assert_eq!(effective_provider_preset_kind(&embedding), "embedding");

        // 显式 action / kind 优先
        assert_eq!(
            effective_provider_preset_action(&json!({"action": "delete", "kind": "llm", "id": "x"})),
            "delete"
        );
        assert_eq!(effective_provider_preset_kind(&json!({"kind": "llm", "id": "x"})), "llm");
        // 看不出形状时不留动作，交给 validate_input 报错
        assert_eq!(effective_provider_preset_action(&json!({})), "");
    }

    #[test]
    fn rejects_partial_embedding_preset() {
        assert!(parse_embedding_preset(&json!({
            "id": "vendor-model",
            "provider": "Vendor",
            "endpoint": "https://api.example.com/v1"
        }))
        .is_err());
        // 只有 model 没有 dimension 同样不合格
        assert!(parse_embedding_preset(&json!({
            "id": "vendor-model",
            "provider": "Vendor",
            "endpoint": "https://api.example.com/v1",
            "model": "embed-v2"
        }))
        .is_err());
    }
}
