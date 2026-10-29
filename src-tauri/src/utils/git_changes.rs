use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_PATCH_BYTES: usize = 256 * 1024;
const MAX_FILES: usize = 2000;

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub original_path: Option<String>,
    pub index_status: char,
    pub worktree_status: char,
    pub untracked: bool,
    pub conflict: bool,
}

impl ChangedFile {
    fn in_scope(&self, scope: &str) -> bool {
        match scope {
            "all" => true,
            "staged" => !self.untracked && self.index_status != ' ',
            "unstaged" => self.untracked || self.worktree_status != ' ',
            _ => false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ChangesSnapshot {
    pub is_repo: bool,
    pub root: String,
    pub files: Vec<ChangedFile>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct FileDiff {
    pub patch: String,
    pub revision: String,
    pub binary: bool,
    pub truncated: bool,
}

struct GitOutput {
    ok: bool,
    bytes: Vec<u8>,
    error: String,
    truncated: bool,
}

fn read_bounded(mut stream: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    std::io::copy(&mut stream, &mut std::io::sink())?;
    Ok((bytes, truncated))
}

fn git(dir: &str, args: &[&str], limit: usize) -> Result<GitOutput, String> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-C",
            dir,
        ])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("Git: {e}"))?;
    let stdout = child.stdout.take().ok_or("Git stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Git stderr unavailable")?;
    let output = std::thread::spawn(move || read_bounded(stdout, limit));
    let errors = std::thread::spawn(move || read_bounded(stderr, 8192));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(15) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(error) => format!("Git: {error}"),
                    _ => "Git timed out after 15 seconds".into(),
                });
            }
        }
    };
    let (bytes, truncated) = output
        .join()
        .map_err(|_| "Git output reader failed")?
        .map_err(|e| e.to_string())?;
    let (error, _) = errors
        .join()
        .map_err(|_| "Git error reader failed")?
        .map_err(|e| e.to_string())?;
    Ok(GitOutput {
        ok: status?.success(),
        bytes,
        truncated,
        error: String::from_utf8_lossy(&error).trim().into(),
    })
}

fn checked(output: GitOutput) -> Result<GitOutput, String> {
    if output.ok {
        Ok(output)
    } else {
        Err(output.error)
    }
}

fn parse_status(bytes: &[u8]) -> Result<Vec<ChangedFile>, String> {
    let mut records = bytes.split(|b| *b == 0).filter(|part| !part.is_empty());
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err("Invalid Git status record".into());
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let decode = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec()).map_err(|_| "Git path is not valid UTF-8".to_string())
        };
        let path = decode(&record[3..])?;
        let original_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            Some(decode(records.next().ok_or("Missing Git rename source")?)?)
        } else {
            None
        };
        files.push(ChangedFile {
            path,
            original_path,
            index_status: x,
            worktree_status: y,
            untracked: x == '?' && y == '?',
            conflict: x == 'U' || y == 'U' || matches!((x, y), ('A', 'A') | ('D', 'D')),
        });
    }
    Ok(files)
}

pub fn changes(dir: &str, scope: &str) -> Result<ChangesSnapshot, String> {
    if !matches!(scope, "all" | "staged" | "unstaged") {
        return Err("Invalid diff scope".into());
    }
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return Err("Workspace directory does not exist".into());
    }
    let probe = git(dir, &["rev-parse", "--show-toplevel"], 32768)?;
    if !probe.ok && probe.error.contains("not a git repository") {
        return Ok(ChangesSnapshot {
            is_repo: false,
            root: String::new(),
            files: Vec::new(),
            truncated: false,
        });
    }
    let probe = checked(probe)?;
    let root = String::from_utf8(probe.bytes)
        .map_err(|e| e.to_string())?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    let output = checked(git(
        &root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        2 * 1024 * 1024,
    )?)?;
    if output.truncated {
        return Err("Git status exceeds the 2 MiB limit".into());
    }
    let mut files: Vec<_> = parse_status(&output.bytes)?
        .into_iter()
        .filter(|f| f.in_scope(scope))
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    Ok(ChangesSnapshot {
        is_repo: true,
        root,
        files,
        truncated,
    })
}

fn addition(root: &str, path: &str) -> Result<(Vec<u8>, bool), String> {
    let full = Path::new(root).join(path);
    let metadata = std::fs::symlink_metadata(&full).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Only regular files can be previewed as new files".into());
    }
    let canonical = full.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(Path::new(root).canonicalize().map_err(|e| e.to_string())?) {
        return Err("File is outside the repository".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&canonical)
        .map_err(|e| e.to_string())?
        .take((MAX_PATCH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() > MAX_PATCH_BYTES;
    bytes.truncate(MAX_PATCH_BYTES);
    if truncated {
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
    }
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Ok((b"Binary files differ\n".to_vec(), truncated));
    }
    let text = String::from_utf8_lossy(&bytes);
    let old_path = format!("a/{path}");
    let new_path = format!("b/{path}");
    let mut patch = format!("diff --git {old_path:?} {new_path:?}\nnew file mode 100644\n--- /dev/null\n+++ {new_path:?}\n");
    if !bytes.is_empty() {
        patch.push_str(&format!("@@ -0,0 +1,{} @@\n", text.lines().count()));
        for line in text.split_terminator('\n') {
            patch.push('+');
            patch.push_str(line);
            patch.push('\n');
        }
        if !text.ends_with('\n') && !truncated {
            patch.push_str("\\ No newline at end of file\n");
        }
    }
    Ok((patch.into_bytes(), truncated))
}

pub fn diff(dir: &str, path: &str, scope: &str) -> Result<FileDiff, String> {
    let snapshot = changes(dir, scope)?;
    let file = snapshot
        .files
        .iter()
        .find(|f| f.path == path)
        .ok_or("File is no longer changed in this scope")?;
    let has_head = checked(git(
        &snapshot.root,
        &["rev-parse", "--verify", "HEAD"],
        128,
    )?)
    .is_ok();
    let (bytes, mut truncated) = if !has_head && scope == "all" && file.worktree_status == 'D' {
        (Vec::new(), false)
    } else if file.untracked || (!has_head && scope == "all") {
        addition(&snapshot.root, path)?
    } else {
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--no-color",
            "--unified=3",
        ];
        if scope == "staged" {
            args.push("--cached");
        } else if scope == "all" {
            args.push("HEAD");
        }
        args.extend(["--", path]);
        if let Some(original) = &file.original_path {
            args.push(original);
        }
        let output = checked(git(&snapshot.root, &args, MAX_PATCH_BYTES)?)?;
        (output.bytes, output.truncated)
    };
    let revision = format!("{:x}", Sha256::digest(&bytes));
    let binary = bytes
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"Binary files "));
    let text = String::from_utf8_lossy(&bytes);
    let mut patch = String::new();
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index >= 4000 {
            truncated = true;
            break;
        }
        if truncated && !line.ends_with('\n') {
            break;
        }
        patch.push_str(line);
    }
    Ok(FileDiff {
        patch,
        revision,
        binary,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn command(root: &Path, args: &[&str]) -> Vec<u8> {
        let result = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    fn repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        command(dir.path(), &["init", "-q"]);
        command(dir.path(), &["config", "user.name", "Fixture"]);
        command(
            dir.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        command(dir.path(), &["config", "core.autocrlf", "false"]);
        dir
    }

    fn commit(root: &Path) {
        command(root, &["add", "-A"]);
        command(root, &["commit", "-qm", "fixture"]);
    }

    #[test]
    fn nul_status_preserves_whitespace_unicode_rename_and_conflicts() {
        let files = parse_status(b" M  leading.txt\0R  \xe4\xb8\xad\xe6\x96\x87.txt\0old\tname\n.txt\0UU conflict\0?? [x].txt\0").unwrap();
        assert_eq!(files[0].path, " leading.txt");
        assert_eq!(files[1].path, "中文.txt");
        assert_eq!(files[1].original_path.as_deref(), Some("old\tname\n.txt"));
        assert!(files[2].conflict);
        assert!(files[3].untracked);
    }

    #[test]
    fn staged_unstaged_and_combined_diffs_read_actual_git_state_without_writing() {
        let dir = repo();
        let root = dir.path();
        let path = "[x] 中文.txt";
        std::fs::write(root.join(path), "original\n").unwrap();
        commit(root);
        std::fs::write(root.join(path), "staged\n").unwrap();
        command(root, &["--literal-pathspecs", "add", "--", path]);
        std::fs::write(root.join(path), "working\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(root.join("ignored"), "secret").unwrap();
        let before = command(root, &["status", "--porcelain=v1", "-z"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let cwd = root.to_str().unwrap();
        let staged = diff(cwd, path, "staged").unwrap();
        assert!(staged.patch.contains("+staged"));
        assert!(!staged.patch.contains("+working"));
        let unstaged = diff(cwd, path, "unstaged").unwrap();
        assert!(unstaged.patch.contains("-staged\n+working"));
        let combined = diff(cwd, path, "all").unwrap();
        assert!(combined.patch.contains("-original\n+working"));
        assert_ne!(combined.revision, staged.revision);
        assert!(!changes(cwd, "all")
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == "ignored"));
        assert_eq!(before, command(root, &["status", "--porcelain=v1", "-z"]));
        assert_eq!(index, std::fs::read(root.join(".git/index")).unwrap());
    }

    #[test]
    fn unborn_repository_untracked_empty_and_binary_files_are_visible() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("new.txt"), "index\n").unwrap();
        command(root, &["add", "new.txt"]);
        std::fs::write(root.join("new.txt"), "working\n").unwrap();
        std::fs::write(root.join("empty"), "").unwrap();
        std::fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        let cwd = root.to_str().unwrap();
        assert_eq!(changes(cwd, "staged").unwrap().files.len(), 1);
        assert_eq!(changes(cwd, "unstaged").unwrap().files.len(), 3);
        assert!(diff(cwd, "new.txt", "staged")
            .unwrap()
            .patch
            .contains("+index"));
        assert!(diff(cwd, "new.txt", "all")
            .unwrap()
            .patch
            .contains("+working"));
        assert!(diff(cwd, "empty", "all")
            .unwrap()
            .patch
            .contains("new file mode"));
        assert!(diff(cwd, "binary", "all").unwrap().binary);
        std::fs::remove_file(root.join("new.txt")).unwrap();
        assert!(diff(cwd, "new.txt", "all").unwrap().patch.is_empty());
    }

    #[test]
    fn rename_deletion_nested_workspace_and_binary_tracked_diff() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("old.txt"), "same\n").unwrap();
        std::fs::write(root.join("deleted.txt"), "deleted\n").unwrap();
        std::fs::write(root.join("image"), [0, 1, 2]).unwrap();
        commit(root);
        command(root, &["mv", "old.txt", "new.txt"]);
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        std::fs::write(root.join("image"), [0, 4, 2]).unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        let cwd = root.join("nested");
        let cwd = cwd.to_str().unwrap();
        let snapshot = changes(cwd, "all").unwrap();
        assert_eq!(snapshot.files.len(), 3);
        let renamed = snapshot.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(renamed.original_path.as_deref(), Some("old.txt"));
        assert!(diff(cwd, "new.txt", "all")
            .unwrap()
            .patch
            .contains("rename from old.txt"));
        assert!(diff(cwd, "deleted.txt", "all")
            .unwrap()
            .patch
            .contains("-deleted"));
        assert!(diff(cwd, "image", "all").unwrap().binary);
    }

    #[test]
    fn invalid_scopes_outside_paths_and_non_repositories() {
        let plain = tempfile::tempdir().unwrap();
        assert!(
            !changes(plain.path().to_str().unwrap(), "all")
                .unwrap()
                .is_repo
        );
        let dir = repo();
        let cwd = dir.path().to_str().unwrap();
        assert!(changes(cwd, "--output=bad").is_err());
        assert!(diff(cwd, "../secret", "all").is_err());
        assert!(diff(cwd, ":(glob)*", "all").is_err());
        assert!(changes("", "all").is_err());
    }

    #[test]
    fn large_unicode_content_is_bounded_and_external_diff_is_disabled() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("tracked"), "before\n").unwrap();
        commit(root);
        command(
            root,
            &[
                "config",
                "diff.external",
                "vivian-nonexistent-diff-executable",
            ],
        );
        std::fs::write(root.join("tracked"), "after\nBinary files are logged as text\n").unwrap();
        std::fs::write(root.join("large"), "中\n".repeat(100_000)).unwrap();
        let cwd = root.to_str().unwrap();
        let tracked = diff(cwd, "tracked", "all").unwrap();
        assert!(tracked.patch.contains("+after"));
        assert!(!tracked.binary, "ordinary text mentioning binary files is still text");
        let large = diff(cwd, "large", "all").unwrap();
        assert!(large.truncated);
        assert!(!large.binary);
        assert!(large.patch.lines().count() <= 4000);
        assert!(!large.patch.contains('\u{fffd}'));
        let (bytes, truncated) = read_bounded(&b"abcdef"[..], 3).unwrap();
        assert_eq!(bytes, b"abc");
        assert!(truncated);
    }
}
