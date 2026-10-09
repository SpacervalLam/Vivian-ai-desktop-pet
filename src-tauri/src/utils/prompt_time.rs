use chrono::{Local, Offset, TimeZone};

/// Use the user's system timezone, and always include its UTC offset.
pub fn format_prompt_time(timestamp: f64) -> String {
    format_in_timezone(timestamp, &Local)
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
}
