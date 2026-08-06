//! Live2D 嘴形联动命令
//!
//! 由于 Rust/Tauri 版本的 Live2D 渲染在前端（PixiJS + pixi-live2d-display），
//! 后端只负责：
//! 1. 维护嘴形联动状态（idle / speaking / manual）
//! 2. 通过事件 `lipsync:start` / `lipsync:update` / `lipsync:stop` 通知前端
//! 3. 提供手动更新嘴形参数的命令（供未来音素级 lipsync 扩展）
//!
//! 与 `commands/tts.rs` 协作：`speak_text` 调用时自动触发 `start_lipsync`，
//! `stop_speaking` 调用时自动触发 `stop_lipsync`。

use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

/// 嘴形联动状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LipsyncState {
    /// 空闲（嘴巴闭合，呼吸节奏叠加微偏移）
    Idle,
    /// TTS 朗读中（嘴巴随语音开合）
    Speaking,
    /// 手动控制（前端直接驱动）
    Manual,
}

impl Default for LipsyncState {
    fn default() -> Self {
        Self::Idle
    }
}

/// 嘴形联动运行时状态
#[derive(Default)]
pub struct LipsyncRuntime {
    state: RwLock<LipsyncState>,
    /// 当前嘴形开合度 [0.0, 1.0]
    current_open: RwLock<f64>,
    /// 目标嘴形开合度
    target_open: RwLock<f64>,
    /// 当前音素（供未来 viseme 扩展）
    current_phoneme: RwLock<Option<String>>,
}

impl LipsyncRuntime {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(LipsyncState::Idle),
            current_open: RwLock::new(0.0),
            target_open: RwLock::new(0.0),
            current_phoneme: RwLock::new(None),
        }
    }

    pub fn state(&self) -> LipsyncState {
        *self.state.read()
    }

    pub fn set_state(&self, new_state: LipsyncState) {
        *self.state.write() = new_state;
    }

    pub fn current_open(&self) -> f64 {
        *self.current_open.read()
    }

    pub fn target_open(&self) -> f64 {
        *self.target_open.read()
    }

    pub fn set_target_open(&self, open: f64) {
        *self.target_open.write() = open.clamp(0.0, 1.0);
    }

    pub fn set_current_open(&self, open: f64) {
        *self.current_open.write() = open.clamp(0.0, 1.0);
    }

    pub fn phoneme(&self) -> Option<String> {
        self.current_phoneme.read().clone()
    }

    pub fn set_phoneme(&self, phoneme: Option<String>) {
        *self.current_phoneme.write() = phoneme;
    }
}

/// 朗读开始时触发嘴形联动
///
/// 前端监听 `lipsync:start` 事件后，将 Live2D 模型的 `ParamMouthOpenY` 切换到
/// 朗读模式（target=0.25）。
#[tauri::command]
pub fn start_lipsync(
    app: AppHandle,
    state: State<'_, Arc<LipsyncRuntime>>,
    text: Option<String>,
) -> Result<(), String> {
    state.set_state(LipsyncState::Speaking);
    state.set_target_open(0.25);
    state.set_phoneme(None);

    let _ = app.emit(
        "lipsync:start",
        json!({
            "text": text.unwrap_or_default(),
            "target_open": state.target_open(),
        }),
    );
    tracing::debug!("[lipsync] start: state=Speaking, target_open=0.25");
    Ok(())
}

/// 朗读过程中实时更新嘴形
///
/// `open_amount` 为 [0.0, 1.0] 的开合度；`viseme` 为可选的音素标识符。
/// 前端监听 `lipsync:update` 事件并应用到 Live2D 模型。
#[tauri::command]
pub fn update_mouth_shape(
    app: AppHandle,
    state: State<'_, Arc<LipsyncRuntime>>,
    open_amount: f64,
    viseme: Option<String>,
) -> Result<(), String> {
    let open = open_amount.clamp(0.0, 1.0);
    state.set_state(LipsyncState::Manual);
    state.set_target_open(open);
    state.set_current_open(open);
    state.set_phoneme(viseme.clone());

    let _ = app.emit(
        "lipsync:update",
        json!({
            "open": open,
            "viseme": viseme,
        }),
    );
    Ok(())
}

/// 朗读结束时停止嘴形联动
///
/// 前端监听 `lipsync:stop` 事件后，将 `ParamMouthOpenY` 回退到呼吸节奏。
#[tauri::command]
pub fn stop_lipsync(
    app: AppHandle,
    state: State<'_, Arc<LipsyncRuntime>>,
) -> Result<(), String> {
    state.set_state(LipsyncState::Idle);
    state.set_target_open(0.0);
    state.set_phoneme(None);

    let _ = app.emit(
        "lipsync:stop",
        json!({
            "target_open": state.target_open(),
        }),
    );
    tracing::debug!("[lipsync] stop: state=Idle");
    Ok(())
}

/// 获取当前嘴形联动状态
#[tauri::command]
pub fn get_lipsync_status(state: State<'_, Arc<LipsyncRuntime>>) -> Result<Value, String> {
    let s = state.state();
    let state_str = match s {
        LipsyncState::Idle => "idle",
        LipsyncState::Speaking => "speaking",
        LipsyncState::Manual => "manual",
    };
    Ok(json!({
        "state": state_str,
        "current_open": state.current_open(),
        "target_open": state.target_open(),
        "phoneme": state.phoneme(),
    }))
}
