//! 用户认知引擎（User Cognition Engine）—— 把行为日志提炼为 Belief，
//! 并在新观察与既有 Belief 出现冲突时进行修正。
//!
//! 三层职责：
//! 1. **认知整理（Consolidate）**：Rest 时调 LLM 读取近期行为日志，提炼为
//!    带结构化度量（metric/value/match_labels）的习惯 Belief，写入 BeliefStore。
//! 2. **冲突检测（Detect Conflict）**：每次封存新行为事件后，按 match_labels
//!    匹配既有 Belief，比较 value 与新观察值的偏差，超过阈值时返回 Conflict。
//!    - 偏差适中：调用 EMA 平滑修正（避免单次异常剧烈改变认知）
//!    - 偏差过大：返回 Conflict 供上层注入 prompt，触发 LLM 主动询问用户
//! 3. **观察注入（Observation Context）**：用户在持续状态中突然说话时
//!    （如睡觉中发消息），生成简短观察文本供 prompt 注入，让 LLM 自然回应。

use std::sync::Arc;

use chrono::Timelike;
use serde::Deserialize;

use crate::error::VivianResult;
use crate::mind::{
    belief::classify_metric, Belief, BeliefCategory, BeliefStatus, MetricKind, Mind,
};
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::world::user_behavior::{SharedUserBehaviorLog, UserBehaviorEntry};
use crate::world::UserEntitySnapshot;

/// EMA 平滑系数（默认 0.85，新值占 15%，越保守 alpha 越大）
const DEFAULT_EMA_ALPHA: f64 = 0.85;

/// 偏差阈值（相对值）：新观察值与既有 value 偏差超过此比例触发冲突
/// 例如 sleep_hours=7.4，新值 11.2，偏差 = (11.2-7.4)/7.4 ≈ 0.51 > 0.3 → 冲突
const CONFLICT_RELATIVE_THRESHOLD: f64 = 0.3;

/// 仅做 EMA 修正（不触发主动询问）的偏差阈值
/// 偏差在 [EMA_THRESHOLD, CONFLICT_THRESHOLD) 之间时静默修正
const EMA_RELATIVE_THRESHOLD: f64 = 0.15;

/// 认知整理报告
#[derive(Debug, Default)]
pub struct CognitionReport {
    /// 本次整理产出的 Belief 数量
    pub beliefs_created: usize,
    /// 强化的既有 Belief 数量
    pub beliefs_reinforced: usize,
    /// LLM 返回的原始条目数
    pub raw_count: usize,
}

/// 冲突检测结果
#[derive(Debug, Clone)]
pub struct BeliefConflict {
    /// 与新观察冲突的既有 Belief ID
    pub belief_id: String,
    /// Belief 的旧 value
    pub old_value: f64,
    /// 新观察值
    pub new_value: f64,
    /// 相对偏差（abs(new-old)/old）
    pub deviation: f64,
    /// 度量名（如 sleep_hours）
    pub metric: String,
    /// Belief 的自然语言陈述（供 prompt 注入）
    pub statement: String,
}

/// LLM 返回的认知草稿
#[derive(Debug, Deserialize)]
struct CognitionDraft {
    /// 自然语言陈述，如"用户通常睡 7.4 小时"
    statement: String,
    /// 度量名（machine-readable，如 sleep_hours / dinner_time / study_hours）
    #[serde(default)]
    metric: Option<String>,
    /// 度量值（与 metric 配对，如 7.4）
    #[serde(default)]
    value: Option<f64>,
    /// 匹配的行为标签（用于冲突检测，如 ["睡觉","午睡","小憩"]）
    #[serde(default)]
    match_labels: Vec<String>,
    /// 置信度 0.0-1.0
    #[serde(default = "default_confidence")]
    confidence: f64,
    /// 类别（默认 habit）
    #[serde(default = "default_category")]
    category: String,
}

fn default_confidence() -> f64 {
    0.7
}

fn default_category() -> String {
    "habit".to_string()
}

/// 用户认知引擎
pub struct UserCognitionEngine {
    router: Arc<ModelRouter>,
    /// 证据交集阈值：≥ 此值则合并而非新建（与 BeliefGenerator 一致）
    merge_overlap_threshold: usize,
}

impl UserCognitionEngine {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self {
            router,
            merge_overlap_threshold: 2,
        }
    }

    /// 整理近期行为日志为用户习惯 Belief
    ///
    /// 由 Rest 后台任务调用。读取行为日志近 N 条，调 LLM 提炼为带度量的 Belief。
    /// 新 Belief 走 `BeliefStore::upsert_with_merge`，证据交集 ≥ 阈值则强化既有。
    pub async fn consolidate_behaviors_to_beliefs(
        &self,
        behavior_log: &SharedUserBehaviorLog,
        mind: &Mind,
        recent_n: usize,
    ) -> VivianResult<CognitionReport> {
        let (text_block, entry_count) = {
            let log = behavior_log.read();
            let count = log.entries.len();
            if count == 0 {
                return Ok(CognitionReport::default());
            }
            (log.serialize_for_consolidation(recent_n), count)
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());

        // 获取既有 habit 类 Belief（最多 10 条），供 LLM 去重/更新参考
        let existing_beliefs_text = {
            let store = mind.beliefs.read();
            let habits = store.by_category(BeliefCategory::Habit);
            if habits.is_empty() {
                String::new()
            } else {
                habits
                    .iter()
                    .take(10)
                    .map(|b| {
                        let metric_str = b.metric.as_deref().unwrap_or("-");
                        let value_str = b
                            .value
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| "-".to_string());
                        format!(
                            "- [{}] {} (metric={}, value={})",
                            b.id, b.statement, metric_str, value_str
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        let prompt = match lang_norm {
            "en" => format!(
                "You are the cognition consolidation subsystem of character \"{}\". Below is the recent behavior log of the user that you observed (with durations).\n\
                 Please extract the user's behavioral habits (Belief of type Habit) from it.\n\n\
                 ## Requirements\n\
                 - Each Belief describes a quantifiable habit (e.g. sleep duration, meal time, study duration)\n\
                 - Must fill metric (machine-readable English snake_case, e.g. sleep_hours / dinner_hour / study_hours)\n\
                 - Must fill value (numeric, e.g. 7.4 means 7.4 hours)\n\
                 - Must fill match_labels (array of Chinese activity labels, e.g. [\"睡觉\",\"午睡\"]), used to match future observations\n\
                 - Only output patterns observable from the log; do not fabricate\n\
                 - Output at most 5 Beliefs\n\
                 - Output an empty array when the log sample is insufficient\n\n\
                 ## Output Format (pure JSON, no markdown markers)\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"User usually sleeps 7.4 hours\",\n      \"metric\": \"sleep_hours\",\n      \"value\": 7.4,\n      \"match_labels\": [\"睡觉\"],\n      \"confidence\": 0.85,\n      \"category\": \"habit\"\n    }}\n  ]\n}}\n\n\
                 ## Existing Beliefs\n{}\n\n\
                 The following Beliefs already exist. If the new data matches one of them, output the same metric to update it; otherwise add a new one.\n\n\
                 ## Recent behavior log (total {} entries, showing the latest {})\n{}",
                mind.char_id, existing_beliefs_text, entry_count, recent_n.min(entry_count), text_block
            ),
            "ja" => format!(
                "あなたはキャラクター「{}」の認知整理サブシステムです。以下はあなたが観察したユーザーの最近の行動ログです（所要時間付き）。\n\
                 ユーザーの行動習慣（Habit タイプの Belief）を抽出してください。\n\n\
                 ## 要件\n\
                 - 各 Belief は定量化可能な習慣を表現する（例：睡眠時間、食事時間、学習時間）\n\
                 - metric を必ず記入する（機械可読な英語 snake_case、例：sleep_hours / dinner_hour / study_hours）\n\
                 - value を必ず記入する（数値、例：7.4 は 7.4 時間を意味する）\n\
                 - match_labels を必ず記入する（中国語の活動ラベル配列、例：[\"睡觉\",\"午睡\"]）、将来の観測と照合するために使用\n\
                 - ログから読み取れる規則性のみを出力し、でっち上げないこと\n\
                 - 最大5件の Belief を出力\n\
                 - ログサンプルが不十分な場合は空配列を出力\n\n\
                 ## 出力形式（純粋な JSON、markdown マーカーなし）\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"ユーザーは通常7.4時間寝る\",\n      \"metric\": \"sleep_hours\",\n      \"value\": 7.4,\n      \"match_labels\": [\"睡觉\"],\n      \"confidence\": 0.85,\n      \"category\": \"habit\"\n    }}\n  ]\n}}\n\n\
                 ## 既存の Belief\n{}\n\n\
                 以下の Belief は既に存在します。新データと吻合する場合は同じ metric を出力して更新し、そうでなければ新規追加してください。\n\n\
                 ## 最近の行動ログ（合計 {} 件、直近の {} 件を表示）\n{}",
                mind.char_id, existing_beliefs_text, entry_count, recent_n.min(entry_count), text_block
            ),
            _ => format!(
                "你是角色「{}」的认知整理子系统。下面是你观察到的用户近期行为日志（带时长）。\n\
                 请从中提炼出用户的行为习惯（Habit 类型的 Belief）。\n\n\
                 ## 要求\n\
                 - 每条 Belief 描述一个可量化的习惯（如睡眠时长、用餐时段、学习时长）\n\
                 - 必须填写 metric（机器可读的英文 snake_case，如 sleep_hours / dinner_hour / study_hours）\n\
                 - 必须填写 value（数值，如 7.4 表示 7.4 小时）\n\
                 - 必须填写 match_labels（中文活动标签数组，如 [\"睡觉\",\"午睡\"]），用于将来匹配新观察\n\
                 - 仅从日志中能看出的规律输出，不要臆造\n\
                 - 最多输出 5 条 Belief\n\
                 - 日志样本不足时输出空数组\n\n\
                 ## 输出格式（纯 JSON，无 markdown 标记）\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"用户通常睡 7.4 小时\",\n      \"metric\": \"sleep_hours\",\n      \"value\": 7.4,\n      \"match_labels\": [\"睡觉\"],\n      \"confidence\": 0.85,\n      \"category\": \"habit\"\n    }}\n  ]\n}}\n\n\
                 ## 既有 Belief\n{}\n\n\
                 以下 Belief 已存在，若新数据与之吻合则输出相同 metric 进行更新，否则新增。\n\n\
                 ## 近期行为日志（共 {} 条，展示最近 {} 条）\n{}",
                mind.char_id, existing_beliefs_text, entry_count, recent_n.min(entry_count), text_block
            ),
        };

        let response = self
            .router
            .generate(LLMRequest::new(
                "reflection",
                vec![ChatMessage::user(prompt)],
            ))
            .await?;

        let parsed = match parse_cognition_response(&response) {
            Some(p) => p,
            None => {
                tracing::debug!(
                    "[UserCognition:{}] LLM 返回无法解析，跳过本次整理",
                    mind.char_id
                );
                return Ok(CognitionReport::default());
            }
        };

        let now = chrono::Utc::now().timestamp();
        let mut report = CognitionReport {
            raw_count: parsed.len(),
            ..Default::default()
        };

        // 写入 BeliefStore
        let mut store = mind.beliefs.write();
        for draft in &parsed {
            // match_labels 归一化（trim + 去重）
            let mut labels: Vec<String> = draft
                .match_labels
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            labels.sort();
            labels.dedup();

            let belief = Belief {
                id: format!(
                    "cog_{}_{}_{}",
                    now,
                    draft.metric.as_deref().unwrap_or("gen"),
                    draft.statement.chars().take(6).collect::<String>()
                ),
                statement: draft.statement.clone(),
                subject: "user".to_string(),
                category: parse_category(&draft.category),
                confidence: draft.confidence.clamp(0.0, 1.0),
                source_memory_ids: vec![format!("behavior_log_{}", now)],
                source_episode_ids: Vec::new(),
                created_at: now,
                last_reinforced_at: now,
                reinforcement_count: 0,
                contradiction_count: 0,
                status: BeliefStatus::Stable,
                metric: draft.metric.clone().filter(|s| !s.is_empty()),
                value: draft.value,
                match_labels: labels,
                superseded_by: None,
            };

            let before_len = store.beliefs.len();
            let _id = store.upsert_with_merge(belief, self.merge_overlap_threshold, now);
            if store.beliefs.len() > before_len {
                report.beliefs_created += 1;
            } else {
                report.beliefs_reinforced += 1;
            }
        }
        drop(store);

        if let Err(e) = mind.persist() {
            tracing::warn!("[UserCognition:{}] 持久化失败：{}", mind.char_id, e);
        }

        tracing::info!(
            "[UserCognition:{}] 行为日志整理：原始 {} 条 → 新建 {} / 强化 {} Belief",
            mind.char_id,
            report.raw_count,
            report.beliefs_created,
            report.beliefs_reinforced
        );

        Ok(report)
    }

    /// 检测新行为事件与既有 Belief 的冲突
    ///
    /// 在 `WorldState::seal_activity` 后调用。按 match_labels 匹配 Belief，
    /// 再根据 metric 类型从新事件提取对应类型的观察值（Duration 用时长、
    /// TimeOfDay 用本地小时、Count 无法从单事件得出则跳过），与 Belief.value
    /// 比较，偏差 ≥ CONFLICT_RELATIVE_THRESHOLD 时返回 Conflict 供上层注入 prompt。
    ///
    /// 同时：
    /// - 偏差在 [EMA_THRESHOLD, CONFLICT_THRESHOLD) 之间：直接 EMA 修正
    ///   （TimeOfDay 走循环 EMA，Duration/Count 走线性 EMA，由 Belief 自行选择）
    /// - 偏差 ≥ CONFLICT_THRESHOLD：返回 Conflict，由上层决定是否触发主动询问
    ///   （用户回应后或一段时间无回应，再由上层调用 apply_ema_revision）
    pub fn detect_conflict(
        &self,
        new_entry: &UserBehaviorEntry,
        mind: &Mind,
    ) -> Option<BeliefConflict> {
        let candidates: Vec<Belief> = {
            let store = mind.beliefs.read();
            store
                .beliefs
                .iter()
                .filter(|b| {
                    b.subject == "user"
                        && b.status != BeliefStatus::Superseded
                        && b.metric.is_some()
                        && b.value.is_some()
                        && !b.match_labels.is_empty()
                        && b
                            .match_labels
                            .iter()
                            .any(|label| label == &new_entry.activity_label)
                })
                .cloned()
                .collect()
        };

        if candidates.is_empty() {
            return None;
        }

        let mut conflict: Option<BeliefConflict> = None;
        let mut ema_fixes: Vec<(String, f64)> = Vec::new();

        for belief in &candidates {
            let metric_name = belief.metric.as_deref().unwrap_or("");
            let kind = classify_metric(metric_name);

            // 从新事件提取对应类型的观察值
            let new_value = match extract_observed_value(new_entry, kind) {
                Some(v) => v,
                None => continue, // Count 类无法从单事件得出，跳过
            };

            let old_value = belief.value.unwrap_or(0.0);
            let deviation = compute_deviation(new_value, old_value, kind);

            if deviation >= CONFLICT_RELATIVE_THRESHOLD {
                // 大偏差：返回 Conflict 供上层注入 prompt
                if conflict.is_none()
                    || deviation > conflict.as_ref().unwrap().deviation
                {
                    conflict = Some(BeliefConflict {
                        belief_id: belief.id.clone(),
                        old_value,
                        new_value,
                        deviation,
                        metric: metric_name.to_string(),
                        statement: belief.statement.clone(),
                    });
                }
            } else if deviation >= EMA_RELATIVE_THRESHOLD {
                // 中等偏差：直接 EMA 修正（不触发主动询问）
                ema_fixes.push((belief.id.clone(), new_value));
            }
        }

        // 批量 EMA 修正（Belief::revise_value_ema 会根据 metric 类型自动选择线性/循环）
        if !ema_fixes.is_empty() {
            let mut store = mind.beliefs.write();
            for (id, new_val) in &ema_fixes {
                store.revise_value_ema(id, *new_val, DEFAULT_EMA_ALPHA);
            }
            drop(store);
            tracing::info!(
                "[UserCognition:{}] {} 条 Belief 触发 EMA 静默修正",
                mind.char_id,
                ema_fixes.len()
            );
            let _ = mind.persist();
        }

        if let Some(c) = &conflict {
            tracing::info!(
                "[UserCognition:{}] 检测到 Belief 冲突：{}（{}）old={:.2} new={:.2} dev={:.2}",
                mind.char_id,
                c.statement,
                c.metric,
                c.old_value,
                c.new_value,
                c.deviation
            );
        }

        conflict
    }

    /// 应用 EMA 修正（用户回应冲突后或超时无回应时调用）
    pub fn apply_ema_revision(&self, mind: &Mind, belief_id: &str, new_value: f64) -> bool {
        let ok = {
            let mut store = mind.beliefs.write();
            store.revise_value_ema(belief_id, new_value, DEFAULT_EMA_ALPHA)
        };
        if ok {
            let _ = mind.persist();
            tracing::info!(
                "[UserCognition:{}] Belief {} 已 EMA 修正为方向 {}",
                mind.char_id,
                belief_id,
                new_value
            );
        }
        ok
    }

    /// 生成观察上下文（用户在持续状态中突然说话时调用）
    ///
    /// 例如：用户 6 小时前进入"睡觉"状态，现在突然发消息，但未明确说"我醒了"。
    /// 返回简短观察文本供 prompt 注入，让 LLM 自然回应"你醒啦？"之类的内容。
    ///
    /// 不修改任何状态——状态修改由 LLM 在反思阶段决定（输出新的 world_update）。
    pub fn generate_observation_context(snapshot: &UserEntitySnapshot, lang: &str) -> Option<String> {
        let activity = snapshot.current_activity.as_ref()?;
        let now = chrono::Local::now().timestamp() as f64;
        let elapsed_secs = activity.elapsed_secs(now);

        // 至少持续 10 分钟才值得提示（避免瞬时状态干扰）
        if elapsed_secs < 600.0 {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let header = crate::pipeline::prompt_modules::section_heading("observation", lang);
        let body = match lang_norm {
            "en" => {
                let desc = if elapsed_secs >= 3600.0 {
                    format!("{:.1} hours", elapsed_secs / 3600.0)
                } else {
                    format!("{:.0} min", elapsed_secs / 60.0)
                };
                format!("The user's current state is \"{}\" (lasting {}), but they just sent a message. \
                         This could mean they were interrupted mid-state, or the state ended but they haven't said so. \
                         You can naturally respond based on this observation, e.g. ask if they're done with what they were doing.",
                        activity.label, desc)
            }
            "ja" => {
                let desc = if elapsed_secs >= 3600.0 {
                    format!("{:.1} 時間", elapsed_secs / 3600.0)
                } else {
                    format!("{:.0} 分", elapsed_secs / 60.0)
                };
                format!("ユーザーの現在の状態は「{}」（継続 {}）ですが、メッセージを受信しました。\
                         これは状態が途中で中断されたか、状態が終了したがまだ明言していないことを意味します。\
                         この観察に基づいて自然に応答できます。例えば状態が終了したか尋ねる。",
                        activity.label, desc)
            }
            _ => {
                let desc = if elapsed_secs >= 3600.0 {
                    format!("{:.1} 小时", elapsed_secs / 3600.0)
                } else {
                    format!("{:.0} 分钟", elapsed_secs / 60.0)
                };
                format!("用户当前状态为「{}」（已持续 {}），但用户刚刚发送了消息。\
                         这意味着用户可能在持续状态下被打断，或状态已结束但尚未明说。\
                         可以基于此观察自然回应，例如询问对方是否已结束当前状态。",
                        activity.label, desc)
            }
        };
        Some(format!("{}\n{}", header, body))
    }

    /// 将 Belief 冲突序列化为 prompt 注入段（供 LLM 主动询问）
    ///
    /// 根据 metric 类型选择合适的措辞：
    /// - TimeOfDay：用 HH:MM 格式描述时间点，方向用"更晚/更早"
    /// - Duration：用带单位的数值（如 11.0 小时），方向用"睡这么多/这么少"
    /// - Count：用次数描述，方向用"这么多/这么少"
    pub fn conflict_to_prompt_section(conflict: &BeliefConflict, lang: &str) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let kind = classify_metric(&conflict.metric);
        let (old_desc, new_desc, question_hint) = match kind {
            MetricKind::TimeOfDay => {
                let old_hm = format_hour_to_hm(conflict.old_value);
                let new_hm = format_hour_to_hm(conflict.new_value);
                let (direction, q) = match lang_norm {
                    "en" => {
                        let d = if conflict.new_value > conflict.old_value { "later" } else { "earlier" };
                        (d, format!("e.g. \"Why are you {} ({}) today? Isn't it usually {}?\"", d, new_hm, old_hm))
                    }
                    "ja" => {
                        let d = if conflict.new_value > conflict.old_value { "遅い" } else { "早い" } ;
                        (d, format!("例えば「今日なんで{}（{}）なの？いつもは{}じゃない？」", d, new_hm, old_hm))
                    }
                    _ => {
                        let d = if conflict.new_value > conflict.old_value { "更晚" } else { "更早" };
                        (d, format!("例如「你今天怎么{}（{}）？一般不都是{}吗？」", d, new_hm, old_hm))
                    }
                };
                let _ = direction;
                (old_hm, new_hm, q)
            }
            MetricKind::Duration => {
                let unit = metric_unit_zh(&conflict.metric);
                let old_desc = format!("{:.1}{}", conflict.old_value, unit);
                let new_desc = format!("{:.1}{}", conflict.new_value, unit);
                let q = match lang_norm {
                    "en" => {
                        let d = if conflict.new_value > conflict.old_value { "so much" } else { "so little" };
                        format!("e.g. \"Why did you {} today? Isn't it usually {}?\"", d, old_desc)
                    }
                    "ja" => {
                        let d = if conflict.new_value > conflict.old_value { "こんなに多い" } else { "こんなに少ない" };
                        format!("例えば「今日なんで{}なの？いつもは{}じゃない？」", d, old_desc)
                    }
                    _ => {
                        let d = if conflict.new_value > conflict.old_value { "这么多" } else { "这么少" };
                        format!("例如「你今天怎么{}？一般不都是{}吗？」", d, old_desc)
                    }
                };
                (old_desc, new_desc, q)
            }
            MetricKind::Count => {
                let (old_desc, new_desc) = match lang_norm {
                    "en" => (format!("{:.0} times", conflict.old_value), format!("{:.0} times", conflict.new_value)),
                    "ja" => (format!("{:.0} 回", conflict.old_value), format!("{:.0} 回", conflict.new_value)),
                    _ => (format!("{:.0} 次", conflict.old_value), format!("{:.0} 次", conflict.new_value)),
                };
                let q = match lang_norm {
                    "en" => {
                        let d = if conflict.new_value > conflict.old_value { "so much" } else { "so little" };
                        format!("e.g. \"Why did you {} today? Isn't it usually {}?\"", d, old_desc)
                    }
                    "ja" => {
                        let d = if conflict.new_value > conflict.old_value { "こんなに多い" } else { "こんなに少ない" };
                        format!("例えば「今日なんで{}なの？いつもは{}じゃない？」", d, old_desc)
                    }
                    _ => {
                        let d = if conflict.new_value > conflict.old_value { "这么多" } else { "这么少" };
                        format!("例如「你今天怎么{}？一般不都是{}吗？」", d, old_desc)
                    }
                };
                (old_desc, new_desc, q)
            }
        };

        let header = crate::pipeline::prompt_modules::section_heading("belief_conflict", lang);
        match lang_norm {
            "en" => format!(
                "{}\n\
                 You previously believed \"{}\" ({}), but the newly observed value is {}, a {:.0}% deviation.\n\
                 This conflicts with your understanding. You should proactively ask the user why——\n\
                 {}, then update your understanding based on their reply.\n\
                 If they don't respond, don't push——observe again next time.",
                header,
                conflict.statement,
                old_desc,
                new_desc,
                conflict.deviation * 100.0,
                question_hint,
            ),
            "ja" => format!(
                "{}\n\
                 あなたは「{}」（{}）と信じていたが、新たに観察された値は {} で、{:.0}% の偏差がある。\n\
                 これはあなたの認識と矛盾する。ユーザーに理由を積極的に尋ねるべき——\n\
                 {}、そして回答に基づいて認識を更新する。\n\
                 応答がない場合は無理に追及せず、次の機会に再度観察する。",
                header,
                conflict.statement,
                old_desc,
                new_desc,
                conflict.deviation * 100.0,
                question_hint,
            ),
            _ => format!(
                "{}\n\
                 你原本认为「{}」（{}），但刚刚观察到的新值是 {}，偏差 {:.0}%。\n\
                 这与你的认知存在冲突。你应当主动向用户询问原因——\n\
                 {}，然后根据用户的回答更新认知。\n\
                 若用户未回应，也无需强行追问，下次有机会再观察。",
                header,
                conflict.statement,
                old_desc,
                new_desc,
                conflict.deviation * 100.0,
                question_hint,
            ),
        }
    }
}

/// 从行为事件提取观察值（按度量类型）
///
/// - Duration：从 duration_secs 派生小时数
/// - TimeOfDay：从 started_at 提取本地小时数（0-24）
/// - Count：单条事件不能直接得出频次，返回 None（detect_conflict 会跳过）
fn extract_observed_value(entry: &UserBehaviorEntry, kind: MetricKind) -> Option<f64> {
    match kind {
        MetricKind::Duration => Some(entry.duration_hours()),
        MetricKind::TimeOfDay => {
            let ts = entry.started_at as i64;
            chrono::DateTime::from_timestamp(ts, 0).map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .time()
                    .num_seconds_from_midnight() as f64
                    / 3600.0
            })
        }
        MetricKind::Count => None,
    }
}

/// 计算偏差（按度量类型）
///
/// - Duration / Count：线性相对偏差 `|new-old|/old`
/// - TimeOfDay：循环距离归一化到 [0, 1]，最大距离 12 对应 deviation=1.0
fn compute_deviation(new_value: f64, old_value: f64, kind: MetricKind) -> f64 {
    match kind {
        MetricKind::Duration | MetricKind::Count => {
            if old_value.abs() < 1e-6 {
                return 0.0;
            }
            ((new_value - old_value).abs() / old_value).max(0.0)
        }
        MetricKind::TimeOfDay => crate::mind::circular_distance(new_value, old_value, 24.0) / 12.0,
    }
}

/// 将小时数（0-24 浮点）格式化为 HH:MM 字符串
fn format_hour_to_hm(hour: f64) -> String {
    let h = hour.floor() as i32;
    let m = ((hour - h as f64) * 60.0).round() as i32;
    format!("{:02}:{:02}", h.rem_euclid(24), m.rem_euclid(60))
}

/// 根据 metric 名称返回中文单位
fn metric_unit_zh(metric: &str) -> &'static str {
    let m = metric.to_lowercase();
    if m.ends_with("_hours") {
        " 小时"
    } else if m.ends_with("_minutes") {
        " 分钟"
    } else if m.ends_with("_secs") || m.ends_with("_seconds") {
        " 秒"
    } else {
        ""
    }
}

#[derive(Debug, Default, Deserialize)]
struct ParsedCognition {
    #[serde(default)]
    beliefs: Vec<CognitionDraft>,
}

fn parse_cognition_response(text: &str) -> Option<Vec<CognitionDraft>> {
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: ParsedCognition = serde_json::from_str(cleaned).ok()?;
    Some(parsed.beliefs)
}

fn parse_category(s: &str) -> BeliefCategory {
    match s.to_lowercase().as_str() {
        "trait" => BeliefCategory::Trait,
        "habit" => BeliefCategory::Habit,
        "preference" => BeliefCategory::Preference,
        "state" => BeliefCategory::State,
        "relationship" => BeliefCategory::Relationship,
        _ => BeliefCategory::Habit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::user_behavior::{BehaviorEndReason, UserBehaviorEntry};

    fn make_entry(label: &str, hours: f64) -> UserBehaviorEntry {
        let started = 1000.0;
        let duration = hours * 3600.0;
        UserBehaviorEntry {
            id: format!("test_{}", label),
            activity_label: label.to_string(),
            started_at: started,
            ended_at: started + duration,
            duration_secs: duration,
            source: "llm_observation".to_string(),
            ended_by: BehaviorEndReason::UserReturn,
            confidence: 0.9,
        }
    }

    /// 构造一个 started_at 对应本地 hour:00 的行为事件（用于 TimeOfDay 度量测试）
    fn make_entry_at_hour(label: &str, local_hour: f64, duration_hours: f64) -> UserBehaviorEntry {
        let now_local = chrono::Local::now();
        let target = now_local
            .date_naive()
            .and_hms_opt(local_hour.floor() as u32, ((local_hour * 60.0) % 60.0) as u32, 0)
            .unwrap_or_else(|| now_local.naive_local());
        let started = target.and_utc().timestamp() as f64;
        let duration = duration_hours * 3600.0;
        UserBehaviorEntry {
            id: format!("test_at_{}", local_hour),
            activity_label: label.to_string(),
            started_at: started,
            ended_at: started + duration,
            duration_secs: duration,
            source: "llm_observation".to_string(),
            ended_by: BehaviorEndReason::UserReturn,
            confidence: 0.9,
        }
    }

    #[test]
    fn test_parse_cognition_response() {
        let json = r#"```json
        {
            "beliefs": [
                {
                    "statement": "用户通常睡 7.4 小时",
                    "metric": "sleep_hours",
                    "value": 7.4,
                    "match_labels": ["睡觉", "午睡"],
                    "confidence": 0.85,
                    "category": "habit"
                }
            ]
        }
        ```"#;
        let parsed = parse_cognition_response(json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].metric.as_deref(), Some("sleep_hours"));
        assert!((parsed[0].value.unwrap() - 7.4).abs() < 1e-6);
        assert_eq!(parsed[0].match_labels.len(), 2);
    }

    #[test]
    fn test_make_entry_duration() {
        let entry = make_entry("睡觉", 7.5);
        assert!((entry.duration_hours() - 7.5).abs() < 1e-6);
    }

    #[test]
    fn test_format_hour_to_hm() {
        assert_eq!(format_hour_to_hm(19.0), "19:00");
        assert_eq!(format_hour_to_hm(19.5), "19:30");
        assert_eq!(format_hour_to_hm(23.75), "23:45");
        assert_eq!(format_hour_to_hm(0.0), "00:00");
        // 越界回绕
        assert_eq!(format_hour_to_hm(24.5), "00:30");
        assert_eq!(format_hour_to_hm(-1.0), "23:00");
    }

    #[test]
    fn test_metric_unit_zh() {
        assert_eq!(metric_unit_zh("sleep_hours"), " 小时");
        assert_eq!(metric_unit_zh("break_minutes"), " 分钟");
        assert_eq!(metric_unit_zh("pause_secs"), " 秒");
        assert_eq!(metric_unit_zh("pause_seconds"), " 秒");
        assert_eq!(metric_unit_zh("dinner_hour"), "");
        assert_eq!(metric_unit_zh("unknown"), "");
    }

    #[test]
    fn test_compute_deviation_duration() {
        // sleep_hours 旧 7.4，新 11.0 → 线性相对偏差
        let dev = compute_deviation(11.0, 7.4, MetricKind::Duration);
        assert!((dev - (3.6 / 7.4)).abs() < 1e-6);
        // old=0 时返回 0（避免除零）
        assert_eq!(compute_deviation(5.0, 0.0, MetricKind::Duration), 0.0);
    }

    #[test]
    fn test_compute_deviation_time_of_day() {
        // dinner_hour 旧 19，新 23 → circular_distance=4，归一化 4/12
        let dev = compute_deviation(23.0, 19.0, MetricKind::TimeOfDay);
        assert!((dev - (4.0 / 12.0)).abs() < 1e-6);
        // 跨日：旧 19，新 2 → circular_distance=5，归一化 5/12
        let dev = compute_deviation(2.0, 19.0, MetricKind::TimeOfDay);
        assert!((dev - (5.0 / 12.0)).abs() < 1e-6);
        // 同点：偏差 0
        assert_eq!(compute_deviation(19.0, 19.0, MetricKind::TimeOfDay), 0.0);
    }

    #[test]
    fn test_extract_observed_value_duration() {
        let entry = make_entry("睡觉", 7.5);
        let v = extract_observed_value(&entry, MetricKind::Duration).unwrap();
        assert!((v - 7.5).abs() < 1e-6);
    }

    #[test]
    fn test_extract_observed_value_time_of_day() {
        // 构造 started_at 对应本地 19:30 的事件
        let entry = make_entry_at_hour("吃晚饭", 19.5, 0.5);
        let v = extract_observed_value(&entry, MetricKind::TimeOfDay).unwrap();
        // 允许 ±0.01 误差（时区/夏令时边界）
        assert!(
            (v - 19.5).abs() < 0.01,
            "expected ~19.5, got {}",
            v
        );
    }

    #[test]
    fn test_extract_observed_value_count_returns_none() {
        let entry = make_entry("吃饭", 1.0);
        assert!(extract_observed_value(&entry, MetricKind::Count).is_none());
    }

    #[test]
    fn test_conflict_to_prompt_section_time_of_day() {
        let conflict = BeliefConflict {
            belief_id: "b1".into(),
            old_value: 19.0,
            new_value: 23.5,
            deviation: 0.375,
            metric: "dinner_hour".into(),
            statement: "用户通常 19:00 吃晚饭".into(),
        };
        let section = UserCognitionEngine::conflict_to_prompt_section(&conflict, "zh");
        assert!(section.contains("19:00"));
        assert!(section.contains("23:30"));
        assert!(section.contains("更晚"));
        assert!(section.contains("信念冲突检测"));
    }

    #[test]
    fn test_conflict_to_prompt_section_duration() {
        let conflict = BeliefConflict {
            belief_id: "b1".into(),
            old_value: 7.4,
            new_value: 11.0,
            deviation: 0.486,
            metric: "sleep_hours".into(),
            statement: "用户通常睡 7.4 小时".into(),
        };
        let section = UserCognitionEngine::conflict_to_prompt_section(&conflict, "zh");
        assert!(section.contains("7.4 小时"));
        assert!(section.contains("11.0 小时"));
        assert!(section.contains("这么多"));
    }

    #[test]
    fn test_conflict_to_prompt_section_count() {
        let conflict = BeliefConflict {
            belief_id: "b1".into(),
            old_value: 3.0,
            new_value: 6.0,
            deviation: 1.0,
            metric: "meal_count".into(),
            statement: "用户通常每天吃 3 顿".into(),
        };
        let section = UserCognitionEngine::conflict_to_prompt_section(&conflict, "zh");
        assert!(section.contains("3 次"));
        assert!(section.contains("6 次"));
        assert!(section.contains("这么多"));
    }

    /// 端到端：dinner_hour Belief 与跨日新观察冲突
    ///
    /// 旧 Belief：dinner_hour=19.0（19:00 吃晚饭）
    /// 新事件：started_at 对应本地 02:00（次日凌晨吃晚饭）
    /// circular_distance(2, 19, 24) = 5，deviation = 5/12 ≈ 0.417 > 0.3 → 触发冲突
    #[test]
    fn test_detect_conflict_time_of_day_wrap_around() {
        // 构造凌晨 2 点的"吃晚饭"事件
        let entry = make_entry_at_hour("吃晚饭", 2.0, 0.5);

        // 直接调用内部逻辑（不依赖 ModelRouter）
        let metric_name = "dinner_hour";
        let kind = classify_metric(metric_name);
        let new_value = extract_observed_value(&entry, kind).unwrap();
        let old_value = 19.0;
        let dev = compute_deviation(new_value, old_value, kind);

        // 验证新值确实是 ~2.0（凌晨 2 点）
        assert!(
            (new_value - 2.0).abs() < 0.01,
            "expected new_value ~2.0, got {}",
            new_value
        );
        // 验证偏差 > 0.3（触发冲突阈值）
        assert!(
            dev > CONFLICT_RELATIVE_THRESHOLD,
            "expected dev > {}, got {}",
            CONFLICT_RELATIVE_THRESHOLD,
            dev
        );
    }

    /// 端到端：sleep_hours Belief 与新观察冲突（保留原行为）
    #[test]
    fn test_detect_conflict_duration_linear() {
        let metric_name = "sleep_hours";
        let kind = classify_metric(metric_name);
        let entry = make_entry("睡觉", 11.0); // 11 小时
        let new_value = extract_observed_value(&entry, kind).unwrap();
        let old_value = 7.4;
        let dev = compute_deviation(new_value, old_value, kind);

        assert!((new_value - 11.0).abs() < 1e-6);
        assert!(
            dev > CONFLICT_RELATIVE_THRESHOLD,
            "sleep 11h vs habit 7.4h should trigger conflict, dev={}",
            dev
        );
    }

    /// 频次类：detect_conflict 应跳过（无法从单事件得出频次）
    #[test]
    fn test_detect_conflict_count_skipped() {
        let metric_name = "meal_count";
        let kind = classify_metric(metric_name);
        let entry = make_entry("吃饭", 0.5);
        // extract_observed_value 对 Count 返回 None
        assert!(extract_observed_value(&entry, kind).is_none());
    }
}
