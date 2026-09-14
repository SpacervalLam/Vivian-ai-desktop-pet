//! 配置中的密钥字段透明保护。
//!
//! Windows 使用 DPAPI（CurrentUser）加密，密文可随 YAML 保存但不能被其他用户或
//! 复制到其他机器后解密。Unix 仍依赖配置文件 0o600 权限。

const PREFIX: &str = "dpapi:v1:";

fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key"
            | "api_secret"
            | "access_key"
            | "auth_token"
            | "secret_key"
            | "client_secret"
            | "password"
            | "token"
    )
}

pub fn protect_yaml(value: &mut serde_yaml::Value) -> Result<(), String> {
    transform(value, true)
}

pub fn unprotect_yaml(value: &mut serde_yaml::Value) -> Result<(), String> {
    transform(value, false)
}

fn transform(value: &mut serde_yaml::Value, protect: bool) -> Result<(), String> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (key, child) in map.iter_mut() {
                let key = key.as_str().unwrap_or_default();
                if is_secret_key(key) {
                    if let Some(secret) = child.as_str() {
                        if secret.is_empty() {
                            continue;
                        }
                        let transformed = if protect {
                            protect_secret(secret)?
                        } else {
                            unprotect_secret(secret)?
                        };
                        *child = serde_yaml::Value::String(transformed);
                    }
                } else {
                    transform(child, protect)?;
                }
            }
        }
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                transform(item, protect)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(windows)]
fn protect_secret(secret: &str) -> Result<String, String> {
    if secret.starts_with(PREFIX) {
        return Ok(secret.to_string());
    }
    use base64::Engine;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| "密钥过长".to_string())?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|e| format!("DPAPI 加密失败: {e}"))?;
        let protected = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let encoded = base64::engine::general_purpose::STANDARD.encode(protected);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(format!("{PREFIX}{encoded}"))
    }
}

#[cfg(windows)]
fn unprotect_secret(secret: &str) -> Result<String, String> {
    let Some(encoded) = secret.strip_prefix(PREFIX) else {
        return Ok(secret.to_string());
    };
    use base64::Engine;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let encrypted = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("DPAPI 密文 Base64 无效: {e}"))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len().try_into().map_err(|_| "密文过长".to_string())?,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|e| format!("DPAPI 解密失败: {e}"))?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let plain = String::from_utf8(bytes.to_vec())
            .map_err(|e| format!("DPAPI 明文不是 UTF-8: {e}"));
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        plain
    }
}

#[cfg(not(windows))]
fn protect_secret(secret: &str) -> Result<String, String> {
    Ok(secret.to_string())
}

#[cfg(not(windows))]
fn unprotect_secret(secret: &str) -> Result<String, String> {
    Ok(secret.to_string())
}
