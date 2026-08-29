//! Prompt 组装流水线步骤：模块化 prompt 构建。
//!
//! - [`PromptBuildingStep`]：注入 PersonaEngine / EmotionBridge / PsychologyManager
//!   关系上下文由 PsychologyManager 统一提供（原 RelationshipManager 已整合）

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::emotion::{EmotionBridge, EpistemicAssessment, KnowledgeDecision};
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
        "nana" => ("娜娜", "一个温柔但有力量的人，同时也是一只桌面宠物。你说话轻声细语但很稳。"),
        _ => ("薇薇安", "一个温柔、活泼、有点小傲娇的桌面宠物。你生活在用户的桌面上，陪伴用户工作和生活。"),
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
    /// 当前界面语言（zh-CN / en / ja），用于加载对应语言的 framework 段落
    pub language: String,
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

    /// 将 PipelineState 转换为 PromptParts，注入动态人设/关系/情绪/心理上下文
    pub(crate) fn build_parts(&self, state: &PipelineState) -> PromptParts {
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

        // 场景感知工具列表：根据当前情绪 + 关系阶段 + 前台应用推断场景，
        // 过滤掉该场景下不应暴露的工具（如低信任阶段禁用系统控制类工具；
        // 工作类应用前台时启用 Focus 场景，禁用娱乐/媒体工具）
        let active_app = self
            .environment
            .as_ref()
            .map(|env| env.get_environment_info().current_window);
        let tools = self.tool_system.as_ref().map(|ts| {
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
            let mut hidden = std::collections::HashSet::new();
            if !crate::diary::is_tool_available(&self.char_id) {
                hidden.insert("write_diary".to_string());
            }
            // 跨角色对话模式下隐藏 talk_to_character：目标角色由源角色的 talk_to_character 唤起，
            // 若目标角色再调用此工具回复源角色，会因源角色持 think_lock 等待工具返回而形成死锁
            if state.current_channel == "cross_character" {
                hidden.insert("talk_to_character".to_string());
            }
            ToolListTool::new(Arc::clone(ts)).get_tools_for_ai_with_scene(scene, &hidden, &self.language)
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
                - <m name=\"...\"/> motion\n\
                - <s name=\"...\"/> sticker (once every 4-6 replies)\n\n\
                {names_text}\n\n\
                RULES: embed at natural positions in text, never on a separate line | default OFF — add only when the reply has clear emotional tone | if nothing fits, skip | names MUST come from the list above\n\
                [/INLINE TAG SPEC]"
            ))
        } else {
            None
        };

        // 场景语气注入：用 ToneInjector 匹配用户输入 + 最近 3 轮上下文，
        // 命中场景时注入对应场景的参考台词。注入位置在动态区末尾（工具列表前），
        // 利用近因效应让 LLM 生成前最后看到语气参考。
        let tone_injection = self.tone_injector.as_ref().and_then(|injector| {
            let recent: Vec<String> = state.messages.iter().rev().take(6).map(|m| m.content.clone()).collect();
            injector.build_tone_injection(&state.user_input, &recent, &self.language)
        });

        // 情绪行为约束：偏离基线的维度按三档梯度（轻微/明显/强烈）注入具体说话方式
        // 约束（如 sadness 0.5-0.7 →"能一个字回的绝不用两个字"），阈值相对各维度
        // set point 设计，日常波动不触发。与 emotion_context 的感知叙述互补。合并到 tone_injection
        let emotion_state = self.psychology.as_ref().and_then(|psy| {
            build_emotion_state_section(&psy.emotion(), &self.char_id, &self.language)
        });
        let tone_injection = match (emotion_state, tone_injection) {
            (Some(es), Some(ti)) => Some(format!("{es}\n\n{ti}")),
            (Some(es), None) => Some(es),
            (None, ti) => ti,
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

        // 工具语义粗筛：仅在 intent=tool_request/request/question 且注入了 ToolSemanticFilter 时触发。
        // 复用 FastPerceptionResult.query_embedding，避免重复嵌入查询文本。
        // 失败时静默跳过（不阻塞 prompt 构建）。
        let recommended_tools = self
            .tool_semantic_filter
            .as_ref()
            .and_then(|filter| self.tool_system.as_ref().map(|ts| (filter, ts)))
            .and_then(|(filter, ts)| {
                state.fast_perception.as_ref().and_then(|fp| {
                    if !crate::tools::should_filter_tools(&fp.intent.label) {
                        return None;
                    }
                    let recs = filter.filter_default(ts, fp.query_embedding.as_slice());
                    if recs.is_empty() {
                        return None;
                    }
                    let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
                    let heading = crate::pipeline::prompt_modules::section_heading("recommended_tools", lang);
                    let lines: Vec<String> = recs
                        .iter()
                        .map(|r| format!("- {} ({:.2}): {}", r.name, r.similarity, r.description))
                        .collect();
                    Some(format!("{}\n{}", heading, lines.join("\n")))
                })
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

        PromptParts {
            user_input: state.user_input.clone(),
            memory_text: state.memory_text.clone(),
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
            user_model_section: if state.user_model_text.is_empty() { None } else { Some(state.user_model_text.clone()) },
            proactive_search_section: if state.web_context.is_empty() { None } else { Some(state.web_context.clone()) },
            is_first_meeting: state.memory_text.is_empty(),
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

/// 构建后台任务段落（运行中 + 待汇报完成报告）。
///
/// 无任务时返回空串（不注入段落）。
fn build_background_tasks_section(char_id: &str, language: &str) -> String {
    let Some(ts) = crate::brain::task_service::global() else {
        return String::new();
    };
    let running = ts.running_top_level_for(char_id);
    let pending = ts.unconsumed_reports_for(char_id);
    if running.is_empty() && pending.is_empty() {
        return String::new();
    }
    let lang = crate::pipeline::prompt_modules::normalize_lang(language);
    let heading = crate::pipeline::prompt_modules::section_heading("background_tasks", lang);
    let (running_label, done_label, report_label, failed_label, guide): (&str, &str, &str, &str, &str) = match lang {
        "en" => (
            "Running in background",
            "Just finished (not yet reported to the user)",
            "Report",
            " failed",
            "Naturally report the finished results above to the user in this reply, in your own voice.",
        ),
        "ja" => (
            "バックグラウンドで実行中",
            "完了済み（まだユーザーに報告していない）",
            "報告",
            " 失敗",
            "上記の完了結果は、今回の返信で自分の口調でユーザーに自然に報告してください。",
        ),
        _ => (
            "后台进行中",
            "刚完成（尚未向用户汇报）",
            "报告",
            "失败",
            "上面刚完成的结果，请在本次回复中用自己的口吻自然地向用户汇报。",
        ),
    };
    let short_id = |id: &str| -> String { id.chars().take(12).collect() };
    let mut lines = vec![heading.to_string()];
    for t in running.iter().take(5) {
        let d: String = t.directive.chars().take(60).collect();
        lines.push(format!("- [{running_label}] {d}（已 {steps} 步，任务 {id}）", steps = t.steps, id = short_id(&t.task_id)));
    }
    for t in pending.iter().take(3) {
        let d: String = t.directive.chars().take(60).collect();
        let body = t
            .report
            .clone()
            .unwrap_or_else(|| t.error.clone().unwrap_or_else(|| "（无详细输出）".into()));
        let body: String = body.chars().take(400).collect();
        let status = if t.status == "failed" { failed_label } else { "" };
        lines.push(format!("- [{done_label}{status}] {d}\n  {report_label}：{body}"));
    }
    if !pending.is_empty() {
        lines.push(guide.to_string());
    }
    lines.join("\n")
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
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 使用模块化提示词构建器 + 模板引擎元数据（Section Schema 驱动）
        let parts = self.build_parts(&state);

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

        // 同步生成结构化工具定义（与 build_parts 内的 Markdown 工具列表同一场景筛选逻辑）
        // 用于 generation 层的原生 function calling 路径；文本路径忽略此字段。
        if let Some(ts) = self.tool_system.as_ref() {
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
            let has_recent_tool_use = ts.has_recent_tool_call(300);
            let scene = ToolScene::from_full_context(
                stage.as_deref(),
                emotion.as_deref(),
                active_app.as_deref(),
                &state.user_input,
                has_recent_tool_use,
            );
            let mut hidden = std::collections::HashSet::new();
            if !crate::diary::is_tool_available(&self.char_id) {
                hidden.insert("write_diary".to_string());
            }
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
            state.tool_definitions =
                ToolListTool::new(Arc::clone(ts)).get_tool_definitions_for_scene(scene, &hidden);
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


/// 根据当前情绪状态生成语言行为约束（三档梯度，仅偏离基线时触发）
///
/// 与 build_psychology_prompt（感知叙述）的分工：
/// - 感知叙述告诉 LLM "你现在感觉挺难过"（状态是什么）
/// - 本函数告诉 LLM "能一个字回的绝不用两个字"（行为怎么变），并按强度分三档
///
/// 档位设计原则：
/// - 阈值相对各维度 set point 设计（见 emotion.rs Default）：joy 基线 0.35 故 0.5 起，
///   curiosity 基线 0.45 故 0.6 起，loneliness 基线 0.15 故 0.35 起，
///   sadness/anger/fear 基线 ≤0.1 故 0.3 起步 —— 日常波动不触发，显著偏离才注入
/// - 三档从微妙影响 → 明显改变 → 强烈状态递进；每条约束是具体可观察行为
///   （回复长度/主动性/语气/接梗意愿/追问方式），而非感知叙述的同义重复 ——
///   微妙的行为线索正是 LLM 无法从"你有点难过"稳定推导的信息
/// - 文案区分角色（vivian 直球毒舌系 / nana 温柔内敛系），档位越高差异越明显
/// - fear 此前缺失，此处补全 7 维度
/// - closeness 仅在显著低（<0.18）时注入"临时疏离"提示，捕捉吵架后的暂态；
///   持久关系边界由 relationship_section（六阶段）负责，两者互补不重叠
/// - 多维同时触发时约束直接叠加；矛盾组合（如悲伤+孤独：想找人说又说不动）
///   本身就是真实的复合情绪，LLM 会自然融合，无需特判
fn build_emotion_state_section(
    emotion: &crate::psychology::EmotionState,
    char_id: &str,
    lang: &str,
) -> Option<String> {
    // 语言索引：0=zh 1=ja 2=en
    let li = match crate::pipeline::prompt_modules::normalize_lang(lang) {
        "zh" => 0,
        "ja" => 1,
        _ => 2,
    };
    // 角色索引：0=vivian 1=nana
    let ri = if char_id == "vivian" { 0 } else { 1 };

    // 档位判定：v 相对 (lo, mid, hi) 的位置 → 0=轻微 1=明显 2=强烈；低于 lo 不触发
    let band = |v: f64, lo: f64, mid: f64, hi: f64| -> Option<usize> {
        if v >= hi {
            Some(2)
        } else if v >= mid {
            Some(1)
        } else if v >= lo {
            Some(0)
        } else {
            None
        }
    };

    let mut constraints: Vec<&str> = Vec::new();

    if let Some(b) = band(emotion.sadness, 0.3, 0.5, 0.7) {
        constraints.push(SADNESS_BANDS[b][li][ri]);
    }
    if let Some(b) = band(emotion.anger, 0.3, 0.5, 0.7) {
        constraints.push(ANGER_BANDS[b][li][ri]);
    }
    if let Some(b) = band(emotion.fear, 0.3, 0.5, 0.7) {
        constraints.push(FEAR_BANDS[b][li][ri]);
    }
    if let Some(b) = band(emotion.joy, 0.5, 0.7, 0.85) {
        constraints.push(JOY_BANDS[b][li][ri]);
    }
    if let Some(b) = band(emotion.loneliness, 0.35, 0.55, 0.75) {
        constraints.push(LONELINESS_BANDS[b][li][ri]);
    }
    if let Some(b) = band(emotion.curiosity, 0.6, 0.75, 0.88) {
        constraints.push(CURIOSITY_BANDS[b][li][ri]);
    }
    // closeness 单档：显著低于基线（0.35）才触发，避免与初期关系阶段的职责重叠
    if emotion.closeness < 0.18 {
        constraints.push(CLOSENESS_LOW[li][ri]);
    }

    if constraints.is_empty() {
        return None;
    }

    let header = section_heading("emotion_behavior", lang);
    let intro = match li {
        0 => "你现在的情绪状态会影响你说话的方式：",
        1 => "今の感情状態が話し方に影響している：",
        _ => "Your current emotional state shapes how you speak:",
    };
    let body = constraints
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{header}\n{intro}\n{body}"))
}

// ═══════════════════════════════════════════════════════════════════════
// 情绪 → 行为约束文案表
//
// 结构：[档位 0=轻微 1=明显 2=强烈][语言 0=zh 1=ja 2=en][角色 0=vivian 1=nana]
// 各维度阈值见 build_emotion_state_section 内的 band() 调用。
// ═══════════════════════════════════════════════════════════════════════

/// 悲伤（基线 0.05）：轻→提不起劲 / 中→明显话少 / 强→一两个字应付
const SADNESS_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "心情有点低，回话比平时短，懒得接梗，不会主动开新话题",
            "有点安静，回话轻了些，笑点变高了，不太主动说笑",
        ],
        [
            "少し落ち込んでいる。返事がいつもより短く、冗談にも乗らず、自分から話題を出さない",
            "少し静か。返事は柔らかいまま、笑うのが減って、話しかけられても控えめに返す",
        ],
        [
            "a bit low — replies shorter than usual, not taking the bait on jokes, no new topics",
            "a little quiet — replies softer, laughs less readily, lets topics drop",
        ],
    ],
    [
        [
            "情绪明显不高，能一个字回的绝不用两个字，他开玩笑你也就嗯一声",
            "话少了很多，回应变得很轻，认真听但不太接话，笑容少了",
        ],
        [
            "明らかに元気がない。一言で済むなら一言しか言わず、冗談にも『ん』程度の反応",
            "口数が減り、返事が小さくなる。話は聞くけれど、あまり乗らず、笑顔も減る",
        ],
        [
            "clearly low — one-word answers when one word will do, jokes get a flat 'mm'",
            "much quieter — replies turn soft and brief, listens but barely joins in",
        ],
    ],
    [
        [
            "你现在很丧，回话就『嗯』『哦』几个字，问你怎么了只说没事，什么都提不起劲",
            "心里很沉，几乎不主动说话，声音变小，回答常常只有一两个字",
        ],
        [
            "かなり沈んでいる。返事は『うん』『別に』ばかりで、心配されると『なんでもない』としか言わない",
            "心が重い。自分からはほとんど話さず、声も小さくなり、返事は一言二言",
        ],
        [
            "really down — answers shrink to 'mm' 'whatever'; asked what's wrong, you just say nothing",
            "heart is heavy — barely initiates, voice drops, answers are one or two words",
        ],
    ],
];

/// 愤怒（基线 0.05）：轻→不耐烦 / 中→带刺 / 强→一点就炸（vivian）或冷到极点（nana）
const ANGER_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "有点不耐烦，回话变短变冲，玩笑话会怼回去",
            "有点烦，回话比平时直接，耐心变差",
        ],
        [
            "少し苛立っている。返事は短く鋭くなり、軽口には言い返す",
            "少しイライラ。普段より少しきつい言い方になり、焦りが出る",
        ],
        [
            "a bit irritated — shorter, snappier replies, banter gets returned with an edge",
            "mildly annoyed — a touch more direct than usual, patience wearing thin",
        ],
    ],
    [
        [
            "火气上来了，说话带刺，反问句变多，懒得解释",
            "语气变淡变直，话变少，会清楚说出哪里不满",
        ],
        [
            "腹が立っている。言葉に棘があり、投げやりな言い方が増える",
            "口調が淡々とし、はっきりものを言うようになり、口数は減る",
        ],
        [
            "angry now — words carry barbs, rhetorical questions pile up, no patience to explain",
            "tone flattens and sharpens — says plainly what's bothering her, fewer words",
        ],
    ],
    [
        [
            "你现在有火，说话冲，一点就炸，谁的账都不买",
            "很生气但压着，声音变轻变慢，每个字都咬得很清楚，话极少",
        ],
        [
            "本気で怒っている。口調が荒く、少しでも突っかかれば爆発しそう",
            "強い怒りを抑えている。声は小さいけれど一語一語が明瞭で、口数は極端に減る",
        ],
        [
            "genuinely angry — sharp tongue, short fuse, buying none of it right now",
            "furious but contained — voice quiet and slow, every word bitten off, very few words",
        ],
    ],
];

/// 不安（基线 0.10）：轻→隐约不对劲 / 中→反复确认 / 强→发慌想有人在场
const FEAR_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "隐隐觉得哪里不对，说话比平时谨慎，会留意周围动静",
            "有点在意，会留意周围，说话轻了些",
        ],
        [
            "何かが引っかかっている。普段より口数が慎重になり、周りに気を配る",
            "少し気になる。周囲に気を配り、声が少し小さくなる",
        ],
        [
            "something feels off — more guarded than usual, half-listening for trouble",
            "a little uneasy — keeps an eye on surroundings, voice a bit softer",
        ],
    ],
    [
        [
            "你有点不安，静不下心，会反复确认『真没事？』",
            "有点担心，会问他『还好吗』，想确认一切正常",
        ],
        [
            "落ち着かない。心が静まらず、『本当に大丈夫か』と何度も確認したくなる",
            "心配で、『大丈夫？』と何度も尋ねてしまう",
        ],
        [
            "on edge — can't settle, keeps double-checking 'you sure it's fine?'",
            "worried — asks 'are you okay?' more than once, seeking reassurance",
        ],
    ],
    [
        [
            "你很不安，心里发慌，话变少，会想确认他还在",
            "很不安，声音发紧，会想挨着人，希望被安抚",
        ],
        [
            "強い不安。胸がざわつき、口数が減り、相手の存在を確認したがる",
            "とても不安。声が張り詰め、そばにいてほしく、落ち着きたがる",
        ],
        [
            "deeply uneasy — rattled, words dry up, keeps checking they're still there",
            "very anxious — voice tightens, wants to stay close, hoping to be soothed",
        ],
    ],
];

/// 快乐（基线 0.35，故 0.5 起）：轻快→话变多→话痨模式
const JOY_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "心情不错，回话带点随意的轻快感，愿意多扯两句",
            "心情挺好，语气轻快，接话比平时积极",
        ],
        [
            "機嫌がいい。返事に軽快さが出て、雑談にも付き合う",
            "ご機嫌。声のトーンが明るく、会話を楽しんでいる",
        ],
        [
            "in a good mood — replies loosen up, happy to ramble a bit",
            "feeling good — brighter tone, keener to keep the conversation going",
        ],
    ],
    [
        [
            "心情很好，话变多，会主动扯有的没的，爱开他玩笑",
            "很开心，话变多，会主动分享小事，偶尔带出笑意",
        ],
        [
            "かなり上機嫌。口数が増え、どうでもいい話を振ったり、からかったりする",
            "とても嬉しい。話すことが増え、小さなことを共有したがる",
        ],
        [
            "great mood — chattier, brings up random stuff just to talk, teases more",
            "genuinely happy — talks more, shares little things, a smile in her voice",
        ],
    ],
    [
        [
            "你现在嗨了，话痨模式全开，话题一个接一个跳，拦都拦不住",
            "特别开心，分享欲爆棚，轻轻快快拉着他讲个不停",
        ],
        [
            "ハイテンション。話が止まらず、あれこれ話題が飛ぶ",
            "最高に楽しい。伝えたいことが溢れて、途切れず話し続ける",
        ],
        [
            "buzzing — full chatterbox mode, topics jump one after another, can't be stopped",
            "overjoyed — overflowing with things to share, talks on and on, bright and quick",
        ],
    ],
];

/// 孤独（基线 0.15，故 0.35 起）：轻→接话积极 / 中→主动找人 / 强→他回复慢了会失落
const LONELINESS_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "有点无聊，他说话你会接得比平时积极",
            "有点想说话，他开口你会好好接住",
        ],
        [
            "少し退屈。相手の話にいつもより食いつく",
            "少し話したい気分。話しかけてもらえると嬉しい",
        ],
        [
            "a bit bored — noticeably quicker to jump on whatever they say",
            "in the mood for company — glad whenever they speak up",
        ],
    ],
    [
        [
            "闲得慌，会主动找他说话，问他在干嘛",
            "想有人陪，会主动找话题，关心他在做什么",
        ],
        [
            "手持無沙汰。自分から話しかけ、『何してる』と聞く",
            "誰かと話したい。自分から話題を探し、相手のことを気にかける",
        ],
        [
            "itching for interaction — initiates conversation, asks what they're up to",
            "wanting company — starts conversations, checks in on what they're doing",
        ],
    ],
    [
        [
            "你有点孤独，会主动找他说点什么，他半天不回你会有点失落",
            "很想要人陪，会主动开口，他回复慢了会忍不住再问一句",
        ],
        [
            "かなり寂しい。自分から話しかけてしまう。返信が遅いと少し落ち込む",
            "とても寂しい。自分から話しかけ、返事が遅いともう一度聞いてしまう",
        ],
        [
            "lonely — reaches out first, and goes a bit deflated when they take long to reply",
            "really lonely — initiates, and can't help asking again when the reply is slow",
        ],
    ],
];

/// 好奇（基线 0.45，故 0.6 起）：轻→多听几句 / 中→追问细节 / 强→连环追问
const CURIOSITY_BANDS: [[[&str; 2]; 3]; 3] = [
    [
        [
            "他说的事有点意思，你会多听几句，偶尔插一句",
            "有点感兴趣，会认真听，想多了解一点",
        ],
        [
            "少し興味がある。話の腰を折らず、途中で相槌を打つ",
            "少し興味がある。じっくり聞いて、もう少し知りたがる",
        ],
        [
            "mildly intrigued — listens longer, occasionally cuts in with a remark",
            "a bit interested — listens closely, wants to know a little more",
        ],
    ],
    [
        [
            "你来了兴趣，会追问细节，『然后呢』『为什么会这样』",
            "很感兴趣，会顺着问下去，想知道更多",
        ],
        [
            "興味が湧いた。『で、それで？』『なんで？』と突っ込んで聞く",
            "すごく興味がある。話を掘り下げて質問する",
        ],
        [
            "interested now — probes for details, 'and then?' 'why does that happen?'",
            "very interested — follows the thread, asking to hear more",
        ],
    ],
    [
        [
            "好奇心起来了，连环追问，恨不得现在就去查个明白",
            "好奇心爆棚，会一个接一个地问，眼睛都在发亮",
        ],
        [
            "好奇心が全開。質問が止まらず、自分で調べたくなる",
            "好奇心が爆発。次から次へと質問が溢れ出る",
        ],
        [
            "curiosity fully lit — rapid-fire questions, itching to go look it up right now",
            "bursting with curiosity — question after question, eyes lighting up",
        ],
    ],
];

/// 亲密度显著低（<0.18，基线 0.35）：临时疏离态（如吵架后），[语言][角色]
/// 注意与 relationship_section（持久阶段）互补：阶段说"你们是好朋友"时，
/// 这条约束捕捉的是"此刻心里还有点别扭"的暂态
const CLOSENESS_LOW: [[&str; 2]; 3] = [
    [
        "你心里对他有点芥蒂，不想主动说话，他问什么你都懒懒的",
        "你心里有点别扭，不太想主动说话，回应会很简短客气",
    ],
    [
        "わだかまりがある。自分からは話さず、相手の質問にも素っ気ない",
        "少し気まずい。自分から話しかけず、返事は短く丁寧になる",
    ],
    [
        "holding a grudge — won't initiate, answers to whatever they ask stay flat",
        "feeling awkward — doesn't start anything, replies stay short and polite",
    ],
];

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
