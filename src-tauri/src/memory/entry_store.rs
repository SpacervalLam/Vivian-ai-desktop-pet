//! 记忆条目 SQLite 存储：行级 upsert/delete，替代全量 JSON 重写。
//!
//! - 表 `entries(id TEXT PRIMARY KEY, json TEXT NOT NULL)`：每条记忆一行，
//!   json 为 MemoryItem 的紧凑序列化
//! - 元数据（version）存 `meta` 表
//! - 旧版 unified_memory.json 首次打开时自动迁移，原文件重命名为 .migrated
//! - 明文镜像：默认关闭；显式设置 `VIVIAN_MEMORY_PLAIN_MIRROR=1` 后写入
//!   `plain/<id>.txt`，删除时同步移除

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
    /// WAL 文件路径（entries.db-wal），用于按体积触发 checkpoint
    wal_path: PathBuf,
    /// Human-readable mirrors duplicate private content and are therefore opt-in.
    plain_mirror_enabled: bool,
}

/// SQLite 页缓存上限（KB）。负值表示以 KiB 计的软上限。
///
/// 默认的 2MB 页缓存会随查询不断扩张；记忆库是行级 JSON 存储，
/// 4MB 足够，多出来的缓存只是挤占常驻内存。
const SQLITE_CACHE_SIZE_KIB: i64 = -4096;

/// WAL 体积超过此阈值时触发 TRUNCATE checkpoint（字节）。
///
/// 不做 checkpoint 的话 WAL 只增不减：实测主库 4KB 而 WAL 已涨到 2MB，
/// 每次启动都要把整个 WAL 回放一遍再全量 load_all。
const WAL_CHECKPOINT_THRESHOLD: u64 = 1024 * 1024;

impl MemoryEntryStore {
    /// 打开（或创建）条目存储；legacy_json 存在且数据库为空时执行迁移
    pub fn open(db_path: PathBuf, legacy_json: &Path) -> VivianResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VivianError::Memory(format!("创建记忆目录失败: {e}")))?;
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| VivianError::Memory(format!("打开记忆数据库失败: {e}")))?;
        // PRAGMA 不支持绑定参数，只能拼字符串
        conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size={};",
            SQLITE_CACHE_SIZE_KIB
        ))
        .map_err(|e| VivianError::Memory(format!("设置数据库参数失败: {e}")))?;
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
        let plain_mirror_enabled = std::env::var("VIVIAN_MEMORY_PLAIN_MIRROR")
            .ok()
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
        if plain_mirror_enabled {
            let _ = std::fs::create_dir_all(&plain_dir);
        }

        // SQLite 的 WAL 路径固定为主库路径 + "-wal"
        let mut wal_path = db_path.clone().into_os_string();
        wal_path.push("-wal");

        let store = Self {
            conn: Mutex::new(conn),
            plain_dir,
            wal_path: PathBuf::from(wal_path),
            plain_mirror_enabled,
        };

        // 启动时先把历史遗留的 WAL 收进主库，避免 load_all 回放一大段 WAL
        store.checkpoint_if_needed(0);

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

    /// WAL 体积超过 `threshold` 时执行 TRUNCATE checkpoint。
    ///
    /// `threshold = 0` 表示无条件执行（启动时用，用于回收历史遗留的 WAL）。
    /// checkpoint 失败只记日志不返回错误——它纯粹是空间回收，不影响数据正确性。
    fn checkpoint_if_needed(&self, threshold: u64) {
        if threshold > 0 {
            let wal_size = std::fs::metadata(&self.wal_path)
                .map(|m| m.len())
                .unwrap_or(0);
            if wal_size < threshold {
                return;
            }
        }
        let conn = self.conn.lock();
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            tracing::warn!("[MemoryEntryStore] WAL checkpoint 失败（不影响数据）: {e}");
        }
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
        // WAL 体积超阈值时收进主库（阈值传 0 表示无条件检查，此处按实际阈值）
        self.checkpoint_if_needed(WAL_CHECKPOINT_THRESHOLD);
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
                        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
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
