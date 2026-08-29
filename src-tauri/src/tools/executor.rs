//! 工具执行管线 - 整合所有组件，提供统一的工具执行接口
//!
//! 完整执行流程：
//! 1. 查找工具
//! 2. 沙箱安全检查
//! 3. 输入验证
//! 4. 缓存检查
//! 5. 权限检查
//! 6. 执行（带超时）
//! 7. 缓存写入

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;

use super::confirmation::{confirmation_info, ConfirmationResponse};
use super::permission::{check_tool_permission, requires_permission};
use super::registry::ToolSystem;
use super::types::{
    AgentAccessLevel, PermissionContext, ToolErrorCode, ToolResult, ToolUseContext,
};

/// 用户确认回调：返回 true 表示允许执行，false 表示拒绝
pub type CanUseTool = Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>;

/// 工具超时配置（工具名 → 超时秒数）
static TOOL_TIMEOUTS: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("read_file", 30);
    m.insert("write_file", 30);
    m.insert("edit_file", 30);
    m.insert("list_directory", 15);
    m.insert("search_files", 60);
    m.insert("grep", 60);
    m.insert("web_search", 60);
    m.insert("take_screenshot", 30);
    // 截屏识图含 LLM 视觉调用，给 90s 余量（截屏 1~2s + 视觉模型 10~60s）
    m.insert("screenshot_analyze", 90);
    m.insert("open_application", 30);
    m.insert("close_application", 30);
    m.insert("save_memory", 15);
    m.insert("search_memory", 15);
    m.insert("set_expression", 10);
    m.insert("play_motion", 10);
    m.insert("trigger_idle_action", 10);
    m
});

/// 工具运行时配置（由 `config.tools` 注入，替代硬编码常量）
///
/// 通过 `update_runtime_config` 在启动/热重载时更新；
/// 执行路径通过 `current_runtime_config` 读取。
#[derive(Debug, Clone)]
pub struct ToolRuntimeConfig {
    /// 默认工具超时（秒）—— 未在 `TOOL_TIMEOUTS` 中登记的工具使用此值
    pub default_tool_timeout_secs: u64,
    /// 单工具结果字符预算，超出则截断为预览版
    pub max_result_chars: usize,
    /// Agent 访问级别（与工具 risk() 共同决定 allow/ask/deny）
    pub access_level: AgentAccessLevel,
}

impl Default for ToolRuntimeConfig {
    fn default() -> Self {
        Self {
            default_tool_timeout_secs: 120,
            max_result_chars: 4000,
            access_level: AgentAccessLevel::FullControl,
        }
    }
}

static RUNTIME_CONFIG: Lazy<RwLock<ToolRuntimeConfig>> =
    Lazy::new(|| RwLock::new(ToolRuntimeConfig::default()));

/// Hook 注册表全局单例（由 brain 初始化时加载）
static HOOK_REGISTRY: Lazy<RwLock<crate::hooks::HookRegistry>> =
    Lazy::new(|| RwLock::new(crate::hooks::HookRegistry::default()));

/// 更新 Hook 注册表（由 ChatChain 装配或热重载时调用）
pub fn set_hook_registry(registry: crate::hooks::HookRegistry) {
    *HOOK_REGISTRY.write() = registry;
}

/// 更新工具运行时配置（由 ChatChain 装配 / 热重载时调用）
pub fn update_runtime_config(cfg: &ToolRuntimeConfig) {
    *RUNTIME_CONFIG.write() = cfg.clone();
}

/// 读取当前运行时配置快照
fn current_runtime_config() -> ToolRuntimeConfig {
    RUNTIME_CONFIG.read().clone()
}

/// 会话级工具放行列表（内存态，应用重启后重置）
///
/// 用户在确认 toast 中选择"本次运行允许"时，工具名写入此集合，
/// 同一应用会话内该工具不再弹出确认。
static SESSION_ALLOWED_TOOLS: Lazy<RwLock<std::collections::HashSet<String>>> =
    Lazy::new(|| RwLock::new(std::collections::HashSet::new()));

/// 检查工具是否已被本次会话放行
fn is_session_allowed(tool: &str) -> bool {
    SESSION_ALLOWED_TOOLS.read().contains(tool)
}

/// 将工具加入本次会话放行列表
fn session_allow(tool: &str) {
    SESSION_ALLOWED_TOOLS.write().insert(tool.to_string());
    tracing::info!("[Executor] 工具 {} 已加入会话级放行列表", tool);
}

/// 查询工具是否已被本次会话放行（供主动截屏等旁路流程复用）
pub(crate) fn is_session_allowed_tool(tool: &str) -> bool {
    is_session_allowed(tool)
}

/// 将工具加入会话级放行列表（旁路流程收到 AllowAlways 后调用）
pub(crate) fn session_allow_tool(tool: &str) {
    session_allow(tool);
}

/// 把 `Value` 序列化为紧凑 JSON 字符串，失败时回退到 String 化
fn value_to_compact_string(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| v.to_string())
}

/// 对工具结果做字符预算检查：超阈值时截断并保留预览。
///
/// 大结果会**落盘到 spill 目录**（`%APPDATA%\Vivian\spill\`），LLM 上下文只拿到
/// 头部预览 + 文件路径定位，需要全文时可经 `read_spilled_result` 命令读取——
/// 避免超大结果直接塞进 LLM 上下文或丢失。
///
/// `max_chars == 0` 表示不限制（设置中填 -1）：跳过预算检查原样返回。
fn enforce_result_budget(tool_name: &str, data: Value, max_chars: usize) -> Value {
    if max_chars == 0 {
        return data;
    }
    let serialized = value_to_compact_string(&data);
    if serialized.chars().count() <= max_chars {
        return data;
    }
    let preview: String = serialized.chars().take(max_chars).collect();
    // 落盘完整结果（spill）
    let spill_path = spill_result(tool_name, &serialized);
    let spill_ref = spill_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string());
    tracing::warn!(
        "[executor] 工具 {} 结果超预算（{} 字符），截断为预览版{}",
        tool_name,
        serialized.chars().count(),
        spill_ref
            .as_deref()
            .map(|p| format!("，完整结果已落盘 {p}"))
            .unwrap_or_default()
    );
    serde_json::json!({
        "_truncated": true,
        "preview": preview,
        "tool": tool_name,
        "full_size_chars": serialized.chars().count(),
        "spill_path": spill_ref,
        "hint": "完整结果已落盘到 spill 文件，可通过 read_spilled_result 命令按路径读取",
    })
}

/// spill 文件最长留存天数：超过即视为可清理的临时产物。
const SPILL_RETENTION_DAYS: i64 = 3;

/// 清理 spill 目录中超过保留期的临时结果文件，避免无限累积。
///
/// spill 文件名比赛含时间戳，因此按文件修改时间判断年龄。
fn prune_spill_dir() {
    let dir = crate::utils::path::get_user_data_dir().join("spill");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = chrono::Local::now().timestamp();
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(modified) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        if now - modified.as_secs() as i64 > SPILL_RETENTION_DAYS * 86_400 {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 把完整结果写入 spill 目录（`%APPDATA%\Vivian\spill\<tool>-<ts>.txt`）。
/// 返回落盘路径；写入失败返回 None（仅告警，不影响主流程）。
fn spill_result(tool_name: &str, content: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let dir = crate::utils::path::get_user_data_dir().join("spill");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    // 每次写入顺带清理一次超期 spill 文件，控制目录体积
    prune_spill_dir();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string();
    let path = dir.join(format!("{}-{}.txt", tool_name, ts));
    match std::fs::File::create(&path) {
        Ok(mut f) => {
            let _ = f.write_all(content.as_bytes());
            Some(path)
        }
        Err(e) => {
            tracing::warn!("[executor] spill 写入失败: {}", e);
            None
        }
    }
}

/// 字符预算裁剪的公开入口（供策略缝对替换后的 post-execute 结果复用同一规则）。
pub fn budget_result(tool_name: String, data: Value, max_chars: usize) -> Value {
    enforce_result_budget(&tool_name, data, max_chars)
}

/// 无模型的文本头尾裁剪：超预算时保留头部大头 + 尾部小段，中段以标记折叠。
///
/// 尾部常承载退出码/报错/diff 结果等高信息量收尾，比纯截头保留得更多。
/// 供编程智能体与陪伴链路（工具反馈历史）共用。
pub fn prune_head_tail(content: &str, max: usize) -> String {
    // 0 = 不限制（设置中填 -1）：原样返回
    if max == 0 {
        return content.to_string();
    }
    let len = content.chars().count();
    if len <= max {
        return content.to_string();
    }
    let head = (max * 2 / 3).max(256).min(len);
    let tail = (max / 6).min(len.saturating_sub(head)).max(0);
    let head_s: String = content.chars().take(head).collect();
    let tail_s: String = content.chars().skip(len.saturating_sub(tail)).collect();
    format!(
        "{head_s}\n…[中段 {} 字符已折叠]…\n{tail_s}",
        len.saturating_sub(head).saturating_sub(tail)
    )
}

/// 获取工具超时时间
pub fn get_tool_timeout(tool_name: &str) -> Duration {
    let cfg = current_runtime_config();
    Duration::from_secs(
        TOOL_TIMEOUTS
            .get(tool_name)
            .copied()
            .unwrap_or(cfg.default_tool_timeout_secs),
    )
}

/// 参数名归一化：LLM 经常幻视参数名（如 app_path → application），
/// 根据工具 schema 的 properties + required 做子串匹配修正。
///
/// 策略：对于 LLM 传来的每个 key，如果 schema.properties 里没有同名项，
/// 就找"该 key 是某个 canonical 名的子串"或"某个 canonical 名是该 key 的子串"的候选，
/// 优先匹配 required 参数。
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn normalize_arguments(tool: &dyn super::types::Tool, arguments: Value) -> Value {
    let schema = tool.parameters_schema();
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return arguments,
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect()
        })
        .unwrap_or_default();

    let canonical_keys: Vec<&str> = properties.keys().map(|k| k.as_str()).collect();

    let obj = match arguments.as_object() {
        Some(o) => o,
        None => return arguments,
    };

    let mut normalized = serde_json::Map::new();
    let mut used_canonicals: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (key, value) in obj {
        if let Some(canonical) = canonical_keys.iter().find(|ck| *ck == key) {
            if used_canonicals.insert(canonical.to_string()) {
                normalized.insert(key.clone(), value.clone());
            } else {
                tracing::warn!(
                    "[executor] 参数名归一化: 跳过重复 key '{}'",
                    key
                );
            }
            continue;
        }

        let key_norm = normalize_key(key);
        let mut candidates: Vec<&&str> = Vec::new();
        for ck in &canonical_keys {
            let ck_norm = normalize_key(ck);
            if key_norm == ck_norm {
                candidates.push(ck);
            }
        }

        if candidates.is_empty() {
            for ck in &canonical_keys {
                let ck_norm = normalize_key(ck);
                if (key_norm.len() >= 3 && ck_norm.starts_with(&key_norm))
                    || (ck_norm.len() >= 3 && key_norm.starts_with(&ck_norm))
                {
                    candidates.push(ck);
                }
            }
        }

        candidates.sort_by_key(|ck| {
            (
                if required.contains(ck) { 0 } else { 1 },
                normalize_key(ck).len().abs_diff(key_norm.len()),
            )
        });

        let mut matched = None;
        for candidate in candidates {
            if used_canonicals.insert(candidate.to_string()) {
                matched = Some(*candidate);
                break;
            }
        }

        if let Some(canonical) = matched {
            tracing::info!(
                "[executor] 参数名归一化: '{}' → '{}' (工具: {})",
                key,
                canonical,
                tool.name()
            );
            normalized.insert(canonical.to_string(), value.clone());
        } else {
            tracing::debug!(
                "[executor] 参数名 '{}' 无法匹配到 schema，保留原样 (工具: {})",
                key,
                tool.name()
            );
            normalized.insert(key.clone(), value.clone());
        }
    }

    Value::Object(normalized)
}

/// 执行工具调用 - 完整的执行管线
///
/// # 参数
/// - `tool_name`: 工具名称
/// - `arguments`: 工具输入参数（JSON）
/// - `tool_system`: 工具系统
/// - `context`: 工具调用上下文
/// - `can_use_tool`: 用户确认回调（`None` 时通过 Tauri 事件向前端请求确认）
///
/// # 返回
/// 工具执行结果
pub async fn execute_tool_use(
    tool_name: &str,
    arguments: Value,
    tool_system: &ToolSystem,
    context: &ToolUseContext,
    can_use_tool: Option<CanUseTool>,
) -> ToolResult {
    // 1. 查找工具
    let tool = match tool_system.find_tool(tool_name) {
        Some(t) => t,
        None => {
            let info = ToolErrorCode::ToolNotFound.get_error_info();
            return ToolResult::standard_error(
                &info.suggestion,
                Some(&info.name),
                None,
            );
        }
    };

    // 1.05 用户禁用的工具直接拒绝（正常情况下禁用工具不会出现在 LLM 工具列表中，
    // 此处防御 LLM 幻觉调用旧工具名 / 历史消息重放）
    if tool_system.is_tool_disabled(tool_name) {
        tracing::warn!("[ToolExecutor] 工具 {} 已被用户禁用，拒绝执行", tool_name);
        return ToolResult::standard_error(
            &format!("工具 {tool_name} 已被用户在设置中禁用，无法执行。请改用其他可用工具。"),
            None,
            None,
        );
    }

    // 1.1 记录工具调用时间戳（用于 ToolScene::from_full_context 判定 Chat/Task 场景）
    tool_system.record_tool_call();

    // 1.5 参数名归一化（LLM 经常用错参数名，如 app_path → application）
    let arguments = normalize_arguments(tool.as_ref(), arguments);

    // 2. 沙箱安全检查
    let safety = tool_system
        .sandbox
        .check_tool_safety(tool_name, &arguments, Some(context));
    if !safety.allowed {
        if safety.requires_confirmation {
            // 委托给 can_use_tool 回调
            if let Some(cb) = &can_use_tool {
                if !cb(tool_name, &arguments) {
                    return ToolResult::standard_error(
                        "用户拒绝了沙箱确认请求",
                        Some("UserDenied"),
                        None,
                    );
                }
            } else {
                return ToolResult::standard_error(
                    &safety.warning,
                    Some("SandboxConfirmationRequired"),
                    None,
                );
            }
        } else {
            return ToolResult::standard_error(
                &safety.message,
                Some("SandboxViolation"),
                None,
            );
        }
    }

    // 3. 输入验证
    let validation = tool.validate_input(&arguments, context).await;
    if !validation.result {
        return ToolResult::standard_error(
            &validation.message,
            Some("InvalidInput"),
            None,
        );
    }
    let validated_args = validation.data.unwrap_or(arguments.clone());

    // 3.5 PreToolUse Hook 拦截（外部脚本可 deny 阻止执行）
    {
        let registry = HOOK_REGISTRY.read().clone(); // clone to avoid holding guard across await
        if registry.has_pre_tool_hooks() {
            let decision = crate::hooks::dispatch_hooks(
                &registry,
                crate::hooks::HookEventName::PreToolUse,
                tool_name,
                &validated_args,
                &context.session_id,
            )
            .await;
            if let crate::hooks::HookDecision::Deny { reason } = decision {
                return ToolResult::standard_error(
                    &format!("Hook 拒绝了工具执行: {}", reason),
                    Some("HookDenied"),
                    None,
                );
            }
        }
    }

    // 4. 缓存检查（只读工具）
    let is_read_only = tool.is_read_only();
    if is_read_only {
        if let Some(cached) = tool_system.cache.get(tool_name, &validated_args) {
            tracing::debug!("工具 {} 命中缓存", tool_name);
            return ToolResult::success(cached);
        }
    }

    // 5. 权限检查
    let runtime_cfg = current_runtime_config();
    // 会话级访问级别覆盖优先（编程智能体按会话权限控制），否则回退全局 runtime config
    let access_level = context.access_level.unwrap_or(runtime_cfg.access_level);
    let mut permission_context = PermissionContext::default().with_access_level(access_level);
    // 注册工具上下文的工作目录为已授权目录：工作目录内的读写操作免用户确认
    // （路径范围已由各工具 validate_input 的沙箱校验限制在工作目录内；
    //   ReadOnly 访问级别注册为只读目录，写入仍会被权限检查拒绝）
    if !context.working_directory.is_empty() {
        permission_context.add_working_directory(
            context.working_directory.clone(),
            access_level == AgentAccessLevel::ReadOnly,
        );
    }
    if requires_permission(tool.as_ref(), &validated_args, &permission_context) {
        let permission =
            check_tool_permission(tool.as_ref(), &validated_args, context, &permission_context)
                .await;

        if permission.is_denied() {
            return ToolResult::standard_error(
                &permission.message,
                Some("PermissionDenied"),
                None,
            );
        }

        if permission.requires_confirmation() {
            // 能力进化事件（create_tool）例外：宿主的 can_use_tool 自动放行回调
            // （如工作智能体的 coding_sandbox_allow）不适用——新工具创建必须经
            // 用户预览卡片授权，无论发起方是陪伴侧还是工作智能体
            let evolution_gate = tool_name == "create_tool";
            if let Some(cb) = &can_use_tool {
                if !evolution_gate {
                    if !cb(tool_name, &validated_args) {
                        let info = ToolErrorCode::UserDenied.get_error_info();
                        return ToolResult::standard_error(
                            &info.suggestion,
                            Some(&info.name),
                            None,
                        );
                    }
                }
            }
            if evolution_gate || can_use_tool.is_none() {
                // 快速通道 1：会话级放行列表（用户本次运行内已选择"本次运行允许"）
                if is_session_allowed(tool_name) {
                    tracing::debug!("[Executor] 工具 {} 命中会话放行列表，跳过确认", tool_name);
                } else if tool_name == "open_application"
                    && validated_args
                        .get("application")
                        .and_then(|v| v.as_str())
                        .map(super::trust::is_trusted_app)
                        .unwrap_or(false)
                {
                    // 快速通道 2：应用信任列表（用户已将该应用加入白名单）
                    tracing::debug!("[Executor] open_application 命中信任列表，跳过确认");
                } else {
                    // 通过 Tauri 事件向前端请求三态确认
                    let (risk_level, reason) = confirmation_info(tool_name, &validated_args);
                    let scope = if tool_name == "open_application" {
                        "persistent"
                    } else {
                        "session"
                    };
                    match tool_system
                        .request_confirmation(
                            tool_name,
                            &validated_args,
                            reason,
                            risk_level,
                            &context.char_id,
                            scope,
                        )
                        .await
                    {
                        Some(ConfirmationResponse::AllowOnce) => {
                            // 用户同意一次 → 会话级放行，后续同类工具自动批准
                            session_allow(tool_name);
                        }
                        Some(ConfirmationResponse::AllowAlways) => {
                            // open_application → 持久化信任列表；其余工具 → 会话级放行
                            if tool_name == "open_application" {
                                if let Some(app) =
                                    validated_args.get("application").and_then(|v| v.as_str())
                                {
                                    super::trust::add_trusted_app(app);
                                }
                            } else {
                                session_allow(tool_name);
                            }
                        }
                        Some(ConfirmationResponse::Deny) => {
                            let info = ToolErrorCode::UserDenied.get_error_info();
                            return ToolResult::standard_error(
                                &info.suggestion,
                                Some(&info.name),
                                None,
                            );
                        }
                        None => {
                            // 无 AppHandle 或用户未响应，返回 PermissionRequired
                            return ToolResult::standard_error(
                                &permission.message,
                                Some("PermissionRequired"),
                                None,
                            );
                        }
                    }
                }
            }
        }
    }

    // 5.5 策略缝 guard 阶段：pre-execute 瀑布（可插桩；未注入 ctx 或空注册时静默跳过）
    if let Some(policy_ctx) = tool_system.policy_ctx() {
        use super::policy::{ToolGuardDecision, ToolGuardFlow};
        let flow = ToolGuardFlow {
            tool_name: tool_name.to_string(),
            arguments: validated_args.clone(),
            char_id: context.char_id.clone(),
            decision: None,
            note: None,
        };
        if let Some(flow) = policy_ctx.emit_waterfall(flow).await {
            if let Some(decision) = flow.decision {
                match decision {
                    ToolGuardDecision::Allow => {}
                    ToolGuardDecision::Deny { reason } => {
                        return ToolResult::standard_error(
                            &reason,
                            Some("GuardDenied"),
                            None,
                        );
                    }
                    ToolGuardDecision::RequireConfirmation { reason } => {
                        if let Some(cb) = &can_use_tool {
                            if !cb(tool_name, &validated_args) {
                                let info = ToolErrorCode::UserDenied.get_error_info();
                                return ToolResult::standard_error(
                                    &info.suggestion,
                                    Some(&info.name),
                                    None,
                                );
                            }
                        } else {
                            return ToolResult::standard_error(
                                &reason,
                                Some("PermissionRequired"),
                                None,
                            );
                        }
                    }
                }
            }
        } else {
            return ToolResult::standard_error(
                "策略 guard 否决了工具执行",
                Some("GuardDenied"),
                None,
            );
        }
    }

    // 6. 执行（带超时）
    let timeout = get_tool_timeout(tool_name);
    let obs_record = tool_system
        .observability
        .start_call(tool_name, validated_args.clone());

    let exec_tool = Arc::clone(&tool);
    let exec_args = validated_args.clone();
    let exec_context = context.clone();
    let exec_future = async move {
        exec_tool.call(exec_args, &exec_context).await
    };

    let mut result = match tokio::time::timeout(timeout, exec_future).await {
        Ok(r) => r,
        Err(_) => {
            tool_system.observability.end_call(
                obs_record,
                false,
                None,
                Some(format!("工具执行超时（{}秒）", timeout.as_secs())),
            );
            let info = ToolErrorCode::TimeoutError.get_error_info();
            return ToolResult::standard_error(
                &info.suggestion,
                Some(&info.name),
                None,
            );
        }
    };

    // 7. 可观测性记录 + 缓存写入 + 结果预算
    if result.success {
        // 全量数据先写入 observability（保留完整结果供二次查询）
        tool_system
            .observability
            .end_call(obs_record, true, result.data.clone(), None);

        if let Some(full_data) = result.data.take() {
            // 应用字符预算：超阈值则截断为预览版
            let budgeted = enforce_result_budget(tool_name, full_data.clone(), current_runtime_config().max_result_chars);
            // 只读工具缓存的是预览版（避免缓存膨胀 + 让 LLM 看到的就是缓存版）
            if is_read_only {
                tool_system
                    .cache
                    .set(tool_name, &validated_args, budgeted.clone());
            }
            result.data = Some(budgeted);
        }
    } else {
        tool_system.observability.end_call(
            obs_record,
            false,
            None,
            result.error.clone(),
        );
    }

    // 7.5 PostToolUse Hook 通知（信息性，无 deny 能力）
    {
        let registry = HOOK_REGISTRY.read();
        if registry.has_post_tool_hooks() {
            let tool_name = tool_name.to_string();
            let args_clone = validated_args.clone();
            let session_id = context.session_id.clone();
            let registry = registry.clone();
            // fire-and-forget：不阻塞工具结果返回
            tauri::async_runtime::spawn(async move {
                crate::hooks::dispatch_hooks(
                    &registry,
                    crate::hooks::HookEventName::PostToolUse,
                    &tool_name,
                    &args_clone,
                    &session_id,
                )
                .await;
            });
        }
    }

    // 8. Goal Satisfaction 检测：工具声明"成功即任务完成"且本调用成功 →
    //    标记 goal_completed，让上层 Agent 循环可据此提前终止，避免 LLM
    //    在任务已达成时继续推理出多余动作。
    if result.success && tool.signals_goal_completion() {
        result.goal_completed = true;
        tracing::info!(
            "[executor] 工具 {} 声明 goal_completion，本次调用成功 → 标记任务完成",
            tool_name
        );
    }

    // 9. 策略缝 post-execute 阶段（可插桩；可替换结果/阻断。未注入 ctx 或空注册时静默即透传）
    if let Some(policy_ctx) = tool_system.policy_ctx() {
        use super::policy::ToolPostExecute;
        let pe = ToolPostExecute {
            tool_name: tool_name.to_string(),
            arguments: validated_args.clone(),
            char_id: context.char_id.clone(),
            success: result.success,
            data: result.data.clone(),
            error: result.error.clone(),
            replace: None,
            block: None,
        };
        if let Some(pe) = policy_ctx.emit_waterfall(pe).await {
            if let Some(mut rewritten) =
                pe.into_result(current_runtime_config().max_result_chars)
            {
                // 保留 goal_completed 语义，避免 post 改写破坏任务完成判定
                rewritten.goal_completed = result.goal_completed;
                result = rewritten;
            }
        }
    }

    result
}

/// 执行多个工具调用（顺序执行）
pub async fn execute_tool_calls(
    calls: Vec<(String, Value)>,
    tool_system: &ToolSystem,
    context: &ToolUseContext,
    can_use_tool: Option<CanUseTool>,
) -> Vec<ToolResult> {
    let mut results = Vec::with_capacity(calls.len());
    for (tool_name, args) in calls {
        let result =
            execute_tool_use(&tool_name, args, tool_system, context, can_use_tool.clone()).await;
        results.push(result);
    }
    results
}

/// 并行执行多个工具调用
pub async fn execute_tool_calls_parallel(
    calls: Vec<(String, Value)>,
    tool_system: Arc<ToolSystem>,
    context: ToolUseContext,
    can_use_tool: Option<CanUseTool>,
) -> Vec<ToolResult> {
    let mut futures = Vec::with_capacity(calls.len());
    for (tool_name, args) in calls {
        let ts = Arc::clone(&tool_system);
        let ctx = context.clone();
        let cb = can_use_tool.clone();
        futures.push(tokio::spawn(async move {
            execute_tool_use(&tool_name, args, &ts, &ctx, cb).await
        }));
    }

    let mut results = Vec::with_capacity(futures.len());
    for f in futures {
        match f.await {
            Ok(r) => results.push(r),
            Err(e) => results.push(ToolResult::standard_error(
                &format!("工具任务异常: {}", e),
                Some("TaskJoinError"),
                None,
            )),
        }
    }
    results
}
