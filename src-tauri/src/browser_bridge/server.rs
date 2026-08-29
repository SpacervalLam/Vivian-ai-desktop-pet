//! 浏览器桥服务端：token 认证的 WebSocket 通道 + 工具调用分发。
//!
//! 同一时刻仅一个活动扩展连接，新认证连接顶替旧连接。模型侧 `browser_*` 工具
//! 通过 `BridgeState::request_tool` 派发 `tool.call`，并按 correlation id 等待扩展
//! 回传 `tool.result`。整条通道仅绑定回环地址。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::State;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::protocol::*;

/// 一次挂起中的工具调用，等扩展回传结果。
struct PendingCall {
    tx: oneshot::Sender<Result<String, String>>,
}

/// 一个已认证的活动扩展连接。
#[derive(Clone)]
struct ActiveSocket {
    generation: u64,
    /// 写向该 WS 连接的通道（由专属 writer task 消费）。
    tx: mpsc::Sender<Message>,
}

struct BridgeInner {
    active: Option<ActiveSocket>,
    pending: std::collections::HashMap<String, PendingCall>,
    next_generation: u64,
    next_call_id: u64,
    /// 扩展注入的"跟随页面快照"（来自 `bridge.injectBrowserSnapshot` RPC）。持久到连接替换。
    injected_snapshot: Option<(u64, String)>,
    /// 最近一次工具调用关联的会话 ID（会话延续/工作区归组）。
    last_session_id: Option<String>,
    /// 扩展上报的平台登录态（platform → 已登录）与上报时间戳（毫秒）。
    platform_status: Option<(u64, std::collections::HashMap<String, bool>)>,
    /// 扩展回传的 X (Twitter) Cookie 头（auth_token + ct0）与时间戳（毫秒）。
    /// 供服务端 cookie 重放发现（twitter-cli）使用；仅在内存中保持，随连接刷新。
    x_cookie: Option<(u64, String)>,
    /// 扩展回传的 Reddit Cookie 头（含 reddit_session）与时间戳（毫秒）。
    /// 供服务端同步 rdt-cli 凭据文件（登录态发现）使用；仅在内存中保持。
    reddit_cookie: Option<(u64, String)>,
}

/// 浏览器桥共享状态（跨工具执行与 WS 连接共享，存放在 AppState 中）。
pub struct BridgeState {
    token: String,
    inner: Mutex<BridgeInner>,
}

impl BridgeState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            token: generate_token(),
            inner: Mutex::new(BridgeInner {
                active: None,
                pending: Default::default(),
                next_generation: 0,
                next_call_id: 0,
                injected_snapshot: None,
                last_session_id: None,
                platform_status: None,
                x_cookie: None,
                reddit_cookie: None,
            }),
        })
    }

    /// 当前 bearer token（供 /ext/bridge-config 回还）。
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 是否已有扩展连接。
    pub fn is_connected(&self) -> bool {
        self.inner.lock().active.is_some()
    }

    /// 最近一次工具调用关联的会话 ID（供前端/诊断展示）。
    pub fn last_session_id(&self) -> Option<String> {
        self.inner.lock().last_session_id.clone()
    }

    /// 注入/更新"跟随页面快照"。`epoch_ms` 用于排序（后到覆盖先到）。
    pub fn inject_snapshot(&self, epoch_ms: u64, snapshot: String) {
        let mut inner = self.inner.lock();
        if let Some((ts, _)) = &inner.injected_snapshot {
            if *ts > epoch_ms {
                return;
            }
        }
        inner.injected_snapshot = Some((epoch_ms, snapshot));
    }

    /// 读取当前注入快照（若从未注入则为 None）。
    pub fn injected_snapshot(&self) -> Option<String> {
        self.inner.lock().injected_snapshot.as_ref().map(|(_, s)| s.clone())
    }

    /// 更新平台登录态（来自扩展上报；epoch_ms 用于后到覆盖先到）。
    pub fn report_platform_status(
        &self,
        epoch_ms: u64,
        status: std::collections::HashMap<String, bool>,
    ) {
        let mut inner = self.inner.lock();
        if let Some((ts, _)) = &inner.platform_status {
            if *ts > epoch_ms {
                return;
            }
        }
        inner.platform_status = Some((epoch_ms, status));
    }

    /// 读取平台登录态快照（(上报毫秒时间戳, platform → 已登录)）。
    pub fn platform_status(&self) -> Option<(u64, std::collections::HashMap<String, bool>)> {
        self.inner.lock().platform_status.clone()
    }

    /// 更新 X (Twitter) Cookie 头（来自扩展回传；epoch_ms 后到覆盖先到）。
    pub fn report_x_cookie(&self, epoch_ms: u64, cookie: String) {
        let mut inner = self.inner.lock();
        if let Some((ts, _)) = &inner.x_cookie {
            if *ts > epoch_ms {
                return;
            }
        }
        inner.x_cookie = Some((epoch_ms, cookie));
    }

    /// 读取 X (Twitter) Cookie 头（auth_token=...; ct0=...；未回传则为 None）。
    pub fn x_cookie(&self) -> Option<String> {
        self.inner
            .lock()
            .x_cookie
            .as_ref()
            .map(|(_, c)| c.clone())
    }

    /// 更新 Reddit Cookie 头（来自扩展回传；epoch_ms 后到覆盖先到）。
    pub fn report_reddit_cookie(&self, epoch_ms: u64, cookie: String) {
        let mut inner = self.inner.lock();
        if let Some((ts, _)) = &inner.reddit_cookie {
            if *ts > epoch_ms {
                return;
            }
        }
        inner.reddit_cookie = Some((epoch_ms, cookie));
    }

    /// 读取 Reddit Cookie 头与回传毫秒时间戳（未回传则为 None）。
    pub fn reddit_cookie(&self) -> Option<(u64, String)> {
        self.inner.lock().reddit_cookie.clone()
    }

    /// 记录一次工具调用关联的会话（会话延续）。
    fn note_session(&self, session_id: Option<&str>) {
        if let Some(sid) = session_id {
            self.inner.lock().last_session_id = Some(sid.to_string());
        }
    }

    /// 模型侧工具把一次浏览器操作派发给扩展执行，返回扩展回传的文本结果。
    pub async fn request_tool(
        &self,
        name: &str,
        args: &Value,
        timeout: Duration,
        session_id: Option<&str>,
    ) -> Result<String, String> {
        self.note_session(session_id);
        // 登记挂起调用并取得写通道；先 clone 活动连接写通道再解除借用，避免与 pending 的写借用冲突。
        let (id, mpsc_tx, rx) = {
            let mut inner = self.inner.lock();
            let active_tx = inner.active.as_ref().map(|a| a.tx.clone());
            let Some(active_tx) = active_tx else {
                return Err(
                    "浏览器扩展未连接。请先在 Chrome 中加载 vivian 浏览器桥扩展，或确认桥服务已启动"
                        .into(),
                );
            };
            inner.next_call_id += 1;
            let id = format!("tool-{}", inner.next_call_id);
            let (tx, rx) = oneshot::channel();
            inner.pending.insert(id.clone(), PendingCall { tx });
            (id, active_tx, rx)
        };

        let frame = tool_call(&id, name, args, now_ms() + timeout.as_millis() as u64, session_id);
        if mpsc_tx.send(Message::Text(serde_json::to_string(&frame).unwrap_or_else(|_| "{}".into()))).await.is_err() {
            self.fail_pending_id(&id, "浏览器桥连接已断开".into());
            return Err("浏览器扩展连接已断开，无法派发操作".into());
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(text))) => Ok(text),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err("浏览器扩展连接关闭，操作未完成".into()),
            Err(_elapsed) => {
                self.send_cancel(&id);
                Err(format!("浏览器操作 `{}` 超时（{}ms）", name, timeout.as_millis()))
            }
        }
    }

    fn fail_pending_id(&self, id: &str, err: String) {
        if let Some(p) = self.inner.lock().pending.remove(id) {
            let _ = p.tx.send(Err(err));
        }
    }

    fn send_cancel(&self, id: &str) {
        let tx = self.inner.lock().active.as_ref().map(|a| a.tx.clone());
        if let Some(tx) = tx {
            let frame = tool_cancel(id);
            let _ = tokio::spawn(async move {
                let _ = tx.send(Message::Text(serde_json::to_string(&frame).unwrap_or_else(|_| "{}".into()))).await;
            });
        }
        self.inner.lock().pending.remove(id);
    }
}

// ── 连接循环 ────────────────────────────────────────────────

/// 处理一次 WebSocket 升级：完成 hello 握手后接管连接，直到断开。
async fn run_connection(state: Arc<BridgeState>, socket: WebSocket) {
    let (mut write, mut read) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);

    // writer task：消费 out_rx 并写入连接。
    let write_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if write.send(msg).await.is_err() {
                break;
            }
        }
        let _ = write.close().await;
    });

    // 1) 握手：在 HELLO_TIMEOUT_MS 内必须收到携带正确 token 的 hello。
    let caps = match tokio::time::timeout(
        Duration::from_millis(HELLO_TIMEOUT_MS),
        read_frame(&mut read),
    )
    .await
    {
        Ok(Some(Ok(ClientFrame::Hello { token, caps }))) if constant_time_eq(&token, &state.token) => caps,
        _ => {
            let _ = out_tx
                .send(Message::Text(serde_json::to_string(&error_frame("auth-failed", "invalid or missing hello")).unwrap()))
                .await;
            let _ = write_task.await;
            return;
        }
    };

    let _ = out_tx.send(Message::Text(serde_json::to_string(&hello_ok(&caps)).unwrap())).await;

    // 2) 注册为活动连接（顶替旧连接）；旧连接的挂起调用继续等其自身超时自然失败。
    let generation = {
        let mut inner = state.inner.lock();
        inner.next_generation += 1;
        let gen = inner.next_generation;
        inner.active = Some(ActiveSocket {
            generation: gen,
            tx: out_tx.clone(),
        });
        gen
    };

    // 3) 读取循环：处理 tool.result / pong，并周期性 ping 探活。
    let mut ping_interval = tokio::time::interval(Duration::from_millis(PING_INTERVAL_MS));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            maybe = read_frame(&mut read) => {
                let Some(Ok(frame)) = maybe else { break };
                match frame {
                    ClientFrame::ToolResult { id, ok, result, error } => {
                        if let Some(p) = state.inner.lock().pending.remove(&id) {
                            let payload = if ok {
                                result.as_ref().map(text_of_result).ok_or_else(|| "扩展未返回结果".into())
                            } else {
                                Err(error.map_or("浏览器操作失败".into(), |e| format!("[{}] {}", e.code, e.message)))
                            };
                            let _ = p.tx.send(payload);
                        }
                    }
                    ClientFrame::Rpc { id, method, payload } => {
                        // 快照注入：扩展主动推送"跟随页面"快照，服务端缓存供 browser_snapshot 无参复用
                        if method == INJECT_BROWSER_SNAPSHOT_METHOD {
                            let text = payload
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| format!("{}", payload));
                            state.inject_snapshot(now_ms(), text);
                            let _ = out_tx
                                .send(Message::Text(serde_json::to_string(&json!({
                                    "t": "rpc.result", "id": id, "ok": true, "result": { "text": "ok" }
                                })).unwrap()))
                                .await;
                        } else if method == REPORT_PLATFORM_STATUS_METHOD {
                            // 平台登录态上报：扩展用 Cookie 哨兵探测后推送 {platforms:[{platform,loggedIn}]}
                            let mut status = std::collections::HashMap::new();
                            if let Some(arr) = payload.get("platforms").and_then(|p| p.as_array()) {
                                for item in arr {
                                    let platform = item
                                        .get("platform")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string();
                                    let logged_in = item
                                        .get("loggedIn")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false);
                                    if !platform.is_empty() {
                                        status.insert(platform, logged_in);
                                    }
                                }
                            }
                            state.report_platform_status(now_ms(), status);
                            let _ = out_tx
                                .send(Message::Text(serde_json::to_string(&json!({
                                    "t": "rpc.result", "id": id, "ok": true, "result": { "text": "ok" }
                                })).unwrap()))
                                .await;
                        } else if method == REPORT_X_COOKIE_METHOD {
                            // X Cookie 回传：扩展把 x.com 的 auth_token+ct0 拼成 Cookie 头推送，
                            // 供服务端 cookie 重放发现（twitter-cli）使用；空值忽略
                            if let Some(cookie) = payload
                                .get("cookie")
                                .and_then(Value::as_str)
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                            {
                                state.report_x_cookie(now_ms(), cookie);
                            }
                            let _ = out_tx
                                .send(Message::Text(serde_json::to_string(&json!({
                                    "t": "rpc.result", "id": id, "ok": true, "result": { "text": "ok" }
                                })).unwrap()))
                                .await;
                        } else if method == REPORT_REDDIT_COOKIE_METHOD {
                            // Reddit Cookie 回传：扩展把 reddit.com 登录态 Cookie 头推送，
                            // 供服务端同步 rdt-cli 凭据（登录态发现）使用；空值忽略
                            if let Some(cookie) = payload
                                .get("cookie")
                                .and_then(Value::as_str)
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                            {
                                state.report_reddit_cookie(now_ms(), cookie);
                            }
                            let _ = out_tx
                                .send(Message::Text(serde_json::to_string(&json!({
                                    "t": "rpc.result", "id": id, "ok": true, "result": { "text": "ok" }
                                })).unwrap()))
                                .await;
                        } else {
                            // 未知 RPC 方法：回执不支持
                            let _ = out_tx
                                .send(Message::Text(serde_json::to_string(&json!({
                                    "t": "rpc.result", "id": id, "ok": false,
                                    "error": { "code": "unsupported-method", "message": format!("不支持的 RPC 方法: {method}") }
                                })).unwrap()))
                                .await;
                        }
                    }
                    ClientFrame::Pong | ClientFrame::Hello { .. } => {}
                }
            }
            _ = ping_interval.tick() => {
                if out_tx.send(Message::Text(serde_json::to_string(&ping()).unwrap())).await.is_err() {
                    break;
                }
            }
        }
    }

    // 4) 清理：仅当仍是本人代际时移除活动连接，并失败所有挂起调用。
    {
        let mut inner = state.inner.lock();
        if inner.active.as_ref().map(|a| a.generation) == Some(generation) {
            inner.active = None;
        }
        let pending = std::mem::take(&mut inner.pending);
        for (_id, p) in pending {
            let _ = p.tx.send(Err("浏览器桥连接已关闭".into()));
        }
    }
    let _ = write_task.await;
}

/// 从连接读取一条合法文本帧；连接关闭 / 非文本 / 解析失败均返回 None。
async fn read_frame(
    read: &mut futures::stream::SplitStream<WebSocket>,
) -> Option<std::io::Result<ClientFrame>> {
    loop {
        match read.next().await {
            Some(Ok(Message::Text(text))) => {
                return parse_client_frame(&text).map(|f| Ok(f));
            }
            Some(Ok(Message::Close(_))) | None => return None,
            Some(Ok(_)) => continue,
            Some(Err(_)) => return None,
        }
    }
}

// ── axum 路由处理器 ─────────────────────────────────────────

/// `/ext/bridge`：WebSocket 升级入口。
pub async fn ws_handler(
    State(state): State<Arc<BridgeState>>,
    ws: axum::extract::WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| run_connection(state, socket))
}

/// `/ext/bridge-config`：零配置发现（仅回环）。返回 wsUrl / token / 连接状态 / 最近会话。
pub async fn config_handler(
    State(state): State<Arc<BridgeState>>,
) -> axum::Json<Value> {
    axum::Json(serde_json::json!({
        "wsUrl": format!("ws://127.0.0.1:{}{}", BRIDGE_PORT, BRIDGE_PATH),
        "token": state.token(),
        "connected": state.is_connected(),
        "lastSessionId": state.last_session_id(),
    }))
}

/// 启动桥服务（仅绑定 127.0.0.1）。由后端启动流程在一个 tokio 任务中调用。
pub async fn serve(state: Arc<BridgeState>) {
    use axum::routing::get;
    use axum::Router;

    let app = Router::new()
        .route(BRIDGE_CONFIG_PATH, get(config_handler))
        .route(BRIDGE_PATH, get(ws_handler))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", BRIDGE_PORT);
    tracing::info!("[Bridge] 浏览器自动化桥已就绪: ws://{}{}", addr, BRIDGE_PATH);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("[Bridge] 监听 {} 失败: {}", addr, e);
            return;
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("[Bridge] 桥服务运行出错: {}", e);
    }
}

// ── 小工具 ──────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成随机 token（两个 UUID 拼接，足够本地回环认证使用）。
fn generate_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4().simple())
}

/// 常量时间字符串比较（长度不等也计算，避免长度提前泄露）。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let len = a.len().min(b.len());
    let mut diff = (a.len() as u8) ^ (b.len() as u8);
    for i in 0..len {
        diff |= a[i] ^ b[i];
    }
    // 长度不同时，剩余字节也参与（与 b 的最后一个字节比较，避免提前退出）。
    if !b.is_empty() {
        for i in len..a.len() {
            diff |= a[i] ^ b[len - 1];
        }
    }
    let _ = std::hint::black_box(diff);
    diff == 0
}