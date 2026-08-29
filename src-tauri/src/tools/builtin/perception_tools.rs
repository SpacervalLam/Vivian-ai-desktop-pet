//! 桌面感知工具 - 光标位置、空闲状态、前台应用上下文
//!
//! 通过 PowerShell P/Invoke 调用 user32.dll 获取桌面环境感知信息。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};
use crate::utils::run_ps_async;

// ===== get_foreground_app_context =====

pub struct GetForegroundAppContextTool;

impl GetForegroundAppContextTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetForegroundAppContextTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetForegroundAppContextTool {
    fn name(&self) -> &str {
        "get_foreground_app_context"
    }

    fn description(&self) -> &str {
        "Get the foreground app context: window title and process name. Useful for understanding the user's current activity."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取前台应用上下文：窗口标题和进程名。可用于了解用户当前活动。",
            "ja" => "フォアグラウンドアプリのコンテキストを取得する：ウィンドウタイトルとプロセス名。ユーザーの現在の活動を理解するのに役立つ。",
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

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let script = r#"Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;using System.Text;public class W{[DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();[DllImport("user32.dll")]public static extern int GetWindowText(IntPtr h,StringBuilder s,int n);[DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);}';
$h = [W]::GetForegroundWindow();
$sb = New-Object System.Text.StringBuilder 512;
[W]::GetWindowText($h,$sb,512) | Out-Null;
$pid = 0;
[W]::GetWindowThreadProcessId($h,[ref]$pid) | Out-Null;
$proc = (Get-Process -Id $pid -ErrorAction SilentlyContinue).ProcessName;
"$($sb.ToString())|$proc|$pid"
"#;
        match run_ps_async(script).await {
            Ok(out) => {
                let parts: Vec<&str> = out.splitn(3, '|').collect();
                let title = parts.first().unwrap_or(&"").to_string();
                let process = parts.get(1).unwrap_or(&"").to_string();
                let pid = parts.get(2).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                ToolResult::standard_success(
                    "前台应用上下文",
                    Some(json!({
                        "title": title,
                        "process": process,
                        "pid": pid,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error("获取前台应用失败", Some(&e), None),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
}
