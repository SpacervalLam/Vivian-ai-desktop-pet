//! 知识图谱存储 — typed edges + 实体节点
//!
//! 设计：
//! - 实体节点（name + type + salience）
//! - typed edges（source → target + relation_type + weight + source_memory_id）
//! - 内存 HashMap 存储 + JSON 持久化
//!
//! 设计取舍：
//! - 用内存 HashMap + JSON 文件
//! - 实体从记忆内容自动抽取
//! - 增加 source_memory_id 溯源（每条边关联到来源记忆）

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::entity_extract::{EntityType, RelationType};

/// 图谱实体节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntity {
    /// 实体名（唯一键）
    pub name: String,
    /// 实体类型
    pub entity_type: EntityType,
    /// 显著性分数（0-1）
    pub salience: f64,
    /// 关联的记忆 ID 列表
    pub memory_ids: Vec<String>,
    /// 创建时间戳
    pub created_at: f64,
    /// 最近更新时间戳
    pub updated_at: f64,
}

/// 图谱边（typed edge）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// 主体实体名
    pub source: String,
    /// 客体实体名
    pub target: String,
    /// 关系类型
    pub relation_type: RelationType,
    /// 边权重（0-1，取自关系置信度）
    pub weight: f64,
    /// 来源记忆 ID（溯源）
    pub source_memory_id: String,
    /// 上下文片段
    pub context: String,
    /// 创建时间戳
    pub created_at: f64,
}

/// 图谱存储数据（持久化到 JSON）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphStoreData {
    /// schema 版本
    pub version: u32,
    /// 实体节点（name → GraphEntity）
    pub entities: HashMap<String, GraphEntity>,
    /// 边列表
    pub edges: Vec<GraphEdge>,
}

impl GraphStoreData {
    pub fn new() -> Self {
        Self {
            version: 1,
            entities: HashMap::new(),
            edges: Vec::new(),
        }
    }
}

/// 边的稳定行键：同一 (source, target, relation) 三元组只允许一条边（与内存去重规则一致）。
fn edge_row_id(edge: &GraphEdge) -> String {
    format!("{}\u{1}{}\u{1}{:?}", edge.source, edge.target, edge.relation_type)
}

/// 行级持久化封装：实体、边各占一行，保存时仅写入新增/变化行、删除已移除行。
struct GraphDb {
    conn: Connection,
    /// id -> json 快照（实体：id=实体名）
    nodes: HashMap<String, String>,
    /// id -> json 快照（边：id=三元组行键）
    edges: HashMap<String, String>,
}

impl GraphDb {
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建图谱目录失败: {e}"))?;
        }
        let conn = Connection::open(path).map_err(|e| format!("打开图谱数据库失败: {e}"))?;
        crate::utils::sqlite::configure(&conn, crate::utils::sqlite::DataClass::Authoritative)
            .map_err(|e| format!("设置图谱数据库参数失败: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graph_nodes (id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS graph_edges (id TEXT PRIMARY KEY, json TEXT NOT NULL);",
        )
        .map_err(|e| format!("初始化图谱数据库失败: {e}"))?;
        conn.prepare("SELECT id, json FROM graph_nodes LIMIT 0")
            .map_err(|e| format!("图谱节点表结构无效: {e}"))?;
        conn.prepare("SELECT id, json FROM graph_edges LIMIT 0")
            .map_err(|e| format!("图谱边表结构无效: {e}"))?;
        let nodes = read_rows(&conn, "SELECT id, json FROM graph_nodes")?;
        let edges = read_rows(&conn, "SELECT id, json FROM graph_edges")?;
        Ok(Self { conn, nodes, edges })
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// 单事务内执行节点/边的行级 upsert/delete。
    fn apply(
        &mut self,
        node_upserts: &[(String, String)],
        node_deletes: &[String],
        edge_upserts: &[(String, String)],
        edge_deletes: &[String],
    ) -> Result<(), String> {
        if node_upserts.is_empty()
            && node_deletes.is_empty()
            && edge_upserts.is_empty()
            && edge_deletes.is_empty()
        {
            return Ok(());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("开启图谱事务失败: {e}"))?;
        write_rows(&tx, "graph_nodes", node_upserts, node_deletes)?;
        write_rows(&tx, "graph_edges", edge_upserts, edge_deletes)?;
        tx.commit().map_err(|e| format!("提交图谱事务失败: {e}"))?;
        Ok(())
    }
}

fn read_rows(conn: &Connection, sql: &str) -> Result<HashMap<String, String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, json) = row.map_err(|e| e.to_string())?;
        out.insert(id, json);
    }
    Ok(out)
}

fn write_rows(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    upserts: &[(String, String)],
    deletes: &[String],
) -> Result<(), String> {
    if !upserts.is_empty() {
        let sql = format!(
            "INSERT INTO {table}(id, json) VALUES(?1, ?2)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json"
        );
        let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
        for (id, json) in upserts {
            stmt.execute([id.as_str(), json.as_str()])
                .map_err(|e| format!("写入 {table} 行失败: {e}"))?;
        }
    }
    if !deletes.is_empty() {
        let sql = format!("DELETE FROM {table} WHERE id = ?1");
        let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
        for id in deletes {
            stmt.execute([id.as_str()])
                .map_err(|e| format!("删除 {table} 行失败: {e}"))?;
        }
    }
    Ok(())
}

/// 知识图谱存储
///
/// 内存 HashMap + SQLite 行级持久化。
/// 写入时更新内存 + 标记脏数据，5 秒节流落盘。
pub struct KnowledgeGraph {
    inner: Arc<RwLock<GraphStoreData>>,
    /// 行级 SQLite 存储；初始化失败时为 None（退化为纯内存态）
    db: Option<Mutex<GraphDb>>,
    dirty: Arc<RwLock<bool>>,
}

impl KnowledgeGraph {
    /// 创建新的知识图谱存储
    pub fn new(store_path: PathBuf) -> Self {
        let db_path = store_path.with_extension("db");
        let mut store = GraphStoreData::new();
        let db = match GraphDb::open(&db_path) {
            Ok(mut db) => {
                // 首次迁移：库为空且存在旧 JSON 时导入，成功后归档旧文件（不删除用户数据）
                if db.is_empty() && store_path.exists() {
                    Self::import_legacy_json(&mut db, &store_path);
                }
                for json in db.nodes.values() {
                    match serde_json::from_str::<GraphEntity>(json) {
                        Ok(entity) => {
                            store.entities.insert(entity.name.clone(), entity);
                        }
                        Err(e) => tracing::warn!("[KnowledgeGraph] 单条实体解析失败，跳过: {e}"),
                    }
                }
                let mut edge_rows: Vec<(String, GraphEdge)> = Vec::with_capacity(db.edges.len());
                for (id, json) in &db.edges {
                    match serde_json::from_str::<GraphEdge>(json) {
                        Ok(edge) => edge_rows.push((id.clone(), edge)),
                        Err(e) => tracing::warn!("[KnowledgeGraph] 单条边解析失败，跳过: {e}"),
                    }
                }
                // 行键排序，确保每次启动的边顺序稳定
                edge_rows.sort_by(|a, b| a.0.cmp(&b.0));
                store.edges = edge_rows.into_iter().map(|(_, edge)| edge).collect();
                tracing::info!(
                    "[KnowledgeGraph] 加载 {} 个实体 / {} 条边",
                    store.entities.len(),
                    store.edges.len()
                );
                Some(db)
            }
            Err(e) => {
                tracing::error!("[KnowledgeGraph] 打开图谱数据库失败，退化为仅内存态: {e}");
                None
            }
        };

        Self {
            inner: Arc::new(RwLock::new(store)),
            db: db.map(Mutex::new),
            dirty: Arc::new(RwLock::new(false)),
        }
    }

    /// 一次性迁移：把旧 `knowledge_graph.json` 导入数据库，成功后归档为 `.json.migrated`。
    ///
    /// 失败时不阻塞启动、不改动旧文件（下次启动可重试）。
    fn import_legacy_json(db: &mut GraphDb, json_path: &Path) {
        let content = match std::fs::read_to_string(json_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[KnowledgeGraph] 读取旧图谱文件失败，跳过迁移: {e}");
                return;
            }
        };
        let file_name = json_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let migrated = json_path.with_file_name(format!("{file_name}.migrated"));
        if content.trim().is_empty() {
            let _ = std::fs::rename(json_path, &migrated);
            return;
        }
        let store: GraphStoreData = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[KnowledgeGraph] 解析旧图谱文件失败，跳过迁移: {e}");
                return;
            }
        };
        if store.entities.is_empty() && store.edges.is_empty() {
            let _ = std::fs::rename(json_path, &migrated);
            return;
        }
        let node_upserts: Vec<(String, String)> = store
            .entities
            .iter()
            .filter_map(|(name, entity)| {
                serde_json::to_string(entity).ok().map(|json| (name.clone(), json))
            })
            .collect();
        let edge_upserts: Vec<(String, String)> = store
            .edges
            .iter()
            .filter_map(|edge| {
                serde_json::to_string(edge).ok().map(|json| (edge_row_id(edge), json))
            })
            .collect();
        if let Err(e) = db.apply(&node_upserts, &[], &edge_upserts, &[]) {
            tracing::warn!("[KnowledgeGraph] 旧图谱导入数据库失败: {e}");
            return;
        }
        for (id, json) in node_upserts {
            db.nodes.insert(id, json);
        }
        for (id, json) in edge_upserts {
            db.edges.insert(id, json);
        }
        tracing::info!("[KnowledgeGraph] 旧图谱迁移完成");
        if let Err(e) = std::fs::rename(json_path, &migrated) {
            tracing::warn!("[KnowledgeGraph] 归档旧图谱文件失败: {e}");
        }
    }

    /// 从记忆内容中提取实体和关系，写入图谱
    ///
    /// 返回写入的实体数和关系数。
    pub fn ingest_from_memory(
        &self,
        memory_id: &str,
        content: &str,
        timestamp: f64,
    ) -> (usize, usize) {
        let result = super::entity_extract::extract(content);

        let mut entity_count = 0;
        let mut edge_count = 0;

        let mut store = self.inner.write();

        // 写入实体
        for entity in &result.entities {
            let entry = store.entities.entry(entity.name.clone()).or_insert_with(|| {
                GraphEntity {
                    name: entity.name.clone(),
                    entity_type: entity.entity_type.clone(),
                    salience: entity.salience,
                    memory_ids: Vec::new(),
                    created_at: timestamp,
                    updated_at: timestamp,
                }
            });

            // 更新显著性（取最大值）
            if entity.salience > entry.salience {
                entry.salience = entity.salience;
            }

            // 关联记忆 ID（去重）
            if !entry.memory_ids.contains(&memory_id.to_string()) {
                entry.memory_ids.push(memory_id.to_string());
            }
            entry.updated_at = timestamp;
            entity_count += 1;
        }

        // 写入边
        for relation in &result.relations {
            let exists = store.edges.iter().any(|e| {
                e.source == relation.subject
                    && e.target == relation.object
                    && e.relation_type == relation.relation_type
            });

            if !exists {
                store.edges.push(GraphEdge {
                    source: relation.subject.clone(),
                    target: relation.object.clone(),
                    relation_type: relation.relation_type.clone(),
                    weight: relation.confidence,
                    source_memory_id: memory_id.to_string(),
                    context: relation.context.clone(),
                    created_at: timestamp,
                });
                edge_count += 1;
            }
        }

        drop(store);

        if entity_count > 0 || edge_count > 0 {
            *self.dirty.write() = true;
        }

        (entity_count, edge_count)
    }

    /// 写入概念实体（概念层 → 图谱"主题层"的接入）。
    ///
    /// 概念名作为 `EntityType::Concept` 实体节点写入图谱，并与其 `related_topics`
    /// 之间建立 `Mentions` 边，使 query 的话题词能通过图谱概念路展开到相关记忆。
    ///
    /// 幂等：重复写入只更新 `updated_at` 与关联记忆 ID，不重复建边。
    /// 返回写入的实体数与边数。
    pub fn ingest_concepts(
        &self,
        concept: &str,
        related_topics: &[String],
        memory_ids: &[String],
        timestamp: f64,
    ) -> (usize, usize) {
        let mut store = self.inner.write();
        let mut entity_count = 0;
        let mut edge_count = 0;

        let concept_name = concept.trim().to_string();
        if concept_name.is_empty() {
            return (0, 0);
        }

        let entry = store.entities.entry(concept_name.clone()).or_insert_with(|| {
            GraphEntity {
                name: concept_name.clone(),
                entity_type: EntityType::Concept,
                salience: 1.0,
                memory_ids: Vec::new(),
                created_at: timestamp,
                updated_at: timestamp,
            }
        });
        for mid in memory_ids {
            if !entry.memory_ids.contains(mid) {
                entry.memory_ids.push(mid.clone());
            }
        }
        entry.updated_at = timestamp;
        entry.salience = 1.0;
        entity_count += 1;

        for topic in related_topics {
            let topic_name = topic.trim().to_string();
            if topic_name.is_empty() || topic_name == concept_name {
                continue;
            }
            let entry = store.entities.entry(topic_name.clone()).or_insert_with(|| {
                GraphEntity {
                    name: topic_name.clone(),
                    entity_type: EntityType::Concept,
                    salience: 0.8,
                    memory_ids: Vec::new(),
                    created_at: timestamp,
                    updated_at: timestamp,
                }
            });
            for mid in memory_ids {
                if !entry.memory_ids.contains(mid) {
                    entry.memory_ids.push(mid.clone());
                }
            }
            entry.updated_at = timestamp;
            entity_count += 1;

            let exists = store.edges.iter().any(|e| {
                e.source == concept_name && e.target == topic_name
                && e.relation_type == RelationType::Mentions
            });
            if !exists {
                store.edges.push(GraphEdge {
                    source: concept_name.clone(),
                    target: topic_name.clone(),
                    relation_type: RelationType::Mentions,
                    weight: 0.7,
                    source_memory_id: memory_ids.first().cloned().unwrap_or_default(),
                    context: format!("概念 {} 关联主题 {}", concept_name, topic_name),
                    created_at: timestamp,
                });
                edge_count += 1;
            }
        }

        drop(store);
        if entity_count > 0 || edge_count > 0 {
            *self.dirty.write() = true;
        }

        (entity_count, edge_count)
    }

    /// 图谱遍历：从种子实体出发，按关系类型过滤，BFS 扩展
    ///
    /// 用内存 BFS 实现。
    ///
    /// - `seeds`：种子实体名列表
    /// - `relation_types`：过滤的关系类型（空 = 所有类型）
    /// - `max_depth`：最大遍历深度（1-3）
    /// - `limit`：最大返回节点数
    pub fn fanout(
        &self,
        seeds: &[&str],
        relation_types: &[RelationType],
        max_depth: usize,
        limit: usize,
    ) -> Vec<FanoutResult> {
        if seeds.is_empty() {
            return Vec::new();
        }

        let store = self.inner.read();
        let max_depth = max_depth.clamp(1, 3);
        let mut visited: HashMap<String, usize> = HashMap::new();
        let mut results: Vec<FanoutResult> = Vec::new();
        let mut queue: VecDeque<(String, usize, Vec<String>)> = VecDeque::new();

        // 初始化种子
        for seed in seeds {
            if store.entities.contains_key(*seed) {
                visited.insert(seed.to_string(), 0);
                queue.push_back((seed.to_string(), 0, vec![seed.to_string()]));
            }
        }

        // BFS
        while let Some((current, depth, path)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            if results.len() >= limit {
                break;
            }

            // 查找从 current 出发的所有边
            for edge in &store.edges {
                // 方向：source → target 或 target → source
                let (neighbor, _is_outgoing) = if edge.source == current {
                    (edge.target.clone(), true)
                } else if edge.target == current {
                    (edge.source.clone(), false)
                } else {
                    continue;
                };

                // 关系类型过滤
                if !relation_types.is_empty()
                    && !relation_types.contains(&edge.relation_type)
                {
                    continue;
                }

                // 环检测
                if visited.contains_key(&neighbor) {
                    continue;
                }

                visited.insert(neighbor.clone(), depth + 1);
                let mut new_path = path.clone();
                new_path.push(neighbor.clone());

                results.push(FanoutResult {
                    entity_name: neighbor.clone(),
                    hop: depth + 1,
                    path: new_path.clone(),
                    via_relation: edge.relation_type.clone(),
                    edge_weight: edge.weight,
                    source_memory_id: edge.source_memory_id.clone(),
                });

                queue.push_back((neighbor, depth + 1, new_path));
            }
        }

        // 按 hop 升序、weight 降序排序
        results.sort_by(|a, b| {
            a.hop.cmp(&b.hop)
                .then_with(|| b.edge_weight.partial_cmp(&a.edge_weight).unwrap_or(std::cmp::Ordering::Equal))
        });
        results.truncate(limit);

        results
    }

    /// 按名称前缀查找实体
    pub fn find_entities_by_prefix(&self, prefix: &str, limit: usize) -> Vec<GraphEntity> {
        let store = self.inner.read();
        store
            .entities
            .values()
            .filter(|e| e.name.contains(prefix))
            .take(limit)
            .cloned()
            .collect()
    }

    /// 按话题词检索概念实体关联的记忆 ID（① 图谱概念路的检索入口）。
    ///
    /// 遍历 `EntityType::Concept` 实体，名称与任一话题词匹配（双向子串）的实体，
    /// 返回其 `memory_ids`（概念关联的 evidence 记忆）。结果去重、按 limit 截断。
    /// 供检索管线把 query 话题映射到概念，再取回概念所支撑的记忆。
    pub fn find_concept_memories(&self, terms: &[&str], limit: usize) -> Vec<String> {
        let store = self.inner.read();
        let lower_terms: Vec<String> = terms
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if lower_terms.is_empty() {
            return Vec::new();
        }

        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (name, entity) in &store.entities {
            if entity.entity_type != EntityType::Concept || entity.memory_ids.is_empty() {
                continue;
            }
            let name_lower = name.to_lowercase();
            let matched = lower_terms
                .iter()
                .any(|t| name_lower.contains(t) || t.contains(&name_lower));
            if !matched {
                continue;
            }
            for mid in &entity.memory_ids {
                if seen.insert(mid.clone()) {
                    out.push(mid.clone());
                }
            }
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        out
    }

    /// 获取实体详情
    pub fn get_entity(&self, name: &str) -> Option<GraphEntity> {
        self.inner.read().entities.get(name).cloned()
    }

    /// 获取实体的所有边
    pub fn get_edges(&self, entity_name: &str) -> Vec<GraphEdge> {
        let store = self.inner.read();
        store
            .edges
            .iter()
            .filter(|e| e.source == entity_name || e.target == entity_name)
            .cloned()
            .collect()
    }

    /// 获取所有实体名
    pub fn list_entity_names(&self) -> Vec<String> {
        self.inner.read().entities.keys().cloned().collect()
    }

    /// 实体数量
    pub fn entity_count(&self) -> usize {
        self.inner.read().entities.len()
    }

    /// 边数量
    pub fn edge_count(&self) -> usize {
        self.inner.read().edges.len()
    }

    /// 持久化到磁盘：只 upsert 新增/变化的实体、边行，删除已移除的行（5 秒节流由调用方管理）
    pub fn save_to_disk(&self) -> Result<(), String> {
        let store = self.inner.read();
        let Some(db_mutex) = self.db.as_ref() else {
            *self.dirty.write() = false;
            return Ok(());
        };

        let mut node_rows: HashMap<String, String> = HashMap::with_capacity(store.entities.len());
        for (name, entity) in &store.entities {
            let json = serde_json::to_string(entity)
                .map_err(|e| format!("序列化图谱实体失败: {e}"))?;
            node_rows.insert(name.clone(), json);
        }
        let mut edge_rows: HashMap<String, String> = HashMap::with_capacity(store.edges.len());
        for edge in &store.edges {
            let json =
                serde_json::to_string(edge).map_err(|e| format!("序列化图谱边失败: {e}"))?;
            edge_rows.insert(edge_row_id(edge), json);
        }

        let mut db = db_mutex.lock();
        let mut node_upserts: Vec<(String, String)> = Vec::new();
        for (id, json) in &node_rows {
            if db.nodes.get(id).map(|existing| existing != json).unwrap_or(true) {
                node_upserts.push((id.clone(), json.clone()));
            }
        }
        let node_deletes: Vec<String> = db
            .nodes
            .keys()
            .filter(|id| !node_rows.contains_key(*id))
            .cloned()
            .collect();
        let mut edge_upserts: Vec<(String, String)> = Vec::new();
        for (id, json) in &edge_rows {
            if db.edges.get(id).map(|existing| existing != json).unwrap_or(true) {
                edge_upserts.push((id.clone(), json.clone()));
            }
        }
        let edge_deletes: Vec<String> = db
            .edges
            .keys()
            .filter(|id| !edge_rows.contains_key(*id))
            .cloned()
            .collect();

        db.apply(&node_upserts, &node_deletes, &edge_upserts, &edge_deletes)?;
        db.nodes = node_rows;
        db.edges = edge_rows;
        drop(db);
        *self.dirty.write() = false;
        Ok(())
    }

    /// 是否有未落盘的写入
    pub fn is_dirty(&self) -> bool {
        *self.dirty.read()
    }
}

/// 图谱遍历结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanoutResult {
    /// 实体名
    pub entity_name: String,
    /// 跳数（1 = 直接邻居）
    pub hop: usize,
    /// 路径（从种子到当前实体的实体名序列）
    pub path: Vec<String>,
    /// 经由的关系类型
    pub via_relation: RelationType,
    /// 边权重
    pub edge_weight: f64,
    /// 来源记忆 ID
    pub source_memory_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn make_test_graph() -> KnowledgeGraph {
        let path = temp_dir().join(format!("test_graph_{}.json", uuid::Uuid::new_v4()));
        KnowledgeGraph::new(path)
    }

    #[test]
    fn test_ingest_from_memory() {
        let graph = make_test_graph();
        let (entities, edges) = graph.ingest_from_memory(
            "mem_test1",
            "马化腾创建了腾讯，张三在腾讯工作",
            1000.0,
        );
        assert!(entities > 0);
        assert!(edges > 0);
    }

    #[test]
    fn test_fanout_finds_neighbors() {
        let graph = make_test_graph();
        // Traversal fixtures use explicit entities; tokenization is tested separately.
        graph.ingest_concepts("马化腾", &["腾讯".into()], &["mem1".into()], 1000.0);
        graph.ingest_concepts("张三", &["腾讯".into()], &["mem2".into()], 2000.0);
        // 从"腾讯"出发，应该能找到"马化腾"和"张三"
        let results = graph.fanout(&["腾讯"], &[], 2, 10);
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.entity_name.contains("马化腾")));
        assert!(results.iter().any(|r| r.entity_name.contains("张三")));
    }

    #[test]
    fn test_fanout_relation_filter() {
        let graph = make_test_graph();
        graph.ingest_from_memory(
            "mem_test1",
            "马化腾创建了腾讯，张三在腾讯工作",
            1000.0,
        );

        // 只查 works_at 关系
        let results = graph.fanout(&["腾讯"], &[RelationType::WorksAt], 2, 10);
        assert!(results.iter().all(|r| r.via_relation == RelationType::WorksAt));
    }

    #[test]
    fn test_fanout_max_depth() {
        let graph = make_test_graph();
        graph.ingest_concepts("A", &["B".into()], &["mem1".into()], 1000.0);
        graph.ingest_concepts("B", &["C".into()], &["mem2".into()], 2000.0);

        // depth=1：只找直接邻居
        let results = graph.fanout(&["A"], &[], 1, 10);
        assert!(results.iter().all(|r| r.hop == 1));

        // depth=2：可以找到二跳邻居
        let results = graph.fanout(&["A"], &[], 2, 10);
        assert!(results.iter().any(|r| r.hop == 2));
    }

    #[test]
    fn test_fanout_empty_seeds() {
        let graph = make_test_graph();
        graph.ingest_from_memory("mem1", "A创建了B", 1000.0);
        let results = graph.fanout(&[], &[], 3, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fanout_nonexistent_seed() {
        let graph = make_test_graph();
        graph.ingest_from_memory("mem1", "A创建了B", 1000.0);
        let results = graph.fanout(&["不存在的实体"], &[], 3, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_entity() {
        let graph = make_test_graph();
        graph.ingest_from_memory("mem1", "马化腾创建了腾讯", 1000.0);
        
        // jieba 可能不把"马化腾"标为 nr，但至少函数不 panic
        let _ = graph.get_entity("马化腾");
        let _ = graph.get_entity("不存在");
    }

    /// 清理图谱测试产生的库文件（含 WAL/SHM 旁文件）。
    fn cleanup_graph_files(path: &std::path::Path) {
        let db = path.with_extension("db");
        for p in [
            db.clone(),
            db.with_extension("db-wal"),
            db.with_extension("db-shm"),
        ] {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_save_and_load() {
        let path = temp_dir().join(format!("test_graph_save_{}.json", uuid::Uuid::new_v4()));
        {
            let graph = KnowledgeGraph::new(path.clone());
            graph.ingest_from_memory("mem1", "马化腾创建了腾讯", 1000.0);
            graph.save_to_disk().unwrap();
        }
        // 重新加载
        let graph2 = KnowledgeGraph::new(path.clone());
        assert!(graph2.entity_count() > 0 || graph2.edge_count() > 0);

        // 清理
        cleanup_graph_files(&path);
    }

    #[test]
    fn test_sqlite_rows_survive_reopen() {
        let path = temp_dir().join(format!("test_graph_reopen_{}.json", uuid::Uuid::new_v4()));
        let entities;
        let edges;
        let names;
        {
            let graph = KnowledgeGraph::new(path.clone());
            graph.ingest_from_memory("mem1", "马化腾创建了腾讯", 1000.0);
            graph.ingest_concepts(
                "agent_autonomy",
                &["proactive".to_string()],
                &["insight1".to_string()],
                1000.0,
            );
            graph.save_to_disk().unwrap();
            entities = graph.entity_count();
            edges = graph.edge_count();
            let mut n = graph.list_entity_names();
            n.sort();
            names = n;
        }
        let reopened = KnowledgeGraph::new(path.clone());
        assert_eq!(reopened.entity_count(), entities);
        assert_eq!(reopened.edge_count(), edges);
        let mut reopened_names = reopened.list_entity_names();
        reopened_names.sort();
        assert_eq!(reopened_names, names);
        cleanup_graph_files(&path);
    }

    #[test]
    fn test_single_mutation_does_not_rewrite_unaffected_rows() {
        let path = temp_dir().join(format!("test_graph_diff_{}.json", uuid::Uuid::new_v4()));
        let graph = KnowledgeGraph::new(path.clone());
        graph.ingest_concepts(
            "agent_autonomy",
            &["proactive".to_string(), "inner_monologue".to_string()],
            &["m1".to_string()],
            1000.0,
        );
        graph.save_to_disk().unwrap();
        let after_first = graph.db.as_ref().unwrap().lock().conn.total_changes();
        // 内容未变化：第二次保存不应产生任何写入
        graph.save_to_disk().unwrap();
        let after_second = graph.db.as_ref().unwrap().lock().conn.total_changes();
        assert_eq!(after_first, after_second, "未变化的实体/边行不应被重写");
        // 新增一个概念：2 个新实体 + 1 条新边 = 仅 3 行被写入
        graph.ingest_concepts("memory_system", &["recall".to_string()], &["m2".to_string()], 2000.0);
        graph.save_to_disk().unwrap();
        let after_add = graph.db.as_ref().unwrap().lock().conn.total_changes();
        assert_eq!(after_add - after_second, 3, "单次新增只应写入相关行");
        cleanup_graph_files(&path);
    }

    #[test]
    fn test_legacy_json_imported_and_archived() {
        let path = temp_dir().join(format!("test_graph_legacy_{}.json", uuid::Uuid::new_v4()));
        let mut data = GraphStoreData::new();
        data.entities.insert(
            "腾讯".to_string(),
            GraphEntity {
                name: "腾讯".to_string(),
                entity_type: EntityType::Concept,
                salience: 0.8,
                memory_ids: vec!["m1".to_string()],
                created_at: 1.0,
                updated_at: 1.0,
            },
        );
        data.edges.push(GraphEdge {
            source: "腾讯".to_string(),
            target: "马化腾".to_string(),
            relation_type: RelationType::Mentions,
            weight: 0.7,
            source_memory_id: "m1".to_string(),
            context: String::new(),
            created_at: 1.0,
        });
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

        let graph = KnowledgeGraph::new(path.clone());
        assert_eq!(graph.entity_count(), 1);
        assert_eq!(graph.edge_count(), 1);
        // 旧文件已归档为 <name>.migrated，不删除用户数据
        assert!(!path.exists());
        let migrated = path.with_file_name(format!(
            "{}.migrated",
            path.file_name().unwrap().to_string_lossy()
        ));
        assert!(migrated.exists());
        let _ = std::fs::remove_file(&migrated);
        cleanup_graph_files(&path);
    }

    #[test]
    fn test_ingest_concepts_creates_entities_and_edges() {
        let graph = make_test_graph();
        let (entities, edges) = graph.ingest_concepts(
            "agent_autonomy",
            &["proactive".to_string(), "inner_monologue".to_string()],
            &["insight1".to_string()],
            1000.0,
        );
        // 概念 + 2 主题 = 3 实体，2 边
        assert_eq!(entities, 3);
        assert_eq!(edges, 2);
        // 幂等：重复写入不新增边
        let (e2, edge2) = graph.ingest_concepts(
            "agent_autonomy",
            &["proactive".to_string()],
            &["insight2".to_string()],
            2000.0,
        );
        assert_eq!(edge2, 0);
        assert!(e2 > 0);
        // 概念实体类型
        let entity = graph.get_entity("agent_autonomy").expect("概念实体应存在");
        assert_eq!(entity.entity_type, EntityType::Concept);
        // 幂等后 memory_ids 累积
        assert!(entity.memory_ids.contains(&"insight1".to_string()));
        assert!(entity.memory_ids.contains(&"insight2".to_string()));
    }

    #[test]
    fn test_find_concept_memories_by_topic() {
        let graph = make_test_graph();
        graph.ingest_concepts(
            "agent_autonomy",
            &["proactive".to_string(), "inner_monologue".to_string()],
            &["insight1".to_string(), "insight2".to_string()],
            1000.0,
        );
        // 话题词命中概念名 → 返回其 evidence 记忆
        let ids = graph.find_concept_memories(&["autonomy"], 10);
        assert!(ids.contains(&"insight1".to_string()));
        assert!(ids.contains(&"insight2".to_string()));
        // 无关话题 → 空
        assert!(graph.find_concept_memories(&["cooking"], 10).is_empty());
    }
}
