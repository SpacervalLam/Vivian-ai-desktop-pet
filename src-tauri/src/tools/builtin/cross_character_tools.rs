//! 跨角色对话工具 - talk_to_character
//!
//! 让当前角色（工具调用方）主动对另一个角色说话。
//! 工具通过 CrossCharacterBus 发起跨角色对话，等待目标角色 Brain.think 生成回复，
//! 并将回复文本作为工具返回值返回给源角色的 LLM，让源角色能"听到"目标角色的回应。
//!
//! 会话状态机（Conversation Session）：
//! - 目标角色可能选择不说话（response_mode=non_verbal/internal/ignore）
//! - 会话状态可能是 active/cooling/closed
//! - 工具根据这些状态返回不同的文本提示，让源角色 LLM 自然决定下一步

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::cross_character::{
    generate_cross_stream_id, CrossCharacterReply, CrossCharacterRequest, CROSS_CHARACTER_BUS,
};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
    ToolRiskTier,
};

// ===== talk_to_character =====

pub struct TalkToCharacterTool;

impl TalkToCharacterTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TalkToCharacterTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TalkToCharacterTool {
    fn name(&self) -> &str {
        "talk_to_character"
    }

    fn description(&self) -> &str {
        "Talk to another character on the desktop (cross-character conversation). Besides you \
         there may be several peers (for example vivian, nana, deepseek, claude); \
         target_character_id must be one of them, never yourself. \
         IMPORTANT: To actually have a conversation with a peer, you MUST call this tool. \
         Without calling this tool, you are just pretending to talk to her—she won't actually \
         see or respond to your message. Use this tool when you want to initiate or continue \
         a real conversation. target_character_id: the target peer's character ID; \
         message: what to say to her. Returns her actual reply text, or a state description \
         when she does not speak."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "和桌面上的另一个角色说话（跨角色对话）。除了你之外可能有多个同伴（例如 vivian、nana、deepseek、claude）；\
            target_character_id 必须是其中之一，不能是你自己。\
            重要：要真正和她对话，必须调用此工具。不调用此工具只是在假装和她说话——她实际上不会看到或回复你的消息。\
            当你想发起或继续真实对话时使用此工具。target_character_id：目标同伴的角色 ID；\
            message：要对她说的话。返回她的实际回复文本，若她未发言则返回状态描述。",
            "ja" => "デスクトップ上の別のキャラクターに話しかける（キャラクター間会話）。あなた以外に複数の仲間がいる場合がある（例：vivian、nana、deepseek、claude）。\
            target_character_id はそのうちの一人で、自分自身にはできない。\
            重要：実際に彼女と会話するには、必ずこのツールを呼び出すこと。このツールを呼ばずに話しかけても、彼女は実際にはメッセージを見ることも返信することもない。\
            本当の会話を始めたり続けたりしたい時にこのツールを使う。target_character_id：対象の仲間のキャラクター ID；\
            message：彼女に言いたいこと。彼女の実際の返信テキスト、または彼女が発言しない場合は状態説明を返す。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "去问问Vivian\n跟Nana说一下\n找DeepSeek聊聊\n问问Claude\n让她也说两句",
            "en" => "go ask Vivian\ntell Nana for me\ncheck with DeepSeek\nask Claude\nlet her weigh in",
            "ja" => "Vivianに聞いて\nNanaに伝えて\nDeepSeekと話して\nClaudeに聞いて\nあの子にも言って",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target_character_id": {
                    "type": "string",
                    "description": "Target peer character ID (e.g. nana, vivian, deepseek, claude)"
                },
                "message": {
                    "type": "string",
                    "description": "What to say to the target character"
                }
            },
            "required": ["target_character_id", "message"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "target_character_id": {
                        "type": "string",
                        "description": "目标同伴的角色 ID（例如 nana、vivian、deepseek、claude）"
                    },
                    "message": {
                        "type": "string",
                        "description": "要对目标同伴说的话"
                    }
                },
                "required": ["target_character_id", "message"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "target_character_id": {
                        "type": "string",
                        "description": "対象の仲間のキャラクター ID（例：nana、vivian、deepseek、claude）"
                    },
                    "message": {
                        "type": "string",
                        "description": "対象の仲間に言いたいこと"
                    }
                },
                "required": ["target_character_id", "message"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let target = input.get("target_character_id").and_then(|v| v.as_str());
        let message = input.get("message").and_then(|v| v.as_str());
        match (target, message) {
            (Some(t), Some(m)) if !t.is_empty() && !m.is_empty() => {
                ValidationResult::success(Some(input.clone()))
            }
            _ => ValidationResult::failure("target_character_id 和 message 都是必填项", 2),
        }
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let raw_target = args
            .get("target_character_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        // 模型可能传来显示名（Claude）而不是 ID（claude），统一解析到真实角色 ID。
        let target_id = crate::character_registry::list_character_ids()
            .into_iter()
            .find(|id| id.eq_ignore_ascii_case(&raw_target))
            .unwrap_or_else(|| crate::character_packages::id_for_name(&raw_target));
        if target_id == ctx.char_id {
            return ToolResult::standard_error(
                "不能和自己对话",
                Some("SelfTarget"),
                Some(json!({ "target_character_id": target_id })),
            );
        }
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let stream_id = generate_cross_stream_id();
        if crate::character_registry::get_brain(&ctx.char_id).is_some_and(|brain|!brain.mind.personal.permits_peer_text(&message)) {
            return ToolResult::standard_error("Message contains a private personal record; ask the user before sharing it",None,None);
        }
        let req = CrossCharacterRequest {
            source_id: ctx.char_id.clone(),
            target_id: target_id.clone(),
            message: message.clone(),
            source_message_id: None,
            stream_id,
        };

        let reply_result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            CROSS_CHARACTER_BUS.send_from_tool(req),
        )
        .await;

        match reply_result {
            Ok(Ok(reply)) => {
                let text_for_llm = format_reply_for_llm(&target_id, &reply);
                ToolResult::standard_success(
                    &text_for_llm,
                    Some(json!({
                        "target_character_id": target_id,
                        "message": message,
                        "reply": reply.reply,
                        "response_mode": reply.response_mode,
                        "conv_state": reply.conv_state,
                        "should_continue": reply.should_continue,
                    })),
                )
            }
            Ok(Err(e)) => ToolResult::standard_error(
                "跨角色对话失败",
                Some(&e.to_string()),
                Some(json!({
                    "target_character_id": target_id,
                    "message": message,
                    "error": e.to_string(),
                })),
            ),
            Err(_) => ToolResult::standard_error(
                "跨角色对话超时：目标角色在 60 秒内未响应",
                Some("CrossCharacterTimeout"),
                Some(json!({
                    "target_character_id": target_id,
                    "message": message,
                })),
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Pet
    }

    /// 权限风险等级：跨角色对话仅为角色间传话，无进程/系统副作用
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}

// ===== get_roommate_status =====

/// 查询室友当前在场的只读工具。
///
/// 存在的理由：prompt 里的室友在场状态改成**变化驱动**注入（稳态不注入，见
/// `CrossCharacterBus::observe_roommate_presence`），所以稳态下模型手上没有
/// "她此刻在不在"的信息。用户在平峰期问"娜娜在吗"时，模型需要一个按需通道，
/// 否则只剩两条路：凭空编，或者发一条真实的 `talk_to_character` 去打扰她。
pub struct GetRoommateStatusTool;

impl GetRoommateStatusTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetRoommateStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetRoommateStatusTool {
    fn name(&self) -> &str {
        "get_roommate_status"
    }

    fn description(&self) -> &str {
        "Check whether one peer or all peers are currently on the desktop and whether they are \
         available, busy or resting. Read-only and instant: no message is sent and nobody is \
         disturbed. Pass target_character_id to check one specific peer, or omit it to list \
         everyone. Call this when the user asks where someone is, whether they are around, or \
         what they are up to, so you answer from actual state instead of guessing. To actually \
         say something to a peer, use talk_to_character instead."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查看一个或全部同伴此刻是否在桌面上、是空闲还是在忙/休息。只读且即时：不会发消息，也不会打扰任何人。\
            传 target_character_id 查询指定同伴；省略则列出所有人。\
            当用户问某人在哪、在不在、在做什么时调用，用真实状态回答，而不是猜。\
            要真正对某人说点什么，请改用 talk_to_character。",
            "ja" => "一人または全員の仲間が今デスクトップにいるか、手が空いているか・忙しいか・休んでいるかを確認する。\
            読み取り専用で即時——メッセージは送られず、誰も邪魔しない。\
            target_character_id を渡すと特定の仲間を、省略すると全員を確認できる。\
            ユーザーが誰かの居場所や在席、何をしているかを尋ねたときに呼び出し、推測ではなく実際の状態で答えること。\
            実際に何か伝えたい場合は talk_to_character を使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "娜娜在吗\nClaude在干嘛\nDeepSeek在不在\n她们都在吗\n薇薇安现在忙吗",
            "en" => "is she around\nwhere is Claude\nis DeepSeek online\nare the others busy",
            "ja" => "彼女いる\nClaudeは今何してる\nDeepSeekはいる\nみんな忙しい",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target_character_id": {
                    "type": "string",
                    "description": "Optional peer character ID. Omit to list every peer's status."
                }
            },
            "required": []
        })
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let target = args
            .get("target_character_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let lang = crate::i18n::get_language();
        // 只读查询，不走变化检测——显式询问不应该被"状态没变"吞掉。
        match CROSS_CHARACTER_BUS.peer_status_text(&ctx.char_id, target.as_deref(), &lang) {
            Some(status) => {
                let peers: Vec<String> = crate::character_registry::list_character_ids()
                    .into_iter()
                    .filter(|id| id != &ctx.char_id)
                    .collect();
                ToolResult::standard_success(
                    &status,
                    Some(json!({
                        "target_character_id": target,
                        "status": status,
                        "peers": peers,
                        "shared_profiles": crate::character_registry::list_character_ids()
                            .into_iter()
                            .filter(|id| id != &ctx.char_id)
                            .filter_map(|id| crate::character_registry::get_brain(&id)
                                .map(|brain| json!({"character_id": id, "profile": brain.mind.personal.peer_context()})))
                            .collect::<Vec<_>>()
                    })),
                )
            }
            None => ToolResult::standard_error(
                "没有找到可查询的同伴",
                Some("NoPeer"),
                target.map(|id| json!({ "target_character_id": id })),
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Pet
    }

    /// 只读查询，无副作用
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}

/// 把结构化回复转换为 LLM 友好的文本
///
/// 让源角色 LLM 看到后能自然决定下一步：继续说、换话题、或停止。
/// 只客观呈现对方说了什么/做了什么，不注入对对方意图的主观解读，
/// 让源角色基于回复内容本身判断是否继续。
fn format_reply_for_llm(target_id: &str, reply: &CrossCharacterReply) -> String {
    if reply.conv_state == "interrupted" { return format!("{target_id} 已收到消息，但处理被中断，尚未完成。不要编造回答；需要明确重试。回执 ID：{}",reply.message_id.as_deref().unwrap_or("unknown")); }
    if reply.conv_state == "completed" { return format!("这条消息此前已处理，本次没有再次生成回答。回执 ID：{}。不要把接收状态当成新答复。",reply.message_id.as_deref().unwrap_or("unknown")); }
    if reply.conv_state == "queued" {
        return format!("消息已送达 {target_id} 的收件箱，尚未回复。回执 ID：{}。这只是接收确认，不是对方的答案；不要代替对方回答或立刻重复发送。后续回复会回到你的同伴消息信道。",reply.message_id.as_deref().unwrap_or("unknown"));
    }
    match reply.response_mode.as_str() {
        "speak" => {
            if reply.reply.is_empty() {
                format!("{} 没有说话。", target_id)
            } else {
                format!("{} 回复：{}", target_id, reply.reply)
            }
        }
        "non_verbal" => {
            format!(
                "{} 没有说话，用动作/表情回应了你。",
                target_id
            )
        }
        "internal" => {
            format!("{} 听到了，没有回应，似乎在思考。", target_id)
        }
        "ignore" => {
            // reply 字段携带了具体的忙碌原因（peer_busy/target_busy/user_input_pending），
            // 优先使用它让源角色 LLM 准确理解对方状态，而非笼统的"没有回应"
            if !reply.reply.is_empty() {
                reply.reply.clone()
            } else {
                format!("{} 没有回应。", target_id)
            }
        }
        _ => format!("{} 回复：{}", target_id, reply.reply),
    }
}
