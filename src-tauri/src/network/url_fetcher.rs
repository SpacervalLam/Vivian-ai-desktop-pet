//! URL 内容抓取器
//!
//! 抓取网页 HTML 并提取正文文本，供用户分享链接时入库知识库。
//! 采用轻量正则方案剥离标签，不引入额外 HTML 解析依赖。
//! 提取策略：移除 script/style/nav/header/footer 等非正文块 → 去标签 → 解码常见 HTML 实体。

use std::time::Duration;

use regex::Regex;

use crate::error::{VivianError, VivianResult};

/// 抓取结果
pub struct FetchedPage {
    pub url: String,
    pub title: String,
    pub text: String,
}

/// 抓取单个 URL 的页面内容
///
/// - 超时 15 秒
/// - 仅接受 text/html 响应
/// - 正文截断到 8000 字符（与知识库 token 预算匹配）
pub async fn fetch_page(url: &str) -> VivianResult<FetchedPage> {
    let client = crate::network::http_client::get_global_client();

    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; VivianBot/1.0)")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| VivianError::Other(format!("抓取 {} 失败: {}", url, e)))?;

    if !resp.status().is_success() {
        return Err(VivianError::Other(format!(
            "抓取 {} 返回 HTTP {}",
            url,
            resp.status()
        )));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
        return Err(VivianError::Other(format!(
            "不支持的内容类型: {}（仅支持 text/html）",
            content_type
        )));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| VivianError::Other(format!("读取响应体失败: {}", e)))?;

    let title = extract_title(&html).unwrap_or_else(|| url.to_string());
    let text = extract_main_text(&html);

    if text.trim().is_empty() {
        return Err(VivianError::Other(format!(
            "页面 {} 未提取到正文内容",
            url
        )));
    }

    // 截断到 8000 字符，避免知识库单条过长
    let text = if text.chars().count() > 8000 {
        text.chars().take(8000).collect::<String>() + "\n...(内容已截断)"
    } else {
        text
    };

    Ok(FetchedPage {
        url: url.to_string(),
        title,
        text,
    })
}

/// 从 HTML 中提取 <title>
fn extract_title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let caps = re.captures(html)?;
    let title = decode_html_entities(caps.get(1)?.as_str().trim());
    let title = title.trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// 从 HTML 提取正文文本
///
/// 步骤：
/// 1. 移除 script/style/nav/header/footer/aside/noscript/iframe/form 块
/// 2. 把 <br> 和 <p> 等块级标签转为换行
/// 3. 剥离所有标签
/// 4. 解码 HTML 实体
/// 5. 压缩多余空白
fn extract_main_text(html: &str) -> String {
    let remove_block_re = Regex::new(
        r"(?is)<(script|style|nav|header|footer|aside|noscript|iframe|form|svg)[^>]*>.*?</\1>",
    )
    .unwrap();
    let br_re = Regex::new(r"(?i)<br\s*/?>").unwrap();
    let block_re = Regex::new(r"(?i)</?(p|div|h[1-6]|li|tr|section|article|blockquote)[^>]*>").unwrap();
    let tag_re = Regex::new(r"(?s)<[^>]+>").unwrap();
    let multi_blank_re = Regex::new(r"\n{3,}").unwrap();

    let html = remove_block_re.replace_all(html, "");
    let html = br_re.replace_all(&html, "\n");
    let html = block_re.replace_all(&html, "\n");
    let text = tag_re.replace_all(&html, "");
    let text = decode_html_entities(&text);
    let text = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    multi_blank_re.replace_all(&text, "\n\n").trim().to_string()
}

/// 解码常见 HTML 实体
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&hellip;", "…")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&ldquo;", "\u{201c}")
        .replace("&rdquo;", "\u{201d}")
        .replace("&lsquo;", "\u{2018}")
        .replace("&rsquo;", "\u{2019}")
}

/// 从文本中提取首个 http(s) URL
///
/// 用于检测用户消息中是否包含网页链接。
/// 返回 None 表示消息中无 URL。
pub fn extract_first_url(text: &str) -> Option<String> {
    let re = Regex::new(r#"https?://[^\s<>"'，。、）)】\]]+"#).ok()?;
    let caps = re.captures(text)?;
    let url = caps.get(0)?.as_str().trim_end_matches(",.;:!?)");
    if url.len() < 12 {
        return None;
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_first_url() {
        assert_eq!(
            extract_first_url("看这篇文章 https://example.com/article/123 很不错"),
            Some("https://example.com/article/123".to_string())
        );
        assert_eq!(extract_first_url("没有链接的纯文本"), None);
        assert_eq!(
            extract_first_url("http://foo.com"),
            Some("http://foo.com".to_string())
        );
    }

    #[test]
    fn test_extract_title() {
        let html = "<html><head><title>  Rust 异步编程指南  </title></head><body>...</body></html>";
        assert_eq!(extract_title(html), Some("Rust 异步编程指南".to_string()));
    }

    #[test]
    fn test_extract_main_text() {
        let html = r#"
        <html>
        <head><title>Test</title><style>body{color:red}</style></head>
        <body>
            <nav>导航栏</nav>
            <header>页头</header>
            <article>
                <h1>标题</h1>
                <p>第一段内容</p>
                <p>第二段<br>换行</p>
                <script>alert('xss')</script>
            </article>
            <footer>页脚</footer>
        </body>
        </html>
        "#;
        let text = extract_main_text(html);
        assert!(text.contains("标题"));
        assert!(text.contains("第一段内容"));
        assert!(text.contains("第二段"));
        assert!(text.contains("换行"));
        assert!(!text.contains("导航栏"));
        assert!(!text.contains("页头"));
        assert!(!text.contains("页脚"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color:red"));
    }
}
