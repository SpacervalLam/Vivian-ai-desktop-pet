//! 增强版工具系统 - 桌宠 AI 调用的统一工具基础设施
//!
//! 整合注册表、缓存、沙箱、可观测性等组件，为 AI 模型提供安全的工具调用能力。
//!
//! # 主要组件
//! - [`ToolSystem`]：工具系统入口，管理工具注册与查找
//! - [`Tool`]：工具 trait，所有工具必须实现
//! - [`ToolResult`]：工具执行结果
//! - [`ToolUseContext`]：工具调用上下文
//! - [`PermissionContext`] / [`PermissionMode`] / [`PermissionResult`]：权限相关类型
//! - [`ValidationResult`]：输入验证结果
//! - [`ToolErrorCode`]：结构化错误码
//!
//! # 工具执行管线
//! [`executor::execute_tool_use`] 实现完整管线：
//! 1. 查找工具
//! 2. 沙箱安全检查
//! 3. 输入验证
//! 4. 缓存检查（只读工具）
//! 5. 权限检查
//! 6. 执行（带超时）
//! 7. 缓存写入（只读工具）

pub mod builtin;
pub mod cache;
pub mod chainer;
pub mod confirmation;
pub mod discovery;
pub mod executor;
pub mod mcp;
pub mod observability;
pub mod permission;
pub mod registry;
pub mod runnable_adapter;
pub mod sandbox;
pub mod semantic_filter;
pub mod services;
pub mod tool_call_manager;
pub mod trust;
pub mod types;

// 重新导出 Hook 系统
pub use crate::hooks::{HookDecision, HookEventName, HookRegistry};

// 重新导出核心 API
pub use registry::{normalize_tool_name, ToolSystem};

pub use types::{
    normalize_path, policy_for, AgentAccessLevel, PermissionBehavior, PermissionContext,
    PermissionMode, PermissionResult, Tool, ToolCategory, ToolDefinition, ToolErrorCode,
    ToolErrorInfo, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
    WorkingDirectoryPermission,
};

// 重新导出执行器
pub use executor::{execute_tool_calls, execute_tool_calls_parallel, execute_tool_use, CanUseTool};

// 重新导出工具调用管理器
pub use tool_call_manager::{MultiStepResult, ToolCallManager, ToolCallStatus};

// 重新导出工具语义筛选器
pub use semantic_filter::{should_filter_tools, ToolRecommendation, ToolSemanticFilter};

// 重新导出沙箱
pub use sandbox::{ProtectionMode, SafetyResult, ToolRiskLevel, ToolSafetyProfile, ToolSandbox};

// 重新导出缓存
pub use cache::ToolCache;

// 重新导出可观测性
pub use observability::{ObsRecord, ToolCallRecord, ToolMetrics, ToolObservability};

// 重新导出权限
pub use permission::{check_tool_permission, PermissionContextBuilder};

// 重新导出工具链
pub use chainer::{
    FailurePolicy, IntentRecognizer, IntentType, LoopTerminationReason,
    MultiStepCallRecord, MultiStepExecutor, MultiStepLoopResult, ToolChain, ToolChainer,
};

// 重新导出工具发现
pub use discovery::{collect_discoverable_tools, DiscoverableTool, ToolSearchIndex};

// 重新导出服务层
pub use services::ServiceContext;

// 重新导出内置工具注册
pub use builtin::register_builtin_tools;

// 重新导出 Tool → Runnable 适配器
pub use runnable_adapter::{ToolRunnableAdapter, ToolRunnableExt};

// 重新导出工具确认注册表
pub use confirmation::{
    ConfirmationRequest, ConfirmationResponse, ConfirmationRisk, SharedConfirmationRegistry,
    ToolConfirmationRegistry,
};

// 重新导出 MCP
pub use mcp::{McpManager, McpServerConfig, McpServerStatus, McpTool};
