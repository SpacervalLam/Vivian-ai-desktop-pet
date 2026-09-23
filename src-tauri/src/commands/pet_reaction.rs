//! 桌宠交互的轻量反应生成 —— 用户操作触发的极简 LLM 回复 + 事件账本记录。
//!
//! 取代前端写死的固定台词（原 `LOCAL_TOUCH_LINES`）。用户对桌宠做出动作
//! （单击 / 双击 / 戳毛了 / 长按 / 快速拖动 / 甩飞撞边）时，这里用一次**极简 prompt** 的
//! LLM 调用生成一句符合角色人设的短反应，并把这次操作作为「有意义的用户行为」
//! 写入统一事件账本（[`crate::memory::unified_event_ledger`]），供日记、内心独白、
//! 记忆沉淀消费。
//!
//! # 提示词只由三部分构成
//!
//! 1. **精简人设** —— 复用 [`build_tool_minimal_identity`]，一两句话的性格描述
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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::State;

use crate::pipeline::prompt_modules::build_tool_minimal_identity;
use crate::providers::base::LLMRequest;
use crate::state::AppState;
use crate::types::response::ChatMessage;

// ============ 动作常量 ============

/// 单击摸头
pub const ACTION_SINGLE_CLICK: &str = "single_click";
/// 双击
pub const ACTION_DOUBLE_CLICK: &str = "double_click";
/// 被戳烦了（前端判定：短时间内戳得太多、或戳得太急）
pub const ACTION_ROUGH_CLICK: &str = "rough_click";
/// 长按（进度环走满）
pub const ACTION_LONG_PRESS: &str = "long_press";
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
/// 同时管住**账本写入**与**LLM 调用**：用户狂点桌宠时，同类动作在窗口内只处理
/// 一次，既不刷屏账本也不烧 token。不同动作各自独立计时 —— 甩飞之后立刻摸头，
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

// ============ 动作描述 ============

/// 用户动作的「角色视角」描述 —— 喂给 LLM，让桌宠知道刚刚被怎么对待了。
///
/// `name` 是角色名（Vivian / Nana / Vivian / Nana），用于第三人称动作里指代自己。
fn action_prompt_line(name: &str, action: &str, impact: Option<f64>) -> String {
    match action {
        ACTION_SINGLE_CLICK => "用户用鼠标轻轻戳了戳你的脑袋，像是在摸头。".to_string(),
        ACTION_DOUBLE_CLICK => "用户连着快速点了你两下，看起来有话想说。".to_string(),
        ACTION_ROUGH_CLICK => {
            "用户戳你戳得太急太频繁了，跟摸头完全不是一回事，你有点被惹毛了。".to_string()
        }
        ACTION_LONG_PRESS => "用户按住你不放，按了整整一秒才松手。".to_string(),
        ACTION_FAST_DRAG => {
            "用户抓住你疯狂甩动，把你晃得头晕目眩、眼冒金星。".to_string()
        }
        ACTION_EDGE_BOUNCE => {
            let force = impact.unwrap_or(0.0);
            if force >= 2.5 {
                "用户把你一把甩飞出去，你「砰」地一头撞在屏幕边缘上，撞得不轻。".to_string()
            } else if force >= 1.0 {
                "用户把你甩了出去，你撞在屏幕边缘上，晕了一下。".to_string()
            } else {
                "用户松手后你滑到了屏幕边缘，轻轻磕了一下。".to_string()
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
        ACTION_SINGLE_CLICK => format!("用户戳了戳{name}的脑袋，摸了摸头"),
        ACTION_DOUBLE_CLICK => format!("用户快速双击了{name}，像是想跟她说话"),
        ACTION_ROUGH_CLICK => format!("用户连着猛戳了{name}好几下，把她惹毛了"),
        ACTION_LONG_PRESS => format!("用户按住{name}不放，长按了整整一秒"),
        ACTION_FAST_DRAG => format!("用户抓着{name}疯狂甩动，把她晃得头晕目眩"),
        ACTION_EDGE_BOUNCE => {
            let force = impact.unwrap_or(0.0);
            if force >= 2.5 {
                format!("用户把{name}甩飞了出去，她重重撞在屏幕边缘上，晕得厉害")
            } else {
                format!("用户把{name}甩了出去，她撞在屏幕边缘上，晕了一下")
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
        ACTION_FAST_DRAG => "pet_fast_drag",
        ACTION_EDGE_BOUNCE => "pet_edge_bounce",
        _ => "pet_action",
    }
}

// ============ 命令 ============

/// 生成一次桌宠反应（前端在用户操作桌宠后调用）。
///
/// - `action`：`single_click` / `double_click` / `rough_click` / `long_press` / `fast_drag` / `edge_bounce`
/// - `impact`：撞击力度（仅 `edge_bounce` 有值，来自后端甩飞线程）
/// - `character_id`：缺省用当前活跃角色
///
/// 返回生成的一句话；被节流跳过、未配置模型或生成失败时返回 `None`（静默）。
#[tauri::command]
pub async fn generate_pet_reaction(
    state: State<'_, Arc<AppState>>,
    action: String,
    impact: Option<f64>,
    character_id: Option<String>,
) -> Result<Option<String>, String> {
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    Ok(react_to_user_action(&state, &char_id, &action, impact).await)
}

/// 桌宠反应核心逻辑（不依赖 tauri `State`，便于其它后端路径复用）。
///
/// 流程：节流 → 记事件账本 → 极简 prompt 生成回复。
/// 账本记录先于 LLM 调用，即使模型没配好，用户这次操作也已经留下痕迹。
pub async fn react_to_user_action(
    state: &AppState,
    char_id: &str,
    action: &str,
    impact: Option<f64>,
) -> Option<String> {
    let now = chrono::Local::now().timestamp() as f64;
    if !throttle_pass(char_id, action, now) {
        return None;
    }

    log_action_to_ledger(state, char_id, action, impact, now);
    generate_reaction(state, char_id, action, impact).await
}

/// 把用户对桌宠的操作写入统一事件账本。
fn log_action_to_ledger(
    state: &AppState,
    char_id: &str,
    action: &str,
    impact: Option<f64>,
    now: f64,
) {
    let name = state
        .characters
        .read()
        .get(char_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| char_id.to_string());

    let Some(text) = action_ledger_text(&name, action, impact) else {
        return;
    };

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
        .map(|c| c.brain.dialogue.get_history())
        .unwrap_or_default();

    let lang = crate::i18n::get_language();
    let messages = build_reaction_messages(&name, char_id, &lang, action, impact, &history);

    let request = LLMRequest::new("intent_judge", messages)
        .with_max_tokens(REACTION_MAX_TOKENS)
        .with_temperature(REACTION_TEMPERATURE)
        .with_character_id(char_id);

    let outcome = tokio::time::timeout(
        Duration::from_secs(REACTION_TIMEOUT_SECS),
        router.generate(request),
    )
    .await;

    match outcome {
        Ok(Ok(raw)) => {
            let cleaned = clean_reply(&raw);
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
                "Reply with exactly ONE short sentence (under 20 words).",
                "## Recent chat (very low weight)",
                "This is only so you remember the tone and what was being talked about. Do NOT repeat or summarize it.",
                "## What the user just did to you",
                "Say your one-sentence reaction now.",
            ),
            "ja" => (
                "## いま起きたこと",
                "たった一文だけ（20文字以内）で返すこと。",
                "## 最近の会話（重みはごく低い）",
                "口調と思い出すためだけの参考。復唱・要約はしないこと。",
                "## ユーザーが今あなたにしたこと",
                "今の一言だけを出力すること。",
            ),
            _ => (
                "## 刚刚发生了什么",
                "只说一句话，20 字以内。",
                "## 最近的聊天（权重很低）",
                "只是让你记得刚才的语气和在聊什么，不要复述、不要总结。",
                "## 用户刚刚对你做的事",
                "现在就说你那句话。",
            ),
        };

    let persona = build_tool_minimal_identity(char_id, lang);
    let system = format!(
        "{persona}\n\n{task_heading}\n\
        用户刚刚对你的桌宠形象做了一个动作，你只需要随口给一句反应。\n\
        - {limit_rule}\n\
        - 符合上面的人设和语气，不要客服腔，不要解释\n\
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
        let content = msg.content.trim();
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
        assert!(fast.contains("甩"));

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
        assert!(action_prompt_line("Nana", ACTION_ROUGH_CLICK, None).contains("惹毛"));
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
