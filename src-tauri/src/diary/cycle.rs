//! Durable daily ordering: dream succeeds → diary is saved → existing diary growth.
use super::DiaryEntry;
use crate::error::{VivianError, VivianResult};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Weak},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamContext {
    pub report: crate::memory::consolidation::DreamReport,
    pub material: Vec<serde_json::Value>,
}

pub fn is_judgment_time(hour: u32) -> bool {
    (20..24).contains(&hour)
}
pub fn may_offer_to_planner(hour: u32, material: usize, threshold: usize) -> bool {
    is_judgment_time(hour) && material >= threshold.max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Dreaming,
    Diary,
    Complete,
}

#[derive(Debug, Serialize, Deserialize)]
struct Checkpoint {
    date: NaiveDate,
    phase: Phase,
    dream: Option<DreamContext>,
    attempts: u32,
    retry_at: i64,
    last_error: Option<String>,
}

#[async_trait]
pub trait CycleWork: Sync {
    fn existing(&self, date: NaiveDate) -> VivianResult<Option<DiaryEntry>>;
    async fn dream(&self, date: NaiveDate) -> VivianResult<DreamContext>;
    async fn diary(
        &self,
        date: NaiveDate,
        trigger: &str,
        dream: &DreamContext,
    ) -> VivianResult<DiaryEntry>;
}

static GATES: once_cell::sync::Lazy<
    parking_lot::Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
> = once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));

pub struct DiaryCycle {
    root: PathBuf,
}
impl DiaryCycle {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn path(&self, date: NaiveDate) -> PathBuf {
        self.root.join(format!("{date}.json"))
    }
    fn read(&self, date: NaiveDate) -> VivianResult<Option<Checkpoint>> {
        let path = self.path(date);
        if !path.exists() {
            return Ok(None);
        }
        let checkpoint: Checkpoint = crate::utils::fs::load_json_or_backup(&path)
            .ok_or_else(|| VivianError::Memory("做梦进度读取失败，已保留原文件".into()))?;
        if checkpoint.date != date {
            return Err(VivianError::Memory("做梦进度日期不匹配".into()));
        }
        Ok(Some(checkpoint))
    }
    fn save(&self, checkpoint: &Checkpoint) -> VivianResult<()> {
        std::fs::create_dir_all(&self.root)?;
        crate::utils::fs::write_atomic(
            &self.path(checkpoint.date),
            &serde_json::to_string(checkpoint)?,
        )?;
        Ok(())
    }
    fn complete(&self, state: &mut Checkpoint) -> VivianResult<()> {
        state.phase = Phase::Complete;
        state.retry_at = 0;
        state.last_error = None;
        self.save(state)?;
        // The diary embeds the completed dream. Only unfinished cycles need a sidecar.
        if let Err(error) = std::fs::remove_file(self.path(state.date)) {
            tracing::warn!("[DreamDiary] 完成进度清理失败: {error}");
        }
        Ok(())
    }
    pub fn acknowledge_existing(&self, date: NaiveDate) -> VivianResult<()> {
        let gate = self.gate();
        let _guard = gate
            .try_lock()
            .map_err(|_| VivianError::Memory("正在写日记，稍后收尾".into()))?;
        if let Some(mut state) = self.read(date)? {
            self.complete(&mut state)?;
        }
        Ok(())
    }
    pub fn ready(&self, date: NaiveDate, now: i64) -> VivianResult<bool> {
        Ok(self.read(date)?.is_none_or(|state| now >= state.retry_at))
    }
    /// Record a qualified catch-up request. Only the environment agent may start it automatically.
    pub fn request(&self, date: NaiveDate) -> VivianResult<()> {
        let gate = self.gate();
        let _guard = gate
            .try_lock()
            .map_err(|_| VivianError::Memory("正在处理日记安排".into()))?;
        if self.read(date)?.is_some() {
            return Ok(());
        }
        self.save(&Checkpoint {
            date,
            phase: Phase::Dreaming,
            dream: None,
            attempts: 0,
            retry_at: 0,
            last_error: None,
        })
    }
    /// Interrupted work may resume after midnight or restart without reapplying today's threshold.
    pub fn pending_date(&self, now: i64) -> VivianResult<Option<NaiveDate>> {
        Ok(self.pending_dates(now)?.into_iter().next())
    }
    pub fn pending_dates(&self, now: i64) -> VivianResult<Vec<NaiveDate>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut dates = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Some(date) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            else {
                continue;
            };
            match self.read(date) {
                Ok(Some(state)) if state.phase != Phase::Complete && now >= state.retry_at => {
                    dates.push(date)
                }
                Err(error) => {
                    tracing::warn!(%date, "[DreamDiary] 单个安排进度不可读，保留文件: {error}")
                }
                _ => {}
            }
        }
        dates.sort();
        Ok(dates)
    }
    fn gate(&self) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = GATES.lock();
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(&self.root).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(self.root.clone(), Arc::downgrade(&gate));
        gate
    }
    pub async fn run<W: CycleWork>(
        &self,
        date: NaiveDate,
        trigger: &str,
        work: &W,
    ) -> VivianResult<DiaryEntry> {
        let gate = self.gate();
        let _guard = gate
            .try_lock()
            .map_err(|_| VivianError::Memory("正在做梦或写日记，请稍后重试".into()))?;
        if let Some(entry) = work.existing(date)? {
            if let Ok(Some(mut state)) = self.read(date) {
                if let Err(error) = self.complete(&mut state) {
                    tracing::warn!("[DreamDiary] 日记已保存，进度收尾稍后重试: {error}");
                }
            }
            return Ok(entry);
        }
        let mut state = self.read(date)?.unwrap_or(Checkpoint {
            date,
            phase: Phase::Dreaming,
            dream: None,
            attempts: 0,
            retry_at: 0,
            last_error: None,
        });
        if matches!(trigger, "auto" | "catch_up") && chrono::Utc::now().timestamp() < state.retry_at
        {
            return Err(VivianError::Memory("做梦或日记正在等待重试".into()));
        }
        if state.dream.is_none() {
            state.phase = Phase::Dreaming;
            self.save(&state)?;
            tracing::info!(%date, "[DreamDiary] 开始整理、联想记忆");
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(600), work.dream(date))
                    .await
                    .unwrap_or_else(|_| {
                        Err(VivianError::Memory("做梦整理超时，保留分段供重试".into()))
                    });
            match result {
                Ok(dream) => {
                    state.dream = Some(dream);
                    state.attempts = 0;
                    state.phase = Phase::Diary;
                    self.save(&state)?;
                }
                Err(error) => {
                    self.failed(&mut state, &error)?;
                    return Err(error);
                }
            }
        }
        state.phase = Phase::Diary;
        self.save(&state)?;
        tracing::info!(%date, "[DreamDiary] 做梦结束，紧接着撰写日记");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            work.diary(date, trigger, state.dream.as_ref().unwrap()),
        )
        .await
        .unwrap_or_else(|_| {
            Err(VivianError::Memory(
                "日记撰写超时，下次从日记阶段继续".into(),
            ))
        });
        match result {
            Ok(entry) => {
                self.complete(&mut state)?;
                Ok(entry)
            }
            Err(error) => {
                self.failed(&mut state, &error)?;
                Err(error)
            }
        }
    }
    fn failed(&self, state: &mut Checkpoint, error: &VivianError) -> VivianResult<()> {
        state.attempts = state.attempts.saturating_add(1);
        state.retry_at =
            chrono::Utc::now().timestamp() + (30 * (1i64 << state.attempts.min(6))).min(1800);
        state.last_error = Some(error.to_string());
        self.save(state)
    }
}

/// Calendar boundaries use the local timezone, including 23/25-hour DST days.
pub fn day_bounds(date: NaiveDate) -> VivianResult<(i64, i64)> {
    use chrono::TimeZone;
    let midnight = |day: NaiveDate| {
        chrono::Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .map(|dt| dt.timestamp())
            .ok_or_else(|| VivianError::Memory("无法确定日记日期的本地时间边界".into()))
    };
    Ok((
        midnight(date)?,
        midnight(
            date.succ_opt()
                .ok_or_else(|| VivianError::Memory("日记日期越界".into()))?,
        )?,
    ))
}

pub fn cycle_for(char_id: &str) -> DiaryCycle {
    DiaryCycle::new(super::diary_dir(char_id).join("cycles"))
}
