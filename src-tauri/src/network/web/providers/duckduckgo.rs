//! DuckDuckGo provider —— HTML / Lite 双端点爬取，零配置兜底引擎。
//!
//! 特点：
//! - `available()` 恒为 `true`：零配置、无凭据，是缝隙的默认兜底供应商
//! - HTML 端点无结果时回退 Lite 端点（Lite 失败不掩盖「HTML 成功但无匹配」的事实）
//! - 请求失败（网络 / HTTP 状态 / 读体）返回结构化 `WebError`，不再吞成空结果

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::WebSearchConfig;
use crate::network::web::providers::util::{
    annotate_sources, build_search_client, strip_html_tags, timeout_from_secs, USER_AGENT,
};
use crate::network::web::types::{WebError, WebSearchRequest, WebSearchResult, WebSearchSource};
use crate::network::web::WebSearchProvider;

/// 稳定注册 id
const DUCKDUCKGO_ID: &str = "duckduckgo";

// DuckDuckGo Lite 搜索结果正则
static LITE_LINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<a[^>]*rel="nofollow"[^>]*href="([^"]*)"[^>]*>\s*(.*?)\s*</a>"#).unwrap()
});

static LITE_SNIPPET_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<td[^>]*class="[^"]*snippet[^"]*"[^>]*>(.*?)</td>"#).unwrap());

/// DuckDuckGo 供应商
pub struct DuckDuckGoProvider {
    timeout: std::time::Duration,
    /// 语言偏好（如 "zh-CN"），None 时用默认多语言头
    language: Option<String>,
    proxy_url: Option<String>,
}

/// 工厂：从配置快照构建（缝隙每次搜索执行时调用，配置变更无需重注册）
pub fn duckduckgo_factory(
    config: Option<&WebSearchConfig>,
    proxy_url: Option<&str>,
) -> Arc<dyn WebSearchProvider> {
    Arc::new(DuckDuckGoProvider {
        timeout: timeout_from_secs(config.map(|c| c.timeout_secs).unwrap_or(0)),
        language: config
            .and_then(|c| c.language.clone())
            .filter(|l| !l.is_empty()),
        proxy_url: proxy_url.map(|p| p.to_string()),
    })
}

impl DuckDuckGoProvider {
    /// Accept-Language 头：配置了语言偏好则优先，否则默认多语言
    fn accept_language(&self) -> String {
        match &self.language {
            Some(lang) => format!("{lang},{lang};q=0.9,en;q=0.8"),
            None => "zh-CN,zh;q=0.9,en;q=0.8".to_string(),
        }
    }

    /// HTML 端点搜索
    async fn ddg_html(
        &self,
        request: &WebSearchRequest,
        max: usize,
    ) -> Result<Vec<WebSearchSource>, WebError> {
        let client = build_search_client(self.timeout, Some(USER_AGENT), self.proxy_url.as_deref());
        let resp = client
            .post("https://html.duckduckgo.com/html/")
            .header("Accept", "text/html,application/xhtml+xml")
            .header(
                "Accept-Language",
                request
                    .language
                    .clone()
                    .unwrap_or_else(|| self.accept_language()),
            )
            .form(&ddg_form(request))
            .send()
            .await
            .map_err(|e| {
                WebError::provider_error(DUCKDUCKGO_ID, format!("DDG HTML 请求失败: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(WebError::provider_error(
                DUCKDUCKGO_ID,
                format!("DDG HTML 状态异常: {}", resp.status()),
            ));
        }
        let html = resp.text().await.map_err(|e| {
            WebError::provider_error(DUCKDUCKGO_ID, format!("DDG HTML 读体失败: {e}"))
        })?;
        detect_challenge(&html)?;
        check_parsed_page(&html, parse_html(&html, max))
    }

    /// Lite 端点搜索
    async fn ddg_lite(
        &self,
        request: &WebSearchRequest,
        max: usize,
    ) -> Result<Vec<WebSearchSource>, WebError> {
        let client = build_search_client(self.timeout, Some(USER_AGENT), self.proxy_url.as_deref());
        let resp = client
            .post("https://lite.duckduckgo.com/lite/")
            .form(&ddg_form(request))
            .send()
            .await
            .map_err(|e| {
                WebError::provider_error(DUCKDUCKGO_ID, format!("DDG Lite 请求失败: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(WebError::provider_error(
                DUCKDUCKGO_ID,
                format!("DDG Lite 状态异常: {}", resp.status()),
            ));
        }
        let html = resp.text().await.map_err(|e| {
            WebError::provider_error(DUCKDUCKGO_ID, format!("DDG Lite 读体失败: {e}"))
        })?;
        detect_challenge(&html)?;
        check_parsed_page(&html, parse_lite(&html, max))
    }
}

#[async_trait]
impl WebSearchProvider for DuckDuckGoProvider {
    fn id(&self) -> &'static str {
        DUCKDUCKGO_ID
    }

    /// 零配置、无凭据：始终可用（廉价本地检查，不发网络请求）
    fn available(&self) -> bool {
        true
    }

    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let max = request.max_results.unwrap_or(5);

        let mut sources = match self.ddg_html(request, max).await {
            Ok(s) => s,
            Err(_) => {
                return self
                    .ddg_lite(request, max)
                    .await
                    .map(|sources| WebSearchResult {
                        content: None,
                        sources,
                        truncated: false,
                        ..Default::default()
                    })
            }
        };
        if sources.is_empty() {
            // HTML 无结果 → Lite 回退；Lite 失败不掩盖「HTML 成功但无匹配」
            match self.ddg_lite(request, max).await {
                Ok(lite) => sources = lite,
                Err(e) => tracing::warn!("[WebSearch:{DUCKDUCKGO_ID}] Lite 回退失败: {e}"),
            }
        }
        sources.truncate(max);

        Ok(WebSearchResult {
            content: None,
            sources,
            truncated: false,
            ..Default::default()
        })
    }
}

// ============================================================================
// HTML 解析（模块私有）
// ============================================================================

/// 解析 DuckDuckGo HTML 搜索结果
fn parse_html(html: &str, limit: usize) -> Vec<WebSearchSource> {
    let document = scraper::Html::parse_document(html);
    let container = scraper::Selector::parse(".result").expect("selector");
    let anchor = scraper::Selector::parse("a.result__a").expect("selector");
    let snippet = scraper::Selector::parse(".result__snippet").expect("selector");
    let mut sources = vec![];
    for result in document.select(&container) {
        if let Some(link) = result.select(&anchor).next() {
            if let Some(href) = link.value().attr("href") {
                let url = decode_ddg_url(href);
                if !url.starts_with("http") {
                    continue;
                }
                let mut source = WebSearchSource::new(url);
                source.title = Some(link.text().collect::<String>());
                source.snippet = result
                    .select(&snippet)
                    .next()
                    .map(|s| s.text().collect::<String>());
                sources.push(source);
            }
        }
        if sources.len() >= limit {
            break;
        }
    }
    // Some lightweight result pages omit containers; keep their actual result anchors.
    if sources.is_empty() {
        for link in document.select(&anchor).take(limit) {
            if let Some(href) = link.value().attr("href") {
                let url = decode_ddg_url(href);
                if !url.starts_with("http") {
                    continue;
                }
                let mut source = WebSearchSource::new(url);
                source.title = Some(link.text().collect::<String>());
                for sibling in link.next_siblings() {
                    let Some(element) = scraper::ElementRef::wrap(sibling) else {
                        continue;
                    };
                    let classes = element.value().attr("class").unwrap_or("");
                    if classes.split_whitespace().any(|c| c == "result__a") {
                        break;
                    }
                    if classes.split_whitespace().any(|c| c == "result__snippet") {
                        source.snippet = Some(element.text().collect::<String>());
                        break;
                    }
                }
                sources.push(source);
            }
        }
    }
    annotate_sources(sources)
}

/// 解析 DuckDuckGo Lite 搜索结果
fn parse_lite(html: &str, limit: usize) -> Vec<WebSearchSource> {
    let mut results = Vec::new();

    let links: Vec<_> = LITE_LINK_RE.captures_iter(html).collect();
    for cap in links.iter().take(limit) {
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_html = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        if href.is_empty() {
            continue;
        }

        let decoded_url = decode_ddg_url(href);
        if !decoded_url.starts_with("http://") && !decoded_url.starts_with("https://") {
            continue;
        }
        let mut s = WebSearchSource::new(decoded_url);
        let title = strip_html_tags(title_html);
        if !title.is_empty() {
            s.title = Some(title);
        }
        results.push(s);
    }

    // 填充摘要
    let snippets: Vec<_> = LITE_SNIPPET_RE.captures_iter(html).collect();
    for (i, cap) in snippets.iter().take(limit).enumerate() {
        if i < results.len() {
            let snippet_html = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if !snippet_html.is_empty() {
                results[i].snippet = Some(strip_html_tags(snippet_html));
            }
        }
    }

    annotate_sources(results)
}

/// 解析 DuckDuckGo 跳转链接
///
/// DDG 的搜索结果链接通常形如：
/// `//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc`
/// 本函数提取并解码 `uddg=` 参数中的真实 URL。
fn decode_ddg_url(href: &str) -> String {
    let href = href.trim();

    // DDG 跳转格式：//duckduckgo.com/l/?uddg=ENCODED_URL
    if let Some(pos) = href.find("uddg=") {
        let encoded = &href[pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        return percent_decode(&encoded[..end]);
    }

    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }

    // 去掉前导 //
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }

    href.to_string()
}

/// 百分号解码
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                result.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            result.push(b' ');
            i += 1;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

/// 十六进制数字转 u8
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_ddg_url() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc";
        assert_eq!(decode_ddg_url(href), "https://example.com");

        let href2 = "https://example.com/page";
        assert_eq!(decode_ddg_url(href2), "https://example.com/page");

        let href3 = "//example.com/path";
        assert_eq!(decode_ddg_url(href3), "https://example.com/path");
        assert_eq!(
            decode_ddg_url("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_parse_html_empty() {
        let results = parse_html("<html></html>", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_html_with_results() {
        let html = r##"
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com">Example</a>
        <a class="result__snippet" href="#">This is a snippet</a>
        "##;
        let results = parse_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title.as_deref(), Some("Example"));
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet.as_deref(), Some("This is a snippet"));
        assert!(!results[0].source_tier.is_empty());
    }

    #[test]
    fn test_parse_lite() {
        let html = r#"
        <a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com">Example</a>
        <td class="result-snippet">snip</td>
        "#;
        let results = parse_lite(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet.as_deref(), Some("snip"));
    }

    #[test]
    fn test_factory_available() {
        let p = duckduckgo_factory(None, None);
        assert_eq!(p.id(), "duckduckgo");
        assert!(p.available());
    }
}

fn ddg_form(r: &WebSearchRequest) -> Vec<(String, String)> {
    let mut params = vec![("q".into(), r.query.clone())];
    if let Some(days) = r.recency_days {
        let range = if days <= 1 {
            "d"
        } else if days <= 7 {
            "w"
        } else if days <= 31 {
            "m"
        } else {
            "y"
        };
        params.push(("df".into(), range.into()));
    }
    if let Some(country) = &r.country {
        params.push((
            "kl".into(),
            format!(
                "{}-{}",
                country.to_ascii_lowercase(),
                r.language
                    .as_deref()
                    .unwrap_or("en")
                    .split('-')
                    .next()
                    .unwrap_or("en")
            ),
        ));
    }
    params
}
fn detect_challenge(html: &str) -> Result<(), WebError> {
    if html.contains("anomaly.js")
        || html.contains("anomaly-modal")
        || html.contains("challenge-form")
    {
        return Err(WebError::provider_error(
            DUCKDUCKGO_ID,
            "搜索引擎返回反机器人验证页，不能视为无匹配",
        ));
    }
    Ok(())
}

fn check_parsed_page(
    html: &str,
    sources: Vec<WebSearchSource>,
) -> Result<Vec<WebSearchSource>, WebError> {
    if sources.is_empty()
        && ![
            "no-results",
            "No results",
            "No more results",
            "result--no-result",
        ]
        .iter()
        .any(|marker| html.contains(marker))
    {
        return Err(WebError::provider_error(
            DUCKDUCKGO_ID,
            "未识别到搜索结果或无结果标记；可能页面结构变化，不能声称没有匹配",
        ));
    }
    Ok(sources)
}
