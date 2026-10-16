//! Inspect and install local plugin directories or ZIPs without executing them.
use super::*;
use std::collections::HashSet;
use std::io::Read;

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 10_000;

pub struct StagedPackage { root: PathBuf, plugin: PathBuf, preserve: bool }
impl Drop for StagedPackage {
    fn drop(&mut self) {
        // Only a fresh UUID directory under the private staging root is removed.
        if !self.preserve && self.root.parent() == Some(plugins_dir().join(".staging").as_path()) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| match c {
        std::path::Component::Normal(part) => {
            let part = part.to_string_lossy();
            let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
            !part.contains([':', '\\']) && !part.ends_with(['.', ' ']) &&
                !["con","prn","aux","nul","com1","com2","com3","com4","com5","com6","com7","com8","com9","lpt1","lpt2","lpt3","lpt4","lpt5","lpt6","lpt7","lpt8","lpt9"].contains(&stem.as_str())
        }, _ => false,
    })
}

fn copy_directory(source: &Path, destination: &Path, files: &mut usize, bytes: &mut u64) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|e|e.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|e|e.to_string())? {
        let entry = entry.map_err(|e|e.to_string())?;
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e|e.to_string())?;
        if meta.file_type().is_symlink() || !is_safe_plugin_path(source, &entry.path()) { return Err("插件包不可包含符号链接或目录联接".into()); }
        if !safe_relative(Path::new(&entry.file_name())) { return Err("插件包包含不安全的文件名".into()); }
        *files += 1;
        *bytes = bytes.checked_add(meta.len()).ok_or("插件包过大")?;
        if *files > MAX_FILES || *bytes > MAX_BYTES { return Err("插件包超过 10000 个条目或 512 MiB".into()); }
        let target = destination.join(entry.file_name());
        if meta.is_dir() { copy_directory(&entry.path(), &target, files, bytes)?; }
        else if meta.is_file() { std::fs::copy(entry.path(), target).map_err(|e|e.to_string())?; }
        else { return Err("插件包包含非普通文件".into()); }
    }
    Ok(())
}

fn extract_zip(source: &Path, destination: &Path) -> Result<(), String> {
    let file = std::fs::File::open(source).map_err(|e|e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e|format!("ZIP 无效: {e}"))?;
    if zip.len() > MAX_FILES { return Err("ZIP 条目过多".into()); }
    let mut total = 0u64;
    let mut names = HashSet::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e|e.to_string())?;
        let relative = entry.enclosed_name().ok_or("ZIP 包含目录穿越路径")?.to_owned();
        if !safe_relative(&relative) || entry.unix_mode().is_some_and(|m|m & 0o170000 == 0o120000) { return Err("ZIP 包含不安全路径或符号链接".into()); }
        let target = destination.join(&relative);
        if entry.is_dir() { std::fs::create_dir_all(target).map_err(|e|e.to_string())?; continue; }
        if !names.insert(relative.to_string_lossy().to_lowercase()) { return Err("ZIP 包含重复文件".into()); }
        let remaining = MAX_BYTES.checked_sub(total).ok_or("ZIP 解压后过大")?;
        if entry.size() > remaining { return Err("ZIP 解压后超过 512 MiB".into()); }
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent).map_err(|e|e.to_string())?; }
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e|e.to_string())?;
        let written = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut file).map_err(|e|e.to_string())?;
        total += written;
        if total > MAX_BYTES { return Err("ZIP 解压后超过 512 MiB".into()); }
    }
    Ok(())
}

pub fn stage(source: &Path) -> Result<StagedPackage, String> {
    let root = plugins_dir().join(".staging").join(uuid::Uuid::new_v4().to_string());
    let unpacked = root.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e|e.to_string())?;
    let mut staged = StagedPackage { root, plugin: unpacked.clone(), preserve: false };
    if source.is_dir() {
        let canonical_source = std::fs::canonicalize(source).map_err(|e|e.to_string())?;
        let canonical_target = std::fs::canonicalize(&unpacked).map_err(|e|e.to_string())?;
        if canonical_target.starts_with(&canonical_source) { return Err("不能导入包含安装暂存目录的父目录".into()); }
        copy_directory(source, &unpacked, &mut 0, &mut 0)?;
    }
    else { extract_zip(source, &unpacked)?; }
    if !unpacked.join("plugin.json").is_file() {
        let children: Vec<_> = std::fs::read_dir(&unpacked).map_err(|e|e.to_string())?.filter_map(Result::ok)
            .map(|e|e.path()).filter(|p|p.is_dir() && p.join("plugin.json").is_file()).collect();
        if children.len() != 1 { return Err("插件包根目录或唯一子目录中必须包含 plugin.json".into()); }
        staged.plugin = children[0].clone();
    }
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(staged.plugin.join("plugin.json")).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if raw.get("name").and_then(serde_json::Value::as_str).is_none_or(|n|n.trim().is_empty()) { return Err("导入包的 plugin.json 必须声明 name".into()); }
    let (name, _) = read_manifest(&staged.plugin).map_err(|(_,e)|e)?;
    if !valid_plugin_dir_name(&name) || BUILTIN_PLUGIN_DIRS.contains(&name.as_str()) || name == "3d-apartment" { return Err("不能通过通用导入覆盖内置插件或公寓插件".into()); }
    Ok(staged)
}

fn package_fingerprint(root: &Path) -> Result<String, String> {
    fn collect(root: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(root).map_err(|e|e.to_string())? {
            let path = entry.map_err(|e|e.to_string())?.path();
            if std::fs::symlink_metadata(&path).map_err(|e|e.to_string())?.file_type().is_symlink() || !is_safe_plugin_path(root,&path) { return Err("插件包含符号链接或越界路径".into()); }
            if path.is_dir() { collect(&path, paths)?; } else { paths.push(path); }
            if paths.len() > MAX_FILES { return Err("插件文件过多".into()); }
        }
        Ok(())
    }
    let mut paths = Vec::new(); collect(root, &mut paths)?; paths.sort();
    let mut hash = Sha256::new();
    let mut total = 0u64;
    for path in paths {
        let relative = path.strip_prefix(root).map_err(|e|e.to_string())?.to_string_lossy().replace('\\', "/");
        let mut file = std::fs::File::open(&path).map_err(|e|e.to_string())?;
        let size = file.metadata().map_err(|e|e.to_string())?.len();
        total = total.checked_add(size).ok_or("插件过大")?;
        if total > MAX_BYTES { return Err("插件超过 512 MiB".into()); }
        hash.update(relative.as_bytes()); hash.update([0]); hash.update(size.to_le_bytes());
        let mut buffer = [0u8; 65536];
        loop { let count = file.read(&mut buffer).map_err(|e|e.to_string())?; if count == 0 { break; } hash.update(&buffer[..count]); }
        hash.update([1]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn preview_staged(staged: &StagedPackage) -> Result<serde_json::Value, String> {
    let (name, manifest) = read_manifest(&staged.plugin).map_err(|(_,e)|e)?;
    let previous = read_manifest(&plugins_dir().join(&name)).ok().map(|(_,m)|m.version);
    let previous_fingerprint = if previous.is_some() { package_fingerprint(&plugins_dir().join(&name))? } else { String::new() };
    let fingerprint = format!("{:x}",Sha256::digest(format!("{}:{previous_fingerprint}",package_fingerprint(&staged.plugin)?).as_bytes()));
    Ok(serde_json::json!({"key":name,"name":manifest.name,"version":manifest.version,"description":manifest.description,
        "previous_version":previous,"fingerprint":fingerprint,
        "missing_requirements":missing_requirements(&staged.plugin,&manifest),
        "skills":manifest.skills,"tools":manifest.tools,"mcp_servers":manifest.mcp_servers,"js":manifest.js,
        "requirements":manifest.requirements,
        "tool_definitions":expanded_tools(&staged.plugin,&manifest.tools),
        "js_sources":manifest.js.iter().flat_map(|pattern|glob_js_files(&staged.plugin,pattern)).map(|path|serde_json::json!({"path":path.strip_prefix(&staged.plugin).unwrap_or(&path).to_string_lossy(),"source":std::fs::read_to_string(path).unwrap_or_default()})).collect::<Vec<_>>()}))
}

pub fn inspect(source: &Path) -> Result<serde_json::Value, String> { preview_staged(&stage(source)?) }

pub async fn install(source: PathBuf, expected_fingerprint: String, skills: &SkillService, mcp: &McpManager, tools: &ToolSystem) -> Result<serde_json::Value, String> {
    let mut staged = tokio::task::spawn_blocking(move ||stage(&source)).await.map_err(|e|e.to_string())??;
    let operation = lifecycle_lock().lock().await;
    let preview = preview_staged(&staged)?;
    if preview["fingerprint"].as_str() != Some(expected_fingerprint.as_str()) { return Err("插件内容在预览后发生变化，请重新选择并预览".into()); }
    let key = preview["key"].as_str().ok_or("插件缺少名称")?;
    let destination = plugins_dir().join(key);
    let backup = staged.root.join("previous");
    let existed = destination.exists();
    if existed {
        if !is_safe_plugin_path(&plugins_dir(), &destination) { return Err("已安装插件目录越界".into()); }
        unload_contributions(skills, mcp, tools, &destination).await;
        js_host::unload_one(key); drop_snapshot(key);
        if let Err(error) = std::fs::rename(&destination, &backup) {
            drop(operation);
            let _ = load_one(skills,mcp,tools,key,false).await;
            return Err(format!("无法备份旧插件，已尝试恢复运行时: {error}"));
        }
    }
    if let Err(error) = std::fs::rename(&staged.plugin, &destination) {
        let restore = if existed {std::fs::rename(&backup, &destination)} else {Ok(())};
        if let Err(restore_error) = restore {
            // Preserve the recoverable copy if the OS also refuses rollback.
            staged.preserve = true;
            let recovery = plugins_dir().join(format!(".recovery-{key}-{}",uuid::Uuid::new_v4()));
            std::fs::rename(&backup,&recovery).map_err(|e|format!("更新失败: {error}；恢复失败: {restore_error}；备份移动失败: {e}，备份位置 {}",backup.display()))?;
            staged.preserve = false;
            return Err(format!("更新失败: {error}；旧版本已保留在 {}", recovery.display()));
        }
        drop(operation);
        if existed { let _ = load_one(skills,mcp,tools,key,false).await; }
        return Err(format!("插件安装失败，已尝试恢复旧版本: {error}"));
    }
    invalidate_fingerprint_cache();
    // Installing is separate from trusting. A changed package remains inactive.
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_package_paths_reject_windows_and_zip_escape_forms() {
        for path in ["../outside", "/outside", "C:/outside", "foo:stream", "CON.txt", "folder/../outside", "folder/trailing."] { assert!(!safe_relative(Path::new(path)), "{path}"); }
        assert!(safe_relative(Path::new("tools/read.json")));
    }

    #[test]
    fn extension_zip_rejects_escape_and_duplicate_files() {
        use std::io::Write;
        for names in [vec!["../outside"], vec!["x.txt", "X.txt"]] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("bad.zip");
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            for name in names { writer.start_file(name,zip::write::SimpleFileOptions::default()).unwrap(); writer.write_all(b"example").unwrap(); }
            writer.finish().unwrap();
            let destination = dir.path().join("out"); std::fs::create_dir(&destination).unwrap();
            assert!(extract_zip(&archive,&destination).is_err());
            assert!(!dir.path().join("outside").exists());
        }
    }

    #[tokio::test]
    async fn extension_package_preview_detects_changed_content_and_install_does_not_execute() {
        assert!(std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some(), "Run via the isolated Rust test runner");
        let source = tempfile::tempdir().unwrap();
        let key = format!("import-{}", uuid::Uuid::new_v4().simple());
        std::fs::create_dir(source.path().join("js")).unwrap();
        std::fs::write(source.path().join("plugin.json"),serde_json::json!({"name":key,"version":"1.0.0","js":["js/*.js"],"requirements":{"commands":["vivian-missing-runtime-fixture"]}}).to_string()).unwrap();
        std::fs::write(source.path().join("js/main.js"),"throw Error('must not execute during installation');").unwrap();
        std::fs::write(source.path().join("asset.txt"),"before").unwrap();
        let preview = inspect(source.path()).unwrap();
        assert_eq!(preview["missing_requirements"].as_array().unwrap().len(),1);
        let skills = SkillService::new(); let tools = std::sync::Arc::new(ToolSystem::new()); let mcp = McpManager::new(tools.clone()).unwrap();
        std::fs::write(source.path().join("asset.txt"),"after").unwrap();
        assert!(install(source.path().to_path_buf(),preview["fingerprint"].as_str().unwrap().into(),&skills,&mcp,&tools).await.is_err());
        let preview = inspect(source.path()).unwrap();
        install(source.path().to_path_buf(),preview["fingerprint"].as_str().unwrap().into(),&skills,&mcp,&tools).await.unwrap();
        let entry = scan_inventory().into_iter().find(|p|p.key==key).unwrap();
        assert_eq!(entry.trust,"untrusted"); assert_eq!(entry.status,"untrusted"); assert!(!js_host::is_loaded(&key));
        set_plugin_enabled(&key,false).unwrap();
        std::fs::write(source.path().join("plugin.json"),serde_json::json!({"name":key,"version":"2.0.0","js":["js/*.js"]}).to_string()).unwrap();
        let stale = inspect(source.path()).unwrap();
        std::fs::write(plugins_dir().join(&key).join("asset.txt"),"edited after preview").unwrap();
        assert!(install(source.path().to_path_buf(),stale["fingerprint"].as_str().unwrap().into(),&skills,&mcp,&tools).await.is_err());
        assert_eq!(read_manifest(&plugins_dir().join(&key)).unwrap().1.version,"1.0.0");
        let update = inspect(source.path()).unwrap();
        install(source.path().to_path_buf(),update["fingerprint"].as_str().unwrap().into(),&skills,&mcp,&tools).await.unwrap();
        let updated = scan_inventory().into_iter().find(|p|p.key==key).unwrap();
        assert_eq!(updated.version,"2.0.0"); assert_eq!(updated.status,"disabled"); assert!(!js_host::is_loaded(&key));
        delete_plugin(&skills,&mcp,&tools,&key).await.unwrap();
    }
}
