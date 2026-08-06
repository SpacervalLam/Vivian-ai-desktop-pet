use std::sync::Arc;

use serde_json::json;

use super::manager::MemoryManager;
use super::types::{current_timestamp, MemoryItem, MemoryType};
use super::vector_search::cosine_similarity;
use crate::error::VivianResult;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

const SEMANTIC_MERGE_THRESHOLD: f64 = 0.85;
const MAX_CLUSTER_SIZE: usize = 5;
const MERGE_IMPORTANCE_BONUS: f64 = 0.10;
const IMPORTANCE_CEILING: f64 = 0.95;
const MIN_IMPORTANCE_FOR_MERGE: f64 = 0.40;
const MAX_PAIRS_PER_RUN: usize = 8;

pub struct TopicMerger {
    router: Arc<ModelRouter>,
}

#[derive(Debug, Clone)]
pub struct MergeReport {
    pub clusters_merged: usize,
    pub memories_removed: usize,
    pub memories_created: usize,
}

impl Default for MergeReport {
    fn default() -> Self {
        Self {
            clusters_merged: 0,
            memories_removed: 0,
            memories_created: 0,
        }
    }
}

impl TopicMerger {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self { router }
    }

    pub async fn run(&self, manager: &MemoryManager) -> VivianResult<MergeReport> {
        let candidates = self.collect_candidates(manager).await?;
        if candidates.len() < 2 {
            return Ok(MergeReport::default());
        }
        let clusters = self.cluster_by_similarity(candidates);
        if clusters.is_empty() {
            return Ok(MergeReport::default());
        }

        let mut report = MergeReport::default();
        let mut pairs_processed = 0usize;
        for cluster in clusters {
            if pairs_processed >= MAX_PAIRS_PER_RUN {
                break;
            }
            if cluster.len() < 2 {
                continue;
            }
            match self.merge_cluster(manager, &cluster).await {
                Ok(()) => {
                    report.clusters_merged += 1;
                    report.memories_removed += cluster.len();
                    report.memories_created += 1;
                    pairs_processed += 1;
                }
                Err(e) => {
                    tracing::warn!("[TopicMerger] 合并失败: {e}");
                }
            }
        }
        Ok(report)
    }

    async fn collect_candidates(&self, manager: &MemoryManager) -> VivianResult<Vec<MemoryItem>> {
        let all = manager.get_all_memories().await?;
        Ok(all
            .into_iter()
            .filter(|m| {
                m.importance >= MIN_IMPORTANCE_FOR_MERGE
                    && !m.protected
                    && m.embedding.is_some()
                    && !m.content.trim().is_empty()
                    && matches!(
                        MemoryType::from_str(&m.memory_type),
                        Some(
                            MemoryType::LongTerm
                                | MemoryType::SessionSummary
                                | MemoryType::Insight
                                | MemoryType::User
                                | MemoryType::Preference
                                | MemoryType::Knowledge
                                | MemoryType::ImportantEvent
                        )
                    )
            })
            .collect())
    }

    fn cluster_by_similarity(&self, items: Vec<MemoryItem>) -> Vec<Vec<MemoryItem>> {
        let mut clusters: Vec<Vec<MemoryItem>> = Vec::new();
        let mut assigned: Vec<bool> = vec![false; items.len()];

        let emb_f32: Vec<Vec<f32>> = items
            .iter()
            .map(|m| m.embedding.as_ref().map(|e| e.iter().map(|v| *v as f32).collect()).unwrap_or_default())
            .collect();

        for (i, item) in items.iter().enumerate() {
            if assigned[i] {
                continue;
            }
            let emb_i = match &item.embedding {
                Some(_) => &emb_f32[i],
                None => continue,
            };
            if emb_i.is_empty() {
                continue;
            }
            let mut cluster: Vec<MemoryItem> = vec![item.clone()];
            assigned[i] = true;
            for (j, other) in items.iter().enumerate() {
                if i == j || assigned[j] {
                    continue;
                }
                if cluster.len() >= MAX_CLUSTER_SIZE {
                    break;
                }
                if other.memory_type != item.memory_type {
                    continue;
                }
                let emb_j = &emb_f32[j];
                if emb_j.is_empty() {
                    continue;
                }
                let sim = cosine_similarity(emb_i, emb_j);
                if sim >= SEMANTIC_MERGE_THRESHOLD {
                    cluster.push(other.clone());
                    assigned[j] = true;
                }
            }
            if cluster.len() >= 2 {
                clusters.push(cluster);
            }
        }
        clusters
    }

    async fn merge_cluster(&self, manager: &MemoryManager, cluster: &[MemoryItem]) -> VivianResult<()> {
        if cluster.len() < 2 {
            return Ok(());
        }
        let merged_content = self.llm_merge_contents(cluster).await?;
        if merged_content.trim().is_empty() {
            return Ok(());
        }
        let primary = cluster
            .iter()
            .max_by(|a, b| {
                a.importance
                    .partial_cmp(&b.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
            .unwrap_or_else(|| cluster[0].clone());

        let merged_importance =
            (primary.importance + MERGE_IMPORTANCE_BONUS).min(IMPORTANCE_CEILING);
        let original_ids: Vec<String> = cluster.iter().map(|m| m.id.clone()).collect();
        let original_timestamp = cluster
            .iter()
            .map(|m| m.timestamp)
            .fold(f64::INFINITY, f64::min);

        let mut metadata = primary.metadata.clone();
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("merged_from".to_string(), json!(original_ids));
            obj.insert("merged_at".to_string(), json!(current_timestamp()));
            obj.insert("cluster_size".to_string(), json!(cluster.len()));
        } else {
            metadata = json!({
                "merged_from": original_ids,
                "merged_at": current_timestamp(),
                "cluster_size": cluster.len(),
            });
        }

        let mem_type = MemoryType::from_str(&primary.memory_type)
            .unwrap_or(MemoryType::LongTerm);

        for original in cluster {
            manager.hard_delete_memory(&original.id).await?;
        }
        manager
            .add_merged_memory(
                &merged_content,
                mem_type,
                merged_importance,
                primary.tags.clone(),
                metadata,
                original_ids,
                Some(original_timestamp),
            )
            .await?;
        Ok(())
    }

    async fn llm_merge_contents(&self, cluster: &[MemoryItem]) -> VivianResult<String> {
        let items_text = cluster
            .iter()
            .enumerate()
            .map(|(i, m)| format!("[{}] {}", i + 1, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let sys = "你负责把多条同主题记忆合并为一条信息密度最高的版本。\
合并时保留所有事实、人名、时间、数字与因果链，剔除重复表述，\
用最简洁的自然语言输出。直接输出合并后的内容，不要加解释、不要加编号。";
        let user = format!("待合并记忆：\n{}", items_text);

        let req = LLMRequest::new(
            "consolidation",
            vec![ChatMessage::system(sys), ChatMessage::user(&user)],
        );
        let resp = self.router.generate(req).await?;
        Ok(resp.trim().to_string())
    }
}
