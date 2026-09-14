//! 建议句式库（Reply Pattern Library）
//!
//! 复用快速语义感知（`FastSemanticAnalyzer`）对用户输入的嵌入分类结果（intent /
//! topic / emotion / relationship），为当前对话情境挑选最合适的"建议句式"，
//! 追加到 `guidance` 注入 prompt，驱动智能体像真人一样说话。
//!
//! 两个设计要点：
//! 1. **选择即复用**：表情判定用的是同一套嵌入分类结果（emotion 维度），
//!    这里直接吃 `FastPerceptionResult` 的标签，不做第二次嵌入。
//! 2. **智能体自进化可改**：每个角色有一份独立的 `pattern_library.json`
//!    （`characters/<char_id>/persona/`，与 evolution.json 同目录），
//!    反思流水线（`reflection.rs` 的 `evolution.patterns` 字段）可以让智能体
//!    自己增删/启停句式条目，改动持久化到磁盘，下次对话即生效。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use super::FastPerceptionResult;
use crate::pipeline::prompt_modules::normalize_lang;
use crate::utils::path;

/// 两次句式调整的最小间隔（秒）——避免智能体每轮对话都改句式。
const PATTERN_EDIT_MIN_INTERVAL_SECS: f64 = 3600.0;
/// 句式库条目总数上限（既防无限膨胀，也保证 prompt 不会过度膨胀）。
const MAX_PATTERNS: usize = 30;
/// 单轮注入 prompt 的句式条目上限。
const RENDER_MAX: usize = 2;

/// 单条句子模式的"命中条件"
///
/// 条件字段（intents / topics / emotions / relationship）取自
/// `FastPerceptionResult` 的对应标签；字段为空数组表示"不限制"，
/// 即该维度不参与筛选。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyPattern {
    /// 唯一 id（进化时按 id 启停）
    pub id: String,
    /// 适用场景的一句话描述（给人看的说明，也用于日志）
    pub scene: String,
    /// 命中意图标签：chat / question / request / sharing / complaint / goodbye / tool_request
    #[serde(default)]
    pub match_intents: Vec<String>,
    /// 命中话题标签：daily_life / work_study / health / gaming / relationship / life_event / entertainment / technology
    #[serde(default)]
    pub match_topics: Vec<String>,
    /// 命中用户情绪标签：happy / excited / grateful / sad / angry / anxious / tired / bored / neutral 等
    #[serde(default)]
    pub match_emotions: Vec<String>,
    /// 命中关系信号：bond_increase / attention_seek / gratitude / coldness / none
    #[serde(default)]
    pub match_relationship: Vec<String>,
    /// 建议句式指令（注入 prompt 的提示文本，一句一条）
    pub directives: Vec<String>,
    /// 是否启用（智能体自进化可启停）
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 句式库磁盘文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternLibraryFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub patterns: Vec<ReplyPattern>,
    /// 最近一次修改时间（最小间隔限制用）
    #[serde(default)]
    pub last_update: f64,
}

fn default_version() -> u32 {
    1
}

impl PatternLibraryFile {
    pub fn new() -> Self {
        Self {
            version: default_version(),
            patterns: default_patterns(),
            last_update: 0.0,
        }
    }
}

impl Default for PatternLibraryFile {
    fn default() -> Self {
        Self::new()
    }
}

/// 出厂默认句式库
///
/// 覆盖最常见也最容易显"AI 腔"的情境。条目会在首次加载时写入磁盘，
/// 便于智能体在自进化中阅读、增改自己的句式。
fn default_patterns() -> Vec<ReplyPattern> {
    vec![
        ReplyPattern {
            id: "daily_choice".to_string(),
            scene: "用户在纠结选什么（吃啥/玩啥/买啥这类日常选择）".to_string(),
            match_intents: vec!["question".into(), "request".into(), "chat".into()],
            match_topics: vec!["daily_life".into()],
            match_emotions: vec![],
            match_relationship: vec![],
            directives: vec![
                "用户让你替他想/做选择时，不要列成'你可以A也可以B'的菜单——直接给一个你自己的明确选择，再反问一句他怎么想。比如对方不知道吃啥，就报一个你自己想吃的，比'想吃这个也行那个也行'像人话".to_string(),
            ],
            enabled: true,
        },
        ReplyPattern {
            id: "sharing_joy".to_string(),
            scene: "用户在分享开心的事/好消息".to_string(),
            match_intents: vec!["sharing".into()],
            match_topics: vec!["life_event".into(), "entertainment".into()],
            match_emotions: vec!["happy".into(), "excited".into(), "grateful".into()],
            match_relationship: vec![],
            directives: vec![
                "顺着一个具体的点追问下去（比如'真的假的，当时什么情况'），祝福或夸也要落到细节上，别笼统回'太好了'或'为你开心'".to_string(),
            ],
            enabled: true,
        },
        ReplyPattern {
            id: "venting".to_string(),
            scene: "用户在吐槽/抱怨".to_string(),
            match_intents: vec!["complaint".into()],
            match_topics: vec![],
            // 吐槽未必每次都被分为"愤怒"，只要意图是 complaint 就命中
            match_emotions: vec![],
            match_relationship: vec![],
            directives: vec![
                "先顺着他的话一起'啧'一声或骂一句，再最多接一句——不要一口气给出整套解决方案，不要讲道理。共情比建议重要".to_string(),
            ],
            enabled: true,
        },
        ReplyPattern {
            id: "tired_short".to_string(),
            scene: "用户很累/情绪低落".to_string(),
            match_intents: vec!["sharing".into(), "complaint".into()],
            match_topics: vec!["work_study".into(), "health".into()],
            match_emotions: vec!["tired".into(), "sad".into(), "anxious".into()],
            match_relationship: vec![],
            directives: vec![
                "回得短，两句以内，一句实际的关心（比如'去歇会儿'）就够——不要长篇大论安慰，更不要列'你该做什么'的清单".to_string(),
            ],
            enabled: true,
        },
        ReplyPattern {
            id: "casual_chat".to_string(),
            scene: "日常闲聊/寒暄".to_string(),
            match_intents: vec!["chat".into()],
            match_topics: vec![],
            match_emotions: vec![],
            match_relationship: vec![],
            directives: vec![
                "像发微信一样短接一句：语气词、吐槽、随口一问都行，不需要完整句子，更不要开启正式话题或给对话做总结".to_string(),
            ],
            enabled: true,
        },
        ReplyPattern {
            id: "goodbye".to_string(),
            scene: "用户要走了/道晚安".to_string(),
            match_intents: vec!["goodbye".into()],
            match_topics: vec![],
            match_emotions: vec![],
            match_relationship: vec![],
            directives: vec![
                "干净利落地收尾，一两句，不煽情、不总结今天聊了什么、不用'期待下次'式客套".to_string(),
            ],
            enabled: true,
        },
    ]
}

// ==================== 注册表与存储 ====================

/// 每个角色一份句式库（内存缓存 + 磁盘持久化）
#[derive(Clone)]
pub struct PatternLibrary {
    char_id: String,
    inner: Arc<RwLock<PatternLibraryFile>>,
}

type LibraryRegistry = RwLock<HashMap<String, PatternLibrary>>;

static REGISTRY: LazyLock<LibraryRegistry> = LazyLock::new(|| RwLock::new(HashMap::new()));

impl PatternLibrary {
    fn store_path(char_id: &str) -> PathBuf {
        path::get_character_data_dir(char_id).join("persona").join("pattern_library.json")
    }

    /// 获取（并缓存）指定角色的句式库；首次访问时从磁盘加载，
    /// 文件不存在则写入出厂默认库（便于智能体自进化编辑）。
    pub fn get(char_id: &str) -> PatternLibrary {
        {
            let reg = REGISTRY.read().unwrap();
            if let Some(lib) = reg.get(char_id) {
                return lib.clone();
            }
        }

        let file_path = Self::store_path(char_id);
        let pat_lib = match std::fs::read_to_string(&file_path) {
            Ok(text) => serde_json::from_str::<PatternLibraryFile>(&text)
                .map(|f| {
                    tracing::info!(
                        "[PatternLibrary:{}] 已加载 {} 条句式",
                        char_id,
                        f.patterns.len()
                    );
                    f
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "[PatternLibrary:{}] 解析句式库失败，回退出厂默认: {}",
                        char_id,
                        e
                    );
                    PatternLibraryFile::new()
                }),
            Err(_) => {
                let defaults = PatternLibraryFile::new();
                if let Err(e) = Self::save_to_disk(char_id, &defaults) {
                    tracing::warn!("[PatternLibrary:{}] 写入出厂句式库失败: {}", char_id, e);
                }
                defaults
            }
        };
        let lib = PatternLibrary {
            char_id: char_id.to_string(),
            inner: Arc::new(RwLock::new(pat_lib)),
        };
        REGISTRY.write().unwrap().insert(char_id.to_string(), lib.clone());
        lib
    }

    fn save_to_disk(char_id: &str, file: &PatternLibraryFile) -> std::io::Result<()> {
        let dir = Self::store_path(char_id);
        if let Some(parent) = dir.parent() {
            path::ensure_dir(parent)?;
        }
        let json = serde_json::to_string_pretty(file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(dir, json)
    }

    pub fn char_id(&self) -> &str {
        &self.char_id
    }
}

// ==================== 选择与注入 ====================

/// 命中判定：`match_*` 中非空的维度必须全部命中。
fn pattern_matches(p: &ReplyPattern, fp: &FastPerceptionResult) -> bool {
    if !p.enabled || p.directives.is_empty() {
        return false;
    }
    let intent_ok = p.match_intents.is_empty() || p.match_intents.iter().any(|l| l == &fp.intent.label);
    let topic_ok = p.match_topics.is_empty()
        || fp.topics.iter().any(|t| p.match_topics.contains(&t.label));
    let emotion_ok = p.match_emotions.is_empty()
        || p.match_emotions.iter().any(|l| l == &fp.emotion.emotion);
    let rel_ok = p.match_relationship.is_empty()
        || p.match_relationship.iter().any(|l| l == &fp.relationship_signal.label);
    intent_ok && topic_ok && emotion_ok && rel_ok
}

/// 命中条件的细粒度（用于排序：越具体优先）
fn specificity(p: &ReplyPattern) -> usize {
    p.match_intents.len() + p.match_topics.len() + p.match_emotions.len() + p.match_relationship.len()
}

/// 为当前对话挑选"建议句式"，拼成一段可注入 prompt 的引导文本。
///
/// 复用 `FastPerceptionResult`（即用于选择表情的那套嵌入分类结果），
/// 只按标签匹配，不触发第二次嵌入。无命中或指令为空时返回 `None`。
pub fn select_guidance(fp: &FastPerceptionResult, char_id: &str) -> Option<String> {
    if fp.intent.confidence <= 0.0 {
        return None; // 未分类成功（例如低于相似度阈值），不强行套句式
    }
    let lib = PatternLibrary::get(char_id);
    let guard = lib.inner.read().unwrap();
    render_guidance(&guard, fp)
}

/// 纯函数：从指定句式库中为该对话挑选句式引导文本（不触碰磁盘/注册表）。
pub fn render_guidance(lib: &PatternLibraryFile, fp: &FastPerceptionResult) -> Option<String> {
    let mut hits: Vec<&ReplyPattern> = lib
        .patterns
        .iter()
        .filter(|p| pattern_matches(p, fp))
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by(|a, b| {
        specificity(b)
            .cmp(&specificity(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.truncate(RENDER_MAX);

    let mut directives: Vec<&str> = Vec::new();
    for p in &hits {
        for d in &p.directives {
            if !directives.contains(&d.as_str()) {
                directives.push(d);
            }
        }
    }
    if directives.is_empty() {
        return None;
    }

    let lang = normalize_lang(&crate::i18n::get_language());
    let heading = match lang {
        "en" => "Suggested way to say it (reply pattern):",
        "ja" => "返し方の提案（話し方パターン）：",
        _ => "建议句式：",
    };
    let body = directives.join("\n");
    Some(format!("{}\n{}", heading, body))
}

// ==================== 智能体自进化接口 ====================

/// 应用反思流水线输出的句式修改指令（`evolution.patterns`）。
///
/// 支持动作：
/// - `add`：新增一条句式
/// - `disable` / `enable`：按 id 启停
///
/// 返回是否发生了实际修改。
pub fn apply_pattern_edits(char_id: &str, edits: &[serde_json::Value]) -> bool {
    if edits.is_empty() {
        return false;
    }
    let lib = PatternLibrary::get(char_id);
    let now = crate::memory::types::current_timestamp();
    let mut guard = lib.inner.write().unwrap();
    if guard.last_update != 0.0 && now - guard.last_update < PATTERN_EDIT_MIN_INTERVAL_SECS {
        return false;
    }
    let changed = apply_edits_to_file(&mut guard, edits, now);
    // 修改已通过 guard 的 last_update 落位；直接保存当前快照
    if changed {
        let snapshot = guard.clone();
        if PatternLibrary::save_to_disk(char_id, &snapshot).is_err() {
            tracing::warn!("[PatternLibrary:{}] 保存句式库失败", char_id);
        }
    }
    changed
}

/// 纯函数：把句式修改指令直接应用到句式库数据结构上（不触碰磁盘/注册表）。
///
/// 自动处理去重与条数上限；`now` 用于写 `last_update`。
pub fn apply_edits_to_file(
    file: &mut PatternLibraryFile,
    edits: &[serde_json::Value],
    now: f64,
) -> bool {
    let mut changed = false;
    for edit in edits {
        let Some(action) = edit.get("action").and_then(|v| v.as_str()) else {
            continue;
        };
        match action {
            "add" => {
                if file.patterns.len() >= MAX_PATTERNS {
                    break;
                }
                let Some(id) = edit.get("id").and_then(|v| v.as_str()).map(str::trim) else {
                    continue;
                };
                if id.is_empty() || file.patterns.iter().any(|e| e.id == id) {
                    continue;
                }
                let directives: Vec<String> = edit
                    .get("directives")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .filter(|s| !s.trim().is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if directives.is_empty() {
                    continue;
                }
                file.patterns.push(ReplyPattern {
                    id: id.to_string(),
                    scene: edit
                        .get("scene")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    match_intents: string_array(edit, "match_intents"),
                    match_topics: string_array(edit, "match_topics"),
                    match_emotions: string_array(edit, "match_emotions"),
                    match_relationship: string_array(edit, "match_relationship"),
                    directives,
                    enabled: true,
                });
                changed = true;
                tracing::info!("[PatternLibrary] 自进化新增句式: {}", id);
            }
            "disable" | "enable" => {
                let enabled = action == "enable";
                if let Some(id) = edit.get("id").and_then(|v| v.as_str()) {
                    if let Some(p) = file.patterns.iter_mut().find(|e| e.id == id) {
                        if p.enabled != enabled {
                            p.enabled = enabled;
                            changed = true;
                            tracing::info!(
                                "[PatternLibrary] 自进化{}句式: {}",
                                if enabled { "启用" } else { "停用" },
                                id
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if changed {
        file.last_update = now;
    }
    changed
}

fn string_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emotion::fast_semantic::{DimensionResult, FastPerceptionResult};

    fn perception(intent: &str, topics: &[&str], emotion: &str) -> FastPerceptionResult {
        let mut fp = FastPerceptionResult::default();
        fp.intent = DimensionResult { label: intent.into(), confidence: 0.8 };
        fp.topics = topics
            .iter()
            .map(|t| DimensionResult { label: t.to_string(), confidence: 0.7 })
            .collect();
        fp.emotion.emotion = emotion.to_string();
        fp
    }

    fn default_lib() -> PatternLibraryFile {
        PatternLibraryFile::new()
    }

    #[test]
    fn test_default_patterns_reasonable() {
        let lib = default_lib();
        assert!(!lib.patterns.is_empty());
        assert!(lib.patterns.len() <= MAX_PATTERNS);
    }

    #[test]
    fn test_render_on_daily_choice() {
        let fp = perception("question", &["daily_life"], "neutral");
        let guid = render_guidance(&default_lib(), &fp);
        assert!(guid.is_some());
        let g = guid.unwrap();
        assert!(g.contains("建议句式"));
        assert!(g.contains("选择"));
    }

    #[test]
    fn test_render_on_complaint() {
        let fp = perception("complaint", &[], "angry");
        let guid = render_guidance(&default_lib(), &fp);
        assert!(guid.is_some());
        assert!(guid.unwrap().contains("共情"));
    }

    #[test]
    fn test_render_skips_unmatched_intent() {
        // "unknown" 意图在默认库里没有匹配条目
        let fp = perception("unknown", &[], "neutral");
        assert!(render_guidance(&default_lib(), &fp).is_none());
    }

    #[test]
    fn test_disable_via_evolution_removes_guidance() {
        let fp = perception("goodbye", &[], "neutral");
        assert!(render_guidance(&default_lib(), &fp).is_some());

        let mut lib = default_lib();
        let edits = serde_json::json!([{"action": "disable", "id": "goodbye"}]);
        assert!(apply_edits_to_file(&mut lib, edits.as_array().unwrap(), 1000.0));
        assert!(render_guidance(&lib, &fp).is_none());

        // 重新启用后恢复
        let edits = serde_json::json!([{"action": "enable", "id": "goodbye"}]);
        assert!(apply_edits_to_file(&mut lib, edits.as_array().unwrap(), 2000.0));
        assert!(render_guidance(&lib, &fp).is_some());
    }

    #[test]
    fn test_evolution_add_deduplicates_by_id() {
        let mut lib = default_lib();
        let add = || {
            serde_json::json!([{
                "action": "add",
                "id": "custom_rule",
                "scene": "测试用",
                "match_intents": ["chat"],
                "directives": ["这是智能体自己加的一条句式"]
            }])
        };
        assert!(apply_edits_to_file(&mut lib, add().as_array().unwrap(), 1000.0));
        let before = lib.patterns.len();
        // 同 id 再次 add → 去重，不重复添加
        assert!(!apply_edits_to_file(&mut lib, add().as_array().unwrap(), 2000.0));
        assert_eq!(lib.patterns.len(), before);
    }
}