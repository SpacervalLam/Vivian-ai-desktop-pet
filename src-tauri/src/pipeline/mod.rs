//! Vivian Brain Pipeline —— 可组合的对话处理管道。
//!
//! - [`base`]：Runnable trait / RunnableConfig / StreamEvent / 组合原语
//! - [`advisor`]：Advisor 拦截器链（日志/限流/Re2/循环检测）
//! - [`state`]：PipelineState（53 字段）
//! - [`errors`]：PipelineError / StageTimeoutError / StageExecutionError
//! - [`steps`]：各阶段 Runnable（pre_processing / memory / mood / prompt / generation）
//! - [`prompt_modules`]：模块化 Prompt 构建器
//! - [`parsers`]：输出解析器（JsonOutputParser / SchemaOutputParser / StateFieldParser / StreamingOutputParser）
//!
//! 高层管道封装见 [`crate::brain::chat_chain::BrainChatChain`]。

pub mod advisor;
pub mod base;
pub mod compaction_reminder;
pub mod context_compress;
pub mod decorators;
pub mod doom_loop;
pub mod errors;
pub mod inline_tag_scanner;
pub mod injection_guard;
pub mod parsers;
pub mod prompt_modules;
pub mod state;
pub mod steps;
pub mod template_engine;
pub mod topic_injection;

// 基础组件
pub use base::{
    Runnable, RunnableConfig, RunnableLambda, RunnableParallel, RunnableSequence, StreamEvent,
    TimingMiddleware,
};

// 装饰器组合子
pub use decorators::{
    compute_backoff, is_retryable, RunnableBranch, RunnableDecorators, RunnableRetry,
    RunnableWithFallbacks,
};

// 错误类型
pub use errors::{PipelineError, StageExecutionError, StageTimeoutError};

// 状态
pub use state::PipelineState;

// 解析器
pub use parsers::{
    BaseOutputParser, JsonOutputParser, SchemaOutputParser, StateFieldParser, StreamingOutputParser,
};

// 话题注入
pub use topic_injection::{InjectionTopic, TopicInjectionManager};
