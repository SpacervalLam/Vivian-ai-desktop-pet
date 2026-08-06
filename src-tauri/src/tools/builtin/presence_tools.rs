//! 在场状态工具 - set_presence_state
//!
//! 让 LLM 通过工具系统自主控制在线/离线状态，替代旧的 presence_change JSON 字段。
//! 工具内部直接调用 PresenceManager::transition，与 proactive_tick 自动触发路径等效：
//! 切换成功后写入行为日志记忆 + emit presence:changed 事件给前端。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::memory::MemoryManager;
use crate::presence::{PresenceChangeReason, PresenceManager, PresenceState};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 按角色 ID 索引的 PresenceManager 注入表。
/// 在 Brain 构造时由 `register_presence_manager` 注入，工具调用时按 char_id 取出。
static PRESENCE_MANAGERS: Lazy<RwLock<std::collections::HashMap<String, Arc<PresenceManager>>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 按角色 ID 索引的 MemoryManager 注入表（用于写入 presence_log 行为日志）。
static MEMORY_MANAGERS: Lazy<RwLock<std::collections::HashMap<String, Arc<MemoryManager>>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 全局 AppHandle（lib.rs setup 阶段注入），用于 emit presence:changed 事件。
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 支持的在场状态
const SUPPORTED_PRESENCE_STATES: &[&str] = &["online", "busy", "rest", "offline"];

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 注册某角色的 PresenceManager + MemoryManager（Brain 构造时调用）
pub fn register_presence_manager(
    char_id: &str,
    pm: Arc<PresenceManager>,
    memory: Arc<MemoryManager>,
) {
    PRESENCE_MANAGERS.write().insert(char_id.to_string(), pm);
    MEMORY_MANAGERS.write().insert(char_id.to_string(), memory);
}

/// 注销某角色的 PresenceManager + MemoryManager（角色销毁时调用，目前无销毁路径，预留）
pub fn unregister_presence_manager(char_id: &str) {
    PRESENCE_MANAGERS.write().remove(char_id);
    MEMORY_MANAGERS.write().remove(char_id);
}

/// set_presence_state 工具 - LLM 主动切换在场状态
///
/// 与旧的 presence_change JSON 字段等效，但走工具系统统一路径。
/// 切换成功后写入 presence_log 记忆 + emit presence:changed 事件。
pub struct SetPresenceStateTool;

impl SetPresenceStateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SetPresenceStateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SetPresenceStateTool {
    fn name(&self) -> &str {
        "set_presence_state"
    }

    fn description(&self) -> &str {
        "Autonomously switch your presence state (online/busy/rest/offline). Call when the \
         conversation context naturally matches, e.g. you say \"I'm going to be busy\" -> busy, \
         \"I'll take a rest\" -> rest, \"I'm back\" -> online. Do not call when no switch is needed."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "自主切换在场状态（online/busy/rest/offline）。当对话上下文自然匹配时调用，\
            例如你说\"我要去忙了\" -> busy、\"我休息一下\" -> rest、\"我回来了\" -> online。\
            不需要切换时不要调用。",
            "ja" => "自発的に在席状態を切り替える（online/busy/rest/offline）。会話の文脈が自然に一致する時に呼び出す。\
            例：「これから忙しくなる」-> busy、「少し休む」-> rest、「戻ったよ」-> online。\
            切り替えが不要な場合は呼び出さない。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "description": "Target presence state",
                    "enum": SUPPORTED_PRESENCE_STATES
                }
            },
            "required": ["state"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "description": "目标在场状态",
                        "enum": SUPPORTED_PRESENCE_STATES
                    }
                },
                "required": ["state"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "description": "対象の在席状態",
                        "enum": SUPPORTED_PRESENCE_STATES
                    }
                },
                "required": ["state"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let state = match input.get("state").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return ValidationResult::failure("state 是必填项", 2),
        };
        if !SUPPORTED_PRESENCE_STATES.contains(&state.as_str()) {
            return ValidationResult::failure(
                format!("不支持的在场状态: {}（支持: {}）", state, SUPPORTED_PRESENCE_STATES.join(", ")),
                502,
            );
        }
        ValidationResult::success(Some(json!({ "state": state })))
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 自主行为，无需用户确认
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let target_str = args.get("state").and_then(|v| v.as_str()).unwrap_or("online");
        let target = PresenceState::from_str(target_str);
        let char_id = context.char_id.clone();

        // 取出当前角色的 PresenceManager
        let pm = {
            let map = PRESENCE_MANAGERS.read();
            match map.get(&char_id) {
                Some(pm) => pm.clone(),
                None => {
                    return ToolResult::standard_success(
                        &format!("已声明状态为 {}（未找到状态管理器，仅记录意图）", target_str),
                        Some(json!({ "state": target_str, "applied": false })),
                    );
                }
            }
        };

        // 应用状态切换（transition 内部会校验最小停留时间，未满足则返回 None）
        let event = pm.transition(target, PresenceChangeReason::LlmTrigger);

        let applied = event.is_some();
        if let Some(ref ev) = event {
            // 写入行为日志记忆（fire-and-forget）
            let memory_text = pm.memory_text(ev);
            let memory = MEMORY_MANAGERS.read().get(&char_id).cloned();
            if let Some(memory) = memory {
                let text = memory_text;
                let char_id_for_mem = char_id.clone();
                tokio::spawn(async move {
                    use crate::memory::types::MemoryType;
                    let meta = serde_json::json!({
                        "channel": "presence",
                        "speaker": char_id_for_mem,
                        "listener": char_id_for_mem,
                        "perspective": "speaker",
                    });
                    let _ = memory
                        .add_memory_with_metadata(&text, MemoryType::ShortTerm, 0.4, vec!["presence_log".to_string(), "assistant".to_string()], meta)
                        .await;
                });
            }

            // emit presence:changed 事件给前端
            if let Some(handle) = APP_HANDLE.read().as_ref() {
                let _ = handle.emit(
                    "presence:changed",
                    json!({
                        "character_id": &char_id,
                        "from": ev.from,
                        "to": ev.to,
                        "reason": ev.reason,
                    }),
                );
            }

            tracing::info!(
                "[Presence:{}] 工具触发: {} → {}",
                char_id,
                ev.from,
                ev.to
            );
        }

        let current = pm.current();
        ToolResult::standard_success(
            &format!(
                "在场状态{}: 当前为 {}",
                if applied { "已切换" } else { "未切换（最小停留时间内）" },
                current.display_zh()
            ),
            Some(json!({
                "state": current.as_str(),
                "applied": applied,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Pet
    }

    fn always_load(&self) -> bool {
        // 在场状态切换是基础能力，所有场景都应可用
        true
    }

    fn search_hint(&self) -> &str {
        "online offline busy rest presence state switch"
    }
}
