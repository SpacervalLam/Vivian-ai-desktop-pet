//! 工具语义筛选器
//!
//! 基于 FastSemanticAnalyzer 的预嵌入能力，对工具描述做语义匹配，
//! 从全量工具中筛选出与用户输入最相关的 Top-N 工具。
//!
//! 设计要点：
//! - 工具描述嵌入懒加载并缓存（key 为工具名，描述变更时重新嵌入）
//! - 复用 FastPerceptionResult.query_embedding，避免重复嵌入查询文本
//! - 仅在 intent=tool_request/request 时由 PromptBuildingStep 调用
//! - 筛选结果作为"推荐工具"注入 prompt，不改变现有 visibility 分流逻辑

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::ToolSystem;
use crate::memory::embedding::MemoryEmbeddingProvider;

/// 推荐工具的最小相似度阈值（低于此值不推荐）
const MIN_SIM_THRESHOLD: f32 = 0.30;
/// 默认推荐的工具数量上限
const DEFAULT_TOP_N: usize = 5;

/// 单个推荐工具的元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecommendation {
    /// 工具名
    pub name: String,
    /// 工具描述（用于 prompt 展示）
    pub description: String,
    /// 与用户输入的语义相似度 [0, 1]
    pub similarity: f64,
}

/// 工具语义筛选器
///
/// 持有 embedding provider 引用和工具描述嵌入缓存。
/// 缓存 key 为 (工具名, 语言, 描述内容) 的组合，描述变化或语言切换时自动重新嵌入。
pub struct ToolSemanticFilter {
    provider: Arc<dyn MemoryEmbeddingProvider>,
    /// 界面语言（"zh"/"en"/"ja"），决定调用 `Tool::description_in(lang)` 取哪一版描述
    language: String,
    /// (工具名, 语言) -> (描述文本, 嵌入向量)
    ///
    /// 嵌入向量在首次查询时懒加载。当工具描述变化或语言切换时，
    /// 下次查询检测到描述不匹配会自动重新嵌入。
    embeddings: Mutex<HashMap<(String, String), (String, Vec<f32>)>>,
}

impl ToolSemanticFilter {
    pub fn new(provider: Arc<dyn MemoryEmbeddingProvider>, language: String) -> Self {
        Self {
            provider,
            language,
            embeddings: Mutex::new(HashMap::new()),
        }
    }

    /// 启动预加载：立即嵌入所有工具描述（阻塞）。
    ///
    /// 供启动流程在开放 API 前调用，避免首个工具相关请求触发懒嵌入。
    /// 失败的工具会跳过，不阻塞启动。
    pub fn preload(&self, tool_system: &ToolSystem) {
        let tools = tool_system.list_tools();
        let lang = self.language.as_str();
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        let to_embed = {
            let mut cache = self.embeddings.lock();
            cache.retain(|(name, _), _| names.contains(name));

            let mut pending: Vec<(String, String)> = Vec::new();
            for tool in &tools {
                let name = tool.name().to_string();
                let desc = tool.description_in(lang).to_string();
                let cache_key = (name.clone(), lang.to_string());
                let need_reembed = match cache.get(&cache_key) {
                    Some((cached_desc, _)) => *cached_desc != desc,
                    None => true,
                };
                if need_reembed {
                    pending.push((name, desc));
                }
            }
            pending
        };

        if to_embed.is_empty() {
            return;
        }

        let texts: Vec<String> = to_embed.iter().map(|(_, d)| d.clone()).collect();
        match self.provider.embed_batch(&texts) {
            Ok(embs) => {
                let mut cache = self.embeddings.lock();
                for ((name, desc), emb) in to_embed.into_iter().zip(embs.into_iter()) {
                    cache.insert((name, lang.to_string()), (desc, emb));
                }
                tracing::info!(
                    "[ToolSemanticFilter] 工具描述嵌入预加载完成: {} 个工具",
                    texts.len()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[ToolSemanticFilter] 启动预加载工具描述嵌入失败，跳过 {} 个新工具: {}",
                    texts.len(),
                    e
                );
            }
        }
    }

    /// 根据查询嵌入向量筛选最相关的工具
    ///
    /// 参数：
    /// - `tool_system`：工具系统引用
    /// - `query_emb`：用户输入的嵌入向量（来自 FastPerceptionResult.query_embedding）
    /// - `top_n`：返回的最大工具数量
    /// - `min_sim`：最小相似度阈值
    ///
    /// 返回按相似度降序排列的工具推荐列表。
    /// 嵌入失败的工具跳过（不阻塞筛选）。
    pub fn filter(
        &self,
        tool_system: &ToolSystem,
        query_emb: &[f32],
        top_n: usize,
        min_sim: f32,
    ) -> Vec<ToolRecommendation> {
        if query_emb.is_empty() {
            return Vec::new();
        }

        let tools = tool_system.list_tools();
        let lang = self.language.as_str();
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        // 同步缓存：移除已不存在的工具，识别需要（重新）嵌入的工具
        let to_embed = {
            let mut cache = self.embeddings.lock();
            // 移除已不存在的工具（任意语言下）
            cache.retain(|(name, _), _| names.contains(name));

            // 收集需要嵌入的工具：未缓存 或 描述已变更
            let mut pending: Vec<(String, String)> = Vec::new();
            for tool in &tools {
                let name = tool.name().to_string();
                let desc = tool.description_in(lang).to_string();
                let cache_key = (name.clone(), lang.to_string());
                let need_reembed = match cache.get(&cache_key) {
                    Some((cached_desc, _)) => *cached_desc != desc,
                    None => true,
                };
                if need_reembed {
                    pending.push((name, desc));
                }
            }
            pending
        };

        // 批量嵌入新工具描述（失败的工具在后续循环中跳过）
        if !to_embed.is_empty() {
            let texts: Vec<String> = to_embed.iter().map(|(_, d)| d.clone()).collect();
            match self.provider.embed_batch(&texts) {
                Ok(embs) => {
                    let mut cache = self.embeddings.lock();
                    for ((name, desc), emb) in to_embed.into_iter().zip(embs.into_iter()) {
                        cache.insert((name, lang.to_string()), (desc, emb));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[ToolSemanticFilter] 批量嵌入工具描述失败，跳过 {} 个新工具: {}",
                        texts.len(),
                        e
                    );
                }
            }
        }

        // 计算相似度并排序
        let cache = self.embeddings.lock();
        let mut scored: Vec<ToolRecommendation> = tools
            .iter()
            .filter_map(|tool| {
                let name = tool.name();
                let desc = tool.description_in(lang).to_string();
                let cache_key = (name.to_string(), lang.to_string());
                let emb = cache.get(&cache_key)?.1.as_slice();
                let sim = cosine_similarity(query_emb, emb);
                if sim < min_sim {
                    return None;
                }
                Some(ToolRecommendation {
                    name: name.to_string(),
                    description: desc,
                    similarity: sim as f64,
                })
            })
            .collect();

        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_n);
        scored
    }

    /// 便捷方法：使用默认参数筛选
    pub fn filter_default(
        &self,
        tool_system: &ToolSystem,
        query_emb: &[f32],
    ) -> Vec<ToolRecommendation> {
        self.filter(tool_system, query_emb, DEFAULT_TOP_N, MIN_SIM_THRESHOLD)
    }

    /// 清除嵌入缓存（工具列表大变更时调用，如 MCP 重连）
    pub fn clear_cache(&self) {
        self.embeddings.lock().clear();
    }
}

/// 判断是否应该触发工具语义筛选
///
/// 仅在用户意图明确指向工具使用或请求时触发，避免无谓的嵌入计算。
pub fn should_filter_tools(intent_label: &str) -> bool {
    matches!(intent_label, "tool_request" | "request" | "question")
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-10 || nb < 1e-10 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_filter_tools() {
        assert!(should_filter_tools("tool_request"));
        assert!(should_filter_tools("request"));
        assert!(should_filter_tools("question"));
        assert!(!should_filter_tools("chat"));
        assert!(!should_filter_tools("sharing"));
        assert!(!should_filter_tools("goodbye"));
    }

    #[test]
    fn test_empty_query_returns_empty() {
        let provider: Arc<dyn MemoryEmbeddingProvider> =
            Arc::new(crate::memory::embedding::HashingMemoryEmbedding::new(256));
        let filter = ToolSemanticFilter::new(provider, "zh".to_string());
        let ts = ToolSystem::new();
        let result = filter.filter_default(&ts, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }
}
