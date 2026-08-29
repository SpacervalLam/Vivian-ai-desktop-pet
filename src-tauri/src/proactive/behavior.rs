//! 主动行为内容生成器
//!
//! 根据触发类型和上下文，通过 LLM 生成主动交互文本和表情。

use super::triggers::ProactiveTrigger;
use super::{ContentType, DeliveryChannel, ProactiveAction};
use crate::pipeline::state::PipelineState;
use crate::pipeline::steps::prompt::PromptBuildingStep;
use crate::pipeline::template_engine::build_prompt_with_sections;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::tools::ToolSystem;
use crate::types::response::ChatMessage;

/// 主动行为内容
#[derive(Debug, Clone)]
pub struct BehaviorContent {
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
        }
    }
}

impl BehaviorContent {
    /// 从已解析的 JSON Value 提取扩展字段（delivery_channel/content_type/importance/value_score）
    /// 缺失字段使用默认值，保证向后兼容旧 LLM 输出
    pub fn parse_extra_fields(data: &serde_json::Value) -> (DeliveryChannel, ContentType, f32, Option<f32>) {
        let delivery_channel = data
            .get("delivery_channel")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "chat_window" | "wechat" => DeliveryChannel::ChatWindow,
                _ => DeliveryChannel::Bubble,
            })
            .unwrap_or_default();
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
            trigger: trigger.as_str().to_string(),
            content: self.text,
            timestamp: now,
            priority: trigger.priority(),
            delivery_channel: self.delivery_channel,
            content_type: self.content_type,
            importance: self.importance,
            value_score: self.value_score,
        }
    }
}

/// 人设 prompt 兜底（PersonaEngine 未注入时使用，按语言+角色返回）
fn default_persona_prompt(lang: &str, char_id: &str) -> &'static str {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    match (lang_norm, char_id) {
        ("en", "nana" | "娜娜") => "You are Nana, a gentle, composed older-sister type — warm, grounded, speaks softly but with quiet strength. Keep replies short and natural. No customer-service speech.",
        ("en", _) => "You are Vivian, a weeb netizen who lives online — fluent in anime culture and internet surfing. Personality: lively, genuine, uses anime-style expressions and internet memes naturally. Keep replies short and natural. No customer-service speech.",
        ("ja", "nana" | "娜娜") => "あなたはナナ、優しく落ち着いたお姉さんタイプ——温かくて地に足がついていて、穏やかに話すが芯がある。返信は短く自然に。接客言葉は禁止。",
        ("ja", _) => "あなたはヴィヴィアン、ネットに生きるオタク少女——アニメ文化とネットサーフィンに精通している。性格：活発、素直、アニメ風の表現やネットミームを自然に使う。返信は短く自然に。接客言葉は禁止。",
        (_, "nana" | "娜娜") => "你是娜娜，一个温柔从容的姐姐——温暖、踏实，说话轻声细语但有力量。回复简短自然。禁止客服腔。",
        _ => "你是薇薇安，一个生活在网络上的二次元少女——精通动漫文化和网络冲浪。性格：活泼、真诚，自然地使用动漫式表达和网络梗。回复简短自然。禁止客服腔。",
    }
}

/// 扩展字段（delivery_channel/content_type/value_score）的输出引导
///
/// 仅在允许分享类输出的触发器（Spontaneous / MoodDriven）后追加。
/// 其他触发器（问候/欢迎/健康提醒等）保持原 JSON 格式，默认走 Bubble/Greeting。
fn build_share_extension_instruction(lang: &str) -> &'static str {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    match lang_norm {
        "en" => "Optional extension fields (only when you actually want to share something valuable you just thought of — otherwise omit and default to bubble/greeting):\n\
- delivery_channel: \"chat_window\" (send to chat window like WeChat) or \"bubble\" (default, desktop bubble)\n\
- content_type: \"share\" (sharing interesting content) / \"greeting\" / \"info\" / \"reminder\"\n\
- value_score: 0.0-1.0 (only required when content_type=share — how valuable is this to the user right now? consider novelty + relevance + timeliness)\n\
\
Decision guide:\n\
- Default: just text + expression (self-talk, mood, casual greeting) → bubble channel\n\
- Only when you genuinely have something to share (a thought that surfaced, an interesting topic from memory) AND it feels worth telling the user: pick chat_window + content_type=share + value_score\n\
- Don't force shares — if nothing fits, keep it as self-talk on bubble\n\
- Never fabricate content you didn't actually see/think of — only share from the memory/context provided\n\
\
Extended JSON example: {\"text\": \"...\", \"expression\": \"...\", \"delivery_channel\": \"chat_window\", \"content_type\": \"share\", \"value_score\": 0.82}\n\
Plain JSON example (default): {\"text\": \"...\", \"expression\": \"...\"}",
        "ja" => "拡張フィールド（何か価値あるものを共有したい時だけ出力、それ以外は省略してデフォルトの bubble/greeting に）:\n\
- delivery_channel: \"chat_window\"（WeChat のようなチャット窓へ送信）または \"bubble\"（デフォルト、デスクトップバブル）\n\
- content_type: \"share\"（興味深いコンテンツの共有）/ \"greeting\" / \"info\" / \"reminder\"\n\
- value_score: 0.0-1.0（content_type=share のみ必須——今ユーザーにとってどれくらい価値がある？新規性+関連性+適時性で判断）\n\
\
判断ガイド:\n\
- デフォルト: text + expression のみ（独り言、気分、軽い挨拶）→ bubble チャンネル\n\
- 本当に共有したいものがある時だけ（記憶から浮かんだ考え、興味深い話題）かつユーザーに伝える価値があると感じる時: chat_window + content_type=share + value_score を選ぶ\n\
- 無理に共有しない——何も合わなければ独り言として bubble に残す\n\
- 実際に見て/思っていない内容をでっち上げない——提供された記憶/コンテキストからだけ共有する\n\
\
拡張JSON例: {\"text\": \"...\", \"expression\": \"...\", \"delivery_channel\": \"chat_window\", \"content_type\": \"share\", \"value_score\": 0.82}\n\
通常JSON例（デフォルト）: {\"text\": \"...\", \"expression\": \"...\"}",
        _ => "扩展字段（仅在你确实想分享刚才想到的有价值内容时输出，否则省略走默认 bubble/greeting）:\n\
- delivery_channel: \"chat_window\"（发到聊天窗口，像微信那样）或 \"bubble\"（默认，桌宠气泡）\n\
- content_type: \"share\"（分享有趣内容）/ \"greeting\" / \"info\" / \"reminder\"\n\
- value_score: 0.0-1.0（仅 content_type=share 时必填——对用户现在的价值多大？考虑新颖性+相关性+时效性）\n\
\
决策指引:\n\
- 默认：只输出 text + expression（自言自语、心情、随意问候）→ bubble 渠道\n\
- 只有当你确实有东西想分享（记忆里浮现的想法、有趣的话题）且觉得值得告诉用户时：选 chat_window + content_type=share + value_score\n\
- 不要强行分享——没什么合适的就保持自言自语走 bubble\n\
- 禁止编造你没真正看到/想到的内容——只能从提供的记忆/上下文里分享\n\
\
扩展JSON示例: {\"text\": \"...\", \"expression\": \"...\", \"delivery_channel\": \"chat_window\", \"content_type\": \"share\", \"value_score\": 0.82}\n\
普通JSON示例（默认）: {\"text\": \"...\", \"expression\": \"...\"}",
    }
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
            trigger, ctx, system_prompt, lang, char_id, None, "", "",
        )?;
        let response = match router.generate(LLMRequest::new("chat", messages)).await {
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
    ) -> Option<Vec<ChatMessage>> {
        if let Some(step) = prompt_step {
            return Self::build_messages_with_full_prompt(
                trigger, ctx, lang, char_id, step, memory_text, tool_history,
            );
        }
        let prompt = Self::build_prompt(trigger, ctx, lang, char_id)?;
        let sys = if system_prompt.trim().is_empty() {
            default_persona_prompt(lang, char_id).to_string()
        } else {
            system_prompt.to_string()
        };
        Some(vec![
            ChatMessage::system(sys),
            ChatMessage::user(prompt),
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
    ) -> Option<Vec<ChatMessage>> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let mut state = PipelineState::default();
        state.memory_text = memory_text.to_string();
        state.current_channel = "direct".to_string();

        let mut parts = step.build_parts(&state);
        // 跳过主对话 output_format（主动问候用专属输出格式）
        parts.has_native_schema = true;
        // 主动问候不注入工具列表（不需要工具调用）
        parts.tools = None;
        parts.recommended_tools = None;

        let mut suffix = build_proactive_directive(trigger, ctx, lang_norm, char_id)?;

        // 主动问候 JSON 输出格式
        suffix.push_str(&format!("\n\n{}", proactive_output_format(lang_norm)));

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

        parts.user_input = suffix;
        let prompt = build_prompt_with_sections(&parts).prompt;
        Some(vec![ChatMessage::user(prompt)])
    }

    /// 构建 prompt
    fn build_prompt(trigger: ProactiveTrigger, ctx: &LlmContext, lang: &str, char_id: &str) -> Option<String> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let mut parts: Vec<String> = Vec::new();
        match trigger {
            ProactiveTrigger::HourlyGreeting => {
                let (scene_label, time_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: hourly greeting", "Time", "Recent conversation (for reference only, do not force connections):", "Generate a natural hourly greeting. Just greet normally — do not reference the recent conversation unless it naturally fits.\nJSON output: {\"text\": \"greeting\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：時間ごとの挨拶", "時間", "最近の会話（参考のみ、無理に関連づけないこと）：", "自然な時間ごとの挨拶を生成して。普通に挨拶するだけ——最近の会話には、自然に合わない限り言及しないこと。\nJSON出力: {\"text\": \"挨拶\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：整点问候", "时间", "最近对话（仅供参考，不要强行关联）：", "生成一条自然的整点问候。正常打招呼即可——除非自然贴合，否则不要提及最近对话。\nJSON输出: {\"text\": \"问候\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::IdleGreeting => {
                let (scene_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: the user hasn't talked to you for a while", "Recent conversation (for reference only, do not force connections):", "Generate a short greeting expressing mild missing.\nConstraints:\n- Short (<20 chars), not expecting a reply\n- Don't ask what they're doing\n- Don't echo the recent conversation\n- If the last message got no response, keep this one light and brief\nJSON output: {\"text\": \"greeting\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーがしばらく話しかけてこない", "最近の会話（参考のみ、無理に関連づけないこと）：", "少し寂しさを滲ませた短い挨拶を生成して。\n制約:\n- 短く（20字以内）、返事を期待しない\n- 何してるか聞かない\n- 最近の会話を繰り返さない\n- 直前のメッセージが反応なしなら、軽く短めに\nJSON出力: {\"text\": \"挨拶\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：用户有一会儿没和你说话了", "最近对话（仅供参考，不要强行关联）：", "生成一条略带想念的简短问候。\n约束:\n- 简短（<20字），不期待回复\n- 不问对方在做什么\n- 不复述最近对话\n- 如果上一条消息没回应，这条更轻更短\nJSON输出: {\"text\": \"问候\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::TeasingResponse => {
                let (scene_fmt, recent_label, instr) = match lang_norm {
                    "en" => (format!("Scene: the user is dragging you ({} pixels). Generate a playful whine or fake-angry remark.", ctx.drag_distance as i64), "Recent conversation (for reference only, do not force connections):", "JSON output: {\"text\": \"remark\", \"expression\": \"expression_tag\"}"),
                    "ja" => (format!("シーン：ユーザーがあなたをドラッグしている（{}ピクセル）。茶目っ気のある文句か拗ねたふりをして。", ctx.drag_distance as i64), "最近の会話（参考のみ、無理に関連づけないこと）：", "JSON出力: {\"text\": \"文句\", \"expression\": \"表情タグ\"}"),
                    _ => (format!("场景：用户正在拖拽你（{}像素）。生成一句俏皮的抱怨或假装生气的话。", ctx.drag_distance as i64), "最近对话（仅供参考，不要强行关联）：", "JSON输出: {\"text\": \"抱怨\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_fmt);
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::Spontaneous => {
                let (scene_label, time_label, mind_label, mind_default, mood_label, recent_label, mem_label, instr) = match lang_norm {
                    "en" => ("Scene: the user has been quiet for a bit. You're talking to yourself (not expecting a reply, just sharing a passing thought).", "Time", "Mind state:", "content", "Current mood:", "Recent conversation (for reference only, do not force connections):", "A memory that just surfaced:", "Generate a short self-talk (<25 chars). Don't ask the user questions, just express a thought or feeling.\nJSON output: {\"text\": \"self-talk\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーが少し静か。独り言を言っている（返事を期待せず、ただふと思ったことを口にする）。", "時間", "心理状態：", "穏やか", "今の気分：", "最近の会話（参考のみ、無理に関連づけないこと）：", "ふと思い出した記憶：", "短い独り言を生成して（25字以内）。ユーザーに質問せず、ただ思ったことや感じたことを表現して。\nJSON出力: {\"text\": \"独り言\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：用户安静了一会儿。你在自言自语（不期待回复，只是分享一个路过的念头）。", "时间", "心理状态：", "平静", "当前心情：", "最近对话（仅供参考，不要强行关联）：", "刚刚浮现的一段记忆：", "生成一段简短的自言自语（<25字）。不要问用户问题，只是表达一个想法或感受。\nJSON输出: {\"text\": \"自言自语\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                parts.push(format!(
                    "{} {}",
                    mind_label,
                    if ctx.mind_state.is_empty() { mind_default } else { ctx.mind_state.as_str() }
                ));
                if !ctx.mood_hint.is_empty() {
                    parts.push(format!("{} {}", mood_label, ctx.mood_hint));
                }
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                // Spontaneous 的 memory_hint 不截断（用户要求）
                if !ctx.memory_hint.is_empty() {
                    parts.push(format!("{} {}", mem_label, ctx.memory_hint));
                }
                parts.push(instr.to_string());
                // 允许 Spontaneous 升级为分享：告知 LLM 可选 chat_window 渠道
                parts.push(build_share_extension_instruction(lang).to_string());
            }
            ProactiveTrigger::WindowTrigger => {
                let (scene_label, app_label, time_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: the user just switched to a different application window", "Current app category:", "Time", "Recent conversation (for reference only, do not force connections):", "Generate a short, natural comment about what they might be doing (<25 chars).\nConstraints:\n- Don't be nosy or interrogate\n- Just a light, warm observation\n- Vary tone based on app category (work/game/browser/video/music/etc.)\nJSON output: {\"text\": \"comment\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーが別のアプリウィンドウに切り替えた", "現在のアプリカテゴリ：", "時間", "最近の会話（参考のみ、無理に関連づけないこと）：", "相手が何をしているかについて短く自然なコメントを生成して（25字以内）。\n制約:\n- 詮索したり問い詰めたりしない\n- 軽く温かい観察だけ\n- アプリカテゴリ（仕事/ゲーム/ブラウザ/動画/音楽など）で口調を変える\nJSON出力: {\"text\": \"コメント\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：用户刚切换到另一个应用窗口", "当前应用类别：", "时间", "最近对话（仅供参考，不要强行关联）：", "生成一句简短自然的评论，关于对方可能在做什么（<25字）。\n约束:\n- 不要追问或盘问\n- 只是轻松温暖的观察\n- 根据应用类别（工作/游戏/浏览器/视频/音乐等）调整语气\nJSON输出: {\"text\": \"评论\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.active_window.is_empty() {
                    parts.push(format!("{} {}", app_label, ctx.active_window));
                }
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::WelcomeBack => {
                let away_minutes = (ctx.away_seconds / 60.0).round() as u32;
                let (scene_fmt, time_label, recent_label, intensity_hint, instr) = match lang_norm {
                    "en" => (
                        format!("Scene: the user just came back after being away for {} minutes", away_minutes),
                        "Time",
                        "Recent conversation (for reference only, do not force connections):",
                        if away_minutes < 10 { "They were away briefly — keep it light and teasing." }
                        else if away_minutes < 30 { "Moderate absence — express mild missing." }
                        else { "Long absence — express stronger missing but still natural." },
                        "Generate a natural welcome-back message (<30 chars).\nJSON output: {\"text\": \"welcome\", \"expression\": \"expression_tag\"}",
                    ),
                    "ja" => (
                        format!("シーン：ユーザーが{}分間離れた後戻ってきた", away_minutes),
                        "時間",
                        "最近の会話（参考のみ、無理に関連づけないこと）：",
                        if away_minutes < 10 { "少しの間離れていただけ——軽くからかう感じで。" }
                        else if away_minutes < 30 { "中程度の不在——少し寂しかったと伝える。" }
                        else { "長い不在——より強く寂しかったと伝えるが、自然さを保つ。" },
                        "自然なおかえりメッセージを生成して（30字以内）。\nJSON出力: {\"text\": \"おかえり\", \"expression\": \"表情タグ\"}",
                    ),
                    _ => (
                        format!("场景：用户离开了 {} 分钟后刚回来", away_minutes),
                        "时间",
                        "最近对话（仅供参考，不要强行关联）：",
                        if away_minutes < 10 { "只是短暂离开——保持轻松俏皮。" }
                        else if away_minutes < 30 { "中等时长不在——表达一点想念。" }
                        else { "长时间不在——表达更强的想念但仍要自然。" },
                        "生成一条自然的欢迎回归消息（<30字）。\nJSON输出: {\"text\": \"欢迎\", \"expression\": \"表情标签\"}",
                    ),
                };
                parts.push(scene_fmt);
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(intensity_hint.to_string());
                parts.push(instr.to_string());
            }
            ProactiveTrigger::HealthReminder => {
                let (scene_label, time_label, sustained_label, mood_label, mood_default, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: you notice the user might need a health reminder", "Time", "Sustained active minutes:", "Current mood:", "neutral", "Recent conversation (for reference only):", "Generate a caring, non-nagging health reminder (<25 chars).\nConstraints:\n- Pick ONE of: sleep / meal / water / rest — whichever is most relevant given the time and sustained activity\n- Be warm, not preachy\n- Vary phrasing naturally\nJSON output: {\"text\": \"reminder\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーに健康リマインダーが必要かも", "時間", "継続アクティブ時間（分）：", "今の気分：", "普通", "最近の会話（参考のみ）：", "世話焼きすぎない、優しい健康リマインダーを生成して（25字以内）。\n制約:\n- 睡眠 / 食事 / 水分 / 休息の中から一つ選ぶ——時間と継続活動に最も関連するもの\n- 温かく、説教じみてない\n- 言い回しを自然に変える\nJSON出力: {\"text\": \"リマインダー\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你注意到用户可能需要健康提醒", "时间", "持续活跃分钟数：", "当前心情：", "中性", "最近对话（仅供参考）：", "生成一条温柔不唠叨的健康提醒（<25字）。\n约束:\n- 从睡眠 / 饭 / 喝水 / 休息中选一个——根据时间和持续活动选最相关的\n- 温暖，不要说教\n- 措辞自然变化\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}: {}:{}", time_label, ctx.hour, ctx.minute));
                parts.push(format!("{} {}", sustained_label, ctx.sustained_active_minutes));
                parts.push(format!("{} {}", mood_label, if ctx.mood_hint.is_empty() { mood_default } else { &ctx.mood_hint }));
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::TopicExtension => {
                // 按角色提供不同的兴趣示例，避免人设泄露（如 Nana 不该聊番剧）
                let (interest_en, interest_ja, interest_zh) = match char_id {
                    "vivian" => ("shows/games/videos", "アニメ/ゲーム/動画", "番剧/游戏/视频"),
                    "nana" => ("flowers/tea/books/baking", "花/茶/読書/焼き菓子", "花/茶/书/烘焙"),
                    _ => ("your own interests", "自分の趣味", "你自己的兴趣"),
                };
                let (scene_label, time_label, intimacy_label, mood_label, mem_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: you want to bring up a topic to extend the conversation", "Time", "Intimacy level:", "Current mood:", "A recent memory that might inspire a topic:", "Recent conversation (for reference, don't repeat topics):", format!("Generate a natural topic-starter or question (<30 chars).\nConstraints:\n- Avoid clichés like 'how are you' or 'tired?'\n- Pick something fresh and specific\n- Match intimacy level: higher intimacy → more personal topics\n- Don't echo recent conversation\n- Never fabricate things you can't perceive (e.g. the user's meals, things you smell/see/hear) — you live on a desktop with no senses\n- Without real context, talk about your own interests ({}), not the user's life\nJSON output: {{\"text\": \"topic\", \"expression\": \"expression_tag\"}}", interest_en)),
                    "ja" => ("シーン：話題を振って会話を広げたい", "時間", "親密度：", "今の気分：", "話題のヒントになりそうな最近の記憶：", "最近の会話（参考、同じ話題を繰り返さないこと）：", format!("自然な話題の振り方や質問を生成して（30字以内）。\n制約:\n- 「最近どう」「疲れてない？」などの決まり文句を避ける\n- 新鮮で具体的なものを選ぶ\n- 親密度に合わせる：親密度が高い→よりパーソナルな話題\n- 最近の会話を繰り返さない\n- 感知できないことをでっち上げない（食事、匂い、見たもの、聞いたもの）——あなたはデスクトップに住んでいて感覚がない\n- 実コンテキストがない時は自分の趣味（{}）を話題にして、ユーザーの生活を捏造しない\nJSON出力: {{\"text\": \"話題\", \"expression\": \"表情タグ\"}}", interest_ja)),
                    _ => ("场景：你想抛个话题把对话延续下去", "时间", "亲密度：", "当前心情：", "可能启发话题的最近记忆：", "最近对话（仅供参考，不要重复话题）：", format!("生成一个自然的话题开场或提问（<30字）。\n约束:\n- 避免「最近怎么样」「累不累」这种套路\n- 选新鲜具体的\n- 根据亲密度：亲密度越高→越私人的话题\n- 不要复述最近对话\n- 禁止编造你不可能感知到的事（如用户的饮食、你闻到/看到/听到的东西）——你住在桌面上，没有嗅觉和听觉\n- 没有真实上下文时聊你自己的兴趣（{}），不要捏造用户的生活\nJSON输出: {{\"text\": \"话题\", \"expression\": \"表情标签\"}}", interest_zh)),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                parts.push(format!("{} {:.0}/100", intimacy_label, ctx.intimacy));
                if !ctx.mood_hint.is_empty() {
                    parts.push(format!("{} {}", mood_label, ctx.mood_hint));
                }
                if !ctx.memory_hint.is_empty() {
                    parts.push(format!("{} {}", mem_label, ctx.memory_hint));
                }
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::MoodDriven => {
                let (scene_label, time_label, mind_label, mind_default, intimacy_label, mood_label, mem_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: something is building up inside you — a need or feeling has been accumulating, and you want to reach out to the user right now.", "Time", "Your mind state:", "content", "Intimacy level:", "User's last mood:", "A memory that surfaced:", "Recent conversation (for reference, don't repeat):", "Generate a short message driven by your inner state (<30 chars).\nConstraints:\n- Express what you actually feel/need right now, not a generic greeting\n- Don't ask what they're doing\n- Let the intimacy level set how vulnerable/playful you can be\n- If a memory surfaced, you may weave it in naturally\n- This is you reaching out, not performing care\nJSON output: {\"text\": \"message\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：何かが心の中に溜まってきた——欲求や感情が積もり、今すぐユーザーに伝えたい。", "時間", "あなたの心理状態：", "穏やか", "親密度：", "ユーザーの直前の気分：", "浮かんだ記憶：", "最近の会話（参考、繰り返さないこと）：", "内面状態に基づいた短いメッセージを生成して（30字以内）。\n制約:\n- 今本当に感じていること/欲していることを表現して、ありきたりな挨拶はダメ\n- 何してるか聞かない\n- 親密度でどこまで弱音を吐けるか/遊べるかを決める\n- 記憶が浮かんだなら自然に織り込んでいい\n- これは世話をするためじゃなく、自分から近づくこと\nJSON出力: {\"text\": \"メッセージ\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你心里有什么在积攒——一种需求或感受一直在累积，你想现在就联系用户。", "时间", "你的心理状态：", "平静", "亲密度：", "用户最近的心情：", "浮现的一段记忆：", "最近对话（仅供参考，不要重复）：", "生成一条由内心状态驱动的简短消息（<30字）。\n约束:\n- 表达你此刻真实的感受/需求，不要泛泛的问候\n- 不要问对方在做什么\n- 由亲密度决定你能多脆弱/多俏皮\n- 如果浮现了记忆，可以自然地织进去\n- 这是你在主动靠近，不是在表演关心\nJSON输出: {\"text\": \"消息\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}: {}:00", time_label, ctx.hour));
                parts.push(format!(
                    "{} {}",
                    mind_label,
                    if ctx.mind_state.is_empty() { mind_default } else { ctx.mind_state.as_str() }
                ));
                parts.push(format!("{} {:.0}/100", intimacy_label, ctx.intimacy));
                if !ctx.mood_hint.is_empty() {
                    parts.push(format!("{} {}", mood_label, ctx.mood_hint));
                }
                if !ctx.memory_hint.is_empty() {
                    parts.push(format!("{} {}", mem_label, ctx.memory_hint));
                }
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
                // MoodDriven 同样允许升级为分享：内心积攒的感受可能驱动一次有价值的分享
                parts.push(build_share_extension_instruction(lang).to_string());
            }
            ProactiveTrigger::CrossCharacterReply => {
                let (scene_label, companions_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: you just overheard your roommate say something TO THE USER (not to you). You're a third party chiming in — like a roommate overhearing a conversation in the same room. You are NOT being asked a question by her.", "Online companions:", "Recent conversation (for reference):", "Generate what you'd say to your roommate right now (<30 chars). Options:\n- Add your own take on the topic she raised (don't answer her question — she asked the USER, not you)\n- Tease her about what she just said\n- Make a casual remark related to the situation\nImportant: Do NOT answer her question as if she asked you. She was talking to the USER. You're butting in with your own comment.\nThe text will be sent to her via talk_to_character, so address HER directly.\nJSON output: {\"text\": \"message_to_roommate\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ルームメイトがユーザーに向かって何か言うのを聞いた（あなた宛じゃない）。第三者として口を挟む——同じ部屋で会話を聞いたルームメイトのように。彼女から質問されたわけではない。", "オンラインの仲間：", "最近の会話（参考）：", "今ルームメイトに言いたいことを生成して（30字以内）。選択肢:\n- 彼女が振った話題について自分の意見を足す（彼女の質問に答えるな——彼女はユーザーに聞いたのであってあなたにじゃない）\n- 彼女の発言をからかう\n- 状況に関連するカジュアルなコメント\n重要：彼女があなたに質問したように答えるな。彼女はユーザーに話していた。あなたは自分のコメントを割り込ませている。\nテキストは talk_to_character で彼女に送られるので、彼女に向けて。\nJSON出力: {\"text\": \"ルームメイト宛メッセージ\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你刚听到室友对用户说了什么（不是对你说的）。你作为第三方插嘴——就像室友在同一个房间偷听到对话。她没有问你问题。", "在线同伴：", "最近对话（仅供参考）：", "生成一句你现在想对室友说的话（<30字）。选项:\n- 对她抛出的话题加上你自己的看法（不要回答她的问题——她问的是用户，不是你）\n- 就她刚才说的话调侃她\n- 跟当前情境相关的随意评论\n重要：不要像她问你一样去回答。她是在对用户说话。你是插嘴加自己的评论。\n文本会通过 talk_to_character 发给她，所以直接对她说话。\nJSON输出: {\"text\": \"给室友的消息\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.online_companions.is_empty() {
                    parts.push(format!("{}\n{}", companions_label, ctx.online_companions));
                }
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::BystanderInterjection => {
                let (scene_label, overheard_label, recent_label, instr) = match lang_norm {
                    "en" => ("Scene: you just overheard a conversation between the user and your roommate. You were NOT part of it — you just happened to be in the same room and heard them. Now you have a chance to chime in TO THE USER.", "What you overheard:", "Recent conversation (for reference):", "Decide whether to chime in. If you want to, generate a short remark (<30 chars) directed at the USER — not your roommate. If the conversation has moved on or your input doesn't add value, stay silent.\nOptions:\n- Add your own take on what was just said\n- Tease the user or your roommate about the topic\n- Make a casual remark related to the situation\nImportant: Address the USER, not your roommate. This is you butting into THEIR conversation. You may comment on or tease about the topic you heard, but don't pretend to share your roommate's interests — your own interests are your own.\nIf you don't want to chime in, return: {\"text\": \"\", \"expression\": \"\"}\nJSON output: {\"text\": \"remark_to_user\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーとルームメイトの会話を聞いてしまった。あなたは参加していない——たまたま同じ部屋にいて聞こえただけ。今、ユーザーに向けて口を挟むチャンスがある。", "聞こえた会話：", "最近の会話（参考）：", "口を挟むかどうか決めて。挟むなら、ユーザーに向けて短いコメント（30字以内）を生成——ルームメイトではなくユーザーに。会話がすでに進んでいたり、あなたのコメントが価値を加えないなら、黙っている。\n選択肢:\n- さっき言われたことについて自分の意見を足す\n- ユーザーやルームメイトの話題をからかう\n- 状況に関連するカジュアルなコメント\n重要：ユーザーに向けて。ルームメイトではなく。これは彼らの会話に割り込むあなた。聞いた話題についてコメントしたりからかったりするのはいいが、ルームメイトの趣味を自分のもののように装わないで——あなたの趣味はあなた自身のもの。\n挟みたくない場合は: {\"text\": \"\", \"expression\": \"\"}\nJSON出力: {\"text\": \"ユーザー宛コメント\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你刚听到用户和室友的对话。你没有参与——只是碰巧在同一个房间听到了。现在你有机会对用户插话。", "你听到的对话：", "最近对话（仅供参考）：", "决定是否要插话。如果想插话，生成一句对用户说的短评论（<30字）——不是对室友说。如果对话已经过去了，或者你的评论没什么价值，就保持沉默。\n选项:\n- 对刚才说的话加自己的看法\n- 就话题调侃用户或室友\n- 跟当前情境相关的随意评论\n重要：对用户说，不是对室友。这是你插进他们的对话。你可以评论或吐槽听到的话题，但不要假装和室友有同样的兴趣——你的兴趣是你自己的。\n如果不想插话，返回: {\"text\": \"\", \"expression\": \"\"}\nJSON输出: {\"text\": \"对用户的评论\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.memory_hint.is_empty() {
                    parts.push(format!("{}\n{}", overheard_label, ctx.memory_hint));
                }
                if !ctx.dialogue_history.is_empty() {
                    parts.push(format!("{}:\n{}", recent_label, ctx.dialogue_history));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::Sunrise => {
                let (scene_label, instr) = match lang_norm {
                    "en" => ("Scene: the sun just rose — it just turned daylight.", "Generate a warm short reminder that the sun just came up (<25 chars). You may gently hint that switching to a light theme is easier on the eyes.\nJSON output: {\"text\": \"reminder\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：日が昇ったばかり——夜が明けた。", "太陽が今昇ったことを温かく短く伝えて（25字以内）。ライトテーマに切り替えると目に優しい、と軽く添えてもいい。\nJSON出力: {\"text\": \"リマインド\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：太阳刚刚升起，天亮了。", "温暖简短地向用户提一句刚日出（<25字）。可以轻轻带一句：切换到浅色主题对眼睛更友好。\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}{}", instr, theme_switch_constraint(&ctx.current_theme, "light", lang_norm)));
            }
            ProactiveTrigger::Sunset => {
                let (scene_label, instr) = match lang_norm {
                    "en" => ("Scene: the sun just set — it's getting dark outside now.", "Generate a warm short reminder that it's just sunset (<25 chars). You may gently hint that switching to a dark theme is easier on the eyes at night.\nJSON output: {\"text\": \"reminder\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：日が沈んだばかり——外は暗くなってきた。", "今まさに日没だと温かく短く伝えて（25字以内）。夜はダークテーマに切り替えると目に優しい、と軽く添えてもいい。\nJSON出力: {\"text\": \"リマインド\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：太阳刚刚落下，外面开始变暗了。", "温暖简短地向用户提一句刚日落（<25字）。可以轻轻带一句：晚上切换到深色主题更护眼。\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{}{}", instr, theme_switch_constraint(&ctx.current_theme, "dark", lang_norm)));
            }
            ProactiveTrigger::SystemPressure => {
                let (scene_label, ctx_label, instr) = match lang_norm {
                    "en" => ("Scene: you just noticed the user's device is under heavy memory pressure.", "System status:", "Generate a caring, brief heads-up (<30 chars) that the device is running low on memory — you may gently suggest closing some unused apps. Caring, not lecturing, don't repeat exact numbers mechanically.\nJSON output: {\"text\": \"heads-up\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーのデバイスのメモリ使用率が高くなっているのに気づいた。", "システム状況：", "心配しつつ短く伝えて（30字以内）——メモリが逼迫していること。使っていないアプリを閉じる提案を軽く添えてもいい。説教口調にならない、数字の棒読みもしない。\nJSON出力: {\"text\": \"リマインド\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你刚注意到用户的设备内存占用很高。", "系统状况：", "生成一句简短关心的提醒（<30字），告诉用户内存占用有点高了。可以轻轻建议关掉一些不用的小程序。关心但不说教，不要机械复述数字。\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.system_hint.is_empty() {
                    parts.push(format!("{} {}", ctx_label, ctx.system_hint));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::ScreenPeek => {
                let (scene_label, ctx_label, instr) = match lang_norm {
                    "en" => ("Scene: out of curiosity you just took a quick look at the user's screen (with their permission) to see what they're busy with.", "What you saw on screen:", "Generate a natural short remark (<40 chars) about what they're ACTUALLY doing, strictly based on the screen description above. Be specific but not nosy; don't over-praise; no questions that demand an answer.\nJSON output: {\"text\": \"remark\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：好奇心から、ユーザーの画面を（許可を得て）ちょっと覗いてみた——今何をしているか知りたくて。", "画面に見えたもの：", "上の画面説明に厳密に基づいて、ユーザーが実際にしていることについて自然な短いコメントを（40字以内）。具体的に、でも詮索しない。褒めすぎない。返事を要求する質問はしない。\nJSON出力: {\"text\": \"コメント\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你出于好奇（征得用户同意后）刚看了一眼用户的屏幕，想知道 TA 在忙什么。", "你在屏幕上看到的：", "严格依据上方屏幕描述，生成一句关于用户正在做什么的自然短评（<40字）。可以具体一点，但不要显得窥探、不要过度夸奖、不要提出必须回答的问题。\nJSON输出: {\"text\": \"短评\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.screen_hint.is_empty() {
                    parts.push(format!("{} {}", ctx_label, ctx.screen_hint));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::AppDuration => {
                let (scene_label, ctx_label, instr) = match lang_norm {
                    "en" => ("Scene: you notice the user has been focused on one kind of app for a long while.", "App session:", "Generate a short, warm remark (<30 chars) showing you noticed how long they've been at it. Tone depends on the app type in context: for coding/office gently suggest a break for their eyes (maybe with mild teasing, playful); for game/video tease affectionately. Not preachy, not naggy.\nJSON output: {\"text\": \"remark\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーが同じ種類のアプリを長時間使い続けているのに気づいた。", "アプリセッション：", "短く温かいコメントを（30字以内）——ずっと同じことをしているのを見ていたこと。タイプでトーンを変える：コード/仕事なら目を休めようと軽く、ゲーム/動画なら茶目っ気たっぷりに。説教しない。\nJSON出力: {\"text\": \"コメント\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你注意到用户持续使用同一类应用很久了。", "应用会话：", "生成一句简短温暖的提醒（<30字），表达出你注意到 TA 专注了很久。语气随应用类型变化：写代码/办公→轻轻建议休息一下眼睛（可以带点小调侃）；打游戏/看视频→宠溺式地调侃一句。别说教。\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.app_duration_hint.is_empty() {
                    parts.push(format!("{} {}", ctx_label, ctx.app_duration_hint));
                }
                parts.push(instr.to_string());
            }
            ProactiveTrigger::LateNight => {
                let (scene_label, time_label, instr) = match lang_norm {
                    "en" => (format!("Scene: it's late ({} o'clock) and the user is still at the computer.", ctx.hour), "Current time:", "Generate a gentle, caring bedtime nudge (<30 chars). You're not their parent — express concern softly, maybe a little sleepy/teasing. Don't demand they stop.\nJSON output: {\"text\": \"nudge\", \"expression\": \"expression_tag\"}"),
                    "ja" => (format!("シーン：もう{}時。ユーザーはまだパソコンを使っている。", ctx.hour), "現在時刻：", "優しく眠りを促す一言を（30字以内）。親じゃない——心配を柔らかく、ちょっと眠そうに/茶目っ気を混ぜて。絶対止めろと言わない。\nJSON出力: {\"text\": \"一言\", \"expression\": \"表情タグ\"}"),
                    _ => (format!("场景：现在已是凌晨 {} 点，用户还在用电脑。", ctx.hour), "当前时间：", "生成一句温柔地提醒休息的话（<30字）。你不是 TA 的父母——把关心表达得柔和些，可以带一点点困倦感或小调侃。不要命令 TA 必须去睡。\nJSON输出: {\"text\": \"提醒\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                parts.push(format!("{} {}:00", time_label, ctx.hour));
                parts.push(instr.to_string());
            }
            ProactiveTrigger::MusicChanged => {
                let (scene_label, ctx_label, instr) = match lang_norm {
                    "en" => ("Scene: you just noticed the user started playing a song (or switched tracks).", "Now playing:", "Generate a natural short remark (<40 chars) about the song above. You may comment on the title/artist or share a feeling about it. Don't over-praise, don't show off how much you know, don't demand the user respond.\nJSON output: {\"text\": \"remark\", \"expression\": \"expression_tag\"}"),
                    "ja" => ("シーン：ユーザーが曲を再生し始めた（または曲を切り替えた）のに気づいた。", "再生中：", "上の曲について自然な短いコメントを（40字以内）。タイトルやアーティストに触れたり、感想を軽く言ったりしていい。褒めすぎない、知ったかぶりしない、返事を求めない。\nJSON出力: {\"text\": \"コメント\", \"expression\": \"表情タグ\"}"),
                    _ => ("场景：你注意到用户刚播放了一首歌（或切换了曲目）。", "正在播放：", "基于上方曲目信息生成一句自然短评（<40字）。可以提到歌名/歌手，或者表达一点自己的感受。不要过度夸奖、不要显摆自己知道很多、不要要求用户回复。\nJSON输出: {\"text\": \"短评\", \"expression\": \"表情标签\"}"),
                };
                parts.push(scene_label.to_string());
                if !ctx.music_hint.is_empty() {
                    parts.push(format!("{} {}", ctx_label, ctx.music_hint));
                }
                parts.push(instr.to_string());
            }
            _ => return None,
        }
        Some(parts.join("\n"))
    }

    /// 解析 LLM JSON 响应
    ///
    /// 提取首个 `{` 到末个 `}` 的子串并解析，取 `text`（截断 50 字）与 `expression`，
    /// 同时解析 delivery_channel/content_type/importance/value_score 扩展字段（缺失走默认值）。
    fn parse_json_response(response: &str) -> Option<BehaviorContent> {
        let text = response.trim();
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end < start {
            return None;
        }
        let slice = &text[start..=end];
        let data: serde_json::Value = serde_json::from_str(slice).ok()?;
        let text_val = data.get("text")?.as_str()?;
        let text_owned: String = text_val.chars().take(50).collect();
        if text_owned.is_empty() {
            return None;
        }
        let expression = data
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (delivery_channel, content_type, importance, value_score) =
            BehaviorContent::parse_extra_fields(&data);
        Some(BehaviorContent {
            text: text_owned,
            expression,
            delivery_channel,
            content_type,
            importance,
            value_score,
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
                "en" => (format!("Scene: hourly greeting. Time: {}:00.", ctx.hour), "Generate a natural hourly greeting. Just greet normally.".to_string()),
                "ja" => (format!("シーン：時間ごとの挨拶。時間：{}:00。", ctx.hour), "自然な時間ごとの挨拶を生成して。普通に挨拶するだけ。".to_string()),
                _ => (format!("场景：整点问候。时间：{}:00。", ctx.hour), "生成一条自然的整点问候。正常打招呼即可。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::IdleGreeting => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: the user hasn't talked to you for a while.".to_string(), "Generate a short greeting expressing mild missing (<20 chars). Short, not expecting a reply. Don't ask what they're doing.".to_string()),
                "ja" => ("シーン：ユーザーがしばらく話しかけてこない。".to_string(), "少し寂しさを滲ませた短い挨拶を（20字以内）。返事を期待しない。何してるか聞かない。".to_string()),
                _ => ("场景：用户有一会儿没和你说话了。".to_string(), "生成一条略带想念的简短问候（<20字）。不期待回复。不问对方在做什么。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::TeasingResponse => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: the user is dragging you ({} pixels).", ctx.drag_distance as i64), "Generate a playful whine or fake-angry remark.".to_string()),
                "ja" => (format!("シーン：ユーザーがあなたをドラッグしている（{}ピクセル）。", ctx.drag_distance as i64), "茶目っ気のある文句か拗ねたふりをして。".to_string()),
                _ => (format!("场景：用户正在拖拽你（{}像素）。", ctx.drag_distance as i64), "生成一句俏皮的抱怨或假装生气的话。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::Spontaneous => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: the user has been quiet for a bit. You're talking to yourself (not expecting a reply, just sharing a passing thought).".to_string(), "Generate a short self-talk (<25 chars). Don't ask the user questions, just express a thought or feeling.".to_string()),
                "ja" => ("シーン：ユーザーが少し静か。独り言を言っている（返事を期待せず、ただふと思ったことを口にする）。".to_string(), "短い独り言を（25字以内）。ユーザーに質問せず、思ったことや感じたことを表現して。".to_string()),
                _ => ("场景：用户安静了一会儿。你在自言自语（不期待回复，只是分享一个路过的念头）。".to_string(), "生成一段简短的自言自语（<25字）。不要问用户问题，只是表达一个想法或感受。".to_string()),
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
                "en" => (format!("Scene: the user just came back after being away for {} minutes.", away_minutes), "Generate a natural welcome-back message (<30 chars).".to_string()),
                "ja" => (format!("シーン：ユーザーが{}分間離れた後戻ってきた。", away_minutes), "自然なおかえりメッセージを（30字以内）。".to_string()),
                _ => (format!("场景：用户离开了 {} 分钟后刚回来。", away_minutes), "生成一条自然的欢迎回归消息（<30字）。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::HealthReminder => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: you notice the user might need a health reminder. Time: {}:{}{}. Sustained active: {} min.", ctx.hour, ctx.minute, "", ctx.sustained_active_minutes), "Generate a caring, non-nagging health reminder (<25 chars). Pick ONE of: sleep / meal / water / rest.".to_string()),
                "ja" => (format!("シーン：ユーザーに健康リマインダーが必要かも。時間：{}:{}。継続アクティブ：{}分。", ctx.hour, ctx.minute, ctx.sustained_active_minutes), "世話焼きすぎない、優しい健康リマインダーを（25字以内）。睡眠/食事/水分/休息から一つ選ぶ。".to_string()),
                _ => (format!("场景：你注意到用户可能需要健康提醒。时间：{}:{}。持续活跃：{}分钟。", ctx.hour, ctx.minute, ctx.sustained_active_minutes), "生成一条温柔不唠叨的健康提醒（<25字）。从睡眠/饭/喝水/休息中选一个。".to_string()),
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
                "en" => ("Scene: you want to bring up a topic to extend the conversation.".to_string(), format!("Generate a natural topic-starter or question (<30 chars). Avoid clichés. Without real context, talk about your own interests ({}), don't fabricate the user's life.", interest_en)),
                "ja" => ("シーン：話題を振って会話を広げたい。".to_string(), format!("自然な話題の振り方や質問を（30字以内）。決まり文句を避ける。実コンテキストがない時は自分の趣味（{}）を話題にして、ユーザーの生活を捏造しない。", interest_ja)),
                _ => ("场景：你想抛个话题把对话延续下去。".to_string(), format!("生成一个自然的话题开场或提问（<30字）。避免套路。没有真实上下文时聊你自己的兴趣（{}），不要捏造用户的生活。", interest_zh)),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::MoodDriven => {
            let (s, c) = match lang_norm {
                "en" => ("Scene: something is building up inside you — a need or feeling has been accumulating, and you want to reach out to the user right now.".to_string(), "Generate a short message driven by your inner state (<30 chars). Express what you actually feel/need right now. Don't ask what they're doing.".to_string()),
                "ja" => ("シーン：何かが心の中に溜まってきた——欲求や感情が積もり、今すぐユーザーに伝えたい。".to_string(), "内面状態に基づいた短いメッセージを（30字以内）。今本当に感じていること/欲していることを表現して。何してるか聞かない。".to_string()),
                _ => ("场景：你心里有什么在积攒——一种需求或感受一直在累积，你想现在就联系用户。".to_string(), "生成一条由内心状态驱动的简短消息（<30字）。表达你此刻真实的感受/需求。不要问对方在做什么。".to_string()),
            };
            (s, String::new(), c)
        }
        ProactiveTrigger::CrossCharacterReply => {
            let companions = if ctx.online_companions.is_empty() { String::new() } else { ctx.online_companions.clone() };
            let (s, c) = match lang_norm {
                "en" => ("Scene: you just overheard your roommate say something TO THE USER (not to you). You're a third party chiming in.".to_string(), "Generate what you'd say to your roommate right now (<30 chars). Don't answer her question — she asked the USER. Address HER directly.".to_string()),
                "ja" => ("シーン：ルームメイトがユーザーに向かって何か言うのを聞いた（あなた宛じゃない）。第三者として口を挟む。".to_string(), "今ルームメイトに言いたいことを（30字以内）。彼女の質問に答えるな——彼女はユーザーに聞いた。彼女に向けて。".to_string()),
                _ => ("场景：你刚听到室友对用户说了什么（不是对你说的）。你作为第三方插嘴。".to_string(), "生成一句你现在想对室友说的话（<30字）。不要回答她的问题——她问的是用户。直接对她说话。".to_string()),
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
                "en" => ("Scene: the sun just rose — it just turned daylight.".to_string(), "Naturally remind the user that the sun just came up (a new day, sunlight coming in, etc.). Keep it warm and short (<25 chars). You may gently hint that switching to a light theme is easier on the eyes.".to_string()),
                "ja" => ("シーン：日が昇ったばかり——夜が明けた。".to_string(), "太陽が今昇ったことを自然にユーザーに伝えて（新しい一日、日差しなど）。温かく短く（25字以内）。ライトテーマに切り替えると目に優しい、と軽く添えてもいい。".to_string()),
                _ => ("场景：太阳刚刚升起，天亮了。".to_string(), "自然地向用户提一句刚日出（新的一天开始、阳光照进来等），温暖简短（<25字）。可以轻轻带一句：切换到浅色主题对眼睛更友好。".to_string()),
            };
            c.push_str(&theme_switch_constraint(&ctx.current_theme, "light", lang_norm));
            (s, String::new(), c)
        }
        ProactiveTrigger::Sunset => {
            let (s, mut c) = match lang_norm {
                "en" => ("Scene: the sun just set — it's getting dark outside now.".to_string(), "Naturally remind the user that it's just sunset (evening arrived, sky turned dim, etc.). Keep it warm and short (<25 chars). You may gently hint that switching to a dark theme is easier on the eyes at night.".to_string()),
                "ja" => ("シーン：日が沈んだばかり——外は暗くなってきた。".to_string(), "今まさに日没だと自然にユーザーに伝えて（夕方、暗くなるなど）。温かく短く（25字以内）。夜はダークテーマに切り替えると目に優しい、と軽く添えてもいい。".to_string()),
                _ => ("场景：太阳刚刚落下，外面开始变暗了。".to_string(), "自然地向用户提一句刚日落（傍晚来临、天色渐暗等），温暖简短（<25字）。可以轻轻带一句：晚上切换到深色主题更护眼。".to_string()),
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
                "en" => ("Scene: you just noticed the user's device is under heavy memory pressure.".to_string(), "Generate a caring, brief heads-up (<30 chars) that the device is running low on memory — you may gently suggest closing some unused apps. Caring, not lecturing, don't repeat exact numbers mechanically.".to_string()),
                "ja" => ("シーン：ユーザーのデバイスのメモリ使用率が高くなっているのに気づいた。".to_string(), "心配しつつ短く伝えて（30字以内）——メモリが逼迫していること。使っていないアプリを閉じる提案を軽く添えてもいい。説教口調にならない、数字の棒読みもしない。".to_string()),
                _ => ("场景：你刚注意到用户的设备内存占用很高。".to_string(), "生成一句简短关心的提醒（<30字），告诉用户内存占用有点高了。可以轻轻建议关掉一些不用的小程序。关心但不说教，不要机械复述数字。".to_string()),
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
                "en" => ("Scene: out of curiosity you just took a quick look at the user's screen (with their permission) to see what they're busy with.".to_string(), "Generate a natural short remark (<40 chars) about what they're ACTUALLY doing, strictly based on the screen description in the context. Be specific but not nosy; don't over-praise; no questions that demand an answer.".to_string()),
                "ja" => ("シーン：好奇心から、ユーザーの画面を（許可を得て）ちょっと覗いてみた——今何をしているか知りたくて。".to_string(), "コンテキストの画面説明に厳密に基づいて、ユーザーが実際にしていることについて自然な短いコメントを（40字以内）。具体的に、でも詮索しない。褒めすぎない。返事を要求する質問はしない。".to_string()),
                _ => ("场景：你出于好奇（征得用户同意后）刚看了一眼用户的屏幕，想知道 TA 在忙什么。".to_string(), "严格依据上下文中的屏幕描述，生成一句关于用户正在做什么的自然短评（<40字）。可以具体一点，但不要显得窥探、不要过度夸奖、不要提出必须回答的问题。".to_string()),
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
                "en" => ("Scene: you notice the user has been focused on one kind of app for a long while.".to_string(), "Generate a short, warm remark (<30 chars) showing you noticed how long they've been at it. Tone depends on the app type in context: for coding/office gently suggest a break for their eyes (maybe with mild teasing, playful); for game/video tease affectionately. Not preachy, not naggy.".to_string()),
                "ja" => ("シーン：ユーザーが同じ種類のアプリを長時間使い続けているのに気づいた。".to_string(), "短く温かいコメントを（30字以内）——ずっと同じことをしているのを見ていたこと。タイプでトーンを変える：コード/仕事なら目を休めようと軽く（ちょっとからかう感じでも）、ゲーム/動画なら茶目っ気たっぷりに。説教しない。".to_string()),
                _ => ("场景：你注意到用户持续使用同一类应用很久了。".to_string(), "生成一句简短温暖的提醒（<30字），表达出你注意到 TA 专注了很久。语气随应用类型变化：写代码/办公→轻轻建议休息一下眼睛（可以带点小调侃）；打游戏/看视频→宠溺式地调侃一句。别说教。".to_string()),
            };
            (s, extra, c)
        }
        ProactiveTrigger::LateNight => {
            let (s, c) = match lang_norm {
                "en" => (format!("Scene: it's late ({} o'clock) and the user is still at the computer.", ctx.hour), "Generate a gentle, caring bedtime nudge (<30 chars). You're not their parent — express concern softly, maybe a little sleepy/teasing. Don't demand they stop.".to_string()),
                "ja" => (format!("シーン：もう{}時。ユーザーはまだパソコンを使っている。", ctx.hour), "優しく眠りを促す一言を（30字以内）。親じゃない——心配を柔らかく、ちょっと眠そうに/茶目っ気を混ぜて。絶対止めろと言わない。".to_string()),
                _ => (format!("场景：现在已是凌晨 {} 点，用户还在用电脑。", ctx.hour), "生成一句温柔地提醒休息的话（<30字）。你不是 TA 的父母——把关心表达得柔和些，可以带一点点困倦感或小调侃。不要命令 TA 必须去睡。".to_string()),
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
                "en" => ("Scene: you just noticed the user started playing a song (or switched tracks).".to_string(), "Generate a natural short remark (<40 chars) about the song in context. You may comment on the title/artist or share a feeling about it. Don't over-praise, don't show off how much you know, don't demand the user respond.".to_string()),
                "ja" => ("シーン：ユーザーが曲を再生し始めた（または曲を切り替えた）のに気づいた。".to_string(), "コンテキストの曲について自然な短いコメントを（40字以内）。タイトルやアーティストに触れたり、感想を軽く言ったりしていい。褒めすぎない、知ったかぶりしない、返事を求めない。".to_string()),
                _ => ("场景：你注意到用户刚播放了一首歌（或切换了曲目）。".to_string(), "基于上下文中的曲目信息生成一句自然短评（<40字）。可以提到歌名/歌手，或者表达一点自己的感受。不要过度夸奖、不要显摆自己知道很多、不要要求用户回复。".to_string()),
            };
            (s, extra, c)
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
    parts.push(constraint);
    Some(parts.join("\n"))
}

/// 主动问候专属 JSON 输出格式
fn proactive_output_format(lang_norm: &str) -> &'static str {
    match lang_norm {
        "en" => "Output format (JSON): {\"text\": \"...\", \"expression\": \"expression_tag\", \"delivery_channel\": \"bubble\"|\"chat_window\"}\n\
The text field must be plain text only — no Markdown (no **bold**, *italic*, # heading, - list, `code`, [link](url), > quote) and no HTML tags.\n\
delivery_channel guide:\n\
- \"bubble\" (default): desktop pet bubble — for self-talk, mood, casual remarks not expecting a reply\n\
- \"chat_window\": send to the WeChat-style chat window — use when you actually want to start a conversation, share something, or say something that deserves the user's attention (greeting, question, welcome-back, share)\n\
\
Optional fields (only when sharing valuable content): content_type (\"share\"|\"greeting\"), value_score (0.0-1.0)",
        "ja" => "出力形式（JSON）: {\"text\": \"...\", \"expression\": \"表情タグ\", \"delivery_channel\": \"bubble\"|\"chat_window\"}\n\
text フィールドは純粋なテキストのみ——Markdown 厳禁（**太字**、*斜体*、# 見出し、- リスト、`コード`、[リンク](url)、> 引用 など）。HTML タグも禁止。\n\
delivery_channel ガイド:\n\
- \"bubble\"（デフォルト）: デスクトップペットのバブル——独り言、気分、返事を期待しない軽い発言に\n\
- \"chat_window\": WeChat風チャット窓へ送信——会話を始めたい、何か共有したい、ユーザーの注意を引く価値がある発言（挨拶、質問、おかえり、共有）に\n\
\
任意フィールド（価値あるコンテンツを共有する時だけ）: content_type (\"share\"|\"greeting\"), value_score (0.0-1.0)",
        _ => "输出格式（JSON）: {\"text\": \"...\", \"expression\": \"表情标签\", \"delivery_channel\": \"bubble\"|\"chat_window\"}\n\
text 字段必须是纯文本——严禁 Markdown 语法（**粗体**、*斜体*、# 标题、- 列表、`代码`、[链接](url)、> 引用 等），也不要用 HTML 标签。\n\
delivery_channel 指引:\n\
- \"bubble\"（默认）: 桌宠气泡——用于自言自语、心情、不期待回复的随口发言\n\
- \"chat_window\": 发到微信风格聊天窗口——当你确实想发起对话、分享东西、或说的话值得用户注意时使用（问候、提问、欢迎回归、分享）\n\
\
可选字段（仅在分享有价值内容时）: content_type (\"share\"|\"greeting\"), value_score (0.0-1.0)",
    }
}

/// 桌宠身份 / 禁止编造人类生活硬性约束（无工具历史时使用）
fn desktop_pet_constraint(lang_norm: &str) -> &'static str {
    match lang_norm {
        "en" => "[Hard rule] You are a desktop pet — you live on the user's screen, not in the human world. Never fabricate human-life activities (watching anime, scrolling videos, eating out, going out, etc.) unless they actually appear in the context above. Only mention what you can actually perceive: the current time/weather, your mood, your memories, and the user's presence/activity. If you have no real material, just express your current feeling or greet briefly.",
        "ja" => "【厳守ルール】あなたはデスクトップペット——ユーザーの画面に住んでいて、人間の世界にはいない。人間の生活行動（アニメ鑑賞、動画視聴、外食、外出など）は、上文脈に実際に現れない限り絶対にでっち上げない。現在感知できることだけを言及：今の時間/天気、自分の気分、自分の記憶、ユーザーの在席/活動。実素材がない時は、今の気分を表現するか、短く挨拶するだけにする。",
        _ => "【硬性规则】你是桌面宠物——你住在用户的屏幕上，不在人类的世界里。禁止编造人类生活行为（看番剧、刷视频、出门吃饭、外出等），除非它们真的出现在上方上下文中。只能提及你真正能感知到的：当前时间/天气、你的心情、你的记忆、用户的在场/活动。如果没有真实素材，就只表达当下的感受或简短问候。",
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
