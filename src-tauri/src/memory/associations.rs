//! Rebuildable memory → evidence/event/topic index. Shared anchors are context,
//! never a generated claim of causality, agreement, or truth.
use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{
    kinds, recall_policy, search_index,
    types::{current_timestamp, MemoryItem, MemoryRetrievalFilter},
};

const VERSION: i64 = 1;
const READY_KEY: &str = "memory_associations_ready";
const MAX_ANCHORS: usize = 96;
const MAX_SEEDS: usize = 3;
const MAX_NEIGHBORS: usize = 128;
const MAX_WEAK_DEGREE: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Anchor {
    kind: String,
    key: String,
    /// Where this membership came from; not a model-generated relation.
    evidence: String,
}

fn key(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= 240).then(|| value.to_string())
}

fn anchors(item: &MemoryItem) -> Vec<Anchor> {
    if !kinds::recallable(item) {
        return Vec::new();
    }
    let mut out = BTreeMap::new();
    let mut add = |kind: &str, value: &str, evidence: String| {
        if let Some(key) = key(value) {
            out.entry((kind.to_string(), key.clone()))
                .or_insert(Anchor {
                    kind: kind.into(),
                    key,
                    evidence,
                });
        }
    };
    add("memory", &item.id, "self".into());
    for id in item.related_ids.iter().take(32) {
        add("memory", id, "related_ids".into());
    }
    if let Some(ids) = item.metadata["promoted_from"].as_array() {
        for id in ids.iter().take(32).filter_map(serde_json::Value::as_str) {
            add("memory", id, "promoted_from".into());
        }
    }
    // Host-written learning anchors retain the actual originating memory ID.
    for tag in &item.tags {
        if let Some(id) = tag.strip_prefix("learning_anchor:memory:") {
            add("memory", id, "learning_origin".into());
        }
    }
    if let Some(id) = item
        .episode_id
        .as_deref()
        .or_else(|| item.metadata["episode_id"].as_str())
    {
        add("episode", id, "episode_id".into());
    }
    let source_ids: HashSet<_> = item.metadata["source_message_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for id in source_ids.iter() {
        add("source", id, "source_message_ids".into());
    }
    if let Some(id) = item.metadata["utterance_id"].as_str() {
        add("source", id, "utterance_id".into());
    }
    if let Some(sources) = item.metadata["evidence_sources"].as_array() {
        for source in sources.iter().take(16) {
            if let Some(ids) = source["message_ids"].as_array() {
                for id in ids.iter().take(32).filter_map(serde_json::Value::as_str) {
                    add("source", id, "evidence_sources.message_ids".into());
                }
            }
        }
    }
    // Only validated session event records can create event-continuity anchors.
    // A free-form title or a common topic never establishes an event identity.
    if item.metadata["event_schema_version"] == 1
        && item.metadata["source"] == "session_consolidation"
        && kinds::kind(item) == kinds::RecordKind::SessionSummary
    {
        for part in item.metadata["summary_parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for event in part["events"].as_array().into_iter().flatten() {
                let Some(id) = event["id"].as_str() else {
                    continue;
                };
                let Some(source) = event["source_message_id"].as_str() else {
                    continue;
                };
                let Some(quote) = event["source_quote"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                else {
                    continue;
                };
                if source_ids.contains(source) {
                    add(
                        "event",
                        id,
                        serde_json::json!({"message_id":source,
                        "quote":quote.chars().take(500).collect::<String>(),
                        "phase":event["phase"],"recorded_at":event["recorded_at"]})
                        .to_string(),
                    );
                }
            }
        }
    }
    let content = item.content.to_lowercase();
    const GENERIC: &[&str] = &[
        "user",
        "identity",
        "preference",
        "project",
        "relationship",
        "knowledge",
        "reference",
        "dialogue",
        "short_term",
        "long_term",
        "session_summary",
        "用户",
        "知识",
        "聊天",
        "记忆",
        "生活",
        "学习",
        "工作",
        "兴趣",
        "偏好",
    ];
    for topic in item.metadata["topics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .take(16)
    {
        let topic = topic.trim().to_lowercase();
        if (2..=80).contains(&topic.chars().count())
            && !GENERIC.contains(&topic.as_str())
            && content.contains(&topic)
        {
            add(
                "topic",
                &topic,
                "topics; literal occurrence in content".into(),
            );
        }
    }
    out.into_values().take(MAX_ANCHORS).collect()
}

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_nodes(
            id TEXT PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,
            signature TEXT NOT NULL, version INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS memory_anchors(
            memory_id TEXT NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
            kind TEXT NOT NULL, anchor TEXT NOT NULL, evidence TEXT NOT NULL,
            occurred_at REAL NOT NULL,
            PRIMARY KEY(memory_id,kind,anchor));
         CREATE INDEX IF NOT EXISTS memory_anchor_lookup
            ON memory_anchors(kind,anchor,occurred_at DESC,memory_id);",
    )
}

fn occurred_at(item: &MemoryItem) -> f64 {
    item.metadata["source_timestamp"]
        .as_f64()
        .or_else(|| item.metadata["started_at"].as_f64())
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(item.timestamp)
}

pub fn upsert(conn: &Connection, item: &MemoryItem) -> rusqlite::Result<()> {
    let anchors = anchors(item);
    let at = occurred_at(item);
    // Visits, heat and embeddings must not rewrite the graph on every recall.
    let signature = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(VERSION, &anchors, at)).unwrap_or_default())
    );
    let previous: Option<String> = conn
        .query_row(
            "SELECT signature FROM memory_nodes WHERE id=?1 AND version=?2",
            params![item.id, VERSION],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref() == Some(&signature) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO memory_nodes(id,signature,version) VALUES(?1,?2,?3)
        ON CONFLICT(id) DO UPDATE SET signature=excluded.signature,version=excluded.version",
        params![item.id, signature, VERSION],
    )?;
    conn.execute("DELETE FROM memory_anchors WHERE memory_id=?1", [&item.id])?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO memory_anchors(memory_id,kind,anchor,evidence,occurred_at)
        VALUES(?1,?2,?3,?4,?5)",
    )?;
    for anchor in anchors {
        insert.execute(params![
            item.id,
            anchor.kind,
            anchor.key,
            anchor.evidence,
            at
        ])?;
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct OrganizationReport {
    pub indexed: usize,
    pub pending: bool,
}

/// Backfill old databases in bounded transactions. Normal writes maintain the
/// projection atomically, so completed backfills never scan the corpus again.
pub fn organize(conn: &Connection, batch_size: usize) -> rusqlite::Result<OrganizationReport> {
    let ready: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key=?1", [READY_KEY], |r| {
            r.get(0)
        })
        .optional()?;
    if ready.as_deref() == Some(&VERSION.to_string()) {
        return Ok(OrganizationReport::default());
    }
    let batch_size = batch_size.clamp(1, 256);
    let tx = conn.unchecked_transaction()?;
    let rows: Vec<(String, String)> = tx
        .prepare(
            "SELECT e.id,e.json FROM entries e LEFT JOIN memory_nodes n ON e.id=n.id
         WHERE n.id IS NULL OR n.version<>?1 ORDER BY e.id LIMIT ?2",
        )?
        .query_map(params![VERSION, batch_size + 1], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let pending = rows.len() > batch_size;
    let indexed = rows.len().min(batch_size);
    for (id, json) in rows.into_iter().take(batch_size) {
        match serde_json::from_str::<MemoryItem>(&json) {
            Ok(item) if item.id == id => upsert(&tx, &item)?,
            _ => {
                tracing::warn!(memory_id=%id, "跳过损坏记忆的关联索引；保留原记录");
                tx.execute(
                    "INSERT OR REPLACE INTO memory_nodes(id,signature,version) VALUES(?1,'',?2)",
                    params![id, VERSION],
                )?;
            }
        }
    }
    if !pending {
        tx.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)
            ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![READY_KEY, VERSION.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(OrganizationReport { indexed, pending })
}

#[derive(Clone, Serialize)]
struct LinkEvidence {
    kind: String,
    anchor: String,
    seed_evidence: String,
    target_evidence: String,
}

struct Candidate {
    seed: String,
    seed_rank: usize,
    links: Vec<LinkEvidence>,
}

fn strong(kind: &str) -> bool {
    kind != "topic"
}

fn authored(item: &MemoryItem) -> bool {
    item.metadata["fictional_backstory"] == true
        || matches!(
            item.metadata["source"].as_str(),
            Some("system_seed" | "environment_preset")
        )
}

/// One hop from admitted, still-visible direct hits. This never recursively
/// expands its own results and never turns common generic tags into evidence.
pub fn expand(
    conn: &Connection,
    hits: &[MemoryItem],
    query: &str,
    owner: &str,
    filter: &MemoryRetrievalFilter,
    limit: usize,
) -> rusqlite::Result<Vec<MemoryItem>> {
    let budget = (limit / 3).min(4);
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    if budget == 0 || hits.is_empty() {
        return Ok(hits.iter().take(limit).cloned().collect());
    }
    let ids: Vec<_> = hits.iter().map(|m| m.id.clone()).collect();
    let current = search_index::load_ids(conn, &ids, Some(owner), filter)?;
    let current: HashMap<_, _> = current.into_iter().map(|m| (m.id.clone(), m)).collect();
    let mut direct: Vec<_> = hits
        .iter()
        .filter(|m| {
            current
                .get(&m.id)
                .is_some_and(|canonical| canonical.content == m.content)
        })
        .cloned()
        .collect();
    let existing: HashSet<_> = direct.iter().map(|m| m.id.clone()).collect();
    let mut candidates: BTreeMap<(String, String), Candidate> = BTreeMap::new();
    // Every bridge must remain in the final direct portion of the result.
    for (rank, seed) in direct
        .iter()
        .take(MAX_SEEDS.min(limit - budget))
        .enumerate()
    {
        // A concurrent correction cannot keep an old query hit alive as a bridge.
        let canonical = &current[&seed.id];
        for anchor in anchors(canonical).into_iter().take(64) {
            if anchor.kind == "memory" && !current.contains_key(&anchor.key) {
                let reference = search_index::load_ids(
                    conn,
                    &[anchor.key.clone()],
                    Some(owner),
                    &MemoryRetrievalFilter::default(),
                )?;
                if reference.is_empty() {
                    continue;
                }
            }
            let cap = if strong(&anchor.kind) {
                64
            } else {
                MAX_WEAK_DEGREE
            };
            let neighbors: Vec<(String, String)> = conn
                .prepare_cached(
                    "SELECT memory_id,evidence FROM memory_anchors WHERE kind=?1 AND anchor=?2
                 ORDER BY occurred_at DESC,memory_id LIMIT ?3",
                )?
                .query_map(params![anchor.kind, anchor.key, cap + 1], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if !strong(&anchor.kind) && neighbors.len() > cap {
                continue;
            }
            for (id, evidence) in neighbors.into_iter().take(cap) {
                if existing.contains(&id) {
                    continue;
                }
                if candidates.len() >= MAX_NEIGHBORS
                    && !candidates.contains_key(&(id.clone(), seed.id.clone()))
                {
                    continue;
                }
                let candidate =
                    candidates
                        .entry((id, seed.id.clone()))
                        .or_insert_with(|| Candidate {
                            seed: seed.id.clone(),
                            seed_rank: rank,
                            links: Vec::new(),
                        });
                candidate.links.push(LinkEvidence {
                    kind: anchor.kind.clone(),
                    anchor: anchor.key.clone(),
                    seed_evidence: anchor.evidence.clone(),
                    target_evidence: evidence,
                });
            }
        }
    }
    let ids: Vec<_> = candidates.keys().map(|(id, _)| id.clone()).collect();
    let targets = search_index::load_ids(conn, &ids, Some(owner), filter)?;
    let now = current_timestamp();
    let mut scored = Vec::new();
    for target in targets {
        if target.metadata["expires_at"]
            .as_f64()
            .is_some_and(|t| now >= t)
            || (kinds::kind(&target) == kinds::RecordKind::Dialogue
                && now - target.timestamp > 12.0 * 3600.0)
            || !occurred_at(&target).is_finite()
            || occurred_at(&target) > now + 1.0
        {
            continue;
        }
        for ((id, _), candidate) in candidates.iter().filter(|((id, _), _)| id == &target.id) {
            if authored(&target) != authored(&current[&candidate.seed]) {
                continue;
            }
            let strong_link = candidate.links.iter().any(|l| strong(&l.kind));
            if !strong_link && candidate.links.len() < 2 {
                continue;
            }
            // Semantic proximity supplements provenance, never declares a new fact.
            let relation = if candidate.links.iter().any(|l| l.kind == "event") {
                "same_event"
            } else if candidate.links.iter().any(|l| l.kind == "episode") {
                "same_episode"
            } else if candidate.links.iter().any(|l| l.kind == "source") {
                "shared_evidence"
            } else if strong_link {
                "memory_reference"
            } else {
                "related_topics"
            };
            let age_days = ((now - occurred_at(&target)).max(0.0)) / 86400.0;
            let temporal = (-age_days / 180.0).exp();
            let weight = if strong_link { 0.8 } else { 0.45 };
            let score = (weight + 0.1 * temporal) / (candidate.seed_rank as f64 + 1.0);
            let mut item = target.clone();
            let admission = recall_policy::admit(query, &item, None, true);
            if let Some(admission) = admission {
                recall_policy::stamp(&mut item, query, admission);
            }
            item.metadata["recall_association"] = serde_json::json!({"seed_memory_id":candidate.seed,
                "relation":relation,"hop":1,"evidence":candidate.links,"weight":score});
            let base = direct[candidate.seed_rank].metadata["fused_score"]
                .as_f64()
                .unwrap_or(0.1);
            item.metadata["fused_score"] = serde_json::json!(base * 0.7 * score);
            item.metadata["combined_score"] = item.metadata["fused_score"].clone();
            scored.push((score, id.clone(), item));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut seen = HashSet::new();
    let additions: Vec<_> = scored
        .into_iter()
        .filter(|(_, id, _)| seen.insert(id.clone()))
        .take(budget)
        .map(|(_, _, m)| m)
        .collect();
    direct.truncate(limit.saturating_sub(additions.len()));
    direct.extend(additions);
    Ok(direct)
}

#[cfg(test)]
#[path = "associations_tests.rs"]
mod tests;
