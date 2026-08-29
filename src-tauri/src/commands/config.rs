//! 配置命令 - 读取、修改、保存与重载配置

use std::path::PathBuf;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::config::manager::WorkModelProfile;
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

/// 前端上报当前生效的界面主题（light/dark；设置"跟随系统"时按系统深浅偏好解析）
///
/// 主窗口在启动、主题配置变化、系统深浅偏好变化时调用。
/// 日出/日落提醒在建议切换主题前用它核对当前实际主题，
/// 避免出现"本来就用浅色，还让用户改成浅色"的无效建议。
#[tauri::command]
pub fn report_effective_theme(theme: String) -> Result<(), String> {
    crate::proactive::set_effective_theme(&theme);
    Ok(())
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
    drop(config);
    // 配置已变更，热重载全局 HTTP 客户端的代理设置（影响天气/Edge TTS/URL 抓取等境外请求）
    let pc = crate::network::proxy::ProxyConfig::from_app_config(
        &state.config.read().get_all(),
    );
    crate::network::http_client::reload_global_client(pc);
    // 工具开关可能已变更：同步用户禁用的工具集合到 ToolSystem（即时生效，
    // 禁用工具立即从 LLM 工具列表移除并被执行入口拒绝）
    let disabled_tools = state.config.read().get_all().tools.disabled_tools.clone();
    state.tool_system.set_disabled_tools(disabled_tools);
    // 远程访问配置可能已变更（启用开关 / 端口），同步服务器状态。
    // 若应用尚未完成初始化（含种子记忆注入与语料嵌入预加载），先不启动远程 HTTP 服务，
    // 待 lib.rs 在初始化完成后统一调用 sync_remote_server 开放 API。
    if state.is_initialized() {
        crate::remote::sync_remote_server(state.inner().clone());
    }
    // 开机自动启动配置可能已变更，同步到当前用户启动项
    let auto_start = state.config.read().get_all().base.auto_start;
    crate::utils::autostart::set_auto_start(auto_start).map_err(err_str)?;
    Ok(())
}

/// 从磁盘重载配置
#[tauri::command]
pub fn reload_config(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let config = state.config.read();
    config.reload().map_err(err_str)?;
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

// ===== LLM 路由 API 可用性测试（一键检测）=====

/// 单条 LLM 路由的最小测试入参 —— 与 TaskRouteConfig 字段一一对应，
/// 由前端传入当前 UI 中的值（含尚未保存的修改），保证测到的就是用户看到的。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRouteTestParams {
    /// 服务商类型（openai / anthropic / gemini / wenxin / spark / custom ...）
    pub provider_type: String,
    /// 模型名称
    pub model: String,
    /// API Key
    pub api_key: String,
    /// 接口端点
    pub endpoint: String,
    /// API Secret（文心 / 讯飞等 OAuth/HMAC 鉴权服务商）
    #[serde(default)]
    pub api_secret: String,
    /// 应用 ID（讯飞星火）
    #[serde(default)]
    pub app_id: String,
}

/// LLM 路由 API 可用性测试结果
#[derive(Debug, Clone, Serialize)]
pub struct LlmRouteTestResult {
    /// 是否可用（收到合法响应）
    pub success: bool,
    /// 端到端耗时（毫秒）
    pub elapsed_ms: u64,
    /// 失败原因（失败时）
    pub error: Option<String>,
    /// 模型回复预览（成功时，截取前 64 字符）
    pub reply: Option<String>,
}

/// 测试单条 LLM 路由的 API 可用性
///
/// 按传入的路由参数构建临时"裸" provider（无 system instructions），发送一条
/// 最小对话请求（"ping"，temperature=0、max_tokens=16），验证端点可达、鉴权有效、
/// 模型存在。与运行时共用 create_probe_provider 的协议分发与代理分流链路。
#[tauri::command]
pub async fn test_llm_route(
    state: State<'_, Arc<AppState>>,
    params: LlmRouteTestParams,
) -> Result<LlmRouteTestResult, String> {
    use crate::providers::factory::{create_probe_provider, ClientCache};
    use crate::types::response::ChatMessage;

    let route = crate::config::manager::TaskRouteConfig {
        provider_type: params.provider_type,
        model: params.model,
        api_key: params.api_key,
        endpoint: params.endpoint,
        api_secret: params.api_secret,
        app_id: params.app_id,
        temperature: Some(0.0),
        max_tokens: Some(16),
        context_window: None,
        reasoning: None,
    };

    // 配置读锁不能跨 await：先取快照再异步执行
    let app_config = {
        let config = state.config.read();
        config.get_all()
    };

    // 探测用独立客户端缓存（命令即用即弃，不与运行时路由共享连接池）
    let provider = create_probe_provider(&route, &app_config, &ClientCache::default())
        .map_err(err_str)?;

    let start = std::time::Instant::now();
    let reply = provider.call_chat(vec![ChatMessage::user("ping")]).await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    match reply {
        Ok(text) => Ok(LlmRouteTestResult {
            success: true,
            elapsed_ms,
            error: None,
            reply: Some(text.chars().take(64).collect()),
        }),
        Err(e) => Ok(LlmRouteTestResult {
            success: false,
            elapsed_ms,
            error: Some(e.to_string()),
            reply: None,
        }),
    }
}

// ===== Token 用量统计 =====

/// 查询近 N 天的 token 用量报表（按天汇总 + 按模型细分，含缓存命中统计）
#[tauri::command]
pub fn get_token_usage(days: u32) -> crate::providers::usage_store::UsageReport {
    crate::providers::usage_store::get_usage_report(days)
}

/// 清空本地 token 用量记录
#[tauri::command]
pub fn clear_token_usage() {
    crate::providers::usage_store::clear();
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

/// 通过文件扩展名检测音频 MIME 类型
fn detect_audio_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("webm") => "audio/webm",
        Some("ogg") => "audio/ogg",
        Some("mp4") | Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        _ => "audio/webm",
    }
}

/// 读取已保存的音频文件并返回 data URL
///
/// `audio_path` 为相对 `<user_data_dir>` 的路径（如 `audio/xxx.webm`），
/// 也接受绝对路径。供聊天窗口加载历史语音消息播放使用。
#[tauri::command]
pub async fn get_audio_data_url(audio_path: String) -> Result<Option<String>, String> {
    let p = std::path::Path::new(&audio_path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        let data_dir = crate::utils::path::get_user_data_dir();
        data_dir.join(p)
    };
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取音频文件失败: {}", e)),
    };
    let mime = detect_audio_mime(&abs);
    let b64 = STANDARD.encode(&bytes);
    Ok(Some(format!("data:{};base64,{}", mime, b64)))
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

// ===== 工作智能体模型预置 =====

/// 工作智能体模型列表 + 当前选中 id（供编程页列表切换加载）
#[tauri::command]
pub fn get_work_models(state: State<'_, Arc<AppState>>) -> Result<crate::config::WorkModelsInfo, String> {
    let cfg = state.config.read().get_all();
    Ok(crate::config::WorkModelsInfo {
        models: cfg.work_models.clone(),
        active_id: cfg.active_work_model.clone(),
    })
}

/// 切换工作智能体模型（reasoning 运行时热切换）
///
/// 按 `model_id` 在预置列表中查找配置，构建 provider 覆盖 ModelRouter 的 reasoning 任务，
/// 并持久化选中 id 保证重启后保持。
#[tauri::command]
pub fn select_work_model(
    state: State<'_, Arc<AppState>>,
    model_id: String,
) -> Result<(), String> {
    let profile: WorkModelProfile = {
        let cfg = state.config.read().get_all();
        cfg.work_models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| format!("未找到工作智能体模型: {}", model_id))?
    };
    let router = state
        .model_router
        .read()
        .clone()
        .ok_or_else(|| "ModelRouter 未初始化".to_string())?;
    let app_config = state.config.read().get_all();
    router
        .set_work_model_override(&profile.route, &app_config)
        .map_err(err_str)?;
    let config = state.config.read();
    config
        .set_no_save("active_work_model", json!(model_id))
        .map_err(err_str)?;
    config.save().map_err(err_str)?;
    Ok(())
}

/// 清除工作智能体模型覆盖（恢复默认路由）并持久化
#[tauri::command]
pub fn clear_work_model(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(router) = state.model_router.read().clone() {
        router.set_reasoning_override(None);
    }
    let config = state.config.read();
    config
        .set_no_save("active_work_model", json!(null))
        .map_err(err_str)?;
    config.save().map_err(err_str)?;
    Ok(())
}
