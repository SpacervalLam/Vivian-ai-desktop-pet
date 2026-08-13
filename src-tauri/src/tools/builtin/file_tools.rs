//! 文件系统工具 - 智能体读取本地文本类文件
//!
//! 当前工具：
//! - `read_file`：按绝对路径读取本地文本/代码/HTML 文件内容。
//!   所有路径操作都经过沙箱校验（`sandbox::is_path_safe` 防路径穿越 +
//!   `is_path_within_working_directory` 工作目录约束），只读、无副作用。
//!
//! 用途：用户给出本地文件路径（如"读一下 C:\xxx\note.html"）时，智能体可读取
//! 文件内容后配合 `create_html_note` 等工具将其转化为笔记。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::chat::read_text_with_encoding_detection;
use crate::tools::sandbox::{is_path_safe, is_path_within_working_directory};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 单次读取返回的最大字符数（默认值，防止超大文件撑爆上下文）
const DEFAULT_MAX_CHARS: usize = 20000;
/// 允许的最大读取上限（防止 LLM 传入离谱值）
const MAX_ALLOWED_CHARS: usize = 400000;

// ============================================================================
// ReadFileTool
// ============================================================================

/// 读取本地文件内容（只读，受沙箱路径校验约束）
///
/// 用户给出本地文件路径时调用。返回文件内容（带编码检测，兼容 UTF-8 / GBK /
/// Shift-JIS 等）。仅允许读取工作目录内的文件，且路径不得包含穿越序列。
pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a local text/code/HTML file by its absolute path and return its content. Use when the user gives you a file path (e.g. an .html file to turn into a note, or a code/config file to inspect). Encoding is auto-detected (UTF-8/GBK/Shift-JIS). Only files inside your working directory are allowed; paths with traversal are rejected. Returns the content (truncated if very large), filename, and char count."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按绝对路径读取本地文本/代码/HTML 文件内容并返回。当用户给出文件路径时使用（例如要转成笔记的 .html 文件，或需要查看的代码/配置文件）。自动检测编码（UTF-8/GBK/Shift-JIS）。仅允许读取工作目录内的文件，含路径穿越的请求会被拒绝。返回文件内容（过大时截断）、文件名与字符数。读取到 HTML 后如需保存为笔记，可配合 create_html_note 使用。",
            "ja" => "絶対パスでローカルのテキスト/コード/HTMLファイルを読み、内容を返す。ユーザーがファイルパスを提示したときに使う（例: ノートに変換する.htmlファイル、確認したいコード/設定ファイル）。エンコーディングは自動検出（UTF-8/GBK/Shift-JIS）。作業ディレクトリ内のファイルのみ読み取り可能で、パストラバーサルは拒否される。内容（大きすぎる場合は切り詰め）、ファイル名、文字数を返す。HTMLを読み取ってノートに保存する場合は create_html_note と組み合わせて使う。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要读取的文件绝对路径（如 C:\\Users\\xx\\note.html）"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "返回内容的最大字符数（默认 20000，上限 400000）",
                    "minimum": 100
                }
            },
            "required": ["path"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        if path.is_empty() {
            return ValidationResult::failure("path 不能为空", 2);
        }
        if !is_path_safe(path) {
            return ValidationResult::failure("路径包含穿越序列（..），已被沙箱拦截", 2);
        }
        if !is_path_within_working_directory(path, &ctx.working_directory) {
            return ValidationResult::failure(
                &format!("路径不在工作目录内，已拒绝读取: {}（工作目录: {}）", path, ctx.working_directory),
                2,
            );
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_CHARS)
            .max(DEFAULT_MAX_CHARS)
            .min(MAX_ALLOWED_CHARS);

        let p = std::path::Path::new(&path);
        if !p.exists() {
            return ToolResult::standard_error("文件不存在", None, None);
        }
        if !p.is_file() {
            return ToolResult::standard_error("目标不是文件", None, None);
        }

        let raw = match read_text_with_encoding_detection(p) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult::standard_error(&format!("读取文件失败: {}", e), None, None);
            }
        };
        let char_count = raw.chars().count();
        let (text, truncated) = if char_count > max_chars {
            let t: String = raw.chars().take(max_chars).collect();
            (t, true)
        } else {
            (raw, false)
        };

        let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        ToolResult::standard_success(
            &format!("已读取文件「{}」，共 {} 字符{}", filename, char_count, if truncated { "（因过长已截断）" } else { "" }),
            Some(json!({
                "filename": filename,
                "path": path,
                "content": text,
                "char_count": char_count,
                "truncated": truncated,
                "max_chars": max_chars,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::File
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_destructive(&self) -> bool {
        false
    }

    fn search_hint(&self) -> &str {
        "read local file path content text code html"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Calling it with a path outside your working directory — the sandbox will reject it; ask the user to move the file into the workspace first",
            "Reading binary files (images, audio, archives) — use the dedicated image/OCR tools instead",
        ]
    }
}