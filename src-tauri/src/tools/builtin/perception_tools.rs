//! 桌面感知工具 - 光标位置、空闲状态、前台应用上下文
//!
//! 前台应用使用现有 Win32 感知原语，其余工具通过 PowerShell 获取信息。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
    ToolRiskTier,
};

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
        "Read app context. scope=foreground returns the actual focused window plus user_app_context: when the companion chat is focused, this is the last observed external app with its observation time and age. Describe it as the app used before chatting, never as still foreground or running; stale=true means over five minutes old. scope=running_apps lists current processes, including background recording apps. A running recording app does not prove recording is active. For screen contents use screenshot_analyze."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取应用上下文。scope=foreground 返回实际前台窗口及 user_app_context：桌宠聊天窗口位于前台时，后者为最近观察到的外部应用，附观察时间与距今秒数。回答为“切来聊天前使用的应用”，不能说它仍在前台或运行；stale=true 表示记录超过五分钟。scope=running_apps 查询当前运行进程，包括后台录屏应用，进程存在不证明正在录制。需要画面时用 screenshot_analyze。",
            "ja" => "scope=foreground で実際の前面ウィンドウと user_app_context を取得する。チャットが前面の場合、後者は最後に観測した外部アプリであり、時刻・経過秒数・stale を確認して過去の観測として説明する。scope=running_apps で現在の実行中プロセスを取得する。録画アプリの存在だけでは録画中と断定できない。画面内容は screenshot_analyze で確認する。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我正在玩什么游戏\n你知道我在玩什么吗\n看看我玩的是什么游戏\n我现在在用哪个软件\n我现在打开了什么\n看看我在用啥\n当前窗口是哪个\n看看运行中的应用\n运行中的进程\n我在录屏吗",
            "en" => "what game am I playing\ncan you tell which game I'm playing\nwhat app am I using right now\nwhich program is open",
            "ja" => "今何のゲームをしてる\n今どのアプリを使ってる\n今開いてるソフトは",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {
            "scope": {"type": "string", "enum": ["foreground", "running_apps"],
                "description": "foreground: focused window plus last external app when companion is focused; running_apps: all current processes", "default": "foreground"}
        }})
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or("foreground");
        if scope == "running_apps" {
            return list_running_apps().await;
        }
        if scope != "foreground" {
            return ToolResult::standard_error("scope 必须为 foreground 或 running_apps", Some("InvalidScope"), None);
        }
        let window = crate::world::foreground_window::get_current_foreground_window();
        foreground_context_result(window, std::process::id(), crate::world::foreground_window::last_external_app(), chrono::Utc::now().timestamp_millis())
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn always_load(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}

fn foreground_context_result(
    window: crate::world::foreground_window::ForegroundWindowSnapshot,
    companion_pid: u32,
    recent: Option<crate::world::foreground_window::ExternalAppObservation>,
    now_ms: i64,
) -> ToolResult {
    let is_companion = window.pid == companion_pid;
    let user_app_context = if window.pid != 0 && !is_companion {
        Some(json!({"window": window, "source": "current_foreground", "observed_at_unix_ms": now_ms, "age_seconds": 0, "stale": false}))
    } else {
        recent.map(|observation| {
            let age_seconds = now_ms.saturating_sub(observation.observed_at_unix_ms).max(0) / 1000;
            json!({"window": observation.window, "source": "last_observed_external_app", "observed_at_unix_ms": observation.observed_at_unix_ms,
                "age_seconds": age_seconds, "stale": age_seconds > 300})
        })
    };
    ToolResult::standard_success(
        "前台窗口与用户应用上下文",
        Some(json!({
            "title": window.title, "process": window.process, "pid": window.pid,
            "available": window.pid != 0, "is_companion_window": is_companion, "scope": "foreground",
            "user_app_context": user_app_context,
            "limitation": "last_observed_external_app 是本次运行内最近观察到的外部应用，不证明它仍在前台或运行。需确认运行情况时用 scope=running_apps；上下文为空时没有记录，不能猜测。",
        })),
    )
}

async fn list_running_apps() -> ToolResult {
    #[cfg(target_os = "windows")]
    {
        let result = tokio::task::spawn_blocking(|| {
            crate::utils::process::silent_command("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command",
                    "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); @(Get-Process | Sort-Object ProcessName, Id | Select-Object @{n='process';e={$_.ProcessName}}, @{n='pid';e={$_.Id}}, @{n='title';e={$_.MainWindowTitle}}) | ConvertTo-Json -Compress"])
                .output()
        }).await;
        match result {
            Ok(Ok(output)) if output.status.success() => match serde_json::from_slice::<Value>(&output.stdout) {
                Ok(processes) => ToolResult::standard_success("运行中的进程与窗口标题", Some(json!({
                    "scope": "running_apps", "processes": processes,
                    "limitation": "进程存在只能证明程序运行，不能确认录屏正在进行；需要画面证据时使用 screenshot_analyze。"
                }))),
                Err(e) => ToolResult::standard_error(&format!("解析进程列表失败: {e}"), Some("ProcessListFailed"), None),
            },
            Ok(Ok(output)) => ToolResult::standard_error(&format!("读取进程列表失败: {}", String::from_utf8_lossy(&output.stderr)), Some("ProcessListFailed"), None),
            other => ToolResult::standard_error(&format!("启动进程查询失败: {other:?}"), Some("ProcessListFailed"), None),
        }
    }
    #[cfg(not(target_os = "windows"))]
    ToolResult::standard_error("当前平台尚未实现进程枚举", Some("ProcessListUnsupported"), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::foreground_window::ForegroundWindowSnapshot;

    #[test]
    fn companion_chat_focus_is_reported_without_tool_failure() {
        let result = foreground_context_result(ForegroundWindowSnapshot {
            title: "Side Chat".into(), process: "vivian".into(), pid: 42,
        }, 42, None, 2000);
        assert!(result.success);
        let data = &result.data.unwrap()["data"];
        assert_eq!(data["title"], "Side Chat");
        assert_eq!(data["is_companion_window"], true);
        assert_eq!(data["available"], true);
    }

    #[test]
    fn absent_foreground_is_an_unavailable_observation_not_an_execution_failure() {
        let result = foreground_context_result(ForegroundWindowSnapshot::default(), 42, None, 2000);
        assert!(result.success);
        assert_eq!(result.data.unwrap()["data"]["available"], false);
    }

    #[test]
    fn chat_query_returns_recent_external_app_with_explicit_age_and_source() {
        let recent = crate::world::foreground_window::ExternalAppObservation {
            window: ForegroundWindowSnapshot { title: "Game".into(), process: "game".into(), pid: 7 }, observed_at_unix_ms: 1000,
        };
        let result = foreground_context_result(ForegroundWindowSnapshot { title: "Side Chat".into(), pid: 42, ..Default::default() }, 42, Some(recent.clone()), 6000);
        let payload = result.data.unwrap();
        let context = &payload["data"]["user_app_context"];
        assert_eq!(payload["data"]["title"], "Side Chat");
        assert_eq!(context["window"]["pid"], 7);
        assert_eq!(context["source"], "last_observed_external_app");
        assert_eq!(context["age_seconds"], 5);
        assert_eq!(context["stale"], false);
        let old = foreground_context_result(ForegroundWindowSnapshot::default(), 42, Some(recent), 302000);
        assert_eq!(old.data.unwrap()["data"]["user_app_context"]["stale"], true);
    }

    #[test]
    fn actual_external_foreground_takes_priority_over_history() {
        let recent = crate::world::foreground_window::ExternalAppObservation { window: ForegroundWindowSnapshot { pid: 7, ..Default::default() }, observed_at_unix_ms: 1000 };
        let result = foreground_context_result(ForegroundWindowSnapshot { pid: 8, ..Default::default() }, 42, Some(recent), 6000);
        let payload = result.data.unwrap();
        let context = &payload["data"]["user_app_context"];
        assert_eq!(context["window"]["pid"], 8);
        assert_eq!(context["source"], "current_foreground");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn running_apps_includes_the_current_background_process() {
        let result = list_running_apps().await;
        assert!(result.success, "{:?}", result.error);
        let payload = result.data.unwrap();
        let processes = payload["data"]["processes"].as_array().unwrap();
        assert!(processes.iter().any(|p| p["pid"].as_u64() == Some(std::process::id() as u64)));
        assert!(processes.iter().all(|p| p["process"].is_string() && p["title"].is_string()));
    }
}
