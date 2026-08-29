//! 情感命令 - 心情状态与表情。
//!
//! `get_current_mood` / `get_psychology_state` 从 PsychologyManager（五层心理架构）获取实时状态。
//! `set_emotion_expression` 通过 emit 事件通知前端 Live2D 应用表情。

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::emotion::{EmotionResult, ResponseStrategy};
use crate::state::AppState;
use crate::tools::builtin::pet_tools::{push_action, PetActionRequest};

/// 获取当前心情状态（从五层心理架构实时计算）
///
/// Mood 不存储，由 PsychologyManager.compute_mood() 从 Emotion/Needs/Relationship 实时投影。
/// 返回 valence/arousal/primary_emotion/fatigue/stress/relationship_score 等前端展示字段。
#[tauri::command]
pub fn get_current_mood(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if let Ok(brain) = state.get_character(character_id.as_deref()).map(|c| c.brain) {
        let mood = brain.psychology.compute_mood();
        let mut result = serde_json::to_value(&mood).map_err(|e| e.to_string())?;
        if let Some(obj) = result.as_object_mut() {
            // 添加前端 MoodState 期望的兼容字段
            // energy = 100 - fatigue（疲劳度的反向映射）
            let energy = (100.0 - mood.fatigue).max(0.0).min(100.0);
            obj.insert("energy".to_string(), Value::from(energy));
            // focus（专注力）= 唤醒度映射：高唤醒=警觉专注，低唤醒=疲惫走神
            let focus = (mood.arousal * 100.0).max(0.0).min(100.0);
            obj.insert("focus".to_string(), Value::from(focus));
            obj.insert("intimacy".to_string(), Value::from(mood.relationship_score));
            obj.insert("trust".to_string(), Value::from(mood.relationship_score));
            obj.insert("positive_affect".to_string(), Value::from(((mood.valence + 1.0) * 50.0).max(0.0).min(100.0)));
            obj.insert("negative_affect".to_string(), Value::from(((1.0 - mood.valence) * 50.0).max(0.0).min(100.0)));
            obj.insert("mood_label".to_string(), Value::String(mood.primary_emotion.display_zh().to_string()));
            obj.insert("mood_score".to_string(), Value::from(((mood.valence + 1.0) * 50.0).max(0.0).min(100.0)));
            obj.insert("mood_emotion".to_string(), Value::String(mood.primary_emotion.as_str().to_string()));
            obj.insert("mood_secondary".to_string(), Value::String(mood.secondary_emotion.as_str().to_string()));
            obj.insert("emotion_label".to_string(), Value::String(mood.primary_emotion.display_zh().to_string()));
            obj.insert("emotion_key".to_string(), Value::String(mood.primary_emotion.as_str().to_string()));
        }
        Ok(result)
    } else {
        // Brain 未初始化，返回默认值（包含前端期望的所有字段）
        Ok(serde_json::json!({
            "valence": 0.3,
            "arousal": 0.4,
            "primary_emotion": "curiosity",
            "secondary_emotion": "neutral",
            "primary_intensity": 0.5,
            "emotion_label": "好奇",
            "emotion_key": "curiosity",
            "mood_label": "好奇",
            "mood_score": 65.0,
            "mood_emotion": "curiosity",
            "mood_secondary": "neutral",
            "fatigue": 20.0,
            "stress": 10.0,
            "energy": 80.0,
            "focus": 50.0,
            "intimacy": 20.0,
            "trust": 20.0,
            "relationship_score": 20.0,
            "positive_affect": 65.0,
            "negative_affect": 35.0,
        }))
    }
}

/// 获取完整心理状态（五层架构 + 5 维关系）
///
/// 供前端心理面板展示：Persona / Needs / Emotion / Behavior Drive / Relationship。
#[tauri::command]
pub fn get_psychology_state(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if let Ok(brain) = state.get_character(character_id.as_deref()).map(|c| c.brain) {
        let snapshot = brain.psychology.snapshot();
        Ok(serde_json::to_value(&snapshot).map_err(|e| e.to_string())?)
    } else {
        Ok(serde_json::json!({}))
    }
}

/// 用户交互事件 — 由前端交互检测触发
///
/// 前端检测到快速点击、快速拖动、抚摸等交互后调用此命令。
/// 返回建议的 Live2D 表情/动作，前端直接播放（不等 LLM）。
#[tauri::command]
pub fn apply_user_interaction(
    interaction: String,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if let Ok(instance) = state.get_character(character_id.as_deref()) {
        let char_id = instance.id.clone();
        let manifest = instance.manifest.clone();

        // 检查是否从长时空闲回来（在record_interaction之前检查）
        let idle_secs = crate::engine::AUTO_TRIGGER.idle_seconds(&char_id);
        let was_long_idle = idle_secs > 300; // 5分钟以上视为长时空闲

        let feedback = instance.brain.psychology.apply_user_interaction(&interaction);

        // 记录用户交互到自动触发器（重置空闲计时）
        crate::engine::record_user_interaction(&char_id);

        // 如果从长时空闲回来，触发user_return事件
        if was_long_idle {
            crate::engine::trigger_event(&char_id, "user_return", &manifest);
        }

        Ok(serde_json::to_value(&feedback).map_err(|e| e.to_string())?)
    } else {
        Ok(serde_json::json!({ "expression": "", "motion": "" }))
    }
}

/// 心理微调 tick — 高频调用（每 3-5 秒），让情绪持续波动
///
/// 与 proactive tick（10s）不同，这个只做 Homeostasis + 微噪声波动，
/// 不触发主动行为。让情绪在无交互时也有自然浮动。
///
/// 每次执行后 emit `psychology:state` 事件推送 snapshot + mood，
/// 前端 StatusPanel 监听该事件替代 800ms 轮询。
#[tauri::command]
pub fn psychology_micro_tick(
    character_id: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // 恢复出厂设置进行中：立即跳过
    if state.is_factory_reset_in_progress() {
        return Ok(());
    }
    if let Ok(instance) = state.get_character(character_id.as_deref()) {
        let brain = instance.brain.clone();
        let char_id = instance.id.clone();
        brain.psychology.micro_tick();
        // 推送最新状态给前端（StatusPanel listen 替代轮询）
        // 携带 character_id 字段，让多角色窗口按角色过滤事件
        let snapshot = brain.psychology.snapshot();
        let mood = brain.psychology.compute_mood();
        let energy = (100.0 - mood.fatigue).max(0.0).min(100.0);
        let _ = app.emit_to(
            format!("{}_status", char_id),
            "psychology:state",
            serde_json::json!({
                "character_id": char_id,
                "snapshot": snapshot,
                "mood": {
                    "valence": mood.valence,
                    "arousal": mood.arousal,
                    "primary_emotion": mood.primary_emotion.as_str(),
                    "secondary_emotion": mood.secondary_emotion.as_str(),
                    "primary_intensity": mood.primary_intensity,
                    "fatigue": mood.fatigue,
                    "stress": mood.stress,
                    "energy": energy,
                    "relationship_score": mood.relationship_score,
                },
            }),
        );
    }
    Ok(())
}

/// 心情表情触发 tick — 由前端周期调用（约 25-35s 一次）
///
/// 根据当前心情状态（主导情绪 + 疲劳 + 唤醒度）从 manifest 的 mood_triggers
/// 表情池中随机选取一个表情，经概率门控 + 冷却后通过 push_action 投递给前端。
/// 让桌宠在无交互时也能自发流露情绪，更生动。
///
/// 概率公式：base(0.22) × primary_intensity × (0.6 + arousal × 0.4)，上限 0.5
/// 冷却：MOOD_EXPRESSION_COOLDOWN_SECS（20s），防止表情过于密集
#[tauri::command]
pub fn mood_expression_tick(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    // 恢复出厂设置进行中：立即跳过
    if state.is_factory_reset_in_progress() {
        return Ok(serde_json::json!({ "triggered": false, "reason": "factory_reset_in_progress" }));
    }

    // 每角色独立冷却表（按 char_id 索引），避免多角色共享同一全局触发节流
    static LAST_TRIGGER: Lazy<RwLock<HashMap<String, i64>>> =
        Lazy::new(|| RwLock::new(HashMap::new()));

    let instance = match state.get_character(character_id.as_deref()) {
        Ok(c) => c,
        Err(_) => return Ok(serde_json::json!({ "triggered": false, "reason": "no_brain" })),
    };
    let brain = instance.brain.clone();
    let char_id = instance.id.clone();

    let behavior = crate::character_behavior::get_behavior(&char_id);
    let cooldown_secs = behavior.mood_expression_cooldown_secs;

    let now = chrono::Utc::now().timestamp();
    let last = LAST_TRIGGER.read().get(&char_id).copied().unwrap_or(0);
    if now - last < cooldown_secs {
        return Ok(serde_json::json!({ "triggered": false, "reason": "cooldown" }));
    }

    let mood = brain.psychology.compute_mood();

    // 派生心情标签：疲劳优先，其次低唤醒度的无聊，最后用主导情绪
    let mood_label = if mood.fatigue > 65.0 {
        "tired".to_string()
    } else if mood.arousal < 0.25 && mood.primary_intensity < 0.35 {
        "bored".to_string()
    } else {
        mood.primary_emotion.as_str().to_string()
    };

    // 从 manifest 心情表情池随机选一个表情（model3.json Name）
    let expression = match instance.manifest.random_mood_expression(&mood_label) {
        Some(e) if !e.is_empty() => e,
        _ => {
            return Ok(serde_json::json!({
                "triggered": false,
                "reason": "no_pool",
                "mood": mood_label,
            }));
        }
    };

    // 概率门控：情绪越强、唤醒度越高 → 越可能触发
    let probability = (0.22 * mood.primary_intensity * (0.6 + mood.arousal * 0.4)).min(0.5);
    if rand::random::<f64>() >= probability {
        return Ok(serde_json::json!({
            "triggered": false,
            "reason": "probability",
            "mood": mood_label,
            "probability": probability,
        }));
    }

    // 触发：投递表情动作到前端队列
    LAST_TRIGGER.write().insert(char_id.clone(), now);
    push_action(PetActionRequest {
        kind: "expression".to_string(),
        target: expression.clone(),
        params: serde_json::json!({ "duration_ms": 4000 }),
        timestamp: now,
        character_id: char_id,
    });

    tracing::debug!(
        "[mood_expression_tick] 触发心情表情: mood={} expr={}",
        mood_label,
        expression
    );

    Ok(serde_json::json!({
        "triggered": true,
        "mood": mood_label,
        "expression": expression,
        "probability": probability,
    }))
}

/// 获取心情历史记录
///
/// 从 PsychologyManager 返回近期情绪采样（带时间戳和情绪快照）。
#[tauri::command]
pub fn get_mood_history(
    limit: Option<usize>,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    if let Ok(brain) = state.get_character(character_id.as_deref()).map(|c| c.brain) {
        let snapshot = brain.psychology.snapshot();
        let events: Vec<Value> = snapshot
            .events
            .iter()
            .rev()
            .take(limit.unwrap_or(50))
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();
        Ok(events)
    } else {
        Ok(Vec::new())
    }
}

/// 获取近期重要事件（供 StatusPanel "近期事件"卡片使用）
///
/// 从 MemoryManager 查询 type=ImportantEvent 的最近 N 条记忆（按 timestamp 降序）。
/// 事件由 LLM 在主调用 JSON 中产出 event_summary 后写入，非每轮都记录。
#[tauri::command]
pub async fn get_recent_events(
    limit: Option<usize>,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();
    let items = memory.recent_by_type(
        crate::memory::types::MemoryType::ImportantEvent,
        limit.unwrap_or(5),
    );
    Ok(items
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "timestamp": m.timestamp,
                "summary": m.content,
                "importance": m.importance,
            })
        })
        .collect())
}

/// 设置桌宠表情
///
/// 通过 emit 事件通知前端（Live2D）应用表情变化。
/// 表情变化不再写入 PetStatusManager（Mood 由 PsychologyManager 实时计算，不存储）。
#[tauri::command]
pub fn set_emotion_expression(
    app: AppHandle,
    expression: String,
) -> Result<(), String> {
    if expression.trim().is_empty() {
        return Err("表情名称不能为空".to_string());
    }

    tracing::info!("[set_emotion_expression] 设置表情: {}", expression);

    // 通过 emit 事件通知前端应用 Live2D 表情
    let _ = app.emit(
        "emotion:expression_changed",
        serde_json::json!({
            "expression": &expression,
            "source": "manual",
        }),
    );

    Ok(())
}

/// 深度情感分析 — 调用 brain.emotion_bridge 进行关键词 + LLM 综合分析
///
/// 流程：
/// 1. 从 Brain 获取已注入 PsychologyManager 的 EmotionBridge
/// 2. 调用 `process_emotion(text)` 返回综合结果（会更新心理状态）
/// 3. 从 PsychologyManager 读取当前主导情绪（EmotionLabel）作为 pet_emotion
/// 4. 附带 ResponseStrategy 推荐
///
/// 返回 JSON：
/// ```json
/// {
///   "pipeline": { "emotion", "intensity", "valence", "arousal",
///                 "expression", "source", "pet_emotion", "from_cache" },
///   "strategy": { "strategy", "user_emotion", "pet_emotion",
///                 "prompt_fragment", "adjusted_by_pet" }
/// }
/// ```
#[tauri::command]
pub async fn analyze_emotion_deep(
    text: String,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Err("文本不能为空".to_string());
    }

    let brain = state.get_character(character_id.as_deref())?.brain;
    let (emotion_bridge, psychology) = (brain.emotion_bridge.clone(), brain.psychology.clone());

    let pipeline_result = emotion_bridge.process_emotion(&text).await;

    // 从 PsychologyManager 读取当前主导情绪（EmotionLabel）
    let (pet_emotion_label, _) = psychology.emotion().dominant();
    let emotion_result = EmotionResult {
        emotion: pipeline_result.emotion.clone(),
        intensity: pipeline_result.intensity,
        valence: pipeline_result.valence,
        arousal: pipeline_result.arousal,
        source: pipeline_result.source.clone(),
        ..Default::default()
    };
    let strategy = ResponseStrategy::recommend_detailed(&emotion_result, &pet_emotion_label);

    Ok(serde_json::json!({
        "pipeline": pipeline_result,
        "strategy": strategy,
    }))
}

/// 批量深度情感分析
#[tauri::command]
pub async fn analyze_emotion_batch(
    texts: Vec<String>,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let emotion_bridge = state.get_character(character_id.as_deref())?.brain.emotion_bridge.clone();

    let results = emotion_bridge.process_emotion_batch(texts).await;
    results
        .into_iter()
        .map(|r| serde_json::to_value(&r).map_err(|e| e.to_string()))
        .collect()
}

/// 即时情绪分析 — 低延迟，用于三层反应系统的 Layer 1/2
///
/// 调用 EmotionBridge.classify_instant：
/// - 优先使用嵌入分类器（本地哈希 <1ms，远程嵌入 50-200ms）
/// - 嵌入不可用时降级到关键词分析
/// - 不更新心理状态，不触发表情，不写缓存
///
/// 返回 JSON：
/// ```json
/// {
///   "emotion": "happy",          // 14 类 LLM 情绪标签
///   "intensity": 0.8,            // 0.0 ~ 1.0
///   "valence": 0.7,              // -1.0 ~ 1.0
///   "arousal": 0.6,              // 0.0 ~ 1.0
///   "source": "embedding",       // embedding / embedding_exact / embedding_fallback:* / instant_keyword_fallback
///   "pet_emotion": "joy",        // 7 类 EmotionLabel
///   "facs": { ... }              // FACS 参数（前端直接应用）
/// }
/// ```
#[tauri::command]
pub fn analyze_emotion_instant(
    text: String,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(serde_json::json!({
            "emotion": "neutral",
            "intensity": 0.0,
            "valence": 0.0,
            "arousal": 0.3,
            "source": "empty",
            "pet_emotion": "curiosity",
            "facs": default_facs(),
        }));
    }

    let brain = state.get_character(character_id.as_deref())?.brain;
    let result = brain.emotion_bridge.classify_instant(&text)?;

    // 映射到 7 类 EmotionLabel
    let pet_emotion = crate::emotion::mapper::llm_to_emotion_label(&result.emotion);

    // 生成 FACS 参数（与前端 emotionToFacs 一致的映射）
    let facs = emotion_to_facs_json(&result.emotion, result.intensity);

    Ok(serde_json::json!({
        "emotion": result.emotion,
        "intensity": result.intensity,
        "valence": result.valence,
        "arousal": result.arousal,
        "source": result.source,
        "pet_emotion": pet_emotion.as_str(),
        "facs": facs,
        "confidence": result.confidence,
        "secondary_emotion": result.secondary_emotion,
        "target": result.target,
    }))
}

/// 默认 FACS 参数（neutral 状态）
fn default_facs() -> Value {
    serde_json::json!({
        "browInnerUp": 0.0,
        "browDown": 0.0,
        "eyeSmile": 0.0,
        "eyeSquint": 0.0,
        "eyeOpen": 1.0,
        "mouthSmile": 0.04,
        "mouthFrown": 0.0,
        "mouthOpen": 0.0,
        "cheekPuff": 0.0,
        "blush": 0.0,
        "headZ": 0.0,
        "headY": 0.0,
    })
}

/// 14 类 LLM 情绪 → FACS 参数 JSON（与前端 EmotionFacs.ts 映射一致）
///
/// 将单一情绪标签 + 强度转换为 FACS 通道值，供前端直接写入 Live2D 模型。
fn emotion_to_facs_json(emotion: &str, intensity: f64) -> Value {
    let i = intensity.clamp(0.0, 1.0) as f32;
    let facs = match emotion {
        "happy" | "excited" => FacsParams {
            mouth_smile: 0.42 * i + 0.22 * i * 0.3,
            eye_smile: 0.38 * i,
            cheek_puff: 0.18 * i,
            blush: 0.12 * i,
            ..Default::default()
        },
        "grateful" => FacsParams {
            mouth_smile: 0.22 * i,
            eye_smile: 0.24 * i,
            blush: 0.32 * i,
            head_z: 0.06 * i,
            ..Default::default()
        },
        "sad" | "disappointed" => FacsParams {
            mouth_frown: 0.36 * i,
            brow_inner_up: 0.26 * i,
            eye_squint: 0.12 * i,
            head_y: 0.12 * i,
            ..Default::default()
        },
        "angry" | "frustrated" => FacsParams {
            mouth_frown: 0.20 * i,
            brow_down: 0.34 * i,
            eye_squint: 0.30 * i,
            cheek_puff: 0.14 * i,
            head_z: -0.05 * i,
            ..Default::default()
        },
        "anxious" => FacsParams {
            brow_inner_up: 0.22 * i,
            eye_open: 1.0 + 0.20 * i,
            mouth_open: 0.05 * i,
            ..Default::default()
        },
        "tired" | "bored" => FacsParams {
            eye_squint: 0.4 * i,
            eye_open: 1.0 - 0.3 * i,
            head_y: 0.10 * i,
            ..Default::default()
        },
        "surprised" => FacsParams {
            brow_inner_up: 0.4 * i,
            eye_open: 1.0 + 0.3 * i,
            mouth_open: 0.3 * i,
            ..Default::default()
        },
        "curious" => FacsParams {
            brow_inner_up: 0.10 * i,
            eye_open: 1.0 + 0.10 * i,
            mouth_smile: 0.06 * i,
            head_z: 0.03 * i,
            ..Default::default()
        },
        "confused" => FacsParams {
            brow_inner_up: 0.15 * i,
            brow_down: 0.10 * i,
            head_z: 0.04 * i,
            ..Default::default()
        },
        _ => FacsParams::default(),
    };

    serde_json::json!({
        "browInnerUp": facs.brow_inner_up,
        "browDown": facs.brow_down,
        "eyeSmile": facs.eye_smile,
        "eyeSquint": facs.eye_squint,
        "eyeOpen": facs.eye_open,
        "mouthSmile": facs.mouth_smile,
        "mouthFrown": facs.mouth_frown,
        "mouthOpen": facs.mouth_open,
        "cheekPuff": facs.cheek_puff,
        "blush": facs.blush,
        "headZ": facs.head_z,
        "headY": facs.head_y,
    })
}

struct FacsParams {
    brow_inner_up: f32,
    brow_down: f32,
    eye_smile: f32,
    eye_squint: f32,
    eye_open: f32,
    mouth_smile: f32,
    mouth_frown: f32,
    mouth_open: f32,
    cheek_puff: f32,
    blush: f32,
    head_z: f32,
    head_y: f32,
}

impl Default for FacsParams {
    fn default() -> Self {
        Self {
            brow_inner_up: 0.0,
            brow_down: 0.0,
            eye_smile: 0.0,
            eye_squint: 0.0,
            eye_open: 1.0,
            mouth_smile: 0.04,
            mouth_frown: 0.0,
            mouth_open: 0.0,
            cheek_puff: 0.0,
            blush: 0.0,
            head_z: 0.0,
            head_y: 0.0,
        }
    }
}

/// 自动表情触发 tick — 前端周期调用（约3-5秒一次）
///
/// 检查空闲状态、心情持续表情、程序事件等，自动触发相应表情/动作。
/// 无需LLM参与，纯规则驱动。
#[tauri::command]
pub fn auto_expression_tick(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    // 恢复出厂设置进行中：立即跳过
    if state.is_factory_reset_in_progress() {
        return Ok(serde_json::json!({ "ticked": false, "reason": "factory_reset_in_progress" }));
    }

    let instance = state.get_character(character_id.as_deref())?;
    let char_id = instance.id.clone();
    let manifest = instance.manifest.clone();

    // 执行自动触发tick（空闲检测+心情持续表情）
    crate::engine::auto_trigger_tick(&char_id, &manifest);

    // 更新心情状态到触发器
    let mood = instance.brain.psychology.compute_mood();
    crate::engine::update_mood_state(
        &char_id,
        &manifest,
        mood.primary_emotion.as_str(),
        mood.primary_intensity,
    );

    Ok(serde_json::json!({ "ticked": true }))
}

/// 触发系统事件（前端感知到窗口聚焦/失焦、音乐播放等时调用）
///
/// 支持事件：window_focus/window_blur/chat_start/chat_end/music_start/music_stop/battery_low/morning/afternoon/evening/night
#[tauri::command]
pub fn trigger_system_event(
    event: String,
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    let char_id = instance.id.clone();
    let manifest = instance.manifest.clone();

    crate::engine::trigger_event(&char_id, &event, &manifest);
    Ok(())
}
