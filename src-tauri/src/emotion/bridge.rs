//! EmotionBridge — 情感桥接器
//!
//!   - 连接 EmotionAnalyzer（关键词）、LlmEmotionClassifier、PsychologyManager、
//!     Expression Manager（通过回调）
//!   - `process_emotion(text)` 流水线：关键词快分析 → LLM 深度分析 →
//!     更新 PsychologyManager → 触发表情 → 返回综合结果
//!   - 入口：`analyze_and_track(text) -> EmotionContext`
//!     （综合分析并更新心理状态）、`get_current_emotion()`、
//!     `get_emotion_history(limit)`
//!   - 5 分钟文本缓存（TTL 60s 由 LLM 分类层负责，本层缓存整条流水线 5 分钟）
//!   - 批量处理支持

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::embedding_classifier::EmbeddingEmotionClassifier;
use super::llm_classifier::LlmEmotionClassifier;
use super::mapper::{
    emotion_label_to_llm, llm_emotion_valence_arousal, llm_to_emotion_label, normalize_llm_emotion,
    LLM_EMOTION_LABELS,
};
use super::{EmotionAnalyzer, EmotionResult};
use crate::psychology::{
    EmotionDeltas, EmotionLabel, PsychEvent, PsychologyManager, PsychologyOutput,
};

/// 情绪分类缓存 TTL（5 分钟），平衡命中率和情绪变化频率
const CACHE_TTL: Duration = Duration::from_secs(300);
/// 情绪分类缓存最大条目数，避免内存无限增长
const CACHE_MAX_ENTRIES: usize = 256;

/// 表情触发回调类型
///
/// 参数：(expression_name, optional_duration_ms)
/// 在生产环境中通过闭包桥接到 `ExpressionManager::set_expression`。
pub type ExpressionTrigger = Arc<dyn Fn(&str, Option<u64>) + Send + Sync>;

/// 情感分析流水线综合结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionPipelineResult {
    /// 用户情绪标签（14 类 LLM 情绪之一）
    pub emotion: String,
    /// 情绪强度（0.0 ~ 1.0）
    pub intensity: f64,
    /// 效价（-1.0 ~ 1.0）
    pub valence: f64,
    /// 唤醒度（0.0 ~ 1.0）
    pub arousal: f64,
    /// 推荐的 Live2D 表情名（空字符串表示不触发表情变化）
    pub expression: String,
    /// 结果来源：`keyword` / `llm` / `llm_fallback` / `no_llm` / `cache`
    pub source: String,
    /// Vivian 当前主导情绪标签（7 类 EmotionLabel 之一）
    pub pet_emotion: String,
    /// 是否命中缓存
    pub from_cache: bool,
}

impl EmotionPipelineResult {
    /// 从 EmotionResult 构造（不含 expression/pet_emotion 信息，后续填充）
    pub fn from_emotion_result(result: &EmotionResult, source: &str) -> Self {
        Self {
            emotion: result.emotion.clone(),
            intensity: result.intensity,
            valence: result.valence,
            arousal: result.arousal,
            expression: String::new(),
            source: source.to_string(),
            pet_emotion: String::new(),
            from_cache: false,
        }
    }
}

/// 情感上下文 — 综合分析后的完整上下文
///
/// 对应任务规范 `EmotionContext`：原始情感、分类后情感、强度、valence/arousal、
/// 对 PsychologyManager 的影响。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionContext {
    /// 关键词分析原始情感（EmotionAnalyzer 直接结果，可能不在 14 类中）
    pub original_emotion: String,
    /// LLM 分类后情感（14 类 LLM 情绪之一，规范化后）
    pub classified_emotion: String,
    /// 情绪强度（0.0 ~ 1.0）
    pub intensity: f64,
    /// 效价（-1.0 ~ 1.0）
    pub valence: f64,
    /// 唤醒度（0.0 ~ 1.0）
    pub arousal: f64,
    /// Vivian 当前主导情绪标签（7 类 EmotionLabel 之一）
    pub pet_emotion: String,
    /// 结果来源
    pub source: String,
    /// 推荐的 Live2D 表情名（空字符串表示不触发）
    pub expression: String,
    /// 是否命中缓存
    pub from_cache: bool,
    /// 情绪感知偏见（Vivian 当前情绪对用户情绪感知的影响）
    pub perception_bias: HashMap<String, f64>,
    /// 是否成功更新了 PsychologyManager
    pub pet_status_updated: bool,
}

/// 缓存条目
struct CacheEntry {
    result: EmotionPipelineResult,
    timestamp: Instant,
}

/// 情感桥接器
///
/// 组合关键词分析器 + LLM 分类器 + PsychologyManager + 表情回调，
/// 提供 `process_emotion(text)` 与 `analyze_and_track(text)` 一站式情感分析流水线。
pub struct EmotionBridge {
    analyzer: Arc<EmotionAnalyzer>,
    classifier: Arc<LlmEmotionClassifier>,
    psychology: Arc<PsychologyManager>,
    expression_trigger: Option<ExpressionTrigger>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// 当前角色的 ResourceManifest（用于情绪→表情映射）
    manifest: Option<Arc<crate::engine::manifest::ResourceManifest>>,
    /// 即时嵌入分类器：低延迟，用于用户消息到达瞬间或 AI 文本首段完成时触发即时反应
    instant_classifier: Option<Arc<EmbeddingEmotionClassifier>>,
}

impl EmotionBridge {
    /// 构造桥接器（简化版，注入 PsychologyManager + manifest）
    ///
    /// 内部自动创建默认的 EmotionAnalyzer 和 LlmEmotionClassifier。
    pub fn new(
        psychology: Arc<PsychologyManager>,
        manifest: Option<Arc<crate::engine::manifest::ResourceManifest>>,
    ) -> Self {
        let analyzer = Arc::new(EmotionAnalyzer::new());
        let classifier = Arc::new(LlmEmotionClassifier::new(None));
        Self {
            analyzer,
            classifier,
            psychology,
            expression_trigger: None,
            cache: Mutex::new(HashMap::new()),
            manifest,
            instant_classifier: None,
        }
    }

    /// 完整构造桥接器（注入所有依赖）
    pub fn with_dependencies(
        analyzer: Arc<EmotionAnalyzer>,
        classifier: Arc<LlmEmotionClassifier>,
        psychology: Arc<PsychologyManager>,
        manifest: Option<Arc<crate::engine::manifest::ResourceManifest>>,
    ) -> Self {
        Self {
            analyzer,
            classifier,
            psychology,
            expression_trigger: None,
            cache: Mutex::new(HashMap::new()),
            manifest,
            instant_classifier: None,
        }
    }

    /// 设置表情触发回调
    pub fn with_expression_trigger(mut self, trigger: ExpressionTrigger) -> Self {
        self.expression_trigger = Some(trigger);
        self
    }

    /// 注入即时嵌入分类器
    pub fn with_instant_classifier(
        mut self,
        classifier: Arc<EmbeddingEmotionClassifier>,
    ) -> Self {
        self.instant_classifier = Some(classifier);
        self
    }

    /// 设置表情触发回调（mutable setter）
    pub fn set_expression_trigger(&self, _trigger: ExpressionTrigger) {
        tracing::warn!("[EmotionBridge] set_expression_trigger 当前未生效，请在构造时使用 with_expression_trigger");
    }

    /// 即时情绪分类（低延迟，不更新心理状态，不触发表情）
    ///
    /// 用于三层反应系统的 Layer 1（用户消息到达）和 Layer 2（AI 文本首段完成）：
    /// - 优先使用嵌入分类器（本地哈希 <1ms，远程嵌入 50-200ms）
    /// - 嵌入不可用或相似度不足时返回 Err，由上层决定如何处理（如弹 toast 报错）
    /// - 不走 LLM 深度分析，不写缓存，不更新 PsychologyManager
    /// - 返回结果供前端立即应用 FACS 参数
    pub fn classify_instant(&self, text: &str) -> Result<EmotionResult, String> {
        if let Some(instant) = &self.instant_classifier {
            return instant.classify(text);
        }
        // 未注入即时分类器视为配置错误
        Err("即时嵌入分类器未配置，请在设置中启用向量检索".to_string())
    }

    /// 处理单条文本：关键词分析 + LLM 深度分析 + 状态更新 + 表情触发
    pub async fn process_emotion(&self, text: &str) -> EmotionPipelineResult {
        // 1. 检查缓存
        if let Some(cached) = self.check_cache(text) {
            return cached;
        }

        // 2. 关键词快速分析（同步）
        let keyword_result = self.analyzer.analyze(text);

        // 3. LLM 深度分析（异步）
        let llm_result = self.classifier.classify(text).await;

        // 4. 选择最终结果：LLM 成功则用 LLM，否则用关键词
        let (final_result, source) = if llm_result.source == "llm" {
            (llm_result, "llm".to_string())
        } else if self.classifier.has_llm() {
            tracing::debug!(
                "[EmotionBridge] LLM 降级到关键词：source={}",
                llm_result.source
            );
            (keyword_result.clone(), "keyword_llm_fallback".to_string())
        } else {
            (keyword_result.clone(), "keyword".to_string())
        };

        // 5. 规范化情绪标签到 14 类
        let normalized_emotion = normalize_llm_emotion(&final_result.emotion).to_string();
        let mut pipeline_result =
            EmotionPipelineResult::from_emotion_result(&final_result, &source);
        pipeline_result.emotion = normalized_emotion.clone();

        // 6. 映射到 EmotionLabel 并通过 PsychologyManager 更新情绪状态
        let emotion_label = llm_to_emotion_label(&normalized_emotion);
        pipeline_result.pet_emotion = emotion_label.as_str().to_string();

        let deltas = llm_emotion_to_deltas(&normalized_emotion, pipeline_result.intensity);
        let psy_output = PsychologyOutput {
            emotion_update: Some(deltas),
            ..Default::default()
        };
        self.psychology.apply_llm_output(&psy_output);

        // 7. 触发表情变化（用当前角色的 manifest 映射情绪→表情）
        let expression = self
            .manifest
            .as_ref()
            .map(|m| m.emotion_to_expression_name(&normalized_emotion))
            .unwrap_or_default();
        pipeline_result.expression = expression.clone();
        if !expression.is_empty() {
            if let Some(trigger) = &self.expression_trigger {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    trigger(&expression, Some(3000));
                }));
            }
        }

        // 8. 写入缓存
        self.save_cache(text.to_string(), pipeline_result.clone());

        pipeline_result
    }

    /// 综合分析并更新 PsychologyManager，返回完整情绪上下文
    ///
    /// 执行 process_emotion 流水线后，附加感知偏见与更新状态，
    /// 返回包含原始情感/分类后情感/强度/valence/arousal/影响的上下文。
    pub async fn analyze_and_track(&self, text: &str) -> EmotionContext {
        let pipeline = self.process_emotion(text).await;
        let original_emotion = self.analyzer.analyze(text).emotion;
        let perception_bias = self.get_perception_bias();
        let pet_status_updated = !pipeline.pet_emotion.is_empty();

        EmotionContext {
            original_emotion,
            classified_emotion: pipeline.emotion.clone(),
            intensity: pipeline.intensity,
            valence: pipeline.valence,
            arousal: pipeline.arousal,
            pet_emotion: pipeline.pet_emotion.clone(),
            source: pipeline.source.clone(),
            expression: pipeline.expression.clone(),
            from_cache: pipeline.from_cache,
            perception_bias,
            pet_status_updated,
        }
    }

    /// 获取当前情绪（来自 PsychologyManager 主导情绪）
    ///
    /// 返回 Vivian 当前情绪对应的 EmotionResult（14 类 LLM 情绪标签 + 效价/唤醒度）。
    pub fn get_current_emotion(&self) -> EmotionResult {
        let emotion = self.psychology.emotion();
        let (label, _intensity) = emotion.dominant();
        let llm_label = emotion_label_to_llm(label);
        let (valence, arousal) = (emotion.valence(), emotion.arousal());
        EmotionResult {
            emotion: llm_label.to_string(),
            intensity: _intensity,
            valence,
            arousal,
            source: "psychology".to_string(),
            ..Default::default()
        }
    }

    /// 获取情绪历史记录（来自 PsychologyManager 心理事件）
    ///
    /// 将 PsychologyManager 最近 N 条 PsychEvent 转换为 EmotionResult 列表。
    pub fn get_emotion_history(&self, limit: usize) -> Vec<EmotionResult> {
        let snapshot = self.psychology.snapshot();
        snapshot
            .events
            .iter()
            .rev()
            .take(limit)
            .map(|ev| psych_event_to_result(ev))
            .collect()
    }

    /// 将 Vivian 的情绪感知偏见应用到情绪分析结果上
    ///
    /// - 接收原始情绪分数（如 `{"happy": 0.6, "neutral": 0.3}`）
    /// - 应用偏见后重新归一化
    /// - 若所有分数被压到 0，返回原始分数
    pub fn apply_perception_bias(
        &self,
        emotion_scores: HashMap<String, f64>,
        bias: Option<&HashMap<String, f64>>,
    ) -> HashMap<String, f64> {
        let owned_bias: HashMap<String, f64>;
        let bias_map = match bias {
            Some(b) => b,
            None => {
                owned_bias = self.get_perception_bias();
                &owned_bias
            }
        };

        let mut adjusted: HashMap<String, f64> = HashMap::with_capacity(emotion_scores.len());
        for (label, score) in &emotion_scores {
            let b = bias_map.get(label).copied().unwrap_or(0.0);
            let v = (score + b).clamp(0.0, 1.0);
            adjusted.insert(label.clone(), v);
        }

        // 重新归一化（防御 zero division）
        let total: f64 = adjusted.values().sum();
        if total > 0.0 {
            for v in adjusted.values_mut() {
                *v /= total;
            }
            adjusted
        } else {
            emotion_scores
        }
    }

    /// 获取当前情绪上下文（供日记和 prompt 使用）
    ///
    /// 返回 Vivian 当前心情、感知偏见、情绪弧线等完整上下文。
    pub fn get_emotion_context(&self) -> serde_json::Value {
        let emotion = self.psychology.emotion();
        let (dominant_label, dominant_intensity) = emotion.dominant();
        let (valence, arousal) = (emotion.valence(), emotion.arousal());
        let snapshot = self.psychology.snapshot();
        let bias = self.get_perception_bias();

        serde_json::json!({
            "vivian_mood": {
                "valence": valence,
                "arousal": arousal,
                "primary_emotion": dominant_label.as_str(),
                "primary_intensity": dominant_intensity,
                "pet_emotion_type": dominant_label.as_str(),
            },
            "perception_bias": bias,
            "emotion_arc": describe_emotion_arc(&snapshot.events),
        })
    }

    /// 获取当前感知偏见（基于 Vivian 当前情绪对 14 类用户情绪的偏置）
    ///
    /// 简化版规则：Vivian 当前情绪为正 → happy/excited/grateful 略增、sad/angry 略减；
    /// Vivian 当前情绪为负 → sad/angry/anxious 略增、happy 略减；
    /// 偏置幅度 ±0.05。
    pub fn get_perception_bias(&self) -> HashMap<String, f64> {
        let emotion = self.psychology.emotion();
        let valence = emotion.valence();
        let mut bias = HashMap::new();
        for &label in LLM_EMOTION_LABELS {
            bias.insert(label.to_string(), 0.0);
        }
        // 按 valence 正负对 14 类情绪施加小幅偏置
        let shift = valence * 0.05;
        if shift > 0.0 {
            *bias.entry("happy".to_string()).or_insert(0.0) += shift;
            *bias.entry("excited".to_string()).or_insert(0.0) += shift;
            *bias.entry("grateful".to_string()).or_insert(0.0) += shift;
            *bias.entry("sad".to_string()).or_insert(0.0) -= shift;
            *bias.entry("angry".to_string()).or_insert(0.0) -= shift;
            *bias.entry("anxious".to_string()).or_insert(0.0) -= shift;
        } else if shift < 0.0 {
            *bias.entry("sad".to_string()).or_insert(0.0) += shift.abs();
            *bias.entry("angry".to_string()).or_insert(0.0) += shift.abs() * 0.5;
            *bias.entry("anxious".to_string()).or_insert(0.0) += shift.abs() * 0.5;
            *bias.entry("happy".to_string()).or_insert(0.0) += shift;
            *bias.entry("excited".to_string()).or_insert(0.0) += shift;
        }
        bias
    }

    /// 批量处理多条文本
    ///
    /// 顺序处理（避免 LLM 并发限流），每条独立缓存。
    pub async fn process_emotion_batch(
        &self,
        texts: Vec<String>,
    ) -> Vec<EmotionPipelineResult> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.process_emotion(&text).await);
        }
        results
    }

    /// 清空缓存（主要用于测试）
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// 当前缓存条目数
    pub fn cache_size(&self) -> usize {
        self.cache.lock().len()
    }

    /// 检查缓存命中
    fn check_cache(&self, text: &str) -> Option<EmotionPipelineResult> {
        let mut cache = self.cache.lock();
        let entry = cache.get(text)?;
        if entry.timestamp.elapsed() < CACHE_TTL {
            let mut result = entry.result.clone();
            result.from_cache = true;
            result.source = "cache".to_string();
            Some(result)
        } else {
            // 过期，移除
            cache.remove(text);
            None
        }
    }

    /// 写入缓存（带容量控制）
    fn save_cache(&self, text: String, result: EmotionPipelineResult) {
        let mut cache = self.cache.lock();
        // 容量控制：超过上限时清理过期项
        if cache.len() >= CACHE_MAX_ENTRIES {
            cache.retain(|_, entry| entry.timestamp.elapsed() < CACHE_TTL);
            // 如果清理后仍超限，移除最早的一半
            if cache.len() >= CACHE_MAX_ENTRIES {
                let to_remove: Vec<String> = cache
                    .iter()
                    .min_by_key(|(_, e)| e.timestamp)
                    .map(|(k, _)| k.clone())
                    .into_iter()
                    .collect();
                for k in to_remove {
                    cache.remove(&k);
                }
            }
        }
        cache.insert(
            text,
            CacheEntry {
                result,
                timestamp: Instant::now(),
            },
        );
    }
}

/// 将 14 类 LLM 情绪转换为 EmotionDeltas（用于 PsychologyManager.apply_llm_output）
///
/// 根据 LlmEmotion 映射到 EmotionLabel，对应维度增量按 intensity 缩放。
fn llm_emotion_to_deltas(emotion: &str, intensity: f64) -> EmotionDeltas {
    let m = intensity.clamp(0.0, 1.0) * 0.2;
    let label = llm_to_emotion_label(emotion);
    match label {
        EmotionLabel::Joy => EmotionDeltas {
            joy: m,
            closeness: m * 0.3,
            ..Default::default()
        },
        EmotionLabel::Sadness => EmotionDeltas {
            sadness: m,
            loneliness: m * 0.3,
            ..Default::default()
        },
        EmotionLabel::Anger => EmotionDeltas {
            anger: m,
            ..Default::default()
        },
        EmotionLabel::Fear => EmotionDeltas {
            fear: m,
            ..Default::default()
        },
        EmotionLabel::Closeness => EmotionDeltas {
            closeness: m,
            joy: m * 0.3,
            loneliness: -m * 0.2,
            ..Default::default()
        },
        EmotionLabel::Loneliness => EmotionDeltas {
            loneliness: m,
            sadness: m * 0.3,
            ..Default::default()
        },
        EmotionLabel::Curiosity => EmotionDeltas {
            curiosity: m,
            ..Default::default()
        },
    }
}

/// 将 PsychEvent 转换为 EmotionResult
fn psych_event_to_result(ev: &PsychEvent) -> EmotionResult {
    let (label, intensity) = ev.emotion_after.dominant();
    let llm_label = emotion_label_to_llm(label);
    let (default_v, default_a) = llm_emotion_valence_arousal(llm_label);
    let v = ev.emotion_after.valence();
    let a = ev.emotion_after.arousal();
    EmotionResult {
        emotion: llm_label.to_string(),
        intensity,
        valence: if v.abs() > 0.001 { v } else { default_v },
        arousal: if a.abs() > 0.001 { a } else { default_a },
        source: "psychology_event".to_string(),
        ..Default::default()
    }
}

/// 描述今天的情绪弧线
fn describe_emotion_arc(events: &[PsychEvent]) -> String {
    if events.is_empty() {
        return "今天还没有什么特别的情绪变化".to_string();
    }
    if events.len() < 2 {
        let (label, _) = events[0].emotion_after.dominant();
        return format!("今天Vivian一直{}", label.display_zh());
    }

    // 分析情绪变化趋势，去重但保持顺序
    let mut unique: Vec<EmotionLabel> = Vec::new();
    for e in events {
        let (label, _) = e.emotion_after.dominant();
        if !unique.contains(&label) {
            unique.push(label);
        }
    }

    if unique.len() == 1 {
        format!("今天Vivian一直{}", unique[0].display_zh())
    } else if unique.len() == 2 {
        format!(
            "今天Vivian从{}变成了{}",
            unique[0].display_zh(),
            unique[unique.len() - 1].display_zh()
        )
    } else {
        format!(
            "今天Vivian经历了{}等多种情绪",
            unique
                .iter()
                .take(3)
                .map(|l| l.display_zh())
                .collect::<Vec<_>>()
                .join("、")
        )
    }
}

/// 14 类 LLM 情绪 → Live2D 表情名映射
///
/// 由 EmotionBridge 持有的 ResourceManifest 按 `model_manifest.json` 中
/// `emotion_map` 字段进行映射。manifest 未注入时返回空串（不触发表情）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 PsychologyManager（临时目录）
    fn make_psychology_manager() -> Arc<PsychologyManager> {
        let dir = std::env::temp_dir().join(format!(
            "vivian_bridge_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("psychology.json");
        Arc::new(PsychologyManager::load_or_init(path))
    }

    /// 构造 Vivian 测试 manifest（emotion_map 与 model_manifest.json 一致）
    fn make_vivian_manifest() -> Arc<crate::engine::manifest::ResourceManifest> {
        use crate::engine::manifest::{ModelManifest, ResourceManifest};

        let json = r#"{
            "display_name": "Vivian",
            "model_file": "Vivian.model3.json",
            "expressions": [
                { "semantic": "shy", "name": "shy", "label": "Shy", "category": "emotion" },
                { "semantic": "star_eyes", "name": "star_eyes", "label": "Star Eyes", "category": "emotion" },
                { "semantic": "angry", "name": "angry", "label": "Angry", "category": "emotion" },
                { "semantic": "cry", "name": "cry", "label": "Cry", "category": "emotion" },
                { "semantic": "love_eyes", "name": "love_eyes", "label": "Love Eyes", "category": "emotion" },
                { "semantic": "dark_face", "name": "dark_face", "label": "Dark Face", "category": "emotion" },
                { "semantic": "sweat", "name": "sweat", "label": "Sweat", "category": "emotion" },
                { "semantic": "speechless", "name": "speechless", "label": "Speechless", "category": "emotion" },
                { "semantic": "blank_eyes", "name": "blank_eyes", "label": "Blank Eyes", "category": "emotion" },
                { "semantic": "confused", "name": "confused", "label": "Confused", "category": "emotion" }
            ],
            "aliases": {},
            "fallbacks": ["shy", "star_eyes", "speechless", "confused", "angry"],
            "emotion_map": {
                "happy": "star_eyes", "excited": "star_eyes", "grateful": "love_eyes",
                "surprised": "star_aura",
                "angry": "angry", "frustrated": "dark_face", "anxious": "sweat",
                "sad": "cry", "disappointed": "speechless",
                "tired": "blank_eyes", "bored": "blank_eyes",
                "neutral": "", "curious": "confused", "confused": "confused"
            },
            "motions": ["idle"],
            "motion_aliases": {},
            "interaction_map": {}
        }"#;
        let mf: ModelManifest = serde_json::from_str(json).unwrap();
        Arc::new(ResourceManifest::from_manifest(mf))
    }

    /// 构造测试用桥接器（无 LLM 客户端 + Vivian manifest）
    fn make_bridge_no_llm() -> EmotionBridge {
        let psychology = make_psychology_manager();
        EmotionBridge::new(psychology, Some(make_vivian_manifest()))
    }

    #[tokio::test]
    async fn test_process_emotion_no_llm_uses_keyword() {
        let bridge = make_bridge_no_llm();
        let result = bridge.process_emotion("今天真的很开心，哈哈！").await;
        // 关键词命中 happy
        assert_eq!(result.emotion, "happy");
        assert_eq!(result.source, "keyword");
        assert!(!result.from_cache);
        // pet_emotion 应映射到 Joy
        assert_eq!(result.pet_emotion, "joy");
        // happy → "star_eyes" 表情（由 Vivian model_manifest.json emotion_map 映射）
        assert_eq!(result.expression, "star_eyes");
    }

    #[tokio::test]
    async fn test_process_emotion_neutral_text() {
        let bridge = make_bridge_no_llm();
        let result = bridge.process_emotion("今天天气不错").await;
        assert_eq!(result.emotion, "neutral");
        assert_eq!(result.source, "keyword");
        // neutral 不触发表情
        assert_eq!(result.expression, "");
    }

    #[tokio::test]
    async fn test_process_emotion_cache_hit() {
        let bridge = make_bridge_no_llm();
        let text = "今天真的很开心，哈哈！";
        let first = bridge.process_emotion(text).await;
        assert!(!first.from_cache);
        assert_eq!(first.source, "keyword");

        // 第二次相同文本应命中缓存
        let second = bridge.process_emotion(text).await;
        assert!(second.from_cache);
        assert_eq!(second.source, "cache");
        assert_eq!(second.emotion, first.emotion);
        assert!(bridge.cache_size() >= 1);
    }

    #[tokio::test]
    async fn test_process_emotion_updates_psychology() {
        let bridge = make_bridge_no_llm();
        let _ = bridge.process_emotion("气死我了，烦死了").await;
        // 应更新 PsychologyManager 主导情绪为 Anger
        let emotion = bridge.psychology.emotion();
        let (label, _) = emotion.dominant();
        assert_eq!(label, EmotionLabel::Anger);
    }

    #[tokio::test]
    async fn test_process_emotion_batch() {
        let bridge = make_bridge_no_llm();
        let texts = vec![
            "今天好开心".to_string(),
            "我好难过".to_string(),
            "气死我了".to_string(),
        ];
        let results = bridge.process_emotion_batch(texts).await;
        assert_eq!(results.len(), 3);
        // 最后一条是 angry
        assert_eq!(results[2].emotion, "angry");
    }

    #[tokio::test]
    async fn test_expression_trigger_callback_fires() {
        use std::sync::Mutex as StdMutex;
        let trigger_calls: Arc<StdMutex<Vec<(String, Option<u64>)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let calls_clone = trigger_calls.clone();
        let trigger: ExpressionTrigger = Arc::new(move |name: &str, duration: Option<u64>| {
            calls_clone
                .lock()
                .unwrap()
                .push((name.to_string(), duration));
        });

        let psychology = make_psychology_manager();
        let bridge = EmotionBridge::new(psychology, Some(make_vivian_manifest()))
            .with_expression_trigger(trigger);

        // happy → "star_eyes" 表情，应触发回调
        let _ = bridge.process_emotion("今天好开心，哈哈").await;
        let calls = trigger_calls.lock().unwrap();
        assert!(calls.iter().any(|(n, _)| n == "star_eyes"));
    }

    #[tokio::test]
    async fn test_cache_expires_after_ttl() {
        let bridge = make_bridge_no_llm();
        let text = "今天好开心，哈哈";
        let _ = bridge.process_emotion(text).await;
        assert!(bridge.cache_size() >= 1);

        // 手动将缓存时间设置为很久以前，模拟过期
        {
            let mut cache = bridge.cache.lock();
            if let Some(entry) = cache.get_mut(text) {
                entry.timestamp = Instant::now() - Duration::from_secs(600);
            }
        }

        // 再次查询应重新计算（from_cache=false）
        let result = bridge.process_emotion(text).await;
        assert!(!result.from_cache);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let bridge = make_bridge_no_llm();
        let _ = bridge.process_emotion("好开心").await;
        assert!(bridge.cache_size() >= 1);
        bridge.clear_cache();
        assert_eq!(bridge.cache_size(), 0);
    }

    #[test]
    fn test_pipeline_result_from_emotion_result() {
        let er = EmotionResult {
            emotion: "happy".to_string(),
            intensity: 0.8,
            valence: 0.7,
            arousal: 0.5,
            source: "llm".to_string(),
            ..Default::default()
        };
        let pr = EmotionPipelineResult::from_emotion_result(&er, "llm");
        assert_eq!(pr.emotion, "happy");
        assert!((pr.intensity - 0.8).abs() < 1e-9);
        assert!((pr.valence - 0.7).abs() < 1e-9);
        assert!((pr.arousal - 0.5).abs() < 1e-9);
        assert_eq!(pr.source, "llm");
        assert!(!pr.from_cache);
        assert!(pr.expression.is_empty());
        assert!(pr.pet_emotion.is_empty());
    }

    #[tokio::test]
    async fn test_analyze_and_track_returns_context() {
        let bridge = make_bridge_no_llm();
        let ctx = bridge.analyze_and_track("今天好开心，哈哈").await;
        // 原始情感来自关键词分析
        assert_eq!(ctx.original_emotion, "happy");
        // 分类后情感已规范化
        assert_eq!(ctx.classified_emotion, "happy");
        // pet_emotion 应为 joy
        assert_eq!(ctx.pet_emotion, "joy");
        // PsychologyManager 应已更新
        assert!(ctx.pet_status_updated);
        // perception_bias 应包含全部 14 类
        assert_eq!(ctx.perception_bias.len(), 14);
    }

    #[tokio::test]
    async fn test_get_current_emotion_reflects_psychology() {
        let bridge = make_bridge_no_llm();
        // 触发一次分析以更新 PsychologyManager
        let _ = bridge.process_emotion("气死我了，烦死了").await;
        let current = bridge.get_current_emotion();
        // 主导情绪为 Anger → 14 类 angry
        assert_eq!(current.emotion, "angry");
        assert_eq!(current.source, "psychology");
    }

    #[tokio::test]
    async fn test_get_emotion_history_returns_results() {
        let bridge = make_bridge_no_llm();
        // 触发多次分析（不同 trigger 避免去重）
        let _ = bridge.process_emotion("今天好开心").await;
        let _ = bridge.process_emotion("我好难过").await;
        let _ = bridge.process_emotion("气死我了").await;

        let history = bridge.get_emotion_history(10);
        assert!(!history.is_empty());
        // 每条记录应是 14 类 LLM 情绪标签
        for r in &history {
            assert!(LLM_EMOTION_LABELS.contains(&r.emotion.as_str()));
            assert_eq!(r.source, "psychology_event");
        }
    }

    #[tokio::test]
    async fn test_apply_perception_bias_normalizes() {
        let bridge = make_bridge_no_llm();
        // 先触发一次积极情绪以建立偏置
        let _ = bridge.process_emotion("今天好开心，哈哈").await;

        let mut scores = HashMap::new();
        scores.insert("happy".to_string(), 0.5);
        scores.insert("neutral".to_string(), 0.5);

        let adjusted = bridge.apply_perception_bias(scores, None);
        // 归一化后总和应为 1.0
        let total: f64 = adjusted.values().sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_get_emotion_context_returns_json() {
        let bridge = make_bridge_no_llm();
        let _ = bridge.process_emotion("今天好开心").await;
        let ctx = bridge.get_emotion_context();
        // 应包含 vivian_mood / perception_bias / emotion_arc 字段
        assert!(ctx.get("vivian_mood").is_some());
        assert!(ctx.get("perception_bias").is_some());
        assert!(ctx.get("emotion_arc").is_some());
    }

    #[test]
    fn test_llm_emotion_to_deltas_joy() {
        let deltas = llm_emotion_to_deltas("happy", 0.8);
        assert!(deltas.joy > 0.0);
        assert!(deltas.closeness > 0.0);
    }

    #[test]
    fn test_llm_emotion_to_deltas_anger() {
        let deltas = llm_emotion_to_deltas("angry", 0.7);
        assert!(deltas.anger > 0.0);
        assert_eq!(deltas.joy, 0.0);
    }

    #[test]
    fn test_llm_emotion_to_deltas_intensity_scaling() {
        let low = llm_emotion_to_deltas("happy", 0.3);
        let high = llm_emotion_to_deltas("happy", 0.9);
        assert!(high.joy > low.joy);
    }
}
