//! 多平台内容详情 — 抓「正文 + 高赞评论」作为评估证据
//!
//! 参考 bb-browser 的思路：**不写爬虫、不要密钥，用本机已登录的浏览器读站点**。
//! 与 [`bilibili_auth`](super::bilibili_auth) 的分工：
//! - B站字幕走 WBI + 登录态 cookie（已有实现，这里只做映射）；
//! - V2EX / Reddit 走公开 JSON API（无需登录）；
//! - 知乎 / 小红书 / 抖音 走内嵌浏览器的**固定详情提取器**（打包脚本，不支持任意 JS）。
//!
//! 纪律：抓不到是数据不是失败 —— 任何失败都返回状态，调用方退回「只看标题/简介」，
//! 绝不因单条详情失败中断整轮。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::Semaphore;

use super::bilibili_auth::{self, CandidateContext, SubtitleStatus};

/// 每轮最多为多少个非 B 站候选抓详情（成本与风控上限）
pub const DETAIL_FETCH_CAP: usize = 6;

const BODY_MAX_CHARS: usize = 4000;
const COMMENTS_PER_ITEM: usize = 5;
const COMMENT_MAX_CHARS: usize = 120;
/// API 平台的整体超时
const API_TIMEOUT_SECS: u64 = 10;
/// 浏览器后台标签的整体超时（页面加载 + SPA 渲染 + 提取）
const DOM_TIMEOUT_SECS: u64 = 25;
const CACHE_TTL_SECS: i64 = 24 * 3600;
const FAILURE_TTL_SECS: i64 = 60;
const CACHE_CAP: usize = 256;

const V2EX_BASE: &str = "https://www.v2ex.com";
const REDDIT_BASE: &str = "https://www.reddit.com";
/// 短链域名：用户从 App 复制出来的分享链接，先跟随重定向还原成标准内容页
const SHORT_LINK_HOSTS: [&str; 3] = ["b23.tv", "v.douyin.com", "xhslink.com"];
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 抓取方式（对照 bb-browser 的 adapter tier）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FetchKind {
    /// 公开 JSON API，无需登录
    PublicApi,
    /// 需要登录态的 JSON API（B站）
    LoginApi,
    /// 内嵌浏览器后台标签 + 固定提取器
    BrowserDom,
}

/// 一个平台适配器的元数据（新增平台只需在 [`ADAPTERS`] 里加一行）
#[derive(Debug, Clone, Copy)]
pub struct PlatformAdapter {
    pub platform: &'static str,
    /// 用于 URL host 匹配的域名（精确或子域名，可多个，如 youtube.com / youtu.be）
    pub domains: &'static [&'static str],
    pub kind: FetchKind,
    /// 是否要求浏览器已登录该平台
    pub needs_login: bool,
    pub supports_comments: bool,
    /// 给模型/用户看的示例链接
    pub example: &'static str,
    pub description: &'static str,
}

pub const ADAPTERS: &[PlatformAdapter] = &[
    PlatformAdapter {
        platform: "bilibili",
        domains: &["bilibili.com"],
        kind: FetchKind::LoginApi,
        needs_login: true,
        supports_comments: true,
        example: "https://www.bilibili.com/video/BV1xxxxxxxxx",
        description: "视频字幕原文 + 热门评论",
    },
    PlatformAdapter {
        platform: "zhihu",
        domains: &["zhihu.com"],
        kind: FetchKind::BrowserDom,
        needs_login: false,
        supports_comments: true,
        example: "https://www.zhihu.com/question/12345678",
        description: "问答/文章正文 + 可见评论",
    },
    PlatformAdapter {
        platform: "xiaohongshu",
        domains: &["xiaohongshu.com"],
        kind: FetchKind::BrowserDom,
        needs_login: true,
        supports_comments: true,
        example: "https://www.xiaohongshu.com/explore/65a1b2c3d4e5f6a7b8c9d0e1",
        description: "笔记正文 + 可见评论",
    },
    PlatformAdapter {
        platform: "douyin",
        domains: &["douyin.com"],
        kind: FetchKind::BrowserDom,
        needs_login: true,
        supports_comments: true,
        example: "https://www.douyin.com/video/7300000000000000000",
        description: "视频简介 + 可见评论",
    },
    PlatformAdapter {
        platform: "v2ex",
        domains: &["v2ex.com"],
        kind: FetchKind::PublicApi,
        needs_login: false,
        supports_comments: true,
        example: "https://www.v2ex.com/t/999888",
        description: "主题正文全文 + 回复",
    },
    PlatformAdapter {
        platform: "reddit",
        domains: &["reddit.com"],
        kind: FetchKind::PublicApi,
        needs_login: false,
        supports_comments: true,
        example: "https://www.reddit.com/r/rust/comments/1abc2d/",
        description: "帖子正文 + 高赞评论",
    },
    PlatformAdapter {
        platform: "twitter",
        domains: &["x.com", "twitter.com"],
        kind: FetchKind::BrowserDom,
        needs_login: true,
        supports_comments: true,
        example: "https://x.com/user/status/1234567890",
        description: "推文正文 + 可见回复",
    },
    PlatformAdapter {
        platform: "youtube",
        domains: &["youtube.com", "youtu.be"],
        kind: FetchKind::PublicApi,
        needs_login: false,
        supports_comments: false,
        example: "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        description: "视频字幕/转写原文",
    },
    PlatformAdapter {
        platform: "github",
        domains: &["github.com"],
        kind: FetchKind::PublicApi,
        needs_login: false,
        supports_comments: true,
        example: "https://github.com/rust-lang/rust",
        description: "仓库简介+README；Issue/PR 正文+评论",
    },
    PlatformAdapter {
        platform: "arxiv",
        domains: &["arxiv.org"],
        kind: FetchKind::PublicApi,
        needs_login: false,
        supports_comments: false,
        example: "https://arxiv.org/abs/2401.12345",
        description: "论文标题、作者与摘要",
    },
];

pub fn adapter(platform: &str) -> Option<&'static PlatformAdapter> {
    ADAPTERS.iter().find(|a| a.platform == platform)
}

/// 按 host 匹配平台（精确或子域名，规则同 bb-browser 的 matchTabOrigin）
pub fn adapter_for_host(host: &str) -> Option<&'static PlatformAdapter> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    ADAPTERS.iter().find(|a| {
        a.domains
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{}", d)))
    })
}

/// 给模型/用户看的能力清单（用于不支持链接的提示）
pub fn supported_summary() -> String {
    ADAPTERS
        .iter()
        .map(|a| format!("{}（{}）", a.platform, a.description))
        .collect::<Vec<_>>()
        .join("；")
}

/// 详情抓取状态（与 `SubtitleStatus` 同构，便于统一证据口径）
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailStatus {
    #[default]
    NotFetched,
    LoginRequired,
    Unavailable,
    Missing,
    Full,
    Excerpt,
}

impl DetailStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DetailStatus::NotFetched => "not_fetched",
            DetailStatus::LoginRequired => "login_required",
            DetailStatus::Unavailable => "unavailable",
            DetailStatus::Missing => "missing",
            DetailStatus::Full => "full",
            DetailStatus::Excerpt => "excerpt",
        }
    }
}

/// 一个候选的内容证据
#[derive(Debug, Clone, Default)]
pub struct ContentDetail {
    /// 正文（B站即字幕原文）
    pub body: Option<String>,
    /// 正文原始字数（截断前）
    pub body_chars: usize,
    pub status: DetailStatus,
    /// 高赞优先、已截断的评论
    pub comments: Vec<String>,
    /// 弹幕时间锚聚合（按分钟密度 + 该分钟代表句）；仅 B 站有
    pub danmaku_line: Option<String>,
    /// 证据类型：subtitle / article / post / thread
    pub kind: &'static str,
    /// 给用户/模型的下一步提示（登录、换链接、稍后重试等）
    pub hint: Option<String>,
}

impl ContentDetail {
    fn status_only(status: DetailStatus, hint: Option<&str>) -> Self {
        Self {
            status,
            hint: hint.map(str::to_string),
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_none() && self.comments.is_empty()
    }

    pub fn evidence(&self) -> &'static str {
        match self.status {
            DetailStatus::Full if self.kind == "subtitle" => "full_subtitle",
            DetailStatus::Excerpt if self.kind == "subtitle" => "subtitle_excerpt",
            DetailStatus::Full => "article_full",
            DetailStatus::Excerpt => "article_excerpt",
            _ if !self.comments.is_empty() => "comments_only",
            _ => "title_only",
        }
    }

    /// B站字幕上下文 → 通用证据（不重复实现抓取）
    pub fn from_bilibili(ctx: &CandidateContext) -> Self {
        let status = match ctx.subtitle_status {
            SubtitleStatus::NotFetched => DetailStatus::NotFetched,
            SubtitleStatus::LoginRequired => DetailStatus::LoginRequired,
            SubtitleStatus::Unavailable => DetailStatus::Unavailable,
            SubtitleStatus::Missing => DetailStatus::Missing,
            SubtitleStatus::Full => DetailStatus::Full,
            SubtitleStatus::Excerpt => DetailStatus::Excerpt,
        };
        let hint = match status {
            DetailStatus::LoginRequired => Some("请在内嵌浏览器登录 B站 后重试".to_string()),
            DetailStatus::Missing => Some("该视频未提供可读字幕（可能是纯音乐/无语音）".to_string()),
            DetailStatus::Unavailable => Some("字幕接口暂时不可用，本轮按标题评估".to_string()),
            _ => None,
        };
        Self {
            body: ctx.subtitle.clone(),
            body_chars: ctx.subtitle_chars,
            status,
            comments: ctx.comments.clone(),
            danmaku_line: ctx.danmaku.as_ref().and_then(|d| d.to_prompt_line()),
            kind: "subtitle",
            hint,
        }
    }
}

static CACHE: Lazy<RwLock<HashMap<(String, String), (i64, ContentDetail)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
/// API 平台限并发；DOM 平台由浏览器桥自身的 TOOL_LOCK 串行化
static FETCH_LIMIT: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(2)));

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// 平台是否受支持（由适配器表决定，新增平台不用改这里）
pub fn supported(platform: &str) -> bool {
    adapter(platform).is_some()
}

fn cache_ttl(detail: &ContentDetail) -> i64 {
    match detail.status {
        DetailStatus::Full | DetailStatus::Excerpt | DetailStatus::Missing => CACHE_TTL_SECS,
        _ => FAILURE_TTL_SECS,
    }
}

fn cached(key: &(String, String)) -> Option<ContentDetail> {
    CACHE
        .read()
        .get(key)
        .filter(|(at, d)| chrono::Utc::now().timestamp() - at < cache_ttl(d))
        .map(|(_, d)| d.clone())
}

fn cache_put(key: (String, String), detail: ContentDetail) {
    let now = chrono::Utc::now().timestamp();
    let mut cache = CACHE.write();
    cache.retain(|_, (at, d)| now - *at < cache_ttl(d));
    if cache.len() >= CACHE_CAP {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (now, detail));
}

/// 抓一个候选的正文 + 评论（带缓存；失败返回状态而非 Err）
pub async fn fetch(platform: &str, content_id: &str, url: &str) -> ContentDetail {
    if content_id.trim().is_empty() {
        return ContentDetail::status_only(DetailStatus::NotFetched, None);
    }
    let key = (platform.to_string(), content_id.to_string());
    if let Some(detail) = cached(&key) {
        return detail;
    }
    if platform == "bilibili" {
        let ctx = bilibili_auth::fetch_context(content_id).await;
        let detail = ContentDetail::from_bilibili(&ctx);
        cache_put(key, detail.clone());
        return detail;
    }
    let Some(adapter) = adapter(platform) else {
        return ContentDetail::status_only(DetailStatus::NotFetched, None);
    };
    let detail = tokio::time::timeout(Duration::from_secs(DOM_TIMEOUT_SECS), async {
        let _permit = FETCH_LIMIT.acquire().await.ok()?;
        Some(match adapter.kind {
            FetchKind::PublicApi => match adapter.platform {
                "v2ex" => fetch_v2ex(content_id).await,
                "reddit" => fetch_reddit(content_id).await,
                "youtube" => fetch_youtube(content_id).await,
                "github" => fetch_github(content_id).await,
                "arxiv" => fetch_arxiv(content_id).await,
                _ => ContentDetail::default(),
            },
            FetchKind::BrowserDom => fetch_dom(adapter, url).await,
            FetchKind::LoginApi => ContentDetail::default(),
        })
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| {
        ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("抓取超时（网络慢或页面未加载完），本轮按标题/简介评估"),
        )
    });
    cache_put(key, detail.clone());
    detail
}

/// 按链接抓取（供「按需读取」工具使用）
///
/// 支持 App 分享出来的短链（b23.tv / v.douyin.com / xhslink.com）：先跟随重定向还原。
pub async fn fetch_by_url(url: &str) -> Result<ContentDetail, String> {
    let (url, platform, id) = resolve_platform_url(url).await.ok_or_else(|| {
        format!(
            "暂不支持这个链接。目前支持：{}",
            supported_summary()
        )
    })?;
    Ok(fetch(&platform, &id, &url).await)
}

pub async fn resolve_platform_url(url: &str) -> Option<(String, String, String)> {
    let mut url = url.trim().to_string();
    if platform_and_id(&url).is_none() {
        if let Some(expanded) = expand_short_link(&url).await { url = expanded; }
    }
    let (platform, id) = platform_and_id(&url)?;
    Some((url, platform, id))
}

pub fn invalidate(platform: &str, id: &str) {
    CACHE.write().remove(&(platform.to_string(), id.to_string()));
}

/// 短链还原：只对已知分享域名发一次只读 GET（跟随重定向）
async fn expand_short_link(url: &str) -> Option<String> {
    let (host, _, _) = split_url(url)?;
    let is_short = SHORT_LINK_HOSTS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{}", d)));
    if !is_short {
        return None;
    }
    let resp = crate::network::http_client::get_global_client()
        .get(url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let final_url = resp.url().to_string();
    (final_url != url).then_some(final_url)
}

fn split_url(url: &str) -> Option<(String, Vec<String>, Vec<(String, String)>)> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..host_end]
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_string();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let path_and_query = &rest[host_end..];
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q.split('#').next().unwrap_or(q)),
        None => (path_and_query.split('#').next().unwrap_or(path_and_query), ""),
    };
    let seg: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let pairs = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Some((host, seg, pairs))
}

fn query_value(query: &[(String, String)], key: &str) -> Option<String> {
    query
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// 从链接推断平台 + 内容 ID；不支持的域名返回 `None`
///
/// 平台匹配由 [`ADAPTERS`] 决定（精确或子域名），各平台只负责解析自己的路径。
pub fn platform_and_id(url: &str) -> Option<(String, String)> {
    let (host, seg, query) = split_url(url)?;
    let adapter = adapter_for_host(&host)?;
    let id = match adapter.platform {
        "bilibili" => {
            let idx = seg.iter().position(|s| s == "video")?;
            let bv = seg.get(idx + 1)?;
            bv.starts_with("BV").then(|| bv.clone())?
        }
        "zhihu" => {
            // /question/<qid>[/answer/<aid>] · /answer/<aid> · /p/<id>
            let pos = seg
                .iter()
                .position(|s| s == "question" || s == "answer" || s == "p")?;
            let id = seg.get(pos + 1)?;
            id.chars().all(|c| c.is_ascii_digit()).then(|| id.clone())?
        }
        "xiaohongshu" => {
            // /explore/<id> · /discovery/item/<id>
            let pos = seg.iter().position(|s| s == "explore" || s == "discovery")?;
            let candidate = if seg[pos] == "discovery" && seg.get(pos + 1).map(String::as_str) == Some("item") {
                seg.get(pos + 2)
            } else {
                seg.get(pos + 1)
            }?;
            (candidate.len() >= 8).then(|| candidate.clone())?
        }
        "douyin" => {
            let pos = seg.iter().position(|s| s == "video")?;
            let id = seg.get(pos + 1)?;
            id.chars().all(|c| c.is_ascii_digit()).then(|| id.clone())?
        }
        "v2ex" => {
            let pos = seg.iter().position(|s| s == "t")?;
            let id = seg.get(pos + 1)?;
            id.chars().all(|c| c.is_ascii_digit()).then(|| id.clone())?
        }
        "reddit" => {
            let pos = seg.iter().position(|s| s == "comments")?;
            let id = seg.get(pos + 1)?;
            (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| id.clone())?
        }
        "twitter" => {
            // /{user}/status/<id> · /i/status/<id> · /i/web/status/<id>
            let pos = seg.iter().position(|s| s == "status")?;
            let id = seg.get(pos + 1)?;
            id.chars().all(|c| c.is_ascii_digit()).then(|| id.clone())?
        }
        "youtube" => {
            // /watch?v=<id> · youtu.be/<id> · /shorts/<id> · /embed/<id>
            if let Some(v) = query_value(&query, "v") {
                v
            } else if host == "youtu.be" {
                seg.first()?.clone()
            } else {
                let pos = seg.iter().position(|s| s == "shorts" || s == "embed" || s == "live")?;
                seg.get(pos + 1)?.clone()
            }
        }
        "github" => {
            // /owner/repo · /owner/repo/issues/<n> · /owner/repo/pull/<n>
            let owner = seg.first()?;
            let repo = seg.get(1)?;
            if seg.len() >= 4 && matches!(seg[2].as_str(), "issues" | "pull") {
                let n = &seg[3];
                if !n.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                format!("{}/{}/{}/{}", owner, repo, seg[2], n)
            } else if seg.len() == 2 {
                format!("{}/{}", owner, repo)
            } else {
                return None;
            }
        }
        "arxiv" => {
            // /abs/<id> · /pdf/<id>（旧式 id 可含 `/`，如 hep-th/9901001）
            let pos = seg.iter().position(|s| s == "abs" || s == "pdf")?;
            let rest = seg.get(pos + 1..)?;
            if rest.is_empty() {
                return None;
            }
            let id = rest.join("/");
            let id = id.strip_suffix(".pdf").unwrap_or(&id).to_string();
            id
        }
        _ => return None,
    };
    Some((adapter.platform.to_string(), id))
}

// ============================================================================
// V2EX（官方公开 API）
// ============================================================================

async fn fetch_v2ex(id: &str) -> ContentDetail {
    let client = crate::network::http_client::get_global_client();
    let topic_resp = client
        .get(format!("{}/api/topics/show.json", V2EX_BASE))
        .query(&[("id", id)])
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let topic = match topic_resp {
        Ok(r) => {
            let status = r.status().as_u16();
            if !(200..300).contains(&status) {
                // V2EX 官方 API 的 401/403/429 基本都是 IP 限频或拒绝，不是"未登录"
                return ContentDetail::status_only(
                    DetailStatus::Unavailable,
                    Some("V2EX 接口限频或拒绝访问，稍后再试"),
                );
            }
            r.json::<Value>().await.ok()
        }
        Err(_) => None,
    };
    let Some(topic) = topic else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("V2EX 接口请求失败，本轮按标题评估"),
        );
    };
    // 回复接口对 IP 限频更严：失败只丢评论，不影响正文
    let replies_resp = client
        .get(format!("{}/api/replies/show.json", V2EX_BASE))
        .query(&[("topic_id", id)])
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let replies = match replies_resp {
        Ok(r) => r.json::<Value>().await.unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    v2ex_detail(&topic, &replies)
}

/// topic + replies → 证据（纯函数，可单测）
pub(crate) fn v2ex_detail(topic: &Value, replies: &Value) -> ContentDetail {
    let first = topic.get(0).unwrap_or(topic);
    let body = first
        .get("content")
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let comments: Vec<String> = replies
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("content").and_then(|c| c.as_str()))
                .map(|t| truncate_chars(t.trim(), COMMENT_MAX_CHARS))
                .filter(|t| !t.is_empty() && t != "+1")
                .take(COMMENTS_PER_ITEM)
                .collect()
        })
        .unwrap_or_default();
    detail_from_parts(body, comments, "thread")
}

// ============================================================================
// Reddit（匿名 JSON）
// ============================================================================

async fn fetch_reddit(id: &str) -> ContentDetail {
    let client = crate::network::http_client::get_global_client();
    let url = format!(
        "{}/comments/{}.json?limit=50&sort=top&raw_json=1",
        REDDIT_BASE, id
    );
    let resp = client
        .get(&url)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let Ok(resp) = resp else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("Reddit 请求失败，本轮按标题评估"),
        );
    };
    // 401/403：私有版块或被拒绝（需要登录）；429：限频
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return ContentDetail::status_only(
            DetailStatus::LoginRequired,
            Some("Reddit 需要登录或该版块不可见；请在内嵌浏览器登录后重试"),
        );
    }
    if status == 429 {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("Reddit 接口限频，稍后再试"),
        );
    }
    let Ok(payload) = resp.json::<Value>().await else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("Reddit 响应解析失败，本轮按标题评估"),
        );
    };
    reddit_detail(&payload)
}

/// `[postListing, commentListing]` → 证据（纯函数，可单测）
pub(crate) fn reddit_detail(payload: &Value) -> ContentDetail {
    let post = payload
        .get(0)
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("children"))
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("data"));
    let body = post
        .and_then(|p| p.get("selftext").or_else(|| p.get("title")))
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "[removed]" && *t != "[deleted]");

    let mut scored: Vec<(i64, String)> = payload
        .get(1)
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("children"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let d = c.get("data")?;
                    let text = d.get("body")?.as_str()?.trim();
                    if text.is_empty() || text == "[removed]" || text == "[deleted]" {
                        return None;
                    }
                    Some((
                        d.get("score").and_then(|s| s.as_i64()).unwrap_or(0),
                        text.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let comments: Vec<String> = scored
        .into_iter()
        .map(|(_, t)| truncate_chars(&t, COMMENT_MAX_CHARS))
        .take(COMMENTS_PER_ITEM)
        .collect();
    detail_from_parts(body, comments, "thread")
}

// ============================================================================
// YouTube（公开页面 → 字幕轨 → 转写文本）
// ============================================================================

async fn fetch_youtube(id: &str) -> ContentDetail {
    let client = crate::network::http_client::get_global_client();
    let page = client
        .get("https://www.youtube.com/watch")
        .query(&[("v", id), ("hl", "en")])
        .header("User-Agent", UA)
        .header("Accept-Language", "en-US,en;q=0.9")
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let Ok(page) = page else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("YouTube 页面请求失败，本轮按标题评估"),
        );
    };
    if !(200..300).contains(&page.status().as_u16()) {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("YouTube 返回异常（可能需要登录或视频不可用）"),
        );
    }
    let html = page.text().await.unwrap_or_default();
    let Some(player) = extract_json_object_after(&html, "ytInitialPlayerResponse") else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("YouTube 页面结构未识别（可能需要登录或验证）"),
        );
    };
    let Some(tracks) = player
        .pointer("/captions/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(|t| t.as_array())
    else {
        return ContentDetail::status_only(
            DetailStatus::Missing,
            Some("该视频未开放字幕/转写（作者未提供或需要登录）"),
        );
    };
    let Some(track) = pick_caption_track(tracks) else {
        return ContentDetail::status_only(
            DetailStatus::Missing,
            Some("该视频未开放字幕/转写（作者未提供或需要登录）"),
        );
    };
    let Some(base) = track.get("baseUrl").and_then(|b| b.as_str()) else {
        return ContentDetail::status_only(
            DetailStatus::Missing,
            Some("字幕轨地址缺失，可能是 YouTube 风控"),
        );
    };
    let url = if base.contains('?') {
        format!("{base}&fmt=json3")
    } else {
        format!("{base}?fmt=json3")
    };
    let transcript_resp = client
        .get(&url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let transcript = match transcript_resp {
        Ok(r) => r.json::<Value>().await.ok(),
        Err(_) => None,
    };
    let Some(transcript) = transcript else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("字幕内容拉取失败，稍后再试"),
        );
    };
    let text = youtube_transcript_text(&transcript);
    if text.trim().is_empty() {
        return ContentDetail::status_only(
            DetailStatus::Missing,
            Some("字幕轨存在但没有可读文本"),
        );
    }
    detail_from_parts(Some(&text), Vec::new(), "subtitle")
}

/// 从 YouTube 播放器 JSON 里选字幕轨：中文优先，其次英文，否则第一条
fn pick_caption_track<'a>(tracks: &'a [Value]) -> Option<&'a Value> {
    let lang = |t: &Value| {
        t.get("languageCode")
            .and_then(|l| l.as_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    tracks
        .iter()
        .find(|t| lang(t).starts_with("zh"))
        .or_else(|| tracks.iter().find(|t| lang(t).starts_with("en")))
        .or_else(|| tracks.first())
}

/// json3 字幕：`events[].segs[].utf8` 拼接
fn youtube_transcript_text(value: &Value) -> String {
    value
        .get("events")
        .and_then(|e| e.as_array())
        .map(|events| {
            events
                .iter()
                .flat_map(|e| {
                    e.get("segs")
                        .and_then(|s| s.as_array())
                        .map(|segs| {
                            segs.iter()
                                .filter_map(|s| s.get("utf8").and_then(|u| u.as_str()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

// ============================================================================
// GitHub（公开 REST API：仓库 / Issue / PR）
// ============================================================================

async fn fetch_github(id: &str) -> ContentDetail {
    let parts: Vec<&str> = id.split('/').collect();
    let (owner, repo) = match (parts.first(), parts.get(1)) {
        (Some(o), Some(r)) => (*o, *r),
        _ => {
            return ContentDetail::status_only(
                DetailStatus::NotFetched,
                Some("GitHub 链接需要 owner/repo 两段路径"),
            )
        }
    };
    let is_issue = parts.len() == 4 && matches!(parts[2], "issues" | "pull");
    if is_issue {
        let number = parts[3];
        let issue = github_get(&format!("/repos/{owner}/{repo}/issues/{number}"), None).await;
        let Some(issue) = issue else {
            return ContentDetail::status_only(
                DetailStatus::Unavailable,
                Some("GitHub Issue 请求失败，本轮按标题评估"),
            );
        };
        let body = issue
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let body = match issue.get("body").and_then(|b| b.as_str()).map(str::trim) {
            Some(b) if !b.is_empty() => format!("{body}\n\n{b}"),
            _ => body,
        };
        let comments = github_get(
            &format!("/repos/{owner}/{repo}/issues/{number}/comments?per_page=30"),
            None,
        )
        .await
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            let mut scored: Vec<(i64, String)> = arr
                .iter()
                .filter_map(|c| {
                    let text = c.get("body")?.as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let score = c
                        .pointer("/reactions/total_count")
                        .and_then(|s| s.as_i64())
                        .unwrap_or(0);
                    Some((score, text.to_string()))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored
                .into_iter()
                .map(|(_, t)| truncate_chars(&t, COMMENT_MAX_CHARS))
                .take(COMMENTS_PER_ITEM)
                .collect()
        })
        .unwrap_or_default();
        return detail_from_parts(Some(&body), comments, "thread");
    }

    // 仓库：简介 + README 片段
    let Some(repo_info) = github_get(&format!("/repos/{owner}/{repo}"), None).await else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("GitHub 仓库请求失败（可能是私有库、不存在或匿名 API 限频）"),
        );
    };
    let description = repo_info
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let readme = github_get(
        &format!("/repos/{owner}/{repo}/readme"),
        Some("application/vnd.github.raw"),
    )
    .await
    .and_then(|v| v.as_str().map(str::to_string))
    .map(|t| truncate_chars(t.trim(), 3000))
    .filter(|t| !t.is_empty());
    let body = match (description.is_empty(), readme) {
        (true, None) => String::new(),
        (true, Some(r)) => r,
        (false, None) => description,
        (false, Some(r)) => format!("{description}\n\n{r}"),
    };
    detail_from_parts(
        (!body.is_empty()).then_some(body.as_str()),
        Vec::new(),
        "article",
    )
}

/// GitHub REST GET；`raw_accept` 用于 README 原始文本
async fn github_get(path: &str, raw_accept: Option<&str>) -> Option<Value> {
    let client = crate::network::http_client::get_global_client();
    let accept = raw_accept.unwrap_or("application/vnd.github+json");
    let resp = client
        .get(format!("https://api.github.com{path}"))
        .header("User-Agent", UA)
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    if !(200..300).contains(&resp.status().as_u16()) {
        return None;
    }
    if raw_accept.is_some() {
        resp.text().await.ok().map(Value::String)
    } else {
        resp.json::<Value>().await.ok()
    }
}

// ============================================================================
// arXiv（Atom API：标题 / 作者 / 摘要）
// ============================================================================

async fn fetch_arxiv(id: &str) -> ContentDetail {
    let client = crate::network::http_client::get_global_client();
    let resp = client
        .get("https://export.arxiv.org/api/query")
        .query(&[("id_list", id)])
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await;
    let Ok(resp) = resp else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("arXiv 请求失败，本轮按标题评估"),
        );
    };
    if !(200..300).contains(&resp.status().as_u16()) {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("arXiv 返回异常（可能是限频），稍后再试"),
        );
    }
    let xml = resp.text().await.unwrap_or_default();
    arxiv_detail(&xml)
}

/// Atom XML → 证据（标题 + 作者 + 摘要；纯函数，可单测）
pub(crate) fn arxiv_detail(xml: &str) -> ContentDetail {
    let entry = match (xml.find("<entry>"), xml.find("</entry>")) {
        (Some(start), Some(end)) if start < end => &xml[start..end],
        _ => xml,
    };
    let title = xml_tag(entry, "title");
    let summary = xml_tag(entry, "summary");
    if title.is_empty() && summary.is_empty() {
        return ContentDetail::status_only(
            DetailStatus::Missing,
            Some("arXiv 未返回条目（论文不存在或 ID 有误）"),
        );
    }
    let authors: Vec<String> = entry
        .split("<name>")
        .skip(1)
        .filter_map(|part| part.split("</name>").next().map(xml_unescape))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(5)
        .collect();
    let mut body = String::new();
    if !title.is_empty() {
        body.push_str(&format!("标题：{title}\n"));
    }
    if !authors.is_empty() {
        body.push_str(&format!("作者：{}\n", authors.join("、")));
    }
    if !summary.is_empty() {
        body.push_str(&format!("摘要：{summary}"));
    }
    detail_from_parts(Some(body.trim()), Vec::new(), "article")
}

/// 取第一个 `<tag>...</tag>` 文本并做空白归一
fn xml_tag(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return String::new();
    };
    let rest = &xml[start + open.len()..];
    let Some(end) = rest.find(&close) else {
        return String::new();
    };
    xml_unescape(&rest[..end])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// 从 HTML 里抠出 `marker` 之后的第一个完整 JSON 对象（大括号配对，跳过字符串）
fn extract_json_object_after(html: &str, marker: &str) -> Option<Value> {
    let idx = html.find(marker)? + marker.len();
    let rest = &html[idx..];
    let start = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&rest[start..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

// ============================================================================
// 知乎 / 小红书 / 抖音（内嵌浏览器固定提取器）
// ============================================================================

/// 后台标签提取失败文本里是否像"未登录/需要验证"（对照 bb-browser 的 auth 正则）
fn looks_like_login_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["401", "403", "unauthorized", "forbidden", "login", "登录", "sign in", "auth"]
        .iter()
        .any(|k| t.contains(k))
}

async fn fetch_dom(adapter: &PlatformAdapter, url: &str) -> ContentDetail {
    let platform = adapter.platform;
    if url.trim().is_empty() {
        return ContentDetail::status_only(DetailStatus::NotFetched, None);
    }
    let Some(bridge) = crate::browser_bridge::tools::global_bridge() else {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("内嵌浏览器未初始化；先在应用里打开一次浏览器"),
        );
    };
    if !bridge.is_connected() {
        return ContentDetail::status_only(
            DetailStatus::Unavailable,
            Some("内嵌浏览器连接已断开；重新打开浏览器再试"),
        );
    }
    if adapter.needs_login
        && !bridge
            .platform_status()
            .and_then(|(_, m)| m.get(platform).copied())
            .unwrap_or(false)
    {
        let hint = format!("请先在内嵌浏览器登录 {}，再重试这条链接", adapter.domains[0]);
        return ContentDetail::status_only(DetailStatus::LoginRequired, Some(&hint));
    }
    let args = serde_json::json!({
        "url": url,
        "extractor": "detail",
        // SPA 详情页（小红书/抖音）需要更长的渲染时间；失败也有标题回退
        "waitMs": 2000,
    });
    let text = bridge
        .request_tool(
            "browser_task_tab",
            &args,
            Duration::from_secs(DOM_TIMEOUT_SECS),
            Some("discovery-detail"),
        )
        .await;
    match text {
        Ok(text) => {
            let mut detail = parse_detail_text(&text, platform);
            if detail.status == DetailStatus::Missing {
                detail.hint = Some(format!(
                    "页面里没读到正文（可能需要登录，或正文是图片/视频）。可打开标准内容页重试：{}",
                    adapter.example
                ));
            }
            detail
        }
        Err(e) => {
            if looks_like_login_error(&e) {
                let hint = format!(
                    "{} 要求登录或验证；请在内嵌浏览器登录后重试",
                    adapter.domains[0]
                );
                return ContentDetail::status_only(DetailStatus::LoginRequired, Some(&hint));
            }
            let hint = format!("{} 页面读取失败（{}），本轮按标题评估", adapter.domains[0], e);
            ContentDetail::status_only(DetailStatus::Unavailable, Some(&hint))
        }
    }
}

/// 后台标签返回文本 → 证据（剥离 untrusted 包裹 + 双重 JSON 编码）
pub(crate) fn parse_detail_text(text: &str, platform: &str) -> ContentDetail {
    let text = text.trim();
    let text = if text.starts_with("<UNTRUSTED_PAGE_CONTENT") {
        match (text.find('>'), text.rfind("</UNTRUSTED_PAGE_CONTENT>")) {
            (Some(start), Some(end)) if start < end => text[start + 1..end].trim(),
            _ => return ContentDetail::default(),
        }
    } else {
        text
    };
    let Some(value) = crate::discovery::parse_json_tolerant(text) else {
        return ContentDetail::default();
    };
    let value = if let Some(inner) = value.as_str() {
        crate::discovery::parse_json_tolerant(inner).unwrap_or(Value::Null)
    } else {
        value
    };
    dom_detail(&value, platform)
}

/// 提取器 JSON → 证据（纯函数，可单测）
pub(crate) fn dom_detail(value: &Value, platform: &str) -> ContentDetail {
    let body = value
        .get("body")
        .and_then(|b| b.as_str())
        .map(str::trim)
        .filter(|b| !b.is_empty());
    let comments: Vec<String> = value
        .get("comments")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.as_str())
                .map(|t| truncate_chars(t.trim(), COMMENT_MAX_CHARS))
                .filter(|t| !t.is_empty())
                .take(COMMENTS_PER_ITEM)
                .collect()
        })
        .unwrap_or_default();
    let kind = match platform {
        "zhihu" => "article",
        _ => "post",
    };
    detail_from_parts(body, comments, kind)
}

/// 统一截断 + 状态判定
fn detail_from_parts(body: Option<&str>, comments: Vec<String>, kind: &'static str) -> ContentDetail {
    let (body, body_chars, status) = match body {
        Some(text) => {
            let chars = text.chars().count();
            let truncated = truncate_chars(text, BODY_MAX_CHARS);
            let status = if chars > BODY_MAX_CHARS {
                DetailStatus::Excerpt
            } else {
                DetailStatus::Full
            };
            (Some(truncated), chars, status)
        }
        // 没有正文是常态（删除/纯图/选择器失效）：仍保留评论，状态记为 Missing
        None => (None, 0, DetailStatus::Missing),
    };
    ContentDetail {
        body,
        body_chars,
        status,
        comments,
        danmaku_line: None,
        kind,
        hint: None,
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_and_id_known_urls() {
        assert_eq!(
            platform_and_id("https://www.bilibili.com/video/BV1abcDEFghi?p=1").unwrap(),
            ("bilibili".to_string(), "BV1abcDEFghi".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.zhihu.com/question/12345678").unwrap(),
            ("zhihu".to_string(), "12345678".to_string())
        );
        assert_eq!(
            platform_and_id("https://zhuanlan.zhihu.com/p/998877").unwrap(),
            ("zhihu".to_string(), "998877".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.xiaohongshu.com/explore/65a1b2c3d4e5f6a7b8c9d0e1?xsec_token=1").unwrap(),
            ("xiaohongshu".to_string(), "65a1b2c3d4e5f6a7b8c9d0e1".to_string())
        );
        // /discovery/item/<id> 也是小红书的笔记详情页
        assert_eq!(
            platform_and_id("https://www.xiaohongshu.com/discovery/item/65a1b2c3d4e5f6a7b8c9d0e1").unwrap(),
            ("xiaohongshu".to_string(), "65a1b2c3d4e5f6a7b8c9d0e1".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.douyin.com/video/7300000000000000000").unwrap(),
            ("douyin".to_string(), "7300000000000000000".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.v2ex.com/t/999888#reply3").unwrap(),
            ("v2ex".to_string(), "999888".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.reddit.com/r/rust/comments/1abc2d/some_title/").unwrap(),
            ("reddit".to_string(), "1abc2d".to_string())
        );
        // X / Twitter：用户时间线 / i/web 形式 / 带分享参数
        assert_eq!(
            platform_and_id("https://x.com/foo/status/1234567890?s=20").unwrap(),
            ("twitter".to_string(), "1234567890".to_string())
        );
        assert_eq!(
            platform_and_id("https://twitter.com/foo/status/1234567890/photo/1").unwrap(),
            ("twitter".to_string(), "1234567890".to_string())
        );
        assert_eq!(
            platform_and_id("https://x.com/i/web/status/9876543210").unwrap(),
            ("twitter".to_string(), "9876543210".to_string())
        );
        // YouTube：watch / 短链 / Shorts
        assert_eq!(
            platform_and_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10s").unwrap(),
            ("youtube".to_string(), "dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            platform_and_id("https://youtu.be/dQw4w9WgXcQ").unwrap(),
            ("youtube".to_string(), "dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            platform_and_id("https://www.youtube.com/shorts/abc123XYZ").unwrap(),
            ("youtube".to_string(), "abc123XYZ".to_string())
        );
        // GitHub：仓库 / Issue / PR
        assert_eq!(
            platform_and_id("https://github.com/rust-lang/rust").unwrap(),
            ("github".to_string(), "rust-lang/rust".to_string())
        );
        assert_eq!(
            platform_and_id("https://github.com/rust-lang/rust/issues/12345").unwrap(),
            ("github".to_string(), "rust-lang/rust/issues/12345".to_string())
        );
        assert_eq!(
            platform_and_id("https://github.com/rust-lang/rust/pull/67890#discussion").unwrap(),
            ("github".to_string(), "rust-lang/rust/pull/67890".to_string())
        );
        // arXiv：新版 id / 旧式 id / pdf
        assert_eq!(
            platform_and_id("https://arxiv.org/abs/2401.12345v2").unwrap(),
            ("arxiv".to_string(), "2401.12345v2".to_string())
        );
        assert_eq!(
            platform_and_id("https://arxiv.org/abs/hep-th/9901001").unwrap(),
            ("arxiv".to_string(), "hep-th/9901001".to_string())
        );
        assert_eq!(
            platform_and_id("https://arxiv.org/pdf/2401.12345.pdf").unwrap(),
            ("arxiv".to_string(), "2401.12345".to_string())
        );
    }

    #[test]
    fn test_platform_and_id_rejects_unknown_or_incomplete() {
        assert!(platform_and_id("https://example.com/article/1").is_none());
        assert!(platform_and_id("https://www.zhihu.com/question/abc").is_none());
        assert!(platform_and_id("https://www.v2ex.com/member/foo").is_none());
        assert!(platform_and_id("not a url").is_none());
        // 短链需要先跟随重定向（fetch_by_url 里做），这里不应误判
        assert!(platform_and_id("https://b23.tv/abcdefg").is_none());
    }

    #[test]
    fn test_adapter_registry_matches_subdomains() {
        assert_eq!(adapter_for_host("zhuanlan.zhihu.com").unwrap().platform, "zhihu");
        assert_eq!(adapter_for_host("www.v2ex.com").unwrap().platform, "v2ex");
        assert_eq!(adapter_for_host("old.reddit.com").unwrap().platform, "reddit");
        assert_eq!(adapter_for_host("m.youtube.com").unwrap().platform, "youtube");
        assert_eq!(adapter_for_host("youtu.be").unwrap().platform, "youtube");
        assert_eq!(adapter_for_host("mobile.x.com").unwrap().platform, "twitter");
        assert_eq!(adapter_for_host("twitter.com").unwrap().platform, "twitter");
        assert_eq!(adapter_for_host("gist.github.com").unwrap().platform, "github");
        assert_eq!(adapter_for_host("export.arxiv.org").unwrap().platform, "arxiv");
        assert!(adapter_for_host("b23.tv").is_none());
        assert!(adapter_for_host("example.com").is_none());
        assert!(supported("douyin"));
        assert!(supported("youtube"));
        assert!(supported("twitter"));
        assert!(!supported("weibo"));
        let summary = supported_summary();
        assert!(summary.contains("v2ex"));
        assert!(summary.contains("xiaohongshu"));
        assert!(summary.contains("github"));
    }

    #[test]
    fn test_extract_json_object_after() {
        let html = r#"<script>var ytInitialPlayerResponse = {"a":{"b":"}{"},"c":[1,2]};</script>"#;
        let v = extract_json_object_after(html, "ytInitialPlayerResponse").unwrap();
        assert_eq!(v["a"]["b"], "}{");
        assert_eq!(v["c"][1], 2);
        assert!(extract_json_object_after("<html></html>", "ytInitialPlayerResponse").is_none());
    }

    #[test]
    fn test_youtube_caption_track_and_text() {
        let tracks = vec![
            serde_json::json!({"languageCode": "en", "baseUrl": "https://a"}),
            serde_json::json!({"languageCode": "zh-CN", "baseUrl": "https://b"}),
        ];
        assert_eq!(
            pick_caption_track(&tracks).unwrap()["baseUrl"],
            "https://b"
        );
        let transcript = serde_json::json!({
            "events": [
                {"segs": [{"utf8": "hello"}, {"utf8": "world"}]},
                {"segs": [{"utf8": "again"}]}
            ]
        });
        assert_eq!(youtube_transcript_text(&transcript), "hello world again");
        assert_eq!(youtube_transcript_text(&serde_json::json!({})), "");
    }

    #[test]
    fn test_arxiv_detail_parses_entry() {
        let xml = r#"<feed>
          <entry>
            <id>http://arxiv.org/abs/2401.12345v1</id>
            <title> A  Study of Things </title>
            <summary>This is the abstract &amp; more.</summary>
            <author><name>Alice</name></author>
            <author><name>Bob</name></author>
          </entry>
        </feed>"#;
        let d = arxiv_detail(xml);
        assert_eq!(d.kind, "article");
        assert_eq!(d.status, DetailStatus::Full);
        let body = d.body.unwrap();
        assert!(body.contains("A Study of Things"));
        assert!(body.contains("Alice、Bob"));
        assert!(body.contains("abstract & more"));
        let missing = arxiv_detail("<feed></feed>");
        assert_eq!(missing.status, DetailStatus::Missing);
    }

    #[test]
    fn test_login_error_detection() {
        assert!(looks_like_login_error("login required"));
        assert!(looks_like_login_error("HTTP 401"));
        assert!(looks_like_login_error("需要登录后查看"));
        assert!(!looks_like_login_error("network timeout"));
    }

    #[test]
    fn test_status_only_carries_hint() {
        let d = ContentDetail::status_only(DetailStatus::LoginRequired, Some("请登录"));
        assert_eq!(d.status, DetailStatus::LoginRequired);
        assert_eq!(d.hint.as_deref(), Some("请登录"));
        assert!(d.body.is_none());
    }

    #[test]
    fn test_v2ex_detail_parts() {
        let topic = serde_json::json!([
            {"content": "这是一篇完整的主题正文"}
        ]);
        let replies = serde_json::json!([
            {"content": "第一条回复"},
            {"content": "+1"},
            {"content": ""}
        ]);
        let d = v2ex_detail(&topic, &replies);
        assert_eq!(d.kind, "thread");
        assert_eq!(d.status, DetailStatus::Full);
        assert_eq!(d.body.as_deref(), Some("这是一篇完整的主题正文"));
        assert_eq!(d.comments, vec!["第一条回复".to_string()]);
        assert_eq!(d.evidence(), "article_full");

        let empty = v2ex_detail(&serde_json::json!([]), &serde_json::json!([]));
        assert_eq!(empty.status, DetailStatus::Missing);
        assert_eq!(empty.evidence(), "title_only");
    }

    #[test]
    fn test_reddit_detail_sorts_by_score_and_filters_removed() {
        let payload = serde_json::json!([
            {"data": {"children": [{"data": {"selftext": "正文内容", "title": "t"}}]}},
            {"data": {"children": [
                {"data": {"body": "低分评论", "score": 1}},
                {"data": {"body": "高分评论", "score": 88}},
                {"data": {"body": "[removed]", "score": 99}},
                {"data": {"body": "   ", "score": 50}}
            ]}}
        ]);
        let d = reddit_detail(&payload);
        assert_eq!(d.status, DetailStatus::Full);
        assert_eq!(d.body.as_deref(), Some("正文内容"));
        assert_eq!(d.comments, vec!["高分评论".to_string(), "低分评论".to_string()]);
    }

    #[test]
    fn test_dom_detail_truncates_and_reports_status() {
        let long = "字".repeat(BODY_MAX_CHARS + 10);
        let v = serde_json::json!({"body": long, "comments": ["a", "b"]});
        let d = dom_detail(&v, "zhihu");
        assert_eq!(d.kind, "article");
        assert_eq!(d.status, DetailStatus::Excerpt);
        assert_eq!(d.body.as_ref().unwrap().chars().count(), BODY_MAX_CHARS);
        assert_eq!(d.evidence(), "article_excerpt");

        let short = dom_detail(&serde_json::json!({"body": "短文", "comments": []}), "douyin");
        assert_eq!(short.status, DetailStatus::Full);
        assert_eq!(short.kind, "post");
        assert_eq!(short.evidence(), "article_full");

        let missing = dom_detail(&serde_json::json!({}), "xiaohongshu");
        assert_eq!(missing.status, DetailStatus::Missing);
        assert_eq!(missing.evidence(), "title_only");
    }

    #[test]
    fn test_parse_detail_text_unwraps_and_parses() {
        let inner = r#"{"body":"正文","comments":["评论"]}"#;
        let wrapped = format!(
            "<UNTRUSTED_PAGE_CONTENT nonce=\"x\">\n{}\n</UNTRUSTED_PAGE_CONTENT>",
            inner
        );
        let d = parse_detail_text(&wrapped, "zhihu");
        assert_eq!(d.body.as_deref(), Some("正文"));
        assert_eq!(d.comments, vec!["评论".to_string()]);

        // 双重 JSON 编码（eval 的 safeSerialize）
        let double = serde_json::to_string(&inner).unwrap();
        let d = parse_detail_text(&double, "zhihu");
        assert_eq!(d.body.as_deref(), Some("正文"));

        assert_eq!(parse_detail_text("", "zhihu").status, DetailStatus::NotFetched);
    }

    #[test]
    fn test_bilibili_mapping_keeps_subtitle_labels() {
        let ctx = CandidateContext {
            subtitle: Some("字幕".into()),
            subtitle_status: SubtitleStatus::Full,
            subtitle_chars: 2,
            ..Default::default()
        };
        let d = ContentDetail::from_bilibili(&ctx);
        assert_eq!(d.kind, "subtitle");
        assert_eq!(d.evidence(), "full_subtitle");
    }

    #[test]
    fn test_failure_cache_outlives_short_ttl_only_for_success() {
        let failed = ContentDetail {
            status: DetailStatus::Unavailable,
            ..Default::default()
        };
        let full = ContentDetail {
            body: Some("x".into()),
            status: DetailStatus::Full,
            kind: "article",
            ..Default::default()
        };
        assert_eq!(cache_ttl(&failed), FAILURE_TTL_SECS);
        assert_eq!(cache_ttl(&full), CACHE_TTL_SECS);
    }
}
