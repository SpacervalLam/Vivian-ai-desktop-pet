//! Live task policy and synchronous process revocation. No tool or model can mint a grant.
use super::types::{AgentAccessLevel, ToolUseContext};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision {
    pub epoch: u64,
    pub level: AgentAccessLevel,
}
struct TaskPolicy {
    identity: String,
    state: watch::Sender<Revision>,
    processes: HashMap<u64, Arc<dyn Fn() + Send + Sync>>,
}
static POLICIES: LazyLock<Mutex<HashMap<String, TaskPolicy>>> = LazyLock::new(Default::default);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn set_permission(session: &str, level: AgentAccessLevel) {
    if session.is_empty() {
        return;
    }
    let callbacks = {
        let mut policies = POLICIES.lock();
        let Some(policy) = policies.get_mut(session) else {
            policies.insert(
                session.into(),
                TaskPolicy {
                    identity: uuid::Uuid::new_v4().to_string(),
                    state: watch::channel(Revision { epoch: 1, level }).0,
                    processes: HashMap::new(),
                },
            );
            return;
        };
        let old = *policy.state.borrow();
        if old.level == level {
            return;
        }
        policy.identity = uuid::Uuid::new_v4().to_string();
        policy.state.send_replace(Revision {
            epoch: old.epoch + 1,
            level,
        });
        if level < old.level {
            policy.processes.values().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    };
    // Synchronous callbacks kill OS process trees before the permission command returns.
    for stop in callbacks {
        stop();
    }
    tracing::info!(session_id = session, ?level, "task_permission_changed");
}

pub fn snapshot(ctx: &ToolUseContext) -> (ToolUseContext, Option<watch::Receiver<Revision>>) {
    let mut ctx = ctx.clone();
    if !ctx.is_work_agent() || ctx.session_id.is_empty() {
        return (ctx, None);
    }
    let mut policies = POLICIES.lock();
    let policy = policies
        .entry(ctx.session_id.clone())
        .or_insert_with(|| TaskPolicy {
            identity: uuid::Uuid::new_v4().to_string(),
            state: watch::channel(Revision {
                epoch: 1,
                level: ctx.access_level.unwrap_or(AgentAccessLevel::FsWrite),
            })
            .0,
            processes: HashMap::new(),
        });
    let state = *policy.state.borrow();
    ctx.access_level = Some(if ctx.subagent_depth > 0 {
        ctx.access_level.unwrap_or(state.level).min(state.level)
    } else {
        state.level
    });
    ctx.policy_epoch = state.epoch;
    (ctx, Some(policy.state.subscribe()))
}

/// Never rebase an already authorized process request onto a newer scope.
pub fn process_snapshot(
    ctx: &ToolUseContext,
) -> Result<(ToolUseContext, Option<watch::Receiver<Revision>>), String> {
    if ctx.policy_epoch == 0 {
        return Ok(snapshot(ctx));
    }
    let policies = POLICIES.lock();
    let policy = policies.get(&ctx.session_id).ok_or("任务授权已失效")?;
    if policy.state.borrow().epoch != ctx.policy_epoch {
        return Err("权限已变化，旧进程请求已取消，请按当前权限重新评估".into());
    }
    Ok((ctx.clone(), Some(policy.state.subscribe())))
}
pub fn current(ctx: &ToolUseContext) -> bool {
    if !ctx.is_work_agent() || ctx.session_id.is_empty() {
        return true;
    }
    POLICIES
        .lock()
        .get(&ctx.session_id)
        .is_none_or(|p| p.state.borrow().epoch == ctx.policy_epoch)
}

pub struct ProcessRegistration {
    session: String,
    id: u64,
}
impl Drop for ProcessRegistration {
    fn drop(&mut self) {
        if let Some(p) = POLICIES.lock().get_mut(&self.session) {
            p.processes.remove(&self.id);
        }
    }
}
pub fn register_process(
    ctx: &ToolUseContext,
    stop: Arc<dyn Fn() + Send + Sync>,
) -> Result<ProcessRegistration, String> {
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut policies = POLICIES.lock();
    if let Some(p) = policies.get_mut(&ctx.session_id) {
        if p.state.borrow().epoch != ctx.policy_epoch {
            drop(policies);
            stop();
            return Err("权限已变化，进程未获得当前执行授权".into());
        }
        p.processes.insert(id, stop);
    }
    Ok(ProcessRegistration {
        session: ctx.session_id.clone(),
        id,
    })
}

/// Routine operations whose actual effects are bounded by the task worker.
pub fn process_tool(name: &str) -> bool {
    matches!(
        name,
        "run_command" | "run_job" | "pwsh_persistent" | "terminal_open" | "terminal_send"
    )
}
pub fn inspect_tool(name: &str, args: &serde_json::Value) -> bool {
    matches!(
        name,
        "read_file"
            | "read_image"
            | "list_dir"
            | "glob"
            | "grep_search"
            | "tool_search"
            | "tool_list"
            | "get_work_status"
            | "working_directory"
            | "terminal_read"
            | "terminal_list"
            | "terminal_close"
    ) || name == "edit_file" && args["action"] == "view"
        || name == "manage_job" && matches!(args["action"].as_str(), Some("get" | "list" | "kill"))
}

/// Credentials belong to the host/provider, never an arbitrary worker environment.
pub fn secret_environment_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.ends_with("_API_KEY")
        || key.ends_with("_TOKEN")
        || key.ends_with("_SECRET")
        || key.ends_with("_PASSWORD")
        || matches!(
            key.as_str(),
            "API_KEY"
                | "TOKEN"
                | "SECRET"
                | "PASSWORD"
                | "GH_TOKEN"
                | "GITHUB_TOKEN"
                | "AWS_ACCESS_KEY_ID"
                | "AWS_SECRET_ACCESS_KEY"
                | "AZURE_CLIENT_SECRET"
        )
}

pub fn can_continue(ctx: &ToolUseContext) -> bool {
    POLICIES.lock().get(&ctx.session_id).is_none_or(|p| {
        p.state.borrow().epoch == ctx.policy_epoch
            || p.state.borrow().level > ctx.access_level.unwrap_or(AgentAccessLevel::FsWrite)
    })
}
pub async fn invalidated(
    mut receiver: Option<watch::Receiver<Revision>>,
    started: Arc<std::sync::atomic::AtomicBool>,
    initial: AgentAccessLevel,
) {
    let Some(rx) = receiver.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        if !started.load(std::sync::atomic::Ordering::Acquire) || rx.borrow().level <= initial {
            return;
        }
    }
}

pub fn write_file(
    ctx: &ToolUseContext,
    path: impl AsRef<std::path::Path>,
    data: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    with_write_authority(ctx, || std::fs::write(path, data))
}
pub fn create_dir_all(
    ctx: &ToolUseContext,
    path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    with_write_authority(ctx, || std::fs::create_dir_all(path))
}
fn with_write_authority<T>(
    ctx: &ToolUseContext,
    operation: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let policies = POLICIES.lock();
    if let Some(p) = policies.get(&ctx.session_id) {
        let revision = *p.state.borrow();
        if revision.epoch != ctx.policy_epoch || revision.level == AgentAccessLevel::ReadOnly {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "权限已变化，文件写入未执行",
            ));
        }
    }
    operation()
}

pub fn write_identity(ctx: &ToolUseContext) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let policies = POLICIES.lock();
    let p = policies.get(&ctx.session_id)?;
    if p.state.borrow().epoch != ctx.policy_epoch {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    p.identity.hash(&mut hash);
    ctx.working_directory.hash(&mut hash);
    format!("{:?}", ctx.access_level).hash(&mut hash);
    serde_json::to_string(&ctx.extra_working_directories)
        .ok()?
        .hash(&mut hash);
    let value = hash.finish();
    Some(format!(
        "S-1-5-21-{}-{}-{}-1001",
        value as u32,
        (value >> 32) as u32,
        ctx.policy_epoch as u32
    ))
}
pub fn invalidate_scope(session: &str) {
    let callbacks = {
        let mut policies = POLICIES.lock();
        let Some(p) = policies.get_mut(session) else {
            return;
        };
        let old = *p.state.borrow();
        p.identity = uuid::Uuid::new_v4().to_string();
        p.state.send_replace(Revision {
            epoch: old.epoch + 1,
            ..old
        });
        p.processes.values().cloned().collect::<Vec<_>>()
    };
    for stop in callbacks {
        stop();
    }
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
