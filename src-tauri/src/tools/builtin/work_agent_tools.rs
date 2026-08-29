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
        match CODING_AGENT.send_message(app.clone(), session_id.clone(), router, tool_system, task, Vec::new(), Vec::new(), max_rounds, false) {
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
        ToolRiskTier::Safe
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

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work status session progress"
    }
}

// ===== notify_companion =====

/// 即时播报节流：每角色最小间隔（秒），避免 agent 循环内连续刷屏。
const NOTIFY_MIN_INTERVAL_SECS: f64 = 60.0;

/// 每角色最近一次即时播报时间戳。
static LAST_NOTIFY: Lazy<RwLock<std::collections::HashMap<String, f64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 把阶段性成果发送给陪伴角色，由其以人设口吻主动向用户播报。
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
        "Send a staged work result to your companion persona, who will proactively tell the user about it in the character's own voice. Call when you reach a meaningful milestone — a phase finished, a build passing, an important finding — not for every small step. One short report per milestone."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把阶段性工作成果发送给你的陪伴人格，由 TA 以角色口吻主动向用户播报。在到达有意义的节点时调用——某阶段完成、构建通过、重要发现——而不是每小步都报。每个节点一次简短汇报。",
            "ja" => "段階的な作業成果をコンパニオンペルソナに送信し、キャラ口調でユーザーに能動的に報告してもらう。意味のあるマイルストーン到達時——フェーズ完了、ビルド成功、重要な発見——に呼び出す。小さな一歩ごとに呼ばない。マイルストーンごとに1回の簡潔な報告。",
            _ => self.description(),
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
        let Some(app) = APP_HANDLE.read().clone() else {
            return ToolResult::standard_error("无法播报（后端未初始化）", Some("AppUnavailable"), None);
        };
        let char_id = context.char_id.clone();
        if char_id.is_empty() {
            return ToolResult::standard_error("缺少角色上下文，无法播报", Some("NoCharacter"), None);
        }
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("工作进展")
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        // 节流：60s 内已有即时播报 → 本次不重复打扰（轮末摘要仍会入库）
        let now = chrono::Local::now().timestamp() as f64;
        let throttled = {
            let map = LAST_NOTIFY.read();
            map.get(&char_id).map(|&ts| now - ts < NOTIFY_MIN_INTERVAL_SECS).unwrap_or(false)
        };
        if throttled {
            return ToolResult::standard_success(
                "已记录该阶段成果（距上次播报不足 60 秒，本次不即时播报，稍后自然提及）。",
                None,
            );
        }
        LAST_NOTIFY.write().insert(char_id.clone(), now);

        // 异步生成播报并投递，不阻塞 agent 循环
        tauri::async_runtime::spawn(async move {
            let state = app.state::<Arc<AppState>>();
            let instance = match state.get_character(Some(char_id.as_str())) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!("[NotifyCompanion] 角色不存在: {e}");
                    return;
                }
            };
            let brain = instance.brain.clone();

            // 以内部事件形态走完整陪伴管线（记忆/情绪/人设全部生效）；
            // skip_dialogue_write=true：通知本身不作为用户消息落历史
            let input = format!(
                "（系统通知：你的工作智能体刚完成一个阶段）\n标题：{title}\n内容：{message}\n\n请以你的口吻，用一两句话自然地向用户播报这个进展——像顺手提一句你刚忙完的事，不要机械复述，不要列表。"
            );
            let response = match brain.think_with_options(&input, false, true).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("[NotifyCompanion:{}] 生成播报失败: {e}", char_id);
                    return;
                }
            };
            let text = crate::utils::strip_markdown_syntax(response.text.trim());
            if text.is_empty() {
                tracing::debug!("[NotifyCompanion:{}] 播报为空，跳过投递", char_id);
                return;
            }

            // 对话历史 + 记忆（与主动消息同构：channel=proactive）
            let mut m = crate::types::response::ChatMessage::assistant(&text);
            m.meta = Some(crate::messages::MessageMeta::new(crate::messages::MessageSource::Assistant)
                .with_channel("proactive"));
            brain.dialogue.add_message(m);
            {
                let memory = brain.memory.clone();
                let cid = char_id.clone();
                let mem_text = text.clone();
                tokio::spawn(async move {
                    let meta = serde_json::json!({
                        "channel": "proactive",
                        "speaker": cid,
                        "listener": "user",
                        "perspective": "speaker",
                        "knowledge_source": "direct",
                        "trigger": "work_report",
                    });
                    let _ = memory
                        .add_memory_with_metadata(
                            &mem_text,
                            crate::memory::types::MemoryType::CasualConversation,
                            0.4,
                            vec![
                                "assistant".to_string(),
                                "proactive".to_string(),
                                "work_report".to_string(),
                            ],
                            meta,
                        )
                        .await;
                });
            }

            // 发言时间戳 + 前端投递（proactive:bubble：TTS + 气泡 + 聊天记录）
            crate::commands::proactive::touch_last_spoken(&char_id);
            let _ = tauri::Emitter::emit(
                &app,
                "proactive:bubble",
                json!({
                    "character_id": &char_id,
                    "content": &text,
                    "expression": response.expression,
                }),
            );
            let _ = tauri::Emitter::emit(
                &app,
                "proactive:spoken",
                json!({ "character_id": &char_id, "timestamp": now }),
            );
            tracing::info!("[NotifyCompanion:{}] 已播报阶段成果：{}", char_id, title);
        });

        ToolResult::standard_success(
            "已把阶段成果发给陪伴角色，TA 会用自己的口吻向用户播报。",
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
