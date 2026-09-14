//! Tavily provider —— 专为 LLM Agent 设计的搜索 API。
//!
//! 特点：
//! - `available()` 要求 `api_key` 非空（廉价本地检查）
//! - 响应支持 `published_date`，映射到 `published_at`
//! - `include_answer` 保持关闭：当前无答案型输出，`content` 恒为 `None`
//!   （字段已在缝隙词汇中预留，未来答案型 provider 直接填充）

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
const TAVILY_ID: &str = "tavily";

/// Tavily 供应商
pub struct TavilyProvider {
    api_key: String,
    include_raw_content: bool,
    search_depth: String,
    timeout: std::time::Duration,
    proxy_url: Option<String>,
}

/// 工厂：从配置快照构建（缝隙每次搜索执行时调用）
pub fn tavily_factory(
    config: Option<&WebSearchConfig>,
    proxy_url: Option<&str>,
) -> Arc<dyn WebSearchProvider> {
    let (api_key, include_raw_content, search_depth, timeout) = match config {
        Some(c) => (
            c.tavily.api_key.clone(),
            c.tavily.include_raw_content,
            c.tavily.search_depth.clone(),
            c.timeout_secs,
        ),
        None => Default::default(),
    };
    Arc::new(TavilyProvider {
        api_key,
        include_raw_content,
        search_depth,
        timeout: timeout_from_secs(timeout),
        proxy_url: proxy_url.map(|p| p.to_string()),
    })
}

#[async_trait]
impl WebSearchProvider for TavilyProvider {
    fn id(&self) -> &'static str {
        TAVILY_ID
    }

    /// api_key 未配置即不可用（廉价本地检查，不发网络请求）
    fn available(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let max = request.max_results.unwrap_or(5);
        let client = build_search_client(self.timeout, None, self.proxy_url.as_deref());

        let body = serde_json::json!({
            "api_key": self.api_key,
            "query": request.query,
            "max_results": max,
            "include_answer": false,
            "include_raw_content": self.include_raw_content,
            "search_depth": self.search_depth,
        });

        let resp = client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                WebError::provider_error(TAVILY_ID, format!("Tavily 请求失败: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(WebError::provider_error(
                TAVILY_ID,
                format!("Tavily 状态异常: {}", resp.status()),
            ));
        }
        let v: Value = resp.json().await.map_err(|e| {
            WebError::provider_error(TAVILY_ID, format!("Tavily JSON 解析失败: {e}"))
        })?;

        Ok(WebSearchResult {
            content: None,
            sources: parse_tavily(&v),
            truncated: false,
        })
    }
}

/// 解析 Tavily JSON 响应
fn parse_tavily(v: &Value) -> Vec<WebSearchSource> {
    let arr = v
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut sources = Vec::new();
    for item in &arr {
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
        s.published_at = non_empty(item.get("published_date").and_then(|p| p.as_str()));
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
        let p = tavily_factory(None, None);
        assert_eq!(p.id(), "tavily");
        assert!(!p.available());

        let mut cfg = WebSearchConfig::default();
        cfg.tavily.api_key = "tvly-xxx".into();
        let p = tavily_factory(Some(&cfg), None);
        assert!(p.available());
    }

    #[test]
    fn test_parse_tavily() {
        let v: Value = serde_json::from_str(
            r#"{"results":[{"title":"T","url":"https://a.com","content":"C","published_date":"2026-01-01"},
                          {"title":"X","url":"https://b.com"}]}"#,
        )
        .unwrap();
        let sources = parse_tavily(&v);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].published_at.as_deref(), Some("2026-01-01"));
        assert_eq!(sources[1].published_at, None);
    }
}
