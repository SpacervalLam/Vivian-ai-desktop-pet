//! observe_user 工具 - LLM 主动记录用户行为习惯观察。
//!
//! 始终暴露给 LLM，用于创建研究课题和记录样本。
//! 后端负责统计聚合和置信度计算，成熟结论注入 prompt 形成用户画像。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::research::ResearchManager;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 按角色 ID 索引的 ResearchManager 注入表。
/// 在 Brain 构造时由 `register_research_manager` 注入，工具调用时按 char_id 取出。
static RESEARCH_MANAGERS: Lazy<RwLock<HashMap<String, Arc<ResearchManager>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 注册某角色的 ResearchManager（Brain 构造时调用）
pub fn register_research_manager(char_id: &str, rm: Arc<ResearchManager>) {
    RESEARCH_MANAGERS.write().insert(char_id.to_string(), rm);
}

/// observe_user 工具 - LLM 主动记录用户行为观察
pub struct ObserveUserTool;

impl ObserveUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ObserveUserTool {
    fn name(&self) -> &str {
        "observe_user"
    }

    fn description(&self) -> &str {
        "Record an observation about the user's behavior or habits. Use this when you notice a recurring pattern worth tracking long-term, such as sleep schedule, meal times, exercise routines, work hours, or any stable behavioral habit. Each call records one data sample; over time the system aggregates samples into confirmed habits."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "记录对用户行为或习惯的观察。当你注意到值得长期跟踪的重复模式时使用此工具，\
            例如睡眠时间、用餐时间、运动习惯、工作时段，或任何稳定的行为习惯。\
            每次调用记录一个数据样本；系统会随时间累积样本并聚合为已确认的习惯。",
            "ja" => "ユーザーの行動や習慣に関する観察を記録する。睡眠時間、食事時間、運動ルーティン、労働時間など、\
            長期的に追跡する価値のある繰り返しパターンに気付いた時に使用。\
            各呼び出しで1つのデータサンプルを記録し、システムは時間とともにサンプルを集約して確認済みの習慣にする。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "A stable identifier for what is being observed, e.g. 'sleep_schedule', 'dinner_time', 'exercise_routine'. Use snake_case. Reuse the same target across multiple observations to build up samples."
                },
                "observation": {
                    "type": "string",
                    "description": "Natural language description of what was observed, e.g. 'User mentioned going to bed' or 'User started cooking dinner'."
                },
                "data": {
                    "type": "object",
                    "description": "Optional structured data. For time-of-day habits include {\"time\": \"HH:MM\"}. For duration habits include {\"duration_min\": number}. Can include both."
                }
            },
            "required": ["target", "observation"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "正在观察的对象的稳定标识符，例如 'sleep_schedule'、'dinner_time'、'exercise_routine'。使用 snake_case。在多次观察中复用同一 target 以累积样本。"
                    },
                    "observation": {
                        "type": "string",
                        "description": "观察内容的自然语言描述，例如「用户提到要去睡觉了」或「用户开始做晚饭」。"
                    },
                    "data": {
                        "type": "object",
                        "description": "可选结构化数据。时间相关习惯请包含 {\"time\": \"HH:MM\"}；时长相关习惯请包含 {\"duration_min\": number}。可同时包含两者。"
                    }
                },
                "required": ["target", "observation"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "観察対象の安定した識別子、例: 'sleep_schedule'、'dinner_time'、'exercise_routine'。snake_case を使用。複数の観察で同じ target を再利用してサンプルを蓄積。"
                    },
                    "observation": {
                        "type": "string",
                        "description": "観察内容の自然言語の説明、例: 「ユーザーが寝ると言った」や「ユーザーが夕食の準備を始めた」。"
                    },
                    "data": {
                        "type": "object",
                        "description": "任意の構造化データ。時刻関連の習慣は {\"time\": \"HH:MM\"} を含める。時間長関連の習慣は {\"duration_min\": number} を含める。両方を含めることも可能。"
                    }
                },
                "required": ["target", "observation"]
            }),
            _ => self.parameters_schema(),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "observe user habit behavior profile research pattern schedule routine"
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let target = input.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let observation = input.get("observation").and_then(|v| v.as_str()).unwrap_or("");

        if target.trim().is_empty() {
            return ValidationResult::failure("target is required and cannot be empty", 400);
        }
        if observation.trim().is_empty() {
            return ValidationResult::failure("observation is required and cannot be empty", 400);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let observation = args.get("observation").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let data = args.get("data").cloned().unwrap_or(Value::Null);

        // 按 char_id 查找对应的 ResearchManager
        let manager = {
            let managers = RESEARCH_MANAGERS.read();
            managers.get(&context.char_id).cloned()
        };

        let manager = match manager {
            Some(m) => m,
            None => {
                return ToolResult::error(format!(
                    "No research manager registered for character '{}'",
                    context.char_id
                ));
            }
        };

        // source_text 取自 ToolUseContext 的 user_message（Phase 3 添加）
        let source_text = context.user_message.clone().unwrap_or_default();

        let outcome = manager.record_observation(&target, &observation, data, &source_text);

        let response = json!({
            "status": "recorded",
            "target": target,
            "created": outcome.created,
            "sample_count": outcome.sample_count,
            "task_status": outcome.status,
            "just_concluded": outcome.just_concluded,
            "confidence": outcome.confidence,
            "summary": outcome.summary,
        });

        ToolResult::success(response)
    }
}
