//! 文件系统工具 - 智能体读取本地文本类文件
//!
//! 当前工具：
//! - `read_file`：按绝对路径读取本地文本/代码/HTML 文件内容。
//!   所有路径操作都经过沙箱校验（`sandbox::is_path_safe` 防路径穿越 +
//!   `ToolUseContext::is_path_authorized` 工作区归属约束），只读、无副作用。
//!
//! 用途：用户给出本地文件路径（如"读一下 C:\xxx\note.html"）时，智能体可读取
//! 文件内容后配合 `create_html_note` 等工具将其转化为笔记。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::chat::read_text_with_encoding_detection;
use crate::tools::sandbox::is_path_safe;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 单次读取返回的最大字符数（默认值，防止超大文件撑爆上下文）
const DEFAULT_MAX_CHARS: usize = 20000;
/// 允许的最大读取上限（防止 LLM 传入离谱值）
const MAX_ALLOWED_CHARS: usize = 400000;

fn page_arguments(args: &Value) -> Result<(usize, usize), &'static str> {
    let offset = match args.get("offset") {
        None => 0,
        Some(value) => value.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or("offset 必须为非负整数")?,
    };
    let max_chars = match args.get("max_chars") {
        None => DEFAULT_MAX_CHARS,
        Some(value) => value.as_u64().and_then(|v| usize::try_from(v).ok())
            .filter(|v| (100..=MAX_ALLOWED_CHARS).contains(v)).ok_or("max_chars 必须为 100 到 400000 的整数")?,
    };
    Ok((offset, max_chars))
}

struct TextPage {
    content: String,
    returned_chars: usize,
    next_offset: Option<usize>,
}

fn text_page(raw: &str, offset: usize, max_chars: usize) -> TextPage {
    let total = raw.chars().count();
    let content: String = raw.chars().skip(offset).take(max_chars).collect();
    let returned_chars = content.chars().count();
    let end = offset.saturating_add(returned_chars);
    TextPage { content, returned_chars, next_offset: (end < total).then_some(end) }
}

/// Reads only host-generated result artifacts by basename; this is not a workspace bypass.
pub struct ReadSpilledResultTool;

fn resolve_spill(root: &std::path::Path, id: &str) -> Result<std::path::PathBuf, String> {
    if id.is_empty() || id.contains(['/', '\\', ':']) || !id.ends_with(".txt") || id.contains("..") {
        return Err("spill_id 必须是回执中的文件名，不能包含路径".into());
    }
    let root = root.canonicalize().map_err(|e| format!("结果目录不可用: {e}"))?;
    let target = root.join(id).canonicalize().map_err(|e| format!("结果不存在或已过期: {e}"))?;
    if target.parent() != Some(root.as_path()) || !target.is_file() {
        return Err("结果文件不在宿主结果目录内".into());
    }
    Ok(target)
}

#[async_trait]
impl Tool for ReadSpilledResultTool {
    fn name(&self) -> &str { "read_spilled_result" }
    fn description(&self) -> &str {
        "Read a host-saved tool result in character pages. Use only the spill_id returned in a truncated tool receipt. Pass next_offset to continue; an expired or missing result is a failure."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object", "properties": {
            "spill_id":{"type":"string","description":"Exact spill_id from a tool receipt, never an arbitrary file path"},
            "offset":{"type":"integer","minimum":0},
            "max_chars":{"type":"integer","minimum":100,"maximum":MAX_ALLOWED_CHARS}
        }, "required":["spill_id"]})
    }
    async fn validate_input(&self, args: &Value, _: &ToolUseContext) -> ValidationResult {
        if let Err(message) = page_arguments(args) { return ValidationResult::failure(message, 2); }
        let id = args.get("spill_id").and_then(Value::as_str).unwrap_or_default();
        match resolve_spill(&crate::utils::path::get_user_data_dir().join("spill"), id) {
            Ok(_) => ValidationResult::success(None),
            Err(message) => ValidationResult::failure(message, 2),
        }
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult { PermissionResult::allow() }
    async fn call(&self, args: Value, _: &ToolUseContext) -> ToolResult {
        let id = args.get("spill_id").and_then(Value::as_str).unwrap_or_default().to_string();
        let (offset, requested) = match page_arguments(&args) {
            Ok(page) => page,
            Err(message) => return ToolResult::standard_error(message, Some("InvalidInput"), None),
        };
        let path = match resolve_spill(&crate::utils::path::get_user_data_dir().join("spill"), &id) {
            Ok(path) => path,
            Err(message) => return ToolResult::standard_error(&message, Some("SpillUnavailable"), None),
        };
        let raw = match tokio::fs::read_to_string(path).await {
            Ok(raw) => raw,
            Err(error) => return ToolResult::standard_error(&error.to_string(), Some("SpillReadFailed"), None),
        };
        let page = text_page(&raw, offset, crate::tools::executor::text_page_budget(requested));
        ToolResult::standard_success("Host result page", Some(json!({"spill_id":id,
            "content":page.content,"offset":offset,"returned_chars":page.returned_chars,
            "char_count":raw.chars().count(),"next_offset":page.next_offset,"truncated":page.next_offset.is_some()})))
    }
    fn is_read_only(&self) -> bool { true }
    fn category(&self) -> ToolCategory { ToolCategory::File }
    fn risk(&self) -> ToolRiskTier { ToolRiskTier::FsRead }
    fn always_load(&self) -> bool { true }
}

// ============================================================================
// ReadFileTool
// ============================================================================

/// 读取本地文件内容（只读，受沙箱路径校验约束）
///
/// 用户给出本地文件路径时调用。返回文件内容（带编码检测，兼容 UTF-8 / GBK /
/// Shift-JIS 等）。仅允许读取工作目录内的文件，且路径不得包含穿越序列。
pub struct ReadFileTool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_pages_have_no_gaps_or_duplicates() {
        let raw = "甲🙂乙ab\n丙";
        let first = text_page(raw, 0, 3);
        assert_eq!(first.content, "甲🙂乙");
        assert_eq!(first.next_offset, Some(3));
        let second = text_page(raw, 3, 100);
        assert_eq!(first.content + &second.content, raw);
        assert!(second.next_offset.is_none());
        let eof = text_page(raw, usize::MAX, 100);
        assert!(eof.content.is_empty() && eof.next_offset.is_none());
    }

    #[test]
    fn page_parameters_do_not_silently_expand_small_requests() {
        assert_eq!(page_arguments(&json!({"max_chars":100})).unwrap(), (0,100));
        for args in [json!({"offset":-1}),json!({"offset":"2"}),json!({"max_chars":99}),json!({"max_chars":400001})] {
            assert!(page_arguments(&args).is_err());
        }
    }

    #[tokio::test]
    async fn real_file_can_be_read_in_small_unicode_pages() {
        let file = std::env::temp_dir().join(format!("vivian-read-{}.txt",uuid::Uuid::new_v4()));
        let raw = "甲🙂乙".repeat(90);
        std::fs::write(&file, &raw).unwrap();
        let tool = ReadFileTool;
        let ctx = ToolUseContext { working_directory: std::env::temp_dir().to_string_lossy().into(), ..Default::default() };
        let mut offset = 0;
        let mut observed = String::new();
        loop {
            let args = json!({"path":file,"offset":offset,"max_chars":100});
            assert!(tool.validate_input(&args,&ctx).await.result);
            let result = tool.call(args,&ctx).await;
            assert!(result.success);
            let page = &result.data.as_ref().unwrap()["data"];
            let content = page["content"].as_str().unwrap();
            assert!(content.chars().count() <= 100);
            observed.push_str(content);
            match page["next_offset"].as_u64() { Some(next) => offset=next, None => break }
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!(observed,raw);
    }

    #[test]
    fn spill_reader_rejects_paths_and_expired_ids() {
        let root = std::env::temp_dir();
        for id in ["../outside.txt","C:\\outside.txt","nested/x.txt","missing-result.txt",""] {
            assert!(resolve_spill(&root,id).is_err());
        }
    }

    #[tokio::test]
    async fn saved_tool_result_is_recoverable_through_executor() {
        let raw = json!({"fact":"证据🙂".repeat(500)});
        let compact = crate::tools::executor::budget_result("pagination_test".into(),raw.clone(),500);
        let id = compact["spill_id"].as_str().unwrap();
        let system = crate::tools::registry::ToolSystem::new();
        system.register_tool(std::sync::Arc::new(ReadSpilledResultTool));
        let ctx = ToolUseContext::default();
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let result = crate::tools::executor::execute_tool_use("read_spilled_result",
                json!({"spill_id":id,"offset":offset,"max_chars":100}),&system,&ctx,None).await;
            assert!(result.success,"{result:?}");
            let page = &result.data.as_ref().unwrap()["data"];
            recovered.push_str(page["content"].as_str().unwrap());
            match page["next_offset"].as_u64() { Some(next) => offset=next, None => break }
        }
        std::fs::remove_file(compact["spill_path"].as_str().unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&recovered).unwrap(),raw);
    }
}

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
        "Read a local text/code/HTML file by absolute path. Encoding is auto-detected. Only authorized workspace files are allowed. offset and max_chars count Unicode characters; use next_offset to continue when truncated=true. Keep the same path across pages."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按绝对路径读取已授权工作区内的文本、代码或 HTML，自动检测编码。offset 与 max_chars 均按 Unicode 字符计数；truncated=true 时使用返回的 next_offset 和同一路径继续读取，不能把预览当作全文。",
            "ja" => "絶対パスでローカルのテキスト/コード/HTMLファイルを読み、内容を返す。ユーザーがファイルパスを提示したときに使う（例: ノートに変換する.htmlファイル、確認したいコード/設定ファイル）。エンコーディングは自動検出（UTF-8/GBK/Shift-JIS）。作業ディレクトリ内のファイルのみ読み取り可能で、パストラバーサルは拒否される。内容（大きすぎる場合は切り詰め）、ファイル名、文字数を返す。HTMLを読み取ってノートに保存する場合は create_html_note と組み合わせて使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "读一下这个文件\n看看文件内容\n打开这个文件",
            "en" => "read this file\nshow me the file contents\nopen this file",
            "ja" => "このファイルを読んで\nファイルの内容を見せて\nファイルを開いて",
            _ => "",
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
                    "minimum": 100,
                    "maximum": MAX_ALLOWED_CHARS
                },
                "offset": {
                    "type": "integer", "minimum": 0,
                    "description": "字符起点，默认 0；续读时传入上次 next_offset"
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
        if !ctx.is_path_authorized(path) {
            return ValidationResult::failure(
                &format!("路径不在任何已授权工作区内，已拒绝读取: {}", path),
                2,
            );
        }
        if let Err(message) = page_arguments(input) {
            return ValidationResult::failure(message, 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let (offset, requested_chars) = match page_arguments(&args) {
            Ok(page) => page,
            Err(message) => return ToolResult::standard_error(message, Some("InvalidInput"), None),
        };
        let max_chars = crate::tools::executor::text_page_budget(requested_chars);

        let p = std::path::Path::new(&path);
        if !p.exists() {
            return ToolResult::standard_error("文件不存在", None, None);
        }
        if !p.is_file() {
            return ToolResult::standard_error("目标不是文件", None, None);
        }

        let read_path = p.to_path_buf();
        let raw = match tokio::task::spawn_blocking(move || read_text_with_encoding_detection(&read_path)).await {
            Ok(Ok(s)) => s,
            other => {
                let e = match other { Ok(Err(e)) => e.to_string(), Err(e) => e.to_string(), _ => unreachable!() };
                return ToolResult::standard_error(&format!("读取文件失败: {}", e), None, None);
            }
        };
        let char_count = raw.chars().count();
        let page = text_page(&raw, offset, max_chars);

        let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        ToolResult::standard_success(
            &format!("已读取文件「{}」的字符分页，共 {} 字符", filename, char_count),
            Some(json!({
                "filename": filename,
                "path": path,
                "content": page.content,
                "char_count": char_count,
                "truncated": page.next_offset.is_some(),
                "offset": offset,
                "returned_chars": page.returned_chars,
                "next_offset": page.next_offset,
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
        ToolRiskTier::FsRead
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
