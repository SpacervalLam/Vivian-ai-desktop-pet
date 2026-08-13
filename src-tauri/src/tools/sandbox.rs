//! 工具沙箱 - 为破坏性工具提供安全边界
//!
//! 功能：
//! - 阻止危险命令（rm -rf、format 等）
//! - 路径校验，防止路径穿越
//! - 按保护模式分级检查
//! - 工具风险等级评估

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::ToolUseContext;

/// 工具风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolRiskLevel {
    /// 安全：只读操作
    Safe,
    /// 低风险：创建文件等可逆操作
    Low,
    /// 中风险：修改文件等半可逆操作
    Medium,
    /// 高风险：删除文件等部分不可逆操作
    High,
    /// 极高风险：执行命令等完全不可控操作
    Critical,
}

impl Default for ToolRiskLevel {
    fn default() -> Self {
        ToolRiskLevel::Safe
    }
}

/// 保护模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionMode {
    /// 谨慎：首次危险操作会担忧，前几次需要确认
    Cautious,
    /// 宽松：仅高风险工具需要确认
    Permissive,
    /// 严格：所有危险操作都需要确认
    Strict,
}

impl Default for ProtectionMode {
    fn default() -> Self {
        ProtectionMode::Cautious
    }
}

/// 工具安全配置
#[derive(Debug, Clone)]
pub struct ToolSafetyProfile {
    pub risk_level: ToolRiskLevel,
    pub requires_first_time_warning: bool,
    pub requires_confirmation: bool,
    pub pet_worries: bool,
}

impl ToolSafetyProfile {
    fn new(risk_level: ToolRiskLevel) -> Self {
        Self {
            risk_level,
            requires_first_time_warning: false,
            requires_confirmation: false,
            pet_worries: false,
        }
    }
}

/// 内置工具安全配置
fn builtin_safety_profiles() -> HashMap<&'static str, ToolSafetyProfile> {
    let mut m = HashMap::new();

    // 安全工具
    for name in [
        "read_file",
        "list_directory",
        "search_files",
        "grep",
        "take_screenshot",
        "screenshot_analyze",
        "web_search",
        "share_link",
    ] {
        m.insert(name, ToolSafetyProfile::new(ToolRiskLevel::Safe));
    }

    // 低风险工具
    let mut write_file = ToolSafetyProfile::new(ToolRiskLevel::Low);
    write_file.requires_first_time_warning = true;
    write_file.requires_confirmation = true;
    m.insert("write_file", write_file);

    let mut open_app = ToolSafetyProfile::new(ToolRiskLevel::Low);
    open_app.requires_confirmation = false;
    m.insert("open_application", open_app);

    // 中风险工具
    let mut edit_file = ToolSafetyProfile::new(ToolRiskLevel::Medium);
    edit_file.requires_first_time_warning = true;
    edit_file.requires_confirmation = true;
    m.insert("edit_file", edit_file);

    // 高风险工具
    let mut close_app = ToolSafetyProfile::new(ToolRiskLevel::High);
    close_app.requires_confirmation = true;
    close_app.pet_worries = true;
    m.insert("close_application", close_app);

    m
}

/// 危险命令模式（正则）
static DANGEROUS_COMMAND_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)\brm\s+(-[a-zA-Z]*r[a-zA-Z]*\s+-[a-zA-Z]*f[a-zA-Z]*|-[a-zA-Z]*f[a-zA-Z]*\s+-[a-zA-Z]*r[a-zA-Z]*|-rf?|-fr)\b").unwrap(),
        Regex::new(r"(?i)\brm\s+(-[a-zA-Z]*r[a-zA-Z]*\s+|--recursive\s+)").unwrap(),
        Regex::new(r"(?i)\bformat\s+[a-zA-Z]:").unwrap(),
        Regex::new(r"(?i)\bshutdown\b").unwrap(),
        Regex::new(r"(?i)\breboot\b").unwrap(),
        Regex::new(r"(?i)\bdel\s+/[fsq]").unwrap(),
        Regex::new(r"(?i)\brmdir\s+/s").unwrap(),
        Regex::new(r"(?i)\bmkfs\b").unwrap(),
        Regex::new(r"(?i)\bdd\b.*if=").unwrap(),
        Regex::new(r"(?i):\(\)\s*\{.*\};").unwrap(),
        Regex::new(r"(?i)>\s*/dev/(null|zero|sda)").unwrap(),
        Regex::new(r"(?i)\bchmod\s+-R\s+777\b").unwrap(),
        Regex::new(r"(?i)\|.*\b(sh|bash|zsh|cmd|powershell)\b").unwrap(),
        Regex::new(r"(?i)`[^`]+`").unwrap(),
        Regex::new(r"(?i)\$\([^)]+\)").unwrap(),
        Regex::new(r"(?i)wget\s+.*\|\s*(sh|bash)").unwrap(),
        Regex::new(r"(?i)curl\s+.*\|\s*(sh|bash)").unwrap(),
        Regex::new(r"(?i)\btaskkill\s+/F\b").unwrap(),
    ]
});

/// 路径穿越模式
static PATH_TRAVERSAL_PATTERNS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\.\.[\\/]").unwrap());

/// 安全检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyResult {
    pub allowed: bool,
    pub risk_level: ToolRiskLevel,
    pub requires_confirmation: bool,
    pub warning: String,
    pub pet_message: Option<String>,
    pub message: String,
}

impl SafetyResult {
    pub fn allowed(risk_level: ToolRiskLevel, message: impl Into<String>) -> Self {
        Self {
            allowed: true,
            risk_level,
            requires_confirmation: false,
            warning: String::new(),
            pet_message: None,
            message: message.into(),
        }
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self {
            allowed: false,
            risk_level: ToolRiskLevel::Critical,
            requires_confirmation: false,
            warning: String::new(),
            pet_message: None,
            message: message.into(),
        }
    }

    pub fn needs_confirmation(
        risk_level: ToolRiskLevel,
        warning: impl Into<String>,
        pet_message: Option<String>,
    ) -> Self {
        Self {
            allowed: false,
            risk_level,
            requires_confirmation: true,
            warning: warning.into(),
            pet_message,
            message: "操作需要用户确认".to_string(),
        }
    }
}

/// 工具沙箱
pub struct ToolSandbox {
    inner: RwLock<SandboxInner>,
}

struct SandboxInner {
    protection_mode: ProtectionMode,
    /// 工具使用次数（用于首次检测）
    tool_usage: HashMap<String, u32>,
    /// 自定义安全配置
    custom_profiles: HashMap<String, ToolSafetyProfile>,
    /// 内置安全配置
    builtin_profiles: HashMap<&'static str, ToolSafetyProfile>,
}

impl ToolSandbox {
    pub fn new(protection_mode: ProtectionMode, _undo_expiry_secs: u64) -> Self {
        Self {
            inner: RwLock::new(SandboxInner {
                protection_mode,
                tool_usage: HashMap::new(),
                custom_profiles: HashMap::new(),
                builtin_profiles: builtin_safety_profiles(),
            }),
        }
    }

    /// 设置保护模式
    pub fn set_protection_mode(&self, mode: ProtectionMode) {
        self.inner.write().protection_mode = mode;
    }

    /// 获取保护模式
    pub fn protection_mode(&self) -> ProtectionMode {
        self.inner.read().protection_mode
    }

    /// 注册自定义安全配置
    pub fn register_custom_profile(&self, tool_name: &str, profile: ToolSafetyProfile) {
        self.inner
            .write()
            .custom_profiles
            .insert(tool_name.to_string(), profile);
    }

    /// 获取工具安全配置
    pub fn get_safety_profile(&self, tool_name: &str) -> Option<ToolSafetyProfile> {
        let inner = self.inner.read();
        inner
            .custom_profiles
            .get(tool_name)
            .cloned()
            .or_else(|| inner.builtin_profiles.get(tool_name).cloned())
    }

    /// 检查工具安全性
    ///
    /// 执行流程：
    /// 1. 危险命令检查（针对 bash/execute_code 等）
    /// 2. 路径穿越检查（针对文件操作工具）
    /// 3. 风险等级评估与首次使用检查
    pub fn check_tool_safety(
        &self,
        tool_name: &str,
        args: &Value,
        context: Option<&ToolUseContext>,
    ) -> SafetyResult {
        // 1. 危险命令检查
        if let Some(cmd) = extract_command(args) {
            if is_dangerous_command(&cmd) {
                return SafetyResult::denied(format!(
                    "检测到危险命令被沙箱拦截: {}",
                    tool_name
                ));
            }
        }

        // 2. 路径穿越检查（search_files 豁免工作目录约束：其语义就是跨目录查找）
        let enforce_working_dir = tool_name != "search_files";
        for path in extract_paths(args) {
            if !is_path_safe(&path) {
                return SafetyResult::denied(format!(
                    "路径包含穿越序列被沙箱拦截: {}",
                    path
                ));
            }
            if enforce_working_dir {
                if let Some(c) = context {
                    if !is_path_within_working_directory(&path, &c.working_directory) {
                        return SafetyResult::denied(format!(
                            "路径不在工作目录中: {} (工作目录: {})",
                            path, c.working_directory
                        ));
                    }
                }
            }
        }

        // 3. 风险评估
        let profile = match self.get_safety_profile(tool_name) {
            Some(p) => p,
            None => {
                // 未注册内置档案的工具：通用检查（危险命令/路径穿越）已在上方完成，
                // 此处放行，风险分级交由下游权限系统（access_level × risk 矩阵 +
                // always_allow/deny 规则 + 用户确认）统一管理
                return SafetyResult::allowed(
                    ToolRiskLevel::Medium,
                    format!("工具 {} 无内置安全档案，由权限系统接管", tool_name),
                );
            }
        };

        let mut inner = self.inner.write();
        let count = inner.tool_usage.entry(tool_name.to_string()).or_insert(0);
        *count += 1;
        let usage_count = *count;
        let protection_mode = inner.protection_mode;
        drop(inner);

        // 首次使用警告
        if profile.requires_first_time_warning && usage_count == 1 {
            let warning = generate_first_time_warning(tool_name, profile.risk_level);
            let pet_message = if profile.pet_worries {
                Some(warning.clone())
            } else {
                None
            };
            return SafetyResult::needs_confirmation(
                profile.risk_level,
                warning,
                pet_message,
            );
        }

        // 需要确认
        if profile.requires_confirmation {
            let needs = match protection_mode {
                ProtectionMode::Permissive => profile.risk_level >= ToolRiskLevel::High,
                ProtectionMode::Cautious => usage_count <= 3 || profile.risk_level >= ToolRiskLevel::High,
                ProtectionMode::Strict => true,
            };
            if needs {
                let pet_message = if profile.pet_worries {
                    Some(generate_worry_message(tool_name))
                } else {
                    None
                };
                return SafetyResult::needs_confirmation(
                    profile.risk_level,
                    "操作需要用户确认".to_string(),
                    pet_message,
                );
            }
        }

        SafetyResult::allowed(profile.risk_level, "安全检查通过")
    }
}

/// 从工具参数中递归提取所有看起来像命令的字符串
fn extract_command(args: &Value) -> Option<String> {
    let mut cmds = Vec::new();
    collect_commands_recursive(args, &mut cmds);
    if cmds.is_empty() {
        None
    } else {
        Some(cmds.join(" ; "))
    }
}

fn collect_commands_recursive(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "command" || k == "cmd" || k == "script" || k == "shell" {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                }
                collect_commands_recursive(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_commands_recursive(v, out);
            }
        }
        _ => {}
    }
}

/// 判断字符串是否像文件路径
fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() < 2 {
        return false;
    }
    let s_lower = s.to_lowercase();
    if s_lower.starts_with("http://") || s_lower.starts_with("https://") {
        return false;
    }
    if s.contains("://") {
        return false;
    }
    let ch0 = s.chars().next().unwrap();
    if (ch0.is_ascii_alphabetic() && s.len() >= 2 && s.as_bytes()[1] == b':')
        || s.starts_with('/')
        || s.starts_with('\\')
        || s.starts_with('.')
        || s.starts_with('~')
        || s.contains("\\")
        || s.contains("/")
    {
        return !s.chars().any(|c| c == '\n' || c == '\r');
    }
    if s_lower.ends_with(".exe")
        || s_lower.ends_with(".dll")
        || s_lower.ends_with(".txt")
        || s_lower.ends_with(".json")
        || s_lower.ends_with(".rs")
        || s_lower.ends_with(".py")
        || s_lower.ends_with(".js")
        || s_lower.ends_with(".ts")
        || s_lower.ends_with(".md")
        || s_lower.ends_with(".toml")
        || s_lower.ends_with(".yaml")
        || s_lower.ends_with(".yml")
        || s_lower.ends_with(".png")
        || s_lower.ends_with(".jpg")
        || s_lower.ends_with(".wav")
        || s_lower.ends_with(".mp3")
    {
        return true;
    }
    false
}

/// 从工具参数中递归提取所有路径字符串
fn extract_paths(args: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    collect_paths_recursive(args, &mut paths);
    paths
}

fn collect_paths_recursive(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let k_lower = k.to_lowercase();
                let is_path_key = k_lower.contains("path")
                    || k_lower.contains("file")
                    || k_lower.contains("dir")
                    || k_lower.contains("directory")
                    || k_lower.contains("src")
                    || k_lower.contains("dst")
                    || k_lower.contains("source")
                    || k_lower.contains("dest")
                    || k_lower.contains("target")
                    || k_lower.contains("output")
                    || k_lower.contains("input")
                    || k_lower.contains("save")
                    || k_lower.contains("location");
                if is_path_key {
                    if let Some(s) = v.as_str() {
                        if looks_like_path(s) {
                            out.push(s.to_string());
                        }
                    }
                }
                collect_paths_recursive(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_paths_recursive(v, out);
            }
        }
        Value::String(s) => {
            if looks_like_path(s) && s.contains("..") {
                out.push(s.to_string());
            }
        }
        _ => {}
    }
}

/// 检查命令是否危险
pub fn is_dangerous_command(command: &str) -> bool {
    for pattern in DANGEROUS_COMMAND_PATTERNS.iter() {
        if pattern.is_match(command) {
            return true;
        }
    }
    false
}

/// 检查路径是否安全（无穿越序列）
pub fn is_path_safe(path: &str) -> bool {
    if PATH_TRAVERSAL_PATTERNS.is_match(path) {
        return false;
    }
    // 同时用 Path 组件检查
    let p = Path::new(path);
    !p.components().any(|c| matches!(c, Component::ParentDir))
}

/// 检查路径是否在工作目录内
pub fn is_path_within_working_directory(path: &str, working_directory: &str) -> bool {
    let normalized_target = normalize_path_buf(PathBuf::from(path));
    let normalized_base = normalize_path_buf(PathBuf::from(working_directory));
    normalized_target.starts_with(&normalized_base)
}

/// 规范化路径（展开 `.`，移除 `..`，统一为绝对路径风格）
fn normalize_path_buf(p: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn generate_first_time_warning(tool_name: &str, risk_level: ToolRiskLevel) -> String {
    match risk_level {
        ToolRiskLevel::Critical => format!(
            "主人！这个工具 ({}) 有点危险哦！我是第一次用，会很小心小心的...",
            tool_name
        ),
        ToolRiskLevel::High => format!(
            "主人，这个操作 ({}) 可能会造成不可逆的影响哦...真的要继续吗？",
            tool_name
        ),
        ToolRiskLevel::Medium => format!(
            "主人，这个操作 ({}) 我会很小心地做的哦！有什么不对随时告诉我~",
            tool_name
        ),
        _ => format!("第一次使用 {}，我会小心的~", tool_name),
    }
}

fn generate_worry_message(tool_name: &str) -> String {
    match tool_name {
        "close_application" => "要关掉程序了...里面的数据没保存会不会有问题呀...".to_string(),
        "edit_file" => "要修改文件了...主人确定要改这里吗？".to_string(),
        _ => "这个操作让我有点担心呢...主人确定吗？".to_string(),
    }
}

/// 创建默认的沙箱实例
pub fn default_sandbox() -> Arc<ToolSandbox> {
    Arc::new(ToolSandbox::new(ProtectionMode::Cautious, 600))
}
