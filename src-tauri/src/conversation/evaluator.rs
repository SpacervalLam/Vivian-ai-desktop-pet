//! Conversation Evaluator —— 会话评估纯函数集。
//!
//! 把"该不该继续/为什么结束/能量/新鲜度/连续性"等评估逻辑从 ConversationManager
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
//! - 外部调用方（chat.rs / cross_character.rs）直接调 Evaluator 的 `detect_close_reason`

use super::session::{CloseReason, Conversation, ResponseMode};

/// 检测是否命中关闭关键词
///
/// 返回 `Some(reason)` 表示应立即关闭会话；
/// 返回 `None` 表示未命中，交由 Energy/Novelty/Continuation 状态机自然推进。
///
/// 规则优先级：GoodNight > GoodBye > Interrupted
pub fn detect_close_reason(text: &str) -> Option<CloseReason> {
    let lower = text.to_lowercase();
    let trimmed = text.trim();

    // 晚安关键词（中英）
    let goodnight_hits = [
        "晚安", "睡了", "睡觉了", "去睡了", "休息了", "去休息", "上床了",
        "good night", "goodnight", "gonna sleep", "going to bed", "bed time", "sleep tight",
    ];
    if goodnight_hits.iter().any(|k| lower.contains(k)) {
        return Some(CloseReason::GoodNight);
    }

    // 再见关键词（中英）
    let goodbye_hits = [
        "拜拜", "再见", "走了", "我先走了", "回头见", "下次聊", "回聊", "撤了", "下线了",
        "bye", "goodbye", "see you", "gotta go", "leaving", "catch you later", "talk later",
    ];
    if goodbye_hits.iter().any(|k| lower.contains(k)) {
        return Some(CloseReason::GoodBye);
    }

    // 中途打断关键词（用户表示暂时离开但意图回来）
    let interrupted_hits = [
        "等一下", "稍等", "我先忙", "老板电话", "电话来了", "接个电话", "有人找我",
        "等会再说", "待会回来", "马上回来", "brb", "hold on", "one sec", "hang on",
    ];
    if interrupted_hits.iter().any(|k| lower.contains(k)) {
        return Some(CloseReason::Interrupted);
    }

    // 兜底：空消息或极短消息不判定
    if trimmed.is_empty() {
        return None;
    }

    None
}

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
    fn test_detect_goodnight() {
        assert_eq!(detect_close_reason("晚安啦"), Some(CloseReason::GoodNight));
        assert_eq!(detect_close_reason("good night"), Some(CloseReason::GoodNight));
    }

    #[test]
    fn test_detect_goodbye() {
        assert_eq!(detect_close_reason("拜拜"), Some(CloseReason::GoodBye));
        assert_eq!(detect_close_reason("bye"), Some(CloseReason::GoodBye));
    }

    #[test]
    fn test_detect_interrupted() {
        assert_eq!(detect_close_reason("稍等一下"), Some(CloseReason::Interrupted));
        assert_eq!(detect_close_reason("brb"), Some(CloseReason::Interrupted));
    }

    #[test]
    fn test_detect_none() {
        assert_eq!(detect_close_reason("今天天气不错"), None);
        assert_eq!(detect_close_reason(""), None);
    }

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
