//! Rebuildable, transactionally maintained Jieba/FTS5 candidate index.
//! Canonical records stay in entries; only selected IDs require JSON deserialization.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use rusqlite::{Connection, params, params_from_iter, types::Value};
use super::{types::{MemoryItem, MemoryRetrievalFilter}, kinds, provenance, tokenize};

const VERSION: &str = "1";
pub const MAX_CANDIDATES: usize = 384;

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    let had_fts: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='memory_fts')", [], |row| row.get(0))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_search(
            rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE REFERENCES entries(id) ON DELETE CASCADE,
            tokens TEXT NOT NULL, body_hash TEXT NOT NULL, created_at REAL NOT NULL,
            memory_type TEXT NOT NULL, is_public INTEGER NOT NULL, seq_no INTEGER, session_key TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS memory_search_time ON memory_search(created_at,id);
         CREATE INDEX IF NOT EXISTS memory_search_seq ON memory_search(session_key,seq_no);
         CREATE TABLE IF NOT EXISTS memory_search_access(
            memory_id TEXT NOT NULL REFERENCES memory_search(id) ON DELETE CASCADE,
            character TEXT NOT NULL, PRIMARY KEY(memory_id,character));
         CREATE TABLE IF NOT EXISTS memory_search_tags(
            memory_id TEXT NOT NULL REFERENCES memory_search(id) ON DELETE CASCADE,
            tag TEXT NOT NULL, PRIMARY KEY(memory_id,tag));
         CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
            tokens, content='memory_search', content_rowid='rowid', tokenize='ascii');
         CREATE TRIGGER IF NOT EXISTS memory_search_ai AFTER INSERT ON memory_search BEGIN
            INSERT INTO memory_fts(rowid,tokens) VALUES(new.rowid,new.tokens); END;
         CREATE TRIGGER IF NOT EXISTS memory_search_ad AFTER DELETE ON memory_search BEGIN
            INSERT INTO memory_fts(memory_fts,rowid,tokens) VALUES('delete',old.rowid,old.tokens); END;
         CREATE TRIGGER IF NOT EXISTS memory_search_au AFTER UPDATE OF tokens ON memory_search
            WHEN old.tokens != new.tokens BEGIN
            INSERT INTO memory_fts(memory_fts,rowid,tokens) VALUES('delete',old.rowid,old.tokens);
            INSERT INTO memory_fts(rowid,tokens) VALUES(new.rowid,new.tokens); END;
         CREATE TABLE IF NOT EXISTS memory_search_pending(id TEXT PRIMARY KEY);
         CREATE TRIGGER IF NOT EXISTS entries_search_ai AFTER INSERT ON entries BEGIN
            INSERT OR IGNORE INTO memory_search_pending VALUES(new.id); END;
         CREATE TRIGGER IF NOT EXISTS entries_search_au AFTER UPDATE ON entries BEGIN
            INSERT OR IGNORE INTO memory_search_pending VALUES(new.id); END;
         CREATE TRIGGER IF NOT EXISTS entries_search_ad AFTER DELETE ON entries BEGIN
            DELETE FROM memory_search_pending WHERE id=old.id; END;"
    )?;
    let version = conn.query_row("SELECT value FROM meta WHERE key='memory_search_version'", [], |row| row.get::<_,String>(0)).ok();
    let tx = conn.unchecked_transaction()?;
    if version.as_deref() != Some(VERSION) || !had_fts {
        // If the virtual table was rebuilt, recreate its index from the content table first
        // so delete triggers see consistent old rows during the canonical backfill.
        tx.execute("INSERT INTO memory_fts(memory_fts) VALUES('rebuild')", [])?;
        tx.execute("DELETE FROM memory_search", [])?;
        let mut statement = tx.prepare("SELECT json FROM entries")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            match serde_json::from_str::<MemoryItem>(&json) {
                Ok(item) => upsert(&tx, &item)?,
                Err(error) => tracing::warn!(%error, "无法为损坏记忆建立索引，规范行保留"),
            }
        }
        tx.execute("INSERT OR REPLACE INTO meta VALUES('memory_search_version',?1)", [VERSION])?;
        tx.execute("DELETE FROM memory_search_pending", [])?;
    } else {
        // SQL triggers also record writes from older app versions. Repair those IDs
        // on open without scanning or deserializing the entire collection again.
        loop {
            let rows: Vec<(String,Option<String>)> = tx.prepare(
                "SELECT p.id,e.json FROM memory_search_pending p LEFT JOIN entries e ON e.id=p.id LIMIT 256")?
                .query_map([], |row| Ok((row.get(0)?,row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            if rows.is_empty() { break; }
            for (id,json) in rows {
                if let Some(item) = json.as_deref().and_then(|raw| serde_json::from_str::<MemoryItem>(raw).ok()) {
                    upsert(&tx,&item)?;
                } else { tx.execute("DELETE FROM memory_search WHERE id=?1", [&id])?; }
                tx.execute("DELETE FROM memory_search_pending WHERE id=?1", [&id])?;
            }
        }
    }
    tx.commit()
}

fn audience(item: &MemoryItem) -> (bool, HashSet<String>) {
    let meta = &item.metadata;
    if let Some(known) = meta.get("known_by").and_then(serde_json::Value::as_array) {
        return (false,known.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect());
    }
    let mut people: HashSet<_> = ["speaker","listener","observer_id"].iter()
        .filter_map(|key| provenance::field(meta,key).map(str::to_owned)).collect();
    let public = if people.is_empty() {
        match provenance::field(meta,"memory_owner") {
            Some(owner) => {people.insert(owner.to_owned());false}
            None => true,
        }
    } else { provenance::field(meta,"listener") == Some("all") && provenance::field(meta,"knowledge_source") == Some("broadcast") };
    (public,people)
}

pub fn upsert(conn: &Connection, item: &MemoryItem) -> rusqlite::Result<()> {
    if !kinds::recallable(item) {
        conn.prepare_cached("DELETE FROM memory_search WHERE id=?1")?.execute([&item.id])?;
    } else {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        item.content.hash(&mut hash); item.tags.hash(&mut hash); item.description.hash(&mut hash);
        let fingerprint = hash.finish().to_string();
        let old: Option<String> = conn.prepare_cached("SELECT body_hash FROM memory_search WHERE id=?1")?
            .query_row([&item.id], |row| row.get(0)).ok();
        let (public,people) = audience(item);
        let session = provenance::field(&item.metadata,"conversation_id")
            .or_else(|| provenance::field(&item.metadata,"session_id")).unwrap_or("");
        if old.as_deref() != Some(&fingerprint) {
            let mut tokens = tokenize::tokenize(&item.content);
            for tag in &item.tags {tokens.extend(tokenize::tokenize(tag));}
            if let Some(description) = &item.description {tokens.extend(tokenize::tokenize(description));}
            conn.prepare_cached(
                "INSERT INTO memory_search(id,tokens,body_hash,created_at,memory_type,is_public,seq_no,session_key)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET
                 tokens=excluded.tokens,body_hash=excluded.body_hash,created_at=excluded.created_at,
                 memory_type=excluded.memory_type,is_public=excluded.is_public,seq_no=excluded.seq_no,session_key=excluded.session_key")?
                .execute(params![item.id,tokens.join(" "),fingerprint,item.timestamp,item.memory_type,public,item.seq_no().and_then(|n|i64::try_from(n).ok()),session])?;
        } else {
            conn.prepare_cached("UPDATE memory_search SET created_at=?2,memory_type=?3,is_public=?4,seq_no=?5,session_key=?6 WHERE id=?1")?
                .execute(params![item.id,item.timestamp,item.memory_type,public,item.seq_no().and_then(|n|i64::try_from(n).ok()),session])?;
        }
        // Audience/tag changes must take effect even when searchable text is unchanged.
        conn.prepare_cached("DELETE FROM memory_search_access WHERE memory_id=?1")?.execute([&item.id])?;
        for person in people {conn.prepare_cached("INSERT INTO memory_search_access VALUES(?1,?2)")?.execute(params![item.id,person])?;}
        conn.prepare_cached("DELETE FROM memory_search_tags WHERE memory_id=?1")?.execute([&item.id])?;
        for tag in item.tags.iter().collect::<HashSet<_>>() {
            conn.prepare_cached("INSERT INTO memory_search_tags VALUES(?1,?2)")?.execute(params![item.id,tag])?;
        }
    }
    conn.prepare_cached("DELETE FROM memory_search_pending WHERE id=?1")?.execute([&item.id])?;
    Ok(())
}

fn scope(owner: &str, filter: &MemoryRetrievalFilter, args: &mut Vec<Value>) -> String {
    args.push(Value::Text(owner.to_owned()));
    let mut sql = "(d.is_public=1 OR EXISTS(SELECT 1 FROM memory_search_access a WHERE a.memory_id=d.id AND a.character=?))".to_owned();
    if let Some(after) = filter.time_after {sql.push_str(" AND d.created_at>=?");args.push(Value::Real(after));}
    if let Some(before) = filter.time_before {sql.push_str(" AND d.created_at<?");args.push(Value::Real(before));}
    for (values,field) in [(&filter.memory_types,"d.memory_type"),(&filter.tags_any,"tag")] {
        if let Some(values) = values {
            if values.is_empty() {sql.push_str(" AND 0");continue;}
            let placeholders = vec!["?";values.len()].join(",");
            if field == "tag" {sql.push_str(&format!(" AND EXISTS(SELECT 1 FROM memory_search_tags t WHERE t.memory_id=d.id AND t.tag IN ({placeholders}))"));}
            else {sql.push_str(&format!(" AND {field} IN ({placeholders})"));}
            args.extend(values.iter().cloned().map(Value::Text));
        }
    }
    sql
}

pub fn search(conn: &Connection, query: &str, owner: &str, filter: &MemoryRetrievalFilter,
    limit: usize) -> rusqlite::Result<Vec<(String,f64)>> {
    if limit == 0 {return Ok(Vec::new());}
    let mut seen = HashSet::new();
    let words = if super::recall_policy::preference_query(query) { vec!["preference".into()] }
        else { tokenize::tokenize(query) };
    let single = words.len() == 1;
    let mut terms: Vec<_> = words.into_iter().filter(|word|
        (single || word.chars().count()>1 || ["猫","狗","饭","药","病","钱","书","歌","茶"].contains(&word.as_str()))
        && seen.insert(word.clone())).collect();
    if terms.len() > 64 {terms.drain(32..terms.len()-32);}
    if terms.is_empty() {return Ok(Vec::new());}
    // Bind a quoted OR expression; user text never becomes FTS operators or SQL.
    let expression = terms.iter().map(|term|format!("\"{}\"",term.replace('"',"\"\""))).collect::<Vec<_>>().join(" OR ");
    let mut args = vec![Value::Text(expression)];
    let predicate = scope(owner,filter,&mut args);
    args.push(Value::Integer(limit.min(MAX_CANDIDATES) as i64));
    conn.prepare_cached(&format!(
        "SELECT d.id,-memory_fts.rank FROM memory_fts JOIN memory_search d ON d.rowid=memory_fts.rowid
         WHERE memory_fts MATCH ? AND {predicate} ORDER BY memory_fts.rank LIMIT ?"))?
        .query_map(params_from_iter(args),|row|Ok((row.get(0)?,row.get(1)?)))?.collect()
}

pub fn load_ids(conn: &Connection, ids: &[String], owner: Option<&str>, filter: &MemoryRetrievalFilter)
    -> rusqlite::Result<Vec<MemoryItem>> {
    let mut result = Vec::new(); let mut seen = HashSet::new();
    // Fixed batches bound SQL parameters and return IDs in caller order.
    for batch in ids.chunks(256) {
        let placeholders = vec!["?";batch.len()].join(",");
        let mut args: Vec<_> = batch.iter().cloned().map(Value::Text).collect();
        let sql = if let Some(owner) = owner {
            let predicate = scope(owner,filter,&mut args);
            format!("SELECT e.id,e.json FROM entries e JOIN memory_search d ON d.id=e.id WHERE e.id IN ({placeholders}) AND {predicate}")
        } else {format!("SELECT id,json FROM entries WHERE id IN ({placeholders})")};
        let rows: HashMap<String,String> = conn.prepare_cached(&sql)?.query_map(params_from_iter(args),|row|Ok((row.get(0)?,row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for id in batch {
            if seen.insert(id.clone()) {
                if let Some(raw) = rows.get(id) {
                    if let Ok(item) = serde_json::from_str::<MemoryItem>(raw) {
                        if owner.is_none_or(|owner| kinds::recallable(&item) && provenance::visible_to(&item.metadata,owner) && filter.matches(&item)) {result.push(item);}
                    }
                }
            }
        }
    }
    Ok(result)
}

pub fn neighbors(conn: &Connection, hits: &[MemoryItem], owner: &str, limit: usize)
    -> rusqlite::Result<Vec<MemoryItem>> {
    let mut ids = Vec::new();
    for hit in hits {
        let Some(seq) = hit.seq_no() else {continue;};
        let session = provenance::field(&hit.metadata,"conversation_id")
            .or_else(||provenance::field(&hit.metadata,"session_id")).unwrap_or("");
        if session.is_empty() {continue;}
        for target in [seq.checked_add(1),seq.checked_sub(1)].into_iter().flatten() {
            let Ok(target) = i64::try_from(target) else {continue;};
            if ids.len() >= limit.min(MAX_CANDIDATES) {break;}
            let mut args = vec![Value::Text(session.to_owned()),Value::Integer(target)];
            let predicate = scope(owner,&MemoryRetrievalFilter::default(),&mut args);
            let mut statement = conn.prepare_cached(&format!("SELECT d.id FROM memory_search d JOIN entries e ON e.id=d.id
                WHERE d.session_key=? AND d.seq_no=? AND json_extract(e.json,'$.granularity')='turn' AND {predicate} LIMIT 4"))?;
            ids.extend(statement.query_map(params_from_iter(args),|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        if ids.len() >= limit.min(MAX_CANDIDATES) {break;}
    }
    ids.retain(|id| !hits.iter().any(|hit|&hit.id==id));
    let mut result = load_ids(conn,&ids,Some(owner),&MemoryRetrievalFilter::default())?;
    result.truncate(limit.min(MAX_CANDIDATES));
    for item in &mut result {item.metadata["expansion"] = serde_json::json!(true);}
    Ok(result)
}

#[cfg(test)]
#[path = "search_index_tests.rs"]
mod tests;
