//! 内容库存 + 已推荐账本 + 反馈记录 — 按角色隔离的 JSON 持久化
//!
//! 存储路径：`<用户数据目录>/characters/<char_id>/discovery/content_store.json`
//! 写入策略：原子写（tmp + rename）。

use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::path::get_character_data_dir;

use super::bilibili::VideoInfo;
use super::sources::ContentCandidate;

/// 库存内容条目（发现并经 LLM 评估入库后的形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentItem {
    /// 平台内容 ID（B 站 bvid / Bangumi subject_id / V2EX topic_id）
    pub bvid: String,
    /// 平台标识：bilibili / bangumi / v2ex / ...（旧数据缺省 bilibili）
    #[serde(default = "default_platform")]
    pub platform: String,
    pub title: String,
    pub description: String,
    pub up_name: String,
    pub cover_url: String,
    pub url: String,
    pub duration_secs: u64,
    pub view_count: u64,
    pub like_count: u64,
    pub pubdate: i64,
    /// 发现来源（search:{query} / hot / ranked / latest）
    pub source: String,
    /// LLM 评估分（0-1）
    pub score: f64,
    /// LLM 评估理由（内部诊断，一句精炼中文）
    pub reason: String,
    /// 粗粒度主题分类（2-4 词，用于推荐去重）
    pub topic_group: String,
    pub added_at: i64,
    /// 被推荐次数
    pub recommended_count: u32,
    /// 最近一次被推荐的时间（Unix 秒）
    pub last_recommended_at: i64,
    /// 用户反馈：like / dislike / neutral（未反馈为空）
    pub feedback: String,
}

fn default_platform() -> String {
    "bilibili".to_string()
}

impl ContentItem {
    pub fn from_video(video: &VideoInfo, score: f64, reason: String, topic_group: String) -> Self {
        Self::from_candidate(
            &ContentCandidate::from_bilibili(video),
            score,
            reason,
            topic_group,
        )
    }

    /// 通用构造（多平台候选入库）
    pub fn from_candidate(
        c: &ContentCandidate,
        score: f64,
        reason: String,
        topic_group: String,
    ) -> Self {
        Self {
            bvid: c.content_id.clone(),
            platform: c.platform.clone(),
            title: c.title.clone(),
            description: c.description.clone(),
            up_name: c.author.clone(),
            cover_url: c.cover_url.clone(),
            url: c.url.clone(),
            duration_secs: c.duration_secs,
            view_count: c.view_count,
            like_count: c.like_count,
            pubdate: c.pubdate,
            source: c.source.clone(),
            score,
            reason,
            topic_group,
            added_at: Utc::now().timestamp(),
            recommended_count: 0,
            last_recommended_at: 0,
            feedback: String::new(),
        }
    }
}

/// 内容库存（库存 + 反馈账本）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentStore {
    /// 已入库内容（按 bvid 去重由 admit 保证）
    pub items: Vec<ContentItem>,
    /// 已推荐过的 bvid 账本，用于「换一批」三层排除
    pub recommended_ledger: Vec<String>,
}

impl ContentStore {
    fn store_path(char_id: &str) -> PathBuf {
        get_character_data_dir(char_id)
            .join("discovery")
            .join("content_store.json")
    }

    pub fn load(char_id: &str) -> Self {
        let path = Self::store_path(char_id);
        crate::utils::fs::load_json_or_backup(&path).unwrap_or_default()
    }

    pub fn save(&self, char_id: &str) {
        let path = Self::store_path(char_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// 内容是否已在库存或账本中（bilibili 兼容入口）
    pub fn contains_bvid(&self, bvid: &str) -> bool {
        self.contains_id("bilibili", bvid)
    }

    /// 多平台去重键检查（旧账本纯 bvid 视为 bilibili）
    pub fn contains_id(&self, platform: &str, content_id: &str) -> bool {
        let item_hit = self
            .items
            .iter()
            .any(|i| i.bvid == content_id && (i.platform == platform || i.platform.is_empty()));
        if item_hit {
            return true;
        }
        let key = format!("{}:{}", platform, content_id);
        self.recommended_ledger.iter().any(|b| {
            b == &key
                // 旧格式纯 ID（无平台前缀）兼容 bilibili
                || (platform == "bilibili" && b == content_id)
        })
    }

    /// 入库新条目；超过库存上限时按分数淘汰（正向反馈条目优先保留）
    pub fn admit(&mut self, item: ContentItem, max_items: usize) {
        if self.contains_id(&item.platform, &item.bvid) {
            return;
        }
        self.items.push(item);
        if self.items.len() > max_items {
            // 未收正向反馈的条目按分数降序保留前 max_items
            let mut liked: Vec<ContentItem> = self
                .items
                .iter()
                .filter(|i| i.feedback == "like")
                .cloned()
                .collect();
            let mut rest: Vec<ContentItem> = self
                .items
                .iter()
                .filter(|i| i.feedback != "like")
                .cloned()
                .collect();
            rest.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            liked.append(&mut rest);
            liked.truncate(max_items);
            self.items = liked;
        }
    }

    /// 标记推荐（入账本 + 计数）
    /// entries 形如 `platform:content_id`；bilibili 兼容纯 bvid。
    pub fn mark_recommended(&mut self, bvids: &[String]) {
        for entry in bvids {
            if !self.recommended_ledger.iter().any(|b| b == entry) {
                self.recommended_ledger.push(entry.clone());
            }
            let (platform, id) = match entry.split_once(':') {
                Some((p, i)) => (p.to_string(), i.to_string()),
                None => (String::new(), entry.clone()),
            };
            if let Some(item) = self
                .items
                .iter_mut()
                .find(|i| i.bvid == id && (platform.is_empty() || i.platform == platform))
            {
                item.recommended_count += 1;
                item.last_recommended_at = Utc::now().timestamp();
            }
        }
        // 账本上限：保留最近 300 条
        let len = self.recommended_ledger.len();
        if len > 300 {
            self.recommended_ledger.drain(..len - 300);
        }
    }

    /// 记录用户反馈（按 URL 或内容 ID 匹配），返回命中的 (bvid, topic_group)
    pub fn apply_feedback(&mut self, target: &str, feedback: &str) -> Option<(String, String)> {
        let target = target.trim();
        let item = if target.starts_with("http") {
            // URL 匹配（跨平台通用）
            self.items.iter_mut().find(|i| i.url == target)?
        } else {
            let (platform, id) = match target.split_once(':') {
                Some((p, i)) => (p.to_string(), i.to_string()),
                None => (String::new(), target.to_string()),
            };
            self.items
                .iter_mut()
                .find(|i| i.bvid == id && (platform.is_empty() || i.platform == platform))?
        };
        item.feedback = feedback.to_string();
        Some((item.bvid.clone(), item.topic_group.clone()))
    }

    /// 未推荐且未反馈的可用库存数
    pub fn available_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.recommended_count == 0 && i.feedback.is_empty())
            .count()
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn item(bvid: &str, score: f64, topic: &str) -> ContentItem {
        ContentItem::from_video(
            &VideoInfo {
                bvid: bvid.to_string(),
                title: format!("标题{}", bvid),
                description: String::new(),
                up_name: "up".to_string(),
                cover_url: String::new(),
                duration_secs: 100,
                view_count: 1000,
                like_count: 100,
                pubdate: 0,
                source: "popular".to_string(),
            },
            score,
            String::new(),
            topic.to_string(),
        )
    }

    #[test]
    fn test_admit_dedup() {
        let mut store = ContentStore::default();
        store.admit(item("BV1", 0.8, "科技"), 100);
        store.admit(item("BV1", 0.9, "科技"), 100);
        assert_eq!(store.items.len(), 1);
    }

    #[test]
    fn test_ledger_blocks_reconsume() {
        let mut store = ContentStore::default();
        store.admit(item("BV1", 0.8, "科技"), 100);
        assert!(!store.contains_bvid("BV1"));
        store.mark_recommended(&["BV1".to_string()]);
        assert!(store.contains_bvid("BV1"));
    }

    #[test]
    fn test_apply_feedback_by_url() {
        let mut store = ContentStore::default();
        store.admit(item("BV1abc", 0.8, "科技"), 100);
        let hit = store.apply_feedback("https://www.bilibili.com/video/BV1abc", "like");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().0, "BV1abc");
        assert_eq!(store.items[0].feedback, "like");
    }

    #[test]
    fn test_capacity_eviction() {
        let mut store = ContentStore::default();
        for i in 0..10 {
            store.admit(item(&format!("BV{}", i), i as f64 / 10.0, "科技"), 5);
        }
        assert!(store.items.len() <= 5);
        assert!(store.items.iter().any(|i| i.bvid == "BV9"));
        assert!(!store.items.iter().any(|i| i.bvid == "BV0"));
    }
}
