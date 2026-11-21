//! 角色自己的学习来源；与用户兴趣画像分开，优先级不排斥其他资料源。
use crate::network::web::WebSearchSource;

pub fn primary_platform(char_id: &str) -> &'static str {
    if char_id.eq_ignore_ascii_case("nana") {
        "xiaohongshu"
    } else {
        "bilibili"
    }
}

pub fn source_guidance(char_id: &str) -> &'static str {
    if primary_platform(char_id) == "xiaohongshu" {
        "你的日常学习优先浏览小红书，围绕自己的人设兴趣与真实好奇心读笔记正文和评论；需要补充时再查其他来源。"
    } else {
        "你的日常学习优先浏览B站，围绕自己的人设兴趣与真实好奇心读视频字幕、弹幕和评论；需要补充时再查其他来源。"
    }
}

pub fn has_reading(source: &WebSearchSource) -> bool {
    source
        .raw_content
        .as_deref()
        .is_some_and(|body| body.trim().chars().count() >= 40)
}

/// 优先阅读材料排在前面，同级保留顺序；URL 原样保留（小红书访问令牌供读取用）。
pub fn merge_sources(
    primary: Vec<WebSearchSource>,
    fallback: Vec<WebSearchSource>,
    limit: usize,
) -> Vec<WebSearchSource> {
    let mut sources: Vec<WebSearchSource> = Vec::new();
    for source in primary
        .into_iter()
        .chain(fallback)
        .filter(|source| !source.url.is_empty())
    {
        if let Some(existing) = sources
            .iter_mut()
            .find(|existing| existing.url == source.url)
        {
            if !has_reading(existing) && has_reading(&source) {
                *existing = source;
            }
        } else {
            sources.push(source);
        }
    }
    sources.sort_by_key(|source| !has_reading(source));
    sources.truncate(limit);
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(url: &str, read: bool) -> WebSearchSource {
        let mut source = WebSearchSource::new(url);
        source.raw_content = read.then(|| "实际读取的原文内容".repeat(8));
        source
    }
    #[test]
    fn characters_have_distinct_primary_sources() {
        assert_eq!(primary_platform("vivian"), "bilibili");
        assert_eq!(primary_platform("Nana"), "xiaohongshu");
        assert!(source_guidance("nana").contains("小红书"));
    }
    #[test]
    fn actual_reading_wins_over_snippets_and_primary_wins_ties() {
        let sources = merge_sources(
            vec![
                source("primary-unread", false),
                source("primary-read", true),
            ],
            vec![source("fallback-read", true), source("primary-read", true)],
            3,
        );
        assert_eq!(
            sources.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            vec!["primary-read", "fallback-read", "primary-unread"]
        );
        assert!(!has_reading(&source("x", false)));
        assert!(has_reading(
            &merge_sources(vec![source("x", false)], vec![source("x", true)], 1)[0]
        ));
        assert!(merge_sources(vec![source("x", true)], vec![], 0).is_empty());
    }
}
