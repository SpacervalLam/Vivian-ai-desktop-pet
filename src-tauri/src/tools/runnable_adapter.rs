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
use crate::tools::types::{PermissionBehavior, Tool, ToolUseContext};

/// Tool → Runnable 适配器
pub struct ToolRunnableAdapter {
    tool: Arc<dyn Tool>,
    /// 默认上下文（当 input 未携带 `_context` 时使用）
    default_context: ToolUseContext,
    /// 是否跳过权限检查（pipeline 已在外层处理时设为 true）
    skip_permissions: bool,
}

impl ToolRunnableAdapter {
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self {
            tool,
            default_context: ToolUseContext::default(),
            skip_permissions: false,
        }
    }

    pub fn with_context(mut self, ctx: ToolUseContext) -> Self {
        self.default_context = ctx;
        self
    }

    pub fn with_skip_permissions(mut self, skip: bool) -> Self {
        self.skip_permissions = skip;
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
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let (args, ctx) = self.split_input(&input);

        // 1. 输入验证
        let validation = self.tool.validate_input(&args, &ctx).await;
        if !validation.result {
            return Err(VivianError::Tool(format!(
                "输入验证失败: {} (code={})",
                validation.message, validation.error_code
            )));
        }
        let final_args = validation.data.unwrap_or(args);

        // 2. 权限检查（可跳过）
        if !self.skip_permissions {
            let perm = self.tool.check_permissions(&final_args, &ctx).await;
            match perm.behavior {
                PermissionBehavior::Deny => {
                    return Err(VivianError::Permission(perm.message));
                }
                PermissionBehavior::Ask => {
                    return Err(VivianError::Permission(format!(
                        "工具 {} 需要用户确认: {}",
                        self.tool.name(),
                        perm.message
                    )));
                }
                PermissionBehavior::Allow | PermissionBehavior::Passthrough => {}
            }
        }

        // 3. 执行
        let result = self.tool.call(final_args, &ctx).await;
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
    use crate::tools::types::{ToolCategory, ToolResult};
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
        async fn call(
            &self,
            args: Value,
            _context: &ToolUseContext,
        ) -> ToolResult {
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
