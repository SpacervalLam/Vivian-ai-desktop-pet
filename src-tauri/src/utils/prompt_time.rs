use chrono::{DateTime, Local, Offset, TimeZone};

/// Use the user's system timezone, and always include its UTC offset.
pub fn format_prompt_time(timestamp: f64) -> String {
    format_in_timezone(timestamp, &Local)
}

/// Absolute local time with an explicit UTC offset, e.g. `2026-10-31 18:39（UTC+8）`.
pub fn format_local_time(now: DateTime<Local>) -> String {
    format_prompt_time(now.timestamp() as f64)
}

/// Human-readable duration without an "ago" suffix.
///
/// Used by history gap markers and recap headers. Values are rounded to the
/// coarsest useful unit so the prompt does not spend tokens on false precision.
pub fn format_duration(seconds: f64, lang: &str) -> String {
    if !seconds.is_finite() {
        return match lang_kind(lang) {
            "en" => "an unknown time".into(),
            "ja" => "不明な時間".into(),
            _ => "未知时间".into(),
        };
    }
    let secs = seconds.max(0.0);
    match lang_kind(lang) {
        "en" => {
            if secs < 60.0 {
                "under a minute".into()
            } else if secs < 3600.0 {
                format!("{} min", (secs / 60.0).round().max(1.0) as u64)
            } else if secs < 86400.0 {
                let total_min = (secs / 60.0).round() as u64;
                let hours = total_min / 60;
                let mins = total_min % 60;
                if mins == 0 {
                    format!("{hours} h")
                } else {
                    format!("{hours} h {mins} min")
                }
            } else {
                let days = (secs / 86400.0).round().max(1.0) as u64;
                if days == 1 { "1 day".into() } else { format!("{days} days") }
            }
        }
        "ja" => {
            if secs < 60.0 {
                "1分未満".into()
            } else if secs < 3600.0 {
                format!("{}分", (secs / 60.0).round().max(1.0) as u64)
            } else if secs < 86400.0 {
                let total_min = (secs / 60.0).round() as u64;
                let hours = total_min / 60;
                let mins = total_min % 60;
                if mins == 0 {
                    format!("{hours}時間")
                } else {
                    format!("{hours}時間{mins}分")
                }
            } else {
                let days = (secs / 86400.0).round().max(1.0) as u64;
                format!("{days}日")
            }
        }
        _ => {
            if secs < 60.0 {
                "不到1分钟".into()
            } else if secs < 3600.0 {
                format!("{}分钟", (secs / 60.0).round().max(1.0) as u64)
            } else if secs < 86400.0 {
                let total_min = (secs / 60.0).round() as u64;
                let hours = total_min / 60;
                let mins = total_min % 60;
                if mins == 0 {
                    format!("{hours}小时")
                } else {
                    format!("{hours}小时{mins}分钟")
                }
            } else {
                let days = (secs / 86400.0).round().max(1.0) as u64;
                format!("{days}天")
            }
        }
    }
}

/// Relative age from *now*, e.g. `刚刚`, `5 分钟前`, `3 小时前`.
pub fn format_age_seconds(seconds: f64, lang: &str) -> String {
    if !seconds.is_finite() {
        return match lang_kind(lang) {
            "en" => "at an unknown time".into(),
            "ja" => "不明な時刻".into(),
            _ => "时间未知".into(),
        };
    }
    let secs = seconds.max(0.0);
    if secs < 60.0 {
        return match lang_kind(lang) {
            "en" => "just now".into(),
            "ja" => "たった今".into(),
            _ => "刚刚".into(),
        };
    }
    if secs < 3600.0 {
        let mins = (secs / 60.0).round().max(1.0) as u64;
        return match lang_kind(lang) {
            "en" => format!("{mins} min ago"),
            "ja" => format!("{mins}分前"),
            _ => format!("{mins} 分钟前"),
        };
    }
    if secs < 86400.0 {
        let hours = (secs / 3600.0).round().max(1.0) as u64;
        return match lang_kind(lang) {
            "en" => format!("{hours} h ago"),
            "ja" => format!("{hours}時間前"),
            _ => format!("{hours} 小时前"),
        };
    }
    let days = (secs / 86400.0).round().max(1.0) as u64;
    match lang_kind(lang) {
        "en" => format!("{days} days ago"),
        "ja" => format!("{days}日前"),
        _ => format!("{days} 天前"),
    }
}

/// Relative age of a timestamp with local date context.
///
/// Within the same day it shows clock time plus age; across days it keeps a
/// date anchor so an old event cannot be mistaken for something from this
/// conversation.
pub fn format_relative_age_at(timestamp: DateTime<Local>, now: DateTime<Local>, lang: &str) -> String {
    let age = (now - timestamp).num_seconds().max(0) as f64;
    let age_text = format_age_seconds(age, lang);
    if timestamp.date_naive() == now.date_naive() {
        return match lang_kind(lang) {
            "en" => format!("{} ({age_text})", timestamp.format("%H:%M")),
            "ja" => format!("{}（{age_text}）", timestamp.format("%H:%M")),
            _ => format!("{}（{age_text}）", timestamp.format("%H:%M")),
        };
    }
    let days = (now.date_naive() - timestamp.date_naive()).num_days();
    let day_anchor = match lang_kind(lang) {
        "en" => {
            if days == 1 { "yesterday".to_string() }
            else if days == 2 { "two days ago".to_string() }
            else { timestamp.format("%Y-%m-%d").to_string() }
        }
        "ja" => {
            if days == 1 { "昨日".to_string() }
            else if days == 2 { "一昨日".to_string() }
            else { timestamp.format("%Y-%m-%d").to_string() }
        }
        _ => {
            if days == 1 { "昨天".to_string() }
            else if days == 2 { "前天".to_string() }
            else { timestamp.format("%Y-%m-%d").to_string() }
        }
    };
    match lang_kind(lang) {
        "en" => format!("{day_anchor} {} ({age_text})", timestamp.format("%H:%M")),
        "ja" => format!("{day_anchor} {}（{age_text}）", timestamp.format("%H:%M")),
        _ => format!("{day_anchor} {}（{age_text}）", timestamp.format("%H:%M")),
    }
}

/// Unix 秒时间戳 → 相对年龄标签，无效时间戳返回本地化的"时间不详"。
///
/// 记忆召回是唯一需要从裸 `f64` 生成时间标签的地方（存档与对话历史直接持有
/// `DateTime`）。集中在这里，日界判据才不会在不同模块之间漂移。
pub fn format_timestamp_age(timestamp: f64, now: DateTime<Local>, lang: &str) -> String {
    if !timestamp.is_finite() || timestamp <= 0.0 {
        return unknown_time(lang);
    }
    match DateTime::from_timestamp(timestamp as i64, 0) {
        Some(dt) => format_relative_age_at(dt.with_timezone(&Local), now, lang),
        None => unknown_time(lang),
    }
}

fn unknown_time(lang: &str) -> String {
    match lang_kind(lang) {
        "en" => "unknown time".into(),
        "ja" => "時期不明".into(),
        _ => "时间不详".into(),
    }
}

fn lang_kind(lang: &str) -> &'static str {
    let normalized = lang.trim().to_ascii_lowercase();
    if normalized.starts_with("en") { "en" }
    else if normalized.starts_with("ja") { "ja" }
    else { "zh" }
}

fn format_in_timezone<T: TimeZone>(timestamp: f64, timezone: &T) -> String
where T::Offset: std::fmt::Display {
    if !timestamp.is_finite() { return "时间未知".into(); }
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|time| {
            let local = time.with_timezone(timezone);
            let seconds = local.offset().fix().local_minus_utc();
            let sign = if seconds < 0 { '-' } else { '+' };
            let hours = seconds.abs() / 3600;
            let minutes = seconds.abs() % 3600 / 60;
            let offset = if minutes == 0 { format!("{sign}{hours}") }
                else { format!("{sign}{hours}:{minutes:02}") };
            format!("{}（UTC{}）", local.format("%Y-%m-%d %H:%M"), offset)
        })
        .unwrap_or_else(|| "时间未知".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn timestamps_include_local_date_and_offset() {
        let china = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(format_in_timezone(1791430800., &china), "2026-10-08 11:40（UTC+8）");
        let west = chrono::FixedOffset::west_opt(7 * 3600).unwrap();
        assert_eq!(format_in_timezone(1791430800., &west), "2026-10-07 20:40（UTC-7）");
        let half_hour = chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
        assert_eq!(format_in_timezone(1791430800., &half_hour), "2026-10-08 09:10（UTC+5:30）");
        assert_eq!(format_in_timezone(f64::NAN, &china), "时间未知");
    }

    #[test] fn duration_and_age_use_human_units() {
        assert_eq!(format_duration(30., "zh"), "不到1分钟");
        assert_eq!(format_duration(90., "zh"), "2分钟");
        assert_eq!(format_duration(2. * 3600. + 17. * 60., "zh"), "2小时17分钟");
        assert_eq!(format_duration(2. * 86400., "zh"), "2天");
        assert_eq!(format_age_seconds(30., "zh"), "刚刚");
        assert_eq!(format_age_seconds(300., "zh"), "5 分钟前");
        assert_eq!(format_age_seconds(7200., "zh"), "2 小时前");
        assert_eq!(format_age_seconds(30., "en"), "just now");
        assert_eq!(format_age_seconds(300., "ja"), "5分前");
    }

    #[test] fn relative_age_keeps_date_anchor_across_day_boundary() {
        let now = Local::now();
        let same_day = format_relative_age_at(now - chrono::Duration::minutes(5), now, "zh");
        assert!(same_day.contains("分钟前"), "{same_day}");
        let older = format_relative_age_at(now - chrono::Duration::days(2), now, "zh");
        assert!(older.contains("2 天前") || older.contains("前天"), "{older}");
    }

    #[test] fn raw_timestamps_share_the_same_boundary_and_language() {
        let now = Local::now();
        let ts = (now - chrono::Duration::hours(3)).timestamp() as f64;
        assert!(format_timestamp_age(ts, now, "zh").contains("小时前"));
        assert!(format_timestamp_age(ts, now, "en").contains("h ago"));
        assert!(format_timestamp_age(ts, now, "ja").contains("時間前"));
        assert_eq!(format_timestamp_age(0.0, now, "zh"), "时间不详");
        assert_eq!(format_timestamp_age(f64::NAN, now, "en"), "unknown time");
        // 昨天的同一时刻必须落在日期锚点上，而不是只给小时数。
        let yesterday = (now - chrono::Duration::days(1)).timestamp() as f64;
        let label = format_timestamp_age(yesterday, now, "zh");
        assert!(label.contains("昨天") || label.contains("前天") || label.contains("天前"), "{label}");
    }
}
