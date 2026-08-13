//! 人格引擎 - 管理桌宠的人格配置、风格约束、场景选择
//!
//! 三层架构（参考 SillyTavern V2 角色卡 + Persona Consistency 记忆引擎分层）：
//! - `PersonaConfig`：纯数据 schema（identity / expression / scene 三层），不承担渲染职责
//! - `prompt_render`：唯一的人设 prompt 渲染器（数据 → prompt 文本）
//! - `SceneModeSelector`：5 信号融合选择场景模式 + 稳定性窗口
//! - `PersonaEngine`：协调上述组件，供 Brain / PromptBuilder 调用
//!
//! 持久化：`%APPDATA%\Vivian\persona\persona.json`

pub mod dynamic_profile;
pub mod evolution;
pub mod persona_card;
pub mod persona_decision;
pub mod prompt_render;
pub mod schemas;
pub mod scene_selector;
pub mod tone_injector;
pub mod worldbook;

pub use dynamic_profile::{AcquiredBehavior, AcquiredBehaviorCategory, DynamicBehaviorProfile};
pub use evolution::{EvolutionCandidate, EvolutionEntry, PersonaEvolution, PersonaEvolutionStore};
pub use persona_card::{CardStatus, PersonaCard, PersonaCardStore, PersonaEvent};
pub use persona_decision::PersonaDecisionWeights;
pub use prompt_render::StylePreset;
pub use tone_injector::ToneInjector;
pub use schemas::{
    CharacterExpression, FewShotExample, FewShotExamplesConfig, FewShotIntent, IdentityLayer,
    LanguageStyle, PersonaConfig, PerformanceRule, SceneMode, SceneModeConfig,
    DEFAULT_NANA_PERSONA, DEFAULT_PERSONA, default_persona_for,
};
pub use scene_selector::{SceneContext, SceneModeSelector};

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::error::{VivianError, VivianResult};

/// 人格引擎 — 协调 PersonaConfig 和 SceneModeSelector，供 Brain / PromptBuilder 调用
pub struct PersonaEngine {
    config: Arc<RwLock<PersonaConfig>>,
    scene_selector: Mutex<SceneModeSelector>,
    current_scene_mode: RwLock<SceneMode>,
    persistence_path: std::path::PathBuf,
    /// 人格卡片存储（表达侧面演化）
    card_store: PersonaCardStore,
    /// 基准表达值（首次加载时的原始人设，intimacy 调整在其上做混合而非覆盖）
    base_expression: RwLock<CharacterExpression>,
    /// 界面语言（影响风格约束块标题语言）
    language: RwLock<String>,
    /// 自我进化覆盖层（独立于原始人设文件，智能体反思中自行调整语气/性格）
    evolution: PersonaEvolutionStore,
}

impl PersonaEngine {
    /// 创建并加载持久化配置；文件不存在时使用 char_id 对应的默认人设
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let persona_dir = crate::utils::path::get_character_data_dir(char_id).join("persona");
        std::fs::create_dir_all(&persona_dir).map_err(|e| {
            VivianError::Memory(format!("创建人格目录失败: {e}"))
        })?;

        let persistence_path = persona_dir.join("persona.json");
        let config = if persistence_path.exists() {
            Self::load_from_for(&persistence_path, char_id).unwrap_or_else(|e| {
                tracing::warn!("加载人格配置失败，使用默认值: {e}");
                schemas::default_persona_for(char_id)
            })
        } else {
            schemas::default_persona_for(char_id)
        };

        let scene_selector =
            SceneModeSelector::with_modes(config.scene_modes.clone());

        let card_store = PersonaCardStore::new(char_id).unwrap_or_else(|e| {
            tracing::warn!("[PersonaEngine] 人格卡片存储初始化失败，使用内存模式: {e}");
            PersonaCardStore::fallback()
        });

        let evolution = PersonaEvolutionStore::new(char_id).unwrap_or_else(|e| {
            tracing::warn!("[PersonaEngine] 自我进化覆盖层初始化失败，使用内存模式: {e}");
            PersonaEvolutionStore::fallback()
        });

        tracing::info!(
            "[PersonaEngine] 初始化完成: name={}, role={}, taboos={}, scenes={}",
            config.identity.name,
            config.identity.role,
            config.identity.taboos.len(),
            config.scene_modes.len()
        );

        Ok(Self {
            config: Arc::new(RwLock::new(config.clone())),
            scene_selector: Mutex::new(scene_selector),
            current_scene_mode: RwLock::new(SceneMode::DailyChat),
            persistence_path,
            card_store,
            base_expression: RwLock::new(config.expression),
            language: RwLock::new("zh".to_string()),
            evolution,
        })
    }

    fn load_from_for(path: &std::path::Path, char_id: &str) -> VivianResult<PersonaConfig> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| VivianError::Memory(format!("读取人格文件失败: {e}")))?;
        if content.trim().is_empty() {
            return Ok(schemas::default_persona_for(char_id));
        }
        let mut config: PersonaConfig = serde_json::from_str(&content)
            .map_err(|e| VivianError::Memory(format!("解析人格文件失败: {e}")))?;

        if !config._examples_definition_compat.trim().is_empty() && config.few_shot_examples.examples.is_empty() {
            let parsed = prompt_render::parse_examples_markdown(&config._examples_definition_compat);
            config.few_shot_examples = parsed;
            config._examples_definition_compat = String::new();
        }

        Ok(config)
    }

    fn save_to(&self) -> VivianResult<()> {
        let config = self.config.read();
        let json = serde_json::to_string_pretty(&*config)
            .map_err(|e| VivianError::Memory(format!("序列化人格配置失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入人格临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换人格文件失败: {e}")))?;
        Ok(())
    }

    pub fn get_config(&self) -> PersonaConfig {
        self.config.read().clone()
    }

    pub fn set_language(&self, lang: &str) {
        *self.language.write() = lang.to_string();
    }

    /// 获取当前界面语言（供外部模块读取用于 prompt 三语化）
    pub fn get_language(&self) -> String {
        self.language.read().clone()
    }

    pub fn set_config(&self, config: PersonaConfig) -> VivianResult<()> {
        self.reload_config_internal(config);
        self.save_to()?;
        Ok(())
    }

    /// 切换风格预设并持久化
    ///
    /// 非法名称回退到 default。切换后立即落盘，下次启动仍生效。
    pub fn set_style_preset(&self, name: &str) -> VivianResult<()> {
        {
            let mut config = self.config.write();
            config.style_preset = if prompt_render::StylePreset::from_str(name).is_some() {
                name.to_string()
            } else {
                "default".to_string()
            };
        }
        self.save_to()
    }

    /// 读取当前激活的风格预设名称
    pub fn get_style_preset(&self) -> String {
        self.config.read().style_preset.clone()
    }

    /// 设置角色段落覆盖文本并持久化
    ///
    /// section_key 使用 CharacterSection::as_key() 返回的值：
    /// identity/personality/background/interests/appearance/speech/relationships。
    /// 注意：examples 不再通过此方法设置，请使用 set_few_shot_examples。
    /// 传入空串则回退到出厂默认。
    pub fn set_section_definition(&self, section_key: &str, body: &str) -> VivianResult<()> {
        use crate::persona::prompt_render::CharacterSection;
        let section = match section_key {
            "identity" => CharacterSection::Identity,
            "personality" => CharacterSection::Personality,
            "background" => CharacterSection::Background,
            "interests" => CharacterSection::Interests,
            "appearance" => CharacterSection::Appearance,
            "speech" => CharacterSection::Speech,
            "relationships" => CharacterSection::Relationships,
            "examples" => {
                return Err(crate::error::VivianError::Config(
                    "examples 请使用 set_few_shot_examples 设置结构化数据".to_string(),
                ));
            }
            "canon_quotes" => {
                return Err(crate::error::VivianError::Config(
                    "canon_quotes 是语气基准，不可编辑；如需调整语气请修改 speech".to_string(),
                ));
            }
            _ => return Err(crate::error::VivianError::Config(format!("未知角色段落: {}", section_key))),
        };
        let text = if body.trim().is_empty() {
            String::new()
        } else {
            let name = self.config.read().identity.name.clone();
            prompt_render::prepend_heading(&name, section, body)
        };
        {
            let mut config = self.config.write();
            let field = match section {
                CharacterSection::Identity => &mut config.role_definition,
                CharacterSection::Personality => &mut config.personality_definition,
                CharacterSection::Background => &mut config.background_definition,
                CharacterSection::Interests => &mut config.interests_definition,
                CharacterSection::Appearance => &mut config.appearance_definition,
                CharacterSection::Speech => &mut config.speech_definition,
                CharacterSection::Relationships => &mut config.relationships_definition,
                CharacterSection::Examples => unreachable!(),
                CharacterSection::CanonQuotes => unreachable!(),
            };
            *field = text;
        }
        self.save_to()
    }

    /// 获取角色段落文本（用户覆盖或出厂默认），返回给前端编辑时已剥离标题行
    ///
    /// canon_quotes 不可编辑，调用此接口会返回错误
    pub fn get_section_definition(&self, section_key: &str) -> VivianResult<String> {
        use crate::persona::prompt_render::CharacterSection;
        let section = match section_key {
            "identity" => CharacterSection::Identity,
            "personality" => CharacterSection::Personality,
            "background" => CharacterSection::Background,
            "interests" => CharacterSection::Interests,
            "appearance" => CharacterSection::Appearance,
            "speech" => CharacterSection::Speech,
            "relationships" => CharacterSection::Relationships,
            "examples" => CharacterSection::Examples,
            "canon_quotes" => {
                return Err(crate::error::VivianError::Config(
                    "canon_quotes 是语气基准，不可编辑".to_string(),
                ));
            }
            _ => return Err(crate::error::VivianError::Config(format!("未知角色段落: {}", section_key))),
        };
        let config = self.config.read();
        let lang = self.language.read().clone();
        let full = crate::persona::prompt_render::resolve_section(&config, section, &lang);
        Ok(crate::persona::prompt_render::strip_heading(&full))
    }

    /// 重置角色段落到出厂默认
    pub fn reset_persona_section(&self, section_key: &str) -> VivianResult<()> {
        if section_key == "examples" {
            let mut config = self.config.write();
            config.few_shot_examples = schemas::FewShotExamplesConfig::default();
            drop(config);
            return self.save_to();
        }
        self.set_section_definition(section_key, "")
    }

    /// 检查角色段落是否被用户修改过
    pub fn is_section_customized(&self, section_key: &str) -> bool {
        let config = self.config.read();
        match section_key {
            "identity" => !config.role_definition.trim().is_empty(),
            "personality" => !config.personality_definition.trim().is_empty(),
            "background" => !config.background_definition.trim().is_empty(),
            "interests" => !config.interests_definition.trim().is_empty(),
            "appearance" => !config.appearance_definition.trim().is_empty(),
            "speech" => !config.speech_definition.trim().is_empty(),
            "relationships" => !config.relationships_definition.trim().is_empty(),
            "examples" => !config.few_shot_examples.examples.is_empty(),
            _ => false,
        }
    }

    /// 获取结构化 Few-shot 示例配置
    pub fn get_few_shot_examples(&self) -> schemas::FewShotExamplesConfig {
        let config = self.config.read();
        if !config.few_shot_examples.examples.is_empty() {
            config.few_shot_examples.clone()
        } else {
            let default_md = prompt_render::default_section_for(&config.identity.name, prompt_render::CharacterSection::Examples);
            prompt_render::parse_examples_markdown(default_md)
        }
    }

    /// 设置结构化 Few-shot 示例配置并持久化
    pub fn set_few_shot_examples(&self, data: schemas::FewShotExamplesConfig) -> VivianResult<()> {
        {
            let mut config = self.config.write();
            config.few_shot_examples = data;
        }
        self.save_to()
    }

    /// 兼容旧 API：设置身份定位层
    pub fn set_role_definition(&self, text: &str) -> VivianResult<()> {
        self.set_section_definition("identity", text)
    }

    /// 兼容旧 API：获取身份定位层
    pub fn get_role_definition(&self) -> String {
        self.get_section_definition("identity").unwrap_or_default()
    }

    pub fn get_name(&self) -> String {
        self.config.read().identity.name.clone()
    }

    pub fn get_tagline(&self) -> String {
        self.config.read().identity.tagline.clone()
    }

    // ===== 核心接口 — 供 Brain / PromptBuilder 调用 =====

    /// 选择场景模式（SceneModeSelector 委托）
    ///
    /// 每次对话轮次调用一次。
    /// - `user_input`：用户输入
    /// - `context`：上下文（含 time, intimacy, energy 等）
    /// - `emotion`：情感标签（可选）
    ///
    /// 返回选中的 SceneMode
    pub fn select_scene(
        &self,
        user_input: &str,
        context: Option<&SceneContext>,
        emotion: Option<&str>,
    ) -> SceneMode {
        let mode = self.scene_selector.lock().select(user_input, context, emotion);
        *self.current_scene_mode.write() = mode;
        mode
    }

    /// 获取带卡片覆盖的 PersonaConfig
    ///
    /// 若有激活的人格卡片，将卡片的 expression / language_style / style_preset 覆盖
    /// 到一份克隆的 PersonaConfig 上。Core Persona（identity）永远不被覆盖。
    fn config_with_card_overlay(&self) -> PersonaConfig {
        let mut cfg = self.config.read().clone();
        if let Some(card) = self.card_store.get_active_card() {
            if let Some(expr) = card.expression_override {
                cfg.expression = expr;
            }
            if let Some(ls) = card.language_style_override {
                cfg.language_style = ls;
            }
            if let Some(preset) = card.style_preset {
                if prompt_render::StylePreset::from_str(&preset).is_some() {
                    cfg.style_preset = preset;
                }
            }
        }
        cfg
    }

    /// 获取当前激活卡片的额外指令（注入 prompt 用）
    pub fn get_card_extra_instructions(&self) -> Vec<String> {
        self.card_store
            .get_active_card()
            .map(|c| c.extra_instructions)
            .unwrap_or_default()
    }

    /// 渲染 Character 块（身份+人格+背景+兴趣+外观+说话风格+关系）
    ///
    /// 作为 prompt 静态段第一模块，让 LLM 明确"我是谁"。
    /// 始终使用 Core Persona，不受卡片覆盖影响。
    /// 末尾追加自我进化覆盖层（如有），让 LLM 感知"我最近对自己做的调整"。
    pub fn get_character_block(&self) -> String {
        let lang = self.language.read().clone();
        let mut block = prompt_render::render_character_block(&self.config.read(), &lang);
        if let Some(evolution_text) = self.evolution.render(&lang) {
            if !block.trim().is_empty() {
                block.push_str("\n\n");
            }
            block.push_str(&evolution_text);
        }
        block
    }

    // ===== 自我进化覆盖层（智能体反思中自行调整，独立于原始人设） =====

    /// 应用一条自我进化调整（语气 or 性格）。
    ///
    /// 由反思流程调用。受最小间隔限制，返回是否成功记录。
    /// 只影响最终拼入 prompt 的覆盖层，不修改原始人设文件。
    pub fn apply_evolution(&self, kind: &str, text: &str, reason: &str) -> bool {
        let added = self.evolution.add_entry(kind, text, reason);
        if added {
            tracing::info!(
                "[PersonaEngine] 自我进化已记录: kind={}, text=\"{}\"",
                kind,
                text
            );
        }
        added
    }

    /// 恢复出厂：清空自我进化覆盖层（原始人设文件不受影响）
    pub fn reset_evolution(&self) {
        self.evolution.reset();
        tracing::info!("[PersonaEngine] 自我进化覆盖层已清空（恢复出厂）");
    }

    /// 自我进化覆盖层是否为空
    pub fn is_evolution_empty(&self) -> bool {
        self.evolution.is_empty()
    }

    /// 最近一次自我进化调整时间
    pub fn evolution_last_update(&self) -> f64 {
        self.evolution.last_update()
    }

    /// 自我进化记录列表
    pub fn evolution_entries(&self) -> Vec<EvolutionEntry> {
        self.evolution.entries()
    }

    /// 待晋升的自我进化候选（未达跨轨迹支持门槛，尚不生效）
    pub fn evolution_candidates(&self) -> Vec<EvolutionCandidate> {
        self.evolution.candidates()
    }

    /// 渲染 Few-shot examples 块（角色专属示例）
    pub fn get_examples_block(&self) -> String {
        let lang = self.language.read().clone();
        prompt_render::render_examples_block(&self.config.read(), &lang)
    }

    /// 兼容旧 API：渲染身份声明块
    pub fn get_identity_block(&self) -> String {
        self.get_character_block()
    }

    /// 获取当前场景的风格约束文本块（注入 prompt 用）
    ///
    /// `scene_mode` 为 None 时使用当前模式。
    /// 若有激活的人格卡片，风格约束会使用卡片覆盖后的表达参数。
    pub fn get_style_block(&self, scene_mode: Option<SceneMode>) -> String {
        let mode = scene_mode.unwrap_or_else(|| *self.current_scene_mode.read());
        let cfg = self.config_with_card_overlay();
        let lang = self.language.read().clone();
        prompt_render::render_style_block(&cfg, mode, &lang)
    }

    /// 获取简短版风格约束（用于工具调用等精简场景）
    pub fn get_short_style_block(&self, scene_mode: Option<SceneMode>) -> String {
        let mode = scene_mode.unwrap_or_else(|| *self.current_scene_mode.read());
        let cfg = self.config_with_card_overlay();
        let lang = self.language.read().clone();
        prompt_render::render_short_style_block(&cfg, mode, &lang)
    }

    /// 获取当前场景模式
    pub fn get_current_mode(&self) -> SceneMode {
        *self.current_scene_mode.read()
    }

    /// 获取模式切换历史
    pub fn get_mode_history(&self) -> Vec<SceneMode> {
        self.scene_selector.lock().get_mode_history()
    }

    /// 强制设置场景模式（如作息触发）
    pub fn force_scene_mode(&self, mode: SceneMode) {
        self.scene_selector.lock().force_mode(mode);
        *self.current_scene_mode.write() = mode;
    }

    /// 重新加载配置（配置变更时调用）
    ///
    /// 传入 None 时仅同步子模块与当前 config，不替换 config
    pub fn reload_config(&self, persona_config: Option<PersonaConfig>) {
        if let Some(cfg) = persona_config {
            self.reload_config_internal(cfg);
        } else {
            let cfg = self.config.read().clone();
            self.scene_selector.lock().reload_modes(cfg.scene_modes.clone());
        }
    }

    fn reload_config_internal(&self, config: PersonaConfig) {
        self.scene_selector.lock().reload_modes(config.scene_modes.clone());
        *self.config.write() = config;
    }

    /// 根据关系阶段动态调整人设表现
    ///
    /// 关系越亲密，Vivian 的回复越热情、亲昵、信任。
    /// 关系较疏远时，Vivian 更矜持、更礼貌。
    ///
    /// 非破坏性：在基准表达值（`base_expression`）上做线性混合，
    /// `final = base * (1 - blend) + target * blend`，
    /// blend 因子随亲密度从 0.3 到 0.7 递增，确保原始人设始终可辨。
    ///
    /// - `intimacy`：亲密度值（0-100）
    /// - `relationship_stage`：关系阶段名称
    pub fn adjust_for_relationship(&self, intimacy: f64, relationship_stage: &str) {
        // 亲密度目标值（与旧逻辑一致）
        let target = if intimacy < 20.0 {
            CharacterExpression { tsundere: 0.5, clingy: 0.1, genki: 0.3, sass: 0.2, healing: 0.3, curiosity: 0.5, ritual: 0.4, habit_awareness: 0.4 }
        } else if intimacy < 40.0 {
            CharacterExpression { tsundere: 0.6, clingy: 0.2, genki: 0.4, sass: 0.3, healing: 0.4, curiosity: 0.55, ritual: 0.45, habit_awareness: 0.5 }
        } else if intimacy < 60.0 {
            CharacterExpression { tsundere: 0.5, clingy: 0.4, genki: 0.5, sass: 0.4, healing: 0.6, curiosity: 0.65, ritual: 0.5, habit_awareness: 0.55 }
        } else if intimacy < 80.0 {
            CharacterExpression { tsundere: 0.3, clingy: 0.6, genki: 0.6, sass: 0.5, healing: 0.7, curiosity: 0.7, ritual: 0.55, habit_awareness: 0.6 }
        } else {
            CharacterExpression { tsundere: 0.2, clingy: 0.7, genki: 0.6, sass: 0.5, healing: 0.8, curiosity: 0.75, ritual: 0.6, habit_awareness: 0.65 }
        };

        // 混合因子：亲密度越低，base 权重越高（保留人设个性）；
        // 亲密度越高，target 权重越高（关系影响表达）
        let blend = (intimacy / 100.0).clamp(0.0, 1.0) * 0.4 + 0.3; // [0.3, 0.7]

        let base = self.base_expression.read().clone();
        let blend_field = |b: f64, t: f64| -> f64 { b * (1.0 - blend) + t * blend };

        let mut config = self.config.write();
        config.expression.tsundere = blend_field(base.tsundere, target.tsundere);
        config.expression.clingy = blend_field(base.clingy, target.clingy);
        config.expression.genki = blend_field(base.genki, target.genki);
        config.expression.sass = blend_field(base.sass, target.sass);
        config.expression.healing = blend_field(base.healing, target.healing);
        config.expression.curiosity = blend_field(base.curiosity, target.curiosity);
        config.expression.ritual = blend_field(base.ritual, target.ritual);
        config.expression.habit_awareness = blend_field(base.habit_awareness, target.habit_awareness);

        let cloned = config.clone();
        drop(config);

        self.scene_selector.lock().reload_modes(cloned.scene_modes);
        tracing::debug!(
            "[PersonaEngine] 人设已根据关系调整: intimacy={}, stage={}, blend={:.2}",
            intimacy,
            relationship_stage,
            blend
        );
    }

    /// 从心理学逆向映射同步 CharacterExpression（双向同步 #14）
    ///
    /// `from_expression()` 是单向的：CharacterExpression → PersonaProfile。
    /// 当心理学系统长期演化（LLM appraisal/情绪累积改变 PersonaTraits）后，
    /// 需要通过 `to_expression_hint()` 反向推导并微调到表达层。
    ///
    /// 策略：低权重混合（blend=0.1）到 `base_expression`，确保：
    /// 1. 原始人设不会被心理学漂移覆盖
    /// 2. 长期趋势（如用户交互使 clingy 逐渐升高）能缓慢反映到表达层
    /// 3. `adjust_for_relationship` 后续混合基于新的 base，自然传播
    pub fn sync_from_persona_hint(&self, hint: &crate::psychology::ExpressionHint) {
        const SYNC_BLEND: f64 = 0.1;
        let mut base = self.base_expression.write();
        let lerp = |current: f64, hint_val: f64| -> f64 {
            current * (1.0 - SYNC_BLEND) + hint_val * SYNC_BLEND
        };
        base.tsundere = lerp(base.tsundere, hint.tsundere);
        base.clingy = lerp(base.clingy, hint.clingy);
        base.genki = lerp(base.genki, hint.genki);
        base.sass = lerp(base.sass, hint.sass);
        base.healing = lerp(base.healing, hint.healing);
        base.curiosity = lerp(base.curiosity, hint.curiosity);
        // ritual 和 habit_awareness 无心理学对应维度，保持原值

        tracing::debug!(
            "[PersonaEngine] base_expression 已从心理学 hint 微调: tsundere={:.2} clingy={:.2} genki={:.2}",
            base.tsundere, base.clingy, base.genki
        );
    }

    // ===== 兼容接口 =====

    /// 根据亲密度与小时生成风格约束 prompt（兼容旧调用方）
    ///
    /// 内部基于 hour/intimacy 构造 SceneContext，调用 select_scene + get_style_block
    pub fn build_style_prompt(&self, intimacy: f64, hour: u32) -> String {
        let ctx = SceneContext {
            hour: Some(hour),
            intimacy: Some(intimacy),
            energy: None,
            active_app: None,
            dominant_emotion: None,
            need_pressure: None,
        };
        let mode = self.select_scene("", Some(&ctx), None);
        self.get_style_block(Some(mode))
    }

    /// 增强版风格 prompt：携带心理学情绪/需求状态，让场景选择感知内心
    ///
    /// `dominant_emotion`：心理学系统主导情绪标签（如 "sad"/"joyful"/"curious"）
    /// `need_pressure`：需求压力 0.0-1.0，高值偏向 Cozy/Guardian
    pub fn build_style_prompt_ex(
        &self,
        intimacy: f64,
        hour: u32,
        dominant_emotion: Option<String>,
        need_pressure: Option<f64>,
    ) -> String {
        let ctx = SceneContext {
            hour: Some(hour),
            intimacy: Some(intimacy),
            energy: None,
            active_app: None,
            dominant_emotion,
            need_pressure,
        };
        let mode = self.select_scene("", Some(&ctx), None);
        self.get_style_block(Some(mode))
    }

    // ===== 人格卡片演化（委托 PersonaCardStore） =====

    /// 获取卡片存储的引用（供命令层直接调用）
    pub fn card_store(&self) -> &PersonaCardStore {
        &self.card_store
    }

    /// 递增对话轮次（每轮对话结束时调用，驱动冷却机制）
    pub fn tick_card_turn(&self) {
        self.card_store.tick_turn();
    }

    /// 获取当前人设的决策权重（Cognitive Tick 决策层用）
    ///
    /// 从 8 维 CharacterExpression 派生 think/act/speak 三个倾向权重，
    /// 让规则层决策受人设影响（而非只靠 prompt 约束 LLM）。
    ///
    /// 若有激活的人格卡片，使用卡片覆盖后的表达参数计算。
    pub fn decision_weights(&self) -> PersonaDecisionWeights {
        let cfg = self.config_with_card_overlay();
        PersonaDecisionWeights::from_expression(&cfg.expression)
    }
}

impl Default for PersonaEngine {
    fn default() -> Self {
        Self::new("default").unwrap_or_else(|e| {
            tracing::error!("人格引擎初始化失败，使用内存模式: {e}");
            let default_config = DEFAULT_PERSONA.clone();
            PersonaEngine {
                config: Arc::new(RwLock::new(default_config.clone())),
                scene_selector: Mutex::new(SceneModeSelector::new()),
                current_scene_mode: RwLock::new(SceneMode::DailyChat),
                persistence_path: std::path::PathBuf::from("persona.json"),
                card_store: PersonaCardStore::fallback(),
                base_expression: RwLock::new(default_config.expression),
                language: RwLock::new("zh".to_string()),
                evolution: PersonaEvolutionStore::fallback(),
            }
        })
    }
}
