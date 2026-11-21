//! Fixed platform routes. No downloaded adapters or page-provided executable code.
use serde_json::{json, Value};

pub const PLATFORMS: &[&str] = &[
    "bilibili", "xiaohongshu", "douyin", "zhihu", "weibo", "twitter",
    "reddit", "github", "arxiv", "youtube",
];

pub fn supports_host(host: &str) -> bool {
    ["bilibili.com", "xiaohongshu.com", "douyin.com", "zhihu.com", "weibo.com",
     "x.com", "twitter.com", "reddit.com", "github.com", "arxiv.org", "youtube.com"]
        .iter().any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// Unwrap the native bridge's untrusted envelope and optional JSON string layer.
/// Only the article body is knowledge evidence; replies remain community opinions.
pub fn detail_body(text: &str) -> Option<String> {
    let text = text.trim();
    let text = if text.starts_with("<UNTRUSTED_PAGE_CONTENT") {
        let start = text.find('>')?;
        let end = text.rfind("</UNTRUSTED_PAGE_CONTENT>")?;
        text.get(start + 1..end)?.trim()
    } else { text };
    let mut value: Value = serde_json::from_str(text).ok()?;
    if let Some(inner) = value.as_str() {
        value = serde_json::from_str(inner).ok()?;
    }
    let body = value.get("body")?.as_str()?.trim();
    (body.chars().count() >= 40).then(|| body.chars().take(6000).collect())
}

/// Resolve platform search to the existing bounded background-tab operation.
/// Explicit URLs keep their original access parameters and extractor selection.
pub fn task_args(args: &Value) -> Result<Value, String> {
    if args.get("url").is_some() {
        if args.get("platform").is_some() || args.get("query").is_some() {
            return Err("使用 url，或使用 platform + query；不要同时提供".into());
        }
        return Ok(args.clone());
    }
    let platform = args["platform"].as_str().ok_or("缺少 url 或 platform + query")?;
    let query = args["query"].as_str().map(str::trim).filter(|q| !q.is_empty())
        .ok_or("平台搜索需要非空 query")?;
    let (base, parameter) = match platform {
        "bilibili" => ("https://search.bilibili.com/all", "keyword"),
        "xiaohongshu" => ("https://www.xiaohongshu.com/search_result", "keyword"),
        "douyin" => ("https://www.douyin.com/search/", ""),
        "zhihu" => ("https://www.zhihu.com/search?type=content", "q"),
        "weibo" => ("https://s.weibo.com/weibo", "q"),
        "twitter" => ("https://x.com/search?f=live", "q"),
        "reddit" => ("https://www.reddit.com/search/", "q"),
        "github" => ("https://github.com/search?type=repositories", "q"),
        "arxiv" => ("https://arxiv.org/search/?searchtype=all", "query"),
        "youtube" => ("https://www.youtube.com/results", "search_query"),
        _ => return Err(format!("不支持的平台：{platform}")),
    };
    let mut url = tauri::Url::parse(base).map_err(|e| e.to_string())?;
    if parameter.is_empty() {
        url.path_segments_mut().map_err(|_| "无效平台路径")?.pop_if_empty().push(query);
    } else {
        url.query_pairs_mut().append_pair(parameter, query);
    }
    let mut resolved = args.clone();
    let obj = resolved.as_object_mut().ok_or("参数必须为对象")?;
    obj.remove("platform");
    obj.remove("query");
    obj.insert("url".into(), json!(url.as_str()));
    obj.entry("extractor").or_insert(json!(platform));
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_routes_encode_queries_and_preserve_explicit_urls() {
        for platform in PLATFORMS {
            let args = task_args(&json!({"platform":platform,"query":"中文 C++ & Rust #教程"})).unwrap();
            let url = tauri::Url::parse(args["url"].as_str().unwrap()).unwrap();
            assert!(supports_host(url.host_str().unwrap()));
            assert!(url.fragment().is_none());
            assert_eq!(args["extractor"], *platform);
            if *platform != "douyin" {
                assert!(url.query_pairs().any(|(_, v)| v == "中文 C++ & Rust #教程"));
            } else {
                assert!(url.path().contains("%23"));
            }
        }
        let direct = json!({"url":"https://www.xiaohongshu.com/explore/abc?xsec_token=original"});
        assert_eq!(task_args(&direct).unwrap(), direct);
        assert!(task_args(&json!({"platform":"github","query":"  "})).is_err());
        assert!(task_args(&json!({"platform":"unknown","query":"x"})).is_err());
        assert!(task_args(&json!({"url":"https://example.com","platform":"github","query":"x"})).is_err());
        assert!(!supports_host("github.com.evil.example"));
    }

    #[test]
    fn detail_requires_article_evidence_and_never_uses_comments_as_body() {
        let body = "Article evidence. ".repeat(8);
        let data = json!({"body":body,"comments":["unverified claim"]}).to_string();
        let wrapped = format!("<UNTRUSTED_PAGE_CONTENT nonce=\"test\">\n{}\n</UNTRUSTED_PAGE_CONTENT>", json!(data));
        assert_eq!(detail_body(&wrapped).as_deref(), Some(body.trim()));
        for text in ["[]", "<html>login required</html>", "{\"body\":\"\",\"comments\":[\"claim\"]}", "<UNTRUSTED_PAGE_CONTENT>"] {
            assert!(detail_body(text).is_none());
        }
    }
}
