//! Dependency validation and deterministic load order, shared by all entry points.
use super::*;
use std::collections::{BTreeMap, HashSet};

pub(super) fn missing(pdir: &Path, manifest: &PluginManifest) -> Vec<String> {
    fn visit(
        pdir: &Path,
        manifest: &PluginManifest,
        stack: &mut Vec<String>,
        missing: &mut Vec<String>,
    ) {
        for command in &manifest.requirements.commands {
            let direct = Path::new(command);
            let found = if direct.is_absolute() {
                direct.is_file()
            } else if command.contains(['/', '\\']) {
                is_safe_manifest_path(command)
                    && pdir.join(command).is_file()
                    && is_safe_plugin_path(pdir, &pdir.join(command))
            } else {
                let extensions: &[&str] = if cfg!(windows) {
                    &["", ".exe", ".cmd", ".bat", ".com"]
                } else {
                    &[""]
                };
                std::env::var_os("PATH").is_some_and(|value| {
                    std::env::split_paths(&value).any(|dir| {
                        extensions
                            .iter()
                            .any(|ext| dir.join(format!("{command}{ext}")).is_file())
                    })
                })
            };
            if !found {
                missing.push(format!("缺少运行命令：{command}"));
            }
        }
        let mut dependencies: Vec<_> = manifest.requirements.plugins.iter().collect();
        dependencies.sort_by_key(|(key, _)| *key);
        for (key, minimum) in dependencies {
            if !valid_plugin_dir_name(key) {
                missing.push(format!("依赖插件目录名无效：{key}"));
                continue;
            }
            if stack.contains(key) {
                missing.push(format!("插件循环依赖：{} → {key}", stack.join(" → ")));
                continue;
            }
            let root = plugins_dir().join(key);
            let Ok((_, dependency)) = read_manifest(&root) else {
                missing.push(format!("缺少依赖插件：{key}"));
                continue;
            };
            if !is_plugin_enabled(key)
                || is_inactive(key)
                || trust_status_of(key, &cached_fingerprint(&root, &dependency)) != "trusted"
            {
                missing.push(format!("需要已启用、受信且未卸载的插件：{key}"));
                continue;
            }
            if !minimum.is_empty()
                && !parse_version(&dependency.version)
                    .zip(parse_version(minimum))
                    .is_some_and(|(actual, min)| actual >= min)
            {
                missing.push(format!(
                    "需要插件 {key} >= {minimum}（当前 {}）",
                    dependency.version
                ));
                continue;
            }
            stack.push(key.clone());
            visit(&root, &dependency, stack, missing);
            stack.pop();
        }
    }
    let key = if pdir.parent() == Some(plugins_dir().as_path()) {
        pdir.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        manifest.name.clone()
    };
    let mut result = Vec::new();
    visit(pdir, manifest, &mut vec![key], &mut result);
    result.sort();
    result.dedup();
    result
}

pub(super) fn ordered_dirs() -> (Vec<PathBuf>, Vec<String>) {
    let mut manifests = BTreeMap::new();
    let mut errors = Vec::new();
    if let Ok(entries) = std::fs::read_dir(plugins_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let key = entry.file_name().to_string_lossy().into_owned();
            if !valid_plugin_dir_name(&key) || !path.is_dir() || !path.join("plugin.json").exists()
            {
                continue;
            }
            match read_manifest(&path) {
                Ok((_, manifest)) => {
                    manifests.insert(key, (path, manifest));
                }
                Err((key, error)) => errors.push(format!("{key}: {error}")),
            }
        }
    }
    fn visit(
        key: &str,
        manifests: &BTreeMap<String, (PathBuf, PluginManifest)>,
        stack: &mut Vec<String>,
        done: &mut HashSet<String>,
        out: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        if done.contains(key) {
            return Ok(());
        }
        if stack.iter().any(|entry| entry == key) {
            return Err(format!("插件循环依赖：{} → {key}", stack.join(" → ")));
        }
        let Some((path, manifest)) = manifests.get(key) else {
            return Err(format!("缺少依赖插件：{key}"));
        };
        stack.push(key.to_string());
        let mut dependencies: Vec<_> = manifest.requirements.plugins.keys().collect();
        dependencies.sort();
        for dependency in dependencies {
            visit(dependency, manifests, stack, done, out)?;
        }
        stack.pop();
        done.insert(key.to_string());
        out.push(path.clone());
        Ok(())
    }
    let mut out = Vec::new();
    let mut done = HashSet::new();
    for key in manifests.keys() {
        if let Err(error) = visit(key, &manifests, &mut Vec::new(), &mut done, &mut out) {
            errors.push(format!("{key}: {error}"));
        }
    }
    errors.sort();
    errors.dedup();
    (out, errors)
}

pub(super) fn runtime_missing(manifest: &PluginManifest) -> Vec<String> {
    runtime_missing_for(&manifest.requirements.plugins)
}

pub(super) fn runtime_missing_for(requirements: &HashMap<String, String>) -> Vec<String> {
    fn ready(key: &str, minimum: &str, visiting: &mut HashSet<String>) -> bool {
        if !visiting.insert(key.into()) {
            return false;
        }
        let result = read_snapshot(key).is_some_and(|snapshot| {
            is_plugin_enabled(key)
                && !is_inactive(key)
                && (minimum.is_empty()
                    || parse_version(&snapshot.version)
                        .zip(parse_version(minimum))
                        .is_some_and(|(actual, min)| actual >= min))
                && (!snapshot.js || js_host::is_loaded(key))
                && snapshot
                    .dependencies
                    .iter()
                    .all(|(key, minimum)| ready(key, minimum, visiting))
        });
        visiting.remove(key);
        result
    }
    let mut missing = Vec::new();
    for (key, minimum) in requirements {
        if !ready(key, minimum, &mut HashSet::new()) {
            missing.push(format!("依赖插件 {key} 尚未成功装载，请先加载该插件"));
        }
    }
    missing.sort();
    missing
}

/// Dependents are stopped before the dependency, using the committed graph so
/// editing a manifest cannot conceal old runtime dependencies.
pub(super) fn dependent_keys(key: &str) -> Vec<String> {
    let snapshots = contrib_snapshots().lock().unwrap().clone();
    fn visit(
        key: &str,
        snapshots: &HashMap<String, PluginContribSnapshot>,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if !seen.insert(key.to_string()) {
            return;
        }
        let mut children: Vec<_> = snapshots
            .iter()
            .filter(|(_, snapshot)| snapshot.dependencies.contains_key(key))
            .map(|(key, _)| key.clone())
            .collect();
        children.sort();
        for child in children {
            visit(&child, snapshots, seen, out);
        }
        out.push(key.to_string());
    }
    let mut out = Vec::new();
    visit(key, &snapshots, &mut HashSet::new(), &mut out);
    out
}
