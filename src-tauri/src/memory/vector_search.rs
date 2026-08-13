//! Memory 向量检索 - sqlite-vec 专业向量数据库
//!
//! ## 索引
//! 使用 [`sqlite-vec`](https://github.com/asg017/sqlite-vec) 扩展:
//! - SQLite 虚拟表 `vec0`,原生 KNN 向量检索
//! - 持久化到 SQLite 数据库文件(事务安全,无需重建索引)
//! - 纯 C 实现,预编译 .dll 仅 289KB,用 `include_bytes!` 嵌入二进制
//!
//! ## 持久化
//! 路径:`%APPDATA%\Vivian\memory\vectors.db`(SQLite 数据库文件)
//! - 表 `memory_vectors`:元数据(doc_id/memory_id/content/importance/memory_type/timestamp)
//! - 虚拟表 `vec_memory`:vec0 向量索引(维度由嵌入服务决定)
//! - 维度或模型变更时自动 DROP + 重建（不同模型的向量空间不兼容）
//!
//! ## 索引策略(项目约束)
//! 仅对满足以下任一条件的记忆建立向量索引:
//! - `importance >= 0.7`,或
//! - `memory_type ∈ {Preference, ImportantEvent, Knowledge}`

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use once_cell::sync::OnceCell;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::memory::types::MemoryType;

/// 向量索引准入阈值:importance >= 0.7
pub const INDEX_IMPORTANCE_THRESHOLD: f64 = 0.7;

/// vec0.dll 嵌入二进制(Windows x86_64 预编译,289KB)
#[cfg(target_arch = "x86_64")]
const VEC0_DLL: &[u8] = include_bytes!("../../vendor/sqlite-vec/vec0.dll");

/// 全局 vec0.dll 释放路径(进程级单例,避免重复释放)
static VEC0_DLL_PATH: OnceCell<PathBuf> = OnceCell::new();

/// 持久化结构:元数据行
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryVector {
    pub doc_id: String,
    pub memory_id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub importance: f64,
    pub memory_type: String,
    pub timestamp: f64,
}

/// Memory 向量存储 - 基于 sqlite-vec
///
/// 线程安全:`Connection` 不是 Sync,用 `Mutex` 保护。
/// 持久化:SQLite 数据库文件,事务安全,启动即用(无需重建索引)。
pub struct MemoryVectorStore {
    conn: Mutex<Connection>,
    dimension: usize,
    /// 当前嵌入模型 ID（写入向量时记录，用于模型切换后的增量/断点续传重建）
    model_name: String,
    /// 外部向量库模式：Some 时所有向量操作走 Qdrant，否则使用内置 sqlite-vec。
    /// `conn` 仍保留（仅用于持久化 dimension/model 元数据与模型切换检测）。
    qdrant: Option<super::qdrant::QdrantClient>,
    /// 外部集合名
    collection: String,
}

impl MemoryVectorStore {
    pub fn new(persistence_path: PathBuf, dimension: usize, model_name: &str) -> VivianResult<Self> {
        Self::open(persistence_path, dimension, model_name, None)
    }

    pub fn load_from(path: &Path, dimension: usize, model_name: &str) -> VivianResult<Self> {
        Self::open(path.to_path_buf(), dimension, model_name, None)
    }

    /// 按配置打开向量库：`config.source == "external"` 时使用外部 Qdrant，否则内置 sqlite-vec。
    pub fn open_configured(
        persistence_path: PathBuf,
        dimension: usize,
        model_name: &str,
        config: &crate::config::manager::VectorStoreConfig,
    ) -> VivianResult<Self> {
        let external = if config.source == "external" && !config.external_url.trim().is_empty() {
            Some(super::qdrant::QdrantClient::new(
                config.external_url.trim().to_string(),
                config.api_key.trim().to_string(),
            ))
        } else {
            None
        };
        Self::open(
            persistence_path,
            dimension,
            model_name,
            external.map(|q| (q, config)),
        )
    }

    fn open(
        persistence_path: PathBuf,
        dimension: usize,
        model_name: &str,
        external: Option<(super::qdrant::QdrantClient, &crate::config::manager::VectorStoreConfig)>,
    ) -> VivianResult<Self> {
        if let Some(parent) = persistence_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VivianError::Memory(format!("创建向量目录失败: {e}")))?;
        }

        let conn = Connection::open_with_flags(
            &persistence_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| VivianError::Memory(format!("打开 SQLite 失败: {e}")))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| VivianError::Memory(format!("设置 WAL 模式失败: {e}")))?;

        // 元数据表（始终存在，用于持久化 dimension/model 与模型切换检测）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vector_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| VivianError::Memory(format!("创建元数据表失败: {e}")))?;

        // 检测维度/模型变更（两者都会使向量空间不兼容，需重建）
        let stored_dim = Self::get_stored_dimension(&conn)?;
        let stored_model = Self::get_stored_model(&conn)?;
        let needs_rebuild = match (&stored_dim, &stored_model) {
            (Some(stored), _) if *stored != dimension => {
                tracing::warn!("向量维度变更: {} → {},重建索引", stored, dimension);
                true
            }
            (_, Some(stored)) if stored != model_name => {
                tracing::warn!("嵌入模型变更: {} → {},重建索引", stored, model_name);
                true
            }
            _ => false,
        };

        // 外部模式：模型/维度变更时 drop 集合；随后确保集合存在
        let (qdrant, collection) = match external {
            Some((client, cfg)) => {
                let collection = if cfg.collection.trim().is_empty() {
                    "vivian_memories".to_string()
                } else {
                    cfg.collection.trim().to_string()
                };
                if needs_rebuild {
                    let _ = client.drop_collection(&collection);
                }
                client.ensure_collection(&collection, dimension, cfg.hnsw_m, cfg.ef_construction)?;
                (Some(client), collection)
            }
            None => {
                // 本地模式：加载 vec0 扩展，检测重建时 DROP 旧表
                Self::load_vec_extension(&conn)?;
                if needs_rebuild {
                    conn.execute_batch("DROP TABLE IF EXISTS memory_vectors; DROP TABLE IF EXISTS vec_memory;")
                        .map_err(|e| VivianError::Memory(format!("DROP 旧表失败: {e}")))?;
                }
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS memory_vectors (
                        doc_id TEXT PRIMARY KEY,
                        memory_id TEXT NOT NULL,
                        content TEXT NOT NULL,
                        importance REAL NOT NULL,
                        memory_type TEXT NOT NULL,
                        timestamp REAL NOT NULL,
                        model TEXT NOT NULL DEFAULT ''
                    );
                    CREATE INDEX IF NOT EXISTS idx_memory_id ON memory_vectors(memory_id);
                    CREATE INDEX IF NOT EXISTS idx_memory_model ON memory_vectors(model);",
                )
                .map_err(|e| VivianError::Memory(format!("创建元数据表失败: {e}")))?;
                // 旧库升级：补齐 model 列
                Self::ensure_model_column(&conn)?;
                let create_vec_sql = format!(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS vec_memory USING vec0(embedding float[{}], doc_id text);",
                    dimension
                );
                conn.execute_batch(&create_vec_sql)
                    .map_err(|e| VivianError::Memory(format!("创建 vec0 虚拟表失败: {e}")))?;
                (None, String::new())
            }
        };

        // 存储当前维度和模型名
        conn.execute(
            "INSERT OR REPLACE INTO vector_meta(key, value) VALUES('dimension', ?1)",
            [dimension.to_string()],
        )
        .map_err(|e| VivianError::Memory(format!("存储维度失败: {e}")))?;
        conn.execute(
            "INSERT OR REPLACE INTO vector_meta(key, value) VALUES('model', ?1)",
            [model_name.to_string()],
        )
        .map_err(|e| VivianError::Memory(format!("存储模型名失败: {e}")))?;

        Ok(Self {
            conn: Mutex::new(conn),
            dimension,
            model_name: model_name.to_string(),
            qdrant,
            collection,
        })
    }

    /// 加载 sqlite-vec 扩展(释放 vec0.dll 到临时目录后加载)
    fn load_vec_extension(conn: &Connection) -> VivianResult<()> {
        let dll_path = VEC0_DLL_PATH
            .get_or_try_init(|| -> VivianResult<PathBuf> {
                let temp_dir = std::env::temp_dir().join("vivian_vec0");
                std::fs::create_dir_all(&temp_dir)
                    .map_err(|e| VivianError::Memory(format!("创建临时目录失败: {e}")))?;
                let dll_path = temp_dir.join("vec0.dll");
                if !dll_path.exists() || dll_path.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
                    std::fs::write(&dll_path, VEC0_DLL)
                        .map_err(|e| VivianError::Memory(format!("释放 vec0.dll 失败: {e}")))?;
                    tracing::info!("[sqlite-vec] vec0.dll 释放到 {:?}", dll_path);
                }
                Ok(dll_path)
            })?
            .clone();

        // rusqlite 的 load_extension 接受不带扩展名的路径
        // 安全性:vec0.dll 来自 sqlite-vec 官方 release,签名固定,无外部输入
        let path_str = dll_path
            .to_str()
            .ok_or_else(|| VivianError::Memory("vec0.dll 路径转换失败".into()))?;
        let path_without_ext = path_str.trim_end_matches(".dll");

        unsafe {
            conn.load_extension(path_without_ext, None)
                .map_err(|e| VivianError::Memory(format!("加载 vec0 扩展失败: {e}")))?;
        }
        tracing::info!("[sqlite-vec] vec0 扩展加载成功");
        Ok(())
    }

    /// 读取已存储的维度
    fn get_stored_dimension(conn: &Connection) -> VivianResult<Option<usize>> {
        let table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='vector_meta')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists {
            return Ok(None);
        }
        let dim_str: Option<String> = conn
            .query_row(
                "SELECT value FROM vector_meta WHERE key='dimension'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(dim_str.and_then(|s| s.parse().ok()))
    }

    /// 旧库升级：为 memory_vectors 表补齐 model 列（老版本无此列）
    fn ensure_model_column(conn: &Connection) -> VivianResult<()> {
        let has_model: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_vectors') WHERE name='model'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_model {
            conn.execute(
                "ALTER TABLE memory_vectors ADD COLUMN model TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(|e| VivianError::Memory(format!("补列 model 失败: {e}")))?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_memory_model ON memory_vectors(model)",
                [],
            )
            .map_err(|e| VivianError::Memory(format!("创建 model 索引失败: {e}")))?;
            tracing::info!("[sqlite-vec] 已为旧库 memory_vectors 补齐 model 列");
        }
        Ok(())
    }

    /// 读取已存储的嵌入模型名（用于检测模型变更）
    fn get_stored_model(conn: &Connection) -> VivianResult<Option<String>> {
        let table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='vector_meta')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists {
            return Ok(None);
        }
        let model_str: Option<String> = conn
            .query_row(
                "SELECT value FROM vector_meta WHERE key='model'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(model_str)
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    pub fn len(&self) -> usize {
        if let Some(q) = &self.qdrant {
            return q.count_points(&self.collection).unwrap_or(0);
        }
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM memory_vectors", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// 返回向量库中所有 doc_id（用于孤儿向量清理）
    pub fn all_doc_ids(&self) -> Vec<String> {
        if let Some(q) = &self.qdrant {
            return match q.scroll_doc_ids(&self.collection) {
                Ok(rows) => rows.into_iter().map(|(doc_id, _, _)| doc_id).collect(),
                Err(_) => Vec::new(),
            };
        }
        let conn = self.conn.lock();
        let mut stmt = match conn.prepare("SELECT doc_id FROM memory_vectors") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| row.get::<_, String>(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 返回已由指定模型嵌入的 doc_id 集合（用于增量/断点续传重建：跳过已用当前模型嵌入的条目）
    pub fn doc_ids_with_model(&self, model: &str) -> std::collections::HashSet<String> {
        if let Some(q) = &self.qdrant {
            return match q.scroll_doc_ids(&self.collection) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|(_, _, m)| m == model)
                    .map(|(doc_id, _, _)| doc_id)
                    .collect(),
                Err(_) => std::collections::HashSet::new(),
            };
        }
        let conn = self.conn.lock();
        let mut stmt = match conn.prepare("SELECT doc_id FROM memory_vectors WHERE model=?1") {
            Ok(s) => s,
            Err(_) => return std::collections::HashSet::new(),
        };
        stmt.query_map([model], |row| row.get::<_, String>(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 当前模型的标识（供增量重建判定）
    pub fn model(&self) -> &str {
        &self.model_name
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 添加/更新向量(事务原子操作)
    pub fn add(&self, vec: MemoryVector) -> VivianResult<()> {
        if let Some(q) = &self.qdrant {
            return q.upsert_points(&self.collection, std::slice::from_ref(&vec), &self.model_name);
        }
        let conn = self.conn.lock();
        let dim = self.dimension;

        // 序列化向量为字节(blob)
        let emb_bytes: Vec<u8> = vec
            .embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        let tx = conn.unchecked_transaction().ok();
        conn.execute(
            "INSERT OR REPLACE INTO memory_vectors(doc_id, memory_id, content, importance, memory_type, timestamp, model)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                vec.doc_id,
                vec.memory_id,
                vec.content,
                vec.importance,
                vec.memory_type,
                vec.timestamp,
                self.model_name,
            ],
        )
        .map_err(|e| VivianError::Tool(format!("向量插入失败: {}", e)))?;
        // 先删除该 doc_id 已有的向量行，避免 vec0 表因 INSERT OR REPLACE 不按 doc_id 去重
        // 而在反复重建时累积重复行（尤其 IndexDrift 周期性全量重建场景）。
        conn.execute("DELETE FROM vec_memory WHERE doc_id=?1", [&vec.doc_id])
            .map_err(|e| VivianError::Tool(format!("向量清理失败: {}", e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO vec_memory(rowid, embedding, doc_id)
             VALUES((SELECT rowid FROM memory_vectors WHERE doc_id=?1), ?2, ?1)",
            rusqlite::params![vec.doc_id, emb_bytes],
        )
        .map_err(|e| VivianError::Tool(format!("向量插入失败: {}", e)))?;
        if let Some(t) = tx {
            t.commit()
                .map_err(|e| VivianError::Tool(format!("事务提交失败: {}", e)))?;
        }
        let _ = dim;
        Ok(())
    }

    pub fn delete(&self, doc_id: &str) -> VivianResult<()> {
        if let Some(q) = &self.qdrant {
            return q.delete_points(&self.collection, &[doc_id.to_string()]);
        }
        let conn = self.conn.lock();
        let rowid: i64 = match conn
            .query_row(
                "SELECT rowid FROM memory_vectors WHERE doc_id=?1",
                [doc_id],
                |row| row.get(0),
            )
            .ok()
        {
            Some(r) => r,
            None => return Ok(()),
        };
        conn.execute("DELETE FROM vec_memory WHERE rowid=?1", [rowid])
            .map_err(|e| VivianError::Tool(format!("向量删除失败: {}", e)))?;
        conn.execute("DELETE FROM memory_vectors WHERE doc_id=?1", [doc_id])
            .map_err(|e| VivianError::Tool(format!("向量删除失败: {}", e)))?;
        Ok(())
    }

    /// 彻底移除某 doc_id 的所有向量行（含 vec0 表可能存在的重复行）与元数据。
    /// 用于孤儿向量清理，避免历史重建造成的重复行残留。
    pub fn remove(&self, doc_id: &str) -> VivianResult<()> {
        if let Some(q) = &self.qdrant {
            return q.delete_points(&self.collection, &[doc_id.to_string()]);
        }
        let conn = self.conn.lock();
        conn.execute("DELETE FROM vec_memory WHERE doc_id=?1", [doc_id])
            .map_err(|e| VivianError::Tool(format!("向量删除失败: {}", e)))?;
        conn.execute("DELETE FROM memory_vectors WHERE doc_id=?1", [doc_id])
            .map_err(|e| VivianError::Tool(format!("向量删除失败: {}", e)))?;
        Ok(())
    }

    pub fn delete_by_memory_id(&self, memory_id: &str) -> bool {
        if let Some(q) = &self.qdrant {
            // 滚动所有点，按 payload.memory_id 过滤后删除
            let doc_ids: Vec<String> = match q.scroll_doc_ids(&self.collection) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|(_, m, _)| m == memory_id)
                    .map(|(doc_id, _, _)| doc_id)
                    .collect(),
                Err(_) => Vec::new(),
            };
            if doc_ids.is_empty() {
                return false;
            }
            return q.delete_points(&self.collection, &doc_ids).is_ok();
        }
        let conn = self.conn.lock();
        let doc_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT doc_id FROM memory_vectors WHERE memory_id=?1")
                .ok();
            match &mut stmt {
                Some(s) => s
                    .query_map([memory_id], |row| row.get(0))
                    .ok()
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
                    .unwrap_or_default(),
                None => return false,
            }
        };
        let mut any_deleted = false;
        for doc_id in doc_ids {
            let rowid: Option<i64> = conn
                .query_row(
                    "SELECT rowid FROM memory_vectors WHERE doc_id=?1",
                    [&doc_id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(r) = rowid {
                let _ = conn.execute("DELETE FROM vec_memory WHERE rowid=?1", [r]);
            }
            if conn
                .execute("DELETE FROM memory_vectors WHERE doc_id=?1", [&doc_id])
                .unwrap_or(0)
                > 0
            {
                any_deleted = true;
            }
        }
        any_deleted
    }

    pub fn clear(&self) -> VivianResult<()> {
        if let Some(q) = &self.qdrant {
            return q.clear_points(&self.collection);
        }
        let conn = self.conn.lock();
        conn.execute("DELETE FROM memory_vectors", [])
            .map_err(|e| VivianError::Tool(format!("向量清空失败: {}", e)))?;
        conn.execute("DELETE FROM vec_memory", [])
            .map_err(|e| VivianError::Tool(format!("向量清空失败: {}", e)))?;
        Ok(())
    }

    /// KNN 向量检索
    ///
    /// 返回 `(doc_id, memory_id, score)` 列表,按分数降序排列,最多 `k` 个。
    /// 分数为余弦相似度(sqlite-vec 的 vec_distance_cosine 返回的是距离,转换为相似度)。
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(String, String, f64)> {
        if query.len() != self.dimension {
            return Vec::new();
        }
        if let Some(q) = &self.qdrant {
            return q.search(&self.collection, query, k, None).unwrap_or_default();
        }

        let conn = self.conn.lock();
        let query_bytes: Vec<u8> = query.iter().flat_map(|f| f.to_le_bytes()).collect();

        let sql = "SELECT v.doc_id, m.memory_id, v.distance
                   FROM vec_memory v
                   JOIN memory_vectors m ON v.doc_id = m.doc_id
                   WHERE v.embedding MATCH ?1
                   AND k = ?2
                   ORDER BY v.distance";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[sqlite-vec] 查询准备失败: {e}");
                return Vec::new();
            }
        };

        let rows = stmt
            .query_map(rusqlite::params![query_bytes, k as i64], |row| {
                let doc_id: String = row.get(0)?;
                let memory_id: String = row.get(1)?;
                let distance: f64 = row.get(2)?;
                // cosine distance ∈ [0, 2], 转换为相似度 ∈ [-1, 1]
                let similarity = 1.0 - distance;
                Ok((doc_id, memory_id, similarity))
            })
            .ok();

        let mut results: Vec<(String, String, f64)> = rows
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        // 过滤相似度 <= 0 的结果
        results.retain(|(_, _, s)| *s > 0.0);
        results
    }

    pub fn save_to(&self) -> VivianResult<()> {
        // 外部模式：Qdrant 服务端自动持久化，无需本地落盘
        if self.qdrant.is_some() {
            return Ok(());
        }
        // SQLite 自动持久化,无需显式 save
        // WAL 模式下 checkpoint 确保数据落盘
        let conn = self.conn.lock();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| VivianError::Memory(format!("WAL checkpoint 失败: {e}")))?;
        Ok(())
    }

    pub fn deleted_count(&self) -> usize {
        0 // sqlite-vec / Qdrant 物理删除,无需标记
    }

    pub fn rebuild_index(&self) {
        // 本地 sqlite-vec 与外部 Qdrant(HNSW) 均自动维护索引,无需重建
        tracing::debug!("[vector] 索引由数据库自动维护,无需重建");
    }

    /// 后端类型（供日志/诊断区分）
    pub fn backend_name(&self) -> &'static str {
        if self.qdrant.is_some() {
            "qdrant"
        } else {
            "sqlite-vec"
        }
    }
}

/// 判断记忆是否应建立向量索引
pub fn should_index(importance: f64, memory_type: &MemoryType) -> bool {
    if importance >= INDEX_IMPORTANCE_THRESHOLD {
        return true;
    }
    matches!(
        memory_type,
        MemoryType::Preference | MemoryType::ImportantEvent | MemoryType::Knowledge
    )
}

/// 余弦相似度(保留用于回退路径)
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na * nb)) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vec(doc_id: &str, memory_id: &str, emb: Vec<f32>) -> MemoryVector {
        MemoryVector {
            doc_id: doc_id.to_string(),
            memory_id: memory_id.to_string(),
            content: format!("content-{doc_id}"),
            embedding: emb,
            importance: 0.8,
            memory_type: "preference".to_string(),
            timestamp: 0.0,
        }
    }

    #[test]
    fn test_should_index_by_importance() {
        assert!(should_index(0.7, &MemoryType::General));
        assert!(should_index(0.95, &MemoryType::ShortTerm));
        assert!(!should_index(0.69, &MemoryType::General));
    }

    #[test]
    fn test_should_index_by_type() {
        assert!(should_index(0.1, &MemoryType::Preference));
        assert!(should_index(0.1, &MemoryType::ImportantEvent));
        assert!(should_index(0.1, &MemoryType::Knowledge));
        assert!(!should_index(0.1, &MemoryType::General));
        assert!(!should_index(0.1, &MemoryType::ShortTerm));
    }

    #[test]
    fn test_cosine_similarity_edge_cases() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0], &[0.0]), 0.0);
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}
