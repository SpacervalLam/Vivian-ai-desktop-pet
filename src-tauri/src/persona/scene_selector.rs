//! 场景模式选择器 — 多信号融合决策
//!
//! 数据流：
//!   EmotionAnalyzer / DialogueManager / MemoryManager
//!             ↓
//!     SceneModeSelector.select(user_input, context, emotion)
//!             ↓
//!        SceneMode (enum)
//!             ↓
//!     prompt_render::render_style_block(mode)
//!
//! 5 信号融合：关键词 / 时间 / 情感 / 关系状态 / 对话历史模式
//! + 模式稳定性窗口（防频繁切换）

use std::collections::HashMap;

use chrono::Local;

use crate::memory::embedding::MemoryEmbeddingProvider;

use super::schemas::{SceneMode, SceneModeConfig, DEFAULT_SCENE_MODES};

/// 场景选择上下文
#[derive(Debug, Clone, Default)]
pub struct SceneContext {
    /// 当前时间（小时 0-23），None 时使用 Local::now()
    pub hour: Option<u32>,
    /// 亲密度（0-100），默认 50
    pub intimacy: Option<f64>,
    /// 精力值（0-100），默认 50
    pub energy: Option<f64>,
    /// 当前活跃应用（保留字段，暂未使用）
    pub active_app: Option<String>,
    /// 心理学系统主导情绪标签（如 "sad"/"joyful"/"curious"），供场景选择器参考
    pub dominant_emotion: Option<String>,
    /// 心理学系统需求压力（0.0-1.0），高值时偏向 Cozy/Guardian 场景
    pub need_pressure: Option<f64>,
}

impl SceneContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 情绪关键词映射 {场景: [关键词列表]}（7 场景）
fn emotion_keywords() -> Vec<(SceneMode, &'static [&'static str])> {
    vec![
        (
            SceneMode::Comforting,
            &[
                "难过", "伤心", "不开心", "难受", "痛苦", "失落",
                "抑郁", "沮丧", "sad", "depressed", "heartbroken",
                "累了", "好累", "疲惫", "累死了", "压力",
            ][..],
        ),
        (
            SceneMode::Guardian,
            &[
                "焦虑", "害怕", "担心", "紧张", "不安",
                "anxious", "scared", "worried", "fear",
                "深夜", "失眠", "睡不着",
            ][..],
        ),
        (
            SceneMode::Banter,
            &[
                "哈哈", "好笑", "lol", "lmao", "好玩", "有趣",
                "开心", "高兴", "逗", "笑死",
            ][..],
        ),
        (
            SceneMode::Morning,
            &[
                "早安", "早上好", "起床", "新的一天",
                "good morning", "wake up", "睡醒了",
                "早", "今天的计划",
            ][..],
        ),
        (
            SceneMode::Energetic,
            &[
                "加油", "冲", "动力", "元气", "干劲",
                "let's go", "fighting",
            ][..],
        ),
        (
            SceneMode::Cozy,
            &[
                "陪我", "无聊", "想你", "好想你", "在干嘛",
                "miss you", "lonely",
            ][..],
        ),
        (
            SceneMode::Companion,
            &[
                "工作中", "在忙", "别打扰", "不要吵",
                "working", "busy", "focus",
            ][..],
        ),
    ]
}

/// 时段规则 (start_hour, end_hour, mode)
/// 22-5 跨午夜，使用特殊判断
fn time_rules() -> Vec<(u32, u32, SceneMode)> {
    vec![
        (6, 10, SceneMode::Morning),   // 06:00-10:00 晨间
        (22, 5, SceneMode::Guardian),  // 22:00-05:00 守护（深夜）
    ]
}

/// 基于多信号融合的场景决策（关键词/时间/亲密度/历史连续性/embedding 语义匹配）
pub struct SceneModeSelector {
    scene_modes: HashMap<SceneMode, SceneModeConfig>,
    /// 多少轮对话内保持模式稳定
    mode_stability_window: usize,
    current_mode: SceneMode,
    /// 最近的模式记录
    mode_history: Vec<SceneMode>,
    /// 场景 embedding 索引（预计算的关键词集合向量，用于语义匹配）
    scene_embeddings: once_cell::sync::OnceCell<Vec<(SceneMode, Vec<f32>)>>,
    /// 哈希嵌入服务（零依赖，离线可用）
    embedder: crate::memory::embedding::HashingMemoryEmbedding,
}

impl SceneModeSelector {
    pub fn new() -> Self {
        Self {
            scene_modes: DEFAULT_SCENE_MODES.clone(),
            mode_stability_window: 3,
            current_mode: SceneMode::DailyChat,
            mode_history: Vec::new(),
            scene_embeddings: once_cell::sync::OnceCell::new(),
            embedder: crate::memory::embedding::HashingMemoryEmbedding::new(256),
        }
    }

    /// 用自定义场景模式配置构造
    pub fn with_modes(scene_modes: HashMap<SceneMode, SceneModeConfig>) -> Self {
        Self {
            scene_modes,
            mode_stability_window: 3,
            current_mode: SceneMode::DailyChat,
            mode_history: Vec::new(),
            scene_embeddings: once_cell::sync::OnceCell::new(),
            embedder: crate::memory::embedding::HashingMemoryEmbedding::new(256),
        }
    }

    /// 懒初始化场景 embedding 索引：把每个场景的关键词拼接成文本后嵌入
    fn get_scene_embeddings(&self) -> &Vec<(SceneMode, Vec<f32>)> {
        self.scene_embeddings.get_or_init(|| {
            let mut result = Vec::new();
            for (mode, keywords) in emotion_keywords() {
                let joined = keywords.join(" ");
                if let Ok(emb) = self.embedder.embed(&joined) {
                    result.push((mode, emb));
                }
            }
            result
        })
    }

    /// 基于 embedding 余弦相似度评分（替代纯关键词匹配的语义补充）
    ///
    /// 与 `score_by_keywords` 互补：关键词匹配精确但召回低，
    /// embedding 匹配能捕获语义相近但字面不同的输入（如"好累啊" vs "疲惫"）。
    fn score_by_embedding(&self, text: &str) -> HashMap<SceneMode, f64> {
        let mut scores: HashMap<SceneMode, f64> = HashMap::new();
        let query_emb = match self.embedder.embed(text) {
            Ok(e) => e,
            Err(_) => return scores,
        };
        let scene_embs = self.get_scene_embeddings();
        for (mode, emb) in scene_embs {
            let sim = cosine_similarity(&query_emb, emb);
            // 相似度 > 0.3 才计入，最高贡献 0.4 分（低于关键词精确匹配的 0.9）
            if sim > 0.3 {
                let score = (sim * 0.4).min(0.4);
                scores.insert(*mode, score);
            }
        }
        scores
    }

    /// 重新加载场景模式配置
    pub fn reload_modes(&mut self, scene_modes: HashMap<SceneMode, SceneModeConfig>) {
        self.scene_modes = scene_modes;
        tracing::debug!("[SceneSelector] 场景模式配置已重新加载");
    }

    /// 选择最佳场景模式（5 信号融合）
    ///
    /// 信号：
    /// 1. 情绪关键词匹配
    /// 2. 时间规则
    /// 3. 外部情感信号
    /// 4. 关系/状态信号（亲密度、精力值）
    /// 5. 对话历史信号（输入长度、句式）
    pub fn select(
        &mut self,
        user_input: &str,
        context: Option<&SceneContext>,
        emotion: Option<&str>,
    ) -> SceneMode {
        let ctx = context.cloned().unwrap_or_default();
        let mut candidate_scores: HashMap<SceneMode, f64> = HashMap::new();

        // 1. 情绪关键词匹配
        let keyword_scores = self.score_by_keywords(user_input);
        for (mode, score) in keyword_scores {
            *candidate_scores.entry(mode).or_insert(0.0) += score;
        }

        // 1.5 embedding 语义匹配（关键词匹配的语义补充）
        let embedding_scores = self.score_by_embedding(user_input);
        for (mode, score) in embedding_scores {
            *candidate_scores.entry(mode).or_insert(0.0) += score;
        }

        // 2. 时间规则
        let time_scores = self.score_by_time(ctx.hour);
        for (mode, score) in time_scores {
            *candidate_scores.entry(mode).or_insert(0.0) += score;
        }

        // 3. 外部情绪信号
        if let Some(emo) = emotion {
            let emotion_scores = self.score_by_emotion(emo);
            for (mode, score) in emotion_scores {
                *candidate_scores.entry(mode).or_insert(0.0) += score;
            }
        }

        // 4. 关系/状态信号（亲密度、精力值）
        let status_scores = self.score_by_status(&ctx);
        for (mode, score) in status_scores {
            *candidate_scores.entry(mode).or_insert(0.0) += score;
        }

        // 5. 对话历史信号（输入长度、句式）
        let history_scores = self.score_by_history_patterns(user_input);
        for (mode, score) in history_scores {
            *candidate_scores.entry(mode).or_insert(0.0) += score;
        }

        // 确定最佳候选
        let mut best_mode = self.decide_mode(&candidate_scores);

        // 6. 模式稳定性检查
        best_mode = self.apply_stability(best_mode);

        self.mode_history.push(best_mode);
        let max_history = self.mode_stability_window * 2;
        if self.mode_history.len() > max_history {
            let drain_count = self.mode_history.len() - max_history;
            self.mode_history.drain(..drain_count);
        }

        if best_mode != self.current_mode {
            tracing::debug!(
                "[SceneSelector] 模式切换: {} → {}",
                self.current_mode.as_str(),
                best_mode.as_str()
            );
            self.current_mode = best_mode;
        }

        best_mode
    }

    /// 获取当前（稳定后的）场景模式
    pub fn get_current_mode(&self) -> SceneMode {
        self.current_mode
    }

    /// 获取模式历史记录
    pub fn get_mode_history(&self) -> Vec<SceneMode> {
        self.mode_history.clone()
    }

    /// 强制设置场景模式（外部调用，如作息触发）
    pub fn force_mode(&mut self, mode: SceneMode) {
        tracing::debug!("[SceneSelector] 强制设置模式: {}", mode.as_str());
        self.current_mode = mode;
        self.mode_history.push(mode);
    }

    // ===== 评分方法 =====

    /// 基于情绪关键词评分
    fn score_by_keywords(&self, text: &str) -> HashMap<SceneMode, f64> {
        let mut scores: HashMap<SceneMode, f64> = HashMap::new();
        let text_lower = text.to_lowercase();

        for (mode, keywords) in emotion_keywords() {
            let match_count = keywords.iter().filter(|kw| text_lower.contains(**kw)).count();
            if match_count > 0 {
                let score = (match_count as f64 * 0.3).min(0.9);
                if let Some(mode_config) = self.scene_modes.get(&mode) {
                    if score >= mode_config.min_confidence {
                        scores.insert(mode, score);
                    }
                }
            }
        }

        scores
    }

    /// 基于时间规则评分
    fn score_by_time(&self, hour: Option<u32>) -> HashMap<SceneMode, f64> {
        let mut scores: HashMap<SceneMode, f64> = HashMap::new();

        let h = match hour {
            Some(h) => h,
            None => Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12),
        };

        for (start_hour, end_hour, mode) in time_rules() {
            let in_range = if start_hour <= end_hour {
                h >= start_hour && h <= end_hour
            } else {
                // 跨午夜，如 22-5
                h >= start_hour || h <= end_hour
            };

            if in_range && self.scene_modes.contains_key(&mode) {
                scores.insert(mode, 0.5);
            }
        }

        scores
    }

    /// 基于外部情感信号评分（10 种情感→场景映射）
    fn score_by_emotion(&self, emotion: &str) -> HashMap<SceneMode, f64> {
        let emotion_lower = emotion.trim().to_lowercase();
        let mapping: &[(&str, &[(SceneMode, f64)])] = &[
            ("sad", &[(SceneMode::Comforting, 0.7)][..]),
            ("angry", &[(SceneMode::Guardian, 0.5)][..]),
            ("happy", &[(SceneMode::Banter, 0.5), (SceneMode::Energetic, 0.4)][..]),
            ("surprised", &[(SceneMode::DailyChat, 0.3)][..]),
            ("neutral", &[(SceneMode::DailyChat, 0.3)][..]),
            ("anxious", &[(SceneMode::Guardian, 0.6), (SceneMode::Comforting, 0.4)][..]),
            ("excited", &[(SceneMode::Energetic, 0.6), (SceneMode::Banter, 0.4)][..]),
            ("tired", &[(SceneMode::Comforting, 0.5), (SceneMode::Guardian, 0.3)][..]),
            ("lonely", &[(SceneMode::Cozy, 0.6), (SceneMode::Companion, 0.4)][..]),
            ("bored", &[(SceneMode::Cozy, 0.5), (SceneMode::Banter, 0.4)][..]),
        ];

        let mut scores: HashMap<SceneMode, f64> = HashMap::new();
        for (key, pairs) in mapping {
            if *key == emotion_lower {
                for (mode, score) in *pairs {
                    if let Some(mode_config) = self.scene_modes.get(mode) {
                        if *score >= mode_config.min_confidence {
                            scores.insert(*mode, *score);
                        }
                    }
                }
            }
        }

        scores
    }

    /// 基于关系/状态信号评分（亲密度、精力值、心理学情绪/需求）
    fn score_by_status(&self, context: &SceneContext) -> HashMap<SceneMode, f64> {
        let mut scores: HashMap<SceneMode, f64> = HashMap::new();

        let intimacy = context.intimacy.unwrap_or(50.0);
        let energy = context.energy.unwrap_or(50.0);

        // 高亲密度 + 高精力 → 撒娇模式
        if intimacy > 65.0 && energy > 60.0 && self.scene_modes.contains_key(&SceneMode::Cozy) {
            scores.insert(SceneMode::Cozy, 0.4);
        }

        // 低精力 → 守护模式（Vivian 累了，外皮崩塌）
        if energy < 20.0 && self.scene_modes.contains_key(&SceneMode::Guardian) {
            scores.insert(SceneMode::Guardian, 0.3);
        }

        // 心理学情绪信号：主导情绪直接影响场景偏好
        if let Some(ref emotion) = context.dominant_emotion {
            match emotion.as_str() {
                "sad" | "lonely" if self.scene_modes.contains_key(&SceneMode::Comforting) => {
                    *scores.entry(SceneMode::Comforting).or_insert(0.0) += 0.35;
                }
                "curious" | "joyful" if self.scene_modes.contains_key(&SceneMode::Banter) => {
                    *scores.entry(SceneMode::Banter).or_insert(0.0) += 0.3;
                }
                "fear" | "anxious" if self.scene_modes.contains_key(&SceneMode::Guardian) => {
                    *scores.entry(SceneMode::Guardian).or_insert(0.0) += 0.35;
                }
                _ => {}
            }
        }

        // 需求压力：高压力时偏向 Cozy（寻求归属感）或 Guardian（安全感）
        if let Some(pressure) = context.need_pressure {
            if pressure > 0.6 {
                if self.scene_modes.contains_key(&SceneMode::Cozy) {
                    *scores.entry(SceneMode::Cozy).or_insert(0.0) += pressure * 0.3;
                }
                if self.scene_modes.contains_key(&SceneMode::Guardian) {
                    *scores.entry(SceneMode::Guardian).or_insert(0.0) += pressure * 0.2;
                }
            }
        }

        scores
    }

    /// 基于对话模式评分（长文本/短文本+情绪词）
    fn score_by_history_patterns(&self, text: &str) -> HashMap<SceneMode, f64> {
        let mut scores: HashMap<SceneMode, f64> = HashMap::new();
        let text_lower = text.to_lowercase();
        let char_count = text.chars().count();

        // 长文本 → 守护模式
        if char_count > 80 && self.scene_modes.contains_key(&SceneMode::Guardian) {
            scores.insert(SceneMode::Guardian, 0.4);
        }

        // 短文本 + 情绪词 → 安慰模式
        if char_count < 20
            && ["sigh", "ugh", "never mind", "whatever"]
                .iter()
                .any(|w| text_lower.contains(w))
            && self.scene_modes.contains_key(&SceneMode::Comforting)
        {
            scores.insert(SceneMode::Comforting, 0.4);
        }

        // 短文本 + 笑声词 → 吐槽模式（用户可能在开玩笑）
        if char_count < 30
            && ["haha", "lol", "wtf", "ridiculous"]
                .iter()
                .any(|w| text_lower.contains(w))
            && self.scene_modes.contains_key(&SceneMode::Banter)
        {
            scores.insert(SceneMode::Banter, 0.3);
        }

        scores
    }

    /// 从候选评分中决策最佳模式（0.2 阈值）
    fn decide_mode(&self, candidate_scores: &HashMap<SceneMode, f64>) -> SceneMode {
        if candidate_scores.is_empty() {
            return SceneMode::DailyChat;
        }

        let mut best_mode = SceneMode::DailyChat;
        let mut best_score = f64::MIN;
        for (&mode, &score) in candidate_scores {
            if score > best_score {
                best_score = score;
                best_mode = mode;
            }
        }

        if best_score < 0.2 {
            return SceneMode::DailyChat;
        }

        best_mode
    }

    /// 应用模式稳定性逻辑，防止频繁切换
    fn apply_stability(&self, proposed_mode: SceneMode) -> SceneMode {
        if proposed_mode == self.current_mode {
            return proposed_mode;
        }

        if self.current_mode != SceneMode::DailyChat {
            let window = self.mode_stability_window;
            if window > 0 {
                let need = window.saturating_sub(1);
                if self.mode_history.len() >= need {
                    let recent: &[SceneMode] =
                        &self.mode_history[self.mode_history.len() - need..];
                    if recent.iter().all(|&m| m == self.current_mode) {
                        tracing::debug!(
                            "[SceneSelector] 稳定性保护: 保持 {}, 拒绝切换到 {}",
                            self.current_mode.as_str(),
                            proposed_mode.as_str()
                        );
                        return self.current_mode;
                    }
                }
            }
        }

        proposed_mode
    }
}

impl Default for SceneModeSelector {
    fn default() -> Self {
        Self::new()
    }
}

/// 余弦相似度（cosine similarity）
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm_a < 1e-9 || norm_b < 1e-9 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)) as f64
}
