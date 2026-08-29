//! 天气预报工具 - 基于 Open-Meteo 免费 API 提供未来 N 天天气预报
//!
//! 与 `world/weather.rs`（世界感知层，被动注入 prompt）不同，
//! 本工具是 LLM **主动调用**的工具：用户问"明天天气怎么样"、"这周会下雨吗"时触发。
//!
//! 数据源：Open-Meteo Forecast API（https://open-meteo.com/en/docs）
//! - 免费，无需 API Key
//! - 支持最多 16 天预报
//! - 返回日级聚合数据（最高/最低温度、降水量、降水概率、风速）

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::config::AppConfig;
use crate::network::http_client::get_global_client;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::world::weather::weather_code_to_desc;

/// 全局 AppHandle（由 lib.rs setup 注入，用于读取 AppState 中的 WorldConfig）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 读取当前经纬度配置（AppHandle 未注入或未配置时返回 None）
fn read_lat_lon() -> Option<(f64, f64)> {
    APP_HANDLE.read().clone().and_then(|handle| {
        let config: AppConfig = handle
            .state::<Arc<AppState>>()
            .config
            .read()
            .get_all();
        match (config.world.latitude, config.world.longitude) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        }
    })
}

// ============================================================================
// Open-Meteo Forecast API 响应结构
// ============================================================================

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    /// 请求纬度，Open-Meteo 回显但本工具未读取（使用配置中的经纬度）
    latitude: f64,
    /// 请求经度，Open-Meteo 回显但本工具未读取
    longitude: f64,
    timezone: Option<String>,
    current: Option<CurrentData>,
    daily: Option<DailyData>,
}

#[derive(Debug, Deserialize)]
struct CurrentData {
    temperature_2m: f64,
    relative_humidity_2m: f64,
    apparent_temperature: f64,
    weather_code: u32,
    wind_speed_10m: f64,
    /// 是否白天（1=白天，0=夜晚），Open-Meteo 返回但本工具未读取
    is_day: u8,
}

#[derive(Debug, Deserialize)]
struct DailyData {
    time: Vec<String>,
    weather_code: Vec<u32>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_sum: Vec<f64>,
    precipitation_probability_max: Vec<Option<u32>>,
    wind_speed_10m_max: Vec<f64>,
}

// ============================================================================
// GetWeatherForecastTool
// ============================================================================

/// 天气预报工具 - 获取未来 N 天的天气预报
///
/// 用户问"明天天气怎么样"、"这周会不会下雨"、"未来几天温度"时调用。
/// 返回当前实况 + 每日预报（温度范围、降水量、降水概率、最大风速）。
pub struct GetWeatherForecastTool;

impl GetWeatherForecastTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetWeatherForecastTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetWeatherForecastTool {
    fn name(&self) -> &str {
        "get_weather_forecast"
    }

    fn description(&self) -> &str {
        "Get the weather forecast for the next few days (based on the free Open-Meteo API).\n\
         Returns: current conditions (temperature, feels-like, weather description, humidity, wind speed)\
         and daily forecast (weather description, high/low temperature, precipitation, precipitation probability, max wind speed).\n\
         \n\
         Typical use cases:\n\
         - User asks \"what's the weather tomorrow\" -> pass days=2, take the second day's data\n\
         - User asks \"will it rain this week\" -> pass days=7, check daily precipitation probability\n\
         - User asks \"temperature for the next three days\" -> pass days=3\n\
         \n\
         Note: requires the user to have configured latitude/longitude (world.latitude / world.longitude) in settings,\
         otherwise an error is returned prompting the user to configure location info."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取未来几天的天气预报（基于免费的 Open-Meteo API）。\n\
         返回：当前实况（温度、体感温度、天气描述、湿度、风速）和每日预报（天气描述、最高/最低温度、降水量、降水概率、最大风速）。\n\
         \n\
         典型用例：\n\
         - 用户问\"明天天气怎么样\" -> 传 days=2，取第二天的数据\n\
         - 用户问\"这周会不会下雨\" -> 传 days=7，查看每日降水概率\n\
         - 用户问\"未来三天温度\" -> 传 days=3\n\
         \n\
         注意：需要用户在设置中已配置经纬度（world.latitude / world.longitude），\
         否则会返回错误提示用户配置位置信息。",
            "ja" => "今後数日間の天気予報を取得する（無料の Open-Meteo API に基づく）。\n\
         戻り値：現在の実況（温度、体感温度、天気説明、湿度、風速）と日別予報（天気説明、最高/最低温度、降水量、降水確率、最大風速）。\n\
         \n\
         典型的なユースケース：\n\
         - ユーザーが「明日の天気は？」と聞いた場合 -> days=2 を渡して2日目のデータを取得\n\
         - ユーザーが「今週雨降る？」と聞いた場合 -> days=7 を渡して日別降水確率を確認\n\
         - ユーザーが「今後3日間の温度」と聞いた場合 -> days=3 を渡す\n\
         \n\
         注意：ユーザーが設定で経緯度（world.latitude / world.longitude）を設定している必要がある。\
         そうでない場合は位置情報の設定を促すエラーが返される。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "days": {
                    "type": "integer",
                    "description": "Number of forecast days, 1-16, default 7. Includes today.",
                    "minimum": 1,
                    "maximum": 16
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "days": {
                        "type": "integer",
                        "description": "预报天数，1-16，默认 7。包含今天。",
                        "minimum": 1,
                        "maximum": 16
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "days": {
                        "type": "integer",
                        "description": "予報日数、1-16、デフォルト 7。今日を含む。",
                        "minimum": 1,
                        "maximum": 16
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let mut data = input.clone();
        if data.get("days").is_none() {
            data["days"] = json!(7);
        }
        let days = data["days"].as_u64().unwrap_or(7);
        if days < 1 || days > 16 {
            return ValidationResult::failure("days 必须在 1~16 之间", 2);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(7) as u32;

        // 1. 读取经纬度
        let (lat, lon) = match read_lat_lon() {
            Some(v) => v,
            None => {
                return ToolResult::standard_error(
                    "未配置经纬度。请在设置中配置 world.latitude 和 world.longitude，\
                     或启用自动定位（world.enable_geolocation）。",
                    Some("LocationNotConfigured"),
                    None,
                );
            }
        };

        // 2. 调用 Open-Meteo Forecast API
        let client = get_global_client();
        let url = format!(
            "https://api.open-meteo.com/v1/forecast\
             ?latitude={lat}&longitude={lon}\
             &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m\
             &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum,precipitation_probability_max,wind_speed_10m_max\
             &timezone=auto\
             &forecast_days={days}"
        );

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("天气预报请求失败: {e}"),
                    Some("NetworkError"),
                    Some(json!({"url": url})),
                );
            }
        };

        if let Err(e) = resp.error_for_status_ref() {
            return ToolResult::standard_error(
                &format!("天气预报响应状态错误: {e}"),
                Some("HttpError"),
                Some(json!({"status": resp.status().as_u16()})),
            );
        }

        let body: ForecastResponse = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("天气预报 JSON 解析失败: {e}"),
                    Some("ParseError"),
                    None,
                );
            }
        };

        tracing::debug!(
            latitude = body.latitude,
            longitude = body.longitude,
            timezone = ?body.timezone,
            "[WeatherTool] Open-Meteo 响应回显位置信息"
        );

        // 3. 构建当前实况
        let current_info = body.current.as_ref().map(|c| {
            tracing::debug!(
                is_day = c.is_day,
                "[WeatherTool] 当前 is_day 标记"
            );
            json!({
                "temperature": c.temperature_2m,
                "feels_like": c.apparent_temperature,
                "weather_code": c.weather_code,
                "description": weather_code_to_desc(c.weather_code),
                "humidity": c.relative_humidity_2m,
                "wind_speed": c.wind_speed_10m,
            })
        });

        // 4. 构建每日预报
        let daily_forecasts: Vec<Value> = match &body.daily {
            Some(d) => {
                let count = d.time.len();
                (0..count)
                    .map(|i| {
                        let code = d.weather_code.get(i).copied().unwrap_or(0);
                        let precip_prob = d.precipitation_probability_max.get(i).and_then(|v| *v);
                        json!({
                            "date": d.time[i],
                            "weather_code": code,
                            "description": weather_code_to_desc(code),
                            "temp_max": d.temperature_2m_max.get(i).copied().unwrap_or(0.0),
                            "temp_min": d.temperature_2m_min.get(i).copied().unwrap_or(0.0),
                            "precipitation_mm": d.precipitation_sum.get(i).copied().unwrap_or(0.0),
                            "precipitation_probability_pct": precip_prob,
                            "wind_speed_max": d.wind_speed_10m_max.get(i).copied().unwrap_or(0.0),
                        })
                    })
                    .collect()
            }
            None => vec![],
        };

        // 5. 生成摘要
        let summary = generate_forecast_summary(&daily_forecasts);

        ToolResult::standard_success(
            &summary,
            Some(json!({
                "current": current_info,
                "daily_forecast": daily_forecasts,
                "days_requested": days,
                "days_returned": daily_forecasts.len(),
                "timezone": body.timezone,
                "source": "Open-Meteo",
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Network
    }

    // 延迟加载：天气预报是长尾需求，通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "weather forecast"
    }
}

/// 根据预报数据生成一段自然语言摘要，方便 LLM 快速理解趋势
fn generate_forecast_summary(forecasts: &[Value]) -> String {
    if forecasts.is_empty() {
        return "无预报数据".to_string();
    }

    let mut parts = Vec::new();

    // 找最高温日
    let hottest = forecasts
        .iter()
        .max_by(|a, b| {
            let ta = a["temp_max"].as_f64().unwrap_or(0.0);
            let tb = b["temp_max"].as_f64().unwrap_or(0.0);
            ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|f| {
            let date = f["date"].as_str()?;
            let temp = f["temp_max"].as_f64()?;
            Some(format!("{date} 最热 {temp}°C"))
        });

    // 找最冷日
    let coldest = forecasts
        .iter()
        .min_by(|a, b| {
            let ta = a["temp_min"].as_f64().unwrap_or(0.0);
            let tb = b["temp_min"].as_f64().unwrap_or(0.0);
            ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|f| {
            let date = f["date"].as_str()?;
            let temp = f["temp_min"].as_f64()?;
            Some(format!("{date} 最冷 {temp}°C"))
        });

    // 找降水日
    let rainy_days: Vec<String> = forecasts
        .iter()
        .filter(|f| {
            let prob = f["precipitation_probability_pct"].as_u64().unwrap_or(0);
            let precip = f["precipitation_mm"].as_f64().unwrap_or(0.0);
            prob > 50 || precip > 1.0
        })
        .filter_map(|f| {
            let date = f["date"].as_str()?;
            let desc = f["description"].as_str().unwrap_or("");
            Some(format!("{date}({desc})"))
        })
        .collect();

    if let Some(h) = hottest {
        parts.push(h);
    }
    if let Some(c) = coldest {
        parts.push(c);
    }
    if rainy_days.is_empty() {
        parts.push("无明显降水".to_string());
    } else {
        parts.push(format!("预计降水日: {}", rainy_days.join("、")));
    }

    parts.join("，")
}
