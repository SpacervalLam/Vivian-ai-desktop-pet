//! Ollama 本地嵌入服务子进程管理 — 启动 / 停止 / 状态查询 / 模型拉取
//!
//! 精简版单实例管理器：固定端口 11434，无端口冲突处理，无双实例。

use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

use crate::utils::process::{assign_child_to_job, silent_command, silent_command_async};
use crate::utils::pid_file;

use crate::error::{VivianError, VivianResult};

/// Ollama 服务运行状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OllamaServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

/// Ollama 服务状态（返回给前端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaServiceState {
    pub status: OllamaServiceStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub endpoint: String,
}

impl Default for OllamaServiceState {
    fn default() -> Self {
        Self {
            status: OllamaServiceStatus::Stopped,
            pid: None,
            error: None,
            endpoint: "http://localhost:11434/v1".to_string(),
        }
    }
}

/// 模型拉取结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResult {
    pub success: bool,
    pub error: Option<String>,
    pub permission_denied: bool,
}

/// Ollama 服务管理器（全局单例）
pub struct OllamaServiceManager {
    state: AsyncMutex<OllamaServiceState>,
    child: AsyncMutex<Option<Child>>,
}

impl OllamaServiceManager {
    pub fn new() -> Self {
        Self {
            state: AsyncMutex::new(OllamaServiceState::default()),
            child: AsyncMutex::new(None),
        }
    }

    /// 启动 Ollama serve 进程
    pub async fn start(&self, ollama_path: &str) -> VivianResult<OllamaServiceState> {
        let mut state = self.state.lock().await;

        if state.status == OllamaServiceStatus::Running {
            return Ok(state.clone());
        }

        // 清理上次崩溃残留的孤儿进程
        pid_file::cleanup_orphan("ollama");

        // 检测外部已运行的 Ollama
        if Self::check_port().await {
            state.status = OllamaServiceStatus::Running;
            state.error = None;
            state.pid = None;
            tracing::info!("[OllamaService] 检测到外部 Ollama 已在运行");
            let ret = state.clone();
            // 不再在此 emit ollama:ready —— 由调用方（lib.rs）在
            // ensure_model_installed 完成后统一 emit
            return Ok(ret);
        }

        let path = ollama_path.trim();
        if path.is_empty() {
            state.status = OllamaServiceStatus::Crashed;
            state.error = Some("Ollama 路径未配置".to_string());
            return Ok(state.clone());
        }

        if !std::path::Path::new(path).exists() {
            state.status = OllamaServiceStatus::Crashed;
            state.error = Some(format!("Ollama 可执行文件不存在: {}", path));
            return Ok(state.clone());
        }

        state.status = OllamaServiceStatus::Starting;
        state.error = None;
        drop(state);

        let mut cmd = silent_command_async(path);
        cmd.arg("serve");
        cmd.kill_on_drop(true);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            VivianError::Other(format!("启动 Ollama 失败: {}", e))
        })?;

        let pid = child.id();
        assign_child_to_job(&child);
        if let Some(pid) = pid {
            pid_file::write_pid("ollama", pid);
        }
        {
            let mut child_slot = self.child.lock().await;
            *child_slot = Some(child);
        }
        {
            let mut state = self.state.lock().await;
            state.pid = pid;
        }

        self.spawn_health_check();

        let state = self.state.lock().await;
        Ok(state.clone())
    }

    /// 后台健康检查：轮询端口直到就绪或超时 30s
    fn spawn_health_check(&self) {
        tauri::async_runtime::spawn(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                if std::time::Instant::now() > deadline {
                    let svc = ollama_service().await;
                    let mut state = svc.state.lock().await;
                    if state.status == OllamaServiceStatus::Starting {
                        state.status = OllamaServiceStatus::Crashed;
                        state.error = Some("启动超时（30s）".to_string());
                    }
                    break;
                }

                if Self::check_port().await {
                    let svc = ollama_service().await;
                    let mut state = svc.state.lock().await;
                    if state.status == OllamaServiceStatus::Starting {
                        state.status = OllamaServiceStatus::Running;
                        state.error = None;
                        tracing::info!("[OllamaService] 健康检查通过，服务已就绪");
                        // 不再在此 emit ollama:ready —— 由调用方（lib.rs）在
                        // ensure_model_installed 完成后统一 emit，避免前端在
                        // 模型拉取开始前就收到 model_installed=false 的误报
                    }
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
    }

    /// 停止 Ollama 进程
    pub async fn stop(&self) -> VivianResult<OllamaServiceState> {
        {
            let mut state = self.state.lock().await;
            state.status = OllamaServiceStatus::Stopping;
        }

        let mut child_slot = self.child.lock().await;
        if let Some(child) = child_slot.as_mut() {
            let _ = child.kill().await;
        }
        *child_slot = None;
        pid_file::remove_pid("ollama");

        let mut state = self.state.lock().await;
        state.status = OllamaServiceStatus::Stopped;
        state.pid = None;
        state.error = None;
        Ok(state.clone())
    }

    /// 刷新状态：检测进程/端口存活
    pub async fn refresh(&self) -> OllamaServiceState {
        if Self::check_port().await {
            let mut state = self.state.lock().await;
            if state.status != OllamaServiceStatus::Running {
                state.status = OllamaServiceStatus::Running;
                state.error = None;
            }
            return state.clone();
        }

        let mut child_slot = self.child.lock().await;
        if let Some(child) = child_slot.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *child_slot = None;
                    let mut state = self.state.lock().await;
                    if state.status == OllamaServiceStatus::Running
                        || state.status == OllamaServiceStatus::Starting
                    {
                        state.status = OllamaServiceStatus::Crashed;
                        state.error = Some("Ollama 进程意外退出".to_string());
                        state.pid = None;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    *child_slot = None;
                }
            }
        }

        let state = self.state.lock().await;
        state.clone()
    }

    /// 列出已安装的 Ollama 模型
    pub async fn list_models() -> VivianResult<Vec<String>> {
        // localhost 本地服务禁止走系统代理，避免代理拦截返回非 JSON 内容
        // 导致误判为"模型未安装"
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| VivianError::Other(format!("构建 HTTP 客户端失败: {}", e)))?;
        let resp = client
            .get("http://localhost:11434/v1/models")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| VivianError::Other(format!("无法连接 Ollama: {}", e)))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VivianError::Other(format!("解析模型列表失败: {}", e)))?;

        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// 带重试的模型列表查询：连接失败（服务未就绪/瞬时不可达）时最多重试
    /// attempts 次（间隔 500ms）；全部失败返回空列表并记录 WARN。
    async fn list_models_retry(attempts: u32) -> Vec<String> {
        let mut last_err: Option<VivianError> = None;
        for i in 0..attempts {
            match Self::list_models().await {
                Ok(list) => return list,
                Err(e) => {
                    last_err = Some(e);
                    if i + 1 < attempts {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }
        if let Some(e) = last_err {
            tracing::warn!(
                "[OllamaService] 查询模型列表连续 {} 次失败: {}",
                attempts,
                e
            );
        }
        Vec::new()
    }

    /// 判断目标模型是否已安装：精确匹配或前缀匹配（"bge-m3" ↔ "bge-m3:latest"）
    ///
    /// 列表查询带重试，避免把"服务器尚未就绪的连接失败"误判为"模型未安装"。
    pub async fn is_model_installed(target: &str) -> bool {
        if target.is_empty() {
            return false;
        }
        let installed = Self::list_models_retry(3).await;
        let prefix = format!("{}:", target);
        installed.iter().any(|m| m == target || m.starts_with(&prefix))
    }

    /// 确保目标模型已安装：未安装则自动拉取，权限不足时自动触发 UAC 修复并重试一次
    ///
    /// 返回值表示最终模型是否可用（已安装）。
    pub async fn ensure_model_installed(model: &str, ollama_path: &str) -> bool {
        if model.is_empty() {
            return false;
        }
        LAST_PERMISSION_DENIED.store(false, std::sync::atomic::Ordering::SeqCst);

        // start() 异步启动服务、快速返回，此时端口可能尚未监听。
        // 先等待服务就绪再检查模型，否则连接失败会被误判为"未安装"，
        // 触发对已安装模型的无意义 pull。
        if !Self::wait_for_ready(std::time::Duration::from_secs(30)).await {
            tracing::warn!(
                "[OllamaService] 等待服务就绪超时，跳过模型 {} 安装检查",
                model
            );
            return false;
        }

        if Self::is_model_installed(model).await {
            return true;
        }

        tracing::info!("[OllamaService] 模型 {} 未安装，启动自动拉取", model);

        let mut result = match Self::pull_model(model, ollama_path).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[OllamaService] 自动拉取 {} 异常: {}", model, e);
                return false;
            }
        };

        if result.success {
            return Self::is_model_installed(model).await;
        }

        if result.permission_denied {
            LAST_PERMISSION_DENIED.store(true, std::sync::atomic::Ordering::SeqCst);
            tracing::warn!(
                "[OllamaService] 自动拉取 {} 权限不足，触发 UAC 修复目录权限",
                model
            );
            match Self::fix_permission(ollama_path).await {
                Ok(()) => {
                    match Self::pull_model(model, ollama_path).await {
                        Ok(r) => result = r,
                        Err(e) => {
                            tracing::warn!("[OllamaService] UAC 修复后重试拉取异常: {}", e);
                            return false;
                        }
                    }
                    if result.success {
                        tracing::info!("[OllamaService] UAC 修复后拉取 {} 成功", model);
                        return Self::is_model_installed(model).await;
                    }
                    tracing::warn!(
                        "[OllamaService] UAC 修复后拉取仍失败: {:?}",
                        result.error
                    );
                }
                Err(e) => {
                    // UAC 被用户取消
                    tracing::warn!("[OllamaService] UAC 权限修复被取消或失败: {}", e);
                }
            }
        } else {
            tracing::warn!(
                "[OllamaService] 自动拉取 {} 失败: {:?}",
                model,
                result.error
            );
        }

        false
    }

    /// 代理相关环境变量：子进程拉取模型时可能继承父进程代理设置，
    /// 导致本地代理（如 Clash/V2Ray）未运行时无法访问 registry.ollama.ai
    const PROXY_ENV_VARS: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];

    /// 判断错误文本是否为代理连接失败（可回退直连重试）
    fn is_proxy_connection_error(text: &str) -> bool {
        text.contains("proxyconnect tcp")
            || text.contains("dial tcp")
            || text.contains("connectex: No connection could be made")
            || text.contains("proxyconnect")
            || text.contains("connection refused")
            || text.contains("Client.Timeout")
    }

    /// 执行一次 ollama pull，返回 (PullResult, 是否因代理失败)
    async fn run_pull_once(
        model: &str,
        ollama_path: &str,
        clear_proxy: bool,
    ) -> VivianResult<(PullResult, bool)> {
        let mut cmd = silent_command_async(ollama_path);
        cmd.arg("pull").arg(model);
        if clear_proxy {
            for var in Self::PROXY_ENV_VARS {
                cmd.env_remove(var);
            }
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // 超时 drop future 时连带杀掉子进程，避免孤儿 ollama pull 驻留
        cmd.kill_on_drop(true);

        // 大模型下载可能很慢，但必须有上限，否则启动任务会无限期挂起
        let output = tokio::time::timeout(std::time::Duration::from_secs(600), cmd.output())
            .await
            .map_err(|_| VivianError::Other(format!("ollama pull {} 超时（600s）", model)))?
            .map_err(|e| VivianError::Other(format!("执行 ollama pull 失败: {}", e)))?;

        if output.status.success() {
            return Ok((
                PullResult {
                    success: true,
                    error: None,
                    permission_denied: false,
                },
                false,
            ));
        }

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let combined = if stdout.is_empty() {
            stderr.clone()
        } else {
            format!("{}\n{}", stdout, stderr)
        };
        let permission_denied = combined.contains("Access is denied")
            || combined.contains("Permission denied")
            || combined.contains("permission denied")
            || combined.contains("Required privilege")
            || combined.contains("not enough permissions");
        let proxy_failed = !clear_proxy && Self::is_proxy_connection_error(&combined);

        let truncated = if stderr.is_empty() && stdout.is_empty() {
            "ollama pull 失败（未知错误）".to_string()
        } else {
            let src = if stderr.is_empty() { &stdout } else { &stderr };
            if src.len() > 200 {
                let mut start = src.len() - 200;
                while start < src.len() && !src.is_char_boundary(start) {
                    start += 1;
                }
                format!("...{}", &src[start..])
            } else {
                src.clone()
            }
        };

        Ok((
            PullResult {
                success: false,
                error: Some(truncated),
                permission_denied,
            },
            proxy_failed,
        ))
    }

    /// 拉取模型（阻塞等待完成，捕获权限错误）
    ///
    /// 回退策略：首次继承父进程环境变量执行；若因代理连接失败，自动清除代理变量
    /// 直连重试一次。任何场景下代理连接失败都回退到直连。
    pub async fn pull_model(model: &str, ollama_path: &str) -> VivianResult<PullResult> {
        let path = ollama_path.trim();
        if path.is_empty() || !std::path::Path::new(path).exists() {
            return Ok(PullResult {
                success: false,
                error: Some("Ollama 路径无效".to_string()),
                permission_denied: false,
            });
        }

        let (result, proxy_failed) = Self::run_pull_once(model, path, false).await?;
        if result.success || !proxy_failed {
            return Ok(result);
        }

        tracing::warn!(
            "[OllamaService] ollama pull {} 因代理连接失败，回退到直连重试",
            model
        );
        let (result_retry, _) = Self::run_pull_once(model, path, true).await?;
        if result_retry.success {
            tracing::info!(
                "[OllamaService] ollama pull {} 直连重试成功",
                model
            );
        }
        Ok(result_retry)
    }

    /// 修复 Ollama models 目录权限（通过 UAC 提权执行 icacls）
    pub async fn fix_permission(ollama_path: &str) -> VivianResult<()> {
        let models_dir = std::path::Path::new(ollama_path)
            .parent()
            .map(|p| p.join("models"))
            .unwrap_or_else(|| std::path::PathBuf::from("G:\\ollama\\models"));

        let models_str = models_dir.to_string_lossy().to_string();

        #[cfg(windows)]
        {
            let icacls_args = format!("\"{}\" /grant Users:(OI)(CI)M /T", models_str);
            let ps_cmd = format!(
                "Start-Process icacls -ArgumentList '{}' -Verb RunAs -Wait",
                icacls_args
            );

            let status = silent_command("powershell")
                .args(["-Command", &ps_cmd])
                .status()
                .map_err(|e| VivianError::Other(format!("执行权限修复失败: {}", e)))?;

            if status.success() {
                tracing::info!("[OllamaService] 目录权限修复成功: {}", models_str);
                Ok(())
            } else {
                Err(VivianError::Other("权限修复被取消或失败".to_string()))
            }
        }

        #[cfg(not(windows))]
        {
            let _ = models_str;
            Err(VivianError::Other("权限修复仅支持 Windows".to_string()))
        }
    }

    /// TCP 探测 11434 端口
    async fn check_port() -> bool {
        tokio::net::TcpStream::connect("127.0.0.1:11434")
            .await
            .is_ok()
    }

    /// 轮询等待服务端口就绪，超时返回 false
    ///
    /// 服务状态已判定为 Crashed（路径无效/不存在等）时提前退出，不再空等。
    async fn wait_for_ready(timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if Self::check_port().await {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            let crashed = {
                let svc = ollama_service().await;
                let st = svc.state.lock().await;
                st.status == OllamaServiceStatus::Crashed
            };
            if crashed {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

// ── 全局单例 ──────────────────────────────────────────────────────────

static SERVICE: tokio::sync::OnceCell<Arc<OllamaServiceManager>> = tokio::sync::OnceCell::const_new();

/// 最近一次自动拉取是否因权限被拒
///
/// 由 ensure_model_installed 维护，ollama:ready 事件携带此标志，
/// 前端据此决定是否展示"权限修复"入口，避免非权限故障误导用户去提权。
static LAST_PERMISSION_DENIED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 获取 Ollama 服务管理器全局单例
pub async fn ollama_service() -> &'static Arc<OllamaServiceManager> {
    SERVICE
        .get_or_init(|| async { Arc::new(OllamaServiceManager::new()) })
        .await
}

static APP_HANDLE: Lazy<RwLock<Option<tauri::AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: tauri::AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 服务就绪时 emit `ollama:ready`，附带目标模型是否已安装的检测结果
///
/// 仅探测端口可达不足以判断嵌入模型可用，必须确认配置的目标模型已安装在本地，
/// 否则会误导用户以为嵌入服务可用。
pub async fn emit_ollama_ready_with_model_check() {
    let handle = APP_HANDLE.read().as_ref().cloned();
    let Some(handle) = handle else { return };

    // 仅向主窗口（非子 webview）emit
    let target_label = {
        let windows = handle.webview_windows();
        windows
            .iter()
            .find(|(_, w)| {
                w.url()
                    .map(|u| !u.as_str().contains("view="))
                    .unwrap_or(false)
            })
            .map(|(label, _)| label.clone())
    };
    let Some(label) = target_label else { return };

    // 读取配置中的目标模型名
    let target_model = handle
        .try_state::<std::sync::Arc<crate::state::AppState>>()
        .and_then(|s| {
            let cfg = s.config.read();
            let m = cfg.get_all().memory.embedding.ollama_model.clone();
            (!m.is_empty()).then_some(m)
        });

    // 拉取本地已安装模型列表（带重试，避免瞬时不可达误报"未安装"）
    let installed_models = OllamaServiceManager::list_models_retry(3).await;

    // 判断目标模型是否已安装：精确匹配或前缀匹配（"bge-m3" → "bge-m3:latest"）
    let model_installed = match &target_model {
        Some(target) => {
            let prefix = format!("{}:", target);
            installed_models
                .iter()
                .any(|m| m == target || m.starts_with(&prefix))
        }
        None => false,
    };

    if model_installed {
        tracing::info!(
            "[OllamaService] 目标模型 {:?} 已安装，emit ollama:ready",
            target_model
        );
    } else {
        tracing::warn!(
            "[OllamaService] 目标模型 {:?} 未安装（已安装: {:?}），emit ollama:ready（model_installed=false）",
            target_model,
            installed_models
        );
    }

    let _ = handle.emit_to(
        label.as_str(),
        "ollama:ready",
        serde_json::json!({
            "model_installed": model_installed,
            "model": target_model,
            "permission_denied": LAST_PERMISSION_DENIED.load(std::sync::atomic::Ordering::SeqCst),
        }),
    );
}
