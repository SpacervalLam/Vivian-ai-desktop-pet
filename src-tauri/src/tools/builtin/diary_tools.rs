//! 日记工具 - 智能体自主写日记

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::character_registry;
use crate::diary::intelligent_generator;
use crate::tools::types::{PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult};

/// write_diary 工具 - 触发智能日记生成
///
/// 调用 `generate_intelligent_diary` 基于当日对话历史与情绪状态生成日记。
/// 仅在满足前置条件时对 LLM 可见（由 PromptBuildingStep 动态控制）：
/// - 当天尚未生成过日记
/// - 最近 24h 交互轮次 ≥ min_interaction_threshold
/// - 触发分数 ≥ 30
pub struct WriteDiaryTool;

impl WriteDiaryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriteDiaryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WriteDiaryTool {
    fn name(&self) -> &str {
        "write_diary"
    }

    fn description(&self) -> &str {
        "Write today's diary based on the day's conversations and emotional state. \
         Call this when you feel the day has had enough interactions worth recording, \
         or when the day is winding down and you want to reflect. \
         The diary is generated automatically from your memories — no content parameter needed. \
         IMPORTANT: A successful write_diary means the diary task is COMPLETE. \
         Do NOT call any further tools after writing the diary."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "根据当天的对话和情绪状态写今天的日记。当你觉得这一天有足够值得记录的互动，\
            或者一天即将结束、你想反思时调用此工具。日记会基于你的记忆自动生成——无需 content 参数。\
            重要：write_diary 成功即表示日记任务完成。写完日记后不要再调用任何其他工具。",
            "ja" => "その日の会話と感情状態に基づいて今日の日記を書く。その日に記録する価値のあるやり取りが十分あったと感じた時、\
            または一日が終わりに近づいて振り返りたい時に呼び出す。日記は記憶から自動的に生成される——content パラメータは不要。\
            重要：write_diary の成功は日記タスクの完了を意味する。日記を書いた後は他のツールを呼び出さないこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        // 无参数，无需翻译
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _context: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, context: &ToolUseContext) -> ToolResult {
        let char_id = if context.char_id.is_empty() {
            return ToolResult::standard_error(
                "char_id 为空，无法路由到角色",
                None,
                None,
            );
        } else {
            &context.char_id
        };

        let brain = match character_registry::get_brain(char_id) {
            Some(b) => b,
            None => {
                return ToolResult::standard_error(
                    "Brain 未初始化（角色未注册）",
                    None,
                    None,
                );
            }
        };

        // 前置条件校验（与 should_trigger 一致，防止 LLM 在不满足条件时调用）
        match crate::diary::should_trigger(&brain).await {
            (false, reason) => {
                return ToolResult::standard_error(
                    &format!("当前不满足写日记条件: {}", reason),
                    None,
                    None,
                );
            }
            (true, _) => {}
        }

        match intelligent_generator::generate_intelligent_diary(&brain, "tool").await {
            Ok(entry) => {
                let preview: String = entry.content.chars().take(80).collect();
                let data = json!({
                    "id": entry.id,
                    "date": entry.date,
                    "word_count": entry.word_count,
                    "mood_tag": entry.mood_tag,
                    "preview": preview,
                });
                ToolResult::standard_success(
                    &format!("日记已生成：{}（{}字）", entry.date, entry.word_count),
                    Some(data),
                )
            }
            Err(e) => ToolResult::standard_error(
                &format!("日记生成失败: {}", e),
                None,
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 写完日记即任务完成，终止 Agent 循环
    fn signals_goal_completion(&self) -> bool {
        true
    }
}
