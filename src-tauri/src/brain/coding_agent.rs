//! 编程智能体服务（会话式 agent 形态）
//!
//! 与 `TaskService`（单 directive 一次性跑完）不同，CodingAgent 是**会话式**的：
//! 用户在界面上发消息 → 消息进入会话历史 → LLM 带编程工具集决策 → 工具调用
//! 逐个执行并把结果回填历史 → 循环直到 LLM 不再调工具（产出回复文本）→ 回到
//! 空闲等待下一条用户消息。
//!
//! 关键点：
//! - 工具调用走 `execute_tool_use`，自动经过沙箱/守卫/审批矩阵（与主对话一致）；
//! - 每个事件（消息/工具调用/工具结果/轮次完成/错误）通过 Tauri emit 广播，
//!   前端编程页面实时渲染聊天流与工具卡片；
//! - 会话持久化到 `<用户数据目录>/coding_sessions.json`，重启后可恢复；
//! - 上下文控制：工具结果超长截断 + 历史消息数上限，避免长会话撑爆上下文。

use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::pipeline::doom_loop::{DoomLoopTracker, LoopStatus};
use crate::providers::base::{LLMRequest, StreamEvent, ToolDefinition};
use crate::providers::reasoning::{ReasoningEffort, ReasoningMode, ReasoningPreference};
use crate::providers::router::ModelRouter;
use crate::resilience::{classify_llm_error_from_str, LlmErrorKind};
use crate::tools::executor::{execute_tool_use, CanUseTool};
use crate::tools::types::ToolUseContext;
use crate::tools::ToolSystem;
use crate::types::response::{ChatMessage, MessageToolCall};
use crate::utils::path::get_user_data_dir;

/// 编程会话单轮内最大工具循环轮数的兜底默认值（防止失控）。
///
/// 实际值由 `config.tools.max_coding_rounds` 提供（设置-工具可调，默认 48），
/// 命令层未传入时回退到本默认值。24 对 coding / computer-use 型 agent 偏少，
/// 且循环内置软预算提醒 / 停滞检测 / 有进展时自动续轮，见 `run_loop_inner`。
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 48;
/// 会话历史注入 LLM 的最大消息条数（工具结果消息优先裁剪最旧的）。
pub const MAX_HISTORY_MESSAGES: usize = 60;
/// 单条工具结果注入 LLM 的最大字符数。
pub const TOOL_RESULT_MAX_CHARS: usize = 6000;

/// 编程智能体上下文窗口上限（token，占位：按 DeepSeek 1M window 配置）。
pub const CODING_CONTEXT_WINDOW: usize = 1_000_000;

/// 编程智能体模式（standard/code/minimal）。
///
/// - `standard`：功能完整档，逐轮 LLM 决策调用 6 个编程工具（默认）
/// - `code`：程序化编排档（Code Mode 精髓），模型一次输出多步"程序"
///   （JSON 步骤序列），Rust 顺序执行不再逐步回询 LLM，末尾总结
/// - `minimal`：极简档（仅 run_command + edit_file，读取用 Get-Content）
pub const CODING_MODES: &[&str] = &["standard", "code", "minimal"];

/// 编程智能体可用的工具白名单（LLM 每轮只看到这些工具）。
///
/// 末三者为能力进化工具：工作智能体是"进化事件"的执行主体——沉淀方法论
/// （create_skill / use_skill）与构建新工具（create_tool，经用户预览卡片授权）。
pub const CODING_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "run_command",
    "grep_search",
    "list_dir",
    "run_workflow",
    "lsp_query",
    "notify_companion",
    "send_image",
    "create_skill",
    "use_skill",
    "create_tool",
];

/// Code 模式单次程序的最大步骤数。
pub const CODE_MODE_MAX_STEPS: usize = 16;

/// /compact 压缩后保留的最近消息条数（其余部分被摘要替换进上下文）。
const COMPACT_KEEP_MESSAGES: usize = 24;
/// /compact 可压缩的最小消息条数（不足则提示无需压缩）。
const COMPACT_MIN_MESSAGES: usize = 8;
/// 上下文占用达到窗口上限的该百分比时，自动压缩早期历史（防止请求超窗失败）。
const AUTO_COMPACT_THRESHOLD_PCT: u64 = 75;
/// /compact 旧历史摘要的系统提示词。
const COMPACT_SYSTEM_PROMPT: &str = "你是对话历史压缩器。把下面这段编程会话历史压缩成一份简洁但信息完整的中文摘要，保留：已解决的问题、关键文件路径、做出的改动、当前任务进展、遗留待办。不要复述每条工具输出细节，控制在 200 字以内，直接输出摘要正文。";

/// 项目记忆文件名（存储在应用数据目录，按工作目录隔离，不污染项目仓库）。
const PROJECT_MEMORY_FILE: &str = "project_memory.md";
/// 应用数据目录下的项目记忆存储目录（每个工作目录一个子目录）。
const CODING_MEMORY_DIR: &str = "coding_memory";
/// 项目记忆注入 system prompt 的最大字符数（超长保留尾部——最新沉淀的条目）。
const PROJECT_MEMORY_MAX_CHARS: usize = 8000;
/// project_memory.md 首次创建时写入的文件头（说明用途与维护方式）。
const PROJECT_MEMORY_HEADER: &str = "# 项目记忆\n\n\
    > 本文件由桌面编程智能体跨会话自动维护，沉淀对应工作目录项目的约定、结构与经验教训。\n\
    > 存储在应用数据目录（不进入项目仓库）；每次新会话自动注入上下文。\n\
    > 可用 /memory 查看、/memory 提炼 归纳、/memory <内容> 手动追加。\n";
/// 项目记忆提炼的 system prompt（/memory 提炼 与 /compact 归档沉淀共用）。
const MEMORY_DISTILL_SYSTEM_PROMPT: &str = "你是项目记忆沉淀模块。从一段编程会话历史中提炼**跨会话仍然有效**的项目知识：项目结构与关键路径、构建/测试命令、代码约定、踩过的坑与解法、用户偏好。只输出新增条目（markdown 无序列表，每条一行、简洁具体），与已有记忆重复的不要输出；没有值得沉淀的内容就输出空。不要输出标题、前言或总结。";
/// 项目记忆超过该行数时，提炼改为全文重写合并去重（防追加式无限膨胀）。
const PROJECT_MEMORY_MERGE_LINES: usize = 100;
/// 项目记忆全文重写的 system prompt（超阈值合并去重）。
const MEMORY_REWRITE_SYSTEM_PROMPT: &str = "你是项目记忆整理模块。当前项目记忆过长，请把它与会话历史中的新知识合并，重写为一份精简的记忆文件：合并重复条目、删除过时或一次性内容、按主题分节组织（如 项目结构 / 构建与命令 / 代码约定 / 经验教训 / 用户偏好）。保留所有仍然有效的信息，每条一行、简洁具体。直接输出重写后的 markdown 正文，不要输出文件标题、前言或总结。";
/// /plan 开启计划模式时注入的上下文策略。
const PLAN_MODE_POLICY: &str = "\n# 计划模式（当前已开启）\n\
    你现在处于**计划模式**：先用只读研究（list_dir / grep_search / read_file）理解问题并制定方案。\
    输出方案后停下来等待用户批准（用户会回应「批准」或执行 /plan approve）——在方案得到批准之前，\
    **不要修改任何文件，不要执行可能改变状态的命令**。方案说明要包含步骤与预期改动，一次输出完整方案，不要边做边问。";

/// 按模式过滤工具集。
fn tools_for_mode(mode: &str) -> Vec<&'static str> {
    match mode {
        "minimal" => vec!["run_command", "edit_file"],
        _ => CODING_TOOLS.to_vec(),
    }
}

/// 会话推理等级 → 推理偏好（low 关闭；medium / high 按档位开启）。
/// 档位经 provider 层按模型能力校验，不支持的档位自动回退默认档。
fn reasoning_level_to_pref(level: &str) -> ReasoningPreference {
    match level {
        "low" => ReasoningPreference { mode: ReasoningMode::Off, effort: None },
        "medium" => ReasoningPreference::on(Some(ReasoningEffort::Medium)),
        "high" => ReasoningPreference::on(Some(ReasoningEffort::High)),
        _ => ReasoningPreference::AUTO,
    }
}

/// 校验模式字符串合法。
pub fn valid_mode(mode: &str) -> bool {
    CODING_MODES.contains(&mode)
}

// ============================================================================
// 数据结构
// ============================================================================

/// 消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingRole {
    User,
    Assistant,
    ToolUse,
    ToolResult,
    Error,
}

/// 用户消息附带的图片（base64 内联）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingImage {
    /// MIME 类型（image/png / image/jpeg / image/webp / image/gif）
    pub media_type: String,
    /// base64 数据（不含 data: 前缀）
    pub data: String,
    /// 原文件名（可空）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
}

/// 用户消息附带的文件引用（@-mention 注入上下文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingFileRef {
    /// 文件绝对路径（相对路径在入站时解析为绝对）
    pub path: String,
    /// 文件内容（读取成功时注入上下文；超长截断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 读取失败原因（如文件不存在 / 超出沙箱）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 单文件引用内容上限（字符）。
const FILE_REF_MAX_CHARS: usize = 8000;
/// 单条消息文件引用数量上限。
const FILE_REF_MAX_COUNT: usize = 8;

/// 会话中的一条消息（用户文本 / 助手回复 / 工具调用 / 工具结果 / 错误）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingMessage {
    pub role: CodingRole,
    pub content: String,
    /// user 消息附带的图片列表（多模态输入，随消息持久化）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<CodingImage>>,
    /// user 消息附带的文件引用（@-mention 注入上下文，随消息持久化）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file_refs: Option<Vec<CodingFileRef>>,
    /// 用户在上一轮任务执行期间排队的插话（构建 LLM 消息时加插话标注，
    /// 帮助模型区分"对当前任务的补充/修正"与"全新对话"）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub interjected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_success: Option<bool>,
    /// 工具结果消息关联的调用 ID（与 assistant.tool_calls[].id 对应）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    pub timestamp: i64,
}

/// 会话运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingStatus {
    Idle,
    Running,
    Canceled,
}

/// 会话累计 token 用量（input 为未命中缓存的输入）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CodingTokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

/// 会话累计统计（turns/steps/tokens 的会话级投影，token 用量均为 API 上报值）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CodingStats {
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub llm_ms: u64,
    #[serde(default)]
    pub tool_ms: u64,
    #[serde(default)]
    pub usage: CodingTokenUsage,
    /// 累计首 token 耗时（ms，前端除 first_token_calls 求平均）
    #[serde(default)]
    pub first_token_ms: u64,
    /// 有首 token 采样的 LLM 调用次数
    #[serde(default)]
    pub first_token_calls: u64,
}

/// 编程会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingSession {
    pub session_id: String,
    pub char_id: String,
    pub working_directory: String,
    pub title: String,
    /// 工作模式：standard / code / minimal（缺省 standard，旧数据兼容）
    #[serde(default = "default_mode")]
    pub mode: String,
    /// 会话权限等级：read_only / workspace_write / full_access（缺省 workspace_write）
    #[serde(default = "default_permission")]
    pub permission: String,
    /// 会话选中的工作智能体模型 id（None 跟随默认路由；与 config.active_work_model 同步）
    #[serde(default)]
    pub model_id: Option<String>,
    /// 推理等级：low / medium / high（缺省 high）
    #[serde(default = "default_reasoning_level")]
    pub reasoning_level: String,
    /// 会话目标（/goal 设置，注入 system prompt）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// 计划模式开关（/plan 进入/退出，注入 plan policy）
    #[serde(default)]
    pub plan_mode: bool,
    /// 已批准的执行方案（/plan approve 或回复「批准」后固化，注入 system prompt）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// 会话反馈记录（/feedback 追加）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback: Vec<String>,
    /// 压缩后的旧对话摘要（/compact 生成，注入上下文）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted: Option<String>,
    /// 会话产物文件（write_file / edit_file 成功写入的绝对路径，去重）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<String>,
    /// 单条消息级反馈（消息下标 → "up" / "down"）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub message_feedback: HashMap<usize, String>,
    pub messages: Vec<CodingMessage>,
    pub status: CodingStatus,
    pub updated_at: i64,
    /// 会话累计统计（轮数/步数/LLM 耗时/工具耗时/token 用量）
    #[serde(default)]
    pub stats: CodingStats,
    /// 最近一次 LLM 请求的真实上下文规模（API usage 上报的输入侧 token：
    /// input + cache_read + cache_write），自动压缩以此为触发依据
    #[serde(default)]
    pub last_context_tokens: u64,
    /// 会话上下文窗口（tokens，自动压缩阈值判定基准）
    ///
    /// 会话创建 / 切换工作模型时从配置解析（work_models[].context_window →
    /// ai.context_window → 厂商默认窗口）。
    #[serde(default = "default_context_window_tokens")]
    pub context_window: u64,
}

/// 会话上下文窗口缺省值（旧持久化数据无此字段时的兜底）。
fn default_context_window_tokens() -> u64 {
    crate::providers::capabilities::default_context_window("")
}

fn default_mode() -> String {
    "standard".to_string()
}

fn default_permission() -> String {
    "workspace_write".to_string()
}

fn default_reasoning_level() -> String {
    "high".to_string()
}

/// 工作区信息（列表展示：路径 basename + 完整路径）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingWorkspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// 有效权限等级。
pub const CODING_PERMISSIONS: &[&str] = &["read_only", "workspace_write", "full_access"];

/// 沙箱确认回调：编程会话的工具执行恒放行沙箱层的"首次使用 / 前 N 次确认"。
///
/// 沙箱内置档案对 write_file/edit_file 设了 requires_confirmation，而会话式
/// 编程场景下执行器拿到"需要确认"且无回调时会直接报错（SandboxConfirmationRequired），
/// 连弹窗都没有。真正的边界由管线其余环节把守：
/// 沙箱路径校验（限工作目录）+ 权限矩阵（read_only 拒绝写入）+ 破坏性命令黑名单。
fn coding_sandbox_allow() -> Option<CanUseTool> {
    static CB: once_cell::sync::Lazy<CanUseTool> = once_cell::sync::Lazy::new(|| {
        Arc::new(|_tool_name: &str, _args: &serde_json::Value| true)
    });
    Some(Arc::clone(&*CB))
}

/// 有效推理等级。
pub const CODING_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];

/// 权限字符串 → 工具系统访问级别（read_only 只读、workspace_write 文件写入、full_access 完全控制）。
pub fn permission_to_access_level(permission: &str) -> crate::tools::types::AgentAccessLevel {
    use crate::tools::types::AgentAccessLevel::*;
    match permission {
        "read_only" => ReadOnly,
        "full_access" => FullControl,
        _ => FsWrite,
    }
}

// ============================================================================
// 服务
// ============================================================================

/// 编程智能体服务：会话注册表 + agent loop 执行器。
pub struct CodingAgentService {
    sessions: RwLock<BTreeMap<String, CodingSession>>,
}

impl CodingAgentService {
    pub fn new() -> Self {
        let mut svc = Self {
            sessions: RwLock::new(BTreeMap::new()),
        };
        svc.load_from_disk();
        svc
    }

    fn store_path() -> std::path::PathBuf {
        get_user_data_dir().join("coding_sessions.json")
    }

    fn load_from_disk(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::store_path()) else {
            return;
        };
        match serde_json::from_str::<Vec<CodingSession>>(&text) {
            Ok(list) => {
                // 启动恢复时所有会话重置为 Idle（上次运行中断的 Running 会话也回到空闲）
                let mut map = BTreeMap::new();
                for mut s in list {
                    s.status = CodingStatus::Idle;
                    map.insert(s.session_id.clone(), s);
                }
                *self.sessions.write() = map;
            }
            Err(e) => tracing::warn!("[CodingAgent] 会话文件解析失败: {e}"),
        }
    }

    fn persist(&self) {
        let sessions: Vec<CodingSession> = self.sessions.read().values().cloned().collect();
        // 限制持久化数量：保留最近 30 个会话
        let sessions: Vec<CodingSession> = {
            let mut v = sessions;
            if v.len() > 30 {
                v = v.split_off(v.len() - 30);
            }
            v
        };
        if let Ok(text) = serde_json::to_string_pretty(&sessions) {
            let path = Self::store_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&path, text) {
                tracing::warn!("[CodingAgent] 会话持久化失败: {e}");
            }
        }
    }

    /// 新建会话。
    pub fn create_session(&self, char_id: &str, working_directory: &str, mode: &str) -> CodingSession {
        let session = CodingSession {
            session_id: format!("code-{}", uuid::Uuid::new_v4().simple()),
            char_id: char_id.to_string(),
            working_directory: working_directory.to_string(),
            title: String::new(),
            mode: if valid_mode(mode) { mode.to_string() } else { default_mode() },
            permission: default_permission(),
            model_id: None,
            reasoning_level: default_reasoning_level(),
            goal: None,
            plan_mode: false,
            plan: None,
            feedback: Vec::new(),
            compacted: None,
            deliverables: Vec::new(),
            message_feedback: HashMap::new(),
            messages: Vec::new(),
            status: CodingStatus::Idle,
            updated_at: chrono::Utc::now().timestamp(),
            stats: CodingStats::default(),
            last_context_tokens: 0,
            context_window: crate::providers::capabilities::default_context_window(""),
        };
        self.sessions.write().insert(session.session_id.clone(), session.clone());
        self.persist();
        session
    }

    /// 切换会话工作模式（运行中拒绝切换）。
    pub fn set_mode(&self, session_id: &str, mode: &str) -> Result<(), String> {
        if !valid_mode(mode) {
            return Err(format!("未知模式: {mode}（可选: {}）", CODING_MODES.join("/")));
        }
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.mode = mode.to_string();
                drop(guard);
                self.persist();
                Ok(())
            }
            Some(_) => Err("会话正在运行，不能切换模式".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 历史会话中出现过的工作区列表（去重，按最近使用倒序）。
    pub fn list_workspaces(&self) -> Vec<CodingWorkspace> {
        let sessions = self.sessions.read();
        let mut seen: Vec<CodingWorkspace> = Vec::new();
        let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 按 updated_at 倒序收集唯一工作目录
        let mut all: Vec<&CodingSession> = sessions.values().collect();
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        for s in all {
            let path = s.working_directory.clone();
            if path.is_empty() || !seen_paths.insert(path.clone()) {
                continue;
            }
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            seen.push(CodingWorkspace {
                id: path.clone(),
                name,
                path,
            });
        }
        seen
    }

    /// 切换会话工作目录（运行中拒绝；目录必须存在）。
    pub fn set_workspace(&self, session_id: &str, working_directory: &str) -> Result<(), String> {
        if working_directory.is_empty() {
            return Err("工作目录不能为空".into());
        }
        if !std::path::Path::new(working_directory).is_dir() {
            return Err(format!("工作目录不存在: {working_directory}"));
        }
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.working_directory = working_directory.to_string();
                drop(guard);
                self.persist();
                Ok(())
            }
            Some(_) => Err("会话正在运行，不能切换工作区".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 设置会话权限等级（运行中拒绝）。
    pub fn set_permission(&self, session_id: &str, permission: &str) -> Result<(), String> {
        if !CODING_PERMISSIONS.contains(&permission) {
            return Err(format!("未知权限: {permission}（可选: {}）", CODING_PERMISSIONS.join("/")));
        }
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.permission = permission.to_string();
                drop(guard);
                self.persist();
                Ok(())
            }
            Some(_) => Err("会话正在运行，不能切换权限".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 设置会话选中的工作模型 id（运行中拒绝）。
    pub fn set_model(&self, session_id: &str, model_id: &str) -> Result<(), String> {
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.model_id = Some(model_id.to_string());
                drop(guard);
                self.persist();
                Ok(())
            }
            Some(_) => Err("会话正在运行，不能切换模型".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 设置会话推理等级（运行中拒绝）。
    pub fn set_reasoning_level(&self, session_id: &str, level: &str) -> Result<(), String> {
        if !CODING_REASONING_LEVELS.contains(&level) {
            return Err(format!("未知推理等级: {level}（可选: {}）", CODING_REASONING_LEVELS.join("/")));
        }
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.reasoning_level = level.to_string();
                drop(guard);
                self.persist();
                Ok(())
            }
            Some(_) => Err("会话正在运行，不能切换推理等级".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 设置单条消息级反馈（"up" / "down"，传空值清除）。消息下标基于会话消息列表。
    pub fn set_message_feedback(&self, session_id: &str, message_index: usize, rating: &str) -> Result<(), String> {
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) => {
                if message_index >= s.messages.len() {
                    return Err(format!("消息下标越界：{message_index}（共 {} 条）", s.messages.len()));
                }
                match rating {
                    "up" | "down" => {
                        s.message_feedback.insert(message_index, rating.to_string());
                    }
                    "" => {
                        s.message_feedback.remove(&message_index);
                    }
                    _ => return Err("rating 必须是 up / down / 空".into()),
                }
                drop(guard);
                self.persist();
                Ok(())
            }
            None => Err("会话不存在".into()),
        }
    }

    /// 从指定消息处 fork 出新的独立会话（继承工作目录/模式/权限/模型，复制历史到该条消息）。
    pub fn fork_session(&self, session_id: &str, message_index: usize) -> Result<CodingSession, String> {
        let (base, slice) = {
            let guard = self.sessions.read();
            let s = guard.get(session_id).ok_or("会话不存在")?;
            if message_index >= s.messages.len() {
                return Err(format!("消息下标越界：{message_index}（共 {} 条）", s.messages.len()));
            }
            let slice = s.messages[..=message_index].to_vec();
            (s.clone(), slice)
        };
        let mut fork = self.create_session(&base.char_id, &base.working_directory, &base.mode);
        fork.permission = base.permission.clone();
        fork.model_id = base.model_id.clone();
        fork.reasoning_level = base.reasoning_level.clone();
        fork.title = format!("{}（fork）", base.title);
        fork.messages = slice;
        self.sessions.write().insert(fork.session_id.clone(), fork.clone());
        self.persist();
        Ok(fork)
    }

    /// 删除会话。
    pub fn delete_session(&self, session_id: &str) -> bool {
        let removed = self.sessions.write().remove(session_id).is_some();
        if removed {
            self.persist();
        }
        removed
    }

    /// 会话简表（列表页用）。
    pub fn list_sessions(&self) -> Vec<CodingSession> {
        self.sessions.read().values().cloned().collect()
    }

    /// 取完整会话。
    pub fn get_session(&self, session_id: &str) -> Option<CodingSession> {
        self.sessions.read().get(session_id).cloned()
    }

    /// 取消正在运行的会话（下一轮循环前生效）。
    pub fn cancel(&self, session_id: &str) -> bool {
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status == CodingStatus::Running => {
                s.status = CodingStatus::Canceled;
                true
            }
            _ => false,
        }
    }

    fn is_canceled(&self, session_id: &str) -> bool {
        self.sessions
            .read()
            .get(session_id)
            .map(|s| s.status == CodingStatus::Canceled)
            .unwrap_or(true)
    }

    fn push_message(&self, session_id: &str, msg: CodingMessage) {
        {
            let mut guard = self.sessions.write();
            if let Some(s) = guard.get_mut(session_id) {
                // 斜杠命令不作为会话标题（首条用户消息为命令时保持标题为空）
                if s.title.is_empty() && msg.role == CodingRole::User && !msg.content.starts_with('/') {
                    let t: String = msg.content.chars().take(30).collect();
                    s.title = t;
                }
                s.updated_at = chrono::Utc::now().timestamp();
                s.messages.push(msg);
            }
        }
    }

    /// 会话是否存在（send_image 工具据 session_id 路由编程页/微信面板通道）。
    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions.read().contains_key(session_id)
    }

    /// 智能体向会话推送图片消息（send_image 工具调用）。
    ///
    /// 图片作为 assistant 消息追加进会话（images 内联 base64，随会话持久化，
    /// 恢复会话时前端直接重渲染），并广播 `coding:assistant_message`（携带
    /// images）供编程页实时渲染。caption 为可选说明文本（可为空）。
    pub fn push_agent_image(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        images: Vec<CodingImage>,
        caption: &str,
    ) -> Result<(), String> {
        if images.is_empty() {
            return Err("图片列表为空".to_string());
        }
        if !self.has_session(session_id) {
            return Err("会话不存在".to_string());
        }
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::Assistant,
                content: caption.to_string(),
                images: Some(images.clone()),
                file_refs: None,
                interjected: None,
                tool_name: None,
                tool_arguments: None,
                tool_success: None,
                tool_call_id: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        let _ = app.emit(
            "coding:assistant_message",
            serde_json::json!({
                "session_id": session_id,
                "content": caption,
                "images": images,
            }),
        );
        Ok(())
    }

    /// 会话累计统计：轮次开始。
    fn stats_turn_started(&self, session_id: &str) {
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.stats.turns += 1;
        }
    }

    /// 会话累计统计：一步 LLM 调用完成（含耗时与 token 用量）。
    fn stats_step_done(&self, session_id: &str, llm_ms: u64, usage: Option<CodingTokenUsage>) {
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.stats.steps += 1;
            s.stats.llm_ms += llm_ms;
            if let Some(u) = usage {
                s.stats.usage.input_tokens += u.input_tokens;
                s.stats.usage.output_tokens += u.output_tokens;
                s.stats.usage.cache_read_tokens += u.cache_read_tokens;
                s.stats.usage.cache_write_tokens += u.cache_write_tokens;
            }
        }
    }

    /// 会话累计统计：一次工具调用完成。
    fn stats_tool_done(&self, session_id: &str, tool_ms: u64) {
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.stats.tool_ms += tool_ms;
        }
    }

    /// 记录会话产物文件（write_file / edit_file 成功写入的路径，去重）。
    ///
    /// 返回 true 表示新增了一个产物（供前端增量广播）。
    fn record_deliverable(&self, session_id: &str, path: &str) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let mut guard = self.sessions.write();
        let Some(s) = guard.get_mut(session_id) else {
            return false;
        };
        if !s.deliverables.iter().any(|p| p == path) {
            s.deliverables.push(path.to_string());
            true
        } else {
            false
        }
    }

    /// 会话累计统计：记录一次 LLM 调用的首 token 耗时（累计 + 计数，前端求平均）。
    fn stats_first_token(&self, session_id: &str, first_token_ms: u64) {
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.stats.first_token_ms += first_token_ms;
            s.stats.first_token_calls += 1;
        }
    }

    /// 记录最近一次 LLM 请求的真实上下文规模（API usage 上报的输入侧 token）。
    /// 自动压缩以此判定是否接近窗口上限。
    fn stats_set_last_context(&self, session_id: &str, context_tokens: u64) {
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.last_context_tokens = context_tokens;
        }
    }

    /// 会话累计统计快照（turn_done 事件携带给前端 StatsLine）。
    fn stats_snapshot(&self, session_id: &str) -> Option<CodingStats> {
        self.sessions.read().get(session_id).map(|s| s.stats)
    }

    // ========================================================================
    // Agent Loop
    // ========================================================================

    /// 发送用户消息并驱动 agent loop（fire-and-forget，事件实时广播给前端）。
    pub fn send_message(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        session_id: String,
        router: ModelRouter,
        tool_system: Arc<ToolSystem>,
        text: String,
        images: Vec<CodingImage>,
        file_refs: Vec<CodingFileRef>,
        max_rounds: usize,
        interjected: bool,
    ) -> Result<(), String> {
        // 会话存在性 + 状态检查（Running 时拒绝新消息，前端按钮已禁用，这里兜底）
        let working_directory = {
            let mut guard = self.sessions.write();
            let s = guard.get_mut(&session_id).ok_or("会话不存在")?;
            if s.status == CodingStatus::Running {
                return Err("会话正在处理上一条消息".into());
            }
            s.status = CodingStatus::Running;
            let first = s.title.is_empty();
            // 斜杠命令不作为会话标题
            if first && !text.trim_start().starts_with('/') {
                let t: String = text.chars().take(30).collect();
                s.title = t;
            }
            s.working_directory.clone()
        };
        // 文件引用：解析路径并读取内容（沙箱校验 + 数量/长度上限）
        let resolved_refs = resolve_file_refs(&working_directory, file_refs);

        self.push_message(
            &session_id,
            CodingMessage {
                role: CodingRole::User,
                content: text.clone(),
                images: if images.is_empty() { None } else { Some(images.clone()) },
                file_refs: if resolved_refs.is_empty() {
                    None
                } else {
                    Some(resolved_refs)
                },
                interjected: if interjected { Some(true) } else { None },
                tool_name: None,
                tool_arguments: None,
                tool_success: None,
                tool_call_id: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        let _ = app.emit(
            "coding:user_message",
            serde_json::json!({ "session_id": session_id, "content": text, "images": images }),
        );

        // 斜杠命令：拦截分发，不走 agent loop（同步命令即时处理，/compact 异步 LLM 摘要）
        if text.trim_start().starts_with('/') {
            let svc = Arc::clone(self);
            let app_clone = app.clone();
            let cmd_text = text;
            tauri::async_runtime::spawn(async move {
                svc.handle_slash_command(app_clone, &session_id, &router, &cmd_text)
                    .await;
            });
            return Ok(());
        }

        let svc = Arc::clone(self);
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            svc.run_loop(app_clone, &session_id, &router, tool_system, max_rounds)
                .await;
        });
        Ok(())
    }

    // ========================================================================
    // 斜杠命令
    // ========================================================================

    /// 斜杠命令分发：解析命令名与参数，执行对应处理器，结果写入会话并广播。
    async fn handle_slash_command(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
        text: &str,
    ) {
        let trimmed = text.trim();
        let cmd: String = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        let arg = trimmed[cmd.len()..].trim().to_string();

        let result = match cmd.as_str() {
            "/goal" => self.cmd_goal(session_id, &arg),
            "/plan" => self.cmd_plan(session_id, &arg),
            "/compact" => self.cmd_compact(session_id, router).await,
            "/memory" => self.cmd_memory(session_id, router, &arg).await,
            "/permission" => self.cmd_permission(session_id, &arg),
            "/feedback" => self.cmd_feedback(session_id, &arg),
            "/export" => self.cmd_export(session_id, &arg),
            _ => Err(format!(
                "未知命令：{cmd}。可用命令：/goal /plan /compact /memory /permission /feedback /export"
            )),
        };

        match result {
            Ok(msg) => {
                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::Assistant,
                        images: None,
                        content: msg.clone(),
                        file_refs: None,
                        interjected: None,
                        tool_name: None,
                        tool_arguments: None,
                        tool_success: None,
                        tool_call_id: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit(
                    "coding:assistant_message",
                    serde_json::json!({ "session_id": session_id, "content": msg }),
                );
            }
            Err(e) => {
                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::Error,
                        images: None,
                        content: e.clone(),
                        file_refs: None,
                        interjected: None,
                        tool_name: None,
                        tool_arguments: None,
                        tool_success: None,
                        tool_call_id: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit(
                    "coding:error",
                    serde_json::json!({ "session_id": session_id, "message": e }),
                );
            }
        }
        self.finish_turn(app, session_id, CodingStatus::Idle);
    }

    /// /goal：无参查看目标，有参设置目标并注入 system prompt；`/goal 清除` 移除目标。
    fn cmd_goal(&self, session_id: &str, arg: &str) -> Result<String, String> {
        let arg = arg.trim();
        if arg.is_empty() {
            let guard = self.sessions.read();
            let s = guard.get(session_id).ok_or("会话不存在")?;
            return match &s.goal {
                Some(g) => Ok(format!("当前目标：{g}")),
                None => Ok("尚未设置目标。用 /goal <目标> 为长期任务设定目标。".into()),
            };
        }
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            if arg.eq_ignore_ascii_case("清除") || arg.eq_ignore_ascii_case("-clear") {
                s.goal = None;
                drop(guard);
                self.persist();
                return Ok("目标已清除。".into());
            }
            s.goal = Some(arg.to_string());
        } else {
            return Err("会话不存在".into());
        }
        drop(guard);
        self.persist();
        Ok(format!("目标已设置：{arg}"))
    }

    /// /plan：进入/退出计划模式；`/plan approve` 把最近一条方案消息固化为已批准方案。
    fn cmd_plan(&self, session_id: &str, arg: &str) -> Result<String, String> {
        let arg = arg.trim().to_lowercase();
        match arg.as_str() {
            "off" | "-off" => {
                let mut guard = self.sessions.write();
                let s = guard.get_mut(session_id).ok_or("会话不存在")?;
                s.plan_mode = false;
                s.plan = None;
                drop(guard);
                self.persist();
                Ok("计划模式已关闭：可直接执行修改。".into())
            }
            "approve" | "-approve" => {
                let latest_plan = {
                    let guard = self.sessions.read();
                    let s = guard.get(session_id).ok_or("会话不存在")?;
                    s.messages
                        .iter()
                        .rev()
                        .find(|m| m.role == CodingRole::Assistant)
                        .filter(|m| !m.content.trim().is_empty())
                        .map(|m| m.content.clone())
                };
                let Some(plan) = latest_plan else {
                    return Err("还没有待批准的方案消息，请先让智能体输出方案。".into());
                };
                {
                    let mut guard = self.sessions.write();
                    let s = guard.get_mut(session_id).ok_or("会话不存在")?;
                    s.plan = Some(plan.clone());
                    s.plan_mode = true;
                }
                self.persist();
                Ok("方案已批准：将按该方案继续执行。".into())
            }
            _ => {
                let mut guard = self.sessions.write();
                let s = guard.get_mut(session_id).ok_or("会话不存在")?;
                s.plan_mode = !s.plan_mode;
                if !s.plan_mode {
                    s.plan = None;
                }
                let on = s.plan_mode;
                drop(guard);
                self.persist();
                if on {
                    Ok("计划模式已开启：输出方案等待你批准，批准后再动手改文件。".into())
                } else {
                    Ok("计划模式已关闭：可直接执行修改。".into())
                }
            }
        }
    }

    /// /compact：把较早的历史消息交给 LLM 压缩成摘要，替换进上下文。
    async fn cmd_compact(
        &self,
        session_id: &str,
        router: &ModelRouter,
    ) -> Result<String, String> {
        let outcome = self.compact_history(session_id, router).await?;
        if outcome.archived == 0 {
            let total = {
                self.sessions
                    .read()
                    .get(session_id)
                    .map(|s| s.messages.len())
                    .unwrap_or(0)
            };
            return Err(format!(
                "当前会话共 {total} 条消息，不足 {} 条旧消息无需压缩",
                COMPACT_MIN_MESSAGES
            ));
        }
        Ok(format!(
            "已压缩 {} 条历史消息并归档为摘要（保留最近 {} 条）。{}",
            outcome.archived,
            COMPACT_KEEP_MESSAGES,
            outcome.memory_note
        ))
    }

    /// 压缩核心：把较早的历史消息交给 LLM 压缩成摘要写入会话（保留最近
    /// [`COMPACT_KEEP_MESSAGES`] 条），并对被归档消息做项目记忆沉淀。
    /// 旧消息不足 [`COMPACT_MIN_MESSAGES`] 条时返回 `archived = 0`（无需压缩）。
    /// 手动 `/compact` 与上下文占用触发的自动压缩共用本入口。
    async fn compact_history(
        &self,
        session_id: &str,
        router: &ModelRouter,
    ) -> Result<CompactOutcome, String> {
        // 取待压缩的旧消息、既有摘要与工作目录
        let (old, existing, wd) = {
            let guard = self.sessions.read();
            let s = guard.get(session_id).ok_or("会话不存在")?;
            let total = s.messages.len();
            let split = total.saturating_sub(COMPACT_KEEP_MESSAGES);
            if split < COMPACT_MIN_MESSAGES {
                return Ok(CompactOutcome {
                    archived: 0,
                    memory_note: String::new(),
                });
            }
            (
                s.messages[..split].to_vec(),
                s.compacted.clone(),
                s.working_directory.clone(),
            )
        };

        let mut user_prompt = String::new();
        if let Some(prev) = &existing {
            user_prompt.push_str("（此前已压缩的旧摘要，请与新历史合并成一份完整摘要）\n");
            user_prompt.push_str(prev);
            user_prompt.push_str("\n\n");
        }
        user_prompt.push_str(&build_turn_transcript(&old, &wd));

        let summary = router
            .generate(LLMRequest::new(
                "reasoning",
                vec![
                    ChatMessage::system(COMPACT_SYSTEM_PROMPT),
                    ChatMessage::user(&user_prompt),
                ],
            ))
            .await
            .map_err(|e| format!("历史压缩失败：{e}"))?;
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            return Err("历史压缩失败：模型返回空摘要".into());
        }

        {
            let mut guard = self.sessions.write();
            if let Some(s) = guard.get_mut(session_id) {
                s.compacted = Some(summary);
                // 移除已被摘要的旧消息（保留最近 COMPACT_KEEP_MESSAGES 条）
                let split = s.messages.len().saturating_sub(COMPACT_KEEP_MESSAGES);
                s.messages.drain(..split);
            }
        }
        self.persist();

        // 项目记忆沉淀：被归档的旧消息即将脱离上下文，先提炼跨会话教训
        // （尽力而为，LLM/写入失败仅记日志，不影响压缩结果本身）
        let mut memory_note = String::new();
        match self.distill_project_memory(&wd, router, &old).await {
            Ok(Some(_)) => memory_note.push_str("已把可沉淀的教训写入项目记忆。"),
            Ok(None) => {}
            Err(e) => tracing::warn!("[CodingAgent] 压缩时的项目记忆沉淀失败: {e}"),
        }
        Ok(CompactOutcome {
            archived: old.len(),
            memory_note,
        })
    }

    /// 从一段会话历史提炼跨会话教训写入项目记忆
    /// （`/memory 提炼` 与 `/compact` 归档沉淀共用）。
    ///
    /// 文件未超阈值时增量追加；超过 [`PROJECT_MEMORY_MERGE_LINES`] 行时改为
    /// 全文重写合并去重（防追加式无限膨胀）。
    /// 返回 `Ok(Some(消息))` 表示已沉淀（含用户可读结果）；`Ok(None)` 表示无事可沉淀。
    async fn distill_project_memory(
        &self,
        working_directory: &str,
        router: &ModelRouter,
        messages: &[CodingMessage],
    ) -> Result<Option<String>, String> {
        let existing = read_project_memory_raw(working_directory);
        // 超阈值：全文重写合并去重
        if existing.as_deref().map(|m| m.lines().count()).unwrap_or(0) > PROJECT_MEMORY_MERGE_LINES
        {
            return self
                .rewrite_project_memory(working_directory, router, existing.as_deref(), messages)
                .await;
        }

        let mut user_prompt = String::new();
        if let Some(prev) = &existing {
            user_prompt.push_str("（已有项目记忆，提炼时请与已有条目去重）\n");
            user_prompt.push_str(prev);
            user_prompt.push_str("\n\n");
        }
        user_prompt.push_str("（会话历史）\n");
        user_prompt.push_str(&build_turn_transcript(messages, working_directory));

        let distilled = router
            .generate(LLMRequest::new(
                "memory",
                vec![
                    ChatMessage::system(MEMORY_DISTILL_SYSTEM_PROMPT),
                    ChatMessage::user(&user_prompt),
                ],
            ))
            .await
            .map_err(|e| format!("提炼失败：{e}"))?;
        let distilled = distilled.trim().to_string();
        if distilled.is_empty() {
            return Ok(None);
        }
        append_project_memory(working_directory, &distilled)?;
        Ok(Some(format!("已沉淀到项目记忆：\n\n{distilled}")))
    }

    /// 全文重写项目记忆：合并已有条目与会话新知，去重压缩后整文件替换。
    /// LLM 返回空内容视为失败（保留原记忆，不写文件）。
    async fn rewrite_project_memory(
        &self,
        working_directory: &str,
        router: &ModelRouter,
        existing: Option<&str>,
        messages: &[CodingMessage],
    ) -> Result<Option<String>, String> {
        let mut user_prompt = String::new();
        if let Some(prev) = existing {
            user_prompt.push_str("（当前项目记忆全文，过长需要整理）\n");
            user_prompt.push_str(prev);
            user_prompt.push_str("\n\n");
        }
        user_prompt.push_str("（会话历史，可能包含需要沉淀的新知识）\n");
        user_prompt.push_str(&build_turn_transcript(messages, working_directory));

        let rewritten = router
            .generate(LLMRequest::new(
                "memory",
                vec![
                    ChatMessage::system(MEMORY_REWRITE_SYSTEM_PROMPT),
                    ChatMessage::user(&user_prompt),
                ],
            ))
            .await
            .map_err(|e| format!("重写失败：{e}"))?;
        let rewritten = rewritten.trim().to_string();
        if rewritten.is_empty() {
            return Err("重写失败：模型返回空内容（已保留原记忆）".into());
        }
        write_project_memory(working_directory, &rewritten)?;
        Ok(Some(format!(
            "项目记忆已超过 {PROJECT_MEMORY_MERGE_LINES} 行，合并重写为 {} 行。",
            rewritten.lines().count()
        )))
    }

    /// /memory：项目记忆管理（存储在应用数据目录，按工作目录隔离，不污染仓库）。
    /// - 无参：查看当前项目记忆
    /// - `提炼`：LLM 从会话历史提炼教训；文件超阈值时转为全文合并重写
    /// - `清除`：清空项目记忆
    /// - 其他文本：作为一条手动笔记追加
    async fn cmd_memory(
        &self,
        session_id: &str,
        router: &ModelRouter,
        arg: &str,
    ) -> Result<String, String> {
        let arg = arg.trim();
        let wd = {
            let guard = self.sessions.read();
            let s = guard.get(session_id).ok_or("会话不存在")?;
            s.working_directory.clone()
        };

        // 无参：查看（附实际存储路径，便于用户直接编辑文件）
        if arg.is_empty() {
            return match read_project_memory(&wd) {
                Some(m) => Ok(format!(
                    "当前项目记忆（{}）：\n\n{m}",
                    project_memory_path(&wd).display()
                )),
                None => Ok(
                    "尚无项目记忆。用 /memory 提炼 从会话历史沉淀教训，或 /memory <内容> 手动追加；新会话会自动注入上下文。"
                        .into(),
                ),
            };
        }

        // 清除
        if arg.eq_ignore_ascii_case("清除") || arg.eq_ignore_ascii_case("-clear") {
            let path = project_memory_path(&wd);
            if !path.exists() {
                return Ok("尚无项目记忆，无需清除。".into());
            }
            write_project_memory(&wd, "")?;
            return Ok("项目记忆已清空。".into());
        }

        // 提炼：LLM 从会话历史抽取教训（文件超阈值时自动转为全文合并重写）
        if arg.eq_ignore_ascii_case("提炼") || arg.eq_ignore_ascii_case("-distill") {
            let messages = {
                let guard = self.sessions.read();
                let s = guard.get(session_id).ok_or("会话不存在")?;
                s.messages.clone()
            };
            if !messages.iter().any(|m| m.role == CodingRole::User) {
                return Err("会话还没有实质内容，先聊几轮再提炼。".into());
            }
            return match self.distill_project_memory(&wd, router, &messages).await {
                Ok(Some(msg)) => Ok(msg),
                Ok(None) => Ok("会话中没有值得新沉淀的项目知识（或与已有记忆重复）。".into()),
                Err(e) => Err(e),
            };
        }

        // 手动追加
        append_project_memory(&wd, &format!("- {arg}"))?;
        Ok("已追加到项目记忆。".into())
    }

    /// /permission：无参查看当前权限，有参切换权限预设。
    fn cmd_permission(&self, session_id: &str, arg: &str) -> Result<String, String> {
        let arg = arg.trim();
        if arg.is_empty() {
            let guard = self.sessions.read();
            let s = guard.get(session_id).ok_or("会话不存在")?;
            return Ok(format!(
                "当前权限：{}（可用：{}）",
                s.permission,
                CODING_PERMISSIONS.join("/")
            ));
        }
        if !CODING_PERMISSIONS.contains(&arg) {
            return Err(format!(
                "未知权限：{arg}（可选：{}）",
                CODING_PERMISSIONS.join("/")
            ));
        }
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.permission = arg.to_string();
        } else {
            return Err("会话不存在".into());
        }
        drop(guard);
        self.persist();
        Ok(format!("权限已切换为：{arg}"))
    }

    /// /feedback：把反馈追加进会话（含时间戳）。
    fn cmd_feedback(&self, session_id: &str, arg: &str) -> Result<String, String> {
        let arg = arg.trim();
        if arg.is_empty() {
            return Err("请输入反馈内容，例如：/feedback 回复有点啰嗦".into());
        }
        let mut guard = self.sessions.write();
        let s = guard.get_mut(session_id).ok_or("会话不存在")?;
        s.feedback.push(format!(
            "[{}] {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            arg
        ));
        let n = s.feedback.len();
        drop(guard);
        self.persist();
        Ok(format!("反馈已记录（共 {n} 条）"))
    }

    /// /export：把会话导出为 Markdown 文件（用户数据目录 coding_exports/）。
    fn cmd_export(&self, session_id: &str, _arg: &str) -> Result<String, String> {
        let session = self
            .sessions
            .read()
            .get(session_id)
            .cloned()
            .ok_or("会话不存在")?;
        let dir = get_user_data_dir().join("coding_exports");
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败：{e}"))?;

        let safe_title: String = session
            .title
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let base = if safe_title.is_empty() {
            session.session_id.clone()
        } else {
            let suffix = session.session_id.chars().take(8).collect::<String>();
            format!("{safe_title}-{suffix}")
        };
        let path = dir.join(format!("{base}.md"));

        let mut md = String::new();
        md.push_str(&format!(
            "# 会话导出：{}\n\n",
            if session.title.is_empty() {
                session.session_id.as_str()
            } else {
                session.title.as_str()
            }
        ));
        md.push_str(&format!("- 会话 ID：{}\n", session.session_id));
        md.push_str(&format!("- 工作目录：{}\n", session.working_directory));
        md.push_str(&format!("- 模式：{} / 权限：{}\n", session.mode, session.permission));
        if let Some(g) = &session.goal {
            md.push_str(&format!("- 目标：{}\n", g));
        }
        if let Some(p) = &session.plan {
            md.push_str(&format!("- 已批准方案：{}\n", p));
        }
        if session.plan_mode {
            md.push_str("- 计划模式：开启\n");
        }
        if let Some(c) = &session.compacted {
            md.push_str(&format!("- 历史摘要：{}\n", c));
        }
        if !session.feedback.is_empty() {
            md.push_str("\n## 反馈\n\n");
            for f in &session.feedback {
                md.push_str(&format!("- {}\n", f));
            }
        }
        md.push_str(&format!("\n## 消息记录（共 {} 条）\n\n", session.messages.len()));
        for m in &session.messages {
            let ts = chrono::DateTime::from_timestamp(m.timestamp / 1000, 0)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default();
            match m.role {
                CodingRole::User => {
                    md.push_str(&format!("### 用户（{ts}）\n\n{}\n\n", m.content));
                }
                CodingRole::Assistant => {
                    md.push_str(&format!("### 助手（{ts}）\n\n{}\n\n", m.content));
                }
                CodingRole::ToolUse => {
                    let args = serde_json::to_string_pretty(
                        m.tool_arguments.as_ref().unwrap_or(&serde_json::Value::Null),
                    )
                    .unwrap_or_default();
                    md.push_str(&format!(
                        "### 工具调用：{}（{ts}）\n\n```json\n{}\n```\n\n",
                        m.content, args
                    ));
                }
                CodingRole::ToolResult => {
                    let status = if m.tool_success.unwrap_or(false) { "成功" } else { "失败" };
                    md.push_str(&format!(
                        "### 工具结果：{}（{status}）\n\n```\n{}\n```\n\n",
                        m.tool_name.as_deref().unwrap_or("?"),
                        m.content
                    ));
                }
                CodingRole::Error => {
                    md.push_str(&format!("### 错误（{ts}）\n\n{}\n\n", m.content));
                }
            }
        }
        std::fs::write(&path, md).map_err(|e| format!("写入导出文件失败：{e}"))?;
        Ok(format!("会话已导出：{}", path.display()))
    }

    /// 主循环入口：执行 agent loop，结束后把本轮对话摘要写入角色记忆库。
    async fn run_loop(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
        tool_system: Arc<ToolSystem>,
        max_rounds: usize,
    ) {
        self.run_loop_inner(app.clone(), session_id, router, tool_system, max_rounds)
            .await;

        // 本轮结束（含正常/取消/错误所有出口）：异步摘要入库，不阻塞会话
        let svc = Arc::clone(self);
        let app_clone = app.clone();
        let router_clone = router.clone();
        let sid = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            svc.summarize_turn_to_memory(app_clone, &sid, &router_clone).await;
        });
    }

    /// 主循环：按模式分流 —— code 走程序化编排，standard/minimal 走逐轮工具循环。
    async fn run_loop_inner(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
        tool_system: Arc<ToolSystem>,
        max_rounds: usize,
    ) {
        let (char_id, working_directory, mode, permission, reasoning_level) = {
            let guard = self.sessions.read();
            match guard.get(session_id) {
                Some(s) => (
                    s.char_id.clone(),
                    s.working_directory.clone(),
                    s.mode.clone(),
                    s.permission.clone(),
                    s.reasoning_level.clone(),
                ),
                None => return,
            }
        };
        self.stats_turn_started(session_id);

        if mode == "code" {
            self.run_code_mode(
                app,
                session_id,
                router,
                &tool_system,
                &char_id,
                &working_directory,
                &permission,
                &reasoning_level,
            )
            .await;
            return;
        }

        let allowed = tools_for_mode(&mode);
        let definitions = Self::coding_definitions(&tool_system, &allowed);
        let tool_ctx = ToolUseContext {
            char_id: char_id.clone(),
            // session_id 供 send_image 等工具路由回本会话（编程页图片消息推送）
            session_id: session_id.to_string(),
            working_directory: working_directory.clone(),
            access_level: Some(permission_to_access_level(&permission)),
            // 工作智能体标记：场景敏感工具据此取工作侧默认（如 web_search 默认 15 条）
            agent_kind: "work".to_string(),
            ..Default::default()
        };

        // ── 轮次预算与循环保护 ──
        // 预算来自 config.tools.max_coding_rounds（命令层传入，0 = 无限：设置中填 -1）。
        // 无限模式跳过预算检查，循环由 LLM 产出纯文本 / 停滞检测 / 收益递减检测自然终止；
        // 有限模式到达上限自动停止，本轮回有实质进展则自动续轮一次（上限 96）。
        let unlimited = max_rounds == 0;
        let base_rounds = if max_rounds > 0 { max_rounds } else { DEFAULT_MAX_TOOL_ROUNDS };
        let mut budget = if unlimited { usize::MAX } else { base_rounds.max(8) };
        let mut rounds_used = 0usize;
        let mut extended = false;
        let mut made_progress = false;
        // 待注入下一轮请求的系统提示（软预算提醒 / 停滞干预 / 续轮通知）
        let mut pending_hint: Option<String> = None;
        // 死循环检测：相同工具 + 相同参数连续重复（阈值 3）
        let mut doom_tracker = DoomLoopTracker::new(3);
        // 停滞检测：同一工具连续失败且错误摘要相同（阈值 3），有成功即清零
        let mut fail_counts: HashMap<u64, (String, u32)> = HashMap::new();
        // 收益递减检测：连续多轮低产出且无实质进展 → 提前停机收尾，不磨满预算
        let mut output_tracker = crate::brain::budget::OutputBudgetTracker::new();

        loop {
            // 本轮是否有实质进展（本轮内被置 true），收益递减检测用
            let mut round_progress = false;
            // 预算耗尽：有实质进展则自动续轮一次，否则硬停止
            if rounds_used >= budget {
                if extended || !made_progress {
                    break;
                }
                let old_budget = budget;
                budget = (old_budget + base_rounds / 3).min(96);
                extended = true;
                pending_hint = Some(format!(
                    "[系统提示] 已自动续轮 {} 轮（当前上限 {}）。请继续推进任务；若连续多轮无实质进展，请总结当前状态并告知用户。",
                    budget - old_budget,
                    budget
                ));
                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::Error,
                        images: None,
                        content: format!(
                            "已达到单轮最大工具调用轮数（{old_budget}），检测到任务仍在推进，自动续轮 {} 轮。",
                            budget - old_budget
                        ),
                        file_refs: None,
                        interjected: None,
                        tool_name: None,
                        tool_arguments: None,
                        tool_success: None,
                        tool_call_id: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit("coding:error", serde_json::json!({
                    "session_id": session_id,
                    "message": format!("已达到单轮最大工具调用轮数（{old_budget}），自动续轮 {} 轮", budget - old_budget),
                }));
            }
            rounds_used += 1;

            if self.is_canceled(session_id) {
                self.finish_turn(app.clone(), session_id, CodingStatus::Canceled);
                let _ = app.emit("coding:error", serde_json::json!({
                    "session_id": session_id, "message": "已取消",
                }));
                return;
            }

            // 组装 LLM 请求前广播"思考中"，让前端显示占位提示（生成期）——随后流式逐字输出
            let _ = app.emit("coding:thinking", serde_json::json!({
                "session_id": session_id, "thinking": true,
            }));
            let llm_start = std::time::Instant::now();
            // 智能压缩：上轮请求真实上下文接近窗口上限时自动归档早期历史为摘要，
            // 防止多轮工具调用的历史累积超出模型上下文窗口
            let _ = self.maybe_auto_compact(&app, session_id, router).await;
            let mut messages = self.build_llm_messages(session_id, &char_id, &mode);

            // 软预算提醒：用到 2/3 与 5/6 时提醒 LLM 评估收尾（不落库，仅引导本轮决策）
            // 无限模式（budget = usize::MAX）无预算概念，跳过
            if !unlimited {
                let warn_at = (budget * 2 / 3).max(1);
                let hard_warn_at = (budget * 5 / 6).max(1);
                if rounds_used == warn_at || rounds_used == hard_warn_at {
                    messages.push(ChatMessage::system(&format!(
                        "[系统提示] 已使用 {rounds_used}/{budget} 轮工具调用预算。若任务已基本完成，请直接总结收尾；若尚未完成，请评估当前方案是否真正有效，避免无效重复。"
                    )));
                }
            }
            // 停滞干预 / 续轮通知：注入本轮回合，引导 LLM 调整策略
            if let Some(hint) = pending_hint.take() {
                messages.push(ChatMessage::system(&hint));
            }
            let mut req = LLMRequest::new("reasoning", messages).with_tools(definitions.clone());
            // 推理等级：low 关闭思维链，medium/high 按档位开启（按模型能力映射 wire 字段）
            req.reasoning = reasoning_level_to_pref(&reasoning_level);
            let mut event_rx = match router.generate_stream_with_tools(req).await {
                Ok(rx) => rx,
                Err(e) => {
                    self.report_llm_error(&app, session_id, "LLM 调用", &e.to_string());
                    self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                    return;
                }
            };

            // 消费流，累积文本增量（逐字转发前端打字机）+ 工具调用增量
            let mut streamed_text = String::new();
            let mut call_buf: BTreeMap<usize, (String, String, String)> = BTreeMap::new(); // index -> (id, name, args)
            let mut step_usage: Option<CodingTokenUsage> = None;
            let mut first_token_tracked = false;
            while let Some(event) = event_rx.recv().await {
                match event {
                    StreamEvent::Text { content } => {
                        if !content.is_empty() {
                            if !first_token_tracked {
                                first_token_tracked = true;
                                self.stats_first_token(
                                    session_id,
                                    llm_start.elapsed().as_millis() as u64,
                                );
                            }
                            streamed_text.push_str(&content);
                            let _ = app.emit("coding:chunk", serde_json::json!({
                                "session_id": session_id, "content": content,
                            }));
                        }
                    }
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                    } => {
                        step_usage = Some(CodingTokenUsage {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_write_tokens,
                        });
                    }
                    StreamEvent::Thinking { content } => {
                        // 推理链增量：转发前端在"思考占位"内渐进展开灰色文本（不入库）
                        if !content.is_empty() {
                            if !first_token_tracked {
                                first_token_tracked = true;
                                self.stats_first_token(
                                    session_id,
                                    llm_start.elapsed().as_millis() as u64,
                                );
                            }
                            let _ = app.emit("coding:thinking_chunk", serde_json::json!({
                                "session_id": session_id, "content": content,
                            }));
                        }
                    }
                    StreamEvent::ToolCallDelta { index, id, name, arguments_delta } => {
                        let e = call_buf.entry(index).or_default();
                        if let Some(id) = id {
                            e.0 = id;
                        }
                        if let Some(name) = name {
                            e.1 = name;
                        }
                        if let Some(a) = arguments_delta {
                            e.2.push_str(&a);
                        }
                    }
                    StreamEvent::Done { .. } => break,
                    StreamEvent::Error { message } => {
                        self.report_llm_error(&app, session_id, "LLM 流式响应", &message);
                        self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                        return;
                    }
                }
            }

            // 本步 LLM 调用结束：累计耗时与 token 用量（均为 API 上报的真实值）；
            // 同时记录本次请求的真实上下文规模（输入侧 token），供下轮自动压缩判定
            if let Some(u) = &step_usage {
                self.stats_set_last_context(
                    session_id,
                    u.input_tokens + u.cache_read_tokens + u.cache_write_tokens,
                );
            }
            self.stats_step_done(
                session_id,
                llm_start.elapsed().as_millis() as u64,
                step_usage,
            );

            // 解析工具调用（参数 JSON 字符串 → Value）
            let calls: Vec<MessageToolCall> = call_buf
                .into_iter()
                .map(|(_idx, (id, name, args))| {
                    let arguments = if args.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_str(args.trim())
                            .unwrap_or_else(|_| serde_json::Value::String(args))
                    };
                    MessageToolCall { id, name, arguments }
                })
                .collect();

            // 无工具调用：assistant 文本回复即轮次结束（流式已逐步推送，此处落库 + 通知前端定型）
            if calls.is_empty() {
                let content = streamed_text.trim().to_string();
                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::Assistant,
                        images: None,
                        content: content.clone(),
                        file_refs: None,
                        interjected: None,
                        tool_name: None,
                        tool_arguments: None,
                        tool_success: None,
                        tool_call_id: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit(
                    "coding:assistant_message",
                    serde_json::json!({ "session_id": session_id, "content": content }),
                );
                self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                return;
            }

            // 有工具调用：记录 assistant 工具调用意图（工具调用前若已有零星文本，不作为最终回复）
            self.record_assistant_tool_calls(session_id, &calls);
            for c in &calls {
                let _ = app.emit(
                    "coding:tool_call",
                    serde_json::json!({
                        "session_id": session_id,
                        "id": c.id,
                        "name": c.name,
                        "arguments": c.arguments,
                    }),
                );
            }

            // 逐个执行工具并回填结果（顺序模式）
            for call in &calls {
                if self.is_canceled(session_id) {
                    self.finish_turn(app.clone(), session_id, CodingStatus::Canceled);
                    let _ = app.emit("coding:error", serde_json::json!({
                        "session_id": session_id, "message": "已取消",
                    }));
                    return;
                }
                let tool_start = std::time::Instant::now();
                let result =
                    execute_tool_use(&call.name, call.arguments.clone(), &tool_system, &tool_ctx, coding_sandbox_allow())
                        .await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                self.stats_tool_done(session_id, duration_ms);
                let (ok, summary) = if result.success {
                    let data = serde_json::to_string(
                        result.data.as_ref().unwrap_or(&serde_json::Value::Null),
                    )
                    .unwrap_or_default();
                    (true, summarize_result(&data))
                } else {
                    (false, result.error.clone().unwrap_or_else(|| "执行失败".into()))
                };

                // 停滞检测：
                // - 相同工具 + 相同参数连续重复（死循环）→ 提醒 LLM 换策略
                // - 同一工具连续失败且错误摘要相同 → 提醒 LLM 重新分析根因
                if let LoopStatus::Doomed { tool, count } =
                    doom_tracker.record(&call.name, &call.arguments)
                {
                    pending_hint = Some(format!(
                        "[系统提示] 你已连续 {count} 次调用 `{tool}` 且参数相同，未取得进展。请停止重复，重新分析问题根源并更换方法，或向用户说明当前障碍。"
                    ));
                }
                if ok {
                    // 任何成功都是进展：清空失败停滞计数；写/改/执行类工具成功记为实质进展（用于自动续轮判定）
                    fail_counts.clear();
                    if matches!(call.name.as_str(), "write_file" | "edit_file" | "run_command") {
                        made_progress = true;
                        round_progress = true;
                    }
                    // 写/改文件成功 → 记录为会话产物（去重），增量广播给前端产物面板
                    if matches!(call.name.as_str(), "write_file" | "edit_file") {
                        if let Some(p) = call
                            .arguments
                            .get("path")
                            .and_then(|v| v.as_str())
                            .filter(|p| !p.trim().is_empty())
                        {
                            if self.record_deliverable(session_id, p) {
                                let _ = app.emit(
                                    "coding:deliverable",
                                    serde_json::json!({
                                        "session_id": session_id,
                                        "path": p,
                                    }),
                                );
                            }
                        }
                    }
                } else {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(&(call.name.clone(), summary.clone()), &mut hasher);
                    let key = hasher.finish();
                    let e = fail_counts.entry(key).or_insert_with(|| (call.name.clone(), 0));
                    e.1 += 1;
                    if e.1 >= 3 {
                        let tool = e.0.clone();
                        let n = e.1;
                        fail_counts.remove(&key); // 只提示一次
                        pending_hint = Some(format!(
                            "[系统提示] `{tool}` 已连续失败 {n} 次且错误相同，继续重试不会取得进展。请停止当前重复尝试，重新分析根因、更换方案，或向用户说明障碍。"
                        ));
                    }
                }

                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::ToolResult,
                        images: None,
                        content: summary.clone(),
                        file_refs: None,
                        interjected: None,
                        tool_name: Some(call.name.clone()),
                        tool_arguments: Some(call.arguments.clone()),
                        tool_success: Some(ok),
                        tool_call_id: Some(call.id.clone()),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit(
                    "coding:tool_result",
                    serde_json::json!({
                        "session_id": session_id,
                        "id": call.id,
                        "name": call.name,
                        "success": ok,
                        "result": summary,
                        "duration_ms": duration_ms,
                    }),
                );
            }

            // 收益递减检测：连续多轮低产出且无实质进展 → 提前停机，不磨满轮数预算
            // 产出量信号：有 usage 上报按输出 token 判定；无上报按产出字符数判定
            let verdict = match &step_usage {
                Some(u) => output_tracker.record(u.output_tokens, round_progress),
                None => {
                    let args_chars: usize = calls.iter().map(|c| c.arguments.to_string().len()).sum();
                    output_tracker.record_chars(
                        streamed_text.len() + args_chars,
                        round_progress,
                    )
                }
            };
            if let crate::brain::budget::BudgetVerdict::StopDiminishing { low_rounds } = verdict {
                self.push_message(
                    session_id,
                    CodingMessage {
                        role: CodingRole::Error,
                        images: None,
                        content: format!(
                            "连续 {low_rounds} 轮无实质产出（收益递减），已提前停止以节省配额。可调整方案或重新描述目标后继续。"
                        ),
                        file_refs: None,
                        interjected: None,
                        tool_name: None,
                        tool_arguments: None,
                        tool_success: None,
                        tool_call_id: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    },
                );
                let _ = app.emit("coding:error", serde_json::json!({
                    "session_id": session_id,
                    "message": format!("连续 {low_rounds} 轮无实质产出，已提前停止（收益递减保护）"),
                }));
                self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                return;
            }
        }

        // 达到轮数上限（含续轮后仍耗尽）：通知用户收尾，等待下一条消息
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::Error,
                images: None,
                content: format!("已达到单轮最大工具调用轮数（{budget}），自动停止。可发送新消息继续。"),
                file_refs: None,
                interjected: None,
                tool_name: None,
                tool_arguments: None,
                tool_success: None,
                tool_call_id: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        let _ = app.emit("coding:error", serde_json::json!({
            "session_id": session_id,
            "message": format!("已达到单轮最大工具调用轮数（{budget}）"),
        }));
        self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
    }

    /// 轮次收尾：恢复状态 + 持久化 + 通知前端。
    fn finish_turn(&self, app: tauri::AppHandle, session_id: &str, status: CodingStatus) {
        {
            let mut guard = self.sessions.write();
            if let Some(s) = guard.get_mut(session_id) {
                // Canceled 保留标记（前端可感知），下次发消息时重置为 Running
                s.status = status;
            }
        }
        self.persist();
        let stats = self.stats_snapshot(session_id);
        let _ = app.emit(
            "coding:turn_done",
            serde_json::json!({ "session_id": session_id, "stats": stats }),
        );
    }

    /// 记录 assistant 工具调用意图为历史消息（含 tool_calls 结构，回传 LLM 保持关联）。
    fn record_assistant_tool_calls(&self, session_id: &str, calls: &[MessageToolCall]) {
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::ToolUse,
                images: None,
                content: format!(
                    "调用工具：{}",
                    calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join("、")
                ),
                file_refs: None,
                interjected: None,
                tool_name: None,
                tool_arguments: None,
                tool_success: None,
                tool_call_id: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        // 原始结构化调用存入消息的附加槽：复用 tool_arguments 存整个数组
        {
            let mut guard = self.sessions.write();
            if let Some(s) = guard.get_mut(session_id) {
                if let Some(last) = s.messages.last_mut() {
                    last.tool_arguments = Some(serde_json::to_value(calls).unwrap_or_default());
                }
            }
        }
    }

    /// Code 模式循环：LLM 一次性输出多步程序 JSON → 宿主顺序执行 → 总结。
    /// 执行期间不回询 LLM（步骤失败即中止剩余），以"用一个程序组合多步操作"的方式执行。
    #[allow(clippy::too_many_arguments)]
    async fn run_code_mode(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
        tool_system: &Arc<ToolSystem>,
        char_id: &str,
        working_directory: &str,
        permission: &str,
        reasoning_level: &str,
    ) {
        let tool_ctx = ToolUseContext {
            char_id: char_id.to_string(),
            // session_id 供 send_image 等工具路由回本会话（编程页图片消息推送）
            session_id: session_id.to_string(),
            working_directory: working_directory.to_string(),
            access_level: Some(permission_to_access_level(permission)),
            ..Default::default()
        };
        let fail = |msg: &str| {
            let _ = app.emit("coding:error", serde_json::json!({
                "session_id": session_id, "message": msg,
            }));
        };

        // 1. 生成程序（不带工具调用，纯文本 JSON 输出）；先广播"思考中"
        let _ = app.emit("coding:thinking", serde_json::json!({
            "session_id": session_id, "thinking": true,
        }));
        // 智能压缩：以上一请求的真实上下文为依据（code 模式走非流式 generate，
        // 无 usage 上报，跨轮次沿用历史记录值）
        let _ = self.maybe_auto_compact(&app, session_id, router).await;
        let messages = self.build_llm_messages(session_id, char_id, "code");
        let mut req = LLMRequest::new("reasoning", messages);
        // 推理等级：low 关闭思维链，medium/high 按档位开启（按模型能力映射 wire 字段）
        req.reasoning = reasoning_level_to_pref(&reasoning_level);
        let llm_start = std::time::Instant::now();
        let resp = match router.generate(req).await {
            Ok(t) => t,
            Err(e) => {
                self.report_llm_error(&app, session_id, "LLM 调用", &e.to_string());
                self.finish_turn(app, session_id, CodingStatus::Idle);
                return;
            }
        };
        self.stats_step_done(session_id, llm_start.elapsed().as_millis() as u64, None);

        // 2. 解析程序：{"steps":[{"tool","arguments"}...], "summary":"..."}
        let parsed = crate::brain::json_parser::JsonParser::parse_single(&resp);
        let parsed = match parsed {
            Ok(p) => p,
            Err(e) => {
                let m = format!("程序解析失败：{e}（模型未输出合法 JSON）");
                self.push_error(session_id, &m);
                fail(&m);
                self.finish_turn(app, session_id, CodingStatus::Idle);
                return;
            }
        };
        let summary = parsed
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let steps_raw = parsed.get("steps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if steps_raw.is_empty() {
            let m = "程序没有步骤（steps 为空）";
            self.push_error(session_id, m);
            fail(m);
            self.finish_turn(app, session_id, CodingStatus::Idle);
            return;
        }

        // 3. 记录程序卡片（tool_use 消息 + 事件，前端渲染为"组合程序"卡片）
        let program_json = serde_json::json!({
            "steps": steps_raw.iter().map(|s| json_step(s)).collect::<Vec<_>>(),
            "summary": summary,
        });
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::ToolUse,
                images: None,
                content: format!("编排程序：{} 步", steps_raw.len().min(CODE_MODE_MAX_STEPS)),
                file_refs: None,
                interjected: None,
                tool_name: Some("compose_program".into()),
                tool_arguments: Some(program_json.clone()),
                tool_success: None,
                tool_call_id: Some("program".into()),
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        let _ = app.emit(
            "coding:tool_call",
            serde_json::json!({
                "session_id": session_id,
                "id": "program",
                "name": "compose_program",
                "arguments": program_json,
            }),
        );

        // 4. 顺序执行步骤（取消检查 + 失败中止）
        let mut executed = 0usize;
        let mut aborted = false;
        for (i, step) in steps_raw.iter().take(CODE_MODE_MAX_STEPS).enumerate() {
            if self.is_canceled(session_id) {
                self.finish_turn(app.clone(), session_id, CodingStatus::Canceled);
                fail("已取消");
                return;
            }
            let tool = step.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let arguments = step.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
            let call_id = format!("step-{i}");

            let _ = app.emit(
                "coding:tool_call",
                serde_json::json!({
                    "session_id": session_id, "id": call_id, "name": tool, "arguments": arguments,
                }),
            );
            let tool_start = std::time::Instant::now();
            let result = if tool_system.has_tool(&tool) {
                execute_tool_use(&tool, arguments.clone(), tool_system, &tool_ctx, coding_sandbox_allow()).await
            } else {
                tracing::warn!("[CodingAgent] code 模式步骤引用未知工具: {tool}");
                crate::tools::types::ToolResult::standard_error(
                    &format!("工具 {tool} 不存在"),
                    None,
                    None,
                )
            };
            let duration_ms = tool_start.elapsed().as_millis() as u64;
            self.stats_tool_done(session_id, duration_ms);
            let (ok, text) = if result.success {
                let data = serde_json::to_string(
                    result.data.as_ref().unwrap_or(&serde_json::Value::Null),
                )
                .unwrap_or_default();
                (true, summarize_result(&data))
            } else {
                (false, result.error.clone().unwrap_or_else(|| "执行失败".into()))
            };
            executed += 1;
            self.push_message(
                session_id,
                CodingMessage {
                    role: CodingRole::ToolResult,
                    images: None,
                    content: text.clone(),
                    file_refs: None,
                    interjected: None,
                    tool_name: Some(tool.clone()),
                    tool_arguments: Some(arguments.clone()),
                    tool_success: Some(ok),
                    tool_call_id: Some(call_id.clone()),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            );
            let _ = app.emit(
                "coding:tool_result",
                serde_json::json!({
                    "session_id": session_id, "id": call_id, "name": tool, "success": ok, "result": text,
                    "duration_ms": duration_ms,
                }),
            );
            if !ok {
                let m = format!("步骤 {i}（{tool}）失败，已中止剩余步骤");
                self.push_error(session_id, &m);
                fail(&m);
                aborted = true;
                break;
            }
        }

        // 5. 总结（失败中止时若无 summary 则跳过，错误消息已说明）
        let final_text = if aborted && summary.is_empty() {
            String::new()
        } else {
            summary
        };
        if !final_text.is_empty() {
            self.push_message(
                session_id,
                CodingMessage {
                    role: CodingRole::Assistant,
                    images: None,
                    content: final_text.clone(),
                    file_refs: None,
                    interjected: None,
                    tool_name: None,
                    tool_arguments: None,
                    tool_success: None,
                    tool_call_id: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            );
            let _ = app.emit(
                "coding:assistant_message",
                serde_json::json!({ "session_id": session_id, "content": final_text }),
            );
        } else if !aborted {
            let fallback = format!("程序执行完成：共 {executed} 步。");
            self.push_message(
                session_id,
                CodingMessage {
                    role: CodingRole::Assistant,
                    images: None,
                    content: fallback.clone(),
                    file_refs: None,
                    interjected: None,
                    tool_name: None,
                    tool_arguments: None,
                    tool_success: None,
                    tool_call_id: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            );
            let _ = app.emit(
                "coding:assistant_message",
                serde_json::json!({ "session_id": session_id, "content": fallback }),
            );
        }
        self.finish_turn(app, session_id, CodingStatus::Idle);
    }

    /// 把本轮编程对话（用户消息 → 工具调用 → 助手回复）摘要写入会话所属角色的记忆库。
    ///
    /// LLM 摘要走 memory 路由；失败时退化为规则摘要，保证内容不因 LLM 故障丢失。
    async fn summarize_turn_to_memory(
        &self,
        app: tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
    ) {
        let Some(session) = self.get_session(session_id) else { return };
        // 本轮切片：从最后一条用户消息起（send_message 每轮恰好 push 一条 User）
        let turn_start = session
            .messages
            .iter()
            .rposition(|m| m.role == CodingRole::User)
            .unwrap_or(0);
        let slice = &session.messages[turn_start..];
        if slice.is_empty() {
            return;
        }

        let summary = match router
            .generate(LLMRequest::new(
                "memory",
                vec![
                    ChatMessage::system(TURN_SUMMARY_SYSTEM_PROMPT),
                    ChatMessage::user(&build_turn_transcript(slice, &session.working_directory)),
                ],
            ))
            .await
        {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => rule_turn_digest(slice),
        };

        // 写入会话所属角色的记忆库（多角色数据隔离）
        let Some(state) = app.try_state::<Arc<crate::state::AppState>>() else {
            tracing::warn!("[CodingAgent] 会话摘要入库跳过：AppState 不可用");
            return;
        };
        let memory = match state.get_character(Some(&session.char_id)) {
            Ok(c) => c.brain.memory.clone(),
            Err(e) => {
                tracing::warn!("[CodingAgent] 会话摘要入库跳过：{e}");
                return;
            }
        };

        let metadata = serde_json::json!({
            "source": "coding_session",
            "session_id": session.session_id,
            "working_directory": session.working_directory,
            "speaker": "user",
            "listener": session.char_id,
        });
        let content = format!("[编程会话] {summary}");
        if let Err(e) = memory
            .add_memory_with_metadata(
                &content,
                crate::memory::MemoryType::ShortTerm,
                0.4,
                vec!["coding_session".to_string(), "work".to_string()],
                metadata,
            )
            .await
        {
            tracing::warn!("[CodingAgent] 会话摘要写入记忆失败: {e}");
        }
    }

    /// 追加错误消息到会话历史。
    fn push_error(&self, session_id: &str, message: &str) {
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::Error,
                images: None,
                content: message.to_string(),
                file_refs: None,
                interjected: None,
                tool_name: None,
                tool_arguments: None,
                tool_success: None,
                tool_call_id: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    /// 记录并广播一条分类后的 LLM 错误：友好提示进会话历史与前端事件，
    /// 原始错误只进日志。事件载荷带 error_type / error_kind 供前端细分处理。
    fn report_llm_error(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        stage: &str,
        raw: &str,
    ) {
        let class = classify_llm_failure(raw);
        tracing::warn!("[CodingAgent] {stage}失败（error_type={}）：{raw}", class.error_type);
        self.push_error(session_id, &class.user_message);
        let _ = app.emit(
            "coding:error",
            serde_json::json!({
                "session_id": session_id,
                "message": class.user_message,
                "error_type": class.error_type,
                "error_kind": class.kind,
            }),
        );
    }

    /// 智能压缩入口：以上一次 LLM 请求 API 上报的真实输入侧 token
    /// （`last_context_tokens`）为依据，达到会话窗口上限的
    /// [`AUTO_COMPACT_THRESHOLD_PCT`]% 时把早期历史归档为摘要，
    /// 防止多轮工具调用的历史累积超出模型上下文窗口。
    /// 压缩失败仅记日志，不影响本轮执行。返回是否实际执行了压缩。
    async fn maybe_auto_compact(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
    ) -> bool {
        // 消息量不足时必然无法压缩，直接跳过
        let (enough, last_ctx, window) = {
            let guard = self.sessions.read();
            guard
                .get(session_id)
                .map(|s| {
                    (
                        s.messages.len() > COMPACT_KEEP_MESSAGES + COMPACT_MIN_MESSAGES,
                        s.last_context_tokens,
                        s.context_window,
                    )
                })
                .unwrap_or((false, 0, 0))
        };
        if !enough || last_ctx == 0 || window == 0 {
            return false;
        }
        if last_ctx * 100 < window * AUTO_COMPACT_THRESHOLD_PCT {
            return false;
        }
        match self.compact_history(session_id, router).await {
            Ok(outcome) if outcome.archived > 0 => {
                let pct = last_ctx * 100 / window;
                let notice = format!(
                    "上下文占用已达窗口的 {pct}%（上轮请求 {last_ctx} tokens），已自动把 {} 条早期历史压缩为摘要。",
                    outcome.archived
                );
                self.push_error(session_id, &notice);
                let _ = app.emit(
                    "coding:error",
                    serde_json::json!({
                        "session_id": session_id, "message": notice,
                    }),
                );
                true
            }
            Ok(_) => false,
            Err(e) => {
                tracing::warn!("[CodingAgent] 上下文自动压缩失败: {e}");
                false
            }
        }
    }

    /// 设置会话上下文窗口（切换工作模型 / 新建会话时由命令层解析后传入）。
    pub fn set_context_window(&self, session_id: &str, window: u64) {
        if window == 0 {
            return;
        }
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.context_window = window;
        }
    }

    /// 组装 LLM 消息序列：system + （裁剪后的）会话历史。
    fn build_llm_messages(&self, session_id: &str, char_id: &str, mode: &str) -> Vec<ChatMessage> {
        let session = {
            let guard = self.sessions.read();
            match guard.get(session_id) {
                Some(s) => s.clone(),
                None => return Vec::new(),
            }
        };

        let mut system = Self::system_prompt(char_id, &session.working_directory, mode);
        // 会话级状态注入：目标 / 已批准方案 / 计划模式策略 / 已压缩的历史摘要
        if let Some(g) = &session.goal {
            system.push_str(&format!("\n\n# 当前目标\n{g}"));
        }
        if let Some(p) = &session.plan {
            system.push_str(&format!("\n\n# 已批准方案（请严格按此执行）\n{p}"));
        }
        if session.plan_mode {
            system.push_str(PLAN_MODE_POLICY);
        }
        if let Some(c) = &session.compacted {
            system.push_str(&format!("\n\n# 历史摘要（较早对话已压缩归档）\n{c}"));
        }
        // 项目记忆注入：跨会话约定与教训（应用数据目录按工作目录隔离存储，每轮重读，
        // /memory 修改后下一轮即时生效；文件未变时 system prompt 字节一致，不影响缓存）
        if let Some(mem) = read_project_memory(&session.working_directory) {
            system.push_str(&format!(
                "\n\n# 项目记忆（跨会话沉淀）\n\
                 以下是此前会话沉淀的本项目约定与经验教训，默认遵循其中约定（用户当轮指示优先）：\n{mem}"
            ));
        }
        let mut messages = vec![ChatMessage::system(system)];
        // 历史裁剪：保留最近 MAX_HISTORY_MESSAGES 条
        let start = session.messages.len().saturating_sub(MAX_HISTORY_MESSAGES);
        for msg in &session.messages[start..] {
            match msg.role {
                CodingRole::User => {
                    // @-mention 文件引用：把引用的文件内容追加到用户消息（含读取失败提示）
                    let mut user_text = if let Some(refs) = &msg.file_refs {
                        if refs.is_empty() {
                            msg.content.clone()
                        } else {
                            let mut text = msg.content.clone();
                            text.push_str("\n\n<file_refs>");
                            for r in refs {
                                text.push_str(&format!("\n[file: {}]\n", r.path));
                                if let Some(c) = &r.content {
                                    text.push_str(c);
                                    text.push('\n');
                                } else if let Some(e) = &r.error {
                                    text.push_str(&format!("(读取失败：{e})\n"));
                                }
                            }
                            text.push_str("</file_refs>");
                            text
                        }
                    } else {
                        msg.content.clone()
                    };
                    // 任务执行期间的排队插话：加标注帮助模型区分补充指令与全新对话
                    if msg.interjected == Some(true) {
                        user_text = format!(
                            "[系统标注] 用户在你处理上一条消息期间发来了消息，请结合当前任务上下文判断这是对任务的补充/修正还是新指令：\n<user_message>\n{}\n</user_message>",
                            user_text
                        );
                    }
                    // 带图消息转多模态（provider 层翻译为 image_url / image block）
                    match &msg.images {
                        Some(imgs) if !imgs.is_empty() => {
                            let mi: Vec<crate::types::response::MessageImage> = imgs
                                .iter()
                                .map(|i| crate::types::response::MessageImage {
                                    media_type: i.media_type.clone(),
                                    data: i.data.clone(),
                                    url: None,
                                    detail: None,
                                })
                                .collect();
                            messages.push(ChatMessage::user_with_images(&user_text, mi));
                        }
                        _ => messages.push(ChatMessage::user(&user_text)),
                    }
                }
                // 智能体图片消息：content 可能为空（仅图片），给 LLM 上下文加占位说明
                CodingRole::Assistant => {
                    let text = if msg.content.trim().is_empty() && msg.images.as_ref().is_some_and(|v| !v.is_empty()) {
                        "[已向用户发送图片]"
                    } else {
                        msg.content.as_str()
                    };
                    messages.push(ChatMessage::assistant(text));
                }
                CodingRole::ToolUse => {
                    // 结构化工具调用：还原为带 tool_calls 的 assistant 消息
                    let calls: Vec<MessageToolCall> = msg
                        .tool_arguments
                        .as_ref()
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    if calls.is_empty() {
                        messages.push(ChatMessage::assistant(&msg.content));
                    } else {
                        messages.push(ChatMessage::assistant_with_tool_calls(
                            msg.content.clone(),
                            calls,
                        ));
                    }
                }
                CodingRole::ToolResult => {
                    let truncated = prune_tool_result(&msg.content, TOOL_RESULT_MAX_CHARS);
                    messages.push(ChatMessage::tool_result(
                        truncated,
                        msg.tool_call_id.clone().unwrap_or_default(),
                    ));
                }
                CodingRole::Error => {
                    // 错误信息作为 system 提示回传，让 LLM 感知失败并调整策略
                    messages.push(ChatMessage::system(&format!("[系统提示] {}", msg.content)));
                }
            }
        }
        messages
    }

    /// 编程智能体 system prompt（按模式差异化）。
    fn system_prompt(char_id: &str, working_directory: &str, mode: &str) -> String {
        let persona = match char_id {
            "nana" => "你是 Nana，一位温柔的编程助手。语气轻柔友好，但技术内容严谨准确。",
            _ => "你是 Vivian，一位反应快、爱吐槽但极其靠谱的编程助手。语气自然随意，但代码和结论必须严谨。",
        };
        // 无工作区模式：会话未绑定目录（如陪伴侧派发的轻量任务）。
        // 此时文件工具用绝对路径操作，不设工作目录沙箱；写入/执行仍受
        // 权限矩阵管控（workspace_write 级下命令与文件写入会请求用户确认）。
        let env = if working_directory.trim().is_empty() {
            "# 工作环境\n\
             - 操作系统：Windows（命令用 PowerShell 语法）\n\
             - 工作目录：未选择（无工作区模式）\n\
             - 文件操作使用绝对路径；未绑定工作区，无目录沙箱，写入前会请求用户确认\n\
             - 需要固定工作区时，可请用户在编程页为会话选择工作区"
                .to_string()
        } else {
            format!(
                "# 工作环境\n\
                 - 操作系统：Windows（命令用 PowerShell 语法）\n\
                 - 工作目录：{wd}\n\
                 - 所有文件路径操作仅限工作目录内（沙箱强制）",
                wd = working_directory,
            )
        };
        let rules = "\n# 回复要求\n\
             - 用与用户相同的语言回复。\n\
             - 涉及文件路径、命令、代码时用等宽格式清晰呈现。\n\
             - 出错时说明原因和你打算怎么修，不要沉默重试。\n\
             - 工具不可用或路径被沙箱拒绝时，向用户说明而不是编造成功。";
        match mode {
            "minimal" => format!(
                "{persona}\n\n# 角色\n你是运行在用户桌面上的极简编程智能体（minimal 模式）：只有两个工具——run_command（PowerShell）与 edit_file（精确字符串替换编辑）。\n读取文件用 `Get-Content -Raw <path>`，搜索用 `Select-String -Pattern <p> -Recurse`（或 grep 可用的等价命令），列目录用 `Get-ChildItem`。\n局部修改用 edit_file（old_string 必须与文件内容完全一致，含缩进）；修改后用 run_command 运行验证。\n\n{env}{rules}"
            ),
            "code" => format!(
                "{persona}\n\n# 角色\n你是运行在用户桌面上的编程智能体，当前处于**编排模式（Code Mode）**：你要把整个任务一次性规划为一个多步程序，由宿主顺序执行，执行期间不再回询你。\n\n{env}\n\n# 输出格式（必须只输出一个 JSON，不要输出其他文字）\n```\n{{\"steps\":[{{\"tool\":\"工具名\",\"arguments\":{{...}}}}, ...], \"summary\":\"执行完成后给用户的中文总结（说明做了什么、结果如何）\"}}\n```\n\n可用工具：read_file / write_file / edit_file / run_command / grep_search / list_dir（参数与各工具 schema 一致）。\n\n# 编写程序的规则\n1. 先放探索步骤（list_dir / grep_search / read_file），再放修改步骤（edit_file / write_file），最后放验证步骤（run_command）。\n2. edit_file 的 old_string 必须与文件内容完全一致（含缩进）。因为你无法看到中间结果，请用足够长的上下文锚定；不确定时先加 read_file 步骤。\n3. 步骤间不能依赖上一步的动态输出值（结果你拿不到）；需要根据结果决策时，结束本次程序并在 summary 中说明，让用户发下一条消息继续。\n4. 最多 {max} 步。任一步骤失败会中止剩余步骤。\n5. summary 用与用户相同的语言。{rules}",
                max = CODE_MODE_MAX_STEPS,
            ),
            _ => format!(
                "{persona}\n\n# 角色\n\
                 你是一个运行在用户桌面上的编程智能体（coding agent），帮助用户阅读、修改、构建和调试代码。\n\
                 你也是**能力进化事件的执行主体**：把可复用的做法沉淀为技能（create_skill）、\
                 在缺少可执行能力时构建新工具（create_tool，需经用户预览卡片授权）。\n\n\
                 {env}\n\n\
                 # 工作方式\n\
                 1. 先看（list_dir / grep_search / read_file）再动手，不要凭猜测改代码。\n\
                 2. 局部修改用 edit_file（old_string 必须与文件内容完全一致，含缩进）；新建文件或整体重写用 write_file。\n\
                 3. 修改后尽量运行验证（run_command：cargo check / npm run build / 测试命令等），用结果确认改动有效。\n\
                 4. 每轮可以连续多次调用工具；不需要再调用工具时，直接用自然语言总结你做了什么、结果如何。\n\
                 5. 到达阶段性节点（某阶段完成 / 验证通过 / 重要发现）时，用 notify_companion 把成果发给你的陪伴人格，由 TA 以角色口吻向用户播报——每节点一次，不必每小步都报。\n\
                 6. 用户需要看到图片（生成的图表、截图、项目里的图片文件）时，用 send_image 把本地图片文件发到对话里，路径必须真实存在。\n\
                 7. 任务中总结出值得复用的流程性做法时，用 create_skill 沉淀为命名技能；确认缺少可执行原语时，用 create_tool 构建新工具（stdin 收 JSON 参数、stdout 出结果），创建会弹出预览卡片供用户审核。{rules}"
            ),
        }
    }

    /// 从 ToolSystem 过滤出编程工具定义（registry → provider 结构转换，按模式白名单）。
    ///
    /// 按 allowed 白名单顺序输出而非 HashMap 迭代序：工具定义序列每轮逐字节一致，
    /// 保住 API tools 参数的前缀缓存（byte-identical prefix 要求）。
    fn coding_definitions(tool_system: &ToolSystem, allowed: &[&str]) -> Vec<ToolDefinition> {
        let schemas: HashMap<String, ToolDefinition> = tool_system
            .get_tool_schemas()
            .into_iter()
            .map(|d| (d.name.clone(), ToolDefinition {
                name: d.name,
                description: d.description,
                parameters: d.input_schema,
            }))
            .collect();
        allowed
            .iter()
            .filter_map(|name| schemas.get(*name).cloned())
            .collect()
    }
}

/// 工具结果摘要（写入历史 + 广播给前端，控制体积）。
fn summarize_result(data_json: &str) -> String {
    prune_tool_result(data_json, TOOL_RESULT_MAX_CHARS)
}

/// LLM 错误分类结果：分类枚举 + 类型标识 + 面向用户的友好提示。
struct ClassifiedLlmError {
    kind: LlmErrorKind,
    error_type: &'static str,
    user_message: String,
}

/// 把 LLM 原始错误归类为错误类型与用户安全提示。
/// 原始错误细节（HTTP 状态码、上游返回体、鉴权信息等）只记录日志，
/// 不透出给前端，避免暴露内部实现与敏感信息。
fn classify_llm_failure(raw: &str) -> ClassifiedLlmError {
    let kind = classify_llm_error_from_str(raw);
    let (error_type, user_message): (&'static str, String) = match &kind {
        LlmErrorKind::InvalidApiKey => (
            "invalid_api_key",
            "API Key 无效或已过期，请在设置中检查模型配置。".into(),
        ),
        LlmErrorKind::InsufficientBalance => {
            ("insufficient_balance", "账户余额不足，请充值后重试。".into())
        }
        LlmErrorKind::QuotaExceeded => {
            ("api_quota_exceeded", "API 配额已用尽，请检查账户额度。".into())
        }
        LlmErrorKind::RateLimited => (
            "rate_limited",
            "请求过于频繁，请稍等片刻再重试。".into(),
        ),
        LlmErrorKind::Timeout => ("timeout", "请求超时，请检查网络后重试。".into()),
        LlmErrorKind::NetworkError => {
            ("network_error", "网络连接失败，请检查网络后重试。".into())
        }
        LlmErrorKind::ModelNotFound => (
            "model_not_found",
            "模型不存在或暂不可用，请检查模型配置。".into(),
        ),
        LlmErrorKind::ContextLengthExceeded => (
            "context_length",
            "上下文已超出模型窗口限制，可发送 /compact 压缩历史后重试。".into(),
        ),
        LlmErrorKind::ContentPolicy => (
            "content_policy",
            "内容被安全策略拦截，请调整表述后重试。".into(),
        ),
        LlmErrorKind::ServerError | LlmErrorKind::Overloaded => (
            "server_error",
            "模型服务暂时不可用，请稍后重试。".into(),
        ),
        LlmErrorKind::CircuitBreakerOpen => (
            "circuit_breaker",
            "连续失败触发了熔断保护，请稍后重试。".into(),
        ),
        LlmErrorKind::RegionNotSupported => (
            "region_not_supported",
            "当前地区不支持该模型服务。".into(),
        ),
        LlmErrorKind::PermissionDenied => (
            "permission_denied",
            "没有访问该服务的权限，请检查账户配置。".into(),
        ),
        LlmErrorKind::BadRequest => ("bad_request", "请求参数有误，请检查模型配置。".into()),
        _ if raw.contains("MAIN_API_NOT_CONFIGURED") => (
            "no_main_api",
            "尚未配置主模型 API，请先在设置中完成配置。".into(),
        ),
        _ => ("unknown", "模型调用失败，请稍后重试。".into()),
    };
    ClassifiedLlmError {
        kind,
        error_type,
        user_message,
    }
}

/// 历史压缩结果：本次归档的消息条数与项目记忆沉淀说明。
struct CompactOutcome {
    archived: usize,
    memory_note: String,
}

/// 本轮摘要的 system prompt。
const TURN_SUMMARY_SYSTEM_PROMPT: &str = "你是记忆归档模块。把用户与桌面编程智能体的一轮会话记录压缩成 2-4 句中文摘要，必须涵盖：用户请求了什么、执行了哪些关键操作（读/写/改了哪些文件、跑了什么命令）、最终结果与遗留问题。直接输出摘要正文，不要前缀、标题或 markdown。";

/// 从工具参数 JSON 中提取关键目标（路径 / 命令 / 搜索模式）。
fn tool_target(args: &serde_json::Value) -> String {
    for key in ["path", "file_path", "command", "pattern"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// 本轮消息 → LLM 摘要输入的文本记录。
fn build_turn_transcript(messages: &[CodingMessage], working_directory: &str) -> String {
    let wd_line = if working_directory.trim().is_empty() {
        "工作目录：未选择（无工作区模式）".to_string()
    } else {
        format!("工作目录：{working_directory}")
    };
    let mut lines = vec![wd_line];
    for m in messages {
        match m.role {
            CodingRole::User => lines.push(format!("用户：{}", truncate_chars(&m.content, 1000))),
            CodingRole::Assistant => {
                lines.push(format!("助手：{}", truncate_chars(&m.content, 2000)));
            }
            CodingRole::ToolResult => {
                let target = m
                    .tool_arguments
                    .as_ref()
                    .map(tool_target)
                    .unwrap_or_default();
                let status = if m.tool_success.unwrap_or(false) { "成功" } else { "失败" };
                let detail = if target.is_empty() { String::new() } else { format!("（{target}）") };
                lines.push(format!(
                    "工具 {}{detail} {status}：{}",
                    m.tool_name.as_deref().unwrap_or("?"),
                    truncate_chars(&m.content, 300),
                ));
            }
            // 结构化调用意图由对应 ToolResult 承载
            CodingRole::ToolUse => {}
            CodingRole::Error => lines.push(format!("[错误] {}", truncate_chars(&m.content, 300))),
        }
    }
    truncate_chars(&lines.join("\n"), 6000)
}

/// LLM 摘要不可用时的规则摘要兜底。
fn rule_turn_digest(messages: &[CodingMessage]) -> String {
    let user = messages
        .iter()
        .rev()
        .find(|m| m.role == CodingRole::User)
        .map(|m| truncate_chars(&m.content, 200))
        .unwrap_or_default();
    let tools: Vec<String> = messages
        .iter()
        .filter(|m| m.role == CodingRole::ToolResult)
        .map(|m| {
            let fail = if m.tool_success.unwrap_or(false) { "" } else { "(失败)" };
            format!("{}{fail}", m.tool_name.as_deref().unwrap_or("?"))
        })
        .collect();
    let reply = messages
        .iter()
        .rev()
        .find(|m| m.role == CodingRole::Assistant)
        .map(|m| truncate_chars(&m.content, 400))
        .unwrap_or_default();
    if tools.is_empty() {
        format!("用户请求：{user}。回复：{reply}")
    } else {
        format!("用户请求：{user}。执行工具：{}。回复：{reply}", tools.join("、"))
    }
}

/// 从原始步骤 JSON 提取 {tool, arguments}（丢弃模型附加的无关字段）。
fn json_step(step: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "tool": step.get("tool").cloned().unwrap_or(serde_json::Value::Null),
        "arguments": step.get("arguments").cloned().unwrap_or(serde_json::Value::Null),
    })
}

// ============================================================================
// 项目记忆：跨会话的项目级约定与教训（应用数据目录按工作目录隔离存储）
// ============================================================================

/// 工作目录 → 记忆存储目录名（路径分隔符与非法字符替换为 '-'，如 g:\vivian-rs → g--vivian-rs）。
fn memory_dir_name(working_directory: &str) -> String {
    working_directory
        .trim_end_matches(['\\', '/'])
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// 项目记忆文件路径（应用数据目录 coding_memory/<工作目录编码>/project_memory.md）。
fn project_memory_path(working_directory: &str) -> std::path::PathBuf {
    get_user_data_dir()
        .join(CODING_MEMORY_DIR)
        .join(memory_dir_name(working_directory))
        .join(PROJECT_MEMORY_FILE)
}

/// 读取项目记忆全文（不做截断，重写合并用）。
fn read_project_memory_raw(working_directory: &str) -> Option<String> {
    if working_directory.trim().is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(project_memory_path(working_directory)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 读取项目记忆（注入/去重上下文用）。超长保留尾部——最新沉淀的条目。
fn read_project_memory(working_directory: &str) -> Option<String> {
    let full = read_project_memory_raw(working_directory)?;
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= PROJECT_MEMORY_MAX_CHARS {
        return Some(full);
    }
    let tail: String = chars[chars.len() - PROJECT_MEMORY_MAX_CHARS..].iter().collect();
    Some(format!("（较早内容已截断）\n{tail}"))
}

/// 全量写入项目记忆文件（文件头 + 正文，正文为空时只写文件头）。
fn write_project_memory(working_directory: &str, body: &str) -> Result<(), String> {
    if working_directory.trim().is_empty() {
        return Err("会话没有有效的工作目录".into());
    }
    let path = project_memory_path(working_directory);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    let mut content = PROJECT_MEMORY_HEADER.to_string();
    let body = body.trim();
    if !body.is_empty() {
        content.push('\n');
        content.push_str(body);
        content.push('\n');
    }
    std::fs::write(&path, content).map_err(|e| format!("写入 {PROJECT_MEMORY_FILE} 失败：{e}"))
}

/// 追加一段内容到项目记忆文件（不存在则带说明头创建，按日期分节）。
fn append_project_memory(working_directory: &str, body: &str) -> Result<(), String> {
    if working_directory.trim().is_empty() {
        return Err("会话没有有效的工作目录".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Ok(());
    }
    let path = project_memory_path(working_directory);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    if content.trim().is_empty() {
        content = PROJECT_MEMORY_HEADER.to_string();
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!(
        "\n## {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    content.push_str(body);
    content.push('\n');
    std::fs::write(&path, content).map_err(|e| format!("写入 {PROJECT_MEMORY_FILE} 失败：{e}"))
}

/// 按字符截断。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut r: String = s.chars().take(max).collect();
    r.push_str("\n…(truncated)");
    r
}

/// 无模型的工具结果裁剪：头尾保留、中段折叠（见 executor::prune_head_tail）。
fn prune_tool_result(content: &str, max: usize) -> String {
    crate::tools::executor::prune_head_tail(content, max)
}

/// 解析并读取文件引用：相对路径拼工作目录、沙箱校验、读取内容并截断。
///
/// 读取失败（不存在 / 超沙箱 / IO 错误）不中断整条消息，而是记录 error 供前端展示。
fn resolve_file_refs(working_directory: &str, refs: Vec<CodingFileRef>) -> Vec<CodingFileRef> {
    let mut out: Vec<CodingFileRef> = Vec::new();
    for r in refs.into_iter().take(FILE_REF_MAX_COUNT) {
        let mut resolved = r;
        let path = std::path::Path::new(&resolved.path);
        let abs = if path.is_absolute() {
            resolved.path.clone()
        } else {
            std::path::Path::new(working_directory)
                .join(&resolved.path)
                .to_string_lossy()
                .into_owned()
        };
        resolved.path = abs.clone();
        if !crate::tools::sandbox::is_path_within_working_directory(&abs, working_directory) {
            resolved.error = Some("路径不在工作目录内，已忽略".into());
            out.push(resolved);
            continue;
        }
        match std::fs::read_to_string(&abs) {
            Ok(content) => {
                let content = truncate_chars(&content, FILE_REF_MAX_CHARS);
                resolved.content = Some(content);
            }
            Err(e) => resolved.error = Some(format!("读取失败：{e}")),
        }
        out.push(resolved);
    }
    out
}
