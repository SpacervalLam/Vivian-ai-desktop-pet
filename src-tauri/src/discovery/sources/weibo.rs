//! 微博匿名源 — m.weibo.cn H5 容器接口（零账号 cookie 重放）
//!
//! 端点：
//! - `GET m.weibo.cn/api/container/getIndex?containerid=100103type=1&q={kw}`：搜索
//! - `GET weibo.com/ajax/side/hotSearch`：实时热搜（无 cookie）
//!
//! 匿名访问模型：先走 visitor.passport.weibo.cn 引导拿游客 `SUB` cookie，
//! 仅注入该游客 SUB（剥离环境 Cookie，杜绝账号重放）；被拒时刷新一次。

use async_trait::async_trait;
use serde_json::Value;

use crate::network::http_client::get_global_client;

use super::{ContentCandidate, SourceAdapter};

const CONTAINER_URL: &str = "https://m.weibo.cn/api/container/getIndex";
const HOT_SEARCH_URL: &str = "https://weibo.com/ajax/side/hotSearch";
const VISITOR_ENTRY_URL: &str = "https://visitor.passport.weibo.cn/visitor/visitor";
const VISITOR_GENERATE_URL: &str = "https://visitor.passport.weibo.cn/visitor/genvisitor2";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
const VISITOR_SDK_UA: &str = "php-sso_sdk_client-0.6.36";
const TIMEOUT_SECS: u64 = 15;

/// 微博匿名适配器（内部缓存游客 SUB）
pub struct WeiboSource {
    visitor_sub: parking_lot::Mutex<Option<String>>,
}

impl WeiboSource {
    pub fn new() -> Self {
        Self {
            visitor_sub: parking_lot::Mutex::new(None),
        }
    }

    /// H5 容器接口 JSON（带游客 SUB，被拒刷新一次）
    async fn mobile_json(&self, url: &str) -> Option<Value> {
        if self.visitor_sub.lock().is_none() {
            *self.visitor_sub.lock() = bootstrap_visitor().await;
        }
        for _attempt in 0..2 {
            let sub = self.visitor_sub.lock().clone();
            let mut req = get_global_client()
                .get(url)
                .header("Accept", "application/json")
                .header("User-Agent", UA);
            if let Some(s) = sub {
                req = req.header("Cookie", format!("SUB={}", s));
            }
            let resp = req
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .ok()?;
            if [302, 403].contains(&resp.status().as_u16()) {
                let stale = self.visitor_sub.lock().clone();
                let fresh = bootstrap_visitor().await;
                if fresh.is_some() && fresh != stale {
                    *self.visitor_sub.lock() = fresh;
                    continue;
                }
                return None;
            }
            let payload: Value = resp.json().await.ok()?;
            if payload.get("ok").and_then(|o| o.as_i64()) == Some(1) {
                return Some(payload);
            }
            let sub2 = bootstrap_visitor().await;
            if sub2.is_some() && sub2 != self.visitor_sub.lock().clone() {
                *self.visitor_sub.lock() = sub2;
                continue;
            }
            return None;
        }
        None
    }
}

impl Default for WeiboSource {
    fn default() -> Self {
        Self::new()
    }
}

/// visitor 引导：entry 页解析 request_id/cb → genvisitor2 换 SUB
async fn bootstrap_visitor() -> Option<String> {
    let client = get_global_client();
    let entry_url = format!(
        "{}?entry=sinawap&a=enter&url={}&domain=.weibo.cn&sudaref=&ua={}&_rand={}",
        VISITOR_ENTRY_URL,
        urlencoding::encode(CONTAINER_URL),
        VISITOR_SDK_UA,
        rand_fraction(),
    );
    let entry = client
        .get(&entry_url)
        .header("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    let body = entry.text().await.ok()?;
    let request_id = regex_extract(&body, r#"var\s+request_id\s*=\s*["']([^"']+)["']"#)?;
    let cb = regex_extract(
        &body,
        r"genvisitor2.*?\bcb=([A-Za-z_$][A-Za-z0-9_$]{0,127})",
    )?;
    let ver = regex_extract(&body, r"genvisitor2.*?\bver=([A-Za-z0-9_.-]{1,32})")
        .unwrap_or_else(|| "v2.32".to_string());

    let gen_url = format!(
        "{}?cb={}&ver={}&request_id={}",
        VISITOR_GENERATE_URL, cb, ver, request_id
    );
    let gen = client
        .post(&gen_url)
        .header("User-Agent", UA)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("tid=&data={}")
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    let text = gen.text().await.ok()?;
    regex_extract(&text, r#""sub"\s*:\s*"([^"]+)""#)
}

fn regex_extract(text: &str, pattern: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn rand_fraction() -> String {
    let v = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    format!("{:.16}", v)
}

/// mblog 行 → 统一候选
fn mblog_to_candidate(row: &Value, source: &str) -> Option<ContentCandidate> {
    let content_id = ["id", "mid", "idstr"]
        .iter()
        .find_map(|k| row.get(k).and_then(|v| v.as_str()).map(|s| s.to_string()))
        .or_else(|| row.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()))?;
    let raw_text = row
        .get("text_raw")
        .or_else(|| row.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let body = strip_html(raw_text);
    if body.is_empty() {
        return None;
    }
    let user = row.get("user").cloned().unwrap_or(Value::Null);
    let author = user
        .get("screen_name")
        .or_else(|| user.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let title: String = body.lines().next().unwrap_or("").chars().take(100).collect();
    let user_id = user.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
    let bid = row.get("bid").and_then(|b| b.as_str()).unwrap_or("");
    let url = if user_id > 0 && !bid.is_empty() {
        format!("https://weibo.com/{}/{}", user_id, bid)
    } else {
        format!("https://m.weibo.cn/detail/{}", content_id)
    };
    Some(ContentCandidate {
        platform: "weibo".to_string(),
        content_id,
        title,
        description: body.chars().take(150).collect(),
        author,
        url,
        cover_url: first_pic(row),
        duration_secs: 0,
        view_count: row.get("reads_count").and_then(|c| c.as_u64()).unwrap_or(0),
        like_count: row.get("attitudes_count").and_then(|c| c.as_u64()).unwrap_or(0),
        pubdate: row
            .get("created_at")
            .and_then(|c| c.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(0),
        source: source.to_string(),
    })
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}

fn first_pic(row: &Value) -> String {
    if let Some(pics) = row.get("pics").and_then(|p| p.as_array()) {
        for pic in pics {
            for key in ["large", "url"] {
                if let Some(u) = pic.get(key).and_then(|u| u.as_str()) {
                    if u.starts_with("http") {
                        return u.to_string();
                    }
                }
            }
        }
    }
    row.get("original_pic")
        .or_else(|| row.get("bmiddle_pic"))
        .or_else(|| row.get("thumbnail_pic"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string()
}

/// 递归收集 mblog 行（cards 可嵌套 card_group）
fn extract_mblogs(cards: &Value, limit: usize, out: &mut Vec<Value>) {
    if out.len() >= limit {
        return;
    }
    if let Some(arr) = cards.as_array() {
        for card in arr {
            if out.len() >= limit {
                return;
            }
            if let Some(m) = card.get("mblog") {
                out.push(m.clone());
            } else if let Some(group) = card.get("card_group").and_then(|g| g.as_array()) {
                for g in group {
                    if out.len() >= limit {
                        return;
                    }
                    if let Some(m) = g.get("mblog") {
                        out.push(m.clone());
                    }
                }
            }
        }
    }
}

#[async_trait]
impl SourceAdapter for WeiboSource {
    fn platform(&self) -> &'static str {
        "weibo"
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<ContentCandidate> {
        let url = format!(
            "{}?containerid={}&page=1",
            CONTAINER_URL,
            urlencoding::encode(&format!("100103type=1&q={}", query))
        );
        let Some(payload) = self.mobile_json(&url).await else {
            tracing::debug!("[discovery:weibo] 搜索失败（游客被拒或网络）");
            return Vec::new();
        };
        let mut rows = Vec::new();
        if let Some(cards) = payload.get("data").and_then(|d| d.get("cards")) {
            extract_mblogs(cards, limit, &mut rows);
        }
        rows.iter()
            .filter_map(|r| mblog_to_candidate(r, &format!("search:{}", query)))
            .collect()
    }

    async fn popular(&self, limit: usize) -> Vec<ContentCandidate> {
        let client = get_global_client();
        let resp = client
            .get(HOT_SEARCH_URL)
            .header("Accept", "application/json")
            .header("Referer", "https://weibo.com/")
            .header("User-Agent", UA)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::debug!("[discovery:weibo] 热搜请求失败");
            return Vec::new();
        };
        let Ok(payload) = resp.json::<Value>().await else {
            tracing::debug!("[discovery:weibo] 热搜响应解析失败");
            return Vec::new();
        };
        payload
            .get("data")
            .and_then(|d| d.get("realtime"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .take(limit)
                    .filter_map(|item| {
                        let word = item.get("word").and_then(|w| w.as_str())?.trim().to_string();
                        if word.is_empty() {
                            return None;
                        }
                        Some(ContentCandidate {
                            platform: "weibo".to_string(),
                            content_id: item
                                .get("word_scheme")
                                .and_then(|w| w.as_str())
                                .unwrap_or(&word)
                                .to_string(),
                            title: format!("#{}#", word),
                            description: item
                                .get("note")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .chars()
                                .take(150)
                                .collect(),
                            author: String::new(),
                            url: format!(
                                "https://s.weibo.com/weibo?q=%23{}%23",
                                urlencoding::encode(&word)
                            ),
                            cover_url: String::new(),
                            duration_secs: 0,
                            view_count: item.get("num").and_then(|n| n.as_u64()).unwrap_or(0),
                            like_count: 0,
                            pubdate: 0,
                            source: "hot".to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
