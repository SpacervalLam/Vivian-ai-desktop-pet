//! 可逆注册（Reversible Effect）
//!
//! Cordis 的核心不变量之一：一切注册都是"可逆 effect"，随其所属 fiber/作用域
//! 卸载自动反注册。Rust 版用 [`Disposer`] 表达这一点——携带一个在 drop 时恰好
//! 执行一次的清理闭包。克隆时共享同一"是否已执行"标志，只有最后一个被 drop 的
//! 副本真正执行清理，保证幂等。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type Cleanup = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Disposer {
    active: Arc<AtomicBool>,
    cleanup: Option<Cleanup>,
}

impl Disposer {
    pub fn new<F>(cleanup: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            cleanup: Some(Arc::new(cleanup)),
        }
    }

    /// 立即执行清理并标记为已执行；幂等，重复调用无副作用。
    pub fn dispose(&self) {
        if self.active.swap(false, Ordering::SeqCst) {
            if let Some(cleanup) = &self.cleanup {
                cleanup();
            }
        }
    }

    /// 是否仍处于活跃（未清理）状态。
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
}

impl Drop for Disposer {
    fn drop(&mut self) {
        if self.active.swap(false, Ordering::SeqCst) {
            if let Some(cleanup) = &self.cleanup {
                cleanup();
            }
        }
    }
}

impl std::fmt::Debug for Disposer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Disposer")
            .field("active", &self.is_active())
            .finish()
    }
}