//! 会话生命周期管理（Conversation Lifecycle）
//!
//! 把**所有对话**（User↔Agent / Agent A↔Agent B）建模为有生命周期的会话对象，
//! 而不是无状态的逐轮转发。
//!
//! ## 核心思想
//!
//! 1. **Response Decision**：LLM 在一次调用里同时返回 `response_mode`
//!    （speak / non_verbal / internal / ignore），避免每条消息都触发完整 LLM 回复
//! 2. **状态机驱动结束**：会话有 `Created → Active → Cooling → Closed` 状态，
//!    结束由 `Continuation Score + Energy + Novelty` 综合判定，不靠轮数概率
//! 3. **Cooling 窗口**：进入 Cooling 后 30 秒内若有高分新消息可抢救回 Active，
//!    超时则自动 Close；Cooling 期间不主动调 LLM
//! 4. **Ownership**：发起方 Owner 若已结束会话，对方默认不重新拉起，
//!    除非 Continuation Score > 0.8
//! 5. **CloseReason**：关闭时记录原因（GoodNight/GoodBye/NoResponse/Timeout/...），
//!    由 `IntentJudge`（规则预检 + LLM 意图判断）共同决定，触发不同后续行为
//! 6. **通用性**：`pair_key` 已支持 "user" 固定标识，User↔Agent 与 Agent↔Agent
//!    共用同一套状态机
//!
//! ## 与现有系统的关系
//!
//! - **User↔Agent**：`commands::chat::send_message_stream` 在 `brain.think` 前调
//!   `start_or_continue("user", char_id)`，think 完成后调 `update_after_round`
//! - **Agent↔Agent**：`CrossCharacterBus::send` 在调用 `brain.think` 前通过
//!   `start_or_continue` 拿到会话，think 完成后调 `update_after_round`
//! - 会话进入 Cooling 时，工具返回值标记 `conv_state="cooling"`，A 的 LLM 自己决定下一步
//! - Cooling 超时由 `sweep_cooling` 清理（挂在 proactive_tick 上）
//! - `IntentJudge` 命中关闭意图（晚安/再见/打断/冲突/话题切换等）→ 立即 `close_with_reason`
//! - 用户超时无响应 → `sweep_user_session_timeouts` → close(Timeout)
//! - Episode 封包：会话 close 时触发 `seal_episode`，让经历边界对齐会话边界

pub mod evaluator;
pub mod integrity;
pub mod manager;
pub mod session;

pub use manager::{maybe_mark_open_loop, ConversationManager, CONVERSATION_MANAGER, TOPIC_PENDING};
pub use session::{CloseReason, Conversation, ConversationState, ResponseMode};

