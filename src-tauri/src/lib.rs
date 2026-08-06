#![recursion_limit = "512"]

pub mod asset_crypto;
pub mod brain;
pub mod bundle_reader;
pub mod character_behavior;
pub mod character_registry;
pub mod commands;
pub mod config;
pub mod conversation;
pub mod cross_character;
pub mod diary;
pub mod dialogue;
pub mod emotion;
pub mod engine;
pub mod error;
pub mod feature_flags;
pub mod hooks;
pub mod i18n;
pub mod memory;
pub mod messages;
pub mod mind;
pub mod metrics;
pub mod network;
pub mod persona;
pub mod self_state;
pub mod pet_controller;
pub mod pipeline;
pub mod presence;
pub mod proactive;
pub mod providers;
pub mod psychology;
pub mod research;
pub mod resilience;
pub mod speech;
pub mod state;
pub mod tools;
pub mod translation;
pub mod types;
pub mod utils;
pub mod world;

use std::str::FromStr;
use std::sync::Arc;

use tauri::{Emitter, Manager};

use crate::state::AppState;
use serde_json::json;

/// URL percent-decoding（处理中文文件名等非 ASCII 字符）
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &bytes[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(hex).unwrap_or("00"),
                16,
            ) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

pub fn run() {
    init_logging();

    let app_state = AppState::new();
    let lipsync_runtime = Arc::new(commands::live2d_lipsync::LipsyncRuntime::new());

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("model", |_app, request, responder| {
            use tauri::http::{Response, StatusCode};

            let uri = request.uri();
            let raw_path = uri.path().trim_start_matches('/').to_string();

            // URL 解码（处理中文文件名，如表情文件"爱心眼.exp3.json"）
            let path = percent_decode(&raw_path);

            if path.is_empty() {
                responder.respond(
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(b"empty path".to_vec())
                        .unwrap(),
                );
                return;
            }

            // 从已解密解压的 Bundle 内存缓存中按路径取资源
            match crate::bundle_reader::get(&path) {
                Some(plaintext) => {
                    let ct = crate::bundle_reader::content_type(&path);
                    responder.respond(
                        Response::builder()
                            .header("Content-Type", ct)
                            .header("Cache-Control", "no-cache")
                            .header("Access-Control-Allow-Origin", "*")
                            .body(plaintext)
                            .unwrap(),
                    );
                }
                None => {
                    tracing::warn!("[model://] resource not found: {} (raw: {})", path, raw_path);
                    responder.respond(
                        Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .header("Access-Control-Allow-Origin", "*")
                            .body(format!("not found: {}", path).into_bytes())
                            .unwrap(),
                    );
                }
            }
        })
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 已有实例运行时唤起活跃角色窗口并聚焦。
            // "main" 现在是无 UI 的隐藏控制器窗口，唤起它用户看不到任何反馈。
            // 这里依次尝试：活跃角色 → 任一在线角色 → 兜底 main。
            let target_window = {
                let state = app.state::<std::sync::Arc<AppState>>();
                let chars = state.characters.read();
                let active_id = state.active_character_id.read().clone();
                // 1. 活跃角色且在线
                chars
                    .get(&active_id)
                    .filter(|c| *c.online.read())
                    .map(|c| c.id.clone())
                    .or_else(|| {
                        // 2. 任一在线角色
                        chars
                            .values()
                            .find(|c| *c.online.read())
                            .map(|c| c.id.clone())
                    })
            };
            let label = target_window.as_deref().unwrap_or("main");
            if let Some(window) = app.get_webview_window(label) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let state = match app.try_state::<std::sync::Arc<AppState>>() {
                        Some(s) => s,
                        None => return,
                    };
                    let pressed = event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed;

                    // 文字快捷键：短按立即触发文本输入，长按 400ms 触发语音输入（仅 vivian/nana）
                    // 互斥：任一角色快捷键按住期间，其他角色快捷键按下事件被忽略，
                    // 避免 voice_shortcut_timer 单槽位被覆盖导致旧计时器无法 abort。
                    let text_map = state.text_shortcuts.lock().clone();
                    for (role, sc) in &text_map {
                        if let Ok(parsed) = tauri_plugin_global_shortcut::Shortcut::from_str(sc) {
                            if shortcut == &parsed {
                                if pressed {
                                    // 互斥检查：已有其他角色按住则忽略本次按下
                                    {
                                        let mut active = state.active_shortcut_role.lock();
                                        if let Some(active_role) = active.as_ref() {
                                            if active_role != role {
                                                return;
                                            }
                                        } else {
                                            *active = Some(role.clone());
                                        }
                                    }
                                    // 短按立即触发文本输入
                                    let event_name = match role.as_str() {
                                        "vivian" => "input:shortcut:vivian",
                                        "nana" => "input:shortcut:nana",
                                        "broadcast" => "input:shortcut:broadcast",
                                        _ => return,
                                    };
                                    let _ = app.emit(event_name, serde_json::json!({}));
                                    // vivian/nana 启动长按计时器，满 1 秒触发语音输入
                                    if role == "vivian" || role == "nana" {
                                        let app_handle = app.clone();
                                        let char_id = role.clone();
                                        // 当前回调运行在全局快捷键 hook 线程，非 Tokio runtime 上下文
                                        let handle = tauri::async_runtime::spawn(async move {
                                            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                            let _ = app_handle.emit(
                                                "input:voice_shortcut",
                                                serde_json::json!({ "character_id": char_id }),
                                            );
                                        });
                                        *state.voice_shortcut_timer.lock() = Some(handle);
                                    }
                                } else {
                                    // 松开时取消长按计时器，并释放互斥锁
                                    // 仅当松开的角色 == 当前按住的角色时才释放，
                                    // 防止被忽略的角色的松开事件误释放他人持有的锁
                                    let mut active = state.active_shortcut_role.lock();
                                    let is_owner = active.as_deref() == Some(role.as_str());
                                    if is_owner {
                                        if let Some(h) = state.voice_shortcut_timer.lock().take() {
                                            h.abort();
                                        }
                                        *active = None;
                                    }
                                }
                                return;
                            }
                        }
                    }
                })
                .build()
        )
        .plugin(tauri_plugin_os::init())
        .manage(Arc::new(app_state))
        .manage(lipsync_runtime)
        .invoke_handler(tauri::generate_handler![
            commands::characters::list_characters,
            commands::characters::set_character_online,
            commands::characters::set_character_offline,
            commands::characters::set_active_character,
            commands::characters::get_active_character,
            commands::characters::get_character_model_path,
            commands::chat::send_message,
            commands::chat::is_main_api_configured,
            commands::chat::send_message_stream,
            commands::chat::stop_generation,
            commands::chat::send_image_message,
            commands::chat::extract_file_text,
            commands::chat::wake_from_presence,
            commands::memory::get_memories,
            commands::memory::get_graph_timeline,
            commands::memory::get_memories_range,
            commands::memory::add_memory,
            commands::memory::delete_memory,
            commands::memory::hard_delete_memory,
            commands::memory::restore_memory,
            commands::memory::list_recycle_bin,
            commands::memory::purge_recycle_entry,
            commands::memory::clear_recycle_bin,
            commands::memory::purge_expired_recycle_bin,
            commands::memory::clear_all_memories,
            commands::memory::get_memory_summary,
            commands::memory::search_memories,
            commands::memory::get_memories_all,
            commands::memory::get_common_memories,
            commands::memory::add_common_memory,
            commands::memory::delete_common_memory,
            commands::memory::clear_common_memories,
            commands::memory::list_unified_events,
            commands::memory::list_world_facts,
            commands::memory::list_relationship_facts,
            commands::memory::get_social_state_snapshot,
            commands::memory::rebuild_memory_embeddings,
            commands::mind::get_mind_state,
            commands::mind::get_world_snapshot,
            commands::mind::list_beliefs,
            commands::mind_inspector::get_recent_reasoning_traces,
            commands::mind_inspector::get_last_prompt_breakdown,
            commands::mind_inspector::get_prompt_template_preview,
            commands::mind_inspector::get_sessions,
            commands::mind_inspector::get_prompt_section_schema,
            commands::user_facts::get_user_facts,
            commands::user_facts::set_user_fact,
            commands::user_facts::pin_user_fact,
            commands::user_facts::delete_user_fact,
            commands::user_facts::get_user_fact_types,
            commands::tools::list_tools,
            commands::tools::get_tool_history,
            commands::tools::confirm_tool_execution,
            commands::tools::list_mcp_servers,
            commands::tools::list_mcp_server_configs,
            commands::tools::add_mcp_server,
            commands::tools::remove_mcp_server,
            commands::config::get_config,
            commands::config::set_config,
            commands::config::get_all_config,
            commands::config::save_config,
            commands::config::reload_config,
            commands::config::test_network_connection,
            commands::config::get_image_data_url,
            commands::emotion::get_current_mood,
            commands::emotion::get_mood_history,
            commands::emotion::get_recent_events,
            commands::emotion::get_psychology_state,
            commands::emotion::apply_user_interaction,
            commands::emotion::psychology_micro_tick,
            commands::emotion::mood_expression_tick,
            commands::emotion::auto_expression_tick,
            commands::emotion::trigger_system_event,
            commands::emotion::set_emotion_expression,
            commands::emotion::analyze_emotion_deep,
            commands::emotion::analyze_emotion_batch,
            commands::emotion::analyze_emotion_instant,
            commands::engine::play_motion,
            commands::engine::set_expression,
            commands::engine::get_model_info,
            commands::engine::get_display_scale,
            commands::engine::get_model_path,
            commands::engine::get_model_url,
            commands::engine::trigger_idle_action,
            commands::engine::drain_pet_actions,
            commands::engine::set_avoid_mouse,
            commands::engine::try_wake_greeting,
            commands::engine::list_available_models,
            commands::engine::get_current_model,
            commands::speech::start_recognition,
            commands::speech::stop_recognition,
            commands::speech::get_recognition_status,
            commands::speech::update_asr_config,
            commands::speech::update_text_shortcuts,
            commands::speech::start_whisper_service,
            commands::speech::stop_whisper_service,
            commands::speech::get_whisper_service_status,
            commands::system::is_initialized,
            commands::system::exit_app,
            commands::system::factory_reset,
            commands::system::reinitialize,
            commands::system::get_system_info,
            commands::system::get_running_processes,
            commands::system::open_application,
            commands::system::close_application,
            commands::proactive::get_proactive_status,
            commands::diary::get_diary_entries,
            commands::diary::get_diary_range,
            commands::diary::generate_diary,
            commands::diary::generate_diary_intelligent,
            commands::diary::get_diary_entry,
            commands::diary::delete_diary_entry,
            commands::diary::get_diary_config,
            commands::diary::set_diary_config,
            commands::diary::get_diary_stats,
            commands::diary::update_diary_entry,
            commands::diary::check_missed_diaries,
            commands::diary::export_diaries_markdown,
            commands::diary::should_trigger_diary,
            commands::diary::get_diary_entries_all,
            commands::diary::get_common_diary_entries,
            commands::diary::add_common_diary_entry,
            commands::diary::delete_common_diary_entry,
            commands::diary::clear_common_diary_entries,
            commands::metrics::get_metrics_summary,
            commands::metrics::persist_metrics,
            commands::metrics::reset_metrics,
            commands::metrics::increment_metric,
            commands::metrics::set_gauge_metric,
            commands::history::get_chat_history,
            commands::history::clear_chat_history,
            commands::history::get_chat_history_all,
            commands::history::get_latest_previews,
            commands::history::search_chat_history,
            commands::chat::update_last_assistant_timestamp,
            commands::window::set_window_position,
            commands::window::get_window_position,
            commands::window::get_cursor_position,
            commands::window::start_cursor_tracking,
            commands::window::stop_cursor_tracking,
            commands::window::start_window_drag,
            commands::window::stop_window_drag,
            commands::window::suspend_click_through,
            commands::window::resume_click_through,
            commands::window::get_click_through_status,
            commands::window::start_side_chat_edge_watcher,
            commands::window::set_side_chat_locked,
            commands::window::set_side_chat_input_open,
            commands::window::show_side_chat_animated,
            commands::window::start_side_chat_mouse_hook,
            commands::window::toggle_always_on_top,
            commands::window::set_window_size,
            commands::window::set_window_rect,
            commands::window::get_window_size,
            commands::window::set_window_opacity,
            commands::window::show_window,
            commands::window::hide_window,
            commands::window::toggle_window_visibility,
            commands::window::minimize_to_tray,
            commands::window::restore_from_tray,
            commands::window::focus_window,
            commands::window::open_child_window,
            commands::window::close_child_window,
            commands::window::list_child_windows,
            commands::window::set_window_resizable,
            commands::window::set_skip_taskbar,
            commands::window::center_window,
            commands::window::is_foreground_fullscreen,
            commands::window::find_safe_position,
            commands::window::debug_log,
            commands::system_tray::set_tray_tooltip,
            commands::system_tray::update_tray_icon,
            commands::system_tray::show_tray_message,
            commands::system_tray::is_tray_visible,
            commands::system_tray::set_tray_visible,
            commands::system_tray::set_tray_menu_check,
            commands::system_tray::destroy_tray,
            commands::live2d_lipsync::start_lipsync,
            commands::live2d_lipsync::update_mouth_shape,
            commands::live2d_lipsync::stop_lipsync,
            commands::live2d_lipsync::get_lipsync_status,
            commands::relationship::get_relationship,
            commands::relationship::get_relationship_stage,
            commands::relationship::get_milestones,
            commands::relationship::reset_relationship,
            commands::persona::get_persona,
            commands::persona::get_persona_name,
            commands::persona::get_persona_tagline,
            commands::persona::get_persona_sections,
            commands::persona::set_persona_section,
            commands::persona::reset_persona_section,
            commands::persona::get_few_shot_examples,
            commands::persona::set_few_shot_examples,
            commands::persona::get_style_prompt,
            commands::persona::list_persona_cards,
            commands::persona::get_active_persona_card,
            commands::persona::create_persona_card,
            commands::persona::update_persona_card,
            commands::persona::switch_persona_card,
            commands::persona::archive_persona_card,
            commands::persona::delete_persona_card,
            commands::persona::get_persona_events,
            commands::persona::get_persona_card_cooldowns,
            commands::proactive::start_proactive,
            commands::proactive::stop_proactive,
            commands::proactive::proactive_tick,
            commands::proactive::drain_proactive_messages,
            commands::proactive::mark_proactive_ignored,
            commands::proactive::update_proactive_config,
            commands::proactive::update_world_config,
            commands::proactive::auto_detect_location,
            commands::presence::get_presence_state,
            commands::presence::get_all_presence_states,
            commands::presence::set_presence_state,
            commands::tts::get_tts_config,
            commands::tts::set_tts_config,
            commands::tts::speak_text,
            commands::tts::stop_speaking,
            commands::tts::get_speaking_status,
            commands::tts::prewarm_tts,
            commands::tts::prefetch_tts,
            commands::tts::list_tts_voices,
            commands::tts::test_tts,
            commands::tts::start_gpt_sovits_service,
            commands::tts::stop_gpt_sovits_service,
            commands::tts::get_gpt_sovits_service_status,
            commands::tts::start_fish_speech_service,
            commands::tts::stop_fish_speech_service,
            commands::tts::get_fish_speech_service_status,
            commands::tts::test_translation,
            commands::tts::list_gpt_sovits_models,
            commands::ollama::start_ollama,
            commands::ollama::stop_ollama,
            commands::ollama::get_ollama_status,
            commands::ollama::pull_ollama_model,
            commands::ollama::fix_ollama_permission,
            commands::ollama::list_ollama_models,
            commands::realtime_voice::get_realtime_status,
            commands::realtime_voice::start_realtime_call,
            commands::realtime_voice::stop_realtime_call,
            commands::realtime_voice::get_realtime_config,
            commands::realtime_voice::set_realtime_config,
            commands::realtime_voice::send_realtime_text,
            commands::rag::add_rag_document,
            commands::rag::delete_rag_document,
            commands::rag::list_rag_documents,
            commands::rag::search_rag,
            commands::rag::clear_rag,
            commands::environment::get_environment_info,
            commands::environment::get_current_state,
            commands::environment::get_user_activity,
            commands::environment::update_environment,
            commands::environment::get_startup_greeting,
            commands::config::save_user_avatar,
            commands::config::get_user_avatar_data_url,
            commands::config::clear_user_avatar,
            commands::config::get_settings_catalog,
            commands::todo::list_todos,
            commands::todo::add_todo_item,
            commands::todo::update_todo_item,
            commands::todo::complete_todo_item,
            commands::todo::delete_todo_item,
            commands::todo::list_scheduled_tasks,
            commands::todo::add_scheduled_reminder,
            commands::todo::cancel_scheduled_task,
            commands::todo::pause_scheduled_task,
            commands::todo::resume_scheduled_task,
        ])
        .setup(|app| {
            // 初始化 Windows Job Object（KILL_ON_JOB_CLOSE 兜底）
            // 必须在任何子进程启动前完成，确保所有子进程都能被绑定
            crate::utils::job_object::init();

            // 初始化系统托盘
            if let Err(e) = commands::system_tray::setup_tray(app.handle()) {
                tracing::warn!("系统托盘创建失败: {e}");
            }

            // 初始化 Bundle（release 模式从 vivian.bundle.enc 加载解密解压到内存）
            if !cfg!(debug_assertions) {
                let bundle_path = crate::utils::path::get_bundle_path();
                if let Err(e) = crate::bundle_reader::init(&bundle_path) {
                    tracing::error!("[bundle] 初始化失败: {}", e);
                }
            }

            // Inject AppHandle into shared tool system (not dependent on character instances)
            {
                let state = app.state::<Arc<AppState>>();
                state.tool_system.set_app_handle(app.handle().clone());
                crate::tools::builtin::todo_tools::set_app_handle(app.handle().clone());
                crate::tools::builtin::pet_tools::set_app_handle(app.handle().clone());
                crate::tools::builtin::presence_tools::set_app_handle(app.handle().clone());
                crate::tools::builtin::web_search_tool::set_app_handle(app.handle().clone());
                crate::tools::builtin::share_link_tool::set_app_handle(app.handle().clone());
                crate::tools::builtin::weather_tools::set_app_handle(app.handle().clone());
                crate::mind::reasoning_trace::set_app_handle(app.handle().clone());
                crate::memory::ollama_service::set_app_handle(app.handle().clone());
                crate::cross_character::CROSS_CHARACTER_BUS.initialize(
                    app.handle().clone(),
                );
                crate::tools::services::TodoService::load();
                commands::proactive::load_arbitration_state();
                commands::speech::start_asr_event_forwarder(
                    app.handle().clone(),
                    state.asr.clone(),
                );
                commands::speech::register_text_shortcuts(app.handle().clone(), &state);
            }

            // 异步初始化所有角色实例（不阻塞 setup）
            // 完成后 emit `app:ready`，前端据此触发启动问候等依赖 Brain 的流程
            {
                let handle = app.handle().clone();
                let state = app.state::<Arc<AppState>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    match state.initialize().await {
                        Ok(()) => {
                            // 启动所有在线角色的 PetController（空闲定时器等需在 tokio runtime 内）
                            {
                                let chars = state.characters.read();
                                for instance in chars.values() {
                                    if *instance.online.read() {
                                        let pc = instance.pet_controller.clone();
                                        tauri::async_runtime::spawn(async move {
                                            pc.start();
                                        });
                                    }
                                }
                            }

                            // 注入 AppHandle，启用路由回退事件发送
                            if let Some(router) = state.model_router.read().as_ref() {
                                router.set_app_handle(handle.clone());
                            }
                            // 注入 AppHandle + Router 到全局事件账本（启用前端通知 + LLM 摘要压缩）
                            {
                                let ledger = crate::memory::unified_event_ledger();
                                ledger.set_app_handle(handle.clone());
                                if let Some(router) = state.model_router.read().as_ref() {
                                    ledger.set_router(Arc::new(router.clone()));
                                }
                            }
                            // 注入 AppHandle 到所有角色的 DialogueManager
                            for instance in state.characters.read().values() {
                                instance.brain.dialogue.set_app_handle(handle.clone());
                                // 注入 AppHandle 到 MemoryManager，启用记忆变更后的 `memory:updated` 事件
                                instance.brain.memory.set_app_handle(handle.clone());
                                // 注入嵌入进度回调到即时情绪分类器
                                if let Some(ref clf) = instance.brain.embedding_classifier {
                                    let emit_handle = handle.clone();
                                    clf.set_progress_callback(Arc::new(move |completed, total| {
                                        let _ = emit_handle.emit("embedding:progress", serde_json::json!({
                                            "current": completed,
                                            "total": total,
                                        }));
                                    }));
                                }
                            }

                            // 启动时立即为所有在线角色触发一次 current_thought 合成
                            // 避免 Mind Inspector 首次打开时显示空白
                            {
                                let language = state.config.read().get_all().base.language.clone();
                                let lang_code = if language.starts_with("ja") {
                                    "ja"
                                } else if language.starts_with("en") {
                                    "en"
                                } else {
                                    "zh"
                                };
                                let chars = state.characters.read();
                                for instance in chars.values() {
                                    if *instance.online.read() {
                                        let mind = instance.brain.mind.clone();
                                        let router = instance.brain.router.clone();
                                        let world_provider = instance.brain.world_provider.clone();
                                        let lang = lang_code.to_string();
                                        tauri::async_runtime::spawn(async move {
                                            crate::mind::thought_synthesis::refresh_current_thought(
                                                &mind, &router, &world_provider, &lang,
                                            ).await;
                                        });
                                    }
                                }
                            }

                            // 注册所有角色的 TtsManager 到全局 SpeechPlanner,并启动 pump 循环
                            {
                                // 先 clone 出 (id, tts) 对,释放 read guard 后再 async 注册
                                let tts_list: Vec<_> = state
                                    .characters
                                    .read()
                                    .iter()
                                    .map(|(id, inst)| (id.clone(), inst.brain.tts.clone()))
                                    .collect();

                                let planner = crate::speech::get_planner().await;
                                for (id, tts) in tts_list {
                                    planner.register(&id, tts).await;
                                }

                                // 设置 Planner 事件回调:将 PlannerEvent 转成 tauri 事件发射给前端
                                // presentation:start / presentation:stop 统一表情/动作/气泡/视线时序
                                let app_for_planner = handle.clone();
                                planner.set_event_callback(std::sync::Arc::new(move |event| {
                                    use crate::speech::PlannerEvent;
                                    use tauri::Emitter;
                                    match event {
                                        PlannerEvent::Start {
                                            speaker_id,
                                            presentation,
                                            text,
                                        } => {
                                            let _ = app_for_planner.emit(
                                                "presentation:start",
                                                serde_json::json!({
                                                    "speaker_id": speaker_id,
                                                    "presentation": presentation,
                                                    "text": text,
                                                }),
                                            );
                                        }
                                        PlannerEvent::Stop { speaker_id } => {
                                            let _ = app_for_planner.emit(
                                                "presentation:stop",
                                                serde_json::json!({
                                                    "speaker_id": speaker_id,
                                                }),
                                            );
                                        }
                                    }
                                }));

                                crate::speech::start_pump_loop().await;
                                tracing::info!("[SpeechPlanner] pump 循环已启动");
                            }

                            // 注册 presence 后台任务分发钩子
                            // - Busy 进入时 spawn 知识采集任务（LLM 决定查什么 → WebSearcher → 写入 Knowledge）
                            // - Rest 进入时 spawn 记忆沉淀任务（跑 ConsolidationPipeline 三阶段）
                            // 任务进行中用户唤醒会被延迟，等任务结束自动切回 Online（见 PresenceManager::transition）
                            for instance in state.characters.read().values() {
                                let brain = instance.brain.clone();
                                let char_id = brain.char_id.clone();
                                let presence = brain.presence.clone();
                                let app = handle.clone();
                                let router = brain.router.clone();
                                let memory = brain.memory.clone();
                                let pipeline = brain.chat_chain.as_ref().map(|c| c.pipeline.clone());

                                // Busy 钩子：知识采集
                                let presence_for_busy = presence.clone();
                                let app_for_busy = app.clone();
                                let router_for_busy = router.clone();
                                let memory_for_busy = memory.clone();
                                let char_id_for_busy = char_id.clone();
                                let proactive_for_busy = brain.proactive.clone();
                                presence.set_busy_task_spawner(std::sync::Arc::new(move || {
                                    crate::presence::spawn_knowledge_acquisition(
                                        char_id_for_busy.clone(),
                                        app_for_busy.clone(),
                                        presence_for_busy.clone(),
                                        router_for_busy.clone(),
                                        memory_for_busy.clone(),
                                        proactive_for_busy.clone(),
                                    );
                                }));

                                // Rest 钩子：记忆沉淀 + 用户认知整理
                                if let Some(pipeline) = pipeline.clone() {
                                    let presence_for_rest = presence.clone();
                                    let app_for_rest = app.clone();
                                    let memory_for_rest = memory.clone();
                                    let char_id_for_rest = char_id.clone();
                                    let router_for_rest = router.clone();
                                    let mind_for_rest = brain.mind.clone();
                                    let world_state_for_rest = brain.world_state.clone();
                                    presence.set_rest_task_spawner(std::sync::Arc::new(move || {
                                        crate::presence::spawn_memory_consolidation(
                                            char_id_for_rest.clone(),
                                            app_for_rest.clone(),
                                            presence_for_rest.clone(),
                                            pipeline.clone(),
                                            memory_for_rest.clone(),
                                        );
                                        // 用户认知整理：从行为日志提炼习惯 Belief（独立任务，不阻塞记忆沉淀）
                                        crate::presence::spawn_user_cognition_consolidation(
                                            char_id_for_rest.clone(),
                                            app_for_rest.clone(),
                                            router_for_rest.clone(),
                                            mind_for_rest.clone(),
                                            world_state_for_rest.clone(),
                                            30,
                                        );
                                    }));
                                }
                            }

                            // 为每个在线角色创建独立窗口
                            //
                            // 窗口尺寸按角色模型画布比例计算：
                            // - Vivian: 355.33×411.33
                            // - Nana: 422×489.33
                            const WIN_GAP: f64 = 15.0; // 窗口间距
                            let chars = state.characters.read();
                            let mut offset_x = 0.0f64;
                            for instance in chars.values() {
                                if !*instance.online.read() {
                                    continue;
                                }
                                let label = instance.id.clone();
                                // 若窗口已存在则跳过（热重载场景）
                                if handle.get_webview_window(&label).is_some() {
                                    continue;
                                }
                                // 按角色决定窗口尺寸
                                let (win_w, win_h) = match instance.id.as_str() {
                                    "nana" => (422.0, 489.33),
                                    "vivian" => (355.33, 411.33),
                                    _ => (345.0, 400.0),
                                };
                                let builder = tauri::WebviewWindowBuilder::new(
                                    &handle,
                                    &label,
                                    tauri::WebviewUrl::App("index.html".into()),
                                )
                                .title(&instance.name)
                                .inner_size(win_w, win_h)
                                .position(offset_x, 100.0)
                                .transparent(true)
                                .decorations(false)
                                .always_on_top(true)
                                .skip_taskbar(true)
                                .shadow(false)
                                .visible(true);

                                match builder.build() {
                                    Ok(win) => {
                                        // 确保窗口初始状态可响应鼠标事件。
                                        // 光标追踪线程由前端 Live2DCanvas 异步启动，
                                        // 在启动前窗口必须能接收事件，否则右键/拖动/缩放等交互全部失效。
                                        let _ = win.set_ignore_cursor_events(false);
                                        instance.pet_controller.set_main_window(win.clone());
                                        #[cfg(debug_assertions)]
                                        win.open_devtools();
                                        tracing::info!("[lib] 已创建角色窗口: {} ({})", label, win.label());

                                        // 按 presence 状态控制初始可见性：
                                        // 若 PresenceManager 从持久化恢复为 Offline，
                                        // 后端直接 hide 窗口，避免依赖前端 useEffect 时序。
                                        // 与 App.tsx 的 hideForOffline 形成双保险。
                                        let presence = &instance.brain.presence;
                                        let current = presence.current();
                                        if matches!(current, crate::presence::PresenceState::Offline) {
                                            let _ = win.hide();
                                            tracing::info!(
                                                "[lib] 角色 {} presence=Offline，已隐藏窗口",
                                                label
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("[lib] 创建角色窗口 {} 失败: {}", label, e);
                                    }
                                }
                                // 累加偏移：当前窗口宽度 + 间距
                                offset_x += win_w + WIN_GAP;
                            }
                            drop(chars);

                            tracing::info!("所有角色初始化完成，发送 app:ready 事件");
                            let _ = handle.emit("app:ready", ());

                            // 启动时打印当前 web_search provider，便于调试
                            let web_search_cfg = state.config.read().get_all().web_search;
                            tracing::info!(
                                "[WebSearch] 启动 provider={}, max_results={}, timeout={}s",
                                web_search_cfg.provider,
                                web_search_cfg.max_results,
                                web_search_cfg.timeout_secs
                            );

                            // 启动时预取天气：若经纬度已配置，立即刷新全局天气缓存
                            // 这样不必等到用户打开世界页或 proactive tick 才有天气数据
                            {
                                let world_cfg_for_weather =
                                    state.config.read().get_all().world.clone();
                                if world_cfg_for_weather.enable_weather
                                    && world_cfg_for_weather.latitude.is_some()
                                    && world_cfg_for_weather.longitude.is_some()
                                {
                                    let wp = state.world_provider.clone();
                                    tauri::async_runtime::spawn(async move {
                                        tracing::info!("[Startup] 启动天气预取开始");
                                        wp.refresh_weather().await;
                                        tracing::info!("[Startup] 启动天气预取完成");
                                    });
                                }
                            }

                            // 启动时预取音乐：读取系统当前播放状态
                            // 音乐检测不依赖经纬度，始终在启动时执行
                            // 同时启动后台轮询（3s 间隔），让歌曲切换能被及时感知
                            {
                                if state.world_provider.has_music_source() {
                                    // 启动后台 3s 轮询循环（幂等，全局只启动一次）
                                    state.world_provider.start_music_polling();

                                    let wp = state.world_provider.clone();
                                    tauri::async_runtime::spawn(async move {
                                        tracing::info!("[Startup] 启动音乐预取开始");
                                        wp.refresh_music().await;
                                        tracing::info!("[Startup] 启动音乐预取完成");
                                    });
                                }
                            }

                            // 启动后台系统指标轮询（CPU/内存/网速/音量/前台窗口/网络状态）
                            // 全局共享：跨角色只启动一份 Windows Hook 与轮询循环
                            {
                                state.world_provider.start_system_polling();
                                state.world_provider.start_volume_events();
                                state.world_provider.start_foreground_events();
                                state.world_provider.start_network_events();
                            }

                            // 启动时自动定位：若世界感知已启用且缺坐标或缺城市名
                            let world_cfg = state.config.read().get_all().world;
                            let need_coords =
                                world_cfg.latitude.is_none() || world_cfg.longitude.is_none();
                            let need_city = world_cfg.city.is_none();
                            if world_cfg.enable && (need_coords || need_city) {
                                let cfg = state.config.clone();
                                let wp_for_geo = state.world_provider.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Some(info) =
                                        crate::world::geolocation::detect_location().await
                                    {
                                        {
                                            let cm = cfg.read();
                                            let _ = cm.set("world.latitude", json!(info.latitude));
                                            let _ = cm.set("world.longitude", json!(info.longitude));
                                            let _ = cm.set("world.city", json!(info.city));
                                            let _ = cm.set("world.region", json!(info.region));
                                            let _ = cm.set("world.country", json!(info.country));
                                        }
                                        // 更新全局 WorldStateProvider
                                        let c = cfg.read().get_all().world.clone();
                                        wp_for_geo.update_config(c);
                                        wp_for_geo.set_location(crate::world::LocationSnapshot {
                                            latitude: info.latitude,
                                            longitude: info.longitude,
                                            city: info.city.clone(),
                                            region: info.region.clone(),
                                            country: info.country.clone(),
                                        });
                                        // 定位成功后立即预取天气
                                        tracing::info!("[Startup] 自动定位后天气预取开始");
                                        wp_for_geo.refresh_weather().await;
                                        tracing::info!(
                                            "启动时自动定位成功: ({:.4}, {:.4}) 城市: {}",
                                            info.latitude,
                                            info.longitude,
                                            info.city.as_deref().unwrap_or("未知")
                                        );
                                    } else {
                                        tracing::info!("启动时自动定位失败，用户可手动设置经纬度");
                                    }
                                });
                            }

                            // 网络连通性监听：网络变化时重新定位（换 WiFi / 断线重连等）
                            {
                                let cfg = state.config.clone();
                                let wp_for_net = state.world_provider.clone();
                                tauri::async_runtime::spawn(async move {
                                    tracing::info!("[NetworkWatch] 网络连通性监听已启动");
                                    let cancel = crate::utils::cancel_token::cancel_token();
                                    loop {
                                        if cancel.is_cancelled() {
                                            tracing::info!("[NetworkWatch] 收到取消信号，退出");
                                            return;
                                        }
                                        let notify = std::sync::Arc::new(tokio::sync::Notify::new());

                                        let _guard = tokio::task::spawn_blocking({
                                            let n = notify.clone();
                                            move || crate::world::network_watch::subscribe_network_events(n)
                                        })
                                        .await
                                        .unwrap_or(None);

                                        // 等待网络变化事件；若订阅失败则 60s 后重试
                                        if _guard.is_some() {
                                            tokio::select! {
                                                _ = notify.notified() => {}
                                                _ = cancel.cancelled() => return,
                                            }
                                            // 去抖：网络切换瞬间可能连续触发多次事件，等 5s 让连接稳定
                                            tokio::select! {
                                                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                                                _ = cancel.cancelled() => return,
                                            }
                                        } else {
                                            tokio::select! {
                                                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                                                _ = cancel.cancelled() => return,
                                            }
                                            continue;
                                        }

                                        tracing::info!("[NetworkWatch] 检测到网络变化，重新定位...");
                                        if let Some(info) = crate::world::geolocation::detect_location().await {
                                            let old_city = {
                                                let cm = cfg.read();
                                                cm.get_all().world.city.clone()
                                            };
                                            // 城市未变则跳过更新
                                            if old_city == info.city {
                                                tracing::info!("[NetworkWatch] 城市未变化({:?})，跳过", info.city);
                                                continue;
                                            }
                                            tracing::info!(
                                                "[NetworkWatch] 城市变化: {:?} -> {:?}，更新配置",
                                                old_city, info.city
                                            );
                                            {
                                                let cm = cfg.read();
                                                let _ = cm.set("world.latitude", json!(info.latitude));
                                                let _ = cm.set("world.longitude", json!(info.longitude));
                                                let _ = cm.set("world.city", json!(info.city));
                                                let _ = cm.set("world.region", json!(info.region));
                                                let _ = cm.set("world.country", json!(info.country));
                                            }
                                            let c = cfg.read().get_all().world.clone();
                                            wp_for_net.update_config(c);
                                            wp_for_net.set_location(crate::world::LocationSnapshot {
                                                latitude: info.latitude,
                                                longitude: info.longitude,
                                                city: info.city.clone(),
                                                region: info.region.clone(),
                                                country: info.country.clone(),
                                            });
                                            wp_for_net.refresh_weather().await;
                                        } else {
                                            tracing::warn!("[NetworkWatch] 网络变化后重新定位失败");
                                        }
                                    }
                                });
                            }

                            // 定期 IP 定位检查（30 分钟）：补充 NetworkWatch 无法覆盖的场景
                            // （VPN 切换、路由器重启等公网 IP 变化但本地网卡状态不变的情况）
                            {
                                let cfg = state.config.clone();
                                let wp_for_ip = state.world_provider.clone();
                                tauri::async_runtime::spawn(async move {
                                    let cancel = crate::utils::cancel_token::cancel_token();
                                    loop {
                                        if cancel.is_cancelled() {
                                            tracing::info!("[IPCheck] 收到取消信号，退出");
                                            return;
                                        }
                                        tokio::select! {
                                            _ = tokio::time::sleep(std::time::Duration::from_secs(1800)) => {}
                                            _ = cancel.cancelled() => return,
                                        }
                                        let world_enabled = cfg.read().get_all().world.enable;
                                        if !world_enabled {
                                            continue;
                                        }
                                        tracing::debug!("[IPCheck] 定期 IP 定位检查...");
                                        if let Some(info) = crate::world::geolocation::detect_location().await {
                                            let old_city = {
                                                let cm = cfg.read();
                                                cm.get_all().world.city.clone()
                                            };
                                            if old_city == info.city {
                                                continue;
                                            }
                                            tracing::info!(
                                                "[IPCheck] 城市变化: {:?} -> {:?}，更新配置",
                                                old_city, info.city
                                            );
                                            {
                                                let cm = cfg.read();
                                                let _ = cm.set("world.latitude", json!(info.latitude));
                                                let _ = cm.set("world.longitude", json!(info.longitude));
                                                let _ = cm.set("world.city", json!(info.city));
                                                let _ = cm.set("world.region", json!(info.region));
                                                let _ = cm.set("world.country", json!(info.country));
                                            }
                                            let c = cfg.read().get_all().world.clone();
                                            wp_for_ip.update_config(c);
                                            wp_for_ip.set_location(crate::world::LocationSnapshot {
                                                latitude: info.latitude,
                                                longitude: info.longitude,
                                                city: info.city.clone(),
                                                region: info.region.clone(),
                                                country: info.country.clone(),
                                            });
                                        }
                                    }
                                });
                            }

                            // GPT-SoVITS 服务自动启动：检查所有角色，找到第一个配置了
                            // gpt_sovits_auto_start=true 且有安装路径的 TtsConfig，触发服务启动。
                            // start() 本身快速返回（健康检查后台进行），不阻塞主流程。
                            {
                                let state_for_tts = state.clone();
                                tauri::async_runtime::spawn(async move {
                                    let target_cfg = {
                                        let chars = state_for_tts.characters.read();
                                        chars.values()
                                            .map(|c| c.brain.tts.get_config())
                                            .find(|c| {
                                                c.gpt_sovits_auto_start
                                                    && c.gpt_sovits_install_path
                                                        .as_deref()
                                                        .map_or(false, |s| !s.is_empty())
                                            })
                                    };
                                    let Some(tts_config) = target_cfg else {
                                        return;
                                    };
                                    let svc = crate::speech::gpt_sovits_service().await;
                                    match svc.start(&tts_config).await {
                                        Ok(s) => tracing::info!(
                                            "[lib] GPT-SoVITS 自动启动已触发: {:?}",
                                            s.status
                                        ),
                                        Err(e) => tracing::warn!(
                                            "[lib] GPT-SoVITS 自动启动失败: {e}"
                                        ),
                                    }
                                });
                            }

                            // Whisper 本地 ASR 服务自动启动：engine=whisper 且 service_auto_start=true 时触发
                            {
                                let state_for_whisper = state.clone();
                                tauri::async_runtime::spawn(async move {
                                    let asr_cfg = {
                                        let cfg = state_for_whisper.config.read();
                                        cfg.get_all().speech_recognition.clone()
                                    };
                                    if asr_cfg.engine == "whisper" && asr_cfg.whisper.service_auto_start {
                                        crate::commands::speech::maybe_autostart_whisper_service(&state_for_whisper).await;
                                    }
                                });
                            }

                            // Fish Speech 本地 TTS 服务自动启动：engine=fishspeech 且 fish_speech_auto_start=true 时触发
                            {
                                let state_for_fish = state.clone();
                                tauri::async_runtime::spawn(async move {
                                    let target_cfg = {
                                        let chars = state_for_fish.characters.read();
                                        chars.values()
                                            .map(|c| c.brain.tts.get_config())
                                            .find(|c| {
                                                c.engine == crate::speech::TtsEngine::FishSpeech
                                                    && c.fish_speech_auto_start
                                                    && c.fish_speech_install_path
                                                        .as_deref()
                                                        .map_or(false, |s| !s.is_empty())
                                            })
                                    };
                                    let Some(tts_config) = target_cfg else {
                                        return;
                                    };
                                    let svc = crate::speech::fish_speech_service().await;
                                    match svc.start(&tts_config).await {
                                        Ok(s) => tracing::info!(
                                            "[lib] Fish Speech 自动启动已触发: {:?}",
                                            s.status
                                        ),
                                        Err(e) => tracing::warn!(
                                            "[lib] Fish Speech 自动启动失败: {e}"
                                        ),
                                    }
                                });
                            }

                            // Ollama 嵌入服务自动启动：source=local 且 ollama_auto_start=true 时触发
                            {
                                let state_for_ollama = state.clone();
                                tauri::async_runtime::spawn(async move {
                                    let emb_cfg = {
                                        let cfg = state_for_ollama.config.read();
                                        cfg.get_all().memory.embedding.clone()
                                    };
                                    if emb_cfg.source == "local"
                                        && emb_cfg.ollama_auto_start
                                        && !emb_cfg.ollama_path.trim().is_empty()
                                    {
                                        let svc = crate::memory::ollama_service::ollama_service().await;
                                        match svc.start(&emb_cfg.ollama_path).await {
                                            Ok(s) => {
                                                tracing::info!(
                                                    "[lib] Ollama 自动启动已触发: {:?}",
                                                    s.status
                                                );
                                                // 服务就绪后确保目标模型已安装：未安装则自动拉取，
                                                // 权限不足时自动触发 UAC 修复并重试
                                                let target_model = emb_cfg.ollama_model.clone();
                                                if !target_model.is_empty() {
                                                    let ok = crate::memory::ollama_service::OllamaServiceManager::ensure_model_installed(&target_model, &emb_cfg.ollama_path).await;
                                                    if ok {
                                                        tracing::info!(
                                                            "[lib] Ollama 模型 {} 已就绪",
                                                            target_model
                                                        );
                                                    } else {
                                                        tracing::warn!(
                                                            "[lib] Ollama 模型 {} 自动拉取未完成，需手动处理",
                                                            target_model
                                                        );
                                                    }
                                                }
                                                // ensure_model_installed 完成后统一 emit，
                                                // 前端收到时模型状态已是最终结果（不会再误报"未安装"）
                                                crate::memory::ollama_service::emit_ollama_ready_with_model_check().await;
                                            }
                                            Err(e) => tracing::warn!(
                                                "[lib] Ollama 自动启动失败: {e}"
                                            ),
                                        }
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            tracing::error!("角色初始化失败: {e}");
                            let _ = handle.emit("app:ready", ());
                        }
                    }
                });
            }

            #[cfg(debug_assertions)]
            {
                // "main" 现在是无 UI 的隐藏控制器，DevTools 应开在角色窗口上。
                // 角色窗口在异步 initialize 完成后才创建，DevTools 由角色窗口创建循环负责打开。
            }
            Ok(())
        })
        .on_window_event({
            // 全局窗口事件处理：用于管理光标追踪线程的生命周期。
            //
            // 设计原则：光标追踪线程是应用级单例，由"是否存在任何在线角色窗口"
            // 决定其运行/停止，不再由前端 Live2DCanvas 的 mount/unmount 控制
            // （Live2DCanvas 的 cleanup 误杀全局线程是历史 bug 的根因）。
            //
            // 触发停止的时机：任一角色窗口 CloseRequested 后检查是否还有其他
            // 在线角色窗口；若全部关闭才停线程。
            move |window, event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    let app = window.app_handle();
                    if let Some(state) =
                        app.try_state::<std::sync::Arc<crate::state::AppState>>()
                    {
                        commands::window::stop_cursor_tracking_if_no_windows(
                            app,
                            state.inner(),
                        );
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                tracing::info!("[exit] 收到退出请求，开始清理资源");
                // 置退出标志，光标追踪线程在下一轮循环（≤60ms）内退出
                commands::window::APP_EXITING
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                // 触发全局 CancellationToken，通知所有后台 tokio 任务优雅停止
                crate::utils::cancel_token::cancel_token().cancel();
                // 带超时的清理逻辑 + yield_now 让在途任务完成
                let _ = tauri::async_runtime::block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        async {
                            // 应用退出前强制落盘所有角色的记忆脏数据（5s 节流期间未写入的数据）
                            if let Some(state) = app_handle
                                .try_state::<std::sync::Arc<crate::state::AppState>>()
                            {
                                {
                                    let chars = state.characters.read();
                                    for (id, instance) in chars.iter() {
                                        if let Err(e) = instance.brain.memory.flush() {
                                            tracing::warn!(
                                                "[lib] 退出时 flush 角色 {} 记忆脏数据失败: {e}",
                                                id
                                            );
                                        }
                                    }
                                }
                                // 停止光标追踪线程，避免访问已销毁的窗口句柄
                                commands::window::stop_cursor_tracking_internal(
                                    app_handle,
                                    state.inner(),
                                );
                                // 停止 side_chat 边缘检测线程
                                commands::window::stop_side_chat_edge_watcher_internal();
                                // 停止 side_chat 全局鼠标 Hook 线程
                                commands::window::stop_side_chat_mouse_hook_internal();
                            }
                            // 停止 Ollama 子进程（仅停止由本应用启动的实例）
                            {
                                let svc = crate::memory::ollama_service::ollama_service().await;
                                let _ = svc.stop().await;
                            }
                            // 停止 GPT-SoVITS 子进程，避免主程序退出后变成孤儿进程
                            // 占用端口导致下次启动失败
                            {
                                let svc = crate::speech::gpt_sovits_service::service().await;
                                let _ = svc.stop().await;
                            }
                            // 停止 Fish Speech 子进程
                            {
                                let svc = crate::speech::fish_speech_service().await;
                                let _ = svc.stop().await;
                            }
                            // 停止 Whisper 子进程
                            {
                                let svc = crate::speech::whisper_service().await;
                                let _ = svc.stop().await;
                            }
                            // yield 几次让在途 fire-and-forget 任务有机会完成关键写入
                            for _ in 0..3 {
                                tokio::task::yield_now().await;
                            }
                        },
                    )
                    .await;
                });
                tracing::info!("[exit] 资源清理完成");
            }
        });
}

/// 启动时清理过期日志文件（保留最近 `keep_days` 天）
fn cleanup_old_logs(log_dir: &std::path::Path, keep_days: u32) {
    use chrono::Local;
    let cutoff = Local::now()
        .date_naive()
        .checked_sub_signed(chrono::Duration::days(keep_days as i64));
    let Some(cutoff) = cutoff else { return };

    let mut cleaned = 0u32;
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // 匹配 vivian_YYYY-MM-DD.log
        if !name_str.starts_with("vivian_") || !name_str.ends_with(".log") {
            continue;
        }
        let date_str = &name_str["vivian_".len()..name_str.len() - ".log".len()];
        let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };
        if date < cutoff {
            let _ = std::fs::remove_file(entry.path());
            cleaned += 1;
        }
    }
    if cleaned > 0 {
        eprintln!("[日志清理] 已删除 {} 个过期日志文件（保留最近 {} 天）", cleaned, keep_days);
    }
}

fn init_logging() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let log_dir = crate::utils::path::get_user_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    // 启动时清理过期日志（保留最近 7 天）
    cleanup_old_logs(&log_dir, 7);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,vivian=debug"));

    // 使用简单的按日文件名（vivian_YYYY-MM-DD.log）
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log_path = log_dir.join(format!("vivian_{}.log", today));

    // panic hook：将 panic 信息和调用栈同步写入日志文件 + stderr
    // 不依赖 tracing 的 non-blocking writer（panic 后进程可能立即退出，缓冲区未刷新）
    let panic_log_path = log_path.clone();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let msg = format!(
            "[PANIC] {}\nBacktrace:\n{}",
            info,
            backtrace
        );
        eprintln!("{}", msg);
        // 同步写入日志文件，确保 panic 信息不丢失
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_log_path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", msg);
        }
        // 触发 CancellationToken 通知后台任务停止。
        // 子进程由 Job Object 的 KILL_ON_JOB_CLOSE 兜底，无需在此手动清理。
        // APP_EXITING 置位让光标追踪等原生线程感知退出。
        commands::window::APP_EXITING
            .store(true, std::sync::atomic::Ordering::SeqCst);
        crate::utils::cancel_token::cancel_token().cancel();
    }));

    // 先用 File::create 确保文件存在（create(true).append(true) 在某些 Windows 环境会失败）
    let file_writer = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&log_path)
        .or_else(|_| {
            // 回退：先创建再以追加模式打开
            std::fs::File::create(&log_path)?;
            std::fs::OpenOptions::new().append(true).open(&log_path)
        });

    match file_writer {
        Ok(file) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(file);
            // guard 必须在应用整个生命周期内存活，此处泄漏以避免被 drop
            std::mem::forget(guard);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_target(false),
                )
                .with(
                    fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false)
                        .with_target(true),
                )
                .init();
        }
        Err(e) => {
            eprintln!("警告：文件日志初始化失败（{e}），仅使用 stdout");
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_target(false),
                )
                .init();
        }
    }
}
