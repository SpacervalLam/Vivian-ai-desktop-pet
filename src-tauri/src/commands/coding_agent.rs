//! 编程智能体 Tauri 命令（前端 CodeAgentPage 调用）
//!
//! 会话数据由全局 `CodingAgentService` 管理（持久化到用户数据目录），
//! LLM 路由与工具系统复用 AppState 的共享实例。

use std::sync::Arc;

use once_cell::sync::Lazy;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::brain::coding_agent::{
    CodingAgentService, CodingSession, CodingWorkspace, ExtraWorkspace,
};
use crate::state::AppState;

/// 文件树节点（前端侧边栏与 @-mention 文件选择共用）。
#[derive(Debug, Clone, Serialize)]
pub struct CodingFileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<CodingFileNode>,
}

/// 遍历目录构建文件树（与 list_dir 工具同规则：目录优先、跳过依赖目录）。
fn build_file_tree(dir: &std::path::Path, depth: usize) -> Vec<CodingFileNode> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|e| (e.path().is_file(), e.file_name()));
    let skip: &[&str] = &[".git", "node_modules", "target", "dist", ".venv", "__pycache__", ".next"];
    let mut nodes = Vec::new();
    for entry in items {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        if is_dir && skip.contains(&name.as_str()) {
            continue;
        }
        nodes.push(CodingFileNode {
            name: name.clone(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir,
            children: if is_dir { build_file_tree(&entry.path(), depth - 1) } else { Vec::new() },
        });
    }
    nodes
}

/// 列出工作目录文件树（前端侧边栏与 @-mention 文件选择）。
#[tauri::command]
pub fn coding_list_dir_tree(
    directory: String,
    max_depth: Option<u8>,
) -> Result<Vec<CodingFileNode>, String> {
    let dir = std::path::Path::new(&directory);
    if !dir.is_dir() {
        return Err(format!("目录不存在: {directory}"));
    }
    let depth = (max_depth.unwrap_or(2)).clamp(1, 4) as usize;
    Ok(build_file_tree(dir, depth))
}

/// 文件预览读取结果。
#[derive(Debug, Clone, Serialize)]
pub struct CodingFileRead {
    pub path: String,
    pub name: String,
    /// text / image / pdf / office / binary
    pub kind: String,
    /// 文本文件内容（kind == "text" 时；超长文件仅含首段）
    pub content: Option<String>,
    /// 文件大小（字节）
    pub size: u64,
    /// 文本总行数（kind == "text" 时）
    pub total_lines: Option<usize>,
    /// 文本是否因超长被截断（配合 `coding_read_file_lines` 分页续读）
    pub truncated: bool,
}

/// 预览首次读取的行数上限；超出部分走分页续读，避免一次性向 WebView 投喂超大文本。
const FILE_PREVIEW_MAX_LINES: usize = 3000;

/// 读取文件供右侧「预览」页展示：自动识别文本 / 图片 / PDF / 二进制。
///
/// - 文本：UTF-8 / 常见编码检测后返回可读内容；超长只返回首段并标记 `truncated`
/// - 图片 / PDF：只返回路径与类型，前端用 `convertFileSrc` / 内嵌方式渲染
/// - 其它二进制：仅返回类型与大小
#[tauri::command]
pub fn coding_read_file(path: String) -> Result<CodingFileRead, String> {
    let p = std::path::Path::new(&path);
    if !p.is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let size = std::fs::metadata(p)
        .map(|m| m.len())
        .unwrap_or(0);

    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // 图片：直接给路径（前端 convertFileSrc 渲染）
    const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return Ok(CodingFileRead { path, name, kind: "image".into(), content: None, size, total_lines: None, truncated: false });
    }
    // PDF：内嵌 iframe 渲染
    if ext == "pdf" {
        return Ok(CodingFileRead { path, name, kind: "pdf".into(), content: None, size, total_lines: None, truncated: false });
    }

    // Office 文档：**不能**落进下面的文本读取分支。
    //
    // .docx/.xlsx/.pptx 本质是 zip（OOXML），.doc/.xls/.ppt 是 OLE2 复合文档，
    // 按文本硬读只会得到一整屏乱码。这里单独标成 office，交给前端按格式渲染
    // （docx → HTML、xls/xlsx → 表格；其余无网页渲染方案的走「用系统程序打开」卡片）。
    const OFFICE_EXTS: &[&str] = &[
        "doc", "docx", "docm", "dot", "dotx", "dotm", "rtf",
        "xls", "xlsx", "xlsm", "xlsb", "xlt", "xltx", "xltm",
        "ppt", "pptx", "pptm", "pot", "potx", "potm", "pps", "ppsx",
        "odt", "ods", "odp", "odg", "wps", "wpt", "et", "ett", "dps", "dpt",
    ];
    if OFFICE_EXTS.contains(&ext.as_str()) {
        return Ok(CodingFileRead { path, name, kind: "office".into(), content: None, size, total_lines: None, truncated: false });
    }

    // 其余一律尝试按文本读取（编码检测兜底）；纯二进制（如 exe/dll/zip）会读到乱码，
    // 通过常见二进制扩展名黑名单提前判为 binary。
    const BINARY_EXTS: &[&str] = &[
        "exe", "dll", "so", "dylib", "bin", "obj", "o", "class", "jar", "zip", "gz",
        "7z", "rar", "tar", "iso", "png", "jpg", "jpeg", "gif", "webp", "bmp", "mp4",
        "mov", "avi", "mkv", "mp3", "wav", "flac", "woff", "woff2", "ttf", "otf", "wasm",
    ];
    if BINARY_EXTS.contains(&ext.as_str()) {
        return Ok(CodingFileRead { path, name, kind: "binary".into(), content: None, size, total_lines: None, truncated: false });
    }

    // 文本读取（复用 chat 侧的编码检测），统一换行符后按行截断
    let text = crate::commands::chat::read_text_with_encoding_detection(p)
        .map_err(|e| format!("读取文件失败: {e}"))?;
    let text = text.replace("\r\n", "\n");
    let total_lines = text.split('\n').count();
    let truncated = total_lines > FILE_PREVIEW_MAX_LINES;
    let content = if truncated {
        text.split('\n')
            .take(FILE_PREVIEW_MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text
    };
    Ok(CodingFileRead {
        path,
        name,
        kind: "text".into(),
        content: Some(content),
        size,
        total_lines: Some(total_lines),
        truncated,
    })
}

/// 分页读取文本文件的指定行区间（0 起始，`start` 含、`start + count` 不含）。
///
/// 供预览页对超长文件「加载更多」续读；只传输请求的行，不整文件搬运。
#[tauri::command]
pub fn coding_read_file_lines(path: String, start: usize, count: usize) -> Result<Vec<String>, String> {
    let p = std::path::Path::new(&path);
    if !p.is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    let count = count.clamp(1, 10_000);
    let text = crate::commands::chat::read_text_with_encoding_detection(p)
        .map_err(|e| format!("读取文件失败: {e}"))?
        .replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    Ok(lines
        .into_iter()
        .skip(start)
        .take(count)
        .map(|s| s.to_string())
        .collect())
}

/// 覆写一个文本文件（预览页编辑保存）。父目录不存在时自动创建。
#[tauri::command]
pub fn coding_write_file(path: String, content: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
        }
    }
    std::fs::write(p, content.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))
}

/// 在系统文件管理器中定位并选中该文件（预览页签右键菜单「在文件资源管理器中显示」）。
///
/// 只负责把文件管理器「呼出来并选中」，不打开文件本身；Windows 走 `explorer /select,`，
/// macOS 走 `open -R`，Linux 没有统一的「选中」语义，退化为打开所在目录。
#[tauri::command]
pub fn coding_reveal_in_explorer(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {path}"));
    }
    let target = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());

    // explorer 的参数解析很特殊：必须是 `/select,<路径>` 这样的单参数形式，
    // 拆成两个参数（`/select,` + 路径）会被当成两个待打开对象而失效。
    #[cfg(windows)]
    let spawned = crate::utils::process::silent_command("explorer")
        .arg(format!("/select,{}", target.to_string_lossy()))
        .spawn();

    #[cfg(target_os = "macos")]
    let spawned = crate::utils::process::silent_command("open")
        .arg("-R")
        .arg(&target)
        .spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = crate::utils::process::silent_command("xdg-open")
        .arg(target.parent().unwrap_or(std::path::Path::new(".")))
        .spawn();

    #[cfg(not(any(windows, unix)))]
    let spawned: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持在文件管理器中显示",
    ));

    spawned.map(|_| ()).map_err(|e| format!("在文件管理器中显示失败: {e}"))
}

/// 把文件字节级复制到目标路径（预览页签右键菜单「另存为」）。
///
/// 走字节复制而不是「前端读文本再写回」：预览里的图片 / PDF / 二进制同样要能另存，
/// 这些内容在前端并没有可用的文本形态。目标父目录不存在时自动创建。
#[tauri::command]
pub fn coding_copy_file_to(from: String, to: String) -> Result<(), String> {
    let src = std::path::Path::new(&from);
    if !src.is_file() {
        return Err(format!("文件不存在: {from}"));
    }
    let dst = std::path::Path::new(&to);
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
        }
    }
    // 同一路径直接当成功：用户可能把另存对话框指回了原文件
    if src == dst {
        return Ok(());
    }
    std::fs::copy(src, dst).map_err(|e| format!("另存为失败: {e}"))?;
    Ok(())
}

/// 预览页「就地改写」：按用户给的编辑指令重写被选中的那段文字。
///
/// 这是**无会话上下文**的一次性调用 —— 不写会话消息、不落工作区记录，
/// 结果只回给前端，由用户在预览页里接受或拒绝。
///
/// 走路由矩阵 `text_rewrite` 任务（辅助分组，不占对话并发额度、不加采样惩罚）。
/// 与 `polish_asr_text` 不同，这里失败直接返回 Err 由前端提示：改写是用户主动
/// 触发的交互，静默降级返回原文会让人误以为改写已经生效。
#[tauri::command]
pub async fn rewrite_preview_selection(
    state: State<'_, Arc<AppState>>,
    selection: String,
    instruction: String,
    context: Option<String>,
) -> Result<String, String> {
    let selection = selection.trim().to_string();
    if selection.is_empty() {
        return Err("没有选中任何文字".into());
    }
    let instruction = instruction.trim().to_string();
    if instruction.is_empty() {
        return Err("请先描述要如何修改".into());
    }

    let character = state.get_character(None)?;

    // 上文语境只用于消歧（代词指代、术语一致性），提示里明确禁止它顺手改掉周围段落。
    let system_prompt = "你是文档编辑助手。用户会给你一段【选中文本】和一条【编辑要求】，\
        请严格按照编辑要求重写这段文本。\n\
        要求：\n\
        1. 只输出重写后的文本本身，不要任何解释、前言、序号或引号包裹\n\
        2. 保持原文的语言（中文就输出中文，英文就输出英文）\n\
        3. 保持原文的 markdown 标记风格（标题层级、列表符号、强调符号等），除非编辑要求明确要求改动\n\
        4. 编辑要求未涉及的部分尽量保持原样，不要自作主张扩写或删减信息\n\
        5. 不要输出 diff、不要用 ``` 代码块包裹，直接给正文";

    let mut user_content = String::new();
    if let Some(ctx) = context.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        user_content.push_str("【上文语境（仅供参考，不要修改这部分）】\n");
        user_content.push_str(ctx);
        user_content.push_str("\n\n");
    }
    user_content.push_str("【选中文本】\n");
    user_content.push_str(&selection);
    user_content.push_str("\n\n【编辑要求】\n");
    user_content.push_str(&instruction);

    let messages = vec![
        crate::types::response::ChatMessage::system(system_prompt),
        crate::types::response::ChatMessage::user(&user_content),
    ];
    let request = crate::providers::base::LLMRequest::new("text_rewrite", messages)
        .with_temperature(0.3)
        .with_character_id(character.id.clone());

    let raw = character
        .brain
        .router
        .generate(request)
        .await
        .map_err(|e| format!("改写失败: {e}"))?;

    // 复用语音润色的清理逻辑：去代码块包裹、去首尾引号
    let rewritten = crate::commands::speech::parse_polished_text(&raw);
    if rewritten.is_empty() {
        return Err("模型返回了空内容".into());
    }
    Ok(rewritten)
}

/// 全局编程智能体服务单例。
pub static CODING_AGENT: Lazy<Arc<CodingAgentService>> =
    Lazy::new(|| Arc::new(CodingAgentService::new()));

/// 解析会话的上下文窗口：工作模型显式配置 → 主配置 → 厂商默认窗口。
fn resolve_context_window(cfg: &crate::config::manager::AppConfig, model_id: Option<&str>) -> u64 {
    if let Some(mid) = model_id {
        if let Some(m) = cfg.work_models.iter().find(|m| m.id == mid) {
            if let Some(w) = m.route.context_window {
                return w;
            }
            return crate::providers::capabilities::default_context_window(&m.route.model);
        }
    }
    if let Some(w) = cfg.ai.context_window {
        return w;
    }
    crate::providers::capabilities::default_context_window(&cfg.ai.model)
}

/// 新建编程会话。
#[tauri::command]
pub fn coding_new_session(
    state: State<'_, Arc<AppState>>,
    char_id: String,
    working_directory: String,
    mode: Option<String>,
) -> Result<CodingSession, String> {
    tracing::info!(
        "[CodingAgent] 新建会话请求: char_id={char_id}, working_directory={working_directory}, mode={}",
        mode.as_deref().unwrap_or("standard")
    );
    // 工作目录必须存在
    if !working_directory.is_empty() && !std::path::Path::new(&working_directory).is_dir() {
        tracing::warn!("[CodingAgent] 新建会话被拒: 工作目录不存在: {working_directory}");
        return Err(format!("工作目录不存在: {working_directory}"));
    }
    let session = CODING_AGENT.create_session(&char_id, &working_directory, mode.as_deref().unwrap_or("standard"));
    // 按当前配置解析会话上下文窗口（active 工作模型 → 主配置 → 厂商默认）
    let cfg = state.config.read().get_all();
    let active_id = cfg.active_work_model.as_deref();
    let window = resolve_context_window(&cfg, active_id);
    CODING_AGENT.set_context_window(&session.session_id, window);
    tracing::info!("[CodingAgent] 会话已创建: id={}, context_window={window}", session.session_id);
    Ok(session)
}

/// 切换会话工作模式（standard / code / minimal；运行中拒绝）。
#[tauri::command]
pub fn coding_set_mode(session_id: String, mode: String) -> Result<(), String> {
    CODING_AGENT.set_mode(&session_id, &mode)
}

/// 会话历史中出现过的工作区列表（去重，按最近使用倒序）。
#[tauri::command]
pub fn coding_list_workspaces() -> Vec<CodingWorkspace> {
    CODING_AGENT.list_workspaces()
}

/// 切换会话主工作目录（目录必须存在；运行中拒绝）。
///
/// 主工作区决定相对路径解析、项目记忆位置、终端 cwd 与提示词环境块；
/// 只扩大可访问范围请用 [`coding_add_workspace`]。
#[tauri::command]
pub fn coding_set_workspace(session_id: String, workspace: String) -> Result<(), String> {
    CODING_AGENT.set_workspace(&session_id, &workspace)
}

/// 挂载附加工作区（目录必须存在；已挂载则更新只读标记；运行中拒绝）。返回最新列表。
#[tauri::command]
pub fn coding_add_workspace(
    session_id: String,
    path: String,
    read_only: bool,
) -> Result<Vec<ExtraWorkspace>, String> {
    CODING_AGENT.add_workspace(&session_id, &path, read_only)
}

/// 卸载附加工作区（运行中拒绝）。返回最新列表。
#[tauri::command]
pub fn coding_remove_workspace(
    session_id: String,
    path: String,
) -> Result<Vec<ExtraWorkspace>, String> {
    CODING_AGENT.remove_workspace(&session_id, &path)
}

/// 切换附加工作区的只读标记（运行中拒绝）。返回最新列表。
#[tauri::command]
pub fn coding_set_workspace_read_only(
    session_id: String,
    path: String,
    read_only: bool,
) -> Result<Vec<ExtraWorkspace>, String> {
    CODING_AGENT.set_workspace_read_only(&session_id, &path, read_only)
}

/// 设置会话权限等级（read_only / workspace_write / full_access；运行中拒绝）。
#[tauri::command]
pub fn coding_set_permission(session_id: String, permission: String) -> Result<(), String> {
    CODING_AGENT.set_permission(&session_id, &permission)
}

/// 设置会话选中的工作模型 id（与 select_work_model 同步；运行中拒绝）。
#[tauri::command]
pub fn coding_set_model(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    model_id: String,
) -> Result<(), String> {
    CODING_AGENT.set_model(&session_id, &model_id)?;
    // 同步会话上下文窗口（该工作模型的配置或厂商默认）
    let cfg = state.config.read().get_all();
    let window = resolve_context_window(&cfg, Some(&model_id));
    CODING_AGENT.set_context_window(&session_id, window);
    Ok(())
}

/// 设置会话推理等级（low / medium / high；运行中拒绝）。
#[tauri::command]
pub fn coding_set_reasoning_level(session_id: String, level: String) -> Result<(), String> {
    CODING_AGENT.set_reasoning_level(&session_id, &level)
}

/// 可用的工作模型列表（id + name，供编程页模型下拉选择）。
#[tauri::command]
pub fn coding_list_available_models(
    state: State<'_, Arc<AppState>>,
) -> Vec<serde_json::Value> {
    let cfg = state.config.read().get_all();
    cfg.work_models
        .iter()
        .map(|m| serde_json::json!({ "id": m.id, "name": m.name }))
        .collect()
}

/// 会话简表（含完整消息，供列表与恢复）。
#[tauri::command]
pub fn coding_list_sessions() -> Vec<CodingSession> {
    CODING_AGENT.list_sessions()
}

/// 删除会话。
#[tauri::command]
pub fn coding_delete_session(session_id: String) -> Result<bool, String> {
    Ok(CODING_AGENT.delete_session(&session_id))
}

/// 重命名会话（侧边栏右键菜单）。返回更新后的会话，前端直接替换列表项。
#[tauri::command]
pub fn coding_rename_session(session_id: String, title: String) -> Result<CodingSession, String> {
    CODING_AGENT.rename_session(&session_id, &title)
}

/// 取消正在运行的会话任务。
#[tauri::command]
pub fn coding_cancel_session(session_id: String) -> Result<bool, String> {
    Ok(CODING_AGENT.cancel(&session_id))
}

/// 发送用户消息并驱动 agent loop（事件实时广播 coding:*）。图片与文件引用随消息注入上下文。
/// `interjected` 标记任务执行期间排队的插话消息，构建 LLM 上下文时加插话标注；
/// `guided` 标记用户在任务执行期间给出的引导消息，构建 LLM 上下文时加引导标注。
#[tauri::command]
pub fn coding_send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message: String,
    images: Option<Vec<crate::brain::coding_agent::CodingImage>>,
    file_refs: Option<Vec<crate::brain::coding_agent::CodingFileRef>>,
    interjected: Option<bool>,
    guided: Option<bool>,
) -> Result<(), String> {
    let router = state
        .model_router
        .read()
        .clone()
        .ok_or("模型路由未初始化（请先配置模型）")?;
    let tool_system = state.tool_system.clone();
    // 单轮 LLM↔工具 循环预算（设置-工具-编程智能体最大轮次，默认 48）
    let max_rounds = state.config.read().get_all().tools.max_coding_rounds as usize;
    CODING_AGENT.send_message(
        app,
        session_id,
        router,
        tool_system,
        message,
        images.unwrap_or_default(),
        file_refs.unwrap_or_default(),
        max_rounds,
        interjected.unwrap_or(false),
        guided.unwrap_or(false),
    )
}

/// 设置单条消息级反馈（up / down，空串清除）。
#[tauri::command]
pub fn coding_set_message_feedback(
    session_id: String,
    message_index: usize,
    rating: String,
) -> Result<(), String> {
    CODING_AGENT.set_message_feedback(&session_id, message_index, &rating)
}

/// 从指定消息处 fork 出新的独立会话。
#[tauri::command]
pub fn coding_fork_session(session_id: String, message_index: usize) -> Result<CodingSession, String> {
    CODING_AGENT.fork_session(&session_id, message_index)
}

// ===== 工作智能体独立待办（与陪伴 todo 分离，随会话持久化） =====

fn emit_work_todo_changed(
    app: &tauri::AppHandle,
    session_id: &str,
    items: &[crate::brain::coding_agent::WorkTodo],
) {
    let _ = app.emit(
        "work_todo:changed",
        serde_json::json!({ "session_id": session_id, "items": items }),
    );
}

/// 读取当前会话的工作待办清单。
#[tauri::command]
pub fn coding_get_work_todos(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Vec<crate::brain::coding_agent::WorkTodo>, String> {
    let items = CODING_AGENT.list_work_todos(&session_id)?;
    emit_work_todo_changed(&app, &session_id, &items);
    Ok(items)
}

/// 回传用户对工作智能体提问的回答，唤醒挂起中的 `work_ask_user`。
///
/// 答案的合法性（选项是否属于本题、单选时自由输入与选项互斥）由注册表裁决，
/// 不合法会原样返回错误，前端据此提示用户重选。
#[tauri::command]
pub fn coding_respond_question(
    question_id: u64,
    answer: crate::brain::work_question::WorkQuestionAnswer,
) -> Result<(), String> {
    crate::brain::work_question::global_work_question_registry().respond(question_id, answer)
}

/// 取当前会话尚未回答的提问（切换会话 / 面板重挂载时用来恢复询问卡片）。
#[tauri::command]
pub fn coding_pending_question(
    session_id: String,
) -> Option<crate::brain::work_question::WorkQuestionRequest> {
    crate::brain::work_question::global_work_question_registry().pending_for(&session_id)
}
