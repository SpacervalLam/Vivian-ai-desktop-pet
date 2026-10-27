//! 记忆条目 SQLite 存储：行级 upsert/delete，替代全量 JSON 重写。
//!
//! - 表 `entries(id TEXT PRIMARY KEY, json TEXT NOT NULL)`：每条记忆一行，
//!   json 为 MemoryItem 的紧凑序列化
//! - 元数据（version）存 `meta` 表
//! - 明文镜像：默认关闭；显式设置 `VIVIAN_MEMORY_PLAIN_MIRROR=1` 后写入
//!   `plain/<id>.txt`，删除时同步移除

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::Connection;

use super::types::{MemoryItem, MemoryRetrievalFilter};
use crate::error::{VivianError, VivianResult};

pub struct MemoryEntryStore {
    conn: Mutex<Connection>,
    /// 明文镜像目录（与数据库同级的 plain/ 子目录）
    plain_dir: PathBuf,
    /// Human-readable mirrors duplicate private content and are therefore opt-in.
    plain_mirror_enabled: bool,
}

/// Pending IDs survive a failed transaction; committing never scans untouched rows.
#[derive(Default)]
pub struct EntryChanges {
    upserts: HashSet<String>,
    deletes: HashSet<String>,
}

impl EntryChanges {
    pub fn changed(&mut self, id: &str) {
        self.deletes.remove(id);
        self.upserts.insert(id.to_owned());
    }

    pub fn removed(&mut self, id: &str) {
        self.upserts.remove(id);
        self.deletes.insert(id.to_owned());
    }

    pub fn flush(&mut self, store: &MemoryEntryStore, entries: &[MemoryItem],
        index: &HashMap<String, usize>) -> VivianResult<()> {
        let upserts = self.upserts.iter().map(|id| {
            let entry = index.get(id).and_then(|&i| entries.get(i))
                .filter(|entry| &entry.id == id)
                .ok_or_else(|| VivianError::Memory(format!("待写入记忆索引缺失: {id}")))?;
            Ok((id.clone(), entry.clone()))
        }).collect::<VivianResult<Vec<_>>>()?;
        store.write_rows(upserts, &self.deletes.iter().cloned().collect::<Vec<_>>())?;
        self.upserts.clear();
        self.deletes.clear();
        Ok(())
    }
}

impl MemoryEntryStore {
    /// 打开（或创建）条目存储。
    pub fn open(db_path: PathBuf) -> VivianResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VivianError::Memory(format!("创建记忆目录失败: {e}")))?;
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| VivianError::Memory(format!("打开记忆数据库失败: {e}")))?;
        crate::utils::sqlite::configure(&conn, crate::utils::sqlite::DataClass::Authoritative)
            .map_err(|e| VivianError::Memory(format!("设置数据库参数失败: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(|e| VivianError::Memory(format!("初始化记忆数据库失败: {e}")))?;
        conn.prepare("SELECT id, json FROM entries LIMIT 0")
            .map_err(|e| VivianError::Memory(format!("记忆表结构无效: {e}")))?;
        conn.prepare("SELECT key, value FROM meta LIMIT 0")
            .map_err(|e| VivianError::Memory(format!("记忆元数据表结构无效: {e}")))?;
        super::search_index::initialize(&conn)
            .map_err(|e| VivianError::Memory(format!("初始化记忆检索索引失败: {e}")))?;

        let plain_dir = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plain");
        let plain_mirror_enabled = std::env::var("VIVIAN_MEMORY_PLAIN_MIRROR")
            .ok()
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
        if plain_mirror_enabled {
            let _ = std::fs::create_dir_all(&plain_dir);
        }

        let store = Self {
            conn: Mutex::new(conn),
            plain_dir,
            plain_mirror_enabled,
        };
        Ok(store)
    }

    /// 写入元数据（version 等）
    pub fn set_meta(&self, key: &str, value: &str) -> VivianResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
            [key, value],
        )
        .map_err(|e| VivianError::Memory(format!("写入元数据失败: {e}")))?;
        Ok(())
    }

    /// 读取元数据（version 等）
    pub fn get_meta(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .ok()
    }

    /// 全量加载所有条目（启动时一次）
    pub fn load_all(&self) -> VivianResult<Vec<MemoryItem>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT json FROM entries")
            .map_err(|e| VivianError::Memory(format!("查询记忆失败: {e}")))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| VivianError::Memory(format!("遍历记忆失败: {e}")))?;
        let mut items = Vec::new();
        for row in rows {
            let json = row.map_err(|e| VivianError::Memory(format!("读取记忆行失败: {e}")))?;
            match serde_json::from_str::<MemoryItem>(&json) {
                Ok(item) => items.push(item),
                Err(e) => tracing::warn!("[MemoryEntryStore] 单条记忆解析失败，跳过: {}", e),
            }
        }
        // 不依赖 SQLite JSON1 扩展，兼容旧运行环境；稳定的次级键确保相同
        // timestamp 在每次启动时仍得到同一顺序，容量淘汰不会随机误删。
        items.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(items)
    }

    /// 单事务内执行行级 upsert/delete，并在提交后同步可选明文镜像。
    pub fn write_rows(
        &self,
        upserts: Vec<(String, MemoryItem)>,
        deletes: &[String],
    ) -> VivianResult<()> {
        if upserts.is_empty() && deletes.is_empty() {
            return Ok(());
        }
        {
            let conn = self.conn.lock();
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| VivianError::Memory(format!("开启事务失败: {e}")))?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO entries(id, json) VALUES(?1, ?2)
                         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                    )
                    .map_err(|e| VivianError::Memory(format!("准备写入语句失败: {e}")))?;
                for (id, item) in &upserts {
                    let json = serde_json::to_string(item)
                        .map_err(|e| VivianError::Serialization(e.to_string()))?;
                    stmt.execute([id.as_str(), json.as_str()])
                        .map_err(|e| VivianError::Memory(format!("写入记忆行失败: {e}")))?;
                    super::search_index::upsert(&tx, item)
                        .map_err(|e| VivianError::Memory(format!("更新记忆检索索引失败: {e}")))?;
                }
            }
            {
                let mut stmt = tx
                    .prepare("DELETE FROM entries WHERE id = ?1")
                    .map_err(|e| VivianError::Memory(format!("准备删除语句失败: {e}")))?;
                for id in deletes {
                    stmt.execute([id.as_str()])
                        .map_err(|e| VivianError::Memory(format!("删除记忆行失败: {e}")))?;
                }
            }
            tx.commit()
                .map_err(|e| VivianError::Memory(format!("提交事务失败: {e}")))?;
        }
        // Mirrors are derived data: update them only after the canonical transaction
        // commits, so a failed DB write cannot delete the last readable copy first.
        for id in deletes {
            let _ = std::fs::remove_file(self.plain_dir.join(format!("{id}.txt")));
        }
        // Shared SQLite policy performs passive automatic checkpoints. Durable
        // commits do not require forcing readers out with a TRUNCATE checkpoint.
        // Plaintext mirrors are opt-in because they duplicate private memory outside
        // the canonical database. Existing mirrors are not deleted automatically.
        if self.plain_mirror_enabled {
            for (id, item) in &upserts {
                let path = self.plain_dir.join(format!("{id}.txt"));
                let text = format!(
                    "类型：{}\n重要度：{:.2}\n时间：{}\n标签：{}\n\n{}",
                    item.memory_type,
                    item.importance,
                    chrono::DateTime::<chrono::Utc>::from_timestamp(item.timestamp as i64, 0)
                        .map(|dt| dt
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string())
                        .unwrap_or_default(),
                    item.tags.join(", "),
                    item.content,
                );
                if let Err(e) = crate::utils::fs::write_atomic(&path, &text) {
                    tracing::warn!("[MemoryEntryStore] 明文镜像写入失败（数据库已提交）: {e}");
                }
            }
        }
        Ok(())
    }

    /// 清空全部条目与明文镜像
    pub fn clear_all(&self) -> VivianResult<()> {
        {
            let conn = self.conn.lock();
            conn.execute("DELETE FROM entries", [])
                .map_err(|e| VivianError::Memory(format!("清空记忆失败: {e}")))?;
        }
        if self.plain_dir.exists() {
            if let Ok(files) = std::fs::read_dir(&self.plain_dir) {
                for f in files.filter_map(|e| e.ok()) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        Ok(())
    }

    pub fn search_candidates(&self, query: &str, owner: &str,
        filter: &MemoryRetrievalFilter, limit: usize) -> VivianResult<Vec<(String,f64)>> {
        super::search_index::search(&self.conn.lock(), query, owner, filter, limit)
            .map_err(|error| VivianError::Memory(format!("检索记忆候选失败: {error}")))
    }

    pub fn load_by_ids(&self, ids: &[String], owner: Option<&str>,
        filter: &MemoryRetrievalFilter) -> VivianResult<Vec<MemoryItem>> {
        if ids.is_empty() {return Ok(Vec::new());}
        super::search_index::load_ids(&self.conn.lock(),ids,owner,filter)
            .map_err(|error| VivianError::Memory(format!("按需读取记忆失败: {error}")))
    }

    pub fn context_neighbors(&self, hits: &[MemoryItem], owner: &str, limit: usize) -> VivianResult<Vec<MemoryItem>> {
        if limit == 0 || hits.is_empty() {return Ok(Vec::new());}
        super::search_index::neighbors(&self.conn.lock(),hits,owner,limit)
            .map_err(|error| VivianError::Memory(format!("读取记忆上下文失败: {error}")))
    }
}

/// 计算条目的内容指纹（用于差异比对；compact 序列化后哈希）
pub fn entry_fingerprint(item: &MemoryItem) -> u64 {
    let json = serde_json::to_string(item).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

/// 批量指纹（id → fingerprint）
pub fn fingerprint_all(entries: &[MemoryItem]) -> HashMap<String, u64> {
    entries
        .iter()
        .map(|e| (e.id.clone(), entry_fingerprint(e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_delta_commit_is_atomic_and_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
        let entries: Vec<_> = ["a", "b"].iter().map(|id| {
            let mut item = MemoryItem::new(id.to_string(), super::super::types::Granularity::Summary, 0.8);
            item.id = id.to_string(); item
        }).collect();
        let index = entries.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        let mut changes = EntryChanges::default();
        changes.changed("a"); changes.changed("b");
        store.conn.lock().execute_batch("CREATE TRIGGER reject_b BEFORE INSERT ON entries WHEN NEW.id='b' BEGIN SELECT RAISE(FAIL,'test failure'); END;").unwrap();
        assert!(changes.flush(&store, &entries, &index).is_err());
        assert!(store.load_all().unwrap().is_empty());
        assert_eq!(changes.upserts.len(), 2);
        store.conn.lock().execute_batch("DROP TRIGGER reject_b").unwrap();
        changes.flush(&store, &entries, &index).unwrap();
        assert_eq!(store.load_all().unwrap().len(), 2);
        assert!(changes.upserts.is_empty());
        changes.removed("a"); changes.changed("a");
        changes.flush(&store, &entries, &index).unwrap();
        assert_eq!(store.load_all().unwrap().len(), 2);
        changes.removed("a"); changes.flush(&store, &entries, &index).unwrap();
        assert_eq!(store.load_all().unwrap()[0].id, "b");
    }

    #[test]
    fn storage_entries_create_current_schema_and_reopen_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entries.db");
        let store = MemoryEntryStore::open(path.clone()).unwrap();
        store.set_meta("version", "4").unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute("INSERT INTO entries VALUES ('retained', '{}')", [])
            .unwrap();
        drop(conn);
        let store = MemoryEntryStore::open(path).unwrap();
        assert_eq!(store.get_meta("version").as_deref(), Some("4"));
        let count: i64 = store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
