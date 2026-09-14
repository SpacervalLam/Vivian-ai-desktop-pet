//! Behavior Drive 层 — 行为驱动力（8 项）。
//!
//! 这是当前方案最关键的新增之一。Emotion 不直接变 Mood，而是先生成 Behavior Drive。
//! Agent 只看 Behavior Drive 最大的是什么，结合场景约束决定是否触发主动行为。
//!
//! 混合模式：
//! - LLM 决策路径：对话回复时，LLM 在 JSON 中直接产出 behavior_drive
//! - 规则决策路径：主动行为 tick 时，由 Needs/Emotion 规则推导 drive

use serde::{Deserialize, Serialize};

/// 行为驱动来源 — 标记 drive 是 LLM 产出还是规则推导
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DriveSource {
    Llm,
    Rule,
}

impl DriveSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DriveSource::Llm => "llm",
            DriveSource::Rule => "rule",
        }
    }
}

/// 行为驱动状态（8 项，0.0-1.0）
///
/// 值越高表示该行为倾向越强。由 Emotion + Needs 推导（规则路径）或由 LLM 直接产出。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BehaviorDrive {
    pub approach: f64, // 靠近
    pub avoid: f64,    // 回避
    pub explore: f64,  // 探索
    pub express: f64,  // 表达
    pub rest: f64,     // 休息
    pub observe: f64,  // 观察
    pub play: f64,     // 玩耍
    pub help: f64,     // 帮助
    pub source: DriveSource,
}

impl Default for BehaviorDrive {
    fn default() -> Self {
        Self {
            approach: 0.2,
            avoid: 0.1,
            explore: 0.3,
            express: 0.2,
            rest: 0.3,
            observe: 0.4,
            play: 0.2,
            help: 0.1,
            source: DriveSource::Rule,
        }
    }
}

/// 行为驱动标签 — 用于选择主动行为触发器
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DriveLabel {
    Approach,
    Avoid,
    Explore,
    Express,
    Rest,
    Observe,
    Play,
    Help,
}

impl DriveLabel {
    pub fn as_str(&self) -> &'static str {
        match self {
            DriveLabel::Approach => "approach",
            DriveLabel::Avoid => "avoid",
            DriveLabel::Explore => "explore",
            DriveLabel::Express => "express",
            DriveLabel::Rest => "rest",
            DriveLabel::Observe => "observe",
            DriveLabel::Play => "play",
            DriveLabel::Help => "help",
        }
    }
}

impl BehaviorDrive {
    /// 返回主导行为驱动（值最高者）及其标签
    pub fn dominant(&self) -> (DriveLabel, f64) {
        let items = [
            (DriveLabel::Approach, self.approach),
            (DriveLabel::Avoid, self.avoid),
            (DriveLabel::Explore, self.explore),
            (DriveLabel::Express, self.express),
            (DriveLabel::Rest, self.rest),
            (DriveLabel::Observe, self.observe),
            (DriveLabel::Play, self.play),
            (DriveLabel::Help, self.help),
        ];
        items
            .iter()
            .copied()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
    }

    /// 转为 prompt 友好的描述（供 LLM 参考当前行为倾向）
    pub fn to_prompt_desc(&self) -> String {
        format!(
            "靠近 {:.0}%  回避 {:.0}%  探索 {:.0}%  表达 {:.0}%  休息 {:.0}%  观察 {:.0}%  玩耍 {:.0}%  帮助 {:.0}%（来源:{}）",
            self.approach * 100.0,
            self.avoid * 100.0,
            self.explore * 100.0,
            self.express * 100.0,
            self.rest * 100.0,
            self.observe * 100.0,
            self.play * 100.0,
            self.help * 100.0,
            self.source.as_str()
        )
    }
}

// ============================================================================
// 规则决策路径 — 由 Needs/Emotion 推导 Behavior Drive
// ============================================================================

/// 规则驱动的 Behavior Drive 解析器
///
/// 在主动行为 tick（无对话时）调用，不触发 LLM。由当前 Needs/Emotion/Persona 推导 drive。
/// 规则是基于心理学的固定映射，不是 if-else 堆砌：
/// - 需求未满足 → 驱动满足该需求的行为
/// - 情绪状态 → 驱动应对该情绪的行为
pub struct RuleBasedDriveResolver;

impl RuleBasedDriveResolver {
    /// 根据当前心理状态推导 Behavior Drive
    pub fn resolve(
        needs: &super::needs::NeedsState,
        emotion: &super::emotion::EmotionState,
        persona: &super::persona::PersonaProfile,
    ) -> BehaviorDrive {
        let mut drive = BehaviorDrive::default();
        drive.source = DriveSource::Rule;

        // --- 需求驱动 ---
        // 归属需求高 + 孤独高 → 靠近
        if needs.belonging > 0.6 {
            drive.approach = (drive.approach + needs.belonging * 0.5).min(1.0);
        }
        // 自主需求高 → 探索/观察（独立活动）
        if needs.autonomy > 0.6 {
            drive.explore = (drive.explore + needs.autonomy * 0.4).min(1.0);
            drive.observe = (drive.observe + needs.autonomy * 0.2).min(1.0);
        }
        // 新鲜需求高 → 探索
        if needs.novelty > 0.6 {
            drive.explore = (drive.explore + needs.novelty * 0.5).min(1.0);
        }
        // 表达需求高 → 表达
        if needs.expression > 0.6 {
            drive.express = (drive.express + needs.expression * 0.6).min(1.0);
        }
        // 安全需求高 → 回避/休息（寻求安全）
        if needs.security > 0.7 {
            drive.avoid = (drive.avoid + needs.security * 0.4).min(1.0);
            drive.rest = (drive.rest + needs.security * 0.3).min(1.0);
        }

        // --- 情绪驱动 ---
        // 快乐高 → 玩耍
        if emotion.joy > 0.6 {
            drive.play = (drive.play + emotion.joy * 0.5).min(1.0);
            drive.express = (drive.express + emotion.joy * 0.3).min(1.0);
        }
        // 恐惧高 → 回避
        if emotion.fear > 0.6 {
            drive.avoid = (drive.avoid + emotion.fear * 0.6).min(1.0);
            drive.rest = (drive.rest + emotion.fear * 0.2).min(1.0);
        }
        // 悲伤高 → 休息/观察（低活力）
        if emotion.sadness > 0.5 {
            drive.rest = (drive.rest + emotion.sadness * 0.4).min(1.0);
            drive.observe = (drive.observe + emotion.sadness * 0.2).min(1.0);
        }
        // 愤怒高 → 回避（避免冲突）
        if emotion.anger > 0.6 {
            drive.avoid = (drive.avoid + emotion.anger * 0.4).min(1.0);
        }
        // 好奇高 → 探索
        if emotion.curiosity > 0.6 {
            drive.explore = (drive.explore + emotion.curiosity * 0.4).min(1.0);
            drive.observe = (drive.observe + emotion.curiosity * 0.3).min(1.0);
        }
        // 亲近高 → 靠近/帮助（不再依赖 emotion.trust）
        if emotion.closeness > 0.6 {
            drive.approach = (drive.approach + emotion.closeness * 0.4).min(1.0);
            drive.help = (drive.help + emotion.closeness * 0.2).min(1.0);
        }
        // 孤独高 → 靠近
        if emotion.loneliness > 0.6 {
            drive.approach = (drive.approach + emotion.loneliness * 0.4).min(1.0);
        }

        // --- Persona 调制 ---
        // 独立性高 → 降低 approach，提升 explore/observe
        let indep = persona.traits.independence;
        if indep > 0.6 {
            drive.approach *= 1.0 - (indep - 0.6) * 0.5;
            drive.explore = (drive.explore + 0.1).min(1.0);
        }
        // 社交性高 → 提升 approach/express
        let soc = persona.traits.sociability;
        if soc > 0.6 {
            drive.approach = (drive.approach + (soc - 0.6) * 0.5).min(1.0);
            drive.express = (drive.express + (soc - 0.6) * 0.3).min(1.0);
        }

        drive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::needs::NeedsState;
    use super::super::emotion::EmotionState;
    use super::super::persona::PersonaProfile;

    #[test]
    fn test_loneliness_drives_approach() {
        let needs = NeedsState::default();
        let emotion = EmotionState {
            loneliness: 0.8,
            ..Default::default()
        };
        let persona = PersonaProfile::default();
        let drive = RuleBasedDriveResolver::resolve(&needs, &emotion, &persona);
        assert!(drive.approach > 0.3);
    }

    #[test]
    fn test_fear_drives_avoid() {
        let needs = NeedsState::default();
        let emotion = EmotionState {
            fear: 0.8,
            ..Default::default()
        };
        let persona = PersonaProfile::default();
        let drive = RuleBasedDriveResolver::resolve(&needs, &emotion, &persona);
        assert!(drive.avoid > 0.4);
    }

    #[test]
    fn test_rule_source() {
        let drive = RuleBasedDriveResolver::resolve(
            &NeedsState::default(),
            &EmotionState::default(),
            &PersonaProfile::default(),
        );
        assert_eq!(drive.source, DriveSource::Rule);
    }
}
