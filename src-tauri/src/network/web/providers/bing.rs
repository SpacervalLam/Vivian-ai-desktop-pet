//! Bing Search API v7 provider —— 国内直连可用（Azure 免费 1000 次/月）。
//!
//! 特点：
//! - `available()` 要求 `api_key` 非空（廉价本地检查）
//! - 支持市场代码（mkt）与分页偏移（offset）

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
const BING_ID: &str = "bing";

/// Bing 供应商
pub struct BingProvider {
    api_key: String,
    mkt: String,
    offset: u32,
    timeout: std::time::Duration,
    proxy_url: Option<String>,
}

/// 工厂：从配置快照构建（缝隙每次搜索执行时调用）
pub fn bing_factory(
    config: Option<&WebSearchConfig>,
    proxy_url: Option<&str>,
) -> Arc<dyn WebSearchProvider> {
    let (api_key, mkt, offset, timeout) = match config {
        Some(c) => (
            c.bing.api_key.clone(),
            if c.bing.mkt.is_empty() { "zh-CN".to_string() } else { c.bing.mkt.clone() },
            c.bing.offset,
            c.timeout_secs,
        ),
        None => (String::new(), "zh-CN".to_string(), 0, 0),
    };
    Arc::new(BingProvider {
        api_key,
        mkt,
        offset,
        timeout: timeout_from_secs(timeout),
        proxy_url: proxy_url.map(|p| p.to_string()),
    })
}

#[async_trait]
impl WebSearchProvider for BingProvider {
    fn id(&self) -> &'static str {
        BING_ID
    }

    /// api_key 未配置即不可用（廉价本地检查，不发网络请求）
    fn available(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let max = request.max_results.unwrap_or(5);
        let client = build_search_client(self.timeout, None, self.proxy_url.as_deref());

        let count = max.min(50);
        let resp = client
            .get("https://api.bing.microsoft.com/v7.0/search")
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
            .query(&[
                ("q", request.query.as_str()),
                ("mkt", self.mkt.as_str()),
                ("count", &count.to_string()),
                ("offset", &self.offset.to_string()),
            ])
            .send()
            .await
            .map_err(|e| WebError::provider_error(BING_ID, format!("Bing 请求失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(WebError::provider_error(
                BING_ID,
                format!("Bing 状态异常: {}", resp.status()),
            ));
        }
        let v: Value = resp.json().await.map_err(|e| {
            WebError::provider_error(BING_ID, format!("Bing JSON 解析失败: {e}"))
        })?;

        Ok(WebSearchResult {
            content: None,
            sources: parse_bing(&v, max),
            truncated: false,
        })
    }
}

/// 解析 Bing JSON 响应
fn parse_bing(v: &Value, limit: usize) -> Vec<WebSearchSource> {
    let arr = v
        .get("webPages")
        .and_then(|w| w.get("value"))
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
        s.title = non_empty(item.get("name").and_then(|t| t.as_str()));
        s.snippet = non_empty(item.get("snippet").and_then(|c| c.as_str()));
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
        let p = bing_factory(None, None);
        assert_eq!(p.id(), "bing");
        assert!(!p.available());

        let mut cfg = WebSearchConfig::default();
        cfg.bing.api_key = "xxx".into();
        let p = bing_factory(Some(&cfg), None);
        assert!(p.available());
    }

    #[test]
    fn test_parse_bing() {
        let v: Value = serde_json::from_str(
            r#"{"webPages":{"value":[{"name":"N","url":"https://a.com","snippet":"S"}]}}"#,
        )
        .unwrap();
        let sources = parse_bing(&v, 5);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].title.as_deref(), Some("N"));
        assert_eq!(sources[0].snippet.as_deref(), Some("S"));
    }
}
