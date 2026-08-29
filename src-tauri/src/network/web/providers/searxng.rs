//! SearXNG provider —— 自部署元搜索引擎（JSON API）。
//!
//! 特点：
//! - 聚合多源结果，国内可用（自部署 / 公共实例）
//! - `available()` 要求 `base_url` 非空（廉价本地检查）
//! - 支持可选 Bearer auth_token 与语言偏好

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchConfig;
use crate::network::web::providers::util::{
    annotate_sources, build_search_client, timeout_from_secs,
};
use crate::network::web::types::{
    WebError, WebSearchRequest, WebSearchResult, WebSearchSource,
};
use crate::network::web::WebSearchProvider;

/// 稳定注册 id
const SEARXNG_ID: &str = "searxng";

/// SearXNG 供应商
pub struct SearXngProvider {
    base_url: String,
    auth_token: String,
    /// 语言偏好（如 "zh-CN"），空表示不限定
    language: Option<String>,
    timeout: std::time::Duration,
    proxy_url: Option<String>,
}

/// 工厂：从配置快照构建（缝隙每次搜索执行时调用）
pub fn searxng_factory(
    config: Option<&WebSearchConfig>,
    proxy_url: Option<&str>,
) -> Arc<dyn WebSearchProvider> {
    let (base_url, auth_token, language, timeout) = match config {
        Some(c) => (
            c.searxng.base_url.clone(),
            c.searxng.auth_token.clone(),
            c.language.clone(),
            c.timeout_secs,
        ),
        None => Default::default(),
    };
    Arc::new(SearXngProvider {
        base_url,
        auth_token,
        language: language.filter(|l| !l.is_empty()),
        timeout: timeout_from_secs(timeout),
        proxy_url: proxy_url.map(|p| p.to_string()),
    })
}

#[async_trait]
impl WebSearchProvider for SearXngProvider {
    fn id(&self) -> &'static str {
        SEARXNG_ID
    }

    /// base_url 未配置即不可用（廉价本地检查，不发网络请求）
    fn available(&self) -> bool {
        !self.base_url.is_empty()
    }

    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let max = request.max_results.unwrap_or(5);
        let client = build_search_client(self.timeout, None, self.proxy_url.as_deref());

        let mut req = client
            .get(format!("{}/search", self.base_url.trim_end_matches('/')))
            .query(&[
                ("q", request.query.as_str()),
                ("format", "json"),
                ("pageno", "1"),
            ]);
        if let Some(lang) = &self.language {
            req = req.query(&[("language", lang.as_str())]);
        }
        if !self.auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.auth_token));
        }

        let resp = req.send().await.map_err(|e| {
            WebError::provider_error(SEARXNG_ID, format!("SearXNG 请求失败: {e}"))
        })?;
        if !resp.status().is_success() {
            return Err(WebError::provider_error(
                SEARXNG_ID,
                format!("SearXNG 状态异常: {}", resp.status()),
            ));
        }
        let v: Value = resp.json().await.map_err(|e| {
            WebError::provider_error(SEARXNG_ID, format!("SearXNG JSON 解析失败: {e}"))
        })?;

        Ok(WebSearchResult {
            content: None,
            sources: parse_searxng(&v, max),
            truncated: false,
        })
    }
}

/// 解析 SearXNG JSON 响应（结构异常时宽松返回空，只有解析失败才算错误）
fn parse_searxng(v: &Value, limit: usize) -> Vec<WebSearchSource> {
    let arr = v
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut sources = Vec::new();
    for item in arr.iter().take(limit) {
        let url = item
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        let mut s = WebSearchSource::new(url);
        s.title = non_empty(item.get("title").and_then(|t| t.as_str()));
        s.snippet = non_empty(item.get("content").and_then(|c| c.as_str()));
        sources.push(s);
    }
    annotate_sources(sources)
}

/// 空串归一为 None（诚实字段：缺失即缺失）
fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|t| !t.is_empty()).map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factory_availability() {
        // 无配置 → 不可用
        let p = searxng_factory(None, None);
        assert_eq!(p.id(), "searxng");
        assert!(!p.available());

        // 有 base_url → 可用
        let cfg = WebSearchConfig::default();
        let p = searxng_factory(Some(&cfg), None);
        assert!(!p.available()); // 默认 base_url 为空

        let mut cfg = WebSearchConfig::default();
        cfg.searxng.base_url = "http://localhost:8080".into();
        let p = searxng_factory(Some(&cfg), None);
        assert!(p.available());
    }

    #[test]
    fn test_parse_searxng() {
        let v: Value = serde_json::from_str(
            r#"{"results":[{"title":"T","url":"https://a.com","content":"C"},
                          {"title":"","url":"https://b.com"}]}"#,
        )
        .unwrap();
        let sources = parse_searxng(&v, 5);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].title.as_deref(), Some("T"));
        assert_eq!(sources[1].title, None); // 空串归一为 None
        assert!(!sources[0].source_tier.is_empty());
    }
}
