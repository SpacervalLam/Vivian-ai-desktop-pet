//! 引擎命令 - Live2D 动作、表情与模型信息
//!
//! 所有命令通过 `AppState.pet_controller` 调用真实的引擎管理器逻辑，
//! 并向前端 emit 对应事件以便 Live2D 渲染层同步状态。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::engine::pre_parsed::{get_embedded_manifest_json, list_embedded_manifest_ids};
use crate::state::AppState;
use crate::tools::builtin::pet_tools::drain_pending_actions;

/// 播放动作
///
/// 通过 PetController 调用 AnimationManager 播放指定动作，
/// 并向前端 emit `engine:play_motion` 事件。
#[tauri::command]
pub fn play_motion(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    motion: String,
    character_id: Option<String>,
) -> Result<(), String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let pc = state.get_character(character_id.as_deref())?.pet_controller;
    // 默认优先级 NORMAL(50)、可打断、非循环
    let result = pc.play_motion(&motion, 50, true, false);
    if !result.success {
        return Err(result.message);
    }

    // 通知前端播放动作（Live2D 渲染层监听此事件驱动模型动画）
    let _ = app.emit(
        "engine:play_motion",
        json!({
            "motion": &motion,
            "priority": 50,
            "character_id": &char_id,
        }),
    );
    Ok(())
}

/// 设置表情
///
/// 通过 PetController 调用 ExpressionManager 设置表情，
/// 并向前端 emit `engine:set_expression` 事件。
#[tauri::command]
pub fn set_expression(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    expression: String,
    duration_ms: Option<u32>,
    character_id: Option<String>,
) -> Result<(), String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let pc = state.get_character(character_id.as_deref())?.pet_controller;
    // duration_ms: None 表示永久，u32 -> u64 转换供 engine 使用
    let duration = duration_ms.map(|v| v as u64);
    let result = pc.set_expression(&expression, duration, false);
    if !result.success {
        return Err(result.message);
    }

    let _ = app.emit(
        "engine:set_expression",
        json!({
            "expression": &expression,
            "duration_ms": duration_ms,
            "character_id": &char_id,
        }),
    );
    Ok(())
}

/// 获取模型信息
///
/// 返回当前模型的动作列表、表情列表、模型路径等（来自 ResourceLoader）。
#[tauri::command]
pub fn get_model_info(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let pc = state.get_character(character_id.as_deref())?.pet_controller;
    let info = pc.get_model_info();
    Ok(info)
}

/// 获取模型的显示缩放系数（用于补偿模型画布留白）
///
/// 返回 { display_scale: f64 }，默认 1.0。
/// 留白较多的模型可设 > 1.0（如 Nana 设 1.3），使角色视觉大小与其他模型对齐。
///
/// 容错：若角色未注册到 state（初始化失败被跳过），从配置中读取该角色的
/// live2d_model 路径，直接加载 model_manifest.json。确保角色初始化失败
/// 不会导致 display_scale 回退到 1.0（Nana 初始化失败但缩放仍应生效）。
#[tauri::command]
pub fn get_display_scale(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    // 优先从已注册的角色实例读取
    if let Ok(instance) = state.get_character(character_id.as_deref()) {
        let scale = instance
            .manifest
            .model_manifest()
            .map(|mf| mf.display_scale)
            .unwrap_or(1.0);
        return Ok(json!({ "display_scale": scale }));
    }

    // 角色未注册（初始化失败被跳过）→ 从嵌入数据读取 display_scale
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    tracing::warn!(
        "[get_display_scale] 角色 {} 未注册到 state，尝试从嵌入数据读取 manifest",
        char_id
    );

    // 优先从嵌入数据获取
    if let Some(embedded_json) = get_embedded_manifest_json(&char_id) {
        let scale = serde_json::from_str::<serde_json::Value>(embedded_json)
            .ok()
            .and_then(|v| v.get("display_scale").and_then(|d| d.as_f64()))
            .unwrap_or(1.0);
        return Ok(json!({ "display_scale": scale }));
    }

    // 回退：从配置读取路径，再直接读取文件
    let config = state.config.read().get_all();
    let entry = config
        .characters
        .list
        .iter()
        .find(|e| e.id == char_id)
        .ok_or_else(|| format!("配置中找不到角色: {}", char_id))?;

    let model_dir = crate::utils::path::get_resource_dir().join(&entry.live2d_model);
    let scale = crate::engine::manifest::ModelManifest::load_from_dir(&model_dir)
        .map(|mf| mf.display_scale)
        .unwrap_or(1.0);
    Ok(json!({ "display_scale": scale }))
}

/// 获取 Live2D 模型文件的绝对路径
///
/// 供前端通过 `convertFileSrc` 转为 asset 协议 URL 后加载模型。
/// 模型文件名优先从 model_manifest.json 读取，降级到 ResourceLoader 扫描结果。
#[tauri::command]
pub fn get_model_path(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let rl = instance
        .pet_controller
        .resource_loader()
        .ok_or_else(|| "ResourceLoader未初始化".to_string())?;

    // 优先从该角色的 manifest 读取 model_file
    let model_file = instance
        .manifest
        .model_manifest()
        .map(|mf| mf.model_file.clone())
        .or_else(|| {
            // 降级：从 ResourceLoader 扫描结果获取 model3.json 文件名
            rl.get_preset("model").map(|p| format!("{}.model3.json", p.name))
        })
        .ok_or_else(|| "未找到模型文件".to_string())?;

    let model_path = rl.model_dir().join(&model_file);
    if model_path.exists() {
        Ok(model_path.to_string_lossy().to_string())
    } else {
        Err(format!(
            "模型文件不存在: {}",
            model_path.display()
        ))
    }
}

/// 获取 Live2D 模型文件的加载 URL
///
/// 开发模式（tauri dev，debug 编译）：返回 Vite dev server 可直接加载的相对 URL
/// （如 `/Vivian/Vivian.model3.json`），因为 `public/` 下的文件被 Vite 以根路径提供。
/// 生产模式（tauri build / 运行 exe，release 编译）：返回自定义 model 协议 URL
/// （如 `model://localhost/Vivian/Vivian.model3.json`），由嵌入二进制的加密资源解密提供。
///
/// 相比 `get_model_path` 返回绝对路径再由前端转换，此命令直接返回可用 URL，
/// 消除前端路径前缀剥离（正则匹配 `public/`）的脆弱性，统一在路径解析逻辑所在的后端处理。
#[tauri::command]
pub fn get_model_url(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    // 角色未注册（窗口先于初始化创建 / 初始化失败被跳过）→ 按模式推导 URL。
    // 不用 "{目录名}.model3.json" 命名约定：文件名大小写不保证与目录一致
    // （如 Nana/ 下是 nana.model3.json），Vite/URL 路径大小写敏感会 404
    let instance = match state.get_character(character_id.as_deref()) {
        Ok(i) => i,
        Err(_) => {
            let char_id = character_id
                .clone()
                .unwrap_or_else(|| state.active_character_id.read().clone());
            let config = state.config.read().get_all();
            let entry = config
                .characters
                .list
                .iter()
                .find(|e| e.id == char_id)
                .ok_or_else(|| format!("配置中找不到角色: {}", char_id))?;
            // dev：扫描磁盘目录取实际文件名；release：从加密 bundle 的编译期
            // 索引查找（资源不存在文件系统上，全在 vivian.bundle.enc 内存中）
            let relative_str = if cfg!(debug_assertions) {
                let model_dir =
                    crate::utils::path::get_resource_dir().join(&entry.live2d_model);
                let model_file = std::fs::read_dir(&model_dir)
                    .map_err(|e| format!("读取模型目录失败 {}: {}", model_dir.display(), e))?
                    .filter_map(|e| e.ok())
                    .find(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .to_lowercase()
                            .ends_with(".model3.json")
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .ok_or_else(|| {
                        format!("模型目录 {} 下未找到 *.model3.json", model_dir.display())
                    })?;
                format!("{}/{}", entry.live2d_model, model_file)
            } else {
                crate::bundle_reader::list_assets_by_prefix(&entry.live2d_model)
                    .into_iter()
                    .find(|p| p.to_lowercase().ends_with(".model3.json"))
                    .ok_or_else(|| {
                        format!("加密 bundle 中未找到 {}/*.model3.json", entry.live2d_model)
                    })?
                    .to_string()
            };
            let url = if cfg!(debug_assertions) {
                format!("/{}", relative_str)
            } else {
                format!("http://model.localhost/{}", relative_str)
            };
            tracing::info!(
                "[get_model_url] 角色 {} 未注册（初始化中或被跳过），推导 url={}",
                char_id,
                url
            );
            return Ok(url);
        }
    };
    let rl = instance
        .pet_controller
        .resource_loader()
        .ok_or_else(|| "ResourceLoader未初始化".to_string())?;

    let model_file = instance
        .manifest
        .model_manifest()
        .map(|mf| mf.model_file.clone())
        .or_else(|| {
            rl.get_preset("model").map(|p| format!("{}.model3.json", p.name))
        })
        .ok_or_else(|| "未找到模型文件".to_string())?;

    let model_path = rl.model_dir().join(&model_file);

    // 计算相对于资源根目录的路径（如 Vivian/Vivian.model3.json）
    let base_dir = rl.base_dir();
    let relative = model_path.strip_prefix(base_dir).map_err(|_| {
        format!(
            "无法计算模型相对路径: {} (base: {})",
            model_path.display(),
            base_dir.display()
        )
    })?;

    // 统一为正斜杠（URL 路径分隔符）
    let relative_str = relative.to_string_lossy().replace('\\', "/");

    let url = if cfg!(debug_assertions) {
        // 开发模式：Vite dev server 从 public/ 根目录提供静态资源
        format!("/{}", relative_str)
    } else {
        // 生产模式：通过自定义 model 协议加载嵌入的加密资源
        // Tauri 2 在 Windows 上自定义 protocol URL 格式为 http://<scheme>.localhost/<path>
        format!("http://model.localhost/{}", relative_str)
    };

    tracing::info!("[get_model_url] character={:?} url={}", character_id, url);
    Ok(url)
}

/// 获取角色头像 icon.png 的加载 URL
///
/// 头像与 Live2D 模型同在 `public/<live2d_model>/icon.png`，且随模型一起被打包进
/// 加密 bundle（scripts/encrypt-assets.mjs 递归打包 `<模型目录>/`）。构建后 vite
/// 的 strip-encrypted-assets 插件会把 `dist/{Vivian,Nana,world-bg}` 删掉，硬编码
/// 相对路径 `/Nana/icon.png` 在生产环境会 404。因此头像 URL 必须同 get_model_url
/// 一样按模式解析：
/// - 开发模式（debug 编译）：返回 Vite dev server 相对路径（如 `/Nana/icon.png`）
/// - 生产模式（release 编译）：返回 model 协议 URL（如 `http://model.localhost/Nana/icon.png`）
#[tauri::command]
pub fn get_avatar_url(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    // 头像恒为 `icon.png`，所在目录即角色的 live2d_model（配置直接决定，无需扫描目录）
    let live2d_model = state
        .config
        .read()
        .get_all()
        .characters
        .list
        .iter()
        .find(|e| e.id == char_id)
        .map(|e| e.live2d_model.clone())
        .ok_or_else(|| format!("配置中找不到角色: {}", char_id))?;
    Ok(avatar_icon_url(&live2d_model))
}

/// 按构建模式拼接角色头像目录下 icon.png 的 URL
fn avatar_icon_url(live2d_model: &str) -> String {
    if cfg!(debug_assertions) {
        format!("/{}/icon.png", live2d_model)
    } else {
        format!("http://model.localhost/{}/icon.png", live2d_model)
    }
}

/// 触发待机动作
///
/// 调用 StateMachine.trigger_random_idle_action 随机播放一个动作或临时表情。
#[tauri::command]
pub fn trigger_idle_action(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let pc = state.get_character(character_id.as_deref())?.pet_controller;
    let result = pc.trigger_idle_action();
    if !result.success {
        return Err(result.message);
    }
    Ok(())
}

/// 消费工具层投递的桌宠动作队列
///
/// 工具（set_expression / play_motion / speak_bubble / soothe_pet 等）通过
/// `pet_tools::push_action` 把请求塞进 PENDING_ACTIONS 队列。前端定期调用此命令
/// 取出并清空队列，再根据 kind/target/params 驱动 Live2D 渲染层。
///
/// 返回格式：`{ "actions": [ {kind, target, params, timestamp, character_id}, ... ] }`
#[tauri::command]
pub fn drain_pet_actions(
    _state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let actions = drain_pending_actions(character_id.as_deref());
    Ok(json!({ "actions": actions }))
}

/// 尝试生成唤醒问候（从休息/离线状态被唤回时调用）
///
/// 根据当前心理状态计算概率，概率命中时调用 LLM 生成问候语。
/// 生成的问候语会作为 CasualConversation 存入记忆系统。
/// 返回 { greeting: Option<String>, probability: f64, triggered: bool }
#[tauri::command]
pub async fn try_wake_greeting(
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
    character_id: Option<String>,
) -> Result<Value, String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let brain = state.get_character(character_id.as_deref())?.brain;

    // 根据心理参数计算唤醒问候概率
    let probability = brain.psychology.compute_wake_greeting_probability();
    let triggered = rand::random::<f64>() < probability;

    if !triggered {
        return Ok(json!({
            "greeting": null,
            "probability": probability,
            "triggered": false,
        }));
    }

    // 主 LLM API 必须配置，否则发 `llm:not_configured` 通知用户
    let api_configured = state
        .model_router
        .read()
        .as_ref()
        .map_or(false, |r| r.has_main_provider());
    if !api_configured {
        let _ = app.emit(
            "llm:not_configured",
            json!({ "scene": "wake_greeting", "character_id": &char_id }),
        );
        return Ok(json!({
            "greeting": null,
            "probability": probability,
            "triggered": false,
        }));
    }

    let greeting = brain.generate_wake_greeting().await;

    // 将唤醒问候存入记忆系统（作为 AI 回复）
    if let Some(g) = &greeting {
        let meta = serde_json::json!({
            "channel": "proactive",
            "speaker": char_id,
            "listener": "user",
            "perspective": "speaker",
            "knowledge_source": "direct",
        });
        let _ = brain
            .memory
            .add_memory_with_metadata(
                &format!(
                    "{} {}",
                    crate::cross_character::build_speaker_prefix(&char_id, "user", &char_id),
                    g
                ),
                crate::memory::types::MemoryType::CasualConversation,
                0.35,
                vec!["assistant".to_string(), "wake_greeting".to_string(), "dialogue_turn".to_string()],
                meta,
            )
            .await;
    }

    Ok(json!({
        "greeting": greeting,
        "probability": probability,
        "triggered": true,
    }))
}

/// 设置智能躲避鼠标模式
#[tauri::command]
pub fn set_avoid_mouse(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
    character_id: Option<String>,
) -> Result<Value, String> {
    let pc = state.get_character(character_id.as_deref())?.pet_controller;
    let result = pc.set_avoid_mouse(enabled);
    if !result.success {
        return Err(result.message);
    }
    Ok(json!({ "enabled": enabled }))
}

/// 列出所有可用的 Live2D 模型
///
/// 优先使用预解析的嵌入数据，回退到扫描资源目录。
#[tauri::command]
pub fn list_available_models() -> Result<Vec<Value>, String> {
    let mut models = Vec::new();

    // 1. 优先使用嵌入的 manifest 数据
    for char_id in list_embedded_manifest_ids() {
        if let Some(embedded_json) = get_embedded_manifest_json(char_id) {
            let display_name = serde_json::from_str::<serde_json::Value>(embedded_json)
                .ok()
                .and_then(|v| v.get("display_name").and_then(|d| d.as_str()).map(|s| s.to_string()))
                .unwrap_or_else(|| char_id.to_string());
            models.push(json!({
                "id": char_id,
                "display_name": display_name,
                "has_manifest": true,
            }));
        }
    }

    if !models.is_empty() {
        return Ok(models);
    }

    // 2. 回退：扫描资源目录
    let base_dir = crate::utils::path::get_resource_dir();
    let entries = match std::fs::read_dir(&base_dir) {
        Ok(it) => it,
        Err(e) => return Err(format!("读取资源目录失败: {} - {}", base_dir.display(), e)),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = match path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let mut has_model3 = false;
        let mut has_manifest = false;
        if let Ok(sub_entries) = std::fs::read_dir(&path) {
            for sub in sub_entries.flatten() {
                let name = sub.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".model3.json") {
                    has_model3 = true;
                }
                if name_str == "model_manifest.json" {
                    has_manifest = true;
                }
            }
        }

        if has_model3 || has_manifest {
            let display_name = if has_manifest {
                let manifest_path = path.join("model_manifest.json");
                std::fs::read_to_string(&manifest_path)
                    .ok()
                    .and_then(|data| {
                        serde_json::from_str::<serde_json::Value>(&data)
                            .ok()
                            .and_then(|v| v.get("display_name").and_then(|d| d.as_str()).map(|s| s.to_string()))
                    })
                    .unwrap_or_else(|| id.clone())
            } else {
                id.clone()
            };
            models.push(json!({
                "id": id,
                "display_name": display_name,
                "has_manifest": has_manifest,
            }));
        }
    }

    Ok(models)
}

/// 获取当前使用的 Live2D 模型 ID
#[tauri::command]
pub fn get_current_model(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let character = state.get_character(character_id.as_deref())?;
    let display_name = character
        .manifest
        .model_manifest()
        .map(|mf| mf.display_name.clone())
        .unwrap_or_default();
    Ok(json!({
        "display_name": display_name,
    }))
}
