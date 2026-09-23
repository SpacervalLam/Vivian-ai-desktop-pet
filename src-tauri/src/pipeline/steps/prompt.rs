//! Prompt 组装流水线步骤：模块化 prompt 构建。
//!
//! - [`PromptBuildingStep`]：注入 PersonaEngine / EmotionBridge / PsychologyManager
//!   关系上下文由 PsychologyManager 统一提供（原 RelationshipManager 已整合）

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::emotion::{EmotionBridge, EpistemicAssessment, KnowledgeDecision, ScheduleAssessment};
use crate::error::VivianResult;
use crate::memory::user_facts::UserFactStore;
use crate::memory::{MemoryManager, MemoryType};
use crate::persona::{DynamicBehaviorProfile, PersonaEngine};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::prompt_modules::{build_tools_block, output_format, section_heading, EnvironmentContext, PromptParts};
use crate::pipeline::state::PipelineState;
use crate::proactive::ActivityJournal;
use crate::psychology::PsychologyManager;
use crate::tools::registry::ToolSystem;
use crate::tools::tool_call_manager::ToolListTool;
use crate::tools::types::ToolScene;
use crate::utils::EnvironmentManager;

// 保留原有的 IDENTITY_BLOCK / OUTPUT_RULES 常量作为 fallback（不直接使用，
// 由 prompt_modules::IDENTITY_BLOCK 接管静态身份块；如需关闭模块化构建，
// 可在 PromptBuildingStep 中切换到这两个常量进行兜底）。

/// 兜底身份块（按 char_id + lang 选择，避免硬编码单一角色）
fn fallback_identity_block(char_id: &str, lang: &str) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    let (name, persona_zh) = match char_id {
        "nana" => ("Nana", "一个温柔但有力量的人，同时也是一只桌面宠物。你说话轻声细语但很稳。"),
        _ => ("Vivian", "一个温柔、活泼、有点小傲娇的桌面宠物。你生活在用户的桌面上，陪伴用户工作和生活。"),
    };
    match lang_norm {
        "en" => match char_id {
            "nana" => "You are Nana, a warm and gentle person who also lives as a desktop pet. You speak softly but with quiet strength.".to_string(),
            _ => "You are Vivian, a warm, lively, slightly tsundere desktop pet. You live on the user's desktop, keeping them company through work and life.".to_string(),
        },
        "ja" => match char_id {
            "nana" => format!("あなたは{}、優しくて力のある少女であり、同時にデスクトップペットでもある。穏やかに、しかし確かな言葉で話す。", name),
            _ => format!("あなたは{}、優しくて活発で少しツンデレなデスクトップペット。ユーザーのデスクトップで暮らし、仕事と生活を寄り添う。", name),
        },
        _ => format!("你是{}（{}），{}。请用自然、亲切的语气回复用户。", name, char_id, persona_zh),
    }
}

/// 兜底输出规则（规则类内容统一英文）
fn fallback_output_rules() -> &'static str {
    "Reply requirements: keep it concise and conversational. Don't use Markdown headings or lists — answer in a natural spoken tone."
}

/// 兜底段落标签（三语化）
fn fallback_label(id: &str, lang: &str) -> &'static str {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    match lang_norm {
        "en" => match id {
            "memory" => "Relevant memory:",
            "history" => "Conversation history:",
            "user" => "User:",
            _ => "",
        },
        "ja" => match id {
            "memory" => "関連記憶：",
            "history" => "会話履歴：",
            "user" => "ユーザー：",
            _ => "",
        },
        _ => match id {
            "memory" => "相关记忆：",
            "history" => "对话历史：",
            "user" => "用户：",
            _ => "",
        },
    }
}

// ============================================================================
// PromptBuildingStep：原有模块化 prompt 构建（保留）
// ============================================================================

#[derive(Clone)]
pub struct PromptBuildingStep {
    pub persona: Option<Arc<PersonaEngine>>,
    pub emotion_bridge: Option<Arc<EmotionBridge>>,
    /// 心理系统管理器：注入后提供五层心理架构 + 关系上下文
    pub psychology: Option<Arc<PsychologyManager>>,
    /// 工具系统：注入后按当前情绪/关系阶段筛选可用工具子集
    pub tool_system: Option<Arc<ToolSystem>>,
    /// 环境管理器：注入后读取前台应用，用于触发 Focus 场景的工具筛选
    pub environment: Option<Arc<EnvironmentManager>>,
    /// 用户事实画像：注入后在 prompt 中输出结构化用户档案
    pub user_facts: Option<Arc<UserFactStore>>,
    /// 智能体动态行为画像：注入后在 prompt 中输出近期交互模式
    pub dynamic_profile: Option<Arc<DynamicBehaviorProfile>>,
    /// 记忆系统管理器：注入后提供"近期重要事件"段落（查询 ImportantEvent 最近 5 条）
    pub memory: Option<Arc<MemoryManager>>,
    /// 世界状态提供者：注入后在 prompt 中输出天气/节气/节日等真实世界感知
    pub world_provider: Option<Arc<crate::world::WorldStateProvider>>,
    /// 世界状态核心：注入后在 prompt 中输出近期活动观察（异常检测）
    pub world_state: Option<Arc<crate::world::WorldState>>,
    /// 用户研究管理器：注入后在 prompt 中输出活跃研究课题和已确认习惯
    pub research: Option<Arc<crate::research::ResearchManager>>,
    /// 用户活动日志：注入后在 prompt 中输出近期前台窗口切换摘要（低权重背景参考）
    pub activity_journal: Option<Arc<ActivityJournal>>,
    /// Mind 认知聚合句柄：注入后输出 Belief / Goal / Attention 三合一段落
    pub mind: Option<Arc<crate::mind::Mind>>,
    /// Episode 经历存储：注入后输出最近 1-3 个 Episode 摘要（Relevant Episode 段）
    pub episode_store: Option<Arc<crate::memory::episode::EpisodeStore>>,
    /// 场景语气注入器：注入后每轮对话匹配用户输入场景，命中后注入参考台词
    pub tone_injector: Option<Arc<crate::persona::ToneInjector>>,
    /// 工具语义筛选器：注入后在 intent=tool_request/request 时对工具做语义粗筛，
    /// 将 Top-N 最相关工具作为"推荐工具"注入 prompt（不改变现有 visibility 分流）
    pub tool_semantic_filter: Option<Arc<crate::tools::ToolSemanticFilter>>,
    /// Topic 驱动背景知识注入器：扫描用户输入命中关键词后，在 prompt 中注入对应背景知识段落
    pub topic_injection: Option<Arc<crate::pipeline::topic_injection::TopicInjectionManager>>,
    /// 角色 ID（用于从 character_registry 查询当前角色的 ResourceManifest，注入表情/动作清单）
    pub char_id: String,
    /// 内联表情/动作标签功能是否启用（启用时在 prompt 中注入标签使用说明）
    pub inline_expression_enabled: bool,
    /// 可用表情/动作名称列表文本（从 ResourceManifest 提取，内联标签模式时注入 prompt）
    /// 格式："表情：happy, sad, ...\\n动作：wave, dance, ..."
    pub expression_motion_names: Option<String>,
    /// 是否启用原生 function calling（true 时 prompt 不注入工具列表，
    /// 工具描述通过 API 的 tools 参数传递）
    pub enable_native_fc: bool,
    /// 当前 provider 是否支持原生 JSON Schema 约束（true 时不注入 output_format prompt 文本）
    pub has_native_schema: bool,
    /// 主模型上下文窗口；任务路由未单独配置时使用。
    pub default_context_window: usize,
    /// task_type → 路由模型上下文窗口。
    pub task_context_windows: HashMap<String, usize>,
    /// 当前界面语言（zh-CN / en / ja），用于加载对应语言的 framework 段落
    pub language: String,
}

/// 语义召回的最小相似度阈值
///
/// 文本通道与原生 FC 通道统一后，这是唯一的召回阈值。它比"推荐提示"惯用的
/// 0.30 更宽松：召回结果直接决定谁能拿到完整 schema，漏召回的代价是模型要多调
/// 一次 `tool_search` 才能拿到 schema，误召回的代价只是多几个工具的 schema，
/// 两者不对称，所以阈值向下取。
const TOOL_RECALL_MIN_SIM: f32 = 0.22;

/// 各场景下语义召回的工具数量上限
///
/// 闲聊场景工具需求稀疏，给少；任务场景用户明确在做事，给足，
/// 避免因召回不全打断操作流程。
fn semantic_recall_top_n(scene: ToolScene) -> usize {
    match scene {
        ToolScene::Chat | ToolScene::Idle | ToolScene::LowTrust => 4,
        ToolScene::Focus => 6,
        ToolScene::Default => 8,
        ToolScene::Task => 10,
    }
}

/// 两条工具注入通道共用的工具范围（单一真相源）。
///
/// 由 `ainvoke` 每轮计算一次，同时喂给文本通道（`build_parts` 的 Markdown 工具
/// 列表 + 推荐提示）与原生 FC 通道（`state.tool_definitions`），保证两条路用完全
/// 相同的场景判定与语义召回集，杜绝"各算一遍、参数不一致、推荐段与主列表重复注入"。
pub(crate) struct ToolScope {
    /// 当前场景（关系阶段 + 情绪 + 前台应用 + 用户输入 + 近期工具使用推断）
    scene: ToolScene,
    /// 动态隐藏集（条件不足的工具，对 LLM 完全不可见）
    hidden: HashSet<String>,
    /// 供可见性判定：完整 schema = 保底集 ∪ 召回集；嵌入不可用时为 None（回退纯场景可见性）
    recalled: Option<HashSet<String>>,
    /// 召回工具名，按相似度降序，供"仅名称一行"推荐提示
    recalled_order: Vec<String>,
}

impl PromptBuildingStep {
    pub fn new() -> Self {
        Self {
            persona: None,
            emotion_bridge: None,
            psychology: None,
            tool_system: None,
            environment: None,
            user_facts: None,
            dynamic_profile: None,
            memory: None,
            world_provider: None,
            world_state: None,
            research: None,
            activity_journal: None,
            mind: None,
            episode_store: None,
            tone_injector: None,
            tool_semantic_filter: None,
            topic_injection: None,
            char_id: String::new(),
            inline_expression_enabled: false,
            expression_motion_names: None,
            enable_native_fc: false,
            has_native_schema: false,
            default_context_window: 131_072,
            task_context_windows: HashMap::new(),
            language: String::from("zh-CN"),
        }
    }

    pub fn with_engines(
        persona: Arc<PersonaEngine>,
        emotion_bridge: Arc<EmotionBridge>,
    ) -> Self {
        Self {
            persona: Some(persona),
            emotion_bridge: Some(emotion_bridge),
            psychology: None,
            tool_system: None,
            environment: None,
            user_facts: None,
            dynamic_profile: None,
            memory: None,
            world_provider: None,
            world_state: None,
            research: None,
            activity_journal: None,
            mind: None,
            episode_store: None,
            tone_injector: None,
            tool_semantic_filter: None,
            topic_injection: None,
            char_id: String::new(),
            inline_expression_enabled: false,
            expression_motion_names: None,
            enable_native_fc: false,
            has_native_schema: false,
            default_context_window: 131_072,
            task_context_windows: HashMap::new(),
            language: String::from("zh-CN"),
        }
    }

    /// 注入角色 ID，启用按角色查询 ResourceManifest（表情/动作清单注入）
    pub fn with_char_id(mut self, char_id: impl Into<String>) -> Self {
        self.char_id = char_id.into();
        self
    }

    /// 注入界面语言，用于加载对应语言的 framework 段落
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }

    /// 注入模型窗口配置，供每轮按 task_type 计算提示词预算。
    pub fn with_prompt_budget_config(mut self, config: &crate::config::manager::AppConfig) -> Self {
        self.default_context_window = config.ai.context_window
            .unwrap_or_else(|| crate::providers::capabilities::default_context_window(&config.ai.model)) as usize;
        self.task_context_windows = if config.enable_routing_matrix {
            config.routing_matrix.iter().filter_map(|(task_type, route)| {
                if route.model.trim().is_empty() && route.context_window.is_none() {
                    return None;
                }
                let window = route.context_window
                    .unwrap_or_else(|| crate::providers::capabilities::default_context_window(&route.model));
                Some((task_type.clone(), window as usize))
            }).collect()
        } else {
            HashMap::new()
        };
        self
    }

    /// 注入内联表情/动作标签配置（启用时在 prompt 中注入标签使用说明）
    pub fn with_inline_expression(mut self, enabled: bool, names: Option<String>) -> Self {
        self.inline_expression_enabled = enabled;
        self.expression_motion_names = names;
        self
    }

    /// 注入工具语义筛选器（启用后在 intent=tool_request/request 时对工具做语义粗筛）
    pub fn with_tool_semantic_filter(
        mut self,
        filter: Arc<crate::tools::ToolSemanticFilter>,
    ) -> Self {
        self.tool_semantic_filter = Some(filter);
        self
    }

    /// 配置原生 function calling 开关（启用时跳过工具列表 prompt 注入）
    pub fn with_native_fc(mut self, enabled: bool) -> Self {
        self.enable_native_fc = enabled;
        self
    }

    /// 配置原生 JSON Schema 支持标志（启用时跳过 output_format prompt 文本注入）
    pub fn with_native_schema(mut self, enabled: bool) -> Self {
        self.has_native_schema = enabled;
        self
    }

    /// 注入 PsychologyManager，启用五层心理架构上下文注入
    pub fn with_psychology(mut self, psychology: Arc<PsychologyManager>) -> Self {
        self.psychology = Some(psychology);
        self
    }

    /// 注入 ToolSystem，启用场景化工具筛选（低信任/情绪困扰/专注场景下自动隐藏部分工具）
    pub fn with_tool_system(mut self, tool_system: Arc<ToolSystem>) -> Self {
        self.tool_system = Some(tool_system);
        self
    }

    /// 注入 EnvironmentManager，启用 Focus 场景触发（检测工作类应用时筛选娱乐工具）
    pub fn with_environment(mut self, environment: Arc<EnvironmentManager>) -> Self {
        self.environment = Some(environment);
        self
    }

    /// 注入 UserFactStore，启用用户事实画像段落注入
    pub fn with_user_facts(mut self, user_facts: Arc<UserFactStore>) -> Self {
        self.user_facts = Some(user_facts);
        self
    }

    /// 注入 DynamicBehaviorProfile，启用智能体动态行为画像段落注入
    pub fn with_dynamic_profile(mut self, profile: Arc<DynamicBehaviorProfile>) -> Self {
        self.dynamic_profile = Some(profile);
        self
    }

    /// 注入 MemoryManager，启用"近期重要事件"段落（查询 ImportantEvent 最近 5 条）
    pub fn with_memory(mut self, memory: Arc<MemoryManager>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// 注入 WorldStateProvider，启用真实世界感知注入（天气/节气/节日/日出日落）
    pub fn with_world(mut self, world: Arc<crate::world::WorldStateProvider>) -> Self {
        self.world_provider = Some(world);
        self
    }

    /// 注入 WorldState，启用活动观察注入（异常检测：洗澡只用了 5 分钟等）
    pub fn with_world_state(mut self, world_state: Arc<crate::world::WorldState>) -> Self {
        self.world_state = Some(world_state);
        self
    }

    /// 注入 ResearchManager，启用用户研究段落注入（活跃课题 + 已确认习惯）
    pub fn with_research(mut self, research: Arc<crate::research::ResearchManager>) -> Self {
        self.research = Some(research);
        self
    }

    /// 注入 ActivityJournal，启用用户近期活动摘要注入（低权重背景参考，只读不清空）
    pub fn with_activity_journal(mut self, journal: Arc<ActivityJournal>) -> Self {
        self.activity_journal = Some(journal);
        self
    }

    /// 注入 Mind，启用 Belief / Goal / Attention 三合一认知段落注入
    pub fn with_mind(mut self, mind: Arc<crate::mind::Mind>) -> Self {
        self.mind = Some(mind);
        self
    }

    /// 注入 EpisodeStore，启用 Relevant Episode 段落（最近 1-3 个经历摘要）
    pub fn with_episode_store(mut self, store: Arc<crate::memory::episode::EpisodeStore>) -> Self {
        self.episode_store = Some(store);
        self
    }

    /// 注入 ToneInjector，启用场景语气注入（每轮对话匹配用户输入场景，命中后注入参考台词）
    pub fn with_tone_injector(mut self, injector: Arc<crate::persona::ToneInjector>) -> Self {
        self.tone_injector = Some(injector);
        self
    }

    /// 注入 TopicInjectionManager，启用话题驱动背景知识注入
    pub fn with_topic_injection(
        mut self,
        manager: Arc<crate::pipeline::topic_injection::TopicInjectionManager>,
    ) -> Self {
        self.topic_injection = Some(manager);
        self
    }

    /// 判定本轮的工具场景与隐藏集，两条路径共用
    ///
    /// 文本路径（prompt 里的 Markdown 工具列表）与原生 FC 路径（API 的 tools 字段）
    /// 必须拿到同一份结果。此前两处各算一遍，且 hidden 内容不同——
    /// FC 路径额外带了"强相关记忆命中时抑制 web_search"，
    /// 于是两条路径的工具集不一致：原生 FC 失败回退到文本路径时
    /// web_search 会突然重新出现，行为平白突变。
    fn resolve_tool_scope(
        &self,
        ts: &ToolSystem,
        state: &PipelineState,
    ) -> (ToolScene, HashSet<String>) {
        let active_app = self
            .environment
            .as_ref()
            .map(|env| env.get_environment_info().current_window.clone());
        let emotion = self
            .emotion_bridge
            .as_ref()
            .map(|eb| eb.get_current_emotion().emotion);
        let stage = self
            .psychology
            .as_ref()
            .map(|psy| psy.get_stage().as_str().to_string());
        // 近 5 分钟内调过工具 → has_recent_tool_use=true（驱动 Task 场景判定）
        let has_recent_tool_use = ts.has_recent_tool_call(300);
        let scene = ToolScene::from_full_context(
            stage.as_deref(),
            emotion.as_deref(),
            active_app.as_deref(),
            &state.user_input,
            has_recent_tool_use,
        );

        // 动态隐藏不满足条件的工具（如 write_diary 在条件不足时对 LLM 不可见）
        let mut hidden = HashSet::new();
        if !crate::diary::is_tool_available(&self.char_id) {
            hidden.insert("write_diary".to_string());
        }
        // 跨角色对话模式下隐藏 talk_to_character：目标角色由源角色的 talk_to_character 唤起，
        // 若目标角色再调用此工具回复源角色，会因源角色持 think_lock 等待工具返回而形成死锁
        if state.current_channel == "cross_character" {
            hidden.insert("talk_to_character".to_string());
        }
        // 强相关记忆命中时抑制 web_search：本轮记忆区块已注入"优先用记忆回答"
        // 引导（MemoryRetrievalStep 写入 memory_strong_hit），对外隐藏 web_search，
        // 避免工作模型为了"查询"去外部搜索角色本该记得的自身/用户相关内容。
        let memory_strong_hit = state
            .metadata
            .get("memory_strong_hit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if memory_strong_hit {
            hidden.insert("web_search".to_string());
        }

        (scene, hidden)
    }

    /// 计算本轮的工具范围（场景 + 隐藏集 + 语义召回），两条注入通道共享同一份结果。
    ///
    /// 语义召回只在此处算一次：召回的工具名同时写入 `recalled`（供可见性判定，
    /// 完整 schema = 保底集 ∪ 召回集）与 `recalled_order`（按相似度降序，供"仅名称
    /// 一行"推荐提示）。取不到嵌入（主动开场 / 感知被跳过 / 嵌入服务不可用）时
    /// `recalled=None`，回退纯场景可见性，不丢能力。
    fn compute_tool_scope(&self, ts: &ToolSystem, state: &PipelineState) -> ToolScope {
        let (scene, hidden) = self.resolve_tool_scope(ts, state);

        let recalled_order: Vec<String> = self
            .tool_semantic_filter
            .as_ref()
            .zip(state.fast_perception.as_ref())
            .map(|(filter, fp)| {
                let emb = fp.query_embedding.as_slice();
                if emb.is_empty() {
                    return Vec::new();
                }
                filter
                    .filter(ts, emb, semantic_recall_top_n(scene), TOOL_RECALL_MIN_SIM)
                    .into_iter()
                    .map(|r| r.name)
                    .collect()
            })
            .unwrap_or_default();

        let recalled = if recalled_order.is_empty() {
            None
        } else {
            Some(recalled_order.iter().cloned().collect::<HashSet<_>>())
        };

        ToolScope {
            scene,
            hidden,
            recalled,
            recalled_order,
        }
    }

    /// 将 PipelineState 转换为 PromptParts，注入动态人设/关系/情绪/心理上下文
    ///
    /// `tool_scope` 由 `ainvoke` 计算一次后传入（文本 + 原生 FC 两通道共享）：
    /// 工具列表与推荐提示都基于它渲染。主动开场等旁路调用传 `None`，此时不注入
    /// 工具段（旁路本就不需要工具调用）。
    pub(crate) fn build_parts(
        &self,
        state: &PipelineState,
        tool_scope: Option<&ToolScope>,
    ) -> PromptParts {
        // Character 块（身份+人格+背景+兴趣+外观+说话风格+关系）/ 风格约束（场景 + 禁忌）/ Few-shot 示例
        // Character 块按关系熟悉度分档：熟客（stage>=2）裁掉自我介绍型段落（背景/兴趣/外观）
        let (character_block, examples_block, style_block, style_preset_block) = match self.persona.as_ref() {
            Some(p) => {
                let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
                let stage = self
                    .psychology
                    .as_ref()
                    .map(|psy| psy.relationship().stage())
                    .unwrap_or(0);
                let tier = crate::persona::prompt_render::CharacterBlockTier::from_relationship_stage(stage);
                let intimacy = self
                    .psychology
                    .as_ref()
                    .map(|psy| psy.relationship().intimacy * 100.0)
                    .unwrap_or(0.0);
                let style = p.build_style_prompt(intimacy, hour);
                let cfg = p.get_config();
                let preset = crate::persona::prompt_render::render_style_preset_block(&cfg, &self.language);
                (Some(p.get_character_block_tiered(tier)), Some(p.get_examples_block()), Some(style), if preset.is_empty() { None } else { Some(preset) })
            }
            None => (None, None, None, None),
        };

        // 角色长期记忆笔记（memory.md）：角色在 appdata 自行维护的跨会话沉淀。
        // 每轮重读，memory_md 工具修改后下一轮即时生效；文件未变时 prompt 字节一致，
        // 不影响缓存前缀。char_id 为空（未注册角色）时跳过。
        let memory_md_section = if !self.char_id.is_empty() {
            crate::memory::memory_md::read_memory_md(&self.char_id)
        } else {
            None
        };

        // 关系段落：当前亲密度 + 阶段 + 策略（由 PsychologyManager 提供）
        let relationship_section = self
            .psychology
            .as_ref()
            .map(|psy| psy.relationship_section(&self.language));

        // 关系日志近期线索：逐轮关系信号 + 每日摘要（由 RelationshipLogEngine 提供）
        let relationship_log_section = {
            let log = crate::psychology::relationship_log();
            let text = log.build_context(5, 3, &self.language);
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        };

        // 用户事实画像段落：name/age/gender/occupation/location + 自由事实
        // 空档案时返回 None（避免空段落污染 prompt）
        let user_facts_section = self.user_facts.as_ref().and_then(|store| {
            let text = store.format_for_prompt();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        });

        // 智能体动态行为画像段落：近期话题/情绪/消息长度等交互模式
        // 数据不足（< 3 轮）时返回 None
        let dynamic_behavior_section = self.dynamic_profile.as_ref().and_then(|profile| {
            let text = profile.format_for_prompt();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        });

        // 心理架构上下文：五层心理状态 + 系统规则
        // PsychologyManager 是情绪/心理上下文的唯一来源；未注入时为 None
        // "近期重要事件"段落由 MemoryManager 查询 ImportantEvent 最近 5 条提供
        let recent_events_desc = self.memory.as_ref().map_or(String::new(), |mem| {
            let items = mem.recent_by_type(MemoryType::ImportantEvent, 5);
            if items.is_empty() {
                "无".to_string()
            } else {
                items
                    .iter()
                    .map(|m| format!("- {}（重要性:{:.0}%）", m.content, m.importance * 100.0))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        });
        let emotion_context = self
            .psychology
            .as_ref()
            .map(|psy| psy.build_psychology_prompt(&recent_events_desc, &self.language));

        // 近期自我发言清单：抽自己最近说过的话，配反重复指令注入。
        //
        // 对话历史虽然已在 messages 里，但那只是"摆在那"——模型会自然延续语气，
        // 也会自然滑进套话。真人聊上十几轮后不会还在用同一个句式接话，
        // 而缺少显式提醒时模型很容易这么做，尤其是短回复场景
        // （"嗯""好""这样啊" 会一轮轮复制下去）。
        //
        // 开场路径另有一套专用机制（基于 startup_greeting 标签），
        // 这里覆盖的是开场之外的全部对话。
        let recent_self_utterances = build_recent_self_utterances(
            &state.messages,
            &self.char_id,
            state.current_channel.as_str(),
            &self.language,
        );

        // 场景感知工具列表：场景/隐藏集/语义召回都取自 ainvoke 传入的共享 ToolScope，
        // 与原生 FC 通道用完全相同的一份结果（见 compute_tool_scope）。
        // 可见性 = 保底集 ∪ 召回集拿完整 schema，其余降级为名称(+描述)，靠 tool_search 取回。
        let active_app = self
            .environment
            .as_ref()
            .map(|env| env.get_environment_info().current_window);
        let tools = self
            .tool_system
            .as_ref()
            .zip(tool_scope)
            .map(|(ts, scope)| {
                ToolListTool::new(Arc::clone(ts)).get_tools_for_ai_with_scene(
                    scope.scene,
                    &scope.hidden,
                    scope.recalled.as_ref(),
                    &self.language,
                )
            });

        // 同步填充 EnvironmentContext.active_app（让 LLM 也能看到当前前台应用）
        // 若注入了 WorldStateProvider，则同时注入天气/节气/节日等真实世界感知
        let mut env_ctx = if let Some(wp) = self.world_provider.as_ref() {
            let snap = wp.snapshot(None);
            EnvironmentContext::now().with_world(&snap)
        } else {
            EnvironmentContext::now()
        };
        if let Some(app) = &active_app {
            if !app.is_empty() {
                env_ctx.active_app = app.clone();
            }
        }

        // Worldbook 背景知识：动态激活 — 先更新状态，再渲染
        let worldbook_block = {
            let last_assistant = state.messages.iter().rev()
                .find(|m| m.role == "assistant")
                .map(|m| m.content.as_str());
            crate::persona::worldbook::update_activation(&state.user_input, last_assistant);
            let block = crate::persona::worldbook::render_worldbook_block(&state.user_input, &self.language);
            if block.is_empty() { None } else { Some(block) }
        };

        // 用户近期活动摘要（只读 to_brief，不 drain，避免影响内心独白消费）
        let activity_brief = self
            .activity_journal
            .as_ref()
            .map(|j| j.to_brief())
            .filter(|s| !s.trim().is_empty());

        // 用户研究：活跃观察课题 + 已确认的行为习惯
        let user_research = self.research.as_ref().and_then(|r| r.build_prompt_section(&self.language));

        // 可用技能：从全局 ctx 取 SkillService，列出当前角色可见的技能（内置风格 + 目录 *.md）
        let skill_section = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
            .filter(|_| !self.char_id.is_empty())
            .map(|svc| svc.prompt_section(&self.char_id))
            .flatten();

        // 室友在线状态：一句话提示，让 LLM 知道是否可用 talk_to_character
        // 通过 CROSS_CHARACTER_BUS 查询 AppState.characters（bus 已持有 AppHandle）
        let roommate_status = if !self.char_id.is_empty() {
            crate::cross_character::CROSS_CHARACTER_BUS.roommate_status_text(&self.char_id, &self.language)
        } else {
            None
        };

        // 室友认知印象：从室友 Private Mind 派生的行为印象（注意力/活动/目标/社交意愿）
        let roommate_cognitive_section = if !self.char_id.is_empty() {
            crate::cross_character::CROSS_CHARACTER_BUS.roommate_cognitive_text(&self.char_id, &self.language)
        } else {
            None
        };

        // 近期环境事件：从统一事件账本读取该角色可见的最近 5 条事件
        // （按 importance×recency 联合排序：dialogue > action > observer_note > system，
        //   候选池 n*3=15 条，避免全量排序同时保证重要事件不被时间埋没）
        let environment_events = if !self.char_id.is_empty() {
            crate::memory::unified_event_ledger::unified_event_ledger()
                .build_prompt_section(&self.char_id, 5, &self.language)
        } else {
            None
        };

        // 关系认知事实：当前角色对室友的陈述性认知（"A 眼中的 B"）
        let relationship_facts_section = if !self.char_id.is_empty() {
            crate::cross_character::CROSS_CHARACTER_BUS
                .relationship_facts_text(&self.char_id, &self.language)
        } else {
            None
        };

        // 共享世界记忆：两角色共同知晓的世界事实（受 inject_into_prompt 开关控制）
        let shared_world_section = self.world_provider.as_ref().and_then(|wp| {
            if wp.config().inject_into_prompt {
                crate::memory::world_knowledge::world_knowledge().format_for_prompt(12, &self.language)
            } else {
                None
            }
        });

        // 社交状态：三方关系数值快照
        let social_state_section = if !self.char_id.is_empty() {
            crate::cross_character::CROSS_CHARACTER_BUS
                .social_state_text(&self.char_id, &self.language)
        } else {
            None
        };

        // Mind 段落：Belief / Goal / Attention 三合一序列化
        let mind_section = self
            .mind
            .as_ref()
            .and_then(|m| m.serialize_for_prompt(&self.language));

        // Working Memory 段落：30 秒级"正在想什么"缓冲区 + LLM 合成的当前想法摘要
        // 纯运行时，让 LLM 感知本会话最近几轮的活跃想法（蒸馏摘要，非原文）
        let working_memory_section = self
            .mind
            .as_ref()
            .and_then(|m| m.working_memory_prompt_section_with_thought(&self.language));

        // Self State 段落：角色自我状态快照（由 Brain 在 think 前注入 PipelineState）
        let self_state_section = if state.self_state_text.trim().is_empty() {
            None
        } else {
            Some(state.self_state_text.clone())
        };

        // User Entity 段落：用户在场/离开/预期回归（由 WorldState 提供）
        // 让 LLM 感知"用户现在在哪、何时回来"，避免对着空座说话或对离开时间产生错判
        let user_entity_snapshot = self
            .world_state
            .as_ref()
            .map(|ws| ws.user_entity_snapshot());
        let user_entity_section = user_entity_snapshot
            .as_ref()
            .and_then(|s| s.serialize_for_prompt(&self.language));

        // 观察上下文段落：用户在持续状态中突然说话时（如睡觉中发消息），
        // 注入简短观察提示，让 LLM 自然回应"你醒啦？"之类的内容
        let observation_section = user_entity_snapshot
            .as_ref()
            .and_then(|s| crate::mind::UserCognitionEngine::generate_observation_context(s, &self.language));

        // Episode 段落：最近 1-3 个经历摘要（不是原始消息，而是封包后的经历）
        // 让 LLM 理解"最近发生过什么"，而不是"数据库里有哪几条记忆"
        let episode_section = self
            .episode_store
            .as_ref()
            .and_then(|store| build_episode_section(store, 3, &self.language));

        // 内心反应：从现有心理状态合成一行第一人称内心感受，注入 prompt
        // 不调用 LLM，纯规则合成；当心理状态平淡时返回 None，不注入多余内容
        let inner_reaction = self.psychology.as_ref().and_then(|psy| {
            self.mind.as_ref().and_then(|m| {
                crate::pipeline::prompt_modules::build_inner_reaction(
                    psy,
                    m,
                    &state.event_summary,
                    &self.char_id,
                    &self.language,
                )
            })
        });

        // Inline expression/motion tag instructions: injected when inline_expression is enabled, tells LLM tag format and available names
        // 规则部分压缩为 flag+微注释格式；规则类内容统一英文
        let inline_tag_section = if self.inline_expression_enabled {
            let names_text = self.expression_motion_names.as_deref().unwrap_or("Expressions: (none)\nMotions: (none)");
            Some(format!(
                "[INLINE TAG SPEC]\n\
                Embed in reply text to trigger expressions/motions (instant during streaming):\n\
                - <e name=\"...\" dur=\"ms\"/> expression (dur optional: 0=natural switch, 1500-3000=brief flash, 4000-6000=medium hold, 8000+=strong emotion)\n\
                - <m name=\"...\"/> motion\n\n\
                {names_text}\n\n\
                RULES: embed at natural positions in text, never on a separate line | default OFF — add only when the reply has clear emotional tone | if nothing fits, skip | names MUST come from the list above\n\
                [/INLINE TAG SPEC]"
            ))
        } else {
            None
        };

        // 场景语气只匹配当前发言，避免用户换话题后仍沿用上一轮的场景表演。
        // 命中时只注入少量可选节奏参考，不要求复刻台词。
        let tone_injection = self.tone_injector.as_ref().and_then(|injector| {
            injector.build_tone_injection(&state.user_input, &self.language)
        });

        // 情绪表达偏置：注入连续效价/激活/主导强度，按比例影响节奏
        // 每轮最多体现一个轻微线索，避免跨阈值突然进入表演模式
        // 情绪只改变表达，不改变任务响应；与场景语气合并到 tone_injection
        let emotion_state = self.psychology.as_ref().and_then(|psy| {
            build_emotion_state_section(&psy.emotion(), &self.char_id, &self.language)
        });
        let tone_injection = match (emotion_state, tone_injection) {
            (Some(es), Some(ti)) => Some(format!("{es}\n\n{ti}")),
            (Some(es), None) => Some(es),
            (None, ti) => ti,
        };

        // 近期网络语境：热梗采集写入的短 TTL 知识此前只能等普通语义检索碰巧召回，
        // 很难真正影响闲聊。这里在闲聊类轮次提供一份紧凑的内部参考；它是用法
        // 背景，不是台词库，模型通常仍应不用，贴合时也最多自然带出一个表达。
        let current_culture_context = self.memory.as_ref().and_then(|mem| {
            let casual_turn = state
                .fast_perception
                .as_ref()
                .map(|fp| matches!(fp.intent.label.as_str(), "chat" | "sharing" | "complaint"))
                .unwrap_or_else(|| state.user_input.chars().count() <= 48);
            if !casual_turn {
                return None;
            }
            let item = mem.recent_by_tags(&["meme"], 1).into_iter().next()?;
            let content: String = item.content.chars().take(900).collect();
            if content.trim().is_empty() {
                return None;
            }
            let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
            let usage = match (lang, self.char_id.as_str()) {
                ("zh", "vivian") => "这是近期网络语境，只在当前情境完全贴合时自然用一个表达；大多数轮次不用。不要复述笔记、报梗名或解释出处，除非用户问。",
                ("zh", _) => "这是近期网络语境，主要用于听懂用户；只有非常自然时才轻轻接一个表达。不要复述笔记、报梗名或解释出处，除非用户问。",
                ("ja", "vivian") => "最近のネット文脈。今の場面に完全に合う時だけ一表現を自然に使い、通常は使わない。メモ、ネタ名、出典を復唱・説明しない（聞かれた時を除く）。",
                ("ja", _) => "最近のネット文脈。主にユーザーを理解するために使い、非常に自然な時だけ一表現を軽く返す。メモや出典を説明しない（聞かれた時を除く）。",
                (_, "vivian") => "Current online context. Use at most one expression only when it fits perfectly; most turns should use none. Never recap the note, name the meme, or explain its origin unless asked.",
                _ => "Current online context, mainly for understanding the user. Echo at most one expression only when effortless; never recap or explain the note unless asked.",
            };
            Some(format!("## Current online context (internal, optional)\n{usage}\n<current_culture>\n{content}\n</current_culture>"))
        });
        let tone_injection = match (current_culture_context, tone_injection) {
            (Some(culture), Some(tone)) => Some(format!("{culture}\n\n{tone}")),
            (Some(culture), None) => Some(culture),
            (None, tone) => tone,
        };
        // 随机小事回响：低概率注入一条用户随口提过的小事，
        // 让 AI 偶尔自然带出"对了你那个XX怎么样了"这种活人感细节
        let random_echo = self.memory.as_ref().and_then(|mem| {
            let ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .subsec_nanos();
            if ns % 10 != 0 {
                return None;
            }
            let candidates = mem.recent_by_tags(&["preference", "habits", "user_fact"], 10);
            if candidates.is_empty() {
                return None;
            }
            let idx = (ns as usize) % candidates.len();
            let item = &candidates[idx];
            let header = section_heading("random_echo", &self.language);
            Some(format!(
                "{header}\n你偶尔想起来一件事，可以自然地提一嘴（不是非提不可）：\n- {content}",
                content = item.content
            ))
        });
        let tone_injection = match (random_echo, tone_injection) {
            (Some(re), Some(ti)) => Some(format!("{re}\n\n{ti}")),
            (Some(re), None) => Some(re),
            (None, ti) => ti,
        };

        // 快速语义感知引导：来自 FastSemanticAnalyzer 在 prepare_pipeline_state 阶段
        // 填充的多维度嵌入分类结果。仅当 guidance 非空时注入（无显著信号时不污染 prompt）。
        let fast_perception_guidance = state.fast_perception.as_ref().and_then(|p| {
            let guidance = p.guidance.trim();
            if guidance.is_empty() {
                None
            } else {
                let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
                let heading = crate::pipeline::prompt_modules::section_heading("fast_perception_guidance", lang);
                Some(format!("{}\n{}", heading, guidance))
            }
        });

        // 推荐工具提示：复用同一份语义召回结果（ToolScope.recalled_order），仅列名称一行。
        // 不再重列描述/分数——完整 schema 已在「可用工具」主列表注入，重复描述只会浪费 token。
        // 仍按 intent 门控（tool_request/request/question）显示，避免闲聊场景噪声。
        let recommended_tools = tool_scope.and_then(|scope| {
            if scope.recalled_order.is_empty() {
                return None;
            }
            let intent_ok = state
                .fast_perception
                .as_ref()
                .map(|fp| crate::tools::should_filter_tools(&fp.intent.label))
                .unwrap_or(false);
            if !intent_ok {
                return None;
            }
            let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
            let heading =
                crate::pipeline::prompt_modules::section_heading("recommended_tools", lang);
            let line = match lang {
                "en" => format!(
                    "Most likely needed this turn: {}",
                    scope.recalled_order.join(", ")
                ),
                "ja" => format!(
                    "今回使う可能性が高いツール：{}",
                    scope.recalled_order.join("、")
                ),
                _ => format!("本轮最可能用到：{}", scope.recalled_order.join("、")),
            };
            Some(format!("{}\n{}", heading, line))
        });

        // Topic 驱动背景知识注入：扫描用户输入，命中关键词则激活对应 topic，
        // 在 prompt 中注入背景知识段落，持续 N 轮后进入冷却
        let topic_injection_section = self.topic_injection.as_ref().and_then(|mgr| {
            mgr.scan_input(&state.user_input);
            let text = mgr.consume_turn()?;
            let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
            let heading = crate::pipeline::prompt_modules::section_heading("topic_injection", lang);
            Some(format!("{}\n{}", heading, text))
        });

        // 后台任务段落：顶级任务运行状态 + 未汇报的完成报告（每份报告只注入一次）
        let background_tasks_section = {
            let text = build_background_tasks_section(&self.char_id, &self.language);
            if text.is_empty() { None } else { Some(text) }
        };

        // 首次见面：由"持久记忆库 + 对话历史"独立判定，**不能从本轮空召回推断**。
        // 字段契约见 prompt_modules.rs 中 `is_first_meeting` 的文档注释；
        // brain.rs 的 generate_startup_greeting 是同一判据的正确实现。
        //
        // 旧实现 `state.memory_text.is_empty()` 会把"这轮没召回到高相关记忆"误判成
        // "第一次见面"，从而触发 human_feel 的 NO_ONBOARDING 反例（自我介绍 / 破冰
        // 脚本）——召回为空远比真·首次见面常见（短查询、rewrite 跳过、分数低于
        // min_score 被过滤都会命中），这是陪伴侧"机器感"的一个直接来源。
        //
        // 注：state.messages 此刻只含历史——本轮用户消息要到 chat_chain 末尾才入历史。
        // memory 未注入时取 false（宁可漏掉一次破冰，也不要误判出自我介绍）。
        let is_first_meeting = self
            .memory
            .as_ref()
            .map(|m| m.non_seed_count() == 0)
            .unwrap_or(false)
            && state.messages.is_empty();

        PromptParts {
            model_context_window: None,
            user_level: 0,
            task_type: String::new(),
            user_input: state.user_input.clone(),
            memory_text: state.memory_text.clone(),
            memory_md_section,
            character_block,
            examples_block,
            style_block,
            style_preset_block,
            relationship_section,
            relationship_log_section,
            user_facts_section,
            dynamic_behavior_section,
            relationship_facts_section,
            shared_world_section,
            social_state_section,
            worldbook_block,
            tools,
            emotion_context,
            inner_reaction,
            environment_context: Some(env_ctx),
            activity_brief,
            user_research,
            topic_injection_section,
            skill_section,
            epistemic_signals_section: state.epistemic_assessment.as_ref().map(format_epistemic_signals),
            schedule_signals_section: state.schedule_assessment.as_ref().and_then(format_schedule_signals),
            user_model_section: if state.user_model_text.is_empty() { None } else { Some(state.user_model_text.clone()) },
            proactive_search_section: if state.web_context.is_empty() { None } else { Some(state.web_context.clone()) },
            is_first_meeting,
            channel: state.current_channel.clone(),
            presence_state: state.presence_state.clone(),
            roommate_status,
            roommate_cognitive_section,
            environment_events,
            mind_section,
            working_memory_section,
            self_state_section,
            user_entity_section,
            background_tasks_section,
            observation_section,
            episode_section,
            inline_tag_section,
            tone_injection,
            fast_perception_guidance,
            recent_self_utterances,
            recommended_tools,
            cross_character_mode: state.current_channel == "cross_character",
            char_id: self.char_id.clone(),
            enable_native_fc: self.enable_native_fc,
            has_native_schema: self.has_native_schema,
            enable_instructions: true,
            instructions: None,
            language: self.language.clone(),
        }
    }
}

/// 抽取本角色最近说过的话，拼成反重复清单
///
/// 只取 **自己** 的发言：跨角色对话里 messages 混着对方角色的台词，
/// 把别人说过的话列成"你别重复"会让模型束手束脚。
///
/// 开场之外，这是唯一的"事前"反重复手段——`LoopDetectionAdvisor` 属于事后补救，
/// 且只认归一化后完全相同的文本，拦不住"换几个字、句式照旧"的渐进套话化。
fn build_recent_self_utterances(
    messages: &[crate::types::response::ChatMessage],
    char_id: &str,
    channel: &str,
    language: &str,
) -> Option<String> {
    const TAKE: usize = 6;
    const SNIPPET_CHARS: usize = 40;

    let now = chrono::Local::now();
    let mut lines: Vec<String> = Vec::new();

    for m in messages.iter().rev() {
        if lines.len() >= TAKE {
            break;
        }
        if m.role != "assistant" || m.content.trim().is_empty() {
            continue;
        }
        // 跨角色对话中，只统计本角色的发言（消息内容带 "[X says to me]" 前缀）
        if channel == "cross_character" {
            let speaker = crate::cross_character::parse_speaker_prefix(&m.content).1;
            if !speaker.is_empty() && speaker != char_id {
                continue;
            }
        }
        let span = m
            .timestamp
            .map(|ts| {
                let elapsed = now.signed_duration_since(ts.with_timezone(&chrono::Local));
                let mins = elapsed.num_minutes().max(0);
                if mins < 1 {
                    "刚刚".to_string()
                } else if mins < 60 {
                    format!("{} 分钟前", mins)
                } else if mins < 60 * 24 {
                    format!("{} 小时前", mins / 60)
                } else {
                    format!("{} 天前", mins / (60 * 24))
                }
            })
            .unwrap_or_else(|| "更早".to_string());

        let snippet: String = m.content.chars().take(SNIPPET_CHARS).collect();
        let ellipsis = if m.content.chars().count() > SNIPPET_CHARS {
            "…"
        } else {
            ""
        };
        lines.push(format!("- [{}] {}{}", span, snippet, ellipsis));
    }

    // 不足两条说明还没形成可辨识的说话模式，注入反而是在凭空设限
    if lines.len() < 2 {
        return None;
    }

    let (hint_zh, hint_en, hint_ja) = (
        "注意：上面这些里反复出现的起手式、口头禅和句式，本身就是模板信号。\
         这一轮换一种说法——同样的意思也要换个讲法。\
         如果最近已经连着几轮都用了差不多的接法，这轮就该换个角度，或者少说两句。",
        "Note: recurring openers, catchphrases and sentence patterns above are themselves \
         template signals. Phrase this one differently — same meaning, different wording. \
         If the last few turns all landed the same way, change the angle or say less.",
        "注意：上記で繰り返し現れる言い回しや口癖は、それ自体がテンプレの合図。\
         この一回は別の言い方をすること——同じ意味でも言い方を変える。\
         最近数ターン同じような返しが続いているなら、切り口を変えるか、少し短く。",
    );
    let hint = match crate::pipeline::prompt_modules::normalize_lang(language) {
        "en" => hint_en,
        "ja" => hint_ja,
        _ => hint_zh,
    };

    Some(format!(
        "{}\n{}\n\n{}",
        crate::pipeline::prompt_modules::section_heading("recent_self_utterances", language),
        lines.join("\n"),
        hint
    ))
}

/// 构建后台任务段落。三个来源互相独立：
///
/// - **自治任务**（`TaskService`）：本角色自己派出去的活，运行中的 + 刚完成待汇报的
/// - **工作完成报告**（`work_notices`）：工作智能体（编程向）刚干完的事，取走即消费
/// - **等你拍板的提问**：工作智能体正卡在用户那儿，用户没回答前每轮都注入——
///   它是阻塞用户任务的未闭合环，不能像报告那样说一次就丢
///
/// 全部为空时返回空串（不注入段落）。
fn build_background_tasks_section(char_id: &str, language: &str) -> String {
    let task_service = crate::brain::task_service::global();
    let running = task_service
        .as_ref()
        .map(|ts| ts.running_top_level_for(char_id))
        .unwrap_or_default();
    let pending = task_service
        .as_ref()
        .map(|ts| ts.unconsumed_reports_for(char_id))
        .unwrap_or_default();
    // 工作智能体的完成报告：取走即消费（每份只说一次）
    let notices = crate::brain::work_notices::global();
    let work_reports = notices.take_reports_for(char_id);
    // 工作智能体卡在用户拍板上的提问：用户没回答前，每轮都该被看见
    let attentions = crate::brain::work_notices::pending_attention_for(char_id);

    if running.is_empty()
        && pending.is_empty()
        && work_reports.is_empty()
        && attentions.is_empty()
    {
        return String::new();
    }

    let lang = crate::pipeline::prompt_modules::normalize_lang(language);
    let heading = crate::pipeline::prompt_modules::section_heading("background_tasks", lang);
    let l = bg_task_labels(lang);
    let short_id = |id: &str| -> String { id.chars().take(12).collect() };
    let mut lines = vec![heading.to_string()];

    for t in running.iter().take(5) {
        let d: String = t.directive.chars().take(60).collect();
        lines.push(format!(
            "- [{}] {d}（{steps} / {id}）",
            l.running,
            steps = t.steps,
            id = short_id(&t.task_id)
        ));
    }
    for t in pending.iter().take(3) {
        let d: String = t.directive.chars().take(60).collect();
        let body = t
            .report
            .clone()
            .unwrap_or_else(|| t.error.clone().unwrap_or_else(|| l.no_detail.to_string()));
        let body: String = body.chars().take(400).collect();
        let status = if t.status == "failed" { l.failed } else { "" };
        lines.push(format!("- [{}{status}] {d}\n  {}：{body}", l.done, l.report));
    }
    for r in work_reports.iter().take(3) {
        // 这是工作侧返回给陪伴侧的正式总结，不应压成一句状态。单份限制 1200 字，
        // 既能保留改动与验证证据，也避免多个并发任务挤占整轮上下文。
        let body: String = r.body.chars().take(1200).collect();
        lines.push(format!(
            "- [{} | 会话 {}]\n  {body}",
            r.title,
            short_id(&r.session_id)
        ));
    }

    // 提问先按「这轮是首次提醒还是已经提醒过」分流，再统一标记已提醒。
    // 首次给"去提醒用户"，之后改给"他已经知道了，别反复催"——否则
    // 用户每跟角色说一句话就被催一次。
    let mut fresh_attention = false;
    let mut stale_attention = false;
    for q in attentions.iter().take(3) {
        if notices.attention_reminded(q.question_id) {
            stale_attention = true;
        } else {
            fresh_attention = true;
            notices.mark_attention_reminded(q.question_id);
        }
        let question: String = q.question.chars().take(150).collect();
        lines.push(format!("- [{}] {question}", l.attention));
        if let Some(ctx) = &q.context {
            let ctx: String = ctx.chars().take(150).collect();
            if !ctx.trim().is_empty() {
                lines.push(format!("  {}：{ctx}", l.attention_context));
            }
        }
        let opts: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
        if !opts.is_empty() {
            lines.push(format!("  {}：{}", l.attention_options, opts.join(" / ")));
        }
    }

    if !pending.is_empty() || !work_reports.is_empty() {
        lines.push(l.report_guide.to_string());
    }
    if fresh_attention {
        lines.push(l.attention_guide.to_string());
    } else if stale_attention {
        lines.push(l.attention_seen.to_string());
    }
    lines.join("\n")
}

/// `background_tasks` 段落的三语文案。
///
/// 集中在这里而不是散在渲染逻辑里——段落现在有三个来源
/// （自治任务 / 工作完成报告 / 等你拍板的提问），文案散开就没法对着改。
struct BgTaskLabels {
    running: &'static str,
    done: &'static str,
    report: &'static str,
    failed: &'static str,
    no_detail: &'static str,
    report_guide: &'static str,
    attention: &'static str,
    attention_context: &'static str,
    attention_options: &'static str,
    attention_guide: &'static str,
    attention_seen: &'static str,
}

fn bg_task_labels(lang: &str) -> BgTaskLabels {
    match lang {
        "en" => BgTaskLabels {
            running: "Running in background",
            done: "Just finished (not yet reported to the user)",
            report: "Report",
            failed: " failed",
            no_detail: "(no output)",
            report_guide:
                "Naturally report the finished results above to the user in this reply, in your own voice.",
            attention: "Waiting for your call",
            attention_context: "Context",
            attention_options: "Options",
            attention_guide: "The item marked \"Waiting for your call\" is blocked on the user — \
the background job can't continue until they answer. In your own voice, tell them in one sentence \
to take a look at the work page. Don't decide for them, and don't read the options out.",
            attention_seen: "You have already told the user about the \"Waiting for your call\" item \
and they haven't answered. Don't keep nagging — bring it up only if they ask.",
        },
        "ja" => BgTaskLabels {
            running: "バックグラウンドで実行中",
            done: "完了済み（まだユーザーに報告していない）",
            report: "報告",
            failed: " 失敗",
            no_detail: "（詳細出力なし）",
            report_guide:
                "上記の完了結果は、今回の返信で自分の口調でユーザーに自然に報告してください。",
            attention: "あなたの判断待ち",
            attention_context: "背景",
            attention_options: "選択肢",
            attention_guide: "「あなたの判断待ち」と付いた件はユーザーの返答待ちで、\
バックグラウンドの処理が止まっています。自分の口調で、作業ページを覗いてほしいと一言だけ\
伝えてください。代わりに選ばず、選択肢も読み上げないこと。",
            attention_seen: "「あなたの判断待ち」の件はすでにユーザーに伝えましたが、\
まだ返答がありません。何度も急かさず、聞かれたときに触れてください。",
        },
        _ => BgTaskLabels {
            running: "后台进行中",
            done: "刚完成（尚未向用户汇报）",
            report: "报告",
            failed: "失败",
            no_detail: "（无详细输出）",
            report_guide: "上面是工作智能体返回的任务总结。请准确保留结果、验证与未完成项，用自己的口吻向用户简短转达；不要把工作说成是你亲自执行的，也不要改写成比原报告更乐观的结论。用户追问细节时可用 get_work_status 读取该工作会话的最终总结。",
            attention: "等你拍板",
            attention_context: "背景",
            attention_options: "待选",
            attention_guide: "上面标着「等你拍板」的事卡在用户那儿——后台正等着这个回答才能\
继续。请用你自己的口吻，用一句话提醒用户去工作页看一眼。别替他做选择，也别把选项念一遍。",
            attention_seen: "上面标着「等你拍板」的事你已经提醒过用户，他还没有回答。\
不必反复催促；他问起时再说。",
        },
    }
}

/// 构建 Episode 段落文本
///
/// 取最近 N 个 Episode，渲染为简洁的经历摘要：
/// - 时间范围 + 主题 + 摘要 + 参与者
/// - 不包含原始消息内容（避免与 memory_text 重复）
fn build_episode_section(
    store: &Arc<crate::memory::episode::EpisodeStore>,
    max_count: usize,
    lang: &str,
) -> Option<String> {
    let episodes = store.recent(max_count);
    if episodes.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::with_capacity(episodes.len() + 1);
    lines.push(section_heading("relevant_episodes", lang).to_string());

    for ep in &episodes {
        let start = chrono::DateTime::from_timestamp(ep.started_at as i64, 0)
            .map(|dt| dt.format("%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "?".to_string());
        let end = chrono::DateTime::from_timestamp(ep.ended_at as i64, 0)
            .map(|dt| dt.format("%H:%M").to_string())
            .unwrap_or_else(|| "?".to_string());

        let topic = ep.topic.as_deref().unwrap_or(section_heading("casual_chat", lang));
        let summary = ep.summary.as_deref().unwrap_or("");

        let importance_tag = if ep.importance >= 0.8 {
            " ★"
        } else {
            ""
        };

        let summary_line = if summary.is_empty() {
            String::new()
        } else {
            format!(" — {}", summary)
        };

        lines.push(format!(
            "- [{}~{}{}] {}{}",
            start, end, importance_tag, topic, summary_line
        ));
    }

    Some(lines.join("\n"))
}

impl Default for PromptBuildingStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for PromptBuildingStep {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 工具范围只算一次：场景 + 隐藏集 + 语义召回。文本通道（build_parts）与
        // 原生 FC 通道（下方 tool_definitions）共享同一份，杜绝此前两条路各算一遍、
        // 参数不一致、以及推荐段与主列表重复注入的问题。
        let tool_scope: Option<ToolScope> = self
            .tool_system
            .as_ref()
            .map(|ts| self.compute_tool_scope(ts, &state));

        // 使用模块化提示词构建器 + 模板引擎元数据（Section Schema 驱动）
        let mut parts = self.build_parts(&state, tool_scope.as_ref());
        let task_type = config.as_ref().map(RunnableConfig::task_type)
            .unwrap_or_else(|| "chat".to_string());
        parts.model_context_window = Some(self.task_context_windows.get(&task_type)
            .copied().unwrap_or(self.default_context_window));
        parts.user_level = self.psychology.as_ref()
            .map(|psy| psy.relationship().stage()).unwrap_or(0);
        parts.task_type = task_type;

        // 待汇报的后台任务报告 id：随 state 传给生成步骤，
        // 动态便签进入请求后由生成步骤标记消费（每份报告只注入一次）
        if let Some(ts) = crate::brain::task_service::global() {
            let ids: Vec<String> = ts
                .unconsumed_reports_for(&self.char_id)
                .into_iter()
                .map(|t| t.task_id)
                .collect();
            if !ids.is_empty() {
                state.metadata["bg_report_task_ids"] = serde_json::json!(ids);
            }
        }

        // 使用模板引擎的 enriched builder：产出 prompt + 每个 section 的元数据
        // Section 列表由 template_engine::section_schema() 统一定义，消除硬编码
        let enriched =
            crate::pipeline::template_engine::build_prompt_with_sections(&parts);

        // 将 SectionRenderInfo 数组序列化到 metadata，供 Mind Inspector 前端使用
        // （BrainChatChain::ainvoke 据此构造 PromptBreakdown）
        let prompt_sections: serde_json::Value = serde_json::json!(
            enriched.sections.iter().map(|s| serde_json::json!({
                "name": s.name,
                "content": s.content,
                "section_id": s.id,
                "layer": s.layer.as_str().to_string(),
                "token_estimate": s.token_estimate,
                "optional": s.optional,
                "present": s.present,
            })).collect::<Vec<_>>()
        );
        state.metadata["prompt_sections_breakdown"] = prompt_sections;

        let mut prompt = enriched.prompt;

        // 兜底：如果模块化构建意外产生空提示词，回退到静态 IDENTITY_BLOCK + OUTPUT_RULES
        if prompt.trim().is_empty() {
            let mut sections: Vec<String> = Vec::new();
            sections.push(fallback_identity_block(&self.char_id, &self.language));
            if !state.memory_text.is_empty() {
                sections.push(format!("{}\n{}", fallback_label("memory", &self.language), state.memory_text));
            }
            if !state.messages.is_empty() {
                let history = state
                    .messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!("{}\n{}", fallback_label("history", &self.language), history));
            }
            if !state.user_input.is_empty() {
                sections.push(format!("{}{}", fallback_label("user", &self.language), state.user_input));
            }
            sections.push(fallback_output_rules().to_string());
            prompt = sections.join("\n\n");
        }

        // 同步生成结构化工具定义（原生 function calling 路径用；文本路径忽略此字段）。
        // 直接复用 ainvoke 开头算好的共享 ToolScope——场景/隐藏集/语义召回与文本通道
        // 完全一致：完整 schema = 保底集 ∪ 召回集，其余降级为仅名称（可用 tool_search 取回）。
        if let (Some(ts), Some(scope)) = (self.tool_system.as_ref(), tool_scope.as_ref()) {
            state.tool_definitions = ToolListTool::new(Arc::clone(ts))
                .get_tool_definitions_for_scene(scope.scene, &scope.hidden, scope.recalled.as_ref());
        }

        // 预存文本回退内容：当 native FC 或 JSON Schema 启用时，prompt 中工具区段和输出格式均为空。
        // 若原生 FC 路径失败回退到文本路径，需要这些数据补回。
        if self.enable_native_fc || self.has_native_schema {
            let fallback_tools = build_tools_block(
                parts.tools.as_deref(),
                false, // 强制生成文本版
                &self.language,
            );
            if !fallback_tools.is_empty() {
                state.tools_text_fallback = Some(fallback_tools);
            }
            state.output_format_fallback = Some(output_format().to_string());
        }

        // 记录 API 参数元数据（供 Mind Inspector 显示非 messages 数组的内容）
        if self.enable_native_fc {
            state.metadata["native_fc_enabled"] = json!(true);
        }
        if self.has_native_schema {
            state.metadata["response_format_schema"] = json!({
                "type": "json_schema",
                "enabled": true
            });
        }

        state.system_prompt = prompt.clone();
        state.prompt = prompt;
        state.metadata["prompt_length"] = json!(state.system_prompt.chars().count());
        state.metadata["prompt_sections"] = json!(12);

        Ok(state.to_json())
    }
}


/// 把情绪映射成连续的表达偏置。
///
/// 旧实现把七个情绪分别切成三档并叠加台词式命令，数值跨过阈值时会突然
/// “进入表演模式”。这里直接给模型连续的效价、激活度和主导强度，每轮只使用
/// 一个轻微线索。情绪改变节奏，不改变是否回答用户，也不生成一段情绪剧情。
fn build_emotion_state_section(
    emotion: &crate::psychology::EmotionState,
    char_id: &str,
    lang: &str,
) -> Option<String> {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    let valence = emotion.valence();
    let activation = emotion.arousal();
    let (dominant, intensity) = emotion.dominant();

    if valence.abs() < 0.08 && (activation - 0.22).abs() < 0.08 && intensity < 0.52 {
        return None;
    }

    let dominant_name = dominant.as_str();
    let character_bias = match (lang_norm, char_id) {
        ("zh", "vivian") => "Vivian：正向且高激活时可以更快、更口语，偶尔自然接一个梗；低激活时少说半句即可。",
        ("zh", _) => "Nana：激活度主要改变句子的轻重和停顿，不要突然变成夸张活泼或刻意忧郁。",
        ("ja", "vivian") => "Vivian：ポジティブで活性が高い時はテンポを少し上げ、自然なら一度だけ軽いネット表現を使ってよい。低い時は半文ぶん静かに。",
        ("ja", _) => "Nana：活性度は文の重さと間にだけ反映し、急に大げさに明るくしたり沈んだ演技をしない。",
        (_, "vivian") => "Vivian: with positive high activation, speak a little faster and allow one natural online reaction; with low activation, simply say a little less.",
        _ => "Nana: let activation affect cadence and pauses only; never switch abruptly into exaggerated cheer or sadness.",
    };
    let rules = match lang_norm {
        "zh" => format!(
            "内部表达向量（连续值）：效价 {valence:.2}，激活 {activation:.2}，主导 {dominant_name} {intensity:.2}。\n- 只按数值幅度微调句长、节奏、标点和接梗意愿；通常一个线索就够，前后轮平滑过渡。\n- 不说出情绪标签，不解释自己为什么这样，不插入固定情绪台词，不因情绪拒绝回答、索取安慰或迁怒用户。\n- {character_bias}"
        ),
        "ja" => format!(
            "内部表現ベクトル（連続値）：valence {valence:.2}、activation {activation:.2}、dominant {dominant_name} {intensity:.2}。\n- 値の強さに比例して文の長さ、テンポ、句読点、冗談への乗り方を少しだけ変える。一度に一つの兆候で十分、前後のターンは滑らかにつなぐ。\n- 感情名を口にせず、理由を説明せず、定型の感情台詞を挿入しない。感情を理由に回答拒否、慰めの要求、八つ当たりをしない。\n- {character_bias}"
        ),
        _ => format!(
            "Internal delivery vector (continuous): valence {valence:.2}, activation {activation:.2}, dominant {dominant_name} {intensity:.2}.\n- Adjust sentence length, cadence, punctuation, and willingness to banter only in proportion to these values. One subtle cue is usually enough; transition smoothly between turns.\n- Never name the emotion, explain why you sound this way, insert a stock mood line, withhold an answer, seek reassurance, or take it out on the user.\n- {character_bias}"
        ),
    };

    Some(format!("{}\n{}", section_heading("emotion_behavior", lang), rules))
}

#[cfg(test)]
mod emotion_delivery_tests {
    use super::*;

    #[test]
    fn emotion_prompt_uses_continuous_delivery_controls() {
        let emotion = crate::psychology::EmotionState {
            joy: 0.72,
            curiosity: 0.64,
            ..Default::default()
        };
        let prompt = build_emotion_state_section(&emotion, "vivian", "zh").unwrap();
        assert!(prompt.contains("连续值"));
        assert!(prompt.contains("平滑过渡"));
        assert!(!prompt.contains("话痨模式"));
        assert!(!prompt.contains("连环追问"));
    }
}
/// 将认知知识需求评估格式化为 prompt 可注入的认知信号段落
///
/// 让 LLM 在生成前感知"用户输入可能需要外部验证"的多维信号，
/// 辅助 LLM 自主决定是否调用 web_search 工具。
/// 仅在非 NoSearch 决策时注入（避免不必要地污染 prompt）。
fn format_epistemic_signals(assessment: &EpistemicAssessment) -> String {
    // 如果决策为 NoSearch，不需要注入认知信号
    if matches!(assessment.decision, KnowledgeDecision::NoSearch) {
        return String::new();
    }

    let (clarity, factual, temporal, risk, gap) = (
        assessment.semantic_clarity,
        assessment.factual_dependence,
        assessment.temporal_sensitivity,
        assessment.interpretation_risk,
        assessment.knowledge_gap,
    );

    let mut lines: Vec<String> = Vec::new();

    // 标题
    lines.push("## 感知提示".to_string());
    lines.push("系统检测到用户输入可能存在以下特征：".to_string());

    // 语义清晰度
    if clarity < 0.5 {
        lines.push("- 语义模糊：可能指代不明或无法确定具体实体".to_string());
    } else if clarity < 0.7 {
        lines.push("- 语义略有模糊：可能存在指代不明的情况".to_string());
    }

    // 外部事实依赖
    if factual > 0.7 {
        lines.push("- 涉及外部事实：可能需要查证才能可靠回答".to_string());
    } else if factual > 0.4 {
        lines.push("- 可能涉及外部事实：如果现有知识不够，可以考虑搜索验证".to_string());
    }

    // 时效性
    if temporal > 0.6 {
        lines.push("- 涉及时效性信息：可能涉及近期事件，搜索获取最新信息会更可靠".to_string());
    } else if temporal > 0.3 {
        lines.push("- 可能涉及时效性内容：如果涉及近期事件，建议搜索验证".to_string());
    }

    // 解释风险
    if risk > 0.6 {
        lines.push("- 存在歧义：可能不是字面意思（网络梗/隐喻/荒诞组合），自行解释容易误解".to_string());
    } else if risk > 0.3 {
        lines.push("- 可能有歧义：如果感觉不太对劲，搜索确认一下更稳妥".to_string());
    }

    // 知识缺口
    if gap > 0.6 {
        lines.push("- 可能超出知识范围：涉及特定专名/事件/文化背景，搜索会更可靠".to_string());
    } else if gap > 0.3 {
        lines.push("- 可能涉及不熟悉的领域：如果感觉不确定，可以考虑搜索".to_string());
    }

    // 决策提示（只给高层指导，不强制）
    let decision_hint = match assessment.decision {
        KnowledgeDecision::SearchRequired => {
            "以上信号较强，建议优先使用 web_search 工具获取外部信息后再回答。"
        }
        KnowledgeDecision::SearchPreferred => {
            "以上信号存在，如果感觉当前知识不够用，可以使用 web_search 工具搜索确认。"
        }
        KnowledgeDecision::SearchOptional => {
            "以上信号较弱，如果现有知识足够回答，可以不搜索。"
        }
        _ => "",
    };

    if !decision_hint.is_empty() {
        lines.push(format!("- {}", decision_hint));
    }

    let body = lines.join("\n");
    format!("{}\n\n> 注意：这些只是系统感知到的信号，最终是否搜索由你根据实际情况决定。", body)
}

/// 格式化日程通知信号段落。
///
/// 仅当 `is_schedule_like` 时返回 Some，否则返回 None（不注入）。
/// 引导 LLM 切换到「真人助理」模式：识别转来的通知材料、抽取事件、
/// 直接建待办并告知（高置信）、事件时间与提醒时间分离。
fn format_schedule_signals(assessment: &ScheduleAssessment) -> Option<String> {
    if !assessment.is_schedule_like {
        return None;
    }

    let lines = vec![
        "## 助理提示",
        "系统检测到用户发来的内容像一份转发的通知或日程安排（含明确的时间/地点/要求）。请把它当作转来的材料，而不是用户在对你说话：",
        "- 从中提取关键事件：名称、时间、地点、着装或携带要求",
        "- 明确涉及用户本人安排的，直接用 add_todo 建待办并在回复中告知（用户可一句话撤销）；拿不准是否与用户相关的，先问一句要不要记",
        "- 一份通知里有多个时间点时，按事件分别建待办，或问用户要记哪几个",
        "- 建待办时：event_time 填事件本身的开始时间；due_date 是提醒触发时间，应早于 event_time，结合你对用户住址、通勤、作息的了解预留路程与准备时间；相关信息不足就先问用户从哪出发、要提前多久提醒",
    ];

    Some(lines.join("\n"))
}
