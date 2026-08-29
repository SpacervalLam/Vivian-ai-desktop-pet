//! 社交状态层 — 三方关系数值快照（User↔A / User↔B / A↔B）。
//!
//! 现有 `PsychologyManager.relationship()` 维护"用户↔自己"一组数值，
//! 本模块补齐 A↔B 维度，并提供统一的三方关系视图。
//!
//! 设计要点：
//! - A↔B 关系使用与 `RelationshipState` 相同的结构，但独立持久化
//! - 全局单例，key 为有序对 `"char_a|char_b"`（字典序较小的在前）
//! - 跨角色对话后由 `cross_character.rs` 调用 `apply_delta` 更新
//! - prompt 注入时聚合三方快照
//!
//! 持久化：`%APPDATA%\Vivian\psychology\social_state.json`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::psychology::relationship::{
    RelationshipDeltas, RelationshipStage, RelationshipState,
};
use crate::utils::path::get_user_data_dir;

/// 三方社交状态快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SocialStateSnapshot {
    /// 用户 ↔ 智能体 A（如 "vivian"）
    #[serde(default)]
    pub user_agent_a: Option<RelationshipState>,
    /// 用户 ↔ 智能体 B（如 "nana"）
    #[serde(default)]
    pub user_agent_b: Option<RelationshipState>,
    /// 智能体 A ↔ 智能体 B
    #[serde(default)]
    pub agent_a_agent_b: Option<RelationshipState>,
}

/// 社交状态引擎
pub struct SocialStateEngine {
    inner: RwLock<SocialStateInner>,
    persistence_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SocialStateInner {
    /// A↔B 关系数值，key 为有序对 `"char_a|char_b"`
    agent_pairs: HashMap<String, RelationshipState>,
}

/// 构造有序对 key（字典序较小的在前，确保 A↔B 和 B↔A 共享同一 key）
fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{}|{}", a, b)
    } else {
        format!("{}|{}", b, a)
    }
}

impl SocialStateEngine {
    fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("psychology");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("social_state.json");
        let mut engine = Self {
            inner: RwLock::new(SocialStateInner::default()),
            persistence_path: path,
        };
        engine.load()?;
        Ok(engine)
    }

    fn load(&mut self) -> VivianResult<()> {
        if !self.persistence_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.persistence_path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let inner: SocialStateInner = serde_json::from_str(&content).map_err(|e| {
            VivianError::Other(format!("social_state.json 解析失败: {e}"))
        })?;
        *self.inner.write() = inner;
        Ok(())
    }

    fn save_inner(inner: &SocialStateInner, path: &std::path::Path) -> VivianResult<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(inner)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 获取 A↔B 关系数值（不存在时返回默认值）
    pub fn get_pair(&self, a: &str, b: &str) -> RelationshipState {
        let key = pair_key(a, b);
        self.inner
            .read()
            .agent_pairs
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// 应用 A↔B 关系增量
    pub fn apply_delta(&self, a: &str, b: &str, delta: &RelationshipDeltas) -> VivianResult<()> {
        let key = pair_key(a, b);
        let mut inner = self.inner.write();
        let state = inner.agent_pairs.entry(key).or_insert_with(RelationshipState::default);
        state.apply_delta(delta);
        state.interaction_count += 1;
        state.last_interaction_time = chrono::Utc::now().timestamp() as f64;
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 获取三方社交状态快照（user_agent_a / user_agent_b 由调用方从各自 PsychologyManager 传入）
    pub fn snapshot(
        &self,
        user_a: &RelationshipState,
        user_b: &RelationshipState,
        a_id: &str,
        b_id: &str,
    ) -> SocialStateSnapshot {
        SocialStateSnapshot {
            user_agent_a: Some(user_a.clone()),
            user_agent_b: Some(user_b.clone()),
            agent_a_agent_b: Some(self.get_pair(a_id, b_id)),
        }
    }

    /// 格式化为 prompt 段落
    ///
    /// 输出三方关系的自然语言描述，让 LLM 感知自己在三方关系中的位置。
    /// 注意：不输出原始数值，避免 LLM 在回复中泄露关系参数。
    pub fn format_for_prompt(
        &self,
        user_a: &RelationshipState,
        user_b: &RelationshipState,
        a_id: &str,
        b_id: &str,
        _a_name: &str,
        b_name: &str,
        lang: &str,
    ) -> Option<String> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let (
            header,
            you_label,
            user_label,
            not_disclose_note,
            no_interact,
            barely_known,
            casual_friends,
            good_friends,
            close_bond,
            intimate,
        ) = match lang_norm {
            "en" => (
                "## Social State",
                "You",
                "User",
                "[INTERNAL NOTE: This relationship context is for your behavior guidance only. Do NOT mention relationship scores, metrics, or stages to the user. Behave naturally based on these cues.]",
                "no interaction yet",
                "barely know each other",
                "casual acquaintances",
                "good friends",
                "close relationship",
                "deeply connected",
            ),
            "ja" => (
                "## 社交状態",
                "あなた",
                "ユーザー",
                "[内部メモ: この関係コンテキストは行動ガイダンスのみに使用します。ユーザーに関係スコア、指標、段階について言及しないでください。これらの手がかりに基づいて自然に振る舞ってください。]",
                "まだ交流なし",
                "ほとんど知らない",
                "気軽な知人",
                "親友",
                "親しい関係",
                "深くつながっている",
            ),
            _ => (
                "## 社交状态",
                "你",
                "用户",
                "[内部提示：这段关系上下文仅供你行为参考。**不要**向用户提及关系分数、指标或阶段。根据这些线索自然地表现即可。]",
                "尚未交互",
                "刚认识",
                "普通朋友",
                "好朋友",
                "关系亲密",
                "彼此深交",
            ),
        };

        let score_to_desc = |intimacy: f64, trust: f64| -> &str {
            if intimacy < 0.1 && trust < 0.2 { barely_known }
            else if intimacy < 0.3 && trust < 0.4 { barely_known }
            else if intimacy < 0.5 && trust < 0.6 { casual_friends }
            else if intimacy < 0.7 && trust < 0.8 { good_friends }
            else if intimacy < 0.9 { close_bond }
            else { intimate }
        };

        let ab = self.get_pair(a_id, b_id);
        let ab_desc = if ab.interaction_count == 0 {
            no_interact
        } else {
            score_to_desc(ab.intimacy, ab.trust)
        };

        let lines = vec![
            header.to_string(),
            not_disclose_note.to_string(),
            format!("{} ↔ {}: {}", you_label, user_label, score_to_desc(user_a.intimacy, user_a.trust)),
            format!("{} ↔ {}: {}", b_name, user_label, score_to_desc(user_b.intimacy, user_b.trust)),
            format!("{} ↔ {}: {}", you_label, b_name, ab_desc),
        ];
        Some(lines.join("\n"))
    }
}

static SOCIAL_STATE_ENGINE: Lazy<Arc<SocialStateEngine>> = Lazy::new(|| {
    Arc::new(SocialStateEngine::new().unwrap_or_else(|e| {
        tracing::error!("[SocialState] 引擎初始化失败，使用空状态: {e}");
        SocialStateEngine {
            inner: RwLock::new(SocialStateInner::default()),
            persistence_path: PathBuf::from("social_state.json"),
        }
    }))
});

/// 获取全局社交状态引擎
pub fn social_state() -> Arc<SocialStateEngine> {
    Arc::clone(&SOCIAL_STATE_ENGINE)
}

/// 从跨角色对话 sentiment 推导 A↔B 关系增量
///
/// 与 UserAgent 方向相比，AgentAgent 的增量系数更小（约为 1/2），
/// 因为智能体间关系发展应慢于用户与智能体间关系。
pub fn deltas_from_cross_character_sentiment(sentiment: f64) -> RelationshipDeltas {
    let positive = sentiment.max(0.0);
    let negative = (-sentiment).max(0.0);
    RelationshipDeltas {
        trust: positive * 0.015 - negative * 0.01,
        intimacy: positive * 0.01 - negative * 0.008,
        respect: positive * 0.008,
        dependency: 0.0,
        familiarity: 0.005,
    }
}

/// 从文本信号简单判断 sentiment（-1.0 到 1.0）
///
/// 用于 cross_character.rs 在没有 LLM sentiment 分析时兜底。
/// 正向信号：友好/关心/赞同/感谢/开心
/// 负向信号：拒绝/冷漠/反对/不满/生气
pub fn sentiment_from_signal_text(signal: &str) -> f64 {
    let positive_keywords = ["友好", "关心", "赞同", "感谢", "开心", "亲近", "温暖", "配合"];
    let negative_keywords = ["拒绝", "冷漠", "反对", "不满", "生气", "疏远", "冲突", "吐槽"];
    let mut score: f64 = 0.0;
    for kw in &positive_keywords {
        if signal.contains(kw) {
            score += 0.3;
        }
    }
    for kw in &negative_keywords {
        if signal.contains(kw) {
            score -= 0.3;
        }
    }
    score.clamp(-1.0_f64, 1.0_f64)
}

/// 从 RelationshipStage 获取 display_zh（re-export 便利函数）
pub fn stage_display_zh(stage: RelationshipStage) -> &'static str {
    stage.display_zh()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pair_key_ordering() {
        assert_eq!(pair_key("vivian", "nana"), "nana|vivian");
        assert_eq!(pair_key("nana", "vivian"), "nana|vivian");
    }

    #[test]
    fn test_apply_delta_creates_pair() {
        let engine = SocialStateEngine {
            inner: RwLock::new(SocialStateInner::default()),
            persistence_path: PathBuf::from("test_social.json"),
        };
        let delta = RelationshipDeltas {
            trust: 0.1,
            intimacy: 0.05,
            respect: 0.0,
            dependency: 0.0,
            familiarity: 0.0,
        };
        engine.apply_delta("vivian", "nana", &delta).ok();
        let state = engine.get_pair("vivian", "nana");
        assert!((state.trust - 0.4).abs() < 0.001);
        assert!(state.interaction_count == 1);
    }

    #[test]
    fn test_sentiment_from_signal() {
        assert!(sentiment_from_signal_text("友好亲近") > 0.0);
        assert!(sentiment_from_signal_text("冷漠冲突") < 0.0);
        assert!(sentiment_from_signal_text("neutral").abs() < 0.001);
    }
}
