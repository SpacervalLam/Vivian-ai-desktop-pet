//! 情感分析模块
//!
//! 包含子模块：
//! - `mapper`：14 类 LLM 情绪 <-> 7 类 EmotionLabel 映射
//! - `llm_classifier`：LLM 驱动的 14 类情绪分类器
//! - `response_strategy`：6 类响应策略 + StrategyConfig
//! - `bridge`：EmotionBridge 情感桥接器，组合关键词 + LLM + PsychologyManager + 表情
//!   提供 `analyze_and_track` / `get_current_emotion`
//!   / `get_emotion_history` / `apply_perception_bias` / `get_emotion_context`
//!
//! 顶层保留原有 `EmotionAnalyzer`（关键词）、`EmotionTracker`、`EmotionResult`。
//! `EmotionClassifier` 为 `LlmEmotionClassifier` 的别名。
//!
//! 14 类 LLM 情绪标签：
//! `happy / excited / grateful / sad / frustrated / anxious / tired /
//!  angry / disappointed / surprised / curious / neutral / bored / confused`
//!
//! EmotionLabel（7 项）是系统唯一情绪枚举，定义在 `crate::psychology` 中。

pub mod bridge;
pub mod embedding_classifier;
pub mod embedding_corpus_en;
pub mod embedding_corpus_ja;
pub mod fast_semantic;
pub mod llm_classifier;
pub mod mapper;
pub mod response_strategy;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ============== 关键类型重导出 ==============

pub use bridge::{
    EmotionBridge, EmotionContext, EmotionPipelineResult, ExpressionTrigger,
};
pub use embedding_classifier::EmbeddingEmotionClassifier;
pub use fast_semantic::{DimensionResult, FastPerceptionResult, FastSemanticAnalyzer};
pub use llm_classifier::{
    parse_llm_response, EmotionLlmClient, LlmEmotionClassifier, ProviderLlmAdapter,
};
pub use mapper::{
    emotion_label_to_llm, is_valid_llm_emotion, llm_emotion_valence_arousal, llm_to_emotion_label,
    normalize_llm_emotion, LlmEmotion, LLM_EMOTION_LABELS,
};
pub use response_strategy::{
    ResponseStrategy, ResponseStrategyRecommendation, ResponseStrategyType, StrategyConfig,
};

/// `EmotionClassifier` 别名
///
/// Rust 端实际类型为 `LlmEmotionClassifier`，这里提供同名别名以便上层
/// 按命名一致性引用。
pub type EmotionClassifier = LlmEmotionClassifier;

// ============== EmotionResult ==============

/// 单次情感分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionResult {
    pub emotion: String,
    pub intensity: f64,
    pub valence: f64,
    pub arousal: f64,
    /// 结果来源：`keyword` / `llm` / `llm_fallback` / `llm_parse_fail` / `no_llm` / `embedding_*` 等
    #[serde(default = "default_source")]
    pub source: String,
    /// 分类置信度 [0.0, 1.0]，仅嵌入分类器填充
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// 次高票情绪标签（当与主情绪差距较小时填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_emotion: Option<String>,
    /// 情绪指向：`self` / `other` / `ai` / `situation`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

fn default_source() -> String {
    "keyword".to_string()
}

impl Default for EmotionResult {
    fn default() -> Self {
        Self {
            emotion: "neutral".to_string(),
            intensity: 0.0,
            valence: 0.0,
            arousal: 0.0,
            source: "keyword".to_string(),
            confidence: None,
            secondary_emotion: None,
            target: None,
        }
    }
}

impl EmotionResult {
    /// 构造一个 neutral 的默认结果（source = "keyword"）
    pub fn neutral() -> Self {
        Self::default()
    }
}

// ============== EmotionAnalyzer（关键词）==============

/// 情感分析器 - 基于中文关键词的情感识别
///
/// 注入 `EmotionLlmClient` 后，`async_analyze` 会优先走 LLM 分类，
/// 失败时回退到关键词分析；`analyze` 始终为同步关键词分析（向后兼容）。
pub struct EmotionAnalyzer {
    happy_keywords: Vec<&'static str>,
    sad_keywords: Vec<&'static str>,
    angry_keywords: Vec<&'static str>,
    /// 可选 LLM 客户端，注入后 `async_analyze` 优先走 LLM
    llm_client: Option<Arc<dyn EmotionLlmClient>>,
}

impl EmotionAnalyzer {
    pub fn new() -> Self {
        Self {
            happy_keywords: vec![
                "开心", "高兴", "快乐", "喜悦", "兴奋", "愉快", "幸福", "喜欢", "爱", "棒",
                "好", "哈哈", "嘿嘿", "呵呵", "😊", "😄", "😀", "😁", "🙂",
            ],
            sad_keywords: vec![
                "难过", "伤心", "悲伤", "哭", "失望", "孤独", "寂寞", "痛苦", "心疼", "郁闷",
                "沮丧", "无奈", "😢", "😭", "😔", "😞",
            ],
            angry_keywords: vec![
                "生气", "愤怒", "气死", "烦死", "讨厌", "可恶", "滚", "闭嘴", "混蛋", "烦",
                "恼火", "😤", "😠", "😡",
            ],
            llm_client: None,
        }
    }

    /// 注入 LLM 客户端构造分析器
    pub fn new_with_llm(client: Arc<dyn EmotionLlmClient>) -> Self {
        let mut analyzer = Self::new();
        analyzer.llm_client = Some(client);
        analyzer
    }

    /// 链式构建器：注入 LLM 路由器
    pub fn with_llm(mut self, router: Arc<crate::providers::ModelRouter>) -> Self {
        self.llm_client = Some(router);
        self
    }

    /// 链式构建器：注入情绪桥接器（保留字段以便后续扩展）
    pub fn with_bridge(self, _bridge: Arc<EmotionBridge>) -> Self {
        self
    }

    /// 是否已注入 LLM 客户端
    pub fn has_llm(&self) -> bool {
        self.llm_client.is_some()
    }

    /// 分析文本中的情感
    pub fn analyze(&self, text: &str) -> EmotionResult {
        let mut happy_count = 0u32;
        let mut sad_count = 0u32;
        let mut angry_count = 0u32;

        for kw in &self.happy_keywords {
            happy_count += text.matches(kw).count() as u32;
        }
        for kw in &self.sad_keywords {
            sad_count += text.matches(kw).count() as u32;
        }
        for kw in &self.angry_keywords {
            angry_count += text.matches(kw).count() as u32;
        }

        let total = happy_count + sad_count + angry_count;
        if total == 0 {
            return EmotionResult {
                emotion: "neutral".to_string(),
                intensity: 0.0,
                valence: 0.0,
                arousal: 0.0,
                source: "keyword".to_string(),
                ..Default::default()
            };
        }

        let (emotion, valence, arousal) = if happy_count >= sad_count && happy_count >= angry_count {
            ("happy", 0.7, 0.5)
        } else if sad_count >= happy_count && sad_count >= angry_count {
            ("sad", -0.6, -0.3)
        } else {
            ("angry", -0.7, 0.7)
        };

        let intensity = (total as f64 / (text.chars().count().max(1) as f64) * 10.0).min(1.0);

        EmotionResult {
            emotion: emotion.to_string(),
            intensity,
            valence,
            arousal,
            source: "keyword".to_string(),
            ..Default::default()
        }
    }

    /// 异步分析入口：优先 LLM 分类，失败回退关键词
    pub async fn async_analyze(&self, text: &str) -> EmotionResult {
        if let Some(client) = &self.llm_client {
            let classifier = LlmEmotionClassifier::new(Some(client.clone()));
            let result = classifier.classify(text).await;
            if result.source == "llm" {
                return result;
            }
            tracing::debug!(
                "[EmotionAnalyzer] LLM 降级({})，回退关键词分析",
                result.source
            );
        }
        self.analyze(text)
    }

    /// 情绪趋势：返回 tracker 历史记录
    pub fn get_trend(&self, tracker: &EmotionTracker) -> Vec<EmotionResult> {
        tracker.get_history().to_vec()
    }

    /// 主导情绪：统计 tracker 历史中最常见的情绪
    pub fn get_dominant_emotion(&self, tracker: &EmotionTracker) -> String {
        tracker.get_dominant_emotion()
    }

    /// 情绪提示词文本（可注入 system prompt）
    pub fn get_emotion_prompt_text(&self, result: &EmotionResult) -> String {
        let label = emotion_to_chinese(&result.emotion);
        format!("用户当前情绪：{}（强度{:.2}）", label, result.intensity)
    }

    /// 带感知偏移的分析：通过 bridge 执行流水线并应用 Vivian 感知偏见
    pub async fn analyze_with_bridge(
        &self,
        bridge: &EmotionBridge,
        text: &str,
    ) -> EmotionResult {
        let pipeline = bridge.process_emotion(text).await;
        let result = EmotionResult {
            emotion: pipeline.emotion,
            intensity: pipeline.intensity,
            valence: pipeline.valence,
            arousal: pipeline.arousal,
            source: pipeline.source,
            ..Default::default()
        };
        let bias = bridge.get_perception_bias();
        apply_bias_to_result(result, &bias)
    }

    /// 薇薇安视角提示词
    pub fn get_vivian_aware_prompt(&self, result: &EmotionResult) -> String {
        vivian_aware_prompt(&result.emotion, result.intensity)
    }
}

impl Default for EmotionAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ============== 工具函数 ==============

/// 14 类 LLM 情绪标签 → 中文描述
fn emotion_to_chinese(label: &str) -> &'static str {
    match label {
        "happy" => "开心",
        "excited" => "兴奋",
        "grateful" => "感激",
        "sad" => "难过",
        "frustrated" => "沮丧",
        "anxious" => "焦虑",
        "tired" => "疲惫",
        "angry" => "生气",
        "disappointed" => "失望",
        "surprised" => "惊讶",
        "curious" => "好奇",
        "neutral" => "平静",
        "bored" => "无聊",
        "confused" => "困惑",
        _ => "未知",
    }
}

/// 将 Vivian 感知偏见应用到情绪结果上
///
/// - 偏置幅度 < 0.01 视为无影响
/// - 偏置幅度 > 0.08 且原情绪为 neutral 时，按偏置正负迁移到 happy / sad
/// - intensity 加偏置后限制在 [0.0, 1.0]
fn apply_bias_to_result(result: EmotionResult, bias: &HashMap<String, f64>) -> EmotionResult {
    let b = bias.get(&result.emotion).copied().unwrap_or(0.0);
    if b.abs() < 0.01 {
        return result;
    }
    let adjusted_intensity = (result.intensity + b).clamp(0.0, 1.0);
    let biased_source = format!("{}_biased", result.source);

    if b.abs() > 0.08 && result.emotion == "neutral" {
        let shifted = if b > 0.0 { "happy" } else { "sad" };
        return EmotionResult {
            emotion: shifted.to_string(),
            intensity: adjusted_intensity,
            valence: result.valence,
            arousal: result.arousal,
            source: biased_source,
            ..Default::default()
        };
    }

    EmotionResult {
        emotion: result.emotion,
        intensity: adjusted_intensity,
        valence: result.valence,
        arousal: result.arousal,
        source: biased_source,
        ..Default::default()
    }
}

/// 生成薇薇安视角的情绪提示词
fn vivian_aware_prompt(emotion: &str, intensity: f64) -> String {
    match emotion {
        "happy" | "excited" | "grateful" => {
            format!("薇薇安感受到用户很开心（强度{:.2}），可以一起庆祝", intensity)
        }
        "sad" | "disappointed" => {
            format!("薇薇安感受到用户有些难过（强度{:.2}），需要温柔陪伴", intensity)
        }
        "angry" | "frustrated" => {
            format!("薇薇安感受到用户有些生气（强度{:.2}），需要耐心倾听", intensity)
        }
        "anxious" => {
            format!("薇薇安感受到用户有些焦虑（强度{:.2}），可以安抚鼓励", intensity)
        }
        "tired" | "bored" => {
            format!("薇薇安感受到用户有些疲惫（强度{:.2}），建议休息一下", intensity)
        }
        "surprised" => {
            format!("薇薇安感受到用户很惊讶（强度{:.2}），可以一起感叹", intensity)
        }
        "curious" => {
            format!("薇薇安感受到用户很好奇（强度{:.2}），可以一起探索", intensity)
        }
        "confused" => {
            format!("薇薇安感受到用户有些困惑（强度{:.2}），可以耐心解释", intensity)
        }
        _ => format!("薇薇安感受到用户情绪平静（强度{:.2}）", intensity),
    }
}

// ============== EmotionTracker ==============

/// 情感追踪器 - 记录情感历史并获取当前状态
///
/// 提供分析并记录、手动记录、趋势查询等功能。
pub struct EmotionTracker {
    history: Vec<EmotionResult>,
    max_history: usize,
}

impl EmotionTracker {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            max_history: 100,
        }
    }

    /// 记录一次情绪分析结果
    pub fn track(&mut self, emotion: &EmotionResult) {
        self.history.push(emotion.clone());
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// 手动记录一次情绪（无需重新分析）
    pub fn record(&mut self, emotion: &str, _text: &str, intensity: f64) {
        let result = EmotionResult {
            emotion: emotion.to_string(),
            intensity,
            valence: 0.0, // 手动记录时不计算 valence/arousal
            arousal: 0.0,
            source: "manual".to_string(),
            ..Default::default()
        };
        self.track(&result);
    }

    /// 分析文本情绪并记录到趋势追踪器
    pub async fn analyze_and_record(
        &mut self,
        analyzer: &EmotionAnalyzer,
        text: &str,
    ) -> EmotionResult {
        let result = analyzer.async_analyze(text).await;
        self.track(&result);
        result
    }

    pub fn get_current(&self) -> Option<EmotionResult> {
        self.history.last().cloned()
    }

    pub fn get_history(&self) -> &[EmotionResult] {
        &self.history
    }

    /// 获取所有历史记录
    pub fn get_all(&self) -> Vec<EmotionResult> {
        self.history.clone()
    }

    /// 获取最近 n 条记录
    pub fn get_recent(&self, n: usize) -> Vec<EmotionResult> {
        self.get_trend(n)
    }

    /// 统计历史中最常见的情绪，空历史返回 "neutral"
    pub fn get_dominant_emotion(&self) -> String {
        let mut counts: HashMap<&str, u32> = HashMap::new();
        for r in &self.history {
            *counts.entry(r.emotion.as_str()).or_insert(0) += 1;
        }
        counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(e, _)| e.to_string())
            .unwrap_or_else(|| "neutral".to_string())
    }

    /// 返回最近 n 条情绪记录（不足 n 条则返回全部，n 为 0 返回空）
    pub fn get_trend(&self, n: usize) -> Vec<EmotionResult> {
        let len = self.history.len();
        if len == 0 || n == 0 {
            return Vec::new();
        }
        let start = len.saturating_sub(n);
        self.history[start..].to_vec()
    }

    /// 生成情绪摘要，可注入 system prompt
    pub fn to_prompt_text(&self) -> String {
        if self.history.is_empty() {
            return "用户情绪：暂无记录".to_string();
        }
        let dominant = self.get_dominant_emotion();
        let recent = self.get_recent(3);
        let recent_desc: Vec<String> = recent
            .iter()
            .map(|r| format!("{}({:.2})", r.emotion, r.intensity))
            .collect();
        format!(
            "用户情绪：主导情绪为{}，最近变化：{}",
            dominant,
            recent_desc.join(" → ")
        )
    }

    /// 清空历史记录
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// 设置历史记录（用于恢复或测试）
    pub fn set_records(&mut self, records: Vec<EmotionResult>) {
        self.history = records;
        if self.history.len() > self.max_history {
            let drop_count = self.history.len() - self.max_history;
            self.history.drain(0..drop_count);
        }
    }
}

impl Default for EmotionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_happy() {
        let analyzer = EmotionAnalyzer::new();
        let result = analyzer.analyze("今天真的很开心，哈哈");
        assert_eq!(result.emotion, "happy");
        assert!(result.valence > 0.0);
        assert_eq!(result.source, "keyword");
    }

    #[test]
    fn test_analyze_neutral() {
        let analyzer = EmotionAnalyzer::new();
        let result = analyzer.analyze("今天天气不错");
        assert_eq!(result.emotion, "neutral");
        assert_eq!(result.source, "keyword");
    }

    #[test]
    fn test_analyze_sad() {
        let analyzer = EmotionAnalyzer::new();
        let result = analyzer.analyze("我好难过，想哭");
        assert_eq!(result.emotion, "sad");
        assert!(result.valence < 0.0);
    }

    #[test]
    fn test_analyze_angry() {
        let analyzer = EmotionAnalyzer::new();
        let result = analyzer.analyze("气死我了，烦死了");
        assert_eq!(result.emotion, "angry");
        assert!(result.valence < 0.0);
        assert!(result.arousal > 0.0);
    }

    #[test]
    fn test_tracker() {
        let mut tracker = EmotionTracker::new();
        assert!(tracker.get_current().is_none());
        tracker.track(&EmotionResult::default());
        assert!(tracker.get_current().is_some());
    }

    #[test]
    fn test_emotion_result_default_has_source() {
        let r = EmotionResult::default();
        assert_eq!(r.source, "keyword");
        assert_eq!(r.emotion, "neutral");
    }

    #[test]
    fn test_emotion_result_serde_with_source() {
        let r = EmotionResult {
            emotion: "happy".to_string(),
            intensity: 0.8,
            valence: 0.7,
            arousal: 0.5,
            source: "llm".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"source\":\"llm\""));
        // 反序列化
        let parsed: EmotionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source, "llm");
    }

    #[test]
    fn test_emotion_result_serde_backwards_compatible() {
        // 不含 source 字段的旧 JSON 应能反序列化（使用默认值）
        let old_json = r#"{"emotion":"neutral","intensity":0.0,"valence":0.0,"arousal":0.0}"#;
        let parsed: EmotionResult = serde_json::from_str(old_json).unwrap();
        assert_eq!(parsed.emotion, "neutral");
        assert_eq!(parsed.source, "keyword"); // 默认值
    }
}
