//! 权限系统 - 工具权限检查、文件路径权限和工作目录限制

use serde_json::Value;

use super::types::{
    normalize_path, policy_for, AgentAccessLevel, PermissionContext, PermissionMode,
    PermissionResult, Tool, ToolUseContext,
};

/// 文件工具 → 操作类型映射
fn file_operation_for(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "read_file" | "list_directory" | "search_files" | "grep" => Some("read"),
        "write_file" | "edit_file" => Some("write"),
        _ => None,
    }
}

/// 需要用户通过 toast 弹窗确认的工具列表
///
/// 这些工具涉及隐私敏感操作（读写文件、截屏、定时任务删除、待办删除），
/// 默认必须经前端用户确认后才能执行。
/// 用户仍可通过 `always_allow` / `always_deny` / `bypass` 规则覆盖。
const CONFIRMATION_REQUIRED_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "list_directory",
    "search_files",
    "grep",
    "take_screenshot",
    "screenshot_analyze",
    "cancel_scheduled",
    "delete_todo",
];

/// 工具是否在用户确认列表中
pub fn is_confirmation_required_tool(tool_name: &str) -> bool {
    CONFIRMATION_REQUIRED_TOOLS.contains(&tool_name)
}

/// 从工具参数中提取文件路径
fn extract_file_paths(tool_name: &str, args: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if matches!(tool_name, "copy_file" | "move_file") {
        for key in &["source", "destination"] {
            if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
                paths.push(v.to_string());
            }
        }
        return paths;
    }
    if tool_name == "list_directory" {
        if let Some(v) = args.get("directory").and_then(|v| v.as_str()) {
            paths.push(v.to_string());
        }
        return paths;
    }
    for key in &["file_path", "path"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            paths.push(v.to_string());
        }
    }
    paths
}

/// 检查工具权限
///
/// 综合考虑：
/// - 权限模式（Bypass 直接放行，Ask 一律询问）
/// - always_allow / always_deny / always_ask 规则
/// - 文件路径的工作目录与只读限制
pub async fn check_tool_permission(
    tool: &dyn Tool,
    args: &Value,
    context: &ToolUseContext,
    permission_context: &PermissionContext,
) -> PermissionResult {
    let tool_name = tool.name();

    // 1. Bypass 模式直接放行
    if permission_context.is_bypass_mode() {
        return PermissionResult::allow();
    }

    // 1.5 权限网关矩阵：access_level × risk 决定 allow/ask/deny
    let access = permission_context.access_level;
    let risk = tool.risk();
    let matrix_behavior = policy_for(access, risk);
    if matches!(
        matrix_behavior,
        super::types::PermissionBehavior::Deny
    ) {
        let hint = if risk == super::types::ToolRiskTier::InputControl {
            "。该操作需要输入控制权限，请在设置中将访问级别提升至 FullControl（完全控制）后重试"
        } else {
            "。可在设置中提升访问级别以解锁更高权限的工具"
        };
        return PermissionResult::deny(format!(
            "工具 '{}' 风险等级 {} 超出当前访问级别 {} 的允许范围{}",
            tool_name,
            risk.as_str(),
            access.as_str(),
            hint,
        ));
    }
    let matrix_ask = matches!(
        matrix_behavior,
        super::types::PermissionBehavior::Ask
    );

    // 2. always_deny 规则
    if matches_pattern(tool_name, &permission_context.always_deny) {
        return PermissionResult::deny(format!(
            "工具 '{}' 被规则拒绝",
            tool_name
        ));
    }

    // 3. 文件路径权限检查
    if let Some(operation) = file_operation_for(tool_name) {
        for fp in extract_file_paths(tool_name, args) {
            let result = check_file_permission(&fp, operation, permission_context);
            if result.is_denied() {
                return result;
            }
            if result.requires_confirmation() {
                return result;
            }
        }
    }

    // 4. always_ask 规则
    if matches_pattern(tool_name, &permission_context.always_ask) {
        return PermissionResult::ask(format!(
            "工具 '{}' 需要用户确认",
            tool_name
        ));
    }

    // 5. always_allow 规则
    if matches_pattern(tool_name, &permission_context.always_allow) {
        return PermissionResult::allow();
    }

    // 6. Ask 模式：一律询问
    if permission_context.is_ask_mode() {
        return PermissionResult::ask(format!(
            "工具 '{}' 在 Ask 模式下需要确认",
            tool_name
        ));
    }

    // 6.5 矩阵判定 Ask：风险等级超出访问级别直接允许范围，向用户请求确认
    //     （always_allow 显式规则已在步骤 5 返回，优先级高于矩阵判定）
    if matrix_ask {
        return PermissionResult::ask(format!(
            "工具 '{}' 风险等级 {} 在当前访问级别 {} 下需要用户确认",
            tool_name,
            risk.as_str(),
            access.as_str(),
        ));
    }

    // 7. 委托给工具自身的权限检查
    //    9 个隐私敏感工具（文件 6 + 感知 3）的 check_permissions 会返回 ask
    tool.check_permissions(args, context).await
}

/// 检查文件路径权限
pub fn check_file_permission(
    file_path: &str,
    operation: &str,
    context: &PermissionContext,
) -> PermissionResult {
    if context.is_bypass_mode() {
        return PermissionResult::allow();
    }

    let normalized = normalize_path(file_path);

    for wd in context.additional_working_directories.values() {
        let wd_normalized = normalize_path(&wd.path);
        if !normalized.starts_with(&wd_normalized) {
            continue;
        }

        if wd.is_read_only && (operation == "write" || operation == "delete") {
            return PermissionResult::deny(format!(
                "目录 '{}' 是只读的，不允许 {} 操作",
                wd.path, operation
            ));
        }

        if !wd.permissions.iter().any(|p| p == operation || p == "*") {
            return PermissionResult::ask(format!(
                "操作 '{}' 在目录 '{}' 中不被显式允许",
                operation, wd.path
            ));
        }

        return PermissionResult::allow();
    }

    // 路径不在任何工作目录中：写入/删除操作需要询问
    if operation == "write" || operation == "delete" {
        return PermissionResult::ask(format!(
            "路径 '{}' 不在已授权的工作目录中，需要确认",
            file_path
        ));
    }

    PermissionResult::allow()
}

/// 通配符匹配
fn matches_pattern(name: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if pattern_match(pattern, name) {
            return true;
        }
    }
    false
}

/// 单个 pattern 匹配（支持 `*`、`?`、`regex:` 前缀）
fn pattern_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(regex_str) = pattern.strip_prefix("regex:") {
        if let Ok(re) = regex::Regex::new(regex_str) {
            return re.is_match(value);
        }
        return false;
    }
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        return glob_match(pattern, value);
    }
    pattern == value
}

/// 简单的 glob 匹配（支持 `*` 和 `?`）
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    glob_match_inner(&p, &v)
}

fn glob_match_inner(pattern: &[char], value: &[char]) -> bool {
    let mut pi = 0;
    let mut vi = 0;
    let mut star_pi = None;
    let mut star_vi = 0;

    while vi < value.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == value[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }

    pi == pattern.len()
}

/// 创建权限上下文构建器
pub struct PermissionContextBuilder {
    context: PermissionContext,
}

impl PermissionContextBuilder {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            context: PermissionContext::new(mode),
        }
    }

    pub fn with_access_level(mut self, level: AgentAccessLevel) -> Self {
        self.context.access_level = level;
        self
    }

    pub fn with_working_directory(mut self, path: impl Into<String>, read_only: bool) -> Self {
        self.context.add_working_directory(path, read_only);
        self
    }

    pub fn allow(mut self, tool: impl Into<String>) -> Self {
        self.context.always_allow.push(tool.into());
        self
    }

    pub fn deny(mut self, tool: impl Into<String>) -> Self {
        self.context.always_deny.push(tool.into());
        self
    }

    pub fn ask(mut self, tool: impl Into<String>) -> Self {
        self.context.always_ask.push(tool.into());
        self
    }

    pub fn build(self) -> PermissionContext {
        self.context
    }
}

/// 工具是否需要权限确认
pub fn requires_permission(
    tool: &dyn Tool,
    args: &Value,
    context: &PermissionContext,
) -> bool {
    if context.is_bypass_mode() {
        return false;
    }
    // 权限网关矩阵：Deny / Ask 都需要进入权限检查流程
    let behavior = policy_for(context.access_level, tool.risk());
    if matches!(
        behavior,
        super::types::PermissionBehavior::Deny | super::types::PermissionBehavior::Ask
    ) {
        return true;
    }
    if matches_pattern(tool.name(), &context.always_deny) {
        return true;
    }
    if matches_pattern(tool.name(), &context.always_ask) {
        return true;
    }
    if matches_pattern(tool.name(), &context.always_allow) {
        return false;
    }
    if context.is_ask_mode() {
        return true;
    }
    // 9 个隐私敏感工具（文件 6 + 感知 3）始终需要进入权限检查流程
    if is_confirmation_required_tool(tool.name()) {
        return true;
    }
    // 文件写入路径不在工作目录内时也需要确认
    if let Some(op) = file_operation_for(tool.name()) {
        if op == "write" {
            for fp in extract_file_paths(tool.name(), args) {
                if !context.is_path_in_working_directory(&fp) {
                    return true;
                }
            }
        }
    }
    !tool.is_read_only()
}
