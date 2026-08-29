//! PsychologyManager — 心理系统中枢。
//!
//! 职责：
//! 1. 持久化 PsychologySnapshot（psychology.json）
//! 2. 执行 Homeostasis tick（稳态调节）
//! 3. 应用 LLM 产出（appraisal + emotion_update + behavior_drive）
//! 4. 应用关系更新
//! 5. 构建心理学 prompt 上下文
//! 6. 计算 Mood（仅 UI）
//! 7. 提供规则驱动的 Behavior Drive

use std::sync::Arc;

use chrono::Timelike;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::appraisal::Appraisal;
use super::behavior_drive::{BehaviorDrive, RuleBasedDriveResolver};
use super::emotion::{EmotionDeltas, EmotionState};
use super::homeostasis::{CircadianFactors, HomeostasisEngine};
use super::mood::{compute_mood, MoodSnapshot};
use super::needs::{NeedDeltas, NeedsState};
use super::relationship::{
    MilestoneEntry, RelationshipEvent, RelationshipStage, RelationshipState, StageStrategy,
    EVENT_INTERACTION, EVENT_LONG_ABSENCE, EVENT_TIME_PASSAGE, EVENT_USER_RETURNED, EVENT_USER_SAD,
};
use super::snapshot::{PsychologySnapshot};

/// LLM 返回的心理状态产出（从 JSON 解析）
///
/// 这些字段都是 Option，因为 LLM 可能不返回（兼容性）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PsychologyOutput {
    pub appraisal: Option<Appraisal>,
    pub emotion_update: Option<EmotionDeltas>,
    pub behavior_drive: Option<BehaviorDrive>,
    /// 需求增量（可选，LLM 也可产出）
    pub need_update: Option<NeedDeltas>,
}

/// 用户交互的反馈 — 前端直接播放的 Live2D 表情/动作
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InteractionFeedback {
    /// 建议的表情（如 "shy"、"star_eyes"、"confused"）
    pub expression: String,
    /// 建议的动作（如 "idle"）
    pub motion: String,
    /// 前端动作（如 "wave_hand"/"blush"/"surprised" 等11个前端动作库动作）
    #[serde(default)]
    pub action: String,
    /// 表情持续时间（毫秒），0或None表示使用表情默认时长
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// 是否触发避让鼠标（连续点击时基于心理参数概率触发）
    #[serde(default)]
    pub avoid_mouse: bool,
    /// 避让概率（0.0-1.0，仅用于调试/日志，avoid_mouse=true 时有意义）
    #[serde(default)]
    pub avoid_probability: f64,
}

/// 心理系统管理器
pub struct PsychologyManager {
    state: Arc<RwLock<PsychologySnapshot>>,
    persistence_path: std::path::PathBuf,
    /// 持久化锁，防止并发写
    persist_lock: Mutex<()>,
    /// micro_tick 累计计数（达到阈值后持久化一次）
    micro_tick_count: parking_lot::Mutex<u32>,
    /// 当前角色的 ResourceManifest（用于交互反馈表情映射）
    manifest: Option<Arc<crate::engine::manifest::ResourceManifest>>,
}

/// micro_tick 每累积 N 次才持久化一次（默认 1 分钟：3s × 20）
const MICRO_TICK_PERSIST_INTERVAL: u32 = 20;

impl PsychologyManager {
    /// 从持久化文件加载，若不存在则用默认值初始化
    pub fn load_or_init(persistence_path: std::path::PathBuf) -> Self {
        let mut snapshot = if persistence_path.exists() {
            match std::fs::read_to_string(&persistence_path) {
                Ok(content) => serde_json::from_str::<PsychologySnapshot>(&content)
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            "psychology.json 解析失败，使用默认值: {}",
                            e
                        );
                        PsychologySnapshot::default()
                    }),
                Err(e) => {
                    tracing::warn!("读取 psychology.json 失败: {}", e);
                    PsychologySnapshot::default()
                }
            }
        } else {
            tracing::info!("psychology.json 不存在，使用默认值初始化");
            PsychologySnapshot::default()
        };

        // 启动时重置 last_interaction_time 为当前时间：
        // 持久化的值是上次运行时用户最后一次交互的时间戳，若距上次运行已超过阈值，
        // 启动后 secs_since_last_interaction() 会立即返回很大的值，
        // 进入疲劳公式后导致疲劳度过高，5 分钟状态保护期一过就触发 Online→Rest。
        // 重置后交互空闲计时从启动时刻重新开始，避免启动即 Rest。
        let now_ts = chrono::Utc::now().timestamp() as f64;
        snapshot.last_interaction_time = now_ts;

        Self {
            state: Arc::new(RwLock::new(snapshot)),
            persistence_path,
            persist_lock: Mutex::new(()),
            micro_tick_count: parking_lot::Mutex::new(0),
            manifest: None,
        }
    }

    /// 用指定 Persona 初始化（首次创建时）
    pub fn with_persona(self, persona: super::persona::PersonaProfile) -> Self {
        {
            let mut state = self.state.write();
            state.persona = persona;
            // 重新调制 set_points
            state.persona.apply_trait_modulation();
        }
        let _ = self.persist();
        self
    }

    /// 注入当前角色的 ResourceManifest（用于交互反馈表情映射）
    pub fn with_manifest(mut self, manifest: Arc<crate::engine::manifest::ResourceManifest>) -> Self {
        self.manifest = Some(manifest);
        self
    }

    /// 执行一次 Homeostasis tick
    ///
    /// 计算距上次 tick 的时间，执行稳态调节。应在后台定时调用（如每 60s）。
    pub fn homeostasis_tick(&self) {
        let now = chrono::Utc::now().timestamp() as f64;
        let dt_raw = {
            let state = self.state.read();
            (now - state.last_tick_time).max(0.0)
        };

        if dt_raw < 1.0 {
            return;
        }

        self.run_homeostasis_with_offline(dt_raw, now);
        let _ = self.persist();
    }

    /// 微调 tick — 高频调用（每 3-5 秒），让情绪持续波动
    ///
    /// 与 `homeostasis_tick` 不同：不跳过短 dt，允许小步长波动。
    /// 噪声在 `fluctuate` 中随 √dt 缩放，保证时间不变性。
    ///
    /// 持久化策略：累积 `MICRO_TICK_PERSIST_INTERVAL` 次才写盘一次（约 1 分钟），
    /// 避免每 3 秒同步写盘造成 I/O 压力与 SSD 损耗。
    pub fn micro_tick(&self) {
        let now = chrono::Utc::now().timestamp() as f64;
        let dt = {
            let state = self.state.read();
            (now - state.last_tick_time).max(0.0).min(60.0) // 最多 60s
        };

        if dt < 0.5 {
            return; // 至少 0.5s 间隔
        }

        self.run_homeostasis(dt, now);

        // 累积计数，达到阈值才持久化
        let should_persist = {
            let mut count = self.micro_tick_count.lock();
            *count = (*count + 1) % MICRO_TICK_PERSIST_INTERVAL;
            *count == 0
        };
        if should_persist {
            let _ = self.persist();
        }
    }

    /// 内部：执行 Homeostasis（不再自动持久化，由调用方决定）
    fn run_homeostasis(&self, dt: f64, now: f64) {
        let mut state = self.state.write();
        let snapshot: &mut PsychologySnapshot = &mut *state;
        let emotion_set_points = snapshot.persona.emotion_set_points.clone();
        let recovery_rates = snapshot.persona.recovery_rates.clone();
        let local = chrono::Local::now();
        let hour = local.hour() as f64 + local.minute() as f64 / 60.0;
        let circadian = CircadianFactors::at_hour(hour);
        HomeostasisEngine::tick(
            &mut snapshot.needs,
            &mut snapshot.emotion,
            &mut snapshot.persona.need_set_points,
            &emotion_set_points,
            &recovery_rates,
            dt,
            circadian,
        );
        snapshot.last_tick_time = now;
    }

    /// 内部：带离线压缩的 Homeostasis
    ///
    /// dt > 3600s 时，Needs 用封顶 1 小时稳态，Emotion 按各通道独立压缩系数结算。
    fn run_homeostasis_with_offline(&self, dt_raw: f64, now: f64) {
        let mut state = self.state.write();
        let snapshot: &mut PsychologySnapshot = &mut *state;
        let emotion_set_points = snapshot.persona.emotion_set_points.clone();
        let recovery_rates = snapshot.persona.recovery_rates.clone();
        let local = chrono::Local::now();
        let hour = local.hour() as f64 + local.minute() as f64 / 60.0;
        let circadian = CircadianFactors::at_hour(hour);

        if dt_raw > 3600.0 {
            HomeostasisEngine::tick(
                &mut snapshot.needs,
                &mut snapshot.emotion,
                &mut snapshot.persona.need_set_points,
                &emotion_set_points,
                &recovery_rates,
                3600.0,
                circadian,
            );
            HomeostasisEngine::apply_offline_compression(
                &mut snapshot.emotion,
                &emotion_set_points,
                &recovery_rates,
                dt_raw / 60.0,
                circadian,
            );
        } else {
            HomeostasisEngine::tick(
                &mut snapshot.needs,
                &mut snapshot.emotion,
                &mut snapshot.persona.need_set_points,
                &emotion_set_points,
                &recovery_rates,
                dt_raw,
                circadian,
            );
        }
        snapshot.last_tick_time = now;
    }

    /// 应用 LLM 产出（在 ResponseParsing 之后调用）
    ///
    /// 这是「事件 → Appraisal → Emotion → Behavior Drive」因果链的落地。
    /// LLM 在一次调用中产出 appraisal + emotion_update + behavior_drive，
    /// 这里将它们应用到内部状态，并记录情绪采样（供情绪弧线叙事）。
    pub fn apply_llm_output(&self, output: &PsychologyOutput) {
        let now = chrono::Utc::now().timestamp() as f64;
        let sensitivity_mult = {
            let state = self.state.read();
            state.persona.traits.sensitivity_multiplier()
        };

        let _ = now; // 时间戳由 add_event 内部生成

        {
            let mut state = self.state.write();
            let snapshot: &mut PsychologySnapshot = &mut *state;

            let dt_raw = (now - snapshot.last_tick_time).max(0.0);
            if dt_raw >= 1.0 {
                let emotion_set_points = snapshot.persona.emotion_set_points.clone();
                let recovery_rates = snapshot.persona.recovery_rates.clone();
                let local = chrono::Local::now();
                let hour = local.hour() as f64 + local.minute() as f64 / 60.0;
                let circadian = CircadianFactors::at_hour(hour);

                if dt_raw > 3600.0 {
                    HomeostasisEngine::tick(
                        &mut snapshot.needs,
                        &mut snapshot.emotion,
                        &mut snapshot.persona.need_set_points,
                        &emotion_set_points,
                        &recovery_rates,
                        3600.0,
                        circadian,
                    );
                    HomeostasisEngine::apply_offline_compression(
                        &mut snapshot.emotion,
                        &emotion_set_points,
                        &recovery_rates,
                        dt_raw / 60.0,
                        circadian,
                    );
                } else {
                    HomeostasisEngine::tick(
                        &mut snapshot.needs,
                        &mut snapshot.emotion,
                        &mut snapshot.persona.need_set_points,
                        &emotion_set_points,
                        &recovery_rates,
                        dt_raw,
                        circadian,
                    );
                }
                snapshot.last_tick_time = now;
            }

            let trust = snapshot.relationship.trust;

            if let Some(appraisal) = &output.appraisal {
                snapshot.last_appraisal = Some(appraisal.clone());

                let emotion_deltas_from_appraisal =
                    appraisal.to_emotion_deltas(sensitivity_mult, trust);
                snapshot.emotion.apply_delta(&emotion_deltas_from_appraisal, 1.0);

                let need_deltas_from_appraisal = appraisal.to_need_deltas();
                snapshot.needs.apply_delta(&need_deltas_from_appraisal);
            }

            if let Some(emotion_update) = &output.emotion_update {
                snapshot.emotion.apply_delta(emotion_update, sensitivity_mult);
            }

            let _ = snapshot.emotion.apply_interactions();

            // 应用 LLM 直接产出的 need_update
            if let Some(need_update) = &output.need_update {
                snapshot.needs.apply_delta(need_update);
            }

            // 应用 Behavior Drive（LLM 产出）
            if let Some(drive) = &output.behavior_drive {
                snapshot.last_drive = Some(drive.clone());
            }

            // 更新互动时间
            snapshot.last_interaction_time = now;

            // 记录情绪采样（供情绪弧线叙事）
            let emotion_after = snapshot.emotion.clone();
            snapshot.add_event(emotion_after);
        }

        let _ = self.persist();
    }

    /// 应用外部世界事件 —— 不经过 LLM，由事件类型映射到 Appraisal 模板
    ///
    /// 与 `apply_llm_output` 的区别：仅应用 appraisal（无 emotion_update/behavior_drive），
    /// 用于世界事件（天气变化/节日到来/日出日落等）对心理状态的隐式影响。
    /// 复用 apply_llm_output 的全部逻辑（Homeostasis 补偿 + 情绪采样 + persist）。
    pub fn apply_external_event(&self, event: &crate::world::WorldEvent) {
        let appraisal = event.to_appraisal();
        let output = PsychologyOutput {
            appraisal: Some(appraisal),
            emotion_update: None,
            behavior_drive: None,
            need_update: None,
        };
        tracing::debug!(
            "应用世界事件: {} ({}) -> appraisal sig={:.2}",
            event.kind.as_str(),
            event.description,
            event.significance
        );
        self.apply_llm_output(&output);
    }

    /// 应用关系更新（兼容旧 record_interaction 接口）
    ///
    /// 在 MoodStep 之后调用，基于 user_emotion 的 sentiment。
    pub fn apply_relationship_update(&self, sentiment: f64, intensity: f64) {
        let now = chrono::Utc::now().timestamp() as f64;
        {
            let mut state = self.state.write();
            let snapshot: &mut PsychologySnapshot = &mut *state;
            let appraisal = snapshot.last_appraisal.clone().unwrap_or_default();
            let emotion = snapshot.emotion.clone();
            let rel_deltas = RelationshipState::deltas_from_interaction(
                &appraisal,
                &emotion,
                sentiment * intensity,
            );
            snapshot.relationship.apply_delta(&rel_deltas);
            snapshot.last_interaction_time = now;
        }
        let _ = self.persist();
    }

    /// 统一 turn boundary：一次性完成单轮对话的所有心理学写路径。
    ///
    /// 把原来散落在 chat_chain.rs 三处的调用合并为原子操作：
    /// 1. `apply_llm_output`：应用 LLM 产出的 appraisal/emotion/drive/need
    /// 2. `apply_relationship_update`：基于 sentiment×intensity 更新 5 维关系
    /// 3. `record_interaction`：更新交互计数、亲密度阶段升级、临时态检测
    ///
    /// 保证三者执行顺序一致，避免中间插入其他写操作导致状态不一致。
    /// 其他调用方（consciousness_update_async、emotion/bridge、proactive）
    /// 仍可直接调用 `apply_llm_output` 做独立心理状态更新。
    pub fn apply_turn_boundary(
        &self,
        psy_output: &PsychologyOutput,
        sentiment: &str,
        intensity: f64,
    ) {
        // Step 1: LLM 产出的心理状态（appraisal + emotion + drive + need）
        self.apply_llm_output(psy_output);

        // Step 2: 5 维关系数值更新
        let sentiment_val = match sentiment {
            "happy" => 0.8 * intensity,
            "sad" => -0.5 * intensity,
            "angry" => -0.7 * intensity,
            "anxious" => -0.4 * intensity,
            "frustrated" => -0.6 * intensity,
            _ => 0.0,
        };
        self.apply_relationship_update(sentiment_val, intensity);

        // Step 3: 交互统计 + 阶段升级 + 临时态
        if let Err(e) = self.record_interaction(sentiment, intensity) {
            tracing::warn!("[PsychologyManager] turn boundary 交互统计更新失败: {}", e);
        }
    }

    /// 计算规则驱动的 Behavior Drive（主动行为 tick 用）
    ///
    /// 不调用 LLM，由当前 Needs/Emotion/Persona 推导。
    pub fn compute_rule_drive(&self) -> BehaviorDrive {
        let state = self.state.read();
        RuleBasedDriveResolver::resolve(&state.needs, &state.emotion, &state.persona)
    }

    /// 获取当前主导 Behavior Drive
    pub fn current_drive(&self) -> Option<BehaviorDrive> {
        let state = self.state.read();
        state.last_drive.clone()
    }

    /// 应用用户交互事件 — 由前端交互检测触发
    ///
    /// 不同的交互类型产生不同的情绪/需求增量，让桌宠对用户操作有"反应"：
    /// - `single_click`：单次点击 → 开心+关注
    /// - `double_click`：双击 → 惊喜+好奇
    /// - `fast_click`：快速连续点击 → 害羞 + 小幅快乐（被关注但有点不好意思）
    /// - `fast_drag`：快速拖动 → 小不满 + 嘟嘴（被打扰）
    /// - `drag_start`：开始拖动 → 疑惑
    /// - `drag_end`：结束拖动 → 放松
    /// - `pet`：缓慢抚摸 → 亲近 + 信任 + 归属满足（被关爱）
    /// - `long_press`：长按 → 好奇（主人在干嘛？）
    /// - `mouse_enter`：鼠标进入窗口 → 注意到用户
    /// - `mouse_leave`：鼠标离开窗口 → 略感失落
    ///
    /// 返回值：建议的 Live2D 表情/动作（前端直接播放，无需等 LLM）
    pub fn apply_user_interaction(&self, interaction: &str) -> InteractionFeedback {
        let sensitivity_mult = {
            let state = self.state.read();
            state.persona.traits.sensitivity_multiplier()
        };

        // fast_click 避让概率预计算：基于当前心理状态决定是否躲闪用户鼠标
        let (avoid_mouse, avoid_probability) = if interaction == "fast_click" {
            let state = self.state.read();
            let p = Self::compute_avoid_probability(&state);
            let triggered = rand::random::<f64>() < p;
            (triggered, p)
        } else {
            (false, 0.0)
        };

        let (emotion_delta, need_delta) = match interaction {
            "single_click" => (
                EmotionDeltas {
                    joy: 0.03,
                    closeness: 0.01,
                    curiosity: 0.02,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    expression: -0.02,
                    novelty: -0.01,
                    ..Default::default()
                },
            ),
            "double_click" => (
                EmotionDeltas {
                    joy: 0.06,
                    curiosity: 0.05,
                    closeness: 0.03,
                    fear: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    expression: -0.03,
                    novelty: -0.03,
                    ..Default::default()
                },
            ),
            "fast_click" => (
                EmotionDeltas {
                    joy: 0.04,
                    closeness: 0.02,
                    curiosity: 0.02,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    expression: -0.03,
                    belonging: -0.02,
                    ..Default::default()
                },
            ),
            "fast_drag" => (
                EmotionDeltas {
                    joy: 0.03,
                    anger: 0.01,
                    curiosity: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    novelty: -0.02,
                    autonomy: 0.03,
                    ..Default::default()
                },
            ),
            "drag_start" => (
                EmotionDeltas {
                    curiosity: 0.03,
                    fear: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    autonomy: 0.02,
                    ..Default::default()
                },
            ),
            "drag_end" => (
                EmotionDeltas {
                    joy: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    autonomy: -0.01,
                    ..Default::default()
                },
            ),
            "pet" => (
                EmotionDeltas {
                    joy: 0.06,
                    closeness: 0.08,
                    loneliness: -0.05,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    belonging: -0.05,
                    security: -0.03,
                    expression: -0.02,
                    ..Default::default()
                },
            ),
            "long_press" => (
                EmotionDeltas {
                    curiosity: 0.04,
                    closeness: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    novelty: -0.01,
                    ..Default::default()
                },
            ),
            "mouse_enter" => (
                EmotionDeltas {
                    curiosity: 0.02,
                    joy: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    novelty: -0.01,
                    ..Default::default()
                },
            ),
            "mouse_leave" => (
                EmotionDeltas {
                    loneliness: 0.01,
                    ..Default::default()
                },
                super::needs::NeedDeltas {
                    belonging: 0.01,
                    ..Default::default()
                },
            ),
            _ => return InteractionFeedback::default(),
        };

        // 从当前角色的 manifest 查询完整交互反馈（expression + motion + action + duration）
        let (expression, motion, action, duration_ms) = self
            .manifest
            .as_ref()
            .map(|m| m.interaction_feedback_full(interaction))
            .unwrap_or_else(|| (String::new(), crate::engine::manifest::DEFAULT_MOTION.to_string(), String::new(), None));

        let mut feedback = InteractionFeedback {
            expression,
            motion,
            action,
            duration_ms,
            avoid_mouse,
            avoid_probability,
        };

        // 避让触发时覆盖表情：从 manifest 查询 avoid_mouse 映射
        if avoid_mouse {
            if let Some(ref m) = self.manifest {
                let (avoid_expr, _, avoid_action, avoid_dur) = m.interaction_feedback_full("avoid_mouse");
                if !avoid_expr.is_empty() {
                    feedback.expression = avoid_expr;
                }
                if !avoid_action.is_empty() {
                    feedback.action = avoid_action;
                }
                if avoid_dur.is_some() {
                    feedback.duration_ms = avoid_dur;
                }
            }
        } else {
            // 交互表情多样化：约 35% 概率从当前主导情绪的表情池随机选一个，
            // 让同样的点击/抚摸在不同心情下产生不同表情，避免每次都一样
            if rand::random::<f64>() < 0.35 {
                let mood_label = {
                    let state = self.state.read();
                    let (label, _) = state.emotion.dominant();
                    label.as_str().to_string()
                };
                if let Some(ref m) = self.manifest {
                    if let Some(expr) = m.random_mood_expression(&mood_label) {
                        if !expr.is_empty() {
                            feedback.expression = expr;
                        }
                    }
                }
            }
        }

        {
            let mut state = self.state.write();
            let snapshot: &mut PsychologySnapshot = &mut *state;
            snapshot.emotion.apply_delta(&emotion_delta, sensitivity_mult);
            snapshot.needs.apply_delta(&need_delta);
            snapshot.last_interaction_time = chrono::Utc::now().timestamp() as f64;
        }
        let _ = self.persist();
        feedback
    }

    /// 计算连续点击时的避让鼠标概率（基于当前心理状态）
    ///
    /// 返回 0.0-1.0 的概率值。公式设计见 apply_user_interaction 注释。
    fn compute_avoid_probability(state: &PsychologySnapshot) -> f64 {
        let e = &state.emotion;
        let n = &state.needs;
        let r = &state.relationship;
        let t = &state.persona.traits;

        // 基础回避驱动：愤怒 + 恐惧 + 自主需求未满足
        let base = e.anger * 0.35 + e.fear * 0.25 + (n.autonomy - 0.5).max(0.0) * 0.15;

        // 关系调节：亲密/信任/关系稳固 → 容忍度提高
        let modulator = (1.0 - e.closeness * 0.3)
            * (1.0 - r.trust * 0.2)
            * (1.0 - r.intimacy * 0.2);

        // 人设调节：敏感型更易触发，韧性高更稳
        let sensitivity_factor = 0.7 + t.sensitivity * 0.6;
        let resilience_factor = 1.0 - t.resilience * 0.4;

        // 累积负面交互增益
        let neg_boost = 1.0 + (r.consecutive_negative as f64 * 0.1).min(0.5);

        let probability = base * modulator * sensitivity_factor * resilience_factor * neg_boost;
        probability.clamp(0.0, 0.95)
    }

    /// 计算从睡眠中唤醒时生成问候语的概率（基于当前心理状态）
    ///
    /// 心理学公式：
    ///   approach = joy * 0.20 + closeness * 0.15 + intimacy * 0.10
    ///              + curiosity * 0.08 + loneliness * 0.15
    ///   avoidance = anger * 0.25 + fear * 0.15
    ///   fatigue_factor = 1.0 - (fatigue / 100) * 0.5      // 疲劳 0-100，最多 50% 抑制
    ///   neg_factor = 1.0 - min(consecutive_negative * 0.1, 0.5)
    ///   trust_factor = 0.75 + trust * 0.25                 // 信任调节 0.75-1.0
    ///   base = 0.25                                        // 基础概率 25%
    ///   probability = (base + approach * 0.7) * fatigue_factor * neg_factor * trust_factor - avoidance
    ///
    /// 含义：积极情绪 + 关系亲近 + 孤单感 → 想找用户说话；
    ///       疲劳（刚醒还困）→ 抑制；连续负面交互 → 抑制；
    ///       信任 → 提高表达意愿；愤怒/恐惧 → 直接抑制。
    pub fn compute_wake_greeting_probability(&self) -> f64 {
        let state = self.state.read();
        let e = &state.emotion;
        let r = &state.relationship;

        // 从 Mood 推导疲劳度（0-100）
        let mood = compute_mood(
            &state.emotion,
            &state.needs,
            &state.relationship,
            state.secs_since_last_interaction(),
        );
        let fatigue = mood.fatigue;

        let approach = e.joy * 0.20
            + e.closeness * 0.15
            + r.intimacy * 0.10
            + e.curiosity * 0.08
            + e.loneliness * 0.15;
        let avoidance = e.anger * 0.25 + e.fear * 0.15;
        let fatigue_factor = 1.0 - (fatigue / 100.0).clamp(0.0, 1.0) * 0.5;
        let neg_factor = 1.0 - (r.consecutive_negative as f64 * 0.1).min(0.5);
        let trust_factor = 0.75 + r.trust * 0.25;

        let base = 0.25;
        let probability =
            (base + approach * 0.7) * fatigue_factor * neg_factor * trust_factor - avoidance;
        probability.clamp(0.0, 0.95)
    }

    /// 计算 Mood 快照（仅 UI）
    pub fn compute_mood(&self) -> MoodSnapshot {
        let state = self.state.read();
        compute_mood(
            &state.emotion,
            &state.needs,
            &state.relationship,
            state.secs_since_last_interaction(),
        )
    }

    /// 计算 PetState（仅 UI 展示，18 种衍生状态）
    ///
    /// PetState 不参与决策，由 EmotionState + Needs + Relationship 投影推导。
    pub fn compute_pet_state(&self) -> super::pet_state::PetState {
        let state = self.state.read();
        super::pet_state::compute_pet_state(
            &state.emotion,
            &state.needs,
            &state.relationship,
            state.secs_since_last_interaction(),
        )
    }

    /// 构建心理学 prompt 上下文（注入 LLM）
    ///
    /// 向 LLM 详细介绍当前五层心理状态，让 LLM 理解系统规则并产出规范 JSON。
    /// `recent_events_desc` 由调用方从记忆系统查询 ImportantEvent 最近 5 条后传入。
    /// Build a concise natural-language psychology snapshot for the LLM.
    ///
    /// Instead of dumping raw percentages and rule tables (which feel like reading
    /// a spreadsheet), this produces a short, human-readable mood summary — like
    /// a brief note about how you're feeling right now.
    ///
    /// Key design choices:
    /// - No percentage numbers — uses natural intensity words (a bit / pretty / really)
    /// - No repetitive rule tables — the LLM already knows that happy people talk more,
    ///   tired people talk less; spelling it out every turn is mechanical
    /// - English-only to match the rest of the system prompt
    /// - ~10 lines instead of ~100
    pub fn build_psychology_prompt(&self, recent_events_desc: &str, lang: &str) -> String {
        let state = self.state.read();
        let (dominant_emotion, dominant_intensity) = state.emotion.dominant();
        let (deficient_need, deficient_val) = state.needs.most_deficient();

        let lang = crate::pipeline::prompt_modules::normalize_lang(lang);

        let intensity_word_en = |v: f64| -> &'static str {
            if v > 0.7 { "really" }
            else if v > 0.5 { "pretty" }
            else if v > 0.3 { "a bit" }
            else { "" }
        };
        let translate_intensity = |en: &'static str| -> &'static str {
            match (lang, en) {
                ("zh", "really") => "很",
                ("zh", "pretty") => "挺",
                ("zh", "a bit") => "有点",
                ("ja", "really") => "とても",
                ("ja", "pretty") => "かなり",
                ("ja", "a bit") => "少し",
                _ => en,
            }
        };

        let emo_word_en = match dominant_emotion {
            crate::psychology::EmotionLabel::Joy => "good",
            crate::psychology::EmotionLabel::Sadness => "down",
            crate::psychology::EmotionLabel::Anger => "irritated",
            crate::psychology::EmotionLabel::Fear => "uneasy",
            crate::psychology::EmotionLabel::Closeness => "warm toward them",
            crate::psychology::EmotionLabel::Loneliness => "lonely",
            crate::psychology::EmotionLabel::Curiosity => "curious",
        };
        let emo_word = match lang {
            "zh" => match emo_word_en {
                "good" => "不错",
                "down" => "低落",
                "irritated" => "烦躁",
                "uneasy" => "不安",
                "warm toward them" => "对他们很亲近",
                "lonely" => "孤独",
                "curious" => "好奇",
                _ => emo_word_en,
            },
            "ja" => match emo_word_en {
                "good" => "良い",
                "down" => "落ち込んでいる",
                "irritated" => "イライラしている",
                "uneasy" => "不安",
                "warm toward them" => "彼らに親密",
                "lonely" => "寂しい",
                "curious" => "好奇心がある",
                _ => emo_word_en,
            },
            _ => emo_word_en,
        };

        let mut lines: Vec<String> = Vec::new();
        lines.push(
            crate::pipeline::prompt_modules::section_heading("emotion_state", lang).to_string(),
        );

        // Dominant emotion — natural phrasing
        let inten_en = intensity_word_en(dominant_intensity);
        if !inten_en.is_empty() {
            let inten = translate_intensity(inten_en);
            let sentence = match lang {
                "zh" => format!("你现在感觉{}{}。", inten, emo_word),
                "ja" => format!("今{}{}と感じている。", inten, emo_word),
                _ => format!("You're feeling {} {} right now.", inten_en, emo_word_en),
            };
            lines.push(sentence);
        } else {
            lines.push(
                match lang {
                    "zh" => "你感觉比较平静。",
                    "ja" => "かなり落ち着いている。",
                    _ => "You're feeling fairly neutral.",
                }
                .to_string(),
            );
        }

        // Secondary emotions (if any are notably high)
        let secondary: Vec<String> = {
            let e = &state.emotion;
            let mut v = Vec::new();
            if e.sadness > 0.4 && dominant_emotion != crate::psychology::EmotionLabel::Sadness {
                v.push(
                    match lang {
                        "zh" => "有点难过",
                        "ja" => "少し悲しい",
                        _ => "a little sad",
                    }
                    .to_string(),
                );
            }
            if e.anger > 0.4 && dominant_emotion != crate::psychology::EmotionLabel::Anger {
                v.push(
                    match lang {
                        "zh" => "有点烦躁",
                        "ja" => "少しイライラ",
                        _ => "mildly annoyed",
                    }
                    .to_string(),
                );
            }
            if e.curiosity > 0.5 && dominant_emotion != crate::psychology::EmotionLabel::Curiosity {
                v.push(
                    match lang {
                        "zh" => "有点好奇",
                        "ja" => "少し好奇心がある",
                        _ => "kind of curious",
                    }
                    .to_string(),
                );
            }
            if e.loneliness > 0.4 && dominant_emotion != crate::psychology::EmotionLabel::Loneliness {
                v.push(
                    match lang {
                        "zh" => "有点孤独",
                        "ja" => "少し寂しい",
                        _ => "a bit lonely",
                    }
                    .to_string(),
                );
            }
            if e.closeness > 0.5 && dominant_emotion != crate::psychology::EmotionLabel::Closeness {
                v.push(
                    match lang {
                        "zh" => "感觉和他们很亲近",
                        "ja" => "彼らと親密だと感じる",
                        _ => "feeling close to them",
                    }
                    .to_string(),
                );
            }
            v
        };
        if !secondary.is_empty() {
            let joined = secondary.join(", ");
            let sentence = match lang {
                "zh" => format!("内心还有{}。", joined),
                "ja" => format!("内心には{}もある。", joined),
                _ => format!("There's also {} underneath.", joined),
            };
            lines.push(sentence);
        }

        // Most pressing need
        if deficient_val > 0.5 {
            let need_desc = match deficient_need {
                "belonging" => match lang {
                    "zh" => "你想找人说说话。",
                    "ja" => "誰かと話したい。",
                    _ => "You kind of want someone to talk to.",
                },
                "autonomy" => match lang {
                    "zh" => "你想先做自己的事。",
                    "ja" => "自分のことをしたい。",
                    _ => "You feel like doing your own thing for a bit.",
                },
                "novelty" => match lang {
                    "zh" => "有点无聊，想找点有趣的事。",
                    "ja" => "少し退屈で、面白いことがしたい。",
                    _ => "You're a bit bored and craving something interesting.",
                },
                "expression" => match lang {
                    "zh" => "有话想说。",
                    "ja" => "言いたいことがある。",
                    _ => "You've got something you feel like saying.",
                },
                "security" => match lang {
                    "zh" => "感觉有点不对劲。",
                    "ja" => "何かが少し違う気がする。",
                    _ => "Something feels a little off.",
                },
                _ => "",
            };
            if !need_desc.is_empty() {
                if deficient_val > 0.7 {
                    let sentence = match lang {
                        "zh" => format!("{} 挺强烈的。", need_desc),
                        "ja" => format!("{} かなり強い。", need_desc),
                        _ => format!("{} It's pretty strong.", need_desc),
                    };
                    lines.push(sentence);
                } else {
                    lines.push(need_desc.to_string());
                }
            }
        }

        // Recent notable events (keep brief, only if meaningful)
        let has_events = !recent_events_desc.trim().is_empty() && recent_events_desc.trim() != "无";
        if has_events {
            lines.push(
                match lang {
                    "zh" => "最近一直在想的事：",
                    "ja" => "最近気になっていること：",
                    _ => "Something that's been on your mind recently:",
                }
                .to_string(),
            );
            // Truncate to first 2 events to avoid bloat
            for line in recent_events_desc.lines().take(2) {
                let clean = line.trim_start_matches("- ").trim();
                if !clean.is_empty() {
                    lines.push(format!("- {}", clean));
                }
            }
        }

        lines.join("\n")
    }

    /// 获取只读快照（用于序列化或调试）。
    ///
    /// 返回 PsychologySnapshot 的深拷贝。调用方（emotion bridge、proactive、diary 等）
    /// 需要独立副本来释放锁后继续访问字段（如 events 列表），因此 clone 是必要的。
    /// 该方法不在每条消息的热路径上（多为命令或周期性任务调用），开销可接受。
    /// 若未来出现高频热路径调用，可考虑新增按字段访问的方法（如 `events_snapshot()`）
    /// 避免克隆整个快照。
    pub fn snapshot(&self) -> PsychologySnapshot {
        self.state.read().clone()
    }

    /// 持久化到文件（后台线程写，避免阻塞 invoke 调用线程）
    ///
    /// 序列化在持锁期间完成后立即释放锁，实际磁盘 I/O 在独立线程执行，
    /// 不阻塞调用方。失败仅记录日志。
    fn persist(&self) -> Result<(), String> {
        let _guard = self.persist_lock.try_lock();
        let (json, path) = {
            let state = self.state.read();
            let json = serde_json::to_string(&*state)
                .map_err(|e| format!("序列化失败: {}", e))?;
            (json, self.persistence_path.clone())
        };
        // 后台线程写盘，不阻塞调用方
        std::thread::spawn(move || {
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("psychology.json 创建目录失败: {}", e);
                    return;
                }
            }
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("psychology.json 写入失败: {}", e);
            }
        });
        Ok(())
    }

    /// 获取 Persona 引用（用于初始化或其他模块读取）
    pub fn persona(&self) -> super::persona::PersonaProfile {
        self.state.read().persona.clone()
    }

    /// 获取 Emotion 引用
    pub fn emotion(&self) -> EmotionState {
        self.state.read().emotion.clone()
    }

    /// 获取 Needs 引用
    pub fn needs(&self) -> NeedsState {
        self.state.read().needs.clone()
    }

    /// 获取 Relationship 引用
    pub fn relationship(&self) -> RelationshipState {
        self.state.read().relationship.clone()
    }

    // ====================================================================
    // 关系系统方法（整合自原 RelationshipManager）
    // ====================================================================

    /// 记录一次用户交互，更新交互统计 + 阶段升级 + 临时态
    ///
    /// 注意：5 维关系数值（trust/intimacy/respect/dependency/familiarity）的更新
    /// 由 `apply_relationship_update` 基于 Appraisal+Emotion 心理学驱动，本方法不更新 5 维。
    /// 本方法只负责：交互计数、连续正/负向统计、阶段升级检查、临时态进入/退出。
    pub fn record_interaction(&self, sentiment: &str, intensity: f64) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp() as f64;
        {
            let mut state = self.state.write();

            // 交互统计
            state.relationship.interaction_count += 1;
            state.relationship.last_interaction_time = now;

            // 连续正/负向统计
            if sentiment == "happy" || sentiment == "positive" {
                state.relationship.consecutive_positive += 1;
                state.relationship.consecutive_negative = 0;
            } else if sentiment == "sad" || sentiment == "anxious" || sentiment == "angry" {
                state.relationship.consecutive_negative += 1;
                state.relationship.consecutive_positive = 0;
            } else {
                state.relationship.consecutive_positive += 1;
                state.relationship.consecutive_negative = 0;
            }

            // 检查阶段升级（基于 intimacy + interaction_count）
            state.relationship.check_stage_upgrade();

            // 检查临时态
            let event = RelationshipEvent::new(EVENT_INTERACTION)
                .with_intensity(intensity)
                .with_sentiment(sentiment);
            state.relationship.check_temporary_stage(&event);

            // 同步快照的 last_interaction_time
            state.last_interaction_time = now;
        }
        self.persist()
    }

    /// 记录用户负面反馈
    pub fn record_negative_feedback(&self, intensity: f64) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp() as f64;
        {
            let mut state = self.state.write();
            state.relationship.interaction_count += 1;
            state.relationship.last_interaction_time = now;
            state.relationship.consecutive_negative += 1;
            state.relationship.consecutive_positive = 0;
            let delta = (3.0 * intensity).floor() / 100.0;
            state.relationship.intimacy = (state.relationship.intimacy - delta).max(0.0);

            // 负面交互不直接触发临时态（避免与 Soothing 冲突）
            state.last_interaction_time = now;
        }
        self.persist()
    }

    /// 主动发话反馈闭环：用户回应 → intimacy 微升；冷落 → intimacy 微降
    ///
    /// 幅度刻意比 record_negative_feedback 小一个量级——
    /// 这不是"负面事件"，只是"她主动搭话被忽略/被回应"的日常微调，
    /// 让她真的会"记住你对她怎么样"。
    pub fn apply_proactive_feedback(&self, positive: bool, char_id: &str) -> Result<(), String> {
        let behavior = crate::character_behavior::get_behavior(char_id);
        {
            let mut state = self.state.write();
            let now = chrono::Utc::now().timestamp() as f64;
            state.relationship.last_interaction_time = now;
            // 情绪敏感度（与 apply_user_interaction / apply_llm_output 一致）
            let sensitivity_mult = state.persona.traits.sensitivity_multiplier();
            if positive {
                state.relationship.intimacy =
                    (state.relationship.intimacy + behavior.proactive_feedback_positive).min(1.0);
                state.relationship.consecutive_positive += 1;
                state.relationship.consecutive_negative = 0;
                // 被回应 → 孤独感下降、亲近感上升
                let delta = super::emotion::EmotionDeltas {
                    loneliness: -0.04,
                    closeness: 0.03,
                    joy: 0.02,
                    ..Default::default()
                };
                state.emotion.apply_delta(&delta, sensitivity_mult);
            } else {
                state.relationship.intimacy =
                    (state.relationship.intimacy - behavior.proactive_feedback_negative).max(0.0);
                state.relationship.consecutive_negative += 1;
                state.relationship.consecutive_positive = 0;
                // 被冷落 → 孤独感上升、小幅度悲伤
                // 幅度刻意较小（单次冷落不会剧变），但累积多次会让 loneliness 真正上升，
                // 从而触发 MoodDriven 主动发话和 behavior_drive 的 approach 行为。
                let delta = super::emotion::EmotionDeltas {
                    loneliness: 0.05,
                    sadness: 0.015,
                    closeness: -0.02,
                    ..Default::default()
                };
                state.emotion.apply_delta(&delta, sensitivity_mult);
            }
            state.last_interaction_time = now;
        }
        self.persist()
    }

    /// 记录用户情绪低落（触发 Soothing 临时态）
    pub fn record_user_sad(&self, intensity: f64) -> Result<(), String> {
        {
            let mut state = self.state.write();
            let event = RelationshipEvent::new(EVENT_USER_SAD).with_intensity(intensity);
            state.relationship.check_temporary_stage(&event);
        }
        self.persist()
    }

    /// 记录长时间未交互（触发 LowActivity 临时态 + 亲密度衰减）
    ///
    /// 缺席 <4h 不处理；衰减 = min(int(hours/24)*2, 15)（×100 后）
    pub fn record_long_absence(&self) -> Result<(), String> {
        let hours = {
            let state = self.state.read();
            let h = state.relationship.absent_hours();
            if h < 4.0 { return Ok(()); } else { h }
        };

        {
            let mut state = self.state.write();
            let decay = ((hours / 24.0).floor() * 2.0).min(15.0) / 100.0;
            state.relationship.intimacy = (state.relationship.intimacy - decay).max(0.0);

            let event = RelationshipEvent::new(EVENT_LONG_ABSENCE).with_duration(hours);
            state.relationship.check_temporary_stage(&event);
        }
        self.persist()
    }

    /// 记录用户归来（触发 Reconnecting 临时态）
    ///
    /// 缺席 <4h 不处理。
    pub fn record_user_returned(&self) -> Result<(), String> {
        let hours = {
            let state = self.state.read();
            let h = state.relationship.absent_hours();
            if h < 4.0 { return Ok(()); } else { h }
        };

        {
            let mut state = self.state.write();
            let event = RelationshipEvent::new(EVENT_USER_RETURNED).with_duration(hours);
            state.relationship.check_temporary_stage(&event);
        }
        self.persist()
    }

    /// 周期性衰减检查（发送 0.5h 时间流逝事件，触发 Soothing 等退出）
    pub fn tick_decay(&self) -> Result<(), String> {
        {
            let mut state = self.state.write();
            let event = RelationshipEvent::new(EVENT_TIME_PASSAGE).with_duration(0.5);
            state.relationship.check_temporary_stage(&event);
        }
        self.persist()
    }

    /// 获取里程碑列表
    pub fn get_milestones(&self) -> Vec<MilestoneEntry> {
        self.state.read().relationship.milestones.clone()
    }

    /// 记录自定义里程碑
    pub fn record_milestone(&self, description: &str) -> Result<(), String> {
        {
            let mut state = self.state.write();
            state.relationship.record_custom_milestone(description);
        }
        self.persist()
    }

    /// 重置关系（回到陌生人）
    pub fn reset_relationship(&self) -> Result<(), String> {
        {
            let mut state = self.state.write();
            state.relationship.reset();
        }
        self.persist()
    }

    /// 恢复出厂设置：将整个心理快照重置为初始默认值
    ///
    /// 与 `reset_relationship` 仅重置关系字段不同，此方法会清空：
    /// - emotion（情绪状态）
    /// - needs（心理需求）
    /// - relationship（关系数值与交互统计）
    /// - last_appraisal / last_drive（上次交互缓存）
    /// - last_interaction_time / last_tick_time（重置为当前时间，避免衰减计算异常）
    /// - events（情绪采样历史）
    ///
    /// 注意：persona 会被重置为 default，但下次启动时 `with_persona` 会从
    /// PersonaEngine 重新推导角色特定的 persona，所以安全。
    /// 运行时的 manifest（表情映射）不持久化，保留不影响。
    pub fn reset_to_initial(&self) -> Result<(), String> {
        {
            let mut state = self.state.write();
            *state = PsychologySnapshot::default();
        }
        self.persist()
    }

    /// 永久阶段
    pub fn get_stage(&self) -> RelationshipStage {
        self.state.read().relationship.permanent_stage
    }

    /// 生效阶段标签（临时态覆盖永久态的中文标签）
    pub fn get_effective_stage_label(&self) -> String {
        self.state.read().relationship.get_effective_stage_label()
    }

    /// 当前生效的策略
    pub fn get_strategy(&self) -> StageStrategy {
        self.state.read().relationship.get_strategy()
    }

    /// 主动级别（0-4）
    pub fn get_proactivity_level(&self) -> u32 {
        self.get_strategy().proactivity_level
    }

    /// 每日最大主动次数
    pub fn get_max_daily_proactive(&self) -> u32 {
        self.get_strategy().max_daily_proactive
    }

    /// 语气风格
    pub fn get_tone(&self) -> String {
        self.get_strategy().tone
    }

    /// 生成关系上下文 prompt 段（注入 system prompt）
    pub fn relationship_section(&self, lang: &str) -> String {
        self.state.read().relationship.to_prompt_section(lang)
    }
}

// parking_lot::RwLock 的 read()/write() 返回的 guard 自动释放，无需额外处理。

/// 持久化到文件的路径辅助
pub fn default_psychology_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("psychology.json")
}
