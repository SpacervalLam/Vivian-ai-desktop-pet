//! 用户研究任务数据模型。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 研究任务状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// 正在收集样本
    Active,
    /// 样本充足但置信度未达标，暂停收集
    Paused,
    /// 已得出稳定结论
    Concluded,
}

/// 单条观察样本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// 记录时刻（Unix epoch 秒）
    pub timestamp: f64,
    /// LLM 对本次观察的自然语言描述
    pub observation: String,
    /// 结构化数据（如 {"time":"22:41","duration_min":45}）
    #[serde(default)]
    pub data: Value,
    /// 触发本次记录的用户原话
    #[serde(default)]
    pub source_text: String,
}

/// 研究结论
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conclusion {
    /// 习惯摘要（如"通常23:00左右入睡"）
    pub summary: String,
    /// 置信度 0.0-1.0
    pub confidence: f64,
    /// 结论计算时刻（Unix epoch 秒）
    pub computed_at: f64,
}

/// 用户研究任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTask {
    /// 研究目标标识符（如 "sleep_schedule"）
    pub id: String,
    /// 研究目标描述（首次 observation 作为 goal）
    pub goal: String,
    /// 当前状态
    pub status: TaskStatus,
    /// 已收集的样本（上限 100，FIFO 淘汰最旧）
    pub samples: Vec<Sample>,
    /// 研究结论（Concluded 时有值）
    #[serde(default)]
    pub conclusion: Option<Conclusion>,
    /// 创建时刻（Unix epoch 秒）
    pub created_at: f64,
    /// 最近一次样本记录时刻
    pub last_sample_at: f64,
}

/// 前端展示用的任务视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTaskView {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub sample_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub created_at: f64,
    pub last_sample_at: f64,
    /// 最近样本（供展开查看）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_samples: Option<Vec<SampleView>>,
}

/// 前端展示用的样本视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleView {
    pub timestamp: f64,
    pub observation: String,
}

impl ResearchTask {
    /// 样本上限
    const MAX_SAMPLES: usize = 100;

    /// 创建新的活跃研究任务
    pub fn new(id: String, goal: String, now: f64) -> Self {
        Self {
            id,
            goal,
            status: TaskStatus::Active,
            samples: Vec::new(),
            conclusion: None,
            created_at: now,
            last_sample_at: now,
        }
    }

    /// 追加样本，超出上限时淘汰最旧
    pub fn push_sample(&mut self, sample: Sample) {
        self.last_sample_at = sample.timestamp;
        self.samples.push(sample);
        if self.samples.len() > Self::MAX_SAMPLES {
            self.samples.remove(0);
        }
    }

    /// 转为前端视图
    pub fn to_view(&self) -> ResearchTaskView {
        let recent_samples = if self.samples.is_empty() {
            None
        } else {
            let start = self.samples.len().saturating_sub(10);
            Some(
                self.samples[start..]
                    .iter()
                    .map(|s| SampleView {
                        timestamp: s.timestamp,
                        observation: s.observation.clone(),
                    })
                    .collect(),
            )
        };

        ResearchTaskView {
            id: self.id.clone(),
            goal: self.goal.clone(),
            status: match self.status {
                TaskStatus::Active => "active".to_string(),
                TaskStatus::Paused => "paused".to_string(),
                TaskStatus::Concluded => "concluded".to_string(),
            },
            sample_count: self.samples.len(),
            conclusion: self.conclusion.as_ref().map(|c| c.summary.clone()),
            confidence: self.conclusion.as_ref().map(|c| c.confidence),
            created_at: self.created_at,
            last_sample_at: self.last_sample_at,
            recent_samples,
        }
    }
}
