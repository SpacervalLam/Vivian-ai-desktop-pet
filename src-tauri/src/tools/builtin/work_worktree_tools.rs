//! 工作树工具 —— 让工作智能体把改动放进一棵隔离的工作副本里做。
//!
//! 工作树（git worktree）是同一份 `.git` 挂出来的额外工作目录：共享提交历史，
//! 但各自拥有文件和 HEAD。拿它试错的代价很低 —— 改坏了整棵目录丢掉即可，
//! 不必在一个已经改乱的工作区里做反向操作。
//!
//! 三个 action 正好对应生命周期：`list` 看现状、`create` 开一处、`remove` 收一处。
//!
//! **边界（重要）**：本工具只负责建 / 查 / 删工作树，不负责授权。会话能读写的
//! 范围在开始时就已经定好（主工作区 + 附加工作区），运行中不扩；因此新建出来的
//! 路径要真的在里面读写，得先把它作为工作区打开。`create` 的返回值里会写明这一点，
//! 免得模型以为建完就能直接改。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::git::{git_worktree_add, git_worktree_list, git_worktree_remove};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

const DESC_ISOLATE_EN: &str = "Manage isolated git worktrees: extra working copies of the same repo that \
share its history but keep their own files, so a change can be tried without dirtying the current \
workspace — and the whole directory can be dropped if the attempt goes wrong. \
action=list shows every worktree with its branch and uncommitted state. \
action=create makes one at <repo parent>/<repo name>-worktrees/<name> on a new vivian/<name> branch. \
action=remove deletes one; a worktree with uncommitted changes needs force=true, which discards them. \
Note: creating a worktree does not by itself grant write access to it — the path must be opened as a \
workspace first.";

const DESC_ISOLATE_ZH: &str = "管理工作树（隔离执行）：同一仓库的额外工作副本，共享提交历史但拥有各自的文件。\
用它可以在不弄脏当前工作区的前提下试改，改坏了整棵目录丢掉即可。\
action=list 列出每棵工作树的分支与未提交状态；\
action=create 在 <仓库父目录>/<仓库名>-worktrees/<名字> 新建一棵，并签出 vivian/<名字> 分支；\
action=remove 移除一棵，有未提交改动时需 force=true（会一并丢弃那些改动）。\
注意：新建工作树本身不等于获得该目录的写权限，要真正在里面干活需先把它作为工作区打开。";

const DESC_ISOLATE_JA: &str = "ワークツリー（分離実行）を管理します。同じリポジトリの追加作業コピーで、\
履歴は共有しますがファイルは別々です。現在の作業ツリーを汚さずに変更を試せ、失敗したらフォルダごと\
捨てられます。action=list は各ワークツリーのブランチと未コミット状態を一覧します。\
action=create は <リポジトリの親>/<リポジトリ名>-worktrees/<名前> に新規作成し、vivian/<名前> \
ブランチを切ります。action=remove は削除します（未コミットがある場合は force=true が必要で、\
その変更も失われます）。なお、作成しただけではそのディレクトリへの書き込み権限は得られません。\
実際に作業するには先にワークスペースとして開く必要があります。";

/// 工作树生命周期管理工具。
pub struct WorkIsolateTool;

impl WorkIsolateTool {
    pub fn new() -> Self {
        Self
    }

    fn schema_for(lang: &str) -> Value {
        let (act, name, base, path, force) = match lang {
            "zh" => (
                "要执行的操作：list 列出全部工作树、create 新建一棵隔离工作树、remove 移除一棵",
                "工作树名字（create 时必填）：仅限字母、数字、中划线和下划线",
                "基于哪个分支或提交创建（create 时可选），留空则用当前 HEAD",
                "要移除的工作树路径（remove 时必填，取自 list 的 path）",
                "该工作树有未提交改动时是否强制移除（remove 时可选，默认 false；为 true 会丢掉那些改动）",
            ),
            "ja" => (
                "実行する操作：list で全ワークツリーを一覧、create で分離ワークツリーを作成、remove で削除",
                "ワークツリー名（create では必須）：英数字・ハイフン・アンダースコアのみ",
                "どのブランチ／コミットを基にするか（create では省略可）。空なら現在の HEAD",
                "削除するワークツリーのパス（remove では必須。list の path を使用）",
                "未コミットの変更があっても強制削除するか（remove では省略可、既定 false。true にするとその変更は失われます）",
            ),
            _ => (
                "Operation: list all worktrees, create a new isolated one, or remove an existing one",
                "Worktree name (required for create): letters, digits, hyphen and underscore only",
                "Branch or commit to base the new worktree on (optional for create); empty means current HEAD",
                "Path of the worktree to remove (required for remove; take it from list's path)",
                "Force removal even when the worktree has uncommitted changes (optional for remove, default false; true discards those changes)",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "remove"], "description": act },
                "name": { "type": "string", "description": name },
                "base": { "type": "string", "description": base },
                "path": { "type": "string", "description": path },
                "force": { "type": "boolean", "description": force }
            },
            "required": ["action"]
        })
    }
}

impl Default for WorkIsolateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorkIsolateTool {
    fn name(&self) -> &str {
        "work_isolate"
    }

    fn description(&self) -> &str {
        DESC_ISOLATE_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_ISOLATE_ZH,
            "ja" => DESC_ISOLATE_JA,
            _ => DESC_ISOLATE_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "另开一份试改\n别弄脏当前分支\n隔离着做\n开个工作树\n想试试这个改法\n改坏了直接丢掉",
            "en" => "try this somewhere else\ndon't dirty my branch\nisolate the change\nopen a worktree\njust test an idea",
            "ja" => "別の場所で試して\nブランチを汚さないで\n分離して作業\nワークツリーを開いて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema_for("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        Self::schema_for(lang)
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "list" => ValidationResult::success(Some(input.clone())),
            "create" => match input.get("name").and_then(|v| v.as_str()) {
                Some(n) if !n.trim().is_empty() => ValidationResult::success(Some(input.clone())),
                _ => ValidationResult::failure("create 需要 name（仅字母、数字、中划线、下划线）", 2),
            },
            "remove" => match input.get("path").and_then(|v| v.as_str()) {
                Some(p) if !p.trim().is_empty() => ValidationResult::success(Some(input.clone())),
                _ => ValidationResult::failure("remove 需要 path（取自 list 返回的 path）", 2),
            },
            _ => ValidationResult::failure("action 必须是 list、create 或 remove 之一", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list")
            .trim()
            .to_string();
        let dir = context.working_directory.trim().to_string();
        if dir.is_empty() {
            return ToolResult::standard_error(
                "当前会话未选择工作区，无法操作工作树",
                Some("WorktreeError"),
                None,
            );
        }

        match action.as_str() {
            "list" => {
                let list = git_worktree_list(dir);
                let items: Vec<Value> = list
                    .iter()
                    .map(|w| {
                        json!({
                            "name": w.name,
                            "path": w.path,
                            "branch": w.branch,
                            "detached": w.detached,
                            "is_main": w.is_main,
                            "dirty": w.dirty,
                            "changed_files": w.changed_files,
                            "head": w.head,
                        })
                    })
                    .collect();
                ToolResult::success(json!({ "count": items.len(), "worktrees": items }))
            }
            "create" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() {
                    return ToolResult::standard_error(
                        "create 需要 name",
                        Some("WorktreeError"),
                        None,
                    );
                }
                let base = args
                    .get("base")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let res = git_worktree_add(dir, name.clone(), base);
                if res.ok {
                    let path = res
                        .steps
                        .first()
                        .map(|s| s.output.clone())
                        .unwrap_or_default();
                    ToolResult::success(json!({
                        "ok": true,
                        "path": path,
                        "branch": format!("vivian/{name}"),
                        "note": "工作树已建好。当前会话的可读写范围在开始时确定、运行中不扩，\
                                 要真的在里面干活需先把它作为工作区打开；本工具只负责建/查/删，不代为授权。",
                    }))
                } else {
                    ToolResult::standard_error(
                        &res.error.unwrap_or_else(|| "创建工作树失败".into()),
                        Some("WorktreeError"),
                        None,
                    )
                }
            }
            "remove" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if path.is_empty() {
                    return ToolResult::standard_error(
                        "remove 需要 path",
                        Some("WorktreeError"),
                        None,
                    );
                }
                let force = args
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let res = git_worktree_remove(dir, path, force);
                if res.ok {
                    ToolResult::success(json!({ "ok": true }))
                } else {
                    ToolResult::standard_error(
                        &res.error.unwrap_or_else(|| "移除工作树失败".into()),
                        Some("WorktreeError"),
                        None,
                    )
                }
            }
            other => ToolResult::standard_error(
                &format!("未知 action：{other}（可用 list / create / remove）"),
                Some("WorktreeError"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        // list 只读，但本工具会建/删目录，整体按写操作对待。
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::File
    }

    fn risk(&self) -> ToolRiskTier {
        // create / remove 都会落盘改动（remove 还可能丢掉未提交内容）
        ToolRiskTier::FsWrite
    }
}
