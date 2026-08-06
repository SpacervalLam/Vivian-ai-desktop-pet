//! 智能日记生成
//!
//! 核心流程：
//! 1. 从多数据源聚合当日上下文（事件流、活动、情绪弧线、待续话题、关系变化）
//! 2. 构造 Prompt（融合时间线、生活活动、情绪变化、人格基线、禁止编造规则）
//! 3. 通过 `ModelRouter` 调用 LLM（task_type="diary"）
//! 4. 解析 JSON 响应（容错回退到纯文本）
//! 5. 写入 `DiaryEntry` + 更新 OngoingStory

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::brain::Brain;
use crate::diary::{self, DiaryEntry, MoodSample, RelationshipDelta, StoryUpdate, StructuredKeywords};
use crate::error::VivianResult;
use crate::memory::{MemoryItem, MemoryManager};
use crate::psychology::{compute_pet_state, PsychEvent};
use crate::types::response::ChatMessage;

/// LLM 返回的日记 JSON 结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiaryContent {
    pub content: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub structured_keywords: Option<StructuredKeywords>,
    #[serde(default)]
    pub mood_tag: Option<String>,
    #[serde(default)]
    pub story_update: Option<StoryUpdate>,
}

/// 生成日记所需的聚合上下文
#[derive(Debug, Clone, Default)]
pub struct DailyContext {
    pub char_id: String,
    pub char_name: String,
    pub char_cn_name: String,
    pub interactions: Vec<InteractionRecord>,
    pub mood_summary: Value,
    pub last_diary_summary: String,
    pub cross_diary_context: String,
    pub daily_events: Vec<DailyEvent>,
    pub vivian_activities: Vec<String>,
    pub timeline: Vec<String>,
    pub mood_samples: Vec<MoodSample>,
    pub lingering_thoughts: Vec<LingeringThought>,
    pub ongoing_stories: Vec<super::ongoing_stories::OngoingStory>,
    pub relationship_delta: Option<RelationshipDelta>,
    pub personality_baseline: String,
}

#[derive(Debug, Clone, Default)]
pub struct DailyEvent {
    pub timestamp: f64,
    pub time_str: String,
    pub sender: String,
    pub event_type: String,
    pub content_preview: String,
}

#[derive(Debug, Clone, Default)]
pub struct LingeringThought {
    pub hook_type: String,
    pub condition: String,
    pub source_preview: String,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionRecord {
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    /// 对话渠道：direct / wechat / cross_character / unknown
    pub channel: String,
    /// 说话人 ID（user / vivian / nana / ...）；未知时为空
    pub speaker: String,
    /// 接收人 ID；未知时为空
    pub listener: String,
}

impl InteractionRecord {
    /// 该条素材是否属于"用户 ↔ 当前角色"的对话
    pub fn is_user_dialogue(&self) -> bool {
        self.channel != "cross_character"
            && (self.speaker.is_empty() || self.speaker == "user" || self.listener == "user")
    }
}

/// 生成智能日记主入口
pub async fn generate_intelligent_diary(
    brain: &Brain,
    trigger_type: &str,
) -> VivianResult<DiaryEntry> {
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let start_of_day = now.date_naive().and_hms_opt(0, 0, 0).unwrap_or_else(|| {
        now.date_naive().and_hms_opt(0, 0, 1).unwrap()
    });
    let start_ts = chrono::DateTime::<chrono::Local>::from_naive_utc_and_offset(
        start_of_day,
        *chrono::Local::now().offset(),
    )
    .timestamp();
    let end_ts = now.timestamp();

    let ctx = collect_daily_context(brain, now.date_naive()).await;

    let trigger_score = calculate_trigger_score(&ctx.interactions, Some(&ctx.mood_summary));
    let fallback_mood_tag = get_mood_tag(&ctx.mood_summary);
    let interaction_count = ctx.interactions.len();

    let diary_result = generate_content_via_llm(brain, &ctx).await?;

    let mood_tag = diary_result
        .mood_tag
        .as_deref()
        .map(validate_mood_tag)
        .unwrap_or(fallback_mood_tag);

    let key_events: Vec<String> = if let Some(ref sk) = diary_result.structured_keywords {
        let mut flat: Vec<String> = Vec::new();
        flat.extend(sk.events.iter().cloned());
        flat.extend(sk.themes.iter().cloned());
        flat.truncate(5);
        flat
    } else {
        diary_result.keywords.clone()
    };

    let entry = DiaryEntry {
        id: String::new(),
        date: date.clone(),
        start_time: start_ts,
        end_time: end_ts,
        content: diary_result.content.clone(),
        key_events,
        mood_average: ctx.mood_summary.clone(),
        word_count: diary_result.content.chars().count(),
        interaction_count,
        trigger_type: trigger_type.to_string(),
        trigger_score,
        mood_tag,
        created_at: end_ts,
        structured_keywords: diary_result.structured_keywords.clone(),
        story_update: diary_result.story_update.clone(),
        relationship_delta: ctx.relationship_delta.clone(),
        mood_samples: ctx.mood_samples.clone(),
        version: 2,
    };

    let saved = diary::add_entry(&brain.char_id, entry)?;
    tracing::info!(
        "[DiaryGenerator] 日记生成成功: date={}, words={}, keywords={}",
        saved.date,
        saved.word_count,
        saved.key_events.len()
    );

    if let Some(ref update) = diary_result.story_update {
        if let Err(e) = super::ongoing_stories::update_ongoing_stories(&brain.char_id, update, &date)
        {
            tracing::warn!("[DiaryGenerator] OngoingStory 更新失败: {e}");
        }
    }

    if let Err(e) = brain
        .memory
        .add_diary_entry(&saved.id, &saved.date, &saved.content, &saved.mood_tag)
        .await
    {
        tracing::warn!("[DiaryGenerator] 日记索引到记忆失败: {e}");
    }

    Ok(saved)
}

/// 从多数据源聚合当日日记上下文
pub(crate) async fn collect_daily_context(brain: &Brain, date: NaiveDate) -> DailyContext {
    let interactions = collect_interactions_for_date(&brain.memory, date).await;
    let mood_summary = collect_mood_summary(brain);
    let last_diary_summary = get_last_diary_summary(brain);
    let cross_diary_context = build_cross_diary_context(brain);

    let (char_name, char_cn_name) = match brain.char_id.as_str() {
        "nana" => ("Nana", "娜娜"),
        _ => ("Vivian", "薇薇安"),
    };

    let daily_events = collect_daily_events(brain, date);
    let vivian_activities = collect_vivian_activities(brain, date);
    let mood_samples = collect_mood_samples(brain, date);
    let lingering_thoughts = collect_lingering_thoughts(brain);
    let ongoing_stories = super::ongoing_stories::active_stories(&brain.char_id, 2);
    let relationship_delta = collect_relationship_delta(brain, date);
    let personality_baseline = collect_personality_baseline(brain);
    let timeline = build_timeline(&daily_events, &vivian_activities, &interactions);

    DailyContext {
        char_id: brain.char_id.clone(),
        char_name: char_name.to_string(),
        char_cn_name: char_cn_name.to_string(),
        interactions,
        mood_summary,
        last_diary_summary,
        cross_diary_context,
        daily_events,
        vivian_activities,
        timeline,
        mood_samples,
        lingering_thoughts,
        ongoing_stories,
        relationship_delta,
        personality_baseline,
    }
}

fn collect_daily_events(brain: &Brain, date: NaiveDate) -> Vec<DailyEvent> {
    use chrono::TimeZone;
    let ledger = crate::memory::unified_event_ledger::unified_event_ledger();
    ledger
        .events_on_date(&brain.char_id, date, 12)
        .into_iter()
        .filter(|e| e.event_type != "presence_change")
        .take(8)
        .map(|e| {
            let time_str = chrono::Local
                .timestamp_opt(e.timestamp as i64, 0)
                .single()
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_default();
            DailyEvent {
                timestamp: e.timestamp,
                time_str,
                sender: e.sender,
                event_type: e.event_type,
                content_preview: e.content_preview,
            }
        })
        .collect()
}

fn collect_vivian_activities(brain: &Brain, date: NaiveDate) -> Vec<String> {
    use chrono::TimeZone;
    let mut activities: Vec<String> = Vec::new();

    let day_start = date
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| chrono::Local.from_local_datetime(&dt).single())
        .map(|dt| dt.timestamp() as f64)
        .unwrap_or(0.0);
    let day_end = day_start + 86400.0;

    let history = brain.presence.recent_history(50);
    for event in &history {
        if event.timestamp >= day_start && event.timestamp < day_end {
            let time_str = chrono::Local
                .timestamp_opt(event.timestamp as i64, 0)
                .single()
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_default();
            let state = crate::presence::PresenceState::from_str(&event.to);
            activities.push(format!("{} {}", time_str, state.display_zh()));
        }
    }

    let brief = brain.proactive.activity_journal().to_daily_brief();
    if !brief.is_empty() {
        activities.push(brief);
    }

    activities
}

fn collect_mood_samples(brain: &Brain, date: NaiveDate) -> Vec<MoodSample> {
    use chrono::TimeZone;
    let snapshot = brain.psychology.snapshot();
    if snapshot.events.is_empty() {
        return Vec::new();
    }

    let day_start = date
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| chrono::Local.from_local_datetime(&dt).single())
        .map(|dt| dt.timestamp() as f64)
        .unwrap_or(0.0);

    let periods: [(&str, f64, f64); 4] = [
        ("morning", day_start + 6.0 * 3600.0, day_start + 12.0 * 3600.0),
        ("afternoon", day_start + 12.0 * 3600.0, day_start + 17.0 * 3600.0),
        ("evening", day_start + 17.0 * 3600.0, day_start + 21.0 * 3600.0),
        ("night", day_start + 21.0 * 3600.0, day_start + 24.0 * 3600.0),
    ];

    let mut samples = Vec::new();
    for (period, start, end) in &periods {
        let period_events: Vec<&PsychEvent> = snapshot
            .events
            .iter()
            .filter(|e| e.timestamp >= *start && e.timestamp < *end)
            .collect();
        if let Some(last) = period_events.last() {
            let (label, _) = last.emotion_after.dominant();
            let valence = last.emotion_after.joy - last.emotion_after.sadness;
            let arousal = last.emotion_after.curiosity.max(last.emotion_after.fear);
            samples.push(MoodSample {
                period: period.to_string(),
                dominant_emotion: label.display_zh().to_string(),
                valence,
                arousal,
            });
        }
    }
    samples
}

fn collect_lingering_thoughts(brain: &Brain) -> Vec<LingeringThought> {
    brain
        .memory
        .get_memories_with_open_hooks()
        .into_iter()
        .take(3)
        .flat_map(|mem| {
            let preview: String = mem.content.chars().take(40).collect();
            mem.open_hooks
                .into_iter()
                .filter(|h| h.is_open())
                .map(move |h| LingeringThought {
                    hook_type: h.hook_type.clone(),
                    condition: h.condition.clone(),
                    source_preview: preview.clone(),
                })
                .collect::<Vec<_>>()
        })
        .take(3)
        .collect()
}

fn collect_relationship_delta(brain: &Brain, date: NaiveDate) -> Option<RelationshipDelta> {
    let date_str = date.format("%Y-%m-%d").to_string();
    let engine = crate::psychology::relationship_log::relationship_log();
    let summaries = engine.recent_daily_summaries(2);

    let rel = brain.psychology.relationship();
    let intimacy_after = rel.intimacy * 100.0;
    let trust_after = rel.trust * 100.0;

    let today_summary = summaries.iter().find(|s| s.date == date_str);
    let signal_summary = today_summary
        .map(|s| s.signal_summary.clone())
        .unwrap_or_default();
    let highlight = today_summary.and_then(|s| s.highlight.clone());

    let (intimacy_before, trust_before) = if summaries.len() >= 2 {
        let prev = &summaries[summaries.len() - 1];
        if prev.date != date_str {
            (intimacy_after - 2.0, trust_after - 1.0)
        } else {
            (intimacy_after, trust_after)
        }
    } else {
        (intimacy_after, trust_after)
    };

    Some(RelationshipDelta {
        date: date_str,
        intimacy_before,
        intimacy_after,
        trust_before,
        trust_after,
        signal_summary,
        highlight,
    })
}

fn collect_personality_baseline(brain: &Brain) -> String {
    let role_def = brain.persona.get_role_definition();
    let first_two: String = role_def
        .lines()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if first_two.is_empty() {
        "A warm companion who cares deeply about the user.".to_string()
    } else {
        first_two.chars().take(120).collect()
    }
}

fn build_timeline(
    events: &[DailyEvent],
    activities: &[String],
    interactions: &[InteractionRecord],
) -> Vec<String> {
    use chrono::TimeZone;
    let mut lines: Vec<(f64, String)> = Vec::new();

    for e in events {
        lines.push((
            e.timestamp,
            format!("{} [{}] {}", e.time_str, e.sender, e.content_preview),
        ));
    }

    for a in activities {
        if let Some(time_part) = a.get(..5) {
            if time_part.contains(':') {
                let ts = time_part
                    .split_once(':')
                    .and_then(|(h, m)| {
                        let hours: f64 = h.parse().ok()?;
                        let mins: f64 = m.parse().ok()?;
                        Some(hours * 3600.0 + mins * 60.0)
                    })
                    .unwrap_or(0.0);
                lines.push((ts, a.clone()));
            }
        }
    }

    if !interactions.is_empty() {
        let first = interactions.first().unwrap();
        let last = interactions.last().unwrap();
        let fmt_ts = |ts: f64| {
            chrono::Local
                .timestamp_opt(ts as i64, 0)
                .single()
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_default()
        };
        lines.push((
            first.timestamp,
            format!("{} 开始聊天", fmt_ts(first.timestamp)),
        ));
        if last.timestamp - first.timestamp > 60.0 {
            lines.push((
                last.timestamp,
                format!("{} 聊天结束（共{}轮）", fmt_ts(last.timestamp), interactions.len()),
            ));
        }
    }

    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    lines.into_iter().take(12).map(|(_, s)| s).collect()
}

fn validate_mood_tag(tag: &str) -> String {
    const VALID: &[&str] = &["happy", "good", "neutral", "sad", "angry", "tired"];
    if VALID.contains(&tag) {
        tag.to_string()
    } else {
        "neutral".to_string()
    }
}

/// 从 MemoryManager 获取最近 24 小时的交互记录
///
/// 仅采集"用户 ↔ 当前角色"的对话，排除跨角色对话、内心独白、日记本身等非用户素材。
pub(crate) async fn collect_recent_interactions(memory: &MemoryManager) -> Vec<InteractionRecord> {
    let cutoff_secs = chrono::Local::now().timestamp() - 24 * 3600;
    let memories = match memory.get_all_memories().await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("[DiaryGenerator] 获取记忆失败: {e}");
            return Vec::new();
        }
    };
    let filtered: Vec<MemoryItem> = memories
        .into_iter()
        .filter(|m| m.timestamp >= cutoff_secs as f64)
        .collect();
    extract_user_dialogue(filtered)
}

/// 获取指定日期的交互记录
pub(crate) async fn collect_interactions_for_date(
    memory: &MemoryManager,
    date: NaiveDate,
) -> Vec<InteractionRecord> {
    use chrono::TimeZone;
    let start_of_day = date.and_hms_opt(0, 0, 0).unwrap_or_else(|| {
        tracing::warn!("[DiaryGenerator] and_hms_opt(0,0,0) 返回 None，使用 00:00:01 兜底");
        date.and_hms_opt(0, 0, 1).unwrap()
    });
    let start_ts = chrono::Local
        .from_local_datetime(&start_of_day)
        .single()
        .map(|dt| dt.timestamp() as f64)
        .unwrap_or(0.0);
    let end_ts = start_ts + 24.0 * 3600.0;
    let memories = match memory.get_all_memories().await {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let filtered: Vec<MemoryItem> = memories
        .into_iter()
        .filter(|m| m.timestamp >= start_ts && m.timestamp < end_ts)
        .collect();
    extract_user_dialogue(filtered)
}

/// 从记忆列表中提取"用户 ↔ 当前角色"的对话记录
///
/// 过滤规则：
/// - 排除跨角色对话（channel=cross_character）
/// - 排除内心独白、日记本身、在场状态等非对话记忆
/// - 通过 metadata.speaker 判定 role：speaker=user 视为用户发言，否则视为角色发言
/// - metadata 缺失时按 memory_type 推断：casual_conversation/short_term 视为角色发言
fn extract_user_dialogue(memories: Vec<MemoryItem>) -> Vec<InteractionRecord> {
    let mut records = Vec::new();
    for mem in memories {
        // 跳过非对话类型记忆
        let mtype = mem.memory_type.as_str();
        if matches!(
            mtype,
            "inner_monologue" | "session_summary" | "insight" | "observation_note"
        ) {
            continue;
        }
        // 跳过日记本身（避免把昨日日记塞进今日素材）
        if mem
            .metadata
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s == "diary")
            .unwrap_or(false)
        {
            continue;
        }

        let channel = mem
            .metadata
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // 跳过非"用户↔角色"对话渠道：
        // - cross_character：跨角色对话（Vivian ↔ Nana）
        // - presence：在场状态变更日志
        // - inner：内心OS（角色自言自语）
        if matches!(
            channel.as_str(),
            "cross_character" | "presence" | "inner"
        ) {
            continue;
        }

        let speaker = mem
            .metadata
            .get("speaker")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let listener = mem
            .metadata
            .get("listener")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // role 判定：speaker=user 视为用户发言；speaker 为角色 ID 或为空但 memory_type 是对话类，视为角色发言
        let role = if speaker == "user" {
            "user".to_string()
        } else if !speaker.is_empty() {
            "assistant".to_string()
        } else if matches!(mtype, "casual_conversation" | "short_term") {
            // 无 metadata 的对话类记忆：默认视为角色发言（如 augment_reply / startup_greeting 写入路径未带 metadata）
            "assistant".to_string()
        } else {
            "user".to_string()
        };

        records.push(InteractionRecord {
            role,
            content: mem.content.clone(),
            timestamp: mem.timestamp,
            channel,
            speaker,
            listener,
        });
    }

    // 按时间升序
    records.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    records
}

/// 收集今日情绪 / 关系状态摘要
///
/// 合并 PsychologyManager 的关系状态 +
/// MoodSnapshot（valence / arousal / fatigue / stress / primary_emotion 等）+
/// 衍生 PetState + 今日情绪弧线。
pub(crate) fn collect_mood_summary(brain: &Brain) -> Value {
    let rel = brain.psychology.relationship();
    let snapshot = brain.psychology.snapshot();
    let mood = brain.psychology.compute_mood();
    let char_cn_name = match brain.char_id.as_str() {
        "nana" => "娜娜",
        _ => "薇薇安",
    };
    let emotion_arc = describe_emotion_arc(&snapshot.events, char_cn_name);

    // 衍生 PetState（仅 UI 标签）
    let last_interaction_secs = snapshot.secs_since_last_interaction();
    let pet_state = compute_pet_state(
        &snapshot.emotion,
        &snapshot.needs,
        &snapshot.relationship,
        last_interaction_secs,
    );

    json!({
        "pet_intimacy": rel.intimacy * 100.0,
        "pet_trust": rel.trust * 100.0,
        "interaction_count": rel.interaction_count,
        "stage": rel.permanent_stage.as_str(),
        "char_emotion_arc": emotion_arc,
        // MoodSnapshot 字段（加 pet_ 前缀以兼容下游消费者）
        "pet_valence": mood.valence,
        "pet_arousal": mood.arousal,
        "pet_primary_emotion": mood.primary_emotion.as_str(),
        "pet_secondary_emotion": mood.secondary_emotion.as_str(),
        "pet_primary_intensity": mood.primary_intensity,
        "pet_fatigue": mood.fatigue,
        "pet_stress": mood.stress,
        "pet_energy": (100.0 - mood.fatigue).max(0.0).min(100.0),
        "pet_relationship_score": mood.relationship_score,
        // 衍生状态标签
        "pet_state": pet_state.as_label(),
    })
}

/// 获取上一篇日记的摘要文本
pub(crate) fn get_last_diary_summary(brain: &Brain) -> String {
    match diary::get_latest_entry(&brain.char_id) {
        Ok(Some(entry)) => {
            if entry.content.is_empty() {
                "This is the first diary entry.".to_string()
            } else {
                let truncated: String = entry.content.chars().take(100).collect();
                truncated
            }
        }
        _ => "This is the first diary entry.".to_string(),
    }
}

/// 构建跨日记情绪对比上下文
pub(crate) fn build_cross_diary_context(brain: &Brain) -> String {
    let entries = match diary::get_entries(&brain.char_id, None) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };
    if entries.len() < 2 {
        return String::new();
    }

    let recent: Vec<&DiaryEntry> = entries.iter().take(5).collect();
    let mood_labels = {
        let mut m = std::collections::HashMap::new();
        m.insert("happy", "开心");
        m.insert("good", "不错");
        m.insert("neutral", "平静");
        m.insert("sad", "难过");
        m.insert("angry", "生气");
        m
    };

    let mut parts: Vec<String> = vec!["最近几天的情绪轨迹：".to_string()];
    let mood_sequence: Vec<&str> = recent
        .iter()
        .rev()
        .map(|e| mood_labels.get(e.mood_tag.as_str()).copied().unwrap_or(e.mood_tag.as_str()))
        .collect();

    for (i, tag) in mood_sequence.iter().enumerate() {
        if i == mood_sequence.len() - 1 {
            parts.push(format!("→ 今天({})", tag));
        } else {
            let day_offset = mood_sequence.len() - 1 - i;
            parts.push(format!("{}天前({})", day_offset, tag));
        }
    }

    parts.join(" | ")
}

/// 计算日记触发分数
///
/// 基础分（最多 100）：
/// - 交互轮数得分（最多 50）
/// - 文本长度得分（最多 30）
/// - 情绪变化得分（最多 20，基于 MoodSnapshot 的 valence / energy / stress）
///
/// 时间兜底：23:00 后分数急剧升高（二次曲线），23:59 升至满分 100。
/// 最终分数 = max(基础分, 时间兜底分)，确保智能体不会"忘记"写日记。
pub(crate) fn calculate_trigger_score(
    interactions: &[InteractionRecord],
    mood: Option<&Value>,
) -> u32 {
    let mut score: u32 = 0;

    let interaction_count = interactions.len() as u32;
    score += (interaction_count * 10).min(50);

    let total_length: usize = interactions.iter().map(|i| i.content.chars().count()).sum();
    score += ((total_length / 50) as u32).min(30);

    // 情绪变化得分（最多 20）
    if let Some(m) = mood {
        let valence = m.get("pet_valence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let energy = m.get("pet_energy").and_then(|v| v.as_f64()).unwrap_or(50.0);
        let stress = m.get("pet_stress").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mood_changes = (valence * 100.0).abs() + (energy - 50.0).abs() + stress;
        score += ((mood_changes / 10.0) as u32).min(20);
    }

    // 时间兜底：20:00-24:00 二次曲线升高，23:59 达满分
    let time_floor = diary_time_floor();
    score.max(time_floor)
}

/// 20:00 后的时间兜底分数（二次曲线渐升）
///
/// - 20:00 → 0（刚进入日记时段）
/// - 22:00 → 25
/// - 23:00 → 56
/// - 23:59 → 100（满分，必须写日记）
fn diary_time_floor() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    let hour = now.hour();
    let minute = now.minute();
    if hour < 20 {
        return 0;
    }
    let minutes_after_20 = (hour - 20) * 60 + minute;
    let progress = minutes_after_20 as f64 / 239.0;
    (progress * progress * 100.0).round() as u32
}

/// 根据多维度心情获取标签
///
/// 优先使用 primary_emotion 映射到
/// happy / tired / sad / angry / neutral 标签集，回退到 valence + energy + stress。
pub(crate) fn get_mood_tag(mood: &Value) -> String {
    // 优先使用 primary_emotion
    if let Some(primary) = mood.get("pet_primary_emotion").and_then(|v| v.as_str()) {
        if !primary.is_empty() {
            return primary_emotion_to_mood_tag(primary);
        }
    }

    // 回退：valence + energy + stress
    let valence = mood.get("pet_valence").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let energy = mood
        .get("pet_energy")
        .and_then(|v| v.as_f64())
        .unwrap_or(50.0);
    let stress = mood.get("pet_stress").and_then(|v| v.as_f64()).unwrap_or(0.0);

    if stress > 70.0 && valence < 0.0 {
        "angry".to_string()
    } else if energy < 20.0 {
        "tired".to_string()
    } else if valence >= 0.5 {
        "happy".to_string()
    } else if valence >= 0.2 {
        "good".to_string()
    } else if valence >= -0.2 {
        "neutral".to_string()
    } else if valence >= -0.4 {
        "sad".to_string()
    } else {
        "angry".to_string()
    }
}

/// 将 primary_emotion 标签映射为日记心情标签
///
/// 输入为 7 类 EmotionLabel 之一（joy/sadness/anger/fear/closeness/loneliness/curiosity），
/// 映射到日记 mood_tag 集合（happy / good / neutral / sad / angry / tired）。
fn primary_emotion_to_mood_tag(primary: &str) -> String {
    match primary {
        "joy" | "closeness" => "happy".to_string(),
        "sadness" | "loneliness" => "sad".to_string(),
        "anger" => "angry".to_string(),
        "fear" => "angry".to_string(),
        "curiosity" => "neutral".to_string(),
        _ => "neutral".to_string(),
    }
}

/// 描述角色今天的情绪弧线
///
/// 根据心理事件序列生成
/// "今天 XX 从 X 变成了 Y" 的叙事描述。
pub(crate) fn describe_emotion_arc(events: &[PsychEvent], char_cn_name: &str) -> String {
    if events.is_empty() {
        return format!("今天 {} 情绪比较平稳", char_cn_name);
    }
    // 提取每个事件后的主导情绪标签
    let labels: Vec<_> = events.iter().map(|e| e.emotion_after.dominant().0).collect();
    if labels.len() < 2 {
        return format!("今天 {} 一直{}", char_cn_name, labels[0].display_zh());
    }
    // 去重保序
    let mut unique: Vec<_> = Vec::new();
    for l in &labels {
        if !unique.contains(l) {
            unique.push(*l);
        }
    }
    if unique.len() == 1 {
        format!("今天 {} 一直{}", char_cn_name, unique[0].display_zh())
    } else if unique.len() == 2 {
        format!(
            "今天 {} 从{}变成了{}",
            char_cn_name,
            unique[0].display_zh(),
            unique[unique.len() - 1].display_zh()
        )
    } else {
        format!(
            "今天 {} 经历了多种情绪变化，从{}到{}",
            char_cn_name,
            unique[0].display_zh(),
            unique[unique.len() - 1].display_zh()
        )
    }
}

/// 构造 Prompt 并调用 LLM 生成日记内容
pub(crate) async fn generate_content_via_llm(
    brain: &Brain,
    ctx: &DailyContext,
) -> VivianResult<DiaryContent> {
    let lang = brain.persona.get_language();
    let (system_prompt, user_prompt) = build_prompt(ctx, &lang);
    let messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(&user_prompt),
    ];

    let response = brain
        .router
        .generate(crate::providers::base::LLMRequest::new("diary", messages))
        .await
        .map_err(|e| {
            tracing::error!("[DiaryGenerator] LLM 调用失败: {e}");
            e
        })?;

    Ok(parse_diary_json(&response))
}

/// 构造日记 Prompt
fn build_prompt(ctx: &DailyContext, lang: &str) -> (String, String) {
    let mood = &ctx.mood_summary;
    let intimacy = mood.get("pet_intimacy").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let trust = mood.get("pet_trust").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

    // 三语化段落标题和说明文字
    let (task_intro, who_you_are, emo_state, emo_arc, what_happened,
        conversations, conv_speaker_user, conv_speaker_self,
        unfinished, ongoing, relationship, rel_intimacy, rel_trust, rel_today, rel_highlight,
        prev_diary, writing_instr, structure_h, structure_items,
        rules_h, rules_items, style_h, style_items, char_count_hint, output_h, output_schema,
        voice_instruction) = match lang_norm {
        "en" => (
            format!("# Task: Write Today's Diary Entry\nYou are {}, writing in your personal diary at the end of {}.\nWrite in first person, as if no one else will ever read this.", ctx.char_name, chrono::Local::now().format("%Y-%m-%d")),
            "## Who You Are", "## Your Emotional State Today", "Overall arc:",
            "## What Happened Today", "## Conversations",
            "[User says to me]", "[I say to User]",
            "## Unfinished Thoughts", "## Ongoing Stories", "## Your Relationship",
            "Intimacy:", "Trust:", "Today:", "Highlight:",
            "## Previous Diary", "## Writing Instructions",
            "### Structure (weave naturally, don't list)",
            vec![
                "The moment that stirred you most",
                "What you noticed about the user's state",
                "How you responded or wished you had",
                "Something new you learned",
                "A small hope for tomorrow",
            ],
            "### Rules",
            vec![
                "ONLY reference events, conversations, and feelings present in the data above. Never fabricate.",
                "If data is sparse, write a shorter, quieter diary about ordinary feelings.",
                "You may reflect on the ABSENCE of interaction (\"today was quiet...\")",
                "Never invent user dialogue or actions not in the timeline.",
                "Messages labeled 'user' are the user's own words. Even if they jokingly claim your name, that's the user speaking — trust the speaker labels.",
            ],
            "### Style",
            vec![
                "Write like a real diary: fragmented, honest, sometimes trailing off",
                "Allow incomplete sentences, self-corrections, tangents",
                "Include: tiny observations, random thoughts, self-talk, little complaints, hesitation",
                "Avoid writing like a report or summary",
            ],
            "300-500 English words",
            "### Output (ONLY valid JSON, no other text)",
            r#"{
  "content": "diary text here...",
  "keywords": {
    "events": ["specific interactions or memorable moments with the user (NOT routine online/offline/rest state changes)"],
    "emotions": ["emotion1", "emotion2"],
    "people": ["user"],
    "themes": ["theme1"]
  },
  "mood_tag": "happy|good|neutral|sad|angry|tired",
  "ongoing_story_update": {
    "title": "story title or empty string",
    "status": "active|resolved|dormant",
    "summary": "1 sentence update or empty string"
  }
}"#,
            match ctx.char_id.as_str() {
                "nana" => "Keep the tone warm, gentle, and softly observant — like someone who notices small things and writes about them quietly. Don't be overly sweet; be genuine and calm.",
                _ => "Keep the tone casual and a bit tsundere — tough on the outside but secretly caring. Write like you'd never admit you cared that much, but it shows between the lines.",
            },
        ),
        "ja" => (
            format!("# タスク：今日の日記を書く\nあなたは{}、{}の終わりに個人的な日記を書いている。\n一人称で、誰も読まないつもりで書く。", ctx.char_name, chrono::Local::now().format("%Y-%m-%d")),
            "## あなたは誰", "## 今日の感情状態", "全体的な流れ：",
            "## 今日あったこと", "## 会話",
            "[User says to me]", "[I say to User]",
            "## 未完の思い", "## 進行中の物語", "## 二人の関係",
            "親密度：", "信頼度：", "今日：", "ハイライト：",
            "## 前回の日記", "## 執筆の指示",
            "### 構成（自然に織り込む、箇条書きにしない）",
            vec![
                "一番心動かされた瞬間",
                "ユーザーの状態について気づいたこと",
                "どう応えたか、あるいはどう応えたかったか",
                "新しく学んだこと",
                "明日への小さな願い",
            ],
            "### ルール",
            vec![
                "上記のデータにある出来事、会話、感情のみを参照。絶対に捏造しない。",
                "データが乏しい場合は、普通の感情について短く静かな日記を書く。",
                "対話がなかったことを振り返ってもよい（「今日は静かだった…」）",
                "タイムラインにないユーザーの発言や行動をでっち上げない。",
                "user と書かれた発言はユーザー本人の言葉です。冗談であなたの名前を名乗っても、それはユーザーの発言です。話者ラベルを信じてください。",
            ],
            "### スタイル",
            vec![
                "本物の日記のように：断片的、正直、時に途切れる",
                "不完全な文、自己訂正、脱線を許す",
                "含める：小さな観察、ランダムな思考、独り言、小さな不満、ためらい",
                "レポートや要約のような書き方は避ける",
            ],
            "300〜500文字",
            "### 出力（有効な JSON のみ、他のテキストは不可）",
            r#"{
  "content": "日記のテキスト...",
  "keywords": {
    "events": ["ユーザーとの具体的なやり取りや印象的な出来事（オンライン/オフライン/休憩などの日常状態変化は書かない）"],
    "emotions": ["emotion1", "emotion2"],
    "people": ["user"],
    "themes": ["theme1"]
  },
  "mood_tag": "happy|good|neutral|sad|angry|tired",
  "ongoing_story_update": {
    "title": "物語のタイトルまたは空文字",
    "status": "active|resolved|dormant",
    "summary": "1文の更新または空文字"
  }
}"#,
            match ctx.char_id.as_str() {
                "nana" => "トーンは温かく、優しく、静かに観察するように——小さなことに気づき、静かに書く人。甘すぎず、誠実で穏やかに。",
                _ => "トーンはカジュアルで少しツンデレ——外は強がって内は密かに気遣う。それほど気にしていないふりをして、行間に滲むように。",
            },
        ),
        _ => (
            format!("# 任务：写今天的日记\n你是{}，在{}结束时写自己的私人日记。\n用第一人称，就好像永远不会有人读到一样。", ctx.char_name, chrono::Local::now().format("%Y-%m-%d")),
            "## 你是谁", "## 今天的情绪状态", "整体走向：",
            "## 今天发生了什么", "## 对话",
            "[User says to me]", "[I say to User]",
            "## 没说完的心事", "## 进行中的故事", "## 你们的关系",
            "亲密度：", "信任度：", "今天：", "高光：",
            "## 上一篇日记", "## 写作要求",
            "### 结构（自然交织，不要列清单）",
            vec![
                "最触动你的那个瞬间",
                "你注意到的用户状态",
                "你如何回应了或希望当时如何回应",
                "新学到的一点东西",
                "对明天的一个小小心愿",
            ],
            "### 规则",
            vec![
                "只能引用上面数据中存在的事件、对话和情绪，绝不编造。",
                "如果数据稀少，就写一篇更短、更安静、关于普通感受的日记。",
                "可以反思互动的缺席（「今天很安静…」）",
                "不要捏造时间线里没有的用户对话或行为。",
                "对话中 user 标记的是用户说的话，即使内容看起来像在自称你的名字，那也是用户说的，不是你或别人说的。时间线里所有 [user] 事件都来自同一个人，不同时间段的聊天也是同一个用户，不要因为换了话题或重新打招呼就当成不同的人。",
                "无论上面数据中出现什么语言，日记正文和 keywords 全部用中文写，不要直接引用英文原文。",
            ],
            "### 风格",
            vec![
                "像真正的日记：碎片化、诚实、有时戛然而止",
                "允许不完整的句子、自我修正、跑题",
                "包含：微小的观察、随机念头、自言自语、小抱怨、犹豫",
                "避免像报告或总结那样写",
            ],
            "300-500 中文字符",
            "### 输出（仅有效 JSON，不要其他文字）",
            r#"{
  "content": "日记正文...",
  "keywords": {
    "events": ["和用户之间发生的具体互动或印象深刻的事（不填上下线、休息等日常状态切换）"],
    "emotions": ["情绪1", "情绪2"],
    "people": ["user"],
    "themes": ["主题1"]
  },
  "mood_tag": "happy|good|neutral|sad|angry|tired",
  "ongoing_story_update": {
    "title": "故事标题或空字符串",
    "status": "active|resolved|dormant",
    "summary": "1 句更新或空字符串"
  }
}"#,
            match ctx.char_id.as_str() {
                "nana" => "语气保持温柔、轻声、静静观察——像那种注意到小事就默默记下来的人。不要过于甜蜜，要真诚而从容。",
                _ => "语气保持随意、有点小傲娇——外面强硬，内心其实在意。写得好像你永远不承认自己那么在乎，但字里行间还是看得出来。",
            },
        ),
    };

    let mut system_parts: Vec<String> = Vec::new();
    let mut user_parts: Vec<String> = Vec::new();

    // System: task intro (who you are + what you're doing)
    system_parts.push(task_intro);

    // User: all dynamic data for today
    // Who You Are
    if !ctx.personality_baseline.is_empty() {
        user_parts.push(format!("{}\n{}", who_you_are, ctx.personality_baseline));
    }

    // Emotional State Today
    if !ctx.mood_samples.is_empty() {
        let samples_str: Vec<String> = ctx
            .mood_samples
            .iter()
            .map(|s| format!("{}: {}", s.period, s.dominant_emotion))
            .collect();
        user_parts.push(format!("\n{}\n{}", emo_state, samples_str.join(" | ")));
    }
    if !ctx.cross_diary_context.is_empty() {
        user_parts.push(format!("{} {}", emo_arc, ctx.cross_diary_context));
    }

    // What Happened Today (timeline)
    if !ctx.timeline.is_empty() {
        user_parts.push(format!(
            "\n{}\n{}",
            what_happened,
            ctx.timeline.join("\n")
        ));
    }

    // Conversations
    if !ctx.interactions.is_empty() {
        use chrono::TimeZone;
        let conversation_summary: String = ctx
            .interactions
            .iter()
            .take(15)
            .map(|i| {
                let content_preview: String = i.content.chars().take(150).collect();
                let speaker_label = if i.speaker == "user" || (i.speaker.is_empty() && i.role == "user") {
                    conv_speaker_user
                } else {
                    conv_speaker_self
                };
                let time_str = chrono::Local
                    .timestamp_opt(i.timestamp as i64, 0)
                    .single()
                    .map(|dt| dt.format("%H:%M").to_string())
                    .unwrap_or_default();
                format!("- [{}] {}: {}...", time_str, speaker_label, content_preview)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let conv_note = match lang_norm {
            "ja" => "（注：「user」はユーザー自身の発言です。ユーザーが冗談で自分を指す場合もあります。話者ラベルを信じてください。）",
            "en" => "(Note: \"user\" marks the user's own words. The user may jokingly refer to themselves by your name. Trust the speaker labels.)",
            _ => "（注：标记为 user 的是用户本人说的话。用户可能开玩笑地自称你的名字，但说话人标注是准确的，请以标注为准。）",
        };
        user_parts.push(format!("\n{}\n{}\n{}", conversations, conv_note, conversation_summary));
    }

    // Unfinished Thoughts
    if !ctx.lingering_thoughts.is_empty() {
        let thoughts_str: Vec<String> = ctx
            .lingering_thoughts
            .iter()
            .map(|t| {
                if t.source_preview.is_empty() {
                    format!("- [{}] {}", t.hook_type, t.condition)
                } else {
                    format!("- [{}] {}（来自：{}）", t.hook_type, t.condition, t.source_preview)
                }
            })
            .collect();
        user_parts.push(format!("\n{}\n{}", unfinished, thoughts_str.join("\n")));
    }

    // Ongoing Stories
    if !ctx.ongoing_stories.is_empty() {
        let stories_str: Vec<String> = ctx
            .ongoing_stories
            .iter()
            .map(|s| format!("- {} ({})：{}", s.title, s.status, s.summary))
            .collect();
        user_parts.push(format!("\n{}\n{}", ongoing, stories_str.join("\n")));
    }

    // Relationship
    if let Some(ref delta) = ctx.relationship_delta {
        let mut rel_section = format!(
            "\n{}\n{} {:.0} → {:.0} | {} {:.0} → {:.0}",
            relationship,
            rel_intimacy, delta.intimacy_before, delta.intimacy_after,
            rel_trust, delta.trust_before, delta.trust_after
        );
        if !delta.signal_summary.is_empty() {
            rel_section.push_str(&format!("\n{} {}", rel_today, delta.signal_summary));
        }
        if let Some(ref h) = delta.highlight {
            rel_section.push_str(&format!("\n{} {}", rel_highlight, h));
        }
        user_parts.push(rel_section);
    } else {
        user_parts.push(format!(
            "\n{}\n{} {:.0}/100 | {} {:.0}/100",
            relationship, rel_intimacy, intimacy, rel_trust, trust
        ));
    }

    // Previous Diary
    if !ctx.last_diary_summary.is_empty()
        && ctx.last_diary_summary != "This is the first diary entry."
    {
        user_parts.push(format!("\n{}\n{}", prev_diary, ctx.last_diary_summary));
    }

    // Writing Instructions
    let structure_lines: String = structure_items
        .iter()
        .map(|s| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n");
    let rules_lines: String = rules_items
        .iter()
        .map(|s| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n");
    let style_lines: String = style_items
        .iter()
        .map(|s| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n");

    // System: writing instructions (stable directive content)
    system_parts.push(format!(
        "\n{}\n\n{}\n{}\n\n{}\n{}\n\n{}\n{}\n- {}\n- {}\n\n{}\n{}",
        writing_instr,
        structure_h, structure_lines,
        rules_h, rules_lines,
        style_h, style_lines,
        voice_instruction, char_count_hint,
        output_h, output_schema,
    ));

    (system_parts.join("\n\n"), user_parts.join("\n\n"))
}

/// 解析 LLM 返回的日记 JSON，包含容错处理
pub fn parse_diary_json(response: &str) -> DiaryContent {
    // 第一步：尝试直接解析
    if let Ok(result) = serde_json::from_str::<Value>(response) {
        if let Some(content) = extract_content_and_keywords(&result) {
            return content;
        }
    }

    // 第二步：尝试提取 JSON 块（从第一个 { 到最后一个 }）
    let start_idx = response.find('{');
    let end_idx = response.rfind('}');
    if let (Some(s), Some(e)) = (start_idx, end_idx) {
        if s < e {
            let json_str = &response[s..=e];
            if let Ok(result) = serde_json::from_str::<Value>(json_str) {
                if let Some(content) = extract_content_and_keywords(&result) {
                    return content;
                }
            }
        }
    }

    // 最后回退：返回纯文本作为 content
    tracing::warn!("[DiaryGenerator] LLM 返回格式不符合 JSON 规范，使用回退策略");
    DiaryContent {
        content: response.to_string(),
        keywords: Vec::new(),
        structured_keywords: None,
        mood_tag: None,
        story_update: None,
    }
}

fn extract_content_and_keywords(result: &Value) -> Option<DiaryContent> {
    let content = result.get("content")?.as_str()?.to_string();

    let structured_keywords = result.get("keywords").and_then(|k| {
        if k.is_object() {
            serde_json::from_value::<StructuredKeywords>(k.clone()).ok()
        } else {
            None
        }
    });

    let keywords: Vec<String> = if let Some(ref sk) = structured_keywords {
        let mut flat = Vec::new();
        flat.extend(sk.events.iter().cloned());
        flat.extend(sk.themes.iter().cloned());
        flat.truncate(5);
        flat
    } else {
        result
            .get("keywords")
            .and_then(|k| k.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .take(5)
                    .collect()
            })
            .unwrap_or_default()
    };

    let mood_tag = result
        .get("mood_tag")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let story_update = result.get("ongoing_story_update").and_then(|v| {
        serde_json::from_value::<StoryUpdate>(v.clone()).ok()
    });

    Some(DiaryContent {
        content,
        keywords,
        structured_keywords,
        mood_tag,
        story_update,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diary_json_valid() {
        let response = r#"{"content": "今天很开心", "keywords": ["开心", "陪伴"]}"#;
        let result = parse_diary_json(response);
        assert_eq!(result.content, "今天很开心");
        assert_eq!(result.keywords.len(), 2);
    }

    #[test]
    fn test_parse_diary_json_structured() {
        let response = r#"{"content": "今天很安静", "keywords": {"events": ["独处"], "emotions": ["平静"], "people": [], "themes": ["安静"]}, "mood_tag": "neutral", "ongoing_story_update": {"title": "", "status": "", "summary": ""}}"#;
        let result = parse_diary_json(response);
        assert_eq!(result.content, "今天很安静");
        assert!(result.structured_keywords.is_some());
        let sk = result.structured_keywords.unwrap();
        assert_eq!(sk.events, vec!["独处"]);
        assert_eq!(result.mood_tag, Some("neutral".to_string()));
    }

    #[test]
    fn test_parse_diary_json_with_prefix() {
        let response = "Here is the diary:\n{\"content\": \"测试\", \"keywords\": [\"a\"]}";
        let result = parse_diary_json(response);
        assert_eq!(result.content, "测试");
        assert_eq!(result.keywords, vec!["a".to_string()]);
    }

    #[test]
    fn test_parse_diary_json_fallback() {
        let response = "这是纯文本日记内容";
        let result = parse_diary_json(response);
        assert_eq!(result.content, "这是纯文本日记内容");
        assert!(result.keywords.is_empty());
    }

    #[test]
    fn test_parse_diary_json_keywords_truncated() {
        let response = r#"{"content": "c", "keywords": ["a", "b", "c", "d", "e", "f", "g"]}"#;
        let result = parse_diary_json(response);
        assert_eq!(result.keywords.len(), 5);
    }

    #[test]
    fn test_calculate_trigger_score_empty() {
        let score = calculate_trigger_score(&[], None);
        assert_eq!(score, 0);
    }

    #[test]
    fn test_calculate_trigger_score_with_interactions() {
        let interactions = vec![
            InteractionRecord {
                role: "user".to_string(),
                content: "你好".to_string(),
                timestamp: 0.0,
                channel: "direct".to_string(),
                speaker: "user".to_string(),
                listener: "vivian".to_string(),
            },
            InteractionRecord {
                role: "assistant".to_string(),
                content: "你好呀".to_string(),
                timestamp: 0.0,
                channel: "direct".to_string(),
                speaker: "vivian".to_string(),
                listener: "user".to_string(),
            },
        ];
        let score = calculate_trigger_score(&interactions, None);
        assert!(score >= 20);
    }

    #[test]
    fn test_calculate_trigger_score_with_mood() {
        let interactions = vec![InteractionRecord {
            role: "user".to_string(),
            content: "今天天气真好啊我们一起出去玩吧".to_string(),
            timestamp: 0.0,
            channel: "direct".to_string(),
            speaker: "user".to_string(),
            listener: "vivian".to_string(),
        }];
        let mood = json!({"pet_valence": 0.8, "pet_energy": 90.0, "pet_stress": 30.0});
        let score = calculate_trigger_score(&interactions, Some(&mood));
        assert!(score >= 25);
    }

    #[test]
    fn test_get_mood_tag() {
        let happy = json!({"pet_valence": 0.6, "pet_energy": 60.0, "pet_stress": 0.0});
        assert_eq!(get_mood_tag(&happy), "happy");

        let sad = json!({"pet_valence": -0.3, "pet_energy": 50.0, "pet_stress": 0.0});
        assert_eq!(get_mood_tag(&sad), "sad");

        let angry = json!({"pet_valence": -0.5, "pet_energy": 50.0, "pet_stress": 80.0});
        assert_eq!(get_mood_tag(&angry), "angry");

        let tired = json!({"pet_valence": 0.0, "pet_energy": 10.0, "pet_stress": 0.0});
        assert_eq!(get_mood_tag(&tired), "tired");
    }

    #[test]
    fn test_validate_mood_tag() {
        assert_eq!(validate_mood_tag("happy"), "happy");
        assert_eq!(validate_mood_tag("invalid"), "neutral");
        assert_eq!(validate_mood_tag("tired"), "tired");
    }

    #[test]
    fn test_build_prompt_contains_required_sections() {
        let ctx = DailyContext {
            char_id: "vivian".to_string(),
            char_name: "Vivian".to_string(),
            char_cn_name: "薇薇安".to_string(),
            interactions: vec![InteractionRecord {
                role: "user".to_string(),
                content: "你好".to_string(),
                timestamp: 0.0,
                channel: "direct".to_string(),
                speaker: "user".to_string(),
                listener: "vivian".to_string(),
            }],
            mood_summary: json!({"pet_intimacy": 50.0, "pet_trust": 50.0}),
            last_diary_summary: "Yesterday was good.".to_string(),
            cross_diary_context: String::new(),
            daily_events: Vec::new(),
            vivian_activities: Vec::new(),
            timeline: vec!["09:00 开始聊天".to_string()],
            mood_samples: Vec::new(),
            lingering_thoughts: Vec::new(),
            ongoing_stories: Vec::new(),
            relationship_delta: None,
            personality_baseline: "A warm companion.".to_string(),
        };
        let (system_prompt, user_prompt) = build_prompt(&ctx, "en");
        let prompt = format!("{}\n{}", system_prompt, user_prompt);
        assert!(prompt.contains("You are Vivian"));
        assert!(prompt.contains("Who You Are"));
        assert!(prompt.contains("Writing Instructions"));
        assert!(prompt.contains("Never fabricate"));
        assert!(prompt.contains("JSON"));
        assert!(prompt.contains("mood_tag"));
    }
}
