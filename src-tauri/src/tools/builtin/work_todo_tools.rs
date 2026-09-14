//! 工作智能体待办工具（独立功能，与陪伴智能体的 todo 系统完全分离）。
//!
//! 工作待办存储在编程会话内（`CodingSession.work_todos`，随 coding_sessions.json
//! 持久化），由 `CodingAgentService::write_work_todos` 整表替换写入。
//!
//! **只有一个工具**，每次提交
//! **完整清单**（整表替换，没有局部更新、没有按下标的单条编辑）。模型每调
//! 一次就得把整个计划重述一遍——这正是清单能持续锚定"当前在做什么、下一步
//! 做什么"、不至于退化成摆设的关键。
//!
//! 每次变更 emit `work_todo:changed`（带 session_id），令前端编程面板实时刷新。

use std::sync::RwLock;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use crate::brain::coding_agent::WorkTodo;
use crate::commands::coding_agent::CODING_AGENT;
use crate::tools::types::{PermissionResult, Tool, ToolResult, ToolUseContext, ValidationResult};

/// 全局 AppHandle（由 lib.rs setup 注入，用于 emit 事件给前端）。
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）。
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write().unwrap() = Some(handle);
}

/// 将最新清单广播给前端编程面板（work_todo:changed，带 session_id 供多会话区分）。
fn emit_work_todo_changed(session_id: &str, items: &[WorkTodo]) {
    if let Some(handle) = APP_HANDLE.read().unwrap().clone() {
        let _ = handle.emit(
            "work_todo:changed",
            json!({ "session_id": session_id, "items": items }),
        );
    }
}

/// 通用：把清单整理成给 LLM 看的一行摘要（含三态计数，让模型看到进度）。
fn format_todo_summary(items: &[WorkTodo]) -> String {
    if items.is_empty() {
        return "工作待办已清空。".to_string();
    }
    let cnt = |s: &str| items.iter().filter(|t| t.status == s).count();
    let mut lines: Vec<String> = Vec::with_capacity(items.len());
    for t in items {
        let (mark, label) = match t.status.as_str() {
            "in_progress" => ("▶", "进行中"),
            "completed" => ("[x]", "已完成"),
            _ => ("[ ]", "未开始"),
        };
        lines.push(format!("{mark} {label} {}", t.content));
    }
    format!(
        "已更新工作待办：未开始 {} / 进行中 {} / 已完成 {}\n{}",
        cnt("pending"),
        cnt("in_progress"),
        cnt("completed"),
        lines.join("\n")
    )
}

fn emit_and_summary(session_id: &str, items: &[WorkTodo]) -> ToolResult {
    emit_work_todo_changed(session_id, items);
    ToolResult::standard_success(&format_todo_summary(items), None)
}

/// 工作智能体写入工作待办清单（整表替换）。
pub struct WorkTodoWriteTool;

impl WorkTodoWriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkTodoWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 英文工具描述：全部约定内嵌于此，模型每次看到工具定义就被提醒一遍。
const DESC_EN: &str = "Record and update the structured task list for the current coding task. \
Send the ENTIRE list every call — it REPLACES the previous list (there are no partial updates, \
no per-item edits). Use it to plan multi-step work and drive execution: add one todo per \
concrete step before you start. Keep AT MOST ONE todo `in_progress` at a time; while work \
remains, exactly one active step should be `in_progress`. Mark a todo `completed` the moment \
it is done (do not batch completions), and allow no `in_progress` item only once all work is \
complete. Skip the list for trivial single-step tasks. Statuses: `pending` (not started), \
`in_progress` (being worked on now), `completed` (finished). The current list is injected into \
every turn as your execution plan — rewrite the whole list whenever the plan changes (add, \
split, merge or drop steps) instead of working around a stale one.";

const DESC_ZH: &str = "记录并更新当前编码任务的工作清单。每次调用必须提交**完整清单**——整表替换，\
没有局部更新、没有单条编辑。用它规划多步工作并驱动执行：动手前为每个具体步骤建一条待办。同一时间\
至多一项 `in_progress`；还有工作没做完时，应保持恰好一项处于进行中。完成一步立刻标 `completed`，\
不要攒着批量标；只有全部工作结束才允许没有 `in_progress` 项。简单单步任务不要建清单。状态：\
`pending`（未开始）、`in_progress`（正在做）、`completed`（已完成）。清单会注入每一轮上下文成为\
你的执行计划；计划有变（新增/拆分/合并/放弃步骤）就重写整张表，不要将就旧清单。";

const DESC_JA: &str = "現在のコーディングタスクの作業リストを記録・更新します。呼び出すたびに\
**完全なリスト**を送信してください（全体置換で、部分更新や単項目の編集はありません）。複数ステップの\
作業を計画し実行を駆動するために使います：着手前に具体的なステップごとに1件のTODOを作成してください。\
同時に `in_progress` にできるのは最大1件で、作業が残っている間はちょうど1件を `in_progress` にして\
ください。1ステップ終わったらすぐ `completed` にし、まとめて更新しないでください。作業がすべて\
終わったときのみ `in_progress` を0件にできます。単一ステップの簡単な作業ではリストを作らないで\
ください。状態：`pending`（未着手）、`in_progress`（作業中）、`completed`（完了）。リストは\
毎ターンのコンテキストに実行計画として注入されます。計画が変わったら（追加・分割・統合・中止）\
古いリストに合わせず、全体を書き直してください。";

#[async_trait]
impl Tool for WorkTodoWriteTool {
    fn name(&self) -> &str {
        "work_todo_write"
    }

    fn description(&self) -> &str {
        DESC_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_ZH,
            "ja" => DESC_JA,
            _ => DESC_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "更新工作待办\n改一下任务清单\n记下要做的事",
            "en" => "update the work todo list\nrevise the task list\nnote what needs doing",
            "ja" => "作業ToDoを更新\nタスクリストを修正\nやることを記録",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema_for("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        Self::schema_for(lang)
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("todos").and_then(|v| v.as_array()) {
            Some(_) => ValidationResult::success(Some(input.clone())),
            None => ValidationResult::failure(
                "todos 是必填项，且必须是数组（整表替换：提交完整清单，不存在局部修改）",
                2,
            ),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let session_id = &context.session_id;
        let Some(raw) = args.get("todos").and_then(|v| v.as_array()) else {
            return ToolResult::standard_error(
                "缺少 todos 参数：本工具是整表替换，需要提交完整清单",
                Some("WorkTodoError"),
                None,
            );
        };
        let mut todos: Vec<WorkTodo> = Vec::with_capacity(raw.len());
        for item in raw {
            let Some(content) = item.get("content").and_then(|v| v.as_str()) else {
                return ToolResult::standard_error(
                    "每条待办都需要 content 字段（字符串，一个具体步骤）",
                    Some("WorkTodoError"),
                    None,
                );
            };
            let Some(status) = item.get("status").and_then(|v| v.as_str()) else {
                return ToolResult::standard_error(
                    "每条待办都需要 status 字段（pending / in_progress / completed）",
                    Some("WorkTodoError"),
                    None,
                );
            };
            todos.push(WorkTodo {
                content: content.to_string(),
                status: status.to_string(),
            });
        }
        // 内容约束（trim / 非空 / 去重 / 单 active）由服务层统一裁决，
        // 拒绝而非静默降级——让模型看到自己究竟写错了什么。
        match CODING_AGENT.write_work_todos(session_id, todos) {
            Ok(items) => emit_and_summary(session_id, &items),
            Err(e) => ToolResult::standard_error(&e, Some("WorkTodoError"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> crate::tools::types::ToolCategory {
        crate::tools::types::ToolCategory::System
    }

    /// 权限风险等级：写入本地文件 / 持久化数据
    fn risk(&self) -> crate::tools::types::ToolRiskTier {
        crate::tools::types::ToolRiskTier::FsWrite
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work todo write plan checklist step in_progress completed"
    }
}

impl WorkTodoWriteTool {
    /// 参数 schema：`additionalProperties: false` 让扩展字段在 schema 边界直接
    /// 失败，而不是被静默摊平后写进会话状态。
    fn schema_for(lang: &str) -> Value {
        let (list_desc, content_desc, status_desc) = match lang {
            "zh" => (
                "完整清单，替换掉此前的整张表。",
                "待办内容：一个具体步骤，一句祈使短句。",
                "pending（未开始）| in_progress（正在做）| completed（已完成）",
            ),
            "ja" => (
                "完全なリスト。以前のリスト全体を置き換えます。",
                "TODOの内容：具体的な1ステップを、短い命令文で。",
                "pending（未着手）| in_progress（作業中）| completed（完了）",
            ),
            _ => (
                "The COMPLETE task list, replacing any previous list.",
                "What the task is — one concrete step, a short imperative line.",
                "pending (not started) | in_progress (being worked on now) | completed (done)",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": list_desc,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": content_desc
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": status_desc
                            }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }
}
