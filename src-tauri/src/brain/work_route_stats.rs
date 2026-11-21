//! work 侧路由结果统计：按「任务类 + 模型」记录人工验收结果，并与用量报表合并成成本视图。
//!
//! 只统计与建议，不自动改路由：静默换模型会让用户无法解释结果的变化。
//! 设计上自动生效只允许作用在 `routing_matrix` 为空的条目，而模型级归因还依赖
//! 供应商回显，暂不满足这个前提。

use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 采样门槛：少于这个验收次数的模型不参与「更便宜」建议。
pub const MIN_REVIEWS_FOR_SUGGESTION: u32 = 3;
/// 可接受的成功率下界：达不到就不建议换过去。
pub const MIN_ACCEPTANCE_FOR_SUGGESTION: f64 = 0.9;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRecord {
    pub task: String,
    pub model: String,
    #[serde(default)]
    pub accepted: u32,
    #[serde(default)]
    pub rejected: u32,
}

impl RouteRecord {
    pub fn reviews(&self) -> u32 {
        self.accepted.saturating_add(self.rejected)
    }

    pub fn acceptance(&self) -> Option<f64> {
        (self.reviews() > 0).then(|| self.accepted as f64 / self.reviews() as f64)
    }
}

/// 用量报表里的一行：某个任务在某模型上的请求数与 token 总量。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteCost {
    pub task: String,
    pub model: String,
    pub requests: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StatsSnapshot {
    #[serde(default)]
    records: Vec<RouteRecord>,
}

pub struct RouteStatsStore {
    path: Option<PathBuf>,
    snapshot: RwLock<StatsSnapshot>,
}

impl RouteStatsStore {
    pub fn new() -> Self {
        Self { path: None, snapshot: RwLock::new(StatsSnapshot::default()) }
    }

    pub fn persistent(path: PathBuf) -> Self {
        let loaded: StatsSnapshot = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        Self { path: Some(path), snapshot: RwLock::new(loaded) }
    }

    fn save_locked(&self, snapshot: &StatsSnapshot) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| error.to_string())?;
        crate::utils::fs::atomic_write(path, &bytes).map_err(|error| error.to_string())
    }

    pub fn records(&self) -> Vec<RouteRecord> {
        self.snapshot.read().records.clone()
    }

    /// 记一次人工验收结果。`accepted=false` 表示打回修改。
    pub fn record(&self, task: &str, model: &str, accepted: bool) {
        let task = task.trim();
        let model = if model.trim().is_empty() { "default" } else { model.trim() };
        if task.is_empty() {
            return;
        }
        let mut snapshot = self.snapshot.write();
        let record = match snapshot
            .records
            .iter_mut()
            .find(|record| record.task == task && record.model == model)
        {
            Some(record) => record,
            None => {
                snapshot.records.push(RouteRecord {
                    task: task.to_string(),
                    model: model.to_string(),
                    accepted: 0,
                    rejected: 0,
                });
                snapshot.records.last_mut().expect("just pushed")
            }
        };
        if accepted {
            record.accepted = record.accepted.saturating_add(1);
        } else {
            record.rejected = record.rejected.saturating_add(1);
        }
        let _ = self.save_locked(&snapshot);
    }
}

impl Default for RouteStatsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 把一个任务下的验收记录与用量成本合并成报表行，并给出「更便宜的同类模型」建议。
///
/// 建议只在采样足够、成功率达标、且确实更便宜时才出现；它不改变任何路由配置。
pub fn summarize(records: &[RouteRecord], costs: &[RouteCost]) -> Vec<serde_json::Value> {
    let mut tasks: Vec<String> = Vec::new();
    for record in records {
        if !tasks.contains(&record.task) {
            tasks.push(record.task.clone());
        }
    }
    for cost in costs {
        if !tasks.contains(&cost.task) {
            tasks.push(cost.task.clone());
        }
    }
    tasks.sort();

    tasks
        .into_iter()
        .map(|task| {
            let task_records: Vec<&RouteRecord> = records.iter().filter(|r| r.task == task).collect();
            let task_costs: Vec<&RouteCost> = costs.iter().filter(|c| c.task == task).collect();
            let accepted: u32 = task_records.iter().map(|r| r.accepted).sum();
            let rejected: u32 = task_records.iter().map(|r| r.rejected).sum();
            let reviews = accepted + rejected;

            let models: Vec<serde_json::Value> = task_costs
                .iter()
                .map(|cost| {
                    let record = task_records.iter().find(|record| record.model == cost.model);
                    serde_json::json!({
                        "model": cost.model,
                        "requests": cost.requests,
                        "tokens": cost.tokens,
                        "tokens_per_request": (cost.requests > 0).then(|| cost.tokens / cost.requests),
                        "accepted": record.map(|r| r.accepted).unwrap_or(0),
                        "rejected": record.map(|r| r.rejected).unwrap_or(0),
                    })
                })
                .collect();

            serde_json::json!({
                "task": task,
                "accepted": accepted,
                "rejected": rejected,
                "acceptance": (reviews > 0).then(|| accepted as f64 / reviews as f64),
                "models": models,
                "suggestion": cheaper_suggestion(&task_records, &task_costs),
            })
        })
        .collect()
}

/// 在同类任务里挑一个「成功率达标且每次请求更省」的模型。没有结论时返回 null。
fn cheaper_suggestion(records: &[&RouteRecord], costs: &[&RouteCost]) -> Option<serde_json::Value> {
    let mut candidates: Vec<(String, u64, f64)> = Vec::new();
    for cost in costs {
        if cost.requests == 0 {
            continue;
        }
        let Some(record) = records.iter().find(|record| record.model == cost.model) else { continue };
        if record.reviews() < MIN_REVIEWS_FOR_SUGGESTION {
            continue;
        }
        let Some(acceptance) = record.acceptance() else { continue };
        if acceptance < MIN_ACCEPTANCE_FOR_SUGGESTION {
            continue;
        }
        candidates.push((cost.model.clone(), cost.tokens / cost.requests, acceptance));
    }
    if candidates.len() < 2 {
        return None;
    }
    let cheapest = candidates
        .iter()
        .min_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)))?;
    let current = candidates
        .iter()
        .max_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)))?;
    if cheapest.0 == current.0 || cheapest.1 >= current.1 {
        return None;
    }
    Some(serde_json::json!({
        "model": cheapest.0,
        "tokens_per_request": cheapest.1,
        "acceptance": cheapest.2,
        "replaces": current.0,
        "saves_tokens_per_request": current.1.saturating_sub(cheapest.1),
        "apply": "manual",
    }))
}

/// 从现有路由配置里挑一个「同模型、已配好」的条目作为模板。
///
/// 采纳建议时只复制已有配置（凭证与端点一起带过来），不凭空构造新供应商；
/// 目标条目自身与空模型条目都不参与。
pub fn donor_task<'a, I>(entries: I, model: &str, target: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    entries
        .into_iter()
        .find(|(task, entry_model)| {
            *task != target && !entry_model.trim().is_empty() && entry_model.trim() == model
        })
        .map(|(task, _)| task)
}

#[cfg(not(test))]
static ROUTE_STATS: once_cell::sync::Lazy<std::sync::Arc<RouteStatsStore>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::Arc::new(RouteStatsStore::persistent(
            crate::utils::path::get_user_data_dir().join("work_route_stats.json"),
        ))
    });

#[cfg(test)]
static ROUTE_STATS: once_cell::sync::Lazy<std::sync::Arc<RouteStatsStore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(RouteStatsStore::new()));

pub fn global_route_stats() -> std::sync::Arc<RouteStatsStore> {
    std::sync::Arc::clone(&ROUTE_STATS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(task: &str, model: &str, accepted: u32, rejected: u32) -> RouteRecord {
        RouteRecord { task: task.into(), model: model.into(), accepted, rejected }
    }

    #[test]
    fn recording_accumulates_per_task_and_model() {
        let store = RouteStatsStore::new();
        store.record("work_agent", "model-a", true);
        store.record("work_agent", "model-a", false);
        store.record("work_agent", "model-b", true);
        store.record("work_memory", "model-a", true);
        store.record("", "model-a", true);
        let rows = store.records();
        assert_eq!(rows.len(), 3, "空任务名被忽略");
        let a = rows.iter().find(|r| r.task == "work_agent" && r.model == "model-a").unwrap();
        assert_eq!((a.accepted, a.rejected), (1, 1));
        assert_eq!(a.acceptance(), Some(0.5));
    }

    #[test]
    fn summarize_joins_acceptance_with_cost() {
        let rows = summarize(
            &[record("work_agent", "cheap", 4, 0), record("work_agent", "pricey", 4, 0)],
            &[
                RouteCost { task: "work_agent".into(), model: "cheap".into(), requests: 10, tokens: 10_000 },
                RouteCost { task: "work_agent".into(), model: "pricey".into(), requests: 10, tokens: 90_000 },
            ],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["accepted"], 8);
        assert_eq!(rows[0]["models"][0]["tokens_per_request"], 1_000);
        assert_eq!(rows[0]["suggestion"]["model"], "cheap");
        assert_eq!(rows[0]["suggestion"]["replaces"], "pricey");
    }

    #[test]
    fn donor_selection_requires_a_configured_entry_with_the_same_model() {
        let entries = [
            ("work_agent", ""),
            ("companion", "model-a"),
            ("judge", "model-a"),
            ("memory", "model-b"),
        ];
        assert_eq!(donor_task(entries, "model-a", "work_agent"), Some("companion"));
        assert_eq!(donor_task(entries, "model-b", "work_agent"), Some("memory"));
        assert_eq!(donor_task(entries, "missing", "work_agent"), None);
        assert_eq!(donor_task(entries, "  ", "work_agent"), None);
        // 目标条目自身不能当模板，空模型也不算。
        assert_eq!(donor_task(entries, "model-a", "companion"), Some("judge"));
        assert_eq!(donor_task([("work_agent", "")], "model-a", "work_agent"), None);
    }

    #[test]
    fn suggestions_need_samples_and_a_good_record() {
        // 采样太少：不给建议
        let rows = summarize(
            &[record("work_agent", "cheap", 1, 0), record("work_agent", "pricey", 5, 0)],
            &[
                RouteCost { task: "work_agent".into(), model: "cheap".into(), requests: 5, tokens: 500 },
                RouteCost { task: "work_agent".into(), model: "pricey".into(), requests: 5, tokens: 5_000 },
            ],
        );
        assert!(rows[0]["suggestion"].is_null());

        // 便宜但成功率低：不给建议
        let rows = summarize(
            &[record("work_agent", "cheap", 1, 5), record("work_agent", "pricey", 6, 0)],
            &[
                RouteCost { task: "work_agent".into(), model: "cheap".into(), requests: 6, tokens: 600 },
                RouteCost { task: "work_agent".into(), model: "pricey".into(), requests: 6, tokens: 6_000 },
            ],
        );
        assert!(rows[0]["suggestion"].is_null());

        // 只有单一模型：无需建议
        let rows = summarize(
            &[record("work_agent", "only", 5, 0)],
            &[RouteCost { task: "work_agent".into(), model: "only".into(), requests: 5, tokens: 5_000 }],
        );
        assert!(rows[0]["suggestion"].is_null());
    }
}
