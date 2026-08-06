//! 心情流水线步骤：关键词情绪分析（fallback）。
//!
//! - [`MoodStep`]：从 `state.emotion` 或关键词匹配得到 emotion_score

use async_trait::async_trait;
use serde_json::Value;

use crate::error::VivianResult;
use crate::pipeline::state::PipelineState;
use crate::pipeline::base::{Runnable, RunnableConfig};

// ============================================================================
// MoodStep：关键词情绪分析（保留，作为 fallback）
// ============================================================================

pub struct MoodStep;

impl MoodStep {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MoodStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for MoodStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        if let Some(response) = state.ai_response.as_mut() {
            // ai_emotion 仅用于生成 emotion_score 供前端展示与记忆持久化。
            // expression 由 LLM 在 JSON 的 expression 字段直接给出，此处不再覆盖。
            let llm_emotion: Option<String> = state
                .emotion
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_lowercase());

            let score = match llm_emotion {
                Some(e) => emotion_to_score(&e),
                None => {
                    // LLM 未给出 ai_emotion，回退到关键词匹配
                    let text = response.text.to_lowercase();
                    let (_, _, score) = analyze_emotion(&text);
                    score
                }
            };
            response.emotion_score = score;
        }

        Ok(state.to_json())
    }
}

/// LLM 情绪标签映射到 emotion_score（[-1, 1]）
fn emotion_to_score(e: &str) -> f64 {
    match e {
        "happy" => 0.6,
        "sad" => -0.4,
        "angry" => -0.6,
        "anxious" => -0.3,
        "surprised" => 0.3,
        "shy" => 0.4,
        "calm" => 0.1,
        _ => 0.0,
    }
}

fn analyze_emotion(text: &str) -> (String, String, f64) {
    let positive = ["开心", "高兴", "快乐", "喜欢", "谢谢", "好的", "好呀", "当然", "happy", "love", "great", "nice"];
    let negative = ["难过", "伤心", "生气", "讨厌", "不好", "不行", "angry", "sad", "hate", "bad"];
    let surprised = ["惊讶", "哇", "真的", "居然", "wow", "really", "oh"];
    let calm = ["嗯", "好的吧", "知道了", "okay", "fine"];

    let positive_count = positive.iter().filter(|k| text.contains(*k)).count();
    let negative_count = negative.iter().filter(|k| text.contains(*k)).count();
    let surprised_count = surprised.iter().filter(|k| text.contains(*k)).count();
    let calm_count = calm.iter().filter(|k| text.contains(*k)).count();

    if positive_count >= negative_count && positive_count > 0 {
        let score = (0.6 + positive_count as f64 * 0.08).min(1.0);
        ("happy".to_string(), "star_eyes".to_string(), score)
    } else if negative_count > positive_count {
        let score = (-(0.4 + negative_count as f64 * 0.08)).max(-1.0);
        ("sad".to_string(), "cry".to_string(), score)
    } else if surprised_count > 0 {
        ("surprised".to_string(), "confused".to_string(), 0.3)
    } else if calm_count > 0 {
        ("calm".to_string(), "neutral".to_string(), 0.1)
    } else {
        ("neutral".to_string(), "neutral".to_string(), 0.0)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_emotion_positive() {
        let (emotion, expression, score) = analyze_emotion("今天好开心啊，哈哈");
        assert_eq!(emotion, "happy");
        assert!(score > 0.0);
        assert!(!expression.is_empty());
    }

    #[test]
    fn test_analyze_emotion_neutral() {
        let (emotion, _, _) = analyze_emotion("今天天气不错");
        assert_eq!(emotion, "neutral");
    }
}
