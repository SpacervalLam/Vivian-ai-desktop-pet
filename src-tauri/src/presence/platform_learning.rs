//! 知识与语料采集共用的「平台内找 → 读原文 → 网页补充」链路。
use std::time::Duration;

use crate::config::WebSearchConfig;
use crate::discovery::{
    learning_policy,
    sources::{content_detail, task_tabs, ContentCandidate},
};
use crate::network::web::{WebSearchRequest, WebSearchService, WebSearchSource};

/// 直接进平台找资料。未登录/接口故障返回错误，由调用方决定补充来源。
pub async fn platform_sources(
    char_id: &str,
    platform: &str,
    topic: &str,
    limit: usize,
) -> Result<Vec<WebSearchSource>, String> {
    let search = async {
        match platform {
            "bilibili" => crate::discovery::BILI_CLIENT
                .search(topic, 1, limit.min(5) as u32)
                .await
                .map(|videos| videos.iter().map(ContentCandidate::from_bilibili).collect()),
            "xiaohongshu" | "douyin" | "zhihu" => {
                task_tabs::search_platform(char_id, platform, topic, limit).await
            }
            _ => Ok(Vec::new()),
        }
    };
    let candidates = tokio::time::timeout(Duration::from_secs(20), search)
        .await
        .map_err(|_| format!("{platform} 平台检索超时"))??;
    let mut sources: Vec<_> = candidates
        .into_iter()
        .map(|candidate| {
            let mut source = WebSearchSource::new(candidate.url);
            source.title = Some(candidate.title);
            source.snippet = Some(candidate.description);
            source.published_at = chrono::DateTime::from_timestamp(candidate.pubdate, 0)
                .filter(|_| candidate.pubdate > 0)
                .map(|date| date.to_rfc3339());
            source.engines = vec![format!("platform:{platform}")];
            source.source_tier = "P3".into();
            source.confidence = "SINGLE-SOURCE".into();
            source.retrieved_at = chrono::Utc::now().to_rfc3339();
            source
        })
        .collect();
    enrich_sources(&mut sources).await;
    Ok(sources)
}

/// 至多读两条原文，平台用原生字幕/正文适配器，普通网页用现有有界抓取器。
/// 不把评论、弹幕或登录页拼进 raw_content，只有正文可成为自我成长证据。
pub async fn enrich_sources(sources: &mut [WebSearchSource]) {
    for source in sources
        .iter_mut()
        .filter(|source| !learning_policy::has_reading(source))
        .take(2)
    {
        let read = async {
            if let Some((platform, id)) = content_detail::platform_and_id(&source.url) {
                let detail = content_detail::fetch(&platform, &id, &source.url).await;
                let mut reactions = String::new();
                if !detail.comments.is_empty() {
                    reactions.push_str(&format!(
                        "\n观众评论样本（不代表事实）：{}",
                        detail.comments.join(" / ")
                    ));
                }
                if let Some(line) = detail.danmaku_line {
                    reactions.push_str(&format!("\n弹幕反应（不代表事实）：{line}"));
                }
                if !reactions.is_empty() {
                    source.snippet = Some(format!("{}{}", source.snippet_text(), reactions));
                }
                detail.body
            } else {
                crate::network::url_fetcher::fetch_page(&source.url)
                    .await
                    .ok()
                    .map(|page| page.text)
            }
        };
        if let Ok(Some(body)) = tokio::time::timeout(Duration::from_secs(12), read).await {
            if body.trim().chars().count() >= 40 {
                source.raw_content = Some(body.chars().take(6000).collect());
            }
        }
    }
}

pub async fn topic_sources(
    char_id: &str,
    topic: &str,
    limit: usize,
    config: &WebSearchConfig,
    proxy: Option<&str>,
) -> Vec<WebSearchSource> {
    let platform = learning_policy::primary_platform(char_id);
    let primary = match platform_sources(char_id, platform, topic, limit.min(3)).await {
        Ok(sources) => sources,
        Err(error) => {
            tracing::info!(
                "[PlatformLearning:{char_id}] 首选来源 {platform} 暂不可读：{error}；尝试补充资料"
            );
            Vec::new()
        }
    };
    // 真读到首选平台正文才算命中；标题命中、未登录不应阻断补充检索。
    if primary.iter().any(learning_policy::has_reading) {
        return learning_policy::merge_sources(primary, vec![], limit);
    }
    let request = WebSearchRequest::new(topic).with_max_results(limit);
    let mut fallback = WebSearchService::shared()
        .search(&request, Some(config), proxy)
        .await
        .map(|result| result.sources)
        .unwrap_or_default();
    enrich_sources(&mut fallback).await;
    learning_policy::merge_sources(primary, fallback, limit)
}

pub fn reflect_on_sources(char_id: &str, sources: &[WebSearchSource]) {
    crate::persona::self_development::spawn_from_reading(
        char_id,
        sources
            .iter()
            .filter(|source| learning_policy::has_reading(source))
            .filter_map(|source| {
                source.raw_content.as_ref().map(|body| {
                    crate::persona::self_development::ReadingExperience {
                        url: source.url.clone(),
                        body: body.clone(),
                    }
                })
            })
            .collect(),
    );
}
