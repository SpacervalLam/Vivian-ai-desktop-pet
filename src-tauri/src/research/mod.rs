//! 用户研究系统：LLM 驱动的行为习惯观察与统计聚合。

pub mod manager;
pub mod stats;
pub mod storage;
pub mod task;

pub use manager::{RecordOutcome, ResearchManager};
pub use task::{Conclusion, ResearchTask, ResearchTaskView, Sample, SampleView, TaskStatus};
