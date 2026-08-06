//! 配置命令 - 读取、修改、保存与重载配置

use std::path::PathBuf;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::network::proxy::{build_client_with_proxy, ProxyConfig};
use crate::state::AppState;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 读取指定键的配置值（支持点号分隔的嵌套键）
#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>, key: String) -> Result<Value, String> {
    let config = state.config.read();
    Ok(config.get(&key))
}

/// 设置指定键的配置值（仅修改内存，不写入磁盘）
#[tauri::command]
pub fn set_config(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: Value,
) -> Result<(), String> {
    let config = state.config.read();
    config.set_no_save(&key, value).map_err(err_str)
}

/// 获取完整配置
#[tauri::command]
pub fn get_all_config(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let config = state.config.read();
    let all = config.get_all();
    serde_json::to_value(&all).map_err(err_str)
}

/// 保存配置到磁盘
#[tauri::command]
pub fn save_config(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let config = state.config.read();
    config.save().map_err(err_str)?;
    // 配置可能变更了模型/路由，清空视觉能力探测缓存让下次发图重新探测
    if let Some(router) = state.model_router.read().as_ref() {
        router.clear_vision_capability_cache();
    }
    Ok(())
}

/// 从磁盘重载配置
#[tauri::command]
pub fn reload_config(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let config = state.config.read();
    config.reload().map_err(err_str)?;
    // 重载后模型可能已变更，清空视觉能力探测缓存让下次发图重新探测
    if let Some(router) = state.model_router.read().as_ref() {
        router.clear_vision_capability_cache();
    }
    Ok(())
}

/// 网络连接测试结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTestResult {
    /// 是否连通
    pub success: bool,
    /// HTTP 状态码（若收到响应）
    pub status_code: Option<u16>,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
    /// 使用的代理模式
    pub proxy_mode: String,
    /// 实际生效的代理 URL（仅显示，可能为 None）
    pub effective_proxy: Option<String>,
    /// 错误信息（失败时）
    pub error: Option<String>,
}

/// 测试网络连接 —— 通过当前网络设置访问 Google 主页验证代理可用性
///
/// 使用 `AppConfig.network` 中的代理模式 / 代理地址 / 超时构建专属客户端，
/// 发送 GET 请求到 `https://www.google.com`，返回连接结果。
#[tauri::command]
pub async fn test_network_connection(
    state: State<'_, Arc<AppState>>,
) -> Result<NetworkTestResult, String> {
    // 在跨 await 之前完成对配置读锁的获取与释放，避免 guard 跨 await 导致 Future 不满足 Send
    let (proxy_config, effective_proxy, proxy_mode) = {
        let config = state.config.read();
        let app_config = config.get_all();
        let pc = ProxyConfig::from_app_config(&app_config);
        let ep = pc.effective_proxy_url();
        let pm = pc.mode.as_str().to_string();
        (pc, ep, pm)
    };

    // 测试目标 —— Google 主页，作为代理可用性的典型判定
    const TEST_URL: &str = "https://www.google.com";

    let client = match build_client_with_proxy(&proxy_config) {
        Ok(c) => c,
        Err(e) => {
            return Ok(NetworkTestResult {
                success: false,
                status_code: None,
                elapsed_ms: 0,
                proxy_mode,
                effective_proxy,
                error: Some(format!("客户端构建失败: {}", e)),
            });
        }
    };

    let start = std::time::Instant::now();
    let result = client.get(TEST_URL).send().await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(resp) => Ok(NetworkTestResult {
            success: true,
            status_code: Some(resp.status().as_u16()),
            elapsed_ms,
            proxy_mode,
            effective_proxy,
            error: None,
        }),
        Err(e) => Ok(NetworkTestResult {
            success: false,
            status_code: None,
            elapsed_ms,
            proxy_mode,
            effective_proxy,
            error: Some(if e.is_timeout() {
                format!("请求超时（{}s）", proxy_config.timeout_secs)
            } else if e.is_connect() {
                format!("连接失败: {}", e)
            } else {
                e.to_string()
            }),
        }),
    }
}

// ===== 用户自定义头像 =====

/// 根据文件头部魔数检测实际图片格式，返回 MIME 类型。
/// 比扩展名更可靠：用户上传的 `.png` 文件可能实际是 JPEG。
pub(crate) fn detect_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 8 {
        // PNG: 89 50 4E 47 0D 0A 1A 0A
        if bytes[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
            return "image/png";
        }
        // JPEG: FF D8 FF
        if bytes[0..3] == [0xFF, 0xD8, 0xFF] {
            return "image/jpeg";
        }
        // GIF: 47 49 46 38 ("GIF8")
        if bytes[0..4] == [0x47, 0x49, 0x46, 0x38] {
            return "image/gif";
        }
        // WebP: RIFF....WEBP
        if bytes[0..4] == [0x52, 0x49, 0x46, 0x46] && bytes[8..12] == [0x57, 0x45, 0x42, 0x50] {
            return "image/webp";
        }
        // BMP: 42 4D ("BM")
        if bytes[0..2] == [0x42, 0x4D] {
            return "image/bmp";
        }
    }
    "image/png"
}

/// 读取任意图片文件并返回 data URL（base64 编码）
///
/// 通过魔数检测实际图片格式，确保 MIME 类型与文件内容一致。
/// 头像、聊天图片、记忆图片等共用此实现。
pub(crate) fn image_to_data_url(path: &std::path::Path) -> Result<Option<String>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取图片文件失败: {}", e)),
    };
    let mime = detect_image_mime(&bytes);
    let b64 = STANDARD.encode(&bytes);
    Ok(Some(format!("data:{};base64,{}", mime, b64)))
}

/// 读取头像文件并返回 data URL（base64 编码）
fn avatar_to_data_url(path: &PathBuf) -> Result<Option<String>, String> {
    image_to_data_url(path)
}

/// 保存用户自定义头像
///
/// 将用户选择的图片文件复制到用户数据目录下作为 avatar，并写入配置。
/// 返回 base64 data URL 供前端立即渲染（避免 fs scope 限制）。
#[tauri::command]
pub async fn save_user_avatar(
    state: State<'_, Arc<AppState>>,
    source_path: String,
) -> Result<Option<String>, String> {
    let src = PathBuf::from(&source_path);
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let data_dir = crate::utils::path::get_user_data_dir();
    crate::utils::path::ensure_dir(&data_dir).map_err(|e| format!("创建数据目录失败: {}", e))?;
    let dest = data_dir.join(format!("avatar.{}", ext));

    // 复制文件（覆盖旧头像）；源文件不存在时直接返回友好错误
    std::fs::copy(&src, &dest).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "源文件不存在".to_string()
        } else {
            format!("复制头像文件失败: {}", e)
        }
    })?;

    // 删除其他扩展名的旧头像，避免残留；失败仅 warn 不阻塞主流程
    for old_ext in &["png", "jpg", "jpeg", "gif", "webp", "bmp"] {
        if *old_ext != ext {
            let old = data_dir.join(format!("avatar.{}", old_ext));
            if let Err(e) = std::fs::remove_file(&old) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("[save_user_avatar] 删除旧头像失败 {:?}: {}", old, e);
                }
            }
        }
    }

    let rel_path = format!("avatar.{}", ext);
    // 写入配置
    {
        let config = state.config.read();
        config
            .set_no_save("base.user_avatar_path", serde_json::json!(rel_path))
            .map_err(|e| e.to_string())?;
        config.save().map_err(|e| e.to_string())?;
    }

    avatar_to_data_url(&dest)
}

/// 读取当前用户头像的 data URL（启动时加载用）
#[tauri::command]
pub async fn get_user_avatar_data_url(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<String>, String> {
    let rel_path: Option<String> = {
        let config = state.config.read();
        config.get_typed("base.user_avatar_path", None)
    };
    let rel = match rel_path {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(None),
    };
    let data_dir = crate::utils::path::get_user_data_dir();
    let path = data_dir.join(&rel);
    avatar_to_data_url(&path)
}

/// 读取已保存的图片文件并返回 data URL
///
/// `image_path` 为相对 `<user_data_dir>` 的路径（如 `images/xxx.png`），
/// 也接受绝对路径。供聊天窗口与记忆面板加载历史图片使用。
#[tauri::command]
pub async fn get_image_data_url(image_path: String) -> Result<Option<String>, String> {
    let p = std::path::Path::new(&image_path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        let data_dir = crate::utils::path::get_user_data_dir();
        data_dir.join(p)
    };
    image_to_data_url(&abs)
}

/// 清除用户自定义头像（删除文件 + 清空配置）
#[tauri::command]
pub async fn clear_user_avatar(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let data_dir = crate::utils::path::get_user_data_dir();
    for ext in &["png", "jpg", "jpeg", "gif", "webp", "bmp"] {
        let p = data_dir.join(format!("avatar.{}", ext));
        if let Err(e) = std::fs::remove_file(&p) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("[clear_user_avatar] 删除头像失败 {:?}: {}", p, e);
            }
        }
    }
    let config = state.config.read();
    config
        .set_no_save("base.user_avatar_path", serde_json::json!(null))
        .map_err(|e| e.to_string())?;
    config.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取设置目录（三层元数据：基础/进阶/专家）
#[tauri::command]
pub fn get_settings_catalog() -> Result<Vec<crate::config::SettingEntry>, String> {
    Ok(crate::config::build_catalog())
}
