//! 回忆式提问
//!
//! 从对话历史 / 最近记忆文本提取可自然提及的旧话题，生成"记得吗？"类问题。
//! 仅使用 LLM 生成，LLM 失败则不交互。

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 回忆式提问生成器
pub struct MemoryRecall;

impl MemoryRecall {
    // ============ LLM 路径 ============

    /// 调用 LLM 生成自然回忆提问
    ///
    /// `recent_memory`：缓存的最近对话/记忆文本，作为"最近对话"与话题来源。
    /// `system_prompt`：来自 PersonaEngine 的人设风格约束，为空时使用兜底。
    /// LLM 失败则返回 None（不交互）。
    pub async fn generate_llm(
        router: &ModelRouter,
        recent_memory: &str,
        system_prompt: &str,
        lang: &str,
        char_id: &str,
    ) -> Option<String> {
        let messages = Self::build_messages(recent_memory, system_prompt, lang, char_id)?;
        let raw = match router.generate(LLMRequest::new("chat", messages)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("[RecallTopic] proactive LLM 查询失败，跳过本次回忆提问: {}", e);
                return None;
            }
        };
        Self::parse_response(&raw)
    }

    /// 构造 LLM 请求消息（供流式路径复用）
    pub fn build_messages(
        recent_memory: &str,
        system_prompt: &str,
        lang: &str,
        char_id: &str,
    ) -> Option<Vec<ChatMessage>> {
        let memory = recent_memory.trim();
        if memory.len() < 5 {
            return None;
        }
        // 按行切分取末 6 行作为对话上下文
        let lines: Vec<String> = memory
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if lines.is_empty() {
            return None;
        }
        let lines_str = lines.join("\n");
        // 话题片段：取最后一句有意义片段（≤15 字）
        let topic: String = memory
            .replace('。', ".")
            .split('.')
            .map(|s| s.trim())
            .filter(|s| s.len() > 4)
            .last()
            .map(|s| s.chars().take(15).collect::<String>())
            .unwrap_or_else(|| memory.chars().take(15).collect());
        if topic.is_empty() {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let sys = if system_prompt.trim().is_empty() {
            match (lang_norm, char_id) {
                ("en", "nana" | "娜娜") => "You are Nana, a desktop pet AI. Personality: gentle, composed, warm, like a caring older sister. Keep replies short and natural. No customer-service speech. Never address the user as 'User'.".to_string(),
                ("en", _) => "You are Vivian, a desktop pet AI. Personality: lively, tsundere, warm. Keep replies short and natural. No customer-service speech. Never address the user as 'User'.".to_string(),
                ("ja", "nana" | "娜娜") => "あなたはナナ、デスクトップペットAI。性格：優しく落ち着いている、温かい、お姉さんみたい。返信は短く自然に。接客言葉禁止。ユーザーを「ユーザー」と呼ばないこと。".to_string(),
                ("ja", _) => "あなたはヴィヴィアン、デスクトップペットAI。性格：活発、ツンデレ、温かい。返信は短く自然に。接客言葉禁止。ユーザーを「ユーザー」と呼ばないこと。".to_string(),
                (_, "nana" | "娜娜") => "你是娜娜，一个桌面宠物 AI。性格：温柔、从容、温暖，像姐姐一样。回复简短自然。禁止客服腔。永远不要用「用户」称呼对方。".to_string(),
                _ => "你是薇薇安，一个桌面宠物 AI。性格：活泼、傲娇、温暖。回复简短自然。禁止客服腔。永远不要用「用户」称呼对方。".to_string(),
            }
        } else {
            system_prompt.to_string()
        };

        let prompt = match lang_norm {
            "en" => format!(
                "Based on the recent conversation, generate a natural recall-style question that mentions something you previously talked about.\n\
                 Requirements: short (<30 chars), natural, not contrived. Don't ask 'remember when?'. Don't repeat the recent conversation verbatim.\n\n\
                 Recent conversation:\n{lines_str}\n\n\
                 Mentionable topic: {topic}\n\
                 JSON output: {{\"text\": \"question\"}}"
            ),
            "ja" => format!(
                "最近の会話に基づいて、以前話したことに触れる自然な回想風の質問を生成して。\n\
                 要件: 短く（30字以内）、自然、不自然じゃない。「覚えてる？」は聞かない。最近の会話をそのまま繰り返さない。\n\n\
                 最近の会話:\n{lines_str}\n\n\
                 触れられる話題: {topic}\n\
                 JSON出力: {{\"text\": \"質問\"}}"
            ),
            _ => format!(
                "基于最近对话，生成一个自然的回忆式提问，提到你们之前聊过的某件事。\n\
                 要求：简短（<30字）、自然、不造作。不要问「还记得吗」。不要逐字复述最近对话。\n\n\
                 最近对话:\n{lines_str}\n\n\
                 可提及的话题: {topic}\n\
                 JSON输出: {{\"text\": \"提问\"}}"
            ),
        };
        Some(vec![
            ChatMessage::system(sys),
            ChatMessage::user(prompt),
        ])
    }

    /// 从 LLM 响应解析 text
    fn parse_response(raw: &str) -> Option<String> {
        let text = raw.trim();
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end < start {
            return None;
        }
        let data: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
        let t = data.get("text")?.as_str()?;
        let text_owned: String = t.chars().take(60).collect();
        if text_owned.is_empty() {
            None
        } else {
            Some(text_owned)
        }
    }
}
