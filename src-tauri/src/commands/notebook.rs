//! 笔记本命令 - 前端调用接口
//!
//! 提供笔记列表/读取/创建/更新/删除等命令，供 MemoryWindow 的笔记本 tab 使用。

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use std::sync::Arc;

use crate::notebook::{storage, NoteBook};
use crate::state::AppState;
use crate::tools::builtin::notebook_tools::{
    parse_blocks, parse_cover, parse_layout, parse_palette, sync_notebook_to_knowledge,
    sync_raw_html_to_knowledge,
};

/// 列出角色的笔记摘要
#[tauri::command]
pub async fn list_notebooks(
    char_id: String,
    _state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let summaries = storage::list(&char_id)?;
    Ok(json!(summaries))
}

/// 读取笔记 HTML 内容
#[tauri::command]
pub async fn get_notebook_html(
    char_id: String,
    note_id: String,
    _state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let html = storage::load_html(&char_id, &note_id)?;
    let font_path = storage::notebook_font_path(&char_id, &note_id);
    let html_path = storage::notebook_html_path(&char_id, &note_id);
    Ok(json!({ "html": html, "note_id": note_id, "font_path": font_path, "html_path": html_path }))
}

/// 读取笔记完整数据（JSON）
#[tauri::command]
pub async fn get_notebook_detail(
    char_id: String,
    note_id: String,
    _state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let note = storage::load(&char_id, &note_id)?;
    serde_json::to_value(&note).map_err(|e| format!("序列化失败: {}", e))
}

/// 创建笔记（前端编辑器入口）
#[tauri::command]
pub async fn create_notebook(
    char_id: String,
    title: String,
    blocks: Value,
    layout: Option<String>,
    palette: Option<String>,
    tags: Option<Vec<String>>,
    cover: Option<Value>,
    app: AppHandle,
) -> Result<Value, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("标题不能为空".to_string());
    }
    let layout = layout.as_deref().map(parse_layout).unwrap_or_default();
    let palette = palette.as_deref().map(parse_palette).unwrap_or_default();
    let tags = tags.unwrap_or_default();
    let cover = match cover {
        Some(c) if !c.is_null() => parse_cover(&c)?,
        _ => None,
    };
    let blocks = parse_blocks(&blocks)?;
    if blocks.is_empty() {
        return Err("内容块不能为空".to_string());
    }

    let now = chrono::Local::now().timestamp() as f64;
    let note = NoteBook {
        id: NoteBook::generate_id(),
        title: title.clone(),
        char_id: char_id.clone(),
        created_at: now,
        updated_at: now,
        tags,
        layout,
        palette,
        cover,
        blocks,
    };

    storage::save(&note)?;
    let _ = app.emit(
        "notebook:created",
        json!({ "note_id": &note.id, "char_id": &char_id, "title": &title }),
    );
    sync_notebook_to_knowledge(&app, &note).await;

    Ok(json!({ "note_id": &note.id, "char_id": &char_id, "title": &title }))
}

/// 更新笔记（前端编辑器入口）
///
/// 仅更新提供的字段，未提供的字段保持原值。blocks 若提供则全量替换。
#[tauri::command]
pub async fn update_notebook(
    char_id: String,
    note_id: String,
    title: Option<String>,
    blocks: Option<Value>,
    layout: Option<String>,
    palette: Option<String>,
    tags: Option<Vec<String>>,
    cover: Option<Value>,
    app: AppHandle,
) -> Result<Value, String> {
    let mut note = storage::load(&char_id, &note_id)?;

    if let Some(t) = title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err("标题不能为空".to_string());
        }
        note.title = t;
    }
    if let Some(l) = layout {
        note.layout = parse_layout(&l);
    }
    if let Some(p) = palette {
        note.palette = parse_palette(&p);
    }
    if let Some(t) = tags {
        note.tags = t;
    }
    if let Some(c) = cover {
        note.cover = parse_cover(&c)?;
    }
    if let Some(b) = blocks {
        let new_blocks = parse_blocks(&b)?;
        if new_blocks.is_empty() {
            return Err("内容块不能为空".to_string());
        }
        note.blocks = new_blocks;
    }

    note.updated_at = chrono::Local::now().timestamp() as f64;
    storage::save(&note)?;
    let _ = app.emit(
        "notebook:updated",
        json!({ "note_id": &note.id, "char_id": &char_id, "title": &note.title }),
    );
    sync_notebook_to_knowledge(&app, &note).await;

    Ok(json!({ "note_id": &note.id, "char_id": &char_id, "title": &note.title }))
}

/// 删除笔记
#[tauri::command]
pub async fn delete_notebook(
    char_id: String,
    note_id: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    // 先清理知识库中关联的条目（笔记文件删除后无法再读 .memory_ref）
    if let Ok(inst) = state.get_character(Some(&char_id)) {
        let memory = inst.brain.memory.clone();
        let ref_path = storage::note_memory_ref_path(&char_id, &note_id);
        if let Ok(memory_id) = std::fs::read_to_string(&ref_path) {
            let memory_id = memory_id.trim();
            if !memory_id.is_empty() {
                if let Err(e) = memory.delete_knowledge_document(memory_id).await {
                    tracing::warn!("[Notebook] 删除笔记 {} 的知识库条目失败: {}", note_id, e);
                }
            }
        }
    }

    storage::delete(&char_id, &note_id)?;
    let _ = app.emit("notebook:deleted", json!({
        "note_id": &note_id,
        "char_id": &char_id,
    }));
    Ok(json!({ "deleted": true, "note_id": note_id }))
}

/// 导入本地 HTML 文件为 raw_html 笔记（前端文件选择器/拖放入口）
///
/// 与 `create_html_note`（LLM 撰写）互补：用户选择或拖入一个完整的本地 .html 文件，
/// 此处直接读取完整内容（无字符截断）并原样保存为 raw_html 笔记，供笔记 tab 渲染。
#[tauri::command]
pub async fn import_html_note(
    char_id: String,
    source_path: String,
    title: Option<String>,
    app: AppHandle,
) -> Result<Value, String> {
    let path = std::path::Path::new(&source_path);
    if !path.exists() {
        return Err("文件不存在".to_string());
    }
    if !path.is_file() {
        return Err("目标不是文件".to_string());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext != "html" && ext != "htm" {
        return Err("仅支持 .html / .htm 文件".to_string());
    }

    // 直接读取完整 HTML，不做字符截断（区别于聊天文件通道的 extract_file_text）
    let html = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 HTML 文件失败: {}", e))?;
    if html.trim().is_empty() {
        return Err("HTML 文件内容为空".to_string());
    }

    let title = match title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("导入笔记")
            .to_string(),
    };

    let note_id = NoteBook::generate_id();
    storage::save_raw_html(&char_id, &note_id, &title, &[], &html)?;
    let _ = app.emit(
        "notebook:created",
        json!({ "note_id": &note_id, "char_id": &char_id, "title": &title }),
    );
    sync_raw_html_to_knowledge(&app, &char_id, &note_id, &title, &[], &html).await;

    Ok(json!({
        "note_id": &note_id,
        "char_id": &char_id,
        "title": &title,
        "render_type": "raw_html",
    }))
}
