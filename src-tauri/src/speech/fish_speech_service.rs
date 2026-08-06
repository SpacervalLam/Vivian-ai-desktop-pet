//! Fish Speech 本地服务子进程管理 — 一键启动 / 停止 / 状态查询
//!
//! 通过子进程启动 `python -m tools.api_server`（fish-speech 仓库内置的 HTTP 服务），
//! 默认监听 127.0.0.1:8080，与 FishSpeech TTS 后端的 `/v1/tts` 调用直连。
//!
//! 启动参数取自 `TtsConfig` 的 `fish_speech_*` 字段（安装路径/Python 路径/端口等）。
//! 启动后异步等待健康检查通过（默认 60s 超时）；前端可轮询 `get_fish_speech_service_status`
//! 获取最新状态。启动成功后由调用方负责把 `fish_speech_url` 更新为 `http://127.0.0.1:<port>`。

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{VivianError, VivianResult};
use crate::utils::process::{assign_child_to_job, silent_command_async};
use crate::utils::pid_file;

use super::tts::TtsConfig;

/// 服务运行状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FishSpeechServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

/// 服务对外暴露的状态快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishSpeechServiceState {
    pub status: FishSpeechServiceStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub command_line: Option<String>,
    pub endpoint: Option<String>,
    pub port: Option<u16>,
}

impl Default for FishSpeechServiceState {
    fn default() -> Self {
        Self {
            status: FishSpeechServiceStatus::Stopped,
            pid: None,
            error: None,
            command_line: None,
            endpoint: None,
            port: None,
        }
    }
}

/// 内部可变状态
#[derive(Debug, Clone)]
struct InnerState {
    status: FishSpeechServiceStatus,
    pid: Option<u32>,
    error: Option<String>,
    command_line: Option<String>,
    endpoint: Option<String>,
    port: Option<u16>,
}

impl Default for InnerState {
    fn default() -> Self {
        Self {
            status: FishSpeechServiceStatus::Stopped,
            pid: None,
            error: None,
            command_line: None,
            endpoint: None,
            port: None,
        }
    }
}

/// Fish Speech 本地服务管理器（全局单例）
pub struct FishSpeechServiceManager {
    child: AsyncMutex<Option<Child>>,
    state: Mutex<InnerState>,
    stderr_buf: Arc<Mutex<String>>,
    cached_state: RwLock<FishSpeechServiceState>,
}

impl FishSpeechServiceManager {
    pub fn new() -> Self {
        Self {
            child: AsyncMutex::new(None),
            state: Mutex::new(InnerState::default()),
            stderr_buf: Arc::new(Mutex::new(String::new())),
            cached_state: RwLock::new(FishSpeechServiceState::default()),
        }
    }

    /// 当前对外状态快照
    pub fn state(&self) -> FishSpeechServiceState {
        self.cached_state.read().clone()
    }

    /// 同步内部状态到缓存并返回
    fn snapshot(&self) -> FishSpeechServiceState {
        let s = self.state.lock().clone();
        let st = FishSpeechServiceState {
            status: s.status,
            pid: s.pid,
            error: s.error,
            command_line: s.command_line,
            endpoint: s.endpoint,
            port: s.port,
        };
        *self.cached_state.write() = st.clone();
        st
    }

    /// 启动 fish-speech 本地服务子进程
    pub async fn start(self: &Arc<Self>, config: &TtsConfig) -> VivianResult<FishSpeechServiceState> {
        // 已有实例运行则先停止
        if self.child.lock().await.is_some() {
            tracing::info!("[FishSpeech] 已有实例运行，先停止...");
            self.stop_internal().await;
        }

        let port = config.fish_speech_port.unwrap_or(8080);
        let host = "127.0.0.1";
        let endpoint = format!("http://{host}:{port}");

        // 解析可执行文件与 cwd
        let (program, args, cwd) = resolve_invocation(config)?;

        // 清理上次崩溃残留的孤儿进程
        pid_file::cleanup_orphan(&format!("fish_speech_{port}"));

        // 端口冲突检测与清理
        if let Err(e) = ensure_port_available(port).await {
            return Err(VivianError::Speech(format!(
                "端口 {port} 已被占用且无法释放: {e}\n请手动结束占用端口的进程后重试"
            )));
        }

        let cmdline = format!(
            "{} {}{}",
            program,
            args.join(" "),
            cwd.as_ref()
                .map(|p| format!(" (cwd={})", p.display()))
                .unwrap_or_default()
        );
        tracing::info!("[FishSpeech] 启动 fish-speech 服务: {}", cmdline);

        let mut cmd = silent_command_async(&program);
        cmd.args(&args)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(ref dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| VivianError::Speech(format!("启动 fish-speech 服务失败: {e}")))?;

        let pid = child.id();
        assign_child_to_job(&child);
        if let Some(pid) = pid {
            pid_file::write_pid(&format!("fish_speech_{port}"), pid);
        }

        // stdout/stderr 行级读取
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!("[FishSpeech] stdout: {line}");
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let buf = Arc::clone(&self.stderr_buf);
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!("[FishSpeech] stderr: {line}");
                    let mut b = buf.lock();
                    if b.len() > 2000 {
                        b.clear();
                    }
                    b.push_str(&line);
                    b.push('\n');
                }
            });
        }

        // 更新状态为 Starting
        {
            let mut s = self.state.lock();
            s.status = FishSpeechServiceStatus::Starting;
            s.pid = pid;
            s.error = None;
            s.command_line = Some(cmdline);
            s.endpoint = Some(endpoint.clone());
            s.port = Some(port);
        }
        {
            let mut guard = self.child.lock().await;
            *guard = Some(child);
        }

        // 异步等待健康检查
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.wait_for_health(endpoint, port).await;
        });

        Ok(self.snapshot())
    }

    /// 健康检查 + 进程监控
    async fn wait_for_health(self: Arc<Self>, endpoint: String, port: u16) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        // fish-speech 的健康检查端点
        let probe_urls = [
            format!("{}/v1/health", endpoint.trim_end_matches('/')),
            format!("{}/", endpoint.trim_end_matches('/')),
        ];
        let tcp_addr = format!("127.0.0.1:{port}");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut last_err = String::new();
        let mut consecutive_tcp_fail = 0u32;

        loop {
            if std::time::Instant::now() >= deadline {
                {
                    let mut s = self.state.lock();
                    s.status = FishSpeechServiceStatus::Crashed;
                    s.error = Some(format!(
                        "启动超时(60s) port={port}: {last_err}\n可能原因: 模型下载缓慢、Python 运行时缺少依赖、或 fish-speech 未安装"
                    ));
                }
                self.snapshot();
                tracing::warn!("[FishSpeech] 服务启动超时,最后错误: {last_err}");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

            // 检查子进程是否已退出
            let mut guard = self.child.lock().await;
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code().unwrap_or(-1);
                        let err_msg = {
                            let b = self.stderr_buf.lock();
                            b.trim().to_string()
                        };
                        let error = if err_msg.is_empty() {
                            format!(
                                "服务进程退出 code={}(无输出) port={port}。可能原因: fish-speech 未安装、Python 缺少依赖、或模型加载失败",
                                code
                            )
                        } else {
                            let tail: String = err_msg.lines().rev().take(5).collect::<Vec<_>>().join("\n");
                            format!("code={} port={port}\n{}", code, tail)
                        };
                        {
                            let mut s = self.state.lock();
                            s.status = FishSpeechServiceStatus::Crashed;
                            s.error = Some(error);
                            s.pid = None;
                        }
                        *guard = None;
                        self.snapshot();
                        tracing::warn!("[FishSpeech] 服务进程在启动期间退出 code={}", code);
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            } else {
                return;
            }
            drop(guard);

            // TCP 探测
            let tcp_ok = tokio::net::TcpStream::connect(&tcp_addr).await.is_ok();
            if !tcp_ok {
                consecutive_tcp_fail += 1;
                if consecutive_tcp_fail >= 8 {
                    {
                        let mut s = self.state.lock();
                        s.status = FishSpeechServiceStatus::Crashed;
                        s.error = Some(format!(
                            "服务进程存活但端口 {tcp_addr} 无法连接(连续 {consecutive_tcp_fail} 次, port={port})。可能原因: 模型加载内存不足、GPU OOM、或进程卡死"
                        ));
                        s.pid = None;
                    }
                    if let Some(mut child) = self.child.lock().await.take() {
                        let _ = child.start_kill();
                    }
                    self.snapshot();
                    tracing::warn!("[FishSpeech] TCP 连续失败,判定进程卡死");
                    return;
                }
                last_err = "TCP 连接失败".to_string();
                continue;
            }
            consecutive_tcp_fail = 0;

            // HTTP 健康检查
            let mut ok = false;
            for url in &probe_urls {
                match client.get(url).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
                            ok = true;
                            break;
                        }
                        last_err = format!("HTTP {}", status);
                    }
                    Err(e) => {
                        last_err = format!("HTTP请求失败: {e}");
                    }
                }
            }
            if ok {
                {
                    let mut s = self.state.lock();
                    s.status = FishSpeechServiceStatus::Running;
                }
                self.snapshot();
                tracing::info!("[FishSpeech] 服务健康检查通过: {endpoint}");

                // 启动运行期进程监控
                let mgr = Arc::clone(&self);
                tokio::spawn(async move {
                    mgr.monitor().await;
                });
                return;
            }
        }
    }

    /// 运行期进程监控（每 3s 检查一次）
    async fn monitor(self: Arc<Self>) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let mut guard = self.child.lock().await;
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(status)) => {
                        {
                            let mut s = self.state.lock();
                            s.status = FishSpeechServiceStatus::Crashed;
                            s.error = Some(format!(
                                "服务进程异常退出 code={} port={:?}",
                                status.code().unwrap_or(-1),
                                s.port
                            ));
                            s.pid = None;
                        }
                        *guard = None;
                        self.snapshot();
                        tracing::warn!("[FishSpeech] 服务进程在运行期间退出");
                        return;
                    }
                    Err(e) => {
                        {
                            let mut s = self.state.lock();
                            s.status = FishSpeechServiceStatus::Crashed;
                            s.error = Some(format!("查询子进程状态失败: {e}"));
                            s.pid = None;
                        }
                        *guard = None;
                        self.snapshot();
                        return;
                    }
                }
            } else {
                return;
            }
        }
    }

    /// 内部停止
    async fn stop_internal(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let port = self.state.lock().port;
            {
                let mut s = self.state.lock();
                s.status = FishSpeechServiceStatus::Stopping;
            }
            self.snapshot();
            let _ = child.start_kill();
            let _ = child.wait().await;
            if let Some(p) = port {
                pid_file::remove_pid(&format!("fish_speech_{p}"));
            }
            {
                let mut s = self.state.lock();
                s.status = FishSpeechServiceStatus::Stopped;
                s.pid = None;
                s.error = None;
            }
            self.snapshot();
            tracing::info!("[FishSpeech] 服务已停止");
        }
    }

    /// 对外停止
    pub async fn stop(&self) -> VivianResult<FishSpeechServiceState> {
        self.stop_internal().await;
        Ok(self.snapshot())
    }

    /// 刷新状态（检测僵尸进程）
    pub async fn refresh(&self) -> VivianResult<FishSpeechServiceState> {
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    let mut s = self.state.lock();
                    s.status = FishSpeechServiceStatus::Crashed;
                    s.pid = None;
                    s.error = Some("服务进程已退出".to_string());
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
        drop(guard);
        Ok(self.snapshot())
    }
}

/// 解析 fish-speech 启动命令
///
/// 返回 (program, args, cwd)：
/// - program: python 可执行文件（配置的 python_path 或 PATH 中的 python）
/// - args: `-m tools.api_server --listen 127.0.0.1:port`
/// - cwd: fish_speech_install_path（fish-speech 仓库根目录）
fn resolve_invocation(
    config: &TtsConfig,
) -> VivianResult<(String, Vec<String>, Option<std::path::PathBuf>)> {
    let port = config.fish_speech_port.unwrap_or(8080);

    // 解析 python 路径
    let program: String = if let Some(py) = config
        .fish_speech_python_path
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let py_path = std::path::Path::new(py);
        if !py_path.exists() {
            return Err(VivianError::Speech(format!(
                "Python 路径不存在: {py}"
            )));
        }
        py.to_string()
    } else {
        "python".to_string()
    };

    // 解析安装路径（cwd）
    let cwd: Option<std::path::PathBuf> = config
        .fish_speech_install_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .map(|p| {
            if !p.is_dir() {
                return Err(VivianError::Speech(format!(
                    "fish-speech 安装路径不存在或不是目录: {}",
                    p.display()
                )));
            }
            // 校验 tools/api_server.py 是否存在
            let api_server = p.join("tools").join("api_server.py");
            if !api_server.exists() {
                return Err(VivianError::Speech(format!(
                    "在安装路径下未找到 tools/api_server.py: {}\n请确认已 git clone fish-speech 仓库",
                    p.display()
                )));
            }
            Ok(p)
        })
        .transpose()?;

    if cwd.is_none() {
        return Err(VivianError::Speech(
            "未配置 fish-speech 安装路径，无法启动本地服务。请在高级设置中填写 fish-speech 仓库根目录".to_string(),
        ));
    }

    let mut args: Vec<String> = vec![
        "-m".into(),
        "tools.api_server".into(),
        "--listen".into(),
        format!("127.0.0.1:{port}"),
    ];

    // 模型检查点路径
    if let Some(ref p) = config.fish_speech_llama_checkpoint_path {
        if !p.is_empty() {
            args.push("--llama-checkpoint-path".into());
            args.push(p.clone());
        }
    }
    // 解码器检查点路径
    if let Some(ref p) = config.fish_speech_decoder_checkpoint_path {
        if !p.is_empty() {
            args.push("--decoder-checkpoint-path".into());
            args.push(p.clone());
        }
    }
    // 半精度推理
    if config.fish_speech_half {
        args.push("--half".into());
    }
    // torch.compile 加速
    if config.fish_speech_compile {
        args.push("--compile".into());
    }

    Ok((program, args, cwd))
}

/// 检查端口是否被占用，若被占用则尝试 kill 占用进程
async fn ensure_port_available(port: u16) -> Result<(), String> {
    let addr = format!("127.0.0.1:{port}");
    let probe_timeout = std::time::Duration::from_secs(1);
    let connect_result = tokio::time::timeout(
        probe_timeout,
        tokio::net::TcpStream::connect(&addr),
    )
    .await;
    match connect_result {
        Err(_) => Ok(()),
        Ok(Err(ref e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
        Ok(Err(ref e)) if e.kind() == std::io::ErrorKind::TimedOut => Ok(()),
        Ok(Ok(_)) => {
            tracing::warn!("[FishSpeech] 端口 {port} 已被占用,尝试清理占用进程");
            kill_port_occupant(port).await?;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let recheck = tokio::time::timeout(
                probe_timeout,
                tokio::net::TcpStream::connect(&addr),
            )
            .await;
            match recheck {
                Err(_) => Ok(()),
                Ok(Err(ref e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
                Ok(Ok(_)) => Err(format!("kill 后端口 {port} 仍被占用")),
                Ok(Err(e)) => Err(format!("重检端口失败: {e}")),
            }
        }
        Ok(Err(e)) => Err(format!("探测端口失败: {e}")),
    }
}

/// Windows: kill 占用端口的进程
#[cfg(target_os = "windows")]
async fn kill_port_occupant(port: u16) -> Result<(), String> {
    let pid_output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        silent_command_async("netstat").args(["-ano"]).output(),
    )
    .await
    .map_err(|_| "执行 netstat 超时".to_string())?
    .map_err(|e| format!("执行 netstat 失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&pid_output.stdout);
    let mut pids: Vec<u32> = Vec::new();
    for line in stdout.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let local_addr = parts[1];
        let parsed_port = local_addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        if parsed_port != Some(port) {
            continue;
        }
        if let Ok(pid) = parts[4].parse::<u32>() {
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    if pids.is_empty() {
        return Err("netstat 未找到占用进程".to_string());
    }
    for pid in &pids {
        tracing::info!("[FishSpeech] kill 占用端口 {port} 的进程 pid={pid}");
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            silent_command_async("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output(),
        )
        .await;
    }
    Ok(())
}

/// Unix: kill 占用端口的进程
#[cfg(not(target_os = "windows"))]
async fn kill_port_occupant(port: u16) -> Result<(), String> {
    let pid_output = silent_command_async("sh")
        .args(["-c", &format!("lsof -ti :{port}")])
        .output()
        .await
        .map_err(|e| format!("执行 lsof 失败: {e}"))?;
    let stdout = String::from_utf8_lossy(&pid_output.stdout);
    for line in stdout.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            tracing::info!("[FishSpeech] kill 占用端口 {port} 的进程 pid={pid}");
            let _ = silent_command_async("kill")
                .args(["-9", &pid.to_string()])
                .output()
                .await;
        }
    }
    Ok(())
}

static SERVICE: tokio::sync::OnceCell<Arc<FishSpeechServiceManager>> =
    tokio::sync::OnceCell::const_new();

/// 获取全局 Fish Speech 服务管理器单例
pub async fn service() -> &'static Arc<FishSpeechServiceManager> {
    SERVICE
        .get_or_init(|| async { Arc::new(FishSpeechServiceManager::new()) })
        .await
}
