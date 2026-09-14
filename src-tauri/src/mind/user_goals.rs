//! 用户长期目标账本 —— Experience Continuity 的核心载体。
//!
//! 与 `mind/goal.rs` 的 Goal（角色自身运行时目标，影响 Attention 分配）不同，
//! UserGoal 是**用户的**长期目标（周~月级，带 deadline），用于：
//! - 让 LLM 知道"用户当前处于什么人生阶段"
//! - 在 inner_monologue / 主对话 prompt 中注入"距离考研还有 X 天"这类时间关系事实
//! - 与 WorldBrief 的瞬时事实组合，产出"凌晨 + 长期目标 + 连续工作 → 疲劳风险"这类推理
//!
//! 创建路径：
//! - `Dialogue` 来源：用户明说（"我明年要考研"），由 reflection 阶段 LLM 抽取，最高可信度
//! - `Inferred` 来源：深层反思 LLM 提议，低可信度，confidence < 0.6 丢弃
//!
//! 状态机：Active → Paused / Completed / Abandoned
//! 状态变更允许 Inferred，但 LLM 需给出理由（写入 source_quote 或 inferred_reason）

use std::path::PathBuf;

use chrono::{DateTime, Local, TimeZone};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};

/// 活跃用户目标上限（超过时按 deadline 紧迫度淘汰最不紧迫的）
const MAX_ACTIVE_GOALS: usize = 5;

/// 用户目标来源
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserGoalSource {
    /// 用户明说，最高可信度
    Dialogue {
        /// 用户原话片段
        quote: String,
        /// 抽取时间（Unix 秒）
        extracted_at: f64,
    },
    /// LLM 反思阶段推断，低可信度
    Inferred {
        confidence: f64,
        /// 推断理由
        reason: String,
        /// 推断时间
        inferred_at: f64,
    },
}

/// 用户目标状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserGoalState {
    /// 活跃进行中
    Active,
    /// 暂停（用户说"先放一放"）
    Paused,
    /// 已完成（用户说"考完了"）
    Completed,
    /// 已放弃（长期未提及 + LLM 判定）
    Abandoned,
}

impl Default for UserGoalState {
    fn default() -> Self {
        Self::Active
    }
}

/// 单条用户长期目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGoal {
    pub id: String,
    /// 目标标签（"准备考研" / "写毕业论文" / "学日语"）
    pub label: String,
    /// 开始时间（Unix 秒，本地时区）
    pub started_at: f64,
    /// 截止时间（None = 无明确 deadline）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Local>>,
    /// 来源
    pub source: UserGoalSource,
    /// 当前状态
    #[serde(default)]
    pub state: UserGoalState,
    /// 最后更新时间（Unix 秒）
    pub last_updated_at: f64,
    /// 关联 Belief ID（如"考研压力大"这类 Belief 指向此目标）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_belief_ids: Vec<String>,
}

impl UserGoal {
    /// 距 deadline 的剩余天数（负数表示已过期，None 表示无 deadline）
    pub fn days_to_deadline(&self) -> Option<i64> {
        self.deadline.map(|dl| {
            let now = Local::now();
            (dl - now).num_days()
        })
    }

    /// 是否处于"活跃"语义（Active 或 Paused 都算仍在用户人生轨迹里）
    pub fn is_active_track(&self) -> bool {
        matches!(self.state, UserGoalState::Active | UserGoalState::Paused)
    }
}

/// prompt 注入用的轻量摘要（不含 ID/source_quote 等元数据）
#[derive(Debug, Clone)]
pub struct UserGoalBrief {
    pub label: String,
    pub days_to_deadline: Option<i64>,
    pub state: UserGoalState,
}

/// 用户目标账本
pub struct UserGoalLedger {
    goals: RwLock<Vec<UserGoal>>,
    persistence_path: PathBuf,
}

impl UserGoalLedger {
    pub fn new(persistence_path: PathBuf) -> Self {
        let goals = Self::load(&persistence_path).unwrap_or_default();
        Self {
            goals: RwLock::new(goals),
            persistence_path,
        }
    }

    /// 取活跃目标（Active + Paused），按 deadline 紧迫度升序
    pub fn active_goals(&self) -> Vec<UserGoal> {
        let goals = self.goals.read();
        let mut active: Vec<UserGoal> = goals
            .iter()
            .filter(|g| g.is_active_track())
            .cloned()
            .collect();
        active.sort_by(|a, b| {
            let da = a.days_to_deadline().unwrap_or(i64::MAX);
            let db = b.days_to_deadline().unwrap_or(i64::MAX);
            da.cmp(&db)
        });
        active
    }

    /// 取活跃目标的 prompt 摘要（Top-N）
    pub fn active_briefs(&self, n: usize) -> Vec<UserGoalBrief> {
        self.active_goals()
            .into_iter()
            .take(n)
            .map(|g| {
                let days = g.days_to_deadline();
                UserGoalBrief {
                    label: g.label,
                    days_to_deadline: days,
                    state: g.state,
                }
            })
            .collect()
    }

    /// 创建新目标
    ///
    /// 返回 goal_id。若已存在相同 label 的 Active 目标，视为延续，不创建。
    pub fn create(&self, label: &str, deadline: Option<DateTime<Local>>, source: UserGoalSource) -> String {
        let label = label.trim().to_string();
        let now = Local::now().timestamp() as f64;
        let mut goals = self.goals.write();

        // 同名 Active 目标存在则不重复创建
        if goals.iter().any(|g| g.label == label && g.state == UserGoalState::Active) {
            tracing::debug!(
                "[UserGoalLedger] 目标「{}」已存在且 Active，跳过创建",
                label
            );
            return String::new();
        }

        let id = format!("ugoal_{}_{}", now as i64, rand::random::<u32>());
        let goal = UserGoal {
            id: id.clone(),
            label: label.clone(),
            started_at: now,
            deadline,
            source,
            state: UserGoalState::Active,
            last_updated_at: now,
            related_belief_ids: Vec::new(),
        };
        goals.push(goal);
        drop(goals);

        let _ = self.persist();
        tracing::info!("[UserGoalLedger] 创建用户目标: id={} label=\"{}\"", id, label);
        id
    }

    /// 按标签模糊匹配更新目标状态（label 包含 query 或 query 包含 label 即视为匹配）
    ///
    /// 返回是否匹配到并更新了目标。
    pub fn transition_state(&self, query: &str, new_state: UserGoalState) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return false;
        }
        let now = Local::now().timestamp() as f64;
        let mut goals = self.goals.write();
        let mut matched = false;
        for g in goals.iter_mut() {
            if g.is_active_track() && (g.label.contains(query) || query.contains(&g.label)) {
                g.state = new_state;
                g.last_updated_at = now;
                matched = true;
                tracing::info!(
                    "[UserGoalLedger] 目标「{}」状态变更: {:?}",
                    g.label,
                    new_state
                );
            }
        }
        drop(goals);
        if matched {
            let _ = self.persist();
        }
        matched
    }

    /// 更新 deadline（按 label 模糊匹配）
    pub fn update_deadline(&self, query: &str, deadline: Option<DateTime<Local>>) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return false;
        }
        let now = Local::now().timestamp() as f64;
        let mut goals = self.goals.write();
        let mut matched = false;
        for g in goals.iter_mut() {
            if g.is_active_track() && (g.label.contains(query) || query.contains(&g.label)) {
                g.deadline = deadline;
                g.last_updated_at = now;
                matched = true;
                tracing::info!(
                    "[UserGoalLedger] 目标「{}」deadline 更新: {:?}",
                    g.label,
                    deadline
                );
            }
        }
        drop(goals);
        if matched {
            let _ = self.persist();
        }
        matched
    }

    /// 活跃目标数超限淘汰：保留 deadline 最紧迫的 MAX_ACTIVE_GOALS 条
    pub fn enforce_capacity(&self) {
        let mut goals = self.goals.write();
        let active_count = goals.iter().filter(|g| g.is_active_track()).count();
        if active_count <= MAX_ACTIVE_GOALS {
            return;
        }
        // 收集活跃目标的索引与 deadline
        let mut indexed: Vec<(usize, i64)> = goals
            .iter()
            .enumerate()
            .filter(|(_, g)| g.is_active_track())
            .map(|(i, g)| (i, g.days_to_deadline().unwrap_or(i64::MAX)))
            .collect();
        indexed.sort_by(|a, b| b.1.cmp(&a.1)); // 最不紧迫的在前
        let drop_n = active_count - MAX_ACTIVE_GOALS;
        for (idx, _) in indexed.into_iter().take(drop_n) {
            goals[idx].state = UserGoalState::Abandoned;
            goals[idx].last_updated_at = Local::now().timestamp() as f64;
        }
        drop(goals);
        let _ = self.persist();
    }

    /// 清空全部目标（恢复出厂设置 / 清空记忆时调用）
    pub fn clear_all(&self) -> VivianResult<()> {
        {
            let mut goals = self.goals.write();
            goals.clear();
        }
        self.persist()
    }

    fn persist(&self) -> VivianResult<()> {
        if let Some(parent) = self.persistence_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VivianError::Other(format!("创建目标目录失败: {}", e)))?;
        }
        let goals = self.goals.read();
        let json = serde_json::to_string_pretty(&*goals)
            .map_err(|e| VivianError::Other(format!("序列化目标失败: {}", e)))?;
        std::fs::write(&self.persistence_path, json)
            .map_err(|e| VivianError::Other(format!("写入目标文件失败: {}", e)))
    }

    fn load(path: &PathBuf) -> VivianResult<Vec<UserGoal>> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                if s.trim().is_empty() {
                    return Ok(Vec::new());
                }
                serde_json::from_str::<Vec<UserGoal>>(&s)
                    .map_err(|e| VivianError::Other(format!("解析目标文件失败: {}", e)))
            }
            Err(_) => Ok(Vec::new()),
        }
    }
}

/// 从 reflection LLM 输出的 goal_updates 字段解析出的目标操作
#[derive(Debug, Clone, Deserialize)]
pub struct GoalUpdateOp {
    /// "create" / "pause" / "complete" / "abandon" / "update_deadline"
    pub action: String,
    /// 目标标签（create 时必填，其他 action 用于匹配）
    #[serde(default)]
    pub label: Option<String>,
    /// ISO8601 截止时间字符串（如 "2026-12-25"），仅 create / update_deadline 使用
    #[serde(default)]
    pub deadline: Option<String>,
    /// 用户原话（create 且来源为 Dialogue 时必填）
    #[serde(default)]
    pub source_quote: Option<String>,
}

/// 解析 deadline 字符串为本地时间
///
/// 支持 "YYYY-MM-DD" / "YYYY-MM-DD HH:MM" / "YYYY-MM-DDTHH:MM:SS" 三种格式。
pub fn parse_deadline(s: &str) -> Option<DateTime<Local>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // 尝试多种常见格式
    let formats = [
        "%Y-%m-%d",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y/%m/%d",
        "%Y/%m/%d %H:%M",
    ];
    for fmt in &formats {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Local.from_local_datetime(&dt).single();
        }
        if *fmt == "%Y-%m-%d" || *fmt == "%Y/%m/%d" {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
                let dt = d.and_hms_opt(23, 59, 0).unwrap_or_default();
                return Local.from_local_datetime(&dt).single();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dialogue_source() -> UserGoalSource {
        UserGoalSource::Dialogue {
            quote: "我明年要考研".to_string(),
            extracted_at: 0.0,
        }
    }

    #[test]
    fn test_create_and_active_goals() {
        let ledger = UserGoalLedger::new(PathBuf::from(":memory:"));
        let id = ledger.create("准备考研", None, make_dialogue_source());
        assert!(!id.is_empty());
        let active = ledger.active_goals();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].label, "准备考研");
    }

    #[test]
    fn test_duplicate_create_skipped() {
        let ledger = UserGoalLedger::new(PathBuf::from(":memory:"));
        let id1 = ledger.create("准备考研", None, make_dialogue_source());
        assert!(!id1.is_empty());
        let id2 = ledger.create("准备考研", None, make_dialogue_source());
        assert!(id2.is_empty(), "重复创建应返回空 id");
        assert_eq!(ledger.active_goals().len(), 1);
    }

    #[test]
    fn test_transition_state_by_label() {
        let ledger = UserGoalLedger::new(PathBuf::from(":memory:"));
        ledger.create("准备考研", None, make_dialogue_source());
        assert!(ledger.transition_state("考研", UserGoalState::Completed));
        let active = ledger.active_goals();
        assert!(active.is_empty(), "Completed 后不应出现在 active_goals");
    }

    #[test]
    fn test_parse_deadline_formats() {
        assert!(parse_deadline("2026-12-25").is_some());
        assert!(parse_deadline("2026-12-25 14:30").is_some());
        assert!(parse_deadline("2026/12/25").is_some());
        assert!(parse_deadline("").is_none());
        assert!(parse_deadline("invalid").is_none());
    }

    #[test]
    fn test_active_briefs_sorted_by_deadline() {
        let ledger = UserGoalLedger::new(PathBuf::from(":memory:"));
        let near = parse_deadline("2026-08-01").unwrap();
        let far = parse_deadline("2026-12-25").unwrap();
        ledger.create("近期考试", Some(near), make_dialogue_source());
        ledger.create("远期目标", Some(far), make_dialogue_source());
        let briefs = ledger.active_briefs(3);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].label, "近期考试");
        assert!(briefs[0].days_to_deadline.unwrap() < briefs[1].days_to_deadline.unwrap());
    }
}
