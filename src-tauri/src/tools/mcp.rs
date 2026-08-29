//! MCP (Model Context Protocol) 原生集成 — 轻量级 stdio 客户端
//!
//! 设计目标：
//! - 不引入外部 MCP SDK，手写 JSON-RPC 2.0 over stdio（tokio::process）
//! - MCP 工具实现 `Tool` trait，与内置工具无差别调度
//! - 工具命名 `mcp__{server_id}__{tool_name}`，避免与内置工具冲突
//! - MCP 工具默认延迟加载（`should_defer=true`），通过 `tool_search` 按需唤起
//! - 外部工具不可信：`is_read_only=false` + `check_permissions` 返回 `ask`
//!
//! 协议参考：https://spec.modelcontextprotocol.io/
//! 仅实现 stdio 传输 + tools 能力（resources/prompts 暂不支持）。

use std::collections::HashMap;
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
    pub command: String,
    /// 命令参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// 工作目录
    #[serde(default)]
    pub cwd: Option<String>,
    /// 是否启用（false 则不连接）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_transport() -> String {
    "stdio".to_string()
}

fn default_enabled() -> bool {
    true
}

/// MCP server 返回的工具描述（listTools 结果项）
#[derive(Debug, Clone, Deserialize)]
struct McpToolInfo {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    input_schema: Option<Value>,
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
    pending: AsyncMutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>,
    next_id: AtomicU64,
    child: AsyncMutex<Option<Child>>,
    tools: Mutex<Vec<McpToolInfo>>,
    config: McpServerConfig,
    exited: AtomicBool,
}

impl McpClient {
    async fn start(config: McpServerConfig) -> VivianResult<Arc<Self>> {
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
            pending: AsyncMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            child: AsyncMutex::new(Some(child)),
            tools: Mutex::new(Vec::new()),
            config,
            exited: AtomicBool::new(false),
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
        client.initialize().await?;

        // 发现工具
        client.discover_tools().await?;

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
            let resp: JsonRpcResponse = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("[MCP] 非 JSON-RPC 行 [{}]: {e}", client.config.id);
                    continue;
                }
            };
            // 取出 pending sender 并发送
            let sender = {
                let mut pending = client.pending.lock().await;
                pending.remove(&resp.id)
            };
            if let Some(tx) = sender {
                let _ = tx.send(resp);
            }
        }
        // stdout 关闭后，清理所有 pending sender（让等待者收到错误）
        client.exited.store(true, Ordering::Relaxed);
        let mut pending = client.pending.lock().await;
        pending.clear();
    }

    fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
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
        let serialized = serde_json::to_string(&req)
            .map_err(|e| VivianError::Tool(format!("JSON-RPC 序列化失败: {e}")))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

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
                let _ = self.pending.lock().await.remove(&id);
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
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: "ping".to_string(),
            params: None,
        };
        let serialized = serde_json::to_string(&req)
            .map_err(|e| VivianError::Tool(format!("JSON-RPC 序列化失败: {e}")))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

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

        match tokio::time::timeout(Duration::from_secs(PING_TIMEOUT_SECS), rx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.error {
                    return Err(VivianError::Tool(format!("MCP ping 错误: {}", err.message)));
                }
                Ok(())
            }
            Ok(Err(_)) => {
                let _ = self.pending.lock().await.remove(&id);
                Err(VivianError::Tool("MCP ping 响应通道关闭".into()))
            }
            Err(_) => {
                let _ = self.pending.lock().await.remove(&id);
                Err(VivianError::Tool("MCP ping 超时".into()))
            }
        }
    }

    /// 发送 initialize 握手 + initialized 通知
    async fn initialize(&self) -> VivianResult<()> {
        let result = self
            .request(
                "initialize",
                Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "vivian",
                        "version": "1.0.0"
                    }
                })),
            )
            .await?;
        tracing::info!(
            "[MCP] server [{}] 握手成功: {:?}",
            self.config.id,
            result.get("serverInfo")
        );
        // 发送 initialized 通知（id 为 null 的通知，无需等待响应）
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    /// 发送通知（不等待响应）
    async fn notify(&self, method: &str, params: Option<Value>) -> VivianResult<()> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null)
        });
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
        let result = self.request("tools/list", None).await?;
        let tools: Vec<McpToolInfo> = result
            .get("tools")
            .cloned()
            .and_then(|t| serde_json::from_value(t).ok())
            .unwrap_or_default();
        tracing::info!(
            "[MCP] server [{}] 发现 {} 个工具",
            self.config.id,
            tools.len()
        );
        *self.tools.lock() = tools;
        Ok(())
    }

    /// 调用工具
    async fn call_tool(&self, name: &str, arguments: Value) -> VivianResult<String> {
        let result = self
            .request(
                "tools/call",
                Some(json!({
                    "name": name,
                    "arguments": arguments
                })),
            )
            .await?;
        // 提取文本内容
        let content = result.get("content").and_then(|c| c.as_array());
        let texts: Vec<String> = content
            .map(|arr| {
                arr.iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            block.get("text").and_then(|t| t.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if texts.is_empty() {
            // 无文本块，返回原始 JSON
            Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
        } else {
            Ok(texts.join("\n"))
        }
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
        let _ = self.notify("shutdown", None).await;
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
        }
        // take() 后 stdin 恒为 None；后续访问必须用 as_mut() 兜底，禁止 unwrap/expect
        let mut stdin_guard = self.stdin.lock().await;
        stdin_guard.take();
    }

    fn is_alive(&self) -> bool {
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
        let tool_name = format!("mcp__{}__{}", server_id, info.name);
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
        _input: &Value,
        _context: &ToolUseContext,
    ) -> ValidationResult {
        // MCP 工具的 schema 由 server 定义，这里不做额外校验，交给 server 处理
        ValidationResult::success(None)
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
            Ok(text) => {
                self.health.record_success();
                ToolResult::success(json!({ "output": text }))
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
    client: Arc<McpClient>,
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
        }
    }

    /// 加载配置文件
    pub fn load_configs(&self) -> Vec<McpServerConfig> {
        if !self.config_path.exists() {
            return Vec::new();
        }
        match std::fs::read_to_string(&self.config_path) {
            Ok(content) if !content.trim().is_empty() => {
                serde_json::from_str::<Vec<McpServerConfig>>(&content).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// 保存配置到磁盘（调用者必须已持有 config_lock）
    fn save_configs_unlocked(&self, configs: &[McpServerConfig]) -> VivianResult<()> {
        let json = serde_json::to_string_pretty(configs)
            .map_err(|e| VivianError::Tool(format!("序列化 MCP 配置失败: {e}")))?;
        std::fs::write(&self.config_path, json)
            .map_err(|e| VivianError::Tool(format!("写入 MCP 配置失败: {e}")))?;
        Ok(())
    }

    /// 合并插件声明的 MCP server 配置（只持久化，不连接）
    ///
    /// 供插件贡献点体系启动装载用：按 id 去重，已存在的 id 保持用户配置不动
    /// （用户手动添加/编辑的优先于插件声明）。后续 `init_all` / 健康循环统一连接。
    /// 返回实际新增的 server id 列表。
    pub fn merge_plugin_servers(&self, incoming: &[McpServerConfig]) -> Vec<String> {
        let _guard = self.config_lock.lock();
        let mut configs = self.load_configs();
        let existing_ids: std::collections::HashSet<String> =
            configs.iter().map(|c| c.id.clone()).collect();
        let mut added = Vec::new();
        for cfg in incoming {
            if existing_ids.contains(&cfg.id) {
                continue;
            }
            tracing::info!(
                "[McpManager] 插件贡献 MCP server: {} ({})",
                cfg.id,
                cfg.name
            );
            configs.push(cfg.clone());
            added.push(cfg.id.clone());
        }
        if !added.is_empty() {
            if let Err(e) = self.save_configs_unlocked(&configs) {
                tracing::warn!("[McpManager] 插件 MCP 配置持久化失败: {e}");
                return Vec::new();
            }
        }
        added
    }

    /// 连接 MCP server 并注册工具
    pub async fn add_server(&self, config: McpServerConfig) -> VivianResult<Vec<String>> {
        if !config.enabled {
            return Ok(Vec::new());
        }
        let client = McpClient::start(config.clone()).await?;
        let tools = client.get_tools();

        let health = Arc::new(ServerHealthTracker::new(config.id.clone()));
        let tool_system = Arc::clone(&self.tool_system);
        let health_for_cb = Arc::clone(&health);
        let server_id_for_cb = config.id.clone();
        let eviction_callback: EvictionCallback = Arc::new(move || {
            if health_for_cb.mark_evicted() {
                let prefix = format!("mcp__{}__", server_id_for_cb);
                let names: Vec<String> = tool_system
                    .list_tool_names()
                    .into_iter()
                    .filter(|n| n.starts_with(&prefix))
                    .collect();
                for name in &names {
                    tool_system.unregister_tool(name);
                }
                tracing::warn!(
                    "[MCP] server [{}] 连续失败达阈值，触发反应式驱逐，注销 {} 个工具",
                    server_id_for_cb,
                    names.len()
                );
            }
        });

        let mut registered = Vec::new();
        for info in &tools {
            let tool = Arc::new(McpTool::new(
                Arc::clone(&client),
                &config.id,
                info,
                Arc::clone(&health),
                Arc::clone(&eviction_callback),
            )) as Arc<dyn Tool>;
            let name = tool.name().to_string();
            self.tool_system.register_tool(tool);
            registered.push(name);
        }
        let state = ServerState {
            config: config.clone(),
            client,
            registered_tools: registered.clone(),
            health,
        };
        self.servers.lock().await.insert(config.id.clone(), state);
        {
            let _guard = self.config_lock.lock();
            let mut configs = self.load_configs();
            configs.retain(|c| c.id != config.id);
            configs.push(config);
            self.save_configs_unlocked(&configs)?;
        }
        Ok(registered)
    }

    /// 断开 MCP server 并注销工具
    pub async fn remove_server(&self, server_id: &str) -> VivianResult<()> {
        let mut servers = self.servers.lock().await;
        if let Some(state) = servers.remove(server_id) {
            for name in &state.registered_tools {
                self.tool_system.unregister_tool(name);
            }
            state.client.shutdown().await;
        }
        drop(servers);
        {
            let _guard = self.config_lock.lock();
            let mut configs = self.load_configs();
            configs.retain(|c| c.id != server_id);
            self.save_configs_unlocked(&configs)?;
        }
        Ok(())
    }

    /// 启动时自动连接所有已保存的 enabled server
    pub async fn init_all(&self) {
        let configs = self.load_configs();
        for config in configs {
            if !config.enabled {
                continue;
            }
            let id = config.id.clone();
            match self.add_server(config).await {
                Ok(tools) => {
                    tracing::info!("[MCP] server [{}] 已连接，注册 {} 个工具", id, tools.len())
                }
                Err(e) => {
                    tracing::warn!("[MCP] server [{}] 连接失败: {e}", id)
                }
            }
        }
    }

    /// 启动后台健康检查循环（仅启动一次）
    ///
    /// 每 `HEALTH_CHECK_INTERVAL_SECS` 秒对所有存活 server 执行 ping：
    /// - ping 成功 → 重置失败计数
    /// - ping 失败 → 递增失败计数，达阈值后驱逐该 server（注销工具 + 标记 evicted）
    /// - 已驱逐的 server 在 `EVICT_RECONNECT_DELAY_SECS` 后尝试自动重连
    pub fn start_health_check_loop(self: Arc<Self>) {
        if self.health_loop_started.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            loop {
                interval.tick().await;
                self.run_health_checks().await;
            }
        });
    }

    async fn run_health_checks(&self) {
        let checks: Vec<(String, Arc<McpClient>, Arc<ServerHealthTracker>)> = {
            let servers = self.servers.lock().await;
            servers
                .values()
                .filter(|s| s.client.is_alive())
                .map(|s| {
                    (
                        s.config.id.clone(),
                        Arc::clone(&s.client),
                        Arc::clone(&s.health),
                    )
                })
                .collect()
        };

        for (id, client, health) in checks {
            if client.has_exited() && !health.is_evicted() {
                tracing::warn!(
                    "[MCP] server [{}] 子进程已退出，立即驱逐",
                    id
                );
                health.record_failure("process exited".to_string());
                self.evict_server(&id).await;
                continue;
            }

            if health.is_evicted() {
                if health.is_circuit_open() {
                    continue;
                }
                let should_reconnect = {
                    let guard = health.last_failure_at.lock();
                    match guard.as_ref() {
                        Some(evicted_at) => {
                            evicted_at.elapsed().as_secs() >= health.next_reconnect_delay()
                        }
                        None => false,
                    }
                };
                if should_reconnect {
                    self.try_reconnect(&id).await;
                }
                continue;
            }

            match client.ping().await {
                Ok(()) => {
                    health.record_success();
                }
                Err(e) => {
                    let failures = health.record_failure(e.to_string());
                    tracing::warn!(
                        "[MCP] server [{}] ping 失败（连续 {} 次）: {}",
                        id,
                        failures,
                        e
                    );
                    if failures >= MAX_CONSECUTIVE_FAILURES as u64 {
                        self.evict_server(&id).await;
                    }
                }
            }
        }
    }

    /// 驱逐指定 server：注销其全部工具并标记为 evicted
    async fn evict_server(&self, server_id: &str) {
        let server_id = server_id.to_string();
        let prefix = format!("mcp__{}__", server_id);
        let names: Vec<String> = self
            .tool_system
            .list_tool_names()
            .into_iter()
            .filter(|n| n.starts_with(&prefix))
            .collect();

        let health = {
            let servers = self.servers.lock().await;
            servers.get(&server_id).map(|s| Arc::clone(&s.health))
        };

        if let Some(health) = &health {
            if !health.mark_evicted() {
                return;
            }
        }

        for name in &names {
            self.tool_system.unregister_tool(name);
        }
        tracing::warn!(
            "[MCP] server [{}] 健康检查连续失败，执行驱逐，注销 {} 个工具",
            server_id,
            names.len()
        );
    }

    async fn try_reconnect(&self, server_id: &str) {
        let server_id = server_id.to_string();
        let (config, health) = {
            let servers = self.servers.lock().await;
            match servers.get(&server_id) {
                Some(state) if state.health.is_evicted() && !state.health.is_circuit_open() => {
                    (state.config.clone(), Arc::clone(&state.health))
                }
                _ => return,
            }
        };

        let attempt = health.increment_reconnect_attempt();
        let delay = health.next_reconnect_delay();
        tracing::info!(
            "[MCP] server [{}] 尝试自动重连（第 {} 次，下次退避 {}s）",
            server_id,
            attempt,
            delay
        );

        {
            let servers = self.servers.lock().await;
            if let Some(state) = servers.get(&server_id) {
                state.client.shutdown().await;
            }
        }

        match McpClient::start(config.clone()).await {
            Ok(new_client) => {
                let tools = new_client.get_tools();
                let tool_system = Arc::clone(&self.tool_system);
                let health_for_cb = Arc::clone(&health);
                let server_id_for_cb = server_id.clone();
                let eviction_callback: EvictionCallback = Arc::new(move || {
                    if health_for_cb.mark_evicted() {
                        let prefix = format!("mcp__{}__", server_id_for_cb);
                        let names: Vec<String> = tool_system
                            .list_tool_names()
                            .into_iter()
                            .filter(|n| n.starts_with(&prefix))
                            .collect();
                        for name in &names {
                            tool_system.unregister_tool(name);
                        }
                        tracing::warn!(
                            "[MCP] server [{}] 重连后再次连续失败，触发反应式驱逐，注销 {} 个工具",
                            server_id_for_cb,
                            names.len()
                        );
                    }
                });

                let mut registered = Vec::new();
                for info in &tools {
                    let tool = Arc::new(McpTool::new(
                        Arc::clone(&new_client),
                        &server_id,
                        info,
                        Arc::clone(&health),
                        Arc::clone(&eviction_callback),
                    )) as Arc<dyn Tool>;
                    let name = tool.name().to_string();
                    self.tool_system.register_tool(tool);
                    registered.push(name);
                }

                {
                    let mut servers = self.servers.lock().await;
                    if let Some(state) = servers.get_mut(&server_id) {
                        state.client = new_client;
                        state.registered_tools = registered.clone();
                        state.health.reset_evicted();
                    }
                }
                tracing::info!(
                    "[MCP] server [{}] 重连成功，重新注册 {} 个工具",
                    server_id,
                    registered.len()
                );
            }
            Err(e) => {
                tracing::warn!("[MCP] server [{}] 重连失败: {}", server_id, e);
                *health.last_failure_at.lock() = Some(Instant::now());
                if attempt >= MAX_RECONNECT_ATTEMPTS as u64 {
                    health.open_circuit();
                    tracing::error!(
                        "[MCP] server [{}] 连续 {} 次重连失败，熔断器已开启，停止自动重连",
                        server_id,
                        attempt
                    );
                }
            }
        }
    }

    /// 列出所有已连接 server 的状态
    pub async fn list_servers(&self) -> Vec<McpServerStatus> {
        let servers = self.servers.lock().await;
        servers
            .values()
            .map(|s| McpServerStatus {
                id: s.config.id.clone(),
                name: s.config.name.clone(),
                enabled: s.config.enabled,
                tool_count: s.registered_tools.len(),
                alive: s.client.is_alive(),
                evicted: s.health.is_evicted(),
                consecutive_failures: s.health.failure_count(),
                circuit_open: s.health.is_circuit_open(),
                reconnect_attempts: s.health.reconnect_attempt_count(),
                process_exited: s.client.has_exited(),
            })
            .collect()
    }

    /// 停止所有 server
    pub async fn shutdown_all(&self) {
        let mut servers = self.servers.lock().await;
        for (_, state) in servers.drain() {
            for name in &state.registered_tools {
                self.tool_system.unregister_tool(name);
            }
            state.client.shutdown().await;
        }
    }
}

/// MCP server 运行时状态
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub tool_count: usize,
    pub alive: bool,
    pub evicted: bool,
    pub consecutive_failures: u64,
    pub circuit_open: bool,
    pub reconnect_attempts: u64,
    pub process_exited: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serde() {
        let config = McpServerConfig {
            id: "test".into(),
            name: "Test Server".into(),
            transport: "stdio".into(),
            command: "echo".into(),
            args: vec!["hello".into()],
            env: HashMap::new(),
            cwd: None,
            enabled: true,
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
            cwd: None,
            enabled: false,
        }];
        let json = serde_json::to_string_pretty(&configs).unwrap();
        std::fs::write(&path, &json).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"id\": \"p1\""));
        let _ = std::fs::remove_file(&path);
    }
}
