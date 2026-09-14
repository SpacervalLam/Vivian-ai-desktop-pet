//! 健康提醒
//!
//! 纯函数式接口，基于当前时间 + 用户活跃状态生成提醒文本。
//! 优先级：睡眠 > 吃饭 > 喝水 > 休息。

use std::collections::HashMap;

use super::super::random_index;

/// 喝水间隔（秒）
pub const WATER_INTERVAL_SECONDS: f64 = 45.0 * 60.0;
/// 休息间隔：连续活跃分钟数阈值
pub const REST_INTERVAL_MINUTES: u32 = 120;
/// 休息提醒重复间隔（秒）
pub const REST_REMINDER_INTERVAL_SECONDS: f64 = 60.0 * 60.0;

/// 吃饭时段（餐名 → (起始小时, 结束小时)）
fn meal_windows() -> Vec<(&'static str, u32, u32)> {
    vec![
        ("早餐", 6, 9),
        ("午餐", 11, 14),
        ("晚餐", 17, 20),
        ("夜宵", 22, 24),
    ]
}

const WATER_PROMPTS: &[&str] = &[
    "到时间喝水啦~",
    "该喝杯水了哦！",
    "喝水时间到，补充水分~",
    "别忙忘了喝水呀！",
];

const REST_PROMPTS: &[&str] = &[
    "一直坐着不太好，起来活动一下吧~",
    "休息一下眼睛，看看远处吧~",
    "工作累了？歇会儿再继续~",
    "起来走走，换个心情~",
];

const SLEEP_PROMPTS: &[&str] = &[
    "已经很晚了，该休息啦~",
    "熬夜对身体不好哦，早点睡吧~",
    "很晚了，还不打算睡吗？",
    "夜深了，注意休息哦~",
];

/// 健康提醒生成器
pub struct HealthReminder;

impl HealthReminder {
    /// 检查喝水提醒
    pub fn check_water(last_water_reminder_time: f64, now: f64) -> Option<&'static str> {
        if now - last_water_reminder_time >= WATER_INTERVAL_SECONDS {
            let idx = random_index(WATER_PROMPTS.len());
            Some(WATER_PROMPTS[idx])
        } else {
            None
        }
    }

    /// 检查休息提醒
    pub fn check_rest(
        sustained_active_minutes: u32,
        last_rest_reminder_time: f64,
        now: f64,
    ) -> Option<&'static str> {
        if sustained_active_minutes < REST_INTERVAL_MINUTES {
            return None;
        }
        if now - last_rest_reminder_time < REST_REMINDER_INTERVAL_SECONDS {
            return None;
        }
        let idx = random_index(REST_PROMPTS.len());
        Some(REST_PROMPTS[idx])
    }

    /// 检查吃饭提醒
    ///
    /// `last_meal_reminder`：餐名 → 上次提醒时间戳
    pub fn check_meal(
        last_meal_reminder: &HashMap<String, f64>,
        now: f64,
        hour: u32,
        minute: u32,
    ) -> Option<String> {
        let current_hour = hour as f64 + minute as f64 / 60.0;
        for (meal_name, start, end) in meal_windows() {
            let start = start as f64;
            let end = end as f64;
            // 时段开始前 30 分钟提醒
            if start <= current_hour && current_hour <= start + 0.5 {
                let last = last_meal_reminder.get(meal_name).copied().unwrap_or(0.0);
                if now - last > 3600.0 * 20.0 {
                    return Some(format!("{meal_name}时间到啦~记得吃饭哦！"));
                }
            } else if end - 0.5 <= current_hour && current_hour <= end + 0.5 {
                // 时段刚过提醒"吃了吗"
                let last = last_meal_reminder.get(meal_name).copied().unwrap_or(0.0);
                if now - last > 3600.0 * 20.0 {
                    return Some(format!("{meal_name}吃了吗？别饿着自己~"));
                }
            }
        }
        None
    }

    /// 检查睡眠提醒
    pub fn check_sleep(
        user_bedtime_hour: Option<u32>,
        last_sleep_reminder_time: f64,
        now: f64,
        hour: u32,
    ) -> Option<&'static str> {
        let bed_hour = user_bedtime_hour.unwrap_or(23);
        if hour >= bed_hour {
            if now - last_sleep_reminder_time > 3600.0 * 20.0 {
                let idx = random_index(SLEEP_PROMPTS.len());
                return Some(SLEEP_PROMPTS[idx]);
            }
        }
        None
    }

    /// 一次调用检查所有提醒，返回优先级最高的提醒文本
    ///
    /// 优先级：睡眠 > 吃饭 > 喝水 > 休息
    pub fn check_all(
        sustained_active_minutes: u32,
        last_reminder_times: &HashMap<String, f64>,
        user_bedtime_hour: Option<u32>,
        now: f64,
        hour: u32,
        minute: u32,
    ) -> Option<String> {
        // 1. 睡眠
        if let Some(t) = Self::check_sleep(
            user_bedtime_hour,
            last_reminder_times.get("sleep").copied().unwrap_or(0.0),
            now,
            hour,
        ) {
            return Some(t.to_string());
        }
        // 2. 吃饭（meal 子键）
        let meal_map: HashMap<String, f64> = last_reminder_times
            .iter()
            .filter(|(k, _)| k.starts_with("meal:"))
            .map(|(k, v)| (k.trim_start_matches("meal:").to_string(), *v))
            .collect();
        if let Some(t) = Self::check_meal(&meal_map, now, hour, minute) {
            return Some(t);
        }
        // 3. 喝水
        if let Some(t) = Self::check_water(
            last_reminder_times.get("water").copied().unwrap_or(0.0),
            now,
        ) {
            return Some(t.to_string());
        }
        // 4. 休息
        if let Some(t) = Self::check_rest(
            sustained_active_minutes,
            last_reminder_times.get("rest").copied().unwrap_or(0.0),
            now,
        ) {
            return Some(t.to_string());
        }
        None
    }
}
