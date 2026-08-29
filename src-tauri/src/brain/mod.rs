//! Vivian Brain 子系统。
//!
//! 模块清单（按职责分组）：
//! - [`brain`]：Brain 主控（编排各子系统）
//! - [`cognitive_tick`]：统一认知循环（6 阶段流水线，替换双 tick）
//! - [`callbacks`]：回调系统（CallbackManager 流式片段与工具调用收集器）
//! - [`chat_chain`]：BrainChatChain 高层管道封装
//! - [`json_parser`]：流式 JSON 解析器
//! - [`rate_limiter`]：Token bucket 限流器
//! - [`scheduler`]：任务调度器
//! - [`interruption_controller`]：中断控制器
//! - [`command_handler`]：命令处理器 + 解析器
//! - [`smart_app_classifier`]：智能应用分类器
//! - [`augment_reply_service`]：回复增强服务
//! - [`computer_control`]：电脑控制执行引擎（简化实现）
//! - [`control_action_executor`]：桌宠自控动作执行器（chat 产出 Live2D 模型控制指令）

pub mod async_reflection;
pub mod augment_reply_service;
pub mod action_planner;
pub mod brain;
pub mod budget;
pub mod callbacks;
pub mod chat_chain;
pub mod cognitive_tick;
pub mod command_handler;
pub mod computer_control;
pub mod control_action_executor;
pub mod focus_mode;
pub mod interruption_controller;
pub mod jobs;
pub mod json_parser;
pub mod plan_mode;
pub mod rate_limiter;
pub mod scheduler;
pub mod smart_app_classifier;
pub mod subagent_context;
pub mod coding_agent;
pub mod agent_presets;
pub mod task_service;
pub mod tool_leak_filter;
pub mod topic_signal;
pub mod workflow;

// 主要导出（保持向后兼容）
pub use brain::Brain;
pub use callbacks::CallbackManager;
pub use chat_chain::BrainChatChain;
pub use cognitive_tick::{CognitiveTickPhase, CognitiveTickResult, CognitiveTickRunner, PhaseDecision};
pub use focus_mode::{CognitionMode, FocusDecision, FocusState, FocusThresholds};
pub use subagent_context::{SubagentContext, SubagentTask};
pub use task_service::{TaskEvent, TaskEventKind, TaskService, TaskStatus, MAX_TASK_STEPS};
