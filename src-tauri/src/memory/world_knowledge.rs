//! 共享世界记忆层（World Knowledge）—— 两个角色共同知晓的世界事实存储。
//!
//! 区别于 `unified_event_ledger`（存一次性事件流），本层存储**持续性事实**，
//! 如"用户喜欢原神""桌宠家规""用户住在上海"等长期有效的世界知识。
//!
//! 设计要点：
//! - **全局单例**：所有角色共享同一份世界事实库。
//! - **持久化**：JSON 文件存储，tmp 文件 + rename 原子写入。
//! - **容量上限**：100 条，FIFO 淘汰最旧条目。
//! - **强化机制**：同一事实被多次观察时通过 `reinforce_fact` 累积权重，
//!   避免重复存储相似事实。
//! - **去重**：`find_similar` 用文本匹配（同 category + 完全相同或包含关系），
//!   先于 embedding 调用，节省成本。
//! - **不调用 LLM**：事实抽取由调用方（manager.rs）负责，本模块只做存储。
//!
//! 集成点：
//! - `MemoryManager` 在 LLM 抽取出世界事实后调用 `append_fact` / `reinforce_fact`
//! - `PromptBuildingStep` 调用 `format_for_prompt` 注入"共享世界知识"段落

use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 世界事实保留上限（FIFO 淘汰最早条目）
const MAX_FACTS: usize = 100;

/// 世界事实类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldFactCategory {
    /// 用户偏好（如"用户喜欢原神"）
    UserPreference,
    /// 家规/约定（如"用户工作时不要打扰"）
    HouseRule,
    /// 环境事实（如"用户住在上海"）
    Environment,
    /// 共同事件总结（如"上周用户考试失败，我们一起安慰了他"）
    SharedEvent,
}

impl WorldFactCategory {
    /// 返回用于 prompt 显示的 PascalCase 名称
    fn as_display_str(self) -> &'static str {
        match self {
            WorldFactCategory::UserPreference => "UserPreference",
            WorldFactCategory::HouseRule => "HouseRule",
            WorldFactCategory::Environment => "Environment",
            WorldFactCategory::SharedEvent => "SharedEvent",
        }
    }
}

/// 单条共享世界事实
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldFact {
    pub id: String,
    pub fact_text: String,
    pub category: WorldFactCategory,
    /// 重要性 0.0-1.0
    pub importance: f64,
    /// 贡献者列表（哪些角色贡献过这条事实）["vivian", "nana"]
    #[serde(default)]
    pub contributors: Vec<String>,
    /// 从哪些事件沉淀而来
    #[serde(default)]
    pub source_event_ids: Vec<String>,
    pub created_at: f64,
    pub last_reinforced_at: f64,
    #[serde(default)]
    pub reinforcement_count: u32,
}

/// 世界知识引擎内部状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorldKnowledgeInner {
    facts: Vec<WorldFact>,
}

/// 共享世界知识引擎
pub struct WorldKnowledgeEngine {
    inner: RwLock<WorldKnowledgeInner>,
    persistence_path: PathBuf,
}

static WORLD_KNOWLEDGE_ENGINE: Lazy<Arc<WorldKnowledgeEngine>> = Lazy::new(|| {
    Arc::new(WorldKnowledgeEngine::new().unwrap_or_else(|e| {
        tracing::error!("[WorldKnowledge] 引擎初始化失败，使用空状态: {e}");
        WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner::default()),
            persistence_path: PathBuf::from("world_knowledge.json"),
        }
    }))
});

impl WorldKnowledgeEngine {
    fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("memory");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("world_knowledge.json");
        let mut engine = Self {
            inner: RwLock::new(WorldKnowledgeInner::default()),
            persistence_path: path,
        };
        engine.load()?;
        Ok(engine)
    }

    fn load(&mut self) -> VivianResult<()> {
        if !self.persistence_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.persistence_path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        match serde_json::from_str::<WorldKnowledgeInner>(&content) {
            Ok(inner) => {
                *self.inner.write() = inner;
            }
            Err(e) => {
                tracing::warn!(
                    "[WorldKnowledge] world_knowledge.json 解析失败，使用空状态: {e}"
                );
            }
        }
        Ok(())
    }

    fn save_inner(inner: &WorldKnowledgeInner, path: &std::path::Path) -> VivianResult<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(inner)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 追加一条新事实，触发容量淘汰
    pub fn append_fact(&self, fact: WorldFact) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.facts.push(fact);
        // FIFO 淘汰最旧
        if inner.facts.len() > MAX_FACTS {
            let drop_n = inner.facts.len() - MAX_FACTS;
            inner.facts.drain(0..drop_n);
        }
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 强化已有事实：reinforcement_count +1，更新 last_reinforced_at，
    /// 追加 contributor（去重），追加 source_event_id（去重）。
    ///
    /// 若 `fact_id` 不存在返回错误。
    pub fn reinforce_fact(
        &self,
        fact_id: &str,
        contributor: &str,
        source_event_id: String,
    ) -> VivianResult<()> {
        let mut inner = self.inner.write();
        let now = now_ts();
        let fact = inner
            .facts
            .iter_mut()
            .find(|f| f.id == fact_id)
            .ok_or_else(|| VivianError::Other(format!("世界事实不存在: {fact_id}")))?;

        fact.reinforcement_count = fact.reinforcement_count.saturating_add(1);
        fact.last_reinforced_at = now;
        if !fact.contributors.iter().any(|c| c == contributor) {
            fact.contributors.push(contributor.to_string());
        }
        if !fact.source_event_ids.iter().any(|s| s == &source_event_id) {
            fact.source_event_ids.push(source_event_id);
        }
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 查找相似事实：同 category 且 fact_text 完全相同或包含关系，返回其 id。
    ///
    /// 用于去重，避免调用 embedding 的成本，先用文本匹配。
    /// 包含关系为双向：新文本包含已有文本，或已有文本包含新文本。
    pub fn find_similar(&self, fact_text: &str, category: WorldFactCategory) -> Option<String> {
        let inner = self.inner.read();
        let needle = fact_text.trim();
        for f in inner.facts.iter() {
            if f.category != category {
                continue;
            }
            let existing = f.fact_text.trim();
            if existing == needle {
                return Some(f.id.clone());
            }
            if !needle.is_empty()
                && !existing.is_empty()
                && (existing.contains(needle) || needle.contains(existing))
            {
                return Some(f.id.clone());
            }
        }
        None
    }

    /// 按 `importance * (1.0 + reinforcement_count as f64 * 0.1)` 加权降序取 top `limit` 条
    pub fn list_top(&self, limit: usize) -> Vec<WorldFact> {
        let inner = self.inner.read();
        let mut scored: Vec<(f64, WorldFact)> = inner
            .facts
            .iter()
            .map(|f| {
                let score = f.importance * (1.0 + f.reinforcement_count as f64 * 0.1);
                (score, f.clone())
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(limit).map(|(_, f)| f).collect()
    }

    /// 列出全部共享世界事实（按 created_at 降序），供记忆管理面板使用
    pub fn list_all(&self) -> Vec<WorldFact> {
        let inner = self.inner.read();
        let mut v: Vec<WorldFact> = inner.facts.iter().cloned().collect();
        v.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// 格式化为 prompt 段落，按 `list_top` 排序。空时返回 None。
    ///
    /// 格式示例：
    /// ```text
    /// ## Shared World Knowledge
    /// - [UserPreference] 用户喜欢原神 (importance: 0.8, reinforced: 3)
    /// - [HouseRule] 用户工作时不要打扰 (importance: 0.7, reinforced: 1)
    /// ```
    pub fn format_for_prompt(&self, limit: usize, lang: &str) -> Option<String> {
        let facts = self.list_top(limit);
        if facts.is_empty() {
            return None;
        }
        Some(Self::render_facts(&facts, lang))
    }

    /// 上下文感知的 prompt 格式化：优先返回与当前对话关键词匹配的事实。
    ///
    /// 策略：
    /// 1. 匹配事实：fact_text 包含任一关键词（大小写不敏感）
    /// 2. 锚定事实：无论是否匹配，始终包含 top 2 高重要性事实（家规、高强化次数等）
    /// 3. 去重合并后按加权分数降序
    /// 4. 若无匹配且无锚定 → 回退到标准 top-N
    ///
    /// 避免全量注入 100 条事实造成 prompt 膨胀，同时确保关键规则不丢失。
    pub fn format_for_prompt_with_context(
        &self,
        limit: usize,
        keywords: &[String],
        lang: &str,
    ) -> Option<String> {
        if keywords.is_empty() {
            return self.format_for_prompt(limit, lang);
        }

        let inner = self.inner.read();
        if inner.facts.is_empty() {
            return None;
        }

        // 按加权分数降序排列所有事实
        let mut scored: Vec<(f64, usize)> = inner
            .facts
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let score = f.importance * (1.0 + f.reinforcement_count as f64 * 0.1);
                (score, i)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let kw_lower: Vec<String> = keywords.iter().map(|k| k.to_lowercase()).collect();

        // 匹配事实：fact_text 包含任一关键词
        let mut matched_indices: Vec<usize> = Vec::new();
        for &(_, idx) in &scored {
            let text_lower = inner.facts[idx].fact_text.to_lowercase();
            if kw_lower.iter().any(|kw| text_lower.contains(kw.as_str())) {
                matched_indices.push(idx);
            }
            if matched_indices.len() >= limit {
                break;
            }
        }

        // 锚定事实：top 2（无论是否匹配关键词）
        let anchor_count = 2.min(scored.len());
        let anchor_indices: Vec<usize> = scored[..anchor_count].iter().map(|&(_, i)| i).collect();

        // 合并去重
        let mut selected: Vec<usize> = matched_indices;
        for ai in anchor_indices {
            if !selected.contains(&ai) {
                selected.push(ai);
            }
        }

        if selected.is_empty() {
            // 回退到标准 top-N
            drop(inner);
            return self.format_for_prompt(limit, lang);
        }

        // 按加权分数排序选中事实
        let mut selected_facts: Vec<(f64, WorldFact)> = selected
            .into_iter()
            .map(|idx| {
                let f = &inner.facts[idx];
                let score = f.importance * (1.0 + f.reinforcement_count as f64 * 0.1);
                (score, f.clone())
            })
            .collect();
        selected_facts.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        drop(inner);

        let facts: Vec<WorldFact> = selected_facts.into_iter().map(|(_, f)| f).collect();
        Some(Self::render_facts(&facts, lang))
    }

    /// 渲染事实列表为 prompt 段落
    fn render_facts(facts: &[WorldFact], lang: &str) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(facts.len() + 1);
        let header = crate::pipeline::prompt_modules::section_heading("shared_world_knowledge", lang);
        lines.push(header.to_string());
        for f in facts {
            lines.push(format!(
                "- [{}] {}",
                f.category.as_display_str(),
                f.fact_text
            ));
        }
        lines.join("\n")
    }

    /// 清空全部共享世界知识
    ///
    /// 用于「清空记忆」操作：世界知识是从对话/记忆中抽取的衍生层，
    /// 私有记忆清空后，世界知识也应一并清空。
    pub fn clear_all(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.facts.clear();
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }
}

/// 当前时间戳（秒）
fn now_ts() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_else(|_| 0.0)
}

/// 获取全局共享世界知识引擎
pub fn world_knowledge() -> Arc<WorldKnowledgeEngine> {
    Arc::clone(&WORLD_KNOWLEDGE_ENGINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fact(
        id: &str,
        text: &str,
        category: WorldFactCategory,
        importance: f64,
        ts: f64,
    ) -> WorldFact {
        WorldFact {
            id: id.to_string(),
            fact_text: text.to_string(),
            category,
            importance,
            contributors: vec![],
            source_event_ids: vec![],
            created_at: ts,
            last_reinforced_at: ts,
            reinforcement_count: 0,
        }
    }

    #[test]
    fn test_find_similar_exact_match_same_category() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![make_fact(
                    "f1",
                    "用户喜欢原神",
                    WorldFactCategory::UserPreference,
                    0.8,
                    1.0,
                )],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        let id = engine.find_similar("用户喜欢原神", WorldFactCategory::UserPreference);
        assert_eq!(id.as_deref(), Some("f1"));
    }

    #[test]
    fn test_find_similar_different_category_no_match() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![make_fact(
                    "f1",
                    "用户喜欢原神",
                    WorldFactCategory::UserPreference,
                    0.8,
                    1.0,
                )],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        let id = engine.find_similar("用户喜欢原神", WorldFactCategory::HouseRule);
        assert!(id.is_none());
    }

    #[test]
    fn test_find_similar_contains_relationship() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![make_fact(
                    "f1",
                    "用户喜欢原神这款游戏",
                    WorldFactCategory::UserPreference,
                    0.8,
                    1.0,
                )],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        // 已有文本包含新文本
        let id = engine.find_similar("原神", WorldFactCategory::UserPreference);
        assert_eq!(id.as_deref(), Some("f1"));
    }

    #[test]
    fn test_list_top_orders_by_weighted_score() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![
                    // importance 0.5, reinforced 0 -> 0.5
                    make_fact(
                        "low",
                        "low",
                        WorldFactCategory::Environment,
                        0.5,
                        1.0,
                    ),
                    // importance 0.5, reinforced 5 -> 0.5 * 1.5 = 0.75
                    {
                        let mut f = make_fact(
                            "mid",
                            "mid",
                            WorldFactCategory::Environment,
                            0.5,
                            2.0,
                        );
                        f.reinforcement_count = 5;
                        f
                    },
                    // importance 0.9, reinforced 0 -> 0.9
                    make_fact(
                        "high",
                        "high",
                        WorldFactCategory::Environment,
                        0.9,
                        3.0,
                    ),
                ],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        let top = engine.list_top(3);
        assert_eq!(top[0].id, "high");
        assert_eq!(top[1].id, "mid");
        assert_eq!(top[2].id, "low");
    }

    #[test]
    fn test_format_for_prompt_empty_returns_none() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner::default()),
            persistence_path: PathBuf::from("test.json"),
        };
        assert!(engine.format_for_prompt(10, "zh").is_none());
    }

    #[test]
    fn test_format_for_prompt_format() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![{
                    let mut f = make_fact(
                        "f1",
                        "用户喜欢原神",
                        WorldFactCategory::UserPreference,
                        0.8,
                        1.0,
                    );
                    f.reinforcement_count = 3;
                    f
                }],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        let s = engine.format_for_prompt(10, "zh").unwrap();
        assert!(s.contains("## 共享世界知识"));
        assert!(s.contains("[UserPreference] 用户喜欢原神"));
        assert!(s.contains("重要性: 0.80"));
        assert!(s.contains("强化次数: 3"));
    }

    #[test]
    fn test_reinforce_fact_updates_and_dedups() {
        // 使用真实临时文件以验证完整写盘流程。
        let dir = std::env::temp_dir();
        let path = dir.join("vivian_world_knowledge_test.json");
        let _ = std::fs::remove_file(&path);
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![make_fact(
                    "f1",
                    "用户喜欢原神",
                    WorldFactCategory::UserPreference,
                    0.8,
                    1.0,
                )],
            }),
            persistence_path: path.clone(),
        };
        engine
            .reinforce_fact("f1", "vivian", "evt-1".to_string())
            .unwrap();
        engine
            .reinforce_fact("f1", "nana", "evt-2".to_string())
            .unwrap();
        // 重复 contributor 与 source_event_id 应被去重
        engine
            .reinforce_fact("f1", "vivian", "evt-1".to_string())
            .unwrap();

        let inner = engine.inner.read();
        let f = &inner.facts[0];
        assert_eq!(f.reinforcement_count, 3);
        assert_eq!(f.contributors, vec!["vivian", "nana"]);
        assert_eq!(f.source_event_ids, vec!["evt-1", "evt-2"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_reinforce_fact_missing_id_errors() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner::default()),
            persistence_path: PathBuf::from("test.json"),
        };
        let res = engine.reinforce_fact("missing", "vivian", "evt-1".to_string());
        assert!(res.is_err());
    }

    #[test]
    fn test_format_for_prompt_with_context_keyword_match() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![
                    make_fact("f1", "用户喜欢原神", WorldFactCategory::UserPreference, 0.8, 1.0),
                    make_fact("f2", "用户工作时不要打扰", WorldFactCategory::HouseRule, 0.9, 2.0),
                    make_fact("f3", "用户住在上海", WorldFactCategory::Environment, 0.5, 3.0),
                    make_fact("f4", "用户喜欢打游戏", WorldFactCategory::UserPreference, 0.6, 4.0),
                ],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        // 关键词 "游戏" 不匹配任何事实，但 "原神" 匹配 f1
        let keywords = vec!["原神".to_string()];
        let s = engine.format_for_prompt_with_context(3, &keywords, "zh").unwrap();
        assert!(s.contains("用户喜欢原神"), "应包含关键词匹配的事实");
        // 锚定事实：top 2 by score (f2=0.9, f1=0.8) 应始终包含
        assert!(s.contains("用户工作时不要打扰"), "应包含锚定高重要性事实");
    }

    #[test]
    fn test_format_for_prompt_with_context_empty_keywords_fallback() {
        let engine = WorldKnowledgeEngine {
            inner: RwLock::new(WorldKnowledgeInner {
                facts: vec![make_fact(
                    "f1",
                    "用户喜欢原神",
                    WorldFactCategory::UserPreference,
                    0.8,
                    1.0,
                )],
            }),
            persistence_path: PathBuf::from("test.json"),
        };
        let s = engine.format_for_prompt_with_context(5, &[], "zh").unwrap();
        // 空关键词应回退到标准 top-N
        assert!(s.contains("用户喜欢原神"));
    }
}
