//! 存储层 - 按角色隔离的笔记文件 CRUD
//!
//! 存储路径：
//! - 笔记目录：`<character_data_dir>/notebook/`
//! - JSON 元数据：`<note_id>/note.json`
//! - 渲染 HTML：`<note_id>/note.html`
//! - 索引文件：`index.json`（仅元数据摘要，不含 blocks）

use std::path::PathBuf;

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

    // 复制本地手写字体到笔记目录（fonts/ma-shan-zheng.woff2），
    // 使 HTML 通过相对路径引用、离线可用；失败时静默跳过（浏览器回退系统中文字体）。
    let _ = copy_font_assets(&dir);

    update_index(&note.char_id, note)?;

    Ok(())
}

/// 保存由 LLM 直接撰写完整 HTML 的笔记（不经过结构化渲染引擎）
///
/// 相比 `save`：note.html 直接写入提供的原始 HTML，成功时更新索引。
/// 返回写入的 HTML 长度供调用方用于知识库同步。
pub fn save_raw_html(
    char_id: &str,
    note_id: &str,
    title: &str,
    tags: &[String],
    html: &str,
) -> Result<(), String> {
    let dir = note_dir(char_id, note_id);
    let html_path = dir.join("note.html");
    std::fs::write(&html_path, html)
        .map_err(|e| format!("写入笔记 HTML 失败: {}", e))?;

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

/// 插入或更新索引中的摘要
fn upsert_index(char_id: &str, summary: NoteSummary) -> Result<(), String> {
    let mut summaries = list_index(char_id).unwrap_or_default();
    summaries.retain(|s| s.id != summary.id);
    summaries.push(summary);
    summaries.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    write_index(char_id, &summaries)
}

/// 将本地中文手写字体 `ma-shan-zheng.woff2` 复制到笔记目录 `fonts/` 子目录。
///
/// 源文件位于项目 `public/fonts/ma-shan-zheng.woff2`，经 `CARGO_MANIFEST_DIR`
/// （= src-tauri 目录）向上定位；源文件不存在时静默跳过。
fn copy_font_assets(dir: &std::path::Path) -> Result<(), String> {
    let src = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../public/fonts/ma-shan-zheng.woff2"
    ));
    if !src.exists() {
        return Ok(());
    }
    let fonts_dir = dir.join("fonts");
    ensure_dir(&fonts_dir).map_err(|e| format!("创建字体目录失败: {}", e))?;
    std::fs::copy(src, fonts_dir.join("ma-shan-zheng.woff2"))
        .map_err(|e| format!("复制字体失败: {}", e))?;
    Ok(())
}

/// 读取笔记完整数据
pub fn load(char_id: &str, note_id: &str) -> Result<NoteBook, String> {
    let json_path = note_dir(char_id, note_id).join("note.json");
    let json_str = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("读取笔记失败: {}", e))?;
    serde_json::from_str(&json_str)
        .map_err(|e| format!("解析笔记失败: {}", e))
}

/// 读取笔记 HTML 内容
pub fn load_html(char_id: &str, note_id: &str) -> Result<String, String> {
    let html_path = note_dir(char_id, note_id).join("note.html");
    std::fs::read_to_string(&html_path)
        .map_err(|e| format!("读取笔记 HTML 失败: {}", e))
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

/// 笔记目录下本地手写字体文件的绝对路径（不存在时返回 None）。
///
/// 供前端改写笔记 HTML 中的相对字体路径为 Tauri asset URL。
pub fn notebook_font_path(char_id: &str, note_id: &str) -> Option<String> {
    let p = note_dir(char_id, note_id)
        .join("fonts")
        .join("ma-shan-zheng.woff2");
    if p.exists() {
        Some(p.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 列出角色的所有笔记摘要
///
/// 结构化笔记从目录扫描 note.json 得到；LLM 直接撰写完整 HTML 的 raw_html 笔记
/// 没有 note.json（仅有 note.html），需从索引文件中补全，否则不会出现在列表里。
pub fn list(char_id: &str) -> Result<Vec<NoteSummary>, String> {
    let dir = notebook_dir(char_id);
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
            layout: format!("{:?}", note.layout).to_lowercase(),
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
        layout: format!("{:?}", note.layout).to_lowercase(),
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
