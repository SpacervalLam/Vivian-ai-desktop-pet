//! 关系日志 — 每轮交互的关系信号记录与演化。
//!
//! 记录用户情绪、关系信号、重要时刻、下次回应提示，形成可回溯的关系演化轨迹。
//! 与 RelationshipState（5 维数值快照）互补：后者是当前状态，本模块是历史轨迹。
//!
//! 集成点：
//! - Stage 2 反思第五路抽取关系信号写入本日志
//! - PromptBuildingStep 读取本日志的近期线索注入 prompt

use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 单轮关系日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipLogEntry {
    /// 唯一 ID
    pub id: String,
    /// 日期字符串（YYYY-MM-DD，用于按日聚合）
    pub date: String,
    /// 创建时间戳（秒）
    pub created_at: f64,
    /// 用户当轮情绪（疲惫/焦虑/低落/开心/平静/烦躁等）
    #[serde(default)]
    pub user_mood: String,
    /// 关系信号（用户对 Vivian 的态度信号，如亲近/疏远/信任/试探/依赖等）
    #[serde(default)]
    pub relationship_signal: String,
    /// 重要时刻（本轮是否发生关系里程碑或值得记住的瞬间，可为空）
    #[serde(default)]
    pub important_moment: Option<String>,
    /// 下次回应提示（基于本轮情况，Vivian 下次该如何回应）
    #[serde(default)]
    pub next_care_cue: String,
    /// 关系方向：UserAgent（用户↔智能体）或 AgentAgent（智能体↔智能体）
    #[serde(default)]
    pub direction: RelationshipDirection,
    /// AgentAgent 方向时，对方智能体 ID（UserAgent 方向时为 None）
    #[serde(default)]
    pub target_agent_id: Option<String>,
}

/// 关系信号方向
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipDirection {
    /// 用户↔智能体（默认，向后兼容）
    #[default]
    UserAgent,
    /// 智能体↔智能体
    AgentAgent,
}

/// 每日关系摘要
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelationshipDailySummary {
    /// 日期（YYYY-MM-DD）
    pub date: String,
    /// 当日交互轮数
    pub turn_count: usize,
    /// 当日主导用户情绪
    pub dominant_mood: String,
    /// 当日关系信号聚合
    pub signal_summary: String,
    /// 当日重要时刻（如有）
    #[serde(default)]
    pub highlight: Option<String>,
    /// 生成时间戳
    pub generated_at: f64,
}

/// 关系日志引擎
pub struct RelationshipLogEngine {
    inner: RwLock<RelationshipLogInner>,
    persistence_path: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RelationshipLogInner {
    /// 逐轮日志（按时间升序，最多保留近期 N 条）
    entries: Vec<RelationshipLogEntry>,
    /// 每日摘要（按日期升序）
    daily_summaries: Vec<RelationshipDailySummary>,
}

/// 逐轮日志保留上限（超出则 FIFO 淘汰最早条目）
const MAX_ENTRIES: usize = 200;
/// 每日摘要保留上限
const MAX_DAILY_SUMMARIES: usize = 90;

static RELATIONSHIP_LOG_ENGINE: Lazy<Arc<RelationshipLogEngine>> = Lazy::new(|| {
    Arc::new(RelationshipLogEngine::new().unwrap_or_else(|e| {
        tracing::error!("[RelationshipLog] 引擎初始化失败，使用空状态: {e}");
        RelationshipLogEngine {
            inner: RwLock::new(RelationshipLogInner::default()),
            persistence_path: std::path::PathBuf::from("relationship_log.json"),
        }
    }))
});

impl RelationshipLogEngine {
    fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("psychology");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("relationship_log.json");
        let mut engine = Self {
            inner: RwLock::new(RelationshipLogInner::default()),
            persistence_path: path,
        };
        engine.load()?;
        Ok(engine)
    }

    fn load(&mut self) -> VivianResult<()> {
        if !self.persistence_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.persistence_path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let inner: RelationshipLogInner = serde_json::from_str(&content).map_err(|e| {
            VivianError::Other(format!("relationship_log.json 解析失败: {e}"))
        })?;
        *self.inner.write() = inner;
        Ok(())
    }

    fn save_inner(inner: &RelationshipLogInner, path: &std::path::Path) -> VivianResult<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(inner)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 追加一条逐轮日志
    pub fn append_entry(&self, entry: RelationshipLogEntry) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.entries.push(entry);
        // FIFO 淘汰
        if inner.entries.len() > MAX_ENTRIES {
            let drop_n = inner.entries.len() - MAX_ENTRIES;
            inner.entries.drain(0..drop_n);
        }
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 查询近期 N 条逐轮日志（按时间倒序）
    pub fn recent_entries(&self, n: usize) -> Vec<RelationshipLogEntry> {
        let inner = self.inner.read();
        let len = inner.entries.len();
        let start = len.saturating_sub(n);
        let v: Vec<RelationshipLogEntry> = inner.entries[start..].iter().rev().cloned().collect();
        v
    }

    /// 查询指定日期的所有日志
    pub fn entries_on_date(&self, date: &str) -> Vec<RelationshipLogEntry> {
        let inner = self.inner.read();
        inner
            .entries
            .iter()
            .filter(|e| e.date == date)
            .cloned()
            .collect()
    }

    /// 写入或更新某日的摘要
    pub fn upsert_daily_summary(&self, summary: RelationshipDailySummary) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if let Some(existing) = inner
            .daily_summaries
            .iter_mut()
            .find(|s| s.date == summary.date)
        {
            *existing = summary;
        } else {
            inner.daily_summaries.push(summary);
            inner.daily_summaries.sort_by(|a, b| a.date.cmp(&b.date));
            if inner.daily_summaries.len() > MAX_DAILY_SUMMARIES {
                let drop_n = inner.daily_summaries.len() - MAX_DAILY_SUMMARIES;
                inner.daily_summaries.drain(0..drop_n);
            }
        }
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 查询近期 N 天的每日摘要（按日期倒序）
    pub fn recent_daily_summaries(&self, n: usize) -> Vec<RelationshipDailySummary> {
        let inner = self.inner.read();
        let len = inner.daily_summaries.len();
        let start = len.saturating_sub(n);
        inner.daily_summaries[start..]
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    /// 生成可注入 prompt 的近期关系上下文段
    ///
    /// 输出近期几轮的关系线索和最近几天的摘要，让 Vivian 的回应贴合关系演化轨迹。
    pub fn build_context(&self, recent_turns: usize, recent_days: usize, lang: &str) -> String {
        let inner = self.inner.read();
        if inner.entries.is_empty() && inner.daily_summaries.is_empty() {
            return String::new();
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (recent_turns_label, recent_days_label) = match lang_norm {
            "en" => ("Recent turns", "Recent daily summaries"),
            "ja" => ("最近のターン", "最近の日次サマリー"),
            _ => ("近期轮次", "近期每日摘要"),
        };

        let mut lines: Vec<String> = Vec::new();
        let header = crate::pipeline::prompt_modules::section_heading("recent_relationship_cues", lang);
        lines.push(header.to_string());

        // 近期逐轮线索
        let entries_len = inner.entries.len();
        let start = entries_len.saturating_sub(recent_turns);
        let recent: &[RelationshipLogEntry] = &inner.entries[start..];
        if !recent.is_empty() {
            lines.push(format!("- {}:", recent_turns_label));
            for e in recent.iter().rev() {
                // 区分 UserAgent 和 AgentAgent 方向，避免 LLM 混淆
                let direction_tag = match e.direction {
                    crate::psychology::relationship_log::RelationshipDirection::AgentAgent => {
                        let target = e.target_agent_id.as_deref().unwrap_or("?");
                        format!("[AgentAgent→{}]", target)
                    }
                    crate::psychology::relationship_log::RelationshipDirection::UserAgent => {
                        "[UserAgent]".to_string()
                    }
                };
                // AgentAgent 方向无 user_mood，省略该字段避免空值误导
                let mood_part = match e.direction {
                    crate::psychology::relationship_log::RelationshipDirection::AgentAgent => {
                        String::new()
                    }
                    crate::psychology::relationship_log::RelationshipDirection::UserAgent => {
                        format!(", mood={}", e.user_mood)
                    }
                };
                let mut s = format!(
                    "  · {} [{}] signal={}",
                    direction_tag, e.date, e.relationship_signal
                );
                s.push_str(&mood_part);
                if let Some(m) = &e.important_moment {
                    if !m.is_empty() {
                        s.push_str(&format!(", moment={}", m));
                    }
                }
                if !e.next_care_cue.is_empty() {
                    s.push_str(&format!(", next_cue={}", e.next_care_cue));
                }
                lines.push(s);
            }
        }

        // 近期每日摘要
        let days_len = inner.daily_summaries.len();
        let d_start = days_len.saturating_sub(recent_days);
        let recent_days_slice: &[RelationshipDailySummary] =
            &inner.daily_summaries[d_start..];
        if !recent_days_slice.is_empty() {
            lines.push(format!("- {}:", recent_days_label));
            for d in recent_days_slice.iter().rev() {
                let mut s = format!(
                    "  · [{}] mood={}, signal={}",
                    d.date, d.dominant_mood, d.signal_summary
                );
                if let Some(h) = &d.highlight {
                    if !h.is_empty() {
                        s.push_str(&format!(", highlight={}", h));
                    }
                }
                lines.push(s);
            }
        }

        lines.join("\n")
    }

    /// 清空全部关系日志
    pub fn clear(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.entries.clear();
        inner.daily_summaries.clear();
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 尝试为指定日期生成每日摘要（基于当天已有的逐轮日志）
    ///
    /// 返回 Some(summary) 表示生成成功，None 表示当天无日志或不足。
    pub fn try_generate_daily_summary(&self, date: &str) -> Option<RelationshipDailySummary> {
        let inner = self.inner.read();
        let day_entries: Vec<&RelationshipLogEntry> = inner
            .entries
            .iter()
            .filter(|e| e.date == date)
            .collect();

        if day_entries.is_empty() {
            return None;
        }

        // 主导情绪：出现次数最多的 mood
        let mut mood_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for e in &day_entries {
            *mood_counts.entry(e.user_mood.as_str()).or_insert(0) += 1;
        }
        let dominant_mood = mood_counts
            .iter()
            .max_by_key(|(_, c)| *c)
            .map(|(m, _)| m.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // 关系信号聚合：收集所有非空 signal，去重后拼接
        let mut signals: Vec<&str> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for e in &day_entries {
            if !e.relationship_signal.is_empty() && seen.insert(e.relationship_signal.as_str()) {
                signals.push(e.relationship_signal.as_str());
            }
        }
        let signal_summary = if signals.is_empty() {
            "—".to_string()
        } else {
            signals.join(", ")
        };

        // 重要时刻：取最后一条非空 important_moment
        let highlight = day_entries
            .iter()
            .rev()
            .find_map(|e| e.important_moment.clone().filter(|s| !s.is_empty()));

        Some(RelationshipDailySummary {
            date: date.to_string(),
            turn_count: day_entries.len(),
            dominant_mood,
            signal_summary,
            highlight,
            generated_at: Utc::now().timestamp() as f64,
        })
    }
}

/// 获取全局关系日志引擎
pub fn relationship_log() -> Arc<RelationshipLogEngine> {
    Arc::clone(&RELATIONSHIP_LOG_ENGINE)
}

/// 当前日期字符串（YYYY-MM-DD，本地时区）
pub fn today_date_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// 从时间戳生成日期字符串
pub fn date_str_from_ts(ts: f64) -> String {
    let dt: DateTime<Utc> = DateTime::<Utc>::from_timestamp(ts as i64, 0)
        .unwrap_or_else(Utc::now);
    let local: DateTime<Local> = dt.with_timezone(&Local);
    local.format("%Y-%m-%d").to_string()
}

/// 昨日日期字符串（YYYY-MM-DD，本地时区）
pub fn yesterday_date_str() -> String {
    let today = Local::now().date_naive();
    today
        .pred_opt()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.format("%Y-%m-%d").to_string())
}
