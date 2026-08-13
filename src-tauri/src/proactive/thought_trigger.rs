//! 自主思绪事件检测 —— 检测事件并产生思绪种子
//!
//! 不再直接决定"现在就生成独白"，而是产出 ThoughtSeed（种子），
//! 交给 ThoughtLifecycle 管理强度积累和表达决策。
//!
//! 公式：thought_intensity = event_importance × emotional_relevance × relationship_weight

use std::collections::HashMap;

use rand::Rng;

use crate::proactive::activity_journal::ActivityEntry;
use crate::proactive::OnlineCompanion;
use crate::psychology::emotion::EmotionLabel;
use crate::psychology::mood::MoodSnapshot;
use crate::world::events::{WorldEvent, WorldEventKind};
use crate::world::WorldSnapshot;

/// 一颗思绪种子：由事件产生，交给 ThoughtLifecycle 培育
#[derive(Debug, Clone)]
pub struct ThoughtSeed {
    /// 思绪 key（同类思绪合并，如 "user_miss"、"weather_rain"）
    pub thought_key: String,
    /// 简短描述（一句话）
    pub description: String,
    /// 注入 prompt 的上下文
    pub context_hint: String,
    /// 基础强度 [0, 1]（经过 relationship_weight 调整后）
    pub intensity: f32,
    /// 情绪色彩 -1(负) ~ +1(正)
    pub valence: f32,
    /// 情绪唤醒度 0(平静) ~ 1(激动)
    pub arousal: f32,
    /// 表达欲基础值 [0, 1]
    pub base_desire: f32,
    /// 事件类型
    pub trigger_kind: &'static str,
    /// 是否是高优先级信号（休息/醒来等应立即播种高浓度种子）
    pub high_priority: bool,
}

/// 事件检测评估器（检测世界/用户/情绪事件，产出 ThoughtSeed）
pub struct ThoughtTriggerEvaluator {
    last_user_present: Option<bool>,
    last_primary_emotion: Option<EmotionLabel>,
    last_companion_spoke_secs: Option<f64>,
    last_companion_id: Option<String>,
    last_event_ts_by_type: HashMap<&'static str, f64>,
    last_background_ts: f64,
    last_deep_reflection_ts: f64,
}

impl ThoughtTriggerEvaluator {
    pub fn new() -> Self {
        Self {
            last_user_present: None,
            last_primary_emotion: None,
            last_companion_spoke_secs: None,
            last_companion_id: None,
            last_event_ts_by_type: HashMap::new(),
            last_background_ts: 0.0,
            last_deep_reflection_ts: 0.0,
        }
    }

    /// 检测事件，返回本 tick 应播种的思绪种子列表
    #[allow(clippy::too_many_arguments)]
    pub fn detect_seeds(
        &mut self,
        user_present: bool,
        away_seconds: f64,
        interaction_count_today: u32,
        snap: &WorldSnapshot,
        world_events: &[WorldEvent],
        mood: &MoodSnapshot,
        intimacy: f64,
        activity_snapshot: &[ActivityEntry],
        companion: &Option<OnlineCompanion>,
        going_to_rest: bool,
        rest_reason: &str,
        waking_up: bool,
        now: f64,
        hour: u32,
        habit_deviation: Option<&crate::proactive::habits::HabitDeviation>,
        char_id: &str,
        needs_novelty: f32,
    ) -> Vec<ThoughtSeed> {
        let mut seeds = Vec::new();
        let rel_weight = relationship_weight(intimacy);

        // ==== 高优先级状态切换 ====

        if going_to_rest {
            seeds.push(ThoughtSeed {
                thought_key: "going_to_rest".into(),
                description: format!("准备去休息：{}", rest_reason),
                context_hint: format!("你现在准备去休息了，原因：{}，心里有点困也有点放松", rest_reason),
                intensity: 0.85,
                valence: -0.2,
                arousal: 0.2,
                base_desire: 0.6,
                trigger_kind: "going_to_rest",
                high_priority: true,
            });
        }

        if waking_up {
            seeds.push(ThoughtSeed {
                thought_key: "waking_up".into(),
                description: "刚醒来".into(),
                context_hint: "你刚从休息中醒来，迷迷糊糊的，慢慢回过神来".into(),
                intensity: 0.75,
                valence: 0.2,
                arousal: 0.3,
                base_desire: 0.4,
                trigger_kind: "waking_up",
                high_priority: true,
            });
        }

        // ==== 用户在场变化 ====

        if let Some(last_present) = self.last_user_present {
            if last_present && !user_present {
                let cooldown_ok = self.check_cooldown("user_left", now, 180.0);
                if cooldown_ok {
                    let mins = away_seconds / 60.0;
                    seeds.push(ThoughtSeed {
                        thought_key: "user_left".into(),
                        description: format!("用户刚离开（{:.0}分钟）", mins),
                        context_hint: format!("用户刚刚离开了，已经过了{:.0}分钟", mins),
                        intensity: (0.35 * rel_weight),
                        valence: -0.2 * rel_weight,
                        arousal: 0.2,
                        base_desire: 0.15,
                        trigger_kind: "user_left",
                        high_priority: false,
                    });
                    self.last_event_ts_by_type.insert("user_left", now);
                }
            }
            if !last_present && user_present {
                let cooldown_ok = self.check_cooldown("user_return", now, 120.0);
                if cooldown_ok {
                    let hours = away_seconds / 3600.0;
                    let miss_factor = if hours > 2.0 { 0.8 } else if hours > 0.5 { 0.6 } else { 0.35 };
                    seeds.push(ThoughtSeed {
                        thought_key: "user_return".into(),
                        description: "用户回来了".into(),
                        context_hint: if hours > 1.0 {
                            format!("用户离开了{:.1}小时后回来了，心里有点高兴", hours)
                        } else {
                            "用户回来了".to_string()
                        },
                        intensity: (miss_factor * rel_weight).min(0.9),
                        valence: 0.6 * rel_weight,
                        arousal: 0.5,
                        base_desire: if hours > 1.0 { 0.8 } else { 0.3 },
                        trigger_kind: "user_return",
                        high_priority: hours > 1.0,
                    });
                    self.last_event_ts_by_type.insert("user_return", now);
                }
            }
        }

        // ==== 长时间无互动 → 用户思念积累 ====

        if let Some(secs) = snap.seconds_since_last_interaction {
            let hours = secs / 3600.0;
            if hours > 0.5 {
                let cooldown_ok = self.check_cooldown("long_silence", now, 1800.0);
                if cooldown_ok {
                    let factor = ((hours - 0.5) / 5.0).min(1.0) as f32;
                    let miss_intensity = (0.2 + factor * 0.55) * rel_weight;
                    seeds.push(ThoughtSeed {
                        thought_key: "user_miss".into(),
                        description: format!("安静了{:.1}小时", hours),
                        context_hint: if hours > 3.0 {
                            format!("用户已经{:.1}小时没说话了，心里有点空落落的", hours)
                        } else {
                            format!("已经安静了{:.1}小时，好像有点冷清", hours)
                        },
                        intensity: miss_intensity,
                        valence: -0.1 - factor * 0.4,
                        arousal: 0.15 + factor * 0.2,
                        base_desire: if hours > 4.0 { 0.7 } else if hours > 2.0 { 0.4 } else { 0.1 },
                        trigger_kind: "long_silence",
                        high_priority: hours > 5.0,
                    });
                    self.last_event_ts_by_type.insert("long_silence", now);
                }
            }
        }

        // ==== 天气/环境事件 ====

        for ev in world_events {
            match ev.kind {
                WorldEventKind::RainStarted => {
                    if self.check_cooldown("weather_rain", now, 7200.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: "weather_rain".into(),
                            description: "开始下雨了".into(),
                            context_hint: ev.description.clone(),
                            intensity: 0.35,
                            valence: -0.1,
                            arousal: 0.2,
                            base_desire: 0.3,
                            trigger_kind: "weather_shift",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("weather_rain", now);
                    }
                }
                WorldEventKind::WeatherChanged => {
                    if self.check_cooldown("weather_change", now, 10800.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: format!("weather_{}", ev.description.chars().take(8).collect::<String>()),
                            description: "天气变了".into(),
                            context_hint: ev.description.clone(),
                            intensity: 0.2,
                            valence: 0.0,
                            arousal: 0.1,
                            base_desire: 0.1,
                            trigger_kind: "weather_shift",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("weather_change", now);
                    }
                }
                WorldEventKind::Sunset => {
                    if self.check_cooldown("sunset", now, 86400.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: "sunset".into(),
                            description: "日落了".into(),
                            context_hint: ev.description.clone(),
                            intensity: 0.35,
                            valence: 0.2,
                            arousal: 0.1,
                            base_desire: 0.25,
                            trigger_kind: "environmental_event",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("sunset", now);
                    }
                }
                WorldEventKind::Sunrise => {
                    if self.check_cooldown("sunrise", now, 86400.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: "sunrise".into(),
                            description: "天亮了".into(),
                            context_hint: ev.description.clone(),
                            intensity: 0.3,
                            valence: 0.3,
                            arousal: 0.2,
                            base_desire: 0.2,
                            trigger_kind: "environmental_event",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("sunrise", now);
                    }
                }
                WorldEventKind::FestivalArrived => {
                    if self.check_cooldown("festival", now, 86400.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: format!("festival_{}", ev.description.chars().take(6).collect::<String>()),
                            description: ev.description.clone(),
                            context_hint: format!("今天是{}，心里有点不一样的感觉", ev.description),
                            intensity: 0.55,
                            valence: 0.5,
                            arousal: 0.4,
                            base_desire: 0.7,
                            trigger_kind: "festival",
                            high_priority: true,
                        });
                        self.last_event_ts_by_type.insert("festival", now);
                    }
                }
                WorldEventKind::SeasonChanged | WorldEventKind::SolarTermChanged => {
                    if self.check_cooldown("season", now, 86400.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: "season_change".into(),
                            description: ev.description.clone(),
                            context_hint: ev.description.clone(),
                            intensity: 0.4,
                            valence: 0.1,
                            arousal: 0.2,
                            base_desire: 0.3,
                            trigger_kind: "environmental_event",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("season", now);
                    }
                }
                _ => {}
            }
        }

        // ==== 用户活动模式 ====

        if let Some((title, category, streak)) = detect_activity_pattern(activity_snapshot) {
            let key = format!("activity_{}", title);
            if self.check_cooldown("activity", now, 3600.0) {
                let is_interesting = !matches!(category.as_str(), "编程" | "系统" | "其他");
                if is_interesting {
                    let int = if streak >= 3 { 0.4 } else { 0.25 };
                    seeds.push(ThoughtSeed {
                        thought_key: key,
                        description: format!("用户在{}{}", category, title),
                        context_hint: if streak >= 3 {
                            format!("用户最近经常在{}：{}，好像很投入的样子", category, title)
                        } else {
                            format!("用户现在在{}：{}", category, title)
                        },
                        intensity: int * rel_weight,
                        valence: 0.2,
                        arousal: 0.25,
                        base_desire: if streak >= 3 { 0.35 } else { 0.1 },
                        trigger_kind: "activity_pattern",
                        high_priority: false,
                    });
                    self.last_event_ts_by_type.insert("activity", now);
                }
            }
        }

        // ==== 情绪变化 ====

        if mood.primary_intensity > 0.6 {
            let emotion_zh = emotion_display_zh(&mood.primary_emotion);
            let is_shift = self.last_primary_emotion.as_ref() != Some(&mood.primary_emotion);
            if is_shift || self.check_cooldown("emotion", now, 900.0) {
                let (val, ar) = emotion_valence_arousal(&mood.primary_emotion);
                let strength = if is_shift { 0.45 } else { 0.3 };
                seeds.push(ThoughtSeed {
                    thought_key: "emotion_shift".into(),
                    description: format!("情绪变了：{}", emotion_zh),
                    context_hint: if is_shift {
                        format!("心情突然变了，现在{}的感觉很强烈", emotion_zh)
                    } else {
                        format!("现在{}的感觉一直萦绕着", emotion_zh)
                    },
                    intensity: strength * rel_weight,
                    valence: val,
                    arousal: ar.max(0.3),
                    base_desire: 0.15,
                    trigger_kind: "emotion_accumulation",
                    high_priority: false,
                });
                if is_shift {
                    self.last_event_ts_by_type.insert("emotion_shift", now);
                }
            }
        }

        // ==== 跨角色事件 ====

        if let Some(comp) = companion {
            if let Some(spoke_ago) = comp.last_spoke_secs_ago {
                let is_new = match self.last_companion_spoke_secs {
                    None => true,
                    Some(prev) => spoke_ago < prev && spoke_ago < 30.0,
                };
                if is_new || (spoke_ago < 20.0
                    && self.last_companion_spoke_secs.map_or(true, |p| p > spoke_ago + 5.0))
                {
                    let snippet = comp
                        .last_spoke_text
                        .as_deref()
                        .map(truncate_snippet)
                        .unwrap_or_default();
                    let key = format!("companion_spoke_{}", comp.id);
                    if self.check_cooldown("companion_spoke", now, 300.0) {
                        seeds.push(ThoughtSeed {
                            thought_key: key,
                            description: format!("{}刚说话了", comp.name),
                            context_hint: if snippet.is_empty() {
                                format!("{}刚才说了句话", comp.name)
                            } else {
                                format!("{}刚才说：「{}」", comp.name, snippet)
                            },
                            intensity: 0.35,
                            valence: 0.2,
                            arousal: 0.3,
                            base_desire: 0.2,
                            trigger_kind: "cross_character_spoke",
                            high_priority: false,
                        });
                        self.last_event_ts_by_type.insert("companion_spoke", now);
                    }
                }
            }
            self.last_companion_spoke_secs = comp.last_spoke_secs_ago;
            self.last_companion_id = Some(comp.id.clone());
        } else {
            self.last_companion_spoke_secs = None;
            self.last_companion_id = None;
        }

        // ==== 分享诱因检测：值得主动找室友聊的事件 ====
        // 与 cross_character_spoke（室友刚说话的被动响应）不同，
        // 这是"发生了值得分享的事"的主动动机，驱动角色主动找室友聊天
        if companion.is_some() && self.check_cooldown("want_share_roommate", now, 1800.0) {
            let mut share_trigger: Option<(String, String, f32)> = None;

            // 1. 用户行为类别切换：从 activity_snapshot 最近两条检测 category 变化
            if activity_snapshot.len() >= 2 {
                let latest = &activity_snapshot[activity_snapshot.len() - 1];
                let prev = &activity_snapshot[activity_snapshot.len() - 2];
                if let (Some(new_cat), Some(old_cat)) = (&latest.category, &prev.category) {
                    if new_cat != old_cat
                        && !matches!(new_cat.as_str(), "系统" | "其他" | "")
                        && !matches!(old_cat.as_str(), "系统" | "其他" | "")
                    {
                        share_trigger = Some((
                            format!("share_activity_{}", new_cat),
                            format!("用户从{}切到了{}", old_cat, new_cat),
                            0.55,
                        ));
                    }
                }
            }

            // 2. 显著世界事件：RainStarted / FestivalArrived
            if share_trigger.is_none() {
                for ev in world_events {
                    if matches!(
                        ev.kind,
                        WorldEventKind::RainStarted | WorldEventKind::FestivalArrived
                    ) {
                        share_trigger = Some((
                            format!("share_event_{:?}", ev.kind),
                            ev.description.clone(),
                            0.60,
                        ));
                        break;
                    }
                }
            }

            // 3. 情绪累积：loneliness 高 + 有在线室友
            if share_trigger.is_none()
                && mood.primary_emotion == EmotionLabel::Loneliness
                && mood.primary_intensity > 0.6
            {
                share_trigger = Some((
                    "share_lonely".into(),
                    "一个人待着有点孤单，想找室友说说话".into(),
                    0.50,
                ));
            }

            if let Some((key, hint, intensity)) = share_trigger {
                seeds.push(ThoughtSeed {
                    thought_key: format!("want_share_roommate_{}", key),
                    description: format!("想和室友分享：{}", hint),
                    context_hint: format!("你{}，想找室友聊聊这件事", hint),
                    // 不乘 rel_weight：想和室友聊的动机不应被 user↔agent intimacy 抑制
                    // intensity 0.50~0.60：单次诱因即可接近 PROACTIVE_SHARE_THRESHOLD(0.70)，
                    // 一次 nourish 即可达到表达阈值，不再需要 3 次同类事件积累
                    intensity,
                    valence: 0.1,
                    arousal: 0.3,
                    base_desire: 0.5,
                    trigger_kind: "want_to_share_with_roommate",
                    high_priority: false,
                });
                self.last_event_ts_by_type.insert("want_share_roommate", now);
            }
        }

        // ==== 习惯偏离（用户当前活动与历史时段习惯不符）====

        if let Some(dev) = habit_deviation {
            if user_present
                && self.check_cooldown("habit_deviation", now, 1800.0)
            {
                seeds.push(ThoughtSeed {
                    thought_key: "habit_deviation".into(),
                    description: format!(
                        "用户平时这个时段通常{}，今天却在{}",
                        dev.typical_label, dev.current_label
                    ),
                    context_hint: format!(
                        "你注意到用户平时这个时段通常{}，今天却在{}，有点不一样",
                        dev.typical_label, dev.current_label
                    ),
                    intensity: 0.45,
                    valence: 0.0,
                    arousal: 0.3,
                    base_desire: 0.5,
                    trigger_kind: "habit_deviation",
                    high_priority: false,
                });
                self.last_event_ts_by_type.insert("habit_deviation", now);
            }
        }

        // ==== 自身愿望（NeedsState 驱动，智能体想做什么）====

        // 新鲜感需求高时，智能体想做一些自己感兴趣的事
        // 起始强度低（0.35），需要积累才会表达，避免频繁打扰
        // 不要求 user_present：用户不在场时角色也会有"想做点自己的事"的念头（走内心独白）
        if needs_novelty > 0.7
            && self.check_cooldown("own_desire", now, 1800.0)
        {
            let (desire_desc, desire_hint) = match char_id {
                "nana" | "娜娜" => (
                    "想找个安静的事做做",
                    "你有点想做点自己喜欢的事——比如听听音乐，或者看会儿书。可以问问用户要不要陪你一起",
                ),
                _ => (
                    "想看番/想玩游戏",
                    "你有点想做点自己喜欢的事——比如看会儿番，或者打一把游戏。可以问问用户要不要一起",
                ),
            };
            seeds.push(ThoughtSeed {
                thought_key: "own_desire".into(),
                description: desire_desc.to_string(),
                context_hint: desire_hint.to_string(),
                intensity: 0.35,
                valence: 0.2,
                arousal: 0.4,
                base_desire: 0.6,
                trigger_kind: "own_desire",
                high_priority: false,
            });
            self.last_event_ts_by_type.insert("own_desire", now);
        }

        // ==== 深夜反思（特殊种子，强度高，是"回顾"的契机）====

        if (hour >= 22 || hour <= 1) && !going_to_rest && !waking_up {
            if now - self.last_deep_reflection_ts > 86400.0 {
                seeds.push(ThoughtSeed {
                    thought_key: "deep_reflection".into(),
                    description: "夜深了，回顾今天".into(),
                    context_hint: "夜深了，该回想一下今天发生的事情了".into(),
                    intensity: 0.5,
                    valence: 0.0,
                    arousal: 0.1,
                    base_desire: 0.0,
                    trigger_kind: "deep_reflection",
                    high_priority: false,
                });
                self.last_deep_reflection_ts = now;
            }
        }

        // ==== 背景思考（极低频率的随机思绪）====
        // intensity 设为 0.32（略高于 INNER_MONOLOGUE_THRESHOLD=0.30），确保能触发一次独白；
        // 独白后 mark_monologue_done 衰减 0.35，强度降到 0 以下自然消退，不会刷屏。

        if now - self.last_background_ts > 7200.0 {
            if rand::rng().random_bool(0.15) {
                seeds.push(ThoughtSeed {
                    thought_key: "background".into(),
                    description: "随机思绪".into(),
                    context_hint: "脑子里突然冒出一些零碎的念头，没什么特别的缘由".into(),
                    intensity: 0.32,
                    valence: 0.0,
                    arousal: 0.05,
                    base_desire: 0.0,
                    trigger_kind: "background",
                    high_priority: false,
                });
                self.last_background_ts = now;
            }
        }

        self.last_user_present = Some(user_present);
        self.last_primary_emotion = Some(mood.primary_emotion);

        // 记录事件种子的交互计数供后续使用
        let _ = interaction_count_today;

        seeds
    }

    fn check_cooldown(&self, kind: &'static str, now: f64, cooldown: f64) -> bool {
        match self.last_event_ts_by_type.get(kind) {
            Some(ts) => now - ts >= cooldown,
            None => true,
        }
    }
}

fn relationship_weight(intimacy: f64) -> f32 {
    (0.4 + intimacy as f32 * 0.8).clamp(0.4, 1.2)
}

fn emotion_valence_arousal(label: &EmotionLabel) -> (f32, f32) {
    match label {
        EmotionLabel::Joy => (0.7, 0.5),
        EmotionLabel::Sadness => (-0.6, 0.2),
        EmotionLabel::Anger => (-0.7, 0.8),
        EmotionLabel::Fear => (-0.5, 0.6),
        EmotionLabel::Closeness => (0.8, 0.3),
        EmotionLabel::Loneliness => (-0.6, 0.15),
        EmotionLabel::Curiosity => (0.3, 0.6),
    }
}

fn detect_activity_pattern(entries: &[ActivityEntry]) -> Option<(String, String, u32)> {
    if entries.is_empty() {
        return None;
    }
    let mut counts: HashMap<(String, String), u32> = HashMap::new();
    for e in entries {
        let cat = e.category.clone().unwrap_or_else(|| "其他".to_string());
        if cat == "编程" || cat == "系统" || cat == "其他" {
            continue;
        }
        let title = simplify_window_title(&e.window_title);
        if title.is_empty() || title.len() < 2 {
            continue;
        }
        *counts.entry((title, cat)).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .filter(|(_, c)| *c >= 2)
        .map(|((title, cat), count)| (title, cat, count))
}

fn simplify_window_title(title: &str) -> String {
    let t = title.split(" - ").next().unwrap_or(title);
    let t = t.split(" — ").next().unwrap_or(t);
    let t = t.split(" | ").next().unwrap_or(t);
    t.trim().to_string()
}

fn truncate_snippet(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > 25 {
        format!("{}…", chars[..25].iter().collect::<String>())
    } else {
        text.to_string()
    }
}

fn emotion_display_zh(label: &EmotionLabel) -> &'static str {
    match label {
        EmotionLabel::Joy => "快乐",
        EmotionLabel::Sadness => "悲伤",
        EmotionLabel::Anger => "愤怒",
        EmotionLabel::Fear => "恐惧",
        EmotionLabel::Closeness => "亲近",
        EmotionLabel::Loneliness => "孤独",
        EmotionLabel::Curiosity => "好奇",
    }
}
