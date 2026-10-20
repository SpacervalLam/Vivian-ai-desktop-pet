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

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

const MAX_BUFFER_CHARS: usize = 200_000;

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
    id: String,
    session_id: String,
    cwd: String,
    proc: TermProc,
    /// 命令级互斥：同一终端上一次只允许一个"写命令→等输出→读输出"的完整回合。
    ///
    /// 没有它的话，两个 pwsh_persistent 并发时输出会交错，各自读到对方的片段，
    /// 拼出来的结果谁也解释不了。terminal_send / terminal_read 也拿这把锁，
    /// 否则一条 send 会插进别人正在等的窗口里。
    cmd_lock: tokio::sync::Mutex<()>,
    /// 累积输出（保留尾部 MAX_BUFFER_CHARS）+ 已读游标
    buffer: Arc<Mutex<String>>,
    cursor: Arc<Mutex<usize>>,
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
fn spawn_reader<R>(reader: R, buffer: Arc<Mutex<String>>, alive: Arc<AtomicBool>, tag: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let mut b = buffer.lock().unwrap_or_else(|e| e.into_inner());
                    b.push_str(tag);
                    b.push_str(&line);
                    b.push('\n');
                    let len = b.chars().count();
                    if len > MAX_BUFFER_CHARS {
                        *b = b.chars().skip(len - MAX_BUFFER_CHARS / 2).collect();
                    }
                }
                _ => {
                    alive.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    });
}

/// 起一个普通 PowerShell 子进程（非沙箱路径）
async fn spawn_plain(
    buffer: &Arc<Mutex<String>>,
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
    let mut child = cmd.spawn().map_err(|e| format!("启动 PowerShell 失败：{e}"))?;
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
    reader: std::fs::File,
    buffer: Arc<Mutex<String>>,
    alive: Arc<AtomicBool>,
    tag: &'static str,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut br = std::io::BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match br.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    alive.store(false, Ordering::Relaxed);
                    break;
                }
                Ok(_) => {
                    let mut b = buffer.lock().unwrap_or_else(|e| e.into_inner());
                    b.push_str(tag);
                    b.push_str(line.trim_end_matches(['\r', '\n']));
                    b.push('\n');
                    let len = b.chars().count();
                    if len > MAX_BUFFER_CHARS {
                        *b = b.chars().skip(len - MAX_BUFFER_CHARS / 2).collect();
                    }
                }
            }
        }
    });
}

async fn open_terminal(ctx: &ToolUseContext, cwd: &str) -> Result<Arc<TermHandle>, String> {
    // 非 Windows 上没有受限令牌可用；配置要求必须有隔离时就地拒绝，
    // 不开一个全权限的长活 shell 糊过去。
    #[cfg(not(windows))]
    if crate::tools::sandbox_env::require_isolation() {
        return Err(
            "当前环境没有可用的 OS 级沙箱，已按配置拒绝打开终端。若要使用请关闭「要求 OS 级隔离」。"
                .to_string(),
        );
    }
    let id = next_term_id();
    let buffer = Arc::new(Mutex::new(String::new()));
    let cursor = Arc::new(Mutex::new(0usize));
    let alive = Arc::new(AtomicBool::new(true));

    #[cfg(windows)]
    let proc = {
        if crate::tools::sandbox_env::sandbox_terminal()
            && crate::tools::sandbox_env::enabled()
        {
            let req = crate::tools::sandbox_env::request_for(ctx);
            let dir = if cwd.trim().is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(cwd))
            };
            let shell = crate::utils::sandbox::spawn_sandboxed_shell(&req, dir.as_deref())?;
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
        id: id.clone(),
        session_id: ctx.session_id.clone(),
        cwd: cwd.to_string(),
        cmd_lock: tokio::sync::Mutex::new(()),
        proc,
        buffer,
        cursor,
        alive,
    });

    // 首条命令把编码切到 UTF-8（与 run_command 同一套理由）
    let _ = write_line(
        &handle,
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[Console]::OutputEncoding",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    terms().lock().await.insert(id, Arc::clone(&handle));
    Ok(handle)
}

/// 取走自上次读取以来的新输出。
fn drain_new(h: &TermHandle) -> String {
    let buffer = h.buffer.lock().unwrap_or_else(|e| e.into_inner());
    let mut cursor = h.cursor.lock().unwrap_or_else(|e| e.into_inner());
    if *cursor > buffer.len() {
        *cursor = buffer.len();
    }
    let slice = buffer[*cursor..].to_string();
    *cursor = buffer.len();
    slice
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
        None => Err(format!("没有 id 为 {id} 的终端。用 terminal_list 看现有终端，或 terminal_open 新建。")),
        Some(h) if h.session_id != ctx.session_id => Err("该终端不属于本会话。".to_string()),
        Some(h) => Ok(Arc::clone(h)),
    }
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

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
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
        if input.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let h = match get_owned(id, ctx).await {
            Ok(h) => h,
            Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
        };
        if !h.alive.load(Ordering::Relaxed) {
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
        if input.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let wait_ms = args.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(800).min(15_000);
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
        let mut out = drain_new(&h);
        while out.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            out = drain_new(&h);
        }
        let alive = h.alive.load(Ordering::Relaxed);
        let truncated = out.chars().count() > max_chars;
        let body: String = out.chars().take(max_chars).collect();
        if body.trim().is_empty() {
            return ToolResult::standard_success(
                &format!(
                    "（{}内没有新输出；进程{}）",
                    if wait_ms == 0 { "此刻".to_string() } else { format!("{wait_ms}ms") },
                    if alive { "仍在运行" } else { "已退出" }
                ),
                Some(json!({"terminal_id": id, "output": "", "process_alive": alive})),
            );
        }
        ToolResult::standard_success(
            &format!("{body}{}", if truncated { "\n（输出已截断）" } else { "" }),
            Some(json!({"terminal_id": id, "output": body, "truncated": truncated, "process_alive": alive})),
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
            "ja" => "このセッションの永続ターミナル一覧：id・作業ディレクトリ・プロセスが生きているか。",
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
            .filter(|h| h.session_id == ctx.session_id)
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
                if r["alive"].as_bool().unwrap_or(false) { "运行中" } else { "已退出" }
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
        if input.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
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
        let id = args.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim();
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
        if input.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("terminal_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let id = args.get("terminal_id").and_then(|v| v.as_str()).unwrap_or("").trim();
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
                "settle_ms": { "type": "integer", "description": "等待输出稳定的毫秒数（默认 700）", "minimum": 100 }
            },
            "required": ["command"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("command 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let settle = args.get("settle_ms").and_then(|v| v.as_u64()).unwrap_or(700).clamp(100, 15_000);

        let handle = if let Some(id) = args.get("terminal_id").and_then(|v| v.as_str()) {
            match get_owned(id.trim(), ctx).await {
                Ok(h) => h,
                Err(e) => return ToolResult::standard_error(&e, Some("TerminalNotFound"), None),
            }
        } else {
            let existing = {
                let map = terms().lock().await;
                map.values()
                    .filter(|h| h.session_id == ctx.session_id && h.alive.load(Ordering::Relaxed))
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
        // 同一终端上并发调用会排队，各自拿到属于自己的输出。
        let _turn = handle.cmd_lock.lock().await;
        // 先清掉历史输出，避免把上一次的残留当成这次的
        let _ = drain_new(&handle);
        if let Err(e) = write_line(&handle, &command).await {
            return ToolResult::standard_error(&e, Some("TerminalError"), None);
        }
        tokio::time::sleep(std::time::Duration::from_millis(settle)).await;
        let mut out = drain_new(&handle);
        // 输出可能还在滚动：再补一次短等待，拿"稳定后"的结果
        if !out.trim_end().ends_with("PS>") && !out.trim().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            out.push_str(&drain_new(&handle));
        }
        let alive = handle.alive.load(Ordering::Relaxed);
        let body: String = out.chars().take(8_000).collect();
        let display = if body.trim().is_empty() {
            "（无输出）".to_string()
        } else {
            body.clone()
        };
        ToolResult::standard_success(
            &display,
            Some(json!({
                "terminal_id": handle.id,
                "output": body,
                "process_alive": alive,
            })),
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
        "pwsh persistent shell keep state cd 持久 保留状态"
    }
}
