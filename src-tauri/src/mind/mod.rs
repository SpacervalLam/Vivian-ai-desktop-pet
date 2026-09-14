//! Mind 模块 —— 角色认知运行时（Mind Runtime）。
//!
//! Mind 是角色的认知聚合句柄，把已有的 PsychologyManager（情绪/需求/人格/关系）
//! 与新增的 Belief / Goal / Attention 三个一等公民统一收口，供 prompt 序列化
//! 与记忆检索使用。
//!
//! 架构关系：
//! ```text
//! Event
//!   ↓
//! Attention（运行时聚焦权重）
//!   ↓ 预过滤
//! Memory Retrieval
//!   ↓ 召回证据
//! Mind { Belief / Goal / Emotion / Persona / Relationship }
//!   ↓ 序列化
//! Prompt（实现细节，业务层不再直接组装）
//!   ↓
//! LLM
//!   ↓
//! Reflection（异步，由 ConsolidationPipeline 触发）
//!   ↓ 写回
//! Belief / Goal
//! ```
//!
//! 核心原则：
//! - Belief 必须可溯源到 Memory（source_memory_ids 不可空），否则视为幻觉
//! - Attention 纯运行时，不持久化，启动时从最近事件重建
//! - Goal 数量稀少（同时活跃 ≤ 5），可由 Reflection / 用户请求 / 日程产生
//! - Mind 不替代 PsychologyManager，只聚合它 —— 已有心理状态不重写

pub mod attention;
pub mod belief;
pub mod belief_generator;
pub mod current_activity;
pub mod goal;
pub mod goal_service;
pub mod mind;
pub mod reasoning_trace;
pub mod temporal_context;
pub mod thought_synthesis;
pub mod user_cognition;
pub mod user_goals;
pub mod working_memory;

pub use attention::{Attention, AttentionFocus};
pub use belief::{
    classify_metric, circular_distance, ema_circular, Belief, BeliefCategory, BeliefStatus,
    BeliefStore, MetricKind,
};
pub use belief_generator::{BeliefGenerationReport, BeliefGenerator};
pub use user_cognition::{BeliefConflict, CognitionReport, UserCognitionEngine};
pub use current_activity::{
    ActivityEvent, ActivityKind, ActivityState, CurrentActivityTracker,
};
pub use goal::{Goal, GoalOrigin};
pub use goal_service::{GoalEvent, GoalEventKind, GoalService};
pub use mind::{Mind, ThoughtSnapshot};
pub use reasoning_trace::{
    PromptBreakdown, PromptSection, ReasoningStep, ReasoningTrace, SessionView, SharedTraceStore,
    TraceStore,
};
pub use user_goals::{
    parse_deadline, GoalUpdateOp, UserGoal, UserGoalBrief, UserGoalLedger, UserGoalSource,
    UserGoalState,
};
pub use temporal_context::{build_temporal_facts, serialize_temporal_facts, TemporalFact, TemporalFactKind};
pub use working_memory::{WorkingMemory, WorkingMemoryEntry, WorkingMemorySource};

/// 已知角色 ID 列表（用于从文本中识别角色实体并 boost 注意力）
pub const KNOWN_CHAR_IDS: &[&str] = &["vivian", "nana", "薇薇安", "娜娜"];

/// 从用户输入提取注意力焦点实体并 boost 到 Mind。
///
/// 规则（刻意保持简单，不做复杂 NLP）：
/// - "user" 永远 boost 到 1.0（用户在跟角色说话，用户本身是最高注意力）
/// - 当前角色自己 boost 到 0.8（被对话的对象）
/// - 文本中提到的另一角色名 boost 到 0.7（提及即关注）
/// - jieba 分词后的实词（长度 ≥ 2 的中文词 / 长度 ≥ 3 的英文词）boost 到 0.4
///   仅取 Top-5，避免注意力被无关词稀释
///
/// 不调 LLM，纯规则，几十微秒。
pub fn boost_attention_from_input(mind: &Mind, user_input: &str, now: i64) {
    // 用户永远是最强焦点
    mind.boost_attention("user", 1.0, now);
    // 当前角色自己
    mind.boost_attention(&mind.char_id, 0.8, now);

    // 识别文本中提到的角色
    let input_lower = user_input.to_lowercase();
    for &char_id in KNOWN_CHAR_IDS {
        if char_id == mind.char_id {
            continue;
        }
        if input_lower.contains(char_id) {
            mind.boost_attention(char_id, 0.7, now);
        }
    }

    // jieba 分词提取实词，Top-5 boost 到 0.4
    let tokens = crate::memory::tokenize::tokenize(user_input);
    let mut picked = 0usize;
    for token in tokens {
        if picked >= 5 {
            break;
        }
        // 跳过已知角色（已 boost 过）和无意义短词
        let is_known_char = KNOWN_CHAR_IDS.iter().any(|c| *c == token);
        if is_known_char {
            continue;
        }
        let is_stopword = matches!(token.as_str(), "的" | "了" | "是" | "我" | "你" | "他" | "她" | "这" | "那" | "就" | "都" | "也" | "和" | "与" | "在" | "有" | "不" | "没");
        if is_stopword {
            continue;
        }
        let worth = token.chars().filter(|c| c.is_alphanumeric()).count() >= 2;
        if !worth {
            continue;
        }
        mind.boost_attention(&token, 0.4, now);
        picked += 1;
    }
}
