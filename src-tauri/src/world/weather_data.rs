//! Shared multi-provider weather queries. Detailed forecasts stay out of world snapshots.
use super::weather::weather_code_to_desc;
use crate::config::{manager::AppleWeatherConfig, WorldConfig};
use crate::network::http_client::get_global_client;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::DecodePrivateKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conditions {
    pub time: Option<String>,
    pub temperature: Option<f64>,
    pub feels_like: Option<f64>,
    pub description: Option<String>,
    pub weather_code: Option<u32>,
    pub condition_code: Option<String>,
    pub humidity_pct: Option<f64>,
    pub pressure_hpa: Option<f64>,
    pub wind_speed_kmh: Option<f64>,
    pub is_day: Option<bool>,
    pub precipitation_probability_pct: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayForecast {
    pub date: String,
    pub description: Option<String>,
    pub temp_max: Option<f64>,
    pub temp_min: Option<f64>,
    pub precipitation_probability_pct: Option<f64>,
    pub precipitation_mm: Option<f64>,
    pub wind_speed_max_kmh: Option<f64>,
    pub sunrise: Option<String>,
    pub sunset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderForecast {
    pub source: String,
    pub timezone: String,
    pub fetched_at: i64,
    pub attribution_url: String,
    pub current: Conditions,
    pub daily: Vec<DayForecast>,
    pub hourly: Vec<Conditions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirQuality {
    pub source: String,
    pub standard: String,
    pub time: Option<String>,
    pub aqi: Option<f64>,
    pub pm2_5_ug_m3: Option<f64>,
    pub pm10_ug_m3: Option<f64>,
    pub fetched_at: i64,
    pub attribution_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherReports {
    pub latitude: f64,
    pub longitude: f64,
    pub providers: Vec<ProviderForecast>,
    pub air_quality: Option<AirQuality>,
    pub source_status: Vec<SourceStatus>,
    pub comparison_note: &'static str,
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key)?.as_f64().filter(|n| n.is_finite())
}
fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_owned)
}
fn array_row(v: &Value, index: usize) -> Value {
    let mut row = serde_json::Map::new();
    if let Some(obj) = v.as_object() {
        for (k, values) in obj {
            row.insert(k.clone(), values.get(index).cloned().unwrap_or(Value::Null));
        }
    }
    Value::Object(row)
}
fn meteo_conditions(v: &Value) -> Conditions {
    let code = v
        .get("weather_code")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    Conditions {
        time: string(v, "time"),
        temperature: num(v, "temperature_2m"),
        feels_like: num(v, "apparent_temperature"),
        description: code.map(weather_code_to_desc),
        weather_code: code,
        condition_code: None,
        humidity_pct: num(v, "relative_humidity_2m"),
        pressure_hpa: num(v, "pressure_msl"),
        wind_speed_kmh: num(v, "wind_speed_10m"),
        is_day: v.get("is_day").and_then(Value::as_u64).map(|n| n == 1),
        precipitation_probability_pct: num(v, "precipitation_probability"),
    }
}
fn parse_meteo(v: &Value, days: usize, hours: usize) -> Result<ProviderForecast, String> {
    let current = meteo_conditions(&v["current"]);
    if current.temperature.is_none() {
        return Err("Open-Meteo current temperature missing".into());
    }
    let daily = v["daily"]["time"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .take(days)
        .map(|(i, date)| {
            let r = array_row(&v["daily"], i);
            DayForecast {
                date: date.as_str().unwrap_or_default().into(),
                description: meteo_conditions(&r).description,
                temp_max: num(&r, "temperature_2m_max"),
                temp_min: num(&r, "temperature_2m_min"),
                precipitation_probability_pct: num(&r, "precipitation_probability_max"),
                precipitation_mm: num(&r, "precipitation_sum"),
                wind_speed_max_kmh: num(&r, "wind_speed_10m_max"),
                sunrise: string(&r, "sunrise"),
                sunset: string(&r, "sunset"),
            }
        })
        .collect();
    let now = current.time.as_deref().unwrap_or("");
    let hourly = v["hourly"]["time"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, t)| {
            t.as_str()
                .and_then(|s| s.get(..13))
                .zip(now.get(..13))
                .is_some_and(|(time, current)| time >= current)
        })
        .take(hours)
        .map(|(i, _)| meteo_conditions(&array_row(&v["hourly"], i)))
        .collect();
    Ok(ProviderForecast {
        source: "Open-Meteo".into(),
        timezone: string(v, "timezone").unwrap_or_default(),
        fetched_at: chrono::Utc::now().timestamp(),
        attribution_url: "https://open-meteo.com/".into(),
        current,
        daily,
        hourly,
    })
}

fn apple_description(code: &str) -> &str {
    match code {
        "Clear" | "MostlyClear" => "晴",
        "PartlyCloudy" => "局部多云",
        "MostlyCloudy" => "多云",
        "Cloudy" => "阴",
        "Foggy" => "雾",
        "Haze" => "霾",
        "Drizzle" => "毛毛雨",
        "Rain" => "雨",
        "HeavyRain" => "大雨",
        "Snow" => "雪",
        "HeavySnow" => "大雪",
        "Flurries" => "阵雪",
        "Sleet" => "雨夹雪",
        "FreezingRain" => "冻雨",
        "FreezingDrizzle" => "冻毛毛雨",
        "Hail" => "冰雹",
        "Thunderstorms" | "ScatteredThunderstorms" | "StrongStorms" | "IsolatedThunderstorms" => {
            "雷雨"
        }
        "Windy" | "Breezy" => "有风",
        "Blizzard" => "暴风雪",
        "Hot" => "炎热",
        "Frigid" => "严寒",
        _ => code,
    }
}
fn apple_conditions(v: &Value, hourly: bool) -> Conditions {
    Conditions {
        time: string(v, if hourly { "forecastStart" } else { "asOf" }),
        temperature: num(v, "temperature"),
        feels_like: num(v, "temperatureApparent"),
        description: v["conditionCode"]
            .as_str()
            .map(|s| apple_description(s).to_owned()),
        condition_code: string(v, "conditionCode"),
        humidity_pct: num(v, "humidity").map(|n| n * 100.0),
        pressure_hpa: num(v, "pressure"),
        wind_speed_kmh: num(v, "windSpeed"),
        is_day: v.get("daylight").and_then(Value::as_bool),
        precipitation_probability_pct: num(v, "precipitationChance").map(|n| n * 100.0),
        weather_code: None,
    }
}
fn parse_apple(v: &Value, days: usize, hours: usize) -> Result<ProviderForecast, String> {
    let current = apple_conditions(&v["currentWeather"], false);
    if current.temperature.is_none() {
        return Err("Apple Weather current temperature missing".into());
    }
    let daily = v["forecastDaily"]["days"]
        .as_array()
        .into_iter()
        .flatten()
        .take(days)
        .map(|r| DayForecast {
            date: string(r, "forecastStart").unwrap_or_default(),
            description: r["conditionCode"]
                .as_str()
                .map(|s| apple_description(s).to_owned()),
            temp_max: num(r, "temperatureMax"),
            temp_min: num(r, "temperatureMin"),
            precipitation_probability_pct: num(r, "precipitationChance").map(|n| n * 100.0),
            precipitation_mm: num(r, "precipitationAmount"),
            wind_speed_max_kmh: num(r, "windSpeedMax"),
            sunrise: string(r, "sunrise"),
            sunset: string(r, "sunset"),
        })
        .collect();
    let hourly = v["forecastHourly"]["hours"]
        .as_array()
        .into_iter()
        .flatten()
        .take(hours)
        .map(|r| apple_conditions(r, true))
        .collect();
    Ok(ProviderForecast {
        source: "Apple Weather".into(),
        timezone: "UTC".into(),
        fetched_at: chrono::Utc::now().timestamp(),
        attribution_url: "https://weatherkit.apple.com/legal-attribution.html".into(),
        current,
        daily,
        hourly,
    })
}

fn apple_token(c: &AppleWeatherConfig) -> Result<String, String> {
    if c.team_id.trim().is_empty()
        || c.service_id.trim().is_empty()
        || c.key_id.trim().is_empty()
        || c.api_secret.trim().is_empty()
    {
        return Err("WeatherKit requires Team ID, Service ID, Key ID and a .p8 private key".into());
    }
    let key = SigningKey::from_pkcs8_pem(&c.api_secret.replace("\\n", "\n"))
        .map_err(|_| "Invalid WeatherKit P-256 PKCS#8 private key".to_string())?;
    let now = chrono::Utc::now().timestamp();
    let header = json!({"alg":"ES256", "kid":c.key_id.trim(), "id":format!("{}.{}", c.team_id.trim(), c.service_id.trim())});
    let claims =
        json!({"iss":c.team_id.trim(), "sub":c.service_id.trim(), "iat":now, "exp":now+3600});
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature: Signature = key.sign(message.as_bytes());
    Ok(format!(
        "{}.{}",
        message,
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

async fn request(
    url: &str,
    params: &[(&str, String)],
    token: Option<String>,
) -> Result<Value, String> {
    let mut req = get_global_client()
        .get(url)
        .query(params)
        .timeout(std::time::Duration::from_secs(15));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .await
        .map_err(|_| "Weather request failed or timed out".to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Weather API HTTP {}", resp.status().as_u16()));
    }
    resp.json()
        .await
        .map_err(|_| "Invalid weather JSON response".into())
}

async fn meteo(lat: f64, lon: f64, days: usize, hours: usize) -> Result<ProviderForecast, String> {
    let v = request("https://api.open-meteo.com/v1/forecast", &[
        ("latitude", lat.to_string()), ("longitude", lon.to_string()), ("timezone", "auto".into()), ("forecast_days", days.max(hours.div_ceil(24) + 1).min(16).to_string()),
        ("current", "temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m,pressure_msl".into()),
        ("hourly", "temperature_2m,weather_code,precipitation_probability".into()),
        ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum,precipitation_probability_max,wind_speed_10m_max,sunrise,sunset".into()),
    ], None).await?;
    parse_meteo(&v, days, hours)
}
async fn apple(
    lat: f64,
    lon: f64,
    c: &AppleWeatherConfig,
    days: usize,
    hours: usize,
    timezone: &str,
) -> Result<ProviderForecast, String> {
    let token = apple_token(c)?;
    let now = chrono::Utc::now();
    let hour_start = chrono::DateTime::from_timestamp(now.timestamp().div_euclid(3600) * 3600, 0)
        .ok_or_else(|| "Invalid forecast start time".to_string())?;
    let mut params = vec![
        (
            "dataSets",
            "currentWeather,forecastDaily,forecastHourly".into(),
        ),
        ("timezone", timezone.into()),
        (
            "hourlyEnd",
            (hour_start + chrono::Duration::hours(hours as i64))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ),
    ];
    // Keep Apple's default ten-day boundary rather than requesting beyond its horizon.
    if days < 10 {
        params.push((
            "dailyEnd",
            (now + chrono::Duration::days(days as i64))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ));
    }
    let v = request(
        &format!("https://weatherkit.apple.com/api/v1/weather/zh/{lat}/{lon}"),
        &params,
        Some(token),
    )
    .await?;
    let mut forecast = parse_apple(&v, days.min(10), hours)?;
    forecast.timezone = timezone.into();
    Ok(forecast)
}
async fn air(lat: f64, lon: f64) -> Result<AirQuality, String> {
    let v = request(
        "https://air-quality-api.open-meteo.com/v1/air-quality",
        &[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("timezone", "GMT".into()),
            ("current", "us_aqi,pm2_5,pm10".into()),
        ],
        None,
    )
    .await?;
    let aqi = num(&v["current"], "us_aqi");
    if aqi.is_none() {
        return Err("Air quality index unavailable".into());
    }
    Ok(AirQuality {
        source: "Open-Meteo / CAMS (model estimate)".into(),
        standard: "US AQI".into(),
        time: string(&v["current"], "time").map(|s| format!("{s}Z")),
        aqi,
        pm2_5_ug_m3: num(&v["current"], "pm2_5"),
        pm10_ug_m3: num(&v["current"], "pm10"),
        fetched_at: chrono::Utc::now().timestamp(),
        attribution_url: "https://open-meteo.com/en/docs/air-quality-api".into(),
    })
}

pub async fn fetch_reports(
    lat: f64,
    lon: f64,
    config: &WorldConfig,
    days: usize,
    hours: usize,
) -> WeatherReports {
    fetch_reports_for(lat, lon, config, days, hours, "all").await
}

pub async fn fetch_reports_for(
    lat: f64,
    lon: f64,
    config: &WorldConfig,
    days: usize,
    hours: usize,
    selection: &str,
) -> WeatherReports {
    let apple_enabled = config.apple_weather.enabled;
    if !lat.is_finite()
        || !lon.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return assemble_reports(
            lat,
            lon,
            apple_enabled,
            Err("Invalid latitude/longitude".into()),
            Err("Invalid latitude/longitude".into()),
            Err("Invalid latitude/longitude".into()),
        );
    }
    let days = days.clamp(1, 16);
    let hours = hours.clamp(1, 240);
    let (m, q) = tokio::join!(
        async {
            if selection != "apple" {
                meteo(lat, lon, days, hours).await
            } else {
                Err("not_requested".into())
            }
        },
        air(lat, lon)
    );
    let timezone = if !config.apple_weather.timezone.trim().is_empty() {
        config.apple_weather.timezone.trim().to_owned()
    } else {
        m.as_ref()
            .ok()
            .map(|p| p.timezone.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| iana_time_zone::get_timezone().ok())
            .unwrap_or_else(|| "Etc/UTC".into())
    };
    let a = if selection == "open_meteo" {
        Err("not_requested".into())
    } else if apple_enabled {
        apple(lat, lon, &config.apple_weather, days, hours, &timezone).await
    } else {
        Err("disabled".into())
    };
    assemble_reports(lat, lon, apple_enabled, m, a, q)
}

fn assemble_reports(
    lat: f64,
    lon: f64,
    apple_enabled: bool,
    m: Result<ProviderForecast, String>,
    a: Result<ProviderForecast, String>,
    q: Result<AirQuality, String>,
) -> WeatherReports {
    let mut reports = WeatherReports { latitude: lat, longitude: lon, providers: vec![], air_quality: None, source_status: vec![],
        comparison_note: "Compare each provider independently at matching times and coordinates. Never average forecasts or invent missing values. AQI uses US standards and model estimates, not Chinese station observations. Apple daily forecasts are limited to 10 days." };
    for (source, result) in [("Open-Meteo", m), ("Apple Weather", a)] {
        let (status, error) = match result {
            Ok(p) => {
                reports.providers.push(p);
                ("ok", None)
            }
            Err(e) if e == "not_requested" => ("not_requested", None),
            Err(_) if source == "Apple Weather" && !apple_enabled => ("disabled", None),
            Err(e) => ("error", Some(e)),
        };
        reports.source_status.push(SourceStatus {
            source: source.into(),
            status: status.into(),
            error,
        });
    }
    let error = match q {
        Ok(q) => {
            reports.air_quality = Some(q);
            None
        }
        Err(e) => Some(e),
    };
    reports.source_status.push(SourceStatus {
        source: "Open-Meteo Air Quality".into(),
        status: if error.is_some() { "error" } else { "ok" }.into(),
        error,
    });
    reports
}

/// Compact context per source, deliberately contains no forecast arrays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WeatherSummary {
    pub source: String,
    pub observed_at: Option<String>,
    pub timezone: String,
    pub temperature: Option<f64>,
    pub description: Option<String>,
    pub humidity_pct: Option<f64>,
    pub pressure_hpa: Option<f64>,
    pub temp_max: Option<f64>,
    pub temp_min: Option<f64>,
    pub next_precipitation_time: Option<String>,
    pub next_precipitation_probability_pct: Option<f64>,
    pub precipitation_window_hours: usize,
    pub precipitation_probability_hours: usize,
    pub attribution_url: String,
}
impl ProviderForecast {
    pub fn summary(&self) -> WeatherSummary {
        let today = self.daily.first();
        let rain = self
            .hourly
            .iter()
            .take(12)
            .find(|h| h.precipitation_probability_pct.is_some_and(|p| p >= 30.0));
        WeatherSummary {
            source: self.source.clone(),
            observed_at: self.current.time.clone(),
            timezone: self.timezone.clone(),
            temperature: self.current.temperature,
            description: self.current.description.clone(),
            humidity_pct: self.current.humidity_pct,
            pressure_hpa: self.current.pressure_hpa,
            temp_max: today.and_then(|d| d.temp_max),
            temp_min: today.and_then(|d| d.temp_min),
            next_precipitation_time: rain.and_then(|h| h.time.clone()),
            next_precipitation_probability_pct: rain.and_then(|h| h.precipitation_probability_pct),
            precipitation_window_hours: self.hourly.len().min(12),
            precipitation_probability_hours: self
                .hourly
                .iter()
                .take(12)
                .filter(|h| h.precipitation_probability_pct.is_some())
                .count(),
            attribution_url: self.attribution_url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::{
        ecdsa::{signature::Verifier, VerifyingKey},
        pkcs8::{EncodePrivateKey, LineEnding},
    };

    fn meteo_fixture() -> Value {
        json!({"timezone":"Asia/Shanghai",
            "current":{"time":"2026-10-07T23:45", "temperature_2m":20.0,"apparent_temperature":19.0,
                "relative_humidity_2m":60.0,"wind_speed_10m":7.0,"pressure_msl":1013.0,"weather_code":3,"is_day":0},
            "daily":{"time":["2026-10-07","2026-10-08"],"temperature_2m_max":[25.0,null],"temperature_2m_min":[15.0],"weather_code":[3]},
            "hourly":{"time":["2026-10-07T22:00","2026-10-07T23:00","2026-10-08T00:00"],
                "temperature_2m":[21.0,20.0,null],"weather_code":[0,3,null],"precipitation_probability":[95,10,80]}})
    }

    #[test]
    fn meteo_weather_preserves_missing_values_and_starts_at_current_hour() {
        let p = parse_meteo(&meteo_fixture(), 2, 12).unwrap();
        assert_eq!(p.current.pressure_hpa, Some(1013.0));
        assert_eq!(p.hourly.len(), 2);
        assert_eq!(p.hourly[0].time.as_deref(), Some("2026-10-07T23:00"));
        assert_eq!(p.hourly[1].temperature, None);
        assert_eq!(p.hourly[1].description, None);
        assert_eq!(p.daily[1].temp_min, None);
        assert_eq!(p.daily[1].temp_max, None);
        let s = p.summary();
        assert_eq!(
            s.next_precipitation_time.as_deref(),
            Some("2026-10-08T00:00")
        );
        assert_eq!(s.next_precipitation_probability_pct, Some(80.0));
        assert_eq!(s.temp_max, Some(25.0));
        let summary = serde_json::to_value(s).unwrap();
        assert!(summary.get("hourly").is_none());
        assert!(summary.get("daily").is_none());
    }

    #[test]
    fn apple_weather_normalizes_units_and_preserves_its_condition_code() {
        let p = parse_apple(&json!({"currentWeather":{"temperature":21.5,"temperatureApparent":22,"conditionCode":"Rain",
            "humidity":0.75,"pressure":1009,"windSpeed":18,"asOf":"2026-10-07T15:45:00Z"},
            "forecastDaily":{"days":[{"forecastStart":"2026-10-06T16:00:00Z","temperatureMax":27,"temperatureMin":17,"precipitationChance":0.7}]},
            "forecastHourly":{"hours":[{"forecastStart":"2026-10-07T15:00:00Z","temperature":21.5,"conditionCode":"Rain","precipitationChance":0.8}]}
        }), 10, 24).unwrap();
        assert_eq!(p.current.humidity_pct, Some(75.0));
        assert_eq!(p.current.wind_speed_kmh, Some(18.0));
        assert_eq!(p.current.condition_code.as_deref(), Some("Rain"));
        assert_eq!(p.current.weather_code, None);
        assert_eq!(p.current.description.as_deref(), Some("雨"));
        assert_eq!(p.hourly[0].precipitation_probability_pct, Some(80.0));
        assert_eq!(p.daily[0].precipitation_probability_pct, Some(70.0));
        assert_eq!(p.daily[0].wind_speed_max_kmh, None);
        assert!(parse_apple(&json!({}), 10, 24).is_err());
    }

    #[test]
    fn weather_provider_failure_does_not_discard_other_sources() {
        let p = parse_meteo(&meteo_fixture(), 1, 12).unwrap();
        let r = assemble_reports(
            31.2,
            121.5,
            true,
            Ok(p),
            Err("HTTP 401".into()),
            Err("unavailable".into()),
        );
        assert_eq!(r.providers.len(), 1);
        assert_eq!(r.providers[0].source, "Open-Meteo");
        assert_eq!(r.source_status[1].status, "error");
        assert!(r.air_quality.is_none());
        let r = assemble_reports(
            31.2,
            121.5,
            false,
            Err("timeout".into()),
            Err("disabled".into()),
            Err("unavailable".into()),
        );
        assert!(r.providers.is_empty());
        assert_eq!(r.source_status[1].status, "disabled");
    }

    #[test]
    fn weatherkit_token_is_es256_signed_and_secret_is_redacted() {
        let key = SigningKey::from_slice(&[7; 32]).unwrap();
        let c = AppleWeatherConfig {
            enabled: true,
            team_id: "TEAM123456".into(),
            service_id: "com.test.weather".into(),
            key_id: "KEY1234567".into(),
            timezone: String::new(),
            api_secret: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        };
        let token = apple_token(&c).unwrap();
        let parts: Vec<_> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["id"], "TEAM123456.com.test.weather");
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["sub"], "com.test.weather");
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            3600
        );
        let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        VerifyingKey::from(&key)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
        assert!(!format!("{c:?}").contains("BEGIN PRIVATE KEY"));
        assert!(apple_token(&AppleWeatherConfig::default()).is_err());
    }
}
