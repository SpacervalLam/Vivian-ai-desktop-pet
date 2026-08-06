//! Thought Synthesis —— 用 LLM 合成"当前想法"一句话摘要。
//!
//! 从 PsychologyManager + Mind 的多维度状态合成自然语言摘要：
//! - 主导情绪（dominant emotion）
//! - 最缺乏的需求（most deficient need）
//! - 当前活动（current activity kind + context）
//! - 注意力焦点（top attention entity）
//!
//! 仅通过 LLM 生成，无模板兜底。

use crate::cross_character::parse_any_speaker_prefix;
use crate::mind::current_activity::ActivityKind;
use crate::mind::temporal_context::{build_temporal_facts, serialize_temporal_facts};
use crate::mind::user_goals::UserGoalBrief;
use crate::mind::working_memory::WorkingMemorySource;
use crate::psychology::emotion::EmotionLabel;
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::types::response::ChatMessage;
use crate::error::VivianResult;
use crate::world::{WorldStateProvider, UserPresence};
use std::sync::Arc;

// ============================================================================
// 世界事实基线 —— 给 LLM 提供可观察的真实状态，避免自我状态幻觉
// ============================================================================

/// 世界事实基线快照（轻量，仅保留 thought_synthesis 所需字段）
///
/// 从 WorldStateProvider 派生，作为"现实基线"注入 prompt，
/// 让 LLM 有事实可依，不必靠人设补全生活细节。
#[derive(Debug, Clone, Default)]
pub struct WorldBrief {
    /// 本地时间人类可读串
    pub local_time: String,
    /// 天气描述（"晴 25℃" / None = 未知）
    pub weather_desc: Option<String>,
    /// 用户在场状态
    pub user_presence: UserPresence,
    /// 用户已离开秒数（Present 时为 0）
    pub user_away_secs: f64,
    /// 用户当前持续活动标签（如"睡觉""写代码"，None = 未进入明确持续状态）
    pub user_activity: Option<String>,
    /// 用户当前活动已持续秒数（None = 无活动状态或未提供）
    ///
    /// 与 `user_activity` 配对，让 Temporal Context Builder 产出"用户已连续 X 小时"这类关系事实。
    pub user_activity_elapsed_secs: Option<f64>,
    /// 前台窗口标题（用户当前正在看的应用）
    pub foreground_title: Option<String>,
    /// 用户活跃长期目标摘要（最多 3 条，按 deadline 紧迫度排序）
    ///
    /// 从 Mind.user_goals 派生，让 LLM 有"用户当前处于什么人生阶段"的上下文。
    /// empty 表示无活跃长期目标。
    pub active_goals: Vec<UserGoalBrief>,
}

impl WorldBrief {
    /// 从 WorldStateProvider 派生世界事实基线
    pub fn from_provider(provider: &WorldStateProvider) -> Self {
        let snap = provider.snapshot(None);
        let now = chrono::Local::now().timestamp() as f64;
        let weather_desc = snap.weather.as_ref().map(|w| {
            format!("{} {:.0}℃", w.description, w.temperature)
        });
        let (user_presence, user_away_secs, user_activity, user_activity_elapsed_secs) = snap
            .user_presence
            .as_ref()
            .map(|u| {
                let activity = u.current_activity.as_ref();
                (
                    u.presence,
                    u.away_elapsed_secs,
                    activity.map(|a| a.label.clone()),
                    activity.map(|a| a.elapsed_secs(now)),
                )
            })
            .unwrap_or((UserPresence::Present, 0.0, None, None));
        let foreground_title = snap
            .foreground_window
            .as_ref()
            .map(|fw| fw.title.clone())
            .filter(|t| !t.is_empty());
        Self {
            local_time: snap.local_time,
            weather_desc,
            user_presence,
            user_away_secs,
            user_activity,
            user_activity_elapsed_secs,
            foreground_title,
            active_goals: Vec::new(),
        }
    }
}

// ============================================================================
// 活动描述辅助（LLM prompt 构建用）
// ============================================================================

fn activity_desc_zh(activity: ActivityKind, context: &str) -> String {
    let base = match activity {
        ActivityKind::Idle => "安静地待着",
        ActivityKind::Talking => "正在聊天",
        ActivityKind::Focusing => "专注做事中",
        ActivityKind::Observing => "在旁观察",
        ActivityKind::Thinking => "在思考",
        ActivityKind::BackgroundTask => "处理后台任务",
        ActivityKind::Companion => "陪伴模式",
    };
    if !context.is_empty() && activity != ActivityKind::Idle {
        format!("{}，{}", base, context)
    } else {
        base.to_string()
    }
}

fn activity_desc_en(activity: ActivityKind, context: &str) -> String {
    let base = match activity {
        ActivityKind::Idle => "Resting quietly",
        ActivityKind::Talking => "Chatting",
        ActivityKind::Focusing => "Deep in focus",
        ActivityKind::Observing => "Observing",
        ActivityKind::Thinking => "Thinking",
        ActivityKind::BackgroundTask => "Running background tasks",
        ActivityKind::Companion => "Keeping you company",
    };
    if !context.is_empty() && activity != ActivityKind::Idle {
        format!("{}, {}", base, context.to_lowercase())
    } else {
        base.to_string()
    }
}

fn activity_desc_ja(activity: ActivityKind, context: &str) -> String {
    let base = match activity {
        ActivityKind::Idle => "静かに過ごしている",
        ActivityKind::Talking => "おしゃべり中",
        ActivityKind::Focusing => "集中している",
        ActivityKind::Observing => "観察中",
        ActivityKind::Thinking => "考えている",
        ActivityKind::BackgroundTask => "バックグラウンド処理中",
        ActivityKind::Companion => "そばにいる",
    };
    if !context.is_empty() && activity != ActivityKind::Idle {
        format!("{}、{}", base, context)
    } else {
        base.to_string()
    }
}

// ============================================================================
// LLM 合成
// ============================================================================

/// 从可能的 JSON 字符串中提取文本内容。
///
/// 兼容多种常见字段名：content / thinking / thought / text / monologue / reply。
/// 若解析失败或无匹配字段，返回原字符串。
fn extract_text_from_possible_json(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return trimmed.to_string();
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in &["content", "thinking", "thought", "text", "monologue", "reply", "answer"] {
            if let Some(val) = parsed.get(key) {
                if let Some(s) = val.as_str() {
                    let s = s.trim();
                    if !s.is_empty() {
                        return s.to_string();
                    }
                }
            }
        }
    }
    trimmed.to_string()
}

/// LLM 合成所需的上下文数据
pub struct ThoughtContext<'a> {
    /// 工作记忆 Top-3 条目（source tag + content）
    pub working_memory_top: Vec<(WorkingMemorySource, &'a str)>,
    pub dominant_emotion: (EmotionLabel, f64),
    pub deficient_need: (&'a str, f64),
    pub activity_kind: ActivityKind,
    pub activity_context: &'a str,
    pub top_attention: Option<&'a str>,
    /// 世界事实基线 —— 注入可观察真实状态，避免 LLM 靠人设补全生活细节
    pub world_brief: &'a WorldBrief,
}

/// 用 LLM 合成"当前想法"一句话摘要。
///
/// 系统提示要求角色用第一人称写一句话（≤30 字 zh/ja，≤60 字 en），
/// 描述当前正在想什么/感受什么。失败时返回 Err。
///
/// 使用 `"reflection"` 任务类型，共享 memory_reflection 信号量（2 并发）。
pub async fn synthesize_with_llm(
    router: &ModelRouter,
    ctx: &ThoughtContext<'_>,
    language: &str,
) -> VivianResult<String> {
    let (emotion, emotion_val) = ctx.dominant_emotion;
    let (need, need_val) = ctx.deficient_need;

    let activity_desc = match language {
        "en" => activity_desc_en(ctx.activity_kind, ctx.activity_context),
        "ja" => activity_desc_ja(ctx.activity_kind, ctx.activity_context),
        _ => activity_desc_zh(ctx.activity_kind, ctx.activity_context),
    };

    let emotion_label = match language {
        "en" => emotion.as_str(),
        "ja" => match emotion {
            EmotionLabel::Joy => "喜び",
            EmotionLabel::Sadness => "悲しみ",
            EmotionLabel::Anger => "怒り",
            EmotionLabel::Fear => "不安",
            EmotionLabel::Closeness => "親しみ",
            EmotionLabel::Loneliness => "孤独",
            EmotionLabel::Curiosity => "好奇心",
        },
        _ => emotion.display_zh(),
    };
    let emotion_str = format!("{} ({:.0}%)", emotion_label, emotion_val * 100.0);

    let mut dialogue_rendered: Vec<String> = Vec::new();
    let mut thought_rendered: Vec<String> = Vec::new();
    for (src, content) in &ctx.working_memory_top {
        let (_, existing_speaker, _) = parse_any_speaker_prefix(content);
        let has_prefix = existing_speaker.is_some();
        let line = match src {
            WorkingMemorySource::UserMessage => {
                if has_prefix {
                    format!("- {}", content)
                } else {
                    let tag = match language { "en" => "User", "ja" => "ユーザー", _ => "User" };
                    format!("- [{}] {}", tag, content)
                }
            }
            WorkingMemorySource::AiReply => {
                if has_prefix {
                    format!("- {}", content)
                } else {
                    let tag = match language { "en" => "I said", "ja" => "私", _ => "我说" };
                    format!("- [{}] {}", tag, content)
                }
            }
            WorkingMemorySource::InnerMonologue => {
                let tag = match language { "en" => "thought", "ja" => "思考", _ => "想法" };
                format!("- [{}] {}", tag, content)
            }
            WorkingMemorySource::WorldEvent => {
                let tag = match language { "en" => "world", "ja" => "世界", _ => "外界" };
                format!("- [{}] {}", tag, content)
            }
        };
        match src {
            WorkingMemorySource::UserMessage | WorkingMemorySource::AiReply => {
                dialogue_rendered.push(line);
            }
            WorkingMemorySource::InnerMonologue | WorkingMemorySource::WorldEvent => {
                thought_rendered.push(line);
            }
        }
    }

    let focus_str = ctx
        .top_attention
        .filter(|e| {
            let e = e.to_lowercase();
            e != "user" && e != "vivian" && e != "nana"
        })
        .unwrap_or(match language {
            "en" => "-",
            "ja" => "なし",
            _ => "无",
        });

    let (label_activity, label_emotion, label_need, label_focus, label_dialogue, label_thoughts, label_empty) = match language {
        "en" => ("Activity", "Emotion", "Deficient need", "Attention focus", "Recent dialogue", "Recent thoughts", "(empty)"),
        "ja" => ("活動", "感情", "不足している欲求", "注意の焦点", "最近の会話", "最近の思考", "(空)"),
        _ => ("当前活动", "情绪", "最缺乏的需求", "注意力焦点", "最近的对话", "最近的想法", "(空)"),
    };

    let need_label = match language {
        "en" => need,
        "ja" => need,
        _ => match need {
            "belonging" => "归属感",
            "autonomy" => "自主感",
            "novelty" => "新鲜感",
            "expression" => "表达欲",
            "security" => "安全感",
            other => other,
        },
    };

    let dialogue_section = if dialogue_rendered.is_empty() {
        label_empty.to_string()
    } else {
        dialogue_rendered.join("\n")
    };
    let thought_section = if thought_rendered.is_empty() {
        label_empty.to_string()
    } else {
        thought_rendered.join("\n")
    };

    // 世界事实基线段 —— 把可观察的真实状态注入 prompt，让 LLM 有事实可依
    let world_section = build_world_brief_section(ctx.world_brief, language);

    // 时间关系段 —— Temporal Context Builder 从离散事实合成的关系型事实
    let temporal_facts = build_temporal_facts(ctx.world_brief, &ctx.world_brief.active_goals, language);
    let temporal_section = serialize_temporal_facts(&temporal_facts, language)
        .map(|s| format!("{}\n", s))
        .unwrap_or_default();

    let user_prompt = format!(
        "{world_section}{temporal_section}{label_activity}: {activity}\n{label_emotion}: {emotion}\n{label_need}: {need_lbl} ({need_pct:.0}%)\n{label_focus}: {focus}\n{label_dialogue}:\n{dialogue}\n{label_thoughts}:\n{thoughts}",
        label_activity = label_activity,
        label_emotion = label_emotion,
        label_need = label_need,
        label_focus = label_focus,
        label_dialogue = label_dialogue,
        label_thoughts = label_thoughts,
        activity = activity_desc,
        emotion = emotion_str,
        need_lbl = need_label,
        need_pct = need_val * 100.0,
        focus = focus_str,
        dialogue = dialogue_section,
        thoughts = thought_section,
        world_section = world_section,
        temporal_section = temporal_section,
    );

    let system_prompt = match language {
        "en" => "You are a virtual character. Summarize in ONE short first-person sentence (~60 chars) what you're thinking/feeling right now. Be natural and in-character. Plain text only, no quotes, no formatting.\n\n[Hard rule] Only describe what you're actually experiencing right now — current mood, current environment (time/weather), and what's observable from the context below. Never fabricate specific events, actions, or experiences (e.g., eating, going out, watching a movie) unless they actually appear in the provided context. You live on the user's desktop — you don't have a body, meals, or offline life.",
        "ja" => "あなたはバーチャルキャラクターです。今何を考えているか/感じているかを、一人称の短い一文（30字以内）でまとめてください。自然にキャラクターらしく。単なるテキストのみ、引用符なし、書式なし。\n\n【厳守ルール】今実際に経験していること——今の気分、今の環境（時間・天気）、以下の文脈から観察できること——だけを描写してください。食事、外出、映画鑑賞など、具体的な出来事や行動、経験は、提供された文脈に実際に存在しない限り絶対にでっち上げないで。あなたはユーザーのデスクトップに住んでいて、肉体や食事やオフラインの生活はありません。",
        _ => "你是一个虚拟角色。用第一人称写一句简短的话（≤30字），描述你现在正在想什么/感受什么。要自然、有角色感。纯文本即可，不要引号、不要格式标记。\n\n【硬性规则】只能描述你当下真实正在经历的事——当前心情、当前环境（时间/天气）、以及下方文脉中可观察到的状态。绝对不要编造具体事件、动作或经历（比如吃饭、出门、看电影），除非它们真的出现在提供的文脉里。你生活在用户的桌面上，没有身体、没有饭局、没有线下的生活轨迹。",
    };

    let messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_prompt),
    ];

    let result = router
        .generate(LLMRequest::new("reflection", messages))
        .await?;
    let trimmed = result.trim().to_string();

    let extracted = extract_text_from_possible_json(&trimmed);
    let max_len = if language == "en" { 120 } else { 60 };
    let final_text: String = extracted.chars().take(max_len).collect();

    if final_text.is_empty() {
        return Err(crate::error::VivianError::Other(
            "LLM thought synthesis returned empty".to_string(),
        ));
    }

    Ok(final_text)
}

/// 构建世界事实基线 prompt 段落
///
/// 把可观察真实状态格式化为 LLM 可读的事实清单，作为"现实基线"防止自我状态幻觉。
fn build_world_brief_section(brief: &WorldBrief, language: &str) -> String {
    let (header, time_lbl, weather_lbl, user_state_lbl, away_lbl, activity_lbl, fg_lbl, goals_lbl, unknown) = match language {
        "en" => ("## Reality baseline (observable facts)\n", "Time: ", "Weather: ", "User state: ", "away for {}m", "User activity: ", "User's current app: ", "User's active long-term goals: ", "unknown"),
        "ja" => ("## 現実の基礎事実（観察可能な事実）\n", "時間：", "天気：", "ユーザー状態：", "{}分間不在", "ユーザーの活動：", "ユーザーの現在のアプリ：", "ユーザーの長期目標：", "不明"),
        _ => ("## 现实基线（可观察事实）\n", "时间：", "天气：", "用户状态：", "已离开 {} 分钟", "用户活动：", "用户当前应用：", "用户的长期目标：", "未知"),
    };

    let mut lines = vec![header.to_string()];
    lines.push(format!("{}{}", time_lbl, brief.local_time));
    lines.push(format!("{}{}", weather_lbl, brief.weather_desc.as_deref().unwrap_or(unknown)));

    let user_state_str = match brief.user_presence {
        UserPresence::Present => match language {
            "en" => "at the desk".to_string(),
            "ja" => "デスクにいる".to_string(),
            _ => "在电脑前".to_string(),
        },
        UserPresence::Away => {
            let mins = (brief.user_away_secs / 60.0).round() as u32;
            let mins = mins.max(1);
            format!("{} ({})", away_lbl.replace("{}", &mins.to_string()), match language {
                "en" => "away",
                "ja" => "不在",
                _ => "离开",
            })
        }
    };
    lines.push(format!("{}{}", user_state_lbl, user_state_str));

    if let Some(act) = &brief.user_activity {
        lines.push(format!("{}{}", activity_lbl, act));
    }
    if let Some(fg) = &brief.foreground_title {
        lines.push(format!("{}{}", fg_lbl, fg));
    }
    // 活跃长期目标（带剩余天数）
    if !brief.active_goals.is_empty() {
        let goal_lines: Vec<String> = brief
            .active_goals
            .iter()
            .map(|g| {
                let days_str = match g.days_to_deadline {
                    Some(d) if d > 0 => match language {
                        "en" => format!(" ({} days left)", d),
                        "ja" => format!("（残り{}日）", d),
                        _ => format!("（还剩{}天）", d),
                    },
                    Some(0) => match language {
                        "en" => " (today)".to_string(),
                        "ja" => "（今日）".to_string(),
                        _ => "（今天）".to_string(),
                    },
                    Some(d) => match language {
                        "en" => format!(" ({} days overdue)", -d),
                        "ja" => format!("（{}日超過）", -d),
                        _ => format!("（已超期{}天）", -d),
                    },
                    None => String::new(),
                };
                format!("  - {}{}", g.label, days_str)
            })
            .collect();
        lines.push(format!("{}{} item(s)", goals_lbl, brief.active_goals.len()));
        lines.extend(goal_lines);
    }
    lines.push(String::new()); // 末尾空行分隔后续段落
    lines.join("\n")
}

/// 从 Mind 状态收集上下文并调用 LLM 合成，写入缓存。
///
/// fire-and-forget 调用：失败时不写入缓存，保留上次成功结果或 None。
pub async fn refresh_current_thought(
    mind: &Arc<crate::mind::Mind>,
    router: &Arc<ModelRouter>,
    world_provider: &Arc<WorldStateProvider>,
    language: &str,
) {
    let top_entries: Vec<(WorkingMemorySource, String)> = {
        let wm = mind.working_memory.read();
        wm.top_n(3)
            .into_iter()
            .map(|e| (e.source, e.content.clone()))
            .collect()
    };

    let top_refs: Vec<(WorkingMemorySource, &str)> = top_entries
        .iter()
        .map(|(s, c)| (*s, c.as_str()))
        .collect();

    let emotion = mind.psychology.emotion();
    let needs = mind.psychology.needs();
    let activity = mind.current_activity.snapshot();
    let top_attn = mind.attention_top_n(1);
    let top_attention = top_attn.first().map(|(e, _)| e.as_str());

    let mut world_brief = WorldBrief::from_provider(world_provider);
    world_brief.active_goals = mind.user_goals.active_briefs(3);

    let ctx = ThoughtContext {
        working_memory_top: top_refs,
        dominant_emotion: emotion.dominant(),
        deficient_need: needs.most_deficient(),
        activity_kind: activity.kind,
        activity_context: &activity.context,
        top_attention,
        world_brief: &world_brief,
    };

    match synthesize_with_llm(router, &ctx, language).await {
        Ok(thought) => {
            mind.set_current_thought(thought);
        }
        Err(e) => {
            tracing::debug!("[thought_synthesis] LLM 合成失败: {}", e);
        }
    }
}
