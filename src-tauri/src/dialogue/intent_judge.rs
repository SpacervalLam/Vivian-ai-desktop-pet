//! 意图判断 — 规则预检 + LLM 驱动的意图分类 / 自然结束判断
//!
//! 简单输入（如"嗯"/"好"/"拜拜"）由规则预检直接判定，跳过 LLM 调用。
//! 规则未覆盖的语义判断由 LLM 完成。

use std::sync::Arc;
use std::time::Duration;

use crate::error::VivianResult;

// ===== 公共数据类型 =====

/// 回复类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyType {
    /// 用户提问、分享新信息、主动延续话题
    Continue,
    /// 简短附和、单词短语、话题渐弱
    ShortReply,
    /// 用户明显结束话题，强行回复会显得突兀
    NoReply,
}

impl Default for ReplyType {
    fn default() -> Self {
        Self::Continue
    }
}

/// 意图判断结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct IntentResult {
    /// 回复类型
    pub reply_type: ReplyType,
    /// 是否应该响应：`None` = 跳过，`Some(false)` = 简短回应，`Some(true)` = 完整响应
    pub should_respond: Option<bool>,
    /// 是否自然结束
    pub should_end: bool,
    /// 结束原因
    pub end_reason: String,
    /// 结束置信度 0.0~1.0
    pub confidence: f64,
}

impl IntentResult {
    /// 异常兜底默认响应（完整响应、不结束）
    fn fallback() -> Self {
        Self {
            reply_type: ReplyType::Continue,
            should_respond: Some(true),
            should_end: false,
            end_reason: String::new(),
            confidence: 0.0,
        }
    }

    /// 无 LLM 提供方时的默认结果（完整响应、不结束）
    fn no_llm() -> Self {
        Self {
            reply_type: ReplyType::Continue,
            should_respond: Some(true),
            should_end: false,
            end_reason: String::new(),
            confidence: 0.0,
        }
    }
}

// ===== LLM 回调 =====

/// LLM 判断回调类型：接收 prompt，返回 LLM 响应文本
pub type LlmJudgeFn = Arc<dyn Fn(&str) -> VivianResult<String> + Send + Sync>;

// ===== 意图判断器 =====

/// 自然结束置信度阈值（≥ 则判定为应结束）
const END_CONFIDENCE_THRESHOLD: f64 = 0.30;

/// 送入 prompt 的最大历史消息条数
const MAX_HISTORY_MSGS: usize = 6;

/// 单条历史消息截断长度
const HISTORY_MSG_TRUNCATE: usize = 150;

/// LLM 判断超时（秒）
const JUDGE_TIMEOUT_SECS: u64 = 8;

/// 意图判断器 — 纯 LLM 驱动
///
/// 单次 LLM 调用同时判断两件事：
/// 1. 回复类型（continue / short_reply / no_reply）
/// 2. 是否自然结束（end_reason + confidence，阈值 0.30）
pub struct IntentJudge {
    llm_judge_fn: Option<LlmJudgeFn>,
}

impl IntentJudge {
    /// 创建新实例
    pub fn new(llm_judge_fn: Option<LlmJudgeFn>) -> Self {
        Self { llm_judge_fn }
    }

    /// 后绑定 LLM 判断回调
    pub fn set_llm_judge_fn(&mut self, llm_judge_fn: LlmJudgeFn) {
        self.llm_judge_fn = Some(llm_judge_fn);
    }

    /// 合并判断（回复意图 + 自然结束），单次 LLM 调用
    ///
    /// `text` 用户最新消息，`history` 近期对话轮次（每项为单条文本）。
    /// 异常时返回兜底默认响应（完整响应、不结束）。
    pub fn judge(&self, text: &str, history: &[String]) -> IntentResult {
        // 规则预检：简单输入直接判定，跳过 LLM 调用
        if let Some(rule_result) = Self::rule_based_check(text) {
            return rule_result;
        }

        let llm_fn = match &self.llm_judge_fn {
            Some(f) => f.clone(),
            None => return IntentResult::no_llm(),
        };

        let history_text = Self::build_history_text(history);
        let prompt = Self::build_prompt(text, &history_text);

        // 注：LLM 调用通过 llm_fn 回调实现；超时保护见 judge_with_end_check
        match llm_fn(&prompt) {
            Ok(result) => Self::parse_judge_response(&result),
            Err(e) => {
                tracing::error!("[IntentJudge] 合并 LLM 判断异常，兜底默认响应: {}", e);
                IntentResult::fallback()
            }
        }
    }

    /// 异步合并判断（带超时保护）
    ///
    /// 单次 LLM 调用同时判断回复意图和是否自然结束，超时返回兜底默认响应。
    pub async fn judge_with_end_check(
        &self,
        text: &str,
        history: &[String],
    ) -> IntentResult {
        let llm_fn = match &self.llm_judge_fn {
            Some(f) => f.clone(),
            None => return IntentResult::no_llm(),
        };

        let history_text = Self::build_history_text(history);
        let prompt = Self::build_prompt(text, &history_text);

        // 用 spawn_blocking 包装同步 LLM 调用，加超时保护
        let timeout_result = tokio::time::timeout(
            Duration::from_secs(JUDGE_TIMEOUT_SECS),
            tokio::task::spawn_blocking(move || llm_fn(&prompt)),
        )
        .await;

        match timeout_result {
            Ok(Ok(Ok(result))) => Self::parse_judge_response(&result),
            Ok(Ok(Err(e))) => {
                tracing::error!(
                    "[IntentJudge] 合并 LLM 判断异常，兜底默认响应: {}",
                    e
                );
                IntentResult::fallback()
            }
            Ok(Err(e)) => {
                tracing::error!("[IntentJudge] spawn_blocking 异常: {}", e);
                IntentResult::fallback()
            }
            Err(_) => {
                tracing::warn!(
                    "[IntentJudge] LLM 判断超时 ({}s)",
                    JUDGE_TIMEOUT_SECS
                );
                IntentResult::fallback()
            }
        }
    }

    // ===== 内部辅助 =====

    /// 规则预检：对简单输入直接判定，避免不必要的 LLM 调用。
    /// 返回 `None` 表示规则未覆盖，需走 LLM 判断。
    fn rule_based_check(text: &str) -> Option<IntentResult> {
        let trimmed = text.trim();
        let chars: Vec<char> = trimmed.chars().collect();

        // 空输入 → 不回复
        if chars.is_empty() {
            return Some(IntentResult {
                reply_type: ReplyType::NoReply,
                should_respond: None,
                should_end: false,
                end_reason: String::new(),
                confidence: 0.0,
            });
        }

        // 极短附和（≤3 字符）→ 简短回应
        if chars.len() <= 3 {
            let lower = trimmed.to_lowercase();
            let short_ack = matches!(
                lower.as_str(),
                "嗯" | "嗯嗯" | "嗯嗯嗯" | "好" | "好的" | "哦" | "ok" | "okay"
                | "好哒" | "收到" | "了解" | "明白" | "知道" | "行" | "行吧"
                | "嗯。" | "嗯！" | "好。" | "好！" | "哦。" | "哦哦"
                | "うん" | "はい" | "いいよ" | "わかった" | "yeah"
            );
            if short_ack {
                return Some(IntentResult {
                    reply_type: ReplyType::ShortReply,
                    should_respond: Some(false),
                    should_end: false,
                    end_reason: String::new(),
                    confidence: 0.0,
                });
            }
        }

        // 明确结束词 → 不回复 + 高置信度结束
        let lower = trimmed.to_lowercase();
        let end_signals = [
            "拜拜", "晚安", "先这样", "去忙了", "先去了", "回头聊", "下次聊",
            "走了", "下了", "先下了", "那我先", "不聊了", "休息了", "睡觉了",
            "bye", "goodbye", "goodnight", "gnight", "see you", "later",
            "バイバイ", "おやすみ", "またね", "じゃあね",
        ];
        for sig in &end_signals {
            if lower.contains(sig) {
                return Some(IntentResult {
                    reply_type: ReplyType::NoReply,
                    should_respond: None,
                    should_end: true,
                    end_reason: "topic_concluded".to_string(),
                    confidence: 0.85,
                });
            }
        }

        None
    }

    /// 构造历史文本块
    ///
    /// 由于 history 每项为纯文本无法判断说话者，按奇偶位置推断：
    /// 第 0 条为 User，第 1 条为 AI（我），交替。
    fn build_history_text(history: &[String]) -> String {
        if history.is_empty() {
            return "(no history)".to_string();
        }
        let max_msgs = MAX_HISTORY_MSGS.min(history.len());
        let tail = &history[history.len() - max_msgs..];
        tail.iter()
            .enumerate()
            .map(|(i, msg)| {
                let tag = if i % 2 == 0 {
                    "[User says to me]"
                } else {
                    "[I say to User]"
                };
                let truncated: String = msg.chars().take(HISTORY_MSG_TRUNCATE).collect();
                if msg.chars().count() > HISTORY_MSG_TRUNCATE {
                    format!("{} {}…", tag, truncated)
                } else {
                    format!("{} {}", tag, truncated)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 构造合并判断 prompt
    fn build_prompt(user_message: &str, history_text: &str) -> String {
        let lang_norm =
            crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        match lang_norm {
            "en" => format!(
                "Analyze the user's latest message and recent conversation. Answer TWO questions.\n\n\
                 Recent conversation:\n\
                 {history_text}\n\n\
                 User's latest message: {user_message}\n\n\
                 === Question 1: Reply type ===\n\
                 Choose one:\n\
                 - continue: user asks questions, shares new info, actively continues topic\n\
                 - short_reply: brief acknowledgment, single word/phrase, topic winding down\n\
                 - no_reply: user clearly ended topic, replying would feel forced\n\n\
                 === Question 2: Should conversation end? ===\n\
                 Consider: topic reached natural conclusion? user indicating done? conversation fading?\n\
                 Would continuing feel forced/repetitive? Any active engagement or new topics?\n\n\
                 Respond in this exact format (two lines):\n\n\
                 Line 1 — reply type: continue / short_reply / no_reply\n\
                 Line 2 — end decision: end_reason | confidence  OR  none | 0.0\n\n\
                 Examples:\n\
                 continue\n\
                 none | 0.0\n\n\
                 short_reply\n\
                 conversation_fading | 0.65\n\n\
                 no_reply\n\
                 topic_concluded | 0.85\n\n\
                 Only two lines, nothing else."
            ),
            "ja" => format!(
                "ユーザーの最新メッセージと最近の会話を分析し、2つの質問に答えてください。\n\n\
                 最近の会話：\n\
                 {history_text}\n\n\
                 ユーザーの最新メッセージ：{user_message}\n\n\
                 === 質問 1：返信タイプ ===\n\
                 いずれかを選択：\n\
                 - continue：ユーザーが質問、新情報を共有、話題を積極的に続ける\n\
                 - short_reply：簡潔な相槌、単語・フレーズのみ、話題が途切れかけている\n\
                 - no_reply：ユーザーが明確に話題を終了、返信は不自然に感じられる\n\n\
                 === 質問 2：会話を終了すべきか？ ===\n\
                 考慮：話題は自然に着地した？ユーザーは終了を示唆？会話がフェードアウト？\n\
                 続けると不自然/繰り返しに感じる？能動的な関与や新しい話題はある？\n\n\
                 以下の厳密な形式で回答してください（2行）：\n\n\
                 行 1 — 返信タイプ：continue / short_reply / no_reply\n\
                 行 2 — 終了判定：end_reason | confidence  または  none | 0.0\n\n\
                 例：\n\
                 continue\n\
                 none | 0.0\n\n\
                 short_reply\n\
                 conversation_fading | 0.65\n\n\
                 no_reply\n\
                 topic_concluded | 0.85\n\n\
                 2行のみ、それ以外は出力しない。"
            ),
            _ => format!(
                "分析用户的最新消息和近期对话，回答两个问题。\n\n\
                 近期对话：\n\
                 {history_text}\n\n\
                 用户最新消息：{user_message}\n\n\
                 === 问题 1：回复类型 ===\n\
                 选择其一：\n\
                 - continue：用户提问、分享新信息、主动延续话题\n\
                 - short_reply：简短附和、单词短语、话题渐弱\n\
                 - no_reply：用户明显结束话题，强行回复会显得突兀\n\n\
                 === 问题 2：对话是否应结束？ ===\n\
                 考虑：话题是否自然结束？用户是否表示完成？对话是否渐弱？\n\
                 继续会显得突兀/重复吗？是否有主动参与或新话题？\n\n\
                 请按以下精确格式回答（两行）：\n\n\
                 第 1 行 — 回复类型：continue / short_reply / no_reply\n\
                 第 2 行 — 结束决策：end_reason | confidence  或  none | 0.0\n\n\
                 示例：\n\
                 continue\n\
                 none | 0.0\n\n\
                 short_reply\n\
                 conversation_fading | 0.65\n\n\
                 no_reply\n\
                 topic_concluded | 0.85\n\n\
                 仅两行，无其他内容。"
            ),
        }
    }

    /// 解析 LLM 两行响应为 IntentResult
    fn parse_judge_response(result: &str) -> IntentResult {
        let lines: Vec<&str> = result
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        // 解析 Line 1: 回复类型
        let reply_type = if let Some(first) = lines.first() {
            let lower = first.to_lowercase();
            if lower.contains("no_reply") {
                ReplyType::NoReply
            } else if lower.contains("short_reply") {
                ReplyType::ShortReply
            } else {
                ReplyType::Continue
            }
        } else {
            ReplyType::Continue
        };

        // 解析 Line 2: 结束判断
        let mut should_end = false;
        let mut end_reason = String::new();
        let mut confidence = 0.0;
        if lines.len() >= 2 {
            let parts: Vec<&str> = lines[1].split('|').collect();
            if parts.len() == 2 {
                let reason = parts[0].trim();
                if reason.to_lowercase() != "none" {
                    if let Ok(conf) = parts[1].trim().parse::<f64>() {
                        if conf >= END_CONFIDENCE_THRESHOLD {
                            should_end = true;
                            end_reason = reason.to_string();
                            confidence = (conf * 100.0).round() / 100.0;
                        }
                    }
                }
            }
        }

        // 映射 reply_type → should_respond
        let should_respond = match reply_type {
            ReplyType::NoReply => None,
            ReplyType::ShortReply => Some(false),
            ReplyType::Continue => Some(true),
        };

        if should_end {
            tracing::info!(
                "[IntentJudge] 合并判断结果: reply={:?}, end={}({})",
                reply_type,
                end_reason,
                confidence
            );
        }

        IntentResult {
            reply_type,
            should_respond,
            should_end,
            end_reason,
            confidence,
        }
    }
}

impl Default for IntentJudge {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::VivianError;

    #[test]
    fn parse_continue_no_end() {
        let r = IntentJudge::parse_judge_response("continue\nnone | 0.0");
        assert_eq!(r.reply_type, ReplyType::Continue);
        assert_eq!(r.should_respond, Some(true));
        assert!(!r.should_end);
    }

    #[test]
    fn parse_short_reply_with_fading() {
        let r = IntentJudge::parse_judge_response("short_reply\nconversation_fading | 0.65");
        assert_eq!(r.reply_type, ReplyType::ShortReply);
        assert_eq!(r.should_respond, Some(false));
        assert!(r.should_end);
        assert_eq!(r.end_reason, "conversation_fading");
        assert!((r.confidence - 0.65).abs() < 1e-9);
    }

    #[test]
    fn parse_no_reply_with_concluded() {
        let r = IntentJudge::parse_judge_response("no_reply\ntopic_concluded | 0.85");
        assert_eq!(r.reply_type, ReplyType::NoReply);
        assert_eq!(r.should_respond, None);
        assert!(r.should_end);
    }

    #[test]
    fn parse_below_threshold_does_not_end() {
        let r = IntentJudge::parse_judge_response("short_reply\nconversation_fading | 0.20");
        assert!(!r.should_end);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn judge_no_llm_returns_default() {
        let j = IntentJudge::new(None);
        let r = j.judge("你好", &[]);
        assert_eq!(r.reply_type, ReplyType::Continue);
        assert_eq!(r.should_respond, Some(true));
    }

    #[test]
    fn judge_llm_error_falls_back() {
        let j = IntentJudge::new(Some(Arc::new(|_p| {
            Err(VivianError::Provider("simulated failure".to_string()))
        })));
        let r = j.judge("你好", &[]);
        assert_eq!(r.should_respond, Some(true));
        assert!(!r.should_end);
    }
}
