//! Hook 事件类型定义
//!
//! 支持两种钩子事件：
//! - `PreToolUse`：工具执行前触发，可拦截（deny 阻止执行）
//! - `PostToolUse`：工具执行后触发，仅信息性（无 deny 能力）

use serde::{Deserialize, Serialize};

/// Hook 事件名称
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEventName {
    /// 工具执行前（可 deny 阻止执行）
    PreToolUse,
    /// 工具执行后（信息性，无 deny 能力）
    PostToolUse,
}

impl std::fmt::Display for HookEventName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEventName::PreToolUse => write!(f, "PreToolUse"),
            HookEventName::PostToolUse => write!(f, "PostToolUse"),
        }
    }
}

/// 传递给 Hook 脚本的事件数据（通过 stdin JSON）
#[derive(Debug, Clone, Serialize)]
pub struct HookEvent {
    /// 事件类型（"PreToolUse" / "PostToolUse"）
    pub event: String,
    /// 工具名称
    pub tool_name: String,
    /// 工具参数
    pub arguments: serde_json::Value,
    /// 会话标识
    pub session_id: String,
    /// ISO-8601 时间戳
    pub timestamp: String,
}

/// Hook 脚本的决策结果（通过 stdout JSON 或退出码）
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// 允许继续执行
    Allow,
    /// 拒绝执行（附带原因）
    Deny { reason: String },
}

impl HookDecision {
    /// 从 stdout JSON 解析决策
    pub fn from_json(json: &str) -> Self {
        let trimmed = json.trim();
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(decision) = val.get("decision").and_then(|d| d.as_str()) {
                return match decision {
                    "deny" => {
                        let reason = val
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .unwrap_or("Hook denied")
                            .to_string();
                        HookDecision::Deny { reason }
                    }
                    _ => HookDecision::Allow,
                };
            }
        }
        // 无效 JSON → fail-open（默认 allow）
        HookDecision::Allow
    }

    /// 从进程退出码解析决策
    pub fn from_exit_code(code: i32) -> Self {
        match code {
            0 => HookDecision::Allow,
            2 => HookDecision::Deny {
                reason: "Hook script exited with code 2 (deny)".to_string(),
            },
            _ => HookDecision::Allow, // 其他退出码 → fail-open
        }
    }
}
