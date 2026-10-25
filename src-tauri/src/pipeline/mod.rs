//! Vivian Brain Pipeline —— 可组合的对话处理管道。
//!
//! - [`base`]：Runnable trait / RunnableConfig / StreamEvent / 组合原语
//! - [`advisor`]：Advisor 拦截器链（日志/限流/Re2/循环检测）
//! - [`state`]：PipelineState（53 字段）
//! - [`errors`]：PipelineError / StageTimeoutError / StageExecutionError
//! - [`steps`]：各阶段 Runnable（pre_processing / memory / mood / prompt / generation）
//! - [`react`]：原生 FC 的 ReAct 工具循环共享骨架与对话阶段（执行态/表达态）
//! - [`retrieval_ledger`]：检索来源台账（宿主核验的来源索引 + 表达阶段正文守卫）
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
pub(crate) mod gather;
pub mod inline_tag_scanner;
pub mod injection_guard;
pub mod message_context;
pub mod companion_prompt;
pub mod parsers;
pub mod prompt_modules;
pub mod presentation_grounding;
pub mod react;
pub mod request_intent;
pub mod retrieval_ledger;
pub(crate) mod tool_execution;
pub(crate) mod spoken_reply;
pub mod state;
pub(crate) mod task_continuity;
pub mod steps;
pub mod template_engine;
pub mod topic_injection;
#[cfg(test)]
mod prompt_eval;

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
