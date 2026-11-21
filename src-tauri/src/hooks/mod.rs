//! Hook 系统 —— PreToolUse / PostToolUse 可扩展拦截点
//!
//! 允许用户通过配置文件定义外部脚本，在工具执行前后进行拦截或记录。
//! - PreToolUse：可 deny 阻止工具执行
//! - PostToolUse：信息性，无 deny 能力
//!
//! 特性：
//! - fail-open：超时/异常/无效 JSON 默认 allow
//! - 通过 stdin 传递 JSON 事件，stdout 解析决策
//! - 支持全局 + 项目级配置合并

pub mod config;
pub mod event;
pub mod runner;

pub use config::{HookRegistry, HookSpec};
pub use event::{HookDecision, HookEvent, HookEventName};
pub use runner::{dispatch_hooks, run_hook};
