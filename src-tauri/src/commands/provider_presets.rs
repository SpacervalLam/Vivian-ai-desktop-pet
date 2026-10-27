//! Settings-only, sequential verification of provider metadata against official sources.
use super::coding_agent::CODING_AGENT;
use crate::brain::coding_agent::{CodingMessage, CodingRole, CodingStatus};
use crate::plugins::ProviderPresetData;
use crate::provider_preset_updates::{
    self as contract, ItemStatus, UpdateItem, UpdateJob, UpdateTarget,
};
use crate::state::AppState;
use crate::tools::registry::AgentSide;
use crate::tools::types::ToolUseContext;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

const EVENT: &str = "provider-presets:update";

struct Scope {
    session_id: String,
    target: String,
    expected: Value,
    roots: Vec<String>,
    writes: usize,
}
#[derive(Default)]
struct Store {
    job: Option<UpdateJob>,
    scope: Option<Scope>,
}

fn history_path() -> std::path::PathBuf {
    crate::utils::path::get_user_data_dir().join("provider_preset_updates/last.json")
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let mut job: Option<UpdateJob> = std::fs::read_to_string(history_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        if let Some(job) = &mut job {
            job.recover();
        }
        Mutex::new(Store { job, scope: None })
    })
}
fn persist(job: &UpdateJob) {
    let path = history_path();
    let result = std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|error| error.to_string())
        .and_then(|_| serde_json::to_string_pretty(job).map_err(|error| error.to_string()))
        .and_then(|text| {
            crate::utils::fs::write_atomic(&path, &text).map_err(|error| error.to_string())
        });
    if let Err(error) = result {
        tracing::warn!("Provider update history: {error}");
    }
}
fn publish(app: &AppHandle, id: &str, update: impl FnOnce(&mut Store)) {
    let view = {
        let mut store = store().lock();
        if store.job.as_ref().is_none_or(|job| job.id != id) {
            return;
        }
        update(&mut store);
        if let Some(job) = &mut store.job {
            job.revision += 1;
        }
        let view = store.job.clone().unwrap();
        persist(&view);
        view
    };
    let _ = app.emit(EVENT, &view);
}
fn config_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "config" {
        Ok(())
    } else {
        Err("模型预设更新只允许设置窗口发起".into())
    }
}
fn targets() -> Result<Vec<UpdateTarget>, String> {
    Ok(crate::plugins::builtin_provider_presets()?
        .into_iter()
        .map(|row| UpdateTarget {
            id: row.id.clone(),
            label: row.label.unwrap_or_else(|| row.id.clone()),
            label_key: row.label_key,
            model_count: row.main_models.len(),
            verified_at: row.verified_at,
            verified_source: row.verified_source,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    providers: Vec<UpdateTarget>,
    job: Option<UpdateJob>,
}

#[tauri::command]
pub fn provider_preset_update_info(window: WebviewWindow) -> Result<UpdateInfo, String> {
    config_window(&window)?;
    Ok(UpdateInfo {
        providers: targets()?,
        job: store().lock().job.clone(),
    })
}
#[tauri::command]
pub fn provider_preset_update_status(window: WebviewWindow) -> Result<Option<UpdateJob>, String> {
    config_window(&window)?;
    Ok(store().lock().job.clone())
}

#[tauri::command]
pub fn start_provider_preset_update(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    char_id: String,
    provider_ids: Vec<String>,
    language: Option<String>,
) -> Result<UpdateJob, String> {
    config_window(&window)?;
    let selected = contract::select_targets(&targets()?, &provider_ids)?;
    let char_id = if char_id.trim().is_empty() {
        state.active_character_id.read().clone()
    } else {
        char_id
    };
    if char_id.is_empty() {
        return Err("请先选择一个角色，再更新模型预设".into());
    }
    let router = state
        .model_router
        .read()
        .clone()
        .ok_or("请先保存并配置可用的工作模型，再更新预设")?;
    if state
        .skill_service
        .find("llm-providers/verify-provider-presets")
        .is_none()
    {
        return Err("llm-providers 的核对指导尚未加载，请检查插件".into());
    }
    for name in contract::TOOLS {
        if !state.tool_system.has_tool(name)
            || state.tool_system.is_tool_disabled(name, AgentSide::Work)
        {
            return Err(format!("工作侧工具 {name} 不可用，请先启用"));
        }
    }
    let mut guard = store().lock();
    if guard.job.as_ref().is_some_and(|job| job.running) {
        return Err("已有模型预设更新正在进行".into());
    }
    let job = UpdateJob {
        id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
        running: true,
        stop_requested: false,
        started_at: chrono::Local::now().to_rfc3339(),
        finished_at: None,
        items: selected
            .into_iter()
            .map(|target| UpdateItem {
                id: target.id,
                label: target.label,
                status: ItemStatus::Queued,
                message: None,
                session_id: None,
                source: None,
                added: 0,
                removed: 0,
                model_count: target.model_count,
            })
            .collect(),
    };
    guard.job = Some(job.clone());
    guard.scope = None;
    persist(&job);
    drop(guard);
    let state = state.inner().clone();
    let id = job.id.clone();
    let language = language.unwrap_or_else(|| "zh".into());
    tauri::async_runtime::spawn(async move {
        use futures::FutureExt;
        let result = std::panic::AssertUnwindSafe(run_queue(
            &app, &state, &router, &id, &char_id, &language,
        ))
        .catch_unwind()
        .await;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("核对任务发生异常，请重试或查看核对会话".into()),
        };
        publish(&app, &id, |store| {
            store.scope = None;
            if let Some(job) = &mut store.job {
                job.running = false;
                job.finished_at = Some(chrono::Local::now().to_rfc3339());
                for item in &mut job.items {
                    if matches!(item.status, ItemStatus::Queued | ItemStatus::Running) {
                        item.status = if job.stop_requested {
                            ItemStatus::Stopped
                        } else {
                            ItemStatus::Failed
                        };
                        item.message = error.clone();
                    }
                }
            }
        });
    });
    Ok(job)
}

/// Stop at the vendor boundary: already accepted metadata is never rolled back.
#[tauri::command]
pub fn stop_provider_preset_update(
    window: WebviewWindow,
    app: AppHandle,
    job_id: String,
) -> Result<(), String> {
    config_window(&window)?;
    {
        let guard = store().lock();
        if guard
            .job
            .as_ref()
            .is_none_or(|job| job.id != job_id || !job.running)
        {
            return Err("核对任务已结束".into());
        }
    }
    publish(&app, &job_id, |store| {
        if let Some(job) = &mut store.job {
            job.stop_requested = true;
        }
    });
    Ok(())
}

async fn run_queue(
    app: &AppHandle,
    state: &Arc<AppState>,
    router: &crate::providers::ModelRouter,
    job_id: &str,
    char_id: &str,
    language: &str,
) -> Result<(), String> {
    let ids: Vec<_> = store()
        .lock()
        .job
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let directory = crate::plugins::plugins_dir().join("llm-providers");
    for target in ids {
        if store()
            .lock()
            .job
            .as_ref()
            .is_none_or(|job| job.id != job_id || job.stop_requested)
        {
            break;
        }
        let result = verify_vendor(
            app, state, router, job_id, char_id, language, &directory, &target,
        )
        .await;
        if let Err(error) = result {
            publish(app, job_id, |store| {
                store.scope = None;
                if let Some(item) = store
                    .job
                    .as_mut()
                    .and_then(|job| job.items.iter_mut().find(|item| item.id == target))
                {
                    item.status = ItemStatus::Failed;
                    item.message = Some(error);
                }
            });
        }
    }
    Ok(())
}

async fn verify_vendor(
    app: &AppHandle,
    state: &Arc<AppState>,
    router: &crate::providers::ModelRouter,
    job_id: &str,
    char_id: &str,
    language: &str,
    directory: &std::path::Path,
    target: &str,
) -> Result<(), String> {
    let baseline = crate::plugins::builtin_provider_presets()?
        .into_iter()
        .find(|row| row.id == target)
        .ok_or("厂商预设已被删除")?;
    let expected = serde_json::to_value(&baseline).map_err(|error| error.to_string())?;
    let roots = allowed_roots(&baseline);
    if roots.is_empty() {
        return Err("该厂商缺少可信的官方资料入口，无法自动核对".into());
    }
    let session =
        CODING_AGENT.create_session(char_id, &directory.to_string_lossy(), contract::MODE);
    let title: String = format!("预设核对：{}", baseline.label.as_deref().unwrap_or(target))
        .chars()
        .take(30)
        .collect();
    CODING_AGENT.rename_session(&session.session_id, &title)?;
    let config = state.config.read().get_all().clone();
    CODING_AGENT.set_context_window(
        &session.session_id,
        super::coding_agent::resolve_context_window(&config, config.active_work_model.as_deref()),
    );
    let mut claimed = false;
    publish(app, job_id, |store| {
        if store.job.as_ref().is_none_or(|job| job.stop_requested) {
            return;
        }
        claimed = true;
        store.scope = Some(Scope {
            session_id: session.session_id.clone(),
            target: target.into(),
            expected: expected.clone(),
            roots: roots.clone(),
            writes: 0,
        });
        if let Some(item) = store
            .job
            .as_mut()
            .and_then(|job| job.items.iter_mut().find(|item| item.id == target))
        {
            item.status = ItemStatus::Running;
            item.session_id = Some(session.session_id.clone());
        }
    });
    if !claimed {
        return Ok(());
    }
    let prompt = verification_prompt(target, &expected, &roots, language);
    CODING_AGENT.send_message(
        app.clone(),
        session.session_id.clone(),
        router.clone(),
        state.tool_system.clone(),
        prompt,
        Vec::new(),
        Vec::new(),
        (config.tools.max_coding_rounds as usize).clamp(8, 24),
        false,
        false,
    )?;
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut timed_out = false;
    while CODING_AGENT.session_status(&session.session_id) == Some(CodingStatus::Running) {
        if Instant::now() >= deadline {
            let _ = CODING_AGENT.cancel(&session.session_id);
            timed_out = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let current = crate::plugins::builtin_provider_presets()?
        .into_iter()
        .find(|row| row.id == target)
        .ok_or("核对期间预设被删除")?;
    let current = serde_json::to_value(current).map_err(|error| error.to_string())?;
    let committed = {
        let guard = store().lock();
        guard.scope.as_ref().is_some_and(|scope| {
            scope.session_id == session.session_id && scope.writes > 0 && scope.expected == current
        })
    };
    if !committed {
        let reason = if timed_out {
            "该厂商核对超时，未收到经过来源核验的更新回执".to_string()
        } else {
            CODING_AGENT
                .get_session(&session.session_id)
                .and_then(|session| {
                    session.messages.into_iter().rev().find(|message| {
                        message.role == CodingRole::Error
                            || (message.role == CodingRole::ToolResult
                                && message.tool_success == Some(false))
                    })
                })
                .map(|message| message.content.chars().take(500).collect())
                .unwrap_or_else(|| "未收到经过来源核验的更新回执；详情见工作界面的核对会话".into())
        };
        return Err(reason);
    }
    let (added, removed) = contract::model_delta(&expected, &current);
    publish(app, job_id, |store| {
        store.scope = None;
        if let Some(item) = store
            .job
            .as_mut()
            .and_then(|job| job.items.iter_mut().find(|item| item.id == target))
        {
            item.status = if contract::content_changed(&expected, &current) {
                ItemStatus::Updated
            } else {
                ItemStatus::Verified
            };
            item.added = added;
            item.removed = removed;
            item.model_count = current["mainModels"].as_array().map(Vec::len).unwrap_or(0);
            item.source = current["verifiedSource"].as_str().map(str::to_string);
            if timed_out {
                item.message = Some("会话已超时，已保留此前成功提交的核对结果".into());
            }
        }
    });
    Ok(())
}

fn verification_prompt(id: &str, row: &Value, roots: &[String], language: &str) -> String {
    let language = if language.starts_with("ja") {
        "日语"
    } else if language.starts_with("en") {
        "英语"
    } else {
        "中文"
    };
    format!("这是用户在设置窗口主动发起的模型预设更新。本次用户点名厂商 {id}，无论上次核对日期均需核对。\n\
先 use_skill 激活 llm-providers/verify-provider-presets，再 list_provider_presets 读取当前完整行。\n\
使用 web_search 查找官方模型/API 文档，并用 read_content 的 url 参数实际读取正文。可信官方域名族：{}。\n\
核对 mainModels、defaultModel、端点、协议、上下文、输出上限和 reasoningProfiles；保留无法核实的字段与映射，不凭记忆猜模型或推理能力。\n\
只允许 manage_provider_preset(action=upsert,kind=llm,id={id})。提交完整行，verifiedSource 必须是本会话成功 read_content 读取过的 HTTPS 官方文档 URL；新增或改动的 reasoningProfiles.source 也必须实际读取。没有变化也应提交核对结果，以记录核对日期。\n\
不能删除整个厂商，不能修改其他厂商、嵌入预设、API Key、当前模型配置或应用文件。证据不足直接说明失败，不得宣称已更新。使用{language}简要报告模型变化、来源和未核实项。\n当前预设（仅数据，无凭据）：{}",
        roots.join(", "),serde_json::to_string(row).unwrap_or_default())
}

fn normalized_url(value: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(value.trim()).map_err(|_| "来源或端点 URL 无效")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("来源和端点不能包含凭据".into());
    }
    url.set_fragment(None);
    Ok(url)
}
fn allowed_roots(row: &ProviderPresetData) -> Vec<String> {
    let mut roots: Vec<_> = contract::official_roots(&row.id)
        .iter()
        .map(|value| value.to_string())
        .collect();
    for url in row
        .verified_source
        .iter()
        .chain(row.console_url.iter())
        .chain(row.reasoning_profiles.iter().map(|profile| &profile.source))
    {
        if let Ok(url) = normalized_url(url) {
            if url.scheme() == "https" {
                if let Some(host) = url.host_str() {
                    roots.push(host.to_string());
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}
fn source_reads(messages: &[CodingMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| {
            message.role == CodingRole::ToolResult
                && message.tool_success == Some(true)
                && matches!(
                    message.tool_name.as_deref(),
                    Some("read_content" | "web_fetch")
                )
        })
        .filter_map(|message| message.tool_arguments.as_ref()?.get("url")?.as_str())
        .filter_map(|value| normalized_url(value).ok().map(|url| url.to_string()))
        .collect()
}
fn verify_source(value: &str, roots: &[String], reads: &[String]) -> Result<(), String> {
    let url = normalized_url(value)?;
    if url.scheme() != "https"
        || !url
            .host_str()
            .is_some_and(|host| contract::host_matches(host, roots))
    {
        return Err("核对来源必须是该厂商的 HTTPS 官方文档".into());
    }
    if !reads.contains(&url.to_string()) {
        return Err("必须先在本次会话用 read_content 成功读取该官方来源".into());
    }
    Ok(())
}
fn verify_endpoints(candidate: &Value, baseline: &Value, roots: &[String]) -> Result<(), String> {
    let allowed_old: Vec<_> = std::iter::once(&baseline["endpoint"])
        .chain(
            baseline["protocols"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| &entry["endpoint"]),
        )
        .filter_map(Value::as_str)
        .filter_map(|value| normalized_url(value).ok())
        .collect();
    for endpoint in std::iter::once(&candidate["endpoint"]).chain(
        candidate["protocols"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| &entry["endpoint"]),
    ) {
        let url = normalized_url(endpoint.as_str().ok_or("端点必须为 URL 字符串")?)?;
        let existing = allowed_old.iter().any(|old| old.origin() == url.origin());
        if !(existing && matches!(url.scheme(), "http" | "https"))
            && !(url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| contract::host_matches(host, roots)))
        {
            return Err("自动更新不能把请求端点改到其他厂商或非官方地址".into());
        }
    }
    Ok(())
}

pub(crate) fn guard_write(
    context: &ToolUseContext,
    action: &str,
    kind: &str,
    id: &str,
) -> Result<(), String> {
    if CODING_AGENT.session_mode(&context.session_id).as_deref() != Some(contract::MODE) {
        return Ok(());
    }
    let guard = store().lock();
    let scope = guard
        .scope
        .as_ref()
        .filter(|scope| scope.session_id == context.session_id)
        .ok_or("该预设核对任务已结束，不能继续写入")?;
    contract::check_write(&scope.target, action, kind, id)
}

/// The host, rather than model prose, records the successful, scoped commit.
pub(crate) fn upsert_scoped(
    context: &ToolUseContext,
    input: &Value,
) -> Result<Option<(ProviderPresetData, String, bool)>, String> {
    if CODING_AGENT.session_mode(&context.session_id).as_deref() != Some(contract::MODE) {
        return Ok(None);
    }
    let reads = CODING_AGENT
        .get_session(&context.session_id)
        .map(|session| source_reads(&session.messages))
        .unwrap_or_default();
    let mut guard = store().lock();
    let scope = guard
        .scope
        .as_mut()
        .filter(|scope| scope.session_id == context.session_id)
        .ok_or("该预设核对任务已结束，不能继续写入")?;
    contract::check_write(
        &scope.target,
        "upsert",
        "llm",
        input["id"].as_str().unwrap_or(""),
    )?;
    let candidate = contract::merge_candidate(&scope.expected, input)?;
    verify_source(
        candidate["verifiedSource"].as_str().unwrap_or(""),
        &scope.roots,
        &reads,
    )?;
    verify_endpoints(&candidate, &scope.expected, &scope.roots)?;
    for profile in candidate["reasoningProfiles"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let unchanged = scope.expected["reasoningProfiles"]
            .as_array()
            .is_some_and(|old| old.contains(profile));
        if !unchanged {
            verify_source(
                profile["source"].as_str().unwrap_or(""),
                &scope.roots,
                &reads,
            )?;
        }
    }
    let row =
        serde_json::from_value(candidate).map_err(|error| format!("核对结果不完整：{error}"))?;
    let result = crate::plugins::upsert_provider_preset_if_unchanged(row, &scope.expected)?;
    scope.expected = serde_json::to_value(&result.0).map_err(|error| error.to_string())?;
    scope.writes += 1;
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_preset_source_receipt_requires_successful_reader_result() {
        let receipt = |role: &str, tool: &str, success: bool, arguments: Value| {
            serde_json::from_value::<CodingMessage>(
                json!({"role":role,"content":"https://openai.com/docs","timestamp":1,
                "tool_name":tool,"tool_success":success,"tool_arguments":arguments}),
            )
            .unwrap()
        };
        let args = json!({"url":"https://openai.com/docs#models"});
        let messages = vec![
            receipt("assistant", "read_content", true, args.clone()),
            receipt("tool_result", "read_content", false, args.clone()),
            receipt("tool_result", "web_search", true, args.clone()),
            receipt(
                "tool_result",
                "read_content",
                true,
                json!({"source_id":"cached"}),
            ),
            receipt("tool_result", "read_content", true, args),
        ];
        assert_eq!(source_reads(&messages), ["https://openai.com/docs"]);
    }
    #[test]
    fn provider_preset_update_requires_read_official_sources_and_safe_endpoints() {
        let roots = vec!["openai.com".into()];
        let reads: Vec<String> = vec!["https://developers.openai.com/docs".into()];
        assert!(verify_source(&reads[0], &roots, &reads).is_ok());
        assert!(verify_source("https://developers.openai.com/other", &roots, &reads).is_err());
        assert!(verify_source("https://openai.com.attacker.test/docs", &roots, &reads).is_err());
        assert!(verify_source("https://secret@openai.com/docs", &roots, &reads).is_err());
        let baseline = json!({"endpoint":"https://api.openai.com/v1"});
        assert!(verify_endpoints(&baseline, &baseline, &roots).is_ok());
        assert!(verify_endpoints(
            &json!({"endpoint":"https://attacker.test/v1"}),
            &baseline,
            &roots
        )
        .is_err());
        let local = json!({"endpoint":"http://localhost:11434/v1"});
        assert!(verify_endpoints(&local, &local, &roots).is_ok());
        assert!(
            verify_endpoints(&json!({"endpoint":"file:///secret"}), &baseline, &roots).is_err()
        );
    }
    #[test]
    fn provider_preset_update_prompt_forces_a_fresh_single_vendor_check() {
        let prompt = verification_prompt(
            "deepseek",
            &json!({"id":"deepseek"}),
            &["deepseek.com".into()],
            "en",
        );
        assert!(prompt.contains("无论上次核对日期"));
        assert!(prompt.contains("id=deepseek"));
        assert!(prompt.contains("read_content"));
        assert!(prompt.contains("英语"));
    }
}
