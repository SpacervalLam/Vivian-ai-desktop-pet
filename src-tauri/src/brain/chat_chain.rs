//! BrainChatChain —— 对话流水线编排器，集成 Memory 子系统。
//!
//! - **MemoryFilter**：检索阶段跨会话过滤临时话题、保留长期偏好
//! - **UserMemorySavingRunnable**：用户消息早期保存（生成前）
//! - **MemorySavingRunnable**：AI 回复 + 长期记忆保存（生成后）
//! - **TimeStampedMemory**：40 阈值摘要容器，保留最近 8 条
//! - **AutoExtractor**：对话后 LLM 自动抽取 ADD/UPDATE/DELETE 记忆
//! - **MemoryRetentionGuard**：定期过期清理（casual 24h/100、temporary 6h/50、long_term 720h+imp<0.3）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::brain::control_action_executor::ControlActionExecutor;
use crate::brain::focus_mode::{compute_focus_score, FocusState, FocusThresholds};
use crate::brain::rate_limiter::RateLimiterRegistry;
use crate::dialogue::DialogueManager;
use crate::emotion::EmotionBridge;
use crate::error::VivianResult;
use crate::memory::auto_extractor::AutoExtractor;
use crate::memory::hooks::HookJudge;
use crate::memory::pipeline::ConsolidationPipeline;
use crate::memory::retention::MemoryRetentionGuard;
use crate::memory::filter::MemoryFilter;
use crate::memory::llm_enricher::{EnricherLlmClient, MemoryEnricher};
use crate::memory::time_stamped::TimeStampedMemory;
use crate::memory::types::{MemoryType, RetrievalStrategy};
use crate::memory::user_facts::{FactLlmClient, UserFactStore};
use crate::memory::user_model::UserModelManager;
use crate::memory::MemoryManager;
use crate::config::manager::AppConfig;
use crate::persona::{DynamicBehaviorProfile, PersonaEngine};
use crate::pet_controller::PetController;
use crate::psychology::{PsychologyManager, PsychologyOutput};
use crate::tools::tool_call_manager::ToolCallManager;
use crate::tools::types::ToolUseContext;
use crate::tools::ToolSystem;
use crate::utils::EnvironmentManager;
use crate::utils::truncate_chars;
use crate::pipeline::advisor::{
    AdvisorChain, LoggingAdvisor, LoopDetectionAdvisor, RateLimitAdvisor, Re2Advisor,
};
use crate::pipeline::base::{Runnable, RunnableConfig, RunnableSequence, TimingMiddleware};
use crate::pipeline::state::PipelineState;
use crate::pipeline::steps::generation::{
    AIResponseGenerationRunnable, ResponseParsingRunnable, SharedStreamEmitter, StreamEmitter,
    new_shared_stream_emitter,
};
use crate::pipeline::steps::memory::{
    MemoryRetrievalStep, MemorySavingRunnable, UserMemorySavingRunnable,
};
use crate::pipeline::steps::mood::MoodStep;
use crate::pipeline::steps::pre_processing::PreProcessingStep;
use crate::pipeline::steps::prompt::PromptBuildingStep;
use crate::pipeline::steps::query_rewrite::QueryRewriteStep;
use crate::pipeline::steps::fast_semantic_step::{FastSemanticStep, ParallelStep};
use crate::pipeline::steps::reflection::ReflectionRunnable;
use crate::pipeline::steps::web_context::WebContextRunnable;
use crate::providers::ModelRouter;
use crate::types::response::{AiResponse, ChatMessage};

/// Consolidator 触发间隔：每 N 次对话后执行一次过期清理
const CONSOLIDATOR_INTERVAL: u64 = 10;

/// 批量累积器：将每轮对话消息累积到缓冲区，达到阈值时一次性处理。
///
/// 用于降低 AutoExtractor 和 Psychology Insight 等辅助 LLM 调用的频率：
/// - 每轮调用 `record_turn()` 追加消息
/// - 达到阈值时返回所有累积消息
/// - 调用方获取累积消息后一次性调用 LLM 处理
///
/// 设计原则：累积全部轮次内容，不丢失任何信息，仅减少 API 调用次数。
struct BatchAccumulator {
    /// 累积的对话消息缓冲区（user + assistant 交替）
    buffer: parking_lot::Mutex<Vec<ChatMessage>>,
    /// 累积轮次计数器
    turn_count: std::sync::atomic::AtomicU64,
    /// 触发阈值（累积多少轮后触发）
    threshold: u64,
    /// 首条消息入库时间（用于时间窗口过期检测）
    first_turn_at: parking_lot::Mutex<Option<std::time::Instant>>,
}

const BATCH_TIME_WINDOW_SECS: u64 = 600;

impl BatchAccumulator {
    fn new(threshold: u64) -> Self {
        Self {
            buffer: parking_lot::Mutex::new(Vec::new()),
            turn_count: std::sync::atomic::AtomicU64::new(0),
            threshold,
            first_turn_at: parking_lot::Mutex::new(None),
        }
    }

    /// 记录一轮对话（user_msg + ai_msg），达到阈值时返回累积的所有消息。
    ///
    /// 返回 `Some(batch)` 当累积轮次达到阈值，包含全部累积的对话消息。
    /// 返回 `None` 表示继续累积。
    fn record_turn(&self, user_msg: &ChatMessage, ai_msg: &ChatMessage) -> Option<Vec<ChatMessage>> {
        let mut buf = self.buffer.lock();
        let mut first_at = self.first_turn_at.lock();
        let now = std::time::Instant::now();
        if first_at.is_none() {
            *first_at = Some(now);
        } else if let Some(at) = *first_at {
            if at.elapsed().as_secs() >= BATCH_TIME_WINDOW_SECS {
                self.turn_count.store(0, std::sync::atomic::Ordering::Relaxed);
                buf.clear();
                *first_at = Some(now);
            }
        }
        buf.push(user_msg.clone());
        buf.push(ai_msg.clone());
        let count = self.turn_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if count >= self.threshold {
            self.turn_count.store(0, std::sync::atomic::Ordering::Relaxed);
            *first_at = None;
            let batch = std::mem::take(&mut *buf);
            Some(batch)
        } else {
            None
        }
    }
}

/// BrainChatChain —— 完整对话链，集成 Memory 子系统。
///
/// 步骤顺序（顺序模式）：
/// 1. PreProcessing（输入 trim + 命令检测）
/// 2. UserMemorySaving（用户消息早期保存）
/// 3. MemoryRetrieval（带 MemoryFilter 的检索）
/// 4. PromptBuilding（Prompt 组装）
/// 5. Generation（AI 响应生成）
/// 6. Mood（心情提取）
/// 7. MemorySaving（AI 回复 + 长期记忆保存）
///
/// 链执行完成后，`ainvoke` 还会触发后处理：
/// - 将 user/ai 消息写入 TimeStampedMemory 容器（40 阈值摘要）
/// - 调用 AutoExtractor 从对话中抽取长期记忆
/// - 每 `CONSOLIDATOR_INTERVAL` 次调用一次 MemoryRetentionGuard 清理过期记忆
pub struct BrainChatChain {
    pub router: Arc<ModelRouter>,
    pub memory: Arc<MemoryManager>,
    /// 跨会话记忆过滤器（新会话只保留长期偏好）
    pub memory_filter: Arc<RwLock<MemoryFilter>>,
    /// LLM 自动记忆抽取器
    pub auto_extractor: Arc<AutoExtractor>,
    /// 过期清理 + 去重合并（保留策略守卫）
    pub consolidator: Arc<MemoryRetentionGuard>,
    /// 是否启用记忆过期清理（来自 config.memory.enable_expiration，reinitialize 时刷新）
    pub enable_expiration: bool,
    /// 巩固流水线（Stage 1/2/3：ShortTerm→MidTerm→LongTerm→Insight）
    pub pipeline: Arc<ConsolidationPipeline>,
    /// 时间戳记忆容器（40 阈值摘要，保留最近 8 条）
    pub time_stamped: Arc<RwLock<TimeStampedMemory>>,
    /// 心理系统管理器：五层心理架构 + 关系系统（已整合原 RelationshipManager）
    pub psychology: Arc<PsychologyManager>,
    /// 情绪桥接器：用于读取 Vivian 当前主导情绪
    pub emotion_bridge: Arc<EmotionBridge>,
    /// 对话历史管理器：从磁盘加载并注入 state.messages
    pub dialogue: Arc<DialogueManager>,
    /// 桌宠自控动作执行器（None 时跳过 control_actions）
    pub control_action_executor: Option<Arc<ControlActionExecutor>>,
    /// 工具调用管理器：每次对话前刷新 context，让工具感知情绪/关系/记忆
    pub tool_call_manager: Option<Arc<ToolCallManager>>,
    /// 用户事实画像存储（name/age/gender/occupation/location + 自由事实）
    pub user_facts: Arc<UserFactStore>,
    /// Open Hooks 闭环判定器：对话后异步检查未闭环钩子
    pub hook_judge: Arc<HookJudge>,
    /// 智能体动态行为画像：跟踪近期交互模式，注入 prompt 动态段
    pub dynamic_profile: Arc<DynamicBehaviorProfile>,
    /// Advisor 链：日志 / 限流 / Re2 / 循环检测，包裹整个 RunnableSequence
    pub advisor_chain: AdvisorChain,
    /// 流式 chunk 推送回调（与 AIResponseGenerationRunnable 共享同一 Arc<RwLock<...>>）
    ///
    /// 由 `chat:send_message_stream` 命令在调用 `think` 前通过 `set_stream_emitter` 注入，
    /// 调用结束后清理为 None。非流式调用（`think(stream=false)`）不应设置此回调。
    pub stream_emitter: SharedStreamEmitter,
    /// 世界状态提供者：注入后 prompt 注入天气/节气/节日等真实世界感知
    pub world_provider: Option<Arc<crate::world::WorldStateProvider>>,
    /// 凝神/专注模式状态机（与 Brain 共享同一实例）
    pub focus_state: Arc<tokio::sync::Mutex<FocusState>>,
    /// 角色 ID（多角色架构下标识当前链所属角色，注入 ToolUseContext 供工具路由）
    pub char_id: String,
    /// 界面语言（从 config.base.language 读取，供 prompt 段落三语化使用）
    pub language: String,
    /// 在场状态管理器（可选，注入后 prompt 注入当前状态供 LLM 感知）
    pub presence: Option<Arc<crate::presence::PresenceManager>>,
    /// 自我状态聚合器（可选，注入后 prompt 注入 SelfState 快照供 LLM 感知）
    pub self_state: Option<Arc<crate::self_state::SelfState>>,
    /// Mind 引用（可选，注入后用于推入工作记忆）
    pub mind: Option<Arc<crate::mind::Mind>>,
    /// 快速语义分析器（可选，注入后在 prepare_pipeline_state 阶段填充 fast_perception）
    pub fast_semantic: Option<Arc<crate::emotion::FastSemanticAnalyzer>>,
    /// 工具语义筛选器（可选，注入后 PromptBuildingStep 在 intent=tool_request 时调用）
    pub tool_semantic_filter: Option<Arc<crate::tools::ToolSemanticFilter>>,
    /// Prompt 构建步骤（与主对话流水线共享同一份配置齐全的实例，
    /// 供 ProactiveOrchestrator 复用主对话完整 prompt：人设/记忆/知识库/环境/用户画像等）
    pub prompt_step: PromptBuildingStep,
    /// 对话调用计数（用于定期触发 Consolidator）
    invoke_count: AtomicU64,
    /// 批量累积器：累积 3 轮对话后一次性触发 AutoExtractor 和心理学批量分析
    batch_accumulator: BatchAccumulator,
    /// 话题信号缓冲：检测话题切换 + 稳定后慢存储到记忆
    pub topic_signal_buffer: Arc<super::topic_signal::TopicSignalBuffer>,
    /// 用户认知模型管理器：将散落的记忆证据组织成"对这个人的理解"
    ///
    /// 负责：
    /// - 管理 UserTrait（偏好/工作风格/价值观等稳定抽象）
    /// - 管理 UserGoal（长期/当前/已完成目标）
    /// - 管理 UserProject（项目生命周期 + 当前注意力激活）
    /// - 强证据在线更新（规则匹配，不调用 LLM）
    /// - 在 prompt 中注入"我对你的了解"段落，让 LLM 感知长期认知
    pub user_model: Arc<UserModelManager>,
}

impl BrainChatChain {
    /// 构造完整对话链 —— 自动初始化所有 Memory 子系统组件。
    pub fn new(
        router: Arc<ModelRouter>,
        memory: Arc<MemoryManager>,
        persona: Arc<PersonaEngine>,
        emotion_bridge: Arc<EmotionBridge>,
        tool_system: Arc<ToolSystem>,
        dialogue: Arc<DialogueManager>,
        psychology: Arc<PsychologyManager>,
        environment: Arc<EnvironmentManager>,
        config: &AppConfig,
        world_provider: Option<Arc<crate::world::WorldStateProvider>>,
        world_state: Option<Arc<crate::world::WorldState>>,
        research: Option<Arc<crate::research::ResearchManager>>,
        activity_journal: Arc<crate::proactive::ActivityJournal>,
        manifest: Arc<crate::engine::manifest::ResourceManifest>,
        mind: Arc<crate::mind::Mind>,
        char_id: &str,
        tool_semantic_filter: Option<Arc<crate::tools::ToolSemanticFilter>>,
        fast_semantic: Option<Arc<crate::emotion::FastSemanticAnalyzer>>,
    ) -> Self {
        // 初始化 Memory 子系统组件
        let memory_filter = Arc::new(RwLock::new(MemoryFilter::new()));
        // 注入写入时 LLM 增强器：让 add_memory_enriched 调用 LLM 抽取 description/keywords/importance
        let enricher_llm: Arc<dyn EnricherLlmClient> = router.clone();
        memory.set_enricher(Arc::new(MemoryEnricher::new(enricher_llm)));
        let auto_extractor = Arc::new(
            AutoExtractor::new()
                .with_llm(router.clone())
                .with_memory(memory.clone()),
        );
        let consolidator = Arc::new(MemoryRetentionGuard::new());
        let pipeline = Arc::new(ConsolidationPipeline::new(
            router.clone(),
            config.memory.consolidation.clone(),
        ));
        // 注入锁定核心文本：让反思明确哪些人设字段不可修改
        pipeline.set_locked_core(persona.get_config().locked_core_summary());
        let time_stamped = Arc::new(RwLock::new(TimeStampedMemory::new()));

        // 用户事实画像：从对话中提取 name/age/gender/occupation/location + 自由事实
        // 走 memory 路由（高频低复杂度），智能合并（旧值优先，冲突时 LLM 仲裁）
        let fact_llm: Arc<dyn FactLlmClient> = router.clone();
        let user_facts = Arc::new(UserFactStore::new(Some(fact_llm), char_id).unwrap_or_else(|e| {
            tracing::warn!("[BrainChatChain] UserFactStore 初始化失败，用户事实提取将不可用: {}", e);
            UserFactStore::new(None, char_id).unwrap_or_else(|_| UserFactStore::fallback())
        }));

        // Open Hooks 闭环判定器：走 memory 路由，对话后异步检查未闭环钩子
        let hook_judge = Arc::new(HookJudge::from_router(router.clone()));

        // 智能体动态行为画像：跟踪近期交互模式（话题/情绪/消息长度），注入 prompt 动态段
        let dynamic_profile = Arc::new(DynamicBehaviorProfile::new().unwrap_or_else(|e| {
            tracing::warn!("[BrainChatChain] DynamicBehaviorProfile 初始化失败，动态行为画像将不可用: {}", e);
            DynamicBehaviorProfile::fallback()
        }));

        // 从配置读取检索策略与过期清理开关
        let retrieval_strategy = RetrievalStrategy::from_str(&config.memory.retrieval_strategy);
        let enable_expiration = config.memory.enable_expiration;

        // 用户认知模型管理器：从磁盘加载已有用户模型数据，无需 LLM
        // 将散落的记忆证据组织成"对这个人的理解"，在 prompt 中注入"我对你的了解"段落
        let user_model = Arc::new(UserModelManager::new(char_id));
        // 注入巩固流水线：Stage 3 末尾把 Insight 归并为概念层（UserModel + 图谱）
        pipeline.set_user_model(user_model.clone());

        // 组装流水线步骤（每步用 TimingMiddleware 包装，输出 stage 耗时日志）
        let mut steps = RunnableSequence::new();
        steps.add_step(Box::new(TimingMiddleware::new(
            "pre_processing",
            Box::new(PreProcessingStep::new()),
        )));
        // 用户消息早期保存
        steps.add_step(Box::new(TimingMiddleware::new(
            "user_memory_saving",
            Box::new(UserMemorySavingRunnable::with_memory(memory.clone())),
        )));
        // 查询重写 ∥ 快速语义感知：两者互不依赖，并行执行缩短用户等待时间
        // - QueryRewriteStep：LLM 改写查询（用于记忆检索）
        // - FastSemanticStep：嵌入分类（情绪/意图/话题，用于 prompt 动态组装）
        // 并行后耗时 = max(LLM改写, 嵌入) 而非 sum
        if let Some(fast_sem) = &fast_semantic {
            let parallel = ParallelStep::new(
                Box::new(QueryRewriteStep::new(router.clone(), memory.clone())),
                Box::new(FastSemanticStep::new(fast_sem.clone(), char_id.to_string())),
            );
            steps.add_step(Box::new(TimingMiddleware::new(
                "query_rewrite_and_fast_semantic",
                Box::new(parallel),
            )));
        } else {
            steps.add_step(Box::new(TimingMiddleware::new(
                "query_rewrite",
                Box::new(QueryRewriteStep::new(router.clone(), memory.clone())),
            )));
        }
        // 记忆检索（带 MemoryFilter 跨会话过滤，策略由 config.memory.retrieval_strategy 决定）
        // 注入 Mind 启用 Attention-weighted 重排序：当前注意力聚焦的实体相关记忆优先保留
        steps.add_step(Box::new(TimingMiddleware::new(
            "memory_retrieval",
            Box::new(
                MemoryRetrievalStep::with_filter_and_strategy(
                    memory.clone(),
                    memory_filter.clone(),
                    retrieval_strategy,
                )
                .with_mind(mind.clone())
                .with_router(router.clone())
                .with_user_model(user_model.clone()),
            ),
        )));
        // Prompt 组装：注入 PsychologyManager，提供五层心理架构 + 关系上下文
        // 同时注入 ToolSystem，启用场景化工具筛选（按情绪/关系阶段/前台应用过滤）
        // 同时注入 UserFactStore，提供结构化用户事实画像
        let prompt_step = PromptBuildingStep::with_engines(
            persona.clone(),
            emotion_bridge.clone(),
        )
        .with_psychology(psychology.clone())
        .with_tool_system(tool_system.clone())
        .with_environment(environment.clone())
        .with_user_facts(user_facts.clone())
        .with_dynamic_profile(dynamic_profile.clone())
        .with_memory(memory.clone())
        .with_activity_journal(activity_journal.clone())
        .with_mind(mind.clone())
        .with_char_id(char_id)
        .with_native_fc(config.tools.enable_native_function_calling)
        .with_native_schema(router.supports_structured_output())
        .with_language(config.base.language.clone());
        // Inject inline expression/motion tag config: when enabled, inject tag format spec + available expression/motion list into prompt
        let prompt_step = if config.inline_expression.enabled {
            let names = format!(
                "Available expressions: {}\nAvailable motions: {}",
                manifest.expressions().join(", "),
                manifest.motions().join(", "),
            );
            prompt_step.with_inline_expression(true, Some(names))
        } else {
            prompt_step
        };
        // 注入 EpisodeStore，启用 Relevant Episode 段落（最近经历摘要）
        let prompt_step = if let Some(ep_store) = memory.episode_store() {
            prompt_step.with_episode_store(ep_store)
        } else {
            prompt_step
        };
        // 注入世界状态提供者（若启用），让 prompt 包含天气/节气/节日等真实世界感知
        let prompt_step = if let Some(wp) = world_provider.clone() {
            prompt_step.with_world(wp)
        } else {
            prompt_step
        };
        // 注入世界状态核心（若启用），让 prompt 包含近期活动观察（异常检测）
        let prompt_step = if let Some(ws) = world_state.clone() {
            prompt_step.with_world_state(ws)
        } else {
            prompt_step
        };
        // 注入用户研究管理器（若启用），让 prompt 包含活跃研究课题和已确认习惯
        let prompt_step = if let Some(r) = research.clone() {
            prompt_step.with_research(r)
        } else {
            prompt_step
        };
        // 注入 ToneInjector，启用场景语气注入（每轮对话匹配用户输入场景，命中后注入参考台词）
        // 使用 memory 的 embedding 服务：若配置了远程 embedding 则用远程（语义匹配更准），
        // 否则用默认哈希嵌入（仅关键词匹配生效）
        let tone_injector = Arc::new(crate::persona::ToneInjector::with_embedding(
            &char_id,
            memory.embedding(),
        ));
        let prompt_step = prompt_step.with_tone_injector(tone_injector);
        // 注入工具语义筛选器（启用后 PromptBuildingStep 在 intent=tool_request 时做语义粗筛）
        let prompt_step = if let Some(filter) = tool_semantic_filter.clone() {
            prompt_step.with_tool_semantic_filter(filter)
        } else {
            prompt_step
        };
        // 注入话题驱动背景知识管理器：扫描用户输入命中关键词后激活对应 topic，
        // 在 prompt 中注入背景知识段落，duration_turns 轮后进入 cooldown_seconds 冷却
        let topic_injection_mgr = Arc::new(crate::pipeline::topic_injection::TopicInjectionManager::new());
        let prompt_step = prompt_step.with_topic_injection(topic_injection_mgr);
        // 克隆一份供 ProactiveOrchestrator 复用主对话完整 prompt（人设/记忆/知识库/环境等）
        let prompt_step_shared = prompt_step.clone();
        // Web 检索决策：在生成前根据用户问题决定是否开启联网搜索
        steps.add_step(Box::new(TimingMiddleware::new(
            "web_context",
            Box::new(WebContextRunnable::with_router(router.clone())),
        )));
        // Prompt 组装：注入人设/记忆/工具/认知信号/主动搜索结果等上下文
        steps.add_step(Box::new(TimingMiddleware::new(
            "prompt_building",
            Box::new(prompt_step),
        )));
        // AI 响应生成（智能路由 + 故障降级 + JSON 提取 + 工具调用执行）
        // 工具调用管理器：max_iterations / feedback_history_chars 从 config.tools 读取
        let tool_call_manager = Arc::new(
            ToolCallManager::new(tool_system.clone(), ToolUseContext::default())
                .with_max_iterations(config.tools.max_iterations as usize)
                .with_feedback_history_chars(config.tools.feedback_history_chars),
        );
        let tcm_handle: Option<Arc<ToolCallManager>> = Some(tool_call_manager.clone());
        // 创建共享 stream_emitter，由 chat 命令层在流式调用前注入回调
        let stream_emitter = new_shared_stream_emitter();
        // 注入 executor 运行时配置（默认工具超时 + 单工具结果字符预算 + 访问级别）
        // 这些值后续 reinitialize 时也会通过 update_runtime_config 重新注入
        crate::tools::executor::update_runtime_config(
            &crate::tools::executor::ToolRuntimeConfig {
                default_tool_timeout_secs: config.tools.default_tool_timeout_secs,
                max_result_chars: config.tools.max_result_chars as usize,
                access_level: crate::tools::types::AgentAccessLevel::from_str(
                    &config.tools.access_level,
                ),
            },
        );
        steps.add_step(Box::new(TimingMiddleware::new(
            "ai_response_generation",
            Box::new(AIResponseGenerationRunnable::with_tool_call_manager(
                router.clone(),
                tool_call_manager,
                stream_emitter.clone(),
                config.tools.enable_native_function_calling,
                config.tools.max_rounds,
                config.tools.compress_threshold_tokens,
                config.tools.compress_keep_recent,
            )),
        )));
        // 响应解析（提取 text/motion/expression/intent 等）
        // 注入 PersonaEngine 启用回复后处理（客服话术过滤 + 禁忌关键词检测）
        steps.add_step(Box::new(TimingMiddleware::new(
            "response_parsing",
            Box::new(
                ResponseParsingRunnable::new()
                    .with_persona(Arc::clone(&persona)),
            ),
        )));
        // 回复格式验证：空文本检测 + 长度截断 + 空白清理 + 轻量幻觉检测
        steps.add_step(Box::new(TimingMiddleware::new(
            "validation",
            Box::new(crate::pipeline::steps::validation::ValidationRunnable::with_router(
                router.clone(),
            )),
        )));
        // 反思调用（合并表情/动作/贴纸 + 心理状态推断 + 世界状态更新）：
        // 单次 LLM 调用产出全部结构化字段，复用主对话 system_prompt 前缀命中 API 缓存。
        // inline_expression 启用时仅做心理推断（表情/动作已由流式扫描器实时处理）。
        // world_update 字段由 LLM 自主判断是否输出用户持续状态，解析后直接写入 WorldState。
        let mut reflection = ReflectionRunnable::new(
            Some(router.clone()),
            Some(manifest.clone()),
            config.inline_expression.enabled,
            char_id,
        );
        if let Some(ws) = world_state.clone() {
            reflection = reflection.with_world_state(ws);
        }
        reflection = reflection.with_user_goals(mind.user_goals.clone());
        reflection = reflection.with_persona(persona.clone());
        steps.add_step(Box::new(TimingMiddleware::new(
            "reflection",
            Box::new(reflection),
        )));
        steps.add_step(Box::new(TimingMiddleware::new(
            "mood",
            Box::new(MoodStep::new()),
        )));
        // AI 回复 + 长期记忆保存
        steps.add_step(Box::new(TimingMiddleware::new(
            "memory_saving",
            Box::new(MemorySavingRunnable::with_memory(memory.clone())),
        )));

        // Advisor 链：横切关注点（日志/限流/Re2/循环检测）包裹整个 RunnableSequence。
        // 顺序（按 order 升序）：LoggingAdvisor(-100) → RateLimitAdvisor(-50) → Re2Advisor(0) → LoopDetectionAdvisor(100) → steps
        // 注意：违禁词过滤按"自用桌宠"场景剔除。
        let rate_limiter_registry = Arc::new(RateLimiterRegistry::default());
        let advisor_chain = AdvisorChain::new(Arc::new(steps))
            .with_advisor(Arc::new(LoggingAdvisor::new("brain_chat")))
            .with_advisor(Arc::new(RateLimitAdvisor::new(
                rate_limiter_registry,
                "vivian",
            )))
            .with_advisor(Arc::new(Re2Advisor::new()))
            .with_advisor(Arc::new(LoopDetectionAdvisor::new(8, 1)))
            .build();

        Self {
            router,
            memory,
            memory_filter,
            auto_extractor,
            consolidator,
            enable_expiration,
            pipeline,
            time_stamped,
            psychology,
            emotion_bridge,
            dialogue,
            control_action_executor: None,
            tool_call_manager: tcm_handle,
            user_facts,
            hook_judge,
            dynamic_profile,
            advisor_chain,
            stream_emitter,
            world_provider,
            focus_state: Arc::new(tokio::sync::Mutex::new(FocusState::new())),
            char_id: char_id.to_string(),
            language: config.base.language.clone(),
            presence: None,
            self_state: None,
            mind: None,
            fast_semantic: None,
            tool_semantic_filter,
            prompt_step: prompt_step_shared,
            invoke_count: AtomicU64::new(0),
            batch_accumulator: BatchAccumulator::new(3),
            topic_signal_buffer: Arc::new(super::topic_signal::TopicSignalBuffer::new()),
            user_model,
        }
    }

    /// 注入共享的 FocusState（与 Brain 共享同一实例）。
    pub fn with_focus_state(
        mut self,
        focus_state: Arc<tokio::sync::Mutex<FocusState>>,
    ) -> Self {
        self.focus_state = focus_state;
        self
    }

    /// 注入角色 ID（多角色架构下供工具系统路由到对应角色资源）。
    pub fn with_char_id(mut self, char_id: impl Into<String>) -> Self {
        self.char_id = char_id.into();
        self
    }

    /// 注入在场状态管理器（注入后 prompt 注入当前状态供 LLM 感知）。
    pub fn with_presence(
        mut self,
        presence: Arc<crate::presence::PresenceManager>,
    ) -> Self {
        self.presence = Some(presence);
        self
    }

    /// 注入自我状态聚合器（注入后 prompt 注入 SelfState 快照供 LLM 感知）。
    pub fn with_self_state(
        mut self,
        self_state: Arc<crate::self_state::SelfState>,
    ) -> Self {
        self.self_state = Some(self_state);
        self
    }

    /// 注入 Mind 引用（注入后在对话后推入工作记忆条目）。
    pub fn with_mind(mut self, mind: Arc<crate::mind::Mind>) -> Self {
        self.mind = Some(mind);
        self
    }

    /// 注入快速语义分析器（注入后在 prepare_pipeline_state 阶段填充 fast_perception，
    /// 驱动 prompt 动态组装：注入引导文本、调整模块加载策略）。
    pub fn with_fast_semantic(
        mut self,
        fast_semantic: Arc<crate::emotion::FastSemanticAnalyzer>,
    ) -> Self {
        self.fast_semantic = Some(fast_semantic);
        self
    }

    /// 注入 PetController，启用 control_actions 执行能力。
    ///
    /// 在 `new` 之后链式调用，避免 PetController 初始化顺序耦合。
    pub fn with_pet_controller(mut self, pc: Arc<PetController>) -> Self {
        self.control_action_executor = Some(Arc::new(
            ControlActionExecutor::with_pet_controller(pc),
        ));
        self
    }

    /// 设置流式 chunk 推送回调
    ///
    /// 由 `chat:send_message_stream` 命令在调用 `brain.think(stream=true)` 前注入：
    /// - 注入的回调会在 LLM 每产生一个 text 增量时被调用，将 chunk 推送到前端 `chat:chunk` 事件
    /// - 调用结束后应清理为 `None`，避免非流式调用误触发回调
    ///
    /// 内部使用 `Arc<RwLock<...>>` 共享给 `AIResponseGenerationRunnable`，
    /// 因此设置后立即对 pipeline 内部的生成步骤生效。
    pub fn set_stream_emitter(&self, emitter: Option<StreamEmitter>) {
        *self.stream_emitter.write() = emitter;
    }

    /// 构建全链路推理轨迹并写入全局 `TRACE_STORE`（供 Mind Inspector 前端）。
    ///
    /// 错误容忍：所有 JSON 读取均用 Option 链式调用，不会 panic；写入失败只记日志。
    /// 从 `final_state.metadata["timings"]` 提取各 step 耗时，并按 stage 名 enrich
    /// 关键步骤的 details（检索结果数、prompt 分区、工具调用、回复文本等）。
    fn record_reasoning_trace(
        &self,
        user_input: &str,
        final_state: &PipelineState,
        response: &crate::types::response::AiResponse,
    ) {
        use crate::mind::reasoning_trace::{
            truncate_chars, ApiParamInfo, PromptBreakdown, PromptSection, ReasoningStep, ReasoningTrace,
            TRACE_STORE,
        };

        let char_id = self.char_id.clone();
        let mut trace = ReasoningTrace::new(&char_id, user_input);
        trace.session_id = if final_state.conversation_id.is_empty() {
            None
        } else {
            Some(final_state.conversation_id.clone())
        };

        // 预取 enrich 所需的 metadata / memory_vars 快照（避免在循环中反复借用）
        let prompt_length = final_state
            .metadata
            .get("prompt_length")
            .and_then(|v| v.as_u64());
        let memory_count = final_state
            .metadata
            .get("memory_count")
            .and_then(|v| v.as_u64());
        let tool_call_count = final_state
            .metadata
            .get("tool_call_count")
            .and_then(|v| v.as_u64());
        let retrieval_results = final_state
            .memory_vars
            .get("_memory_retrieval_results")
            .and_then(|v| v.as_array())
            .cloned();
        let breakdown_sections = final_state
            .metadata
            .get("prompt_sections_breakdown")
            .and_then(|v| v.as_array())
            .cloned();
        let ai_text = final_state
            .ai_response
            .as_ref()
            .map(|r| r.text.clone())
            .unwrap_or_else(|| final_state.text.clone());
        let tools_used = final_state.tools_used.clone();

        // 从 metadata["timings"] 数组构建各步骤
        if let Some(arr) = final_state.metadata.get("timings").and_then(|v| v.as_array()) {
            for t in arr {
                let stage = t
                    .get("stage")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let ms = t
                    .get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                // 按 stage 名 enrich 输入/输出摘要与 details
                let (input_summary, output_summary, details) = match stage.as_str() {
                    "memory_retrieval" => {
                        let count = retrieval_results.as_ref().map(|a| a.len()).unwrap_or(0);
                        let top3: Vec<String> = retrieval_results
                            .as_ref()
                            .map(|a| {
                                a.iter()
                                    .take(3)
                                    .filter_map(|m| {
                                        m.get("content_snippet")
                                            .and_then(|v| v.as_str())
                                            .map(|s| truncate_chars(s, 60))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        (
                            truncate_chars(&final_state.user_input, 200),
                            format!("命中 {} 条记忆", count),
                            serde_json::json!({
                                "count": count,
                                "memory_count_metadata": memory_count,
                                "top3_previews": top3,
                            }),
                        )
                    }
                    "prompt_building" => {
                        let section_count = breakdown_sections.as_ref().map(|a| a.len()).unwrap_or(0);
                        (
                            format!("{} 个分区待组装", section_count),
                            format!("prompt 长度 {} 字符", prompt_length.unwrap_or(0)),
                            serde_json::json!({
                                "prompt_length": prompt_length,
                                "section_count": section_count,
                            }),
                        )
                    }
                    "ai_response_generation" => {
                        let reply_len = ai_text.chars().count();
                        (
                            format!("prompt {} 字符", prompt_length.unwrap_or(0)),
                            format!("回复 {} 字符", reply_len),
                            serde_json::json!({
                                "reply_char_count": reply_len,
                                "tool_call_count": tool_call_count,
                                "tools_used": tools_used,
                            }),
                        )
                    }
                    "response_parsing" => {
                        (
                            "LLM 原始输出".to_string(),
                            truncate_chars(&ai_text, 200),
                            serde_json::json!({
                                "intent": final_state.intent,
                                "response_mode": final_state.response_mode,
                                "should_respond": final_state.should_respond,
                            }),
                        )
                    }
                    _ => (String::new(), String::new(), serde_json::Value::Null),
                };

                trace.add_step(ReasoningStep {
                    name: stage,
                    input_summary,
                    output_summary,
                    duration_ms: ms,
                    details,
                    success: true,
                    error: None,
                });
            }
        }

        trace.finish(Some(response.text.clone()));
        let trace_char_id = trace.character_id.clone();
        TRACE_STORE.write().add_trace(trace);
        crate::mind::reasoning_trace::emit_trace_added(&trace_char_id);

        // 构造 PromptBreakdown 并写入（按角色索引保留最近一条）
        if let Some(sections_arr) = breakdown_sections {
            let mut sections = Vec::with_capacity(sections_arr.len());
            let mut total = 0usize;
            let mut total_tokens = 0usize;
            for s in sections_arr {
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = s
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let char_count = content.chars().count();
                let token_estimate = s
                    .get("token_estimate")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                total += char_count;
                total_tokens += token_estimate;
                sections.push(PromptSection {
                    name,
                    preview: truncate_chars(&content, 300),
                    full_content: content,
                    char_count,
                    section_id: s
                        .get("section_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    layer: s
                        .get("layer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    token_estimate,
                    optional: s
                        .get("optional")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    present: s
                        .get("present")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                });
            }

            // Post-processing: Expression/Motion/Sticker Selector (独立第二次 LLM 调用)
            {
                let emote_prompt = crate::pipeline::steps::reflection::EXPRESSION_MOTION_SYSTEM_PROMPT;
                let sticker_list = crate::pipeline::steps::reflection::STICKER_LIST;
                let emote_content = format!(
"=== [POST-PROCESSOR] Second LLM call — runs AFTER main reply is generated ===\n\
This is NOT part of the main chat prompt. It is a separate post-processing LLM request\n\
that selects Live2D expression, motion, and sticker based on the conversation.\n\n\
--- System Prompt ---\n{}\n\n\
--- User Message (template, filled at runtime) ---\n\
Available expressions: [dynamically injected from character model manifest]\n\
Available motions: [dynamically injected from character model manifest]\n\
Available stickers: {}\n\n\
User said: [the user's actual message this turn]\n\
Character reply: [the AI's generated reply text]\n\n\
Choose the appropriate expression, motion, and sticker. Leave empty if nothing fits.",
                    emote_prompt, sticker_list
                );
                let emote_char_count = emote_content.chars().count();
                let emote_tokens = crate::utils::token_estimate::estimate_tokens(&emote_content);
                sections.push(PromptSection {
                    name: "Post-processor: Expression & Sticker".to_string(),
                    preview: truncate_chars(&emote_content, 300),
                    full_content: emote_content,
                    char_count: emote_char_count,
                    section_id: "post_expression_sticker".to_string(),
                    layer: "postprocess".to_string(),
                    token_estimate: emote_tokens,
                    optional: true,
                    present: true,
                });
            }

            // 构建非 messages 数组的 API 参数信息（供 Mind Inspector 显示）
            let mut api_params = Vec::new();

            // 1. Native FC tools 参数
            if !final_state.tool_definitions.is_empty() {
                let tool_defs_json = serde_json::to_string_pretty(&final_state.tool_definitions)
                    .unwrap_or_default();
                api_params.push(ApiParamInfo {
                    param_type: "native_tools".into(),
                    label: format!("Native Function Calling: {} tools", final_state.tool_definitions.len()),
                    content: tool_defs_json,
                    present: true,
                });
            }

            // 2. response_format / JSON Schema
            if let Some(schema) = final_state.metadata.get("response_format_schema") {
                if schema != &serde_json::Value::Null {
                    api_params.push(ApiParamInfo {
                        param_type: "response_format".into(),
                        label: "Response Format (JSON Schema)".into(),
                        content: serde_json::to_string_pretty(schema).unwrap_or_default(),
                        present: true,
                    });
                }
            }

            // 3. instructions 字段
            if let Some(inst) = final_state.metadata.get("api_instructions") {
                if let Some(s) = inst.as_str() {
                    if !s.is_empty() {
                        api_params.push(ApiParamInfo {
                            param_type: "instructions".into(),
                            label: "Instructions Field".into(),
                            content: s.to_string(),
                            present: true,
                        });
                    }
                }
            }

            let breakdown = PromptBreakdown {
                character_id: char_id,
                sections,
                total_chars: total,
                total_tokens,
                timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
                scene_modes: Vec::new(),
                api_params,
            };
            TRACE_STORE.write().set_last_prompt(breakdown);
        }
    }

    /// Parse the `[X says to me]` prefix from user_input, returns (text, speaker, listener)
    ///
    /// - With prefix: `[Nana says to me] xxx` → ("xxx", "nana", self.char_id)
    /// - Without prefix: `xxx` → ("xxx", "user", self.char_id)
    ///
    /// speaker is normalized to char_id (lowercase), listener is always the current character.
    fn parse_speaker_prefix(&self, user_input: &str) -> (String, String, String) {
        let (text, speaker) = crate::cross_character::parse_speaker_prefix(user_input);
        (text, speaker, self.char_id.clone())
    }

    /// 初始化 PipelineState：加载对话历史、注入会话回顾/在场状态/SelfState、
    /// 更新凝神模式状态机、刷新工具调用上下文。
    async fn prepare_pipeline_state(&self, user_input: &str) -> PipelineState {
        let mut state = PipelineState::default();
        state.user_input = user_input.to_string();
        // 工作记忆通道隔离：仅隔离 cross_character（两个 AI 角色之间的私聊），
        // 避免其污染用户↔AI 主上下文。用户可见渠道（direct/wechat/proactive）
        // 视为同一对话，切换入口时上下文连续。
        let current_ch = self.dialogue.get_channel();
        state.messages = if current_ch == "cross_character" {
            self.dialogue.get_history_filtered_by_channel(Some("cross_character"))
        } else {
            self.dialogue.get_user_visible_history()
        };
        // 会话回顾注入：从 TimeStampedMemory 摘要构造 recap 消息，插入消息列表头部
        {
            let tsm = self.time_stamped.read();
            crate::memory::session_compressor::inject_recap_if_available(
                &mut state.messages,
                &tsm,
            );
        }
        state.current_channel = current_ch;
        if let Some(presence) = &self.presence {
            state.presence_state = presence.current().as_str().to_string();
        }
        if let Some(self_state) = &self.self_state {
            state.self_state_text = self_state.snapshot().serialize_for_prompt(&self.language);
        }

        // 用户认知模型注入：将 UserModel 格式化为"我对你的了解"段落
        // 在 prompt 中注入，让 LLM 感知"我对这个人的长期认识"
        state.user_model_text = self
            .user_model
            .format_for_prompt(&self.language)
            .unwrap_or_default();

        // 凝神模式状态机更新：本轮输入 + 当前用户情绪 → 评分 → 三态切换
        {
            let emotion = self.emotion_bridge.get_current_emotion().emotion;
            let score = compute_focus_score(user_input, &emotion);
            let topic_changed = self.topic_signal_buffer.detect_topic_change(&self.char_id, user_input);
            let now = chrono::Local::now().timestamp() as f64;
            let th = FocusThresholds::default();
            let mut fs = self.focus_state.lock().await;
            let decision = fs.update(score, topic_changed, true, now, &th);
            if fs.is_focus() {
                state.focus_active = true;
                state.focus_extra_tokens = th.thinking_extra_tokens;
            }
            if decision.action == crate::brain::focus_mode::FocusAction::Enter {
                tracing::info!(
                    "[BrainChatChain] 凝神模式激活（charge={:.3}），本轮 max_tokens 额外余量={}",
                    decision.charge,
                    th.thinking_extra_tokens
                );
            } else if decision.action == crate::brain::focus_mode::FocusAction::Exit {
                tracing::info!(
                    "[BrainChatChain] 凝神模式退出（reason={:?}）",
                    decision.reason
                );
            }
        }

        // 工具调用上下文刷新：让工具感知当前情绪 / 关系阶段 / 最近记忆摘要
        if let Some(tcm) = &self.tool_call_manager {
            let emotion = self.emotion_bridge.get_current_emotion().emotion;
            let stage = self.psychology.get_stage().as_str().to_string();
            let memory_summary: String = self
                .time_stamped
                .read()
                .recent_summary()
                .chars()
                .take(200)
                .collect();
            let ctx = ToolUseContext::default()
                .with_emotion(emotion)
                .with_intimacy_stage(stage)
                .with_memory_summary(memory_summary)
                .with_char_id(&self.char_id)
                .with_user_message(user_input);
            tcm.update_context(ctx);
        }

        // 快速语义感知已迁移至 FastSemanticStep，与 QueryRewriteStep 并行执行
        // （见 BrainChatChain::new 中 ParallelStep 组装逻辑）
        state
    }

    /// 执行流水线并构造 AiResponse。
    ///
    /// 包含：stream 配置 → 8 维情绪向量转温度覆盖 → 调用 advisor_chain →
    /// 清除温度覆盖 → 反序列化 PipelineState → 构造 AiResponse（含兜底）→
    /// 记录推理轨迹。
    async fn execute_pipeline_and_build_response(
        &self,
        state: PipelineState,
        stream: bool,
        user_input: &str,
    ) -> VivianResult<(PipelineState, AiResponse)> {
        let config = if stream {
            let mut c = RunnableConfig::default();
            c.tags.push("stream".to_string());
            Some(c)
        } else {
            None
        };

        // 8 维情绪向量 → temperature 覆盖：让 LLM 输出温度随当前情绪变化
        let emotion_state = self.psychology.emotion();
        let trust = self.psychology.relationship().trust;
        let temp = emotion_state.to_8d_vector(trust).to_temperature_default();
        self.router.set_temperature_override(Some(temp));

        let result = self.advisor_chain.ainvoke(state.to_json(), config).await?;

        // 清除温度覆盖，避免影响后续非对话路径的 LLM 调用
        self.router.set_temperature_override(None);

        let final_state = PipelineState::from_json(result);

        let mut response = match final_state.ai_response.clone() {
            Some(response) => response,
            None => AiResponse::new(final_state.text.clone()),
        };
        response.user_emotion = final_state.user_emotion.clone();
        response.expression = final_state.expression.clone();
        response.expression_duration_ms = final_state.expression_duration_ms;
        response.motion = final_state.motion.clone();
        response.sticker = final_state.sticker.clone();
        // text 字段仅保留纯文本，剥离 Markdown / 富文本渲染语法，以及 TTS 控制标记
        // （[EMO]/[THINKING]/[SPEED]/[PAUSE] 是 TTS 指令，不应显示在聊天气泡里）
        let tts_ctrl = crate::speech::tts::parse_tts_controls(&response.text);
        response.text = crate::utils::strip_markdown_syntax(&tts_ctrl.text);

        self.record_reasoning_trace(user_input, &final_state, &response);

        Ok((final_state, response))
    }

    /// 调用对话链 —— 执行步骤流水线 + 后处理记忆操作。
    ///
    /// 后处理（记忆写回逻辑）：
    /// 1. 将用户消息与 AI 回复写入 TimeStampedMemory 容器；超过 40 条触发摘要
    /// 2. 调用 AutoExtractor 从本轮对话中抽取长期记忆（ADD/UPDATE/DELETE）
    /// 3. 每 `CONSOLIDATOR_INTERVAL` 次调用一次 MemoryRetentionGuard 清理过期记忆
    pub async fn ainvoke(&self, user_input: &str, stream: bool) -> VivianResult<AiResponse> {
        self.ainvoke_with_options(user_input, stream, false).await
    }

    /// 带选项的对话链调用。
    ///
    /// `skip_dialogue_write`: 跳过将本轮用户消息和 AI 回复写入对话历史。
    /// 用于插话等场景——插话指令是内部生成的系统提示，不应作为用户消息出现在对话历史和记忆图谱中。
    pub async fn ainvoke_with_options(
        &self,
        user_input: &str,
        stream: bool,
        skip_dialogue_write: bool,
    ) -> VivianResult<AiResponse> {
        let state = self.prepare_pipeline_state(user_input).await;

        let (final_state, response) = self
            .execute_pipeline_and_build_response(state, stream, user_input)
            .await?;

        // ── Working Memory：推入本轮对话摘要 ──
        // 蒸馏为 ≤80 字短摘要，让 LLM 下一轮感知"最近几轮在聊什么"。
        // 跨角色对话也推入，但用不同 source 标签（AiReply / UserMessage）。
        if let Some(mind) = &self.mind {
            let user_summary: String = truncate_chars(user_input, 80);
            mind.push_working_memory(
                user_summary,
                crate::mind::working_memory::WorkingMemorySource::UserMessage,
            );
            let ai_summary: String = truncate_chars(&final_state.text, 80);
            mind.push_working_memory(
                ai_summary,
                crate::mind::working_memory::WorkingMemorySource::AiReply,
            );
            // 对话结束后触发 current_thought 刷新（下次 cognitive tick 用 LLM 重新合成）
            mind.request_thought_refresh();
        }

        // ── 话题信号慢存储：记录本轮 topics，稳定后写入记忆 ──
        if let Some(ref perception) = final_state.fast_perception {
            let topic_labels: Vec<String> = perception
                .topics
                .iter()
                .map(|t| t.label.clone())
                .collect();
            if !topic_labels.is_empty() {
                // 更新用户认知模型中的项目激活度（基于当前话题信号）
                self.user_model.update_project_activations(&topic_labels);

                // 共现式关联构建：把当前话题关联到最近更新的特征，
                // 让特征逐渐积累"用户常把它和哪些话题一起讨论"，供多跳检索使用。
                // 窗口 10 分钟：仅当某特征在本轮附近被强化过才建立关联，避免无关话题硬塞。
                self.user_model
                    .associate_active_traits_with_topics(&topic_labels, 600.0);

                self.topic_signal_buffer
                    .record_topics(&self.char_id, topic_labels.clone());
                if let Some(topics_to_flush) = self.topic_signal_buffer.should_flush(&self.char_id) {
                    let memory = self.memory.clone();
                    let char_id = self.char_id.clone();
                    tokio::spawn(async move {
                        let text = format!("近期话题：{}", topics_to_flush.join("、"));
                        let meta = serde_json::json!({
                            "topic_signal": true,
                            "topics": topics_to_flush,
                        });
                        let _ = memory
                            .add_memory_with_metadata(
                                &text,
                                crate::memory::types::MemoryType::ShortTerm,
                                0.3,
                                vec!["topic_signal".to_string()],
                                meta,
                            )
                            .await;
                        tracing::debug!(
                            "[TopicSignal:{}] 话题信号慢存储写入：{}",
                            char_id,
                            text
                        );
                    });
                }
            }
        }

        // ── 补充回复调度：fast 检索已完成，异步触发 slow 检索 + 补充回复 ──
        // 仅在正常对话回复（非命令、非静默）时调度，fire-and-forget 不阻塞主路径。
        if !final_state.is_command && final_state.should_respond {
            if let Some(svc) = crate::brain::augment_reply_service::try_get_augment_reply_service() {
                // 从 raw_semantic_memory 反序列化为 MemoryEntry 列表
                let fast_memories: Vec<crate::brain::augment_reply_service::MemoryEntry> = final_state
                    .raw_semantic_memory
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                let _ = svc.schedule(
                    user_input,
                    &final_state.text,
                    &final_state.motion,
                    &final_state.expression,
                    &fast_memories,
                    None,
                    &self.char_id,
                );
            }
        }

        // ── 心理架构更新 + 关系更新：基于 LLM 产出的 appraisal/emotion_update/behavior_drive ──
        // 这是「事件 → Appraisal → Emotion → Behavior Drive」因果链的落地。
        // 通过 apply_turn_boundary 原子化写入：LLM 心理状态 → 关系更新 → 交互统计。
        if !final_state.is_command && final_state.should_respond {
            // 提取用户情绪标签（供 turn boundary 和 event_summary 记忆写入共用）
            let user_emo = final_state.user_emotion.trim().to_lowercase();
            let intensity = if final_state.user_emotion_intensity > 0.0 {
                final_state.user_emotion_intensity
            } else if user_emo != "neutral" && !user_emo.is_empty() {
                0.5
            } else {
                0.0
            };
            let sentiment = match user_emo.as_str() {
                "happy" | "joy" | "excited" => "happy",
                "sad" | "angry" | "anxious" | "frustrated" => user_emo.as_str(),
                _ => "neutral",
            };

            let psy_output = PsychologyOutput {
                appraisal: final_state.appraisal.clone(),
                emotion_update: final_state.emotion_update.clone(),
                behavior_drive: final_state.behavior_drive.clone(),
                need_update: None,
            };
            // 统一 turn boundary：LLM 心理状态 → 5 维关系更新 → 交互统计（原子操作）
            self.psychology
                .apply_turn_boundary(&psy_output, sentiment, intensity);

            // LLM 自判事件摘要：非空时写入记忆系统 ImportantEvent（持久化、可被检索/巩固）
            // event_summary 由 LLM 在主调用 JSON 中产出，避免每轮都记录低信息事件
            let event_summary = final_state.event_summary.trim();
            if !event_summary.is_empty() {
                let significance = final_state
                    .appraisal
                    .as_ref()
                    .map(|a| a.significance)
                    .unwrap_or(0.5);
                let user_emo_tag = format!("user_emotion:{}", final_state.user_emotion);
                let tags = vec![
                    "important_event".to_string(),
                    "interaction".to_string(),
                    user_emo_tag,
                    "assistant".to_string(),
                ];
                let memory = self.memory.clone();
                let summary_owned = event_summary.to_string();
                let router_for_router = self.router.clone();
                let char_id_for_router = self.char_id.clone();
                let user_emotion_for_router = final_state.user_emotion.clone();
                let channel_for_router = self.dialogue.get_channel();
                tokio::spawn(async move {
                    // 为 ImportantEvent 补全跨角色上下文 metadata，确保事件账本注册字段正确
                    let event_metadata = serde_json::json!({
                        "channel": channel_for_router,
                        "speaker": "user",
                        "listener": char_id_for_router,
                        "perspective": "speaker",
                        "knowledge_source": "direct",
                    });
                    if let Err(e) = memory
                        .add_memory_enriched_with_metadata(
                            &summary_owned,
                            MemoryType::ImportantEvent,
                            significance,
                            tags,
                            Some(event_metadata),
                            None,
                        )
                        .await
                    {
                        tracing::warn!("[BrainChatChain] ImportantEvent 写入失败: {}", e);
                    }

                    // Memory Router LLM 仲裁：对候选条目二次判定是否应升级到共享世界层
                    // 同步路由已由 manager.rs 内部的 route_to_shared_world 处理，
                    // 此处补 LLM 仲裁覆盖同步规则漏判的边界场景（如"用户提到家规但没用持久性词"）。
                    use crate::memory::memory_router::{
                        route_with_llm, MemoryDestination, RouteContext,
                    };
                    let ctx = RouteContext {
                        content: &summary_owned,
                        importance: significance,
                        channel: &channel_for_router,
                        speaker: "user",
                        listener: &char_id_for_router,
                        perspective: "speaker",
                        char_id: &char_id_for_router,
                    };
                    let dest = route_with_llm(&ctx, router_for_router.as_ref()).await;
                    if dest == MemoryDestination::SharedWorld {
                        use crate::memory::world_knowledge::{world_knowledge, WorldFact};
                        let engine = world_knowledge();
                        let now = chrono::Utc::now().timestamp() as f64;
                        let category = crate::memory::manager::infer_world_fact_category(&summary_owned);
                        if engine.find_similar(&summary_owned, category).is_none() {
                            let fact = WorldFact {
                                id: format!("wf-llm-{}-{}", now as u64, rand::random::<u32>()),
                                fact_text: summary_owned.clone(),
                                category,
                                importance: significance,
                                contributors: vec![char_id_for_router.clone()],
                                source_event_ids: vec![format!("llm-router-{}", now as u64)],
                                created_at: now,
                                last_reinforced_at: now,
                                reinforcement_count: 0,
                            };
                            if let Err(e) = engine.append_fact(fact) {
                                tracing::debug!("[MemoryRouter] LLM 仲裁写入共享世界失败: {}", e);
                            }
                        }
                    }
                    let _ = user_emotion_for_router;
                });
            }
        }

        // ── 后处理：桌宠自控动作（control_actions）──
        // control_actions 由反思调用产出，在此分发到 PetController。
        // best-effort：单条失败不影响主流程；executor 未注入时跳过。
        if !final_state.control_actions.is_empty() {
            if let Some(executor) = &self.control_action_executor {
                executor.execute(&final_state.control_actions);
            } else {
                tracing::debug!(
                    "[BrainChatChain] 收到 {} 条 control_actions，但 PetController 未注入，跳过",
                    final_state.control_actions.len()
                );
            }
        }

        // ── 后处理：Memory 子系统写回（fire-and-forget，不阻塞响应）──
        // 命令命中或无需响应时跳过记忆写回
        // 记忆后处理（TimeStampedMemory + AutoExtractor + Consolidator）改为异步 spawn，
        // 让用户立即收到响应，记忆巩固在后台进行。
        if !final_state.is_command && final_state.should_respond {
            let memory = self.memory.clone();
            let consolidator = self.consolidator.clone();
            let enable_expiration = self.enable_expiration;
            let pipeline = self.pipeline.clone();
            let time_stamped = self.time_stamped.clone();
            let router = self.router.clone();
            let user_facts = self.user_facts.clone();
            let hook_judge = self.hook_judge.clone();
            let dynamic_profile = self.dynamic_profile.clone();
            let user_model = self.user_model.clone();
            let user_emotion = final_state.user_emotion.clone();
            let ai_emotion = self.emotion_bridge.get_current_emotion().emotion;
            // 记忆系统存原文（剥离 [X 对你说] 前缀），LLM 可见的前缀已在 generation 层处理
            let (raw_input_for_memory, speaker, _) = self.parse_speaker_prefix(user_input);
            let is_cross_character = speaker != "user";
            let user_input_owned = raw_input_for_memory;
            let response_clone = response.clone();
            let count = self.invoke_count.fetch_add(1, Ordering::Relaxed) + 1;

            tokio::spawn(async move {
                Self::post_process_memory_async(
                    memory,
                    consolidator,
                    enable_expiration,
                    pipeline,
                    time_stamped,
                    router,
                    user_facts,
                    hook_judge,
                    dynamic_profile,
                    user_model,
                    &user_input_owned,
                    &response_clone,
                    &user_emotion,
                    &ai_emotion,
                    count,
                    is_cross_character,
                )
                .await;
            });
        }

        // ── 后处理：工具调用与记忆联动 ──
        // 将执行过的工具调用记录为记忆，
        // 让 Vivian 能在后续对话中回忆"我曾为你做过什么"。
        if !final_state.is_command
            && final_state.tool_call_executed
            && !final_state.tool_calls.is_empty()
        {
            let memory = self.memory.clone();
            let tool_calls = final_state.tool_calls.clone();
            // 工具执行时刻在生成步骤中捕获；用该时刻回写工具记忆时间戳，
            // 保证时间线上"执行工具"排在"回复"之前（与实际发生顺序一致）
            let tool_executed_at = final_state
                .metadata
                .get("tool_executed_at")
                .and_then(|v| v.as_f64());
            tokio::spawn(async move {
                Self::record_tool_memories_async(
                    memory,
                    &tool_calls,
                    tool_executed_at,
                )
                .await;
            });
        }

        // ── 后处理：将当前轮次写入对话管理器，确保下一次调用时历史会累积。
        // skip_dialogue_write 时跳过：插话等内部指令不应作为用户消息出现在对话历史和记忆图谱中。
        if final_state.should_respond && !skip_dialogue_write {
            let clean_ai = MemorySavingRunnable::strip_json_if_any(&response.text);
            let channel = self.dialogue.get_channel();
            // 解析 [X 对你说] 前缀：剥离前缀存原文，用 speaker/listener 元数据标注来源
            let (raw_input, speaker, listener) = self.parse_speaker_prefix(user_input);
            let is_cross_character_input = speaker != "user";
            // 用户消息元数据：完整标注 channel/speaker/listener/perspective/knowledge_source
            let knowledge_source = if is_cross_character_input {
                "heard"
            } else {
                "direct"
            };
            let user_metadata = serde_json::json!({
                "channel": channel,
                "speaker": speaker,
                "listener": listener,
                "perspective": "speaker",
                "knowledge_source": knowledge_source,
            });
            let mut user_msg = ChatMessage::user(&raw_input);
            user_msg.meta = Some(
                crate::messages::MessageMeta::user().with_channel(&channel),
            );
            // AI 回复元数据：与用户消息对称标注，并持久化贴纸
            // 前端刷新历史时从 metadata.sticker 重建独立的表情包气泡，避免刷新后丢失
            let ai_metadata = serde_json::json!({
                "channel": channel,
                "speaker": listener,
                "listener": speaker,
                "perspective": "speaker",
                "knowledge_source": if is_cross_character_input { "heard" } else { "direct" },
                "sticker": response.sticker,
            });
            let mut ai_msg = ChatMessage::assistant(&clean_ai);
            ai_msg.meta = Some(
                crate::messages::MessageMeta::assistant().with_channel(&channel),
            );
            self.dialogue.add_message_with_metadata(user_msg, user_metadata);
            self.dialogue.add_message_with_metadata(ai_msg, ai_metadata);

            // 工具失败容错：将失败信息加入对话历史，让 LLM 下一轮能感知并自然补救
            // 跨角色对话场景下跳过，避免系统提示污染室友对话上下文
            // 时效约束：仅当本轮最后一次工具调用也失败时才注入，
            // 避免重试已成功的情况下还残留失败提示导致 LLM 自相矛盾
            if !is_cross_character_input {
                let last_tool_failed = final_state
                    .tool_calls
                    .last()
                    .and_then(|c| c.get("success").and_then(serde_json::Value::as_bool))
                    .map(|s| !s)
                    .unwrap_or(false);
                if last_tool_failed {
                    if let Some(failures) = final_state
                        .metadata
                        .get("tool_failures")
                        .and_then(serde_json::Value::as_array)
                    {
                        if !failures.is_empty() {
                            let failed_tools: Vec<&str> = failures
                                .iter()
                                .filter_map(|f| f.get("tool").and_then(serde_json::Value::as_str))
                                .collect();
                            if !failed_tools.is_empty() {
                                let sys_note = format!(
                                    "[系统提示] 工具{}执行失败，下一轮回复中请自然地告知用户并尝试替代方案。",
                                    failed_tools.join("、")
                                );
                                self.dialogue.add_message(ChatMessage::system(sys_note));
                            }
                        }
                    }
                }
            }
        }

        // ── 批量累积后处理：累积 3 轮对话后一次性触发 AutoExtractor 和心理学分析 ──
        // 将本轮对话消息累积到 BatchAccumulator，达到 3 轮阈值时获取全部累积消息，
        // 在 spawn 中一次性调用 AutoExtractor 进行处理，减少 LLM 调用次数。
        // 同时将累积的对话传递到 post_process_memory_async 中做批量抽取。
        // 注意：此处的 batch 数据仅用于 AutoExtractor 批量分析，
        // TimeStampedMemory 写入、动态行为画像等每轮单独执行的操作不受影响。
        if !final_state.is_command && final_state.should_respond {
            let (raw_input, _, _) = self.parse_speaker_prefix(user_input);
            let clean_ai = MemorySavingRunnable::strip_json_if_any(&response.text);
            let user_msg = ChatMessage::user(&raw_input);
            let ai_msg = ChatMessage::assistant(&clean_ai);
            
            if let Some(batch) = self.batch_accumulator.record_turn(&user_msg, &ai_msg) {
                // 达到阈值，在后台一次性批量处理所有累积的对话
                let auto_extractor = self.auto_extractor.clone();
                tokio::spawn(async move {
                    // 一次性调用 AutoExtractor 分析全部 3 轮对话
                    // AutoExtractor 内部有并发控制（extraction_in_progress），
                    // 且支持接收多轮对话作为输入 -> extract_memories(&batch)
                    let extracted = auto_extractor.extract_memories(&batch, None).await;
                    if !extracted.is_empty() {
                        tracing::info!(
                            "[BrainChatChain][Batch] AutoExtractor 从累积的 {} 轮对话中批量抽取 {} 条长期记忆",
                            batch.len() / 2,
                            extracted.len()
                        );
                    }
                });
            }
        }

        // 在场状态切换已迁移至 set_presence_state 工具（presence_tools.rs），
        // LLM 通过工具系统调用，不再走 JSON presence_change 字段路径。

        Ok(response)
    }

    /// 启动问候专用：走完整对话流水线生成，但跳过记忆写入与对话写回。
    ///
    /// 与一般直接渠道对话复用同一套完整提示词（含记忆检索 → 种子记忆进入 prompt），
    /// 同时设置 `skip_memory_save`，让 UserMemorySavingRunnable / MemorySavingRunnable
    /// 跳过写入——问候的对话历史与记忆由调用方（Brain::generate_startup_greeting）
    /// 独立完成后处理，避免把合成的问候指令当作用户消息污染记忆库。
    pub async fn ainvoke_greeting(&self, user_input: &str) -> VivianResult<AiResponse> {
        let mut state = self.prepare_pipeline_state(user_input).await;
        state.metadata["skip_memory_save"] = serde_json::json!(true);
        let (_, response) = self
            .execute_pipeline_and_build_response(state, false, user_input)
            .await?;
        Ok(response)
    }

    /// 后处理：将对话写入 TimeStampedMemory + 触发 AutoExtractor + 定期 RetentionGuard + 巩固流水线。
    ///
    /// **fire-and-forget 模式**：此函数设计为在 `tokio::spawn` 中调用，不阻塞主响应路径。
    /// 所有操作均为"尽力而为"：失败仅记录日志，不影响主路径。
    async fn post_process_memory_async(
        memory: Arc<MemoryManager>,
        consolidator: Arc<MemoryRetentionGuard>,
        enable_expiration: bool,
        pipeline: Arc<ConsolidationPipeline>,
        time_stamped: Arc<RwLock<TimeStampedMemory>>,
        router: Arc<ModelRouter>,
        user_facts: Arc<UserFactStore>,
        hook_judge: Arc<HookJudge>,
        dynamic_profile: Arc<DynamicBehaviorProfile>,
        user_model: Arc<UserModelManager>,
        user_input: &str,
        response: &AiResponse,
        user_emotion: &str,
        ai_emotion: &str,
        invoke_count: u64,
        is_cross_character: bool,
    ) {
        // 1. 写入 TimeStampedMemory 容器（40 阈值摘要，保留最近 8 条）
        let pending_summary: Option<Vec<ChatMessage>> = {
            let mut tsm = time_stamped.write();
            tsm.add_message(ChatMessage::user(user_input));
            // 防御性清理：避免原始 JSON 串写入记忆
            let clean_ai = MemorySavingRunnable::strip_json_if_any(&response.text);
            tsm.add_message(ChatMessage::assistant(&clean_ai));
            // 超过阈值：锁内切割出待压缩消息，锁外调用 LLM
            if tsm.should_summarize() {
                Some(tsm.summarize())
            } else {
                None
            }
        };

        // 锁外执行 LLM 窗口压缩（避免持有 RwLock 期间 await，导致 future 不是 Send）
        if let Some(removed) = pending_summary {
            if !removed.is_empty() {
                let summary = TimeStampedMemory::compress_with_llm(&router, &removed).await;
                let removed_count = removed.len();
                let retained_count = {
                    let mut tsm = time_stamped.write();
                    tsm.commit_summary(summary, &removed);
                    tsm.get_messages().len()
                };
                tracing::info!(
                    "[BrainChatChain] TimeStampedMemory 触发摘要：移除 {} 条，保留 {} 条",
                    removed_count,
                    retained_count
                );
            }
        }

        // 2. 此处的单轮 AutoExtractor 调用已由主流程中的 batch_accumulator 替代
        //    当 batch_accumulator 未达到阈值时，本轮不会触发 AutoExtractor；
        //    达到阈值时由 batch 路径一次性处理全部累积的对话。
        //    跨角色对话场景仍然跳过 AutoExtractor。
        if !is_cross_character {
            // 这里不再每轮都调用 extract_memories，交由 batch_accumulator 控制
            tracing::debug!(
                "[BrainChatChain] AutoExtractor 已迁移到 batch_accumulator 调度，跳过单轮提取"
            );
        }

        // 2.5 用户事实画像：从对话中提取 L0 身份 + L0.5 偏好 + L2 自由事实
        // 走 memory 路由（高频低复杂度），智能合并（旧值优先，冲突时 LLM 仲裁）
        // source_memory_id 传 None：此 fire-and-forget 路径无法获取 MemoryItem ID，
        // 溯源能力由 extract_and_upsert 内部 timestamp 提供
        let clean_ai_text = MemorySavingRunnable::strip_json_if_any(&response.text);
        match user_facts.extract_and_upsert(user_input, &clean_ai_text, None).await {
            Ok(facts) if !facts.is_empty() => {
                tracing::info!(
                    "[BrainChatChain] UserFactStore 提取并更新 {} 条用户事实",
                    facts.len()
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[BrainChatChain] UserFactStore 提取失败: {}", e);
            }
        }

        // 2.5.5 用户认知模型：强证据检测与弱证据积累
        // 在非跨角色对话时，从用户输入中检测偏好/工作方式/兴趣/目标等信号
        // 强证据直接更新 UserTrait，弱证据进入候选池等待批量归纳
        if !is_cross_character {
            use crate::memory::user_model::{detect_strong_evidence, detect_weak_evidence};

            // 强证据检测
            let strong_evidence = detect_strong_evidence(user_input);
            for ev in &strong_evidence {
                if ev.is_contradiction {
                    user_model.apply_contradicting_evidence(&ev.key, "realtime", ev.strength);
                } else {
                    user_model.apply_strong_evidence(
                        ev.category,
                        &ev.key,
                        &ev.value,
                        &ev.scope,
                        "realtime",
                        ev.strength,
                    );
                }
            }
            if !strong_evidence.is_empty() {
                tracing::debug!(
                    "[BrainChatChain] 用户认知模型：检测到 {} 条强证据",
                    strong_evidence.len()
                );
            }

            // 弱证据检测（进入候选池）
            let weak_evidence = detect_weak_evidence(user_input);
            for ev in &weak_evidence {
                user_model.add_candidate_evidence(
                    ev.category,
                    &ev.key,
                    &ev.value,
                    "realtime",
                    ev.strength,
                );
            }
            if !weak_evidence.is_empty() {
                tracing::debug!(
                    "[BrainChatChain] 用户认知模型：检测到 {} 条弱证据（进入候选池）",
                    weak_evidence.len()
                );
            }
        }

        // 2.6 Open Hooks 闭环判定：用本轮对话检查所有未闭环钩子
        // 走 memory 路由，LLM 判断是否满足闭环条件；失败仅 warn 不阻塞
        let recent_dialog = format!("用户：{}\n薇薇安：{}", user_input, clean_ai_text);
        match hook_judge.judge_and_close(&memory, &recent_dialog).await {
            Ok(closed) if closed > 0 => {
                tracing::info!(
                    "[BrainChatChain] HookJudge 本轮闭环 {} 个 open_hooks",
                    closed
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[BrainChatChain] HookJudge 判定失败: {}", e);
            }
        }

        // 2.7 动态行为画像：记录本轮对话（写路径，用于后续 prompt 注入）
        // 跟踪近期话题/情绪/消息长度，作为 base persona 的动态补充
        dynamic_profile.record_turn(user_input, &clean_ai_text, user_emotion, ai_emotion);

        // 3. 每 CONSOLIDATOR_INTERVAL 次调用一次 MemoryRetentionGuard 清理过期记忆
        if invoke_count % CONSOLIDATOR_INTERVAL == 0 {
            match consolidator.cleanup_expired(&memory, enable_expiration).await {
                Ok(deleted) => {
                    tracing::info!(
                        "[BrainChatChain] MemoryRetentionGuard 清理过期记忆 {} 条",
                        deleted
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[BrainChatChain] MemoryRetentionGuard 过期清理失败: {}",
                        e
                    );
                }
            }
            // 去重合并
            if let Err(e) = consolidator.consolidate(&memory).await {
                tracing::warn!("[BrainChatChain] MemoryRetentionGuard 去重合并失败: {}", e);
            }
            // 同步清理回收站中超过 7 天保留期的条目
            let purged = memory.purge_expired_recycle_bin();
            if purged > 0 {
                tracing::info!(
                    "[BrainChatChain] 回收站过期清理 {} 条",
                    purged
                );
            }
        }

        // 4. 巩固流水线：ShortTerm → MidTerm SessionSummary → LongTerm → Insight
        // 三阶段触发器，所有 LLM 调用走 reflection 路由
        match pipeline.run(&memory).await {
            Ok(report) => {
                if report.stage1_summaries > 0
                    || report.stage2_facts > 0
                    || report.stage3_insights > 0
                {
                    tracing::info!(
                        "[BrainChatChain] ConsolidationPipeline 完成：stage1_summaries={}, stage2_facts={}, stage3_insights={}, stage2_acquired_behaviors={}, stage2_relationship_signals={}",
                        report.stage1_summaries,
                        report.stage2_facts,
                        report.stage3_insights,
                        report.stage2_acquired_behaviors.len(),
                        report.stage2_relationship_signals.len()
                    );
                }
                // Stage 2 第四路抽取的语义级行为画像合并到 DynamicBehaviorProfile
                // （由 pipeline 返回，BrainChatChain 负责持久化到 dynamic_profile.json）
                if !report.stage2_acquired_behaviors.is_empty() {
                    dynamic_profile.merge_acquired_behaviors(report.stage2_acquired_behaviors);
                }
                // Stage 2 第五路抽取的关系信号写入关系日志
                if !report.stage2_relationship_signals.is_empty() {
                    let log = crate::psychology::relationship_log();
                    let now = crate::memory::types::current_timestamp();
                    let date = crate::psychology::date_str_from_ts(now);
                    for (idx, signal) in report.stage2_relationship_signals.iter().enumerate() {
                        let entry = crate::psychology::RelationshipLogEntry {
                            id: format!("sig_{}_{}", now as u64, idx),
                            date: date.clone(),
                            created_at: now,
                            user_mood: signal.user_mood.clone(),
                            relationship_signal: signal.relationship_signal.clone(),
                            important_moment: signal.important_moment.clone(),
                            next_care_cue: signal.next_care_cue.clone(),
                            direction: crate::psychology::RelationshipDirection::UserAgent,
                            target_agent_id: None,
                        };
                        if let Err(e) = log.append_entry(entry) {
                            tracing::warn!("[BrainChatChain] 关系日志写入失败: {}", e);
                        }
                    }
                    // 尝试生成昨日摘要
                    let yesterday = crate::psychology::yesterday_date_str();
                    if let Some(summary) = log.try_generate_daily_summary(&yesterday) {
                        if let Err(e) = log.upsert_daily_summary(summary) {
                            tracing::warn!("[BrainChatChain] 昨日关系摘要生成失败: {}", e);
                        }
                    }
                }
                // Stage 2 第六路抽取的 L1 近期状态更新到 UserFactStore
                if let Some(l1) = report.stage2_recent_state {
                    // 从 L1 近期状态自动注册项目到用户认知模型
                    for project_desc in &l1.current_projects {
                        if !project_desc.trim().is_empty() {
                            // 使用项目描述的前 20 字作为项目名
                            let project_name: String = project_desc.chars().take(20).collect();
                            user_model.upsert_project(
                                &project_name,
                                project_desc,
                                Vec::new(),
                                "active",
                                crate::memory::user_model::ProjectStatus::Active,
                            );
                        }
                    }
                    if let Err(e) = user_facts.update_recent_state(
                        l1.recent_goals,
                        l1.current_projects,
                        l1.recent_preferences,
                    ) {
                        tracing::warn!("[BrainChatChain] L1 近期状态更新失败: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[BrainChatChain] ConsolidationPipeline 执行失败: {}", e);
            }
        }

        // 5. 同主题记忆整合：在巩固流水线完成后，对 LongTerm/Insight 等同主题节点做语义合并
        if invoke_count % (CONSOLIDATOR_INTERVAL * 4) == 0 {
            let merger = crate::memory::topic_merger::TopicMerger::new(Arc::clone(&router));
            match merger.run(&memory).await {
                Ok(report) if report.clusters_merged > 0 => {
                    tracing::info!(
                        "[BrainChatChain] TopicMerger 合并 {} 簇（删除 {} 条，新增 {} 条）",
                        report.clusters_merged,
                        report.memories_removed,
                        report.memories_created
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("[BrainChatChain] TopicMerger 执行失败: {}", e);
                }
            }
        }
    }

    /// 将工具调用记录为记忆。
    ///
    /// 每个执行过的工具调用会生成一条 `General` 记忆，内容包含工具名、
    /// 简要参数。这让 Vivian 在后续对话中能回忆起"我曾为你做过什么"。
    async fn record_tool_memories_async(
        memory: Arc<MemoryManager>,
        tool_calls: &[serde_json::Value],
        tool_executed_at: Option<f64>,
    ) {
        for tc in tool_calls {
            let tool_name = tc
                .get("tool")
                .or_else(|| tc.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");

            // 跳过元工具（tool_list 只是列出可用工具，无实际语义）
            if tool_name == "tool_list" {
                continue;
            }

            let args = tc
                .get("arguments")
                .or_else(|| tc.get("args"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let brief_args = summarize_tool_args(&args);
            let content = if brief_args.is_empty() {
                format!("执行工具「{}」", tool_name)
            } else {
                format!("执行工具「{}」（{}）", tool_name, brief_args)
            };

            let tags = vec![
                "tool_call".to_string(),
                tool_name.to_string(),
            ];

            // 为工具调用记忆补全 metadata，确保事件账本注册字段正确
            let tool_metadata = serde_json::json!({
                "channel": "direct",
                "speaker": memory.char_id(),
                "listener": "user",
                "perspective": "speaker",
                "knowledge_source": "direct",
            });
            if let Err(e) = memory
                .add_memory_enriched_with_metadata(
                    &content,
                    MemoryType::General,
                    0.4,
                    tags,
                    Some(tool_metadata),
                    tool_executed_at,
                )
                .await
            {
                tracing::warn!("[BrainChatChain] 工具记忆写入失败: {}", e);
            }
        }
    }
}

/// 简要汇总工具参数（最多 3 个键值对，每个值截断 50 字符）
fn summarize_tool_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => {
            let mut parts = Vec::new();
            for (k, v) in map.iter().take(3) {
                let v_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                let v_short: String = truncate_chars(&v_str, 50);
                parts.push(format!("{}={}", k, v_short));
            }
            parts.join(", ")
        }
        _ => String::new(),
    }
}