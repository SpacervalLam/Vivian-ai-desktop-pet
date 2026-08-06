//! 主动行动规划与执行管线。
//!
//! 由 CognitiveTick Phase 5 (Act) 调用：
//! Planner 从 Goal/WorldState/BehaviorDrive 推导候选动作序列，
//! Executor 按 ActionActivationType 分流执行。

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::VivianResult;
use crate::mind::goal::Goal;
use crate::proactive::TickContext;
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::psychology::{BehaviorDrive, DriveLabel};
use crate::tools::executor::execute_tool_use;
use crate::tools::types::{ToolResult, ToolUseContext};
use crate::tools::ToolSystem;
use crate::utils::fnv1a_64_bytes;

/// 动作激活类型五分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionActivationType {
    /// 每次 tick 都执行
    Always,
    /// 按概率执行
    Random,
    /// 上下文含关键词时执行
    Keyword,
    /// 调用 LLM 判定是否执行（带 30s 缓存）
    LlmJudge,
    /// 不执行（占位/禁用）
    Never,
}

impl Default for ActionActivationType {
    fn default() -> Self {
        Self::Never
    }
}

impl ActionActivationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Random => "random",
            Self::Keyword => "keyword",
            Self::LlmJudge => "llm_judge",
            Self::Never => "never",
        }
    }
}

/// 单条规划动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedAction {
    /// 动作 ID（用于缓存 key 与日志）
    pub id: String,
    /// 工具名
    pub tool_name: String,
    /// 工具参数
    pub arguments: Value,
    /// 激活类型
    #[serde(default)]
    pub activation: ActionActivationType,
    /// Keyword 类型的触发关键词
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Random 类型的触发概率 [0.0, 1.0]
    #[serde(default)]
    pub probability: f64,
    /// 优先级（高先执行）
    #[serde(default)]
    pub priority: f64,
    /// 来源 Goal ID（可空）
    #[serde(default)]
    pub source_goal_id: Option<String>,
    /// 决策依据（日志用）
    #[serde(default)]
    pub rationale: String,
}

impl PlannedAction {
    fn new(id: impl Into<String>, tool_name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            tool_name: tool_name.into(),
            arguments,
            activation: ActionActivationType::Never,
            keywords: Vec::new(),
            probability: 0.0,
            priority: 0.5,
            source_goal_id: None,
            rationale: String::new(),
        }
    }
}

/// 一次规划产出的动作序列
#[derive(Debug, Clone, Default)]
pub struct ActionSequence {
    pub actions: Vec<PlannedAction>,
    /// 规划依据（日志用）
    pub rationale: String,
}

impl ActionSequence {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }
}

/// 主动行动规划器
///
/// 规则驱动：从 Goal/WorldState/BehaviorDrive 推导候选动作序列。
/// 不在规划阶段调用 LLM；LLM 调用交给 Executor 处理 LlmJudge 类型时按需触发。
pub struct ActionPlanner;

impl ActionPlanner {
    /// 规划上限：单次 tick 最多产出的动作数
    const MAX_ACTIONS_PER_TICK: usize = 4;

    /// 主导驱动阈值
    const DOMINANT_DRIVE_THRESHOLD: f64 = 0.35;

    /// 根据 Goal/WorldState/BehaviorDrive 推导候选动作序列
    pub fn plan(
        goals: &[Goal],
        drive: Option<&BehaviorDrive>,
        context: &TickContext,
    ) -> ActionSequence {
        let mut actions: Vec<PlannedAction> = Vec::new();
        let mut rationales: Vec<String> = Vec::new();

        // 来源 1：Goal 推导
        for goal in goals.iter().take(3) {
            if let Some(action) = Self::action_from_goal(goal, context) {
                actions.push(action);
                rationales.push(format!("goal:{}", goal.description));
            }
        }

        // 来源 2：BehaviorDrive 推导
        if let Some(drive) = drive {
            let (label, value) = drive.dominant();
            if value >= Self::DOMINANT_DRIVE_THRESHOLD {
                if let Some(action) = Self::action_from_drive(label, value, context) {
                    actions.push(action);
                    rationales.push(format!("drive:{}({:.2})", label.as_str(), value));
                }
            }
        }

        // 来源 3：长时间无活动 → 健康提醒类
        if context.idle_seconds > 1800.0 && context.user_present {
            let mut action = PlannedAction::new(
                "idle_health_reminder",
                "memory_recall",
                serde_json::json!({
                    "intent": "health_reminder",
                    "idle_seconds": context.idle_seconds
                }),
            );
            action.activation = ActionActivationType::Random;
            action.probability = 0.08;
            action.priority = 0.3;
            action.rationale = "idle>30min".to_string();
            actions.push(action);
            rationales.push("idle_health".to_string());
        }

        // 去重（按 tool_name + activation 去重，保留优先级高的）
        actions.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut deduped: Vec<PlannedAction> = Vec::new();
        for action in actions {
            let key = format!("{}|{}", action.tool_name, action.activation.as_str());
            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key, 0);
            deduped.push(action);
            if deduped.len() >= Self::MAX_ACTIONS_PER_TICK {
                break;
            }
        }

        ActionSequence {
            actions: deduped,
            rationale: rationales.join(","),
        }
    }

    /// 从 Goal 推导动作
    fn action_from_goal(goal: &Goal, context: &TickContext) -> Option<PlannedAction> {
        let desc = goal.description.to_lowercase();
        // 简单的关键词匹配，把目标描述映射到工具调用
        if desc.contains("提醒") || desc.contains("喝水") || desc.contains("休息") {
            let mut action = PlannedAction::new(
                format!("goal_{}", goal.id),
                "memory_recall",
                serde_json::json!({
                    "intent": "reminder",
                    "goal_description": goal.description
                }),
            );
            action.activation = ActionActivationType::Random;
            action.probability = 0.15;
            action.priority = goal.priority;
            action.source_goal_id = Some(goal.id.clone());
            action.rationale = format!("goal_reminder:{}", goal.description);
            return Some(action);
        }
        if desc.contains("陪伴") || desc.contains("逗") {
            let mut action = PlannedAction::new(
                format!("goal_{}", goal.id),
                "memory_recall",
                serde_json::json!({
                    "intent": "companionship",
                    "goal_description": goal.description
                }),
            );
            action.activation = ActionActivationType::LlmJudge;
            action.priority = goal.priority * 0.8;
            action.source_goal_id = Some(goal.id.clone());
            action.rationale = format!("goal_companion:{}", goal.description);
            let _ = context;
            return Some(action);
        }
        None
    }

    /// 从 BehaviorDrive 推导动作
    fn action_from_drive(
        label: DriveLabel,
        value: f64,
        context: &TickContext,
    ) -> Option<PlannedAction> {
        match label {
            DriveLabel::Explore => {
                let mut action = PlannedAction::new(
                    "drive_explore",
                    "memory_recall",
                    serde_json::json!({ "intent": "explore_topic" }),
                );
                action.activation = ActionActivationType::LlmJudge;
                action.priority = value * 0.6;
                action.rationale = "drive:explore".to_string();
                let _ = context;
                Some(action)
            }
            DriveLabel::Help => {
                let mut action = PlannedAction::new(
                    "drive_help",
                    "memory_recall",
                    serde_json::json!({ "intent": "help_suggestion" }),
                );
                action.activation = ActionActivationType::Keyword;
                action.keywords = vec!["累".to_string(), "忙".to_string(), "烦".to_string()];
                action.priority = value * 0.6;
                action.rationale = "drive:help".to_string();
                Some(action)
            }
            _ => None,
        }
    }
}

/// LLM 判定缓存条目
#[derive(Debug, Clone)]
struct LlmJudgeCacheEntry {
    decision: bool,
    cached_at: f64,
}

/// LLM 判定缓存：key = (char_id, context_hash, action_id)
static LLM_JUDGE_CACHE: Lazy<RwLock<HashMap<(String, u64, String), LlmJudgeCacheEntry>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// LLM 判定缓存 TTL（秒）
const LLM_JUDGE_CACHE_TTL_SECS: f64 = 30.0;

/// 缓存上限
const LLM_JUDGE_CACHE_CAP: usize = 200;

fn context_hash(context: &TickContext) -> u64 {
    // 量化上下文：把秒级时间戳降为 30s 粒度，让同一窗口内命中缓存
    let bucket = (context.now / 30.0) as u64;
    let mut buf = Vec::new();
    buf.extend_from_slice(bucket.to_string().as_bytes());
    buf.push(b'|');
    buf.extend_from_slice(context.user_emotion.as_bytes());
    buf.push(b'|');
    buf.extend_from_slice(format!("{:.0}", context.idle_seconds / 60.0).as_bytes());
    fnv1a_64_bytes(&buf)
}

fn llm_judge_cache_get(char_id: &str, ctx_hash: u64, action_id: &str, now: f64) -> Option<bool> {
    let key = (char_id.to_string(), ctx_hash, action_id.to_string());
    let cache = LLM_JUDGE_CACHE.read();
    let entry = cache.get(&key)?;
    if now - entry.cached_at > LLM_JUDGE_CACHE_TTL_SECS {
        return None;
    }
    Some(entry.decision)
}

fn llm_judge_cache_put(char_id: &str, ctx_hash: u64, action_id: &str, decision: bool, now: f64) {
    let mut cache = LLM_JUDGE_CACHE.write();
    if cache.len() >= LLM_JUDGE_CACHE_CAP {
        // 简单 LRU：超限时清空一半最旧条目
        let drop_count = cache.len() / 2;
        let mut entries: Vec<(f64, (String, u64, String))> = cache
            .iter()
            .map(|(k, v)| (v.cached_at, k.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, key) in entries.into_iter().take(drop_count) {
            cache.remove(&key);
        }
    }
    let key = (char_id.to_string(), ctx_hash, action_id.to_string());
    cache.insert(
        key,
        LlmJudgeCacheEntry {
            decision,
            cached_at: now,
        },
    );
}

/// 主动行动执行器
pub struct ActionExecutor {
    router: Option<Arc<ModelRouter>>,
    tool_system: Arc<ToolSystem>,
    char_id: String,
}

/// 单个动作的执行结果
#[derive(Debug, Clone)]
pub struct ActionExecutionResult {
    pub action_id: String,
    pub tool_name: String,
    pub executed: bool,
    pub skip_reason: Option<String>,
    pub tool_result: Option<ToolResult>,
}

impl ActionExecutor {
    pub fn new(
        router: Option<Arc<ModelRouter>>,
        tool_system: Arc<ToolSystem>,
        char_id: impl Into<String>,
    ) -> Self {
        Self {
            router,
            tool_system,
            char_id: char_id.into(),
        }
    }

    /// 执行动作序列：按 activation 分流，LlmJudge 类型并行判定
    pub async fn execute(
        &self,
        sequence: &ActionSequence,
        context: &TickContext,
        tool_ctx: &ToolUseContext,
    ) -> VivianResult<Vec<ActionExecutionResult>> {
        if sequence.is_empty() {
            return Ok(Vec::new());
        }

        let now = context.now;
        let ctx_hash = context_hash(context);

        // 分流：立即执行队列 / LLM 判定队列
        let mut immediate: Vec<&PlannedAction> = Vec::new();
        let mut judge_queue: Vec<&PlannedAction> = Vec::new();

        for action in &sequence.actions {
            match action.activation {
                ActionActivationType::Always => immediate.push(action),
                ActionActivationType::Random => {
                    let prob = action.probability.clamp(0.0, 1.0);
                    if prob <= 0.0 {
                        continue;
                    }
                    // 简单确定性随机：基于 (action_id, time_bucket) 哈希
                    let bucket = (now / 60.0) as u64;
                    let mut buf = action.id.as_bytes().to_vec();
                    buf.extend_from_slice(bucket.to_string().as_bytes());
                    let h = fnv1a_64_bytes(&buf);
                    let r = (h % 10000) as f64 / 10000.0;
                    if r < prob {
                        immediate.push(action);
                    }
                }
                ActionActivationType::Keyword => {
                    if Self::match_keywords(action, context) {
                        immediate.push(action);
                    }
                }
                ActionActivationType::LlmJudge => {
                    // 先查缓存
                    if let Some(decision) =
                        llm_judge_cache_get(&self.char_id, ctx_hash, &action.id, now)
                    {
                        if decision {
                            immediate.push(action);
                        }
                    } else {
                        judge_queue.push(action);
                    }
                }
                ActionActivationType::Never => {}
            }
        }

        // 并行 LLM 判定
        let judge_results = if judge_queue.is_empty() {
            Vec::new()
        } else {
            self.parallel_judge(&judge_queue, context, ctx_hash, now).await
        };

        for (i, should_run) in judge_results.into_iter().enumerate() {
            if should_run {
                immediate.push(judge_queue[i]);
            }
        }

        // 执行
        let mut results = Vec::with_capacity(immediate.len());
        for action in immediate {
            let result = self.execute_one(action, tool_ctx).await;
            results.push(result);
        }
        Ok(results)
    }

    /// 关键词匹配
    fn match_keywords(action: &PlannedAction, context: &TickContext) -> bool {
        if action.keywords.is_empty() {
            return false;
        }
        // 在用户情绪标签 + 活动窗口中匹配
        let haystack = format!("{} {}", context.user_emotion, context.active_window);
        for kw in &action.keywords {
            if !kw.is_empty() && haystack.contains(kw.as_str()) {
                return true;
            }
        }
        false
    }

    /// 并行 LLM 判定多个动作，返回按顺序的决策结果
    async fn parallel_judge(
        &self,
        actions: &[&PlannedAction],
        context: &TickContext,
        ctx_hash: u64,
        now: f64,
    ) -> Vec<bool> {
        let router = match &self.router {
            Some(r) => Arc::clone(r),
            None => {
                return actions
                    .iter()
                    .map(|a| Self::fallback_decision(&a.id, now))
                    .collect();
            }
        };

        let char_id = self.char_id.clone();
        let mut futures = Vec::with_capacity(actions.len());
        for action in actions {
            let router = Arc::clone(&router);
            let char_id = char_id.clone();
            let action_id = action.id.clone();
            let tool_name = action.tool_name.clone();
            let rationale = action.rationale.clone();
            let user_emotion = context.user_emotion.clone();
            let idle = context.idle_seconds;
            let present = context.user_present;
            futures.push(tokio::spawn(async move {
                let decision =
                    Self::llm_judge_single(&router, &char_id, &action_id, &tool_name, &rationale, &user_emotion, idle, present)
                        .await
                        .unwrap_or(false);
                (action_id, decision)
            }));
        }

        let mut out = Vec::with_capacity(actions.len());
        for f in futures {
            let decision = match f.await {
                Ok((action_id, decision)) => {
                    llm_judge_cache_put(&self.char_id, ctx_hash, &action_id, decision, now);
                    decision
                }
                Err(_) => false,
            };
            out.push(decision);
        }
        out
    }

    /// 单个动作的 LLM 判定
    async fn llm_judge_single(
        router: &ModelRouter,
        char_id: &str,
        action_id: &str,
        tool_name: &str,
        rationale: &str,
        user_emotion: &str,
        idle_seconds: f64,
        user_present: bool,
    ) -> VivianResult<bool> {
        let system = ChatMessageLike::system("你是一个动作执行决策助手。只回答 yes 或 no。");
        let user = ChatMessageLike::user(format!(
            "角色 {} 当前是否应该执行以下动作？\n动作ID: {}\n工具: {}\n依据: {}\n用户情绪: {}\n用户空闲: {:.0}s\n用户在场: {}\n请只回答 yes 或 no。",
            char_id, action_id, tool_name, rationale, user_emotion, idle_seconds, user_present
        ));
        let messages = vec![system, user];
        let req = LLMRequest::new("action_judge", messages);
        let resp = router.generate(req).await?;
        let text = resp.trim().to_lowercase();
        Ok(text.starts_with("yes") || text.contains("是") || text == "y")
    }

    /// 无 router 时的降级决策
    fn fallback_decision(action_id: &str, now: f64) -> bool {
        let bucket = (now / 60.0) as u64;
        let mut buf = action_id.as_bytes().to_vec();
        buf.extend_from_slice(bucket.to_string().as_bytes());
        let h = fnv1a_64_bytes(&buf);
        (h % 4) == 0
    }

    /// 执行单个动作
    async fn execute_one(
        &self,
        action: &PlannedAction,
        tool_ctx: &ToolUseContext,
    ) -> ActionExecutionResult {
        let result = execute_tool_use(
            &action.tool_name,
            action.arguments.clone(),
            &self.tool_system,
            tool_ctx,
            None,
        )
        .await;
        let executed = result.success;
        ActionExecutionResult {
            action_id: action.id.clone(),
            tool_name: action.tool_name.clone(),
            executed,
            skip_reason: if executed {
                None
            } else {
                Some("tool returned error".to_string())
            },
            tool_result: Some(result),
        }
    }
}

/// 简易 ChatMessage 构造辅助
struct ChatMessageLike;

impl ChatMessageLike {
    fn system(content: impl Into<String>) -> crate::types::response::ChatMessage {
        crate::types::response::ChatMessage::system(content)
    }
    fn user(content: impl Into<String>) -> crate::types::response::ChatMessage {
        crate::types::response::ChatMessage::user(content)
    }
}

/// 清空 LLM 判定缓存（测试用）
pub fn clear_llm_judge_cache() {
    LLM_JUDGE_CACHE.write().clear();
}

/// 当前 LLM 判定缓存条目数（诊断用）
pub fn llm_judge_cache_size() -> usize {
    LLM_JUDGE_CACHE.read().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context() -> TickContext {
        TickContext {
            now: 1000.0,
            idle_seconds: 60.0,
            away_seconds: 0.0,
            user_present: true,
            interaction_count_today: 5,
            active_window: "VSCode".to_string(),
            window_changed: false,
            last_topic_relevant: false,
            has_relevant_memory: false,
            drag_distance: 0.0,
            user_emotion: "neutral".to_string(),
            lay_low: false,
            is_user_chatting: false,
            is_speaking_leader: true,
        }
    }

    #[test]
    fn plan_empty_when_no_signals() {
        let ctx = make_context();
        let seq = ActionPlanner::plan(&[], None, &ctx);
        assert!(seq.is_empty());
    }

    #[test]
    fn plan_from_idle() {
        let mut ctx = make_context();
        ctx.idle_seconds = 2400.0; // 40 分钟
        let seq = ActionPlanner::plan(&[], None, &ctx);
        assert!(seq.len() >= 1);
        assert!(seq.actions.iter().any(|a| a.id == "idle_health_reminder"));
    }

    #[test]
    fn plan_dedupes_by_tool_and_activation() {
        use crate::mind::goal::GoalOrigin;
        let ctx = make_context();
        let goal1 = Goal::new("g1", "提醒喝水", GoalOrigin::UserRequest, 0.8, 0);
        let goal2 = Goal::new("g2", "提醒休息", GoalOrigin::UserRequest, 0.6, 0);
        let goals = vec![goal1, goal2];
        let seq = ActionPlanner::plan(&goals, None, &ctx);
        // 两个 goal 都映射到 memory_recall + random，去重后只保留一个
        let random_count = seq
            .actions
            .iter()
            .filter(|a| a.activation == ActionActivationType::Random)
            .count();
        assert!(random_count <= 1);
    }

    #[test]
    fn activation_type_as_str() {
        assert_eq!(ActionActivationType::Always.as_str(), "always");
        assert_eq!(ActionActivationType::Never.as_str(), "never");
        assert_eq!(ActionActivationType::LlmJudge.as_str(), "llm_judge");
    }

    #[test]
    fn llm_judge_cache_put_get() {
        clear_llm_judge_cache();
        llm_judge_cache_put("vivian", 123, "act1", true, 100.0);
        assert_eq!(llm_judge_cache_get("vivian", 123, "act1", 100.0), Some(true));
        // 过期
        assert_eq!(llm_judge_cache_get("vivian", 123, "act1", 131.0), None);
    }

    #[test]
    fn keyword_match_logic() {
        let action = PlannedAction {
            id: "a1".to_string(),
            tool_name: "memory_recall".to_string(),
            arguments: Value::Null,
            activation: ActionActivationType::Keyword,
            keywords: vec!["累".to_string()],
            probability: 0.0,
            priority: 0.5,
            source_goal_id: None,
            rationale: String::new(),
        };
        let mut ctx = make_context();
        ctx.user_emotion = "累".to_string();
        assert!(ActionExecutor::match_keywords(&action, &ctx));
        ctx.user_emotion = "happy".to_string();
        assert!(!ActionExecutor::match_keywords(&action, &ctx));
    }
}
