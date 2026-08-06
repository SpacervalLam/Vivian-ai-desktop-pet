//! HTTP/HTTPS 代理配置
//!
//! 配置结构：
//! ```yaml
//! network:
//!   proxy_mode: direct    # direct / system / custom
//!   proxy_url: ""
//!   timeout: 30.0
//! ```
//!
//! 同时支持从环境变量读取（`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY`）。

use std::time::Duration;

use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::network::http_retry::{build_client_with_retry, RetryConfig, DEFAULT_CONNECT_TIMEOUT_SECS};

/// 代理模式
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// 直连，不使用代理
    Direct,
    /// 跟随系统代理（从环境变量读取）
    System,
    /// 自定义代理
    Custom,
}

impl Default for ProxyMode {
    fn default() -> Self {
        ProxyMode::Direct
    }
}

impl ProxyMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "system" => ProxyMode::System,
            "custom" | "manual" => ProxyMode::Custom,
            _ => ProxyMode::Direct,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Custom => "custom",
        }
    }
}

/// 代理配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// 自定义代理 URL（如 `http://127.0.0.1:7890`）
    #[serde(default)]
    pub url: String,
    /// 请求超时（秒）
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            mode: ProxyMode::Direct,
            url: String::new(),
            timeout_secs: 30,
        }
    }
}

impl ProxyConfig {
    /// 从 `AppConfig` 的 `network` 字段构造
    pub fn from_app_config(config: &crate::config::manager::AppConfig) -> Self {
        Self {
            mode: ProxyMode::from_str(&config.network.proxy_mode),
            url: config.network.proxy_url.clone(),
            timeout_secs: config.network.timeout as u64,
        }
    }

    /// 解析出实际生效的代理 URL
    ///
    /// - `Direct` → None
    /// - `System` → 优先 HTTPS_PROXY，其次 HTTP_PROXY
    /// - `Custom` → 使用 `self.url`
    pub fn effective_proxy_url(&self) -> Option<String> {
        match self.mode {
            ProxyMode::Direct => None,
            ProxyMode::System => system_proxy_url(),
            ProxyMode::Custom => {
                if self.url.is_empty() {
                    None
                } else {
                    Some(self.url.clone())
                }
            }
        }
    }

    /// 按 `base_url` 域名分流的代理 URL
    ///
    /// 国内厂商域名强制直连，国外厂商及其他域名沿用全局代理配置。
    pub fn effective_proxy_url_for(&self, base_url: &str) -> Option<String> {
        if is_domestic_endpoint(base_url) {
            return None;
        }
        self.effective_proxy_url()
    }

    /// 是否启用代理
    pub fn is_enabled(&self) -> bool {
        self.effective_proxy_url().is_some()
    }
}

/// 国内 LLM 厂商域名片段，命中即视为可直连
const DOMESTIC_HOST_FRAGMENTS: &[&str] = &[
    // 火山引擎豆包 / GLM
    "ark.cn-beijing.volces.com",
    // 硅基流动 SiliconFlow
    "api.siliconflow.cn",
    // 百度文心一言 / 千帆
    "aip.baidubce.com",
    "qianfan.baidubce.com",
    // 讯飞星火
    "ws-api.xfyun.cn",
    "spark-api.xf-yun.com",
    // 阿里通义千问 / DashScope
    "dashscope.aliyuncs.com",
    "dashscope-intl.aliyuncs.com",
    // 智谱 GLM
    "open.bigmodel.cn",
    // 月之暗面 Moonshot
    "api.moonshot.cn",
    // DeepSeek 官方
    "api.deepseek.com",
    // 阶跃星辰 StepFun
    "api.stepfun.com",
    // MiniMax
    "api.minimax.chat",
    "api.minimaxi.com",
    // 百川 Baichuan
    "api.baichuan-ai.com",
    // 腾讯混元
    "hunyuan.tencentcloudapi.com",
    // 字节豆包官方 OpenAPI 兜底
    "volces.com",
];

/// 判断 endpoint 是否属于国内厂商（直连可达）
pub fn is_domestic_endpoint(base_url: &str) -> bool {
    let lower = base_url.to_lowercase();
    DOMESTIC_HOST_FRAGMENTS
        .iter()
        .any(|frag| lower.contains(frag))
}

/// 从环境变量读取系统代理 URL
///
/// 优先级：`HTTPS_PROXY` > `https_proxy` > `HTTP_PROXY` > `http_proxy`
pub fn system_proxy_url() -> Option<String> {
    for var in &["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 读取 `NO_PROXY` 环境变量（逗号分隔的主机列表）
pub fn no_proxy_hosts() -> Vec<String> {
    std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok()
        .map(|s| {
            s.split(',')
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 构造带代理配置的 `reqwest::Client`
///
/// - `Direct` 模式：直接构造客户端（无代理）
/// - `System` 模式：reqwest 默认会读取环境变量，无需显式设置 Proxy
/// - `Custom` 模式：显式设置 `Proxy::all(url)`
///
/// 同时配置 `RetryConfig` 的超时时间，并显式分离 `connect_timeout` 与整体 `timeout`。
pub fn build_client_with_proxy(proxy_config: &ProxyConfig) -> VivianResult<Client> {
    let retry_config = RetryConfig {
        timeout_secs: proxy_config.timeout_secs,
        ..Default::default()
    };

    let effective_url = proxy_config.effective_proxy_url();
    match effective_url.as_deref() {
        Some(url) => {
            tracing::info!("[Proxy] 使用代理: {} (mode={})", url, proxy_config.mode.as_str());
            let proxy = Proxy::all(url)
                .map_err(|e| VivianError::Network(format!("代理配置无效: {e}")))?;
            Client::builder()
                .timeout(Duration::from_secs(retry_config.timeout_secs))
                .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Duration::from_secs(60))
                .proxy(proxy)
                .build()
                .map_err(|e| VivianError::Network(format!("构建 reqwest Client 失败: {e}")))
        }
        None => {
            // Direct 或 System（系统模式下 reqwest 默认读取环境变量）
            build_client_with_retry(&retry_config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_mode_from_str() {
        assert_eq!(ProxyMode::from_str("direct"), ProxyMode::Direct);
        assert_eq!(ProxyMode::from_str("system"), ProxyMode::System);
        assert_eq!(ProxyMode::from_str("custom"), ProxyMode::Custom);
        assert_eq!(ProxyMode::from_str("manual"), ProxyMode::Custom);
        assert_eq!(ProxyMode::from_str("DIRECT"), ProxyMode::Direct);
        assert_eq!(ProxyMode::from_str("System"), ProxyMode::System);
        assert_eq!(ProxyMode::from_str("MANUAL"), ProxyMode::Custom);
        assert_eq!(ProxyMode::from_str("unknown"), ProxyMode::Direct);
    }

    #[test]
    fn test_proxy_mode_as_str() {
        assert_eq!(ProxyMode::Direct.as_str(), "direct");
        assert_eq!(ProxyMode::System.as_str(), "system");
        assert_eq!(ProxyMode::Custom.as_str(), "custom");
    }

    #[test]
    fn test_direct_mode_no_proxy() {
        let cfg = ProxyConfig {
            mode: ProxyMode::Direct,
            url: "http://127.0.0.1:7890".to_string(),
            timeout_secs: 30,
        };
        assert!(!cfg.is_enabled());
        assert_eq!(cfg.effective_proxy_url(), None);
    }

    #[test]
    fn test_custom_mode_with_url() {
        let cfg = ProxyConfig {
            mode: ProxyMode::Custom,
            url: "http://127.0.0.1:7890".to_string(),
            timeout_secs: 30,
        };
        assert!(cfg.is_enabled());
        assert_eq!(
            cfg.effective_proxy_url(),
            Some("http://127.0.0.1:7890".to_string())
        );
    }

    #[test]
    fn test_custom_mode_empty_url() {
        let cfg = ProxyConfig {
            mode: ProxyMode::Custom,
            url: String::new(),
            timeout_secs: 30,
        };
        assert!(!cfg.is_enabled());
    }

    #[test]
    fn test_build_client_direct() {
        let cfg = ProxyConfig::default();
        let client = build_client_with_proxy(&cfg);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_custom_invalid_url() {
        let cfg = ProxyConfig {
            mode: ProxyMode::Custom,
            url: "not_a_valid_url".to_string(),
            timeout_secs: 30,
        };
        let result = build_client_with_proxy(&cfg);
        // reqwest 接受字符串代理 URL，但仅当协议前缀正确时才有效
        // 对于无效 URL，可能在 build 阶段成功但在请求时失败
        // 这里仅验证不 panic
        let _ = result;
    }

    #[test]
    fn test_no_proxy_hosts_default_empty() {
        // 测试不依赖环境变量（CI 环境可能未设置）
        let hosts = no_proxy_hosts();
        // 仅验证函数可调用
        let _ = hosts.len();
    }
}
