//! Bounded document retrieval shared by chat links and web_fetch.
use crate::error::{VivianError, VivianResult};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use scraper::{Html, Selector};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT: usize = 500_000;
#[derive(Clone, Serialize)]
pub struct PageLink {
    pub title: String,
    pub url: String,
}
#[derive(Clone)]
pub struct FetchedPage {
    pub url: String,
    pub title: String,
    pub text: String,
    pub links: Vec<PageLink>,
    pub content_type: String,
    pub retrieved_at: String,
    pub published_at: Option<String>,
    pub truncated: bool,
    pub cached: bool,
}
static CACHE: Lazy<Mutex<HashMap<String, (Instant, Arc<FetchedPage>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
pub fn validate_url(url: &str) -> VivianResult<reqwest::Url> {
    let u = reqwest::Url::parse(url)
        .map_err(|_| VivianError::Other("需要完整的 http(s) URL".into()))?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(VivianError::Other(
            "仅支持不含登录凭据的 http(s) URL".into(),
        ));
    }
    Ok(u)
}
/// Knowledge ingestion retains its small budget; interactive reading uses the full cached document.
pub async fn fetch_page(url: &str) -> VivianResult<FetchedPage> {
    let mut page = fetch_page_with_refresh(url, false).await?;
    if page.text.chars().count() > 8000 {
        page.text = page.text.chars().take(8000).collect();
        page.truncated = true;
    }
    Ok(page)
}
pub async fn fetch_page_with_refresh(url: &str, refresh: bool) -> VivianResult<FetchedPage> {
    let u = validate_url(url)?;
    let key = u.to_string();
    if !refresh {
        if let Some((at, page)) = CACHE.lock().get(&key) {
            if at.elapsed() < Duration::from_secs(300) {
                let mut p = (**page).clone();
                p.cached = true;
                return Ok(p);
            }
        }
    }
    let mut resp = crate::network::http_client::get_global_client()
        .get(u)
        .header("User-Agent", "Mozilla/5.0 (compatible; VivianBot/1.0)")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| VivianError::Other(format!("抓取失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(VivianError::Other(format!("HTTP {}", resp.status())));
    }
    let url = resp.url().to_string();
    validate_url(&url)?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(VivianError::Other("文档超过 8 MiB 上限".into()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| VivianError::Other(format!("读体失败: {e}")))?
    {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err(VivianError::Other("文档超过 8 MiB 上限".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let decode_url = url.clone();
    let decode_type = content_type.clone();
    let (title, text, links, published_at) =
        tokio::task::spawn_blocking(move || decode_document(&decode_url, &decode_type, &bytes))
            .await
            .map_err(|e| VivianError::Other(format!("解析失败: {e}")))??;
    if text.trim().is_empty() {
        return Err(VivianError::Other("未提取到正文；可能需要登录、浏览器渲染或扫描 PDF 的 OCR。可使用已连接的浏览器桥读取页面。".into()));
    }
    let page = FetchedPage {
        url,
        title,
        truncated: text.chars().count() > MAX_TEXT,
        text: text.chars().take(MAX_TEXT).collect(),
        links,
        content_type,
        retrieved_at: chrono::Utc::now().to_rfc3339(),
        published_at,
        cached: false,
    };
    let mut cache = CACHE.lock();
    cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
    if cache.len() >= 16 {
        if let Some(k) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&k);
        }
    }
    cache.insert(key, (Instant::now(), Arc::new(page.clone())));
    Ok(page)
}
fn decode_document(
    url: &str,
    ct: &str,
    bytes: &[u8],
) -> VivianResult<(String, String, Vec<PageLink>, Option<String>)> {
    if ct.contains("application/pdf") || bytes.starts_with(b"%PDF-") {
        let text = pdf_extract::extract_text_from_mem(bytes)
            .map_err(|e| VivianError::Other(format!("PDF 提取失败: {e}")))?;
        return Ok((url.into(), text, vec![], None));
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let (text, _, _) = detector.guess(None, true).decode(bytes);
    if ct.contains("html") || (ct.is_empty() && text.trim_start().starts_with('<')) {
        return Ok(extract_html(url, &text));
    }
    if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") {
        return Ok((url.into(), text.into_owned(), vec![], None));
    }
    Err(VivianError::Other(format!(
        "不支持 {ct}；支持 HTML、文本、Markdown、JSON、XML 和 PDF 文本"
    )))
}
fn extract_html(url: &str, html: &str) -> (String, String, Vec<PageLink>, Option<String>) {
    let document = Html::parse_document(html);
    let title_selector = Selector::parse("title").expect("constant selector");
    let title = document
        .select(&title_selector)
        .next()
        .map(|e| e.text().collect::<String>())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| url.into());
    let main_selector = Selector::parse("main, article, [role=main]").expect("constant selector");
    let root = document
        .select(&main_selector)
        .next()
        .unwrap_or_else(|| document.root_element());
    let mut text = String::new();
    for node in root.descendants() {
        if node.ancestors().any(|n| {
            n.value().as_element().is_some_and(|e| {
                matches!(
                    e.name(),
                    "script"
                        | "style"
                        | "nav"
                        | "header"
                        | "footer"
                        | "aside"
                        | "noscript"
                        | "iframe"
                        | "form"
                        | "svg"
                        | "head"
                ) || e.attr("hidden").is_some()
                    || e.attr("aria-hidden") == Some("true")
            })
        }) {
            continue;
        }
        match node.value() {
            scraper::Node::Text(t) => text.push_str(t),
            scraper::Node::Element(e)
                if matches!(
                    e.name(),
                    "p" | "div"
                        | "br"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "li"
                        | "tr"
                        | "section"
                        | "pre"
                        | "blockquote"
                ) =>
            {
                text.push('\n')
            }
            _ => {}
        }
    }
    let text = text
        .lines()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let link_selector = Selector::parse("a[href]").expect("constant selector");
    let base = reqwest::Url::parse(url).ok();
    let mut seen = std::collections::HashSet::new();
    let links = document
        .select(&link_selector)
        .filter_map(|e| {
            let target = base.as_ref()?.join(e.value().attr("href")?).ok()?;
            if !matches!(target.scheme(), "http" | "https") || !seen.insert(target.to_string()) {
                return None;
            }
            Some(PageLink {
                title: e.text().collect::<String>().trim().to_string(),
                url: target.to_string(),
            })
        })
        .take(100)
        .collect();
    let date_selector = Selector::parse("meta[property='article:published_time'], meta[name='date'], meta[itemprop='datePublished'], time[itemprop='datePublished']").expect("constant selector");
    let published_at = document.select(&date_selector).find_map(|e| {
        e.value()
            .attr("content")
            .or(e.value().attr("datetime"))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    (title.trim().into(), text, links, published_at)
}
pub fn extract_first_url(text: &str) -> Option<String> {
    static RE: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(r#"https?://[^\s<>"'，。、）)】\]]+"#).expect("constant regex")
    });
    RE.find(text)
        .map(|m| m.as_str().trim_end_matches(",.;:!?)").to_string())
        .filter(|s| validate_url(s).is_ok())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn web_http_redirect_cache_and_refresh() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut hits = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..n]);
                let response = if request.starts_with("GET /redirect ") {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
                } else {
                    hits += 1;
                    let body=format!("<meta property='article:published_time' content='2026-09-30'><main>{}证据-{hits}</main>","中".repeat(12000));
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body)
                };
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let url = format!("http://{address}/redirect");
        let first = fetch_page_with_refresh(&url, false).await.unwrap();
        assert!(first.url.ends_with("/final"));
        assert!(first.text.chars().count() > 12000);
        assert_eq!(first.published_at.as_deref(), Some("2026-09-30"));
        let cached = fetch_page_with_refresh(&url, false).await.unwrap();
        assert!(cached.cached);
        assert_eq!(first.text, cached.text);
        let refreshed = fetch_page_with_refresh(&url, true).await.unwrap();
        assert!(!refreshed.cached);
        assert!(refreshed.text.ends_with("证据-2"));
        let knowledge = fetch_page(&url).await.unwrap();
        assert_eq!(knowledge.text.chars().count(), 8000);
        assert!(knowledge.truncated);
        server.abort();
    }
    #[test]
    fn web_pdf_text_extraction() {
        let stream = "BT /F1 12 Tf 72 720 Td (Evidence PDF) Tj ET";
        let objects=vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),format!("<< /Length {} >>\nstream\n{}\nendstream",stream.len(),stream)];
        let mut pdf = "%PDF-1.4\n".to_string();
        let mut offsets = vec![0];
        for (n, obj) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", n + 1, obj));
        }
        let xref = pdf.len();
        pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        ));
        let (_, text, _, _) = decode_document(
            "https://example.com/test.pdf",
            "application/pdf",
            pdf.as_bytes(),
        )
        .unwrap();
        assert!(text.contains("Evidence PDF"));
    }
    #[test]
    fn html_evidence() {
        let (title, text, links, _) = extract_html("https://example.com/docs/", "<title>T &amp; X</title><nav>noise</nav><main><h1>标题</h1><p>第一段 &#x4e2d;<b>文</b></p><script>bad()</script><p hidden>隐藏</p><a href='../next'>下一页</a></main>");
        assert_eq!(title, "T & X");
        assert!(text.contains("第一段 中文"));
        assert!(!text.contains("bad()"));
        assert!(!text.contains("noise"));
        assert!(!text.contains("隐藏"));
        assert_eq!(links[0].url, "https://example.com/next");
    }
    #[test]
    fn long_document() {
        let (_, text, _, _) = extract_html(
            "https://example.com",
            &format!("<main>{}</main>", "中".repeat(40_000)),
        );
        assert_eq!(text.chars().count(), 40_000);
    }
    #[test]
    fn text_and_urls() {
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("https://u:p@example.com").is_err());
        assert_eq!(
            extract_first_url("看 https://example.com/a。"),
            Some("https://example.com/a".into())
        );
        assert_eq!(
            decode_document("https://example.com", "text/markdown", b"# hello")
                .unwrap()
                .1,
            "# hello"
        );
    }
}
