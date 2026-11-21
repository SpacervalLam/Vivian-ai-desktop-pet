//! 地理定位 —— 混合策略：Windows 系统定位优先，IP 定位兜底。
//!
//! - Windows 系统定位：精度较高，依赖定位服务开启 + 应用权限授予
//! - IP 定位：无需权限，精度城市级，作为系统定位不可用时的兜底
//!
//! 所有定位请求均走直连客户端（显式绕过系统代理）：
//! 经代理请求 IP 定位接口时，对方看到的是代理出口 IP，
//! 返回的是代理服务器位置而非用户实际位置。

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// 直连 HTTP 客户端（绕过系统代理）
static DIRECT_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default()
});

/// 地理定位结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoInfo {
    pub latitude: f64,
    pub longitude: f64,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub source: String,
}

/// 检测设备地理位置，返回包含坐标与城市信息的 GeoInfo。
///
/// 优先尝试 Windows 系统定位（5 秒超时），失败或超时则回退到 IP 定位。
pub async fn detect_location() -> Option<GeoInfo> {
    #[cfg(windows)]
    {
        let win_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(try_windows_geolocation),
        )
        .await;

        if let Ok(Ok(Some(info))) = win_result {
            tracing::info!(
                "系统定位成功: ({:.4}, {:.4})",
                info.latitude,
                info.longitude
            );
            return Some(enrich_city_info(info).await);
        }
    }

    match try_ip_geolocation().await {
        Some(info) => {
            tracing::info!(
                "IP 定位成功: ({:.4}, {:.4}) 城市: {}",
                info.latitude,
                info.longitude,
                info.city.as_deref().unwrap_or("未知")
            );
            Some(info)
        }
        None => {
            tracing::warn!("地理定位失败：系统定位与 IP 定位均不可用");
            None
        }
    }
}

/// 反向地理编码 —— 按真实坐标补充城市/地区/国家信息。
///
/// Windows 系统定位仅返回坐标，城市信息通过 BigDataCloud 免费反查接口补充，
/// 保证城市与坐标严格一致（IP 查询的城市可能因运营商 IP 漂移与实际坐标偏差）。
/// 反查失败时回退直连 IP 查询（真实出口 IP 的城市级位置），
/// 仍失败则保持 None（坐标仍可用于天气等功能）。
async fn enrich_city_info(mut info: GeoInfo) -> GeoInfo {
    if info.city.is_some() {
        return info;
    }

    let url = format!(
        "https://api.bigdatacloud.net/data/reverse-geocode-client?latitude={}&longitude={}&localityLanguage=zh",
        info.latitude, info.longitude
    );
    if let Ok(resp) = DIRECT_CLIENT.get(&url).send().await {
        if let Ok(body) = resp.json::<ReverseGeoResponse>().await {
            info.city = body.city.or(body.locality);
            info.region = body.principal_subdivision;
            info.country = body.country_name;
        }
    }

    // 回退：按真实出口 IP 查询（直连）
    if info.city.is_none() {
        if let Ok(resp) = DIRECT_CLIENT.get("https://ipwho.is/").send().await {
            if let Ok(body) = resp.json::<IpGeoResponse>().await {
                if body.success {
                    info.city = body.city;
                    info.region = body.region;
                    info.country = body.country;
                }
            }
        }
    }

    info
}

/// Windows 系统定位（阻塞式，需在 spawn_blocking 中调用）。
///
/// windows 0.58 的 IAsyncOperation 未实现 Future，用阻塞 `.get()` 等待完成。
/// 系统定位仅返回坐标，城市信息由 `enrich_city_info` 补充。
#[cfg(windows)]
fn try_windows_geolocation() -> Option<GeoInfo> {
    use windows::Devices::Geolocation::{Geolocator, GeolocationAccessStatus};

    // 请求定位权限（系统会弹出授权提示，已授权则直接返回 Allowed）
    let access_op = Geolocator::RequestAccessAsync().ok()?;
    let access = access_op.get().ok()?;

    if !matches!(access, GeolocationAccessStatus::Allowed) {
        return None;
    }

    let geolocator = Geolocator::new().ok()?;
    let pos_op = geolocator.GetGeopositionAsync().ok()?;
    let pos = pos_op.get().ok()?;

    let coord = pos.Coordinate().ok()?;
    let point = coord.Point().ok()?;
    let position = point.Position().ok()?;
    Some(GeoInfo {
        latitude: position.Latitude,
        longitude: position.Longitude,
        city: None,
        region: None,
        country: None,
        source: "windows".to_string(),
    })
}

#[cfg(not(windows))]
fn try_windows_geolocation() -> Option<GeoInfo> {
    None
}

/// IP 定位 —— 通过 ipwho.is 免费 API 获取城市级坐标与地点信息。
///
/// 直连请求（绕过代理），确保返回用户真实出口 IP 的位置。
async fn try_ip_geolocation() -> Option<GeoInfo> {
    let resp = DIRECT_CLIENT.get("https://ipwho.is/").send().await.ok()?;
    let body: IpGeoResponse = resp.json().await.ok()?;
    if body.success {
        Some(GeoInfo {
            latitude: body.latitude,
            longitude: body.longitude,
            city: body.city,
            region: body.region,
            country: body.country,
            source: "ip".to_string(),
        })
    } else {
        None
    }
}

#[derive(Deserialize)]
struct IpGeoResponse {
    success: bool,
    latitude: f64,
    longitude: f64,
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReverseGeoResponse {
    city: Option<String>,
    locality: Option<String>,
    principal_subdivision: Option<String>,
    country_name: Option<String>,
}
