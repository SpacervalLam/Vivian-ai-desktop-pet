//! Provider 公共辅助：HTTP 客户端构建、HTML 清洗、URL 规范化、来源分级标注。
//!
//! 从旧 `network/web_context.rs` 迁移，供四个内置 provider 与缝隙合并逻辑共用。

use crate::network::web::types::WebSearchSource;

/// 通用浏览器 UA（HTML 爬取型 provider 使用）
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// ============================================================================
// HTTP 客户端
// ============================================================================

/// 构建带可选代理与超时的 reqwest 客户端
pub fn build_search_client(
    timeout: std::time::Duration,
    user_agent: Option<&str>,
    proxy_url: Option<&str>,
) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(timeout).redirect(reqwest::redirect::Policy::none());
    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }
    if let Some(url) = proxy_url {
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// 从配置秒数派生请求超时（0 或未配置按 15 秒）
pub fn timeout_from_secs(secs: u64) -> std::time::Duration {
    if secs == 0 {
        std::time::Duration::from_secs(15)
    } else {
        std::time::Duration::from_secs(secs)
    }
}

// ============================================================================
// HTML 清洗
// ============================================================================

/// 去除 HTML 标签并解码常见实体
pub fn strip_html_tags(s: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
    let stripped = TAG_RE.replace_all(s, "").to_string();
    decode_html_entities(&stripped).trim().to_string()
}

/// 解码常见 HTML 实体
pub fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

// ============================================================================
// URL 规范化（缝隙合并去重用）
// ============================================================================

/// 规范化 URL 用于去重比较
///
/// 规则：
/// - 转小写
/// - 去除 fragment（# 后内容）
/// - 去除常见跟踪查询参数（utm_*、gclid、fbclid 等）
/// - 去除末尾斜杠（除非是根路径）
/// - http/https 视为等价（去 scheme）
pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| {
            !k.to_ascii_lowercase().starts_with("utm_")
                && !matches!(k.as_ref(), "gclid" | "fbclid" | "mc_cid" | "mc_eid")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    parsed.set_query(None);
    if !pairs.is_empty() {
        parsed.query_pairs_mut().extend_pairs(pairs);
    }
    // URL paths and query values are case-sensitive. Only the parsed host is normalized.
    let lower = parsed.as_str().to_string();

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
// 来源权威分级标注（vivian-rs 特有，与 provider 无关）
// ============================================================================

/// 依据域名推断来源权威分级（对应 research-guide 的 P0~P3 层级）
///
/// - P0：官方/原始来源（政府、学术论文、官方文档、权威白皮书）
/// - P1：权威二手（主流媒体、行业报告、同行评审）
/// - P2：专业社区（带数据/代码的技术博客、论坛、问答）
/// - P3：一般参考（百科、自媒体、未核验内容）
pub fn classify_source_tier(url: &str) -> String {
    let normalized_url = url.trim().to_lowercase();
    let host = reqwest::Url::parse(url.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let host = host.trim_start_matches("www.").trim_start_matches("m.");

    // P0：政府 / 官方 / 学术 / 原始数据
    if host == "gov.cn"
        || host.ends_with(".gov")
        || host.ends_with(".gov.cn")
        || host.ends_with(".edu")
        || host.ends_with(".edu.cn")
        || host.ends_with(".mil")
        || host.ends_with(".int")
        || host.eq_ignore_ascii_case("arxiv.org")
        || host.eq_ignore_ascii_case("doi.org")
        || host.eq_ignore_ascii_case("semanticscholar.org")
        || host.eq_ignore_ascii_case("pubmed.ncbi.nlm.nih.gov")
        || host.eq_ignore_ascii_case("docs.rs")
        || host.eq_ignore_ascii_case("developer.mozilla.org")
        || host.eq_ignore_ascii_case("developers.openai.com")
        || host.eq_ignore_ascii_case("platform.openai.com")
        || host.eq_ignore_ascii_case("learn.microsoft.com")
        || host.eq_ignore_ascii_case("ai.google.dev")
    {
        return "P0".to_string();
    }

    // P1：权威媒体 / 行业报告
    if host.eq_ignore_ascii_case("reuters.com")
        || host.eq_ignore_ascii_case("apnews.com")
        || host.eq_ignore_ascii_case("bbc.com")
        || host.eq_ignore_ascii_case("bloomberg.com")
        || host.eq_ignore_ascii_case("ft.com")
        || host.eq_ignore_ascii_case("wsj.com")
        || host.eq_ignore_ascii_case("nytimes.com")
        || host.eq_ignore_ascii_case("nature.com")
        || host.eq_ignore_ascii_case("science.org")
        || host.eq_ignore_ascii_case("forbes.com")
        || host.eq_ignore_ascii_case("gartner.com")
        || host.eq_ignore_ascii_case("idc.com")
    {
        return "P1".to_string();
    }

    // P2：专业技术社区 / 问答 / 论坛
    if host.eq_ignore_ascii_case("stackoverflow.com")
        || host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("stackexchange.com")
        || host.eq_ignore_ascii_case("zhihu.com")
        || host.eq_ignore_ascii_case("medium.com")
        || host.eq_ignore_ascii_case("dev.to")
        || host.eq_ignore_ascii_case("csdn.net")
        || host.eq_ignore_ascii_case("juejin.cn")
        || host.eq_ignore_ascii_case("segmentfault.com")
        || host.eq_ignore_ascii_case("opensource.org")
        || normalized_url
            .split(['?', '#'])
            .next()
            .is_some_and(|value| value.contains("/blog"))
    {
        return "P2".to_string();
    }

    // 其余视为 P3
    "P3".to_string()
}

/// 依据来源分级派生信心标注（对应 research-guide 的 CONFIRMED 等）
///
/// 单条结果仅代表该来源自身的可信度，最终结论级信心由 LLM 依据多来源综合。
fn confidence_from_tier(_tier: &str) -> String {
    "UNVERIFIED".to_string()
}

/// 为一条搜索来源补齐来源分级与信心标注（provider 解析后统一调用）
pub fn annotate_source(mut s: WebSearchSource) -> WebSearchSource {
    s.source_id = source_id(&s.url);
    s.retrieved_at = chrono::Utc::now().to_rfc3339();
    s.confidence = "UNVERIFIED".into();
    if s.source_tier.is_empty() {
        let tier = classify_source_tier(&s.url);
        s.confidence = confidence_from_tier(&tier);
        s.source_tier = tier;
    }
    s
}

/// Stable FNV-1a ID of a canonical URL; never claims that an excerpt verifies a claim.
pub fn source_id(url: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalize_url(url).bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("web_{hash:016x}")
}

pub fn domain_matches(url: &str, domains: &[String]) -> bool {
    let Some(host) = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    domains
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// 批量标注
pub fn annotate_sources(sources: Vec<WebSearchSource>) -> Vec<WebSearchSource> {
    sources.into_iter().map(annotate_source).collect()
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
        assert_eq!(
            strip_html_tags("<a href='x'>Link &amp; Co</a>"),
            "Link & Co"
        );
        assert_eq!(strip_html_tags("no tags"), "no tags");
    }

    #[test]
    fn test_normalize_url() {
        assert_ne!(
            normalize_url("https://example.com/A?value=X"),
            normalize_url("https://example.com/a?value=x")
        );
        assert_eq!(
            source_id("https://example.com/A?utm_source=ads"),
            source_id("https://example.com/A")
        );
        assert_eq!(classify_source_tier("https://official-fake.test/"), "P3");
        assert_eq!(classify_source_tier("https://en.wikipedia.org/"), "P3");
        assert_eq!(normalize_url("https://Example.com/a/"), "example.com/a");
        assert_eq!(normalize_url("http://example.com/a"), "example.com/a");
        assert_eq!(
            normalize_url("https://example.com/a?utm_source=x&id=1"),
            "example.com/a?id=1"
        );
        assert_eq!(normalize_url("https://example.com/a#frag"), "example.com/a");
        assert_eq!(normalize_url(""), "");
    }

    #[test]
    fn test_classify_source_tier() {
        assert_eq!(classify_source_tier("https://www.gov.cn/news"), "P0");
        assert_eq!(classify_source_tier("https://arxiv.org/abs/1234"), "P0");
        assert_eq!(classify_source_tier("https://github.com/user/repo"), "P2");
        assert_eq!(classify_source_tier("https://reuters.com/world"), "P1");
        assert_eq!(classify_source_tier("https://zhihu.com/question/1"), "P2");
        assert_eq!(classify_source_tier("https://csdn.net/article"), "P2");
        assert_eq!(classify_source_tier("https://example.com/blog"), "P2");
        assert_eq!(
            classify_source_tier("https://some-unknown-site.com/x"),
            "P3"
        );
    }

    #[test]
    fn test_annotate_source() {
        let s = annotate_source(WebSearchSource::new("https://github.com/user/repo"));
        assert_eq!(s.source_tier, "P2");
        assert_eq!(s.confidence, "UNVERIFIED");
    }
}
