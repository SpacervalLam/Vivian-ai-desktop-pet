//! Whisper 本地服务子进程管理 — 一键启动 / 停止 / 状态查询
//!
//! 通过子进程启动 `faster-whisper-server`（OpenAI 兼容）推理服务，
//! 默认监听 127.0.0.1:8000，与 Whisper ASR 后端的 `/v1/audio/transcriptions` 调用直连。
//!
//! 启动参数取自 `WhisperConfig` 的 `service_*` 字段（模型/设备/精度/端口/Python 路径等）。
//! 启动后异步等待健康检查通过（默认 60s 超时）；前端可轮询 `get_whisper_service_status`
//! 获取最新状态。启动成功后由调用方负责把 `server_url` 更新为 `http://127.0.0.1:<port>`、
//! `api_format` 更新为 `openai` 并触发 `update_asr_config`。

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{VivianError, VivianResult};
use crate::utils::process::{assign_child_to_job, silent_command, silent_command_async};
use crate::utils::pid_file;

use super::whisper_backend::WhisperConfig;

/// 服务运行状态（与 GPT-SoVITS 服务状态语义一致）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WhisperServiceStatus {
    Stopped,
    Installing,
    Starting,
    Running,
    Stopping,
    Crashed,
}

/// Whisper 服务对外暴露的状态快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperServiceState {
    pub status: WhisperServiceStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub command_line: Option<String>,
    pub endpoint: Option<String>,
    pub port: Option<u16>,
}

impl Default for WhisperServiceState {
    fn default() -> Self {
        Self {
            status: WhisperServiceStatus::Stopped,
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
    status: WhisperServiceStatus,
    pid: Option<u32>,
    error: Option<String>,
    command_line: Option<String>,
    endpoint: Option<String>,
    port: Option<u16>,
}

impl Default for InnerState {
    fn default() -> Self {
        Self {
            status: WhisperServiceStatus::Stopped,
            pid: None,
            error: None,
            command_line: None,
            endpoint: None,
            port: None,
        }
    }
}

/// Whisper 本地服务管理器（全局单例，单实例）
pub struct WhisperServiceManager {
    child: AsyncMutex<Option<Child>>,
    state: Mutex<InnerState>,
    stderr_buf: Arc<Mutex<String>>,
    cached_state: RwLock<WhisperServiceState>,
}

impl WhisperServiceManager {
    pub fn new() -> Self {
        Self {
            child: AsyncMutex::new(None),
            state: Mutex::new(InnerState::default()),
            stderr_buf: Arc::new(Mutex::new(String::new())),
            cached_state: RwLock::new(WhisperServiceState::default()),
        }
    }

    /// 当前对外状态快照
    pub fn state(&self) -> WhisperServiceState {
        self.cached_state.read().clone()
    }

    /// 同步内部状态到缓存并返回
    fn snapshot(&self) -> WhisperServiceState {
        let s = self.state.lock().clone();
        let st = WhisperServiceState {
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

    /// 启动 faster-whisper-server 子进程（自动检测并安装依赖）
    ///
    /// `proxy_url`：应用层解析的代理 URL（来自 ProxyConfig.effective_proxy_url），
    ///              pip 安装时透传为 HTTPS_PROXY/HTTP_PROXY 环境变量。
    pub async fn start(
        self: &Arc<Self>,
        config: &WhisperConfig,
        proxy_url: Option<String>,
    ) -> VivianResult<WhisperServiceState> {
        // 已有实例运行则先停止
        if self.child.lock().await.is_some() {
            tracing::info!("[Whisper] 已有实例运行，先停止...");
            self.stop_internal().await;
        }

        // 快速检测 Python 与 faster-whisper-server 是否就绪
        let python = find_python(config).ok();
        let default_install_path = default_install_path();
        let install_path: Option<&str> = config
            .service_install_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| Some(default_install_path.as_str()));
        let installed = python
            .as_deref()
            .map(|py| is_faster_whisper_server_installed(py, install_path))
            .unwrap_or(false);

        if !installed {
            // 设状态 Installing，后台执行自动安装 + 启动
            {
                let mut s = self.state.lock();
                s.status = WhisperServiceStatus::Installing;
                s.pid = None;
                s.error = None;
                s.command_line = None;
                s.endpoint = None;
                s.port = config.service_port;
            }
            let snap = self.snapshot();
            tracing::info!("[Whisper] faster-whisper-server 未安装，开始后台自动安装...");

            let mgr = Arc::clone(self);
            let cfg = config.clone();
            tokio::spawn(async move {
                let py = match ensure_installed(&cfg, proxy_url.as_deref()).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("[Whisper] 自动安装失败: {e}");
                        let mut s = mgr.state.lock();
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(format!("自动安装失败: {e}"));
                        drop(s);
                        mgr.snapshot();
                        return;
                    }
                };
                if let Err(e) = mgr.launch(&cfg, Some(&py)).await {
                    tracing::error!("[Whisper] 安装后启动失败: {e}");
                    let mut s = mgr.state.lock();
                    s.status = WhisperServiceStatus::Crashed;
                    s.error = Some(format!("{e}"));
                    drop(s);
                    mgr.snapshot();
                }
            });

            return Ok(snap);
        }

        // 已安装，直接启动
        self.launch(config, python.as_deref()).await
    }

    /// 实际启动子进程（resolve_invocation → spawn → 健康检查）
    async fn launch(
        self: &Arc<Self>,
        config: &WhisperConfig,
        resolved_python: Option<&str>,
    ) -> VivianResult<WhisperServiceState> {
        let port = config.service_port.unwrap_or(8000);
        let host = "127.0.0.1";
        let endpoint = format!("http://{host}:{port}");

        // 解析可执行文件、cwd 与额外环境变量
        let (program, args, cwd, env_extras) = resolve_invocation(config, resolved_python)?;

        // 清理上次崩溃残留的孤儿进程（exe 名用于 PID 复用防护）
        pid_file::cleanup_orphan(
            &format!("whisper_{port}"),
            &crate::utils::pid_file::normalize_exe_name(&program),
        );

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
        tracing::info!("[Whisper] 启动 faster-whisper-server: {}", cmdline);

        let mut cmd = silent_command_async(&program);
        cmd.args(&args)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &env_extras {
            cmd.env(k, v);
        }
        if let Some(ref dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| VivianError::Speech(format!("启动 faster-whisper-server 失败: {e}")))?;

        let pid = child.id();
        assign_child_to_job(&child);
        if let Some(pid) = pid {
            pid_file::write_pid(&format!("whisper_{port}"), pid);
        }

        // stdout/stderr 行级读取
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!("[Whisper] stdout: {line}");
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
                    tracing::warn!("[Whisper] stderr: {line}");
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
            s.status = WhisperServiceStatus::Starting;
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

        // faster-whisper-server 0.0.2 提供 /health 端点（返回 200 + {"status":"ok"}）
        // 旧版可能只有根路径 / （返回 200 或 404）
        let probe_urls = [
            format!("{}/health", endpoint.trim_end_matches('/')),
            format!("{}/", endpoint.trim_end_matches('/')),
        ];
        let tcp_addr = format!("127.0.0.1:{port}");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut last_err = String::new();
        let mut consecutive_tcp_fail = 0u32;

        loop {
            if std::time::Instant::now() >= deadline {
                let mut s = self.state.lock();
                s.status = WhisperServiceStatus::Crashed;
                s.error = Some(format!(
                    "启动超时(60s) port={port}: {last_err}\n可能原因: 模型下载缓慢、Python 运行时缺少依赖、或 faster-whisper-server 未安装"
                ));
                drop(s);
                self.snapshot();
                tracing::warn!("[Whisper] 服务启动超时,最后错误: {last_err}");
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
                                "服务进程退出 code={}(无输出) port={port}。可能原因: faster-whisper-server 未安装、Python 缺少依赖、或模型下载失败",
                                code
                            )
                        } else {
                            let tail: String = err_msg.lines().rev().take(5).collect::<Vec<_>>().join("\n");
                            format!("code={} port={port}\n{}", code, tail)
                        };
                        let mut s = self.state.lock();
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(error);
                        s.pid = None;
                        drop(s);
                        *guard = None;
                        self.snapshot();
                        tracing::warn!("[Whisper] 服务进程在启动期间退出 code={}", code);
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
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(format!(
                            "服务进程存活但端口 {tcp_addr} 无法连接(连续 {consecutive_tcp_fail} 次, port={port})。可能原因: 模型加载内存不足、GPU OOM、或进程卡死"
                        ));
                        s.pid = None;
                    }
                    if let Some(mut child) = self.child.lock().await.take() {
                        let _ = child.start_kill();
                    }
                    self.snapshot();
                    tracing::warn!("[Whisper] TCP 连续失败,判定进程卡死");
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
                let mut s = self.state.lock();
                s.status = WhisperServiceStatus::Running;
                drop(s);
                self.snapshot();
                tracing::info!("[Whisper] 服务健康检查通过: {endpoint}");

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
                        let mut s = self.state.lock();
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(format!(
                            "服务进程异常退出 code={} port={:?}",
                            status.code().unwrap_or(-1),
                            s.port
                        ));
                        s.pid = None;
                        drop(s);
                        *guard = None;
                        self.snapshot();
                        tracing::warn!("[Whisper] 服务进程在运行期间退出");
                        return;
                    }
                    Err(e) => {
                        let mut s = self.state.lock();
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(format!("查询子进程状态失败: {e}"));
                        s.pid = None;
                        drop(s);
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

    /// 内部停止（不获取额外锁）
    async fn stop_internal(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let port = self.state.lock().port;
            {
                let mut s = self.state.lock();
                s.status = WhisperServiceStatus::Stopping;
            }
            self.snapshot();
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(p) = port {
                pid_file::remove_pid(&format!("whisper_{p}"));
            }
            {
                let mut s = self.state.lock();
                s.status = WhisperServiceStatus::Stopped;
                s.pid = None;
                s.error = None;
            }
            self.snapshot();
            tracing::info!("[Whisper] 服务已停止");
        } else {
            // 子进程已不存在，仅同步状态
            let mut s = self.state.lock();
            if s.status != WhisperServiceStatus::Stopped {
                s.status = WhisperServiceStatus::Stopped;
                s.pid = None;
            }
            drop(s);
            self.snapshot();
        }
    }

    /// 停止服务
    pub async fn stop(&self) -> VivianResult<WhisperServiceState> {
        self.stop_internal().await;
        Ok(self.state())
    }

    /// 刷新状态（检测进程是否存活，防止状态失真）
    pub async fn refresh(&self) -> WhisperServiceState {
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    let mut s = self.state.lock();
                    if s.status != WhisperServiceStatus::Stopping {
                        s.status = WhisperServiceStatus::Crashed;
                        s.error = Some(format!(
                            "子进程退出 code={}",
                            status.code().unwrap_or(-1)
                        ));
                        s.pid = None;
                    }
                    *guard = None;
                }
                Err(e) => {
                    let mut s = self.state.lock();
                    s.status = WhisperServiceStatus::Crashed;
                    s.error = Some(format!("查询子进程状态失败: {e}"));
                    s.pid = None;
                    *guard = None;
                }
            }
        } else {
            let mut s = self.state.lock();
            if s.status == WhisperServiceStatus::Starting
                || s.status == WhisperServiceStatus::Running
            {
                s.status = WhisperServiceStatus::Stopped;
            }
        }
        drop(guard);
        self.snapshot()
    }
}

/// 解析 faster-whisper-server 启动命令
///
/// 返回 (program, args, cwd, env_extras)：
/// - `env_extras`：需要注入子进程的环境变量（如 PYTHONPATH，用于 `--target` 安装场景）
///
/// 启动模式（按优先级）：
/// 1. **`python -m faster_whisper_server` 模式**（推荐，兼容 `--target` 安装）：
///    - 当 `service_install_path` 指向一个含 `faster_whisper_server/` 子目录的路径时启用
///    - 把 `service_install_path` 加入 PYTHONPATH，用 `python -m faster_whisper_server` 启动
/// 2. **控制台脚本模式**：`service_python_path` 同级目录有 `Scripts/faster-whisper-server.exe` 时启用
/// 3. **PATH 查找**：直接用 PATH 中的 `faster-whisper-server`
fn resolve_invocation(
    config: &WhisperConfig,
    resolved_python: Option<&str>,
) -> VivianResult<(String, Vec<String>, Option<std::path::PathBuf>, Vec<(&'static str, String)>)> {
    let model = config
        .service_model
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("small")
        .to_string();
    let port = config.service_port.unwrap_or(8000);
    let device = config
        .service_device
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("auto")
        .to_string();
    let compute_type = config
        .service_compute_type
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("auto")
        .to_string();

    // 服务参数（faster-whisper-server 0.0.2 命令行只支持 model 位置参数、--host、--port、--batch-size）
    // device/compute_type 通过 YAML config 文件 + FWS_CONFIG_PATH 环境变量传入
    let server_args: Vec<String> = vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        model.clone(),
    ];

    // 生成临时 YAML config 文件，配置 device/compute_type
    let config_yaml = format!(
        r#"batch_size: 1
model_options:
  device: {device}
  compute_type: {compute_type}
models:
  - name: default
    path: {model}
"#,
        device = device,
        compute_type = compute_type,
        model = model,
    );
    let config_path = std::env::temp_dir().join(format!("vivian_whisper_{}.yml", port));
    if let Err(e) = std::fs::write(&config_path, &config_yaml) {
        tracing::warn!("[Whisper] 写入临时 config 失败: {e},device/compute_type 配置将不生效");
    }
    let fws_config_env = config_path.to_string_lossy().to_string();
    // HuggingFace 模型下载镜像（国内 HF 被墙，faster-whisper 默认从 HF 下载模型）
    // 用户若已设置 HF_ENDPOINT 环境变量则尊重其选择
    let hf_endpoint = std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://hf-mirror.com".to_string());

    // 推导 Python 来源
    let py_source: Option<String> = resolved_python
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            config
                .service_python_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    // 模式 1：install_path 含 faster_whisper_server/ 子目录 → python -m 模式
    //   install_path 优先级：config.service_install_path → 默认应用数据目录
    let default_install_path = default_install_path();
    let install_path: Option<&str> = config
        .service_install_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(default_install_path.as_str()));
    if let Some(path) = install_path {
        let install_dir = std::path::Path::new(path);
        if install_dir.is_dir() && install_dir.join("faster_whisper_server").is_dir() {
            // 解析实际可用的 Python：py_source 优先，路径不存在则回退到 PATH 查找
            let py = if let Some(ref p) = py_source {
                if std::path::Path::new(p).exists() {
                    p.clone()
                } else {
                    tracing::warn!(
                        "[Whisper] 配置的 Python 路径不存在: {p}，回退到系统 PATH 查找"
                    );
                    find_python_in_path()?
                }
            } else {
                find_python_in_path()?
            };
            let mut args = vec![
                "-m".into(),
                "faster_whisper_server".into(),
            ];
            args.extend(server_args);
            let env_extras = vec![
                ("PYTHONPATH", path.to_string()),
                ("FWS_CONFIG_PATH", fws_config_env.clone()),
                ("HF_ENDPOINT", hf_endpoint.clone()),
            ];
            tracing::info!(
                "[Whisper] 启动模式: python -m faster_whisper_server (PYTHONPATH={}, FWS_CONFIG_PATH={}, HF_ENDPOINT={})",
                path,
                config_path.display(),
                hf_endpoint
            );
            return Ok((py, args, None, env_extras));
        }
    }

    // 模式 2：Python 同级目录有 Scripts/faster-whisper-server.exe
    if let Some(py) = py_source {
        let py_path = std::path::Path::new(&py);
        if py_path.exists() {
            let parent = py_path.parent().ok_or_else(|| {
                VivianError::Speech(format!("无法解析 Python 路径父目录: {py}"))
            })?;
            #[cfg(target_os = "windows")]
            let script = parent.join("Scripts").join("faster-whisper-server.exe");
            #[cfg(not(target_os = "windows"))]
            let script = parent.join("bin").join("faster-whisper-server");
            if script.exists() {
                let env_extras = vec![
                    ("FWS_CONFIG_PATH", fws_config_env.clone()),
                    ("HF_ENDPOINT", hf_endpoint.clone()),
                ];
                return Ok((script.to_string_lossy().to_string(), server_args, None, env_extras));
            }
        }
    }

    // 模式 3：PATH 查找 faster-whisper-server
    let env_extras = vec![
        ("FWS_CONFIG_PATH", fws_config_env.clone()),
        ("HF_ENDPOINT", hf_endpoint.clone()),
    ];
    Ok(("faster-whisper-server".to_string(), server_args, None, env_extras))
}

/// 查找 Python 解释器路径
///
/// 优先级：config.service_python_path > PATH 中的 python > Windows py launcher
fn find_python(config: &WhisperConfig) -> VivianResult<String> {
    if let Some(py) = config.service_python_path.as_deref().filter(|s| !s.is_empty()) {
        if std::path::Path::new(py).exists() {
            return Ok(py.to_string());
        }
        tracing::warn!(
            "[Whisper] 配置的 Python 路径不存在: {py}，回退到系统 PATH 查找"
        );
    }
    find_python_in_path()
}

/// 在系统 PATH 中查找 Python 解释器
fn find_python_in_path() -> VivianResult<String> {
    #[cfg(target_os = "windows")]
    let candidates = ["python.exe", "python", "py.exe"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["python3", "python"];
    for c in candidates {
        if let Ok(output) = silent_command(c).arg("--version").output() {
            if output.status.success() {
                return Ok(c.to_string());
            }
        }
    }
    Err(VivianError::Speech(
        "未找到 Python 解释器。请安装 Python 3.9+ 并加入系统 PATH，或在下方「Python 路径」中填写 python.exe 完整路径。".into(),
    ))
}

/// 检测 faster-whisper-server 是否已安装（尝试 import 模块）
fn is_faster_whisper_server_installed(python: &str, install_path: Option<&str>) -> bool {
    let mut cmd = silent_command(python);
    cmd.args(["-c", "import faster_whisper_server"]);
    // --target 安装场景：把 install_path 加入 PYTHONPATH
    if let Some(path) = install_path.filter(|s| !s.is_empty()) {
        cmd.env("PYTHONPATH", path);
    }
    let ok = cmd.output();
    matches!(ok, Ok(o) if o.status.success())
}

/// 执行 pip install faster-whisper-server
///
/// 安装策略：
/// 1. 优先透传应用代理配置（HTTPS_PROXY/HTTP_PROXY）——让 pip 走用户配置的境外代理直连 pypi.org
/// 2. 代理未配置或失败时回退到国内镜像源（清华 → 阿里）+ trusted-host
///
/// `install_path`：若提供则用 `--target` 安装到指定目录（解决 user site-packages 权限问题，
///                 也便于 PYTHONPATH 隔离）；否则用默认 user 安装
async fn install_faster_whisper_server(
    python: &str,
    proxy_url: Option<&str>,
    install_path: Option<&str>,
) -> VivianResult<()> {
    // 共用参数
    let mut base_args: Vec<String> = vec!["-m".into(), "pip".into(), "install".into(), "--upgrade".into()];
    base_args.push("faster-whisper-server".into());
    if let Some(path) = install_path.filter(|s| !s.is_empty()) {
        base_args.push("--target".into());
        base_args.push(path.into());
        // --target 安装时禁止 pip 写入 user site-packages，避免冲突
        base_args.push("--no-user".into());
    }

    // 尝试 1：有代理时直接用官方源 + 代理
    if let Some(purl) = proxy_url.filter(|s| !s.is_empty()) {
        tracing::info!(
            "[Whisper] 自动安装 faster-whisper-server (代理 + 官方源): {} {}",
            python,
            base_args.join(" ")
        );
        let output = silent_command_async(python)
            .args(&base_args)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("HTTPS_PROXY", purl)
            .env("HTTP_PROXY", purl)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| VivianError::Speech(format!("执行 pip install 失败: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stdout.lines() {
            tracing::info!("[Whisper] pip: {line}");
        }
        for line in stderr.lines() {
            tracing::warn!("[Whisper] pip: {line}");
        }
        if output.status.success() {
            tracing::info!("[Whisper] faster-whisper-server 安装完成 (代理: {})", purl);
            return Ok(());
        }
        tracing::warn!(
            "[Whisper] 代理 + 官方源失败 (exit={}),回退到国内镜像源",
            output.status
        );
    }

    // 尝试 2：国内镜像源兜底（清华 → 阿里）
    const MIRRORS: [(&str, &str); 2] = [
        ("https://pypi.tuna.tsinghua.edu.cn/simple", "pypi.tuna.tsinghua.edu.cn"),
        ("https://mirrors.aliyun.com/pypi/simple", "mirrors.aliyun.com"),
    ];

    let mut last_err: Option<String> = None;
    for (idx, (index_url, host)) in MIRRORS.iter().enumerate() {
        let mut args = base_args.clone();
        args.push("-i".into());
        args.push((*index_url).to_string());
        args.push("--trusted-host".into());
        args.push((*host).to_string());
        tracing::info!(
            "[Whisper] 自动安装 faster-whisper-server (镜像 {}/{}): {} {}",
            idx + 1,
            MIRRORS.len(),
            python,
            args.join(" ")
        );
        let mut cmd = silent_command_async(python);
        cmd.args(&args)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8");
        // 镜像源走代理可能反而更慢/失败，仅在镜像本身被墙时才透传代理
        if let Some(purl) = proxy_url.filter(|s| !s.is_empty()) {
            cmd.env("HTTPS_PROXY", purl).env("HTTP_PROXY", purl);
        }
        let output = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| VivianError::Speech(format!("执行 pip install 失败: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stdout.lines() {
            tracing::info!("[Whisper] pip: {line}");
        }
        for line in stderr.lines() {
            tracing::warn!("[Whisper] pip: {line}");
        }
        if output.status.success() {
            tracing::info!("[Whisper] faster-whisper-server 安装完成 (镜像: {})", index_url);
            return Ok(());
        }
        last_err = Some(format!(
            "镜像 {index_url} 失败 (exit={}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Err(VivianError::Speech(format!(
        "pip install faster-whisper-server 全部安装策略失败\n{}\n常见原因：网络问题、Python 版本不兼容、缺少编译工具链",
        last_err.unwrap_or_default()
    )))
}

/// 自动检测 Python 并安装 faster-whisper-server（如未安装）
///
/// `proxy_url`：应用层解析的代理 URL，透传给 pip 作为 HTTPS_PROXY/HTTP_PROXY。
/// 返回找到的 Python 路径，供 launch 推导脚本位置。
///
/// 若 `config.service_install_path` 为空，则使用应用数据目录下的 `python-libs/`
/// 作为 `--target` 安装目录（避免 user site-packages 权限问题）。
async fn ensure_installed(config: &WhisperConfig, proxy_url: Option<&str>) -> VivianResult<String> {
    let python = find_python(config)?;
    // 解析实际安装目录：用户配置 → 默认应用数据目录
    let default_install_path = default_install_path();
    let install_path: Option<&str> = config
        .service_install_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(default_install_path.as_str()));
    if is_faster_whisper_server_installed(&python, install_path) {
        tracing::info!("[Whisper] faster-whisper-server 已安装，跳过自动安装");
        return Ok(python);
    }
    tracing::info!("[Whisper] 未检测到 faster-whisper-server，开始自动安装...");
    install_faster_whisper_server(&python, proxy_url, install_path).await?;
    // 安装后二次确认
    if !is_faster_whisper_server_installed(&python, install_path) {
        return Err(VivianError::Speech(
            "安装已完成但仍无法导入 faster_whisper_server，请检查 Python 环境或手动执行 `pip install faster-whisper-server`".into(),
        ));
    }
    Ok(python)
}

/// 默认安装目录：应用数据目录下的 `python-libs/`
fn default_install_path() -> String {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let p = base.join("vivian").join("python-libs");
    let _ = std::fs::create_dir_all(&p);
    p.to_string_lossy().to_string()
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
            tracing::warn!("[Whisper] 端口 {port} 已被占用,尝试清理占用进程");
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
        tracing::info!("[Whisper] kill 占用端口 {port} 的进程 pid={pid}");
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
            tracing::info!("[Whisper] kill 占用端口 {port} 的进程 pid={pid}");
            let _ = silent_command_async("kill")
                .args(["-9", &pid.to_string()])
                .output()
                .await;
        }
    }
    Ok(())
}

static SERVICE: tokio::sync::OnceCell<Arc<WhisperServiceManager>> =
    tokio::sync::OnceCell::const_new();

/// 获取全局 Whisper 服务管理器单例
pub async fn service() -> &'static Arc<WhisperServiceManager> {
    SERVICE
        .get_or_init(|| async { Arc::new(WhisperServiceManager::new()) })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_invocation_uses_path_when_no_python() {
        let cfg = WhisperConfig::default();
        let (program, args, cwd, _envs) = resolve_invocation(&cfg, None).unwrap();
        assert_eq!(program, "faster-whisper-server");
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"small".to_string()));
        assert!(args.contains(&"--port".to_string()));
        assert!(args.contains(&"8000".to_string()));
        assert!(cwd.is_none());
    }

    #[test]
    fn test_resolve_invocation_overrides_defaults() {
        let mut cfg = WhisperConfig::default();
        cfg.service_model = Some("large-v3".into());
        cfg.service_port = Some(9000);
        cfg.service_device = Some("cuda".into());
        cfg.service_compute_type = Some("float16".into());
        let (_, args, _, _) = resolve_invocation(&cfg, None).unwrap();
        assert!(args.contains(&"large-v3".to_string()));
        assert!(args.contains(&"9000".to_string()));
        assert!(args.contains(&"cuda".to_string()));
        assert!(args.contains(&"float16".to_string()));
    }

    #[test]
    fn test_state_snapshot_default() {
        let mgr = WhisperServiceManager::new();
        let st = mgr.state();
        assert_eq!(st.status, WhisperServiceStatus::Stopped);
        assert!(st.pid.is_none());
    }
}
