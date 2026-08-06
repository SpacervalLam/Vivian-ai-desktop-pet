//! 网络搜索后端 — 多引擎混用搜索
//!
//! 提供 `WebSearcher` 给以下消费方使用：
//! - `tools::builtin::web_search_tool::WebSearchTool`（LLM 可调用的搜索工具）
//! - `presence::background_tasks::spawn_knowledge_acquisition`（Busy 状态知识采集）
//!
//! 搜索策略为**多引擎混用**：用户可同时启用 duckduckgo / searxng / tavily，
//! 搜索工具会并发调用所有已配置的引擎，按 URL 去重后合并返回：
//! - `duckduckgo`（默认，零配置，HTML/Lite 爬取，始终可用作兜底）
//! - `searxng`（自部署元搜索引擎，聚合多源结果）
//! - `tavily`（专为 LLM Agent 设计的搜索 API）
//!
//! 注：搜索决策已完全交由 LLM 通过 `web_search` 工具自主判断，
//! 本模块不参与决策，仅提供搜索后端能力。

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// SearchResult — 搜索结果
// ============================================================================

/// 单条搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

// ============================================================================
// HTTP 客户端 & 预编译正则
// ============================================================================

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// DuckDuckGo HTML 搜索结果正则
static RESULT_A_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .unwrap()
});

static RESULT_SNIPPET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<a[^>]*class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>"#).unwrap()
});

// 通用链接正则（备用）
static GENERIC_LINK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<a[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());

// DuckDuckGo Lite 搜索结果正则
static LITE_LINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<a[^>]*rel="nofollow"[^>]*href="([^"]*)"[^>]*>\s*(.*?)\s*</a>"#).unwrap()
});

static LITE_SNIPPET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<td[^>]*class="[^"]*snippet[^"]*"[^>]*>(.*?)</td>"#).unwrap()
});

// HTML 标签正则
static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());

// ============================================================================
// WebSearcher — 多引擎混用
// ============================================================================

/// 网络搜索引擎（多引擎混用）
///
/// 支持同时启用多个引擎（通过 `WebSearchConfig.providers` 列表）：
/// - `duckduckgo`（默认，零配置，HTML/Lite 爬取，始终可用作兜底）
/// - `searxng`（自部署元搜索引擎，聚合多源结果）
/// - `tavily`（专为 LLM Agent 设计的搜索 API）
///
/// 搜索策略：
/// 1. 收集 `providers` 中所有启用的引擎
/// 2. 跳过未配置必要参数的引擎（SearXNG 缺 base_url / Tavily 缺 api_key），并记录日志
/// 3. 并发调用所有可用引擎（`futures::future::join_all`）
/// 4. 按 providers 顺序合并结果，按 URL 去重
/// 5. 截断到 `max_results`
/// 6. 若所有引擎都返回空（或全部跳过），最终回退到 DuckDuckGo 兜底
pub struct WebSearcher;

impl WebSearcher {
    /// 执行搜索（使用默认 DuckDuckGo 后端）
    ///
    /// 兼容旧调用点：未传入配置时走 DuckDuckGo。
    pub async fn search(query: &str, max_results: usize) -> Vec<SearchResult> {
        Self::search_with_config(query, max_results, None).await
    }

    /// 执行搜索（按配置混用多引擎）
    ///
    /// `config` 为 None 时走 DuckDuckGo（保持向后兼容）。
    pub async fn search_with_config(
        query: &str,
        max_results: usize,
        config: Option<&crate::config::WebSearchConfig>,
    ) -> Vec<SearchResult> {
        let timeout = config
            .map(|c| std::time::Duration::from_secs(c.timeout_secs))
            .unwrap_or_else(|| std::time::Duration::from_secs(15));

        // 收集启用的引擎列表（去重 + 过滤未知值）
        // 若配置为空，回退到 DuckDuckGo
        let providers: Vec<String> = config
            .map(|c| {
                if c.providers.is_empty() {
                    vec!["duckduckgo".to_string()]
                } else {
                    c.providers
                        .iter()
                        .filter(|p| matches!(p.as_str(), "duckduckgo" | "searxng" | "tavily"))
                        .cloned()
                        .collect()
                }
            })
            .unwrap_or_else(|| vec!["duckduckgo".to_string()]);

        if providers.is_empty() {
            return Self::search_ddg(query, max_results, timeout).await;
        }

        tracing::info!(
            "[WebSearcher] 混用搜索: query={:?}, providers={:?}, max_results={}",
            query,
            providers,
            max_results
        );

        // 为每个引擎构建一个 future，跳过未配置必要参数的引擎
        // 用 BoxFuture 借用 query / config 引用，避免不必要的 clone
        let mut futures: Vec<futures::future::BoxFuture<'_, (&'static str, Vec<SearchResult>)>> =
            Vec::new();
        let mut skipped: Vec<&'static str> = Vec::new();

        for p in &providers {
            match p.as_str() {
                "duckduckgo" => {
                    // DuckDuckGo 始终可用
                    futures.push(Box::pin(async move {
                        let results = Self::search_ddg(query, max_results, timeout).await;
                        ("duckduckgo", results)
                    }));
                }
                "searxng" => {
                    if let Some(cfg) = config {
                        if cfg.searxng.base_url.is_empty() {
                            tracing::warn!(
                                "[WebSearcher] SearXNG base_url 未配置，跳过该引擎"
                            );
                            skipped.push("searxng");
                            continue;
                        }
                        futures.push(Box::pin(async move {
                            let results =
                                Self::search_searxng(query, max_results, cfg, timeout).await;
                            ("searxng", results)
                        }));
                    }
                }
                "tavily" => {
                    if let Some(cfg) = config {
                        if cfg.tavily.api_key.is_empty() {
                            tracing::warn!(
                                "[WebSearcher] Tavily API Key 未配置，跳过该引擎"
                            );
                            skipped.push("tavily");
                            continue;
                        }
                        futures.push(Box::pin(async move {
                            let results =
                                Self::search_tavily(query, max_results, cfg, timeout).await;
                            ("tavily", results)
                        }));
                    }
                }
                _ => {}
            }
        }

        if futures.is_empty() {
            tracing::warn!(
                "[WebSearcher] 所有引擎均不可用，回退到 DuckDuckGo 兜底"
            );
            return Self::search_ddg(query, max_results, timeout).await;
        }

        // 并发执行所有引擎搜索
        let results_raw = futures::future::join_all(futures).await;

        // 收集成功结果（按 providers 列表中的顺序排列）
        // 用 Vec 而非 HashMap 以保留用户配置的优先级顺序
        let mut by_provider: Vec<(&'static str, Vec<SearchResult>)> = results_raw;

        // 按 providers 列表中的顺序排序
        by_provider.sort_by_key(|(label, _)| {
            providers
                .iter()
                .position(|p| p.as_str() == *label)
                .unwrap_or(usize::MAX)
        });

        // 合并去重（按 URL 规范化后比较，大小写不敏感）
        let mut seen_urls: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut merged: Vec<SearchResult> = Vec::new();
        for (_, results) in by_provider.iter() {
            for r in results.iter() {
                let key = normalize_url(&r.url);
                if key.is_empty() {
                    // URL 为空也允许进入（避免丢掉仅有的结果）
                    merged.push(r.clone());
                    continue;
                }
                if seen_urls.insert(key) {
                    merged.push(r.clone());
                }
            }
        }

        if merged.is_empty() {
            tracing::warn!(
                "[WebSearcher] 所有引擎均无结果（skipped={:?}），回退到 DuckDuckGo 兜底",
                skipped
            );
            return Self::search_ddg(query, max_results, timeout).await;
        }

        merged.truncate(max_results);
        merged
    }

    // ========================================================================
    // DuckDuckGo（HTML + Lite 回退）
    // ========================================================================

    async fn search_ddg(
        query: &str,
        max_results: usize,
        timeout: std::time::Duration,
    ) -> Vec<SearchResult> {
        let mut results = Self::ddg_html(query, max_results, timeout).await;
        if results.is_empty() {
            tracing::warn!("[WebSearcher] HTML 搜索无结果，尝试 lite 回退");
            results = Self::ddg_lite(query, max_results, timeout).await;
        }
        results.truncate(max_results);
        results
    }

    async fn ddg_html(
        query: &str,
        max_results: usize,
        timeout: std::time::Duration,
    ) -> Vec<SearchResult> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let resp = client
            .post("https://html.duckduckgo.com/html/")
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .form(&[("q", query)])
            .send()
            .await;

        match resp {
            Ok(r) => {
                if !r.status().is_success() {
                    tracing::warn!("[WebSearcher] DDG HTML 状态: {}", r.status());
                    return Vec::new();
                }
                match r.text().await {
                    Ok(html) => Self::parse_html(&html, max_results),
                    Err(e) => {
                        tracing::warn!("[WebSearcher] DDG HTML 读体失败: {}", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[WebSearcher] DDG HTML 请求失败: {}", e);
                Vec::new()
            }
        }
    }

    async fn ddg_lite(
        query: &str,
        max_results: usize,
        timeout: std::time::Duration,
    ) -> Vec<SearchResult> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let resp = client
            .post("https://lite.duckduckgo.com/lite/")
            .form(&[("q", query)])
            .send()
            .await;

        match resp {
            Ok(r) => {
                if !r.status().is_success() {
                    tracing::warn!("[WebSearcher] DDG Lite 状态: {}", r.status());
                    return Vec::new();
                }
                match r.text().await {
                    Ok(html) => Self::parse_lite(&html, max_results),
                    Err(e) => {
                        tracing::warn!("[WebSearcher] DDG Lite 读体失败: {}", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[WebSearcher] DDG Lite 请求失败: {}", e);
                Vec::new()
            }
        }
    }

    // ========================================================================
    // SearXNG（自部署元搜索引擎 JSON API）
    // ========================================================================

    async fn search_searxng(
        query: &str,
        max_results: usize,
        config: &crate::config::WebSearchConfig,
        timeout: std::time::Duration,
    ) -> Vec<SearchResult> {
        use crate::config::SearXngConfig;
        let SearXngConfig {
            base_url,
            auth_token,
        } = &config.searxng;

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let mut req = client
            .get(format!("{}/search", base_url.trim_end_matches('/')))
            .query(&[
                ("q", query),
                ("format", "json"),
                ("pageno", "1"),
            ]);

        if let Some(lang) = &config.language {
            if !lang.is_empty() {
                req = req.query(&[("language", lang.as_str())]);
            }
        }
        if !auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", auth_token));
        }

        let resp = req.send().await;
        match resp {
            Ok(r) => {
                if !r.status().is_success() {
                    tracing::warn!("[WebSearcher] SearXNG 状态: {}", r.status());
                    return Vec::new();
                }
                match r.json::<Value>().await {
                    Ok(v) => Self::parse_searxng(&v, max_results),
                    Err(e) => {
                        tracing::warn!("[WebSearcher] SearXNG JSON 解析失败: {}", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[WebSearcher] SearXNG 请求失败: {}", e);
                Vec::new()
            }
        }
    }

    fn parse_searxng(v: &Value, limit: usize) -> Vec<SearchResult> {
        v.get("results")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .take(limit)
                    .filter_map(|item| {
                        let title = item
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let url = item
                            .get("url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        let snippet = item
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        if title.is_empty() && url.is_empty() {
                            None
                        } else {
                            Some(SearchResult {
                                title,
                                url,
                                snippet,
                            })
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // ========================================================================
    // Tavily（专为 LLM Agent 设计的搜索 API）
    // ========================================================================

    async fn search_tavily(
        query: &str,
        max_results: usize,
        config: &crate::config::WebSearchConfig,
        timeout: std::time::Duration,
    ) -> Vec<SearchResult> {
        use crate::config::TavilyConfig;
        let TavilyConfig {
            api_key,
            include_raw_content,
            search_depth,
        } = &config.tavily;

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let body = serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "include_answer": false,
            "include_raw_content": include_raw_content,
            "search_depth": search_depth,
        });

        let resp = client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        match resp {
            Ok(r) => {
                if !r.status().is_success() {
                    tracing::warn!("[WebSearcher] Tavily 状态: {}", r.status());
                    return Vec::new();
                }
                match r.json::<Value>().await {
                    Ok(v) => Self::parse_tavily(&v, max_results),
                    Err(e) => {
                        tracing::warn!("[WebSearcher] Tavily JSON 解析失败: {}", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[WebSearcher] Tavily 请求失败: {}", e);
                Vec::new()
            }
        }
    }

    fn parse_tavily(v: &Value, _limit: usize) -> Vec<SearchResult> {
        v.get("results")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let title = item
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let url = item
                            .get("url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        let snippet = item
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        if title.is_empty() && url.is_empty() {
                            None
                        } else {
                            Some(SearchResult {
                                title,
                                url,
                                snippet,
                            })
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 解析 DuckDuckGo HTML 搜索结果
    fn parse_html(html: &str, limit: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();

        let titles: Vec<_> = RESULT_A_RE.captures_iter(html).collect();
        let snippets: Vec<_> = RESULT_SNIPPET_RE.captures_iter(html).collect();

        let max = titles.len().max(snippets.len()).min(limit);
        for i in 0..max {
            let mut result = SearchResult {
                title: String::new(),
                url: String::new(),
                snippet: String::new(),
            };

            if i < titles.len() {
                let href = titles[i].get(1).map(|m| m.as_str()).unwrap_or("");
                let title_html = titles[i].get(2).map(|m| m.as_str()).unwrap_or("");
                result.url = decode_ddg_url(href);
                result.title = strip_html_tags(title_html);
            }

            if i < snippets.len() {
                let snippet_html = snippets[i].get(1).map(|m| m.as_str()).unwrap_or("");
                result.snippet = strip_html_tags(snippet_html);
            }

            if !result.url.is_empty() || !result.title.is_empty() {
                results.push(result);
            }
        }

        // 备用模式：通用链接提取
        if results.is_empty() {
            for cap in GENERIC_LINK_RE.captures_iter(html).take(limit) {
                let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let title_html = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                let title = strip_html_tags(title_html);

                if title.is_empty() || href.is_empty() {
                    continue;
                }

                let url = if href.contains("uddg=") {
                    decode_ddg_url(href)
                } else if href.starts_with("http") && !href.contains("duckduckgo.com") {
                    href.to_string()
                } else {
                    continue;
                };

                if !url.is_empty() {
                    results.push(SearchResult {
                        title,
                        url,
                        snippet: String::new(),
                    });
                }
            }
        }

        results
    }

    /// 解析 DuckDuckGo Lite 搜索结果
    fn parse_lite(html: &str, limit: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();

        let links: Vec<_> = LITE_LINK_RE.captures_iter(html).collect();
        for cap in links.iter().take(limit) {
            let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let title_html = cap.get(2).map(|m| m.as_str()).unwrap_or("");

            if href.is_empty() || href.starts_with("//") {
                continue;
            }

            results.push(SearchResult {
                title: strip_html_tags(title_html),
                url: decode_ddg_url(href),
                snippet: String::new(),
            });
        }

        // 填充摘要
        let snippets: Vec<_> = LITE_SNIPPET_RE.captures_iter(html).collect();
        for (i, cap) in snippets.iter().take(limit).enumerate() {
            if i < results.len() {
                let snippet_html = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                results[i].snippet = strip_html_tags(snippet_html);
            }
        }

        results
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 去除 HTML 标签并解码常见实体
fn strip_html_tags(s: &str) -> String {
    let stripped = TAG_RE.replace_all(s, "").to_string();
    decode_html_entities(&stripped).trim().to_string()
}

/// 解码常见 HTML 实体
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// 解析 DuckDuckGo 跳转链接
///
/// DDG 的搜索结果链接通常形如：
/// `//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc`
/// 本函数提取并解码 `uddg=` 参数中的真实 URL。
fn decode_ddg_url(href: &str) -> String {
    let href = href.trim();

    // 直接是完整 URL
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }

    // DDG 跳转格式：//duckduckgo.com/l/?uddg=ENCODED_URL
    if let Some(pos) = href.find("uddg=") {
        let encoded = &href[pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        return percent_decode(&encoded[..end]);
    }

    // 去掉前导 //
    if let Some(stripped) = href.strip_prefix("//") {
        return stripped.to_string();
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

/// 规范化 URL 用于去重比较
///
/// 规则：
/// - 转小写
/// - 去除 fragment（# 后内容）
/// - 去除常见跟踪查询参数（utm_*、gclid、fbclid 等）
/// - 去除末尾斜杠（除非是根路径）
/// - http/https 视为等价（去 scheme）
fn normalize_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }

    let lower = url.to_lowercase();

    // 去 fragment
    let without_fragment = lower.split('#').next().unwrap_or("");

    // 去 scheme（http/https 等价）
    let after_scheme: &str = if let Some(idx) = without_fragment.find("://") {
        &without_fragment[idx + 3..]
    } else {
        without_fragment
    };

    let (path_part, query_part) = match after_scheme.find('?') {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx + 1..]),
        None => (after_scheme, ""),
    };

    // 过滤跟踪参数
    let filtered_query: String = if !query_part.is_empty() {
        query_part
            .split('&')
            .filter(|kv| {
                if kv.is_empty() {
                    return false;
                }
                let key = kv.split('=').next().unwrap_or("");
                !key.starts_with("utm_")
                    && key != "gclid"
                    && key != "fbclid"
                    && key != "mc_cid"
                    && key != "mc_eid"
            })
            .collect::<Vec<_>>()
            .join("&")
    } else {
        String::new()
    };

    // 去末尾斜杠（保留根路径 /）
    let trimmed_path = if path_part.len() > 1 && path_part.ends_with('/') {
        &path_part[..path_part.len() - 1]
    } else {
        path_part
    };

    if filtered_query.is_empty() {
        trimmed_path.to_string()
    } else {
        format!("{}?{}", trimmed_path, filtered_query)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<b>Title</b>"), "Title");
        assert_eq!(strip_html_tags("<a href='x'>Link &amp; Co</a>"), "Link & Co");
        assert_eq!(strip_html_tags("no tags"), "no tags");
    }

    #[test]
    fn test_decode_ddg_url() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc";
        assert_eq!(decode_ddg_url(href), "https://example.com");

        let href2 = "https://example.com/page";
        assert_eq!(decode_ddg_url(href2), "https://example.com/page");

        let href3 = "//example.com/path";
        assert_eq!(decode_ddg_url(href3), "example.com/path");
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
        let results = WebSearcher::parse_html("<html></html>", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_html_with_results() {
        let html = r##"
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com">Example</a>
        <a class="result__snippet" href="#">This is a snippet</a>
        "##;
        let results = WebSearcher::parse_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "This is a snippet");
    }
}
