//! 数据备份与恢复命令
//!
//! 用于更换设备时迁移桌宠的记忆、自进化、心理状态与配置等定制数据。
//!
//! - **备份**（`backup_user_data`）：把用户数据目录打包压缩为单个 `.altn` 归档，
//!   命名 `vivian_backup_YYYYMMDD_HHMMSS.altn`，跳过运行时基础设施
//!   （python-libs / logs / cache / pids）。
//! - **恢复**（`restore_user_data`）：校验备份源后写入 `.restore_pending` 标记
//!   （内含备份路径）并重启应用；重启后在 `AppState` 构造前由
//!   `restore_pending_if_any()` 消费标记——先解压到暂存目录，成功后才清空当前
//!   数据并搬入（解压失败则保留现有数据），复用 factory_reset 的标记-重启模式。
//!   同时兼容旧版「文件夹备份」与手工整目录拷贝作为恢复源。
//!
//! ## `.altn` 归档格式（扩展名仅为文件名标识，可任意改名，内容不变）
//!
//! ```text
//! zstd( MAGIC "ALTNBAK1" (8 字节)
//!       | manifest_len (u32 LE)
//!       | manifest JSON { version, created_at, dirs[], files[{path, size}] }
//!       | 各文件内容按 manifest 顺序拼接 )
//! ```
//!
//! manifest 内路径统一用 `/` 分隔（Windows 备份可在 macOS/Linux 恢复）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

/// 备份/恢复均跳过的运行时基础设施（用户数据目录顶层目录名）
///
/// - `python-libs`：内嵌 Python 运行时（约 280MB，新设备安装时自行释放）
/// - `logs` / `cache` / `pids`：运行期产物，无迁移价值
///
/// 注意：skills（智能体自建技能）/ plugins（用户插件）/ mcp（MCP 工具配置）等
/// 自进化内容属于用户定制数据，随备份打包并在恢复时回填；应用自带的美术与
/// Live2D 资源打包在安装目录（vivian.bundle.enc），不在用户数据目录内，天然
/// 不参与备份。
const BACKUP_EXCLUDE: &[&str] = &["python-libs", "logs", "cache", "pids"];

/// 备份归档扩展名（自定义格式标识，仅文件名后缀）
const BACKUP_EXT: &str = "altn";
/// 归档魔数（解压后有效负载的开头 8 字节）
const ARCHIVE_MAGIC: &[u8; 8] = b"ALTNBAK1";
/// 恢复待执行标记文件名（写入用户数据目录根，重启后消费）
const RESTORE_MARKER: &str = ".restore_pending";
/// 备份文件名前缀
const BACKUP_DIR_PREFIX: &str = "vivian_backup_";
/// 恢复暂存目录名（用户数据目录根下，解压成功后才正式搬入）
const STAGING_DIR: &str = ".restore_staging";

/// 归档清单：目录与文件的相对路径列表（`/` 分隔）+ 文件大小
#[derive(Serialize, Deserialize)]
struct ArchiveManifest {
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
fn is_path_inside(child: &Path, parent: &Path) -> bool {
    let norm = |p: &Path| -> PathBuf {
        // canonicalize 失败（目录不存在等）时退回绝对化，尽力比较
        p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
    };
    let c = norm(child);
    let p = norm(parent);
    c == p || c.starts_with(&p)
}

/// 备份时用户数据目录顶层需要跳过的条目
///
/// 注意 `.credentials.json` 等凭据文件需要随备份迁移，不跳过任何 dot 文件
/// （仅排除已知标记/暂存目录）。
fn top_level_backup_skip(name: &str) -> bool {
    BACKUP_EXCLUDE.contains(&name)
        || name == RESTORE_MARKER
        || name == STAGING_DIR
        || name == ".factory_reset_pending"
}

/// 校验归档内相对路径：拒绝绝对路径、盘符、反斜杠与 `.`/`..`/空组件（防路径穿越）
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains('\\')
        && !rel.contains(':')
        && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// 递归收集 `dir` 下全部子目录与文件（不含 `dir` 本身），`prefix` 为其相对路径
fn collect_walk(
    dir: &Path,
    prefix: &str,
    dirs: &mut Vec<String>,
    files: &mut Vec<(String, u64)>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("遍历 {} 失败: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let path = entry.path();
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

/// 打包：用户数据目录 → `.altn` 归档（zstd 流式压缩），返回文件数
fn write_archive(root: &Path, dest: &Path) -> Result<u64, String> {
    // 1. 收集清单（顶层应用排除规则，其下全量）
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| format!("读取用户数据目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("遍历用户数据目录失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if top_level_backup_skip(&name) {
            continue;
        }
        let path = entry.path();
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

    let manifest = ArchiveManifest {
        version: 1,
        created_at: chrono::Local::now().to_rfc3339(),
        files: files
            .iter()
            .map(|(p, s)| ArchiveFileEntry { path: p.clone(), size: *s })
            .collect(),
        dirs,
    };
    let header = serde_json::to_vec(&manifest).map_err(|e| format!("序列化清单失败: {e}"))?;

    // 2. 流式写入：magic + 清单 + 文件内容，整体经 zstd 压缩
    let out = std::fs::File::create(dest).map_err(|e| format!("创建备份文件失败: {e}"))?;
    let mut writer = std::io::BufWriter::new(out);
    let mut enc =
        zstd::stream::Encoder::new(&mut writer, 3).map_err(|e| format!("初始化压缩器失败: {e}"))?;
    enc.write_all(ARCHIVE_MAGIC).map_err(|e| format!("写入备份失败: {e}"))?;
    enc.write_all(&(header.len() as u32).to_le_bytes())
        .map_err(|e| format!("写入备份失败: {e}"))?;
    enc.write_all(&header).map_err(|e| format!("写入备份失败: {e}"))?;
    let mut count = 0u64;
    for (rel, _size) in &files {
        // 清单路径为 '/' 分隔；Windows 文件系统 API 同样接受 '/' 分隔符
        let src = root.join(rel);
        let mut f = std::fs::File::open(&src).map_err(|e| format!("打开 {} 失败: {e}", src.display()))?;
        std::io::copy(&mut f, &mut enc).map_err(|e| format!("写入 {} 失败: {e}", src.display()))?;
        count += 1;
    }
    enc.finish()
        .map_err(|e| format!("完成压缩失败: {e}"))?;
    writer.flush().map_err(|e| format!("写入备份失败: {e}"))?;
    Ok(count)
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
    let mut dec =
        zstd::stream::Decoder::new(f).map_err(|e| format!("备份文件解压失败（可能已损坏）: {e}"))?;
    let mut magic = [0u8; 8];
    dec.read_exact(&mut magic).map_err(|e| format!("读取备份失败: {e}"))?;
    if &magic != ARCHIVE_MAGIC {
        return Err("不是有效的 .altn 备份文件（格式标识不匹配）".to_string());
    }
    let mut len_buf = [0u8; 4];
    dec.read_exact(&mut len_buf).map_err(|e| format!("读取备份失败: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 32 * 1024 * 1024 {
        return Err("备份清单长度非法（可能已损坏）".to_string());
    }
    let mut buf = vec![0u8; len];
    dec.read_exact(&mut buf).map_err(|e| format!("读取备份失败: {e}"))?;
    let manifest: ArchiveManifest =
        serde_json::from_slice(&buf).map_err(|e| format!("备份清单解析失败: {e}"))?;
    Ok((manifest, 8 + 4 + len, dec))
}

/// 读取归档清单（仅解出 magic + manifest，不读文件内容），用于恢复前快速校验
fn read_manifest(archive: &Path) -> Result<ArchiveManifest, String> {
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
            return Err(format!("备份内容不完整: {}（期望 {} 字节，实际 {}）", entry.path, entry.size, n));
        }
        out.flush().map_err(|e| format!("写入 {} 失败: {e}", entry.path))?;
        count += 1;
    }
    Ok(count)
}

/// 一键备份：把用户数据目录压缩为 `<dest>/<vivian_backup_时间戳>.altn`
///
/// 备份前先 flush 各角色 MemoryManager，确保内存中的记忆条目落盘，
/// 让备份中的 SQLite 文件尽量新。
#[tauri::command]
pub async fn backup_user_data(
    dest_dir: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let dest = PathBuf::from(&dest_dir);
    let user_data = crate::utils::path::get_user_data_dir();

    if !dest.is_dir() {
        return Err(format!("目标目录不存在或不是文件夹: {}", dest.display()));
    }
    if is_path_inside(&dest, &user_data) {
        return Err("备份保存位置不能在应用数据目录内部（恢复时会被清除）".to_string());
    }

    // 先落盘各角色记忆（best effort，失败不阻塞备份）
    {
        let chars = state.characters.read();
        for (id, instance) in chars.iter() {
            if let Err(e) = instance.brain.memory.flush() {
                tracing::warn!("[backup] 角色 {} 记忆落盘失败（继续备份）: {e}", id);
            }
        }
    }

    // 时间戳文件名；同秒冲突时追加序号
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let mut archive_path = dest.join(format!("{BACKUP_DIR_PREFIX}{stamp}.{BACKUP_EXT}"));
    let mut seq = 1u32;
    while archive_path.exists() {
        seq += 1;
        archive_path = dest.join(format!("{BACKUP_DIR_PREFIX}{stamp}_{seq}.{BACKUP_EXT}"));
    }
    let archive_path = archive_path; // 后续只读

    let user_data_clone = user_data.clone();
    let result_path = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
        let files = write_archive(&user_data_clone, &archive_path)?;
        tracing::info!(
            "[backup] 备份完成：{} 个文件 → {}",
            files,
            archive_path.display()
        );
        Ok(archive_path)
    })
    .await
    .map_err(|e| format!("备份任务执行失败: {e}"))??;

    Ok(result_path.to_string_lossy().to_string())
}

/// 校验并解析用户选择的备份源（`.altn` 文件 / 旧版备份文件夹）：
/// 1. 是文件 → `.altn` 归档（内容校验由调用方 read_manifest 完成）
/// 2. 是目录且有备份特征（`characters/`、`config.yaml` 或旧版标记）→ 文件夹备份
/// 3. 目录下恰有一个 `.altn` 文件 → 自动下钻（用户选了归档所在文件夹）
/// 4. 目录下恰有一个旧版 `vivian_backup_*` 子目录 → 自动下钻
fn resolve_backup_source(path: &Path) -> Result<PathBuf, String> {
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
                && p.extension().map(|x| x.eq_ignore_ascii_case(BACKUP_EXT)).unwrap_or(false)
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

/// 一键恢复：校验备份 → 写恢复标记 → 重启应用
///
/// 与 factory_reset 相同的原子化模式：命令返回即意味着即将重启，
/// 实际数据回填发生在重启后、任何数据文件被打开之前。
#[tauri::command]
pub async fn restore_user_data(backup_path: String, app: tauri::AppHandle) -> Result<(), String> {
    let selected = PathBuf::from(&backup_path);
    let user_data = crate::utils::path::get_user_data_dir();

    let resolved = resolve_backup_source(&selected)?;
    if is_path_inside(&resolved, &user_data) {
        return Err("备份不能位于应用数据目录内部".to_string());
    }
    // 归档文件先做快速校验（魔数 + 清单），无效即刻反馈而不重启
    if resolved.is_file() {
        read_manifest(&resolved)?;
    }

    // 标记内容：备份绝对路径（重启后进程内读取）
    let marker = user_data.join(RESTORE_MARKER);
    std::fs::write(&marker, resolved.to_string_lossy().as_bytes())
        .map_err(|e| format!("写入恢复标记失败: {e}"))?;

    tracing::info!(
        "[restore] 已写入恢复标记，来源: {}，准备重启应用",
        resolved.display()
    );
    app.request_restart();
    Ok(())
}

/// 启动时消费恢复标记：解压备份 → 清空当前数据 → 搬入 → 删除标记
///
/// 必须在 `AppState::new()` 之前调用（任何 MemoryManager / 向量库初始化之前），
/// 此时数据文件未被打开，可无锁删除/覆盖（规避 vectors.db 的 SQLite 共享冲突）。
/// 任何失败都删除标记并按现有数据继续启动，绝不阻塞应用启动。
pub fn restore_pending_if_any() {
    let root = crate::utils::path::get_user_data_dir();
    let marker = root.join(RESTORE_MARKER);
    let Ok(content) = std::fs::read_to_string(&marker) else {
        return;
    };
    let source = PathBuf::from(content.trim());
    tracing::info!("[restore] 检测到恢复标记，开始回填备份: {}", source.display());

    // 校验失败：删除标记、按现有数据启动（不中断）
    if let Err(e) = run_restore(&root, &source) {
        tracing::error!("[restore] 恢复失败，保留现有数据继续启动: {e}");
    }
    let _ = std::fs::remove_file(&marker);
}

/// 清空用户数据目录（保留运行时基础设施、标记与暂存目录），返回删除条目数
fn clear_root(root: &Path) -> usize {
    let mut removed = 0usize;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if BACKUP_EXCLUDE.contains(&name.as_str())
            || name == RESTORE_MARKER
            || name == STAGING_DIR
            || name == ".factory_reset_pending"
        {
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
fn run_restore(root: &Path, source: &Path) -> Result<(), String> {
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
    for entry in std::fs::read_dir(backup_dir).map_err(|e| format!("读取备份目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("遍历备份目录失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if BACKUP_EXCLUDE.contains(&name.as_str()) {
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
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取目录 {} 失败: {e}", src.display()))? {
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
        std::fs::write(src.join("characters/nana/memory/entries.db"), vec![7u8; 1024]).unwrap();
        std::fs::write(src.join("characters/nana/memory/plain/abc.txt"), "plain mirror").unwrap();
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
        assert!(manifest.dirs.iter().any(|d| d == "characters/nana/memory/plain"));
        assert!(manifest.dirs.iter().any(|d| d == "todo"));
        assert!(!manifest.files.iter().any(|f| f.path.starts_with("logs/")));

        let extracted = extract_archive(&out, &dest).unwrap();
        assert_eq!(extracted, 4);
        assert_eq!(std::fs::read_to_string(dest.join("config.yaml")).unwrap(), "hello: 世界");
        assert_eq!(std::fs::read(dest.join("characters/nana/memory/entries.db")).unwrap(), vec![7u8; 1024]);
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
        std::fs::write(&wrong, zstd::encode_all(b"WRONGMAGIC".as_slice(), 3).unwrap()).unwrap();
        assert!(read_manifest(&wrong).is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
