//! V2EX 匿名源 — 官方 API（无需登录；可选 PAT 增强暂未接入）
//!
//! 端点（官方 /api，IP 限频较严，仅在发现轮次调用一次热门）：
//! - `GET /api/topics/hot.json`：热门主题
//! - `GET /api/topics/latest.json`：最新主题
//!
//! 主题为纯文字卡片（无封面/时长）；node 命名空间归入 author 字段展示。

use async_trait::async_trait;
use serde_json::Value;

use crate::network::http_client::get_global_client;

use super::{ContentCandidate, SourceAdapter};

const BASE: &str = "https://www.v2ex.com";
const TIMEOUT_SECS: u64 = 15;

fn topic_to_candidate(item: &Value, source: &str) -> Option<ContentCandidate> {
    let id = item.get("id")?.as_i64()?;
    let title = item.get("title").and_then(|t| t.as_str())?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let node_title = item
        .get("node")
        .and_then(|n| n.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let member = item
        .get("member")
        .and_then(|m| m.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let author = if !node_title.is_empty() {
        format!("{} · @{}", node_title, member)
    } else {
        member.to_string()
    };
    Some(ContentCandidate {
        platform: "v2ex".to_string(),
        content_id: id.to_string(),
        title,
        description: item
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .chars()
            .take(150)
            .collect::<String>(),
        author,
        url: item
            .get("url")
            .and_then(|u| u.as_str())
            .map(|u| u.to_string())
            .unwrap_or_else(|| format!("{}/t/{}", BASE, id)),
        cover_url: String::new(),
        duration_secs: 0,
        view_count: item.get("replies").and_then(|r| r.as_u64()).unwrap_or(0),
        like_count: 0,
        pubdate: item.get("created").and_then(|c| c.as_i64()).unwrap_or(0),
        source: source.to_string(),
    })
}

/// V2EX 匿名适配器
///
/// 官方 API 未提供匿名搜索端点：search 走 latest（新帖池按画像评估筛选），
/// popular 走 hot。
pub struct V2exSource;

#[async_trait]
impl SourceAdapter for V2exSource {
    fn platform(&self) -> &'static str {
        "v2ex"
    }

    async fn search(&self, _query: &str, limit: usize) -> Vec<ContentCandidate> {
        self.fetch_topics("latest.json", limit).await
    }

    async fn popular(&self, limit: usize) -> Vec<ContentCandidate> {
        self.fetch_topics("hot.json", limit).await
    }
}

impl V2exSource {
    async fn fetch_topics(&self, endpoint: &str, limit: usize) -> Vec<ContentCandidate> {
        let client = get_global_client();
        let resp = client
            .get(format!("{}/api/topics/{}", BASE, endpoint))
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::debug!("[discovery:v2ex] {} 请求失败", endpoint);
            return Vec::new();
        };
        let Ok(payload) = resp.json::<Value>().await else {
            tracing::debug!("[discovery:v2ex] {} 响应解析失败", endpoint);
            return Vec::new();
        };
        let source = endpoint.trim_end_matches(".json");
        payload
            .as_array()
            .map(|arr| {
                arr.iter()
                    .take(limit)
                    .filter_map(|item| topic_to_candidate(item, source))
                    .collect()
            })
            .unwrap_or_default()
    }
}
