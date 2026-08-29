//! 扩展系统工具 - 剪贴板、URL、文件夹、系统信息

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::utils::process::silent_command;

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
