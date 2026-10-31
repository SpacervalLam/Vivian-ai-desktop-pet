//! Host-observed step outcomes and a whole-request settlement for companion execution.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const FINISH_TOOL: &str = "finish_companion_task";

#[derive(Clone, Serialize)]
pub(crate) struct StepOutcome {
    pub call_id: String,
    pub tool: String,
    pub success: bool,
    pub step_completed: bool,
    pub pending: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskStatus { Completed, Partial, Blocked, Waiting }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settlement {
    pub status: TaskStatus,
    pub remaining_work: Vec<String>,
    pub evidence_ids: Vec<String>,
}

pub(crate) struct ExecutionTask {
    goal: String,
    outcomes: Vec<StepOutcome>,
    pub settlement: Option<Settlement>,
}

impl ExecutionTask {
    pub(crate) fn new(goal: &str) -> Self { Self { goal: goal.into(), outcomes: vec![], settlement: None } }
    pub(crate) fn record(&mut self, outcome: StepOutcome) { self.outcomes.push(outcome); }
    pub(crate) fn waiting(&self) -> bool { self.outcomes.iter().any(|outcome| outcome.pending) }
    pub(crate) fn snapshot(&self, rounds: usize, budget: usize) -> Value {
        let recent = &self.outcomes[self.outcomes.len().saturating_sub(32)..];
        json!({"goal":self.goal,"rounds_used":rounds,"rounds_remaining":budget.saturating_sub(rounds),
            "recent_step_outcomes":recent,"pending_work":self.outcomes.iter().filter(|o|o.pending).collect::<Vec<_>>(),
            "settlement":self.settlement,
            "scope":include_str!("../../prompts/execution/task-snapshot-boundary.en.md")})
    }
    pub(crate) fn settle(&mut self, value: Value) -> Result<(), String> {
        let result: Settlement = serde_json::from_value(value).map_err(|error| format!("Invalid settlement: {error}"))?;
        if result.remaining_work.len() > 32 || result.evidence_ids.len() > 128
            || result.remaining_work.iter().any(|item| item.trim().is_empty() || item.chars().count() > 2000) {
            return Err("Keep remaining_work bounded and nonempty per item".into());
        }
        for id in &result.evidence_ids {
            let mut matches = self.outcomes.iter().filter(|outcome| outcome.call_id == *id);
            if !matches.next().is_some_and(|outcome| outcome.success) || matches.next().is_some() {
                return Err(format!("Evidence must reference an actual successful receipt: {id}"));
            }
        }
        if result.status == TaskStatus::Completed {
            if !result.remaining_work.is_empty() || self.waiting() || result.evidence_ids.is_empty() {
                return Err("Completed requires successful evidence, no remaining_work and no pending background work".into());
            }
        } else if result.remaining_work.is_empty() {
            return Err("An incomplete task must describe remaining_work or the blocker".into());
        }
        if result.status == TaskStatus::Waiting && !self.waiting() {
            return Err("Waiting requires an actual pending background receipt".into());
        }
        self.settlement = Some(result);
        Ok(())
    }
    pub(crate) fn stop(&mut self, status: TaskStatus) {
        if self.settlement.is_none() {
            self.settlement = Some(Settlement { status, remaining_work: vec![self.goal.clone()], evidence_ids: vec![] });
        }
    }
}

pub(crate) fn finish_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{
        "status":{"type":"string","enum":["completed","partial","blocked","waiting"]},
        "remaining_work":{"type":"array","maxItems":32,"items":{"type":"string","maxLength":2000}},
        "evidence_ids":{"type":"array","maxItems":128,"items":{"type":"string"}}
    },"required":["status","remaining_work","evidence_ids"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_requires_whole_request_settlement_and_actual_evidence() {
        let mut task = ExecutionTask::new("Find a wallpaper, exclude the current one, then apply it");
        task.record(StepOutcome { call_id:"lookup".into(),tool:"wallpaper_list".into(),success:true,step_completed:true,pending:false });
        assert!(task.settlement.is_none(), "a tool's completion marker cannot settle the whole goal");
        assert!(task.settle(json!({"status":"completed","remaining_work":["apply it"],"evidence_ids":["lookup"]})).is_err());
        assert!(task.settle(json!({"status":"completed","remaining_work":[],"evidence_ids":["invented"]})).is_err());
        assert!(task.settle(json!({"status":"waiting","remaining_work":["apply it"],"evidence_ids":["lookup"]})).is_err());
        task.record(StepOutcome { call_id:"apply".into(),tool:"wallpaper_set".into(),success:true,step_completed:true,pending:false });
        task.settle(json!({"status":"completed","remaining_work":[],"evidence_ids":["lookup","apply"]})).unwrap();
        assert_eq!(task.settlement.unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn background_launch_is_waiting_and_snapshot_preserves_goal_after_history_trimming() {
        let mut task = ExecutionTask::new("Create the requested slides with sources");
        task.record(StepOutcome { call_id:"job".into(),tool:"delegate_to_work_agent".into(),success:true,step_completed:true,pending:true });
        assert!(task.settle(json!({"status":"completed","remaining_work":[],"evidence_ids":["job"]})).is_err());
        task.stop(TaskStatus::Waiting);
        let snapshot = task.snapshot(1, 6);
        assert_eq!(snapshot["goal"], "Create the requested slides with sources");
        assert_eq!(snapshot["settlement"]["status"], "waiting");
        assert_eq!(snapshot["pending_work"][0]["call_id"], "job");
    }
}
