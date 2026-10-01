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
        "Get the current foreground window title and process name. Use this first to identify which game or app the user is using; request a screenshot only if this is inconclusive or actual visual details are needed. Window metadata alone cannot reveal gameplay or screen contents."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取当前前台应用的窗口标题和进程名。判断用户正在玩什么游戏、使用什么软件时优先调用；只有信息不足或需要查看实际画面细节时才请求截图。窗口信息不能证明游戏进度或画面内容。",
            "ja" => "前面のウィンドウタイトルとプロセス名を取得する。どのゲームやアプリを使っているかはまずこのツールで確認し、情報不足や画面の詳細が必要な場合にスクリーンショットを求める。ゲームの進行や画面内容はこの情報だけでは分からない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我正在玩什么游戏\n你知道我在玩什么吗\n看看我玩的是什么游戏\n我现在在用哪个软件\n我现在打开了什么\n看看我在用啥\n当前窗口是哪个",
            "en" => "what game am I playing\ncan you tell which game I'm playing\nwhat app am I using right now\nwhich program is open",
            "ja" => "今何のゲームをしてる\n今どのアプリを使ってる\n今開いてるソフトは",
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

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let window = crate::world::foreground_window::get_foreground_window();
        if window.pid == 0 {
            return ToolResult::standard_error(
                "当前没有可识别的外部前台应用，不能据此判断用户正在玩什么",
                Some("ForegroundAppUnavailable"),
                None,
            );
        }
        ToolResult::standard_success(
            "前台应用上下文",
            Some(json!({
                "title": window.title,
                "process": window.process,
                "pid": window.pid,
            })),
        )
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

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}
