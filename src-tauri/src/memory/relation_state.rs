//! Request-local memory state. Only explicit persisted relations can expand a
//! query-admitted seed; similarity never establishes replacement or conflict.
use std::collections::{HashSet, VecDeque};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use super::super::{kinds, provenance, types::MemoryItem};

const MAX_GROUP: usize = 8;
const MAX_DEPTH: usize = 4;
const MAX_SEEDS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind { Supersedes, ConflictsWith }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryRelation {
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryBundle {
    pub items: Vec<MemoryItem>,
    pub relations: Vec<MemoryRelation>,
    /// Bounds, missing endpoints and access boundaries must not imply certainty.
    pub incomplete: bool,
}

impl MemoryBundle {
    pub fn single(item: MemoryItem) -> Self {
        Self { items: vec![item], relations: Vec::new(), incomplete: false }
    }

    /// A deterministic usage policy, not a new fact or an authorization.
    pub fn usage_hint(&self, id: &str) -> &'static str {
        let historical = self.relations.iter().any(|r| r.kind == RelationKind::Supersedes && r.target == id);
        let disputed = self.relations.iter().any(|r| r.kind == RelationKind::ConflictsWith && (r.source == id || r.target == id));
        if historical && disputed { "历史版本；同时有未解决的冲突，不代表当前事实" }
        else if historical { "已被替代的历史版本；仅用于历史或变化问题，不代表当前事实" }
        else if disputed { "有待核实的冲突；不可将任一方当作已确认事实" }
        else if self.incomplete { "关联状态不完整；不要推断其为最新或唯一结论" }
        else if self.relations.iter().any(|r| r.kind == RelationKind::Supersedes && r.source == id) {
            "替代旧版本的记录；仅在其时间和适用范围内使用"
        } else { "按来源、时间与适用范围使用；偏好和推测不构成客观事实" }
    }
}

/// Different explicit subjects/scopes cannot silently overwrite one another.
/// Missing legacy fields do not fabricate a scope; an explicit scope cannot
/// replace an unscoped value automatically either.
pub fn same_slot(a: &Value, b: &Value) -> bool {
    ["subject", "scope"].iter().all(|key| {
        let a = a.get(*key).filter(|v| !v.is_null());
        let b = b.get(*key).filter(|v| !v.is_null());
        a == b
    })
}

fn fact(item: &MemoryItem) -> bool {
    kinds::kind(item) == kinds::RecordKind::Fact
        && !item.content.trim().is_empty()
        && item.metadata["recall_hidden"] != true
        && item.metadata["index_active"] != false
        && item.metadata["fictional_backstory"] != true
        && !matches!(item.metadata["source"].as_str(), Some("system_seed" | "environment_preset"))
        && !item.tags.iter().any(|t| t == "quick_note")
}

pub fn mark_superseded(old: &mut MemoryItem, new_id: &str) {
    old.consolidated = true;
    old.metadata["superseded_by"] = json!(new_id);
}

pub fn mark_conflict(new: &mut MemoryItem, old_id: &str) {
    let mut ids = new.metadata["conflicts_with"].as_array().cloned().unwrap_or_default();
    if !ids.iter().any(|v| v.as_str() == Some(old_id)) { ids.push(json!(old_id)); }
    new.metadata["conflicts_with"] = Value::Array(ids);
}

pub fn clear_conflict(new: &mut MemoryItem, old_id: &str) {
    if let Some(ids) = new.metadata["conflicts_with"].as_array_mut() {
        ids.retain(|v| v.as_str() != Some(old_id));
    }
}

/// Explicit relations live on canonical records, so the SQLite projection can
/// be dropped and rebuilt. Never infer reverse relations from semantic overlap.
fn declared(item: &MemoryItem) -> Vec<MemoryRelation> {
    if !fact(item) { return Vec::new(); }
    let mut out = Vec::new();
    if let Some(id) = item.metadata["supersedes"].as_str().filter(|id| !id.is_empty() && *id != item.id) {
        out.push(MemoryRelation { source: item.id.clone(), target: id.into(), kind: RelationKind::Supersedes });
    }
    if let Some(ids) = item.metadata["conflicts_with"].as_array() {
        for id in ids.iter().filter_map(Value::as_str).filter(|id| !id.is_empty() && *id != item.id).take(16) {
            let edge = MemoryRelation { source: item.id.clone(), target: id.into(), kind: RelationKind::ConflictsWith };
            if !out.contains(&edge) { out.push(edge); }
        }
    }
    out.sort_by(|a,b| {
        let key = |r: &MemoryRelation| if r.kind == RelationKind::Supersedes { 1 } else { 0 };
        key(a).cmp(&key(b)).then_with(||a.target.cmp(&b.target))
    });
    out
}

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_state_relations(
            source TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
            target TEXT NOT NULL, kind TEXT NOT NULL,
            PRIMARY KEY(source,target,kind));
         CREATE INDEX IF NOT EXISTS memory_state_incoming ON memory_state_relations(target,source,kind);
         CREATE TRIGGER IF NOT EXISTS entries_state_ad AFTER DELETE ON entries BEGIN
            DELETE FROM memory_state_relations WHERE source=old.id OR target=old.id; END;"
    )
}

pub fn upsert(conn: &Connection, item: &MemoryItem) -> rusqlite::Result<()> {
    // Skip visits/heat-only writes when the relation set is unchanged.
    let expected = declared(item);
    let current = outgoing(conn, &item.id)?;
    if current == expected { return Ok(()); }
    conn.execute("DELETE FROM memory_state_relations WHERE source=?1", [&item.id])?;
    for relation in expected {
        conn.execute("INSERT INTO memory_state_relations VALUES(?1,?2,?3)",
            params![relation.source, relation.target, if relation.kind == RelationKind::Supersedes { "supersedes" } else { "conflicts_with" }])?;
    }
    Ok(())
}

fn edges(conn: &Connection, id: &str, incoming: bool) -> rusqlite::Result<Vec<MemoryRelation>> {
    let sql = if incoming {
        "SELECT source,target,kind FROM memory_state_relations WHERE target=?1 ORDER BY source,kind LIMIT 17"
    } else {
        "SELECT source,target,kind FROM memory_state_relations WHERE source=?1 ORDER BY kind,target LIMIT 17"
    };
    conn.prepare_cached(sql)?.query_map([id], |r| {
        let kind: String = r.get(2)?;
        Ok(MemoryRelation { source: r.get(0)?, target: r.get(1)?,
            kind: if kind == "supersedes" { RelationKind::Supersedes } else { RelationKind::ConflictsWith } })
    })?.collect()
}

fn outgoing(conn: &Connection, id: &str) -> rusqlite::Result<Vec<MemoryRelation>> { edges(conn, id, false) }

fn load(conn: &Connection, id: &str) -> rusqlite::Result<Option<MemoryItem>> {
    let raw: Option<String> = conn.query_row("SELECT json FROM entries WHERE id=?1", [id], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str::<MemoryItem>(&raw).ok()).filter(|item| item.id == id))
}

fn visible(item: &MemoryItem, owner: &str) -> bool {
    fact(item) && provenance::visible_to(&item.metadata, owner)
        // Archived records are historical only after an explicit replacement.
        // A generic archive/delete must never be revived by an old link.
        && (!item.consolidated || item.metadata["superseded_by"].as_str().is_some())
}

fn valid_edge(edge: &MemoryRelation, source: &MemoryItem, target: &MemoryItem) -> bool {
    same_slot(&source.metadata, &target.metadata)
        && declared(source).contains(edge)
        && (edge.kind != RelationKind::Supersedes
            || !target.consolidated
            || target.metadata["superseded_by"] == source.id)
}

/// Read a bounded relation closure from canonical rows. Caller supplies only
/// query-admitted seeds after session/verifier filtering. All linked endpoints
/// are checked independently; a public node is never a private knowledge grant.
pub fn assemble(conn: &Connection, seeds: &[MemoryItem], owner: &str) -> rusqlite::Result<Vec<MemoryBundle>> {
    let mut bundles: Vec<MemoryBundle> = Vec::new();
    let mut used = HashSet::new();
    for seed in seeds.iter().take(MAX_SEEDS) {
        if used.contains(&seed.id) { continue; }
        let Some(current) = load(conn, &seed.id)? else { continue; };
        if current.content != seed.content || !kinds::recallable(&current)
            || current.metadata["recall_hidden"] == true || !provenance::visible_to(&current.metadata, owner) { continue; }
        let mut current = current;
        // Query-local relevance belongs to the seed, never to expanded nodes.
        for key in ["recall_admission", "recall_admission_query", "fused_score", "combined_score"] {
            if let Some(value) = seed.metadata.get(key) { current.metadata[key] = value.clone(); }
        }
        let mut bundle = MemoryBundle::single(current);
        let mut queue = VecDeque::from([(seed.id.clone(), 0usize)]);
        let mut known = HashSet::from([seed.id.clone()]);
        while let Some((id, depth)) = queue.pop_front() {
            if !fact(bundle.items.iter().find(|m| m.id == id).unwrap()) { continue; }
            let mut links = outgoing(conn, &id)?;
            links.extend(edges(conn, &id, true)?);
            for edge in links {
                let other_id = if edge.source == id { &edge.target } else { &edge.source };
                if !known.contains(other_id) && (bundle.items.len() >= MAX_GROUP || depth >= MAX_DEPTH) {
                    bundle.incomplete = true; continue;
                }
                let Some(other) = load(conn, other_id)? else { bundle.incomplete = true; continue; };
                if !visible(&other, owner) { bundle.incomplete = true; continue; }
                let this = bundle.items.iter().find(|m| m.id == id).unwrap();
                let (source, target) = if edge.source == id { (this, &other) } else { (&other, this) };
                if !valid_edge(&edge, source, target) { bundle.incomplete = true; continue; }
                if known.insert(other_id.clone()) {
                    queue.push_back((other_id.clone(), depth + 1));
                    bundle.items.push(other);
                }
                if !bundle.relations.contains(&edge) { bundle.relations.push(edge); }
            }
        }
        // A bounded traversal can intersect an earlier group. Merge rather than
        // emit the same record twice or assign it contradictory group budgets.
        let mut i = 0;
        while i < bundles.len() {
            if bundles[i].items.iter().any(|item| known.contains(&item.id)) {
                let previous = bundles.remove(i);
                for item in previous.items { if known.insert(item.id.clone()) { bundle.items.push(item); } }
                for relation in previous.relations { if !bundle.relations.contains(&relation) { bundle.relations.push(relation); } }
                bundle.incomplete |= previous.incomplete;
            } else { i += 1; }
        }
        used.extend(known);
        bundles.push(bundle);
    }
    bundles.sort_by_key(|bundle| seeds.iter().position(|seed| bundle.items.iter().any(|m|m.id == seed.id)).unwrap_or(usize::MAX));
    Ok(bundles)
}

/// Whole-group token admission; oversized groups are omitted, never split.
pub fn select_groups(groups: &[Vec<usize>], budget: usize) -> Vec<usize> {
    let mut used = 0usize;
    groups.iter().enumerate().filter_map(|(index, costs)| {
        let cost = costs.iter().fold(0usize, |sum, cost| sum.saturating_add(*cost));
        if cost <= budget.saturating_sub(used) { used += cost; Some(index) } else { None }
    }).collect()
}

/// Applicable fields remain data, not instructions. Unknown dates/scopes are
/// preserved, never guessed from extraction time or from a relation edge.
pub fn applicability(item: &MemoryItem) -> Option<String> {
    let mut fields = serde_json::Map::new();
    for key in ["scope", "time", "valid_from", "valid_to", "expires_at"] {
        if let Some(value) = item.metadata.get(key).filter(|v| !v.is_null()) { fields.insert(key.into(), value.clone()); }
    }
    (!fields.is_empty()).then(|| Value::Object(fields).to_string().chars().take(600).collect())
}

#[cfg(test)]
#[path = "relation_state_tests.rs"]
mod tests;
