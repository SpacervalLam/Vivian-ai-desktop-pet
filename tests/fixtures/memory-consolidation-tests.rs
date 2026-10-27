use crate::{config::manager::ConsolidationConfig, dialogue::{DialogueManager, HistoryEntry},
    memory::{conversations::build_conversations, manager::MemoryManager,
        pipeline::ConsolidationPipeline}, providers::ModelRouter};
use std::sync::{Arc, atomic::Ordering};

fn row(id: &str, time: f64, text: &str, boundary: bool) -> HistoryEntry {
    HistoryEntry { id: id.into(), role: "user".into(), content: text.into(), timestamp: time,
        session_id: Some("runtime".into()), metadata: if boundary {
            serde_json::json!({"conversation_boundary":"new"})
        } else { serde_json::json!({}) } }
}

fn pipeline(history: Arc<DialogueManager>, router: Arc<ModelRouter>) -> ConsolidationPipeline {
    ConsolidationPipeline::new(router, ConsolidationConfig {
        stage1_idle_timeout_sec: 1.0, stage1_short_term_threshold: 3,
    }, history)
}

#[tokio::test]
async fn idle_pass_reads_one_history_snapshot_and_never_clones_all_memories() {
    let history = Arc::new(DialogueManager::new(vec![row("a", 1.0, "你好", false)]));
    let router = Arc::new(ModelRouter::default()); let memory = MemoryManager::default();
    assert_eq!(pipeline(history.clone(), router.clone()).run(&memory).await.unwrap().stage1_summaries, 0);
    assert_eq!(history.history_reads.load(Ordering::Relaxed), 1);
    assert_eq!(memory.full_reads.load(Ordering::Relaxed), 0);
    assert_eq!(memory.summary_reads.load(Ordering::Relaxed), 1);
    assert_eq!(memory.index_checks.load(Ordering::Relaxed), 0);
    assert!(router.prompts.lock().is_empty());
}

fn backlog() -> Vec<HistoryEntry> {
    (0..3).flat_map(|session| (0..3).map(move |turn| {
        row(&format!("{session}-{turn}"), 1.0 + (session * 10 + turn) as f64,
            &format!("第{session}次交流第{turn}条原文"), turn == 0)
    })).collect()
}

#[tokio::test]
async fn automatic_budget_drains_oldest_backlog_first() {
    let history = Arc::new(DialogueManager::new(backlog()));
    let sessions = build_conversations(&history.entries.read(), "vivian");
    let router = Arc::new(ModelRouter::default()); let memory = MemoryManager::default();
    assert_eq!(pipeline(history, router.clone()).run(&memory).await.unwrap().stage1_summaries, 2);
    let all = memory.items.read();
    assert_eq!(all[0].metadata["conversation_id"], sessions[0].id);
    assert_eq!(all[1].metadata["conversation_id"], sessions[1].id);
    assert_eq!(router.prompts.lock().len(), 2);
    assert_eq!(memory.summary_reads.load(Ordering::Relaxed), 1);
    assert_eq!(memory.full_reads.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn malformed_session_preserves_budget_for_another_session() {
    let history = Arc::new(DialogueManager::new(backlog()));
    let sessions = build_conversations(&history.entries.read(), "vivian");
    let router = Arc::new(ModelRouter::default()); router.outputs.lock().push_back("bad-json".into());
    let memory = MemoryManager::default();
    assert_eq!(pipeline(history, router.clone()).run(&memory).await.unwrap().stage1_summaries, 1);
    assert_eq!(memory.items.read()[0].metadata["conversation_id"], sessions[1].id);
    assert_eq!(router.prompts.lock().len(), 2);
}

#[tokio::test]
async fn edits_during_generation_never_advance_the_checkpoint() {
    for delete in [false, true] {
        let history = Arc::new(DialogueManager::new(vec![row("a", 1.0, "明天一起完成项目", false)]));
        let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
        let router = Arc::new(ModelRouter::default()); let edited = history.clone();
        router.hooks.lock().push_back(Box::new(move || {
            if delete { edited.entries.write().clear(); }
            else { edited.entries.write()[0].content = "计划取消了".into(); }
        }));
        let memory = MemoryManager::default();
        assert!(pipeline(history, router).summarize_session(&memory, &id).await.is_err());
        assert!(memory.items.read().is_empty());
    }
}

#[tokio::test]
async fn boundary_epoch_change_revalidates_sources_even_when_history_is_unchanged() {
    let history = Arc::new(DialogueManager::new(vec![row("a", 1.0, "明天一起完成项目", false)]));
    let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
    let router = Arc::new(ModelRouter::default()); let changed = history.clone();
    router.hooks.lock().push_back(Box::new(move || { changed.conversation_boundaries.clear().unwrap(); }));
    let memory = MemoryManager::default();
    pipeline(history.clone(), router).summarize_session(&memory, &id).await.unwrap();
    assert_eq!(history.history_reads.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn source_changes_keep_the_last_successful_partial_checkpoint() {
    let history = Arc::new(DialogueManager::new(vec![row("long", 1.0, &"原始🙂".repeat(10_000), false)]));
    let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
    let router = Arc::new(ModelRouter::default()); let edited = history.clone();
    router.hooks.lock().push_back(Box::new(|| {}));
    router.hooks.lock().push_back(Box::new(move || { edited.entries.write()[0].metadata["speaker"] = "nana".into(); }));
    let memory = MemoryManager::default();
    assert!(pipeline(history, router.clone()).summarize_session(&memory, &id).await.is_err());
    assert_eq!(memory.items.read()[0].metadata["completed_parts"], 1);
    assert_eq!(memory.items.read()[0].metadata["summary_status"], "partial");
    assert_eq!(router.prompts.lock().len(), 2);
}

#[tokio::test]
async fn appended_turn_is_not_claimed_by_the_in_flight_summary() {
    let history = Arc::new(DialogueManager::new(vec![row("a", 1.0, "明天一起完成项目", false)]));
    let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
    let router = Arc::new(ModelRouter::default()); let appended = history.clone();
    router.hooks.lock().push_back(Box::new(move || {
        appended.entries.write().push(row("b", 2.0, "后来追加的内容", false));
    }));
    let memory = MemoryManager::default(); let pipeline = pipeline(history, router.clone());
    let first = pipeline.summarize_session(&memory, &id).await.unwrap();
    assert_eq!(first.metadata["source_message_ids"], serde_json::json!(["a"]));
    let next = pipeline.summarize_session(&memory, &id).await.unwrap();
    assert_eq!(next.metadata["source_message_ids"], serde_json::json!(["a","b"]));
    assert_eq!(router.prompts.lock().len(), 2);
}

#[tokio::test]
async fn partially_covered_long_turn_still_counts_toward_the_incremental_threshold() {
    let now = crate::memory::types::current_timestamp();
    let history = Arc::new(DialogueManager::new(vec![
        row("long", now - 2.0, &"原始🙂".repeat(10_000), false),
        row("b", now - 1.0, "第二条原文", false), row("c", now, "第三条原文", false),
    ]));
    let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
    let router = Arc::new(ModelRouter::default()); router.outputs.lock().extend([
        r#"{"title":"交流","summary":"第一段完成","topics":[],"importance":0.7,"events":[]}"#.into(), "bad-json".into(),
    ]);
    let memory = MemoryManager::default();
    let pipeline = ConsolidationPipeline::new(router.clone(), ConsolidationConfig {
        stage1_idle_timeout_sec: 3600.0, stage1_short_term_threshold: 3,
    }, history);
    assert!(pipeline.summarize_session(&memory, &id).await.is_err());
    assert_eq!(memory.items.read()[0].metadata["completed_parts"], 1);
    assert_eq!(pipeline.run(&memory).await.unwrap().stage1_summaries, 1);
    assert_eq!(memory.items.read()[0].metadata["completed_parts"], 3);
    assert_eq!(router.prompts.lock().len(), 4);
}

#[tokio::test]
async fn event_catalogue_refreshes_between_sessions_within_one_pass() {
    let history = Arc::new(DialogueManager::new(vec![
        row("plan", 1.0, "周末一起完成桌宠项目吧", false), row("a", 2.0, "约好了", false), row("b", 3.0, "好呀", false),
        row("done", 10.0, "桌宠项目今天完成了", true), row("c", 11.0, "谢谢陪伴", false), row("d", 12.0, "记住今天吧", false),
    ]));
    let event_id = crate::memory::session_summary::summary_id("event", "plan:完成桌宠项目");
    let router = Arc::new(ModelRouter::default());
    for (existing, message, quote, phase) in [
        (serde_json::Value::Null, "plan", "一起完成桌宠项目", "planned"),
        (serde_json::json!(event_id), "done", "桌宠项目今天完成了", "completed"),
    ] {
        router.outputs.lock().push_back(serde_json::json!({"title":"桌宠项目","summary":"项目进展","topics":[],"importance":0.8,
            "events":[{"existing_event_id":existing,"title":"完成桌宠项目","phase":phase,"detail":quote,
                "source_message_id":message,"source_quote":quote,"confidence":0.96}]}).to_string());
    }
    let memory = MemoryManager::default();
    assert_eq!(pipeline(history, router.clone()).run(&memory).await.unwrap().stage1_summaries, 2);
    assert_eq!(memory.items.read()[1].metadata["summary_parts"][0]["events"][0]["id"], event_id);
    assert!(router.prompts.lock()[1].contains(&event_id));
}

#[tokio::test]
async fn before_diary_bypasses_chat_backoff_and_scopes_the_day() {
    let history = Arc::new(DialogueManager::new(vec![row("a", 10.0, "明天一起完成项目", false)]));
    let router = Arc::new(ModelRouter::default()); let memory = MemoryManager::default();
    let pipeline = pipeline(history.clone(), router.clone());
    assert_eq!(pipeline.run(&memory).await.unwrap().stage1_summaries, 0, "ordinary short conversations still retain originals only");
    assert_eq!(pipeline.run_before_diary(&memory, 20.0, 30.0).await.unwrap().stage1_summaries, 0);
    assert_eq!(pipeline.run_before_diary(&memory, 0.0, 20.0).await.unwrap().stage1_summaries, 1);
    history.entries.write()[0].content = "计划改到下周了".into();
    assert_eq!(pipeline.run(&memory).await.unwrap().stage1_summaries, 0);
    assert_eq!(pipeline.run_before_diary(&memory, 0.0, 20.0).await.unwrap().stage1_summaries, 1);
    assert_eq!(router.prompts.lock().len(), 2);
}

#[tokio::test]
async fn before_diary_reports_pending_work_when_the_budget_is_spent() {
    let history = Arc::new(DialogueManager::new(vec![row("long", 10.0, &"原始🙂".repeat(30_000), false)]));
    let router = Arc::new(ModelRouter::default()); let memory = MemoryManager::default();
    let pipeline = pipeline(history, router.clone());
    let first = pipeline.run_before_diary(&memory, 0.0, 20.0).await.unwrap();
    assert_eq!(router.prompts.lock().len(), 8); assert_eq!(first.pending_sessions, 1);
    assert_eq!(memory.items.read()[0].metadata["summary_status"], "partial");
    let second = pipeline.run_before_diary(&memory, 0.0, 20.0).await.unwrap();
    assert_eq!(second.pending_sessions, 0); assert_eq!(memory.items.read()[0].metadata["summary_status"], "complete");
}

#[tokio::test]
async fn before_diary_does_not_treat_uncertain_grouping_as_completed() {
    let history = Arc::new(DialogueManager::new(vec![row("a", 1.0, "周几见面", false), row("b", 30_000.0, "周五", false)]));
    let router = Arc::new(ModelRouter::default()); router.outputs.lock().push_back("bad-json".into());
    let memory = MemoryManager::default();
    assert!(pipeline(history, router).run_before_diary(&memory, 0.0, 40_000.0).await.is_err());
    assert!(memory.items.read().is_empty());
}
