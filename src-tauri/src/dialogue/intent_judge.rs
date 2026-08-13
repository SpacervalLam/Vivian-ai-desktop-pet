//! 意图判断 — 规则预检 + LLM 驱动的会话关闭原因判断
//!
//! 规则预检使用 n-gram 嵌入 Top-K 投票 + softmax 加权将输入文本与预定义的
//! 晚安/再见/打断三个意图的种子短语集合做匹配，替代预设关键词列表匹配。
//! 匹配成功时直接判定，跳过 LLM 调用。
//! 规则未覆盖的语义判断（冲突/话题切换/隐含告别等）由 LLM 完成，
//! 通过路由矩阵的 `intent_judge` 任务路由到专用 provider。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use crate::conversation::CloseReason;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// LLM 判断超时（秒）
const JUDGE_TIMEOUT_SECS: u64 = 8;

/// 送入 prompt 的最大历史消息条数
const MAX_HISTORY_MSGS: usize = 6;

/// 单条历史消息截断长度
const HISTORY_MSG_TRUNCATE: usize = 150;

// ── n-gram 嵌入匹配（用于规则预检，替代预设关键词列表） ──

type NGramVector = HashMap<String, f64>;

const NGRAM_N: usize = 3;
const TOP_K: usize = 5;
const SOFTMAX_TEMP: f64 = 0.15;
const CLOSE_REASON_THRESHOLD: f64 = 0.20;

/// 种子短语的上下文元数据
///
/// 用于区分同一告别类别下的不同子场景，便于未来上下文感知的排序和日志分析。
#[derive(Debug, Clone, Copy, PartialEq)]
enum FarewellContext {
    /// 睡前告别（去睡觉、就寝）
    Bedtime,
    /// 日常离开（出门、上班、外出）
    DailyLeave,
    /// 对话结束（拜拜、再见、下次聊）
    ChatEnd,
    /// 临时离开（马上回来、稍等）
    TemporaryLeave,
    /// 打断/忙（电话、有事要忙）
    BusyInterruption,
}

impl FarewellContext {
    fn as_str(&self) -> &'static str {
        match self {
            FarewellContext::Bedtime => "睡前告别",
            FarewellContext::DailyLeave => "日常离开",
            FarewellContext::ChatEnd => "对话结束",
            FarewellContext::TemporaryLeave => "临时离开",
            FarewellContext::BusyInterruption => "打断/忙",
        }
    }
}

/// 带元数据的种子短语条目
struct SeedEntry {
    text: &'static str,
    reason: CloseReason,
    context: FarewellContext,
}

/// 所有告别种子短语的扁平列表（含元数据）
const FAREWELL_SEEDS: &[SeedEntry] = &[
    // ── GoodNight ──
    SeedEntry { text: "晚安啦", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "睡了哦", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "睡觉了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "去睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "休息了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "去休息", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "上床了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "我想睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "我要睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "准备睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "该睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "睡去了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "要睡了", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "good night", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "goodnight", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "gonna sleep", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "going to bed", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "sleep tight", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "おやすみ", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "寝ますね", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },
    SeedEntry { text: "もう寝る", reason: CloseReason::GoodNight, context: FarewellContext::Bedtime },

    // ── GoodBye ──
    SeedEntry { text: "拜拜啦", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "再见了", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "回头见", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "下次聊", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "回聊哦", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "撤了哦", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "下线了", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "我先撤了", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "bye bye", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "goodbye", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "see you", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "catch you", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "talk later", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "see ya", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "バイバイ", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "またね", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "じゃあね", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "さようなら", reason: CloseReason::GoodBye, context: FarewellContext::ChatEnd },
    SeedEntry { text: "我走了", reason: CloseReason::GoodBye, context: FarewellContext::DailyLeave },
    SeedEntry { text: "先走了", reason: CloseReason::GoodBye, context: FarewellContext::DailyLeave },
    SeedEntry { text: "我先走了", reason: CloseReason::GoodBye, context: FarewellContext::DailyLeave },
    SeedEntry { text: "gotta go", reason: CloseReason::GoodBye, context: FarewellContext::DailyLeave },
    SeedEntry { text: "leaving", reason: CloseReason::GoodBye, context: FarewellContext::DailyLeave },

    // ── Interrupted ──
    SeedEntry { text: "等一下", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "稍等哦", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "等会儿哦", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "等会儿说", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "等会再说", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "待会回来", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "马上回来", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "hold on", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "one sec", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "hang on", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "brb now", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "ちょっと待って", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "少し待って", reason: CloseReason::Interrupted, context: FarewellContext::TemporaryLeave },
    SeedEntry { text: "我先忙", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "忙一下", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "先忙了", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "老板电话", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "电话来了", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "接个电话", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
    SeedEntry { text: "有人找我", reason: CloseReason::Interrupted, context: FarewellContext::BusyInterruption },
];

/// 预计算的种子向量缓存（(entry, vector) 对）
static FAREWELL_CACHED: LazyLock<Vec<(&SeedEntry, NGramVector)>> = LazyLock::new(|| {
    FAREWELL_SEEDS.iter().map(|s| (s, compute_ngram_vector(s.text))).collect()
});

/// 计算文本的字符 n-gram 向量（归一化至单位长度）
fn compute_ngram_vector(text: &str) -> NGramVector {
    let chars: Vec<char> = text.chars().collect();
    let n = NGRAM_N.min(chars.len());
    let mut counts = HashMap::new();
    for i in 0..=chars.len().saturating_sub(n) {
        let gram: String = chars[i..i + n].iter().collect();
        *counts.entry(gram).or_insert(0.0) += 1.0;
    }
    let norm: f64 = counts.values().map(|v| v * v).sum();
    if norm > 0.0 {
        for v in counts.values_mut() {
            *v /= norm.sqrt();
        }
    }
    counts
}

/// 计算两个归一化 n-gram 向量的余弦相似度
fn cosine_similarity(a: &NGramVector, b: &NGramVector) -> f64 {
    let (smaller, larger) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut dot = 0.0;
    for (k, va) in smaller {
        if let Some(vb) = larger.get(k) {
            dot += va * vb;
        }
    }
    dot
}

/// 用 Top-K 投票 + softmax 加权选择最佳匹配的告别类别。
///
/// 将所有种子短语的相似度排名，取前 K 条，对每个类别按 softmax 权重加总，
/// 取加权票数最高的类别。平票时按优先级 GoodNight > GoodBye > Interrupted 裁决。
fn topk_vote(vec: &NGramVector) -> Option<CloseReason> {
    // 计算与所有种子短语的相似度
    let mut scored: Vec<(CloseReason, FarewellContext, f64)> = FAREWELL_CACHED
        .iter()
        .map(|(entry, seed_vec)| (entry.reason, entry.context, cosine_similarity(vec, seed_vec)))
        .collect();

    // 按相似度降序排列
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    // 取 top-K
    let top_k = &scored[..TOP_K.min(scored.len())];

    // 最高相似度低于阈值 → 无匹配
    if top_k.is_empty() || top_k[0].2 < CLOSE_REASON_THRESHOLD {
        return None;
    }

    // softmax 加权投票
    let max_sim = top_k[0].2;
    let mut goodnight_votes = 0.0_f64;
    let mut goodbye_votes = 0.0_f64;
    let mut interrupted_votes = 0.0_f64;

    for (reason, context, sim) in top_k {
        let weight = ((sim - max_sim) / SOFTMAX_TEMP).exp();
        match reason {
            CloseReason::GoodNight => goodnight_votes += weight,
            CloseReason::GoodBye => goodbye_votes += weight,
            CloseReason::Interrupted => interrupted_votes += weight,
            _ => {} // 不属于这三种的种子不应出现在 FAREWELL_SEEDS 中
        }
        tracing::trace!(
            "[IntentJudge] Top-K 种子: reason={:?}, context={}, sim={:.4}, weight={:.4}",
            reason,
            context.as_str(),
            sim,
            weight,
        );
    }

    // 按加权票数降序 + 优先级裁决
    let mut candidates = [
        (CloseReason::GoodNight, goodnight_votes),
        (CloseReason::GoodBye, goodbye_votes),
        (CloseReason::Interrupted, interrupted_votes),
    ];
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let a_prio = match a.0 {
                    CloseReason::GoodNight => 0,
                    CloseReason::GoodBye => 1,
                    _ => 2,
                };
                let b_prio = match b.0 {
                    CloseReason::GoodNight => 0,
                    CloseReason::GoodBye => 1,
                    _ => 2,
                };
                a_prio.cmp(&b_prio)
            })
    });

    if candidates[0].1 > 0.0 {
        tracing::debug!(
            "[IntentJudge] Top-K 投票结果: {:?} (votes={:.4}), GoodNight={:.4}, GoodBye={:.4}, Interrupted={:.4}",
            candidates[0].0,
            candidates[0].1,
            goodnight_votes,
            goodbye_votes,
            interrupted_votes,
        );
        Some(candidates[0].0)
    } else {
        None
    }
}

/// 意图判断器 — LLM 驱动的会话关闭原因判断
///
/// 单次 LLM 调用判断用户的最新消息（或 Agent 回复）是否表示应关闭会话，
/// 并返回对应的 `CloseReason`。规则预检覆盖明确的中/日/英告别词，
/// 未覆盖的语义判断由 LLM 通过 `intent_judge` 任务完成。
///
/// LLM 不可用 / 超时 / 解析失败时返回 `None`，由 Energy/Novelty/Continuation
/// 状态机自然推进，避免误关闭。
pub struct IntentJudge {
    router: Option<Arc<ModelRouter>>,
}

impl IntentJudge {
    pub fn new(router: Option<Arc<ModelRouter>>) -> Self {
        Self { router }
    }

    /// 后绑定 ModelRouter
    pub fn set_router(&mut self, router: Arc<ModelRouter>) {
        self.router = Some(router);
    }

    /// 判断会话是否应关闭并返回关闭原因
    ///
    /// `text` 用户最新消息（或 Agent 回复），`history` 近期对话轮次（每项为单条文本，
    /// 按奇偶位置推断说话者：第 0 条为 User，第 1 条为 AI，交替）。
    ///
    /// 流程：
    /// 1. 规则预检：明确告别词命中 → 直接返回对应 CloseReason
    /// 2. LLM 判断：通过 `intent_judge` 任务路由，让 LLM 在 9 个枚举值中判断
    /// 3. 异常兜底：返回 None（不关闭）
    pub async fn judge_close_reason(&self, text: &str, history: &[String]) -> Option<CloseReason> {
        // 规则预检：明确告别词直接判定，跳过 LLM 调用
        if let Some(reason) = Self::rule_based_check(text) {
            return Some(reason);
        }

        let router = self.router.as_ref()?;
        let history_text = Self::build_history_text(history);
        let prompt = Self::build_prompt(text, &history_text);
        let messages = vec![ChatMessage::user(prompt)];

        let request = LLMRequest::new("intent_judge", messages)
            .with_max_tokens(64)
            .with_temperature(0.0);

        // 超时保护：LLM 判断不应阻塞主对话流程
        let timeout_result = tokio::time::timeout(
            Duration::from_secs(JUDGE_TIMEOUT_SECS),
            router.generate(request),
        )
        .await;

        match timeout_result {
            Ok(Ok(response)) => Self::parse_close_reason(&response),
            Ok(Err(e)) => {
                tracing::warn!("[IntentJudge] LLM 判断失败，跳过关闭判定: {}", e);
                None
            }
            Err(_) => {
                tracing::warn!("[IntentJudge] LLM 判断超时 ({}s)", JUDGE_TIMEOUT_SECS);
                None
            }
        }
    }

    // ===== 内部辅助 =====

    /// 规则预检：对明确告别词直接判定，避免不必要的 LLM 调用
    ///
    /// 用 Top-K 投票 + softmax 加权（而非单一最大相似度）将输入文本与
    /// 晚安/再见/打断三个意图的种子短语做匹配，替代预设关键词列表匹配。
    /// 优先级：GoodNight > GoodBye > Interrupted（平票时）。
    ///
    /// 注意：不再使用 contains 关键词匹配，完全依赖 n-gram 嵌入的语义相似度。
    /// 种子短语的意图变体覆盖率在构建时已确保，用户的新表达通过 n-gram 重叠自然匹配。
    fn rule_based_check(text: &str) -> Option<CloseReason> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.chars().count() < 3 {
            return None;
        }

        let vec = compute_ngram_vector(trimmed);
        if vec.is_empty() {
            return None;
        }

        // Top-K 投票：跨所有告别类别综合判断
        topk_vote(&vec)
    }

    /// 构造历史文本块
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

    /// 构造关闭原因判断 prompt
    fn build_prompt(user_message: &str, history_text: &str) -> String {
        let lang_norm =
            crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        match lang_norm {
            "en" => format!(
                "Analyze the latest message in the conversation context. Decide whether the session should close now, and if so, why.\n\n\
                 Recent conversation:\n\
                 {history_text}\n\n\
                 Latest message to analyze: {user_message}\n\n\
                 Choose exactly ONE from the following enum values:\n\
                 - good_night: speaker is going to sleep / bed\n\
                 - good_bye: speaker is leaving / saying goodbye / ending chat\n\
                 - interrupted: speaker temporarily steps away but intends to come back (phone call, busy moment, brb)\n\
                 - conflict: conversation ended after an argument / fight / tension\n\
                 - switch_topic: speaker explicitly opens a clearly new topic, signaling the old one is done\n\
                 - no_response: passive silence — the other side has stopped replying\n\
                 - timeout: long silence with no engagement\n\
                 - natural: topic reached natural conclusion, conversation fading, no explicit farewell\n\
                 - none: session should NOT close; the message continues the topic or is just a normal reply\n\n\
                 Reply with ONLY the enum value (one word, lowercase, no explanation, no punctuation)."
            ),
            "ja" => format!(
                "会話文脈の最新メッセージを分析し、セッションを終了すべきか、その場合は理由を判定してください。\n\n\
                 最近の会話：\n\
                 {history_text}\n\n\
                 分析対象の最新メッセージ：{user_message}\n\n\
                 以下の列挙値から厳密に 1 つを選んでください：\n\
                 - good_night：発言者が寝る / 就寝する\n\
                 - good_bye：発言者が立ち去る / 別れを告げる / チャットを終える\n\
                 - interrupted：発言者が一時的に離れるが戻るつもり（電話、忙しい場面、brb）\n\
                 - conflict：口論 / けんか / 緊張の後に会話が終了\n\
                 - switch_topic：発言者が明示的に新しい話題を始め、旧話題の終了を示唆\n\
                 - no_response：受動的な沈黙 — 相手が返信をやめた\n\
                 - timeout：長い沈黙、エンゲージメントなし\n\
                 - natural：話題が自然に着地、会話がフェードアウト、明示的な別れはない\n\
                 - none：セッションは終了すべきでない — メッセージは話題を続けるか通常の返信\n\n\
                 列挙値のみ（1 語、小文字、説明なし、句読点なし）で回答すること。"
            ),
            _ => format!(
                "分析对话上下文中的最新消息，判断会话是否应关闭，如果应关闭则给出原因。\n\n\
                 近期对话：\n\
                 {history_text}\n\n\
                 待分析的最新消息：{user_message}\n\n\
                 从以下枚举值中精确选择一个：\n\
                 - good_night：发言者准备睡觉 / 就寝\n\
                 - good_bye：发言者要离开 / 道别 / 结束聊天\n\
                 - interrupted：发言者暂时离开但打算回来（电话、临时有事、brb）\n\
                 - conflict：争吵 / 冲突 / 紧张后对话结束\n\
                 - switch_topic：发言者明确开启新话题，标志着旧话题结束\n\
                 - no_response：被动沉默——对方停止回复\n\
                 - timeout：长时间沉默，无互动\n\
                 - natural：话题自然结束，对话渐弱，无明确告别\n\
                 - none：会话不应关闭——消息在延续话题或是正常回复\n\n\
                 仅回答枚举值（一个词、小写、无解释、无标点）。"
            ),
        }
    }

    /// 解析 LLM 单行响应为 CloseReason
    fn parse_close_reason(response: &str) -> Option<CloseReason> {
        let trimmed = response.trim().to_lowercase();
        // 取首行首词，容错 LLM 偶发的换行/标点尾巴
        let first_token = trimmed
            .lines()
            .next()?
            .split_whitespace()
            .next()?
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
        match first_token {
            "good_night" | "goodnight" => Some(CloseReason::GoodNight),
            "good_bye" | "goodbye" => Some(CloseReason::GoodBye),
            "interrupted" => Some(CloseReason::Interrupted),
            "conflict" => Some(CloseReason::Conflict),
            "switch_topic" => Some(CloseReason::SwitchTopic),
            "no_response" => Some(CloseReason::NoResponse),
            "timeout" => Some(CloseReason::Timeout),
            "natural" => Some(CloseReason::Natural),
            _ => None,
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

    #[test]
    fn rule_goodnight_zh() {
        assert_eq!(
            IntentJudge::rule_based_check("晚安啦"),
            Some(CloseReason::GoodNight)
        );
    }

    #[test]
    fn rule_goodnight_en() {
        assert_eq!(
            IntentJudge::rule_based_check("good night"),
            Some(CloseReason::GoodNight)
        );
    }

    #[test]
    fn rule_goodbye_ja() {
        assert_eq!(
            IntentJudge::rule_based_check("じゃあね"),
            Some(CloseReason::GoodBye)
        );
    }

    #[test]
    fn rule_interrupted() {
        assert_eq!(
            IntentJudge::rule_based_check("稍等一下"),
            Some(CloseReason::Interrupted)
        );
        assert_eq!(
            IntentJudge::rule_based_check("brb"),
            Some(CloseReason::Interrupted)
        );
    }

    #[test]
    fn rule_none_for_normal() {
        assert_eq!(IntentJudge::rule_based_check("今天天气不错"), None);
        assert_eq!(IntentJudge::rule_based_check(""), None);
    }

    #[test]
    fn parse_known_values() {
        assert_eq!(
            IntentJudge::parse_close_reason("good_night"),
            Some(CloseReason::GoodNight)
        );
        assert_eq!(
            IntentJudge::parse_close_reason("switch_topic"),
            Some(CloseReason::SwitchTopic)
        );
        assert_eq!(
            IntentJudge::parse_close_reason("  Conflict  \n"),
            Some(CloseReason::Conflict)
        );
    }

    #[test]
    fn parse_none_and_unknown() {
        assert_eq!(IntentJudge::parse_close_reason("none"), None);
        assert_eq!(IntentJudge::parse_close_reason("xyz"), None);
        assert_eq!(IntentJudge::parse_close_reason(""), None);
    }

    #[test]
    fn parse_strips_punctuation() {
        assert_eq!(
            IntentJudge::parse_close_reason("good_bye."),
            Some(CloseReason::GoodBye)
        );
        assert_eq!(
            IntentJudge::parse_close_reason("\"natural\""),
            Some(CloseReason::Natural)
        );
    }

    #[tokio::test]
    async fn no_router_returns_none() {
        let judge = IntentJudge::new(None);
        let reason = judge.judge_close_reason("我要走了", &[]).await;
        // "我要走了" 不在规则预检中，且无 router → None
        assert_eq!(reason, None);
    }
}
