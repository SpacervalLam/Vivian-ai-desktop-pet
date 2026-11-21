//! 陪伴智能体的内部续思考信号。
//!
//! `continue_thinking` 不读取或修改外部状态，也不要求模型暴露完整思维链。
//! 它只让陪伴侧现有 ReAct 循环再运行一轮，使复杂问题可以先检查假设、
//! 验证结论或决定下一步工具，而不是首轮草稿一生成就结束。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ToolSemantics, ValidationResult,
};

pub const CONTINUE_THINKING_TOOL: &str = "continue_thinking";

pub struct ContinueThinkingTool;

impl ContinueThinkingTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ContinueThinkingTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ContinueThinkingTool {
    fn name(&self) -> &str {
        CONTINUE_THINKING_TOOL
    }

    fn description(&self) -> &str {
        "Continue the companion agent's private deliberation for one more round without changing external state. Use before the final answer when a complex request still needs assumption checking, verification, decomposition, or a decision about which real tool to call. Pass only a short high-level focus, never hidden chain-of-thought. Do not use for greetings, casual chat, or when the answer is already clear."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "让陪伴智能体在不改变外部状态的情况下继续推演一轮。复杂请求仍需检查假设、核验结论、拆解步骤或决定下一项真实工具时，在最终回答前使用。只传简短的高层关注点，不要写出完整思维链。寒暄、闲聊或答案已明确时不要使用。",
            "ja" => "外部状態を変更せず、コンパニオンエージェントの検討をもう1ラウンド続けます。複雑な依頼で、前提確認・結論検証・分解・次に使う実ツールの判断が必要な場合に、最終回答の前に使用します。完全な思考過程ではなく短い高レベルの確認点だけを渡してください。挨拶、雑談、答えが明確な場合は使用しません。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "stage": {
                    "type": "string",
                    "enum": ["analyze", "verify", "plan", "reflect"],
                    "description": "The kind of deliberation needed next"
                },
                "focus": {
                    "type": "string",
                    "description": "One short high-level checkpoint for the next round; do not include chain-of-thought",
                    "maxLength": 160
                }
            },
            "required": ["stage", "focus"],
            "additionalProperties": false
        })
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let valid_stage = matches!(
            input.get("stage").and_then(Value::as_str),
            Some("analyze" | "verify" | "plan" | "reflect")
        );
        let focus = input.get("focus").and_then(Value::as_str).unwrap_or("").trim();
        if !valid_stage {
            return ValidationResult::failure(
                "stage 必须是 analyze / verify / plan / reflect",
                2,
            );
        }
        if focus.is_empty() || focus.chars().count() > 160 {
            return ValidationResult::failure("focus 必须为不超过 160 字符的简短关注点", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let stage = args.get("stage").and_then(Value::as_str).unwrap_or("analyze");
        let focus = args.get("focus").and_then(Value::as_str).unwrap_or("").trim();
        ToolResult::standard_success(
            "Internal deliberation checkpoint accepted. Continue with the next reasoning step; use a real tool if evidence or action is needed, otherwise give the final answer.",
            Some(json!({ "continue": true, "stage": stage, "focus": focus })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // 虽然实现上只读，但它不是“向用户转述检索结果”的检索工具。
    // 标为 Action 可避免 ReAct 收尾错误注入资料转述提示。
    fn semantics(&self) -> ToolSemantics {
        ToolSemantics::Action
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "continue deliberate verify reflect think"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validates_bounded_high_level_checkpoint() {
        let tool = ContinueThinkingTool::new();
        let ctx = ToolUseContext::default();
        assert!(tool
            .validate_input(&json!({"stage": "verify", "focus": "检查关键假设"}), &ctx)
            .await
            .result);
        assert!(!tool
            .validate_input(&json!({"stage": "unknown", "focus": "x"}), &ctx)
            .await
            .result);
        assert!(!tool
            .validate_input(&json!({"stage": "analyze", "focus": ""}), &ctx)
            .await
            .result);
    }

    #[tokio::test]
    async fn is_side_effect_free_and_does_not_complete_goal() {
        let tool = ContinueThinkingTool::new();
        let result = tool
            .call(
                json!({"stage": "reflect", "focus": "比较两种解释"}),
                &ToolUseContext::default(),
            )
            .await;
        assert!(result.success);
        assert!(!result.goal_completed);
        assert_eq!(result.data.unwrap()["data"]["continue"], true);
    }
}
