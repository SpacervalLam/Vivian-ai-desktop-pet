//! 持久终端工具（`terminal_*` + `pwsh_persistent`）。
//!
//! 与 `run_command` 的区别：`run_command` 每次都是一条独立命令（自带超时、跑完即走），
//! 适合构建/测试这类一次性动作；**这里是一条长期活着的 shell**，`cd`、环境变量、
//! 进 REPL、跑交互式工具都能跨调用保留。
//!
//! 实现要点：
//! - 一个终端 = 一个 PowerShell 子进程 + stdin 管道 + 输出环形缓冲。
//! - 后台任务持续把 stdout/stderr 读进缓冲；`terminal_read` 只是取走增量。
//! - 输出强制 UTF-8（中文 Windows 的 CP936 会让每行都变乱码，见 run_command 的同类问题）。
//! - 信号用 kill 实现：Windows 没有可用的 SIGINT 语义，假装支持会误导模型。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::tools::terminal_buffer::{OutputBuffer, Utf8Stream};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdin, Command};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// 终端背后的进程形态
///
/// 两种：普通子进程（沙箱未启用 / 非 Windows）与受限令牌沙箱进程。两者的 stdin
/// 都是异步写、stdout/stderr 都要被后台读进缓冲，但句柄类型不同——统一在这里
/// 收口，避免每个访问点各自判断。
enum TermProc {
    Plain {
        child: tokio::sync::Mutex<Child>,
        stdin: tokio::sync::Mutex<ChildStdin>,
    },
    #[cfg(windows)]
    Sandboxed {
        shell: std::sync::Arc<crate::utils::sandbox::SandboxedShell>,
        stdin: tokio::sync::Mutex<tokio::fs::File>,
    },
}

impl TermProc {
    fn control_path(&self) -> std::path::PathBuf {
        let directory = match self {
            #[cfg(windows)]
            Self::Sandboxed { shell, .. } => shell.scratch_dir().to_path_buf(),
            _ => std::env::temp_dir(),
        };
        directory.join(format!(
            "vivian-command-{}.status",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn start_kill(&self) {
        match self {
            Self::Plain { child, .. } => {
                if let Ok(mut child) = child.try_lock() {
                    let _ = child.start_kill();
                }
            }
            #[cfg(windows)]
            Self::Sandboxed { shell, .. } => shell.kill(),
        }
    }
    async fn is_alive(&self) -> bool {
        match self {
            Self::Plain { child, .. } => child
                .lock()
                .await
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false),
            #[cfg(windows)]
            Self::Sandboxed { shell, .. } => shell.try_wait().is_none(),
        }
    }
    /// 是否运行在沙箱里（结果里回传给模型，便于解释"写不进去"这类现象）
    fn sandboxed(&self) -> bool {
        !matches!(self, TermProc::Plain { .. })
    }

    async fn kill(&self) {
        match self {
            TermProc::Plain { child, .. } => {
                let mut c = child.lock().await;
                let _ = c.kill().await;
            }
            #[cfg(windows)]
            TermProc::Sandboxed { shell, .. } => shell.kill(),
        }
    }
}

struct TermHandle {
    _slot: tokio::sync::OwnedSemaphorePermit,
    id: String,
    session_id: String,
    char_id: String,
    agent_kind: String,
    user_id: String,
    access_level: Option<crate::tools::types::AgentAccessLevel>,
    workspaces: Vec<crate::tools::types::WorkingDirectoryPermission>,
    workspace_root: String,
    sandbox_config: crate::config::manager::SandboxConfig,
    cwd: String,
    proc: TermProc,
    /// 命令级互斥：同一终端上一次只允许一个"写命令→等输出→读输出"的完整回合。
    ///
    /// 没有它的话，两个 pwsh_persistent 并发时输出会交错，各自读到对方的片段，
    /// 拼出来的结果谁也解释不了。terminal_send / terminal_read 也拿这把锁，
    /// 否则一条 send 会插进别人正在等的窗口里。
    cmd_lock: tokio::sync::Mutex<()>,
    /// 累积输出（保留尾部 MAX_BUFFER_CHARS）+ 已读游标
    buffer: Arc<Mutex<OutputBuffer>>,

    alive: Arc<AtomicBool>,
}

fn terms() -> &'static tokio::sync::Mutex<HashMap<String, Arc<TermHandle>>> {
    static TERMS: OnceLock<tokio::sync::Mutex<HashMap<String, Arc<TermHandle>>>> = OnceLock::new();
    TERMS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn next_term_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!("term{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 起一个持久 PowerShell。
fn spawn_reader<R>(
    mut reader: R,
    buffer: Arc<Mutex<OutputBuffer>>,
    _alive: Arc<AtomicBool>,
    tag: &'static str,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut decoder = Utf8Stream::default();
        let mut chunk = [0u8; 8192];
        loop {
            let n = match reader.read(&mut chunk).await {
                Ok(n) => n,
                Err(_) => 0,
            };
            let text = decoder.decode(&chunk[..n], n == 0);
            if !text.is_empty() {
                let mut output = buffer.lock().unwrap_or_else(|e| e.into_inner());
                output.append(tag);
                output.append(&text);
            }
            if n == 0 {
                break;
            }
        }
    });
}
/// 起一个普通 PowerShell 子进程（非沙箱路径）
async fn spawn_plain(
    buffer: &Arc<Mutex<OutputBuffer>>,
    alive: &Arc<AtomicBool>,
    cwd: &str,
) -> Result<TermProc, String> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NoLogo", "-Command", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if !cwd.trim().is_empty() {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        // 不弹控制台窗口：tokio 的 Command 自带这个方法（Windows），无需引入 std 的扩展 trait
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 PowerShell 失败：{e}"))?;
    let stdin = child.stdin.take().ok_or("拿不到 stdin")?;
    let stdout = child.stdout.take().ok_or("拿不到 stdout")?;
    let stderr = child.stderr.take().ok_or("拿不到 stderr")?;
    spawn_reader(stdout, Arc::clone(buffer), Arc::clone(alive), "");
    spawn_reader(stderr, Arc::clone(buffer), Arc::clone(alive), "[stderr] ");
    Ok(TermProc::Plain {
        child: tokio::sync::Mutex::new(child),
        stdin: tokio::sync::Mutex::new(stdin),
    })
}

/// 后台读取（阻塞管道版）
///
/// 沙箱进程的 stdio 是匿名管道，只拿到同步句柄——tokio 的 `File` 面向常规文件，
/// 套在管道上行为不可靠。这里直接起阻塞线程读，读到的行推进同一个缓冲。
fn spawn_reader_blocking(
    mut reader: std::fs::File,
    buffer: Arc<Mutex<OutputBuffer>>,
    _alive: Arc<AtomicBool>,
    tag: &'static str,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut decoder = Utf8Stream::default();
        let mut chunk = [0u8; 8192];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(n) => n,
                Err(_) => 0,
            };
            let text = decoder.decode(&chunk[..n], n == 0);
            if !text.is_empty() {
                let mut output = buffer.lock().unwrap_or_else(|e| e.into_inner());
                output.append(tag);
                output.append(&text);
            }
            if n == 0 {
                break;
            }
        }
    });
}
async fn open_terminal(ctx: &ToolUseContext, cwd: &str) -> Result<Arc<TermHandle>, String> {
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let slot = Arc::clone(SLOTS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(64))))
        .try_acquire_owned()
        .map_err(|_| "终端数量已达到上限（64），请先关闭不再使用的终端。")?;
    let sandbox_config = crate::tools::sandbox_env::config();
    // 非 Windows 上没有受限令牌可用；配置要求必须有隔离时就地拒绝，
    // 不开一个全权限的长活 shell 糊过去。
    let isolated = cfg!(windows) && sandbox_config.enabled && sandbox_config.sandbox_terminal;
    if sandbox_config.require_isolation && !isolated {
        return Err(
            "当前环境没有可用的 OS 级沙箱，已按配置拒绝打开终端。若要使用请关闭「要求 OS 级隔离」。"
                .to_string(),
        );
    }
    let id = next_term_id();
    let buffer = Arc::new(Mutex::new(OutputBuffer::default()));

    let alive = Arc::new(AtomicBool::new(true));

    #[cfg(windows)]
    let proc = {
        if isolated {
            let req = crate::tools::sandbox_env::request_for(ctx);
            let dir = if cwd.trim().is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(cwd))
            };
            let shell = tokio::task::spawn_blocking(move || {
                crate::utils::sandbox::spawn_sandboxed_shell(&req, dir.as_deref())
            })
            .await
            .map_err(|e| format!("沙箱终端任务失败: {e}"))??;
            let sin = shell
                .stdin
                .try_clone()
                .map_err(|e| format!("克隆沙箱 stdin 失败：{e}"))?;
            let sout = shell
                .stdout
                .try_clone()
                .map_err(|e| format!("克隆沙箱 stdout 失败：{e}"))?;
            let serr = shell
                .stderr
                .try_clone()
                .map_err(|e| format!("克隆沙箱 stderr 失败：{e}"))?;
            spawn_reader_blocking(sout, Arc::clone(&buffer), Arc::clone(&alive), "");
            spawn_reader_blocking(serr, Arc::clone(&buffer), Arc::clone(&alive), "[stderr] ");
            TermProc::Sandboxed {
                shell: std::sync::Arc::new(shell),
                stdin: tokio::sync::Mutex::new(tokio::fs::File::from_std(sin)),
            }
        } else {
            spawn_plain(&buffer, &alive, cwd).await?
        }
    };
    #[cfg(not(windows))]
    let proc = spawn_plain(&buffer, &alive, cwd).await?;

    let handle = Arc::new(TermHandle {
        _slot: slot,
        id: id.clone(),
        session_id: ctx.session_id.clone(),
        char_id: ctx.char_id.clone(),
        agent_kind: ctx.agent_kind.clone(),
        user_id: ctx.user_id.clone(),
        access_level: ctx.access_level,
        workspaces: ctx.extra_working_directories.clone(),
        workspace_root: ctx.working_directory.clone(),
        sandbox_config,
        cwd: cwd.to_string(),
        cmd_lock: tokio::sync::Mutex::new(()),
        proc,
        buffer,

        alive,
    });

    // 首条命令把编码切到 UTF-8（与 run_command 同一套理由）
    let _ = write_line(
        &handle,
        "[Console]::InputEncoding=[System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $OutputEncoding=[Console]::OutputEncoding",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    terms().lock().await.insert(id, Arc::clone(&handle));
    Ok(handle)
}

/// 取走自上次读取以来的新输出。
fn drain_new(h: &TermHandle, max_chars: usize) -> String {
    h.buffer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .read(max_chars)
}
async fn write_line(h: &TermHandle, text: &str) -> Result<(), String> {
    match &h.proc {
        TermProc::Plain { stdin, .. } => {
            let mut w = stdin.lock().await;
            write_line_to(&mut *w, text).await
        }
        #[cfg(windows)]
        TermProc::Sandboxed { stdin, .. } => {
            let mut w = stdin.lock().await;
            write_line_to(&mut *w, text).await
        }
    }
}

async fn write_line_to<W>(w: &mut W, text: &str) -> Result<(), String>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    w.write_all(text.as_bytes())
        .await
        .map_err(|e| format!("写入终端失败：{e}"))?;
    if !text.ends_with('\n') {
        w.write_all(b"\n").await.map_err(|e| e.to_string())?;
    }
    w.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn get_owned(id: &str, ctx: &ToolUseContext) -> Result<Arc<TermHandle>, String> {
    let map = terms().lock().await;
    match map.get(id) {
        None => Err(format!(
            "没有 id 为 {id} 的终端。用 terminal_list 看现有终端，或 terminal_open 新建。"
        )),
        Some(h) if !owns(h, ctx) => Err("该终端不属于本角色、调用侧和会话。".to_string()),
        Some(h) => Ok(Arc::clone(h)),
    }
}

fn owns(h: &TermHandle, ctx: &ToolUseContext) -> bool {
    h.session_id == ctx.session_id
        && h.char_id == ctx.char_id
        && h.agent_kind == ctx.agent_kind
        && h.user_id == ctx.user_id
}

fn check_terminal_scope(h: &TermHandle, ctx: &ToolUseContext) -> Result<(), String> {
    if h.access_level != ctx.access_level
        || h.workspaces != ctx.extra_working_directories
        || h.workspace_root != ctx.working_directory
        || h.sandbox_config != crate::tools::sandbox_env::config()
    {
        return Err("终端的权限或沙箱配置已变化，请关闭旧终端后重新打开。".into());
    }
    Ok(())
}

// ============================================================================
// terminal_open
// ============================================================================

pub struct TerminalOpenTool;
impl TerminalOpenTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalOpenTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalOpenTool {
    fn name(&self) -> &str {
        "terminal_open"
    }

    fn description(&self) -> &str {
        "Open a long-lived PowerShell session and return its id. Unlike run_command, state persists across calls: \
         cd, environment variables, and interactive programs (REPL, debugger, watcher) survive. \
         Prefer run_command for one-shot commands — a persistent shell is for stateful sessions only."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "开一条长期活着的 PowerShell 会话，返回 id。与 run_command 的区别是**状态跨调用保留**：\
                     cd、环境变量、交互式程序（REPL / 调试器 / 监听进程）都不会随命令结束而丢失。\
                     一次性命令仍然用 run_command——持久 shell 只给真正需要状态的场景。",
            "ja" => "長寿命の PowerShell セッションを開き id を返す。run_command と違い**状態が呼び出しを跨いで残る**：\
                     cd・環境変数・対話型プログラム（REPL/デバッガ/ウォッチャ）が維持される。\
                     単発コマンドは run_command を使い、状態が必要な時だけこちらを使う。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string", "description": "起始目录（默认当前工作目录）" }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        if let Some(cwd) = input.get("cwd").and_then(Value::as_str) {
            if !cwd.is_empty()
                && (!std::path::Path::new(cwd).is_absolute() || !ctx.is_path_authorized(cwd))
            {
                return ValidationResult::failure("终端 cwd 必须是授权工作区内的绝对路径", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.working_directory.clone());
        match open_terminal(ctx, &cwd).await {
            Ok(h) => ToolResult::standard_success(
                &format!(
                    "已打开终端 {}（cwd={}）。接下来：terminal_send 发命令，terminal_read 取输出，terminal_close 关闭。",
                    h.id, h.cwd
                ),
                Some(json!({
                    "terminal_id": h.id,
                    "cwd": h.cwd,
                    // 回传给模型：沙箱里的工作区之外写不进去，遇到「拒绝访问」
                    // 时不必反复重试，也不必怀疑是命令写错了
                    "sandboxed": h.proc.sandboxed(),
                })),
            ),
            Err(e) => ToolResult::standard_error(&e, Some("TerminalError"), None),
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
    fn search_hint(&self) -> &str {
        "terminal open shell session persistent 终端 打开会话 交互"
    }
}

// ============================================================================
// terminal_send
// ============================================================================

pub struct TerminalSendTool;
impl TerminalSendTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalSendTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalSendTool {
    fn name(&self) -> &str {
        "terminal_send"
    }

    fn description(&self) -> &str {
        "Type a line into a persistent terminal (a newline is appended). Returns immediately — \
         follow with terminal_read to see the output. For REPLs, send one expression per call."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "往持久终端里输入一行（自动补回车）。立即返回——输出用 terminal_read 取。\
                     REPL 场景一次发一个表达式，别把整段脚本糊进去。",
            "ja" => "永続ターミナルに 1 行入力する（改行を自動付加）。すぐ戻るので、出力は terminal_read で取る。\
                     REPL では式を 1 つずつ送る。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "terminal_id": { "type": "string", "description": "终端 id" },
                "text": { "type": "string", "description": "要输入的内容（不含换行）" }
            },
            "required": ["terminal_id", "text"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let h = match get_owned(id, ctx).await {
            Ok(h) => h,
            Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
        };
        if let Err(e) = check_terminal_scope(&h, ctx) {
            return ToolResult::standard_error(&e, Some("TerminalPermissionChanged"), None);
        }
        if !h.proc.is_alive().await {
            return ToolResult::standard_error(
                &format!("终端 {id} 的进程已经退出。用 terminal_open 重开一条。"),
                Some("TerminalClosed"),
                None,
            );
        }
        // 写一行也要排队：否则会插进另一个正在等输出的回合里
        let _turn = h.cmd_lock.lock().await;
        match write_line(&h, text).await {
            Ok(()) => ToolResult::standard_success(
                &format!("已发送到 {id}。"),
                Some(json!({"terminal_id": id, "sent": text})),
            ),
            Err(e) => ToolResult::standard_error(&e, Some("TerminalError"), None),
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
    fn search_hint(&self) -> &str {
        "terminal send type input 终端 输入 发送命令"
    }
}

// ============================================================================
// terminal_read
// ============================================================================

pub struct TerminalReadTool;
impl TerminalReadTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalReadTool {
    fn name(&self) -> &str {
        "terminal_read"
    }

    fn description(&self) -> &str {
        "Read output produced since your last read on this terminal. Waits up to wait_ms for new output first. \
         Output is incremental — you will not see previous lines again unless you ask for a larger wait."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取该终端自上次读取以来的新输出（先最多等 wait_ms 毫秒）。是**增量**的——\
                     不会重复返回已经读过的内容。",
            "ja" => "このターミナルで前回読んで以降の出力を読む（先に最大 wait_ms ミリ秒待つ）。**増分**のみで、既読は再返却されない。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "terminal_id": { "type": "string", "description": "终端 id" },
                "wait_ms": { "type": "integer", "description": "最多等待多少毫秒（默认 800，上限 15000）", "minimum": 0 },
                "max_chars": { "type": "integer", "description": "最多返回多少字符（默认 8000，上限 40000）", "minimum": 200 }
            },
            "required": ["terminal_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let wait_ms = args
            .get("wait_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(800)
            .min(15_000);
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(8_000)
            .clamp(200, 40_000) as usize;
        let h = match get_owned(id, ctx).await {
            Ok(h) => h,
            Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
        };
        // 读也要排队：等待窗口内如果有别的命令在跑，读到的是别人的输出
        let _turn = h.cmd_lock.lock().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
        let mut out = drain_new(&h, max_chars);
        while out.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            out = drain_new(&h, max_chars);
        }
        let alive = h.proc.is_alive().await;
        h.alive.store(alive, Ordering::Relaxed);
        let truncated = h
            .buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .has_unread();
        let body: String = out.chars().take(max_chars).collect();
        if body.trim().is_empty() {
            return ToolResult::standard_success(
                &format!(
                    "（{}内没有新输出；进程{}）",
                    if wait_ms == 0 {
                        "此刻".to_string()
                    } else {
                        format!("{wait_ms}ms")
                    },
                    if alive { "仍在运行" } else { "已退出" }
                ),
                Some(json!({"terminal_id": id, "output": "", "process_alive": alive})),
            );
        }
        ToolResult::standard_success(
            &format!(
                "{body}{}",
                if truncated {
                    "\n（输出已截断）"
                } else {
                    ""
                }
            ),
            Some(
                json!({"terminal_id": id, "output": body, "truncated": truncated, "process_alive": alive}),
            ),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
    fn search_hint(&self) -> &str {
        "terminal read output 终端 输出 读取结果"
    }
}

// ============================================================================
// terminal_list / terminal_signal / terminal_close
// ============================================================================

pub struct TerminalListTool;
impl TerminalListTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalListTool {
    fn name(&self) -> &str {
        "terminal_list"
    }

    fn description(&self) -> &str {
        "List persistent terminals belonging to this session, with id, cwd and whether the process is still alive."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出本会话的持久终端：id、起始目录、进程是否还活着。",
            "ja" => {
                "このセッションの永続ターミナル一覧：id・作業ディレクトリ・プロセスが生きているか。"
            }
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, ctx: &ToolUseContext) -> ToolResult {
        let map = terms().lock().await;
        let rows: Vec<_> = map
            .values()
            .filter(|h| owns(h, ctx))
            .map(|h| {
                json!({
                    "terminal_id": h.id,
                    "cwd": h.cwd,
                    "alive": h.alive.load(Ordering::Relaxed),
                })
            })
            .collect();
        if rows.is_empty() {
            return ToolResult::standard_success(
                "本会话没有打开的终端。用 terminal_open 新建。",
                Some(json!({"terminals": []})),
            );
        }
        let mut text = format!("本会话的终端：{} 个\n", rows.len());
        for r in &rows {
            text.push_str(&format!(
                "- {} cwd={} {}\n",
                r["terminal_id"].as_str().unwrap_or(""),
                r["cwd"].as_str().unwrap_or(""),
                if r["alive"].as_bool().unwrap_or(false) {
                    "运行中"
                } else {
                    "已退出"
                }
            ));
        }
        ToolResult::standard_success(&text, Some(json!({"terminals": rows})))
    }

    fn is_read_only(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
    fn search_hint(&self) -> &str {
        "terminal list sessions 终端列表 有哪些会话"
    }
}

pub struct TerminalSignalTool;
impl TerminalSignalTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalSignalTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalSignalTool {
    fn name(&self) -> &str {
        "terminal_signal"
    }

    fn description(&self) -> &str {
        "Send a signal to the terminal's process. Only `kill` is real on Windows — SIGINT/SIGTERM have no equivalent \
         here, so the tool rejects them instead of pretending. For a REPL that is stuck, `kill` then reopen."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "给终端进程发信号。Windows 上**只有 kill 是真的**——SIGINT/SIGTERM 在这里没有等价物，\
                     所以直接拒绝而不是假装支持。REPL 卡死时：kill，再重开一条。",
            "ja" => "ターミナルのプロセスにシグナルを送る。Windows で実際に効くのは **kill のみ**（SIGINT/SIGTERM に相当するものは無い）。\
                     REPL が固まったら kill して開き直す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "terminal_id": { "type": "string", "description": "终端 id" },
                "signal": { "type": "string", "enum": ["kill"], "description": "目前只支持 kill" }
            },
            "required": ["terminal_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        match input.get("signal").and_then(|v| v.as_str()) {
            None | Some("kill") => ValidationResult::success(None),
            Some(other) => ValidationResult::failure(
                &format!("signal={other} 在 Windows 上没有等价实现，只支持 kill"),
                2,
            ),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let h = match get_owned(id, ctx).await {
            Ok(h) => h,
            Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
        };
        h.proc.kill().await;
        h.alive.store(false, Ordering::Relaxed);
        ToolResult::standard_success(
            &format!("已终止终端 {id} 的进程。缓冲里已有的输出仍可用 terminal_read 读完。"),
            Some(json!({"terminal_id": id, "killed": true})),
        )
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
    fn search_hint(&self) -> &str {
        "terminal signal kill 终止 杀进程"
    }
}

pub struct TerminalCloseTool;
impl TerminalCloseTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TerminalCloseTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalCloseTool {
    fn name(&self) -> &str {
        "terminal_close"
    }

    fn description(&self) -> &str {
        "Close a persistent terminal: kill the process if it is still alive, then drop it from the registry. \
         Call it when you are done — idle shells hold a process and a pipe for the rest of the app's lifetime."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "关闭持久终端：进程还活着就杀掉，然后从注册表移除。用完就关——闲置的 shell 会一直占着一个进程和管道。",
            "ja" => "永続ターミナルを閉じる：生きていれば kill し、レジストリから外す。使い終わったら閉じること。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "terminal_id": { "type": "string", "description": "终端 id" } },
            "required": ["terminal_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args
            .get("terminal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let h = match get_owned(id, ctx).await {
            Ok(h) => h,
            Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
        };
        h.proc.kill().await;
        h.alive.store(false, Ordering::Relaxed);
        terms().lock().await.remove(id);
        ToolResult::standard_success(
            &format!("已关闭终端 {id}。"),
            Some(json!({"terminal_id": id, "closed": true})),
        )
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
    fn search_hint(&self) -> &str {
        "terminal close dispose 关闭终端 清理"
    }
}

// ============================================================================
// pwsh_persistent
// ============================================================================

/// 在持久 shell 里跑一条命令：状态（cd / 变量）跨调用保留。
pub struct PwshPersistentTool;
impl PwshPersistentTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for PwshPersistentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PwshPersistentTool {
    fn name(&self) -> &str {
        "pwsh_persistent"
    }

    fn description(&self) -> &str {
        "Run one PowerShell command in a persistent shell (state kept between calls: cwd, variables, loaded modules). \
         Returns the output produced by this command only. Use run_command for stateless one-shots; use this when \
         `cd` or environment setup must carry over."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在**持久 shell** 里跑一条 PowerShell 命令（cd / 变量 / 已加载模块跨调用保留），只返回这条命令新产生的输出。\
                     无状态的一次性命令用 run_command；需要 `cd` 或环境设置延续时用这个。",
            "ja" => "**永続シェル**で PowerShell コマンドを 1 つ実行する（cwd・変数・読み込み済みモジュールが呼び出しを跨いで残る）。\
                     このコマンドが生んだ出力だけ返す。状態不要の単発は run_command、cd や環境設定を引き継ぐならこちら。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "PowerShell 命令" },
                "terminal_id": { "type": "string", "description": "可选：复用已有终端；不传则用本会话最近的一条，没有就新建" },
                "settle_ms": { "type": "integer", "description": "输出检查间隔毫秒数（默认 25，上限 100）；命令完成即返回", "minimum": 100 }
            },
            "required": ["command"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("command 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let settle = args
            .get("settle_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(25)
            .clamp(10, 100);

        let handle = if let Some(id) = args.get("terminal_id").and_then(|v| v.as_str()) {
            match get_owned(id.trim(), ctx).await {
                Ok(h) => h,
                Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
            }
        } else {
            let existing = {
                let map = terms().lock().await;
                map.values()
                    .filter(|h| owns(h, ctx) && h.alive.load(Ordering::Relaxed))
                    .map(Arc::clone)
                    .next()
            };
            match existing {
                Some(h) => h,
                None => match open_terminal(ctx, &ctx.working_directory).await {
                    Ok(h) => h,
                    Err(e) => return ToolResult::standard_error(&e, Some("TerminalError"), None),
                },
            }
        };

        // 整个「清缓冲 → 写命令 → 等输出 → 读输出」是一个原子回合：
        if let Err(e) = check_terminal_scope(&handle, ctx) {
            return ToolResult::standard_error(&e, Some("TerminalPermissionChanged"), None);
        }
        // 同一终端上并发调用会排队，各自拿到属于自己的输出。
        let _turn = handle.cmd_lock.lock().await;
        // 先清掉历史输出，避免把上一次的残留当成这次的
        handle
            .buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .discard();
        use base64::Engine;
        let status = handle.proc.control_path();
        struct RunningCommand<'a> {
            process: &'a TermProc,
            status: std::path::PathBuf,
            finished: bool,
        }
        impl Drop for RunningCommand<'_> {
            fn drop(&mut self) {
                if !self.finished {
                    self.process.start_kill();
                }
                let _ = std::fs::remove_file(&self.status);
                let _ = std::fs::remove_file(self.status.with_extension("status.tmp"));
            }
        }
        let mut running = RunningCommand {
            process: &handle.proc,
            status: status.clone(),
            finished: false,
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
        let status_text = status.to_string_lossy().replace('\'', "''");
        // Publish the exit code by atomic rename. It cannot be confused with tool
        // output, and state from Invoke-Expression stays in this shell's scope.
        let wrapped = format!("$global:LASTEXITCODE=0; $__vivian_code=0; try {{ Invoke-Expression ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}'))); $__vivian_ok=$?; if ($LASTEXITCODE) {{ $__vivian_code=[int]$LASTEXITCODE }} elseif (-not $__vivian_ok) {{ $__vivian_code=1 }} }} catch {{ $__vivian_code=1; [Console]::Error.WriteLine($_.ToString()) }} finally {{ [IO.File]::WriteAllText('{status_text}.tmp', [string]$__vivian_code); [IO.File]::Move('{status_text}.tmp','{status_text}') }}");
        if let Err(e) = write_line(&handle, &wrapped).await {
            return ToolResult::standard_error(&e, Some("TerminalError"), None);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut out = String::new();
        let mut remaining = 8_000;
        let code = loop {
            let page = drain_new(&handle, remaining);
            remaining = remaining.saturating_sub(page.chars().count());
            out.push_str(&page);
            if let Ok(raw) = std::fs::read_to_string(&status) {
                if let Ok(code) = raw.trim().parse::<i32>() {
                    break code;
                }
            }
            if !handle.proc.is_alive().await {
                return ToolResult::standard_error(
                    "终端在命令完成前退出。",
                    Some("TerminalClosed"),
                    Some(json!({"output":out})),
                );
            }
            if std::time::Instant::now() >= deadline {
                return ToolResult::standard_error(
                    "持久终端命令执行超时（120 秒），已终止进程树。",
                    Some("timeout"),
                    Some(json!({"output":out})),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(settle)).await;
        };
        running.finished = true;
        // Allow pipe readers to collect bytes written immediately before status.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        out.push_str(&drain_new(&handle, remaining));
        let alive = handle.proc.is_alive().await;
        handle.alive.store(alive, Ordering::Relaxed);
        let truncated = handle
            .buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .has_unread();
        let display = if out.trim().is_empty() {
            format!("命令退出码 {code}（无输出）")
        } else {
            out.clone()
        };
        ToolResult::standard_success(
            &display,
            Some(json!({
                "terminal_id": handle.id, "output": out, "process_alive": alive,
                "command_completed": true, "exit_code": code, "success": code == 0, "truncated": truncated,
            })),
        )
    }

    fn execution_timeout(&self, _args: &Value) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(135))
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
    fn search_hint(&self) -> &str {
        "pwsh persistent shell keep state cd 持久 保留状态"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_cwd_is_checked_against_the_work_scope() {
        let ctx = ToolUseContext::default()
            .with_agent_kind("work")
            .with_working_directory("C:/audit/work");
        assert!(
            !TerminalOpenTool
                .validate_input(&json!({"cwd":"C:/outside"}), &ctx)
                .await
                .result
        );
        assert!(
            !TerminalOpenTool
                .validate_input(&json!({"cwd":"relative"}), &ctx)
                .await
                .result
        );
        assert!(
            TerminalOpenTool
                .validate_input(&json!({"cwd":"C:/audit/work/sub"}), &ctx)
                .await
                .result
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "launches Windows PowerShell; run explicitly in isolated test data"]
    async fn persistent_completion_ownership_paging_and_cancellation() {
        let ctx = ToolUseContext::default()
            .with_char_id("terminal-audit-a")
            .with_session_id(format!("terminal-{}", uuid::Uuid::new_v4()));
        let handle = open_terminal(&ctx, "").await.unwrap();
        let other = ctx.clone().with_char_id("terminal-audit-b");
        assert!(get_owned(&handle.id, &other).await.is_err());
        assert!(get_owned(&handle.id, &ctx.clone().with_agent_kind("work"))
            .await
            .is_err());
        let tool = PwshPersistentTool;
        let result = tool.call(json!({"terminal_id":handle.id,"command":"$n=40; Start-Sleep -Milliseconds 1100; $n+=2; [Console]::Write($n)"}), &ctx).await;
        assert!(result.success, "{result:?}");
        let data = &result.data.as_ref().unwrap()["data"];
        assert_eq!(data["exit_code"], 0);
        assert_eq!(data["output"], "42");
        let result = tool
            .call(
                json!({"terminal_id":handle.id,"command":"[Console]::Write($n); cmd /d /c exit 7"}),
                &ctx,
            )
            .await;
        assert_eq!(
            result.data.as_ref().unwrap()["data"]["exit_code"],
            7,
            "{result:?}"
        );
        let result = tool
            .call(
                json!({"terminal_id":handle.id,"command":"[Console]::Write(('x' * 10000))"}),
                &ctx,
            )
            .await;
        assert!(result.success, "{result:?}");
        assert_eq!(
            result.data.as_ref().unwrap()["data"]["output"]
                .as_str()
                .unwrap()
                .len(),
            8000
        );
        assert!(handle.buffer.lock().unwrap().has_unread());
        assert_eq!(drain_new(&handle, 8000).len(), 2000);
        let future = tool.call(
            json!({"terminal_id":handle.id,"command":"Start-Sleep -Seconds 30"}),
            &ctx,
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), future)
                .await
                .is_err()
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!handle.proc.is_alive().await);
        terms().lock().await.remove(&handle.id);
    }
}
