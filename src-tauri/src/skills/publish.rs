//! 技能发布的宿主侧边界：有界写入、版本指纹与可回退快照。
//!
//! 这里是**发布纪律**，不是优化器：它保证一次自动写入的幅度有上界、每次覆盖前
//! 都留下一个可恢复的版本，并且候选/版本目录不进入技能加载与热重载扫描树。
//! 「新版本是否比旧版本更好」仍由后续独立评测决定，本模块不代替它做判断。
//!
//! 版本目录以 `.` 开头（`skills/.versions/`）：`load_from_dir` 与
//! `dir_fingerprint` 都跳过点前缀条目，因此快照永远不会被当成技能注册。
//! 本模块刻意不依赖 `super::`，可以单独编译成离线契约测试 crate。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// 技能正文上限：一次发布不得提交无限长的全文。
pub const MAX_BODY_CHARS: usize = 12_000;
/// 描述上限（与 `validate.rs` 的 200 字提示错开，硬上限更宽松但确实存在）。
pub const MAX_DESCRIPTION_CHARS: usize = 300;
pub const MAX_KEYWORDS: usize = 24;
pub const MAX_KEYWORD_CHARS: usize = 64;
pub const MAX_KEYWORDS_TOTAL_CHARS: usize = 512;
/// 未声明 `expected_base_hash` 时，一次覆盖允许新增+删除的字符总量。
pub const MAX_UPDATE_CHANGED_CHARS: usize = 2_000;
/// 每个技能保留的历史版本数。
pub const MAX_SNAPSHOTS: usize = 12;
/// 版本目录名，点前缀确保不进入加载与热重载扫描。
pub const VERSIONS_DIR: &str = ".versions";

/// 内容指纹，用作 `base_revision` / `revision`。
pub fn revision(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))[..16].to_string()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// 版本目录里只允许读回 `references/` 或 `scripts/` 下的一层文件。
/// 快照内容是本地文件，但清单仍可能被手工改动，回退时不能让它指向目录外。
fn safe_resource_rel(path: &str) -> bool {
    let mut parts = path.split('/');
    let root = parts.next().unwrap_or("");
    if root != "references" && root != "scripts" {
        return false;
    }
    let Some(file) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    !file.is_empty()
        && file != "."
        && file != ".."
        && !file.starts_with('.')
        && !file.contains('\\')
        && !file.contains(':')
}

/// 名称必须能安全用作单个路径分量。宿主另有一层 `is_valid_skill_name`，
/// 这里再做一次，避免快照路径被 `..` 或分隔符逃逸。
pub fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.starts_with('.')
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && value.chars().count() <= 64
}

/// 一次写入的形状与体量预算。描述、关键词变化也算改动，不能藏在正文以外。
pub fn validate_budget(description: &str, body: &str, keywords: &[String]) -> Result<(), String> {
    let description_chars = description.chars().count();
    if description_chars > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "description 有 {description_chars} 字符，超过上限 {MAX_DESCRIPTION_CHARS}"
        ));
    }
    let body_chars = body.chars().count();
    if body_chars > MAX_BODY_CHARS {
        return Err(format!("技能正文有 {body_chars} 字符，超过上限 {MAX_BODY_CHARS}"));
    }
    if keywords.len() > MAX_KEYWORDS {
        return Err(format!("keywords 有 {} 项，超过上限 {MAX_KEYWORDS}", keywords.len()));
    }
    let total: usize = keywords.iter().map(|k| k.chars().count()).sum();
    if total > MAX_KEYWORDS_TOTAL_CHARS {
        return Err(format!(
            "keywords 合计 {total} 字符，超过上限 {MAX_KEYWORDS_TOTAL_CHARS}"
        ));
    }
    if let Some(long) = keywords.iter().find(|k| k.chars().count() > MAX_KEYWORD_CHARS) {
        return Err(format!("关键词「{long}」超过 {MAX_KEYWORD_CHARS} 字符"));
    }
    Ok(())
}

/// 按行的顺序敏感改动幅度：被删除行 + 被新增行的字符数（各含换行）。
///
/// 用 LCS 定位未改动的行，再统计两侧剩余行；纯重排也会计为非零。
/// 行对数过大时直接返回 `usize::MAX`，交由调用方按超预算拒绝。
pub fn changed_chars(old: &str, new: &str) -> usize {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // O(n*m) 的 DP 必须有上界，避免超长技能把发布路径拖死。
    if a.len().saturating_mul(b.len()) > 1_000_000 {
        return usize::MAX;
    }
    let mut dp = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    let mut kept_a = vec![false; a.len()];
    let mut kept_b = vec![false; b.len()];
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            kept_a[i - 1] = true;
            kept_b[j - 1] = true;
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    let size = |line: &&str| line.chars().count().saturating_add(1);
    let deleted: usize = a
        .iter()
        .enumerate()
        .filter(|(k, _)| !kept_a[*k])
        .map(|(_, line)| size(line))
        .sum();
    let inserted: usize = b
        .iter()
        .enumerate()
        .filter(|(k, _)| !kept_b[*k])
        .map(|(_, line)| size(line))
        .sum();
    deleted.saturating_add(inserted)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePlan {
    /// 被覆盖版本的指纹；新建技能为 `None`。
    pub base_revision: Option<String>,
    pub changed_chars: usize,
    /// 调用方是否用 `expected_base_hash` 显式声明整版替换。
    pub explicit_base: bool,
}

/// 覆盖前的版本检查。
///
/// - 新建技能：不允许带 `expected_base_hash`。
/// - 已存在且未声明 base：改动幅度必须 ≤ `MAX_UPDATE_CHANGED_CHARS`。
/// - 已存在且声明 base：指纹必须匹配当前内容，才允许整版替换。
///
/// 这给「同名全文覆盖」加了一道显式的、可核验的意图声明，但**不**证明新版本更好。
pub fn check_update(
    old_content: Option<&str>,
    new_content: &str,
    expected_base: Option<&str>,
) -> Result<UpdatePlan, String> {
    let expected = expected_base.map(str::trim).filter(|s| !s.is_empty());
    let Some(old) = old_content else {
        if expected.is_some() {
            return Err("新建技能不需要 expected_base_hash".into());
        }
        return Ok(UpdatePlan {
            base_revision: None,
            changed_chars: 0,
            explicit_base: false,
        });
    };
    let base = revision(old);
    let changed = changed_chars(old, new_content);
    if let Some(expected) = expected {
        if expected != base {
            return Err(format!(
                "base 已变化（当前 {base}），请重新读取技能后再提交"
            ));
        }
        return Ok(UpdatePlan {
            base_revision: Some(base),
            changed_chars: changed,
            explicit_base: true,
        });
    }
    if changed > MAX_UPDATE_CHANGED_CHARS {
        return Err(format!(
            "本次改动 {changed} 字符，超过单步上限 {MAX_UPDATE_CHANGED_CHARS}；\
             请拆成小步，或显式传入 expected_base_hash=\"{base}\" 声明整版替换"
        ));
    }
    Ok(UpdatePlan {
        base_revision: Some(base),
        changed_chars: changed,
        explicit_base: false,
    })
}

pub fn versions_root(skills_dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !safe_component(name) {
        return Err("技能名不能用作版本目录".into());
    }
    Ok(skills_dir.join(VERSIONS_DIR).join(name))
}

/// 技能目录内允许的支撑资源（`references/` 与 `scripts/` 下的一层文件）。
fn collect_resources(base_dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for root in ["references", "scripts"] {
        let dir = base_dir.join(root);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                out.push((format!("{root}/{file_name}"), text));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub revision: String,
    pub layout: String,
    pub origin: String,
    pub created_at_ms: u64,
    pub changed_from: Option<String>,
}

/// 覆盖前保存不可变版本快照，返回其指纹；内容未变化时不产生新快照。
///
/// 快照写在点前缀目录，加载器与热重载指纹都会跳过，不会被注册成技能，
/// 也不会因为写文件而触发一次「半截版本」的自动重载。
pub fn save_snapshot(
    skills_dir: &Path,
    name: &str,
    skill_file: &Path,
    layout: &str,
    origin: &str,
    changed_from: Option<&str>,
) -> Result<Option<String>, String> {
    if !skill_file.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(skill_file).map_err(|e| format!("读取现行技能失败: {e}"))?;
    let rev = revision(&content);
    let root = versions_root(skills_dir, name)?;
    if list_snapshots(skills_dir, name)?
        .iter()
        .any(|s| s.revision == rev)
    {
        return Ok(Some(rev));
    }
    let stamp = now_millis();
    let dir = root.join(format!("{stamp}-{rev}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建版本目录失败: {e}"))?;
    std::fs::write(dir.join("SKILL.md"), &content)
        .map_err(|e| format!("保存版本正文失败: {e}"))?;
    let resources: Vec<(String, String)> = if layout == "folder" {
        skill_file
            .parent()
            .map(collect_resources)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for (relative, text) in &resources {
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建版本资源目录失败: {e}"))?;
        }
        std::fs::write(target, text).map_err(|e| format!("保存版本资源失败: {e}"))?;
    }
    let manifest = json!({
        "revision": rev,
        "layout": layout,
        "origin": origin,
        "created_at_ms": stamp,
        "changed_from": changed_from,
        "resources": resources.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("保存版本清单失败: {e}"))?;
    prune_snapshots(&root)?;
    Ok(Some(rev))
}

fn prune_snapshots(root: &Path) -> Result<(), String> {
    let mut dirs: Vec<(String, PathBuf)> = read_snapshot_dirs(root)?
        .into_iter()
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, dir) in dirs.iter().skip(MAX_SNAPSHOTS) {
        // 只清理本模块在点前缀目录下创建的版本目录。
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

fn read_snapshot_dirs(root: &Path) -> Result<HashMap<String, PathBuf>, String> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        if dir_name.starts_with('.') || !dir_name.contains('-') {
            continue;
        }
        out.insert(dir_name, entry.path());
    }
    Ok(out)
}

fn parse_manifest(dir: &Path, fallback_rev: &str) -> SnapshotInfo {
    let raw = std::fs::read_to_string(dir.join("manifest.json")).unwrap_or_default();
    let value: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    SnapshotInfo {
        revision: value
            .get("revision")
            .and_then(Value::as_str)
            .unwrap_or(fallback_rev)
            .to_string(),
        layout: value
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or("folder")
            .to_string(),
        origin: value
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string(),
        created_at_ms: value
            .get("created_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        changed_from: value
            .get("changed_from")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// 列出可用版本，按时间倒序。
pub fn list_snapshots(skills_dir: &Path, name: &str) -> Result<Vec<SnapshotInfo>, String> {
    let root = versions_root(skills_dir, name)?;
    let mut items: Vec<(String, SnapshotInfo)> = read_snapshot_dirs(&root)?
        .into_iter()
        .map(|(dir_name, path)| {
            let fallback = dir_name.rsplit('-').next().unwrap_or("").to_string();
            (dir_name.clone(), parse_manifest(&path, &fallback))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(items.into_iter().map(|(_, info)| info).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredVersion {
    pub revision: String,
    pub content: String,
    pub layout: String,
    pub resources: Vec<(String, String)>,
}

/// 回退目标计算结果。抽成纯函数，便于在隔离契约 runner 里实测布局冲突。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreTarget {
    /// 要写回的 SKILL.md 路径。
    pub file: PathBuf,
    /// 归一后的目标布局（清单里的 layout 可能被手工改动）。
    pub layout: &'static str,
    /// 磁盘上**真实存在**的现行版本，回退前必须给它留快照。
    pub current: Option<(PathBuf, &'static str)>,
}

/// 决定回退写哪个文件、给哪个现行文件留快照。
///
/// 两个陷阱：
/// - 按目标布局推路径去存现行快照，跨布局时该文件不存在，`save_snapshot` 会返回
///   `Ok(None)`，现行版本被静默丢掉；
/// - 跨布局回退会留下 flat + folder 同名双份，磁盘上长期残留隐性旧版本。
pub fn restore_target(
    skills_dir: &Path,
    name: &str,
    layout: &str,
) -> Result<RestoreTarget, String> {
    if !safe_component(name) {
        return Err("技能名不合法".into());
    }
    let flat_file = skills_dir.join(format!("{name}.md"));
    let folder_file = skills_dir.join(name).join("SKILL.md");
    let target_layout: &'static str = if layout == "flat" { "flat" } else { "folder" };
    if target_layout == "flat" && folder_file.is_file() {
        return Err("目标版本是单文件布局，但当前磁盘上存在同名目录版技能，回退会造成双份；请先清理目录版".into());
    }
    if target_layout == "folder" && flat_file.is_file() {
        return Err("目标版本是目录布局，但当前磁盘上存在同名单文件技能，回退会造成双份；请先清理单文件".into());
    }
    let current = if flat_file.is_file() {
        Some((flat_file.clone(), "flat"))
    } else if folder_file.is_file() {
        Some((folder_file.clone(), "folder"))
    } else {
        None
    };
    Ok(RestoreTarget {
        file: if target_layout == "flat" {
            flat_file
        } else {
            folder_file
        },
        layout: target_layout,
        current,
    })
}

/// 读取一个历史版本的完整内容，由调用方原子写回并重新注册。
pub fn restore_snapshot(
    skills_dir: &Path,
    name: &str,
    rev: &str,
) -> Result<RestoredVersion, String> {
    let rev = rev.trim();
    if rev.is_empty() || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("revision 必须是版本指纹".into());
    }
    let root = versions_root(skills_dir, name)?;
    let dirs = read_snapshot_dirs(&root)?;
    let matched = dirs
        .into_iter()
        .find(|(dir_name, _)| dir_name.rsplit('-').next() == Some(rev))
        .map(|(_, path)| path)
        .ok_or_else(|| format!("找不到版本 {rev}"))?;
    let info = parse_manifest(&matched, rev);
    let content = std::fs::read_to_string(matched.join("SKILL.md"))
        .map_err(|e| format!("读取版本正文失败: {e}"))?;
    let resources: Vec<String> = std::fs::read_to_string(matched.join("manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| {
            v.get("resources")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
        })
        .unwrap_or_default();
    let mut files = Vec::new();
    for relative in resources {
        if !safe_resource_rel(&relative) {
            continue;
        }
        let path = matched.join(&relative);
        if path.is_file() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                files.push((relative, text));
            }
        }
    }
    Ok(RestoredVersion {
        revision: info.revision,
        content,
        layout: info.layout,
        resources: files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords(n: usize, width: usize) -> Vec<String> {
        (0..n).map(|i| "k".repeat(width) + &i.to_string()).collect()
    }

    #[test]
    fn budget_accepts_normal_skill_and_rejects_each_bound() {
        assert!(validate_budget("一句话", "按步骤做", &["召回".into()]).is_ok());
        assert!(validate_budget("d", &"字".repeat(MAX_BODY_CHARS + 1), &[]).is_err());
        assert!(validate_budget(&"d".repeat(MAX_DESCRIPTION_CHARS + 1), "正文", &[]).is_err());
        assert!(validate_budget("d", "正文", &keywords(MAX_KEYWORDS + 1, 3)).is_err());
        assert!(validate_budget("d", "正文", &vec!["k".repeat(MAX_KEYWORD_CHARS + 1)]).is_err());
        assert!(validate_budget("d", "正文", &keywords(4, 200)).is_err());
    }

    #[test]
    fn changed_chars_counts_insertions_deletions_and_reordering() {
        assert_eq!(changed_chars("a\nb\nc\n", "a\nb\nc\n"), 0);
        let appended = changed_chars("a\nb\n", "a\nb\nc\n");
        assert!(appended > 0 && appended < 10, "{appended}");
        assert!(changed_chars("a\nb\nc\n", "c\nb\na\n") > 0, "纯重排也必须计为改动");
        assert_eq!(
            changed_chars(&"x\n".repeat(1100), &"y\n".repeat(1100)),
            usize::MAX,
            "DP 上界必须生效"
        );
    }

    #[test]
    fn new_skill_rejects_an_unexpected_base() {
        assert!(check_update(None, "---\nname: a\n---\n\n正文\n", Some("deadbeef")).is_err());
        let plan = check_update(None, "---\nname: a\n---\n\n正文\n", None).unwrap();
        assert_eq!(plan.base_revision, None);
    }

    #[test]
    fn small_update_passes_but_whole_rewrite_needs_explicit_base() {
        let old = "---\nname: a\ndescription: d\n---\n\n第一步\n第二步\n";
        let small = "---\nname: a\ndescription: d\n---\n\n第一步\n第二步\n第三步\n";
        let plan = check_update(Some(old), small, None).unwrap();
        assert!(plan.base_revision.is_some());
        assert!(plan.changed_chars > 0 && !plan.explicit_base);

        let whole = format!("---\nname: a\ndescription: d\n---\n\n{}\n", "新写法".repeat(1200));
        assert!(whole.chars().count() < MAX_BODY_CHARS, "夹具必须只越过改动预算");
        assert!(
            changed_chars(old, &whole) > MAX_UPDATE_CHANGED_CHARS,
            "夹具本身必须越过单步改动上限，否则这个用例什么也没测"
        );
        let err = check_update(Some(old), &whole, None).unwrap_err();
        assert!(err.contains("expected_base_hash"), "{err}");

        let base = revision(old);
        assert!(check_update(Some(old), &whole, Some("deadbeef")).is_err());
        let allowed = check_update(Some(old), &whole, Some(&base)).unwrap();
        assert!(allowed.explicit_base);
        assert_eq!(allowed.base_revision.as_deref(), Some(base.as_str()));
    }

    #[test]
    fn snapshots_are_hidden_versioned_and_restorable() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        let skill_dir = skills.join("demo");
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        let file = skill_dir.join("SKILL.md");
        let v1 = "---\nname: demo\ndescription: 旧版\n---\n\n旧步骤\n";
        std::fs::write(&file, v1).unwrap();
        std::fs::write(skill_dir.join("references/n.md"), "旧资料").unwrap();

        let rev1 = save_snapshot(skills, "demo", &file, "folder", "auto", None)
            .unwrap()
            .unwrap();
        assert_eq!(rev1, revision(v1));
        assert!(skills.join(VERSIONS_DIR).join("demo").is_dir());
        assert!(list_snapshots(skills, "demo").unwrap()[0].revision == rev1);

        // 同名快照不重复占用历史。
        assert_eq!(
            save_snapshot(skills, "demo", &file, "folder", "auto", None).unwrap(),
            Some(rev1.clone())
        );
        assert_eq!(list_snapshots(skills, "demo").unwrap().len(), 1);

        let restored = restore_snapshot(skills, "demo", &rev1).unwrap();
        assert_eq!(restored.content, v1);
        assert_eq!(restored.layout, "folder");
        assert_eq!(
            restored.resources,
            vec![("references/n.md".to_string(), "旧资料".to_string())]
        );
        assert!(restore_snapshot(skills, "demo", "not-a-hex").is_err());
        assert!(restore_snapshot(skills, "demo", "0000000000000000").is_err());

        let v2 = "---\nname: demo\ndescription: 新版\n---\n\n新步骤\n";
        std::fs::write(&file, v2).unwrap();
        let _rev2 = save_snapshot(skills, "demo", &file, "folder", "manual", Some(&rev1))
            .unwrap()
            .unwrap();
        let all = list_snapshots(skills, "demo").unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.origin == "manual"));
    }

    #[test]
    fn snapshot_names_cannot_escape_the_versions_directory() {
        for bad in ["../evil", "a/b", ".hidden", "", ".."] {
            assert!(!safe_component(bad), "{bad}");
        }
        assert!(safe_component("正常技能"));
        for bad in [
            "../../etc/passwd",
            "references/../../x",
            "references/a/b.md",
            "/absolute",
            "scripts/../x",
            "assets/a.png",
        ] {
            assert!(!safe_resource_rel(bad), "{bad}");
        }
        assert!(safe_resource_rel("references/notes.md"));
        assert!(safe_resource_rel("scripts/run.py"));
    }

    #[test]
    fn restore_ignores_a_tampered_manifest_resource_path() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        let skill_dir = skills.join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let file = skill_dir.join("SKILL.md");
        let content = "---\nname: demo\ndescription: d\n---\n\n正文\n";
        std::fs::write(&file, content).unwrap();
        let rev = save_snapshot(skills, "demo", &file, "folder", "auto", None)
            .unwrap()
            .unwrap();
        let versions = list_snapshots(skills, "demo").unwrap();
        let root = versions_root(skills, "demo").unwrap();
        let entry = read_snapshot_dirs(&root)
            .unwrap()
            .into_iter()
            .find(|(name, _)| name.ends_with(&rev))
            .unwrap()
            .1;
        std::fs::write(
            entry.join("manifest.json"),
            json!({
                "revision": versions[0].revision,
                "layout": "folder",
                "origin": "auto",
                "created_at_ms": 1,
                "resources": ["../../etc/passwd"],
            })
            .to_string(),
        )
        .unwrap();
        let restored = restore_snapshot(skills, "demo", &rev).unwrap();
        assert_eq!(restored.content, content);
        assert!(restored.resources.is_empty(), "越界资源不得被读回");
    }

    #[test]
    fn restore_target_pins_the_real_current_file_and_rejects_cross_layout() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();

        // 现行是目录版，目标也是目录版：快照必须指向磁盘上真实存在的那个文件。
        std::fs::create_dir_all(skills.join("demo")).unwrap();
        let folder_file = skills.join("demo/SKILL.md");
        std::fs::write(&folder_file, "current").unwrap();
        let target = restore_target(skills, "demo", "folder").unwrap();
        assert_eq!(target.layout, "folder");
        assert_eq!(target.file, folder_file);
        assert_eq!(target.current, Some((folder_file.clone(), "folder")));

        // 跨布局回退必须拒绝，否则磁盘上会残留同名双份。
        assert!(restore_target(skills, "demo", "flat")
            .unwrap_err()
            .contains("双份"));

        std::fs::remove_file(&folder_file).unwrap();
        let flat_file = skills.join("demo.md");
        std::fs::write(&flat_file, "current").unwrap();
        assert!(restore_target(skills, "demo", "folder").is_err());
        let flat_target = restore_target(skills, "demo", "flat").unwrap();
        assert_eq!(flat_target.current, Some((flat_file.clone(), "flat")));
        assert_eq!(flat_target.file, flat_file);

        // 没有现行版本也要能回退，只是无需再留快照。
        std::fs::remove_file(&flat_file).unwrap();
        let fresh = restore_target(skills, "demo", "folder").unwrap();
        assert_eq!(fresh.current, None);
        assert_eq!(fresh.file, skills.join("demo/SKILL.md"));

        // 清单里的未知 layout 归一到 folder，不会落到第三种路径。
        assert_eq!(restore_target(skills, "demo", "weird").unwrap().layout, "folder");
        assert!(restore_target(skills, "../evil", "folder").is_err());
    }
}
