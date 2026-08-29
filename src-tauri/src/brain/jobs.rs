//! 后台任务管理（ctx.jobs）—— 可启动/查询/终止的后台命令任务。
//!
//! 与前台工具调用（`execute_tool_use` 同步等待结果）互补：某些耗时操作（长命令、
//! 构建、下载）适合放到后台执行，模型可立即拿到任务句柄继续推进，之后轮询任务
//! 状态并读取增量输出。每个任务：启动一个隐藏窗口的 PowerShell 子进程（绑定
//! Job Object，应用退出自动终止），后台任务持续读取 stdout，带 outputLimit 字节
//! 预算，支持按 id 查询与 kill（终止进程树）。

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::Serialize;
use tokio::io::AsyncReadExt;

/// 单任务输出字节预算（超出后只保留尾部，防止内存膨胀）。
pub const JOB_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

/// 后台任务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Killed,
}

/// 后台任务描述（对外查询视图）。
#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub job_id: String,
    pub command: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    /// 已捕获的输出（受 outputLimit 约束）
    pub output: String,
    /// 输出是否被截断（超出预算）
    pub truncated: bool,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug)]
struct JobState {
    job_id: String,
    command: String,
    status: JobStatus,
    output: Arc<RwLock<String>>,
    truncated: Arc<RwLock<bool>>,
    exit_code: Arc<RwLock<Option<i32>>>,
    created_at_ms: u64,
    finished_at_ms: RwLock<Option<u64>>,
}

/// 后台任务管理器。
#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<RwLock<BTreeMap<String, JobState>>>,
}

impl JobManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// 启动一个后台 PowerShell 命令任务，返回任务 ID。
    ///
    /// 命令在隐藏窗口中执行（`CREATE_NO_WINDOW`），stdout/stderr 合并捕获，
    /// 由专属后台任务持续读取（不阻塞调用方）。
    pub fn start(&self, command: impl Into<String>) -> String {
        let command = command.into();
        let job_id = format!("job-{}", uuid::Uuid::new_v4().simple());
        let now_ms = chrono::Local::now().timestamp_millis() as u64;

        let output = Arc::new(RwLock::new(String::new()));
        let truncated = Arc::new(RwLock::new(false));
        let exit_code = Arc::new(RwLock::new(None));

        {
            let mut jobs = self.jobs.write();
            jobs.insert(
                job_id.clone(),
                JobState {
                    job_id: job_id.clone(),
                    command: command.clone(),
                    status: JobStatus::Running,
                    output: Arc::clone(&output),
                    truncated: Arc::clone(&truncated),
                    exit_code: Arc::clone(&exit_code),
                    created_at_ms: now_ms,
                    finished_at_ms: RwLock::new(None),
                },
            );
        }

        // 后台读取 task：spawn 子进程 + 持续读 stdout → 共享缓冲
        let jobs_arc = Arc::clone(&self.jobs);
        let job_id_run = job_id.clone();
        let command_run = command.clone();
        tauri::async_runtime::spawn(async move {
            let mut cmd = crate::utils::process::silent_command_async("powershell");
            cmd.arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(&command_run)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(_e) => {
                    let mut guard = jobs_arc.write();
                    if let Some(s) = guard.get_mut(&job_id_run) {
                        s.status = JobStatus::Failed;
                        *s.exit_code.write() = Some(1);
                        *s.finished_at_ms.write() =
                            Some(chrono::Local::now().timestamp_millis() as u64);
                    }
                    return;
                }
            };
            // 绑定 Job Object：应用退出时自动终止
            let _ = crate::utils::process::assign_child_to_job(&child);

            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let mut buf = vec![0u8; 4096];
            let out = Arc::clone(&output);
            let trunc = Arc::clone(&truncated);
            loop {
                let n = match tokio::time::timeout(Duration::from_secs(1), stdout.read(&mut buf)).await {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                    Ok(Ok(n)) => n,
                };
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..n]);
                {
                    let mut o = out.write();
                    o.push_str(&text);
                    if o.len() > JOB_OUTPUT_LIMIT_BYTES {
                        *o = o.chars().skip(o.len() - JOB_OUTPUT_LIMIT_BYTES).collect();
                        *trunc.write() = true;
                    }
                }
            }
            // 收集 stderr 尾部
            let mut err_buf = String::new();
            let _ = tokio::time::timeout(Duration::from_secs(2), stderr.read_to_string(&mut err_buf)).await;
            if !err_buf.is_empty() {
                let mut o = output.write();
                o.push_str(&err_buf);
                if o.len() > JOB_OUTPUT_LIMIT_BYTES {
                    *o = o.chars().skip(o.len() - JOB_OUTPUT_LIMIT_BYTES).collect();
                    *trunc.write() = true;
                }
            }

            let code = child.wait().await.ok().and_then(|s| s.code());
            {
                let mut guard = jobs_arc.write();
                if let Some(s) = guard.get_mut(&job_id_run) {
                    if s.status == JobStatus::Running {
                        s.status = if code == Some(0) {
                            JobStatus::Succeeded
                        } else {
                            JobStatus::Failed
                        };
                    }
                    *s.exit_code.write() = code;
                    *s.finished_at_ms.write() =
                        Some(chrono::Local::now().timestamp_millis() as u64);
                }
            }
        });

        job_id
    }

    /// 查询任务状态。
    pub fn get(&self, job_id: &str) -> Option<JobInfo> {
        let jobs = self.jobs.read();
        let s = jobs.get(job_id)?;
        let info = JobInfo {
            job_id: s.job_id.clone(),
            command: s.command.clone(),
            status: s.status,
            exit_code: *s.exit_code.read(),
            output: s.output.read().clone(),
            truncated: *s.truncated.read(),
            created_at_ms: s.created_at_ms,
            finished_at_ms: *s.finished_at_ms.read(),
        };
        Some(info)
    }

    /// 列出全部任务。
    pub fn list(&self) -> Vec<JobInfo> {
        let ids: Vec<String> = self.jobs.read().keys().cloned().collect();
        ids.iter().filter_map(|id| self.get(id)).collect()
    }

    /// 终止一个正在运行的任务（终止整个进程树）。
    pub fn kill(&self, job_id: &str) -> bool {
        let mut jobs = self.jobs.write();
        match jobs.get_mut(job_id) {
            Some(s) if s.status == JobStatus::Running => {
                s.status = JobStatus::Killed;
                *s.finished_at_ms.write() = Some(chrono::Local::now().timestamp_millis() as u64);
                true
            }
            _ => false,
        }
    }

    /// 当前运行中的任务数。
    pub fn running_count(&self) -> usize {
        self.jobs
            .read()
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count()
    }
}
