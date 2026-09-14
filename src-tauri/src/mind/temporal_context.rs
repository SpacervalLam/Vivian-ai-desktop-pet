//! Temporal Context Builder —— 把离散世界事实合成为关系型时间事实。
//!
//! 输入：WorldBrief（瞬时事实）+ UserGoalLedger 摘要（长期目标）
//! 输出：Vec<String> 时间关系事实清单，作为 prompt 段落注入 thought_synthesis
//!       和 inner_monologue，让 LLM 不必从离散事实现算关系。
//!
//! 设计原则：
//! - 纯函数，无锁，无 IO，无 LLM 调用
//! - 零跨模块依赖：只读 WorldBrief + UserGoalBrief，不访问 UserBehaviorLog/Ledger
//! - 阈值保守：只在显著情境下产出事实，避免每条都重复
//! - 三语化：根据 lang 参数输出 zh/en/ja

use crate::mind::thought_synthesis::WorldBrief;
use crate::mind::user_goals::UserGoalBrief;

/// 时间事实类型（供调试/日志区分）
#[derive(Debug, Clone, Copy)]
pub enum TemporalFactKind {
    /// 持续时长类："用户已连续编码 3 小时"
    Duration,
    /// 时段类："现在是凌晨 2 点"
    TimeOfDay,
    /// 饭点类："接近晚饭时间"
    MealTime,
    /// 长期目标紧迫类："考研还有 5 天"
    Deadline,
    /// 离开异常类（与 UserEntitySnapshot 已有提示互补，仅在长时间异常时产出）
    AwayAnomaly,
    /// 累积事实类（跨事实组合）："凌晨 + 连续工作"
    Compound,
}

/// 单条时间事实
#[derive(Debug, Clone)]
pub struct TemporalFact {
    pub kind: TemporalFactKind,
    pub text: String,
}

/// Builder 入口：从 WorldBrief + 长期目标摘要产出时间关系事实
pub fn build_temporal_facts(
    brief: &WorldBrief,
    goals: &[UserGoalBrief],
    lang: &str,
) -> Vec<TemporalFact> {
    let mut facts: Vec<TemporalFact> = Vec::new();
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

    // 1. 时段类：解析 local_time 中的 hour
    if let Some(hour) = parse_hour(&brief.local_time) {
        if let Some(text) = time_of_day_fact(hour, lang_norm) {
            facts.push(TemporalFact { kind: TemporalFactKind::TimeOfDay, text });
        }
        if let Some(text) = meal_time_fact(hour, lang_norm) {
            facts.push(TemporalFact { kind: TemporalFactKind::MealTime, text });
        }
    }

    // 2. 持续时长类：用户当前活动已持续多久
    if let Some(activity) = &brief.user_activity {
        if let Some(elapsed_secs) = brief.user_activity_elapsed_secs {
            if let Some(text) = duration_fact(activity, elapsed_secs, lang_norm) {
                facts.push(TemporalFact { kind: TemporalFactKind::Duration, text });
            }
        }
    }

    // 3. 长期目标紧迫类
    for goal in goals {
        if let Some(text) = deadline_fact(goal, lang_norm) {
            facts.push(TemporalFact { kind: TemporalFactKind::Deadline, text });
        }
    }

    // 4. 离开异常类：away_secs > 2h 且 user 未回来
    if brief.user_presence == crate::world::UserPresence::Away && brief.user_away_secs > 7200.0 {
        if let Some(text) = away_anomaly_fact(brief.user_away_secs, lang_norm) {
            facts.push(TemporalFact { kind: TemporalFactKind::AwayAnomaly, text });
        }
    }

    // 5. 组合事实：凌晨 + 连续工作（最容易触发疲劳风险）
    if let Some(hour) = parse_hour(&brief.local_time) {
        if hour < 5 {
            if let Some(activity) = &brief.user_activity {
                if let Some(elapsed) = brief.user_activity_elapsed_secs {
                    if elapsed > 3600.0 && is_work_activity(activity) {
                        if let Some(text) =
                            compound_late_night_work_fact(elapsed, lang_norm)
                        {
                            facts.push(TemporalFact { kind: TemporalFactKind::Compound, text });
                        }
                    }
                }
            }
        }
    }

    facts
}

/// 序列化为 prompt 段落（供 WorldBrief 注入）
pub fn serialize_temporal_facts(facts: &[TemporalFact], lang: &str) -> Option<String> {
    if facts.is_empty() {
        return None;
    }
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    let header = match lang_norm {
        "en" => "## Temporal context (relationships between facts)",
        "ja" => "## 時間的文脈（事実間の関係）",
        _ => "## 时间关系（事实之间的关联）",
    };
    let mut lines = vec![header.to_string()];
    for f in facts {
        lines.push(format!("- {}", f.text));
    }
    Some(lines.join("\n"))
}

// ── 内部规则函数 ──

fn parse_hour(local_time: &str) -> Option<u32> {
    // local_time 由 WorldStateProvider 格式化为 "HH:MM" 或类似形式
    // 取前 2 位作为 hour
    if local_time.len() < 2 {
        return None;
    }
    let prefix: String = local_time.chars().take(2).collect();
    prefix.parse::<u32>().ok().filter(|h| *h < 24)
}

fn time_of_day_fact(hour: u32, lang: &str) -> Option<String> {
    match lang {
        "en" => match hour {
            0..=4 => Some(format!("It's {}am and you're still wide awake", hour)),
            5..=6 => Some("The sun's barely up".to_string()),
            22..=23 => Some(format!("It's getting late ({}:00), you're a bit drowsy", hour)),
            _ => None,
        },
        "ja" => match hour {
            0..=4 => Some(format!("深夜（{}時）、まだ眠くない", hour)),
            5..=6 => Some("朝が来た".to_string()),
            22..=23 => Some(format!("夜更け（{}時）、少し眠くなってきた", hour)),
            _ => None,
        },
        _ => match hour {
            0..=4 => Some(format!("现在凌晨 {} 点了，你还醒着", hour)),
            5..=6 => Some("天刚亮".to_string()),
            22..=23 => Some(format!("现在深夜 {} 点了，你有点困了", hour)),
            _ => None,
        },
    }
}

fn meal_time_fact(hour: u32, lang: &str) -> Option<String> {
    let (lunch, dinner, late_snack) = match lang {
        "en" => ("It's lunch time", "It's dinner time", "It's late-night snack time"),
        "ja" => ("昼食の時間", "夕食の時間", "夜食の時間"),
        _ => ("接近午饭时间", "接近晚饭时间", "接近夜宵时间"),
    };
    match hour {
        11..=13 => Some(lunch.to_string()),
        17..=19 => Some(dinner.to_string()),
        21..=23 => Some(late_snack.to_string()),
        _ => None,
    }
}

fn duration_fact(activity: &str, elapsed_secs: f64, lang: &str) -> Option<String> {
    let hours = elapsed_secs / 3600.0;
    if hours < 1.0 {
        return None;
    }
    let rounded = (hours * 10.0).round() / 10.0;
    match lang {
        "en" => Some(format!("User has been {} for {:.1}h continuously", activity.to_lowercase(), rounded)),
        "ja" => Some(format!("ユーザーは「{}」を {:.1} 時間連続している", activity, rounded)),
        _ => Some(format!("用户已连续{} {:.1} 小时", activity, rounded)),
    }
}

fn deadline_fact(goal: &UserGoalBrief, lang: &str) -> Option<String> {
    let days = goal.days_to_deadline?;
    // 仅在 7 天内紧迫或已过期时产出
    if days > 7 {
        return None;
    }
    match lang {
        "en" => {
            if days > 0 {
                Some(format!("\"{}\" deadline in {} days", goal.label, days))
            } else if days == 0 {
                Some(format!("\"{}\" deadline is today", goal.label))
            } else {
                Some(format!("\"{}\" deadline {} days overdue", goal.label, -days))
            }
        }
        "ja" => {
            if days > 0 {
                Some(format!("「{}」の期限まで残り{}日", goal.label, days))
            } else if days == 0 {
                Some(format!("「{}」の期限は今日", goal.label))
            } else {
                Some(format!("「{}」の期限を{}日超過", goal.label, -days))
            }
        }
        _ => {
            if days > 0 {
                Some(format!("「{}」还有 {} 天到期", goal.label, days))
            } else if days == 0 {
                Some(format!("「{}」今天到期", goal.label))
            } else {
                Some(format!("「{}」已超期 {} 天", goal.label, -days))
            }
        }
    }
}

fn away_anomaly_fact(away_secs: f64, lang: &str) -> Option<String> {
    let hours = away_secs / 3600.0;
    let rounded = (hours * 10.0).round() / 10.0;
    match lang {
        "en" => Some(format!("User has been away for {:.1}h (long absence)", rounded)),
        "ja" => Some(format!("ユーザーが {:.1} 時間不在（長時間離席）", rounded)),
        _ => Some(format!("用户已离开 {:.1} 小时（长时间未归）", rounded)),
    }
}

fn compound_late_night_work_fact(elapsed_secs: f64, lang: &str) -> Option<String> {
    let hours = elapsed_secs / 3600.0;
    let rounded = (hours * 10.0).round() / 10.0;
    match lang {
        "en" => Some(format!("Late-night continuous work ({:.1}h), fatigue risk rising", rounded)),
        "ja" => Some(format!("深夜の連続作業（{:.1}時間）、疲労リスク上昇", rounded)),
        _ => Some(format!("深夜连续工作 {:.1} 小时，疲劳风险上升", rounded)),
    }
}

/// 判断活动是否属于"工作类"（用于组合事实）
fn is_work_activity(label: &str) -> bool {
    let work_keywords = ["写代码", "编程", "工作", "加班", "学习", "写论文", "改论文", "做作业", "复习", "备考"];
    work_keywords.iter().any(|k| label.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::user_goals::UserGoalState;
    use crate::world::UserPresence;

    fn make_brief(hour: u32) -> WorldBrief {
        let mut brief = WorldBrief::default();
        brief.local_time = format!("{:02}:30", hour);
        brief
    }

    #[test]
    fn test_parse_hour() {
        assert_eq!(parse_hour("18:30"), Some(18));
        assert_eq!(parse_hour("02:00"), Some(2));
        assert_eq!(parse_hour("invalid"), None);
    }

    #[test]
    fn test_time_of_day_late_night() {
        let brief = make_brief(2);
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("凌晨")));
    }

    #[test]
    fn test_meal_time() {
        let brief = make_brief(12);
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("午饭")));
    }

    #[test]
    fn test_duration_fact_below_one_hour() {
        let mut brief = make_brief(14);
        brief.user_activity = Some("写代码".to_string());
        brief.user_activity_elapsed_secs = Some(1800.0); // 30 min
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(!facts.iter().any(|f| matches!(f.kind, TemporalFactKind::Duration)));
    }

    #[test]
    fn test_duration_fact_above_one_hour() {
        let mut brief = make_brief(14);
        brief.user_activity = Some("写代码".to_string());
        brief.user_activity_elapsed_secs = Some(3.5 * 3600.0);
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("连续写代码") && f.text.contains("3.5")));
    }

    #[test]
    fn test_deadline_fact_within_7_days() {
        let brief = make_brief(14);
        let goals = vec![UserGoalBrief {
            label: "考研".to_string(),
            days_to_deadline: Some(3),
            state: UserGoalState::Active,
        }];
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("考研") && f.text.contains("3 天")));
    }

    #[test]
    fn test_deadline_fact_far_future_skipped() {
        let brief = make_brief(14);
        let goals = vec![UserGoalBrief {
            label: "考研".to_string(),
            days_to_deadline: Some(100),
            state: UserGoalState::Active,
        }];
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(!facts.iter().any(|f| matches!(f.kind, TemporalFactKind::Deadline)));
    }

    #[test]
    fn test_compound_late_night_work() {
        let mut brief = make_brief(2);
        brief.user_activity = Some("写代码".to_string());
        brief.user_activity_elapsed_secs = Some(2.0 * 3600.0);
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("疲劳风险")));
    }

    #[test]
    fn test_away_anomaly() {
        let mut brief = make_brief(14);
        brief.user_presence = UserPresence::Away;
        brief.user_away_secs = 3.0 * 3600.0; // 3h
        let goals: Vec<UserGoalBrief> = Vec::new();
        let facts = build_temporal_facts(&brief, &goals, "zh");
        assert!(facts.iter().any(|f| f.text.contains("长时间未归")));
    }

    #[test]
    fn test_serialize_empty_returns_none() {
        let facts: Vec<TemporalFact> = Vec::new();
        assert!(serialize_temporal_facts(&facts, "zh").is_none());
    }

    #[test]
    fn test_serialize_with_header() {
        let facts = vec![TemporalFact {
            kind: TemporalFactKind::TimeOfDay,
            text: "test".to_string(),
        }];
        let s = serialize_temporal_facts(&facts, "zh").unwrap();
        assert!(s.contains("时间关系"));
        assert!(s.contains("- test"));
    }
}
