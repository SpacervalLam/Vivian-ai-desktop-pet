//! 思绪生命周期系统 —— 连接内心独白与主动对话的桥梁
//!
//! 核心思路：事件不直接触发独白或消息，而是播下"思绪种子"。
//! 思绪随时间积累强度，跨过不同阈值时产生不同行为：
//!
//! ```text
//! Seed Thought (种子)
//!     ↓ 时间流逝 / 相关事件叠加
//! Growing (滋长)
//!     ↓ intensity ≥ INNER_MONOLOGUE_THRESHOLD
//! Active → Inner Monologue (内心独白，不打扰用户)
//!     ↓ 继续积累，desire_to_share 足够高
//! Expressed → Proactive Message (主动说出来)
//!     ↓ 表达后自然衰减
//! Faded (消退)
//! ```
//!
//! 阈值：
//! - 0.0 ~ 0.3  无意识：只影响 Mind/情绪，不产生可感知输出
//! - 0.3 ~ 0.7  内心独白：产生 inner_monologue，写入记忆
//! - 0.7 ~ 1.0  主动表达：产生 proactive_message，对用户说话

use serde::{Deserialize, Serialize};

/// 思绪表达阈值
pub const INNER_MONOLOGUE_THRESHOLD: f32 = 0.30;
pub const PROACTIVE_SHARE_THRESHOLD: f32 = 0.70;

/// 思绪自然衰减速率（每秒衰减量，在无新事件滋养时）
const NATURAL_DECAY_PER_SEC: f32 = 0.0008;
/// 相关新事件对思绪的增幅
const RELEVANT_EVENT_BOOST: f32 = 0.15;
/// 内心独白表达后强度衰减（独白后思绪不会立刻消失，但降下来）
const AFTER_MONOLOGUE_DECAY: f32 = 0.35;
/// 主动表达后强度衰减（说出来了，念头释放了）
const AFTER_EXPRESSED_DECAY: f32 = 0.75;
/// 表达欲自然增长率（在 user_present 时思绪更想表达）
const DESIRE_GROWTH_PER_SEC_PRESENT: f32 = 0.003;
/// 表达欲在用户离开时衰减
const DESIRE_DECAY_PER_SEC_AWAY: f32 = 0.005;
/// 同时存在的思绪最大数量
const MAX_CONCURRENT_THOUGHTS: usize = 4;

/// 思绪生命周期阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThoughtPhase {
    /// 种子：刚被事件触发，还很微弱
    Seed,
    /// 滋长：在心里萦绕，强度在上升
    Growing,
    /// 活跃：已产生过内心独白，但还没说出口
    Active,
    /// 已表达：已经主动说出来了
    Expressed,
    /// 消退：强度降低到可忽略，等待被清理
    Faded,
}

/// 一条正在"活着"的思绪
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveThought {
    /// 思绪的唯一标识（由触发类型决定，同类思绪合并）
    pub thought_key: String,
    /// 思绪的简短描述（用于上下文注入）
    pub description: String,
    /// 思绪来源事件的上下文提示（注入 prompt）
    pub context_hint: String,
    /// 当前强度 [0, 1]
    pub intensity: f32,
    /// 表达欲 [0, 1]：这个念头多想"说出来"
    pub desire_to_share: f32,
    /// 情绪色彩：正/负/中性，影响独白和表达的语气
    pub valence: f32,
    /// 情绪唤醒度
    pub arousal: f32,
    /// 当前阶段
    pub phase: ThoughtPhase,
    /// 思绪产生的时间戳
    pub created_at: f64,
    /// 最后一次被相关事件滋养的时间
    pub last_nourished_at: f64,
    /// 是否已通过主动消息表达过
    pub expressed: bool,
    /// 触发这个思绪的事件类型
    pub trigger_kind: String,
}

impl ActiveThought {
    /// 创建一个新的种子思绪
    pub fn new_seed(
        thought_key: &str,
        description: &str,
        context_hint: &str,
        base_intensity: f32,
        valence: f32,
        arousal: f32,
        trigger_kind: &str,
        now: f64,
    ) -> Self {
        Self {
            thought_key: thought_key.to_string(),
            description: description.to_string(),
            context_hint: context_hint.to_string(),
            intensity: base_intensity.clamp(0.05, 0.4),
            desire_to_share: 0.1,
            valence: valence.clamp(-1.0, 1.0),
            arousal: arousal.clamp(0.0, 1.0),
            phase: if base_intensity >= INNER_MONOLOGUE_THRESHOLD {
                ThoughtPhase::Growing
            } else {
                ThoughtPhase::Seed
            },
            created_at: now,
            last_nourished_at: now,
            expressed: false,
            trigger_kind: trigger_kind.to_string(),
        }
    }

    /// 是否应该产生内心独白
    pub fn should_produce_monologue(&self) -> bool {
        if self.expressed {
            return false;
        }
        if self.intensity < INNER_MONOLOGUE_THRESHOLD {
            return false;
        }
        if matches!(self.phase, ThoughtPhase::Faded) {
            return false;
        }
        self.phase != ThoughtPhase::Active || self.intensity > INNER_MONOLOGUE_THRESHOLD + 0.15
    }

    /// 是否应该主动说出来
    pub fn should_share(&self) -> bool {
        if self.expressed {
            return false;
        }
        self.intensity >= PROACTIVE_SHARE_THRESHOLD && self.desire_to_share >= 0.5
    }

    /// 标记为已产生内心独白
    pub fn mark_monologue(&mut self) {
        self.intensity = (self.intensity - AFTER_MONOLOGUE_DECAY).max(0.0);
        self.phase = if self.intensity < INNER_MONOLOGUE_THRESHOLD {
            ThoughtPhase::Faded
        } else {
            ThoughtPhase::Active
        };
    }

    /// 标记为已主动表达
    pub fn mark_expressed(&mut self, now: f64) {
        self.expressed = true;
        self.intensity = (self.intensity - AFTER_EXPRESSED_DECAY).max(0.0);
        self.desire_to_share = (self.desire_to_share - 0.5).max(0.0);
        self.phase = ThoughtPhase::Expressed;
        let _ = now;
    }

    /// 时间流逝更新：自然衰减 + 阶段转换
    pub fn tick(&mut self, dt_secs: f64, user_present: bool) {
        if matches!(self.phase, ThoughtPhase::Faded | ThoughtPhase::Expressed) {
            self.intensity = (self.intensity - NATURAL_DECAY_PER_SEC * dt_secs as f32 * 2.0).max(0.0);
            if self.intensity < 0.05 {
                self.phase = ThoughtPhase::Faded;
            }
            return;
        }

        let dt = dt_secs as f32;
        let since_nourished = dt_secs - (self.last_nourished_at);

        if since_nourished > 60.0 {
            self.intensity = (self.intensity - NATURAL_DECAY_PER_SEC * dt).max(0.0);
        } else {
            let idle_factor = ((since_nourished - 30.0).max(0.0) / 300.0) as f32;
            self.intensity = (self.intensity - NATURAL_DECAY_PER_SEC * dt * idle_factor).max(0.0);
        }

        if user_present && !self.expressed && matches!(self.phase, ThoughtPhase::Active | ThoughtPhase::Growing) {
            self.desire_to_share = (self.desire_to_share + DESIRE_GROWTH_PER_SEC_PRESENT * dt).min(1.0);
        } else if !user_present {
            self.desire_to_share = (self.desire_to_share - DESIRE_DECAY_PER_SEC_AWAY * dt).max(0.0);
        }

        self.update_phase();
    }

    /// 被相关事件滋养：增强强度
    pub fn nourish(&mut self, boost: f32, context_update: Option<&str>, now: f64) {
        self.intensity = (self.intensity + boost).min(1.0);
        self.desire_to_share = (self.desire_to_share + boost * 0.5).min(1.0);
        self.last_nourished_at = now;
        if let Some(ctx) = context_update {
            self.context_hint = ctx.to_string();
        }
        if self.intensity >= PROACTIVE_SHARE_THRESHOLD {
            self.phase = ThoughtPhase::Active;
        } else if self.intensity >= INNER_MONOLOGUE_THRESHOLD {
            self.phase = ThoughtPhase::Growing;
        }
    }

    fn update_phase(&mut self) {
        if self.expressed {
            self.phase = if self.intensity < 0.05 {
                ThoughtPhase::Faded
            } else {
                ThoughtPhase::Expressed
            };
            return;
        }
        self.phase = if self.intensity >= PROACTIVE_SHARE_THRESHOLD {
            ThoughtPhase::Active
        } else if self.intensity >= INNER_MONOLOGUE_THRESHOLD {
            ThoughtPhase::Growing
        } else if self.intensity >= 0.05 {
            ThoughtPhase::Seed
        } else {
            ThoughtPhase::Faded
        };
    }
}

/// 思绪生命周期管理器
///
/// 维护所有活跃思绪，负责：
/// 1. 接收新事件 → 播种新思绪 / 滋养已有思绪
/// 2. 每 tick 更新强度衰减和表达欲
/// 3. 选择当前最强烈的思绪输出（独白或主动消息）
pub struct ThoughtLifecycle {
    thoughts: Vec<ActiveThought>,
    last_tick_time: f64,
}

impl ThoughtLifecycle {
    pub fn new() -> Self {
        Self {
            thoughts: Vec::new(),
            last_tick_time: 0.0,
        }
    }

    /// 播下一颗思绪种子（或滋养已有思绪）
    ///
    /// 返回是否产生了新种子（用于日志）。
    /// 如果同 key 的思绪已存在，则 nourish 而不是新建。
    pub fn seed_thought(
        &mut self,
        thought_key: &str,
        description: &str,
        context_hint: &str,
        base_intensity: f32,
        valence: f32,
        arousal: f32,
        trigger_kind: &str,
        now: f64,
    ) -> bool {
        if let Some(existing) = self.thoughts.iter_mut().find(|t| t.thought_key == thought_key) {
            existing.nourish(RELEVANT_EVENT_BOOST.min(base_intensity * 0.5), Some(context_hint), now);
            return false;
        }

        let thought = ActiveThought::new_seed(
            thought_key,
            description,
            context_hint,
            base_intensity,
            valence,
            arousal,
            trigger_kind,
            now,
        );
        self.thoughts.push(thought);

        if self.thoughts.len() > MAX_CONCURRENT_THOUGHTS {
            self.evict_faintest();
        }

        true
    }

    /// 直接滋养某个思绪（外部知道 key 时使用）
    pub fn nourish_thought(&mut self, thought_key: &str, boost: f32, now: f64) {
        if let Some(t) = self.thoughts.iter_mut().find(|t| t.thought_key == thought_key) {
            t.nourish(boost, None, now);
        }
    }

    /// 更新一个 tick 的时间流逝
    pub fn tick(&mut self, now: f64, user_present: bool) {
        if self.last_tick_time <= 0.0 {
            self.last_tick_time = now;
            return;
        }
        let dt = (now - self.last_tick_time).max(0.0);
        if dt <= 0.0 {
            return;
        }
        for t in &mut self.thoughts {
            t.tick(dt, user_present);
        }
        self.thoughts.retain(|t| t.phase != ThoughtPhase::Faded || t.intensity > 0.03);
        self.last_tick_time = now;
    }

    /// 获取当前最强烈的可独白思绪（返回引用），不改变状态
    pub fn pick_monologue_candidate(&self) -> Option<&ActiveThought> {
        self.thoughts
            .iter()
            .filter(|t| t.should_produce_monologue())
            .max_by(|a, b| a.intensity.partial_cmp(&b.intensity).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// 获取当前可主动表达的思绪（最强且表达欲足够）
    pub fn pick_share_candidate(&self) -> Option<&ActiveThought> {
        self.thoughts
            .iter()
            .filter(|t| t.should_share())
            .max_by(|a, b| {
                let sa = a.intensity * a.desire_to_share;
                let sb = b.intensity * b.desire_to_share;
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// 标记某个思绪已产生独白（会衰减强度）
    pub fn mark_monologue_done(&mut self, thought_key: &str) {
        if let Some(t) = self.thoughts.iter_mut().find(|t| t.thought_key == thought_key) {
            t.mark_monologue();
        }
        self.thoughts.retain(|t| t.phase != ThoughtPhase::Faded || t.intensity > 0.03);
    }

    /// 标记某个思绪已主动表达
    pub fn mark_expressed(&mut self, thought_key: &str, now: f64) {
        if let Some(t) = self.thoughts.iter_mut().find(|t| t.thought_key == thought_key) {
            t.mark_expressed(now);
        }
        self.thoughts.retain(|t| t.phase != ThoughtPhase::Faded || t.intensity > 0.03);
    }

    /// 获取当前最强烈的思绪（用于 UI 展示 Current Thought 状态，不消费）
    pub fn dominant_thought(&self) -> Option<&ActiveThought> {
        self.thoughts
            .iter()
            .filter(|t| t.intensity >= INNER_MONOLOGUE_THRESHOLD)
            .max_by(|a, b| a.intensity.partial_cmp(&b.intensity).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// 构建注入 prompt 的思绪上下文
    pub fn build_context_hint(&self) -> String {
        let active: Vec<&ActiveThought> = self
            .thoughts
            .iter()
            .filter(|t| t.intensity >= INNER_MONOLOGUE_THRESHOLD && !t.expressed)
            .collect();
        if active.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for t in active.iter().take(2) {
            let phase_str = match t.phase {
                ThoughtPhase::Seed => "刚刚冒出来的念头",
                ThoughtPhase::Growing => "心里一直在想的事",
                ThoughtPhase::Active => "很想说出来的话",
                ThoughtPhase::Expressed => "已经说过了",
                ThoughtPhase::Faded => "正在淡忘",
            };
            parts.push(format!("- [{}] {}（强度{:.0}%）", phase_str, t.context_hint, t.intensity * 100.0));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("## 你心里正在想的事\n{}", parts.join("\n"))
        }
    }

    fn evict_faintest(&mut self) {
        if self.thoughts.len() <= MAX_CONCURRENT_THOUGHTS {
            return;
        }
        if let Some(min_idx) = self.thoughts
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.intensity.partial_cmp(&b.intensity).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
        {
            self.thoughts.remove(min_idx);
        }
    }
}

impl Default for ThoughtLifecycle {
    fn default() -> Self {
        Self::new()
    }
}
