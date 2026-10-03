use crate::{
    dialogue_lab::{self as lab, Branch, LabTurn},
    state::AppState,
    types::response::ChatMessage,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc};
use tauri::State;

static BUSY: Lazy<Mutex<HashSet<String>>> = Lazy::new(Default::default);
struct Permit(String);
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.lock().remove(&self.0);
    }
}
fn root() -> std::path::PathBuf {
    crate::utils::get_user_data_dir().join("dialogue-lab")
}
fn title(value: String) -> String {
    value.trim().chars().take(80).collect()
}

#[tauri::command]
pub fn list_dialogue_lab(
    character_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let router = state.get_character(Some(&character_id))?.brain.router;
    let mut branches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root()) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if let Ok(branch) = lab::load(&root(), &id) {
                if branch.snapshot.character_id == character_id {
                    branches.push(branch);
                }
            }
        }
    }
    branches.sort_by(|a, b| {
        b.snapshot
            .captured_at
            .total_cmp(&a.snapshot.captured_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    branches.truncate(100);
    let summaries: Vec<_> = branches.iter().map(|b| json!({"id":b.id,"title":b.title,
        "captured_at":b.snapshot.captured_at,"user_input":b.snapshot.user_input,"turn_count":b.turns.len()})).collect();
    let snapshot = lab::latest(&character_id).map(|s| {
        json!({"user_input":s.user_input,
        "captured_at":s.captured_at,"model":s.model,"route":s.route})
    });
    let routes = ["chat", "reasoning"]
        .map(|route| json!({"id":route,"model":router.dialogue_model_name(route)}));
    Ok(json!({"snapshot": snapshot, "branches": summaries, "routes": routes}))
}

#[tauri::command]
pub fn get_dialogue_lab(branch_id: String) -> Result<Branch, String> {
    lab::load(&root(), &branch_id)
}

#[tauri::command]
pub fn delete_dialogue_lab(branch_id: String) -> Result<(), String> {
    let id = uuid::Uuid::parse_str(&branch_id)
        .map_err(|_| "无效分支 ID")?
        .to_string();
    if !BUSY.lock().insert(id.clone()) {
        return Err("分支正在生成，请等待完成".into());
    }
    let _permit = Permit(id.clone());
    std::fs::remove_file(lab::branch_path(&root(), &id)?).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_dialogue_lab(character_id: String, name: String) -> Result<Branch, String> {
    let snapshot =
        lab::latest(&character_id).ok_or("请先与该角色进行一次普通文字对话，再创建试聊")?;
    let branch = Branch {
        id: uuid::Uuid::new_v4().to_string(),
        parent_id: None,
        title: title(name),
        snapshot,
        turns: Vec::new(),
    };
    lab::save(&root(), &branch)?;
    Ok(branch)
}

#[tauri::command]
pub fn fork_dialogue_lab(
    branch_id: String,
    name: String,
    from_start: bool,
) -> Result<Branch, String> {
    let source = lab::load(&root(), &branch_id)?;
    let branch = lab::fork(&source, title(name), from_start);
    lab::save(&root(), &branch)?;
    Ok(branch)
}

#[tauri::command]
pub async fn run_dialogue_lab(
    state: State<'_, Arc<AppState>>,
    branch_id: String,
    input: String,
    route: String,
    prompt_note: String,
) -> Result<Branch, String> {
    // Canonicalization also makes differently formatted UUIDs share one reservation.
    let id = uuid::Uuid::parse_str(&branch_id)
        .map_err(|_| "无效分支 ID")?
        .to_string();
    if !BUSY.lock().insert(id.clone()) {
        return Err("这个试聊分支正在生成，请等待完成".into());
    }
    let _permit = Permit(id.clone());
    if !matches!(route.as_str(), "chat" | "reasoning") {
        return Err("请选择已配置的聊天模型路由".into());
    }
    if input.chars().count() > 4000 || prompt_note.chars().count() > 2000 {
        return Err("试聊输入或补充提示过长".into());
    }
    let mut branch = lab::load(&root(), &id)?;
    if branch.turns.len() >= 16 {
        return Err("此分支已满 16 轮，请创建新分支".into());
    }
    if !branch.turns.is_empty() && input.trim().is_empty() {
        return Err("请输入接下来的话，或从起点创建对照分支".into());
    }
    let router = state
        .get_character(Some(&branch.snapshot.character_id))?
        .brain
        .router;
    let messages = lab::request_messages(&branch, &input, &prompt_note)
        .into_iter()
        .map(|m| match m.role.as_str() {
            "user" => ChatMessage::user(m.content),
            "assistant" => ChatMessage::assistant(m.content),
            _ => ChatMessage::system(m.content),
        })
        .collect();
    let mut request =
        crate::pipeline::steps::generation::AIResponseGenerationRunnable::build_chat_request(
            &route, messages,
        )
        .with_character_id(branch.snapshot.character_id.clone())
        .with_search(false)
        .without_framework_instructions();
    request.usage_tag = Some("dialogue_lab".into());
    let start = std::time::Instant::now();
    let raw = router
        .scope_call_options(
            crate::providers::base::ProviderCallOptions {
                enable_search: Some(false),
                response_cache_allowed: Some(false),
                ..Default::default()
            },
            router.generate(request),
        )
        .await
        .map_err(|e| e.to_string())?;
    let value = raw
        .find('{')
        .zip(raw.rfind('}'))
        .and_then(|(a, b)| raw.get(a..=b))
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let reply = match &value {
        Some(value) if value.get("intent").and_then(Value::as_str) == Some("no_reply") => {
            String::new()
        }
        Some(value) => value
            .get("text")
            .and_then(Value::as_str)
            .ok_or("模型未返回有效的台词字段")?
            .to_string(),
        None => return Err("模型未返回有效的试聊 JSON，请调整模型或重试".into()),
    };
    branch.turns.push(LabTurn {
        user: input.trim().into(),
        reply,
        raw,
        route: route.clone(),
        model: router.dialogue_model_name(&route),
        prompt_note,
        elapsed_ms: start.elapsed().as_millis() as u64,
    });
    lab::save(&root(), &branch)?;
    Ok(branch)
}
