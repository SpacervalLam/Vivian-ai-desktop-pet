//! ResponseStrategy — 根据用户情绪推荐 AI 响应策略
//!
//! Rust 端采用 6 类策略
//! （Comfort/Encourage/Celebrate/Empathize/Redirect/Listen），
//! 并额外提供 `StrategyConfig`（语气/紧急度/共情程度/风格覆盖/建议动作）与
//! `apply_to_prompt` 用于将策略注入 LLM prompt。
//!
//! 14 类情绪 → 6 类策略映射：
//! - happy / excited         → Celebrate
//! - sad / disappointed      → Comfort
//! - anxious                 → Comfort
//! - grateful                → Listen
//! - frustrated / angry      → Empathize
//! - tired                   → Encourage
//! - bored                   → Redirect
//! - surprised / curious     → Listen
//! - confused                → Listen
//! - neutral                 → Listen

use serde::{Deserialize, Serialize};

use crate::psychology::EmotionLabel;
use super::EmotionResult;

/// 响应策略类型（6 类）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseStrategyType {
    /// 安慰 — 用于 sad/anxious/disappointed 等负面低唤醒情绪
    Comfort,
    /// 鼓励 — 用于 tired 等需要正向激励的情绪
    Encourage,
    /// 庆祝 — 用于 happy/excited 等积极高唤醒情绪
    Celebrate,
    /// 共情 — 用于 angry/frustrated 等需要先理解再回应的情绪
    Empathize,
    /// 转移话题 — 用于 bored 等需要换轨的情绪
    Redirect,
    /// 倾听 — 默认策略，用于 neutral/curious/grateful 等中性情绪
    Listen,
}

impl Default for ResponseStrategyType {
    fn default() -> Self {
        ResponseStrategyType::Listen
    }
}

impl ResponseStrategyType {
    /// 字符串标签
    pub fn as_str(&self) -> &'static str {
        match self {
            ResponseStrategyType::Comfort => "comfort",
            ResponseStrategyType::Encourage => "encourage",
            ResponseStrategyType::Celebrate => "celebrate",
            ResponseStrategyType::Empathize => "empathize",
            ResponseStrategyType::Redirect => "redirect",
            ResponseStrategyType::Listen => "listen",
        }
    }

    /// 由字符串标签解析；未知回退为 Listen
    pub fn from_str(label: &str) -> Self {
        match label {
            "comfort" => ResponseStrategyType::Comfort,
            "encourage" => ResponseStrategyType::Encourage,
            "celebrate" => ResponseStrategyType::Celebrate,
            "empathize" => ResponseStrategyType::Empathize,
            "redirect" => ResponseStrategyType::Redirect,
            "listen" => ResponseStrategyType::Listen,
            _ => ResponseStrategyType::Listen,
        }
    }

    /// 策略对应的中文标签
    pub fn as_label_cn(&self) -> &'static str {
        match self {
            ResponseStrategyType::Comfort => "安慰",
            ResponseStrategyType::Encourage => "鼓励",
            ResponseStrategyType::Celebrate => "庆祝",
            ResponseStrategyType::Empathize => "共情",
            ResponseStrategyType::Redirect => "转移话题",
            ResponseStrategyType::Listen => "倾听",
        }
    }

    /// 策略对应的 prompt 提示词片段（英文，用于注入 LLM system prompt）
    pub fn prompt_fragment(&self) -> &'static str {
        match self {
            ResponseStrategyType::Comfort => {
                "The user appears to be sad, anxious, or disappointed. Respond with comfort and warmth. \
                 Acknowledge their feelings first, then offer gentle reassurance. \
                 Do not minimize their emotions or rush to give solutions. \
                 Keep tone soft, supportive, and unhurried."
            }
            ResponseStrategyType::Encourage => {
                "The user seems tired or low on energy. Respond with encouragement. \
                 Recognize their effort, offer small actionable suggestions, \
                 and use a warm but not overly enthusiastic tone. \
                 Avoid toxic positivity — validate the difficulty first."
            }
            ResponseStrategyType::Celebrate => {
                "The user is happy or excited. Match their energy and celebrate with them! \
                 Use exclamation where appropriate, mirror their enthusiasm, \
                 and ask follow-up questions that let them share more of the good news."
            }
            ResponseStrategyType::Empathize => {
                "The user is angry or frustrated. First empathize without arguing or defending. \
                 Acknowledge their perspective ('That does sound frustrating...'). \
                 Do not contradict or correct them in this turn. \
                 Stay calm, neutral, and on their side."
            }
            ResponseStrategyType::Redirect => {
                "The user seems bored or stuck. Gently redirect the conversation \
                 to a fresh topic or a lighter angle. Offer a small interesting tidbit \
                 or ask an open-ended question to re-engage them."
            }
            ResponseStrategyType::Listen => {
                "The user is calm, curious, grateful, or neutral. Practice active listening. \
                 Ask clarifying questions, reflect back what they say, \
                 and let them lead the direction of the conversation."
            }
        }
    }
}

/// 用户情绪 → 基础策略（不考虑宠物情绪时的默认选择）
///
/// 14 类情绪全覆盖的策略映射：
/// - happy / excited         → Celebrate
/// - grateful                → Listen
/// - sad / disappointed      → Comfort
/// - frustrated / angry      → Empathize
/// - anxious                 → Comfort
/// - tired                   → Encourage
/// - bored                   → Redirect
/// - surprised / curious     → Listen
/// - confused                → Listen
/// - neutral                 → Listen
fn base_strategy_for_user_emotion(emotion: &str) -> ResponseStrategyType {
    match emotion {
        // 积极高唤醒 → 庆祝
        "happy" | "excited" => ResponseStrategyType::Celebrate,
        // 负面低唤醒（悲伤/失望） → 安慰
        "sad" | "disappointed" => ResponseStrategyType::Comfort,
        // 焦虑 → 安慰（提供安全感）
        "anxious" => ResponseStrategyType::Comfort,
        // 愤怒/沮丧 → 共情（不反驳）
        "angry" | "frustrated" => ResponseStrategyType::Empathize,
        // 疲惫 → 鼓励（温和、不强势）
        "tired" => ResponseStrategyType::Encourage,
        // 无聊 → 转移话题
        "bored" => ResponseStrategyType::Redirect,
        // 温暖积极/中性/好奇/惊讶/困惑 → 倾听
        "grateful" | "neutral" | "curious" | "surprised" | "confused" => {
            ResponseStrategyType::Listen
        }
        // 未知 → 倾听
        _ => ResponseStrategyType::Listen,
    }
}

/// 根据宠物情绪对基础策略做微调
///
/// 规则示例：
/// - 用户 sad + 宠物 Joy → 仍 comfort（避免欢快宠物显得冷漠）
/// - 用户 angry + 宠物 Anger → 升级为 comfort（避免共变愤怒）
/// - 用户 excited + 宠物 Joy → celebrate（同步兴奋）
/// - 用户 anxious + 宠物 Fear → comfort（提供安全感，避免共变焦虑）
fn adjust_strategy_for_pet(
    base: ResponseStrategyType,
    user_emotion: &str,
    pet_emotion: EmotionLabel,
) -> ResponseStrategyType {
    match (user_emotion, pet_emotion) {
        // 用户愤怒 + 宠物也愤怒：升级为 comfort，避免冲突
        ("angry", EmotionLabel::Anger) => ResponseStrategyType::Comfort,
        // 用户沮丧 + 宠物愤怒：升级为 comfort
        ("frustrated", EmotionLabel::Anger) => ResponseStrategyType::Comfort,
        // 用户焦虑 + 宠物恐惧：保持 comfort
        ("anxious", EmotionLabel::Fear) => ResponseStrategyType::Comfort,
        // 用户悲伤/失望 + 宠物欢快：保持 comfort（不要被宠物情绪带偏）
        ("sad", EmotionLabel::Joy) => ResponseStrategyType::Comfort,
        ("disappointed", EmotionLabel::Joy) => ResponseStrategyType::Comfort,
        // 用户兴奋 + 宠物喜悦：保持 celebrate
        ("excited", EmotionLabel::Joy) => ResponseStrategyType::Celebrate,
        // 用户开心 + 宠物喜悦：保持 celebrate
        ("happy", EmotionLabel::Joy) => ResponseStrategyType::Celebrate,
        // 用户无聊 + 宠物孤独/疲惫：升级为 redirect
        ("bored", EmotionLabel::Loneliness) => ResponseStrategyType::Redirect,
        // 其他情况保持基础策略
        _ => base,
    }
}

/// 响应策略选择器
///
/// - `recommend_strategy(user_emotion, pet_emotion)` 综合考虑双方情绪
/// - `strategy_prompt(strategy)` 返回可注入 prompt 的提示词
pub struct ResponseStrategy;

impl ResponseStrategy {
    /// 根据用户情绪 + 宠物情绪推荐响应策略
    ///
    /// 入参：
    /// - `user_emotion`: 用户情绪分析结果（14 类 LLM 情绪之一）
    /// - `pet_emotion`: Vivian 当前情绪（7 类 EmotionLabel 之一）
    pub fn recommend_strategy(
        user_emotion: &EmotionResult,
        pet_emotion: &EmotionLabel,
    ) -> ResponseStrategyType {
        let normalized = super::mapper::normalize_llm_emotion(&user_emotion.emotion);
        let base = base_strategy_for_user_emotion(normalized);
        adjust_strategy_for_pet(base, normalized, *pet_emotion)
    }

    /// 仅根据用户情绪标签推荐策略（不考虑宠物情绪）
    pub fn recommend_by_user_emotion(emotion: &str) -> ResponseStrategyType {
        base_strategy_for_user_emotion(super::mapper::normalize_llm_emotion(emotion))
    }

    /// 生成可注入 prompt 的策略指令
    pub fn strategy_prompt(strategy: ResponseStrategyType) -> &'static str {
        strategy.prompt_fragment()
    }

    /// 生成完整的策略提示词块（含策略标签和说明），可直接拼到 system prompt
    pub fn build_prompt_block(
        user_emotion: &EmotionResult,
        pet_emotion: &EmotionLabel,
    ) -> String {
        let strategy = Self::recommend_strategy(user_emotion, pet_emotion);
        let user_emo = super::mapper::normalize_llm_emotion(&user_emotion.emotion);
        let pet_emo = pet_emotion.as_str();
        format!(
            "[Response Strategy]\n\
             User emotion: {user_emo} (intensity={intensity:.2})\n\
             Vivian emotion: {pet_emo}\n\
             Strategy: {strategy_label} ({strategy_cn})\n\
             Guideline: {guideline}",
            intensity = user_emotion.intensity,
            strategy_label = strategy.as_str(),
            strategy_cn = strategy.as_label_cn(),
            guideline = strategy.prompt_fragment(),
        )
    }
}

/// 响应策略推荐结果（带上下文，便于上层使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseStrategyRecommendation {
    pub strategy: ResponseStrategyType,
    pub user_emotion: String,
    pub pet_emotion: String,
    pub prompt_fragment: String,
    /// 是否因宠物情绪而调整过基础策略
    pub adjusted_by_pet: bool,
}

impl ResponseStrategy {
    /// 详细推荐：返回完整结果（含是否调整标记）
    pub fn recommend_detailed(
        user_emotion: &EmotionResult,
        pet_emotion: &EmotionLabel,
    ) -> ResponseStrategyRecommendation {
        let normalized = super::mapper::normalize_llm_emotion(&user_emotion.emotion);
        let base = base_strategy_for_user_emotion(normalized);
        let final_strategy = adjust_strategy_for_pet(base, normalized, *pet_emotion);
        ResponseStrategyRecommendation {
            strategy: final_strategy,
            user_emotion: normalized.to_string(),
            pet_emotion: pet_emotion.as_str().to_string(),
            prompt_fragment: final_strategy.prompt_fragment().to_string(),
            adjusted_by_pet: base != final_strategy,
        }
    }
}

// ============== StrategyConfig（任务规范要求）==============
//
// 在 6 类策略基础上提供更细粒度的配置：语气、紧急度、共情程度、风格覆盖、建议动作。
// 14 类情绪各有对应策略（通过 base_strategy_for_user_emotion 间接映射）。

/// 响应策略配置 — 提供细粒度的语气与行为参数
///
/// 对应任务规范 `StrategyConfig`：
/// - tone：语气描述
/// - urgency：紧急度（0.0 ~ 1.0，越高越需要快速回应）
/// - empathy_level：共情程度（0.0 ~ 1.0，越高越强调情感共鸣）
/// - style_overrides：风格覆盖指令
/// - suggested_actions：建议动作
/// - prompt_fragment：可注入 prompt 的策略指令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// 策略类型
    pub strategy: ResponseStrategyType,
    /// 语气描述
    pub tone: String,
    /// 紧急度（0.0 ~ 1.0）
    pub urgency: f64,
    /// 共情程度（0.0 ~ 1.0）
    pub empathy_level: f64,
    /// 风格覆盖指令
    pub style_overrides: Vec<String>,
    /// 建议动作
    pub suggested_actions: Vec<String>,
    /// 可注入 prompt 的策略指令片段
    pub prompt_fragment: String,
}

impl StrategyConfig {
    /// 由策略类型构造默认配置
    pub fn from_strategy(strategy: ResponseStrategyType) -> Self {
        let (tone, urgency, empathy, style_overrides, suggested_actions) = match strategy {
            ResponseStrategyType::Celebrate => (
                "热情活泼",
                0.3,
                0.5,
                vec![
                    "用感叹句匹配用户能量".to_string(),
                    "积极追问好细节".to_string(),
                ],
                vec!["庆祝".to_string(), "追问细节".to_string()],
            ),
            ResponseStrategyType::Comfort => (
                "温柔安抚",
                0.6,
                0.9,
                vec![
                    "语气柔软、不催促".to_string(),
                    "先共情再回应".to_string(),
                ],
                vec!["共情".to_string(), "提供安全感".to_string()],
            ),
            ResponseStrategyType::Encourage => (
                "温暖鼓励",
                0.4,
                0.7,
                vec![
                    "避免毒性积极".to_string(),
                    "认可用户努力".to_string(),
                ],
                vec!["认可努力".to_string(), "给出小步建议".to_string()],
            ),
            ResponseStrategyType::Empathize => (
                "冷静中立",
                0.5,
                0.8,
                vec![
                    "不反驳、不辩解".to_string(),
                    "先理解对方立场".to_string(),
                ],
                vec!["共情对方".to_string(), "站到用户一边".to_string()],
            ),
            ResponseStrategyType::Redirect => (
                "轻松活泼",
                0.3,
                0.5,
                vec![
                    "自然换话题".to_string(),
                    "提供新鲜感".to_string(),
                ],
                vec!["转移话题".to_string(), "分享趣闻".to_string()],
            ),
            ResponseStrategyType::Listen => (
                "自然对话",
                0.3,
                0.6,
                vec![
                    "主动倾听".to_string(),
                    "提问澄清".to_string(),
                ],
                vec!["倾听".to_string(), "追问澄清".to_string()],
            ),
        };
        Self {
            strategy,
            tone: tone.to_string(),
            urgency,
            empathy_level: empathy,
            style_overrides,
            suggested_actions,
            prompt_fragment: strategy.prompt_fragment().to_string(),
        }
    }
}

impl ResponseStrategy {
    /// 根据用户情绪（14 类 LLM 情绪之一）获取策略配置
    ///
    /// 对应任务规范 `get_strategy(emotion) -> StrategyConfig`。
    pub fn get_strategy(emotion: &str) -> StrategyConfig {
        let normalized = super::mapper::normalize_llm_emotion(emotion);
        let strategy = base_strategy_for_user_emotion(normalized);
        StrategyConfig::from_strategy(strategy)
    }

    /// 根据用户情绪 + 宠物情绪获取策略配置（含宠物调整）
    pub fn get_strategy_with_pet(
        user_emotion: &EmotionResult,
        pet_emotion: &EmotionLabel,
    ) -> StrategyConfig {
        let normalized = super::mapper::normalize_llm_emotion(&user_emotion.emotion);
        let base = base_strategy_for_user_emotion(normalized);
        let final_strategy = adjust_strategy_for_pet(base, normalized, *pet_emotion);
        StrategyConfig::from_strategy(final_strategy)
    }

    /// 将策略配置注入到 prompt 中
    ///
    /// 对应任务规范 `apply_to_prompt(prompt, strategy) -> String`。
    /// 在原 prompt 末尾追加策略指令块。
    pub fn apply_to_prompt(prompt: &str, strategy: &StrategyConfig) -> String {
        let style = if strategy.style_overrides.is_empty() {
            String::from("（无）")
        } else {
            strategy.style_overrides.join("；")
        };
        let actions = if strategy.suggested_actions.is_empty() {
            String::from("（无）")
        } else {
            strategy.suggested_actions.join("；")
        };
        format!(
            "{prompt}\n\n\
             [ResponseStrategy/{strategy_label}]\n\
             Tone: {tone}\n\
             Urgency: {urgency:.2}\n\
             Empathy: {empathy:.2}\n\
             Style overrides: {style}\n\
             Suggested actions: {actions}\n\
             Guideline: {guideline}",
            prompt = prompt,
            strategy_label = strategy.strategy.as_str(),
            tone = strategy.tone,
            urgency = strategy.urgency,
            empathy = strategy.empathy_level,
            style = style,
            actions = actions,
            guideline = strategy.prompt_fragment,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::mapper::{LlmEmotion, LLM_EMOTION_LABELS};

    fn make_result(emotion: &str, intensity: f64) -> EmotionResult {
        EmotionResult {
            emotion: emotion.to_string(),
            intensity,
            valence: 0.0,
            arousal: 0.0,
            source: "test".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_strategy_type_roundtrip() {
        for s in [
            ResponseStrategyType::Comfort,
            ResponseStrategyType::Encourage,
            ResponseStrategyType::Celebrate,
            ResponseStrategyType::Empathize,
            ResponseStrategyType::Redirect,
            ResponseStrategyType::Listen,
        ] {
            assert_eq!(ResponseStrategyType::from_str(s.as_str()), s);
        }
        assert_eq!(
            ResponseStrategyType::from_str("unknown"),
            ResponseStrategyType::Listen
        );
    }

    #[test]
    fn test_strategy_prompt_nonempty() {
        for s in [
            ResponseStrategyType::Comfort,
            ResponseStrategyType::Encourage,
            ResponseStrategyType::Celebrate,
            ResponseStrategyType::Empathize,
            ResponseStrategyType::Redirect,
            ResponseStrategyType::Listen,
        ] {
            assert!(!s.prompt_fragment().is_empty());
        }
    }

    #[test]
    fn test_recommend_by_user_emotion_covers_all_14_python_emotions() {
        // happy / excited → Celebrate
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("happy"),
            ResponseStrategyType::Celebrate
        );
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("excited"),
            ResponseStrategyType::Celebrate
        );
        // grateful → Listen
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("grateful"),
            ResponseStrategyType::Listen
        );
        // sad → Comfort
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("sad"),
            ResponseStrategyType::Comfort
        );
        // frustrated → Empathize
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("frustrated"),
            ResponseStrategyType::Empathize
        );
        // anxious → Comfort
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("anxious"),
            ResponseStrategyType::Comfort
        );
        // tired → Encourage
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("tired"),
            ResponseStrategyType::Encourage
        );
        // angry → Empathize
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("angry"),
            ResponseStrategyType::Empathize
        );
        // disappointed → Comfort
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("disappointed"),
            ResponseStrategyType::Comfort
        );
        // surprised → Listen
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("surprised"),
            ResponseStrategyType::Listen
        );
        // curious → Listen
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("curious"),
            ResponseStrategyType::Listen
        );
        // neutral → Listen
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("neutral"),
            ResponseStrategyType::Listen
        );
        // bored → Redirect
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("bored"),
            ResponseStrategyType::Redirect
        );
        // confused → Listen
        assert_eq!(
            ResponseStrategy::recommend_by_user_emotion("confused"),
            ResponseStrategyType::Listen
        );
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_angry_angry() {
        // 用户 angry + 宠物 Anger → 升级为 Comfort（避免共变愤怒）
        let user = make_result("angry", 0.8);
        let pet = EmotionLabel::Anger;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Comfort);

        // 用户 angry + 宠物 Curiosity → 保持 Empathize
        let pet_calm = EmotionLabel::Curiosity;
        let strategy2 = ResponseStrategy::recommend_strategy(&user, &pet_calm);
        assert_eq!(strategy2, ResponseStrategyType::Empathize);
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_excited_joy() {
        // 用户 excited + 宠物 Joy → 保持 Celebrate
        let user = make_result("excited", 0.9);
        let pet = EmotionLabel::Joy;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Celebrate);

        // 用户 excited + 宠物 Sadness → 仍 Celebrate（基础策略不被宠物拉低）
        let pet_sad = EmotionLabel::Sadness;
        let strategy2 = ResponseStrategy::recommend_strategy(&user, &pet_sad);
        assert_eq!(strategy2, ResponseStrategyType::Celebrate);
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_sad_joy() {
        // 用户 sad + 宠物 Joy → 保持 Comfort（不被宠物带偏）
        let user = make_result("sad", 0.7);
        let pet = EmotionLabel::Joy;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Comfort);
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_anxious_fear() {
        // 用户 anxious + 宠物 Fear → Comfort（提供安全感）
        let user = make_result("anxious", 0.6);
        let pet = EmotionLabel::Fear;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Comfort);
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_disappointed_joy() {
        // 用户 disappointed + 宠物 Joy → 仍 Comfort（不被带偏）
        let user = make_result("disappointed", 0.6);
        let pet = EmotionLabel::Joy;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Comfort);
    }

    #[test]
    fn test_recommend_strategy_pet_adjustment_frustrated_anger() {
        // 用户 frustrated + 宠物 Anger → 升级为 Comfort
        let user = make_result("frustrated", 0.7);
        let pet = EmotionLabel::Anger;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Comfort);

        // 用户 frustrated + 宠物 Curiosity → 保持 Empathize
        let strategy2 = ResponseStrategy::recommend_strategy(&user, &EmotionLabel::Curiosity);
        assert_eq!(strategy2, ResponseStrategyType::Empathize);
    }

    #[test]
    fn test_recommend_strategy_unknown_emotion() {
        // 未知情绪标签 → normalize 为 neutral → Listen
        let user = make_result("??unknown??", 0.3);
        let pet = EmotionLabel::Curiosity;
        let strategy = ResponseStrategy::recommend_strategy(&user, &pet);
        assert_eq!(strategy, ResponseStrategyType::Listen);
    }

    #[test]
    fn test_build_prompt_block_contains_strategy_label() {
        let user = make_result("sad", 0.7);
        let pet = EmotionLabel::Curiosity;
        let block = ResponseStrategy::build_prompt_block(&user, &pet);
        assert!(block.contains("Response Strategy"));
        assert!(block.contains("comfort"));
        assert!(block.contains("User emotion: sad"));
        assert!(block.contains("Vivian emotion: curiosity"));
    }

    #[test]
    fn test_recommend_detailed_marks_adjustment() {
        // 用户 angry + 宠物 Anger → 调整为 Comfort，应标记 adjusted_by_pet=true
        let user = make_result("angry", 0.8);
        let pet = EmotionLabel::Anger;
        let detail = ResponseStrategy::recommend_detailed(&user, &pet);
        assert_eq!(detail.strategy, ResponseStrategyType::Comfort);
        assert!(detail.adjusted_by_pet);

        // 用户 angry + 宠物 Curiosity → 不调整
        let pet2 = EmotionLabel::Curiosity;
        let detail2 = ResponseStrategy::recommend_detailed(&user, &pet2);
        assert_eq!(detail2.strategy, ResponseStrategyType::Empathize);
        assert!(!detail2.adjusted_by_pet);
    }

    #[test]
    fn test_strategy_for_all_14_llm_emotions() {
        // 确保所有 14 类 LLM 情绪都有策略映射（不 panic）
        for &label in LLM_EMOTION_LABELS {
            let _ = LlmEmotion::from_label(label);
            let strategy = ResponseStrategy::recommend_by_user_emotion(label);
            // 策略 prompt 片段必须非空
            assert!(!ResponseStrategy::strategy_prompt(strategy).is_empty());
        }
    }

    #[test]
    fn test_get_strategy_returns_config_for_each_emotion() {
        // 14 类情绪各有对应 StrategyConfig
        for &label in LLM_EMOTION_LABELS {
            let config = ResponseStrategy::get_strategy(label);
            assert!(!config.tone.is_empty());
            assert!(config.urgency >= 0.0 && config.urgency <= 1.0);
            assert!(config.empathy_level >= 0.0 && config.empathy_level <= 1.0);
            assert!(!config.prompt_fragment.is_empty());
        }
    }

    #[test]
    fn test_get_strategy_specific_mappings() {
        // 抽样核对几个关键情绪的策略配置
        let happy_cfg = ResponseStrategy::get_strategy("happy");
        assert_eq!(happy_cfg.strategy, ResponseStrategyType::Celebrate);
        assert!(!happy_cfg.suggested_actions.is_empty());

        let sad_cfg = ResponseStrategy::get_strategy("sad");
        assert_eq!(sad_cfg.strategy, ResponseStrategyType::Comfort);
        assert!(sad_cfg.empathy_level > 0.8);

        let bored_cfg = ResponseStrategy::get_strategy("bored");
        assert_eq!(bored_cfg.strategy, ResponseStrategyType::Redirect);
    }

    #[test]
    fn test_apply_to_prompt_appends_strategy_block() {
        let config = ResponseStrategy::get_strategy("sad");
        let original = "Please respond to the user.";
        let enriched = ResponseStrategy::apply_to_prompt(original, &config);
        assert!(enriched.starts_with(original));
        assert!(enriched.contains("ResponseStrategy/"));
        assert!(enriched.contains("Tone:"));
        assert!(enriched.contains("Urgency:"));
        assert!(enriched.contains("Empathy:"));
        assert!(enriched.contains("Style overrides:"));
        assert!(enriched.contains("Suggested actions:"));
        assert!(enriched.contains("Guideline:"));
    }

    #[test]
    fn test_apply_to_prompt_with_empty_overrides() {
        // 自定义一个空 style_overrides 的配置，验证不 panic
        let mut config = ResponseStrategy::get_strategy("neutral");
        config.style_overrides.clear();
        config.suggested_actions.clear();
        let enriched = ResponseStrategy::apply_to_prompt("hi", &config);
        assert!(enriched.contains("（无）"));
    }
}
