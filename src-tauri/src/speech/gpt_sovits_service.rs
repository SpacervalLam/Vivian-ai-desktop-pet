//! GPT-SoVITS 服务子进程管理 — 一键启动 / 停止 / 状态查询
//!
//! 支持单实例和双实例模式：
//! - 单实例：启动一个 api_v2.py 进程（默认端口 9880），两个角色共享同一服务（GPU 推理串行排队）
//! - 双实例：启动两个独立 api_v2.py 进程（端口 9880 + 9881），两个角色各自连接自己的实例实现真正并行
//!
//! 对齐 `api_v2.py` 实际 CLI 参数(仅 3 个):
//!   -c <config_path>    TTS 配置文件路径(默认 GPT_SoVITS/configs/tts_infer.yaml)
//!   -a <bind_ip>        监听地址(默认 127.0.0.1)
//!   -p <port>           监听端口(默认 9880)
//!
//! 模型路径 / GPU 等通过 tts_infer.yaml 配置文件传递:
//! - 若用户配置了 gpt_sovits_config_path,直接使用该文件
//! - 否则若用户配置了模型路径/GPU/device,自动生成临时 yaml 到 %APPDATA%/Vivian/gpt_sovits_tts_infer.yaml
//! - 否则使用 GPT-SoVITS 默认的 GPT_SoVITS/configs/tts_infer.yaml

use std::collections::HashMap;
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
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

/// 单个实例的信息（用于前端展示多实例状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub port: u16,
    pub status: ServiceStatus,
    pub pid: Option<u32>,
    pub endpoint: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    pub status: ServiceStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub command_line: Option<String>,
    pub endpoint: Option<String>,
    /// 双实例模式下每个实例的详细状态；单实例模式下只有一个条目
    #[serde(default)]
    pub instances: Vec<InstanceInfo>,
    /// 是否为双实例模式
    #[serde(default)]
    pub dual_instance: bool,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            status: ServiceStatus::Stopped,
            pid: None,
            error: None,
            command_line: None,
            endpoint: None,
            instances: Vec::new(),
            dual_instance: false,
        }
    }
}

/// 单个实例的内部状态（不包含 instances 列表，避免递归）
#[derive(Debug, Clone)]
struct InstanceLocalState {
    status: ServiceStatus,
    pid: Option<u32>,
    error: Option<String>,
    command_line: Option<String>,
    endpoint: Option<String>,
}

impl Default for InstanceLocalState {
    fn default() -> Self {
        Self {
            status: ServiceStatus::Stopped,
            pid: None,
            error: None,
            command_line: None,
            endpoint: None,
        }
    }
}

/// 单个服务实例槽位
struct InstanceSlot {
    port: u16,
    child: AsyncMutex<Option<Child>>,
    state: Mutex<InstanceLocalState>,
    stderr_buf: Arc<Mutex<String>>,
    endpoint: String,
}

/// GPT-SoVITS 服务管理器(全局单例,支持管理 1~2 个实例)
pub struct GptSoVitsServiceManager {
    instances: Arc<AsyncMutex<HashMap<u16, Arc<InstanceSlot>>>>,
    /// 最近一次启动时使用的配置（用于双实例模式）
    warmup_config: Arc<Mutex<Option<WarmupConfig>>>,
    cached_state: Arc<RwLock<ServiceState>>,
}

impl GptSoVitsServiceManager {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(AsyncMutex::new(HashMap::new())),
            warmup_config: Arc::new(Mutex::new(None)),
            cached_state: Arc::new(RwLock::new(ServiceState::default())),
        }
    }

    pub fn state(&self) -> ServiceState {
        self.cached_state.read().clone()
    }

    /// 获取当前所有实例的聚合状态
    pub async fn get_aggregated_state(&self) -> ServiceState {
        let instances = self.instances.lock().await;
        if instances.is_empty() {
            let state = ServiceState::default();
            *self.cached_state.write() = state.clone();
            return state;
        }

        let mut infos = Vec::new();
        let mut any_running = false;
        let mut any_starting = false;
        let mut any_stopping = false;
        let mut all_crashed = true;
        let mut primary_pid = None;
        let mut primary_endpoint = None;
        let mut primary_error = None;
        let mut primary_cmdline = None;
        let dual = instances.len() > 1;

        let mut slots: Vec<_> = instances.values().collect();
        slots.sort_by_key(|s| s.port);

        for (i, slot) in slots.iter().enumerate() {
            let s = slot.state.lock();
            infos.push(InstanceInfo {
                port: slot.port,
                status: s.status,
                pid: s.pid,
                endpoint: slot.endpoint.clone(),
                error: s.error.clone(),
            });

            match s.status {
                ServiceStatus::Running => {
                    any_running = true;
                    all_crashed = false;
                }
                ServiceStatus::Starting => {
                    any_starting = true;
                    all_crashed = false;
                }
                ServiceStatus::Stopping => {
                    any_stopping = true;
                    all_crashed = false;
                }
                _ => {}
            }

            if i == 0 {
                primary_pid = s.pid;
                primary_endpoint = s.endpoint.clone();
                primary_error = s.error.clone();
                primary_cmdline = s.command_line.clone();
            }
        }

        let aggregated = if any_running {
            ServiceStatus::Running
        } else if any_stopping {
            ServiceStatus::Stopping
        } else if any_starting {
            ServiceStatus::Starting
        } else if all_crashed {
            ServiceStatus::Crashed
        } else {
            ServiceStatus::Stopped
        };

        let state = ServiceState {
            status: aggregated,
            pid: primary_pid,
            error: primary_error,
            command_line: primary_cmdline,
            endpoint: primary_endpoint,
            instances: infos,
            dual_instance: dual,
        };
        *self.cached_state.write() = state.clone();
        state
    }

    /// 启动服务（支持单/双实例）
    pub async fn start(self: &Arc<Self>, config: &TtsConfig) -> VivianResult<ServiceState> {
        // 先停止已有实例（如果存在）
        let existing_instances = {
            let instances = self.instances.lock().await;
            instances.len()
        };
        if existing_instances > 0 {
            tracing::info!("[GPT-SoVITS] 已有实例运行，先停止...");
            self.stop_internal().await;
        }

        let install_path = config
            .gpt_sovits_install_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("未配置 GPT-SoVITS 安装路径".to_string()))?;

        let install_dir = std::path::Path::new(install_path);
        if !install_dir.is_dir() {
            return Err(VivianError::Speech(format!(
                "GPT-SoVITS 安装路径不存在或不是目录: {install_path}"
            )));
        }

        let api_v2 = install_dir.join("api_v2.py");
        if !api_v2.exists() {
            return Err(VivianError::Speech(format!(
                "在安装路径下找不到 api_v2.py: {}",
                api_v2.display()
            )));
        }

        // 确定要启动的端口列表
        let primary_port = config.gpt_sovits_port.unwrap_or(9880);
        let mut ports = vec![primary_port];
        if config.gpt_sovits_dual_instance {
            let second_port = config.gpt_sovits_second_port.unwrap_or(9881);
            if second_port != primary_port {
                ports.push(second_port);
            }
        }

        let bind_ip = "127.0.0.1";

        // 清理上次崩溃残留的孤儿进程（按端口区分）
        // exe 名用于 PID 复用防护：仅清理可执行名匹配的进程
        let python_exe = config
            .gpt_sovits_python_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "python".to_string());
        let orphan_exe = crate::utils::pid_file::normalize_exe_name(&python_exe);
        for &port in &ports {
            pid_file::cleanup_orphan(&format!("gpt_sovits_{port}"), &orphan_exe);
        }

        // 检查所有端口是否可用
        for &port in &ports {
            if let Err(e) = ensure_port_available(port).await {
                return Err(VivianError::Speech(format!(
                    "端口 {port} 已被占用且无法释放: {e}\n请手动结束占用端口的进程后重试"
                )));
            }
        }

        // 预下载 NLTK 数据
        ensure_nltk_data(&python_exe, install_dir).await;

        // 决定配置文件路径
        let config_path = resolve_config_path(config, install_dir)?;

        // 预热配置
        let warmup_config = WarmupConfig::from_tts_config(config);
        *self.warmup_config.lock() = Some(warmup_config.clone());

        // 启动所有实例
        let mut slots = HashMap::new();
        for &port in &ports {
            let endpoint = format!("http://{bind_ip}:{port}");
            let slot = Arc::new(InstanceSlot {
                port,
                child: AsyncMutex::new(None),
                state: Mutex::new(InstanceLocalState::default()),
                stderr_buf: Arc::new(Mutex::new(String::new())),
                endpoint: endpoint.clone(),
            });

            // 拼接参数
            let mut args: Vec<String> = vec!["api_v2.py".to_string()];
            args.push("-c".into());
            args.push(config_path.to_string_lossy().into());
            args.push("-a".into());
            args.push(bind_ip.into());
            args.push("-p".into());
            args.push(port.to_string());

            let cmdline = format!("{} {} (cwd={})", python_exe, args.join(" "), install_path);
            tracing::info!("[GPT-SoVITS] 启动实例 port={}: {}", port, cmdline);

            let mut cmd = silent_command_async(&python_exe);
            cmd.args(&args)
                .current_dir(install_dir)
                .env("PYTHONUNBUFFERED", "1")
                .env("PYTHONIOENCODING", "utf-8")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            let mut child = cmd
                .spawn()
                .map_err(|e| VivianError::Speech(format!("启动 GPT-SoVITS 服务(port={port})失败: {e}")))?;

            let pid = child.id();
            assign_child_to_job(&child);
            if let Some(pid) = pid {
                pid_file::write_pid(&format!("gpt_sovits_{port}"), pid);
            }

            // stdout/stderr 处理
            if let Some(stdout) = child.stdout.take() {
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let reader = tokio::io::BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::info!("[GPT-SoVITS:{port}] stdout: {line}");
                    }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                let buf = Arc::clone(&slot.stderr_buf);
                let p = port;
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let reader = tokio::io::BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::warn!("[GPT-SoVITS:{p}] stderr: {line}");
                        let mut b = buf.lock();
                        if b.len() > 2000 {
                            b.clear();
                        }
                        b.push_str(&line);
                        b.push('\n');
                    }
                });
            }

            // 更新 slot 状态
            {
                let mut s = slot.state.lock();
                s.status = ServiceStatus::Starting;
                s.pid = pid;
                s.error = None;
                s.command_line = Some(cmdline);
                s.endpoint = Some(endpoint.clone());
            }
            {
                let mut guard = slot.child.lock().await;
                *guard = Some(child);
            }

            slots.insert(port, slot);
        }

        // 将所有 slot 注册到管理器
        {
            let mut instances = self.instances.lock().await;
            *instances = slots;
        }

        // 为每个实例启动健康检查后台任务
        let manager = Arc::clone(self);
        for &port in &ports {
            let mgr = Arc::clone(&manager);
            let wc = warmup_config.clone();
            tokio::spawn(async move {
                if let Some(slot) = {
                    let instances = mgr.instances.lock().await;
                    instances.get(&port).cloned()
                } {
                    Self::wait_for_health_for_instance(slot, wc).await;
                }
            });
        }

        Ok(self.get_aggregated_state().await)
    }

    /// 内部停止方法（不获取额外锁）
    async fn stop_internal(&self) {
        let mut instances = self.instances.lock().await;
        for (port, slot) in instances.drain() {
            {
                let mut state = slot.state.lock();
                state.status = ServiceStatus::Stopping;
            }

            let mut guard = slot.child.lock().await;
            if let Some(mut child) = guard.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            pid_file::remove_pid(&format!("gpt_sovits_{port}"));

            {
                let mut state = slot.state.lock();
                state.status = ServiceStatus::Stopped;
                state.pid = None;
                state.error = None;
            }
            tracing::info!("[GPT-SoVITS] 实例 port={port} 已停止");
        }
    }

    /// 停止所有实例
    pub async fn stop(&self) -> VivianResult<ServiceState> {
        self.stop_internal().await;
        *self.warmup_config.lock() = None;
        Ok(ServiceState::default())
    }

    /// 刷新所有实例状态（检测进程是否存活）
    pub async fn refresh(&self) -> ServiceState {
        let instances = self.instances.lock().await;
        for (_port, slot) in instances.iter() {
            let mut guard = slot.child.lock().await;
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(status)) => {
                        let mut s = slot.state.lock();
                        if s.status != ServiceStatus::Stopping {
                            s.status = ServiceStatus::Crashed;
                            s.error = Some(format!("子进程退出 code={}", status.code().unwrap_or(-1)));
                            s.pid = None;
                        }
                        *guard = None;
                    }
                    Err(e) => {
                        let mut s = slot.state.lock();
                        s.status = ServiceStatus::Crashed;
                        s.error = Some(format!("查询子进程状态失败: {e}"));
                        s.pid = None;
                        *guard = None;
                    }
                }
            } else {
                let mut s = slot.state.lock();
                if s.status == ServiceStatus::Starting || s.status == ServiceStatus::Running {
                    s.status = ServiceStatus::Stopped;
                }
            }
        }
        drop(instances);
        self.get_aggregated_state().await
    }

    /// 对单个实例执行健康检查等待
    async fn wait_for_health_for_instance(slot: Arc<InstanceSlot>, warmup_config: WarmupConfig) {
        let endpoint = slot.endpoint.clone();
        let port = slot.port;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let url = format!("{}/", endpoint.trim_end_matches('/'));
        let tcp_addr = endpoint
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/');

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut last_err = String::new();
        let mut consecutive_tcp_fail = 0u32;

        loop {
            if std::time::Instant::now() >= deadline {
                let mut s = slot.state.lock();
                s.status = ServiceStatus::Crashed;
                let timeout_hint = if last_err.to_lowercase().contains("not enough memory")
                    || last_err.to_lowercase().contains("defaultcpuallocator")
                {
                    friendly_error(&last_err, port)
                } else {
                    format!(
                        "启动超时(60s) port={port}: {last_err}\n可能原因: 模型加载内存不足、端口被占用、或 Python 运行时缺少依赖"
                    )
                };
                s.error = Some(timeout_hint);
                tracing::warn!("[GPT-SoVITS:{port}] 服务启动超时,最后错误: {last_err}");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

            // 检查子进程是否已退出
            {
                let mut guard = slot.child.lock().await;
                if let Some(child) = guard.as_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code().unwrap_or(-1);
                            let err_msg = {
                                let guard = slot.stderr_buf.lock();
                                guard.trim().to_string()
                            };
                            let error = if err_msg.is_empty() {
                                format!(
                                    "服务进程退出 code={}(无输出) port={port}。可能原因:Python 运行时缺失依赖、模型文件损坏、或内存不足",
                                    code
                                )
                            } else {
                                let extracted = extract_error(&err_msg);
                                if extracted.is_empty() {
                                    let tail: String = err_msg.lines().rev().take(5).collect::<Vec<_>>().join("\n");
                                    format!("code={} port={port}\n{}", code, tail)
                                } else {
                                    friendly_error(&extracted, port)
                                }
                            };
                            let mut s = slot.state.lock();
                            s.status = ServiceStatus::Crashed;
                            s.error = Some(error);
                            s.pid = None;
                            *guard = None;
                            tracing::warn!("[GPT-SoVITS:{port}] 服务进程在启动期间退出 code={}", code);
                            return;
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                } else {
                    return;
                }
            }

            // TCP 探测
            let tcp_ok = tokio::net::TcpStream::connect(tcp_addr).await.is_ok();
            if !tcp_ok {
                consecutive_tcp_fail += 1;
                if consecutive_tcp_fail >= 8 {
                    {
                        let mut s = slot.state.lock();
                        s.status = ServiceStatus::Crashed;
                        s.error = Some(format!(
                            "服务进程存活但端口 {tcp_addr} 无法连接(连续 {consecutive_tcp_fail} 次, port={port})。可能原因: 模型加载内存不足、GPU OOM、或进程卡死"
                        ));
                        s.pid = None;
                    }
                    if let Some(mut child) = slot.child.lock().await.take() {
                        let _ = child.start_kill();
                    }
                    tracing::warn!("[GPT-SoVITS:{port}] TCP 连续失败,判定进程卡死");
                    return;
                }
            } else {
                consecutive_tcp_fail = 0;
            }

            // HTTP 健康检查
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
                        {
                            let mut s = slot.state.lock();
                            s.status = ServiceStatus::Running;
                            tracing::info!("[GPT-SoVITS:{port}] 服务健康检查通过: {endpoint} (HTTP {})", status);
                        }
                        // 启动进程监控
                        let slot_for_monitor = Arc::clone(&slot);
                        tokio::spawn(async move {
                            Self::monitor_instance(slot_for_monitor).await;
                        });
                        // 预热模型
                        let ep = endpoint.clone();
                        let wc = warmup_config.clone();
                        tokio::spawn(async move {
                            Self::warmup_instance(&ep, &wc, port).await;
                        });
                        return;
                    } else {
                        last_err = format!("HTTP {}", status);
                    }
                }
                Err(e) => {
                    last_err = format!("HTTP请求失败: {e}");
                }
            }
        }
    }

    /// 监控单个实例的运行状态
    async fn monitor_instance(slot: Arc<InstanceSlot>) {
        let port = slot.port;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let mut guard = slot.child.lock().await;
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(status)) => {
                        let mut s = slot.state.lock();
                        s.status = ServiceStatus::Crashed;
                        s.error = Some(format!(
                            "服务进程异常退出 code={} port={port}",
                            status.code().unwrap_or(-1)
                        ));
                        s.pid = None;
                        *guard = None;
                        tracing::warn!("[GPT-SoVITS:{port}] 服务进程在运行期间退出");
                        return;
                    }
                    Err(e) => {
                        let mut s = slot.state.lock();
                        s.status = ServiceStatus::Crashed;
                        s.error = Some(format!("查询子进程状态失败 port={port}: {e}"));
                        s.pid = None;
                        *guard = None;
                        return;
                    }
                }
            } else {
                return;
            }
        }
    }

    /// 预热单个实例的模型
    async fn warmup_instance(endpoint: &str, config: &WarmupConfig, port: u16) {
        let Some(ref_audio) = config.ref_audio_path.as_deref() else {
            tracing::info!("[GPT-SoVITS:{port}] 跳过预热：未配置参考音频");
            return;
        };
        if ref_audio.is_empty() {
            tracing::info!("[GPT-SoVITS:{port}] 跳过预热：参考音频路径为空");
            return;
        }
        if !std::path::Path::new(ref_audio).exists() {
            tracing::warn!(
                "[GPT-SoVITS:{port}] 跳过预热：参考音频文件不存在: {}",
                ref_audio
            );
            return;
        }

        let url = format!("{}/tts", endpoint.trim_end_matches('/'));
        let warmup_text = "你好";
        let lang = "zh";
        let prompt_lang = config
            .prompt_lang
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(lang);
        let text_split_method = config
            .text_split_method
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("cut5");
        let parallel_infer = config.parallel_infer.unwrap_or(true);
        let speed_factor = config.speed_factor.unwrap_or(1.0);

        let mut body = serde_json::json!({
            "text": warmup_text,
            "text_lang": lang,
            "prompt_lang": prompt_lang,
            "media_type": "wav",
            "text_split_method": text_split_method,
            "speed_factor": speed_factor,
            "parallel_infer": parallel_infer,
            "ref_audio_path": ref_audio,
        });
        if let Some(prompt_text) = config.prompt_text.as_deref() {
            if !prompt_text.is_empty() {
                body["prompt_text"] = serde_json::json!(prompt_text);
            }
        }

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[GPT-SoVITS:{port}] 预热客户端构建失败: {e}");
                return;
            }
        };

        tracing::info!("[GPT-SoVITS:{port}] 开始预热模型（首次推理冷启动）...");
        let start = std::time::Instant::now();
        match client.post(&url).json(&body).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::info!(
                        "[GPT-SoVITS:{port}] 预热完成，耗时 {:.1}s（模型已驻留内存）",
                        start.elapsed().as_secs_f64()
                    );
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        "[GPT-SoVITS:{port}] 预热请求返回非 2xx: [{}] {}",
                        status,
                        body.chars().take(300).collect::<String>()
                    );
                }
            }
            Err(e) => {
                tracing::warn!("[GPT-SoVITS:{port}] 预热请求失败: {e}");
            }
        }
    }
}

/// 检查指定端口是否被占用,若被占用则尝试 kill 占用进程
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
            tracing::warn!("[GPT-SoVITS] 端口 {port} 已被占用,尝试清理占用进程");
            kill_port_occupant(port).await?;
            // Windows 强制 kill 进程后端口可能短暂处于 TIME_WAIT 或尚未释放，
            // 不能仅靠一次 300ms sleep 判定，需轮询直到端口释放或超时（5s）
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let recheck = tokio::time::timeout(
                    probe_timeout,
                    tokio::net::TcpStream::connect(&addr),
                )
                .await;
                match recheck {
                    Err(_) => return Ok(()),
                    Ok(Err(ref e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        return Ok(())
                    }
                    Ok(Ok(_)) => {
                        if std::time::Instant::now() >= deadline {
                            return Err(format!("kill 后端口 {port} 仍被占用"));
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                    Ok(Err(e)) => return Err(format!("重检端口失败: {e}")),
                }
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
        // 端口可连接但 netstat 找不到 LISTENING 进程，可能是进程刚退出
        // 端口尚未释放或处于 TIME_WAIT，返回 Ok 让上层轮询等待端口释放
        tracing::info!("[GPT-SoVITS] 端口 {port} 可连接但 netstat 未找到占用进程，等待端口释放");
        return Ok(());
    }
    for pid in &pids {
        tracing::info!("[GPT-SoVITS] kill 占用端口 {port} 的进程 pid={pid}");
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
            tracing::info!("[GPT-SoVITS] kill 占用端口 {port} 的进程 pid={pid}");
            let _ = silent_command_async("kill")
                .args(["-9", &pid.to_string()])
                .output()
                .await;
        }
    }
    Ok(())
}

/// 从 stderr 文本中提取最有价值的错误行
fn extract_error(stderr: &str) -> String {
    for line in stderr.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("RuntimeError")
            || trimmed.starts_with("Error")
            || trimmed.starts_with("Exception")
            || trimmed.starts_with("OSError")
            || trimmed.starts_with("MemoryError")
            || trimmed.starts_with("CUDA")
            || trimmed.contains("not enough memory")
            || trimmed.contains("CUDA out of memory")
        {
            return trimmed.to_string();
        }
    }
    let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join("\n");
    if tail.len() > 300 {
        tail[..300].to_string()
    } else {
        tail
    }
}

/// 将技术性错误信息包装为更友好的用户提示
fn friendly_error(raw: &str, port: u16) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("not enough memory") || lower.contains("defaultcpuallocator") {
        return format!(
            "内存不足：系统可用内存不够加载模型（port={port}）。\n请关闭其他占用内存的程序后重试。\n如果使用双实例模式，建议显存≥12GB、内存≥16GB。\n\n技术详情：{raw}"
        );
    }
    if lower.contains("cuda out of memory") || lower.contains("cuda error") {
        return format!(
            "显存不足：GPU 显存不够加载模型（port={port}）。\n请关闭其他占用 GPU 的程序，或在设置中切换到 CPU 推理（GPU 卡号填 -1）。\n双实例模式需要双倍显存。\n\n技术详情：{raw}"
        );
    }
    if lower.contains("no such file") || lower.contains("filenotfounderror") {
        return format!(
            "文件缺失：启动所需的模型或依赖文件不存在（port={port}）。\n请检查 GPT-SoVITS 安装路径和模型文件是否完整。\n\n技术详情：{raw}"
        );
    }
    format!("{raw} (port={port})")
}

/// 预热所需的 TTS 配置字段
#[derive(Debug, Clone)]
struct WarmupConfig {
    ref_audio_path: Option<String>,
    prompt_text: Option<String>,
    prompt_lang: Option<String>,
    text_split_method: Option<String>,
    parallel_infer: Option<bool>,
    speed_factor: Option<f64>,
}

impl WarmupConfig {
    fn from_tts_config(config: &TtsConfig) -> Self {
        Self {
            ref_audio_path: config.gpt_sovits_ref_audio.clone(),
            prompt_text: config.gpt_sovits_prompt_text.clone(),
            prompt_lang: config.gpt_sovits_prompt_lang.clone(),
            text_split_method: config.gpt_sovits_text_split_method.clone(),
            parallel_infer: config.gpt_sovits_parallel_infer,
            speed_factor: Some(config.rate),
        }
    }
}

/// 预下载 NLTK 数据
async fn ensure_nltk_data(python_exe: &str, install_dir: &std::path::Path) {
    let script = concat!(
        "import nltk\n",
        "for pkg in ['averaged_perceptron_tagger_eng', 'averaged_perceptron_tagger']:\n",
        "    try:\n",
        "        nltk.download(pkg, quiet=True)\n",
        "    except Exception as e:\n",
        "        print(f'download {pkg} failed: {e}')\n",
    );
    let mut cmd = silent_command_async(python_exe);
    cmd.current_dir(install_dir)
        .arg("-c")
        .arg(script)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    match cmd.output().await {
        Ok(output) => {
            if output.status.success() {
                tracing::info!("[GPT-SoVITS] NLTK 数据预下载完成");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    "[GPT-SoVITS] NLTK 数据预下载返回非零状态: {}",
                    stderr.trim()
                );
            }
        }
        Err(e) => {
            tracing::warn!("[GPT-SoVITS] 执行 NLTK 数据预下载命令失败: {e}");
        }
    }
}

/// 决定传递给 api_v2.py 的配置文件路径
fn resolve_config_path(
    config: &TtsConfig,
    install_dir: &std::path::Path,
) -> VivianResult<std::path::PathBuf> {
    if let Some(p) = config.gpt_sovits_config_path.as_deref() {
        if !p.is_empty() {
            let path = std::path::Path::new(p);
            if !path.exists() {
                return Err(VivianError::Speech(format!(
                    "配置文件不存在: {p}"
                )));
            }
            return Ok(path.to_path_buf());
        }
    }

    let has_model_config = config
        .gpt_sovits_gpt_model
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false)
        || config
            .gpt_sovits_sovits_model
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        || config.gpt_sovits_gpu.is_some();

    if has_model_config {
        return generate_tts_infer_yaml(config, install_dir);
    }

    let default_yaml = install_dir.join("GPT_SoVITS").join("configs").join("tts_infer.yaml");
    if default_yaml.exists() {
        Ok(default_yaml)
    } else {
        Err(VivianError::Speech(format!(
            "GPT-SoVITS 默认配置文件不存在: {}(请在安装路径下确认 GPT_SoVITS/configs/tts_infer.yaml)",
            default_yaml.display()
        )))
    }
}

/// 生成临时 tts_infer.yaml
fn generate_tts_infer_yaml(
    config: &TtsConfig,
    install_dir: &std::path::Path,
) -> VivianResult<std::path::PathBuf> {
    let default_yaml = install_dir.join("GPT_SoVITS").join("configs").join("tts_infer.yaml");

    let yaml_str = if default_yaml.exists() {
        std::fs::read_to_string(&default_yaml).map_err(|e| {
            VivianError::Speech(format!("读取默认配置文件失败: {}: {e}", default_yaml.display()))
        })?
    } else {
        DEFAULT_TTS_INFER_YAML.to_string()
    };

    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&yaml_str).map_err(|e| {
        VivianError::Speech(format!("解析配置文件失败: {}: {e}", default_yaml.display()))
    })?;

    if let serde_yaml::Value::Mapping(ref mut root) = yaml {
        if let Some(serde_yaml::Value::Mapping(ref mut custom)) = root.get_mut("custom") {
            if let Some(gpt) = config.gpt_sovits_gpt_model.as_deref() {
                if !gpt.is_empty() {
                    custom.insert(
                        serde_yaml::Value::String("t2s_weights_path".into()),
                        serde_yaml::Value::String(gpt.into()),
                    );
                }
            }
            if let Some(sovits) = config.gpt_sovits_sovits_model.as_deref() {
                if !sovits.is_empty() {
                    custom.insert(
                        serde_yaml::Value::String("vits_weights_path".into()),
                        serde_yaml::Value::String(sovits.into()),
                    );
                }
            }
            if let Some(gpu) = config.gpt_sovits_gpu {
                let (device, is_half) = if gpu < 0 {
                    ("cpu".to_string(), false)
                } else {
                    (format!("cuda:{}", gpu), true)
                };
                custom.insert(
                    serde_yaml::Value::String("device".into()),
                    serde_yaml::Value::String(device),
                );
                custom.insert(
                    serde_yaml::Value::String("is_half".into()),
                    serde_yaml::Value::Bool(is_half),
                );
            }
        }
    }

    let vivian_dir = crate::utils::path::get_user_data_dir();
    std::fs::create_dir_all(&vivian_dir).map_err(|e| {
        VivianError::Speech(format!("创建配置目录失败: {}: {e}", vivian_dir.display()))
    })?;
    let yaml_path = vivian_dir.join("gpt_sovits_tts_infer.yaml");

    let new_yaml_str = serde_yaml::to_string(&yaml).map_err(|e| {
        VivianError::Speech(format!("序列化配置文件失败: {e}"))
    })?;
    std::fs::write(&yaml_path, new_yaml_str).map_err(|e| {
        VivianError::Speech(format!("写入配置文件失败: {}: {e}", yaml_path.display()))
    })?;

    tracing::info!("[GPT-SoVITS] 生成配置文件: {}", yaml_path.display());
    Ok(yaml_path)
}

/// 内置默认 tts_infer.yaml 模板
const DEFAULT_TTS_INFER_YAML: &str = r#"custom:
  bert_base_path: GPT_SoVITS/pretrained_models/chinese-roberta-wwm-ext-large
  cnhuhbert_base_path: GPT_SoVITS/pretrained_models/chinese-hubert-base
  device: cpu
  is_half: false
  t2s_weights_path: GPT_SoVITS/pretrained_models/s1bert25hz-2kh-longer-epoch=68e-step=50232.ckpt
  version: v1
  vits_weights_path: GPT_SoVITS/pretrained_models/s2G488k.pth
v1:
  bert_base_path: GPT_SoVITS/pretrained_models/chinese-roberta-wwm-ext-large
  cnhuhbert_base_path: GPT_SoVITS/pretrained_models/chinese-hubert-base
  device: cpu
  is_half: false
  t2s_weights_path: GPT_SoVITS/pretrained_models/s1bert25hz-2kh-longer-epoch=68e-step=50232.ckpt
  version: v1
  vits_weights_path: GPT_SoVITS/pretrained_models/s2G488k.pth
"#;

static SERVICE: tokio::sync::OnceCell<Arc<GptSoVitsServiceManager>> =
    tokio::sync::OnceCell::const_new();

pub async fn service() -> &'static Arc<GptSoVitsServiceManager> {
    SERVICE
        .get_or_init(|| async { Arc::new(GptSoVitsServiceManager::new()) })
        .await
}
