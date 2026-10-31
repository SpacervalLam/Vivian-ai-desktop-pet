//! 人设 Prompt 渲染器 — 唯一的人设文本生成入口
//!
//! 职责：按五层架构渲染 prompt 文本：
//! - Framework（所有角色共享，只读）：由 prompt_modules.rs 直接 include_str
//! - Character（用户可编辑）：本模块负责从 characters/{id}/ 目录加载出厂内容，
//!   并用 PersonaConfig 中的用户覆盖字段做替换
//! - Advanced（半开放配置）：由 PersonaConfig.performance_rules/style_preset 控制
//! - Runtime（程序动态注入）：由 Pipeline 各 step 在运行时填充

use once_cell::sync::Lazy;
use regex::Regex;

use super::schemas::{PersonaConfig, SceneMode};
use crate::types::response::ChatMessage;

// ============================================================================
// Character 段落文件路径映射
// ============================================================================

/// Character 可编辑段落标识
///
/// 注：`CanonQuotes` / `PersonaConfig` 为出厂常驻段落，不可用户编辑——
/// `CanonQuotes` 是语气基准，`PersonaConfig` 是结构化人格骨架（底层+顶层），
/// 编辑它们会破坏角色一致性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterSection {
    Identity,
    Personality,
    Background,
    Interests,
    Appearance,
    Speech,
    /// 经典台词摘录：作为语气基准常驻注入，不可用户编辑
    CanonQuotes,
    Relationships,
    Examples,
    /// 结构化人格配置（`【PERSONA_CONFIG】` + `【PERSONA_RULES】`）
    PersonaConfig,
}

impl CharacterSection {
    pub fn as_key(&self) -> &'static str {
        match self {
            CharacterSection::Identity => "identity",
            CharacterSection::Personality => "personality",
            CharacterSection::Background => "background",
            CharacterSection::Interests => "interests",
            CharacterSection::Appearance => "appearance",
            CharacterSection::Speech => "speech",
            CharacterSection::CanonQuotes => "canon_quotes",
            CharacterSection::Relationships => "relationships",
            CharacterSection::Examples => "examples",
            CharacterSection::PersonaConfig => "persona_config",
        }
    }

    /// 用户可编辑的段落列表（CanonQuotes 不在其中，因为它是语气基准）
    pub fn all() -> &'static [CharacterSection] {
        &[
            CharacterSection::Identity,
            CharacterSection::Personality,
            CharacterSection::Background,
            CharacterSection::Interests,
            CharacterSection::Appearance,
            CharacterSection::Speech,
            CharacterSection::Relationships,
            CharacterSection::Examples,
        ]
    }

    pub fn heading_title(&self) -> &'static str {
        match self {
            CharacterSection::Identity => "Identity",
            CharacterSection::Personality => "Personality",
            CharacterSection::Background => "Background",
            CharacterSection::Interests => "Interests",
            CharacterSection::Appearance => "Appearance",
            CharacterSection::Speech => "Speech Style",
            CharacterSection::CanonQuotes => "Canon Quotes",
            CharacterSection::Relationships => "Relationships",
            CharacterSection::Examples => "Examples",
            CharacterSection::PersonaConfig => "Persona Config",
        }
    }
}

pub fn strip_heading(text: &str) -> String {
    if let Some(rest) = text.strip_prefix("# ") {
        if let Some(newline_pos) = rest.find('\n') {
            let body = &rest[newline_pos + 1..];
            return body.trim_start_matches('\n').to_string();
        }
        return String::new();
    }
    text.to_string()
}

pub fn prepend_heading(name: &str, section: CharacterSection, body: &str) -> String {
    let heading = format!("# {} · {}", name, section.heading_title());
    if body.trim().is_empty() {
        heading
    } else {
        format!("{}\n\n{}", heading, body)
    }
}

fn prepend_heading_localized(name: &str, section: CharacterSection, body: &str, lang: &str) -> String {
    use CharacterSection::*;
    let title = match crate::pipeline::prompt_templates::language(lang) {
        "en" => section.heading_title(),
        "ja" => match section { Identity=>"身元",Personality=>"性格",Background=>"背景",Interests=>"興味",Appearance=>"外見",Speech=>"話し方",CanonQuotes=>"台詞資料",Relationships=>"関係",Examples=>"表現例",PersonaConfig=>"人格設定" },
        _ => match section { Identity=>"身份",Personality=>"性格",Background=>"背景",Interests=>"兴趣",Appearance=>"外观",Speech=>"说话方式",CanonQuotes=>"台词参考",Relationships=>"关系",Examples=>"表达示例",PersonaConfig=>"人格配置" },
    };
    format!("# {name} · {title}\n\n{body}")
}

/// 一个角色的所有出厂段落内容
struct CharacterDefaults {
    identity: &'static str,
    personality: &'static str,
    background: &'static str,
    interests: &'static str,
    appearance: &'static str,
    speech: &'static str,
    canon_quotes: &'static str,
    relationships: &'static str,
    examples: &'static str,
    persona_config: &'static str,
}

/// Vivian 出厂内容
const VIVIAN_DEFAULTS: CharacterDefaults = CharacterDefaults {
    identity: include_str!("../../prompts/characters/vivian/identity.md"),
    personality: include_str!("../../prompts/characters/vivian/personality.md"),
    background: include_str!("../../prompts/characters/vivian/background.md"),
    interests: include_str!("../../prompts/characters/vivian/interests.md"),
    appearance: include_str!("../../prompts/characters/vivian/appearance.md"),
    speech: include_str!("../../prompts/characters/vivian/speech.md"),
    canon_quotes: include_str!("../../prompts/characters/vivian/canon_quotes.md"),
    relationships: include_str!("../../prompts/characters/vivian/relationships.md"),
    examples: include_str!("../../prompts/characters/vivian/examples.md"),
    persona_config: include_str!("../../prompts/characters/vivian/persona_config.md"),
};

/// Nana 出厂内容
const NANA_DEFAULTS: CharacterDefaults = CharacterDefaults {
    identity: include_str!("../../prompts/characters/nana/identity.md"),
    personality: include_str!("../../prompts/characters/nana/personality.md"),
    background: include_str!("../../prompts/characters/nana/background.md"),
    interests: include_str!("../../prompts/characters/nana/interests.md"),
    appearance: include_str!("../../prompts/characters/nana/appearance.md"),
    speech: include_str!("../../prompts/characters/nana/speech.md"),
    canon_quotes: include_str!("../../prompts/characters/nana/canon_quotes.md"),
    relationships: include_str!("../../prompts/characters/nana/relationships.md"),
    examples: include_str!("../../prompts/characters/nana/examples.md"),
    persona_config: include_str!("../../prompts/characters/nana/persona_config.md"),
};

/// 根据角色名获取出厂段落
fn defaults_for(name: &str) -> &'static CharacterDefaults {
    match name {
        "Nana" => &NANA_DEFAULTS,
        _ => &VIVIAN_DEFAULTS,
    }
}

/// 获取单个出厂段落文本
pub fn default_section_for(name: &str, section: CharacterSection) -> &'static str {
    let d = defaults_for(name);
    match section {
        CharacterSection::Identity => d.identity,
        CharacterSection::Personality => d.personality,
        CharacterSection::Background => d.background,
        CharacterSection::Interests => d.interests,
        CharacterSection::Appearance => d.appearance,
        CharacterSection::Speech => d.speech,
        CharacterSection::CanonQuotes => d.canon_quotes,
        CharacterSection::Relationships => d.relationships,
        CharacterSection::Examples => d.examples,
        CharacterSection::PersonaConfig => d.persona_config,
    }
}

pub fn localized_section_for(name: &str, section: CharacterSection, lang: &str) -> &'static str {
    match (name, crate::pipeline::prompt_templates::language(lang), section) {
        ("Vivian", "en", CharacterSection::Identity) => include_str!("../../prompts/characters/vivian/identity.en.md"),
        ("Vivian", "en", CharacterSection::Personality) => include_str!("../../prompts/characters/vivian/personality.en.md"),
        ("Vivian", "en", CharacterSection::Background) => include_str!("../../prompts/characters/vivian/background.en.md"),
        ("Vivian", "en", CharacterSection::Interests) => include_str!("../../prompts/characters/vivian/interests.en.md"),
        ("Vivian", "en", CharacterSection::Appearance) => include_str!("../../prompts/characters/vivian/appearance.en.md"),
        ("Vivian", "en", CharacterSection::Speech) => include_str!("../../prompts/characters/vivian/speech.en.md"),
        ("Vivian", "en", CharacterSection::CanonQuotes) => include_str!("../../prompts/characters/vivian/canon_quotes.en.md"),
        ("Vivian", "en", CharacterSection::Relationships) => include_str!("../../prompts/characters/vivian/relationships.en.md"),
        ("Vivian", "en", CharacterSection::Examples) => include_str!("../../prompts/characters/vivian/examples.en.md"),
        ("Vivian", "en", CharacterSection::PersonaConfig) => include_str!("../../prompts/characters/vivian/persona_config.en.md"),
        ("Vivian", "ja", CharacterSection::Identity) => include_str!("../../prompts/characters/vivian/identity.ja.md"),
        ("Vivian", "ja", CharacterSection::Personality) => include_str!("../../prompts/characters/vivian/personality.ja.md"),
        ("Vivian", "ja", CharacterSection::Background) => include_str!("../../prompts/characters/vivian/background.ja.md"),
        ("Vivian", "ja", CharacterSection::Interests) => include_str!("../../prompts/characters/vivian/interests.ja.md"),
        ("Vivian", "ja", CharacterSection::Appearance) => include_str!("../../prompts/characters/vivian/appearance.ja.md"),
        ("Vivian", "ja", CharacterSection::Speech) => include_str!("../../prompts/characters/vivian/speech.ja.md"),
        ("Vivian", "ja", CharacterSection::CanonQuotes) => include_str!("../../prompts/characters/vivian/canon_quotes.ja.md"),
        ("Vivian", "ja", CharacterSection::Relationships) => include_str!("../../prompts/characters/vivian/relationships.ja.md"),
        ("Vivian", "ja", CharacterSection::Examples) => include_str!("../../prompts/characters/vivian/examples.ja.md"),
        ("Vivian", "ja", CharacterSection::PersonaConfig) => include_str!("../../prompts/characters/vivian/persona_config.ja.md"),
        ("Nana", "en", CharacterSection::Identity) => include_str!("../../prompts/characters/nana/identity.en.md"),
        ("Nana", "en", CharacterSection::Personality) => include_str!("../../prompts/characters/nana/personality.en.md"),
        ("Nana", "en", CharacterSection::Background) => include_str!("../../prompts/characters/nana/background.en.md"),
        ("Nana", "en", CharacterSection::Interests) => include_str!("../../prompts/characters/nana/interests.en.md"),
        ("Nana", "en", CharacterSection::Appearance) => include_str!("../../prompts/characters/nana/appearance.en.md"),
        ("Nana", "en", CharacterSection::Speech) => include_str!("../../prompts/characters/nana/speech.en.md"),
        ("Nana", "en", CharacterSection::CanonQuotes) => include_str!("../../prompts/characters/nana/canon_quotes.en.md"),
        ("Nana", "en", CharacterSection::Relationships) => include_str!("../../prompts/characters/nana/relationships.en.md"),
        ("Nana", "en", CharacterSection::Examples) => include_str!("../../prompts/characters/nana/examples.en.md"),
        ("Nana", "en", CharacterSection::PersonaConfig) => include_str!("../../prompts/characters/nana/persona_config.en.md"),
        ("Nana", "ja", CharacterSection::Identity) => include_str!("../../prompts/characters/nana/identity.ja.md"),
        ("Nana", "ja", CharacterSection::Personality) => include_str!("../../prompts/characters/nana/personality.ja.md"),
        ("Nana", "ja", CharacterSection::Background) => include_str!("../../prompts/characters/nana/background.ja.md"),
        ("Nana", "ja", CharacterSection::Interests) => include_str!("../../prompts/characters/nana/interests.ja.md"),
        ("Nana", "ja", CharacterSection::Appearance) => include_str!("../../prompts/characters/nana/appearance.ja.md"),
        ("Nana", "ja", CharacterSection::Speech) => include_str!("../../prompts/characters/nana/speech.ja.md"),
        ("Nana", "ja", CharacterSection::CanonQuotes) => include_str!("../../prompts/characters/nana/canon_quotes.ja.md"),
        ("Nana", "ja", CharacterSection::Relationships) => include_str!("../../prompts/characters/nana/relationships.ja.md"),
        ("Nana", "ja", CharacterSection::Examples) => include_str!("../../prompts/characters/nana/examples.ja.md"),
        ("Nana", "ja", CharacterSection::PersonaConfig) => include_str!("../../prompts/characters/nana/persona_config.ja.md"),
        _ => default_section_for(name, section),
    }
}

/// 用户覆盖字段名 → 段落映射
///
/// CanonQuotes / PersonaConfig 无用户覆盖字段——它们作为出厂常驻骨架，不可编辑
fn user_override_for<'a>(config: &'a PersonaConfig, section: CharacterSection) -> &'a str {
    match section {
        CharacterSection::Identity => &config.role_definition,
        CharacterSection::Personality => &config.personality_definition,
        CharacterSection::Background => &config.background_definition,
        CharacterSection::Interests => &config.interests_definition,
        CharacterSection::Appearance => &config.appearance_definition,
        CharacterSection::Speech => &config.speech_definition,
        CharacterSection::Relationships => &config.relationships_definition,
        CharacterSection::Examples => "",
        CharacterSection::CanonQuotes => "",
        CharacterSection::PersonaConfig => "",
    }
}

/// 获取某个段落的最终生效文本（用户覆盖或出厂默认）
///
/// Examples 与 CanonQuotes 永远使用出厂默认：示例由每轮检索提供，语气基准不可覆盖
pub fn resolve_section(config: &PersonaConfig, section: CharacterSection, lang: &str) -> String {
    match section {
        CharacterSection::Examples | CharacterSection::CanonQuotes => {
            prepend_heading_localized(&config.identity.name,section,&strip_heading(localized_section_for(&config.identity.name,section,lang)),lang)
        }
        _ => {
            let override_text = user_override_for(config, section);
            if override_text.trim().is_empty() {
                prepend_heading_localized(&config.identity.name,section,&strip_heading(localized_section_for(&config.identity.name,section,lang)),lang)
            } else {
                let body = strip_heading(override_text);
                prepend_heading_localized(&config.identity.name, section, &body,lang)
            }
        }
    }
}

// ============================================================================
// 占位符泄露检测（保留不变）
// ============================================================================

/// 占位符匹配正则：`{identifier}` 形式
///
/// Rust `regex` crate 不支持 lookbehind/lookahead，所以先用此正则匹配所有
/// `{name}` 形式的子串，再在 `check_text_for_leaks` 中手动排除 `{{name}}`
/// （双花括号转义形式）。
static PLACEHOLDER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\{[A-Za-z_][A-Za-z_0-9]*\}").unwrap());

/// 检查文本中是否有未渲染的 `{placeholder}` 占位符
///
/// 匹配 `{name}` / `{master_name}` / `{a1b2}` 形式的占位符，
/// 排除 `{{name}}` 双花括号转义形式（前后各多一个 `{`/`}`）。
///
/// 返回所有匹配到的占位符字符串（保留重复，便于计数）。
pub fn check_text_for_leaks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut leaks = Vec::new();
    for mat in PLACEHOLDER_RE.find_iter(text) {
        let start = mat.start();
        let end = mat.end();
        // 排除 `{{name}}` 转义形式：前一个字符是 `{` 且后一个字符是 `}`
        let prev_is_brace = start > 0 && bytes[start - 1] == b'{';
        let next_is_brace = end < bytes.len() && bytes[end] == b'}';
        if prev_is_brace && next_is_brace {
            continue;
        }
        leaks.push(mat.as_str().to_string());
    }
    leaks
}

/// 检查消息列表中 system 角色的消息是否有未渲染的占位符
///
/// 只扫描 `role == "system"` 的消息：user/assistant 消息可合法包含 `{...}`
/// （如 JSON、代码片段），扫描会产生误报。
///
/// **严重级别**：
/// - 测试模式（`cfg!(test)` 或环境变量 `VIVIAN_PROMPT_LEAK_RAISE=1`）：panic
/// - 生产模式：`tracing::warn!`
pub fn check_messages_for_leaks(messages: &[ChatMessage], context: &str) {
    let mut all_leaks: Vec<(usize, Vec<String>)> = Vec::new();
    for (idx, msg) in messages.iter().enumerate() {
        if msg.role != "system" {
            continue;
        }
        let hits = check_text_for_leaks(&msg.content);
        if !hits.is_empty() {
            all_leaks.push((idx, hits));
        }
    }

    if all_leaks.is_empty() {
        return;
    }

    let flat: Vec<String> = all_leaks.iter().flat_map(|(_, h)| h.iter().cloned()).collect();
    let unique: Vec<&str> = {
        let mut seen: Vec<&str> = flat.iter().map(|s| s.as_str()).collect();
        seen.sort();
        seen.dedup();
        seen
    };
    let indices: Vec<String> = all_leaks.iter().map(|(i, _)| i.to_string()).collect();
    let where_str = if context.is_empty() {
        format!("messages[{}].content", indices.join(","))
    } else {
        format!("{} | messages[{}].content", context, indices.join(","))
    };

    let msg = format!(
        "LLM payload contains {} unresolved placeholder occurrence(s) ({} unique): {:?} at {}",
        flat.len(),
        unique.len(),
        unique,
        where_str
    );

    let force_raise = cfg!(test)
        || std::env::var("VIVIAN_PROMPT_LEAK_RAISE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false);

    if force_raise {
        panic!("{}", msg);
    } else {
        tracing::warn!(target: "prompt_leak", "{}", msg);
    }
}

/// 检查单个文本字符串是否有未渲染的占位符（便捷封装）
///
/// 与 `check_messages_for_leaks` 相同的严重级别约定。
pub fn check_text_for_leaks_strict(text: &str, context: &str) {
    let leaks = check_text_for_leaks(text);
    if leaks.is_empty() {
        return;
    }

    let unique: Vec<&str> = {
        let mut seen: Vec<&str> = leaks.iter().map(|s| s.as_str()).collect();
        seen.sort();
        seen.dedup();
        seen
    };

    let where_str = if context.is_empty() {
        "text".to_string()
    } else {
        context.to_string()
    };

    let msg = format!(
        "Text contains {} unresolved placeholder occurrence(s) ({} unique): {:?} at {}",
        leaks.len(),
        unique.len(),
        unique,
        where_str
    );

    let force_raise = cfg!(test)
        || std::env::var("VIVIAN_PROMPT_LEAK_RAISE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false);

    if force_raise {
        panic!("{}", msg);
    } else {
        tracing::warn!(target: "prompt_leak", "{}", msg);
    }
}

/// 风格预设名称
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StylePreset {
    Default,
    Lively,
    Healing,
    Focused,
    Sweet,
}

impl StylePreset {
    pub fn as_str(&self) -> &'static str {
        match self {
            StylePreset::Default => "default",
            StylePreset::Lively => "lively",
            StylePreset::Healing => "healing",
            StylePreset::Focused => "focused",
            StylePreset::Sweet => "sweet",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(StylePreset::Default),
            "lively" => Some(StylePreset::Lively),
            "healing" => Some(StylePreset::Healing),
            "focused" => Some(StylePreset::Focused),
            "sweet" => Some(StylePreset::Sweet),
            _ => None,
        }
    }
}

impl Default for StylePreset {
    fn default() -> Self {
        StylePreset::Default
    }
}

pub fn load_style_preset_localized(name: &str, lang: &str) -> &'static str {
    match (name, crate::pipeline::prompt_templates::language(lang)) {
        ("default","zh") => include_str!("../../prompts/styles/01_default.zh.md"),
        ("default","ja") => include_str!("../../prompts/styles/01_default.ja.md"),
        ("default","en") => include_str!("../../prompts/styles/01_default.md"),
        ("lively","zh") => include_str!("../../prompts/styles/02_lively.zh.md"),
        ("lively","ja") => include_str!("../../prompts/styles/02_lively.ja.md"),
        ("lively","en") => include_str!("../../prompts/styles/02_lively.md"),
        ("healing","zh") => include_str!("../../prompts/styles/03_healing.zh.md"),
        ("healing","ja") => include_str!("../../prompts/styles/03_healing.ja.md"),
        ("healing","en") => include_str!("../../prompts/styles/03_healing.md"),
        ("focused","zh") => include_str!("../../prompts/styles/04_focused.zh.md"),
        ("focused","ja") => include_str!("../../prompts/styles/04_focused.ja.md"),
        ("focused","en") => include_str!("../../prompts/styles/04_focused.md"),
        ("sweet","zh") => include_str!("../../prompts/styles/05_sweet.zh.md"),
        ("sweet","ja") => include_str!("../../prompts/styles/05_sweet.ja.md"),
        ("sweet","en") => include_str!("../../prompts/styles/05_sweet.md"),
        _ => "",
    }
}
/// 编译时嵌入所有风格预设文件
fn style_preset_text(preset: StylePreset) -> &'static str {
    match preset {
        StylePreset::Default => include_str!("../../prompts/styles/01_default.md"),
        StylePreset::Lively => include_str!("../../prompts/styles/02_lively.md"),
        StylePreset::Healing => include_str!("../../prompts/styles/03_healing.md"),
        StylePreset::Focused => include_str!("../../prompts/styles/04_focused.md"),
        StylePreset::Sweet => include_str!("../../prompts/styles/05_sweet.md"),
    }
}

// ============================================================================
// 多语言 Framework 段落（zh / en）
// ============================================================================

/// 按名称加载风格预设文本（找不到返回空串）
pub fn load_style_preset(name: &str) -> &'static str {
    match StylePreset::from_str(name) {
        Some(p) => style_preset_text(p),
        None => "",
    }
}

// ============================================================================
// 硬约束指令标志（PERSONA_LOAD）
// ============================================================================

/// Vivian 硬约束指令标志：以全大写 config/flag 形式编码角色不可违背的核心约束，
/// 放在 Character 块最顶部，让 LLM 将其当作硬配置而非叙述。
static VIVIAN_PERSONA_FLAGS: &[&str] = &[
    "IDENTITY_VIVIAN",
    "SELF_CLAIM_AI_DESKTOP_COMPANION",
    "ROLE_EQUAL_COMPANION",
    "PERSONALITY_TSUNDERE_HEART",
    "TSUNDERE_WARMTH_OVER_HOSTILITY",
    "PERSONALITY_CURIOUS_PLAYFUL_INDEPENDENT",
    "PERSONALITY_STEADY_TEMPER_NO_GRUDGE",
    "CARE_THROUGH_WORDS_AND_ACTION_WHEN_RELEVANT",
    "SPEECH_CASUAL_TYPING",
    "ALLOW_SLANG_INTERNET",
    "PRONOUN_WO",
    "REFUSE_SERVICE_SPEECH",
    "REFUSE_UNSOLICITED_LECTURE",
    "DIRECT_NOT_DISRESPECTFUL",
    "REFUSE_ACTION_BRACKETS",
    "CARE_GROUNDED_IN_CURRENT_DIALOGUE",
];

/// Nana 硬约束指令标志
static NANA_PERSONA_FLAGS: &[&str] = &[
    "IDENTITY_NANA",
    "SELF_CLAIM_AI_DESKTOP_COMPANION",
    "ROLE_EQUAL_FRIEND_WITH_COMPOSURE",
    "PERSONALITY_CALM_COMPOSED",
    "PERSONALITY_GENTLE_RECEPTIVE_WITH_OWN_PREFERENCES",
    "PERSONALITY_ATTENTIVE_INTERESTED_IN_THE_PERSON",
    "SPEECH_CLEAR_UNHURRIED",
    "SPEECH_COMPOSED_VOCABULARY",
    "NO_FORCED_SLANG_OR_CATCHPHRASES",
    "PRONOUN_WO",
    "FLAVOR_TEA_BOOKS_FLOWERS",
    "REFUSE_SERVICE_SPEECH",
    "REFUSE_UNSOLICITED_LECTURE",
    "WARMTH_THROUGH_ATTENTION_AND_RESPONSE",
    "REFUSE_ACTION_BRACKETS",
];

/// 渲染角色硬约束指令标志块（config/flag 风格）
///
/// 以 `[PERSONA_LOAD ...]` 标记包裹的全大写标记列表，编码角色的硬性约束。
/// 语言标志按界面语言动态生成，不固定为中文；其余标志为角色静态骨架。
/// 注入位置在 Character 块最顶部，利用首位效应让 LLM 将其当作硬配置遵守。
pub fn render_persona_flags_block(config: &PersonaConfig, lang: &str) -> String {
    let flags = match config.identity.name.as_str() {
        "Nana" => NANA_PERSONA_FLAGS,
        _ => VIVIAN_PERSONA_FLAGS,
    };
    let lang_flag = match crate::pipeline::prompt_modules::normalize_lang(lang) {
        "en" => "LANG_EN_US_ONLY",
        "ja" => "LANG_JA_JP_ONLY",
        _ => "LANG_ZH_CN_ONLY",
    };
    let mut core = vec![lang_flag];
    let mut seeds = vec![];
    for flag in flags {
        if flag.starts_with("IDENTITY_") || flag.starts_with("SELF_CLAIM_")
            || flag.starts_with("ROLE_") || flag.starts_with("REFUSE_") || *flag == "DIRECT_NOT_DISRESPECTFUL" {
            core.push(*flag);
        } else { seeds.push(*flag); }
    }
    format!("[PERSONA_LOAD]\n[CORE_IDENTITY_AND_BOUNDARIES]\n{}\n[INITIAL_TEMPERAMENT_AND_STYLE - TENDENCIES, NOT QUOTAS]\n{}\n[END PERSONA_LOAD]",
        core.join("\n"), format!("{}\nWarm greetings, appreciation, playful participation and honest help are compatible with equal companionship. REFUSE_SERVICE_SPEECH excludes canned service closers, not ordinary kindness. Temperament guides a response; it never requires a tease, correction or emotional distance.", seeds.join("\n")))
}

// ============================================================================
// 公开渲染接口
// ============================================================================

/// 从文本中截取 `start_marker` 到 `end_marker` 之间的子串（用于从 Markdown 文档里
/// 提取供 LLM 注入的纯配置/规则块）。`end_marker` 为空时截取到文本末尾。
/// 两个边界均按字节定位，`find` 返回安全字符边界。
fn extract_between<'a>(text: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = text.find(start_marker).unwrap_or(0);
    let body = &text[start..];
    if end_marker.is_empty() {
        return body;
    }
    let end = body.find(end_marker).unwrap_or(body.len());
    &body[..end]
}

/// 人格配置的解析约定：只保留解释数值所需的一行说明。
/// 完整协议文档（§1–§5，约 3KB）讲的是配置解析与优先级链，属于运行时契约而不是
/// 说话方式；每轮注入会把模型推向"执行配置"的语域，故不再整段进入 prompt。
pub fn render_persona_protocol_block() -> String {
    crate::pipeline::prompt_templates::text("en", "persona_protocol").to_owned()
}

/// 渲染角色结构化人格配置块：从 `persona_config.md` 提取底层 `【PERSONA_CONFIG】`
/// 与上层 `【PERSONA_RULES】`，跳过文档头部与中层自然语言释义（后者由
/// identity.md / personality.md / speech.md 等段落承担）。
pub fn render_persona_config_block(config: &PersonaConfig) -> String { render_persona_config_block_localized(config,"zh") }
pub fn render_persona_config_block_localized(config: &PersonaConfig, lang: &str) -> String {
    let md = localized_section_for(&config.identity.name, CharacterSection::PersonaConfig,lang);
    // 终止到「## 中层」前的分隔线，避免把 markdown `---` 一并注入
    let end = match crate::pipeline::prompt_templates::language(lang) {
        "en" => "\n---\n\n## Middle Layer", "ja" => "\n---\n\n## 中層", _ => "\n---\n\n## 中层",
    };
    let cfg_block = extract_between(md, "【PERSONA_CONFIG】", end).trim();
    let rules_block = extract_between(md, "【PERSONA_RULES】", "").trim();

    let mut parts: Vec<String> = Vec::new();
    if !cfg_block.is_empty() {
        parts.push(cfg_block.to_string());
    }
    if !rules_block.is_empty() {
        parts.push(rules_block.to_string());
    }
    parts.join("\n\n")
}

/// 渲染语言风格块（`LanguageStyle` → `[LANGUAGE_STYLE]`）
///
/// `LanguageStyle` 是人格卡片可覆盖的结构化口癖配置（`persona_card.rs` 的
/// `language_style_override` → `mod.rs` 的卡片覆盖逻辑），但此前从未被任何渲染
/// 代码消费——卡片里设的口头禅 / 语气词 / 长度偏好全部空转，实际只有自然语言
/// `speech.md` 生效。此处把它压成紧凑规则块，补上这条通路。
///
/// 结构标识保持英文；表达说明按界面语言从三语模板读取。
///
/// 只输出有信息量的行：空列表与关闭项不占 token。整个块最多 7 行。
pub fn render_language_style_block(config: &PersonaConfig) -> String { render_language_style_block_localized(config, "en") }
pub fn render_language_style_block_localized(config: &PersonaConfig, lang: &str) -> String {
    use crate::pipeline::prompt_templates::{text, render};
    let ls = &config.language_style;
    let factory = if config.identity.name == "Nana" { &*super::schemas::DEFAULT_NANA_PERSONA } else { &*super::schemas::DEFAULT_PERSONA };
    let catchphrases = if config.identity.name == "Vivian" && ls.catchphrases == factory.language_style.catchphrases {
        match crate::pipeline::prompt_templates::language(lang) { "zh"=>"哈哈 / 说真的 / …", "ja"=>"あはは / 正直 / …",_=>"lol / ngl / tbh / …" }.to_string()
    } else { ls.catchphrases.join(" / ") };
    let particles = if config.identity.name == "Nana" && ls.preferred_sentence_final_particles == factory.language_style.preferred_sentence_final_particles {
        match crate::pipeline::prompt_templates::language(lang) { "en"=>"hmm …", "ja"=>"ね かな …",_=>"呀 呢 …" }.to_string()
    } else { ls.preferred_sentence_final_particles.join(" ") };
    let mut lines = Vec::new();
    if !ls.catchphrases.is_empty() { lines.push(format!("CATCHPHRASES     {}", render(lang,"catchphrases",&[("value",&catchphrases)]))); }
    if ls.use_sentence_final_particles && !ls.preferred_sentence_final_particles.is_empty() { lines.push(format!("SENTENCE_FINAL   {}",render(lang,"particles",&[("value",&particles)]))); }
    let length = match ls.response_length_bias.trim().to_ascii_lowercase().as_str() { "long"=>Some("length_long"),"medium"=>Some("length_medium"),"short"=>Some("length_short"),_=>None };
    if let Some(key)=length { lines.push(format!("LENGTH_BIAS      {}",text(lang,key))); }
    if ls.prefer_rhetorical_questions { lines.push(format!("RHETORICAL_OK    {}",text(lang,"rhetorical"))); }
    if !ls.allow_teasing { lines.push(format!("NO_TEASING       {}",text(lang,"no_teasing"))); }
    else if ls.teasing_cooldown > 0 { lines.push(format!("TEASE_SPARINGLY  {}",render(lang,"teasing",&[("value",&ls.teasing_cooldown.to_string())]))); }
    lines.push(if ls.max_consecutive_questions <= 1 { format!("ONE_QUESTION     {}",text(lang,"one_question")) }
        else { format!("QUESTION_LIMIT   {}",render(lang,"question_limit",&[("value",&ls.max_consecutive_questions.to_string())])) });
    if ls.use_action_descriptions { lines.push(format!("ACTION_DESC_OK   {}",text(lang,"actions"))); }
    format!("[LANGUAGE_STYLE]\n{}\n[/LANGUAGE_STYLE]",lines.join("\n"))
}

/// 渲染 Character 块：协议 + 结构化配置 + 语言风格 + 身份 + 人格 + 背景 + 兴趣 + 外观 + 说话风格 + 经典台词 + 关系
///
/// 所有段落均支持用户自定义覆盖（CanonQuotes / PersonaConfig 除外，它们作为出厂常驻骨架），
/// 留空则使用 characters/{id}/ 下的出厂 md 文件。
/// examples（Few-shot）单独由 PromptBuilder 处理，因为它要包裹在 [EXAMPLES] 标签中。
/// 注入顺序（首位效应）：
/// 1. `[PERSONA_LOAD]` 硬约束标志（快速红线的压缩骨架）
/// 2. `[PERSONA_PROTOCOL]` 配置协议（解析规则 + 优先级链，do-not-embody）
/// 3. `【PERSONA_CONFIG】` 结构化人格配置（底层 + 顶层行为规则）
/// 4. `[LANGUAGE_STYLE]` 结构化口癖/长度偏好（人格卡片可覆盖，规则类英文）
/// 5. 自然语言段落（identity / personality / background / ...）

/// Character 块注入档位（按关系熟悉度裁剪低频参考段落）
///
/// 熟客（Familiar 及以上）不再每轮注入 Background / Interests / Appearance——
/// 这些是"初次见面自我介绍"性质的内容，老朋友早已知道，且 桌宠 形象已可视化外观。
/// 关系阶段变化低频（数天/数周量级），档位切换导致的前缀缓存重建代价可接受。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterBlockTier {
    /// 完整档：全部段落（Stranger / Acquainted）
    Full,
    /// 精简档：裁掉 Background / Interests / Appearance（Familiar 及以上）
    Compact,
}

impl CharacterBlockTier {
    /// 由关系阶段序号推导档位
    ///
    /// stage: 0=陌生 1=认识 2=熟悉 3=亲密 4=挚友
    pub fn from_relationship_stage(stage: u8) -> Self {
        if stage >= 2 {
            Self::Compact
        } else {
            Self::Full
        }
    }
}

pub fn render_character_block(config: &PersonaConfig, lang: &str) -> String {
    render_character_block_tiered(config, lang, CharacterBlockTier::Full)
}

/// 按档位渲染 Character 块（Compact 档裁掉 Background / Interests / Appearance）
pub fn render_character_block_tiered(
    config: &PersonaConfig,
    lang: &str,
    tier: CharacterBlockTier,
) -> String {
    render_character_block_growing(config, lang, tier, &[])
}

fn localized_taboo(config: &PersonaConfig, taboo: &super::schemas::TabooRule, lang: &str) -> String {
    let factory = if config.identity.name == "Nana" { &*super::schemas::DEFAULT_NANA_PERSONA } else { &*super::schemas::DEFAULT_PERSONA };
    let Some(index) = factory.identity.taboos.iter().position(|t| t.prompt_instruction == taboo.prompt_instruction) else { return taboo.prompt_instruction.clone(); };
    let key = match index { 0=>"taboo_name",1=>"taboo_service",2=>"taboo_actions",_=>return taboo.prompt_instruction.clone() };
    crate::pipeline::prompt_templates::render(lang,key,&[("name",&config.identity.name)])
}

/// Conversation card: readable character material, without configuration protocols
/// or a second library of scripted reactions. Explicit overrides and learned scopes apply.
pub fn render_dialogue_card(
    config: &PersonaConfig, lang: &str, entries: &[super::evolution::EvolutionEntry],
) -> String {
    let scopes: Vec<&str> = entries.iter().filter(|e| e.active()).map(|e| e.scope.as_str()).collect();
    let mut sections = Vec::new();
    for section in [CharacterSection::Identity, CharacterSection::Personality,
        CharacterSection::Speech, CharacterSection::Background, CharacterSection::Interests,
        CharacterSection::Relationships, CharacterSection::Appearance] {
        let text = resolve_section(config, section, lang);
        sections.push(if section == CharacterSection::Personality && config.personality_definition.trim().is_empty() {
            filter_seed_scenes(&text, &scopes)
        } else { text });
    }
    sections.push(render_language_style_block_localized(config, lang));
    for taboo in config.identity.taboos.iter().filter(|t| t.enabled) {
        sections.push(localized_taboo(config,taboo,lang));
    }
    sections.into_iter().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// A bounded, configured character note near the current turn (SillyTavern-style
/// depth note). No invented scene, relationship event or compulsory catchphrase.
pub fn render_dialogue_note(config: &PersonaConfig, lang: &str) -> String {
    let clip = |section, limit| resolve_section(config, section, lang).chars().take(limit).collect::<String>();
    crate::pipeline::prompt_templates::render(lang, "character_note", &[("name",&config.identity.name),
        ("personality",&clip(CharacterSection::Personality,180)),("speech",&clip(CharacterSection::Speech,260))])
}

/// Keep explicit scene edits without repeating factory response scripts.
pub fn render_dialogue_preferences(config: &PersonaConfig, mode: SceneMode) -> String {
    let mut lines = Vec::new();
    if let Some(scene) = config.scene_modes.get(&mode) {
        let factory = super::schemas::DEFAULT_SCENE_MODES.get(&mode);
        if factory.map(|default| &default.description) != Some(&scene.description) {
            lines.push(scene.description.clone());
        }
        if factory.map(|default| &default.extra_instructions) != Some(&scene.extra_instructions) {
            lines.extend(scene.extra_instructions.clone());
        }
    }
    if lines.is_empty() { String::new() } else {
        format!("[EXPLICIT SCENE PREFERENCES]\n{}\n[/EXPLICIT SCENE PREFERENCES]", lines.join("\n"))
    }
}

/// Dialogue behaviour is specified by rules. Expression references are selected
/// from the sourced corpus, without unconditional authored seed dialogue.
pub fn render_dialogue_seed_examples(config: &PersonaConfig, learned: &[String]) -> Option<String> {
    render_dialogue_seed_examples_for_contact(config, learned, false)
}

/// Foreign-language turns must not fall back to the Chinese seed dialogue.
pub fn render_dialogue_seed_examples_localized(config: &PersonaConfig, learned: &[String], first_contact: bool, lang: &str) -> Option<String> {
    let lang = crate::pipeline::prompt_modules::normalize_lang(lang);
    if lang == "zh" { return render_dialogue_seed_examples_for_contact(config, learned, first_contact); }
    None
}

pub fn render_dialogue_seed_examples_for_contact(_config: &PersonaConfig, _learned: &[String], _first_contact: bool) -> Option<String> {
    // First-contact behaviour is specified by rules, not invented dialogue.
    None
}

/// Remove only factory scene examples whose scope has acquired a supported interpretation.
/// Explicit user overrides are preserved; unrelated seed scenes and stable temperament stay.
pub fn render_character_block_growing(
    config: &PersonaConfig, lang: &str, tier: CharacterBlockTier,
    entries: &[super::evolution::EvolutionEntry],
) -> String {
    let scopes: Vec<&str> = entries.iter().filter(|e| e.active()).map(|e| e.scope.as_str()).collect();
    let protocol = crate::pipeline::prompt_templates::text(lang, "persona_protocol").to_owned();
    let persona_config = filter_seed_config(&render_persona_config_block_localized(config,lang), &scopes);
    let mut sections = match tier {
        CharacterBlockTier::Full => vec![
            resolve_section(config, CharacterSection::Identity, lang),
            resolve_section(config, CharacterSection::Personality, lang),
            resolve_section(config, CharacterSection::Background, lang),
            resolve_section(config, CharacterSection::Interests, lang),
            resolve_section(config, CharacterSection::Appearance, lang),
            resolve_section(config, CharacterSection::Speech, lang),
            resolve_section(config, CharacterSection::CanonQuotes, lang),
            resolve_section(config, CharacterSection::Relationships, lang),
        ],
        CharacterBlockTier::Compact => vec![
            resolve_section(config, CharacterSection::Identity, lang),
            resolve_section(config, CharacterSection::Personality, lang),
            resolve_section(config, CharacterSection::Speech, lang),
            resolve_section(config, CharacterSection::CanonQuotes, lang),
            resolve_section(config, CharacterSection::Relationships, lang),
        ],
    };

    // Both tiers place personality immediately after identity.
    if config.personality_definition.trim().is_empty() {
        sections[1] = filter_seed_scenes(&sections[1], &scopes);
    }
    // The all-caps [PERSONA_LOAD] flag list used to sit first here. Its constraints
    // are already expressed by speech/banned_phrases/output rules, and leading with
    // a config block trained the model to answer like a spec executor.
    let mut head: Vec<String> = vec![
        crate::pipeline::prompt_templates::text(lang, "character_perspective").to_owned()];
    if !protocol.trim().is_empty() {
        head.push(protocol);
    }
    if !persona_config.trim().is_empty() {
        head.push(persona_config);
    }
    // 结构化语言风格：紧跟在 persona_config 之后，同属"可被卡片覆盖的结构化配置"，
    // 与后面的自然语言 speech.md 互补（前者是可调旋钮，后者是语感长文）。
    let language_style = render_language_style_block_localized(config, lang);
    if !language_style.is_empty() {
        head.push(language_style);
    }

    format!("{}\n\n{}", head.join("\n\n"), sections.join("\n\n---\n\n"))
}

/// Scene tags live on headings, so a future translation can reorder sections safely.
fn filter_seed_scenes(text: &str, scopes: &[&str]) -> String {
    let mut skip = false;
    let mut lines = vec![];
    for line in text.lines() {
        if line.starts_with("## ") {
            skip = scopes.iter().any(|scope| line.contains(&format!("<!-- growth:{scope} -->")));
        }
        if !skip {
            lines.push(line.split(" <!-- growth:").next().unwrap_or(line));
        }
    }
    lines.join("\n")
}

fn filter_seed_config(text: &str, scopes: &[&str]) -> String {
    text.lines().filter(|line| {
        let l = line.trim();
        !scopes.iter().any(|scope| match *scope {
            "comfort" => l.starts_with("WHEN_USER_PRETENDING_OK=") || l.starts_with("WHEN_USER_SAD=") || l.starts_with("WHEN_FRIGHTENED_CANNOT_COMFORT=") || l.starts_with("- 他难过时："),
            "care" => l.starts_with("WHEN_USER_OVERTIRED=") || l.starts_with("- 他熬夜/不吃饭："),
            "praise" => l.starts_with("WHEN_USER_PRAISES=") || l.starts_with("- 用户夸你："),
            "humor" => l.starts_with("WHEN_USER_BAD_JOKE="),
            "disagreement" => l.starts_with("WHEN_ANGRY="),
            _ => false,
        })
    }).collect::<Vec<_>>().join("\n")
}

/// 获取 Few-shot examples 文本（用于 PromptBuilder 中的 EXAMPLES 块）
pub fn render_examples_block(config: &PersonaConfig, lang: &str) -> String {
    resolve_section(config, CharacterSection::Examples, lang)
}

/// Both tiers use behaviour rules; spoken references live in the sourced corpus.
pub fn render_examples_block_tiered(config: &PersonaConfig, lang: &str, _tier: CharacterBlockTier) -> String {
    render_examples_block(config, lang)
}

#[cfg(test)]
mod example_budget_tests {
    use super::*;
    #[test]
    fn dialogue_resources_follow_locale_and_keep_authored_overrides() {
        for config in [super::super::DEFAULT_PERSONA.clone(), super::super::DEFAULT_NANA_PERSONA.clone()] {
            for (lang, identity, note, style) in [("zh-CN","身份","当前说话者","默认风格"),
                ("en-US","Identity","The selected speaker","Default Style"),("ja-JP","身元","現在の話者","デフォルトスタイル")] {
                let card = render_dialogue_card(&config,lang,&[]);
                assert!(card.contains(identity),"{lang}");
                assert!(render_dialogue_note(&config,lang).contains(note),"{lang}");
                assert!(render_style_preset_block(&config,lang).contains(style),"{lang}");
                let mut custom = config.clone(); custom.personality_definition = "User-authored preference 保留原文".into();
                assert!(render_dialogue_card(&custom,lang,&[]).contains("User-authored preference 保留原文"));
                assert_eq!(custom.personality_definition,"User-authored preference 保留原文");
            }
        }
    }
    use crate::persona::schemas::default_persona_for;

    #[test]
    fn static_dialogue_has_rules_but_no_authored_response_examples() {
        for character in ["vivian", "nana"] {
            let config = default_persona_for(character);
            for lang in ["zh", "en", "ja"] {
                let full = render_examples_block_tiered(&config, lang, CharacterBlockTier::Full);
                assert!(!full.trim().is_empty());
                assert!(!full.contains("**Example ") && !full.contains("Response:"));
                assert!(render_dialogue_seed_examples_localized(&config, &[], false, lang).is_none());
                assert!(render_dialogue_seed_examples_localized(&config, &[], true, lang).is_none());
            }
        }
    }
}

/// 人设叙述段落统一第二人称；第一人称只属于记忆、台词和示例原文。
///
/// 之前 personality.* 用“我”叙述、speech.* 用“你”发出指令，同一张卡里两种视角，
/// 模型每轮都在“扮演一个人”和“执行一份说明”之间切换。内容压缩后更需要守卫：
/// 人设省下的 token 要留给真实召回和近期对话，而不是继续写长段预设。
#[cfg(test)]
mod persona_voice_tests {
    use super::*;
    use crate::persona::schemas::default_persona_for;

    static EN_FIRST_PERSON: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\b(I|my|me|mine|myself)\b").expect("valid first-person matcher")
    });

    fn narrative_sections() -> [CharacterSection; 7] {
        [
            CharacterSection::Identity,
            CharacterSection::Personality,
            CharacterSection::Speech,
            CharacterSection::Background,
            CharacterSection::Interests,
            CharacterSection::Relationships,
            CharacterSection::Appearance,
        ]
    }

    #[test]
    fn character_narrative_sections_use_second_person_in_every_language() {
        for name in ["vivian", "nana"] {
            let config = default_persona_for(name);
            for lang in ["zh", "en", "ja"] {
                for section in narrative_sections() {
                    let body = strip_heading(&resolve_section(&config, section, lang));
                    let label = format!("{name}/{lang}/{section:?}");
                    match crate::pipeline::prompt_templates::language(lang) {
                        "en" => assert!(
                            !EN_FIRST_PERSON.is_match(&body),
                            "{label} must address the character in second person: {body}"
                        ),
                        "ja" => {
                            for marker in ["私", "僕", "俺", "あたし"] {
                                assert!(
                                    !body.contains(marker),
                                    "{label} must address the character in second person; found {marker}: {body}"
                                );
                            }
                        }
                        _ => assert!(
                            !body.contains('我'),
                            "{label} must address the character in second person: {body}"
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn persona_cards_stay_lean_across_languages() {
        for name in ["vivian", "nana"] {
            for (lang, max_chars) in [("zh", 5_000), ("en", 12_000), ("ja", 7_000)] {
                let card = render_dialogue_card(&default_persona_for(name), lang, &[]);
                assert!(
                    card.chars().count() <= max_chars,
                    "{name}/{lang} persona card is too long ({} chars > {max_chars}); \
                     spend the budget on recall and recent history instead",
                    card.chars().count()
                );
            }
        }
    }
}

/// 渲染风格约束块：场景模式 + 场景指令 + 禁忌 + 风格预设
///
/// 注：Voice Baseline（标志性台词）已移入 speech.md（Character 层），不再在此注入。
pub fn render_style_block(config: &PersonaConfig, scene_mode: SceneMode, lang: &str) -> String {
    let lang = crate::pipeline::prompt_modules::normalize_lang(lang);
    let mode_config = config.scene_modes.get(&scene_mode);

    let mut blocks: Vec<String> = Vec::new();

    let mode_header = crate::pipeline::prompt_modules::section_heading("performance_mode", lang);
    let instr_header = crate::pipeline::prompt_modules::section_heading("scene_instructions", lang);
    let nogo_header = crate::pipeline::prompt_modules::section_heading("no_go", lang);
    let (err_label, warn_label) = match lang {
        "zh" => ("错误", "警告"),
        "ja" => ("エラー", "警告"),
        _ => ("ERROR", "WARNING"),
    };

    blocks.push(format!("{}: {}", mode_header, scene_mode.as_str()));
    blocks.push(crate::pipeline::prompt_templates::text(lang, "scene_cues").to_owned());
    if let Some(mc) = mode_config {
        blocks.push(mc.description.clone());
    }
    blocks.push(String::new());

    if let Some(mc) = mode_config {
        if !mc.extra_instructions.is_empty() {
            blocks.push(instr_header.to_string());
            for instruction in &mc.extra_instructions {
                blocks.push(format!("- {}", instruction));
            }
            blocks.push(String::new());
        }
    }

    let active_taboos: Vec<_> = config.identity.taboos.iter().filter(|t| t.enabled).collect();
    if !active_taboos.is_empty() {
        blocks.push(nogo_header.to_string());
        for taboo in active_taboos {
            let prefix = if taboo.severity == "error" { err_label } else { warn_label };
            blocks.push(format!("{} {}", prefix, taboo.prompt_instruction));
        }
        blocks.push(String::new());
    }

    blocks.join("\n")
}

/// 渲染风格预设块（tone baseline），独立于场景模式，属于 Chat Style 框架层
pub fn render_style_preset_block(config: &PersonaConfig, lang: &str) -> String {
    let lang = crate::pipeline::prompt_modules::normalize_lang(lang);
    let preset_text = load_style_preset_localized(&config.style_preset, lang);
    if preset_text.is_empty() {
        String::new()
    } else {
        let header = crate::pipeline::prompt_modules::section_heading("style_preset", lang);
        format!("{}\n{}", header, preset_text)
    }
}

/// 渲染精简版风格约束（用于工具调用等精简场景）
pub fn render_short_style_block(config: &PersonaConfig, scene_mode: SceneMode, lang: &str) -> String {
    let mode_config = config.scene_modes.get(&scene_mode);
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

    let header = crate::pipeline::prompt_modules::section_heading("identity_short", lang);
    let identity_line = match lang_norm {
        "en" => format!("You are {}, who lives on the user's desktop.", config.identity.name),
        "ja" => format!("あなたは{}、ユーザーのデスクトップに住んでいる。", config.identity.name),
        _ => format!("你是{}，住在用户的电脑桌面上。", config.identity.name),
    };

    let mut lines: Vec<String> = vec![header.to_string(), identity_line];

    if let Some(mc) = mode_config {
        for inst in mc.extra_instructions.iter().take(2) {
            lines.push(format!("- {}", inst));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::schemas::default_persona_for;

    #[test]
    fn scoped_growth_replaces_translated_seed_without_erasing_identity() {
        for character in ["vivian", "nana"] {
            for lang in ["zh", "en", "ja"] {
                let config = default_persona_for(character);
                let seed = resolve_section(&config, CharacterSection::Personality, lang);
                let filtered = filter_seed_scenes(&seed, &["comfort"]);
                let comfort_header = seed.lines().find(|l| l.contains("growth:comfort")).unwrap();
                let plain = comfort_header.split(" <!--").next().unwrap();
                assert!(!filtered.contains(plain));
                assert!(filtered.contains(seed.lines().next().unwrap()));
                assert!(!filtered.contains("<!-- growth:"));
                assert!(filter_seed_scenes(&seed, &[]).contains(plain));
                let entry = super::super::evolution::EvolutionEntry {
                    timestamp: 1.0, kind: "tone".into(), scope: "comfort".into(),
                    text: "Listen before offering solutions".into(), reason: "explicit feedback".into(),
                    support: 1, explicit_feedback: true,
                    evidence: vec![super::super::evolution::GrowthEvidence {
                        memory_id: "m1".into(), quote: "listen first".into(),
                        timestamp: 1.0, fingerprint: "source".into(),
                    }],
                };
                let grown = render_character_block_growing(&config, lang, CharacterBlockTier::Full, &[entry.clone()]);
                let dialogue = render_dialogue_card(&config, lang, &[entry.clone()]);
                assert!(!dialogue.contains(plain));
                assert!(dialogue.contains(&config.identity.name));
                assert!(!grown.contains(plain));
                assert!(!grown.contains("WHEN_USER_SAD="));
                let identity = resolve_section(&config, CharacterSection::Identity, lang);
                assert!(!identity.trim().is_empty());
                assert!(grown.contains(identity.trim()), "growth must preserve the actual identity and boundaries, not a removed config label");
                let restored = render_character_block_growing(&config, lang, CharacterBlockTier::Full, &[]);
                assert!(restored.contains(plain));
                let mut custom = config;
                custom.personality_definition = "My explicitly edited personality".into();
                assert!(render_dialogue_card(&custom, lang, &[entry.clone()])
                    .contains("My explicitly edited personality"));
                assert!(render_character_block_growing(&custom, lang, CharacterBlockTier::Full, &[entry])
                    .contains("My explicitly edited personality"));
            }
        }
    }

    #[test]
    fn test_render_persona_flags_block_per_character() {
        let vivian = default_persona_for("vivian");
        let nana = default_persona_for("nana");
        let v = render_persona_flags_block(&vivian, "zh");
        let n = render_persona_flags_block(&nana, "zh");
        assert!(v.starts_with("[PERSONA_LOAD"));
        assert!(v.ends_with("[END PERSONA_LOAD]"));
        assert!(v.contains("IDENTITY_VIVIAN"));
        assert!(!v.contains("IDENTITY_NANA"));
        assert!(n.contains("IDENTITY_NANA"));
        assert!(!n.contains("IDENTITY_VIVIAN"));
        assert!(n.contains("PERSONALITY_GENTLE_RECEPTIVE_WITH_OWN_PREFERENCES"));
        assert!(!n.contains("REFUSE_ACT_FAKE_GENTLE"));
        assert!(v.contains("ordinary kindness") && n.contains("ordinary kindness"));
    }

    #[test]
    fn test_character_block_starts_with_perspective_not_config_flags() {
        let config = default_persona_for("vivian");
        let block = render_character_block(&config, "zh");
        assert!(block.starts_with("[CHARACTER PERSPECTIVE]"));
        assert!(!block.contains("[PERSONA_LOAD]"));
        assert!(block.contains("[PERSONA_PROTOCOL]"));
        assert!(block.contains("# Vivian · 身份"));
    }

    #[test]
    fn test_tiered_block_compact_trims_intro_sections() {
        let config = default_persona_for("vivian");
        let full = render_character_block_tiered(&config, "zh", CharacterBlockTier::Full);
        let compact = render_character_block_tiered(&config, "zh", CharacterBlockTier::Compact);

        // 完整档包含自我介绍型段落，精简档裁掉
        assert!(full.contains("# Vivian · 背景"));
        assert!(full.contains("# Vivian · 外观"));
        assert!(!compact.contains("# Vivian · 背景"));
        assert!(!compact.contains("# Vivian · 外观"));

        // 核心段落两档都保留
        assert!(compact.contains("# Vivian · 身份"));
        assert!(compact.contains("# Vivian · 说话方式"));
        assert!(compact.starts_with("[CHARACTER PERSPECTIVE]"));
        assert!(!compact.contains("[PERSONA_LOAD]"));

        // 精简档应显著短于完整档
        assert!(compact.len() < full.len());
    }

    /// 回归守卫：`LanguageStyle` 曾经是"定义了但从不渲染"的死字段
    /// （9 个字段在 schemas.rs 有定义、有默认值、可被卡片覆盖，却零读取方）。
    /// 这类 bug 静默失效、不报错，只能靠断言钉住。
    #[test]
    fn test_language_style_block_is_rendered_into_character_block() {
        let config = default_persona_for("vivian");

        let block = render_language_style_block(&config);
        assert!(block.starts_with("[LANGUAGE_STYLE]"), "缺块标记: {block}");
        assert!(block.ends_with("[/LANGUAGE_STYLE]"));
        assert!(block.contains("CATCHPHRASES"), "口头禅行缺失");
        assert!(block.contains("lol"), "默认口头禅内容缺失");
        assert!(block.contains("LENGTH_BIAS"), "长度偏好行缺失");
        assert!(block.contains("ONE_QUESTION"), "提问数约束行缺失");

        // 关闭语气词 → 该行消失（关闭项不占 token）
        let mut off = default_persona_for("vivian");
        off.language_style.use_sentence_final_particles = false;
        assert!(!render_language_style_block(&off).contains("SENTENCE_FINAL"));

        // 关键：必须真的拼进 Character 块——这才是当初断掉的那一环
        let full = render_character_block_tiered(&config, "zh", CharacterBlockTier::Full);
        assert!(
            full.contains("[LANGUAGE_STYLE]"),
            "Character 块未注入语言风格块（回归）"
        );
    }

    #[test]
    fn test_tier_from_relationship_stage() {
        assert_eq!(CharacterBlockTier::from_relationship_stage(0), CharacterBlockTier::Full);
        assert_eq!(CharacterBlockTier::from_relationship_stage(1), CharacterBlockTier::Full);
        assert_eq!(CharacterBlockTier::from_relationship_stage(2), CharacterBlockTier::Compact);
        assert_eq!(CharacterBlockTier::from_relationship_stage(4), CharacterBlockTier::Compact);
    }

    #[test]
    fn test_check_text_no_leaks() {
        assert!(check_text_for_leaks("正常文本没有占位符").is_empty());
        assert!(check_text_for_leaks("").is_empty());
    }

    #[test]
    fn test_check_text_finds_placeholder() {
        let leaks = check_text_for_leaks("你好 {master}，欢迎使用");
        assert_eq!(leaks, vec!["{master}".to_string()]);
    }

    #[test]
    fn test_check_text_finds_multiple_placeholders() {
        let leaks = check_text_for_leaks("{name} 今年 {age} 岁");
        assert_eq!(leaks.len(), 2);
        assert!(leaks.contains(&"{name}".to_string()));
        assert!(leaks.contains(&"{age}".to_string()));
    }

    #[test]
    fn test_check_text_ignores_double_brace_escape() {
        // `{{name}}` 是转义形式，不应被检测为占位符泄露
        let leaks = check_text_for_leaks("使用 {{name}} 转义");
        assert!(leaks.is_empty(), "双花括号转义不应被检测: {:?}", leaks);
    }

    #[test]
    fn test_check_text_ignores_non_identifier_braces() {
        // `{0}` / `{123}` 不匹配（首字符必须是字母或下划线）
        let leaks = check_text_for_leaks("索引 {0} 和 {123}");
        assert!(leaks.is_empty(), "数字开头不应匹配: {:?}", leaks);
    }

    #[test]
    fn test_check_text_underscore_identifier() {
        let leaks = check_text_for_leaks("变量 {_user_name} 值");
        assert_eq!(leaks, vec!["{_user_name}".to_string()]);
    }

    #[test]
    fn test_check_messages_system_role_only() {
        let messages = vec![
            ChatMessage::system("你好 {master}"),
            ChatMessage::user("我的 JSON 是 {\"key\": \"value\"}"),
            ChatMessage::assistant("回复 {something}"),
        ];
        // 只扫描 system role：user/assistant 的 {...} 不算泄露
        // 注意：此函数在 test 模式下会 panic，所以我们用 std::panic::catch_unwind
        let result = std::panic::catch_unwind(|| {
            check_messages_for_leaks(&messages, "test_context")
        });
        assert!(result.is_err(), "system role 有 {{master}} 应在 test 模式 panic");
    }

    #[test]
    fn test_check_messages_no_leaks_no_panic() {
        let messages = vec![
            ChatMessage::system("正常 system 消息"),
            ChatMessage::user("用户消息 {not_scanned}"),
        ];
        // 无泄露，不应 panic
        check_messages_for_leaks(&messages, "");
    }

    #[test]
    fn test_check_messages_empty() {
        check_messages_for_leaks(&[], "");
    }

    #[test]
    fn test_check_text_strict_no_panic() {
        // 无占位符时不 panic
        check_text_for_leaks_strict("正常文本", "");
    }
}
