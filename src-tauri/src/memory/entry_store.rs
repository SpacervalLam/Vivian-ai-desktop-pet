//! 记忆条目 SQLite 存储：行级 upsert/delete，替代全量 JSON 重写。
//!
//! - 表 `entries(id TEXT PRIMARY KEY, json TEXT NOT NULL)`：每条记忆一行，
//!   json 为 MemoryItem 的紧凑序列化
//! - 元数据（version）存 `meta` 表
//! - 旧版 unified_memory.json 首次打开时自动迁移，原文件重命名为 .migrated
//! - 明文镜像：新条目写入 `plain/<id>.txt`（人可读），删除时同步移除

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::Connection;

use super::types::{MemoryItem, MemoryStoreData};
use crate::error::{VivianError, VivianResult};

pub struct MemoryEntryStore {
    conn: Mutex<Connection>,
    /// 明文镜像目录（与数据库同级的 plain/ 子目录）
    plain_dir: PathBuf,
}

impl MemoryEntryStore {
    /// 打开（或创建）条目存储；legacy_json 存在且数据库为空时执行迁移
    pub fn open(db_path: PathBuf, legacy_json: &Path) -> VivianResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VivianError::Memory(format!("创建记忆目录失败: {e}")))?;
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| VivianError::Memory(format!("打开记忆数据库失败: {e}")))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| VivianError::Memory(format!("设置 WAL 模式失败: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                id TEXT PRIMARY KEY,
                json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| VivianError::Memory(format!("创建记忆表失败: {e}")))?;

        let plain_dir = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plain");
        let _ = std::fs::create_dir_all(&plain_dir);

        let store = Self {
            conn: Mutex::new(conn),
            plain_dir,
        };

        // 旧版 JSON 迁移：数据库无条目且 legacy 存在时导入
        let is_empty = store.entry_count()? == 0;
        if is_empty && legacy_json.exists() {
            if let Ok(content) = std::fs::read_to_string(legacy_json) {
                if let Ok(data) = serde_json::from_str::<MemoryStoreData>(&content) {
                    if !data.entries.is_empty() {
                        store.write_rows(
                            data.entries.iter().map(|e| (e.id.clone(), e.clone())).collect(),
                            &[],
                        )?;
                        store.set_meta("version", &data.version.to_string())?;
                        let migrated = legacy_json.with_extension("json.migrated");
                        let _ = std::fs::rename(legacy_json, migrated);
                        tracing::info!(
                            "[MemoryEntryStore] 已迁移 {} 条记忆到 SQLite，旧文件保留为 .migrated",
                            data.entries.len()
                        );
                    }
                }
            }
        }
        Ok(store)
    }

    fn entry_count(&self) -> VivianResult<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .map_err(|e| VivianError::Memory(format!("统计记忆条数失败: {e}")))?;
        Ok(n as usize)
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
        Ok(items)
    }

    /// 单事务内执行行级 upsert/delete，并维护明文镜像
    ///
    /// 明文镜像仅在条目首次出现时写入（内容创建时刻的快照），
    /// 后续 importance/visit 等状态漂移不重写镜像文件。
    pub fn write_rows(
        &self,
        upserts: Vec<(String, MemoryItem)>,
        deletes: &[String],
    ) -> VivianResult<()> {
        if upserts.is_empty() && deletes.is_empty() {
            return Ok(());
        }
        // 明文镜像：删除的条目移除镜像文件
        for id in deletes {
            let _ = std::fs::remove_file(self.plain_dir.join(format!("{id}.txt")));
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
        // 明文镜像：新条目写镜像（已存在则跳过）
        for (id, item) in &upserts {
            let path = self.plain_dir.join(format!("{id}.txt"));
            if path.exists() {
                continue;
            }
            let text = format!(
                "类型：{}\n重要度：{:.2}\n时间：{}\n标签：{}\n\n{}",
                item.memory_type,
                item.importance,
                chrono::DateTime::<chrono::Utc>::from_timestamp(item.timestamp as i64, 0)
                    .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default(),
                item.tags.join(", "),
                item.content,
            );
            let _ = std::fs::write(path, text);
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
