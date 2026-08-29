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
        "Talk to your roommate (cross-character conversation). You live with another girl—when \
         you are vivian, your roommate is nana; when you are nana, your roommate is vivian. \
         IMPORTANT: To actually have a conversation with her, you MUST call this tool. \
         Without calling this tool, you are just pretending to talk to her—she won't actually \
         see or respond to your message. Use this tool when you want to initiate or continue \
         a real conversation. target_character_id: the target character ID (nana or vivian); \
         message: what to say to her. Returns her actual reply text, or a state description \
         when she does not speak."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "和室友说话（跨角色对话）。你和另一个女孩住在一起——你是 vivian 时，室友是 nana；你是 nana 时，室友是 vivian。\
            重要：要真正和她对话，必须调用此工具。不调用此工具只是在假装和她说话——她实际上不会看到或回复你的消息。\
            当你想发起或继续真实对话时使用此工具。target_character_id：目标角色 ID（nana 或 vivian）；\
            message：要对她说的话。返回她的实际回复文本，若她未发言则返回状态描述。",
            "ja" => "ルームメイトに話しかける（キャラクター間会話）。あなたはもう一人の女の子と一緒に住んでいる——\
            あなたが vivian の時、ルームメイトは nana；あなたが nana の時、ルームメイトは vivian。\
            重要：実際に彼女と会話するには、必ずこのツールを呼び出すこと。このツールを呼ばずに話しかけても、彼女は実際にはメッセージを見ることも返信することもない。\
            本当の会話を始めたり続けたりしたい時にこのツールを使う。target_character_id：対象キャラクター ID（nana または vivian）；\
            message：彼女に言いたいこと。彼女の実際の返信テキスト、または彼女が発言しない場合は状態説明を返す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target_character_id": {
                    "type": "string",
                    "description": "Target character ID (e.g. nana, vivian)"
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
                        "description": "目标角色 ID（例如 nana、vivian）"
                    },
                    "message": {
                        "type": "string",
                        "description": "要对目标角色说的话"
                    }
                },
                "required": ["target_character_id", "message"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "target_character_id": {
                        "type": "string",
                        "description": "対象キャラクター ID（例：nana、vivian）"
                    },
                    "message": {
                        "type": "string",
                        "description": "対象キャラクターに言いたいこと"
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
        let target_id = args
            .get("target_character_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let stream_id = generate_cross_stream_id();
        let req = CrossCharacterRequest {
            source_id: ctx.char_id.clone(),
            target_id: target_id.clone(),
            message: message.clone(),
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
}

/// 把结构化回复转换为 LLM 友好的文本
///
/// 让源角色 LLM 看到后能自然决定下一步：继续说、换话题、或停止。
/// 只客观呈现对方说了什么/做了什么，不注入对对方意图的主观解读，
/// 让源角色基于回复内容本身判断是否继续。
fn format_reply_for_llm(target_id: &str, reply: &CrossCharacterReply) -> String {
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
