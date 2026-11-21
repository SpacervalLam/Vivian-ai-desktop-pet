//! 主动贴边工具：模型决定贴边就直接调用，不需要先宣告状态、也不需要用户发话。
//!
//! 与 `set_presence_state` 严格分离：那是在场语义，这是窗口动作。执行只认
//! `ToolUseContext.char_id`，模型不能指定别的角色；是否可贴边由 `DockController`
//! 的几何状态裁决，贴边之后重复调用是幂等 no-op。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

fn app_handle() -> Option<AppHandle> {
    APP_HANDLE.read().clone()
}

/// 工具是否该出现在这一轮的工具面：必须真有桌宠窗口，且当前不在贴边流程中。
/// chat-only partner 不进 `state.characters`、没有窗口，这里会挡住误注入。
pub fn can_show_dock_tool(char_id: &str) -> bool {
    let Some(app) = app_handle() else { return false; };
    app.get_webview_window(char_id).is_some() && crate::dock::can_dock(char_id)
}

pub struct DockToEdgeTool;

impl DockToEdgeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DockToEdgeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DockToEdgeTool {
    fn name(&self) -> &str {
        "dock_to_edge"
    }

    fn description(&self) -> &str {
        "Retreat to the nearest screen edge and peek from outside it. Call whenever you decide to dock: no spoken announcement and no user request are needed. If the window is not at the edge yet, it walks there first. This is a window action, not a presence state change."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "退到最近的屏幕边缘，只露半个头扒着边探头。想贴边就直接调用，不需要先说一句话、也不需要用户要求；窗口不在边缘时会先走过去。这是窗口动作，不是在场状态切换。",
            "ja" => "最寄りの画面端に退避し、外側から半分だけ顔をのぞかせる。貼り付きたいときは直接呼ぶ。事前の発言やユーザーの依頼は不要で、端にいなければ先に歩いて移動する。在席状態の変更ではない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "想躲到屏幕边上\n扒着边缘探头\n退到画面角落\n靠边待一会儿\n贴到屏幕边缘",
            "ja" => "画面の端に隠れたい\n端から顔をのぞかせる\n隅に退避したい\n端に寄りたい",
            _ => "retreat to the screen edge\npeek from the edge\nhide at the screen border\nmove over to the edge",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "properties": {}})
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        if input.as_object().is_some_and(|object| !object.is_empty()) {
            return ValidationResult::failure("dock_to_edge 不接受参数", 2);
        }
        ValidationResult::success(Some(json!({})))
    }

    async fn check_permissions(&self, _input: &Value, context: &ToolUseContext) -> PermissionResult {
        // hidden 只是省 token，不是权限边界：这里挡住没有窗口的角色。
        let has_window = match app_handle() {
            Some(app) => app.get_webview_window(&context.char_id).is_some(),
            None => false,
        };
        if context.char_id.is_empty() || !has_window {
            return PermissionResult::deny("当前角色没有可贴边的桌宠窗口");
        }
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, context: &ToolUseContext) -> ToolResult {
        // 只用上下文里的角色 ID，绝不接受模型传入的 character_id。
        if context.char_id.is_empty() {
            return ToolResult::standard_error("角色上下文缺失", None, None);
        }
        if !crate::dock::can_dock(&context.char_id) {
            return ToolResult::standard_success(
                "已经在贴边了。",
                Some(json!({ "applied": false, "docked": true })),
            );
        }
        let Some(app) = app_handle() else {
            return ToolResult::standard_error("窗口句柄不可用", None, None);
        };
        let applied = crate::commands::window::dock_to_edge(&app, &context.char_id);
        ToolResult::standard_success(
            if applied { "正在退到屏幕边缘。" } else { "这次没能贴边。" },
            Some(json!({ "applied": applied, "docked": applied })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Pet
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "dock edge peek retreat screen border 贴边 屏幕边缘 探头 扒着边"
    }
}
