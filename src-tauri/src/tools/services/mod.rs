//! 服务层 - 工具子系统的统一服务协调层
//!
//! 为工具提供共享的后端服务访问器，避免每个工具各自维护全局状态。
//! 服务层只做"持有 + 暴露"，不重复实现工具逻辑（单一职责）。

pub mod memory_service;
pub mod pet_service;
pub mod proactive_service;
pub mod todo_service;

pub use memory_service::MemoryService;
pub use pet_service::PetService;
pub use proactive_service::ProactiveService;
pub use todo_service::TodoService;

use std::sync::Arc;

use parking_lot::RwLock;

use crate::memory::MemoryManager;
use crate::proactive::ProactiveOrchestrator;
use crate::psychology::PsychologyManager;

/// 服务上下文 - 持有所有后端服务的 Arc 引用
///
/// 由 `state.rs` 在应用启动时创建并注入到各工具模块。
/// 关系系统已整合到 PsychologyManager，不再需要独立的 RelationshipManager。
pub struct ServiceContext {
    pub memory: RwLock<Option<Arc<MemoryManager>>>,
    pub psychology: RwLock<Option<Arc<PsychologyManager>>>,
    pub proactive: RwLock<Option<Arc<ProactiveOrchestrator>>>,
}

impl ServiceContext {
    pub fn new() -> Self {
        Self {
            memory: RwLock::new(None),
            psychology: RwLock::new(None),
            proactive: RwLock::new(None),
        }
    }

    /// 注入记忆管理器
    pub fn set_memory(&self, mgr: Arc<MemoryManager>) {
        *self.memory.write() = Some(mgr);
    }

    /// 注入心理系统管理器（含关系系统）
    pub fn set_psychology(&self, mgr: Arc<PsychologyManager>) {
        *self.psychology.write() = Some(mgr);
    }

    /// 注入主动对话编排器
    pub fn set_proactive(&self, orch: Arc<ProactiveOrchestrator>) {
        *self.proactive.write() = Some(orch);
    }

    /// 获取记忆管理器
    pub fn get_memory(&self) -> Option<Arc<MemoryManager>> {
        self.memory.read().clone()
    }

    /// 获取心理系统管理器（含关系系统）
    pub fn get_psychology(&self) -> Option<Arc<PsychologyManager>> {
        self.psychology.read().clone()
    }

    /// 获取主动对话编排器
    pub fn get_proactive(&self) -> Option<Arc<ProactiveOrchestrator>> {
        self.proactive.read().clone()
    }
}

impl Default for ServiceContext {
    fn default() -> Self {
        Self::new()
    }
}
