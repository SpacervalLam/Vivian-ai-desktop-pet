//! 宠物行为基础设施 - 动作请求队列 + toggle_watch_mode 工具
//!
//! push_action / drain_pending_actions 是 Rust 工具层与 Live2D 前端之间的动作桥接，
//! emotion.rs / auto_trigger.rs / commands/engine.rs 等均依赖此队列。

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 表情/动作请求 - 由引擎消费
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetActionRequest {
    pub kind: String,
    pub target: String,
    pub params: Value,
    pub timestamp: i64,
    /// 目标角色 ID（多角色架构下前端按此过滤）
    #[serde(default)]
    pub character_id: String,
}

/// 全局动作请求队列，引擎可订阅消费
static PENDING_ACTIONS: once_cell::sync::Lazy<Arc<RwLock<Vec<PetActionRequest>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(Vec::new())));

/// 全局 AppHandle（由 lib.rs setup 注入，用于 emit `pet:action_pending` 事件给前端）
static APP_HANDLE: once_cell::sync::Lazy<RwLock<Option<AppHandle>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// PENDING_ACTIONS 容量上限，防止异常累积导致内存增长
const PENDING_ACTIONS_MAX: usize = 100;

/// 注入 AppHandle（lib.rs setup 调用）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 推送动作请求到队列，并 emit `pet:action_pending` 事件触发前端一次性 drain
///
/// 容量保护：超过 `PENDING_ACTIONS_MAX` 时丢弃最旧请求，避免异常累积。
pub fn push_action(req: PetActionRequest) {
    let char_id = req.character_id.clone();
    {
        let mut q = PENDING_ACTIONS.write();
        if q.len() >= PENDING_ACTIONS_MAX {
            q.remove(0);
        }
        q.push(req);
    }
    // 通知前端立即消费（前端仍保留 600ms 兜底轮询防丢事件）
    if let Some(handle) = APP_HANDLE.read().as_ref() {
        let _ = handle.emit("pet:action_pending", json!({ "character_id": char_id }));
    }
}

/// 取出所有待处理的动作请求（可选按角色过滤）
pub fn drain_pending_actions(character_id: Option<&str>) -> Vec<PetActionRequest> {
    let mut q = PENDING_ACTIONS.write();
    if let Some(cid) = character_id {
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *q)
            .into_iter()
            .partition(|r| r.character_id == cid);
        *q = others;
        mine
    } else {
        std::mem::take(&mut *q)
    }
}

fn make_request(kind: &str, target: &str, params: Value, character_id: &str) -> PetActionRequest {
    PetActionRequest {
        kind: kind.to_string(),
        target: target.to_string(),
        params,
        timestamp: chrono::Utc::now().timestamp(),
        character_id: character_id.to_string(),
    }
}

// ==================== toggle_watch_mode ====================

/// 按角色索引的注视模式状态（消除跨角色干扰）
///
/// true=关注用户（视线跟随），false=发呆/走神模式
static WATCH_MODE: once_cell::sync::Lazy<RwLock<std::collections::HashMap<String, bool>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// toggle_watch_mode 工具 - 切换注视模式
///
/// 更新全局状态并推送请求到引擎。
pub struct ToggleWatchModeTool;

impl ToggleWatchModeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToggleWatchModeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ToggleWatchModeTool {
    fn name(&self) -> &str {
        "toggle_watch_mode"
    }

    fn description(&self) -> &str {
        "Enable/disable gaze-following mode. Enable=focus on user, disable=distracted/daydreaming mode."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "启用/禁用视线跟随模式。启用=关注用户，禁用=分心/发呆模式。",
            "ja" => "視線追従モードを有効/無効にする。有効=ユーザーに注目、無効=気散り/ぼんやりモード。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "active": {
                    "type": "boolean",
                    "description": "Whether to enable gaze following: true=focus on user, false=distracted mode"
                }
            },
            "required": ["active"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "active": {
                        "type": "boolean",
                        "description": "是否启用视线跟随：true=关注用户，false=分心模式"
                    }
                },
                "required": ["active"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "active": {
                        "type": "boolean",
                        "description": "視線追従を有効にするか：true=ユーザーに注目、false=気散りモード"
                    }
                },
                "required": ["active"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        match input.get("active").and_then(|v| v.as_bool()) {
            Some(active) => ValidationResult::success(Some(json!({ "active": active }))),
            None => ValidationResult::failure("active 是必填项", 2),
        }
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let active = args.get("active").and_then(|v| v.as_bool()).unwrap_or(false);

        // 更新当前角色的注视模式状态
        WATCH_MODE.write().insert(context.char_id.clone(), active);
        // 推送动作请求到队列，由引擎执行实际的鼠标跟随切换
        push_action(make_request(
            "watch_mode",
            "toggle",
            json!({ "active": active }),
            &context.char_id,
        ));

        ToolResult::standard_success(
            &format!("视线跟随已{}", if active { "启用" } else { "禁用" }),
            Some(json!({ "active": active })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Pet
    }
}
