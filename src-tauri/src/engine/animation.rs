//! 动画管理器
//!
//! 负责动作优先级、动作队列与播放控制。

use super::motion_player::MotionPlayer;
use super::resource_loader::ResourceLoader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// 动作优先级（5 级）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum MotionPriority {
    Idle = 0,
    Low = 10,
    Normal = 50,
    High = 100,
    Critical = 200,
}

impl MotionPriority {
    pub fn value(self) -> u32 {
        self as u32
    }

    /// 从数值构造优先级（不在已知档位时按大小归类到最近档位）
    pub fn from_value(v: u32) -> Self {
        match v {
            0 => Self::Idle,
            1..=10 => Self::Low,
            11..=50 => Self::Normal,
            51..=100 => Self::High,
            _ => Self::Critical,
        }
    }
}

/// 动作事件回调参数
pub type MotionEventArgs = HashMap<String, serde_json::Value>;

/// 动作事件回调类型
pub type MotionCallback = Arc<dyn Fn(MotionEventArgs) + Send + Sync>;

/// 动作状态
#[derive(Clone)]
pub struct MotionState {
    pub name: String,
    pub priority: u32,
    pub motion_path: String,
    pub start_time: Instant,
    pub duration: f64,
    pub interruptible: bool,
    pub is_looping: bool,
    callbacks: HashMap<String, MotionCallback>,
}

impl std::fmt::Debug for MotionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotionState")
            .field("name", &self.name)
            .field("priority", &self.priority)
            .field("duration", &self.duration)
            .field("interruptible", &self.interruptible)
            .field("is_looping", &self.is_looping)
            .finish()
    }
}

impl MotionState {
    pub fn new(name: impl Into<String>, priority: u32, motion_path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            priority,
            motion_path: motion_path.into(),
            start_time: Instant::now(),
            duration: 0.0,
            interruptible: true,
            is_looping: false,
            callbacks: HashMap::new(),
        }
    }

    /// 设置动作开始回调，返回自身便于链式调用
    pub fn on_start(mut self, callback: MotionCallback) -> Self {
        self.callbacks.insert("on_start".to_string(), callback);
        self
    }

    /// 设置动作结束回调，返回自身便于链式调用
    pub fn on_end(mut self, callback: MotionCallback) -> Self {
        self.callbacks.insert("on_end".to_string(), callback);
        self
    }

    /// 设置动作循环回调，返回自身便于链式调用
    pub fn on_loop(mut self, callback: MotionCallback) -> Self {
        self.callbacks.insert("on_loop".to_string(), callback);
        self
    }

    /// 触发指定事件回调
    pub fn trigger_callback(&self, event: &str, args: MotionEventArgs) {
        if let Some(cb) = self.callbacks.get(event) {
            // 回调内部异常不影响主流程
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(args)));
        }
    }
}

/// 动画管理器内部状态
struct AnimationManagerInner {
    current_motion: Option<MotionState>,
    motion_queue: Vec<MotionState>,
    is_playing: bool,
    last_motion_name: Option<String>,
    motion_count: u64,
    /// 非循环动作的自动结束定时器句柄
    motion_timer: Option<tokio::task::JoinHandle<()>>,
}

impl AnimationManagerInner {
    fn new() -> Self {
        Self {
            current_motion: None,
            motion_queue: Vec::new(),
            is_playing: false,
            last_motion_name: None,
            motion_count: 0,
            motion_timer: None,
        }
    }
}

/// 动画管理器 - 控制动作播放、优先级与队列
pub struct AnimationManager {
    resource_loader: Arc<ResourceLoader>,
    inner: Arc<parking_lot::RwLock<AnimationManagerInner>>,
    motion_player: Arc<MotionPlayer>,
}

impl AnimationManager {
    pub fn new(resource_loader: Arc<ResourceLoader>) -> Self {
        Self {
            resource_loader,
            inner: Arc::new(parking_lot::RwLock::new(AnimationManagerInner::new())),
            motion_player: Arc::new(MotionPlayer::new()),
        }
    }

    /// 获取共享的 MotionPlayer（供外部驱动每帧取值）
    pub fn motion_player(&self) -> Arc<MotionPlayer> {
        self.motion_player.clone()
    }

    /// 播放动作，返回是否成功启动
    pub fn play_motion(
        &self,
        name: &str,
        priority: u32,
        interruptible: bool,
        r#loop: bool,
    ) -> Option<MotionState> {
        let motion_info = self.resource_loader.get_motion(name)?;
        let mut new_motion = MotionState::new(name, priority, &motion_info.path);
        new_motion.duration = motion_info.duration;
        new_motion.is_looping = r#loop || motion_info.r#loop;
        new_motion.interruptible = interruptible;

        // 检查是否可以打断当前动作
        let need_queue = {
            let inner = self.inner.read();
            if let Some(current) = &inner.current_motion {
                if !self.can_interrupt(current.priority, priority) {
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if need_queue {
            if interruptible {
                self.inner.write().motion_queue.push(new_motion.clone());
                debug!(
                    "[AnimationManager] 动作 '{}' 已加入队列 (优先级: {})",
                    name, priority
                );
            } else {
                debug!(
                    "[AnimationManager] 无法播放动作 '{}': 当前有更高优先级动作",
                    name
                );
            }
            return None;
        }

        // 打断当前动作
        let need_interrupt = {
            let inner = self.inner.read();
            inner.current_motion.is_some()
        };
        if need_interrupt {
            let mut inner = self.inner.write();
            if let Some(current) = inner.current_motion.take() {
                debug!(
                    "[AnimationManager] 打断当前动作 '{}' -> 播放 '{}'",
                    current.name, name
                );
                let mut args = MotionEventArgs::new();
                args.insert("interrupted".to_string(), serde_json::Value::Bool(true));
                current.trigger_callback("on_end", args);
            }
            self.stop_current_motion_locked(&mut inner);
        }

        self.start_motion(new_motion.clone());
        Some(new_motion)
    }

    /// 随机播放动作
    pub fn play_random_motion(&self, min_priority: u32, max_priority: u32) -> Option<MotionState> {
        let motions = self.resource_loader.get_all_motions();
        if motions.is_empty() {
            return None;
        }

        let valid: Vec<String> = motions
            .iter()
            .filter(|(_, info)| {
                let p = MotionPriority::from_value(50).value(); // 默认 NORMAL
                let info_p = if info.duration > 0.0 { p } else { p };
                min_priority <= info_p && info_p <= max_priority
            })
            .map(|(name, _)| name.clone())
            .collect();

        if valid.is_empty() {
            return None;
        }

        let idx = random_u64() as usize % valid.len();
        self.play_motion(&valid[idx], MotionPriority::Normal.value(), true, false)
    }

    /// 停止当前动作
    pub fn stop_motion(&self, force: bool) -> bool {
        let mut inner = self.inner.write();
        if inner.current_motion.is_none() {
            return false;
        }

        if let Some(current) = &inner.current_motion {
            if !force && !current.interruptible {
                debug!(
                    "[AnimationManager] 无法停止动作 '{}': 不可中断",
                    current.name
                );
                return false;
            }
        }

        let current = inner.current_motion.take().unwrap();
        let mut args = MotionEventArgs::new();
        args.insert("interrupted".to_string(), serde_json::Value::Bool(false));
        current.trigger_callback("on_end", args);

        self.stop_current_motion_locked(&mut inner);

        // 锁外处理队列
        drop(inner);
        self.process_queue();
        true
    }

    /// 停止所有动作
    pub fn stop_all_motions(&self) {
        loop {
            let has_current = self.inner.read().current_motion.is_some();
            let queue_len = self.inner.read().motion_queue.len();
            if !has_current && queue_len == 0 {
                break;
            }
            if !self.stop_motion(true) {
                break;
            }
        }
        self.inner.write().motion_queue.clear();
    }

    /// 是否正在播放
    pub fn is_playing(&self, motion_name: Option<&str>) -> bool {
        let inner = self.inner.read();
        match motion_name {
            Some(name) => inner
                .current_motion
                .as_ref()
                .map(|m| m.name == name)
                .unwrap_or(false),
            None => inner.current_motion.is_some(),
        }
    }

    /// 获取当前动作的克隆
    pub fn get_current_motion(&self) -> Option<MotionState> {
        self.inner.read().current_motion.clone()
    }

    /// 获取动作队列的克隆
    pub fn get_motion_queue(&self) -> Vec<MotionState> {
        self.inner.read().motion_queue.clone()
    }

    /// 为指定动作设置结束回调（不立即播放）
    pub fn on_motion_end(
        &self,
        motion_name: &str,
        callback: MotionCallback,
        priority: u32,
    ) -> Option<MotionState> {
        let motion_info = self.resource_loader.get_motion(motion_name)?;
        Some(MotionState::new(motion_name, priority, &motion_info.path).on_end(callback))
    }

    /// 优先级打断判定
    fn can_interrupt(&self, current_priority: u32, new_priority: u32) -> bool {
        new_priority > current_priority
    }

    /// 启动动作
    fn start_motion(&self, motion: MotionState) {
        {
            let mut inner = self.inner.write();
            // 取消旧的定时器
            if let Some(handle) = inner.motion_timer.take() {
                handle.abort();
            }
            inner.current_motion = Some(motion.clone());
            inner.is_playing = true;
            inner.last_motion_name = Some(motion.name.clone());
            inner.motion_count += 1;
        }

        info!(
            "[AnimationManager] 开始播放动作: {} (优先级: {})",
            motion.name, motion.priority
        );
        motion.trigger_callback("on_start", MotionEventArgs::new());

        if self.motion_player.load_motion(&motion.motion_path) {
            self.motion_player
                .play(motion.is_looping, None);
            debug!(
                "[AnimationManager] 动作播放器已启动: {}",
                motion.name
            );
        } else {
            warn!(
                "[AnimationManager] 无法加载动作文件: {}",
                motion.motion_path
            );
        }

        // 非循环动作启动自动结束定时器
        if !motion.is_looping && motion.duration > 0.0 {
            let duration_ms = (motion.duration * 1000.0) as u64;
            let inner = self.inner.clone();
            let motion_player = self.motion_player.clone();
            let handle = spawn_timer(duration_ms, move || {
                on_motion_timer_end(inner, motion_player);
            });
            if let Some(h) = handle {
                self.inner.write().motion_timer = Some(h);
            }
        }
    }

    /// 停止当前动作（调用方需持锁）
    fn stop_current_motion_locked(&self, inner: &mut AnimationManagerInner) {
        self.motion_player.stop();
        inner.current_motion = None;
        inner.is_playing = false;
        if let Some(handle) = inner.motion_timer.take() {
            handle.abort();
        }
    }

    /// 处理动作队列
    fn process_queue(&self) {
        let next_motion = {
            let mut inner = self.inner.write();
            if inner.current_motion.is_some() || inner.motion_queue.is_empty() {
                return;
            }
            // 按优先级降序排序
            inner.motion_queue.sort_by(|a, b| b.priority.cmp(&a.priority));
            inner.motion_queue.remove(0)
        };

        debug!(
            "[AnimationManager] 从队列播放动作: {}",
            next_motion.name
        );
        self.start_motion(next_motion);
    }

    /// 获取统计信息
    pub fn get_statistics(&self) -> AnimationStatistics {
        let inner = self.inner.read();
        AnimationStatistics {
            is_playing: inner.is_playing,
            current_motion: inner.current_motion.as_ref().map(|m| m.name.clone()),
            queue_length: inner.motion_queue.len(),
            total_motions_played: inner.motion_count,
            last_motion: inner.last_motion_name.clone(),
        }
    }

    /// 清空统计信息
    pub fn clear_statistics(&self) {
        let mut inner = self.inner.write();
        inner.motion_count = 0;
        inner.last_motion_name = None;
    }
}

/// 动画管理器统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationStatistics {
    pub is_playing: bool,
    pub current_motion: Option<String>,
    pub queue_length: usize,
    pub total_motions_played: u64,
    pub last_motion: Option<String>,
}

/// 动作定时器结束回调
fn on_motion_timer_end(
    inner: Arc<parking_lot::RwLock<AnimationManagerInner>>,
    motion_player: Arc<MotionPlayer>,
) {
    let current_motion = {
        let mut inner_w = inner.write();
        if let Some(handle) = inner_w.motion_timer.take() {
            handle.abort();
        }
        match inner_w.current_motion.take() {
            Some(m) => {
                inner_w.is_playing = false;
                m
            }
            None => return,
        }
    };

    let mut args = MotionEventArgs::new();
    args.insert("completed".to_string(), serde_json::Value::Bool(true));
    current_motion.trigger_callback("on_end", args);
    debug!(
        "[AnimationManager] 动作播放完成: {}",
        current_motion.name
    );

    if current_motion.is_looping {
        // 循环动作重新启动
        restart_motion(inner, motion_player, current_motion);
    } else {
        // 处理队列：启动下一个排队动作
        process_queue_static(inner, motion_player);
    }
}

/// 重启循环动作
fn restart_motion(
    inner: Arc<parking_lot::RwLock<AnimationManagerInner>>,
    motion_player: Arc<MotionPlayer>,
    motion: MotionState,
) {
    {
        let mut inner_w = inner.write();
        inner_w.current_motion = Some(motion.clone());
        inner_w.is_playing = true;
        inner_w.motion_count += 1;
    }
    motion.trigger_callback("on_loop", MotionEventArgs::new());
    motion_player.load_motion(&motion.motion_path);
    motion_player.play(motion.is_looping, None);

    if !motion.is_looping && motion.duration > 0.0 {
        let duration_ms = (motion.duration * 1000.0) as u64;
        let inner_clone = inner.clone();
        let mp_clone = motion_player.clone();
        let handle = spawn_timer(duration_ms, move || {
            on_motion_timer_end(inner_clone, mp_clone);
        });
        if let Some(h) = handle {
            inner.write().motion_timer = Some(h);
        }
    }
}

/// 处理动作队列（静态版本，用于定时器回调）
///
/// 从队列中取出最高优先级的动作并启动。
fn process_queue_static(
    inner: Arc<parking_lot::RwLock<AnimationManagerInner>>,
    motion_player: Arc<MotionPlayer>,
) {
    let next_motion = {
        let mut inner_w = inner.write();
        if inner_w.current_motion.is_some() || inner_w.motion_queue.is_empty() {
            return;
        }
        // 取消旧定时器
        if let Some(handle) = inner_w.motion_timer.take() {
            handle.abort();
        }
        // 按优先级降序排序
        inner_w.motion_queue.sort_by(|a, b| b.priority.cmp(&a.priority));
        inner_w.motion_queue.remove(0)
    };

    debug!(
        "[AnimationManager] 从队列播放动作: {}",
        next_motion.name
    );
    // 启动动作（包含加载、播放与自动结束定时器，对应 start_motion 的完整逻辑）
    start_motion_static(inner, motion_player, next_motion);
}

/// 启动动作（静态版本，对应 AnimationManager::start_motion 的完整逻辑）
///
/// 在定时器回调等无法获取 `&AnimationManager` 的场景中使用：
/// 设置当前动作、触开始回调、加载到 MotionPlayer 并播放、启动自动结束定时器。
fn start_motion_static(
    inner: Arc<parking_lot::RwLock<AnimationManagerInner>>,
    motion_player: Arc<MotionPlayer>,
    motion: MotionState,
) {
    {
        let mut inner_w = inner.write();
        // 取消旧的定时器
        if let Some(handle) = inner_w.motion_timer.take() {
            handle.abort();
        }
        inner_w.current_motion = Some(motion.clone());
        inner_w.is_playing = true;
        inner_w.last_motion_name = Some(motion.name.clone());
        inner_w.motion_count += 1;
    }

    info!(
        "[AnimationManager] 开始播放动作: {} (优先级: {})",
        motion.name, motion.priority
    );
    motion.trigger_callback("on_start", MotionEventArgs::new());

    if motion_player.load_motion(&motion.motion_path) {
        motion_player.play(motion.is_looping, None);
        debug!(
            "[AnimationManager] 动作播放器已启动: {}",
            motion.name
        );
    } else {
        warn!(
            "[AnimationManager] 无法加载动作文件: {}",
            motion.motion_path
        );
    }

    // 非循环动作启动自动结束定时器
    if !motion.is_looping && motion.duration > 0.0 {
        let duration_ms = (motion.duration * 1000.0) as u64;
        let inner_clone = inner.clone();
        let mp_clone = motion_player.clone();
        let handle = spawn_timer(duration_ms, move || {
            on_motion_timer_end(inner_clone, mp_clone);
        });
        if let Some(h) = handle {
            inner.write().motion_timer = Some(h);
        }
    }
}

/// 启动一个可取消的 tokio 定时器
fn spawn_timer(
    duration_ms: u64,
    callback: impl FnOnce() + Send + 'static,
) -> Option<tokio::task::JoinHandle<()>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => Some(handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
            callback();
        })),
        Err(_) => {
            warn!("[AnimationManager] 无法启动定时器：不在 tokio runtime 上下文中");
            None
        }
    }
}

/// 生成一个简易的伪随机 u64（基于系统时间纳秒）
fn random_u64() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_motion_priority_ordering() {
        assert!(MotionPriority::Critical.value() > MotionPriority::High.value());
        assert!(MotionPriority::High.value() > MotionPriority::Normal.value());
        assert!(MotionPriority::Normal.value() > MotionPriority::Low.value());
        assert!(MotionPriority::Low.value() > MotionPriority::Idle.value());
    }

    #[test]
    fn test_motion_state_callbacks() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();
        let cb: MotionCallback = Arc::new(move |_| {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let state = MotionState::new("test", 50, "/tmp/test").on_start(cb);
        state.trigger_callback("on_start", MotionEventArgs::new());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn test_can_interrupt() {
        let loader = Arc::new(ResourceLoader::new("/tmp", "Vivian"));
        let mgr = AnimationManager::new(loader);
        assert!(!mgr.can_interrupt(100, 50));
        assert!(!mgr.can_interrupt(50, 50));
        assert!(mgr.can_interrupt(50, 100));
    }
}
