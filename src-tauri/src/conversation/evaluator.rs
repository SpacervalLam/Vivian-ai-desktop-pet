//! Conversation Evaluator —— 会话评估纯函数集。
//!
//! 把"该不该继续/能量/新鲜度/连续性"等评估逻辑从 ConversationManager
//! 拆出，让 Manager 只负责状态机写入与仓储，评估由纯函数完成。
//!
//! 设计原则：
//! - **纯函数**：所有评估函数只读输入，无副作用，不持锁
//! - **零外部依赖**：不依赖 ConversationManager 内部状态，只依赖 Conversation 数据
//! - **可测试**：每个函数可独立单元测试
//!
//! 与 Manager 的关系：
//! - Manager 的 `update_after_round` / `start_or_continue` 等方法在持锁后调用
//!   Evaluator 的纯函数计算指标，再根据指标写状态
//! - 关闭原因判断由 `IntentJudge::judge_close_reason`（LLM 驱动）完成，见
//!   `crate::dialogue::intent_judge`

use super::session::{Conversation, ResponseMode};

/// 计算新信息密度 [0, 1]
///
/// 纯规则（不调 LLM）：
/// - 含问号 → +0.3
/// - 长度 > 10 字 → +0.2
/// - 长度 > 30 字 → +0.2
/// - jieba 分词后实词数量 > 3 → +0.3
/// - 回复也较长 → +0.1
pub fn compute_novelty(user_input: &str, reply_text: Option<&str>) -> f64 {
    let mut score: f64 = 0.0;

    if user_input.contains('？') || user_input.contains('?') {
        score += 0.3;
    }

    let input_len = user_input.chars().count();
    if input_len > 10 {
        score += 0.2;
    }
    if input_len > 30 {
        score += 0.2;
    }

    // jieba 分词后的实词数量（长度 > 1 且非停用词）
    use jieba_rs::Jieba;
    let jieba = Jieba::new();
    let words = jieba.cut(user_input, true);
    let content_words = words
        .iter()
        .filter(|w| w.len() > 1)
        .filter(|w| !is_stopword(w))
        .count();
    if content_words > 3 {
        score += 0.3;
    }

    if let Some(reply) = reply_text {
        if reply.chars().count() > 15 {
            score += 0.1;
        }
    }

    score.min(1.0)
}

fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        "的" | "了"
            | "是"
            | "在"
            | "我"
            | "你"
            | "他"
            | "她"
            | "它"
            | "和"
            | "与"
            | "或"
            | "也"
            | "都"
            | "就"
            | "这"
            | "那"
            | "嗯"
            | "啊"
            | "哦"
            | "哈"
            | "好"
    )
}

/// 计算 Energy 增量
///
/// - Speak + Novelty 上升 → Energy 上升
/// - NonVerbal → Energy 中性（角色仍在参与，只是非语言回应）
/// - Internal → Energy 微降
/// - Ignore → Energy 大幅下降
pub fn compute_energy_delta(
    response_mode: ResponseMode,
    new_novelty: f64,
    old_novelty: f64,
) -> f64 {
    match response_mode {
        ResponseMode::Speak => {
            let novelty_delta = new_novelty - old_novelty;
            0.1 + novelty_delta * 0.3
        }
        ResponseMode::NonVerbal => 0.0,
        ResponseMode::Internal => -0.02,
        ResponseMode::Ignore => -0.3,
    }
}

/// 计算 Continuation Score
///
/// ```text
/// Continuation = Base
///              + NoveltyBonus (Novelty × 0.3)
///              + QuestionBonus (Novelty > 0.5 时 +0.2)
///              + EnergyBonus (Energy × 0.2)
///              - RoundPenalty (min(0.3, rounds × 0.02))
///              - LowEnergyPenalty (Energy < 0.3 时 -0.2)
/// ```
pub fn compute_continuation_score(conv: &Conversation) -> f64 {
    let mut score = 0.3;

    if conv.novelty > 0.5 {
        score += 0.2;
    }
    score += conv.novelty * 0.3;
    score += conv.energy * 0.2;

    let round_penalty = (conv.rounds as f64 * 0.02).min(0.3);
    score -= round_penalty;

    if conv.energy < 0.3 {
        score -= 0.2;
    }

    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_novelty_question_mark() {
        let score = compute_novelty("你在干嘛？", None);
        assert!(score >= 0.3);
    }

    #[test]
    fn test_energy_delta_speak() {
        let delta = compute_energy_delta(ResponseMode::Speak, 0.8, 0.5);
        assert!(delta > 0.1);
    }

    #[test]
    fn test_energy_delta_ignore() {
        let delta = compute_energy_delta(ResponseMode::Ignore, 0.5, 0.5);
        assert_eq!(delta, -0.3);
    }
}
