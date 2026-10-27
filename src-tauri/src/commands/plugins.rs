//! 插件 / 技能清单查询与运行时装卸命令
//!
//! 查询类供设置窗口「插件」页盘点展示；装卸类（reload/unload/delete/trust）是
//! 运行时插件生命周期的界面入口——`create_plugin` 元工具落盘后走的也是
//! 同一套 `plugins::load_one` 装载逻辑（经授权通道自动写信任）。
//! 装卸与信任命令在**后端校验调用窗口**（仅设置窗口可发起），前端确认
//! 只是体验层，不是安全边界。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// 单个技能条目（含来源、形态与作用域徽章所需信息）
#[derive(Debug, Clone, Serialize)]
pub struct SkillEntryInfo {
    pub name: String,
    pub description: String,
    /// 作用域：`None` = 全局（所有角色可见）；`Some(char_id)` = 仅该角色
    pub scope: Option<String>,
    /// 来源：`builtin`（内置）/ `user`（用户技能目录）/ `plugin`（插件贡献）
    pub origin: String,
    /// 磁盘形态：`folder`（`<名称>/SKILL.md`）/ `flat`（单文件）/ `inline`（内置或插件，无磁盘目录）
    pub kind: String,
    /// 技能来源路径；内置技能为 None
    pub dir: Option<String>,
    /// 技能目录内的支撑资源相对路径
    pub resources: Vec<String>,
    /// 是否可在设置页编辑（仅用户技能为 true）
    pub editable: bool,
    /// 正文长度（字符数，供概览判断体量）
    pub body_len: usize,
}

/// 技能支撑资源条目（详情页展示）。
#[derive(Debug, Clone, Serialize)]
pub struct SkillResourceInfo {
    pub path: String,
    pub size: u64,
}

/// 技能详情（设置页编辑器数据源）。
#[derive(Debug, Clone, Serialize)]
pub struct SkillDetailInfo {
    pub name: String,
    pub description: String,
    pub origin: String,
    pub kind: String,
    pub dir: Option<String>,
    pub body: String,
    pub keywords: Vec<String>,
    pub resources: Vec<SkillResourceInfo>,
    pub editable: bool,
}

/// 设置页提交的资源写入项。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillResourceInput {
    pub path: String,
    pub content: String,
}

/// 单个资源文件上限（与 create_skill 保持一致）。
const SKILL_RESOURCE_MAX_BYTES: usize = 256 * 1024;

/// 内置名单（区分技能来源，同时是 create_skill / save_skill 的防覆盖名单）。
use crate::skills::{BUILTIN_SKILL_NAMES, BUILTIN_STYLE_SKILLS};

/// 技能来源：`plugin`（命名空间 `插件/技能`）/ `builtin`（内置）/ `user`（其余）。
pub(crate) fn skill_origin(name: &str) -> &'static str {
    if name.contains('/') {
        "plugin"
    } else if BUILTIN_SKILL_NAMES.contains(&name) {
        "builtin"
    } else {
        "user"
    }
}

/// 只有用户目录技能可被设置页编辑：内置与插件技能一律拒绝。
pub(crate) fn ensure_user_skill(name: &str) -> Result<(), String> {
    if name.contains('/') {
        return Err("插件技能由插件管理，不能在此编辑或删除".into());
    }
    if BUILTIN_SKILL_NAMES.contains(&name) {
        return Err(format!("「{name}」是内置技能，不可编辑或删除"));
    }
    Ok(())
}

/// 技能磁盘形态。
fn skill_kind(skill: &crate::skills::Skill) -> &'static str {
    match skill.path.as_deref() {
        Some(path) if std::path::Path::new(path).is_dir() => "folder",
        Some(_) => "flat",
        None => "inline",
    }
}

/// 组装技能详情（含资源清单与文件大小）。
fn detail_of(skill: crate::skills::Skill) -> SkillDetailInfo {
    let origin = skill_origin(&skill.name).to_string();
    let dir = skill.path.clone();
    let resources = dir
        .as_deref()
        .filter(|path| std::path::Path::new(path).is_dir())
        .map(|path| {
            let base = std::path::Path::new(path);
            crate::skills::collect_skill_resources(base)
                .into_iter()
                .map(|relative| {
                    let size = std::fs::metadata(base.join(&relative))
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                    SkillResourceInfo { path: relative, size }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    SkillDetailInfo {
        editable: origin == "user",
        name: skill.name.clone(),
        description: skill.description.clone(),
        kind: skill_kind(&skill).to_string(),
        origin,
        dir,
        body: skill.body.clone(),
        keywords: skill.keywords.clone(),
        resources,
    }
}

/// 插件清单（盘点插件目录，只读不装载；附带信任状态供设置页展示）。
#[tauri::command]
pub async fn list_plugins(state: State<'_, Arc<AppState>>) -> Result<Vec<crate::plugins::PluginInventoryEntry>, String> {
    let mut entries = crate::plugins::scan_inventory();
    let servers = state.mcp_manager.list_servers().await;
    for entry in &mut entries {
        if entry.status == "loaded" && entry.mcp_servers.iter().any(|id| !servers.iter().any(|s|s.id == *id && s.alive)) {
            entry.status = "partial".into();
            entry.reason = Some(entry.mcp_servers.iter().filter_map(|id| servers.iter().find(|s|s.id == *id).and_then(|s|s.last_error.clone())).collect::<Vec<_>>().join("；"));
        }
    }
    if crate::commands::apartment::installed() {
        entries.push(crate::plugins::PluginInventoryEntry {
            enabled: state.config.read().get_all().base.apartment_enabled,
            requirements: Vec::new(),
            key: "3d-apartment".into(),
            name: "3d-apartment".into(),
            version: "1.1.0".into(),
            description: "3D 公寓与街区探索。可在设置 → 通用中启用或禁用。".into(),
            skills: Vec::new(),
            capability_kind: "executable".into(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            providers: Vec::new(),
            embeddings: Vec::new(),
            status: if state.config.read().get_all().base.apartment_enabled { "loaded" } else { "disabled" }.into(),
            trust: "trusted".into(),
            reason: None,
            dir: crate::commands::apartment::plugin_dir()
                .map(|path| path.display().to_string()).unwrap_or_default(),
        });
    }
    Ok(entries)
}

/// 敏感插件管理命令的调用边界：只允许设置窗口（label=config）发起。
///
/// 前端 `window.confirm` 不是安全控制——任何拿到 IPC 能力的 WebView 都能直接
/// invoke。重载/卸载/删除/信任在这里做后端窗口校验，WebView 侧越权调用直接拒绝。
fn ensure_config_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "config" {
        Ok(())
    } else {
        Err(format!(
            "插件管理命令只允许设置窗口调用（当前窗口: {}）",
            window.label()
        ))
    }
}

#[tauri::command]
pub async fn set_plugin_enabled(window: tauri::WebviewWindow, state: State<'_, Arc<AppState>>, key: String, enabled: bool) -> Result<(), String> {
    ensure_config_window(&window)?;
    crate::plugins::set_enabled(&state.skill_service, &state.mcp_manager, &state.tool_system, &key, enabled).await
}

#[tauri::command]
pub async fn inspect_plugin_package(window: tauri::WebviewWindow, source: String) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    tokio::task::spawn_blocking(move ||crate::plugins::package::inspect(std::path::Path::new(&source))).await.map_err(|e|e.to_string())?
}

#[tauri::command]
pub async fn install_plugin_package(window: tauri::WebviewWindow, state: State<'_, Arc<AppState>>, source: String, fingerprint: String) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    crate::plugins::package::install(source.into(), fingerprint, &state.skill_service, &state.mcp_manager, &state.tool_system).await
}

/// 信任插件：记录当前清单指纹并立即装载（设置页信任按钮的后端入口）。
///
/// 信任是把「目录里的文件」升级为「可执行贡献」的唯一用户授权面——
/// 信任后清单再变更即回到未信任，须重新确认。
#[tauri::command]
pub async fn trust_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    key: String,
) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    let report = crate::plugins::load_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &key,
        true,
    )
    .await?;
    Ok(serde_json::json!({
        "plugin": key,
        "skills": report.skills,
        "tools": report.tools,
        "mcp_servers": report.mcp_servers,
        "skipped": report.skipped,
    }))
}

/// 重载单个插件：撤销旧贡献并按磁盘当前内容重新装载（含连接 MCP server）。
/// 手工编辑插件目录后用它即时生效，无需重启。
///
/// 仅受信且清单未变更的插件可重载；清单变更后须先重新信任。
#[tauri::command]
pub async fn reload_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    let report = crate::plugins::load_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
        false,
    )
    .await?;
    Ok(serde_json::json!({
        "plugin": name,
        "skills": report.skills,
        "tools": report.tools,
        "mcp_servers": report.mcp_servers,
        "skipped": report.skipped,
    }))
}

/// 卸载单个插件的运行时贡献（不动磁盘文件；重启后按目录重新装载）。
#[tauri::command]
pub async fn unload_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    ensure_config_window(&window)?;
    crate::plugins::unload_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
    )
    .await
}

/// 删除插件：撤销运行时贡献并移除目录（内置插件禁删）。
#[tauri::command]
pub async fn delete_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    ensure_config_window(&window)?;
    crate::plugins::delete_plugin(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
    )
    .await
}

/// 插件贡献的 LLM 供应商预设（设置 → LLM 页厂商卡片数据源）。
///
/// 先确保内置 `llm-providers` 插件已播种（不存在或版本落后时写入），再从磁盘
/// 读取全部插件贡献的预设；每次调用都重新读盘——用户/技能直接编辑
/// `plugins/llm-providers/providers.json` 后重开设置窗口即生效。
#[tauri::command]
pub fn list_provider_presets() -> Vec<crate::plugins::ProviderPresetData> {
    crate::plugins::ensure_builtin_plugins();
    crate::plugins::load_provider_presets()
}

/// 插件 / 技能目录路径（供界面提示用户放置位置）。
#[tauri::command]
pub fn plugin_paths() -> serde_json::Value {
    serde_json::json!({
        "plugins_dir": crate::plugins::plugins_dir().display().to_string(),
        "skills_dir": crate::skills::SkillService::default_dir().display().to_string(),
    })
}

/// 插件运行时诊断（设置页「诊断」区数据源）。
///
/// - `last_report`：最近一次装载（启动 load_all 或设置页 load_one）的报告——
///   装载了哪些插件/技能/工具/MCP/协议，以及 skipped 清单（未信任、清单
///   损坏、JS 装载失败等）
/// - `events`：运行时静默决策的登记（MCP id 被占用、工具重名跳过、provider
///   预设冲突等），最新在前，有界去重
#[tauri::command]
pub fn plugin_diagnostics() -> serde_json::Value {
    let last_report = crate::plugins::last_report().map(|r| {
        serde_json::json!({
            "plugins": r.plugins,
            "skills": r.skills,
            "tools": r.tools,
            "mcp_servers": r.mcp_servers,
            "protocols": r.protocols,
            "skipped": r.skipped,
        })
    });
    serde_json::json!({
        "last_report": last_report,
        "events": crate::plugins::diag_events(),
    })
}

/// 技能清单（用户/插件/内置指导型技能；内置风格预设不展示）。
#[tauri::command]
pub fn list_skills(state: State<'_, Arc<AppState>>) -> Vec<SkillEntryInfo> {
    state
        .skill_service
        .list_all()
        .into_iter()
        // 内置风格预设不出现在设置窗口；两枚内置指导型技能照常展示
        .filter(|s| !BUILTIN_STYLE_SKILLS.contains(&s.name.as_str()))
        .map(|s| SkillEntryInfo {
            origin: skill_origin(&s.name).to_string(),
            kind: skill_kind(&s).to_string(),
            editable: skill_origin(&s.name) == "user",
            body_len: s.body.chars().count(),
            dir: s.path.clone(),
            resources: s.resources.clone(),
            name: s.name,
            description: s.description,
            scope: s.scope,
        })
        .collect()
}

/// 技能详情：正文、关键词、资源清单与是否可编辑（内置技能可读不可编辑）。
#[tauri::command]
pub fn read_skill_detail(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<SkillDetailInfo, String> {
    let name = name.trim();
    let skill = state
        .skill_service
        .find(name)
        .ok_or_else(|| format!("未找到技能「{name}」"))?;
    Ok(detail_of(skill))
}

/// 保存用户技能（目录形态 `<名称>/SKILL.md` + 可选 `references/`、`scripts/` 资源）。
///
/// 只在设置窗口可用；内置与插件技能被后端拒绝。写入前跑同一套形态校验，
/// 硬错误直接拒绝，避免落盘即坏。保存后立即注册，不等 30s 热重载。
#[tauri::command]
pub fn save_skill(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
    description: String,
    body: String,
    keywords: Vec<String>,
    resources: Option<Vec<SkillResourceInput>>,
    create_new: bool,
) -> Result<SkillDetailInfo, String> {
    ensure_config_window(&window)?;
    let name = name.trim().to_string();
    let description = description.replace(['\r', '\n'], " ").trim().to_string();
    let body = body.trim().to_string();
    if !crate::skills::is_valid_skill_name(&name) {
        return Err("技能名仅允许字母、数字、下划线、连字符或中文（≤64 字符）".into());
    }
    if description.is_empty() {
        return Err("description 不能为空".into());
    }
    if body.is_empty() {
        return Err("技能正文不能为空".into());
    }
    if create_new {
        if name.contains('/') || BUILTIN_SKILL_NAMES.contains(&name.as_str()) {
            return Err(format!("「{name}」与内置或插件技能重名，请换一个名称"));
        }
    } else {
        ensure_user_skill(&name)?;
    }

    let dir = crate::skills::SkillService::default_dir();
    let skill_dir = dir.join(&name);
    // 单文件形态不在设置页编辑：提示用户先迁移，避免同名双份
    if !skill_dir.is_dir() && dir.join(format!("{name}.md")).is_file() {
        return Err(format!(
            "「{name}」目前是单文件技能（{name}.md），设置页只编辑目录技能；请先删除该文件或直接编辑它"
        ));
    }

    let keywords: Vec<String> = keywords
        .into_iter()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect();
    let kw_line = keywords.join(", ");
    let content =
        format!("---\nname: {name}\ndescription: {description}\nkeywords: {kw_line}\n---\n\n{body}\n");

    let validation = crate::skills::validate::validate_skill_text(&name, &content);
    if !validation.ok {
        let errors: Vec<String> = validation
            .issues
            .iter()
            .filter(|issue| issue.level == "error")
            .map(|issue| issue.message.clone())
            .collect();
        return Err(format!("技能未通过校验：{}", errors.join("；")));
    }

    std::fs::create_dir_all(&skill_dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
    std::fs::write(skill_dir.join("SKILL.md"), content)
        .map_err(|e| format!("写入 SKILL.md 失败: {e}"))?;

    if let Some(resources) = resources {
        for item in resources {
            let relative = item.path.trim();
            if !crate::skills::is_safe_skill_rel_path(relative) {
                return Err(format!(
                    "资源路径「{relative}」不合法：仅允许 references/ 或 scripts/ 下的一层文件名"
                ));
            }
            if item.content.len() > SKILL_RESOURCE_MAX_BYTES {
                return Err(format!("资源「{relative}」超过 256 KiB"));
            }
            let target = skill_dir.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建资源目录失败: {e}"))?;
            }
            std::fs::write(&target, item.content).map_err(|e| format!("写入资源失败: {e}"))?;
        }
    }

    // 立即注册（不等 30s 热重载）
    let skill = crate::skills::Skill::global(name.clone(), description, body)
        .with_keywords(keywords)
        .with_path(skill_dir.display().to_string())
        .with_resources(crate::skills::collect_skill_resources(&skill_dir));
    state.skill_service.replace_or_register(skill);

    state
        .skill_service
        .find(&name)
        .map(detail_of)
        .ok_or_else(|| "保存后未能重新读取技能".to_string())
}

/// 删除用户技能（目录或单文件）。内置与插件技能被后端拒绝。
#[tauri::command]
pub fn delete_skill(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    ensure_config_window(&window)?;
    let name = name.trim().to_string();
    ensure_user_skill(&name)?;

    let dir = crate::skills::SkillService::default_dir();
    let skill_dir = dir.join(&name);
    let flat = dir.join(format!("{name}.md"));
    let mut removed = false;
    if skill_dir.is_dir() {
        std::fs::remove_dir_all(&skill_dir).map_err(|e| format!("删除技能目录失败: {e}"))?;
        removed = true;
    }
    if flat.is_file() {
        std::fs::remove_file(&flat).map_err(|e| format!("删除技能文件失败: {e}"))?;
        removed = true;
    }
    if !removed {
        return Err(format!("未找到可删除的用户技能「{name}」"));
    }
    state.skill_service.remove_exact(&name);
    Ok(())
}

/// 校验技能形态：传 `content` 校验草稿；传 `name` 校验技能目录。
#[tauri::command]
pub fn validate_skill(
    name: Option<String>,
    content: Option<String>,
) -> Result<crate::skills::validate::SkillValidation, String> {
    let content = content.filter(|text| !text.trim().is_empty());
    let name = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match (content, name) {
        (Some(content), name) => Ok(crate::skills::validate::validate_skill_text(
            name.as_deref().unwrap_or("skill"),
            &content,
        )),
        (None, Some(name)) => {
            let skill_dir = crate::skills::SkillService::default_dir().join(&name);
            if skill_dir.is_dir() {
                Ok(crate::skills::validate::validate_skill_dir(&skill_dir))
            } else {
                Err(format!("未找到技能目录「{name}」"))
            }
        }
        (None, None) => Err("name 与 content 至少给一项".into()),
    }
}

/// Reasoning-only preview. Contains no key, endpoint credentials, messages or tools.
#[tauri::command]
pub fn preview_reasoning_config(
    provider_type: String, model: String,
    preference: Option<crate::providers::reasoning::ReasoningPreference>,
    overrides: Option<serde_json::Value>,
    temperature: Option<f64>, max_tokens: Option<u32>,
    send_temperature: Option<bool>, send_max_tokens: Option<bool>,
    sample_body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    use crate::providers::{reasoning, reasoning_profiles};
    crate::plugins::ensure_builtin_plugins();
    let profile = reasoning_profiles::resolve(&provider_type, &model);
    if let Some(patch) = &overrides { crate::providers::request_body::validate(patch, profile.as_ref())?; }
    let pref = preference.unwrap_or(reasoning::ReasoningPreference::AUTO);
    let base_body = sample_body.unwrap_or_else(||crate::providers::request_body::example(&provider_type, &model, temperature.unwrap_or(0.7), max_tokens.unwrap_or(2048)));
    if !base_body.is_object() || serde_json::to_vec(&base_body).map_err(|e|e.to_string())?.len()>256*1024 {
        return Err("示例请求体必须是 256 KB 以内的 JSON 对象".into());
    }
    let (known, disable, efforts, budget, source, verified_at) = if let Some(p) = &profile {
        (true, p.disabled.is_some(), p.efforts.keys().cloned().collect::<Vec<_>>(), serde_json::to_value(&p.budget).unwrap_or_default(), Some(p.source.clone()), Some(p.verified_at.clone()))
    } else {
        let cap = reasoning::resolve_reasoning_capability(&model);
        (cap.control != reasoning::ReasoningControl::None, cap.supports_disable, cap.supported_efforts.iter().map(|e|e.as_str().to_string()).collect(), serde_json::Value::Null, None, None)
    };
    let adapter = reasoning_profiles::RequestCustomization { profile, overrides: overrides.clone(), provider_type };
    let preview = adapter.finalize(base_body.clone(), pref, &model, send_temperature.unwrap_or(true), send_max_tokens.unwrap_or(true));
    let sampling = adapter.sampling();
    Ok(serde_json::json!({"sampling":sampling,"adapterOrigin":if source.is_some(){"verified_profile"}else{"compatibility"},"known":known,"supportsDisable":disable,"efforts":efforts,"budget":budget,"source":source,"verifiedAt":verified_at,"preview":crate::providers::request_body::redact(&preview),"baseBody":base_body,"overridden":overrides.as_ref().and_then(|v|v.as_object()).map(|v|!v.is_empty()).unwrap_or(false)}))
}
