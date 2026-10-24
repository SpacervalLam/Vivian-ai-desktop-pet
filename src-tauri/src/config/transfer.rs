//! Portable preferences only. Never traverse user-data directories or copy memory stores.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const FORMAT: &str = "vivian-settings";
pub const VERSION: u32 = 1;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPackage {
    pub format: String,
    pub version: u32,
    pub settings: Value,
    pub character_settings: BTreeMap<String, CharacterSettings>,
    pub extra_settings: BTreeMap<String, Value>,
    pub chat_partners: Vec<crate::sns_store::ContactInput>,
    pub user_avatar: Option<String>,
    #[serde(default)]
    pub sticker_images: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterSettings {
    pub speech: Value,
    pub diary: Value,
}

// This list is deliberately closed: accepting arbitrary paths would allow memories to
// enter a share file, or a crafted share file to overwrite unrelated application data.
pub const EXTRA_FILES: &[&str] = &[
    "stickers/settings.json",
    "stickers/catalog.json",
    "mcp/servers.json",
    "mcp/overrides.json",
    "plugins/.disabled.json",
    "plugins/llm-providers/providers.json",
    "plugins/llm-providers/embedding-providers.json",
];

fn secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "key"
            | "token"
            | "password"
            | "authorization"
            | "cookie"
            | "apikey"
            | "apisecret"
            | "accesstoken"
    ) || ["_key", "_token", "_secret", "_password"]
        .iter()
        .any(|suffix| key.ends_with(suffix))
        || [
            "api_key",
            "api_secret",
            "access_key",
            "secret_key",
            "client_secret",
            "auth_token",
            "access_token",
            "refresh_token",
            "private_key",
            "password",
        ]
        .iter()
        .any(|suffix| key == *suffix || key.ends_with(&format!("_{suffix}")))
        || matches!(
            key.as_str(),
            "azure_key" | "fish_speech_key" | "minimax_key" | "mimo_key" | "elevenlabs_key"
        )
}

/// Redact in both directions: even a hand-edited file cannot supply API credentials.
pub fn clear_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if secret(key) {
                    if !child.is_null() {
                        *child = Value::String(String::new());
                    }
                } else {
                    clear_secrets(child);
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                clear_secrets(child);
            }
        }
        _ => {}
    }
}

/// Keep local credentials only when the service identity is unchanged. Arrays are
/// matched by id, never by position (work-model profiles may have been reordered).
pub fn retain_local_secrets(next: &mut Value, local: &Value) {
    match (next, local) {
        (Value::Object(next), Value::Object(local)) => {
            if next.keys().chain(local.keys()).any(|key| {
                (matches!(
                    key.as_str(),
                    "provider_type"
                        | "provider"
                        | "endpoint"
                        | "url"
                        | "command"
                        | "engine"
                        | "source"
                        | "app_id"
                        | "team_id"
                        | "service_id"
                ) || key.ends_with("_url")
                    || key.ends_with("_endpoint"))
                    && next.get(key) != local.get(key)
            }) {
                return;
            }
            for (key, value) in next {
                if let Some(old) = local.get(key) {
                    if secret(key) {
                        *value = old.clone();
                    } else {
                        retain_local_secrets(value, old);
                    }
                }
            }
        }
        (Value::Array(next), Value::Array(local)) => {
            for value in next {
                if let Some(id) = value.get("id") {
                    if let Some(old) = local.iter().find(|old| old.get("id") == Some(id)) {
                        retain_local_secrets(value, old);
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn decode(bytes: &[u8]) -> Result<SettingsPackage, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("配置文件超过 64 MiB".into());
    }
    let mut package: SettingsPackage = serde_json::from_slice(bytes)
        .map_err(|_| "不是有效的配置分享文件；完整数据备份请使用恢复备份功能".to_string())?;
    if package.format != FORMAT || package.version != VERSION {
        return Err("不支持的配置分享格式或版本".into());
    }
    if package
        .extra_settings
        .keys()
        .any(|key| !EXTRA_FILES.contains(&key.as_str()))
    {
        return Err("配置文件包含不支持的附加数据".into());
    }
    clear_secrets(&mut package.settings);
    for config in package.character_settings.values_mut() {
        clear_secrets(&mut config.speech);
    }
    for value in package.extra_settings.values_mut() {
        clear_secrets(value);
    }
    for partner in &mut package.chat_partners {
        if let Some(model) = &mut partner.model {
            clear_secrets(model);
        }
    }
    Ok(package)
}

pub fn valid_sticker_filename(name: &str) -> bool {
    name.len() <= 220
        && name.ends_with(".png")
        && name[..name.len() - 4]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Publish the preference files as one recoverable operation. Callers commit the
/// validated main configuration and contact transaction only after all files write.
pub fn apply_files<T>(
    writes: &[(std::path::PathBuf, Vec<u8>)],
    commit: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let originals: Vec<_> = writes
        .iter()
        .map(|(path, _)| match std::fs::read(path) {
            Ok(bytes) => Ok((path.clone(), Some(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((path.clone(), None)),
            Err(e) => Err(e.to_string()),
        })
        .collect::<Result<_, _>>()?;
    let mut written = 0;
    let result = (|| {
        for (path, bytes) in writes {
            crate::utils::fs::atomic_write(path, bytes).map_err(|e| e.to_string())?;
            written += 1;
        }
        commit()
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut failures = Vec::new();
            for (path, bytes) in originals.into_iter().take(written).rev() {
                let result = if let Some(bytes) = bytes {
                    crate::utils::fs::atomic_write(&path, &bytes)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(e) = result {
                    failures.push(e.to_string());
                }
            }
            Err(if failures.is_empty() {
                error
            } else {
                format!("{error}；附加设置回滚失败：{}", failures.join("；"))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_commit_restores_settings_and_removes_new_assets_without_touching_memory() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        let asset = temp.path().join("new.png");
        let memory = temp.path().join("memory.sqlite");
        std::fs::write(&settings, b"old-settings").unwrap();
        std::fs::write(&memory, b"existing-memory").unwrap();
        let result: Result<(), String> = apply_files(
            &[
                (settings.clone(), b"new-settings".to_vec()),
                (asset.clone(), b"new-avatar".to_vec()),
            ],
            || Err("invalid contact".into()),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&settings).unwrap(), b"old-settings");
        assert!(!asset.exists());
        assert_eq!(std::fs::read(&memory).unwrap(), b"existing-memory");
        apply_files(&[(settings.clone(), b"new-settings".to_vec())], || Ok(())).unwrap();
        assert_eq!(std::fs::read(&settings).unwrap(), b"new-settings");
    }

    #[test]
    fn removes_nested_and_speech_credentials_but_keeps_routes() {
        let mut data = json!({"routing_matrix":{"companion":{"model":"test", "api_key":"secret", "api_secret":"secret"}},
            "provider_cache":{"x":{"api_key":"secret"}}, "azure_key":"secret", "translation_api_key":"secret",
            "doubao_access_token":"secret", "headers":{"Authorization":"Bearer secret", "X-API-Key":"secret"},
            "env":{"OPENAI_API_KEY":"secret"}, "max_tokens":123});
        clear_secrets(&mut data);
        assert!(!data.to_string().contains(":\"secret\""));
        assert!(!data.to_string().contains("Bearer secret"));
        assert_eq!(data["routing_matrix"]["companion"]["model"], "test");
        assert_eq!(data["max_tokens"], 123);
    }

    #[test]
    fn retains_only_matching_service_credentials_and_matches_profiles_by_id() {
        let local = json!({"profiles":[{"id":"a","endpoint":"a","api_key":"A"},{"id":"b","endpoint":"b","api_key":"B"}]});
        let mut next = json!({"profiles":[{"id":"b","endpoint":"b","api_key":""},{"id":"a","endpoint":"evil","api_key":""}]});
        retain_local_secrets(&mut next, &local);
        assert_eq!(next["profiles"][0]["api_key"], "B");
        assert_eq!(next["profiles"][1]["api_key"], "");
    }

    #[test]
    fn share_codec_rejects_backups_versions_and_unlisted_data() {
        let package = json!({"format":FORMAT,"version":VERSION,"settings":{},"character_settings":{},
            "extra_settings":{},"chat_partners":[],"user_avatar":null});
        assert!(decode(&serde_json::to_vec(&package).unwrap()).is_ok());
        assert!(decode(b"ALTNBAK1").is_err());
        let mut invalid = package.clone();
        invalid["version"] = json!(999);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = package;
        invalid["extra_settings"]["characters/vivian/memory/data.json"] = json!({});
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        assert!(!valid_sticker_filename("../../config.png"));
        assert!(!valid_sticker_filename("C:\\config.png"));
        assert!(valid_sticker_filename("vivian_custom_1-v2.png"));
    }

    #[test]
    fn imports_append_and_rename_roles_and_rollback_invalid_batch() {
        use crate::sns_store::{ContactInput, SnsStore};
        let temp = tempfile::tempdir().unwrap();
        let store = SnsStore::new(temp.path().join("contacts.sqlite"));
        let role = || ContactInput {
            name: "Alice".into(),
            persona: "A friendly character".into(),
            model: None,
            avatar: String::new(),
        };
        let old = store.save_contact(None, role()).unwrap();
        let imported = store.append_shared_contacts(vec![role(), role()]).unwrap();
        assert_eq!(imported[0].name, "Alice (2)");
        assert_eq!(imported[1].name, "Alice (3)");
        assert_ne!(old.id, imported[0].id);
        assert_eq!(store.contact(&old.id).unwrap().persona, old.persona);
        let mut invalid = role();
        invalid.persona.clear();
        assert!(store.append_shared_contacts(vec![role(), invalid]).is_err());
        assert_eq!(store.contacts().unwrap().len(), 3);
    }
}
