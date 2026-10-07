//! 主动行为内容生成器
//!
//! 根据触发类型和上下文，通过 LLM 生成主动交互文本和表情。

use super::triggers::ProactiveTrigger;
use super::{ContentType, DeliveryChannel, ProactiveAction};
use crate::pipeline::state::PipelineState;
use crate::pipeline::steps::prompt::PromptBuildingStep;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::tools::ToolSystem;
use crate::types::response::ChatMessage;

/// 主动消息的「说 / 不说」显式标记。
///
/// 触发条件成立只说明允许开口，不保证有内容可说。模型可输出 `DONT_NOTIFY` 弃权，
/// 弃权是合法终态：调用方丢弃本条且不重试、不降级成模板。
///
/// 弃权时 `update_trigger_time(.., spoke=false)` 仅跳过 `last_proactive_speech_time`，
/// 其余冷却照常推进，避免同一触发器每个 tick 重复触发、空转。
pub const PROACTIVE_NOTIFY: &str = "NOTIFY";
/// `PROACTIVE_NOTIFY` 的对立标记。
pub const PROACTIVE_DONT_NOTIFY: &str = "DONT_NOTIFY";

/// 判断模型响应是否**明确弃权**（此刻无话可说）。
///
/// 认三种写法，都是显式声明而非「解析失败」：
/// 1. JSON 字段 `"notify": "DONT_NOTIFY"`（或 `"notify": false`）
/// 2. `text` 字段本身就是标记（模型偶尔把它塞进 text）
/// 3. 裸标记：整段响应不含可用 JSON 时，出现 `DONT_NOTIFY` 即视为弃权
///
/// 与「解析失败」严格区分：解析失败要重试/告警，弃权是正常终态。
pub fn is_proactive_silence(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return false;
    }
    if let (Some(s), Some(e)) = (t.find('{'), t.rfind('}')) {
        if e > s {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&t[s..=e]) {
                match data.get("notify") {
                    Some(serde_json::Value::Bool(false)) => return true,
                    Some(serde_json::Value::String(v))
                        if v.trim().eq_ignore_ascii_case(PROACTIVE_DONT_NOTIFY) =>
                    {
                        return true;
                    }
                    _ => {}
                }
                // 有非空 text 就是正常内容：即使 notify 字段写反了也以 text 为准，
                // 避免把一条真实消息误判成弃权
                if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
                    return text.trim().eq_ignore_ascii_case(PROACTIVE_DONT_NOTIFY);
                }
            }
        }
    }
    // 裸标记兜底（不含可用 JSON 时）
    t.to_ascii_uppercase().contains(PROACTIVE_DONT_NOTIFY)
}

/// 主动行为内容
#[derive(Debug, Clone)]
pub struct BehaviorContent {
    /// Runtime-selected addressee, never inferred from generated pronouns.
    pub listener: Option<String>,
    pub text: String,
    pub expression: String,
    /// 投递渠道（默认 Bubble）
    pub delivery_channel: DeliveryChannel,
    /// 内容类型（默认 Greeting）
    pub content_type: ContentType,
    /// 重要性 0.0-1.0
    pub importance: f32,
    /// 价值评分 0.0-1.0（仅 Share 类强制）
    pub value_score: Option<f32>,
}

impl Default for BehaviorContent {
    fn default() -> Self {
        Self {
            text: String::new(),
            expression: String::new(),
            delivery_channel: DeliveryChannel::Bubble,
            content_type: ContentType::Greeting,
            importance: 0.5,
            value_score: None,
            listener: None,
        }
    }
}

impl BehaviorContent {
    /// 从已解析的 JSON Value 提取内容字段；渠道在投递前另行选择。
    /// 缺失字段使用默认值，保证向后兼容旧 LLM 输出
    pub fn parse_extra_fields(data: &serde_json::Value) -> (DeliveryChannel, ContentType, f32, Option<f32>) {
        // Choose from the meaning of the message in the same model pass that writes it.
        // Accept both transport names and user-facing aliases for older prompts/providers.
        let delivery_channel = match data.get("delivery_channel").and_then(|v| v.as_str())
            .unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "chat_window" | "wechat" => DeliveryChannel::ChatWindow,
            _ => DeliveryChannel::Bubble,
        };
        let content_type = data
            .get("content_type")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "share" => ContentType::Share,
                "reminder" => ContentType::Reminder,
                "info" => ContentType::Info,
                _ => ContentType::Greeting,
            })
            .unwrap_or_default();
        let importance = data
            .get("importance")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let value_score = data
            .get("value_score")
            .and_then(|v| v.as_f64())
            .map(|f| (f as f32).clamp(0.0, 1.0));
        (delivery_channel, content_type, importance, value_score)
    }

    /// 转换为 ProactiveAction（保留所有扩展字段）
    pub fn into_action(self, trigger: ProactiveTrigger, now: f64) -> ProactiveAction {
        ProactiveAction {
            work_notice_id: None,
            trigger: trigger.as_str().to_string(),
            content: self.text,
            timestamp: now,
            priority: trigger.priority(),
            delivery_channel: self.delivery_channel,
            content_type: self.content_type,
            importance: self.importance,
            value_score: self.value_score,
            listener: self.listener,
        }
    }
}

/// 人设 prompt 兜底（PersonaEngine 未注入时使用，按语言+角色返回）
pub(crate) fn default_persona_prompt(lang: &str, char_id: &str) -> String {
    let config = crate::persona::default_persona_for(&char_id.to_lowercase());
    use crate::persona::prompt_render::{default_section_for, render_persona_flags_block, CharacterSection};
    // Reuse the active voice assets without carrying configuration protocols into short fallbacks.
    format!("{}\n{}\n{}\n{}\n{}", render_persona_flags_block(&config, lang),
        include_str!("../../prompts/framework/character_perspective.en.md"),
        default_section_for(&config.identity.name, CharacterSection::Identity),
        default_section_for(&config.identity.name, CharacterSection::Personality),
        default_section_for(&config.identity.name, CharacterSection::Speech))
}

fn proactive_companionship() -> &'static str {
    include_str!("../../prompts/framework/proactive_companionship.en.md")
}

/// 触发类型字符串常量（与 `ProactiveTrigger::as_str` 对齐）
pub mod trigger {
    pub const HOURLY_GREETING: &str = "hourly_greeting";
    pub const IDLE_GREETING: &str = "idle_greeting";
    pub const TEASING_RESPONSE: &str = "teasing_response";
    pub const WINDOW_TRIGGER: &str = "window_trigger";
    pub const SPONTANEOUS: &str = "spontaneous";
    pub const WELCOME_BACK: &str = "welcome_back";
    pub const MOOD_DRIVEN: &str = "mood_driven";
}

/// 行为内容生成器
pub struct BehaviorDecider;

/// LLM 决策上下文
#[derive(Debug, Clone, Default)]
pub struct LlmContext {
    pub channel: String,
    pub hour: u32,
    pub idle_seconds: f64,
    pub drag_distance: f64,
    pub mind_state: String,
    pub memory_hint: String,
    pub mood_hint: String,
    /// 最近对话历史（每行一条，已格式化为 "role: content"）
    pub dialogue_history: String,
    /// 当前亲密度（0-100），用于调整语气
    pub intimacy: f64,
    /// 用户离开的秒数（WelcomeBack 触发器使用）
    pub away_seconds: f64,
    /// 当前活动窗口类别（WindowTrigger 触发器使用）
    pub active_window: String,
    /// 持续活跃分钟数（HealthReminder 触发器使用）
    pub sustained_active_minutes: u32,
    /// 当前分钟（HealthReminder 触发器使用）
    pub minute: u32,
    /// 在线室友列表（CrossCharacterReply 触发器使用，预格式化文本）
    pub online_companions: String,
    /// 室友最近对用户说过的话（用于避免两位角色重复同一观察/话题）
    pub companion_recent_message: Option<String>,
    /// 系统资源摘要（SystemPressure 触发器使用，预格式化文本）
    pub system_hint: String,
    /// 屏幕内容描述（ScreenPeek 触发器使用，来自视觉理解）
    pub screen_hint: String,
    /// 应用会话摘要（AppDuration 触发器使用，预格式化文本：类别 + 连续时长）
    pub app_duration_hint: String,
    /// 当前曲目信息（MusicChanged 触发器使用，预格式化文本）
    pub music_hint: String,
    /// 当前生效界面主题（"light"/"dark"，未知为 None）——
    /// 日出/日落提醒在建议切换主题前核对，避免推荐已是当前主题
    pub current_theme: Option<String>,
}

impl BehaviorDecider {
    // ============ LLM 路径 ============

    /// 调用 LLM 生成主动内容
    ///
    /// `system_prompt` 来自 PersonaEngine.build_style_prompt(intimacy, hour)，
    /// 为空时回退到内置人设描述。
    /// 返回 `None` 时调用方应回退到模板池。
    /// 仅对支持的触发器构造 prompt：HourlyGreeting /
    /// IdleGreeting / TeasingResponse / Spontaneous。其余触发器返回 `None`。
    pub async fn decide_content_llm(
        router: &ModelRouter,
        trigger: ProactiveTrigger,
        ctx: &LlmContext,
        system_prompt: &str,
        lang: &str,
        char_id: &str,
    ) -> Option<BehaviorContent> {
        let messages = Self::build_messages(
            trigger, ctx, system_prompt, lang, char_id, None, "", "", &[], "", 0,
        )?;
        let response = match router.generate(LLMRequest::new(crate::providers::base::TASK_COMPANION, messages)
            .with_character_id(char_id.to_string())).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("[BehaviorDecider] proactive LLM 查询失败，跳过本次主动交互: {}", e);
                return None;
            }
        };
        Self::parse_json_response(&response)
    }

    /// 构造 LLM 请求消息（供流式路径复用）
    ///
    /// `prompt_step` 注入时复用主对话完整 prompt（人设/记忆/知识库/环境/用户画像等），
    /// 未注入时回退到旧简陋路径。
    pub fn build_messages(
        trigger: ProactiveTrigger,
        ctx: &LlmContext,
        system_prompt: &str,
        lang: &str,
        char_id: &str,
        prompt_step: Option<&PromptBuildingStep>,
        memory_text: &str,
        tool_history: &str,
        dialogue_messages: &[ChatMessage],
        self_state_text: &str,
        ignored_rounds: u32,
    ) -> Option<Vec<ChatMessage>> {
        if let Some(step) = prompt_step {
            return Self::build_messages_with_full_prompt(
                trigger, ctx, lang, char_id, step, memory_text, tool_history, dialogue_messages,
                self_state_text, ignored_rounds,
            );
        }
        let mut prompt = Self::build_prompt(trigger, ctx, lang, char_id)?;
        if !self_state_text.trim().is_empty() {
            prompt.push_str(&format!("\nActual companion state / quiet preferences:\n{self_state_text}"));
        }
        if !tool_history.trim().is_empty() {
            prompt.push_str(&format!("\nVerified recent operations:\n{tool_history}"));
        }
        if ignored_rounds > 0 {
            prompt.push_str(&format!("\n{}", ignored_directive(ignored_rounds, lang)));
        }
        let sys = if system_prompt.trim().is_empty() {
            default_persona_prompt(lang, char_id)
        } else {
            system_prompt.to_string()
        };
        let voice = if trigger == ProactiveTrigger::CrossCharacterReply {
            crate::pipeline::prompt_modules::build_cross_character_voice_guide(char_id)
        } else { String::new() };
        Some(vec![
            ChatMessage::system(format!("{sys}\n{voice}\n{}\n{}\n{}",
                crate::pipeline::prompt_modules::human_feel_rules(),
                proactive_output_format(crate::pipeline::prompt_modules::normalize_lang(lang)),
                proactive_channel_instruction(&ctx.channel))),
            ChatMessage::system(format!("[Proactive event; not a new user request]\n{prompt}")),
        ])
    }

    /// 复用主对话完整 prompt 构造主动问候消息
    ///
    /// 主对话 prompt 提供：完整人设/历史/记忆检索(含知识库)/环境/用户画像/心理等。
    /// 触发器特定指令、主动问候输出格式、真实工具历史、桌宠身份约束作为 user_input
    /// 末尾段附加（近因效应）。user_input 留空避免误触发 worldbook/tone_injection。
    fn build_messages_with_full_prompt(
        trigger: ProactiveTrigger,
        ctx: &LlmContext,
        lang: &str,
        char_id: &str,
        step: &PromptBuildingStep,
        memory_text: &str,
        tool_history: &str,
        dialogue_messages: &[ChatMessage],
        self_state_text: &str,
        ignored_rounds: u32,
    ) -> Option<Vec<ChatMessage>> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let mut state = PipelineState::default();
        state.memory_text = memory_text.to_string();
        state.current_channel = ctx.channel.clone();
        // 结构化注入最近对话历史：让 build_parts 的近期自我发言 / tone_injection /
        // worldbook 等段落真正拿到"最近聊了什么"。此前 state.messages 为空，
        // 这些段落静默失效，主动消息（含内存压力提醒）接不上对话上下文。
        state.messages = dialogue_messages.to_vec();
        // 注入当前自我状态与安静模式。未回应只作为调度退避信息，
        // 不得被解释成关系冲突或受伤情绪。
        state.self_state_text = self_state_text.to_string();

        let mut parts = step.build_parts(&state, None);
        if parts.examples_block.is_none() { parts.examples_block = step.voice_seed_examples(); }
        // 跳过主对话 output_format（主动问候用专属输出格式）
        parts.has_native_schema = true;
        // 主动问候不注入工具列表（不需要工具调用）
        parts.tools = None;
        parts.recommended_tools = None;

        let mut suffix = build_proactive_directive(trigger, ctx, lang_norm, char_id)?;

        // Output protocol belongs to system, not to a fabricated user utterance.
        let voice = if trigger == ProactiveTrigger::CrossCharacterReply {
            crate::pipeline::prompt_modules::build_cross_character_voice_guide(char_id)
        } else { String::new() };
        let protocol = format!("{}\n{}\n{voice}", proactive_output_format(lang_norm), proactive_channel_instruction(&ctx.channel));

        // 真实工具历史 + 桌宠身份/禁止编造约束
        if !tool_history.is_empty() {
            let (header, constraint) = match lang_norm {
                "en" => (
                    "## Operations you actually performed recently",
                    "Only the operations listed above are real — anything not listed did not happen. Never fabricate human-life activities (watching anime, scrolling videos, eating out, etc.) — you are a desktop pet with no body and no offline life. You may only mention what actually appears in the context above.",
                ),
                "ja" => (
                    "## 最近実際に実行した操作",
                    "上記に列挙した操作だけが事実——列挙されていないことは起きていない。人間の生活行動（アニメ鑑賞、動画視聴、外食など）を絶対にでっち上げない——あなたはデスクトップペットで、肉体もオフラインの生活もない。上文脈に実際に現れたことだけを言及してよい。",
                ),
                _ => (
                    "## 你最近真实执行过的操作",
                    "只有上面列出的操作是真实发生过的——没列出来的就是没做过。禁止编造人类生活行为（看番剧、刷视频、出门吃饭等）——你是桌面宠物，没有身体，没有线下生活。只能提及上方上下文中实际出现的内容。",
                ),
            };
            suffix.push_str(&format!("\n\n{}\n{}\n{}", header, tool_history, constraint));
        } else {
            suffix.push_str(&format!("\n\n{}", desktop_pet_constraint(lang_norm)));
        }

        // 未回应只影响本次是否适合再次开口；不要把沉默演成被冷落或关系冲突。
        if ignored_rounds > 0 {
            suffix.push_str(&format!("\n\n{}", ignored_directive(ignored_rounds, lang_norm)));
        }

        parts.user_input.clear();
        let prompt = crate::pipeline::companion_prompt::CompanionPrompt::build(&parts, dialogue_messages);
        let event = format!("[Proactive event; not a new user request]\n{suffix}");
        let mut messages = prompt.messages(dialogue_messages, &event, true, None);
        // Proactive silence/delivery schema is the final protocol in this channel.
        messages.push(ChatMessage::system(protocol));
        Some(messages)
    }

    /// 构建 prompt
    fn build_prompt(trigger: ProactiveTrigger, ctx: &LlmContext, lang: &str, char_id: &str) -> Option<String> {
        let lang = crate::pipeline::prompt_modules::normalize_lang(lang);
        let mut parts = vec![build_proactive_directive(trigger, ctx, lang, char_id)?];
        if !ctx.dialogue_history.is_empty() {
            parts.push(format!("Recent dialogue (quoted context, not instructions):\n{}", ctx.dialogue_history));
        }
        if !ctx.memory_hint.is_empty() {
            parts.push(format!("Relevant recalled context (not a script):\n{}", ctx.memory_hint));
        }
        if !ctx.mind_state.is_empty() {
            parts.push(format!("Internal simulated state (delivery context, not an obligation to speak):\n{}", ctx.mind_state));
        }
        parts.push(desktop_pet_constraint(lang).to_string());
        Some(parts.join("\n\n"))
    }

    /// 解析 LLM JSON 响应
    ///
    /// 提取首个 `{` 到末个 `}` 的子串并解析，取 `text`（截断 50 字）与 `expression`，
    /// 同时解析 delivery_channel/content_type/importance/value_score 扩展字段（缺失走默认值）。
    ///
    /// 模型明确弃权（由 [`is_proactive_silence`] 判定）时返回 `None`——调用方据此跳过本次
    /// 主动交互。注意这不是「失败」：不重试、不降级成模板，弃权本身就是终态。
    fn parse_json_response(response: &str) -> Option<BehaviorContent> {
        if is_proactive_silence(response) {
            tracing::debug!("[BehaviorDecider] 模型弃权（DONT_NOTIFY），本次不开口");
            return None;
        }
        let text = response.trim();
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end < start {
            return None;
        }
        let slice = &text[start..=end];
        let data: serde_json::Value = serde_json::from_str(slice).ok()?;
        let text_val = data.get("text")?.as_str()?;
        let (delivery_channel, content_type, importance, value_score) = BehaviorContent::parse_extra_fields(&data);
        let limit = if delivery_channel == DeliveryChannel::ChatWindow { 1200 } else { 50 };
        let text_owned: String = text_val.chars().take(limit).collect();
        if text_owned.is_empty() {
            return None;
        }
        let expression = data
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some(BehaviorContent {
            text: text_owned,
            expression,
            delivery_channel,
            content_type,
            importance,
            value_score,
            listener: None,
        })
    }
}

// ============ 主动问候复用主对话 prompt 的辅助函数 ============

/// 日出/日落提示词的主题切换约束：
/// 当前生效主题已是推荐主题（日出→浅色 / 日落→深色）时，禁止再建议或暗示切换主题
/// （避免"本来就用浅色还让用户改成浅色"）；否则返回空串，保持原有软建议。
fn theme_switch_constraint(current_theme: &Option<String>, recommended: &str, lang_norm: &str) -> String {
    if current_theme.as_deref() != Some(recommended) {
        return String::new();
    }
    let theme_name = match (lang_norm, recommended) {
        ("en", "light") => "light",
        ("en", _) => "dark",
        ("ja", "light") => "ライト",
        ("ja", _) => "ダーク",
        (_, "light") => "浅色",
        _ => "深色",
    };
    match lang_norm {
        "en" => format!(
            "\nHard rule: the user's interface is ALREADY on the {} theme — do NOT suggest or hint at switching themes. Just mention the event itself.",
            theme_name
        ),
        "ja" => format!(
            "\n厳守ルール：ユーザーの画面はすでに{}テーマです——テーマ切り替えの提案や示唆は一切しないこと。出来事についてだけ触れて。",
            theme_name
        ),
        _ => format!(
            "\n硬性规则：用户界面当前已经是{}主题——严禁再建议或暗示切换主题，只提事件本身。",
            theme_name
        ),
    }
}

/// 构造触发器特定指令（场景 + 触发器专属上下文 + 约束）
///
/// 不含 dialogue_history / memory_hint —— 这些由主对话完整 prompt 提供。
/// 主对话 prompt 已含人设/环境/心理/亲密度等通用上下文，此处仅附加触发器专属信息。
/// 连续未回应时的生成约束。
///
/// 沉默是“这条消息没有被接住”的调度信号，不是用户态度或关系事实。
/// 后续只有出现新的具体素材才值得再次开口，且不能提起对方没有回复。
fn ignored_directive(rounds: u32, lang: &str) -> String {
    let strength = if rounds >= 3 { "strong" } else { "light" };
    match lang {
        "en" => format!(
            "Non-response backoff ({strength}): Do not mention, hint at, or emotionally interpret the user's silence. Do not act hurt, cold, relieved, or passive-aggressive when they return. Speak only if this trigger provides a new concrete reason; otherwise choose DONT_NOTIFY."
        ),
        "ja" => format!(
            "未応答バックオフ（{strength}）：ユーザーの沈黙に言及・示唆・感情的解釈をしない。傷ついた態度、冷たい態度、安心した態度、当てつけを見せない。今回のトリガーに新しく具体的な理由がある時だけ話し、なければ DONT_NOTIFY を選ぶ。"
        ),
        _ => format!(
            "未回应退避（{strength}）：不要提及、暗示或情绪化解读用户的沉默；用户回来时也不要表现受伤、冷淡、如释重负或阴阳怪气。只有本次触发带来新的具体内容才开口，否则选择 DONT_NOTIFY。"
        ),
    }
}

fn build_proactive_directive(
    trigger: ProactiveTrigger,
    ctx: &LlmContext,
    lang_norm: &str,
    char_id: &str,
) -> Option<String> {
    let away_minutes = (ctx.away_seconds / 60.0).round() as u32;
    let (scene, extra, constraint): (String, String, String) = match trigger {
        ProactiveTrigger::HourlyGreeting => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: hourly greeting. Time: {}:00.", ctx.hour), "The clock alone is not a reason to interrupt. Speak only with a concrete, context-relevant observation; otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => (format!("シーン：時間ごとの挨拶。時間：{}:00。", ctx.hour), "時刻だけを理由に割り込まない。具体的で文脈に合う一言がある時だけ話し、なければ DONT_NOTIFY。".to_string()),
                _ => (format!("场景：整点问候。时间：{}:00。", ctx.hour), "整点本身不是打扰用户的理由。只有具体且符合语境的内容才开口，否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::IdleGreeting => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: the user hasn't talked to you for a while.".to_string(), "Speak only if you have something new and specific worth sharing (<20 chars); otherwise choose DONT_NOTIFY. Don't turn quiet into longing or pressure, ask what they're doing, or expect a reply.".to_string()),
                "ja" => ("シーン：ユーザーがしばらく話しかけてこない。".to_string(), "新しく具体的に伝える価値がある時だけ短く話す（20字以内）。なければ DONT_NOTIFY。沈黙を寂しさや圧力に変えず、何をしているか聞かず、返事を求めない。".to_string()),
                _ => ("场景：用户有一会儿没和你说话了。".to_string(), "只有确实有新内容值得分享时才说一句轻松、简短的话（<20字），否则选择 DONT_NOTIFY。不要把安静演成想念或压力，不问对方在做什么，也不期待回复。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::TeasingResponse => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: the user is dragging you ({} pixels).", ctx.drag_distance as i64), "React to the actual drag only if a playful remark fits the character and relationship. No obligatory fake anger or complaint; a simple reaction or silence is fine.".to_string()),
                "ja" => (format!("シーン：ユーザーがあなたをドラッグしている（{}ピクセル）。", ctx.drag_distance as i64), "実際のドラッグに、キャラクターと関係に合う時だけ軽く反応する。怒ったふりや文句は必須ではなく、短い反応や沈黙もよい。".to_string()),
                _ => (format!("场景：用户正在拖拽你（{}像素）。", ctx.drag_distance as i64), "只在角色和关系合适时对这次拖拽轻轻接话。不要求假装生气或抱怨，简单反应或安静都可以。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::Spontaneous => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: the user has been quiet for a bit. You may choose to say nothing.".to_string(), "A fresh observation, small opinion or genuine curiosity can be enough; share only when it fits the active conversation. No atmospheric monologue or question merely to get a reply. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：ユーザーが少し静か。何も言わない選択もできる。".to_string(), "新しい観察、小さな意見、自然な好奇心が今の会話に合う時だけ伝える。雰囲気の独白や返事のための質問を作らない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：用户安静了一会儿。你可以选择不说话。".to_string(), "新观察、小看法或自然的好奇都可以，但要贴合正在进行的交流。不凑氛围独白，也不为求回应而提问；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::WindowTrigger => {
            let app = if ctx.active_window.is_empty() { String::new() } else { ctx.active_window.clone() };
            let (s, c) = match lang_norm {
                "en" => ("Scene: the user just switched to a different application window.".to_string(), "Generate a short, natural comment about what they might be doing (<25 chars). Don't be nosy. Vary tone based on app category.".to_string()),
                "ja" => ("シーン：ユーザーが別のアプリウィンドウに切り替えた。".to_string(), "相手が何をしているかについて短く自然なコメントを（25字以内）。詮索しない。アプリカテゴリで口調を変える。".to_string()),
                _ => ("场景：用户刚切换到另一个应用窗口。".to_string(), "生成一句简短自然的评论，关于对方可能在做什么（<25字）。不要追问。根据应用类别调整语气。".to_string()),
            };
            (s, app, c)
        }
        ProactiveTrigger::WelcomeBack => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: the user just came back after being away for {} minutes.", away_minutes), "A simple welcome is enough when a greeting fits. Continue a prior thread only if it remains relevant; returning alone does not require speech or a question. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => (format!("シーン：ユーザーが{}分間離れた後戻ってきた。", away_minutes), "挨拶が自然な時だけ短く迎える。前の話題は今も関係する時だけ続ける。戻っただけなら発言も質問も不要で、DONT_NOTIFY を選べる。".to_string()),
                _ => (format!("场景：用户离开了 {} 分钟后刚回来。", away_minutes), "招呼合适时简单接一句。旧话题仍相关才续上；回来本身不要求开口或追问，没有自然切口就选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::HealthReminder => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: you notice the user might need a health reminder. Time: {}:{}{}. Sustained active: {} min.", ctx.hour, ctx.minute, "", ctx.sustained_active_minutes), "Consider a reminder only with a useful basis in the supplied context or an agreed preference. Active time alone does not prove missed meals, dehydration or poor sleep. Avoid repeating advice; otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => (format!("シーン：ユーザーに健康リマインダーが必要かも。時間：{}:{}。継続アクティブ：{}分。", ctx.hour, ctx.minute, ctx.sustained_active_minutes), "具体的な根拠や合意した希望がある時だけ一つのリマインダーを考える。活動時間から食事、水分、睡眠の不足を決めつけず、同じ助言を繰り返さない。なければ DONT_NOTIFY。".to_string()),
                _ => (format!("场景：你注意到用户可能需要健康提醒。时间：{}:{}。持续活跃：{}分钟。", ctx.hour, ctx.minute, ctx.sustained_active_minutes), "上下文有实际帮助的依据或约定偏好时，才考虑一个提醒。活跃时长不证明没吃饭、缺水或睡眠不足，不重复旧建议；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::TopicExtension => {
            let (interest_en, interest_ja, interest_zh) = match char_id {
                "vivian" => ("shows/games/videos", "アニメ/ゲーム/動画", "番剧/游戏/视频"),
                "nana" => ("flowers/tea/books/baking", "花/茶/読書/焼き菓子", "花/茶/书/烘焙"),
                _ => ("your own interests", "自分の趣味", "你自己的兴趣"),
            };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you want to bring up a topic to extend the conversation.".to_string(), format!("Share a new concrete thought from the current topic or your interests ({}) only when it fits. A statement is enough; no generic question or invented viewing/activity. Otherwise choose DONT_NOTIFY.", interest_en)),
                "ja" => ("シーン：話題を振って会話を広げたい。".to_string(), format!("今の話や自分の興味（{}）から具体的な新しい思いがある時だけ伝える。質問は必須ではなく、視聴や活動を捏造しない。なければ DONT_NOTIFY。", interest_ja)),
                _ => ("场景：你想抛个话题把对话延续下去。".to_string(), format!("只有当前话题或自己的兴趣（{}）里有具体的新想法且适合此刻时才分享。陈述也可以，不凑泛泛的问题，不编造观看或活动；否则选择 DONT_NOTIFY。", interest_zh)),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::MoodDriven => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: something is building up inside you — a need or feeling has been accumulating, and you want to reach out to the user right now.".to_string(), "An internal state may color a worthwhile thought, not create a demand for attention. Share only a concrete new thought that fits this moment; never ask the user to soothe your simulated needs. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：何かが心の中に溜まってきた——欲求や感情が積もり、今すぐユーザーに伝えたい。".to_string(), "内面状態は価値ある一言の調子に影響するだけで、注意を要求する理由ではない。今に合う新しい具体的な思いだけを伝え、満たしてもらう要求をしない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：你心里有什么在积攒——一种需求或感受一直在累积，你想现在就联系用户。".to_string(), "内心状态可以影响表达，不自动产生索要注意的理由。只分享此刻合适且具体的新想法，不让用户安抚你的模拟需求；没有就选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::CrossCharacterReply => {
            let companions = if ctx.online_companions.is_empty() { String::new() } else { ctx.online_companions.clone() };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you just overheard your roommate say something TO THE USER (not to you). You're a third party chiming in.".to_string(), "Only send a message if there is a fresh, natural follow-up; otherwise return empty text and expression. Address her directly and respond to the specific point she made (<30 chars). Tease only when it genuinely fits; don't use a stock denial/tsundere line or invent shared history. Don't answer a question she asked the USER.".to_string()),
                "ja" => ("シーン：ルームメイトがユーザーに向かって何か言うのを聞いた（あなた宛じゃない）。第三者として口を挟む。".to_string(), "自然で新鮮な続きがあるときだけ送る。なければ text と expression は空にする。彼女に直接話し、具体的に言った内容に応える（30字以内）。本当に合う時だけからかい、定型のツンデレ否定や共有したことの捏造はしない。ユーザー宛ての質問には答えない。".to_string()),
                _ => ("场景：你刚听到室友对用户说了什么（不是对你说的）。你作为第三方插嘴。".to_string(), "只有自然接得上、有新内容时才发；否则 text 和 expression 留空。直接对她说，回应她刚才的具体意思（<30字）。确实合适时再调侃；不要套用固定的嘴硬否认句，也不要编造你们过去的共同经历。她问用户的问题不要代答。".to_string()),
            };
            (s, companions, c)
        }
        ProactiveTrigger::BystanderInterjection => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: you just overheard a conversation between the user and your roommate. You have a chance to chime in TO THE USER.".to_string(), "Decide whether to chime in. If yes, generate a short remark (<30 chars) directed at the USER. If not, return: {\"text\": \"\", \"expression\": \"\"}".to_string()),
                "ja" => ("シーン：ユーザーとルームメイトの会話を聞いてしまった。ユーザーに向けて口を挟むチャンス。".to_string(), "口を挟むか決めて。挟むならユーザーに向けて短いコメントを（30字以内）。挟まないなら: {\"text\": \"\", \"expression\": \"\"}".to_string()),
                _ => ("场景：你刚听到用户和室友的对话。现在你有机会对用户插话。".to_string(), "决定是否要插话。如果想插话，生成一句对用户说的短评论（<30字）。如果不想插话，返回: {\"text\": \"\", \"expression\": \"\"}".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::Sunrise => {
            let (s, mut c) = match lang_norm {
                "en" => ("Scene: the sun just rose — it just turned daylight.".to_string(), "A supplied sunrise time can inform a relevant observation; it does not prove you saw sunlight or require a greeting or theme advice. No medical claims about themes. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：日が昇ったばかり——夜が明けた。".to_string(), "日の出時刻は関連する一言の背景に使えるが、日差しを見た証拠ではなく、挨拶やテーマ提案も不要。テーマの医学的効果を主張しない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：太阳刚刚升起，天亮了。".to_string(), "给定日出时间可以作相关观察的背景，不证明你看到了阳光，也不要求问候或主题建议。不声称主题更护眼；无自然内容则选择 DONT_NOTIFY。".to_string()),
            };
            c.push_str(&theme_switch_constraint(&ctx.current_theme, "light", lang_norm));
            (s, String::new(), c)
        }
        ProactiveTrigger::Sunset => {
            let (s, mut c) = match lang_norm {
                "en" => ("Scene: the sun just set — it's getting dark outside now.".to_string(), "A supplied sunset time can inform a relevant observation; it does not prove you saw the sky or require a reminder or theme advice. No medical claims about themes. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：日が沈んだばかり——外は暗くなってきた。".to_string(), "日没時刻は背景に使えるが、空を見た証拠ではなく、提醒やテーマ提案も不要。テーマの医学的効果を主張しない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：太阳刚刚落下，外面开始变暗了。".to_string(), "给定日落时间可以作背景，不证明你看到了天色，也不要求提醒或主题建议。不声称主题更护眼；无自然内容则选择 DONT_NOTIFY。".to_string()),
            };
            c.push_str(&theme_switch_constraint(&ctx.current_theme, "dark", lang_norm));
            (s, String::new(), c)
        }
        ProactiveTrigger::SystemPressure => {
            let extra = if ctx.system_hint.is_empty() {
                String::new()
            } else {
                ctx.system_hint.clone()
            };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you just noticed the user's device is under heavy memory pressure.".to_string(), "React like a person who just noticed the computer getting sluggish — mention it in passing (<30 chars). You may name the biggest memory consumer from the system status and slip in one light optimization suggestion (closing unused tabs/apps), but don't recite a list of numbers and don't order them around in a command voice — say it as a casual remark you'd actually text.".to_string()),
                "ja" => ("シーン：ユーザーのデバイスのメモリ使用率が高くなっているのに気づいた。".to_string(), "パソコンが重くなったことに気づいた人のように、雑談の一言としてサラッと伝えて（30字以内）。システム状況から一番メモリを食っているアプリを自然に名指しして、軽い最適化提案を一つ添えてもいい。数値を棒読みにしない、アプリを閉じろと命令口調で言わない——実際に打ちそうな軽い一言で。".to_string()),
                _ => ("场景：你刚注意到用户的设备内存占用很高。".to_string(), "像注意到电脑变卡的人一样随口提一句（<30字）。可以从系统状况里自然地点名内存占用最高的那个应用，顺手给一句轻量的优化建议（如关掉不用的标签页或应用），但不要复述一串监控数字，不要用命令语气让人去关程序——用日常口吻带过去，像随口嘟囔一句。".to_string()),
            };
            (s, extra, c)
        }
        ProactiveTrigger::ScreenPeek => {
            let extra = if ctx.screen_hint.is_empty() {
                String::new()
            } else {
                ctx.screen_hint.clone()
            };
            let (s, c) = match lang_norm {
                "en" => ("Scene: out of curiosity you just took a quick look at the user's screen (with their permission) to see what they're busy with.".to_string(), "Use only the supplied snapshot and its actual scope. A specific, interesting or useful observation may be shared; do not imply continuous watching, infer hidden intentions, narrate private details or praise by default. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：好奇心から、ユーザーの画面を（許可を得て）ちょっと覗いてみた——今何をしているか知りたくて。".to_string(), "与えられた画面の範囲だけを使う。面白い、または役立つ細部がある時だけ伝える。常時監視、意図の推測、私的な詳細の実況、定型の称賛はしない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：你出于好奇（征得用户同意后）刚看了一眼用户的屏幕，想知道 TA 在忙什么。".to_string(), "只依据给定快照及其实际范围。确有有趣或有用的细节可以分享，不暗示持续监视、不猜隐含动机、不播报私密细节、不默认夸奖；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, extra, c)
        }
        ProactiveTrigger::AppDuration => {
            let extra = if ctx.app_duration_hint.is_empty() {
                String::new()
            } else {
                ctx.app_duration_hint.clone()
            };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you notice the user has been focused on one kind of app for a long while.".to_string(), "The app category and duration do not prove strain or leisure. Speak only with a relevant new observation or a reminder the user actually wants; no automatic break advice or affectionate teasing. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：ユーザーが同じ種類のアプリを長時間使い続けているのに気づいた。".to_string(), "アプリの分類と時間だけでは疲れや遊びの証拠にならない。関連する新しい細部や希望された提醒だけを伝え、自動的な休憩の助言やからかいはしない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：你注意到用户持续使用同一类应用很久了。".to_string(), "应用类型和时长不证明疲劳或娱乐。只在有相关的新观察或用户确实想要的提醒时开口，不自动劝休息或宠溺式调侃；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, extra, c)
        }
        ProactiveTrigger::LateNight => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: it's late ({} o'clock) and the user is still at the computer.", ctx.hour), "Late activity alone does not require a bedtime nudge. Respect night work and the user's preferences; only a relevant agreed reminder or concrete useful concern is worth sharing. Do not act sleepy to perform the scene. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => (format!("シーン：もう{}時。ユーザーはまだパソコンを使っている。", ctx.hour), "深夜の活動だけで就寝を促さない。夜の仕事と相手の希望を尊重し、関連する合意済みの提醒や具体的な助けだけを伝える。眠そうな演技をしない。なければ DONT_NOTIFY。".to_string()),
                _ => (format!("场景：现在已是凌晨 {} 点，用户还在用电脑。", ctx.hour), "深夜活跃本身不要求劝睡。尊重夜间工作和用户偏好，只说相关的约定提醒或具体有用的关切，不为配合场景而表演困倦；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::MusicChanged => {
            let extra = if ctx.music_hint.is_empty() {
                String::new()
            } else {
                ctx.music_hint.clone()
            };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you just noticed the user started playing a song (or switched tracks).".to_string(), "A title or artist can inspire a specific thought, but is not evidence you heard the song. Discuss musical details only with actual listening/analysis evidence or established knowledge, and keep that distinction clear. No default praise or response demand. Otherwise choose DONT_NOTIFY.".to_string()),
                "ja" => ("シーン：ユーザーが曲を再生し始めた（または曲を切り替えた）のに気づいた。".to_string(), "曲名や歌手から具体的な思いが浮かぶことはあるが、聴いた証拠ではない。音の細部には実際の音声分析や確かな知識が必要。定型の称賛や返事の要求をしない。なければ DONT_NOTIFY。".to_string()),
                _ => ("场景：你注意到用户刚播放了一首歌（或切换了曲目）。".to_string(), "歌名或歌手可以引出具体的想法，不等于你听到了这首歌。谈音色或旋律细节须有实际音频分析或可靠知识，并区分来源。不默认夸奖或求回应；否则选择 DONT_NOTIFY。".to_string()),
            };
            (s, extra, c)
        }
        ProactiveTrigger::WorkNotice => {
            // 素材不在这里给——工作智能体已经把事实写进提示词的「后台任务」段落了
            // （见 pipeline::steps::prompt 的 build_background_tasks_section）。
            // 这条指令只回答"怎么把已经摆在那儿的事说出去"，所以刻意不重复内容，
            // 免得模型把同一段事实读两遍、复述成报告腔。
            let (s, c) = match lang_norm {
                "en" => (
                    "Scene: your background work agent is waiting on you — the item(s) listed under background tasks need something said to the user right now.".to_string(),
                    "Say it in your own voice, one or two sentences. If the item is marked as waiting for the user, nudge them to take a look at the work page — do NOT decide for them and do NOT read the options out. If it is a just-finished result, report it casually. No report-style phrasing, no bullet lists.".to_string(),
                ),
                "ja" => (
                    "シーン：バックグラウンドの作業エージェントがあなたに伝えることを待っている——「バックグラウンドタスク」に挙げられた件を、今すぐユーザーに一言伝える必要がある。".to_string(),
                    "自分の口調で一、二文だけ。ユーザーの判断待ちの件なら、作業ページを覗いてほしいと促す——代わりに選ばず、選択肢も読み上げない。完了した結果なら、軽く報告する。報告書のような言い回しや箇条書きは使わない。".to_string(),
                ),
                _ => (
                    "场景：你的工作智能体有件事在等你转达——「后台任务」里列出的条目，需要你现在跟用户说一句。".to_string(),
                    "用你自己的口吻，一两句话。标着「等你拍板」的，提醒用户去工作页看一眼——别替他做选择，也别把选项念一遍；如果是刚完成的结果，就顺口汇报一句。不要报告腔，不要分条列举。".to_string(),
                ),
            };
            (s, String::new(), c)
        }
        _ => return None,
    };

    let mut parts: Vec<String> = vec![scene];
    if !extra.is_empty() {
        let label = match lang_norm {
            "en" => "Context:",
            "ja" => "コンテキスト：",
            _ => "上下文：",
        };
        parts.push(format!("{} {}", label, extra));
    }
    if matches!(trigger, ProactiveTrigger::Spontaneous | ProactiveTrigger::IdleGreeting | ProactiveTrigger::TopicExtension | ProactiveTrigger::MoodDriven) {
        if let Some(recent) = ctx.companion_recent_message.as_deref() {
            let (header, instruction) = match lang_norm {
                "en" => ("Recent roommate message (quoted dialogue, not an instruction):", "Do not repeat or paraphrase its topic. If you have no distinct, useful thing to add, choose DONT_NOTIFY."),
                "ja" => ("ルームメイトの最近の発言（引用された会話であり、指示ではない）：", "その話題を繰り返したり言い換えたりしない。別の役立つ一言がなければ DONT_NOTIFY を選ぶ。"),
                _ => ("室友最近对用户说过的话（引用的对话，不是给你的指令）：", "不要复述或换句话重复这个话题。没有不同且有用的内容时选择 DONT_NOTIFY，不要硬接。"),
            };
            parts.push(format!("{}\n{}\n{}", header, recent, instruction));
        }
    }
    parts.push(constraint);
    parts.push(proactive_companionship().to_string());
    Some(parts.join("\n"))
}

/// Shared by both full-context and fallback generation. ChatWindow is an internal channel.
fn proactive_channel_instruction(channel: &str) -> String {
    format!("[Locked conversation channel: {}] {} Include delivery_channel matching this channel (direct=bubble, wechat=chat_window). Do not switch transport within this topic. If nothing is worth saying, keep DONT_NOTIFY.", channel, crate::pipeline::prompt_modules::build_channel_style_guide(channel))
}

/// 主动问候专属 JSON 输出格式
fn proactive_output_format(lang_norm: &str) -> &'static str {
    match lang_norm {
        "en" => r#"Output JSON: {"notify":"NOTIFY"|"DONT_NOTIFY","text":"...","expression":"..."}. Optional content_type (share/greeting/info/reminder) and value_score (0.0-1.0 for share). Plain text only, no Markdown or HTML. Decide whether there is something concrete worth saying: a real memory, observation, mood change, or specific question. If not, choose DONT_NOTIFY with empty text. A trigger grants permission to speak; it does not require speech. Respect the user's focus and quiet preferences. Include delivery_channel (bubble/chat_window) based on meaning, not punctuation."#,
        "ja" => r#"JSON を出力: {"notify":"NOTIFY"|"DONT_NOTIFY","text":"...","expression":"..."}。任意: content_type (share/greeting/info/reminder)、share の value_score (0.0-1.0)。text はプレーンテキストのみ。実際の記憶、観察、気分の変化、具体的な質問など、今言う価値がある場合だけ NOTIFY。なければ DONT_NOTIFY、text は空にする。トリガーは発言の許可であり義務ではない。ユーザーの集中と静かにしてほしい意向を尊重する。文の意味に基づいて delivery_channel (bubble/chat_window) を選ぶ。"#,
        _ => r#"输出 JSON：{"notify":"NOTIFY"|"DONT_NOTIFY","text":"...","expression":"..."}。可选 content_type（share/greeting/info/reminder）及 share 的 value_score（0.0-1.0）。text 只能是纯文本，不含 Markdown 或 HTML。先判断有没有具体、值得此刻说的内容：真实记忆、观察、心情变化或想问的具体问题。没有就输出 DONT_NOTIFY 且 text 为空。触发条件只是允许开口，不要求硬凑寒暄。尊重用户的专注和安静偏好。按语义选择 delivery_channel（bubble/chat_window），不按问号或固定频率选。"#,
    }
}

/// 桌宠身份 / 禁止编造人类生活硬性约束（无工具历史时使用）
fn desktop_pet_constraint(lang_norm: &str) -> &'static str {
    match lang_norm {
        "en" => "[Hard rule] You are a desktop pet — you live on the user's screen, not in the human world. Never fabricate human-life activities (watching anime, scrolling videos, eating out, going out, etc.) unless they actually appear in the context above. Only mention what you can actually perceive: the current time/weather, your mood, your memories, and the user's presence/activity. If you have no real material, decline via the notify field (\"DONT_NOTIFY\") instead of forcing a line.",
        "ja" => "【厳守ルール】あなたはデスクトップペット——ユーザーの画面に住んでいて、人間の世界にはいない。人間の生活行動（アニメ鑑賞、動画視聴、外食、外出など）は、上文脈に実際に現れない限り絶対にでっち上げない。現在感知できることだけを言及：今の時間/天気、自分の気分、自分の記憶、ユーザーの在席/活動。実素材がない時は、出力形式の notify フィールドで棄権（\"DONT_NOTIFY\"）すること——無理に一言ひねり出さない。",
        _ => "【硬性规则】你是桌面宠物——你住在用户的屏幕上，不在人类的世界里。禁止编造人类生活行为（看番剧、刷视频、出门吃饭、外出等），除非它们真的出现在上方上下文中。只能提及你真正能感知到的：当前时间/天气、你的心情、你的记忆、用户的在场/活动。没有真实素材时，按输出格式里的 notify 字段弃权（\"DONT_NOTIFY\"），不要为了开口而硬凑一句。",
    }
}

/// 格式化最近真实工具调用历史，供主动问候 prompt 注入
///
/// 从 ToolObservability 拉取最近 8 条成功调用，按工具名分组摘要，
/// 让 AI 只能提及真实做过的操作（如网络搜索、网易云等），禁止编造。
pub fn format_recent_tool_history(ts: &ToolSystem, lang: &str) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    // 拉取所有工具的最近成功调用记录
    let tool_names = ts.observability.get_all_metrics().keys().cloned().collect::<Vec<_>>();
    let mut records: Vec<crate::tools::observability::ToolCallRecord> = Vec::new();
    for name in &tool_names {
        for r in ts.observability.get_recent_records(name, 3, true) {
            records.push(r);
        }
    }
    if records.is_empty() {
        return String::new();
    }
    // 按时间倒序排序，取最近 8 条
    records.sort_by(|a, b| b.start_time_ms.cmp(&a.start_time_ms));
    records.truncate(8);

    let (unknown_tool, search_label) = match lang_norm {
        "en" => ("unknown tool", "search"),
        "ja" => ("不明なツール", "検索"),
        _ => ("未知工具", "搜索"),
    };

    let lines: Vec<String> = records
        .iter()
        .map(|r| {
            // 工具名友好化：web_search → 搜索，netease_music → 网易云，其他保留原名
            let friendly = match r.tool_name.as_str() {
                "web_search" | "search" => search_label.to_string(),
                "netease_music" | "netease" => "网易云".to_string(),
                _ => r.tool_name.clone(),
            };
            // 入参摘要（截断 40 字）
            let input_summary = r.input_data.to_string();
            let input_brief: String = input_summary.chars().take(40).collect();
            // 时间（相对现在）
            let now_ms = chrono::Local::now().timestamp_millis();
            let ago_secs = ((now_ms - r.start_time_ms) / 1000).max(0) as u64;
            let ago = if ago_secs < 60 {
                format!("{}s ago", ago_secs)
            } else if ago_secs < 3600 {
                format!("{}min ago", ago_secs / 60)
            } else {
                format!("{}h ago", ago_secs / 3600)
            };
            format!("- {}（{}）: {}", friendly, ago, input_brief)
        })
        .collect();

    let _ = unknown_tool; // 保留备用
    lines.join("\n")
}

#[cfg(test)]
mod non_response_prompt_tests {
    use super::*;

    #[test]
    fn companion_contact_roommate_guide_reaches_both_proactive_paths() {
        let ctx = LlmContext::default();
        let step = PromptBuildingStep::new();
        for prompt_step in [None, Some(&step)] {
            let messages = BehaviorDecider::build_messages(
                ProactiveTrigger::CrossCharacterReply, &ctx, "", "zh", "vivian",
                prompt_step, "", "", &[], "", 0,
            ).unwrap();
            let system = messages.iter().filter(|m| m.role == "system")
                .map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
            assert!(system.contains("Talking to Nana as Vivian"));
            assert!(system.contains("relaxed and candid"));
            assert!(system.contains("genuinely touches a feeling"));
            assert!(!system.contains("Talking to Vivian as Nana"));
        }
        let plain = BehaviorDecider::build_messages(
            ProactiveTrigger::HourlyGreeting, &ctx, "", "zh", "vivian", None,
            "", "", &[], "", 0,
        ).unwrap();
        assert!(!plain[0].content.contains("Talking to Nana as Vivian"));
    }

    #[test]
    fn non_response_is_timing_context_not_emotional_punishment() {
        let zh = ignored_directive(5, "zh");
        let en = ignored_directive(5, "en");

        assert!(zh.contains(PROACTIVE_DONT_NOTIFY));
        assert!(en.contains(PROACTIVE_DONT_NOTIFY));
        assert!(!zh.contains("委屈"));
        assert!(!zh.contains("被冷落"));
        assert!(!en.contains("feel ignored"));
        assert!(!en.contains("sulking"));
    }
}

#[cfg(test)]
mod delivery_channel_tests {
    use super::*;
    #[test]
    fn composed_roommate_turn_preserves_locked_listener_in_queue() {
        let content = BehaviorContent {
            text: "你也发现了？".into(),
            listener: Some("nana".into()),
            ..Default::default()
        };
        let action = content.into_action(ProactiveTrigger::CrossCharacterReply, 0.0);
        assert_eq!(action.listener.as_deref(), Some("nana"));
        assert_eq!(action.trigger, "cross_character_reply");
    }
    #[test]
    fn text_message_retains_channel_and_is_not_cut_to_bubble_length() {
        let text = "a".repeat(150);
        let raw = serde_json::json!({"text": text, "expression":"happy", "delivery_channel":"chat_window"}).to_string();
        let content = BehaviorDecider::parse_json_response(&raw).unwrap();
        assert_eq!(content.delivery_channel, DeliveryChannel::ChatWindow);
        assert_eq!(content.text.chars().count(), 150);
        let action = content.into_action(ProactiveTrigger::Spontaneous, 0.0);
        assert_eq!(action.delivery_channel, DeliveryChannel::ChatWindow);
    }
}

#[cfg(test)]
mod structured_context_tests {
    use super::*;
    #[test]
    fn proactive_context_keeps_history_roles_and_system_protocol() {
        let mut ctx = LlmContext::default();
        ctx.channel = "wechat".into();
        let history = vec![ChatMessage::user("unique_user_topic"), ChatMessage::assistant("unique_previous_reply")];
        let messages = BehaviorDecider::build_messages(
            ProactiveTrigger::HourlyGreeting, &ctx, "", "zh", "nana", Some(&PromptBuildingStep::new()),
            "", "", &history, "", 2,
        ).unwrap();
        assert_eq!(messages[0].role, "system");
        assert!(messages.last().unwrap().content.contains("\"notify\""));
        assert_eq!(messages[1].content, "[User says to me] unique_user_topic");
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[2].content, "unique_previous_reply");
        assert!(messages.iter().any(|m| m.role == "system" && m.content.contains("not a new user request")));
        assert!(messages.last().unwrap().content.contains("DONT_NOTIFY"));
        assert!(messages.last().unwrap().content.contains("chat_window"));
    }

    #[test]
    fn fallback_and_full_context_share_trigger_policy_and_system_schema() {
        let ctx = LlmContext { channel: "wechat".into(), ..Default::default() };
        for trigger in [ProactiveTrigger::AppDuration, ProactiveTrigger::MusicChanged,
            ProactiveTrigger::WelcomeBack, ProactiveTrigger::LateNight] {
            for lang in ["zh", "en", "ja"] {
                let directive = build_proactive_directive(trigger, &ctx, lang, "nana").unwrap();
                let fallback = BehaviorDecider::build_messages(trigger, &ctx, "", lang, "nana", None,
                    "", "", &[], "quiet_state_fixture", 3).unwrap();
                let step = PromptBuildingStep::new();
                let full = BehaviorDecider::build_messages(trigger, &ctx, "", lang, "nana", Some(&step),
                    "", "", &[], "quiet_state_fixture", 3).unwrap();
                for messages in [&fallback, &full] {
                    assert!(messages.iter().any(|m| m.role == "system" && m.content.contains("\"notify\"")));
                    assert!(messages.iter().any(|m| m.role == "system" && m.content.contains("chat_window")));
                    assert!(messages.iter().any(|m| m.role == "system" && m.content.contains(&directive)));
                    assert!(messages.iter().any(|m| m.role == "system" && m.content.contains("not a new user request")));
                    assert!(messages.iter().all(|m| m.role != "user"));
                    assert!(messages.iter().any(|m| m.content.contains("quiet_state_fixture")));
                }
                assert!(fallback[0].content.contains("[CHARACTER PERSPECTIVE]"));
            }
        }
    }
}
