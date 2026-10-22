//! 存储层 - 按角色隔离的笔记文件 CRUD
//!
//! 存储路径：
//! - 笔记目录：`<character_data_dir>/notebook/`
//! - JSON 元数据：`<note_id>/note.json`
//! - 渲染 HTML：`<note_id>/note.html`
//! - 索引文件：`index.json`（仅元数据摘要，不含 blocks）

use std::path::PathBuf;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;
use serde::{Deserialize, Serialize};

use super::NoteBook;
use crate::utils::path::{ensure_dir, get_character_data_dir};

/// 笔记摘要（用于列表展示，不含完整 blocks）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub char_id: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub tags: Vec<String>,
    pub palette: String,
    pub layout: String,
    pub block_count: usize,
    /// 渲染类型："structured"=结构化内容块渲染，"raw_html"=LLM 直接提供的完整 HTML
    #[serde(default = "default_render_type")]
    pub render_type: String,
}

fn default_render_type() -> String {
    "structured".to_string()
}

/// 手写字体文件名。
const FONT_FILE: &str = "ma-shan-zheng.woff2";
/// 旧版把字体复制到 `<note>/fonts/`，HTML 用相对路径引用。
const LEGACY_FONT_URL: &str = "fonts/ma-shan-zheng.woff2";
/// 共享字体在数据目录根部；note.html 位于 `characters/<char>/notebook/<note>/`，
/// 相对上溯四级即可引用，独立打开笔记仍能离线加载。
const SHARED_FONT_URL: &str = "../../../../fonts/ma-shan-zheng.woff2";

/// 获取角色的笔记目录
fn notebook_dir(char_id: &str) -> PathBuf {
    let dir = get_character_data_dir(char_id).join("notebook");
    let _ = ensure_dir(&dir);
    dir
}

/// 获取单个笔记的目录
fn note_dir(char_id: &str, note_id: &str) -> PathBuf {
    let dir = notebook_dir(char_id).join(note_id);
    let _ = ensure_dir(&dir);
    dir
}

/// 笔记与知识库条目的关联文件路径（存 memory_id，更新时据此删除旧条目）
pub fn note_memory_ref_path(char_id: &str, note_id: &str) -> PathBuf {
    note_dir(char_id, note_id).join(".memory_ref")
}

/// 保存笔记（JSON + HTML）
pub fn save(note: &NoteBook) -> Result<(), String> {
    let dir = note_dir(&note.char_id, &note.id);
    let json_path = dir.join("note.json");
    let html_path = dir.join("note.html");

    let json_str = serde_json::to_string_pretty(note)
        .map_err(|e| format!("序列化笔记失败: {}", e))?;
    std::fs::write(&json_path, json_str)
        .map_err(|e| format!("写入笔记 JSON 失败: {}", e))?;

    let html = super::renderer::render_html(note);
    std::fs::write(&html_path, html)
        .map_err(|e| format!("写入笔记 HTML 失败: {}", e))?;

    // 字体只保留数据目录根部的一份共享副本，不再逐篇挂载。
    // 结构化模板本身不引用字体文件；旧的逐篇副本在共享字体可用时清理。
    if ensure_shared_font().is_some() {
        remove_note_local_font(&dir);
    }

    update_index(&note.char_id, note)?;

    Ok(())
}

/// 黑名单式 HTML 消毒。
///
/// 笔记 HTML 需要完整表达能力（div/table/style/class 等），不能用全量白名单，
/// 因此只做黑名单式移除：脚本标签、事件处理器属性、嵌套 iframe、javascript: 协议。
/// 真正的脚本执行边界是前端 iframe 的 `sandbox`（不带 allow-scripts，脚本不执行），
/// 此处是纵深防御兜底——防止 sandbox 未来被改松后引入脚本执行面。
pub(crate) fn sanitize_html(html: &str) -> String {
    static SCRIPT_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>").unwrap());
    static IFRAME_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?is)<iframe\b[^>]*>.*?</iframe\s*>").unwrap());
    static ON_ATTR_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"(?i)\son\w+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)"#).unwrap()
    });
    static JS_PROTO_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)javascript:").unwrap());

    let cleaned = SCRIPT_RE.replace_all(html, "");
    let cleaned = IFRAME_RE.replace_all(&cleaned, "");
    let cleaned = ON_ATTR_RE.replace_all(&cleaned, "");
    let cleaned = JS_PROTO_RE.replace_all(&cleaned, "");
    cleaned.to_string()
}

/// 保存由 LLM 直接撰写完整 HTML 的笔记（不经过结构化渲染引擎）
///
/// 相比 `save`：note.html 直接写入提供的原始 HTML，成功时更新索引。
/// 写入前经 `sanitize_html` 消毒（移除脚本/事件/iframe），保证磁盘上的
/// 笔记永远是安全的。返回写入的 HTML 长度供调用方用于知识库同步。
pub fn save_raw_html(
    char_id: &str,
    note_id: &str,
    title: &str,
    tags: &[String],
    html: &str,
) -> Result<(), String> {
    let html = sanitize_html(html);
    let shared_font = ensure_shared_font();
    let html = if shared_font.is_some() { rewrite_font_url(&html) } else { html };
    let dir = note_dir(char_id, note_id);
    let html_path = dir.join("note.html");
    std::fs::write(&html_path, html)
        .map_err(|e| format!("写入笔记 HTML 失败: {}", e))?;
    if shared_font.is_some() {
        remove_note_local_font(&dir);
    }

    let now = chrono::Local::now().timestamp() as f64;
    let summary = NoteSummary {
        id: note_id.to_string(),
        title: title.to_string(),
        char_id: char_id.to_string(),
        created_at: now,
        updated_at: now,
        tags: tags.to_vec(),
        palette: "warm".to_string(),
        layout: "cover_flow".to_string(),
        block_count: 0,
        render_type: "raw_html".to_string(),
    };
    upsert_index(char_id, summary)?;
    Ok(())
}

/// 判断笔记是否为 LLM 直接撰写完整 HTML 的类型（存在 note.html 但无 note.json）
pub fn is_raw_html(char_id: &str, note_id: &str) -> bool {
    let json_path = note_dir(char_id, note_id).join("note.json");
    let html_path = note_dir(char_id, note_id).join("note.html");
    !json_path.exists() && html_path.exists()
}

/// raw_html 笔记的侧车元数据文件名。
///
/// raw_html 笔记只有 `note.html`、没有 `note.json`，所以「不渲染但参与检索」的
/// 来源说明（对应结构化笔记的 `Block::Meta`）存在这个侧车里。独立文件而不是
/// 塞进 note.json：raw_html 的定义就是「没有 note.json」，加回来会破坏
/// `is_raw_html` 的判据。
const META_SIDECAR: &str = "meta.json";

/// 写入 raw_html 笔记的侧车元数据（数据来源说明等，不渲染但参与 RAG 检索）。
///
/// 空白 `meta_note` 会删掉侧车文件，避免留下空壳。
pub fn save_raw_html_meta(
    char_id: &str,
    note_id: &str,
    meta_note: Option<&str>,
) -> Result<(), String> {
    let dir = note_dir(char_id, note_id);
    let path = dir.join(META_SIDECAR);
    match meta_note.map(str::trim).filter(|m| !m.is_empty()) {
        Some(text) => {
            ensure_dir(&dir).map_err(|e| format!("创建笔记目录失败: {}", e))?;
            let value = json!({ "provenance": text });
            std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap_or_default())
                .map_err(|e| format!("写入笔记元数据失败: {}", e))
        }
        None => {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| format!("删除笔记元数据失败: {}", e))?;
            }
            Ok(())
        }
    }
}

/// 读取 raw_html 笔记的侧车元数据（供检索文本拼接用）。
pub fn load_raw_html_meta(char_id: &str, note_id: &str) -> Option<String> {
    let path = note_dir(char_id, note_id).join(META_SIDECAR);
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let text = value.get("provenance")?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// 插入或更新索引中的摘要
fn upsert_index(char_id: &str, summary: NoteSummary) -> Result<(), String> {
    let mut summaries = list_index(char_id).unwrap_or_default();
    summaries.retain(|s| s.id != summary.id);
    summaries.push(summary);
    // Legacy user quick notes are imported into the independent user store;
    // retain original files as backups, but never present them as pet notebooks.
    summaries.retain(|note| !note.tags.iter().any(|tag| tag == "quick_note"));
    summaries.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    write_index(char_id, &summaries)
}

/// 数据目录根部共享的手写字体，所有笔记共用同一份。
fn shared_font_path() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("fonts").join(FONT_FILE)
}

/// 内容寻址对象池：共享字体按内容复用，可随时重建，不进入备份。
fn font_object_dir() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("cache/assets/notebook-fonts")
}

/// 确保共享字体存在（首次保存时从可选资源包物化一份）。
///
/// 资源包未安装或读取失败时返回 None，笔记回退系统中文字体；此时保留旧版
/// 逐篇副本，避免迁移反而让离线打开失效。
fn ensure_shared_font() -> Option<PathBuf> {
    let target = shared_font_path();
    if target.metadata().is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
        return Some(target);
    }
    let source = crate::commands::optional_resources::resource_path(
        "fonts",
        &format!("fonts/{FONT_FILE}"),
    )?;
    let bytes = std::fs::read(source).ok()?;
    crate::utils::assets::materialize(&font_object_dir(), &target, &bytes).ok()?;
    target.metadata().ok().filter(|meta| meta.is_file() && meta.len() > 0).map(|_| target)
}

/// 旧版 HTML 使用 `fonts/ma-shan-zheng.woff2`，迁移时改写为共享字体的相对路径。
fn rewrite_font_url(html: &str) -> String {
    if !has_legacy_font_url(html) {
        return html.to_string();
    }
    html.replace(LEGACY_FONT_URL, SHARED_FONT_URL)
}

/// 共享路径本身也包含 `fonts/ma-shan-zheng.woff2` 子串，必须先确认尚未迁移，
/// 否则重复迁移会不断叠加 `../`。
fn has_legacy_font_url(html: &str) -> bool {
    html.contains(LEGACY_FONT_URL) && !html.contains(SHARED_FONT_URL)
}

/// 删除某篇笔记旧版复制的字体；只动已知字体文件，目录空了才移除。
fn remove_note_local_font(dir: &std::path::Path) {
    let fonts_dir = dir.join("fonts");
    if std::fs::remove_file(fonts_dir.join(FONT_FILE)).is_ok() {
        let _ = std::fs::remove_dir(&fonts_dir);
    }
}

/// 把目录内所有笔记的逐篇字体迁移到共享副本。
///
/// 只在共享字体可用时执行：先把 HTML 中的旧相对路径改写为共享路径，
/// 再删除本地副本，保证笔记独立打开仍然可用。幂等，已迁移的目录只剩元数据扫描。
fn migrate_legacy_fonts(dir: &std::path::Path) {
    if ensure_shared_font().is_none() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let note_dir = entry.path();
        if !note_dir.is_dir() || !note_dir.join("fonts").join(FONT_FILE).exists() {
            continue;
        }
        let html_path = note_dir.join("note.html");
        if let Ok(html) = std::fs::read_to_string(&html_path) {
            if has_legacy_font_url(&html) {
                let _ = std::fs::write(&html_path, rewrite_font_url(&html));
            }
        }
        remove_note_local_font(&note_dir);
    }
}

/// 读取笔记完整数据
pub fn load(char_id: &str, note_id: &str) -> Result<NoteBook, String> {
    let json_path = note_dir(char_id, note_id).join("note.json");
    let json_str = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("读取笔记失败: {}", e))?;
    let note: NoteBook = serde_json::from_str(&json_str).map_err(|e| format!("解析笔记失败: {}", e))?;
    if note.tags.iter().any(|tag| tag == "quick_note") {
        return Err("这是用户随手记的旧备份，请通过用户随手记页面或 read_user_quick_notes 读取，不属于角色笔记。".into());
    }
    Ok(note)
}

/// 读取笔记 HTML 内容
/// 旧模板只刷新派生 HTML，不改笔记正文、时间或知识库关联；原生 HTML 原样读取。
pub fn load_html(char_id: &str, note_id: &str) -> Result<String, String> {
    let dir = note_dir(char_id, note_id);
    let html_path = dir.join("note.html");
    let cached = std::fs::read_to_string(&html_path);
    if dir.join("note.json").exists()
        && !cached.as_ref().is_ok_and(|html| html.contains(super::renderer::TEMPLATE_MARKER))
    {
        let note = load(char_id, note_id)?;
        let html = super::renderer::render_html(&note);
        std::fs::write(&html_path, &html)
            .map_err(|e| format!("更新笔记模板失败: {}", e))?;
        return Ok(html);
    }
    // 旧版 raw_html 笔记仍引用 note-local 字体；共享字体可用时就地迁移，
    // 之后独立打开 note.html 也不会因副本被清理而回退字体。
    if let Ok(html) = &cached {
        if has_legacy_font_url(html) && ensure_shared_font().is_some() {
            let migrated = rewrite_font_url(html);
            std::fs::write(&html_path, &migrated)
                .map_err(|e| format!("迁移笔记字体失败: {}", e))?;
            remove_note_local_font(&dir);
            return Ok(migrated);
        }
    }
    cached.map_err(|e| format!("读取笔记 HTML 失败: {}", e))
}

/// 笔记渲染 HTML 文件的绝对路径（不存在时返回 None）。
///
/// 供前端以 asset 协议 URL（convertFileSrc）加载预览 iframe 的 src，
/// 使笔记文档与应用窗口处于跨源（cross-origin）上下文，隔离笔记内的脚本/按钮。
pub fn notebook_html_path(char_id: &str, note_id: &str) -> Option<String> {
    let p = note_dir(char_id, note_id).join("note.html");
    if p.exists() {
        Some(p.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 共享手写字体文件的绝对路径（不存在时返回 None）。
///
/// 供前端以 asset 协议 URL 加载字体；所有笔记共用同一份，不再逐篇存放。
pub fn notebook_font_path(_char_id: &str, _note_id: &str) -> Option<String> {
    ensure_shared_font().map(|p| p.to_string_lossy().to_string())
}

/// 列出角色的所有笔记摘要
///
/// 结构化笔记从目录扫描 note.json 得到；LLM 直接撰写完整 HTML 的 raw_html 笔记
/// 没有 note.json（仅有 note.html），需从索引文件中补全，否则不会出现在列表里。
pub fn list(char_id: &str) -> Result<Vec<NoteSummary>, String> {
    let dir = notebook_dir(char_id);
    migrate_legacy_fonts(&dir);
    let mut summaries = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(summaries),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let json_path = path.join("note.json");
        let json_str = match std::fs::read_to_string(&json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let note: NoteBook = match serde_json::from_str(&json_str) {
            Ok(n) => n,
            Err(_) => continue,
        };
        seen.insert(note.id.clone());
        summaries.push(NoteSummary {
            id: note.id.clone(),
            title: note.title.clone(),
            char_id: note.char_id.clone(),
            created_at: note.created_at,
            updated_at: note.updated_at,
            tags: note.tags.clone(),
            palette: format!("{:?}", note.palette).to_lowercase(),
            layout: note.layout.as_str().to_string(),
            block_count: note.blocks.len(),
            render_type: "structured".to_string(),
        });
    }

    // 补全索引中仅含 HTML（无 note.json）的 raw_html 笔记
    for s in list_index(char_id).unwrap_or_default() {
        if !seen.contains(&s.id) {
            seen.insert(s.id.clone());
            summaries.push(s);
        }
    }

    summaries.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    Ok(summaries)
}

/// 删除笔记
pub fn delete(char_id: &str, note_id: &str) -> Result<(), String> {
    let dir = note_dir(char_id, note_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("删除笔记失败: {}", e))?;
    }
    remove_from_index(char_id, note_id)?;
    Ok(())
}

/// 清空角色的所有笔记（恢复出厂设置调用）
///
/// 直接删除整个 notebook/ 目录，包括 index.json 与所有笔记的
/// note.json / note.html / .memory_ref 文件。
/// 知识库条目由 MemoryManager::clear_all_memories 的 entries.clear() 一并清空，
/// 无需逐个读 .memory_ref 调用 delete_knowledge_document。
pub fn clear_all(char_id: &str) -> Result<(), String> {
    let dir = get_character_data_dir(char_id).join("notebook");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("清空笔记目录失败: {}", e))?;
    }
    Ok(())
}

/// 索引文件路径
fn index_path(char_id: &str) -> PathBuf {
    notebook_dir(char_id).join("index.json")
}

/// 更新索引（保存/更新时调用）
fn update_index(char_id: &str, note: &NoteBook) -> Result<(), String> {
    let mut summaries = list_index(char_id).unwrap_or_default();
    summaries.retain(|s| s.id != note.id);
    summaries.push(NoteSummary {
        id: note.id.clone(),
        title: note.title.clone(),
        char_id: note.char_id.clone(),
        created_at: note.created_at,
        updated_at: note.updated_at,
        tags: note.tags.clone(),
        palette: format!("{:?}", note.palette).to_lowercase(),
        layout: note.layout.as_str().to_string(),
        block_count: note.blocks.len(),
        render_type: "structured".to_string(),
    });
    summaries.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    write_index(char_id, &summaries)
}

/// 从索引中移除
fn remove_from_index(char_id: &str, note_id: &str) -> Result<(), String> {
    let mut summaries = list_index(char_id).unwrap_or_default();
    summaries.retain(|s| s.id != note_id);
    write_index(char_id, &summaries)
}

/// 读取索引
fn list_index(char_id: &str) -> Result<Vec<NoteSummary>, String> {
    let path = index_path(char_id);
    let json_str = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(Vec::new()),
    };
    serde_json::from_str(&json_str).map_err(|e| format!("解析索引失败: {}", e))
}

/// 写入索引
fn write_index(char_id: &str, summaries: &[NoteSummary]) -> Result<(), String> {
    let path = index_path(char_id);
    let json_str = serde_json::to_string_pretty(summaries)
        .map_err(|e| format!("序列化索引失败: {}", e))?;
    std::fs::write(&path, json_str)
        .map_err(|e| format!("写入索引失败: {}", e))
}

#[cfg(test)]
mod font_tests {
    use super::*;

    #[test]
    fn legacy_font_url_migrates_once() {
        let legacy = "src: url('fonts/ma-shan-zheng.woff2') format('woff2')";
        let shared = rewrite_font_url(legacy);
        assert_eq!(shared, "src: url('../../../../fonts/ma-shan-zheng.woff2') format('woff2')");
        assert_eq!(rewrite_font_url(&shared), shared);
    }

    #[test]
    fn removing_note_font_keeps_note_html() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note");
        let fonts = note.join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join(FONT_FILE), b"font").unwrap();
        std::fs::write(note.join("note.html"), b"html").unwrap();
        remove_note_local_font(&note);
        assert!(!fonts.exists());
        assert!(note.join("note.html").exists());
    }
}
