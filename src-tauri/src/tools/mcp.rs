//! MCP (Model Context Protocol) 原生集成 — stdio 与 Streamable HTTP 客户端
//!
//! 设计目标：
//! - 不引入外部 MCP SDK，手写 JSON-RPC 2.0 over stdio（tokio::process）
//! - MCP 工具实现 `Tool` trait，与内置工具无差别调度
//! - 工具命名 `mcp__{server_id}__{tool_name}`，避免与内置工具冲突
//! - MCP 工具默认延迟加载（`should_defer=true`），通过 `tool_search` 按需唤起
//! - 外部工具不可信：`is_read_only=false` + `check_permissions` 返回 `ask`
//!
//! 协议参考：https://spec.modelcontextprotocol.io/
//! 实现 tools 能力、分页发现及 JSON/SSE HTTP 回执；resources/prompts 暂不支持。

use std::collections::{HashMap, HashSet};

#[path = "mcp_protocol.rs"]
mod protocol;
#[path = "mcp_http.rs"]
mod http;
use protocol::McpToolInfo;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::error::{VivianError, VivianResult};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};
use crate::utils::path::get_user_data_dir;
use crate::utils::process::silent_command_async;

const HEALTH_CHECK_INTERVAL_SECS: u64 = 60;

const PING_TIMEOUT_SECS: u64 = 5;

const MAX_CONSECUTIVE_FAILURES: u32 = 3;

const RECONNECT_BASE_DELAY_SECS: u64 = 15;

const RECONNECT_MAX_DELAY_SECS: u64 = 600;

const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// 服务器健康追踪器
///
/// 由 `McpManager` 和该服务器下所有 `McpTool` 共享：
/// - 工具调用失败时自增 `consecutive_failures`
/// - 后台健康检查 ping 失败时自增
/// - 成功调用 / 成功 ping 重置计数
/// - 达到阈值后标记 `evicted`，触发工具注销
pub struct ServerHealthTracker {
    server_id: String,
    consecutive_failures: AtomicU64,
    evicted: AtomicBool,
    circuit_open: AtomicBool,
    reconnect_attempts: AtomicU64,
    last_success_at: Mutex<Option<Instant>>,
    last_failure_at: Mutex<Option<Instant>>,
    last_error: Mutex<Option<String>>,
}

impl ServerHealthTracker {
    fn new(server_id: String) -> Self {
        Self {
            server_id,
            consecutive_failures: AtomicU64::new(0),
            evicted: AtomicBool::new(false),
            circuit_open: AtomicBool::new(false),
            reconnect_attempts: AtomicU64::new(0),
            last_success_at: Mutex::new(None),
            last_failure_at: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        *self.last_success_at.lock() = Some(Instant::now());
    }

    fn record_failure(&self, reason: String) -> u64 {
        let count = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_failure_at.lock() = Some(Instant::now());
        *self.last_error.lock() = Some(reason);
        count
    }

    fn failure_count(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    fn is_evicted(&self) -> bool {
        self.evicted.load(Ordering::Relaxed)
    }

    fn mark_evicted(&self) -> bool {
        self.evicted.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_ok()
    }

    fn is_circuit_open(&self) -> bool {
        self.circuit_open.load(Ordering::Relaxed)
    }

    fn reconnect_attempt_count(&self) -> u64 {
        self.reconnect_attempts.load(Ordering::Relaxed)
    }

    fn increment_reconnect_attempt(&self) -> u64 {
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn open_circuit(&self) {
        self.circuit_open.store(true, Ordering::Relaxed);
    }

    fn reset_evicted(&self) {
        self.evicted.store(false, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.reconnect_attempts.store(0, Ordering::Relaxed);
        self.circuit_open.store(false, Ordering::Relaxed);
    }

    fn next_reconnect_delay(&self) -> u64 {
        let attempts = self.reconnect_attempts.load(Ordering::Relaxed);
        let delay = RECONNECT_BASE_DELAY_SECS * (1u64 << attempts.min(6));
        delay.min(RECONNECT_MAX_DELAY_SECS)
    }

    fn server_id(&self) -> &str {
        &self.server_id
    }
}

/// MCP server 配置（单条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 唯一标识（用于工具名前缀和持久化键）
    pub id: String,
    /// 展示名
    pub name: String,
    /// 传输方式：目前仅支持 "stdio"
    #[serde(default = "default_transport")]
    pub transport: String,
    /// stdio：可执行文件路径或命令名（如 "npx" / "uvx" / "node"）
    #[serde(default)]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty", serialize_with = "crate::plugin_contract::serialize_string_map")]
    pub headers: HashMap<String, String>,
    /// 命令参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量
    #[serde(default, serialize_with = "crate::plugin_contract::serialize_string_map")]
    pub env: HashMap<String, String>,
    /// 工作目录
    #[serde(default)]
    pub cwd: Option<String>,
    /// 是否启用（false 则不连接）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 归属插件名（插件贡献点体系写入；`None` = 用户手动配置）
    ///
    /// 归属是插件运行时卸载/重载的依据：卸载插件时按此字段找到并撤销该插件
    /// 贡献的全部 server。存量 servers.json 无此字段的条目视为用户手配，
    /// 永不被插件操作触碰。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_plugin: Option<String>,
}

impl McpServerConfig {
    pub fn validate(&self) -> VivianResult<()> {
        if self.id.is_empty() || self.id.len() > 32 || self.id == "browser" || self.id.contains("__") || !self.id.chars().all(|c|c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(VivianError::Tool("MCP ID 需为 1–32 位字母、数字、- 或 _，不能使用 browser 或连续下划线".into()));
        }
        match self.transport.as_str() {
            "stdio" => {
                if self.command.trim().is_empty() { return Err(VivianError::Tool("MCP 启动命令不能为空".into())); }
                if self.cwd.as_ref().is_some_and(|p|!std::path::Path::new(p).is_dir()) { return Err(VivianError::Tool("MCP 工作目录不存在".into())); }
            },
            "http" => {
                let url = self.url.as_deref().and_then(|s|reqwest::Url::parse(s).ok()).ok_or_else(||VivianError::Tool("请输入有效的 MCP HTTP 地址".into()))?;
                if !["http", "https"].contains(&url.scheme()) || !url.username().is_empty() || url.password().is_some() { return Err(VivianError::Tool("MCP 地址需使用 http(s)，认证凭据请填写在请求头中".into())); }
                for (name, value) in &self.headers {
                    if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() || reqwest::header::HeaderValue::from_str(value).is_err() { return Err(VivianError::Tool("MCP HTTP 请求头格式无效".into())); }
                }
            },
            _ => return Err(VivianError::Tool("仅支持 stdio 或 Streamable HTTP 传输".into())),
        }
        Ok(())
    }
}

fn default_transport() -> String {
    "stdio".to_string()
}

fn default_enabled() -> bool {
    true
}

/// JSON-RPC 2.0 请求
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC 2.0 响应（仅取我们关心的字段）
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: u64,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    message: String,
}

/// MCP 客户端:stdin/pending/child 跨 await 持有用 AsyncMutex,tools 同步访问用 Mutex
struct McpClient {
    stdin: AsyncMutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>,
    next_id: AtomicU64,
    child: AsyncMutex<Option<Child>>,
    tools: Mutex<Vec<McpToolInfo>>,
    config: McpServerConfig,
    exited: AtomicBool,
    http: Option<http::HttpTransport>,
}

struct PendingGuard<'a> { pending: &'a Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>, id: u64 }
impl Drop for PendingGuard<'_> { fn drop(&mut self) { self.pending.lock().remove(&self.id); } }

impl McpClient {
    async fn start(config: McpServerConfig) -> VivianResult<Arc<Self>> {
        config.validate()?;
        if config.transport == "http" {
            let client = Arc::new(Self { stdin: AsyncMutex::new(None), pending: Mutex::new(HashMap::new()), next_id: AtomicU64::new(1), child: AsyncMutex::new(None), tools: Mutex::new(Vec::new()), config, exited: AtomicBool::new(false), http: Some(http::HttpTransport::new()?) });
            if let Err(error) = client.initialize().await { client.shutdown().await; return Err(error); }
            if let Err(error) = client.discover_tools().await { client.shutdown().await; return Err(error); }
            return Ok(client);
        }
        let mut cmd = silent_command_async(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| VivianError::Tool(format!("启动 MCP server 失败 [{}]: {e}", config.id)))?;

        // 绑定全局 Job Object：宿主崩溃/强杀时子进程由 OS 兜底回收
        // （kill_on_drop 只覆盖应用层正常清理路径）
        crate::utils::process::assign_child_to_job(&child);

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| VivianError::Tool("MCP server stdin 不可用".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VivianError::Tool("MCP server stdout 不可用".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| VivianError::Tool("MCP server stderr 不可用".into()))?;

        let client = Arc::new(Self {
            stdin: AsyncMutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            child: AsyncMutex::new(Some(child)),
            tools: Mutex::new(Vec::new()),
            config,
            exited: AtomicBool::new(false),
            http: None,
        });

        let stderr_id = client.config.id.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if !line.trim().is_empty() {
                    tracing::debug!("[MCP:{}] stderr: {}", stderr_id, line);
                }
            }
        });

        // 启动 stdout 读取线程
        let client_clone = Arc::clone(&client);
        tokio::spawn(async move {
            Self::read_loop(client_clone, stdout).await;
        });

        // 发送 initialize 握手
        if let Err(error) = client.initialize().await { client.shutdown().await; return Err(error); }

        // 发现工具
        if let Err(error) = client.discover_tools().await { client.shutdown().await; return Err(error); }

        Ok(client)
    }

    /// stdout 读取循环：逐行解析 JSON-RPC 响应，分发到 pending sender
    async fn read_loop(client: Arc<Self>, stdout: ChildStdout) {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    tracing::warn!("[MCP] server [{}] stdout EOF", client.config.id);
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("[MCP] server [{}] stdout 读取错误: {e}", client.config.id);
                    break;
                }
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(trimmed) {
                Ok(message) => message,
                Err(_) => continue,
            };
            if message.get("method").is_some() {
                if let Some(id) = message.get("id") {
                    let reply = if message["method"] == "ping" { json!({"jsonrpc":"2.0","id":id,"result":{}}) }
                        else { json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client method not supported"}}) };
                    let _ = tokio::time::timeout(Duration::from_secs(5), async {
                        let mut guard = client.stdin.lock().await;
                        if let Some(stdin) = guard.as_mut() {
                            let bytes = format!("{reply}\n");
                            stdin.write_all(bytes.as_bytes()).await?;
                            stdin.flush().await?;
                        }
                        Ok::<_, std::io::Error>(())
                    }).await;
                }
                continue;
            }
            let resp: JsonRpcResponse = match serde_json::from_value(message) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("[MCP] 非 JSON-RPC 行 [{}]: {e}", client.config.id);
                    continue;
                }
            };
            // 取出 pending sender 并发送
            let sender = {
                let mut pending = client.pending.lock();
                pending.remove(&resp.id)
            };
            if let Some(tx) = sender {
                let _ = tx.send(resp);
            }
        }
        // stdout 关闭后，清理所有 pending sender（让等待者收到错误）
        client.exited.store(true, Ordering::Relaxed);
        let mut pending = client.pending.lock();
        pending.clear();
    }

    fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed) || self.http.as_ref().is_some_and(|http|http.has_expired())
    }

    /// 发送 JSON-RPC 请求并等待响应（带超时）
    async fn request(&self, method: &str, params: Option<Value>) -> VivianResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        if let Some(http) = &self.http {
            let response = http.send(&self.config, serde_json::to_value(&req).map_err(|e|VivianError::Tool(e.to_string()))?).await?;
            if let Some(error) = response.get("error").filter(|e|!e.is_null()) { return Err(VivianError::Tool(format!("MCP 错误 [{}]: {}", method, error.get("message").and_then(Value::as_str).unwrap_or("unknown error")))); }
            return response.get("result").cloned().ok_or_else(||VivianError::Tool("MCP 响应无 result".into()));
        }
        let serialized = serde_json::to_string(&req)
            .map_err(|e| VivianError::Tool(format!("JSON-RPC 序列化失败: {e}")))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock();
            pending.insert(id, tx);
        }
        let _pending_guard = PendingGuard { pending: &self.pending, id };

        {
            let mut stdin_guard = self.stdin.lock().await;
            let stdin = stdin_guard
                .as_mut()
                .ok_or_else(|| VivianError::Tool("MCP server stdin 已关闭".into()))?;
            stdin
                .write_all(serialized.as_bytes())
                .await
                .map_err(|e| VivianError::Tool(format!("写入 MCP stdin 失败: {e}")))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| VivianError::Tool(format!("写入 MCP stdin 换行失败: {e}")))?;
            stdin.flush().await.map_err(|e| {
                VivianError::Tool(format!("flush MCP stdin 失败: {e}"))
            })?;
        }

        let resp = match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(r) => r.map_err(|_| VivianError::Tool("MCP 响应通道关闭".into()))?,
            Err(_) => {
                let _ = self.pending.lock().remove(&id);
                return Err(VivianError::Tool(format!("MCP 请求超时 [{}]", method)));
            }
        };

        if let Some(err) = resp.error {
            return Err(VivianError::Tool(format!(
                "MCP 错误 [{}]: {}",
                method, err.message
            )));
        }
        resp.result.ok_or_else(|| VivianError::Tool("MCP 响应无 result".into()))
    }

    /// 发送 ping 请求（带短超时），用于健康检查
    async fn ping(&self) -> VivianResult<()> {
        tokio::time::timeout(Duration::from_secs(PING_TIMEOUT_SECS), self.request("ping", None)).await
            .map_err(|_|VivianError::Tool("MCP ping 超时".into()))??;
        Ok(())
    }

    /// 发送 initialize 握手 + initialized 通知
    async fn initialize(&self) -> VivianResult<()> {
        let result = self
            .request(
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "vivian",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;
        tracing::info!(
            "[MCP] server [{}] 握手成功: {:?}",
            self.config.id,
            result.get("serverInfo")
        );
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
            if !["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"].contains(&version) { return Err(VivianError::Tool(format!("MCP 协议版本不兼容: {version}"))); }
            if let Some(http) = &self.http { http.set_version(version); }
        }
        // 发送 initialized 通知（id 为 null 的通知，无需等待响应）
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    /// 发送通知（不等待响应）
    async fn notify(&self, method: &str, params: Option<Value>) -> VivianResult<()> {
        let mut notification = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if let Some(params) = params { notification["params"] = params; }
        if let Some(http) = &self.http { http.send(&self.config, notification).await?; return Ok(()); }
        let serialized = serde_json::to_string(&notification)
            .map_err(|e| VivianError::Tool(format!("通知序列化失败: {e}")))?;
        let mut stdin_guard = self.stdin.lock().await;
        let stdin = stdin_guard
            .as_mut()
            .ok_or_else(|| VivianError::Tool("MCP stdin 已关闭".into()))?;
        stdin.write_all(serialized.as_bytes()).await.map_err(|e| {
            VivianError::Tool(format!("写入通知失败: {e}"))
        })?;
        stdin.write_all(b"\n").await.map_err(|e| {
            VivianError::Tool(format!("写入通知换行失败: {e}"))
        })?;
        Ok(())
    }

    /// 发现工具（listTools）
    async fn discover_tools(&self) -> VivianResult<()> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        loop {
            let result = self.request("tools/list", cursor.as_ref().map(|c|json!({"cursor":c}))).await?;
            let page: Vec<McpToolInfo> = serde_json::from_value(result.get("tools").cloned().ok_or_else(||VivianError::Tool("MCP tools/list 缺少 tools".into()))?)
                .map_err(|e|VivianError::Tool(format!("MCP 工具清单无效: {e}")))?;
            for tool in page {
                if !tool.name.is_empty() && !tools.iter().any(|t: &McpToolInfo|t.name == tool.name) { tools.push(tool); }
            }
            cursor = result.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            match &cursor {
                Some(c) if seen.insert(c.clone()) && seen.len() <= 100 => {},
                Some(_) => return Err(VivianError::Tool("MCP 工具分页游标重复或超过 100 页".into())),
                None => break,
            }
        }
        *self.tools.lock() = tools;
        Ok(())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> VivianResult<Value> {
        self.request("tools/call", Some(json!({"name": name, "arguments": arguments}))).await
    }

    /// 获取已发现的工具信息
    fn get_tools(&self) -> Vec<McpToolInfo> {
        self.tools.lock().clone()
    }

    /// 停止子进程
    ///
    /// 状态约束：本方法调用后，`child` 与 `stdin` 字段恒为 `None`，状态不可逆。
    /// 任何后续 `request` / `notify` 调用都会通过 `as_mut().ok_or_else(...)`
    /// 返回 `VivianError::Tool("MCP stdin 已关闭")`，不会 panic。
    async fn shutdown(&self) {
        self.exited.store(true, Ordering::Relaxed);
        if let Some(http) = &self.http { http.shutdown(&self.config).await; return; }
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
        }
        // take() 后 stdin 恒为 None；后续访问必须用 as_mut() 兜底，禁止 unwrap/expect
        let mut stdin_guard = self.stdin.lock().await;
        stdin_guard.take();
    }

    fn is_alive(&self) -> bool {
        if self.has_exited() { return false; }
        if self.http.is_some() { return true; }
        let child_guard = self.child.try_lock();
        if let Ok(guard) = child_guard {
            guard.is_some()
        } else {
            true // 锁忙时保守认为存活
        }
    }
}

/// 反应式驱逐回调 — 由 `McpManager` 注入，工具调用连续失败达阈值时触发
type EvictionCallback = Arc<dyn Fn() + Send + Sync>;

/// MCP 工具适配器 — 将单个 MCP server 工具包装为 `Tool` trait 实现
pub struct McpTool {
    /// 工具名（`mcp__{server_id}__{tool_name}`）
    tool_name: String,
    /// 工具描述
    tool_desc: String,
    /// 输入参数 JSON Schema
    schema: Value,
    /// search_hint（从 description 截取）
    hint: String,
    /// MCP 客户端引用
    client: Arc<McpClient>,
    /// MCP server 内的工具名（原始名，不含前缀）
    mcp_tool_name: String,
    /// 所属 server 的健康追踪器
    health: Arc<ServerHealthTracker>,
    /// 驱逐回调（连续失败达阈值时触发，注销该 server 的全部工具）
    eviction_callback: EvictionCallback,
}

impl McpTool {
    fn new(
        client: Arc<McpClient>,
        server_id: &str,
        info: &McpToolInfo,
        health: Arc<ServerHealthTracker>,
        eviction_callback: EvictionCallback,
    ) -> Self {
        let tool_name = protocol::tool_name(server_id, &info.name);
        let tool_desc = info.description.clone().unwrap_or_else(|| info.name.clone());
        let schema = info.input_schema.clone().unwrap_or_else(|| {
            json!({"type": "object", "properties": {}})
        });
        // search_hint：取 description 的前 30 字符
        let hint: String = tool_desc.chars().take(30).collect();
        Self {
            tool_name,
            tool_desc,
            schema,
            hint,
            client,
            mcp_tool_name: info.name.clone(),
            health,
            eviction_callback,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_desc
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _context: &ToolUseContext,
    ) -> ValidationResult {
        crate::tools::custom_tools::validate_tool_input(&self.schema, input)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        // 外部工具不可信，默认需要确认
        PermissionResult::ask("MCP 外部工具调用需要用户确认")
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        if self.health.is_evicted() {
            return ToolResult::error(format!(
                "[MCP 工具不可用] server [{}] 已被驱逐，请稍后重试或重新连接",
                self.health.server_id()
            ));
        }
        match self.client.call_tool(&self.mcp_tool_name, args).await {
            Ok(result) => {
                self.health.record_success();
                let failed = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
                let payload = protocol::result_payload(result);
                if failed {
                    let mut error = ToolResult::error(payload["output"].as_str().unwrap_or("MCP 工具执行失败"));
                    error.data = Some(payload);
                    error
                } else { ToolResult::success(payload) }
            }
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!("[MCP] 工具 [{}] 调用失败: {msg}", self.tool_name);
                let failures = self.health.record_failure(msg.clone());
                if failures >= MAX_CONSECUTIVE_FAILURES as u64 {
                    (self.eviction_callback)();
                }
                ToolResult::error(format!("[MCP 工具调用失败] {msg}"))
            }
        }
    }

    fn is_read_only(&self) -> bool {
        // 保守默认：外部工具视为非只读
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn should_defer(&self) -> bool {
        // MCP 工具默认延迟加载，避免 prompt 膨胀
        true
    }

    fn search_hint(&self) -> &str {
        &self.hint
    }
}

/// 已连接的 MCP server 状态
struct ServerState {
    config: McpServerConfig,
    client: Option<Arc<McpClient>>,
    /// 已注册到 ToolSystem 的工具名列表
    registered_tools: Vec<String>,
    /// 健康追踪器
    health: Arc<ServerHealthTracker>,
}

/// MCP manager - manages connections and tool registration for all MCP servers
pub struct McpManager {
    servers: Arc<AsyncMutex<HashMap<String, ServerState>>>,
    config_path: PathBuf,
    tool_system: Arc<crate::tools::ToolSystem>,
    config_lock: Mutex<()>,
    /// 健康检查循环是否已启动
    health_loop_started: AtomicBool,
    lifecycle_lock: AsyncMutex<()>,
}

impl McpManager {
    pub fn new(
        tool_system: Arc<crate::tools::ToolSystem>,
    ) -> VivianResult<Self> {
        let mcp_dir = get_user_data_dir().join("mcp");
        std::fs::create_dir_all(&mcp_dir)
            .map_err(|e| VivianError::Tool(format!("创建 MCP 目录失败: {e}")))?;
        let config_path = mcp_dir.join("servers.json");
        Ok(Self {
            servers: Arc::new(AsyncMutex::new(HashMap::new())),
            config_path,
            tool_system,
            config_lock: Mutex::new(()),
            health_loop_started: AtomicBool::new(false),
            lifecycle_lock: AsyncMutex::new(()),
        })
    }

    /// 创建降级实例：当主目录不可写时使用临时目录承载配置
    ///
    /// MCP 在进程内仍可连接已存在的 server，但配置无法持久化到用户数据目录，
    /// 重启后丢失。仅用于 `new()` 失败时的兜底。
    pub fn new_disabled(tool_system: Arc<crate::tools::ToolSystem>) -> Self {
        let mcp_dir = std::env::temp_dir().join("vivian-mcp");
        let _ = std::fs::create_dir_all(&mcp_dir);
        let config_path = mcp_dir.join("servers.json");
        tracing::warn!(
            "[McpManager] 主目录不可用，降级使用临时目录: {}",
            mcp_dir.display()
        );
        Self {
            servers: Arc::new(AsyncMutex::new(HashMap::new())),
            config_path,
            tool_system,
            config_lock: Mutex::new(()),
            health_loop_started: AtomicBool::new(false),
            lifecycle_lock: AsyncMutex::new(()),
        }
    }

    /// 加载配置文件（损坏文件备份现场后按空态处理，不静默吞掉用户数据）
    pub fn load_configs(&self) -> Vec<McpServerConfig> {
        if !self.config_path.exists() {
            return Vec::new();
        }
        match crate::utils::fs::load_json_or_backup::<Vec<McpServerConfig>>(&self.config_path) {
            Some(configs) => configs,
            None => Vec::new(),
        }
    }

    /// 保存配置到磁盘（原子写：临时文件 + rename，调用者必须已持有 config_lock）
    fn save_configs_unlocked(&self, configs: &[McpServerConfig]) -> VivianResult<()> {
        let json = serde_json::to_string_pretty(configs)
            .map_err(|e| VivianError::Tool(format!("序列化 MCP 配置失败: {e}")))?;
        crate::utils::fs::write_atomic(&self.config_path, &json)
            .map_err(|e| VivianError::Tool(format!("写入 MCP 配置失败: {e}")))?;
        Ok(())
    }

    fn load_plugin_overrides(&self) -> VivianResult<HashMap<String, McpServerConfig>> {
        let path = self.config_path.with_file_name("overrides.json");
        if !path.exists() { return Ok(HashMap::new()); }
        let bytes = std::fs::read(path).map_err(|e|VivianError::Tool(format!("读取 MCP 用户覆盖配置失败: {e}")))?;
        serde_json::from_slice(&bytes).map_err(|e|VivianError::Tool(format!("MCP 用户覆盖配置无效: {e}")))
    }

    /// Settings edits to a plugin connection survive reloads and app restarts.
    pub async fn save_user_server(&self, mut config: McpServerConfig) -> VivianResult<Vec<String>> {
        config.validate()?;
        {
            let _guard = self.config_lock.lock();
            let owner = self.load_configs().into_iter().find(|c|c.id == config.id).and_then(|c|c.source_plugin);
            config.source_plugin = owner;
            if let Some(plugin) = &config.source_plugin {
                let mut overrides = self.load_plugin_overrides()?;
                overrides.insert(format!("{plugin}:{}",config.id),config.clone());
                let json = serde_json::to_string_pretty(&overrides).map_err(|e|VivianError::Tool(e.to_string()))?;
                crate::utils::fs::write_atomic(&self.config_path.with_file_name("overrides.json"),&json).map_err(|e|VivianError::Tool(e.to_string()))?;
            }
        }
        self.add_server(config).await
    }

    pub fn clear_plugin_overrides(&self, plugin: &str, server_id: Option<&str>) -> VivianResult<()> {
        let _guard = self.config_lock.lock();
        let mut overrides = self.load_plugin_overrides()?;
        let prefix = format!("{plugin}:");
        overrides.retain(|key,_| !(key.starts_with(&prefix) && server_id.is_none_or(|id|key == &format!("{prefix}{id}"))));
        let json = serde_json::to_string_pretty(&overrides).map_err(|e|VivianError::Tool(e.to_string()))?;
        crate::utils::fs::write_atomic(&self.config_path.with_file_name("overrides.json"),&json).map_err(|e|VivianError::Tool(e.to_string()))
    }

    /// 合并插件声明的 MCP server 配置（只持久化，不连接）
    ///
    /// 供插件贡献点体系装载用，语义按 id 分三种情形：
    /// - id 不存在 → 写入并标记 `source_plugin`
    /// - id 存在且归属**同一插件** → 用新声明覆盖（插件重载时自身声明为准）
    /// - id 存在且无归属（用户手配）或归属**其他插件** → 跳过（用户配置优先）
    ///
    /// 后续 `init_all` / 健康循环 / 插件装载流程统一连接。
    /// 返回本次落盘后处于本插件名下、待连接的 server id 列表。
    pub fn merge_plugin_servers(&self, incoming: &[McpServerConfig], plugin_name: &str) -> Vec<String> {
        self.try_merge_plugin_servers(incoming, plugin_name).unwrap_or_else(|error| {
            crate::plugins::push_diag_event(format!("插件 {plugin_name} MCP 配置合并失败：{error}"));
            Vec::new()
        })
    }

    pub(crate) fn try_merge_plugin_servers(&self, incoming: &[McpServerConfig], plugin_name: &str) -> VivianResult<Vec<String>> {
        if incoming.is_empty() { return Ok(Vec::new()); }
        let _guard = self.config_lock.lock();
        let overrides = self.load_plugin_overrides()?;
        let mut configs = self.load_configs();
        let mut added = Vec::new();
        for cfg in incoming {
            let mut cfg = overrides.get(&format!("{plugin_name}:{}",cfg.id)).cloned().unwrap_or_else(||cfg.clone());
            cfg.source_plugin = Some(plugin_name.to_string());
            match configs.iter().position(|c| c.id == cfg.id) {
                None => {
                    tracing::info!(
                        "[McpManager] 插件 {} 贡献 MCP server: {} ({})",
                        plugin_name,
                        cfg.id,
                        cfg.name
                    );
                    configs.push(cfg.clone());
                    added.push(cfg.id.clone());
                }
                Some(idx) => {
                    let existing = &configs[idx];
                    if existing.source_plugin.as_deref() == Some(plugin_name) {
                        // 同插件重载：自身声明为准（保留 enabled 由新声明携带）
                        tracing::info!(
                            "[McpManager] 插件 {} 重载 MCP server: {}",
                            plugin_name,
                            cfg.id
                        );
                        configs[idx] = cfg.clone();
                        added.push(cfg.id.clone());
                    } else {
                        // 用户手配或其他插件的 id：保持不动
                        tracing::debug!(
                            "[McpManager] MCP server {} 已被占用（非插件 {}），跳过",
                            cfg.id,
                            plugin_name
                        );
                        crate::plugins::push_diag_event(format!(
                            "MCP server {} 声明来自插件 {plugin_name}，但 id 已被占用，已跳过",
                            cfg.id
                        ));
                    }
                }
            }
        }
        if !added.is_empty() {
            self.save_configs_unlocked(&configs)?;
        }
        Ok(added)
    }

    /// 撤销一个插件贡献的全部 MCP server（断开连接 + 注销工具 + 移除配置）。
    ///
    /// 只动 `source_plugin == plugin_name` 的条目；用户手配与其他插件的
    /// server 不受影响。返回被撤销的 server id 列表。
    pub async fn remove_plugin_servers(&self, plugin_name: &str) -> Vec<String> {
        let victims: Vec<String> = {
            let _guard = self.config_lock.lock();
            self.load_configs()
                .into_iter()
                .filter(|c| c.source_plugin.as_deref() == Some(plugin_name))
                .map(|c| c.id)
                .collect()
        };
        for id in &victims {
            if let Err(e) = self.remove_server(id).await {
                tracing::warn!("[McpManager] 撤销插件 {} 的 server {} 失败: {e}", plugin_name, id);
            }
        }
        victims
    }

    /// Save even disabled or failed connections, so they remain editable.
    pub async fn add_server(&self, config: McpServerConfig) -> VivianResult<Vec<String>> {
        config.validate()?;
        let _operation = self.lifecycle_lock.lock().await;
        {
            let _guard = self.config_lock.lock();
            let mut configs = self.load_configs();
            configs.retain(|c| c.id != config.id);
            configs.push(config.clone());
            self.save_configs_unlocked(&configs)?;
        }
        self.disconnect(&config.id).await;
        if !config.enabled { return Ok(Vec::new()); }
        self.connect(config, Arc::new(ServerHealthTracker::new(String::new()))).await
    }

    async fn disconnect(&self, id: &str) {
        let old = self.servers.lock().await.remove(id);
        if let Some(state) = old {
            for name in state.registered_tools { self.tool_system.unregister_tool_if_owner(&name, &format!("mcp:{id}")); }
            if let Some(client) = state.client { client.shutdown().await; }
        }
    }

    async fn connect(&self, config: McpServerConfig, mut health: Arc<ServerHealthTracker>) -> VivianResult<Vec<String>> {
        if health.server_id().is_empty() { health = Arc::new(ServerHealthTracker::new(config.id.clone())); }
        if let Some(plugin) = &config.source_plugin {
            if !crate::plugins::is_plugin_name_trusted(plugin) {
                return Err(VivianError::Tool(format!("来源插件 {plugin} 未启用或未信任")));
            }
        }
        match McpClient::start(config.clone()).await {
            Ok(client) => {
                let tools = client.get_tools();
                let system = Arc::clone(&self.tool_system);
                let health_for_cb = Arc::clone(&health);
                let owner = format!("mcp:{}", config.id);
                let callback_owner = owner.clone();
                let callback: EvictionCallback = Arc::new(move || {
                    if health_for_cb.mark_evicted() {
                        for name in system.list_tool_names() { system.unregister_tool_if_owner(&name, &callback_owner); }
                    }
                });
                let mut registered = Vec::new();
                for info in &tools {
                    let tool = Arc::new(McpTool::new(client.clone(), &config.id, info, health.clone(), callback.clone())) as Arc<dyn Tool>;
                    let name = tool.name().to_string();
                    if self.tool_system.has_tool(&name) {
                        tracing::warn!("[MCP] 工具名称冲突，跳过 {name}");
                        continue;
                    }
                    self.tool_system.register_tool_with_owner(tool, Some(owner.clone()));
                    registered.push(name);
                }
                health.reset_evicted();
                health.record_success();
                *health.last_error.lock() = None;
                self.servers.lock().await.insert(config.id.clone(), ServerState { config, client: Some(client), registered_tools: registered.clone(), health });
                Ok(registered)
            }
            Err(error) => {
                health.record_failure(error.to_string());
                health.mark_evicted();
                self.servers.lock().await.insert(config.id.clone(), ServerState { config, client: None, registered_tools: Vec::new(), health });
                Err(error)
            }
        }
    }

    pub async fn remove_server(&self, server_id: &str) -> VivianResult<()> {
        let _operation = self.lifecycle_lock.lock().await;
        let saved = {
            let _guard = self.config_lock.lock();
            let mut configs = self.load_configs();
            configs.retain(|c| c.id != server_id);
            self.save_configs_unlocked(&configs)
        };
        self.disconnect(server_id).await;
        saved
    }

    pub async fn reconnect_server(&self, server_id: &str) -> VivianResult<Vec<String>> {
        let config = self.load_configs().into_iter().find(|c|c.id == server_id)
            .ok_or_else(||VivianError::Tool("MCP 配置不存在".into()))?;
        self.add_server(config).await
    }

    pub async fn test_connection(&self, mut config: McpServerConfig) -> VivianResult<Value> {
        config.enabled = true;
        let client = McpClient::start(config).await?;
        let tools = client.get_tools();
        let result = json!({"tool_count":tools.len(), "tools":tools.iter().map(|t|json!({"name":t.name,"description":t.description,"inputSchema":t.input_schema})).collect::<Vec<_>>()});
        client.shutdown().await;
        Ok(result)
    }

    pub async fn init_all(&self) {
        for config in self.load_configs() {
            if !config.enabled || config.source_plugin.as_deref().is_some_and(|p|!crate::plugins::is_plugin_name_trusted(p)) { continue; }
            if let Err(error) = self.add_server(config).await { tracing::warn!("[MCP] 启动连接失败: {error}"); }
        }
    }

    pub fn start_health_check_loop(self: Arc<Self>) {
        if self.health_loop_started.swap(true, Ordering::SeqCst) { return; }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            loop { interval.tick().await; self.run_health_checks().await; }
        });
    }

    async fn run_health_checks(&self) {
        // Failed starts have no client, but must still take part in recovery.
        let checks: Vec<_> = self.servers.lock().await.values()
            .map(|s|(s.config.id.clone(), s.client.clone(), s.health.clone())).collect();
        for (id, client, health) in checks {
            if health.is_evicted() {
                let retry_due = !health.is_circuit_open() && health.last_failure_at.lock().as_ref()
                    .is_some_and(|t|t.elapsed().as_secs() >= health.next_reconnect_delay());
                if retry_due {
                    self.try_reconnect(&id).await;
                }
                continue;
            }
            if client.as_ref().is_some_and(|c| c.has_exited()) {
                health.record_failure("MCP 服务进程已退出".into());
                self.evict_server(&id).await;
                continue;
            }
            let result = match client {
                Some(client) if client.is_alive() => client.ping().await,
                _ => Err(VivianError::Tool("MCP 服务进程已退出".into())),
            };
            if let Err(error) = result {
                if health.record_failure(error.to_string()) >= MAX_CONSECUTIVE_FAILURES as u64 {
                    self.evict_server(&id).await;
                }
            } else { health.record_success(); }
        }
    }

    async fn evict_server(&self, id: &str) {
        let _operation = self.lifecycle_lock.lock().await;
        let mut servers = self.servers.lock().await;
        if let Some(state) = servers.get_mut(id) {
            state.health.mark_evicted();
            for name in state.registered_tools.drain(..) { self.tool_system.unregister_tool_if_owner(&name, &format!("mcp:{id}")); }
        }
    }

    async fn try_reconnect(&self, id: &str) {
        let _operation = self.lifecycle_lock.lock().await;
        let Some(config) = self.load_configs().into_iter().find(|c|c.id == id && c.enabled) else { return };
        if config.source_plugin.as_deref().is_some_and(|p|!crate::plugins::is_plugin_name_trusted(p)) { return; }
        let health = {
            let servers = self.servers.lock().await;
            let Some(state) = servers.get(id) else { return };
            if !state.health.is_evicted() || state.health.is_circuit_open() { return; }
            state.health.clone()
        };
        let attempt = health.increment_reconnect_attempt();
        self.disconnect(id).await;
        if self.connect(config, health.clone()).await.is_err() && attempt >= MAX_RECONNECT_ATTEMPTS as u64 { health.open_circuit(); }
    }

    pub async fn list_servers(&self) -> Vec<McpServerStatus> {
        let configs = self.load_configs();
        let servers = self.servers.lock().await;
        let mut rows = Vec::new();
        for config in configs {
            let state = servers.get(&config.id);
            let blocked = config.source_plugin.as_deref().is_some_and(|p|!crate::plugins::is_plugin_name_trusted(p));
            let alive = state.and_then(|s|s.client.as_ref()).is_some_and(|c|c.is_alive())
                && state.is_some_and(|s|!s.health.is_evicted()) && !blocked && config.enabled;
            let evicted = state.is_some_and(|s|s.health.is_evicted());
            let circuit_open = state.is_some_and(|s|s.health.is_circuit_open());
            let status = if !config.enabled { "disabled" } else if blocked { "blocked" } else if alive { "connected" } else if circuit_open { "failed" } else if evicted { "reconnecting" } else { "disconnected" };
            rows.push(McpServerStatus {
                id:config.id, name:config.name, enabled:config.enabled, transport:config.transport,
                tool_count:if alive { state.map_or(0,|s|s.registered_tools.len()) } else { 0 }, alive, evicted, circuit_open,
                consecutive_failures:state.map_or(0,|s|s.health.failure_count()),
                reconnect_attempts:state.map_or(0,|s|s.health.reconnect_attempt_count()),
                process_exited:state.and_then(|s|s.client.as_ref()).is_some_and(|c|c.has_exited()),
                last_error:if blocked {Some("来源插件未启用或未信任".into())} else {state.and_then(|s|s.health.last_error.lock().clone())},
                status:status.into(), source_plugin:config.source_plugin,
                tools:if alive {state.map(|s|s.registered_tools.clone()).unwrap_or_default()} else {Vec::new()},
            });
        }
        rows.sort_by(|a,b|a.id.cmp(&b.id));
        rows
    }

    pub async fn shutdown_all(&self) {
        let _operation = self.lifecycle_lock.lock().await;
        let ids:Vec<_> = self.servers.lock().await.keys().cloned().collect();
        for id in ids { self.disconnect(&id).await; }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: String,
    pub tool_count: usize,
    pub alive: bool,
    pub evicted: bool,
    pub consecutive_failures: u64,
    pub circuit_open: bool,
    pub reconnect_attempts: u64,
    pub process_exited: bool,
    pub last_error: Option<String>,
    pub status: String,
    pub source_plugin: Option<String>,
    pub tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_at(dir: &std::path::Path) -> McpManager {
        McpManager {
            servers: Arc::new(AsyncMutex::new(HashMap::new())),
            config_path: dir.join("servers.json"),
            tool_system: Arc::new(crate::tools::ToolSystem::new()),
            config_lock: Mutex::new(()), health_loop_started: AtomicBool::new(false), lifecycle_lock: AsyncMutex::new(()),
        }
    }

    fn config_for(command: &str) -> McpServerConfig {
        serde_json::from_value(json!({"id":"fixture","name":"Fixture","command":command})).unwrap()
    }

    #[tokio::test]
    async fn extension_disabled_and_failed_connections_remain_editable() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager_at(dir.path());
        let mut config = config_for("node"); config.enabled = false;
        manager.add_server(config).await.unwrap();
        assert_eq!(manager.list_servers().await[0].status, "disabled");
        let config = config_for(&dir.path().join("missing-executable.exe").display().to_string());
        assert!(manager.add_server(config).await.is_err());
        assert_eq!(manager.load_configs().len(), 1);
        assert!(manager.list_servers().await[0].last_error.is_some());
        manager.remove_server("fixture").await.unwrap();
        assert!(manager.list_servers().await.is_empty());
        let default = config_for("node");
        manager.merge_plugin_servers(std::slice::from_ref(&default), "fixture-plugin");
        let mut custom = manager.load_configs()[0].clone();
        custom.enabled = false; custom.env.insert("TOKEN".into(),"synthetic".into());
        manager.save_user_server(custom).await.unwrap();
        manager.remove_server("fixture").await.unwrap();
        manager.merge_plugin_servers(std::slice::from_ref(&default), "fixture-plugin");
        assert!(!manager.load_configs()[0].enabled);
        assert_eq!(manager.load_configs()[0].env["TOKEN"],"synthetic");
        manager.clear_plugin_overrides("fixture-plugin",Some("fixture")).unwrap();
        manager.merge_plugin_servers(std::slice::from_ref(&default),"fixture-plugin");
        assert!(manager.load_configs()[0].enabled);
        assert!(manager.load_configs()[0].env.is_empty());
    }

    #[tokio::test]
    async fn extension_failed_reconnects_continue_without_a_live_client_and_stop_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager_at(dir.path());
        let config = config_for(&dir.path().join("missing-executable.exe").display().to_string());
        assert!(manager.add_server(config).await.is_err());
        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            let health = manager.servers.lock().await["fixture"].health.clone();
            *health.last_failure_at.lock() = Some(Instant::now() - Duration::from_secs(1000));
            manager.run_health_checks().await;
            assert_eq!(manager.list_servers().await[0].reconnect_attempts, attempt as u64);
        }
        manager.run_health_checks().await;
        let row = &manager.list_servers().await[0];
        assert!(row.circuit_open); assert!(!row.alive); assert_eq!(row.tool_count, 0);
        let mut disabled = manager.load_configs()[0].clone(); disabled.enabled = false;
        manager.add_server(disabled).await.unwrap();
        assert_eq!(manager.list_servers().await[0].status, "disabled");
    }

    #[tokio::test]
    async fn extension_stdio_roundtrip_keeps_schema_pagination_errors_and_mixed_content() {
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("server.mjs");
        std::fs::write(&server, r#"
import readline from 'node:readline';
for await (const line of readline.createInterface({input:process.stdin})) {
 const r=JSON.parse(line); if(r.method==='notifications/initialized' && r.params===null)throw Error('notification params must be omitted');if(r.id===undefined||!r.method)continue;
 let result={};
 if(r.method==='initialize'){console.log(JSON.stringify({jsonrpc:'2.0',id:r.id,method:'ping'}));result={protocolVersion:'2025-06-18',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}};}
 if(r.method==='tools/list') result=r.params?.cursor?{tools:[{name:'second',inputSchema:{type:'object'}}]}:{tools:[{name:'first',inputSchema:{type:'object',properties:{path:{type:'string'}},required:['path']}}],nextCursor:'page2'};
 if(r.method==='tools/call') result={isError:!!r.params.arguments.fail,content:[{type:'text',text:process.env.MCP_TEST_VALUE},{type:'image',data:'abc',mimeType:'image/png'}],structuredContent:{cwd:process.cwd()}};
 console.log(JSON.stringify({jsonrpc:'2.0',id:r.id,result}));
}
"#).unwrap();
        let mut config = config_for("node");
        config.args = vec![server.display().to_string()];
        config.cwd = Some(dir.path().display().to_string());
        config.env.insert("MCP_TEST_VALUE".into(), "with spaces = value".into());
        let client = McpClient::start(config).await.unwrap();
        let tools = client.get_tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].input_schema.as_ref().unwrap()["required"], json!(["path"]));
        let health = Arc::new(ServerHealthTracker::new("fixture".into()));
        let tool = McpTool::new(client.clone(), "fixture", &tools[0], health.clone(), Arc::new(||{}));
        let result = tool.call(json!({"path":"G:/My Files","fail":true}), &ToolUseContext::default()).await;
        assert!(!result.success);
        assert_eq!(result.data.as_ref().unwrap()["content"].as_array().unwrap().len(), 2);
        assert_eq!(result.data.as_ref().unwrap()["output"], "with spaces = value");
        assert!(result.data.unwrap()["structuredContent"]["cwd"].is_string());
        assert_eq!(health.failure_count(), 0, "business errors must not evict a healthy server");
        let success = tool.call(json!({"path":"x"}), &ToolUseContext::default()).await;
        assert!(success.success);
        client.shutdown().await;
        assert!(!client.is_alive());
    }

    #[tokio::test]
    async fn extension_streamable_http_negotiates_session_and_accepts_sse_results() {
        use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
        let requests = Arc::new(Mutex::new(Vec::<(reqwest::header::HeaderMap, Value)>::new()));
        async fn handle(State(log): State<Arc<Mutex<Vec<(reqwest::header::HeaderMap, Value)>>>>, headers: reqwest::header::HeaderMap, Json(request): Json<Value>) -> axum::response::Response {
            log.lock().push((headers, request.clone()));
            let id = request["id"].clone();
            match request["method"].as_str().unwrap_or("") {
                "initialize" => ([("mcp-session-id","fixture-session")], Json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}}}}))).into_response(),
                "notifications/initialized" => if request.get("params").is_none() { axum::http::StatusCode::ACCEPTED.into_response() } else { axum::http::StatusCode::BAD_REQUEST.into_response() },
                "tools/list" => ([("content-type","text/event-stream")], format!("data: {}\r\n\r\ndata: {}\n\n",json!({"jsonrpc":"2.0","id":id,"method":"ping"}),json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"remote","inputSchema":{"type":"object","properties":{}}}]}}))).into_response(),
                "tools/call" => axum::http::StatusCode::NOT_FOUND.into_response(),
                "" => axum::http::StatusCode::ACCEPTED.into_response(),
                _ => Json(json!({"jsonrpc":"2.0","id":id,"result":{},"error":null})).into_response(),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route("/mcp", post(handle).delete(||async{axum::http::StatusCode::NO_CONTENT})).with_state(requests.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        let mut config = config_for(""); config.transport = "http".into(); config.url = Some(format!("http://{address}/mcp"));
        config.headers.insert("Authorization".into(), "Bearer synthetic".into());
        let client = McpClient::start(config).await.unwrap();
        assert_eq!(client.get_tools().len(), 1);
        client.ping().await.unwrap();
        assert!(client.call_tool("remote",json!({})).await.is_err());
        assert!(client.has_exited(), "Expired sessions must enter recovery without replaying tool calls");
        client.shutdown().await;
        let rows = requests.lock();
        assert_eq!(rows[0].0["authorization"], "Bearer synthetic");
        for (headers, _) in rows.iter().skip(1) {
            assert_eq!(headers["mcp-session-id"], "fixture-session");
            assert_eq!(headers["mcp-protocol-version"], "2025-06-18");
        }
        server.abort();
    }

    #[test]
    fn test_config_serde() {
        let config = McpServerConfig {
            id: "test".into(),
            name: "Test Server".into(),
            transport: "stdio".into(),
            command: "echo".into(),
            args: vec!["hello".into()],
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            cwd: None,
            enabled: true,
            source_plugin: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test");
        assert_eq!(parsed.command, "echo");
        assert!(parsed.enabled);
    }

    #[test]
    fn test_tool_name_format() {
        let info = McpToolInfo {
            name: "search".into(),
            description: Some("Search the web".into()),
            input_schema: None,
        };
        // 模拟工具名生成
        let tool_name = format!("mcp__{}__{}", "web", info.name);
        assert_eq!(tool_name, "mcp__web__search");
    }

    #[test]
    fn test_config_persistence() {
        let dir = std::env::temp_dir().join("vivian_mcp_test");
        let path = dir.join("servers.json");
        let _ = std::fs::create_dir_all(&dir);
        let configs = vec![McpServerConfig {
            id: "p1".into(),
            name: "Persistent".into(),
            transport: "stdio".into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            cwd: None,
            enabled: false,
            source_plugin: Some("p1-plugin".into()),
        }];
        let json = serde_json::to_string_pretty(&configs).unwrap();
        std::fs::write(&path, &json).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"id\": \"p1\""));
        let _ = std::fs::remove_file(&path);
    }
}
