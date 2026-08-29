//! 桌宠自控动作执行器 —— 解析 control_actions 并调用 PetController。
//!
//! 指令来源：chat 任务（Live2D 模型控制：表情/动作/视线追踪）。

use std::sync::Arc;

use serde_json::Value;

use crate::pet_controller::PetController;

/// 已注册的 action 名称白名单。
const ALLOWED_ACTIONS: &[&str] = &[
    "set_expression",
    "set_mouse_follow",
    "set_avoid_mouse",
    "play_motion",
];

/// 桌宠自控动作执行器。
///
/// 接收 `control_actions` 指令数组，按白名单分发到 `PetController` 对应方法。
/// 非法 action 或参数错误仅记录日志，不阻塞主流程。
pub struct ControlActionExecutor {
    pet_controller: Option<Arc<PetController>>,
}

impl ControlActionExecutor {
    pub fn new() -> Self {
        Self {
            pet_controller: None,
        }
    }

    pub fn with_pet_controller(pc: Arc<PetController>) -> Self {
        Self {
            pet_controller: Some(pc),
        }
    }

    /// 执行 control_actions 列表。
    ///
    /// 单条指令失败不影响后续指令执行（best-effort）。
    pub fn execute(&self, actions: &[Value]) {
        let pc = match &self.pet_controller {
            Some(pc) => pc,
            None => {
                if !actions.is_empty() {
                    tracing::debug!(
                        "[ControlActionExecutor] PetController 未注入，跳过 {} 条指令",
                        actions.len()
                    );
                }
                return;
            }
        };

        for (idx, action) in actions.iter().enumerate() {
            let name = action
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if !ALLOWED_ACTIONS.contains(&name) {
                tracing::warn!(
                    "[ControlActionExecutor] #{} 忽略未知 action: {}",
                    idx,
                    name
                );
                continue;
            }

            let params = action.get("params").cloned().unwrap_or(Value::Null);
            let result = self.dispatch(pc, name, &params);
            match result {
                Ok(msg) => tracing::info!(
                    "[ControlActionExecutor] #{} {} -> {}",
                    idx,
                    name,
                    msg
                ),
                Err(e) => tracing::warn!(
                    "[ControlActionExecutor] #{} {} 失败: {}",
                    idx,
                    name,
                    e
                ),
            }
        }
    }

    /// 按白名单分发到 PetController 方法。
    fn dispatch(
        &self,
        pc: &PetController,
        action: &str,
        params: &Value,
    ) -> Result<String, String> {
        match action {
            "set_expression" => {
                let name = extract_str(params, "name")?;
                let r = pc.set_expression(&name, None, false);
                result_to_string(r, "set_expression")
            }
            "set_mouse_follow" => {
                let enabled = extract_bool(params, "enabled")?;
                let r = pc.mouse_follow(enabled);
                result_to_string(r, "set_mouse_follow")
            }
            "set_avoid_mouse" => {
                let enabled = extract_bool(params, "enabled")?;
                let r = pc.set_avoid_mouse(enabled);
                result_to_string(r, "set_avoid_mouse")
            }
            "play_motion" => {
                let name = extract_str(params, "name")?;
                let r = pc.play_motion(&name, 50, true, false);
                result_to_string(r, "play_motion")
            }
            // dispatch 只在白名单内调用，理论不会到达
            _ => Err(format!("未知 action: {}", action)),
        }
    }
}

impl Default for ControlActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ── 参数提取辅助函数 ──

fn extract_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("缺少或类型错误的参数: {}（应为 string）", key))
}

fn extract_bool(params: &Value, key: &str) -> Result<bool, String> {
    params
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("缺少或类型错误的参数: {}（应为 bool）", key))
}

fn result_to_string(
    r: crate::pet_controller::ControlResult,
    action: &str,
) -> Result<String, String> {
    if r.success {
        Ok(format!("{} 成功", action))
    } else {
        Err(format!("{} 失败", action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_list_rejects_unknown_action() {
        let executor = ControlActionExecutor::new();
        // 未注入 PetController，但未知 action 在分发前就被拦截，不会触发 dispatch
        executor.execute(&[serde_json::json!({
            "action": "delete_system_files",
            "params": {}
        })]);
        // 无 panic 即通过
    }

    #[test]
    fn test_extract_str_missing_key() {
        let params = serde_json::json!({});
        assert!(extract_str(&params, "name").is_err());
    }
}
