//! File-backed SQLite policy: durable user records and rebuildable indexes.
//!
//! WAL is part of a live database, never an independent backup artifact. Use
//! `snapshot` to produce a self-contained copy without checkpointing the source.

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};

#[derive(Clone, Copy)]
pub enum DataClass {
    /// The only copy of user data: commit must sync the WAL to disk.
    Authoritative,
    /// Can be reconstructed from authoritative records (e.g. embeddings).
    Derived,
}

pub fn configure(conn: &Connection, class: DataClass) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(3))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-4096;
         PRAGMA wal_autocheckpoint=256;
         PRAGMA journal_size_limit=8388608;",
    )?;
    conn.pragma_update(
        None,
        "synchronous",
        match class {
            DataClass::Authoritative => "FULL",
            DataClass::Derived => "NORMAL",
        },
    )
}

/// Copy committed pages, including pages still in WAL, to a standalone database.
/// The destination must be a new path in a private staging directory. A bounded
/// read transaction pins the source version while other connections keep writing.
pub fn snapshot(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("数据库快照目标已存在".into());
    }
    let result = (|| -> Result<(), String> {
        let source = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        source
            .busy_timeout(Duration::from_millis(250))
            .map_err(|e| e.to_string())?;
        let read = source.unchecked_transaction().map_err(|e| e.to_string())?;
        // Establish the snapshot before the first incremental backup step.
        read.query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())?;
        let mut target = Connection::open(destination).map_err(|e| e.to_string())?;
        {
            let backup = Backup::new(&read, &mut target).map_err(|e| e.to_string())?;
            let started = Instant::now();
            loop {
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err("数据库快照超时，请稍后重试备份".into());
                }
                match backup.step(256).map_err(|e| e.to_string())? {
                    StepResult::Done => break,
                    StepResult::More => {}
                    StepResult::Busy | StepResult::Locked => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    _ => return Err("数据库快照返回未知状态".into()),
                }
            }
        }
        read.commit().map_err(|e| e.to_string())?;
        // Backup copies the source's WAL header too. Normalize the private target
        // so restoring just this file never requires a matching -wal or -shm.
        target
            .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
            .map_err(|e| e.to_string())?;
        target.close().map_err(|(_, e)| e.to_string())?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(destination)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_policy_distinguishes_records_and_indexes() {
        let dir = tempfile::tempdir().unwrap();
        for (name, class, synchronous) in [
            ("records.db", DataClass::Authoritative, 2),
            ("index.db", DataClass::Derived, 1),
        ] {
            let conn = Connection::open(dir.path().join(name)).unwrap();
            configure(&conn, class).unwrap();
            let journal: String = conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap();
            assert_eq!(journal, "wal");
            let sync: i64 = conn
                .pragma_query_value(None, "synchronous", |r| r.get(0))
                .unwrap();
            assert_eq!(sync, synchronous);
            let timeout: i64 = conn
                .pragma_query_value(None, "busy_timeout", |r| r.get(0))
                .unwrap();
            assert_eq!(timeout, 3000);
        }
    }

    #[test]
    fn storage_snapshot_includes_committed_wal_and_excludes_uncommitted_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.db");
        let copy = dir.path().join("snapshot.db");
        let writer = Connection::open(&path).unwrap();
        configure(&writer, DataClass::Authoritative).unwrap();
        writer.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE records(value TEXT); INSERT INTO records VALUES ('原始数据');").unwrap();
        // Keep the writer and its WAL open throughout backup, with an uncommitted row.
        let tx = writer.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO records VALUES ('uncommitted')", [])
            .unwrap();
        snapshot(&path, &copy).unwrap();
        assert!(path.with_file_name("live.db-wal").exists());
        assert!(!copy.with_file_name("snapshot.db-wal").exists());
        assert!(!copy.with_file_name("snapshot.db-shm").exists());
        let restored = Connection::open(&copy).unwrap();
        let values: Vec<String> = restored
            .prepare("SELECT value FROM records")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(values, ["原始数据"]);
        let check: String = restored
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        assert!(snapshot(&path, &copy).is_err());
    }
}
