//! 天气摘要 —— Open-Meteo + 可选 Apple WeatherKit + 空气质量。
//!
//! 失败即"不知道"：网络错误/超时/解析失败均返回 Err，由调用方保留旧缓存或 None，
//! 不做任何时间推断兜底（用户明确要求）。
//!
//! Open-Meteo 文档：https://open-meteo.com/en/docs

use serde::{Deserialize, Serialize};

use super::weather_data::{fetch_reports, AirQuality, SourceStatus, WeatherSummary};
use crate::error::{VivianError, VivianResult};

/// 天气快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherSnapshot {
    /// 温度（℃）
    pub temperature: f64,
    /// 体感温度（℃）
    pub feels_like: f64,
    /// WMO 天气代码
    pub weather_code: u32,
    /// 中文描述（"晴"/"多云"/"小雨"等）
    pub description: String,
    /// 是否正在降水
    pub is_precipitating: bool,
    /// 风速（km/h）
    pub wind_speed: f64,
    /// 湿度（%）
    pub humidity: f64,
    /// 日出小时（本地时区，如 5.7 表示 5:42），API 未返回时为 None
    pub sunrise_hour: Option<f64>,
    /// 日落小时（本地时区，如 19.2 表示 19:12），API 未返回时为 None
    pub sunset_hour: Option<f64>,
    /// API 计算的昼夜标记（fetch 时刻），坐标时区口径，不受本地时区错位影响
    #[serde(default)]
    pub is_day: Option<bool>,
    /// 数据来源（如 "Open-Meteo"），供前端展示
    pub weather_source: String,
    /// 缓存时间戳（UTC 秒）
    pub cached_at: i64,
    #[serde(default)]
    pub sources: Vec<WeatherSummary>,
    #[serde(default)]
    pub air_quality: Option<AirQuality>,
    #[serde(default)]
    pub source_status: Vec<SourceStatus>,
}

/// 天气数据源
pub struct WeatherSource;

impl WeatherSource {
    pub fn new() -> Self {
        Self
    }

    pub async fn fetch_with_config(
        &self,
        lat: f64,
        lon: f64,
        config: &crate::config::WorldConfig,
    ) -> VivianResult<WeatherSnapshot> {
        let reports = fetch_reports(lat, lon, config, 1, 12).await;
        let primary = reports
            .providers
            .iter()
            .find(|p| {
                let c = &p.current;
                c.temperature.is_some()
                    && c.feels_like.is_some()
                    && c.humidity_pct.is_some()
                    && c.wind_speed_kmh.is_some()
            })
            .ok_or_else(|| VivianError::Network("所有天气来源均不可用或当前数据不完整".into()))?;
        let c = &primary.current;
        let temperature = c
            .temperature
            .ok_or_else(|| VivianError::Network("当前气温缺失".into()))?;
        let feels_like = c
            .feels_like
            .ok_or_else(|| VivianError::Network("当前体感气温缺失".into()))?;
        let humidity = c
            .humidity_pct
            .ok_or_else(|| VivianError::Network("当前湿度缺失".into()))?;
        let wind_speed = c
            .wind_speed_kmh
            .ok_or_else(|| VivianError::Network("当前风速缺失".into()))?;
        let code = c.weather_code.unwrap_or(999);
        let sun = primary.daily.first();
        let parse_hour = |s: &str| {
            // Apple timestamps are UTC; convert to the system timezone used by the room.
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                use chrono::Timelike;
                let t = t.with_timezone(&chrono::Local);
                return Some(t.hour() as f64 + t.minute() as f64 / 60.0);
            }
            let local = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok()?;
            use chrono::Timelike;
            Some(local.hour() as f64 + local.minute() as f64 / 60.0)
        };
        Ok(WeatherSnapshot {
            temperature,
            feels_like,
            weather_code: code,
            description: c.description.clone().unwrap_or_else(|| "未知".into()),
            is_precipitating: c.weather_code.map(is_precipitating).unwrap_or_else(|| {
                c.condition_code.as_deref().is_some_and(|d| {
                    matches!(
                        d,
                        "Rain"
                            | "Drizzle"
                            | "HeavyRain"
                            | "Snow"
                            | "HeavySnow"
                            | "Thunderstorms"
                            | "Sleet"
                            | "FreezingRain"
                            | "FreezingDrizzle"
                            | "Hail"
                            | "Blizzard"
                            | "ScatteredThunderstorms"
                            | "StrongStorms"
                            | "IsolatedThunderstorms"
                            | "Flurries"
                    )
                })
            }),
            wind_speed,
            humidity,
            sunrise_hour: sun.and_then(|d| d.sunrise.as_deref()).and_then(parse_hour),
            sunset_hour: sun.and_then(|d| d.sunset.as_deref()).and_then(parse_hour),
            is_day: c.is_day,
            weather_source: primary.source.clone(),
            cached_at: chrono::Utc::now().timestamp(),
            sources: reports.providers.iter().map(|p| p.summary()).collect(),
            air_quality: reports.air_quality,
            source_status: reports.source_status,
        })
    }
}

impl WeatherSnapshot {
    pub fn context_summary(&self) -> String {
        let number = |n: Option<f64>| {
            n.map(|n| format!("{n:.0}"))
                .unwrap_or_else(|| "unknown".into())
        };
        let mut lines = Vec::new();
        for s in &self.sources {
            let mut line = format!(
                "{}: {} {}°C, at {} (daily timezone {}); humidity {}%, sea-level pressure {}hPa",
                s.source,
                s.description.as_deref().unwrap_or("unknown"),
                number(s.temperature),
                s.observed_at.as_deref().unwrap_or("unknown"),
                s.timezone,
                number(s.humidity_pct),
                number(s.pressure_hpa)
            );
            if let (Some(low), Some(high)) = (s.temp_min, s.temp_max) {
                line.push_str(&format!(
                    "; today {low:.0}–{high:.0}°C, range {:.0}°C",
                    high - low
                ));
            }
            if let (Some(time), Some(prob)) = (
                &s.next_precipitation_time,
                s.next_precipitation_probability_pct,
            ) {
                line.push_str(&format!(
                    "; next {}h precipitation forecast {time}: {prob:.0}%",
                    s.precipitation_window_hours
                ));
            } else if s.precipitation_probability_hours == 12 {
                line.push_str("; next 12h: precipitation probability below 30%");
            } else {
                line.push_str("; next 12h precipitation forecast incomplete/unknown");
            }
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push(format!("{} {:.0}°C", self.description, self.temperature));
        }
        if let Some(q) = &self.air_quality {
            lines.push(format!(
                "{}: {} {}, PM2.5 {}µg/m³, at {}",
                q.source,
                q.standard,
                number(q.aqi),
                number(q.pm2_5_ug_m3),
                q.time.as_deref().unwrap_or("unknown")
            ));
        }
        for status in &self.source_status {
            if status.status == "error" {
                lines.push(format!("{} unavailable", status.source));
            }
        }
        lines.push(format!("Fetched at UTC unix {}. Compare sources at matching times; do not average. Full hourly / 10-day forecasts: get_weather_forecast.", self.cached_at));
        lines.join(" | ")
    }
}

impl Default for WeatherSource {
    fn default() -> Self {
        Self::new()
    }
}

/// WMO 天气代码 → 中文描述
pub fn weather_code_to_desc(code: u32) -> String {
    match code {
        0 => "晴".to_string(),
        1 => "多云".to_string(),
        2 => "局部多云".to_string(),
        3 => "阴".to_string(),
        45 => "雾".to_string(),
        48 => "冻雾".to_string(),
        51 => "小毛毛雨".to_string(),
        53 => "中毛毛雨".to_string(),
        55 => "大毛毛雨".to_string(),
        56 => "冻毛毛雨".to_string(),
        57 => "强冻毛毛雨".to_string(),
        61 => "小雨".to_string(),
        63 => "中雨".to_string(),
        65 => "大雨".to_string(),
        66 => "冻雨".to_string(),
        67 => "强冻雨".to_string(),
        71 => "小雪".to_string(),
        73 => "中雪".to_string(),
        75 => "大雪".to_string(),
        77 => "雪粒".to_string(),
        80 => "小阵雨".to_string(),
        81 => "中阵雨".to_string(),
        82 => "大阵雨".to_string(),
        85 => "小阵雪".to_string(),
        86 => "大阵雪".to_string(),
        95 => "雷阵雨".to_string(),
        96 => "雷阵雨伴小冰雹".to_string(),
        99 => "雷阵雨伴大冰雹".to_string(),
        _ => "未知".to_string(),
    }
}

pub fn is_precipitating(code: u32) -> bool {
    // 51-67: 雨/冻雨  71-77: 雪  80-82: 阵雨  85-86: 阵雪  95-99: 雷雨
    (51..=67).contains(&code)
        || (71..=77).contains(&code)
        || (80..=82).contains(&code)
        || (85..=86).contains(&code)
        || (95..=99).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weather_context_is_compact_and_does_not_claim_missing_rain_data_is_dry() {
        let mut w: WeatherSnapshot = serde_json::from_value(serde_json::json!({
            "temperature":20,"feels_like":19,"weather_code":3,"description":"阴","is_precipitating":false,
            "wind_speed":7,"humidity":60,"weather_source":"Open-Meteo","cached_at":1234,
        })).unwrap();
        w.sources.push(WeatherSummary {
            source: "Open-Meteo".into(),
            temperature: Some(20.0),
            temp_max: Some(25.0),
            temp_min: Some(15.0),
            observed_at: Some("2026-10-07T23:45".into()),
            timezone: "Asia/Shanghai".into(),
            ..WeatherSummary::default()
        });
        let text = w.context_summary();
        assert!(text.contains("range 10°C"));
        assert!(text.contains("incomplete/unknown"));
        assert!(!text.contains("Some("));
        assert!(text.len() < 1500);
        let value = serde_json::to_value(w).unwrap();
        assert!(value.get("hourly").is_none());
        assert!(value.get("daily").is_none());
    }
}
