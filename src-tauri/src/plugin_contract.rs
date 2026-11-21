//! Version of the interfaces available to an executable plugin.
pub const PLUGIN_API_VERSION: u32 = 1;

pub fn default_api_version() -> u32 { PLUGIN_API_VERSION }

/// Map iteration order must not invalidate an otherwise unchanged plugin's trust.
pub(crate) fn serialize_string_map<S: serde::Serializer>(
    values: &std::collections::HashMap<String, String>, serializer: S,
) -> Result<S::Ok, S::Error> {
    let sorted: std::collections::BTreeMap<_, _> = values.iter().collect();
    serde::Serialize::serialize(&sorted, serializer)
}

pub fn validate_api_version(version: u32) -> Result<(), String> {
    if version == PLUGIN_API_VERSION { return Ok(()); }
    Err(format!("插件 API {version} 与宿主 API {PLUGIN_API_VERSION} 不兼容，请使用匹配版本的插件或升级应用"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_future_and_obsolete_contracts() {
        assert!(validate_api_version(PLUGIN_API_VERSION).is_ok());
        assert!(validate_api_version(0).is_err());
        assert!(validate_api_version(PLUGIN_API_VERSION + 1).is_err());
    }
}
