//! 破冰话题生成器
//!
//! 在用户长时间未交互时生成自然、个性化的破冰内容。
//! 仅使用 LLM 生成，LLM 失败则不交互（保持自然，避免机械化模板词）。

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 破冰强度级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceBreakerLevel {
    /// 不破冰
    None,
    /// 轻柔
    Gentle,
    /// 温暖
    Warm,
    /// 重连
    Reengage,
}

impl IceBreakerLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            IceBreakerLevel::None => "none",
            IceBreakerLevel::Gentle => "gentle",
            IceBreakerLevel::Warm => "warm",
            IceBreakerLevel::Reengage => "reengage",
        }
    }

    /// 根据用户空闲秒数自动判定级别
    pub fn from_idle(idle_seconds: f64) -> Self {
        if idle_seconds >= 7200.0 {
            // 2 小时以上 → 重连
            IceBreakerLevel::Reengage
        } else if idle_seconds >= 1800.0 {
            // 30 分钟以上 → 温暖
            IceBreakerLevel::Warm
        } else if idle_seconds >= 600.0 {
            // 10 分钟以上 → 轻柔
            IceBreakerLevel::Gentle
        } else {
            IceBreakerLevel::None
        }
    }
}

/// 破冰生成结果
#[derive(Debug, Clone)]
pub struct IcebreakerContent {
    pub text: String,
    pub expression: &'static str,
    /// 内容来源：llm / memory / shared_memory
    pub kind: &'static str,
    pub level: &'static str,
}

/// 破冰内容生成器
pub struct IcebreakerGenerator;

impl IcebreakerGenerator {
    // ============ LLM 路径 ============

    /// 调用 LLM 基于记忆生成破冰内容
    ///
    /// 仅在 `recent_memory` 非空时尝试。LLM 失败则返回 None（不交互）。
    pub async fn generate_llm(
        router: &ModelRouter,
        level: IceBreakerLevel,
        recent_memory: Option<&str>,
        hour: u32,
        system_prompt: &str,
        dialogue_history: &str,
        lang: &str,
        char_id: &str,
        idle_seconds: f64,
    ) -> Option<IcebreakerContent> {
        let messages = Self::build_messages(
            level,
            recent_memory,
            hour,
            system_prompt,
            dialogue_history,
            lang,
            char_id,
            idle_seconds,
        )?;
        let raw = match router.generate(LLMRequest::new("chat", messages)
            .with_character_id(char_id.to_string())).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("[IceBreaker] proactive LLM 查询失败，跳过本次破冰: {}", e);
                return None;
            }
        };
        Self::parse_response(&raw, level)
    }

    /// 构造 LLM 请求消息（供流式路径复用）
    pub fn build_messages(
        level: IceBreakerLevel,
        recent_memory: Option<&str>,
        hour: u32,
        system_prompt: &str,
        dialogue_history: &str,
        lang: &str,
        char_id: &str,
        idle_seconds: f64,
    ) -> Option<Vec<ChatMessage>> {
        if level == IceBreakerLevel::None {
            return None;
        }
        let memory = recent_memory.filter(|m| !m.is_empty())?;
        let memory_trunc: String = memory.chars().take(200).collect();
        let level_str = match level {
            IceBreakerLevel::Gentle => "gentle",
            IceBreakerLevel::Warm => "warm",
            IceBreakerLevel::Reengage => "reengage",
            IceBreakerLevel::None => return None,
        };

        let sys = if system_prompt.trim().is_empty() {
            super::behavior::default_persona_prompt(lang, char_id)
        } else {
            system_prompt.to_string()
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let elapsed_str = crate::proactive::format_elapsed_lang(idle_seconds, lang_norm);
        let (scene_fmt, time_label, mem_label, recent_label, instr) = match lang_norm {
            "en" => (
                format!("Scene: user has been away for {elapsed_str} ({level_str} level). Time-since-last-talk is real — calibrate your greeting accordingly (a 5-minute gap is 'just now', a 2-hour gap need not become a reunion)."),
                "Time",
                "Memory about the user:",
                "Recent conversation (for reference only, do not force connections):",
                "If a greeting fits, send one brief, ordinary thought in your own voice. Use memory only for a specific relevant thread, without staging a reunion or asking for attention. Otherwise leave text and expression empty.\nJSON output: {\"text\": \"greeting or empty\", \"expression\": \"expression_tag or empty\"}",
            ),
            "ja" => (
                format!("シーン：ユーザーが{elapsed_str}離れている（{level_str}レベル）。この経過時間は事実——挨拶の重みをそれに合わせて（5分なら「さっき」、2時間だけで再会の演出はしない）。"),
                "時間",
                "ユーザーについての記憶：",
                "最近の会話（参考のみ、無理に関連づけないこと）：",
                "挨拶が自然に合うなら、自分の言葉で短い一言を。記憶は具体的で今も関係する話題にだけ使い、再会を演出したり相手の注意を求めたりしない。合わなければ text と expression は空にする。\nJSON出力: {\"text\": \"挨拶または空\", \"expression\": \"表情タグまたは空\"}",
            ),
            _ => (
                format!("场景：用户离开了 {elapsed_str}（{level_str} 级别）。这个时长是真实的——请据此校准问候的语气（5分钟是「刚才」，2小时本身不要求演成重逢）。"),
                "时间",
                "关于用户的记忆：",
                "最近对话（仅供参考，不要强行关联）：",
                "适合打招呼时，用自己的口吻说一句简短、平常的话。记忆只用于具体且仍相关的话头，不演重逢，不索要关注；不适合就让 text 和 expression 为空。\nJSON输出: {\"text\": \"问候或空\", \"expression\": \"表情标签或空\"}",
            ),
        };

        let mut parts: Vec<String> = Vec::new();
        parts.push(scene_fmt);
        parts.push(format!("{}: {hour}:00", time_label));
        parts.push(format!("{} {}", mem_label, memory_trunc));
        if !dialogue_history.is_empty() {
            parts.push(format!("{}:\n{}", recent_label, dialogue_history));
        }
        parts.push(instr.to_string());
        parts.push(include_str!("../../prompts/framework/proactive_companionship.en.md").to_string());
        parts.push("No greeting is required. Continue only a specific, still-relevant thread when it naturally fits; otherwise text and expression can be empty. A remembered preference alone does not justify checking up on the user.".to_string());
        let prompt = parts.join("\n\n");

        Some(vec![
            ChatMessage::system(format!("{sys}\n{}", crate::pipeline::prompt_modules::human_feel_rules())),
            ChatMessage::user(prompt),
        ])
    }

    /// 从 LLM 响应解析 IcebreakerContent
    fn parse_response(raw: &str, level: IceBreakerLevel) -> Option<IcebreakerContent> {
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
            return None;
        }
        let expression_static: &'static str = match data
            .get("expression")
            .and_then(|v| v.as_str())
        {
            Some("happy") => "happy",
            Some("shy") => "happy",
            Some("sad") => "dizzy",
            Some("angry") => "angry",
            Some("surprised") => "think",
            Some("content") => "happy",
            _ => "happy",
        };
        Some(IcebreakerContent {
            text: text_owned,
            expression: expression_static,
            kind: "icebreaker_llm",
            level: level.as_str(),
        })
    }
}
