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

use crate::brain::coding_agent::{CodingRole, CodingSession};
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

/// 取工作会话最后一条可见的助手总结。
///
/// 工具调用意图与结果使用独立 role；这里只返回工作智能体真正交付给用户的
/// assistant 文本。限制长度是为了避免状态查询把整个陪伴侧上下文挤满。
fn latest_work_summary(session: &CodingSession) -> Option<String> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| {
            message.role == CodingRole::Assistant && !message.content.trim().is_empty()
        })
        .map(|message| message.content.trim().chars().take(4000).collect())
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
        "Use the work agent as an execution tool shared by the user and companion. Write a self-contained task prompt on the user's behalf, preserving the user's objective, constraints, expected deliverable, and known context without inventing authority. The work runs in a separate background session. You receive a session_id immediately; get_work_status returns progress and the final work summary. After delegation, do not duplicate the same execution yourself."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把工作智能体当作用户和陪伴侧共同使用的执行工具。你要以用户身份写一份自包含的任务提示词，完整保留用户目标、约束、期望交付物和已知上下文，不添加用户没有给出的授权。任务在独立会话后台执行，立即返回 session_id；get_work_status 可读取进度和最终工作总结。派发后不要自己重复执行同一项工作。",
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
                "task": {"type": "string", "description": "Self-contained task prompt written on the user's behalf. Include objective, constraints, relevant context, expected deliverable, and verification criteria; do not invent authorization."},
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
                    "task": {"type": "string", "description": "以用户身份写给工作智能体的自包含提示词：包含目标、约束、相关上下文、预期交付物和验证标准，不得虚构授权。"},
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
            Some(t) if !t.trim().is_empty() => ValidationResult::success(Some(input.clone())),
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
                    .filter(|s| s.char_id == context.char_id)
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
        if let Err(e) = CODING_AGENT.mark_delegated_by_companion(&session_id) {
            CODING_AGENT.delete_session(&session_id);
            return ToolResult::standard_error(
                &format!("派发失败：无法标记任务来源：{e}"),
                Some("DelegateSourceFailed"),
                None,
            );
        }
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
                        "delegated_by_companion": true,
                    }),
                );
                ToolResult::standard_success(
                    &format!("任务已派发给工作智能体（会话 {session_id}），正在后台执行。可用 get_work_status 查询进度。"),
                    Some(json!({
                        "session_id": session_id,
                        "working_directory": session.working_directory,
                        "mode": session.mode,
                        "delegated_by_companion": true,
                    })),
                )
            }
            Err(e) => {
                // 首条消息都没能启动时不留下一个看似可继续的空会话。
                CODING_AGENT.delete_session(&session_id);
                ToolResult::standard_error(
                    &format!("派发失败：{e}"),
                    Some("DelegateFailed"),
                    None,
                )
            }
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
        "Check work-agent sessions. With session_id, returns status plus the latest final assistant summary when available; omit it to list recent sessions. Use the returned summary as the work agent's report: preserve its outcome, verification, and blockers when relaying it to the user."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查询工作智能体会话。传 session_id 时会返回状态，以及可用时工作智能体最后一条完整总结；省略则列出最近会话。向用户转达时要保留总结里的结果、验证和阻塞，不要擅自美化。",
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

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let mut sessions = CODING_AGENT.list_sessions();
        // 陪伴侧只读取绑定到自身角色的工作会话，避免跨角色串任务或拿错总结。
        sessions.retain(|session| session.char_id == context.char_id);
        if let Some(sid) = args.get("session_id").and_then(|v| v.as_str()) {
            match sessions.iter().find(|s| s.session_id == sid) {
                Some(s) => {
                    let final_summary = latest_work_summary(s);
                    let message = match &final_summary {
                        Some(summary) => format!(
                            "会话「{}」状态：{}\n\n工作智能体总结：\n{}",
                            s.title,
                            status_text(s.status),
                            summary
                        ),
                        None => format!(
                            "会话「{}」状态：{}，尚无可返回的工作总结。",
                            s.title,
                            status_text(s.status)
                        ),
                    };
                    ToolResult::standard_success(
                        &message,
                        Some(json!({
                            "session_id": s.session_id,
                            "title": s.title,
                            "status": status_text(s.status),
                            "mode": s.mode,
                            "working_directory": s.working_directory,
                            "updated_at": s.updated_at,
                            "message_count": s.messages.len(),
                            "delegated_by_companion": s.delegated_by_companion,
                            "final_summary": final_summary,
                        })),
                    )
                }
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
                        "delegated_by_companion": s.delegated_by_companion,
                        "has_final_summary": latest_work_summary(s).is_some(),
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
