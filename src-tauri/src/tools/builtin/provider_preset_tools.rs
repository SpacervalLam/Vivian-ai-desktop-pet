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

use crate::plugins::{EmbeddingProviderPresetData, ProviderPresetData, ProviderProtocolData};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 供应商预设更新工具（按 id 整行 upsert 到 llm-providers 插件数据）。
pub struct UpdateProviderPresetTool;

impl UpdateProviderPresetTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UpdateProviderPresetTool {
    fn default() -> Self {
        Self::new()
    }
}

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

#[async_trait]
impl Tool for UpdateProviderPresetTool {
    fn name(&self) -> &str {
        "update_provider_preset"
    }

    fn description(&self) -> &str {
        "Update or add one LLM provider preset in the llm-providers plugin data file (the data source of the provider cards in Settings -> LLM page). Use it after verifying a provider's official API docs: fix stale endpoints, retired model IDs, context window sizes, protocol variants, or add a brand-new provider. Semantics: the row REPLACES the existing preset with the same id entirely, so always pass the COMPLETE row including every field you want to keep. The id is a stable identifier indexed by the credential cache and must NOT be changed; a new id appends a new preset. verifiedAt is stamped by the system clock automatically (never trust your own memory for today's date); pass verifiedSource with the official doc URL you relied on. Changes take effect after reopening the settings window, no restart needed."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "更新或新增一条 LLM 供应商预设（写入 llm-providers 插件数据文件，即设置 → LLM 页厂商卡片的数据源）。联网核对某供应商官方 API 文档后用它落修正：过期端点、已退役模型 ID、上下文窗口、协议变体，或全新供应商。语义是按 id 整行替换——必须传完整行（含所有要保留的字段，漏传即丢失）。id 是凭据缓存索引的稳定标识，不可变更；新 id 即新增预设。verifiedAt 由系统时钟自动写入（不要自己凭记忆报日期）；verifiedSource 传本次核对依据的官方文档 URL。改完重开设置窗口生效，无需重启。",
            "ja" => "LLM プロバイダのプリセットを 1 件更新または新規追加する（llm-providers プラグインのデータファイル、設定 → LLM ページのプロバイダカードのデータソース）。公式 API ドキュメントを照合した後の修正に使う：期限切れのエンドポイント、廃止されたモデル ID、コンテキストウィンドウ、プロトコル変体、または新規プロバイダ。意味は id による行全体の置換——保持したい全フィールドを含む完全な行を渡すこと（省略したフィールドは失われる）。id は認証情報キャッシュのキーとなる安定識別子で変更不可。新 id は新規追加。verifiedAt はシステム時計で自動記録される（自分の記憶で日付を書かない）。verifiedSource に照合根拠の公式ドキュメント URL を渡す。設定ウィンドウを開き直せば反映、再起動不要。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "更新供应商预设\n修正预设\n把核对结果写进预设\n新增一个厂商预设",
            "en" => "update the provider preset\nfix the preset data\nsave the verification result",
            "ja" => "プロバイダのプリセットを更新\nプリセットを修正",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Stable preset identifier (e.g. deepseek); indexed by the credential cache, never rename an existing one" },
                "labelKey": { "type": "string", "description": "i18n key for display name (reuse existing config.preset_* keys)" },
                "label": { "type": "string", "description": "Direct display name (providers without an i18n key; takes precedence over labelKey)" },
                "providerType": { "type": "string", "description": "Default protocol: openai (Responses) / chat_completions / anthropic / gemini / zhipu / wenxin / doubao" },
                "endpoint": { "type": "string", "description": "Base URL of the default protocol, exactly as in the official docs" },
                "defaultModel": { "type": "string", "description": "Recommended flagship/default model ID" },
                "mainModels": { "type": "array", "items": { "type": "string" }, "description": "Currently valid model IDs from the official model list (remove retired ones)" },
                "contextWindow": { "type": "integer", "description": "Context window in tokens (input + output combined)" },
                "suggestedMaxTokens": { "type": "integer", "description": "Suggested per-request output cap; be conservative (a 400 is thrown when exceeded)" },
                "needsSecret": { "type": "boolean", "description": "Whether an api_secret is required (OAuth/HMAC providers like wenxin)" },
                "needsAppId": { "type": "boolean", "description": "Whether an app_id is required" },
                "consoleUrl": { "type": "string", "description": "API key console / getting-started page URL" },
                "protocols": {
                    "type": "array",
                    "description": "Protocol variants (>=2 shows the protocol switcher). Each: providerType + endpoint (+ optional labelKey/label)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "providerType": { "type": "string" },
                            "labelKey": { "type": "string" },
                            "label": { "type": "string" },
                            "endpoint": { "type": "string" }
                        },
                        "required": ["providerType", "endpoint"]
                    }
                },
                "verifiedSource": { "type": "string", "description": "Official doc URL this update was verified against (for direct revisiting next time)" }
            },
            "required": ["id", "providerType", "endpoint"],
            "additionalProperties": false
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "预设稳定标识（如 deepseek）；凭据缓存按它索引，已有 id 不可改名" },
                    "labelKey": { "type": "string", "description": "显示名 i18n key（复用现有 config.preset_* 键）" },
                    "label": { "type": "string", "description": "直接显示名（无 i18n 键的厂商用，优先于 labelKey）" },
                    "providerType": { "type": "string", "description": "默认协议：openai（Responses）/ chat_completions / anthropic / gemini / zhipu / wenxin / doubao" },
                    "endpoint": { "type": "string", "description": "默认协议的 base_url，以官方文档原文为准" },
                    "defaultModel": { "type": "string", "description": "推荐的旗舰/默认模型 ID" },
                    "mainModels": { "type": "array", "items": { "type": "string" }, "description": "官方模型列表中当前有效的模型 ID（退役的要移除）" },
                    "contextWindow": { "type": "integer", "description": "上下文窗口（tokens，输入+输出合计）" },
                    "suggestedMaxTokens": { "type": "integer", "description": "建议单次输出上限，保守取值（超限会被 400 拒绝）" },
                    "needsSecret": { "type": "boolean", "description": "是否需要 api_secret（文心等 OAuth/HMAC 厂商）" },
                    "needsAppId": { "type": "boolean", "description": "是否需要 app_id" },
                    "consoleUrl": { "type": "string", "description": "API Key 控制台/获取页 URL" },
                    "protocols": {
                        "type": "array",
                        "description": "协议变体列表（≥2 个显示协议切换器）。每项：providerType + endpoint（可选 labelKey/label）",
                        "items": {
                            "type": "object",
                            "properties": {
                                "providerType": { "type": "string" },
                                "labelKey": { "type": "string" },
                                "label": { "type": "string" },
                                "endpoint": { "type": "string" }
                            },
                            "required": ["providerType", "endpoint"]
                        }
                    },
                    "verifiedSource": { "type": "string", "description": "本次核对依据的官方文档 URL（下次核对直接回访）" }
                },
                "required": ["id", "providerType", "endpoint"],
                "additionalProperties": false
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "プリセットの安定識別子（例 deepseek）；認証キャッシュのキー、既存 id の改名不可" },
                    "providerType": { "type": "string", "description": "デフォルトプロトコル：openai / chat_completions / anthropic / gemini / zhipu / wenxin / doubao" },
                    "endpoint": { "type": "string", "description": "デフォルトプロトコルの base_url、公式ドキュメントの原文どおり" },
                    "defaultModel": { "type": "string", "description": "推奨の主力/デフォルトモデル ID" },
                    "mainModels": { "type": "array", "items": { "type": "string" }, "description": "公式モデル一覧の現行有効なモデル ID（廃止分は除去）" },
                    "contextWindow": { "type": "integer", "description": "コンテキストウィンドウ（tokens、入力+出力合計）" },
                    "suggestedMaxTokens": { "type": "integer", "description": "推奨出力上限、控えめに（超過は 400 拒否）" },
                    "consoleUrl": { "type": "string", "description": "API キーコンソール URL" },
                    "protocols": {
                        "type": "array",
                        "description": "プロトコル変体（2 個以上で切替表示）。各項：providerType + endpoint",
                        "items": {
                            "type": "object",
                            "properties": {
                                "providerType": { "type": "string" },
                                "endpoint": { "type": "string" }
                            },
                            "required": ["providerType", "endpoint"]
                        }
                    },
                    "verifiedSource": { "type": "string", "description": "照合根拠の公式ドキュメント URL" }
                },
                "required": ["id", "providerType", "endpoint"],
                "additionalProperties": false
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        // 结构校验交给 parse_preset（与执行路径同源，避免两套规则漂移）
        match parse_preset(input) {
            Ok(_) => ValidationResult::success(None),
            Err(e) => ValidationResult::failure(e, 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // 写用户数据目录的插件数据文件；risk=FsWrite 由权限矩阵按访问级别决定 allow/ask/deny
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let verified_source = args
            .get("verifiedSource")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let preset = match parse_preset(&args) {
            Ok(mut p) => {
                p.verified_source = verified_source;
                p
            }
            Err(e) => return ToolResult::standard_error("预设参数不合法", Some(&e), None),
        };

        match crate::plugins::upsert_provider_preset(preset) {
            Ok((row, version, is_new)) => {
                let summary = format!(
                    "供应商预设 {} 已{}（插件版本 {version}，核对日期 {}）；重开设置窗口生效",
                    row.id,
                    if is_new { "新增" } else { "更新" },
                    row.verified_at.as_deref().unwrap_or("-")
                );
                ToolResult::standard_success(
                    &summary,
                    Some(json!({
                        "id": row.id,
                        "isNew": is_new,
                        "version": version,
                        "verifiedAt": row.verified_at,
                        "verifiedSource": row.verified_source,
                        "endpoint": row.endpoint,
                        "defaultModel": row.default_model,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error("更新供应商预设失败", Some(&e), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        // 长尾工具：经 tool_search 唤起（核对技能文档会引导精确加载）
        true
    }

    fn risk(&self) -> ToolRiskTier {
        // 写用户数据目录文件（预设数据不含密钥，密钥在 provider_cache，影响面为默认填充）
        ToolRiskTier::FsWrite
    }

    fn search_hint(&self) -> &str {
        "provider preset 供应商 预设 厂商 verify 核对 更新预设"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Passing a partial row (missing fields you meant to keep) — the row is replaced entirely",
            "Renaming an existing preset id (credential cache indexes by id)",
            "Guessing today's date for verification metadata (system clock stamps it)",
            "Using it before actually checking the official docs (verification-first workflow)",
        ]
    }
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

/// 云端嵌入预设的结构化 upsert，以及两类 Provider 预设的安全删除入口。
pub struct ManageProviderPresetTool;

impl ManageProviderPresetTool {
    pub fn new() -> Self { Self }
}

fn parse_embedding_preset(input: &Value) -> Result<EmbeddingProviderPresetData, String> {
    let get = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let preset = EmbeddingProviderPresetData {
        id: get("id"),
        provider: get("provider"),
        endpoint: get("endpoint"),
        model: get("model"),
        dimension: input.get("dimension").and_then(Value::as_u64).unwrap_or(0) as usize,
        recommended_for: input.get("recommendedFor").and_then(Value::as_str).map(str::to_string),
        verified_at: None,
        verified_source: input.get("verifiedSource").and_then(Value::as_str).map(str::to_string),
    };
    if preset.id.is_empty() || preset.provider.is_empty() || preset.endpoint.is_empty()
        || preset.model.is_empty() || preset.dimension == 0
    {
        return Err("embedding upsert 需要完整的 id/provider/endpoint/model/dimension".into());
    }
    Ok(preset)
}

#[async_trait]
impl Tool for ManageProviderPresetTool {
    fn name(&self) -> &str { "manage_provider_preset" }
    fn description(&self) -> &str {
        "Manage provider preset data in the built-in llm-providers plugin. Actions: upsert a complete LLM or cloud embedding preset, or delete either kind by stable id. Verify current official docs first and call list_provider_presets before replacing/deleting. This changes preset metadata only, never API keys or the active runtime selection."
    }
    fn description_in(&self, lang: &str) -> &str {
        if lang == "zh" { "管理内置 llm-providers 插件中的供应商预设：可完整新增/更新 LLM 或云端嵌入预设，也可按稳定 id 删除。操作前必须核对当前官方文档并先调用 list_provider_presets；只修改候选预设，不读取或修改 API Key，也不会擅自切换当前运行配置。" } else { self.description() }
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["upsert","delete"]},
                "kind":{"type":"string","enum":["llm","embedding"]},
                "id":{"type":"string","description":"Stable preset id"},
                "labelKey":{"type":"string"}, "label":{"type":"string"},
                "providerType":{"type":"string"}, "endpoint":{"type":"string"},
                "defaultModel":{"type":"string"}, "mainModels":{"type":"array","items":{"type":"string"}},
                "contextWindow":{"type":"integer"}, "suggestedMaxTokens":{"type":"integer"},
                "needsSecret":{"type":"boolean"}, "needsAppId":{"type":"boolean"},
                "consoleUrl":{"type":"string"}, "protocols":{"type":"array","items":{"type":"object"}},
                "provider":{"type":"string"}, "model":{"type":"string"},
                "dimension":{"type":"integer","minimum":1}, "recommendedFor":{"type":"string"},
                "verifiedSource":{"type":"string","description":"Official documentation URL used for verification"}
            },
            "required":["action","kind","id"],
            "additionalProperties":false
        })
    }
    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let kind = input.get("kind").and_then(Value::as_str).unwrap_or("");
        let id = input.get("id").and_then(Value::as_str).unwrap_or("").trim();
        if !matches!(action, "upsert" | "delete") || !matches!(kind, "llm" | "embedding") || id.is_empty() {
            return ValidationResult::failure("action/kind/id 不合法", 2);
        }
        if action == "upsert" {
            let result = if kind == "llm" { parse_preset(input).map(|_| ()) } else { parse_embedding_preset(input).map(|_| ()) };
            if let Err(e) = result { return ValidationResult::failure(&e, 2); }
        }
        ValidationResult::success(None)
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        if input.get("action").and_then(Value::as_str) == Some("delete") {
            let kind = input.get("kind").and_then(Value::as_str).unwrap_or("provider");
            let id = input.get("id").and_then(Value::as_str).unwrap_or("");
            PermissionResult::ask(&format!("将删除 {kind} 供应商预设「{id}」（不会删除 API Key），是否继续？"))
        } else {
            PermissionResult::allow()
        }
    }
    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
        let id = args.get("id").and_then(Value::as_str).unwrap_or("");
        if action == "delete" {
            return match crate::plugins::remove_provider_preset(kind, id) {
                Ok((id, version)) => ToolResult::standard_success(
                    &format!("{kind} 供应商预设 {id} 已删除（插件版本 {version}）"),
                    Some(json!({"action":"delete","kind":kind,"id":id,"version":version})),
                ),
                Err(e) => ToolResult::standard_error("删除供应商预设失败", Some(&e), None),
            };
        }
        if kind == "llm" {
            let verified_source = args.get("verifiedSource").and_then(Value::as_str).map(str::to_string);
            match parse_preset(&args).and_then(|mut p| { p.verified_source = verified_source; crate::plugins::upsert_provider_preset(p) }) {
                Ok((row, version, is_new)) => ToolResult::standard_success(
                    &format!("LLM 预设 {} 已{}（插件版本 {version}）", row.id, if is_new {"新增"} else {"更新"}),
                    Some(json!({"action":"upsert","kind":"llm","id":row.id,"isNew":is_new,"version":version})),
                ),
                Err(e) => ToolResult::standard_error("更新 LLM 供应商预设失败", Some(&e), None),
            }
        } else {
            match parse_embedding_preset(&args).and_then(crate::plugins::upsert_embedding_provider_preset) {
                Ok((row, version, is_new)) => ToolResult::standard_success(
                    &format!("嵌入预设 {} 已{}（插件版本 {version}）", row.id, if is_new {"新增"} else {"更新"}),
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
    fn search_hint(&self) -> &str { "manage provider preset update add delete 供应商 预设 嵌入 更新 新增 删除" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_embedding_preset() {
        let p = parse_embedding_preset(&json!({
            "id": "vendor-model",
            "provider": "Vendor",
            "endpoint": "https://api.example.com/v1",
            "model": "embed-v2",
            "dimension": 1024,
            "recommendedFor": "multilingual",
            "verifiedSource": "https://docs.example.com/embeddings"
        }))
        .expect("完整嵌入预设应可解析");
        assert_eq!(p.dimension, 1024);
        assert_eq!(p.model, "embed-v2");
        assert!(p.verified_at.is_none());
    }

    #[test]
    fn rejects_partial_embedding_preset() {
        assert!(parse_embedding_preset(&json!({
            "id": "vendor-model",
            "provider": "Vendor",
            "endpoint": "https://api.example.com/v1"
        }))
        .is_err());
    }
}
