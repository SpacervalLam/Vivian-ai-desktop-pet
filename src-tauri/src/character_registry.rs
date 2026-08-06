//! 角色资源注册表 — 多角色架构下，按 char_id 索引 MemoryManager / PsychologyManager / manifest
//!
//! 工具系统（memory_tools / relationship_tools）和 manifest 归一化函数
//! 原来使用全局单例，只能服务一个角色。多角色架构下改为按 char_id 索引。
//!
//! 每个角色在 state.rs::AppState::initialize 时调用 register_character 注册自己的资源。
//! 工具执行时从 ToolUseContext.char_id 读取当前角色，再通过本注册表获取对应实例。

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::engine::manifest::ResourceManifest;
use crate::memory::{MemoryManager, VerifierLlmClient};
use crate::psychology::PsychologyManager;

/// 单个角色的资源集合
pub struct CharacterResources {
    pub memory: MemoryManager,
    pub psychology: Arc<PsychologyManager>,
    pub manifest: Arc<ResourceManifest>,
    pub verifier_llm: Arc<dyn VerifierLlmClient>,
}

// 注意：CharacterResources 字段都不是 Sync（MemoryManager 内部含非 Sync 组件），
// 所以不能直接放进 static。我们用 RwLock<HashMap<String, Arc<...>>> 包一层，
// 但 Arc<CharacterResources> 要求 CharacterResources: Send + Sync。
// 实测 MemoryManager 是 Send + Sync（内部用 Mutex/RwLock 保护），PsychologyManager 同理。
// verifier_llm 是 Arc<dyn VerifierLlmClient>，trait 已要求 Send + Sync。

/// 全局角色资源注册表
static REGISTRY: Lazy<RwLock<HashMap<String, Arc<CharacterResources>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 注册一个角色的资源（在 AppState::initialize 中调用）
pub fn register_character(
    char_id: &str,
    memory: MemoryManager,
    psychology: Arc<PsychologyManager>,
    manifest: Arc<ResourceManifest>,
    verifier_llm: Arc<dyn VerifierLlmClient>,
) {
    let res = Arc::new(CharacterResources {
        memory,
        psychology,
        manifest,
        verifier_llm,
    });
    REGISTRY.write().insert(char_id.to_string(), res);
    tracing::info!("[CharacterRegistry] 已注册角色资源: {}", char_id);
}

/// 注销一个角色（角色下线时调用，目前未使用）
pub fn unregister_character(char_id: &str) {
    REGISTRY.write().remove(char_id);
}

/// 获取指定角色的资源克隆
pub fn get_resources(char_id: &str) -> Option<Arc<CharacterResources>> {
    REGISTRY.read().get(char_id).cloned()
}

/// 获取指定角色的 MemoryManager 克隆
pub fn get_memory_manager(char_id: &str) -> Option<MemoryManager> {
    REGISTRY.read().get(char_id).map(|r| r.memory.clone())
}

/// 获取指定角色的 PsychologyManager
pub fn get_psychology_manager(char_id: &str) -> Option<Arc<PsychologyManager>> {
    REGISTRY.read().get(char_id).map(|r| r.psychology.clone())
}

/// 获取指定角色的 ResourceManifest
pub fn get_manifest(char_id: &str) -> Option<Arc<ResourceManifest>> {
    REGISTRY.read().get(char_id).map(|r| r.manifest.clone())
}

/// 获取指定角色的 verifier LLM
pub fn get_verifier_llm(char_id: &str) -> Option<Arc<dyn VerifierLlmClient>> {
    REGISTRY
        .read()
        .get(char_id)
        .map(|r| r.verifier_llm.clone())
}

/// 获取已注册的所有角色 ID
pub fn list_character_ids() -> Vec<String> {
    REGISTRY.read().keys().cloned().collect()
}

/// 全局 Brain 注册表（按 char_id 索引）
///
/// Brain 为 Clone（全部字段为 Arc，clone 开销极低），
/// 供工具系统按 char_id 获取 Brain 实例（如 write_diary 工具调用 generate_intelligent_diary）。
static BRAIN_REGISTRY: Lazy<RwLock<HashMap<String, crate::brain::Brain>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 注册角色 Brain（在 AppState::initialize 中 Brain 构造完成后调用）
pub fn register_brain(char_id: &str, brain: crate::brain::Brain) {
    BRAIN_REGISTRY.write().insert(char_id.to_string(), brain);
    tracing::info!("[CharacterRegistry] 已注册角色 Brain: {}", char_id);
}

/// 获取指定角色的 Brain 克隆
pub fn get_brain(char_id: &str) -> Option<crate::brain::Brain> {
    BRAIN_REGISTRY.read().get(char_id).cloned()
}
