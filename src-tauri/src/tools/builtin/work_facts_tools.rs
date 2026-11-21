//! 跨会话工作事实工具。
//!
//! 一条事实 = 一句话 + 来源会话 + 写入者 + 版本。模型写入前先过一道
//! 「是否别的会话也需要知道」的判断，用来挡掉会话内的临时细节；用户手改
//! 绕过判断直接生效。清单本身注入每一轮上下文，模型平时只需要读，不需要反复调用本工具。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::brain::work_facts::{
    global_work_facts, judge_instruction, remember_rejection, take_rejection, WRITER_MODEL,
    WRITER_USER,
};
use crate::providers::router::ModelRouter;
use crate::state::AppState;
use crate::tools::types::{PermissionResult, Tool, ToolResult, ToolUseContext, ValidationResult};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用，与待办工具共用同一套注入方式）。
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

fn emit_facts_changed() {
    if let Some(handle) = APP_HANDLE.read().clone() {
        let store = global_work_facts();
        let _ = handle.emit(
            "work_facts:changed",
            json!({ "revision": store.revision(), "facts": store.list() }),
        );
    }
}

/// 取出判断用的 router；未就绪时返回 None，调用方降级为「不过滤」。
fn judge_router() -> Option<Arc<ModelRouter>> {
    let app = APP_HANDLE.read().clone()?;
    let state = app.state::<Arc<AppState>>();
    let router = state.model_router.read().clone();
    let resolved = router.map(Arc::new);
    resolved
}

/// 候选事实是否值得跨会话共享。判断失败时放行：漏挡一条噪音，
/// 好过让一次模型调用失败阻断正常写入。
async fn worth_sharing(text: &str, session_id: &str, char_id: &str) -> bool {
    let Some(router) = judge_router() else {
        return true;
    };
    match router
        .choose_simple(
            json!({ "fact": text, "session": session_id }),
            judge_instruction(),
            &[("yes", "值得所有工作会话共享"), ("no", "只属于当前会话")],
            char_id,
        )
        .await
    {
        Some(choice) => choice == "yes",
        None => true,
    }
}

const DESC_ZH: &str = "维护跨工作会话共享的一句话事实清单。清单会注入每一轮上下文，\
所以「读」不需要调用本工具；只有确认了新的稳定前提、用户偏好或跨任务决策时才写入。\
action=list 查看当前清单；action=add 新增一条（text）；action=update 改写一条（id + text）；\
action=remove 删除一条（id）。新增会先判断这条事实是否别的会话也需要知道——只属于当前会话的\
临时细节会被拒绝，不要反复尝试。事实是一句话（240 字以内），不写段落、不写推测、\n\
不要把能从工作区文件直接读到的内容抄进来。用户手动编辑的条目优先，不要覆盖与用户手改冲突的内容。";

const DESC_EN: &str = "Maintain the one-line facts shared across work sessions. The list is injected \
into every turn, so reading it needs no tool call. Write only stable premises, explicit user \
preferences, or decisions that other tasks must follow. action=list shows the current list; \
action=add appends one fact (text); action=update rewrites one (id + text); action=remove deletes \
one (id). A new fact is first checked for whether other sessions need it — session-local details \
are rejected, so do not retry them. One sentence per fact (under 240 characters), no paragraphs, \
no speculation, and do not copy content that workspace files already contain. Entries the user \
edited by hand take precedence.";

const DESC_JA: &str = "作業セッション間で共有する一行事実の一覧を維持します。一覧は毎ターンの\
コンテキストに注入されるため、読むだけならツール呼び出しは不要です。安定した前提・ユーザーの\
明示的な好み・複数タスクが従う決定だけを書き込んでください。action=list は一覧表示、\
action=add は追加（text）、action=update は書き換え（id + text）、action=remove は削除（id）。\
追加時は他のセッションにも必要な情報かを先に判定します。そのセッション限りの一時的な詳細は\
拒否されるので、繰り返さないでください。事実は一文（240文字以内）で、段落や推測は書かず、\
ワークスペースのファイルから直接読める内容を転記しないでください。ユーザーが手で編集した\
項目が優先です。";

pub struct WorkFactsTool;

impl WorkFactsTool {
    pub fn new() -> Self {
        Self
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "update", "remove"],
                    "description": "list: read the shared list. add: append one fact. update: rewrite one by id. remove: delete one by id."
                },
                "text": {
                    "type": "string",
                    "description": "One sentence (<=240 chars). Required for add and update."
                },
                "id": {
                    "type": "string",
                    "description": "Fact id such as fact_3. Required for update and remove."
                },
                "override_rejection": {
                    "type": "string",
                    "description": "Rejection id returned by a previous add. Resubmits the SAME text despite the shared-fact judgement; single use and expires after ten minutes."
                }
            },
            "required": ["action"]
        })
    }

    fn render_list() -> String {
        let store = global_work_facts();
        let facts = store.list();
        if facts.is_empty() {
            return "跨会话事实清单为空。".to_string();
        }
        let mut text = format!("跨会话事实清单（rev {}，{} 条）：", store.revision(), facts.len());
        for fact in facts {
            text.push_str(&format!(
                "\n- {} [{}]{}",
                fact.text,
                fact.id,
                if fact.updated_by == WRITER_USER { "（用户确认）" } else { "" }
            ));
        }
        text
    }
}

impl Default for WorkFactsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorkFactsTool {
    fn name(&self) -> &str {
        "work_facts"
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
            "zh" => "记录跨会话事实\n查看共享事实\n这条约定要记住",
            "en" => "record a fact across sessions\nshow shared facts\nremember this convention",
            "ja" => "セッション間の事実を記録\n共有事実を表示\nこの約束を覚える",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema()
    }

    /// 读动作无副作用，写动作会改持久化清单；按最重的一种声明。
    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> crate::tools::types::ToolCategory {
        crate::tools::types::ToolCategory::System
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "list" => ValidationResult::success(Some(input.clone())),
            "add" => {
                if input.get("text").and_then(Value::as_str).map(str::trim).is_some_and(|t| !t.is_empty()) {
                    ValidationResult::success(Some(input.clone()))
                } else {
                    ValidationResult::failure("add 需要 text（一句话事实）", 2)
                }
            }
            "update" => {
                if input.get("id").and_then(Value::as_str).is_some()
                    && input.get("text").and_then(Value::as_str).map(str::trim).is_some_and(|t| !t.is_empty())
                {
                    ValidationResult::success(Some(input.clone()))
                } else {
                    ValidationResult::failure("update 需要 id 与 text", 2)
                }
            }
            "remove" => {
                if input.get("id").and_then(Value::as_str).is_some() {
                    ValidationResult::success(Some(input.clone()))
                } else {
                    ValidationResult::failure("remove 需要 id", 2)
                }
            }
            _ => ValidationResult::failure("action 必须是 list / add / update / remove", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let store = global_work_facts();
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "list" => ToolResult::standard_success(
                &Self::render_list(),
                Some(json!({ "revision": store.revision(), "facts": store.list() })),
            ),
            "add" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                let override_id = args
                    .get("override_rejection")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                // 显式覆写只认「原样重提 + 有效编号」：编号一次性、十分钟过期、限本会话。
                let allowed = if !override_id.is_empty() {
                    if take_rejection(&context.session_id, override_id, text).is_none() {
                        return ToolResult::standard_error(
                            "拒绝编号无效、已过期，或与本条内容不匹配；请原样重提或接受这次的判断",
                            Some("WorkFactsError"),
                            None,
                        );
                    }
                    true
                } else {
                    worth_sharing(text, &context.session_id, &context.char_id).await
                };
                if !allowed {
                    let rejection = remember_rejection(&context.session_id, text);
                    return ToolResult::standard_success(
                        &format!(
                            "这条内容只对当前会话有意义，没有写入跨会话清单。\
                             若确认它跨任务仍然成立，可带 override_rejection={rejection} 原样重提一次；\
                             否则请改写成别的会话也需要知道的稳定前提，或让用户在工作记忆面板里手动添加。"
                        ),
                        Some(json!({ "recorded": false, "reason": "session_local", "rejection": rejection })),
                    );
                }
                match store.add(text, &context.session_id, WRITER_MODEL) {
                    Ok(fact) => {
                        emit_facts_changed();
                        ToolResult::standard_success(
                            &format!("已记录跨会话事实 [{}]：{}", fact.id, fact.text),
                            Some(json!({ "recorded": true, "fact": fact })),
                        )
                    }
                    Err(error) => ToolResult::standard_error(&error, Some("WorkFactsError"), None),
                }
            }
            "update" => {
                let id = args.get("id").and_then(Value::as_str).unwrap_or("");
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                match store.update(id, text, WRITER_MODEL) {
                    Ok(fact) => {
                        emit_facts_changed();
                        ToolResult::standard_success(
                            &format!("已更新事实 [{}]（rev {}）：{}", fact.id, fact.revision, fact.text),
                            Some(json!({ "fact": fact })),
                        )
                    }
                    Err(error) => ToolResult::standard_error(&error, Some("WorkFactsError"), None),
                }
            }
            "remove" => {
                let id = args.get("id").and_then(Value::as_str).unwrap_or("");
                match store.remove(id) {
                    Ok(true) => {
                        emit_facts_changed();
                        ToolResult::standard_success(
                            &format!("已删除事实 [{id}]"),
                            Some(json!({ "removed": true })),
                        )
                    }
                    Ok(false) => ToolResult::standard_error(
                        &format!("找不到事实 {id}"),
                        Some("WorkFactsError"),
                        None,
                    ),
                    Err(error) => ToolResult::standard_error(&error, Some("WorkFactsError"), None),
                }
            }
            _ => ToolResult::standard_error(
                "action 必须是 list / add / update / remove",
                Some("WorkFactsError"),
                None,
            ),
        }
    }
}
