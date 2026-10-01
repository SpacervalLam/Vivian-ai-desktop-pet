//! 桌宠交互的轻量反应生成 —— 用户操作触发的极简 LLM 回复 + 事件账本记录。
//!
//! 取代前端写死的固定台词（原 `LOCAL_TOUCH_LINES`）。用户对桌宠做出动作
//! （单击 / 双击 / 戳毛了 / 长按 / 快速拖动 / 甩飞撞边）时，这里在概率与统一预算允许时，用一次**极简 prompt** 的
//! LLM 调用生成一句符合角色人设的短反应，并把这次操作作为「有意义的用户行为」
//! 写入统一事件账本（[`crate::memory::unified_event_ledger`]），供日记、内心独白、
//! 记忆沉淀消费。
//!
//! # 提示词只由三部分构成
//!
//! 1. **精简人设** —— 复用 `build_tool_minimal_identity`，一两句话的性格描述
//!    + `PERSONA_LOAD` 硬约束 + 语言标志，不加载完整 persona / 记忆 / 工具表。
//! 2. **低权重历史对话窗口** —— 最近若干条 user↔角色消息，只作语气参考，
//!    在 prompt 里显式标注"权重很低、不要复述"。
//! 3. **用户动作** —— 由 `action` 映射而来的一句自然语言描述（角色视角）。
//!
//! # 模型档位
//!
//! 路由走 `intent_judge` 任务标签 —— 即路由矩阵里那档「极高频、建议用最便宜的
//! 快速模型」的配置。复用而非新增标签，桌宠反应与意图判定共用同一份 flash 模型，
//! 以后要整体调档只改一处。
//!
//! # 失败策略：完全静默
//!
//! 超时 / 无 router / 未配置 API / 输出为空时一律返回 `None`，前端不显示任何气泡。
//! 不回退到本地固定台词 —— 宁可这一下没反应，也不让用户看到写死的话术。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::State;

/// Observed input only; mouse hardware does not provide physical force.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetInteractionMetrics {
    click_count: Option<u32>,
    window_ms: Option<f64>,
    interval_ms: Option<f64>,
    press_duration_ms: Option<f64>,
    drag_speed_px_per_ms: Option<f64>,
    drag_distance_px: Option<f64>,
}

impl PetInteractionMetrics {
    fn prompt_facts(&self) -> String {
        let mut facts = Vec::new();
        if let (Some(count), Some(window)) = (self.click_count, self.window_ms) {
            if window.is_finite() && window > 0.0 {
                facts.push(format!("最近 {window:.0} 毫秒内点击了 {count} 次"));
            }
        }
        for (label, value, unit) in [
            ("距上次点击", self.interval_ms, "毫秒"),
            ("本次按住时长", self.press_duration_ms, "毫秒"),
            ("拖动峰值速度", self.drag_speed_px_per_ms, "像素/毫秒"),
            ("拖动轨迹总长度", self.drag_distance_px, "像素"),
        ] {
            if let Some(v) = value.filter(|v| v.is_finite() && *v >= 0.0) {
                facts.push(format!("{label}：{v:.2} {unit}"));
            }
        }
        facts.join("；")
    }
}

use crate::pipeline::prompt_modules::build_tool_minimal_identity;
use crate::providers::base::LLMRequest;
use crate::state::AppState;
use crate::types::response::ChatMessage;

// ============ 动作常量 ============

/// 单击
pub const ACTION_SINGLE_CLICK: &str = "single_click";
/// 双击
pub const ACTION_DOUBLE_CLICK: &str = "double_click";
/// 被戳烦了（前端判定：短时间内戳得太多、或戳得太急）
pub const ACTION_ROUGH_CLICK: &str = "rough_click";
/// 长按（进度环走满）
pub const ACTION_LONG_PRESS: &str = "long_press";
/// 普通拖动结束
pub const ACTION_DRAG: &str = "drag";
/// 拖动过快（拖动期间被判定疯狂甩动）
pub const ACTION_FAST_DRAG: &str = "fast_drag";
/// 甩飞撞到屏幕边缘
pub const ACTION_EDGE_BOUNCE: &str = "edge_bounce";

/// 生成超时：桌宠反应是「随手一摸」的轻反馈，超时即静默放弃
const REACTION_TIMEOUT_SECS: u64 = 6;
/// 输出上限：只要一句话
const REACTION_MAX_TOKENS: u32 = 64;
/// 采样温度：略高一点，让同一动作的反应不至于每次都一样
const REACTION_TEMPERATURE: f64 = 0.85;
/// 历史对话窗口条数（仅作极低权重语气参考）
const HISTORY_WINDOW: usize = 6;
/// 单条历史消息截断长度（字符）
const HISTORY_ENTRY_MAX_CHARS: usize = 60;
/// 模型输出清洗后的最大字符数（超出直接截断，避免长段落糊在气泡里）
const REPLY_MAX_CHARS: usize = 60;

// ============ 节流 ============

/// 同类动作的处理节流表：key = `"{char_id}:{action}"`，value = 上次处理时间戳（秒）。
///
/// 只合并账本事件；开口另走跨动作、跨角色的概率和预算门。用户狂点桌宠时，同类动作在窗口内只处理
/// 一次，避免刷屏账本。不同动作的事件各自独立计时 —— 甩飞之后立刻摸头，
/// 两件事都会如实记下。
static ACTION_THROTTLE: Lazy<Mutex<HashMap<String, f64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// 各动作的节流窗口（秒）
fn throttle_secs(action: &str) -> f64 {
    match action {
        // 长按本身就是低频操作，窗口拉长避免心智观察器开关时反复触发
        ACTION_LONG_PRESS => 30.0,
        // 拖动过快在拖动期间可能连续命中，窗口取中长
        ACTION_FAST_DRAG => 20.0,
        // 一次甩飞通常连续撞 1~3 次边缘，合并成一次记录
        ACTION_EDGE_BOUNCE => 12.0,
        // 戳毛了本身就是「连续戳」的产物，不节流住会跟着每一次点击刷一遍
        ACTION_ROUGH_CLICK => 20.0,
        // 单击 / 双击：手快连点很常见，8 秒一次足够
        _ => 8.0,
    }
}

/// 尝试占用节流窗口。返回 `false` 表示本次动作应被跳过。
fn throttle_pass(char_id: &str, action: &str, now: f64) -> bool {
    let key = format!("{char_id}:{action}");
    let mut map = ACTION_THROTTLE.lock();
    if let Some(last) = map.get(&key) {
        if now - *last < throttle_secs(action) {
            return false;
        }
    }
    map.insert(key, now);
    true
}

// Speech and observation have separate budgets. An animation can acknowledge every touch;
// speech is optional and must not accumulate across click/drag/long-press callbacks.
const VOICE_COOLDOWN: Duration = Duration::from_secs(40);
const SHARED_VOICE_GAP: Duration = Duration::from_secs(12);
const VOICE_WINDOW: Duration = Duration::from_secs(120);
const VOICE_WINDOW_LIMIT: usize = 2;

#[derive(Default)]
struct VoiceWindow {
    attempts: VecDeque<Instant>,
    last_attempt: Option<Instant>,
    in_flight: bool,
}

#[derive(Default)]
struct ReactionLimiter {
    characters: HashMap<String, VoiceWindow>,
    last_attempt: Option<Instant>,
}

fn reaction_probability(action: &str, metrics: Option<&PetInteractionMetrics>) -> f64 {
    let base = match action {
        ACTION_SINGLE_CLICK => 0.20,
        ACTION_DOUBLE_CLICK => 0.35,
        ACTION_LONG_PRESS => 0.08, // Opening the inspector already provides useful feedback.
        ACTION_DRAG => 0.10,
        ACTION_ROUGH_CLICK => 0.25,
        ACTION_FAST_DRAG | ACTION_EDGE_BOUNCE => 0.35,
        _ => return 0.0,
    };
    // A burst calls for less chatter, not a progressively more angry conversation.
    let burst = metrics.and_then(|m| m.click_count).is_some_and(|n| n >= 4);
    if burst { base * 0.5 } else { base }
}

impl ReactionLimiter {
    fn reserve(&mut self, char_id: &str, action: &str, metrics: Option<&PetInteractionMetrics>, now: Instant, roll: f64) -> bool {
        let probability = reaction_probability(action, metrics);
        if !roll.is_finite() || roll < 0.0 || roll >= probability { return false; }
        if self.last_attempt.is_some_and(|last| now.saturating_duration_since(last) < SHARED_VOICE_GAP) {
            return false;
        }
        let window = self.characters.entry(char_id.to_string()).or_default();
        while window.attempts.front().is_some_and(|last| now.saturating_duration_since(*last) >= VOICE_WINDOW) {
            window.attempts.pop_front();
        }
        if window.in_flight || window.attempts.len() >= VOICE_WINDOW_LIMIT
            || window.last_attempt.is_some_and(|last| now.saturating_duration_since(last) < VOICE_COOLDOWN) {
            return false;
        }
        window.in_flight = true;
        window.attempts.push_back(now);
        window.last_attempt = Some(now);
        self.last_attempt = Some(now);
        true
    }

    fn release(&mut self, char_id: &str) {
        if let Some(window) = self.characters.get_mut(char_id) { window.in_flight = false; }
    }
}

static REACTION_LIMITER: Lazy<Mutex<ReactionLimiter>> = Lazy::new(|| Mutex::new(ReactionLimiter::default()));

struct ReactionPermit { char_id: String }
impl Drop for ReactionPermit {
    fn drop(&mut self) { REACTION_LIMITER.lock().release(&self.char_id); }
}

fn reserve_reaction(char_id: &str, action: &str, metrics: Option<&PetInteractionMetrics>) -> Option<ReactionPermit> {
    REACTION_LIMITER.lock().reserve(char_id, action, metrics, Instant::now(), rand::random::<f64>())
        .then(|| ReactionPermit { char_id: char_id.to_string() })
}

// ============ 动作描述 ============

/// 用户动作的「角色视角」描述 —— 喂给 LLM，让桌宠知道刚刚被怎么对待了。
///
/// `name` 是角色名（Vivian / Nana / Vivian / Nana），用于第三人称动作里指代自己。
fn action_prompt_line(name: &str, action: &str, impact: Option<f64>) -> String {
    match action {
        ACTION_SINGLE_CLICK => "用户用鼠标点了你的桌宠形象一下。".to_string(),
        ACTION_DOUBLE_CLICK => "用户连着快速点了你两下。".to_string(),
        ACTION_ROUGH_CLICK => {
            "用户在短时间内连续快速点击了你的桌宠形象；这只说明点击节奏。".to_string()
        }
        ACTION_LONG_PRESS => "用户按住你，已达到长按触发时长；此时可能还没有松手。".to_string(),
        ACTION_DRAG => "用户按住并拖动了你的桌宠形象，速度和距离见观测数据。".to_string(),
        ACTION_FAST_DRAG => {
            "用户快速拖动了你的桌宠形象。".to_string()
        }
        ACTION_EDGE_BOUNCE => {
            let force = impact.unwrap_or(0.0);
            if force >= 2.5 {
                "你的桌宠形象较快地碰到了屏幕边缘。".to_string()
            } else if force >= 1.0 {
                "你的桌宠形象滑动后碰到了屏幕边缘。".to_string()
            } else {
                "用户松手后，你的桌宠形象滑到了屏幕边缘。".to_string()
            }
        }
        _ => format!("用户对{name}做了一个动作（{action}）。"),
    }
}

/// 用户动作写入事件账本时的「有意义总结」—— 第三人称，带角色名，供日记/独白取材。
///
/// 返回 `None` 表示该动作不值得记账（当前没有这类动作，保留扩展位）。
fn action_ledger_text(name: &str, action: &str, impact: Option<f64>) -> Option<String> {
    let text = match action {
        ACTION_SINGLE_CLICK => format!("用户点击了{name}的桌宠形象"),
        ACTION_DOUBLE_CLICK => format!("用户快速双击了{name}"),
        ACTION_ROUGH_CLICK => format!("用户连续快速点击了{name}的桌宠形象"),
        ACTION_LONG_PRESS => format!("用户按住{name}，触发了长按"),
        ACTION_DRAG => format!("用户拖动了{name}的桌宠形象"),
        ACTION_FAST_DRAG => format!("用户快速拖动了{name}的桌宠形象"),
        ACTION_EDGE_BOUNCE => {
            let force = impact.unwrap_or(0.0);
            if force >= 2.5 {
                format!("{name}的桌宠形象较快地碰到了屏幕边缘")
            } else {
                format!("{name}的桌宠形象滑动后碰到了屏幕边缘")
            }
        }
        _ => return None,
    };
    Some(text)
}

/// 动作的账本标签（在通用标签之后追加）
fn action_tag(action: &str) -> &'static str {
    match action {
        ACTION_SINGLE_CLICK => "pet_tap",
        ACTION_DOUBLE_CLICK => "pet_double_click",
        ACTION_ROUGH_CLICK => "pet_rough_click",
        ACTION_LONG_PRESS => "pet_long_press",
        ACTION_DRAG => "pet_drag",
        ACTION_FAST_DRAG => "pet_fast_drag",
        ACTION_EDGE_BOUNCE => "pet_edge_bounce",
        _ => "pet_action",
    }
}

// ============ 命令 ============

/// 生成一次桌宠反应（前端在用户操作桌宠后调用）。
///
/// - `action`：`single_click` / `double_click` / `rough_click` / `long_press` / `drag` / `fast_drag` / `edge_bounce`
/// - `impact`：撞击力度（仅 `edge_bounce` 有值，来自后端甩飞线程）
/// - `character_id`：缺省用当前活跃角色
/// - `metrics`：点击节奏、按住时长与拖动速度等观测数据；缺省兼容旧调用
///
/// 返回生成的一句话；被节流跳过、未配置模型或生成失败时返回 `None`（静默）。
#[tauri::command]
pub async fn generate_pet_reaction(
    state: State<'_, Arc<AppState>>,
    action: String,
    impact: Option<f64>,
    character_id: Option<String>,
    metrics: Option<PetInteractionMetrics>,
) -> Result<Option<String>, String> {
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    Ok(react_to_user_action(&state, &char_id, &action, impact, metrics.as_ref()).await)
}

/// 桌宠反应核心逻辑（不依赖 tauri `State`，便于其它后端路径复用）。
///
/// 流程：合并事件 → 中性账本记录 → 概率/冷却/预算/在途许可 → 可选短反馈。
/// 账本记录先于 LLM 调用，即使模型没配好，用户这次操作也已经留下痕迹。
pub async fn react_to_user_action(
    state: &AppState,
    char_id: &str,
    action: &str,
    impact: Option<f64>,
    metrics: Option<&PetInteractionMetrics>,
) -> Option<String> {
    if reaction_probability(action, metrics) == 0.0
        || state.factory_reset_in_progress.load(std::sync::atomic::Ordering::SeqCst) { return None; }
    let now = chrono::Local::now().timestamp() as f64;
    if !throttle_pass(char_id, action, now) {
        return None;
    }

    log_action_to_ledger(state, char_id, action, impact, now, metrics);
    let _permit = reserve_reaction(char_id, action, metrics)?;
    // Failed/empty/cancelled requests consume the same rate budget; Drop releases in-flight.
    generate_reaction(state, char_id, action, impact, metrics).await
}

/// 把用户对桌宠的操作写入统一事件账本。
fn log_action_to_ledger(
    state: &AppState,
    char_id: &str,
    action: &str,
    impact: Option<f64>,
    now: f64,
    metrics: Option<&PetInteractionMetrics>,
) {
    let name = state
        .characters
        .read()
        .get(char_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| char_id.to_string());

    let Some(mut text) = action_ledger_text(&name, action, impact) else {
        return;
    };

    if let Some(metrics) = metrics {
        let facts = metrics.prompt_facts();
        if !facts.is_empty() {
            text.push_str(&format!("（{facts}）"));
        }
    }

    crate::memory::unified_event_ledger::register_world_event(
        "user_pet_action",
        &text,
        vec![
            "user_action".to_string(),
            "pet_interaction".to_string(),
            action_tag(action).to_string(),
        ],
        now,
        Some(char_id),
    );
}

/// 用极简 prompt 调一次 flash 模型，生成一句桌宠反应。
async fn generate_reaction(
    state: &AppState,
    char_id: &str,
    action: &str,
    impact: Option<f64>,
    metrics: Option<&PetInteractionMetrics>,
) -> Option<String> {
    let router = state.model_router.read().as_ref().cloned()?;

    let name = state
        .characters
        .read()
        .get(char_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| char_id.to_string());

    // 低权重历史对话窗口：先取出消息再 await，避免持锁跨 await
    let history = state
        .characters
        .read()
        .get(char_id)
        .map(|c| c.brain.dialogue.get_user_visible_history())
        .unwrap_or_default();

    let lang = crate::i18n::get_language();
    let mut messages = build_reaction_messages(&name, char_id, &lang, action, impact, &history);

    if let Some(metrics) = metrics {
        let facts = metrics.prompt_facts();
        if !facts.is_empty() {
            if let Some(message) = messages.last_mut() {
                message.content.push_str(&format!("\n可观测的交互数据：{facts}。根据节奏与速度理解动作，亲近、逗弄、催促或打扰只作为可能含义，不要断言用户意图。鼠标无法测量真实力度，不要把速度或频率说成测得的压力；回复不要报数字。"));
            }
        }
    }

    let request = LLMRequest::new("intent_judge", messages)
        .with_max_tokens(REACTION_MAX_TOKENS)
        .with_temperature(REACTION_TEMPERATURE)
        .with_character_id(char_id)
        .without_framework_instructions();

    let outcome = tokio::time::timeout(
        Duration::from_secs(REACTION_TIMEOUT_SECS),
        router.generate(request),
    )
    .await;

    match outcome {
        Ok(Ok(raw)) => {
            let cleaned = clean_reply(&raw);
            let cleaned = if cleaned.eq_ignore_ascii_case("[SILENT]") { String::new() } else { cleaned };
            if cleaned.is_empty() {
                tracing::debug!("[PetReaction] {char_id} 模型返回空文本，静默跳过");
                None
            } else {
                tracing::debug!("[PetReaction] {char_id} {action} → {cleaned}");
                Some(cleaned)
            }
        }
        Ok(Err(e)) => {
            tracing::debug!("[PetReaction] {char_id} 生成失败，静默跳过: {e}");
            None
        }
        Err(_) => {
            tracing::debug!(
                "[PetReaction] {char_id} 生成超时 ({}s)，静默跳过",
                REACTION_TIMEOUT_SECS
            );
            None
        }
    }
}

fn reaction_voice(char_id: &str) -> &'static str {
    match char_id {
        "nana" => "用温柔的疑问或轻轻应声接住这份注意；即使想让对方停一停，也轻声说明自己的界限，不责问。",
        _ => "可以有一点突然被注意到的惊讶；害羞时短促地别扭一下，暖意仍可感受到。没有羞涩的情境就正常回应，不靠训人或贬低用户表现傲娇。",
    }
}

/// 组装极简 prompt：精简人设 + 低权重历史 + 本次动作。
fn build_reaction_messages(
    name: &str,
    char_id: &str,
    lang: &str,
    action: &str,
    impact: Option<f64>,
    history: &[ChatMessage],
) -> Vec<ChatMessage> {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    let (task_heading, limit_rule, history_heading, history_note, action_heading, tail) =
        match lang_norm {
            "en" => (
                "## What just happened",
                "If you speak, use one short sentence (under 20 words); [SILENT] is also valid.",
                "## Recent chat (very low weight)",
                "This is only so you remember the tone and what was being talked about. Do NOT repeat or summarize it.",
                "## What the user just did to you",
                "A brief reaction or [SILENT], whichever fits.",
            ),
            "ja" => (
                "## いま起きたこと",
                "話すなら短い一文で。自然な一言がなければ [SILENT]。",
                "## 最近の会話（重みはごく低い）",
                "口調と思い出すためだけの参考。復唱・要約はしないこと。",
                "## ユーザーが今あなたにしたこと",
                "今の一言、または [SILENT] を出力すること。",
            ),
            _ => (
                "## 刚刚发生了什么",
                "开口时只说一句，20 字以内；没有自然的一句可输出 [SILENT]。",
                "## 最近的聊天（权重很低）",
                "只是让你记得刚才的语气和在聊什么，不要复述、不要总结。",
                "## 用户刚刚对你做的事",
                "输出自然的一句或 [SILENT]。",
            ),
        };

    let persona = build_tool_minimal_identity(char_id, lang);
    let voice = reaction_voice(char_id);
    let system = format!(
        "{persona}\n\n{task_heading}\n\
        用户刚刚操作了你的桌宠形象。这是可选的轻反馈，不是被冒犯或需要审问的证据。\n\
        - {limit_rule}\n\
        - {voice}\n\
        - 普通点击和开关窗口不需要责问、催用户说话或抱怨用户手闲；连续操作也只可轻轻提出自己的界限。不猜测无聊、恶意、真实力度或受伤。\n\
        - 符合上面的人设和语气，不要客服腔，不要解释；没有自然的一句话可以只输出 [SILENT]\n\
        - 只输出这句话本身：不要引号、不要动作描写、不要括号旁白、不要 Markdown"
    );

    let history_block = build_history_block(history, lang_norm, history_heading, history_note);
    let action_line = action_prompt_line(name, action, impact);

    let user = match history_block {
        Some(block) => format!("{block}\n\n{action_heading}\n{action_line}\n\n{tail}"),
        None => format!("{action_heading}\n{action_line}\n\n{tail}"),
    };

    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// 把最近若干条对话渲染成「低权重参考」段落；没有历史时返回 `None`。
fn build_history_block(
    history: &[ChatMessage],
    lang_norm: &str,
    heading: &str,
    note: &str,
) -> Option<String> {
    if history.is_empty() {
        return None;
    }
    let history: Vec<&ChatMessage> = history.iter().filter(|msg| {
        if msg.role != "user" && msg.role != "assistant" { return false; }
        if msg.meta.as_ref().and_then(|meta| meta.channel.as_deref()) == Some("cross_character") {
            return false;
        }
        let (_, speaker, listener) = crate::cross_character::parse_any_speaker_prefix(&msg.content);
        speaker.as_deref().is_none_or(|s| s == "user" || s == "i")
            && listener.as_deref().is_none_or(|l| l == "me" || l == "user")
    }).collect();
    let start = history.len().saturating_sub(HISTORY_WINDOW);
    let (user_label, me_label) = match lang_norm {
        "en" => ("User", "Me"),
        "ja" => ("ユーザー", "わたし"),
        _ => ("用户", "我"),
    };
    let mut lines: Vec<String> = Vec::with_capacity(HISTORY_WINDOW + 2);
    lines.push(heading.to_string());
    lines.push(note.to_string());
    for msg in &history[start..] {
        let (content, _, _) = crate::cross_character::parse_any_speaker_prefix(&msg.content);
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let label = if msg.role == "user" { user_label } else { me_label };
        let snippet: String = content.chars().take(HISTORY_ENTRY_MAX_CHARS).collect();
        lines.push(format!("{label}: {snippet}"));
    }
    // 只有标题+说明、没有实际消息时视为无历史
    if lines.len() <= 2 {
        return None;
    }
    Some(lines.join("\n"))
}

/// 清洗模型输出：去掉引号/换行/Markdown 痕迹，截断到合理长度。
fn clean_reply(raw: &str) -> String {
    let mut text = raw.trim().to_string();

    // 只取第一行非空内容，避免模型输出多段
    if let Some(first) = text.lines().map(str::trim).find(|l| !l.is_empty()) {
        text = first.to_string();
    }

    // 去掉成对包裹的引号（中英文都覆盖）
    for (open, close) in [('"', '"'), ('\'', '\''), ('“', '”'), ('「', '」'), ('『', '』')] {
        if text.starts_with(open) && text.ends_with(close) && text.chars().count() > 2 {
            text = text[open.len_utf8()..text.len() - close.len_utf8()]
                .trim()
                .to_string();
        }
    }

    // 整句被旁白括号包住时剥掉外层（如「（晕……）」→「晕……」）
    const WRAPPERS: [(char, char); 4] =
        [('（', '）'), ('(', ')'), ('【', '】'), ('[', ']')];
    if text.chars().count() > 2 {
        if let (Some(open), Some(close)) = (text.chars().next(), text.chars().last()) {
            if WRAPPERS.iter().any(|(o, c)| *o == open && *c == close) {
                let inner: String = text.chars().skip(1).collect();
                let inner: String = inner.chars().take(inner.chars().count() - 1).collect();
                text = inner.trim().to_string();
            }
        }
    }

    let cleaned: String = text.chars().take(REPLY_MAX_CHARS).collect();
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_contact_voice_cooldown_covers_different_actions() {
        let mut limiter = ReactionLimiter::default();
        let t = Instant::now();
        assert!(limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t, 0.0));
        limiter.release("nana");
        assert!(!limiter.reserve("nana", ACTION_DOUBLE_CLICK, None, t + Duration::from_secs(8), 0.0));
        assert!(!limiter.reserve("nana", ACTION_LONG_PRESS, None, t + Duration::from_secs(39), 0.0));
        assert!(limiter.reserve("nana", ACTION_EDGE_BOUNCE, None, t + VOICE_COOLDOWN, 0.0));
    }

    #[test]
    fn companion_contact_shared_gap_and_sliding_budget() {
        let mut limiter = ReactionLimiter::default();
        let t = Instant::now();
        assert!(limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t, 0.0));
        limiter.release("nana");
        assert!(!limiter.reserve("vivian", ACTION_DOUBLE_CLICK, None, t + Duration::from_secs(1), 0.0));
        assert!(limiter.reserve("vivian", ACTION_DOUBLE_CLICK, None, t + SHARED_VOICE_GAP, 0.0));
        limiter.release("vivian");
        assert!(limiter.reserve("nana", ACTION_DRAG, None, t + VOICE_COOLDOWN, 0.0));
        limiter.release("nana");
        assert!(!limiter.reserve("nana", ACTION_ROUGH_CLICK, None, t + Duration::from_secs(90), 0.0));
        // Old attempt expires at the rolling-window boundary, not at a wall-clock minute.
        assert!(limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t + VOICE_WINDOW, 0.0));
    }

    #[test]
    fn companion_contact_burst_and_silence_do_not_spend_voice_budget() {
        let mut limiter = ReactionLimiter::default();
        let t = Instant::now();
        let burst = PetInteractionMetrics { click_count: Some(5), ..Default::default() };
        assert!(reaction_probability(ACTION_SINGLE_CLICK, Some(&burst)) < reaction_probability(ACTION_SINGLE_CLICK, None));
        assert!(!limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t, 0.9));
        assert!(!limiter.reserve("nana", ACTION_SINGLE_CLICK, Some(&burst), t, 0.15));
        assert!(!limiter.reserve("nana", "unknown", None, t, 0.0));
        assert!(!limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t, f64::NAN));
        assert!(limiter.characters.is_empty());
        assert!(limiter.reserve("nana", ACTION_SINGLE_CLICK, None, t, 0.15));
    }

    #[test]
    fn companion_contact_in_flight_blocks_even_after_cooldown() {
        let mut limiter = ReactionLimiter::default();
        let t = Instant::now();
        assert!(limiter.reserve("vivian", ACTION_SINGLE_CLICK, None, t, 0.0));
        assert!(!limiter.reserve("vivian", ACTION_FAST_DRAG, None, t + VOICE_WINDOW, 0.0));
        limiter.release("vivian");
        assert!(limiter.reserve("vivian", ACTION_FAST_DRAG, None, t + VOICE_WINDOW, 0.0));
    }

    #[tokio::test]
    async fn companion_contact_cancel_releases_permit_but_keeps_budget() {
        let id = "test-reaction-cancel";
        let t = Instant::now();
        REACTION_LIMITER.lock().characters.insert(id.into(), VoiceWindow {
            attempts: VecDeque::from([t]), last_attempt: Some(t), in_flight: true,
        });
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _permit = ReactionPermit { char_id: id.into() };
            let _ = ready.send(());
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let limiter = REACTION_LIMITER.lock();
        let window = &limiter.characters[id];
        assert!(!window.in_flight);
        assert_eq!(window.attempts.len(), 1);
        assert_eq!(window.last_attempt, Some(t));
    }

    #[test]
    fn companion_contact_touch_history_does_not_attribute_roommate_to_user() {
        let mut unprefixed_roommate = ChatMessage::user("未加前缀的室友消息");
        let mut meta = crate::messages::MessageMeta::new(crate::messages::MessageSource::User);
        meta.channel = Some("cross_character".to_string());
        unprefixed_roommate.meta = Some(meta);
        let history = vec![ChatMessage::user("你好"),
            ChatMessage::user("[Nana says to me] 被点了一下"),
            ChatMessage::assistant("[I say to Nana] 室友交流"),
            unprefixed_roommate,
            ChatMessage::assistant("你好呀")];
        let block = build_history_block(&history, "zh", "历史", "说明").unwrap();
        assert!(block.contains("用户: 你好"));
        assert!(block.contains("我: 你好呀"));
        assert!(!block.contains("被点了一下"));
        assert!(!block.contains("室友交流"));
        assert!(!block.contains("未加前缀"));
    }

    #[test]
    fn frontend_metrics_deserialize_and_preserve_drag_units() {
        let metrics: PetInteractionMetrics = serde_json::from_value(serde_json::json!({
            "clickCount": 4, "windowMs": 5000, "intervalMs": 150,
            "pressDurationMs": 80, "dragSpeedPxPerMs": 4.5, "dragDistancePx": 600,
        })).unwrap();
        let facts = metrics.prompt_facts();
        assert!(facts.contains("4 次"));
        assert!(facts.contains("80.00 毫秒"));
        assert!(facts.contains("4.50 像素/毫秒"));
        assert!(facts.contains("600.00 像素"));
        assert!(PetInteractionMetrics::default().prompt_facts().is_empty());
    }

    #[test]
    fn observations_are_neutral_and_reject_invalid_numbers() {
        let facts = PetInteractionMetrics {
            click_count: Some(4), window_ms: Some(5000.0), interval_ms: Some(150.0),
            drag_speed_px_per_ms: Some(f64::NAN), press_duration_ms: Some(-1.0),
            ..Default::default()
        }.prompt_facts();
        assert!(facts.contains("4 次"));
        assert!(facts.contains("150.00"));
        assert!(!facts.contains("NaN"));
        assert!(!facts.contains("按住"));
        assert!(!action_prompt_line("Nana", ACTION_SINGLE_CLICK, None).contains("摸头"));
        assert!(!action_ledger_text("Nana", ACTION_SINGLE_CLICK, None).unwrap().contains("摸头"));
    }

    #[test]
    fn clean_reply_strips_quotes_and_takes_first_line() {
        assert_eq!(clean_reply("“哎呀，别晃我啦！”"), "哎呀，别晃我啦！");
        assert_eq!(clean_reply("\"喂喂，轻一点！\"\n\n（旁白）"), "喂喂，轻一点！");
        assert_eq!(clean_reply("  「晕……」  "), "晕……");
        assert_eq!(clean_reply(""), "");
    }

    #[test]
    fn clean_reply_truncates_overlong_output() {
        let long = "啊".repeat(200);
        assert_eq!(clean_reply(&long).chars().count(), REPLY_MAX_CHARS);
    }

    #[test]
    fn throttle_blocks_same_action_within_window() {
        // 用独立 key 避免与其它测试共享状态
        let cid = "test-throttle-char";
        let action = ACTION_SINGLE_CLICK;
        let now = 1_000_000.0;
        assert!(throttle_pass(cid, action, now));
        assert!(!throttle_pass(cid, action, now + 1.0));
        assert!(throttle_pass(cid, action, now + throttle_secs(action) + 0.1));
        // 不同动作互不影响
        assert!(throttle_pass(cid, ACTION_FAST_DRAG, now));
    }

    #[test]
    fn ledger_text_is_meaningful_for_dizzy_cases() {
        let fast = action_ledger_text("Vivian", ACTION_FAST_DRAG, None).unwrap();
        assert!(fast.contains("Vivian"));
        assert!(fast.contains("快速拖动"));

        let hard = action_ledger_text("Vivian", ACTION_EDGE_BOUNCE, Some(3.0)).unwrap();
        let soft = action_ledger_text("Vivian", ACTION_EDGE_BOUNCE, Some(0.5)).unwrap();
        assert_ne!(hard, soft);
    }

    #[test]
    fn rough_click_is_distinct_from_single_click() {
        let rough = action_ledger_text("Nana", ACTION_ROUGH_CLICK, None).unwrap();
        let tap = action_ledger_text("Nana", ACTION_SINGLE_CLICK, None).unwrap();
        assert_ne!(rough, tap);
        assert!(rough.contains("Nana"));
        // 动作语与账本文案都要认得出这个动作，不能落到 `_` 兜底上
        assert!(action_prompt_line("Nana", ACTION_ROUGH_CLICK, None).contains("连续快速点击"));
        assert!(!rough.contains("惹毛"));
        assert!(!rough.contains("恶意"));
        assert_eq!(action_tag(ACTION_ROUGH_CLICK), "pet_rough_click");
        // 狂戳时不该跟着每次点击烧一次 token
        assert!(throttle_secs(ACTION_ROUGH_CLICK) > throttle_secs(ACTION_SINGLE_CLICK));
    }

    #[test]
    fn history_block_skips_when_empty() {
        assert!(build_history_block(&[], "zh", "## 最近的聊天", "说明").is_none());
    }

    #[test]
    fn history_block_keeps_recent_window() {
        let msgs: Vec<ChatMessage> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    ChatMessage::user(format!("消息{i}"))
                } else {
                    ChatMessage::assistant(format!("回复{i}"))
                }
            })
            .collect();
        let block = build_history_block(&msgs, "zh", "## 最近的聊天", "说明").unwrap();
        // 标题 + 说明 + 最近 6 条
        assert_eq!(block.lines().count(), 8);
        assert!(block.contains("消息8"));
        assert!(!block.contains("消息0"));
    }
}
