//! 世界事件 —— 真实世界变化产生的事件，映射到 Appraisal 进而影响心理状态。
//!
//! 大部分世界事件只影响内部状态，不打扰用户。
//! 仅当事件显著度高 + 时机合适 + 关系亲密度足够时，才转为主动消息。

use serde::{Deserialize, Serialize};

use crate::psychology::appraisal::Appraisal;

use super::{Festival, SolarTerm};

/// 世界事件类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorldEventKind {
    /// 天气变化（如晴→雨）
    WeatherChanged,
    /// 降水开始（无雨→有雨）
    RainStarted,
    /// 节日到来
    FestivalArrived,
    /// 节气交替
    SolarTermChanged,
    /// 日出
    Sunrise,
    /// 日落
    Sunset,
    /// 季节变化
    SeasonChanged,
    /// 用户长时间未交互（世界继续转动）
    LongAbsence,
}

impl WorldEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorldEventKind::WeatherChanged => "weather_changed",
            WorldEventKind::RainStarted => "rain_started",
            WorldEventKind::FestivalArrived => "festival_arrived",
            WorldEventKind::SolarTermChanged => "solar_term_changed",
            WorldEventKind::Sunrise => "sunrise",
            WorldEventKind::Sunset => "sunset",
            WorldEventKind::SeasonChanged => "season_changed",
            WorldEventKind::LongAbsence => "long_absence",
        }
    }
}

/// 世界事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    pub kind: WorldEventKind,
    /// 人类可读描述（"外面开始下雨了"/"今天是中秋节"）
    pub description: String,
    pub timestamp: i64,
    /// 事件显著度（0.0-1.0），决定是否值得打扰用户
    pub significance: f64,
}

impl WorldEvent {
    pub fn new(kind: WorldEventKind, description: impl Into<String>, significance: f64) -> Self {
        Self {
            kind,
            description: description.into(),
            timestamp: chrono::Utc::now().timestamp(),
            significance: significance.clamp(0.0, 1.0),
        }
    }

    /// 映射到 Appraisal（认知评估），再走固定心理学映射影响情绪/需求
    ///
    /// 设计原则：世界事件大多是中性偏正面的"新奇"信号，
    /// 极端天气/节日才提升 significance。
    pub fn to_appraisal(&self) -> Appraisal {
        let sig = self.significance;
        match self.kind {
            // 降水开始：略微紧张（出门不便）但新奇
            WorldEventKind::RainStarted => Appraisal {
                threat: 0.15,
                rejection: 0.0,
                control: 0.5,
                fairness: 0.5,
                novelty: 0.6,
                significance: sig,
            },
            // 天气变化：中性新奇
            WorldEventKind::WeatherChanged => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.5,
                fairness: 0.5,
                novelty: 0.4,
                significance: sig,
            },
            // 节日到来：正面（公平感/归属）
            WorldEventKind::FestivalArrived => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.7,
                fairness: 0.8,
                novelty: 0.5,
                significance: sig,
            },
            // 节气交替：轻微新奇
            WorldEventKind::SolarTermChanged => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.5,
                fairness: 0.6,
                novelty: 0.35,
                significance: sig,
            },
            // 日出：希望、好奇
            WorldEventKind::Sunrise => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.6,
                fairness: 0.7,
                novelty: 0.3,
                significance: sig,
            },
            // 日落：略带感伤、亲密
            WorldEventKind::Sunset => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.4,
                fairness: 0.5,
                novelty: 0.3,
                significance: sig,
            },
            // 季节变化：中性新奇
            WorldEventKind::SeasonChanged => Appraisal {
                threat: 0.0,
                rejection: 0.0,
                control: 0.5,
                fairness: 0.6,
                novelty: 0.5,
                significance: sig,
            },
            // 用户长时间未交互：被拒绝感、孤独
            WorldEventKind::LongAbsence => Appraisal {
                threat: 0.1,
                rejection: 0.4,
                control: 0.3,
                fairness: 0.4,
                novelty: 0.2,
                significance: sig,
            },
        }
    }
}

/// 事件检测器 —— 比较前后两个 WorldSnapshot，产出世界事件
pub struct WorldEventDetector {
    last_weather_code: Option<u32>,
    last_was_precipitating: Option<bool>,
    last_festival: Option<Festival>,
    last_solar_term: Option<SolarTerm>,
    last_is_daytime: Option<bool>,
    last_season: Option<crate::world::Season>,
}

impl WorldEventDetector {
    pub fn new() -> Self {
        Self {
            last_weather_code: None,
            last_was_precipitating: None,
            last_festival: None,
            last_solar_term: None,
            last_is_daytime: None,
            last_season: None,
        }
    }

    /// 检测事件，并更新内部状态
    pub fn detect(&mut self, snap: &crate::world::WorldSnapshot) -> Vec<WorldEvent> {
        let mut events = Vec::new();

        // 天气变化
        if let Some(w) = &snap.weather {
            if let Some(last_code) = self.last_weather_code {
                if last_code != w.weather_code {
                    let was_rain = self.last_was_precipitating.unwrap_or(false);
                    if w.is_precipitating && !was_rain {
                        events.push(WorldEvent::new(
                            WorldEventKind::RainStarted,
                            format!("外面开始{}了", w.description),
                            0.6,
                        ));
                    } else {
                        events.push(WorldEvent::new(
                            WorldEventKind::WeatherChanged,
                            format!("天气变成{}", w.description),
                            0.3,
                        ));
                    }
                }
            }
            self.last_weather_code = Some(w.weather_code);
            self.last_was_precipitating = Some(w.is_precipitating);
        }

        // 节日到来
        if snap.festival != self.last_festival {
            if let Some(f) = snap.festival {
                events.push(WorldEvent::new(
                    WorldEventKind::FestivalArrived,
                    format!("今天是{}", f.as_str()),
                    0.8,
                ));
            }
            self.last_festival = snap.festival;
        }

        // 节气交替
        if snap.solar_term != self.last_solar_term {
            if let Some(st) = snap.solar_term {
                if self.last_solar_term.is_some() {
                    events.push(WorldEvent::new(
                        WorldEventKind::SolarTermChanged,
                        format!("进入{}节气", st.as_str()),
                        0.4,
                    ));
                }
            }
            self.last_solar_term = snap.solar_term;
        }

        // 日出日落
        if let Some(ss) = snap.sunrise_sunset {
            let daytime = ss.is_daytime;
            if let Some(last) = self.last_is_daytime {
                if last != daytime {
                    if daytime {
                        events.push(WorldEvent::new(
                            WorldEventKind::Sunrise,
                            format!("天亮了，日出 {}", ss.sunrise_str()),
                            0.4,
                        ));
                    } else {
                        events.push(WorldEvent::new(
                            WorldEventKind::Sunset,
                            format!("天黑了，日落 {}", ss.sunset_str()),
                            0.5,
                        ));
                    }
                }
            }
            self.last_is_daytime = Some(daytime);
        }

        // 季节变化
        if Some(snap.season) != self.last_season {
            if self.last_season.is_some() {
                events.push(WorldEvent::new(
                    WorldEventKind::SeasonChanged,
                    format!("进入{}", snap.season.as_str()),
                    0.5,
                ));
            }
            self.last_season = Some(snap.season);
        }

        events
    }
}

impl Default for WorldEventDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// 根据"距上次交互时长"生成长时间缺席事件
pub fn long_absence_event(seconds: f64) -> Option<WorldEvent> {
    // 超过 6 小时算显著缺席
    if seconds > 6.0 * 3600.0 {
        let hours = seconds / 3600.0;
        let significance = ((hours - 6.0) / 18.0).clamp(0.0, 1.0) * 0.5 + 0.3;
        Some(WorldEvent::new(
            WorldEventKind::LongAbsence,
            format!("已经 {:.1} 小时没有和用户说话了", hours),
            significance,
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rain_event_appraisal() {
        let e = WorldEvent::new(WorldEventKind::RainStarted, "开始下雨", 0.6);
        let a = e.to_appraisal();
        assert!(a.threat > 0.0);
        assert!(a.novelty > 0.5);
    }

    #[test]
    fn test_long_absence_threshold() {
        assert!(long_absence_event(3600.0).is_none());
        assert!(long_absence_event(7.0 * 3600.0).is_some());
    }
}
