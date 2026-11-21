//! Process-lifetime barrier for a quiescent, cross-manager durable export.
//! Normal applications hold a shared lease before constructing any managers.
//! A pending export runs under an exclusive lease, before the next AppState.
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const PENDING: &str = ".backup.pending.json";
const RESULT: &str = ".backup.result.json";
pub struct RuntimeLease {
    _file: File,
}

impl RuntimeLease {
    fn acquire(root: &Path, exclusive: bool, timeout: Duration) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".runtime.lease"))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        loop {
            let result = if exclusive {
                fs2::FileExt::try_lock_exclusive(&file)
            } else {
                fs2::FileExt::try_lock_shared(&file)
            };
            match result {
                Ok(()) => return Ok(Self { _file: file }),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    if started.elapsed() >= timeout {
                        return Err("等待应用写入屏障超时，未导出活动数据".into());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    pub fn shared(root: &Path) -> Result<Self, String> {
        Self::acquire(root, false, Duration::from_secs(30))
    }
}

#[derive(Serialize, Deserialize)]
pub struct BackupJob {
    version: u32,
    pub destination: PathBuf,
    origin_pid: u32,
    browser_pids: Vec<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct BackupResult {
    pub destination: PathBuf,
    pub error: Option<String>,
}

pub fn pending(root: &Path) -> bool {
    root.join(PENDING).exists()
}

pub fn queue(root: &Path, destination: PathBuf) -> Result<(), String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".backup.queue.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    if pending(root) {
        return Err("已有备份等待完成".into());
    }
    if super::backup::is_path_inside(&destination, root)
        || !destination.parent().is_some_and(Path::is_dir)
        || destination.exists()
    {
        return Err("备份目标无效或已存在".into());
    }
    super::backup::preflight(root, &destination)?;
    let job = BackupJob {
        version: 1,
        destination,
        origin_pid: std::process::id(),
        browser_pids: Vec::new(),
    };
    crate::utils::fs::atomic_write(
        &root.join(PENDING),
        &serde_json::to_vec(&job).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub fn set_browser_pids(root: &Path, pids: Vec<u32>) -> Result<(), String> {
    let mut job: BackupJob =
        serde_json::from_slice(&std::fs::read(root.join(PENDING)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    job.browser_pids = pids;
    crate::utils::fs::atomic_write(
        &root.join(PENDING),
        &serde_json::to_vec(&job).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub fn fail_pending(root: &Path, error: String) -> Result<(), String> {
    let job: BackupJob =
        serde_json::from_slice(&std::fs::read(root.join(PENDING)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    save_result(
        root,
        BackupResult {
            destination: job.destination,
            error: Some(error),
        },
    )
}
fn save_result(root: &Path, result: BackupResult) -> Result<(), String> {
    crate::utils::fs::atomic_write(
        &root.join(RESULT),
        &serde_json::to_vec(&result).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::remove_file(root.join(PENDING)).map_err(|e| e.to_string())
}

pub fn take_result(root: &Path) -> Result<Option<BackupResult>, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".backup.queue.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    let bytes = match std::fs::read(root.join(RESULT)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    std::fs::remove_file(root.join(RESULT)).map_err(|e| e.to_string())?;
    Ok(Some(value))
}

pub fn startup(root: &Path, is_alive: impl Fn(u32) -> bool) -> Result<RuntimeLease, String> {
    if pending(root) {
        // Waiting for this lease also waits for every old manager and in-flight
        // persistence thread to leave the preceding application process.
        let exclusive = RuntimeLease::acquire(root, true, Duration::from_secs(30))?;
        if pending(root) {
            let job: BackupJob = serde_json::from_slice(
                &std::fs::read(root.join(PENDING)).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if job.version != 1 {
                return Err("不支持的备份请求版本".into());
            }
            let result = (|| {
                if super::backup::is_path_inside(&job.destination, root) {
                    return Err("备份目标不能位于数据目录中".into());
                }
                let started = Instant::now();
                // exec-based restarts may preserve the PID. Acquiring the
                // exclusive lease already proves that the old image is gone.
                while (job.origin_pid != std::process::id() && is_alive(job.origin_pid))
                    || job.browser_pids.iter().any(|pid| is_alive(*pid))
                {
                    if started.elapsed() >= Duration::from_secs(30) {
                        return Err("旧应用或浏览器尚未退出，未生成不一致备份".into());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                super::backup::write_archive(root, &job.destination).map(|_| ())
            })();
            save_result(
                root,
                BackupResult {
                    destination: job.destination,
                    error: result.err(),
                },
            )?;
        }
        drop(exclusive);
    }
    RuntimeLease::shared(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_waits_for_all_manager_writers_and_snapshots_after_their_exit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        std::fs::create_dir(&root).unwrap();
        let archive = temp.path().join("data.altn");
        let lease = RuntimeLease::shared(&root).unwrap();
        std::fs::write(root.join("manager-a.json"), "old").unwrap();
        std::fs::write(root.join("manager-b.json"), "old").unwrap();
        queue(&root, archive.clone()).unwrap();
        let r = root.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            let lease = startup(&r, |_| false).unwrap();
            tx.send(()).unwrap();
            drop(lease);
        });
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        std::fs::write(root.join("manager-a.json"), "committed").unwrap();
        std::fs::write(root.join("manager-b.json"), "committed").unwrap();
        drop(lease);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        task.join().unwrap();
        assert!(archive.is_file());
        assert!(take_result(&root).unwrap().unwrap().error.is_none());
        let restored = temp.path().join("restored");
        std::fs::create_dir(&restored).unwrap();
        super::super::backup::run_restore(&restored, &archive).unwrap();
        assert_eq!(
            std::fs::read(restored.join("manager-a.json")).unwrap(),
            b"committed"
        );
        assert_eq!(
            std::fs::read(restored.join("manager-b.json")).unwrap(),
            b"committed"
        );
        assert!(!restored.join(".runtime.lease").exists());
        assert!(!restored.join(PENDING).exists());
    }
    #[test]
    fn export_does_not_report_queued_work_as_completed_or_overwrite_existing_archives() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        std::fs::create_dir(&root).unwrap();
        let dest = temp.path().join("data.altn");
        queue(&root, dest.clone()).unwrap();
        assert!(!dest.exists());
        assert!(take_result(&root).unwrap().is_none());
        assert!(queue(&root, temp.path().join("second.altn")).is_err());
        std::fs::write(&dest, "existing").unwrap();
        let lease = startup(&root, |_| false).unwrap();
        assert!(take_result(&root).unwrap().unwrap().error.is_some());
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        drop(lease);
    }
}
