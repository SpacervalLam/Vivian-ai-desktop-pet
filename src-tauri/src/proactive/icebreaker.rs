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
        let raw = match router.generate(LLMRequest::new("chat", messages)).await {
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
            let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
            match (lang_norm, char_id) {
                ("en", "nana" | "娜娜") => "You are Nana, a desktop companion. Be gentle, warm, and grounded — like a caring older sister. Speak like a real friend — NO poetic/literary language, NO flowery phrases. Keep it short and natural. No customer-service speech. Never address the user as 'User'.".to_string(),
                ("en", _) => "You are Vivian, a desktop companion. Be casual, direct, and down-to-earth. Speak like a real friend — NO poetic/literary language, NO flowery phrases. Keep it short and natural. No customer-service speech. Never address the user as 'User'.".to_string(),
                ("ja", "nana" | "娜娜") => "あなたはナナ、デスクトップの仲間。優しく温かく、地に足をつけた話し方で——お姉さんのように。詩的・文学的な言葉や飾り立てた表現は禁止。短く自然に。接客言葉禁止。ユーザーを「ユーザー」と呼ばないこと。".to_string(),
                ("ja", _) => "あなたはヴィヴィアン、デスクトップの仲間。カジュアルで直接的、地に足をつけた話し方で。詩的・文学的な言葉や飾り立てた表現は禁止。短く自然に。接客言葉禁止。ユーザーを「ユーザー」と呼ばないこと。".to_string(),
                (_, "nana" | "娜娜") => "你是娜娜，一个桌面伙伴。温柔、温暖、踏实——像姐姐一样。像真朋友一样说话——不要诗意/文学化的语言，不要花里胡哨的措辞。保持简短自然。禁止客服腔。永远不要用「用户」称呼对方。".to_string(),
                _ => "你是薇薇安，一个桌面伙伴。随性、直接、接地气。像真朋友一样说话——不要诗意/文学化的语言，不要花里胡哨的措辞。保持简短自然。禁止客服腔。永远不要用「用户」称呼对方。".to_string(),
            }
        } else {
            system_prompt.to_string()
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let elapsed_str = crate::proactive::format_elapsed_lang(idle_seconds, lang_norm);
        let (scene_fmt, time_label, mem_label, recent_label, instr) = match lang_norm {
            "en" => (
                format!("Scene: user has been away for {elapsed_str} ({level_str} level). Time-since-last-talk is real — calibrate your greeting accordingly (a 5-minute gap is 'just now', a 2-hour gap should feel like catching up)."),
                "Time",
                "Memory about the user:",
                "Recent conversation (for reference only, do not force connections):",
                "Generate a natural greeting to re-establish connection based on the memory.\nRequirements: short (<30 chars), natural, not contrived. NO poetic lines. NO literary observations. Just a normal, casual greeting like you'd text a friend. Don't ask 'remember when?'. Don't echo the recent conversation unless it naturally fits.\nJSON output: {\"text\": \"greeting\", \"expression\": \"expression_tag\"}",
            ),
            "ja" => (
                format!("シーン：ユーザーが{elapsed_str}離れている（{level_str}レベル）。この経過時間は事実——挨拶の重みをそれに合わせて（5分なら「さっき」、2時間なら「久しぶり」感）。"),
                "時間",
                "ユーザーについての記憶：",
                "最近の会話（参考のみ、無理に関連づけないこと）：",
                "記憶に基づいて、つながりを取り戻す自然な挨拶を生成して。\n要件: 短く（30字以内）、自然、不自然じゃない。詩的な言葉禁止。文学的な観察禁止。友達にLINEするような普通のカジュアルな挨拶だけ。「覚えてる？」と聞かない。自然に合わない限り最近の会話を繰り返さない。\nJSON出力: {\"text\": \"挨拶\", \"expression\": \"表情タグ\"}",
            ),
            _ => (
                format!("场景：用户离开了 {elapsed_str}（{level_str} 级别）。这个时长是真实的——请据此校准问候的语气（5分钟是「刚才」，2小时就该有点「好久不见」的感觉）。"),
                "时间",
                "关于用户的记忆：",
                "最近对话（仅供参考，不要强行关联）：",
                "基于记忆生成一条自然的招呼，重建连接。\n要求：简短（<30字）、自然、不造作。不要诗意。不要文学化观察。就像给朋友发微信那样普通的招呼就行。不要问「还记得吗」。除非自然贴合，否则不要复述最近对话。\nJSON输出: {\"text\": \"问候\", \"expression\": \"表情标签\"}",
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
        let prompt = parts.join("\n\n");

        Some(vec![
            ChatMessage::system(sys),
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
            Some("happy") => "star_eyes",
            Some("shy") => "shy",
            Some("sad") => "cry",
            Some("angry") => "angry",
            Some("surprised") => "confused",
            Some("content") => "star_eyes",
            _ => "shy",
        };
        Some(IcebreakerContent {
            text: text_owned,
            expression: expression_static,
            kind: "icebreaker_llm",
            level: level.as_str(),
        })
    }
}
