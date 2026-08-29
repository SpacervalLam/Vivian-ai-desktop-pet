//! DeepSeek 官方原生搜索 provider —— Anthropic 兼容 Messages API + `web_search_20250305` server tool。
//!
//! 参考 deepseek-harness 的 `web-search-deepseek` provider 设计：
//! - **一次搜索 = 一次模型调用**：向 DeepSeek 的 Anthropic 兼容 Messages API
//!   发送带原生 web_search server tool 的请求，DeepSeek 服务端执行搜索并在
//!   响应中返回结构化 `web_search_tool_result` 块
//! - **snippet 的真实来源**：`web_search_result` 项自带 url/title/page_age 但无内联
//!   摘要，摘录藏在 `text` 块的 `citations[].cited_text` 里，按 url 关联（首次出现优先）
//! - **没有 result 块即报错**（诚实失败），不做散文爬取兜底
//! - 双认证头（`x-api-key` + `Authorization: Bearer`）：官方 DeepSeek 用前者，
//!   Anthropic 兼容代理用后者，两者都发让任一端点可用
//! - key 复用链：`web_search.deepseek.api_key` → 主对话 `ai` 配置的 DeepSeek key
//!   （provider 为 deepseek 时）→ 不可用。base_url 独立于主对话端点（协议不同）
//! - 重定向不跟随（`redirect: none`），3xx 按错误处理

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchConfig;
use crate::network::web::providers::util::annotate_sources;
use crate::network::web::types::{
    WebError, WebSearchRequest, WebSearchResult, WebSearchSource,
};
use crate::network::web::WebSearchProvider;

/// 稳定注册 id
const DEEPSEEK_ID: &str = "deepseek";

/// 默认 `anthropic-version` 头
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 归一化后的运行参数（含 key/model 复用解析结果）
#[derive(Debug, Clone)]
struct ResolvedOptions {
    api_key: String,
    base_url: String,
    model: String,
    max_uses: u32,
    max_tokens: u32,
    timeout: std::time::Duration,
}

/// DeepSeek 供应商
pub struct DeepSeekProvider {
    options: ResolvedOptions,
    /// 专用 HTTP 客户端（不跟随重定向，带超时与可选代理）
    client: reqwest::Client,
}

/// 工厂：从配置快照构建。
///
/// key / model 复用解析在此完成（等价于 dsh 插件层的 resolveOptions）：
/// `web_search.deepseek.api_key` 为空且主对话 provider 为 deepseek 时，
/// 复用主对话的 key 与模型名。每次搜索执行时调用，配置变更无需重注册。
pub fn deepseek_factory(
    config: Option<&WebSearchConfig>,
    proxy_url: Option<&str>,
) -> Arc<dyn WebSearchProvider> {
    let options = resolve_options(config);
    // 重定向不跟随：3xx 按非 2xx 错误处理（对齐 dsh 的 redirect: 'error'）
    let mut builder = reqwest::Client::builder()
        .timeout(options.timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(url) = proxy_url {
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            builder = builder.proxy(proxy);
        }
    }
    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
    Arc::new(DeepSeekProvider { options, client })
}

/// 解析运行参数（含主对话 key/model 复用）
fn resolve_options(config: Option<&WebSearchConfig>) -> ResolvedOptions {
    let mut cfg = config
        .map(|c| c.deepseek.clone())
        .unwrap_or_default();

    // key 复用：主对话 provider 为 deepseek 且未单独配置搜索 key
    // （base_url 不复用：搜索走 Anthropic 兼容端点，主对话走 chat-completions）
    if cfg.api_key.is_empty() {
        if let Some(handle) = crate::network::web::current_app_handle() {
            use tauri::Manager;
            let ai = handle
                .state::<Arc<crate::state::AppState>>()
                .config
                .read()
                .get_all()
                .ai;
            if ai.provider.eq_ignore_ascii_case("deepseek") {
                if let Some(key) = ai.api_key {
                    if !key.is_empty() {
                        cfg.api_key = key;
                        // 模型也复用主对话的（同一家厂商的凭据应一致）
                        if cfg.model.is_empty() && !ai.model.is_empty() {
                            cfg.model = ai.model;
                        }
                    }
                }
            }
        }
    }

    // base_url / model 默认值
    if cfg.base_url.is_empty() {
        cfg.base_url = "https://api.deepseek.com/anthropic/v1".to_string();
    }
    if cfg.model.is_empty() {
        cfg.model = "deepseek-chat".to_string();
    }
    let timeout_secs = if cfg.timeout_secs == 0 { 60 } else { cfg.timeout_secs };

    ResolvedOptions {
        api_key: cfg.api_key,
        base_url: cfg.base_url,
        model: cfg.model,
        max_uses: if cfg.max_uses == 0 { 5 } else { cfg.max_uses },
        max_tokens: if cfg.max_tokens == 0 { 4096 } else { cfg.max_tokens },
        timeout: std::time::Duration::from_secs(timeout_secs),
    }
}

impl DeepSeekProvider {
    /// 注入参数已由工厂解析；此处组装请求体并发送
    async fn dispatch(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let endpoint = format!("{}/messages", self.options.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.options.model,
            "max_tokens": self.options.max_tokens,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": format!("Perform a web search for the query: {}", request.query)
                }]
            }],
            "tools": [{
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": self.options.max_uses
            }]
        });

        let resp = self.client
            .post(&endpoint)
            // 官方 DeepSeek 用 x-api-key；Anthropic 兼容代理用 Bearer —— 两者都发
            .header("x-api-key", &self.options.api_key)
            .header("authorization", format!("Bearer {}", self.options.api_key))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                WebError::provider_error(DEEPSEEK_ID, format!("DeepSeek 搜索请求失败: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            // 尽量提取 provider 错误详情（网关 5xx/429 常是非 JSON 体，失败则只用状态码）
            let detail = resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| extract_error_message(&v))
                .unwrap_or_else(|| format!("DeepSeek API 错误 (HTTP {status})"));
            return Err(WebError::provider_error(DEEPSEEK_ID, detail));
        }

        let payload = resp
            .json::<Value>()
            .await
            .map_err(|e| {
                WebError::provider_error(
                    DEEPSEEK_ID,
                    format!("DeepSeek 返回无法解析的响应体: {e}"),
                )
            })?;

        map_anthropic_response(&payload)
    }
}

#[async_trait]
impl WebSearchProvider for DeepSeekProvider {
    fn id(&self) -> &'static str {
        DEEPSEEK_ID
    }

    /// api_key 解析后非空即可用（廉价本地检查，不发网络请求）
    fn available(&self) -> bool {
        !self.options.api_key.is_empty()
    }

    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        // 一次操作一份配置快照：调用入口检查取消/可用性后直接派发
        self.dispatch(request).await
    }
}

// ============================================================================
// 响应归一化（dsh mapAnthropicResponse 的 Rust 移植）
// ============================================================================

/// 从错误响应体提取 `error.message` / `error` / `message` 字段
fn extract_error_message(v: &Value) -> Option<String> {
    let err = v.get("error");
    let msg = err
        .and_then(|e| {
            e.as_str()
                .map(|s| s.to_string())
                .or_else(|| e.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()))
        })
        .or_else(|| v.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()));
    msg.filter(|m| !m.is_empty())
}

/// 从所有 `text` 块的 `citations[]` 构建 `url → cited_text` 映射。
///
/// Anthropic `web_search_result` 项自带 url/title/page_age 但**没有内联摘要**；
/// 摘录以引用形式出现在 `text` 块里，按 url 关联（首次出现优先）。
fn citation_snippets(content: &[Value]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        if let Some(cites) = block.get("citations").and_then(|c| c.as_array()) {
            for cite in cites {
                let url = cite.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let text = cite.get("cited_text").and_then(|t| t.as_str()).unwrap_or("");
                if !url.is_empty() && !text.is_empty() {
                    map.entry(url.to_string()).or_insert_with(|| text.to_string());
                }
            }
        }
    }
    map
}

/// 将 Anthropic Messages 响应归一化为搜索结果。
///
/// 走 `web_search_tool_result` 块提取可引用的 `web_search_result` 项，
/// 按 url 关联 `citations` 摘录作为 snippet，并按 url 去重
/// （`max_uses > 1` 时同一 URL 可能跨轮出现）。
///
/// **没有 result 块即报错**：原生搜索未触发时宁可失败也不做散文爬取兜底，
/// 保持结果可追溯。
fn map_anthropic_response(response: &Value) -> Result<WebSearchResult, WebError> {
    let content = response
        .get("content")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let result_blocks: Vec<&Value> = content
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("web_search_tool_result"))
        .collect();

    if result_blocks.is_empty() {
        return Err(WebError::provider_error(
            DEEPSEEK_ID,
            "DeepSeek 未返回 web_search_tool_result 块（请求可能未触发原生联网搜索）",
        ));
    }

    let snippets = citation_snippets(&content);
    let mut seen = std::collections::HashSet::new();
    let mut sources: Vec<WebSearchSource> = Vec::new();

    for block in &result_blocks {
        let items = block
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        for item in &items {
            if item.get("type").and_then(|t| t.as_str()) != Some("web_search_result") {
                continue;
            }
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() || !seen.insert(url.to_string()) {
                continue;
            }
            let mut s = WebSearchSource::new(url);
            s.title = item
                .get("title")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string());
            s.snippet = snippets.get(url).cloned().filter(|t| !t.is_empty());
            s.published_at = item
                .get("page_age")
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string());
            sources.push(s);
        }
    }

    // 缝隙负责最终 maxResults 截断，此处 truncated 恒为 false（对齐 dsh）
    Ok(WebSearchResult {
        // 不返回模型生成的 text 总结（对齐 dsh：sources + 引用摘录已足够，
        // 模型总结面向 API 消费格式不稳定）
        content: None,
        sources: annotate_sources(sources),
        truncated: false,
    })
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_citation_snippets_first_wins() {
        let content = vec![
            serde_json::json!({
                "type": "text",
                "text": "answer",
                "citations": [
                    {"url": "https://a.com", "cited_text": "first"},
                    {"url": "https://a.com", "cited_text": "second"},
                    {"url": "https://b.com", "cited_text": "b text"},
                ]
            }),
            serde_json::json!({"type": "web_search_tool_result", "content": []}),
        ];
        let map = citation_snippets(&content);
        assert_eq!(map.get("https://a.com").map(|s| s.as_str()), Some("first"));
        assert_eq!(map.get("https://b.com").map(|s| s.as_str()), Some("b text"));
    }

    #[test]
    fn test_map_response_success() {
        let response = serde_json::json!({
            "content": [
                {"type": "text", "text": "summary",
                 "citations": [{"url": "https://a.com", "cited_text": "cited snippet"}]},
                {"type": "web_search_tool_result", "content": [
                    {"type": "web_search_result", "url": "https://a.com", "title": "A", "page_age": "2026-01-01"},
                    {"type": "web_search_result", "url": "https://b.com"},
                    {"type": "web_search_result", "url": "https://a.com"},  // 重复 url → 去重
                ]},
            ]
        });
        let result = map_anthropic_response(&response).expect("should succeed");
        assert_eq!(result.sources.len(), 2);
        assert_eq!(result.sources[0].url, "https://a.com");
        assert_eq!(result.sources[0].title.as_deref(), Some("A"));
        assert_eq!(result.sources[0].snippet.as_deref(), Some("cited snippet"));
        assert_eq!(result.sources[0].published_at.as_deref(), Some("2026-01-01"));
        assert!(result.sources[0].snippet.is_some());
        // b.com 无 citation → snippet 为 None（诚实字段）
        assert_eq!(result.sources[1].url, "https://b.com");
        assert!(result.sources[1].snippet.is_none());
        assert!(!result.truncated);
        assert!(result.content.is_none());
    }

    #[test]
    fn test_map_response_no_result_blocks_is_error() {
        let response = serde_json::json!({
            "content": [{"type": "text", "text": "just text, no search"}]
        });
        let err = map_anthropic_response(&response).unwrap_err();
        assert_eq!(err.code, crate::network::web::WebErrorCode::ProviderError);
        assert_eq!(err.provider.as_deref(), Some("deepseek"));
    }

    #[test]
    fn test_extract_error_message() {
        assert_eq!(
            extract_error_message(&serde_json::json!({"error": {"message": "bad key"}})),
            Some("bad key".to_string())
        );
        assert_eq!(
            extract_error_message(&serde_json::json!({"error": "plain string"})),
            Some("plain string".to_string())
        );
        assert_eq!(
            extract_error_message(&serde_json::json!({"message": "m"})),
            Some("m".to_string())
        );
        assert_eq!(extract_error_message(&serde_json::json!({})), None);
        assert_eq!(
            extract_error_message(&serde_json::json!({"error": {"message": ""}})),
            None
        );
    }

    #[test]
    fn test_factory_unavailable_without_key() {
        // 无 AppHandle（测试环境）+ 空 api_key → 不可用
        let p = deepseek_factory(None, None);
        assert_eq!(p.id(), "deepseek");
        assert!(!p.available());
    }
}
