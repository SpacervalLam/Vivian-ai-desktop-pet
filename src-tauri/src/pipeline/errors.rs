use thiserror::Error;

#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("stage timeout")]
    StageTimeout,

    #[error("stage execution failed: {0}")]
    StageExecution(String),

    #[error("recoverable error: {0}")]
    Recoverable(String),
}

impl PipelineError {
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::Recoverable(_) | Self::StageTimeout => true,
            Self::StageExecution(_) => false,
        }
    }

    pub fn recoverable(message: impl Into<String>) -> Self {
        Self::Recoverable(message.into())
    }
}

#[derive(Debug, Clone)]
pub struct StageTimeoutError {
    pub stage: String,
    pub timeout_ms: u64,
}

impl StageTimeoutError {
    pub fn new(stage: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            stage: stage.into(),
            timeout_ms,
        }
    }
}

impl std::fmt::Display for StageTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stage '{}' timed out after {}ms", self.stage, self.timeout_ms)
    }
}

impl std::error::Error for StageTimeoutError {}

#[derive(Debug, Clone)]
pub struct StageExecutionError {
    pub stage: String,
    pub message: String,
}

impl StageExecutionError {
    pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StageExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stage '{}' failed: {}", self.stage, self.message)
    }
}

impl std::error::Error for StageExecutionError {}
