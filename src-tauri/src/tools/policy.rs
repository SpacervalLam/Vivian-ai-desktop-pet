//! 工具流水线策略缝（pre-execute / guard / post-execute）
//!
//! 现有 `executor` 内置了沙箱/权限/确认的硬编码顺序。这里通过 Cordis 式运行时
//! 事件总线开放一组**可插桩**的策略缝，让外部模块（如 sandbox、
//! approval、自定义守卫）能无侵入地加入 pre/post 阶段，而不用改动 executor 本体。
//!
//! 设计要点：
//! - **guard（瀑布 + 否决）**：`emit_waterfall` 依注册顺序处理同一个
//!   [`ToolGuardFlow`]，任一监听器返回 `None` 表示否决，或把 `decision` 置为
//!   `Deny / RequireConfirmation` 拒绝/转确认；
//! - **post-execute（可替换/阻断）**：执行后对结果做可替换与阻断。
//!
//! 默认不注册任何策略（空注册），现有执行行为保持不变；策略由上层按需挂载。

use serde_json::Value;

use crate::cordis::{BoxFuture, RuntimeContext};

/// guard 阶段载荷：携带工具名、参数与每个 guard 的裁决。
#[derive(Debug, Clone)]
pub struct ToolGuardFlow {
    pub tool_name: String,
    pub arguments: Value,
    pub char_id: String,
    /// guard 的最终裁决（`None` = 尚未裁决/放行）。
    pub decision: Option<ToolGuardDecision>,
    /// 供策略携带临时上下文。
    pub note: Option<String>,
}

/// guard 的裁决结果。
#[derive(Debug, Clone)]
pub enum ToolGuardDecision {
    /// 明确放行。
    Allow,
    /// 硬性拒绝（reason 进入返回给 LLM 的错误）。
    Deny { reason: String },
    /// 需要用户二次确认（reason 用于确认弹窗）。
    RequireConfirmation { reason: String },
}

/// post-execute 阶段载荷：工具执行结果的可变视图。
#[derive(Debug, Clone)]
pub struct ToolPostExecute {
    pub tool_name: String,
    pub arguments: Value,
    pub char_id: String,
    pub success: bool,
    pub data: Option<Value>,
    pub error: Option<String>,
    /// post 策略可替换结果内容。
    pub replace: Option<Value>,
    /// post 策略可阻断（整条结果改为错误）。
    pub block: Option<String>,
}

impl ToolPostExecute {
    /// 用替换内容与应用字符预算后回填到最终结果（替换优先于阻断不做，遵循语义）。
    pub fn into_result(self, max_chars: usize) -> Option<super::types::ToolResult> {
        if let Some(msg) = self.block {
            return Some(super::types::ToolResult::standard_error(
                &msg,
                Some("PostExecuteBlocked"),
                None,
            ));
        }
        if let Some(replaced) = self.replace {
            // 直通执行器预算裁剪，避免大结果塞进 LLM 上下文
            let budgeted = super::executor::budget_result(self.tool_name.clone(), replaced, max_chars);
            return Some(super::types::ToolResult::success(budgeted));
        }
        None
    }
}

/// 在运行时 ctx 上注册一个 guard 监听器。
///
/// `handler` 返回 `Some(flow)` 继续链；返回 `None` 表示否决（按 `Deny` 处理）。
pub fn register_guard<F>(ctx: &RuntimeContext, handler: F) -> crate::cordis::Disposer
where
    F: Fn(ToolGuardFlow) -> BoxFuture<'static, Option<ToolGuardFlow>> + Send + Sync + 'static,
{
    ctx.on_waterfall(handler)
}

/// 在运行时 ctx 上注册一个 post-execute 监听器。
///
/// `handler` 返回 `Some(pe)` 继续链；返回 `None` 表示阻断并中止后续 post 策略。
pub fn register_post_execute<F>(ctx: &RuntimeContext, handler: F) -> crate::cordis::Disposer
where
    F: Fn(ToolPostExecute) -> BoxFuture<'static, Option<ToolPostExecute>> + Send + Sync + 'static,
{
    ctx.on_waterfall(handler)
}

// ============================================================================
// 默认策略（挂载后可生效的纵深防御；与 executor 内置检查互补、非重复）
// ============================================================================

use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;

/// 常见密钥/令牌形状（post-execute 脱敏用）。命中的值替换为 `[REDACTED]`。
static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // sk- / pk- 开头的 API key（OpenAI 系 / 自定义）
        Regex::new(r"(?i)\b(sk|pk|rk)-[A-Za-z0-9_-]{12,}\b").unwrap(),
        // 常见 provider 前缀 key
        Regex::new(r"(?i)\b(ai[az][a-z]?|sk-[a-z0-9]+)-[A-Za-z0-9_-]{16,}\b").unwrap(),
        // Bearer / Token 后跟长串
        Regex::new(r"(?i)(bearer|token|apikey|api_key|secret)\s*[=:]\s*[A-Za-z0-9+/=_-]{16,}").unwrap(),
        // 私钥块
        Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----")
            .unwrap(),
    ]
});

/// 递归扫描工具参数，收集需要二次校验的可疑字符串值。
///
/// 与 executor 内置 `sandbox::check_tool_safety`（仅按固定 key 提取路径/命令）互补：
/// 这里扫描所有字符串值，专门捕获「参数值内嵌危险命令 / 路径穿越 / 危险 URL」的纵深攻击面。
fn scan_suspicious_values(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (_, v) in map {
                scan_suspicious_values(v, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                scan_suspicious_values(v, out);
            }
        }
        serde_json::Value::String(s) => {
            if !s.is_empty() {
                out.push(s.clone());
            }
        }
        _ => {}
    }
}

/// 单个字符串是否命中危险内容（命令/穿越/URL）。
fn is_suspicious_string(s: &str) -> Option<&'static str> {
    let lower = s.to_lowercase();
    // 内嵌换行或命令连接符 → 疑似命令注入
    if lower.contains('\n')
        || lower.contains("; ") && lower.contains("rm ")
        || lower.contains(" && ")
        || lower.contains(" || ")
        || lower.contains("powershell")
            && (lower.contains("-enc") || lower.contains("iex"))
    {
        return Some("命令注入模式");
    }
    // 路径穿越（.. 前后带路径分隔符）
    if s.contains("..") && (s.contains('/') || s.contains('\\')) {
        let has_traversal = s
            .split(['/', '\\'])
            .any(|seg| seg == "..");
        if has_traversal {
            return Some("路径穿越序列");
        }
    }
    // 危险 URL 协议
    if lower.starts_with("javascript:")
        || lower.starts_with("data:text/html")
        || lower.starts_with("file://")
    {
        return Some("危险 URL 协议");
    }
    None
}

/// 挂载默认策略到运行时 ctx：guard 纵深扫描 + post-execute 结果脱敏。
///
/// 返回组合 Disposer（drop 时全部注销）；通常进程生命周期内持存，返回值可忽略。
pub fn register_default_policies(ctx: &RuntimeContext) -> crate::cordis::Disposer {
    let ctx = Arc::new(ctx.clone());

    // ── guard：通用参数纵深扫描 ──
    let guard_ctx = Arc::clone(&ctx);
    let guard = register_guard(&guard_ctx, move |mut flow: ToolGuardFlow| {
        Box::pin(async move {
            let mut suspicious: Vec<String> = Vec::new();
            scan_suspicious_values(&flow.arguments, &mut suspicious);
            for s in &suspicious {
                if let Some(kind) = is_suspicious_string(s) {
                    flow.decision = Some(ToolGuardDecision::Deny {
                        reason: format!("默认策略拦截：{}（参数值不合法）", kind),
                    });
                    flow.note = Some(kind.to_string());
                    return Some(flow);
                }
            }
            Some(flow)
        })
    });

    // ── post-execute：结果脱敏 ──
    let post = register_post_execute(&ctx, move |mut pe: ToolPostExecute| {
        Box::pin(async move {
            if pe.success {
                if let Some(data) = pe.data.take() {
                    pe.data = Some(redact_json(&data));
                }
            }
            Some(pe)
        })
    });

    // 组合 Disposer：任一注销时全部注销
    let guard_dispose = guard.clone();
    let post_dispose = post.clone();
    crate::cordis::Disposer::new(move || {
        guard_dispose.dispose();
        post_dispose.dispose();
    })
}

/// 递归脱敏 JSON 中的密钥/令牌形状。
fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let v = if k.to_lowercase().contains("key")
                    || k.to_lowercase().contains("token")
                    || k.to_lowercase().contains("secret")
                    || k.to_lowercase().contains("password")
                    || k.to_lowercase().contains("credential")
                {
                    // 敏感 key：整个值掩码（保留类型形态便于后续展示）
                    match v {
                        serde_json::Value::String(_) => serde_json::Value::String("[REDACTED]".into()),
                        serde_json::Value::Null => v.clone(),
                        other => other.clone(),
                    }
                } else {
                    redact_json(v)
                };
                out.insert(k.clone(), v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_json).collect())
        }
        serde_json::Value::String(s) => {
            let mut s = s.clone();
            for re in SECRET_PATTERNS.iter() {
                s = re.replace_all(&s, "[REDACTED]").into_owned();
            }
            serde_json::Value::String(s)
        }
        other => other.clone(),
    }
}