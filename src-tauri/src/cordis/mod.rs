//! Cordis 式运行时内核（Rust 版）
//!
//! 提炼出的最小原语，供桌宠后端采用
//! "一切皆插件"的架构组织能力：
//!
//! - [`RuntimeContext`]（对应 Cordis 的 `ctx`）：聚合服务注册表 + 类型化事件总线
//!   + 作用域，是插件/服务/能力缝挂载的统一运行时入口；
//! - [`EventBus`]：三种分发语义的事件总线，是 pre/post-execute、approval、
//!   sandbox 等策略无侵入介入的工具；
//! - [`Disposer`]：可逆注册，随作用域/生命周期自动反注册；
//! - [`Scope`]：作用域隔离，多智能体场景下通常一个角色一个作用域。
//!
//! 该模块刻意保持小且自包含，不依赖业务模块，可作为独立能力被逐步接入。

pub mod disposer;
pub mod events;
pub mod scope;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

pub use disposer::Disposer;
pub use events::{BoxFuture, EventBus, EventPayload};
pub use scope::{Scope, ScopeId};

use std::sync::OnceLock;

/// 进程级全局运行时上下文（便于不穿透构造链也能取到共享 ctx）。
///
/// 由 `AppState::new()` 初始化时写入；`None` 表示尚未初始化。
static GLOBAL_CTX: OnceLock<RuntimeContext> = OnceLock::new();

/// 设置进程级全局运行时上下文（进程生命周期内只应设置一次）。
pub fn set_global(ctx: RuntimeContext) {
    let _ = GLOBAL_CTX.set(ctx);
}

/// 读取全局运行时上下文。
pub fn global_ctx() -> Option<RuntimeContext> {
    GLOBAL_CTX.get().cloned()
}

/// 可注册进运行时的服务约束。
pub trait Service: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Service for T {}

/// Cordis 式运行时上下文。
///
/// 内部共享一个 [`RuntimeContextInner`]，`RuntimeContext` 可廉价 clone 分发到
/// 各处；服务与事件总线按类型键存储，访问时自动 downcast。
#[derive(Clone)]
pub struct RuntimeContext {
    inner: Arc<RuntimeContextInner>,
}

struct RuntimeContextInner {
    /// 服务注册表：`TypeId -> Arc<S>`（撑在类型擦除容器里）。
    services: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// 事件总线注册表：`type_name::<C>() -> Arc<EventBus<C>>`。
    event_buses: RwLock<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>,
    /// 可选的作用域标签：当前正在构造的插件/服务所属作用域。
    current_scope: RwLock<Option<ScopeId>>,
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeContext {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RuntimeContextInner {
                services: RwLock::new(HashMap::new()),
                event_buses: RwLock::new(HashMap::new()),
                current_scope: RwLock::new(None),
            }),
        }
    }

    // ---------- 服务注册表 ----------

    /// 注册一个服务（全局作用域）。返回 [`Disposer`]，drop/作用域卸载时自动注销。
    pub fn set_service<S: Service>(&self, value: S) -> Disposer {
        let tid = TypeId::of::<S>();
        self.inner
            .services
            .write()
            .insert(tid, Arc::new(value) as Arc<dyn Any + Send + Sync>);
        let inner = Arc::downgrade(&self.inner);
        Disposer::new(move || {
            if let Some(inner) = inner.upgrade() {
                // 仅删除指向同一实例的注册；若被替换则不动
                inner.services.write().remove(&tid);
            }
        })
    }

    /// 获取一个服务。
    pub fn get_service<S: Service>(&self) -> Option<Arc<S>> {
        let guard = self.inner.services.read();
        guard
            .get(&TypeId::of::<S>())
            .and_then(|b| b.clone().downcast::<S>().ok())
    }

    /// 是否已注册某服务。
    pub fn has_service<S: Service>(&self) -> bool {
        self.inner.services.read().contains_key(&TypeId::of::<S>())
    }

    /// 当前已注册的服务类型数量（调试/统计）。
    pub fn service_count(&self) -> usize {
        self.inner.services.read().len()
    }

    // ---------- 事件总线 ----------

    /// 按载荷类型获取（不存在则创建）类型化事件总线。
    pub fn event_bus<C: EventPayload>(&self) -> Arc<EventBus<C>> {
        let key = std::any::type_name::<C>();
        if let Some(b) = self.inner.event_buses.read().get(key) {
            if let Some(bus) = b.clone().downcast::<EventBus<C>>().ok() {
                return bus;
            }
        }
        let bus = Arc::new(EventBus::<C>::new());
        self.inner
            .event_buses
            .write()
            .insert(key, bus.clone() as Arc<dyn Any + Send + Sync>);
        bus
    }

    /// 对载荷类型 `C` 的瀑布分发。返回 `Some(payload)` 表示全部监听器通过。
    pub async fn emit_waterfall<C: EventPayload>(&self, payload: C) -> Option<C> {
        self.event_bus::<C>().emit_waterfall(payload).await
    }

    /// 对载荷类型 `C` 的串行分发（观察语义）。
    pub async fn emit_serial<C: EventPayload>(&self, payload: C)
    where
        C: Clone,
    {
        self.event_bus::<C>().emit_serial(payload).await;
    }

    /// 对载荷类型 `C` 的并行分发（观察语义）。
    pub async fn emit_parallel<C: EventPayload>(&self, payload: C)
    where
        C: Clone,
    {
        self.event_bus::<C>().emit_parallel(payload).await;
    }

    /// 在事件总线上注册瀑布监听器。
    pub fn on_waterfall<C: EventPayload, F>(&self, handler: F) -> Disposer
    where
        F: Fn(C) -> BoxFuture<'static, Option<C>> + Send + Sync + 'static,
    {
        self.event_bus::<C>().on_waterfall(handler)
    }

    /// 在事件总线上注册广播监听器（观察语义）。
    pub fn on_broadcast<C: EventPayload, F>(&self, handler: F) -> Disposer
    where
        F: Fn(C) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.event_bus::<C>().on_broadcast(handler)
    }

    // ---------- 作用域 ----------

    /// 在当前线程上标记"以下注册归属于某个作用域"。
    ///
    /// 返回一个守卫；调用者应在执行注册后立即 drop 该守卫，或用
    /// [`RuntimeContext::scoped`] 的方式自动处理。
    pub fn enter_scope(&self, scope: &ScopeId) -> ScopeGuard<'_> {
        let prev = self.inner.current_scope.write().replace(scope.clone());
        ScopeGuard {
            ctx: self,
            prev,
        }
    }

    /// 当前上下文所在作用域（None = 全局）。
    pub fn current_scope(&self) -> Option<ScopeId> {
        self.inner.current_scope.read().clone()
    }
}

/// 作用域进入守卫：drop 时恢复进入前的作用域（栈式）。
pub struct ScopeGuard<'a> {
    ctx: &'a RuntimeContext,
    prev: Option<ScopeId>,
}

impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        *self.ctx.inner.current_scope.write() = self.prev.clone();
    }
}