//! 日记工具 - 智能体自主写日记

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::character_registry;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
    ToolRiskTier,
};

/// write_diary 兼容工具名：只提交做梦与日记安排请求。
///
/// 环境智能体决定是否开始；获准后先做梦、随后写日记，工具不直接写入。
/// 仅在满足前置条件时对 LLM 可见（由 PromptBuildingStep 动态控制）：
/// - 当天尚未生成过日记
/// - 当日原始对话 ≥ min_interaction_threshold，且在允许时间窗内
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
        "Request scheduling of the daily dream: organize memories and associate experiences, then write today's diary. \
         Call this when you feel the day has had enough interactions worth recording, \
         or when the day is winding down and you want to reflect. \
         The diary is generated automatically from your memories — no content parameter needed. \
         IMPORTANT: Success means the request was submitted, not that a diary was written. \
         Completion is notified separately. Do not repeat this request or claim the diary is ready."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "请求安排做梦整理、联想记忆，获准后紧接着写今天的日记，无需分别触发。当你觉得这一天有足够值得记录的互动，\
            或者一天即将结束、你想反思时调用此工具。日记会基于你的记忆自动生成——无需 content 参数。\
            重要：成功只代表安排请求已提交，日记尚未生成；完成后另行通知。不要重复提交，也不要宣称日记已经写好。",
            "ja" => "先に夢として記憶を整理・関連付け、その直後に今日の日記を書く。別々に呼ぶ必要はない。その日に記録する価値のあるやり取りが十分あったと感じた時、\
            または一日が終わりに近づいて振り返りたい時に呼び出す。日記は記憶から自動的に生成される——content パラメータは不要。\
            重要：成功は予約リクエストの送信を意味し、日記の完成ではない。完了は別途通知される。再送したり完成したと伝えたりしないこと。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "做个梦整理记忆\n整理联想后写日记\n把我今天的事写进日记\n记一篇日记\n帮我写日记\n今天值得记下来",
            "en" => "write today into my diary\nkeep a diary entry\nnote this day down",
            "ja" => "今日のことを日記に書いて\n日記をつけて\n今日を記録して",
            _ => "",
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

        if !matches!(crate::diary::dream::candidate(&brain, chrono::Local::now()), Ok(Some(_))) {
            return ToolResult::standard_error("素材量或时间窗等程序条件不满足，暂不安排判断", None, None);
        }
        let Some(environment) = crate::world::environment_agent::shared() else {
            return ToolResult::standard_error("日常安排尚未就绪，请稍后重试", None, None);
        };
        environment.request_diary_check(char_id);
        ToolResult::standard_success("已请求安排做梦与日记，尚未生成日记，完成后会另行通知。",
            Some(json!({"requested":true,"completed":false})))
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：写入本地文件 / 持久化数据
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    /// 安排请求提交后结束本轮，避免反复请求；不表示日记已完成。
    fn signals_goal_completion(&self) -> bool {
        true
    }
}
