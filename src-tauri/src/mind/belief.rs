//! Belief —— 角色的长期信念（Knowledge 层）。
//!
//! Belief 是 Memory 的压缩结论，不是 Memory 本身。Memory 是证据链（Evidence），
//! Belief 是从证据中提炼出的世界观陈述（Knowledge）。
//!
//! 关键约束：每条 Belief 必须可溯源到至少一条 Memory（source_memory_ids 不可空），
//! 否则视为幻觉。Reflection 生成 Belief 时必须传入支撑记忆 ID。
//!
//! 合并机制复用 PersonaCard 的 reinforce_insight 模式：
//! 新 Belief 生成前先查 source_memory_ids 交集 ≥ 2 的既有 Belief，命中则合并
//! （reinforcement_count 递增，confidence 取加权平均），不命中则新建。

use std::collections::HashSet;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 信念类别 —— 用于检索分组与衰减策略
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BeliefCategory {
    /// 特质："主人容易因为工作焦虑" —— 长期稳定，几乎不衰减
    Trait,
    /// 习惯："主人晚上更愿意聊天" —— 行为模式，低衰减
    Habit,
    /// 偏好："主人不喜欢被催睡觉" —— 除非有反证，否则稳定
    Preference,
    /// 当前状态："主人最近压力很大" —— 会过期，高衰减
    State,
    /// 关系认知："Agent B 越来越依赖 A" —— 由跨角色交互产生
    Relationship,
}

impl Default for BeliefCategory {
    fn default() -> Self {
        BeliefCategory::State
    }
}

/// 信念状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BeliefStatus {
    /// 稳定：未被新证据动摇
    Stable,
    /// 质疑中：检测到冲突，等待用户回应或 EMA 修正
    Questioning,
    /// 已被新信念取代
    Superseded,
}

impl Default for BeliefStatus {
    fn default() -> Self {
        BeliefStatus::Stable
    }
}

/// 度量类型 —— 决定冲突检测与 EMA 修正的算法
///
/// 同一个数值 value 在不同度量类型下含义完全不同：
/// - Duration（时长）：value=7.4 表示 7.4 小时，新值从 `duration_secs` 派生
/// - TimeOfDay（时点）：value=19.5 表示 19:30，新值从 `started_at` 提取本地小时数
/// - Count（频次）：value=3.0 表示每日 3 次，需跨日累计统计，单条事件不能直接得出
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// 时长类（小时/分钟/秒）：新值 = entry.duration_hours()
    Duration,
    /// 时点类（一天内的小时数 0-24）：新值 = entry.started_at 的本地小时
    TimeOfDay,
    /// 频次类（次数/天）：单条事件不能直接得出，detect_conflict 会跳过
    Count,
}

impl Default for MetricKind {
    fn default() -> Self {
        MetricKind::Duration
    }
}

/// 根据 metric 名称推断度量类型
///
/// 命名约定：
/// - `_hours` / `_minutes` / `_duration` / `_secs` / `_seconds` → Duration
/// - `_hour` / `_time` / `_time_of_day` → TimeOfDay
/// - `_count` / `_frequency` / `_times_per_day` → Count
/// - 其他（含 None）→ Duration（保持向后兼容）
pub fn classify_metric(metric: &str) -> MetricKind {
    let m = metric.to_lowercase();
    if m.ends_with("_hours")
        || m.ends_with("_minutes")
        || m.ends_with("_duration")
        || m.ends_with("_secs")
        || m.ends_with("_seconds")
    {
        MetricKind::Duration
    } else if m.ends_with("_hour") || m.ends_with("_time") || m.ends_with("_time_of_day") {
        MetricKind::TimeOfDay
    } else if m.ends_with("_count")
        || m.ends_with("_frequency")
        || m.ends_with("_times_per_day")
    {
        MetricKind::Count
    } else {
        MetricKind::Duration
    }
}

/// 循环距离（用于时点类度量的偏差计算）
///
/// 例如 dinner_hour=19 与新值 23，circular_distance=4
/// dinner_hour=19 与新值 2（次日凌晨），circular_distance=5（而非 17）
pub fn circular_distance(a: f64, b: f64, period: f64) -> f64 {
    let diff = (a - b).abs() % period;
    diff.min(period - diff)
}

/// 循环 EMA 修正（用于时点类度量）
///
/// 先把新值调整到旧值附近 ±period/2 范围内，再做线性 EMA，
/// 最后用 rem_euclid 取模回 [0, period) 区间。
/// 例如 dinner_hour=19，今天凌晨 2 点吃：adjusted_diff=+5（而非 -17），EMA 朝 20 漂移。
pub fn ema_circular(old: f64, new: f64, alpha: f64, period: f64) -> f64 {
    let diff = new - old;
    let half = period / 2.0;
    let adjusted_diff = ((diff + half).rem_euclid(period)) - half;
    let new_adjusted = old + adjusted_diff;
    (alpha * old + (1.0 - alpha) * new_adjusted).rem_euclid(period)
}

/// 单条信念
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    /// 唯一 ID（uuid 或 "belief_<timestamp>_<short_hash>"）
    pub id: String,
    /// 自然语言陈述，如"主人最近学习压力越来越大"
    pub statement: String,
    /// 主体：关于谁的信念
    /// "user" / "self" / 角色ID（如 "nana"）/ "world"
    pub subject: String,
    /// 类别
    #[serde(default)]
    pub category: BeliefCategory,
    /// 置信度 0.0-1.0，由支撑证据数量和强度决定
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// 溯源：支撑这条信念的记忆 ID 列表（不可空）
    pub source_memory_ids: Vec<String>,
    /// 溯源：支撑这条信念的 Episode ID 列表（从 source_memory_ids 的 episode_id 聚合）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_episode_ids: Vec<String>,
    /// 产生时间（Unix 秒）
    pub created_at: i64,
    /// 最近强化时间（Unix 秒）
    pub last_reinforced_at: i64,
    /// 被反思强化的次数（用于衰减/合并判断）
    #[serde(default)]
    pub reinforcement_count: u32,
    /// 矛盾计数：被新证据冲击的次数（冲突检测递增）
    #[serde(default)]
    pub contradiction_count: u32,
    /// 信念状态（Stable/Questioning/Superseded）
    #[serde(default)]
    pub status: BeliefStatus,
    /// 可选结构化度量（用于习惯类信念的冲突检测与 EMA 修正）
    ///
    /// 例如睡眠时长信念的 metric="sleep_hours", value=7.4。
    /// 当新观察值与此偏差超过阈值时触发冲突检测。
    /// 非数值型信念（如"用户喜欢晚上聊天"）此字段为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    /// 度量值（与 metric 配对，如 7.4 表示 7.4 小时）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// 匹配的行为标签列表（冲突检测用：新行为 label 命中此列表时，与 value 比较）
    ///
    /// 由认知整理 LLM 产出，如睡眠信念的 match_labels=["睡觉","午睡","小憩"]。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_labels: Vec<String>,
    /// 被哪条新信念取代（Superseded 状态时指向取代者 ID）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

fn default_confidence() -> f64 {
    0.5
}

impl Belief {
    /// 计算与另一条 Belief 的证据交集大小。
    ///
    /// 交集由两部分组成：
    /// 1. source_memory_ids 交集（直接证据重叠）
    /// 2. source_episode_ids 交集（上下文经验重叠）
    ///
    /// Episode 重叠意味着两条 Belief 从同一段经历沉淀而来，
    /// 即使恰好只共享 1 条 memory，Episode 维度的重叠也能触发合并。
    pub fn evidence_overlap(&self, other: &Belief) -> usize {
        let mem_a: HashSet<&String> = self.source_memory_ids.iter().collect();
        let mem_b: HashSet<&String> = other.source_memory_ids.iter().collect();
        let mem_overlap = mem_a.intersection(&mem_b).count();

        let ep_a: HashSet<&String> = self.source_episode_ids.iter().collect();
        let ep_b: HashSet<&String> = other.source_episode_ids.iter().collect();
        let ep_overlap = ep_a.intersection(&ep_b).count();

        mem_overlap + ep_overlap
    }

    /// 合并另一条 Belief（吸收其证据、加权 confidence、递增计数）
    pub fn merge_with(&mut self, other: &Belief, now: i64) {
        // 吸收对方记忆证据（去重）
        for id in &other.source_memory_ids {
            if !self.source_memory_ids.contains(id) {
                self.source_memory_ids.push(id.clone());
            }
        }
        // 吸收对方 Episode 证据（去重）
        for id in &other.source_episode_ids {
            if !self.source_episode_ids.contains(id) {
                self.source_episode_ids.push(id.clone());
            }
        }
        // confidence 加权平均（按 reinforcement_count 作权重）
        let w_self = (self.reinforcement_count as f64).max(1.0);
        let w_other = (other.reinforcement_count as f64).max(1.0);
        self.confidence = (self.confidence * w_self + other.confidence * w_other)
            / (w_self + w_other);
        // 数值型度量：加权平均 value
        if self.metric.is_some() && other.metric.is_some() {
            self.value = Some(match (self.value, other.value) {
                (Some(v_self), Some(v_other)) => {
                    (v_self * w_self + v_other * w_other) / (w_self + w_other)
                }
                (None, Some(v)) | (Some(v), None) => v,
                (None, None) => 0.0,
            });
        }
        self.reinforcement_count += other.reinforcement_count.max(1);
        self.last_reinforced_at = now.max(self.last_reinforced_at).max(other.last_reinforced_at);
    }

    /// EMA 修正：用新观察值平滑更新 belief 的 value（缓慢漂移，不剧烈变化）
    ///
    /// alpha 越大越保守（旧值权重越高）。默认 0.85 表示新值只占 15%。
    /// 同时递增 contradiction_count 并标记为 Questioning（若尚未被取代）。
    ///
    /// 算法选择由 metric 类型决定：
    /// - Duration / Count：线性 EMA（alpha*old + (1-alpha)*new）
    /// - TimeOfDay：循环 EMA（period=24，先调整 new 到 old 附近 ±12 范围内）
    ///   避免晚餐习惯 19:00、今天凌晨 2 点吃导致 EMA 朝错误方向（变早）漂移
    pub fn revise_value_ema(&mut self, new_value: f64, alpha: f64) {
        if let Some(old) = self.value {
            let updated = match self
                .metric
                .as_deref()
                .map(classify_metric)
                .unwrap_or_default()
            {
                MetricKind::TimeOfDay => ema_circular(old, new_value, alpha, 24.0),
                MetricKind::Duration | MetricKind::Count => {
                    alpha * old + (1.0 - alpha) * new_value
                }
            };
            self.value = Some(updated);
            self.contradiction_count += 1;
            if self.status != BeliefStatus::Superseded {
                self.status = BeliefStatus::Questioning;
            }
        }
    }

    /// 标记为已被取代
    pub fn mark_superseded(&mut self, by_id: &str) {
        self.status = BeliefStatus::Superseded;
        self.superseded_by = Some(by_id.to_string());
    }
}

/// 信念存储 —— 单角色的全部 Belief 集合
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BeliefStore {
    pub beliefs: Vec<Belief>,
}

impl BeliefStore {
    pub fn new() -> Self {
        Self { beliefs: Vec::new() }
    }

    /// 尝试合并插入：若新 Belief 与既有 Belief 证据交集 ≥ threshold，则合并；
    /// 否则新建。返回最终 Belief 的 ID。
    pub fn upsert_with_merge(&mut self, draft: Belief, threshold: usize, now: i64) -> String {
        // 找出证据交集最大的既有 Belief
        let mut best_idx: Option<(usize, usize)> = None;
        for (i, existing) in self.beliefs.iter().enumerate() {
            let overlap = existing.evidence_overlap(&draft);
            if overlap >= threshold {
                match best_idx {
                    Some((_, cur)) if overlap <= cur => {}
                    _ => best_idx = Some((i, overlap)),
                }
            }
        }

        match best_idx {
            Some((idx, _)) => {
                self.beliefs[idx].merge_with(&draft, now);
                self.beliefs[idx].id.clone()
            }
            None => {
                let id = draft.id.clone();
                self.beliefs.push(draft);
                id
            }
        }
    }

    /// 按主体过滤（检索时常用）
    pub fn by_subject(&self, subject: &str) -> Vec<&Belief> {
        self.beliefs.iter().filter(|b| b.subject == subject).collect()
    }

    /// 按类别过滤
    pub fn by_category(&self, cat: BeliefCategory) -> Vec<&Belief> {
        self.beliefs.iter().filter(|b| b.category == cat).collect()
    }

    /// 取置信度 Top-N（prompt 注入用）
    pub fn top_n_by_confidence(&self, n: usize) -> Vec<&Belief> {
        let mut v: Vec<&Belief> = self.beliefs.iter().collect();
        v.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(n);
        v
    }

    /// 按主体 + 度量名查找（冲突检测用）
    ///
    /// 返回该 subject 下拥有指定 metric 的、未被取代的信念。
    pub fn by_subject_and_metric(&self, subject: &str, metric: &str) -> Vec<&Belief> {
        self.beliefs
            .iter()
            .filter(|b| {
                b.subject == subject
                    && b.metric.as_deref() == Some(metric)
                    && b.status != BeliefStatus::Superseded
            })
            .collect()
    }

    /// 按主体过滤，仅未被取代的（prompt 注入与认知展示用）
    pub fn active_by_subject(&self, subject: &str) -> Vec<&Belief> {
        self.beliefs
            .iter()
            .filter(|b| b.subject == subject && b.status != BeliefStatus::Superseded)
            .collect()
    }

    /// 按 ID 获取可变引用
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Belief> {
        self.beliefs.iter_mut().find(|b| b.id == id)
    }

    /// EMA 修正指定信念的 value
    pub fn revise_value_ema(&mut self, belief_id: &str, new_value: f64, alpha: f64) -> bool {
        if let Some(belief) = self.get_mut(belief_id) {
            belief.revise_value_ema(new_value, alpha);
            true
        } else {
            false
        }
    }

    /// 持久化到 JSON
    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// 从 JSON 加载
    pub fn load(path: &PathBuf) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::new(),
        }
    }
}

/// 线程安全的 BeliefStore 句柄
pub type SharedBeliefStore = std::sync::Arc<RwLock<BeliefStore>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_metric() {
        assert_eq!(classify_metric("sleep_hours"), MetricKind::Duration);
        assert_eq!(classify_metric("study_minutes"), MetricKind::Duration);
        assert_eq!(classify_metric("game_duration"), MetricKind::Duration);
        assert_eq!(classify_metric("dinner_hour"), MetricKind::TimeOfDay);
        assert_eq!(classify_metric("wake_time"), MetricKind::TimeOfDay);
        assert_eq!(classify_metric("bed_time_of_day"), MetricKind::TimeOfDay);
        assert_eq!(classify_metric("meal_count"), MetricKind::Count);
        assert_eq!(classify_metric("break_frequency"), MetricKind::Count);
        assert_eq!(classify_metric("snack_times_per_day"), MetricKind::Count);
        // 未知 metric 默认为 Duration（向后兼容）
        assert_eq!(classify_metric("unknown_metric"), MetricKind::Duration);
        assert_eq!(classify_metric(""), MetricKind::Duration);
    }

    #[test]
    fn test_circular_distance_basic() {
        // 同点距离为 0
        assert!((circular_distance(19.0, 19.0, 24.0) - 0.0).abs() < 1e-6);
        // 简单距离
        assert!((circular_distance(19.0, 23.0, 24.0) - 4.0).abs() < 1e-6);
        // 跨日距离取最短弧
        assert!((circular_distance(19.0, 2.0, 24.0) - 5.0).abs() < 1e-6);
        assert!((circular_distance(23.5, 0.5, 24.0) - 1.0).abs() < 1e-6);
        // 对径点距离 = period/2
        assert!((circular_distance(0.0, 12.0, 24.0) - 12.0).abs() < 1e-6);
    }

    #[test]
    fn test_ema_circular_linear_case() {
        // dinner_hour=19, new=23, alpha=0.85
        // diff=4 < 12, adjusted_diff=4, ema = 0.85*19 + 0.15*23 = 19.6
        let v = ema_circular(19.0, 23.0, 0.85, 24.0);
        assert!((v - 19.6).abs() < 1e-6);
    }

    #[test]
    fn test_ema_circular_wrap_around() {
        // dinner_hour=19, new=2（次日凌晨）, alpha=0.85
        // 原始 diff=-17，调整后 +7（朝"更晚"方向），new_adjusted=26
        // ema = 0.85*19 + 0.15*26 = 16.15 + 3.9 = 20.05
        let v = ema_circular(19.0, 2.0, 0.85, 24.0);
        assert!((v - 20.05).abs() < 1e-6);
        // 结果应在 [19, 22] 区间内（朝更晚方向漂移），不会变早
        assert!(v > 19.0 && v < 22.0);
    }

    #[test]
    fn test_revise_value_ema_picks_circular_for_time_of_day() {
        let mut belief = Belief {
            id: "test".into(),
            statement: "用户通常 19:00 吃晚饭".into(),
            subject: "user".into(),
            category: BeliefCategory::Habit,
            confidence: 0.9,
            source_memory_ids: vec!["m1".into()],
            source_episode_ids: vec![],
            created_at: 0,
            last_reinforced_at: 0,
            reinforcement_count: 5,
            contradiction_count: 0,
            status: BeliefStatus::Stable,
            metric: Some("dinner_hour".into()),
            value: Some(19.0),
            match_labels: vec!["吃晚饭".into()],
            superseded_by: None,
        };
        // 凌晨 2 点吃晚饭：循环 EMA 应让 value 朝更晚方向漂移
        belief.revise_value_ema(2.0, 0.85);
        let v = belief.value.unwrap();
        assert!(v > 19.0 && v < 22.0, "expected v in (19,22), got {}", v);
        assert_eq!(belief.contradiction_count, 1);
        assert_eq!(belief.status, BeliefStatus::Questioning);
    }

    #[test]
    fn test_revise_value_ema_linear_for_duration() {
        let mut belief = Belief {
            id: "test".into(),
            statement: "用户通常睡 7.4 小时".into(),
            subject: "user".into(),
            category: BeliefCategory::Habit,
            confidence: 0.9,
            source_memory_ids: vec!["m1".into()],
            source_episode_ids: vec![],
            created_at: 0,
            last_reinforced_at: 0,
            reinforcement_count: 5,
            contradiction_count: 0,
            status: BeliefStatus::Stable,
            metric: Some("sleep_hours".into()),
            value: Some(7.4),
            match_labels: vec!["睡觉".into()],
            superseded_by: None,
        };
        // sleep_hours=7.4, new=11.0, alpha=0.85 → 线性 EMA
        belief.revise_value_ema(11.0, 0.85);
        let v = belief.value.unwrap();
        // 0.85*7.4 + 0.15*11 = 6.29 + 1.65 = 7.94
        assert!((v - 7.94).abs() < 1e-6, "got {}", v);
    }
}
