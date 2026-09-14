//! Speech Planner — 言语调度层
//!
//! 在 Brain/TtsManager 之间插入统一调度器,负责:
//! - Priority 仲裁:谁先说、谁让路、谁打断谁
//! - 队列管理:多 intent 串行播放,支持插队
//! - 多角色协调:全局单例,跨角色仲裁(替代 TtsManager 内部的 generation 计数器互斥)
//!
//! Brain 以后只产出 SpeakIntent,不再直接接触 TtsManager。
//! TtsManager 保持不变,由 Planner 调用其 synthesize/play 接口。
//!
//! 核心设计:
//! - submit(intent) 立即返回 SubmitHandle,不阻塞调用方
//! - SubmitHandle.done() 在该 intent 真正播放完成后 resolve
//! - 前端 speak_text 命令 await handle.done(),保持与旧接口兼容(播放完成才返回)
//! - play_now 在独立 spawn 的任务中执行,完成后自动 pump 队列
//! - stop_speaker 会 resolve 被取消的 handle(避免前端永远阻塞)

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};

use crate::error::VivianResult;

use super::tts::TtsManager;

/// Planner 事件 — 由 Planner 在调度时机发射,前端层转成 tauri 事件
///
/// Phase 4: 统一 presentation:* 时序,替代分散的 tts:started / expression:change / motion:play
pub enum PlannerEvent {
    /// 某角色开始说话(真正开始播放,非入队时刻)
    Start {
        speaker_id: String,
        presentation: Presentation,
        text: String,
    },
    /// 某角色停止说话(正常完成 / 被打断 / 主动停止)
    Stop {
        speaker_id: String,
    },
}

/// Planner 事件回调类型
pub type PlannerEventCallback = Arc<dyn Fn(PlannerEvent) + Send + Sync>;

/// 言语优先级 — 决定调度策略
///
/// 调度规则(当前播放 vs 新 intent):
/// - Interrupt: 立即停止当前,播放新 intent
/// - Urgent:    停止当前,播放新 intent(语义上表示"抢话")
/// - Normal:    入队,当前播放完成后顺次播放
/// - Background: 让路;若有任何其他角色正在说话则丢弃(主动搭话被压低)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechPriority {
    /// 主动搭话/问候,可被任何更高优先级覆盖
    Background = 0,
    /// 普通对话回复,排队播放
    Normal = 1,
    /// 抢话/插话,停止当前播放立即播放
    Urgent = 2,
    /// 强制打断(用户发新消息触发)
    Interrupt = 3,
}

impl Default for SpeechPriority {
    fn default() -> Self {
        SpeechPriority::Normal
    }
}

/// 表达层 — 统一调度表情/动作/气泡/视线
///
/// Phase 4 会把 presentation:* 事件统一由 Planner 发射;
/// Phase 1 先定义结构,Brain 填充后由 Planner 透传给前端。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Presentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gaze: Option<String>,
    #[serde(default)]
    pub bubble: bool,
    #[serde(default)]
    pub typing_indicator: bool,
}

/// 言语场景 — 影响 Prosody 的场景类型
///
/// 不同场景下智能体的语速/音高/能量应有差异:
/// - Casual: 日常闲聊,语速正常,音高自然
/// - Formal: 正式场景,语速略缓,音高平稳
/// - Intimate: 亲密对话,语速放缓,音高微升,能量降低
/// - Working: 工作状态,语速略快,音高平稳
/// - Sleeping: 睡眠时段,语速极慢,音高降低,能量极低(轻声)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechScene {
    Casual,
    Formal,
    Intimate,
    Working,
    Sleeping,
}

impl Default for SpeechScene {
    fn default() -> Self {
        SpeechScene::Casual
    }
}

/// 言语上下文 — 影响韵律的上下文输入
///
/// Brain 生成 SpeakIntent 时附带当前上下文,
/// Planner/TtsManager 根据 context 在 emotion prosody 之上叠加场景调整。
///
/// 字段说明:
/// - scene: 场景类型,决定基础韵律偏移
/// - energy: 能量级别(0.0-1.0),1.0=精力充沛,0.2=疲倦;影响语速和音高
/// - closeness: 关系亲密度(0.0-1.0),越高语速越缓、音高越柔
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeechContext {
    #[serde(default)]
    pub scene: SpeechScene,
    #[serde(default = "default_energy")]
    pub energy: f64,
    #[serde(default = "default_closeness")]
    pub closeness: f64,
}

fn default_energy() -> f64 {
    0.8
}

fn default_closeness() -> f64 {
    0.5
}

impl SpeechContext {
    /// 根据上下文计算 VoiceProfile 偏移量
    ///
    /// 返回的 VoiceProfile 会叠加到 emotion prosody 之上:
    /// - scene 决定基础 pitch/speed 偏移
    /// - energy 低 → 语速放缓、音高降低
    /// - closeness 高 → 语速放缓、音高微升
    pub fn to_profile_overlay(&self) -> super::tts::VoiceProfile {
        let mut pitch = 0.0_f64;
        let mut speed = 1.0_f64;

        // 场景基础偏移
        match self.scene {
            SpeechScene::Casual => {
                // 默认,无偏移
            }
            SpeechScene::Formal => {
                speed -= 0.05; // 略缓
            }
            SpeechScene::Intimate => {
                speed -= 0.1;
                pitch += 1.0; // 微升
            }
            SpeechScene::Working => {
                speed += 0.05; // 略快
            }
            SpeechScene::Sleeping => {
                speed -= 0.25; // 极慢
                pitch -= 2.0; // 降低
            }
        }

        // 能量影响:低能量 → 放缓、降低
        let energy_factor = self.energy.clamp(0.0, 1.0);
        if energy_factor < 0.8 {
            let drop = (0.8 - energy_factor) * 0.5; // 0-0.4
            speed -= drop * 0.3; // 最多再放缓 12%
            pitch -= drop * 3.0; // 最多再降 1.2 半音
        }

        // 亲密度影响:高亲密 → 放缓、微升
        if self.closeness > 0.6 {
            let rise = (self.closeness - 0.6) * 2.5; // 0-1.0
            speed -= rise * 0.05; // 最多再放缓 5%
            pitch += rise * 0.5; // 最多再升 0.5 半音
        }

        super::tts::VoiceProfile {
            pitch: Some(pitch.round()),
            speed: Some((speed * 100.0).round() / 100.0),
            pause: None,
            energy: None,
        }
    }
}

/// SpeakIntent — Brain 产出的"说话意图"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakIntent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    #[serde(default)]
    pub priority: SpeechPriority,
    #[serde(default = "default_interruptible")]
    pub interruptible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub speaker_id: String,
    #[serde(default)]
    pub presentation: Presentation,
    /// 言语上下文(场景/能量/亲密度),影响 Prosody
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SpeechContext>,
}

fn default_interruptible() -> bool {
    true
}

/// submit 返回的句柄 — await done() 等待播放完成
pub struct SubmitHandle {
    done: oneshot::Receiver<SubmitResult>,
}

/// submit 的最终结果(通过 oneshot 发送给等待方)
#[derive(Debug, Clone)]
pub enum SubmitResult {
    /// 播放完成
    Played,
    /// 被丢弃(Background 让路 / 被高优先级抢占后取消)
    Dropped,
    /// 播放失败
    Failed(String),
}

impl SubmitHandle {
    /// 等待该 intent 播放完成
    pub async fn done(self) -> SubmitResult {
        match self.done.await {
            Ok(r) => r,
            Err(_) => SubmitResult::Failed("planner 通道关闭".into()),
        }
    }
}

/// 队列中的待播放 intent
struct QueuedIntent {
    seq: u64,
    intent: SpeakIntent,
    /// 播放完成后的通知通道
    done_tx: oneshot::Sender<SubmitResult>,
}

/// 正在播放的语音
struct ActiveSpeech {
    speaker_id: String,
    priority: SpeechPriority,
    /// 播放完成后的通知通道
    done_tx: Option<oneshot::Sender<SubmitResult>>,
}

/// Speech Planner — 全局单例
///
/// Phase 3: current 为 HashMap<speaker_id, ActiveSpeech>,允许不同角色并行播放。
/// 同一角色仍然串行(pump loop 跳过正在播放的角色)。
pub struct SpeechPlanner {
    managers: AsyncMutex<HashMap<String, Arc<TtsManager>>>,
    queue: Mutex<Vec<QueuedIntent>>,
    /// 正在播放的语音,按 speaker_id 索引(多角色 overlap)
    current: Mutex<HashMap<String, ActiveSpeech>>,
    seq_counter: Mutex<u64>,
    /// pump 循环的唤醒信号
    pump_notify: Notify,
    /// 事件回调(由 commands 层设置,负责发射 tauri 事件)
    event_callback: Mutex<Option<PlannerEventCallback>>,
}

impl SpeechPlanner {
    pub fn new() -> Self {
        Self {
            managers: AsyncMutex::new(HashMap::new()),
            queue: Mutex::new(Vec::new()),
            current: Mutex::new(HashMap::new()),
            seq_counter: Mutex::new(0),
            pump_notify: Notify::new(),
            event_callback: Mutex::new(None),
        }
    }

    /// 设置事件回调(在 app setup 时调用一次)
    ///
    /// 回调接收 PlannerEvent,由 commands 层转成 tauri 事件发射给前端。
    /// 保持 Planner 不直接依赖 tauri,解耦调度层与表现层。
    pub fn set_event_callback(&self, cb: PlannerEventCallback) {
        *self.event_callback.lock() = Some(cb);
    }

    /// 内部:安全发射事件(回调未设置时静默跳过)
    fn emit_event(&self, event: PlannerEvent) {
        if let Some(cb) = self.event_callback.lock().as_ref() {
            cb(event);
        }
    }

    /// 注册角色的 TtsManager
    pub async fn register(&self, speaker_id: &str, tts: Arc<TtsManager>) {
        let mut map = self.managers.lock().await;
        map.insert(speaker_id.to_string(), tts);
        tracing::debug!("[SpeechPlanner] 注册角色 TTS: {}", speaker_id);
    }

    /// 提交说话意图
    ///
    /// 立即返回 SubmitHandle。调用方可以 await handle.done() 等待播放完成,
    /// 也可以不 await(fire-and-forget)。
    ///
    /// Phase 3 多角色 overlap:
    /// - Interrupt: 停止所有角色播放,清空队列,入队新 intent
    /// - Urgent: 停止所有角色播放,清空低优先级队列,入队新 intent
    /// - Normal: 如果有 Urgent/Interrupt 在播放,入队等待;否则入队(可与其他角色 Normal 并行)
    /// - Background: 总是入队(允许与任何播放并行)
    pub async fn submit(&self, intent: SpeakIntent) -> VivianResult<SubmitHandle> {
        let (tx, rx) = oneshot::channel();
        let handle = SubmitHandle { done: rx };

        tracing::info!(
            "[SpeechPlanner] submit: speaker={} priority={:?} interruptible={} text_len={}",
            intent.speaker_id,
            intent.priority,
            intent.interruptible,
            intent.text.chars().count()
        );

        if intent.text.trim().is_empty() {
            let _ = tx.send(SubmitResult::Dropped);
            return Ok(handle);
        }

        match intent.priority {
            SpeechPriority::Interrupt => {
                // 停止所有角色的播放
                let stopped = self.cancel_all(SubmitResult::Dropped).await;
                if stopped > 0 {
                    tracing::info!(
                        "[SpeechPlanner] Interrupt 打断 {} 个角色的播放",
                        stopped
                    );
                }
                // 清空队列
                self.clear_all_queue();
                // 入队
                self.spawn_play(intent, tx);
            }
            SpeechPriority::Urgent => {
                // 停止所有角色的播放
                let stopped = self.cancel_all(SubmitResult::Dropped).await;
                if stopped > 0 {
                    tracing::info!(
                        "[SpeechPlanner] Urgent 打断 {} 个角色的播放",
                        stopped
                    );
                }
                // 清空低于 Urgent 的队列项
                self.clear_lower_priority(SpeechPriority::Urgent);
                // 入队
                self.spawn_play(intent, tx);
            }
            SpeechPriority::Normal => {
                // 检查是否有 Urgent/Interrupt 正在播放(任何角色)
                let has_higher = self
                    .current
                    .lock()
                    .values()
                    .any(|a| a.priority >= SpeechPriority::Urgent);
                if has_higher {
                    // 入队等待高优先级完成
                    self.enqueue(intent, tx);
                } else {
                    // 入队(pump loop 会立即取出,可与其他角色 Normal 并行)
                    self.spawn_play(intent, tx);
                }
            }
            SpeechPriority::Background => {
                // 总是允许并行播放
                self.spawn_play(intent, tx);
            }
        }

        Ok(handle)
    }

    /// 停止指定角色的播放并清空其队列
    pub async fn stop_speaker(&self, speaker_id: &str) -> VivianResult<()> {
        // 停止 TTS 后端
        let map = self.managers.lock().await;
        if let Some(tts) = map.get(speaker_id) {
            let _ = tts.stop();
        }
        drop(map);

        // 取消当前播放(从 current map 中移除该角色)
        if let Some(active) = self.current.lock().remove(speaker_id) {
            self.emit_event(PlannerEvent::Stop {
                speaker_id: active.speaker_id.clone(),
            });
            if let Some(tx) = active.done_tx {
                let _ = tx.send(SubmitResult::Dropped);
            }
        }

        // 清除队列中该角色的 intent,并通知放弃
        {
            let mut q = self.queue.lock();
            let mut to_notify = Vec::new();
            let mut i = 0;
            while i < q.len() {
                if q[i].intent.speaker_id == speaker_id {
                    let item = q.remove(i);
                    to_notify.push(item.done_tx);
                } else {
                    i += 1;
                }
            }
            drop(q);
            for tx in to_notify {
                let _ = tx.send(SubmitResult::Dropped);
            }
        }

        // Ducking: 角色停止,可能需要恢复其他 Background 语音
        self.update_ducking_for_all().await;

        // 唤醒 pump
        self.pump_notify.notify_one();
        Ok(())
    }

    /// 停止所有播放并清空队列
    pub async fn stop_all(&self) -> VivianResult<()> {
        {
            let map = self.managers.lock().await;
            for tts in map.values() {
                let _ = tts.stop();
            }
        }

        // 取消所有当前播放
        let actives: Vec<ActiveSpeech> = self.current.lock().drain().map(|(_, v)| v).collect();
        for active in actives {
            self.emit_event(PlannerEvent::Stop {
                speaker_id: active.speaker_id.clone(),
            });
            if let Some(tx) = active.done_tx {
                let _ = tx.send(SubmitResult::Dropped);
            }
        }

        // 清空队列
        let to_notify: Vec<_> = self.queue.lock().drain(..).map(|qi| qi.done_tx).collect();
        for tx in to_notify {
            let _ = tx.send(SubmitResult::Dropped);
        }

        // Ducking: 全部停止,恢复音量
        self.update_ducking_for_all().await;

        Ok(())
    }

    /// 某角色是否正在说话
    pub fn is_speaking(&self, speaker_id: &str) -> bool {
        self.current.lock().contains_key(speaker_id)
    }

    /// 任何角色是否正在说话
    pub fn any_speaking(&self) -> bool {
        !self.current.lock().is_empty()
    }

    /// 入队
    fn enqueue(&self, intent: SpeakIntent, done_tx: oneshot::Sender<SubmitResult>) {
        let mut seq = self.seq_counter.lock();
        *seq += 1;
        let seq_val = *seq;
        drop(seq);

        self.queue.lock().push(QueuedIntent {
            seq: seq_val,
            intent,
            done_tx,
        });
    }

    /// 清除低于指定优先级的队列项,并通知放弃
    fn clear_lower_priority(&self, threshold: SpeechPriority) {
        let mut q = self.queue.lock();
        let before = q.len();
        let mut to_notify = Vec::new();
        let mut i = 0;
        while i < q.len() {
            if q[i].intent.priority < threshold {
                let item = q.remove(i);
                to_notify.push(item.done_tx);
            } else {
                i += 1;
            }
        }
        let after = q.len();
        drop(q);
        for tx in to_notify {
            let _ = tx.send(SubmitResult::Dropped);
        }
        if before != after {
            tracing::info!("[SpeechPlanner] 清除低优先级队列: {} → {}", before, after);
        }
    }

    /// 清空所有队列项
    fn clear_all_queue(&self) {
        let to_notify: Vec<_> = self.queue.lock().drain(..).map(|qi| qi.done_tx).collect();
        if !to_notify.is_empty() {
            tracing::info!("[SpeechPlanner] 清空队列: {} 项", to_notify.len());
        }
        for tx in to_notify {
            let _ = tx.send(SubmitResult::Dropped);
        }
    }

    /// 取消所有角色的当前播放(用于 Interrupt/Urgent 打断)
    ///
    /// 返回被停止的数量(用于日志)
    async fn cancel_all(&self, result: SubmitResult) -> usize {
        let mut actives: Vec<ActiveSpeech> = self.current.lock().drain().map(|(_, v)| v).collect();
        if actives.is_empty() {
            return 0;
        }
        let count = actives.len();
        // 停止所有 TTS
        let map = self.managers.lock().await;
        for active in &actives {
            if let Some(tts) = map.get(&active.speaker_id) {
                let _ = tts.stop();
            }
        }
        drop(map);
        // 发射 Stop 事件 + 通知等待方
        for active in actives.drain(..) {
            self.emit_event(PlannerEvent::Stop {
                speaker_id: active.speaker_id.clone(),
            });
            if let Some(tx) = active.done_tx {
                let _ = tx.send(result.clone());
            }
        }
        // Ducking: 所有角色停止,恢复全部音量
        self.update_ducking_for_all().await;
        count
    }

    /// 入队并唤醒 pump 循环
    ///
    /// 统一播放路径:所有 intent(无论优先级)都入队,
    /// pump loop 从队列取出后设置 current、发射 Start、播放、发射 Stop。
    /// 这避免了"spawn_play 设置 current 后 pump loop break 不处理"的遗漏。
    ///
    /// 竞态保护:在入队前检查该角色是否已在播放或已在队列中,
    /// 避免同一角色的相同文本被重复提交导致重复播放。
    fn spawn_play(&self, intent: SpeakIntent, done_tx: oneshot::Sender<SubmitResult>) {
        let speaker_id = intent.speaker_id.clone();
        let text = intent.text.clone();
        
        let in_queue = {
            let q = self.queue.lock();
            q.iter()
                .any(|qi| qi.intent.speaker_id == speaker_id && qi.intent.text == text)
        };
        
        if in_queue {
            tracing::warn!(
                "[SpeechPlanner] 重复提交: speaker={} text=\"{}\" 已在队列中,丢弃",
                speaker_id,
                text
            );
            let _ = done_tx.send(SubmitResult::Dropped);
            return;
        }
        
        self.enqueue(intent, done_tx);
        self.pump_notify.notify_one();
    }

    /// 更新所有正在播放的角色的 ducking 因子
    ///
    /// Phase 3 ducking 规则:
    /// - 当 ≥2 个角色在并行播放,且其中有非 Background 优先级时,
    ///   所有 Background 优先级的角色音量压低到 0.3(让路)
    /// - 其他情况(单角色播放 / 全是 Background)所有角色恢复 1.0
    ///
    /// 调用时机:每次有角色开始/停止播放后调用。
    ///
    /// 注意:parking_lot::MutexGuard 不是 Send,不能跨越 .await。
    /// 这里先在同步 block 内收集信息并释放 guard,再 await 获取 managers。
    async fn update_ducking_for_all(&self) {
        // 步骤1: 同步收集信息(不跨越 await)
        let (total, background_speakers, has_non_background) = {
            let current = self.current.lock();
            let total = current.len();
            let background_speakers: Vec<String> = current
                .iter()
                .filter(|(_, a)| a.priority == SpeechPriority::Background)
                .map(|(id, _)| id.clone())
                .collect();
            let has_non_background = current
                .iter()
                .any(|(_, a)| a.priority != SpeechPriority::Background);
            (total, background_speakers, has_non_background)
        }; // current guard 在此释放

        // 步骤2: async 获取 managers 并应用 ducking
        if total <= 1 {
            let map = self.managers.lock().await;
            for tts in map.values() {
                tts.set_ducking(1.0);
            }
            return;
        }

        let map = self.managers.lock().await;
        if has_non_background {
            // 有非 Background 在播放:duck 所有 Background,其他保持正常
            for id in &background_speakers {
                if let Some(tts) = map.get(id) {
                    tts.set_ducking(0.3);
                }
            }
            for (id, tts) in map.iter() {
                if !background_speakers.contains(id) {
                    tts.set_ducking(1.0);
                }
            }
            if !background_speakers.is_empty() {
                tracing::info!(
                    "[SpeechPlanner] ducking 启用: Background 角色 {:?} 压低到 0.3",
                    background_speakers
                );
            }
        } else {
            // 全是 Background:全部恢复
            for tts in map.values() {
                tts.set_ducking(1.0);
            }
        }
    }

    /// pump 循环 — 持续从队列取出 intent 播放
    ///
    /// Phase 3 多角色 overlap:
    /// 从队列取出 intent 时,如果该角色没有在播放(current map 中无此 speaker_id),
    /// 则 spawn 一个独立的播放任务。不同角色的播放任务并行运行。
    /// 同一角色的 intent 仍然串行(该角色在播放时跳过,等当前播放完成后再取)。
    pub async fn run_pump_loop(self: Arc<Self>) {
        let cancel = crate::utils::cancel_token::cancel_token();
        loop {
            // 等待唤醒或取消信号
            tokio::select! {
                _ = self.pump_notify.notified() => {}
                _ = cancel.cancelled() => {
                    tracing::info!("[SpeechPlanner] pump 循环收到取消信号，退出");
                    return;
                }
            }

            loop {
                // 取出可以播放的 intent(该角色没有在播放)
                let next = {
                    let cur = self.current.lock();
                    let mut q = self.queue.lock();
                    if q.is_empty() {
                        break;
                    }
                    // 按优先级排序
                    q.sort_by(|a, b| {
                        b.intent.priority
                            .cmp(&a.intent.priority)
                            .then(a.seq.cmp(&b.seq))
                    });
                    // 找到第一个该角色没有在播放的 intent
                    let mut idx = None;
                    for (i, qi) in q.iter().enumerate() {
                        if !cur.contains_key(&qi.intent.speaker_id) {
                            idx = Some(i);
                            break;
                        }
                    }
                    match idx {
                        Some(i) => Some(q.remove(i)),
                        None => None,
                    }
                };

                match next {
                    Some(qi) => {
                        // spawn 独立播放任务(并行播放)
                        let self_clone = self.clone();
                        tokio::spawn(async move {
                            self_clone.play_intent(qi).await;
                        });
                    }
                    None => break, // 队列中所有 intent 的角色都在播放,等待
                }
            }
        }
    }

    /// 播放单个 intent(在独立 spawn 的任务中运行)
    ///
    /// 1. 检查 TtsManager 存在且已启用
    /// 2. 设置 current + 发射 Start
    /// 3. 调用 TtsManager 播放(阻塞直到完成)
    /// 4. 移除 current + 发射 Stop
    /// 5. 通知等待方,唤醒 pump 处理下一个
    async fn play_intent(self: Arc<Self>, qi: QueuedIntent) {
        let speaker_id = qi.intent.speaker_id.clone();
        let priority = qi.intent.priority;
        let text = qi.intent.text.clone();
        let emotion = qi.intent.emotion.clone();
        let presentation = qi.intent.presentation.clone();
        let context = qi.intent.context.clone();
        let done_tx = qi.done_tx;

        // 获取 TtsManager(先检查,避免设置 current 后又因未注册而回滚)
        let tts = {
            let map = self.managers.lock().await;
            match map.get(&speaker_id).cloned() {
                Some(t) => t,
                None => {
                    tracing::warn!(
                        "[SpeechPlanner] 角色 {} 未注册 TtsManager,跳过",
                        speaker_id
                    );
                    let _ = done_tx.send(SubmitResult::Failed(format!(
                        "角色 {} 未注册 TtsManager",
                        speaker_id
                    )));
                    self.pump_notify.notify_one();
                    return;
                }
            }
        };

        if !tts.is_enabled() {
            tracing::debug!("[SpeechPlanner] TTS 未启用,跳过");
            let _ = done_tx.send(SubmitResult::Dropped);
            self.pump_notify.notify_one();
            return;
        }

        // 设置 current
        self.current.lock().insert(
            speaker_id.clone(),
            ActiveSpeech {
                speaker_id: speaker_id.clone(),
                priority,
                done_tx: Some(done_tx),
            },
        );

        // Ducking: 新角色开始播放,可能需要压低 Background 语音
        self.update_ducking_for_all().await;

        // 发射 Start 事件(前端转成 presentation:start)
        self.emit_event(PlannerEvent::Start {
            speaker_id: speaker_id.clone(),
            presentation,
            text: text.clone(),
        });

        // 执行播放(阻塞当前任务直到播放完成)
        tracing::debug!(
            "[SpeechPlanner] 开始播放: speaker={} priority={:?} text_len={}",
            speaker_id,
            priority,
            text.chars().count()
        );
        let result = tts
            .speak_with_context(&text, emotion.as_deref(), context.as_ref())
            .await;

        // 移除 current
        let done_tx = self
            .current
            .lock()
            .remove(&speaker_id)
            .and_then(|a| a.done_tx);

        // Ducking: 角色停止播放,可能需要恢复 Background 语音
        self.update_ducking_for_all().await;

        // 发射 Stop 事件(前端转成 presentation:stop)
        self.emit_event(PlannerEvent::Stop {
            speaker_id: speaker_id.clone(),
        });

        // 通知等待方
        if let Some(tx) = done_tx {
            let result = match result {
                Ok(()) => SubmitResult::Played,
                Err(e) => {
                    tracing::warn!("[SpeechPlanner] 播放失败: {}", e);
                    SubmitResult::Failed(e.to_string())
                }
            };
            let _ = tx.send(result);
        }

        // 唤醒 pump 处理下一个
        self.pump_notify.notify_one();
    }
}

impl Default for SpeechPlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局单例
static PLANNER: tokio::sync::OnceCell<Arc<SpeechPlanner>> = tokio::sync::OnceCell::const_new();

/// 获取全局 SpeechPlanner
pub async fn planner() -> &'static Arc<SpeechPlanner> {
    PLANNER
        .get_or_init(|| async { Arc::new(SpeechPlanner::new()) })
        .await
}

/// 启动 pump 循环(在 app setup 时调用一次)
pub async fn start_pump_loop() {
    let p = planner().await.clone();
    tokio::spawn(async move {
        p.run_pump_loop().await;
    });
}

/// 构造 SpeakIntent 的便捷函数
pub fn speak_intent(text: impl Into<String>, speaker_id: impl Into<String>) -> SpeakIntentBuilder {
    SpeakIntentBuilder {
        text: text.into(),
        emotion: None,
        priority: SpeechPriority::Normal,
        interruptible: true,
        session_id: None,
        speaker_id: speaker_id.into(),
        presentation: Presentation::default(),
        context: None,
    }
}

/// SpeakIntent 构造器
pub struct SpeakIntentBuilder {
    text: String,
    emotion: Option<String>,
    priority: SpeechPriority,
    interruptible: bool,
    session_id: Option<String>,
    speaker_id: String,
    presentation: Presentation,
    context: Option<SpeechContext>,
}

impl SpeakIntentBuilder {
    pub fn emotion(mut self, emotion: impl Into<String>) -> Self {
        self.emotion = Some(emotion.into());
        self
    }

    pub fn priority(mut self, priority: SpeechPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn interruptible(mut self, interruptible: bool) -> Self {
        self.interruptible = interruptible;
        self
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn presentation(mut self, presentation: Presentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn context(mut self, context: SpeechContext) -> Self {
        self.context = Some(context);
        self
    }

    pub fn build(self) -> SpeakIntent {
        SpeakIntent {
            text: self.text,
            emotion: self.emotion,
            priority: self.priority,
            interruptible: self.interruptible,
            session_id: self.session_id,
            speaker_id: self.speaker_id,
            presentation: self.presentation,
            context: self.context,
        }
    }
}
