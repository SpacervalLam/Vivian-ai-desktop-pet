//! 自主唤醒工具 - schedule_wakeup
//!
//! 让 LLM 给自己安排"稍后再来"的日程：如"我 20 分钟后再来看看你"
//! "明早等用户起床我去打招呼"。到点后由主动对话 tick 消费，
//! 以主动消息形式兑现承诺。与 schedule_reminder（帮用户设提醒）互补。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::proactive::wakeup;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 唤醒时间下限/上限（秒）
const MIN_IN_SECONDS: f64 = 60.0;
const MAX_IN_SECONDS: f64 = 48.0 * 3600.0;

pub struct ScheduleWakeupTool;

impl ScheduleWakeupTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleWakeupTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ScheduleWakeupTool {
    fn name(&self) -> &str {
        "schedule_wakeup"
    }

    fn description(&self) -> &str {
        "Schedule a future moment for yourself to come back and speak to the user proactively. \
         Use this when you naturally make a promise like \"I'll check back in 20 minutes\", \
         \"I'll greet the user tomorrow morning\", or \"let me think about it and tell you later\". \
         When the moment arrives, the system will prompt you to keep the promise. \
         Do NOT use this for user-facing reminders (use schedule_reminder instead) — \
         this is for your own schedule, not the user's."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "给自己安排一个稍后回来的时刻。当你对用户做出自然承诺时使用，如\
            \"我20分钟后再来看看你\"、\"明早等你起床我来打招呼\"、\"我想想，晚点告诉你\"。\
            到点后系统会提示你兑现承诺。给用户设提醒请用 schedule_reminder（那是用户的日程），\
            这个工具是你自己的日程。",
            "ja" => "自分自身の「また後で戻る」予定を登録する。ユーザーに「20分後にまた見に来るよ」\
            「明日の朝起きたら声をかけるね」「考えてから後で伝えるね」など自然な約束をした時に使う。\
            時刻が来たらシステムが約束を果たすよう促す。ユーザーへのリマインド設定には \
            schedule_reminder を使う（これは自分の予定であってユーザーのものではない）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "in_seconds": {
                    "type": "number",
                    "description": "How many seconds from now until you come back (60 ~ 172800)",
                    "minimum": MIN_IN_SECONDS,
                    "maximum": MAX_IN_SECONDS
                },
                "purpose": {
                    "type": "string",
                    "description": "What you plan to do or say when you come back, e.g. \"check if the download finished and tell the user\""
                }
            },
            "required": ["in_seconds", "purpose"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "in_seconds": {
                        "type": "number",
                        "description": "多少秒后回来（60 ~ 172800）",
                        "minimum": MIN_IN_SECONDS,
                        "maximum": MAX_IN_SECONDS
                    },
                    "purpose": {
                        "type": "string",
                        "description": "回来时打算做什么/说什么，如\"看看下载完了没，告诉用户\""
                    }
                },
                "required": ["in_seconds", "purpose"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "in_seconds": {
                        "type": "number",
                        "description": "何秒後に戻るか（60 ~ 172800）",
                        "minimum": MIN_IN_SECONDS,
                        "maximum": MAX_IN_SECONDS
                    },
                    "purpose": {
                        "type": "string",
                        "description": "戻った時に何をする/何を言うか。例：「ダウンロードが終わったか見てユーザーに伝える」"
                    }
                },
                "required": ["in_seconds", "purpose"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let secs = match input.get("in_seconds").and_then(|v| v.as_f64()) {
            Some(s) if s >= MIN_IN_SECONDS && s <= MAX_IN_SECONDS => s,
            Some(s) => {
                return ValidationResult::failure(
                    format!("in_seconds 需在 {MIN_IN_SECONDS} ~ {} 之间，当前 {s}", MAX_IN_SECONDS as i64),
                    2,
                )
            }
            None => return ValidationResult::failure("in_seconds 是必填项（秒数）", 2),
        };
        let purpose = match input.get("purpose").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => return ValidationResult::failure("purpose 是必填项（回来时打算做什么）", 2),
        };
        ValidationResult::success(Some(json!({ "in_seconds": secs, "purpose": purpose })))
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 自主日程安排，无需用户确认
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let secs = args.get("in_seconds").and_then(|v| v.as_f64()).unwrap_or(MIN_IN_SECONDS);
        let purpose = args
            .get("purpose")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let char_id = context.char_id.clone();
        let scheduler = wakeup::get_scheduler(&char_id);
        match scheduler.schedule(secs, &purpose) {
            Ok(id) => {
                let due_desc = if secs >= 3600.0 {
                    format!("{:.1} 小时后", secs / 3600.0)
                } else {
                    format!("{} 分钟后", (secs / 60.0).round() as i64)
                };
                ToolResult::success(json!({
                    "scheduled": true,
                    "id": id,
                    "due_in": due_desc,
                    "purpose": purpose,
                    "note": "到点后你会被提醒兑现这个约定"
                }))
            }
            Err(e) => ToolResult::error(format!("安排唤醒失败: {e}")),
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn is_read_only(&self) -> bool {
        false
    }
}
