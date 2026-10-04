//! Bounded document retrieval shared by chat links and web_fetch.
use crate::error::{VivianError, VivianResult};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT: usize = 500_000;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLink {
    pub title: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Original PDF retained independently of the bounded text preview.
    #[serde(skip)]
    pub raw_pdf: Option<Arc<Vec<u8>>>,
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
    fetch_document(url, refresh, false).await
}
async fn fetch_document(url: &str, refresh: bool, allow_local: bool) -> VivianResult<FetchedPage> {
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
    let mut resp = public_response(u, allow_local).await?;
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
    let raw_pdf = (content_type.contains("application/pdf") || bytes.starts_with(b"%PDF-"))
        .then(|| Arc::new(bytes.clone()));
    let (title, text, links, published_at) =
        tokio::task::spawn_blocking(move || decode_document(&decode_url, &decode_type, &bytes))
            .await
            .map_err(|e| VivianError::Other(format!("解析失败: {e}")))??;
    if text.trim().is_empty() && raw_pdf.is_none() {
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
        raw_pdf,
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
fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && ip.octets()[0] != 0
                && ip.octets()[0] < 240
                && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                && !(ip.octets()[0] == 198 && (18..=19).contains(&ip.octets()[1]))
        }
        std::net::IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|v| public_ip(v.into()))
            .unwrap_or_else(|| {
                (ip.segments()[0] & 0xe000) == 0x2000
                    && !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && (ip.segments()[0] & 0xfe00) != 0xfc00
                    && (ip.segments()[0] & 0xffc0) != 0xfe80
                    && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0xdb8)
            }),
    }
}
/// Redirects are followed manually: inspect every destination before sending a request.
async fn public_response(
    mut url: reqwest::Url,
    allow_local: bool,
) -> VivianResult<reqwest::Response> {
    let started = Instant::now();
    let (_, proxy) = crate::network::web::read_search_config();
    for hop in 0..=5 {
        validate_url(url.as_str())?;
        let host = url
            .host_str()
            .ok_or_else(|| VivianError::Other("Missing hostname".into()))?;
        let addresses = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(443))),
        )
        .await
        .map_err(|_| VivianError::Other("DNS lookup timed out".into()))?
        .map_err(|e| VivianError::Other(format!("DNS lookup failed: {e}")))?
        .collect::<Vec<_>>();
        if addresses.is_empty() || (!allow_local && addresses.iter().any(|a| !public_ip(a.ip()))) {
            return Err(VivianError::Other("Web retrieval only permits public internet destinations; use authorized workspace/browser tools for local resources".into()));
        }
        let remaining = Duration::from_secs(20)
            .checked_sub(started.elapsed())
            .ok_or_else(|| VivianError::Other("Fetch reached its time budget".into()))?;
        let mut builder = reqwest::Client::builder()
            .timeout(remaining)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses);
        if let Some(proxy) = &proxy {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|e| VivianError::Other(e.to_string()))?);
        } else {
            builder = builder.no_proxy();
        }
        let response = builder
            .build()
            .map_err(|e| VivianError::Other(e.to_string()))?
            .get(url.clone())
            .header("User-Agent", "Mozilla/5.0 (compatible; VivianBot/1.0)")
            .send()
            .await
            .map_err(|e| VivianError::Other(format!("Fetch failed: {e}")))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if hop == 5 {
            return Err(VivianError::Other("Too many redirects".into()));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| VivianError::Other("Redirect omitted Location".into()))?;
        url = url
            .join(location)
            .map_err(|e| VivianError::Other(e.to_string()))?;
    }
    unreachable!()
}

fn decode_document(
    url: &str,
    ct: &str,
    bytes: &[u8],
) -> VivianResult<(String, String, Vec<PageLink>, Option<String>)> {
    if ct.contains("application/pdf") || bytes.starts_with(b"%PDF-") {
        let text = pdf_extract::extract_text_from_mem(bytes).unwrap_or_default();
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
    let text = markdown_element(root, url).trim().to_string();
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
/// Preserve source structure, especially code, lists and tables; discard executable/UI noise.
fn markdown_element(element: scraper::ElementRef<'_>, base: &str) -> String {
    markdown_at_depth(element, base, 0)
}
fn markdown_at_depth(element: scraper::ElementRef<'_>, base: &str, depth: usize) -> String {
    if depth > 128 {
        return String::new();
    }
    let e = element.value();
    if matches!(
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
        || e.attr("style").is_some_and(|style| {
            let style = style
                .split_whitespace()
                .collect::<String>()
                .to_ascii_lowercase();
            style.contains("display:none") || style.contains("visibility:hidden")
        })
    {
        return String::new();
    }
    if e.name() == "pre" {
        let raw = element.text().collect::<String>();
        let fence = "`".repeat(
            raw.split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1,
        );
        return format!("\n\n{fence}\n{raw}\n{fence}\n\n");
    }
    let mut body = String::new();
    for child in element.children() {
        if let Some(el) = scraper::ElementRef::wrap(child) {
            body.push_str(&markdown_at_depth(el, base, depth + 1));
        } else if let Some(text) = child.value().as_text() {
            // Preserve an inter-element space without joining adjacent paragraphs.
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.starts_with(char::is_whitespace) && !body.ends_with(char::is_whitespace) {
                body.push(' ');
            }
            body.push_str(&collapsed);
            if text.ends_with(char::is_whitespace) {
                body.push(' ');
            }
        }
    }
    let trimmed = body.trim();
    match e.name() {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => format!(
            "\n\n{} {trimmed}\n\n",
            "#".repeat(e.name()[1..].parse().unwrap_or(1))
        ),
        "p" | "div" | "section" | "article" | "main" | "ul" | "ol" => format!("\n\n{trimmed}\n\n"),
        "li" => format!(
            "\n{} {trimmed}\n",
            if element
                .parent()
                .and_then(scraper::ElementRef::wrap)
                .is_some_and(|p| p.value().name() == "ol")
            {
                "1."
            } else {
                "-"
            }
        ),
        "br" => "\n".into(),
        "code" => format!("`{trimmed}`"),
        "a" => e
            .attr("href")
            .and_then(|href| reqwest::Url::parse(base).ok()?.join(href).ok())
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(|u| format!("[{trimmed}]({u})"))
            .unwrap_or(body),
        "th" | "td" => format!(" {} |", trimmed.replace('|', "\\|")),
        "tr" => {
            let header = element
                .children()
                .filter_map(scraper::ElementRef::wrap)
                .any(|c| c.value().name() == "th");
            let divider = if header {
                format!(
                    "\n|{}",
                    " --- |".repeat(
                        element
                            .children()
                            .filter_map(scraper::ElementRef::wrap)
                            .count()
                    )
                )
            } else {
                String::new()
            };
            format!("\n|{trimmed}{divider}\n")
        }
        _ => body,
    }
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
    #[test]
    fn web_markdown_preserves_code_tables_and_lists() {
        let (_,text,_,_)=extract_html("https://example.com", "<main><h2>Evidence</h2><ul><li>one</li><li>two</li></ul><pre>  a\n    b</pre><table><tr><th>Name</th><th>Value</th></tr><tr><td>AI</td><td>42</td></tr></table></main>");
        assert!(text.contains("## Evidence"));
        assert!(text.contains("- one"));
        assert!(text.contains("- two"));
        assert!(text.contains("```\n  a\n    b\n```"));
        assert!(text.contains("| --- | --- |"));
        assert!(text.contains("AI |"));
    }
    #[test]
    fn web_public_destinations_exclude_local_and_mapped_ips() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
    }
    #[tokio::test]
    async fn web_local_fetch_is_rejected_before_request() {
        assert!(fetch_page_with_refresh("http://127.0.0.1:9/private", true)
            .await
            .unwrap_err()
            .to_string()
            .contains("public internet"));
    }
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
        let first = fetch_document(&url, false, true).await.unwrap();
        assert!(first.url.ends_with("/final"));
        assert!(first.text.chars().count() > 12000);
        assert_eq!(first.published_at.as_deref(), Some("2026-09-30"));
        let cached = fetch_document(&url, false, true).await.unwrap();
        assert!(cached.cached);
        assert_eq!(first.text, cached.text);
        let refreshed = fetch_document(&url, true, true).await.unwrap();
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
