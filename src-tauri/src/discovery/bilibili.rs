//! B 站开放 API 客户端 — WBI 签名 + 匿名搜索 / 热门
//!
//! 匿名可用端点（无需登录 Cookie）：
//! - `/x/web-interface/nav`：获取 WBI 签名密钥（img_key / sub_key）
//! - `/x/web-interface/wbi/search/type`：视频搜索（需 WBI 签名）
//! - `/x/web-interface/popular`：综合热门（无需签名）
//!
//! WBI 签名算法：img_key+sub_key 按重排表取前 32 位得 mixin_key；
//! 参数加 wts 后按 key 排序、值过滤 `[!'()*]`，urlencode 后拼 mixin_key 取 MD5。

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

use crate::network::http_client::get_global_client;

const BASE_URL: &str = "https://api.bilibili.com";
const SEARCH_WEB_LOCATION: i64 = 1430654;
const WBI_KEY_TTL: Duration = Duration::from_secs(300);
/// 触发 412（IP 级封锁）后的搜索冷却时长
const SEARCH_COOLDOWN_412_SECS: u64 = 600;
/// v_voucher 连续质疑次数达到阈值后的搜索冷却时长
const SEARCH_COOLDOWN_VOUCHER_SECS: u64 = 180;
/// 连续 v_voucher 质疑的阈值（单次质疑仅丢弃该关键词）
const VOUCHER_STREAK_THRESHOLD: u32 = 3;

/// WBI mixin key 重排表（64 项，取前 32）
const WBI_MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 视频信息（搜索 / 热门统一归一化后的结构）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoInfo {
    pub bvid: String,
    pub title: String,
    pub description: String,
    pub up_name: String,
    pub cover_url: String,
    /// 播放时长（秒；解析失败为 0）
    pub duration_secs: u64,
    pub view_count: u64,
    pub like_count: u64,
    /// 发布时间（Unix 秒；缺失为 0）
    pub pubdate: i64,
    /// 发现来源：search:{query} / popular
    pub source: String,
}

/// 视频元信息（view 接口返回的 cid / aid，字幕与评论接口要用）
#[derive(Debug, Clone, Default)]
pub struct VideoMeta {
    pub cid: u64,
    pub aid: u64,
    pub title: String,
}

/// 评论样本（字幕/评论判断用的一条热门评论）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CommentSample {
    pub text: String,
    pub like: u64,
}

impl VideoInfo {
    pub fn url(&self) -> String {
        format!("https://www.bilibili.com/video/{}", self.bvid)
    }

    /// 时长 "mm:ss" / "hh:mm:ss" → 秒
    fn parse_duration(raw: &Value) -> u64 {
        if let Some(n) = raw.as_u64() {
            return n;
        }
        let s = raw.as_str().unwrap_or("");
        let parts: Vec<u64> = s
            .split(':')
            .filter_map(|p| p.trim().parse::<u64>().ok())
            .collect();
        match parts.len() {
            0 => 0,
            1 => parts[0],
            2 => parts[0] * 60 + parts[1],
            _ => parts[0] * 3600 + parts[1] * 60 + parts[2],
        }
    }

    /// 清理搜索结果标题中的 `<em class="keyword">` 高亮标签与转义字符
    fn clean_text(raw: &str) -> String {
        raw.replace("<em class=\"keyword\">", "")
            .replace("</em>", "")
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&#39;", "'")
            .trim()
            .to_string()
    }

    fn from_search_item(item: &Value, query: &str) -> Option<Self> {
        let bvid = item.get("bvid")?.as_str()?.trim().to_string();
        if bvid.is_empty() {
            return None;
        }
        Some(Self {
            bvid,
            title: Self::clean_text(item.get("title").and_then(|v| v.as_str()).unwrap_or("")),
            description: Self::clean_text(
                item.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            ),
            up_name: Self::clean_text(item.get("author").and_then(|v| v.as_str()).unwrap_or("")),
            cover_url: item
                .get("pic")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            duration_secs: Self::parse_duration(item.get("duration").unwrap_or(&Value::Null)),
            view_count: item.get("play").and_then(|v| v.as_u64()).unwrap_or(0),
            like_count: item.get("like").and_then(|v| v.as_u64()).unwrap_or(0),
            pubdate: item
                .get("pubdate")
                .and_then(|v| v.as_i64())
                .or_else(|| item.get("publish_time").and_then(|v| v.as_i64()))
                .unwrap_or(0),
            source: format!("search:{}", query),
        })
    }

    fn from_popular_item(item: &Value) -> Option<Self> {
        let bvid = item.get("bvid")?.as_str()?.trim().to_string();
        if bvid.is_empty() {
            return None;
        }
        let stat = item.get("stat").cloned().unwrap_or(Value::Null);
        Some(Self {
            bvid,
            title: Self::clean_text(item.get("title").and_then(|v| v.as_str()).unwrap_or("")),
            description: Self::clean_text(item.get("desc").and_then(|v| v.as_str()).unwrap_or("")),
            up_name: item
                .get("owner")
                .and_then(|o| o.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            cover_url: item
                .get("pic")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            duration_secs: item.get("duration").and_then(|v| v.as_u64()).unwrap_or(0),
            view_count: stat.get("view").and_then(|v| v.as_u64()).unwrap_or(0),
            like_count: stat.get("like").and_then(|v| v.as_u64()).unwrap_or(0),
            pubdate: item.get("pubdate").and_then(|v| v.as_i64()).unwrap_or(0),
            source: "popular".to_string(),
        })
    }
}

/// B 站 API 客户端（WBI 密钥缓存 5 分钟 + 搜索冷却与 v_voucher 质疑处理）
pub struct BilibiliClient {
    wbi_keys: Mutex<Option<((String, String), Instant)>>,
    /// 搜索冷却截止时刻（None = 无冷却）
    search_cooldown_until: Mutex<Option<Instant>>,
    /// 连续 v_voucher 质疑计数（成功请求归零）
    voucher_streak: std::sync::atomic::AtomicU32,
}

impl BilibiliClient {
    pub fn new() -> Self {
        Self {
            wbi_keys: Mutex::new(None),
            search_cooldown_until: Mutex::new(None),
            voucher_streak: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 搜索冷却剩余秒数
    pub fn search_cooldown_remaining(&self) -> u64 {
        self.search_cooldown_until
            .lock()
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn activate_cooldown(&self, secs: u64) {
        *self.search_cooldown_until.lock() = Some(Instant::now() + Duration::from_secs(secs));
    }

    async fn get_json(
        &self,
        path: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        cookie: Option<&str>,
    ) -> Result<Value, String> {
        let client = get_global_client();
        let url = format!("{}{}", BASE_URL, path);
        let mut req = client.get(&url).query(query).header("User-Agent", UA);
        // 登录态请求：带 Cookie，并默认带 B 站 Referer（显式 headers 可覆盖）
        if let Some(c) = cookie {
            req = req
                .header("Cookie", c)
                .header("Referer", "https://www.bilibili.com/");
        }
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("http status {}", resp.status()));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("json parse failed: {}", e))
    }

    /// 获取 WBI 密钥（img_key, sub_key），带 5 分钟缓存
    pub(crate) async fn get_wbi_keys(&self) -> Result<(String, String), String> {
        {
            let cached = self.wbi_keys.lock();
            if let Some(((img, sub), at)) = cached.as_ref() {
                if at.elapsed() < WBI_KEY_TTL {
                    return Ok((img.clone(), sub.clone()));
                }
            }
        }

        let payload = self
            .get_json("/x/web-interface/nav", &[], &[], None)
            .await?;
        let wbi_img = payload
            .get("data")
            .and_then(|d| d.get("wbi_img"))
            .cloned()
            .unwrap_or(Value::Null);
        let img_url = wbi_img
            .get("img_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let sub_url = wbi_img
            .get("sub_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let img_key = extract_key_component(img_url);
        let sub_key = extract_key_component(sub_url);
        if img_key.is_empty() || sub_key.is_empty() {
            return Err("missing wbi keys in nav response".to_string());
        }
        *self.wbi_keys.lock() = Some(((img_key.clone(), sub_key.clone()), Instant::now()));
        Ok((img_key, sub_key))
    }

    /// WBI 参数签名：mixin_key 重排 → wts 注入 → 排序 → 过滤特殊字符 → MD5
    pub(crate) fn sign_wbi_params(
        params: &[(String, String)],
        img_key: &str,
        sub_key: &str,
    ) -> Vec<(String, String)> {
        let merged: Vec<char> = format!("{}{}", img_key, sub_key).chars().collect();
        let mut mixin = String::new();
        for &idx in WBI_MIXIN_KEY_ENC_TAB.iter().take(32) {
            if let Some(c) = merged.get(idx) {
                mixin.push(*c);
            }
        }

        let wts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut signed: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| (k.clone(), sanitize_value(v)))
            .collect();
        signed.push(("wts".to_string(), wts.to_string()));
        signed.sort_by(|a, b| a.0.cmp(&b.0));

        // urlencode 查询串（与 Python urlencode 默认行为一致）
        let query: String = signed
            .iter()
            .map(|(k, v)| format!("{}={}", urlencode_component(k), urlencode_component(v)))
            .collect::<Vec<_>>()
            .join("&");
        let digest = md5_hex(&format!("{}{}", query, mixin));
        signed.push(("w_rid".to_string(), digest));
        signed
    }

    /// 视频搜索（WBI 签名 + 冷却/质疑处理；最多重试 2 次）
    pub async fn search(
        &self,
        keyword: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Vec<VideoInfo>, String> {
        self.search_ordered(keyword, page, page_size, false).await
    }

    /// 值班使用最新排序，避免第一页综合排序被旧热门占满后又全被新鲜度过滤。
    pub async fn search_ordered(
        &self,
        keyword: &str,
        page: u32,
        page_size: u32,
        newest_first: bool,
    ) -> Result<Vec<VideoInfo>, String> {
        use std::sync::atomic::Ordering;

        {
            let cooldown = self.search_cooldown_until.lock();
            if let Some(until) = *cooldown {
                if Instant::now() < until {
                    tracing::debug!(
                        "[discovery:bili] 搜索冷却中（剩 {}s），跳过 {}",
                        (until - Instant::now()).as_secs(),
                        keyword
                    );
                    return Ok(Vec::new());
                }
            }
        }

        let mut attempt = 0;
        loop {
            attempt += 1;
            let (img_key, sub_key) = self.get_wbi_keys().await?;
            let params = vec![
                ("keyword".to_string(), keyword.to_string()),
                ("search_type".to_string(), "video".to_string()),
                ("page".to_string(), page.to_string()),
                ("page_size".to_string(), page_size.to_string()),
                (
                    "order".to_string(),
                    if newest_first { "pubdate" } else { "totalrank" }.to_string(),
                ),
                ("web_location".to_string(), SEARCH_WEB_LOCATION.to_string()),
            ];
            let signed = Self::sign_wbi_params(&params, &img_key, &sub_key);
            let referer = format!(
                "https://search.bilibili.com/all?keyword={}",
                urlencode_component(keyword)
            );
            let result = self
                .get_json(
                    "/x/web-interface/wbi/search/type",
                    &signed,
                    &[
                        ("Referer".to_string(), referer),
                        (
                            "Origin".to_string(),
                            "https://search.bilibili.com".to_string(),
                        ),
                    ],
                    None,
                )
                .await;

            let payload = match result {
                Ok(p) => p,
                Err(e) => {
                    if e.contains("status 412") {
                        self.activate_cooldown(SEARCH_COOLDOWN_412_SECS);
                        tracing::warn!(
                            "[discovery:bili] 搜索被 412 封锁，冷却 {}s",
                            SEARCH_COOLDOWN_412_SECS
                        );
                        return Ok(Vec::new());
                    }
                    return Err(e);
                }
            };

            // v_voucher 质疑（密钥过期/限流）：刷新密钥重试
            let has_voucher = payload.get("v_voucher").is_some();
            let data = payload.get("data").cloned().unwrap_or(Value::Null);
            let empty_result = data
                .get("result")
                .map(|r| r.as_array().map(|a| a.is_empty()).unwrap_or(true))
                .unwrap_or(true);
            if has_voucher && empty_result {
                *self.wbi_keys.lock() = None;
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(1500 * attempt as u64)).await;
                    continue;
                }
                let streak = self.voucher_streak.fetch_add(1, Ordering::Relaxed) + 1;
                if streak >= VOUCHER_STREAK_THRESHOLD {
                    self.activate_cooldown(SEARCH_COOLDOWN_VOUCHER_SECS);
                    tracing::warn!(
                        "[discovery:bili] 连续 {} 次 v_voucher 质疑，搜索冷却 {}s",
                        streak,
                        SEARCH_COOLDOWN_VOUCHER_SECS
                    );
                }
                return Ok(Vec::new());
            }

            check_api_response(&payload)?;
            self.voucher_streak.store(0, Ordering::Relaxed);
            let results = data
                .get("result")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            let videos = results
                .iter()
                .filter_map(|item| VideoInfo::from_search_item(item, keyword))
                .collect();
            return Ok(videos);
        }
    }

    /// 综合热门（无需签名）
    pub async fn popular(&self, page: u32, page_size: u32) -> Result<Vec<VideoInfo>, String> {
        let payload = self
            .get_json(
                "/x/web-interface/popular",
                &[
                    ("ps".to_string(), page_size.to_string()),
                    ("pn".to_string(), page.to_string()),
                ],
                &[(
                    "Referer".to_string(),
                    "https://www.bilibili.com".to_string(),
                )],
                None,
            )
            .await?;
        check_api_response(&payload)?;
        let list = payload
            .get("data")
            .and_then(|d| d.get("list"))
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(VideoInfo::from_popular_item)
            .collect())
    }

    /// 拉取绝对 URL 的 JSON（字幕 CDN 在 BASE_URL 之外，不能用 get_json）
    async fn get_absolute_json(&self, url: &str) -> Result<Value, String> {
        let client = get_global_client();
        let req = client
            .get(url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/");
        let resp = req
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("http status {}", resp.status()));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("json parse failed: {}", e))
    }

    /// 视频元信息：`/x/web-interface/view` → cid / aid（无需 WBI）
    pub async fn video_view(&self, bvid: &str, cookie: Option<&str>) -> Result<VideoMeta, String> {
        let payload = self
            .get_json(
                "/x/web-interface/view",
                &[("bvid".to_string(), bvid.to_string())],
                &[],
                cookie,
            )
            .await?;
        check_api_response(&payload)?;
        let data = payload.get("data").cloned().unwrap_or(Value::Null);
        let cid = data.get("cid").and_then(|v| v.as_u64()).unwrap_or(0);
        let aid = data.get("aid").and_then(|v| v.as_u64()).unwrap_or(0);
        if cid == 0 || aid == 0 {
            return Err("view 接口缺少 cid/aid".to_string());
        }
        Ok(VideoMeta {
            cid,
            aid,
            title: data
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    /// 字幕全文：`/x/player/wbi/v2`（WBI 签名 + 登录态）→ subtitle_url → 字幕 JSON
    ///
    /// 无可读字幕返回 `Ok(None)`；不能由此判断视频是否有语音或硬字幕。
    pub async fn subtitles(
        &self,
        bvid: &str,
        cid: u64,
        cookie: Option<&str>,
    ) -> Result<Option<String>, String> {
        let (img_key, sub_key) = self.get_wbi_keys().await?;
        let params = vec![
            ("bvid".to_string(), bvid.to_string()),
            ("cid".to_string(), cid.to_string()),
        ];
        let signed = Self::sign_wbi_params(&params, &img_key, &sub_key);
        let payload = self
            .get_json("/x/player/wbi/v2", &signed, &[], cookie)
            .await?;
        check_api_response(&payload)?;
        let subtitles = payload
            .get("data")
            .and_then(|d| d.get("subtitle"))
            .and_then(|s| s.get("subtitles"))
            .and_then(|s| s.as_array())
            .ok_or_else(|| "字幕列表响应缺失".to_string())?;
        if subtitles.is_empty() {
            return Ok(None);
        }
        // 优先 AI 中文字幕，否则取第一条
        let target = subtitles
            .iter()
            .find(|s| {
                matches!(
                    s.get("lan").and_then(|l| l.as_str()).unwrap_or(""),
                    "ai-zh" | "zh-CN" | "zh"
                )
            })
            .or_else(|| subtitles.first());
        let Some(target) = target else {
            return Ok(None);
        };
        let raw_url = target
            .get("subtitle_url")
            .and_then(|u| u.as_str())
            .unwrap_or("");
        if raw_url.is_empty() {
            return Err("字幕轨缺少下载地址".to_string());
        }
        let sub = self
            // CDN 不需要账号 Cookie，避免向接口返回的任意地址发送登录凭据。
            .get_absolute_json(&normalize_subtitle_url(raw_url))
            .await?;
        if sub.get("body").and_then(Value::as_array).is_none() {
            return Err("字幕正文响应缺失".to_string());
        }
        let text = extract_subtitle_text(&sub);
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// 热门评论：`/x/v2/reply/wbi/main`（WBI 签名 + 登录态，按热度）
    ///
    /// 风控或业务错误交由富化层标为不可用，不冒充空评论。
    pub async fn top_comments(
        &self,
        bvid: &str,
        aid: u64,
        cookie: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommentSample>, String> {
        let (img_key, sub_key) = self.get_wbi_keys().await?;
        let params = vec![
            ("oid".to_string(), aid.to_string()),
            ("type".to_string(), "1".to_string()),
            ("mode".to_string(), "3".to_string()),
            ("ps".to_string(), "20".to_string()),
        ];
        let signed = Self::sign_wbi_params(&params, &img_key, &sub_key);
        let referer = format!("https://www.bilibili.com/video/{}", bvid);
        let payload = self
            .get_json(
                "/x/v2/reply/wbi/main",
                &signed,
                &[("Referer".to_string(), referer)],
                cookie,
            )
            .await;
        let payload = payload?;
        check_api_response(&payload)?;
        Ok(extract_comments(&payload, limit))
    }
}

/// HTTP 200 仍可能是登录失效/风控；不能把业务错误当成「没有字幕/评论」。
fn check_api_response(payload: &Value) -> Result<(), String> {
    let code = payload.get("code").and_then(Value::as_i64);
    if code != Some(0)
        || payload.get("v_voucher").is_some()
        || payload
            .get("data")
            .and_then(|d| d.get("v_voucher"))
            .is_some()
    {
        // 不回显上游正文，避免把账号信息或令牌混入日志。
        return Err(format!(
            "B站接口不可用（code={}）",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }
    Ok(())
}

impl Default for BilibiliClient {
    fn default() -> Self {
        Self::new()
    }
}

/// 从 WBI 图片 URL 提取密钥段（文件名去扩展名）
fn extract_key_component(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next().unwrap_or("");
    filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename)
        .to_string()
}

/// WBI 签名的值过滤：去除 `[!'()*]`
fn sanitize_value(v: &str) -> String {
    v.chars()
        .filter(|c| !matches!(c, '!' | '\'' | '(' | ')' | '*'))
        .collect()
}

/// 字幕 URL 归一化：`//host/path` → `https://host/path`
pub fn normalize_subtitle_url(url: &str) -> String {
    let url = url.trim();
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{}", rest),
        None => url.to_string(),
    }
}

/// 从字幕 JSON 拼接全文（`body[].content`）
pub fn extract_subtitle_text(value: &Value) -> String {
    value
        .get("body")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("content").and_then(|c| c.as_str()))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// 从评论 JSON 抽取样本：按赞降序、文本去重、截断到 limit
pub fn extract_comments(value: &Value, limit: usize) -> Vec<CommentSample> {
    let replies = value
        .get("data")
        .and_then(|d| d.get("replies"))
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<CommentSample> = replies
        .iter()
        .filter_map(|r| {
            let text = r
                .get("content")
                .and_then(|c| c.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                return None;
            }
            Some(CommentSample {
                text,
                like: r.get("like").and_then(|l| l.as_u64()).unwrap_or(0),
            })
        })
        .collect();
    out.sort_by(|a, b| b.like.cmp(&a.like));
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.text.clone()));
    out.truncate(limit);
    out
}

/// 查询串百分号编码（application/x-www-form-urlencoded 风格，
/// 保留字母数字与 `-_.~`，空格转 `+`——与 Python urlencode 默认行为一致）
fn urlencode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// MD5 十六进制摘要
fn md5_hex(input: &str) -> String {
    use md5::{Digest, Md5};
    let digest = Md5::digest(input.as_bytes());
    format!("{:x}", digest)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_errors_are_not_empty_content() {
        assert!(check_api_response(&serde_json::json!({"code": 0, "data": {}})).is_ok());
        for payload in [
            serde_json::json!({"code": -101}),
            serde_json::json!({"code": -352}),
            serde_json::json!({"code": 0, "data": {"v_voucher": "challenge"}}),
            serde_json::json!({"code": 0, "v_voucher": "challenge"}),
            serde_json::json!({"message": "not an API response"}),
        ] {
            assert!(check_api_response(&payload).is_err());
        }
    }

    #[test]
    fn test_urlencode() {
        assert_eq!(
            urlencode_component("机械键盘"),
            "%E6%9C%BA%E6%A2%B0%E9%94%AE%E7%9B%98"
        );
        assert_eq!(urlencode_component("a b"), "a+b");
        assert_eq!(urlencode_component("C++"), "C%2B%2B");
    }

    #[test]
    fn test_sanitize_value() {
        assert_eq!(sanitize_value("a!b'c(d)e*f"), "abcdef");
    }

    #[test]
    fn test_extract_key_component() {
        assert_eq!(
            extract_key_component(
                "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"
            ),
            "7cd084941338484aae1ad9425b84077c"
        );
    }

    #[test]
    fn test_wbi_sign_deterministic() {
        let params = vec![
            ("foo".to_string(), "值1".to_string()),
            ("bar".to_string(), " baz ".to_string()),
        ];
        let signed = BilibiliClient::sign_wbi_params(&params, "img_key", "sub_key");
        assert_eq!(signed.len(), params.len() + 2);
        assert!(signed.iter().any(|(k, _)| k == "w_rid"));
        assert!(signed.iter().any(|(k, _)| k == "wts"));
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(VideoInfo::parse_duration(&serde_json::json!("12:34")), 754);
        assert_eq!(
            VideoInfo::parse_duration(&serde_json::json!("1:02:03")),
            3723
        );
        assert_eq!(VideoInfo::parse_duration(&serde_json::json!(95)), 95);
    }

    #[test]
    fn test_normalize_subtitle_url() {
        assert_eq!(
            normalize_subtitle_url("//aisubtitle.hdslb.com/bfs/a.json"),
            "https://aisubtitle.hdslb.com/bfs/a.json"
        );
        assert_eq!(
            normalize_subtitle_url("https://i0.hdslb.com/x.json"),
            "https://i0.hdslb.com/x.json"
        );
    }

    #[test]
    fn test_extract_subtitle_text_joins_and_filters_empty() {
        let v = serde_json::json!({
            "body": [
                {"content": " 第一段 "},
                {"content": ""},
                {"content": "第二段"}
            ]
        });
        assert_eq!(extract_subtitle_text(&v), "第一段 第二段");
        assert_eq!(extract_subtitle_text(&serde_json::json!({})), "");
        assert_eq!(extract_subtitle_text(&serde_json::json!({"body": []})), "");
    }

    #[test]
    fn test_extract_comments_sorts_dedupes_truncates() {
        let v = serde_json::json!({
            "data": {"replies": [
                {"content": {"message": "低赞"}, "like": 1},
                {"content": {"message": "高赞"}, "like": 99},
                {"content": {"message": "高赞"}, "like": 5},
                {"content": {"message": "   "}, "like": 50}
            ]}
        });
        let out = extract_comments(&v, 5);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "高赞");
        assert_eq!(out[0].like, 99);
        assert_eq!(out[1].text, "低赞");
        // limit 生效
        assert_eq!(extract_comments(&v, 1).len(), 1);
        // 结构缺失时为空
        assert!(extract_comments(&serde_json::json!({}), 5).is_empty());
    }
}
