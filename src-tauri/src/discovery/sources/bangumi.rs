//! Bangumi 匿名源 — 官方 v0 只读 API（无需 Cookie/token）
//!
//! 端点：
//! - `POST /v0/search/subjects`：条目搜索（书/动画/游戏/三次元全类型）
//! - `GET /v0/subjects?type=2&sort=rank`：高分榜单（动画）
//! - `GET /v0/users/{username}/collections`：公开收藏（画像初始化信号）
//!
//! API 礼仪：必须携带可识别的 User-Agent；条目为文字卡片（无时长/播放数）。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::network::http_client::get_global_client;

use super::{ContentCandidate, SourceAdapter};

const BASE: &str = "https://api.bgm.tv";
/// Bangumi API 礼仪要求的可识别 UA
const UA: &str = "vivian-desktop-pet/1.0 (vivian discovery)";

fn subject_url(id: i64) -> String {
    format!("https://bgm.tv/subject/{}", id)
}

fn subject_to_candidate(item: &Value, source: &str) -> Option<ContentCandidate> {
    let id = item.get("id")?.as_i64()?;
    let name_cn = item
        .get("name_cn")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    let title = if !name_cn.is_empty() {
        name_cn.to_string()
    } else {
        name.to_string()
    };
    if title.is_empty() {
        return None;
    }
    let rating = item.get("rating").cloned().unwrap_or(Value::Null);
    let score = rating.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
    let total = rating.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
    let collection = item.get("collection").cloned().unwrap_or(Value::Null);
    let collect_count = collection
        .get("total")
        .and_then(|c| c.as_u64())
        .or_else(|| collection.get("collect").and_then(|c| c.as_u64()))
        .unwrap_or(0);
    Some(ContentCandidate {
        platform: "bangumi".to_string(),
        content_id: id.to_string(),
        title,
        description: item
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .chars()
            .take(150)
            .collect::<String>(),
        author: String::new(),
        url: subject_url(id),
        cover_url: item
            .get("images")
            .and_then(|i| i.get("large").or(i.get("common")))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string(),
        duration_secs: 0,
        // 收藏人数作热度代理（条目无播放数）
        view_count: collect_count.max(total),
        like_count: (score * 10.0) as u64,
        pubdate: item
            .get("date")
            .and_then(|d| d.as_str())
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .and_then(|dt| dt.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp()))
            .unwrap_or(0),
        source: source.to_string(),
    })
}

/// Bangumi 匿名适配器
pub struct BangumiSource;

#[async_trait]
impl SourceAdapter for BangumiSource {
    fn platform(&self) -> &'static str {
        "bangumi"
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<ContentCandidate> {
        let client = get_global_client();
        let body = json!({
            "keyword": query,
            "sort": "match",
            "filter": {},
        });
        let resp = client
            .post(format!("{}/v0/search/subjects?limit={}", BASE, limit.min(50)))
            .header("User-Agent", UA)
            .json(&body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::debug!("[discovery:bangumi] 搜索请求失败");
            return Vec::new();
        };
        let Ok(payload) = resp.json::<Value>().await else {
            tracing::debug!("[discovery:bangumi] 搜索响应解析失败");
            return Vec::new();
        };
        payload
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| subject_to_candidate(item, &format!("search:{}", query)))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn popular(&self, limit: usize) -> Vec<ContentCandidate> {
        let client = get_global_client();
        let resp = client
            .get(format!(
                "{}/v0/subjects?type=2&sort=rank&limit={}",
                BASE,
                limit.min(50)
            ))
            .header("User-Agent", UA)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::debug!("[discovery:bangumi] 榜单请求失败");
            return Vec::new();
        };
        let Ok(payload) = resp.json::<Value>().await else {
            tracing::debug!("[discovery:bangumi] 榜单响应解析失败");
            return Vec::new();
        };
        payload
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| subject_to_candidate(item, "ranked"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 读取公开用户收藏（画像初始化信号）
///
/// 返回「看过/在看」条目名列表（type=2 看过 / 3 在看；1 想看不算已消费信号）。
/// 用户名不存在或收藏为私密时返回空列表。
pub async fn fetch_public_collections(username: &str, limit: usize) -> Vec<String> {
    let client = get_global_client();
    let resp = client
        .get(format!(
            "{}/v0/users/{}/collections?limit={}",
            BASE,
            username,
            limit.min(100)
        ))
        .header("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await;
    let Ok(resp) = resp else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        tracing::debug!("[discovery:bangumi] 收藏读取失败（用户名或权限）: {}", username);
        return Vec::new();
    }
    let Ok(payload) = resp.json::<Value>().await else {
        return Vec::new();
    };
    payload
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|c| {
                    // 2 = 看过，3 = 在看（已消费信号）；1 想看 / 4 搁置 / 5 抛弃不算
                    matches!(c.get("type").and_then(|t| t.as_i64()), Some(2) | Some(3))
                })
                .filter_map(|c| {
                    c.get("subject")
                        .and_then(|s| s.get("name_cn").or(s.get("name")))
                        .and_then(|n| n.as_str())
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                })
                .collect()
        })
        .unwrap_or_default()
}
