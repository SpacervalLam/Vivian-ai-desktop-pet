//! 状态机
//!
//! 负责宠物状态流转、事件分发与空闲触发。

use super::animation::AnimationManager;
use super::expression::ExpressionManager;
use super::resource_loader::ResourceLoader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// 默认空闲触发最小间隔（毫秒）
pub const DEFAULT_IDLE_INTERVAL_MIN: u64 = 3000;
/// 默认空闲触发最大间隔（毫秒）
pub const DEFAULT_IDLE_INTERVAL_MAX: u64 = 8000;

/// 宠物状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PetState {
    Idle,
    Interacting,
    Panicked,
    Playing,
    AiTalking,
}

impl PetState {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Interacting => "INTERACTING",
            Self::Panicked => "PANICKED",
            Self::Playing => "PLAYING",
            Self::AiTalking => "AI_TALKING",
        }
    }
}

impl std::fmt::Display for PetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// 状态转换条件
pub type TransitionCondition = Arc<dyn Fn(&HashMap<String, serde_json::Value>) -> bool + Send + Sync>;

/// 状态转换
pub struct StateTransition {
    pub from_state: PetState,
    pub to_state: PetState,
    pub condition: TransitionCondition,
}

/// 事件处理器类型
pub type EventHandler = Arc<dyn Fn(HashMap<String, serde_json::Value>) + Send + Sync>;

/// 状态变化回调类型
pub type StateCallback = Arc<dyn Fn(PetState) + Send + Sync>;

/// 状态机内部状态
struct StateMachineInner {
    current_state: PetState,
    previous_state: Option<PetState>,
    state_start_time: Instant,
    idle_timer: Option<tokio::task::JoinHandle<()>>,
    idle_interval_min: u64,
    idle_interval_max: u64,
    event_handlers: HashMap<String, EventHandler>,
    state_callbacks: HashMap<PetState, StateCallback>,
    event_queue: Vec<(String, HashMap<String, serde_json::Value>)>,
    is_active: bool,
    /// 管理器引用（供静态函数访问，启动空闲定时器等）
    animation_manager: Arc<AnimationManager>,
    expression_manager: Arc<ExpressionManager>,
    resource_loader: Arc<ResourceLoader>,
}

impl StateMachineInner {
    fn new(
        animation_manager: Arc<AnimationManager>,
        expression_manager: Arc<ExpressionManager>,
        resource_loader: Arc<ResourceLoader>,
    ) -> Self {
        Self {
            current_state: PetState::Idle,
            previous_state: None,
            state_start_time: Instant::now(),
            idle_timer: None,
            idle_interval_min: DEFAULT_IDLE_INTERVAL_MIN,
            idle_interval_max: DEFAULT_IDLE_INTERVAL_MAX,
            event_handlers: HashMap::new(),
            state_callbacks: HashMap::new(),
            event_queue: Vec::new(),
            is_active: true,
            animation_manager,
            expression_manager,
            resource_loader,
        }
    }
}

/// 状态机 - 管理宠物状态流转与事件分发
pub struct StateMachine {
    animation_manager: Arc<AnimationManager>,
    expression_manager: Arc<ExpressionManager>,
    resource_loader: Arc<ResourceLoader>,
    inner: Arc<parking_lot::RwLock<StateMachineInner>>,
}

impl StateMachine {
    pub fn new(
        animation_manager: Arc<AnimationManager>,
        expression_manager: Arc<ExpressionManager>,
        resource_loader: Arc<ResourceLoader>,
    ) -> Self {
        let sm = Self {
            animation_manager: animation_manager.clone(),
            expression_manager: expression_manager.clone(),
            resource_loader: resource_loader.clone(),
            inner: Arc::new(parking_lot::RwLock::new(StateMachineInner::new(
                animation_manager,
                expression_manager,
                resource_loader,
            ))),
        };
        sm.setup_default_event_handlers();
        sm
    }

    /// 启动状态机
    pub fn start(&self) {
        {
            let mut inner = self.inner.write();
            inner.state_start_time = Instant::now();
        }
        self.start_idle_timer();
    }

    /// 停止状态机
    pub fn stop(&self) {
        self.stop_idle_timer();
        self.inner.write().is_active = false;
    }

    /// 设置当前状态
    pub fn set_state(&self, new_state: PetState, force: bool) {
        let callback_to_run = {
            let mut inner = self.inner.write();
            if inner.current_state == new_state && !force {
                return;
            }
            let old_state = inner.current_state;
            inner.previous_state = Some(old_state);
            inner.current_state = new_state;
            inner.state_start_time = Instant::now();
            inner.state_callbacks.get(&new_state).cloned()
        };

        // 触发状态回调
        if let Some(cb) = callback_to_run {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(new_state)));
        }

        // 状态相关的定时器管理
        if new_state == PetState::Idle {
            self.start_idle_timer();
        } else {
            self.stop_idle_timer();
        }
    }

    /// 获取当前状态
    pub fn get_current_state(&self) -> PetState {
        self.inner.read().current_state
    }

    /// 获取上一个状态
    pub fn get_previous_state(&self) -> Option<PetState> {
        self.inner.read().previous_state
    }

    /// 通知事件
    pub fn notify_event(&self, event_name: &str, meta: Option<HashMap<String, serde_json::Value>>) {
        let meta = meta.unwrap_or_default();
        let handler = self.inner.read().event_handlers.get(event_name).cloned();
        if let Some(handler) = handler {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handler(meta);
            }));
        } else {
            self.inner
                .write()
                .event_queue
                .push((event_name.to_string(), meta));
        }
    }

    /// 设置状态变化回调
    pub fn on_state_change(&self, state: PetState, callback: StateCallback) {
        self.inner.write().state_callbacks.insert(state, callback);
    }

    /// 注册事件处理器
    pub fn register_event_handler(&self, event_name: &str, handler: EventHandler) {
        self.inner
            .write()
            .event_handlers
            .insert(event_name.to_string(), handler);
    }

    /// 设置空闲间隔
    pub fn set_idle_interval(&self, min_ms: u64, max_ms: u64) {
        let mut inner = self.inner.write();
        inner.idle_interval_min = min_ms;
        inner.idle_interval_max = max_ms;
    }

    /// 触发随机空闲动作
    pub fn trigger_random_idle_action(&self) {
        let inner = self.inner.read();
        if !inner.is_active {
            return;
        }
        if inner.current_state != PetState::Idle {
            return;
        }
        drop(inner);

        if self.expression_manager.get_current_expression().as_deref() == Some("speechless") {
            return;
        }

        // 随机选择动作或表情
        let pick = random_u64() % 2;
        if pick == 0 {
            // 动作
            if let Some(motion) = self.resource_loader.get_random_motion() {
                self.animation_manager
                    .play_motion(&motion.name, 0, true, false);
                self.set_state(PetState::Playing, false);
            }
        } else {
            // 表情
            if let Some(expression) = self.resource_loader.get_random_expression() {
                let duration = random_range(2000, 5000);
                self.expression_manager
                    .set_expression(&expression.name, Some(duration), false, 0);
            }
        }
    }

    /// 是否活跃
    pub fn is_active(&self) -> bool {
        self.inner.read().is_active
    }

    /// 获取状态持续时间（秒）
    pub fn get_state_duration(&self) -> f64 {
        let inner = self.inner.read();
        inner.state_start_time.elapsed().as_secs_f64()
    }

    /// 获取统计信息
    pub fn get_statistics(&self) -> StateMachineStatistics {
        let inner = self.inner.read();
        StateMachineStatistics {
            current_state: inner.current_state.to_string(),
            previous_state: inner.previous_state.map(|s| s.to_string()),
            is_active: inner.is_active,
            event_queue_size: inner.event_queue.len(),
            idle_interval: format!(
                "{}-{}ms",
                inner.idle_interval_min, inner.idle_interval_max
            ),
        }
    }

    /// 设置默认事件处理器
    fn setup_default_event_handlers(&self) {
        // click
        {
            let exp = self.expression_manager.clone();
            let inner = self.inner.clone();
            self.register_event_handler(
                "click",
                Arc::new(move |meta| {
                    Self::handle_click(&exp, &inner, &meta);
                }),
            );
        }

        // double_click
        {
            let exp = self.expression_manager.clone();
            self.register_event_handler(
                "double_click",
                Arc::new(move |_meta| {
                    if exp.get_current_expression().as_deref() == Some("speechless") {
                        return;
                    }
                    exp.set_expression("speechless", Some(2000), false, 0);
                }),
            );
        }

        // panic
        {
            let inner = self.inner.clone();
            let exp = self.expression_manager.clone();
            self.register_event_handler(
                "panic",
                Arc::new(move |meta| {
                    let duration = meta
                        .get("duration")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(3000);
                    let inner_clone = inner.clone();
                    let exp_clone = exp.clone();
                    let handle = spawn_timer(duration, move || {
                        on_panic_end(inner_clone, exp_clone);
                    });
                    if let Some(h) = handle {
                        // 保存 panic 定时器到 idle_timer 字段（复用）
                        // 注意：这里只是确保句柄被持有，避免提前释放
                        inner.write().idle_timer = Some(h);
                    }
                }),
            );
        }

        // ai_response
        {
            let exp = self.expression_manager.clone();
            let inner = self.inner.clone();
            self.register_event_handler(
                "ai_response",
                Arc::new(move |meta| {
                    let text = meta
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !text.is_empty() {
                        let duration_ms = (text.chars().count() as u64) * 100;
                        exp.set_expression("shy", Some(duration_ms), false, 0);
                        // 切换到 AI 说话状态
                        let sm_inner = inner.clone();
                        set_state_static(sm_inner, PetState::AiTalking);
                    }
                }),
            );
        }

        // motion_end
        {
            let inner = self.inner.clone();
            self.register_event_handler(
                "motion_end",
                Arc::new(move |meta| {
                    let interrupted = meta
                        .get("interrupted")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !interrupted {
                        set_state_static(inner.clone(), PetState::Idle);
                    }
                }),
            );
        }

        // mouse_enter
        {
            let inner = self.inner.clone();
            self.register_event_handler(
                "mouse_enter",
                Arc::new(move |_meta| {
                    let _ = inner.read().current_state;
                    // 保持空实现
                }),
            );
        }

        // mouse_leave
        {
            let inner = self.inner.clone();
            self.register_event_handler(
                "mouse_leave",
                Arc::new(move |_meta| {
                    let current = inner.read().current_state;
                    if current == PetState::Interacting {
                        set_state_static(inner.clone(), PetState::Idle);
                    }
                }),
            );
        }
    }

    /// 处理点击事件
    fn handle_click(
        expression_manager: &ExpressionManager,
        inner: &Arc<parking_lot::RwLock<StateMachineInner>>,
        meta: &HashMap<String, serde_json::Value>,
    ) {
        if expression_manager
            .get_current_expression()
            .as_deref()
            == Some("speechless")
        {
            return;
        }

        let click_count = meta
            .get("click_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(1);

        if click_count == 1 {
            expression_manager.set_expression("shy", Some(3000), false, 0);
            set_state_static(inner.clone(), PetState::Interacting);
        } else if click_count == 3 {
            expression_manager.set_expression("cry", Some(5000), false, 0);
            set_state_static(inner.clone(), PetState::Interacting);
        } else if click_count >= 5 {
            expression_manager.set_expression("sweat", None, false, 0);
            set_state_static(inner.clone(), PetState::Panicked);
            // 通知 panic 事件
            let mut panic_meta = HashMap::new();
            panic_meta.insert(
                "duration".to_string(),
                serde_json::Value::Number(serde_json::Number::from(3000u64)),
            );
            notify_event_static(inner.clone(), "panic", panic_meta);
        }
    }

    /// 启动空闲定时器
    fn start_idle_timer(&self) {
        start_idle_timer_static(
            self.inner.clone(),
            self.animation_manager.clone(),
            self.expression_manager.clone(),
            self.resource_loader.clone(),
        );
    }

    /// 停止空闲定时器
    fn stop_idle_timer(&self) {
        if let Some(handle) = self.inner.write().idle_timer.take() {
            handle.abort();
        }
    }
}

/// 状态机统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMachineStatistics {
    pub current_state: String,
    pub previous_state: Option<String>,
    pub is_active: bool,
    pub event_queue_size: usize,
    pub idle_interval: String,
}

/// 启动空闲定时器（静态版本，供事件处理器与定时器回调使用）
fn start_idle_timer_static(
    inner: Arc<parking_lot::RwLock<StateMachineInner>>,
    animation_manager: Arc<AnimationManager>,
    expression_manager: Arc<ExpressionManager>,
    resource_loader: Arc<ResourceLoader>,
) {
    // 先停止已有定时器
    if let Some(handle) = inner.write().idle_timer.take() {
        handle.abort();
    }

    let (min_ms, max_ms) = {
        let inner_r = inner.read();
        if inner_r.idle_interval_min == 0 {
            return;
        }
        (inner_r.idle_interval_min, inner_r.idle_interval_max)
    };

    let interval = random_range(min_ms, max_ms);
    let inner_for_closure = inner.clone();
    let handle = spawn_timer(interval, move || {
        on_idle_timer(
            inner_for_closure,
            animation_manager,
            expression_manager,
            resource_loader,
        );
    });
    if let Some(h) = handle {
        inner.write().idle_timer = Some(h);
    }
}

/// 空闲定时器回调
///
/// 当处于 Idle 状态时调用
/// `trigger_random_idle_action` 的等价逻辑（随机播放动作或表情）。
fn on_idle_timer(
    inner: Arc<parking_lot::RwLock<StateMachineInner>>,
    animation_manager: Arc<AnimationManager>,
    expression_manager: Arc<ExpressionManager>,
    resource_loader: Arc<ResourceLoader>,
) {
    let (is_active, is_idle) = {
        let inner_r = inner.read();
        (inner_r.is_active, inner_r.current_state == PetState::Idle)
    };
    if !is_active || !is_idle {
        return;
    }

    debug!("[StateMachine] 空闲定时器触发");

    // speechless 表情时不触发空闲动作
    if expression_manager
        .get_current_expression()
        .as_deref()
        == Some("speechless")
    {
        // 仍然处于 Idle，安排下一次空闲定时器
        start_idle_timer_static(inner, animation_manager, expression_manager, resource_loader);
        return;
    }

    // 随机选择动作或表情
    let pick = random_u64() % 2;
    if pick == 0 {
        // 播放随机动作
        if let Some(motion) = resource_loader.get_random_motion() {
            animation_manager.play_motion(&motion.name, 0, true, false);
            // 动作播放后切换到 Playing 状态（set_state_static 不会启动空闲定时器）
            set_state_static(inner, PetState::Playing);
            return;
        }
    } else {
        // 设置随机临时表情
        if let Some(expression) = resource_loader.get_random_expression() {
            let duration = random_range(2000, 5000);
            expression_manager.set_expression(&expression.name, Some(duration), false, 0);
        }
    }

    // 若未切换状态（例如仅设置表情或资源为空），安排下一次空闲定时器
    let still_idle = inner.read().current_state == PetState::Idle;
    if still_idle {
        start_idle_timer_static(inner, animation_manager, expression_manager, resource_loader);
    }
}

/// panic 结束回调
fn on_panic_end(
    inner: Arc<parking_lot::RwLock<StateMachineInner>>,
    expression_manager: Arc<ExpressionManager>,
) {
    expression_manager.reset_expression();
    set_state_static(inner, PetState::Idle);
}

/// 静态方法：设置状态（用于事件处理器闭包）
fn set_state_static(inner: Arc<parking_lot::RwLock<StateMachineInner>>, new_state: PetState) {
    let callback_to_run = {
        let mut inner_w = inner.write();
        if inner_w.current_state == new_state {
            return;
        }
        let old_state = inner_w.current_state;
        inner_w.previous_state = Some(old_state);
        inner_w.current_state = new_state;
        inner_w.state_start_time = Instant::now();
        inner_w.state_callbacks.get(&new_state).cloned()
    };

    if let Some(cb) = callback_to_run {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(new_state)));
    }

    info!("[StateMachine] 状态切换 -> {}", new_state);

    // 切换到 Idle 状态时重新启动空闲定时器
    if new_state == PetState::Idle {
        let (am, em, rl) = {
            let inner_r = inner.read();
            (
                inner_r.animation_manager.clone(),
                inner_r.expression_manager.clone(),
                inner_r.resource_loader.clone(),
            )
        };
        start_idle_timer_static(inner, am, em, rl);
    }
}

/// 静态方法：通知事件（用于事件处理器内部触发）
fn notify_event_static(
    inner: Arc<parking_lot::RwLock<StateMachineInner>>,
    event_name: &str,
    meta: HashMap<String, serde_json::Value>,
) {
    let handler = inner.read().event_handlers.get(event_name).cloned();
    if let Some(handler) = handler {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler(meta);
        }));
    } else {
        inner
            .write()
            .event_queue
            .push((event_name.to_string(), meta));
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
            warn!("[StateMachine] 无法启动定时器：不在 tokio runtime 上下文中");
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

/// 生成 [min, max) 范围内的伪随机数
fn random_range(min: u64, max: u64) -> u64 {
    if min >= max {
        return min;
    }
    min + random_u64() % (max - min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pet_state_name() {
        assert_eq!(PetState::Idle.name(), "IDLE");
        assert_eq!(PetState::AiTalking.name(), "AI_TALKING");
    }

    #[test]
    fn test_default_intervals() {
        assert_eq!(DEFAULT_IDLE_INTERVAL_MIN, 3000);
        assert_eq!(DEFAULT_IDLE_INTERVAL_MAX, 8000);
    }

    #[test]
    fn test_random_range() {
        for _ in 0..100 {
            let v = random_range(10, 20);
            assert!(v >= 10 && v < 20);
        }
    }
}
