//! LSP 能力缝 —— 四个语义操作（goToDefinition / findReferences / goToImplementation / hover）。
//!
//! 按文件扩展路由到语言服务器（stdio JSON-RPC），配置驱动的提供方注册：
//! `<用户数据目录>/lsp.json` 声明 `{ "<扩展名>": { "command": ["命令", "参数"...] } }`。
//! 未配置扩展返回明确错误。无通用 JSON-RPC 逃生口——只暴露四个语义操作。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// LSP 语义操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LspQueryKind {
    GoToDefinition,
    FindReferences,
    GoToImplementation,
    Hover,
}

impl LspQueryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LspQueryKind::GoToDefinition => "go_to_definition",
            LspQueryKind::FindReferences => "find_references",
            LspQueryKind::GoToImplementation => "go_to_implementation",
            LspQueryKind::Hover => "hover",
        }
    }
}

/// 一次语义查询的位置参数。
#[derive(Debug, Clone, Serialize)]
pub struct LspQuery {
    pub kind: LspQueryKind,
    /// 文件绝对路径
    pub file: String,
    /// 行（0 基）
    pub line: u32,
    /// 列（0 基）
    pub column: u32,
}

/// 语言服务器提供方配置（按文件扩展声明启动命令）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspProviderConfig {
    /// 启动语言服务器的命令行（如 ["rust-analyzer"] / ["pylsp"]）
    pub command: Vec<String>,
}

/// LSP 服务：配置加载 + 单次连接查询。
///
/// 每次查询启动一个语言服务器实例（initialize → 请求 → shutdown）。这对轻量查询
/// 简单可靠；长驻连接待高频使用时再引入。
pub struct LspService {
    providers: RwLock<BTreeMap<String, LspProviderConfig>>,
    config_path: PathBuf,
}

impl LspService {
    pub fn new() -> Arc<Self> {
        let config_path = crate::utils::path::get_user_data_dir().join("lsp.json");
        let providers =
            crate::utils::fs::load_json_or_backup::<BTreeMap<String, LspProviderConfig>>(
                &config_path,
            )
            .unwrap_or_default();
        Arc::new(Self {
            providers: RwLock::new(providers),
            config_path,
        })
    }

    /// 已配置的扩展列表。
    pub fn configured_extensions(&self) -> Vec<String> {
        self.providers.read().keys().cloned().collect()
    }

    /// 更新提供方配置（运行时热更新）。
    pub fn update_config(&self, providers: BTreeMap<String, LspProviderConfig>) {
        *self.providers.write() = providers.clone();
        if let Ok(json) = serde_json::to_string_pretty(&providers) {
            let _ = std::fs::write(&self.config_path, json);
        }
    }

    /// 执行一次语义查询。
    pub async fn query(&self, q: &LspQuery) -> Result<Value, String> {
        let ext = PathBuf::from(&q.file)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let provider = {
            let providers = self.providers.read();
            providers.get(&ext).cloned()
        };
        let Some(provider) = provider else {
            return Err(format!(
                "扩展 .{ext} 未配置语言服务器。可在 lsp.json 中为该扩展声明提供方命令。已配置：{:?}",
                self.configured_extensions()
            ));
        };
        if provider.command.is_empty() {
            return Err(format!(".{ext} 的语言服务器命令为空"));
        }

        let mut child = crate::utils::process::silent_command_async(&provider.command[0]);
        child
            .args(&provider.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = child.spawn().map_err(|e| format!("启动语言服务器失败: {e}"))?;
        let _ = crate::utils::process::assign_child_to_job(&child);
        let mut stdin = child.stdin.take().ok_or("无法取得 stdin")?;
        let mut stdout = child.stdout.take().ok_or("无法取得 stdout")?;

        let uri = format!("file:///{}", q.file.replace('\\', "/"));
        let file_uri = uri.clone();

        // initialize
        let init_req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": Value::Null,
                "capabilities": {},
            }
        });
        send_lsp(&mut stdin, &init_req).await?;
        // 读 initialize 响应（直到 Content-Length 消息处理完）
        let _init_resp = read_lsp(&mut stdout).await?;

        // initialized 通知
        send_lsp(&mut stdin, &json!({"jsonrpc":"2.0","method":"initialized","params":{}})).await?;

        // 语义请求（definition / references / implementation / hover）
        let (method, params) = match q.kind {
            LspQueryKind::GoToDefinition => (
                "textDocument/definition",
                json!({"textDocument": {"uri": file_uri}, "position": {"line": q.line, "character": q.column}}),
            ),
            LspQueryKind::FindReferences => (
                "textDocument/references",
                json!({
                    "textDocument": {"uri": file_uri},
                    "position": {"line": q.line, "character": q.column},
                    "context": {"includeDeclaration": true}
                }),
            ),
            LspQueryKind::GoToImplementation => (
                "textDocument/implementation",
                json!({"textDocument": {"uri": file_uri}, "position": {"line": q.line, "character": q.column}}),
            ),
            LspQueryKind::Hover => (
                "textDocument/hover",
                json!({"textDocument": {"uri": file_uri}, "position": {"line": q.line, "character": q.column}}),
            ),
        };
        let query_req = json!({"jsonrpc":"2.0","id":2,"method":method,"params":params});
        send_lsp(&mut stdin, &query_req).await?;

        // 读到 id=2 的响应
        let mut result = Value::Null;
        for _ in 0..5 {
            match read_lsp(&mut stdout).await {
                Ok(Some(resp)) => {
                    if resp.get("id") == Some(&json!(2)) {
                        result = resp.get("result").cloned().unwrap_or(Value::Null);
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        // shutdown / exit
        let _ = send_lsp(&mut stdin, &json!({"jsonrpc":"2.0","id":3,"method":"shutdown"})).await;
        let _ = send_lsp(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"})).await;
        drop(stdin);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await;

        Ok(result)
    }
}

/// 发送一条 LSP 消息（Content-Length 帧）。
async fn send_lsp(
    stdin: &mut tokio::process::ChildStdin,
    msg: &Value,
) -> Result<(), String> {
    let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    stdin
        .write_all(frame.as_bytes())
        .await
        .map_err(|e| format!("写入 LSP 失败: {e}"))
}

/// 读取一条 LSP 消息；流结束时返回 Ok(None)。
async fn read_lsp(
    stdout: &mut tokio::process::ChildStdout,
) -> Result<Option<Value>, String> {
    // 读头直到空行
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stdout.read(&mut byte).await {
            Ok(0) => return Ok(None),
            Ok(_) => {
                header.push(byte[0]);
                if header.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(e) => return Err(format!("读取 LSP 头失败: {e}")),
        }
    }
    let header_str = String::from_utf8_lossy(&header);
    let content_length: usize = header_str
        .lines()
        .find_map(|l| {
            let lower = l.to_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                rest.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .ok_or("LSP 头缺少 Content-Length")?;
    let mut body = vec![0u8; content_length];
    stdout
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("读取 LSP 体失败: {e}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| format!("解析 LSP 消息失败: {e}"))
}
