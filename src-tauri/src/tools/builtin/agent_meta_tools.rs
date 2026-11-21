//! 工作智能体的目标与交付声明工具。
//!
//! 两个工具补的是「会话状态」这一环——现有工具都能改文件、能跑命令、能问用户，
//! 但没有一个能让模型**声明**「这轮到底要达成什么终态」和「哪些文件算交付」。
//!
//! - `goal`：一句话的目标，跨轮持久，是压缩摘要与最终交付的锚点。
//!   与 `work_todo_write` 分工：待办是步骤，目标是终态；待办可以整表重写，目标不应随步骤漂移。
//! - `present`：把已存在的结果登记为交付物。区别在于**文件真的在**——待办说"要做"，
//!   present 说"已经在"。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::coding_agent::CODING_AGENT;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

// ============================================================================
// GoalTool
// ============================================================================

/// 会话目标的读取与设置。
pub struct GoalTool;

impl GoalTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GoalTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GoalTool {
    fn name(&self) -> &str {
        "goal"
    }

    fn description(&self) -> &str {
        "Read or set this session's one-sentence goal (the end state), persisted across turns.\n\
         Use action=get to re-anchor yourself when a long task drifts, action=set when the user states \
         or changes the end state, action=clear when it is finished or replaced.\n\
         This is NOT the todo list: todos are steps that get rewritten constantly, the goal is the \
         stable target they serve."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取或设置本会话的**一句话目标**（终态），跨轮持久。\n\
                     长任务跑偏时用 action=get 重新对齐；用户说了或改了终态用 action=set；\
                     完成或被取代用 action=clear。\n\
                     它**不是待办清单**：待办是随时会重写的步骤，目标是这些步骤服务的稳定靶子。",
            "ja" => "このセッションの**一文の目標**（最終状態）を読み書きする。ターンを跨いで保持される。\n\
                     長期タスクで方向がぶれたら action=get で再確認、ユーザーが最終状態を述べた／変えたら action=set、\
                     完了または置き換わったら action=clear。\n\
                     **todo リストではない**：todo は書き換えられる手順、目標はその手順が仕える安定した的。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我们到底要做什么\n目标是什么\n改一下目标",
            "en" => "what are we trying to achieve\nwhat's the goal\nupdate the goal",
            "ja" => "最終的に何を作るのか\n目標は\n目標を変えて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get", "set", "clear"],
                    "description": "get 读取当前目标；set 写入（需 text）；clear 清除"
                },
                "text": {
                    "type": "string",
                    "description": "action=set 时的目标，一句话讲清终态（上限 400 字）"
                }
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
        if !matches!(action, "get" | "set" | "clear") {
            return ValidationResult::failure("action 必须是 get / set / clear", 2);
        }
        if action == "set" {
            let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                return ValidationResult::failure("action=set 时 text 不能为空", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("get").trim();
        match action {
            "get" => match CODING_AGENT.work_goal(&ctx.session_id) {
                Some(goal) => ToolResult::standard_success(
                    &format!("当前目标：{goal}"),
                    Some(json!({"goal": goal})),
                ),
                None => ToolResult::standard_success(
                    "本会话还没有设定目标。如果用户已经讲清终态，用 action=set 记下来，后续压缩与交付都以它为锚。",
                    Some(json!({"goal": null})),
                ),
            },
            "set" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                match CODING_AGENT.set_work_goal(&ctx.session_id, Some(text)) {
                    Ok(Some(goal)) => ToolResult::standard_success(
                        &format!("已设定目标：{goal}"),
                        Some(json!({"goal": goal})),
                    ),
                    Ok(None) => ToolResult::standard_success(
                        "目标为空，已清除。",
                        Some(json!({"goal": null})),
                    ),
                    Err(e) => ToolResult::standard_error(&e, Some("InvalidInput"), None),
                }
            }
            _ => match CODING_AGENT.set_work_goal(&ctx.session_id, None) {
                Ok(_) => ToolResult::standard_success(
                    "已清除本会话目标。",
                    Some(json!({"goal": null})),
                ),
                Err(e) => ToolResult::standard_error(&e, Some("InvalidInput"), None),
            },
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "goal objective target end state 目标 终态 目的"
    }
}

// ============================================================================
// PresentTool
// ============================================================================

/// 交付声明：把已存在的文件登记为本次任务的交付物。
pub struct PresentTool;

impl PresentTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PresentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PresentTool {
    fn name(&self) -> &str {
        "present"
    }

    fn description(&self) -> &str {
        "Declare the files this task delivers (must already exist on disk). Use it once the real output \
         is written, so the user gets a concrete artifact list instead of digging through the transcript.\n\
         Only existing paths are accepted — this records what is there, it does not create anything."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "声明本次任务的交付物（文件必须已经真实存在于磁盘上）。真正写完产物后调用一次，\
                     让用户拿到具体的产物清单，而不是在对话里翻。\n\
                     只接受已存在的路径——这个工具是登记「已经在的东西」，不负责创建。",
            "ja" => "このタスクの成果物を宣言する（ファイルは実際に存在している必要がある）。\
                     成果物を書き終えたら一度呼び、ユーザーが会話を遡らずに成果物一覧を得られるようにする。\n\
                     存在するパスのみ受け付ける——既にあるものを記録するだけで、作成はしない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "做完了给我看看\n产物在哪\n交付什么了",
            "en" => "show me what you produced\nwhere are the files\nwhat did you deliver",
            "ja" => "できたものを見せて\n成果物はどこ\n何を納品した",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "交付文件的路径（绝对路径，或相对工作目录的路径）"
                },
                "note": {
                    "type": "string",
                    "description": "可选：一句话说明这批产物是什么（会随登记一起展示）"
                }
            },
            "required": ["paths"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let n = input
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if n == 0 {
            return ValidationResult::failure("paths 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let raw: Vec<String> = args
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        // 相对路径按工作目录解析：模型经常写相对路径，这里补齐后再判存在性。
        let resolved: Vec<String> = raw
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                if path.is_absolute() || ctx.working_directory.trim().is_empty() {
                    p.clone()
                } else {
                    std::path::Path::new(&ctx.working_directory)
                        .join(p)
                        .to_string_lossy()
                        .to_string()
                }
            })
            .collect();

        let (ok, missing) = CODING_AGENT.present_deliverables(&ctx.session_id, &resolved);
        let note = args.get("note").and_then(|v| v.as_str()).unwrap_or("");

        if ok.is_empty() {
            return ToolResult::standard_error(
                &format!(
                    "没有登记任何交付物：这些路径都不存在或无法读取 → {}。\
                     先确认文件真的写出来了（list_dir / glob），再调用一次。",
                    missing.join("、")
                ),
                Some("InvalidInput"),
                None,
            );
        }

        let mut text = format!("已登记 {} 个交付物", ok.len());
        if !note.trim().is_empty() {
            text.push_str(&format!("：{note}"));
        }
        text.push('\n');
        for p in &ok {
            text.push_str(&format!("- {p}\n"));
        }
        if !missing.is_empty() {
            text.push_str(&format!(
                "\n未登记（不存在）：{}\n",
                missing.join("、")
            ));
        }
        ToolResult::standard_success(
            &text,
            Some(json!({"deliverables": ok, "missing": missing, "note": note})),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "present deliverable artifact output files 交付 产物 成果 展示"
    }
}
