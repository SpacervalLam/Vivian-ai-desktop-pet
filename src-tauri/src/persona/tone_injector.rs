//! 场景语气注入器（Tone Injector）
//!
//! 基于 embedding + 关键词匹配用户输入的场景，命中后注入对应场景的语气规则 + 参考台词。
//! 不依赖 LLM 主动调用，是硬约束——每轮对话都会执行。
//!
//! ## 匹配策略
//!
//! 1. **关键词匹配**（primary）：场景样本作为关键词集合，检查用户输入是否包含任何关键词
//!    - 快速、确定性、不依赖 embedding 服务质量
//!    - 适用于默认哈希嵌入场景
//! 2. **embedding 匹配**（secondary）：仅当配置了远程 embedding 服务时启用
//!    - 语义相似度匹配，能识别关键词未覆盖的同义表达
//!    - 命中阈值 0.72
//!
//! ## 注入位置
//!
//! 动态区末尾（工具列表前），利用近因效应强化语气控制。

use std::sync::Arc;

use parking_lot::RwLock;

use crate::memory::embedding::{default_embedding, MemoryEmbeddingProvider};
use crate::memory::vector_search::cosine_similarity;

/// 场景命中阈值：cosine similarity >= 此值才触发 embedding 匹配注入
const SCENE_MATCH_THRESHOLD: f64 = 0.72;

/// 上下文窗口：匹配时考虑最近 N 轮对话（含用户输入）
const CONTEXT_WINDOW_TURNS: usize = 3;

/// 场景条目
#[derive(Debug, Clone)]
struct SceneEntry {
    /// 场景标识（如 "greeting" / "comfort"）
    id: String,
    /// 匹配样本（用于关键词匹配和 embedding 匹配的典型用户输入）
    samples: Vec<String>,
    /// 参考台词（命中后注入 prompt）
    quotes: Vec<String>,
    /// 预计算的样本 embedding（仅远程 embedding 时填充）
    sample_embeddings: Vec<Vec<f32>>,
}

/// 场景语气注入器
///
/// 每个角色一个实例，惰性初始化 embedding 缓存。
pub struct ToneInjector {
    /// 角色 ID
    char_id: String,
    /// 场景列表（按 scenes.md 顺序）
    scenes: RwLock<Vec<SceneEntry>>,
    /// embedding 服务
    embedding: Arc<dyn MemoryEmbeddingProvider>,
    /// 是否已初始化 embedding
    initialized: RwLock<bool>,
}

impl ToneInjector {
    /// 创建 ToneInjector（使用默认哈希嵌入）
    pub fn new(char_id: &str) -> Self {
        Self::with_embedding(char_id, default_embedding())
    }

    /// 创建 ToneInjector（指定 embedding 服务）
    pub fn with_embedding(char_id: &str, embedding: Arc<dyn MemoryEmbeddingProvider>) -> Self {
        let scenes_md = load_scenes_md(char_id);
        let scenes = parse_scenes_md(scenes_md);

        tracing::info!(
            "[ToneInjector] 已加载角色 {} 的场景库：{} 个场景",
            char_id,
            scenes.len()
        );

        Self {
            char_id: char_id.to_string(),
            scenes: RwLock::new(scenes),
            embedding,
            initialized: RwLock::new(false),
        }
    }

    /// 惰性初始化：预计算所有场景样本的 embedding（仅远程 embedding 时有意义）
    fn ensure_initialized(&self) {
        let mut initialized = self.initialized.write();
        if *initialized {
            return;
        }

        // 只有远程 embedding 才预计算（哈希嵌入每次计算很快，无需缓存）
        if self.embedding.is_remote() {
            let mut scenes = self.scenes.write();
            for scene in scenes.iter_mut() {
                scene.sample_embeddings = scene
                    .samples
                    .iter()
                    .filter_map(|s| self.embedding.embed(s).ok())
                    .collect();
            }
            tracing::info!(
                "[ToneInjector] 角色 {} 已预计算 {} 个场景的 embedding",
                self.char_id,
                scenes.len()
            );
        }

        *initialized = true;
    }

    /// 匹配场景并构建注入文本
    ///
    /// - `user_input`：当前用户输入
    /// - `recent_messages`：最近几轮对话消息（user + assistant），用于上下文感知
    /// - `lang`：界面语言（zh/en/ja），用于三语化标题
    ///
    /// 返回注入文本（None 表示无命中）
    pub fn build_tone_injection(
        &self,
        user_input: &str,
        recent_messages: &[String],
        lang: &str,
    ) -> Option<String> {
        if user_input.trim().is_empty() {
            return None;
        }

        self.ensure_initialized();

        // 构造匹配文本：用户输入 + 最近 N 轮上下文
        let match_text = build_match_text(user_input, recent_messages);
        let scenes = self.scenes.read();

        // 1. 关键词匹配（primary）
        for scene in scenes.iter() {
            for sample in &scene.samples {
                if sample.is_empty() {
                    continue;
                }
                if match_text.contains(sample.as_str()) {
                    return Some(format_injection(scene, 1.0, "keyword", lang));
                }
            }
        }

        // 2. embedding 匹配（secondary，仅远程 embedding 时启用）
        if self.embedding.is_remote() {
            if let Ok(query_emb) = self.embedding.embed(&match_text) {
                let mut best: Option<(&SceneEntry, f64)> = None;
                for scene in scenes.iter() {
                    for sample_emb in &scene.sample_embeddings {
                        let sim = cosine_similarity(&query_emb, sample_emb);
                        if best.is_none() || sim > best.unwrap().1 {
                            best = Some((scene, sim));
                        }
                    }
                }
                if let Some((scene, score)) = best {
                    if score >= SCENE_MATCH_THRESHOLD {
                        return Some(format_injection(scene, score, "embedding", lang));
                    }
                }
            }
        }

        None
    }

    /// 获取当前角色 ID
    pub fn char_id(&self) -> &str {
        &self.char_id
    }
}

/// 构造匹配文本：用户输入 + 最近 N 轮上下文
fn build_match_text(user_input: &str, recent_messages: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let ctx_start = recent_messages.len().saturating_sub(CONTEXT_WINDOW_TURNS);
    for msg in &recent_messages[ctx_start..] {
        if !msg.trim().is_empty() {
            parts.push(msg.as_str());
        }
    }
    parts.push(user_input);
    parts.join(" ")
}

/// 格式化注入文本
fn format_injection(scene: &SceneEntry, score: f64, match_type: &str, lang: &str) -> String {
    let quotes_text = scene
        .quotes
        .iter()
        .map(|q| format!("- {}", q))
        .collect::<Vec<_>>()
        .join("\n");

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
    let header = crate::pipeline::prompt_modules::section_heading("scene_tone", lang);
    let (match_label, sim_label, intro) = match lang_norm {
        "en" => ("match", "similarity",
            "Here are things you'd say in this scene. Internalize the rhythm and tone — don't repeat verbatim:"),
        "ja" => ("マッチ", "類似度",
            "このシーンであなたが言いそうなこと。リズムとトーンを内面化し、原文をそのまま繰り返さない："),
        _ => ("命中", "相似度",
            "以下是你在该场景下会说的话，内化语气节奏，不要直接复述原文："),
    };

    format!(
        "{}（{}: {} | {} | {}: {:.2}）\n{}\n{}",
        header, match_label, scene.id, match_type, sim_label, score, intro, quotes_text
    )
}

/// 加载角色的 scenes.md
fn load_scenes_md(char_id: &str) -> &'static str {
    match char_id {
        "nana" => include_str!("../../prompts/characters/nana/scenes.md"),
        _ => include_str!("../../prompts/characters/vivian/scenes.md"),
    }
}

/// 解析 scenes.md 为场景列表
///
/// 格式：
/// ```text
/// ## [scene_id]
/// ### 匹配样本
/// 样本1 样本2 样本3 ...
/// ### 参考台词
/// - 台词1
/// - 台词2
/// ```
fn parse_scenes_md(md: &str) -> Vec<SceneEntry> {
    let mut scenes: Vec<SceneEntry> = Vec::new();
    let mut current_scene: Option<SceneEntry> = None;
    let mut current_section = "";

    for line in md.lines() {
        let line = line.trim();

        // 场景头：## [scene_id]
        if line.starts_with("## [") && line.ends_with(']') {
            if let Some(scene) = current_scene.take() {
                scenes.push(scene);
            }
            let id = line
                .trim_start_matches("## [")
                .trim_end_matches(']')
                .to_string();
            current_scene = Some(SceneEntry {
                id,
                samples: Vec::new(),
                quotes: Vec::new(),
                sample_embeddings: Vec::new(),
            });
            current_section = "";
        } else if line.starts_with("### 匹配样本") {
            current_section = "samples";
        } else if line.starts_with("### 参考台词") {
            current_section = "quotes";
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(scene) = current_scene.as_mut() {
                match current_section {
                    "samples" => {
                        // 样本按空格分词（每个词都是独立的匹配关键词）
                        for word in line.split_whitespace() {
                            if !word.is_empty() {
                                scene.samples.push(word.to_string());
                            }
                        }
                    }
                    "quotes" => {
                        // 台词以 "- " 开头
                        if let Some(quote) = line.strip_prefix("- ") {
                            scene.quotes.push(quote.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(scene) = current_scene {
        scenes.push(scene);
    }

    scenes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vivian_scenes() {
        let md = load_scenes_md("vivian");
        let scenes = parse_scenes_md(md);
        assert!(scenes.len() >= 9, "应至少解析出 9 个场景，实际: {}", scenes.len());
        assert!(scenes.iter().any(|s| s.id == "greeting"));
        assert!(scenes.iter().any(|s| s.id == "comfort"));
        assert!(scenes.iter().any(|s| s.id == "daily"));
    }

    #[test]
    fn parse_nana_scenes() {
        let md = load_scenes_md("nana");
        let scenes = parse_scenes_md(md);
        assert!(scenes.len() >= 9, "应至少解析出 9 个场景，实际: {}", scenes.len());
    }

    #[test]
    fn vivian_scenes_have_quotes() {
        let md = load_scenes_md("vivian");
        let scenes = parse_scenes_md(md);
        for scene in &scenes {
            assert!(
                !scene.quotes.is_empty(),
                "场景 {} 的参考台词为空",
                scene.id
            );
            assert!(
                !scene.samples.is_empty(),
                "场景 {} 的匹配样本为空",
                scene.id
            );
        }
    }

    #[test]
    fn keyword_match_greeting() {
        let injector = ToneInjector::new("vivian");
        let result = injector.build_tone_injection("早安", &[], "zh");
        assert!(result.is_some(), "早安应通过关键词命中 greeting 场景");
        let text = result.unwrap();
        assert!(text.contains("greeting"));
        assert!(text.contains("keyword"));
    }

    #[test]
    fn keyword_match_comfort() {
        let injector = ToneInjector::new("vivian");
        let result = injector.build_tone_injection("今天好累啊", &[], "zh");
        assert!(result.is_some(), "好累应通过关键词命中 comfort 或 tired 场景");
    }

    #[test]
    fn no_match_for_empty_input() {
        let injector = ToneInjector::new("vivian");
        let result = injector.build_tone_injection("", &[], "zh");
        assert!(result.is_none(), "空输入不应命中任何场景");
    }

    #[test]
    fn context_aware_matching() {
        let injector = ToneInjector::new("vivian");
        // 上下文中包含关键词也应触发匹配
        let recent = vec!["我回来了".to_string()];
        let result = injector.build_tone_injection("你在吗", &recent, "zh");
        assert!(result.is_some(), "上下文中的关键词应触发匹配");
    }

    #[test]
    fn nana_tone_injector_works() {
        let injector = ToneInjector::new("nana");
        let result = injector.build_tone_injection("晚安", &[], "zh");
        assert!(result.is_some(), "晚安应命中 farewell 场景");
    }
}
