//! 记忆保留策略：基于规则的过期清理 + 内容去重 + 证据驱动归档。
//!
//! 原文件名为 `consolidation.rs`，但实际逻辑只做过期清理与字面去重，
//! 没有"短期→长期"的真正巩固。为消除概念误导、为后续 `pipeline.rs`
//! 中真正的巩固流水线让路，重命名为 `retention.rs`。
//!
//! - `MemoryExpirationRule` / `MemoryRetentionPolicy`：策略数据结构
//! - 三条默认规则：闲聊 24h/100 条、临时上下文 6h/50 条、长期 720h 且 importance<0.3
//! - `should_keep`：importance<0.3 且 >24h 删；importance<0.5 且 >72h 删
//! - `MemoryRetentionGuard`（原 `MemoryConsolidator`）：清理 + 字面去重
//!
//! ## 证据驱动归档
//!
//! 保留决策综合考虑证据评分（`evidence_score`）：
//! - `protected=true` 的记忆永不归档（evidence_score 返回 +∞）
//! - evidence_score <= ARCHIVE_THRESHOLD (-2.0) 且 sub_zero_days >= 14 天 → 归档
//! - 去重合并时优先保留 evidence_score 更高的条目（而非仅看 importance）

use std::collections::{HashMap, HashSet};

use crate::error::VivianResult;

use super::evidence::{evidence_score, ARCHIVE_THRESHOLD};
use super::manager::MemoryManager;
use super::types::{current_timestamp, MemoryItem};

/// 记忆过期规则。
#[derive(Debug, Clone)]
pub struct MemoryExpirationRule {
    /// 记忆类型（与 `MemoryType::as_str` 对应）
    pub memory_type: String,
    /// 最大存活小时数
    pub max_age_hours: f64,
    /// 最大数量限制（超过则按淘汰策略删除）
    pub max_count: Option<usize>,
    /// 最低重要度阈值（低于此值才可被规则删除）
    pub min_importance: f64,
    /// 淘汰策略：true 按 evidence_score+importance 升序（证据弱、重要性低优先淘汰），
    /// false 按 timestamp 升序（最旧优先淘汰，默认）
    pub evict_by_score: bool,
}

impl MemoryExpirationRule {
    pub fn new(memory_type: impl Into<String>, max_age_hours: f64) -> Self {
        Self {
            memory_type: memory_type.into(),
            max_age_hours,
            max_count: None,
            min_importance: 0.0,
            evict_by_score: false,
        }
    }

    pub fn with_max_count(mut self, max_count: usize) -> Self {
        self.max_count = Some(max_count);
        self
    }

    pub fn with_min_importance(mut self, min_importance: f64) -> Self {
        self.min_importance = min_importance;
        self
    }

    /// 启用按证据+重要性评分淘汰（用于 Knowledge/Insight 等需要按价值淘汰的类型）
    pub fn with_evict_by_score(mut self) -> Self {
        self.evict_by_score = true;
        self
    }
}

/// 记忆保留策略。
pub struct MemoryRetentionPolicy;

impl MemoryRetentionPolicy {
    /// 永不删除的内容
    pub fn keep_always() -> HashSet<&'static str> {
        let mut s = HashSet::new();
        s.insert("preference");
        s.insert("identity");
        s.insert("important_event");
        s.insert("user_preferences");
        s.insert("user_identity");
        s.insert("important_events");
        s
    }

    /// 可以删除的临时内容
    pub fn can_delete() -> HashSet<&'static str> {
        let mut s = HashSet::new();
        s.insert("casual_conversation");
        s.insert("temporary_context");
        s.insert("old_sessions");
        s
    }

    /// 判断是否应该保留此记忆。
    ///
    /// 策略：
    /// - KEEP_ALWAYS 类型永远保留
    /// - CAN_DELETE 类型：importance<0.3 且 >24h 删；importance<0.5 且 >72h 删
    /// - 其他类型默认保留
    pub fn should_keep(memory_type: &str, importance: f64, age_hours: f64) -> bool {
        if Self::keep_always().contains(memory_type) {
            return true;
        }
        if Self::can_delete().contains(memory_type) {
            if importance < 0.3 && age_hours > 24.0 {
                return false;
            }
            if importance < 0.5 && age_hours > 72.0 {
                return false;
            }
        }
        true
    }
}

/// 二次函数衰减遗忘
///
/// 记忆强度随时间呈二次函数衰减：
/// strength(t) = max(0, 1 - (t / T)^2)
///
/// 其中 T = 30 天（衰减常数）。当 strength 衰减到 0.01 以下时自动删除。
/// 高 importance 的记忆衰减更慢（按 importance 反向缩放时间）。
///
/// 衰减特性：
/// - 0 天：strength = 1.0
/// - 15 天（T/2）：strength = 0.75
/// - 30 天（T）：strength = 0.0 → 删除
///
/// protected 记忆与 KEEP_ALWAYS 类型不参与衰减。
pub struct QuadraticDecay;

/// 衰减常数：30 天（单位：秒）
const DECAY_CONSTANT_SECONDS: f64 = 30.0 * 24.0 * 3600.0;
/// 衰减阈值：strength 低于此值时删除
const DECAY_PURGE_THRESHOLD: f64 = 0.01;
/// importance 对衰减的缩放系数（importance=1 时衰减时长翻倍）
const IMPORTANCE_DECAY_SCALE: f64 = 1.0;

impl QuadraticDecay {
    /// 计算记忆的当前衰减强度 [0, 1]
    pub fn strength(memory: &MemoryItem, now: f64) -> f64 {
        let age = (now - memory.timestamp).max(0.0);
        if age <= 0.0 {
            return 1.0;
        }
        // importance 越高，有效衰减常数越长
        let effective_t = DECAY_CONSTANT_SECONDS * (1.0 + memory.importance * IMPORTANCE_DECAY_SCALE);
        let ratio = age / effective_t;
        (1.0 - ratio * ratio).max(0.0)
    }

    /// 判断记忆是否应被衰减删除
    pub fn should_purge(memory: &MemoryItem, now: f64) -> bool {
        // protected 与 KEEP_ALWAYS 类型不参与衰减
        if memory.protected {
            return false;
        }
        let mem_type = extract_memory_type(memory);
        if MemoryRetentionPolicy::keep_always().contains(mem_type.as_str()) {
            return false;
        }
        // 高重要度记忆（>=0.8）不参与衰减删除
        if memory.importance >= 0.8 {
            return false;
        }
        Self::strength(memory, now) < DECAY_PURGE_THRESHOLD
    }

    /// 计算衰减后的有效重要性（用于检索时降权）
    pub fn decayed_importance(memory: &MemoryItem, now: f64) -> f64 {
        let s = Self::strength(memory, now);
        memory.importance * s
    }
}

/// 从记忆条目中提取类型字符串（兼容 tags 与 metadata 两种来源）。
fn extract_memory_type(memory: &MemoryItem) -> String {
    // 1. 优先看 tags 中是否含有已知的 memory_type 关键字
    let known: &[&str] = &[
        "preference",
        "identity",
        "important_event",
        "knowledge",
        "temporary_context",
        "casual_conversation",
        "long_term",
        "short_term",
        "mid_term",
        "user",
        "feedback",
        "project",
        "reference",
        "general",
    ];
    for t in &memory.tags {
        let lower = t.to_lowercase();
        if known.contains(&lower.as_str()) {
            return lower;
        }
    }
    // 2. 看 metadata.memory_type
    if let Some(s) = memory.metadata.get("memory_type").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    // 3. 默认归为 general
    "general".to_string()
}

/// 记忆保留守卫：过期清理 + 字面去重。
///
/// 原名 `MemoryConsolidator`，因实际逻辑只做保留/清理、不涉及真正的
/// "短期→长期"巩固，重命名为 `MemoryRetentionGuard` 以消除概念混淆。
/// 真正的巩固流水线见 `pipeline.rs`。
pub struct MemoryRetentionGuard {
    /// 过期规则列表
    rules: Vec<MemoryExpirationRule>,
}

impl MemoryRetentionGuard {
    pub fn new() -> Self {
        Self {
            rules: Self::default_rules(),
        }
    }

    /// 默认规则：
    /// - casual_conversation: 24h / 100 条（最旧优先淘汰）
    /// - temporary_context: 6h / 50 条（最旧优先淘汰）
    /// - long_term: 720h 且 importance<0.3
    /// - knowledge: 永不过期 / 500 条（证据弱+重要性低优先淘汰，TTL 已处理短期过期）
    /// - insight: 永不过期 / 100 条（证据弱+重要性低优先淘汰）
    /// - inner_monologue: 168h(7天) / 200 条（最旧优先淘汰，内心独白保留近期即可）
    pub fn default_rules() -> Vec<MemoryExpirationRule> {
        vec![
            MemoryExpirationRule::new("casual_conversation", 24.0).with_max_count(100),
            MemoryExpirationRule::new("temporary_context", 6.0).with_max_count(50),
            MemoryExpirationRule::new("long_term", 720.0).with_min_importance(0.3),
            // Knowledge：TTL 分级已处理短期过期，此处仅控制长期累积上限
            MemoryExpirationRule::new("knowledge", f64::INFINITY)
                .with_max_count(500)
                .with_evict_by_score(),
            // Insight：高层洞察，按价值淘汰
            MemoryExpirationRule::new("insight", f64::INFINITY)
                .with_max_count(100)
                .with_evict_by_score(),
            // InnerMonologue：内心独白，7 天后过期，最多 200 条
            // min_importance=1.0 确保所有 inner_monologue（importance 通常 0.4）7 天后都过期
            MemoryExpirationRule::new("inner_monologue", 168.0)
                .with_max_count(200)
                .with_min_importance(1.0),
        ]
    }

    /// 获取规则列表引用（用于测试/查看）
    pub fn rules(&self) -> &[MemoryExpirationRule] {
        &self.rules
    }

    /// 判断单条记忆是否过期。
    ///
    /// 综合考虑三个维度：
    /// 1. 证据系统：`protected` 永不归档；evidence_score 触发归档倒计时
    /// 2. 保留策略：KEEP_ALWAYS / CAN_DELETE 类型规则
    /// 3. 过期规则：max_age_hours + min_importance 阈值
    pub fn is_expired(memory: &MemoryItem, rules: &[MemoryExpirationRule]) -> bool {
        let now = current_timestamp();

        // protected 记忆永不归档（证据系统核心约束）
        if memory.protected {
            return false;
        }

        let age_hours = ((now - memory.timestamp).max(0.0)) / 3600.0;
        let mem_type = extract_memory_type(memory);

        // 证据系统归档倒计时：score <= ARCHIVE_THRESHOLD 时累积 sub_zero_days
        // 达 ARCHIVE_DAYS (14) 天 → 真正归档
        let score = evidence_score(memory, now);
        if score <= ARCHIVE_THRESHOLD && memory.sub_zero_days >= super::evidence::ARCHIVE_DAYS {
            return true;
        }

        // 保留策略优先
        if !MemoryRetentionPolicy::should_keep(&mem_type, memory.importance, age_hours) {
            return true;
        }

        // 规则匹配（支持 "*" 通配）
        for rule in rules {
            if rule.memory_type == mem_type || rule.memory_type == "*" {
                if age_hours > rule.max_age_hours && memory.importance < rule.min_importance {
                    return true;
                }
            }
        }

        // 二次函数衰减遗忘：30 天衰减到 0.01 自动删除
        if QuadraticDecay::should_purge(memory, now) {
            return true;
        }

        false
    }

    /// 清理过期记忆，返回删除数量。
    ///
    /// `enabled`: 是否启用过期清理。来自 `config.memory.enable_expiration`，
    /// 关闭时直接返回 0，跳过所有清理逻辑（用户可在设置面板关闭）。
    pub async fn cleanup_expired(
        &self,
        manager: &MemoryManager,
        enabled: bool,
    ) -> VivianResult<usize> {
        if !enabled {
            return Ok(0);
        }
        let memories = manager.get_all_memories().await?;
        let mut deleted = 0usize;

        for memory in &memories {
            if Self::is_expired(memory, &self.rules) {
                manager.hard_delete_memory(&memory.id).await?;
                deleted += 1;
            }
        }

        // 处理 max_count 限制：按类型聚合，超过上限则按淘汰策略删除
        let mut by_type: HashMap<String, Vec<&MemoryItem>> = HashMap::new();
        for memory in &memories {
            let mem_type = extract_memory_type(memory);
            by_type.entry(mem_type).or_default().push(memory);
        }
        let now = current_timestamp();
        for rule in &self.rules {
            if let Some(max_count) = rule.max_count {
                if let Some(items) = by_type.get(&rule.memory_type) {
                    if items.len() > max_count {
                        let mut sorted: Vec<&MemoryItem> = items.clone();
                        if rule.evict_by_score {
                            // 按证据+重要性升序（证据弱、重要性低优先淘汰）
                            // protected 记忆 evidence_score=+∞，自然排到最后不会被淘汰
                            sorted.sort_by(|a, b| {
                                let sa = evidence_score(a, now) + a.importance;
                                let sb = evidence_score(b, now) + b.importance;
                                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        } else {
                            // 按时间戳升序（最旧优先）
                            sorted.sort_by(|a, b| {
                                a.timestamp
                                    .partial_cmp(&b.timestamp)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                        let to_remove = sorted.len() - max_count;
                        for memory in sorted.into_iter().take(to_remove) {
                            manager.hard_delete_memory(&memory.id).await?;
                            deleted += 1;
                        }
                    }
                }
            }
        }

        Ok(deleted)
    }

    /// 内容去重合并：对内容相同（不区分大小写/空白）的记忆，保留证据评分最高的一条。
    ///
    /// 综合评分 = evidence_score + importance * 0.1。
    /// protected 记忆永远胜出（evidence_score 返回 +∞）。
    ///
    /// 返回被删除的重复记忆数量（0 表示无重复）。
    pub async fn consolidate(&self, manager: &MemoryManager) -> VivianResult<usize> {
        let memories = manager.get_all_memories().await?;
        let now = current_timestamp();

        let mut best: HashMap<String, (String, f64)> = HashMap::new();
        let mut to_delete: Vec<String> = Vec::new();

        for memory in &memories {
            let key = memory.content.trim().to_lowercase();
            // 综合评分：evidence_score 为主，importance 为辅（避免证据为 0 时无法区分）
            let combined = evidence_score(memory, now) + memory.importance * 0.1;
            match best.get(&key) {
                None => {
                    best.insert(key, (memory.id.clone(), combined));
                }
                Some((_, existing_score)) => {
                    if combined > *existing_score {
                        if let Some((old_id, _)) =
                            best.insert(key, (memory.id.clone(), combined))
                        {
                            to_delete.push(old_id);
                        }
                    } else {
                        to_delete.push(memory.id.clone());
                    }
                }
            }
        }

        let deleted_count = to_delete.len();
        for id in to_delete {
            manager.hard_delete_memory(&id).await?;
        }

        Ok(deleted_count)
    }
}

impl Default for MemoryRetentionGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_keep_always() {
        // 永不删除的类型
        assert!(MemoryRetentionPolicy::should_keep("preference", 0.1, 1000.0));
        assert!(MemoryRetentionPolicy::should_keep("identity", 0.1, 1000.0));
        assert!(MemoryRetentionPolicy::should_keep("important_event", 0.1, 1000.0));
        assert!(MemoryRetentionPolicy::should_keep(
            "user_preferences",
            0.1,
            1000.0
        ));
    }

    #[test]
    fn test_should_keep_can_delete() {
        // importance<0.3 且 >24h → 删
        assert!(!MemoryRetentionPolicy::should_keep(
            "casual_conversation",
            0.2,
            25.0
        ));
        // importance<0.5 且 >72h → 删
        assert!(!MemoryRetentionPolicy::should_keep(
            "temporary_context",
            0.4,
            73.0
        ));
        // 高重要度，保留
        assert!(MemoryRetentionPolicy::should_keep(
            "casual_conversation",
            0.6,
            100.0
        ));
    }

    #[test]
    fn test_default_rules() {
        let c = MemoryRetentionGuard::new();
        assert_eq!(c.rules().len(), 6);
        assert_eq!(c.rules()[0].memory_type, "casual_conversation");
        assert_eq!(c.rules()[1].memory_type, "temporary_context");
        assert_eq!(c.rules()[2].memory_type, "long_term");
        assert_eq!(c.rules()[3].memory_type, "knowledge");
        assert_eq!(c.rules()[4].memory_type, "insight");
        assert_eq!(c.rules()[5].memory_type, "inner_monologue");
        assert_eq!(c.rules()[0].max_age_hours, 24.0);
        assert_eq!(c.rules()[1].max_age_hours, 6.0);
        assert_eq!(c.rules()[2].max_age_hours, 720.0);
        assert_eq!(c.rules()[0].max_count, Some(100));
        assert_eq!(c.rules()[1].max_count, Some(50));
        assert_eq!(c.rules()[2].min_importance, 0.3);
        assert_eq!(c.rules()[3].max_count, Some(500));
        assert!(c.rules()[3].evict_by_score);
        assert_eq!(c.rules()[4].max_count, Some(100));
        assert!(c.rules()[4].evict_by_score);
        assert_eq!(c.rules()[5].max_count, Some(200));
        assert_eq!(c.rules()[5].max_age_hours, 168.0);
        assert_eq!(c.rules()[5].min_importance, 1.0);
    }

    // ── 证据驱动归档测试 ──

    fn make_evidence_memory(
        content: &str,
        reinforcement: f64,
        disputation: f64,
        protected: bool,
        sub_zero_days: u32,
    ) -> MemoryItem {
        let mut m = MemoryItem::new(content.to_string(), crate::memory::types::Granularity::Summary, 0.5);
        m.reinforcement = reinforcement;
        m.disputation = disputation;
        m.protected = protected;
        m.sub_zero_days = sub_zero_days;
        m
    }

    #[test]
    fn test_protected_memory_never_expires() {
        let rules = MemoryRetentionGuard::default_rules();
        // protected=true 的记忆即使 evidence_score 极低也不应过期
        let m = make_evidence_memory("受保护的事实", 0.0, 100.0, true, 100);
        assert!(!MemoryRetentionGuard::is_expired(&m, &rules));
    }

    #[test]
    fn test_evidence_archive_threshold_triggers_expiry() {
        let rules = MemoryRetentionGuard::default_rules();
        // evidence_score = 0 - 10 = -10 <= ARCHIVE_THRESHOLD (-2.0)
        // sub_zero_days = 14 >= ARCHIVE_DAYS (14) → 归档
        let m = make_evidence_memory("被反驳的事实", 0.0, 10.0, false, 14);
        assert!(MemoryRetentionGuard::is_expired(&m, &rules));
    }

    #[test]
    fn test_evidence_below_threshold_but_not_enough_days() {
        let rules = MemoryRetentionGuard::default_rules();
        // evidence_score <= ARCHIVE_THRESHOLD 但 sub_zero_days 不足 → 不归档
        let m = make_evidence_memory("被反驳的事实", 0.0, 10.0, false, 5);
        assert!(!MemoryRetentionGuard::is_expired(&m, &rules));
    }

    #[test]
    fn test_positive_evidence_not_expired() {
        let rules = MemoryRetentionGuard::default_rules();
        // evidence_score = 5 - 0 = 5 > 0 → 不归档
        let m = make_evidence_memory("被强化的事实", 5.0, 0.0, false, 0);
        assert!(!MemoryRetentionGuard::is_expired(&m, &rules));
    }
}
