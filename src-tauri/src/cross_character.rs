//! 跨角色通信总线
//!
//! 让两个角色之间可以互相对话。源角色调用 `talk_to_character` 工具发起对话，
//! 总线将消息投递到目标角色的 Brain.think，并将响应通过 stream_id 路由回源角色上下文。
//!
//! 设计要点：
//! - 每个角色拥有独立的 think_lock，源角色和目标角色可同时 think（无全局锁）
//! - 目标角色的 Brain.think 写入目标角色自己的 dialogue/memory/psychology，不污染源角色
//! - 跨角色对话产生的流式 chunk 通过 `cross:chunk` 事件推送到前端
//! - 工具调用方（源角色）可通过返回值拿到目标角色的完整回复文本
//! - 仅持有 AppHandle（通过 app.state() 获取 AppState），避免回环引用

use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::VivianResult;
use crate::pipeline::steps::generation::StreamEmitter;
use crate::state::AppState;
use crate::utils::truncate_chars;

/// 剥离合成输入尾部的记忆触发锚点脚手架（`[近期你们的话题]` / `[请基于上述记忆自然地回应]`）。
///
/// 锚点由 deliver_message 拼接到合成输入尾部供 LLM 参考，
/// 不应随真实话语流入记忆 / 对话历史 / 事件账本。
fn strip_memory_anchor(text: &str) -> String {
    const MARKERS: [&str; 2] = ["[近期你们的话题]", "[请基于上述记忆自然地回应]"];
    let mut end = text.len();
    for marker in MARKERS {
        if let Some(pos) = text[..end].find(marker) {
            end = pos;
        }
    }
    text[..end].trim_end().to_string()
}

/// 清洗用于组装记忆锚点的事件预览文本。
///
/// 去除历史数据中残留的锚点标记。
fn clean_anchor_preview(text: &str) -> String {
    let stripped = strip_memory_anchor(text);
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 将角色 ID 转换为首字母大写的显示名称（user→User, vivian→Vivian, nana→Nana）
pub fn display_name(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// 构建说话者前缀字符串。
///
/// - speaker_id: 说话者 ID（"user" / 角色 ID 如 "vivian" / "i" 表示第一人称）
/// - listener_id: 听话者 ID（"user" / 角色 ID / "me" 表示第一人称）
/// - char_id: 当前角色 ID（当 speaker_id == char_id 时用 "I"，当 listener_id == char_id 时用 "me"）
///
/// 返回形如 "[User says to me]" / "[Vivian says to User]" / "[I say to Nana]" 的前缀。
pub fn build_speaker_prefix(speaker_id: &str, listener_id: &str, char_id: &str) -> String {
    let speaker_is_self = speaker_id == char_id || speaker_id == "i" || speaker_id == "I";
    let listener_is_self = listener_id == char_id || listener_id == "me";

    let speaker_name = if speaker_is_self {
        "I".to_string()
    } else {
        display_name(speaker_id)
    };
    let listener_name = if listener_is_self {
        "me".to_string()
    } else {
        display_name(listener_id)
    };

    let verb = if speaker_is_self { "say" } else { "says" };
    format!("[{} {} to {}]", speaker_name, verb, listener_name)
}

/// 解析任意说话者前缀（支持第一人称/第三人称/旁观视角）。
///
/// 支持的格式：
/// - "[User says to me] xxx" → (xxx, "user", "me")
/// - "[Vivian says to me] xxx" → (xxx, "vivian", "me")
/// - "[I say to User] xxx" → (xxx, char_id, "user")
/// - "[I say to Vivian] xxx" → (xxx, char_id, "vivian")
/// - "[User says to Vivian] xxx" → (xxx, "user", "vivian")  (旁观)
/// - "[Vivian says to User] xxx" → (xxx, "vivian", "user")  (旁观)
/// - "[Nana says to Vivian] xxx" → (xxx, "nana", "vivian")  (旁观)
///
/// 返回 (剥离前缀的文本, speaker_id, listener_id)。
/// speaker_id "i" 表示第一人称（需调用方映射为具体 char_id）；
/// listener_id "me" 表示第一人称接收者。
pub fn parse_any_speaker_prefix(text: &str) -> (String, Option<String>, Option<String>) {
    if !text.starts_with('[') {
        return (text.to_string(), None, None);
    }
    // 匹配模式: [Speaker say/says to Listener] 内容
    // Speaker 可以是: I, User, Vivian, Nana, 或以大写字母开头的名字
    // say/says: I 搭配 say，其他人搭配 says
    // Listener 可以是: me, User, Vivian, Nana, 或以大写字母开头的名字
    if let Some(close_bracket) = text.find(']') {
        let inside = &text[1..close_bracket];
        // 尝试匹配 "X say/says to Y"
        let parts: Vec<&str> = inside.split_whitespace().collect();
        if parts.len() == 4 && (parts[1] == "say" || parts[1] == "says") && parts[2] == "to" {
            let speaker = parts[0];
            let listener = parts[3];
            let rest = text[close_bracket + 1..].trim_start().to_string();

            let speaker_id = match speaker {
                "I" => "i".to_string(),
                "User" => "user".to_string(),
                "Vivian" => "vivian".to_string(),
                "Nana" => "nana".to_string(),
                other => other.to_lowercase(),
            };
            let listener_id = match listener {
                "me" => "me".to_string(),
                "User" => "user".to_string(),
                "Vivian" => "vivian".to_string(),
                "Nana" => "nana".to_string(),
                other => other.to_lowercase(),
            };
            return (strip_memory_anchor(&rest), Some(speaker_id), Some(listener_id));
        }
    }
    (text.to_string(), None, None)
}

/// 解析 `[X says to me]` 前缀，返回 (剥离前缀与锚点脚手架后的文本, 说话者 ID)。
///
/// - 带前缀：`[Vivian says to me] xxx` → ("xxx", "vivian")
/// - 无前缀：`xxx` → ("xxx", "user")
///
/// 说话者名称归一化为小写 char_id；"User" 特判为 "user"。
pub fn parse_speaker_prefix(user_input: &str) -> (String, String) {
    if let Some(close) = user_input.find(" says to me]") {
        if user_input.starts_with('[') {
            let speaker_name = &user_input[1..close];
            let rest = &user_input[close + " says to me]".len()..];
            let speaker_id = match speaker_name {
                "User" => "user".to_string(),
                "Vivian" => "vivian".to_string(),
                "Nana" => "nana".to_string(),
                other => other.to_lowercase(),
            };
            return (strip_memory_anchor(rest.trim_start()), speaker_id);
        }
    }
    (user_input.to_string(), "user".to_string())
}

/// 跨角色消息请求
pub struct CrossCharacterRequest {
    /// 源角色 ID（发起方）
    pub source_id: String,
    /// 目标角色 ID（接收方）
    pub target_id: String,
    /// 源角色要说的话
    pub message: String,
    /// 流式 ID（用于前端路由 chunk 事件）
    pub stream_id: String,
}

/// 跨角色对话的返回结果（结构化）
///
/// 由 `CrossCharacterBus::send` 返回，工具层根据 `response_mode` 和 `conv_state`
/// 转换为 LLM 友好的文本提示。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrossCharacterReply {
    /// 目标角色回复文本（仅 `response_mode="speak"` 时非空）
    pub reply: String,
    /// 目标角色本轮响应模式：speak / non_verbal / internal / ignore
    pub response_mode: String,
    /// 会话状态：active / cooling / closed
    pub conv_state: String,
    /// 是否建议源角色继续投递（false 时源角色应停止/切换话题）
    pub should_continue: bool,
    /// 表情（speak/non_verbal 才有意义）
    pub expression: String,
    /// 动作
    pub motion: String,
}

/// 跨角色交接上下文包
///
/// 当源角色向目标角色发起对话时，打包源角色当前的状态与最近用户对话片段，
/// 让目标角色在回应时能感知"源角色刚才在做什么、用户说了什么"，避免目标角色
/// 凭空回话或重复用户已经说过的话。
#[derive(Debug, Clone, Default)]
pub struct HandoffContext {
    /// 源角色当前主导情绪（如 "joy"/"sadness"/"anger"）
    pub source_emotion: String,
    /// 源角色当前疲劳度 [0, 1]
    pub source_fatigue: f64,
    /// 源角色最近 2 轮与用户的对话（按时间顺序，已格式化为 "User: ..." / "I: ..."）
    pub recent_user_dialogue: Vec<String>,
    /// 源角色发起交接的原因简述（由调用方填写，如 "user_asked_for_nana" / "topic_handoff"）
    pub handoff_reason: String,
}

impl HandoffContext {
    /// 渲染为 prompt 注入文本（注入到目标角色合成输入尾部）
    pub fn render(&self, source_name: &str) -> String {
        if self.recent_user_dialogue.is_empty()
            && self.source_emotion.is_empty()
            && self.handoff_reason.is_empty()
        {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("[交接上下文：{}]", source_name));
        if !self.handoff_reason.is_empty() {
            lines.push(format!("交接原因：{}", self.handoff_reason));
        }
        if !self.source_emotion.is_empty() {
            lines.push(format!(
                "{} 当前情绪：{}，疲劳度：{:.2}",
                source_name, self.source_emotion, self.source_fatigue
            ));
        }
        if !self.recent_user_dialogue.is_empty() {
            lines.push(format!("{} 最近与用户的对话：", source_name));
            for dlg in &self.recent_user_dialogue {
                lines.push(format!("  {}", dlg));
            }
        }
        lines.join("\n")
    }
}

/// 从源角色 Brain 构建交接上下文包
fn build_handoff_context(source_brain: &crate::brain::Brain, handoff_reason: &str) -> HandoffContext {
    let mood = source_brain.psychology.compute_mood();
    let source_emotion = mood.primary_emotion.as_str().to_string();
    let source_fatigue = mood.fatigue;

    let recent_user_dialogue: Vec<String> = {
        let history = source_brain.dialogue.get_history_filtered_by_channel(Some("wechat"));
        let user_turns: Vec<&crate::types::response::ChatMessage> = history
            .iter()
            .rev()
            .filter(|m| m.role == "user")
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        user_turns
            .into_iter()
            .map(|m| format!("User: {}", m.content))
            .collect()
    };

    HandoffContext {
        source_emotion,
        source_fatigue,
        recent_user_dialogue,
        handoff_reason: handoff_reason.to_string(),
    }
}

/// 跨角色通信总线（全局单例）
pub static CROSS_CHARACTER_BUS: Lazy<Arc<CrossCharacterBus>> = Lazy::new(|| {
    Arc::new(CrossCharacterBus {
        app_handle: RwLock::new(None),
    })
});

pub struct CrossCharacterBus {
    /// Tauri AppHandle，由 lib.rs 启动时注入，用于 emit 事件和获取 AppState
    app_handle: RwLock<Option<AppHandle>>,
}

impl CrossCharacterBus {
    /// 注入 AppHandle（在 lib.rs setup 中调用一次）
    pub fn initialize(&self, handle: AppHandle) {
        *self.app_handle.write() = Some(handle);
    }

    /// 工具系统调用入口：通过 AppHandle 获取 AppState 发起跨角色对话
    pub async fn send_from_tool(&self, req: CrossCharacterRequest) -> VivianResult<CrossCharacterReply> {
        let handle = self
            .app_handle
            .read()
            .clone()
            .ok_or_else(|| crate::error::VivianError::Engine(
                "AppHandle 未注入，无法发起跨角色对话".to_string(),
            ))?;
        let state = handle.state::<Arc<AppState>>().inner().clone();
        self.send(&handle, &state, req).await
    }

    /// 生成室友的 Public State prompt 段落（共享世界，不共享心智）
    ///
    /// 只暴露 Public 信息：在线状态、在场状态、主导情绪、最近发言时间。
    /// **绝对不暴露** Thought / Belief / Memory / Attention / Goal —— 这些是 Private Mind。
    pub fn roommate_status_text(&self, source_id: &str, lang: &str) -> Option<String> {
        let handle = self.app_handle.read().clone()?;
        let state = handle.state::<Arc<AppState>>().inner().clone();
        let characters = state.characters.read();
        // 只有一个室友：找除自己外的第一个角色
        let roommate = characters.values().find(|c| c.id != source_id)?;

        let lang = crate::pipeline::prompt_modules::normalize_lang(lang);
        let name = &roommate.name;

        let online = *roommate.online.read();
        if !online {
            return Some(match lang {
                "en" => format!(
                    "Your roommate {} is offline (resting) right now. You can't reach her.",
                    name
                ),
                "ja" => format!(
                    "ルームメイトの{}は今オフライン（休憩中）で、連絡できない。",
                    name
                ),
                _ => format!(
                    "你的室友{}现在离线（休息中），你无法联系她。",
                    name
                ),
            });
        }

        // ── 拼装自然语言叙述 ──
        let presence_elapsed = roommate.brain.presence.elapsed_seconds();
        let duration = format_duration_short(presence_elapsed);

        // 主导情绪强度 → 关系描述（不暴露 7 维详情）
        let emotion = roommate.brain.psychology.emotion();
        let (_, intensity) = emotion.dominant();
        let emotion_desc: &str = if intensity > 0.7 {
            match lang {
                "en" => "very close",
                "ja" => "とても親密",
                _ => "十分亲近",
            }
        } else if intensity > 0.4 {
            match lang {
                "en" => "fairly close",
                "ja" => "まあ親密",
                _ => "比较亲近",
            }
        } else {
            match lang {
                "en" => "not particularly close",
                "ja" => "あまり親密ではない",
                _ => "关系一般",
            }
        };

        Some(match lang {
            "en" => format!(
                "Your roommate {} is the user's other desktop pet. She's online right now, been on the desktop for {}. You two are {}. You can reach her via the talk_to_character tool.",
                name, duration, emotion_desc
            ),
            "ja" => format!(
                "ルームメイトの{}はユーザーのもう一つのデスクトップペットで、今オンラインだ。デスクトップに{}いる。二人は{}。talk_to_character ツールで連絡できる。",
                name, duration, emotion_desc
            ),
            _ => format!(
                "你的室友{}是用户的另一个桌面宠物，她现在在线。已经在桌面上呆了{}了，你们{}，你可以通过 talk_to_character 工具联系她。",
                name, duration, emotion_desc
            ),
        })
    }

    /// 查询室友的 ID 和名称（供 prompt 注入层获取室友信息）
    ///
    /// 返回 (roommate_id, roommate_name)，无室友时返回 None
    pub fn roommate_info(&self, source_id: &str) -> Option<(String, String)> {
        let handle = self.app_handle.read().clone()?;
        let state = handle.state::<Arc<AppState>>().inner().clone();
        let characters = state.characters.read();
        let roommate = characters.values().find(|c| c.id != source_id)?;
        Some((roommate.id.clone(), roommate.name.clone()))
    }

    /// 生成社交状态 prompt 段落（三方关系数值快照）
    ///
    /// 需要读取当前角色和室友的 RelationshipState，调用 SocialStateEngine 格式化。
    /// 无室友或无 PsychologyManager 时返回 None。
    pub fn social_state_text(&self, source_id: &str, lang: &str) -> Option<String> {
        let handle = self.app_handle.read().clone()?;
        let state = handle.state::<Arc<AppState>>().inner().clone();

        let (roommate_id, roommate_name) = self.roommate_info(source_id)?;

        let characters = state.characters.read();
        let source_instance = characters.get(source_id)?;
        let roommate_instance = characters.get(&roommate_id)?;

        let source_rel = source_instance.brain.psychology.relationship();
        let roommate_rel = roommate_instance.brain.psychology.relationship();

        crate::psychology::social_state::social_state().format_for_prompt(
            &source_rel,
            &roommate_rel,
            source_id,
            &roommate_id,
            "", // source_name 未使用
            &roommate_name,
            lang,
        )
    }

    /// 生成关系认知事实 prompt 段落（"A 眼中的 B"陈述性认知）
    ///
    /// 从 RelationshipFactsEngine 读取当前角色对室友的认知事实。
    /// 无室友时返回 None。
    pub fn relationship_facts_text(&self, source_id: &str, lang: &str) -> Option<String> {
        let (roommate_id, _) = self.roommate_info(source_id)?;
        crate::psychology::relationship_facts::relationship_facts().format_for_prompt(
            source_id,
            &roommate_id,
            8,
            lang,
        )
    }

    /// 生成室友认知印象段落（从 Private Mind 派生的行为印象，不暴露原始数据）
    ///
    /// 设计原则：不暴露 Belief / Goal / Memory 的原始结构，只分享"行为印象"——
    /// 类似真实室友通过观察得出的感觉，而非读心术。
    ///
    /// 数据来源：
    /// - 注意力焦点 → "似乎关注什么"（top-3 注意力实体）
    /// - 当前活动   → "似乎在做什么"（ActivityKind + 上下文）
    /// - 活跃目标   → "似乎在忙什么"（top-1 高优先级目标描述）
    /// - 在场状态   → "社交意愿"（Rest/Busy 时降低）
    pub fn roommate_cognitive_text(&self, source_id: &str, lang: &str) -> Option<String> {
        let handle = self.app_handle.read().clone()?;
        let state = handle.state::<Arc<AppState>>().inner().clone();
        let characters = state.characters.read();
        let roommate = characters.values().find(|c| c.id != source_id)?;

        if !*roommate.online.read() {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let header = crate::pipeline::prompt_modules::section_heading("roommate_cognitive", lang);
        let (att_label, state_label, busy_label, willingness_label,
             kind_talking, kind_focusing, kind_observing, kind_thinking, kind_bg, kind_companion, kind_idle,
             will_rest, will_busy, will_offline, will_normal,
             impression_prefix, impression_suffix) = match lang_norm {
            "en" => (
                "Seems focused on", "Current state", "Seems busy with", "Social willingness",
                "chatting", "focusing", "observing", "thinking", "background task", "keeping company", "idle",
                "low (resting)", "low (busy)", "unreachable", "normal",
                "Your roommate ", "'s behavioral impression:\n",
            ),
            "ja" => (
                "注目していそう", "現在の状態", "忙しそう", "社交意欲",
                "雑談", "集中", "観察", "思考", "バックグラウンド", "付き添い", "アイドル",
                "低（休息中）", "低（忙しい）", "連絡不可", "普通",
                "ルームメイト ", " の行動印象：\n",
            ),
            _ => (
                "似乎在关注", "当前状态", "似乎在忙", "社交意愿",
                "聊天", "专注", "观察", "思考", "后台任务", "陪伴", "空闲",
                "低（在休息）", "低（在忙）", "无法联系", "正常",
                "你的室友 ", " 的行为印象：\n",
            ),
        };
        let sep = match lang_norm { "en" => ", ", _ => "、" };

        let mut signals: Vec<String> = Vec::new();

        // 1. 注意力焦点 → "似乎在关注什么"
        let att_top = roommate.brain.mind.attention_top_n(3);
        if !att_top.is_empty() {
            let items: Vec<&str> = att_top.iter()
                .filter(|(_, w)| *w > 0.2)
                .map(|(k, _)| k.as_str())
                .collect();
            if !items.is_empty() {
                signals.push(format!("- {}：{}", att_label, items.join(sep)));
            }
        }

        // 2. 当前活动 → "似乎在做什么"
        let activity = roommate.brain.mind.current_activity.snapshot();
        if activity.kind != crate::mind::current_activity::ActivityKind::Idle {
            let kind_label = match activity.kind {
                crate::mind::current_activity::ActivityKind::Talking => kind_talking,
                crate::mind::current_activity::ActivityKind::Focusing => kind_focusing,
                crate::mind::current_activity::ActivityKind::Observing => kind_observing,
                crate::mind::current_activity::ActivityKind::Thinking => kind_thinking,
                crate::mind::current_activity::ActivityKind::BackgroundTask => kind_bg,
                crate::mind::current_activity::ActivityKind::Companion => kind_companion,
                _ => kind_idle,
            };
            if activity.context.is_empty() {
                signals.push(format!("- {}：{}", state_label, kind_label));
            } else {
                signals.push(format!("- {}：{}（{}）", state_label, kind_label, activity.context));
            }
        }

        // 3. 最高优先级活跃目标 → "似乎在忙什么"（仅取 top-1，不暴露全部目标列表）
        let goals_guard = roommate.brain.mind.goals.read();
        let top_goal = goals_guard.active_top_n(1);
        if let Some(goal) = top_goal.first() {
            if goal.priority > 0.3 {
                signals.push(format!("- {}：{}", busy_label, goal.description));
            }
        }

        // 4. 社交意愿推断（从在场状态推导，不暴露 needs 数值）
        let presence = roommate.brain.presence.current();
        let willingness = match presence {
            crate::presence::PresenceState::Rest => will_rest,
            crate::presence::PresenceState::Busy => will_busy,
            crate::presence::PresenceState::Offline => will_offline,
            _ => will_normal,
        };
        signals.push(format!("- {}：{}", willingness_label, willingness));

        if signals.is_empty() {
            return None;
        }

        Some(format!(
            "{}\n{}{}{}",
            header,
            impression_prefix,
            roommate.name,
            impression_suffix,
        ) + &signals.join("\n"))
    }

    /// 发起跨角色对话
    ///
    /// 源角色 `source_id` 对目标角色 `target_id` 说 `message`，
    /// 目标角色通过 Brain.think 生成回复，流式 chunk 通过 `cross:chunk` 事件推送，
    /// 最终返回结构化的 `CrossCharacterReply`（含 response_mode 和会话状态）。
    pub async fn send(
        &self,
        app: &AppHandle,
        state: &Arc<AppState>,
        req: CrossCharacterRequest,
    ) -> VivianResult<CrossCharacterReply> {
        if req.source_id == req.target_id {
            return Err(crate::error::VivianError::Engine(
                "角色不能与自己对话".to_string(),
            ));
        }

        // ── 会话生命周期：获取或创建会话 ──
        // 返回 None 表示会话处于冷却期（抢救失败 / Closed 在创建冷却内），
        // 此时不应调用 LLM，直接返回"会话冷却中"的结构化回复。
        let conv = match crate::conversation::CONVERSATION_MANAGER.start_or_continue(
            &req.source_id,
            &req.target_id,
            &req.message,
        ) {
            Some(c) => c,
            None => {
                return Ok(CrossCharacterReply {
                    reply: String::new(),
                    response_mode: "ignore".to_string(),
                    conv_state: "cooling".to_string(),
                    should_continue: false,
                    expression: String::new(),
                    motion: String::new(),
                });
            }
        };

        // 获取目标角色实例
        let target_instance = state
            .get_character(Some(&req.target_id))
            .map_err(|e| crate::error::VivianError::Engine(e))?;
        let target_online = *target_instance.online.read();
        if !target_online {
            return Err(crate::error::VivianError::Engine(format!(
                "目标角色 {} 当前离线",
                req.target_id
            )));
        }

        // 获取源角色名称（用于事件标注和合成输入）
        let source_name = state
            .get_character(Some(&req.source_id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|_| req.source_id.clone());
        let target_name = target_instance.name.clone();

        // 通知前端：跨角色对话开始（源角色即将说话）
        let _ = app.emit(
            "cross:start",
            json!({
                "stream_id": req.stream_id,
                "speaker_id": req.source_id,
                "speaker_name": source_name,
                "listener_id": req.target_id,
                "listener_name": target_name,
                "message": req.message,
                "conv_id": conv.id,
                "conv_state": conv.state.as_str(),
            }),
        );

        // 构造目标角色的流式 emitter，将 chunk 转发为 cross:chunk 事件
        // 注意：此时 brain.think() 是在目标角色上执行的，生成的是目标角色的回复，
        // 所以 speaker_id 应为 target_id（当前说话者），而非发起对话的 source_id
        let sid_for_emitter = req.stream_id.clone();
        let speaker_id_for_emitter = req.target_id.clone();
        let listener_id_for_emitter = req.source_id.clone();
        let listener_name_for_emitter = source_name.clone();
        let app_for_emitter = app.clone();

        let emitter: StreamEmitter = Arc::new(move |chunk: &str| {
            let _ = app_for_emitter.emit(
                "cross:chunk",
                json!({
                    "text": chunk,
                    "stream_id": sid_for_emitter,
                    "speaker_id": speaker_id_for_emitter,
                    "listener_id": listener_id_for_emitter,
                    "listener_name": listener_name_for_emitter,
                }),
            );
        });

        // 获取目标角色的 think_lock：用户对话优先，跨角色对话最多等待 8 秒
        // 超时后放弃，避免源角色 LLM 长时间阻塞，并返回友好提示让对方知道"她在忙"
        let think_lock = target_instance.think_lock.clone();
        let _guard = match tokio::time::timeout(Duration::from_secs(8), think_lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                let _ = app.emit(
                    "cross:done",
                    json!({
                        "stream_id": req.stream_id,
                        "speaker_id": req.target_id,
                        "listener_id": req.source_id,
                        "response_mode": "ignore",
                        "conv_state": "target_busy",
                        "should_continue": false,
                    }),
                );
                return Ok(CrossCharacterReply {
                    reply: format!("{}现在似乎在忙，没有回应", target_name),
                    response_mode: "ignore".to_string(),
                    conv_state: "target_busy".to_string(),
                    should_continue: false,
                    expression: String::new(),
                    motion: String::new(),
                });
            }
        };

        let brain = target_instance.brain.clone();
        brain.set_stream_emitter(Some(emitter));

        // 临时切换 channel 为 cross_character，确保 brain.think 写入 dialogue 时 channel 正确
        // 完成后恢复原 channel，避免影响后续正常对话
        let original_channel = brain.dialogue.get_channel();
        brain.dialogue.set_channel("cross_character");
        let _session_guard = match state.session_coordinator.try_enter_cross_turn(
            &req.target_id,
            &conv.id,
            &brain.memory,
            &brain.dialogue,
        ) {
            Some(g) => g,
            None => {
                // 用户输入等待中：放弃跨角色对话，让位给用户
                brain.dialogue.set_channel(&original_channel);
                brain.set_stream_emitter(None);
                drop(_guard);
                let _ = app.emit(
                    "cross:done",
                    json!({
                        "stream_id": req.stream_id,
                        "speaker_id": req.target_id,
                        "listener_id": req.source_id,
                        "response_mode": "ignore",
                        "conv_state": "user_input_pending",
                        "should_continue": false,
                    }),
                );
                return Ok(CrossCharacterReply {
                    reply: format!("{}正在和用户说话，我先不打扰了", target_name),
                    response_mode: "ignore".to_string(),
                    conv_state: "user_input_pending".to_string(),
                    should_continue: false,
                    expression: String::new(),
                    motion: String::new(),
                });
            }
        };

        // 记忆触发锚点：从统一事件账本检索源角色与目标角色最近的共同事件，
        // 注入到合成输入中，让目标角色能围绕已有记忆展开对话，避免无意义空聊。
        //
        // 取条数按对话密度动态调整：最近两条间隔 < 60s（密集）只取 2 条避免冗余；
        // 间隔 ≥ 60s（稀疏）取最多 4 条以补全上下文。
        let memory_anchor = {
            let ledger = crate::memory::unified_event_ledger::unified_event_ledger();
            let mut events = ledger.events_between(&req.source_id, &req.target_id, 4);
            if events.len() >= 3 {
                let last = &events[events.len() - 1];
                let prev = &events[events.len() - 2];
                let gap = last.timestamp - prev.timestamp;
                if gap < 60.0 {
                    let start = events.len() - 2;
                    events = events[start..].to_vec();
                }
            }
            if events.is_empty() {
                String::new()
            } else {
                let anchor_lines: Vec<String> = events
                    .iter()
                    .map(|e| {
                        let preview = clean_anchor_preview(&e.content_preview);
                        if e.sender == target_name {
                            format!("- I said to {}: {}", source_name, preview)
                        } else {
                            format!("- {} said to me: {}", source_name, preview)
                        }
                    })
                    .collect();
                format!(
                    "\n\n[近期你们的话题]\n{}\n[请基于上述记忆自然地回应]",
                    anchor_lines.join("\n")
                )
            }
        };

        // Construct the "input" seen by the target character: speaking as the source character + memory anchor
        let synthesized_input = format!("[{} says to me] {}{}", source_name, req.message, memory_anchor);

        // 构建交接上下文包：打包源角色当前状态与最近用户对话，让目标角色感知源角色刚才在做什么
        // 同时注入共同情境：双方都在观察同一个用户，用户最近的活动是天然共同话题
        let handoff_text = match state.get_character(Some(&req.source_id)) {
            Ok(source_instance) => {
                let ctx = build_handoff_context(&source_instance.brain, "cross_character_reply");
                let rendered = ctx.render(&source_name);
                let activity_brief = source_instance.brain.proactive.activity_journal().to_brief();
                let shared_context = if activity_brief.is_empty() {
                    String::new()
                } else {
                    format!("\n\n[共同观察]\n你们都在看着用户做这些事：\n{}", activity_brief)
                };
                if rendered.is_empty() && shared_context.is_empty() {
                    String::new()
                } else {
                    format!("\n\n{}{}", rendered, shared_context)
                }
            }
            Err(_) => String::new(),
        };
        let synthesized_input = format!("{}{}", synthesized_input, handoff_text);

        // 获取目标角色的焦点租约：跨角色 think 期间屏蔽其他角色的主动打断
        let _focus_lease = crate::commands::proactive::FocusLeaseGuard::acquire(&req.target_id);
        let result = brain.think(&synthesized_input, true).await;
        drop(_focus_lease);

        // 恢复原 channel
        brain.dialogue.set_channel(&original_channel);
        drop(_session_guard);
        brain.set_stream_emitter(None);
        drop(_guard);

        match result {
            Ok(ai_response) => {
                let final_text = ai_response.text.clone();
                let response_mode_str = if ai_response.response_mode.is_empty() {
                    "speak".to_string()
                } else {
                    ai_response.response_mode.clone()
                };
                let response_mode = crate::conversation::ResponseMode::from_str(&response_mode_str);

                // ── 会话状态更新：根据本轮响应模式、Energy、Novelty 计算 Continuation Score ──
                let updated_conv = crate::conversation::CONVERSATION_MANAGER.update_after_round(
                    &conv.id,
                    response_mode,
                    if response_mode.needs_speech() { Some(&final_text) } else { None },
                    &req.message,
                );

                let conv_state_str = updated_conv
                    .as_ref()
                    .map(|c| c.state.as_str().to_string())
                    .unwrap_or_else(|| "closed".to_string());
                let should_continue = updated_conv
                    .as_ref()
                    .map(|c| c.state == crate::conversation::ConversationState::Active)
                    .unwrap_or(false);

                let _ = app.emit(
                    "cross:done",
                    json!({
                        "stream_id": req.stream_id,
                        "speaker_id": req.target_id,
                        "listener_id": req.source_id,
                        "listener_name": source_name,
                        "text": final_text,
                        "expression": ai_response.expression,
                        "motion": ai_response.motion,
                        "response_mode": response_mode.as_str(),
                        "conv_state": conv_state_str,
                        "should_continue": should_continue,
                    }),
                );

                // ── 源角色记忆持久化 ──
                // 目标角色已通过 brain.think 记录了对话和记忆（dialogue + memory）。
                // 源角色也必须记住这次跨角色交流：写入 dialogue（2 条消息）+ 1 条记忆，
                // 并标注 speaker/listener/perspective，供检索和中期记忆巩固使用。
                //
                // 非 speak 模式下，目标角色没有回复文本，源角色记录"她没说话/没理我"。
                if let Ok(source_instance) = state.get_character(Some(&req.source_id)) {
                    let source_brain = source_instance.brain.clone();
                    let source_dialogue = source_brain.dialogue.clone();
                    let source_memory = source_brain.memory.clone();

                    // 1. 源角色的发言（assistant 角色=源 AI 发出）
                    let mut source_msg = crate::types::response::ChatMessage::assistant(req.message.clone());
                    source_msg.meta = Some(
                        crate::messages::MessageMeta::assistant().with_channel("cross_character"),
                    );
                    let source_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.source_id,
                        "listener": req.target_id,
                        "perspective": "speaker",
                    });
                    dialogue_add_with_meta(&source_dialogue, source_msg, source_meta);

                    // 2. 目标角色的反馈（user 角色=源 AI 接收的输入）
                    //    非 speak 模式下 final_text 为空，用 response_mode 标注反馈类型
                    let target_feedback = if response_mode.needs_speech() {
                        final_text.clone()
                    } else {
                        match response_mode {
                            crate::conversation::ResponseMode::NonVerbal => {
                                format!("（{}没有说话，做了一个动作回应）", target_name)
                            }
                            crate::conversation::ResponseMode::Internal => {
                                format!("（{}听到了，但没有回应，似乎在思考）", target_name)
                            }
                            crate::conversation::ResponseMode::Ignore => {
                                format!("（{}没有理我）", target_name)
                            }
                            _ => final_text.clone(),
                        }
                    };
                    let mut target_msg = crate::types::response::ChatMessage::user(target_feedback.clone());
                    target_msg.meta = Some(
                        crate::messages::MessageMeta::user().with_channel("cross_character"),
                    );
                    let target_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.target_id,
                        "listener": req.source_id,
                        "perspective": "listener",
                        "response_mode": response_mode.as_str(),
                    });
                    dialogue_add_with_meta(&source_dialogue, target_msg, target_meta);

                    // 2.5. 源角色逐条 ShortTerm 记忆：双方对话各一条，确保时间线显示完整对话
                    // 目标角色通过 brain.think 流水线自动写入 ShortTerm，源角色需要手动补写
                    let source_shortterm_tags = vec![
                        "short_term".to_string(),
                        "cross_character".to_string(),
                        "dialogue_turn".to_string(),
                        "assistant".to_string(),
                    ];
                    let source_shortterm_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.source_id,
                        "listener": req.target_id,
                        "perspective": "speaker",
                        "knowledge_source": "direct",
                    });
                    let target_shortterm_tags = vec![
                        "short_term".to_string(),
                        "cross_character".to_string(),
                        "dialogue_turn".to_string(),
                        "user".to_string(),
                    ];
                    let target_shortterm_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.target_id,
                        "listener": req.source_id,
                        "perspective": "listener",
                        "knowledge_source": "heard",
                        "response_mode": response_mode.as_str(),
                    });
                    let mem_st = source_memory.clone();
                    let msg_st = req.message.clone();
                    let fb_st = target_feedback.clone();
                    let st_src = source_shortterm_tags.clone();
                    let st_tgt = target_shortterm_tags.clone();
                    let meta_st_src = source_shortterm_meta.clone();
                    let meta_st_tgt = target_shortterm_meta.clone();
                    let src_id = req.source_id.clone();
                    let tgt_id = req.target_id.clone();
                    tokio::spawn(async move {
                        use crate::memory::types::MemoryType;
                        // 源角色自己的发言：[I say to {Target}]
                        let src_prefix = build_speaker_prefix(&src_id, &tgt_id, &src_id);
                        let src_content = format!("{} {}", src_prefix, msg_st);
                        if let Err(e) = mem_st
                            .add_memory_with_metadata(
                                &src_content,
                                MemoryType::ShortTerm,
                                0.2,
                                st_src,
                                meta_st_src,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[CrossCharacter] 源角色写入自己的发言 ShortTerm 失败: {}",
                                e
                            );
                        }
                        // 目标角色的回复：[{Target} says to me]
                        let tgt_prefix = build_speaker_prefix(&tgt_id, &src_id, &src_id);
                        let tgt_content = format!("{} {}", tgt_prefix, fb_st);
                        if let Err(e) = mem_st
                            .add_memory_with_metadata(
                                &tgt_content,
                                MemoryType::ShortTerm,
                                0.2,
                                st_tgt,
                                meta_st_tgt,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[CrossCharacter] 源角色写入对方回复 ShortTerm 失败: {}",
                                e
                            );
                        }
                    });

                    // 3. 源角色记忆：以源角色视角记录这次交流
                    let memory_content = if response_mode.needs_speech() {
                        format!(
                            "我和 {} 聊了聊：我对她说：{}；她回复我：{}",
                            target_name, req.message, final_text
                        )
                    } else {
                        format!(
                            "我对 {} 说了：{}；她{}",
                            target_name,
                            req.message,
                            match response_mode {
                                crate::conversation::ResponseMode::NonVerbal => "没有说话，只是做了一个动作回应".to_string(),
                                crate::conversation::ResponseMode::Internal => "听到了但没有回应，似乎在思考".to_string(),
                                crate::conversation::ResponseMode::Ignore => "没有理我".to_string(),
                                _ => "没有回应".to_string(),
                            }
                        )
                    };
                    let memory_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.source_id,
                        "listener": req.target_id,
                        "perspective": "speaker",
                        "response_mode": response_mode.as_str(),
                    });
                    let sid = req.source_id.clone();
                    let tid = req.target_id.clone();
                    let mem_for_spawn = source_memory.clone();
                    let content_for_spawn = memory_content;
                    tokio::spawn(async move {
                        use crate::memory::types::MemoryType;
                        if let Err(e) = mem_for_spawn
                            .add_memory_with_metadata(
                                &content_for_spawn,
                                MemoryType::CasualConversation,
                                0.45,
                                vec!["cross_character".to_string(), "dialogue".to_string(), "topic_summary".to_string()],
                                memory_meta,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[CrossCharacter] 源角色 {} 写入跨角色对话记忆失败: {}",
                                sid,
                                e
                            );
                        } else {
                            tracing::debug!(
                                "[CrossCharacter] 源角色 {} 已记录与 {} 的跨角色对话记忆",
                                sid,
                                tid
                            );
                        }
                    });

                    // 4. 目标角色也补写一条带 speaker/listener 标注的记忆
                    //    非 speak 模式下，目标角色自己 think 时 text 已被清空，
                    //    这里补一条带 response_mode 元数据的记忆，便于后续巩固。
                    let target_memory = target_instance.brain.memory.clone();
                    let target_mem_content = if response_mode.needs_speech() {
                        format!(
                            "{} 和我聊天：她说：{}；我回复她：{}",
                            source_name, req.message, final_text
                        )
                    } else {
                        format!(
                            "{} 对我说：{}；我{}",
                            source_name,
                            req.message,
                            match response_mode {
                                crate::conversation::ResponseMode::NonVerbal => "没有说话，只是做了一个动作回应".to_string(),
                                crate::conversation::ResponseMode::Internal => "听到了但没有回应，在心里记下了".to_string(),
                                crate::conversation::ResponseMode::Ignore => "没有回应".to_string(),
                                _ => "没有回应".to_string(),
                            }
                        )
                    };
                    let target_mem_meta = json!({
                        "channel": "cross_character",
                        "speaker": req.target_id,
                        "listener": req.source_id,
                        "perspective": "speaker",
                        "response_mode": response_mode.as_str(),
                    });
                    let tid2 = req.target_id.clone();
                    let sid2 = req.source_id.clone();
                    let content2 = target_mem_content;
                    tokio::spawn(async move {
                        use crate::memory::types::MemoryType;
                        if let Err(e) = target_memory
                            .add_memory_with_metadata(
                                &content2,
                                MemoryType::CasualConversation,
                                0.45,
                                vec!["cross_character".to_string(), "dialogue".to_string(), "topic_summary".to_string()],
                                target_mem_meta,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[CrossCharacter] 目标角色 {} 写入跨角色对话记忆失败: {}",
                                tid2,
                                e
                            );
                        } else {
                            tracing::debug!(
                                "[CrossCharacter] 目标角色 {} 已补记与 {} 的跨角色对话记忆",
                                tid2, sid2
                            );
                        }
                    });

                    // 5. 写入 AgentAgent 关系日志（记录跨角色关系信号）
                    // 源角色主动发起与目标角色的对话，记录一条 AgentAgent 方向的关系信号。
                    // 关系日志为全局共享，通过 target_agent_id 区分目标角色。
                    let rel_log = crate::psychology::relationship_log::relationship_log();
                    let rel_entry = crate::psychology::relationship_log::RelationshipLogEntry {
                        id: format!(
                            "agent-agent-{}-{}",
                            chrono::Utc::now().timestamp_millis(),
                            rand::random::<u32>()
                        ),
                        date: crate::psychology::relationship_log::today_date_str(),
                        created_at: chrono::Utc::now().timestamp() as f64,
                        user_mood: String::new(),
                        relationship_signal: format!(
                            "主动联系 {}：{}",
                            target_name,
                            truncate_chars(&req.message, 30)
                        ),
                        important_moment: None,
                        next_care_cue: String::new(),
                        direction:
                            crate::psychology::relationship_log::RelationshipDirection::AgentAgent,
                        target_agent_id: Some(req.target_id.clone()),
                    };
                    if let Err(e) = rel_log.append_entry(rel_entry) {
                        tracing::warn!("[CrossCharacter] 写入 AgentAgent 关系日志失败: {}", e);
                    }

                    // 6. 更新 A↔B 关系数值（Social State）
                    // sentiment 从源角色消息（发起方态度）提取，而非目标回复。
                    // 符合"relationship sentiment analysis must use user input emotion"约定：
                    // 跨角色场景中源角色是发起方（相当于 user 角色）。
                    let sentiment =
                        crate::psychology::social_state::sentiment_from_signal_text(&req.message);
                    let rel_delta =
                        crate::psychology::social_state::deltas_from_cross_character_sentiment(
                            sentiment,
                        );
                    if let Err(e) = crate::psychology::social_state::social_state().apply_delta(
                        &req.source_id,
                        &req.target_id,
                        &rel_delta,
                    ) {
                        tracing::warn!("[CrossCharacter] 更新 A↔B 关系数值失败: {}", e);
                    }

                    // 7. 异步抽取关系认知事实（RelationshipFacts）
                    // spawn LLM 调用，不阻塞主流程。LLM 不可用时跳过。
                    // 非 speak 模式下 final_text 为空，跳过事实抽取（无内容可分析）。
                    if response_mode.needs_speech() && !final_text.is_empty() {
                        let source_id_for_facts = req.source_id.clone();
                        let target_id_for_facts = req.target_id.clone();
                        let source_msg_for_facts = req.message.clone();
                        let target_reply_for_facts = final_text.clone();
                        let target_name_for_facts = target_name.clone();
                        let handle_for_facts = app.clone();
                        tokio::spawn(async move {
                            extract_relationship_facts(
                                &handle_for_facts,
                                &source_id_for_facts,
                                &target_id_for_facts,
                                &target_name_for_facts,
                                &source_msg_for_facts,
                                &target_reply_for_facts,
                            )
                            .await;
                        });
                    }
                }

                // 跨角色对话完成：更新双方的 LAST_SPOKEN 和 LAST_SPOKEN_TEXT，
                // 让 CrossCharacterReply 触发器能感知到室友最近和谁聊过天。
                // 否则非 leader 角色（只发跨角色消息）的 LAST_SPOKEN 永远为空，
                // 导致 leader 切换后对方永远无法触发跨角色回复，形成死锁。
                // 源角色总是说了话（req.message）；目标角色仅在 speak 模式下有 final_text。
                crate::commands::proactive::record_cross_character_spoken(&req.source_id, &req.message);
                if response_mode.needs_speech() && !final_text.is_empty() {
                    crate::commands::proactive::record_cross_character_spoken(&req.target_id, &final_text);
                } else {
                    // 非 speak 模式：目标角色参与了交流但没说话，仅更新时间戳不覆盖文本
                    crate::commands::proactive::touch_last_spoken(&req.target_id);
                }

                Ok(CrossCharacterReply {
                    reply: final_text,
                    response_mode: response_mode.as_str().to_string(),
                    conv_state: conv_state_str,
                    should_continue,
                    expression: ai_response.expression,
                    motion: ai_response.motion,
                })
            }
            Err(e) => {
                let _ = app.emit(
                    "cross:error",
                    json!({
                        "stream_id": req.stream_id,
                        "source_id": req.source_id,
                        "target_id": req.target_id,
                        "error": e.to_string(),
                    }),
                );
                Err(e)
            }
        }
    }
}

/// 生成跨角色对话的 stream_id
pub fn generate_cross_stream_id() -> String {
    format!(
        "cross-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        rand::random::<u32>()
    )
}

/// 将秒数格式化为简短的中文时长（如 "3分20秒"、"1小时5分"、"刚刚"）
fn format_duration_short(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 5 {
        return "刚刚".to_string();
    }
    if s < 60 {
        return format!("{}秒", s);
    }
    let m = s / 60;
    let remain_s = s % 60;
    if m < 60 {
        return if remain_s == 0 {
            format!("{}分", m)
        } else {
            format!("{}分{}秒", m, remain_s)
        };
    }
    let h = m / 60;
    let remain_m = m % 60;
    if h < 24 {
        return if remain_m == 0 {
            format!("{}小时", h)
        } else {
            format!("{}小时{}分", h, remain_m)
        };
    }
    let d = h / 24;
    let remain_h = h % 24;
    if remain_h == 0 {
        format!("{}天", d)
    } else {
        format!("{}天{}小时", d, remain_h)
    }
}

/// 向对话管理器写入带 metadata 的消息（跨角色对话标注 speaker/listener/perspective）
fn dialogue_add_with_meta(
    dialogue: &Arc<crate::dialogue::DialogueManager>,
    msg: crate::types::response::ChatMessage,
    metadata: Value,
) {
    dialogue.add_message_with_metadata(msg, metadata);
}

/// 异步抽取关系认知事实
///
/// 调用 LLM 分析跨角色对话内容，抽取 0-2 条"A 对 B 的认知"，
/// 写入 RelationshipFactsEngine。支持 Semantic Reinforcement：既有事实命中则合并。
async fn extract_relationship_facts(
    app: &AppHandle,
    source_id: &str,
    target_id: &str,
    target_name: &str,
    source_msg: &str,
    target_reply: &str,
) {
    use crate::memory::llm_enricher::EnricherLlmClient;
    use crate::psychology::relationship_facts::{
        relationship_facts, FactCategory, RelationshipFact,
    };

    let state = app.state::<Arc<AppState>>().inner().clone();
    let llm_opt = state.model_router.read().clone();
    let llm = match llm_opt {
        Some(router) => router,
        None => {
            tracing::debug!("[CrossCharacter] ModelRouter 未初始化，跳过关系认知抽取");
            return;
        }
    };

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let prompt = match lang_norm {
        "en" => format!(
            r#"You are a relationship cognition extractor. Analyze the following conversation between two agents and extract the source agent's cognitive facts about the target agent.

The source agent ({source}) said to the target agent ({target}): {source_msg}
The target agent ({target}) replied: {target_reply}

Please extract 0-2 cognitive facts the source agent has about the target agent (personality traits, preferences, habits, specific event impressions).
State each fact in a single sentence, with the subject omitted (defaults to the target agent).

Return a JSON array:
[{{"fact_text": "She is tough on the surface but very caring", "category": "personality", "confidence": 0.8}}]

Allowed values for category: personality / preference / habit / incident
If there are no valuable cognitive facts, return an empty array []"#,
            source = source_id,
            target = target_name,
            source_msg = truncate_chars(&source_msg, 200),
            target_reply = truncate_chars(&target_reply, 200),
        ),
        "ja" => format!(
            r#"あなたは関係認知抽出器です。以下の2つのエージェント間の会話を分析し、ソースエージェントからターゲットエージェントへの認知事実を抽出してください。

ソースエージェント（{source}）がターゲットエージェント（{target}）に言った：{source_msg}
ターゲットエージェント（{target}）の返信：{target_reply}

ソースエージェントからターゲットエージェントへの認知（人格特質、偏好、習慣、具体的な事件印象）を0-2件抽出してください。
各認知は1文で陳述し、主語は省略する（デフォルトはターゲットエージェント）。

JSON配列を返す：
[{{"fact_text": "彼女は強がるがとても面倒見がいい", "category": "personality", "confidence": 0.8}}]

category の可能値：personality / preference / habit / incident
価値ある認知がない場合は、空配列 [] を返す"#,
            source = source_id,
            target = target_name,
            source_msg = truncate_chars(&source_msg, 200),
            target_reply = truncate_chars(&target_reply, 200),
        ),
        _ => format!(
            r#"你是一个关系认知抽取器。分析以下两个智能体之间的对话，抽取源角色对目标角色的认知事实。

源角色（{source}）对目标角色（{target}）说：{source_msg}
目标角色（{target}）回复：{target_reply}

请抽取 0-2 条源角色对目标角色的认知（人格特质、偏好、习惯、具体事件印象）。
每条认知用一句话陈述，主语省略（默认是目标角色）。

返回 JSON 数组：
[{{"fact_text": "她嘴硬但很关心人", "category": "personality", "confidence": 0.8}}]

category 可选值：personality / preference / habit / incident
如果没有有价值的认知，返回空数组 []"#,
            source = source_id,
            target = target_name,
            source_msg = truncate_chars(&source_msg, 200),
            target_reply = truncate_chars(&target_reply, 200),
        ),
    };

    let response = match llm.complete(&prompt).await {
        Ok(text) => text,
        Err(e) => {
            tracing::debug!(
                "[CrossCharacter] 关系认知抽取 LLM 调用失败，跳过: {}",
                e
            );
            return;
        }
    };

    let facts: Vec<ExtractedFact> = match serde_json::from_str(
        strip_code_fence(&response).trim(),
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                "[CrossCharacter] 关系认知抽取 JSON 解析失败，跳过: {}",
                e
            );
            return;
        }
    };

    let engine = relationship_facts();
    let now = chrono::Utc::now().timestamp() as f64;

    for fact in facts.into_iter().take(2) {
        let category = match fact.category.as_str() {
            "personality" => FactCategory::Personality,
            "preference" => FactCategory::Preference,
            "habit" => FactCategory::Habit,
            "incident" => FactCategory::Incident,
            _ => FactCategory::Personality,
        };

        let source_event_id = format!("cross-{}-{}", now as u64, rand::random::<u32>());

        // 文本相似度匹配：如果新事实文本与既有事实有显著重叠，则强化既有事实
        let existing_facts = engine.list_for(source_id, target_id);
        let similar = existing_facts.iter().find(|e| {
            e.category == category
                && (e.fact_text.contains(&fact.fact_text)
                    || fact.fact_text.contains(&e.fact_text)
                    || text_overlap_ratio(&e.fact_text, &fact.fact_text) > 0.5)
        });

        if let Some(existing) = similar {
            if let Err(e) = engine.reinforce_fact(&existing.id, source_event_id) {
                tracing::warn!("[CrossCharacter] 强化关系认知失败: {}", e);
            }
        } else {
            let new_fact = RelationshipFact {
                id: format!("fact-{}-{}", now as u64, rand::random::<u32>()),
                owner_agent: source_id.to_string(),
                target_agent: target_id.to_string(),
                fact_text: fact.fact_text,
                category,
                confidence: fact.confidence.unwrap_or(0.7).clamp(0.0, 1.0),
                source_event_ids: vec![source_event_id],
                created_at: now,
                last_reinforced_at: now,
                reinforcement_count: 0,
            };
            if let Err(e) = engine.append_fact(new_fact) {
                tracing::warn!("[CrossCharacter] 写入关系认知失败: {}", e);
            }
        }
    }
}

/// 计算两个文本的 2-gram 重叠比例（Jaccard 相似度）
///
/// 使用 2-gram（相邻字符对）而非单字符，显著提高短句区分度。
/// 例如 "她喜欢猫" vs "她喜欢狗" 的 2-gram 重叠率约为 0.5（共享 "她喜"/"喜欢"），
/// 而单字符 Jaccard 会误判为 0.6 相似。
fn text_overlap_ratio(a: &str, b: &str) -> f64 {
    let grams_a: std::collections::HashSet<String> = a
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w.iter().collect::<String>())
        .collect();
    let grams_b: std::collections::HashSet<String> = b
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w.iter().collect::<String>())
        .collect();
    if grams_a.is_empty() || grams_b.is_empty() {
        return 0.0;
    }
    let intersection = grams_a.intersection(&grams_b).count();
    let union = grams_a.union(&grams_b).count();
    intersection as f64 / union as f64
}

#[derive(serde::Deserialize)]
struct ExtractedFact {
    fact_text: String,
    category: String,
    #[serde(default)]
    confidence: Option<f64>,
}

/// 去除 LLM 输出可能的代码围栏
fn strip_code_fence(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            return lines[1..lines.len() - 1].join("\n");
        }
    }
    s.to_string()
}
