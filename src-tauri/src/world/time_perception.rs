//! 时间感知 —— 节气 / 节日 / 季节 / 日出日落（纯本地，无需网络）
//!
//! 节气：24 节气每年日期接近固定，用查表 + 年份小偏移近似（精度 ±1 天）。
//! 节日：公历固定 + 农历节日（春节/端午/中秋）用预置近 10 年农历表。
//! 日出日落：NOAA 简化算法（精度 ±5 分钟）。

use chrono::{Datelike, Local, Timelike, Weekday};
use serde::{Deserialize, Serialize};

use super::WorldConfig;

/// 季节
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub fn as_str(&self) -> &'static str {
        match self {
            Season::Spring => "春季",
            Season::Summer => "夏季",
            Season::Autumn => "秋季",
            Season::Winter => "冬季",
        }
    }

    pub fn from_month(month: u32) -> Self {
        match month {
            3 | 4 | 5 => Season::Spring,
            6 | 7 | 8 => Season::Summer,
            9 | 10 | 11 => Season::Autumn,
            _ => Season::Winter,
        }
    }
}

/// 24 节气
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolarTerm {
    MinorCold,        // 小寒
    MajorCold,        // 大寒
    BeginningOfSpring, // 立春
    RainWater,        // 雨水
    AwakeningOfInsects, // 惊蛰
    SpringEquinox,    // 春分
    PureBrightness,   // 清明
    GrainRain,        // 谷雨
    BeginningOfSummer, // 立夏
    GrainBuds,        // 小满
    GrainInEar,       // 芒种
    SummerSolstice,   // 夏至
    MinorHeat,        // 小暑
    MajorHeat,        // 大暑
    BeginningOfAutumn, // 立秋
    EndOfHeat,        // 处暑
    WhiteDew,         // 白露
    AutumnEquinox,    // 秋分
    ColdDew,          // 寒露
    FrostDescent,     // 霜降
    BeginningOfWinter, // 立冬
    MinorSnow,        // 小雪
    MajorSnow,        // 大雪
    WinterSolstice,   // 冬至
}

impl SolarTerm {
    pub fn as_str(&self) -> &'static str {
        match self {
            SolarTerm::MinorCold => "小寒",
            SolarTerm::MajorCold => "大寒",
            SolarTerm::BeginningOfSpring => "立春",
            SolarTerm::RainWater => "雨水",
            SolarTerm::AwakeningOfInsects => "惊蛰",
            SolarTerm::SpringEquinox => "春分",
            SolarTerm::PureBrightness => "清明",
            SolarTerm::GrainRain => "谷雨",
            SolarTerm::BeginningOfSummer => "立夏",
            SolarTerm::GrainBuds => "小满",
            SolarTerm::GrainInEar => "芒种",
            SolarTerm::SummerSolstice => "夏至",
            SolarTerm::MinorHeat => "小暑",
            SolarTerm::MajorHeat => "大暑",
            SolarTerm::BeginningOfAutumn => "立秋",
            SolarTerm::EndOfHeat => "处暑",
            SolarTerm::WhiteDew => "白露",
            SolarTerm::AutumnEquinox => "秋分",
            SolarTerm::ColdDew => "寒露",
            SolarTerm::FrostDescent => "霜降",
            SolarTerm::BeginningOfWinter => "立冬",
            SolarTerm::MinorSnow => "小雪",
            SolarTerm::MajorSnow => "大雪",
            SolarTerm::WinterSolstice => "冬至",
        }
    }
}

/// 节日
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Festival {
    // 公历
    NewYear,           // 元旦 1/1
    Valentine,         // 情人节 2/14
    Labour,            // 劳动节 5/1
    Children,          // 儿童节 6/1
    National,          // 国庆节 10/1
    Christmas,         // 圣诞 12/25
    NewYearEve,        // 除夕（农历）
    // 农历
    SpringFestival,    // 春节
    Lantern,           // 元宵
    DragonBoat,        // 端午
    MidAutumn,         // 中秋
    DoubleNinth,       // 重阳
}

impl Festival {
    pub fn as_str(&self) -> &'static str {
        match self {
            Festival::NewYear => "元旦",
            Festival::Valentine => "情人节",
            Festival::Labour => "劳动节",
            Festival::Children => "儿童节",
            Festival::National => "国庆节",
            Festival::Christmas => "圣诞节",
            Festival::NewYearEve => "除夕",
            Festival::SpringFestival => "春节",
            Festival::Lantern => "元宵节",
            Festival::DragonBoat => "端午节",
            Festival::MidAutumn => "中秋节",
            Festival::DoubleNinth => "重阳节",
        }
    }

    pub fn is_lunar(&self) -> bool {
        matches!(
            self,
            Festival::NewYearEve
                | Festival::SpringFestival
                | Festival::Lantern
                | Festival::DragonBoat
                | Festival::MidAutumn
                | Festival::DoubleNinth
        )
    }
}

/// 时间感知体
pub struct TimePerception {
    local_time: String,
    hour: u32,
    weekday: u32,
    is_weekend: bool,
    season: Season,
    solar_term: Option<SolarTerm>,
    festival: Option<Festival>,
}

impl TimePerception {
    pub fn at(now: &chrono::DateTime<Local>, _config: &WorldConfig) -> Self {
        let hour = now.hour();
        let weekday = now.weekday();
        let month = now.month();

        let is_weekend = matches!(weekday, Weekday::Sat | Weekday::Sun);
        let season = Season::from_month(month);
        let solar_term = current_solar_term(now);
        let festival = festival_on(now);

        let weekday_en = match weekday {
            Weekday::Mon => "Monday",
            Weekday::Tue => "Tuesday",
            Weekday::Wed => "Wednesday",
            Weekday::Thu => "Thursday",
            Weekday::Fri => "Friday",
            Weekday::Sat => "Saturday",
            Weekday::Sun => "Sunday",
        };
        Self {
            local_time: format!(
                "{} {} {}",
                now.format("%Y-%m-%d"),
                weekday_en,
                now.format("%H:%M")
            ),
            hour,
            weekday: weekday.num_days_from_monday(),
            is_weekend,
            season,
            solar_term,
            festival,
        }
    }

    pub fn local_time_str(&self) -> String {
        self.local_time.clone()
    }
    pub fn hour(&self) -> u32 {
        self.hour
    }
    pub fn weekday(&self) -> u32 {
        self.weekday
    }
    pub fn is_weekend(&self) -> bool {
        self.is_weekend
    }
    pub fn season(&self) -> Season {
        self.season
    }
    pub fn solar_term(&self) -> Option<SolarTerm> {
        self.solar_term
    }
    pub fn festival(&self) -> Option<Festival> {
        self.festival
    }
}

/// 24 节气近似日期（公历每月两个节气，日期接近固定）
///
/// 返回 (month, day, solar_term) 列表，按时间顺序。day 为近似值，
/// 实际误差通常 ±1 天，对"Vivian 感知当前节气"用途足够。
fn solar_term_table(year: i32) -> Vec<(u32, u32, SolarTerm)> {
    // 每年日期随年份有小幅波动，世纪年份略偏后
    let y = year as f64;
    let century_drift = ((y / 100.0).floor() - 19.0) * 0.05; // 世纪修正
    let year_drift = ((y - 2000.0) * 0.003).min(0.3); // 年份累积偏移

    // 基准（2000 年附近）日期表：(month, day_base, term)
    let base: [(u32, u32, SolarTerm); 24] = [
        (1, 6, SolarTerm::MinorCold),
        (1, 20, SolarTerm::MajorCold),
        (2, 4, SolarTerm::BeginningOfSpring),
        (2, 19, SolarTerm::RainWater),
        (3, 6, SolarTerm::AwakeningOfInsects),
        (3, 21, SolarTerm::SpringEquinox),
        (4, 5, SolarTerm::PureBrightness),
        (4, 20, SolarTerm::GrainRain),
        (5, 6, SolarTerm::BeginningOfSummer),
        (5, 21, SolarTerm::GrainBuds),
        (6, 6, SolarTerm::GrainInEar),
        (6, 21, SolarTerm::SummerSolstice),
        (7, 7, SolarTerm::MinorHeat),
        (7, 23, SolarTerm::MajorHeat),
        (8, 8, SolarTerm::BeginningOfAutumn),
        (8, 23, SolarTerm::EndOfHeat),
        (9, 8, SolarTerm::WhiteDew),
        (9, 23, SolarTerm::AutumnEquinox),
        (10, 8, SolarTerm::ColdDew),
        (10, 23, SolarTerm::FrostDescent),
        (11, 7, SolarTerm::BeginningOfWinter),
        (11, 22, SolarTerm::MinorSnow),
        (12, 7, SolarTerm::MajorSnow),
        (12, 22, SolarTerm::WinterSolstice),
    ];

    let drift = (century_drift + year_drift).round() as i32;
    base.iter()
        .map(|(m, d, t)| (*m, (*d as i32 + drift).max(1).min(28) as u32, *t))
        .collect()
}

fn current_solar_term(now: &chrono::DateTime<Local>) -> Option<SolarTerm> {
    let year = now.year();
    let month = now.month();
    let day = now.day();
    let table = solar_term_table(year);

    // 找到当前日期对应的节气区间（最近一个已过的节气）
    let mut current: Option<SolarTerm> = None;
    for (m, d, term) in &table {
        if (*m, *d) <= (month, day) {
            current = Some(*term);
        } else {
            break;
        }
    }
    // 若年初还没到第一个节气，用去年最后一个（冬至）
    if current.is_none() {
        current = Some(SolarTerm::WinterSolstice);
    }
    current
}

/// 农历关键节日查表（2024-2030，覆盖近期使用）
///
/// 返回 (year, month, day, festival) 公历日期
fn lunar_festival_dates() -> Vec<(i32, u32, u32, Festival)> {
    vec![
        // 2024
        (2024, 2, 9, Festival::NewYearEve),
        (2024, 2, 10, Festival::SpringFestival),
        (2024, 2, 24, Festival::Lantern),
        (2024, 6, 10, Festival::DragonBoat),
        (2024, 9, 17, Festival::MidAutumn),
        (2024, 10, 11, Festival::DoubleNinth),
        // 2025
        (2025, 1, 28, Festival::NewYearEve),
        (2025, 1, 29, Festival::SpringFestival),
        (2025, 2, 12, Festival::Lantern),
        (2025, 5, 31, Festival::DragonBoat),
        (2025, 10, 6, Festival::MidAutumn),
        (2025, 10, 29, Festival::DoubleNinth),
        // 2026
        (2026, 2, 16, Festival::NewYearEve),
        (2026, 2, 17, Festival::SpringFestival),
        (2026, 3, 3, Festival::Lantern),
        (2026, 6, 19, Festival::DragonBoat),
        (2026, 9, 25, Festival::MidAutumn),
        (2026, 10, 18, Festival::DoubleNinth),
        // 2027
        (2027, 2, 5, Festival::NewYearEve),
        (2027, 2, 6, Festival::SpringFestival),
        (2027, 3, 20, Festival::Lantern),
        (2027, 6, 9, Festival::DragonBoat),
        (2027, 9, 15, Festival::MidAutumn),
        (2027, 10, 8, Festival::DoubleNinth),
        // 2028
        (2028, 1, 25, Festival::NewYearEve),
        (2028, 1, 26, Festival::SpringFestival),
        (2028, 3, 9, Festival::Lantern),
        (2028, 5, 28, Festival::DragonBoat),
        (2028, 10, 3, Festival::MidAutumn),
        (2028, 10, 26, Festival::DoubleNinth),
        // 2029
        (2029, 2, 12, Festival::NewYearEve),
        (2029, 2, 13, Festival::SpringFestival),
        (2029, 2, 27, Festival::Lantern),
        (2029, 6, 16, Festival::DragonBoat),
        (2029, 9, 22, Festival::MidAutumn),
        (2029, 10, 15, Festival::DoubleNinth),
        // 2030
        (2030, 2, 2, Festival::NewYearEve),
        (2030, 2, 3, Festival::SpringFestival),
        (2030, 2, 17, Festival::Lantern),
        (2030, 6, 5, Festival::DragonBoat),
        (2030, 9, 12, Festival::MidAutumn),
        (2030, 10, 5, Festival::DoubleNinth),
    ]
}

fn festival_on(now: &chrono::DateTime<Local>) -> Option<Festival> {
    let year = now.year();
    let month = now.month();
    let day = now.day();

    // 公历固定节日
    let solar = match (month, day) {
        (1, 1) => Some(Festival::NewYear),
        (2, 14) => Some(Festival::Valentine),
        (5, 1) => Some(Festival::Labour),
        (6, 1) => Some(Festival::Children),
        (10, 1) => Some(Festival::National),
        (12, 25) => Some(Festival::Christmas),
        _ => None,
    };
    if solar.is_some() {
        return solar;
    }

    // 农历节日查表
    lunar_festival_dates()
        .into_iter()
        .find(|(y, m, d, _)| *y == year && *m == month && *d == day)
        .map(|(_, _, _, f)| f)
}

/// NOAA 简化日出日落算法
///
/// `lat` 纬度（度），`lon` 经度（度，东正西负），返回本地时区的日出日落小时（小数）。
/// 精度约 ±5 分钟，对"Vivian 知道天黑了"用途足够。
pub fn compute_sunrise_sunset(
    lat: f64,
    lon: f64,
    now: &chrono::DateTime<Local>,
) -> Option<super::SunriseSunset> {
    if lat.abs() > 85.0 {
        return None; // 极地地区算法失效
    }

    let n = now.ordinal() as f64;

    // 日出计算
    let sunrise = sunrise_time(lat, lon, n, now);
    let sunset = sunset_time(lat, lon, n, now);

    let (sunrise, sunset) = (sunrise?, sunset?);

    let current_hour = now.hour() as f64 + now.minute() as f64 / 60.0;
    let is_daytime = current_hour >= sunrise && current_hour < sunset;

    Some(super::SunriseSunset {
        sunrise_hour: sunrise,
        sunset_hour: sunset,
        is_daytime,
    })
}

fn sunrise_time(
    lat: f64,
    lon: f64,
    n: f64,
    now: &chrono::DateTime<Local>,
) -> Option<f64> {
    compute_event(lat, lon, n, now).map(|(rise, _)| rise)
}

fn sunset_time(
    lat: f64,
    lon: f64,
    n: f64,
    now: &chrono::DateTime<Local>,
) -> Option<f64> {
    compute_event(lat, lon, n, now).map(|(_, set)| set)
}

/// 简化 NOAA 日出日落计算，返回 (日出小时, 日落小时)（本地时区）
///
/// 以太阳中心高度 -0.833°（含大气折射与太阳视半径）为晨昏界限。
fn compute_event(
    lat: f64,
    lon: f64,
    n: f64,
    now: &chrono::DateTime<Local>,
) -> Option<(f64, f64)> {
    // 日角度（自春分起算）
    let b = 2.0 * std::f64::consts::PI * (n - 81.0) / 364.0;
    // 时差（equation of time，分钟）
    let eot = 9.87 * (2.0 * b).sin() - 7.53 * b.cos() - 1.5 * b.sin();
    // 太阳赤纬（度）
    let decl = 23.45 * b.sin();
    let lat_rad = lat.to_radians();
    let decl_rad = decl.to_radians();
    // 半日长：太阳高度 -0.833° 时的时角（小时）
    let cos_h = ((-0.833_f64).to_radians().sin()
        - lat_rad.sin() * decl_rad.sin())
        / (lat_rad.cos() * decl_rad.cos());
    if !cos_h.is_finite() {
        return None;
    }
    let h = cos_h.clamp(-1.0, 1.0).acos().to_degrees() / 15.0;
    // 平太阳正午（本地时区小时）
    let noon = 12.0 - lon / 15.0 + timezone_offset(now);
    let eot_h = eot / 60.0;
    Some((
        (noon - h + eot_h).clamp(0.0, 24.0),
        (noon + h + eot_h).clamp(0.0, 24.0),
    ))
}

fn timezone_offset(now: &chrono::DateTime<Local>) -> f64 {
    // 本地时间 - UTC 时间的小时偏移
    let diff = now.naive_local().and_utc() - now.naive_utc().and_utc();
    diff.num_seconds() as f64 / 3600.0
}
