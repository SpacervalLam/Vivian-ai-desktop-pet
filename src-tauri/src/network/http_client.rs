use std::time::Duration;

use once_cell::sync::OnceCell;
use reqwest::Client;

static GLOBAL_CLIENT: OnceCell<Client> = OnceCell::new();

pub fn get_global_client() -> Client {
    GLOBAL_CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Duration::from_secs(60))
                .build()
                .expect("Failed to build global HTTP client")
        })
        .clone()
}

pub async fn close_global_sessions() {
    // Noop placeholder for compatibility.
}
