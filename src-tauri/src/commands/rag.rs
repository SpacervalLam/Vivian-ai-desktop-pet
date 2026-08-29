//! RAG 命令 - 知识库文档管理（已合并入 MemoryManager）
//!
//! 旧 RagEngine 已废弃，所有用户主动写入的知识文档统一存入
//! MemoryManager（MemoryType::Knowledge）。本模块仅作为前端兼容层，
//! 保留原命令签名不变。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// 序列化 MemoryItem 为前端期望的文档结构
fn item_to_doc(item: &crate::memory::types::MemoryItem) -> Value {
    let title = item
        .metadata
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source = item
        .metadata
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("manual")
        .to_string();
    json!({
        "id": item.id,
        "title": title,
        "content": item.content,
        "source": source,
        "tags": item.tags,
        "created_at": item.timestamp,
        "updated_at": item.last_visit_at.max(item.timestamp),
    })
}

/// 添加文档
#[tauri::command]
pub async fn add_rag_document(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    title: String,
    content: String,
    source: String,
    tags: Option<Vec<String>>,
) -> Result<Value, String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();

    let final_tags = tags.unwrap_or_default();
    let effective_source = if source.is_empty() { "manual" } else { &source };

    let item = memory
        .add_knowledge_document(&title, &content, final_tags, effective_source, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(item_to_doc(&item))
}

/// 删除文档
#[tauri::command]
pub async fn delete_rag_document(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    document_id: String,
) -> Result<bool, String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();
    memory
        .delete_knowledge_document(&document_id)
        .await
        .map_err(|e| e.to_string())
}

/// 列出所有文档
#[tauri::command]
pub async fn list_rag_documents(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();
    let items = memory
        .list_knowledge_documents()
        .await
        .map_err(|e| e.to_string())?;
    let docs: Vec<Value> = items.iter().map(item_to_doc).collect();
    Ok(json!({ "documents": docs, "total": docs.len() }))
}

/// 检索文档（复用 MemoryManager.search_memories，仅过滤 knowledge 类型）
#[tauri::command]
pub async fn search_rag(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    query: String,
    limit: Option<usize>,
) -> Result<Value, String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();
    let k = limit.unwrap_or(5);
    let all = memory
        .search_memories(&query, crate::memory::types::RetrievalStrategy::Auto, k * 3)
        .await
        .map_err(|e| e.to_string())?;

    let results: Vec<Value> = all
        .into_iter()
        .filter(|m| {
            m.tags.iter().any(|t| t == "knowledge")
                && matches!(m.metadata.get("kind"), Some(v) if v == "knowledge_document")
        })
        .take(k)
        .map(|item| {
            let doc = item_to_doc(&item);
            json!({
                "document": doc,
                "score": item.heat_score,
                "snippet": item.content.chars().take(120).collect::<String>(),
            })
        })
        .collect();

    Ok(json!({ "results": results, "query": query }))
}

/// 清空知识库（仅清空 knowledge 文档，保留其他记忆）
#[tauri::command]
pub async fn clear_rag(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let memory = state.get_character(character_id.as_deref())?.brain.memory.clone();
    memory
        .clear_knowledge_documents()
        .await
        .map_err(|e| e.to_string())
}
