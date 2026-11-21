//! 主动对话服务 - 桥接 ProactiveOrchestrator 与工具/前端
//!
//! 提供 ProactiveOrchestrator 的共享访问器。

use std::sync::Arc;

use parking_lot::RwLock;

use crate::proactive::ProactiveOrchestrator;

/// 主动对话服务：持有 ProactiveOrchestrator 引用并提供注入/获取接口
pub struct ProactiveService;

static INSTANCE: once_cell::sync::Lazy<RwLock<Option<Arc<ProactiveOrchestrator>>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

impl ProactiveService {
    /// 注入 ProactiveOrchestrator
    pub fn install(orch: Arc<ProactiveOrchestrator>) {
        *INSTANCE.write() = Some(orch);
    }

    /// 获取 ProactiveOrchestrator 的 Arc 引用
    pub fn get() -> Option<Arc<ProactiveOrchestrator>> {
        INSTANCE.read().clone()
    }
}
