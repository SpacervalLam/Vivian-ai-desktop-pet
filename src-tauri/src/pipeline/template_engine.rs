//! Prompt 模板引擎 —— Section 定义 + 元数据驱动渲染
//!
//! ## 设计目标
//!
//! 将 prompt 的"结构定义"（有哪些 section、顺序、层级、是否可选）与"内容填充"
//! （PersonaEngine/MemoryManager 等引擎输出）解耦。Section 定义作为单一真相源，
//! 同时驱动：
//!
//! 1. **后端 prompt 组装** —— `build_prompt_with_sections()` 按定义顺序组装
//! 2. **前端 Context Pipeline 可视化** —— `section_schema()` 通过 Tauri 命令暴露
//! 3. **模板预览** —— `system_prompt.tera` 作为人类可读的结构参考
//!
//! ## 与现有 build_prompt 的关系
//!
//! - `build_prompt()` 保持不变（向后兼容），内部委托给 `build_prompt_with_sections()`
//! - `build_prompt_with_sections()` 返回 `PromptRenderResult`，含最终 prompt + 每个
//!   section 的元数据（char_count, token_estimate, present）
//! - `PromptBuildingStep::ainvoke()` 使用 enriched 版本，自动获得 section breakdown

use serde::{Deserialize, Serialize};

use super::prompt_modules::{self, PromptBuilder, PromptParts};
use crate::utils::token_estimate;

// ========== Section 定义 ==========

/// Section 类型（静态区 / 动态区）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SectionType {
    /// 静态区（API 可缓存，`<static>` 标签内）
    Static,
    /// 动态区（每轮变化，`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 之后）
    Dynamic,
}

/// Section 层级（对应八层意识模型 + 尾部指令）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SectionLayer {
    /// 框架规则：安全规则、会话规则、输出格式
    Framework,
    /// 角色设定：身份/人格/背景/风格/示例
    Character,
    /// 心智状态：当前心智、工作记忆、自我状态、情绪、内心反应
    Mind,
    /// 世界感知：环境、用户、世界书、事件、室友、观察
    World,
    /// 社交关系：关系定义、社交状态、关系事实、共享世界
    Relationship,
    /// 记忆经历：相关经历、关系日志、记忆上下文、记忆规则
    Memory,
    /// 用户画像：用户事实、动态行为
    UserProfile,
    /// 生成引导：响应决策、工具、渠道指南、在场指南、用户输入
    Generation,
}

impl SectionLayer {
    /// 返回 snake_case 字符串表示（与 serde rename_all = "snake_case" 一致）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Framework => "framework",
            Self::Character => "character",
            Self::Mind => "mind",
            Self::World => "world",
            Self::Relationship => "relationship",
            Self::Memory => "memory",
            Self::UserProfile => "user_profile",
            Self::Generation => "generation",
        }
    }
}

/// 单个 Section 定义（模板引擎的结构单元）
#[derive(Debug, Clone, Serialize)]
pub struct SectionDef {
    /// 唯一标识（snake_case）
    pub id: &'static str,
    /// 显示名称（英文，用于 PromptBreakdown 和前端 fallback）
    pub name: &'static str,
    /// i18n key（前端 `prompt_section.{key}` 查找翻译）
    pub i18n_key: &'static str,
    /// 所属层级
    pub layer: SectionLayer,
    /// 静态区 / 动态区
    pub section_type: SectionType,
    /// 是否为条件注入（Some 时才出现）
    pub optional: bool,
}

/// 单个 Section 的渲染结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionRenderInfo {
    /// Section 唯一 ID
    pub id: String,
    /// 显示名称
    pub name: String,
    /// i18n key
    pub i18n_key: String,
    /// 层级
    pub layer: SectionLayer,
    /// 静态/动态
    pub section_type: SectionType,
    /// 是否为可选 section
    pub optional: bool,
    /// 实际内容（空字符串表示未注入）
    pub content: String,
    /// 字符数
    pub char_count: usize,
    /// 估算 token 数
    pub token_estimate: usize,
    /// 本次是否实际注入（optional=false 的 section 总是 true）
    pub present: bool,
}

/// Prompt 渲染结果（最终 prompt + 各 section 元数据）
#[derive(Debug, Clone)]
pub struct PromptRenderResult {
    /// 最终组装后的完整 prompt
    pub prompt: String,
    /// 各 section 的渲染信息（按组装顺序）
    pub sections: Vec<SectionRenderInfo>,
    /// 总字符数
    pub total_chars: usize,
    /// 总估算 token 数
    pub total_tokens: usize,
}

// ========== Section Schema（单一真相源）==========

/// Prompt 模板结构信息（通过 Tauri 命令暴露给前端）
#[derive(Debug, Clone, Serialize)]
pub struct PromptSectionSchema {
    /// 所有 section 定义（按组装顺序）
    pub sections: Vec<SectionDef>,
    /// 总 section 数
    pub total_count: usize,
    /// 静态 section 数
    pub static_count: usize,
    /// 动态 section 数
    pub dynamic_count: usize,
    /// 条件 section 数
    pub optional_count: usize,
}

/// 获取 Prompt section schema（单一真相源）
///
/// 定义了所有 section 的 ID、名称、i18n key、层级、类型和可选性。
/// 此函数是 section 结构的唯一真相源，被以下场景消费：
///
/// - `build_prompt_with_sections()` —— 按此顺序组装并生成元数据
/// - `get_prompt_section_schema` Tauri 命令 —— 前端 Context Pipeline 展示
/// - `build_prompt_template_preview()` —— 模板预览使用相同的 section 列表
pub fn section_schema() -> PromptSectionSchema {
    let sections = vec![
        // ── 框架规则：安全规则、会话规则、输出格式 ──
        SectionDef {
            id: "framework",
            name: "Framework",
            i18n_key: "framework",
            layer: SectionLayer::Framework,
            section_type: SectionType::Static,
            optional: false,
        },
        SectionDef {
            id: "output_format",
            name: "Output Format",
            i18n_key: "output_format",
            layer: SectionLayer::Framework,
            section_type: SectionType::Static,
            optional: false,
        },

        // ── 角色设定：身份/人格/背景/风格/示例 ──
        SectionDef {
            id: "character",
            name: "Character",
            i18n_key: "character",
            layer: SectionLayer::Character,
            section_type: SectionType::Static,
            optional: true,
        },
        SectionDef {
            id: "style",
            name: "Style",
            i18n_key: "style",
            layer: SectionLayer::Character,
            section_type: SectionType::Static,
            optional: true,
        },
        SectionDef {
            id: "examples",
            name: "Examples",
            i18n_key: "examples",
            layer: SectionLayer::Character,
            section_type: SectionType::Static,
            optional: true,
        },

        // ── 生成引导（伪静态归位）：响应决策/渠道指南/内联标签 ──
        // 内容不随轮次变化，已移入 <static> 区随前缀缓存复用
        SectionDef {
            id: "response_decision",
            name: "Response Decision",
            i18n_key: "response_decision",
            layer: SectionLayer::Generation,
            section_type: SectionType::Static,
            optional: false,
        },
        SectionDef {
            id: "channel_guide",
            name: "Channel Guide",
            i18n_key: "channel_guide",
            layer: SectionLayer::Generation,
            section_type: SectionType::Static,
            optional: true,
        },
        SectionDef {
            id: "inline_tag_format",
            name: "Inline Tag Format",
            i18n_key: "inline_tag_format",
            layer: SectionLayer::Generation,
            section_type: SectionType::Static,
            optional: true,
        },

        // ── 心智状态：当前心智、工作记忆、自我状态、情绪、内心反应 ──
        SectionDef {
            id: "mind",
            name: "Current Mind",
            i18n_key: "mind",
            layer: SectionLayer::Mind,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "working_memory",
            name: "Working Memory",
            i18n_key: "working_memory",
            layer: SectionLayer::Mind,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "self_state",
            name: "Self State",
            i18n_key: "self_state",
            layer: SectionLayer::Mind,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "emotion",
            name: "Emotion Context",
            i18n_key: "emotion",
            layer: SectionLayer::Mind,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "inner_reaction",
            name: "Inner Reaction",
            i18n_key: "inner_reaction",
            layer: SectionLayer::Mind,
            section_type: SectionType::Dynamic,
            optional: true,
        },

        // ── 记忆经历（合并组）：episode + 关系日志 + 记忆本体（+首见/使用规则）──
        // 已上移至 Mind 之后（注意力黄金位置），并合并为单一 section
        SectionDef {
            id: "memory_group",
            name: "Memory Group",
            i18n_key: "memory_group",
            layer: SectionLayer::Memory,
            section_type: SectionType::Dynamic,
            optional: false,
        },

        // ── 世界感知：环境、用户、世界书、事件、观察、室友 ──
        SectionDef {
            id: "environment",
            name: "Environment Context",
            i18n_key: "environment",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: false,
        },
        SectionDef {
            id: "user",
            name: "User",
            i18n_key: "user",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "worldbook",
            name: "Worldbook",
            i18n_key: "worldbook",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "environment_events",
            name: "Environment Events",
            i18n_key: "environment_events",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "user_research",
            name: "User Research",
            i18n_key: "user_research",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "roommate_status",
            name: "Roommate Status",
            i18n_key: "roommate_status",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "roommate_cognitive",
            name: "Roommate Cognitive",
            i18n_key: "roommate_cognitive",
            layer: SectionLayer::World,
            section_type: SectionType::Dynamic,
            optional: true,
        },

        // ── 社交关系：关系定义、社交状态、关系事实、共享世界 ──
        SectionDef {
            id: "relationship",
            name: "Relationship",
            i18n_key: "relationship",
            layer: SectionLayer::Relationship,
            section_type: SectionType::Static,
            optional: true,
        },
        SectionDef {
            id: "social_state",
            name: "Social State",
            i18n_key: "social_state",
            layer: SectionLayer::Relationship,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "relationship_facts",
            name: "Relationship Facts",
            i18n_key: "relationship_facts",
            layer: SectionLayer::Relationship,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "shared_world",
            name: "Shared World",
            i18n_key: "shared_world",
            layer: SectionLayer::Relationship,
            section_type: SectionType::Dynamic,
            optional: true,
        },

        // ── 用户画像（合并组）：用户事实 + 认知模型 + 动态行为 ──
        SectionDef {
            id: "user_profile_group",
            name: "User Profile Group",
            i18n_key: "user_profile_group",
            layer: SectionLayer::UserProfile,
            section_type: SectionType::Dynamic,
            optional: true,
        },

        // ── 生成引导（动态）：在场指南、语气注入、感知引导、推荐工具、工具、用户输入 ──
        SectionDef {
            id: "presence_guide",
            name: "Presence Guide",
            i18n_key: "presence_guide",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "tone_injection",
            name: "Tone Injection",
            i18n_key: "tone_injection",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "fast_perception_guidance",
            name: "Fast Perception Guidance",
            i18n_key: "fast_perception_guidance",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "recommended_tools",
            name: "Recommended Tools",
            i18n_key: "recommended_tools",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: true,
        },
        SectionDef {
            id: "tools",
            name: "Tools",
            i18n_key: "tools",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: false,
        },
        SectionDef {
            id: "user_input",
            name: "User Input",
            i18n_key: "user_input",
            layer: SectionLayer::Generation,
            section_type: SectionType::Dynamic,
            optional: true,
        },
    ];

    let total_count = sections.len();
    let static_count = sections
        .iter()
        .filter(|s| s.section_type == SectionType::Static)
        .count();
    let dynamic_count = total_count - static_count;
    let optional_count = sections.iter().filter(|s| s.optional).count();

    PromptSectionSchema {
        sections,
        total_count,
        static_count,
        dynamic_count,
        optional_count,
    }
}

// ========== Enriched Prompt Builder ==========

/// 带 section 元数据的 prompt 构建
///
/// 与 `PromptBuilder::build_prompt()` 产生**完全相同的 prompt 字符串**，
/// 同时返回每个 section 的元数据（char_count、token_estimate、present）。
///
/// `PromptBuildingStep::ainvoke()` 使用此函数，自动获得 section breakdown，
/// 无需手动维护硬编码的 JSON 列表。
pub fn build_prompt_with_sections(parts: &PromptParts) -> PromptRenderResult {
    // 调用原有的 build_prompt 获得最终 prompt（保证行为一致）
    let prompt = PromptBuilder::build_prompt(parts);

    // 按 schema 定义的顺序，逐 section 提取内容并计算元数据
    let schema = section_schema();
    let mut sections = Vec::with_capacity(schema.sections.len());

    for def in &schema.sections {
        let content = extract_section_content(def.id, parts, &parts.language);
        let present = !content.trim().is_empty();
        let char_count = content.chars().count();
        let token_estimate = token_estimate::estimate_tokens(&content);

        sections.push(SectionRenderInfo {
            id: def.id.to_string(),
            name: def.name.to_string(),
            i18n_key: def.i18n_key.to_string(),
            layer: def.layer,
            section_type: def.section_type,
            optional: def.optional,
            content,
            char_count,
            token_estimate,
            present: if def.optional { present } else { true },
        });
    }

    let total_chars = prompt.chars().count();
    let total_tokens = token_estimate::estimate_tokens(&prompt);

    PromptRenderResult {
        prompt,
        sections,
        total_chars,
        total_tokens,
    }
}

/// 根据 section ID 从 PromptParts 中提取对应的内容
///
/// 这里复现 `build_prompt()` 中每个 section 的内容格式化逻辑。
/// 与 `build_prompt()` 保持同步——如果 build_prompt 的格式化变了，这里也要变。
fn extract_section_content(id: &str, parts: &PromptParts, lang: &str) -> String {
    use prompt_modules::*;

    match id {
        "framework" => {
            if parts.enable_instructions {
                String::new()
            } else {
                let mut framework_parts = vec![
                    safety_rules().to_string(),
                    session_rules().to_string(),
                    address_rules().to_string(),
                    conversation_rhythm().to_string(),
                    speaker_prefix().to_string(),
                    chat_style_framework().to_string(),
                ];
                if let Some(preset) = parts.style_preset_block.as_deref() {
                    if !preset.trim().is_empty() {
                        framework_parts.push(preset.to_string());
                    }
                }
                format!(
                    "[FRAMEWORK - DO NOT EMBODY, JUST FOLLOW]\n{}\n[END FRAMEWORK]",
                    framework_parts.join("\n\n")
                )
            }
        },
        "output_format" => {
            // 与 build_prompt 保持同步：原生 Schema 路径或原生 FC 路径（有工具）下不注入 OUTPUT_FORMAT
            if parts.has_native_schema
                || (parts.enable_native_fc
                    && parts.tools.as_deref().map_or(false, |t| !t.is_empty()))
            {
                String::new()
            } else {
                format!(
                    "[FORMAT SPEC - DO NOT EMBODY]\n{}\n[END FORMAT]",
                    output_format()
                )
            }
        },
        "character" => parts
            .character_block
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "style" => parts.style_block.as_deref().unwrap_or("").to_string(),
        "relationship" => parts
            .relationship_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "examples" => parts
            .examples_block
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "mind" => parts.mind_section.as_deref().unwrap_or("").to_string(),
        "working_memory" => parts
            .working_memory_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "self_state" => parts
            .self_state_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "emotion" => parts
            .emotion_context
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "inner_reaction" => {
            let has_thought = parts
                .working_memory_section
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if has_thought {
                String::new()
            } else {
                parts
                    .inner_reaction
                    .as_deref()
                    .unwrap_or("")
                    .to_string()
            }
        },
        "environment" => {
            let env_ctx = parts
                .environment_context
                .clone()
                .unwrap_or_else(EnvironmentContext::now);
            build_context_block(&env_ctx, &parts.language)
        }
        "user" => {
            let mut user_parts: Vec<String> = Vec::new();
            if let Some(entity) = parts.user_entity_section.as_deref() {
                if !entity.trim().is_empty() {
                    // entity_state 已通过 section_heading("user_state", lang) 输出标题，
                    // 这里 strip 掉当前语言下的标题，统一改用 presence 标题包装
                    let user_state_heading = section_heading("user_state", lang);
                    let strip_pat = format!("{}\n", user_state_heading);
                    let content = entity.strip_prefix(&strip_pat).unwrap_or(entity);
                    user_parts.push(format!("{}\n{}", section_heading("presence", lang), content));
                }
            }
            if let Some(brief) = parts.activity_brief.as_deref() {
                if !brief.trim().is_empty() {
                    user_parts.push(format!("{}\n{}", section_heading("recent_activity", lang), brief));
                }
            }
            if user_parts.is_empty() {
                String::new()
            } else {
                format!("{}\n{}", section_heading("the_person", lang), user_parts.join("\n\n"))
            }
        },
        "roommate_status" => parts
            .roommate_status
            .as_deref()
            .map(|s| {
                format!(
                    "{}\n{}",
                    section_heading("who_else", lang),
                    s
                )
            })
            .unwrap_or_default(),
        "roommate_cognitive" => parts
            .roommate_cognitive_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "environment_events" => parts
            .environment_events
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "user_research" => parts
            .user_research
            .as_deref()
            .map(|o| {
                format!(
                    "{}\n{o}",
                    section_heading("learning_about_user", lang)
                )
            })
            .unwrap_or_default(),
        "relationship_facts" => parts
            .relationship_facts_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "shared_world" => parts
            .shared_world_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "social_state" => parts
            .social_state_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        // 记忆组合并 section（episode + relationship_log + 记忆本体 + 首见/使用规则）
        "memory_group" => build_memory_group_section(parts),
        // 画像组合并 section（user_facts + user_model + dynamic_behavior）
        "user_profile_group" => build_user_profile_group_section(parts),
        "worldbook" => parts
            .worldbook_block
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "channel_guide" => {
            if parts.channel.is_empty() {
                String::new()
            } else {
                build_channel_style_guide(&parts.channel)
            }
        }
        "presence_guide" => {
            if parts.presence_state.is_empty() {
                String::new()
            } else {
                build_presence_guide(&parts.presence_state)
            }
        }
        "response_decision" => {
            if parts.cross_character_mode {
                let voice_guide = build_cross_character_voice_guide(&parts.char_id);
                if voice_guide.is_empty() {
                    cross_character_response_decision().to_string()
                } else {
                    format!("{}\n\n{}", voice_guide, cross_character_response_decision())
                }
            } else {
                user_agent_response_decision().to_string()
            }
        }
        "inline_tag_format" => parts
            .inline_tag_section
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "tone_injection" => parts
            .tone_injection
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "fast_perception_guidance" => parts
            .fast_perception_guidance
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "recommended_tools" => parts
            .recommended_tools
            .as_deref()
            .unwrap_or("")
            .to_string(),
        "tools" => build_tools_block(parts.tools.as_deref(), parts.enable_native_fc, &parts.language),
        "user_input" => {
            if parts.user_input.is_empty() {
                String::new()
            } else {
                format!("{}\n{}", section_heading("user_input", lang), parts.user_input)
            }
        }
        _ => String::new(),
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_expected_counts() {
        let schema = section_schema();
        // 32 = 9 static + 23 dynamic
        // static: framework, output_format, character, style, examples,
        //         response_decision, channel_guide, inline_tag_format（伪静态归位）, relationship
        // 动态区合并组：memory_group（episode+log+memory+规则）、user_profile_group（facts+model+behavior）
        assert_eq!(schema.total_count, 32);
        assert_eq!(schema.static_count, 9);
        assert_eq!(schema.dynamic_count, 23);
        assert_eq!(schema.optional_count, 26);
    }

    #[test]
    fn schema_ids_are_unique() {
        let schema = section_schema();
        let mut ids: Vec<&str> = schema.sections.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), schema.sections.len(), "Section IDs must be unique");
    }

    #[test]
    fn schema_i18n_keys_are_unique() {
        let schema = section_schema();
        let mut keys: Vec<&str> = schema.sections.iter().map(|s| s.i18n_key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), schema.sections.len(), "i18n keys must be unique");
    }

    #[test]
    fn enriched_matches_build_prompt() {
        let parts = PromptParts {
            user_input: "你好".to_string(),
            memory_text: "一些记忆".to_string(),
            character_block: Some("我是 Vivian".to_string()),
            ..Default::default()
        };

        let original = PromptBuilder::build_prompt(&parts);
        let enriched = build_prompt_with_sections(&parts);

        // 最终 prompt 必须完全一致
        assert_eq!(original, enriched.prompt);
        assert_eq!(original.chars().count(), enriched.total_chars);
    }

    #[test]
    fn present_flags_correct() {
        let parts = PromptParts {
            user_input: "hello".to_string(),
            character_block: Some("I am Vivian".to_string()),
            emotion_context: None,
            ..Default::default()
        };

        let result = build_prompt_with_sections(&parts);

        // character 是 optional 且有内容 → present=true
        let character = result.sections.iter().find(|s| s.id == "character").unwrap();
        assert!(character.present);

        // emotion 是 optional 且为空 → present=false
        let emotion = result.sections.iter().find(|s| s.id == "emotion").unwrap();
        assert!(!emotion.present);

        // framework 不是 optional → present=true
        let framework = result.sections.iter().find(|s| s.id == "framework").unwrap();
        assert!(framework.present);
    }

    #[test]
    fn token_estimates_positive() {
        let parts = PromptParts {
            user_input: "test".to_string(),
            ..Default::default()
        };

        let result = build_prompt_with_sections(&parts);
        assert!(result.total_tokens > 0);

        // 每个 present section 的 token_estimate > 0
        for s in &result.sections {
            if s.present && !s.content.is_empty() {
                assert!(
                    s.token_estimate > 0,
                    "Section '{}' should have positive token estimate",
                    s.id
                );
            }
        }
    }
}
