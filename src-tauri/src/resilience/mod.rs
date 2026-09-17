use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// 半开探测的兜底时长。
///
/// 探测请求可能不回报结果：响应解析失败会提前 `?` 返回，大 prompt 又按设计跳过
/// 熔断器记账。以时刻记账 + 超时重新放行，可避免熔断器被这类请求永久卡在半开。
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub failure_threshold: u32,
    pub failure_rate_threshold: f64,
    pub reset_timeout: Duration,
    pub last_failure_time: Option<Instant>,
    /// 滑动窗口：最近一次调用是否成功（true=成功，false=失败）
    pub recent_results: VecDeque<bool>,
    /// 滑动窗口大小（默认 20）
    pub window_size: u32,
    /// 触发失败率判定的最小样本数（默认 5）
    pub min_samples: u32,
    /// 半开探测请求的放行时刻。
    ///
    /// 半开只应放行一个请求去试探后端是否恢复，其余立即拒绝，否则「探测」会退化成
    /// 全放行——刚充值的瞬间积压请求一起涌入，其中任一失败就立刻打回熔断。
    /// 记时刻而非布尔量，是为了让超时未回报的探测能够重新放行（见 [`PROBE_TIMEOUT`]）。
    probe_started_at: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, threshold: u32, rate: f64, timeout: Duration) -> Self {
        Self {
            name: name.into(),
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            failure_threshold: threshold,
            failure_rate_threshold: rate,
            reset_timeout: timeout,
            last_failure_time: None,
            recent_results: VecDeque::new(),
            window_size: 20,
            min_samples: 5,
            probe_started_at: None,
        }
    }

    /// 半开探测是否还能放行：无探测在飞行中，或上一个探测已超时未回报。
    fn probe_slot_available(&self) -> bool {
        match self.probe_started_at {
            None => true,
            Some(at) => at.elapsed() >= PROBE_TIMEOUT,
        }
    }

    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                if self.probe_slot_available() {
                    self.probe_started_at = Some(Instant::now());
                    true
                } else {
                    false
                }
            }
            CircuitState::Open => {
                if let Some(last) = self.last_failure_time {
                    if last.elapsed() >= self.reset_timeout {
                        self.state = CircuitState::HalfOpen;
                        // 放行的这一个请求即探测请求
                        self.probe_started_at = Some(Instant::now());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.success_count += 1;
        self.push_result(true);
        // 探测已回报结果，释放半开占位
        self.probe_started_at = None;
        self.state = CircuitState::Closed;
    }

    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_time = Some(Instant::now());
        self.push_result(false);
        // 探测已回报结果，释放半开占位
        self.probe_started_at = None;

        // 半开状态失败 → 立即熔断
        if self.state == CircuitState::HalfOpen {
            self.state = CircuitState::Open;
            return;
        }

        if self.state == CircuitState::Closed {
            let samples = self.recent_results.len() as u32;
            let rate = self.recent_failure_rate();
            // 失败次数达阈值 或 滑动窗口失败率达阈值（需满足最小样本数）→ 熔断
            if self.failure_count >= self.failure_threshold
                || (samples >= self.min_samples && rate >= self.failure_rate_threshold)
            {
                self.state = CircuitState::Open;
            }
        }
    }

    /// 记录一次调用结果到滑动窗口，并维护窗口大小
    fn push_result(&mut self, success: bool) {
        self.recent_results.push_back(success);
        while (self.recent_results.len() as u32) > self.window_size {
            self.recent_results.pop_front();
        }
    }

    /// 计算滑动窗口内的失败率（false 比例）
    fn recent_failure_rate(&self) -> f64 {
        if self.recent_results.is_empty() {
            return 0.0;
        }
        let failures = self.recent_results.iter().filter(|&&r| !r).count();
        failures as f64 / self.recent_results.len() as f64
    }

    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.success_count = 0;
        self.last_failure_time = None;
        self.recent_results.clear();
        self.probe_started_at = None;
    }

    pub fn get_stats(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "state": self.state,
            "failure_count": self.failure_count,
            "success_count": self.success_count,
            "failure_threshold": self.failure_threshold,
            "failure_rate_threshold": self.failure_rate_threshold,
            "reset_timeout_ms": self.reset_timeout.as_millis() as u64,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorCategory {
    Permanent,
    Transient,
    RateLimit,
}

/// 把细分的错误类别折叠成「该不该重试」的三分类。
///
/// 两个 API 的分工：`LlmErrorKind` 决定给用户看什么提示，`ErrorCategory` 决定
/// 要不要退避重试。二者必须同源，否则会出现「提示说余额不足、后台却在猛重试」。
///
/// `CircuitBreakerOpen` 归 Transient 是沿用的既有语义：熔断器自身会拦掉后续请求，
/// 重试判定无需替它兜底。
fn kind_to_category(kind: &LlmErrorKind) -> ErrorCategory {
    match kind {
        LlmErrorKind::RateLimited => ErrorCategory::RateLimit,
        LlmErrorKind::ServerError
        | LlmErrorKind::Overloaded
        | LlmErrorKind::Timeout
        | LlmErrorKind::NetworkError
        | LlmErrorKind::CircuitBreakerOpen
        | LlmErrorKind::Unknown => ErrorCategory::Transient,
        // 欠费、认证失败、模型不存在、上下文超长、内容审核、参数错误：
        // 重试多少次都不会变好，必须立刻失败并把问题交回给人。
        LlmErrorKind::InvalidApiKey
        | LlmErrorKind::InsufficientBalance
        | LlmErrorKind::QuotaExceeded
        | LlmErrorKind::ModelNotFound
        | LlmErrorKind::ContextLengthExceeded
        | LlmErrorKind::ContentPolicy
        | LlmErrorKind::BadRequest
        | LlmErrorKind::RegionNotSupported
        | LlmErrorKind::PermissionDenied => ErrorCategory::Permanent,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    InvalidApiKey,
    InsufficientBalance,
    QuotaExceeded,
    RateLimited,
    ModelNotFound,
    ContextLengthExceeded,
    ContentPolicy,
    ServerError,
    Overloaded,
    Timeout,
    NetworkError,
    BadRequest,
    RegionNotSupported,
    PermissionDenied,
    CircuitBreakerOpen,
    Unknown,
}

pub fn classify_error(error: &dyn std::error::Error) -> ErrorCategory {
    classify_error_from_str(&error.to_string())
}

pub fn classify_error_from_str(msg: &str) -> ErrorCategory {
    if msg.contains("circuit_breaker") || msg.contains("熔断器") {
        return ErrorCategory::Transient;
    }

    // 与错误提示共用同一张厂商映射表。Provider 错误里混着语义完全相反的两类故障，
    // 结构化判定必须优先：智谱欠费的提示是「429 + 您的账户已欠费」，只看见 429 会判成
    // 限流类，而限流是可重试类——等于在余额耗尽时反复重试。
    if let Some(kind) = classify_structured(msg) {
        return kind_to_category(&kind);
    }

    let msg = msg.to_lowercase();
    if msg.contains("401")
        || msg.contains("invalid_api_key")
        || msg.contains("invalid authentication")
        || msg.contains("incorrect api key")
    {
        ErrorCategory::Permanent
    } else if msg.contains("400")
        || msg.contains("bad request")
        || msg.contains("invalid_argument")
    {
        // 400 Bad Request：请求格式错误，重试无意义
        ErrorCategory::Permanent
    } else if msg.contains("insufficient")
        || msg.contains("balance")
        || msg.contains("quota")
        // 欠费类在中文与各厂商自有命名下没有统一词根，逐一列出：
        // 阿里百炼 Arrearage、火山/百度 overdue、智谱「账户已欠费」
        || msg.contains("arrearage")
        || msg.contains("overdue")
        || msg.contains("欠费")
        || (msg.contains("403") && !msg.contains("country") && !msg.contains("region") && !msg.contains("territory"))
    {
        ErrorCategory::Permanent
    } else if msg.contains("404")
        || msg.contains("model_not_found")
        || msg.contains("does not exist")
    {
        ErrorCategory::Permanent
    } else if msg.contains("429") || msg.contains("rate_limit") || msg.contains("rate limit") {
        ErrorCategory::RateLimit
    } else if msg.contains("context_length")
        || msg.contains("content_policy")
        || msg.contains("content_filter")
        || msg.contains("moderation")
    {
        ErrorCategory::Permanent
    } else if msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("internal server error")
        || msg.contains("overloaded")
    {
        ErrorCategory::Transient
    } else if msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("deadline")
    {
        ErrorCategory::Transient
    } else if msg.contains("dns")
        || msg.contains("unreachable")
        || msg.contains("refused")
        || msg.contains("broken pipe")
        || msg.contains("certificate")
        || msg.contains("ssl")
        || msg.contains("hyper error")
    {
        ErrorCategory::Transient
    } else if msg.contains("country")
        || msg.contains("region")
        || msg.contains("territory")
        || msg.contains("geolocation")
        || msg.contains("ip not authorized")
        || msg.contains("ip allowlist")
        || msg.contains("ip whitelist")
    {
        ErrorCategory::Permanent
    } else if msg.contains("permission")
        || msg.contains("access denied")
    {
        ErrorCategory::Permanent
    } else if msg.contains("circuit_breaker")
        || msg.contains("熔断器")
    {
        ErrorCategory::Transient
    } else {
        ErrorCategory::Transient
    }
}

pub fn classify_llm_error(error: &dyn std::error::Error) -> LlmErrorKind {
    classify_llm_error_from_str(&error.to_string())
}

/// provider 响应里的结构化错误字段。
///
/// 各厂商对同一故障类别用完全不同的「状态码 + 业务码」组合表达，仅凭 HTTP 状态码
/// 会判错，仅凭错误串子串匹配也会判错（见 [`extract_status_code`] 的边界说明）。
/// 因此统一先解出三元组，再由 [`classify_vendor_error`] 查表定性。
#[derive(Debug, Default)]
struct ParsedProviderError {
    status: Option<u16>,
    /// 厂商业务码或 OpenAI 的 `error.type`，保留原始大小写以便还原驼峰形态
    code: Option<String>,
    /// 服务端 `message`，已转小写
    message: String,
}

/// 仅当三位数出现在紧跟左括号的位置时才认定是状态码，并要求落在已知集合内。
///
/// provider 层统一把响应格式化为 `… 请求失败 (402 Payment Required): {json}`，
/// 左括号是可靠的锚点。若放宽成全文子串匹配，`request_id=4012…`、`bandwidth 500`
/// 这类噪声都会被误认成 HTTP 状态。
const RECOGNIZED_STATUS: &[u16] = &[400, 401, 402, 403, 404, 408, 409, 422, 429, 498, 499, 500, 502, 503, 504];

fn extract_status_code(raw: &str) -> Option<u16> {
    for (i, _) in raw.match_indices('(') {
        let rest = raw[i + 1..].trim_start_matches(|c: char| !c.is_ascii_digit());
        let digits: String = rest.chars().take(3).collect();
        if digits.len() == 3 {
            if let Ok(n) = digits.parse::<u16>() {
                if RECOGNIZED_STATUS.contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// 键名大小写不敏感取值。
///
/// 信封字段大小写不统一：`error.code`（OpenAI 系）、`error.Code`（腾讯云兼容层）、
/// `Error.Code`（腾讯云原生 API）必须同等对待，精确匹配会整条路径落空。
fn pick_ci<'a>(obj: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    obj.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

fn find_error_code(value: &serde_json::Value) -> Option<String> {
    let mut containers: Vec<&serde_json::Value> = vec![value];
    if let Some(r) = pick_ci(value, "Response") {
        containers.push(r);
    }
    for k in ["error", "Error", "base_resp"] {
        if let Some(c) = pick_ci(value, k) {
            containers.push(c);
            // 腾讯云原生形态 Response.Error 的内层
            if let Some(inner) = pick_ci(c, "Error") {
                containers.push(inner);
            }
        }
    }
    for container in containers {
        for key in ["code", "type", "status_code"] {
            let Some(v) = pick_ci(container, key) else { continue };
            match v {
                // 腾讯混元的业务码是整型，其余多为字符串，两种都要接住
                serde_json::Value::String(s) if !s.is_empty() => return Some(s.clone()),
                serde_json::Value::Number(n) => return Some(n.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn extract_code_and_message(raw: &str) -> (Option<String>, String) {
    if let Some(start) = raw.find('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw[start..].trim()) {
            let code = find_error_code(&value);
            let message = pick_ci(&value, "error")
                .or_else(|| pick_ci(&value, "Error"))
                .and_then(|e| pick_ci(e, "message").or_else(|| pick_ci(e, "Message")))
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_lowercase();
            return (code, message);
        }
    }
    (None, String::new())
}

/// 业务码 / message 是否命中任一关键词。
///
/// 同时比对原串与「去分隔符」形态，兼顾三种命名风格：
/// `insufficient_balance`（下划线）、`AccountOverdueError`（驼峰）、`1113`（纯数字）。
fn matches_any(code: &Option<String>, needles: &[&str]) -> bool {
    let Some(c) = code else { return false };
    let lower = c.to_lowercase();
    let flat = lower.replace(['_', '-', '.', ' '], "");
    needles.iter().any(|n| {
        // needle 同样要归一化：业务码有下划线、驼峰、纯数字三种书写，
        // 不统一大小写的话形如 `PrepaidBillOverdue` 的驼峰永远匹配不上。
        let needle = n.to_lowercase();
        let needle_flat = needle.replace(['_', '-', '.', ' '], "");
        lower.contains(&needle) || flat.contains(&needle_flat)
    })
}

fn message_matches(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| message.contains(n))
}

/// 按 (状态码, 业务码, message) 查厂商映射表，返回 None 表示本表未覆盖。
///
/// 判定顺序是本表的关键：同一状态码下必须先排除「厂商把钱的问题塞进来的情况」，
/// 才能落到该状态码的默认语义。
fn classify_vendor_error(p: &ParsedProviderError) -> Option<LlmErrorKind> {
    let status = p.status?;
    let code = &p.code;
    let msg = &p.message;

    match status {
        401 => Some(LlmErrorKind::InvalidApiKey),
        404 => Some(LlmErrorKind::ModelNotFound),
        408 => Some(LlmErrorKind::Timeout),
        500 | 502 | 504 => Some(LlmErrorKind::ServerError),
        503 => Some(LlmErrorKind::Overloaded),

        // 402 在多数 OpenAI 兼容层的本义就是「没钱」：DeepSeek、腾讯混元(403004)、
        // MiniMax(1008)、小米 MiMo、OpenRouter。两家例外同属配额而非余额：
        // 腾讯把「免费额度耗尽」也放在 402（401007/401008），Together 用 402 表达
        // 月度消费上限——都不会因为重试而恢复。
        402 => {
            if matches_any(code, &["401007", "401008"])
                || message_matches(msg, &["monthly spend", "spending limit", "monthly limit"])
            {
                return Some(LlmErrorKind::QuotaExceeded);
            }
            Some(LlmErrorKind::InsufficientBalance)
        }

        // 429 只有一部分是真限流。多家厂商把「没钱」映射到 429：
        // 智谱业务码 1113、阿里账单逾期、百度 AppBuilder 断供、OpenAI 配额耗尽、
        // Kimi 额度耗尽、Anthropic tier 月度上限。这类降级在多数 firms 不返回
        // Retry-After，退避重试只会把失败记录刷满熔断器。
        429 => {
            if matches_any(
                code,
                &[
                    "1113",
                    "PrepaidBillOverdue",
                    "PostpaidBillOverdue",
                    "BILLING_INSUFFICIENT_BALANCE",
                ],
            ) || message_matches(msg, &["insufficient balance", "账户已欠费", "余额不足"])
            {
                return Some(LlmErrorKind::InsufficientBalance);
            }
            if matches_any(
                code,
                &[
                    "insufficient_quota",
                    "exceeded_current_quota",
                    "enforced_spend_limit_reached",
                ],
            ) {
                return Some(LlmErrorKind::QuotaExceeded);
            }
            Some(LlmErrorKind::RateLimited)
        }

        // 403 的语义在各厂商互相排斥：火山方舟与百度用它报欠费，OpenRouter 报内容
        // 审核命中，Together 报上下文超长，其余才是真正的权限不足。按状态码统一提示
        // 「权限不足」会严重误导，必须落到 code 上区分。
        403 => {
            if matches_any(code, &["AccountOverdueError", "ServiceOverdue", "account_overdue"])
                || message_matches(
                    msg,
                    &[
                        "overdue balance",
                        "overdue account",
                        "overdue payment",
                        "access denied due to overdue",
                    ],
                )
            {
                return Some(LlmErrorKind::InsufficientBalance);
            }
            if matches_any(code, &["moderation"]) || message_matches(msg, &["moderation flagged"])
            {
                return Some(LlmErrorKind::ContentPolicy);
            }
            if message_matches(msg, &["context length", "max_tokens", "context window"]) {
                return Some(LlmErrorKind::ContextLengthExceeded);
            }
            Some(LlmErrorKind::PermissionDenied)
        }

        // 400 通常是请求格式错误，但两家把余额问题放在 400：阿里百炼的 Arrearage，
        // Anthropic 的「credit balance is too low」。落进 BadRequest 会让用户误以为
        // 是自己的请求写错了。
        400 => {
            if matches_any(code, &["Arrearage", "OUT_OF_SERVICE"])
                || message_matches(
                    msg,
                    &["credit balance is too low", "account is in good standing"],
                )
            {
                return Some(LlmErrorKind::InsufficientBalance);
            }
            Some(LlmErrorKind::BadRequest)
        }

        _ => None,
    }
}

/// 结构化分类的统一入口：能从错误串里解出 HTTP 状态码时才返回 Some。
///
/// `classify_error_from_str`（决定重不重试）与 `classify_llm_error_from_str`
/// （决定给用户看什么）都必须从这里过，保证同一张厂商表是唯一真源；
/// 解不出状态码时各自回落到自己的关键字启发式。
fn classify_structured(msg: &str) -> Option<LlmErrorKind> {
    if msg.contains("circuit_breaker") || msg.contains("熔断器") {
        return Some(LlmErrorKind::CircuitBreakerOpen);
    }
    let status = extract_status_code(msg)?;
    let (code, message) = extract_code_and_message(msg);
    classify_vendor_error(&ParsedProviderError {
        status: Some(status),
        code,
        message,
    })
}

/// 供重试判定使用：只在能从错误串里结构化解出故障类别时才给出结论。
///
/// 返回 `None` 表示无法定性（例如本地错误被包装成 `Provider`），此时调用方应沿用
/// 「可重试」的默认语义。刻意不走关键字兜底：兜底会在子串上误伤——错误串里偶然
/// 出现 `400`（request_id、token 数）就会被判成不可重试，而「误判为不可重试」会
/// 直接让请求失败，「误判为可重试」只是多退避几次，两者代价不对称。
pub fn classified_retry_verdict(msg: &str) -> Option<bool> {
    let kind = classify_structured(msg)?;
    Some(matches!(
        kind_to_category(&kind),
        ErrorCategory::Transient | ErrorCategory::RateLimit
    ))
}

pub fn classify_llm_error_from_str(msg: &str) -> LlmErrorKind {
    if msg.contains("circuit_breaker") || msg.contains("熔断器") {
        return LlmErrorKind::CircuitBreakerOpen;
    }

    // 先按 (状态码, 业务码, message) 查厂商映射表。必须优先于下面的子串匹配：
    // 智谱欠费的提示是「429 + 您的账户已欠费」，子串匹配只看得见 "429" 会判成限流，
    // 而限流是可重试类——等于在余额耗尽时反复重试。
    if let Some(kind) = classify_structured(msg) {
        return kind;
    }

    let lower = msg.to_lowercase();

    if lower.contains("invalid_api_key")
        || lower.contains("invalid authentication")
        || lower.contains("incorrect api key")
        || (lower.contains("api key") && (lower.contains("invalid") || lower.contains("expired") || lower.contains("revoked") || lower.contains("incorrect")))
        || (lower.contains("401") && !lower.contains("ip"))
    {
        return LlmErrorKind::InvalidApiKey;
    }

    if (lower.contains("403") && (lower.contains("insufficient") || lower.contains("balance")))
        || lower.contains("account balance is insufficient")
        || lower.contains("insufficient balance")
        || lower.contains("insufficient_quota")
        || lower.contains("余额不足")
        // 各厂商自有命名，无统一词根：阿里百炼 Arrearage、火山/百度 overdue、
        // 智谱「账户已欠费」。这条兜底只在结构化判定未命中时生效。
        || lower.contains("arrearage")
        || lower.contains("overdue")
        || lower.contains("欠费")
    {
        return LlmErrorKind::InsufficientBalance;
    }

    if lower.contains("exceeded your current quota")
        || lower.contains("quota exceeded")
        || lower.contains("billing details")
        || lower.contains("run out of credits")
        || lower.contains("you exceeded")
    {
        return LlmErrorKind::QuotaExceeded;
    }

    if lower.contains("429")
        || lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("slow down")
    {
        return LlmErrorKind::RateLimited;
    }

    if lower.contains("model_not_found")
        || lower.contains("does not exist")
        || (lower.contains("model") && lower.contains("not found"))
        || (lower.contains("model") && lower.contains("unavailable"))
        || (lower.contains("404") && lower.contains("model"))
    {
        return LlmErrorKind::ModelNotFound;
    }

    if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("token limit")
        || (lower.contains("max_tokens") && lower.contains("exceed"))
    {
        return LlmErrorKind::ContextLengthExceeded;
    }

    if lower.contains("content_policy")
        || lower.contains("content_filter")
        || lower.contains("content was filtered")
        || lower.contains("moderation")
        || lower.contains("rejected")
        || lower.contains("敏感")
    {
        return LlmErrorKind::ContentPolicy;
    }

    if lower.contains("500") || lower.contains("internal server error") {
        return LlmErrorKind::ServerError;
    }

    if lower.contains("overloaded")
        || lower.contains("engine is currently overloaded")
        || lower.contains("capacity")
        || (lower.contains("503") && !lower.contains("model"))
    {
        return LlmErrorKind::Overloaded;
    }

    if lower.contains("502") || lower.contains("504") {
        return LlmErrorKind::ServerError;
    }

    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("deadline exceeded")
        || lower.contains("request timeout")
    {
        return LlmErrorKind::Timeout;
    }

    if lower.contains("dns")
        || lower.contains("unreachable")
        || lower.contains("refused")
        || lower.contains("broken pipe")
        || lower.contains("certificate")
        || lower.contains("ssl")
        || lower.contains("hyper error")
        || (lower.contains("connect") && (lower.contains("error") || lower.contains("fail") || lower.contains("reset")))
    {
        return LlmErrorKind::NetworkError;
    }

    if lower.contains("country")
        || lower.contains("region")
        || lower.contains("territory")
        || lower.contains("geolocation")
        || lower.contains("ip not authorized")
        || lower.contains("ip allowlist")
        || lower.contains("ip whitelist")
    {
        return LlmErrorKind::RegionNotSupported;
    }

    if lower.contains("permission")
        || lower.contains("forbidden")
        || lower.contains("access denied")
        || lower.contains("not authorized")
    {
        return LlmErrorKind::PermissionDenied;
    }

    if lower.contains("bad request")
        || lower.contains("400")
    {
        return LlmErrorKind::BadRequest;
    }

    LlmErrorKind::Unknown
}

pub fn error_kind_to_message_key(kind: &LlmErrorKind) -> &'static str {
    match kind {
        LlmErrorKind::InvalidApiKey => "toast.llm_error_invalid_api_key",
        LlmErrorKind::InsufficientBalance => "toast.llm_error_insufficient_balance",
        LlmErrorKind::QuotaExceeded => "toast.llm_error_quota_exceeded",
        LlmErrorKind::RateLimited => "toast.llm_error_rate_limited",
        LlmErrorKind::ModelNotFound => "toast.llm_error_model_not_found",
        LlmErrorKind::ContextLengthExceeded => "toast.llm_error_context_length",
        LlmErrorKind::ContentPolicy => "toast.llm_error_content_policy",
        LlmErrorKind::ServerError => "toast.llm_error_server_error",
        LlmErrorKind::Overloaded => "toast.llm_error_overloaded",
        LlmErrorKind::Timeout => "toast.llm_error_timeout",
        LlmErrorKind::NetworkError => "toast.llm_error_network",
        LlmErrorKind::BadRequest => "toast.llm_error_bad_request",
        LlmErrorKind::RegionNotSupported => "toast.llm_error_region",
        LlmErrorKind::PermissionDenied => "toast.llm_error_permission",
        LlmErrorKind::CircuitBreakerOpen => "toast.llm_error_circuit_breaker",
        LlmErrorKind::Unknown => "toast.llm_error_unknown",
    }
}

#[derive(Debug)]
pub struct CircuitBreakerError(pub String);

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CircuitBreaker error: {}", self.0)
    }
}

impl std::error::Error for CircuitBreakerError {}

// 全局熔断器注册表：所有调用方通过 Arc 共享同一熔断器状态，避免 clone 导致状态不同步
pub static GLOBAL_BREAKERS: Lazy<RwLock<HashMap<String, Arc<RwLock<CircuitBreaker>>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// 注册熔断器：返回 Arc 共享句柄，调用方与注册表持有同一 Arc，状态实时同步
pub fn register_circuit_breaker(
    name: String,
    threshold: u32,
    rate: f64,
    timeout: Duration,
) -> Arc<RwLock<CircuitBreaker>> {
    let breaker = Arc::new(RwLock::new(CircuitBreaker::new(
        name.clone(),
        threshold,
        rate,
        timeout,
    )));
    let mut breakers = GLOBAL_BREAKERS.write();
    breakers.insert(name, Arc::clone(&breaker));
    breaker
}

// 获取熔断器共享句柄：返回与注册表相同的 Arc，状态实时同步
pub fn get_circuit_breaker(name: &str) -> Option<Arc<RwLock<CircuitBreaker>>> {
    let breakers = GLOBAL_BREAKERS.read();
    breakers.get(name).map(|arc| Arc::clone(arc))
}

// ===================== 重试机制 =====================

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 最大尝试次数（含首次调用，默认 3）
    pub max_attempts: u32,
    /// 基础退避时长（默认 500ms）
    pub base_delay: Duration,
    /// 最大退避时长（默认 10s）
    pub max_delay: Duration,
    /// 是否启用 jitter 抖动（默认 true）
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// 计算第 `attempt` 次失败后的重试延迟（指数退避 + 可选 jitter）
    ///
    /// - 指数退避：`delay = min(max_delay, base_delay * 2^(attempt-1))`
    /// - RateLimit 错误使用更长退避（×2）
    /// - 启用 jitter 时叠加 ±20% 随机抖动
    pub fn get_delay(&self, attempt: u32, category: &ErrorCategory) -> Duration {
        // 指数退避：base_delay * 2^(attempt-1)，限制指数防止溢出
        let exp = attempt.saturating_sub(1).min(31);
        let multiplier = 1u64 << exp;
        let base_ms = self.base_delay.as_millis() as u64;
        let mut delay_ms = base_ms.saturating_mul(multiplier);

        // RateLimit 错误用更长退避（×2）
        if matches!(category, ErrorCategory::RateLimit) {
            delay_ms = delay_ms.saturating_mul(2);
        }

        // 不超过最大延迟
        let max_ms = self.max_delay.as_millis() as u64;
        delay_ms = delay_ms.min(max_ms);

        if self.jitter {
            // ±20% 随机抖动
            let factor = pseudo_random_factor();
            delay_ms = ((delay_ms as f64) * factor) as u64;
        }

        Duration::from_millis(delay_ms)
    }
}

/// 基于时间的伪随机因子（无 rand 依赖时使用），返回 [0.8, 1.2)
fn pseudo_random_factor() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let r = (nanos as f64) / (u32::MAX as f64);
    0.8 + r * 0.4
}

/// 异步重试：按 `RetryConfig` 对 `operation` 进行指数退避重试
///
/// - Transient/RateLimit 错误会重试，Permanent 错误立即返回
/// - 达到 `max_attempts` 仍失败则返回最后一次错误
pub async fn async_retry<F, Fut, T, E>(config: &RetryConfig, operation: F) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::error::Error,
{
    let mut attempt = 1u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let category = classify_error(&err);
                // Permanent 错误不重试
                if matches!(category, ErrorCategory::Permanent) {
                    return Err(err);
                }
                // 达到最大尝试次数，返回错误
                if attempt >= config.max_attempts {
                    return Err(err);
                }
                let delay = config.get_delay(attempt, &category);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// 组合重试 + 熔断器：每次调用前检查熔断器，失败时记录并按配置重试
///
/// - 熔断器打开时直接返回错误
/// - 调用成功记录 success，失败记录 failure
/// - Permanent 错误不重试，其余按 `RetryConfig` 重试
pub async fn with_retry_and_breaker<F, Fut, T, E>(
    breaker: &Arc<RwLock<CircuitBreaker>>,
    config: &RetryConfig,
    operation: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut attempt = 1u32;
    loop {
        // 熔断器检查
        {
            let mut b = breaker.write();
            if !b.allow_request() {
                return Err(anyhow::anyhow!("熔断器已打开，拒绝请求: {}", b.name));
            }
        }

        match operation().await {
            Ok(value) => {
                breaker.write().record_success();
                return Ok(value);
            }
            Err(err) => {
                let category = classify_error(&err);
                breaker.write().record_failure();

                if matches!(category, ErrorCategory::Permanent) {
                    return Err(err.into());
                }
                if attempt >= config.max_attempts {
                    return Err(err.into());
                }
                let delay = config.get_delay(attempt, &category);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 走一遍完整分类入口：结构化命中优先，未命中落到 Unknown（真实链路由后续
    /// 关键字兜底，这里只验证厂商映射表本身的判定）。
    fn kind_of(raw: &str) -> LlmErrorKind {
        if raw.contains("circuit_breaker") || raw.contains("熔断器") {
            return LlmErrorKind::CircuitBreakerOpen;
        }
        match extract_status_code(raw) {
            Some(status) => {
                let (code, message) = extract_code_and_message(raw);
                classify_vendor_error(&ParsedProviderError {
                    status: Some(status),
                    code,
                    message,
                })
                .unwrap_or(LlmErrorKind::Unknown)
            }
            None => LlmErrorKind::Unknown,
        }
    }

    /// 智谱欠费是 429 + 业务码 1113 的中文提示。旧实现只看见 "429" 会判成限流，
    /// 而限流属可重试类——等于在余额耗尽时反复重试，务必守住。
    #[test]
    fn deepseek_402_is_insufficient_balance() {
        let e = r#"Responses API 请求失败 (402 Payment Required): {"error":{"message":"Insufficient Balance","type":"unknown_error","code":"invalid_request_error"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    #[test]
    fn zhipu_arrears_is_balance_not_rate_limit() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"1113","message":"您的账户已欠费，请充值后重试"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    /// 上面那条不能矫枉过正：智谱真正的限流（业务码 1302）仍要识别为限流。
    #[test]
    fn zhipu_real_rate_limit_stays_rate_limited() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"1302","message":"您的账户已达到速率限制"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::RateLimited);
    }

    #[test]
    fn volcengine_overdue_is_balance() {
        let e = r#"Responses API 请求失败 (403 Forbidden): {"error":{"code":"AccountOverdueError","message":"your account has an overdue balance"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    /// 同为 403：火山方舟的 AccessDenied 是权限问题，不能与欠费混为一谈。
    #[test]
    fn volcengine_access_denied_is_permission() {
        let e = r#"Responses API 请求失败 (403 Forbidden): {"error":{"code":"AccessDenied","message":"do not have access to the requested resource"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::PermissionDenied);
    }

    #[test]
    fn dashscope_arrearage_400_is_balance() {
        let e = r#"Responses API 请求失败 (400 Bad Request): {"error":{"code":"Arrearage","message":"please make sure your account is in good standing"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    #[test]
    fn dashscope_bill_overdue_429_is_balance() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"PrepaidBillOverdue","message":"your prepaid bill is overdue"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    #[test]
    fn openai_insufficient_quota_is_quota_not_rate_limit() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::QuotaExceeded);
    }

    #[test]
    fn openai_plain_rate_limit_stays_rate_limited() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"type":"rate_limit_exceeded","message":"Rate limit reached for requests"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::RateLimited);
    }

    #[test]
    fn anthropic_credit_too_low_400_is_balance() {
        let e = r#"Responses API 请求失败 (400 Bad Request): {"error":{"message":"Your credit balance is too low to access the API"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    #[test]
    fn anthropic_enforced_spend_limit_is_quota() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"enforced_spend_limit_reached","message":"spend limit reached"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::QuotaExceeded);
    }

    /// 腾讯返回整型业务码且键名是 `Code`（非小写 code），必须接住。
    #[test]
    fn hunyuan_integer_code_and_mixed_case_key() {
        let e = r#"Responses API 请求失败 (402 Payment Required): {"error":{"Code":403004,"Message":"余额不足"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
        // 401007/401008 是免费额度耗尽，属配额而非余额
        let e2 = r#"Responses API 请求失败 (402 Payment Required): {"error":{"Code":401007,"Message":"free quota exhausted"}}"#;
        assert_eq!(kind_of(e2), LlmErrorKind::QuotaExceeded);
    }

    #[test]
    fn minimax_underscore_code_and_base_resp() {
        let e = r#"Responses API 请求失败 (402 Payment Required): {"base_resp":{"status_code":1008}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    #[test]
    fn baidu_wenxin_overdue_403_is_balance() {
        let e = r#"API 请求失败 (403 Forbidden): {"error":{"code":"account_overdue","message":"Access denied due to overdue account"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::InsufficientBalance);
    }

    /// 同为 403：OpenRouter 用它表示内容审核命中，按「权限不足」提示会误导。
    #[test]
    fn openrouter_403_moderation_is_content_policy() {
        let e = r#"Responses API 请求失败 (403 Forbidden): {"error":{"message":"moderation flagged this request"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::ContentPolicy);
    }

    /// 同为 402：Together 用它表示月度消费上限，属配额而非余额。
    #[test]
    fn together_402_is_monthly_spend_cap() {
        let e = r#"Responses API 请求失败 (402 Payment Required): {"error":{"message":"You have reached your monthly spending limit"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::QuotaExceeded);
    }

    #[test]
    fn together_403_context_length_is_not_permission() {
        let e = r#"Responses API 请求失败 (403 Forbidden): {"error":{"message":"input + max_tokens exceeds the model context length"}}"#;
        assert_eq!(kind_of(e), LlmErrorKind::ContextLengthExceeded);
    }

    #[test]
    fn auth_and_server_paths() {
        assert_eq!(
            kind_of(r#"请求失败 (401 Unauthorized): {"error":{"message":"Incorrect API key provided"}}"#),
            LlmErrorKind::InvalidApiKey
        );
        assert_eq!(
            kind_of(r#"请求失败 (500 Internal Server Error): {"error":{"message":"boom"}}"#),
            LlmErrorKind::ServerError
        );
        assert_eq!(
            kind_of(r#"请求失败 (503 Service Unavailable): {"error":{"message":"overloaded"}}"#),
            LlmErrorKind::Overloaded
        );
    }

    #[test]
    fn circuit_breaker_is_local_not_http() {
        assert_eq!(
            kind_of("熔断器已打开: 熔断器已打开: provider:deepseek-flash"),
            LlmErrorKind::CircuitBreakerOpen
        );
    }

    /// 状态码只认紧跟左括号的位置：request_id 里的数字不得污染分类。
    #[test]
    fn status_code_does_not_leak_from_request_id() {
        let e = "provider call failed, quote request id 401299 in your ticket";
        assert_eq!(kind_of(e), LlmErrorKind::Unknown);
    }

    #[test]
    fn ip_port_is_not_status_code() {
        assert_eq!(
            kind_of("connect error to upstream (127.0.0.1:8080): timeout"),
            LlmErrorKind::Unknown
        );
    }

    // ------------------------------------------------ 重试判定与提示必须同源

    /// 提示说「余额不足」、后台却在猛重试，是这次要防的核心不一致。
    #[test]
    fn arrears_is_permanent_so_retry_stops() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"1113","message":"您的账户已欠费，请充值后重试"}}"#;
        assert_eq!(classify_error_from_str(e), ErrorCategory::Permanent);
    }

    /// 真限流仍要可重试，别把两种情况一起判死。
    #[test]
    fn real_rate_limit_stays_retryable() {
        let e = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"code":"1302","message":"您的账户已达到速率限制"}}"#;
        assert_eq!(classify_error_from_str(e), ErrorCategory::RateLimit);

        let e2 = r#"Responses API 请求失败 (429 Too Many Requests): {"error":{"type":"rate_limit_exceeded","message":"Rate limit reached"}}"#;
        assert_eq!(classify_error_from_str(e2), ErrorCategory::RateLimit);
    }

    #[test]
    fn permanent_and_transient_split() {
        // 401 invalid_api_key：密钥错了，重试一万次也没用 —— 必须判 Permanent
        assert_eq!(
            classify_error_from_str(
                r#"请求失败 (401 Unauthorized): {"error":{"message":"invalid_api_key"}}"#
            ),
            ErrorCategory::Permanent
        );
        assert_eq!(
            classify_error_from_str(r#"请求失败 (500 Internal Server Error): {"error":{"message":"boom"}}"#),
            ErrorCategory::Transient
        );
        assert_eq!(
            classify_error_from_str(r#"请求失败 (503 Service Unavailable): {"error":{"message":"overloaded"}}"#),
            ErrorCategory::Transient
        );
    }

    /// 纯响应体（无状态码前缀）也不能误判：这是 HTTP 层拿 body 判定的入口形态。
    #[test]
    fn body_only_judgement() {
        assert_eq!(
            classify_error_from_str(r#"{"error":{"code":"Arrearage","message":"account is in good standing"}}"#),
            ErrorCategory::Permanent
        );
    }

    // ------------------------------------------------ 半开只放行一个探测

    fn breaker() -> CircuitBreaker {
        // 阈值 2、失败率 1.0、冷却 30s
        CircuitBreaker::new("test", 2, 1.0, Duration::from_secs(30))
    }

    /// 打满阈值进入熔断。
    fn trip(b: &mut CircuitBreaker) {
        b.record_failure();
        b.record_failure();
        assert_eq!(b.state, CircuitState::Open);
    }

    #[test]
    fn half_open_admits_single_probe_only() {
        let mut b = breaker();
        trip(&mut b);

        // 冷却期内一律拒绝
        assert!(!b.allow_request());
        assert!(!b.allow_request());

        // 把冷却时间拨过去，触发半开
        b.last_failure_time = Some(Instant::now() - Duration::from_secs(31));
        assert!(b.allow_request(), "首个请求应被放行作为探测");
        assert_eq!(b.state, CircuitState::HalfOpen);

        // 关键：探测在飞行中时，其余并发请求必须被拒，
        // 否则充值瞬间积压请求一起涌入，任一失败就立刻打回熔断。
        assert!(!b.allow_request(), "探测在飞行中时不得放行第二个请求");
        assert!(!b.allow_request());
    }

    #[test]
    fn half_open_probe_success_closes_circuit() {
        let mut b = breaker();
        trip(&mut b);
        b.last_failure_time = Some(Instant::now() - Duration::from_secs(31));
        assert!(b.allow_request());

        b.record_success();
        assert_eq!(b.state, CircuitState::Closed);
        assert!(b.allow_request(), "恢复正常后应全部放行");
    }

    #[test]
    fn half_open_probe_failure_reopens_circuit() {
        let mut b = breaker();
        trip(&mut b);
        b.last_failure_time = Some(Instant::now() - Duration::from_secs(31));
        assert!(b.allow_request());

        b.record_failure();
        assert_eq!(b.state, CircuitState::Open);
        assert!(!b.allow_request(), "探测失败后应重新进入冷却");
    }

    /// 探测请求可能没回报结果（响应解析失败会提前 `?` 返回，大 prompt 又按设计
    /// 跳过熔断记账）。超时后必须能重新放行，否则熔断器永久卡在半开。
    #[test]
    fn stuck_probe_does_not_deadlock() {
        let mut b = breaker();
        trip(&mut b);
        b.last_failure_time = Some(Instant::now() - Duration::from_secs(31));
        assert!(b.allow_request());

        // 探测没有回报任何结果时，仍然被挡住
        assert!(!b.allow_request());

        // 把探测时刻拨到超时之前
        b.probe_started_at = Some(Instant::now() - (PROBE_TIMEOUT + Duration::from_secs(1)));
        assert!(b.allow_request(), "探测超时后应重新放行，避免永久卡死");
    }

    #[test]
    fn reset_clears_probe_slot() {
        let mut b = breaker();
        trip(&mut b);
        b.last_failure_time = Some(Instant::now() - Duration::from_secs(31));
        assert!(b.allow_request());

        b.reset();
        assert_eq!(b.state, CircuitState::Closed);
        assert!(b.allow_request());
    }
}
