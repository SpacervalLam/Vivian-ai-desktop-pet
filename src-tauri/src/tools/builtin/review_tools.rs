//! review_diff 工具 —— 为代码审查可靠地取到改动、变更文件清单与规约文件。
//!
//! 审查是只读场景：本工具只解析目标、跑只读 git 命令、读规约文件，不改工作区、
//! 不建提交。目标解析、条件发现与截断都抽成纯函数，便于单测。

use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::git::{detect_default_base, run_git, run_git_opt};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// diff 正文字符上限（超出按头尾保留并标记截断）。
const DEFAULT_MAX_CHARS: usize = 60_000;
const MAX_MAX_CHARS: usize = 200_000;
/// 需要遵循的规约文件名（仓库根与变更文件目录链上都查）。
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CODE_WIKI.md"];

/// review_diff 工具 - 解析审查目标并取回 diff、变更文件与规约文件。
pub struct ReviewDiffTool;

impl ReviewDiffTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReviewDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 在子命令之后插入一个 git 选项（`diff`/`show` 两类子命令都接受）。
fn with_flag(spec: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(spec.len() + 1);
    out.push(spec[0].clone());
    out.push(flag.to_string());
    out.extend(spec[1..].iter().cloned());
    out
}

/// 解析 `--name-status` 输出为 `(路径, 状态)` 列表。
fn parse_name_status(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let status = parts.next()?.trim().to_string();
            let path = parts.next()?.trim().to_string();
            if status.is_empty() || path.is_empty() {
                None
            } else {
                Some((path, status))
            }
        })
        .collect()
}

/// 解析 `--numstat` 输出为 `路径 → (新增, 删除)`；二进制文件的 `-` 记 0。
fn parse_numstat_rows(text: &str) -> std::collections::HashMap<String, (i64, i64)> {
    let mut rows = std::collections::HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = path.trim().to_string();
        if path.is_empty() {
            continue;
        }
        rows.insert(
            path,
            (
                added.trim().parse::<i64>().unwrap_or(0),
                removed.trim().parse::<i64>().unwrap_or(0),
            ),
        );
    }
    rows
}

/// 变更文件清单：名字状态 + 每文件增删行数。
fn changed_files(root: &str, spec: &[String]) -> Vec<Value> {
    let name_refs = with_flag(spec, "--name-status");
    let name_args: Vec<&str> = name_refs.iter().map(|s| s.as_str()).collect();
    let entries = run_git_opt(root, &name_args)
        .map(|text| parse_name_status(&text))
        .unwrap_or_default();

    let num_refs = with_flag(spec, "--numstat");
    let num_args: Vec<&str> = num_refs.iter().map(|s| s.as_str()).collect();
    let stats = run_git_opt(root, &num_args)
        .map(|text| parse_numstat_rows(&text))
        .unwrap_or_default();

    entries
        .into_iter()
        .map(|(path, status)| {
            let (added, removed) = stats.get(&path).copied().unwrap_or((0, 0));
            json!({ "path": path, "status": status, "added": added, "removed": removed })
        })
        .collect()
}

fn add_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// 规约文件发现：仓库根的约定文件 + 每个变更文件所在目录链上的约定文件（去重）。
fn instruction_files(root: &str, changed: &[Value]) -> Vec<String> {
    let root_path = Path::new(root);
    let mut found: Vec<String> = Vec::new();
    for name in INSTRUCTION_FILES {
        if root_path.join(name).is_file() {
            add_unique(&mut found, (*name).to_string());
        }
    }
    for entry in changed {
        let Some(path) = entry.get("path").and_then(|value| value.as_str()) else {
            continue;
        };
        let mut directory = Path::new(path).parent();
        while let Some(current) = directory {
            if current.as_os_str().is_empty() {
                break;
            }
            for name in INSTRUCTION_FILES {
                if root_path.join(current).join(name).is_file() {
                    let relative = current.join(name).to_string_lossy().replace('\\', "/");
                    add_unique(&mut found, relative);
                }
            }
            directory = current.parent();
        }
    }
    found.sort();
    found
}

/// 按字符上限截断 diff：保留头尾并在中段标注省略量（按字符计，不切坏多字节）。
fn truncate_diff(diff: &str, max_chars: usize) -> (String, bool) {
    let total = diff.chars().count();
    if total <= max_chars {
        return (diff.to_string(), false);
    }
    let head_chars = max_chars / 2;
    let tail_chars = max_chars - head_chars;
    let head: String = diff.chars().take(head_chars).collect();
    let tail: String = diff.chars().skip(total - tail_chars).collect();
    let omitted = total - head_chars - tail_chars;
    (
        format!(
            "{head}\n\n…（已省略中间 {omitted} 字符；需要完整内容时对上方文件用 read_file 逐个读取）…\n\n{tail}"
        ),
        true,
    )
}

#[async_trait]
impl Tool for ReviewDiffTool {
    fn name(&self) -> &str {
        "review_diff"
    }

    fn description(&self) -> &str {
        "Collect the input for a read-only code review: resolve the target (uncommitted / a base \
         branch / a commit), return the diff, the changed-file list with line counts, untracked \
         files, and the repository instruction files (AGENTS.md / CODE_WIKI.md) that must be read \
         first. Read-only: never modifies the working tree."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "为只读代码审查取输入：解析目标（未提交改动 / 某基准分支 / 某个提交），\
            返回 diff、带增删行数的变更文件清单、未跟踪文件，以及必须先读的仓库规约文件\
            （AGENTS.md / CODE_WIKI.md）。只读，不会改动工作区。",
            "ja" => "読み取り専用のコードレビュー入力を収集する：対象（未コミット / 基準ブランチ / コミット）を解決し、\
            diff・行数付きの変更ファイル一覧・未追跡ファイル・先に読むべき規約ファイル（AGENTS.md / CODE_WIKI.md）を返す。\
            読み取り専用で、作業ツリーを変更しない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "审查这次改动\n看看这个 diff\n代码审查 变更",
            "en" => "review this diff\ncollect the changes for review",
            "ja" => "この変更をレビューして\ndiff を見せて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "enum": ["uncommitted", "base", "commit"],
                    "description": "What to review: 'uncommitted' (default) = staged+unstaged vs HEAD; 'base' = merge-base diff against a base branch; 'commit' = a single commit"
                },
                "base": {
                    "type": "string",
                    "description": "Base branch/tag/ref for target='base'; when omitted the default base is detected"
                },
                "commit": {
                    "type": "string",
                    "description": "Commit ref for target='commit'"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Max characters of diff text before truncation (default 60000)"
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["uncommitted", "base", "commit"],
                        "description": "审查目标：uncommitted（默认）= 已暂存+未暂存相对 HEAD；base = 相对基准分支的 merge-base 差异；commit = 单个提交"
                    },
                    "base": { "type": "string", "description": "target=base 时的基准分支/标签/引用；缺省则自动推测" },
                    "commit": { "type": "string", "description": "target=commit 时的提交引用" },
                    "max_chars": { "type": "integer", "description": "diff 文本字符上限（默认 60000），超出按头尾截断" }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["uncommitted", "base", "commit"],
                        "description": "レビュー対象：uncommitted（既定）= HEAD に対するステージ済+未ステージ；base = 基準ブランチとの merge-base 差分；commit = 単一コミット"
                    },
                    "base": { "type": "string", "description": "target=base の基準ブランチ/タグ/参照。省略時は既定基準を自動判定" },
                    "commit": { "type": "string", "description": "target=commit のコミット参照" },
                    "max_chars": { "type": "integer", "description": "diff テキストの文字数上限（既定 60000）。超過分は頭と末尾を残して省略" }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let target = input.get("target").and_then(|v| v.as_str()).unwrap_or("uncommitted");
        if !matches!(target, "uncommitted" | "base" | "commit") {
            return ValidationResult::failure("target 只能是 uncommitted / base / commit", 2);
        }
        if target == "commit"
            && !input
                .get("commit")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        {
            return ValidationResult::failure("target=commit 时必须提供 commit", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 只读探测：不写工作区、不建提交
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let dir = context.working_directory.trim().to_string();
        if dir.is_empty() {
            return ToolResult::standard_error("未选择工作区，无法解析审查目标", None, None);
        }
        if !Path::new(&dir).is_dir() {
            return ToolResult::standard_error(&format!("工作目录不存在：{dir}"), None, None);
        }
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("uncommitted")
            .trim()
            .to_string();
        let base = args.get("base").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let commit = args
            .get("commit")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1_000, MAX_MAX_CHARS))
            .unwrap_or(DEFAULT_MAX_CHARS);

        let Ok(root) = run_git(&dir, &["rev-parse", "--show-toplevel"]) else {
            return ToolResult::standard_error(&format!("工作目录不在 git 仓库内：{dir}"), None, None);
        };

        let (descriptor, spec, merge_base, base_ref) = match target.as_str() {
            "uncommitted" => (
                format!("未提交改动（{root}）"),
                vec!["diff".to_string(), "HEAD".to_string()],
                String::new(),
                String::new(),
            ),
            "base" => {
                let resolved_base = if base.is_empty() {
                    match detect_default_base(&root) {
                        Some(detected) => detected,
                        None => {
                            return ToolResult::standard_error(
                                "未能推测默认基准分支，请显式提供 base",
                                None,
                                None,
                            )
                        }
                    }
                } else {
                    base.clone()
                };
                let Some(merge_base) = run_git_opt(&root, &["merge-base", &resolved_base, "HEAD"])
                else {
                    return ToolResult::standard_error(
                        &format!("无法解析基准「{resolved_base}」与 HEAD 的 merge-base"),
                        None,
                        None,
                    );
                };
                (
                    format!("相对基准 {resolved_base} 的改动（merge-base {merge_base}）"),
                    vec!["diff".to_string(), merge_base.clone()],
                    merge_base,
                    resolved_base,
                )
            }
            "commit" => {
                if commit.is_empty() {
                    return ToolResult::standard_error("target=commit 时必须提供 commit", None, None);
                }
                // 根提交没有父：退回 `show --format=` 只给补丁
                let is_root =
                    run_git_opt(&root, &["rev-parse", "--verify", &format!("{commit}^")]).is_none();
                let spec = if is_root {
                    vec!["show".to_string(), "--format=".to_string(), commit.clone()]
                } else {
                    vec!["diff".to_string(), format!("{commit}^"), commit.clone()]
                };
                (format!("提交 {commit} 的改动"), spec, String::new(), String::new())
            }
            other => {
                return ToolResult::standard_error(
                    &format!("未知 target「{other}」：只支持 uncommitted / base / commit"),
                    None,
                    None,
                )
            }
        };

        let spec_args: Vec<&str> = spec.iter().map(|s| s.as_str()).collect();
        let diff = match run_git(&root, &spec_args) {
            Ok(diff) => diff,
            Err(error) => {
                return ToolResult::standard_error(&format!("读取 diff 失败：{error}"), None, None)
            }
        };

        let changed = changed_files(&root, &spec);
        let untracked: Vec<String> = run_git_opt(&root, &["ls-files", "--others", "--exclude-standard"])
            .map(|text| {
                text.lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let instruction_files = instruction_files(&root, &changed);
        let (diff_text, truncated) = truncate_diff(&diff, max_chars);

        let instruction_hint = if instruction_files.is_empty() {
            "（无）".to_string()
        } else {
            instruction_files.join("、")
        };
        let summary = format!(
            "审查目标：{descriptor}\n变更文件 {} 个，未跟踪 {} 个{}\n先读的规约文件：{instruction_hint}\n\n{diff_text}",
            changed.len(),
            untracked.len(),
            if truncated {
                format!("（diff 已截断至 {max_chars} 字符）")
            } else {
                String::new()
            }
        );

        ToolResult::standard_success(
            &summary,
            Some(json!({
                "target": target,
                "descriptor": descriptor,
                "root": root,
                "base_ref": base_ref,
                "merge_base": merge_base,
                "files": changed,
                "untracked": untracked,
                "instruction_files": instruction_files,
                "truncated": truncated,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "review diff 代码审查 变更 改动 评审 merge-base 规约 review_diff"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "提交或推送改动（改用 git_commit_push 或 run_command）",
            "查看单个文件内容（改用 read_file）",
            "执行任意 git 写操作（改用 run_command）",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_and_numstat_rows_parse() {
        let names = "M\tsrc/a.rs\nA\tsrc/b.rs\n";
        assert_eq!(
            parse_name_status(names),
            vec![
                ("src/a.rs".to_string(), "M".to_string()),
                ("src/b.rs".to_string(), "A".to_string()),
            ]
        );

        let stats = parse_numstat_rows("10\t2\tsrc/a.rs\n-\t-\tassets/logo.png\n");
        assert_eq!(stats.get("src/a.rs"), Some(&(10, 2)));
        assert_eq!(stats.get("assets/logo.png"), Some(&(0, 0)));
    }

    #[test]
    fn truncate_keeps_head_and_tail_with_marker() {
        let long = "字".repeat(100);
        let (text, truncated) = truncate_diff(&long, 40);
        assert!(truncated);
        assert!(text.contains("已省略中间"));
        // 截断按字符计，不会切坏多字节
        assert!(text.starts_with("字"));
    }

    #[test]
    fn short_diff_is_not_truncated() {
        let (text, truncated) = truncate_diff("short", 40);
        assert_eq!(text, "short");
        assert!(!truncated);
    }

    #[test]
    fn instruction_files_finds_root_convention_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "规则").unwrap();
        let root = dir.path().to_string_lossy().to_string();
        let changed = vec![json!({ "path": "src/main.rs", "status": "M", "added": 1, "removed": 0 })];
        let found = instruction_files(&root, &changed);
        assert!(found.iter().any(|entry| entry == "AGENTS.md"), "{found:?}");
    }
}
