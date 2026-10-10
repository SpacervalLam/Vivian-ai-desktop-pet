//! Optional artwork lives beside the executable, outside embedded frontend assets.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct PackSpec { pub id: String, pub label: String, pub version: String, pub files: Vec<String> }
pub fn specs() -> Vec<PackSpec> {
    serde_json::from_str(include_str!("../../../src/optional-resources.json")).expect("optional resource manifest")
}
fn installed_root(spec: &PackSpec, directory: PathBuf, development: bool) -> Option<PathBuf> {
    if !development {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(directory.join("pack.json")).ok()?).ok()?;
        if manifest["id"] != spec.id || manifest["version"] != spec.version { return None; }
        if !spec.files.iter().all(|file| manifest["files"][file].as_str().is_some()) { return None; }
    }
    spec.files.iter().all(|file| directory.join(file).metadata().is_ok_and(|m| m.is_file() && m.len() > 0)).then_some(directory)
}
pub fn pack_root(id: &str) -> Option<PathBuf> {
    let spec = specs().into_iter().find(|spec| spec.id == id)?;
    let directory = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public")
    } else {
        std::env::current_exe().ok()?.parent()?.join("optional").join(id)
    };
    installed_root(&spec, directory, cfg!(debug_assertions))
}
pub fn installed(id: &str) -> bool { pack_root(id).is_some() }
pub fn resource_path(id: &str, path: &str) -> Option<PathBuf> {
    let spec = specs().into_iter().find(|spec| spec.id == id)?;
    if !spec.files.iter().any(|file| file == path) { return None; }
    Some(pack_root(id)?.join(path))
}
#[derive(Serialize)]
pub struct PackStatus { id: String, label: String, installed: bool, root: Option<String> }
#[tauri::command]
pub fn optional_resource_status() -> Vec<PackStatus> {
    specs().into_iter().map(|spec| {
        let root = pack_root(&spec.id);
        PackStatus { id: spec.id, label: spec.label, installed: root.is_some(), root: root.map(|root| root.to_string_lossy().into_owned()) }
    }).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_and_wrong_version_packs_are_unavailable() {
        let folder = tempfile::tempdir().unwrap();
        let spec = PackSpec { id: "fonts".into(), label: "fonts".into(), version: "1.0.0".into(), files: vec!["font.woff2".into()] };
        assert!(installed_root(&spec, folder.path().to_path_buf(), false).is_none());
        std::fs::write(folder.path().join("pack.json"), r#"{"id":"fonts","version":"1.0.0","files":{"font.woff2":"hash"}}"#).unwrap();
        assert!(installed_root(&spec, folder.path().to_path_buf(), false).is_none());
        std::fs::write(folder.path().join("font.woff2"), b"font").unwrap();
        assert!(installed_root(&spec, folder.path().to_path_buf(), false).is_some());
        std::fs::write(folder.path().join("pack.json"), r#"{"id":"fonts","version":"2.0.0"}"#).unwrap();
        assert!(installed_root(&spec, folder.path().to_path_buf(), false).is_none());
        assert!(resource_path("fonts", "../../outside").is_none());
    }
}
