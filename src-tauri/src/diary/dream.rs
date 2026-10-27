//! Production adapter for the daily dream → diary cycle. Dreaming creates no fictional facts.
use super::{
    cycle::{day_bounds, CycleWork, DreamContext},
    DiaryEntry,
};
use crate::{brain::Brain, error::VivianResult, memory::consolidation::DreamReport};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde_json::json;

pub async fn run(brain: &Brain, date: NaiveDate, trigger: &str) -> VivianResult<DiaryEntry> {
    let _activity =
        crate::presence::activity_budget::try_acquire(&brain.char_id).ok_or_else(|| {
            crate::error::VivianError::Memory("已有后台记忆任务，稍后再做梦与写日记".into())
        })?;
    super::cycle::cycle_for(&brain.char_id)
        .run(date, trigger, brain)
        .await
}

/// All program gates run before any diary-specific instruction reaches the environment model.
pub fn candidate(
    brain: &Brain,
    now: chrono::DateTime<chrono::Local>,
) -> VivianResult<Option<crate::world::environment_policy::DiaryCandidate>> {
    use chrono::Timelike;
    if !super::cycle::is_judgment_time(now.hour())
        || crate::presence::activity_budget::is_active(&brain.char_id)
    {
        return Ok(None);
    }
    let config = super::get_config(&brain.char_id)?;
    if !config.enable_auto_diary {
        return Ok(None);
    }
    let threshold = config.min_interaction_threshold.max(1);
    let cycle = super::cycle::cycle_for(&brain.char_id);
    let dates = cycle
        .pending_dates(now.timestamp())?
        .into_iter()
        .filter(|date| *date <= now.date_naive())
        .map(|date| (date, true))
        .chain(std::iter::once((now.date_naive(), false)));
    let mut seen = std::collections::HashSet::new();
    for (date, resume) in dates {
        if !seen.insert(date) || !cycle.ready(date, now.timestamp())? {
            continue;
        }
        if !super::get_entries(&brain.char_id, Some(&date.to_string()))?.is_empty() {
            if let Err(error) = cycle.acknowledge_existing(date) {
                tracing::warn!("[DreamDiary] 已写日记的进度收尾失败: {error}");
            }
            continue;
        }
        let (after, before) = day_bounds(date)?;
        let count = brain
            .memory
            .count_diary_material(after as f64, before as f64, threshold);
        if !super::cycle::may_offer_to_planner(now.hour(), count, threshold) {
            continue;
        }
        return Ok(Some(crate::world::environment_policy::DiaryCandidate {
            character_id: brain.char_id.clone(),
            date: date.to_string(),
            resume,
            reason: if resume {
                "未完成的日记安排，原始素材与时间窗均满足，可考虑从检查点继续".into()
            } else {
                "原始素材达到阈值且位于允许时间窗，由情境决定是否安排做梦与日记".into()
            },
            material_count_at_least: count,
            material_threshold: threshold,
        }));
    }
    Ok(None)
}

/// Called only after a grounded environment-agent selection. Foreground input preempts this work.
pub fn spawn_selected(
    brain: Brain,
    selected: crate::world::environment_policy::DiaryCandidate,
    priority: std::sync::Arc<crate::utils::background_priority::BackgroundPriority>,
    revision: u64,
    app: Option<tauri::AppHandle>,
    char_name: String,
) {
    tokio::spawn(async move {
        if priority.revision() != revision || !priority.is_idle() {
            return;
        }
        let Ok(date) = NaiveDate::parse_from_str(&selected.date, "%Y-%m-%d") else {
            return;
        };
        if !matches!(candidate(&brain, chrono::Local::now()), Ok(Some(ref fresh)) if fresh.date==selected.date && fresh.resume==selected.resume)
        {
            return;
        }
        super::set_tool_availability(&brain.char_id, false);
        let cancel = crate::utils::cancel_token::cancel_token();
        let result = tokio::select! {
            _=cancel.cancelled()=>return,
            result=priority.run(run(&brain,date,"auto"))=>result,
        };
        match result {
            Some(Ok(entry)) => {
                use tauri::Emitter;
                tracing::info!(character_id=%brain.char_id,%date,"[DreamDiary] 环境智能体发起的流程完成");
                if let Some(app) = app {
                    let _=app.emit("diary:written",json!({"character_id":brain.char_id,"character_name":char_name,"date":entry.date}));
                }
            }
            Some(Err(error)) => {
                tracing::warn!(character_id=%brain.char_id,"[DreamDiary] 保留进度供环境智能体下次调度: {error}")
            }
            None => tracing::debug!("[DreamDiary] 前台活动优先，保留检查点"),
        }
    });
}

#[async_trait]
impl CycleWork for Brain {
    fn existing(&self, date: NaiveDate) -> VivianResult<Option<DiaryEntry>> {
        Ok(super::get_entries(&self.char_id, Some(&date.to_string()))?
            .into_iter()
            .next())
    }
    async fn dream(&self, date: NaiveDate) -> VivianResult<DreamContext> {
        let (after, before) = day_bounds(date)?;
        let began_at = chrono::Utc::now().timestamp();
        let report = match &self.consolidator {
            Some(consolidator) => {
                consolidator
                    .before_diary(after as f64, before as f64)
                    .await?
            }
            None => DreamReport {
                disabled: true,
                ..Default::default()
            },
        };
        if report.disabled {
            return Ok(DreamContext {
                report,
                material: Vec::new(),
            });
        }
        let mut summaries = self.memory.consolidation_summaries(&[])?;
        summaries.retain(|m| {
            m.metadata["started_at"]
                .as_f64()
                .is_some_and(|start| start >= after as f64)
                && m.metadata["ended_at"]
                    .as_f64()
                    .is_some_and(|end| end < before as f64)
                && !m.content.trim().is_empty()
        });
        summaries.sort_by(|a, b| {
            b.metadata["ended_at"]
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&a.metadata["ended_at"].as_f64().unwrap_or_default())
        });
        let mut material: Vec<_> = summaries
            .iter()
            .take(4)
            .map(|m| {
                json!({
                    "kind":"derived_session_summary", "memory_id":m.id,
                    "source_message_ids":m.metadata["source_message_ids"].as_array().map(|ids|ids.iter().take(32).cloned().collect::<Vec<_>>()).unwrap_or_default(),
                    "text":m.content.chars().take(600).collect::<String>(),
                })
            })
            .collect();
        let beliefs = self.mind.beliefs.read();
        material.extend(
            beliefs
                .top_n_by_confidence(20)
                .into_iter()
                .filter(|b| b.last_reinforced_at >= began_at && !b.source_memory_ids.is_empty())
                .take(3)
                .map(|b| {
                    json!({"kind":"interpretation", "belief_id":b.id,
                "source_memory_ids":b.source_memory_ids,"confidence":b.confidence,
                "text":b.statement.chars().take(240).collect::<String>()})
                }),
        );
        drop(beliefs);
        material.extend(self.mind.thought_threads_snapshot(chrono::Utc::now().timestamp()).into_iter()
            .filter(|thread| thread.created_at < before && thread.updated_at >= after && thread.updated_at < before)
            .map(|thread| json!({"kind":"private_thought_thread","subjective":true,"thread":thread})));
        Ok(DreamContext { report, material })
    }
    async fn diary(
        &self,
        date: NaiveDate,
        trigger: &str,
        dream: &DreamContext,
    ) -> VivianResult<DiaryEntry> {
        super::intelligent_generator::write_after_dream(self, date, trigger, dream).await
    }
}
