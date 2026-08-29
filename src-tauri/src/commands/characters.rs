//! 角色管理命令 —— 多角色架构下的角色列表、在线/离线切换、活跃角色选择
//!
//! 每个角色对应一个独立的窗口和 Brain 实例。前端通过这些命令管理角色状态，
//! 并通过 `character:online_changed` / `character:active_changed` 事件感知变化。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 列出所有角色及其在线状态
///
/// 返回格式：
/// ```json
/// {
///   "active_id": "nana",
///   "characters": [
///     { "id": "nana", "name": "Nana", "online": true, "live2d_model": "Vivian" },
///     { "id": "vivian", "name": "Vivian", "online": true, "live2d_model": "Vivian" }
///   ]
/// }
/// ```
#[tauri::command]
pub fn list_characters(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let chars = state.characters.read();
    let active_id = state.active_character_id.read().clone();

    let list: Vec<Value> = chars
        .values()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "online": *c.online.read(),
            })
        })
        .collect();

    Ok(json!({
        "active_id": active_id,
        "characters": list,
    }))
}

/// 设置角色在线（显示窗口并启用交互）
///
/// 仅修改运行时状态，不持久化到配置文件。
/// 成功后 emit `character:online_changed` 事件，前端据此创建/显示对应角色的窗口。
#[tauri::command]
pub fn set_character_online(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<Value, String> {
    let instance = {
        let chars = state.characters.read();
        chars
            .get(&character_id)
            .cloned()
            .ok_or_else(|| format!("角色未找到: {}", character_id))?
    };

    let was_online = *instance.online.read();
    if was_online {
        return Ok(json!({ "character_id": character_id, "online": true, "changed": false }));
    }

    *instance.online.write() = true;

    // 启动 PetController（空闲定时器等需在 tokio runtime 内）
    let pc = instance.pet_controller.clone();
    tauri::async_runtime::spawn(async move {
        pc.start();
    });

    let _ = app.emit(
        "character:online_changed",
        json!({ "character_id": character_id, "online": true }),
    );

    tracing::info!("[characters] 角色 {} 已上线", character_id);
    Ok(json!({ "character_id": character_id, "online": true, "changed": true }))
}

/// 设置角色离线（隐藏窗口并暂停交互）
///
/// 仅修改运行时状态，不持久化到配置文件。
/// 成功后 emit `character:online_changed` 事件，前端据此隐藏对应角色的窗口。
#[tauri::command]
pub fn set_character_offline(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<Value, String> {
    let instance = {
        let chars = state.characters.read();
        chars
            .get(&character_id)
            .cloned()
            .ok_or_else(|| format!("角色未找到: {}", character_id))?
    };

    let was_online = *instance.online.read();
    if !was_online {
        return Ok(json!({ "character_id": character_id, "online": false, "changed": false }));
    }

    *instance.online.write() = false;

    // 停止 PetController
    instance.pet_controller.stop();

    // 若离线的是当前活跃角色，切换活跃角色到下一个在线角色
    {
        let active_id = state.active_character_id.read().clone();
        if active_id == character_id {
            let next = state
                .characters
                .read()
                .values()
                .find(|c| c.id != character_id && *c.online.read())
                .map(|c| c.id.clone());
            if let Some(next_id) = next {
                *state.active_character_id.write() = next_id.clone();
                let _ = app.emit(
                    "character:active_changed",
                    json!({ "character_id": next_id }),
                );
            }
        }
    }

    let _ = app.emit(
        "character:online_changed",
        json!({ "character_id": character_id, "online": false }),
    );

    tracing::info!("[characters] 角色 {} 已离线", character_id);
    Ok(json!({ "character_id": character_id, "online": false, "changed": true }))
}

/// 设置当前活跃角色（点击选中的对话目标）
///
/// 活跃角色是默认的对话接收方（无 character_id 参数的命令会路由到此角色）。
/// 成功后 emit `character:active_changed` 事件。
#[tauri::command]
pub fn set_active_character(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<Value, String> {
    {
        let chars = state.characters.read();
        if !chars.contains_key(&character_id) {
            return Err(format!("角色未找到: {}", character_id));
        }
    }

    let old = state.active_character_id.read().clone();
    if old == character_id {
        return Ok(json!({ "character_id": character_id, "changed": false }));
    }

    *state.active_character_id.write() = character_id.clone();

    let _ = app.emit(
        "character:active_changed",
        json!({ "character_id": character_id }),
    );

    tracing::info!("[characters] 活跃角色切换: {} -> {}", old, character_id);
    Ok(json!({ "character_id": character_id, "changed": true }))
}

/// 获取当前活跃角色 ID
#[tauri::command]
pub fn get_active_character(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let id = state.active_character_id.read().clone();
    Ok(json!({ "character_id": id }))
}

/// 获取指定角色的 Live2D 模型路径
///
/// 供前端通过 `convertFileSrc` 转为 asset 协议 URL 后加载模型。
/// `character_id` 为 None 时使用活跃角色。
#[tauri::command]
pub fn get_character_model_path(
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
            rl.get_preset("model").map(|p| format!("{}.model3.json", p.name))
        })
        .ok_or_else(|| "未找到模型文件".to_string())?;

    let model_path = rl.model_dir().join(&model_file);
    if model_path.exists() {
        Ok(model_path.to_string_lossy().to_string())
    } else {
        Err(format!("模型文件不存在: {}", model_path.display()))
    }
}
