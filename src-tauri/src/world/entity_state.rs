//! World Entity State —— 用户作为世界实体的在场/离开/预期回归追踪。
//!
//! 核心问题：现有 `ActivityTracker` 只跟踪"用户正在做某活动"（洗澡/睡觉/吃饭），
//! 但缺少对"用户作为实体是否在场"的统一视图。当用户没说为什么离开就直接消失时，
//! Agent 无法感知"用户已离开 X 分钟，预计何时回来"。
//!
//! 本模块提供：
//! - **UserEntityState**：追踪用户 presence（Present/Away）、away_since、expected_return
//! - **ExpectationEngine**：从对话文本抽取预期回归时间（"我去洗澡，20分钟"→ 20min）
//! - **ExpectedReturn**：预期回归范围 + 来源（Dialogue/CommonSense/Inferred）
//!
//! 与现有系统的关系：
//! - `ActivityTracker` 跟踪具体活动（per-activity 状态机）
//! - `UserEntityState` 跟踪用户在场/离开（per-entity 状态机）
//! - 两者互补：活动期望可派生预期回归（"洗澡" → 30-60min 常识）
//! - `Observation` 系统已扩展支持回归异常观察（早回/晚回/超时未归）

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

// ── 预期回归 ──

/// 预期回归来源
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ExpectationSource {
    /// 用户明说："20分钟后回来" / "我去洗澡，半小时"
    Dialogue { quote: String },
    /// 从活动常识推断："洗澡" → 30-60min（来自 ActivityExpectation）
    CommonSense { activity: String },
    /// 从历史模式推断（远期，当前未启用）
    Inferred,
}

/// 预期回归范围
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedReturn {
    /// 预期最短回归秒数
    pub min_secs: f64,
    /// 预期最长回归秒数
    pub max_secs: f64,
    /// 来源
    pub source: ExpectationSource,
}

impl ExpectedReturn {
    /// 用户实际离开秒数是否落在预期范围内
    pub fn classify(&self, actual_secs: f64) -> ReturnClassification {
        if actual_secs < self.min_secs * 0.5 {
            ReturnClassification::MuchEarlier
        } else if actual_secs < self.min_secs {
            ReturnClassification::Earlier
        } else if actual_secs <= self.max_secs {
            ReturnClassification::OnTime
        } else if actual_secs <= self.max_secs * 1.5 {
            ReturnClassification::Later
        } else {
            ReturnClassification::MuchLater
        }
    }
}

/// 回归分类（供 Observation 生成用）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReturnClassification {
    MuchEarlier,
    Earlier,
    OnTime,
    Later,
    MuchLater,
}

impl ReturnClassification {
    pub fn is_notable(&self) -> bool {
        !matches!(self, ReturnClassification::OnTime)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ReturnClassification::MuchEarlier => "much_earlier",
            ReturnClassification::Earlier => "earlier",
            ReturnClassification::OnTime => "on_time",
            ReturnClassification::Later => "later",
            ReturnClassification::MuchLater => "much_later",
        }
    }
}

// ── 用户实体状态 ──

/// 用户在场状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UserPresence {
    /// 在场（最近 60s 内有输入活动）
    Present,
    /// 离开（idle > away_threshold）
    Away,
}

/// 用户持续活动状态（由 LLM 在反思阶段产出）
///
/// LLM 自主判断用户是否进入了一个值得记录的持续状态（如睡觉/写代码/玩游戏），
/// 用简短中文词语概括。不预设枚举，由 LLM 语义概括。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserActivity {
    /// 活动标签（LLM 产出的简短中文词语，如"睡觉""写代码""玩游戏"）
    pub label: String,
    /// 开始时间（Unix 秒）
    pub started_at: f64,
    /// 置信度（0.0-1.0，LLM 自评，< 0.7 的建议在后处理阶段被忽略）
    #[serde(default = "default_activity_confidence")]
    pub confidence: f64,
}

fn default_activity_confidence() -> f64 {
    0.8
}

impl UserActivity {
    /// 已持续秒数
    pub fn elapsed_secs(&self, now: f64) -> f64 {
        (now - self.started_at).max(0.0)
    }
}

/// 用户实体状态快照（只读视图）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEntitySnapshot {
    pub presence: UserPresence,
    /// 离开起始时间（Unix 秒），Present 时为 None
    pub away_since: Option<f64>,
    /// 已离开秒数（现在 - away_since），Present 时为 0
    pub away_elapsed_secs: f64,
    /// 预期回归（None = 未知，Agent 不做预测）
    pub expected_return: Option<ExpectedReturn>,
    /// 最近一次活跃时间（Unix 秒）
    pub last_active_at: f64,
    /// 用户当前持续活动（由 LLM 反思产出，None 表示未进入明确持续状态）
    #[serde(default)]
    pub current_activity: Option<UserActivity>,
}

impl UserEntitySnapshot {
    /// 序列化为 prompt 段落
    ///
    /// 渲染为 "## User State" 区块，让 LLM 感知用户当前在场状态与持续活动。
    /// Present 且无预期且无活动时不输出，避免污染 prompt。
    pub fn serialize_for_prompt(&self, lang: &str) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        let now = chrono::Local::now().timestamp() as f64;
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let min_unit = match lang_norm { "en" => "min", "ja" => "分", _ => "分钟" };
        let sec_unit = match lang_norm { "en" => "s", "ja" => "秒", _ => "秒" };

        match self.presence {
            UserPresence::Present => {
                // 在场时仅在最近刚回归（< 60s）或有持续活动时提示
                if self.away_elapsed_secs > 0.0 && self.away_elapsed_secs < 60.0 {
                    let txt = match lang_norm {
                        "en" => format!("- Presence: just returned (was away {:.0}{})", self.away_elapsed_secs, sec_unit),
                        "ja" => format!("- 在席：戻ったばかり（{}{}離席）", self.away_elapsed_secs, sec_unit),
                        _ => format!("- 在场状态：刚回来（离开了 {:.0}{}）", self.away_elapsed_secs, sec_unit),
                    };
                    lines.push(txt);
                }
            }
            UserPresence::Away => {
                let away_min = (self.away_elapsed_secs / 60.0).round() as u64;
                let txt = match lang_norm {
                    "en" => format!("- Presence: away ({}{})", away_min, min_unit),
                    "ja" => format!("- 在席：離席中（{}{}）", away_min, min_unit),
                    _ => format!("- 在场状态：离开（{}{}）", away_min, min_unit),
                };
                lines.push(txt);
                if let Some(expected) = &self.expected_return {
                    let min_min = (expected.min_secs / 60.0).round() as u64;
                    let max_min = (expected.max_secs / 60.0).round() as u64;
                    match &expected.source {
                        ExpectationSource::Dialogue { quote } => {
                            let txt = match lang_norm {
                                "en" => format!("- Expected back: {}-{}{} (user said: \"{}\")", min_min, max_min, min_unit, quote),
                                "ja" => format!("- 帰還予想：{}-{}{}（ユーザー発言：「{}」）", min_min, max_min, min_unit, quote),
                                _ => format!("- 预计回归：{}-{}{}（用户说过：「{}」）", min_min, max_min, min_unit, quote),
                            };
                            lines.push(txt);
                        }
                        ExpectationSource::CommonSense { activity } => {
                            let txt = match lang_norm {
                                "en" => format!("- Expected back: {}-{}{} (inferred from: {})", min_min, max_min, min_unit, activity),
                                "ja" => format!("- 帰還予想：{}-{}{}（推測元：{}）", min_min, max_min, min_unit, activity),
                                _ => format!("- 预计回归：{}-{}{}（根据：{}）", min_min, max_min, min_unit, activity),
                            };
                            lines.push(txt);
                        }
                        ExpectationSource::Inferred => {
                            let txt = match lang_norm {
                                "en" => format!("- Expected back: {}-{}{}", min_min, max_min, min_unit),
                                "ja" => format!("- 帰還予想：{}-{}{}", min_min, max_min, min_unit),
                                _ => format!("- 预计回归：{}-{}{}", min_min, max_min, min_unit),
                            };
                            lines.push(txt);
                        }
                    }
                    // 预期回归超时提示
                    if self.away_elapsed_secs > expected.max_secs {
                        let overdue_secs = self.away_elapsed_secs - expected.max_secs;
                        let overdue_min = (overdue_secs / 60.0).round() as u64;
                        let txt = match lang_norm {
                            "en" => format!("- Overdue: {}{} past expected return", overdue_min, min_unit),
                            "ja" => format!("- 延滞：予想回帰を{}{}超過", overdue_min, min_unit),
                            _ => format!("- 超时：已超过预计回归时间 {}{}", overdue_min, min_unit),
                        };
                        lines.push(txt);
                    }
                }
            }
        }

        // 持续活动状态
        if let Some(activity) = &self.current_activity {
            let elapsed_min = (activity.elapsed_secs(now) / 60.0).round() as u64;
            let txt = match lang_norm {
                "en" => format!("- User activity: {} ({}{})", activity.label, elapsed_min, min_unit),
                "ja" => format!("- ユーザー活動：{}（{}{}）", activity.label, elapsed_min, min_unit),
                _ => format!("- 用户活动：{}（{}{}）", activity.label, elapsed_min, min_unit),
            };
            lines.push(txt);
        }

        if lines.is_empty() {
            None
        } else {
            let header = crate::pipeline::prompt_modules::section_heading("user_state", lang);
            Some(format!("{}\n{}", header, lines.join("\n")))
        }
    }
}

/// 用户实体状态（内部可变状态）
pub struct UserEntityState {
    inner: RwLock<UserEntityStateInner>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UserEntityStateInner {
    presence: UserPresence,
    away_since: Option<f64>,
    last_active_at: f64,
    expected_return: Option<ExpectedReturn>,
    /// 本轮离开是否已产生过超时观察（避免每个 tick 重复生成）
    #[serde(default)]
    overdue_observed: bool,
    /// 用户当前持续活动（由 LLM 反思产出）
    #[serde(default)]
    current_activity: Option<UserActivity>,
}

impl Default for UserPresence {
    fn default() -> Self {
        UserPresence::Present
    }
}

impl UserEntityState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(UserEntityStateInner {
                presence: UserPresence::Present,
                away_since: None,
                last_active_at: chrono::Local::now().timestamp() as f64,
                expected_return: None,
                overdue_observed: false,
                current_activity: None,
            }),
        }
    }

    /// 生成只读快照
    pub fn snapshot(&self) -> UserEntitySnapshot {
        let now = chrono::Local::now().timestamp() as f64;
        let inner = self.inner.read();
        let away_elapsed = match (inner.presence, inner.away_since) {
            (UserPresence::Away, Some(since)) => (now - since).max(0.0),
            _ => 0.0,
        };
        UserEntitySnapshot {
            presence: inner.presence,
            away_since: inner.away_since,
            away_elapsed_secs: away_elapsed,
            expected_return: inner.expected_return.clone(),
            last_active_at: inner.last_active_at,
            current_activity: inner.current_activity.clone(),
        }
    }

    /// 用户活跃信号（来自 proactive tick 的 idle_seconds < 60）
    ///
    /// 当从 Away → Present 时返回回归事件，携带实际离开时长供 Observation 生成。
    /// 回归时清除当前活动状态（上一个持续状态如睡觉/出门已结束）。
    pub fn mark_present(&self) -> Option<ReturnEvent> {
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        let was_away = inner.presence == UserPresence::Away;
        let away_secs = inner
            .away_since
            .map(|s| (now - s).max(0.0))
            .unwrap_or(0.0);
        let expected = inner.expected_return.clone();

        inner.presence = UserPresence::Present;
        inner.last_active_at = now;
        inner.away_since = None;
        // 回归后清空预期与超时标志（下次离开时重新建立）
        inner.expected_return = None;
        inner.overdue_observed = false;
        // 回归时清除持续活动：用户回来意味着上一个持续状态（睡觉/出门/上班）已结束
        inner.current_activity = None;

        if was_away {
            Some(ReturnEvent {
                away_secs,
                expected,
                returned_at: now,
            })
        } else {
            None
        }
    }

    /// 设置用户当前持续活动（由 LLM 反思阶段调用）
    ///
    /// 覆盖已有活动。label 为空时视为清除。
    pub fn set_user_activity(&self, label: &str, confidence: f64) {
        let label = label.trim();
        if label.is_empty() {
            self.clear_user_activity();
            return;
        }
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        inner.current_activity = Some(UserActivity {
            label: label.to_string(),
            started_at: now,
            confidence: confidence.clamp(0.0, 1.0),
        });
    }

    /// 清除用户当前持续活动
    pub fn clear_user_activity(&self) {
        self.inner.write().current_activity = None;
    }

    /// 原子取出当前活动并置空（供 WorldState 封存到行为日志后调用）
    pub fn take_current_activity(&self) -> Option<UserActivity> {
        self.inner.write().current_activity.take()
    }

    /// 原子替换当前活动，返回旧活动（供 WorldState 封存）
    ///
    /// new_label 为空时等同于 take_current_activity。
    pub fn swap_user_activity(&self, new_label: &str, new_confidence: f64) -> Option<UserActivity> {
        let mut inner = self.inner.write();
        let old = inner.current_activity.take();
        let label = new_label.trim();
        if !label.is_empty() {
            let now = chrono::Local::now().timestamp() as f64;
            inner.current_activity = Some(UserActivity {
                label: label.to_string(),
                started_at: now,
                confidence: new_confidence.clamp(0.0, 1.0),
            });
        }
        old
    }

    /// 刷新当前活动的置信度（同名活动重新确认时，不重置 started_at）
    pub fn refresh_activity_confidence(&self, confidence: f64) {
        if let Some(activity) = self.inner.write().current_activity.as_mut() {
            activity.confidence = confidence.clamp(0.0, 1.0);
        }
    }

    /// 用户离开信号（来自 proactive tick 的 idle_seconds > away_threshold）
    ///
    /// 重复调用安全：已 Away 状态下只更新 expected_return（若传入新预期）。
    pub fn mark_away(&self, expected: Option<ExpectedReturn>) {
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        if inner.presence == UserPresence::Present {
            inner.presence = UserPresence::Away;
            inner.away_since = Some(now);
            inner.expected_return = expected;
            inner.overdue_observed = false;
        } else if expected.is_some() {
            // 已 Away，但收到新预期（如用户离开前说了"我去洗澡"）则更新
            inner.expected_return = expected;
        }
    }

    /// 检查是否首次超时（用于 check_return_expectation 去重）
    ///
    /// 返回 true 表示这是本轮首次超时，应生成观察；后续调用返回 false。
    /// mark_present / mark_away 时重置为 false。
    pub fn check_and_mark_overdue(&self) -> bool {
        let mut inner = self.inner.write();
        if inner.overdue_observed {
            return false;
        }
        inner.overdue_observed = true;
        true
    }

    /// 从对话文本抽取预期并更新当前预期（仅当用户正在/即将离开时生效）
    ///
    /// Gap 1：同时抽取活动意图，若用户明说"我去上班了"等意图信号词 + 活动关键词，
    /// 直接写入 current_activity，无需等 LLM 反思 tick。
    pub fn ingest_dialogue(&self, text: &str) {
        let expected = ExpectationEngine::extract(text);
        let intent = ExpectationEngine::extract_activity_intent(text);

        let mut inner = self.inner.write();
        if let Some(exp) = expected {
            // 无论在场还是离开，都记录预期：在场时缓存供下次 mark_away 使用；
            // 离开时直接更新当前预期
            inner.expected_return = Some(exp);
        }
        if let Some(intent) = intent {
            let now = chrono::Local::now().timestamp() as f64;
            // 仅在当前无活动或活动不同名时覆盖（同名视为延续，不重置 started_at）
            let should_set = match &inner.current_activity {
                None => true,
                Some(existing) => existing.label != intent.label,
            };
            if should_set {
                inner.current_activity = Some(UserActivity {
                    label: intent.label,
                    started_at: now,
                    confidence: intent.confidence,
                });
            }
        }
    }

    /// 清空当前预期（用户重新活跃后调用）
    pub fn clear_expectation(&self) {
        self.inner.write().expected_return = None;
    }
}

impl Default for UserEntityState {
    fn default() -> Self {
        Self::new()
    }
}

/// 回归事件（mark_present 时产出，供 Observation 生成）
pub struct ReturnEvent {
    pub away_secs: f64,
    pub expected: Option<ExpectedReturn>,
    pub returned_at: f64,
}

impl ReturnEvent {
    /// 生成人类可读的观察描述（注入 Observation 系统）
    pub fn describe(&self) -> Option<String> {
        let expected = self.expected.as_ref()?;
        let classification = expected.classify(self.away_secs);
        if !classification.is_notable() {
            return None;
        }
        let away_min = (self.away_secs / 60.0).round() as u64;
        let min_min = (expected.min_secs / 60.0).round() as u64;
        let max_min = (expected.max_secs / 60.0).round() as u64;
        let quote = match &expected.source {
            ExpectationSource::Dialogue { quote } => format!("（用户说过：\"{}\"）", quote),
            ExpectationSource::CommonSense { activity } => format!("（推断自：{}）", activity),
            ExpectationSource::Inferred => String::new(),
        };
        Some(match classification {
            ReturnClassification::MuchEarlier => {
                format!("用户回来了，只离开了 {}min，远早于预期的 {}-{}min{}", away_min, min_min, max_min, quote)
            }
            ReturnClassification::Earlier => {
                format!("用户回来了，离开了 {}min，略早于预期的 {}-{}min{}", away_min, min_min, max_min, quote)
            }
            ReturnClassification::OnTime => return None,
            ReturnClassification::Later => {
                format!("用户回来了，离开了 {}min，略晚于预期的 {}-{}min{}", away_min, min_min, max_min, quote)
            }
            ReturnClassification::MuchLater => {
                format!("用户终于回来了，离开了 {}min，远超预期的 {}-{}min{}", away_min, min_min, max_min, quote)
            }
        })
    }
}

// ── ExpectationEngine ──

/// 活动意图（Gap 1：用户明说的活动快速通道）
///
/// 与 `ExpectedReturn` 配对：当用户说"我去上班了"时，ExpectationEngine 同时产出
/// - ExpectedReturn(28800~36000s)：用于 mark_away 的预期回归时间
/// - ActivityIntent { label: "上班" }：用于 set_user_activity 写入当前活动状态
///
/// 这样不需要等 LLM 反思阶段才产生活动，规则抽取的快速通道直接生效。
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityIntent {
    /// 中文活动标签（与 UserActivity.label 同语义，如"上班""洗澡""睡觉"）
    pub label: String,
    /// 置信度（规则抽取固定 0.85，低于 LLM 反思的 0.9 但远高于猜测）
    pub confidence: f64,
}

/// 预期回归抽取引擎
///
/// 从对话文本中识别"X 分钟后回来" / "我去 Y，Z 分钟" 等模式，
/// 产出 ExpectedReturn。纯规则实现，无 LLM 调用。
///
/// 同时支持抽取活动意图（Gap 1）：用户明说"我去上班了"时直接产出
/// ActivityIntent，避免等待 LLM 反思 tick 才更新 current_activity。
pub struct ExpectationEngine;

impl ExpectationEngine {
    /// 从文本抽取预期回归时间
    ///
    /// 识别模式（按优先级）：
    /// 1. 数字 + 分钟/小时单位 + 回来/后语境
    /// 2. 活动 + 默认常识范围（洗澡/吃饭/睡觉等）
    /// 3. 半小时/一小时/一会儿 等中文量词
    pub fn extract(text: &str) -> Option<ExpectedReturn> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        // 策略 1：数字 + 单位（"20分钟"、"半小时"、"2小时"）
        if let Some(expected) = Self::extract_explicit_duration(text) {
            return Some(expected);
        }

        // 策略 2：活动关键词 → 常识范围
        if let Some(expected) = Self::extract_from_activity(text) {
            return Some(expected);
        }

        None
    }

    /// 从文本抽取活动意图（Gap 1）
    ///
    /// 仅在用户明说"我去 X" / "我准备 X" / "开始 X" 等活动意图信号词 + 活动关键词
    /// 同时出现时产出，避免在用户讨论 X 话题时误判（如"上班真累"不应触发活动状态切换）。
    pub fn extract_activity_intent(text: &str) -> Option<ActivityIntent> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        // 活动意图信号词：必须是"开始/去做"某事，而非讨论某事
        let intent_signals = [
            "我去", "我准备", "我要去", "我要开始", "我开始", "准备开始",
            "去吃饭", "去洗澡", "去睡觉", "去上班", "去开会", "去散步", "去运动", "去跑步",
            "吃饭去", "睡觉去", "下班", "上班去", "出门去", "出去一下",
        ];
        let has_intent = intent_signals.iter().any(|s| text.contains(s));
        if !has_intent {
            return None;
        }
        // 在意图信号词存在的前提下，匹配活动关键词
        for (keyword, _min, _max, _activity, label) in ACTIVITY_MAP {
            if text.contains(keyword) {
                return Some(ActivityIntent {
                    label: label.to_string(),
                    confidence: 0.85,
                });
            }
        }
        None
    }

    /// 抽取显式时长（"20分钟"、"半小时"、"2小时后回来"）
    fn extract_explicit_duration(text: &str) -> Option<ExpectedReturn> {
        // 中文数字映射
        let cn_nums = [
            ("一", 1u64), ("二", 2), ("两", 2), ("三", 3), ("四", 4), ("五", 5),
            ("六", 6), ("七", 7), ("八", 8), ("九", 9), ("十", 10),
            ("半", 0), // 特殊处理
        ];

        // 模式：数字 + "分钟" / "小时" / "分" / "小时"
        // 先尝试阿拉伯数字
        let patterns: &[(&str, f64)] = &[
            (r"(\d+(?:\.\d+)?)\s*个?小时", 3600.0),
            (r"(\d+(?:\.\d+)?)\s*分钟", 60.0),
            (r"(\d+(?:\.\d+)?)\s*分(?!钟)", 60.0),
        ];

        for (pat, unit) in patterns {
            if let Some(caps) = simple_regex_capture(pat, text) {
                if let Ok(n) = caps.parse::<f64>() {
                    let secs = n * unit;
                    return Some(ExpectedReturn {
                        min_secs: (secs * 0.7).max(60.0),
                        max_secs: secs * 1.5,
                        source: ExpectationSource::Dialogue {
                            quote: text.chars().take(60).collect(),
                        },
                    });
                }
            }
        }

        // 中文数字 + 单位
        for (cn, _) in &cn_nums {
            if text.contains(&format!("{}小时", cn)) || text.contains(&format!("{}个半小时", cn)) {
                let n = if *cn == "半" { 0.5 } else { parse_cn_num(cn) as f64 };
                let secs = n * 3600.0;
                return Some(ExpectedReturn {
                    min_secs: (secs * 0.7).max(60.0),
                    max_secs: secs * 1.5,
                    source: ExpectationSource::Dialogue {
                        quote: text.chars().take(60).collect(),
                    },
                });
            }
            if text.contains(&format!("{}分钟", cn)) || text.contains(&format!("{}分", cn)) {
                let n = if *cn == "半" { 30.0 } else { parse_cn_num(cn) as f64 };
                let secs = n * 60.0;
                return Some(ExpectedReturn {
                    min_secs: (secs * 0.7).max(60.0),
                    max_secs: secs * 1.5,
                    source: ExpectationSource::Dialogue {
                        quote: text.chars().take(60).collect(),
                    },
                });
            }
        }

        // "半小时" 单独模式
        if text.contains("半小时") {
            return Some(ExpectedReturn {
                min_secs: 1200.0,
                max_secs: 2400.0,
                source: ExpectationSource::Dialogue {
                    quote: text.chars().take(60).collect(),
                },
            });
        }

        // "一会儿" / "马上" / "很快" → 短时预期
        if text.contains("一会儿") || text.contains("马上") || text.contains("很快") || text.contains("稍等") {
            return Some(ExpectedReturn {
                min_secs: 60.0,
                max_secs: 600.0,
                source: ExpectationSource::Dialogue {
                    quote: text.chars().take(60).collect(),
                },
            });
        }

        None
    }

    /// 从活动关键词推断常识范围
    fn extract_from_activity(text: &str) -> Option<ExpectedReturn> {
        for (keyword, min, max, activity, _label) in ACTIVITY_MAP {
            if text.contains(keyword) {
                // 必须包含"离开意图"信号词才认定为离开预期
                let leaving_signals = ["去", "我", "回", "出", "一下", "马上", "等"];
                if leaving_signals.iter().any(|s| text.contains(s)) {
                    return Some(ExpectedReturn {
                        min_secs: *min,
                        max_secs: *max,
                        source: ExpectationSource::CommonSense {
                            activity: activity.to_string(),
                        },
                    });
                }
            }
        }
        None
    }
}

/// 活动 + 时长 + 标签的统一映射表
///
/// 字段：(关键词, min_secs, max_secs, activity_英文标识, label_中文标签)
const ACTIVITY_MAP: &[(&str, f64, f64, &str, &str)] = &[
    ("洗澡", 600.0, 2400.0, "shower", "洗澡"),
    ("沐浴", 600.0, 2400.0, "shower", "洗澡"),
    ("吃饭", 1200.0, 3600.0, "meal", "吃饭"),
    ("吃饭去", 1200.0, 3600.0, "meal", "吃饭"),
    ("午饭", 1200.0, 3600.0, "meal", "吃饭"),
    ("晚饭", 1200.0, 3600.0, "meal", "吃饭"),
    ("早饭", 600.0, 1800.0, "meal", "吃饭"),
    ("睡觉", 21600.0, 36000.0, "sleep", "睡觉"),
    ("睡一觉", 21600.0, 36000.0, "sleep", "睡觉"),
    ("午休", 1800.0, 5400.0, "nap", "午休"),
    ("午睡", 1800.0, 5400.0, "nap", "午休"),
    ("出门", 1800.0, 14400.0, "outing", "出门"),
    ("出去", 1800.0, 14400.0, "outing", "出门"),
    ("上班", 28800.0, 36000.0, "work", "上班"),
    ("工作", 3600.0, 28800.0, "work", "工作"),
    ("开会", 1800.0, 7200.0, "meeting", "开会"),
    ("散步", 900.0, 3600.0, "walk", "散步"),
    ("运动", 1800.0, 7200.0, "exercise", "运动"),
    ("跑步", 1800.0, 5400.0, "exercise", "跑步"),
    ("看电影", 5400.0, 10800.0, "movie", "看电影"),
    ("买东西", 1800.0, 7200.0, "shopping", "买东西"),
    ("买菜", 1200.0, 3600.0, "shopping", "买菜"),
];

// ── 辅助函数 ──

/// 极简正则捕获（避免引入 regex crate 依赖）
///
/// 仅支持 `(\d+(?:\.\d+)?)` 形式的捕获组。
fn simple_regex_capture(pattern: &str, text: &str) -> Option<String> {
    // 我们只用到数字捕获，直接扫描文本找数字
    let _ = pattern;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let num: String = chars[start..i].iter().collect();
            // 检查后续是否跟着单位
            let rest: String = chars[i..].iter().collect();
            if rest.starts_with("小时")
                || rest.starts_with("个小时")
                || rest.starts_with("分钟")
                || rest.starts_with("分")
            {
                return Some(num);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// 解析单个中文数字字符
fn parse_cn_num(s: &str) -> u64 {
    match s {
        "一" => 1,
        "二" => 2,
        "两" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "七" => 7,
        "八" => 8,
        "九" => 9,
        "十" => 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_explicit_minutes() {
        let r = ExpectationEngine::extract("我去洗澡，20分钟后回来").unwrap();
        assert_eq!(r.source, ExpectationSource::Dialogue { quote: "我去洗澡，20分钟后回来".to_string() });
        assert!(r.min_secs < 1200.0); // 20min * 60 * 0.7 = 840
        assert!(r.max_secs > 1200.0); // 20min * 60 * 1.5 = 1800
    }

    #[test]
    fn test_extract_half_hour() {
        let r = ExpectationEngine::extract("我出去一下，半小时回来").unwrap();
        assert!(r.min_secs > 0.0);
    }

    #[test]
    fn test_extract_activity_commonsense() {
        let r = ExpectationEngine::extract("我去洗澡").unwrap();
        match r.source {
            ExpectationSource::CommonSense { activity } => assert_eq!(activity, "shower"),
            _ => panic!("expected CommonSense"),
        }
    }

    #[test]
    fn test_extract_no_match() {
        assert!(ExpectationEngine::extract("今天天气真好").is_none());
    }

    #[test]
    fn test_classify_return() {
        let e = ExpectedReturn {
            min_secs: 600.0,
            max_secs: 1800.0,
            source: ExpectationSource::Inferred,
        };
        assert_eq!(e.classify(300.0), ReturnClassification::MuchEarlier);
        assert_eq!(e.classify(800.0), ReturnClassification::Earlier);
        assert_eq!(e.classify(1200.0), ReturnClassification::OnTime);
        assert_eq!(e.classify(2000.0), ReturnClassification::Later);
        assert_eq!(e.classify(4000.0), ReturnClassification::MuchLater);
    }

    #[test]
    fn test_snapshot_prompt_present() {
        let s = UserEntitySnapshot {
            presence: UserPresence::Present,
            away_since: None,
            away_elapsed_secs: 0.0,
            expected_return: None,
            last_active_at: 0.0,
            current_activity: None,
        };
        assert!(s.serialize_for_prompt("zh").is_none());
    }

    #[test]
    fn test_snapshot_prompt_away() {
        let s = UserEntitySnapshot {
            presence: UserPresence::Away,
            away_since: Some(100.0),
            away_elapsed_secs: 1800.0,
            expected_return: Some(ExpectedReturn {
                min_secs: 600.0,
                max_secs: 1200.0,
                source: ExpectationSource::Dialogue { quote: "20分钟".to_string() },
            }),
            last_active_at: 0.0,
            current_activity: None,
        };
        let prompt = s.serialize_for_prompt("en").unwrap();
        assert!(prompt.contains("away"));
        assert!(prompt.contains("Overdue"));
    }
}
