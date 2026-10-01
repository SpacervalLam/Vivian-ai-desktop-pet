//! Legacy configuration tombstone: Bing Search v7 retired on 2025-08-11.
//! Credentials remain readable for migration, but are never sent to a retired endpoint.
use crate::config::WebSearchConfig;
use crate::network::web::{WebError, WebSearchProvider, WebSearchRequest, WebSearchResult};
use async_trait::async_trait;
use std::sync::Arc;
struct RetiredBing;
pub fn bing_factory(_: Option<&WebSearchConfig>, _: Option<&str>) -> Arc<dyn WebSearchProvider> {
    Arc::new(RetiredBing)
}
#[async_trait]
impl WebSearchProvider for RetiredBing {
    fn id(&self) -> &'static str {
        "bing"
    }
    fn available(&self) -> bool {
        false
    }
    async fn search(&self, _: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
        Err(WebError::configured_unavailable(
            "bing (Search v7 retired; choose an enabled supported engine)",
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_bing_never_available() {
        let mut c = WebSearchConfig::default();
        c.bing.api_key = "legacy".into();
        assert!(!bing_factory(Some(&c), None).available());
    }
}
