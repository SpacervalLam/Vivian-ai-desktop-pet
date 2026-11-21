//! Data-only character packages with immutable generations for live activation.
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct CharacterPackage {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub animations: BTreeMap<String, serde_json::Value>,
    #[serde(default = "master_cell")]
    pub cell_size: u32,
    /// New packs require authored left and right drawings; old residents remain loadable.
    #[serde(default)]
    pub independent_directions: bool,
    #[serde(default)]
    pub anchors: BTreeMap<String, Vec<[f64; 2]>>,
    #[serde(default)]
    pub scales: BTreeMap<String, Vec<[f64; 2]>>,
    #[serde(default)]
    pub presentation: serde_json::Value,
}
struct Loaded {
    manifest: CharacterPackage,
    root: PathBuf,
    texts: BTreeMap<String, String>,
}
fn master_cell() -> u32 {
    512
}
static ERRORS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// The compatibility surface that `animations: {}` residents were installed against.
/// Keep this list in sync with the pre-legacy `layouts()` fallback: adding an entry
/// would retroactively require artwork that old residents do not have.
const LEGACY_REQUIRED: [&str; 22] = [
    "float",
    "walk",
    "turn",
    "blink",
    "cast",
    "happy",
    "angry",
    "think",
    "smug",
    "sleep",
    "busy-in",
    "busy-loop",
    "wake",
    "remind",
    "tired",
    "umbrella",
    "music",
    "clipboard",
    "look",
    "edge",
    "lean",
    "edge-bottom",
];
/// Frozen copy of the vocabulary in effect before packages carried their own
/// `animations`. Never edit this file: a future breaking change gets a new contract.
static LEGACY: Lazy<serde_json::Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../src/chibi/animations.legacy-v1.json"))
        .expect("legacy animation contract is valid JSON")
});
fn legacy_specs() -> &'static [serde_json::Value] {
    LEGACY["animations"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}
fn spec_directions(spec: &serde_json::Value) -> Vec<&str> {
    spec["directions"]
        .as_array()
        .map(|d| d.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_else(|| vec!["left"])
}
/// Old residents have no per-package geometry. Hydrate them from the frozen
/// contract so validation and playback both use the drawings they were shipped with.
fn hydrate_legacy_animations(manifest: &mut CharacterPackage) {
    if !manifest.animations.is_empty() {
        return;
    }
    for spec in legacy_specs() {
        let Some(name) = spec["name"].as_str() else {
            continue;
        };
        if !LEGACY_REQUIRED.iter().any(|required| *required == name) {
            continue;
        }
        let mut concrete = spec.clone();
        if let Some(sheet) = spec["sheet"].as_str() {
            let mut sheet = sheet.replace("{character}", &manifest.id);
            // Old residents that declared independent directions shipped both
            // walk/turn drawings; keep validating and playing the right-side file
            // instead of silently mirroring the left one.
            if manifest.independent_directions && matches!(name, "walk" | "turn") {
                sheet = sheet.replace("-left-sheet.webp", "-{direction}-sheet.webp");
                concrete["mirrorRight"] = false.into();
            }
            concrete["sheet"] = sheet.into();
        }
        manifest.animations.insert(name.into(), concrete);
    }
}

pub fn report_error(error: String) {
    tracing::error!("{error}");
    let mut errors = ERRORS.lock().unwrap_or_else(|e| e.into_inner());
    if !errors.contains(&error) {
        errors.push(error);
    }
}
pub fn relationship_context(id: &str, available: impl Fn(&str) -> bool) -> String {
    let links: Vec<serde_json::Value> =
        serde_json::from_str(text(id, "relation_links.json")).unwrap_or_default();
    relationship_context_for(&links, available)
}
fn relationship_context_for(
    links: &[serde_json::Value],
    available: impl Fn(&str) -> bool,
) -> String {
    if links.is_empty() {
        return String::new();
    }
    let rows: Vec<_> = links
        .iter()
        .map(|link| {
            let id = link["id"].as_str().unwrap_or("");
            serde_json::json!({"id":id,"name":link["name"],"initialized":available(id)})
        })
        .collect();
    format!("[Authored relationship availability] {}\nAn authored relationship is background, never proof of residence or current presence. If initialized=false, the other person has not moved into this running household; do not address them as present, invent messages, request matchmaking, or load their files. If true, check actual public presence before interacting. Shared backstory grants no access to private conversations, memories or state.",serde_json::to_string(&rows).unwrap_or_default())
}
fn layouts(id: &str, independent_directions: bool, animations: &BTreeMap<String, serde_json::Value>) -> BTreeMap<String, (u32, u32, usize)> {
    let mut rows = BTreeMap::from([
        (format!("assets/chibi/{id}-atlas.webp"), (1536, 1024, 6)),
        (
            format!("assets/chibi/motion/{id}/{id}-idle-sheet.webp"),
            (512, 512, 1),
        ),
    ]);
    // A configured package owns its geometry, filenames and physical directions.
    // Older packages are interpreted against the frozen contract they were installed with,
    // never against the vocabulary that happens to be current at load time.
    let configured: Vec<serde_json::Value> = if animations.is_empty() {
        legacy_specs()
            .iter()
            .filter(|spec| {
                spec["name"]
                    .as_str()
                    .is_some_and(|name| LEGACY_REQUIRED.iter().any(|required| *required == name))
            })
            .cloned()
            .collect()
    } else {
        animations.values().cloned().collect()
    };
    for spec in &configured {
        let name = spec["name"].as_str().unwrap_or("");
        let directions = if animations.is_empty()
            && !(independent_directions && matches!(name, "walk" | "turn"))
        {
            vec!["left"]
        } else {
            spec_directions(spec)
        };
        for direction in directions {
            let mut url = spec["sheet"]
                .as_str()
                .unwrap()
                .replace("{character}", id)
                .replace("{direction}", direction);
            // The legacy contract only names the left drawing. A resident that
            // declared independent directions still promised a right drawing, so
            // validate that sibling path instead of silently accepting the left one.
            if animations.is_empty()
                && independent_directions
                && matches!(name, "walk" | "turn")
                && direction == "right"
            {
                url = url.replace("-left-sheet.webp", "-right-sheet.webp");
            }
            rows.insert(
                format!("assets{url}"),
                (
                    spec["cols"].as_u64().unwrap() as u32 * 512,
                    spec["rows"].as_u64().unwrap() as u32 * 512,
                    spec["frames"].as_u64().unwrap() as usize,
                ),
            );
        }
    }
    rows
}
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let kind = &bytes[at..at + 4];
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let d = bytes.get(at + 8..at + 8 + n)?;
        if kind == b"VP8X" && d.len() >= 10 {
            return Some((
                1 + u32::from_le_bytes([d[4], d[5], d[6], 0]),
                1 + u32::from_le_bytes([d[7], d[8], d[9], 0]),
            ));
        }
        if kind == b"VP8L" && d.len() >= 5 && d[0] == 0x2f {
            let bits = u32::from_le_bytes(d[1..5].try_into().ok()?);
            return Some((1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff)));
        }
        if kind == b"VP8 " && d.len() >= 10 && d[3..6] == [0x9d, 1, 0x2a] {
            return Some((
                u16::from_le_bytes([d[6], d[7]]) as u32 & 0x3fff,
                u16::from_le_bytes([d[8], d[9]]) as u32 & 0x3fff,
            ));
        }
        at = at.checked_add(8 + n + (n % 2))?;
    }
    None
}
pub fn root() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("character-packages")
}
pub fn asset_path(source: &str) -> Option<PathBuf> {
    let path = format!("assets/{}", source.trim_start_matches('/'));
    ACTIVE
        .read()
        .unwrap()
        .values()
        .find(|p| p.manifest.files.contains_key(&path))
        .map(|p| p.root.join(path))
}
pub fn contains(id: &str) -> bool {
    matches!(id, "vivian" | "nana") || ACTIVE.read().unwrap().contains_key(id)
}
fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .all(|v| !v.is_empty() && v != "." && v != "..")
}
fn validate(m: &CharacterPackage) -> Result<(), String> {
    if !matches!(m.cell_size, 320 | 512) {
        return Err("不支持的角色图集格位".into());
    }
    // `id` is the resident's immutable identity: directories, config, memory,
    // prompts and residency records all key on it. `name` is display metadata.
    if m.format != 1
        || m.id.len() > 48
        || m.id.is_empty()
        || !m
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || matches!(
            m.id.as_str(),
            "vivian" | "nana" | "user" | "system" | "all" | "i" | "me"
        )
    {
        return Err("角色包格式或角色标识无效；不能覆盖原始角色".into());
    }
    if m.name.trim().is_empty()
        || m.name.len() > 100
        || matches!(
            m.name.trim().to_lowercase().as_str(),
            "vivian" | "nana" | "user" | "system" | "all" | "i" | "me"
        )
        || m.version.trim().is_empty()
        || m.files.is_empty()
        || m.files.len() > 4096
    {
        return Err("角色包清单不完整或名称保留".into());
    }
    for (table, is_scale) in [(&m.anchors, false), (&m.scales, true)] {
        for (path, frames) in table {
            let name = Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if !path.starts_with("/chibi/")
                || !safe_path(path.trim_start_matches('/'))
                || !m.files.contains_key(&format!("assets{path}"))
                || !name.starts_with(&format!("{}-", m.id))
                || frames.len() > 256
                || frames.iter().any(|f| {
                    f.iter().any(|n| !n.is_finite() || n.abs() > 1000.0)
                        || (is_scale && !(0.05..=5.0).contains(&f[0]))
                })
            {
                return Err("角色包的逐帧定位数据无效".into());
            }
        }
    }
    for (path, hash) in &m.files {
        let allowed = path.starts_with("prompts/") || path.starts_with("assets/");
        let ext = Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !safe_path(path)
            || !allowed
            || !matches!(
                ext,
                "md" | "json" | "png" | "webp" | "jpg" | "svg" | "ogg" | "wav" | "mp3"
            )
            || hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("角色包文件无效: {path}"));
        }
        if path.starts_with("assets/") {
            let filename = Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if !filename.starts_with(&format!("{}-", m.id))
                && !filename.starts_with(&format!("{}_", m.id))
                && !filename.starts_with(&format!("{}.", m.id))
            {
                return Err("角色包只能提供本角色的资源".into());
            }
        }
    }
    for path in [
        format!("assets/chibi/{}-atlas.webp", m.id),
        "prompts/identity.md".into(),
        "prompts/personality.md".into(),
    ] {
        if !m.files.contains_key(&path) {
            return Err(format!("角色包缺少 {path}"));
        }
    }
    for (name, spec) in &m.animations {
        let cols = spec["cols"].as_u64().unwrap_or(0);
        let rows = spec["rows"].as_u64().unwrap_or(0);
        let frames = spec["frames"].as_u64().unwrap_or(0);
        let sheet = spec["sheet"].as_str().unwrap_or("");
        let durations = spec["durations"].as_array();
        if spec["name"].as_str() != Some(name.as_str())
            || !(1..=16).contains(&cols) || !(1..=16).contains(&rows)
            || frames == 0 || frames > cols * rows || frames > 256
            || durations.map(|d| d.len()) != Some(frames as usize)
            || durations.unwrap().iter().any(|d| d.as_u64().map_or(true, |n| n == 0 || n > 60_000))
            || !spec["loop"].is_boolean()
            || !(spec["directions"].is_null() || spec["directions"].as_array().map_or(false, |d| !d.is_empty() && d.iter().all(|v| matches!(v.as_str(), Some("left" | "right")))))
            || !sheet.starts_with("/chibi/")
            || !m.files.contains_key(&format!("assets{}", sheet.replace("{direction}", "left")))
        {
            return Err(format!("角色包动作配置无效: {name}"));
        }
    }
    for (path, (_, _, frames)) in layouts(&m.id, m.independent_directions, &m.animations) {
        if !m.files.contains_key(&path) {
            return Err(format!("角色包缺少动作资源: {path}"));
        }
        let url = path.trim_start_matches("assets");
        if m.anchors.get(url).map(Vec::len) != Some(frames)
            || m.scales.get(url).map(Vec::len) != Some(frames)
        {
            return Err(format!("角色包缺少完整逐帧定位: {path}"));
        }
    }
    for section in [
        "identity",
        "personality",
        "appearance",
        "background",
        "interests",
        "speech",
        "relationships",
        "canon_quotes",
        "examples",
        "persona_config",
        "scenes",
        "environment_presets",
    ] {
        if !m.files.contains_key(&format!("prompts/{section}.md")) {
            return Err(format!("角色包缺少人设段落: {section}"));
        }
    }
    if !m
        .files
        .contains_key(&format!("assets/icons/charaters/{}.webp", m.id))
    {
        return Err("角色包缺少独立头像".into());
    }
    for kind in ["defaultSize", "bottomDock"] {
        let value = &m.presentation[kind];
        if !value["scale"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && (0.05..=5.0).contains(&n))
            || !value["translateY"]
                .as_f64()
                .is_some_and(|n| n.is_finite() && n.abs() <= 1000.0)
        {
            return Err(format!("角色显示校准不完整: {kind}"));
        }
    }
    Ok(())
}
fn read_manifest(directory: &Path) -> Result<CharacterPackage, String> {
    let bytes =
        std::fs::read(directory.join("character.json")).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn load(directory: PathBuf) -> Result<Loaded, String> {
    let mut manifest = read_manifest(&directory)?;
    hydrate_legacy_animations(&mut manifest);
    validate(&manifest)?;
    let mut texts = BTreeMap::<String, String>::new();
    let expected_layouts = layouts(&manifest.id, manifest.independent_directions, &manifest.animations);
    for (path, hash) in &manifest.files {
        let bytes = std::fs::read(directory.join(path)).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != hash.to_lowercase() {
            return Err(format!("角色包校验失败: {path}"));
        }
        if let Some((w, h, _)) = expected_layouts.get(path) {
            if webp_size(&bytes)
                != Some((*w / 512 * manifest.cell_size, *h / 512 * manifest.cell_size))
            {
                return Err(format!("动作图集尺寸或格式不正确: {path}"));
            }
        }
        if let Some(name) = path.strip_prefix("prompts/") {
            texts.insert(
                name.into(),
                String::from_utf8(bytes).map_err(|e| e.to_string())?,
            );
        }
    }
    for (file, value) in &texts {
        if file.ends_with(".json") {
            let parsed: serde_json::Value =
                serde_json::from_str(value).map_err(|e| format!("人设数据 {file} 无效: {e}"))?;
            if file == "persona.json"
                && parsed["identity"]["name"].as_str() != Some(manifest.name.as_str())
            {
                return Err("角色身份不能覆盖其他角色".into());
            }
            if file == "relation_links.json" {
                let links = parsed.as_array().ok_or("关系链接必须是数组")?;
                for link in links {
                    let target = link["id"].as_str().ok_or("关系缺少目标")?;
                    if target == manifest.id
                        || target.is_empty()
                        || !target
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    {
                        return Err("关系链接无效".into());
                    }
                }
            }
        }
    }
    for file in [
        "traits.json",
        "behavior.json",
        "flags.json",
        "stickers.json",
    ] {
        if !texts.contains_key(file) {
            return Err(format!("缺少角色独立参数: {file}"));
        }
    }
    let traits: serde_json::Value =
        serde_json::from_str(&texts["traits.json"]).map_err(|e| e.to_string())?;
    for key in [
        "tsundere",
        "clingy",
        "genki",
        "sass",
        "healing",
        "curiosity",
        "ritual",
        "habit_awareness",
    ] {
        if !traits[key]
            .as_f64()
            .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n))
        {
            return Err(format!("角色表达参数缺失或超界: {key}"));
        }
    }
    serde_json::from_str::<crate::character_behavior::CharacterBehavior>(&texts["behavior.json"])
        .map_err(|e| format!("角色行为参数不完整: {e}"))?;
    serde_json::from_str::<Vec<String>>(&texts["flags.json"])
        .map_err(|e| format!("角色标签不完整: {e}"))?;
    let stickers: Vec<serde_json::Value> =
        serde_json::from_str(&texts["stickers.json"]).map_err(|e| format!("贴纸目录无效: {e}"))?;
    for sticker in stickers {
        let file = sticker["filename"].as_str().ok_or("贴纸缺少文件名")?;
        if sticker["character_id"].as_str() != Some(manifest.id.as_str())
            || !manifest
                .files
                .contains_key(&format!("assets/stickers/{file}"))
        {
            return Err("贴纸引用了其他角色或缺失图片".into());
        }
    }
    if let Some(file) = manifest.presentation["diaryArt"].as_str() {
        if !manifest
            .files
            .contains_key(&format!("assets/record-art/{file}"))
        {
            return Err("日记背景图片缺失".into());
        }
    }
    Ok(Loaded {
        manifest,
        root: directory,
        texts,
    })
}
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn apply_pending(base: &Path) {
    // A crash after moving the old package is recoverable from its rollback slot.
    let rollback = base.join(".rollback");
    if let Ok(dirs) = std::fs::read_dir(&rollback) {
        for d in dirs.flatten() {
            let id = d.file_name();
            let target = base.join(&id);
            if !target.exists() {
                let _ = std::fs::rename(d.path(), target);
            }
        }
    }
    if let Ok(dirs) = std::fs::read_dir(base.join(".pending")) {
        for d in dirs.flatten() {
            let next = match load(d.path()) {
                Ok(p) => p,
                Err(e) => {
                    report_error(format!("暂存角色更新无效: {e}"));
                    continue;
                }
            };
            if d.file_name().to_string_lossy() != next.manifest.id {
                continue;
            }
            let target = base.join(&next.manifest.id);
            let backup = rollback.join(&next.manifest.id);
            if std::fs::create_dir_all(&rollback).is_err() {
                continue;
            }
            if backup.exists() && target.exists() {
                if std::fs::remove_dir_all(&backup).is_err() {
                    continue;
                }
            }
            if target.exists() && std::fs::rename(&target, &backup).is_err() {
                continue;
            }
            if let Err(error) = std::fs::rename(d.path(), &target) {
                if backup.exists() {
                    let _ = std::fs::rename(&backup, &target);
                }
                tracing::error!("角色包更新提交失败: {error}");
            } else {
                let _ = std::fs::remove_file(base.join(format!(".{}.active", next.manifest.id)));
                if backup.exists() {
                    let _ = std::fs::remove_dir_all(&backup);
                }
            }
        }
    }
}
fn snapshot_from(base: &Path) -> BTreeMap<String, Loaded> {
    apply_pending(base);
    let mut entries = BTreeMap::new();
    if let Ok(dirs) = std::fs::read_dir(base) {
        for dir in dirs.flatten() {
            if !dir.file_type().is_ok_and(|t| t.is_dir())
                || dir.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            // Legacy removal markers are ignored: residents cannot be uninstalled.
            let pointer = base.join(format!(".{}.active", dir.file_name().to_string_lossy()));
            let directory = std::fs::read_to_string(&pointer)
                .ok()
                .map(PathBuf::from)
                .filter(|p| p.starts_with(base.join(".versions")) && p.is_dir())
                .unwrap_or_else(|| dir.path());
            match load(directory) {
                Ok(p) if dir.file_name().to_string_lossy() == p.manifest.id => {
                    entries.insert(p.manifest.id.clone(), p);
                }
                Ok(_) => report_error(format!("角色包目录与标识不一致: {}", dir.path().display())),
                Err(e) => report_error(format!(
                    "角色包装载失败（{}）: {e}",
                    dir.file_name().to_string_lossy()
                )),
            }
        }
    }
    entries
}
// Generations remain immutable while in-flight prompts hold borrowed text.
static ACTIVE: Lazy<std::sync::RwLock<BTreeMap<String, &'static Loaded>>> = Lazy::new(|| {
    std::sync::RwLock::new(
        snapshot_from(&root())
            .into_iter()
            .map(|(id, p)| (id, Box::leak(Box::new(p)) as &'static Loaded))
            .collect(),
    )
});
pub fn active() -> Vec<CharacterPackage> {
    ACTIVE
        .read()
        .unwrap()
        .values()
        .map(|p| p.manifest.clone())
        .collect()
}
pub fn text(id: &str, file: &str) -> &'static str {
    let loaded = ACTIVE.read().unwrap().get(id).copied();
    loaded
        .and_then(|p| p.texts.get(file))
        .map(String::as_str)
        .unwrap_or("")
}
pub fn activate(id: &str) -> Result<bool, String> {
    let _ = &*ACTIVE;
    let _guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let pending = root().join(".pending").join(id);
    let directory = if pending.is_dir() {
        load(pending.clone())?;
        let versions = root().join(".versions");
        std::fs::create_dir_all(&versions).map_err(|e| e.to_string())?;
        let path = versions.join(format!("{id}-{}", uuid::Uuid::new_v4()));
        std::fs::rename(&pending, &path).map_err(|e| e.to_string())?;
        path
    } else {
        if active_root(id).is_some() {
            return Ok(false);
        }
        root().join(id)
    };
    let loaded = load(directory.clone())?;
    if loaded.manifest.id != id {
        return Err("角色标识不匹配".into());
    }
    crate::utils::fs::write_atomic(
        &root().join(format!(".{id}.active")),
        &directory.to_string_lossy(),
    )
    .map_err(|e| e.to_string())?;
    ACTIVE
        .write()
        .unwrap()
        .insert(id.into(), Box::leak(Box::new(loaded)));
    Ok(true)
}
pub fn active_root(id: &str) -> Option<PathBuf> {
    ACTIVE.read().unwrap().get(id).map(|p| p.root.clone())
}
pub fn restore_active(id: &str, previous: Option<PathBuf>) -> Result<(), String> {
    let _guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = previous {
        let loaded = load(path.clone())?;
        crate::utils::fs::write_atomic(
            &root().join(format!(".{id}.active")),
            &path.to_string_lossy(),
        )
        .map_err(|e| e.to_string())?;
        ACTIVE
            .write()
            .unwrap()
            .insert(id.into(), Box::leak(Box::new(loaded)));
    } else {
        ACTIVE.write().unwrap().remove(id);
        let _ = std::fs::remove_file(root().join(format!(".{id}.active")));
    }
    Ok(())
}
pub fn localized(id: &str, key: &str, lang: &str) -> &'static str {
    let value = text(id, &format!("{key}.{lang}.md"));
    if value.is_empty() {
        text(id, &format!("{key}.md"))
    } else {
        value
    }
}
pub fn id_for_name(name: &str) -> String {
    ACTIVE
        .read()
        .unwrap()
        .values()
        .find(|p| p.manifest.name.eq_ignore_ascii_case(name))
        .map(|p| p.manifest.id.clone())
        .unwrap_or_else(|| name.to_lowercase())
}
pub fn section(name: &str, key: &str, lang: &str) -> Option<&'static str> {
    if matches!(name, "Vivian" | "Nana") {
        return None;
    }
    let p = ACTIVE
        .read()
        .unwrap()
        .values()
        .copied()
        .find(|p| p.manifest.name == name)?;
    let localized = format!("{key}.{lang}.md");
    p.texts
        .get(&localized)
        .or_else(|| p.texts.get(&format!("{key}.md")))
        .map(String::as_str)
}
pub fn active_rows() -> serde_json::Value {
    serde_json::json!(ACTIVE
        .read()
        .unwrap()
        .values()
        .map(|p| serde_json::json!({"manifest":p.manifest,"root":p.root}))
        .collect::<Vec<_>>())
}
fn package_status_row(package: &Loaded) -> serde_json::Value {
    let mut value = serde_json::to_value(&package.manifest).unwrap();
    let id = &package.manifest.id;
    value["atlas_path"] = package
        .root
        .join(format!("assets/chibi/{id}-atlas.webp"))
        .to_string_lossy()
        .into_owned()
        .into();
    let avatar = format!("assets/icons/charaters/{id}.webp");
    if package.manifest.files.contains_key(&avatar) {
        value["avatar_path"] = package
            .root
            .join(avatar)
            .to_string_lossy()
            .into_owned()
            .into();
    }
    value
}

pub fn character_package_status() -> serde_json::Value {
    let active = active_rows(); // Freeze before reporting a newly imported package.
    let installed = std::fs::read_dir(root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| !d.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|d| load(d.path()).ok())
        .map(|p| {
            let live = ACTIVE.read().unwrap().get(&p.manifest.id).copied();
            let p = live.unwrap_or(&p);
            let mut value = package_status_row(p);
            value["pending_update"] = root().join(".pending").join(&p.manifest.id).is_dir().into();
            if value["pending_update"] == true {
                if let Ok(next) = load(root().join(".pending").join(&p.manifest.id)) {
                    let mut next = package_status_row(&next);
                    next["pending_update"] = true.into();
                    return next;
                }
            }
            value
        })
        .collect::<Vec<_>>();
    serde_json::json!({"active":active,"installed":installed,"errors":ERRORS.lock().unwrap_or_else(|e|e.into_inner()).clone()})
}
#[cfg(test)]
fn install_into(source: &Path, base: &Path) -> Result<CharacterPackage, String> {
    install_into_with_retry(source, base, false)
}
fn install_into_with_retry(
    source: &Path,
    base: &Path,
    retry_identical: bool,
) -> Result<CharacterPackage, String> {
    let _ = &*ACTIVE;
    let _guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let input = std::fs::File::open(source).map_err(|e| e.to_string())?;
    if input.metadata().map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
        return Err("角色包超过 512 MiB".into());
    }
    let mut zip = zip::ZipArchive::new(input).map_err(|e| e.to_string())?;
    if zip.len() > 4097 {
        return Err("角色包文件过多".into());
    }
    let manifest: CharacterPackage = {
        let file = zip
            .by_name("character.json")
            .map_err(|_| "缺少 character.json")?;
        let mut text = String::new();
        file.take(1024 * 1024)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())?
    };
    validate(&manifest)?;
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let stage = base.join(format!(".stage-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = file.name().to_owned();
            if !safe_path(&name)
                || !seen.insert(name.clone())
                || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            {
                return Err("角色包包含不安全或重复路径".into());
            }
            if name == "character.json" {
                continue;
            }
            let hash = manifest.files.get(&name).ok_or("角色包存在未声明文件")?;
            total = total.checked_add(file.size()).ok_or("角色包大小溢出")?;
            if total > 1024 * 1024 * 1024 || file.size() > 128 * 1024 * 1024 {
                return Err("角色包解压体积过大".into());
            }
            let mut bytes = Vec::new();
            (&mut file)
                .take(128 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 128 * 1024 * 1024
                || format!("{:x}", Sha256::digest(&bytes)) != hash.to_lowercase()
            {
                return Err(format!("角色包校验失败: {name}"));
            }
            let target = stage.join(name);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(target, bytes).map_err(|e| e.to_string())?;
        }
        if manifest.files.keys().any(|p| !seen.contains(p)) {
            return Err("角色包文件不完整".into());
        }
        std::fs::write(
            stage.join("character.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        load(stage.clone())?;
        let target = base.join(&manifest.id);
        if target.exists() {
            let live = ACTIVE
                .read()
                .unwrap()
                .get(&manifest.id)
                .map(|p| p.manifest.clone());
            let loaded = load(target.clone());
            // Identity is the package id. Reading the name must not depend on the old
            // resident passing today's validation: a broken resident still has to be
            // replaceable by an update with the same id and name.
            let previous = live.or_else(|| loaded.as_ref().ok().map(|p| p.manifest.clone()));
            let previous_name = match previous.as_ref() {
                Some(p) => p.name.clone(),
                None => read_manifest(&target)
                    .map_err(|e| format!("已有角色包元信息损坏，无法校验更新: {e}"))?
                    .name,
            };
            if previous_name != manifest.name {
                return Err("更新不能改变角色名称".into());
            }
            // Compare normalized forms: an old resident is hydrated in memory while
            // an old-format ZIP arrives with an empty animations map.
            let mut normalized = manifest.clone();
            hydrate_legacy_animations(&mut normalized);
            let identical = loaded.is_ok()
                && previous.as_ref().is_some_and(|p| {
                    serde_json::to_value(p).ok() == serde_json::to_value(&normalized).ok()
                });
            if identical {
                if retry_identical || (base == root() && active_root(&manifest.id).is_none()) {
                    return Ok(manifest);
                }
                return Err("相同角色包已安装".into());
            }
            let pending = base.join(".pending");
            std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
            let destination = pending.join(&manifest.id);
            if destination.exists() {
                return Err("此角色已有更新正在等待装载，请稍后再试".into());
            }
            std::fs::rename(&stage, &destination).map_err(|e| e.to_string())?;
            return Ok(manifest);
        }
        if std::fs::read_dir(base)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|d| !d.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|d| read_manifest(&d.path()).ok())
            .any(|p| p.name.eq_ignore_ascii_case(&manifest.name))
        {
            return Err("此角色名称已被使用".into());
        }
        std::fs::rename(&stage, &target).map_err(|e| {
            format!(
                "提交角色包 {} → {} 失败: {e}",
                stage.display(),
                target.display()
            )
        })?;
        Ok(manifest)
    })();
    if stage.exists() {
        let _ = std::fs::remove_dir_all(stage);
    }
    result
}
pub fn import_character_package(source: String) -> Result<CharacterPackage, String> {
    // Retrying a validated package must also work after runtime initialization failed.
    install_into_with_retry(Path::new(&source), &root(), true)
}
/// Uninstall is deliberately unsupported, including calls from older frontends.
pub fn remove_character_package(_id: String) -> Result<(), String> {
    Err("角色入住后不允许卸载".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn temp() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("package-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    fn archive(path: &Path, extra: Option<&str>, corrupt: bool) {
        archive_named(path, "test", "Test", "1.0.0", extra, corrupt);
    }
    fn archive_named(
        path: &Path,
        id: &str,
        name: &str,
        version: &str,
        extra: Option<&str>,
        corrupt: bool,
    ) {
        let mut files = BTreeMap::<String, Vec<u8>>::new();
        let shape = layouts(id, false, &BTreeMap::new());
        let mut anchors = BTreeMap::new();
        let mut scales = BTreeMap::new();
        for (path, (w, h, n)) in &shape {
            let mut bytes = b"RIFF".to_vec();
            bytes.extend(22u32.to_le_bytes());
            bytes.extend(b"WEBPVP8X");
            bytes.extend(10u32.to_le_bytes());
            bytes.extend([0u8; 4]);
            bytes.extend(&(*w - 1).to_le_bytes()[..3]);
            bytes.extend(&(*h - 1).to_le_bytes()[..3]);
            files.insert(path.clone(), bytes);
            let url = path.trim_start_matches("assets").to_string();
            anchors.insert(url.clone(), vec![[0.0, 0.0]; *n]);
            scales.insert(url, vec![[1.0, 90.0]; *n]);
        }
        for section in [
            "identity",
            "personality",
            "appearance",
            "background",
            "interests",
            "speech",
            "relationships",
            "canon_quotes",
            "examples",
            "persona_config",
            "scenes",
            "environment_presets",
        ] {
            files.insert(format!("prompts/{section}.md"), b"Test companion".to_vec());
        }
        files.insert(
            format!("assets/icons/charaters/{id}.webp"),
            b"avatar".to_vec(),
        );
        files.insert(
            "prompts/traits.json".into(),
            include_bytes!("../../character-packs/chatgpt/prompts/traits.json").to_vec(),
        );
        files.insert(
            "prompts/behavior.json".into(),
            include_bytes!("../../character-packs/chatgpt/prompts/behavior.json").to_vec(),
        );
        files.insert("prompts/flags.json".into(), b"[]".to_vec());
        files.insert("prompts/stickers.json".into(), b"[]".to_vec());
        let manifest = CharacterPackage {
            animations: BTreeMap::new(),
            format: 1,
            cell_size: 512,
            independent_directions: false,
            id: id.into(),
            name: name.into(),
            version: version.into(),
            files: files
                .iter()
                .map(|(n, b)| (n.clone(), format!("{:x}", Sha256::digest(b))))
                .collect(),
            anchors,
            scales,
            presentation: serde_json::json!({"defaultSize":{"scale":1,"translateY":0},"bottomDock":{"scale":1,"translateY":0}}),
        };
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("character.json", options).unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        for (name, bytes) in files {
            zip.start_file(name, options).unwrap();
            zip.write_all(if corrupt { b"wrong" } else { &bytes })
                .unwrap();
        }
        if let Some(extra) = extra {
            zip.start_file(extra, options).unwrap();
            zip.write_all(b"unexpected").unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn legacy_residents_are_hydrated_from_the_frozen_contract() {
        let temp = temp();
        let zip = temp.path().join("test.zip");
        let base = temp.path().join("installed");
        archive(&zip, None, false);
        let installed = install_into(&zip, &base).unwrap();
        assert!(
            installed.animations.is_empty(),
            "the authored ZIP manifest stays free of derived data"
        );
        let loaded = load(base.join("test")).unwrap();
        assert_eq!(loaded.manifest.animations.len(), LEGACY_REQUIRED.len());
        assert_eq!(loaded.manifest.animations["blink"]["frames"], 16);
        assert_eq!(loaded.manifest.animations["blink"]["rows"], 4);
        assert_eq!(loaded.manifest.animations["turn"]["frames"], 16);
        let layout = layouts("test", false, &BTreeMap::new());
        assert_eq!(
            layout["assets/chibi/motion/test/test-blink-sheet.webp"],
            (2048, 2048, 16)
        );
    }
    #[test]
    fn same_name_legacy_resident_can_be_updated() {
        let temp = temp();
        let base = temp.path().join("installed");
        let old = temp.path().join("old.zip");
        archive_named(&old, "test", "Test", "1.0.0", None, false);
        install_into(&old, &base).unwrap();
        let next = temp.path().join("next.zip");
        archive_named(&next, "test", "Test", "1.0.1", None, false);
        install_into(&next, &base).unwrap();
        assert!(base.join(".pending/test").is_dir());
        assert_eq!(
            load(base.join("test")).unwrap().manifest.version,
            "1.0.0"
        );
        assert_eq!(
            load(base.join(".pending/test")).unwrap().manifest.version,
            "1.0.1"
        );
    }
    #[test]
    fn a_broken_same_name_resident_is_still_replaceable() {
        let temp = temp();
        let base = temp.path().join("installed");
        let old = temp.path().join("old.zip");
        archive_named(&old, "test", "Test", "1.0.0", None, false);
        install_into(&old, &base).unwrap();
        let mut raw = read_manifest(&base.join("test")).unwrap();
        raw.anchors
            .get_mut("/chibi/motion/test/test-blink-sheet.webp")
            .unwrap()
            .pop();
        std::fs::write(
            base.join("test/character.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        assert!(load(base.join("test")).is_err());
        let next = temp.path().join("next.zip");
        archive_named(&next, "test", "Test", "1.0.1", None, false);
        install_into(&next, &base).unwrap();
        assert!(base.join(".pending/test").is_dir());
        assert!(load(base.join(".pending/test")).is_ok());
    }
    #[test]
    fn id_is_the_identity_and_display_name_cannot_take_over_another_resident() {
        let temp = temp();
        let base = temp.path().join("installed");
        let old = temp.path().join("old.zip");
        archive_named(&old, "test", "Test", "1.0.0", None, false);
        install_into(&old, &base).unwrap();
        let renamed = temp.path().join("renamed.zip");
        archive_named(&renamed, "test", "Renamed", "1.0.1", None, false);
        assert!(install_into(&renamed, &base)
            .err()
            .unwrap()
            .contains("更新不能改变角色名称"));
        let other = temp.path().join("other.zip");
        archive_named(&other, "other", "Test", "1.0.0", None, false);
        assert!(install_into(&other, &base)
            .err()
            .unwrap()
            .contains("此角色名称已被使用"));
        assert_eq!(
            std::fs::read_dir(&base)
                .unwrap()
                .flatten()
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .count(),
            1,
            "one id maps to exactly one resident directory"
        );
    }
    #[test]
    fn round_trip_and_duplicate_install_preserves_existing_files() {
        let temp = temp();
        let zip = temp.path().join("test.zip");
        let output = temp.path().join("installed");
        archive(&zip, None, false);
        let before = active().len();
        let running = snapshot_from(&output);
        assert_eq!(install_into(&zip, &output).unwrap().id, "test");
        assert_eq!(
            load(output.join("test")).unwrap().texts["identity.md"],
            "Test companion"
        );
        assert!(install_into(&zip, &output).is_err());
        assert_eq!(
            active().len(),
            before,
            "import does not change the active restart snapshot"
        );
        assert!(
            running.is_empty(),
            "the running snapshot remains unchanged after import"
        );
        assert!(
            snapshot_from(&output).contains_key("test"),
            "a new startup discovers the import"
        );
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
        std::fs::write(output.join("test/.remove-on-restart"), b"").unwrap();
        assert!(
            output.join("test/assets/chibi/test-atlas.webp").is_file(),
            "pending removal retains live assets"
        );
        assert!(snapshot_from(&output).contains_key("test"));
        assert!(remove_character_package("test".into()).is_err());
        assert!(
            output.join("test").exists(),
            "legacy removal markers never evict a resident"
        );
    }
    #[test]
    fn rejects_traversal_undeclared_files_and_corrupt_payloads_without_partial_install() {
        for (extra, corrupt) in [
            (Some("../escape"), false),
            (Some("assets/chibi/test-extra.webp"), false),
            (None, true),
        ] {
            let temp = temp();
            let zip = temp.path().join("test.zip");
            let output = temp.path().join("installed");
            archive(&zip, extra, corrupt);
            assert!(install_into(&zip, &output).is_err());
            assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
            assert!(!temp.path().join("escape").exists());
        }
    }
    #[test]
    fn original_character_and_foreign_asset_names_are_reserved() {
        let temp = temp();
        let zip = temp.path().join("test.zip");
        let output = temp.path().join("installed");
        archive(&zip, None, false);
        let mut manifest = install_into(&zip, &output).unwrap();
        manifest.id = "vivian".into();
        assert!(validate(&manifest).is_err());
        manifest.id = "test".into();
        manifest
            .files
            .insert("assets/chibi/nana-atlas.webp".into(), "0".repeat(64));
        assert!(validate(&manifest).is_err());
    }
    #[test]
    fn updates_wait_for_restart_and_keep_personal_data_and_live_resources() {
        let temp = temp();
        let path = temp.path().join("test.zip");
        let base = temp.path().join("installed");
        archive(&path, None, false);
        install_into(&path, &base).unwrap();
        let running = snapshot_from(&base);
        let personal = temp.path().join("characters/test/private-memory.txt");
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::write(&personal, b"private").unwrap();
        let input = std::fs::File::open(&path).unwrap();
        let mut zip = zip::ZipArchive::new(input).unwrap();
        let mut rows = Vec::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            let name = f.name().to_string();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).unwrap();
            if name == "character.json" {
                let mut m: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                m["version"] = "1.0.1".into();
                bytes = serde_json::to_vec(&m).unwrap();
            }
            rows.push((name, bytes));
        }
        drop(zip);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in rows {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        install_into(&path, &base).unwrap();
        assert_eq!(running["test"].manifest.version, "1.0.0");
        assert_eq!(load(base.join("test")).unwrap().manifest.version, "1.0.0");
        assert_eq!(
            load(base.join(".pending/test")).unwrap().manifest.version,
            "1.0.1"
        );
        assert_eq!(snapshot_from(&base)["test"].manifest.version, "1.0.1");
        assert_eq!(std::fs::read(&personal).unwrap(), b"private");
        archive(&path, None, true);
        assert!(install_into(&path, &base).is_err());
        assert_eq!(load(base.join("test")).unwrap().manifest.version, "1.0.1");
    }
    #[test]
    fn missing_frames_wrong_grids_and_missing_soft_relations_are_explicit() {
        let temp = temp();
        let path = temp.path().join("test.zip");
        let base = temp.path().join("installed");
        archive(&path, None, false);
        let mut m = install_into(&path, &base).unwrap();
        let asset = "assets/chibi/test-atlas.webp";
        let mut data = std::fs::read(base.join("test").join(asset)).unwrap();
        data[24] = 0;
        std::fs::write(base.join("test").join(asset), &data).unwrap();
        m.files
            .insert(asset.into(), format!("{:x}", Sha256::digest(&data)));
        std::fs::write(
            base.join("test/character.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        assert!(load(base.join("test")).is_err());
        let mut m = m.clone();
        m.anchors.remove("/chibi/test-atlas.webp");
        assert!(validate(&m).is_err());
        let links = vec![serde_json::json!({"id":"claude","name":"Claude"})];
        assert!(relationship_context_for(&links, |_| false).contains("\"initialized\":false"));
        assert!(relationship_context_for(&links, |_| true).contains("\"initialized\":true"));
    }
    #[test]
    fn live_generations_preserve_borrows_and_can_roll_back_without_evicting_residents() {
        let tmp = temp();
        let archive_path = tmp.path().join("test.zip");
        archive(&archive_path, None, false);
        let base = root();
        install_into(&archive_path, &base).unwrap();
        activate("test").unwrap();
        let original = active_root("test").unwrap();
        import_character_package(archive_path.to_string_lossy().into_owned()).unwrap();
        assert!(!activate("test").unwrap(), "identical imports preserve the active generation");
        assert_eq!(active_root("test").unwrap(), original);
        let borrowed = text("test", "identity.md");
        let old_asset = asset_path("/chibi/test-atlas.webp").unwrap();
        let pending = base.join(".pending/test");
        std::fs::create_dir_all(&pending).unwrap();
        let mut manifest = load(original.clone()).unwrap().manifest;
        for file in manifest.files.keys() {
            let path = pending.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::copy(original.join(file), path).unwrap();
        }
        manifest.version = "1.0.1".into();
        let content = "Updated independent identity";
        std::fs::write(pending.join("prompts/identity.md"), content).unwrap();
        manifest.files.insert(
            "prompts/identity.md".into(),
            format!("{:x}", Sha256::digest(content)),
        );
        std::fs::write(
            pending.join("character.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(text("test", "identity.md"), "Test companion");
        activate("test").unwrap();
        assert_eq!(text("test", "identity.md"), content);
        assert_eq!(borrowed, "Test companion");
        assert!(old_asset.is_file());
        assert_eq!(
            snapshot_from(&base)["test"].manifest.version,
            "1.0.1",
            "restart keeps the same hot-loaded generation"
        );
        restore_active("test", Some(original)).unwrap();
        assert_eq!(text("test", "identity.md"), "Test companion");
        assert_eq!(snapshot_from(&base)["test"].manifest.version, "1.0.0");
        std::fs::create_dir_all(&pending).unwrap();
        std::fs::write(
            pending.join("character.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            activate("test").is_err(),
            "an incomplete update cannot replace a resident"
        );
        assert_eq!(text("test", "identity.md"), "Test companion");
        assert!(old_asset.is_file());
        restore_active("test", None).unwrap();
    }
    #[test]
    fn package_animation_layout_and_timing_are_authoritative() {
        let temp = temp();
        let source = temp.path().join("test.zip");
        archive(&source, None, false);
        let base = temp.path().join("installed");
        let mut manifest = install_into(&source, &base).unwrap();
        let sheet = "/chibi/motion/test/test-blink-sheet.webp";
        manifest.animations.insert("blink".into(), serde_json::json!({
            "name":"blink", "sheet":sheet, "cols":2, "rows":1,
            "frames":2, "durations":[70,90], "loop":false, "directions":null
        }));
        manifest.anchors.insert(sheet.into(), vec![[0.0,0.0];2]);
        manifest.scales.insert(sheet.into(), vec![[1.0,90.0];2]);
        validate(&manifest).unwrap();
        assert_eq!(layouts(&manifest.id, false, &manifest.animations)[&format!("assets{sheet}")], (1024,512,2));
        let restored: CharacterPackage = serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
        assert_eq!(restored.animations["blink"]["durations"], serde_json::json!([70,90]));
        manifest.animations.get_mut("blink").unwrap()["durations"] = serde_json::json!([70]);
        assert!(validate(&manifest).unwrap_err().contains("动作配置无效"));
    }
    #[test]
    fn independently_authored_directions_require_both_sheets_and_complete_calibration() {
        let temp = temp();
        let source = temp.path().join("test.zip");
        archive(&source, None, false);
        let base = temp.path().join("installed");
        let mut manifest = install_into(&source, &base).unwrap();
        // Legacy residents must remain readable before their artwork update arrives.
        let mut legacy = serde_json::to_value(&manifest).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("independent_directions");
        assert!(
            !serde_json::from_value::<CharacterPackage>(legacy)
                .unwrap()
                .independent_directions
        );
        manifest.independent_directions = true;
        assert!(
            validate(&manifest).is_err(),
            "declared independent directions cannot omit right-side drawings"
        );
        for kind in ["turn", "walk"] {
            let folder = if kind == "turn" { "motion" } else { "walk" };
            let left = format!("/chibi/{folder}/test/test-{kind}-left-sheet.webp");
            let right = left.replace("-left-", "-right-");
            let bytes =
                std::fs::read(base.join("test/assets").join(left.trim_start_matches('/'))).unwrap();
            std::fs::write(
                base.join("test/assets").join(right.trim_start_matches('/')),
                &bytes,
            )
            .unwrap();
            manifest.files.insert(
                format!("assets{right}"),
                format!("{:x}", Sha256::digest(&bytes)),
            );
            manifest
                .anchors
                .insert(right.clone(), manifest.anchors[&left].clone());
            manifest
                .scales
                .insert(right, manifest.scales[&left].clone());
        }
        validate(&manifest).unwrap();
        std::fs::write(
            base.join("test/character.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let loaded = load(base.join("test")).unwrap();
        assert_eq!(
            loaded.manifest.animations["turn"]["sheet"],
            "/chibi/motion/test/test-turn-{direction}-sheet.webp",
            "independent directions keep validating and playing the right drawing"
        );
        assert_eq!(loaded.manifest.animations["turn"]["mirrorRight"], false);
        manifest
            .scales
            .remove("/chibi/walk/test/test-walk-right-sheet.webp");
        assert!(
            validate(&manifest).is_err(),
            "right-side calibration is mandatory"
        );
    }
    #[test]
    fn release_archives_remain_self_contained_after_source_zip_is_deleted() {
        let directory = std::env::var("VIVIAN_CHARACTER_TEST_PACKS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/characters")
            });
        let temp = temp();
        let catalog: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(directory.join("catalog.json")).unwrap())
                .unwrap();
        for id in ["deepseek", "claude", "chatgpt"] {
            let filename = catalog.iter().find(|row| row["id"] == id).unwrap()["filename"]
                .as_str()
                .unwrap();
            let downloads = self::temp();
            let path = downloads.path().join(filename);
            std::fs::copy(directory.join(filename), &path).unwrap();
            let installed = install_into(&path, temp.path()).unwrap();
            let isolated = self::temp();
            install_into(&path, isolated.path()).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert!(!path.exists(), "the original downloaded ZIP has been deleted");
            assert_eq!(installed.id, id);
            let loaded = load(temp.path().join(id)).unwrap();
            assert_eq!(loaded.manifest.animations, installed.animations);
            assert!(loaded.manifest.animations.contains_key("blink"));
            for (file, expected_hash) in &installed.files {
                let bytes = std::fs::read(loaded.root.join(file)).unwrap();
                assert_eq!(format!("{:x}", Sha256::digest(bytes)), *expected_hash,
                    "{id}/{file}: installed assets survive deletion of the source ZIP");
            }
            assert!(!loaded.texts["appearance.md"].trim().is_empty());
            for file in [
                "traits.json",
                "behavior.json",
                "mood.json",
                "platforms.json",
            ] {
                assert!(
                    serde_json::from_str::<serde_json::Value>(&loaded.texts[file]).is_ok(),
                    "{id}/{file}: portable structured persona data"
                );
            }
            assert!(!loaded.texts["coding_voice.md"].trim().is_empty());
            assert!(!loaded.texts["diary_voice.zh.md"].trim().is_empty());
            assert!(loaded.manifest.files.len() >= 30);
            assert!(loaded
                .manifest
                .files
                .contains_key(&format!("assets/chibi/motion/{id}/{id}-blink-sheet.webp")));
            let only = snapshot_from(isolated.path());
            assert_eq!(
                only.keys().cloned().collect::<Vec<_>>(),
                vec![id.to_string()],
                "each package boots after source deletion without any companion dependency"
            );
            assert_eq!(only[id].manifest.animations, installed.animations,
                "a fresh startup recovers animation configuration from installed data");
            assert!(!only[id].texts["relationships.md"].trim().is_empty());
        }
        assert_eq!(snapshot_from(temp.path()).len(), 3);
    }
}
