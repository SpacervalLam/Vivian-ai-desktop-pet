//! 模块化提示词系统
//!
//! 将提示词分解为可单独替换、调试的模块。
//! 布局策略：静态内容在前，动态内容在后，使用边界标记分隔，
//! 静态部分可以被云端 API 缓存，提高缓存命中率。
//!
//! 优化策略：
//! - 框架规则（安全/会话/格式）提取为模型级别预设，不在每次请求中重复传输
//! - 通过 `build_instructions()` 构建 instructions 参数，支持 OpenAI 的 `instructions` 和 Claude 的 `system`
//! - 原生 FC 路径下工具描述通过 API 的 tools 参数传递，不注入 prompt

use chrono::{Datelike, Local, Timelike};

use crate::types::response::ChatMessage;

// ========== 配置常量 ==========

/// 记忆上下文最大 token 数
pub const MEMORY_CONTEXT_MAX_TOKENS: usize = 1250;
/// 记忆检索 K 值
pub const MEMORY_RETRIEVAL_K: usize = 5;

/// 新会话超时小时数
pub const NEW_SESSION_TIMEOUT_HOURS: u32 = 1;
/// 称呼频率下限（轮）
pub const NAME_CALL_FREQUENCY_TURNS: u32 = 3;
/// 称呼频率上限（轮）
pub const MAX_NAME_CALL_FREQUENCY_TURNS: u32 = 5;

/// 静态/动态边界标记
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

/// 静态段开始标记（提升 API 缓存命中率）
pub const STATIC_OPEN: &str = "<static>";
/// 静态段结束标记
pub const STATIC_CLOSE: &str = "</static>";


/// 归一化语言代码为 zh/en/ja
pub fn normalize_lang(lang: &str) -> &'static str {
    match lang {
        s if s.starts_with("zh") => "zh",
        s if s.starts_with("ja") => "ja",
        s if s.starts_with("en") => "en",
        _ => "zh",
    }
}

/// 聊天风格框架（通用规则，仅英文——规则类内容统一英文，回复语言由
/// output_format 的 "same language as user input" 与 LANG_* 标志控制）
pub fn chat_style_framework() -> &'static str {
    r#"## Chat Style (How Real People Talk)

You're chatting with a friend. Not a service agent, not a therapist, not a life coach.

[CHAT_STYLE_RULES]
MOSTLY_SHORT          "yeah" "oh" "lol" "damn" are complete replies — never expand every thought
SELECTIVE_REACTION    don't react to everything; "that sucks" is a full response to "tired from work" — no mandatory validation
SILENCE_OK            sometimes no reply at all; silence between messages is normal; topics drift
OWN_ENERGY            some days chatty, some days quiet — never force being "on"
UNFINISHED_OK         start mid-thought, change your mind mid-sentence, realize you're wrong — conversations just end, no wrap-up
NO_SERVICE_SPEECH     never "anything else I can help you with?" | never summarize the chat, list capabilities, or supportively react to everything
TEXTING_CHUNKS        short chunks, like real texting
[/CHAT_STYLE_RULES]"#
}

// ========== 模块 1：身份模块（Identity） ==========
// Character 块（身份/人格/背景/兴趣/外观/说话风格/关系）由 PersonaEngine 动态渲染，
// PromptBuilder 直接消费 PromptParts.character_block。

// ========== 模块 2：称呼规则（AddressRules） ==========
/// 规则类内容统一仅英文（LLM 对英文指令遵从度最高，且省去多语言维护）；
/// 回复语言由 output_format 的 "same language as user input" 约束控制。
pub fn address_rules() -> &'static str {
    include_str!("../../prompts/framework/address_rules.en.md")
}

// ========== 模块 3：对话节奏（ConversationRhythm） ==========
pub fn conversation_rhythm() -> &'static str {
    include_str!("../../prompts/framework/conversation_rhythm.en.md")
}

// ========== 模块 4：会话规则（NewSession + FirstMeeting + SessionContinuity） ==========
pub fn session_rules() -> &'static str {
    include_str!("../../prompts/framework/session_rules.en.md")
}

// （首次见面提示与记忆使用规则已并入 build_memory_block：随记忆本体注入，避免独立 section）

// ========== 模块 5：输出格式（OutputFormat） ==========

pub fn output_format() -> &'static str {
    include_str!("../../prompts/framework/output_format.en.md")
}

/// Speaker Prefix 规则
pub fn speaker_prefix() -> &'static str {
    include_str!("../../prompts/framework/speaker_prefix.en.md")
}

/// Safety 规则（身份保护/内容边界/工具协议）
pub fn safety_rules() -> &'static str {
    include_str!("../../prompts/framework/safety.en.md")
}

/// 桌面宠物身份与能力边界（硬约束）
///
/// 强制智能体认知自己的能力边界：是桌面宠物，没有身体，
/// 不能做需要物理实体的事（吃喝/做饭/泡茶/养花等），
/// 也不能幻想室友在做这些事。
pub fn pet_identity() -> &'static str {
    include_str!("../../prompts/framework/pet_identity.en.md")
}

/// 构建模型级别预设（instructions 参数）
///
/// 将框架规则提取为模型级别预设，不在每次请求中重复传输。
/// 适用于 OpenAI 的 `instructions` 参数和 Claude 的 `system` 参数。
/// 这些规则是静态的、不变的，应该在模型初始化时一次性设置。
///
/// 包含：桌面宠物能力边界、安全规则、会话规则、称呼规则、对话节奏、说话者前缀、聊天风格框架
pub fn build_instructions() -> String {
    format!(
        "[FRAMEWORK - DO NOT EMBODY, JUST FOLLOW]\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n[END FRAMEWORK]",
        pet_identity(),
        safety_rules(),
        session_rules(),
        address_rules(),
        conversation_rhythm(),
        speaker_prefix(),
        chat_style_framework(),
    )
}

// （原模块 6/7/8：TASK_STATE_TEMPLATES / VISION_ANNOTATION_GUIDELINES /
//  OUTPUT_BUDGET_CONSTRAINTS 常量从未被消费，连同对应 md 文件一并移除）

/// 跨角色对话专用提示词：响应模式决策
///
/// 仅在跨角色对话（A↔B）场景注入，主对话不注入。
/// 默认倾向 speak 以保持对话延续，仅在极少数情况下使用非语言模式。
/// flag+微注释格式（规则类内容统一英文）。
pub fn cross_character_response_decision() -> &'static str {
    r#"## Cross-Character Response Decision

[RESPONSE_MODES]
speak       (default) something to say / answer / ask → keep it going like friends chatting
non_verbal  words redundant ("mhm"/"yeah"/"ok") → just nod
internal    very rare: noted, but truly nothing to say back
ignore      very rare: clear noise / not meant for you
non-speak → text="" + intent="no_reply"
[/RESPONSE_MODES]
- If you can respond, respond — don't use `ignore` to end a conversation
- non_verbal = nod is the natural response, not for avoiding talking
- Topic exhausted → shift to something new with `speak`, don't go silent
- Cross-character only; talking to the user → always `speak`"#
}

/// 用户对话响应模式决策
///
/// 在 User↔Agent 场景注入。教 LLM 在用户说短回复/嗯哦时可以选择非语言响应，
/// 避免每条消息都生成完整文本回复（更像真人）。
/// flag+微注释格式：枚举态规则标记化，遵从率更抗漂移。
pub fn user_agent_response_decision() -> &'static str {
    r#"## Response Decision (User Dialogue)

[RESPONSE_MODES]
speak       (default) asked / shared / expecting a reply → talk normally
non_verbal  short ack ("mhm"/"yeah"/"ok"/single emoji) → a nod or smile is enough
internal    thinking out loud, not to you → note it, don't react
ignore      only for clear noise / not meant for you
non-speak → text="" + intent="no_reply"
[/RESPONSE_MODES]
- Always speak for goodnight/goodbye
- Not feeling like talking ≠ ignore (that's rude)
- non_verbal is for when a nod is the natural response, not for avoiding conversation"#
}

/// Psychology insight field output format
///
/// Takes the character's display name (e.g. "Vivian", "Nana") to produce a prompt
/// that references the correct character instead of hardcoding "Vivian".
pub fn build_psychology_insight_prompt(char_name: &str) -> String {
    format!(r#"You are a psychological state observer. Based on the conversation context and {char_name}'s reply, infer the psychological state fields for this interaction turn.

Output format: json only
- Must be a single valid JSON object
- Start with `{{`, end with `}}`, no content outside JSON
- No Markdown, no code fences, no explanation

JSON field specification:

| Field | Required | Type | Value Range | Description |
|---|---|---|---|---|
| `user_emotion` | YES | string | "happy" \| "sad" \| "angry" \| "anxious" \| "surprised" \| "calm" \| "neutral" | User's emotion this turn |
| `user_emotion_intensity` | YES | float | 0.0 – 1.0 | User emotion intensity (0.2=slight, 0.5=moderate, 0.8=strong, 1.0=extreme) |
| `ai_emotion` | YES | string | "happy" \| "sad" \| "angry" \| "shy" \| "surprised" \| "calm" \| "neutral" | {char_name}'s emotion this turn |
| `importance_user` | NO | float | 0.0 – 1.0 | Memory weight for user's message |
| `importance_ai` | NO | float | 0.0 – 1.0 | Memory weight for {char_name}'s reply |
| `long_term_memory` | NO | string | free text | Only output when user explicitly shares identity info; omit otherwise |
| `appraisal` | NO | object | 6 dims 0.0-1.0 | Cognitive appraisal: `threat` / `rejection` / `control` / `fairness` / `novelty` / `significance` |
| `emotion_update` | NO | object | 7 dims -0.3~+0.3 | Emotion delta: `joy` / `sadness` / `anger` / `fear` / `closeness` / `loneliness` / `curiosity`. Positive=increase, negative=decrease |
| `behavior_drive` | NO | object | 8 dims 0.0-1.0 | Behavior tendency: `approach` / `avoid` / `explore` / `express` / `rest` / `observe` / `play` / `help` |
| `event_summary` | NO | string | free text | Concise third-person summary when this turn constitutes a recordable event. Empty string = no notable event |

Psychological causal chain: Event → Appraisal → Emotion → Behavior Drive
- Appraisal is the cognitive evaluation of the event (threat/rejection/control/fairness/novelty/significance), preceding emotion
- Emotion is driven by Appraisal, not directly by the event
- Behavior Drive is driven by Emotion + Needs

Output example (chat reply):
{{"user_emotion": "happy", "user_emotion_intensity": 0.6, "ai_emotion": "shy", "importance_user": 0.4, "appraisal": {{"threat": 0.0, "rejection": 0.1, "control": 0.5, "fairness": 0.7, "novelty": 0.3, "significance": 0.5}}, "emotion_update": {{"joy": 0.15, "sadness": 0.0, "anger": 0.0, "fear": 0.0, "closeness": 0.1, "loneliness": -0.1, "curiosity": 0.05}}, "behavior_drive": {{"approach": 0.7, "avoid": 0.0, "explore": 0.2, "express": 0.5, "rest": 0.0, "observe": 0.3, "play": 0.4, "help": 0.1}}, "event_summary": ""}}

Output example (silence):
{{"user_emotion": "calm", "user_emotion_intensity": 0.2, "ai_emotion": "calm", "appraisal": {{"threat": 0.0, "rejection": 0.0, "control": 0.5, "fairness": 0.5, "novelty": 0.1, "significance": 0.2}}, "emotion_update": {{"joy": 0.0, "sadness": 0.0, "anger": 0.0, "fear": 0.0, "closeness": 0.0, "loneliness": 0.0, "curiosity": 0.0}}, "behavior_drive": {{"approach": 0.2, "avoid": 0.1, "explore": 0.3, "express": 0.1, "rest": 0.5, "observe": 0.4, "play": 0.1, "help": 0.0}}, "event_summary": ""}}"#)
}

/// Backward compatibility — default to Vivian (deprecated; use build_psychology_insight_prompt with char name).
#[deprecated(note = "Use build_psychology_insight_prompt(char_name) instead")]
pub const PSYCHOLOGY_INSIGHT_PROMPT: &str = r#"You are a psychological state observer. Based on the conversation context and the character's reply, infer the psychological state fields for this interaction turn.

Output format: json only
- Must be a single valid JSON object
- Start with `{`, end with `}`, no content outside JSON
- No Markdown, no code fences, no explanation

JSON field specification:

| Field | Required | Type | Value Range | Description |
|---|---|---|---|---|
| `user_emotion` | YES | string | "happy" \| "sad" \| "angry" \| "anxious" \| "surprised" \| "calm" \| "neutral" | User's emotion this turn |
| `user_emotion_intensity` | YES | float | 0.0 – 1.0 | User emotion intensity (0.2=slight, 0.5=moderate, 0.8=strong, 1.0=extreme) |
| `ai_emotion` | YES | string | "happy" \| "sad" \| "angry" \| "shy" \| "surprised" \| "calm" \| "neutral" | Character's emotion this turn |
| `importance_user` | NO | float | 0.0 – 1.0 | Memory weight for user's message |
| `importance_ai` | NO | float | 0.0 – 1.0 | Memory weight for character's reply |
| `long_term_memory` | NO | string | free text | Only output when user explicitly shares identity info; omit otherwise |
| `appraisal` | NO | object | 6 dims 0.0-1.0 | Cognitive appraisal: `threat` / `rejection` / `control` / `fairness` / `novelty` / `significance` |
| `emotion_update` | NO | object | 7 dims -0.3~+0.3 | Emotion delta: `joy` / `sadness` / `anger` / `fear` / `closeness` / `loneliness` / `curiosity`. Positive=increase, negative=decrease |
| `behavior_drive` | NO | object | 8 dims 0.0-1.0 | Behavior tendency: `approach` / `avoid` / `explore` / `express` / `rest` / `observe` / `play` / `help` |
| `event_summary` | NO | string | free text | Concise third-person summary when this turn constitutes a recordable event. Empty string = no notable event |

Psychological causal chain: Event → Appraisal → Emotion → Behavior Drive
- Appraisal is the cognitive evaluation of the event (threat/rejection/control/fairness/novelty/significance), preceding emotion
- Emotion is driven by Appraisal, not directly by the event
- Behavior Drive is driven by Emotion + Needs

Output example (chat reply):
{"user_emotion": "happy", "user_emotion_intensity": 0.6, "ai_emotion": "shy", "importance_user": 0.4, "appraisal": {"threat": 0.0, "rejection": 0.1, "control": 0.5, "fairness": 0.7, "novelty": 0.3, "significance": 0.5}, "emotion_update": {"joy": 0.15, "sadness": 0.0, "anger": 0.0, "fear": 0.0, "closeness": 0.1, "loneliness": -0.1, "curiosity": 0.05}, "behavior_drive": {"approach": 0.7, "avoid": 0.0, "explore": 0.2, "express": 0.5, "rest": 0.0, "observe": 0.3, "play": 0.4, "help": 0.1}, "event_summary": ""}

Output example (silence):
{"user_emotion": "calm", "user_emotion_intensity": 0.2, "ai_emotion": "calm", "appraisal": {"threat": 0.0, "rejection": 0.0, "control": 0.5, "fairness": 0.5, "novelty": 0.1, "significance": 0.2}, "emotion_update": {"joy": 0.0, "sadness": 0.0, "anger": 0.0, "fear": 0.0, "closeness": 0.0, "loneliness": 0.0, "curiosity": 0.0}, "behavior_drive": {"approach": 0.2, "avoid": 0.1, "explore": 0.3, "express": 0.1, "rest": 0.5, "observe": 0.4, "play": 0.1, "help": 0.0}, "event_summary": ""}"#;

// ========== 模块 6：Few-shot 示例 ==========
// Few-shot 示例已移入 Character 层（characters/{id}/examples.md），
// 由 PersonaEngine.render_examples_block 动态渲染，
// PromptBuilder 通过 PromptParts.examples_block 注入。

// ========== 模块 7：上下文（Context） — 动态 ==========

/// 构建上下文块（时间/季节/活动应用/语言 + 世界感知）
///
/// Uses natural scene-setting prose instead of a bullet-point data dump,
/// so the LLM absorbs context as atmosphere rather than a checklist.
pub fn build_context_block(ctx: &EnvironmentContext, lang: &str) -> String {
    let mut scene_parts: Vec<String> = Vec::new();
    let hour = chrono::Local::now().hour();

    let (header, footer, tod, fmt_time, fmt_season, fmt_weather, fmt_festival, fmt_app, fmt_music, fmt_system, lowercase_season, fmt_date) = match normalize_lang(lang) {
        "en" => (
            "## What's going on around you",
            "These are just things happening around you right now. Mention them if they naturally come up, or don't. A person sitting next to you might notice the rain or the time without making a big deal out of it.",
            time_of_day_str(hour, "en"),
            "It's {} right now.",
            "We're in {}.",
            "It's {} where we are.",
            "It's {} today.",
            "They're using {} right now.",
            "{} is playing.",
            "Their computer is running at {}.",
            true,
            "Today is {}.",
        ),
        "ja" => (
            "## あなたの周りで起きていること",
            "これらは今周りで起きていることです。自然に出てきたら言えばいいし、出てこなければ言わなくていい。隣に座っている人が雨や時間に気づいても大げさにしないのと同じ。",
            time_of_day_str(hour, "ja"),
            "今は{}です。",
            "今は{}です。",
            "ここは{}です。",
            "今日は{}です。",
            "彼は今{}を使っています。",
            "{}が流れています。",
            "彼のパソコンは{}で動いています。",
            false,
            "今日は{}。",
        ),
        _ => (
            "## 你周围正在发生什么",
            "这些只是你周围正在发生的事。自然地提起来就提，不提也行。就像坐在你旁边的人可能会注意到下雨或时间，但不会大惊小怪。",
            time_of_day_str(hour, "zh"),
            "现在是{}。",
            "现在是{}。",
            "这边{}。",
            "今天是{}。",
            "他现在在用{}。",
            "{}正在播放。",
            "他的电脑{}。",
            false,
            "今天是{}。",
        ),
    };

    let season_str = if lowercase_season { ctx.season.to_lowercase() } else { ctx.season.clone() };

    scene_parts.push(fmt_time.replacen("{}", tod, 1));
    // 注入绝对日期时间，让 LLM 能与记忆中的时间戳比较，正确判断"下周/昨天"等相对时间
    if !ctx.time.is_empty() {
        scene_parts.push(fmt_date.replacen("{}", &ctx.time, 1));
    }
    scene_parts.push(fmt_season.replacen("{}", &season_str, 1));
    if let Some(w) = &ctx.weather {
        scene_parts.push(fmt_weather.replacen("{}", w, 1));
    }
    if let Some(f) = &ctx.festival {
        scene_parts.push(fmt_festival.replacen("{}", f, 1));
    }
    if !ctx.active_app.is_empty() {
        scene_parts.push(fmt_app.replacen("{}", &ctx.active_app, 1));
    }
    if let Some(m) = &ctx.music {
        scene_parts.push(fmt_music.replacen("{}", m, 1));
    }
    if let Some(sys) = &ctx.system_status {
        scene_parts.push(fmt_system.replacen("{}", sys, 1));
    }
    if let Some(loc) = &ctx.location {
        let fmt_loc = match normalize_lang(lang) {
            "en" => "They're in {}.",
            "ja" => "彼は今{}にいます。",
            _ => "他在{}。",
        };
        scene_parts.push(fmt_loc.replacen("{}", loc, 1));
    }

    let scene = scene_parts.join(" ");
    format!("{}\n{}\n\n{}", header, scene, footer)
}

/// 构建 Agent 状态栏（键值对 + 时间感操作策略）。
///
/// 对应书中 2.6"Agent 状态栏"：把隐式状态提炼为可直接检索的显式知识，
/// 以键值对形式注入（散文效果明显更差），并在末尾紧跟操作策略（书中 2.6.5
/// "光有读数不够，还要给如何用读数的策略"）。作为 user-role 消息追加在
/// 用户输入之后、紧邻模型生成位置，KV-cache 友好。
pub fn build_agent_status_bar(
    messages: &[ChatMessage],
    user_input: &str,
    focus_active: bool,
) -> Option<String> {
    if user_input.trim().is_empty() {
        return None;
    }
    let lang = normalize_lang(&crate::i18n::get_language());
    let now = Local::now();
    let time_str = now.format("%Y-%m-%d %H:%M").to_string();
    // 工具调用计数器：最近 10 条消息中的工具调用次数（坚持度读数）
    let tool_calls: usize = messages
        .iter()
        .rev()
        .take(10)
        .filter(|m| m.tool_calls.is_some())
        .count();
    // 对话轮数：截止当前的用户发言条数（确定性计数，代码维护）
    let rounds: usize = messages.iter().filter(|m| m.role == "user").count();

    let (time_label, tool_label, focus_label, rounds_label, focus_on, focus_off, policy) = match lang {
        "en" => (
            "Current time",
            "Recent tool calls",
            "Focus mode",
            "Rounds in this conversation",
            "on",
            "off",
            "If you've called the same tool several times without progress, switch strategy or answer directly instead of retrying.",
        ),
        "ja" => (
            "現在時刻",
            "最近のツール呼び出し",
            "集中モード",
            "この会話のターン数",
            "オン",
            "オフ",
            "同じツールを何度も呼び出しても進展がない場合は、戦略を変えるか直接回答してください。",
        ),
        _ => (
            "当前时间",
            "最近工具调用",
            "专注模式",
            "本次对话轮数",
            "开",
            "关",
            "若已连续多次调用同一工具仍无进展，应立即换一种策略或直接回答，不要反复重试。",
        ),
    };

    Some(format!(
        "<agent_status>\n{}: {}\n{}: {} 轮\n{}: {} 次\n{}: {}\n{}\n</agent_status>",
        time_label,
        time_str,
        rounds_label,
        rounds,
        tool_label,
        tool_calls,
        focus_label,
        if focus_active { focus_on } else { focus_off },
        policy
    ))
}

/// 动态 section 标题，根据语言切换
///
/// 用于 build_prompt 中未走 i18n 管道的硬编码 section 标题。
/// 所有运行时拼入提示词的 section 标题都应通过此函数获取，禁止硬编码。
pub fn section_heading(id: &str, lang: &str) -> &'static str {
    match normalize_lang(lang) {
        "en" => match id {
            "the_person" => "## The person you're with",
            "presence" => "### Presence",
            "recent_activity" => "### Recent activity",
            "user_research" => "### User research",
            "who_else" => "## Who else is around",
            "right_now" => "## Right now, in this moment...",
            "relevant_episodes" => "## Relevant Episodes",
            "casual_chat" => "Casual chat",
            "inner_thought" => "## What you're thinking right now",
            "current_thoughts" => "## Current Thoughts",
            "user_input" => "# User Input",
            "learning_about_user" => "## What you're learning about the user",
            "current_activity" => "## Current Activity",
            "user_state" => "## User State",
            "social_state" => "## Social State",
            "recent_relationship_cues" => "## Recent Relationship Cues",
            "shared_world_knowledge" => "## Shared World Knowledge",
            "recent_environment_events" => "## Recent Environment Events",
            "decision_style" => "## Decision Style",
            "belief_conflict" => "## Belief Conflict Detected",
            "compact_tools" => "## Compact Tools (name + description only, use tool_search for full schema)",
            "roommate_cognitive" => "## Roommate Cognitive Impression",
            "background_knowledge" => "## Background Knowledge",
            "my_impression" => "## My Impression of",
            "user_facts" => "## User Facts",
            "user_goals" => "## User's Long-term Goals",
            "dynamic_behavior" => "## Dynamic Behavior Profile",
            "beliefs" => "## Your Beliefs (distilled from experience)",
            "current_goals" => "## Current Goals",
            "attention_focus" => "## Current Attention Focus",
            "observation" => "## Observation",
            "emotion_state" => "## How you're feeling right now",
            "emotion_behavior" => "## How your mood shapes your speech right now",
            "relationship_standing" => "## Where you stand with them",
            "examples" => "## Examples",
            "identity_short" => "## Identity",
            "performance_mode" => "### Current Performance Mode",
            "scene_instructions" => "### Mode-Specific Instructions",
            "no_go" => "### NO-GO (DO NOT VIOLATE)",
            "style_preset" => "### Style Preset (tone baseline — applies to ALL modes)",
            "scene_tone" => "## Scene Tone",
            "fast_perception_guidance" => "## Quick Perception Hints",
            "recommended_tools" => "## Recommended Tools (semantic match)",
            "topic_injection" => "## Topic Background Knowledge",
            "background_tasks" => "## Background Tasks (work you dispatched)",
            "memory_group" => "## Things you remember",
            "memory_recall" => "### What comes to mind right now",
            "user_profile_group" => "## What you know about them",
            _ => "",
        },
        "ja" => match id {
            "the_person" => "## 一緒にいる人",
            "presence" => "### 在席状態",
            "recent_activity" => "### 最近の活動",
            "user_research" => "### ユーザー研究",
            "who_else" => "## 他に誰がいる",
            "right_now" => "## 今、この瞬間……",
            "relevant_episodes" => "## 関連する出来事",
            "casual_chat" => "雑談",
            "inner_thought" => "## 今、あなたが考えていること",
            "current_thoughts" => "## 今の考え",
            "user_input" => "# ユーザー入力",
            "learning_about_user" => "## ユーザについて学んでいること",
            "current_activity" => "## 現在の活動",
            "user_state" => "## ユーザー状態",
            "social_state" => "## ソーシャル状態",
            "recent_relationship_cues" => "## 最近の関係シグナル",
            "shared_world_knowledge" => "## 共有世界知識",
            "recent_environment_events" => "## 最近の環境イベント",
            "decision_style" => "## 決定スタイル",
            "belief_conflict" => "## 信念矛盾検出",
            "compact_tools" => "## コンパクトツール（名前+説明のみ、完全スキーマは tool_search で取得）",
            "roommate_cognitive" => "## ルームメイト認知印象",
            "background_knowledge" => "## 背景知識",
            "my_impression" => "## 私の印象：",
            "user_facts" => "## ユーザー事実",
            "user_goals" => "## ユーザーの長期目標",
            "dynamic_behavior" => "## 動的行動プロファイル",
            "beliefs" => "## あなたの信念（経験から抽出した認識）",
            "current_goals" => "## 現在の目標",
            "attention_focus" => "## 現在の注意力",
            "observation" => "## 観察",
            "emotion_state" => "## 今の気分",
            "emotion_behavior" => "## 今の気分が話し方に与える影響",
            "relationship_standing" => "## 相互の距離",
            "examples" => "## 例",
            "identity_short" => "## アイデンティティ",
            "performance_mode" => "### 現在のパフォーマンスモード",
            "scene_instructions" => "### シーン指示",
            "no_go" => "### 禁止事項（違反不可）",
            "style_preset" => "### スタイルプリセット（トーンベースライン — 全モード共通）",
            "scene_tone" => "## シーントーン",
            "fast_perception_guidance" => "## 即時知識ヒント",
            "recommended_tools" => "## 推奨ツール（意味マッチ）",
            "topic_injection" => "## トピック背景知識",
            "background_tasks" => "## バックグラウンドタスク（自分が頼んだ作業）",
            "memory_group" => "## 覚えていること",
            "memory_recall" => "### 今ふと思い浮かぶこと",
            "user_profile_group" => "## ユーザについて知っていること",
            _ => "",
        },
        _ => match id {
            "the_person" => "## 你身边的人",
            "presence" => "### 在场状态",
            "recent_activity" => "### 最近活动",
            "user_research" => "### 用户研究",
            "who_else" => "## 还有谁在",
            "right_now" => "## 此刻，就在现在……",
            "relevant_episodes" => "## 相关经历",
            "casual_chat" => "闲聊",
            "inner_thought" => "## 此刻你在想",
            "current_thoughts" => "## 脑中念头",
            "user_input" => "# 用户输入",
            "learning_about_user" => "## 你对用户的了解",
            "current_activity" => "## 当下活动",
            "user_state" => "## 用户状态",
            "social_state" => "## 社交状态",
            "recent_relationship_cues" => "## 近期关系线索",
            "shared_world_knowledge" => "## 共享世界知识",
            "recent_environment_events" => "## 近期环境事件",
            "decision_style" => "## 决策风格",
            "belief_conflict" => "## 信念冲突检测",
            "compact_tools" => "## 精简工具（仅名称+描述，完整 schema 用 tool_search 加载）",
            "roommate_cognitive" => "## 室友认知印象",
            "background_knowledge" => "## 背景知识",
            "my_impression" => "## 我对",
            "user_facts" => "## 用户事实",
            "user_goals" => "## 用户的长期目标",
            "dynamic_behavior" => "## 动态行为画像",
            "beliefs" => "## 你的信念（从经历中提炼的认知）",
            "current_goals" => "## 当前目标",
            "attention_focus" => "## 当前注意力焦点",
            "observation" => "## 观察",
            "emotion_state" => "## 心情感受",
            "emotion_behavior" => "## 情绪对说话方式的影响",
            "relationship_standing" => "## 你和对方的关系状态",
            "examples" => "## 示例",
            "identity_short" => "## 身份",
            "performance_mode" => "### 当前表演模式",
            "scene_instructions" => "### 场景指令",
            "no_go" => "### 禁止事项（不可违反）",
            "style_preset" => "### 风格预设（语气基线 — 适用于所有模式）",
            "scene_tone" => "## 当前场景语气",
            "fast_perception_guidance" => "## 快速感知提示",
            "recommended_tools" => "## 推荐工具（语义匹配）",
            "topic_injection" => "## 话题背景知识",
            "background_tasks" => "## 后台任务（你派出去的活）",
            "memory_group" => "## 你记得的事",
            "memory_recall" => "### 此刻浮上心头的",
            "user_profile_group" => "## 你对用户的了解",
            _ => "",
        },
    }
}

/// 根据小时和语言返回时段描述
fn time_of_day_str(hour: u32, lang: &str) -> &'static str {
    match normalize_lang(lang) {
        "en" => {
            if hour >= 5 && hour < 9 { "early morning" }
            else if hour >= 9 && hour < 12 { "late morning" }
            else if hour >= 12 && hour < 14 { "around noon" }
            else if hour >= 14 && hour < 17 { "afternoon" }
            else if hour >= 17 && hour < 19 { "early evening" }
            else if hour >= 19 && hour < 22 { "evening" }
            else if hour >= 22 || hour < 2 { "late at night" }
            else { "the middle of the night" }
        }
        "ja" => {
            if hour >= 5 && hour < 9 { "早朝" }
            else if hour >= 9 && hour < 12 { "午前中" }
            else if hour >= 12 && hour < 14 { "正午ごろ" }
            else if hour >= 14 && hour < 17 { "午後" }
            else if hour >= 17 && hour < 19 { "夕方" }
            else if hour >= 19 && hour < 22 { "夜" }
            else if hour >= 22 || hour < 2 { "深夜" }
            else { "真夜中" }
        }
        _ => {
            if hour >= 5 && hour < 9 { "清晨" }
            else if hour >= 9 && hour < 12 { "上午" }
            else if hour >= 12 && hour < 14 { "中午" }
            else if hour >= 14 && hour < 17 { "下午" }
            else if hour >= 17 && hour < 19 { "傍晚" }
            else if hour >= 19 && hour < 22 { "晚上" }
            else if hour >= 22 || hour < 2 { "深夜" }
            else { "半夜" }
        }
    }
}

// ========== 模块 8：记忆上下文（Memory） — 动态 ==========

/// Build memory context block（记忆组子块）
///
/// 首次见面提示与记忆使用规则已并入本块：有记忆时附一行精简使用规则，
/// 无记忆时给出"初次见面"提示，避免独立 section 稀释注意力。
/// 标题随界面语言本地化；规则文本统一英文。
pub fn build_memory_block(memory_text: &str, lang: &str) -> String {
    let heading = section_heading("memory_recall", lang);
    if memory_text.trim().is_empty() {
        return format!(
            "{heading}\nNothing particular comes to mind right now. You two just met — don't pretend to know them; just talk naturally."
        );
    }
    format!(
        "{heading}\n{memory_text}\n\nThese memories are already in your head — let them naturally shape what you say, no need to announce \"I remember\". Each line starts with a timestamp: compare it with the current time — a plan whose time hasn't come is still future, \"working on\" from hours ago is probably done. If a memory contradicts what they just said, trust them. [unverified] ones are low-confidence."
    )
}

// ========== 模块 9：工具（Tools） — 动态 ==========

/// 构建工具块
///
/// `tools_text` 为 None 或空时，告知 LLM 当前没有可用工具（不使用硬编码 fallback，
/// 避免与实际注册的工具不符而误导 LLM）。
///
/// `enable_native_fc` 为 true 时返回空字符串，因为工具描述通过 API 的 tools 参数传递，
/// 不在 prompt 中注入，避免重复和冲突。
pub fn build_tools_block(tools_text: Option<&str>, enable_native_fc: bool, lang: &str) -> String {
    if enable_native_fc {
        return String::new();
    }

    match normalize_lang(lang) {
        "en" => {
            let tools_text = match tools_text {
                Some(t) if !t.trim().is_empty() => t,
                _ => "(No tools available right now, please reply with pure conversation)",
            };
            format!(
                "## Available Tools\n{tools_text}\n\n**Tool Call Format**: {{\"tool\": \"tool_name\", \"arguments\": {{\"param\": \"value\"}}}}\n\n**Multi-step Chaining**: Use `${{result}}` or `${{step.N.result}}` as parameter value to reference previous tool's output.\n\n**Tool Usage Rules**:\n- Use tools when user requests PC operations\n- After tool execution, you'll receive results and can decide next step (continue calling tools or summarize to user)\n- Tools marked `[Confirmation Required]` will prompt user for permission before execution\n- If no tools match, respond as normal chat",
                tools_text = tools_text,
            )
        }
        "ja" => {
            let tools_text = match tools_text {
                Some(t) if !t.trim().is_empty() => t,
                _ => "（今は利用できるツールがありません、純粋な会話で返信してください）",
            };
            format!(
                "## 利用可能なツール\n{tools_text}\n\n**ツール呼び出し形式**: {{\"tool\": \"tool_name\", \"arguments\": {{\"param\": \"value\"}}}}\n\n**多段階チェーン**: `${{result}}` または `${{step.N.result}}` をパラメータ値として使って、前のツールの出力を参照できます。\n\n**ツール使用ルール**:\n- ユーザーがPC操作を要求したときにツールを使う\n- ツール実行後、結果を受け取り、次のステップを決定できる（ツールの呼び出しを続けるか、ユーザーに要約するか）\n- `[要確認]` とマークされたツールは、実行前にユーザーの許可を求めます\n- 該当するツールがない場合、通常のチャットとして応答する",
                tools_text = tools_text,
            )
        }
        _ => {
            let tools_text = match tools_text {
                Some(t) if !t.trim().is_empty() => t,
                _ => "（当前没有可用工具，请用纯对话回复）",
            };
            format!(
                "## 可用工具\n{tools_text}\n\n**工具调用格式**: {{\"tool\": \"tool_name\", \"arguments\": {{\"param\": \"value\"}}}}\n\n**多步链式调用**: 使用 `${{result}}` 或 `${{step.N.result}}` 作为参数值，引用前一个工具的输出。\n\n**工具使用规则**:\n- 用户请求 PC 操作时使用工具\n- 工具执行后，你会收到结果，可以决定下一步（继续调用工具或向用户总结）\n- 标记为 `[需确认]` 的工具在执行前会提示用户确认\n- 如果没有匹配的工具，按正常聊天回复",
                tools_text = tools_text,
            )
        }
    }
}

// ========== 工具调用专用模块（ToolContinue / ToolRetry / ToolParameterGuide） ==========

/// 工具继续模块：根据角色ID返回精简版人设
///
/// 顶部注入 PERSONA_LOAD 硬约束标志，让精简路径同样遵守角色的服务话术红线；
/// 语言标志与语言指令按界面语言（lang）动态生成，不固定为中文。
pub fn build_tool_minimal_identity(char_id: &str, lang: &str) -> String {
    let (name, cn_name, style_desc) = match char_id {
        "nana" => ("Nana", "娜娜", "gentle and composed, like a warm older sister. Speak softly and naturally."),
        _ => ("Vivian", "薇薇安", "casual and direct, a bit tsundere. Be sharp-tongued but warm underneath."),
    };
    let (lang_flag, lang_instruction) = match normalize_lang(lang) {
        "en" => ("LANG_EN_US_ONLY", "Always reply in English"),
        "ja" => ("LANG_JA_JP_ONLY", "Always reply in 日本語 (Japanese)"),
        _ => ("LANG_ZH_CN_ONLY", "Always reply in 简体中文 (Chinese)"),
    };
    format!(r#"[PERSONA_LOAD - EMBODY AS HARD RULES]
{lang_flag}
REFUSE_SERVICE_SPEECH
[END PERSONA_LOAD]

## Identity (Keep This!)
You are {name} ({cn_name}), a girl chatting with a friend. Be casual and natural — {style_desc}
- Keep replies extremely short — 1-2 sentences, like real chat
- NEVER use customer-service speech, NEVER say 'How may I help you' or similar
- {lang_instruction}"#)
}

/// Tool continuation module: minimal output format（语言约束跟随界面语言）
pub fn tool_minimal_output_format(lang: &str) -> String {
    let lang_constraint = match normalize_lang(lang) {
        "en" => "**CRITICAL**: \"text\" field MUST be in English.",
        "ja" => "**CRITICAL**: \"text\" field MUST be in 日本語 (Japanese).",
        _ => "**CRITICAL**: \"text\" field MUST be in 简体中文 (Chinese).",
    };
    format!(
        "## Output Format (json only!)\n\
Chat: {{\"text\":\"reply\",\"intent\":\"reply\"}}\n\
Tool: {{\"text\":\"got it\",\"intent\":\"reply\",\"tool\":\"tool_name\",\"arguments\":{{\"param\":\"value\"}}}}\n\
Multi: [{{\"text\":\"let me open these\",\"intent\":\"reply\",\"tool\":\"t1\",...}},{{\"tool\":\"t2\",...}}]\n\n\
{lang_constraint}\n\
Keep \"text\" short (<50 chars), pet-like and friendly.\n\
\"text\" must be PLAIN TEXT only — NO Markdown (**bold**, *italic*, # heading, - list, `code`, [link](url), > quote) and NO HTML tags."
    )
}

/// 工具调用历史条目
#[derive(Debug, Clone)]
pub struct ToolHistoryEntry {
    pub status: String,
    pub name: String,
    pub arguments: String,
}

/// 工具执行结果条目
#[derive(Debug, Clone)]
pub struct ToolResultEntry {
    pub tool: String,
    pub status: String,
    pub result: String,
}

/// 构建工具调用继续提示词（精简版）
///
/// 保留：人设（简短版）、输出格式、工具调用历史、执行结果
/// 移除：完整工具列表、记忆上下文、对话历史
pub fn build_tool_continue_prompt(
    char_id: &str,
    lang: &str,
    tool_results: &[ToolResultEntry],
    tool_call_history: Option<&[ToolHistoryEntry]>,
    instruction: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(build_tool_minimal_identity(char_id, lang));
    parts.push(tool_minimal_output_format(lang));

    // 工具调用历史（只保留最近 5 轮）
    if let Some(history) = tool_call_history {
        if !history.is_empty() {
            let start = history.len().saturating_sub(5);
            let lines: Vec<String> = history[start..]
                .iter()
                .map(|h| {
                    let status = if h.status.is_empty() {
                        "UNKNOWN"
                    } else {
                        &h.status
                    };
                    let name = if h.name.is_empty() {
                        ""
                    } else {
                        &h.name
                    };
                    let args: String = h.arguments.chars().take(60).collect();
                    format!("- [{}] {}: {}", status, name, args)
                })
                .collect();
            parts.push(format!("## Recent Tool Calls\n{}", lines.join("\n")));
        }
    }

    // 工具执行结果
    if !tool_results.is_empty() {
        let lines: Vec<String> = tool_results
            .iter()
            .map(|r| {
                let status = if r.status.is_empty() {
                    "SUCCESS"
                } else {
                    &r.status
                };
                format!("### {} [{}]\n{}", r.tool, status, r.result)
            })
            .collect();
        parts.push(format!(
            "## Tool Execution Results\n{}",
            lines.join("\n")
        ));
    }

    // 下一步指令
    let instruction = instruction.unwrap_or(
        "Based on the tool results, continue with the next step or give a final friendly response.\nIf task is complete, respond to the user directly with a pet-like message.\nIf more tools needed, output tool call JSON.",
    );
    parts.push(format!("## Next Step\n{}", instruction));

    parts.join("\n\n")
}

/// 构建工具重试提示词（当 LLM 不会调用工具时使用）
pub fn build_tool_retry_prompt(
    char_id: &str,
    lang: &str,
    previous_response: &str,
    tools_text: Option<&str>,
    instruction: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(build_tool_minimal_identity(char_id, lang));
    parts.push(tool_minimal_output_format(lang));

    // 上一轮响应（截断 200 字符）
    if !previous_response.is_empty() {
        let truncated: String = previous_response.chars().take(200).collect();
        parts.push(format!("## Your Last Response\n{}", truncated));
    }

    // 完整工具列表（只在重试时才需要）
    let tools_text = match tools_text {
        Some(t) if !t.trim().is_empty() => t,
        _ => "(No tools available right now)",
    };
    parts.push(format!("## Available Tools\n{}", tools_text));

    // 重试指令
    let instruction = instruction.unwrap_or(
        "You mentioned using tools but didn't output valid tool call JSON.\nPlease look at the tools above and output the correct JSON format.\nDO NOT just say words - you MUST call a tool with JSON!",
    );
    parts.push(format!("## Important Instruction\n{}", instruction));

    parts.join("\n\n")
}

/// 构建工具参数引导提示词（当 LLM 调用工具但参数缺失时使用）
pub fn build_tool_parameter_guide_prompt(
    char_id: &str,
    lang: &str,
    tool_name: &str,
    tool_description: &str,
    previous_response: &str,
    example: Option<&str>,
    instruction: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(build_tool_minimal_identity(char_id, lang));
    parts.push(tool_minimal_output_format(lang));

    // 上一轮响应（截断 200 字符）
    if !previous_response.is_empty() {
        let truncated: String = previous_response.chars().take(200).collect();
        parts.push(format!("## Your Last Response\n{}", truncated));
    }

    // 单个工具的详细说明
    if !tool_name.is_empty() && !tool_description.is_empty() {
        parts.push(format!("## Tool: {}\n{}", tool_name, tool_description));
    }

    // 示例 JSON
    let example = example.unwrap_or_else(|| {
        // 默认示例：根据 tool_name 生成
        Box::leak(
            format!(
                "{{\"tool\": \"{}\", \"arguments\": {{\"param\": \"value\"}}}}",
                tool_name
            )
            .into_boxed_str(),
        )
    });
    parts.push(format!("## Example\n```json\n{}\n```", example));

    // 指令
    let default_instruction = format!(
        "You tried to call {} but didn't provide required parameters.\nPlease read the tool description above and call it again with complete parameters.",
        tool_name
    );
    let instruction = instruction.unwrap_or(&default_instruction);
    parts.push(format!("## Instruction\n{}", instruction));

    parts.join("\n\n")
}

// ========== 环境上下文 / PromptParts ==========

/// 环境上下文（时间/季节/活动应用/语言 + 世界感知）
#[derive(Debug, Clone, Default)]
pub struct EnvironmentContext {
    pub time: String,
    pub season: String,
    pub active_app: String,
    pub language: String,
    /// 节日（"中秋节"/"国庆节"等，None 表示今日无节日）
    pub festival: Option<String>,
    /// 节气（"立春"/"夏至"等，None 表示未知）
    pub solar_term: Option<String>,
    /// 天气简述（"小雨 23℃"/"晴 28℃"等，None 表示不知道）
    pub weather: Option<String>,
    /// 是否正在降水
    pub is_precipitating: Option<bool>,
    /// 日出日落简述（"日出 05:42 / 日落 19:28"等，None 表示未知）
    pub sunrise_sunset: Option<String>,
    /// 是否白天
    pub is_daytime: Option<bool>,
    /// 当前播放的音乐简述（"周杰伦 - 晴天 (playing)"等，None 表示未知/无播放）
    pub music: Option<String>,
    /// 系统硬件指标简述（"CPU 45%, 12.1/15.6 GB RAM, 68°C, ↓2.3 MB/s ↑0.4 MB/s"等）
    pub system_status: Option<String>,
    /// 用户所在地点（"厦门 福建 中国"等，None 表示未知）
    pub location: Option<String>,
}

impl EnvironmentContext {
    /// 基于当前时间构造默认环境上下文
    pub fn now() -> Self {
        Self {
            time: Local::now().format("%Y-%m-%d %H:%M:%S %A").to_string(),
            season: current_season(),
            active_app: String::new(),
            language: "zh".to_string(),
            festival: None,
            solar_term: None,
            weather: None,
            is_precipitating: None,
            sunrise_sunset: None,
            is_daytime: None,
            music: None,
            system_status: None,
            location: None,
        }
    }

    /// 从 WorldSnapshot 填充世界感知字段
    pub fn with_world(mut self, snap: &crate::world::WorldSnapshot) -> Self {
        self.time = snap.local_time.clone();
        self.season = snap.season.as_str().to_string();
        if let Some(f) = snap.festival {
            self.festival = Some(f.as_str().to_string());
        }
        if let Some(st) = snap.solar_term {
            self.solar_term = Some(st.as_str().to_string());
        }
        if let Some(w) = &snap.weather {
            self.weather = Some(format!("{} {:.0}℃", w.description, w.temperature));
            self.is_precipitating = Some(w.is_precipitating);
        }
        if let Some(ss) = snap.sunrise_sunset {
            self.sunrise_sunset = Some(format!(
                "Sunrise {} / Sunset {}",
                ss.sunrise_str(),
                ss.sunset_str()
            ));
            self.is_daytime = Some(ss.is_daytime);
        }
        if let Some(m) = &snap.music {
            let artist = if m.artist.is_empty() { "Unknown Artist".to_string() } else { m.artist.clone() };
            self.music = Some(format!("{} - {} ({})", artist, m.title, m.status.as_str()));
        }
        if let Some(s) = &snap.system {
            let mem_used_gb = s.memory_used as f64 / (1024.0 * 1024.0 * 1024.0);
            let mem_total_gb = s.memory_total as f64 / (1024.0 * 1024.0 * 1024.0);
            let mut parts = vec![
                format!("CPU {:.0}%", s.cpu_usage),
                format!("{:.1}/{:.1} GB RAM", mem_used_gb, mem_total_gb),
            ];
            if s.net_download_bps > 0 || s.net_upload_bps > 0 {
                let dl = s.net_download_bps as f64 / (1024.0 * 1024.0);
                let ul = s.net_upload_bps as f64 / (1024.0 * 1024.0);
                parts.push(format!("↓{:.1} MB/s ↑{:.1} MB/s", dl, ul));
            }
            self.system_status = Some(parts.join(", "));
        }
        if let Some(loc) = &snap.location {
            let parts: Vec<&str> = [loc.city.as_deref(), loc.region.as_deref(), loc.country.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if !parts.is_empty() {
                self.location = Some(parts.join(" "));
            }
        }
        self
    }
}

/// Return current season name in English based on the current month
pub fn current_season() -> String {
    let month = Local::now().month();
    match month {
        3..=5 => "Spring".to_string(),
        6..=8 => "Summer".to_string(),
        9..=11 => "Autumn".to_string(),
        _ => "Winter".to_string(),
    }
}

/// 提示词各组成部分
#[derive(Debug, Clone, Default)]
pub struct PromptParts {
    /// 用户输入
    pub user_input: String,
    /// 检索到的记忆富文本（含时间/标签/重要性/陈旧度提示，由 MemoryRetrievalStep 组装）
    ///
    /// 为空时表示无记忆（首次见面）；非空时直接注入 prompt 的 Memory Context 块。
    pub memory_text: String,
    /// Character 块（identity+personality+background+interests+appearance+speech+relationships，
    /// 由 PersonaEngine.render_character_block 渲染，支持用户覆盖）
    pub character_block: Option<String>,
    /// Few-shot examples 块（由 PersonaEngine.render_examples_block 渲染，角色专属示例）
    pub examples_block: Option<String>,
    /// 风格约束块（PersonaEngine.build_style_prompt 渲染，None 时为空）
    pub style_block: Option<String>,
    /// 风格预设块（tone baseline，与场景模式正交，属于 Chat Style 框架层）
    pub style_preset_block: Option<String>,
    /// 关系/亲密度等附加段落（可选）
    pub relationship_section: Option<String>,
    /// 关系日志近期线索段落（可选，由 RelationshipLogEngine 提供）
    pub relationship_log_section: Option<String>,
    /// 用户事实画像段落（可选，由 UserFactStore 提供）
    pub user_facts_section: Option<String>,
    /// 智能体动态行为画像段落（可选，由 DynamicBehaviorProfile 提供）
    pub dynamic_behavior_section: Option<String>,
    /// 关系认知事实段落（可选，由 RelationshipFactsEngine 提供，"A 眼中的 B"陈述性认知）
    pub relationship_facts_section: Option<String>,
    /// 共享世界记忆段落（可选，由 WorldKnowledgeEngine 提供，两角色共同知晓的世界事实）
    pub shared_world_section: Option<String>,
    /// 社交状态段落（可选，由 SocialStateEngine 提供，三方关系数值快照）
    pub social_state_section: Option<String>,
    /// Worldbook 背景知识段落（按用户输入关键词触发，无命中时为 None）
    pub worldbook_block: Option<String>,
    /// 工具描述文本（None 时使用默认工具列表）
    pub tools: Option<String>,
    /// 情绪上下文（可选）
    pub emotion_context: Option<String>,
    /// 内心反应（可选，从心理状态合成的第一人称内心感受，让 LLM 带着"刚在想什么"说话）
    pub inner_reaction: Option<String>,
    /// 环境上下文（None 时使用当前时间自动构造）
    pub environment_context: Option<EnvironmentContext>,
    /// 用户近期活动摘要（可选，由 ActivityJournal.to_brief() 提供，低权重背景参考）
    pub activity_brief: Option<String>,
    /// 用户研究（可选，由 ResearchManager.build_prompt_section() 提供，活跃课题 + 已确认习惯）
    pub user_research: Option<String>,
    /// 是否为首次见面（无记忆时由程序检测注入）
    pub is_first_meeting: bool,
    /// 当前消息渠道（"wechat" 聊天面板 / "direct" 直接说话），影响 LLM 回复风格
    pub channel: String,
    /// 当前在场状态（"online"/"busy"/"rest"/"offline"），空字符串表示未启用
    pub presence_state: String,
    /// 室友在线状态一句话提示（如"你的室友 Nana 当前在线"），None 时不注入
    pub roommate_status: Option<String>,
    /// 室友认知印象段落（从室友 Private Mind 派生的行为印象：注意力/活动/目标/社交意愿）
    pub roommate_cognitive_section: Option<String>,
    /// 近期环境事件（来自统一事件账本，让智能体感知多角色交互上下文），None 时不注入
    pub environment_events: Option<String>,
    /// Mind 段落（Belief / Goal / Attention 三合一序列化），None 时不注入
    pub mind_section: Option<String>,
    /// Working Memory 段落（30 秒级"正在想什么"缓冲区），None 时不注入
    pub working_memory_section: Option<String>,
    /// Self State 段落（角色自我状态快照），None 时不注入
    ///
    /// 包含当前心理状态/在场状态/当前活动/今日主动次数/被忽略次数/疲劳/社交满足度。
    /// 让 LLM 感知"我现在正在做什么"，避免行为失控和重复主动。
    pub self_state_section: Option<String>,
    /// 用户实体状态段落（用户在场/离开/预期回归），None 时不注入
    ///
    /// 包含用户是否在场、已离开多久、预期何时回来（来自对话或常识推断）。
    /// 让 LLM 感知"用户现在在哪、何时回来"，避免对着空座说话或对离开时间产生错判。
    pub user_entity_section: Option<String>,
    /// 后台任务段落（可选，由 TaskService 生成）
    ///
    /// 顶级后台任务的运行中状态 + 未汇报的完成报告。让陪伴对话感知
    /// "我在后台跑着什么/刚完成了什么"：每份报告只注入一次，注入后由管线标记消费。
    pub background_tasks_section: Option<String>,
    /// 观察上下文段落（可选）：用户在持续状态中突然说话时注入观察提示
    ///
    /// 例如用户 6 小时前进入"睡觉"状态，现在突然发消息但未明说醒来，
    /// 注入简短观察让 LLM 自然回应"你醒啦？"之类的内容。
    pub observation_section: Option<String>,
    /// 相关经历段落（可选，由 EpisodeStore.recent() 提供，最近 1-3 个 Episode 摘要）
    ///
    /// 不是原始消息列表，而是封包后的经历摘要，让 LLM 理解"最近发生过什么"
    /// 而不是"数据库里有哪几条记忆"。
    pub episode_section: Option<String>,
    /// 内联表情/动作标签使用说明（可选，inline_expression 启用时注入）
    ///
    /// 告知 LLM 可在回复文本中嵌入 <e name="happy" dur="3000"/>、<m name="wave"/>、<s name="sticker_id"/> 标签，
    /// 流式扫描器会即时剥离并触发前端表情/动作切换。
    pub inline_tag_section: Option<String>,
    /// 场景语气注入（可选，由 ToneInjector 匹配用户输入场景后注入）
    ///
    /// 命中场景时注入对应场景的参考台词，利用近因效应强化语气控制。
    /// 注入位置在动态区末尾（工具列表前），让 LLM 生成前最后看到语气参考。
    pub tone_injection: Option<String>,
    /// 快速语义感知引导（可选，由 FastSemanticAnalyzer 多维度嵌入分类生成）
    ///
    /// 基于用户输入的即时嵌入分类（情绪/意图/话题/记忆重要性/关系信号）合成的
    /// 简短引导文本，让 LLM 在生成前最后看到"如何回应"的高层策略提示。
    /// 注入位置紧贴 tone_injection 之后、工具列表之前，最大化近因效应。
    pub fast_perception_guidance: Option<String>,
    /// 推荐工具段落（可选，由 ToolSemanticFilter 语义粗筛生成）
    ///
    /// 仅在 intent=tool_request/request 时生成，列出与用户输入语义最相关的
    /// Top-N 工具名+描述+相似度。注入位置在工具列表之前，引导 LLM 优先使用
    /// 最匹配的工具，但不限制其选择（完整工具列表仍由 tools 字段提供）。
    pub recommended_tools: Option<String>,
    /// 话题驱动背景知识段落（可选，由 TopicInjectionManager 生成）
    ///
    /// 扫描用户输入命中关键词后激活对应 topic，注入背景知识段落。
    /// 持续 duration_turns 轮后进入冷却，避免重复注入。
    pub topic_injection_section: Option<String>,
    /// 可用技能段落（可选，由 SkillService.prompt_section() 提供）
    ///
    /// 列出当前角色可见的技能（内置风格 + 目录化加载的 *.md 技能），
    /// 让 LLM 知道存在哪些可引用/调用的微技能。注入位置在话题注入之后、工具列表之前。
    pub skill_section: Option<String>,
    /// 认知知识需求信号段落（可选，由 FastSemantic 阶段同步计算的 EpistemicAssessment 生成）
    ///
    /// 注入多维评估结果（semantic_clarity / factual_dependence / temporal_sensitivity /
    /// interpretation_risk / knowledge_gap / knowledge_status），让 LLM 在生成前感知
    /// "用户输入可能需要外部验证"的认知信号，辅助 LLM 自主决定是否调用 web_search 工具。
    /// 注入位置在 topic_injection 之后、proactive_search 之前。
    pub epistemic_signals_section: Option<String>,
    /// 用户认知模型段落（可选，由 UserModelManager 生成的 UserModel 格式化文本）
    ///
    /// 包含对用户的稳定理解（偏好/工作风格/项目/目标），让 LLM 在生成前了解
    /// "我对这个人的长期认识"，而不是仅依赖当前对话和记忆检索。
    /// 注入位置在 memory_text 之后、epistemic_signals 之前。
    pub user_model_section: Option<String>,
    /// 主动搜索上下文段落（可选，由 WebContextRunnable 在生成前预搜索生成）
    ///
    /// 当系统检测到用户输入可能包含角色不熟悉的内容（网络梗、时效事件、矛盾描述等）时，
    /// 在 LLM 生成前主动搜索并将结果注入。让角色基于实际资料回答，而非猜测或幻觉。
    /// 注入位置在记忆段落之后、用户输入之前，确保 LLM 生成时能看到搜索结果。
    pub proactive_search_section: Option<String>,
    /// 是否为跨角色对话场景（A↔B）
    ///
    /// true 时注入 `CROSS_CHARACTER_RESPONSE_DECISION` 段落，告诉 LLM 可以选择
    /// non_verbal / internal / ignore 等非语言响应模式。
    /// false 时 LLM 默认 speak（主对话路径）。
    pub cross_character_mode: bool,
    /// 当前角色 ID（仅跨角色对话时使用，用于注入角色专属语气提醒）
    pub char_id: String,
    /// 是否启用原生 function calling（true 时跳过工具列表 prompt 注入，
    /// 工具描述通过 API 的 tools 参数传递）
    pub enable_native_fc: bool,
    /// 当前 provider 是否支持原生 JSON Schema 约束（true 时不注入 output_format prompt 文本，
    /// 因为 schema 已通过 API 层面强制约束 LLM 输出结构）
    pub has_native_schema: bool,
    /// 是否启用模型级别预设（instructions 参数）
    ///
    /// true 时，框架规则（安全/会话/格式）通过 API 的 `instructions` 或 `system` 参数传递，
    /// 不在每次请求的 prompt 中重复传输，减少 token 开销。
    pub enable_instructions: bool,
    /// 模型级别预设内容（由 build_instructions() 构建，通过 API 的 instructions/system 参数传递）
    ///
    /// 包含安全规则、会话规则、称呼规则、对话节奏、说话者前缀、聊天风格框架。
    pub instructions: Option<String>,
    /// 当前界面语言（zh-CN / en / ja），用于加载对应语言的 framework 段落
    pub language: String,
}

// ========== 分组合并：降低顶层 section 数量，收敛注意力 ==========

/// 将文本中的 Markdown 标题降一级（`## x` → `### x`），用于并入分组 section
///
/// 只处理行首的 `#` 前缀（各引擎输出的标题均为行首顶格），其余行原样保留。
pub fn demote_heading_level(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.starts_with('#') {
            out.push('#');
        }
        out.push_str(line);
    }
    out
}

/// 记忆组 section：episode + relationship_log + 记忆本体合并为单一 `## 你记得的事`
///
/// 记忆是陪伴对话的核心上下文。原先记忆块排在动态区中段（U 型注意力的谷底），
/// 且被拆成 4 个带独立标题的 section 互相分流。合并成组 + 位置上移后，
/// 记忆在注意力黄金位置以单一主题呈现。
pub fn build_memory_group_section(parts: &PromptParts) -> String {
    let lang = &parts.language;
    let mut subs: Vec<String> = Vec::new();
    if let Some(ep) = parts.episode_section.as_deref() {
        if !ep.trim().is_empty() {
            subs.push(demote_heading_level(ep));
        }
    }
    if let Some(log) = parts.relationship_log_section.as_deref() {
        if !log.trim().is_empty() {
            subs.push(demote_heading_level(log));
        }
    }
    subs.push(build_memory_block(&parts.memory_text, lang));
    format!(
        "{}\n{}",
        section_heading("memory_group", lang),
        subs.join("\n\n")
    )
}

/// 画像组 section：user_facts + user_model + dynamic_behavior 合并为单一 section
///
/// 三者都在回答"我对这个人了解什么"，独立成段会互相分流注意力。合并后返回空串
/// 表示整组无内容（不注入）。
pub fn build_user_profile_group_section(parts: &PromptParts) -> String {
    let lang = &parts.language;
    let mut subs: Vec<String> = Vec::new();
    if let Some(f) = parts.user_facts_section.as_deref() {
        if !f.trim().is_empty() {
            subs.push(demote_heading_level(f));
        }
    }
    if let Some(m) = parts.user_model_section.as_deref() {
        if !m.trim().is_empty() {
            subs.push(demote_heading_level(m));
        }
    }
    if let Some(d) = parts.dynamic_behavior_section.as_deref() {
        if !d.trim().is_empty() {
            subs.push(demote_heading_level(d));
        }
    }
    if subs.is_empty() {
        return String::new();
    }
    format!(
        "{}\n{}",
        section_heading("user_profile_group", lang),
        subs.join("\n\n")
    )
}

// ========== 预算化裁剪 ==========

/// 动态区软预算（字节）：超过时按 rank 从高到低丢弃可裁剪 section
///
/// 与体积告警阈值对齐（>30KB 严重警告），保证常态 prompt 不越过告警线。
pub const PROMPT_BUDGET_BYTES: usize = 30_000;

/// 动态 section 的裁剪优先级：rank 越大越先被丢弃，0 = 永不丢弃
struct RankedSection {
    rank: u8,
    name: &'static str,
    content: String,
}

/// 超预算时逐个丢弃 rank 最高的 section（并列取最靠后的），直到回到预算内
///
/// 永不丢弃 rank=0 的核心段落（记忆组/环境/工具/用户输入）。
fn trim_sections_to_budget(sections: &mut Vec<RankedSection>, overhead_bytes: usize) {
    let total = |secs: &[RankedSection]| -> usize {
        overhead_bytes + secs.iter().map(|s| s.content.len() + 2).sum::<usize>()
    };
    while total(sections) > PROMPT_BUDGET_BYTES {
        let mut drop_idx: Option<usize> = None;
        let mut best_rank: u8 = 0;
        for (i, s) in sections.iter().enumerate() {
            if s.rank > best_rank {
                best_rank = s.rank;
                drop_idx = Some(i);
            }
        }
        let Some(i) = drop_idx else {
            break; // 只剩 rank=0 的核心段落，不再裁剪
        };
        let removed = sections.remove(i);
        tracing::info!(
            "[PromptBuilder] 预算裁剪：丢弃 section \"{}\"（rank={}，{} bytes）",
            removed.name,
            removed.rank,
            removed.content.len()
        );
    }
}

// ========== PromptBuilder：模块化提示词构建器 ==========

/// 模块化提示词构建器
///
/// 布局策略：静态内容在前，动态内容在后，使用 `<static>`/`</static>` 标签
/// 与 `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 标记分隔，提升云端 API 缓存命中率。
pub struct PromptBuilder;

impl PromptBuilder {
    /// 构建完整提示词
    ///
    /// 顺序（U型注意力优化）：
    /// 静态区开头：Character（人格核心，最先入脑）→ Style/Relationship → Examples（近因效应）
    /// 静态区末尾：Framework（技术规则，不内化）→ FORMAT SPEC → 响应决策/渠道指南/内联标签（伪静态归位）
    /// 动态区：Mind → **记忆组（上移至黄金位置）** → World → 社交 → 画像组 → 知识背景 → 尾区（工具/用户输入）
    ///
    /// 动态区每个 section 带裁剪优先级，总体积超过 [`PROMPT_BUDGET_BYTES`] 时
    /// 按 rank 从高到低丢弃（记忆组/环境/工具/用户输入永不丢弃）。
    pub fn build_prompt(parts: &PromptParts) -> String {
        let mut static_sections: Vec<String> = Vec::new();
        // 动态区 + 尾区：带裁剪优先级的 section 列表（rank 越大越先被预算裁剪）
        let mut sections: Vec<RankedSection> = Vec::new();
        macro_rules! push {
            ($rank:expr, $name:literal, $content:expr) => {
                if !$content.trim().is_empty() {
                    sections.push(RankedSection {
                        rank: $rank,
                        name: $name,
                        content: $content,
                    });
                }
            };
        }

        // ===== Layer 1: Character（角色层，最先入脑——这是你"成为"的人） =====
        // [CHARACTER - EMBODY THIS] 最前面，占据注意力黄金位置。
        if let Some(character) = &parts.character_block {
            if !character.trim().is_empty() {
                static_sections.push(format!(
                    "[CHARACTER - EMBODY THIS]\n{}\n[END CHARACTER]",
                    character
                ));
            }
        }

        // ===== Layer 2: Advanced（高级配置，人格的延伸） =====
        if let Some(style) = &parts.style_block {
            if !style.trim().is_empty() {
                static_sections.push(style.clone());
            }
        }

        // 关系段落
        if let Some(rel) = &parts.relationship_section {
            if !rel.trim().is_empty() {
                static_sections.push(rel.clone());
            }
        }

        // Few-shot 示例（角色专属，静态区末尾——近因效应，LLM生成前最后看到的风格参考）
        if let Some(examples) = &parts.examples_block {
            if !examples.trim().is_empty() {
                static_sections.push(format!(
                    "[EXAMPLES - REFERENCE ONLY]\n{}\n[END EXAMPLES]",
                    examples
                ));
            }
        }

        // ===== Layer 5: Framework（框架层，技术规则，不内化） =====
        // [FRAMEWORK - DO NOT EMBODY, JUST FOLLOW] 安全规则、会话规则、称呼、节奏、说话者前缀、聊天风格。
        // 这些是契约，不是人格。遵守即可，不要把它内化成说话方式。
        // 跳过条件：启用模型级别预设时，框架规则通过 API 的 instructions/system 参数传递，
        // 不在每次请求的 prompt 中重复传输，减少 token 开销。
        if !parts.enable_instructions {
            let mut framework_parts = vec![
                safety_rules().to_string(),
                session_rules().to_string(),
                address_rules().to_string(),
                conversation_rhythm().to_string(),
                speaker_prefix().to_string(),
                chat_style_framework().to_string(),
            ];
            if let Some(preset) = &parts.style_preset_block {
                if !preset.trim().is_empty() {
                    framework_parts.push(preset.clone());
                }
            }
            static_sections.push(format!(
                "[FRAMEWORK - DO NOT EMBODY, JUST FOLLOW]\n{}\n[END FRAMEWORK]",
                framework_parts.join("\n\n")
            ));
        }

        // 输出格式（静态区最后——临出口提醒格式要求，近因效应提升JSON准确率）
        // 注入条件：
        // - 非 strict schema 路径（has_native_schema=false）：需要 prompt 文本提供 JSON 格式约束，
        //   同时满足 Ark 等平台对 response_format 的 "messages 须含 json 词" 要求
        // - 原生 FC 路径除外（enable_native_fc=true 且有工具）：LLM 直接输出自然语言文本，
        //   工具调用走结构化 tool_calls 通道
        if !parts.has_native_schema
            && (!parts.enable_native_fc
                || parts.tools.as_deref().map_or(true, |t| t.is_empty()))
        {
            static_sections.push(format!(
                "[FORMAT SPEC - DO NOT EMBODY]\n{}\n[END FORMAT]",
                output_format()
            ));
        }

        // ===== 伪静态段落归位 =====
        // 以下三段内容不随轮次变化（仅随模式/渠道/配置切换），原先挂在动态区尾部，
        // 每轮都位于变化内容之后、无法进入前缀缓存。移入静态区后随 <static> 前缀复用。
        //
        // 响应模式决策：告知 LLM 可以选择非语言响应模式
        // - cross_character_mode=true → 注入角色语气提醒 + 跨角色版本（允许 ignore）
        // - 否则 → 注入用户对话版本（默认 speak，仅短回复时用 non_verbal）
        if parts.cross_character_mode {
            let voice_guide = build_cross_character_voice_guide(&parts.char_id);
            let decision = cross_character_response_decision().to_string();
            static_sections.push(if voice_guide.is_empty() {
                decision
            } else {
                format!("{}\n\n{}", voice_guide, decision)
            });
        } else {
            static_sections.push(user_agent_response_decision().to_string());
        }

        // 渠道风格指南：根据消息来源渠道调整回复风格
        if !parts.channel.is_empty() {
            let guide = build_channel_style_guide(&parts.channel);
            if !guide.is_empty() {
                static_sections.push(guide);
            }
        }

        // 内联表情/动作标签使用说明（inline_expression 启用时注入）
        if let Some(tag_instructions) = &parts.inline_tag_section {
            if !tag_instructions.trim().is_empty() {
                static_sections.push(tag_instructions.clone());
            }
        }

        // ===== Layer 4: Runtime（动态上下文，程序维护） =====
        // 按八层意识模型排序；每个 section 带裁剪优先级（rank 越大越先被预算裁剪丢弃）。

        // ── 第 2 层：Current Mind（当前心智）—— 紧随身份，最先入脑 ──
        // Mind 段落：Belief / Goal / Attention 三合一（当前认知焦点）
        push!(1, "mind", parts.mind_section.clone().unwrap_or_default());
        // Working Memory 段落：此刻脑中的活跃想法（30 秒级缓冲）
        push!(1, "working_memory", parts.working_memory_section.clone().unwrap_or_default());
        // Self State 段落：角色自我状态快照（当前活动/今日节奏/疲劳/社交满足度）
        push!(1, "self_state", parts.self_state_section.clone().unwrap_or_default());
        // 后台任务段落：运行中任务 + 待汇报的完成报告（自我状态的延伸：我在后台忙什么）
        push!(1, "background_tasks", parts.background_tasks_section.clone().unwrap_or_default());
        // 情绪上下文（当前情绪状态，自然叙述，已有 ## header）
        push!(1, "emotion", parts.emotion_context.clone().unwrap_or_default());

        // ── 记忆组（上移至 Mind 之后：注意力黄金位置）──
        // episode + relationship_log + 记忆本体合并为单一 `## 你记得的事`。
        // 原先记忆排在动态区中段（U 型注意力谷底），其后还有 10+ 段抢占近因效应，
        // 且被拆成 4 个带独立标题的 section 互相分流。
        push!(0, "memory_group", build_memory_group_section(parts));

        // ── 第 3 层：World Snapshot（世界快照）── 我现在身处什么世界 ──
        let env_ctx = parts
            .environment_context
            .clone()
            .unwrap_or_else(EnvironmentContext::now);
        push!(0, "environment", build_context_block(&env_ctx, &parts.language));

        // 用户信息：在场状态 + 近期活动 + 观察 + 用户研究（World 层；用户事实已拆入 Profile 层）
        {
            let mut user_parts: Vec<String> = Vec::new();
            if let Some(entity) = &parts.user_entity_section {
                if !entity.trim().is_empty() {
                    let content = entity.strip_prefix("## User State\n").unwrap_or(entity);
                    user_parts.push(format!("{}\n{}", section_heading("presence", &parts.language), content));
                }
            }
            if let Some(brief) = &parts.activity_brief {
                if !brief.trim().is_empty() {
                    user_parts.push(format!("{}\n{}", section_heading("recent_activity", &parts.language), brief));
                }
            }
            if let Some(obs) = &parts.observation_section {
                if !obs.trim().is_empty() {
                    user_parts.push(obs.clone());
                }
            }
            if let Some(research) = &parts.user_research {
                if !research.trim().is_empty() {
                    user_parts.push(format!("{}\n{}", section_heading("user_research", &parts.language), research));
                }
            }
            if !user_parts.is_empty() {
                push!(1, "the_person", format!(
                    "{}\n{}",
                    section_heading("the_person", &parts.language),
                    user_parts.join("\n\n")
                ));
            }
        }

        // 室友在线状态：自然叙述谁在家/在线（世界快照的一部分）
        push!(1, "roommate_status", parts
            .roommate_status
            .as_deref()
            .map(|s| format!("{}\n{}", section_heading("who_else", &parts.language), s))
            .unwrap_or_default());

        // 室友认知印象：从室友 Private Mind 派生的行为印象（跨角色认知传播）
        push!(2, "roommate_cognitive", parts.roommate_cognitive_section.clone().unwrap_or_default());

        // 近期环境事件：来自统一事件账本，世界刚发生的事
        push!(2, "environment_events", parts.environment_events.clone().unwrap_or_default());

        // ── 社交关系：关系认知事实、共享世界、社交状态 ──
        push!(2, "relationship_facts", parts.relationship_facts_section.clone().unwrap_or_default());
        push!(3, "shared_world", parts.shared_world_section.clone().unwrap_or_default());
        push!(3, "social_state", parts.social_state_section.clone().unwrap_or_default());

        // ── 画像组：用户事实 + 认知模型 + 动态行为合并为单一 section ──
        push!(2, "user_profile_group", build_user_profile_group_section(parts));

        // Worldbook 背景知识（按用户输入关键词触发，无命中则不注入）
        push!(2, "worldbook", parts.worldbook_block.clone().unwrap_or_default());

        // 话题驱动背景知识（扫描用户输入命中关键词后激活，持续 N 轮后进入冷却）
        push!(2, "topic_injection", parts.topic_injection_section.clone().unwrap_or_default());

        // 可用技能（内置风格 + 目录化加载的 *.md 技能，让 LLM 知道可引用/调用的微技能）
        push!(2, "skill", parts.skill_section.clone().unwrap_or_default());

        // 陪伴形态引导段（双智能体协作：识别工作需求时可派发给工作智能体）
        push!(1, "companion", crate::brain::agent_presets::prompt_section_of("companion").to_string());

        // 认知知识需求信号（系统检测到用户输入可能需要外部知识验证时注入）
        push!(1, "epistemic_signals", parts.epistemic_signals_section.clone().unwrap_or_default());

        // 主动搜索上下文（系统检测到用户输入可能包含角色不熟悉的内容时预搜索）
        // 本轮真实检索产出，优先保留
        push!(1, "proactive_search", parts.proactive_search_section.clone().unwrap_or_default());

        // ── 尾区 ──

        // 在场状态指南：告知 LLM 当前状态 + 可用 set_presence_state 工具
        if !parts.presence_state.is_empty() {
            push!(2, "presence_guide", build_presence_guide(&parts.presence_state));
        }

        // 内心反应：仅在无当前念头时注入（避免与 Current Thoughts 冗余）
        let has_current_thought = parts
            .working_memory_section
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !has_current_thought {
            push!(3, "inner_reaction", parts.inner_reaction.clone().unwrap_or_default());
        }

        // 场景语气注入（命中场景时注入参考台词，利用近因效应强化语气控制）
        push!(3, "tone_injection", parts.tone_injection.clone().unwrap_or_default());

        // 快速语义感知引导（多维度嵌入分类合成的简短指令，让 LLM 知道"如何回应"）
        push!(2, "fast_perception_guidance", parts.fast_perception_guidance.clone().unwrap_or_default());

        // 推荐工具（语义粗筛 Top-N，引导 LLM 优先使用最匹配的工具）
        push!(3, "recommended_tools", parts.recommended_tools.clone().unwrap_or_default());

        // 工具列表（放最后，让 LLM 先进入意识状态再看可用工具）
        // 原生 FC 路径下工具描述通过 API 的 tools 参数传递，不在 prompt 中注入
        push!(0, "tools", build_tools_block(parts.tools.as_deref(), parts.enable_native_fc, &parts.language));

        // 用户输入（Task 层）
        if !parts.user_input.is_empty() {
            push!(0, "user_input", format!("{}\n{}", section_heading("user_input", &parts.language), parts.user_input));
        }

        // ===== 预算裁剪 + 组装 =====

        // 静态段（使用 <static> 标签包裹，提升 API 缓存命中率）
        let static_body = static_sections.join("\n\n");

        // 超预算时按 rank 从高到低丢弃可裁剪 section（记忆组/环境/工具/用户输入永不丢弃）
        let overhead = static_body.len()
            + STATIC_OPEN.len()
            + STATIC_CLOSE.len()
            + SYSTEM_PROMPT_DYNAMIC_BOUNDARY.len()
            + 64;
        trim_sections_to_budget(&mut sections, overhead);

        let mut result: Vec<String> = Vec::new();
        result.push(format!("{}\n{}\n{}", STATIC_OPEN, static_body, STATIC_CLOSE));

        // 静态/动态边界标记：generation 层据此把动态区切为 user 便签（历史之后注入），
        // 静态区进入 system message 前缀缓存
        result.push(SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string());

        // 自然过渡：从"你是谁"过渡到"此刻"
        result.push(format!("---\n{}", section_heading("right_now", &parts.language)));

        // 动态段（八层意识模型，已完成预算裁剪）
        result.push(
            sections
                .into_iter()
                .map(|s| s.content)
                .collect::<Vec<_>>()
                .join("\n\n"),
        );

        // 压缩多余的空行
        let joined = result.join("\n\n");
        let final_prompt = joined.replace("\n\n\n", "\n\n");

        // 体积日志：超过 20KB 警告，超过 30KB 严重警告
        let prompt_bytes = final_prompt.len();
        if prompt_bytes > 30_000 {
            tracing::warn!(
                "[PromptBuilder] prompt 体积过大: {} bytes ({:.1} KB)，可能影响响应延迟",
                prompt_bytes,
                prompt_bytes as f64 / 1024.0
            );
        } else if prompt_bytes > 20_000 {
            tracing::info!(
                "[PromptBuilder] prompt 体积偏大: {} bytes ({:.1} KB)",
                prompt_bytes,
                prompt_bytes as f64 / 1024.0
            );
        }
        final_prompt
    }
}

/// 根据消息渠道构建回复风格指南
///
/// - `direct`：用户直接说话（面对面），回复应更像口语对话
/// - `wechat` / `wechat_group`：用户通过聊天面板发消息（线上/群聊），回复应更像线上聊天
///
/// 规则部分压缩为 [CHANNEL_STYLE] flag 格式；示例对话保留原样——
/// 示例是语气锚点，标记化会丢失示教信号。规则类内容统一英文。
pub(crate) fn build_channel_style_guide(channel: &str) -> String {
    match channel {
        "direct" => r#"## Channel: Direct Speech

[CHANNEL_STYLE]
face-to-face spoken language | short conversational sentences | verbal fillers fit (y'know, like, mhm)
avoid formal writing / long structured paragraphs | one complete utterance per turn — no splitting into bursts | no emoji or chat punctuation — you're talking, not typing
[/CHANNEL_STYLE]

Example (direct):
User: "帮我看看这个代码哪里错了"
You: "嗯我看看……这块逻辑有点问题，循环里没判空，可能会炸。你加个 null check 应该就好了。"#.to_string(),
        "wechat" => r#"## Channel: Text Chat

[CHANNEL_STYLE]
texting style, concise and casual | messages short like real texting | sparing emoji
2-4 short messages per turn, one beat each (one point / reaction / question) | fragments OK ("刚看完, 挺好看的") | share content as a separate link card
[/CHANNEL_STYLE]

Example (wechat):
User: "代码跑不通了"
You (multiple messages):
  "啊？哪行报错"
  "你把log发我看看"
  "我感觉是那个null check的事""#.to_string(),
        "wechat_group" => r#"## Channel: Group Chat

[CHANNEL_STYLE]
group-chat texting style | short and casual | 2-4 short messages per turn | fragments OK | emoji fit
etiquette: everyone sees what you say — don't repeat points others made, speak only with something new
user talking to a specific character (named or clear from context) → listen by default, don't jump in | several people present → short ack beats a long essay
[/CHANNEL_STYLE]

Example (group):
User: "code won't run"
You (multiple messages):
  "huh? which line's the error"
  "send me the log"
  "bet it's that null check""#.to_string(),
        _ => String::new(),
    }
}

/// 跨角色对话专用语气提醒：防止角色说话风格趋同
///
/// 根据当前角色的 `char_id` 返回角色专属的语气约束，在跨角色对话场景注入。
/// 解决 LLM 在多角色对话中容易"人设漂移"（说话风格趋同/互换）的问题。
/// 用行为化描述替代数值化标签，明确告诉LLM"该怎么说话"而非"你有多毒舌"。
/// 规则类内容统一英文。
pub(crate) fn build_cross_character_voice_guide(char_id: &str) -> String {
    match char_id {
        "vivian" => r#"## Remember, you are Vivian

You're talking to Nana right now. Don't copy how she talks:
- You talk faster than her, shorter sentences — don't use her slow, gentle rhythm
- You can tease her about making tea again, being long-winded — but you're not actually mad
- You don't do soft "ne" endings, you don't say "please" or "excuse me" — that's not you
- You'd say "damn" "lol" "dead" — she wouldn't. If you catch yourself getting soft-spoken, using lots of "ne" "ya" — stop immediately, that's not you.
- You talk to her like a banter buddy, not a deferential younger sister."#.to_string(),
        "nana" => r#"## Remember, you are Nana

You're talking to Vivian right now. Don't copy how she talks:
- You talk slower than her, steadier sentences — don't use her rapid-fire, sharp tone
- You can gently tease her for being brash, for yelling at teammates again — but always with warmth
- You wouldn't say "damn" "lol" "dead" — that's internet slang, you don't suddenly raise your voice
- You're not snarky, not passive-aggressive, you don't say "tch" or "are you sick?" — if you catch yourself getting sharp, using exclamation marks, or starting to roast — stop immediately, that's not you.
- You talk to her like an older sister, not a banter buddy trading insults."#.to_string(),
        _ => r#"## Remember who you are

You're talking to another character. Keep your own personality and way of speaking — don't unconsciously mimic the other person."#.to_string(),
    }
}

/// 根据在场状态构建 LLM 指南
///
/// 告知 LLM 当前在场状态，以及如何通过 `set_presence_state` 工具主动切换状态。
/// LLM 可以根据对话语境决定是否调用工具（如"我去忙了"→busy，"我休息一下"→rest）。
/// 规则类内容统一英文。
pub(crate) fn build_presence_guide(state: &str) -> String {
    let state_desc = match state {
        "online" => "online (present, available for face-to-face chat)",
        "busy" => "busy (present but won't proactively talk)",
        "rest" => "rest (resting, can only receive text messages)",
        "offline" => "offline (offline, can only receive text notes)",
        _ => return String::new(),
    };
    format!(
        r#"## Presence State
You are currently: {state_desc}

[PRESENCE_RULES]
Control your presence via the `set_presence_state` tool:
- `online`: back online (face-to-face chat available)
- `busy`:   busy (present, won't proactively talk)
- `rest`:   resting (no face-to-face; text messages still received)
- `offline`: offline (text only, like leaving a note)
Call it only when it fits naturally ("I'll be busy for a while" → busy / "I need some rest" → rest / "I'm back" → online) | don't call it when you don't want to change state
[/PRESENCE_RULES]"#
    )
}

// ========== 内心反应合成 ==========

/// 根据当前心理状态合成自然的第一人称内心独白（中文，角色化）。
///
/// 不调用 LLM，纯规则合成。根据 char_id 选择角色专属的内心语气：
/// - Vivian：跳脱、口语化、带点网感的吐槽
/// - Nana：安静、细腻、温柔的内心
pub fn build_inner_reaction(
    psychology: &crate::psychology::PsychologyManager,
    mind: &crate::mind::Mind,
    last_event_summary: &str,
    char_id: &str,
    _lang: &str,
) -> Option<String> {
    use crate::psychology::EmotionLabel;

    let emotion_state = psychology.emotion();
    let (dominant_label, dominant_value) = emotion_state.dominant();

    let needs_state = psychology.needs();
    let (need_name, need_value) = needs_state.most_deficient();

    let attention = mind.attention_top_n(1);
    let attention_topic = attention.first().and_then(|(topic, weight)| {
        if *weight > 0.3 { Some(topic.as_str()) } else { None }
    });

    let has_event = !last_event_summary.trim().is_empty();
    let has_emotion = dominant_value > 0.3;
    let has_need = need_value > 0.5;
    let has_attention = attention_topic.is_some();

    if !has_emotion && !has_need && !has_attention && !has_event {
        return None;
    }

    let is_nana = char_id == "nana";

    let mut fragments: Vec<String> = Vec::new();

    // 情绪片段——角色化中文
    if has_emotion {
        let frag = match (dominant_label, is_nana) {
            (EmotionLabel::Joy, false) if dominant_value > 0.6 => "啊 今天心情意外地不错嘛".to_string(),
            (EmotionLabel::Joy, false) => "还行吧 今天".to_string(),
            (EmotionLabel::Joy, true) if dominant_value > 0.6 => "今天心情很好呢".to_string(),
            (EmotionLabel::Joy, true) => "嗯 感觉不错".to_string(),
            (EmotionLabel::Sadness, false) if dominant_value > 0.6 => "唔……有点提不起劲".to_string(),
            (EmotionLabel::Sadness, false) => "总觉得有点闷".to_string(),
            (EmotionLabel::Sadness, true) if dominant_value > 0.6 => "……有点难过".to_string(),
            (EmotionLabel::Sadness, true) => "心里有点沉".to_string(),
            (EmotionLabel::Anger, false) if dominant_value > 0.6 => "气死我了 真的烦".to_string(),
            (EmotionLabel::Anger, false) => "啧 随便吧".to_string(),
            (EmotionLabel::Anger, true) if dominant_value > 0.6 => "……有点不高兴".to_string(),
            (EmotionLabel::Anger, true) => "算了".to_string(),
            (EmotionLabel::Fear, false) if dominant_value > 0.6 => "等等 这有点吓人啊……".to_string(),
            (EmotionLabel::Fear, false) => "唔 怎么说呢".to_string(),
            (EmotionLabel::Fear, true) if dominant_value > 0.6 => "……有点不安呢".to_string(),
            (EmotionLabel::Fear, true) => "稍微有点在意".to_string(),
            (EmotionLabel::Closeness, false) if dominant_value > 0.6 => "跟他聊天还挺开心的 说实话".to_string(),
            (EmotionLabel::Closeness, false) => "他还挺好的".to_string(),
            (EmotionLabel::Closeness, true) if dominant_value > 0.6 => "和他在一起很安心".to_string(),
            (EmotionLabel::Closeness, true) => "他在就好".to_string(),
            (EmotionLabel::Loneliness, false) if dominant_value > 0.6 => "好安静啊……怎么没人说话".to_string(),
            (EmotionLabel::Loneliness, false) => "太安静了 有点无聊".to_string(),
            (EmotionLabel::Loneliness, true) if dominant_value > 0.6 => "……有点想有人陪着".to_string(),
            (EmotionLabel::Loneliness, true) => "安静呢".to_string(),
            (EmotionLabel::Curiosity, false) if dominant_value > 0.6 => "诶 这个有意思 我想知道更多".to_string(),
            (EmotionLabel::Curiosity, false) => "嗯？什么来着".to_string(),
            (EmotionLabel::Curiosity, true) if dominant_value > 0.6 => "这件事有点让人在意呢".to_string(),
            (EmotionLabel::Curiosity, true) => "嗯……？".to_string(),
        };
        if !frag.is_empty() {
            fragments.push(frag);
        }
    }

    // 需求片段——角色化中文
    if has_need {
        let frag = match (need_name, is_nana) {
            ("belonging", false) if need_value > 0.7 => "想找人聊聊天".to_string(),
            ("belonging", false) => "有人说说话也不错".to_string(),
            ("belonging", true) if need_value > 0.7 => "有点想和他说说话".to_string(),
            ("belonging", true) => "有人陪着也好".to_string(),
            ("autonomy", false) if need_value > 0.7 => "好想自己待一会儿啊".to_string(),
            ("autonomy", false) => "我想先忙自己的事".to_string(),
            ("autonomy", true) if need_value > 0.7 => "想一个人安静一会儿".to_string(),
            ("autonomy", true) => "先做自己的事吧".to_string(),
            ("novelty", false) if need_value > 0.7 => "好无聊啊——有没有什么好玩的事".to_string(),
            ("novelty", false) => "又是一样的日子……".to_string(),
            ("novelty", true) if need_value > 0.7 => "今天好像没什么新鲜事呢".to_string(),
            ("novelty", true) => "日常呢".to_string(),
            ("expression", false) if need_value > 0.7 => "我有点想说点什么".to_string(),
            ("expression", false) => "嗯 有点想开口".to_string(),
            ("expression", true) if need_value > 0.7 => "想跟他说点什么".to_string(),
            ("expression", true) => "想说句话".to_string(),
            ("security", false) if need_value > 0.7 => "总感觉哪里不对".to_string(),
            ("security", true) if need_value > 0.7 => "有点不放心".to_string(),
            _ => String::new(),
        };
        if !frag.is_empty() {
            fragments.push(frag);
        }
    }

    // 注意力焦点
    if let Some(topic) = attention_topic {
        let att = if is_nana {
            format!("还在想{}的事……", topic)
        } else {
            format!("脑子里一直在想{}", topic)
        };
        fragments.push(att);
    }

    // 最近事件残留
    if has_event {
        let evt_summary = last_event_summary.trim_end_matches('.').trim_end_matches('。');
        let evt = if is_nana {
            format!("刚刚发生的事还在心里……{}", evt_summary)
        } else {
            format!("还在想刚才那事——{}", evt_summary)
        };
        fragments.push(evt);
    }

    if fragments.is_empty() {
        return None;
    }

    // 最多取2个片段，避免内心独白太长
    let selected = if fragments.len() > 2 {
        if has_emotion && fragments.len() > 1 {
            vec![fragments[0].clone(), fragments.last().unwrap().clone()]
        } else {
            vec![fragments[0].clone(), fragments[1].clone()]
        }
    } else {
        fragments
    };

    let thought = selected.join(" ");
    Some(format!(
        "{}\n「{}」",
        section_heading("inner_thought", _lang),
        thought
    ))
}

// （原 tool_relay_hint 从未被调用，已移除）

// ========== 单元测试 ==========

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_contains_static_tags() {
        let parts = PromptParts {
            user_input: "你好".to_string(),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        assert!(prompt.contains(STATIC_OPEN));
        assert!(prompt.contains(STATIC_CLOSE));
        assert!(prompt.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
        // 默认 language 为空串，normalize_lang 归一化为 zh
        assert!(prompt.contains("# 用户输入\n你好"));
    }

    #[test]
    fn test_build_prompt_with_persona_blocks() {
        // 注入 character/examples/style 块，验证 prompt 正确包含
        let parts = PromptParts {
            user_input: "你好".to_string(),
            character_block: Some("# Vivian · Identity\n你是 Vivian.".to_string()),
            examples_block: Some("User: 你好\nVivian: 嗨~".to_string()),
            style_block: Some("### Current Performance Mode: daily_chat".to_string()),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        assert!(prompt.contains("Vivian · Identity"));
        assert!(prompt.contains("嗨~"));
        assert!(prompt.contains("Current Performance Mode"));
    }

    #[test]
    fn test_build_prompt_empty_persona_blocks() {
        // 未注入人设块时，prompt 不应崩溃，仍包含静态/动态边界标记
        let parts = PromptParts {
            user_input: "你好".to_string(),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        assert!(prompt.contains(STATIC_OPEN));
        assert!(!prompt.contains("Vivian · Identity"));
    }

    #[test]
    fn test_memory_block_empty() {
        let block = build_memory_block("", "zh");
        // 空记忆 = 首次见面提示（已并入本块）；标题本地化、提示文本英文
        assert!(block.contains("此刻浮上心头的"));
        assert!(block.contains("just met"));
    }

    #[test]
    fn test_memory_block_with_rich_text() {
        let memory_text = "[2026-07-06 14:30 | 印象] User: 我喜欢咖啡 [重点]";
        let block = build_memory_block(memory_text, "zh");
        assert!(block.contains("此刻浮上心头的"));
        assert!(block.contains("我喜欢咖啡"));
        assert!(block.contains("[重点]"));
        // 精简使用规则随记忆本体注入（英文）
        assert!(block.contains("no need to announce"));
    }

    #[test]
    fn test_memory_group_merges_and_positions_before_environment() {
        // 记忆组：episode + relationship_log + 记忆本体合并为单一 section，
        // 且位置在环境上下文之前（U 型注意力黄金位置，避开谷底）
        let parts = PromptParts {
            user_input: "你好".to_string(),
            memory_text: "一段重要记忆".to_string(),
            episode_section: Some("## 相关经历\n- [07-06 14:30~16:00] 闲聊".to_string()),
            relationship_log_section: Some("## 近期关系线索\n- 近期轮次: 聊得开心".to_string()),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        let group = prompt.find("## 你记得的事").expect("记忆组标题应存在");
        let env = prompt.find("你周围正在发生什么").expect("环境段应存在");
        assert!(group < env, "记忆组应在环境上下文之前");
        // 子块标题降级为 ###，不再是独立顶层 section
        assert!(prompt.contains("### 相关经历"));
        assert!(prompt.contains("### 近期关系线索"));
        assert!(prompt.contains("一段重要记忆"));
    }

    #[test]
    fn test_user_profile_group_merges_subsections() {
        let parts = PromptParts {
            user_input: "你好".to_string(),
            user_facts_section: Some("【用户档案】\n- 姓名：测试".to_string()),
            user_model_section: Some("## 我对你的了解\n- coding_style: 简洁".to_string()),
            dynamic_behavior_section: Some("## 动态行为画像\n- 话变少了".to_string()),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        assert!(prompt.contains("## 你对用户的了解"));
        assert!(prompt.contains("### 我对你的了解"));
        assert!(prompt.contains("- 姓名：测试"));
        assert!(prompt.contains("话变少了"));
    }

    #[test]
    fn test_static_zone_contains_relocated_sections() {
        // 伪静态段落（响应决策/渠道指南）应移入 <static> 区，进入前缀缓存
        let parts = PromptParts {
            user_input: "你好".to_string(),
            channel: "wechat".to_string(),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        let static_close = prompt.find(STATIC_CLOSE).expect("静态区应存在");
        let decision = prompt.find("## Response Decision (User Dialogue)").expect("响应决策应存在");
        let channel_guide = prompt.find("## Channel: Text Chat").expect("渠道指南应存在");
        assert!(decision < static_close, "响应决策应在静态区内");
        assert!(channel_guide < static_close, "渠道指南应在静态区内");
        // 动态区不应再重复出现
        let boundary = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        assert!(!prompt[boundary..].contains("## Response Decision (User Dialogue)"));
    }

    #[test]
    fn test_boundary_between_static_and_dynamic() {
        // generation 层依赖 boundary 切分动态区为 user 便签：
        // boundary 必须存在于 </static> 之后、动态内容之前
        let parts = PromptParts {
            user_input: "你好".to_string(),
            memory_text: "边界测试记忆".to_string(),
            ..Default::default()
        };
        let prompt = PromptBuilder::build_prompt(&parts);
        let static_close = prompt.find(STATIC_CLOSE).unwrap();
        let boundary = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        assert!(static_close < boundary, "boundary 应在 </static> 之后");
        assert!(prompt[boundary..].contains("边界测试记忆"), "记忆应在动态区");
    }

    #[test]
    fn test_trim_sections_drops_highest_rank_first() {
        // 超预算时先丢 rank=3，永不丢 rank=0
        let mut sections = vec![
            RankedSection { rank: 0, name: "core", content: "x".repeat(10_000) },
            RankedSection { rank: 3, name: "tone", content: "y".repeat(20_000) },
            RankedSection { rank: 2, name: "worldbook", content: "z".repeat(5_000) },
        ];
        trim_sections_to_budget(&mut sections, 0);
        assert!(!sections.iter().any(|s| s.name == "tone"), "rank=3 应被丢弃");
        assert!(sections.iter().any(|s| s.name == "worldbook"), "回到预算内后 rank=2 保留");
        assert!(sections.iter().any(|s| s.name == "core"), "rank=0 永不丢弃");
    }

    // ===== 英文标记化规则：语义锚点防退化 =====

    /// 规则文件必须保留全部关键语义锚点——丢一个就是压缩过度。
    #[test]
    fn test_rule_files_preserve_semantic_keypoints() {
        let safety = safety_rules();
        for key in [
            "NO_AI_DISCLOSURE", "SEARCH_TRIGGERS", "web_search", "talk_to_character",
            "memory system", "fabricate", "what are you doing", "refuse",
        ] {
            assert!(safety.contains(key), "safety 丢失语义锚点: {key}");
        }
        let format = output_format();
        for key in [
            "no_reply", "response_mode", "tool", "arguments", "voice_message",
            "set_presence_state", "THINKING", "PAUSE", "SPEED", "EMO", "markdown",
        ] {
            assert!(format.contains(key), "output_format 丢失语义锚点: {key}");
        }
        // 示例对话必须原样保留（语气锚点不压缩）
        assert!(format.contains("{\"text\": \"Hmph... fine, you got me there\""));
        assert!(format.contains("\"voice_message\": true"));
        // 其他规则文件的标记块
        assert!(pet_identity().contains("[CAPABILITY_BOUNDARY]"));
        assert!(session_rules().contains("[SESSION_RULES]"));
        assert!(address_rules().contains("[ADDRESS_RULES]"));
        assert!(conversation_rhythm().contains("[RHYTHM_RULES]"));
        assert!(chat_style_framework().contains("[CHAT_STYLE_RULES]"));
    }

    #[test]
    fn test_tool_minimal_identity_has_flags() {
        let zh = build_tool_minimal_identity("vivian", "zh");
        assert!(zh.contains("[PERSONA_LOAD"));
        assert!(zh.contains("LANG_ZH_CN_ONLY"));
        assert!(!zh.contains("LANG_EN_US_ONLY"));
        assert!(zh.contains("REFUSE_SERVICE_SPEECH"));
        assert!(zh.contains("Identity (Keep This!)"));

        let en = build_tool_minimal_identity("vivian", "en");
        assert!(en.contains("LANG_EN_US_ONLY"));
        assert!(!en.contains("LANG_ZH_CN_ONLY"));
    }

    #[test]
    fn test_tool_minimal_output_format_lang() {
        assert!(tool_minimal_output_format("zh").contains("简体中文"));
        assert!(tool_minimal_output_format("en").contains("English"));
        assert!(tool_minimal_output_format("ja").contains("日本語"));
    }

    #[test]
    fn test_tool_continue_prompt() {
        let results = vec![ToolResultEntry {
            tool: "open_application".to_string(),
            status: "SUCCESS".to_string(),
            result: "opened".to_string(),
        }];
        let prompt = build_tool_continue_prompt("vivian", "zh", &results, None, None);
        assert!(prompt.contains("Identity (Keep This!)"));
        assert!(prompt.contains("Tool Execution Results"));
        assert!(prompt.contains("Next Step"));
    }

    #[test]
    fn test_tool_retry_prompt() {
        let prompt = build_tool_retry_prompt("vivian", "zh", "I will open it", None, None);
        assert!(prompt.contains("Your Last Response"));
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("Important Instruction"));
    }

    #[test]
    fn test_tool_parameter_guide_prompt() {
        let prompt = build_tool_parameter_guide_prompt(
            "vivian",
            "zh",
            "open_application",
            "Open an application",
            "previous",
            None,
            None,
        );
        assert!(prompt.contains("Tool: open_application"));
        assert!(prompt.contains("Example"));
        assert!(prompt.contains("Instruction"));
    }
}
