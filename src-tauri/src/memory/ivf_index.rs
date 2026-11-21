//! IVF (Inverted File) 倒排索引 — 向量搜索性能加速
//!
//! 对大量向量做精确 KNN 需要遍历所有向量（暴力扫描），
//! IVF 通过 k-means 聚类将向量空间划分为 nlist 个聚类，
//! 查询时只扫描查询向量最近的 nprobe 个聚类，将复杂度从 O(N) 降到 O(N/nlist * nprobe)。
//!
//! 算法：
//! 1. 构建：对全部向量做 k-means++ 聚类，得到 nlist 个质心
//! 2. 分配：每个向量分配到最近的质心，形成倒排列表
//! 3. 查询：计算查询向量到所有质心的距离，取最近 nprobe 个质心
//! 4. 扫描：只在这 nprobe 个质心的倒排列表中做精确 KNN
//!
//! 适用场景：
//! - 向量数量 > 1000 时收益明显
//! - 召回率可接受轻微下降（nprobe 越大召回越高，性能收益越小）
//!
//! 注意：这是 sqlite-vec KNN 之上的应用层加速。
//! sqlite-vec 本身已优化，但当向量数量很大时（>10K），IVF 仍能提供 5-10x 加速。

use std::collections::HashMap;

/// IVF 索引配置
#[derive(Debug, Clone)]
pub struct IvfConfig {
    /// 聚类数量（nlist）
    /// 经验值：sqrt(N) / 2，上限 512
    pub nlist: usize,
    /// 查询时探测的聚类数量（nprobe）
    /// nprobe / nlist 越大，召回率越高，但性能收益越小
    pub nprobe: usize,
    /// 构建索引的最小向量数量（低于此值不构建索引，直接暴力扫描）
    pub min_vectors_to_build: usize,
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            nlist: 64,
            nprobe: 8,
            min_vectors_to_build: 500,
        }
    }
}

/// IVF 索引
///
/// 持有质心和倒排列表（doc_id → cluster_id 的映射）。
/// 线程安全：内部状态通过 Mutex 保护。
pub struct IvfIndex {
    config: IvfConfig,
    /// 质心向量（nlist 个）
    centroids: parking_lot::Mutex<Vec<Vec<f32>>>,
    /// 倒排列表：cluster_id → [doc_id]
    inverted_lists: parking_lot::Mutex<HashMap<usize, Vec<String>>>,
    /// doc_id → cluster_id（用于删除时定位）
    doc_cluster: parking_lot::Mutex<HashMap<String, usize>>,
    /// 是否已构建
    built: std::sync::atomic::AtomicBool,
}

impl IvfIndex {
    pub fn new(config: IvfConfig) -> Self {
        Self {
            config,
            centroids: parking_lot::Mutex::new(Vec::new()),
            inverted_lists: parking_lot::Mutex::new(HashMap::new()),
            doc_cluster: parking_lot::Mutex::new(HashMap::new()),
            built: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 是否已构建索引
    pub fn is_built(&self) -> bool {
        self.built.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 构建索引
    ///
    /// `vectors` 是 (doc_id, embedding) 对的列表。
    /// 如果向量数量 < min_vectors_to_build，则不构建（返回 false）。
    pub fn build(&self, vectors: &[(String, Vec<f32>)]) -> bool {
        if vectors.len() < self.config.min_vectors_to_build {
            tracing::debug!(
                "[IVF] 向量数量 {} < {}，跳过索引构建",
                vectors.len(),
                self.config.min_vectors_to_build
            );
            return false;
        }

        // 动态调整 nlist：min(nlist, sqrt(N)/2)
        let dynamic_nlist = (vectors.len() as f64).sqrt() as usize / 2;
        let nlist = self.config.nlist.min(dynamic_nlist.max(1));

        if vectors.is_empty() || nlist == 0 {
            return false;
        }

        // k-means++ 初始化 + Lloyd 迭代
        let centroids = kmeans_plus_plus(vectors, nlist);
        let assignments = assign_to_centroids(vectors, &centroids);

        // 构建倒排列表
        let mut inverted_lists: HashMap<usize, Vec<String>> = HashMap::new();
        let mut doc_cluster: HashMap<String, usize> = HashMap::new();
        for (doc_id, cluster_id) in assignments {
            inverted_lists
                .entry(cluster_id)
                .or_default()
                .push(doc_id.0.clone());
            doc_cluster.insert(doc_id.0.clone(), cluster_id);
        }

        *self.centroids.lock() = centroids;
        *self.inverted_lists.lock() = inverted_lists;
        *self.doc_cluster.lock() = doc_cluster;
        self.built.store(true, std::sync::atomic::Ordering::Relaxed);

        tracing::info!(
            "[IVF] 索引构建完成: {} 个向量, {} 个聚类, 平均每聚类 {:.1} 个向量",
            vectors.len(),
            nlist,
            vectors.len() as f64 / nlist as f64
        );
        true
    }

    /// 查询：返回应扫描的 doc_id 候选集
    ///
    /// 如果索引未构建，返回 None（调用方应做全量扫描）。
    pub fn probe(&self, query: &[f32]) -> Option<Vec<String>> {
        if !self.is_built() {
            return None;
        }

        let centroids = self.centroids.lock();
        if centroids.is_empty() {
            return None;
        }

        // 计算查询到所有质心的距离，取最近的 nprobe 个
        let mut dists: Vec<(usize, f64)> = centroids
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cosine_distance(query, c)))
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let probe_clusters: Vec<usize> = dists
            .iter()
            .take(self.config.nprobe)
            .map(|(i, _)| *i)
            .collect();

        // 收集这些聚类中的所有 doc_id
        let inverted = self.inverted_lists.lock();
        let mut candidates = Vec::new();
        for cluster_id in probe_clusters {
            if let Some(list) = inverted.get(&cluster_id) {
                candidates.extend_from_slice(list);
            }
        }

        Some(candidates)
    }

    /// 添加单个向量到索引（增量更新）
    ///
    /// 如果索引未构建，此操作无意义（返回）。
    pub fn add(&self, doc_id: &str, embedding: &[f32]) {
        if !self.is_built() {
            return;
        }
        let centroids = self.centroids.lock();
        if centroids.is_empty() {
            return;
        }
        // 找到最近的质心
        let mut best_cluster = 0;
        let mut best_dist = f64::MAX;
        for (i, c) in centroids.iter().enumerate() {
            let d = cosine_distance(embedding, c);
            if d < best_dist {
                best_dist = d;
                best_cluster = i;
            }
        }
        drop(centroids);

        self.inverted_lists
            .lock()
            .entry(best_cluster)
            .or_default()
            .push(doc_id.to_string());
        self.doc_cluster
            .lock()
            .insert(doc_id.to_string(), best_cluster);
    }

    /// 删除单个向量
    pub fn remove(&self, doc_id: &str) {
        if !self.is_built() {
            return;
        }
        let cluster_id = self
            .doc_cluster
            .lock()
            .remove(doc_id);
        if let Some(cid) = cluster_id {
            if let Some(list) = self.inverted_lists.lock().get_mut(&cid) {
                list.retain(|id| id != doc_id);
            }
        }
    }

    /// 清空索引
    pub fn clear(&self) {
        self.centroids.lock().clear();
        self.inverted_lists.lock().clear();
        self.doc_cluster.lock().clear();
        self.built.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// 获取统计信息
    pub fn stats(&self) -> IvfStats {
        let centroids = self.centroids.lock();
        let inverted = self.inverted_lists.lock();
        let doc_cluster = self.doc_cluster.lock();
        IvfStats {
            nlist: centroids.len(),
            nprobe: self.config.nprobe,
            total_vectors: doc_cluster.len(),
            built: self.is_built(),
            avg_cluster_size: if centroids.is_empty() {
                0.0
            } else {
                doc_cluster.len() as f64 / centroids.len() as f64
            },
            empty_clusters: inverted.values().filter(|l| l.is_empty()).count(),
        }
    }
}

/// IVF 索引统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct IvfStats {
    pub nlist: usize,
    pub nprobe: usize,
    pub total_vectors: usize,
    pub built: bool,
    pub avg_cluster_size: f64,
    pub empty_clusters: usize,
}

/// k-means++ 初始化 + Lloyd 迭代
fn kmeans_plus_plus(vectors: &[(String, Vec<f32>)], k: usize) -> Vec<Vec<f32>> {
    if vectors.is_empty() || k == 0 {
        return Vec::new();
    }

    let dim = vectors[0].1.len();
    let n = vectors.len();
    let k = k.min(n);

    // 1. k-means++ 初始化
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(k);
    // 随机选第一个质心（使用伪随机）
    let first_idx = pseudo_random_index(n, 0);
    centroids.push(vectors[first_idx].1.clone());

    for _ in 1..k {
        // 计算每个点到最近质心的距离平方
        let dists: Vec<f64> = vectors
            .iter()
            .map(|(_, v)| {
                centroids
                    .iter()
                    .map(|c| cosine_distance(v, c).powi(2))
                    .fold(f64::MAX, f64::min)
            })
            .collect();
        let total: f64 = dists.iter().sum();
        if total <= 0.0 {
            // 所有点相同，随机选一个
            let idx = pseudo_random_index(n, centroids.len());
            centroids.push(vectors[idx].1.clone());
            continue;
        }
        // 轮盘赌选择
        let r = pseudo_random_f64(centroids.len());
        let threshold = r * total;
        let mut cumsum = 0.0;
        let mut selected = n - 1;
        for (i, d) in dists.iter().enumerate() {
            cumsum += d;
            if cumsum >= threshold {
                selected = i;
                break;
            }
        }
        centroids.push(vectors[selected].1.clone());
    }

    // 2. Lloyd 迭代（最多 10 轮）
    for _ in 0..10 {
        let assignments = assign_to_centroids(vectors, &centroids);
        let mut new_centroids: Vec<Vec<f32>> = vec![vec![0.0; dim]; k];
        let mut counts: Vec<usize> = vec![0; k];

        for ((_, v), cluster_id) in vectors.iter().zip(assignments.iter()) {
            counts[cluster_id.1] += 1;
            for (j, val) in v.iter().enumerate() {
                new_centroids[cluster_id.1][j] += val;
            }
        }

        let mut changed = false;
        for (i, c) in new_centroids.iter_mut().enumerate() {
            if counts[i] > 0 {
                for val in c.iter_mut() {
                    *val /= counts[i] as f32;
                }
                // 检查是否变化
                if i < centroids.len() {
                    let diff: f32 = c
                        .iter()
                        .zip(centroids[i].iter())
                        .map(|(a, b)| (a - b).abs())
                        .sum();
                    if diff > 1e-4 {
                        changed = true;
                    }
                }
            } else {
                // 空聚类保留旧质心
                if i < centroids.len() {
                    *c = centroids[i].clone();
                }
            }
        }

        centroids = new_centroids;
        if !changed {
            break;
        }
    }

    centroids
}

/// 将向量分配到最近的质心
fn assign_to_centroids<'a>(
    vectors: &'a [(String, Vec<f32>)],
    centroids: &[Vec<f32>],
) -> Vec<(&'a (String, Vec<f32>), usize)> {
    vectors
        .iter()
        .map(|v| {
            let mut best = 0;
            let mut best_dist = f64::MAX;
            for (i, c) in centroids.iter().enumerate() {
                let d = cosine_distance(&v.1, c);
                if d < best_dist {
                    best_dist = d;
                    best = i;
                }
            }
            (v, best)
        })
        .collect()
}

/// 余弦距离（1 - 余弦相似度）
fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm_a < 1e-9 || norm_b < 1e-9 {
        return 1.0;
    }
    1.0 - (dot / (norm_a * norm_b)) as f64
}

/// 伪随机索引 [0, n)
fn pseudo_random_index(n: usize, seed: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as usize)
        .unwrap_or(0);
    let mut x = nanos.wrapping_add(seed.wrapping_mul(0x9E3779B9));
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % n
}

/// 伪随机浮点数 [0, 1)
fn pseudo_random_f64(seed: usize) -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos.wrapping_add(seed as u64);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as f64) / (u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skip_small_dataset() {
        let index = IvfIndex::new(IvfConfig {
            min_vectors_to_build: 100,
            ..Default::default()
        });
        let vectors: Vec<(String, Vec<f32>)> = (0..10)
            .map(|i| (format!("doc_{}", i), vec![i as f32, (i * 2) as f32]))
            .collect();
        assert!(!index.build(&vectors));
        assert!(!index.is_built());
    }

    #[test]
    fn test_build_and_probe() {
        let index = IvfIndex::new(IvfConfig {
            nlist: 4,
            nprobe: 2,
            min_vectors_to_build: 10,
        });
        // 生成 50 个 2D 向量，分布在 4 个区域
        let vectors: Vec<(String, Vec<f32>)> = (0..50)
            .map(|i| {
                let cluster = i % 4;
                let base = match cluster {
                    0 => vec![0.0, 0.0],
                    1 => vec![10.0, 0.0],
                    2 => vec![0.0, 10.0],
                    _ => vec![10.0, 10.0],
                };
                (format!("doc_{}", i), vec![base[0] + i as f32 * 0.01, base[1] + i as f32 * 0.01])
            })
            .collect();

        assert!(index.build(&vectors));
        assert!(index.is_built());

        let candidates = index.probe(&[0.0, 0.0]);
        assert!(candidates.is_some());
        assert!(!candidates.unwrap().is_empty());
    }

    #[test]
    fn test_add_remove() {
        let index = IvfIndex::new(IvfConfig {
            nlist: 2,
            nprobe: 1,
            min_vectors_to_build: 10,
        });
        let vectors: Vec<(String, Vec<f32>)> = (0..20)
            .map(|i| (format!("doc_{}", i), vec![i as f32, (i * 2) as f32]))
            .collect();
        index.build(&vectors);

        // 添加新向量
        index.add("new_doc", &[5.0, 10.0]);
        let stats = index.stats();
        assert_eq!(stats.total_vectors, 21);

        // 删除
        index.remove("new_doc");
        let stats = index.stats();
        assert_eq!(stats.total_vectors, 20);
    }
}
