//! 关系认知事实层 — 存储"A 眼中的 B 是什么样的人"这类陈述性关系认知。
//!
//! 与 RelationshipLog（逐轮关系信号轨迹）互补：后者记录每轮发生了什么，
//! 本模块沉淀跨轮稳定的关系印象（人格特质 / 偏好 / 习惯 / 事件印象）。
//!
//! 集成点：
//! - LLM 抽取（由 cross_character.rs 负责）产出 RelationshipFact 后写入本引擎
//! - Semantic Reinforcement 模式：新事件与既有事实共享 ≥2 个 source_event_id 时
//!   调用 reinforce_fact 强化既有事实，而非新增
//! - PromptBuildingStep 读取 format_for_prompt 注入"我对 X 的印象"段
//!
//! 本模块只做存储与检索，不调用 LLM。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 每对 (owner_agent, target_agent) 保留上限
const MAX_FACTS_PER_PAIR: usize = 30;

/// 关系认知事实类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactCategory {
    /// 人格特质（如"她嘴硬但很关心人"）
    Personality,
    /// 偏好（如"她喜欢被夸奖"）
    Preference,
    /// 习惯（如"她总在深夜才活跃"）
    Habit,
    /// 具体事件印象（如"她上次帮我提醒了主人"）
    Incident,
}

impl FactCategory {
    /// prompt 中展示的 PascalCase 标签
    fn label(self) -> &'static str {
        match self {
            FactCategory::Personality => "Personality",
            FactCategory::Preference => "Preference",
            FactCategory::Habit => "Habit",
            FactCategory::Incident => "Incident",
        }
    }
}

/// 单条关系认知事实
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipFact {
    pub id: String,
    /// 谁的认知（如 "vivian"）
    pub owner_agent: String,
    /// 关于谁（如 "nana"）
    pub target_agent: String,
    /// 认知内容（中文陈述句）
    pub fact_text: String,
    pub category: FactCategory,
    /// 置信度 0.0-1.0
    pub confidence: f64,
    /// 从哪些事件沉淀而来（事件 ID 列表）
    #[serde(default)]
    pub source_event_ids: Vec<String>,
    pub created_at: f64,
    pub last_reinforced_at: f64,
    #[serde(default)]
    pub reinforcement_count: u32,
}

/// 关系认知事实引擎
pub struct RelationshipFactsEngine {
    inner: RwLock<RelationshipFactsInner>,
    persistence_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RelationshipFactsInner {
    facts: Vec<RelationshipFact>,
}

static RELATIONSHIP_FACTS_ENGINE: Lazy<Arc<RelationshipFactsEngine>> = Lazy::new(|| {
    Arc::new(RelationshipFactsEngine::new().unwrap_or_else(|e| {
        tracing::error!("[RelationshipFacts] 引擎初始化失败，使用空状态: {e}");
        RelationshipFactsEngine {
            inner: RwLock::new(RelationshipFactsInner::default()),
            persistence_path: PathBuf::from("relationship_facts.json"),
        }
    }))
});

impl RelationshipFactsEngine {
    fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("psychology");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("relationship_facts.json");
        let mut engine = Self {
            inner: RwLock::new(RelationshipFactsInner::default()),
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
        let inner: RelationshipFactsInner = serde_json::from_str(&content).map_err(|e| {
            VivianError::Other(format!("relationship_facts.json 解析失败: {e}"))
        })?;
        *self.inner.write() = inner;
        Ok(())
    }

    fn save_inner(inner: &RelationshipFactsInner, path: &std::path::Path) -> VivianResult<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(inner)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 追加新事实，触发容量淘汰
    pub fn append_fact(&self, fact: RelationshipFact) -> VivianResult<()> {
        let mut inner = self.inner.write();
        let owner = fact.owner_agent.clone();
        let target = fact.target_agent.clone();
        inner.facts.push(fact);
        Self::evict_for_pair(&mut inner.facts, &owner, &target);
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 强化已有事实：reinforcement_count +1，更新 last_reinforced_at，
    /// 追加 source_event_id（去重）
    pub fn reinforce_fact(&self, fact_id: &str, source_event_id: String) -> VivianResult<()> {
        let mut inner = self.inner.write();
        let now = Utc::now().timestamp() as f64;
        let fact = inner
            .facts
            .iter_mut()
            .find(|f| f.id == fact_id)
            .ok_or_else(|| VivianError::Other(format!("relationship fact 不存在: {fact_id}")))?;
        fact.reinforcement_count = fact.reinforcement_count.saturating_add(1);
        fact.last_reinforced_at = now;
        if !source_event_id.is_empty() && !fact.source_event_ids.contains(&source_event_id) {
            fact.source_event_ids.push(source_event_id);
        }
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 查询某对关系的所有事实，按 reinforcement_count 降序排列
    pub fn list_for(&self, owner_agent: &str, target_agent: &str) -> Vec<RelationshipFact> {
        let inner = self.inner.read();
        let mut v: Vec<RelationshipFact> = inner
            .facts
            .iter()
            .filter(|f| f.owner_agent == owner_agent && f.target_agent == target_agent)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.reinforcement_count.cmp(&a.reinforcement_count));
        v
    }

    /// 列出全部关系认知事实（按 created_at 降序），供记忆管理面板使用
    pub fn list_all(&self) -> Vec<RelationshipFact> {
        let inner = self.inner.read();
        let mut v: Vec<RelationshipFact> = inner.facts.iter().cloned().collect();
        v.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// 查找可强化的事实：source_event_ids 交集 ≥ 2 的既有事实，返回其 id
    /// （用于 Semantic Reinforcement 模式）
    ///
    /// 多个候选时取交集最大者，并列时取 reinforcement_count 最高者。
    pub fn find_reinforceable(
        &self,
        owner_agent: &str,
        target_agent: &str,
        source_event_ids: &[String],
    ) -> Option<String> {
        if source_event_ids.is_empty() {
            return None;
        }
        let inner = self.inner.read();
        let mut best: Option<(&RelationshipFact, usize)> = None;
        for f in inner
            .facts
            .iter()
            .filter(|f| f.owner_agent == owner_agent && f.target_agent == target_agent)
        {
            let inter = f
                .source_event_ids
                .iter()
                .filter(|sid| source_event_ids.contains(sid))
                .count();
            if inter < 2 {
                continue;
            }
            let take = match best {
                None => true,
                Some((bf, cur_inter)) => {
                    inter > cur_inter
                        || (inter == cur_inter && f.reinforcement_count > bf.reinforcement_count)
                }
            };
            if take {
                best = Some((f, inter));
            }
        }
        best.map(|(f, _)| f.id.clone())
    }

    /// 格式化为 prompt 段落，按 reinforcement_count 降序取 top `limit` 条。空时返回 None。
    pub fn format_for_prompt(
        &self,
        owner_agent: &str,
        target_agent: &str,
        limit: usize,
        lang: &str,
    ) -> Option<String> {
        let facts = self.list_for(owner_agent, target_agent);
        if facts.is_empty() {
            return None;
        }
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let title_target = capitalize_first(target_agent);
        let header_prefix = crate::pipeline::prompt_modules::section_heading("my_impression", lang);
        let reinforced_label = match lang_norm {
            "en" => "reinforced",
            "ja" => "強化回数",
            _ => "强化次数",
        };
        let header = match lang_norm {
            "en" => format!("{} {}", header_prefix, title_target),
            "ja" => format!("{}{}", header_prefix, title_target),
            _ => format!("{}{}的印象", header_prefix, title_target),
        };
        let mut lines: Vec<String> = Vec::new();
        lines.push(header);
        for f in facts.iter().take(limit) {
            lines.push(format!(
                "- [{}] {} ({}: {})",
                f.category.label(),
                f.fact_text,
                reinforced_label,
                f.reinforcement_count
            ));
        }
        Some(lines.join("\n"))
    }

    /// 清空全部关系认知事实
    ///
    /// 用于「清空记忆」操作：关系事实是从交互中衍生的认知层，
    /// 私有记忆清空后，关系事实也应一并清空，避免数据不一致。
    pub fn clear_all(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.facts.clear();
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 清除指定角色参与的关系认知事实
    ///
    /// 保留其他角色之间的事实。用于单角色记忆清空场景。
    pub fn clear_for_character(&self, char_id: &str) -> VivianResult<()> {
        let mut inner = self.inner.write();
        let before = inner.facts.len();
        inner.facts.retain(|f| f.owner_agent != char_id && f.target_agent != char_id);
        let dropped = before - inner.facts.len();
        if dropped > 0 {
            Self::save_inner(&inner, &self.persistence_path)?;
            tracing::info!("[RelationshipFacts] 已清除 {} 相关事实 {} 条", char_id, dropped);
        }
        Ok(())
    }

    /// 对某对关系执行容量淘汰：超出 MAX_FACTS_PER_PAIR 时，
    /// FIFO 淘汰 reinforcement_count == 0 中最旧的（按 created_at 升序）。
    /// 若无可淘汰的零计数事实，则保持现状。
    fn evict_for_pair(facts: &mut Vec<RelationshipFact>, owner: &str, target: &str) {
        let pair_count = facts
            .iter()
            .filter(|f| f.owner_agent == owner && f.target_agent == target)
            .count();
        if pair_count <= MAX_FACTS_PER_PAIR {
            return;
        }
        let to_remove = pair_count - MAX_FACTS_PER_PAIR;

        // 收集该对关系中 reinforcement_count == 0 的事实索引，按 created_at 升序
        let mut zero_idx: Vec<usize> = facts
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.owner_agent == owner
                    && f.target_agent == target
                    && f.reinforcement_count == 0
            })
            .map(|(i, _)| i)
            .collect();
        zero_idx.sort_by(|&a, &b| {
            facts[a]
                .created_at
                .partial_cmp(&facts[b].created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let ids_to_remove: std::collections::HashSet<String> = zero_idx
            .iter()
            .take(to_remove)
            .map(|&i| facts[i].id.clone())
            .collect();
        if ids_to_remove.is_empty() {
            return;
        }
        facts.retain(|f| !ids_to_remove.contains(&f.id));
    }
}

/// 获取全局关系认知事实引擎
pub fn relationship_facts() -> Arc<RelationshipFactsEngine> {
    Arc::clone(&RELATIONSHIP_FACTS_ENGINE)
}

/// 首字母大写（用于 prompt 标题中展示 target_agent 名称）
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
