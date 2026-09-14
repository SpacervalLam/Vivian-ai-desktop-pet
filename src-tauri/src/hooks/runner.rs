//! Hook 命令执行器
//!
//! 通过 `tokio::process::Command` 执行外部 Hook 脚本：
//! - 通过 stdin 传递 JSON 事件数据
//! - 从 stdout 解析决策 JSON，或从退出码推断
//! - 超时/异常/无效 JSON → fail-open（默认 allow）

use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::config::HookSpec;
use super::event::{HookDecision, HookEvent};
use crate::utils::process::{silent_command, silent_command_async};

/// 执行单个 Hook 脚本，返回决策结果
///
/// 执行流程：
/// 1. 构造子进程，设置工作目录
/// 2. 通过 stdin 写入 JSON 事件
/// 3. 等待 stdout 输出（带超时）
/// 4. 解析决策（优先 stdout JSON，回退到退出码）
pub async fn run_hook(spec: &HookSpec, event: &HookEvent) -> HookDecision {
    let timeout = Duration::from_millis(spec.timeout_ms);

    let event_json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(
                "[HookRunner] 序列化 HookEvent 失败: {} (hook: {})",
                e,
                spec.name
            );
            return HookDecision::Allow; // fail-open
        }
    };

    // 构造命令（支持 shell 语法）
    let shell = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut child = match silent_command_async(shell.0)
        .arg(shell.1)
        .arg(&spec.command)
        .current_dir(&spec.source_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "[HookRunner] 启动 Hook 脚本失败: {} (hook: {}, command: {})",
                e,
                spec.name,
                spec.command
            );
            return HookDecision::Allow; // fail-open
        }
    };

    // 写入 stdin
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(event_json.as_bytes()).await {
            tracing::warn!(
                "[HookRunner] 写入 Hook stdin 失败: {} (hook: {})",
                e,
                spec.name
            );
        }
        drop(stdin); // 关闭 stdin 以通知脚本读取完毕
    }

    // 带超时等待
    let child_pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stderr.is_empty() {
                tracing::debug!(
                    "[HookRunner] Hook {} stderr: {}",
                    spec.name,
                    stderr.trim()
                );
            }

            // 优先从 stdout JSON 解析决策
            let stdout_trimmed = stdout.trim();
            if !stdout_trimmed.is_empty() {
                let decision = HookDecision::from_json(stdout_trimmed);
                tracing::debug!(
                    "[HookRunner] Hook {} stdout 决策: {:?}",
                    spec.name,
                    decision
                );
                return decision;
            }

            // 回退到退出码
            let code = output.status.code().unwrap_or(-1);
            let decision = HookDecision::from_exit_code(code);
            tracing::debug!(
                "[HookRunner] Hook {} 退出码 {} → {:?}",
                spec.name,
                code,
                decision
            );
            decision
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "[HookRunner] 等待 Hook 脚本完成失败: {} (hook: {})",
                e,
                spec.name
            );
            HookDecision::Allow // fail-open
        }
        Err(_) => {
            tracing::warn!(
                "[HookRunner] Hook 脚本超时 ({}ms): {} (command: {})",
                spec.timeout_ms,
                spec.name,
                spec.command
            );
            // 超时 → 通过 PID 强制终止子进程
            if let Some(pid) = child_pid {
                #[cfg(unix)]
                {
                    unsafe { libc::kill(pid as i32, libc::SIGKILL); }
                }
                #[cfg(windows)]
                {
                    if let Err(e) = silent_command("taskkill")
                        .args(["/F", "/PID", &pid.to_string()])
                        .output()
                    {
                        tracing::warn!(pid = %pid, error = %e, "[hooks] 超时后 kill 子进程失败，可能残留孤儿进程");
                    }
                }
            }
            HookDecision::Allow // fail-open
        }
    }
}

/// 批量执行匹配的 Hook，返回第一个 deny 或全部 allow
///
/// - 按配置顺序执行
/// - 第一个 deny 即返回（短路）
/// - 全部 allow 则返回 Allow
pub async fn dispatch_hooks(
    registry: &super::config::HookRegistry,
    event_name: super::event::HookEventName,
    tool_name: &str,
    arguments: &serde_json::Value,
    session_id: &str,
) -> HookDecision {
    let matching = registry.matching_hooks(event_name, tool_name);
    if matching.is_empty() {
        return HookDecision::Allow;
    }

    let event = HookEvent {
        event: event_name.to_string(),
        tool_name: tool_name.to_string(),
        arguments: arguments.clone(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    for spec in matching {
        let decision = run_hook(spec, &event).await;
        if let HookDecision::Deny { ref reason } = decision {
            tracing::info!(
                "[HookRunner] Hook {} 拒绝了工具 {} 的执行: {}",
                spec.name,
                tool_name,
                reason
            );
            return decision;
        }
    }

    HookDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_from_json_allow() {
        let d = HookDecision::from_json(r#"{"decision": "allow"}"#);
        assert!(matches!(d, HookDecision::Allow));
    }

    #[test]
    fn decision_from_json_deny() {
        let d = HookDecision::from_json(r#"{"decision": "deny", "reason": "blocked"}"#);
        match d {
            HookDecision::Deny { reason } => assert_eq!(reason, "blocked"),
            _ => panic!("Expected Deny"),
        }
    }

    #[test]
    fn decision_from_invalid_json_defaults_allow() {
        let d = HookDecision::from_json("not json");
        assert!(matches!(d, HookDecision::Allow));
    }

    #[test]
    fn decision_from_exit_code() {
        assert!(matches!(
            HookDecision::from_exit_code(0),
            HookDecision::Allow
        ));
        assert!(matches!(
            HookDecision::from_exit_code(2),
            HookDecision::Deny { .. }
        ));
        assert!(matches!(
            HookDecision::from_exit_code(1),
            HookDecision::Allow
        ));
    }
}
