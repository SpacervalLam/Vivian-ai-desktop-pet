//! 计划模式（plan mode）—— plan-then-approve-then-execute。
//!
//! 让模型在动手前先产出一份可执行计划（步骤列表），交给用户审阅；用户批准后
//! 模型才继续执行。用于高风险/多步骤任务的渐进式协作，避免"一上来就改一堆文件"。
//!
//! 状态机：`Draft`（模型产出计划）→ `AwaitingApproval`（等待用户批准）→
//! `Approved`（用户批准，可执行）→ `Executed` / `Rejected`（用户否决）。

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 计划状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// 模型已产出计划，等待用户批准
    AwaitingApproval,
    /// 用户已批准，可执行
    Approved,
    /// 用户否决
    Rejected,
    /// 已执行完成
    Executed,
}

/// 计划中的单个步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// 步骤序号
    pub index: usize,
    /// 步骤描述（做什么）
    pub description: String,
    /// 期望的工具（可选提示，如 read_file / edit_file）
    pub tool_hint: Option<String>,
}

/// 一份完整计划。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// 计划 ID
    pub plan_id: String,
    /// 角色 ID
    pub char_id: String,
    /// 计划主题/目标
    pub objective: String,
    /// 步骤列表
    pub steps: Vec<PlanStep>,
    /// 当前状态
    pub status: PlanStatus,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

/// 计划服务：管理角色的进行中计划。
#[derive(Clone)]
pub struct PlanService {
    plans: Arc<RwLock<BTreeMap<String, Plan>>>,
}

impl PlanService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            plans: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// 创建一个待批准计划（status = AwaitingApproval）。
    pub fn create_plan(
        &self,
        char_id: impl Into<String>,
        objective: impl Into<String>,
        steps: Vec<PlanStep>,
    ) -> Plan {
        let now = chrono::Local::now().timestamp();
        let plan = Plan {
            plan_id: format!("plan-{}", uuid::Uuid::new_v4().simple()),
            char_id: char_id.into(),
            objective: objective.into(),
            steps,
            status: PlanStatus::AwaitingApproval,
            created_at: now,
            decided_at: None,
        };
        self.plans.write().insert(plan.plan_id.clone(), plan.clone());
        plan
    }

    /// 批准计划（AwaitingApproval → Approved）。
    pub fn approve(&self, plan_id: &str) -> bool {
        self.decide(plan_id, PlanStatus::Approved)
    }

    /// 否决计划（AwaitingApproval → Rejected）。
    pub fn reject(&self, plan_id: &str) -> bool {
        self.decide(plan_id, PlanStatus::Rejected)
    }

    fn decide(&self, plan_id: &str, status: PlanStatus) -> bool {
        let mut plans = self.plans.write();
        match plans.get_mut(plan_id) {
            Some(p) if p.status == PlanStatus::AwaitingApproval => {
                p.status = status;
                p.decided_at = Some(chrono::Local::now().timestamp());
                true
            }
            _ => false,
        }
    }

    /// 标记已执行完成。
    pub fn mark_executed(&self, plan_id: &str) -> bool {
        let mut plans = self.plans.write();
        match plans.get_mut(plan_id) {
            Some(p) if p.status == PlanStatus::Approved => {
                p.status = PlanStatus::Executed;
                p.decided_at = Some(chrono::Local::now().timestamp());
                true
            }
            _ => false,
        }
    }

    /// 查询计划。
    pub fn get(&self, plan_id: &str) -> Option<Plan> {
        self.plans.read().get(plan_id).cloned()
    }

    /// 某角色待批准的计划。
    pub fn pending_for(&self, char_id: &str) -> Vec<Plan> {
        self.plans
            .read()
            .values()
            .filter(|p| p.char_id == char_id && p.status == PlanStatus::AwaitingApproval)
            .cloned()
            .collect()
    }
}
