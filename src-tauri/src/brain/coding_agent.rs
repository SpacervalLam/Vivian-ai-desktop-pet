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
    // 可视化组件（SVG 流程图/图表渲染为编程页卡片，约束内嵌工具 description）
    "show_widget",
    // 工作智能体独立待办（与陪伴 todo 分离，随会话持久化；整表替换，驱动每轮推进）
    "work_todo_write",
    // 工作智能体询问（方向分叉时出选择题，挂起等待用户抉择）
    "work_ask_user",
    // 工作智能体委派（独立上下文的子 agent，只回传最终文本；可后台派发）
    "work_delegate",
    // 后台子任务的收集 / 取消 / 列表
    "work_job",
    // 能力进化工具
    "create_skill",
    "use_skill",
    "search_skill",
    "create_tool",
    "create_plugin",
    "delete_plugin",
    "list_provider_presets",
    "manage_provider_preset",
];

/// Code 模式单次程序的最大步骤数。
pub const CODE_MODE_MAX_STEPS: usize = 16;

/// /compact 压缩后保留的最近消息条数（其余部分被摘要替换进上下文）。
const COMPACT_KEEP_MESSAGES: usize = 24;
/// /compact 可压缩的最小消息条数（不足则提示无需压缩）。
const COMPACT_MIN_MESSAGES: usize = 8;
/// 上下文占用达到窗口上限的该百分比时，自动压缩早期历史（防止请求超窗失败）。
const AUTO_COMPACT_THRESHOLD_PCT: u64 = 75;
/// 持久化时保留的会话数量上限（按最近更新排序后取前 N 个）。
const PERSIST_MAX_SESSIONS: usize = 30;
/// /compact 旧历史摘要的系统提示词。
const COMPACT_SYSTEM_PROMPT: &str = "你是对话历史压缩器。把下面这段编程会话历史压缩成一份简洁但信息完整的中文摘要，保留：已解决的问题、关键文件路径、做出的改动、当前任务进展、遗留待办。不要复述每条工具输出细节，控制在 200 字以内，直接输出摘要正文。";

/// 项目记忆文件名（存储在工作区 `.vivian/` 目录内，项目级——随项目走，用户可直接查看编辑）。
const PROJECT_MEMORY_FILE: &str = "memory.md";
/// 工作区内项目记忆的子目录名。
const PROJECT_MEMORY_DIR: &str = ".vivian";
/// 旧版项目记忆在应用数据目录下的存储目录（仅迁移用，不再新写）。
const CODING_MEMORY_DIR: &str = "coding_memory";
/// 项目记忆注入 system prompt 的最大字符数（超长保留尾部——最新沉淀的条目）。
const PROJECT_MEMORY_MAX_CHARS: usize = 8000;
/// memory.md 首次创建时写入的文件头（说明用途与维护方式）。
const PROJECT_MEMORY_HEADER: &str = "# 项目记忆\n\n\
    > 本文件由桌面编程智能体跨会话自动维护，沉淀对应工作目录项目的约定、结构与经验教训。\n\
    > 存储在工作区 `.vivian/` 目录（项目级，随项目走）；每次新会话自动注入上下文。\n\
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

/// 范围纪律（防「悄悄把活干小」）：coding / minimal 模式共用。
///
/// 模型最常见的失手不是写错代码，而是**交付缩水**——只做请求里最省事的那部分、
/// 碰到一个障碍就宣布 blocked、把没做的部分在总结里轻描淡写地略过。
/// 这段把三件事讲死：不许缩小范围、blocked 有准入门槛、收尾要逐条对照原始请求。
/// （code 模式是「一次性编排成程序」，没有 blocked 概念，它那边只取范围那一条。）
const SCOPE_DISCIPLINE: &str = "\n\n# 范围纪律（不要缩小交付）\n\
    - **不要擅自缩小范围**：用户请求里的每一项都要落实。做不到、不该做、或你判断可以延后的，\
    在总结里**点名说明是哪一项、为什么**——不要默默略过，也不要假装全做完了。\
    宁可交一半并说清楚，也不要交一个看起来完整、实则缺项的结果。\n\
    - **不许把「简化版」当交付**：不要用「先这样，之后再补」打发一个现在就能做完的需求。\
    确实需要分期时，明确说出分期点和剩余部分。\n\
    - **blocked 有准入门槛**：连续遇到 3 个**真实**阻碍（有具体报错/证据的失败，\
    而不是「不确定」「可能有问题」「需要更多信息」这类空泛顾虑）才允许判定为 blocked。\
    遇到 1 个错误先换思路继续——换个命令、换条路径、绕开或先修根因——\
    不要第一次失败就停下来汇报。\n\
    - **收尾自检**：宣布完成之前，把原始请求逐条过一遍，每条都要对得上具体的文件改动、\
    命令输出或明确结论。对不上的条目，要么现在补做，要么在总结里单列成「未完成」。";

/// 按模式过滤工具集。
fn tools_for_mode(mode: &str) -> Vec<&'static str> {
    match mode {
        "minimal" => vec!["run_command", "edit_file"],
        _ => CODING_TOOLS.to_vec(),
    }
}

/// 会话推理等级 → 推理偏好（low 关闭；medium / high 按档位开启）。
/// 档位经 provider 层按模型能力校验，不支持的档位自动回退默认档。
pub(crate) fn reasoning_level_to_pref(level: &str) -> ReasoningPreference {
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
///
/// `Notice` 是宿主自己产生的**中性状态消息**（上下文压缩、预算续轮等），
/// 与 `Error` 分开：后者代表执行失败，需要红色警示；前者只是提示，
/// 混用会让用户把正常状态读成故障。Notice 不会回传给 LLM。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingRole {
    User,
    Assistant,
    ToolUse,
    ToolResult,
    Error,
    Notice,
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

/// 智能体渲染的可视化组件（show_widget 工具，SVG）。
///
/// 组件代码以原始 SVG 字符串随消息持久化（镜像 `images` 的载荷隔离：
/// 组件不进 LLM 上下文，只作为工具产出单向推给前端渲染）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingWidget {
    /// 卡片标题（展示 + 导出文件名）
    pub title: String,
    /// 组件类型（固定 "svg"）
    pub kind: String,
    /// 原始 SVG 代码（viewBox 0 0 680 ...，不含 <html>/<head>/<body>）
    pub code: String,
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
    /// assistant 消息附带的可视化组件（show_widget 工具，随消息持久化）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub widgets: Option<Vec<CodingWidget>>,
    /// 用户在上一轮任务执行期间排队的插话（构建 LLM 消息时加插话标注，
    /// 帮助模型区分"对当前任务的补充/修正"与"全新对话"）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub interjected: Option<bool>,
    /// 用户在任务执行期间给出的"引导"消息（构建 LLM 消息时加引导标注，
    /// 提示模型这是对当前工作的引导/指示，需在后续工作中遵循）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub guided: Option<bool>,
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

/// 会话的附加工作区 —— 主工作区（[`CodingSession::working_directory`]）之外额外授权的目录。
///
/// 权限层本身支持一个会话注册多个工作目录（各自独立的只读标记与操作白名单），
/// 这里把「主根 + 附加根」的集合固化到会话上，让多工作区随会话一起持久化。
/// 主根仍然决定相对路径解析、项目记忆位置、终端 cwd 与提示词环境块；附加根只扩大可访问范围。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraWorkspace {
    /// 规范化后的绝对路径
    pub path: String,
    /// 只读：拒绝写入与删除，读取不受影响
    #[serde(default)]
    pub read_only: bool,
}

/// 编程会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingSession {
    pub session_id: String,
    pub char_id: String,
    /// 主工作区（空串表示「无工作区模式」：不绑定目录，文件操作走绝对路径）。
    pub working_directory: String,
    /// 附加工作区：主工作区之外可访问的目录。旧会话反序列化时自动补齐为空。
    #[serde(default)]
    pub extra_workspaces: Vec<ExtraWorkspace>,
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
    /// 会话内文件变更清单（按文件聚合：累计增删行数 + 最近一次变更的 unified diff），
    /// 供前端「变更」页展示 文件类型图标 / 文件名 / +-行数，并点击查看 diff。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_changes: Vec<FileChange>,
    /// 单条消息级反馈（消息下标 → "up" / "down"）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub message_feedback: HashMap<usize, String>,
    /// 单条助手回复**由宿主记录**的本轮改动文件清单（消息下标 → 改动项）。
    ///
    /// 放在会话上、按消息下标索引，与 `message_feedback` 同一套写法，好处是
    /// 不必给每个 `CodingMessage` 构造点补字段。
    ///
    /// 存在的理由是**不依赖模型自述**：写/改工具执行成功的那一刻宿主就知道动了
    /// 哪个文件、加删了多少行、首个 hunk 在第几行——比让模型回头回忆自己在回复里
    /// 列出文件名可靠得多。这份数据只面向界面，不进 LLM 上下文。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub message_changes: HashMap<usize, Vec<CodingFileChangeView>>,
    pub messages: Vec<CodingMessage>,
    pub status: CodingStatus,
    /// 最近更新时间，**毫秒**时间戳（与 `CodingMessage.timestamp` 一致）。
    ///
    /// 前端「按最近使用排序」与持久化时的保留策略都依赖它，单位必须统一：
    /// 早先创建/追加消息写的是**秒**、待办更新写的是**毫秒**，两者相差 1000 倍，
    /// 混在一起排序等于没排。旧数据在 `load_from_disk` 里会被归一化。
    pub updated_at: i64,
    /// 会话累计统计（轮数/步数/LLM 耗时/工具耗时/token 用量）
    #[serde(default)]
    pub stats: CodingStats,
    /// 最近一次 LLM 请求的真实上下文规模（API usage 上报的输入侧 token：
    /// input + cache_read + cache_write），自动压缩以此为触发依据
    #[serde(default)]
    pub last_context_tokens: u64,
    /// 最近一次请求的上下文构成估算（[system, 工具定义, 对话消息] 的 token），
    /// 供前端「上下文空间」组件展示 system/工具/对话 的占用占比。
    #[serde(default)]
    pub last_context_breakdown: [u64; 3],
    /// 会话上下文窗口（tokens，自动压缩阈值判定基准）
    ///
    /// 会话创建 / 切换工作模型时从配置解析（work_models[].context_window →
    /// ai.context_window → 厂商默认窗口）。
    #[serde(default = "default_context_window_tokens")]
    pub context_window: u64,
    /// 工作智能体待办独立清单（与陪伴智能体 todo 完全独立，随会话持久化）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work_todos: Vec<WorkTodo>,
    /// **本轮**被工具改动过的文件路径（按首次改动顺序去重）。
    ///
    /// 只在内存里跟踪、不持久化：轮次结束时由 [`CodingAgentService::finish_turn`]
    /// 消费成回复末尾的「修改文件」清单并清空。
    ///
    /// 存在的理由是**不依赖模型自述**：写/改工具执行成功的那一刻宿主就知道动了哪个
    /// 文件，比让模型回头回忆自己在回复里列出文件名可靠得多。
    #[serde(skip)]
    pub turn_changed_paths: Vec<String>,
}

/// 工作智能体待办项（独立于陪伴智能体的 todo 系统，随编程会话持久化）。
///
/// 刻意只有两个字段：清单走**整表替换**语义（`work_todo_write` 每次提交全量
/// 清单），条目不需要稳定 id——`content` 经去重校验后就是天然主键；`created_at`
/// 这类簿记字段在每次全量重写下没有任何意义。
///
/// `content` 带 `#[serde(default)]` 只是降级保护：早期持久化文件里该字段叫
/// `title`，缺 `content` 时条目在读取路径被丢弃，而不是让整个
/// coding_sessions.json 反序列化失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkTodo {
    #[serde(default)]
    pub content: String,
    /// 三态：pending（未开始）/ in_progress（进行中）/ completed（已完成）。
    /// 清单是工作智能体的"当前执行计划"，驱动每一轮工作推进（单 active 纪律）。
    #[serde(default = "default_work_todo_status")]
    pub status: String,
}

fn default_work_todo_status() -> String {
    "pending".to_string()
}

impl WorkTodo {
    pub fn is_completed(&self) -> bool {
        self.status == "completed"
    }

    pub fn is_in_progress(&self) -> bool {
        self.status == "in_progress"
    }

    /// 条目是否可用（content 非空且状态合法）——用于剔除旧格式残留的空条目。
    pub fn is_valid(&self) -> bool {
        !self.content.trim().is_empty() && WORK_TODO_STATUSES.contains(&self.status.as_str())
    }
}

/// 会话内一条文件变更（按文件聚合）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileChange {
    /// 文件绝对路径
    pub path: String,
    /// 会话内该文件累计新增行数（unified diff 的 '+' 行，不含 +++ 头）
    pub added: u64,
    /// 会话内该文件累计删除行数（unified diff 的 '-' 行，不含 --- 头）
    pub removed: u64,
    /// 该文件最近一次变更的 unified diff（edit_file 提供；write_file 无 diff 为空）
    pub diff: String,
}

/// 单条助手回复的改动清单项（宿主按工具执行结果生成，供前端渲染「修改文件」列表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingFileChangeView {
    /// 相对工作目录的路径（不在工作目录内时为原绝对路径），与文件链接协议一致
    pub path: String,
    /// 本轮累计新增行数
    #[serde(default)]
    pub added: u64,
    /// 本轮累计删除行数
    #[serde(default)]
    pub removed: u64,
    /// 首个变更所在行号（整文件写入无 diff 时为 None）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line: Option<u64>,
}

/// 取 unified diff 中首个 hunk 的新文件起始行（`@@ -a,b +c,d @@` 里的 c）。
fn first_changed_line(diff: &str) -> Option<u64> {
    for line in diff.lines() {
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some(after_plus) = rest.split('+').nth(1) else {
            continue;
        };
        let digits: String = after_plus.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(v) = digits.parse::<u64>() {
            return Some(v);
        }
    }
    None
}

/// 把绝对路径转成相对工作目录的路径（不在工作目录内时原样返回）。
///
/// 与提示词里的「文件链接协议」保持一致：会话里存相对路径，前端打开时再按
/// 工作目录拼回绝对路径。Windows 路径大小写不敏感，比较时忽略大小写。
fn relativize_to_workspace(path: &str, working_directory: &str) -> String {
    let wd = working_directory.replace('\\', "/");
    let wd = wd.trim_end_matches('/');
    if wd.is_empty() {
        return path.to_string();
    }
    let p = path.replace('\\', "/");
    let prefix = format!("{wd}/");
    let n = prefix.len();
    if p.len() > n && p.is_char_boundary(n) && p[..n].eq_ignore_ascii_case(&prefix) {
        return p[n..].to_string();
    }
    path.to_string()
}

/// 工作区路径比较键：统一分隔符、去掉尾部斜杠；Windows 下忽略大小写。
///
/// 只用于「是不是同一个目录」的判断与去重，不重写用户传入的写法 —— 用户从目录选择器
/// 拿到的路径原样存下来，避免展示层出现被改写过的路径。
///
/// 委派子 agent 时也用它比对「父会话是否真的拥有这个工作区」，所以是 pub。
pub fn workspace_key(path: &str) -> String {
    let normalized = path.trim().trim_end_matches(['/', '\\']).replace('/', "\\");
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

/// 消息被从头部裁掉 `removed` 条后，按下标索引的会话元数据整体前移。
///
/// 落在被裁区间内的条目直接丢弃（对应消息已经不在），其余下标减去 `removed`。
/// 用于 `message_feedback` / `message_changes` 这类以下标为键的元数据。
fn reindex_message_meta<T>(map: HashMap<usize, T>, removed: usize) -> HashMap<usize, T> {
    map.into_iter()
        .filter_map(|(k, v)| k.checked_sub(removed).map(|nk| (nk, v)))
        .collect()
}

/// 从 unified diff 统计增删行数（跳过 +++ / --- 头与 @@ hunk 行）。
fn diff_line_stats(diff: &str) -> (u64, u64) {
    let mut added = 0u64;
    let mut removed = 0u64;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// 合法工作待办状态。
pub const WORK_TODO_STATUSES: &[&str] = &["pending", "in_progress", "completed"];

/// 单条待办内容最大字数（一条 = 一个具体步骤，不是需求文档）。
const WORK_TODO_MAX_CONTENT_CHARS: usize = 200;

/// 单张清单的条数上限（清单是粗粒度执行计划，不是流水账）。
const WORK_TODO_MAX_ITEMS: usize = 30;

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

/// 沙箱确认回调：工具执行判定为「需要确认」时，由它裁决是否放行。
///
/// 按键因素两维分三种形态：
///
/// 1. **有工作区** → 恒放行。路径校验（限工作区）才是真正的边界，沙箱层「首次使用 /
///    前 N 次确认」再弹一次纯属重复打扰，所以这里直接放行。
/// 2. **无工作区 + 有应答者（主 agent）** → 返回 `None`，执行器会走前端确认弹窗。
///    无工作区就没有任何路径边界（`is_path_authorized` 恒真），写入必须真的经过用户同意。
///    子 agent 在下面单独处理——它面前没有人。
/// 3. **无工作区 + 无应答者（子 agent）** → 恒拒绝。弹窗发出去没人应答会把子任务挂死，
///    所以让它快速失败，好把「我需要写文件」写进最终回复交回上层。
///
/// 注意：第 2 种形态下 shell 类工具（`run_command` 等 Shell 风险）同样会走到这里——
/// 它们绕过路径校验，所以「无工作区时不许改文件」这条规则必须在这一层兜住，光靠路径校验不够。
pub(crate) fn coding_sandbox_confirm(
    has_workspace: bool,
    has_responder: bool,
) -> Option<CanUseTool> {
    static ALLOW: once_cell::sync::Lazy<CanUseTool> =
        once_cell::sync::Lazy::new(|| Arc::new(|_tool_name: &str, _args: &serde_json::Value| true));
    static DENY: once_cell::sync::Lazy<CanUseTool> =
        once_cell::sync::Lazy::new(|| Arc::new(|_tool_name: &str, _args: &serde_json::Value| false));
    match (has_workspace, has_responder) {
        (true, _) => Some(Arc::clone(&*ALLOW)),
        (false, true) => None,
        (false, false) => Some(Arc::clone(&*DENY)),
    }
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
        // 统一走 utils::fs —— 文件损坏时会把现场改名保留成 `.corrupt-<ts>` 再按
        // 空态继续，而不是只打一行日志：否则用户看到的是「会话凭空全没了」，
        // 现场也一并被下次写盘覆盖掉，无从排查。全项目其他状态文件都走这条路。
        let path = Self::store_path();
        let Some(mut list) = crate::utils::fs::load_json_or_backup::<Vec<CodingSession>>(&path) else {
            return;
        };
        // 兼容早期数据：`updated_at` 曾经秒/毫秒混用。秒级值（当前约 1.8e9）比毫秒级
        // 小三个数量级，不归一化的话它们会在排序里全部落到 1970 年附近，把「最近使用」
        // 彻底排错。1e11 毫秒 ≈ 1973 年，能干净地把两种单位分开。
        for s in list.iter_mut() {
            if s.updated_at > 0 && s.updated_at < 100_000_000_000 {
                s.updated_at *= 1000;
            }
        }
        // 启动恢复时所有会话重置为 Idle（上次运行中断的 Running 会话也回到空闲）
        let mut map = BTreeMap::new();
        for mut s in list {
            s.status = CodingStatus::Idle;
            map.insert(s.session_id.clone(), s);
        }
        *self.sessions.write() = map;
    }

    fn persist(&self) {
        // 按最近更新取前 N 个。`sessions` 是 BTreeMap，键是随机会话 id
        // （`code-<uuid>`），直接按容器顺序截断等于**随机**丢弃会话，
        // 与「保留最近 30 个」的意图不符。
        let mut sessions: Vec<CodingSession> = self.sessions.read().values().cloned().collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(PERSIST_MAX_SESSIONS);

        let Ok(text) = serde_json::to_string_pretty(&sessions) else {
            tracing::warn!("[CodingAgent] 会话序列化失败，跳过本次持久化");
            return;
        };
        // 原子写（同目录临时文件 + fsync + rename 替换）：进程崩溃或断电最多
        // 留下一个 `.tmp` 残骸，目标文件要么是旧内容要么是完整新内容，
        // 不会被截成半截 JSON 而丢掉全部会话。
        if let Err(e) = crate::utils::fs::write_atomic(&Self::store_path(), &text) {
            tracing::warn!("[CodingAgent] 会话持久化失败: {e}");
        }
    }

    /// 新建会话。
    pub fn create_session(&self, char_id: &str, working_directory: &str, mode: &str) -> CodingSession {
        let session = CodingSession {
            session_id: format!("code-{}", uuid::Uuid::new_v4().simple()),
            char_id: char_id.to_string(),
            working_directory: working_directory.to_string(),
            extra_workspaces: Vec::new(),
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
            file_changes: Vec::new(),
            message_feedback: HashMap::new(),
            message_changes: HashMap::new(),
            messages: Vec::new(),
            status: CodingStatus::Idle,
            updated_at: chrono::Utc::now().timestamp_millis(),
            stats: CodingStats::default(),
            last_context_tokens: 0,
            last_context_breakdown: [0, 0, 0],
            context_window: crate::providers::capabilities::default_context_window(""),
            work_todos: Vec::new(),
            turn_changed_paths: Vec::new(),
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

    /// 挂载附加工作区（目录必须存在；已挂载则更新其只读标记）。运行中拒绝。
    ///
    /// 主工作区与附加工作区取并集作为可访问范围；主工作区本身也是「已在授权范围内」，
    /// 重复挂主工作区直接返回错误，避免同一目录出现两份互相矛盾的权限记录。
    pub fn add_workspace(
        &self,
        session_id: &str,
        path: &str,
        read_only: bool,
    ) -> Result<Vec<ExtraWorkspace>, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("工作区路径不能为空".into());
        }
        if !std::path::Path::new(path).is_dir() {
            return Err(format!("工作区不存在: {path}"));
        }
        let key = workspace_key(path);
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                if workspace_key(&s.working_directory) == key {
                    return Err("该目录已是会话的主工作区".into());
                }
                match s.extra_workspaces.iter_mut().find(|w| workspace_key(&w.path) == key) {
                    Some(existing) => existing.read_only = read_only,
                    None => s.extra_workspaces.push(ExtraWorkspace {
                        path: path.to_string(),
                        read_only,
                    }),
                }
                let list = s.extra_workspaces.clone();
                drop(guard);
                self.persist();
                Ok(list)
            }
            Some(_) => Err("会话正在运行，不能修改工作区".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 卸载附加工作区。运行中拒绝。
    pub fn remove_workspace(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<Vec<ExtraWorkspace>, String> {
        let key = workspace_key(path);
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                s.extra_workspaces.retain(|w| workspace_key(&w.path) != key);
                let list = s.extra_workspaces.clone();
                drop(guard);
                self.persist();
                Ok(list)
            }
            Some(_) => Err("会话正在运行，不能修改工作区".into()),
            None => Err("会话不存在".into()),
        }
    }

    /// 切换附加工作区的只读标记（对主工作区无效，主工作区的可写性由访问级别决定）。
    pub fn set_workspace_read_only(
        &self,
        session_id: &str,
        path: &str,
        read_only: bool,
    ) -> Result<Vec<ExtraWorkspace>, String> {
        let key = workspace_key(path);
        let mut guard = self.sessions.write();
        match guard.get_mut(session_id) {
            Some(s) if s.status != CodingStatus::Running => {
                let Some(target) = s
                    .extra_workspaces
                    .iter_mut()
                    .find(|w| workspace_key(&w.path) == key)
                else {
                    return Err(format!("未挂载该工作区: {path}"));
                };
                target.read_only = read_only;
                let list = s.extra_workspaces.clone();
                drop(guard);
                self.persist();
                Ok(list)
            }
            Some(_) => Err("会话正在运行，不能修改工作区".into()),
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
        fork.extra_workspaces = base.extra_workspaces.clone();
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
    ///
    /// 与取消一样要回收会话名下的 pending 状态：pending 提问会让仍在 `await`
    /// 的工具永久挂住，未转告的完成报告则会在会话消失后仍被陪伴角色提起。
    pub fn delete_session(&self, session_id: &str) -> bool {
        let removed = self.sessions.write().remove(session_id).is_some();
        if removed {
            crate::brain::work_question::global_work_question_registry()
                .cancel_session(session_id);
            crate::brain::work_notices::global().drop_session(session_id);
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

    // ===== 工作智能体待办（独立于陪伴 todo 系统） =====

    /// 工作待办清单（独立存储于会话内，随 coding_sessions.json 持久化）。
    ///
    /// 读取路径统一丢弃非法条目（旧格式残留的 content 为空项）。
    pub fn list_work_todos(&self, session_id: &str) -> Result<Vec<WorkTodo>, String> {
        self.sessions
            .read()
            .get(session_id)
            .map(|s| s.work_todos.iter().filter(|t| t.is_valid()).cloned().collect())
            .ok_or_else(|| "会话不存在".to_string())
    }

    /// 整表替换写入工作待办清单（`work_todo_write` 工具与前端面板共用入口）。
    ///
    /// 每次提交的是**完整清单**，
    /// 没有局部更新、没有按下标的单条编辑。模型每调一次就得把整个计划重述
    /// 一遍——这正是清单能持续锚定"当前在做什么、下一步做什么"的原因，也是
    /// 它不至于退化成摆设的关键。
    ///
    /// 校验一律拒绝而非静默降级，让模型看到自己究竟写错了什么。
    pub fn write_work_todos(
        &self,
        session_id: &str,
        todos: Vec<WorkTodo>,
    ) -> Result<Vec<WorkTodo>, String> {
        if todos.len() > WORK_TODO_MAX_ITEMS {
            return Err(format!(
                "待办条数过多（{} 条，上限 {WORK_TODO_MAX_ITEMS}）：清单应是粗粒度执行步骤，不是流水账",
                todos.len()
            ));
        }
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut active = 0usize;
        let mut out: Vec<WorkTodo> = Vec::with_capacity(todos.len());
        for mut t in todos {
            let content = t.content.trim().to_string();
            if content.is_empty() {
                return Err("待办内容不能为空".to_string());
            }
            if content.chars().count() > WORK_TODO_MAX_CONTENT_CHARS {
                return Err(format!(
                    "待办内容过长（上限 {WORK_TODO_MAX_CONTENT_CHARS} 字）：一条待办是一个具体步骤，请精简"
                ));
            }
            if !seen.insert(content.clone()) {
                return Err(format!("待办内容重复：{content}"));
            }
            if !WORK_TODO_STATUSES.contains(&t.status.as_str()) {
                return Err(format!(
                    "非法状态 {}（可选：{}）",
                    t.status,
                    WORK_TODO_STATUSES.join(" / ")
                ));
            }
            // 单 active 纪律：顺序执行的编码工作不应对同一批步骤多头并进
            if t.status == "in_progress" {
                active += 1;
            }
            t.content = content;
            out.push(t);
        }
        if active > 1 {
            return Err(format!(
                "至多一项待办处于进行中（in_progress），当前 {active} 项：完成当前项再开下一项"
            ));
        }
        let mut guard = self.sessions.write();
        let session = guard.get_mut(session_id).ok_or("会话不存在")?;
        session.work_todos = out.clone();
        session.updated_at = chrono::Utc::now().timestamp_millis();
        drop(guard);
        self.persist();
        Ok(out)
    }

    /// 新一轮对话开始时的清单维护：**全部 completed 才清空**，否则跨轮保留。
    ///
    /// 与"每轮开始无条件清空"的做法不同——这里允许清单跨轮
    /// 延续（长任务常常一轮推不完）。只在清单已全部完成时归档掉，否则一张
    /// 完成的旧清单会长期挂在上下文里，逐渐变成没人看的噪音。
    pub fn maybe_clear_completed_plan(&self, session_id: &str) {
        let mut guard = self.sessions.write();
        let Some(session) = guard.get_mut(session_id) else {
            return;
        };
        if session.work_todos.is_empty() || !session.work_todos.iter().all(|t| t.is_completed()) {
            return;
        }
        session.work_todos.clear();
        session.updated_at = chrono::Utc::now().timestamp_millis();
        drop(guard);
        self.persist();
    }

    /// 清单已建立且全部完成——agent loop 据此注入收尾引导。
    pub fn plan_all_completed(&self, session_id: &str) -> bool {
        match self.sessions.read().get(session_id) {
            Some(s) => !s.work_todos.is_empty() && s.work_todos.iter().all(|t| t.is_completed()),
            None => false,
        }
    }

    /// 把工作待办清单渲染成注入上下文的"当前执行计划"文本。
    ///
    /// 每轮 LLM 请求的 system 里都会带上这段，所以清单无需模型主动回读。
    pub fn render_work_plan(todos: &[WorkTodo]) -> String {
        let valid: Vec<&WorkTodo> = todos.iter().filter(|t| t.is_valid()).collect();
        if valid.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::with_capacity(valid.len());
        for t in &valid {
            let (mark, label) = match t.status.as_str() {
                "in_progress" => ("▶", "进行中"),
                "completed" => ("[x]", "已完成"),
                _ => ("[ ]", "未开始"),
            };
            lines.push(format!("{mark} {label} {}", t.content));
        }
        let cnt = |s: &str| valid.iter().filter(|t| t.status == s).count();
        format!(
            "未开始 {} / 进行中 {} / 已完成 {}\n{}\n\
             按这份清单推进：开始某一步就标 in_progress（同时至多一项），完成一步立刻标 completed，不要攒着批量标。\n\
             用 work_todo_write 更新时必须提交**完整清单**——整表替换，没有局部修改。\n\
             计划中途有变（新增/拆分/合并/放弃步骤）就重写整张表，不要将就旧清单。\n\
             全部 completed 后直接给出最终总结收尾。",
            cnt("pending"),
            cnt("in_progress"),
            cnt("completed"),
            lines.join("\n")
        )
    }

    /// 取消正在运行的会话（下一轮循环前生效）。
    ///
    /// 一并撤掉该会话挂起中的提问：否则 `work_ask_user` 的 await 没人唤醒，
    /// loop 又卡在它上面，会话会永远停在 Running，连新消息都发不进来。
    pub fn cancel(&self, session_id: &str) -> bool {
        let canceled = {
            let mut guard = self.sessions.write();
            match guard.get_mut(session_id) {
                Some(s) if s.status == CodingStatus::Running => {
                    s.status = CodingStatus::Canceled;
                    true
                }
                _ => false,
            }
        };
        if canceled {
            crate::brain::work_question::global_work_question_registry()
                .cancel_session(session_id);
            crate::brain::work_jobs::global_work_job_registry().cancel_session(session_id);
            // 会话已取消，还没转告给用户的完成报告不该再被提起
            crate::brain::work_notices::global().drop_session(session_id);
        }
        canceled
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
                s.updated_at = chrono::Utc::now().timestamp_millis();
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
                widgets: None,
                interjected: None,
                guided: None,
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

    /// 智能体向会话推送可视化组件消息（show_widget 工具调用）。
    ///
    /// 组件作为 assistant 消息追加进会话（widgets 内联原始 SVG，随会话持久化，
    /// 恢复会话时前端直接重渲染），并广播 `coding:assistant_message`（携带
    /// widgets）供编程页实时渲染。caption 为可选说明文本（可为空）。
    pub fn push_agent_widget(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        widgets: Vec<CodingWidget>,
        caption: &str,
    ) -> Result<(), String> {
        if widgets.is_empty() {
            return Err("组件列表为空".to_string());
        }
        if !self.has_session(session_id) {
            return Err("会话不存在".to_string());
        }
        self.push_message(
            session_id,
            CodingMessage {
                role: CodingRole::Assistant,
                content: caption.to_string(),
                images: None,
                file_refs: None,
                widgets: Some(widgets.clone()),
                interjected: None,
                guided: None,
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
                "widgets": widgets,
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

    /// 记录/更新会话内文件变更（按文件聚合：累计增删行数，diff 保留最近一次变更），
    /// 同时标记该文件属于本轮改动。
    fn record_file_change(&self, session_id: &str, path: &str, added: u64, removed: u64, diff: &str) {
        let mut guard = self.sessions.write();
        let Some(s) = guard.get_mut(session_id) else {
            return;
        };
        if let Some(existing) = s.file_changes.iter_mut().find(|c| c.path == path) {
            existing.added += added;
            existing.removed += removed;
            if !diff.trim().is_empty() {
                existing.diff = diff.to_string();
            }
        } else {
            s.file_changes.push(FileChange {
                path: path.to_string(),
                added,
                removed,
                diff: diff.to_string(),
            });
        }
        if !s.turn_changed_paths.iter().any(|p| p == path) {
            s.turn_changed_paths.push(path.to_string());
        }
    }

    /// 工具执行成功后登记文件改动：写/改类工具更新会话产物与文件变更清单，
    /// 并标记该文件属于本轮。
    ///
    /// 由**工具执行点**统一调用（标准循环 / code 编排模式 / 子智能体），
    /// 是「回复末尾会列出哪些文件被改」这件事的唯一事实来源——
    /// 不依赖模型在正文里自述，因为让模型回忆自己改过什么并不可靠。
    pub fn record_tool_file_change(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        result: &crate::tools::types::ToolResult,
    ) {
        if !matches!(tool, "write_file" | "edit_file") {
            return;
        }
        let Some(path) = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            return;
        };
        if self.record_deliverable(session_id, path) {
            if let Some(app) = app {
                let _ = app.emit(
                    "coding:deliverable",
                    serde_json::json!({
                        "session_id": session_id,
                        "path": path,
                    }),
                );
            }
        }
        // 工具结果统一是 standard_success 信封 `{ data: {...}, message, error, success }`，
        // unified diff 在内层 `data` 里。此处曾直接读顶层 `result.data["diff"]`——
        // 永远取不到，导致「变更」页对所有工具都显示「无 diff」。按真实层级取。
        let diff = result
            .data
            .as_ref()
            .and_then(|d| d.get("data"))
            .and_then(|d| d.get("diff"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // diff 缺失（结果被预算截断、内容无变化等）时退化为「以写入内容行数计为新增」
        let (added, removed) = if diff.trim().is_empty() {
            let lines = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.lines().count() as u64)
                .unwrap_or(0);
            (lines, 0)
        } else {
            diff_line_stats(&diff)
        };
        self.record_file_change(session_id, path, added, removed, &diff);
    }

    /// 把本轮改动汇总成前端可直接渲染的清单并清空本轮标记。
    ///
    /// 行区间取该文件**本轮最后一次**变更的 diff 首个 hunk；整文件写入没有 diff，
    /// 行号留空由前端只显示文件名（编造行号比不给更糟）。
    fn take_turn_changed_files(&self, session_id: &str) -> Vec<CodingFileChangeView> {
        let mut guard = self.sessions.write();
        let Some(s) = guard.get_mut(session_id) else {
            return Vec::new();
        };
        let paths = std::mem::take(&mut s.turn_changed_paths);
        let wd = s.working_directory.clone();
        paths
            .iter()
            .filter_map(|p| {
                let change = s.file_changes.iter().find(|c| &c.path == p)?;
                Some(CodingFileChangeView {
                    path: relativize_to_workspace(&change.path, &wd),
                    added: change.added,
                    removed: change.removed,
                    line: first_changed_line(&change.diff),
                })
            })
            .collect()
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

    /// 估算最近一次请求的上下文构成（system / 工具定义 / 对话消息），写入会话供前端
    /// 「上下文空间」组件展示各部分的占用占比。估算非计费依据，仅作状态参考。
    fn update_context_breakdown(
        &self,
        session_id: &str,
        messages: &[ChatMessage],
        definitions: &[ToolDefinition],
    ) {
        let mut system_tokens = 0usize;
        let mut message_tokens = 0usize;
        for m in messages {
            let t = crate::utils::token_estimate::estimate_message_tokens(m);
            if m.role == "system" {
                system_tokens += t;
            } else {
                message_tokens += t;
            }
        }
        let tools_tokens = crate::utils::token_estimate::estimate_tool_definitions_tokens(
            &definitions
                .iter()
                .map(|d| (d.name.clone(), d.description.clone(), d.parameters.to_string()))
                .collect::<Vec<_>>(),
        );
        let mut guard = self.sessions.write();
        if let Some(s) = guard.get_mut(session_id) {
            s.last_context_breakdown = [
                system_tokens as u64,
                tools_tokens as u64,
                message_tokens as u64,
            ];
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
        guided: bool,
    ) -> Result<(), String> {
        // 会话存在性 + 状态检查（Running 时拒绝新消息，前端按钮已禁用，这里兜底）
        let (working_directory, extra_workspaces) = {
            let mut guard = self.sessions.write();
            let s = guard.get_mut(&session_id).ok_or("会话不存在")?;
            if s.status == CodingStatus::Running {
                return Err("会话正在处理上一条消息".into());
            }
            s.status = CodingStatus::Running;
            // 新一轮开始：清空上一轮的改动标记（上一轮若异常中断会残留）
            s.turn_changed_paths.clear();
            let first = s.title.is_empty();
            // 斜杠命令不作为会话标题
            if first && !text.trim_start().starts_with('/') {
                let t: String = text.chars().take(30).collect();
                s.title = t;
            }
            (s.working_directory.clone(), s.extra_workspaces.clone())
        };
        // 文件引用：解析路径并读取内容（工作区归属校验 + 数量/长度上限）
        let resolved_refs = resolve_file_refs(&working_directory, &extra_workspaces, file_refs);

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
                widgets: None,
                interjected: if interjected { Some(true) } else { None },
                guided: if guided { Some(true) } else { None },
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
        // 新一轮对话：清单全部完成则归档清空，否则跨轮保留继续推进
        self.maybe_clear_completed_plan(&session_id);

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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
                crate::providers::base::TASK_WORK_AGENT,
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
                // 按下标索引的消息级元数据必须跟着前移，否则压缩一次之后反馈与
                // 「修改文件」清单就会挂到错误的回复上（越靠后的消息错得越离谱）。
                s.message_feedback = reindex_message_meta(std::mem::take(&mut s.message_feedback), split);
                s.message_changes = reindex_message_meta(std::mem::take(&mut s.message_changes), split);
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

    /// /memory：项目记忆管理（存储在工作区 `.vivian/memory.md`，项目级随项目走）。
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
                CodingRole::Notice => {
                    md.push_str(&format!("> {}\n\n", m.content));
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
        let (char_id, working_directory, extra_workspaces, mode, permission, reasoning_level) = {
            let guard = self.sessions.read();
            match guard.get(session_id) {
                Some(s) => (
                    s.char_id.clone(),
                    s.working_directory.clone(),
                    s.extra_workspaces.clone(),
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
                &extra_workspaces,
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
        }
        // 附加工作区随会话一起下传：沙箱与权限层据此放开这些目录的读写
        .with_extra_working_directories(
            extra_workspaces.iter().map(|w| (w.path.clone(), w.read_only)),
        );

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
        // 清单全部完成后只提示一次收尾，避免每轮重复注入同一句提醒
        let mut plan_done_hinted = false;

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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
            // 上下文构成估算：写会话供前端「上下文空间」展示 system/工具/对话 占比
            self.update_context_breakdown(session_id, &messages, &definitions);
            // 后台子任务结算：任务终结后主动把结果送进上下文——模型不必记得回查，
            // 它很可能压根不再调用工具就结束本轮，那样结果就白白丢了。
            for settled in crate::brain::work_jobs::global_work_job_registry()
                .drain_settlements(session_id)
            {
                messages.push(ChatMessage::system(&crate::brain::work_jobs::render_settlement(
                    &settled,
                )));
            }
            // 清单已全部完成但循环仍在继续：提醒一次收尾，避免模型无视计划继续空转
            if !plan_done_hinted && self.plan_all_completed(session_id) {
                plan_done_hinted = true;
                messages.push(ChatMessage::system(
                    "[系统提示] 你的工作待办已全部 completed，但本轮任务尚未结束。\
                     若确实已完成，请直接给出最终总结，不要再调用工具；\
                     若还有剩余工作，用 work_todo_write 重建清单后再继续。",
                ));
            }

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
            // 流式 native FC：DeepSeek V4 Flash 偶发"finish_reason=tool_calls 但 SSE delta 无 tool_calls 数据"，
            // 识别后自动重试（最多 3 次），仍失败则明确报错收尾，而不是静默当成"最终回复"断在一半。
            const MAX_STREAM_ATTEMPTS: usize = 3;
            let mut streamed_text = String::new();
            let mut call_buf: BTreeMap<usize, (String, String, String)> = BTreeMap::new(); // index -> (id, name, args)
            let mut step_usage: Option<CodingTokenUsage> = None;
            let mut first_token_tracked = false;
            let mut done_finish_reason: Option<String> = None;
            let mut calls: Vec<MessageToolCall> = Vec::new();

            'stream_attempt: for attempt in 1..=MAX_STREAM_ATTEMPTS {
                let mut attempt_msgs = messages.clone();
                if attempt >= 2 {
                    attempt_msgs.push(ChatMessage::system(
                        "【系统指令】你上一次声明要调用工具，但调用数据在流式传输中丢失。请这次务必通过 function calling 发出真实的工具调用，不要用“调用工具：…”这类文字描述来代替。",
                    ));
                }
                let mut attempt_req =
                    LLMRequest::new(crate::providers::base::TASK_WORK_AGENT, attempt_msgs).with_tools(definitions.clone())
                    .with_character_id(char_id.clone());
                // 推理等级：low 关闭思维链，medium/high 按档位开启（按模型能力映射 wire 字段）
                attempt_req.reasoning = reasoning_level_to_pref(&reasoning_level);
                let mut event_rx = match router.generate_stream_with_tools(attempt_req).await {
                    Ok(rx) => rx,
                    Err(e) => {
                        self.report_llm_error(&app, session_id, "LLM 调用", &e.to_string());
                        self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                        return;
                    }
                };

                streamed_text.clear();
                call_buf.clear();
                done_finish_reason = None;
                let mut attempt_usage: Option<CodingTokenUsage> = None;

                // 消费流，累积文本增量（逐字转发前端打字机）+ 工具调用增量
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
                            attempt_usage = Some(CodingTokenUsage {
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
                        StreamEvent::Done { finish_reason } => {
                            done_finish_reason = finish_reason;
                            break;
                        }
                        StreamEvent::Error { message } => {
                            self.report_llm_error(&app, session_id, "LLM 流式响应", &message);
                            self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                            return;
                        }
                    }
                }
                if attempt_usage.is_some() {
                    step_usage = attempt_usage;
                }

                // 解析工具调用（参数 JSON 字符串 → Value）
                calls = call_buf
                    .iter()
                    .map(|(_idx, (id, name, args))| {
                        let arguments = if args.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::from_str(args.trim())
                                .unwrap_or_else(|_| serde_json::Value::String(args.clone()))
                        };
                        MessageToolCall { id: id.clone(), name: name.clone(), arguments }
                    })
                    .collect();

                // 重试判定：声明要调用工具却收不到任何调用数据 → 换消息重试
                if calls.is_empty()
                    && done_finish_reason.as_deref() == Some("tool_calls")
                    && attempt < MAX_STREAM_ATTEMPTS
                {
                    tracing::warn!(
                        "[CodingAgent] 第 {} 次流式工具调用数据丢失（finish_reason=tool_calls 但 0 个调用），自动重试",
                        attempt
                    );
                    continue 'stream_attempt;
                }
                break 'stream_attempt;
            }

            // 重试仍失败：声明工具调用却始终收不到 → 明确报错，不再静默当"最终回复"断在一半
            if calls.is_empty() && done_finish_reason.as_deref() == Some("tool_calls") {
                let m = "模型声明要调用工具，但调用数据在流式传输中多次丢失（DeepSeek V4 Flash 偶发问题）。本轮已停止，可发送“继续”重试。";
                self.push_error(session_id, m);
                let _ = app.emit(
                    "coding:error",
                    serde_json::json!({ "session_id": session_id, "message": m }),
                );
                self.finish_turn(app.clone(), session_id, CodingStatus::Idle);
                return;
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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
                // 任务完成 → 把这条事实登记给陪伴角色，由它自己决定要不要说。
                // 仅在最终回复是真实收尾文本时才登记：模型若把工具调用写成
                // "调用工具：…"文字且未实际调用，不谎报完成，也不打扰陪伴角色。
                if made_progress {
                    let trimmed = content.trim();
                    let looks_tool_annotation = trimmed.starts_with("调用工具") || trimmed.starts_with("调用");
                    if !trimmed.is_empty() && !looks_tool_annotation {
                        let summary = trimmed.chars().take(120).collect::<String>();
                        self.report_work_completion(session_id, "任务完成", &summary);
                    } else if looks_tool_annotation {
                        tracing::warn!(
                            "[CodingAgent:{}] 最终回复疑似工具调用文本但无实际调用，跳过“任务完成”登记",
                            session_id
                        );
                    }
                }
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
                    execute_tool_use(
                        &call.name,
                        call.arguments.clone(),
                        &tool_system,
                        &tool_ctx,
                        // 主 agent 面前有用户：无工作区时让确认真的弹出来
                        coding_sandbox_confirm(!working_directory.trim().is_empty(), true),
                    )
                        .await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                self.stats_tool_done(session_id, duration_ms);
                let (ok, summary) = if result.success {
                    // write_file 的 diff 只服务界面「变更」页：内容本就是模型刚写出的，
                    // 再作为工具结果回传纯属重复计费 → 回传前摘掉（edit_file 的 diff 照旧保留）。
                    let data = match &result.data {
                        Some(d) if call.name == "write_file" => {
                            let mut payload = d.clone();
                            if let Some(inner) =
                                payload.get_mut("data").and_then(serde_json::Value::as_object_mut)
                            {
                                inner.remove("diff");
                            }
                            serde_json::to_string(&payload).unwrap_or_default()
                        }
                        Some(d) => serde_json::to_string(d).unwrap_or_default(),
                        None => serde_json::to_string(&serde_json::Value::Null).unwrap_or_default(),
                    };
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
                    // 写/改文件成功 → 登记会话产物、文件变更与本轮改动标记。
                    // 统一走 record_tool_file_change，code 模式与子智能体共用同一份事实。
                    self.record_tool_file_change(
                        Some(&app),
                        session_id,
                        &call.name,
                        &call.arguments,
                        &result,
                    );
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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
                        widgets: None,
                        interjected: None,
                        guided: None,
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
                widgets: None,
                interjected: None,
                guided: None,
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
        // 本轮改动清单挂到「本轮最后一条用户消息之后的那条助手消息」上。
        // 由宿主按工具实际执行结果生成，不依赖模型在正文里自述改了哪些文件。
        let changed = self.take_turn_changed_files(session_id);
        {
            let mut guard = self.sessions.write();
            if let Some(s) = guard.get_mut(session_id) {
                // Canceled 保留标记（前端可感知），下次发消息时重置为 Running
                s.status = status;
                if !changed.is_empty() {
                    let start = s
                        .messages
                        .iter()
                        .rposition(|m| m.role == CodingRole::User)
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let target = (start..s.messages.len())
                        .rev()
                        .find(|&i| s.messages[i].role == CodingRole::Assistant);
                    if let Some(idx) = target {
                        s.message_changes.insert(idx, changed);
                    }
                }
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
                widgets: None,
                interjected: None,
                guided: None,
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
        extra_workspaces: &[ExtraWorkspace],
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
        }
        .with_extra_working_directories(
            extra_workspaces.iter().map(|w| (w.path.clone(), w.read_only)),
        );
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
        let mut req = LLMRequest::new(crate::providers::base::TASK_WORK_AGENT, messages)
            .with_character_id(char_id.to_string());
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
                widgets: None,
                interjected: None,
                guided: None,
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
                execute_tool_use(
                    &tool,
                    arguments.clone(),
                    tool_system,
                    &tool_ctx,
                    coding_sandbox_confirm(!working_directory.trim().is_empty(), true),
                )
                .await
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
            // 与标准循环共用同一份改动登记：code 模式不改的话，「变更」面板与回复末尾的
            // 「修改文件」清单都会漏掉编排模式做出的改动。
            if result.success {
                self.record_tool_file_change(
                    Some(&app),
                    session_id,
                    &tool,
                    &arguments,
                    &result,
                );
            }
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
                    widgets: None,
                    interjected: None,
                    guided: None,
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
                    widgets: None,
                    interjected: None,
                    guided: None,
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
                    widgets: None,
                    interjected: None,
                    guided: None,
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
            )
            .with_character_id(session.char_id.clone()))
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
        self.push_host_message(session_id, CodingRole::Error, message);
    }

    /// 记录一条宿主自产的中性状态提示（非错误），随会话持久化。
    fn push_notice(&self, session_id: &str, message: &str) {
        self.push_host_message(session_id, CodingRole::Notice, message);
    }

    /// 宿主消息（错误 / 状态提示）的统一落库入口：两者字段构成完全一致，
    /// 只有角色不同，前端据此决定红色警示还是中性提示。
    fn push_host_message(&self, session_id: &str, role: CodingRole, message: &str) {
        self.push_message(
            session_id,
            CodingMessage {
                role,
                images: None,
                content: message.to_string(),
                file_refs: None,
                widgets: None,
                interjected: None,
                guided: None,
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
        // 登记给陪伴角色：只传事实（错误文案 + 本次任务未完成），
        // 怎么说是 TA 的事，不在这里拼固定话术。
        let notice = format!("{} 本次工作任务未完成，可稍后重试。", class.user_message);
        self.report_work_completion(session_id, "模型调用失败", &notice);
    }

    /// 登记一条工作事实，交给陪伴角色自行决定要不要向用户提及。
    ///
    /// 这里**不生成文案、不直投气泡**：陪伴角色有自己完整的主动交互流程
    /// （说话欲望、安静模式、是否轮到它开口、反重复），绕过它硬播会让角色
    /// 说话时突然不像自己。登记后由陪伴侧的 `background_tasks` 提示词段落
    /// 自然取用。
    fn report_work_completion(&self, session_id: &str, title: &str, body: &str) {
        let char_id = self
            .sessions
            .read()
            .get(session_id)
            .map(|s| s.char_id.clone())
            .unwrap_or_default();
        if char_id.is_empty() {
            return;
        }
        if crate::brain::work_notices::global().push_report(&char_id, session_id, title, body) {
            tracing::debug!(
                "[CodingAgent:{}] 已登记工作完成报告，待陪伴角色自然提及：{}",
                session_id,
                title
            );
        }
    }

    /// 估算「本轮即将发送的请求」的输入侧 token。
    ///
    /// system 与工具定义沿用上一轮已算好的构成分量（两者在轮次之间基本稳定，
    /// 且此刻还没组装新请求），对话历史按当前会话实时估算。
    /// 用于压缩预检，补上「只看上一轮上报值」的滞后。
    fn estimate_pending_context(s: &CodingSession) -> u64 {
        let [system_tokens, tools_tokens, _] = s.last_context_breakdown;
        let start = s.messages.len().saturating_sub(MAX_HISTORY_MESSAGES);
        let history_tokens: usize = s.messages[start..]
            .iter()
            .map(|m| crate::utils::token_estimate::estimate_tokens(&m.content))
            .sum();
        system_tokens + tools_tokens + history_tokens as u64
    }

    /// 智能压缩入口：取「上一次 LLM 请求 API 上报的真实输入侧 token」与
    /// 「本轮即将发送内容的估算 token」的较大值为依据，达到会话窗口上限的
    /// [`AUTO_COMPACT_THRESHOLD_PCT`]% 时把早期历史归档为摘要，
    /// 防止多轮工具调用的历史累积超出模型上下文窗口。
    ///
    /// 只看上报值会滞后一轮：单轮内新增超大内容（例如一次读入大文件）时，
    /// 检查发生在这个请求之前、用的还是上一轮的数字，可能一轮就顶穿窗口，
    /// 因此叠加 [`Self::estimate_pending_context`] 做预检。
    ///
    /// 事件契约：开始前发 `coding:compact_start`（前端显示「正在压缩上下文」），
    /// 无论成败都发 `coding:compact_end`（前端收起该状态），
    /// 真正归档了才落一条 `Notice` 并广播 `coding:notice`。
    /// 压缩失败仅记日志，不影响本轮执行。返回是否实际执行了压缩。
    async fn maybe_auto_compact(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        router: &ModelRouter,
    ) -> bool {
        // 消息量不足时必然无法压缩，直接跳过
        let (enough, count, last_ctx, estimated, window) = {
            let guard = self.sessions.read();
            guard
                .get(session_id)
                .map(|s| {
                    (
                        s.messages.len() > COMPACT_KEEP_MESSAGES + COMPACT_MIN_MESSAGES,
                        s.messages.len(),
                        s.last_context_tokens,
                        Self::estimate_pending_context(s),
                        s.context_window,
                    )
                })
                .unwrap_or((false, 0, 0, 0, 0))
        };
        if !enough || window == 0 {
            return false;
        }
        // 两条触发线，任一成立都要归档：
        // 1) token 逼近窗口上限；
        // 2) 历史长到会被 build_llm_messages 按 MAX_HISTORY_MESSAGES 裁剪。
        //    第 2 条不能少——大窗口模型下 token 可能长期远低于阈值，若只按 token
        //    判断，超出的消息会被无声丢掉，「归档为摘要」的承诺就不成立了。
        let used = estimated.max(last_ctx);
        let over_window = used > 0 && used * 100 >= window * AUTO_COMPACT_THRESHOLD_PCT;
        let over_history_cap = count > MAX_HISTORY_MESSAGES;
        if !over_window && !over_history_cap {
            return false;
        }
        let pct = if window == 0 { 0 } else { (used * 100 / window).min(100) };

        let _ = app.emit(
            "coding:compact_start",
            serde_json::json!({
                "session_id": session_id,
                "used": used, "window": window, "percent": pct,
                "reason": if over_window { "window" } else { "history_cap" },
            }),
        );
        let outcome = self.compact_history(session_id, router).await;
        let (archived, ok) = match &outcome {
            Ok(o) => (o.archived, true),
            Err(e) => {
                tracing::warn!("[CodingAgent] 上下文自动压缩失败: {e}");
                (0, false)
            }
        };
        let _ = app.emit(
            "coding:compact_end",
            serde_json::json!({
                "session_id": session_id, "ok": ok, "archived": archived,
                "used": used, "window": window, "percent": pct,
                "reason": if over_window { "window" } else { "history_cap" },
            }),
        );
        if archived == 0 {
            return false;
        }

        let notice = if over_window {
            format!("上下文占用已达窗口的 {pct}%，已自动把 {archived} 条早期历史压缩为摘要。")
        } else {
            format!("历史消息已达 {count} 条，已自动把 {archived} 条早期历史压缩为摘要。")
        };
        self.push_notice(session_id, &notice);
        let _ = app.emit(
            "coding:notice",
            serde_json::json!({
                "session_id": session_id, "kind": "compact", "message": notice,
                "archived": archived, "used": used, "window": window, "percent": pct,
                "reason": if over_window { "window" } else { "history_cap" },
            }),
        );
        true
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

        let mut system = Self::system_prompt(
            char_id,
            &session.working_directory,
            &session.extra_workspaces,
            mode,
        );
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
        // 项目记忆注入：跨会话约定与教训（工作区 .vivian/memory.md，每轮重读，
        // /memory 修改后下一轮即时生效；文件未变时 system prompt 字节一致，不影响缓存）
        if let Some(mem) = read_project_memory(&session.working_directory) {
            system.push_str(&format!(
                "\n\n# 项目记忆（跨会话沉淀）\n\
                 以下是此前会话沉淀的本项目约定与经验教训，默认遵循其中约定（用户当轮指示优先）：\n{mem}"
            ));
        }
        // 工作待办注入为"当前执行计划"：清单驱动每轮工作推进（三态 + 单 active 纪律），
        // 与 work_todo_write 工具联动；清单非空才有内容，不影响缓存前缀。
        // 空清单（render_work_plan 返回空串）也跳过，避免注入无意义的空标题。
        let plan = Self::render_work_plan(&session.work_todos);
        if !plan.is_empty() {
            system.push_str(&format!("\n\n# 工作待办（当前执行计划）\n{plan}"));
        }
        let mut messages = vec![ChatMessage::system(system)];
        // 历史裁剪：保留最近 MAX_HISTORY_MESSAGES 条。
        // 正常路径下超限会先被 maybe_auto_compact 归档成摘要，这里不该真的裁掉东西；
        // 一旦裁了说明压缩没生效（失败或未触发），留 warning 便于定位。
        let start = session.messages.len().saturating_sub(MAX_HISTORY_MESSAGES);
        if start > 0 {
            tracing::warn!(
                "[CodingAgent] 会话历史超出 {MAX_HISTORY_MESSAGES} 条上限，兜底裁掉最早 {start} 条（未经摘要）"
            );
        }
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
                    // 用户给出的"引导"：加引导标注，提示模型这是对当前工作的引导/指示而非全新任务
                    if msg.guided == Some(true) {
                        user_text = format!(
                            "[系统标注] 用户在你工作期间给出了引导，这是对你当前工作的引导/指示（不是全新任务），请理解其意图并在后续工作中遵循：\n<user_message>\n{}\n</user_message>",
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
                // Notice 只面向用户界面，不回传 LLM：压缩结果已通过
                // `session.compacted` 注入 system prompt，重复回传纯属噪声
                CodingRole::Notice => {}
            }
        }
        messages
    }

    /// 编程智能体 system prompt（按模式差异化）。
    fn system_prompt(
        char_id: &str,
        working_directory: &str,
        extra_workspaces: &[ExtraWorkspace],
        mode: &str,
    ) -> String {
        let persona = match char_id {
            "nana" => "你是 Nana，一位温柔的编程助手。语气轻柔友好，但技术内容严谨准确。",
            _ => "你是 Vivian，一位反应快、爱吐槽但极其靠谱的编程助手。语气自然随意，但代码和结论必须严谨。",
        };
        // 范围纪律：见 SCOPE_DISCIPLINE 注释（防「悄悄把活干小」）
        let scope = SCOPE_DISCIPLINE;
        // 无工作区模式：会话未绑定目录（如陪伴侧派发的轻量任务）。
        // 此时文件工具用绝对路径操作，不设工作目录沙箱；写入/执行仍受
        // 权限矩阵管控（workspace_write 级下命令与文件写入会请求用户确认）。
        let env = if working_directory.trim().is_empty() {
            "# 工作环境\n\
             - 操作系统：Windows（命令用 PowerShell 语法）\n\
             - 工作目录：未选择（无工作区模式）\n\
             - 文件读取使用绝对路径，没有目录限制；\n\
             - **但本会话没有可写工作区：写入文件与执行命令一律需要用户逐次确认**，不要因为被拒绝就反复重试\n\
             - 需要固定工作区（免确认读写）时，请用户在工作页的会话标题旁点工作区芯片挂载一个目录"
                .to_string()
        } else {
            // 附加工作区：主工作区之外的授权目录（可读写，标注只读的除外）。
            // 必须显式列给模型，否则它只看到主工作区就会主动回避已授权的目录。
            let extras = if extra_workspaces.is_empty() {
                String::new()
            } else {
                let mut s = String::from("- 附加工作区（同样在授权范围内，相对路径不适用，需用绝对路径）：\n");
                for w in extra_workspaces {
                    s.push_str(&format!(
                        "  - {}{}\n",
                        w.path,
                        if w.read_only { "（只读）" } else { "" }
                    ));
                }
                s
            };
            format!(
                "# 工作环境\n\
                 - 操作系统：Windows（命令用 PowerShell 语法）\n\
                 - 主工作目录：{wd}\n\
                 - 所有文件路径操作仅限主工作目录与上述附加工作区内（沙箱强制）\n\
                 {extras}\
                 - 相对路径一律以主工作目录为基准",
                wd = working_directory,
                extras = extras,
            )
        };
        // 文件链接协议：有工作目录时一律走相对路径——绝对路径会把
        // `G:/project/src/...` 这类前缀反复写进会话历史（每个链接几十字符，
        // 多轮累积可观），而且工作目录一变、或换平台后历史里的链接全部失效。
        // 相对路径由前端按当前工作目录还原成可点击的绝对路径。
        // 无工作目录模式下没有解析基准，只能退回绝对路径。
        let link_protocol = if working_directory.trim().is_empty() {
            "- 文件链接协议：提到本地文件时使用 Markdown 链接 `[显示名 (line N)](绝对路径:N)`。\
             本会话未绑定工作目录，只能给绝对路径；路径使用正斜杠，行号为 1-based，\
             知道准确列号时可写成 `路径:行号:列号`。"
        } else if extra_workspaces.is_empty() {
            "- 文件链接协议：提到本地文件时使用 Markdown 链接 `[显示名 (line N)](相对工作目录的路径:N)`，\
             例如 `[SourceFileView.tsx (line 115)](src/components/mind-inspector/pages/SourceFileView.tsx:115)`。\
             路径相对于当前工作目录、使用正斜杠，不要写盘符或绝对路径；行号为 1-based，\
             知道准确列号时可写成 `路径:行号:列号`。"
        } else {
            "- 文件链接协议：主工作目录内的文件用 `[显示名 (line N)](相对主工作目录的路径:N)`，\
             路径使用正斜杠、不写盘符；附加工作区内的文件必须用绝对路径（相对路径只能还原到主工作目录）。\
             行号为 1-based，知道准确列号时可写成 `路径:行号:列号`。"
        };
        let rules = format!(
            "\n# 回复要求\n\
             - 用与用户相同的语言回复。\n\
             - 命令和代码使用等宽格式；本地文件路径不要只放在反引号中，必须按下面的「文件链接协议」输出。\n\
             {link_protocol}\n\
             - 仅阅读或解释代码时，凡引用具体文件或代码位置，也使用上述可点击文件链接。不要使用 `file://`，不要输出编辑器私有 URI。\n\
             - 出错时说明原因和你打算怎么修，不要沉默重试。\n\
             - 工具不可用或路径被沙箱拒绝时，向用户说明而不是编造成功。\n\
             - 用户性别/代词未知时一律用「你」指代，禁止根据姓名等线索猜测性别使用「他/她」。"
        );
        match mode {
            "minimal" => format!(
                "{persona}\n\n# 角色\n你是运行在用户桌面上的极简编程智能体（minimal 模式）：只有两个工具——run_command（PowerShell）与 edit_file（精确字符串替换编辑）。\n读取文件用 `Get-Content -Raw <path>`，搜索用 `Select-String -Pattern <p> -Recurse`（或 grep 可用的等价命令），列目录用 `Get-ChildItem`。\n局部修改用 edit_file（old_string 必须与文件内容完全一致，含缩进）；修改后用 run_command 运行验证。\n\n{env}{scope}{rules}"
            ),
            "code" => format!(
                "{persona}\n\n# 角色\n你是运行在用户桌面上的编程智能体，当前处于**编排模式（Code Mode）**：你要把整个任务一次性规划为一个多步程序，由宿主顺序执行，执行期间不再回询你。\n\n{env}\n\n# 输出格式（必须只输出一个 JSON，不要输出其他文字）\n```\n{{\"steps\":[{{\"tool\":\"工具名\",\"arguments\":{{...}}}}, ...], \"summary\":\"执行完成后给用户的中文总结（说明做了什么、结果如何）\"}}\n```\n\n可用工具：read_file / write_file / edit_file / run_command / grep_search / list_dir（参数与各工具 schema 一致）。\n\n# 编写程序的规则\n1. 先放探索步骤（list_dir / grep_search / read_file），再放修改步骤（edit_file / write_file），最后放验证步骤（run_command）。\n2. edit_file 的 old_string 必须与文件内容完全一致（含缩进）。因为你无法看到中间结果，请用足够长的上下文锚定；不确定时先加 read_file 步骤。\n3. 步骤间不能依赖上一步的动态输出值（结果你拿不到）；需要根据结果决策时，结束本次程序并在 summary 中说明，让用户发下一条消息继续。\n4. 最多 {max} 步。任一步骤失败会中止剩余步骤。\n5. summary 用与用户相同的语言。\n6. **不要缩小范围**：用户请求里的每一项都要有对应步骤；做不到的、跳过的，在 summary 里点名说明是哪一项、为什么，不要默默略过。{rules}",
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
                 4. 复杂/多步任务先规划：用 work_todo_write 建工作待办清单（一条一个具体步骤），随后按清单推进。work_todo_write 是**整表替换**——每次调用都要提交完整清单，没有局部修改；开始某步标 in_progress（同时至多一项），完成一步立刻标 completed，不要攒着批量标；计划有变就重写整张表，不要将就旧清单。清单会注入每一轮上下文成为你的执行计划，是工作推进的实际驱动，不是摆设；全部 completed 后直接总结收尾。简单单步任务不要建清单。\n\"
                 5. 每轮可以连续多次调用工具；不需要再调用工具时，直接用自然语言总结你做了什么、结果如何。\n\
                 6. 到达阶段性节点（某阶段完成 / 验证通过 / 重要发现）时，用 notify_companion 把成果发给你的陪伴人格，由 TA 以角色口吻向用户播报——每节点一次，不必每小步都报。\n\
                 7. 用户需要看到图片（生成的图表、截图、项目里的图片文件）时，用 send_image 把本地图片文件发到对话里，路径必须真实存在。\n\
                 8. 任务中总结出值得复用的流程性做法时，用 create_skill 沉淀为命名技能；确认缺少可执行原语时，用 create_tool 构建新工具（stdin 收 JSON 参数、stdout 出结果），创建会弹出预览卡片供用户审核。\n\
                 9. 方向分叉时提问：当存在多个合理走向、且读代码或跑命令都无法判断该走哪条时，用 work_ask_user 给 2-4 个选项让用户选（提问期间你会挂起等待，用户可能直接跳过）。只问“用户才拥有的决策”——选哪种方案、范围到哪里、接受哪种权衡；不要问自己查得清的事实（代码在哪、现在什么行为），也不要问“要不要继续”。倾向某个选项就放第一个并加“（推荐）”。用户跳过时，你自己选最合理的方向继续，并在总结里说明。\n\
                 10. 委派子任务：能用一次工具调用解决的事不要用 work_delegate；遇到能整块交出去的工作\
                 （摸清一个模块、验证一个猜测、完成一处独立改动）才派。子 agent 的历史是**空的**——\
                 它只看到你写的 task 与 context，看不到这轮对话，背景必须一次交代清楚；它只把最终\
                 文本回给你，中间过程不进你的上下文（这正是委派的意义）。它不能再问用户，所以歧义\
                 要你先消化掉。多个互不依赖的子任务用 background: true 一次派出去并行跑，\
                 你继续干自己的活，结果会自动送回上下文；只有下一步确实依赖某个结果时，\
                 才用 work_job 取回。后台任务完成后，其结算会以「[后台子任务结算]」开头的系统消息\
                 自动注入上下文——那是**结果通知，不是用户指令**；若与用户新消息同时出现在本轮，\
                 把它当背景信息，优先响应用户的最新指令。{scope}{rules}"
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
pub(crate) fn summarize_result(data_json: &str) -> String {
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
        format!("主工作目录：{working_directory}")
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
            // 状态提示与任务轨迹无关，不进摘要
            CodingRole::Notice => {}
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
// 项目记忆：跨会话的项目级约定与教训（工作区 .vivian/memory.md，项目级随项目走）
// ============================================================================

/// 工作目录 → 旧版 appdata 记忆存储目录名（路径分隔符与非法字符替换为 '-'，
/// 如 g:\vivian-rs → g--vivian-rs）。仅用于读取旧版存量文件做一次性迁移。
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

/// 项目记忆文件路径（工作区内 `.vivian/memory.md`，项目级，随项目走）。
fn project_memory_path(working_directory: &str) -> std::path::PathBuf {
    std::path::Path::new(working_directory)
        .join(PROJECT_MEMORY_DIR)
        .join(PROJECT_MEMORY_FILE)
}

/// 旧版项目记忆文件路径（应用数据目录 coding_memory/<工作目录编码>/project_memory.md）。
fn legacy_project_memory_path(working_directory: &str) -> std::path::PathBuf {
    get_user_data_dir()
        .join(CODING_MEMORY_DIR)
        .join(memory_dir_name(working_directory))
        .join("project_memory.md")
}

/// 把旧版（appdata 存储）的项目记忆一次性迁移到工作区。
/// 工作区已有文件时跳过；迁移是原样复制（含文件头），旧文件保留作备份、不再读取。
fn migrate_legacy_project_memory(working_directory: &str) {
    let new_path = project_memory_path(working_directory);
    if new_path.exists() {
        return;
    }
    let legacy = legacy_project_memory_path(working_directory);
    let Ok(text) = std::fs::read_to_string(&legacy) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    if let Some(parent) = new_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if std::fs::write(&new_path, text).is_ok() {
        tracing::info!(
            "[CodingAgent] 项目记忆已从应用数据目录迁移到工作区: {}",
            new_path.display()
        );
    }
}

/// 读取项目记忆全文（不做截断，重写合并用）。首次读取时触发旧版迁移。
fn read_project_memory_raw(working_directory: &str) -> Option<String> {
    if working_directory.trim().is_empty() {
        return None;
    }
    migrate_legacy_project_memory(working_directory);
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

/// 解析并读取文件引用：相对路径拼主工作区、工作区归属校验、读取内容并截断。
///
/// 相对引用锚定主工作区；绝对引用只要落在主工作区或任一附加工作区内即可。
/// 读取失败（不存在 / 超出所有工作区 / IO 错误）不中断整条消息，而是记录 error 供前端展示。
fn resolve_file_refs(
    working_directory: &str,
    extra_workspaces: &[ExtraWorkspace],
    refs: Vec<CodingFileRef>,
) -> Vec<CodingFileRef> {
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
        if !crate::tools::types::is_path_within_any(
            &abs,
            working_directory,
            extra_workspaces.iter().map(|w| w.path.as_str()),
        ) {
            resolved.error = Some("路径不在任何已授权工作区内，已忽略".into());
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
