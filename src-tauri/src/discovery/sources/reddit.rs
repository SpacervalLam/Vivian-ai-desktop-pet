//! Reddit 源 — rdt-cli 登录态路径优先，匿名 .json 端点回退
//!
//! 登录态路径：扩展回传 reddit.com 整罐 Cookie（bridge.reportRedditCookie，
//! 需含 reddit_session），服务端同步进 rdt-cli 凭据文件后调用外部 `rdt`
//! CLI（`uv tool install rdt-cli`）执行只读发现：
//! - `rdt search <query> -n N --json`：搜索
//! - `rdt popular -n N --json`：热门
//!
//! 匿名回退（无登录/CLI 不可用时）走公开 .json 端点（限频较严）：
//! - `GET reddit.com/search.json?q={kw}&sort=relevance&limit={n}`
//! - `GET reddit.com/r/popular/hot.json?limit={n}`
//!
//! 两路输出同为 Reddit listing 行形状（id/title/selftext/subreddit/author/
//! permalink/score/num_comments/created_utc），共用一套候选转换。
//! 礼仪：可识别 UA + 每轮发现最多各调一次；429/403 静默返回空。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;

use crate::network::http_client::get_global_client;

use super::{ContentCandidate, SourceAdapter};

const BASE: &str = "https://www.reddit.com";
const UA: &str = "vivian-desktop-pet/1.0 (vivian discovery)";
const TIMEOUT_SECS: u64 = 15;
/// CLI 单次调用预算
const CLI_TIMEOUT_SECS: u64 = 75;
/// CLI 可用性缓存的重新探测间隔（秒）
const CLI_PROBE_TTL_SECS: i64 = 3600;
/// rdt-cli 登录态所需 Cookie（缺则不同步凭据）
const RDT_REQUIRED_COOKIE: &str = "reddit_session";

/// `rdt` CLI 可用性缓存：(是否可用, 检查时间戳)。NotFound 时短路后续调用。
static CLI_AVAILABLE: Lazy<RwLock<Option<(bool, i64)>>> = Lazy::new(|| RwLock::new(None));

/// rdt-cli 凭据文件路径（rdt_cli.constants.CREDENTIAL_FILE 的通用回退位置）
fn rdt_credential_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".config")
            .join("rdt-cli")
            .join("credential.json"),
    )
}

/// 解析 Cookie 头为 name→value 表
fn parse_cookie_header(cookie: &str) -> std::collections::BTreeMap<String, String> {
    let mut pairs = std::collections::BTreeMap::new();
    for chunk in cookie.split(';') {
        let Some((name, value)) = chunk.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if !name.is_empty() && !value.is_empty() {
            pairs.insert(name.to_string(), value.to_string());
        }
    }
    pairs
}

/// 把扩展回传的 Reddit Cookie 同步进 rdt-cli 凭据文件（仅当比现有凭据新）。
/// 无桥/无 Cookie/缺 reddit_session 时静默跳过（用户可自行 `rdt login`）。
fn sync_rdt_credential_from_bridge() {
    let Some(bridge) = crate::browser_bridge::tools::global_bridge() else {
        return;
    };
    let Some((epoch_ms, header)) = bridge.reddit_cookie() else {
        return;
    };
    let pairs = parse_cookie_header(&header);
    if !pairs.contains_key(RDT_REQUIRED_COOKIE) {
        return;
    }
    let Some(path) = rdt_credential_path() else {
        return;
    };

    // 已有更新（或同代）凭据则不重写，避免每次发现都落盘
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&existing) {
            if let Some(saved_at) = v.get("saved_at").and_then(|x| x.as_f64()) {
                if (saved_at * 1000.0) as u64 >= epoch_ms {
                    return;
                }
            }
        }
    }

    let modhash = pairs
        .get("modhash")
        .or_else(|| pairs.get("csrf_token"))
        .cloned();
    let payload = serde_json::json!({
        "cookies": pairs,
        "source": "vivian:extension",
        "username": null,
        "modhash": modhash,
        "saved_at": epoch_ms as f64 / 1000.0,
        "last_verified_at": null,
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&payload).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
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

/// 组装带隐藏窗口的 rdt CLI 命令
fn build_cli_command(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("rdt");
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        // 隐藏控制台窗口，避免后台发现时闪窗
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// 执行 rdt CLI 并提取 item 字典列表；不可用/失败/超时返回 None（触发匿名回退）
async fn run_rdt_cli(args: &[&str]) -> Option<Vec<Value>> {
    if cli_available_cached() == Some(false) {
        return None;
    }
    // 顺带把扩展回传的 Cookie 同步进凭据文件（无则依赖用户既有凭据）
    sync_rdt_credential_from_bridge();

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(CLI_TIMEOUT_SECS),
        build_cli_command(args).output(),
    )
    .await
    .ok()?
    .ok()?;

    if output.status.success() {
        set_cli_available(true);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: Value = serde_json::from_str(stdout.trim()).ok()?;
        let mut items = Vec::new();
        extract_item_dicts(&parsed, &mut items);
        return Some(items);
    }

    // 二进制缺失（PATH 上没有 rdt）→ 记缓存短路；其余失败仅记录
    if output.status.code().is_none() {
        set_cli_available(false);
        tracing::debug!("[discovery:reddit] rdt CLI 未安装，走匿名回退");
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::debug!(
        "[discovery:reddit] rdt CLI 退出码 {:?}: {}",
        output.status.code(),
        stderr.trim().chars().take(200).collect::<String>()
    );
    // 退出码存在但非零：CLI 本身可用（多为凭据过期/限频）
    set_cli_available(true);
    None
}

/// 递归提取 item 字典：数组展开 / `{children:[{data}]}` listing / items|results|posts|comments|data 信封 / 裸行
fn extract_item_dicts(parsed: &Value, out: &mut Vec<Value>) {
    match parsed {
        Value::Array(arr) => {
            for item in arr {
                extract_item_dicts(item, out);
            }
        }
        Value::Object(obj) => {
            if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
                for child in children {
                    if let Some(data) = child.get("data").filter(|d| d.is_object()) {
                        out.push(data.clone());
                    }
                }
                return;
            }
            for key in ["items", "results", "posts", "comments", "data"] {
                if let Some(v) = obj.get(key) {
                    let before = out.len();
                    extract_item_dicts(v, out);
                    if out.len() > before {
                        return;
                    }
                }
            }
            if obj.keys().any(|k| {
                matches!(k.as_str(), "id" | "title" | "permalink" | "url" | "selftext" | "body")
            }) {
                out.push(parsed.clone());
            }
        }
        _ => {}
    }
}

/// Reddit listing 行（rdt 输出与匿名 .json 同形状）→ 统一候选
fn item_to_candidate(d: &Value, source: &str) -> Option<ContentCandidate> {
    let id_raw = d
        .get("id")
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().map(|n| n.to_string())))?;
    // rdt 输出的 id 可能带 t3_/t1_ fullname 前缀，统一剥掉做内容键
    let id = id_raw
        .strip_prefix("t3_")
        .or_else(|| id_raw.strip_prefix("t1_"))
        .unwrap_or(&id_raw)
        .to_string();
    let title = d
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let body = d
        .get("selftext")
        .or_else(|| d.get("body"))
        .or_else(|| d.get("text"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim();
    if id.is_empty() || (title.is_empty() && body.is_empty()) {
        return None;
    }
    let permalink = d
        .get("permalink")
        .or_else(|| d.get("url"))
        .or_else(|| d.get("link"))
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .trim();
    let url = if permalink.starts_with('/') {
        format!("{}{}", BASE, permalink)
    } else if permalink.starts_with("http") {
        permalink.to_string()
    } else {
        format!("{}/comments/{}/", BASE, id)
    };
    let subreddit = d
        .get("subreddit")
        .or_else(|| d.get("subreddit_name_prefixed"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim_start_matches("r/")
        .to_string();
    let author = d
        .get("author")
        .or_else(|| d.get("username"))
        .or_else(|| d.get("user"))
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    Some(ContentCandidate {
        platform: "reddit".to_string(),
        content_id: id,
        title: if title.is_empty() {
            body.chars().take(80).collect()
        } else {
            title
        },
        description: body.chars().take(150).collect(),
        author: if !subreddit.is_empty() {
            format!("r/{} · u/{}", subreddit, author)
        } else {
            author
        },
        url,
        cover_url: d
            .get("thumbnail")
            .and_then(|t| t.as_str())
            .filter(|t| t.starts_with("http"))
            .unwrap_or("")
            .to_string(),
        duration_secs: 0,
        view_count: d
            .get("score")
            .or_else(|| d.get("ups"))
            .or_else(|| d.get("upvotes"))
            .and_then(|s| s.as_i64())
            .unwrap_or(0)
            .max(0) as u64,
        like_count: d
            .get("ups")
            .or_else(|| d.get("score"))
            .and_then(|u| u.as_i64())
            .unwrap_or(0)
            .max(0) as u64,
        pubdate: d.get("created_utc").and_then(|c| c.as_i64()).unwrap_or(0),
        source: source.to_string(),
    })
}

/// Reddit 适配器（rdt-cli 优先，匿名 .json 回退）
pub struct RedditSource;

#[async_trait]
impl SourceAdapter for RedditSource {
    fn platform(&self) -> &'static str {
        "reddit"
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<ContentCandidate> {
        let limit_s = limit.min(30).to_string();
        let args = ["search", query, "-n", &limit_s, "--json"];
        if let Some(items) = run_rdt_cli(&args).await {
            return items
                .iter()
                .take(limit)
                .filter_map(|d| item_to_candidate(d, &format!("rdt-search:{}", query)))
                .collect();
        }
        let url = format!(
            "{}/search.json?q={}&sort=relevance&limit={}",
            BASE,
            urlencoding::encode(query),
            limit.min(50)
        );
        self.fetch_listing(&url).await
    }

    async fn popular(&self, limit: usize) -> Vec<ContentCandidate> {
        let limit_s = limit.min(30).to_string();
        let args = ["popular", "-n", &limit_s, "--json"];
        if let Some(items) = run_rdt_cli(&args).await {
            return items
                .iter()
                .take(limit)
                .filter_map(|d| item_to_candidate(d, "rdt-popular"))
                .collect();
        }
        let url = format!("{}/r/popular/hot.json?limit={}", BASE, limit.min(50));
        self.fetch_listing(&url).await
    }
}

impl RedditSource {
    async fn fetch_listing(&self, url: &str) -> Vec<ContentCandidate> {
        let client = get_global_client();
        let resp = client
            .get(url)
            .header("User-Agent", UA)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::debug!("[discovery:reddit] 请求失败");
            return Vec::new();
        };
        let status = resp.status().as_u16();
        if status == 429 || status == 403 {
            tracing::debug!("[discovery:reddit] 限频/拒绝（{}），本轮跳过", status);
            return Vec::new();
        }
        let Ok(payload) = resp.json::<Value>().await else {
            tracing::debug!("[discovery:reddit] 响应解析失败");
            return Vec::new();
        };
        let mut items = Vec::new();
        extract_item_dicts(&payload, &mut items);
        items
            .iter()
            .filter_map(|d| item_to_candidate(d, "listing"))
            .collect()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cookie_header() {
        let pairs = parse_cookie_header("reddit_session=abc; csv=1; empty=");
        assert_eq!(pairs.get("reddit_session").map(|s| s.as_str()), Some("abc"));
        assert_eq!(pairs.get("csv").map(|s| s.as_str()), Some("1"));
        assert!(!pairs.contains_key("empty"));
    }

    #[test]
    fn test_extract_item_dicts_listing_shape() {
        let payload = serde_json::json!({
            "data": { "children": [ { "data": { "id": "a1", "title": "T" } } ] }
        });
        let mut items = Vec::new();
        extract_item_dicts(&payload, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].get("id").and_then(|v| v.as_str()), Some("a1"));
    }

    #[test]
    fn test_extract_item_dicts_rdt_envelope() {
        let payload = serde_json::json!({ "ok": true, "items": [ { "id": "b2", "title": "X" } ] });
        let mut items = Vec::new();
        extract_item_dicts(&payload, &mut items);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_item_to_candidate() {
        let d = serde_json::json!({
            "id": "t3_abc", "title": "Hello", "selftext": "Body",
            "subreddit": "rust", "author": "u1", "permalink": "/r/rust/comments/abc/",
            "score": 42, "created_utc": 1_700_000_000
        });
        let c = item_to_candidate(&d, "listing").unwrap();
        assert_eq!(c.platform, "reddit");
        assert_eq!(c.content_id, "abc");
        assert_eq!(c.title, "Hello");
        assert_eq!(c.url, "https://www.reddit.com/r/rust/comments/abc/");
        assert_eq!(c.view_count, 42);
    }

    #[test]
    fn test_item_to_candidate_empty_title_uses_body() {
        let d = serde_json::json!({ "id": "z9", "body": "only body text" });
        let c = item_to_candidate(&d, "listing").unwrap();
        assert_eq!(c.title, "only body text");
    }

    #[test]
    fn test_item_to_candidate_rejects_empty() {
        let d = serde_json::json!({ "id": "x" });
        assert!(item_to_candidate(&d, "listing").is_none());
    }
}