//! 证据驱动记忆可信度系统
//!
//! 每条记忆携带 `reinforcement`（正面证据）+ `disputation`（负面证据）双独立时钟，
//! 按不同半衰期衰减。用户反驳可直接削弱已有记忆，避免"删除即不可逆"。
//!
//! 核心设计：
//! - 衰减在读取时计算（read-time decay），不修改存储状态
//! - reinforcement / disputation 拥有独立时间戳，一侧信号不影响另一侧衰减
//! - 7 种证据来源映射到不同的 delta 权重
//! - `protected` 标记的角色卡来源记忆永不被归档
//! - `sub_zero_days` 独立归档倒计时：score<0 累计达 14 天才真正归档

use super::types::{current_timestamp, MemoryItem};

// ============================================================================
// 常量
// ============================================================================

/// reinforcement 半衰期（天）：30 天衰到一半
pub const REIN_HALF_LIFE_DAYS: f64 = 30.0;
/// disputation 半衰期（天）：180 天衰到一半（6 个月）
///
/// 否定信号保留更久，给用户态度回转的可能。
pub const DISP_HALF_LIFE_DAYS: f64 = 180.0;
/// score ≤ 此阈值 → archive_candidate
pub const ARCHIVE_THRESHOLD: f64 = -2.0;
/// score ≥ 此阈值 → confirmed
pub const CONFIRMED_THRESHOLD: f64 = 1.0;
/// score ≥ 此阈值 → promoted
pub const PROMOTED_THRESHOLD: f64 = 2.0;
/// sub_zero_days 累计达此天数 → 真正归档
pub const ARCHIVE_DAYS: u32 = 14;
/// 反驳后重新确认宽限期（次数）。
/// 用户反驳后，正面信号需经过这么多轮才能完全恢复可信度。
pub const REBUTTAL_GRACE_TICKS: u32 = 3;

/// user_fact 正面信号 delta（间接强化，银标准）
pub const USER_FACT_REINFORCE_DELTA: f64 = 0.5;
/// user_fact 负面信号 delta（间接否定，即使间接也强权）
pub const USER_FACT_NEGATE_DELTA: f64 = 1.0;
/// user_confirm delta（直接确认，金标准）
pub const USER_CONFIRM_DELTA: f64 = 1.0;
/// user_rebut delta（直接反驳）
pub const USER_REBUT_DELTA: f64 = 1.0;
/// user_keyword_rebut delta（关键词 + LLM target）
pub const USER_KEYWORD_REBUT_DELTA: f64 = 1.0;
/// user_ignore delta（扣在 rein 侧，可为负）
pub const IGNORED_REINFORCEMENT_DELTA: f64 = -0.2;
/// user_fact combo 触发阈值
pub const USER_FACT_REINFORCE_COMBO_THRESHOLD: u32 = 2;
/// combo 超阈值后每条额外加权
pub const USER_FACT_REINFORCE_COMBO_BONUS: f64 = 0.5;

/// 一天的秒数
const SECS_PER_DAY: f64 = 86400.0;

// ============================================================================
// 证据来源枚举
// ============================================================================

/// 7 种证据来源
///
/// 每种来源有不同的 delta 权重和作用方向（reinforcement 或 disputation）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceSource {
    /// Stage-2 LLM 判定新事实强化/反驳某记忆（间接，银标准）
    UserFact,
    /// check_feedback 返回 confirmed（直接，金标准）
    UserConfirm,
    /// check_feedback 返回 denied（直接）
    UserRebut,
    /// check_feedback 返回 ignored（扣在 rein 侧）
    UserIgnore,
    /// 本地负面关键词命中 + LLM 判 target（直接 + 显式）
    UserKeywordRebut,
    /// 一次性迁移（legacy status → evidence seed）
    MigrationSeed,
    /// reflection promote 时合并入 persona entry（max-rule 合并）
    PromoteMerge,
}

impl EvidenceSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            EvidenceSource::UserFact => "user_fact",
            EvidenceSource::UserConfirm => "user_confirm",
            EvidenceSource::UserRebut => "user_rebut",
            EvidenceSource::UserIgnore => "user_ignore",
            EvidenceSource::UserKeywordRebut => "user_keyword_rebut",
            EvidenceSource::MigrationSeed => "migration_seed",
            EvidenceSource::PromoteMerge => "promote_merge",
        }
    }
}

/// 信号方向：强化或否定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// 强化（reinforcement +=）
    Reinforces,
    /// 否定（disputation +=）
    Negates,
}

// ============================================================================
// 派生状态
// ============================================================================

/// 记忆的派生可信度状态。
///
/// 注意：`Protected` 和 `SubZero` 不是 `derive_status` 的返回值，
/// 而是独立的判定（前者看 `protected` 字段，后者看 `sub_zero_days` 计数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    /// score ≥ 2.0，已晋升为长期核心记忆
    Promoted,
    /// 1.0 ≤ score < 2.0，已确认
    Confirmed,
    /// -2.0 < score < 1.0，待定（补集）
    Pending,
    /// score ≤ -2.0，归档候选
    ArchiveCandidate,
}

// ============================================================================
// 衰减与评分（纯函数，读路径）
// ============================================================================

/// 计算从给定时间戳到 now 的天数。
///
/// - 时间戳为 0 或无效 → age = 0（不衰减）
/// - delta ≤ 0（时钟回拨或未来时间戳）→ age = 0（不衰减）
fn age_days(last_signal_at: f64, now: f64) -> f64 {
    if last_signal_at <= 0.0 {
        return 0.0;
    }
    let delta = now - last_signal_at;
    if delta <= 0.0 {
        return 0.0;
    }
    delta / SECS_PER_DAY
}

/// 衰减后的正面证据有效值：`reinforcement * 0.5^(age_days / 30)`
pub fn effective_reinforcement(memory: &MemoryItem, now: f64) -> f64 {
    let age = age_days(memory.rein_last_signal_at, now);
    memory.reinforcement * 0.5_f64.powf(age / REIN_HALF_LIFE_DAYS)
}

/// 衰减后的负面证据有效值：`disputation * 0.5^(age_days / 180)`
pub fn effective_disputation(memory: &MemoryItem, now: f64) -> f64 {
    let age = age_days(memory.disp_last_signal_at, now);
    memory.disputation * 0.5_f64.powf(age / DISP_HALF_LIFE_DAYS)
}

/// 计算证据评分。
///
/// - `protected` 记忆返回 `+∞`，永不归档/预算挤出
/// - 否则返回 `effective_reinforcement - effective_disputation`
pub fn evidence_score(memory: &MemoryItem, now: f64) -> f64 {
    if memory.protected {
        return f64::INFINITY;
    }
    effective_reinforcement(memory, now) - effective_disputation(memory, now)
}

/// 派生记忆的可信度状态。
pub fn derive_status(memory: &MemoryItem, now: f64) -> EvidenceStatus {
    let s = evidence_score(memory, now);
    if s >= PROMOTED_THRESHOLD {
        EvidenceStatus::Promoted
    } else if s >= CONFIRMED_THRESHOLD {
        EvidenceStatus::Confirmed
    } else if s <= ARCHIVE_THRESHOLD {
        EvidenceStatus::ArchiveCandidate
    } else {
        EvidenceStatus::Pending
    }
}

// ============================================================================
// Delta 应用（写路径）
// ============================================================================

/// 证据信号应用结果（快照字段）。
///
/// 由 `apply_evidence_signal` 返回，调用方负责持久化到 MemoryItem。
#[derive(Debug, Clone)]
pub struct EvidenceSnapshot {
    pub reinforcement: f64,
    pub disputation: f64,
    pub rein_last_signal_at: f64,
    pub disp_last_signal_at: f64,
    pub user_fact_reinforce_count: u32,
    /// 反驳宽限期剩余次数
    pub rebuttal_grace_remaining: u32,
}

impl EvidenceSnapshot {
    /// 将快照应用到记忆条目（仅更新证据字段，不触碰其他）。
    pub fn apply_to(&self, memory: &mut MemoryItem) {
        memory.reinforcement = self.reinforcement;
        memory.disputation = self.disputation;
        memory.rein_last_signal_at = self.rein_last_signal_at;
        memory.disp_last_signal_at = self.disp_last_signal_at;
        memory.user_fact_reinforce_count = self.user_fact_reinforce_count;
        memory.rebuttal_grace_remaining = self.rebuttal_grace_remaining;
    }
}

/// 根据来源 + 方向解析 delta 数值。
///
/// 返回 `(rein_delta, disp_delta)`：
/// - `rein_delta`：作用在 reinforcement 上的增量（可为负，如 user_ignore）
/// - `disp_delta`：作用在 disputation 上的增量（非负）
pub fn resolve_delta(source: EvidenceSource, kind: SignalKind) -> (f64, f64) {
    match (source, kind) {
        (EvidenceSource::UserFact, SignalKind::Reinforces) => (USER_FACT_REINFORCE_DELTA, 0.0),
        (EvidenceSource::UserFact, SignalKind::Negates) => (0.0, USER_FACT_NEGATE_DELTA),
        (EvidenceSource::UserConfirm, _) => (USER_CONFIRM_DELTA, 0.0),
        (EvidenceSource::UserRebut, _) => (0.0, USER_REBUT_DELTA),
        (EvidenceSource::UserIgnore, _) => (IGNORED_REINFORCEMENT_DELTA, 0.0),
        (EvidenceSource::UserKeywordRebut, _) => (0.0, USER_KEYWORD_REBUT_DELTA),
        // 迁移种子与 promote_merge 的 delta 由调用方直接指定，此处返回 0
        (EvidenceSource::MigrationSeed, _) | (EvidenceSource::PromoteMerge, _) => (0.0, 0.0),
    }
}

/// 应用证据信号到记忆条目，返回新的证据快照。
///
/// 这是写路径的核心函数。调用方负责：
/// 1. 先记录事件（EventLog，见 event_log.rs）
/// 2. 再应用此快照到 MemoryItem
/// 3. 最后持久化
///
/// Combo 机制：仅 `user_fact + reinforces` 时触发，count > 2 后每条额外 +0.5。
/// 时间戳重置规则：仅当对应侧 delta ≠ 0 时重置该侧时间戳。
pub fn apply_evidence_signal(
    memory: &MemoryItem,
    source: EvidenceSource,
    kind: SignalKind,
    now: f64,
) -> EvidenceSnapshot {
    let (mut rein_delta, mut disp_delta) = resolve_delta(source, kind);

    // 迁移种子：直接使用传入的 seed 值（调用方应预先设置 memory 字段）
    // 此处返回当前快照不变
    if source == EvidenceSource::MigrationSeed {
        return EvidenceSnapshot {
            reinforcement: memory.reinforcement,
            disputation: memory.disputation,
            rein_last_signal_at: memory.rein_last_signal_at,
            disp_last_signal_at: memory.disp_last_signal_at,
            user_fact_reinforce_count: memory.user_fact_reinforce_count,
            rebuttal_grace_remaining: memory.rebuttal_grace_remaining,
        };
    }

    // promote_merge：max-rule 合并，调用方应预先设置 memory 字段
    if source == EvidenceSource::PromoteMerge {
        return EvidenceSnapshot {
            reinforcement: memory.reinforcement,
            disputation: memory.disputation,
            rein_last_signal_at: memory.rein_last_signal_at,
            disp_last_signal_at: memory.disp_last_signal_at,
            user_fact_reinforce_count: memory.user_fact_reinforce_count,
            rebuttal_grace_remaining: memory.rebuttal_grace_remaining,
        };
    }

    // 反驳宽限期管理：
    // - 反驳信号触发时启动宽限期（grace = REBUTTAL_GRACE_TICKS）
    // - 宽限期内正面信号 delta 减半（重新确认需要更多证据）
    // - 每次正面信号消耗 1 点宽限
    let is_rebuttal = matches!(
        source,
        EvidenceSource::UserRebut | EvidenceSource::UserKeywordRebut
    );
    let mut grace = memory.rebuttal_grace_remaining;

    if is_rebuttal {
        grace = REBUTTAL_GRACE_TICKS;
    }

    // 宽限期内正面信号减半
    if grace > 0 && rein_delta > 0.0 && !is_rebuttal {
        rein_delta *= 0.5;
        grace = grace.saturating_sub(1);
    }

    let mut new_rein = memory.reinforcement + rein_delta;
    let mut new_disp = memory.disputation + disp_delta;
    // disputation 非负
    if new_disp < 0.0 {
        new_disp = 0.0;
    }

    let mut new_count = memory.user_fact_reinforce_count;

    // Combo：仅 user_fact + reinforces
    if source == EvidenceSource::UserFact && rein_delta > 0.0 {
        new_count += 1;
        if new_count > USER_FACT_REINFORCE_COMBO_THRESHOLD {
            new_rein += USER_FACT_REINFORCE_COMBO_BONUS;
        }
    }

    // 时间戳重置：仅当对应侧 delta ≠ 0
    let new_rein_ts = if rein_delta != 0.0 {
        now
    } else {
        memory.rein_last_signal_at
    };
    let new_disp_ts = if disp_delta != 0.0 {
        now
    } else {
        memory.disp_last_signal_at
    };

    // 抑制未使用赋值警告（rein_delta/disp_delta 在 MigrationSeed/PromoteMerge 提前返回后仍可能被读）
    let _ = (&mut rein_delta, &mut disp_delta);

    EvidenceSnapshot {
        reinforcement: new_rein,
        disputation: new_disp,
        rein_last_signal_at: new_rein_ts,
        disp_last_signal_at: new_disp_ts,
        user_fact_reinforce_count: new_count,
        rebuttal_grace_remaining: grace,
    }
}

/// 根据重要性给出 reflection 的初始 reinforcement seed。
///
/// 高 importance 的事实给一个初始 rein seed，加速穿过 CONFIRMED/PROMOTED。
/// 基于 MAX（非 avg/sum），因为一个高重要性事实就足以标记记忆为重要。
pub fn initial_reinforcement_from_importance(importance: f64) -> f64 {
    if importance >= 10.0 {
        0.8
    } else if importance >= 9.0 {
        0.6
    } else if importance >= 8.0 {
        0.4
    } else if importance >= 7.0 {
        0.2
    } else {
        0.0
    }
}

// ============================================================================
// sub_zero 归档倒计时
// ============================================================================

/// 获取当前日期的 YYYY-MM-DD 字符串（本地时区）。
fn today_date_str() -> String {
    use chrono::Local;
    Local::now().format("%Y-%m-%d").to_string()
}

/// 检查并递增 sub_zero_days（按自然日防抖）。
///
/// 当 `score < 0` 且当天未计数过 → `sub_zero_days += 1`。
/// 累计达 `ARCHIVE_DAYS` (14) 天 → 返回 true，调用方应触发真正归档。
///
/// **宽限期保护**：`rebuttal_grace_remaining > 0` 时，即使 score 回正也不重置计数器，
/// 要求多次正面证据才能完全撤销反驳。
///
/// 返回 `(new_sub_zero_days, should_archive)`。
pub fn maybe_mark_sub_zero(memory: &mut MemoryItem, now: f64) -> (u32, bool) {
    if memory.protected {
        return (memory.sub_zero_days, false);
    }

    let score = evidence_score(memory, now);
    if score >= 0.0 && memory.rebuttal_grace_remaining == 0 {
        // score 非负且无宽限期 → 重置计数器（用户态度回转）
        if memory.sub_zero_days > 0 {
            memory.sub_zero_days = 0;
        }
        return (0, false);
    }

    let today = today_date_str();
    if memory.sub_zero_last_increment_date == today {
        // 今天已计数过
        return (memory.sub_zero_days, memory.sub_zero_days >= ARCHIVE_DAYS);
    }

    memory.sub_zero_days += 1;
    memory.sub_zero_last_increment_date = today;
    let should_archive = memory.sub_zero_days >= ARCHIVE_DAYS;
    (memory.sub_zero_days, should_archive)
}

// ============================================================================
// 迁移种子
// ============================================================================

/// legacy 状态 → evidence seed 映射。
///
/// 用于一次性迁移已有的 importance/age 体系到 evidence 体系：
/// - 高 importance → promoted seed (rein=2.0)
/// - 中 importance → confirmed seed (rein=1.0)
/// - 低 importance + 老旧 → archive seed (disp=2.0)
pub fn migration_seed_from_importance(importance: f64, age_hours: f64) -> (f64, f64) {
    if importance >= 0.8 {
        (PROMOTED_THRESHOLD, 0.0)
    } else if importance >= 0.5 {
        (CONFIRMED_THRESHOLD, 0.0)
    } else if importance < 0.3 && age_hours > 168.0 {
        // 低重要度 + 超过一周 → 否定种子
        (0.0, ARCHIVE_THRESHOLD.abs())
    } else {
        (0.0, 0.0)
    }
}

/// 应用迁移种子到记忆条目（一次性）。
///
/// 仅当记忆的 reinforcement 和 disputation 都为 0（未迁移过）时执行。
pub fn apply_migration_seed(memory: &mut MemoryItem) {
    if memory.reinforcement != 0.0 || memory.disputation != 0.0 {
        return;
    }
    if memory.protected {
        // protected 记忆无需 seed，evidence_score 已返回 +∞
        return;
    }
    let age_hours = (current_timestamp() - memory.timestamp).max(0.0) / 3600.0;
    let (rein, disp) = migration_seed_from_importance(memory.importance, age_hours);
    memory.reinforcement = rein;
    memory.disputation = disp;
    let now = current_timestamp();
    if rein != 0.0 {
        memory.rein_last_signal_at = now;
    }
    if disp != 0.0 {
        memory.disp_last_signal_at = now;
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;

    fn make_memory() -> MemoryItem {
        MemoryItem::new("test".to_string(), Granularity::Summary, 0.5)
    }

    #[test]
    fn test_evidence_score_initial_zero() {
        let m = make_memory();
        let now = current_timestamp();
        // 初始 reinforcement=0, disputation=0 → score=0
        let s = evidence_score(&m, now);
        assert_eq!(s, 0.0);
        assert_eq!(derive_status(&m, now), EvidenceStatus::Pending);
    }

    #[test]
    fn test_protected_returns_infinity() {
        let mut m = make_memory();
        m.protected = true;
        let now = current_timestamp();
        assert!(evidence_score(&m, now).is_infinite());
    }

    #[test]
    fn test_user_confirm_promotes_to_confirmed() {
        let mut m = make_memory();
        let now = current_timestamp();
        let snap = apply_evidence_signal(&m, EvidenceSource::UserConfirm, SignalKind::Reinforces, now);
        snap.apply_to(&mut m);
        let s = evidence_score(&m, now);
        assert!((s - 1.0).abs() < 1e-9, "expected 1.0, got {}", s);
        assert_eq!(derive_status(&m, now), EvidenceStatus::Confirmed);
    }

    #[test]
    fn test_user_rebut_creates_archive_candidate() {
        let mut m = make_memory();
        let now = current_timestamp();
        // 两次反驳 → disputation=2.0 → score=-2.0 → archive_candidate
        let snap1 = apply_evidence_signal(&m, EvidenceSource::UserRebut, SignalKind::Negates, now);
        snap1.apply_to(&mut m);
        let snap2 = apply_evidence_signal(&m, EvidenceSource::UserRebut, SignalKind::Negates, now);
        snap2.apply_to(&mut m);
        assert_eq!(derive_status(&m, now), EvidenceStatus::ArchiveCandidate);
    }

    #[test]
    fn test_user_fact_combo_bonus() {
        let mut m = make_memory();
        let now = current_timestamp();
        // 3 次 user_fact reinforces: 0.5 + 0.5 + 0.5 + 0.5(bonus) = 2.0
        for _ in 0..3 {
            let snap = apply_evidence_signal(&m, EvidenceSource::UserFact, SignalKind::Reinforces, now);
            snap.apply_to(&mut m);
        }
        let s = evidence_score(&m, now);
        assert!((s - 2.0).abs() < 1e-9, "expected 2.0 with combo, got {}", s);
        assert_eq!(derive_status(&m, now), EvidenceStatus::Promoted);
    }

    #[test]
    fn test_disputation_non_negative() {
        let mut m = make_memory();
        let now = current_timestamp();
        // user_ignore 给 rein -= 0.2，不影响 disp
        let snap = apply_evidence_signal(&m, EvidenceSource::UserIgnore, SignalKind::Reinforces, now);
        snap.apply_to(&mut m);
        assert!((m.reinforcement - (-0.2)).abs() < 1e-9);
        assert_eq!(m.disputation, 0.0);
    }

    #[test]
    fn test_decay_over_time() {
        let mut m = make_memory();
        let now = current_timestamp();
        // 给一个 reinforcement=1.0
        let snap = apply_evidence_signal(&m, EvidenceSource::UserConfirm, SignalKind::Reinforces, now);
        snap.apply_to(&mut m);
        // 30 天后应该衰到 0.5
        let future = now + 30.0 * SECS_PER_DAY;
        let s = evidence_score(&m, future);
        assert!((s - 0.5).abs() < 0.01, "expected ~0.5 after 30 days, got {}", s);
    }

    #[test]
    fn test_disputation_decays_slower() {
        let mut m = make_memory();
        let now = current_timestamp();
        // disp=1.0
        let snap = apply_evidence_signal(&m, EvidenceSource::UserRebut, SignalKind::Negates, now);
        snap.apply_to(&mut m);
        // 30 天后 disp 应衰到 ~0.89（半衰期 180 天）
        let future = now + 30.0 * SECS_PER_DAY;
        let eff_disp = effective_disputation(&m, future);
        assert!(eff_disp > 0.8 && eff_disp < 0.9, "expected ~0.89, got {}", eff_disp);
    }

    #[test]
    fn test_independent_clocks() {
        let mut m = make_memory();
        let now = current_timestamp();
        // 只触 reinforcement
        let snap = apply_evidence_signal(&m, EvidenceSource::UserConfirm, SignalKind::Reinforces, now);
        snap.apply_to(&mut m);
        assert!(m.rein_last_signal_at > 0.0);
        assert_eq!(m.disp_last_signal_at, 0.0);
        // disp 时钟未触 → effective_disputation 不衰减（age=0）
        let future = now + 100.0 * SECS_PER_DAY;
        let eff_disp = effective_disputation(&m, future);
        assert_eq!(eff_disp, 0.0); // disp=0, 衰减后仍为 0
    }

    #[test]
    fn test_sub_zero_increment() {
        let mut m = make_memory();
        let now = current_timestamp();
        // 给 disp=2.0 让 score=-2.0
        let snap1 = apply_evidence_signal(&m, EvidenceSource::UserRebut, SignalKind::Negates, now);
        snap1.apply_to(&mut m);
        let snap2 = apply_evidence_signal(&m, EvidenceSource::UserRebut, SignalKind::Negates, now);
        snap2.apply_to(&mut m);

        let (days, archive) = maybe_mark_sub_zero(&mut m, now);
        assert_eq!(days, 1);
        assert!(!archive);
    }

    #[test]
    fn test_migration_seed() {
        let mut m = make_memory();
        m.importance = 0.9;
        apply_migration_seed(&mut m);
        assert!((m.reinforcement - PROMOTED_THRESHOLD).abs() < 1e-9);
    }
}
