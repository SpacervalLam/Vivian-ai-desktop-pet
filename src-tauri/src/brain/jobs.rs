//! Task-owned background execution using the same process broker as run_command.
use crate::tools::types::ToolUseContext;
use parking_lot::RwLock;
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
pub const JOB_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Killed,
}
#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub job_id: String,
    pub command: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub output: String,
    pub truncated: bool,
    pub output_file: Option<String>,
    pub dropped_chars: u64,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}
struct JobState {
    info: JobInfo,
    owner: (String, String, String, String),
    cancel: tokio::sync::watch::Sender<bool>,
}
fn owner(ctx: &ToolUseContext) -> (String, String, String, String) {
    (
        ctx.user_id.clone(),
        ctx.char_id.clone(),
        ctx.agent_kind.clone(),
        ctx.session_id.clone(),
    )
}
#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<RwLock<BTreeMap<String, JobState>>>,
}
impl JobManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            jobs: Default::default(),
        })
    }
    pub fn start(
        &self,
        command: String,
        ctx: &ToolUseContext,
        timeout: u64,
    ) -> Result<String, String> {
        let (ctx, changes) = crate::tools::access::process_snapshot(ctx)?;
        let id = format!("job-{}", uuid::Uuid::new_v4().simple());
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        self.jobs.write().insert(
            id.clone(),
            JobState {
                info: JobInfo {
                    job_id: id.clone(),
                    command: command.clone(),
                    status: JobStatus::Running,
                    exit_code: None,
                    output: String::new(),
                    truncated: false,
                    output_file: None,
                    dropped_chars: 0,
                    created_at_ms: chrono::Utc::now().timestamp_millis() as u64,
                    finished_at_ms: None,
                },
                owner: owner(&ctx),
                cancel,
            },
        );
        let jobs = self.jobs.clone();
        let job = id.clone();
        tauri::async_runtime::spawn(async move {
            let progress_jobs = jobs.clone();
            let progress_id = job.clone();
            let progress: crate::tools::builtin::terminal_tools::Progress = Arc::new(move |text| {
                if let Some(j) = progress_jobs.write().get_mut(&progress_id) {
                    j.info.output = text.into();
                }
            });
            let args = serde_json::json!({"command":command,"timeout_secs":timeout});
            let result = tokio::select! {
                biased;
                _=cancelled.wait_for(|value|*value)=>None,
                _=crate::tools::access::invalidated(changes, Arc::new(std::sync::atomic::AtomicBool::new(true)), ctx.access_level.unwrap_or(crate::tools::types::AgentAccessLevel::FsWrite))=>None,
                r=crate::tools::builtin::terminal_tools::execute_process(&ctx,&args,Some(progress))=>Some(r),
            };
            if let Some(job) = jobs.write().get_mut(&job) {
                let info = &mut job.info;
                if info.status != JobStatus::Running {
                    return;
                }
                if let Some(result) = result {
                    let data = result.data.as_ref().and_then(|v| v.get("data"));
                    if let Some(output) = data.and_then(|d| d["output"].as_str()) {
                        info.output = output.into();
                    }
                    info.exit_code = data.and_then(|d| d["exit_code"].as_i64()).map(|v| v as i32);
                    info.truncated = data.is_some_and(|d| d["truncated"] == true);
                    info.output_file = data
                        .and_then(|d| d["output_file"].as_str())
                        .map(str::to_string);
                    info.dropped_chars =
                        data.and_then(|d| d["dropped_chars"].as_u64()).unwrap_or(0);
                    info.status = if result.success && info.exit_code == Some(0) {
                        JobStatus::Succeeded
                    } else if result.error.as_deref() == Some("PermissionChanged") {
                        JobStatus::Killed
                    } else {
                        JobStatus::Failed
                    };
                    if !result.success {
                        info.output
                            .push_str(&format!("\n{}", result.data.unwrap_or_default()));
                    }
                } else {
                    info.status = JobStatus::Killed;
                }
                info.finished_at_ms = Some(chrono::Utc::now().timestamp_millis() as u64);
            }
        });
        Ok(id)
    }
    pub fn get(&self, id: &str, ctx: &ToolUseContext) -> Option<JobInfo> {
        self.jobs
            .read()
            .get(id)
            .filter(|j| j.owner == owner(ctx))
            .map(|j| j.info.clone())
    }
    pub fn list(&self, ctx: &ToolUseContext) -> Vec<JobInfo> {
        self.jobs
            .read()
            .values()
            .filter(|j| j.owner == owner(ctx))
            .map(|j| j.info.clone())
            .collect()
    }
    pub fn kill(&self, id: &str, ctx: &ToolUseContext) -> bool {
        let mut jobs = self.jobs.write();
        if let Some(j) = jobs
            .get_mut(id)
            .filter(|j| j.owner == owner(ctx) && j.info.status == JobStatus::Running)
        {
            j.cancel.send_replace(true);
            j.info.status = JobStatus::Killed;
            j.info.finished_at_ms = Some(chrono::Utc::now().timestamp_millis() as u64);
            true
        } else {
            false
        }
    }
    pub fn running_count(&self) -> usize {
        self.jobs
            .read()
            .values()
            .filter(|j| j.info.status == JobStatus::Running)
            .count()
    }
}
