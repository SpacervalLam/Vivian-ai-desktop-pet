//! 工具沙箱 - 为破坏性工具提供安全边界
//!
//! 功能：
//! - 阻止危险命令（rm -rf、format 等）
//! - 路径校验，防止路径穿越
//! - 按保护模式分级检查
//! - 工具风险等级评估

use std::collections::HashMap;
use std::path::{Component, Path};
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
        // 1. 危险命令检查（命令文本在 2.5 复用，故先取出来）
        let command = extract_command(args);
        if let Some(cmd) = &command {
            if is_dangerous_command(cmd) {
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
                    // 多工作区：主工作区或任一附加目录内都算授权。
                    // 判定走 ToolUseContext::is_path_authorized，与各工具的 validate_input、
                    // 权限层的归属检查共用同一口径；主工作区为空串时此处恒通过（无目录沙箱）。
                    if !c.is_path_authorized(&path) {
                        let primary = if c.working_directory.is_empty() {
                            "未绑定"
                        } else {
                            c.working_directory.as_str()
                        };
                        return SafetyResult::denied(format!(
                            "路径不在任何已授权工作区中: {} (主工作区: {primary})",
                            path
                        ));
                    }
                }
            }
        }

        // 2.5 命令文本里的**字面绝对路径**必须落在授权工作区内。
        //
        // 为什么需要单独一条：shell 是路径校验的天然缺口——命令是一段不透明程序，
        // 参数里没有路径键（`command` / `cmd` 不是 path-ish 键名），`extract_paths`
        // 抓不到命令内容。所以工作区边界对 `run_command` 一直是"建议性"的：
        // 进程 cwd 虽已绑到工作区（相对路径受限），但模型随手写个 `type D:\other\x.txt`
        // 就能绕出去。
        //
        // 这是**尽力而为**的检测，不是安全边界：路径可以动态拼装
        // （`$p = 'D:'; Get-Content "$p\other\x.txt"`），真要封死需要 OS 级约束
        // （Job Object / 受限令牌 / AppContainer）。本层的目标是挡住「意外越界」，
        // 并给出可操作的补救路径——把该目录挂成会话的附加工作区。
        //
        // 只对**工作会话**生效：陪伴侧没有"声明过的工作区"（其 working_directory 只是
        // 进程 cwd），拿它当边界属于凭空收紧，会误伤用户正常的跨目录操作。
        if let (Some(cmd), Some(c)) = (&command, context) {
            if c.is_work_agent() && !c.working_directory.is_empty() {
                for path in extract_literal_absolute_paths(cmd) {
                    if !c.is_path_authorized(&path) {
                        return SafetyResult::denied(format!(
                            "命令引用了工作区之外的绝对路径: {path}（主工作区: {}）。\
                             需要访问该目录时，请用户在工作页把它挂为会话的附加工作区，\
                             或改用工作区内的相对路径。",
                            c.working_directory
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

/// 引号感知的 shell 词法切分——引号内的空白不切分。
///
/// 必须做对引号，否则 `"G:\my project\a.txt"` 会被切成 `G:\my` + `project\a.txt`，
/// 而 `G:\my` 落在工作区 `G:\my project` **之外**，会把合法命令误判成越界。
/// 未闭合的引号按"到行尾"处理（命令本就畸形，交给执行阶段报错）。
fn split_shell_tokens(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                    if !cur.trim().is_empty() {
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                } else {
                    cur.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    if !cur.trim().is_empty() {
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                    quote = Some(ch);
                } else if ch.is_whitespace()
                    || matches!(ch, ';' | '|' | ',' | '(' | ')' | '{' | '}' | '[' | ']' | '`')
                {
                    if !cur.trim().is_empty() {
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                } else {
                    cur.push(ch);
                }
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// 从命令行文本里提取**字面绝对路径**（盘符绝对 `C:\…` / `C:/…` / UNC `\\server\share`）。
///
/// 刻意只认这两种形态，不猜"看起来像路径"的相对串：
/// - 相对路径已被进程 cwd（绑在工作区）约束住，无需再判；
/// - `..` 穿越已由 [`is_path_safe`] 在上一步拦下；
/// - 把 `/xxx` 之类也当绝对路径会和 PowerShell 开关（`/silent`）混淆。
///
/// 覆盖不到动态拼装的路径——本函数是「尽力而为」的意外越界检测，不是安全边界，
/// 调用方（`check_tool_safety`）的注释里写明了这一限制与真正的解法。
fn extract_literal_absolute_paths(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in split_shell_tokens(command) {
        let t = token.trim().trim_matches(|c: char| matches!(c, '\'' | '"' | '`'));
        let b = t.as_bytes();
        let drive_absolute = b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/');
        let unc = t.starts_with("\\\\");
        if !(drive_absolute || unc) {
            continue;
        }
        if !out.iter().any(|p| p.eq_ignore_ascii_case(t)) {
            out.push(t.to_string());
        }
    }
    out
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

// 单根工作目录判定已移除：工作区可能是「主工作区 + 若干附加目录」的集合，
// 归属判定统一走 `tools::types::is_path_within_any`（经 `ToolUseContext::is_path_authorized`
// 与权限层共用），避免这里再长出第二套单根口径。

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn work_ctx(root: &str) -> ToolUseContext {
        ToolUseContext::new("s", "u")
            .with_working_directory(root)
            .with_agent_kind("work")
    }

    #[test]
    fn quoted_path_with_spaces_stays_one_token() {
        // 引号内的空白不能切分：否则 `G:\my` 会落在工作区 `G:\my project` 之外，
        // 把合法命令误判成越界。
        let toks = split_shell_tokens(r#"type "G:\my project\a.txt" --raw"#);
        assert!(
            toks.iter().any(|t| t == r"G:\my project\a.txt"),
            "引号内容被切碎了: {toks:?}"
        );
    }

    #[test]
    fn extracts_only_absolute_paths() {
        let got = extract_literal_absolute_paths(
            r#"Set-Location D:\other; type C:/x.txt; foo %TEMP%\a; bar /silent; type .\rel.txt"#,
        );
        assert_eq!(
            got,
            vec![r"D:\other".to_string(), "C:/x.txt".to_string()],
            "只应提取盘符绝对的路径"
        );
    }

    #[test]
    fn extracts_unc_and_dedupes_case_insensitively() {
        let got = extract_literal_absolute_paths(r#"copy \\nas\share\a.txt d:\b.txt D:\B.TXT"#);
        assert_eq!(
            got,
            vec![r"\\nas\share\a.txt".to_string(), r"d:\b.txt".to_string()],
            "UNC 要认，大小写不同的重复项要去重"
        );
    }

    #[test]
    fn shell_command_reaching_outside_workspace_is_denied() {
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = work_ctx("G:\\work");
        let r = sandbox.check_tool_safety(
            "run_command",
            &json!({ "command": "type D:\\other\\secret.txt" }),
            Some(&ctx),
        );
        assert!(!r.allowed, "工作区外的绝对路径必须被拒");
        assert!(r.message.contains("D:\\other\\secret.txt"), "{}", r.message);
    }

    #[test]
    fn shell_command_inside_workspace_passes() {
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = work_ctx("G:\\work");
        for cmd in [
            "cargo build",
            "type G:\\work\\src\\main.rs",
            // 工作区内带空格的引号路径：切词必须按引号走，否则会被误判
            r#"& "G:\work\my app\a.exe" --flag"#,
        ] {
            let r = sandbox.check_tool_safety("run_command", &json!({ "command": cmd }), Some(&ctx));
            assert!(r.allowed, "`{cmd}` 不该被拦: {}", r.message);
        }
    }

    #[test]
    fn shell_command_inside_a_spaced_workspace_passes() {
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = work_ctx("G:\\my work");
        let r = sandbox.check_tool_safety(
            "run_command",
            &json!({ "command": r#"type "G:\my work\a.txt""# }),
            Some(&ctx),
        );
        assert!(r.allowed, "带空格的工作区路径不该被误拦: {}", r.message);
    }

    #[test]
    fn companion_shell_is_not_bounded_by_process_cwd() {
        // 陪伴侧没有「声明过的工作区」，其 working_directory 只是进程 cwd。
        // 拿它当边界属于凭空收紧，会误伤用户正常的跨目录操作。
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = ToolUseContext::new("s", "u").with_working_directory("G:\\app");
        let r = sandbox.check_tool_safety(
            "run_command",
            &json!({ "command": "type D:\\photos\\list.txt" }),
            Some(&ctx),
        );
        assert!(r.allowed, "陪伴侧不应被进程 cwd 限制: {}", r.message);
    }

    #[test]
    fn no_workspace_session_has_no_path_boundary_to_enforce() {
        // 无工作区模式：没有可比的边界，这条检查不生效（写入由确认回调兜）
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = work_ctx("");
        let r = sandbox.check_tool_safety(
            "run_command",
            &json!({ "command": "type D:\\other\\x.txt" }),
            Some(&ctx),
        );
        assert!(r.allowed, "无工作区时不该由本层拒绝: {}", r.message);
    }

    #[test]
    fn traversal_in_command_is_still_caught() {
        let sandbox = ToolSandbox::new(ProtectionMode::Permissive, 600);
        let ctx = work_ctx("G:\\work");
        let r = sandbox.check_tool_safety(
            "run_command",
            &json!({ "command": r"type ..\..\outside\x.txt" }),
            Some(&ctx),
        );
        assert!(!r.allowed, "命令里的 .. 穿越仍应被拦");
    }
}
