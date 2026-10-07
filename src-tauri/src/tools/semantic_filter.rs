//! 工具语义筛选器
//!
//! 基于 FastSemanticAnalyzer 的预嵌入能力，对工具描述做语义匹配，
//! 从全量工具中筛选出与用户输入最相关的 Top-N 工具。
//!
//! Current message and recent context are scored separately (80/20); learned phrases
//! have independent vectors and never replace builtin descriptions.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::types::Tool;
use super::ToolSystem;
use crate::memory::embedding::MemoryEmbeddingProvider;

/// 单个推荐工具的元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecommendation {
    /// 工具名
    pub name: String,
    /// 工具描述（用于 prompt 展示）
    pub description: String,
    /// 与用户输入的语义相似度 [0, 1]
    pub similarity: f64,
    pub current_similarity: f64,
    pub context_similarity: f64,
}

/// 工具语义筛选器
///
/// 持有 embedding provider 引用和工具描述嵌入缓存。
/// 缓存 key 为 (工具名, 语言, 描述内容) 的组合，描述变化或语言切换时自动重新嵌入。
pub struct ToolSemanticFilter {
    provider: Arc<dyn MemoryEmbeddingProvider>,
    /// 界面语言（"zh"/"en"/"ja"），决定调用 `Tool::description_in(lang)` 取哪一版描述
    language: String,
    pub(crate) corpus: Arc<super::usage_corpus::ToolUsageCorpus>,
    /// (工具名, 语言) -> (描述文本, 嵌入向量)
    ///
    /// 嵌入向量在首次查询时懒加载。当工具描述变化或语言切换时，
    /// 下次查询检测到描述不匹配会自动重新嵌入。
    embeddings: Mutex<HashMap<(String, String), (String, Vec<f32>)>>,
}

impl ToolSemanticFilter {
    /// 构造工具的嵌入文本：描述 + 使用场景语料
    ///
    /// 只嵌描述时，检索是"功能描述 ↔ 用户提问"的跨说法匹配；
    /// 拼上 `usage_corpus`（用户真实可能的说法）后，变成
    /// "用户说法 ↔ 用户说法"，相似度分布更集中，Top-N 命中率更高。
    /// 工具未提供语料时退化为仅描述，行为与改动前一致。
    fn embed_text_of(tool: &dyn Tool, lang: &str) -> String {
        let desc = tool.description_in(lang);
        let corpus = tool.usage_corpus(lang);
        if corpus.is_empty() {
            desc.to_string()
        } else {
            format!("{}\n{}", desc, corpus)
        }
    }

    pub fn new(provider: Arc<dyn MemoryEmbeddingProvider>, language: String) -> Self {
        Self {
            provider,
            language,
            corpus: Arc::new(super::usage_corpus::ToolUsageCorpus::new(None)),
            embeddings: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_corpus_path(mut self, path: std::path::PathBuf) -> Self {
        self.corpus = super::usage_corpus::ToolUsageCorpus::shared(path);
        self
    }
    pub(crate) fn learn_usage(&self, candidates: Vec<super::usage_corpus::UsageCandidate>,
        batch: &super::usage_corpus::EvolutionBatch, system: &ToolSystem) -> usize {
        self.corpus.learn(candidates, batch, self.provider.as_ref(), system, &self.language)
    }
    /// Blocking embedding work; callers use a blocking worker.
    pub(crate) fn embed_query(&self, query: &str) -> Result<Vec<f32>, String> {
        self.provider.embed(query).map_err(|error| error.to_string())
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
                let desc = Self::embed_text_of(tool.as_ref(), lang);
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
        self.filter_weighted(tool_system, query_emb, None, top_n, min_sim, false)
    }

    pub fn filter_weighted(&self, tool_system: &ToolSystem, current: &[f32],
        context: Option<&[f32]>, top_n: usize, min_sim: f32, reference: bool) -> Vec<ToolRecommendation> {
        if !valid_query(current) { return Vec::new(); }
        self.preload(tool_system);
        let learned = self.corpus.embeddings(self.provider.as_ref(), tool_system, &self.language);
        let context = context.filter(|v| v.len() == current.len() && valid_query(v));
        let cache = self.embeddings.lock();
        let mut scored = Vec::new();
        for tool in tool_system.list_tools() {
            if tool.name() == "tool_search" || crate::tools::registry::is_work_agent_only(tool.name()) { continue; }
            let Some((_, vector)) = cache.get(&(tool.name().to_string(), self.language.clone())) else { continue; };
            if vector.len() != current.len() || !valid_query(vector) { continue; }
            let mut current_sim = cosine_similarity(current, vector).max(0.0);
            let mut context_sim = context.map(|q| cosine_similarity(q, vector).max(0.0)).unwrap_or(0.0);
            for entry in learned.iter().filter(|e| e.tool_name == tool.name()) {
                current_sim = current_sim.max(0.95 * cosine_similarity(current, &entry.embedding).max(0.0));
                context_sim = context_sim.max(context.map(|q| 0.95 * cosine_similarity(q, &entry.embedding).max(0.0)).unwrap_or(0.0));
            }
            let sim = if context.is_some() { 0.8 * current_sim + 0.2 * context_sim } else { current_sim };
            // Old context cannot revive a tool on a new topic. Explicit short references may rely on context.
            let threshold = if reference && context.is_some() { min_sim.min(0.12) } else { min_sim };
            if sim < threshold || (!reference && current_sim < 0.18) { continue; }
            scored.push(ToolRecommendation { name: tool.name().into(), description: tool.description_in(&self.language).into(),
                similarity: sim as f64, current_similarity: current_sim as f64, context_similarity: context_sim as f64 });
        }
        scored.sort_by(|a,b| b.similarity.total_cmp(&a.similarity).then_with(|| a.name.cmp(&b.name)));
        scored.truncate(top_n);
        scored
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

fn valid_query(v: &[f32]) -> bool {
    !v.is_empty() && v.iter().all(|x| x.is_finite()) && v.iter().any(|x| *x != 0.0)
}

pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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
pub(crate) mod tests {
    use super::*;

    pub(crate) struct FixtureEmbedding { pub model: &'static str }
    impl MemoryEmbeddingProvider for FixtureEmbedding {
        fn dimension(&self) -> usize { 3 }
        fn model_id(&self) -> &str { self.model }
        fn embed(&self, text: &str) -> crate::error::VivianResult<Vec<f32>> {
            Ok(if text.contains("photo") { vec![1.0,0.0,0.0] }
                else if text.contains("code") { vec![0.0,1.0,0.0] } else { vec![0.0,0.0,1.0] })
        }
    }
    pub(crate) struct FixtureTool { pub name: &'static str, pub description: &'static str }
    #[async_trait::async_trait]
    impl super::super::types::Tool for FixtureTool {
        fn name(&self) -> &str { self.name }
        fn description(&self) -> &str { self.description }
        fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({"type":"object","properties":{}}) }
        async fn validate_input(&self, _: &serde_json::Value, _: &super::super::types::ToolUseContext) -> super::super::types::ValidationResult { super::super::types::ValidationResult::success(None) }
        async fn check_permissions(&self, _: &serde_json::Value, _: &super::super::types::ToolUseContext) -> super::super::types::PermissionResult { super::super::types::PermissionResult::allow() }
        async fn call(&self, _: serde_json::Value, _: &super::super::types::ToolUseContext) -> super::super::types::ToolResult { super::super::types::ToolResult::success(serde_json::json!(null)) }
        fn is_read_only(&self) -> bool { true }
        fn category(&self) -> super::super::types::ToolCategory { super::super::types::ToolCategory::System }
    }
    pub(crate) fn fixture_system() -> ToolSystem {
        let system = ToolSystem::new();
        system.register_tool(Arc::new(FixtureTool { name: "photo_tool", description: "photo" }));
        system.register_tool(Arc::new(FixtureTool { name: "code_tool", description: "code" }));
        system
    }
    #[test]
    fn current_message_dominates_context_and_context_breaks_ambiguous_ties() {
        let filter = ToolSemanticFilter::new(Arc::new(FixtureEmbedding {model:"one"}), "en".into());
        let system = fixture_system();
        let ranked = filter.filter_weighted(&system, &[1.0,0.0,0.0], Some(&[0.0,1.0,0.0]), 2, 0.22, false);
        assert_eq!(ranked.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["photo_tool"]);
        assert!((ranked[0].similarity - 0.8).abs() < 1e-6);
        let ranked = filter.filter_weighted(&system, &[1.0,1.0,0.0], Some(&[0.0,1.0,0.0]), 1, 0.22, false);
        assert_eq!(ranked[0].name, "code_tool");
        assert_eq!(ranked.len(), 1);
    }
    #[test]
    fn context_cannot_revive_old_tools_except_explicit_followup_references() {
        let filter = ToolSemanticFilter::new(Arc::new(FixtureEmbedding {model:"one"}), "en".into());
        let system = fixture_system();
        assert!(filter.filter_weighted(&system, &[0.0,0.0,1.0], Some(&[1.0,0.0,0.0]), 4, 0.22, false).is_empty());
        assert_eq!(filter.filter_weighted(&system, &[0.0,0.0,1.0], Some(&[1.0,0.0,0.0]), 4, 0.22, true)[0].name, "photo_tool");
        assert!(filter.filter_weighted(&system, &[f32::NAN,0.0,0.0], None, 4, 0.22, false).is_empty());
        assert_eq!(filter.filter_weighted(&system, &[1.0,0.0,0.0], Some(&[1.0]), 1, 0.22, false)[0].similarity, 1.0);
    }
    #[test]
    fn learned_phrase_changes_retrieval_without_changing_builtin_descriptions() {
        let filter = ToolSemanticFilter::new(Arc::new(FixtureEmbedding {model:"one"}), "en".into());
        let system = fixture_system();
        let names = vec!["photo_tool".into()];
        assert!(filter.filter(&system, &[0.0,0.0,1.0], 1, 0.22).is_empty());
        for _ in 0..4 { filter.corpus.observe("nickname", &names, &system); }
        let batch = filter.corpus.begin_evolution().unwrap();
        let evidence_ids = batch.observations.iter().take(2).map(|o|o.id.clone()).collect();
        assert_eq!(filter.learn_usage(vec![super::super::usage_corpus::UsageCandidate {
            tool_name:"photo_tool".into(), utterance:"nickname".into(), evidence_ids }], &batch, &system),1);
        assert_eq!(filter.filter(&system, &[0.0,0.0,1.0], 1, 0.22)[0].name, "photo_tool");
        assert_eq!(system.find_tool("photo_tool").unwrap().description(), "photo");
        filter.corpus.finish_evolution(&batch);
    }

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
        let result = filter.filter(&ts, &[], 5, 0.22);
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
