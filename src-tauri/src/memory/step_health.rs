//! 步骤健康跟踪 —— 长时后台任务的每步健康状态 + 错误去重 + 原子持久化
//!
//! - **步骤级健康状态**：每个步骤独立记录 last_success_at / last_error_at /
//!   last_error_msg / fail_count（连续失败才递增，一次成功立即清零），
//!   供 UI / 诊断接口读取"记忆系统现在健康吗"。
//! - **错误去重**：相同根因（步骤+错误消息签名）只打一次 error 日志，
//!   恢复时打一条"恢复正常"。避免凭证失效之类的持续故障每轮刷屏。
//! - **原子写入**：tmp + rename，崩溃时不会留下半截状态文件。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 单个步骤的健康状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepHealth {
    /// 上次成功时间（ISO 8601，None 表示从未成功）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
    /// 上次失败时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
    /// 上次失败消息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_msg: Option<String>,
    /// 连续失败次数（一次成功立即清零）
    #[serde(default)]
    pub fail_count: u32,
    /// 熔断暂停原因（连续失败达到阈值时写入；成功或半开恢复时清除）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
    /// 滑动窗口样本（最近 N 次成败，成功也计入——错误率口径的关键）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_results: Vec<WindowSample>,
}

/// 滑动窗口中的单次执行样本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSample {
    /// 执行时刻（RFC 3339）
    pub t: String,
    /// 是否成功
    pub ok: bool,
}

impl Default for StepHealth {
    fn default() -> Self {
        Self {
            last_success_at: None,
            last_error_at: None,
            last_error_msg: None,
            fail_count: 0,
            paused_reason: None,
            recent_results: Vec::new(),
        }
    }
}

/// 连续失败熔断阈值：达到后该步骤进入 paused 状态（快路径：彻底失败）
pub const CIRCUIT_BREAK_THRESHOLD: u32 = 5;

/// 滑动窗口容量：记录最近 N 次成败样本
const WINDOW_SIZE: usize = 20;

/// 错误率熔断的最小样本数：窗口不满时只用连续失败快路径，避免小样本误判
const WINDOW_MIN_SAMPLES: usize = 5;

/// 错误率熔断阈值：窗口内失败占比达到即熔断（慢路径：半死不活状态）
const WINDOW_ERROR_RATE_THRESHOLD: f64 = 0.6;

/// 追加窗口样本并裁剪到容量上限
fn push_window(h: &mut StepHealth, ok: bool) {
    h.recent_results.push(WindowSample {
        t: chrono::Utc::now().to_rfc3339(),
        ok,
    });
    let len = h.recent_results.len();
    if len > WINDOW_SIZE {
        h.recent_results.drain(..len - WINDOW_SIZE);
    }
}

/// 窗口统计：(失败数, 总样本数)
fn window_stats(h: &StepHealth) -> (usize, usize) {
    let total = h.recent_results.len();
    let fails = h.recent_results.iter().filter(|s| !s.ok).count();
    (fails, total)
}

/// 窗口错误率（样本不足最小采样数时返回 0，即不参与熔断判断）
fn window_error_rate(h: &StepHealth) -> f64 {
    let (fails, total) = window_stats(h);
    if total < WINDOW_MIN_SAMPLES {
        return 0.0;
    }
    fails as f64 / total as f64
}

/// 持久化文件 schema 版本：结构变更时递增，旧版本读取失败走重置
const SCHEMA_VERSION: u32 = 1;

/// 持久化状态
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    schema_version: u32,
    steps: HashMap<String, StepHealth>,
    updated_at: String,
}

/// 步骤健康跟踪器
///
/// 每个长时任务（如记忆巩固）持有一个实例，按步骤名上报成败。
/// 状态持久化到用户数据目录，重启后恢复（仅用于观测，不用于断点控制——
/// 各流水线自带幂等触发条件）。
pub struct StepHealthTracker {
    steps: Mutex<HashMap<String, StepHealth>>,
    /// 上次打过 error 的签名（步骤名|错误消息），同签名重复只进 debug 日志
    last_error_sig: Mutex<Option<String>>,
    persist_path: Option<PathBuf>,
}

impl StepHealthTracker {
    /// 创建跟踪器并从磁盘恢复（文件缺失/损坏/schema 不符时静默重置）
    pub fn load(persist_path: Option<PathBuf>) -> Self {
        let steps = persist_path.as_ref().and_then(|p| Self::read_state(p)).unwrap_or_default();
        Self {
            steps: Mutex::new(steps),
            last_error_sig: Mutex::new(None),
            persist_path,
        }
    }

    fn read_state(path: &Path) -> Option<HashMap<String, StepHealth>> {
        let state: PersistedState = crate::utils::fs::load_json_or_backup(path)?;
        if state.schema_version != SCHEMA_VERSION {
            return None;
        }
        Some(state.steps)
    }

    fn persist(&self) {
        let Some(path) = &self.persist_path else { return };
        let state = PersistedState {
            schema_version: SCHEMA_VERSION,
            steps: self.steps.lock().clone(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = atomic_write(path, &text) {
            tracing::debug!("[StepHealth] 状态持久化失败: {e}");
        }
    }

    /// 步骤成功：清零连续失败计数与熔断暂停，恢复通知（若之前有故障）
    pub fn mark_success(&self, step: &str) {
        let recovered = {
            let mut steps = self.steps.lock();
            let h = steps.entry(step.to_string()).or_default();
            let was_failing = h.fail_count > 0 || h.paused_reason.is_some();
            h.last_success_at = Some(chrono::Utc::now().to_rfc3339());
            h.last_error_at = None;
            h.last_error_msg = None;
            h.fail_count = 0;
            h.paused_reason = None;
            push_window(h, true);
            was_failing
        };
        if recovered {
            // 步骤从故障中恢复，打一条恢复日志
            let prev_sig = self.last_error_sig.lock().clone();
            let prev = prev_sig.unwrap_or_default();
            *self.last_error_sig.lock() = None;
            tracing::info!("[StepHealth] `{step}` 恢复正常（此前故障: {prev}）");
        }
        self.persist();
    }

    /// 步骤失败：递增连续失败计数并推进滑动窗口；同根因签名只打一次 error；
    /// 连续失败达到阈值（快路径）或窗口错误率超标（慢路径）时写入熔断暂停原因
    /// （半开恢复由 [`Self::try_resume`] 控制）
    pub fn mark_failure(&self, step: &str, error: &str) {
        {
            let mut steps = self.steps.lock();
            let h = steps.entry(step.to_string()).or_default();
            h.last_error_at = Some(chrono::Utc::now().to_rfc3339());
            h.last_error_msg = Some(error.to_string());
            h.fail_count += 1;
            push_window(h, false);
            let consecutive = h.fail_count >= CIRCUIT_BREAK_THRESHOLD;
            let rate_exceeded = window_error_rate(h) >= WINDOW_ERROR_RATE_THRESHOLD;
            if (consecutive || rate_exceeded) && h.paused_reason.is_none() {
                let reason = if consecutive {
                    format!(
                        "连续失败 {fail} 次，已熔断暂停（排查后自动恢复）: {error}",
                        fail = h.fail_count
                    )
                } else {
                    let (fails, total) = window_stats(h);
                    format!(
                        "近期错误率 {fails}/{total}，已熔断暂停（排查后自动恢复）: {error}"
                    )
                };
                h.paused_reason = Some(reason);
                tracing::error!(
                    "[StepHealth] `{step}` 熔断暂停（连续 {} 次，窗口错误率 {:.0}%）: {error}",
                    h.fail_count,
                    window_error_rate(h) * 100.0
                );
            }
        }
        // 错误去重：相同（步骤|消息）签名只在首次打 error，重复只进 debug
        let sig = format!("{step}|{error}");
        let mut last_sig = self.last_error_sig.lock();
        if *last_sig != Some(sig.clone()) {
            *last_sig = Some(sig);
            drop(last_sig);
            tracing::error!("[StepHealth] `{step}` 失败: {error}");
        } else {
            drop(last_sig);
            tracing::debug!("[StepHealth] `{step}` 失败（同根因已上报，抑制重复）: {error}");
        }
        self.persist();
    }

    /// 是否有任一步骤处于熔断暂停状态
    pub fn any_paused(&self) -> bool {
        self.steps.lock().values().any(|h| h.paused_reason.is_some())
    }

    /// 指定步骤是否处于熔断暂停状态（返回暂停原因）
    pub fn is_paused(&self, step: &str) -> Option<String> {
        self.steps
            .lock()
            .get(step)
            .and_then(|h| h.paused_reason.clone())
    }

    /// 半开恢复：暂停中的步骤距上次失败超过 `cooldown_secs` 时清除暂停标记，
    /// 允许下一次尝试（失败会重新计数并可能再次熔断）。
    ///
    /// 返回本次是否有步骤被解除暂停。
    pub fn try_resume(&self, cooldown_secs: f64) -> bool {
        let now = chrono::Utc::now();
        let mut resumed_any = false;
        {
            let mut steps = self.steps.lock();
            for (name, h) in steps.iter_mut() {
                if h.paused_reason.is_none() {
                    continue;
                }
                let elapsed = h
                    .last_error_at
                    .as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds() as f64)
                    .unwrap_or(f64::MAX);
                if elapsed >= cooldown_secs {
                    h.paused_reason = None;
                    resumed_any = true;
                    tracing::info!(
                        "[StepHealth] `{name}` 熔断暂停已超过 {cooldown_secs:.0}s，半开重试"
                    );
                }
            }
        }
        if resumed_any {
            self.persist();
        }
        resumed_any
    }

    /// 健康快照（深拷贝，调用方安全持有）
    pub fn snapshot(&self) -> HashMap<String, StepHealth> {
        self.steps.lock().clone()
    }

    /// 是否全部步骤健康（无连续失败）
    pub fn is_healthy(&self) -> bool {
        self.steps.lock().values().all(|h| h.fail_count == 0)
    }
}

/// 原子写入：先写临时文件再 rename，崩溃不会留下半截文件
pub fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows 上 rename 覆盖已存在文件可能失败，回退 remove+rename
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_lifecycle() {
        let dir = std::env::temp_dir().join(format!("vivian-health-{}", uuid::Uuid::new_v4()));
        let path = dir.join("health.json");
        let tracker = StepHealthTracker::load(Some(path.clone()));

        tracker.mark_failure("stage1", "LLM 超时");
        tracker.mark_failure("stage1", "LLM 超时"); // 同签名去重
        assert_eq!(tracker.snapshot()["stage1"].fail_count, 2);
        assert!(!tracker.is_healthy());

        tracker.mark_success("stage1");
        assert_eq!(tracker.snapshot()["stage1"].fail_count, 0);
        assert!(tracker.is_healthy());

        // 持久化 + 重新加载恢复
        drop(tracker);
        let reloaded = StepHealthTracker::load(Some(path));
        assert!(reloaded.snapshot()["stage1"].last_success_at.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_error_rate_breaks_flaky_steps() {
        // 交替失败/成功：连续计数永远不超过 1，但窗口错误率超阈值也应熔断
        let dir = std::env::temp_dir().join(format!("vivian-health-{}", uuid::Uuid::new_v4()));
        let path = dir.join("health.json");
        let tracker = StepHealthTracker::load(Some(path.clone()));

        for i in 0..10 {
            tracker.mark_failure("flaky", "上游 502");
            if i < 6 {
                tracker.mark_success("flaky"); // 偶发成功清零连续计数
            }
        }
        let snap = tracker.snapshot()["flaky"].clone();
        // 交替成败下连续计数被成功不断清零，熔断只能来自错误率路径
        assert!(snap.paused_reason.is_some(), "窗口错误率超标应熔断: {snap:?}");
        assert!(
            snap.fail_count < CIRCUIT_BREAK_THRESHOLD,
            "连续计数未达快路径阈值，熔断来自错误率路径: {}",
            snap.fail_count
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn occasional_failure_does_not_break() {
        // 偶发失败（错误率低于阈值）不熔断
        let dir = std::env::temp_dir().join(format!("vivian-health-{}", uuid::Uuid::new_v4()));
        let path = dir.join("health.json");
        let tracker = StepHealthTracker::load(Some(path.clone()));

        for _ in 0..20 {
            tracker.mark_success("steady");
            tracker.mark_failure("steady", "偶发抖动");
        }
        let snap = tracker.snapshot()["steady"].clone();
        // 窗口 20 条中约一半失败：60% 边界附近不误熔（实际 10/20=50% < 60%）
        assert!(snap.paused_reason.is_none(), "错误率未超阈值不应熔断: {snap:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_roundtrip() {
        let dir = std::env::temp_dir().join(format!("vivian-atomic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        atomic_write(&path, "{\"a\":1}").unwrap();
        atomic_write(&path, "{\"a\":2}").unwrap(); // 覆盖已有文件
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
