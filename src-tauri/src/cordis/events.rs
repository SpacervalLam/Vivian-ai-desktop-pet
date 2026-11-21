//! Cordis 式事件总线（Rust 版）
//!
//! 提供三种分发语义（对应 Cordis 的 EventsService）：
//! - `waterfall`：监听器按外层→内层顺序串联处理同一份载荷，只有当一个
//!   监听器返回 `Some(payload)` 时传给下一个；返回 `None` 表示否决（veto），
//!   可中止整条链。这是 pre/post-execute、approval、sandbox 等策略无侵入
//!   介入工具执行的基础。
//! - `serial`：所有监听器按注册顺序串行收到同一份载荷副本（仅观察，可否决
//!   的修改交给 waterfall）。
//! - `parallel`：所有监听器并行收到一份拷贝，并发执行并等待全部完成。
//!
//! 每个事件总线按载荷类型 `C` 类型化。注册返回 [`super::disposer::Disposer`]，
//! 随生命周期自动反注册（可逆 effect）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

use super::disposer::Disposer;

/// 异步回调的 `boxed` 形式（避免手写 `Box::pin` 样板）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 瀑布（waterfall）监听器：处理载荷，返回 `Some(next)` 继续链，`None` 否决。
pub type WaterfallHandler<C> = Arc<dyn Fn(C) -> BoxFuture<'static, Option<C>> + Send + Sync>;

/// 串行/并行监听器：处理一份载荷副本，不返回（观察语义）。
pub type BroadcastHandler<C> = Arc<dyn Fn(C) -> BoxFuture<'static, ()> + Send + Sync>;

/// 事件载荷约束：`Send + Sync + 'static`，可被访问器替换。
pub trait EventPayload: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> EventPayload for T {}

/// 类型化事件总线。
pub struct EventBus<C: EventPayload> {
    waterfall: RwLock<Vec<(u64, WaterfallHandler<C>)>>,
    broadcast: RwLock<Vec<(u64, BroadcastHandler<C>)>>,
    next_id: AtomicU64,
}

impl<C: EventPayload> Default for EventBus<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: EventPayload> EventBus<C> {
    pub fn new() -> Self {
        Self {
            waterfall: RwLock::new(Vec::new()),
            broadcast: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 注册瀑布监听器。返回的 [`Disposer`] drop 时按 id 精确注销该监听器。
    pub fn on_waterfall<F>(self: &Arc<Self>, handler: F) -> Disposer
    where
        F: Fn(C) -> BoxFuture<'static, Option<C>> + Send + Sync + 'static,
    {
        let id = self.alloc_id();
        self.waterfall.write().push((id, Arc::new(handler)));
        let bus = Arc::clone(self);
        Disposer::new(move || bus.remove(id))
    }

    /// 注册串行/并行监听器（观察语义，不能否决）。
    pub fn on_broadcast<F>(self: &Arc<Self>, handler: F) -> Disposer
    where
        F: Fn(C) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        let id = self.alloc_id();
        self.broadcast.write().push((id, Arc::new(handler)));
        let bus = Arc::clone(self);
        Disposer::new(move || bus.remove(id))
    }

    fn remove(&self, id: u64) {
        self.waterfall.write().retain(|(i, _)| *i != id);
        self.broadcast.write().retain(|(i, _)| *i != id);
    }

    /// 瀑布分发：按注册顺序处理同一份载荷；任一监听器返回 `None` 即否决并返回 `None`。
    pub async fn emit_waterfall(&self, mut payload: C) -> Option<C> {
        let snapshot: Vec<WaterfallHandler<C>> = self
            .waterfall
            .read()
            .iter()
            .map(|(_, h)| Arc::clone(h))
            .collect();
        for handler in snapshot {
            payload = handler(payload).await?;
        }
        Some(payload)
    }

    /// 串行分发：按注册顺序处理一份副本（观察语义）。
    pub async fn emit_serial(&self, payload: C)
    where
        C: Clone,
    {
        let snapshot: Vec<BroadcastHandler<C>> = self
            .broadcast
            .read()
            .iter()
            .map(|(_, h)| Arc::clone(h))
            .collect();
        for handler in snapshot {
            handler(payload.clone()).await;
        }
    }

    /// 并行分发：每个监听器拿一份拷贝，并发执行并等待全部完成。
    pub async fn emit_parallel(&self, payload: C)
    where
        C: Clone,
    {
        let snapshot: Vec<BroadcastHandler<C>> = self
            .broadcast
            .read()
            .iter()
            .map(|(_, h)| Arc::clone(h))
            .collect();
        if snapshot.is_empty() {
            return;
        }
        let mut futs = Vec::with_capacity(snapshot.len());
        for handler in snapshot {
            let item = payload.clone();
            futs.push(handler(item));
        }
        futures::future::join_all(futs).await;
    }

    /// 当前注册的瀑布监听器数量。
    pub fn waterfall_len(&self) -> usize {
        self.waterfall.read().len()
    }

    /// 当前注册的广播监听器数量。
    pub fn broadcast_len(&self) -> usize {
        self.broadcast.read().len()
    }
}