//! The real manager lookup, persistent keyword index and fusion pipeline with
//! controlled semantic vectors. This checks wiring, not model relevance quality.
use super::*;
use crate::memory::agentic_recall::{recall_in_parallel, RecallQuery, RecallScope};
use crate::memory::vector_search::MemoryVector;

struct SemanticProvider { fail: bool }
impl MemoryEmbeddingProvider for SemanticProvider {
    fn dimension(&self) -> usize { 2 }
    fn model_id(&self) -> &str { "semantic-fixture" }
    fn embed(&self, _: &str) -> VivianResult<Vec<f32>> {
        if self.fail { Err(crate::error::VivianError::Memory("embedding unavailable".into())) }
        else { Ok(vec![1.0, 0.0]) }
    }
}

fn row(id: &str, content: &str, kind: &str, owner: &str) -> MemoryItem {
    let mut item = MemoryItem::new(content.into(), Granularity::Summary, 0.8);
    item.id = id.into();
    item.memory_type = kind.into();
    item.metadata = serde_json::json!({"known_by":[owner],"record_kind":"fact"});
    item
}

fn manager(fail: bool) -> IndexedHarness {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
    let rows = vec![
        row("semantic", "Cannot drink espresso after sunset", "user", "nana"),
        row("keyword", "Q7-Night is the ticket identifier", "user", "nana"),
        row("private", "A private espresso preference", "user", "vivian"),
        row("note", "A beverage document", "knowledge", "nana"),
    ];
    store.write_rows(rows.iter().map(|item| (item.id.clone(), item.clone())).collect(), &[]).unwrap();
    let mut manager = IndexedHarness::new(store, "nana");
    manager._dir = Arc::new(dir);
    {
        let mut inner = manager.inner.write();
        inner.embedding = Arc::new(SemanticProvider { fail });
        inner.vector_store = Arc::new(MemoryVectorStore::new(manager._dir.path().join("semantic.db"), 2, "semantic-fixture").unwrap());
        for item in &rows {
            inner.vector_store.add(MemoryVector {
                doc_id:item.id.clone(), memory_id:item.id.clone(), content:item.content.clone(),
                embedding:if item.id == "keyword" { vec![0.0, 1.0] } else { vec![1.0, 0.0] },
                importance:item.importance, memory_type:item.memory_type.clone(), timestamp:item.timestamp,
            }).unwrap();
        }
        inner.data.entries = rows;
        inner.rebuild_index();
    }
    manager
}

#[tokio::test]
async fn explicit_lookup_fuses_keyword_and_no_overlap_semantic_hits_and_obeys_scope() {
    let manager = manager(false);
    let query = "Q7-Night 晚间提神饮料导致难以入睡";
    let hits = manager.search_tool_memories(query, 10, &RecallScope::Fact.filter()).await.unwrap();
    assert!(hits.iter().any(|item| item.id == "keyword"));
    let semantic = hits.iter().find(|item| item.id == "semantic").unwrap();
    assert_eq!(semantic.metadata["recall_admission"]["kind"], "semantic");
    assert!(!hits.iter().any(|item| item.id == "private" || item.id == "note"));
    let ids: std::collections::HashSet<_> = hits.iter().map(|item| &item.id).collect();
    assert_eq!(ids.len(), hits.len());
    for limit in [1, 2] {
        assert_eq!(manager.search_tool_memories(query, limit, &RecallScope::Fact.filter()).await.unwrap().len(), limit);
    }
    let note = manager.search_tool_memories(query, 10, &RecallScope::Note.filter()).await.unwrap();
    assert_eq!(note.len(), 1);
    assert_eq!(note[0].id, "note");
}

#[tokio::test]
async fn embedding_failure_preserves_keyword_hits_without_fabricated_semantic_matches() {
    let manager = manager(true);
    let hits = manager.search_tool_memories("Q7-Night 晚间提神饮料导致难以入睡", 10, &RecallScope::All.filter()).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "keyword");
    assert!(manager.search_tool_memories("晚间提神饮料导致难以入睡", 10, &RecallScope::All.filter()).await.unwrap().is_empty());
}

#[tokio::test]
async fn deep_recall_uses_the_same_semantic_pipeline_and_deduplicates_queries() {
    let manager = manager(false);
    let queries = vec![
        RecallQuery { keywords:"晚间提神饮料导致难以入睡".into(), scope:RecallScope::Fact },
        RecallQuery { keywords:"Q7-Night".into(), scope:RecallScope::Fact },
    ];
    let hits = recall_in_parallel(&manager, &queries, "nana", 10).await;
    assert_eq!(hits.iter().filter(|item| item.id == "semantic").count(), 1);
    assert!(hits.iter().any(|item| item.id == "keyword"));
    assert!(!hits.iter().any(|item| item.id == "private" || item.id == "note"));
}
