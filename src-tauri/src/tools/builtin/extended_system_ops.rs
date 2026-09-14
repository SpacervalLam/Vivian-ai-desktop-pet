//! 扩展系统工具 - 剪贴板、URL、文件夹、系统信息

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::utils::process::silent_command;

/// 全局 AppHandle（由 lib.rs setup 注入，用于读取 AppState 中的 WorldStateProvider）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// open_url 工具 - 用默认浏览器打开 URL
pub struct OpenUrlTool;

impl OpenUrlTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenUrlTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for OpenUrlTool {
    fn name(&self) -> &str {
        "open_url"
    }

    fn description(&self) -> &str {
        "Open the specified URL in the default browser."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在默认浏览器中打开指定的 URL。",
            "ja" => "デフォルトブラウザで指定された URL を開く。",
            _ => self.description(),
        }
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Open local files or folders (use open_folder instead)",
            "Launch applications (use open_application instead)",
            "Search the web (first use open_url to open a search engine, or instruct the user to search themselves)",
        ]
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "帮我打开这个链接\n打开这个网址\n访问一下这个地址",
            "en" => "open this link\ngo to this URL\nvisit this address",
            "ja" => "このリンクを開いて\nこのURLを開いて\nこのアドレスへ",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to open"}
            },
            "required": ["url"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "要打开的 URL"}
                },
                "required": ["url"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "開く URL"}
                },
                "required": ["url"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return ValidationResult::failure("url 是必填项且不能为空", 2),
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ValidationResult::failure("url 必须以 http:// 或 https:// 开头", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        PermissionResult::ask("打开浏览器需要用户确认")
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");

        #[cfg(target_os = "windows")]
        {
            match silent_command("explorer").arg(url).spawn() {
                Ok(_) => {
                    return ToolResult::standard_success(
                        &format!("已打开 URL: {url}"),
                        Some(json!({ "url": url })),
                    );
                }
                Err(e) => {
                    return ToolResult::standard_error(
                        &format!("打开 URL 失败: {e}"),
                        None,
                        None,
                    );
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = url;
            ToolResult::standard_error("当前平台不支持打开 URL", None, None)
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "open URL link"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Network
    }
}

/// get_active_window 工具 - 获取当前活动窗口标题
pub struct GetActiveWindowTool;

impl GetActiveWindowTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetActiveWindowTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetActiveWindowTool {
    fn name(&self) -> &str {
        "get_active_window"
    }

    fn description(&self) -> &str {
        "Get the title of the current foreground active window."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取当前前台活动窗口的标题。",
            "ja" => "現在のフォアグラウンドアクティブウィンドウのタイトルを取得する。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我现在开着什么\n当前窗口是哪个\n看看活动窗口",
            "en" => "what window is active\nwhich window is focused\nwhat's currently open",
            "ja" => "今アクティブなウィンドウは\n前面のウィンドウは\n今開いてるのは",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({"type": "object"}),
            "ja" => json!({"type": "object"}),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, _input: &Value, _context: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
        #[cfg(target_os = "windows")]
        {
            let script = r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32 {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
}
"@
$h = [Win32]::GetForegroundWindow()
$sb = New-Object System.Text.StringBuilder 256
[Win32]::GetWindowText($h, $sb, 256) | Out-Null
Write-Output $sb.ToString()
"#;
            match silent_command("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .output()
            {
                Ok(o) if o.status.success() => {
                    let title = String::from_utf8_lossy(&o.stdout)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    return ToolResult::standard_success(
                        "获取活动窗口成功",
                        Some(json!({ "title": title })),
                    );
                }
                _ => {
                    return ToolResult::standard_error("获取活动窗口失败", None, None);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            ToolResult::standard_error("当前平台不支持获取活动窗口", None, None)
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "get current active window"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}

/// get_memory_usage 工具 - 查询系统内存占用概况与占用内存最高的进程/应用
///
/// 用户问"内存谁占的 / 电脑怎么这么卡 / 怎么优化内存"时，LLM 调用本工具
/// 拿到总览 + 按可执行名聚合的 Top 内存进程，才能给出具体优化建议。
/// 进程枚举按需执行（几十到几百毫秒），只读、无副作用。
pub struct GetMemoryUsageTool;

impl GetMemoryUsageTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetMemoryUsageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetMemoryUsageTool {
    fn name(&self) -> &str {
        "get_memory_usage"
    }

    fn description(&self) -> &str {
        "Query overall memory usage and the top processes/apps consuming memory (aggregated by executable name, e.g. chrome.exe with N processes). Use when the user asks who is eating memory, why the computer feels slow, or how to free up memory."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查询系统内存占用概况与占用内存最高的进程/应用（按可执行名聚合，如 chrome.exe 共 N 个进程）。用户问内存谁占的、电脑为什么卡、怎么释放内存时使用。",
            "ja" => "システムのメモリ使用量と、メモリを多く消費しているプロセス/アプリを調べる（実行ファイル名で集計、例：chrome.exe が N プロセス）。ユーザーがメモリの使用状況や動作が遅い原因、メモリ解放の方法を尋ねたときに使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "内存谁占的\n谁在吃内存\n电脑怎么这么卡\n内存占用高的应用有哪些\n内存快满了怎么办\n帮我看看内存\n怎么释放内存",
            "en" => "what's eating my memory\nwhy is my computer so slow\nwhich apps use the most memory\nmemory usage\nhow to free up RAM",
            "ja" => "メモリの使用量が多い\nなんでパソコンが重いの\nメモリをたくさん使っているアプリ\nメモリを解放したい",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _context: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 只读查询，无副作用，不需要用户确认
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
        let handle = APP_HANDLE.read().clone();
        let Some(handle) = handle else {
            return ToolResult::standard_error("AppHandle 未注入，无法读取系统信息", None, None);
        };
        let Some(state) = handle.try_state::<Arc<AppState>>() else {
            return ToolResult::standard_error("AppState 未初始化", None, None);
        };

        // 总览取 10s 轮询缓存（近乎免费）；进程明细按需枚举一次
        let (metrics, top) = (
            state.world_provider.system_metrics(),
            state.world_provider.top_memory_processes(10),
        );
        let Some(m) = metrics else {
            return ToolResult::standard_error("系统指标暂不可用", None, None);
        };

        let total_gb = m.memory_total as f64 / 1024.0 / 1024.0 / 1024.0;
        let used_gb = m.memory_used as f64 / 1024.0 / 1024.0 / 1024.0;
        let mut summary = format!(
            "内存占用 {:.0}%（已用 {:.1}GB / 总量 {:.1}GB），CPU 占用 {:.0}%",
            m.memory_usage_pct, used_gb, total_gb, m.cpu_usage
        );
        if !top.is_empty() {
            let list = top
                .iter()
                .map(|p| {
                    format!(
                        "{}（{}个进程）{:.1}GB",
                        p.name,
                        p.process_count,
                        p.total_memory_bytes as f64 / 1024.0 / 1024.0 / 1024.0
                    )
                })
                .collect::<Vec<_>>()
                .join("、");
            summary.push_str(&format!("。内存占用最高的应用：{}", list));
        }

        ToolResult::standard_success(
            &summary,
            Some(json!({
                "memory_usage_pct": m.memory_usage_pct,
                "memory_used_bytes": m.memory_used,
                "memory_total_bytes": m.memory_total,
                "cpu_usage_pct": m.cpu_usage,
                "top_processes": top.iter().map(|p| json!({
                    "name": p.name,
                    "process_count": p.process_count,
                    "total_memory_bytes": p.total_memory_bytes,
                    "peak_memory_bytes": p.peak_memory_bytes,
                })).collect::<Vec<_>>(),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起（用户不问内存就不占上下文）
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "memory usage top processes ram"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}
