pub mod http_client;
pub mod http_retry;
pub mod proxy;
pub mod request_utils;
pub mod url_fetcher;
pub mod web;

pub use http_client::{close_global_sessions, get_global_client};
pub use http_retry::{
    build_client_with_retry, default_retry_client, is_retryable_reqwest_error,
    is_retryable_status_code, retry_request_async, HttpRetryError, RetryConfig,
    RETRYABLE_STATUS_CODES,
};
pub use proxy::{
    build_client_with_proxy, no_proxy_hosts, system_proxy_url, ProxyConfig, ProxyMode,
};
pub use request_utils::{build_input, detect_format, SmartRequestBuilder};
pub use web::{
    WebError, WebErrorCode, WebSearchProvider, WebSearchRequest, WebSearchResult,
    WebSearchService, WebSearchSource,
};
