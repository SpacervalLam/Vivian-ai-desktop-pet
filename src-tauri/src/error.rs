use thiserror::Error;

#[derive(Error, Debug)]
pub enum VivianError {
    #[error("配置错误: {0}")]
    Config(String),

    #[error("AI 提供商错误: {0}")]
    Provider(String),

    #[error("网络请求失败: {0}")]
    Network(String),

    #[error("工具执行错误: {0}")]
    Tool(String),

    #[error("权限拒绝: {0}")]
    Permission(String),

    #[error("记忆系统错误: {0}")]
    Memory(String),

    #[error("沙箱安全检查未通过: {0}")]
    Sandbox(String),

    #[error("熔断器已打开: {0}")]
    CircuitBreaker(String),

    #[error("超时: {0}")]
    Timeout(String),

    #[error("序列化错误: {0}")]
    Serialization(String),

    #[error("数据库错误: {0}")]
    Database(String),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("引擎错误: {0}")]
    Engine(String),

    #[error("语音识别错误: {0}")]
    Speech(String),

    #[error("功能尚未实现: {0}")]
    NotImplemented(String),

    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for VivianError {
    fn from(e: reqwest::Error) -> Self {
        VivianError::Network(e.to_string())
    }
}

impl From<rusqlite::Error> for VivianError {
    fn from(e: rusqlite::Error) -> Self {
        VivianError::Database(e.to_string())
    }
}

impl Serialize for VivianError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

use serde::Serialize;

pub type VivianResult<T> = Result<T, VivianError>;
