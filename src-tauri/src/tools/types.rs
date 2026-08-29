//! 核心工具类型定义 - 增强版工具系统
//!
//! 定义工具系统的所有核心类型，包括工具 trait、工具结果、
//! 验证结果、权限结果、工具上下文、错误码等。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    File,
    Web,
    System,
    Memory,
    Media,
    Pet,
    Mcp,
}

impl ToolCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCategory::File => "file",
            ToolCategory::Web => "web",
            ToolCategory::System => "system",
            ToolCategory::Memory => "memory",
            ToolCategory::Media => "media",
            ToolCategory::Pet => "pet",
            ToolCategory::Mcp => "mcp",
        }
    }
}

/// 工具可见性分层：控制工具在 LLM 上下文中的展示粒度，减少 token 开销。
///
/// - `Always`：完整 schema 注入（核心高频工具）
/// - `Lazy`：仅名称+一行描述，完整 schema 通过 `tool_search` 按需加载
/// - `Deferred`：仅名称出现在 `<available-deferred-tools>` 块中
///
/// 默认行为由 `tool_call_manager` 根据 `ToolCategory` + `should_defer()` + `always_load()` 推断，
/// 个别工具可通过 `Tool::visibility_tier()` 覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolVisibility {
    /// 完整 schema 注入（默认）
    Always,
    /// 仅名称+一行描述（节省 token）
    Lazy,
    /// 仅名称，需 tool_search 加载完整 schema
    Deferred,
}

/// 工具场景：用于动态筛选暴露给 LLM 的工具子集
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolScene {
    /// 默认场景：全量暴露（向后兼容）
    #[default]
    Default,
    /// 低信任场景（陌生人/初识）：禁用系统控制类工具
    LowTrust,
    /// 专注场景（用户在工作）：只保留 Memory + 必要 System
    Focus,
    /// 闲聊场景：用户消息短 + 无任务关键词 + 近期无工具调用
    /// 只注入核心工具（memory_search / tool_search / cross_character）
    Chat,
    /// 任务场景：用户消息含任务关键词 OR 近期调过工具
    /// 注入核心 + 任务相关工具类别
    Task,
    /// 后台场景：proactive_tick 触发，无用户输入
    /// 只注入 memory_search / cross_character
    Idle,
}

impl ToolScene {
    /// 根据关系阶段 + 当前情绪 + 活动应用推断场景
    ///
    /// 优先级：低信任 > 专注 > 默认
    pub fn from_context(intimacy_stage: Option<&str>, emotion: Option<&str>) -> Self {
        Self::from_context_with_app(intimacy_stage, emotion, None)
    }

    /// 带活动应用上下文的场景推断
    ///
    /// `active_app` 为前台应用标题/进程名，用于判断是否处于专注工作场景。
    pub fn from_context_with_app(
        intimacy_stage: Option<&str>,
        emotion: Option<&str>,
        active_app: Option<&str>,
    ) -> Self {
        Self::from_full_context(
            intimacy_stage,
            emotion,
            active_app,
            "",
            false,
        )
    }

    /// 全上下文场景推断（整合关系/专注 + 用户输入/历史工具调用）
    ///
    /// 优先级：
    /// 1. 低信任阶段（stranger/acquainted）→ `LowTrust`
    /// 2. 专注场景（前台为 IDE/编辑器/办公软件）→ `Focus`
    /// 3. 用户输入驱动（Chat/Task/Idle）：
    ///    - `user_input` 为空 → `Idle`
    ///    - 含任务关键词（中日英三语）OR 近期调过工具 → `Task`
    ///    - 其他 → `Chat`
    ///
    /// `has_recent_tool_use` 由调用方通过 `ToolSystem::has_recent_tool_call` 查询。
    pub fn from_full_context(
        intimacy_stage: Option<&str>,
        _emotion: Option<&str>,
        active_app: Option<&str>,
        user_input: &str,
        has_recent_tool_use: bool,
    ) -> Self {
        // 低信任阶段：禁用控制类工具
        match intimacy_stage.unwrap_or("") {
            "stranger" | "acquainted" => return ToolScene::LowTrust,
            _ => {}
        }
        // 专注场景：检测到工作类应用时启用
        if let Some(app) = active_app {
            let app_lower = app.to_lowercase();
            const FOCUS_APPS: &[&str] = &[
                "code", "visual studio", "vscode", "idea", "pycharm", "rustrover",
                "webstorm", "goland", "clion", "eclipse", "intellij",
                "notepad++", "sublime", "vim", "neovim", "emacs",
                "terminal", "powershell", "cmd", "wsl",
                "word", "excel", "powerpoint", "outlook", "onenote",
                "photoshop", "illustrator", "figma", "blender",
                "unity", "unreal", "godot",
                "matlab", "jupyter", "rstudio",
            ];
            if FOCUS_APPS.iter().any(|k| app_lower.contains(k)) {
                return ToolScene::Focus;
            }
        }
        // 用户输入驱动：Chat / Task / Idle
        Self::from_user_input(user_input, has_recent_tool_use)
    }

    /// 基于用户输入 + 历史工具调用判定场景（Chat/Task/Idle）
    ///
    /// 规则（纯规则，不用 LLM）：
    /// - `user_input` 为空 → `Idle`
    /// - `user_input` 含任务关键词（中日英三语）→ `Task`
    /// - `has_recent_tool_use=true`（最近 3 轮调过工具）→ `Task`
    /// - `user_input` 长度 < 50 字 + 无任务关键词 + 无近期工具调用 → `Chat`
    /// - 其他（长消息但无任务关键词）→ `Chat`（保守判定，避免误注入大量工具）
    pub fn from_user_input(user_input: &str, has_recent_tool_use: bool) -> Self {
        // 无用户输入 → 后台场景
        let trimmed = user_input.trim();
        if trimmed.is_empty() {
            return ToolScene::Idle;
        }

        // 近期调过工具 → 任务场景
        if has_recent_tool_use {
            return ToolScene::Task;
        }

        // 含任务关键词 → 任务场景
        if contains_task_keyword(trimmed) {
            return ToolScene::Task;
        }

        // 默认闲聊场景
        ToolScene::Chat
    }

    /// 当前场景下给 LLM 的工具使用软提示（注入到 prompt，不强制屏蔽）
    ///
    /// 设计哲学：从"硬黑名单过滤"转向"延迟加载 + 场景软提示 + 执行时确认门"。
    /// 所有工具仍对 LLM 可见（活跃或延迟），由 LLM 自主判断是否调用，
    /// 危险操作通过 `check_permissions` 在执行时弹窗确认（软门），而非 prompt 层硬屏蔽。
    /// 这样可以避免"用户难过时想听音乐却被屏蔽媒体工具"之类死板行为。
    pub fn soft_hint(&self) -> &'static str {
        match self {
            ToolScene::LowTrust => {
                "当前关系阶段较低（陌生人/初识），用户对系统控制类操作可能尚未建立信任。\
                 调用 open_application / close_application / open_url / 输入控制类工具前，\
                 优先用语言确认用户意图；这些工具执行时会请求用户确认，请配合用户选择。"
            }
            ToolScene::Focus => {
                "用户正在专注工作（检测到 IDE/编辑器/办公软件前台）。\
                 避免主动发起娱乐性操作（媒体播放、桌宠大幅动作），\
                 优先低打扰的回应方式。用户明确请求时不受此限制。"
            }
            ToolScene::Chat => {
                "当前是陪伴对话场景，工作能力已直接注入：查资料（web_search / web_fetch）、\
                 后台任务（run_job / manage_job）、待办（update_todo / add_todo）、\
                 计划（plan_task）、多步编排（run_workflow）、向用户提问（ask_user）、\
                 派发大型工程任务（delegate_to_work_agent + get_work_status）。\
                 优先直接使用这些能力完成用户请求；未列出的工具先调用 tool_search 加载。"
            }
            ToolScene::Task => {
                "当前是任务场景，已注入任务相关工具的完整 schema。\
                 若需要的工具未在列表中，先调用 tool_search 加载。"
            }
            ToolScene::Idle => {
                "当前是后台场景（无用户输入），仅注入核心工具。\
                 优先低打扰的自主行为，避免主动发起需要用户确认的操作。"
            }
            ToolScene::Default => "",
        }
    }
}

/// 任务关键词检测（中日英三语）
///
/// 覆盖常见任务动词：帮助/查找/打开/执行/搜索/读写/启动/关闭/运行/播放/创建/删除/修改/复制/移动/下载/上传
/// 中文 / 英文 / 日文 三语支持
fn contains_task_keyword(text: &str) -> bool {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static TASK_KEYWORD_REGEX: Lazy<Regex> = Lazy::new(|| {
        // 中日英三语任务关键词
        // 中文：帮/帮我/查找/找/打开/开/执行/搜索/搜/读取/读/写入/写/启动/起/关闭/关/运行/播放/放/创建/建/删除/删/修改/改/复制/拷贝/移动/搬/下载/上传
        // 英文：help/find/search/open/run/execute/read/write/start/stop/close/launch/play/create/delete/modify/copy/move/download/upload
        // 日文：助けて/探す/探して/開く/開いて/実行/検索/読む/読んで/書く/書いて/起動/終了/閉じる/閉じて/再生/作成/削除/変更/コピー/移動/ダウンロード/アップロード
        Regex::new(
            r"(?i)\
            \b(help|find|search|open|run|execute|read|write|start|stop|close|launch|play|create|delete|modify|copy|move|download|upload|look\s*up|show\s*me|get\s*the|fetch)\b|\
            (帮|帮我|帮忙|查找|找一下|找下|打开|开一下|开下|执行|搜索|搜一下|搜下|读取|读一下|读下|写入|写一下|写下|启动|起一下|关闭|关一下|关下|运行|跑一下|播放|放一下|创建|建一个|删除|删掉|删一下|修改|改一下|改下|复制|拷贝|移动|搬|下载|上传|看一下|看下|查一下|查下|搞一下|搞定|处理|办一下|办下)\
            |(助けて|探して|探す|開いて|開く|実行|検索|読んで|読む|書いて|書く|起動|終了|閉じて|閉じる|再生|作成|削除|変更|コピー|移動|ダウンロード|アップロード|やって|して)"
        ).expect("task keyword regex")
    });

    TASK_KEYWORD_REGEX.is_match(text)
}

/// 工具上下文修改器：工具执行后回调，可修改后续工具的 `ToolUseContext`。
///
/// 使用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>` 以保留 `Clone`。
/// 调用方约定只触发一次（由 `ToolCallManager` 在 `execute_multi_step` 中应用）。
pub type ContextModifier = Arc<dyn Fn(&mut ToolUseContext) + Send + Sync>;

/// 工具结果
#[derive(Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 工具返回的数据
    pub data: Option<Value>,
    /// 错误信息（失败时）
    pub error: Option<String>,
    /// 是否成功
    pub success: bool,
    /// 是否标志着用户目标已达成（true 时 Agent 循环应立即终止，不再继续推理或调用其他工具）
    ///
    /// 由 `Tool::signals_goal_completion() && result.success` 自动推导，
    /// 也可由工具在特殊分支显式设置（如壁纸切换成功后任务即完成）。
    /// 默认 false。Executor 在工具执行后根据 trait 声明覆写此字段。
    #[serde(default)]
    pub goal_completed: bool,
    /// 上下文修改器（不参与序列化，工具执行后由编排器应用）
    #[serde(skip)]
    pub context_modifier: Option<ContextModifier>,
}

impl std::fmt::Debug for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolResult")
            .field("data", &self.data)
            .field("error", &self.error)
            .field("success", &self.success)
            .field("goal_completed", &self.goal_completed)
            .field(
                "context_modifier",
                &self.context_modifier.as_ref().map(|_| "<closure>"),
            )
            .finish()
    }
}

impl ToolResult {
    /// 创建成功结果
    pub fn success(data: Value) -> Self {
        Self {
            data: Some(data),
            error: None,
            success: true,
            goal_completed: false,
            context_modifier: None,
        }
    }

    /// 创建标准格式的成功返回
    pub fn standard_success(message: &str, data: Option<Value>) -> Self {
        let payload = serde_json::json!({
            "success": true,
            "message": message,
            "data": data,
            "error": null,
        });
        Self {
            data: Some(payload),
            error: None,
            success: true,
            goal_completed: false,
            context_modifier: None,
        }
    }

    /// 创建标准格式的错误返回
    pub fn standard_error(message: &str, error: Option<&str>, data: Option<Value>) -> Self {
        let payload = serde_json::json!({
            "success": false,
            "message": message,
            "data": data,
            "error": error.unwrap_or(message),
        });
        Self {
            data: Some(payload),
            error: Some(error.unwrap_or(message).to_string()),
            success: false,
            goal_completed: false,
            context_modifier: None,
        }
    }

    /// 创建简单错误结果
    pub fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            data: None,
            error: Some(msg.clone()),
            success: false,
            goal_completed: false,
            context_modifier: None,
        }
    }

    /// 附加上下文修改器，返回自身便于链式构造。
    pub fn with_context_modifier(mut self, modifier: ContextModifier) -> Self {
        self.context_modifier = Some(modifier);
        self
    }

    /// 显式标记本工具调用已完成用户目标（Agent 循环应在反馈结果后终止）。
    /// 通常无需手动调用：Executor 会根据 `Tool::signals_goal_completion()` 自动设置；
    /// 仅当工具在特定分支才视为"目标完成"时使用。
    pub fn with_goal_completed(mut self) -> Self {
        self.goal_completed = self.success;
        self
    }
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            success: false,
            goal_completed: false,
            context_modifier: None,
        }
    }
}

/// 工具使用上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseContext {
    /// 会话 ID
    pub session_id: String,
    /// 用户 ID
    pub user_id: String,
    /// 工作目录
    pub working_directory: String,
    /// 调用时间戳
    pub timestamp: DateTime<Utc>,
    /// Vivian 当前主导情绪标签（如 "joy" / "sadness" / "neutral"）
    #[serde(default)]
    pub current_emotion: Option<String>,
    /// 当前关系阶段（如 "stranger" / "close" / "soulmate"）
    #[serde(default)]
    pub intimacy_stage: Option<String>,
    /// 最近记忆摘要（供工具感知上下文，如"用户最近在聊工作压力"）
    #[serde(default)]
    pub recent_memory_summary: Option<String>,
    /// 工具场景（由情绪/关系阶段推断，控制暴露给 LLM 的工具子集）
    #[serde(default)]
    pub tool_scene: ToolScene,
    /// 当前角色 ID（多角色架构下用于路由到对应角色的 MemoryManager / PsychologyManager / manifest）
    #[serde(default)]
    pub char_id: String,
    /// 当前用户消息原文（供 observe_user 等工具记录 source_text）
    #[serde(default)]
    pub user_message: Option<String>,
    /// 会话级访问级别覆盖（None 时回退全局 runtime config 的 access_level；
    /// 编程智能体按会话权限设置，实现会话粒度的工具放行控制）
    #[serde(default)]
    pub access_level: Option<AgentAccessLevel>,
    /// 调用方智能体类型："chat"（陪伴对话）/ "work"（编程/工作智能体）
    ///
    /// 供场景敏感工具做差异化默认（如 web_search 的默认结果数：聊天 10 / 工作 15）。
    /// 缺省为 "chat"（主对话链与后台任务均为聊天侧）。
    #[serde(default)]
    pub agent_kind: String,
}

impl Default for ToolUseContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            user_id: String::new(),
            working_directory: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            timestamp: Utc::now(),
            current_emotion: None,
            intimacy_stage: None,
            recent_memory_summary: None,
            tool_scene: ToolScene::Default,
            char_id: String::new(),
            user_message: None,
            access_level: None,
            agent_kind: "chat".to_string(),
        }
    }
}

impl ToolUseContext {
    pub fn new(session_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            user_id: user_id.into(),
            working_directory: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            timestamp: Utc::now(),
            current_emotion: None,
            intimacy_stage: None,
            recent_memory_summary: None,
            tool_scene: ToolScene::Default,
            char_id: String::new(),
            user_message: None,
            access_level: None,
            agent_kind: "chat".to_string(),
        }
    }

    pub fn with_working_directory(mut self, dir: impl Into<String>) -> Self {
        self.working_directory = dir.into();
        self
    }

    /// 设置调用方智能体类型（"chat" / "work"）
    pub fn with_agent_kind(mut self, kind: impl Into<String>) -> Self {
        self.agent_kind = kind.into();
        self
    }

    /// 是否来自工作智能体（编程 / 子代理 / 工作会话）
    pub fn is_work_agent(&self) -> bool {
        self.agent_kind == "work"
    }

    pub fn with_emotion(mut self, emotion: impl Into<String>) -> Self {
        self.current_emotion = Some(emotion.into());
        self.recompute_scene();
        self
    }

    pub fn with_intimacy_stage(mut self, stage: impl Into<String>) -> Self {
        self.intimacy_stage = Some(stage.into());
        self.recompute_scene();
        self
    }

    pub fn with_memory_summary(mut self, summary: impl Into<String>) -> Self {
        self.recent_memory_summary = Some(summary.into());
        self
    }

    pub fn with_char_id(mut self, char_id: impl Into<String>) -> Self {
        self.char_id = char_id.into();
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    pub fn with_user_message(mut self, msg: impl Into<String>) -> Self {
        self.user_message = Some(msg.into());
        self
    }

    /// 根据当前情绪/关系阶段重新计算工具场景
    pub fn recompute_scene(&mut self) {
        self.tool_scene = ToolScene::from_context(
            self.intimacy_stage.as_deref(),
            self.current_emotion.as_deref(),
        );
    }
}

/// 工具错误码
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolErrorCode {
    UnknownError,
    ToolNotFound,
    InvalidInput,
    PermissionDenied,
    UserDenied,
    TimeoutError,
}

impl ToolErrorCode {
    /// 获取错误码对应的详细信息和建议
    pub fn get_error_info(&self) -> ToolErrorInfo {
        match self {
            ToolErrorCode::UnknownError => ToolErrorInfo {
                name: "UnknownError".to_string(),
                code: 0,
                suggestion: "发生了未知错误，请重试或联系开发者。".to_string(),
            },
            ToolErrorCode::ToolNotFound => ToolErrorInfo {
                name: "ToolNotFound".to_string(),
                code: 1,
                suggestion: "工具不存在，请检查工具名称是否正确，或使用 tool_list 查看可用工具。".to_string(),
            },
            ToolErrorCode::InvalidInput => ToolErrorInfo {
                name: "InvalidInput".to_string(),
                code: 2,
                suggestion: "输入参数格式不正确，请检查参数类型和必填项。".to_string(),
            },
            ToolErrorCode::PermissionDenied => ToolErrorInfo {
                name: "PermissionDenied".to_string(),
                code: 3,
                suggestion: "权限不足，需要用户授权或切换到自动模式。".to_string(),
            },
            ToolErrorCode::UserDenied => ToolErrorInfo {
                name: "UserDenied".to_string(),
                code: 400,
                suggestion: "用户拒绝了操作，请不要重复尝试。".to_string(),
            },
            ToolErrorCode::TimeoutError => ToolErrorInfo {
                name: "Timeout".to_string(),
                code: 4,
                suggestion: "操作超时，请重试或尝试简化操作。".to_string(),
            },
        }
    }
}

/// 工具错误信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorInfo {
    pub name: String,
    pub code: i32,
    pub suggestion: String,
}

/// 权限模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// 默认模式：按规则检查
    Default,
    /// 绕过模式：直接允许所有操作
    Bypass,
    /// 询问模式：所有操作都需用户确认
    Ask,
}

impl Default for PermissionMode {
    fn default() -> Self {
        PermissionMode::Default
    }
}

/// 工具风险分级
///
/// 按副作用范围从低到高排序，越高表示对系统/用户环境影响越大。
/// 与 `AgentAccessLevel` 共同决定 `allow` / `ask` / `deny`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolRiskTier {
    /// 无副作用：纯查询、内存读取、宠物表情等
    Safe,
    /// 文件系统读取：read_file / list_directory / search_files / grep
    FsRead,
    /// 文件系统写入：write_file / edit_file / delete_file
    FsWrite,
    /// Shell 命令执行：open_application / close_application / 系统控制
    Shell,
    /// 网络访问：web_search / open_url / 远程 API 调用
    Network,
    /// 输入控制：键鼠模拟 / 剪贴板 / 焦点切换
    InputControl,
}

impl ToolRiskTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolRiskTier::Safe => "safe",
            ToolRiskTier::FsRead => "fs-read",
            ToolRiskTier::FsWrite => "fs-write",
            ToolRiskTier::Shell => "shell",
            ToolRiskTier::Network => "network",
            ToolRiskTier::InputControl => "input-control",
        }
    }
}

/// Agent 访问级别
///
/// 由 `config.tools.access_level` 配置，决定当前会话允许执行的风险等级。
/// 级别从低到高，高级别包含低级别权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentAccessLevel {
    /// 只读：仅允许 Safe 工具（无副作用查询）
    ReadOnly,
    /// 文件读取：+ FsRead
    FsRead,
    /// 文件写入：+ FsWrite + Network（可联网搜索）
    FsWrite,
    /// 完全控制：+ Shell + InputControl
    FullControl,
}

impl AgentAccessLevel {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "read-only" | "readonly" => Self::ReadOnly,
            "fs-write" | "fswrite" => Self::FsWrite,
            "full-control" | "fullcontrol" | "full" => Self::FullControl,
            _ => Self::FsRead,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::FsRead => "fs-read",
            Self::FsWrite => "fs-write",
            Self::FullControl => "full-control",
        }
    }
}

/// 权限网关矩阵：访问级别 × 工具风险 → 行为
///
/// 规则：
/// - Safe 工具在所有级别下允许
/// - 工具风险 ≤ 当前访问级别时允许
/// - 工具风险 = 下一级时需要询问
/// - 工具风险超出当前级别两级以上时拒绝
pub fn policy_for(access: AgentAccessLevel, risk: ToolRiskTier) -> PermissionBehavior {
    use PermissionBehavior::*;
    // Safe 永远允许
    if risk == ToolRiskTier::Safe {
        return Allow;
    }
    // FullControl 永远允许（除 InputControl 询问以保留兜底）
    if access == AgentAccessLevel::FullControl {
        return if risk == ToolRiskTier::InputControl { Ask } else { Allow };
    }
    // Network 单独处理：一维 ordinal 无法表达"FsWrite 包含 Network 但不包含 Shell"
    // （Network ordinal 高于 Shell，通用矩阵会在 FsRead/FsWrite 级别误判为 Deny）
    if risk == ToolRiskTier::Network {
        return match access {
            AgentAccessLevel::ReadOnly => Deny,
            AgentAccessLevel::FsRead => Ask,
            AgentAccessLevel::FsWrite | AgentAccessLevel::FullControl => Allow,
        };
    }
    // Shell 单独处理：启动/关闭应用等 Shell 工具在中间级别一律询问，
    // FullControl 才直接放行（open_application 另有信任列表快速通道兜底）
    if risk == ToolRiskTier::Shell {
        return match access {
            AgentAccessLevel::ReadOnly => Deny,
            AgentAccessLevel::FullControl => Allow,
            AgentAccessLevel::FsRead | AgentAccessLevel::FsWrite => Ask,
        };
    }
    // 计算等级差：access 的最大允许风险
    let access_max = match access {
        AgentAccessLevel::ReadOnly => ToolRiskTier::Safe,
        AgentAccessLevel::FsRead => ToolRiskTier::FsRead,
        AgentAccessLevel::FsWrite => ToolRiskTier::FsWrite,
        AgentAccessLevel::FullControl => ToolRiskTier::InputControl,
    };
    if risk <= access_max {
        Allow
    } else if risk as u8 - access_max as u8 == 1 {
        Ask
    } else {
        Deny
    }
}

/// 权限行为
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    /// 允许
    Allow,
    /// 拒绝
    Deny,
    /// 需要询问
    Ask,
    /// 透传（由上层决定）
    Passthrough,
}

/// 权限检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResult {
    /// 权限行为
    pub behavior: PermissionBehavior,
    /// 提示消息
    pub message: String,
    /// 更新后的输入参数（权限层可调整）
    pub updated_input: Option<Value>,
}

impl Default for PermissionResult {
    fn default() -> Self {
        Self {
            behavior: PermissionBehavior::Passthrough,
            message: String::new(),
            updated_input: None,
        }
    }
}

impl PermissionResult {
    pub fn allow() -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            message: String::new(),
            updated_input: None,
        }
    }

    pub fn allow_with_input(updated_input: Value) -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            message: String::new(),
            updated_input: Some(updated_input),
        }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            message: message.into(),
            updated_input: None,
        }
    }

    pub fn ask(message: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            message: message.into(),
            updated_input: None,
        }
    }

    pub fn passthrough(message: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Passthrough,
            message: message.into(),
            updated_input: None,
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.behavior == PermissionBehavior::Allow
    }

    pub fn is_denied(&self) -> bool {
        self.behavior == PermissionBehavior::Deny
    }

    pub fn requires_confirmation(&self) -> bool {
        matches!(self.behavior, PermissionBehavior::Ask | PermissionBehavior::Passthrough)
    }
}

/// 验证结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// 是否通过
    pub result: bool,
    /// 提示消息
    pub message: String,
    /// 错误码
    pub error_code: i32,
    /// 验证后的输入参数（含 schema 默认值/类型转换）
    pub data: Option<Value>,
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self {
            result: true,
            message: String::new(),
            error_code: 0,
            data: None,
        }
    }
}

impl ValidationResult {
    pub fn success(data: Option<Value>) -> Self {
        Self {
            result: true,
            message: String::new(),
            error_code: 0,
            data,
        }
    }

    pub fn failure(message: impl Into<String>, error_code: i32) -> Self {
        Self {
            result: false,
            message: message.into(),
            error_code,
            data: None,
        }
    }
}

/// 权限上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    /// 权限模式
    pub mode: PermissionMode,
    /// Agent 访问级别（与工具 risk() 共同决定 allow/ask/deny）
    #[serde(default = "default_access_level_enum")]
    pub access_level: AgentAccessLevel,
    /// 额外工作目录（路径 → 权限集合）
    pub additional_working_directories: HashMap<String, WorkingDirectoryPermission>,
    /// 始终允许的工具规则
    pub always_allow: Vec<String>,
    /// 始终拒绝的工具规则
    pub always_deny: Vec<String>,
    /// 始终询问的工具规则
    pub always_ask: Vec<String>,
}

fn default_access_level_enum() -> AgentAccessLevel {
    AgentAccessLevel::FullControl
}

/// 工作目录权限
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingDirectoryPermission {
    /// 路径
    pub path: String,
    /// 允许的操作（read/write/delete）
    pub permissions: Vec<String>,
    /// 是否只读
    pub is_read_only: bool,
}

impl Default for PermissionContext {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            access_level: AgentAccessLevel::FullControl,
            additional_working_directories: HashMap::new(),
            always_allow: Vec::new(),
            always_deny: Vec::new(),
            always_ask: Vec::new(),
        }
    }
}

impl PermissionContext {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    pub fn with_access_level(mut self, level: AgentAccessLevel) -> Self {
        self.access_level = level;
        self
    }

    pub fn is_bypass_mode(&self) -> bool {
        self.mode == PermissionMode::Bypass
    }

    pub fn is_ask_mode(&self) -> bool {
        self.mode == PermissionMode::Ask
    }

    /// 添加工作目录
    pub fn add_working_directory(&mut self, path: impl Into<String>, read_only: bool) {
        let path = path.into();
        self.additional_working_directories.insert(
            path.clone(),
            WorkingDirectoryPermission {
                path,
                permissions: if read_only {
                    vec!["read".to_string()]
                } else {
                    vec!["read".to_string(), "write".to_string(), "delete".to_string()]
                },
                is_read_only: read_only,
            },
        );
    }

    /// 检查路径是否在工作目录中
    pub fn is_path_in_working_directory(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        for wd in self.additional_working_directories.values() {
            let wd_normalized = normalize_path(&wd.path);
            if normalized.starts_with(&wd_normalized) {
                return true;
            }
        }
        false
    }

    /// 获取工作目录权限
    pub fn get_working_directory_permissions(&self, path: &str) -> Option<&[String]> {
        let normalized = normalize_path(path);
        for wd in self.additional_working_directories.values() {
            let wd_normalized = normalize_path(&wd.path);
            if normalized.starts_with(&wd_normalized) {
                return Some(&wd.permissions);
            }
        }
        None
    }
}

/// 路径规范化（解析 `..`/`.`，统一分隔符）
///
/// 注意：必须真正"解析" `..`（弹出上一级）而非简单过滤，
/// 否则 `/a/b/../c` 会被错算成 `/a/b/c`，导致权限检查评估的路径
/// 与操作系统实际访问的路径不一致（如 `/wd/../../etc` 被误判仍在 `/wd` 内）。
pub fn normalize_path(path: &str) -> String {
    use std::path::{Component, PathBuf};

    let p = PathBuf::from(path);
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// 工具定义（用于序列化和传输）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub is_read_only: bool,
    pub category: String,
}

/// 工具 trait - 所有工具必须实现的核心接口
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称
    fn name(&self) -> &str;

    /// 工具描述
    fn description(&self) -> &str;

    /// 按界面语言返回工具描述，用于 ToolSemanticFilter 语义匹配与多语言 prompt 注入
    ///
    /// `lang` 已归一化为 "zh"/"en"/"ja"。默认实现 fallback 到 `description()`（英文），
    /// 具体工具可 override 提供 ZH/JA 版本，让用户输入语言与工具描述语言对齐以提升语义匹配精度。
    fn description_in(&self, lang: &str) -> &str {
        let _ = lang;
        self.description()
    }

    /// 输入参数 JSON Schema
    fn parameters_schema(&self) -> Value;

    /// 按界面语言返回输入参数 JSON Schema（参数 description 字段本地化）
    ///
    /// `lang` 已归一化为 "zh"/"en"/"ja"。默认实现 fallback 到 `parameters_schema()`（英文），
    /// 具体工具可 override 提供 ZH/JA 版本，让心智页模板预览的参数说明与界面语言一致。
    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        self.parameters_schema()
    }

    /// 验证输入参数
    async fn validate_input(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> ValidationResult;

    /// 检查权限
    async fn check_permissions(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> PermissionResult;

    /// 执行工具
    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult;

    /// 是否只读操作
    fn is_read_only(&self) -> bool;

    /// 工具类别
    fn category(&self) -> ToolCategory;

    /// 是否为智能体自进化创造的非默认工具（自建工具）
    ///
    /// 默认 false；`DynamicTool`（自建工具）覆盖为 true。
    /// 供前端设置页以特殊样式区分展示。
    fn is_custom(&self) -> bool {
        false
    }

    /// 是否破坏性操作（默认 false）
    fn is_destructive(&self) -> bool {
        false
    }

    /// 工具风险分级（默认 Safe）
    ///
    /// 权限网关根据此返回值与 `AgentAccessLevel` 矩阵决定 allow/ask/deny。
    /// 副作用越大的工具应返回越高的风险分级。
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    /// 是否始终全量暴露给 LLM（核心工具默认 false，由具体工具覆盖为 true）
    ///
    /// true：工具的完整 name + description + schema 始终注入 system prompt / tools 字段
    /// false：可能被延迟加载（参见 `should_defer`）
    fn always_load(&self) -> bool {
        false
    }

    /// 是否延迟加载（默认 false）
    ///
    /// true：初始 prompt 只注入工具名（在 `<available-deferred-tools>` 块中），
    ///       LLM 需先调用 `tool_search` 拿到完整 schema 才能调用本工具。
    /// false：与 `always_load` 配合决定注入方式：
    ///       - always_load=true → 全量注入
    ///       - always_load=false && should_defer=false → 全量注入（常用工具）
    ///       - always_load=false && should_defer=true → 仅注入名字（长尾工具）
    fn should_defer(&self) -> bool {
        false
    }

    /// 一行能力描述（供 ToolSearch 关键词匹配，比 description 短）
    ///
    /// 建议 3-10 词，不要与工具名重复的词。
    /// 例如 wallpaper 工具的 search_hint 可以是 "动态壁纸切换与控制"
    fn search_hint(&self) -> &str {
        ""
    }

    /// 反用例清单：明确告诉 LLM "什么情况下不要调用本工具"
    ///
    /// 每条是一个简短的自然语言句子，会被拼接到 description 末尾的
    /// `Do NOT use this tool for:` 段落。返回空切片表示无反用例约束。
    ///
    /// 设计目的：降低 LLM 在语义相近工具间的误调用（如"打开应用"vs"打开网址"）。
    fn anti_use_cases(&self) -> &[&str] {
        &[]
    }

    /// 工具可见性分层：控制本工具在 LLM 上下文中的展示粒度。
    ///
    /// 默认实现从现有标志推断：
    /// - `should_defer=true` → `Deferred`
    /// - `always_load=true` → `Always`
    /// - 其余 → `Always`（由渲染层按 category 进一步调整为 `Lazy`）
    ///
    /// 个别工具可覆盖此方法以显式指定分层。
    fn visibility_tier(&self) -> ToolVisibility {
        if self.should_defer() {
            ToolVisibility::Deferred
        } else {
            ToolVisibility::Always
        }
    }

    /// 是否在执行成功后即标志用户目标已完成。
    ///
    /// 返回 `true` 时，Executor 会在 `result.success == true` 的情况下
    /// 自动把 `ToolResult.goal_completed` 置为 `true`；Agent 循环检测到
    /// 该标志后会立即终止后续工具调用轮次，避免 LLM 在任务已完成的
    /// 情况下继续推理出多余动作（例如壁纸切换成功后又去 web_search 找图）。
    ///
    /// 适用工具：副作用型且"成功即终结"的工具，例如：
    /// - `wallpaper_set`：壁纸切换成功 = 用户目标达成
    /// - `open_application`：打开应用成功 = 用户目标达成
    /// - 媒体播放/暂停、消息发送等
    ///
    /// 不适用工具：查询型工具（如 `wallpaper_list` / `web_search` /
    /// `read_file`），它们只是为后续步骤提供信息，成功不等于任务完成。
    fn signals_goal_completion(&self) -> bool {
        false
    }

    /// 转换为工具定义（含反用例拼接的完整描述）
    fn to_definition(&self) -> ToolDefinition {
        let full_desc = self.render_full_description();
        ToolDefinition {
            name: self.name().to_string(),
            description: full_desc,
            input_schema: self.parameters_schema(),
            is_read_only: self.is_read_only(),
            category: self.category().as_str().to_string(),
        }
    }

    /// 渲染含反用例的完整描述
    fn render_full_description(&self) -> String {
        let base = self.description();
        let anti = self.anti_use_cases();
        if anti.is_empty() {
            return base.to_string();
        }
        let mut bullets = String::new();
        for case in anti {
            bullets.push_str(&format!("- {}\n", case));
        }
        format!("{}\n\nDo NOT use this tool for:\n{}", base, bullets.trim_end())
    }

    /// 转换为 OpenAI 工具格式（含反用例）
    fn to_openai_format(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.render_full_description(),
                "parameters": self.parameters_schema(),
            }
        })
    }
}
