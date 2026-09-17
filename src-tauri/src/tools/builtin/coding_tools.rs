//! 编程智能体工具集（fs / shell 能力面）
//!
//! 覆盖"读代码 → 搜代码 → 改代码 → 跑命令"的最小闭环：
//! - `write_file`：创建/覆写文本文件（FsWrite，限工作目录内）
//! - `edit_file`：精确字符串替换编辑（FsWrite，限工作目录内）
//! - `run_command`：在工作目录执行 PowerShell 命令（Shell，超时+输出截断）
//! - `grep_search`：递归搜索文件内容（FsRead，跳过 .git/node_modules/target）
//! - `list_dir`：树状列出目录结构（FsRead，深度受限）
//!
//! 与 `read_file`（已有）合用即构成完整编程工具面。所有路径走沙箱校验，
//! 写/执行类操作由 ToolSystem 的守卫与审批矩阵统一管控。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::sandbox::is_path_safe;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 搜索/列目录时跳过的目录名
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", ".venv", "__pycache__", ".next"];
/// 二进制扩展名（grep 时跳过）
const BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "mp3", "wav", "ogg", "flac",
    "mp4", "mkv", "avi", "mov", "zip", "rar", "7z", "tar", "gz", "exe", "dll",
    "so", "dylib", "bin", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "ttf", "otf", "woff", "woff2",
];
/// run_command / 自建工具脚本的明显破坏性命令片段（直接拒绝，不进入执行）
pub(crate) const FORBIDDEN_FRAGMENTS: &[&str] = &[
    "format ", "format-volume", "shutdown", "reg delete", "reg delete ", "diskpart",
    "cipher /w", "rd /s", "del /f /s /q", "Remove-Item -Recurse -Force C:\\",
];
/// 命令输出截断上限（字符）
const CMD_OUTPUT_MAX_CHARS: usize = 8000;
/// 命令执行超时（秒）
const CMD_TIMEOUT_SECS: u64 = 120;

fn is_binary_ext(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| BINARY_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

// ============================================================================
// WriteFileTool
// ============================================================================

/// 创建/覆写文本文件（UTF-8）。自动创建父目录。仅允许工作目录内路径。
pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create or overwrite a text file with the given content (UTF-8). Parent directories are created automatically. Only paths inside your working directory are allowed. Use for creating new code files, configs, or fully rewriting a file."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建或覆写文本文件（UTF-8 编码），自动创建父目录。仅允许工作目录内的路径。适用于新建代码文件、配置文件，或整体重写文件内容。局部修改请优先使用 edit_file。",
            "ja" => "テキストファイルを作成または上書きする（UTF-8）。親ディレクトリは自動作成される。作業ディレクトリ内のパスのみ許可。新しいコードや設定ファイルの作成、ファイル全体の書き換えに使う。部分的な修正には edit_file を使うこと。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "写个文件\n把这个存成文件\n新建文件",
            "en" => "write a file\nsave this to a file\ncreate a new file",
            "ja" => "ファイルを書いて\nこれをファイルに保存\n新しいファイルを作成",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目标文件绝对路径（工作目录内）" },
                "content": { "type": "string", "description": "完整文件内容" }
            },
            "required": ["path", "content"]
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
        if !is_path_safe(path) {
            return ValidationResult::failure("路径包含穿越序列（..），已被沙箱拦截", 2);
        }
        if !ctx.is_path_authorized(path) {
            return ValidationResult::failure(
                &format!("路径不在任何已授权工作区内，已拒绝写入: {}", path),
                2,
            );
        }
        if input.get("content").and_then(|v| v.as_str()).is_none() {
            return ValidationResult::failure("content 是必填项", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let p = std::path::Path::new(&path);
        if let Some(parent) = p.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::standard_error(&format!("创建父目录失败: {e}"), None, None);
                }
            }
        }
        let existed = p.exists();
        // 写入前的旧内容：用于生成「变更」页可见的 unified diff。
        // 读不到（新建 / 非 UTF-8 文本）按「无旧内容」处理 → 全文记作新增。
        let previous = if existed { std::fs::read_to_string(p).ok() } else { None };
        match std::fs::write(p, content.as_bytes()) {
            Ok(()) => {
                let diff = build_write_diff(&path, previous.as_deref(), &content);
                ToolResult::standard_success(
                    &format!("已{}文件「{}」（{} 字符）", if existed { "覆写" } else { "创建" }, path, content.chars().count()),
                    Some(json!({
                        "path": path,
                        "existed": existed,
                        "bytes": content.len(),
                        "diff": diff,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error(&format!("写入文件失败: {e}"), None, None),
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

    fn is_destructive(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "write create file code script save 编程 写文件 代码"
    }
}

// ============================================================================
// write_file 的 unified diff
// ============================================================================

/// 写入 diff 的行数上限（比 edit_file 宽松：新建文件的 diff 就是整份内容）。
const WRITE_DIFF_MAX_LINES: usize = 300;
/// 变化中段超过该规模即退化为整段替换，避免 LCS 的 O(N*M) 内存失控。
const WRITE_DIFF_LCS_BUDGET: usize = 4_000_000;

/// 生成整文件写入的 unified diff（供「变更」页展示）。
///
/// - `previous` 为 None（文件原本不存在 / 不是 UTF-8 文本）→ 全文记作新增：
///   hunk 头 `@@ -0,0 +1,N @@`，每行 `+`。
/// - 否则先剥离公共前缀与后缀，只对真正变化的中段求 LCS——「万行文件改一行」
///   这类场景工作集极小，不会付出全量代价。
///
/// 输出受行数 / 字符预算约束，与 `edit_file` 的 diff 共用「变更」页渲染。
fn build_write_diff(path: &str, previous: Option<&str>, content: &str) -> String {
    let new_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = previous.map(|p| p.lines().collect()).unwrap_or_default();

    // 前后皆空：没有可展示的内容
    if new_lines.is_empty() && old_lines.is_empty() {
        return String::new();
    }

    // 新建：整文件新增
    if old_lines.is_empty() {
        let mut out = vec![format!("--- {path}"), format!("+++ {path}")];
        out.push(format!("@@ -0,0 +1,{} @@", new_lines.len()));
        out.extend(new_lines.iter().map(|l| format!("+{l}")));
        return cap_write_diff(out, false);
    }

    // 剥离公共前缀 / 后缀（循环上界已保证不会下溢）
    let mut prefix = 0usize;
    while prefix < old_lines.len() && prefix < new_lines.len() && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];

    // 内容完全一致：无变更
    if old_mid.is_empty() && new_mid.is_empty() {
        return String::new();
    }

    // 变化区过大 → 退化为整段替换（内存恒定）。两侧各分一半行预算，
    // 否则行数上限会被删除行吃光，新增行一行都看不见。
    let per_side = WRITE_DIFF_MAX_LINES.saturating_sub(12) / 2;
    let (body, body_truncated) = if old_mid.len().saturating_mul(new_mid.len()) > WRITE_DIFF_LCS_BUDGET {
        (
            full_replace_lines(old_mid, new_mid, per_side),
            old_mid.len() > per_side || new_mid.len() > per_side,
        )
    } else {
        (lcs_diff_lines(old_mid, new_mid), false)
    };

    let back = DIFF_CONTEXT.min(prefix);
    let fwd = DIFF_CONTEXT.min(suffix);
    let old_block = back + old_mid.len() + fwd;
    let new_block = back + new_mid.len() + fwd;
    let start = prefix - back + 1; // 1-based
    // 旧侧为空（纯插入）时起点写 0，与 git 的 `-0,0` 表示法一致
    let old_start = if old_block == 0 { 0 } else { start };

    let mut out = vec![format!("--- {path}"), format!("+++ {path}")];
    out.push(format!("@@ -{old_start},{old_block} +{start},{new_block} @@"));
    out.extend(old_lines[prefix - back..prefix].iter().map(|l| format!(" {l}")));
    out.extend(body);
    out.extend(
        new_lines[new_lines.len() - suffix..new_lines.len() - suffix + fwd]
            .iter()
            .map(|l| format!(" {l}")),
    );
    cap_write_diff(out, body_truncated)
}

/// 行级 LCS diff：返回带 `' '` / `'-'` / `'+'` 前缀的行。
/// 规模由调用方按 `WRITE_DIFF_LCS_BUDGET` 保证有界。
fn lcs_diff_lines(old: &[&str], new: &[&str]) -> Vec<String> {
    let (n, m) = (old.len(), new.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if old[i] == new[j] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let mut out = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            out.push(format!(" {}", old[i]));
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            out.push(format!("-{}", old[i]));
            i += 1;
        } else {
            out.push(format!("+{}", new[j]));
            j += 1;
        }
    }
    out.extend(old[i..].iter().map(|l| format!("-{l}")));
    out.extend(new[j..].iter().map(|l| format!("+{l}")));
    out
}

/// 变化区过大时的退化表示：旧段整体删除 + 新段整体新增（不做最小化，内存恒定）。
/// `per_side` 限制每侧行数，保证增删两侧都看得见。
fn full_replace_lines(old: &[&str], new: &[&str], per_side: usize) -> Vec<String> {
    let mut out: Vec<String> = old.iter().take(per_side).map(|l| format!("-{l}")).collect();
    out.extend(new.iter().take(per_side).map(|l| format!("+{l}")));
    out
}

/// diff 输出预算：超行数 / 字符上限即在行边界截断并标注。
/// `body_truncated` 用于上游已经丢过内容（退化路径）的情形。
fn cap_write_diff(mut out: Vec<String>, body_truncated: bool) -> String {
    let mut truncated = body_truncated;
    if out.len() > WRITE_DIFF_MAX_LINES {
        out.truncate(WRITE_DIFF_MAX_LINES);
        truncated = true;
    }
    let mut text = out.join("\n");
    if text.chars().count() > DIFF_MAX_CHARS {
        let cut: String = text.chars().take(DIFF_MAX_CHARS).collect();
        text = match cut.rfind('\n') {
            Some(idx) => cut[..idx].to_string(),
            None => cut,
        };
        truncated = true;
    }
    if truncated {
        text.push_str("\n…（diff 过大已截断）");
    }
    text
}

// ============================================================================
// EditFileTool
// ============================================================================

/// 精确字符串替换编辑：old_string 必须在文件中唯一（或显式 replace_all）。
pub struct EditFileTool;

impl EditFileTool {
    pub fn new() -> Self {
        Self
    }
}

/// 编辑 diff 的体积控制（结果 JSON 需整体留在 6000 字符的工具结果预算内）
const DIFF_MAX_HUNKS: usize = 6;
const DIFF_MAX_LINES: usize = 100;
const DIFF_MAX_CHARS: usize = 4000;
/// hunk 上下文行数
const DIFF_CONTEXT: usize = 3;

/// 由替换位置生成 unified diff（每处替换一个 hunk，含上下文；相邻区间自动合并）。
///
/// `replacements` 为 `(起始行 1-based, 旧文本行数, 新文本)` 列表（按出现顺序）。
/// 返回 None 表示体积超限无法生成；返回 Some("") 表示无变化。
fn build_edit_diff(path: &str, raw: &str, replacements: &[(usize, usize, &str)]) -> Option<String> {
    if replacements.is_empty() {
        return Some(String::new());
    }
    let old_lines: Vec<&str> = raw.lines().collect();

    // 合并相邻/重叠的替换区间（间隙 ≤ 2*上下文 时合并，避免上下文行重复出现）
    let mut ranges: Vec<(usize, usize)> = Vec::new(); // 旧文件行区间（闭区间，1-based）
    let mut range_hits: Vec<Vec<(usize, usize, &str)>> = Vec::new();
    for &(s, old_n, new_text) in replacements {
        let e = s + old_n.saturating_sub(1);
        let mergeable = matches!(ranges.last(), Some((_, last_e)) if s <= *last_e + DIFF_CONTEXT * 2);
        if mergeable {
            let last = ranges.last_mut().expect("checked above");
            if e > last.1 {
                last.1 = e;
            }
            range_hits.last_mut().expect("paired with ranges").push((s, e, new_text));
        } else {
            ranges.push((s, e));
            range_hits.push(vec![(s, e, new_text)]);
        }
    }

    let mut out_lines: Vec<String> = vec![format!("--- {path}"), format!("+++ {path}")];
    let mut delta: i64 = 0; // 已输出 hunk 累计的行数变化（新 - 旧）
    let mut truncated = false;

    for (ri, &(s, e)) in ranges.iter().enumerate() {
        if ri >= DIFF_MAX_HUNKS {
            truncated = true;
            break;
        }
        let hits = &range_hits[ri];
        // 新片段行数：区间内逐段游走（未变行 + 各替换的新文本行）
        let mut new_len = 0usize;
        let mut pos = s;
        for &(hs, he, nt) in hits {
            new_len += hs.saturating_sub(pos); // 替换前的未变行
            new_len += if nt.is_empty() { 0 } else { nt.lines().count() };
            pos = he + 1;
        }
        new_len += (e + 1).saturating_sub(pos); // 末次替换后的未变行

        // 上下文范围
        let back = DIFF_CONTEXT.min(s - 1);
        let fwd = DIFF_CONTEXT.min(old_lines.len().saturating_sub(e));
        let first = s - back; // 1-based
        let last = e + fwd; // 1-based（含）
        let old_block_len = last - first + 1;
        let new_block_len = back + new_len + fwd;
        let first_new = (first as i64 + delta).max(1) as usize;

        // 变更区行（上下文行 + '-' 旧行 + '+' 新行，区间内未变行作为上下文只出现一次）
        let mut body: Vec<String> = Vec::new();
        for i in first..s {
            body.push(format!(" {}", old_lines[i - 1]));
        }
        let mut pos = s;
        for &(hs, he, nt) in hits {
            while pos < hs {
                body.push(format!(" {}", old_lines[pos - 1]));
                pos += 1;
            }
            for i in hs..=he {
                body.push(format!("-{}", old_lines[i - 1]));
            }
            for l in nt.lines() {
                body.push(format!("+{l}"));
            }
            pos = he + 1;
        }
        while pos <= e {
            body.push(format!(" {}", old_lines[pos - 1]));
            pos += 1;
        }
        for i in (e + 1)..=last {
            body.push(format!(" {}", old_lines[i - 1]));
        }

        if out_lines.len() + body.len() + 1 > DIFF_MAX_LINES {
            truncated = true;
            break;
        }
        out_lines.push(format!(
            "@@ -{first},{old_block_len} +{first_new},{new_block_len} @@"
        ));
        out_lines.extend(body);
        delta += new_block_len as i64 - old_block_len as i64;
    }

    let mut text = out_lines.join("\n");
    if truncated {
        text.push_str("\n…（diff 过大已截断）");
    }
    // 字符硬上限（超限在行边界截断，避免 JSON 预算爆炸）
    if text.chars().count() > DIFF_MAX_CHARS {
        let cut: String = text.chars().take(DIFF_MAX_CHARS).collect();
        text = match cut.rfind('\n') {
            Some(idx) => cut[..idx].to_string(),
            None => cut,
        };
        text.push_str("\n…（diff 过大已截断）");
    }
    Some(text)
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a text file by exact string replacement. `old_string` must appear exactly once in the file (include surrounding lines to disambiguate), or set `replace_all` to replace every occurrence. Use for surgical code edits instead of rewriting whole files."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "精确字符串替换编辑文件。old_string 必须在文件中恰好出现一次（可带上下文行消歧），或设置 replace_all 替换全部出现。适合精准修改代码片段而非整文件重写。",
            "ja" => "文字列の完全一致置換でファイルを編集する。old_string はファイル内にちょうど1回だけ出現する必要がある（文脈行を含めて一意にする）。全置換には replace_all を指定。ファイル全体の書き換えではなく部分的なコード修正に使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "改一下这个文件\n编辑文件\n把这段改改",
            "en" => "edit this file\nmodify the file\nchange this code",
            "ja" => "このファイルを編集して\nファイルを修正して\nコードを変更して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目标文件绝对路径（工作目录内）" },
                "old_string": { "type": "string", "description": "要被替换的原文（含足够上下文使其唯一）" },
                "new_string": { "type": "string", "description": "替换后的新文本" },
                "replace_all": { "type": "boolean", "description": "替换全部出现（默认 false，要求唯一匹配）" }
            },
            "required": ["path", "old_string", "new_string"]
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
        if !is_path_safe(path) {
            return ValidationResult::failure("路径包含穿越序列（..），已被沙箱拦截", 2);
        }
        if !ctx.is_path_authorized(path) {
            return ValidationResult::failure(
                &format!("路径不在任何已授权工作区内，已拒绝编辑: {}", path),
                2,
            );
        }
        if input.get("old_string").and_then(|v| v.as_str()).map_or(true, |s| s.is_empty()) {
            return ValidationResult::failure("old_string 不能为空", 2);
        }
        if input.get("new_string").and_then(|v| v.as_str()).is_none() {
            return ValidationResult::failure("new_string 是必填项", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let old_string = args.get("old_string").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let new_string = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);

        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => return ToolResult::standard_error(&format!("读取文件失败: {e}"), None, None),
        };
        let count = raw.matches(&old_string).count();
        if count == 0 {
            return ToolResult::standard_error(
                "old_string 在文件中未找到。请确认原文与文件内容完全一致（含缩进）。",
                Some("not_found"),
                Some(json!({ "path": path })),
            );
        }
        if count > 1 && !replace_all {
            return ToolResult::standard_error(
                &format!("old_string 出现了 {count} 次，不唯一。请在 old_string 中加入更多上下文行，或设置 replace_all=true。"),
                Some("ambiguous"),
                Some(json!({ "path": path, "matches": count })),
            );
        }

        let updated = if replace_all {
            raw.replace(&old_string, &new_string)
        } else {
            raw.replacen(&old_string, &new_string, 1)
        };

        // 收集各替换的行位置（1-based），用于生成 unified diff
        let mut replacements: Vec<(usize, usize, &str)> = Vec::new();
        {
            let mut from = 0usize;
            while let Some(rel) = raw[from..].find(&old_string) {
                let pos = from + rel;
                let start_line = raw[..pos].lines().count() + 1;
                let old_n = old_string.lines().count().max(1);
                replacements.push((start_line, old_n, new_string.as_str()));
                from = pos + old_string.len();
                if !replace_all {
                    break; // 非 replace_all 时 count 必为 1（上方已校验唯一性）
                }
            }
        }
        let diff = build_edit_diff(&path, &raw, &replacements);

        match std::fs::write(&path, updated.as_bytes()) {
            Ok(()) => ToolResult::standard_success(
                &format!("已编辑「{}」：替换 {count} 处", path, count = if replace_all { count } else { 1 }),
                Some(json!({
                    "path": path,
                    "replaced": if replace_all { count } else { 1 },
                    "diff": diff.unwrap_or_default(),
                })),
            ),
            Err(e) => ToolResult::standard_error(&format!("写回文件失败: {e}"), None, None),
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

    fn is_destructive(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "edit modify fix refactor replace code 编辑 修改 代码 重构"
    }
}

// ============================================================================
// RunCommandTool
// ============================================================================

/// 在工作目录执行 PowerShell 命令（非交互，超时受限，输出截断）。
pub struct RunCommandTool;

impl RunCommandTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RunCommandTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a PowerShell command in the working directory (non-interactive, timeout 120s, output truncated to 8000 chars). Use for builds, tests, git operations, package installs, and quick inspections (dir, git status, cargo check...). The command runs with the user's permissions — avoid destructive system operations."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在工作目录执行 PowerShell 命令（非交互，超时 120 秒，输出截断至 8000 字符）。适用于构建、测试、git 操作、包安装与快速查看（dir / git status / cargo check 等）。命令以用户权限运行，禁止破坏性系统操作。",
            "ja" => "作業ディレクトリで PowerShell コマンドを実行する（非対話、タイムアウト120秒、出力は8000文字で切り詰め）。ビルド、テスト、git 操作、パッケージインストール、簡単な確認（dir / git status / cargo check など）に使う。コマンドはユーザー権限で動くため、破壊的なシステム操作は禁止。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "跑个命令\n执行一下这个\n在终端里跑",
            "en" => "run this command\nexecute it\nrun it in the terminal",
            "ja" => "コマンドを実行して\nこれを実行して\nターミナルで動かして",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "要执行的命令（PowerShell 语法）" },
                "timeout_secs": { "type": "integer", "description": "超时秒数（默认 120，上限 600）" }
            },
            "required": ["command"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
        if command.is_empty() {
            return ValidationResult::failure("command 不能为空", 2);
        }
        let lower = command.to_ascii_lowercase();
        if let Some(frag) = FORBIDDEN_FRAGMENTS.iter().find(|f| lower.contains(*f)) {
            return ValidationResult::failure(
                &format!("命令包含破坏性片段「{frag}」，已被拒绝。如确需执行请让用户手动操作。"),
                2,
            );
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let timeout = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(5, 600))
            .unwrap_or(CMD_TIMEOUT_SECS);

        let cwd = if ctx.working_directory.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(&ctx.working_directory))
        };

        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &command])
            .creation_flags_windows();
        if let Some(dir) = cwd.as_ref() {
            cmd.current_dir(dir);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout),
            cmd.output(),
        )
        .await;
        match output {
            Err(_) => ToolResult::standard_error(
                &format!("命令执行超时（{timeout} 秒），已终止"),
                Some("timeout"),
                Some(json!({ "command": command })),
            ),
            Ok(Err(e)) => ToolResult::standard_error(&format!("启动命令失败: {e}"), None, None),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let code = out.status.code().unwrap_or(-1);
                let truncated = stdout.len() + stderr.len() > CMD_OUTPUT_MAX_CHARS;
                let stdout_t = truncate_chars(&stdout, CMD_OUTPUT_MAX_CHARS);
                let stderr_t = truncate_chars(&stderr, CMD_OUTPUT_MAX_CHARS / 2);
                let success = out.status.success();
                ToolResult::standard_success(
                    &format!("命令退出码 {code}{}", if truncated { "（输出已截断）" } else { "" }),
                    Some(json!({
                        "command": command,
                        "exit_code": code,
                        "success": success,
                        "stdout": stdout_t,
                        "stderr": stderr_t,
                        "truncated": truncated,
                    })),
                )
            }
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
        "run command shell powershell terminal build test git npm cargo 执行 命令 构建 测试"
    }
}

/// Windows 下隐藏 powershell 弹出的控制台窗口（仅 Windows 生效）。
#[cfg(windows)]
trait CreationFlagsExt {
    fn creation_flags_windows(&mut self) -> &mut Self;
}
#[cfg(windows)]
impl CreationFlagsExt for tokio::process::Command {
    fn creation_flags_windows(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        self
    }
}
#[cfg(not(windows))]
trait CreationFlagsExt {
    fn creation_flags_windows(&mut self) -> &mut Self {
        self
    }
}
#[cfg(not(windows))]
impl CreationFlagsExt for tokio::process::Command {}

/// 按字符数截断（chars 边界天然安全），超长时附加截断标记。
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut result: String = s.chars().take(max).collect();
    result.push_str("\n…(truncated)");
    result
}

// ============================================================================
// GrepSearchTool
// ============================================================================

/// 递归搜索文件内容（大小写不敏感的子串或字面正则），返回匹配文件+行。
pub struct GrepSearchTool;

impl GrepSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrepSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GrepSearchTool {
    fn name(&self) -> &str {
        "grep_search"
    }

    fn description(&self) -> &str {
        "Recursively search file contents under a directory (case-insensitive substring). Skips .git/node_modules/target and binary files. Returns up to 50 matches with file path, line number and line text. Use to locate functions, configs, or usages before editing."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "递归搜索目录下文件内容（大小写不敏感子串匹配），跳过 .git/node_modules/target 与二进制文件。返回最多 50 条匹配（文件路径+行号+该行内容）。编辑前用它定位函数、配置或引用位置。",
            "ja" => "ディレクトリ以下のファイル内容を再帰的に検索する（大文字小文字を区別しない部分一致）。.git/node_modules/target とバイナリファイルはスキップ。最大50件の一致（ファイルパス+行番号+行の内容）を返す。編集前に 関数・設定・使用箇所 の位置を特定するのに使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "搜一下代码里的内容\n找找这个字符串在哪\n在这堆文件里搜",
            "en" => "search the code for this\nfind where this string is\ngrep for it",
            "ja" => "コード内を検索して\nこの文字列を探して\nファイルから検索",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "directory": { "type": "string", "description": "搜索起始目录（默认工作目录）" },
                "query": { "type": "string", "description": "搜索的文本（大小写不敏感）" },
                "glob": { "type": "string", "description": "可选：仅搜索匹配此扩展名的文件（如 rs / ts / py，不带点）" }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
        if query.is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        if let Some(dir) = input.get("directory").and_then(|v| v.as_str()) {
            let dir = dir.trim();
            if !dir.is_empty() {
                if !is_path_safe(dir) {
                    return ValidationResult::failure("目录路径包含穿越序列，已被沙箱拦截", 2);
                }
                if !ctx.is_path_authorized(dir) {
                    return ValidationResult::failure("目录不在任何已授权工作区内，已拒绝", 2);
                }
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let glob = args.get("glob").and_then(|v| v.as_str()).map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase());
        let dir = args
            .get("directory")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ctx.working_directory.clone());

        if dir.is_empty() {
            return ToolResult::standard_error("未指定目录且工作目录为空", None, None);
        }
        let root = std::path::PathBuf::from(&dir);
        if !root.is_dir() {
            return ToolResult::standard_error(&format!("目录不存在: {dir}"), None, None);
        }

        let needle = query.to_ascii_lowercase();
        let mut matches: Vec<(String, usize, String)> = Vec::new();
        let mut files_scanned = 0usize;
        let mut stack = vec![root.clone()];

        while let Some(dir_path) = stack.pop() {
            if matches.len() >= 50 || files_scanned >= 3000 {
                break;
            }
            let entries = match std::fs::read_dir(&dir_path) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let p = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if p.is_dir() {
                    if !SKIP_DIRS.contains(&name.as_str()) {
                        stack.push(p);
                    }
                    continue;
                }
                if is_binary_ext(&p) {
                    continue;
                }
                if let Some(g) = glob.as_ref() {
                    if p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() != Some(g.as_str()) {
                        continue;
                    }
                }
                files_scanned += 1;
                if files_scanned > 3000 {
                    break;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                for (idx, line) in text.lines().enumerate() {
                    if line.to_ascii_lowercase().contains(&needle) {
                        let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().into_owned();
                        let line_text: String = line.trim().chars().take(200).collect();
                        matches.push((rel, idx + 1, line_text));
                        if matches.len() >= 50 {
                            break;
                        }
                    }
                }
                if matches.len() >= 50 {
                    break;
                }
            }
        }

        if matches.is_empty() {
            return ToolResult::standard_success(
                &format!("未找到匹配「{query}」（扫描 {files_scanned} 个文件）"),
                Some(json!({ "query": query, "matches": [], "files_scanned": files_scanned })),
            );
        }
        let lines: Vec<String> = matches
            .iter()
            .map(|(f, n, l)| format!("{f}:{n}: {l}"))
            .collect();
        ToolResult::standard_success(
            &format!("找到 {} 处匹配「{}」（扫描 {files_scanned} 个文件）", matches.len(), query),
            Some(json!({
                "query": query,
                "directory": dir,
                "matches": matches.iter().map(|(f, n, l)| json!({"file": f, "line": n, "text": l})).collect::<Vec<_>>(),
                "files_scanned": files_scanned,
                "preview": lines.join("\n"),
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

    fn search_hint(&self) -> &str {
        "grep search find code text content 搜索 查找 代码"
    }
}

// ============================================================================
// ListDirTool
// ============================================================================

/// 树状列出目录结构（深度限制，跳过依赖目录）。
pub struct ListDirTool;

impl ListDirTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListDirTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List a directory as a tree (default depth 2, max 4). Skips .git/node_modules/target. Use to understand project structure before reading or editing files."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "以树状结构列出目录内容（默认深度 2，上限 4），跳过 .git/node_modules/target。在读取或编辑文件前用它了解项目结构。",
            "ja" => "ディレクトリをツリー構造で一覧表示する（デフォルト深さ2、最大4）。.git/node_modules/target はスキップ。ファイルの読み書きの前にプロジェクト構造を把握するのに使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "这个目录下有什么\n列一下文件\n看看文件夹里",
            "en" => "what's in this directory\nlist the files\nshow me the folder contents",
            "ja" => "このフォルダの中身は\nファイル一覧を見せて\nディレクトリを表示",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "directory": { "type": "string", "description": "要列出的目录（默认工作目录）" },
                "depth": { "type": "integer", "description": "遍历深度（默认 2，上限 4）", "minimum": 1 }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        if let Some(dir) = input.get("directory").and_then(|v| v.as_str()) {
            let dir = dir.trim();
            if !dir.is_empty() {
                if !is_path_safe(dir) {
                    return ValidationResult::failure("目录路径包含穿越序列，已被沙箱拦截", 2);
                }
                if !ctx.is_path_authorized(dir) {
                    return ValidationResult::failure("目录不在任何已授权工作区内，已拒绝", 2);
                }
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let depth = args.get("depth").and_then(|v| v.as_u64()).map(|v| v.clamp(1, 4) as usize).unwrap_or(2);
        let dir = args
            .get("directory")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ctx.working_directory.clone());
        if dir.is_empty() {
            return ToolResult::standard_error("未指定目录且工作目录为空", None, None);
        }
        let root = std::path::PathBuf::from(&dir);
        if !root.is_dir() {
            return ToolResult::standard_error(&format!("目录不存在: {dir}"), None, None);
        }

        let mut lines: Vec<String> = Vec::new();
        let mut count = 0usize;
        walk_tree(&root, "", depth, &mut lines, &mut count);
        let tree = lines.join("\n");
        ToolResult::standard_success(
            &format!("「{dir}」共 {} 项（深度 ≤{depth}）", count.min(400)),
            Some(json!({ "directory": dir, "tree": tree, "entries": count })),
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

    fn search_hint(&self) -> &str {
        "list directory tree structure files 目录 结构 文件列表"
    }
}

/// 递归构建目录树文本（限制总条目 400）。
fn walk_tree(dir: &std::path::Path, prefix: &str, depth: usize, out: &mut Vec<String>, count: &mut usize) {
    if depth == 0 || *count >= 400 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|e| (e.path().is_file(), e.file_name()));
    for (i, entry) in items.iter().enumerate() {
        if *count >= 400 {
            out.push(format!("{prefix}…"));
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let last = i == items.len() - 1;
        let branch = if last { "└── " } else { "├── " };
        out.push(format!("{prefix}{branch}{name}{}", if is_dir { "/" } else { "" }));
        *count += 1;
        if is_dir {
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            walk_tree(&entry.path(), &child_prefix, depth - 1, out, count);
        }
    }
}

#[cfg(test)]
mod write_diff_tests {
    use super::*;

    /// 新建文件：全文记作新增，hunk 头 `-0,0 +1,N`，且不应出现删除行。
    #[test]
    fn new_file_is_all_added() {
        let d = build_write_diff("a.py", None, "print(1)\nprint(2)\n");
        assert!(d.starts_with("--- a.py\n+++ a.py\n@@ -0,0 +1,2 @@"), "head:/n{d}");
        assert!(d.contains("\n+print(1)"), "{d}");
        assert!(d.contains("\n+print(2)"), "{d}");
        assert!(
            !d.lines().any(|l| l.starts_with('-') && !l.starts_with("---")),
            "new file must have no deletion lines:/n{d}"
        );
    }

    /// 覆写：真正变化的行要同时出现删除与新增，并且保留上下文行。
    #[test]
    fn overwrite_keeps_removals() {
        let d = build_write_diff("f.txt", Some("a\nb\nc\nd\n"), "a\nB\nc\nd\n");
        assert!(d.contains("@@ -1,4 +1,4 @@"), "head:/n{d}");
        assert!(d.contains("-b"), "{d}");
        assert!(d.contains("+B"), "{d}");
        assert!(d.contains(" a"), "context kept:/n{d}");
        assert!(d.contains(" d"), "context kept:/n{d}");
    }

    /// 内容一致：不产生 diff（前端据此显示「无改动」）。
    #[test]
    fn identical_content_has_no_diff() {
        assert_eq!(build_write_diff("f", Some("x\ny\n"), "x\ny\n"), "");
    }

    /// 大文件改一行：公共前后缀剥离后工作集极小，输出只含 1 增 1 删。
    #[test]
    fn large_file_single_change_is_minimal() {
        let old: String = (1..=500).map(|i| format!("line{i}\n")).collect();
        let new = old.replace("line250\n", "CHANGED\n");
        let d = build_write_diff("big.txt", Some(&old), &new);
        let added = d.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
        let removed = d.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
        assert_eq!((added, removed), (1, 1), "diff:/n{d}");
        assert!(d.contains("-line250"), "{d}");
        assert!(d.contains("+CHANGED"), "{d}");
    }

    /// 上下文收敛：只保留变化点前后各 DIFF_CONTEXT 行。
    #[test]
    fn context_is_bounded() {
        let old: String = (1..=200).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l100\n", "l100x\n");
        let d = build_write_diff("c.txt", Some(&old), &new);
        let lines = d.lines().count();
        assert!(lines <= 2 + 1 + (DIFF_CONTEXT * 2 + 2), "too many lines ({lines}):\n{d}");
    }

    /// 超大变化区走整段替换兜底：不触发 LCS 内存失控，且增删两侧都要看得见。
    #[test]
    fn huge_change_falls_back_to_full_replace() {
        let old: String = (1..=3000).map(|i| format!("a{i}\n")).collect();
        let new: String = (1..=3000).map(|i| format!("b{i}\n")).collect();
        let d = build_write_diff("h.txt", Some(&old), &new);
        assert!(d.contains("-a1"), "旧段删除应可见:\n{d}");
        assert!(d.contains("+b1"), "新段新增应可见（不能被删除行挤掉）:\n{d}");
        assert!(d.contains("已截断"), "应标注截断:\n{d}");
        // 兜底路径同样受行数预算约束
        assert!(d.lines().count() <= WRITE_DIFF_MAX_LINES + 1, "not capped");
    }
}
