//! X (Twitter) 源 — twitter-cli cookie 重放（服务端登录态发现）
//!
//! 模型：扩展回传 x.com 的 auth_token + ct0（bridge.reportXCookie），
//! 服务端把它们注入 `TWITTER_AUTH_TOKEN`/`TWITTER_CT0` 环境变量，调用外部
//! `twitter` CLI（`uv tool install twitter-cli`）执行只读发现：
//! - `twitter search <query> --max N --json`：搜索
//! - `twitter feed --max N --json`：For You 时间线（热门兜底）
//!
//! 输出为统一信封 `{ok, data}`（SCHEMA.md 契约），tweet 字典字段：
//! id / text / author{name, screenName} / metrics{likes, retweets, views, ...} /
//! createdAtISO / media。
//!
//! 环境变量 `VIVIAN_X_COOKIE`（完整 Cookie 头）优先于扩展回传，供调试覆盖。
//! cookie 缺失或 CLI 未安装时静默返回空（源禁用，不阻断其它源）。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;

use super::{ContentCandidate, SourceAdapter};

/// CLI 单次调用预算（twitter-cli 首次运行可能较慢）
const CLI_TIMEOUT_SECS: u64 = 75;
/// CLI 可用性缓存的重新探测间隔（秒）
const CLI_PROBE_TTL_SECS: i64 = 3600;

/// `twitter` CLI 可用性缓存：(是否可用, 检查时间戳)。NotFound 时短路后续调用。
static CLI_AVAILABLE: Lazy<RwLock<Option<(bool, i64)>>> = Lazy::new(|| RwLock::new(None));

/// 解析 Cookie 头中的 auth_token 与 ct0（服务端重放两枚都必需，缺一 401）
fn parse_auth_pair(cookie: &str) -> Option<(String, String)> {
    let mut auth_token = String::new();
    let mut ct0 = String::new();
    for chunk in cookie.split(';') {
        let Some((name, value)) = chunk.split_once('=') else {
            continue;
        };
        match name.trim() {
            "auth_token" => auth_token = value.trim().to_string(),
            "ct0" => ct0 = value.trim().to_string(),
            _ => {}
        }
    }
    if auth_token.is_empty() || ct0.is_empty() {
        None
    } else {
        Some((auth_token, ct0))
    }
}

/// 当前可用的 X 凭据（环境变量优先，其次扩展回传）
fn resolve_auth() -> Option<(String, String)> {
    if let Ok(env_cookie) = std::env::var("VIVIAN_X_COOKIE") {
        if let Some(pair) = parse_auth_pair(env_cookie.trim()) {
            return Some(pair);
        }
    }
    let bridge = crate::browser_bridge::tools::global_bridge()?;
    let cookie = bridge.x_cookie()?;
    parse_auth_pair(&cookie)
}

fn cli_available_cached() -> Option<bool> {
    let cached = *CLI_AVAILABLE.read();
    let now = chrono::Utc::now().timestamp();
    match cached {
        Some((ok, ts)) if now - ts < CLI_PROBE_TTL_SECS => Some(ok),
        _ => None,
    }
}

fn set_cli_available(ok: bool) {
    *CLI_AVAILABLE.write() = Some((ok, chrono::Utc::now().timestamp()));
}

/// 组装带凭据与隐藏窗口的 CLI 命令
fn build_cli_command(args: &[&str], auth_token: &str, ct0: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("twitter");
    cmd.args(args)
        .env("TWITTER_AUTH_TOKEN", auth_token)
        .env("TWITTER_CT0", ct0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        // 隐藏控制台窗口，避免后台发现时闪窗
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// 执行 twitter CLI 并解析信封；不可用/失败/超时返回 None
async fn run_twitter_cli(args: &[&str]) -> Option<Value> {
    if cli_available_cached() == Some(false) {
        return None;
    }
    let (auth_token, ct0) = resolve_auth()?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(CLI_TIMEOUT_SECS),
        build_cli_command(args, &auth_token, &ct0).output(),
    )
    .await
    .ok()?
    .ok()?;

    if output.status.success() {
        set_cli_available(true);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return serde_json::from_str(stdout.trim()).ok();
    }

    // 二进制缺失（PATH 上没有 twitter）→ 记缓存短路；其余失败仅记录
    if output.status.code().is_none() {
        set_cli_available(false);
        tracing::debug!("[discovery:x] twitter CLI 未安装，X 源禁用");
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::debug!(
        "[discovery:x] twitter CLI 退出码 {:?}: {}",
        output.status.code(),
        stderr.trim().chars().take(200).collect::<String>()
    );
    // 退出码存在但非零：CLI 本身可用（多为 cookie 过期/限频）
    set_cli_available(true);
    None
}

/// tweet 字典 → 统一候选
fn tweet_to_candidate(t: &Value, source: &str) -> Option<ContentCandidate> {
    let id = t
        .get("id")
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().map(|n| n.to_string())))?;
    let text = t.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
    if id.is_empty() || text.is_empty() {
        return None;
    }
    let author = t.get("author").cloned().unwrap_or(Value::Null);
    let screen_name = author
        .get("screenName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let author_name = author
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let metrics = t.get("metrics").cloned().unwrap_or(Value::Null);
    let cover = t
        .get("media")
        .and_then(|m| m.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| {
            m.get("url")
                .or_else(|| m.get("media_url"))
                .and_then(|u| u.as_str())
        })
        .unwrap_or("")
        .to_string();
    let pubdate = t
        .get("createdAtISO")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp())
        .or_else(|| {
            t.get("createdAt")
                .and_then(|v| v.as_i64())
                .map(|n| if n > 1_000_000_000_000 { n / 1000 } else { n })
        })
        .unwrap_or(0);
    Some(ContentCandidate {
        platform: "twitter".to_string(),
        content_id: id.clone(),
        title: text.lines().next().unwrap_or("").chars().take(100).collect(),
        description: text.chars().take(150).collect(),
        author: if author_name.is_empty() {
            format!("@{}", screen_name)
        } else {
            format!("{} (@{})", author_name, screen_name)
        },
        url: if screen_name.is_empty() {
            format!("https://x.com/i/web/status/{}", id)
        } else {
            format!("https://x.com/{}/status/{}", screen_name, id)
        },
        cover_url: cover,
        duration_secs: 0,
        view_count: metrics.get("views").and_then(|v| v.as_u64()).unwrap_or(0),
        like_count: metrics.get("likes").and_then(|v| v.as_u64()).unwrap_or(0),
        pubdate,
        source: source.to_string(),
    })
}

/// 信封 `{ok, data}` → tweet 列表 → 候选
fn envelope_to_candidates(payload: &Value, source: &str, limit: usize) -> Vec<ContentCandidate> {
    // 信封失败（cookie 过期/限频）或 data 非数组时为空
    if payload.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return Vec::new();
    }
    let data = payload
        .get("data")
        .unwrap_or(payload)
        .as_array()
        .cloned()
        .unwrap_or_default();
    data.iter()
        .take(limit)
        .filter_map(|t| tweet_to_candidate(t, source))
        .collect()
}

/// X (Twitter) 适配器（twitter-cli cookie 重放）
pub struct XSource;

#[async_trait]
impl SourceAdapter for XSource {
    fn platform(&self) -> &'static str {
        "twitter"
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<ContentCandidate> {
        let limit_s = limit.min(30).to_string();
        let args = ["search", query, "--max", &limit_s, "--json"];
        match run_twitter_cli(&args).await {
            Some(payload) => envelope_to_candidates(&payload, &format!("search:{}", query), limit),
            None => Vec::new(),
        }
    }

    async fn popular(&self, limit: usize) -> Vec<ContentCandidate> {
        let limit_s = limit.min(30).to_string();
        let args = ["feed", "--max", &limit_s, "--json"];
        match run_twitter_cli(&args).await {
            Some(payload) => envelope_to_candidates(&payload, "feed", limit),
            None => Vec::new(),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auth_pair() {
        assert_eq!(
            parse_auth_pair("auth_token=abc; ct0=def; other=1"),
            Some(("abc".to_string(), "def".to_string()))
        );
        assert_eq!(parse_auth_pair("auth_token=abc"), None);
        assert_eq!(parse_auth_pair(""), None);
    }

    #[test]
    fn test_envelope_to_candidates() {
        let payload = serde_json::json!({
            "ok": true,
            "data": [
                {
                    "id": "123",
                    "text": "hello world\nsecond line",
                    "author": { "name": "Foo", "screenName": "foo" },
                    "metrics": { "likes": 10, "views": 100 },
                    "createdAtISO": "2026-08-01T00:00:00Z"
                }
            ]
        });
        let out = envelope_to_candidates(&payload, "feed", 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].platform, "twitter");
        assert_eq!(out[0].title, "hello world");
        assert_eq!(out[0].url, "https://x.com/foo/status/123");
        assert_eq!(out[0].view_count, 100);
    }

    #[test]
    fn test_envelope_error_is_empty() {
        let payload = serde_json::json!({ "ok": false, "error": { "code": "not_authenticated" } });
        assert!(envelope_to_candidates(&payload, "feed", 10).is_empty());
    }
}