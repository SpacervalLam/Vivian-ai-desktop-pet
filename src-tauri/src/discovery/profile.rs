//! 兴趣画像层 — 发现阶段的内容口味模型
//!
//! 轻量版兴趣画像：兴趣域（含权重与生命周期状态）+ 不喜欢主题 + 探索开放度。
//! 人格画像（MBTI/认知风格/深层需求）由 Vivian 已有的 UserFacts / UserModel 承担，不在此重复。
//!
//! 种子合成：首次加载时从同角色的 `user_facts.json`（hobby / favorite_game /
//! recent_preferences）读取显式兴趣作为初始画像；
//! 之后随推荐反馈与兴趣探针 promote 演化。
//!
//! 存储路径：`<用户数据目录>/characters/<char_id>/discovery/interest_profile.json`

use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::utils::path::get_character_data_dir;

/// 兴趣域（粗粒度兴趣方向 + 权重 + 证据生命周期）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestDomain {
    pub domain: String,
    /// 权重 0-1（越高越核心）
    pub weight: f64,
    /// 来源：seed（user_facts 种子）/ feedback（推荐反馈）/ probe（探针升级）
    pub source: String,
    /// 证据计数（正反馈 / 命中累计）
    pub evidence_count: u32,
    /// 最近证据时间（Unix 秒）
    pub last_evidence_at: i64,
    /// 生命周期：active | decaying | archived
    pub state: String,
}

/// 兴趣画像
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InterestProfile {
    /// 兴趣域列表（按权重降序使用）
    pub interests: Vec<InterestDomain>,
    /// 不喜欢的主题（dislike 反馈累积）
    pub disliked_topics: Vec<String>,
    /// 探索开放度 0-1（高 = 更愿意接受陌生领域；dislike 反馈降低，like 跨域反馈升高）
    pub exploration_openness: f64,
    /// 是否已完成 user_facts 种子合成
    pub seeded: bool,
    pub updated_at: i64,
}

impl InterestProfile {
    /// 种子合成：从角色 user_facts.json 提取显式兴趣
    pub fn seed_from_user_facts(&mut self, char_id: &str) {
        if self.seeded {
            return;
        }
        let facts_path = get_character_data_dir(char_id).join("user_facts.json");
        let Ok(raw) = std::fs::read_to_string(&facts_path) else {
            self.seeded = true;
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            self.seeded = true;
            return;
        };

        let mut seeds: Vec<String> = Vec::new();

        // L0.5 结构化偏好：hobby / favorite_game
        for key in ["hobby", "favorite_game"] {
            if let Some(content) = value
                .get("basic_data")
                .and_then(|b| b.get(key))
                .and_then(|f| f.get("content"))
                .and_then(|c| c.as_str())
            {
                seeds.push(content.to_string());
            }
        }
        // L1 近期偏好
        if let Some(prefs) = value
            .get("recent_state")
            .and_then(|r| r.get("recent_preferences"))
            .and_then(|p| p.as_array())
        {
            for p in prefs.iter().take(5) {
                if let Some(s) = p.as_str() {
                    seeds.push(s.to_string());
                }
            }
        }
        // L2 自由事实中标签为 hobby 的条目
        if let Some(customs) = value.get("custom_facts").and_then(|c| c.as_array()) {
            for fact in customs.iter().take(50) {
                let is_hobby = fact
                    .get("fact_type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "hobby")
                    .unwrap_or(false);
                if is_hobby {
                    if let Some(c) = fact.get("content").and_then(|c| c.as_str()) {
                        seeds.push(c.to_string());
                    }
                }
            }
        }

        for seed in seeds {
            let seed = seed.trim();
            if seed.is_empty() || seed.chars().count() > 30 {
                continue; // 过长的自由文本不适合做兴趣域
            }
            self.upsert_interest(seed, 0.7, "seed");
        }
        self.seeded = true;
        self.updated_at = Utc::now().timestamp();
    }

    /// 新增或强化兴趣域
    pub fn upsert_interest(&mut self, domain: &str, weight: f64, source: &str) {
        let domain = domain.trim();
        if domain.is_empty() {
            return;
        }
        let now = Utc::now().timestamp();
        if let Some(existing) = self.interests.iter_mut().find(|i| i.domain == domain) {
            existing.weight = (existing.weight.max(weight)).min(1.0);
            existing.evidence_count += 1;
            existing.last_evidence_at = now;
            if existing.state != "active" {
                existing.state = "active".to_string();
            }
            return;
        }
        self.interests.push(InterestDomain {
            domain: domain.to_string(),
            weight: weight.clamp(0.0, 1.0),
            source: source.to_string(),
            evidence_count: 1,
            last_evidence_at: now,
            state: "active".to_string(),
        });
        // 兴趣域上限 30，超出按权重截断
        if self.interests.len() > 30 {
            self.interests.sort_by(|a, b| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.interests.truncate(30);
        }
    }

    /// 降权兴趣域（dislike 反馈时对同 topic_group 的兴趣降权）
    pub fn decay_interest(&mut self, domain: &str, amount: f64) {
        if let Some(existing) = self.interests.iter_mut().find(|i| i.domain == domain) {
            existing.weight = (existing.weight - amount).max(0.0);
            if existing.weight < 0.15 {
                existing.state = "archived".to_string();
            }
        }
    }

    /// 加入不喜欢主题（去重，上限 20）
    pub fn add_dislike(&mut self, topic: &str) {
        let topic = topic.trim();
        if topic.is_empty() || self.disliked_topics.iter().any(|t| t == topic) {
            return;
        }
        self.disliked_topics.push(topic.to_string());
        if self.disliked_topics.len() > 20 {
            self.disliked_topics.drain(..self.disliked_topics.len() - 20);
        }
        // dislike 同时降低探索开放度
        self.exploration_openness = (self.exploration_openness - 0.05).max(0.1);
    }

    /// 画像 → LLM prompt 上下文（发现词生成 / 评估 / 推荐文案共用）
    pub fn to_prompt_context(&self) -> String {
        let mut parts = Vec::new();
        let active: Vec<&InterestDomain> = self
            .interests
            .iter()
            .filter(|i| i.state == "active")
            .collect();
        if !active.is_empty() {
            let lines: Vec<String> = active
                .iter()
                .take(10)
                .map(|i| format!("- {}（权重 {:.1}）", i.domain, i.weight))
                .collect();
            parts.push(format!("## 用户兴趣\n{}", lines.join("\n")));
        }
        if !self.disliked_topics.is_empty() {
            parts.push(format!(
                "## 用户不喜欢\n{}",
                self.disliked_topics
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        let openness = if self.exploration_openness > 0.0 {
            self.exploration_openness
        } else {
            0.5
        };
        parts.push(format!(
            "## 探索开放度\n{:.1}（0-1，越高越愿意接受陌生领域）",
            openness
        ));
        parts.join("\n\n")
    }

    /// 顶层兴趣域列表（发现词生成的锚点）
    pub fn top_interest_names(&self, n: usize) -> Vec<String> {
        let mut active: Vec<&InterestDomain> = self
            .interests
            .iter()
            .filter(|i| i.state == "active")
            .collect();
        active.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        active.iter().take(n).map(|i| i.domain.clone()).collect()
    }

    fn profile_path(char_id: &str) -> PathBuf {
        get_character_data_dir(char_id)
            .join("discovery")
            .join("interest_profile.json")
    }

    pub fn load(char_id: &str) -> Self {
        let path = Self::profile_path(char_id);
        let mut profile: Self =
            crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        if profile.exploration_openness <= 0.0 {
            profile.exploration_openness = 0.5;
        }
        profile.seed_from_user_facts(char_id);
        profile
    }

    pub fn save(&self, char_id: &str) {
        let path = Self::profile_path(char_id);
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
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_decay() {
        let mut p = InterestProfile::default();
        p.upsert_interest("机械键盘", 0.7, "seed");
        p.upsert_interest("机械键盘", 0.9, "seed");
        assert_eq!(p.interests.len(), 1);
        assert!((p.interests[0].weight - 0.9).abs() < 1e-6);
        assert_eq!(p.interests[0].evidence_count, 2);

        p.decay_interest("机械键盘", 0.8);
        assert!(p.interests[0].weight < 0.2);
        assert_eq!(p.interests[0].state, "archived");
    }

    #[test]
    fn test_dislike_and_openness() {
        let mut p = InterestProfile::default();
        p.exploration_openness = 0.5;
        p.add_dislike("营销号");
        p.add_dislike("营销号");
        assert_eq!(p.disliked_topics.len(), 1);
        assert!(p.exploration_openness < 0.5);
    }

    #[test]
    fn test_prompt_context() {
        let mut p = InterestProfile::default();
        p.upsert_interest("机械键盘", 0.8, "seed");
        p.add_dislike("低质短视频");
        let ctx = p.to_prompt_context();
        assert!(ctx.contains("机械键盘"));
        assert!(ctx.contains("低质短视频"));
        assert!(ctx.contains("探索开放度"));
    }

    #[test]
    fn test_top_interests_sorted() {
        let mut p = InterestProfile::default();
        p.upsert_interest("a", 0.3, "seed");
        p.upsert_interest("b", 0.9, "seed");
        p.upsert_interest("c", 0.6, "seed");
        let top = p.top_interest_names(2);
        assert_eq!(top, vec!["b".to_string(), "c".to_string()]);
    }
}
