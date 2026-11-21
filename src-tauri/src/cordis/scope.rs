//! 作用域与隔离（Scope / Isolate）
//!
//! 对应 Cordis 的 `isolate()`：服务/监听器可绑定到一个作用域标签上，使得它们
//! 只对某个隔离环境可见、可在该作用域失活时被统一反注册。在多智能体桌宠里，
//! 一个作用域通常对应一个角色（char_id），从而把"某角色专属的工具/策略/事件
//! 监听"与全局共享区分开。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::RwLock;

use super::disposer::Disposer;

static NEXT_SCOPE_ID: AtomicUsize = AtomicUsize::new(1);

/// 作用域标签。`global` 为全局；其余为按角色/插件创建的子作用域。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeId {
    id: usize,
    name: String,
}

impl ScopeId {
    fn new(name: impl Into<String>) -> Self {
        Self {
            id: NEXT_SCOPE_ID.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
        }
    }

    /// 全局作用域（不归任何插件/角色所有）。
    pub fn global() -> Self {
        Self {
            id: 0,
            name: "global".to_string(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// 作用域会话：持有根作用域与一系列已绑定的 disposer，当整个作用域被销毁时
/// 统一执行这些 disposer（等价于 Cordis 的 fiber 卸载）。
#[derive(Clone)]
pub struct Scope {
    inner: Arc<ScopeInner>,
}

struct ScopeInner {
    id: ScopeId,
    disposers: RwLock<Vec<Disposer>>,
}

impl Scope {
    /// 以指定标签创建新作用域。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                id: ScopeId::new(name),
                disposers: RwLock::new(Vec::new()),
            }),
        }
    }

    pub fn id(&self) -> &ScopeId {
        &self.inner.id
    }

    /// 挂载一个 disposer：作用域销毁或全部副本 drop 时会执行它。
    pub fn attach(&self, disposer: Disposer) {
        self.inner.disposers.write().push(disposer);
    }

    /// 立即卸载该作用域下的所有 disposer（可提前主动解绑，等价于插件卸载）。
    pub fn dispose(&self) {
        let mut guard = self.inner.disposers.write();
        for d in guard.drain(..) {
            d.dispose();
        }
    }

    /// 当前挂载的 disposer 数量。
    pub fn disposer_count(&self) -> usize {
        self.inner.disposers.read().len()
    }
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope")
            .field("id", &self.inner.id)
            .field("disposers", &self.disposer_count())
            .finish()
    }
}