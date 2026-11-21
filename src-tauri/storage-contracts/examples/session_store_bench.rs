use serde_json::{json, Value};
use std::{path::Path, time::Instant};
use vivian_storage_contracts::coding_session_store as store;
fn files(root: &Path) -> std::collections::BTreeMap<String, (std::time::SystemTime, u64)> {
    let mut all = std::collections::BTreeMap::new();
    fn walk(
        root: &Path,
        path: &Path,
        all: &mut std::collections::BTreeMap<String, (std::time::SystemTime, u64)>,
    ) {
        for e in std::fs::read_dir(path).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, all)
            } else {
                let m = p.metadata().unwrap();
                all.insert(
                    p.strip_prefix(root).unwrap().to_string_lossy().to_string(),
                    (m.modified().unwrap(), m.len()),
                );
            }
        }
    }
    walk(root, root, &mut all);
    all
}
fn main() {
    for count in [1000, 10000] {
        let dir = tempfile::tempdir().unwrap();
        store::save(dir.path(),|| (0..count).map(|i| {let id=format!("s{i}");(id.clone(), json!({"session_id":id,"title":"session","updated_at":1,"messages":[{"role":"assistant","content":"x".repeat(256)}]}))}).collect()).unwrap();
        let mut heads = store::load_heads::<Value>(dir.path()).unwrap().unwrap();
        let before = files(dir.path());
        let start = Instant::now();
        store::save_cached(dir.path(), || (vec![], heads.clone(), vec![])).unwrap();
        let noop_ms = start.elapsed().as_millis();
        let after = files(dir.path());
        let noop_bytes: u64 = after
            .iter()
            .filter(|(p, v)| before.get(*p) != Some(*v))
            .map(|(_, v)| v.1)
            .sum();
        heads[0].1["title"] = "changed".into();
        let start = Instant::now();
        store::save_cached(dir.path(), || (vec![], heads, vec![])).unwrap();
        let change_ms = start.elapsed().as_millis();
        let last = files(dir.path());
        let change_bytes: u64 = last
            .iter()
            .filter(|(p, v)| after.get(*p) != Some(*v))
            .map(|(_, v)| v.1)
            .sum();
        println!("sessions={count} noop_bytes={noop_bytes} noop_ms={noop_ms} changed_bytes={change_bytes} changed_ms={change_ms}");
    }
}
