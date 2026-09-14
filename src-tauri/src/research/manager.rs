//! ResearchManager：研究任务的创建、样本收集、统计聚合与结论生成。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde_json::Value;

use super::stats;
use super::storage;
use super::task::{Conclusion, ResearchTask, ResearchTaskView, Sample, TaskStatus};

/// 记录结果（返回给工具调用方）
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordOutcome {
    /// 是否为新创建的任务
    pub created: bool,
    /// 当前样本数
    pub sample_count: usize,
    /// 当前状态
    pub status: String,
    /// 是否刚得出结论
    pub just_concluded: bool,
    /// 结论置信度（有结论时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// 结论摘要（有结论时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// 用户研究管理器
pub struct ResearchManager {
    tasks: RwLock<HashMap<String, ResearchTask>>,
    storage_path: PathBuf,
}

impl ResearchManager {
    /// 创建并加载持久化数据
    pub fn new(char_id: &str) -> Self {
        let storage_path = crate::utils::path::get_character_data_dir(char_id)
            .join("research")
            .join("tasks.json");
        let tasks = storage::load(&storage_path);
        Self {
            tasks: RwLock::new(tasks),
            storage_path,
        }
    }

    /// 记录一条观察样本
    pub fn record_observation(
        &self,
        target: &str,
        observation: &str,
        data: Value,
        source_text: &str,
    ) -> RecordOutcome {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut tasks = self.tasks.write();

        let created = !tasks.contains_key(target);
        if created {
            let task = ResearchTask::new(
                target.to_string(),
                observation.to_string(),
                now,
            );
            tasks.insert(target.to_string(), task);
        }

        let task = tasks.get_mut(target).unwrap();

        // 已结论的任务：检查新样本是否偏离 >2 sigma，若是则重新开启
        let mut reopened = false;
        if task.status == TaskStatus::Concluded {
            let should_reopen = if task.conclusion.is_some() {
                if let Some(time_value) = Self::extract_time_hours(&data) {
                    let existing_hours = Self::collect_time_hours(task);
                    if let Some(circular) = stats::circular_mean_stddev(&existing_hours) {
                        stats::deviates_2sigma(time_value, circular.mean_hour, circular.stddev_hours)
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if should_reopen {
                let prev_confidence = task.conclusion.as_ref().map(|c| c.confidence).unwrap_or(0.0);
                task.status = TaskStatus::Active;
                task.conclusion = None;
                reopened = true;
                tracing::info!(
                    "[Research] task {} reopened due to deviation (confidence was {:.2})",
                    target,
                    prev_confidence
                );
            }
        }

        // 追加样本
        let sample = Sample {
            timestamp: now,
            observation: observation.to_string(),
            data,
            source_text: source_text.to_string(),
        };
        task.push_sample(sample);

        // 尝试聚合得出结论
        let mut just_concluded = false;
        if task.status == TaskStatus::Active && !reopened {
            if let Some(conclusion) = Self::try_conclude(task) {
                task.status = TaskStatus::Concluded;
                task.conclusion = Some(conclusion);
                just_concluded = true;
            }
        }

        let outcome = RecordOutcome {
            created,
            sample_count: task.samples.len(),
            status: match task.status {
                TaskStatus::Active => "active".to_string(),
                TaskStatus::Paused => "paused".to_string(),
                TaskStatus::Concluded => "concluded".to_string(),
            },
            just_concluded,
            confidence: task.conclusion.as_ref().map(|c| c.confidence),
            summary: task.conclusion.as_ref().map(|c| c.summary.clone()),
        };

        // 持久化
        storage::save(&self.storage_path, &tasks);

        outcome
    }

    /// 构建注入 prompt 的段落
    pub fn build_prompt_section(&self, lang: &str) -> Option<String> {
        let tasks = self.tasks.read();
        if tasks.is_empty() {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (active_header, concluded_header) = match lang_norm {
            "en" => ("Ongoing Observations", "Confirmed Habits"),
            "ja" => ("進行中の観察", "確認された習慣"),
            _ => ("进行中的观察", "已确认习惯"),
        };

        let mut active_lines: Vec<String> = Vec::new();
        let mut concluded_lines: Vec<String> = Vec::new();

        for task in tasks.values() {
            match task.status {
                TaskStatus::Active | TaskStatus::Paused => {
                    active_lines.push(format!("- {}", task.goal));
                }
                TaskStatus::Concluded => {
                    if let Some(ref c) = task.conclusion {
                        concluded_lines.push(format!("- {}", c.summary));
                    }
                }
            }
        }

        if active_lines.is_empty() && concluded_lines.is_empty() {
            return None;
        }

        let mut section = String::new();

        if !active_lines.is_empty() {
            section.push_str(&format!("[{}]\n", active_header));
            for line in &active_lines {
                section.push_str(line);
                section.push('\n');
            }
        }

        if !concluded_lines.is_empty() {
            if !section.is_empty() {
                section.push('\n');
            }
            section.push_str(&format!("[{}]\n", concluded_header));
            for line in &concluded_lines {
                section.push_str(line);
                section.push('\n');
            }
        }

        Some(section)
    }

    /// 获取所有任务的视图快照（供前端命令使用）
    pub fn tasks_snapshot(&self) -> Vec<ResearchTaskView> {
        let tasks = self.tasks.read();
        let mut views: Vec<ResearchTaskView> = tasks.values().map(|t| t.to_view()).collect();
        views.sort_by(|a, b| b.last_sample_at.partial_cmp(&a.last_sample_at).unwrap_or(std::cmp::Ordering::Equal));
        views
    }

    /// 从样本 data 中提取时间（小时浮点数）
    fn extract_time_hours(data: &Value) -> Option<f64> {
        if let Some(time_str) = data.get("time").and_then(|v| v.as_str()) {
            return stats::parse_time_to_hours(time_str);
        }
        if let Some(hour) = data.get("hour").and_then(|v| v.as_f64()) {
            if hour >= 0.0 && hour < 24.0 {
                return Some(hour);
            }
        }
        None
    }

    /// 收集任务中所有样本的时间值（小时）
    fn collect_time_hours(task: &ResearchTask) -> Vec<f64> {
        task.samples
            .iter()
            .filter_map(|s| Self::extract_time_hours(&s.data))
            .collect()
    }

    /// 收集任务中所有样本的时长值（分钟）
    fn collect_duration_minutes(task: &ResearchTask) -> Vec<f64> {
        task.samples
            .iter()
            .filter_map(|s| s.data.get("duration_min").and_then(|v| v.as_f64()))
            .collect()
    }

    /// 尝试从样本中得出结论
    fn try_conclude(task: &ResearchTask) -> Option<Conclusion> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        // 优先尝试时间型聚合
        let time_hours = Self::collect_time_hours(task);
        if time_hours.len() >= 5 {
            if let Some(circular) = stats::circular_mean_stddev(&time_hours) {
                let confidence = stats::compute_confidence(
                    time_hours.len(),
                    circular.resultant_length,
                );
                if stats::should_conclude(confidence, time_hours.len()) {
                    let summary = format!(
                        "usually around {} (+/- {:.0} min)",
                        stats::format_hour(circular.mean_hour),
                        circular.stddev_hours * 60.0
                    );
                    return Some(Conclusion {
                        summary,
                        confidence,
                        computed_at: now,
                    });
                }
            }
        }

        // 尝试时长型聚合
        let durations = Self::collect_duration_minutes(task);
        if durations.len() >= 5 {
            if let Some(linear) = stats::linear_mean_stddev(&durations) {
                let concentration = if linear.mean > 0.0 {
                    (1.0 - linear.stddev / linear.mean).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let confidence = stats::compute_confidence(durations.len(), concentration);
                if stats::should_conclude(confidence, durations.len()) {
                    let summary = format!(
                        "usually lasts about {:.0} min (+/- {:.0} min)",
                        linear.mean,
                        linear.stddev
                    );
                    return Some(Conclusion {
                        summary,
                        confidence,
                        computed_at: now,
                    });
                }
            }
        }

        None
    }
}
