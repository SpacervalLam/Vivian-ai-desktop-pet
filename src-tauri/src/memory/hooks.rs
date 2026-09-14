//! Open Hooks 闭环判定器
//!
//! 记忆条目携带"未闭环钩子"（承诺/约定/待跟进事项），
//! 后续对话中由 HookJudge 异步判定是否闭环。
//!
//! 闭环后：
//! - 在记忆的 open_hooks 中标记 closed_at + closed_by
//! - 闭环的记忆不再获得检索 boost（阶段 2.1 实现）
//!
//! 设计原则：
//! - 读路径零 LLM：闭环判定只在写路径（对话后处理）进行
//! - 失败兜底：LLM 不可用时跳过判定，不阻塞主流程
//! - 节流：每轮对话最多判定一次，且只检查未闭环的 hooks

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{VivianError, VivianResult};
use crate::memory::manager::MemoryManager;
use crate::memory::types::{MemoryItem, OpenHook};
use crate::types::response::ChatMessage;

/// HookJudge 使用的 LLM 客户端抽象
#[async_trait]
pub trait HookJudgeLlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 ModelRouter 实现
#[async_trait]
impl HookJudgeLlmClient for crate::providers::ModelRouter {
    async fn complete(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![ChatMessage::user(prompt.to_string())];
        let schema = {
            let root = schemars::schema_for!(ClosureVerdict);
            serde_json::to_value(&root.schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
        };
        self.generate(crate::providers::base::LLMRequest::new("memory", messages).with_json_schema(schema))
            .await
    }
}

/// LLM 闭环判定结果
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ClosureVerdict {
    /// 是否闭环
    closed: bool,
    /// 闭环理由（可选，仅用于调试日志）
    #[serde(default)]
    reason: Option<String>,
}

/// Open Hooks 闭环判定器
pub struct HookJudge {
    llm: Option<Arc<dyn HookJudgeLlmClient>>,
}

impl HookJudge {
    pub fn new(llm: Option<Arc<dyn HookJudgeLlmClient>>) -> Self {
        Self { llm }
    }

    /// 从 ModelRouter 构造
    pub fn from_router(router: Arc<crate::providers::ModelRouter>) -> Self {
        let llm: Arc<dyn HookJudgeLlmClient> = router;
        Self::new(Some(llm))
    }

    /// 兜底构造：无 LLM 时只读不判定
    pub fn fallback() -> Self {
        Self::new(None)
    }

    /// 判定并闭环：检查所有未闭环 hooks，用 LLM 判断当前对话是否满足闭环条件
    ///
    /// 返回本轮闭环的 hook 数量。LLM 不可用时返回 0。
    pub async fn judge_and_close(
        &self,
        memory: &MemoryManager,
        recent_dialog: &str,
    ) -> VivianResult<usize> {
        let llm = match &self.llm {
            Some(l) => l,
            None => return Ok(0),
        };

        if recent_dialog.trim().is_empty() {
            return Ok(0);
        }

        // 读取所有含未闭环 hooks 的记忆
        let candidates = memory.get_memories_with_open_hooks();
        if candidates.is_empty() {
            return Ok(0);
        }

        let mut total_closed = 0usize;
        for item in candidates {
            let closed = self.judge_item(&item, recent_dialog, llm, memory).await?;
            total_closed += closed;
        }

        if total_closed > 0 {
            tracing::info!(
                "[HookJudge] 本轮闭环 {} 个 open_hooks",
                total_closed
            );
        }
        Ok(total_closed)
    }

    /// 判定单条记忆的未闭环 hooks
    async fn judge_item(
        &self,
        item: &MemoryItem,
        recent_dialog: &str,
        llm: &Arc<dyn HookJudgeLlmClient>,
        memory: &MemoryManager,
    ) -> VivianResult<usize> {
        let mut hooks = item.open_hooks.clone();
        let mut closed_count = 0usize;

        for hook in hooks.iter_mut() {
            if !hook.is_open() {
                continue;
            }
            match self.judge_single_hook(&item.content, hook, recent_dialog, llm).await {
                Ok(true) => {
                    hook.close(None);
                    closed_count += 1;
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        "[HookJudge] hook 判定失败 (memory={}, hook_type={}): {}",
                        item.id,
                        hook.hook_type,
                        e
                    );
                }
            }
        }

        // 有闭环则更新记忆
        if closed_count > 0 {
            memory.update_open_hooks(&item.id, hooks)?;
        }
        Ok(closed_count)
    }

    /// 用 LLM 判定单个 hook 是否闭环
    async fn judge_single_hook(
        &self,
        memory_content: &str,
        hook: &OpenHook,
        recent_dialog: &str,
        llm: &Arc<dyn HookJudgeLlmClient>,
    ) -> VivianResult<bool> {
        let prompt = build_judge_prompt(memory_content, &hook.hook_type, &hook.condition, recent_dialog);
        let resp = llm.complete(&prompt).await?;
        let cleaned = strip_code_fence(&resp);
        let verdict: ClosureVerdict = serde_json::from_str(cleaned).map_err(|e| {
            VivianError::Other(format!("解析闭环判定响应失败: {e}"))
        })?;
        tracing::debug!(
            closed = verdict.closed,
            reason = ?verdict.reason,
            "[HookJudge] 闭环判定结果"
        );
        Ok(verdict.closed)
    }
}

/// 构造闭环判定 prompt
fn build_judge_prompt(
    memory_content: &str,
    hook_type: &str,
    condition: &str,
    recent_dialog: &str,
) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    match lang_norm {
        "en" => format!(
            "You are a closure judge. Determine whether the recent conversation satisfies the closure condition of an unclosed hook for a memory.\n\n\
            ## Memory Content\n{memory_content}\n\n\
            ## Unclosed Hook\n\
            - Type: {hook_type}\n\
            - Closure condition: {condition}\n\n\
            ## Recent Conversation\n{recent_dialog}\n\n\
            ## Judgment Rules\n\
            - Only judge as closed when the conversation clearly indicates the condition is met (e.g., user says \"I've repaid the loan\" matching \"user mentions having repaid next time\")\n\
            - Vague or indirect mentions do not count as closure\n\
            - If the conversation is unrelated to the hook, judge as not closed\n\n\
            Output JSON only: {{\"closed\":true/false,\"reason\":\"brief explanation\"}}"
        ),
        "ja" => format!(
            "あなたはクロージャ判定器です。最近の会話が特定の記憶の未クローズフックのクロージャ条件を満たしているか判断してください。\n\n\
            ## 記憶内容\n{memory_content}\n\n\
            ## 未クローズフック\n\
            - タイプ：{hook_type}\n\
            - クロージャ条件：{condition}\n\n\
            ## 最近の会話\n{recent_dialog}\n\n\
            ## 判定ルール\n\
            - 会話が条件を満たしていることを明確に示した場合のみクローズと判定（例：ユーザーが「ローンを返済した」と言い、「ユーザーが次回返済したと言及」に該当）\n\
            - 曖昧または間接的な言及はクローズと見なさない\n\
            - 会話がフックと無関係な場合、未クローズと判定\n\n\
            JSONのみ出力：{{\"closed\":true/false,\"reason\":\"簡潔な説明\"}}"
        ),
        _ => format!(
            "你是闭环判定器。判断最近的对话是否满足某条记忆的未闭环钩子的闭环条件。\n\n\
            ## 记忆内容\n{memory_content}\n\n\
            ## 未闭环钩子\n\
            - 类型：{hook_type}\n\
            - 闭环条件：{condition}\n\n\
            ## 最近对话\n{recent_dialog}\n\n\
            ## 判定规则\n\
            - 只有当对话明确表明条件已满足时才判定为闭环（如用户说「我已经还款了」对应「用户下次提到已还款」）\n\
            - 模糊或间接的提及不算闭环\n\
            - 如果对话与钩子无关，判定为未闭环\n\n\
            只输出 JSON：{{\"closed\":true/false,\"reason\":\"简要说明\"}}"
        ),
    }
}

/// 去除 ```json ... ``` 围栏
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim();
    }
    if let Some(rest) = t.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_closure_verdict_closed() {
        let resp = r#"{"closed":true,"reason":"用户提到已还款"}"#;
        let v: ClosureVerdict = serde_json::from_str(resp).unwrap();
        assert!(v.closed);
        assert_eq!(v.reason.as_deref(), Some("用户提到已还款"));
    }

    #[test]
    fn parse_closure_verdict_open() {
        let resp = r#"{"closed":false}"#;
        let v: ClosureVerdict = serde_json::from_str(resp).unwrap();
        assert!(!v.closed);
    }

    #[test]
    fn strip_fence_works() {
        assert_eq!(strip_code_fence("```json\n{\"x\":1}\n```"), r#"{"x":1}"#);
        assert_eq!(strip_code_fence(r#"{"x":1}"#), r#"{"x":1}"#);
    }
}
