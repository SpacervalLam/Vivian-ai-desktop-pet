//! Local, per-scope Git history for explicitly selected memory documents.
//! Canonical writes enqueue immutable snapshots; a single worker runs Git off
//! the response path. Pending snapshots survive missing Git and process exits.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FORMAT: u32 = 1;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    key: String,
    source: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    version: u32,
    sequence: u64,
    document: Document,
    before: Option<String>,
    after: String,
    reason: String,
    captured_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct QueueState {
    next: u64,
    last_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Version {
    pub commit: String,
    pub metadata: serde_json::Value,
}

#[derive(Clone)]
pub struct HistoryStore {
    data: PathBuf,
    git: PathBuf,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}
fn valid_key(key: &str) -> bool {
    let parts: Vec<_> = key.split('/').collect();
    match parts.as_slice() {
        ["characters", id] => component(id),
        ["work", "global"] => true,
        ["projects", hash] => hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        _ => false,
    }
}
fn regular_path(path: &Path) -> bool {
    !path
        .ancestors()
        .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|e| format!("history state: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let raw = serde_json::to_string(value).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(path, &raw).map_err(|e| e.to_string())
}

impl HistoryStore {
    pub fn new(data: impl Into<PathBuf>) -> Self {
        Self {
            data: data.into(),
            git: "git".into(),
        }
    }
    pub fn root(&self) -> PathBuf {
        self.data.join("memory-history")
    }

    pub fn document(&self, source: &Path) -> Option<Document> {
        let key = if source == self.data.join("work_memory/user.md") {
            "work/global".to_owned()
        } else if let Ok(relative) = source.strip_prefix(self.data.join("characters")) {
            let parts: Vec<_> = relative.iter().map(|p| p.to_str()).collect();
            match parts.as_slice() {
                [Some(id), Some("companion-v2"), Some("memory"), Some("memory.md")]
                    if component(id) =>
                {
                    format!("characters/{id}")
                }
                _ => return None,
            }
        } else if source.file_name()?.to_str()? == "memory.md"
            && source.parent()?.file_name()?.to_str()? == ".vivian"
        {
            let workspace = source.parent()?.parent()?.canonicalize().ok()?;
            let identity = workspace.to_string_lossy().into_owned();
            #[cfg(windows)]
            let identity = identity.to_lowercase();
            format!("projects/{}", digest(identity.as_bytes()))
        } else {
            return None;
        };
        Some(Document {
            key,
            source: source.to_owned(),
        })
    }

    fn directory(&self, doc: &Document) -> Result<PathBuf, String> {
        if !valid_key(&doc.key) {
            return Err("invalid memory history scope".into());
        }
        let dir = self.root().join(&doc.key);
        if !regular_path(&dir) {
            return Err("memory history does not follow symbolic links".into());
        }
        Ok(dir)
    }

    pub fn repository(&self, doc: &Document) -> Result<PathBuf, String> {
        Ok(self.directory(doc)?.join("repository.git"))
    }

    fn lock(&self, doc: &Document, kind: &str) -> Result<std::fs::File, String> {
        if !valid_key(&doc.key) {
            return Err("invalid memory history scope".into());
        }
        let path = self
            .data
            .join("memory-history-locks")
            .join(format!("{}.{kind}.lock", digest(doc.key.as_bytes())));
        if !regular_path(&path) {
            return Err("invalid history lock path".into());
        }
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        FileExt::lock_exclusive(&file).map_err(|e| e.to_string())?;
        Ok(file)
    }

    /// Only immutable note bodies are queued; credentials, DBs and indexes have
    /// no matching document route. No Git process is started by this function.
    pub fn enqueue(
        &self,
        doc: &Document,
        before: Option<&str>,
        after: &str,
        reason: &str,
    ) -> Result<bool, String> {
        let _queue = self.lock(doc, "queue")?;
        self.enqueue_locked(doc, before, after, reason)
    }

    fn enqueue_locked(
        &self,
        doc: &Document,
        before: Option<&str>,
        after: &str,
        reason: &str,
    ) -> Result<bool, String> {
        let dir = self.directory(doc)?;
        let pending_dir = dir.join("pending");
        std::fs::create_dir_all(&pending_dir).map_err(|e| e.to_string())?;
        let state_path = dir.join("queue.json");
        let mut state: QueueState = read_json(&state_path)?.unwrap_or_default();
        let files = pending_files(&pending_dir)?;
        let content_hash = digest(after.as_bytes());
        if state.last_content.as_deref() == Some(&content_hash)
            && (!files.is_empty() || dir.join("repository.git/HEAD").exists())
        {
            return Ok(false);
        }
        // Recover the allocation watermark if the process exited after the
        // pending file was durable but before queue.json was published.
        if let Some(last) = files
            .last()
            .and_then(|p| p.file_stem())
            .and_then(|n| n.to_str())
        {
            let next = last
                .parse::<u64>()
                .map_err(|_| "invalid history queue sequence")?
                .checked_add(1)
                .ok_or("history sequence exhausted")?;
            state.next = state.next.max(next);
        }
        let sequence = state.next;
        state.next = sequence
            .checked_add(1)
            .ok_or("history sequence exhausted")?;
        let pending = Pending {
            version: FORMAT,
            sequence,
            document: doc.clone(),
            before: before.map(str::to_owned),
            after: after.to_owned(),
            reason: reason.chars().take(80).collect(),
            captured_at: now(),
        };
        write_json(&pending_dir.join(format!("{sequence:020}.json")), &pending)?;
        state.last_content = Some(content_hash);
        write_json(&state_path, &state)?;
        Ok(true)
    }

    pub fn bootstrap(&self) -> Result<(), String> {
        std::fs::create_dir_all(self.root()).map_err(|e| e.to_string())?;
        write_json(
            &self.root().join("format.json"),
            &serde_json::json!({"version":FORMAT,"enabled":true,"local_only":true}),
        )?;
        // An empty global repository makes activation inspectable even before
        // the installation has accumulated its first note.
        let global = Document {
            key: "work/global".into(),
            source: self.data.join("work_memory/user.md"),
        };
        {
            let _history = self.lock(&global, "history")?;
            self.ensure_repository(&global)?;
        }
        let mut paths = vec![global.source];
        match std::fs::read_dir(self.data.join("characters")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                        paths.push(entry.path().join("companion-v2/memory/memory.md"));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        for source in paths {
            let Some(doc) = self.document(&source) else {
                continue;
            };
            let _queue = self.lock(&doc, "queue")?;
            if !regular_path(&source) {
                continue;
            }
            match std::fs::read_to_string(&source) {
                Ok(text) => {
                    self.enqueue_locked(&doc, None, &text, "startup_snapshot")?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    fn documents(&self) -> Result<Vec<Document>, String> {
        let mut documents = vec![Document {
            key: "work/global".into(),
            source: self.data.join("work_memory/user.md"),
        }];
        for group in ["characters", "projects"] {
            let entries = match std::fs::read_dir(self.root().join(group)) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            };
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                let key = format!("{group}/{}", entry.file_name().to_string_lossy());
                if entry.file_type().map_err(|e| e.to_string())?.is_dir() && valid_key(&key) {
                    documents.push(Document {
                        key,
                        source: PathBuf::new(),
                    });
                }
            }
        }
        Ok(documents)
    }

    pub fn flush(&self) -> Result<usize, String> {
        let mut processed = 0;
        let mut errors = Vec::new();
        for doc in self.documents()? {
            match self.flush_document(&doc) {
                Ok(count) => processed += count,
                Err(error) => errors.push(format!("{}: {error}", doc.key)),
            }
        }
        if errors.is_empty() {
            Ok(processed)
        } else {
            Err(errors.join("; "))
        }
    }

    fn flush_document(&self, doc: &Document) -> Result<usize, String> {
        let _history = self.lock(doc, "history")?;
        let files = pending_files(&self.directory(doc)?.join("pending"))?;
        if files.is_empty() {
            return Ok(0);
        }
        self.ensure_repository(doc)?;
        let mut count = 0;
        for path in files.into_iter().take(64) {
            let pending: Pending = read_json(&path)?.ok_or("history task disappeared")?;
            if pending.version != FORMAT || pending.document.key != doc.key {
                return Err("invalid history task".into());
            }
            if self.head(doc)?.is_none() {
                if let Some(before) = &pending.before {
                    self.commit(&pending, before, "initial_baseline")?;
                }
            }
            self.commit(&pending, &pending.after, &pending.reason)?;
            // Never advance to a later task if cleanup fails: restart can safely
            // replay this commit without reverting a more recent note.
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            count += 1;
        }
        Ok(count)
    }

    fn ensure_repository(&self, doc: &Document) -> Result<(), String> {
        let repo = self.repository(doc)?;
        let owner_path = self.directory(doc)?.join("owner.json");
        let owner: Option<serde_json::Value> = read_json(&owner_path)?;
        if owner
            .as_ref()
            .is_some_and(|value| value["version"] != FORMAT || value["scope"] != doc.key)
        {
            return Err("unsupported memory history repository format".into());
        }
        if repo.join("HEAD").is_file() {
            if owner.is_none() {
                return Err("unrecognized history repository".into());
            }
            return Ok(());
        }
        std::fs::create_dir_all(repo.parent().unwrap()).map_err(|e| e.to_string())?;
        write_json(
            &owner_path,
            &serde_json::json!({"version":FORMAT,"scope":doc.key}),
        )?;
        self.git(
            None,
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                "--object-format=sha1",
                "--template=",
                repo.to_str().ok_or("non-UTF-8 history path")?,
            ],
            None,
        )?;
        Ok(())
    }

    fn head(&self, doc: &Document) -> Result<Option<String>, String> {
        let repo = self.repository(doc)?;
        if repo.join("HEAD").is_file()
            && !repo.join("refs/heads/main").exists()
            && !repo.join("packed-refs").exists()
        {
            return Ok(None);
        }
        let raw = self.git(
            Some(&repo),
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            None,
        )?;
        Ok(Some(String::from_utf8_lossy(&raw).trim().to_owned()))
    }

    fn commit(&self, pending: &Pending, text: &str, reason: &str) -> Result<(), String> {
        let repo = self.repository(&pending.document)?;
        let head = self.head(&pending.document)?;
        let blob = self.git(
            Some(&repo),
            &["hash-object", "--no-filters", "-w", "--stdin"],
            Some(text.as_bytes()),
        )?;
        let blob = String::from_utf8_lossy(&blob).trim().to_owned();
        if let Some(head) = &head {
            let old = self.git(
                Some(&repo),
                &["rev-parse", &format!("{head}:memory.md")],
                None,
            )?;
            if String::from_utf8_lossy(&old).trim() == blob {
                return Ok(());
            }
        }
        let metadata = serde_json::to_vec(
            &serde_json::json!({"version":FORMAT,"sequence":pending.sequence,
            "scope":pending.document.key,"source":pending.document.source,"reason":reason,
            "captured_at":pending.captured_at,"content_sha256":digest(text.as_bytes()),
            "input_sha256":pending.before.as_ref().map(|s|digest(s.as_bytes()))}),
        )
        .map_err(|e| e.to_string())?;
        let meta_blob = self.git(
            Some(&repo),
            &["hash-object", "--no-filters", "-w", "--stdin"],
            Some(&metadata),
        )?;
        let tree = format!(
            "100644 blob {blob}\tmemory.md\n100644 blob {}\tmetadata.json\n",
            String::from_utf8_lossy(&meta_blob).trim()
        );
        let tree = self.git(Some(&repo), &["mktree"], Some(tree.as_bytes()))?;
        let tree = String::from_utf8_lossy(&tree).trim().to_owned();
        let mut args = vec!["commit-tree", &tree];
        if let Some(head) = &head {
            args.extend(["-p", head]);
        }
        let message = format!("Memory snapshot: {reason}\n");
        let commit = self.git(Some(&repo), &args, Some(message.as_bytes()))?;
        let commit = String::from_utf8_lossy(&commit).trim().to_owned();
        self.git(
            Some(&repo),
            &[
                "update-ref",
                "refs/heads/main",
                &commit,
                head.as_deref()
                    .unwrap_or("0000000000000000000000000000000000000000"),
            ],
            None,
        )?;
        Ok(())
    }

    pub fn versions(&self, doc: &Document, limit: usize) -> Result<Vec<Version>, String> {
        let _history = self.lock(doc, "history")?;
        let repo = self.repository(doc)?;
        if !repo.join("HEAD").exists() || self.head(doc)?.is_none() {
            return Ok(Vec::new());
        }
        let hashes = self.git(
            Some(&repo),
            &["log", &format!("-{}", limit.clamp(1, 50)), "--format=%H"],
            None,
        )?;
        String::from_utf8_lossy(&hashes)
            .lines()
            .map(|commit| {
                let raw = self.git(
                    Some(&repo),
                    &["show", &format!("{commit}:metadata.json")],
                    None,
                )?;
                let metadata = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
                Ok(Version {
                    commit: commit.into(),
                    metadata,
                })
            })
            .collect()
    }

    pub fn read_version(&self, doc: &Document, revision: &str) -> Result<String, String> {
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid history revision".into());
        }
        let _history = self.lock(doc, "history")?;
        let repo = self.repository(doc)?;
        self.git(
            Some(&repo),
            &["merge-base", "--is-ancestor", revision, "HEAD"],
            None,
        )?;
        let raw = self.git(
            Some(&repo),
            &["show", &format!("{revision}:memory.md")],
            None,
        )?;
        String::from_utf8(raw).map_err(|e| e.to_string())
    }

    /// Each scope has its own object database, so forgetting one character also
    /// removes its unreachable objects and pending snapshots without touching others.
    pub fn forget(&self, doc: &Document) -> Result<(), String> {
        let _queue = self.lock(doc, "queue")?;
        let _history = self.lock(doc, "history")?;
        let dir = self.directory(doc)?;
        if !dir.exists() {
            return Ok(());
        }
        let root = self.root().canonicalize().map_err(|e| e.to_string())?;
        let resolved = dir.canonicalize().map_err(|e| e.to_string())?;
        if resolved == root || !resolved.starts_with(&root) {
            return Err("invalid history deletion target".into());
        }
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
    }

    fn git(
        &self,
        repo: Option<&Path>,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let mut command = Command::new(&self.git);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Vivian Memory")
            .env("GIT_AUTHOR_EMAIL", "memory@vivian.local")
            .env("GIT_COMMITTER_NAME", "Vivian Memory")
            .env("GIT_COMMITTER_EMAIL", "memory@vivian.local");
        command.args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.logAllRefUpdates=false",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
        ]);
        if let Some(repo) = repo {
            command.arg("--git-dir").arg(repo);
        }
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start Git: {e}"))?;
        #[cfg(all(windows, not(test)))]
        {
            use std::os::windows::io::AsRawHandle;
            crate::utils::job_object::assign_process(child.as_raw_handle() as isize);
        }
        let stdin = child.stdin.take();
        let bytes = input.unwrap_or_default().to_vec();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                stdin.write_all(&bytes)
            } else {
                Ok(())
            }
        });
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        // Drain both pipes concurrently, including large historical project notes.
        let output = std::thread::spawn(move || {
            let mut out = Vec::new();
            stdout
                .take(32 * 1024 * 1024)
                .read_to_end(&mut out)
                .map(|_| out)
        });
        let errors = std::thread::spawn(move || {
            let mut out = Vec::new();
            stderr.take(64 * 1024).read_to_end(&mut out).map(|_| out)
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if start.elapsed() < COMMAND_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err("Git history command timed out".to_owned());
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error.to_string());
                }
            }
        };
        let write = writer.join().map_err(|_| "Git input worker failed")?;
        let output = output
            .join()
            .map_err(|_| "Git output worker failed")?
            .map_err(|e| e.to_string())?;
        let errors = errors
            .join()
            .map_err(|_| "Git error worker failed")?
            .map_err(|e| e.to_string())?;
        let status = status?;
        if !status.success() {
            return Err(format!(
                "Git {} failed: {}",
                args.first().unwrap_or(&"command"),
                String::from_utf8_lossy(&errors).trim()
            ));
        }
        write.map_err(|e| e.to_string())?;
        Ok(output)
    }
}

fn pending_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    match std::fs::read_dir(dir) {
        Ok(files) => {
            let mut paths = Vec::new();
            for entry in files {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_file()
                    && entry.path().extension().is_some_and(|e| e == "json")
                {
                    paths.push(entry.path());
                }
            }
            paths.sort();
            Ok(paths)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

struct Runtime {
    store: HistoryStore,
    wake: mpsc::SyncSender<()>,
}
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn start_runtime(data: PathBuf) {
    let (wake, receive) = mpsc::sync_channel(1);
    let store = HistoryStore::new(data);
    if RUNTIME
        .set(Runtime {
            store: store.clone(),
            wake,
        })
        .is_err()
    {
        return;
    }
    crate::brain::coding_memory_persistence::set_commit_observer(record_runtime_path);
    if let Err(error) = std::thread::Builder::new()
        .name("memory-git-history".into())
        .spawn(move || {
            let mut bootstrap = true;
            loop {
                let result = (|| {
                    if bootstrap {
                        store.bootstrap()?;
                        bootstrap = false;
                    }
                    let processed = store.flush()?;
                    write_json(
                        &store.root().join("status.json"),
                        &serde_json::json!({"enabled":true,"last_success":now(),"error":null}),
                    )?;
                    Ok::<_, String>(processed)
                })();
                match result {
                    Ok(processed) if processed >= 64 => continue,
                    Ok(_) => {
                        if receive.recv().is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        tracing::warn!("[memory-history] {error}; queued snapshots retained");
                        let _ = write_json(
                            &store.root().join("status.json"),
                            &serde_json::json!({"enabled":true,"error":error,"last_attempt":now()}),
                        );
                        std::thread::sleep(Duration::from_secs(30));
                    }
                }
            }
        })
    {
        tracing::error!("[memory-history] cannot start worker: {error}; snapshots remain queued");
    }
}

pub fn record_runtime_path(path: &Path, before: Option<&str>, after: &str, reason: &str) {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let Some(document) = runtime.store.document(path) else {
        return;
    };
    if let Err(error) = runtime.store.enqueue(&document, before, after, reason) {
        tracing::error!("[memory-history] cannot queue snapshot: {error}");
    }
    let _ = runtime.wake.try_send(());
}

pub fn runtime_character_versions(
    character: &str,
    revision: Option<&str>,
) -> Result<serde_json::Value, String> {
    let runtime = RUNTIME.get().ok_or("memory history has not started")?;
    let path = runtime
        .store
        .data
        .join("characters")
        .join(character)
        .join("companion-v2/memory/memory.md");
    let doc = runtime
        .store
        .document(&path)
        .ok_or("invalid character history scope")?;
    match revision {
        Some(revision) => Ok(
            serde_json::json!({"revision":revision,"content":runtime.store.read_version(&doc,revision)?}),
        ),
        None => Ok(
            serde_json::json!({"repository":runtime.store.repository(&doc)?,"versions":runtime.store.versions(&doc,20)?,"pending":pending_files(&runtime.store.directory(&doc)?.join("pending"))?.len()}),
        ),
    }
}

pub fn forget_runtime_path(path: &Path) -> Result<(), String> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    let doc = runtime
        .store
        .document(path)
        .ok_or("invalid memory history scope")?;
    runtime.store.forget(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, HistoryStore, Document) {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("中文 history data"));
        let source = store
            .data
            .join("characters/vivian/companion-v2/memory/memory.md");
        let doc = store.document(&source).unwrap();
        (dir, store, doc)
    }

    #[test]
    fn initial_baseline_and_changes_are_real_git_commits_with_exact_bytes() {
        let (_dir, store, doc) = setup();
        std::fs::create_dir_all(doc.source.parent().unwrap()).unwrap();
        std::fs::write(&doc.source, "新的约定\r\n").unwrap();
        store
            .enqueue(
                &doc,
                Some("旧的约定\r\n"),
                "新的约定\r\n",
                "note_consolidation",
            )
            .unwrap();
        assert_eq!(store.flush().unwrap(), 1);
        let versions = store.versions(&doc, 20).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].metadata["reason"], "note_consolidation");
        assert_eq!(
            store.read_version(&doc, &versions[1].commit).unwrap(),
            "旧的约定\r\n"
        );
        assert_eq!(
            store.read_version(&doc, &versions[0].commit).unwrap(),
            "新的约定\r\n"
        );
        assert_eq!(
            std::fs::read_to_string(&doc.source).unwrap(),
            "新的约定\r\n",
            "history must never change the live note"
        );
        assert!(store
            .git(Some(&store.repository(&doc).unwrap()), &["remote"], None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unchanged_content_does_not_queue_or_create_another_commit() {
        let (_dir, store, doc) = setup();
        store
            .enqueue(&doc, None, "记住约定", "note_append")
            .unwrap();
        store.flush().unwrap();
        assert!(!store
            .enqueue(&doc, Some("记住约定"), "记住约定", "startup_snapshot")
            .unwrap());
        assert_eq!(store.flush().unwrap(), 0);
        assert_eq!(store.versions(&doc, 20).unwrap().len(), 1);
    }

    #[test]
    fn missing_git_keeps_queue_and_a_reopened_store_can_retry() {
        let (_dir, mut store, doc) = setup();
        store.git = store.data.join("git-does-not-exist.exe");
        store
            .enqueue(&doc, Some("旧版本"), "新版本", "note_rewrite")
            .unwrap();
        assert!(store.flush().is_err());
        assert_eq!(
            pending_files(&store.directory(&doc).unwrap().join("pending"))
                .unwrap()
                .len(),
            1
        );
        let reopened = HistoryStore::new(&store.data);
        assert_eq!(reopened.flush().unwrap(), 1);
        assert_eq!(reopened.versions(&doc, 20).unwrap().len(), 2);
    }

    #[test]
    fn crash_after_commit_before_queue_cleanup_is_idempotent_and_fifo() {
        let (_dir, store, doc) = setup();
        store.enqueue(&doc, None, "第一份", "note_append").unwrap();
        let file = pending_files(&store.directory(&doc).unwrap().join("pending"))
            .unwrap()
            .remove(0);
        let raw = std::fs::read(&file).unwrap();
        store.flush().unwrap();
        std::fs::write(&file, raw).unwrap();
        store
            .enqueue(&doc, Some("第一份"), "第二份", "note_rewrite")
            .unwrap();
        store.flush().unwrap();
        let versions = store.versions(&doc, 20).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(
            store.read_version(&doc, &versions[0].commit).unwrap(),
            "第二份"
        );
    }

    #[test]
    fn pending_publication_recovers_a_stale_allocation_watermark() {
        let (_dir, store, doc) = setup();
        store.enqueue(&doc, None, "一", "note_append").unwrap();
        write_json(
            &store.directory(&doc).unwrap().join("queue.json"),
            &QueueState::default(),
        )
        .unwrap();
        store
            .enqueue(&doc, Some("一"), "二", "note_append")
            .unwrap();
        assert_eq!(
            pending_files(&store.directory(&doc).unwrap().join("pending"))
                .unwrap()
                .len(),
            2
        );
        store.flush().unwrap();
        assert_eq!(store.versions(&doc, 20).unwrap().len(), 2);
    }

    #[test]
    fn concurrent_producers_and_independent_workers_preserve_all_versions() {
        let (_dir, store, doc) = setup();
        std::thread::scope(|threads| {
            for n in 0..4 {
                let store = &store;
                let doc = &doc;
                threads.spawn(move || {
                    store
                        .enqueue(doc, None, &format!("约定 {n}"), "note_append")
                        .unwrap()
                });
            }
        });
        let another = HistoryStore::new(&store.data);
        std::thread::scope(|threads| {
            let a = threads.spawn(|| store.flush());
            let b = threads.spawn(|| another.flush());
            a.join().unwrap().unwrap();
            b.join().unwrap().unwrap();
        });
        let versions = store.versions(&doc, 20).unwrap();
        assert_eq!(versions.len(), 4);
        assert_eq!(versions[0].metadata["sequence"], 3);
    }

    #[test]
    fn forgetting_removes_only_one_characters_objects_and_pending_snapshots() {
        let (_dir, store, vivian) = setup();
        let nana = store
            .document(
                &store
                    .data
                    .join("characters/nana/companion-v2/memory/memory.md"),
            )
            .unwrap();
        store
            .enqueue(&vivian, None, "Vivian 的约定", "note_append")
            .unwrap();
        store
            .enqueue(&nana, None, "Nana 的约定", "note_append")
            .unwrap();
        store.flush().unwrap();
        store
            .enqueue(
                &vivian,
                Some("Vivian 的约定"),
                "尚未归档的新约定",
                "note_append",
            )
            .unwrap();
        store.forget(&vivian).unwrap();
        store.flush().unwrap();
        assert!(!store.directory(&vivian).unwrap().exists());
        assert_eq!(store.versions(&nana, 20).unwrap().len(), 1);
    }

    #[test]
    fn allowlist_and_scope_validation_exclude_credentials_databases_and_traversal() {
        let (dir, store, mut doc) = setup();
        for relative in [
            "config/config.yaml",
            "characters/vivian/companion-v2/memory/entries.db",
            "memory/key.json",
            "memory-history/status.json",
        ] {
            assert!(store.document(&store.data.join(relative)).is_none());
        }
        let workspace = dir.path().join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        let project = store
            .document(&workspace.join(".vivian/memory.md"))
            .unwrap();
        assert!(project.key.starts_with("projects/"));
        doc.key = "../outside".into();
        assert!(store.enqueue(&doc, None, "secret", "bad").is_err());
        assert!(store.forget(&doc).is_err());
        assert!(!dir.path().join("outside").exists());
    }

    #[test]
    fn bootstrap_imports_only_note_sources_and_rejects_unknown_repo_ownership() {
        let (_dir, store, doc) = setup();
        std::fs::create_dir_all(doc.source.parent().unwrap()).unwrap();
        std::fs::write(&doc.source, "已有的约定").unwrap();
        store.bootstrap().unwrap();
        store.flush().unwrap();
        assert_eq!(store.versions(&doc, 20).unwrap().len(), 1);
        store.bootstrap().unwrap();
        store.flush().unwrap();
        assert_eq!(store.versions(&doc, 20).unwrap().len(), 1);
        std::fs::remove_file(store.directory(&doc).unwrap().join("owner.json")).unwrap();
        store
            .enqueue(&doc, Some("已有的约定"), "新的内容", "note_rewrite")
            .unwrap();
        assert!(store.flush().unwrap_err().contains("unrecognized"));
    }
}
