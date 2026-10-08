//! 按需天气工具：Open-Meteo 与可选 Apple WeatherKit 多来源预报、逐小时天气和空气质量。
//! 常驻摘要由 world/weather.rs 提供，完整数组只在调用工具时返回。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// 全局 AppHandle（由 lib.rs setup 注入，用于读取 AppState 中的 WorldConfig）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 读取当前经纬度配置（AppHandle 未注入或未配置时返回 None）
fn read_world_config() -> Option<crate::config::WorldConfig> {
    APP_HANDLE.read().clone().map(|handle| {
        handle
            .state::<Arc<AppState>>()
            .config
            .read()
            .get_all()
            .world
    })
}

/// 天气预报工具 - 获取未来 N 天的天气预报
///
/// 用户问"明天天气怎么样"、"这周会不会下雨"、"未来几天温度"时调用。
/// 返回各来源当前实况、逐小时与每日预报及独立空气质量数据。
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
        "Get current weather, hourly forecasts and daily forecasts from Open-Meteo and configured Apple WeatherKit, plus US AQI / PM2.5 / PM10. Compare sources independently with matching timestamps and coordinates; never average or invent missing values. Includes source availability, units and attribution. Cite the source and attribution_url when presenting provider data. Apple timestamps include UTC offsets; its timezone is the daily aggregation timezone. Apple supports up to 10 daily forecast days. Requires configured location."
    }

    fn description_in(&self, _lang: &str) -> &str {
        self.description()
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "天气预报\n十日天气\n每小时天气\n气压\n空气质量AQI\nApple天气\n比对天气来源\n最高最低气温\n明天会下雨吗",
            "ja" => "天気予報\n時間ごとの天気\n空気質\nApple天気",
            _ => "weather forecast\nhourly weather\n10 day forecast\nair quality AQI\ncompare Apple weather",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type":"object", "properties": {
            "days": {"type":"integer", "minimum":1, "maximum":16, "default":10, "description":"Daily forecast days including today. Apple provides at most 10."},
            "hours": {"type":"integer", "minimum":1, "maximum":240, "default":24, "description":"Forecast hours starting at the current hour"},
            "source": {"type":"string", "enum":["all", "open_meteo", "apple"], "default":"all", "description":"Use all to compare available providers"}
        }})
    }
    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match forecast_args(input) {
            Ok((days, hours, source)) => {
                ValidationResult::success(Some(json!({"days":days,"hours":hours,"source":source})))
            }
            Err(e) => ValidationResult::failure(e, 2),
        }
    }
    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let (days, hours, source) = match forecast_args(&args) {
            Ok(v) => v,
            Err(e) => return ToolResult::standard_error(e, Some("InvalidWeatherArguments"), None),
        };
        let config = match read_world_config() {
            Some(c) => c,
            None => {
                return ToolResult::standard_error(
                    "天气配置不可用",
                    Some("WeatherConfigUnavailable"),
                    None,
                )
            }
        };
        let (lat, lon) = match (config.latitude, config.longitude) {
            (Some(lat), Some(lon)) => (lat, lon),
            _ => {
                return ToolResult::standard_error(
                    "请在真实感知设置中配置经纬度或自动定位",
                    Some("LocationNotConfigured"),
                    None,
                )
            }
        };
        let reports =
            crate::world::weather_data::fetch_reports_for(lat, lon, &config, days, hours, source)
                .await;
        let summary = reports
            .providers
            .iter()
            .map(|p| {
                let c = &p.current;
                format!(
                    "{}: {} {}°C; {} daily forecasts, {} hourly forecasts",
                    p.source,
                    c.description.as_deref().unwrap_or("unknown"),
                    c.temperature
                        .map(|n| format!("{n:.1}"))
                        .unwrap_or_else(|| "unknown".into()),
                    p.daily.len(),
                    p.hourly.len()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let data = serde_json::to_value(&reports).ok();
        if reports.providers.is_empty() {
            return ToolResult::standard_error(
                "所选天气来源不可用；检查 source_status 和 Apple 凭据",
                Some("WeatherUnavailable"),
                data,
            );
        }
        ToolResult::standard_success(&summary, data)
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

/// Validate direct calls too; fractional and negative counts must never silently become defaults.
fn forecast_args(input: &Value) -> Result<(usize, usize, &str), &'static str> {
    let days = match input.get("days") {
        None => 10,
        Some(v) => v.as_u64().ok_or("days 必须是整数")?,
    };
    let hours = match input.get("hours") {
        None => 24,
        Some(v) => v.as_u64().ok_or("hours 必须是整数")?,
    };
    if !(1..=16).contains(&days) || !(1..=240).contains(&hours) {
        return Err("days 必须在 1–16，hours 必须在 1–240");
    }
    let source = match input.get("source") {
        None => "all",
        Some(v) => v.as_str().ok_or("source 必须是字符串")?,
    };
    if !matches!(source, "all" | "open_meteo" | "apple") {
        return Err("source 必须为 all、open_meteo 或 apple");
    }
    Ok((days as usize, hours as usize, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weather_forecast_arguments_reject_invalid_counts_and_sources() {
        assert_eq!(forecast_args(&json!({})).unwrap(), (10, 24, "all"));
        for args in [
            json!({"days":0}),
            json!({"days":17}),
            json!({"days":1.5}),
            json!({"hours":-1}),
            json!({"hours":241}),
            json!({"source":"unknown"}),
        ] {
            assert!(forecast_args(&args).is_err());
        }
        assert_eq!(
            forecast_args(&json!({"days":10,"hours":240,"source":"apple"})).unwrap(),
            (10, 240, "apple")
        );
    }
}
