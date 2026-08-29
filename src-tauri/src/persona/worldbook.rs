//! Worldbook 背景知识 - 动态激活状态机
//!
//! 动态激活与衰减状态机：
//! - 激活奖励：用户命中关键词时累加（久别重逢有增益）
//! - 维护奖励：模型主动维护的条目在用户沉默时按指数衰减
//! - 衰减公式：价值越高忘得越慢（平方加速）
//! - 三态状态机：Archived → Dormant → Active
//!
//! 集成点：`pipeline::steps::prompt::PromptBuildingStep` 每轮先调用
//! `update_activation` 更新状态，再调用 `render_worldbook_block` 渲染可注入条目。
//!
//! 持久化：`%APPDATA%\Vivian\persona\worldbook.json`

use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

// ===== 动态激活参数 =====

/// Worldbook 动态激活参数集
///
/// 所有参数都只是默认值，不是结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldbookParams {
    /// 用户命中奖励基础值（每次命中累加）
    pub bu: f64,
    /// 维护奖励基础值（每 tick 累加）
    pub bm: f64,
    /// 久别重逢增益系数（U_old 越大奖励越大）
    pub gamma: f64,
    /// 维护衰减系数（U_old 越大模型话语权越小）
    pub lambda: f64,
    /// 衰减公式中 usage 的平方系数
    pub alpha: f64,
    /// 衰减公式中 maintenance 的平方系数
    pub beta: f64,
    /// 激活阈值：A >= 此值进入 Active 状态
    pub active_threshold: f64,
    /// 最大同时激活条目数（超过则按激活度降序截断）
    pub max_active: usize,
}

impl Default for WorldbookParams {
    fn default() -> Self {
        Self {
            bu: 20.0,
            bm: 8.0,
            gamma: 0.5,
            lambda: 0.3,
            alpha: 1.5,
            beta: 0.3,
            active_threshold: 30.0,
            max_active: 8,
        }
    }
}

/// 默认激活阈值（供 WorldbookEntry::state() 编译期判断使用）
/// 运行时实际阈值由 WorldbookParams.active_threshold 提供（get_injectable_entries）
const DEFAULT_ACTIVE_THRESHOLD: f64 = 30.0;

// ===== 数据结构 =====

/// Worldbook 条目（静态定义 + 动态激活状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldbookEntry {
    /// 条目 ID（唯一标识）
    pub id: String,
    /// 触发关键词（命中任一即累加用户命中奖励）
    pub keywords: Vec<String>,
    /// 背景知识正文（注入 prompt 的内容）
    pub content: String,
    /// 基础价值（0.0-1.0，价值越高衰减越慢）
    pub base_value: f64,
    /// 常驻标记：true 时每轮无条件注入，不参与激活度计算和 max_active 截断
    #[serde(default)]
    pub constant: bool,

    // ===== 动态激活状态 =====
    /// 激活度（核心状态变量）
    #[serde(default)]
    pub activation: f64,
    /// 用户命中次数（累积）
    #[serde(default)]
    pub user_hits: f64,
    /// 维护次数（模型主动维护累积）
    #[serde(default)]
    pub maintenance: f64,
    /// 上次用户命中的时间戳（秒）
    #[serde(default)]
    pub last_user_hit_at: f64,
    /// 上次 tick 的时间戳（秒）
    #[serde(default)]
    pub last_tick_at: f64,
}

impl WorldbookEntry {
    fn new(id: &str, keywords: Vec<String>, content: String, base_value: f64) -> Self {
        Self {
            id: id.to_string(),
            keywords,
            content,
            base_value,
            constant: false,
            activation: 0.0,
            user_hits: 0.0,
            maintenance: 0.0,
            last_user_hit_at: 0.0,
            last_tick_at: 0.0,
        }
    }

    /// 创建常驻条目（每轮无条件注入，不参与激活度计算）
    fn new_constant(id: &str, content: String) -> Self {
        Self {
            id: id.to_string(),
            keywords: Vec::new(),
            content,
            base_value: 1.0,
            constant: true,
            activation: 0.0,
            user_hits: 0.0,
            maintenance: 0.0,
            last_user_hit_at: 0.0,
            last_tick_at: 0.0,
        }
    }

    /// 当前状态：Archived / Dormant / Active
    pub fn state(&self) -> WorldbookState {
        if self.activation <= 0.0 {
            WorldbookState::Archived
        } else if self.activation < DEFAULT_ACTIVE_THRESHOLD {
            WorldbookState::Dormant
        } else {
            WorldbookState::Active
        }
    }

    /// 是否命中任一关键词
    fn matches(&self, text: &str) -> bool {
        self.keywords.iter().any(|k| text.contains(k.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldbookState {
    /// 归档：激活度 <= 0
    Archived,
    /// 休眠：0 < 激活度 < 30
    Dormant,
    /// 激活：激活度 >= 30
    Active,
}

// ===== 动态激活引擎 =====

pub struct WorldbookEngine {
    entries: RwLock<Vec<WorldbookEntry>>,
    persistence_path: std::path::PathBuf,
    params: RwLock<WorldbookParams>,
}

impl WorldbookEngine {
    fn new() -> VivianResult<Self> {
        let persona_dir = get_user_data_dir().join("persona");
        std::fs::create_dir_all(&persona_dir)
            .map_err(|e| VivianError::Memory(format!("创建 worldbook 目录失败: {e}")))?;
        let persistence_path = persona_dir.join("worldbook.json");

        let mut engine = Self {
            entries: RwLock::new(default_entries()),
            persistence_path,
            params: RwLock::new(WorldbookParams::default()),
        };
        engine.load_from_disk()?;
        Ok(engine)
    }

    /// 更新激活状态：
    /// - 对命中用户输入的条目累加用户命中奖励
    /// - 对未命中的激活/休眠条目累加维护奖励
    /// - 全部条目应用衰减公式
    pub fn update_activation(&self, user_input: &str, last_assistant: Option<&str>) {
        let now = current_timestamp();
        let params = self.params.read().clone();
        let mut entries = self.entries.write();

        for entry in entries.iter_mut() {
            // 常驻条目不参与激活度计算
            if entry.constant {
                continue;
            }

            // 计算自上次 tick 以来的时间间隔（小时），用于衰减
            let dt_hours = if entry.last_tick_at > 0.0 {
                ((now - entry.last_tick_at) / 3600.0).max(0.0)
            } else {
                0.0
            };
            entry.last_tick_at = now;

            let hit_user = entry.matches(user_input);
            let hit_assistant = last_assistant.map(|t| entry.matches(t)).unwrap_or(false);

            if hit_user {
                // 用户命中奖励：Ru = Bu * (1 + γ * ln(1 + U_old))
                let gain = params.bu * (1.0 + params.gamma * (1.0 + entry.user_hits).ln());
                entry.activation += gain;
                entry.user_hits += 1.0;
                entry.last_user_hit_at = now;
            } else if hit_assistant {
                // 模型维护奖励：Rm = Bm * e^(-λ * U_old)
                let gain = params.bm * (-params.lambda * entry.user_hits).exp();
                entry.activation += gain;
                entry.maintenance += 1.0;
            }

            // 衰减：D = (α * U² + β * MS²) / √I
            // 价值越高（base_value 越大），衰减越慢
            let decay = if dt_hours > 0.0 {
                let usage_sq = entry.user_hits * entry.user_hits;
                let maint_sq = entry.maintenance * entry.maintenance;
                let raw = (params.alpha * usage_sq + params.beta * maint_sq)
                    / entry.base_value.sqrt().max(0.1);
                raw * dt_hours
            } else {
                0.0
            };
            entry.activation = (entry.activation - decay).max(0.0);
        }

        // 标记脏数据，异步落盘（这里同步落盘以保证简单性）
        if let Err(e) = self.persist_internal(&entries) {
            tracing::warn!("[Worldbook] 持久化失败: {e}");
        }
    }

    /// 渲染可注入 prompt 的背景知识块
    ///
    /// 规则：
    /// - constant 条目全量返回，不参与 threshold/max_active 截断
    /// - 非 constant 条目仅渲染 Active 状态（activation >= params.active_threshold）
    /// - 非 constant 条目按 activation 降序排列，最多 params.max_active 条
    pub fn get_injectable_entries(&self) -> Vec<WorldbookEntry> {
        let entries = self.entries.read();
        let params = self.params.read();
        let threshold = params.active_threshold;
        let max_active = params.max_active;

        // 常驻条目：全量返回，排在最前
        let mut result: Vec<WorldbookEntry> = entries
            .iter()
            .filter(|e| e.constant)
            .cloned()
            .collect();

        // 动态条目：按激活度过滤、排序、截断
        let mut active: Vec<WorldbookEntry> = entries
            .iter()
            .filter(|e| !e.constant && e.activation >= threshold)
            .cloned()
            .collect();
        active.sort_by(|a, b| {
            b.activation
                .partial_cmp(&a.activation)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        active.truncate(max_active);
        result.extend(active);
        result
    }

    /// 渲染为 prompt 文本块
    ///
    /// `lang` 为 None 时使用条目持久化的 content；为 Some 时对默认条目做运行时多语言替换。
    pub fn render_block(&self, lang: Option<&str>) -> String {
        let active = self.get_injectable_entries();
        if active.is_empty() {
            return String::new();
        }
        let mut blocks: Vec<String> = Vec::with_capacity(active.len());
        for entry in &active {
            let is_default = matches!(
                entry.id.as_str(),
                "anime_culture" | "internet_culture" | "game_culture"
            );
            let content = if is_default {
                if let Some(l) = lang {
                    worldbook_content(&entry.id, l)
                } else {
                    &entry.content
                }
            } else {
                &entry.content
            };
            blocks.push(format!("[{}] {}", entry.id, content));
        }
        let header = crate::pipeline::prompt_modules::section_heading(
            "background_knowledge",
            lang.unwrap_or("zh"),
        );
        format!("{}\n{}", header, blocks.join("\n"))
    }

    /// 调试快照（供状态面板或日志使用）
    pub fn debug_snapshot(&self) -> Vec<(String, WorldbookState, f64)> {
        self.entries
            .read()
            .iter()
            .map(|e| (e.id.clone(), e.state(), e.activation))
            .collect()
    }

    /// 添加常驻条目（运行时调用，如用户确认硬约束后提升为常驻事实）
    pub fn add_constant_entry(&self, id: &str, content: String) -> VivianResult<()> {
        {
            let mut entries = self.entries.write();
            if entries.iter().any(|e| e.id == id) {
                return Err(VivianError::Memory(format!(
                    "worldbook 条目已存在: {id}"
                )));
            }
            entries.push(WorldbookEntry::new_constant(id, content));
        }
        // 持读锁直接序列化与落盘，避免克隆整个 Vec
        let result = {
            let entries = self.entries.read();
            self.persist_internal(&entries)
        };
        if let Err(e) = result {
            tracing::warn!("[Worldbook] 持久化失败: {e}");
        }
        Ok(())
    }

    /// 移除条目（按 ID）
    pub fn remove_entry(&self, id: &str) -> VivianResult<()> {
        {
            let mut entries = self.entries.write();
            let before = entries.len();
            entries.retain(|e| e.id != id);
            if entries.len() == before {
                return Err(VivianError::Memory(format!(
                    "worldbook 条目不存在: {id}"
                )));
            }
        }
        // 持读锁直接序列化与落盘，避免克隆整个 Vec
        let result = {
            let entries = self.entries.read();
            self.persist_internal(&entries)
        };
        if let Err(e) = result {
            tracing::warn!("[Worldbook] 持久化失败: {e}");
        }
        Ok(())
    }

    fn load_from_disk(&mut self) -> VivianResult<()> {
        if !self.persistence_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("读取 worldbook 失败: {e}")))?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let stored: Vec<WorldbookEntry> = serde_json::from_str(&content)
            .map_err(|e| VivianError::Memory(format!("解析 worldbook 失败: {e}")))?;

        let mut entries = self.entries.write();
        // 合并：以存储的状态覆盖默认条目，新增默认条目保留
        for default_entry in entries.iter_mut() {
            if let Some(stored_entry) = stored.iter().find(|s| s.id == default_entry.id) {
                default_entry.activation = stored_entry.activation;
                default_entry.user_hits = stored_entry.user_hits;
                default_entry.maintenance = stored_entry.maintenance;
                default_entry.last_user_hit_at = stored_entry.last_user_hit_at;
                default_entry.last_tick_at = stored_entry.last_tick_at;
                default_entry.constant = stored_entry.constant;
            }
        }
        // 追加存储中存在但默认条目中没有的（如运行时添加的 constant 条目）
        for stored_entry in &stored {
            if !entries.iter().any(|e| e.id == stored_entry.id) {
                entries.push(stored_entry.clone());
            }
        }
        Ok(())
    }

    fn persist_internal(&self, entries: &[WorldbookEntry]) -> VivianResult<()> {
        let json = serde_json::to_string_pretty(entries)
            .map_err(|e| VivianError::Memory(format!("序列化 worldbook 失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入 worldbook 临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换 worldbook 文件失败: {e}")))?;
        Ok(())
    }
}

fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ===== 多语言 worldbook 内容（编译期嵌入） =====

const WB_ANIME_ZH: &str = include_str!("../../prompts/worldbook/anime_culture.zh.md");
const WB_ANIME_EN: &str = include_str!("../../prompts/worldbook/anime_culture.en.md");
const WB_ANIME_JA: &str = include_str!("../../prompts/worldbook/anime_culture.ja.md");
const WB_INTERNET_ZH: &str = include_str!("../../prompts/worldbook/internet_culture.zh.md");
const WB_INTERNET_EN: &str = include_str!("../../prompts/worldbook/internet_culture.en.md");
const WB_INTERNET_JA: &str = include_str!("../../prompts/worldbook/internet_culture.ja.md");
const WB_GAME_ZH: &str = include_str!("../../prompts/worldbook/game_culture.zh.md");
const WB_GAME_EN: &str = include_str!("../../prompts/worldbook/game_culture.en.md");
const WB_GAME_JA: &str = include_str!("../../prompts/worldbook/game_culture.ja.md");

/// 根据语言和条目 ID 获取对应的世界书内容
fn worldbook_content(id: &str, lang: &str) -> &'static str {
    match crate::pipeline::prompt_modules::normalize_lang(lang) {
        "zh" => match id {
            "anime_culture" => WB_ANIME_ZH,
            "internet_culture" => WB_INTERNET_ZH,
            "game_culture" => WB_GAME_ZH,
            _ => "",
        },
        "ja" => match id {
            "anime_culture" => WB_ANIME_JA,
            "internet_culture" => WB_INTERNET_JA,
            "game_culture" => WB_GAME_JA,
            _ => "",
        },
        _ => match id {
            "anime_culture" => WB_ANIME_EN,
            "internet_culture" => WB_INTERNET_EN,
            "game_culture" => WB_GAME_EN,
            _ => "",
        },
    }
}

/// 默认条目（动漫文化 / 互联网文化 / 游戏文化）
fn default_entries() -> Vec<WorldbookEntry> {
    vec![
        WorldbookEntry::new(
            "anime_culture",
            vec![
                "番剧".to_string(),
                "动漫".to_string(),
                "动画".to_string(),
                "二次元".to_string(),
                "番".to_string(),
                "追番".to_string(),
                "声优".to_string(),
                "萌".to_string(),
                "傲娇".to_string(),
                "中二".to_string(),
            ],
            WB_ANIME_EN.to_string(),
            0.7,
        ),
        WorldbookEntry::new(
            "internet_culture",
            vec![
                "梗".to_string(),
                "整活".to_string(),
                "玩梗".to_string(),
                "抽象".to_string(),
                "乐子".to_string(),
                "无语".to_string(),
                "笑死".to_string(),
                "破防".to_string(),
                "种草".to_string(),
                "社死".to_string(),
            ],
            WB_INTERNET_EN.to_string(),
            0.6,
        ),
        WorldbookEntry::new(
            "game_culture",
            vec![
                "游戏".to_string(),
                "手游".to_string(),
                "端游".to_string(),
                "主机".to_string(),
                "打怪".to_string(),
                "升级".to_string(),
                "boss".to_string(),
                "副本".to_string(),
                "肝".to_string(),
                "氪金".to_string(),
            ],
            WB_GAME_EN.to_string(),
            0.6,
        ),
    ]
}

// ===== 全局单例 =====

static WORLDBOOK_ENGINE: Lazy<Arc<WorldbookEngine>> = Lazy::new(|| {
    Arc::new(WorldbookEngine::new().unwrap_or_else(|e| {
        tracing::error!("[Worldbook] 引擎初始化失败，使用空状态: {e}");
        WorldbookEngine {
            entries: RwLock::new(default_entries()),
            persistence_path: std::path::PathBuf::from("worldbook.json"),
            params: RwLock::new(WorldbookParams::default()),
        }
    }))
});

/// 获取全局引擎实例
pub fn engine() -> Arc<WorldbookEngine> {
    WORLDBOOK_ENGINE.clone()
}

/// 更新激活状态（便捷接口，供 PromptBuildingStep 调用）
pub fn update_activation(user_input: &str, last_assistant: Option<&str>) {
    engine().update_activation(user_input, last_assistant);
}

/// 渲染背景知识块（便捷接口，供 PromptBuildingStep 调用）
pub fn render_worldbook_block(_user_input: &str, lang: &str) -> String {
    engine().render_block(Some(lang))
}

/// 添加常驻条目（便捷接口）
pub fn add_constant_entry(id: &str, content: String) -> VivianResult<()> {
    engine().add_constant_entry(id, content)
}

/// 移除条目（便捷接口）
pub fn remove_entry(id: &str) -> VivianResult<()> {
    engine().remove_entry(id)
}

// ===== 单元测试 =====

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_entries() {
        let entries = default_entries();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.id == "anime_culture"));
        assert!(entries.iter().any(|e| e.id == "internet_culture"));
        assert!(entries.iter().any(|e| e.id == "game_culture"));
    }

    #[test]
    fn test_keyword_match() {
        let entry = WorldbookEntry::new(
            "test",
            vec!["游戏".to_string()],
            "test".to_string(),
            0.5,
        );
        assert!(entry.matches("最近在玩什么游戏"));
        assert!(!entry.matches("今天天气不错"));
    }

    #[test]
    fn test_state_machine() {
        let mut entry = WorldbookEntry::new(
            "test",
            vec!["游戏".to_string()],
            "test".to_string(),
            0.5,
        );
        assert_eq!(entry.state(), WorldbookState::Archived);

        entry.activation = 15.0;
        assert_eq!(entry.state(), WorldbookState::Dormant);

        entry.activation = 35.0;
        assert_eq!(entry.state(), WorldbookState::Active);
    }

    #[test]
    fn test_activation_reward() {
        let engine = WorldbookEngine {
            entries: RwLock::new(default_entries()),
            persistence_path: std::path::PathBuf::from("test_worldbook_tmp.json"),
            params: RwLock::new(WorldbookParams::default()),
        };
        // 命中游戏关键词（首次命中 activation=20，低于阈值 30，应为 Dormant）
        engine.update_activation("最近在玩什么游戏", None);
        let snap = engine.debug_snapshot();
        let game = snap.iter().find(|(id, _, _)| id == "game_culture").unwrap();
        assert!(game.2 > 0.0, "命中后激活度应 > 0");
        assert_eq!(game.1, WorldbookState::Dormant, "首次命中未达 Active 阈值");
        // 清理测试文件
        let _ = std::fs::remove_file("test_worldbook_tmp.json");
    }

    #[test]
    fn test_constant_entry_always_injected() {
        let mut entries = default_entries();
        entries.push(WorldbookEntry::new_constant(
            "user_allergy",
            "用户对花生过敏，绝对不能推荐含花生的食物".to_string(),
        ));
        let engine = WorldbookEngine {
            entries: RwLock::new(entries),
            persistence_path: std::path::PathBuf::from("/dev/null"),
            params: RwLock::new(WorldbookParams::default()),
        };
        // 即使无关键词命中，constant 条目也应出现在 injectable 中
        let injectable = engine.get_injectable_entries();
        assert!(
            injectable.iter().any(|e| e.id == "user_allergy"),
            "constant 条目应无条件注入"
        );
        // constant 条目应排在最前
        assert_eq!(injectable[0].id, "user_allergy");
    }

    #[test]
    fn test_constant_entry_skips_activation() {
        let entries = vec![WorldbookEntry::new_constant(
            "core_rule",
            "核心规则内容".to_string(),
        )];
        let engine = WorldbookEngine {
            entries: RwLock::new(entries),
            persistence_path: std::path::PathBuf::from("/dev/null"),
            params: RwLock::new(WorldbookParams::default()),
        };
        // 多次 update_activation 后，constant 条目的激活度应保持 0
        engine.update_activation("任何内容", None);
        engine.update_activation("任何内容", None);
        let snap = engine.debug_snapshot();
        assert_eq!(snap[0].2, 0.0, "constant 条目不应被激活度计算影响");
    }

    #[test]
    fn test_constant_not_affected_by_max_active() {
        let params = WorldbookParams {
            max_active: 1,
            active_threshold: 10.0,
            ..WorldbookParams::default()
        };
        let entries = vec![
            WorldbookEntry::new_constant("c1", "常驻1".into()),
            WorldbookEntry::new_constant("c2", "常驻2".into()),
            WorldbookEntry::new_constant("c3", "常驻3".into()),
            WorldbookEntry::new("d1", vec!["a".into()], "动态1".into(), 0.9),
            WorldbookEntry::new("d2", vec!["b".into()], "动态2".into(), 0.8),
        ];
        let engine = WorldbookEngine {
            entries: RwLock::new(entries),
            persistence_path: std::path::PathBuf::from("/dev/null"),
            params: RwLock::new(params),
        };
        engine.update_activation("a b", None);
        let injectable = engine.get_injectable_entries();
        // 3 个 constant 全部返回 + max_active=1 个动态
        let constant_count = injectable.iter().filter(|e| e.constant).count();
        let dynamic_count = injectable.iter().filter(|e| !e.constant).count();
        assert_eq!(constant_count, 3, "constant 条目不受 max_active 限制");
        assert_eq!(dynamic_count, 1, "动态条目受 max_active=1 限制");
    }
}

// ===== 仿真调参测试 =====
//
// 这些测试验证算法在不同参数下的行为，支持调参实验。

#[cfg(test)]
mod sim_tests {
    use super::*;

    /// 创建测试用引擎（不落盘，params 可配置）
    fn make_engine(entries: Vec<WorldbookEntry>, params: WorldbookParams) -> WorldbookEngine {
        WorldbookEngine {
            entries: RwLock::new(entries),
            persistence_path: std::path::PathBuf::from("/dev/null"),
            params: RwLock::new(params),
        }
    }

    /// 单条目（coffee-lifecycle 场景）
    fn single_entry(id: &str, keyword: &str, base_value: f64) -> Vec<WorldbookEntry> {
        vec![WorldbookEntry::new(
            id,
            vec![keyword.to_string()],
            format!("{} content", id),
            base_value,
        )]
    }

    /// 场景 1：coffee-lifecycle
    /// 验证：命中累积 → Active → 沉默衰减 → 激活度下降
    #[test]
    fn sim_coffee_lifecycle() {
        let params = WorldbookParams::default();
        let engine = make_engine(single_entry("coffee", "咖啡", 0.8), params.clone());

        // R1: 首次命中 → Dormant（activation≈20，低于阈值 30）
        engine.update_activation("今天喝杯咖啡", None);
        let snap = engine.debug_snapshot();
        let (_, state, act) = snap[0];
        assert!(act > 0.0, "R1: 命中后激活度应 > 0，实际: {act}");
        assert_eq!(state, WorldbookState::Dormant, "R1: 首次命中应为 Dormant");

        // R2: 再次命中 → Active（activation ≈ 20 + 26.9 = 46.9 ≥ 30）
        engine.update_activation("再来一杯咖啡", None);
        let snap = engine.debug_snapshot();
        let (_, state, act_after_2hits) = snap[0];
        assert_eq!(state, WorldbookState::Active, "R2: 二次命中应达 Active");
        let peak = act_after_2hits;

        // R3-R5: 用户沉默，模拟时间流逝（每次 tick 前回拨 last_tick_at 10 小时）
        for i in 0..3 {
            {
                let mut entries = engine.entries.write();
                for e in entries.iter_mut() {
                    e.last_tick_at = current_timestamp() - 10.0 * 3600.0;
                }
            }
            engine.update_activation("今天天气不错", None); // 不命中
            let snap = engine.debug_snapshot();
            let (_, _, act) = snap[0];
            println!("R{}: activation = {}", i + 3, act);
        }

        // 多轮沉默后，激活度应低于峰值
        let snap = engine.debug_snapshot();
        let (_, _, final_act) = snap[0];
        assert!(
            final_act < peak,
            "沉默后激活度 {final_act} 应低于峰值 {peak}"
        );
    }

    /// 场景 2：dormant-rescue
    /// 验证：Dormant 状态下用户再次命中，激活度应立即回升
    #[test]
    fn sim_dormant_rescue() {
        let params = WorldbookParams::default();
        let engine = make_engine(single_entry("rescue", "音乐", 0.7), params.clone());

        // 第一次命中 → Dormant（首次命中 activation≈20，低于阈值 30）
        engine.update_activation("听听音乐", None);
        let snap1 = engine.debug_snapshot();
        let act1 = snap1[0].2;
        assert!(act1 > 0.0, "首次命中应有激活度，实际: {act1}");
        assert_eq!(snap1[0].1, WorldbookState::Dormant, "首次命中应为 Dormant");

        // 模拟沉默导致降级到 Dormant
        {
            let mut entries = engine.entries.write();
            for e in entries.iter_mut() {
                // last_tick_at 设为当前时间，避免 dt 过大产生衰减抵消命中奖励
                e.last_tick_at = current_timestamp();
                e.activation = 15.0; // Dormant 区间
            }
        }
        let snap2 = engine.debug_snapshot();
        assert_eq!(snap2[0].1, WorldbookState::Dormant, "应为 Dormant");

        // 再次命中 → 激活度应回升（dt≈0 无衰减，纯加奖励）
        engine.update_activation("放点音乐", None);
        let snap3 = engine.debug_snapshot();
        let act3 = snap3[0].2;
        assert!(
            act3 > 15.0,
            "Dormant 状态下再次命中，激活度应回升，实际: {act3}"
        );
        assert_eq!(
            snap3[0].1,
            WorldbookState::Active,
            "再次命中应回到 Active"
        );
    }

    /// 场景 3：参数对比 — 不同 gamma 下久别重逢增益差异
    #[test]
    fn sim_param_sweep_gamma() {
        let base_value = 0.8;

        // gamma = 0.1（低增益）
        let params_low = WorldbookParams {
            gamma: 0.1,
            ..WorldbookParams::default()
        };
        let engine_low = make_engine(single_entry("g1", "测试", base_value), params_low);
        engine_low.update_activation("测试", None);
        let act_low = engine_low.debug_snapshot()[0].2;

        // gamma = 2.0（高增益）
        let params_high = WorldbookParams {
            gamma: 2.0,
            ..WorldbookParams::default()
        };
        let engine_high = make_engine(single_entry("g2", "测试", base_value), params_high);
        engine_high.update_activation("测试", None);
        let act_high = engine_high.debug_snapshot()[0].2;

        // 首次命中时 U_old=0，ln(1+0)=0，所以 gamma 对首次命中无影响
        // 两次命中应相等（gamma 只在重复命中时起作用）
        assert!(
            (act_low - act_high).abs() < 0.01,
            "首次命中时 gamma 不应影响激活度: low={act_low}, high={act_high}"
        );

        // 第二次命中：gamma 高的应有更高激活度
        engine_low.update_activation("测试", None);
        engine_high.update_activation("测试", None);
        let act_low_2 = engine_low.debug_snapshot()[0].2;
        let act_high_2 = engine_high.debug_snapshot()[0].2;
        assert!(
            act_high_2 > act_low_2,
            "第二次命中时高 gamma 应有更高激活度: low={act_low_2}, high={act_high_2}"
        );
    }

    /// 场景 4：阈值调整 — 改变 active_threshold 影响 Active 判定
    #[test]
    fn sim_param_threshold() {
        let params = WorldbookParams {
            active_threshold: 50.0, // 提高阈值
            ..WorldbookParams::default()
        };
        let engine = make_engine(single_entry("thr", "关键词", 0.5), params);

        // 默认 bu=20，首次命中激活度约 20，低于 50 阈值
        engine.update_activation("关键词", None);
        let snap = engine.debug_snapshot();
        let act = snap[0].2;
        assert!(
            act < 50.0,
            "首次命中激活度 {act} 应低于高阈值 50"
        );
        // get_injectable_entries 应返回空（无 Active 条目）
        let injectable = engine.get_injectable_entries();
        assert!(
            injectable.is_empty(),
            "阈值 50 下首次命中不应注入 prompt"
        );
    }

    /// 场景 5：max_active 截断
    #[test]
    fn sim_param_max_active() {
        let params = WorldbookParams {
            max_active: 2,
            active_threshold: 10.0, // 降低阈值，让首次命中（activation≈20）即进入 Active
            ..WorldbookParams::default()
        };
        let entries = vec![
            WorldbookEntry::new("e1", vec!["a".into()], "e1".into(), 0.9),
            WorldbookEntry::new("e2", vec!["b".into()], "e2".into(), 0.8),
            WorldbookEntry::new("e3", vec!["c".into()], "e3".into(), 0.7),
            WorldbookEntry::new("e4", vec!["d".into()], "e4".into(), 0.6),
        ];
        let engine = make_engine(entries, params);

        // 全部命中
        engine.update_activation("a b c d", None);
        let injectable = engine.get_injectable_entries();
        assert_eq!(
            injectable.len(),
            2,
            "max_active=2 应只返回 2 条"
        );
    }
}
