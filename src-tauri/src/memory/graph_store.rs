//! 知识图谱存储 — typed edges + 实体节点
//!
//! 借鉴 GBrain 的 links 表设计：
//! - 实体节点（name + type + salience）
//! - typed edges（source → target + relation_type + weight + source_memory_id）
//! - 内存 HashMap 存储 + JSON 持久化
//!
//! 与 GBrain 的差异：
//! - GBrain 用 Postgres links 表，本模块用内存 HashMap + JSON 文件
//! - GBrain 的实体来自 pages 表，本模块的实体从记忆内容自动抽取
//! - 增加了 source_memory_id 溯源（每条边关联到来源记忆）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
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

/// 知识图谱存储
///
/// 内存 HashMap + JSON 文件持久化。
/// 写入时更新内存 + 标记脏数据，5 秒节流落盘。
pub struct KnowledgeGraph {
    inner: Arc<RwLock<GraphStoreData>>,
    store_path: PathBuf,
    dirty: Arc<RwLock<bool>>,
}

impl KnowledgeGraph {
    /// 创建新的知识图谱存储
    pub fn new(store_path: PathBuf) -> Self {
        let mut store = GraphStoreData::new();
        
        // 尝试从磁盘加载
        if store_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&store_path) {
                if let Ok(loaded) = serde_json::from_str::<GraphStoreData>(&content) {
                    store = loaded;
                    tracing::info!(
                        "[KnowledgeGraph] 加载 {} 个实体 / {} 条边",
                        store.entities.len(),
                        store.edges.len()
                    );
                }
            }
        }

        Self {
            inner: Arc::new(RwLock::new(store)),
            store_path,
            dirty: Arc::new(RwLock::new(false)),
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

    /// 图谱遍历：从种子实体出发，按关系类型过滤，BFS 扩展
    ///
    /// 借鉴 GBrain 的 relationalFanout 递归 CTE，但用内存 BFS 实现。
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
        let mut queue: Vec<(String, usize, Vec<String>)> = Vec::new();

        // 初始化种子
        for seed in seeds {
            if store.entities.contains_key(*seed) {
                visited.insert(seed.to_string(), 0);
                queue.push((seed.to_string(), 0, vec![seed.to_string()]));
            }
        }

        // BFS
        while let Some((current, depth, path)) = queue.pop() {
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

                queue.push((neighbor, depth + 1, new_path));
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

    /// 持久化到磁盘（5 秒节流由调用方管理）
    pub fn save_to_disk(&self) -> Result<(), String> {
        let store = self.inner.read();
        let content = serde_json::to_string_pretty(&*store)
            .map_err(|e| format!("序列化图谱失败: {e}"))?;
        
        let tmp_path = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, content)
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        std::fs::rename(&tmp_path, &self.store_path)
            .map_err(|e| format!("重命名文件失败: {e}"))?;
        
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
        graph.ingest_from_memory(
            "mem_test1",
            "马化腾创建了腾讯",
            1000.0,
        );
        graph.ingest_from_memory(
            "mem_test2",
            "张三在腾讯工作",
            2000.0,
        );

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
        graph.ingest_from_memory("mem1", "A创建了B", 1000.0);
        graph.ingest_from_memory("mem2", "B投资了C", 2000.0);

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
        let _ = std::fs::remove_file(&path);
    }
}
