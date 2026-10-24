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

use crate::state::AppState;
use crate::storage::backup::{
    is_path_inside, read_manifest, resolve_backup_source, run_restore, BACKUP_DIR_PREFIX,
    BACKUP_EXT, RESTORE_MARKER,
};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// 一键备份：把用户数据目录压缩为 `<dest>/<vivian_backup_时间戳>.altn`
///
/// 备份前刷新各角色对话和记忆；落盘失败时不生成成功备份。
#[tauri::command]
pub async fn backup_user_data(
    dest_dir: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let dest = PathBuf::from(dest_dir);
    let root = crate::utils::path::get_user_data_dir();
    if !dest.is_dir() || is_path_inside(&dest, &root) {
        return Err("备份位置必须是数据目录外的文件夹".into());
    }
    if let Some(agent) = once_cell::sync::Lazy::get(&crate::commands::coding_agent::CODING_AGENT) {
        if agent.busy_for_backup() {
            return Err("工作任务仍在运行，请结束后再备份".into());
        }
    }
    {
        let chars = state.characters.read();
        if chars.values().any(|c| {
            c.think_lock.try_lock().is_err() || state.task_service.running_count(&c.id) > 0
        }) {
            return Err("角色正在处理任务，请本轮结束后再备份".into());
        }
    }
    flush_managers(&state)?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let destination = dest.join(format!(
        "{BACKUP_DIR_PREFIX}{stamp}_{}.{BACKUP_EXT}",
        uuid::Uuid::new_v4().simple()
    ));
    crate::storage::export::queue(&root, destination.clone())?;
    #[cfg(windows)]
    {
        match crate::embedded_browser::native::shutdown_for_backup(app.clone())
            .await
            .and_then(|mut pids| {
                let engine = root
                    .join("browser/engine")
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .replace('/', "\\");
                let processes = sysinfo::System::new_all();
                for (pid, process) in processes.processes() {
                    if process
                        .name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("msedgewebview2.exe")
                        && process.cmd().iter().any(|arg| {
                            arg.to_string_lossy()
                                .to_ascii_lowercase()
                                .replace('/', "\\")
                                .contains(&engine)
                        })
                    {
                        pids.push(pid.as_u32());
                    }
                }
                pids.sort_unstable();
                pids.dedup();
                crate::storage::export::set_browser_pids(&root, pids)
            }) {
            Ok(()) => {}
            Err(error) => {
                crate::storage::export::fail_pending(&root, error.clone())?;
                return Err(error);
            }
        }
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        app.request_restart();
    });
    // This is a queued destination. Completion is reported only after offline export.
    Ok(destination.to_string_lossy().into_owned())
}

fn flush_managers(state: &AppState) -> Result<(), String> {
    for (id, c) in state.characters.read().iter() {
        c.brain
            .dialogue
            .force_flush()
            .map_err(|e| format!("{id} 对话落盘失败: {e}"))?;
        c.brain
            .memory
            .flush_for_backup()
            .map_err(|e| format!("{id} 记忆落盘失败: {e}"))?;
        c.brain
            .mind
            .persist()
            .map_err(|e| format!("{id} 心智落盘失败: {e}"))?;
        c.brain
            .self_state
            .persist()
            .map_err(|e| format!("{id} 自我状态落盘失败: {e}"))?;
        c.brain.psychology.flush_for_backup()?;
        c.brain.persona.flush_for_backup().map_err(|e| format!("{id} 人设、进化及模仿语料落盘失败: {e}"))?;
        if let Some(chain) = &c.brain.chat_chain {
            chain.dynamic_profile.flush_for_backup().map_err(|e| format!("{id} 动态画像落盘失败: {e}"))?;
        }
        c.brain
            .proactive
            .flush_for_backup()
            .map_err(|e| e.to_string())?;
        c.brain
            .presence
            .flush_for_backup()
            .map_err(|e| e.to_string())?;
    }
    if let Some(agent) = once_cell::sync::Lazy::get(&crate::commands::coding_agent::CODING_AGENT) {
        agent.flush_for_backup()?;
    }
    #[cfg(windows)]
    crate::embedded_browser::native::flush_persistence()?;
    crate::providers::usage_store::try_flush()
}

pub(crate) fn flush_pending_on_exit(state: &AppState) {
    let root = crate::utils::path::get_user_data_dir();
    if crate::storage::export::pending(&root) {
        if let Err(error) = flush_managers(state) {
            if let Err(failure) = crate::storage::export::fail_pending(&root, error) {
                tracing::error!("[backup] 保存备份失败状态失败: {failure}");
            }
        }
    }
}

#[tauri::command]
pub fn take_backup_result() -> Result<Option<crate::storage::export::BackupResult>, String> {
    crate::storage::export::take_result(&crate::utils::path::get_user_data_dir())
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
    tracing::info!(
        "[restore] 检测到恢复标记，开始回填备份: {}",
        source.display()
    );

    // 校验失败：删除标记、按现有数据启动（不中断）
    if let Err(e) = run_restore(&root, &source) {
        tracing::error!("[restore] 恢复失败，保留现有数据继续启动: {e}");
    }
    let _ = std::fs::remove_file(&marker);
}
