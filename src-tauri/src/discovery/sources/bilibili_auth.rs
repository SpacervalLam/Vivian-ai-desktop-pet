//! B 站登录态富化 — 复用内嵌浏览器的 B站 cookie，抓「字幕原文」与热门评论
//!
//! 与 `x.rs` / `reddit.rs` 同一套「cookie 重放」模式：cookie 来自
//! [`BridgeState::bilibili_cookie`](crate::browser_bridge::service::BridgeState::bilibili_cookie)
//! （内嵌浏览器普通配置，用户在浏览器里登录过 B站即可）。
//!
//! 显式区分未登录、抓取失败、无可读字幕与字幕节选，避免把缺失证据当成视频事实。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use tokio::sync::Semaphore;

use crate::discovery::danmaku::{self, DanmakuDigest};

/// 每轮最多富化几个视频（成本与风控上限）
pub const SUBTITLE_FETCH_CAP: usize = 6;
/// 字幕注入 LLM 前的字数上限
const SUBTITLE_MAX_CHARS: usize = 4000;
/// 评论样本条数 / 每条字数上限
const COMMENTS_PER_VIDEO: usize = 5;
const COMMENT_MAX_CHARS: usize = 120;
/// 弹幕只取第一个分段（前 6 分钟的黄金段，密度最高）；多取分段收益递减
/// 而请求数线性增长，评估阶段用不上
const DANMAKU_SEGMENT_INDEX: u32 = 1;
const DANMAKU_SEGMENT_SECS: u64 = 360;
const DANMAKU_TIMEOUT_SECS: u64 = 6;
/// 单个视频富化的整体超时
const FETCH_TIMEOUT_SECS: u64 = 12;
/// 缓存 TTL（同一 bvid 会被 discovery 的多条路径反复取）
const CACHE_TTL_SECS: i64 = 24 * 3600;
const FAILURE_TTL_SECS: i64 = 60;
const CACHE_CAP: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleStatus {
    #[default]
    NotFetched,
    LoginRequired,
    Unavailable,
    Missing,
    Full,
    Excerpt,
}

/// 一个候选的富化上下文（字幕 + 热门评论 + 弹幕密度），供 LLM 评估注入
#[derive(Debug, Clone, Default)]
pub struct CandidateContext {
    pub subtitle: Option<String>,
    pub comments: Vec<String>,
    pub subtitle_status: SubtitleStatus,
    pub subtitle_chars: usize,
    /// 弹幕时间锚聚合（公开接口，未登录也有）；`None` = 未取得或该段无弹幕
    pub danmaku: Option<DanmakuDigest>,
}

impl CandidateContext {
    pub fn is_empty(&self) -> bool {
        self.subtitle.is_none() && self.comments.is_empty()
    }

    pub fn transcript_source(&self) -> &'static str {
        match self.subtitle_status {
            SubtitleStatus::Full => "full_subtitle",
            SubtitleStatus::Excerpt => "subtitle_excerpt",
            _ if !self.comments.is_empty() => "comments_only",
            _ => "title_only",
        }
    }
}

static CACHE: Lazy<RwLock<HashMap<(String, String), (i64, CandidateContext)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
/// 限并发（B站风控考量），同时最多 2 个视频在抓
static FETCH_LIMIT: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(2)));

/// cookie 里是否带登录凭据（SESSDATA）
fn has_sessdata(cookie: &str) -> bool {
    cookie
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .any(|(name, value)| name == "SESSDATA" && !value.trim().is_empty())
}

/// 当前可用的 B站 cookie（环境变量优先，其次内嵌浏览器回传）
fn resolve_cookie() -> Option<String> {
    if let Ok(env_cookie) = std::env::var("VIVIAN_BILI_COOKIE") {
        let c = env_cookie.trim();
        if has_sessdata(c) {
            return Some(c.to_string());
        }
    }
    let bridge = crate::browser_bridge::tools::global_bridge()?;
    let (_, cookie) = bridge.bilibili_cookie()?;
    if has_sessdata(&cookie) {
        Some(cookie)
    } else {
        None
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn cache_ttl(ctx: &CandidateContext) -> i64 {
    // 拿到弹幕就按内容缓存：弹幕是稳定数据，不该因为字幕未登录而在 60 秒后失效
    if ctx.danmaku.is_some() {
        return CACHE_TTL_SECS;
    }
    match ctx.subtitle_status {
        SubtitleStatus::Full | SubtitleStatus::Excerpt | SubtitleStatus::Missing => CACHE_TTL_SECS,
        _ => FAILURE_TTL_SECS,
    }
}

fn cached(key: &(String, String)) -> Option<CandidateContext> {
    CACHE
        .read()
        .get(key)
        .filter(|(at, ctx)| chrono::Utc::now().timestamp() - at < cache_ttl(ctx))
        .map(|(_, ctx)| ctx.clone())
}

/// 按登录态隔离缓存；短缓存故障、限制容量，并将排队时间纳入总超时。
///
/// 没有登录态时**仍然抓弹幕**（公开接口）——字幕与评论都拿不到，但观众反应
/// 的密度信号还能作为评估锚点，比只报「LoginRequired」多一层信息。
pub async fn fetch_context(bvid: &str) -> CandidateContext {
    fetch_part_context(bvid, 1).await
}

pub fn invalidate_context(bvid: &str, page: u32) {
    let key = if page == 1 { bvid.to_string() } else { format!("{bvid}:p{page}") };
    CACHE.write().retain(|(cached, _), _| cached != &key);
}

pub async fn fetch_part_context(bvid: &str, page: u32) -> CandidateContext {
    let bvid = bvid.trim();
    if bvid.is_empty() {
        return CandidateContext::default();
    }
    let cookie = resolve_cookie();
    use md5::{Digest, Md5};
    // 缓存按登录态隔离：未登录拿到的弹幕结果不能与登录后的字幕结果共用同一格
    let key = (
        if page == 1 { bvid.to_string() } else { format!("{bvid}:p{page}") },
        match cookie.as_deref() {
            Some(c) => format!("{:x}", Md5::digest(c.as_bytes())),
            None => "anon".to_string(),
        },
    );
    if let Some(ctx) = cached(&key) {
        return ctx;
    }
    let ctx = tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_SECS), async {
        let _permit = FETCH_LIMIT.acquire().await.ok()?;
        if let Some(ctx) = cached(&key) {
            return Some(ctx);
        }
        let ctx = fetch_inner(bvid, page, cookie.as_deref()).await;
        let now = chrono::Utc::now().timestamp();
        let mut cache = CACHE.write();
        cache.retain(|_, (at, c)| now - *at < cache_ttl(c));
        if cache.len() >= CACHE_CAP {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(key.clone(), (now, ctx.clone()));
        Some(ctx)
    })
    .await
    .ok()
    .flatten();
    ctx.unwrap_or_else(|| CandidateContext {
        subtitle_status: SubtitleStatus::Unavailable,
        ..Default::default()
    })
}

/// 分段解析结果 → 聚合摘要
///
/// 「何时算取到」的判定在 `danmaku::DanmakuDigest::new_if_useful` 里，
/// 那是纯逻辑，能进隔离 harness 覆盖。
fn digest_danmaku(elems: Vec<danmaku::DanmakuElem>) -> Option<DanmakuDigest> {
    DanmakuDigest::new_if_useful(&elems, DANMAKU_SEGMENT_SECS)
}

async fn fetch_inner(bvid: &str, page: u32, cookie: Option<&str>) -> CandidateContext {
    let client = &crate::discovery::BILI_CLIENT;
    let Ok(meta) = client.video_part(bvid, page, cookie).await else {
        return CandidateContext {
            // 无登录态时请求失败也不能报「接口不可用」——根因仍然是没登录，
            // 状态会直接决定 hint 措辞与日报里的证据档位
            subtitle_status: if cookie.is_some() {
                SubtitleStatus::Unavailable
            } else {
                SubtitleStatus::LoginRequired
            },
            ..Default::default()
        };
    };

    // 弹幕是公开数据，不发送登录凭据（与字幕 CDN 同一口径）
    let danmaku_fut = client.danmaku_segment(meta.cid, meta.aid, DANMAKU_SEGMENT_INDEX, None);

    let Some(cookie) = cookie else {
        // 无登录态：字幕与评论都要登录，只有弹幕还能取到
        let danmaku = tokio::time::timeout(
            Duration::from_secs(DANMAKU_TIMEOUT_SECS),
            danmaku_fut,
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(digest_danmaku);
        return CandidateContext {
            subtitle_status: SubtitleStatus::LoginRequired,
            danmaku,
            ..Default::default()
        };
    };

    // 每条支路独立超时：评论失败不能抹掉已经读到的字幕，反之亦然。
    let (subtitle, comments, danmaku) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(6),
            client.subtitles(bvid, meta.cid, Some(cookie))
        ),
        tokio::time::timeout(
            Duration::from_secs(6),
            client.top_comments(bvid, meta.aid, Some(cookie), COMMENTS_PER_VIDEO)
        ),
        tokio::time::timeout(
            Duration::from_secs(DANMAKU_TIMEOUT_SECS),
            danmaku_fut
        ),
    );
    let (subtitle, subtitle_status, subtitle_chars) = match subtitle {
        Ok(Ok(Some(text))) => {
            let chars = text.chars().count();
            (
                Some(truncate_chars(&text, SUBTITLE_MAX_CHARS)),
                if chars > SUBTITLE_MAX_CHARS {
                    SubtitleStatus::Excerpt
                } else {
                    SubtitleStatus::Full
                },
                chars,
            )
        }
        Ok(Ok(None)) => (None, SubtitleStatus::Missing, 0),
        _ => (None, SubtitleStatus::Unavailable, 0),
    };
    let comments: Vec<String> = comments
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
        .into_iter()
        .map(|c| truncate_chars(c.text.trim(), COMMENT_MAX_CHARS))
        .filter(|s| !s.is_empty())
        .collect();
    let danmaku = danmaku.ok().and_then(Result::ok).and_then(digest_danmaku);
    CandidateContext {
        subtitle,
        comments,
        subtitle_status,
        subtitle_chars,
        danmaku,
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_sessdata() {
        assert!(has_sessdata("SESSDATA=abc; bili_jct=def"));
        assert!(!has_sessdata("buvid3=xyz"));
        assert!(!has_sessdata(""));
        assert!(!has_sessdata("NOT_SESSDATA=abc"));
        assert!(!has_sessdata("note=SESSDATA; SESSDATA=  "));
        assert!(has_sessdata("buvid3=xyz; SESSDATA=abc=="));
    }

    #[test]
    fn test_truncate_chars() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        // 按字符而非字节截断
        assert_eq!(truncate_chars("中文测试", 2), "中文");
    }

    #[test]
    fn test_context_is_empty() {
        assert!(CandidateContext::default().is_empty());
        assert!(!CandidateContext {
            subtitle: Some("x".into()),
            comments: vec![],
            ..Default::default()
        }
        .is_empty());
        assert!(!CandidateContext {
            subtitle: None,
            comments: vec!["y".into()],
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn failure_cache_expires_before_content_cache() {
        let failed = CandidateContext {
            subtitle_status: SubtitleStatus::Unavailable,
            ..Default::default()
        };
        let missing = CandidateContext {
            subtitle_status: SubtitleStatus::Missing,
            ..Default::default()
        };
        let now = chrono::Utc::now().timestamp();
        let failed_key = ("test-cache-expiry".to_string(), "account-a".to_string());
        let valid_key = ("test-cache-expiry".to_string(), "account-b".to_string());
        {
            let mut cache = CACHE.write();
            cache.insert(failed_key.clone(), (now - 120, failed));
            cache.insert(valid_key.clone(), (now - 120, missing));
        }
        assert!(cached(&failed_key).is_none());
        assert_eq!(
            cached(&valid_key).unwrap().subtitle_status,
            SubtitleStatus::Missing
        );
        assert!(cached(&("test-cache-expiry".to_string(), "new-login".to_string())).is_none());
        let mut cache = CACHE.write();
        cache.remove(&failed_key);
        cache.remove(&valid_key);
    }

    #[test]
    fn excerpts_and_comments_do_not_claim_full_transcript() {
        let ctx = CandidateContext {
            subtitle: Some("字幕前段".into()),
            subtitle_status: SubtitleStatus::Excerpt,
            subtitle_chars: 9000,
            ..Default::default()
        };
        assert_eq!(ctx.transcript_source(), "subtitle_excerpt");
        let ctx = CandidateContext {
            comments: vec!["评论".into()],
            subtitle_status: SubtitleStatus::Unavailable,
            ..Default::default()
        };
        assert_eq!(ctx.transcript_source(), "comments_only");
        assert_eq!(
            CandidateContext::default().transcript_source(),
            "title_only"
        );
    }
}
