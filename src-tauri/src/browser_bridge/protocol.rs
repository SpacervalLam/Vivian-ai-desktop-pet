//! 浏览器自动化桥的 WebSocket 线协议（与 Chrome 扩展共享的帧契约）。
//!
//! 帧是每个 WebSocket message 一个 JSON 对象，按 `t` 字段判别。correlation id
//! 由请求方生成、响应方原样回显，视为不透明字符串。

use serde_json::{json, Value};

/// 桥 WebSocket 路径
pub const BRIDGE_PATH: &str = "/ext/bridge";
/// 零配置发现端点：返回 `{ wsUrl, token }`（仅回环地址用）
pub const BRIDGE_CONFIG_PATH: &str = "/ext/bridge-config";

/// 桥服务默认监听端口（仅绑定 127.0.0.1）
pub const BRIDGE_PORT: u16 = 3080;

/// 新 socket 需在时限内提交 `hello`，否则关闭
pub const HELLO_TIMEOUT_MS: u64 = 5_000;
/// 服务端 ping 间隔；客户端回 `pong` 证明存活。
/// 必须显著小于 Chrome MV3 service worker 的 ~30s 空闲终止时限：
/// 每次 WS 消息交换都会重置该计时器（Chrome 116+），间隔设 20s 留出余量，
/// 否则 ping 与空闲终止同拍竞速会导致连接周期性掉线。
pub const PING_INTERVAL_MS: u64 = 20_000;
/// 单次工具调用默认预算（ms），为扩展侧页面稳定检测预留时间
pub const DEFAULT_TOOL_TIMEOUT_MS: u64 = 90_000;
/// 单个快照渲染字符上限
pub const SNAPSHOT_MAX_CHARS: usize = 32_000;
/// 单个快照交互清单条数上限
pub const MAX_INTERACTIVE_ITEMS: usize = 60;

/// 快照可见预算上下界（对齐扩展侧约束）
pub const MIN_SNAPSHOT_MAX_CHARS: usize = 500;

/// 扩展主动注入"跟随页面快照"的 RPC 方法名。
/// 扩展侧用户显式选择跟随标签后，把该页快照经 `rpc` 帧推送；服务端缓存，
/// 供 `browser_snapshot` 无参调用时优先返回（等价于把快照注入 Agent 上下文）。
pub const INJECT_BROWSER_SNAPSHOT_METHOD: &str = "bridge.injectBrowserSnapshot";
/// 扩展主动上报各平台登录态（Cookie 哨兵探测结果）
pub const REPORT_PLATFORM_STATUS_METHOD: &str = "bridge.reportPlatformStatus";
/// 扩展主动回传 X (Twitter) Cookie 头（auth_token + ct0），供服务端 cookie 重放发现。
pub const REPORT_X_COOKIE_METHOD: &str = "bridge.reportXCookie";
/// 扩展主动回传 Reddit Cookie 头（含 reddit_session），供服务端同步 rdt-cli 凭据。
pub const REPORT_REDDIT_COOKIE_METHOD: &str = "bridge.reportRedditCookie";

/// 一条工具调用失败（稳定机器码 + 面向模型的文本）
#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

/// 在 `hello`/`hello.ok` 中协商的能力——扩展自行执行动作，这些边界只影响快照。
#[derive(Debug, Clone)]
pub struct BridgeCaps {
    pub text_only: bool,
    pub snapshot_max_chars: usize,
    pub max_interactive_items: usize,
}

impl Default for BridgeCaps {
    fn default() -> Self {
        Self {
            text_only: true,
            snapshot_max_chars: SNAPSHOT_MAX_CHARS,
            max_interactive_items: MAX_INTERACTIVE_ITEMS,
        }
    }
}

impl BridgeCaps {
    pub fn to_value(&self) -> Value {
        json!({
            "textOnly": self.text_only,
            "snapshotMaxChars": self.snapshot_max_chars,
            "maxInteractiveItems": self.max_interactive_items,
        })
    }

    /// 从 JSON 对象解析能力。无效（含低于最小预算）返回 None。
    fn from_value(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let text_only = obj.get("textOnly").and_then(Value::as_bool)?;
        let snapshot_max_chars = obj.get("snapshotMaxChars").and_then(Value::as_u64)? as usize;
        if snapshot_max_chars < MIN_SNAPSHOT_MAX_CHARS {
            return None;
        }
        let max_interactive_items = obj.get("maxInteractiveItems").and_then(Value::as_u64)? as usize;
        Some(Self {
            text_only,
            snapshot_max_chars,
            max_interactive_items,
        })
    }
}

/// 扩展 -> 桥 的帧。
#[derive(Debug, Clone)]
pub enum ClientFrame {
    /// 首帧，需在 HELLO_TIMEOUT_MS 内提交。
    Hello { token: String, caps: BridgeCaps },
    /// 网关 RPC 透传（v1 不启用）
    Rpc { id: String, method: String, payload: Value },
    /// 之前分发的工具调用结果。
    ToolResult {
        id: String,
        ok: bool,
        result: Option<Value>,
        error: Option<ToolError>,
    },
    /// Liveness 应答。
    Pong,
}

/// 解析一条客户端帧；不是合法帧时返回 None。
pub fn parse_client_frame(text: &str) -> Option<ClientFrame> {
    let value: Value = serde_json::from_str(text).ok()?;
    let obj = value.as_object()?;
    let t = obj.get("t")?.as_str()?;
    match t {
        "hello" => {
            let token = obj.get("token")?.as_str()?.to_string();
            let caps = BridgeCaps::from_value(obj.get("caps")?)?;
            Some(ClientFrame::Hello { token, caps })
        }
        "rpc" => {
            let id = obj.get("id")?.as_str()?.to_string();
            let method = obj.get("method")?.as_str()?.to_string();
            let payload = obj.get("payload")?.clone();
            Some(ClientFrame::Rpc { id, method, payload })
        }
        "tool.result" => {
            let id = obj.get("id")?.as_str()?.to_string();
            let ok = obj.get("ok")?.as_bool()?;
            if ok {
                let result = obj.get("result").cloned();
                Some(ClientFrame::ToolResult { id, ok: true, result, error: None })
            } else {
                let error = obj.get("error").and_then(parse_tool_error)?;
                Some(ClientFrame::ToolResult { id, ok: false, result: None, error: Some(error) })
            }
        }
        "pong" => Some(ClientFrame::Pong),
        _ => None,
    }
}

fn parse_tool_error(v: &Value) -> Option<ToolError> {
    let obj = v.as_object()?;
    Some(ToolError {
        code: obj.get("code")?.as_str()?.to_string(),
        message: obj.get("message")?.as_str()?.to_string(),
    })
}

// ── 服务端帧构造（写向扩展） ────────────────────────────────

/// 认可扩展的连接并回显协商后的 caps。
pub fn hello_ok(caps: &BridgeCaps) -> Value {
    json!({ "t": "hello.ok", "caps": caps.to_value() })
}

/// 一条由模型请求的浏览器动作。
pub fn tool_call(id: &str, name: &str, args: &Value, expires_at_ms: u64, session_id: Option<&str>) -> Value {
    let mut frame = json!({
        "t": "tool.call",
        "id": id,
        "name": name,
        "args": args,
        "expiresAt": expires_at_ms,
    });
    if let Some(sid) = session_id {
        frame["sessionId"] = json!(sid);
    }
    frame
}

/// 撤回已派发但超时/被取消的工具调用。
pub fn tool_cancel(id: &str) -> Value {
    json!({ "t": "tool.cancel", "id": id })
}

/// Liveness 探针。
pub fn ping() -> Value {
    json!({ "t": "ping" })
}

/// 致命连接错误；客户端应重新认证。
pub fn error_frame(code: &str, message: &str) -> Value {
    json!({ "t": "error", "code": code, "message": message })
}

/// 取工具结果里的 `text` 字段；无则退回原始 JSON 字符串。
pub fn text_of_result(result: &Value) -> String {
    result
        .get("text")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}", result))
}