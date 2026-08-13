//! 用户认知模型（User Model）
//!
//! 将散落的记忆证据组织成"对这个人的理解"。
//!
//! 核心职责：
//! - 管理 UserTrait（偏好/工作风格/价值观等稳定抽象）
//! - 管理 UserGoal（长期/当前/已完成目标）
//! - 管理 UserProject（项目生命周期 + 当前注意力激活）
//! - 强证据在线更新（规则匹配，不调用 LLM）
//! - 弱证据进入后台候选池，等待批量归纳
//! - 所有 trait 均可反向追溯至记忆证据
//!
//! 与现有系统的关系：
//! - Memory 负责"记住具体经历"
//! - UserModel 负责"理解这个人"
//! - Observation 负责"感知当前状态"
//! - 三者并行，在 Context Builder 层汇合

use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::VivianResult;
use crate::utils::path;

// ============================================================================
// 常量
// ============================================================================

/// User Model 持久化文件名
const USER_MODEL_FILE: &str = "user_model.json";
/// 候选 Trait 提升为 StableTrait 的置信度阈值
const CANDIDATE_PROMOTION_THRESHOLD: f64 = 0.70;
/// 候选 Trait 因证据不足而降级的衰减因子
const CANDIDATE_DECAY_FACTOR: f64 = 0.95;
/// Trait 稳定所需的连续证据最少次数
const STABILITY_MIN_EVIDENCE_COUNT: u32 = 3;
/// Trait 稳定所需的证据时间跨度（天）
const STABILITY_MIN_SPAN_DAYS: f64 = 14.0;
/// 项目激活衰减率（每非活跃日衰减）
const PROJECT_ACTIVATION_DECAY: f64 = 0.85;
/// 项目激活升到活跃状态所需的阈值
const PROJECT_ACTIVE_THRESHOLD: f64 = 0.70;
/// 项目激活降到休眠状态的阈值
#[allow(dead_code)]
const PROJECT_DORMANT_THRESHOLD: f64 = 0.15;

// ============================================================================
// 核心枚举
// ============================================================================

/// Trait 类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserTraitCategory {
    /// 偏好（如 UI 风格、技术选型等）
    Preference,
    /// 工作方式（如倾向工程落地 vs 理论研究）
    WorkStyle,
    /// 沟通风格（如直接、委婉等）
    CommunicationStyle,
    /// 价值观（如重视实用性、完整性等）
    Value,
    /// 兴趣领域（如 AI、游戏、设计等）
    Interest,
    /// 能力特征（如 Rust、推荐系统等）
    Skill,
}

impl UserTraitCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::WorkStyle => "work_style",
            Self::CommunicationStyle => "communication_style",
            Self::Value => "value",
            Self::Interest => "interest",
            Self::Skill => "skill",
        }
    }
}

/// Trait 生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraitLifecycle {
    /// 刚出现，证据不足
    Emerging,
    /// 有足够证据，但尚未验证长期性
    Active,
    /// 经过验证，长期稳定
    Stable,
    /// 证据在减弱，正在消退
    Fading,
    /// 出现矛盾证据
    Contradicted,
}

/// 项目生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    /// 正在活跃开发/关注
    Active,
    /// 暂时不活跃，但可能恢复
    Dormant,
    /// 已完成
    Completed,
    /// 已放弃
    Abandoned,
}

/// 目标状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// 长期目标（持续进行中）
    LongTerm,
    /// 当前目标（正在积极追求）
    Current,
    /// 已完成
    Completed,
    /// 已放弃
    Abandoned,
}

// ============================================================================
// 核心数据结构
// ============================================================================

/// 候选 Trait（尚未达到稳定阈值的观察假设）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateTrait {
    pub category: UserTraitCategory,
    pub key: String,
    pub value: String,
    /// 当前置信度 [0.0, 1.0]
    pub confidence: f64,
    /// 证据记忆 ID 列表
    pub evidence_ids: Vec<String>,
    /// 首次观察时间
    pub first_observed_at: f64,
    /// 最近一次证据时间
    pub last_observed_at: f64,
    /// 观察次数
    pub observation_count: u32,
}

/// 用户特征（稳定的抽象理解）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTrait {
    pub category: UserTraitCategory,
    /// 特征键（如 "ui_style", "engineering_vs_research"）
    pub key: String,
    /// 特征值（如 "custom_css", "engineering"）
    pub value: String,
    /// 概念含义：一句话说明"用户长期在乎什么 / 为什么"（概念层的语义表达）
    #[serde(default)]
    pub meaning: String,
    /// 综合置信度 [0.0, 1.0]
    pub confidence: f64,
    /// 稳定性 [0.0, 1.0]（区分"现在喜欢"和"长期稳定"）
    pub stability: f64,
    /// 重要性 [0.0, 1.0]
    pub importance: f64,
    /// 适用范围（如 "project:vivian", "frontend", "global"）
    pub scope: String,
    /// 证据记忆 ID 列表（可反向追溯）
    pub evidence_ids: Vec<String>,
    /// 关联话题（多跳检索锚点，如 agent_autonomy → [proactive, inner_monologue, web_search]）
    #[serde(default)]
    pub related_topics: Vec<String>,
    /// 生命周期状态
    pub lifecycle: TraitLifecycle,
    /// 创建时间
    pub created_at: f64,
    /// 最后更新时间
    pub updated_at: f64,
    /// 证据计数
    pub evidence_count: u32,
    /// 矛盾证据计数
    pub contradiction_count: u32,
}

/// 用户目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGoal {
    /// 目标描述
    pub description: String,
    /// 目标状态
    pub status: GoalStatus,
    /// 重要性 [0.0, 1.0]
    pub importance: f64,
    /// 证据记忆 ID 列表
    pub evidence_ids: Vec<String>,
    /// 创建时间
    pub created_at: f64,
    /// 最后更新时间
    pub updated_at: f64,
}

/// 用户项目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProject {
    /// 项目名称
    pub name: String,
    /// 项目描述
    pub description: String,
    /// 生命周期状态
    pub status: ProjectStatus,
    /// 当前阶段描述（如 "cognitive_architecture", "memory_optimization"）
    pub phase: String,
    /// 当前关注点列表
    pub focus: Vec<String>,
    /// 当前注意力激活度 [0.0, 1.0]（由语义信号动态计算，非持久化）
    #[serde(default)]
    pub activation: f64,
    /// 关联话题关键词（用于话题匹配计算 activation）
    pub topics: Vec<String>,
    /// 相关记忆 ID 列表
    pub related_memory_ids: Vec<String>,
    /// 创建时间
    pub created_at: f64,
    /// 最后活跃时间
    pub last_active_at: f64,
}

impl UserProject {
    /// 计算当前激活度（基于话题匹配 + 时间衰减）
    pub fn calculate_activation(&self, matched_topics: &[String]) -> f64 {
        let now = crate::memory::types::current_timestamp();
        let days_since_active = (now - self.last_active_at) / 86400.0;

        // 基础衰减：随时间衰减
        let time_decay = PROJECT_ACTIVATION_DECAY.powf(days_since_active.max(0.0));

        // 话题匹配提升
        let topic_boost = if matched_topics.is_empty() || self.topics.is_empty() {
            0.0
        } else {
            let match_count = self
                .topics
                .iter()
                .filter(|t| matched_topics.iter().any(|mt| mt.to_lowercase().contains(&t.to_lowercase()) || t.to_lowercase().contains(&mt.to_lowercase())))
                .count() as f64;
            let total = self.topics.len() as f64;
            if total > 0.0 {
                (match_count / total).min(1.0) * 0.5
            } else {
                0.0
            }
        };

        let raw = self.activation * time_decay + topic_boost;
        raw.clamp(0.0, 1.0)
    }
}

// ============================================================================
// 聚合结构
// ============================================================================

/// 用户认知模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    /// 稳定特征列表
    pub traits: Vec<UserTrait>,
    /// 候选特征（尚未稳定的观察假设）
    pub candidate_traits: Vec<CandidateTrait>,
    /// 目标列表
    pub goals: Vec<UserGoal>,
    /// 项目列表
    pub projects: Vec<UserProject>,
    /// 最后更新时间
    pub updated_at: f64,
}

impl UserModel {
    pub fn empty() -> Self {
        Self {
            traits: Vec::new(),
            candidate_traits: Vec::new(),
            goals: Vec::new(),
            projects: Vec::new(),
            updated_at: crate::memory::types::current_timestamp(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.traits.is_empty()
            && self.candidate_traits.is_empty()
            && self.goals.is_empty()
            && self.projects.is_empty()
    }

    /// 获取活跃项目列表（按 activation 降序）
    pub fn active_projects(&self) -> Vec<&UserProject> {
        let mut active: Vec<&UserProject> = self
            .projects
            .iter()
            .filter(|p| p.status == ProjectStatus::Active || p.activation >= PROJECT_ACTIVE_THRESHOLD)
            .collect();
        active.sort_by(|a, b| b.activation.partial_cmp(&a.activation).unwrap_or(std::cmp::Ordering::Equal));
        active
    }

    /// 获取稳定特征列表（按 importance 降序）
    pub fn stable_traits(&self) -> Vec<&UserTrait> {
        let mut stable: Vec<&UserTrait> = self
            .traits
            .iter()
            .filter(|t| t.lifecycle == TraitLifecycle::Stable || t.lifecycle == TraitLifecycle::Active)
            .collect();
        stable.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
        stable
    }

    /// 将与当前话题相关的特征/项目展开为关联话题（多跳检索锚点）。
    ///
    /// 输入为当前对话的话题标签（来自 FastSemantic 或用户输入分词），
    /// 对每个词：若命中某特征（key/value/related_topics 子串匹配）则展开它的
    /// `related_topics`；若命中某项目（name/topics 子串匹配）则展开它的 `topics`。
    /// 结果去重。展开得到的关联话题用于召回"与当前话题在概念上相关、但字面不相似"的旧记忆。
    pub fn expand_related_topics(&self, terms: &[String]) -> Vec<String> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<String> = Vec::new();
        let lower_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
        if lower_terms.is_empty() {
            return out;
        }

        let push_unique = |topic: &str, seen: &mut std::collections::HashSet<String>, out: &mut Vec<String>| {
            let key = topic.trim().to_lowercase();
            if key.is_empty() || seen.contains(&key) {
                return;
            }
            seen.insert(key);
            out.push(topic.trim().to_string());
        };

        for tr in &self.traits {
            let hay = format!(
                "{} {} {}",
                tr.key,
                tr.value,
                tr.related_topics.join(" ")
            )
            .to_lowercase();
            if lower_terms.iter().any(|t| hay.contains(t)) {
                for rt in &tr.related_topics {
                    push_unique(rt, &mut seen, &mut out);
                }
            }
        }

        for p in &self.projects {
            if p.status == ProjectStatus::Abandoned {
                continue;
            }
            let hay = format!("{} {}", p.name, p.topics.join(" ")).to_lowercase();
            if lower_terms.iter().any(|t| hay.contains(t)) {
                for pt in &p.topics {
                    push_unique(pt, &mut seen, &mut out);
                }
            }
        }

        out
    }

    /// 将当前话题关联到最近更新的特征（共现式关联构建）。
    ///
    /// 当用户围绕某个长期特征讨论时，把当前话题补充进该特征的 `related_topics`，
    /// 后续"surface 相似但语义相关"的输入就能通过该关联展开到相关旧话题。
    /// 仅对最近 `window_secs` 内更新过的特征生效，避免把无关话题硬塞给冷淡特征。
    pub fn associate_active_traits_with_topics(&mut self, topics: &[String], window_secs: f64) {
        if topics.is_empty() {
            return;
        }
        let now = crate::memory::types::current_timestamp();
        for tr in &mut self.traits {
            // 只关联活跃/稳定特征，且最近更新过
            if tr.lifecycle != TraitLifecycle::Stable && tr.lifecycle != TraitLifecycle::Active {
                continue;
            }
            if now - tr.updated_at > window_secs {
                continue;
            }
            for t in topics {
                let key = t.trim().to_lowercase();
                if key.is_empty() {
                    continue;
                }
                // 避免把特征自身的 key/value 重复塞进关联
                if key == tr.key.to_lowercase() || key == tr.value.to_lowercase() {
                    continue;
                }
                if !tr.related_topics.iter().any(|rt| rt.to_lowercase() == key) {
                    tr.related_topics.push(t.trim().to_string());
                }
            }
        }
    }

    /// 归并一个概念（模型层纯逻辑，无 IO）。
    ///
    /// 由 `UserModelManager::merge_concept` 委托调用，便于测试与复用。
    /// 已存在同名概念则强化（合并 meaning / related_topics / evidence / 上浮 strength），
    /// 否则新建（category=Value，lifecycle=Active）。
    pub fn merge_concept(
        &mut self,
        key: &str,
        value: &str,
        meaning: &str,
        related_topics: &[String],
        evidence_ids: &[String],
        strength: f64,
    ) {
        let now = crate::memory::types::current_timestamp();
        if let Some(t) = self.traits.iter_mut().find(|t| t.key == key) {
            if !meaning.trim().is_empty() {
                t.meaning = meaning.trim().to_string();
            }
            t.value = value.to_string();
            for rt in related_topics {
                let key_rt = rt.trim().to_lowercase();
                if key_rt.is_empty() {
                    continue;
                }
                if !t.related_topics.iter().any(|x| x.to_lowercase() == key_rt) {
                    t.related_topics.push(rt.trim().to_string());
                }
            }
            for eid in evidence_ids {
                if !t.evidence_ids.contains(eid) {
                    t.evidence_ids.push(eid.clone());
                    t.evidence_count += 1;
                }
            }
            t.confidence = (t.confidence + strength * 0.1).min(0.95);
            t.importance = (t.importance + strength * 0.08).min(1.0);
            t.updated_at = now;
            if t.lifecycle != TraitLifecycle::Contradicted {
                t.lifecycle = if t.evidence_count >= STABILITY_MIN_EVIDENCE_COUNT {
                    TraitLifecycle::Stable
                } else {
                    TraitLifecycle::Active
                };
            }
        } else {
            self.traits.push(UserTrait {
                category: UserTraitCategory::Value,
                key: key.to_string(),
                value: value.to_string(),
                meaning: meaning.trim().to_string(),
                confidence: strength.min(1.0),
                stability: 0.0,
                importance: strength.min(1.0),
                scope: "global".to_string(),
                evidence_ids: evidence_ids.to_vec(),
                related_topics: related_topics.to_vec(),
                lifecycle: TraitLifecycle::Active,
                created_at: now,
                updated_at: now,
                evidence_count: evidence_ids.len() as u32,
                contradiction_count: 0,
            });
        }
        self.updated_at = now;
    }

    /// 为 prompt 格式化 User Model 段落
    pub fn format_for_prompt(&self, lang: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut sections: Vec<String> = Vec::new();

        // 活跃项目
        let active_projects = self.active_projects();
        if !active_projects.is_empty() {
            let mut lines = Vec::new();
            for p in active_projects {
                let focus_str = if p.focus.is_empty() {
                    String::new()
                } else {
                    format!("（关注：{}）", p.focus.join("、"))
                };
                lines.push(format!("- {} [{:?}]: {}{}", p.name, p.status, p.phase, focus_str));
            }
            sections.push(format!("【当前项目】\n{}", lines.join("\n")));
        }

        // 稳定特征
        let stable_traits = self.stable_traits();
        if !stable_traits.is_empty() {
            let mut lines = Vec::new();
            for t in stable_traits {
                let scope_hint = if t.scope.is_empty() || t.scope == "global" {
                    String::new()
                } else {
                    format!("（{}）", t.scope)
                };
                lines.push(format!("- {}: {} {}", t.key, t.value, scope_hint));
            }
            sections.push(format!("【对你的了解】\n{}", lines.join("\n")));
        }

        // 当前目标
        let current_goals: Vec<&UserGoal> = self
            .goals
            .iter()
            .filter(|g| g.status == GoalStatus::Current || g.status == GoalStatus::LongTerm)
            .collect();
        if !current_goals.is_empty() {
            let mut lines = Vec::new();
            for g in current_goals {
                lines.push(format!("- {}", g.description));
            }
            sections.push(format!("【你的目标】\n{}", lines.join("\n")));
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let heading = match lang_norm {
            "en" => "## My Understanding of You",
            "ja" => "## あなたへの理解",
            _ => "## 我对你的了解",
        };

        Some(format!("{}\n\n{}", heading, sections.join("\n\n")))
    }
}

// ============================================================================
// UserModelManager
// ============================================================================

/// 用户认知模型管理器
///
/// 管理 UserModel 的加载、保存、更新。
/// 不调用 LLM——强证据更新基于规则匹配，弱证据进入候选池。
pub struct UserModelManager {
    inner: RwLock<UserModel>,
    store_path: PathBuf,
    /// 角色 ID（用于持久化路径隔离）
    pub char_id: String,
}

impl UserModelManager {
    /// 创建 UserModelManager，从磁盘加载已有数据
    pub fn new(char_id: &str) -> Self {
        let store_path = Self::store_path(char_id);
        let model = Self::load_from_disk(&store_path).unwrap_or_else(|_| UserModel::empty());
        Self {
            inner: RwLock::new(model),
            store_path,
            char_id: char_id.to_string(),
        }
    }

    /// 获取 UserModel 引用
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, UserModel> {
        self.inner.read()
    }

    /// 为 prompt 格式化 User Model 段落
    pub fn format_for_prompt(&self, lang: &str) -> Option<String> {
        self.inner.read().format_for_prompt(lang)
    }

    // ── 强证据在线更新 ──

    /// 添加强证据并更新 Trait
    ///
    /// 由 AutoExtractor 在检测到强表达时调用。
    /// 规则匹配，不调用 LLM。
    pub fn apply_strong_evidence(
        &self,
        category: UserTraitCategory,
        key: &str,
        value: &str,
        scope: &str,
        memory_id: &str,
        strength: f64,
    ) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        // 查找是否已有匹配的 trait
        if let Some(existing) = model
            .traits
            .iter_mut()
            .find(|t| t.category == category && t.key == key)
        {
            // 更新现有 trait
            existing.value = value.to_string();
            existing.confidence = (existing.confidence + strength * 0.3).min(1.0);
            existing.importance = (existing.importance + strength * 0.1).min(1.0);
            if !existing.evidence_ids.contains(&memory_id.to_string()) {
                existing.evidence_ids.push(memory_id.to_string());
                existing.evidence_count += 1;
            }
            existing.updated_at = now;
            if !scope.is_empty() {
                existing.scope = scope.to_string();
            }
            existing.lifecycle = if existing.evidence_count >= STABILITY_MIN_EVIDENCE_COUNT {
                TraitLifecycle::Stable
            } else {
                TraitLifecycle::Active
            };
        } else {
            // 创建新 trait
            model.traits.push(UserTrait {
                category,
                key: key.to_string(),
                value: value.to_string(),
                meaning: String::new(),
                confidence: strength.min(1.0),
                stability: 0.0,
                importance: strength.min(1.0),
                scope: scope.to_string(),
                evidence_ids: vec![memory_id.to_string()],
                related_topics: Vec::new(),
                lifecycle: TraitLifecycle::Active,
                created_at: now,
                updated_at: now,
                evidence_count: 1,
                contradiction_count: 0,
            });
        }

        model.updated_at = now;
        self.save_inner();
    }

    /// 添加矛盾证据（降低对应 trait 的置信度）
    pub fn apply_contradicting_evidence(
        &self,
        key: &str,
        memory_id: &str,
        strength: f64,
    ) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        if let Some(trait_) = model.traits.iter_mut().find(|t| t.key == key) {
            trait_.confidence = (trait_.confidence - strength * 0.5).max(0.0);
            trait_.contradiction_count += 1;
            trait_.updated_at = now;
            if !trait_.evidence_ids.contains(&memory_id.to_string()) {
                trait_.evidence_ids.push(memory_id.to_string());
            }
            if trait_.contradiction_count >= 2 {
                trait_.lifecycle = TraitLifecycle::Contradicted;
            }
            model.updated_at = now;
            self.save_inner();
        }
    }

    /// 归并一个概念（由 ConsolidationPipeline Stage 3 调用）。
    ///
    /// 把 LLM 从 Insight 中归纳出的高层概念（"用户长期在乎什么"）写入用户模型：
    /// - 已存在同名概念：强化（合并 meaning / related_topics / evidence，提升 strength 与置信度）
    /// - 不存在：新建概念（category=Value，lifecycle=Active）
    ///
    /// 这是概念层"跨主题抽象"的写入入口：key 是概念名（如 agent_autonomy），
    /// related_topics 是与之关联的主题（如 proactive / inner_monologue / observation）。
    pub fn merge_concept(
        &self,
        key: &str,
        value: &str,
        meaning: &str,
        related_topics: &[String],
        evidence_ids: &[String],
        strength: f64,
    ) {
        let mut model = self.inner.write();
        model.merge_concept(
            key,
            value,
            meaning,
            related_topics,
            evidence_ids,
            strength,
        );
        model.updated_at = crate::memory::types::current_timestamp();
        drop(model);
        self.save_inner();
    }

    // ── 候选 Trait 管理 ──

    /// 添加弱证据到候选池
    pub fn add_candidate_evidence(
        &self,
        category: UserTraitCategory,
        key: &str,
        value: &str,
        memory_id: &str,
        strength: f64,
    ) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        if let Some(candidate) = model
            .candidate_traits
            .iter_mut()
            .find(|c| c.category == category && c.key == key && c.value == value)
        {
            // 更新现有候选
            candidate.confidence = (candidate.confidence + strength * 0.15).min(1.0);
            if !candidate.evidence_ids.contains(&memory_id.to_string()) {
                candidate.evidence_ids.push(memory_id.to_string());
            }
            candidate.last_observed_at = now;
            candidate.observation_count += 1;
        } else {
            // 创建新候选
            model.candidate_traits.push(CandidateTrait {
                category,
                key: key.to_string(),
                value: value.to_string(),
                confidence: strength.min(1.0),
                evidence_ids: vec![memory_id.to_string()],
                first_observed_at: now,
                last_observed_at: now,
                observation_count: 1,
            });
        }

        model.updated_at = now;
        self.save_inner();
    }

    // ── 项目激活度计算 ──

    /// 更新所有项目的激活度（基于当前话题信号）
    ///
    /// 由 FastSemantic 阶段或 Context Builder 在每次对话时调用。
    pub fn update_project_activations(&self, current_topics: &[String]) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        for project in &mut model.projects {
            let new_activation = project.calculate_activation(current_topics);
            project.activation = new_activation;

            // 如果激活度超过阈值，更新 last_active_at
            if new_activation >= PROJECT_ACTIVE_THRESHOLD {
                project.last_active_at = now;
            }

            // 如果激活度超过阈值且当前为 dormant → 自动激活
            if new_activation >= PROJECT_ACTIVE_THRESHOLD
                && project.status == ProjectStatus::Dormant
            {
                project.status = ProjectStatus::Active;
            }
        }

        model.updated_at = now;
        // 激活度是运行时状态，不持久化
    }

    /// 多跳检索锚点：根据当前话题展开关联话题。
    ///
    /// 命中特征/项目后返回其关联话题，供检索管线召回"概念相关但字面不相似"的旧记忆。
    pub fn expand_related_topics(&self, terms: &[String]) -> Vec<String> {
        self.inner.read().expand_related_topics(terms)
    }

    /// 共现式关联构建：把当前话题关联到最近更新的特征。
    ///
    /// 由对话后处理调用（拿到 FastSemantic 话题标签时），让特征逐渐积累"用户常把它和哪些话题一起讨论"。
    pub fn associate_active_traits_with_topics(&self, topics: &[String], window_secs: f64) {
        {
            let mut model = self.inner.write();
            model.associate_active_traits_with_topics(topics, window_secs);
            model.updated_at = crate::memory::types::current_timestamp();
        }
        self.save_inner();
    }

    // ── 项目管理 ──

    /// 注册或更新项目
    pub fn upsert_project(
        &self,
        name: &str,
        description: &str,
        topics: Vec<String>,
        phase: &str,
        status: ProjectStatus,
    ) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        if let Some(project) = model.projects.iter_mut().find(|p| p.name == name) {
            project.description = description.to_string();
            project.phase = phase.to_string();
            project.status = status;
            project.last_active_at = now;
            if !topics.is_empty() {
                project.topics = topics;
            }
        } else {
            model.projects.push(UserProject {
                name: name.to_string(),
                description: description.to_string(),
                status,
                phase: phase.to_string(),
                focus: Vec::new(),
                activation: 0.5,
                topics,
                related_memory_ids: Vec::new(),
                created_at: now,
                last_active_at: now,
            });
        }

        model.updated_at = now;
        self.save_inner();
    }

    /// 关联记忆到项目
    pub fn relate_memory_to_project(&self, project_name: &str, memory_id: &str) {
        let mut model = self.inner.write();
        if let Some(project) = model.projects.iter_mut().find(|p| p.name == project_name) {
            if !project.related_memory_ids.contains(&memory_id.to_string()) {
                project.related_memory_ids.push(memory_id.to_string());
            }
        }
        self.save_inner();
    }

    // ── 目标管理 ──

    /// 添加或更新目标
    pub fn upsert_goal(
        &self,
        description: &str,
        status: GoalStatus,
        importance: f64,
        memory_id: &str,
    ) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();

        if let Some(goal) = model.goals.iter_mut().find(|g| g.description == description) {
            goal.status = status;
            goal.importance = importance.max(goal.importance);
            goal.updated_at = now;
            if !goal.evidence_ids.contains(&memory_id.to_string()) {
                goal.evidence_ids.push(memory_id.to_string());
            }
        } else {
            model.goals.push(UserGoal {
                description: description.to_string(),
                status,
                importance,
                evidence_ids: vec![memory_id.to_string()],
                created_at: now,
                updated_at: now,
            });
        }

        model.updated_at = now;
        self.save_inner();
    }

    // ── 后台维护 ──

    /// 后台维护：候选 → stable 提升 / 衰减
    ///
    /// 在闲置时调用（如 Observation 后台循环）。
    pub fn maintain(&self) {
        let mut model = self.inner.write();
        let now = crate::memory::types::current_timestamp();
        let mut promoted = Vec::new();

        // 处理候选 Trait
        model.candidate_traits.retain(|c| {
            let days_since_last = (now - c.last_observed_at) / 86400.0;

            // 达到提升阈值且证据充分 → 提升为稳定 trait
            if c.confidence >= CANDIDATE_PROMOTION_THRESHOLD
                && c.observation_count >= STABILITY_MIN_EVIDENCE_COUNT
            {
                promoted.push(c.clone());
                return false; // 从候选池移除
            }

            // 长时间无证据 → 衰减
            if days_since_last > 7.0 {
                let decayed = c.confidence * CANDIDATE_DECAY_FACTOR.powf(days_since_last / 7.0);
                // 不能直接修改 retain 中的 & 引用，用 clone 判断
                decayed < 0.1
            } else {
                true
            }
        });

        // 将提升的候选加入稳定 trait
        for candidate in promoted {
            let time_span_days = (candidate.last_observed_at - candidate.first_observed_at) / 86400.0;
            let stability = if time_span_days >= STABILITY_MIN_SPAN_DAYS {
                (time_span_days / 90.0).min(1.0) * 0.5 + 0.5
            } else {
                time_span_days / STABILITY_MIN_SPAN_DAYS * 0.5
            };
            model.traits.push(UserTrait {
                category: candidate.category,
                key: candidate.key,
                value: candidate.value,
                meaning: String::new(),
                confidence: candidate.confidence,
                stability,
                importance: (candidate.confidence * 0.7 + 0.3).min(1.0),
                scope: String::new(),
                evidence_ids: candidate.evidence_ids,
                related_topics: Vec::new(),
                lifecycle: TraitLifecycle::Stable,
                created_at: candidate.first_observed_at,
                updated_at: candidate.last_observed_at,
                evidence_count: candidate.observation_count,
                contradiction_count: 0,
            });
        }

        // 处理已存在的 trait：长期无证据 → fading
        for trait_ in &mut model.traits {
            if trait_.lifecycle == TraitLifecycle::Stable
                || trait_.lifecycle == TraitLifecycle::Active
            {
                let days_since_update = (now - trait_.updated_at) / 86400.0;
                if days_since_update > 30.0 && trait_.evidence_count < 3 {
                    trait_.lifecycle = TraitLifecycle::Fading;
                }
            }
        }

        model.updated_at = now;
        self.save_inner();
    }

    // ── 持久化 ──

    fn store_path(char_id: &str) -> PathBuf {
        let mut p = path::get_character_data_dir(char_id);
        p.push(USER_MODEL_FILE);
        p
    }

    fn load_from_disk(path: &PathBuf) -> VivianResult<UserModel> {
        let data = std::fs::read_to_string(path)?;
        let model: UserModel = serde_json::from_str(&data)?;
        Ok(model)
    }

    fn save_inner(&self) {
        let model = self.inner.read();
        if let Ok(data) = serde_json::to_string_pretty(&*model) {
            if let Some(parent) = self.store_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.store_path, data);
        }
    }
}

// ============================================================================
// 强证据检测器（规则驱动，不调用 LLM）
// ============================================================================

/// 强证据检测结果
#[derive(Debug, Clone)]
pub struct DetectedEvidence {
    pub category: UserTraitCategory,
    pub key: String,
    pub value: String,
    pub scope: String,
    pub strength: f64,
    /// 是否是矛盾证据（降低置信度）
    pub is_contradiction: bool,
}

/// 从用户输入中检测强证据
///
/// 纯规则匹配，适用于：
/// - "我喜欢/讨厌/更喜欢/以后都..."
/// - "我准备/打算/目标是..."
/// - "我主要做/我是做..."
/// - "我不喜欢/不用/不会再..."
pub fn detect_strong_evidence(input: &str) -> Vec<DetectedEvidence> {
    let mut results = Vec::new();
    let input_lower = input.to_lowercase();

    // === 偏好表达 ===
    // "我喜欢X" / "我更偏好X" / "我还是喜欢X"
    if let Some(captured) = extract_after_prefix(&input_lower, &["我喜欢", "我更喜欢", "我还是喜欢", "我更偏好", "我比较喜欢"]) {
        if let Some((key, value)) = classify_preference(&captured) {
            results.push(DetectedEvidence {
                category: UserTraitCategory::Preference,
                key,
                value,
                scope: String::new(),
                strength: 0.85,
                is_contradiction: false,
            });
        }
    }

    // "我讨厌X" / "我不喜欢X" / "我不用X" / "以后都不用X"
    if let Some(captured) = extract_after_prefix(&input_lower, &["我讨厌", "我不喜欢", "我不用", "以后都不用", "我绝对不会用", "别用"]) {
        if let Some((key, value)) = classify_preference(&captured) {
            results.push(DetectedEvidence {
                category: UserTraitCategory::Preference,
                key,
                value: format!("avoid_{}", value),
                scope: String::new(),
                strength: 0.90,
                is_contradiction: false,
            });
        }
    }

    // === 工作方式表达 ===
    // "我更关心落地" / "我倾向于工程" / "我重视可实现性"
    if contains_any(&input_lower, &["更关心落地", "倾向工程", "工程落地", "可落地", "重视实现", "实用优先"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::WorkStyle,
            key: "engineering_vs_research".to_string(),
            value: "engineering".to_string(),
            scope: String::new(),
            strength: 0.80,
            is_contradiction: false,
        });
    }
    if contains_any(&input_lower, &["更偏理论", "理论研究", "做理论", "理论优先"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::WorkStyle,
            key: "engineering_vs_research".to_string(),
            value: "research".to_string(),
            scope: String::new(),
            strength: 0.80,
            is_contradiction: false,
        });
    }

    // === 目标表达 ===
    // "我准备做X" / "我的目标是X" / "我打算X"
    if let Some(captured) = extract_after_prefix(&input_lower, &["我的目标是", "我准备", "我打算", "我要把", "我希望让"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::Value,
            key: "stated_goal".to_string(),
            value: captured.clone(),
            scope: String::new(),
            strength: 0.85,
            is_contradiction: false,
        });
    }

    // === 兴趣领域 ===
    if let Some(captured) = extract_after_prefix(&input_lower, &["我在学", "我在研究", "我主要做", "我是做", "我的方向是", "我搞"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::Interest,
            key: "field".to_string(),
            value: captured.clone(),
            scope: String::new(),
            strength: 0.75,
            is_contradiction: false,
        });
    }

    // === 明确的反驳（矛盾证据） ===
    if contains_any(&input_lower, &["其实不是", "其实不对", "我不是那个意思", "你理解错了", "你搞错了", "不是这样"]) {
        // 矛盾证据不需要具体 key/value，交由上层处理
        results.push(DetectedEvidence {
            category: UserTraitCategory::Value, // 占位
            key: "contradiction_signal".to_string(),
            value: "user_disagreed".to_string(),
            scope: String::new(),
            strength: 0.80,
            is_contradiction: true,
        });
    }

    results
}

// ============================================================================
// 弱证据检测器（可选，为后台归纳提供线索）
// ============================================================================

/// 从用户输入中检测弱证据
///
/// 弱证据不直接更新 trait，而是进入候选池。
/// 适用于含蓄表达、行为暗示等。
pub fn detect_weak_evidence(input: &str) -> Vec<DetectedEvidence> {
    let mut results = Vec::new();
    let input_lower = input.to_lowercase();

    // "X有点烦" / "X不太好用" → 弱偏好
    if let Some(captured) = extract_after_prefix(&input_lower, &["有点烦", "不太好用", "不太好", "有点麻烦", "不太喜欢"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::Preference,
            key: "vague_dislike".to_string(),
            value: captured.trim().to_string(),
            scope: String::new(),
            strength: 0.35,
            is_contradiction: false,
        });
    }

    // 连续两次提到某种技术/框架 → 强信号
    // （由调用方管理，不在本函数内处理）

    // "X还不错" / "X挺好的" → 弱偏好
    if let Some(captured) = extract_after_prefix(&input_lower, &["还不错", "挺好的", "太好用了", "很好用"]) {
        results.push(DetectedEvidence {
            category: UserTraitCategory::Preference,
            key: "vague_like".to_string(),
            value: captured.trim().to_string(),
            scope: String::new(),
            strength: 0.30,
            is_contradiction: false,
        });
    }

    results
}

// ============================================================================
// 工具函数
// ============================================================================

/// 在输入中查找指定前缀并返回后面的内容
fn extract_after_prefix<'a>(input: &'a str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(idx) = input.find(prefix) {
            let after = input[idx + prefix.len()..].trim();
            if !after.is_empty() {
                // 截取到句号、逗号、感叹号、问号、分号等标点
                let end = after
                    .find(|c: char| matches!(c, '。' | '，' | '！' | '？' | '；' | '、' | '.' | ',' | '!' | '?' | ';'))
                    .unwrap_or(after.len());
                let captured = after[..end].trim();
                if !captured.is_empty() && captured.len() <= 50 {
                    return Some(captured.to_string());
                }
            }
        }
    }
    None
}

/// 检查输入是否包含任一关键词
fn contains_any(input: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|k| input.contains(k))
}

/// 对捕获的偏好内容进行分类
fn classify_preference(captured: &str) -> Option<(String, String)> {
    // 尝试从常见模式中提取 key-value
    let tech_keywords = [
        "tailwind", "vue", "react", "rust", "python", "typescript", "javascript",
        "css", "html", "docker", "kubernetes", "postgresql", "mysql", "redis",
        "linux", "macos", "windows", "vim", "vscode", "neovim", "git",
        "tailwindcss", "bootstrap", "mui", "shadcn", "nextjs", "nuxt",
        "tauri", "electron", "langchain", "pytorch", "tensorflow",
    ];

    let lower = captured.to_lowercase();
    for tech in &tech_keywords {
        if lower.contains(tech) {
            return Some(("technology".to_string(), tech.to_string()));
        }
    }

    // 设计相关
    let design_keywords = [
        "赛博朋克", "极简", "扁平", "拟物", "毛玻璃", "霓虹", "暗色", "浅色",
        "cyberpunk", "minimal", "flat", "neumorphic", "glass", "neon", "dark", "light",
        "手绘风", "卡通", "写实", "像素",
    ];
    for design in &design_keywords {
        if lower.contains(design) {
            return Some(("style".to_string(), design.to_string()));
        }
    }

    // 无法分类时返回通用键
    Some(("general".to_string(), captured.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::current_timestamp;

    #[test]
    fn test_detect_strong_preference_like() {
        let results = detect_strong_evidence("我喜欢用 Rust 写后端");
        assert!(!results.is_empty());
        let r = &results[0];
        assert_eq!(r.category, UserTraitCategory::Preference);
        assert_eq!(r.strength, 0.85);
        assert!(!r.is_contradiction);
    }

    #[test]
    fn test_detect_strong_preference_dislike() {
        let results = detect_strong_evidence("我以后都不用 Tailwind 了");
        assert!(!results.is_empty());
        let r = &results[0];
        assert_eq!(r.key, "preference");
        assert!(r.value.contains("tailwind"));
        assert_eq!(r.strength, 0.90);
    }

    #[test]
    fn test_detect_engineering_style() {
        let results = detect_strong_evidence("我更关心工程落地");
        assert!(!results.is_empty());
        let r = &results[0];
        assert_eq!(r.key, "engineering_vs_research");
        assert_eq!(r.value, "engineering");
    }

    #[test]
    fn test_detect_goal() {
        let results = detect_strong_evidence("我的目标是打造一个 Virtual Being");
        assert!(!results.is_empty());
        let r = &results[0];
        assert_eq!(r.key, "stated_goal");
        assert!(r.value.contains("virtual") || r.value.contains("Virtual"));
    }

    #[test]
    fn test_detect_weak_evidence() {
        let results = detect_weak_evidence("Tailwind 写起来有点烦");
        assert!(!results.is_empty());
        assert_eq!(results[0].strength, 0.35);
    }

    #[test]
    fn test_project_activation_calculation() {
        let project = UserProject {
            name: "Vivian".to_string(),
            description: "Desktop Pet Agent".to_string(),
            status: ProjectStatus::Active,
            phase: "cognitive_architecture".to_string(),
            focus: vec!["memory".to_string(), "web_search".to_string()],
            activation: 0.8,
            topics: vec!["memory".to_string(), "agent".to_string(), "vivian".to_string()],
            related_memory_ids: Vec::new(),
            created_at: 0.0,
            last_active_at: crate::memory::types::current_timestamp(),
        };

        let activation = project.calculate_activation(&["memory".to_string(), "web_search".to_string()]);
        assert!(activation > 0.5);

        let activation_no_match = project.calculate_activation(&["cooking".to_string()]);
        assert!(activation_no_match < activation);
    }

    #[test]
    fn test_format_for_prompt_empty() {
        let model = UserModel::empty();
        assert!(model.is_empty());
        assert!(model.format_for_prompt("zh").is_none());
    }

    #[test]
    fn test_candidate_promotion() {
        use crate::memory::types::current_timestamp;
        let now = current_timestamp();

        let mut candidate = CandidateTrait {
            category: UserTraitCategory::Preference,
            key: "test".to_string(),
            value: "value".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["mem1".to_string(), "mem2".to_string(), "mem3".to_string()],
            first_observed_at: now - 86400.0 * 20.0, // 20 days ago
            last_observed_at: now,
            observation_count: 3,
        };

        let qualifies = candidate.confidence >= CANDIDATE_PROMOTION_THRESHOLD
            && candidate.observation_count >= STABILITY_MIN_EVIDENCE_COUNT;
        assert!(qualifies);
    }

    #[test]
    fn test_expand_related_topics_via_trait() {
        use crate::memory::types::current_timestamp;
        let now = current_timestamp();
        let mut model = UserModel::empty();
        model.traits.push(UserTrait {
            category: UserTraitCategory::Value,
            key: "agent_autonomy".to_string(),
            value: "high".to_string(),
            meaning: String::new(),
            confidence: 0.9,
            stability: 0.8,
            importance: 0.9,
            scope: "global".to_string(),
            evidence_ids: Vec::new(),
            related_topics: vec![
                "proactive".to_string(),
                "inner_monologue".to_string(),
                "web_search".to_string(),
            ],
            lifecycle: TraitLifecycle::Stable,
            created_at: now,
            updated_at: now,
            evidence_count: 5,
            contradiction_count: 0,
        });

        // 命中特征（用户说"机械"，关联到 agent_autonomy 的 related_topics）
        let expanded = model.expand_related_topics(&["机械".to_string(), "proactive".to_string()]);
        assert_eq!(expanded.len(), 3);
        assert!(expanded.contains(&"proactive".to_string()));
        assert!(expanded.contains(&"web_search".to_string()));
    }

    #[test]
    fn test_expand_related_topics_no_match_returns_empty() {
        let model = UserModel::empty();
        let expanded = model.expand_related_topics(&["机械".to_string()]);
        assert!(expanded.is_empty());
    }

    #[test]
    fn test_associate_active_traits_with_topics() {
        use crate::memory::types::current_timestamp;
        let now = current_timestamp();
        let mut model = UserModel::empty();
        model.traits.push(UserTrait {
            category: UserTraitCategory::Value,
            key: "agent_autonomy".to_string(),
            value: "high".to_string(),
            meaning: String::new(),
            confidence: 0.9,
            stability: 0.8,
            importance: 0.9,
            scope: "global".to_string(),
            evidence_ids: Vec::new(),
            related_topics: Vec::new(),
            lifecycle: TraitLifecycle::Stable,
            created_at: now,
            updated_at: now, // 最近更新，应被关联
            evidence_count: 5,
            contradiction_count: 0,
        });

        model.associate_active_traits_with_topics(
            &["proactive".to_string(), "inner_monologue".to_string()],
            600.0,
        );
        let tr = &model.traits[0];
        assert_eq!(tr.related_topics.len(), 2);
        assert!(tr.related_topics.contains(&"proactive".to_string()));
        // 不重复添加
        model.associate_active_traits_with_topics(&["proactive".to_string()], 600.0);
        assert_eq!(model.traits[0].related_topics.len(), 2);
    }

    #[test]
    fn test_associate_skips_stale_trait() {
        use crate::memory::types::current_timestamp;
        let now = current_timestamp();
        let mut model = UserModel::empty();
        model.traits.push(UserTrait {
            category: UserTraitCategory::Preference,
            key: "old".to_string(),
            value: "x".to_string(),
            meaning: String::new(),
            confidence: 0.5,
            stability: 0.5,
            importance: 0.5,
            scope: String::new(),
            evidence_ids: Vec::new(),
            related_topics: Vec::new(),
            lifecycle: TraitLifecycle::Stable,
            created_at: now - 1000000.0,
            updated_at: now - 1000000.0, // 很久没更新，不应被关联
            evidence_count: 5,
            contradiction_count: 0,
        });

        model.associate_active_traits_with_topics(&["proactive".to_string()], 600.0);
        assert!(model.traits[0].related_topics.is_empty());
    }

    #[test]
    fn test_merge_concept_creates_new() {
        let now = current_timestamp();
        let mut model = UserModel::empty();

        model.merge_concept(
            "agent_autonomy",
            "high",
            "用户希望智能体具备较强自主性",
            &["proactive".to_string(), "inner_monologue".to_string()],
            &["mem1".to_string(), "mem2".to_string()],
            0.8,
        );

        assert_eq!(model.traits.len(), 1);
        let t = &model.traits[0];
        assert_eq!(t.key, "agent_autonomy");
        assert_eq!(t.meaning, "用户希望智能体具备较强自主性");
        assert_eq!(t.related_topics.len(), 2);
        assert_eq!(t.evidence_count, 2);
        assert!((t.confidence - 0.8).abs() < 1e-6);
        assert_eq!(t.created_at, now); // 新建
    }

    #[test]
    fn test_merge_concept_reinforces_existing() {
        let now = current_timestamp();
        let mut model = UserModel::empty();

        // 先建一个概念
        model.merge_concept(
            "agent_autonomy",
            "high",
            "用户希望智能体具备较强自主性",
            &["proactive".to_string()],
            &["mem1".to_string()],
            0.8,
        );
        // 再次归并：同名强化 + 追加 related_topics + evidence + strength 上浮
        model.merge_concept(
            "agent_autonomy",
            "high",
            "用户希望智能体具备较强自主性",
            &["proactive".to_string(), "observation".to_string()],
            &["mem3".to_string()],
            0.8,
        );

        assert_eq!(model.traits.len(), 1); // 不重复新建
        let t = &model.traits[0];
        assert_eq!(t.related_topics.len(), 2); // proactive 去重 + observation 追加
        assert_eq!(t.evidence_count, 2); // mem1 + mem3
        assert!(t.confidence > 0.8); // 强化上浮
        assert!(t.confidence <= 0.95);
        assert_eq!(t.created_at, now); // created_at 保留首次
    }
}