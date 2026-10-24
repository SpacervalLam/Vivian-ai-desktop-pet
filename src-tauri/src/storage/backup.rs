//! Portable backup codec, live database snapshots, and startup restore operations.
//! Commands handle application lifecycle; this module only accepts explicit paths.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 备份归档扩展名（自定义格式标识，仅文件名后缀）
pub(crate) const BACKUP_EXT: &str = "altn";
/// 归档魔数（解压后有效负载的开头 8 字节）
const ARCHIVE_MAGIC: &[u8; 8] = b"ALTNBAK1";
/// 恢复待执行标记文件名（写入用户数据目录根，重启后消费）
pub(crate) const RESTORE_MARKER: &str = ".restore_pending";
/// 备份文件名前缀
pub(crate) const BACKUP_DIR_PREFIX: &str = "vivian_backup_";
/// 恢复暂存目录名（用户数据目录根下，解压成功后才正式搬入）
pub(crate) const STAGING_DIR: &str = ".restore_staging";

/// 归档清单：目录与文件的相对路径列表（`/` 分隔）+ 文件大小
#[derive(Serialize, Deserialize)]
pub(crate) struct ArchiveManifest {
    version: u32,
    created_at: String,
    dirs: Vec<String>,
    files: Vec<ArchiveFileEntry>,
}

#[derive(Serialize, Deserialize)]
struct ArchiveFileEntry {
    path: String,
    size: u64,
}

/// 判断 `child` 是否位于 `parent` 目录内（含相等），用于防止备份/恢复源
/// 落在用户数据目录内部（恢复清扫会删除它，造成数据自毁）
pub(crate) fn is_path_inside(child: &Path, parent: &Path) -> bool {
    let norm = |p: &Path| -> PathBuf {
        // canonicalize 失败（目录不存在等）时退回绝对化，尽力比较
        p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
    };
    let c = norm(child);
    let p = norm(parent);
    c == p || c.starts_with(&p)
}

/// 备份时用户数据目录顶层需要跳过的条目。
///
/// 分层契约见 [`super::policy`]：派生层（可重建）与运行期层不收录，权威层与
/// 未知层保留；标记与暂存目录同属运行期层。
fn top_level_backup_skip(name: &str) -> bool {
    super::policy::backup_skip_top_level(name)
}

/// 校验归档内相对路径：拒绝绝对路径、盘符、反斜杠与 `.`/`..`/空组件（防路径穿越）
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains('\\')
        && !rel.contains(':')
        && rel
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}

/// 递归收集 `dir` 下全部子目录与文件（不含 `dir` 本身），`prefix` 为其相对路径
fn collect_walk(
    dir: &Path,
    prefix: &str,
    dirs: &mut Vec<String>,
    files: &mut Vec<(String, u64)>,
) -> Result<(), String> {
    for entry in
        std::fs::read_dir(dir).map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| format!("遍历 {} 失败: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if super::policy::browser_cache(Path::new(&rel)) {
            continue;
        }
        if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            return Err(format!("备份不支持符号链接: {}", path.display()));
        }
        if path.is_dir() {
            dirs.push(rel.clone());
            collect_walk(&path, &rel, dirs, files)?;
        } else {
            let size = entry
                .metadata()
                .map_err(|e| format!("读取元数据 {} 失败: {e}", path.display()))?
                .len();
            files.push((rel, size));
        }
    }
    Ok(())
}

/// Private, short-lived copies of live databases. Only sidecars belonging to a
/// successfully snapshotted database are omitted; unrelated files are preserved.
struct DatabaseSnapshots {
    dir: PathBuf,
    sources: BTreeMap<String, PathBuf>,
}

impl Drop for DatabaseSnapshots {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.dir) {
            tracing::warn!(
                "[backup] 清理数据库快照暂存目录 {} 失败: {error}",
                self.dir.display()
            );
        }
    }
}

impl DatabaseSnapshots {
    fn prepare(root: &Path, dest: &Path, files: &mut Vec<(String, u64)>) -> Result<Self, String> {
        let dir = dest.with_file_name(format!(".vivian-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).map_err(|e| format!("创建数据库备份暂存目录失败: {e}"))?;
        let mut snapshots = Self {
            dir,
            sources: BTreeMap::new(),
        };
        for (rel, size) in files.iter_mut() {
            // Sorted input puts each database before its sidecars. The source
            // connection may remove sidecars on close, so do not open them again.
            if ["-wal", "-shm", "-journal"].iter().any(|suffix| {
                rel.strip_suffix(suffix)
                    .is_some_and(|base| snapshots.sources.contains_key(base))
            }) {
                continue;
            }
            let path = root.join(&*rel);
            let mut file = std::fs::File::open(&path)
                .map_err(|e| format!("读取备份文件 {} 失败: {e}", path.display()))?;
            let mut magic = [0u8; 16];
            if *size < magic.len() as u64 {
                continue;
            }
            file.read_exact(&mut magic)
                .map_err(|e| format!("读取备份文件头 {} 失败: {e}", path.display()))?;
            if &magic != b"SQLite format 3\0" {
                continue;
            }
            drop(file);
            let target = snapshots
                .dir
                .join(format!("{}.db", snapshots.sources.len()));
            crate::utils::sqlite::snapshot(&path, &target)
                .map_err(|e| format!("数据库 {} 快照失败，备份已取消: {e}", rel))?;
            *size = std::fs::metadata(&target).map_err(|e| e.to_string())?.len();
            snapshots.sources.insert(rel.clone(), target);
        }
        let sidecars: BTreeSet<String> = snapshots
            .sources
            .keys()
            .flat_map(|rel| {
                [
                    format!("{rel}-wal"),
                    format!("{rel}-shm"),
                    format!("{rel}-journal"),
                ]
            })
            .collect();
        files.retain(|(rel, _)| !sidecars.contains(rel));
        Ok(snapshots)
    }
}

/// 打包：用户数据目录 → `.altn` 归档（zstd 流式压缩），返回文件数
fn inventory(root: &Path) -> Result<(Vec<String>, Vec<(String, u64)>), String> {
    // 1. 收集清单（顶层应用排除规则，其下全量）
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| format!("读取用户数据目录失败: {e}"))?
    {
        let entry = entry.map_err(|e| format!("遍历用户数据目录失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if top_level_backup_skip(&name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            return Err(format!("备份不支持符号链接: {}", path.display()));
        }
        if path.is_dir() {
            dirs.push(name.clone());
            collect_walk(&path, &name, &mut dirs, &mut files)?;
        } else {
            let size = entry
                .metadata()
                .map_err(|e| format!("读取元数据 {} 失败: {e}", path.display()))?
                .len();
            files.push((name, size));
        }
    }
    dirs.sort();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    Ok((dirs, files))
}

pub(crate) fn preflight(root: &Path, dest: &Path) -> Result<(), String> {
    let (_, files) = inventory(root)?;
    let required = files
        .iter()
        .try_fold(64 * 1024 * 1024u64, |sum, (_, size)| {
            sum.checked_add(size.saturating_mul(2))
                .ok_or("备份空间估算溢出")
        })?;
    require_space(dest.parent().ok_or("备份目标缺少父目录")?, required)?;

    Ok(())
}

pub(crate) fn write_archive(root: &Path, dest: &Path) -> Result<u64, String> {
    if dest.exists() {
        return Err("备份目标已存在，未覆盖".into());
    }
    if is_path_inside(dest, root) {
        return Err("备份目标不能位于数据目录内".into());
    }
    let (dirs, mut files) = inventory(root)?;
    let required = files
        .iter()
        .try_fold(64 * 1024 * 1024u64, |sum, (_, size)| {
            sum.checked_add(size.saturating_mul(2))
                .ok_or("备份空间估算溢出")
        })?;
    require_space(dest.parent().ok_or("备份目标缺少父目录")?, required)?;

    // Snapshot before recording manifest lengths. A live DB and its WAL can
    // change independently while the archive streams; copying their raw bytes
    // in sequence cannot produce a coherent database backup.
    let snapshots = DatabaseSnapshots::prepare(root, dest, &mut files)?;

    let manifest = ArchiveManifest {
        version: 1,
        created_at: chrono::Local::now().to_rfc3339(),
        files: files
            .iter()
            .map(|(p, s)| ArchiveFileEntry {
                path: p.clone(),
                size: *s,
            })
            .collect(),
        dirs,
    };
    let header = serde_json::to_vec(&manifest).map_err(|e| format!("序列化清单失败: {e}"))?;

    // 2. 流式写入：magic + 清单 + 文件内容，整体经 zstd 压缩
    let partial =
        PartialArchive(dest.with_file_name(format!(".backup-{}.part", uuid::Uuid::new_v4())));
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)
        .map_err(|e| format!("创建备份文件失败: {e}"))?;
    let mut writer = std::io::BufWriter::new(out);
    let mut enc =
        zstd::stream::Encoder::new(&mut writer, 3).map_err(|e| format!("初始化压缩器失败: {e}"))?;
    enc.write_all(ARCHIVE_MAGIC)
        .map_err(|e| format!("写入备份失败: {e}"))?;
    enc.write_all(&(header.len() as u32).to_le_bytes())
        .map_err(|e| format!("写入备份失败: {e}"))?;
    enc.write_all(&header)
        .map_err(|e| format!("写入备份失败: {e}"))?;
    let mut count = 0u64;
    for (rel, size) in &files {
        // 清单路径为 '/' 分隔；Windows 文件系统 API 同样接受 '/' 分隔符
        let src = snapshots
            .sources
            .get(rel)
            .cloned()
            .unwrap_or_else(|| root.join(rel));
        let mut f =
            std::fs::File::open(&src).map_err(|e| format!("打开 {} 失败: {e}", src.display()))?;
        // 文件可能在打包期间继续追加；严格遵守清单长度，避免后续文件错位。
        let copied = std::io::copy(&mut (&mut f).take(*size), &mut enc)
            .map_err(|e| format!("写入 {} 失败: {e}", src.display()))?;
        if copied != *size {
            return Err(format!("备份期间文件被截断: {}，请重试备份", src.display()));
        }
        count += 1;
    }
    enc.finish().map_err(|e| format!("完成压缩失败: {e}"))?;
    writer.flush().map_err(|e| format!("写入备份失败: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("同步备份文件失败: {e}"))?;
    drop(writer);
    crate::utils::fs::publish_atomic_file(&partial.0, dest)
        .map_err(|e| format!("发布备份失败: {e}"))?;
    Ok(count)
}

struct PartialArchive(PathBuf);
impl Drop for PartialArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn require_space(path: &Path, required: u64) -> Result<(), String> {
    let available = fs2::available_space(path).map_err(|e| format!("无法读取目标磁盘空间: {e}"))?;
    check_space(available, required)
}

fn check_space(available: u64, required: u64) -> Result<(), String> {
    if available < required {
        return Err(format!(
            "磁盘空间不足：至少需要 {} MiB（含快照和归档），可用 {} MiB",
            required.div_ceil(1024 * 1024),
            available / (1024 * 1024)
        ));
    }
    Ok(())
}

/// 打开归档并读取头部（magic + 清单），返回（清单, 已消费的头部字节数, 解码器）
///
/// 返回的解码器已定位到第一个文件内容的起始位置，可直接按清单顺序读取。
fn open_archive(
    archive: &Path,
) -> Result<
    (
        ArchiveManifest,
        usize,
        zstd::stream::Decoder<'static, std::io::BufReader<std::fs::File>>,
    ),
    String,
> {
    let f = std::fs::File::open(archive).map_err(|e| format!("打开备份文件失败: {e}"))?;
    let mut dec = zstd::stream::Decoder::new(f)
        .map_err(|e| format!("备份文件解压失败（可能已损坏）: {e}"))?;
    let mut magic = [0u8; 8];
    dec.read_exact(&mut magic)
        .map_err(|e| format!("读取备份失败: {e}"))?;
    if &magic != ARCHIVE_MAGIC {
        return Err("不是有效的 .altn 备份文件（格式标识不匹配）".to_string());
    }
    let mut len_buf = [0u8; 4];
    dec.read_exact(&mut len_buf)
        .map_err(|e| format!("读取备份失败: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 32 * 1024 * 1024 {
        return Err("备份清单长度非法（可能已损坏）".to_string());
    }
    let mut buf = vec![0u8; len];
    dec.read_exact(&mut buf)
        .map_err(|e| format!("读取备份失败: {e}"))?;
    let manifest: ArchiveManifest =
        serde_json::from_slice(&buf).map_err(|e| format!("备份清单解析失败: {e}"))?;
    Ok((manifest, 8 + 4 + len, dec))
}

/// 读取归档清单（仅解出 magic + manifest，不读文件内容），用于恢复前快速校验
pub(crate) fn read_manifest(archive: &Path) -> Result<ArchiveManifest, String> {
    Ok(open_archive(archive)?.0)
}

/// 解包：`.altn` 归档 → 目标目录，返回文件数
///
/// 写盘前先全量校验清单路径（防恶意归档路径穿越），任何文件内容不完整即报错。
fn extract_archive(archive: &Path, dest: &Path) -> Result<u64, String> {
    let (manifest, _header_len, mut dec) = open_archive(archive)?;
    for d in &manifest.dirs {
        if !safe_rel(d) {
            return Err(format!("备份内含非法路径: {d}"));
        }
    }
    for f in &manifest.files {
        if !safe_rel(&f.path) {
            return Err(format!("备份内含非法路径: {}", f.path));
        }
    }

    let required = manifest
        .files
        .iter()
        .try_fold(64 * 1024 * 1024u64, |sum, file| {
            sum.checked_add(file.size).ok_or("恢复空间估算溢出")
        })?;
    let existing = dest
        .ancestors()
        .find(|p| p.exists())
        .ok_or("恢复目录没有可用父目录")?;
    require_space(existing, required)?;

    for d in &manifest.dirs {
        std::fs::create_dir_all(dest.join(d)).map_err(|e| format!("创建目录 {d} 失败: {e}"))?;
    }
    let mut count = 0u64;
    for entry in &manifest.files {
        let target = dest.join(&entry.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        let out = std::fs::File::create(&target)
            .map_err(|e| format!("创建文件 {} 失败: {e}", target.display()))?;
        let mut out = std::io::BufWriter::new(out);
        let mut take = dec.by_ref().take(entry.size);
        let n = std::io::copy(&mut take, &mut out)
            .map_err(|e| format!("解压 {} 失败: {e}", entry.path))?;
        if n != entry.size {
            return Err(format!(
                "备份内容不完整: {}（期望 {} 字节，实际 {}）",
                entry.path, entry.size, n
            ));
        }
        out.flush()
            .map_err(|e| format!("写入 {} 失败: {e}", entry.path))?;
        count += 1;
    }
    Ok(count)
}

/// 校验并解析用户选择的备份源（`.altn` 文件 / 旧版备份文件夹）：
/// 1. 是文件 → `.altn` 归档（内容校验由调用方 read_manifest 完成）
/// 2. 是目录且有备份特征（`characters/`、`config.yaml` 或旧版标记）→ 文件夹备份
/// 3. 目录下恰有一个 `.altn` 文件 → 自动下钻（用户选了归档所在文件夹）
/// 4. 目录下恰有一个旧版 `vivian_backup_*` 子目录 → 自动下钻
pub(crate) fn resolve_backup_source(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(format!("所选路径不存在: {}", path.display()));
    }
    // 旧版文件夹备份特征
    let has_marker_or_data = path.join("characters").is_dir() || path.join("config.yaml").is_file();
    if has_marker_or_data {
        return Ok(path.to_path_buf());
    }
    // 唯一 .altn 文件（选了归档所在的外层文件夹）
    let mut altns: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| format!("读取目录失败: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|x| x.eq_ignore_ascii_case(BACKUP_EXT))
                    .unwrap_or(false)
        })
        .collect();
    if altns.len() == 1 {
        return Ok(altns.remove(0));
    }
    // 唯一 vivian_backup_* 子目录（旧版备份）
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| format!("读取目录失败: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(BACKUP_DIR_PREFIX))
                    .unwrap_or(false)
        })
        .collect();
    if candidates.len() == 1 {
        return Ok(candidates.remove(0));
    }
    Err("所选位置不是有效的备份。请选择“备份数据”生成的 .altn 备份文件（或旧版 vivian_backup_* 备份文件夹）".to_string())
}

/// 清空用户数据目录（保留运行时基础设施、标记与暂存目录），返回删除条目数
fn clear_root(root: &Path) -> usize {
    let mut removed = 0usize;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if super::policy::backup_skip_top_level(&name) {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("[restore] 删除 {} 失败（继续）: {e}", name),
        }
    }
    removed
}

/// 恢复主体：按备份源类型分流（归档文件 / 旧版文件夹）
pub(crate) fn run_restore(root: &Path, source: &Path) -> Result<(), String> {
    if source.is_file() {
        run_restore_from_archive(root, source)
    } else {
        run_restore_from_dir(root, source)
    }
}

/// 归档恢复：先解压到暂存目录，成功后才清空现有数据并搬入
///
/// 暂存式两阶段提交：解压失败时现有数据完好无损（仅删暂存目录），
/// 避免半途失败留下「旧数据已删、新数据不全」的中间态。
fn run_restore_from_archive(root: &Path, archive: &Path) -> Result<(), String> {
    // 1. 解压到暂存目录（不动现有数据）
    let staging = root.join(STAGING_DIR);
    if staging.exists() {
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            tracing::warn!("[restore] 清理旧暂存目录失败: {e}");
        }
    }
    let files = extract_archive(archive, &staging).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        e
    })?;

    // 2. 清空现有数据（保留运行时基础设施 / 标记 / 暂存目录）
    let removed = clear_root(root);

    // 3. 暂存内容搬入根目录（同卷 rename，失败回退复制）
    let entries = std::fs::read_dir(&staging).map_err(|e| format!("读取暂存目录失败: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if super::policy::backup_skip_top_level(&name.to_string_lossy()) {
            continue;
        }
        let target = root.join(&name);
        if target.exists() {
            let _ = if target.is_dir() {
                std::fs::remove_dir_all(&target)
            } else {
                std::fs::remove_file(&target)
            };
        }
        let src = entry.path();
        if std::fs::rename(&src, &target).is_err() {
            if src.is_dir() {
                copy_dir_recursive(&src, &target)?;
            } else {
                std::fs::copy(&src, &target)
                    .map_err(|e| format!("搬入 {} 失败: {e}", src.display()))?;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    tracing::info!(
        "[restore] 归档回填完成：删除旧数据 {} 项，恢复文件 {} 个",
        removed,
        files
    );
    Ok(())
}

/// 旧版文件夹恢复：清空当前数据 → 复制备份内容（源在外部磁盘，保持完整可重试）
fn run_restore_from_dir(root: &Path, backup_dir: &Path) -> Result<(), String> {
    if !backup_dir.join("characters").is_dir() && !backup_dir.join("config.yaml").is_file() {
        return Err(format!("备份目录无效: {}", backup_dir.display()));
    }

    // 1. 清空现有数据
    let removed = clear_root(root);

    // 2. 回填备份（跳过运行时基础设施）
    let mut restored = 0u64;
    for entry in std::fs::read_dir(backup_dir).map_err(|e| format!("读取备份目录失败: {e}"))?
    {
        let entry = entry.map_err(|e| format!("遍历备份目录失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if super::policy::backup_skip_top_level(&name) {
            continue;
        }
        let path = entry.path();
        let target = root.join(&name);
        if path.is_dir() {
            restored += copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| format!("恢复 {} 失败: {e}", path.display()))?;
            restored += 1;
        }
    }

    tracing::info!(
        "[restore] 备份回填完成：删除旧数据 {} 项，恢复文件 {} 个",
        removed,
        restored
    );
    Ok(())
}

/// 递归复制目录（含所有子项），返回复制的文件数
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<u64, String> {
    let mut count = 0u64;
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录 {} 失败: {e}", dst.display()))?;
    for entry in
        std::fs::read_dir(src).map_err(|e| format!("读取目录 {} 失败: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("遍历 {} 失败: {e}", src.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let target = dst.join(&name);
        if path.is_dir() {
            count += copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| format!("复制 {} 失败: {e}", path.display()))?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_keeps_browser_login_and_offline_state_but_omits_only_known_caches() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let keep = [
            "browser/state.json",
            "browser/engine/EBWebView/Local State",
            "browser/engine/EBWebView/WV2Profile_default/Network/Cookies",
            "browser/engine/EBWebView/WV2Profile_default/Local Storage/leveldb/data",
            "browser/engine/EBWebView/WV2Profile_default/Service Worker/CacheStorage/offline",
            "browser/engine/EBWebView/WV2Profile_default/Sessions/current",
        ];
        let skip = [
            "browser/engine/EBWebView/WV2Profile_default/Cache/cache",
            "browser/engine/EBWebView/GrShaderCache/data",
            "browser/engine/EBWebView/Crashpad/reports/report",
        ];
        for path in keep.iter().chain(skip.iter()) {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "preserve bytes").unwrap();
        }
        let archive = temp.path().join("data.altn");
        write_archive(&root, &archive).unwrap();
        let restored = temp.path().join("restored");
        extract_archive(&archive, &restored).unwrap();
        for path in keep {
            assert_eq!(
                std::fs::read(restored.join(path)).unwrap(),
                b"preserve bytes"
            );
        }
        for path in skip {
            assert!(!restored.join(path).exists());
            assert!(
                root.join(path).exists(),
                "backup must not delete live caches"
            );
        }
    }
    #[test]
    fn disk_budget_errors_and_completed_files_are_not_overwritten() {
        assert!(check_space(9, 10).unwrap_err().contains("空间不足"));
        assert!(check_space(10, 10).is_ok());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("file"), "data").unwrap();
        let archive = temp.path().join("existing.altn");
        std::fs::write(&archive, "existing").unwrap();
        assert!(write_archive(&root, &archive).is_err());
        assert_eq!(std::fs::read(&archive).unwrap(), b"existing");
        assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));
    }

    #[test]
    fn storage_backup_restores_live_wal_database_without_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("restored");
        let db = src.join("characters/vivian/companion-v2/memory/entries.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let writer = rusqlite::Connection::open(&db).unwrap();
        crate::utils::sqlite::configure(&writer, crate::utils::sqlite::DataClass::Authoritative)
            .unwrap();
        writer.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE entries(id TEXT, json TEXT); INSERT INTO entries VALUES ('a', '原始记忆');").unwrap();
        // An unrelated suffix must not be mistaken for SQLite infrastructure.
        std::fs::write(src.join("notes-wal"), "user file").unwrap();
        let transaction = writer.unchecked_transaction().unwrap();
        transaction
            .execute("INSERT INTO entries VALUES ('b', 'not committed')", [])
            .unwrap();
        let archive = tmp.path().join("live.altn");
        assert_eq!(write_archive(&src, &archive).unwrap(), 2);
        let manifest = read_manifest(&archive).unwrap();
        assert!(manifest.files.iter().any(|f| f.path == "notes-wal"));
        assert!(!manifest
            .files
            .iter()
            .any(|f| f.path.ends_with(".db-wal") || f.path.ends_with(".db-shm")));
        extract_archive(&archive, &dest).unwrap();
        let restored = rusqlite::Connection::open(
            dest.join("characters/vivian/companion-v2/memory/entries.db"),
        )
        .unwrap();
        let count: i64 = restored
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let content: String = restored
            .query_row("SELECT json FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content, "原始记忆");
        let check: String = restored
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        assert!(!std::fs::read_dir(tmp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vivian-backup-")));
    }

    #[test]
    fn storage_backup_snapshot_failure_cleans_staging_without_output() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        // Valid magic with a broken database body: never silently copy a failed snapshot.
        let mut bytes = b"SQLite format 3\0".to_vec();
        bytes.extend_from_slice(&[0u8; 128]);
        std::fs::write(src.join("broken.db"), &bytes).unwrap();
        let archive = tmp.path().join("broken.altn");
        assert!(write_archive(&src, &archive).is_err());
        assert!(!archive.exists());
        assert_eq!(std::fs::read(src.join("broken.db")).unwrap(), bytes);
        assert!(!std::fs::read_dir(tmp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vivian-backup-")));
    }

    #[test]
    fn conversation_backup_restore_preserves_original_bytes() {
        let tmp = std::env::temp_dir().join(format!(
            "vivian_conversation_backup_{}",
            uuid::Uuid::new_v4()
        ));
        let src = tmp.join("src");
        let dest = tmp.join("dest");
        let archive = tmp.join("conversation.altn");
        let records = [
            ("characters/vivian/companion-v2/history/chat_history.jsonl", "{\"role\":\"user\",\"content\":\"原文：你好 🌸\\n第二行\"}\n{\"role\":\"assistant\",\"content\":\"完整回复\"}\n"),
            ("characters/vivian/companion-v2/memory/conversation_archive.jsonl", "{\"level\":2,\"summary\":\"压缩摘要\"}\n"),
            ("characters/nana/companion-v2/history/chat_history.jsonl", "{\"role\":\"user\",\"content\":\"另一角色原文\"}\n"),
            ("characters/vivian/history/full_chat_history.json.migrated", "[\"旧版对话备份\"]"),
        ];
        for (rel, content) in records {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content.as_bytes()).unwrap();
        }
        assert_eq!(write_archive(&src, &archive).unwrap(), records.len() as u64);
        std::fs::create_dir_all(&dest).unwrap();
        run_restore(&dest, &archive).unwrap();
        for (rel, content) in records {
            assert_eq!(
                std::fs::read(dest.join(rel)).unwrap(),
                content.as_bytes(),
                "{rel}"
            );
        }
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn persona_backup_restores_definition_full_corpus_growth_and_its_source_memories() {
        use crate::persona::example_library::{self, ExampleEdit, ExampleOverride};
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let baseline = example_library::factory_examples("vivian");
        let mut sample = baseline
            .iter()
            .find(|e| e.scope == "comfort")
            .unwrap()
            .clone();
        sample.id = "grown-backup-voice".into();
        sample.response["text"] = "先歇会儿，想说了我再听。".into();
        let understanding = "先尊重沉默，不把陪伴变成追问";
        let update = ExampleOverride {
            scope: "comfort".into(),
            understanding: Some(understanding.into()),
            updated_at: 172800.0,
            edit: ExampleEdit {
                id: sample.id.clone(),
                reference: None,
                example: Some(sample.clone()),
                reason: "多日交流后的自主理解".into(),
            },
        };
        let evolution = serde_json::json!({
            "entries":[{"timestamp":172800.0,"kind":"personality","scope":"comfort","text":understanding,"reason":"跨日经历",
                "support":2,"explicit_feedback":false,"evidence":[{"memory_id":"day1","quote":"今天不想讲经过","timestamp":86400.0,"fingerprint":format!("{:x}",Sha256::digest("今天不想讲经过".as_bytes()))},
                {"memory_id":"day2","quote":"今天也先别追问","timestamp":172800.0,"fingerprint":format!("{:x}",Sha256::digest("今天也先别追问".as_bytes()))}]}],
            "candidates":[{"kind":"tone","scope":"daily","text":"待验证的表达","reason":"观察中","first_seen":172800.0,"support":1,"evidence":[],"examples":[]}],
            "history":[],"updated_at":172800.0,"example_overrides":[update],"example_history":[],"review_cursor":24
        });
        let mut expected = BTreeMap::new();
        for character in ["vivian", "nana"] {
            for (name, content) in [
                (
                    "persona.json",
                    serde_json::json!({"personality_definition":format!("{character} 的自定义人格"),"speech_definition":"自然简洁",
                    "expression":{"tsundere":0.3,"clingy":0.5,"genki":0.75,"sass":0.65,"healing":0.65,"curiosity":0.75,"ritual":0.5,"habit_awareness":0.65},"language_style":{}}),
                ),
                (
                    example_library::BASE_FILE,
                    serde_json::to_value(example_library::factory_examples(character)).unwrap(),
                ),
                ("evolution.json", evolution.clone()),
                (
                    "self_profile.json",
                    serde_json::json!({"entries":[],"last_web_reflection":172800.0}),
                ),
                (
                    "reading_corpus.json",
                    serde_json::json!({"items":[{"url":"https://example.com/read","body":"积累的阅读内容","fingerprint":"read-hash","read_at":172800.0}]}),
                ),
                (
                    "cards.json",
                    serde_json::json!({"cards":[],"active_card_id":null,"turn_counter":5,"last_update_turns":{}}),
                ),
                (
                    "dynamic_profile.json",
                    serde_json::json!({"turns":[],"acquired_behaviors":[]}),
                ),
            ] {
                let rel = format!("characters/{character}/persona/{name}");
                let bytes = serde_json::to_vec(&content).unwrap();
                let path = source.join(&rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, &bytes).unwrap();
                expected.insert(rel, bytes);
            }
        }
        for character in ["vivian", "nana"] {
            let db_path = source.join(format!(
                "characters/{character}/companion-v2/memory/entries.db"
            ));
            std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
            let db = rusqlite::Connection::open(&db_path).unwrap();
            db.execute_batch("CREATE TABLE entries(id TEXT PRIMARY KEY, content TEXT); INSERT INTO entries VALUES ('day1','今天不想讲经过'),('day2','今天也先别追问');").unwrap();
        }
        let archive = temp.path().join("complete.altn");
        write_archive(&source, &archive).unwrap();
        for (label, backup) in [
            ("archive", archive.as_path()),
            ("legacy-directory", source.as_path()),
        ] {
            let restored = temp.path().join(label);
            std::fs::create_dir_all(restored.join("characters/vivian/persona")).unwrap();
            std::fs::write(
                restored.join("characters/vivian/persona/post-backup.json"),
                "newer state",
            )
            .unwrap();
            run_restore(&restored, backup).unwrap();
            assert!(!restored
                .join("characters/vivian/persona/post-backup.json")
                .exists());
            for (rel, bytes) in &expected {
                assert_eq!(
                    &std::fs::read(restored.join(rel)).unwrap(),
                    bytes,
                    "{label}/{rel}"
                );
            }
            for character in ["vivian", "nana"] {
                let state: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(
                        restored.join(format!("characters/{character}/persona/evolution.json")),
                    )
                    .unwrap(),
                )
                .unwrap();
                let loaded_base = example_library::read_base_at(&restored.join(format!(
                    "characters/{character}/persona/{}",
                    example_library::BASE_FILE
                )))
                .unwrap();
                let edits: Vec<ExampleOverride> =
                    serde_json::from_value(state["example_overrides"].clone()).unwrap();
                let effective = example_library::materialize(
                    &loaded_base,
                    &edits,
                    &[("comfort".into(), understanding.into())],
                );
                assert!(effective
                    .iter()
                    .any(|e| e.id == sample.id && e.response == sample.response));
                let db = rusqlite::Connection::open(restored.join(format!(
                    "characters/{character}/companion-v2/memory/entries.db"
                )))
                .unwrap();
                for evidence in state["entries"][0]["evidence"].as_array().unwrap() {
                    let content: String = db
                        .query_row(
                            "SELECT content FROM entries WHERE id=?1",
                            [evidence["memory_id"].as_str().unwrap()],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert!(content.contains(evidence["quote"].as_str().unwrap()));
                    assert_eq!(
                        format!("{:x}", Sha256::digest(content.as_bytes())),
                        evidence["fingerprint"].as_str().unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn incomplete_backup_does_not_clear_existing_conversation() {
        let tmp =
            std::env::temp_dir().join(format!("vivian_incomplete_backup_{}", uuid::Uuid::new_v4()));
        let root = tmp.join("data");
        let history = root.join("characters/vivian/companion-v2/history/chat_history.jsonl");
        std::fs::create_dir_all(history.parent().unwrap()).unwrap();
        std::fs::write(&history, "现有对话原文").unwrap();
        let manifest = ArchiveManifest {
            version: 1,
            created_at: String::new(),
            dirs: vec![],
            files: vec![ArchiveFileEntry {
                path: "characters/vivian/companion-v2/history/chat_history.jsonl".into(),
                size: 100,
            }],
        };
        let header = serde_json::to_vec(&manifest).unwrap();
        let mut payload = ARCHIVE_MAGIC.to_vec();
        payload.extend_from_slice(&(header.len() as u32).to_le_bytes());
        payload.extend_from_slice(&header);
        payload.extend_from_slice(b"short");
        let archive = tmp.join("incomplete.altn");
        std::fs::write(&archive, zstd::encode_all(payload.as_slice(), 3).unwrap()).unwrap();
        assert!(run_restore(&root, &archive).is_err());
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "现有对话原文");
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// 归档写读往返：目录结构、文件内容、排除规则、清单路径一致性
    #[test]
    fn archive_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("vivian_backup_test_{}", std::process::id()));
        let src = tmp.join("src");
        let out = tmp.join("out.altn");
        let dest = tmp.join("dest");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(src.join("characters/nana/memory/plain")).unwrap();
        std::fs::create_dir_all(src.join("common/memory")).unwrap();
        std::fs::write(src.join("config.yaml"), "hello: 世界").unwrap();
        std::fs::write(
            src.join("characters/nana/memory/entries.db"),
            vec![7u8; 1024],
        )
        .unwrap();
        std::fs::write(
            src.join("characters/nana/memory/plain/abc.txt"),
            "plain mirror",
        )
        .unwrap();
        std::fs::write(src.join("common/memory/unified_memory.json"), "{}").unwrap();
        // 顶层排除项（logs）不应进入归档
        std::fs::create_dir_all(src.join("logs")).unwrap();
        std::fs::write(src.join("logs/x.log"), "log").unwrap();
        // 空目录也应被清单记录
        std::fs::create_dir_all(src.join("todo")).unwrap();

        let count = write_archive(&src, &out).unwrap();
        assert_eq!(count, 4, "logs 外的 4 个文件应全部打包");

        let manifest = read_manifest(&out).unwrap();
        assert_eq!(manifest.files.len(), 4);
        assert_eq!(manifest.version, 1);
        assert!(manifest
            .dirs
            .iter()
            .any(|d| d == "characters/nana/memory/plain"));
        assert!(manifest.dirs.iter().any(|d| d == "todo"));
        assert!(!manifest.files.iter().any(|f| f.path.starts_with("logs/")));

        let extracted = extract_archive(&out, &dest).unwrap();
        assert_eq!(extracted, 4);
        assert_eq!(
            std::fs::read_to_string(dest.join("config.yaml")).unwrap(),
            "hello: 世界"
        );
        assert_eq!(
            std::fs::read(dest.join("characters/nana/memory/entries.db")).unwrap(),
            vec![7u8; 1024]
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("characters/nana/memory/plain/abc.txt")).unwrap(),
            "plain mirror"
        );
        assert!(dest.join("todo").is_dir());
        assert!(!dest.join("logs").exists());

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// 路径安全校验：拒绝穿越 / 绝对路径 / 盘符 / 反斜杠
    #[test]
    fn safe_rel_rejects_traversal() {
        assert!(safe_rel("a/b/c.txt"));
        assert!(safe_rel("config.yaml"));
        assert!(!safe_rel("../evil"));
        assert!(!safe_rel("a/../../evil"));
        assert!(!safe_rel("/etc/passwd"));
        assert!(!safe_rel("C:/evil"));
        assert!(!safe_rel("a\\b"));
        assert!(!safe_rel(""));
        assert!(!safe_rel("a//b"));
    }

    /// 损坏的归档（随机内容）应在 read_manifest 即被拒绝
    #[test]
    fn invalid_archive_rejected() {
        let tmp = std::env::temp_dir().join(format!("vivian_backup_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let bad = tmp.join("bad.altn");
        std::fs::write(&bad, b"not a zstd stream at all").unwrap();
        assert!(read_manifest(&bad).is_err());
        // 合法 zstd 流但魔数不匹配
        let wrong = tmp.join("wrong.altn");
        std::fs::write(
            &wrong,
            zstd::encode_all(b"WRONGMAGIC".as_slice(), 3).unwrap(),
        )
        .unwrap();
        assert!(read_manifest(&wrong).is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
