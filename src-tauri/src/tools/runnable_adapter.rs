//! Tool → Runnable 适配器
//!
//! 让 `Arc<dyn Tool>` 可以作为 `Box<dyn Runnable>` 接入 pipeline。
//!
//! 输入约定：`input: Value` 即工具参数（args）；若需覆盖默认 `ToolUseContext`，
//! 在 input 中以 `_context` 字段传入。
//!
//! 输出约定：
//! - `ToolResult.success=true` → `Ok(data.unwrap_or(Value::Null))`
//! - `ToolResult.success=false` → `Err(VivianError::Tool(error))`

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::tools::types::{Tool, ToolUseContext};
use crate::tools::ToolSystem;

/// Tool → Runnable 适配器
pub struct ToolRunnableAdapter {
    tool: Arc<dyn Tool>,
    /// 默认上下文（当 input 未携带 `_context` 时使用）
    default_context: ToolUseContext,
    tool_system: Arc<ToolSystem>,
}

impl ToolRunnableAdapter {
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        let tool_system = Arc::new(ToolSystem::new());
        tool_system.register_tool(tool.clone());
        Self {
            tool,
            default_context: ToolUseContext::default(),
            tool_system,
        }
    }

    pub fn with_context(mut self, ctx: ToolUseContext) -> Self {
        self.default_context = ctx;
        self
    }

    /// Reuse the application's live registry, permissions, policies and telemetry.
    /// The tool must already be registered in this system.
    pub fn with_tool_system(mut self, system: Arc<ToolSystem>) -> Self {
        self.tool_system = system;
        self
    }

    /// Compatibility setter: unified execution always applies security checks.
    pub fn with_skip_permissions(self, _skip: bool) -> Self {
        self
    }

    /// 从 input Value 中拆出 args 和 context
    fn split_input(&self, input: &Value) -> (Value, ToolUseContext) {
        if let Some(obj) = input.as_object() {
            if let Some(ctx_val) = obj.get("_context") {
                if let Ok(ctx) = serde_json::from_value::<ToolUseContext>(ctx_val.clone()) {
                    let mut args = obj.clone();
                    args.remove("_context");
                    return (Value::Object(args), ctx);
                }
            }
        }
        (input.clone(), self.default_context.clone())
    }
}

#[async_trait]
impl Runnable for ToolRunnableAdapter {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let (args, ctx) = self.split_input(&input);

        let result =
            crate::tools::execute_tool_use(self.tool.name(), args, &self.tool_system, &ctx, None)
                .await;
        if result.success {
            Ok(result.data.unwrap_or(Value::Null))
        } else {
            let err_msg = result
                .error
                .unwrap_or_else(|| format!("工具 {} 执行失败", self.tool.name()));
            Err(VivianError::Tool(err_msg))
        }
    }
}

/// 为 `Arc<dyn Tool>` 提供 `into_runnable()` 扩展方法
pub trait ToolRunnableExt {
    fn into_runnable(self) -> ToolRunnableAdapter;
}

impl ToolRunnableExt for Arc<dyn Tool> {
    fn into_runnable(self) -> ToolRunnableAdapter {
        ToolRunnableAdapter::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::types::{PermissionBehavior, ToolCategory, ToolResult};
    use async_trait::async_trait;
    use serde_json::json;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "回显输入"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn validate_input(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> crate::tools::types::ValidationResult {
            crate::tools::types::ValidationResult::success(None)
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> crate::tools::types::PermissionResult {
            crate::tools::types::PermissionResult {
                behavior: PermissionBehavior::Allow,
                message: String::new(),
                updated_input: None,
            }
        }
        async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
            ToolResult::success(args)
        }
        fn is_read_only(&self) -> bool {
            true
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }
    }

    #[tokio::test]
    async fn test_tool_runnable_adapter_basic() {
        let tool: Arc<dyn Tool> = Arc::new(EchoTool);
        let runnable = tool.into_runnable();
        let input = json!({"text": "hello"});
        let result = runnable.ainvoke(input, None).await.unwrap();
        assert_eq!(result["text"], "hello");
    }

    #[tokio::test]
    async fn test_tool_runnable_adapter_context_extraction() {
        let tool: Arc<dyn Tool> = Arc::new(EchoTool);
        let runnable = ToolRunnableAdapter::new(tool);
        let input = json!({
            "text": "hi",
            "_context": {
                "session_id": "s1",
                "user_id": "u1",
                "working_directory": "/tmp",
                "timestamp": "2026-01-01T00:00:00Z"
            }
        });
        let result = runnable.ainvoke(input, None).await.unwrap();
        assert_eq!(result["text"], "hi");
    }
}
