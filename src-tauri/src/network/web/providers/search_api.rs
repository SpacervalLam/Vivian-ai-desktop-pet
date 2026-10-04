//! Explicit, credential-gated search adapters. All responses share evidence metadata.
use super::util::{annotate_sources, build_search_client};
use crate::config::manager::SearchApiConfig;
use crate::config::WebSearchConfig;
use crate::network::web::{
    WebError, WebSearchProvider, WebSearchRequest, WebSearchResult, WebSearchSource,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc, time::Duration};

struct SearchApi {
    id: &'static str,
    config: SearchApiConfig,
    client: reqwest::Client,
}
macro_rules! factory {
    ($name:ident, $id:literal, $field:ident, $url:literal) => {
        pub fn $name(
            c: Option<&WebSearchConfig>,
            proxy: Option<&str>,
        ) -> Arc<dyn WebSearchProvider> {
            let mut config = c.map(|c| c.$field.clone()).unwrap_or_default();
            if config.base_url.trim().is_empty() {
                config.base_url = $url.into();
            }
            Arc::new(SearchApi {
                id: $id,
                config,
                client: build_search_client(Duration::from_secs(50), None, proxy),
            })
        }
    };
}
factory!(exa_factory, "exa", exa, "https://api.exa.ai");
factory!(
    perplexity_factory,
    "perplexity",
    perplexity,
    "https://api.perplexity.ai"
);
factory!(
    openai_factory,
    "openai",
    openai,
    "https://api.openai.com/v1"
);
factory!(xai_factory, "xai", xai, "https://api.x.ai/v1");
factory!(
    anthropic_factory,
    "anthropic",
    anthropic,
    "https://api.anthropic.com/v1"
);

fn request_body(id: &str, model: &str, r: &WebSearchRequest) -> Result<Value, String> {
    let max = r.max_results.unwrap_or(10).clamp(1, 20);
    let after = r
        .recency_days
        .map(|days| chrono::Utc::now() - chrono::Duration::days(days as i64));
    let mut body = match id {
        "exa" => json!({"query":r.query,"type":"auto","numResults":max,
            "includeDomains":r.include_domains,"excludeDomains":r.exclude_domains,
            "contents":{"highlights":true,"text":{"maxCharacters":if r.research {20000} else {4000}}}}),
        "perplexity" => {
            if r.include_domains.len() + r.exclude_domains.len() > 20 {
                return Err("Perplexity supports at most 20 domain filters".into());
            }
            let domains: Vec<_> = r
                .include_domains
                .iter()
                .cloned()
                .chain(r.exclude_domains.iter().map(|d| format!("-{d}")))
                .collect();
            json!({"query":r.query,"max_results":max,"search_domain_filter":domains,"max_tokens_per_page":if r.research {4000} else {1000}})
        }
        "anthropic" => {
            let mut tool = json!({"type":"web_search_20250305","name":"web_search","max_uses":if r.research {3} else {1}});
            if !r.include_domains.is_empty() {
                tool["allowed_domains"] = json!(r.include_domains);
            } else if !r.exclude_domains.is_empty() {
                tool["blocked_domains"] = json!(r.exclude_domains);
            }
            json!({"model":model,"max_tokens":4096,"messages":[{"role":"user","content":format!("Search the web for: {}. Return sources and cited excerpts, not uncited guesses.",r.query)}],"tools":[tool]})
        }
        _ => {
            let mut filters = json!({});
            if id == "xai"
                && (r.include_domains.len() > 5
                    || r.exclude_domains.len() > 5
                    || (!r.include_domains.is_empty() && !r.exclude_domains.is_empty()))
            {
                return Err("xAI accepts at most 5 allowed OR excluded domains per request".into());
            }
            if !r.include_domains.is_empty() {
                filters["allowed_domains"] = json!(r.include_domains);
            }
            if !r.exclude_domains.is_empty() {
                filters[if id == "xai" {
                    "excluded_domains"
                } else {
                    "blocked_domains"
                }] = json!(r.exclude_domains);
            }
            let mut tool = json!({"type":"web_search"});
            if filters.as_object().is_some_and(|m| !m.is_empty()) {
                tool["filters"] = filters;
            }
            let mut b = json!({"model":model,"input":format!("Search the web for: {}. Cite source URLs and distinguish publication dates from retrieval dates. Desired sources: {max}. Recency preference in days: {:?}.",r.query,r.recency_days),"tools":[tool],"max_output_tokens":4096,"store":false});
            if id == "openai" {
                b["include"] = json!(["web_search_call.action.sources"]);
            }
            b
        }
    };
    if let Some(after) = after {
        if id == "exa" {
            body["startPublishedDate"] = json!(after.to_rfc3339());
        }
        if id == "perplexity" {
            body["search_after_date_filter"] = json!(after.format("%m/%d/%Y").to_string());
        }
    }
    Ok(body)
}

fn parse_response(id: &str, v: &Value) -> Result<WebSearchResult, WebError> {
    if id == "anthropic" {
        return super::deepseek::map_anthropic_response(v)
            .map_err(|e| WebError::provider_error(id, e.message));
    }
    let mut sources = vec![];
    let mut seen = HashSet::new();
    let mut push = |item: &Value| {
        let Some(url) = item["url"].as_str() else {
            return;
        };
        if crate::network::url_fetcher::validate_url(url).is_err() || !seen.insert(url.to_string())
        {
            return;
        }
        let mut source = WebSearchSource::new(url);
        source.title = item["title"].as_str().map(str::to_string);
        source.published_at = item["publishedDate"]
            .as_str()
            .or(item["date"].as_str())
            .map(str::to_string);
        source.raw_content = item["text"]
            .as_str()
            .map(|s| s.chars().take(20000).collect());
        source.snippet = item["snippet"]
            .as_str()
            .or(item["cited_text"].as_str())
            .map(str::to_string)
            .or_else(|| {
                item["highlights"].as_array().map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            });
        sources.push(source);
    };
    let mut answer = String::new();
    if matches!(id, "exa" | "perplexity") {
        let Some(results) = v["results"].as_array() else {
            return Err(WebError::provider_error(id, "Search API omitted results[]"));
        };
        for item in results {
            push(item);
        }
    } else {
        for item in v["output"].as_array().into_iter().flatten() {
            for source in item["action"]["sources"].as_array().into_iter().flatten() {
                push(source);
            }
            for block in item["content"].as_array().into_iter().flatten() {
                if block["type"] == "output_text" {
                    if let Some(text) = block["text"].as_str() {
                        answer.push_str(text);
                        answer.push('\n');
                    }
                    for citation in block["annotations"].as_array().into_iter().flatten() {
                        if citation["type"] == "url_citation" {
                            push(citation);
                        }
                    }
                }
            }
        }
        // A model answer alone is not proof that the search tool ran.
        if sources.is_empty() {
            return Err(WebError::provider_error(
                id,
                "Native search returned no traceable URL sources",
            ));
        }
    }
    Ok(WebSearchResult {
        sources: annotate_sources(sources),
        content: (!answer.trim().is_empty()).then(|| answer.chars().take(12000).collect()),
        ..Default::default()
    })
}

#[async_trait]
impl WebSearchProvider for SearchApi {
    fn id(&self) -> &'static str {
        self.id
    }
    fn available(&self) -> bool {
        !self.config.api_key.trim().is_empty()
            && (matches!(self.id, "exa" | "perplexity") || !self.config.model.trim().is_empty())
    }
    async fn search(&self, r: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        let body = request_body(self.id, &self.config.model, r)
            .map_err(|e| WebError::provider_error(self.id, e))?;
        let endpoint = format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            match self.id {
                "exa" | "perplexity" => "search",
                "anthropic" => "messages",
                _ => "responses",
            }
        );
        crate::network::url_fetcher::validate_url(&endpoint)
            .map_err(|e| WebError::provider_error(self.id, e.to_string()))?;
        let mut req = self
            .client
            .post(endpoint)
            .timeout(Duration::from_secs(r.timeout_secs.clamp(5, 50)))
            .json(&body);
        req = match self.id {
            "exa" => req.header("x-api-key", &self.config.api_key),
            "anthropic" => req
                .header("x-api-key", &self.config.api_key)
                .header("anthropic-version", "2023-06-01"),
            _ => req.bearer_auth(&self.config.api_key),
        };
        let mut response = req
            .send()
            .await
            .map_err(|e| WebError::provider_error(self.id, e.to_string()))?;
        let status = response.status();
        // Never follow redirects, nor consume arbitrary redirect bodies.
        if status.is_redirection() {
            return Err(WebError::provider_error(
                self.id,
                format!(
                    "Search API HTTP {status}; redirects are disabled for credentialed requests"
                ),
            ));
        }
        let mut bytes = vec![];
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| WebError::provider_error(self.id, e.to_string()))?
        {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(WebError::provider_error(
                    self.id,
                    "Search response exceeds 2 MiB",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let payload = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
            let detail = payload["error"]["message"]
                .as_str()
                .or(payload["message"].as_str())
                .unwrap_or("Check endpoint, model support, account credits and API permissions")
                .replace(&self.config.api_key, "[redacted]")
                .chars()
                .take(500)
                .collect::<String>();
            return Err(WebError::provider_error(
                self.id,
                format!("Search API HTTP {status}: {detail}"),
            ));
        }
        let payload = serde_json::from_slice(&bytes)
            .map_err(|e| WebError::provider_error(self.id, format!("Invalid search JSON: {e}")))?;
        parse_response(self.id, &payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_api_citations_and_structured_sources() {
        let r=parse_response("openai",&json!({"output":[{"type":"message","content":[{"type":"output_text","text":"Answer","annotations":[{"type":"url_citation","url":"https://example.com/a","title":"A"},{"type":"url_citation","url":"https://example.com/a"}]}]}]})).unwrap();
        assert_eq!(r.sources.len(), 1);
        assert_eq!(r.sources[0].title.as_deref(), Some("A"));
        assert!(r.content.is_some());
        assert!(parse_response(
            "xai",
            &json!({"output":[{"content":[{"type":"output_text","text":"guessed"}]}]})
        )
        .is_err());
        let r=parse_response("exa",&json!({"results":[{"url":"https://example.com","text":"full body","publishedDate":"2026-10-01","highlights":["quote"]}]})).unwrap();
        assert_eq!(r.sources[0].raw_content.as_deref(), Some("full body"));
        assert!(parse_response("perplexity", &json!({"error":"bad"})).is_err());
    }
    #[test]
    fn web_api_provider_specific_filters() {
        let mut r = WebSearchRequest::new("AI");
        r.include_domains = vec!["example.com".into()];
        r.exclude_domains = vec!["bad.com".into()];
        assert!(request_body("xai", "grok", &r).is_err());
        assert_eq!(
            request_body("openai", "gpt", &r).unwrap()["tools"][0]["filters"]["blocked_domains"][0],
            "bad.com"
        );
        assert_eq!(
            request_body("exa", "", &r).unwrap()["includeDomains"][0],
            "example.com"
        );
        assert_eq!(
            request_body("perplexity", "", &r).unwrap()["search_domain_filter"][1],
            "-bad.com"
        );
        assert!(!openai_factory(None, None).available());
    }
    #[tokio::test]
    async fn web_api_credentials_do_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 16384];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_lowercase();
            assert!(request.contains("authorization: bearer fixture-key"));
            assert!(request.starts_with("post /search "));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let mut config = WebSearchConfig::default();
        config.perplexity.api_key = "fixture-key".into();
        config.perplexity.base_url = format!("http://{address}");
        let result = perplexity_factory(Some(&config), None)
            .search(&WebSearchRequest::new("AI"))
            .await;
        assert!(result.unwrap_err().message.contains("302"));
        server.await.unwrap();
    }
}
