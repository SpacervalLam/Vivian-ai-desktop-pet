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

/// 每轮最多富化几个视频（成本与风控上限）
pub const SUBTITLE_FETCH_CAP: usize = 6;
/// 字幕注入 LLM 前的字数上限
const SUBTITLE_MAX_CHARS: usize = 4000;
/// 评论样本条数 / 每条字数上限
const COMMENTS_PER_VIDEO: usize = 5;
const COMMENT_MAX_CHARS: usize = 120;
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

/// 一个候选的富化上下文（字幕 + 热门评论），供 LLM 评估注入
#[derive(Debug, Clone, Default)]
pub struct CandidateContext {
    pub subtitle: Option<String>,
    pub comments: Vec<String>,
    pub subtitle_status: SubtitleStatus,
    pub subtitle_chars: usize,
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
pub async fn fetch_context(bvid: &str) -> CandidateContext {
    let bvid = bvid.trim();
    if bvid.is_empty() {
        return CandidateContext::default();
    }
    let Some(cookie) = resolve_cookie() else {
        return CandidateContext {
            subtitle_status: SubtitleStatus::LoginRequired,
            ..Default::default()
        };
    };
    use md5::{Digest, Md5};
    let key = (
        bvid.to_string(),
        format!("{:x}", Md5::digest(cookie.as_bytes())),
    );
    if let Some(ctx) = cached(&key) {
        return ctx;
    }
    let ctx = tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_SECS), async {
        let _permit = FETCH_LIMIT.acquire().await.ok()?;
        if let Some(ctx) = cached(&key) {
            return Some(ctx);
        }
        let ctx = fetch_inner(bvid, &cookie).await;
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

async fn fetch_inner(bvid: &str, cookie: &str) -> CandidateContext {
    let client = &crate::discovery::BILI_CLIENT;
    let Ok(meta) = client.video_view(bvid, Some(cookie)).await else {
        return CandidateContext {
            subtitle_status: SubtitleStatus::Unavailable,
            ..Default::default()
        };
    };
    // 每条支路独立超时：评论失败不能抹掉已经读到的字幕，反之亦然。
    let (subtitle, comments) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(6),
            client.subtitles(bvid, meta.cid, Some(cookie))
        ),
        tokio::time::timeout(
            Duration::from_secs(6),
            client.top_comments(bvid, meta.aid, Some(cookie), COMMENTS_PER_VIDEO)
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
    CandidateContext {
        subtitle,
        comments,
        subtitle_status,
        subtitle_chars,
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
