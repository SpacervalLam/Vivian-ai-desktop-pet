//! 引擎命令 - 桌宠 动作、表情与模型信息
//!
//! 所有命令通过 `AppState.pet_controller` 调用真实的引擎管理器逻辑，
//! 并向前端 emit 对应事件以便 桌宠 渲染层同步状态。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

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

    // 通知前端播放动作（桌宠 渲染层监听此事件驱动模型动画）
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
/// model_dir 路径，直接加载 model_manifest.json。确保角色初始化失败
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

    // 角色未注册（初始化失败被跳过）→ 从配置读取路径，经动作词汇表清单读取 display_scale
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    tracing::warn!(
        "[get_display_scale] 角色 {} 未注册到 state，从配置 model_dir 加载动作清单",
        char_id
    );

    let config = state.config.read().get_all();
    let entry = config
        .characters
        .list
        .iter()
        .find(|e| e.id == char_id)
        .ok_or_else(|| format!("配置中找不到角色: {}", char_id))?;

    let model_dir = crate::utils::path::get_resource_dir().join(&entry.model_dir);
    let scale = crate::engine::manifest::ModelManifest::load_from_dir(&model_dir)
        .map(|mf| mf.display_scale)
        .unwrap_or(1.0);
    Ok(json!({ "display_scale": scale }))
}


/// 获取角色头像的加载 URL
///
/// 头像美术资源现置于普通 public 静态目录 `public/icons/charaters/<character_id>.webp`
/// （与 chibi 图集、provider logo 同机制），由 vite 的 copy-public-assets 随前端静态
/// 资源发布（KEEP 列表含 `icons` 整目录，递归拷贝）。dev / release 均走同源绝对路径
/// `/icons/charaters/<character_id>.webp`，不依赖 model.localhost 加密协议：
/// - 开发态：Vite dev server 直接服务 `public/` 下该文件
/// - 生产态：dist 同源静态服务（与 `/chibi/...` 图集加载方式一致）
/// 文件名使用角色 id（小写，如 `nana` / `vivian`），与 `public/icons/charaters/` 资源对应。
#[tauri::command]
pub fn get_avatar_url(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    // 校验角色存在，保持原有错误语义（配置找不到角色时返回明确错误）
    state
        .config
        .read()
        .get_all()
        .characters
        .list
        .iter()
        .find(|e| e.id == char_id)
        .ok_or_else(|| format!("配置中找不到角色: {}", char_id))?;
    Ok(avatar_icon_url(&char_id))
}

/// 拼接角色头像的同源绝对路径 URL（dev / release 一致：`/icons/charaters/<id>.webp`）
fn avatar_icon_url(character_id: &str) -> String {
    format!("/icons/charaters/{}.webp", character_id)
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
/// 取出并清空队列，再根据 kind/target/params 驱动 桌宠 渲染层。
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

/// 列出所有可用的角色
///
/// 角色清单来自 chibi 动作词汇表（`src/chibi/animations.json`，构建期嵌入）的
/// `characters` 声明，与前端渲染器共用同一份来源。
#[tauri::command]
pub fn list_available_models() -> Result<Vec<Value>, String> {
    let Some(vocab) = crate::engine::pre_parsed::chibi_animations_json() else {
        return Ok(Vec::new());
    };
    let root: serde_json::Value = serde_json::from_str(vocab)
        .map_err(|e| format!("解析 chibi 动作词汇表失败: {}", e))?;

    let models = root
        .get("characters")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.as_str())
                .map(|id| json!({ "id": id, "display_name": id }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(models)
}

/// 获取当前使用的 角色模型 ID
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
