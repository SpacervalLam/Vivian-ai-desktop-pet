//! 子代理上下文隔离
//!
//! 当 Brain 需要执行独立子任务（内心独白、主动消息生成、记忆巩固、
//! 查询重写等）时，这些子代理不应继承主对话的完整历史和状态，
//! 而应获得一个**隔离的、最小化的上下文**。
//!
//! 设计目标：
//! - 防止子代理任务污染主对话历史（不写入主 messages 数组）
//! - 限制上下文大小（只传入必要信息，降低 token 消耗）
//! - 敏感信息脱敏（如用户隐私数据不传入内心独白）
//! - 明确的 task boundary（每个子任务有明确的 task_type 和 scope）
//!
//! 使用方式：
//! ```ignore
//! let ctx = SubagentContext::new(SubagentTask::InnerMonologue)
//!     .with_world_snapshot(snap)
//!     .with_mind_state(mind_state)
//!     .with_memory_excerpt(recent_memories)  // 只传摘要，不传完整历史
//!     .with_intimacy(intimacy);
//! let messages = ctx.build_messages(system_prompt);
//! ```

use serde::{Deserialize, Serialize};

use crate::types::response::ChatMessage;

/// 子代理任务类型（决定上下文组装策略）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubagentTask {
    /// 内心独白：Vivian 自主思考
    InnerMonologue,
    /// 主动消息：主动发起对话
    ProactiveMessage,
    /// 记忆巩固：整理/压缩记忆
    MemoryConsolidation,
    /// 查询重写：改写用户输入用于检索
    QueryRewrite,
    /// 启动问候：生成问候语
    StartupGreeting,
    /// 场景分析：分析当前场景
    SceneAnalysis,
    /// 情感分类：分类用户情感
    EmotionClassification,
}

impl SubagentTask {
    /// 该任务类型的路由键（对应 ModelRouter 的 task_route 配置）
    pub fn route_key(&self) -> &'static str {
        match self {
            SubagentTask::InnerMonologue => "inner_monologue",
            SubagentTask::ProactiveMessage => "chat",
            SubagentTask::MemoryConsolidation => "consolidation",
            SubagentTask::QueryRewrite => "memory",
            SubagentTask::StartupGreeting => "chat",
            SubagentTask::SceneAnalysis => "vision_describe",
            SubagentTask::EmotionClassification => "emotion_analysis",
        }
    }

    /// 该任务的最大历史轮次（从主对话中截取的轮数上限）
    ///
    /// 0 表示不传入任何主对话历史。
    pub fn max_history_turns(&self) -> usize {
        match self {
            // 内心独白不需要对话历史（避免被具体对话内容带偏）
            SubagentTask::InnerMonologue => 0,
            // 主动消息需要最近 2 轮作参考（避免重复话题）
            SubagentTask::ProactiveMessage => 2,
            // 记忆巩固传入最近 5 轮的摘要
            SubagentTask::MemoryConsolidation => 5,
            // 查询重写只需要当前用户输入
            SubagentTask::QueryRewrite => 0,
            // 启动问候不需要历史
            SubagentTask::StartupGreeting => 0,
            // 场景分析需要最近 3 轮判断模式
            SubagentTask::SceneAnalysis => 3,
            // 情感分类只需要当前输入
            SubagentTask::EmotionClassification => 0,
        }
    }

    /// 是否允许传入用户个人信息（偏好/身份等）
    ///
    /// 隐私敏感的任务（如内心独白写入记忆）不传入用户真实姓名等。
    pub fn allow_user_profile(&self) -> bool {
        match self {
            SubagentTask::InnerMonologue => false,
            SubagentTask::MemoryConsolidation => true,
            _ => true,
        }
    }
}

/// 子代理隔离上下文
///
/// 携带子代理执行所需的最小化信息，不包含完整主对话历史。
#[derive(Debug, Clone, Default)]
pub struct SubagentContext {
    /// 任务类型
    pub task: Option<SubagentTask>,
    /// 世界快照摘要（天气/时间/节气等）
    pub world_brief: String,
    /// 心理状态简述（如 "Sleepy, intimacy=45"）
    pub mind_state: String,
    /// 情绪简述（如 "valence=0.3, arousal=0.4"）
    pub emotion_brief: String,
    /// 记忆摘要（已脱敏/压缩的最近记忆片段）
    pub memory_excerpt: String,
    /// 亲密度（0-100）
    pub intimacy: f64,
    /// 当前用户输入（仅 QueryRewrite / EmotionClassification 需要）
    pub user_input: String,
    /// 从主对话中截取的最近 N 轮（已脱敏）
    pub recent_dialogue: Vec<ChatMessage>,
    /// 触发器名称（仅 ProactiveMessage 需要）
    pub trigger_name: String,
}

impl SubagentContext {
    pub fn new(task: SubagentTask) -> Self {
        Self {
            task: Some(task),
            ..Default::default()
        }
    }

    pub fn with_world_brief(mut self, brief: impl Into<String>) -> Self {
        self.world_brief = brief.into();
        self
    }

    pub fn with_mind_state(mut self, state: impl Into<String>) -> Self {
        self.mind_state = state.into();
        self
    }

    pub fn with_emotion_brief(mut self, brief: impl Into<String>) -> Self {
        self.emotion_brief = brief.into();
        self
    }

    pub fn with_memory_excerpt(mut self, excerpt: impl Into<String>) -> Self {
        self.memory_excerpt = excerpt.into();
        self
    }

    pub fn with_intimacy(mut self, intimacy: f64) -> Self {
        self.intimacy = intimacy;
        self
    }

    pub fn with_user_input(mut self, input: impl Into<String>) -> Self {
        self.user_input = input.into();
        self
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger_name = trigger.into();
        self
    }

    /// 从主对话历史中截取最近 N 轮（脱敏后存入 recent_dialogue）
    ///
    /// `full_history` 是主对话的完整消息列表。
    /// 只截取最近 `max_turns` 轮（1 轮 = 1 user + 1 assistant）。
    /// 超过 200 字的单条消息会被截断。
    pub fn with_sandboxed_history(mut self, full_history: &[ChatMessage]) -> Self {
        let max_turns = self
            .task
            .map(|t| t.max_history_turns())
            .unwrap_or(0);
        if max_turns == 0 {
            return self;
        }

        let max_messages = max_turns * 2;
        let start = full_history.len().saturating_sub(max_messages);
        self.recent_dialogue = full_history[start..]
            .iter()
            .map(|m| sandbox_message(m))
            .collect();
        self
    }

    /// 组装 LLM 请求消息（system + user）
    ///
    /// system_prompt 由调用方提供（来自 PersonaEngine）。
    /// user 消息由本方法根据 task 类型组装。
    pub fn build_messages(&self, system_prompt: &str) -> Vec<ChatMessage> {
        let mut messages = Vec::with_capacity(2 + self.recent_dialogue.len());
        messages.push(ChatMessage::system(system_prompt));

        // 插入脱敏后的历史（如有）
        for m in &self.recent_dialogue {
            messages.push(m.clone());
        }

        let user_content = self.build_user_prompt();
        messages.push(ChatMessage::user(&user_content));
        messages
    }

    /// 根据任务类型组装 user prompt
    fn build_user_prompt(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if !self.world_brief.is_empty() {
            parts.push(format!("[World]\n{}", self.world_brief));
        }
        if !self.mind_state.is_empty() {
            parts.push(format!("[MindState]\n{}", self.mind_state));
        }
        if !self.emotion_brief.is_empty() {
            parts.push(format!("[Emotion]\n{}", self.emotion_brief));
        }
        if !self.memory_excerpt.is_empty() {
            parts.push(format!("[Memory]\n{}", self.memory_excerpt));
        }
        if self.intimacy > 0.0 {
            parts.push(format!("[Intimacy]\n{:.1}", self.intimacy));
        }
        if !self.trigger_name.is_empty() {
            parts.push(format!("[Trigger]\n{}", self.trigger_name));
        }
        if !self.user_input.is_empty() {
            parts.push(format!("[UserInput]\n{}", self.user_input));
        }

        // 任务指令
        if let Some(task) = self.task {
            parts.push(format!("[Task]\n{}", task_instruction(task)));
        }

        parts.join("\n\n")
    }
}

/// 对单条消息进行脱敏/截断
fn sandbox_message(msg: &ChatMessage) -> ChatMessage {
    let max_len = 200;
    let content = if msg.content.chars().count() > max_len {
        let truncated: String = msg.content.chars().take(max_len).collect();
        format!("{}...", truncated)
    } else {
        msg.content.clone()
    };
    ChatMessage {
        role: msg.role.clone(),
        content,
        timestamp: msg.timestamp,
        tool_calls: None,    // 子代理上下文不传递工具调用
        tool_call_id: None,
        images: None,
        reasoning: None,
        meta: None,
    }
}

/// 任务指令文本
fn task_instruction(task: SubagentTask) -> &'static str {
    match task {
        SubagentTask::InnerMonologue => {
            "Generate a brief inner monologue (1-2 sentences) reflecting on the current state. \
             Do not address the user directly. This is Vivian's private thought."
        }
        SubagentTask::ProactiveMessage => {
            "Generate a proactive message to send to the user. \
             Keep it short and natural. Do not repeat recent topics."
        }
        SubagentTask::MemoryConsolidation => {
            "Summarize and consolidate the recent conversation into a memory entry. \
             Extract key facts, preferences, and events."
        }
        SubagentTask::QueryRewrite => {
            "Rewrite the user input as a search query for memory retrieval. \
             Output only the rewritten query, nothing else."
        }
        SubagentTask::StartupGreeting => {
            "Generate a startup greeting. Keep it short and warm. \
             Do not use templates."
        }
        SubagentTask::SceneAnalysis => {
            "Analyze the current scene and suggest the best interaction mode. \
             Output JSON: {\"mode\": \"...\", \"confidence\": 0.0-1.0}"
        }
        SubagentTask::EmotionClassification => {
            "Classify the user's emotion from their input. \
             Output JSON: {\"emotion\": \"...\", \"intensity\": 0.0-1.0}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_routing() {
        assert_eq!(SubagentTask::InnerMonologue.route_key(), "inner_monologue");
        assert_eq!(SubagentTask::ProactiveMessage.route_key(), "chat");
    }

    #[test]
    fn test_history_sandboxing() {
        let ctx = SubagentContext::new(SubagentTask::ProactiveMessage);
        let history = vec![
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Hi there"),
            ChatMessage::user("How are you"),
            ChatMessage::assistant("I'm good"),
        ];
        let ctx = ctx.with_sandboxed_history(&history);
        // max_turns=2 for ProactiveMessage, so max 4 messages
        assert_eq!(ctx.recent_dialogue.len(), 4);
    }

    #[test]
    fn test_no_history_for_monologue() {
        let ctx = SubagentContext::new(SubagentTask::InnerMonologue);
        let history = vec![ChatMessage::user("Hello"), ChatMessage::assistant("Hi")];
        let ctx = ctx.with_sandboxed_history(&history);
        assert!(ctx.recent_dialogue.is_empty());
    }

    #[test]
    fn test_message_truncation() {
        let long_content = "a".repeat(300);
        let msg = ChatMessage::user(&long_content);
        let sandboxed = sandbox_message(&msg);
        assert!(sandboxed.content.chars().count() <= 203); // 200 + "..."
    }

    #[test]
    fn test_build_messages() {
        let ctx = SubagentContext::new(SubagentTask::InnerMonologue)
            .with_mind_state("Sleepy")
            .with_intimacy(45.0);
        let messages = ctx.build_messages("You are Vivian");
        assert_eq!(messages.len(), 2); // system + user
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("Sleepy"));
        assert!(messages[1].content.contains("45.0"));
    }
}
