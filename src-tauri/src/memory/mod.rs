//! Memory 模块：统一记忆管理系统。
//!
//! 包含：记忆管理器、类型定义、过滤器、过期合并、时间戳记忆、自动提取器、
//! 嵌入服务、向量检索、写入时 LLM 增强、混合检索、陈旧度提示、分词器。

pub mod age;
pub mod auto_extractor;
pub mod conflict;
pub mod consolidation;
pub mod embedding;
pub mod entity_extract;
pub mod episode;
pub mod evidence;
pub mod event_log;
pub mod filter;
pub mod graph_store;
pub mod hooks;
pub mod ivf_index;
pub mod llm_enricher;
pub mod manager;
pub mod memory_router;
pub mod ollama_service;
pub mod pipeline;
pub mod precision_filter;
pub mod redact;
pub mod recycle_bin;
pub mod refiner;
pub mod relaxation;
pub mod relational_recall;
pub mod retention;
pub mod retriever;
pub mod session_compressor;
pub mod strategy;
pub mod takes_fence;
pub mod time_anchor;
pub mod unified_event_ledger;
pub mod time_stamped;
pub mod tokenize;
pub mod topic_merger;
pub mod world_knowledge;
pub mod types;
pub mod user_facts;
pub mod verifier;
pub mod vector_search;

pub use age::{bump_visit, compute_heat_score, init_heat, staleness_text};
pub use embedding::{build_embedding, default_embedding, HashingMemoryEmbedding, MemoryEmbeddingProvider};
pub use evidence::{
    apply_evidence_signal, apply_migration_seed, derive_status, evidence_score,
    effective_disputation, effective_reinforcement, maybe_mark_sub_zero, EvidenceSnapshot,
    EvidenceSource, EvidenceStatus, SignalKind,
};
pub use event_log::{EventLog, EventRecord, EventType};
pub use hooks::{HookJudge, HookJudgeLlmClient};
pub use llm_enricher::{EnricherLlmClient, EnrichedMeta, MemoryEnricher};
pub use manager::MemoryManager;
pub use memory_router::{route_sync, route_with_llm, MemoryDestination, RouteContext, RouterLlmClient};
pub use pipeline::{ConsolidationPipeline, ConsolidationReport};
pub use precision_filter::{apply_precision_filter, exclude_visible_context, EntityScope, PrecisionFilterCriteria};
pub use relaxation::{RelaxationLadder, STAGE_NAMES as RELAXATION_STAGE_NAMES};
pub use retention::{MemoryExpirationRule, MemoryRetentionPolicy, MemoryRetentionGuard, QuadraticDecay};
pub use refiner::{keyword_prefilter, llm_refine, refine_candidates};
pub use recycle_bin::{RecycleBin, RecycleBinData, RecycleEntry};
pub use retriever::{attach_items, attach_items_with_weights, expand_context, hybrid_search, RetrievalHit, RetrievalResultType, RetrievalWeights};
pub use strategy::{
    create_strategy, AutoStrategy, HybridStrategy, KeywordStrategy, MemoryRetrievalStrategy,
    RetrievalContext, VectorStrategy,
};
pub use time_anchor::apply_time_anchor;
pub use time_stamped::estimate_tokens;
pub use tokenize::tokenize;
pub use unified_event_ledger::{
    register_event, unified_event_ledger, EventVisibility,
    UnifiedEvent, UnifiedEventLedger,
};
pub use world_knowledge::{world_knowledge, WorldFact, WorldFactCategory, WorldKnowledgeEngine};
pub use types::{Granularity, MemoryItem, MemoryType, OpenHook, RetrievalStrategy, SemanticType};
pub use user_facts::{FactLlmClient, L1RecentState, UserFact, UserFactStore, UserFactType};
pub use verifier::{verify_retrieval, VerifierLlmClient};
pub use vector_search::{cosine_similarity, should_index, MemoryVector, MemoryVectorStore};
pub use ivf_index::{IvfConfig, IvfIndex, IvfStats};
pub use entity_extract::{extract as extract_entities, Entity, EntityType, Relation, RelationType};
pub use episode::{EpisodeIndex, EpisodeStore, EpisodeStoreData};
pub use graph_store::{FanoutResult, GraphEdge, GraphEntity, GraphStoreData, KnowledgeGraph};
pub use relational_recall::{build_relational_arm, parse_relational_query, ParsedRelationalQuery, RelationalHit, RelationalKind, RelationDirection};
pub use takes_fence::{TakesFence, TakesRow, TakesTable};
pub use topic_merger::{MergeReport, TopicMerger};
