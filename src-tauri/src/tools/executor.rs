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

/// 把 `Value` 序列化为紧凑 JSON 字符串，失败时回退到 String 化
fn value_to_compact_string(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| v.to_string())
}

/// 对工具结果做字符预算检查：超阈值时截断并保留预览，原始数据塞入 `_truncated_full_data` 字段
/// （由可观测性记录保留，便于二次查询；不让大结果直接塞进 LLM 上下文）
fn enforce_result_budget(tool_name: &str, data: Value, max_chars: usize) -> Value {
    let serialized = value_to_compact_string(&data);
    if serialized.chars().count() <= max_chars {
        return data;
    }
    let preview: String = serialized.chars().take(max_chars).collect();
    tracing::warn!(
        "[executor] 工具 {} 结果超预算（{} 字符），截断为预览版",
        tool_name,
        serialized.chars().count()
    );
    serde_json::json!({
        "_truncated": true,
        "preview": preview,
        "tool": tool_name,
        "full_size_chars": serialized.chars().count(),
        "hint": "完整结果已存入可观测性记录，可通过 tool_history 命令查询",
    })
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
    let permission_context = PermissionContext::default().with_access_level(runtime_cfg.access_level);
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
