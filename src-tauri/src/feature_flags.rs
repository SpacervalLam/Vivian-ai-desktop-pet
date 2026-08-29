//! 功能开关系统 — 运行时可热更新的功能开关
//!
//! - 预设标志定义（按类别分组：核心 / 实验性 / 性能 / 界面 / 调试）
//! - 单例管理器，支持运行时覆盖与重置
//! - 持久化到 `%APPDATA%\Vivian\config\feature_flags.json`
//! - 线程安全（`RwLock`）
//!
//! 用法：
//! ```ignore
//! use vivian_lib::feature_flags::FeatureFlags;
//!
//! if FeatureFlags::is_enabled("voice") {
//!     init_voice_engine();
//! }
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::utils::path;

/// 功能标志类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagCategory {
    /// 核心功能
    Core,
    /// 实验性功能
    Experimental,
    /// 性能优化
    Performance,
    /// 界面功能
    Ui,
    /// 外部集成
    Integration,
    /// 调试功能
    Debug,
}

impl FlagCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            FlagCategory::Core => "core",
            FlagCategory::Experimental => "experimental",
            FlagCategory::Performance => "performance",
            FlagCategory::Ui => "ui",
            FlagCategory::Integration => "integration",
            FlagCategory::Debug => "debug",
        }
    }
}

/// 单个功能标志的定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub default: bool,
    pub category: FlagCategory,
    pub requires_restart: bool,
}

impl FlagDefinition {
    const fn new(
        name: &'static str,
        description: &'static str,
        default: bool,
        category: FlagCategory,
        requires_restart: bool,
    ) -> Self {
        Self {
            name,
            description,
            default,
            category,
            requires_restart,
        }
    }
}

/// 预设标志定义表
pub static PRESET_FLAGS: &[FlagDefinition] = &[
    // ── 核心功能 ──
    FlagDefinition::new(
        "voice",
        "语音识别与 TTS 引擎",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "proactive",
        "主动交互系统（主动搭话）",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "diary",
        "日记生成系统",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "emotion",
        "情感分析引擎",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "memory_semantic",
        "语义记忆层（长期记忆）",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "relationship",
        "关系状态管理系统",
        true,
        FlagCategory::Core,
        false,
    ),
    FlagDefinition::new(
        "desktop_control",
        "桌面控制工具集（鼠标、键盘、窗口操作）",
        false,
        FlagCategory::Core,
        true,
    ),
    FlagDefinition::new(
        "screen_perception",
        "屏幕感知工具集（截图、OCR）",
        false,
        FlagCategory::Core,
        true,
    ),
    // ── 实验性功能 ──
    FlagDefinition::new(
        "rag_knowledge_graph",
        "RAG 知识图谱增强检索",
        false,
        FlagCategory::Experimental,
        false,
    ),
    FlagDefinition::new(
        "multimodal_output",
        "多模态输出（表情+动画+语音联动）",
        false,
        FlagCategory::Experimental,
        false,
    ),
    // ── 性能优化 ──
    FlagDefinition::new(
        "deferred_tools",
        "工具延迟加载（减少启动时间）",
        true,
        FlagCategory::Performance,
        false,
    ),
    // ── 界面 ──
    FlagDefinition::new(
        "wechat_style_chat",
        "微信风格聊天窗口",
        true,
        FlagCategory::Ui,
        false,
    ),
    FlagDefinition::new(
        "advanced_config",
        "高级配置面板",
        false,
        FlagCategory::Ui,
        false,
    ),
    FlagDefinition::new(
        "memory_visualization",
        "记忆可视化面板",
        false,
        FlagCategory::Ui,
        false,
    ),
    // ── 调试 ──
    FlagDefinition::new(
        "verbose_logging",
        "详细日志输出",
        false,
        FlagCategory::Debug,
        false,
    ),
    FlagDefinition::new(
        "tool_observability",
        "工具调用可观测性统计",
        false,
        FlagCategory::Debug,
        false,
    ),
];

/// 持久化文件结构
#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    /// 标志当前值
    flags: BTreeMap<String, bool>,
    /// 运行时覆盖标记
    overrides: Vec<String>,
}

/// 全局功能标志管理器
pub struct FeatureFlags {
    flags: RwLock<HashMap<String, bool>>,
    overrides: RwLock<HashSet<String>>,
    /// 持久化文件路径（启动时初始化）
    persist_path: RwLock<PathBuf>,
}

static GLOBAL: OnceCell<Arc<FeatureFlags>> = OnceCell::new();

impl FeatureFlags {
    /// 获取全局单例
    pub fn global() -> Arc<FeatureFlags> {
        GLOBAL
            .get_or_init(|| {
                let flags = FeatureFlags::new_with_defaults();
                let path = persistence_path();
                if let Err(e) = flags.load_from_file(&path) {
                    tracing::warn!("[FeatureFlags] 加载持久化文件失败: {e}");
                }
                flags.set_persist_path(path);
                Arc::new(flags)
            })
            .clone()
    }

    /// 仅用于测试或显式构造
    pub fn new_with_defaults() -> Self {
        let mut flags = HashMap::new();
        for def in PRESET_FLAGS {
            flags.insert(def.name.to_string(), def.default);
        }
        Self {
            flags: RwLock::new(flags),
            overrides: RwLock::new(HashSet::new()),
            persist_path: RwLock::new(persistence_path()),
        }
    }

    fn set_persist_path(&self, path: PathBuf) {
        *self.persist_path.write() = path;
    }

    /// 检查功能标志是否启用（未注册的标志默认放行 = true）
    pub fn is_enabled(&self, name: &str) -> bool {
        let flags = self.flags.read();
        *flags.get(name).unwrap_or(&true)
    }

    /// 设置功能标志的值
    pub fn set(&self, name: &str, value: bool, override_flag: bool) {
        {
            let mut flags = self.flags.write();
            flags.insert(name.to_string(), value);
        }
        if override_flag {
            let mut overrides = self.overrides.write();
            overrides.insert(name.to_string());
        }
        if let Err(e) = self.persist() {
            tracing::warn!(error = %e, "[feature_flags] 持久化失败，重启后 flag 可能回退");
        }
    }

    /// 重置标志为预设默认值
    pub fn reset(&self, name: &str) {
        if let Some(def) = Self::find_definition(name) {
            let mut flags = self.flags.write();
            flags.insert(name.to_string(), def.default);
        }
        let mut overrides = self.overrides.write();
        overrides.remove(name);
        if let Err(e) = self.persist() {
            tracing::warn!(error = %e, "[feature_flags] 持久化失败，重启后 flag 可能回退");
        }
    }

    /// 重置所有标志为预设默认值
    pub fn reset_all(&self) {
        {
            let mut flags = self.flags.write();
            flags.clear();
            for def in PRESET_FLAGS {
                flags.insert(def.name.to_string(), def.default);
            }
        }
        self.overrides.write().clear();
        if let Err(e) = self.persist() {
            tracing::warn!(error = %e, "[feature_flags] 持久化失败，重启后 flag 可能回退");
        }
    }

    /// 获取所有标志的当前状态。
    ///
    /// 返回 HashMap 的深拷贝。功能标志数量有限（预定义集合），且该方法调用频率低
    /// （仅前端面板或调试查询），clone 开销可忽略。保留 clone 以维持稳定 API；
    /// 若未来出现高频调用，可改为 `Vec<(String, bool)>` 或回调式 API。
    pub fn get_all(&self) -> HashMap<String, bool> {
        self.flags.read().clone()
    }

    /// 获取当前启用的标志名称列表
    pub fn get_enabled(&self) -> Vec<String> {
        self.flags
            .read()
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// 获取当前禁用的标志名称列表
    pub fn get_disabled(&self) -> Vec<String> {
        self.flags
            .read()
            .iter()
            .filter(|(_, v)| !**v)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// 获取标志定义
    pub fn find_definition(name: &str) -> Option<&'static FlagDefinition> {
        PRESET_FLAGS.iter().find(|d| d.name == name)
    }

    /// 按类别获取标志
    pub fn get_by_category(&self, category: FlagCategory) -> HashMap<String, bool> {
        let flags = self.flags.read();
        PRESET_FLAGS
            .iter()
            .filter(|d| d.category == category)
            .map(|d| (d.name.to_string(), *flags.get(d.name).unwrap_or(&true)))
            .collect()
    }

    /// 检查修改该标志是否需要重启
    pub fn requires_restart(name: &str) -> bool {
        Self::find_definition(name)
            .map(|d| d.requires_restart)
            .unwrap_or(false)
    }

    /// 从配置字典加载功能标志
    pub fn load_from_config(&self, config: &HashMap<String, bool>) {
        let mut flags = self.flags.write();
        let mut loaded = 0usize;
        for (name, value) in config {
            if PRESET_FLAGS.iter().any(|d| d.name == name) {
                flags.insert(name.clone(), *value);
                loaded += 1;
            }
        }
        tracing::info!(
            "[FeatureFlags] 从配置加载了 {} 个功能标志 (共 {} 个预定义)",
            loaded,
            PRESET_FLAGS.len()
        );
    }

    /// 导出当前状态为可序列化字典
    pub fn dump_state(&self) -> serde_json::Value {
        let flags = self.flags.read();
        let mut overrides: Vec<String> = self.overrides.read().iter().cloned().collect();
        overrides.sort();
        serde_json::json!({
            "flags": flags.iter().map(|(k, v)| (k.clone(), *v)).collect::<BTreeMap<_, _>>(),
            "overrides": overrides,
        })
    }

    fn load_from_file(&self, path: &PathBuf) -> Result<(), std::io::Error> {
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        match serde_json::from_str::<PersistedState>(&content) {
            Ok(state) => {
                let mut flags = self.flags.write();
                for (name, value) in state.flags {
                    if PRESET_FLAGS.iter().any(|d| d.name == name) {
                        flags.insert(name, value);
                    }
                }
                let mut overrides = self.overrides.write();
                overrides.clear();
                for name in state.overrides {
                    overrides.insert(name);
                }
                tracing::info!("[FeatureFlags] 从 {} 加载了持久化状态", path.display());
                Ok(())
            }
            Err(e) => {
                tracing::warn!("[FeatureFlags] 解析持久化文件失败: {e}");
                Ok(())
            }
        }
    }

    fn persist(&self) -> Result<(), std::io::Error> {
        let path = self.persist_path.read().clone();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let flags = self.flags.read();
        let sorted_flags: BTreeMap<String, bool> = flags.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let mut overrides: Vec<String> = self.overrides.read().iter().cloned().collect();
        overrides.sort();
        let state = PersistedState {
            flags: sorted_flags,
            overrides,
        };
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// 持久化文件路径：`%APPDATA%\Vivian\config\feature_flags.json`
fn persistence_path() -> PathBuf {
    let dir = path::get_user_data_dir().join("config");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("feature_flags.json")
}

// ── 静态方法 ──

impl FeatureFlags {
    pub fn is_enabled_global(name: &str) -> bool {
        Self::global().is_enabled(name)
    }

    pub fn set_global(name: &str, value: bool, override_flag: bool) {
        Self::global().set(name, value, override_flag);
    }

    pub fn reset_global(name: &str) {
        Self::global().reset(name);
    }

    pub fn reset_all_global() {
        Self::global().reset_all();
    }
}

/// 模块级简写函数
pub fn feature(name: &str) -> bool {
    FeatureFlags::is_enabled_global(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_flags_nonempty() {
        assert!(PRESET_FLAGS.len() >= 18);
    }

    #[test]
    fn test_defaults_loaded() {
        let flags = FeatureFlags::new_with_defaults();
        assert!(flags.is_enabled("voice"));
        assert!(flags.is_enabled("diary"));
        assert!(!flags.is_enabled("desktop_control"));
        assert!(!flags.is_enabled("verbose_logging"));
    }

    #[test]
    fn test_unknown_flag_defaults_true() {
        let flags = FeatureFlags::new_with_defaults();
        assert!(flags.is_enabled("this_flag_does_not_exist"));
    }

    #[test]
    fn test_set_and_reset() {
        let flags = FeatureFlags::new_with_defaults();
        flags.set("voice", false, true);
        assert!(!flags.is_enabled("voice"));
        flags.reset("voice");
        assert!(flags.is_enabled("voice"));
    }

    #[test]
    fn test_requires_restart() {
        assert!(FeatureFlags::requires_restart("desktop_control"));
        assert!(!FeatureFlags::requires_restart("voice"));
    }

    #[test]
    fn test_get_by_category() {
        let flags = FeatureFlags::new_with_defaults();
        let ui_flags = flags.get_by_category(FlagCategory::Ui);
        assert!(ui_flags.contains_key("wechat_style_chat"));
        assert!(ui_flags["wechat_style_chat"]);
    }

    #[test]
    fn test_dump_state_serializable() {
        let flags = FeatureFlags::new_with_defaults();
        let state = flags.dump_state();
        assert!(state.is_object());
        assert!(state.get("flags").is_some());
        assert!(state.get("overrides").is_some());
    }
}
