use crate::{config::manager::ConsolidationConfig, dialogue::{DialogueManager, HistoryEntry},
    memory::{conversations::build_conversations, manager::MemoryManager,
        pipeline::ConsolidationPipeline, types::{Granularity, MemoryItem}}, providers::ModelRouter};
use std::{sync::{Arc, atomic::Ordering}, time::Instant};

#[tokio::test]
#[ignore = "synthetic consolidation benchmark; no provider or application data"]
async fn consolidation_benchmark() {
    let history = Arc::new(DialogueManager::new(vec![HistoryEntry {
        id: "long-source".into(), role: "user".into(), content: "原始🙂".repeat(10_000),
        timestamp: 1.0, session_id: Some("bench".into()), metadata: serde_json::json!({}),
    }]));
    let id = build_conversations(&history.entries.read(), "vivian")[0].id.clone();
    let facts: Vec<_> = (0..20_000).map(|i| {
        let mut item = MemoryItem::new("长期事实内容".repeat(40), Granularity::Summary, 0.7);
        item.id = format!("fact-{i}"); item.memory_type = "user".into();
        item.metadata = serde_json::json!({"record_kind":"fact","speaker":"user","known_by":["vivian"]}); item
    }).collect();
    let mut samples = Vec::new(); let mut counters = serde_json::Value::Null;
    for _ in 0..7 {
        let memory = MemoryManager::default(); *memory.items.write() = facts.clone();
        let router = Arc::new(ModelRouter::default());
        let pipeline = ConsolidationPipeline::new(router.clone(), ConsolidationConfig {
            stage1_idle_timeout_sec: 1.0, stage1_short_term_threshold: 3,
        }, history.clone());
        history.history_reads.store(0, Ordering::Relaxed);
        let started = Instant::now();
        let item = pipeline.summarize_session(&memory, &id).await.unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(item.metadata["summary_status"], "complete");
        counters = serde_json::json!({"full_memory_reads":memory.full_reads.load(Ordering::Relaxed),
            "summary_reads":memory.summary_reads.load(Ordering::Relaxed),
            "history_reads":history.history_reads.load(Ordering::Relaxed),
            "model_calls":router.prompts.lock().len(),"parts":item.metadata["completed_parts"]});
    }
    samples.sort_by(f64::total_cmp);
    println!("CONSOLIDATION_BENCH {}", serde_json::json!({"facts":facts.len(),"samples_ms":samples,
        "median_ms":samples[3],"counters":counters}));
}

#[test]
#[ignore = "compare reconciliation copy policies with unchanged evidence links"]
fn derived_links_benchmark() {
    use crate::memory::conversations::{refresh_derived_links, updated_derived_links};
    use std::collections::{HashMap, HashSet};
    let valid: HashSet<_> = ["conversation"].into_iter().collect();
    let owners: HashMap<_, _> = [("source", "conversation")].into_iter().collect();
    let rows: Vec<_> = (0..20_000).map(|i| {
        let mut item = MemoryItem::new("长期事实内容".repeat(40), Granularity::Summary, 0.7);
        item.id = format!("fact-{i}"); item.memory_type = "user".into();
        item.metadata = serde_json::json!({"record_kind":"fact","source_message_ids":["source"],
            "conversation_id":"conversation","evidence_sources":[{"message_ids":["source"],"conversation_id":"conversation"}]}); item
    }).collect();
    for clone_all in [true, false] {
        let mut samples = Vec::new(); let mut copied_bytes = 0;
        for _ in 0..7 {
            let started = Instant::now(); let mut copied = 0;
            for row in &rows {
                if clone_all {
                    let mut updated = row.clone(); copied += updated.content.len();
                    refresh_derived_links(&mut updated, &valid, &owners);
                    assert_eq!(updated.metadata, row.metadata);
                } else {
                    assert!(updated_derived_links(row, &valid, &owners).is_none());
                }
            }
            samples.push(started.elapsed().as_secs_f64() * 1000.0); copied_bytes = copied;
        }
        samples.sort_by(f64::total_cmp);
        println!("DERIVED_LINKS_BENCH {}", serde_json::json!({"clone_all":clone_all,"median_ms":samples[3],
            "samples_ms":samples,"copied_content_bytes":copied_bytes,"facts":rows.len()}));
    }
}
