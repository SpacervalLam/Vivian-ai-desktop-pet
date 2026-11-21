//! Layered explicit repository instructions, independent of optional learned memories.
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: usize = 32 * 1024;
#[derive(Debug, Clone, Serialize)]
pub struct Instruction {
    pub path: PathBuf,
    pub scope: PathBuf,
    pub content: String,
}

pub fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|p| p.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// One nonempty file per directory, override first. Canonical paths prevent symlink escapes.
pub fn discover(global: &Path, cwd: &Path, targets: &[PathBuf]) -> Vec<Instruction> {
    let mut directories = vec![global.to_path_buf()];
    if let Ok(cwd) = cwd.canonicalize() {
        let root = project_root(&cwd);
        let mut chain: Vec<_> = cwd
            .ancestors()
            .take_while(|p| p.starts_with(&root))
            .map(Path::to_path_buf)
            .collect();
        chain.reverse();
        directories.extend(chain);
        for target in targets.iter().take(64) {
            let target = if target.is_absolute() {
                target.clone()
            } else {
                cwd.join(target)
            };
            // Nonexistent leaf files are common. Resolve the nearest existing directory before
            // checking containment, so ../ and directory symlinks cannot add outside guidance.
            let Some(parent) = target
                .parent()
                .and_then(|p| p.ancestors().find_map(|a| a.canonicalize().ok()))
            else {
                continue;
            };
            if !parent.starts_with(&root) {
                continue;
            }
            let mut dir = Some(parent.as_path());
            let mut nested = Vec::new();
            while let Some(p) = dir.filter(|p| p.starts_with(&root)) {
                nested.push(p.to_path_buf());
                dir = p.parent();
            }
            nested.reverse();
            directories.extend(nested);
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut output = Vec::new();
    let mut remaining = MAX_BYTES;
    for directory in directories.into_iter().take(256) {
        if !seen.insert(directory.clone()) {
            continue;
        }
        for name in ["AGENTS.override.md", "AGENTS.md"] {
            let path = directory.join(name);
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            let Ok(scope) = directory.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&scope) {
                continue;
            }
            let Ok(file) = std::fs::File::open(&canonical) else {
                continue;
            };
            let mut bytes = Vec::new();
            if file
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .is_err()
            {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            if text.trim().is_empty() {
                continue;
            }
            let mut end = remaining.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut content = text[..end].to_string();
            if end < text.len() {
                content.push_str(
                    "\n[规约超出上下文预算，执行前必须用 read_file 分页读取此文件剩余内容]",
                );
            }
            remaining = remaining.saturating_sub(end);
            output.push(Instruction {
                path,
                scope: directory.clone(),
                content,
            });
            break;
        }
    }
    output
}

pub fn render(instructions: &[Instruction]) -> String {
    if instructions.is_empty() {
        return String::new();
    }
    let mut output = "\n\n# AGENTS.md 项目规约\n以下是显式规则，不是自动记忆。全局规则适用于所有项目，目录规则仅适用于该目录及其后代；同一目录 override 优先，较深目录优先。当前用户明确要求与工具权限边界优先。进入尚未列出的子目录前，先查找并读取该目录链上的 AGENTS.override.md / AGENTS.md。不得把自动经验擅自写入这些规则文件。\n".to_string();
    for instruction in instructions {
        output.push_str(&format!(
            "\n[规约 {}；作用域 {}]\n{}\n[规约结束]\n",
            instruction.path.display(),
            instruction.scope.display(),
            instruction.content
        ));
    }
    output
}

/// Per invocation, not a global cache: parallel delegates must each receive new rules.
#[derive(Default)]
pub struct Tracker(std::collections::BTreeSet<String>);
impl Tracker {
    pub fn seed(&mut self, instructions: &[Instruction]) {
        for rule in instructions {
            self.0
                .insert(format!("{}\n{}", rule.path.display(), rule.content));
        }
    }
    pub fn before_tool(
        &mut self,
        global: &Path,
        cwd: &Path,
        args: &serde_json::Value,
    ) -> Option<String> {
        let targets: Vec<_> = ["path", "file_path", "file", "target_path"]
            .iter()
            .filter_map(|key| {
                args.get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(PathBuf::from)
            })
            .collect();
        let rules = discover(global, cwd, &targets);
        let fresh: Vec<_> = rules
            .into_iter()
            .filter(|r| {
                self.0
                    .insert(format!("{}\n{}", r.path.display(), r.content))
            })
            .collect();
        (!fresh.is_empty()).then(|| format!("发现尚未提供或刚修改的项目规约。本次工具尚未执行；先应用下面规则，再重新调用工具。{}", render(&fresh)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layers_overrides_scopes_and_no_outside_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let global = dir.path().join("global");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&global).unwrap();
        for (p, text) in [
            (global.join("AGENTS.md"), "global"),
            (root.join("AGENTS.md"), "root"),
            (root.join("src/AGENTS.md"), "hidden"),
            (root.join("src/AGENTS.override.md"), "override"),
        ] {
            std::fs::write(p, text).unwrap();
        }
        let rules = discover(&global, &root.join("src/deep"), &[]);
        assert_eq!(
            rules.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(),
            ["global", "root", "override"]
        );
        let targeted = discover(
            &global,
            &root,
            &[
                PathBuf::from("src/deep/main.rs"),
                dir.path().join("elsewhere/a"),
            ],
        );
        assert_eq!(targeted.len(), 3);
        assert!(render(&targeted).contains("作用域"));
    }
    #[test]
    fn oversized_rules_are_bounded_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "约".repeat(MAX_BYTES)).unwrap();
        let rules = discover(dir.path(), Path::new(""), &[]);
        assert!(rules[0].content.len() < MAX_BYTES + 200);
        assert!(rules[0].content.contains("分页读取"));
    }
    #[test]
    fn traversal_targets_cannot_import_outside_rules() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("AGENTS.md"), "outside").unwrap();
        assert!(discover(
            &dir.path().join("global"),
            &repo,
            &[PathBuf::from("../outside/x.rs")]
        )
        .is_empty());
    }
    #[test]
    fn tracker_requires_rules_before_first_edit_and_after_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/AGENTS.md"), "nested v1").unwrap();
        let mut tracker = Tracker::default();
        let args = serde_json::json!({"path":"src/new.rs"});
        let global = dir.path().join("global");
        assert!(tracker
            .before_tool(&global, dir.path(), &args)
            .unwrap()
            .contains("nested v1"));
        assert!(tracker.before_tool(&global, dir.path(), &args).is_none());
        std::fs::write(dir.path().join("src/AGENTS.md"), "nested v2").unwrap();
        assert!(tracker
            .before_tool(&global, dir.path(), &args)
            .unwrap()
            .contains("nested v2"));
        // A second subagent has its own tracker, regardless of what the first one read.
        assert!(Tracker::default()
            .before_tool(&global, dir.path(), &args)
            .is_some());
    }
}
