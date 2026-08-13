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

/// 兜底输出规则（三语化）
fn fallback_output_rules(lang: &str) -> &'static str {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    match lang_norm {
        "en" => "Reply requirements: keep it concise and conversational. Don't use Markdown headings or lists — answer in a natural spoken tone.",
        "ja" => "返信要件：簡潔で口語的に。Markdown見出しやリストは使わず、自然な会話口調で答える。",
        _ => "回复要求：保持简洁、口语化，不要使用 Markdown 标题或列表语法，直接用自然对话的口吻回答。",
    }
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
        let (character_block, examples_block, style_block, style_preset_block) = match self.persona.as_ref() {
            Some(p) => {
                let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
                let intimacy = self
                    .psychology
                    .as_ref()
                    .map(|psy| psy.relationship().intimacy * 100.0)
                    .unwrap_or(0.0);
                let style = p.build_style_prompt(intimacy, hour);
                let cfg = p.get_config();
                let preset = crate::persona::prompt_render::render_style_preset_block(&cfg, &self.language);
                (Some(p.get_character_block()), Some(p.get_examples_block()), Some(style), if preset.is_empty() { None } else { Some(preset) })
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
        let inline_tag_section = if self.inline_expression_enabled {
            let names_text = self.expression_motion_names.as_deref().unwrap_or("Expressions: (none)\nMotions: (none)");
            let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
            Some(match lang {
                "en" => format!(
                    "[INLINE EXPRESSION TAGS - FORMAT SPEC]\n\
                    You can embed the following tags in your reply text to trigger expressions and motions; tags take effect instantly during streaming output:\n\
                    - Expression: <e name=\"expression_name\" dur=\"milliseconds\"/>  (dur is optional: 0=natural switch, 1500-3000=brief flash, 4000-6000=medium hold, 8000+=strong emotion)\n\
                    - Motion: <m name=\"motion_name\"/>\n\
                    - Sticker: <s name=\"sticker_name\"/>  (use once every 4-6 replies, not every turn)\n\n\
                    {names_text}\n\n\
                    Rules:\n\
                    - Embed tags at natural positions within text, not on a separate line\n\
                    - Don't use expressions/motions by default; only add when the reply has clear emotional tone\n\
                    - If nothing fits, don't force one — skip it\n\
                    - Tag names MUST be chosen from the list above; do not use any name not listed\n\
                    [END INLINE EXPRESSION TAGS]"
                ),
                "ja" => format!(
                    "[インライン表情タグ - フォーマット仕様]\n\
                    返信テキストに以下のタグを埋め込むことで、表情とモーションをトリガーできます。タグはストリーミング出力中に即座に効果を発揮します：\n\
                    - 表情：<e name=\"表情名\" dur=\"ミリ秒\"/>  (dur は省略可：0=自然切替、1500-3000=短フラッシュ、4000-6000=中持続、8000+=強感情)\n\
                    - モーション：<m name=\"モーション名\"/>\n\
                    - ステッカー：<s name=\"ステッカー名\"/>  (4-6返信に1回使用、毎回使用しない)\n\n\
                    {names_text}\n\n\
                    ルール：\n\
                    - タグはテキストの自然な位置に埋め込む、独立した行にしない\n\
                    - デフォルトでは表情/モーションを使用しない、明確な感情トーンがある場合のみ追加\n\
                    - 合わなければ無理に追加しない——スキップする\n\
                    - タグ名は上のリストから選ぶこと、リストにない名前は使用しない\n\
                    [終了インライン表情タグ]"
                ),
                _ => format!(
                    "[内联表情标签 - 格式规范]\n\
                    你可以在回复文本中嵌入以下标签来触发表情和动作，标签在流式输出时即时生效：\n\
                    - 表情：<e name=\"表情名\" dur=\"毫秒\"/>  (dur 可选：0=自然切换，1500-3000=短暂闪现，4000-6000=中等持续，8000+=强烈情绪)\n\
                    - 动作：<m name=\"动作名\"/>\n\
                    - 贴纸：<s name=\"贴纸名\"/>  (每 4-6 轮回复用一次，不要每轮都用)\n\n\
                    {names_text}\n\n\
                    规则：\n\
                    - 标签嵌入在文本的自然位置，不要单独成行\n\
                    - 默认不使用表情/动作；只在回复有明显情感色彩时添加\n\
                    - 如果不合适就不要强行加——跳过就好\n\
                    - 标签名必须从上面的列表中选择，不要使用未列出的名称\n\
                    [结束内联表情标签]"
                ),
            })
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

        // 情绪状态语言约束：从 PsychologyManager 获取 7 维情绪，
        // 翻译为具体的语言行为约束（如 sadness 高则话变少），合并到 tone_injection
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
            sections.push(fallback_output_rules(&self.language).to_string());
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
            state.output_format_fallback = Some(output_format(&self.language).to_string());
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


/// 根据当前情绪状态生成语言行为约束
///
/// 双梯度设计：强情绪（>0.6）明确改变说话方式，温和情绪（0.3-0.6）微妙影响语气。
/// 让 LLM 在大多数对话中都能感知到情绪对表达的影响，而非只有极端时刻才触发。
fn build_emotion_state_section(
    emotion: &crate::psychology::EmotionState,
    char_id: &str,
    lang: &str,
) -> Option<String> {
    let mut constraints: Vec<&str> = Vec::new();

    // ── 悲伤：强→话少丧气，温和→话偏轻 ──
    if emotion.sadness > 0.6 {
        constraints.push(if char_id == "vivian" {
            "你现在有点丧，话很少，不会主动找话题，回一两个字就完了"
        } else {
            "你现在心里有点沉，话更少更轻，不会主动开口"
        });
    } else if emotion.sadness > 0.3 {
        constraints.push(if char_id == "vivian" {
            "今天情绪不太高，话比平时少一点，不太想扯太多"
        } else {
            "今天稍微有点安静，回话会轻一些"
        });
    }

    // ── 愤怒：强→冲，温和→有点不耐烦 ──
    if emotion.anger > 0.6 {
        constraints.push(if char_id == "vivian" {
            "你现在有火，说话冲，不耐烦"
        } else {
            "你现在语气变淡了，声音更轻但每句话都清楚"
        });
    } else if emotion.anger > 0.3 {
        constraints.push(if char_id == "vivian" {
            "你现在有点烦躁，回话会短一点，不想多聊"
        } else {
            "你现在有点点烦，话会减少，语气偏淡"
        });
    }

    // ── 快乐：强→话多主动，温和→稍微轻松 ──
    if emotion.joy > 0.7 {
        constraints.push(if char_id == "vivian" {
            "你现在心情不错，话变多了，会主动扯有的没的"
        } else {
            "你现在心情很好，会多聊两句，偶尔带出笑意"
        });
    } else if emotion.joy > 0.3 {
        constraints.push(if char_id == "vivian" {
            "心情还行，回话带点随意的轻松感"
        } else {
            "心情还算不错，语气比平时温和一点"
        });
    }

    // ── 孤独：强→主动找人，温和→有点想说话 ──
    if emotion.loneliness > 0.7 {
        constraints.push(if char_id == "vivian" {
            "你有点孤独，会主动找他说点什么"
        } else {
            "你有点想有人陪着，会主动开口说点什么"
        });
    } else if emotion.loneliness > 0.3 {
        constraints.push(if char_id == "vivian" {
            "有点无聊，他说话你会接得比平时积极一点"
        } else {
            "如果有人说说话就好了，你会接话"
        });
    }

    // ── 亲密度低：远→不聊私事，稍远→有分寸 ──
    if emotion.closeness < 0.3 {
        constraints.push(if char_id == "vivian" {
            "你跟这个人还不太熟，不会主动聊私事"
        } else {
            "你还在了解这个人，不会主动聊太深的话题"
        });
    } else if emotion.closeness < 0.5 {
        constraints.push(if char_id == "vivian" {
            "跟这个人不算特别熟，聊私事会有分寸"
        } else {
            "跟这个人还在熟悉阶段，聊天会注意边界"
        });
    }

    // ── 好奇：强→追问，温和→有兴趣 ──
    if emotion.curiosity > 0.7 {
        constraints.push(if char_id == "vivian" {
            "你对他说的事好奇，会追问"
        } else {
            "你对他说的事感兴趣，会多问几句"
        });
    } else if emotion.curiosity > 0.3 {
        constraints.push(if char_id == "vivian" {
            "他说的事有点意思，你会多听几句"
        } else {
            "你对他说的话有点在意，会认真听"
        });
    }

    if constraints.is_empty() {
        return None;
    }

    let header = section_heading("emotion_state", lang);
    let intro = "你现在的情绪状态会影响你说话的方式：";
    let body = constraints
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{header}\n{intro}\n{body}"))
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
