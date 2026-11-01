//! Optional resources live beside the executable (bundled by the installer) or in the
//! writable user data directory (installed on demand). A single manifest describes every
//! pack; one that declares `download` is published on its own instead of being bundled.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAX_PACK_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Deserialize, Serialize, Clone)]
pub struct Download {
    pub filename: String,
    pub url: String,
    #[serde(default)]
    pub approx_mb: u64,
}

#[derive(Deserialize, Clone)]
pub struct PackSpec {
    pub id: String,
    pub label: String,
    pub version: String,
    /// Files that must be present for the pack to count as installed. Packs produced by a
    /// dedicated script ship more than they require; the rest is hash-verified at install.
    pub files: Vec<String>,
    /// Repository-relative directory used by debug builds instead of `public/`.
    #[serde(default)]
    pub dev_root: Option<String>,
    /// Present when the pack is published separately. The installer bundles packs that omit it.
    #[serde(default)]
    pub download: Option<Download>,
}

pub fn specs() -> Vec<PackSpec> {
    serde_json::from_str(include_str!("../../../src/optional-resources.json"))
        .expect("optional resource manifest")
}

pub fn spec(id: &str) -> Option<PackSpec> {
    specs().into_iter().find(|spec| spec.id == id)
}

fn installed_root(spec: &PackSpec, directory: &Path, development: bool) -> Option<PathBuf> {
    if !development {
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("pack.json")).ok()?).ok()?;
        if manifest["id"] != spec.id || manifest["version"] != spec.version {
            return None;
        }
        if !spec
            .files
            .iter()
            .all(|file| manifest["files"][file].as_str().is_some())
        {
            return None;
        }
    }
    spec.files
        .iter()
        .all(|file| directory.join(file).metadata().is_ok_and(|m| m.is_file() && m.len() > 0))
        .then(|| directory.to_path_buf())
}

fn optional_root() -> PathBuf {
    if let Some(directory) = std::env::var_os("VIVIAN_OPTIONAL_DIR") {
        return PathBuf::from(directory);
    }
    crate::utils::path::get_user_data_dir().join("optional")
}

/// Search order: the on-demand pack lives in the user directory, a bundled pack ships beside
/// the executable, and the other location is only a fallback. The development tree comes last.
fn candidates(spec: &PackSpec) -> Vec<(PathBuf, bool)> {
    let installed = optional_root().join(&spec.id);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("optional").join(&spec.id)));
    let mut roots = Vec::new();
    if spec.download.is_some() {
        roots.push((installed, false));
        roots.extend(beside.map(|path| (path, false)));
    } else {
        roots.extend(beside.map(|path| (path, false)));
        roots.push((installed, false));
    }
    #[cfg(debug_assertions)]
    {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        roots.push(match &spec.dev_root {
            Some(root) => (repository.join(root), true),
            None => (repository.join("public"), true),
        });
    }
    roots
}

pub fn pack_root(id: &str) -> Option<PathBuf> {
    let spec = spec(id)?;
    candidates(&spec)
        .into_iter()
        .find_map(|(directory, development)| installed_root(&spec, &directory, development))
}

pub fn installed(id: &str) -> bool {
    pack_root(id).is_some()
}

pub fn resource_path(id: &str, path: &str) -> Option<PathBuf> {
    let spec = spec(id)?;
    if !spec.files.iter().any(|file| file == path) {
        return None;
    }
    Some(pack_root(id)?.join(path))
}

#[derive(Serialize)]
pub struct PackStatus {
    id: String,
    label: String,
    version: String,
    installed: bool,
    root: Option<String>,
    download: Option<Download>,
}

fn status(spec: &PackSpec) -> PackStatus {
    let root = pack_root(&spec.id);
    PackStatus {
        id: spec.id.clone(),
        label: spec.label.clone(),
        version: spec.version.clone(),
        installed: root.is_some(),
        root: root.map(|root| root.to_string_lossy().into_owned()),
        download: spec.download.clone(),
    }
}

#[tauri::command]
pub fn optional_resource_status() -> Vec<PackStatus> {
    specs().iter().map(status).collect()
}

fn emit_progress(app: &tauri::AppHandle, id: &str, phase: &str, downloaded: u64, total: Option<u64>) {
    use tauri::Emitter;
    let _ = app.emit(
        "optional-resource:install-progress",
        serde_json::json!({"id":id,"phase":phase,"downloaded":downloaded,"total":total}),
    );
}

async fn download_pack(
    app: &tauri::AppHandle,
    spec: &PackSpec,
    download: &Download,
) -> Result<PathBuf, String> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    let url = std::env::var("VIVIAN_OPTIONAL_PACK_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| download.url.clone());
    let parsed = reqwest::Url::parse(&url).map_err(|e| format!("资源包下载地址无效: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("资源包下载地址必须使用 HTTPS".into());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("下载资源包失败: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("下载资源包失败: HTTP {}", response.status()));
    }
    let total = response.content_length();
    if total.is_some_and(|bytes| bytes > MAX_PACK_BYTES) {
        return Err(format!("资源包超过 {} MiB", MAX_PACK_BYTES / 1048576));
    }
    let directory = optional_root().join(".downloads");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| e.to_string())?;
    let path = directory.join(format!("{}.zip", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    let mut report_at = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载资源包失败: {e}"))?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or("资源包下载大小溢出")?;
        if downloaded > MAX_PACK_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(format!("资源包超过 {} MiB", MAX_PACK_BYTES / 1048576));
        }
        if let Err(error) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error.to_string());
        }
        if downloaded >= report_at {
            emit_progress(app, &spec.id, "downloading", downloaded, total);
            report_at = downloaded + 1024 * 1024;
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    emit_progress(app, &spec.id, "downloading", downloaded, total);
    Ok(path)
}

fn collect_files(root: &Path) -> Result<Vec<String>, String> {
    fn walk(root: &Path, directory: &Path, output: &mut Vec<String>) -> Result<(), String> {
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err("资源包不可包含符号链接".into());
            }
            let path = entry.path();
            if kind.is_dir() {
                walk(root, &path, output)?;
            } else if kind.is_file() {
                output.push(
                    path.strip_prefix(root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            } else {
                return Err("资源包包含非普通文件".into());
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    walk(root, root, &mut output)?;
    output.sort();
    Ok(output)
}

/// Recycled binary payloads: `<name>.lz` is an LZMA stream that becomes `<name>` on install.
/// The archive is deflated as a whole otherwise, which costs ~50% more for the Node binary.
fn expand_payloads(directory: &Path) -> Result<(), String> {
    use std::io::{BufReader, BufWriter, Write};
    struct Bounded<W> {
        inner: W,
        written: u64,
        limit: u64,
    }
    impl<W: Write> Write for Bounded<W> {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written = self
                .written
                .checked_add(buffer.len() as u64)
                .ok_or_else(|| std::io::Error::other("资源包解压大小溢出"))?;
            if self.written > self.limit {
                return Err(std::io::Error::other("资源包解压后过大"));
            }
            self.inner.write(buffer)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    fn walk(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                walk(&path, output)?;
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "lz") {
                output.push(path);
            }
        }
        Ok(())
    }
    let mut payloads = Vec::new();
    walk(directory, &mut payloads)?;
    // The budget spans every payload: a ZIP with many `.lz` entries must not fill the disk
    // merely because each individual stream stays under the per-file cap.
    let mut expanded = 0u64;
    for payload in payloads {
        let remaining = MAX_PACK_BYTES
            .checked_sub(expanded)
            .ok_or("资源包解压后过大")?;
        let target = payload.with_extension("");
        let mut input =
            BufReader::new(std::fs::File::open(&payload).map_err(|e| e.to_string())?);
        let mut output = Bounded {
            inner: BufWriter::new(std::fs::File::create(&target).map_err(|e| e.to_string())?),
            written: 0,
            limit: remaining,
        };
        lzma_rs::lzma_decompress(&mut input, &mut output)
            .map_err(|e| format!("解压 {} 失败: {e}", payload.display()))?;
        output.flush().map_err(|e| e.to_string())?;
        expanded = expanded
            .checked_add(output.written)
            .ok_or("资源包解压后过大")?;
        drop(output);
        std::fs::remove_file(&payload).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Reject extra entries, tampered files and packages built for another version.
fn verify_pack_tree(directory: &Path, spec: &PackSpec) -> Result<(), String> {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("pack.json")).map_err(|_| "资源包缺少 pack.json")?,
    )
    .map_err(|e| format!("资源包清单无效: {e}"))?;
    if manifest["id"] != spec.id || manifest["version"] != spec.version {
        return Err("资源包与当前版本不匹配".into());
    }
    let files = manifest["files"]
        .as_object()
        .ok_or("资源包清单缺少文件哈希")?;
    let mut expected = files.keys().cloned().collect::<Vec<_>>();
    expected.push("pack.json".into());
    expected.sort();
    if collect_files(directory)? != expected {
        return Err("资源包包含清单之外的文件".into());
    }
    for file in &spec.files {
        if !files.contains_key(file) || !directory.join(file).is_file() {
            return Err(format!("资源包缺少必需文件: {file}"));
        }
    }
    let mut total = 0u64;
    for (file, hash) in files {
        let expected = hash.as_str().ok_or("资源包文件哈希无效")?;
        let path = directory.join(file);
        let size = path.metadata().map_err(|e| format!("{file}: {e}"))?.len();
        total = total.checked_add(size).ok_or("资源包过大")?;
        if total > MAX_PACK_BYTES {
            return Err(format!("资源包解压后超过 {} MiB", MAX_PACK_BYTES / 1048576));
        }
        let mut hasher = Sha256::new();
        let mut source = std::fs::File::open(&path).map_err(|e| format!("{file}: {e}"))?;
        std::io::copy(&mut source, &mut hasher).map_err(|e| format!("{file}: {e}"))?;
        if format!("{:x}", hasher.finalize()) != expected {
            return Err(format!("资源包文件校验失败: {file}"));
        }
    }
    Ok(())
}

fn install_archive(source: &Path, root: &Path, spec: &PackSpec) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let staging = root
        .join(".staging")
        .join(uuid::Uuid::new_v4().to_string());
    let unpacked = staging.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&staging);
    };
    if let Err(error) = crate::plugins::package::extract_zip(source, &unpacked) {
        cleanup();
        return Err(error);
    }
    if let Err(error) = expand_payloads(&unpacked).and_then(|()| verify_pack_tree(&unpacked, spec))
    {
        cleanup();
        return Err(error);
    }
    let destination = root.join(&spec.id);
    let previous = staging.join("previous");
    let existed = destination.exists();
    if existed {
        if let Err(error) = std::fs::rename(&destination, &previous) {
            cleanup();
            return Err(format!("无法备份旧资源包: {error}"));
        }
    }
    if let Err(error) = std::fs::rename(&unpacked, &destination) {
        if existed && std::fs::rename(&previous, &destination).is_err() {
            return Err(format!(
                "资源包安装失败: {error}；旧版本保留在 {}",
                previous.display()
            ));
        }
        cleanup();
        return Err(format!("资源包安装失败: {error}"));
    }
    cleanup();
    Ok(destination)
}

static INSTALL_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// Install a published pack from its download or from a ZIP the user picked.
#[tauri::command]
pub async fn install_optional_resource(
    app: tauri::AppHandle,
    id: String,
    source: Option<String>,
) -> Result<PackStatus, String> {
    let _guard = INSTALL_LOCK.lock().await;
    let spec = spec(&id).ok_or_else(|| format!("未知的可选资源: {id}"))?;
    let download = spec
        .download
        .clone()
        .ok_or_else(|| format!("{} 随安装程序分发，无需单独下载", spec.label))?;
    let mut fetched = None;
    let archive = match source
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        Some(path) => {
            let path =
                std::fs::canonicalize(&path).map_err(|e| format!("无法读取所选 ZIP: {e}"))?;
            if !path.is_file() {
                return Err("所选路径不是 ZIP 文件".into());
            }
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("zip"))
            {
                return Err("所选文件不是 ZIP 文件".into());
            }
            path
        }
        None => {
            emit_progress(&app, &spec.id, "downloading", 0, None);
            let path = download_pack(&app, &spec, &download).await?;
            fetched = Some(path.clone());
            path
        }
    };
    emit_progress(&app, &spec.id, "verifying", 0, None);
    let install_root = optional_root();
    let task_spec = spec.clone();
    let result = tokio::task::spawn_blocking(move || {
        install_archive(&archive, &install_root, &task_spec)
    })
    .await;
    if let Some(path) = fetched {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result.map_err(|e| e.to_string())??;
    emit_progress(&app, &spec.id, "done", 0, None);
    Ok(status(&spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn font_spec() -> PackSpec {
        PackSpec {
            id: "fonts".into(),
            label: "fonts".into(),
            version: "1.0.0".into(),
            files: vec!["font.woff2".into()],
            dev_root: None,
            download: None,
        }
    }

    #[test]
    fn partial_and_wrong_version_packs_are_unavailable() {
        let folder = tempfile::tempdir().unwrap();
        let spec = font_spec();
        assert!(installed_root(&spec, folder.path(), false).is_none());
        std::fs::write(
            folder.path().join("pack.json"),
            r#"{"id":"fonts","version":"1.0.0","files":{"font.woff2":"hash"}}"#,
        )
        .unwrap();
        assert!(installed_root(&spec, folder.path(), false).is_none());
        std::fs::write(folder.path().join("font.woff2"), b"font").unwrap();
        assert!(installed_root(&spec, folder.path(), false).is_some());
        std::fs::write(
            folder.path().join("pack.json"),
            r#"{"id":"fonts","version":"2.0.0"}"#,
        )
        .unwrap();
        assert!(installed_root(&spec, folder.path(), false).is_none());
        assert!(resource_path("fonts", "../../outside").is_none());
    }

    fn runtime_spec() -> PackSpec {
        PackSpec {
            id: "browser-runtime".into(),
            label: "runtime".into(),
            version: "1".into(),
            files: vec!["node.exe".into()],
            dev_root: None,
            download: Some(Download {
                filename: "runtime.zip".into(),
                url: "https://example.invalid/runtime.zip".into(),
                approx_mb: 1,
            }),
        }
    }

    fn write_archive(path: &Path, spec: &PackSpec, extra: bool, corrupt: bool) {
        // The manifest always describes the genuine payload; `corrupt` swaps the shipped bytes
        // so the hash check is what rejects the archive.
        let genuine = b"binary".to_vec();
        let shipped = if corrupt { b"tampered".to_vec() } else { genuine.clone() };
        let mut files = serde_json::Map::new();
        files.insert(
            "node.exe".into(),
            serde_json::json!(format!("{:x}", Sha256::digest(&genuine))),
        );
        let manifest = serde_json::json!({"id":spec.id,"version":spec.version,"files":files});
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        writer
            .start_file("pack.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(manifest.to_string().as_bytes()).unwrap();
        writer
            .start_file("node.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&shipped).unwrap();
        if extra {
            writer
                .start_file("unexpected.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"extra").unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn runtime_install_verifies_hashes_versions_and_entry_sets() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("optional");
        let spec = runtime_spec();
        let archive = folder.path().join("runtime.zip");
        write_archive(&archive, &spec, false, false);
        let installed = install_archive(&archive, &root, &spec).unwrap();
        assert!(installed.join("node.exe").is_file());
        assert!(installed_root(&spec, &installed, false).is_some());

        let corrupt = folder.path().join("corrupt.zip");
        write_archive(&corrupt, &spec, false, true);
        assert!(install_archive(&corrupt, &root, &spec).is_err());
        assert!(installed.join("node.exe").is_file(), "旧版本必须保留");

        let extra = folder.path().join("extra.zip");
        write_archive(&extra, &spec, true, false);
        assert!(install_archive(&extra, &root, &spec).is_err());
    }

    #[test]
    fn compressed_payloads_expand_into_the_declared_name() {
        let folder = tempfile::tempdir().unwrap();
        let payload = b"node binary".to_vec();
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress(&mut payload.as_slice(), &mut compressed).unwrap();
        std::fs::write(folder.path().join("node.exe.lz"), &compressed).unwrap();
        std::fs::create_dir_all(folder.path().join("nested")).unwrap();
        std::fs::write(folder.path().join("nested/task.mjs.lz"), &compressed).unwrap();
        expand_payloads(folder.path()).unwrap();
        assert_eq!(std::fs::read(folder.path().join("node.exe")).unwrap(), payload);
        assert_eq!(
            std::fs::read(folder.path().join("nested/task.mjs")).unwrap(),
            payload
        );
        assert!(!folder.path().join("node.exe.lz").exists());
        assert!(!folder.path().join("nested/task.mjs.lz").exists());
    }
}
