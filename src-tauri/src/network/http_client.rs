use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use reqwest::Client;

use crate::network::proxy::ProxyConfig;

/// 全局 HTTP 客户端（带代理配置，支持热重载）
///
/// - 启动时由 `init_global_client` 注入 ProxyConfig 构造一次
/// - 配置变更时由 `reload_global_client` 重新构造（旧 client 由 Arc 引用计数自动回收）
/// - 未初始化时 fallback 到无代理直连 client
static GLOBAL_CLIENT: once_cell::sync::OnceCell<RwLock<Arc<Client>>> = once_cell::sync::OnceCell::new();

fn build_client(proxy_config: Option<&ProxyConfig>) -> Client {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60));

    if let Some(pc) = proxy_config {
        if let Some(url) = pc.effective_proxy_url() {
            tracing::info!(
                "[HttpClient] 全局客户端使用代理: {} (mode={})",
                url,
                pc.mode.as_str()
            );
            if let Ok(proxy) = reqwest::Proxy::all(&url) {
                builder = builder.proxy(proxy);
            } else {
                tracing::warn!("[HttpClient] 代理 URL 无效: {url},回退直连");
            }
        } else {
            // 不需要代理时显式禁用环境变量代理检测
            builder = builder.no_proxy();
        }
    }

    builder.build().unwrap_or_else(|_| Client::new())
}

/// 启动时初始化全局 HTTP 客户端（注入代理配置）
///
/// 应在 app setup 阶段、所有角色初始化之前调用一次。
/// 重复调用会被 OnceCell 忽略；要更新代理请用 `reload_global_client`。
pub fn init_global_client(proxy_config: ProxyConfig) {
    GLOBAL_CLIENT.get_or_init(|| RwLock::new(Arc::new(build_client(Some(&proxy_config)))));
}

/// 配置变更时重新构造全局客户端（保留 Arc 引用计数兼容旧连接）
///
/// 若 `init_global_client` 从未被调用则初始化一次。
pub fn reload_global_client(proxy_config: ProxyConfig) {
    let cell = GLOBAL_CLIENT.get_or_init(|| RwLock::new(Arc::new(build_client(None))));
    let new_client = Arc::new(build_client(Some(&proxy_config)));
    *cell.write() = new_client;
}

/// 获取全局客户端快照
///
/// 未初始化时返回一个无代理的临时 client（向后兼容早期启动阶段调用）。
pub fn get_global_client() -> Client {
    match GLOBAL_CLIENT.get() {
        Some(cell) => {
            let guard = cell.read();
            let arc: &Arc<Client> = &*guard;
            let client: &Client = arc.as_ref();
            client.clone()
        }
        None => build_client(None),
    }
}

/// Noop placeholder for compatibility.
pub async fn close_global_sessions() {}
