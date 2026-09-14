//! 自建工具（Custom Tools）—— 智能体运行时构建的可执行能力
//!
//! 与 `create_skill`（提示词级知识沉淀）互补，这里让智能体把**可执行能力**
//! 沉淀为新工具：一个自建工具 = (名称, 描述, JSON Schema, PowerShell 脚本)，
//! 持久化为 `<用户数据目录>/tools/<name>.json`，由 [`DynamicTool`] 适配器
//! 包装成 `Tool` trait 实现并注册进 `ToolSystem`——注册表是 `RwLock<HashMap>`，
//! 工具列表每请求实时读取，因此**注册即生效**（下一轮对话、甚至同一 agent
//! 循环内的后续工具调用都可见可用）。
//!
//! 执行契约（stdin/stdout）：
//! - 调用参数以 JSON 字符串写入脚本 stdin；
//! - 脚本用 `$args = [Console]::In.ReadToEnd() | ConvertFrom-Json` 读取；
//! - stdout 作为工具结果返回给 LLM。
//!
//! 安全护栏：
//! - 工具名 `^[a-zA-Z0-9_-]{1,64}$`（OpenAI 函数调用兼容，防路径穿越）
//! - 不可与任何已注册工具重名（不能影子化内置工具）
//! - 脚本双层审查：字符串黑名单 [`FORBIDDEN_FRAGMENTS`]（廉价预筛）+
//!   AST 审计（[`crate::tools::ps_audit`]，别名/结构级拦截破坏性命令；
//!   创建时 + 装载时 + 每次执行时三重校验，执行层带结果缓存）
//! - `risk() = Shell`：每次调用走权限网关审批矩阵，用户可三态确认
//!   （拒绝 / 放行一次 / 本次运行允许），与 `run_command` 同级管控
//! - 进程加固：`-NoProfile -NonInteractive` + CREATE_NO_WINDOW + 超时 kill
//!   （kill_on_drop）+ Job Object 兜底回收 + 输出截断，全部复用 run_command 策略

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::builtin::coding_tools::{truncate_chars, FORBIDDEN_FRAGMENTS};
use super::registry::ToolSystem;
use super::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::utils::path::get_user_data_dir;

/// 脚本执行超时（秒），与 run_command 默认一致
const SCRIPT_TIMEOUT_SECS: u64 = 120;
/// stdout/stderr 合计截断上限（字符），与 run_command 一致
const OUTPUT_MAX_CHARS: usize = 8000;

/// 自建工具定义（持久化 schema）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDef {
    /// 工具名（`^[a-zA-Z0-9_-]{1,64}$`，同时是文件名）
    pub name: String,
    /// 工具描述（注入 LLM 工具列表，说明何时调用）
    pub description: String,
    /// 输入参数 JSON Schema（type: object）
    pub parameters: Value,
    /// PowerShell 脚本：stdin 读 JSON 参数，stdout 输出结果
    pub script: String,
    /// 动态注入等级：false = 始终注入（完整 schema 常驻 prompt）；
    /// true = 延迟加载（仅列名于 deferred 块，经 tool_search 按需加载 schema）
    #[serde(default)]
    pub deferred: bool,
    /// 创建时间戳（秒）
    pub created_at: f64,
}

/// 工具名合法性：ASCII 字母/数字/`_`/`-`，长度 1-64。
///
/// 兼容 OpenAI 函数调用命名约束（`^[a-zA-Z0-9_-]+$`），
/// 同时天然排除路径分隔符与中文，防穿越。
pub fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 脚本内容黑名单校验：命中破坏性片段返回命中的片段。
pub fn forbidden_fragment_in(script: &str) -> Option<&'static str> {
    let lower = script.to_ascii_lowercase();
    FORBIDDEN_FRAGMENTS
        .iter()
        .copied()
        .find(|f| lower.contains(f))
}

/// 校验参数 schema：必须是 object（或空值 → 默认空 schema）。
pub(crate) fn sanitize_parameters(params: &Value) -> Option<Value> {
    match params {
        Value::Null => Some(serde_json::json!({
            "type": "object", "properties": {}, "required": []
        })),
        v if !v.is_object() => None,
        v => {
            // 若声明了 type，必须是 object
            if let Some(t) = v.get("type").and_then(Value::as_str) {
                if t != "object" {
                    return None;
                }
            }
            let mut out = v.clone();
            if out.get("type").is_none() {
                out["type"] = Value::String("object".into());
            }
            Some(out)
        }
    }
}

/// 对工具输入执行受限 JSON Schema 校验。
fn validate_schema_value(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("{path}: schema 嵌套过深"));
    }
    if schema == &Value::Bool(true) || schema.is_null() {
        return Ok(());
    }
    if schema == &Value::Bool(false) {
        return Err(format!("{path}: schema 拒绝该值"));
    }
    let Some(schema_obj) = schema.as_object() else {
        return Err(format!("{path}: schema 必须是对象"));
    };
    if let Some(enum_values) = schema_obj.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|candidate| candidate == value) {
            return Err(format!("{path}: 值不在 enum 允许范围内"));
        }
    }
    if let Some(type_value) = schema_obj.get("type") {
        let matches_type = |kind: &str| match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        let valid = type_value
            .as_str()
            .map(matches_type)
            .or_else(|| type_value.as_array().map(|types| {
                types.iter().any(|t| t.as_str().map(matches_type).unwrap_or(true))
            }))
            .unwrap_or(true);
        if !valid {
            return Err(format!("{path}: 类型不符合 schema"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema_obj.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path}: 缺少必填参数 `{name}`"));
                }
            }
        }
        if let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) {
            for (name, child) in properties {
                if let Some(child_value) = object.get(name) {
                    validate_schema_value(child, child_value, &format!("{path}.{name}"), depth + 1)?;
                }
            }
            if schema_obj.get("additionalProperties") == Some(&Value::Bool(false)) {
                for name in object.keys() {
                    if !properties.contains_key(name) {
                        return Err(format!("{path}: 不允许额外参数 `{name}`"));
                    }
                }
            }
        }
    }
    if let Some(array) = value.as_array() {
        if let Some(min) = schema_obj.get("minItems").and_then(Value::as_u64) {
            if array.len() < min as usize {
                return Err(format!("{path}: 数组元素少于 {min}"));
            }
        }
        if let Some(max) = schema_obj.get("maxItems").and_then(Value::as_u64) {
            if array.len() > max as usize {
                return Err(format!("{path}: 数组元素多于 {max}"));
            }
        }
        if let Some(items) = schema_obj.get("items") {
            for (index, child) in array.iter().enumerate() {
                validate_schema_value(items, child, &format!("{path}[{index}]"), depth + 1)?;
            }
        }
    }
    if let Some(string) = value.as_str() {
        if let Some(min) = schema_obj.get("minLength").and_then(Value::as_u64) {
            if string.chars().count() < min as usize {
                return Err(format!("{path}: 字符串长度小于 {min}"));
            }
        }
        if let Some(max) = schema_obj.get("maxLength").and_then(Value::as_u64) {
            if string.chars().count() > max as usize {
                return Err(format!("{path}: 字符串长度超过 {max}"));
            }
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(min) = schema_obj.get("minimum").and_then(Value::as_f64) {
            if number < min {
                return Err(format!("{path}: 数值小于 minimum"));
            }
        }
        if let Some(max) = schema_obj.get("maximum").and_then(Value::as_f64) {
            if number > max {
                return Err(format!("{path}: 数值大于 maximum"));
            }
        }
    }
    Ok(())
}

/// 对工具输入执行受限 JSON Schema 校验（文件工具与 JS 插件工具共用）。
pub(crate) fn validate_tool_input(schema: &Value, input: &Value) -> ValidationResult {
    if !input.is_object() {
        return ValidationResult::failure("工具参数必须是 JSON object", 400);
    }
    match validate_schema_value(schema, input, "$", 0) {
        Ok(()) => ValidationResult::success(None),
        Err(message) => ValidationResult::failure(message, 400),
    }
}

// ============================================================================
// DynamicTool —— CustomToolDef → Tool trait 适配器
// ============================================================================

/// 自建工具的运行时形态：包装 [`CustomToolDef`] 实现 `Tool`。
pub struct DynamicTool {
    def: CustomToolDef,
}

impl DynamicTool {
    pub fn new(def: CustomToolDef) -> Self {
        Self { def }
    }
}

#[async_trait]
impl Tool for DynamicTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters_schema(&self) -> Value {
        self.def.parameters.clone()
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        validate_tool_input(&self.def.parameters, input)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        // 风险由 risk()=Shell 声明，走权限网关审批矩阵（通常需用户确认）
        PermissionResult::allow()
    }

    // Windows 分支走沙箱执行，沙箱接口不接收工作目录，context 在该平台不被消费；
    // 非 Windows 分支由 Command builder 读取 context.working_directory。
    #[cfg_attr(windows, allow(unused_variables))]
    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        // 执行时防御（第一层）：文件可能被创建后手动改写，字符串黑名单零成本先过
        if let Some(frag) = forbidden_fragment_in(&self.def.script) {
            return ToolResult::standard_error(
                &format!("工具脚本包含破坏性片段「{frag}」，已被拒绝执行。请用 create_tool 修正脚本。"),
                Some("ForbiddenScript"),
                None,
            );
        }
        // 执行时防御（第二层）：AST 审计（带缓存，同一脚本仅首次有进程开销）——
        // 拦截别名/结构绕过字符串黑名单的破坏性命令
        let audit = match crate::tools::ps_audit::audit_cached(&self.def.script).await {
            Ok(a) => a,
            Err(e) => {
                // 审计不可用 → 失败关闭，不放行不可审计的脚本
                return ToolResult::standard_error(
                    &format!("脚本安全审计不可用，已拒绝执行: {e}"),
                    Some("AuditUnavailable"),
                    None,
                );
            }
        };
        if let Some(summary) = audit.summary() {
            return ToolResult::standard_error(
                &format!("{summary}\n脚本已被拒绝执行，请用 create_tool 修正脚本。"),
                Some("ScriptAuditViolation"),
                None,
            );
        }

        // 执行时防御（第三层，Windows）：受限令牌沙箱——特权全剥 + Users
        // restricting SID + 低完整性级别，唯一可写面是 Low IL scratch 目录。
        // 建立失败即拒绝执行（不回退全权限）。非 Windows 无此威胁面，走普通 spawn。
        #[cfg(windows)]
        {
            let script = self.def.script.clone();
            let payload = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
            let out =
                match tokio::task::spawn_blocking(move || {
                    crate::utils::sandbox::run_powershell_sandboxed(
                        &script,
                        &payload,
                        SCRIPT_TIMEOUT_SECS,
                    )
                })
                .await
                {
                    Ok(Ok(out)) => out,
                    Ok(Err(e)) => {
                        return ToolResult::standard_error(
                            &format!("沙箱执行不可用，已拒绝执行脚本: {e}"),
                            Some("SandboxUnavailable"),
                            None,
                        );
                    }
                    Err(e) => {
                        return ToolResult::standard_error(
                            &format!("沙箱任务失败: {e}"),
                            Some("SandboxFailed"),
                            None,
                        );
                    }
                };
            if out.timed_out {
                return ToolResult::standard_error(
                    &format!("工具脚本执行超时（{SCRIPT_TIMEOUT_SECS} 秒），已终止"),
                    Some("timeout"),
                    None,
                );
            }
            let truncated = out.stdout.len() + out.stderr.len() > OUTPUT_MAX_CHARS;
            let stdout_t = truncate_chars(&out.stdout, OUTPUT_MAX_CHARS);
            let stderr_t = truncate_chars(&out.stderr, OUTPUT_MAX_CHARS / 2);
            return ToolResult::standard_success(
                &format!(
                    "脚本退出码 {}{}",
                    out.exit_code,
                    if truncated { "（输出已截断）" } else { "" }
                ),
                Some(serde_json::json!({
                    "tool": self.def.name,
                    "exit_code": out.exit_code,
                    "success": out.exit_code == 0,
                    "stdout": stdout_t,
                    "stderr": stderr_t,
                    "truncated": truncated,
                    "sandboxed": true,
                })),
            );
        }

        #[cfg(not(windows))]
        {
            let cwd = if context.working_directory.is_empty() {
                None
            } else {
                Some(PathBuf::from(&context.working_directory))
            };

            let mut cmd = tokio::process::Command::new("powershell");
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", &self.def.script])
                .kill_on_drop(true) // 超时 drop future 时同步终止进程
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if let Some(dir) = cwd.as_ref() {
                cmd.current_dir(dir);
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    return ToolResult::standard_error(&format!("启动脚本进程失败: {e}"), None, None)
                }
            };
            // 绑定全局 Job Object：宿主崩溃/强杀时脚本进程由 OS 兜底回收
            // （kill_on_drop 只覆盖超时/应用层正常清理路径；插件工具执行的是
            // 不可信 PowerShell，生命周期遏制必须不依赖应用层存活）
            crate::utils::process::assign_child_to_job(&child);

            // 参数 JSON 写入 stdin 后关闭管道（脚本读到 EOF）
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let payload = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                if let Err(e) = stdin.write_all(payload.as_bytes()).await {
                    return ToolResult::standard_error(&format!("写入脚本 stdin 失败: {e}"), None, None);
                }
                drop(stdin);
            }

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS),
                child.wait_with_output(),
            )
            .await;

            match output {
                Err(_) => ToolResult::standard_error(
                    &format!("工具脚本执行超时（{SCRIPT_TIMEOUT_SECS} 秒），已终止"),
                    Some("timeout"),
                    None,
                ),
                Ok(Err(e)) => {
                    ToolResult::standard_error(&format!("执行脚本失败: {e}"), None, None)
                }
                Ok(Ok(out)) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    let code = out.status.code().unwrap_or(-1);
                    let truncated = stdout.len() + stderr.len() > OUTPUT_MAX_CHARS;
                    let stdout_t = truncate_chars(&stdout, OUTPUT_MAX_CHARS);
                    let stderr_t = truncate_chars(&stderr, OUTPUT_MAX_CHARS / 2);
                    let success = out.status.success();
                    ToolResult::standard_success(
                        &format!("脚本退出码 {code}{}", if truncated { "（输出已截断）" } else { "" }),
                        Some(serde_json::json!({
                            "tool": self.def.name,
                            "exit_code": code,
                            "success": success,
                            "stdout": stdout_t,
                            "stderr": stderr_t,
                            "truncated": truncated,
                        })),
                    )
                }
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn is_custom(&self) -> bool {
        // 智能体自进化创造的非默认工具
        true
    }

    fn risk(&self) -> ToolRiskTier {
        // 智能体生成的可执行脚本，与 run_command 同级管控
        ToolRiskTier::Shell
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        // 动态注入等级由创建时选择：延迟加载的工具仅列名，经 tool_search 按需加载
        self.def.deferred
    }

    fn search_hint(&self) -> &str {
        &self.def.description
    }
}

// ============================================================================
// 目录装载与热重载（模式同 skills）
// ============================================================================

/// 自建工具目录：`<用户数据目录>/tools`
pub fn tools_dir() -> PathBuf {
    get_user_data_dir().join("tools")
}

/// 从目录装载全部自建工具并注册。返回成功注册的工具名。
///
/// 跳过不合法的定义（名字非法 / 脚本含黑名单片段 / schema 非 object /
/// AST 审计违规），单个文件损坏不影响其余装载。
pub fn load_all(tool_system: &ToolSystem) -> Vec<String> {
    let dir = tools_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(), // 目录不存在：尚无自建工具，正常
    };

    let mut candidates: Vec<(String, CustomToolDef)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(def) = serde_json::from_str::<CustomToolDef>(&text) else {
            tracing::warn!("[CustomTools] 定义文件损坏，跳过: {}", path.display());
            continue;
        };
        if !is_valid_tool_name(&def.name) {
            tracing::warn!("[CustomTools] 工具名不合法，跳过: {}", def.name);
            continue;
        }
        if forbidden_fragment_in(&def.script).is_some() {
            tracing::warn!("[CustomTools] 脚本含破坏性片段，跳过: {}", def.name);
            continue;
        }
        let Some(params) = sanitize_parameters(&def.parameters) else {
            tracing::warn!("[CustomTools] 参数 schema 非 object，跳过: {}", def.name);
            continue;
        };
        candidates.push((def.name.clone(), CustomToolDef { parameters: params, ..def }));
    }

    // AST 审计批量过检（一个 PS 进程处理全部候选；审计不可用即整批拒绝）
    let scripts: Vec<String> = candidates.iter().map(|(_, d)| d.script.clone()).collect();
    let audits = match crate::tools::ps_audit::audit_scripts_blocking(&scripts) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[CustomTools] 脚本审计不可用，本批全部跳过: {e}");
            return Vec::new();
        }
    };

    let mut loaded = Vec::new();
    for ((name, def), audit) in candidates.into_iter().zip(audits) {
        if let Some(summary) = audit.summary() {
            tracing::warn!("[CustomTools] 工具 {name} 脚本审计未通过，跳过: {}", summary.replace('\n', " | "));
            continue;
        }
        tool_system.register_tool(Arc::new(DynamicTool::new(def)));
        loaded.push(name);
    }
    loaded
}

/// 目录指纹：`(文件名, mtime 毫秒)` 列表，用于廉价变更检测。
fn dir_fingerprint(dir: &std::path::Path) -> Vec<(String, u128)> {
    let mut fp: Vec<(String, u128)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                .filter_map(|e| {
                    let mtime = e
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())?;
                    Some((e.file_name().to_string_lossy().into_owned(), mtime))
                })
                .collect()
        })
        .unwrap_or_default();
    fp.sort();
    fp
}

/// 启动后台热刷新：目录变更时重装载（新建/更新 → 重注册替换；删除 → 注销）。
///
/// 与技能热重载同模式：轮询 stat 对比指纹，不引入 notify 依赖。
/// 指纹只能感知"变了"，增删改的具体差异由注册表 diff 补齐。
///
/// 进程内幂等：循环是永不退出的常驻任务，重复调用（如 reinitialize 重跑
/// initialize）只会叠加循环，故用静态标志保证只 spawn 一次。
pub fn spawn_hot_reload(tool_system: Arc<ToolSystem>, interval: std::time::Duration) {
    static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SPAWNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let dir = tools_dir();
    let mut last = dir_fingerprint(&dir);
    tauri::async_runtime::spawn(async move {
        crate::utils::watchdog::register("custom_tools_hot_reload", interval.as_secs_f64(), None);
        loop {
            tokio::time::sleep(interval).await;
            crate::utils::watchdog::beat("custom_tools_hot_reload");
            let current = dir_fingerprint(&dir);
            if current != last {
                // 删除检测：旧指纹中已消失的文件 → 注销对应工具
                // （文件名与工具名一致是装载约定，create_tool 写入时保证）
                let current_names: Vec<String> =
                    current.iter().map(|(n, _)| n.trim_end_matches(".json").to_string()).collect();
                for (old, _) in &last {
                    let old_name = old.trim_end_matches(".json");
                    if !current_names.iter().any(|n| n == old_name) {
                        tool_system.unregister_tool(old_name);
                    }
                }
                let loaded = load_all(&tool_system);
                tracing::info!("[CustomTools] 工具目录变更，热重载完成：{:?}", loaded);
                last = current;
            }
        }
    });
}

// ============================================================================
// 创建入口（供 create_tool 元工具调用）
// ============================================================================

/// 校验并落盘一个自建工具定义，注册后返回工具名。
///
/// 调用方（create_tool 元工具）须在调用前完成用户授权确认。
/// 错误以中文消息返回（面向 LLM 自我修正），成功返回注册后的工具名。
pub fn create_custom_tool(
    tool_system: &ToolSystem,
    name: &str,
    description: &str,
    parameters: &Value,
    script: &str,
    deferred: bool,
) -> Result<String, String> {
    let name = name.trim();
    if !is_valid_tool_name(name) {
        return Err(
            "工具名仅允许 ASCII 字母、数字、下划线、连字符（1-64 字符），不能包含空格、中文或斜杠".into(),
        );
    }
    if description.trim().is_empty() {
        return Err("description 是必填项（说明何时调用此工具）".into());
    }
    // 同名区分：工具目录已有同名 .json → 更新自己的自建工具（允许，能力迭代必需）；
    // 无同名文件但已注册 → 影子化内置/其他工具（禁止）
    let dir = tools_dir();
    let is_update = dir.join(format!("{name}.json")).exists();
    if !is_update && tool_system.has_tool(name) {
        return Err(format!(
            "工具名「{name}」已被占用（内置或已注册的工具），不能影子化；请换一个名字"
        ));
    }
    let Some(params) = sanitize_parameters(parameters) else {
        return Err("parameters 必须是 type 为 object 的 JSON Schema".into());
    };
    let script = script.trim();
    if script.is_empty() {
        return Err("script 是必填项（PowerShell 脚本）".into());
    }
    if let Some(frag) = forbidden_fragment_in(script) {
        return Err(format!("脚本包含破坏性片段「{frag}」，已被拒绝。如确需该操作请让用户手动执行。"));
    }
    // AST 审计（阻塞入口）：别名/结构级绕过在此拦截，失败即拒绝
    let audit = crate::tools::ps_audit::audit_scripts_blocking(std::slice::from_ref(&script.to_string()))
        .and_then(|mut v| v.pop().ok_or_else(|| "审计结果为空".to_string()))?;
    if let Some(summary) = audit.summary() {
        return Err(format!("{summary}\n如确需该能力，请拆分为更小、更透明的操作。"));
    }

    let def = CustomToolDef {
        name: name.to_string(),
        description: description.trim().to_string(),
        parameters: params,
        script: script.to_string(),
        deferred,
        created_at: crate::memory::types::current_timestamp(),
    };

    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(format!("创建工具目录失败: {e}"));
    }
    let file = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&def)
        .map_err(|e| format!("序列化工具定义失败: {e}"))?;
    std::fs::write(&file, json).map_err(|e| format!("写入工具文件失败: {e}"))?;

    tool_system.register_tool(Arc::new(DynamicTool::new(def)));

    tracing::info!(
        "[CustomTools] 自建工具「{}」已{}（{}）",
        name,
        if is_update { "更新" } else { "创建" },
        file.display()
    );
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_validation() {
        assert!(is_valid_tool_name("fetch_issues"));
        assert!(is_valid_tool_name("my-tool-2"));
        assert!(is_valid_tool_name("a"));
        assert!(!is_valid_tool_name(""));
        assert!(!is_valid_tool_name("fetch issues"));
        assert!(!is_valid_tool_name("fetch/issues"));
        assert!(!is_valid_tool_name("查天气"));
        assert!(!is_valid_tool_name("../escape"));
        assert!(!is_valid_tool_name(&"x".repeat(65)));
    }

    #[test]
    fn parameters_sanitization() {
        assert!(sanitize_parameters(&Value::Null).is_some());
        assert!(sanitize_parameters(&serde_json::json!({"type": "object", "properties": {}})).is_some());
        // 无 type 的 object 自动补 type=object
        let out = sanitize_parameters(&serde_json::json!({"properties": {}})).unwrap();
        assert_eq!(out["type"], "object");
        // 非 object 类型拒绝
        assert!(sanitize_parameters(&serde_json::json!({"type": "string"})).is_none());
        // 非 object 值拒绝
        assert!(sanitize_parameters(&serde_json::json!("oops")).is_none());
        assert!(sanitize_parameters(&serde_json::json!([1, 2])).is_none());
    }

    #[test]
    fn forbidden_fragment_detection() {
        assert!(forbidden_fragment_in("Format-Volume -DriveLetter C").is_some());
        assert!(forbidden_fragment_in("shutdown /s /t 0").is_some());
        assert!(forbidden_fragment_in("Write-Output 'hello'").is_none());
    }
}
