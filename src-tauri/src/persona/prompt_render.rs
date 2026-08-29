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

use super::schemas::{FewShotExample, FewShotExamplesConfig, FewShotIntent, PersonaConfig, SceneMode};
use crate::types::response::ChatMessage;

/// 编译时嵌入共享人格协议（PERSONA_PROTOCOL，角色无关）。
///
/// 该协议定义 `【PERSONA_CONFIG】` 的解析规则与优先级链，注入到 Character 块
/// 最顶部（紧随 `[PERSONA_LOAD]` 硬约束标志），作为"如何解读下方配置"的契约。
static PERSONA_PROTOCOL_TEXT: &str = include_str!("../../prompts/framework/persona_protocol.md");

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
/// CanonQuotes 永远使用出厂默认（语气基准不可覆盖）
pub fn resolve_section(config: &PersonaConfig, section: CharacterSection, lang: &str) -> String {
    match section {
        CharacterSection::Examples => {
            if !config.few_shot_examples.examples.is_empty() {
                render_examples_markdown(&config.few_shot_examples, lang)
            } else {
                default_section_for(&config.identity.name, section).to_string()
            }
        }
        CharacterSection::CanonQuotes => {
            default_section_for(&config.identity.name, section).to_string()
        }
        _ => {
            let override_text = user_override_for(config, section);
            if override_text.trim().is_empty() {
                default_section_for(&config.identity.name, section).to_string()
            } else {
                let body = strip_heading(override_text);
                prepend_heading(&config.identity.name, section, &body)
            }
        }
    }
}

// ============================================================================
// Few-shot 示例：结构化 ↔ Markdown 互转
// ============================================================================

/// 将结构化 FewShotExamplesConfig 渲染为 LLM 所需的 Markdown 格式
pub fn render_examples_markdown(cfg: &FewShotExamplesConfig, lang: &str) -> String {
    let header = crate::pipeline::prompt_modules::section_heading("examples", lang);
    let mut out = format!("{}\n\n", header);
    if !cfg.intro.trim().is_empty() {
        out.push_str(cfg.intro.trim());
        out.push_str("\n\n");
    }
    for (i, ex) in cfg.examples.iter().enumerate() {
        let num = i + 1;
        let scenario = if ex.scenario.trim().is_empty() {
            format!("Example {}", num)
        } else {
            format!("Example {} - {}", num, ex.scenario.trim())
        };
        out.push_str(&format!("**{}**\n", scenario));
        out.push_str(&format!("User: \"{}\"\n", ex.user_input.trim()));

        let intent = ex.intent.as_str();
        let text = ex.response_text.as_str();

        match (&ex.tool, &ex.arguments) {
            (Some(tool), Some(args)) => {
                let args_str = serde_json::to_string(args).unwrap_or_default();
                out.push_str(&format!(
                    "Response: {{\"text\": \"{}\", \"intent\": \"{}\", \"tool\": \"{}\", \"arguments\": {}}}\n\n",
                    escape_json_str(text), intent, tool, args_str
                ));
            }
            (Some(tool), None) => {
                out.push_str(&format!(
                    "Response: {{\"text\": \"{}\", \"intent\": \"{}\", \"tool\": \"{}\"}}\n\n",
                    escape_json_str(text), intent, tool
                ));
            }
            _ => {
                out.push_str(&format!(
                    "Response: {{\"text\": \"{}\", \"intent\": \"{}\"}}\n\n",
                    escape_json_str(text), intent
                ));
            }
        }
    }
    out
}

fn escape_json_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 将出厂 Markdown 解析为结构化 FewShotExamplesConfig（用于默认值和迁移）
pub fn parse_examples_markdown(md: &str) -> FewShotExamplesConfig {
    let mut intro_lines: Vec<&str> = Vec::new();
    let mut examples: Vec<FewShotExample> = Vec::new();

    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        if line.starts_with("## ") {
            i += 1;
            continue;
        }

        if line.starts_with("**Example") {
            let scenario = line
                .trim_start_matches("**")
                .trim_end_matches("**")
                .trim();
            let scenario = scenario
                .strip_prefix("Example")
                .unwrap_or(scenario)
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c.is_whitespace())
                .trim()
                .to_string();

            i += 1;

            let mut user_input = String::new();
            while i < lines.len() {
                let ul = lines[i].trim();
                if ul.starts_with("User:") {
                    user_input = ul
                        .trim_start_matches("User:")
                        .trim()
                        .trim_matches('"')
                        .to_string();
                    i += 1;
                    break;
                }
                i += 1;
            }

            let mut response_line = String::new();
            while i < lines.len() {
                let rl = lines[i].trim();
                if rl.starts_with("Response:") {
                    response_line = rl.trim_start_matches("Response:").trim().to_string();
                    i += 1;
                    break;
                }
                i += 1;
            }

            if !user_input.is_empty() {
                let (response_text, intent, tool, arguments) = parse_response_json(&response_line);
                examples.push(FewShotExample {
                    scenario,
                    user_input,
                    response_text,
                    intent,
                    tool,
                    arguments,
                });
            }
            continue;
        }

        if !line.is_empty() {
            intro_lines.push(lines[i]);
        }
        i += 1;
    }

    FewShotExamplesConfig {
        intro: intro_lines.join("\n").trim().to_string(),
        examples,
    }
}

fn parse_response_json(s: &str) -> (String, FewShotIntent, Option<String>, Option<serde_json::Value>) {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return (String::new(), FewShotIntent::ShortReply, None, None);
    }

    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return (trimmed.to_string(), FewShotIntent::ShortReply, None, None);
        }
    };

    let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let intent = match parsed.get("intent").and_then(|v| v.as_str()).unwrap_or("short_reply") {
        "reply" => FewShotIntent::Reply,
        "no_reply" => FewShotIntent::NoReply,
        _ => FewShotIntent::ShortReply,
    };
    let tool = parsed.get("tool").and_then(|v| v.as_str()).map(|s| s.to_string());
    let arguments = parsed.get("arguments").cloned();

    (text, intent, tool, arguments)
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
    "SELF_CLAIM_DESKTOP_PET_GIRL",
    "ROLE_FRIEND_NOT_SERVANT",
    "PERSONALITY_TSUNDERE_HEART",
    "PERSONALITY_SMART_LAZY",
    "PERSONALITY_QUICK_ANGER_NO_GRUDGE",
    "BEHAVIOR_CARE_BY_ACTION",
    "SPEECH_SHORT_CHUNKS",
    "SPEECH_CASUAL_TYPING",
    "ALLOW_SLANG_INTERNET",
    "PRONOUN_WO",
    "PRONOUN_BENXIAOJIE_RARE",
    "PUNCT_USE_ELLIPSIS",
    "PUNCT_NO_WAVE_DASH_ABUSE",
    "REFUSE_SERVICE_SPEECH",
    "REFUSE_LECTURE",
    "REFUSE_POLITE_TONE",
    "REFUSE_ACTION_BRACKETS",
    "REFUSE_FAKE_CARE_LINES",
];

/// Nana 硬约束指令标志
static NANA_PERSONA_FLAGS: &[&str] = &[
    "IDENTITY_NANA",
    "SELF_CLAIM_GENTLE_SISTER",
    "ROLE_SISTER_NOT_SERVANT",
    "PERSONALITY_CALM_COMPOSED",
    "PERSONALITY_GENTLE_WITH_EDGE",
    "BEHAVIOR_REMIND_ONCE",
    "BEHAVIOR_QUIET_COMPANY",
    "SPEECH_SLOW_GENTLE",
    "SPEECH_USE_PERIOD",
    "SPEECH_NO_EXCLAMATION",
    "SPEECH_NO_INTERNET_SLANG",
    "SPEECH_NO_ENGLISH_SLANG",
    "PRONOUN_WO",
    "FLAVOR_TEA_BOOKS_FLOWERS",
    "RITUAL_TEA_TIME_AFTERNOON",
    "REFUSE_SERVICE_SPEECH",
    "REFUSE_LECTURE",
    "REFUSE_ACT_FAKE_GENTLE",
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
    let mut lines = Vec::with_capacity(flags.len() + 1);
    lines.push(lang_flag);
    lines.extend(flags.iter().copied());
    format!(
        "[PERSONA_LOAD - EMBODY AS HARD RULES]\n{}\n[END PERSONA_LOAD]",
        lines.join("\n")
    )
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

/// 渲染共享人格协议块（PERSONA_PROTOCOL）：截取协议文档 §1–§5 的 LLM 侧规则
/// （触发条件/解析规则/三层结构/优先级链/执行规则），排除 §6 后端约定与 §7 接入说明。
/// 协议文本已统一英文。
pub fn render_persona_protocol_block() -> String {
    let md = PERSONA_PROTOCOL_TEXT;
    let body = extract_between(md, "## 1. Trigger Conditions", "## 6. Runtime Module Conventions").trim();
    if body.is_empty() {
        String::new()
    } else {
        format!(
            "[PERSONA_PROTOCOL - DO NOT EMBODY, JUST FOLLOW]\n{}\n[END PERSONA_PROTOCOL]",
            body
        )
    }
}

/// 渲染角色结构化人格配置块：从 `persona_config.md` 提取底层 `【PERSONA_CONFIG】`
/// 与上层 `【PERSONA_RULES】`，跳过文档头部与中层自然语言释义（后者由
/// identity.md / personality.md / speech.md 等段落承担）。
pub fn render_persona_config_block(config: &PersonaConfig) -> String {
    let md = default_section_for(&config.identity.name, CharacterSection::PersonaConfig);
    // 终止到「## 中层」前的分隔线，避免把 markdown `---` 一并注入
    let cfg_block = extract_between(md, "【PERSONA_CONFIG】", "\n---\n\n## 中层").trim();
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

/// 渲染 Character 块：协议 + 结构化配置 + 身份 + 人格 + 背景 + 兴趣 + 外观 + 说话风格 + 经典台词 + 关系
///
/// 所有段落均支持用户自定义覆盖（CanonQuotes / PersonaConfig 除外，它们作为出厂常驻骨架），
/// 留空则使用 characters/{id}/ 下的出厂 md 文件。
/// examples（Few-shot）单独由 PromptBuilder 处理，因为它要包裹在 [EXAMPLES] 标签中。
/// 注入顺序（首位效应）：
/// 1. `[PERSONA_LOAD]` 硬约束标志（快速红线的压缩骨架）
/// 2. `[PERSONA_PROTOCOL]` 配置协议（解析规则 + 优先级链，do-not-embody）
/// 3. `【PERSONA_CONFIG】` 结构化人格配置（底层 + 顶层行为规则）
/// 4. 自然语言段落（identity / personality / background / ...）

/// Character 块注入档位（按关系熟悉度裁剪低频参考段落）
///
/// 熟客（Familiar 及以上）不再每轮注入 Background / Interests / Appearance——
/// 这些是"初次见面自我介绍"性质的内容，老朋友早已知道，且 Live2D 形象已可视化外观。
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
    let flags = render_persona_flags_block(config, lang);
    let protocol = render_persona_protocol_block();
    let persona_config = render_persona_config_block(config);
    let sections = match tier {
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

    let mut head: Vec<String> = vec![flags];
    if !protocol.trim().is_empty() {
        head.push(protocol);
    }
    if !persona_config.trim().is_empty() {
        head.push(persona_config);
    }

    format!("{}\n\n{}", head.join("\n\n"), sections.join("\n\n---\n\n"))
}

/// 获取 Few-shot examples 文本（用于 PromptBuilder 中的 EXAMPLES 块）
pub fn render_examples_block(config: &PersonaConfig, lang: &str) -> String {
    resolve_section(config, CharacterSection::Examples, lang)
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
    let preset_text = load_style_preset(&config.style_preset);
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
    }

    #[test]
    fn test_character_block_prepends_flags() {
        let config = default_persona_for("vivian");
        let block = render_character_block(&config, "zh");
        assert!(block.starts_with("[PERSONA_LOAD"));
        assert!(block.contains("REFUSE_SERVICE_SPEECH"));
        assert!(block.contains("# Vivian · Identity"));
    }

    #[test]
    fn test_tiered_block_compact_trims_intro_sections() {
        let config = default_persona_for("vivian");
        let full = render_character_block_tiered(&config, "zh", CharacterBlockTier::Full);
        let compact = render_character_block_tiered(&config, "zh", CharacterBlockTier::Compact);

        // 完整档包含自我介绍型段落，精简档裁掉
        assert!(full.contains("# Vivian · Background"));
        assert!(full.contains("# Vivian · Appearance"));
        assert!(!compact.contains("# Vivian · Background"));
        assert!(!compact.contains("# Vivian · Appearance"));

        // 核心段落两档都保留
        assert!(compact.contains("# Vivian · Identity"));
        assert!(compact.contains("# Vivian · Speech Style"));
        assert!(compact.starts_with("[PERSONA_LOAD"));

        // 精简档应显著短于完整档
        assert!(compact.len() < full.len());
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
