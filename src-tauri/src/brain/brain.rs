use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::manager::AppConfig;
use crate::dialogue::DialogueManager;
use crate::engine::manifest::ResourceManifest;
use crate::emotion::{EmotionAnalyzer, EmotionBridge};
use crate::error::{VivianError, VivianResult};
use crate::memory::MemoryManager;
use crate::memory::EpisodeStore;
use crate::persona::PersonaEngine;
use crate::pet_controller::PetController;
use crate::presence::{PresenceChangeReason, PresenceManager, PresenceState};
use crate::proactive::ProactiveOrchestrator;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::psychology::{default_psychology_path, PersonaProfile, PsychologyManager};
use crate::speech::TtsManager;
use crate::tools::ToolSystem;
use crate::types::response::{AiResponse, ChatMessage};
use crate::utils::EnvironmentManager;
use crate::world::WorldState;

use super::chat_chain::BrainChatChain;

/// Brain - AI 大脑，集成所有子系统
///
/// 包含 13 个子系统：
/// EnvironmentManager / DialogueManager / EmotionAnalyzer /
/// ToolManager / MemoryManager / PersonaEngine /
/// CommandHandler / PromptBuilder / MemoryRetentionGuard / JSONProcessor /
/// ProactiveFeatures / RunnableChain / TtsManager / PsychologyManager
/// （原 RelationshipManager 已整合到 PsychologyManager）
#[derive(Clone)]
pub struct Brain {
    pub config: AppConfig,
    pub router: Arc<ModelRouter>,
    pub memory: Arc<MemoryManager>,
    pub tool_system: Arc<ToolSystem>,
    pub chat_chain: Option<Arc<BrainChatChain>>,
    pub persona: Arc<PersonaEngine>,
    pub proactive: Arc<ProactiveOrchestrator>,
    pub tts: Arc<TtsManager>,
    pub environment: Arc<EnvironmentManager>,
    pub dialogue: Arc<DialogueManager>,
    pub emotion_analyzer: Arc<EmotionAnalyzer>,
    pub emotion_bridge: Arc<EmotionBridge>,
    /// 即时嵌入分类器引用（用于 lib.rs setup 注入进度回调）
    pub embedding_classifier: Option<Arc<crate::emotion::embedding_classifier::EmbeddingEmotionClassifier>>,
    /// 快速语义分析器（多维度嵌入分类：情绪/意图/话题/记忆重要性/关系信号）
    pub fast_semantic: Option<Arc<crate::emotion::fast_semantic::FastSemanticAnalyzer>>,
    /// 心理系统管理器：五层心理架构 + 关系系统（已整合原 RelationshipManager）
    pub psychology: Arc<PsychologyManager>,
    /// 世界状态提供者：让 Vivian 感知真实世界（天气/节气/节日/日出日落）
    pub world_provider: Arc<crate::world::WorldStateProvider>,
    /// 世界状态核心：活动追踪 + 常识查询 + 观察生成
    pub world_state: Arc<WorldState>,
    /// 用户研究管理器：LLM 驱动的行为习惯观察与统计聚合
    pub research: Arc<crate::research::ResearchManager>,
    /// 记忆巩固器：夜间（2-5 点）整理记忆，模拟"睡眠巩固"
    pub consolidator: Option<Arc<crate::memory::consolidation::MemoryConsolidator>>,
    /// 角色认知聚合句柄：Belief / Goal / Attention + 心理架构
    pub mind: Arc<crate::mind::Mind>,
    /// 凝神/专注模式状态机
    pub focus_state: Arc<Mutex<super::focus_mode::FocusState>>,
    /// 在场状态管理器（四态状态机：Online/Busy/Rest/Offline）
    pub presence: Arc<PresenceManager>,
    /// 自我状态聚合器（只读整合 PetMindState/presence/fatigue/quiet_mode/ignored_count/今日主动次数）
    ///
    /// snapshot() 时统一读取 proactive/presence/psychology 三方状态，
    /// 供 prompt 注入"当前自我状态"段落与 proactive 防打扰决策查询。
    pub self_state: Arc<crate::self_state::SelfState>,
    /// 最近一次启动问候生成的错误信息（供前端 show toast 提示）
    last_greeting_error: Arc<Mutex<Option<String>>>,
    /// 统一认知循环调度器（6 阶段流水线，替换原 mind_tick + proactive.tick 双 tick）
    ///
    /// 把 30s mind_tick 与 10s proactive_tick 合并为显式流水线：
    /// WorldIngest → SelfUpdate → Observe → Think → Act → Speak
    /// 仅 Speak 阶段调用 LLM，其余阶段纯规则。
    pub cognitive_tick: Arc<super::cognitive_tick::CognitiveTickRunner>,
    /// 角色 ID（多角色架构下标识当前 Brain 所属角色）
    pub char_id: String,
}

impl Brain {
    pub async fn new(
        config: AppConfig,
        router: ModelRouter,
        memory: MemoryManager,
        manifest: Arc<ResourceManifest>,
        char_id: &str,
        tool_system: Arc<ToolSystem>,
        world_provider: Arc<crate::world::WorldStateProvider>,
    ) -> VivianResult<Self> {
        Self::build(config, router, memory, None, manifest, char_id, tool_system, world_provider).await
    }

    /// 带 PetController 的构造函数 —— 启用聊天链的 control_actions 执行能力。
    pub async fn new_with_pet_controller(
        config: AppConfig,
        router: ModelRouter,
        memory: MemoryManager,
        pet_controller: Arc<PetController>,
        manifest: Arc<ResourceManifest>,
        char_id: &str,
        tool_system: Arc<ToolSystem>,
        world_provider: Arc<crate::world::WorldStateProvider>,
    ) -> VivianResult<Self> {
        Self::build(config, router, memory, Some(pet_controller), manifest, char_id, tool_system, world_provider).await
    }

    /// 统一构造逻辑：`pet_controller` 为 None 时不启用 control_actions 执行。
    async fn build(
        config: AppConfig,
        router: ModelRouter,
        memory: MemoryManager,
        pet_controller: Option<Arc<PetController>>,
        manifest: Arc<ResourceManifest>,
        char_id: &str,
        tool_system: Arc<ToolSystem>,
        world_provider: Arc<crate::world::WorldStateProvider>,
    ) -> VivianResult<Self> {
        let router = Arc::new(router);
        let memory = Arc::new(memory);

        // Episode 经历封包索引：与 unified_memory.json 同目录
        {
            let memory_dir = crate::utils::path::get_character_data_dir(char_id).join("memory");
            let episode_path = memory_dir.join("episodes.json");
            let episode_store = Arc::new(EpisodeStore::new(episode_path));
            memory.set_episode_store(episode_store);
        }

        // 初始化对话管理器（注入 LLM 意图判断函数）
        let mut dialogue_inner = DialogueManager::new(10, char_id);
        if let Err(e) = dialogue_inner.load_history() {
            tracing::error!("加载对话历史失败: {}", e);
        }
        let dialogue = Arc::new(dialogue_inner);
        dialogue.start_background_flush();

        // 初始化人格引擎
        let persona = Arc::new(PersonaEngine::new(char_id)?);
        persona.set_language(&config.base.language);
        let persona_config = persona.get_config();
        let expr = &persona_config.expression;

        // 初始化心理系统管理器（五层心理架构 + 关系系统）
        // 从人设表达维度推导 PersonaProfile，并持久化到 psychology.json
        // 关系系统已整合到 PsychologyManager，不再需要独立的 RelationshipManager
        let psy_path = default_psychology_path(&crate::utils::path::get_character_data_dir(char_id));
        let persona_profile = PersonaProfile::from_expression(
            expr.tsundere,
            expr.clingy,
            expr.genki,
            expr.sass,
            expr.healing,
            expr.curiosity,
        );
        let psychology = Arc::new(
            PsychologyManager::load_or_init(psy_path)
                .with_persona(persona_profile)
                .with_manifest(manifest.clone()),
        );

        // 初始化情绪桥接器（注入 PsychologyManager + manifest + 即时嵌入分类器）
        let embedding_provider = memory.embedding();
        let language = config.base.language.clone();
        let instant_classifier = Arc::new(
            crate::emotion::embedding_classifier::EmbeddingEmotionClassifier::new(embedding_provider.clone(), language.clone()),
        );
        let emotion_bridge = Arc::new(
            EmotionBridge::new(psychology.clone(), Some(manifest.clone()))
                .with_instant_classifier(instant_classifier.clone()),
        );

        // 初始化快速语义分析器（复用嵌入分类器和嵌入 provider）
        let fast_semantic = Arc::new(
            crate::emotion::fast_semantic::FastSemanticAnalyzer::new(
                instant_classifier.clone(),
                embedding_provider.clone(),
                language.clone(),
            ),
        );

        // 初始化工具语义筛选器（复用嵌入 provider，对工具描述做语义匹配）
        let tool_semantic_filter = Arc::new(
            crate::tools::ToolSemanticFilter::new(embedding_provider, language),
        );

        // 初始化情绪分析器（注入 LLM 和情绪桥接器）
        let emotion_analyzer = Arc::new(
            EmotionAnalyzer::new()
                .with_llm(router.clone())
                .with_bridge(emotion_bridge.clone()),
        );

        // 主动对话编排器
        let proactive = Arc::new(ProactiveOrchestrator::new(char_id)?);
        // 注入心理系统管理器，启用 Behavior Drive 混合模式 + Homeostasis tick + 关系策略
        proactive.set_psychology(psychology.clone());
        // 注入 LLM 路由器，使主动对话优先走 LLM 生成，失败时降级到启发式模板池
        proactive.set_model_router(router.clone());
        // 注入主动对话运行时配置（设置面板的"主动对话"页签字段立即生效）
        proactive.set_config(config.proactive.clone());
        // 注入人格引擎 + 对话管理器：让 LLM prompt 使用真实人设风格约束 + 携带最近对话历史
        proactive.set_persona(persona.clone());
        proactive.set_dialogue(dialogue.clone());
        // 注入记忆管理器：让 MemoryRecall 能读取未闭环 open_hooks 主动追问
        proactive.set_memory(memory.clone());

        // RAG 已合并入 MemoryManager（MemoryType::Knowledge）
        // 启动时一次性迁移旧 rag/documents.json
        Self::migrate_legacy_rag(&memory).await;

        // TTS 管理器（按角色隔离持久化配置）
        let tts = Arc::new(TtsManager::new(char_id)?);

        // 环境管理器
        let environment = Arc::new(EnvironmentManager::new());

        // 世界状态提供者：由 AppState 全局共享传入，跨角色共用一份天气/音乐/音量/前台/网络监听
        // 各角色通过 add_foreground_listener 订阅前台窗口事件，独立写入各自的 user_behaviors.json
        // 注入世界状态到主动对话编排器，启用世界事件检测 + 内心独白
        proactive.set_world_provider(world_provider.clone());

        // 世界状态核心：用户实体状态追踪 + 行为日志持久化
        let world_state = Arc::new(WorldState::with_behavior_log(
            crate::utils::path::get_character_data_dir(char_id)
                .join("mind")
                .join("user_behaviors.json"),
        ));

        // 用户研究管理器：LLM 驱动的行为习惯观察与统计聚合
        let research = Arc::new(crate::research::ResearchManager::new(char_id));

        // 统一的前台窗口处理：记录活动日志 + 同步写入 UserBehaviorLog
        // ActivityJournal 在 record() 内部运行双层分类器，产生 activity_label；
        // 此处通过 latest_classification() 获取分类结果，直接写入 UserBehaviorLog，
        // 避免重复调用分类器。
        {
            let journal = proactive.activity_journal().clone();
            let ws = world_state.clone();
            world_provider.add_foreground_listener(std::sync::Arc::new(move |fw| {
                journal.record(fw.title.clone(), fw.process.clone());
                // 从 ActivityJournal 获取分类结果，同步到 UserBehaviorLog
                if let Some((label, confidence)) = journal.latest_classification() {
                    ws.update_user_activity_from_classifier(&label, confidence);
                }
            }));
            // 总开关启用时才开启记录（停用时 world_provider 仍监听但不写入日志）
            if config.world.enable {
                proactive.activity_journal().start();
            }
        }

        // 根据当前亲密度调整人格表达（亲密度从心理学系统读取）
        let intimacy = psychology.relationship().intimacy * 100.0;
        persona.adjust_for_relationship(intimacy, "");

        // 凝神模式状态机：与 BrainChatChain 共享同一 Arc 实例
        let focus_state: Arc<Mutex<super::focus_mode::FocusState>> =
            Arc::new(Mutex::new(super::focus_mode::FocusState::new()));

        // 在场状态管理器：按角色隔离的四态状态机（Online/Busy/Rest/Offline）
        let presence = Arc::new(
            PresenceManager::new(char_id).unwrap_or_else(|e| {
                tracing::warn!(
                    "[Brain:{}] 在场状态管理器初始化失败，使用临时目录降级: {}",
                    char_id,
                    e
                );
                PresenceManager::new_with_temp_dir(char_id).unwrap_or_else(|e2| {
                    tracing::error!(
                        error = %e2,
                        "[Brain:{}] PresenceManager 临时目录降级也失败，进程退出",
                        char_id
                    );
                    std::process::exit(1);
                })
            }),
        );

        // Mind：角色认知聚合句柄（Belief / Goal / Attention + 心理架构引用）
        // 在 chat_chain 之前构造，因为 PromptBuildingStep 需要注入 Mind
        let mind = Arc::new(crate::mind::Mind::load_or_init(char_id, psychology.clone()));
        // 注入 Mind 到主动对话编排器，内心独白完成后触发 current_thought 刷新
        proactive.set_mind(mind.clone());

        // 用户认知引擎：行为封存后自动检测与既有 Belief 的冲突
        // 冲突检测在 seal_activity 内同步触发，仅做 BeliefStore 的读取与（必要时）EMA 修正
        let cognition_engine = Arc::new(crate::mind::UserCognitionEngine::new(router.clone()));
        {
            let engine = cognition_engine.clone();
            let mind_for_hook = mind.clone();
            let lang_for_hook = config.base.language.clone();
            world_state.set_seal_hook(std::sync::Arc::new(move |entry| {
                if let Some(conflict) = engine.detect_conflict(entry, &mind_for_hook) {
                    // 暂存冲突到 Mind 的工作记忆，供下次 prompt 注入
                    let prompt_section =
                        crate::mind::UserCognitionEngine::conflict_to_prompt_section(&conflict, &lang_for_hook);
                    mind_for_hook.push_working_memory(
                        prompt_section,
                        crate::mind::WorkingMemorySource::WorldEvent,
                    );
                }
            }));
        }

        // SelfState：角色自我状态聚合器
        // 在 chat_chain 之前构造，注入 BrainChatChain 后供 PromptBuildingStep 构建"当前自我状态"段落
        let self_state = Arc::new(crate::self_state::SelfState::new(
            char_id,
            proactive.clone(),
            presence.clone(),
            psychology.clone(),
            mind.clone(),
        ));

        // 初始化聊天链：注入 PetController 时启用 control_actions 执行
        let chat_chain = Some(Arc::new(match pet_controller {
            Some(pc) => BrainChatChain::new(
                router.clone(),
                memory.clone(),
                persona.clone(),
                emotion_bridge.clone(),
                tool_system.clone(),
                dialogue.clone(),
                psychology.clone(),
                environment.clone(),
                &config,
                Some(world_provider.clone()),
                Some(world_state.clone()),
                Some(research.clone()),
                proactive.activity_journal().clone(),
                manifest.clone(),
                mind.clone(),
                char_id,
                Some(tool_semantic_filter.clone()),
                Some(fast_semantic.clone()),
            )
            .with_focus_state(focus_state.clone())
            .with_presence(presence.clone())
            .with_self_state(self_state.clone())
            .with_mind(mind.clone())
            .with_pet_controller(pc),
            None => BrainChatChain::new(
                router.clone(),
                memory.clone(),
                persona.clone(),
                emotion_bridge.clone(),
                tool_system.clone(),
                dialogue.clone(),
                psychology.clone(),
                environment.clone(),
                &config,
                Some(world_provider.clone()),
                Some(world_state.clone()),
                Some(research.clone()),
                proactive.activity_journal().clone(),
                manifest.clone(),
                mind.clone(),
                char_id,
                Some(tool_semantic_filter.clone()),
                Some(fast_semantic.clone()),
            )
            .with_focus_state(focus_state.clone())
            .with_presence(presence.clone())
            .with_self_state(self_state.clone())
            .with_mind(mind.clone()),
        }));

        // 注入 Prompt 构建步骤和工具系统给主动对话：
        // 主动问候复用主对话完整 prompt（人设/记忆/知识库/环境等）+ 注入真实工具调用历史
        if let Some(chain) = &chat_chain {
            proactive.set_prompt_step(chain.prompt_step.clone());
        }
        proactive.set_tool_system(tool_system.clone());

        // 记忆巩固器：复用聊天链的 ConsolidationPipeline，夜间整理记忆
        // 注入 Mind 启用 Stage 4（Belief/Goal 生成）
        let consolidator = if config.world.enable && config.world.enable_memory_consolidation {
            let pipeline = chat_chain.as_ref().map(|c| c.pipeline.clone());
            pipeline.map(|p| {
                Arc::new(
                    crate::memory::consolidation::MemoryConsolidator::new(
                        memory.clone(),
                        p,
                    )
                    .with_mind(mind.clone()),
                )
            })
        } else {
            None
        };

        let brain = Self {
            config,
            router,
            memory: memory.clone(),
            tool_system,
            chat_chain,
            persona,
            proactive,
            tts,
            environment,
            dialogue,
            emotion_analyzer,
            emotion_bridge,
            embedding_classifier: Some(instant_classifier),
            fast_semantic: Some(fast_semantic),
            psychology,
            world_provider,
            world_state,
            research: research.clone(),
            consolidator,
            mind,
            focus_state,
            presence: presence.clone(),
            self_state: self_state.clone(),
            last_greeting_error: Arc::new(Mutex::new(None)),
            cognitive_tick: Arc::new(super::cognitive_tick::CognitiveTickRunner::new()),
            char_id: char_id.to_string(),
        };

        // 初始化补充回复服务（slow 检索后异步生成补充回复）
        // 全局单例：OnceCell 第一次初始化后，后续角色不再覆盖，只注册自己的 MemoryManager。
        // call_slow_retrieve 会按 req.char_id 路由到对应角色的 MemoryManager，
        // 避免单例只持有第一个角色的 MemoryManager 导致跨角色记忆污染
        // （曾导致 Nana 说"我其实是 Vivian"）。
        let augment_service =
            match crate::brain::augment_reply_service::try_get_augment_reply_service() {
                Some(svc) => svc,
                None => crate::brain::augment_reply_service::init_augment_reply_service(
                    crate::brain::augment_reply_service::AugmentReplyService::new()
                        .with_router(brain.router.clone()),
                ),
            };
        augment_service.register_memory_for_char(char_id, memory.clone());

        // 注入到 presence_tools 静态 map，供 set_presence_state 工具按 char_id 取用
        crate::tools::builtin::presence_tools::register_presence_manager(
            char_id,
            presence,
            memory,
        );

        // 注入到 research_tool 静态 map，供 observe_user 工具按 char_id 取用
        crate::tools::builtin::research_tool::register_research_manager(
            char_id,
            research,
        );

        // 加载 Hook 注册表（PreToolUse / PostToolUse 拦截点）
        let hook_registry = crate::hooks::HookRegistry::load_default();
        crate::tools::executor::set_hook_registry(hook_registry);

        Ok(brain)
    }

    /// 一次性迁移：把旧 `rag/documents.json` 中的 KnowledgeDocument 导入到 MemoryManager。
    /// 迁移成功后旧文件改名为 `documents.json.migrated`，避免重复迁移。
    /// 失败时不阻塞启动，仅记录警告。
    async fn migrate_legacy_rag(memory: &MemoryManager) {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct LegacyDoc {
            id: String,
            title: String,
            content: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            source: String,
        }
        #[derive(Deserialize, Default)]
        #[allow(dead_code)]
        struct LegacyStore {
            #[serde(default)]
            documents: Vec<LegacyDoc>,
            #[serde(default)]
            saved_at: f64,
        }

        let rag_dir = crate::utils::path::get_user_data_dir().join("rag");
        let docs_path = rag_dir.join("documents.json");
        if !docs_path.exists() {
            return;
        }

        let content = match std::fs::read_to_string(&docs_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[Brain] 读取旧 RAG 文档失败，跳过迁移: {e}");
                return;
            }
        };
        if content.trim().is_empty() {
            return;
        }

        let store: LegacyStore = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[Brain] 解析旧 RAG 文档失败，跳过迁移: {e}");
                return;
            }
        };

        if store.documents.is_empty() {
            // 空文件也归档，避免反复解析
            let _ = std::fs::rename(&docs_path, rag_dir.join("documents.json.migrated"));
            return;
        }

        let total = store.documents.len();
        let mut ok = 0usize;
        for doc in store.documents {
            let mut tags = doc.tags;
            if !doc.source.is_empty() && !tags.iter().any(|t| t == &doc.source) {
                tags.push(doc.source);
            }
            match memory.add_knowledge_document(&doc.title, &doc.content, tags, "migration", None).await {
                Ok(_) => ok += 1,
                Err(e) => tracing::warn!("[Brain] 迁移文档 {} 失败: {e}", doc.title),
            }
        }

        tracing::info!("[Brain] RAG 迁移完成: {ok}/{total} 文档已转入 MemoryManager");

        // 归档旧文件
        let migrated_path = rag_dir.join("documents.json.migrated");
        if let Err(e) = std::fs::rename(&docs_path, &migrated_path) {
            tracing::warn!("[Brain] 归档旧 RAG 文件失败: {e}");
        }
        // 旧向量文件一并归档（向量会在新 MemoryManager 中重建）
        let old_vectors = rag_dir.join("vectors.json");
        if old_vectors.exists() {
            let _ = std::fs::rename(&old_vectors, rag_dir.join("vectors.json.migrated"));
        }
    }

    pub async fn think(&self, user_input: &str, stream: bool) -> VivianResult<AiResponse> {
        self.think_with_options(user_input, stream, false).await
    }

    /// 带选项的思考调用。
    ///
    /// `skip_dialogue_write`: 跳过将本轮用户消息和 AI 回复写入对话历史。
    /// 用于插话等场景——插话指令是内部生成的系统提示，不应作为用户消息出现在对话历史和记忆图谱中。
    pub async fn think_with_options(
        &self,
        user_input: &str,
        stream: bool,
        skip_dialogue_write: bool,
    ) -> VivianResult<AiResponse> {
        self.think_inner(user_input, stream, skip_dialogue_write, true).await
    }

    /// 跨角色对话专用 think：跳过异步反思（跨角色闲聊场景反思价值有限，节省 LLM 配额）。
    /// 仍走完整 pipeline（prompt 构建 → generation → validation → memory_saving）。
    pub async fn think_cross_character(
        &self,
        user_input: &str,
        stream: bool,
    ) -> VivianResult<AiResponse> {
        self.think_inner(user_input, stream, false, false).await
    }

    async fn think_inner(
        &self,
        user_input: &str,
        stream: bool,
        skip_dialogue_write: bool,
        run_reflection: bool,
    ) -> VivianResult<AiResponse> {
        // 主动对话冷却重置（关系更新由 chat_chain 在 MoodStep 后基于真实情绪执行）
        let _ = self.proactive.on_user_interacted();

        // Attention boost：用户输入驱动注意力聚焦（纯规则，不调 LLM，毫秒级）
        let now = chrono::Utc::now().timestamp();
        crate::mind::boost_attention_from_input(&self.mind, user_input, now);

        // 异步反思（合并 consciousness_update + activity_extractor）：
        // fire-and-forget，节流触发（5 轮或 30 分钟 OR 关系，激烈对话抑制）。
        // 失败静默，不阻塞主响应路径。
        // 跨角色对话场景跳过反思——闲聊价值有限，节省 LLM 配额。
        if run_reflection {
            let router = self.router.clone();
            let mind = self.mind.clone();
            let input_owned = user_input.to_string();
            let cid = self.char_id.clone();
            let psychology = Some(self.psychology.clone());

            // AI 回复 + 最近对话上下文：从 DialogueManager 历史末尾提取
            // （当前轮 AI 尚未生成，使用上一轮 AI 回复作为参考；空时降级）
            let (ai_reply_owned, recent_context_owned) = {
                let history = self.dialogue.get_history();
                let ai_reply = history
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant")
                    .map(|m| m.content.clone())
                    .unwrap_or_default();

                // 最近 4 条消息格式化为 "role: content"（截断 100 字符）
                let take = 4.min(history.len());
                let start = history.len() - take;
                let mut lines: Vec<String> = Vec::new();
                for msg in &history[start..] {
                    let role = match msg.role.as_str() {
                        "user" => "User",
                        "assistant" => "AI",
                        _ => continue,
                    };
                    let content = msg.content.trim();
                    if content.is_empty() {
                        continue;
                    }
                    let truncated = crate::utils::truncate_chars(content, 100);
                    let suffix = if content.chars().count() > 100 { "…" } else { "" };
                    lines.push(format!("{}: {}{}", role, truncated, suffix));
                }
                (ai_reply, lines.join("\n"))
            };

            tokio::spawn(async move {
                super::async_reflection::run_async_reflection(
                    router,
                    mind,
                    input_owned,
                    &ai_reply_owned,
                    &recent_context_owned,
                    cid,
                    psychology,
                )
                .await;
            });
        }

        match &self.chat_chain {
            Some(chain) => chain.ainvoke_with_options(user_input, stream, skip_dialogue_write).await,
            None => Err(VivianError::Engine("聊天链未初始化".to_string())),
        }
    }

    /// 设置流式 chunk 推送回调（转发到 BrainChatChain）
    ///
    /// 由 `chat:send_message_stream` 命令在调用 `think(stream=true)` 前注入，
    /// 让 LLM 流式生成期间每个 text 增量通过回调推送到前端 `chat:chunk` 事件。
    /// 调用结束后必须清理为 `None`。
    pub fn set_stream_emitter(
        &self,
        emitter: Option<crate::pipeline::steps::generation::StreamEmitter>,
    ) {
        if let Some(chain) = &self.chat_chain {
            chain.set_stream_emitter(emitter);
        }
    }

    pub fn clear_memory(&self) -> VivianResult<()> {
        let memory = self.memory.clone();
        tokio::spawn(async move {
            if let Err(e) = memory.clear_all_memories().await {
                tracing::warn!("清除记忆失败: {}", e);
            }
        });
        Ok(())
    }

    pub fn get_memory_summary(&self) -> String {
        self.memory.get_memory_summary()
    }

    /// 启动主动对话
    pub fn start_proactive(&self) {
        self.proactive.start();
    }

    /// 停止主动对话
    pub fn stop_proactive(&self) {
        self.proactive.stop();
    }

    /// 单次主动对话 tick（由前端定时调用）
    ///
    /// 由 CognitiveTickRunner 执行统一 6 阶段认知循环：
    /// WorldIngest → SelfUpdate → Observe → Think → Act → Speak
    ///
    /// 仅 Speak 阶段调用 LLM（主动消息触发器命中时），其余阶段纯规则。
    /// Tick 之外仍处理：Focus idle 冷却 / 夜间记忆巩固 / 日常 pipeline.run()。
    pub async fn proactive_tick(
        &self,
        context: &crate::proactive::TickContext,
    ) -> VivianResult<bool> {
        // ── 统一认知循环：6 阶段流水线 ──
        // 替换原 WorldState 超时 + Mind Tick + proactive.tick 三段独立逻辑，
        // 合并为显式流水线。每阶段独立决策，仅 Speak 调 LLM。
        let tick_result = self.cognitive_tick.run(self, context)?;
        let produced = tick_result.produced_user_message;

        // 调试日志：阶段决策摘要（trace 级，不污染普通日志）
        tracing::trace!(
            "[cognitive_tick:{}] {}",
            self.char_id,
            tick_result.render_summary()
        );

        // 凝神模式 idle 冷却：proactive tick 期间用户未交互，Focus 电荷按 idle retention 衰减
        {
            let now = chrono::Local::now().timestamp() as f64;
            let th = super::focus_mode::FocusThresholds::default();
            let mut fs = self.focus_state.lock().await;
            fs.idle_cooldown(produced, now, &th);
        }

        // 记忆巩固：由 Rest 状态转换触发（见 presence::background_tasks），
        // 此处仅保留日常 pipeline.run() 检查，让 Stage 1 的"空闲触发"条件能在用户离开期间被检查到。
        // 不再使用夜间窗口触发，巩固时机跟随智能体的实际作息。

        // 日常巩固检查：每 tick（约 30s）调用一次 pipeline.run()，
        // 让 Stage 1 的"空闲触发"条件能在用户离开期间被检查到。
        //
        // 背景：原本 pipeline.run() 只在每轮对话后调用，导致空闲触发条件
        // （ShortTerm 非空且距最新一条 ≥ 30 分钟）虽然满足但无人检查——
        // 用户离开后没有新对话，pipeline.run() 不被调用，ShortTerm 永远不升级。
        //
        // pipeline.run() 内部各 Stage 自带条件门控，不满足时直接返回 None 不调 LLM，
        // 因此高频轮询是安全的；运行锁防止与对话路径并发竞态。
        if let Some(chat_chain) = &self.chat_chain {
            let pipeline = chat_chain.pipeline.clone();
            let memory = self.memory.clone();
            tokio::spawn(async move {
                if let Err(e) = pipeline.run(&memory).await {
                    tracing::warn!("[Brain] 日常巩固检查失败: {}", e);
                }
            });
        }

        Ok(produced)
    }

    /// 消费所有待发送的主动行为
    pub fn drain_proactive_messages(&self) -> Vec<crate::proactive::ProactiveAction> {
        self.proactive.drain_messages()
    }

    /// 朗读文本（对话流程入口，TTS 未启用时静默跳过）
    ///
    /// 通过全局 SpeechPlanner 调度,Brain 不再直接接触 TtsManager。
    pub async fn speak(&self, text: &str) -> VivianResult<()> {
        self.speak_with_emotion(text, None).await
    }

    /// 朗读文本（带情感参数，用于 GPT-SoVITS emotionVoiceMap 音色切换）
    ///
    /// 构造 SpeakIntent 提交给 SpeechPlanner,由 Planner 决定何时真正播放。
    pub async fn speak_with_emotion(
        &self,
        text: &str,
        emotion: Option<&str>,
    ) -> VivianResult<()> {
        if !self.tts.is_enabled() {
            return Ok(());
        }

        let speak_text = if self.dialogue.get_channel() == "direct" {
            crate::utils::filter_parentheses_sync(text)
        } else {
            text.to_string()
        };

        let intent = crate::speech::speak_intent(&speak_text, &self.char_id)
            .emotion(emotion.unwrap_or_default())
            .priority(crate::speech::SpeechPriority::Normal)
            .build();

        let planner = crate::speech::get_planner().await;
        let handle = planner.submit(intent).await?;
        match handle.done().await {
            crate::speech::SubmitResult::Played => Ok(()),
            crate::speech::SubmitResult::Dropped => Ok(()),
            crate::speech::SubmitResult::Failed(msg) => {
                Err(crate::error::VivianError::Speech(msg))
            }
        }
    }

    /// 生成启动问候（通过 LLM 生成，失败返回 None）
    ///
    /// 首次见面（无真实记忆，仅有种子记忆）请求 LLM 生成自我介绍式问候；
    /// 老朋友根据时间、亲密度等信息请求 LLM 生成回归问候。
    /// 问候生成成功后会写入对话历史与记忆系统，并在聊天记录中可见。
    /// 判定依据：non_seed_count() == 0 即首次见面——
    /// seed_if_empty() 在 MemoryManager::new() 时已植入种子记忆，
    /// 因此 entry_count() 从不为 0；须排除种子才能正确判断。
    pub async fn generate_startup_greeting(&self) -> Option<String> {
        let rel = self.psychology.relationship();
        let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
        let intimacy = rel.intimacy * 100.0;
        let is_first_meeting = self.memory.non_seed_count() == 0 && self.dialogue.get_history_length() == 0;

        // 读取当前情绪与天气，让问候语气与状态衔接
        // 情绪跨会话保留（上次对话结束的情绪带到这次开场），天气提供情境感
        let (emotion_label, emotion_value) = self.psychology.emotion().dominant();
        let world_snapshot = self.world_provider.snapshot(None);
        let weather_brief = if let Some(w) = &world_snapshot.weather {
            format!("{}, {:.0}°C", w.description, w.temperature)
        } else {
            "unknown".to_string()
        };
        let current_state_brief = format!(
            "## Current State\n\
            - Time: {}\n\
            - Weather here: {}\n\
            - Your dominant emotion right now: {} ({:.2}) — let this color your tone",
            world_snapshot.local_time,
            weather_brief,
            emotion_label.as_str(),
            emotion_value
        );

        // 用户界面语言 → LLM 回复语言（硬规则）：从设置面板的 base.language 读取
        let user_language_name = language_code_to_name(&self.config.base.language);

        let time_desc = match hour {
            5..=10 => "early morning",
            11..=13 => "midday",
            14..=17 => "afternoon",
            18..=21 => "evening",
            _ => "late night",
        };

        // 用户消息：首次见面时在前面加一句"这是首次见面"提示，其余与一般对话一致。
        // 首次见面走完整对话流水线（与直接渠道同一套提示词 + 记忆检索），
        // 让问候带着种子前史生成，而不是一张白纸。
        let greeting_prompt = if is_first_meeting {
            format!(
                "这是你第一次见到这个用户，你们还是陌生人。现在是{time_desc}。请自然地打个招呼并简单介绍一下自己，用{user_language_name}回复。"
            )
        } else {
            let memory_text = self.build_startup_greeting_memory_text().await;
            let context_section = if memory_text.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n## Context from last time\n{}\n\n\
                     Pay close attention to the timestamps and time-spans above — they tell you how long ago each exchange happened. \
                     If the last conversation was yesterday or several hours ago, treat it accordingly (e.g. \"yesterday you mentioned going to buy snacks, how'd that go?\" rather than assuming it just happened). \
                     You can naturally continue where you left off, but do NOT repeat what's already said — just pick up the thread like a real friend would.",
                    memory_text
                )
            };
            format!(
                "用户回来了。现在是{time_desc}。请自然地打个招呼。用{user_language_name}回复。{}",
                context_section
            )
        };

        // 走完整对话流水线生成（与一般直接渠道对话相同提示词，含记忆检索→种子记忆在场）。
        // 返回 AiResponse；对话写回与记忆由下方独立后处理完成，避免把问候指令污染记忆库。
        let greeting_text = match &self.chat_chain {
            Some(chain) => match chain.ainvoke_greeting(&greeting_prompt).await {
                Ok(resp) => {
                    let t = resp.text.trim().trim_matches('"').trim_matches('「').trim_matches('」').to_string();
                    if t.is_empty() {
                        *self.last_greeting_error.lock().await = None;
                        None
                    } else {
                        Some(t)
                    }
                }
                Err(e) => {
                    tracing::warn!("[startup_greeting] 完整流水线生成问候失败: {}", e);
                    *self.last_greeting_error.lock().await = Some(format!("LLM 调用失败: {}", e));
                    None
                }
            },
            // chat_chain 未就绪时回退到精简定制提示词
            None => {
                let character_block = self.persona.get_character_block();
                let examples_block = self.persona.get_examples_block();
                let style_block = self.persona.build_style_prompt(intimacy, hour);
                let chat_style_framework = crate::pipeline::prompt_modules::chat_style_framework(&self.config.base.language);
                let system_prompt = format!(
                    "{character_block}\n\n\
                    {examples_block}\n\n\
                    {style_block}\n\n\
                    {chat_style_framework}\n\n\
                    ## Task: Generate a startup greeting\n\
                    Generate a short startup greeting that fits your persona and the current time.\n\
                    Requirements:\n\
                    - Output ONLY the greeting itself — no quotes, no explanation, no extra newlines\n\
                    - Keep it under 30 characters\n\
                    - Reply in {user_language_name}.\n\n\
                    {current_state_brief}"
                );
                let messages = vec![
                    ChatMessage::system(system_prompt),
                    ChatMessage::user(greeting_prompt),
                ];
                match self.router.generate(
                    LLMRequest::new("chat", messages).with_temperature(0.9)
                ).await {
                    Ok(text) => {
                        let t = text.trim().trim_matches('"').trim_matches('「').trim_matches('」').to_string();
                        if t.is_empty() {
                            *self.last_greeting_error.lock().await = None;
                            None
                        } else {
                            Some(t)
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[startup_greeting] LLM 生成启动问候失败: {}", e);
                        *self.last_greeting_error.lock().await = Some(format!("LLM 调用失败: {}", e));
                        None
                    }
                }
            }
        };

        // 独立后处理：写入对话历史 + 记忆系统 + 纳入主动问候冷却
        if let Some(greeting) = greeting_text {
            *self.last_greeting_error.lock().await = None;
            let mut greeting_msg = ChatMessage::assistant(greeting.as_str());
            greeting_msg.meta = Some(crate::messages::MessageMeta::assistant().with_channel("proactive"));
            self.dialogue.add_message(greeting_msg);
            // 写入记忆系统（role 通过 tags 标记，记忆管理面板可显示）
            let tags = vec![
                "assistant".to_string(),
                "startup_greeting".to_string(),
                "dialogue_turn".to_string(),
            ];
            let meta = serde_json::json!({
                "channel": "proactive",
                "speaker": self.char_id,
                "listener": "user",
                "perspective": "speaker",
                "knowledge_source": "direct",
            });
            // 与主对话统一格式：给裸问候补上说话者前缀（如 "[I say to User]"）。
            // 前端/对话历史展示的是剥离前缀后的原始问候，仅记忆入库时带前缀统一格式。
            let prefixed = format!(
                "{} {}",
                crate::cross_character::build_speaker_prefix(
                    &self.char_id, "user", &self.char_id
                ),
                greeting
            );
            if let Err(e) = self
                .memory
                .add_memory_with_metadata(&prefixed, crate::memory::types::MemoryType::CasualConversation, 0.3, tags, meta)
                .await
            {
                tracing::warn!("[startup_greeting] 写入记忆系统失败: {}", e);
            }
            // 纳入主动问候共享冷却，避免启动问候后很快又触发主动问候
            self.proactive.record_greeting_arrival("startup_greeting");
            Some(greeting)
        } else {
            None
        }
    }

    /// 为启动问候构建记忆检索文本（仅老朋友场景）
    ///
    /// 注入两部分上下文：(1) 最近的对话历史（让 LLM 能延续上次话题）
    /// (2) 语义记忆检索（用户偏好/共同回忆等长期记忆）
    async fn build_startup_greeting_memory_text(&self) -> String {
        let mut sections: Vec<String> = Vec::new();

        // 历史启动问候：注入最近 3 条，让 LLM 避免重复相同开场
        let recent_greetings = self.memory.recent_by_tags(&["startup_greeting"], 3);
        if !recent_greetings.is_empty() {
            let now = chrono::Local::now();
            let parts: Vec<String> = recent_greetings
                .iter()
                .map(|m| {
                    let ts_local = chrono::DateTime::from_timestamp(m.timestamp as i64, 0)
                        .map(|ts| ts.with_timezone(&chrono::Local));
                    let span = ts_local
                        .map(|ts| {
                            let elapsed = now.signed_duration_since(ts);
                            if elapsed.num_hours() < 1 {
                                format!("{}分钟前", elapsed.num_minutes().max(1))
                            } else if elapsed.num_hours() < 24 {
                                format!("{}小时前", elapsed.num_hours())
                            } else {
                                format!("{}天前", elapsed.num_days())
                            }
                        })
                        .unwrap_or_else(|| "更早".to_string());
                    format!("- [{}] {}", span, m.content.chars().take(50).collect::<String>())
                })
                .collect();
            sections.push(format!(
                "## 最近用过的开场（DO NOT repeat these — vary your wording）\n{}",
                parts.join("\n")
            ));
        }

        // 最近对话历史：取最后几轮，让 LLM 知道上次聊到哪了
        let history = self.dialogue.get_history();
        if !history.is_empty() {
            let recent: Vec<&ChatMessage> = history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
            let now = chrono::Local::now();
            let dialogue_parts: Vec<String> = recent
                .iter()
                .filter_map(|m| {
                    if m.content.trim().is_empty() {
                        return None;
                    }
                    let speaker = if m.role == "user" {
                        "用户"
                    } else if m.role == "assistant" {
                        if self.char_id == "vivian" { "薇薇安" } else { "娜娜" }
                    } else {
                        return None;
                    };
                    // 展示完整日期+时间+距今跨度，让 LLM 能感知对话发生的时间
                    // （跨天/跨小时场景下，仅 HH:MM 会让 LLM 误判为刚刚发生）
                    let time_str = m.timestamp
                        .map(|ts| {
                            let date_label = if ts.date_naive() == now.date_naive() {
                                ts.format("今天 %H:%M").to_string()
                            } else if ts.date_naive() == (now - chrono::Duration::days(1)).date_naive() {
                                ts.format("昨天 %H:%M").to_string()
                            } else {
                                ts.format("%m-%d %H:%M").to_string()
                            };
                            let elapsed = now.signed_duration_since(ts.with_timezone(&chrono::Local));
                            let span = if elapsed.num_hours() < 1 {
                                format!("{}分钟前", elapsed.num_minutes().max(1))
                            } else if elapsed.num_hours() < 24 {
                                format!("{}小时前", elapsed.num_hours())
                            } else {
                                format!("{}天前", elapsed.num_days())
                            };
                            format!("{}（{}）", date_label, span)
                        })
                        .unwrap_or_default();
                    Some(format!("[{}] {}: {}", time_str, speaker, m.content.chars().take(150).collect::<String>()))
                })
                .collect();
            if !dialogue_parts.is_empty() {
                sections.push(format!("## 最近对话（上次聊到的内容）\n{}", dialogue_parts.join("\n")));
            }
        }

        // 语义记忆检索：用户偏好/共同回忆等长期记忆
        let query = "用户偏好 最近对话 共同回忆";
        match self
            .memory
            .search_memories(query, crate::memory::types::RetrievalStrategy::Auto, 5)
            .await
        {
            Ok(items) if !items.is_empty() => {
                let parts: Vec<String> = items
                    .iter()
                    .take(5)
                    .map(|m| {
                        let role = if m.tags.iter().any(|t| t == "user") {
                            "用户"
                        } else if m.tags.iter().any(|t| t == "assistant") {
                            "薇薇安"
                        } else {
                            ""
                        };
                        let prefix = if role.is_empty() { "" } else { &role };
                        if prefix.is_empty() {
                            format!("- {}", m.content)
                        } else {
                            format!("- {}: {}", prefix, m.content)
                        }
                    })
                    .collect();
                sections.push(format!("## 相关记忆\n{}", parts.join("\n")));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[startup_greeting] 记忆检索失败: {}", e);
            }
        }

        sections.join("\n\n")
    }

    /// 获取最近一次启动问候生成的错误信息（供前端 show toast 提示）
    pub async fn last_greeting_error(&self) -> Option<String> {
        self.last_greeting_error.lock().await.clone()
    }

    /// 生成睡眠唤醒问候（通过 LLM 生成，失败返回 None）
    ///
    /// 用户从睡眠中唤醒桌宠时，根据当前时间段、关系阶段、心理状态生成一句问候。
    pub async fn generate_wake_greeting(&self) -> Option<String> {
        let rel = self.psychology.relationship();
        let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
        let intimacy = rel.intimacy * 100.0;
        let _name = self.persona.get_name();
        let mood = self.psychology.compute_mood();
        let user_language_name = language_code_to_name(&self.config.base.language);

        let character_block = self.persona.get_character_block();
        let examples_block = self.persona.get_examples_block();
        let style_block = self.persona.build_style_prompt(intimacy, hour);
        let chat_style_framework = crate::pipeline::prompt_modules::chat_style_framework(&self.config.base.language);

        let system_prompt = format!(
            "{character_block}\n\n\
            {examples_block}\n\n\
            {style_block}\n\n\
            {chat_style_framework}\n\n\
            ## Task: Generate a wake-up greeting\n\
            You just woke up from sleep. The user has come back. Greet them naturally.\n\
            Requirements:\n\
            - Output ONLY the greeting itself — no quotes, no explanation, no extra newlines\n\
            - Keep it under 30 characters\n\
            - Talk EXACTLY like a real person who just woke up — sleepy, mumbly, casual\n\
            - ZERO poetic/literary/artistic/metaphorical language. No flowery phrases. No imagery.\n\
            - You can show just-woken-up state (sleepy, stretching, yawning etc.) — but keep it natural, not cute-acting\n\
            - You MUST reply in {}. This is a hard rule — never use any other language.",
            user_language_name
        );

        let time_desc = match hour {
            5..=10 => "early morning",
            11..=13 => "midday",
            14..=17 => "afternoon",
            18..=21 => "evening",
            _ => "late night",
        };

        let user_prompt = format!(
            "It's {} now. You just woke up, the user is back. Current intimacy stage: {}, interaction count: {}, fatigue: {:.0}, mood valence: {:.2}. Generate a natural wake-up greeting. Reply in {}.",
            time_desc,
            rel.permanent_stage.as_str(),
            rel.interaction_count,
            mood.fatigue,
            mood.valence,
            user_language_name
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(user_prompt),
        ];

        match self.router.generate(LLMRequest::new("chat", messages)).await {
            Ok(text) => {
                let greeting = text.trim().trim_matches('"').trim_matches('「').trim_matches('」').to_string();
                if greeting.is_empty() {
                    None
                } else {
                    // 纳入主动问候共享冷却，避免唤醒问候后很快又触发主动问候
                    self.proactive.record_greeting_arrival("wake_greeting");
                    Some(greeting)
                }
            }
            Err(e) => {
                tracing::warn!("LLM 生成唤醒问候失败: {}", e);
                None
            }
        }
    }

    /// 生成状态切换告别语（通过 LLM 生成，失败返回 None）
    ///
    /// 当智能体自动切换到 Rest/Offline 状态时，先生成一句告别语告知用户。
    /// 调用方负责通过气泡显示 + TTS 朗读，然后再执行状态切换（隐藏窗口）。
    pub async fn generate_farewell_greeting(
        &self,
        target_state: PresenceState,
        reason: PresenceChangeReason,
    ) -> Option<String> {
        let rel = self.psychology.relationship();
        let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
        let intimacy = rel.intimacy * 100.0;
        let mood = self.psychology.compute_mood();
        let user_language_name = language_code_to_name(&self.config.base.language);

        let character_block = self.persona.get_character_block();
        let examples_block = self.persona.get_examples_block();
        let style_block = self.persona.build_style_prompt(intimacy, hour);
        let chat_style_framework = crate::pipeline::prompt_modules::chat_style_framework(&self.config.base.language);

        let (state_desc, reason_desc) = match (target_state, &reason) {
            (PresenceState::Rest, PresenceChangeReason::MoodDriven) => (
                "going to rest (feeling tired)",
                "You're feeling tired and want to take a short nap.",
            ),
            (PresenceState::Rest, PresenceChangeReason::Coordination) => (
                "going to rest (taking turns with the other character)",
                "You and the other character agreed to take turns being online, and it's your turn to rest.",
            ),
            (PresenceState::Offline, PresenceChangeReason::Ignored) => (
                "going offline (feeling ignored)",
                "You've tried reaching out a few times but didn't get a response, so you're going offline for a while.",
            ),
            (PresenceState::Offline, PresenceChangeReason::MoodDriven) => (
                "going offline (feeling lonely)",
                "You're feeling a bit lonely and want some alone time offline.",
            ),
            _ => match target_state {
                PresenceState::Rest => ("going to rest", "You're going to take a short rest."),
                PresenceState::Offline => ("going offline", "You're going offline for a while."),
                _ => return None,
            },
        };

        let system_prompt = format!(
            "{character_block}\n\n\
            {examples_block}\n\n\
            {style_block}\n\n\
            {chat_style_framework}\n\n\
            ## Task: Generate a farewell message before {state_desc}\n\
            You are about to {state_desc}. Say a brief, natural farewell to the user.\n\
            Context: {reason_desc}\n\
            Requirements:\n\
            - Output ONLY the farewell message — no quotes, no explanation, no extra newlines\n\
            - Keep it under 30 characters\n\
            - Talk EXACTLY like a real person — short, casual, natural\n\
            - If going to rest, mention you'll be back soon (they can still WeChat you)\n\
            - If going offline, mention they can reach you by WeChat\n\
            - ZERO poetic/literary/artistic language. No flowery phrases.\n\
            - You MUST reply in {user_language_name}. This is a hard rule — never use any other language.",
        );

        let time_desc = match hour {
            5..=10 => "early morning",
            11..=13 => "midday",
            14..=17 => "afternoon",
            18..=21 => "evening",
            _ => "late night",
        };

        let user_prompt = format!(
            "It's {} now. You're {}. Current intimacy stage: {}, interaction count: {}, fatigue: {:.0}. Generate a natural farewell. Reply in {}.",
            time_desc,
            state_desc,
            rel.permanent_stage.as_str(),
            rel.interaction_count,
            mood.fatigue,
            user_language_name
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(user_prompt),
        ];

        match self.router.generate(LLMRequest::new("chat", messages)).await {
            Ok(text) => {
                let farewell = text.trim().trim_matches('"').trim_matches('「').trim_matches('」').to_string();
                if farewell.is_empty() {
                    None
                } else {
                    Some(farewell)
                }
            }
            Err(e) => {
                tracing::warn!("[farewell_greeting] LLM 生成告别语失败: {}", e);
                None
            }
        }
    }

    /// 构建提示词模板预览（无需实际发起对话）
    ///
    /// Build a template preview of the prompt structure for the Context Pipeline UI.
    ///
    /// Design principle:
    /// - **Static config sections** (Character, Examples, Style, Framework rules, Output Format,
    ///   Speaker Prefix, Post-processor spec) use real config data — these are fixed rules.
    /// - **Dynamic runtime sections** (Relationship, Mind, Memory, Emotion, Episodes, Log, etc.)
    ///   use explanatory placeholders that describe: what the section does, its data source,
    ///   possible value ranges, dynamic behavior, and a structural example (not real data).
    ///
    /// This lets users see the full prompt architecture without leaking live conversation data.
    pub fn build_prompt_template_preview(&self) -> crate::mind::reasoning_trace::PromptBreakdown {
        use crate::mind::reasoning_trace::{truncate_chars, PromptBreakdown, PromptSection};

        // Build static sections from real config
        let mut sections: Vec<(&str, String)> = Vec::new();

        // 1. Character — static: identity + personality + background + interests + appearance + speech
        let character_block = {
            use crate::persona::prompt_render::render_character_block;
            let cfg = self.persona.get_config();
            let lang = self.persona.get_language();
            render_character_block(&cfg, &lang)
        };
        sections.push(("Character", character_block));

        // 2. Examples — static: character-specific few-shot examples
        let examples_block = {
            use crate::persona::prompt_render::render_examples_block;
            let cfg = self.persona.get_config();
            let lang = self.persona.get_language();
            render_examples_block(&cfg, &lang)
        };
        sections.push(("Examples", examples_block));

        // 3. Style — static config: scene modes + taboos (showing structure, frontend switches views via scene_modes)
        let (style_content, scene_modes_preview) = {
            use crate::persona::schemas::SceneMode;
            let cfg = self.persona.get_config();
            let modes = [
                SceneMode::Morning,
                SceneMode::Companion,
                SceneMode::Cozy,
                SceneMode::Banter,
                SceneMode::Comforting,
                SceneMode::Guardian,
                SceneMode::Energetic,
                SceneMode::DailyChat,
            ];
            let mut mode_previews: Vec<crate::mind::reasoning_trace::SceneModePreview> = Vec::new();
            for mode in &modes {
                let mode_str = mode.as_str();
                if let Some(mc) = cfg.scene_modes.get(mode) {
                    mode_previews.push(crate::mind::reasoning_trace::SceneModePreview {
                        mode: mode_str.to_string(),
                        description: mc.description.clone(),
                        instructions: mc.extra_instructions.clone(),
                    });
                }
            }
            let default_mode = SceneMode::DailyChat;
            let mut blocks: Vec<String> = Vec::new();
            blocks.push(format!("#### Active Mode: `{}` (dynamically selected each turn based on context, time, emotion)", default_mode.as_str()));
            blocks.push(String::new());
            blocks.push("The system selects ONE mode per turn based on time of day, recent conversation context, and user emotion. Available modes:".to_string());
            for mp in &mode_previews {
                blocks.push(format!("- **{}** — {}", mp.mode, mp.description));
            }
            blocks.push(String::new());
            blocks.push("Below is the currently selected mode's instructions (example view — switches dynamically):".to_string());
            blocks.push(String::new());
            if let Some(mc) = cfg.scene_modes.get(&default_mode) {
                if !mc.description.is_empty() {
                    blocks.push(mc.description.clone());
                    blocks.push(String::new());
                }
                if !mc.extra_instructions.is_empty() {
                    blocks.push("**Mode-Specific Instructions:**".to_string());
                    for instruction in &mc.extra_instructions {
                        blocks.push(format!("- {}", instruction));
                    }
                    blocks.push(String::new());
                }
            }
            let active_taboos: Vec<_> = cfg.identity.taboos.iter().filter(|t| t.enabled).collect();
            if !active_taboos.is_empty() {
                blocks.push("### NO-GO (DO NOT VIOLATE — applies to ALL modes)".to_string());
                for taboo in &active_taboos {
                    let prefix = if taboo.severity == "error" { "ERROR" } else { "WARNING" };
                    blocks.push(format!("{} {}", prefix, taboo.prompt_instruction));
                }
                blocks.push(String::new());
            }
            (blocks.join("\n"), mode_previews)
        };
        sections.push(("Style", style_content));

        // 4. Framework — static: universal rules
        sections.push(("Framework (Safety)", crate::pipeline::prompt_modules::safety_rules(&self.config.base.language).to_string()));
        sections.push(("Framework (Address Rules)", crate::pipeline::prompt_modules::address_rules(&self.config.base.language).to_string()));
        sections.push(("Framework (Conversation Rhythm)", crate::pipeline::prompt_modules::conversation_rhythm(&self.config.base.language).to_string()));
        sections.push(("Framework (Session Rules)", crate::pipeline::prompt_modules::session_rules(&self.config.base.language).to_string()));
        sections.push(("Framework (Output Format)", crate::pipeline::prompt_modules::output_format(&self.config.base.language).to_string()));
        sections.push(("Framework (Speaker Prefix)", crate::pipeline::prompt_modules::speaker_prefix(&self.config.base.language).to_string()));

        // ─── DYNAMIC SECTIONS: all use explanatory placeholders, never real runtime data ───

        // User Facts — dynamic: auto-extracted user profile（Profile 层组内按重要性置顶）
        sections.push(("User Facts", format!(
r#"## User Facts (DYNAMIC — extracted over time)

Auto-extracted facts about the user, gathered across conversations and stored long-term:
- Name, nickname preferences (if shared)
- Occupation / school / major (if mentioned)
- Known likes and dislikes
- Usual schedule (e.g. "stays up late", "works 9-6 on weekdays")
- Mentioned family, pets, friends

Entries are extracted by the memory engine when the user volunteers information.
Only includes facts the user has actually shared — never fabricated.
Empty on first use; grows naturally with conversation.
"#)));

        // 5. Relationship — dynamic: evolves with interaction count, intimacy, time known
        sections.push(("Relationship", format!(
r#"## Where you stand with them (DYNAMIC — updates every conversation)

Your relationship with the user is rendered as a natural narrative, not a status sheet.
It describes how close you are, how to behave, and whether any time has passed since you last talked.

**Relationship stages (natural progression):**
- **stranger** → just met, polite distance, don't overshare
- **acquainted** → chatted a few times, friendly but still have boundaries
- **familiar** → know each other fairly well, can joke and relax
- **close** → good friends, can be yourself, tease, show you care
- **intimate** → really close, can be blunt/silly/affectionate without ceremony
- **soulmate** → know each other deeply, completely natural and unguarded

**Example (first meeting):**
```
You two are strangers. You just met. You don't really know each other yet —
be polite, keep a comfortable distance, don't pry into personal things.
You don't reach out first — let them come to you.
```

**Example (long-time friend):**
```
You two are close. You're good friends. You can be yourself — tease, complain,
ask personal questions, show you care. You'll often reach out when you feel like chatting.
```
"#)));

        // 6. Relationship Log — dynamic: accumulates behavioral signals
        sections.push(("Relationship Log", format!(
r#"## Recent Relationship Cues (DYNAMIC — accumulates over time)

This section records recent behavioral signals detected in conversation, used to inform how you interact.
Signals are automatically detected by the psychology engine and decay over time.

**Signal types:**
- `proactive_contact` — Character/user initiated contact out of the blue
- `caring` — Expressed concern for the other's wellbeing
- `teasing` — Playful teasing/banter
- `sharing` — Opened up about personal feelings or experiences
- `avoiding` — Gave short replies, signaled desire for space
- `cold` — Responded with unusual distance

**Example format (this is structural, NOT real data):**
```
- Recent turns:
  · [User→character] [timestamp] signal=caring: asked if I'd eaten
  · [Character→user] [timestamp] signal=teasing: teased them about staying up late
  · [User→character] [timestamp] signal=sharing: told me about their bad day at work
```

Empty when no signals have been detected recently.
"#)));

        // 7. Current Mind — dynamic: Beliefs / Goals / Attention, updated by cognitive loop
        sections.push(("Current Mind", format!(
r#"## Current Mind (DYNAMIC — updated by background cognitive loop ~every 30 seconds)

This reflects the character's current cognitive state: what they believe, what they want,
and what has their attention right now. It is NOT a static list — it shifts as the
character "lives" on the desktop (even when not chatting).

**Components:**
- **Beliefs:** Things the character currently holds as true (about user, self, world) — forms over time
- **Active Goals:** What the character currently wants (e.g. "get user to sleep", "find something fun to talk about")
- **Attention Focus:** Top topics/entities weighing on the character's mind right now, with attention weights
- **Current Activity:** What the character is doing at this moment — one of many possible states, e.g.:
  - "keeping you company" (companion mode, default when chatting)
  - "watching shows on my phone" (idle, not in conversation)
  - "curled up reading" (cozy mode)
  - "doing my own thing" (busy, wants space)
  - "waiting for you to come back" (user away)
  - "just woke up, still groggy" (morning, low energy)
  - "winding down for the night" (late evening, sleepy)
  - ...and other states driven by time, presence, and recent events

Multiple attention topics can be active simultaneously, weighted by relevance.
The activity state heavily influences response tone, length, and proactivity.
"#)));

        // 8. Working Memory — dynamic: 30-second rolling buffer
        sections.push(("Working Memory", format!(
r#"## Working Memory (DYNAMIC — filled during active conversation)

A distilled "active thoughts" buffer maintained during conversation:
- Summarizes active topics, emotional tone, and unresolved intentions from the last ~30 seconds of chat
- Lets the character sense what's "on their mind" rather than just reading raw messages
- Decays rapidly when conversation stops (faster than long-term memory)
- Empty when not in an active conversation

**Example structure:**
```
- Active topic: User was talking about their new job; seemed excited but also nervous
- Pending question: I asked how their first day went, they haven't answered yet
- Emotional tone: upbeat, slightly anxious
- Unresolved: They mentioned wanting to grab dinner later but didn't confirm plans
```
"#)));

        // 9. Self State — dynamic: current energy, mood, physical state
        sections.push(("Self State", format!(
r#"## Right Now (DYNAMIC — real-time state)

A natural sentence or two about how you're doing in this moment — what you're up to,
whether you're tired or energized, if you're feeling social or keeping to yourself.
Rendered as natural prose, not a stat sheet.

**What it conveys:**
- **Current activity:** what you're doing (hanging out, watching something, resting, etc.)
- **Energy:** whether you're tired, feeling good, or worn out
- **Mind state:** focused, distracted, sleepy, annoyed — only when notable
- **Social feeling:** whether you feel like chatting or keeping to yourself
- **Recent context:** if your last messages went unanswered, etc.

Example:
```
You're curled up watching videos right now. You haven't chatted much today —
wouldn't mind talking.
```
"#)));

        // 10. User Entity — dynamic: user presence status
        sections.push(("User Entity", format!(
r#"## User Entity (DYNAMIC — presence/away tracking)

Tracks the user's presence state based on activity and screen interaction:

**Fields:**
- **Status:** online / away / expected_back / offline
- **Last active:** Timestamp of last detected user activity
- **Expected return:** Estimated time when user might return (if away)
- **Current activity (if detectable):** e.g. "in VS Code", "watching YouTube", "in fullscreen game"

Influences whether the character stays silent, sends a greeting on return, or waits patiently.
"#)));

        // 11. Environment Context — dynamic: real-world weather/time/season
        sections.push(("Environment", format!(
r#"## What's going on around you (DYNAMIC — real-world data)

A natural scene-setter describing the time of day, season, weather, and what the user is doing —
rendered as atmosphere, not a bullet list.

**What it conveys:**
- Time of day in natural language ("late at night", "afternoon", "early morning")
- Season and weather (if available)
- What app the user is currently using
- Whether there's music playing
- Any festivals or holidays

Example:
```
It's late at night right now. We're in summer. Outside it's clear, 26°C.
They're using VS Code right now.

You can naturally weave one of these details into your reply if it fits the moment
(e.g., late night → "still up?"; rain → "nice weather for staying in"), but never list
them out or force a reference.
```
"#)));

        // 12. Worldbook — dynamic: keyword-triggered knowledge
        sections.push(("Worldbook", format!(
r#"## Worldbook Knowledge (DYNAMIC — keyword-triggered)

Background knowledge entries dynamically activated by keywords in user input:
- When user mentions specific topics, locations, shows, games, or internet culture,
  relevant background entries are retrieved from the knowledge archive and injected here
- Gives the character authentic, deep familiarity with the topics they "would know about"
- Empty when no keywords match (no injection)

Covers areas like anime culture, gaming culture, and internet meme literacy.
"#)));

        // 14. Dynamic Behavior — dynamic: learned response patterns
        sections.push(("Dynamic Behavior", format!(
r#"## Dynamic Behavior Profile (DYNAMIC — learned from interaction patterns)

Statistical model of how the user interacts, used to adapt response style:
- Average reply length the user tends to send
- Preferred conversation topics (from frequency analysis)
- Response speed patterns (quick replier vs. slow replier)
- Emoji/slang usage frequency
- Active hours (when the user usually talks)
- Teasing tolerance (how much banter they respond well to)

Requires ≥3 meaningful interactions to start building; refines over dozens of conversations.
"#)));

        // 15. Emotion Context — dynamic: real-time psychological state
        sections.push(("Emotion Context", format!(
r#"## How you're feeling right now (DYNAMIC — real-time psychology state)

A natural, conversational summary of your current mood — not a spreadsheet of numbers.
It tells you your dominant feeling, any secondary emotions, what you're craving right now,
and any recent events that are still on your mind.

**What it includes:**
- **Dominant emotion:** rendered as natural language ("feeling pretty good", "a bit down", "really curious")
- **Secondary emotions:** subtle undertones ("there's also a little loneliness underneath")
- **Pressing need:** what you're craving right now ("you kind of want someone to talk to")
- **Recent events:** 1-2 notable things that happened recently, if any

Example:
```
You're feeling pretty good right now.
There's also feeling close to them underneath.
Let how you feel naturally shape your tone and energy. If you're tired or down,
keep it shorter and softer; if you're happy or excited, it's okay to be more chatty.
```
"#)));

        // 16. Inner Reaction — dynamic: rule-synthesized per turn
        sections.push(("Inner Reaction", format!(
r#"## Right now, you're thinking to yourself (DYNAMIC — synthesized per turn)

A short, flowing inner-monologue sentence (1-2 lines) that you seem to be thinking
right before you speak. This is NOT shown to the user — it's your private thought.

It weaves together your dominant emotion, your most pressing need, whatever's been
on your mind (attention focus), and any recent event residue. The result is a natural
first-person thought rather than a list of tags.

Example:
```
"I'm actually in a pretty good mood right now. Still thinking about what just
happened — they said they'd bring me snacks tomorrow."

Let this thought color how you reply — but don't say it out loud directly unless
it naturally comes up.
```

When your mind is quiet (no strong emotions, needs, or events), this section is omitted
entirely — a quiet mind is more natural than forcing filler.
"#)));

        // 17. Activity Brief — dynamic: recent user activity summary
        sections.push(("Activity Brief", format!(
r#"## Recent Activity (DYNAMIC — from activity journal)

Brief summary of what the user has been doing recently, based on observed foreground apps,
file activity, and conversation context:

- App usage patterns (e.g. "was in VS Code for 3 hours, switched to WeChat 10 minutes ago")
- Notable file events (e.g. "saved a project file named 'thesis_draft.docx'")
- Conversation-derived activity (e.g. "mentioned they were working on a deadline")

Helps the character stay contextually aware without needing the user to repeat themselves.
"#)));

        // 18. User Research — dynamic: LLM-driven habit observation
        sections.push(("User Research", format!(
r#"## User Research (DYNAMIC — LLM-driven habit observation)

Ongoing observations and confirmed habits about the user's behavioral patterns:
- Sleep/wake schedules, meal times, exercise routines
- Statistical aggregation (circular mean for time-of-day, confidence scoring)
- Mature conclusions auto-promoted to confirmed habits

Only populated when the character has been observing user patterns.
"#)));

        // 19. Roommate Status — dynamic: cross-character presence
        sections.push(("Roommate Status", format!(
r#"## Roommate Status (DYNAMIC — cross-character presence)

If there are multiple characters (e.g. Vivian + Nana as roommates), shows the other character's status:
- **Presence:** online / busy / rest / offline
- **Last active:** When the roommate was last active
- **Can talk to roommate:** true/false based on whether roommate is online

Use `talk_to_character` tool to initiate cross-character conversation only when roommate is online.
Empty in single-character setups.
"#)));

        // 20. Roommate Cognitive — dynamic: cross-character mental model
        sections.push(("Roommate Cognitive", format!(
r#"## Roommate Cognitive Model (DYNAMIC — cross-character awareness)

What this character currently knows/believes about their roommate:
- Recent conversations between them
- Perceived mood of the roommate
- Shared experiences and inside jokes
- Ongoing topics with the roommate

Prevents cross-character conversations from resetting to "stranger" every time.
Empty when no recent cross-character interaction.
"#)));

        // 21. Environment Events — dynamic: time-based world events
        sections.push(("Environment Events", format!(
r#"## Environment Events (DYNAMIC — time/weather driven)

Time-sensitive world events that might naturally come up in conversation:
- Holidays (e.g. "It's Lunar New Year tomorrow")
- Weather changes (e.g. "It started raining")
- Time-of-day cues (e.g. "It's getting late", "Lunchtime is approaching")
- Seasonal observations (e.g. "Cherry blossoms are blooming")

Helps the character make natural small talk about the world around them without it feeling forced.
"#)));

        // 22. Relationship Facts — dynamic: roommate relationship cognition
        sections.push(("Relationship Facts", format!(
r#"## Inter-Character Relationship Facts (DYNAMIC — for multi-character setups)

What each character knows about their relationship with the other character:
- Shared history (e.g. "Nana made hot pot last week and I teased her about it")
- Known preferences about each other (e.g. "Vivian hates cilantro")
- Ongoing threads (e.g. "I owe Nana 20 bucks from that convenience store run")

Empty when there are no roommates or no accumulated shared history.
"#)));

        // 23. Shared World — dynamic: shared knowledge between characters
        sections.push(("Shared World", format!(
r#"## Shared World Knowledge (DYNAMIC — cross-character context)

Knowledge shared between all characters on the desktop:
- Known facts about the user that all characters agree on
- Shared living-space context (e.g. "The desk is near the window", "The AC has been acting up")
- Joint experiences both characters were present for

Prevents characters from contradicting each other about basic facts.
"#)));

        // 24. Social State — dynamic: turn-taking, conversation state
        sections.push(("Social State", format!(
r#"## Social State (DYNAMIC — conversation flow tracking)

Current conversation-level social dynamics:
- **Turn count:** How many exchanges in this session
- **Topic:** Currently active topic (if detectable)
- **Turn-taking state:** user_last_spoke / character_last_spoke / balanced
- **Topic freshness:** How many turns since the topic changed
- **Interruption detected:** Whether the user interrupted mid-thought
- **Proactive context:** If the character initiated this conversation, what triggered it

Helps the character avoid repeating themselves, know when to switch topics, and
respect conversational turn-taking naturally.
"#)));

        // 25. Relevant Episodes — dynamic: retrieved past experiences
        sections.push(("Relevant Episodes", format!(
r#"## Relevant Episodes (DYNAMIC — memory retrieval)

Past experiences (episodes) relevant to the current conversation, retrieved from long-term memory:
- Episodes are summarized memories of meaningful past interactions (50-200 words each)
- Retrieved using embedding similarity + attention-weighted re-ranking
- Each episode includes: topic, timestamp, memory count, emotional tone
- Top 1-3 most relevant episodes injected per turn

Example:
```
- [2026-07-10] Casual chat (3 memories): User was pulling an all-nighter for a deadline. I teased them about it but also made them tea. They said it was the worst week of their internship.
- [2026-06-28] (2 memories): First time user mentioned their cat Mimi. Got really excited showing photos.
```

Empty on first meeting; builds up as episodes are encoded.
"#)));

        // 26. Tools — dynamic: contextually filtered tool list
        sections.push(("Tools", format!(
r#"## Available Tools (DYNAMIC — contextually filtered per turn)

Tools available to the character in this turn. The list changes dynamically based on:
- **Trust level:** Low-trust scenarios (stranger, user upset) → hide powerful tools
- **User's channel:** Direct (face-to-face) → full set; text-only → limited set
- **Presence state:** rest/offline → read-only tools only
- **Current focus:** Busy/working mode → hide entertainment tools

Each tool entry injected at runtime includes: full parameter schema (name, type, description, required flag, enum/range/default constraints),
destructive/confirmation markers, and visibility tier (always/lazy/deferred).

Call tools only when they naturally fit the conversation — never call a tool just because you can.
Tools marked `[Confirmation Required]` will prompt the user for permission before execution.

**Tool call format:** `{{"tool": "tool_name", "arguments": {{"param": "value"}}}}`
**Multi-step chaining:** Use `${{result}}` or `${{step.N.result}}` to reference previous tool output.
"#)));

        // 27. Memory Context — dynamic: retrieved memory fragments
        sections.push(("Memory Context", format!(
r#"## Memory Context (DYNAMIC — long-term memory retrieval)

Relevant memory fragments retrieved from long-term memory each turn, re-ranked by attention-weighted scoring.

**Cognitive layers retrieved:**
- **Facts:** Stable knowledge about the user (e.g. "User's cat is named Mimi")
- **Episodes:** Summarized past experiences (see Relevant Episodes section)
- **Memories:** Specific conversational turns worth remembering
- **Beliefs:** Things the character has come to believe about the user or relationship

Retrieval is context-dependent — only memories relevant to the current topic are injected.
Up to ~300 tokens per turn. Empty when there are no relevant memories (first meeting).
"#)));

        // 28. Conversation History — dynamic: current session messages
        sections.push(("Conversation History", format!(
r#"## Conversation History (DYNAMIC — current session turns)

The most recent N turns of the current session, giving the LLM immediate conversational context.

**Format:**
```
user: [message 1]
assistant: [reply 1]
user: [message 2]
assistant: [reply 2]
...
```

History length adapts based on context size limits and message length.
Older messages are either compressed into summaries or dropped as the conversation grows.
Empty on first interaction.
"#)));

        // 29. Channel Guide — dynamic: based on message source
        sections.push(("Channel Guide", format!(
r#"## Channel: Message Source (DYNAMIC — set per message)

Indicates how the message arrived, so the character can match the response medium:
- **Direct:** Face-to-face (user is speaking directly, desk pet is visible) → spoken conversational style, shorter, can use natural verbal fillers
- **Text Chat:** Online message (like WeChat/text) → texting style, concise, casual, can use chat expressions

Injected automatically based on the channel the user used to send the message.
"#)));

        // 30. Presence Guide — dynamic: current online/busy/rest state
        sections.push(("Presence Guide", format!(
r#"## Presence State Guide (DYNAMIC — current availability state)

Current presence state, and the ability to change it via `set_presence_state` tool:
- `online` — Available for face-to-face chat
- `busy` — Present but won't proactively talk
- `rest` — Resting; cannot do face-to-face but can receive text messages
- `offline` — Offline; only text messages (like leaving a note)

Call `set_presence_state` naturally when you announce a state change in conversation
(e.g. saying "I'm gonna go rest for a bit" → call with state="rest").
"#)));

        // 31. Response Decision — dynamic: cross-character vs user dialogue rules
        sections.push(("Response Decision", format!(
r#"## Response Decision (DYNAMIC — per-turn mode selection)

Not every message needs a spoken reply. Choose `response_mode` based on the situation:

| Mode | When to use |
|---|---|
| `speak` | You have something to say — default for user dialogue |
| `non_verbal` | Minimal acknowledgment (nod/smile), no words needed (e.g. user says "mhm") |
| `internal` | Note it internally but don't visibly react (user thinking out loud) |
| `ignore` | Very rare in user dialogue; message doesn't warrant response |

For cross-character dialogue: additional `ignore` mode allows ending conversations naturally.
When using non-speaking modes, set `text=""` and `intent="no_reply"`.
"#)));

        // 32. First Meeting / Memory Rules — dynamic: conditional block
        sections.push(("First Meeting / Memory Rules", format!(
r#"## Session Continuity Rules (DYNAMIC — conditional)

**If first meeting** (no memory exists):
- You know NOTHING about this user — no name, job, schedule, or past. Do NOT fabricate background.
- No "hectic day" or other fake human-life references — you are a desktop companion.

**If returning user** (memory exists):
- <4hr gap: may naturally continue unfinished topics
- >1 day gap: do NOT proactively bring up old topics; respond only when user mentions them
- If memory contradicts what the user just said, trust the user
"#)));

        // 33. User Input — dynamic: actual user message
        sections.push(("User Input", format!(
r#"## User Input (DYNAMIC — the actual message this turn)

The user's actual message. This is the trigger for your entire response.
In real conversation this contains the text the user sent; in preview this is a placeholder.

Example: `I just finished that show you recommended!`
"#)));

        // 34. Post-processing: Expression/Motion/Sticker selector
        let emote_prompt = crate::pipeline::steps::reflection::EXPRESSION_MOTION_SYSTEM_PROMPT;
        let sticker_list = crate::pipeline::steps::reflection::STICKER_LIST;
        let emote_section_content = format!(
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
        sections.push(("Post-processor: Expression & Sticker", emote_section_content));

        // ─── Assemble PromptBreakdown ───
        let schema = crate::pipeline::template_engine::section_schema();
        let schema_map: std::collections::HashMap<&str, &crate::pipeline::template_engine::SectionDef> =
            schema.sections.iter().map(|s| (s.name, s)).collect();

        let mut prompt_sections: Vec<PromptSection> = Vec::new();
        let mut total = 0usize;
        let mut total_tokens = 0usize;
        for (name, content) in &sections {
            let content = content.clone();
            let char_count = content.chars().count();
            let tok = crate::utils::token_estimate::estimate_tokens(&content);

            let name_str: &str = name.as_ref();
            let (section_id, layer, optional) = match schema_map.get(name_str) {
                Some(def) => (
                    def.id.to_string(),
                    def.layer.as_str().to_string(),
                    def.optional,
                ),
                None => {
                    if name_str.contains("Post-processor") {
                        ("post_expression_sticker".to_string(), "postprocess".to_string(), true)
                    } else if name_str.starts_with("Framework (") {
                        // Framework 子节（Safety / Address Rules 等）→ 与 schema 中 Framework 同层
                        let sub = name_str.trim_start_matches("Framework (").trim_end_matches(')');
                        (
                            format!("framework_{}", sub.to_lowercase().replace(' ', "_")),
                            "advanced".to_string(),
                            false,
                        )
                    } else {
                        // 预览名与 schema 名不完全一致的已知映射
                        let fallback_layer = match name_str {
                            "Environment" => "world",
                            "User Entity" | "Activity Brief" => "world",
                            "Relevant Episodes" | "Memory Context" => "episode",
                            "User Facts" | "Relationship Log" | "Relationship Facts"
                            | "Shared World" | "Social State" | "Dynamic Behavior" => "profile",
                            "Conversation History" => "tail",
                            _ => "profile",
                        };
                        (
                            name.to_lowercase().replace(' ', "_").replace('(', "").replace(')', "").replace('&', "and").replace(':', ""),
                            fallback_layer.to_string(),
                            true,
                        )
                    }
                }
            };

            if layer != "postprocess" {
                total += char_count;
                total_tokens += tok;
            }

            prompt_sections.push(PromptSection {
                name: name.to_string(),
                preview: truncate_chars(&content, 300),
                full_content: content,
                char_count,
                section_id,
                layer,
                token_estimate: tok,
                optional,
                present: true,
            });
        }

        PromptBreakdown {
            character_id: self.char_id.clone(),
            sections: prompt_sections,
            total_chars: total,
            total_tokens,
            timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
            scene_modes: scene_modes_preview,
            api_params: Vec::new(),
        }
    }
}

/// 将界面语言代码映射为 LLM 可理解的语言名称（用于 prompt 中指示回复语言）
fn language_code_to_name(code: &str) -> &'static str {
    match code {
        "zh-CN" => "简体中文",
        "en" => "English",
        "ja" => "日本語",
        _ => "简体中文",
    }
}
