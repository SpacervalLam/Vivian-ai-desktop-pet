//! 资源清单归一化 + 守门员 + 模型语义映射
//!
//! 从每个模型目录下的 `model_manifest.json` 加载：
//! - 表情语义映射（semantic name → 注册动作/表情名）
//! - 别名归一化表（LLM 通用名 → 语义名）
//! - 回退候选链
//! - 14 类情绪 → 表情语义名映射
//! - 交互反馈映射（fast_click/pet/long_press 等）
//! - 动作别名归一化表

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::pre_parsed::chibi_animations_json;
use super::resource_loader::ResourceLoader;

/// 单个表情的语义映射
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpressionMapping {
    /// 语义名（LLM 输出 / 内部统一标识）
    pub semantic: String,
    /// 注册的动作/表情名
    pub name: String,
    /// 显示标签
    pub label: String,
    /// 分类："emotion" | "action"
    pub category: String,
}

/// 交互反馈映射
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InteractionMapping {
    pub expression: String,
    #[serde(default)]
    pub motion: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// 空闲触发映射（按空闲时长阈值触发）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdleTrigger {
    /// 空闲阈值（秒）
    pub threshold_secs: u64,
    pub expression: String,
    #[serde(default)]
    pub motion: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub probability: f64,
}

/// 程序事件触发映射
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EventTrigger {
    pub expression: String,
    #[serde(default)]
    pub motion: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub probability: f64,
}

/// 心情持续表情配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MoodIdleExpression {
    pub expression: String,
    #[serde(default)]
    pub priority: i32,
}

/// 模型清单 — 从 model_manifest.json 加载
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelManifest {
    /// 模型显示名
    pub display_name: String,
    /// 模型清单文件名
    pub model_file: String,
    /// 模型后端类型（sprite / mmd / vrm / pngtuber）
    #[serde(default = "default_model_kind")]
    pub model_kind: String,
    /// 表情语义映射列表
    #[serde(default)]
    pub expressions: Vec<ExpressionMapping>,
    /// 别名归一化表（LLM 通用名 / 中文 → 语义名）
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// 表情回退候选链（语义名）
    #[serde(default)]
    pub fallbacks: Vec<String>,
    /// 14 类情绪 → 表情语义名映射（空串表示不设置表情）
    #[serde(default)]
    pub emotion_map: HashMap<String, String>,
    /// 可用动作列表
    #[serde(default)]
    pub motions: Vec<String>,
    /// 动作别名归一化表
    #[serde(default)]
    pub motion_aliases: HashMap<String, String>,
    /// 交互反馈映射（扩展：single_click/double_click/drag_start/drag_end/mouse_enter/mouse_leave等）
    #[serde(default)]
    pub interaction_map: HashMap<String, InteractionMapping>,
    /// 心情状态 → 表情池（用于空闲时心情随机触发，让桌宠更生动）
    ///
    /// key 为情绪标签（joy/closeness/curiosity/sadness/loneliness/anger/fear）
    /// 或派生心情（tired/bored）。value 为该心情下可随机触发的表情语义名列表。
    #[serde(default)]
    pub mood_triggers: HashMap<String, Vec<String>>,
    /// 心情持续表情：主导情绪持续一段时间后显示的基调表情（低优先级，可被其他表情覆盖）
    /// key 为情绪标签，value 为对应表情配置
    #[serde(default)]
    pub mood_idle_expressions: HashMap<String, MoodIdleExpression>,
    /// 空闲触发：按空闲时长阈值触发不同表情/动作
    /// key 为事件标识（如 "idle_30s"/"idle_60s"/"idle_180s"/"idle_300s"/"user_return"）
    #[serde(default)]
    pub idle_triggers: HashMap<String, IdleTrigger>,
    /// 程序事件触发：key 为事件类型
    /// 支持事件：morning/afternoon/evening/night/window_focus/window_blur/
    ///          music_start/music_stop/battery_low/chat_start/chat_end
    #[serde(default)]
    pub event_triggers: HashMap<String, EventTrigger>,
    /// 显示缩放系数（默认 1.0），用于补偿模型画布留白。
    /// 留白较多的模型可设 > 1.0（如 1.3），使角色视觉大小与其他模型对齐。
    #[serde(default = "default_display_scale")]
    pub display_scale: f64,
}

fn default_display_scale() -> f64 {
    1.0
}

fn default_model_kind() -> String {
    "sprite".to_string()
}

/// 取 JSON 对象里的字符串字段；类型不符或缺失时返回 `None`。
fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| field.as_str()).map(str::to_string)
}

/// 把 JSON 对象里的字符串键值对收进 map，非字符串值直接跳过。
fn collect_str_map(
    root: &serde_json::Value,
    key: &str,
    out: &mut HashMap<String, String>,
) {
    if let Some(map) = root.get(key).and_then(|value| value.as_object()) {
        for (field, value) in map {
            if let Some(value) = value.as_str() {
                out.insert(field.clone(), value.to_string());
            }
        }
    }
}

impl ModelManifest {
    /// 从模型目录加载动作清单。
    ///
    /// 动作词汇由 `src/chibi/animations.json` 声明（构建期嵌入二进制），
    /// 按 `model_dir` 目录名匹配角色；目录名不在词汇表角色列表中时返回 `None`。
    pub fn load_from_dir(model_dir: &Path) -> Option<Self> {
        Self::from_chibi_vocab(model_dir)
    }

    /// 从 Q 版动作词汇表构建清单。
    ///
    /// Q 版渲染只有一套词汇：图集格位是可停留的神态，帧序列是一次性动作。这里把
    /// 两者归一到既有结构，于是情绪映射、prompt 动作清单、空闲与事件触发等链路
    /// 无需区分渲染后端即可继续工作。
    ///
    /// 只有词表里标了 `promptable` 的条目会写进 prompt；`category` 决定它落在
    /// 「表情」还是「动作」一栏。两栏刻意不重叠——一次回复里可以同时挑一个神态和
    /// 一个动作，词表重叠会让 LLM 把同一个动作填进两个字段。
    fn from_chibi_vocab(model_dir: &Path) -> Option<Self> {
        let char_id = model_dir.file_name()?.to_str()?.to_lowercase();
        let root: serde_json::Value = serde_json::from_str(chibi_animations_json()?).ok()?;
        if !root
            .get("characters")?
            .as_array()?
            .iter()
            .any(|id| id.as_str() == Some(char_id.as_str()))
        {
            return None;
        }

        let mut expressions: Vec<ExpressionMapping> = Vec::new();
        let mut motions: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();

        // 帧序列排在格位之前：同名时以表现更完整的帧序列为准
        let entries = root
            .get("animations")
            .and_then(|animations| animations.as_array())
            .into_iter()
            .flatten()
            .chain(
                root.get("atlas")
                    .and_then(|atlas| atlas.get("poses"))
                    .and_then(|poses| poses.as_array())
                    .into_iter()
                    .flatten(),
            );

        for entry in entries {
            if entry.get("promptable").and_then(|value| value.as_bool()) != Some(true) {
                continue;
            }
            let name = match entry.get("name").and_then(|value| value.as_str()) {
                Some(name) => name,
                None => continue,
            };
            if !seen.insert(name) {
                continue;
            }
            if entry.get("category").and_then(|value| value.as_str()) == Some("action") {
                motions.push(name.to_string());
            } else {
                expressions.push(ExpressionMapping {
                    semantic: name.to_string(),
                    name: name.to_string(),
                    label: json_str(entry, "label").unwrap_or_else(|| name.to_string()),
                    category: "emotion".to_string(),
                });
            }
        }

        let mut aliases = HashMap::new();
        collect_str_map(&root, "expression_aliases", &mut aliases);

        let mut emotion_map = HashMap::new();
        collect_str_map(&root, "emotion_map", &mut emotion_map);

        let mut mood_idle_expressions = HashMap::new();
        if let Some(map) = root.get("mood_idle_expressions").and_then(|value| value.as_object()) {
            for (key, value) in map {
                mood_idle_expressions.insert(
                    key.clone(),
                    MoodIdleExpression {
                        expression: json_str(value, "expression").unwrap_or_default(),
                        priority: value
                            .get("priority")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(0) as i32,
                    },
                );
            }
        }

        let mut idle_triggers = HashMap::new();
        if let Some(map) = root.get("idle_triggers").and_then(|value| value.as_object()) {
            for (key, value) in map {
                idle_triggers.insert(
                    key.clone(),
                    IdleTrigger {
                        threshold_secs: value
                            .get("threshold_secs")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                        expression: json_str(value, "expression").unwrap_or_default(),
                        motion: json_str(value, "motion").unwrap_or_default(),
                        action: String::new(),
                        duration_ms: value.get("duration_ms").and_then(|value| value.as_u64()),
                        probability: value
                            .get("probability")
                            .and_then(|value| value.as_f64())
                            .unwrap_or(0.0),
                    },
                );
            }
        }

        let mut event_triggers = HashMap::new();
        if let Some(map) = root.get("event_triggers").and_then(|value| value.as_object()) {
            for (key, value) in map {
                event_triggers.insert(
                    key.clone(),
                    EventTrigger {
                        expression: json_str(value, "expression").unwrap_or_default(),
                        motion: json_str(value, "motion").unwrap_or_default(),
                        action: String::new(),
                        duration_ms: value.get("duration_ms").and_then(|value| value.as_u64()),
                        probability: value
                            .get("probability")
                            .and_then(|value| value.as_f64())
                            .unwrap_or(0.0),
                    },
                );
            }
        }

        tracing::info!(
            "[ModelManifest] 从 Q 版动作词汇表加载成功: {} (expressions={}, motions={})",
            char_id,
            expressions.len(),
            motions.len()
        );

        Some(Self {
            display_name: char_id,
            model_file: String::new(),
            model_kind: "sprite".to_string(),
            expressions,
            aliases,
            fallbacks: Vec::new(),
            emotion_map,
            motions,
            motion_aliases: HashMap::new(),
            interaction_map: HashMap::new(),
            mood_triggers: HashMap::new(),
            mood_idle_expressions,
            idle_triggers,
            event_triggers,
            display_scale: 1.0,
        })
    }

    /// 语义名 → 注册表情名
    pub fn resolve_expression_name(&self, semantic: &str) -> Option<&str> {
        self.expressions
            .iter()
            .find(|e| e.semantic == semantic)
            .map(|e| e.name.as_str())
    }

    /// 所有表情的 注册名（供前端按名取表情）
    pub fn expression_names(&self) -> Vec<&str> {
        self.expressions.iter().map(|e| e.name.as_str()).collect()
    }

    /// 所有表情的语义名（供 LLM prompt 使用）
    pub fn semantic_names(&self) -> Vec<&str> {
        self.expressions.iter().map(|e| e.semantic.as_str()).collect()
    }

    /// 推荐给 LLM 的表情语义名（emotion 类别优先）
    pub fn prompt_semantic_names(&self) -> Vec<&str> {
        let mut emotion: Vec<&str> = self
            .expressions
            .iter()
            .filter(|e| e.category == "emotion")
            .map(|e| e.semantic.as_str())
            .collect();
        let action: Vec<&str> = self
            .expressions
            .iter()
            .filter(|e| e.category == "action")
            .map(|e| e.semantic.as_str())
            .collect();
        emotion.extend(action);
        emotion
    }

    /// 14 类情绪 → 表情语义名（空串表示不设置）
    pub fn emotion_to_expression(&self, emotion: &str) -> &str {
        self.emotion_map
            .get(emotion)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// 交互类型 → 反馈映射
    pub fn interaction_feedback(&self, interaction: &str) -> Option<&InteractionMapping> {
        self.interaction_map.get(interaction)
    }

    /// 心情标签 → 表情池（语义名列表）
    pub fn mood_trigger_pool(&self, mood: &str) -> Option<&Vec<String>> {
        self.mood_triggers.get(mood)
    }

    /// 从心情表情池中随机选取一个表情（返回 注册表情名）
    ///
    /// 池为空或语义名无法解析时返回 None。
    pub fn random_mood_expression(&self, mood: &str) -> Option<String> {
        let pool = self.mood_trigger_pool(mood)?;
        if pool.is_empty() {
            return None;
        }
        let idx = rand::random::<u64>() as usize % pool.len();
        let semantic = &pool[idx];
        self.resolve_expression_name(semantic).map(|s| s.to_string())
    }

    /// 表情是否存在（按 注册名）
    pub fn has_expression_name(&self, name: &str) -> bool {
        self.expressions.iter().any(|e| e.name == name)
    }

    /// 动作是否存在
    pub fn has_motion(&self, name: &str) -> bool {
        self.motions.iter().any(|m| m == name)
    }
}

/// 默认动作名
pub const DEFAULT_MOTION: &str = "idle";

/// 已知表情语义名的说明映射（emotion 类）
///
/// 未在此表中的语义名会回退到通用描述 "conveys emotion"
fn describe_emotion_semantics(names: &[String]) -> String {
    let descriptions: HashMap<&str, &str> = HashMap::from([
        ("star_eyes", "unexpectedly rewarded (eyes light up with stars)"),
        ("star_aura", "thrilled / sudden joy (sparkle aura around face)"),
        ("shy", "embarrassed/blushing"),
        ("blush_intense", "deeply flustered"),
        ("love_eyes", "affectionate/grateful"),
        ("angry", "mad"),
        ("angry_symbol", "mad"),
        ("cry", "sad/tears"),
        ("tears", "sad/tears"),
        ("confused", "puzzled"),
        ("confused_intense", "very puzzled"),
        ("speechless", "exasperated"),
        ("sweat", "anxious"),
        ("pout", "mild displeasure"),
        ("puff_cheek", "sulking"),
        ("dark_face", "frustrated/serious"),
        ("blank_eyes", "dazed/bored"),
        ("money_eyes", "greedy"),
        ("tongue_out", "playful defiance"),
        ("rinnegan", "dramatic/powerful"),
        ("pupil_color", "mood-based eye color shift"),
        ("dizzy", "dazed/confused (spiral eyes)"),
        ("blindfold", "mysterious/tired (eye mask)"),
    ]);
    let parts: Vec<String> = names
        .iter()
        .map(|n| {
            match descriptions.get(n.as_str()) {
                Some(desc) => format!("{} = {}", n, desc),
                None => format!("{} = conveys emotion", n),
            }
        })
        .collect();
    parts.join(", ")
}

/// 已知动作表情语义名的说明映射（action 类）
fn describe_action_semantics(names: &[String]) -> String {
    let descriptions: HashMap<&str, &str> = HashMap::from([
        ("mirror", "selfie/mirror"),
        ("gaming", "playing games"),
        ("notebook", "writing/reading"),
        ("notebook2", "writing/reading (alt)"),
        ("fan", "fanning"),
        ("microphone", "singing/speaking"),
        ("hold_fox", "hugging a fox plushie"),
        ("heart_hands", "finger heart"),
        ("ear_drop", "ear blow (flirty)"),
        ("fox", "holding fox ears"),
        ("long_hair", "appearance: long hair"),
        ("twin_tails", "appearance: twin tails"),
        ("blood_1", "blood stain (mild)"),
        ("blood_2", "blood stain (intense)"),
        ("reach_hand", "reaching out hand"),
        ("holding_knife", "holding a knife"),
    ]);
    let parts: Vec<String> = names
        .iter()
        .map(|n| {
            match descriptions.get(n.as_str()) {
                Some(desc) => format!("{} = {}", n, desc),
                None => format!("{} = action", n),
            }
        })
        .collect();
    parts.join(", ")
}

/// 资源清单 — 从 ResourceLoader 动态扫描 + ModelManifest 语义映射
pub struct ResourceManifest {
    /// 可用表情名列表（注册表情名）
    expressions: Vec<String>,
    /// 可用动作名列表
    motions: Vec<String>,
    /// 模型清单（语义映射）
    model_manifest: Option<ModelManifest>,
}

impl ResourceManifest {
    /// 从 ResourceLoader 构建清单
    pub fn from_loader(loader: &ResourceLoader) -> Self {
        let model_manifest = ModelManifest::load_from_dir(loader.model_dir());

        // 优先使用 manifest 中的表情/动作列表，降级到 ResourceLoader 扫描结果
        let expressions = if let Some(ref mf) = model_manifest {
            mf.expressions.iter().map(|e| e.name.clone()).collect()
        } else {
            loader.list_expression_names()
        };
        let motions = if let Some(ref mf) = model_manifest {
            mf.motions.clone()
        } else {
            loader.list_motion_names()
        };

        Self {
            expressions,
            motions,
            model_manifest,
        }
    }

    /// 从 ModelManifest 构建清单（用于模型切换 / 测试）
    pub fn from_manifest(mf: ModelManifest) -> Self {
        let expressions = mf.expressions.iter().map(|e| e.name.clone()).collect();
        let motions = mf.motions.clone();
        Self {
            expressions,
            motions,
            model_manifest: Some(mf),
        }
    }

    /// 表情列表（注册名，只读）
    pub fn expressions(&self) -> &[String] {
        &self.expressions
    }

    /// 动作列表（只读）
    pub fn motions(&self) -> &[String] {
        &self.motions
    }

    /// 模型清单
    pub fn model_manifest(&self) -> Option<&ModelManifest> {
        self.model_manifest.as_ref()
    }

    /// 表情是否存在（按 注册名）
    fn has_expression(&self, name: &str) -> bool {
        self.expressions.iter().any(|e| e == name)
    }

    /// 动作是否存在
    fn has_motion(&self, name: &str) -> bool {
        self.motions.iter().any(|m| m == name)
    }

    /// 守门员：归一化表情名 + 回退链
    ///
    /// 流程：
    /// 1. 空名/default/neutral → 返回空串（不指定，由调用方决定如何处理）
    /// 2. 查别名表归一化到语义名 → 再映射到 注册名
    /// 3. 原名直接存在 → 返回
    /// 4. 按回退候选链查找第一个存在的
    /// 5. 全部失败 → 返回空串（遵循"无匹配时留空，不强制使用"原则）
    pub fn normalize_expression(&self, name: &str) -> String {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed == "default" || trimmed == "neutral" {
            return String::new();
        }

        // 有 manifest：走语义映射路径
        if let Some(ref mf) = self.model_manifest {
            // 别名归一化 → 语义名
            let lower = trimmed.to_lowercase();
            let semantic = mf
                .aliases
                .get(lower.as_str())
                .or_else(|| mf.aliases.get(trimmed))
                .map(|s| s.as_str())
                .unwrap_or(trimmed);

            // 语义名 → 注册名
            if let Some(expr_name) = mf.resolve_expression_name(semantic) {
                if self.has_expression(expr_name) {
                    return expr_name.to_string();
                }
            }

            // 原名可能是 注册名，直接检查
            if self.has_expression(trimmed) {
                return trimmed.to_string();
            }

            // 回退候选链（语义名 → Name）
            for fb in &mf.fallbacks {
                if let Some(expr_name) = mf.resolve_expression_name(fb) {
                    if self.has_expression(expr_name) {
                        return expr_name.to_string();
                    }
                }
            }

            // 全部失败 → 留空（遵循"无匹配时留空，不强制使用"原则）
            return String::new();
        }

        // 无 manifest 降级：原名直接检查
        if self.has_expression(trimmed) {
            return trimmed.to_string();
        }
        String::new()
    }

    /// 守门员：归一化动作名 + 回退
    pub fn normalize_motion(&self, name: &str) -> String {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return DEFAULT_MOTION.to_string();
        }

        // 有 manifest：走别名归一化
        if let Some(ref mf) = self.model_manifest {
            let lower = trimmed.to_lowercase();
            let normalized = mf
                .motion_aliases
                .get(lower.as_str())
                .or_else(|| mf.motion_aliases.get(trimmed))
                .map(|s| s.as_str())
                .unwrap_or(trimmed);

            if self.has_motion(normalized) {
                return normalized.to_string();
            }
        }

        // 原名存在
        if self.has_motion(trimmed) {
            return trimmed.to_string();
        }

        // 回退到 idle
        if self.has_motion(DEFAULT_MOTION) {
            return DEFAULT_MOTION.to_string();
        }

        // 第一个可用动作
        self.motions
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_MOTION.to_string())
    }

    /// 14 类情绪 → 表情语义名 → 注册名
    ///
    /// 返回 注册表情名。无 manifest 或映射为空时返回空串。
    pub fn emotion_to_expression_name(&self, emotion: &str) -> String {
        let mf = match self.model_manifest.as_ref() {
            Some(mf) => mf,
            None => return String::new(),
        };
        let semantic = mf.emotion_to_expression(emotion);
        if semantic.is_empty() {
            return String::new();
        }
        mf.resolve_expression_name(semantic)
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// 交互类型 → (expression name, motion name) 反馈
    ///
    /// 返回 (注册表情名, motion name)。
    /// 无 manifest 或映射缺失时返回 ("", DEFAULT_MOTION)。
    pub fn interaction_feedback_names(&self, interaction: &str) -> (String, String) {
        let mf = match self.model_manifest.as_ref() {
            Some(mf) => mf,
            None => return (String::new(), DEFAULT_MOTION.to_string()),
        };
        let fb = match mf.interaction_feedback(interaction) {
            Some(fb) => fb,
            None => return (String::new(), DEFAULT_MOTION.to_string()),
        };
        let expr_name = if fb.expression.is_empty()
            || fb.expression == "default"
            || fb.expression == "idle"
        {
            String::new()
        } else {
            mf.resolve_expression_name(&fb.expression)
                .map(|s| s.to_string())
                .unwrap_or_else(|| fb.expression.clone())
        };
        let motion = if fb.motion.is_empty() {
            DEFAULT_MOTION.to_string()
        } else {
            self.normalize_motion(&fb.motion)
        };
        (expr_name, motion)
    }

    /// 交互类型 → 完整反馈（表情+动作+前端动作+持续时间）
    ///
    /// 返回 (注册表情名, motion name, frontend action, duration_ms)。
    pub fn interaction_feedback_full(&self, interaction: &str) -> (String, String, String, Option<u64>) {
        let mf = match self.model_manifest.as_ref() {
            Some(mf) => mf,
            None => return (String::new(), DEFAULT_MOTION.to_string(), String::new(), None),
        };
        let fb = match mf.interaction_feedback(interaction) {
            Some(fb) => fb,
            None => return (String::new(), DEFAULT_MOTION.to_string(), String::new(), None),
        };
        let expr_name = if fb.expression.is_empty()
            || fb.expression == "default"
            || fb.expression == "idle"
        {
            String::new()
        } else {
            mf.resolve_expression_name(&fb.expression)
                .map(|s| s.to_string())
                .unwrap_or_else(|| fb.expression.clone())
        };
        let motion = if fb.motion.is_empty() {
            DEFAULT_MOTION.to_string()
        } else {
            self.normalize_motion(&fb.motion)
        };
        (expr_name, motion, fb.action.clone(), fb.duration_ms)
    }

    /// 获取空闲触发配置
    pub fn get_idle_trigger(&self, trigger_key: &str) -> Option<(String, String, String, Option<u64>, f64)> {
        let mf = self.model_manifest.as_ref()?;
        let trigger = mf.idle_triggers.get(trigger_key)?;
        let expr_name = if trigger.expression.is_empty()
            || trigger.expression == "default"
            || trigger.expression == "idle"
        {
            String::new()
        } else {
            mf.resolve_expression_name(&trigger.expression)
                .map(|s| s.to_string())
                .unwrap_or_else(|| trigger.expression.clone())
        };
        let motion = if trigger.motion.is_empty() {
            DEFAULT_MOTION.to_string()
        } else {
            self.normalize_motion(&trigger.motion)
        };
        Some((expr_name, motion, trigger.action.clone(), trigger.duration_ms, trigger.probability))
    }

    /// 获取程序事件触发配置
    pub fn get_event_trigger(&self, event_key: &str) -> Option<(String, String, String, Option<u64>, f64)> {
        let mf = self.model_manifest.as_ref()?;
        let trigger = mf.event_triggers.get(event_key)?;
        let expr_name = if trigger.expression.is_empty()
            || trigger.expression == "default"
            || trigger.expression == "idle"
        {
            String::new()
        } else {
            mf.resolve_expression_name(&trigger.expression)
                .map(|s| s.to_string())
                .unwrap_or_else(|| trigger.expression.clone())
        };
        let motion = if trigger.motion.is_empty() {
            DEFAULT_MOTION.to_string()
        } else {
            self.normalize_motion(&trigger.motion)
        };
        Some((expr_name, motion, trigger.action.clone(), trigger.duration_ms, trigger.probability))
    }

    /// 获取心情持续表情
    pub fn get_mood_idle_expression(&self, mood: &str) -> Option<(String, i32)> {
        let mf = self.model_manifest.as_ref()?;
        let config = mf.mood_idle_expressions.get(mood)?;
        let expr_name = if config.expression.is_empty() {
            String::new()
        } else {
            mf.resolve_expression_name(&config.expression)
                .map(|s| s.to_string())
                .unwrap_or_else(|| config.expression.clone())
        };
        Some((expr_name, config.priority))
    }

    /// 从心情表情池中随机选取一个表情（返回 注册表情名）
    ///
    /// 无 manifest、池为空、或语义名无法解析时返回 None。
    pub fn random_mood_expression(&self, mood: &str) -> Option<String> {
        let mf = self.model_manifest.as_ref()?;
        mf.random_mood_expression(mood)
    }

    /// 构建 prompt 上下文：告诉 LLM 当前可用的表情和动作
    pub fn build_prompt_context(&self) -> String {
        let mut parts = Vec::new();

        // 表情语义名列表（emotion 优先，action 随后）
        let mut emotion_names: Vec<String> = Vec::new();
        let mut action_names: Vec<String> = Vec::new();

        if let Some(ref mf) = self.model_manifest {
            for expr in &mf.expressions {
                match expr.category.as_str() {
                    "emotion" => emotion_names.push(expr.semantic.clone()),
                    "action" => action_names.push(expr.semantic.clone()),
                    _ => emotion_names.push(expr.semantic.clone()),
                }
            }
            let all_names: Vec<&str> = mf.prompt_semantic_names();
            if !all_names.is_empty() {
                parts.push(format!("Available expressions: {}", all_names.join(" / ")));
            }
        } else if !self.expressions.is_empty() {
            parts.push(format!("Available expressions: {}", self.expressions.join(" / ")));
        }

        if !self.motions.is_empty() {
            parts.push(format!("Available motions: {}", self.motions.join(" / ")));
        }

        if parts.is_empty() {
            return String::new();
        }

        // 动态构建语义说明
        let mut guidelines = String::new();
        guidelines.push_str("- Your reply TEXT is always the top priority. Think about what to say FIRST, then consider if an expression fits\n");
        guidelines.push_str("- Expressions are icing on the cake, not mandatory. Use one when it genuinely matches the mood or context; leave it empty if nothing fits — never force it\n");

        // emotion 类表情语义说明
        if !emotion_names.is_empty() {
            let desc = describe_emotion_semantics(&emotion_names);
            guidelines.push_str(&format!("- Emotion expressions convey your current feeling: {}\n", desc));
        }

        // action 类表情语义说明
        if !action_names.is_empty() {
            let desc = describe_action_semantics(&action_names);
            guidelines.push_str(&format!("- Action expressions only when the conversation context involves that specific activity: {}\n", desc));
        }

        guidelines.push_str("- Don't use an expression in every single reply — naturally about 60-80% of replies having an expression is ideal\n");
        guidelines.push_str("- ONLY use names from the list above; never invent expression or motion names");

        format!(
            "{}\n\n**Expression & Motion Guidelines**:\n{}",
            parts.join("\n"),
            guidelines
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest_with_model(mf: ModelManifest) -> ResourceManifest {
        let expressions: Vec<String> = mf.expressions.iter().map(|e| e.name.clone()).collect();
        let motions = mf.motions.clone();
        ResourceManifest {
            expressions,
            motions,
            model_manifest: Some(mf),
        }
    }

    fn make_manifest_without(expressions: &[&str], motions: &[&str]) -> ResourceManifest {
        ResourceManifest {
            expressions: expressions.iter().map(|s| s.to_string()).collect(),
            motions: motions.iter().map(|s| s.to_string()).collect(),
            model_manifest: None,
        }
    }

    fn vivian_manifest() -> ModelManifest {
        let json = r#"{
            "display_name": "Vivian",
            "model_file": "Vivian",
            "expressions": [
                { "semantic": "star_eyes", "name": "star_eyes", "label": "Star Eyes", "category": "emotion" },
                { "semantic": "shy", "name": "shy", "label": "Shy", "category": "emotion" },
                { "semantic": "angry", "name": "angry", "label": "Angry", "category": "emotion" },
                { "semantic": "cry", "name": "cry", "label": "Cry", "category": "emotion" }
            ],
            "aliases": {
                "happy": "star_eyes", "smile": "star_eyes",
                "angry": "angry",
                "cry": "cry",
                "shy": "shy"
            },
            "fallbacks": ["shy", "star_eyes", "angry", "cry"],
            "emotion_map": {
                "happy": "star_eyes", "excited": "star_eyes",
                "angry": "angry", "frustrated": "angry",
                "sad": "cry", "neutral": ""
            },
            "motions": ["idle"],
            "motion_aliases": { "idle": "idle" },
            "interaction_map": {
                "fast_click": { "expression": "shy", "motion": "idle" },
                "pet": { "expression": "shy", "motion": "idle" }
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_normalize_expression_direct_hit() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_expression("shy"), "shy");
        assert_eq!(m.normalize_expression("angry"), "angry");
    }

    #[test]
    fn test_normalize_expression_alias() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_expression("happy"), "star_eyes");
        assert_eq!(m.normalize_expression("smile"), "star_eyes");
        assert_eq!(m.normalize_expression("shy"), "shy");
    }

    #[test]
    fn test_normalize_expression_empty_defaults_to_neutral() {
        let m = make_manifest_with_model(vivian_manifest());
        // 空 / default / neutral 都视为"不指定"，返回空串由调用方处理
        assert_eq!(m.normalize_expression(""), "");
        assert_eq!(m.normalize_expression("default"), "");
        assert_eq!(m.normalize_expression("neutral"), "");
    }

    #[test]
    fn test_normalize_expression_fallback_chain() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_expression("confused"), "shy");
    }

    #[test]
    fn test_normalize_expression_no_resources() {
        let m = make_manifest_without(&[], &["idle"]);
        assert_eq!(m.normalize_expression("shy"), "");
    }

    #[test]
    fn test_normalize_expression_manifest_no_match_returns_empty() {
        // manifest 路径：别名/原名/fallbacks 全部未命中 → 空串（不强制使用）
        let json = r#"{
            "display_name": "Test",
            "model_file": "test",
            "expressions": [
                { "semantic": "shy", "name": "shy", "label": "Shy", "category": "emotion" }
            ],
            "aliases": {},
            "fallbacks": [],
            "emotion_map": {},
            "motions": ["idle"],
            "motion_aliases": {},
            "interaction_map": {}
        }"#;
        let mf: ModelManifest = serde_json::from_str(json).unwrap();
        let m = make_manifest_with_model(mf);
        assert_eq!(m.normalize_expression("nonexistent"), "");
    }

    #[test]
    fn test_normalize_expression_no_manifest_no_match_returns_empty() {
        // 无 manifest 路径：原名不存在 → 空串（不强制使用）
        let m = make_manifest_without(&["shy", "angry"], &["idle"]);
        assert_eq!(m.normalize_expression("nonexistent"), "");
    }

    #[test]
    fn test_normalize_motion_direct_hit() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_motion("idle"), "idle");
        assert_eq!(m.normalize_motion("nonexistent_motion"), "idle");
    }

    #[test]
    fn test_normalize_motion_fallback_to_idle() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_motion("nonexistent"), "idle");
    }

    #[test]
    fn test_normalize_motion_empty() {
        let m = make_manifest_with_model(vivian_manifest());
        assert_eq!(m.normalize_motion(""), "idle");
    }

    #[test]
    fn test_build_prompt_context() {
        let m = make_manifest_with_model(vivian_manifest());
        let ctx = m.build_prompt_context();
        assert!(ctx.contains("shy"));
        assert!(ctx.contains("angry"));
        assert!(ctx.contains("idle"));
        assert!(ctx.contains("Available expressions"));
        assert!(ctx.contains("TEXT is always the top priority"));
        assert!(ctx.contains("never invent"));
    }

    #[test]
    fn test_build_prompt_context_empty() {
        let m = make_manifest_without(&[], &[]);
        assert_eq!(m.build_prompt_context(), "");
    }

    #[test]
    fn test_emotion_to_expression() {
        let m = make_manifest_with_model(vivian_manifest());
        let mf = m.model_manifest().unwrap();
        assert_eq!(mf.emotion_to_expression("happy"), "star_eyes");
        assert_eq!(mf.emotion_to_expression("excited"), "star_eyes");
        assert_eq!(mf.emotion_to_expression("angry"), "angry");
        assert_eq!(mf.emotion_to_expression("sad"), "cry");
    }

    #[test]
    fn test_interaction_feedback() {
        let m = make_manifest_with_model(vivian_manifest());
        let mf = m.model_manifest().unwrap();
        let fb = mf.interaction_feedback("pet").unwrap();
        assert_eq!(fb.expression, "shy");
        assert_eq!(fb.motion, "idle");
    }

    #[test]
    fn test_semantic_to_name_mapping() {
        // Custom model mapping: semantic "shy" → name "expression2"
        let json = r#"{
            "display_name": "TestModel",
            "model_file": "test",
            "expressions": [
                { "semantic": "star_eyes", "name": "expression1", "label": "Star Eyes", "category": "emotion" },
                { "semantic": "shy", "name": "expression2", "label": "Shy", "category": "emotion" },
                { "semantic": "angry", "name": "expression19", "label": "Angry", "category": "emotion" }
            ],
            "aliases": { "shy": "shy", "angry": "angry", "happy": "star_eyes" },
            "fallbacks": ["shy", "star_eyes", "angry"],
            "emotion_map": { "happy": "star_eyes", "angry": "angry" },
            "motions": ["idle"],
            "motion_aliases": { "idle": "idle" },
            "interaction_map": {}
        }"#;
        let mf: ModelManifest = serde_json::from_str(json).unwrap();
        let m = make_manifest_with_model(mf);

        assert_eq!(m.normalize_expression("shy"), "expression2");
        assert_eq!(m.normalize_expression("happy"), "expression1");
        assert_eq!(m.normalize_expression("angry"), "expression19");
    }
}
