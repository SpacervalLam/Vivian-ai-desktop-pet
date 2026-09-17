//! Git 仓库状态命令 —— 工作页右上角「置顶摘要 · 环境信息」的数据源
//!
//! 设计取向：**全部走系统 `git` CLI，不引入 libgit2 / git2**。
//! 工作页本来就把 git 操作交给终端与 `run_command`，这里再挂一份 C 库依赖只会
//! 让构建变重；而 CLI 的输出格式（`--porcelain` / `--numstat` / `--pretty=format`）
//! 足够稳定，解析成本很低。
//!
//! 只读探测与写操作分开：
//! - 只读：`git_repo_status` / `git_branch_diff` / `git_list_branches`，可被前端轮询。
//! - 写：`git_commit_push`。只在用户点「提交或推送」并经前端确认后调用；
//!   后端不替用户决定提交信息，也不做任何隐式 add（add -A 是显式步骤，结果回传）。
//!
//! 所有命令都以 `-C <dir>` 指定工作目录，不依赖进程 cwd —— 前端可能同时开着
//! 多个会话，进程级 cwd 是有状态且会互相踩的。

use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

// ============ 进程调用 ============

/// Windows 下隐藏 git 弹出的控制台窗口。
///
/// 不加这个标志时，每次轮询状态都会在屏幕上闪一次黑框——状态是定时刷新的，
/// 闪烁频率足以让人无法工作。
#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

/// 执行一次 git 子命令，成功返回 stdout（已 trim）。
///
/// `core.quotepath=false` 是必须的：默认 git 会把非 ASCII 路径转义成
/// `\344\270\255` 这种八进制串，中文目录名在界面上会变成天书。
fn run_git(dir: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-c")
        .arg("core.quotepath=false")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null());
    hide_console(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| format!("无法执行 git（未安装或不在 PATH 中）：{e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let fallback = format!("git {} 执行失败", args.join(" "));
        return Err(if err.is_empty() { fallback } else { err });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 同 `run_git`，但把失败折叠成 `None`。
///
/// 大量 git 探测的「失败」本身就是有效信息：没有 upstream、没有 origin、
/// 空仓库没有 HEAD —— 这些不是异常，不该冒泡成错误。
fn run_git_opt(dir: &str, args: &[&str]) -> Option<String> {
    run_git(dir, args).ok().filter(|s| !s.is_empty())
}

// ============ 数据结构 ============

/// 一条提交摘要
#[derive(Debug, Clone, Serialize, Default)]
pub struct GitCommitInfo {
    /// 短 hash
    pub hash: String,
    /// 提交说明首行
    pub subject: String,
    pub author: String,
    /// ISO 8601 作者时间
    pub time: String,
}

/// 仓库状态快照（置顶摘要「环境信息」整块的数据）
#[derive(Debug, Clone, Serialize, Default)]
pub struct GitRepoStatus {
    /// 工作目录是否位于 git 仓库内
    pub is_repo: bool,
    /// 仓库根目录（绝对路径）
    pub root: String,
    /// 当前分支名；detached HEAD 时为空
    pub branch: String,
    /// 是否处于 detached HEAD
    pub detached: bool,
    /// 上游分支全名（如 `origin/main`）；未设置跟踪分支时为空
    pub upstream: Option<String>,
    /// 领先上游的提交数
    pub ahead: i32,
    /// 落后上游的提交数
    pub behind: i32,
    /// 工作区相对 HEAD 的新增行数（含未跟踪文件）
    pub added: i32,
    /// 工作区相对 HEAD 的删除行数
    pub removed: i32,
    /// 有改动的文件数（暂存 + 未暂存 + 未跟踪）
    pub changed_files: i32,
    /// 已暂存文件数
    pub staged: i32,
    /// 已修改未暂存文件数
    pub unstaged: i32,
    /// 未跟踪文件数
    pub untracked: i32,
    /// 是否存在任何未提交改动
    pub dirty: bool,
    pub last_commit: Option<GitCommitInfo>,
    /// origin 远端地址
    pub remote_url: Option<String>,
    /// 探测失败原因（目录不存在 / 未安装 git）；非仓库时为 None
    pub error: Option<String>,
}

/// 单个写操作步骤的结果（前端逐条打勾展示）
#[derive(Debug, Clone, Serialize)]
pub struct GitStepResult {
    /// 步骤名：`add` / `commit` / `push`
    pub step: String,
    pub ok: bool,
    /// stdout 摘要；失败时是 stderr
    pub output: String,
}

/// 提交 / 推送的总体结果
#[derive(Debug, Clone, Serialize, Default)]
pub struct GitActionResult {
    pub ok: bool,
    pub steps: Vec<GitStepResult>,
    /// 失败原因（人类可读）
    pub error: Option<String>,
}

/// 两个分支的差异摘要
#[derive(Debug, Clone, Serialize, Default)]
pub struct GitBranchDiff {
    /// 比较基准（如 `origin/main`）
    pub base: String,
    /// 当前分支
    pub current: String,
    /// 当前分支领先基准的提交数
    pub ahead: i32,
    /// 当前分支落后基准的提交数
    pub behind: i32,
    /// 相对基准的新增行数
    pub added: i32,
    /// 相对基准的删除行数
    pub removed: i32,
    /// 领先的提交列表（最多 50 条）
    pub commits: Vec<GitCommitInfo>,
    pub error: Option<String>,
}

/// 分支清单（「比较分支」下拉用）
#[derive(Debug, Clone, Serialize, Default)]
pub struct GitBranchList {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    /// 推测的默认基准分支（origin/HEAD → origin/main → origin/master → main → master）
    pub default_base: Option<String>,
}

// ============ 只读探测 ============

/// 未跟踪文件的行数上限：超过就不再读内容，避免 `git status` 被一个
/// node_modules 大小的目录拖成秒级卡顿。
const UNTRACKED_MAX_FILES: usize = 200;
const UNTRACKED_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 统计未跟踪文件带来的新增行数。
///
/// `git diff --numstat HEAD` 看不见未跟踪文件，而「变更 +N」在用户眼里理应
/// 包含刚新建、还没 add 的文件——否则刚写的代码在摘要里显示为「无改动」。
fn count_untracked_lines(dir: &str) -> i32 {
    let Some(list) = run_git_opt(dir, &["ls-files", "--others", "--exclude-standard"]) else {
        return 0;
    };
    let mut total = 0i32;
    let mut budget = UNTRACKED_MAX_BYTES;

    for rel in list.lines().take(UNTRACKED_MAX_FILES) {
        let rel = rel.trim();
        if rel.is_empty() {
            continue;
        }
        let full = Path::new(dir).join(rel);
        let Ok(meta) = std::fs::metadata(&full) else { continue };
        if !meta.is_file() || meta.len() > budget {
            continue;
        }
        budget = budget.saturating_sub(meta.len());
        // 按字节读再数换行：比 read_to_string 宽容，遇到非 UTF-8 文件不会整块丢弃
        if let Ok(bytes) = std::fs::read(&full) {
            let mut lines = bytes.iter().filter(|b| **b == b'\n').count() as i32;
            // 最后一行没有换行符时也要算一行
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                lines += 1;
            }
            total = total.saturating_add(lines);
        }
    }
    total
}

/// 解析 `--porcelain` 输出，返回 (已暂存, 未暂存, 未跟踪)。
///
/// porcelain v1 每行前两个字符是 XY：X = 暂存区相对 HEAD 的状态，
/// Y = 工作区相对暂存区的状态。`??` 是未跟踪。注意 `git status --porcelain`
/// 对「未跟踪目录」只输出一行目录名，所以这里数的是条目数而非文件数——
/// 与 git 自身在 `status` 里的计数口径一致。
fn parse_porcelain(text: &str) -> (i32, i32, i32) {
    let (mut staged, mut unstaged, mut untracked) = (0i32, 0i32, 0i32);
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let bytes = line.as_bytes();
        let (x, y) = (bytes[0] as char, bytes[1] as char);
        if x == '?' && y == '?' {
            untracked += 1;
            continue;
        }
        if x != ' ' && x != '?' {
            staged += 1;
        }
        if y != ' ' && y != '?' {
            unstaged += 1;
        }
    }
    (staged, unstaged, untracked)
}

/// 解析 `--numstat` 输出，累加增删行数。
///
/// 二进制文件的增删列是 `-`，跳过即可；路径里可能含制表符（少见），
/// 所以只取前两列、剩下的全部当路径。
fn parse_numstat(text: &str) -> (i32, i32) {
    let (mut added, mut removed) = (0i32, 0i32);
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(a), Some(r)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let Ok(n) = a.trim().parse::<i32>() {
            added = added.saturating_add(n);
        }
        if let Ok(n) = r.trim().parse::<i32>() {
            removed = removed.saturating_add(n);
        }
    }
    (added, removed)
}

/// 读取一条提交摘要。字段用 `\x1f`（unit separator）分隔：
/// 提交说明里出现空格、竖线、制表符都很常见，用可见字符当分隔符会串列。
fn read_commit(dir: &str, rev: &str) -> Option<GitCommitInfo> {
    let raw = run_git_opt(dir, &["log", "-1", "--pretty=format:%h%x1f%s%x1f%an%x1f%aI", rev])?;
    let mut it = raw.split('\u{1f}');
    let hash = it.next()?.trim().to_string();
    if hash.is_empty() {
        return None;
    }
    Some(GitCommitInfo {
        hash,
        subject: it.next().unwrap_or("").trim().to_string(),
        author: it.next().unwrap_or("").trim().to_string(),
        time: it.next().unwrap_or("").trim().to_string(),
    })
}

/// 查询工作目录的仓库状态。目录不是仓库时返回 `is_repo=false` 而非报错。
#[tauri::command]
pub fn git_repo_status(working_directory: String) -> GitRepoStatus {
    let dir = working_directory.trim();
    if dir.is_empty() {
        return GitRepoStatus {
            error: Some("未选择工作区".into()),
            ..Default::default()
        };
    }
    if !Path::new(dir).is_dir() {
        return GitRepoStatus {
            error: Some("工作目录不存在".into()),
            ..Default::default()
        };
    }

    let root = match run_git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(r) => r,
        Err(_) => {
            // 非零退出 = 不在仓库内。这是常态（用户可能就打开了一个普通文件夹），
            // 不写 error，由前端按 is_repo=false 展示引导文案。
            return GitRepoStatus::default();
        }
    };

    // 分支：detached HEAD 时 `--abbrev-ref HEAD` 返回字面量 "HEAD"
    let branch_raw = run_git_opt(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let detached = branch_raw == "HEAD" || branch_raw.is_empty();
    let branch = if detached { String::new() } else { branch_raw };

    // 上游跟踪分支
    let upstream = run_git_opt(
        &root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    );

    // 领先 / 落后：`--left-right --count HEAD...@{upstream}` 输出 "ahead\tbehind"
    let (mut ahead, mut behind) = (0i32, 0i32);
    if upstream.is_some() {
        if let Some(raw) = run_git_opt(&root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
            let mut nums = raw.split_whitespace().filter_map(|s| s.parse::<i32>().ok());
            ahead = nums.next().unwrap_or(0);
            behind = nums.next().unwrap_or(0);
        }
    }

    // 工作区改动：`diff HEAD` 覆盖「已暂存 + 未暂存」两部分
    let tracked_stat = run_git_opt(&root, &["diff", "HEAD", "--numstat"]).unwrap_or_default();
    let (mut added, removed) = parse_numstat(&tracked_stat);

    let porcelain = run_git_opt(&root, &["status", "--porcelain"]).unwrap_or_default();
    let (staged, unstaged, untracked) = parse_porcelain(&porcelain);

    // 未跟踪文件的行数单独数，计入新增
    if untracked > 0 {
        added = added.saturating_add(count_untracked_lines(&root));
    }

    let last_commit = read_commit(&root, "HEAD");
    let remote_url = run_git_opt(&root, &["remote", "get-url", "origin"]);

    GitRepoStatus {
        is_repo: true,
        root,
        branch,
        detached,
        upstream,
        ahead,
        behind,
        added,
        removed,
        changed_files: staged + unstaged + untracked,
        staged,
        unstaged,
        untracked,
        dirty: !porcelain.is_empty(),
        last_commit,
        remote_url,
        error: None,
    }
}

/// 列出本地与远端分支，并推测一个默认比较基准。
#[tauri::command]
pub fn git_list_branches(working_directory: String) -> GitBranchList {
    let dir = working_directory.trim();
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return GitBranchList::default();
    }
    let local: Vec<String> = run_git_opt(dir, &["branch", "--format=%(refname:short)"])
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let remote: Vec<String> = run_git_opt(dir, &["branch", "-r", "--format=%(refname:short)"])
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                // `origin/HEAD` 是指针不是分支，混进下拉里会让用户选到一个不可比较的项
                .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // 基准优先级：远端默认 HEAD 指向的分支 > origin/main > origin/master > main > master
    let remote_head = run_git_opt(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .map(|s| s.trim().to_string());
    let default_base = remote_head
        .filter(|s| !s.is_empty())
        .or_else(|| {
            ["origin/main", "origin/master", "main", "master"]
                .iter()
                .find(|c| local.iter().any(|l| l == *c) || remote.iter().any(|r| r == *c))
                .map(|c| c.to_string())
        });

    GitBranchList {
        local,
        remote,
        default_base,
    }
}

/// 比较当前分支与基准分支的差异（「比较分支」用）。
#[tauri::command]
pub fn git_branch_diff(working_directory: String, base: String) -> GitBranchDiff {
    let dir = working_directory.trim().to_string();
    let base = base.trim().to_string();
    let mut out = GitBranchDiff {
        base: base.clone(),
        ..Default::default()
    };
    if dir.is_empty() || base.is_empty() {
        out.error = Some("缺少工作目录或基准分支".into());
        return out;
    }

    out.current = run_git_opt(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();

    // `base...HEAD`（三点）比的是 merge-base 到 HEAD，即「本分支独有的改动」；
    // 两点会把基准分支自己的新提交也算成差异，结论会误导人。
    match run_git(&dir, &["diff", "--numstat", &format!("{base}...HEAD")]) {
        Ok(text) => {
            let (a, r) = parse_numstat(&text);
            out.added = a;
            out.removed = r;
        }
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    }

    if let Some(raw) = run_git_opt(&dir, &["rev-list", "--left-right", "--count", &format!("{base}...HEAD")]) {
        let mut nums = raw.split_whitespace().filter_map(|s| s.parse::<i32>().ok());
        out.behind = nums.next().unwrap_or(0);
        out.ahead = nums.next().unwrap_or(0);
    }

    // 提交列表：最多 50 条，够表达「领先了什么」，又不至于把面板撑爆
    if let Some(raw) = run_git_opt(
        &dir,
        &["log", "--max-count=50", "--pretty=format:%h%x1f%s%x1f%an%x1f%aI", &format!("{base}..HEAD")],
    ) {
        out.commits = raw
            .lines()
            .filter_map(|line| {
                let mut it = line.split('\u{1f}');
                let hash = it.next()?.trim().to_string();
                if hash.is_empty() {
                    return None;
                }
                Some(GitCommitInfo {
                    hash,
                    subject: it.next().unwrap_or("").trim().to_string(),
                    author: it.next().unwrap_or("").trim().to_string(),
                    time: it.next().unwrap_or("").trim().to_string(),
                })
            })
            .collect();
    }

    out
}

// ============ 写操作 ============

/// 暂存全部改动 → 提交 →（可选）推送。
///
/// 调用前提：前端已弹确认框、用户明确点了执行。后端不做二次询问，
/// 但也不做任何「自作主张」的事——不自动改写提交信息、不 force push、
/// 不在没有 upstream 时猜远端。
#[tauri::command]
pub fn git_commit_push(
    working_directory: String,
    message: String,
    push: bool,
) -> GitActionResult {
    let dir = working_directory.trim().to_string();
    let message = message.trim().to_string();
    let mut result = GitActionResult::default();

    if dir.is_empty() {
        result.error = Some("未选择工作区".into());
        return result;
    }
    if message.is_empty() {
        result.error = Some("提交说明不能为空".into());
        return result;
    }

    // ① add -A：显式全量暂存。用户点的是「提交」，期望包含所有改动；
    //    只暂存部分文件的行为留给终端里的精细操作。
    match run_git(&dir, &["add", "-A"]) {
        Ok(out) => result.steps.push(GitStepResult {
            step: "add".into(),
            ok: true,
            output: out,
        }),
        Err(e) => {
            result.steps.push(GitStepResult {
                step: "add".into(),
                ok: false,
                output: e.clone(),
            });
            result.error = Some(format!("暂存失败：{e}"));
            return result;
        }
    }

    // ② commit
    match run_git(&dir, &["commit", "-m", &message]) {
        Ok(out) => result.steps.push(GitStepResult {
            step: "commit".into(),
            ok: true,
            output: out,
        }),
        Err(e) => {
            result.steps.push(GitStepResult {
                step: "commit".into(),
                ok: false,
                output: e.clone(),
            });
            result.error = Some(e);
            return result;
        }
    }

    if !push {
        result.ok = true;
        return result;
    }

    // ③ push：没有 upstream 时用 `-u origin <branch>` 建立跟踪关系，
    //    否则 `git push` 会直接失败并让用户困惑。
    let upstream = run_git_opt(&dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    let mut push_args: Vec<String> = vec!["push".into()];
    if upstream.is_none() {
        let branch = run_git_opt(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
        if branch.is_empty() || branch == "HEAD" {
            result.error = Some("当前处于 detached HEAD，无法自动建立上游分支，请先切回分支".into());
            return result;
        }
        push_args.push("-u".into());
        push_args.push("origin".into());
        push_args.push(branch);
    }
    let push_refs: Vec<&str> = push_args.iter().map(|s| s.as_str()).collect();

    match run_git(&dir, &push_refs) {
        Ok(out) => result.steps.push(GitStepResult {
            step: "push".into(),
            ok: true,
            output: out,
        }),
        Err(e) => {
            result.steps.push(GitStepResult {
                step: "push".into(),
                ok: false,
                output: e.clone(),
            });
            result.error = Some(format!("推送失败：{e}"));
            return result;
        }
    }

    result.ok = true;
    result
}

// ============ 测试 ============

#[cfg(test)]
mod tests {
    use super::*;

    /// porcelain 的 XY 两列语义：X 相对 HEAD（暂存），Y 相对暂存区（工作区）。
    /// `??` 是未跟踪，不进前两个计数——否则未跟踪文件会被算成「已修改」。
    #[test]
    fn porcelain_splits_staged_unstaged_untracked() {
        let text = "M  src/a.rs\n M src/b.rs\nMM src/c.rs\nA  src/d.rs\n?? src/new.rs\n?? docs/";
        let (staged, unstaged, untracked) = parse_porcelain(text);
        assert_eq!(staged, 3, "M_ / MM / A_ 三个进暂存");
        assert_eq!(unstaged, 2, "_M / MM 两个进工作区");
        assert_eq!(untracked, 2, "两个 ?? 条目");
    }

    /// 二进制文件在 numstat 里是 `-\t-`，解析必须跳过而不是当成 0 或报错；
    /// 路径列允许含制表符，所以只能 splitn(3)。
    #[test]
    fn numstat_skips_binary_and_sums_rest() {
        let text = "10\t2\tsrc/a.rs\n-\t-\tassets/logo.png\n3\t0\tsrc/with\ttab.rs";
        let (added, removed) = parse_numstat(text);
        assert_eq!(added, 13);
        assert_eq!(removed, 2);
    }

    /// 空输入不能 panic，也不能给出非零值。
    #[test]
    fn empty_inputs_are_safe() {
        assert_eq!(parse_porcelain(""), (0, 0, 0));
        assert_eq!(parse_numstat(""), (0, 0));
        // 只有两列、没有路径的畸形行应被忽略
        assert_eq!(parse_numstat("5\t1"), (5, 1));
    }

    /// 工作区为空时直接返回提示，不去起 git 进程。
    #[test]
    fn empty_workspace_short_circuits() {
        let s = git_repo_status(String::new());
        assert!(!s.is_repo);
        assert!(s.error.is_some());
    }
}
