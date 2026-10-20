//! 编辑与看图工具：`str_replace_editor`、`read_image`。
//!
//! `str_replace_editor` 是 Claude Code 风格的多动作编辑器（view / create / str_replace / insert / undo）：
//! 与 `edit_file` 的区别是**能撤销**——改错了不用靠记忆反推原文。
//!
//! `read_image` 让模型真的"看见"图：图片不走 JSON 文本（base64 塞进 data 会吃满上下文），
//! 而是放进 `ToolResult::images`，由调用方映射成 provider 的 image part。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use crate::brain::coding_agent::CodingImage;
use crate::tools::sandbox::is_path_safe;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 单张图片上限（超过就拒绝：base64 之后还会膨胀 1/3，塞进上下文不划算）
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

fn resolve_path(raw: &str, ctx: &ToolUseContext) -> String {
    let p = std::path::Path::new(raw);
    if p.is_absolute() || ctx.working_directory.trim().is_empty() {
        raw.to_string()
    } else {
        std::path::Path::new(&ctx.working_directory)
            .join(p)
            .to_string_lossy()
            .to_string()
    }
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    None
}

// ============================================================================
// read_image
// ============================================================================

pub struct ReadImageTool;

impl ReadImageTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for ReadImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &str {
        "read_image"
    }

    fn description(&self) -> &str {
        "Open an image file and actually look at it. Supports png / jpeg / gif / webp / bmp up to 8 MiB. \
         Use it whenever the answer depends on what is IN the picture — UI screenshots, generated art, charts, \
         error dialogs, design references. Describing an image you never loaded is guessing."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "打开一张图片并**真的看它**（png / jpeg / gif / webp / bmp，上限 8 MiB）。\
                     凡是答案取决于图里画了什么——UI 截图、生成的图、图表、报错弹窗、设计参考——都用它。\
                     没加载图就描述图像，那是猜。",
            "ja" => "画像ファイルを開いて**実際に見る**（png / jpeg / gif / webp / bmp、上限 8 MiB）。\
                     答えが「画像に何が写っているか」に依存する場合——UI スクショ・生成画像・グラフ・エラーダイアログ・参考デザイン——に使う。\
                     読み込まずに説明するのは推測にすぎない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "看看这张图\n截图里是什么\n这张图哪里不对",
            "en" => "look at this image\nwhat's in the screenshot\nwhat's wrong in this picture",
            "ja" => "この画像を見て\nスクショの内容は\nどこがおかしい",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "图片路径（绝对路径，或相对工作目录）" }
            },
            "required": ["path"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        if path.is_empty() {
            return ValidationResult::failure("path 不能为空", 2);
        }
        let resolved = resolve_path(path, ctx);
        if !is_path_safe(&resolved) {
            return ValidationResult::failure("路径包含穿越序列（..），已被沙箱拦截", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        let path = resolve_path(raw, ctx);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("读不到 {path}：{e}。先用 glob / list_dir 确认路径。"),
                    Some("NotFound"),
                    None,
                )
            }
        };
        if meta.len() as usize > MAX_IMAGE_BYTES {
            return ToolResult::standard_error(
                &format!(
                    "图片过大（{:.1} MiB，上限 8 MiB）：{path}。先缩小/裁切再读。",
                    meta.len() as f64 / 1_048_576.0
                ),
                Some("InvalidInput"),
                None,
            );
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                return ToolResult::standard_error(&format!("读取失败：{e}"), None, None);
            }
        };
        let Some(mime) = detect_image_mime(&bytes) else {
            return ToolResult::standard_error(
                &format!("{path} 不是可识别的图片格式（支持 png/jpeg/gif/webp/bmp）。"),
                Some("InvalidInput"),
                None,
            );
        };
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string());

        let mut result = ToolResult::standard_success(
            &format!(
                "已读取图片 {path}（{mime}，{} KiB）。图片内容已随本次工具结果送达，直接基于画面回答，不要靠猜。",
                bytes.len() / 1024
            ),
            Some(json!({
                "path": path,
                "media_type": mime,
                "bytes": bytes.len(),
            })),
        );
        result.images = Some(vec![CodingImage {
            media_type: mime.to_string(),
            data,
            name,
        }]);
        result
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
    fn search_hint(&self) -> &str {
        "read image look at picture screenshot 看图 图片 截图 画了什么"
    }
}

// ============================================================================
// str_replace_editor
// ============================================================================

type UndoKey = String;

fn undo_stack() -> &'static Mutex<HashMap<UndoKey, Vec<String>>> {
    static STACK: OnceLock<Mutex<HashMap<UndoKey, Vec<String>>>> = OnceLock::new();
    STACK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn undo_key(session_id: &str, path: &str) -> UndoKey {
    format!("{session_id}|{path}")
}

fn push_undo(session_id: &str, path: &str, previous: String) {
    let mut stack = undo_stack().lock().unwrap_or_else(|e| e.into_inner());
    let entry = stack.entry(undo_key(session_id, path)).or_default();
    entry.push(previous);
    // 只保留最近 20 次，避免长跑会话堆积整份文件副本
    if entry.len() > 20 {
        entry.remove(0);
    }
}

pub struct StrReplaceEditorTool;

impl StrReplaceEditorTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for StrReplaceEditorTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for StrReplaceEditorTool {
    fn name(&self) -> &str {
        "str_replace_editor"
    }

    fn description(&self) -> &str {
        "Multi-action file editor with undo. Actions:\n\
         - view: show a file with line numbers (optional line range)\n\
         - create: create a NEW file (fails if it exists)\n\
         - str_replace: replace an exact string (set replace_all for every occurrence)\n\
         - insert: insert text after a given line number\n\
         - undo: revert the most recent edit to this file in this session\n\
         Prefer this over edit_file when you may need to roll back, or when you want line numbers to reason about."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "带撤销的多动作文件编辑器：\n\
                     - view：带行号查看（可指定行区间）\n\
                     - create：新建文件（已存在则失败）\n\
                     - str_replace：精确字符串替换（replace_all 替换全部）\n\
                     - insert：在第 N 行之后插入\n\
                     - undo：撤销本会话内对该文件的最近一次修改\n\
                     可能改错需要回退、或者需要行号推理时，优先用它而不是 edit_file。",
            "ja" => "取り消し可能な多機能エディタ：\n\
                     - view：行番号付きで表示（範囲指定可）\n\
                     - create：新規作成（既存なら失敗）\n\
                     - str_replace：完全一致の文字列置換（replace_all で全置換）\n\
                     - insert：指定行の後ろに挿入\n\
                     - undo：このセッションで当該ファイルに加えた直近の変更を取り消す\n\
                     戻す可能性があるとき、行番号で考えるときは edit_file よりこちら。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "改这一行\n把这段替换掉\n撤销刚才的修改\n新建一个文件",
            "en" => "change this line\nreplace that block\nundo my last edit\ncreate a file",
            "ja" => "この行を変えて\nその部分を置換\n直前の変更を取り消し\nファイルを作成",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["view", "create", "str_replace", "insert", "undo"],
                    "description": "要执行的动作"
                },
                "path": { "type": "string", "description": "文件路径（绝对或相对工作目录）" },
                "old_string": { "type": "string", "description": "str_replace：要被替换的原文（必须完全一致，含缩进）" },
                "new_string": { "type": "string", "description": "str_replace / insert：替换成什么 / 插入什么" },
                "replace_all": { "type": "boolean", "description": "str_replace：是否替换全部出现（默认 false，且要求唯一）" },
                "file_text": { "type": "string", "description": "create：文件的完整内容" },
                "line": { "type": "integer", "description": "insert：在这一行之后插入（1-based，0 表示文件开头）" },
                "view_start": { "type": "integer", "description": "view：起始行（1-based，可空）" },
                "view_end": { "type": "integer", "description": "view：结束行（1-based，可空）" }
            },
            "required": ["action", "path"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
        if !matches!(action, "view" | "create" | "str_replace" | "insert" | "undo") {
            return ValidationResult::failure(
                "action 必须是 view / create / str_replace / insert / undo",
                2,
            );
        }
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        if path.is_empty() {
            return ValidationResult::failure("path 不能为空", 2);
        }
        if !is_path_safe(&resolve_path(path, ctx)) {
            return ValidationResult::failure("路径包含穿越序列（..），已被沙箱拦截", 2);
        }
        if action == "str_replace" {
            if input.get("old_string").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                return ValidationResult::failure("str_replace 需要 old_string", 2);
            }
        }
        if action == "create" && input.get("file_text").and_then(|v| v.as_str()).is_none() {
            return ValidationResult::failure("create 需要 file_text", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("view").trim();
        let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        let path = resolve_path(raw, ctx);

        match action {
            "view" => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        return ToolResult::standard_error(
                            &format!("读不到 {path}：{e}"),
                            Some("NotFound"),
                            None,
                        )
                    }
                };
                let start = args.get("view_start").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let end = args
                    .get("view_end")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(usize::MAX);
                let lines: Vec<&str> = text.lines().collect();
                let total = lines.len();
                if start > total {
                    return ToolResult::standard_error(
                        &format!("起始行 {start} 超出文件总行数 {total}"),
                        Some("InvalidInput"),
                        None,
                    );
                }
                let end = end.min(total);
                let mut out = format!("{path}（共 {total} 行，显示 {start}..{end}）\n");
                for (i, line) in lines.iter().enumerate().skip(start - 1).take(end - start + 1) {
                    out.push_str(&format!("{:>5}\t{}\n", i + 1, line));
                }
                ToolResult::standard_success(
                    &out,
                    Some(json!({"path": path, "total_lines": total, "start": start, "end": end})),
                )
            }
            "create" => {
                if std::path::Path::new(&path).exists() {
                    return ToolResult::standard_error(
                        &format!("{path} 已存在。要改已有文件用 str_replace 或 view；要整体覆盖用 write_file。"),
                        Some("InvalidInput"),
                        None,
                    );
                }
                let body = args.get("file_text").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    if !parent.exists() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            return ToolResult::standard_error(
                                &format!("创建父目录失败：{e}"),
                                None,
                                None,
                            );
                        }
                    }
                }
                match std::fs::write(&path, body) {
                    Ok(()) => ToolResult::standard_success(
                        &format!("已创建 {path}（{} 字节）", body.len()),
                        Some(json!({"path": path, "bytes": body.len()})),
                    ),
                    Err(e) => ToolResult::standard_error(&format!("写入失败：{e}"), None, None),
                }
            }
            "str_replace" => {
                let old = args.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
                let original = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        return ToolResult::standard_error(
                            &format!("读不到 {path}：{e}"),
                            Some("NotFound"),
                            None,
                        )
                    }
                };
                let count = original.matches(old).count();
                if count == 0 {
                    return ToolResult::standard_error(
                        &format!(
                            "{path} 里找不到 old_string。old_string 必须与文件内容逐字一致（含缩进与换行）——\
                             先用 view 看清原文再改。"
                        ),
                        Some("InvalidInput"),
                        None,
                    );
                }
                if count > 1 && !replace_all {
                    return ToolResult::standard_error(
                        &format!(
                            "old_string 在 {path} 里出现了 {count} 次。补足上下文让它唯一，或显式设 replace_all=true。"
                        ),
                        Some("InvalidInput"),
                        None,
                    );
                }
                let updated = if replace_all {
                    original.replace(old, new)
                } else {
                    original.replacen(old, new, 1)
                };
                push_undo(&ctx.session_id, &path, original.clone());
                match std::fs::write(&path, &updated) {
                    Ok(()) => ToolResult::standard_success(
                        &format!("已替换 {path}（{} 处）。改坏了可以 undo。", if replace_all { count } else { 1 }),
                        Some(json!({"path": path, "replacements": if replace_all { count } else { 1 }})),
                    ),
                    Err(e) => ToolResult::standard_error(&format!("写入失败：{e}"), None, None),
                }
            }
            "insert" => {
                let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let new = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                let original = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        return ToolResult::standard_error(
                            &format!("读不到 {path}：{e}"),
                            Some("NotFound"),
                            None,
                        )
                    }
                };
                let mut lines: Vec<String> = original.lines().map(|s| s.to_string()).collect();
                if line > lines.len() {
                    return ToolResult::standard_error(
                        &format!("第 {line} 行超出文件总行数 {}", lines.len()),
                        Some("InvalidInput"),
                        None,
                    );
                }
                for (offset, part) in new.lines().enumerate() {
                    lines.insert(line + offset, part.to_string());
                }
                let updated = lines.join("\n") + "\n";
                push_undo(&ctx.session_id, &path, original);
                match std::fs::write(&path, &updated) {
                    Ok(()) => ToolResult::standard_success(
                        &format!("已在 {path} 第 {line} 行之后插入内容。"),
                        Some(json!({"path": path, "inserted_after": line})),
                    ),
                    Err(e) => ToolResult::standard_error(&format!("写入失败：{e}"), None, None),
                }
            }
            _ => {
                let previous = {
                    let mut stack = undo_stack().lock().unwrap_or_else(|e| e.into_inner());
                    stack.get_mut(&undo_key(&ctx.session_id, &path)).and_then(|v| v.pop())
                };
                match previous {
                    None => ToolResult::standard_success(
                        &format!("{path} 在本会话里没有可撤销的修改。"),
                        Some(json!({"path": path, "reverted": false})),
                    ),
                    Some(content) => match std::fs::write(&path, &content) {
                        Ok(()) => ToolResult::standard_success(
                            &format!("已撤销 {path} 的最近一次修改。"),
                            Some(json!({"path": path, "reverted": true})),
                        ),
                        Err(e) => ToolResult::standard_error(&format!("回写失败：{e}"), None, None),
                    },
                }
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::File
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }
    fn search_hint(&self) -> &str {
        "str replace editor view insert undo 编辑 撤销 替换 行号"
    }
}
