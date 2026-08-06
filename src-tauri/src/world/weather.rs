//! 天气数据源 —— Open-Meteo 免费 API（无需 key）
//!
//! 失败即"不知道"：网络错误/超时/解析失败均返回 Err，由调用方保留旧缓存或 None，
//! 不做任何时间推断兜底（用户明确要求）。
//!
//! Open-Meteo 文档：https://open-meteo.com/en/docs

use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::network::http_client::get_global_client;

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
    /// 数据来源（如 "Open-Meteo"），供前端展示
    pub weather_source: String,
    /// 缓存时间戳（UTC 秒）
    pub cached_at: i64,
}

/// 天气数据源
pub struct WeatherSource {
    endpoint: String,
}

impl WeatherSource {
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.open-meteo.com/v1/forecast".to_string(),
        }
    }

    /// 获取指定经纬度的天气
    ///
    /// 失败返回 Err，由调用方决定是否保留旧缓存。
    pub async fn fetch(&self, lat: f64, lon: f64) -> VivianResult<WeatherSnapshot> {
        tracing::info!("[WeatherSource] 开始获取天气，经纬度: ({}, {})", lat, lon);
        let client = get_global_client();

        let url = format!(
            "{}?latitude={lat}&longitude={lon}&current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m&timezone=auto",
            self.endpoint
        );
        tracing::debug!("[WeatherSource] 请求 URL: {}", url);

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!("[WeatherSource] 天气请求失败: {}", e);
                VivianError::Network(format!("天气请求失败: {e}"))
            })?
            .error_for_status()
            .map_err(|e| {
                tracing::warn!("[WeatherSource] 天气响应状态错误: {}", e);
                VivianError::Network(format!("天气响应状态错误: {e}"))
            })?;
        tracing::info!("[WeatherSource] 天气请求成功，状态码: {}", resp.status());

        let body: OpenMeteoResponse = resp
            .json()
            .await
            .map_err(|e| {
                tracing::warn!("[WeatherSource] 天气 JSON 解析失败: {}", e);
                VivianError::Network(format!("天气 JSON 解析失败: {e}"))
            })?;

        let current = body.current.ok_or_else(|| {
            tracing::warn!("[WeatherSource] 天气响应缺少 current 字段");
            VivianError::Network("天气响应缺少 current 字段".to_string())
        })?;

        let weather_code = current.weather_code;
        let description = weather_code_to_desc(weather_code);
        let is_precipitating = is_precipitating(weather_code);

        tracing::debug!(
            is_day = current.is_day,
            "[WeatherSource] Open-Meteo is_day 标记"
        );

        tracing::info!(
            "[WeatherSource] 天气获取成功: {}°C, 体感 {}°C, {}, 湿度 {}%, 风速 {}km/h, 描述: {}",
            current.temperature_2m,
            current.apparent_temperature,
            weather_code,
            current.relative_humidity_2m,
            current.wind_speed_10m,
            description
        );

        Ok(WeatherSnapshot {
            temperature: current.temperature_2m,
            feels_like: current.apparent_temperature,
            weather_code,
            description,
            is_precipitating,
            wind_speed: current.wind_speed_10m,
            humidity: current.relative_humidity_2m,
            weather_source: "Open-Meteo".to_string(),
            cached_at: chrono::Utc::now().timestamp(),
        })
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

#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    current: Option<CurrentWeather>,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    temperature_2m: f64,
    relative_humidity_2m: f64,
    apparent_temperature: f64,
    /// 是否白天（1=白天，0=夜晚），Open-Meteo 返回但本模块未读取
    is_day: u8,
    weather_code: u32,
    wind_speed_10m: f64,
}
