//! 工作智能体桥接工具 — 让陪伴对话中的 LLM 以用户身份向工作智能体派发任务。
//!
//! 双智能体形态：陪伴智能体（主对话人格）在交流中识别到用户的工作需求时，
//! 调用 `delegate_to_work_agent` 把任务交给工作智能体（会话式编程/执行 agent）
//! 后台执行；工作智能体完成后经既有记忆链路（每轮摘要入库）让陪伴侧自然知晓
//! 结果，实现"陪伴中顺手派活、做完自然汇报"的协作。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::commands::coding_agent::CODING_AGENT;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 全局 AppHandle（lib.rs setup 注入，用于取 AppState 与 emit 事件）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 会话状态转文本。
fn status_text(s: crate::brain::coding_agent::CodingStatus) -> &'static str {
    use crate::brain::coding_agent::CodingStatus;
    match s {
        CodingStatus::Idle => "空闲",
        CodingStatus::Running => "运行中",
        CodingStatus::Canceled => "已取消",
    }
}

// ===== delegate_to_work_agent =====

/// 以用户身份向工作智能体派发任务。
pub struct DelegateToWorkAgentTool;

impl DelegateToWorkAgentTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DelegateToWorkAgentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DelegateToWorkAgentTool {
    fn name(&self) -> &str {
        "delegate_to_work_agent"
    }

    fn description(&self) -> &str {
        "Delegate a work task to the work agent (a separate coding/execution agent) on behalf of the user. Use when the user mentions work that needs doing — coding, file processing, running commands, multi-step execution — while you keep chatting as the companion. The task runs in the background in its own session; you get a session_id immediately and can check progress with get_work_status. The work agent summarizes its results into memory when done, so you can naturally mention them later."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "以用户身份把工作任务派发给工作智能体（独立的编程/执行智能体）。当用户提到需要完成的工作——写代码、处理文件、跑命令、多步执行——而你继续以陪伴身份聊天时使用。任务在其独立会话中后台执行；立即返回 session_id，之后可用 get_work_status 查进度。工作智能体完成后会把结果摘要写入记忆，你可以自然地向用户提及。",
            "ja" => "ユーザーの代理として作業エージェント（独立したコーディング/実行エージェント）にタスクを委任する。コーディング、ファイル処理、コマンド実行、多段階実行などユーザーが作業を必要としていることに会話中に気づいた際、あなたはコンパニオンとして話し続けながら使用する。タスクは独立セッションでバックグラウンド実行され、即座に session_id が返る。進捗は get_work_status で確認できる。作業エージェントは完了時に結果サマリをメモリに書き込むので、後で自然に言及できる。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "让工作智能体去做\n交给编程那边\n这个交给它处理",
            "en" => "hand this to the coding agent\nlet the work agent handle it\ndelegate this",
            "ja" => "作業エージェントに任せて\nプログラム側に任せる\nこれは委任して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Complete task instruction for the work agent, written as if from the user"},
                "working_directory": {"type": "string", "description": "Optional working directory for the task. Omit to reuse the most recent work session's directory."},
                "mode": {"type": "string", "enum": ["standard", "code", "minimal"], "description": "Work agent mode (default standard)"}
            },
            "required": ["task"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "给工作智能体的完整任务说明（以用户口吻书写）"},
                    "working_directory": {"type": "string", "description": "可选的工作目录。省略则复用最近一次工作会话的目录。"},
                    "mode": {"type": "string", "enum": ["standard", "code", "minimal"], "description": "工作智能体模式（默认 standard）"}
                },
                "required": ["task"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "作業エージェントへの完全なタスク指示（ユーザー口調で記述）"},
                    "working_directory": {"type": "string", "description": "任意の作業ディレクトリ。省略時は直近の作業セッションのディレクトリを再利用。"},
                    "mode": {"type": "string", "enum": ["standard", "code", "minimal"], "description": "作業エージェントのモード（デフォルト standard）"}
                },
                "required": ["task"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("task").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("task 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let Some(app) = APP_HANDLE.read().clone() else {
            return ToolResult::standard_error("无法派发任务（后端未初始化）", Some("WorkAgentUnavailable"), None);
        };

        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("standard")
            .to_string();

        // 工作目录：显式指定 > 最近一次工作会话目录 > 用户主目录
        let working_directory = match args.get("working_directory").and_then(|v| v.as_str()) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => {
                let sessions = CODING_AGENT.list_sessions();
                sessions
                    .iter()
                    .max_by_key(|s| s.updated_at)
                    .map(|s| s.working_directory.clone())
                    .unwrap_or_default()
            }
        };

        let state = app.state::<Arc<AppState>>();
        let router = match state.model_router.read().clone() {
            Some(r) => r,
            None => {
                return ToolResult::standard_error(
                    "模型路由未初始化，无法派发工作任务",
                    Some("RouterUnavailable"),
                    None,
                )
            }
        };
        let tool_system = state.tool_system.clone();
        // 单轮 LLM↔工具 循环预算（与设置-工具-编程智能体最大轮次一致）
        let max_rounds = state.config.read().get_all().tools.max_coding_rounds as usize;

        // 创建工作会话并以用户身份发送任务（异步后台执行）
        let session = CODING_AGENT.create_session(&context.char_id, &working_directory, &mode);
        let session_id = session.session_id.clone();
        match CODING_AGENT.send_message(app.clone(), session_id.clone(), router, tool_system, task, Vec::new(), Vec::new(), max_rounds, false, false) {
            Ok(()) => {
                // 广播给前端（工作面板可感知新任务）
                let _ = tauri::Emitter::emit(
                    &app,
                    "work:delegated",
                    json!({
                        "session_id": session_id,
                        "char_id": context.char_id,
                        "working_directory": session.working_directory,
                        "mode": session.mode,
                    }),
                );
                ToolResult::standard_success(
                    &format!("任务已派发给工作智能体（会话 {session_id}），正在后台执行。可用 get_work_status 查询进度。"),
                    Some(json!({
                        "session_id": session_id,
                        "working_directory": session.working_directory,
                        "mode": session.mode,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error(&format!("派发失败：{e}"), Some("DelegateFailed"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "delegate work task agent coding"
    }
}

// ===== get_work_status =====

/// 查询工作智能体会话状态。
pub struct GetWorkStatusTool;

impl GetWorkStatusTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetWorkStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetWorkStatusTool {
    fn name(&self) -> &str {
        "get_work_status"
    }

    fn description(&self) -> &str {
        "Check the status of work agent sessions. Optional session_id for one session; omit to list recent sessions (returns id, title, status, working directory, last update). Use after delegating a task to report progress to the user."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查询工作智能体会话状态。可选 session_id 查单个会话；省略则列出最近会话（id、标题、状态、工作目录、最近更新）。派发任务后用它向用户汇报进度。",
            "ja" => "作業エージェントのセッション状態を確認する。任意の session_id で1件取得、省略時は最近のセッション一覧（id、タイトル、状態、作業ディレクトリ、最終更新）。タスク委任後にユーザーへ進捗を報告するために使用。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "那个任务进度如何\n它做到哪一步了\n工作智能体在干嘛",
            "en" => "how's that task going\nwhat's the progress\nwhat is the work agent doing",
            "ja" => "あのタスクの進捗は\nどこまで進んだ\n作業の状況は",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "description": "Optional session ID to query one session"}
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "可选的会话 ID（查单个会话）"}
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "任意のセッション ID（1件取得）"}
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let sessions = CODING_AGENT.list_sessions();
        if let Some(sid) = args.get("session_id").and_then(|v| v.as_str()) {
            match sessions.iter().find(|s| s.session_id == sid) {
                Some(s) => ToolResult::standard_success(
                    &format!("会话「{}」状态：{}", s.title, status_text(s.status)),
                    Some(json!({
                        "session_id": s.session_id,
                        "title": s.title,
                        "status": status_text(s.status),
                        "mode": s.mode,
                        "working_directory": s.working_directory,
                        "updated_at": s.updated_at,
                        "message_count": s.messages.len(),
                    })),
                ),
                None => ToolResult::standard_error("会话不存在", Some(&format!("未找到 {sid}")), None),
            }
        } else {
            let mut recent: Vec<_> = sessions.into_iter().collect();
            recent.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
            recent.truncate(5);
            let arr: Vec<Value> = recent
                .iter()
                .map(|s| {
                    json!({
                        "session_id": s.session_id,
                        "title": s.title,
                        "status": status_text(s.status),
                        "working_directory": s.working_directory,
                        "updated_at": s.updated_at,
                    })
                })
                .collect();
            ToolResult::standard_success(
                &format!("共 {} 个工作会话（最近）", arr.len()),
                Some(json!({ "sessions": arr })),
            )
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work status session progress"
    }
}

/// 把需要用户知道的事交给陪伴角色，由 TA 以角色口吻转告。
///
/// 只登记事实，不生成文案、不直投气泡——文案由陪伴角色在它自己的主动交互
/// 流程里生成（人设、语气、反重复都在那儿），否则角色一开口就不像自己。
pub struct NotifyCompanionTool;

impl NotifyCompanionTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NotifyCompanionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for NotifyCompanionTool {
    fn name(&self) -> &str {
        "notify_companion"
    }

    fn description(&self) -> &str {
        "Hand something to your companion persona so it tells the user in the character's own voice. Use it for things the user should know but cannot see in the work panel — a key finding, a blocker, a decision they need to make. Do NOT use it for routine progress the panel already shows: if the user is looking at this session's work page, the message will not be repeated out loud."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把需要用户知道的事交给你的陪伴人格，由 TA 以角色口吻转告用户。用于用户在工作面板上看不到、但应该知道的信息——关键发现、卡点、需要他拍板的事。不要用来说面板上本来就显示着的常规进度：用户正看着本会话的工作页时，这条不会再重复说。",
            "ja" => "ユーザーが知っておくべきことをコンパニオンペルソナに渡し、キャラ口調で伝えてもらう。作業パネルでは見えないが知っておくべき情報——重要な発見、詰まり、判断を仰ぐ事柄——に使う。パネルに既に表示されている通常の進捗には使わない：ユーザーがこのセッションの作業ページを見ている場合は口頭で繰り返されない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "通知一下她\n跟她说一声\n告诉另一个",
            "en" => "notify her\nlet her know\ntell the other one",
            "ja" => "彼女に知らせて\nあの人に伝えて\n通知して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short milestone title (e.g. 'build passing')"},
                "message": {"type": "string", "description": "What was accomplished, key results, anything the user should know"}
            },
            "required": ["message"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "里程碑短标题（如「构建通过」）"},
                    "message": {"type": "string", "description": "完成了什么、关键结果、用户需要知道的事"}
                },
                "required": ["message"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "マイルストーンの短いタイトル（例「ビルド成功」）"},
                    "message": {"type": "string", "description": "何を達成したか、重要な結果、ユーザーが知るべきこと"}
                },
                "required": ["message"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("message 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let char_id = context.char_id.clone();
        if char_id.is_empty() {
            return ToolResult::standard_error(
                "缺少角色上下文，无法转达",
                Some("NoCharacter"),
                None,
            );
        }
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("工作进展");
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if message.is_empty() {
            return ToolResult::standard_error("message 不能为空", Some("EmptyMessage"), None);
        }

        let ok = crate::brain::work_notices::global().push_alert(
            &char_id,
            &context.session_id,
            title,
            &message,
        );
        if !ok {
            return ToolResult::standard_error(
                "登记失败（角色或内容为空）",
                Some("NotifyFailed"),
                None,
            );
        }
        ToolResult::standard_success(
            "已登记，陪伴角色会用自己的口吻转告用户。\
             若用户此刻正开着这个会话的工作页，就不会再重复说一遍——他已经看到了。",
            None,
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

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "notify companion report milestone progress user"
    }
}
