//! 会话对象与状态机定义

use serde::{Deserialize, Serialize};

/// 响应模式（由 LLM 在一次调用中返回，决定是否生成回复文本）
///
/// 跨角色场景下才允许返回非 speak 模式；主对话路径默认 speak。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    /// 正常回复（生成文本）
    Speak,
    /// 只做动作/表情，不生成文本（如点头、微笑）
    NonVerbal,
    /// 只更新内部想法/记忆，不说话也不做动作
    Internal,
    /// 完全忽略（如对方在休息/忙/内容无关）
    Ignore,
}

impl Default for ResponseMode {
    fn default() -> Self {
        ResponseMode::Speak
    }
}

impl ResponseMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResponseMode::Speak => "speak",
            ResponseMode::NonVerbal => "non_verbal",
            ResponseMode::Internal => "internal",
            ResponseMode::Ignore => "ignore",
        }
    }

    /// 从字符串解析，未知值回退为 Speak（保证主对话路径安全）
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "non_verbal" | "nonverbal" => ResponseMode::NonVerbal,
            "internal" => ResponseMode::Internal,
            "ignore" => ResponseMode::Ignore,
            _ => ResponseMode::Speak,
        }
    }

    /// 是否需要生成回复文本
    pub fn needs_speech(&self) -> bool {
        matches!(self, ResponseMode::Speak)
    }

    /// 是否完全不应答（连动作/表情都不做）
    pub fn is_silent(&self) -> bool {
        matches!(self, ResponseMode::Ignore)
    }
}

/// 会话状态机
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationState {
    /// 刚创建（未开始首轮）
    Created,
    /// 活跃进行中
    Active,
    /// 冷却中：30 秒窗口，期间高分新消息可抢救回 Active，超时则 Close
    Cooling,
    /// 已关闭（不可再继续）
    Closed,
}

/// 会话关闭原因
///
/// 不同原因触发不同后续行为：
/// - `GoodNight` → 睡眠时间内不再主动搭话
/// - `NoResponse` → 知道用户忙，后续可重新开启
/// - `Interrupted` → 用户回来后可恢复旧会话（"刚刚说到哪了？"）
/// - `Timeout` → 长时间无互动，自然淡出
///
/// 由 `IntentJudge`（规则预检 + LLM 意图判断）共同决定。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// 自然结束（话题耗尽，energy/novelty 低）
    Natural,
    /// 晚安
    GoodNight,
    /// 再见/走了/拜拜
    GoodBye,
    /// 主动聊天被用户忽略
    NoResponse,
    /// 中途打断（"老板电话来了"）
    Interrupted,
    /// 超时无响应
    Timeout,
    /// 冲突（争吵后中断）
    Conflict,
    /// 话题切换（显式开启新话题）
    SwitchTopic,
}

impl Default for CloseReason {
    fn default() -> Self {
        CloseReason::Natural
    }
}

impl CloseReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            CloseReason::Natural => "natural",
            CloseReason::GoodNight => "good_night",
            CloseReason::GoodBye => "good_bye",
            CloseReason::NoResponse => "no_response",
            CloseReason::Interrupted => "interrupted",
            CloseReason::Timeout => "timeout",
            CloseReason::Conflict => "conflict",
            CloseReason::SwitchTopic => "switch_topic",
        }
    }

    /// 是否为"用户主动告别"（GoodNight/GoodBye）
    pub fn is_user_farewell(&self) -> bool {
        matches!(self, CloseReason::GoodNight | CloseReason::GoodBye)
    }

    /// 是否为"被动中断"（NoResponse/Timeout）
    pub fn is_passive(&self) -> bool {
        matches!(
            self,
            CloseReason::NoResponse | CloseReason::Timeout
        )
    }
}

impl Default for ConversationState {
    fn default() -> Self {
        ConversationState::Created
    }
}

impl ConversationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConversationState::Created => "created",
            ConversationState::Active => "active",
            ConversationState::Cooling => "cooling",
            ConversationState::Closed => "closed",
        }
    }

    /// 是否仍可继续（未关闭）
    pub fn is_alive(&self) -> bool {
        !matches!(self, ConversationState::Closed)
    }

    /// 是否可接受新消息（Active 或 Cooling 均可，Cooling 视抢救规则决定）
    pub fn can_accept_message(&self) -> bool {
        matches!(self, ConversationState::Active | ConversationState::Cooling)
    }
}

/// 跨角色会话对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// 会话 ID
    pub id: String,
    /// 当前话题（从首条消息提取，最长 20 字）
    pub topic: String,
    /// 发起者 ID（Owner）
    pub owner: String,
    /// 参与者 ID 列表（owner + 对方）
    pub participants: Vec<String>,
    /// 当前状态
    pub state: ConversationState,
    /// 活跃度 [0, 1]：低于 ENERGY_THRESHOLD 倾向结束
    pub energy: f64,
    /// 新信息密度 [0, 1]：低于 NOVELTY_THRESHOLD 倾向结束
    pub novelty: f64,
    /// 连续轮数
    pub rounds: u32,
    /// 继续得分 [0, 1]：< CONTINUATION_THRESHOLD 进入 Cooling
    pub continuation_score: f64,
    /// 创建时间戳（Unix 秒）
    pub created_at: f64,
    /// 最近活跃时间戳
    pub last_active_at: f64,
    /// 进入 Cooling 的时间戳
    pub cooling_since: Option<f64>,
    /// 关闭时间戳（用于创建冷却判定）
    ///
    /// 会话关闭后 `CLOSED_COOLDOWN_SECS` 秒内不允许同一对角色创建新会话，
    /// 防止 A 的 LLM 在 B 总 ignore 时反复创建新会话导致无限调用。
    pub closed_at: Option<f64>,
    /// 关闭原因
    ///
    /// 由 `IntentJudge`（规则预检 + LLM 意图判断）决定。
    /// 不同原因触发不同后续行为（如 GoodNight → 睡眠时间内不主动搭话）。
    pub close_reason: Option<CloseReason>,
    /// 用户最近一次发言时间戳（用于 NoResponse/Timeout 判定）
    ///
    /// 仅 User↔Agent 会话使用；Agent↔Agent 会话此字段始终等于 last_active_at。
    pub last_user_message_at: Option<f64>,
    /// 最近一轮的响应模式
    pub last_response_mode: ResponseMode,
    /// 会话期间产生的记忆 ID 列表（用于 close 时触发 seal_episode）
    ///
    /// 由 memory_saving step 在写入记忆后调用 `add_memory_to_session` 追加。
    /// close 时调用方据此触发 `EpisodeStore::seal_episode`，让经历边界对齐会话边界。
    pub memory_ids: Vec<String>,
    /// 最近 5 轮的 continuation_score（用于会话关闭时判断是否有未完成话题）
    ///
    /// Open Loop 检测：当会话因冷却超时关闭时，如果最近几轮平均分数较高（> 0.5），
    /// 说明话题还有生命力，在最近一条记忆上附加 follow_up hook，下次自然续接。
    pub recent_continuation_scores: Vec<f64>,
    /// 追加式会话事件日志（事件溯源底层）：所有会话事件按序追加，可投影出 LLM 消息历史
    #[serde(default)]
    pub event_log: Vec<SessionEvent>,
}

/// 会话事件（追加式日志条目）。
///
/// 事件溯源的核心：会话的一切状态变化都以追加事件记录，`derive_messages` 据此
/// 投影出模型消息历史；崩溃/恢复时可重放事件重建会话状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 单调递增序号（同一会话内唯一，投影排序依据）
    pub seq: u64,
    /// 事件时间戳（Unix 秒）
    pub timestamp: f64,
    /// 事件类别
    pub kind: SessionEventKind,
    /// 事件正文（消息内容 / 状态描述等）
    pub content: String,
}

/// 会话事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEventKind {
    /// 用户消息
    UserMessage,
    /// 助手消息
    AssistantMessage,
    /// 一轮开始
    TurnStart,
    /// 一轮结束
    TurnEnd,
    /// 会话状态迁移（Created→Active 等）
    StateTransition,
}

impl Conversation {
    /// 追加一条会话事件到事件日志（返回新事件的 seq）。
    pub fn record_event(&mut self, kind: SessionEventKind, content: impl Into<String>) -> u64 {
        let seq = self.event_log.len() as u64;
        self.event_log.push(SessionEvent {
            seq,
            timestamp: chrono::Local::now().timestamp() as f64,
            kind,
            content: content.into(),
        });
        seq
    }

    /// 从事件日志投影出 LLM 消息历史（按 seq 排序，抽取 User/Assistant 消息）。
    ///
    /// 事件溯源语义：状态由日志推导，而非单独维护的消息数组——保证重放一致性。
    pub fn derive_messages(&self) -> Vec<(SessionEventKind, String)> {
        let mut out = Vec::new();
        for ev in &self.event_log {
            match ev.kind {
                SessionEventKind::UserMessage | SessionEventKind::AssistantMessage => {
                    out.push((ev.kind, ev.content.clone()));
                }
                _ => {}
            }
        }
        out
    }

    /// 从事件日志投影出精简的轮次记录（turn/start 计数）。
    pub fn derived_turns(&self) -> usize {
        self.event_log
            .iter()
            .filter(|ev| ev.kind == SessionEventKind::TurnStart)
            .count()
    }
}

impl Conversation {
    /// Cooling 窗口（秒）：超过后自动 Close
    pub const COOLING_WINDOW_SECS: f64 = 30.0;

    /// 会话关闭后创建冷却（秒）：此期间内不允许同一对角色创建新会话
    pub const CLOSED_COOLDOWN_SECS: f64 = 60.0;

    /// Energy 阈值：低于此值倾向结束
    pub const ENERGY_THRESHOLD: f64 = 0.25;

    /// Novelty 阈值：低于此值倾向结束
    pub const NOVELTY_THRESHOLD: f64 = 0.15;

    /// Continuation Score 阈值：< 此值进入 Cooling
    pub const CONTINUATION_THRESHOLD: f64 = 0.30;

    /// 抢救回 Active 所需的 Continuation Score（Cooling 期间收到新消息时检查）
    pub const RESCUE_THRESHOLD: f64 = 0.80;

    /// 轮次提醒阈值：达到此轮次后在提示词中提醒 LLM 准备结束话题
    pub const WARN_ROUNDS: u32 = 10;

    /// 轮次硬上限：达到此轮次后强制进入 Cooling，要求 LLM 给出结束语
    pub const MAX_ROUNDS: u32 = 20;

    pub fn new(id: String, owner: &str, participant: &str, topic: &str) -> Self {
        let now = chrono::Local::now().timestamp() as f64;
        Self {
            id,
            topic: topic.to_string(),
            owner: owner.to_string(),
            participants: vec![owner.to_string(), participant.to_string()],
            state: ConversationState::Created,
            energy: 0.7,
            novelty: 0.8,
            rounds: 0,
            continuation_score: 0.7,
            created_at: now,
            last_active_at: now,
            cooling_since: None,
            closed_at: None,
            close_reason: None,
            last_user_message_at: None,
            last_response_mode: ResponseMode::Speak,
            memory_ids: Vec::new(),
            recent_continuation_scores: Vec::new(),
            event_log: Vec::new(),
        }
    }

    /// 另一参与者 ID
    pub fn other_participant(&self, self_id: &str) -> Option<&str> {
        self.participants
            .iter()
            .find(|p| p.as_str() != self_id)
            .map(|s| s.as_str())
    }

    /// 是否仍可继续（未关闭）
    pub fn is_alive(&self) -> bool {
        self.state.is_alive()
    }

    /// 记录一轮 continuation_score（保留最近 5 轮）
    pub fn record_continuation_score(&mut self, score: f64) {
        self.recent_continuation_scores.push(score);
        if self.recent_continuation_scores.len() > 5 {
            self.recent_continuation_scores.remove(0);
        }
    }

    /// 最近 N 轮 continuation_score 的平均值（用于 Open Loop 检测）
    ///
    /// 返回 None 表示没有记录（不应触发 Open Loop）。
    pub fn recent_score_avg(&self, n: usize) -> Option<f64> {
        if self.recent_continuation_scores.is_empty() {
            return None;
        }
        let tail = &self.recent_continuation_scores
            [self.recent_continuation_scores.len().saturating_sub(n)..];
        Some(tail.iter().sum::<f64>() / tail.len() as f64)
    }
}
