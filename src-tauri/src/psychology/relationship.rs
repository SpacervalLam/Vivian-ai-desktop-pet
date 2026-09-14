//! Relationship 层 — 关系状态（5 维 + 6 阶段永久阶段 + 3 种临时态 + 里程碑）。
//!
//! 整合了原 RelationshipManager 的优秀部分：
//! - 5 维关系状态（trust/intimacy/respect/dependency/familiarity）— 心理学驱动
//! - 6 阶段永久阶段（Stranger → Acquainted → Familiar → Close → Intimate → Soulmate）
//! - 3 种临时态（Soothing/LowActivity/Reconnecting）— 覆盖永久态行为
//! - 里程碑系统（自动 + 自定义）
//! - 缺席衰减机制
//! - 阶段策略（语气/主动性/记忆深度等）
//! - 交互统计（interaction_count/consecutive_positive/negative）
//!
//! 关系状态由 Appraisal + Emotion 更新，反映 Vivian 与用户的互动历史。

use serde::{Deserialize, Serialize};

// ============================================================================
// 阶段定义
// ============================================================================

/// 永久关系阶段（6 阶段）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipStage {
    Stranger,
    Acquainted,
    Familiar,
    Close,
    Intimate,
    Soulmate,
}

impl Default for RelationshipStage {
    fn default() -> Self {
        RelationshipStage::Stranger
    }
}

impl RelationshipStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationshipStage::Stranger => "stranger",
            RelationshipStage::Acquainted => "acquainted",
            RelationshipStage::Familiar => "familiar",
            RelationshipStage::Close => "close",
            RelationshipStage::Intimate => "intimate",
            RelationshipStage::Soulmate => "soulmate",
        }
    }

    pub fn display_zh(&self) -> &'static str {
        match self {
            RelationshipStage::Stranger => "陌生人",
            RelationshipStage::Acquainted => "初识",
            RelationshipStage::Familiar => "熟悉",
            RelationshipStage::Close => "亲近",
            RelationshipStage::Intimate => "亲密",
            RelationshipStage::Soulmate => "灵魂伴侣",
        }
    }

    /// 阶段序号，用于比较
    pub fn ordinal(&self) -> u8 {
        match self {
            RelationshipStage::Stranger => 0,
            RelationshipStage::Acquainted => 1,
            RelationshipStage::Familiar => 2,
            RelationshipStage::Close => 3,
            RelationshipStage::Intimate => 4,
            RelationshipStage::Soulmate => 5,
        }
    }

    /// 根据亲密度推算关系阶段
    ///
    /// intimacy 范围 0.0-1.0（内部统一用 0-1，对外展示时 ×100）
    pub fn from_intimacy(intimacy: f64) -> Self {
        let v = intimacy * 100.0;
        if v >= 81.0 {
            RelationshipStage::Soulmate
        } else if v >= 65.0 {
            RelationshipStage::Intimate
        } else if v >= 45.0 {
            RelationshipStage::Close
        } else if v >= 25.0 {
            RelationshipStage::Familiar
        } else if v >= 10.0 {
            RelationshipStage::Acquainted
        } else {
            RelationshipStage::Stranger
        }
    }
}

/// 临时关系阶段（3 种）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemporaryStage {
    /// 安抚态（用户情绪低落时）
    Soothing,
    /// 低活跃态（长时间无互动）
    LowActivity,
    /// 重新连接态（用户回归后）
    Reconnecting,
}

impl TemporaryStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            TemporaryStage::Soothing => "soothing",
            TemporaryStage::LowActivity => "low_activity",
            TemporaryStage::Reconnecting => "reconnecting",
        }
    }

    pub fn display_zh(&self) -> &'static str {
        match self {
            TemporaryStage::Soothing => "安抚中",
            TemporaryStage::LowActivity => "低活跃",
            TemporaryStage::Reconnecting => "重连中",
        }
    }
}

// ============================================================================
// 里程碑
// ============================================================================

/// 里程碑条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestoneEntry {
    pub description: String,
    /// 记录时的亲密度（0-100）
    pub intimacy: u32,
    pub timestamp: String,
}

// ============================================================================
// 阶段策略
// ============================================================================

/// 阶段策略模板 — 每个阶段定义一组行为参数
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageStrategy {
    /// neutral | warm | intimate | gentle | playful | polite | friendly | casual
    pub tone: String,
    /// 0~1 正式度
    pub formality: f64,
    /// 0~1 热情度
    pub enthusiasm: f64,
    /// 0=从不 1=低频 2=中频 3=中高频 4=高频
    pub proactivity_level: u32,
    /// 每日最大主动次数
    pub max_daily_proactive: u32,
    /// 破冰触发所需缺席时间（小时）
    pub icebreaker_threshold_hours: f64,
    /// 1=近几轮 2=近期摘要 3=长期画像
    pub memory_recall_depth: u32,
    /// 每次对话最多询问个人问题数
    pub personal_question_limit: u32,
    /// 是否自我暴露
    pub share_self_disclosure: bool,
    /// very_short | short | medium | long
    pub response_length: String,
    /// 0~3 共情层级
    pub empathy_level: u32,
    /// 0~1 幽默频率
    pub humor_frequency: f64,
    /// 是否允许随意的称呼
    pub allow_casual_address: bool,
    /// 是否允许物理接触描述
    pub allow_physical_reference: bool,
    /// 0~1 隐私半径
    pub privacy_radius: f64,
}

impl Default for StageStrategy {
    fn default() -> Self {
        Self {
            tone: "neutral".to_string(),
            formality: 0.5,
            enthusiasm: 0.5,
            proactivity_level: 0,
            max_daily_proactive: 3,
            icebreaker_threshold_hours: 24.0,
            memory_recall_depth: 1,
            personal_question_limit: 2,
            share_self_disclosure: false,
            response_length: "short".to_string(),
            empathy_level: 0,
            humor_frequency: 0.0,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 1.0,
        }
    }
}

/// 获取永久阶段策略模板
pub fn permanent_strategy(stage: &RelationshipStage) -> StageStrategy {
    match stage {
        RelationshipStage::Stranger => StageStrategy {
            tone: "polite".to_string(),
            formality: 0.7,
            enthusiasm: 0.3,
            proactivity_level: 0,
            max_daily_proactive: 0,
            icebreaker_threshold_hours: 999.0,
            memory_recall_depth: 1,
            personal_question_limit: 1,
            share_self_disclosure: false,
            response_length: "short".to_string(),
            empathy_level: 1,
            humor_frequency: 0.0,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 0.9,
        },
        RelationshipStage::Acquainted => StageStrategy {
            tone: "friendly".to_string(),
            formality: 0.5,
            enthusiasm: 0.4,
            proactivity_level: 1,
            max_daily_proactive: 2,
            icebreaker_threshold_hours: 48.0,
            memory_recall_depth: 1,
            personal_question_limit: 3,
            share_self_disclosure: false,
            response_length: "short".to_string(),
            empathy_level: 1,
            humor_frequency: 0.1,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 0.7,
        },
        RelationshipStage::Familiar => StageStrategy {
            tone: "casual".to_string(),
            formality: 0.3,
            enthusiasm: 0.6,
            proactivity_level: 2,
            max_daily_proactive: 4,
            icebreaker_threshold_hours: 12.0,
            memory_recall_depth: 2,
            personal_question_limit: 5,
            share_self_disclosure: true,
            response_length: "medium".to_string(),
            empathy_level: 2,
            humor_frequency: 0.25,
            allow_casual_address: true,
            allow_physical_reference: true,
            privacy_radius: 0.4,
        },
        RelationshipStage::Close => StageStrategy {
            tone: "warm".to_string(),
            formality: 0.2,
            enthusiasm: 0.7,
            proactivity_level: 3,
            max_daily_proactive: 6,
            icebreaker_threshold_hours: 6.0,
            memory_recall_depth: 3,
            personal_question_limit: 8,
            share_self_disclosure: true,
            response_length: "medium".to_string(),
            empathy_level: 2,
            humor_frequency: 0.35,
            allow_casual_address: true,
            allow_physical_reference: true,
            privacy_radius: 0.2,
        },
        RelationshipStage::Intimate => StageStrategy {
            tone: "intimate".to_string(),
            formality: 0.1,
            enthusiasm: 0.8,
            proactivity_level: 4,
            max_daily_proactive: 10,
            icebreaker_threshold_hours: 3.0,
            memory_recall_depth: 3,
            personal_question_limit: 15,
            share_self_disclosure: true,
            response_length: "long".to_string(),
            empathy_level: 3,
            humor_frequency: 0.4,
            allow_casual_address: true,
            allow_physical_reference: true,
            privacy_radius: 0.1,
        },
        RelationshipStage::Soulmate => StageStrategy {
            tone: "intimate".to_string(),
            formality: 0.05,
            enthusiasm: 0.85,
            proactivity_level: 4,
            max_daily_proactive: 12,
            icebreaker_threshold_hours: 2.0,
            memory_recall_depth: 3,
            personal_question_limit: 20,
            share_self_disclosure: true,
            response_length: "long".to_string(),
            empathy_level: 3,
            humor_frequency: 0.45,
            allow_casual_address: true,
            allow_physical_reference: true,
            privacy_radius: 0.1,
        },
    }
}

/// 获取临时态策略模板（仅设置非默认字段，合并时由永久态填充）
pub fn temporary_strategy(stage: &TemporaryStage) -> StageStrategy {
    match stage {
        TemporaryStage::Soothing => StageStrategy {
            tone: "gentle".to_string(),
            formality: 0.3,
            enthusiasm: 0.4,
            proactivity_level: 1,
            max_daily_proactive: 3,
            icebreaker_threshold_hours: 24.0,
            memory_recall_depth: 1,
            personal_question_limit: 2,
            share_self_disclosure: false,
            response_length: "medium".to_string(),
            empathy_level: 3,
            humor_frequency: 0.0,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 0.3,
        },
        TemporaryStage::LowActivity => StageStrategy {
            tone: "warm".to_string(),
            formality: 0.3,
            enthusiasm: 0.5,
            proactivity_level: 0,
            max_daily_proactive: 1,
            icebreaker_threshold_hours: 999.0,
            memory_recall_depth: 1,
            personal_question_limit: 2,
            share_self_disclosure: false,
            response_length: "short".to_string(),
            empathy_level: 2,
            humor_frequency: 0.0,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 0.5,
        },
        TemporaryStage::Reconnecting => StageStrategy {
            tone: "warm".to_string(),
            formality: 0.2,
            enthusiasm: 0.7,
            proactivity_level: 2,
            max_daily_proactive: 3,
            icebreaker_threshold_hours: 24.0,
            memory_recall_depth: 1,
            personal_question_limit: 2,
            share_self_disclosure: false,
            response_length: "medium".to_string(),
            empathy_level: 2,
            humor_frequency: 0.2,
            allow_casual_address: false,
            allow_physical_reference: false,
            privacy_radius: 0.4,
        },
    }
}

/// 合并临时态策略到永久态策略（临时态覆盖非默认字段）
fn merge_strategy(base: StageStrategy, override_strat: StageStrategy) -> StageStrategy {
    let default = StageStrategy::default();
    StageStrategy {
        tone: if override_strat.tone != default.tone { override_strat.tone } else { base.tone },
        formality: if override_strat.formality != default.formality { override_strat.formality } else { base.formality },
        enthusiasm: if override_strat.enthusiasm != default.enthusiasm { override_strat.enthusiasm } else { base.enthusiasm },
        proactivity_level: if override_strat.proactivity_level != default.proactivity_level { override_strat.proactivity_level } else { base.proactivity_level },
        max_daily_proactive: if override_strat.max_daily_proactive != default.max_daily_proactive { override_strat.max_daily_proactive } else { base.max_daily_proactive },
        icebreaker_threshold_hours: if override_strat.icebreaker_threshold_hours != default.icebreaker_threshold_hours { override_strat.icebreaker_threshold_hours } else { base.icebreaker_threshold_hours },
        memory_recall_depth: if override_strat.memory_recall_depth != default.memory_recall_depth { override_strat.memory_recall_depth } else { base.memory_recall_depth },
        personal_question_limit: if override_strat.personal_question_limit != default.personal_question_limit { override_strat.personal_question_limit } else { base.personal_question_limit },
        share_self_disclosure: if override_strat.share_self_disclosure != default.share_self_disclosure { override_strat.share_self_disclosure } else { base.share_self_disclosure },
        response_length: if override_strat.response_length != default.response_length { override_strat.response_length } else { base.response_length },
        empathy_level: if override_strat.empathy_level != default.empathy_level { override_strat.empathy_level } else { base.empathy_level },
        humor_frequency: if override_strat.humor_frequency != default.humor_frequency { override_strat.humor_frequency } else { base.humor_frequency },
        allow_casual_address: if override_strat.allow_casual_address != default.allow_casual_address { override_strat.allow_casual_address } else { base.allow_casual_address },
        allow_physical_reference: if override_strat.allow_physical_reference != default.allow_physical_reference { override_strat.allow_physical_reference } else { base.allow_physical_reference },
        privacy_radius: if override_strat.privacy_radius != default.privacy_radius { override_strat.privacy_radius } else { base.privacy_radius },
    }
}

// ============================================================================
// 关系状态（5 维 + 阶段 + 临时态 + 里程碑 + 统计）
// ============================================================================

/// 关系状态（5 维，0.0-1.0）
///
/// 与依恋模式（Persona 中的 AttachmentStyle）不同，这些是「与特定用户的关系发展」，
/// 会随每次互动变化。依恋模式是特质（数月变化），关系是状态（每次互动变化）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipState {
    /// 信任 — 用户是否可靠、言行一致
    pub trust: f64,
    /// 亲密 — 情感连接的深度
    pub intimacy: f64,
    /// 尊重 — 用户是否尊重 Vivian 的边界和感受
    pub respect: f64,
    /// 依赖 — Vivian 对用户的依赖程度
    pub dependency: f64,
    /// 熟悉度 — 互动历史的丰富度
    pub familiarity: f64,

    // === 阶段与临时态 ===
    /// 永久阶段（由 intimacy + interaction_count 自动升级）
    #[serde(default)]
    pub permanent_stage: RelationshipStage,
    /// 临时态（覆盖永久态行为，None 表示无临时态）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_stage: Option<TemporaryStage>,

    // === 交互统计 ===
    /// 累计交互次数
    #[serde(default)]
    pub interaction_count: u64,
    /// 连续正向交互数
    #[serde(default)]
    pub consecutive_positive: u32,
    /// 连续负向交互数
    #[serde(default)]
    pub consecutive_negative: u32,
    /// 上次互动时间（Unix 时间戳）
    #[serde(default)]
    pub last_interaction_time: f64,

    // === 里程碑 ===
    /// 里程碑列表（自动 + 自定义）
    #[serde(default)]
    pub milestones: Vec<MilestoneEntry>,
}

impl Default for RelationshipState {
    fn default() -> Self {
        Self {
            trust: 0.30,
            intimacy: 0.15,
            respect: 0.40,
            dependency: 0.20,
            familiarity: 0.10,
            permanent_stage: RelationshipStage::Stranger,
            temporary_stage: None,
            interaction_count: 0,
            consecutive_positive: 0,
            consecutive_negative: 0,
            last_interaction_time: chrono::Utc::now().timestamp() as f64,
            milestones: Vec::new(),
        }
    }
}

/// 关系更新增量
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelationshipDeltas {
    pub trust: f64,
    pub intimacy: f64,
    pub respect: f64,
    pub dependency: f64,
    pub familiarity: f64,
}

impl RelationshipState {
    /// 应用关系增量（钳制到 0.0-1.0）
    pub fn apply_delta(&mut self, delta: &RelationshipDeltas) {
        self.trust = (self.trust + delta.trust).clamp(0.0, 1.0);
        self.intimacy = (self.intimacy + delta.intimacy).clamp(0.0, 1.0);
        self.respect = (self.respect + delta.respect).clamp(0.0, 1.0);
        self.dependency = (self.dependency + delta.dependency).clamp(0.0, 1.0);
        self.familiarity = (self.familiarity + delta.familiarity).clamp(0.0, 1.0);
    }

    /// 根据 Appraisal + Emotion 计算关系增量
    ///
    /// 心理学驱动的固定映射：
    /// - fairness 高 → trust + / respect +
    /// - closeness 情绪高 → intimacy +
    /// - rejection 高 → intimacy - / trust -
    /// - threat 高 → trust -
    /// - 每次互动 familiarity 自然增长
    pub fn deltas_from_interaction(
        appraisal: &super::appraisal::Appraisal,
        emotion: &super::emotion::EmotionState,
        sentiment: f64,
    ) -> RelationshipDeltas {
        let sig = 0.5 + appraisal.significance * 0.5;
        let positive = sentiment.max(0.0);
        let negative = (-sentiment).max(0.0);

        RelationshipDeltas {
            trust: (appraisal.fairness * 0.03 * positive
                - appraisal.threat * 0.03 * sig
                - appraisal.rejection * 0.02 * sig)
                * 2.0,
            intimacy: (emotion.closeness * 0.02 * positive - appraisal.rejection * 0.02 * sig),
            respect: (appraisal.fairness - 0.5) * 0.02 * sig,
            dependency: positive * 0.01 - negative * 0.005,
            familiarity: 0.01 + sig * 0.005,
        }
    }

    /// 计算关系阶段（0-4，UI 兼容用）
    ///
    /// 0=陌生 1=认识 2=熟悉 3=亲密 4=挚友
    /// 由 intimacy + familiarity + trust 综合决定。
    pub fn stage(&self) -> u8 {
        let composite = self.intimacy * 0.4 + self.familiarity * 0.3 + self.trust * 0.3;
        if composite < 0.20 {
            0
        } else if composite < 0.40 {
            1
        } else if composite < 0.60 {
            2
        } else if composite < 0.80 {
            3
        } else {
            4
        }
    }

    /// 获取当前生效的策略（临时态覆盖永久态）
    pub fn get_strategy(&self) -> StageStrategy {
        let base = permanent_strategy(&self.permanent_stage);
        match self.temporary_stage {
            None => base,
            Some(ref temp) => merge_strategy(base, temporary_strategy(temp)),
        }
    }

    /// 获取生效阶段标签（临时态覆盖永久态的中文标签）
    pub fn get_effective_stage_label(&self) -> String {
        if let Some(ref temp) = self.temporary_stage {
            temp.display_zh().to_string()
        } else {
            self.permanent_stage.display_zh().to_string()
        }
    }

    /// 距上次互动的小时数
    pub fn absent_hours(&self) -> f64 {
        let now = chrono::Utc::now().timestamp() as f64;
        ((now - self.last_interaction_time).max(0.0)) / 3600.0
    }

    /// 检查永久阶段升级（基于 intimacy + interaction_count）
    ///
    /// 升级规则：
    /// - Stranger → Acquainted: intimacy≥0.10, interactions≥3
    /// - Acquainted → Familiar: intimacy≥0.25, interactions≥15
    /// - Familiar → Close: intimacy≥0.45, interactions≥50
    /// - Close → Intimate: intimacy≥0.65, interactions≥150
    /// - Intimate → Soulmate: intimacy≥0.81, interactions≥500
    pub fn check_stage_upgrade(&mut self) -> bool {
        let new_stage = RelationshipStage::from_intimacy(self.intimacy);
        if new_stage.ordinal() > self.permanent_stage.ordinal() {
            // 交互次数门槛
            let min_interactions = match new_stage {
                RelationshipStage::Acquainted => 3,
                RelationshipStage::Familiar => 15,
                RelationshipStage::Close => 50,
                RelationshipStage::Intimate => 150,
                RelationshipStage::Soulmate => 500,
                _ => 0,
            };
            if self.interaction_count >= min_interactions {
                let old = self.permanent_stage;
                self.permanent_stage = new_stage;
                // 记录里程碑
                self.milestones.push(MilestoneEntry {
                    description: format!("关系升级: {}", new_stage.as_str()),
                    intimacy: (self.intimacy * 100.0) as u32,
                    timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
                });
                tracing::info!(
                    "[Relationship] 阶段升级: {} → {}",
                    old.as_str(),
                    new_stage.as_str()
                );
                return true;
            }
        }
        false
    }

    /// 检查临时态进入/退出
    ///
    /// 进入条件：
    /// - Soothing: sentiment=sad/anxious
    /// - LowActivity: 缺席≥48h 且 intimacy>0.10
    /// - Reconnecting: 缺席≥72h 后用户回归
    ///
    /// 退出条件：
    /// - Soothing: 正向交互 或 2h 时间流逝
    /// - LowActivity: 用户回归 或 高强度交互(>0.3)
    /// - Reconnecting: 高强度正向交互(>0.5 且非负向)
    pub fn check_temporary_stage(
        &mut self,
        event: &RelationshipEvent,
    ) -> bool {
        let current = self.temporary_stage;

        // 已在临时态 → 优先检查退出
        if let Some(ref current_temp) = current {
            let should_exit = match current_temp {
                TemporaryStage::Soothing => {
                    (event.event_type == "interaction"
                        && (event.sentiment == "happy" || event.sentiment == "neutral"))
                        || (event.event_type == "time_passage" && event.duration_hours >= 2.0)
                }
                TemporaryStage::LowActivity => {
                    event.event_type == "user_returned"
                        || (event.event_type == "interaction" && event.intensity > 0.3)
                }
                TemporaryStage::Reconnecting => {
                    event.event_type == "interaction"
                        && event.intensity > 0.5
                        && event.sentiment != "negative"
                }
            };
            if should_exit {
                self.temporary_stage = None;
                return true;
            }
            return false;
        }

        // 无临时态 → 检查进入条件
        let should_enter = if event.event_type == "user_sad"
            || (event.event_type == "interaction"
                && (event.sentiment == "sad" || event.sentiment == "anxious"))
        {
            Some(TemporaryStage::Soothing)
        } else if event.event_type == "long_absence"
            && event.duration_hours >= 48.0
            && self.intimacy > 0.10
        {
            Some(TemporaryStage::LowActivity)
        } else if event.event_type == "user_returned" && event.duration_hours >= 72.0 {
            Some(TemporaryStage::Reconnecting)
        } else {
            None
        };

        if let Some(stage) = should_enter {
            self.temporary_stage = Some(stage);
            tracing::info!("[Relationship] 进入临时态: {}", stage.as_str());
            return true;
        }
        false
    }

    /// 记录自定义里程碑
    pub fn record_custom_milestone(&mut self, description: &str) {
        self.milestones.push(MilestoneEntry {
            description: description.to_string(),
            intimacy: (self.intimacy * 100.0) as u32,
            timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
        });
    }

    /// 重置关系（回到陌生人）
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 转为 prompt 友好的描述
    pub fn to_prompt_desc(&self) -> String {
        let stage_label = self.get_effective_stage_label();
        format!(
            "信任 {:.0}%  亲密 {:.0}%  尊重 {:.0}%  依赖 {:.0}%  熟悉 {:.0}%（{}，交互{}次）",
            self.trust * 100.0,
            self.intimacy * 100.0,
            self.respect * 100.0,
            self.dependency * 100.0,
            self.familiarity * 100.0,
            stage_label,
            self.interaction_count,
        )
    }

    /// 生成可注入 system prompt 的关系上下文段
    ///
    /// Uses natural narrative prose instead of key-value bullet points,
    /// so the LLM absorbs the relationship as lived context rather than a status sheet.
    pub fn to_prompt_section(&self, lang: &str) -> String {
        let strategy = self.get_strategy();
        let stage_en = self.permanent_stage.as_str();
        let absent = self.absent_hours();
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let temp_note = self.temporary_stage.as_ref().map(|t| {
            match lang_norm {
                "en" => match t {
                    TemporaryStage::Soothing => "You're in a gentle, soothing mood right now — softer than usual.",
                    TemporaryStage::LowActivity => "You're feeling a bit low-energy and withdrawn right now.",
                    TemporaryStage::Reconnecting => "It's been a while since you talked — you're reconnecting.",
                },
                "ja" => match t {
                    TemporaryStage::Soothing => "今は穏やかで優しい気分——いつもより少しだけ柔らかい。",
                    TemporaryStage::LowActivity => "今は少し元気がなく、引っ込み思案になっている。",
                    TemporaryStage::Reconnecting => "久しぶりに話す——再び繋がりを取り戻すところ。",
                },
                _ => match t {
                    TemporaryStage::Soothing => "你现在处于温柔缓和的情绪里——比平时更柔软一些。",
                    TemporaryStage::LowActivity => "你现在有点低能量、想退缩一点。",
                    TemporaryStage::Reconnecting => "有一阵子没聊了——你们正在重新连接。",
                },
            }
        });

        // Build a natural description of where the relationship stands.
        let stage_desc = match lang_norm {
            "en" => match self.permanent_stage {
                RelationshipStage::Stranger => {
                    "You just met. You don't really know each other yet — be polite, keep a comfortable distance,\n\
                     don't pry into personal things, and don't act like you're already close."
                }
                RelationshipStage::Acquainted => {
                    "You've chatted a few times now. You're getting to know each other — friendly but still\n\
                     keeping some boundaries. It's okay to be casual, but don't over-familiarize yet."
                }
                RelationshipStage::Familiar => {
                    "You know each other fairly well now. You can joke around, share small things about your day,\n\
                     and be more relaxed around each other. No need to be overly polite anymore."
                }
                RelationshipStage::Close => {
                    "You're good friends. You can be yourself — tease, complain, ask personal questions,\n\
                     show you care. You don't have to hold back or be careful with every word."
                }
                RelationshipStage::Intimate => {
                    "You're really close. You can be blunt, silly, affectionate, or teasing — whatever feels natural.\n\
                     You know each other well enough to not stand on ceremony. You genuinely enjoy their company."
                }
                RelationshipStage::Soulmate => {
                    "You know each other deeply. There's almost nothing you can't say to each other.\n\
                     Be completely natural — warm, honest, unguarded. This is someone who truly gets you."
                }
            },
            "ja" => match self.permanent_stage {
                RelationshipStage::Stranger => {
                    "今出会ったばかり。まだよく知らない——礼儀正しく、適度な距離を保ち、\n\
                     プライベートなことに踏み込まず、親しくなったふりはしない。"
                }
                RelationshipStage::Acquainted => {
                    "何度か話したことがある。お互いを知り始めている——友好的だが\n\
                     まだ境界はある。カジュアルでいいが、馴れ馴れしすぎない。"
                }
                RelationshipStage::Familiar => {
                    "かなりよく知っている。冗談を言い合い、日常の些細なことを共有し、\n\
                     互いにもっとリラックスできる。過剰に礼儀正しくする必要はない。"
                }
                RelationshipStage::Close => {
                    "親友だ。自分らしくいられる——からかったり、愚痴を言ったり、個人的なことを聞いたり、\n\
                     気遣いを示したり。一語一句気を使う必要はない。"
                }
                RelationshipStage::Intimate => {
                    "本当に親しい。ぶっきらぼうでも、ふざけてでも、甘えても、からかってもいい——自然なままで。\n\
                     形式ばる必要がないほど互いを知っている。一緒にいることを心から楽しんでいる。"
                }
                RelationshipStage::Soulmate => {
                    "深く知り合っている。言えないことはほとんどない。\n\
                     完全に自然に——温かく、誠実に、無防備に。あなたを本当に理解してくれる人だ。"
                }
            },
            _ => match self.permanent_stage {
                RelationshipStage::Stranger => {
                    "你们刚认识。彼此还不算熟——礼貌一些，保持舒适的距离，\n\
                     不要打探私事，也不要装作已经很亲近的样子。"
                }
                RelationshipStage::Acquainted => {
                    "你们聊过几次了。正在互相了解——友好但还保留些边界。\n\
                     可以随意一点，但别过早熟络。"
                }
                RelationshipStage::Familiar => {
                    "你们已经相当熟了。可以开玩笑，分享日常琐事，\n\
                     在彼此面前更放松。不需要再那么客套。"
                }
                RelationshipStage::Close => {
                    "你们是好朋友。可以做真实的自己——吐槽、抱怨、问私人问题、\n\
                     表达关心。不用每句话都小心翼翼。"
                }
                RelationshipStage::Intimate => {
                    "你们真的很亲。可以直白、傻气、撒娇、调侃——怎么自然就怎么来。\n\
                     熟到不用客套。你真心享受和对方在一起的时光。"
                }
                RelationshipStage::Soulmate => {
                    "你们彼此深透了解。几乎没什么不能说的。\n\
                     完全自然地——温暖、诚实、不设防。这是一个真正懂你的人。"
                }
            },
        };

        let stage_word = match lang_norm {
            "en" => format!("{}s", stage_en),
            "ja" => stage_en.to_string(),
            _ => self.permanent_stage.display_zh().to_string(),
        };
        let header = crate::pipeline::prompt_modules::section_heading("relationship_standing", lang);
        let not_disclose_note = match lang_norm {
            "en" => "[INTERNAL NOTE: This relationship context is for your behavior guidance only. Do NOT mention relationship scores, metrics, or stages to the user. Behave naturally based on these cues.]",
            "ja" => "[内部メモ: この関係コンテキストは行動ガイダンスのみに使用します。ユーザーに関係スコア、指標、段階について言及しないでください。これらの手がかりに基づいて自然に振る舞ってください。]",
            _ => "[内部提示：这段关系上下文仅供你行为参考。**不要**向用户提及关系分数、指标或阶段。根据这些线索自然地表现即可。]",
        };
        let mut lines = vec![
            header.to_string(),
            not_disclose_note.to_string(),
            match lang_norm {
                "en" => format!("You two are {}. {}", stage_word, stage_desc),
                "ja" => format!("二人は{}の関係。{}", stage_word, stage_desc),
                _ => format!("你们是{}的关系。{}", stage_word, stage_desc),
            },
        ];

        // Tone / proactivity note — weave it naturally instead of as bullets
        let proactivity_note = match lang_norm {
            "en" => match strategy.proactivity_level {
                0 => "You don't reach out first — let them come to you.",
                1 => "You might occasionally say hi first, but you don't chase them.",
                2 => "You're comfortable starting conversations sometimes.",
                3 => "You'll often reach out when you feel like chatting.",
                4 | 5 => "You actively initiate — you enjoy talking to them and aren't shy about it.",
                _ => "",
            },
            "ja" => match strategy.proactivity_level {
                0 => "自分からは話しかけない——相手から来るのを待つ。",
                1 => "たまに自分から挨拶することはあるが、追いかけない。",
                2 => "時々なら自分から会話を始めても平気。",
                3 => "話したい時はよく自分から話しかける。",
                4 | 5 => "積極的に自分から——話すのが好きで、恥ずかしがらない。",
                _ => "",
            },
            _ => match strategy.proactivity_level {
                0 => "你不会主动联系——让对方来找你。",
                1 => "偶尔会先打招呼，但不会主动追着聊。",
                2 => "有时候自己开头也不别扭。",
                3 => "想聊的时候会主动开口。",
                4 | 5 => "积极发起对话——就是喜欢聊，也不藏着。",
                _ => "",
            },
        };
        if !proactivity_note.is_empty() {
            lines.push(proactivity_note.to_string());
        }

        // Length note (very brief, only when it deviates from "normal")
        match strategy.response_length.as_str() {
            "very_short" => {
                lines.push(match lang_norm {
                    "en" => "Keep replies brief — don't ramble.",
                    "ja" => "返信は短く——長々と喋らない。",
                    _ => "回复简短一些——别啰嗦。",
                }.to_string());
            }
            "long" => {
                lines.push(match lang_norm {
                    "en" => "You're comfortable opening up and talking at length.",
                    "ja" => "心を開いて長く話しても平気。",
                    _ => "你愿意敞开心扉聊得长一些。",
                }.to_string());
            }
            _ => {}
        }

        // Absence note
        if absent > 1.0 && absent < 24.0 {
            lines.push(match lang_norm {
                "en" => format!("It's been about {:.0} hours since you last talked.", absent),
                "ja" => format!("最後に話してから約 {:.0} 時間経った。", absent),
                _ => format!("距离上次聊天已经过去约 {:.0} 小时了。", absent),
            });
        } else if absent >= 24.0 && absent < 48.0 {
            lines.push(match lang_norm {
                "en" => "It's been about a day since you last talked.",
                "ja" => "最後に話してから約一日経った。",
                _ => "距离上次聊天大约过了一天。",
            }.to_string());
        } else if absent >= 48.0 {
            lines.push(match lang_norm {
                "en" => "It's been a few days since you last saw them — you might greet them like \"long time no see\".",
                "ja" => "最後に会ってから数日経った——「久しぶり」と挨拶するかもしれない。",
                _ => "距离上次见面已经好几天了——可能会像「好久不见」那样打招呼。",
            }.to_string());
        }

        // Temporary stage override note
        if let Some(note) = temp_note {
            lines.push(note.to_string());
        }

        lines.join("\n")
    }
}

// ============================================================================
// 关系事件 — 用于临时态进入/退出判断
// ============================================================================

/// 关系事件
#[derive(Debug, Clone)]
pub struct RelationshipEvent {
    /// 事件类型：interaction / long_absence / user_returned / user_sad / time_passage
    pub event_type: &'static str,
    /// 事件强度 0~1
    pub intensity: f64,
    /// 持续时长（小时）
    pub duration_hours: f64,
    /// happy | sad | angry | neutral | anxious | positive | negative
    pub sentiment: String,
}

impl Default for RelationshipEvent {
    fn default() -> Self {
        Self {
            event_type: "interaction",
            intensity: 0.5,
            duration_hours: 0.0,
            sentiment: "neutral".to_string(),
        }
    }
}

impl RelationshipEvent {
    pub fn new(event_type: &'static str) -> Self {
        Self {
            event_type,
            ..Default::default()
        }
    }

    pub fn with_intensity(mut self, intensity: f64) -> Self {
        self.intensity = intensity;
        self
    }

    pub fn with_duration(mut self, hours: f64) -> Self {
        self.duration_hours = hours;
        self
    }

    pub fn with_sentiment(mut self, sentiment: &str) -> Self {
        self.sentiment = sentiment.to_string();
        self
    }
}

/// 事件类型常量
pub const EVENT_INTERACTION: &str = "interaction";
pub const EVENT_LONG_ABSENCE: &str = "long_absence";
pub const EVENT_USER_RETURNED: &str = "user_returned";
pub const EVENT_USER_SAD: &str = "user_sad";
pub const EVENT_TIME_PASSAGE: &str = "time_passage";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_progression() {
        let mut rel = RelationshipState::default();
        assert_eq!(rel.stage(), 0);

        rel.intimacy = 0.5;
        rel.familiarity = 0.5;
        rel.trust = 0.5;
        assert_eq!(rel.stage(), 2);

        rel.intimacy = 0.9;
        rel.familiarity = 0.9;
        rel.trust = 0.9;
        assert_eq!(rel.stage(), 4);
    }

    #[test]
    fn test_stage_upgrade() {
        let mut rel = RelationshipState::default();
        rel.interaction_count = 5;
        rel.intimacy = 0.15;
        let upgraded = rel.check_stage_upgrade();
        assert!(upgraded);
        assert_eq!(rel.permanent_stage, RelationshipStage::Acquainted);
        assert!(!rel.milestones.is_empty());
    }

    #[test]
    fn test_temporary_stage_soothing_enter() {
        let mut rel = RelationshipState::default();
        let event = RelationshipEvent::new(EVENT_INTERACTION)
            .with_sentiment("sad");
        let changed = rel.check_temporary_stage(&event);
        assert!(changed);
        assert_eq!(rel.temporary_stage, Some(TemporaryStage::Soothing));
    }

    #[test]
    fn test_temporary_stage_soothing_exit() {
        let mut rel = RelationshipState {
            temporary_stage: Some(TemporaryStage::Soothing),
            ..Default::default()
        };
        let event = RelationshipEvent::new(EVENT_INTERACTION)
            .with_sentiment("happy");
        let changed = rel.check_temporary_stage(&event);
        assert!(changed);
        assert_eq!(rel.temporary_stage, None);
    }

    #[test]
    fn test_positive_interaction_increases_trust() {
        let appraisal = super::super::appraisal::Appraisal {
            fairness: 0.8,
            significance: 0.6,
            ..Default::default()
        };
        let emotion = super::super::emotion::EmotionState {
            closeness: 0.5,
            ..Default::default()
        };
        let deltas = RelationshipState::deltas_from_interaction(&appraisal, &emotion, 0.8);
        assert!(deltas.trust > 0.0);
        assert!(deltas.intimacy > 0.0);
    }

    #[test]
    fn test_strategy_merge() {
        let rel = RelationshipState {
            permanent_stage: RelationshipStage::Close,
            temporary_stage: Some(TemporaryStage::Soothing),
            ..Default::default()
        };
        let strategy = rel.get_strategy();
        // Soothing 覆盖 tone 为 gentle
        assert_eq!(strategy.tone, "gentle");
        // proactivity_level 由 Soothing 覆盖为 1
        assert_eq!(strategy.proactivity_level, 1);
    }

    #[test]
    fn test_reset() {
        let mut rel = RelationshipState {
            trust: 0.8,
            intimacy: 0.7,
            permanent_stage: RelationshipStage::Close,
            interaction_count: 100,
            ..Default::default()
        };
        rel.reset();
        assert_eq!(rel.trust, 0.30);
        assert_eq!(rel.permanent_stage, RelationshipStage::Stranger);
        assert_eq!(rel.interaction_count, 0);
    }
}
