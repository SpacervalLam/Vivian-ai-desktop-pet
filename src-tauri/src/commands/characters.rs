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
///     { "id": "nana", "name": "Nana", "online": true, "model_dir": "Vivian" },
///     { "id": "vivian", "name": "Vivian", "online": true, "model_dir": "Vivian" }
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

/// 列出聊天窗口专属角色（chat-only partner）
///
/// 这类角色只出现在聊天窗口：没有桌宠窗口、没有 Brain / 记忆 / 心理状态，
/// 也不参与 `state.characters`。前端聊天窗口把它和 `list_characters` 的结果
/// 合并成一份联系人列表，其余界面（托盘、广播、设置里的角色管理）都看不到它。
#[tauri::command]
pub fn list_chat_partners() -> Result<Value, String> {
    Ok(json!({ "characters": crate::chat_partner::list_json() }))
}

/// 聊天专属角色的模型配置入参
///
/// 只暴露用户在聊天窗口里真正会填的几个字段；其余路由字段（send_* / reasoning 等）
/// 由后端按默认值补齐，避免前端为一条路由维护完整表单。
#[derive(Debug, serde::Deserialize)]
pub struct ChatPartnerModelInput {
    pub provider_type: String,
    pub model: String,
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u64>,
}

/// 读取聊天专属角色的模型配置
///
/// `configured=false` 表示该路由留空，实际请求会沿 catalog 的 fallback 链退回
/// `companion`——这时 `fallback_model` 给出它当前实际使用的模型，供界面提示。
#[tauri::command]
pub fn get_chat_partner_model(
    state: State<'_, Arc<AppState>>,
    partner_id: String,
) -> Result<Value, String> {
    crate::chat_partner::get(&partner_id)
        .ok_or_else(|| format!("聊天角色未找到: {}", partner_id))?;
    let config = state.config.read();
    let route=crate::sns_runtime::store().contact(&partner_id)?.model.unwrap_or_else(||json!({}));
    let route = route.as_object().cloned().unwrap_or_default();
    let model = route.get("model").and_then(Value::as_str).unwrap_or("").trim();
    let endpoint = route.get("endpoint").and_then(Value::as_str).unwrap_or("").trim();
    let configured = !model.is_empty() && !endpoint.is_empty();
    let fallback = config.get("routing_matrix.companion");
    Ok(json!({
        "route": crate::chat_partner::MODEL_ROUTE,
        "configured": configured,
        "model": route,
        "fallback_route": "companion",
        "fallback_model": fallback,
    }))
}

/// 保存聊天专属角色的模型配置
///
/// `model=None` 表示清空该路由、恢复跟随 fallback（companion）。
/// 保存后**只热重建 ModelRouter**，不跑 `reinitialize`：该路由只有聊天专属角色
/// 使用，两位桌宠的 Brain 仍持有各自的旧 router 实例，不受影响。
#[tauri::command]
pub async fn set_chat_partner_model(
    window: tauri::WebviewWindow,
    app: AppHandle,
    partner_id: String,
    model: Option<ChatPartnerModelInput>,
) -> Result<Value, String> {
    if window.label() != "chat" { return Err("请在聊天窗口修改好友".into()); }
    let _guard=crate::commands::sns::MUTATE.lock().await;
    crate::chat_partner::get(&partner_id)
        .ok_or_else(|| format!("聊天角色未找到: {}", partner_id))?;

    let route_value = match model {
        Some(input) => json!({
            "provider_type": input.provider_type,
            "model": input.model,
            "api_key": input.api_key,
            "endpoint": input.endpoint,
            "temperature": input.temperature,
            "send_temperature": true,
            "send_max_tokens": true,
            "max_tokens": input.max_tokens,
            "context_window": input.context_window,
        }),
        // 留空即可让 provider_configuration_complete 判为未配置 → 走 fallback 链
        None => json!({
            "provider_type": "",
            "model": "",
            "api_key": "",
            "endpoint": "",
            "temperature": null,
            "send_temperature": null,
            "send_max_tokens": null,
            "max_tokens": null,
            "context_window": null,
        }),
    };

    let mut profile=crate::sns_runtime::store().contact(&partner_id)?;
    profile.model=if model_cleared(&route_value){None}else{Some(route_value)};
    let saved=crate::sns_runtime::store().save_contact(Some(&partner_id),crate::sns_store::ContactInput{name:profile.name,persona:profile.persona,model:profile.model,avatar:profile.avatar})?;
    crate::chat_partner::register(saved);
    let _=app.emit("sns:changed",json!({"contact_id":partner_id}));
    Ok(json!({"character_id":partner_id,"saved":true}))
}

/// 该路由配置是否已被清空（用于日志）
fn model_cleared(route: &Value) -> bool {
    route.get("model").and_then(Value::as_str).unwrap_or("").trim().is_empty()
}

