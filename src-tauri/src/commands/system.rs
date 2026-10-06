//! 系统命令 - 系统信息、进程管理与应用控制

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tauri::State;

use crate::state::AppState;

/// Keep CPU sampling history and share a one-second snapshot across windows.
/// System::new() deliberately avoids enumerating processes for a CPU/RAM query.
struct SystemInfoSampler {
    system: System,
    cached: Option<(Instant, Value)>,
}

impl SystemInfoSampler {
    fn new() -> Self { Self { system: System::new(), cached: None } }

    fn sample(&mut self) -> Value {
        if let Some((at, value)) = &self.cached {
            if at.elapsed() < Duration::from_secs(1) { return value.clone(); }
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let total = self.system.total_memory();
        let used = self.system.used_memory();
        let value = json!({
            "cpu_usage": self.system.global_cpu_usage(),
            "cpu_count": self.system.cpus().len(),
            "total_memory": total,
            "used_memory": used,
            "available_memory": total.saturating_sub(used),
            "memory_usage_pct": if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 },
            "uptime": System::uptime(),
            "host_name": System::host_name().unwrap_or_default(),
            "os_name": System::name().unwrap_or_default(),
            "os_version": System::os_version().unwrap_or_default(),
        });
        self.cached = Some((Instant::now(), value.clone()));
        value
    }
}

static SYSTEM_INFO: Lazy<Mutex<SystemInfoSampler>> = Lazy::new(|| Mutex::new(SystemInfoSampler::new()));

#[cfg(test)]
mod sampler_tests {
    use super::*;

    #[test]
    fn snapshots_never_enumerate_processes_and_refresh_after_expiry() {
        let mut sampler = SystemInfoSampler::new();
        let first = sampler.sample();
        assert_eq!(sampler.sample(), first);
        assert!(sampler.system.processes().is_empty());
        sampler.cached.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(2);
        let next = sampler.sample();
        assert!(next["cpu_usage"].as_f64().unwrap().is_finite());
        assert!(sampler.cached.as_ref().unwrap().0.elapsed() < Duration::from_secs(1));
        assert!(sampler.system.processes().is_empty());
    }

    #[test]
    #[ignore = "controlled local performance measurement"]
    fn system_info_performance_baseline() {
        for _ in 0..3 {
            let started = Instant::now();
            let mut old = System::new_with_specifics(RefreshKind::everything());
            old.refresh_cpu_usage();
            old.refresh_memory();
            let old_us = started.elapsed().as_micros();
            let started = Instant::now();
            let mut sampler = SystemInfoSampler::new();
            let value = sampler.sample();
            let cold_us = started.elapsed().as_micros();
            let started = Instant::now();
            for _ in 0..100 { std::hint::black_box(sampler.sample()); }
            let cached_us = started.elapsed().as_micros() / 100;
            assert!(value["total_memory"].as_u64().unwrap() > 0);
            println!("system_info old_us={old_us} new_cold_us={cold_us} new_cached_us={cached_us} old_processes={} new_processes={}",
                old.processes().len(), sampler.system.processes().len());
        }
    }
}

/// 恢复出厂清扫标记文件（写入用户数据目录根，下次启动时消费并删除）
const FACTORY_RESET_MARKER: &str = ".factory_reset_pending";

/// 只清理明确的记忆路径；未知目录、配置和内容资产默认保留。
const MEMORY_RESET_PATHS: &[&str] = &[
    "memory", "history", "psychology", "mind", "proactive", "user_facts.json",
    "self_state.json", "meme_acquisition_state.json", "presence/state.json",
    "diary/diaries.json", "persona/evolution.json", "persona/dynamic_profile.json",
    "persona/persona_events.jsonl", "companion-v2/memory", "companion-v2/history",
    "companion-v2/mind", "companion-v2/proactive", "companion-v2/psychology.json",
    "companion-v2/user_facts.json", "companion-v2/user_model.json", "user_model.json", "companion-v2/research",
];
const SHARED_MEMORY_RESET_PATHS: &[&str] = &[
    "memory", "history", "psychology", "mind", "proactive", "user_facts.json",
    "user_model.json", "companion-v2/user_model.json",
    "common/memory", "common/diary/diaries.json", "diary/diaries.json",
    "companion-v2/memory", "companion-v2/common/memory", "companion-v2/psychology", "shared/memory",
    "persona/evolution.json", "persona/dynamic_profile.json", "persona/persona_events.jsonl",
];

/// 记忆目录内也保留配置，避免删除混存的设置；不跟随符号链接。
fn remove_memory_path(path: &std::path::Path) -> std::io::Result<()> {
    if path.ancestors().any(|ancestor| std::fs::symlink_metadata(ancestor)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())) {
        return Ok(());
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if metadata.file_type().is_symlink() { return Ok(()); }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_memory_path(&entry?.path())?;
        }
        if std::fs::read_dir(path)?.next().is_none() { std::fs::remove_dir(path)?; }
        return Ok(());
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.contains("config") || name == "trigger_preferences.json" || name.ends_with(".yaml") || name.ends_with(".yml") {
        return Ok(());
    }
    std::fs::remove_file(path)
}

/// 写入恢复出厂清扫标记
///
/// 标记在下次启动、任何数据模块打开文件前被消费：
/// 此时 SQLite 连接等尚未建立，可清理记忆文件而不改动配置。
fn mark_factory_reset_sweep() -> Result<(), String> {
    let marker = crate::utils::path::get_user_data_dir().join(FACTORY_RESET_MARKER);
    std::fs::write(&marker, b"1").map_err(|e| format!("写入恢复出厂清扫标记失败: {e}"))
}

/// 启动时消费清扫标记：只删除明确的记忆内容，保留全部配置和内容资产
///
/// 必须在 AppState::new() 之前调用（任何 MemoryManager / 向量库 /
/// 事件账本初始化之前），否则被占用的文件（如 vectors.db）在 Windows
/// 上因共享冲突无法删除。
pub fn factory_reset_sweep_if_pending() -> Result<(), String> {
    let root = crate::utils::path::get_user_data_dir();
    sweep_factory_reset_dir(&root)
}

fn sweep_factory_reset_dir(root: &std::path::Path) -> Result<(), String> {
    let marker = root.join(FACTORY_RESET_MARKER);
    if !marker.exists() {
        return Ok(());
    }
    tracing::info!("[factory_reset] 检测到清扫标记，开始重置用户本地数据目录: {}", root.display());

    let mut targets: Vec<std::path::PathBuf> = SHARED_MEMORY_RESET_PATHS.iter()
        .map(|name| root.join(name)).collect();
    let characters = root.join("characters");
    if characters.is_dir() {
        for entry in std::fs::read_dir(&characters).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                targets.extend(MEMORY_RESET_PATHS.iter().map(|name| entry.path().join(name)));
            }
        }
    }
    let mut removed = 0usize;
    let mut failed = 0usize;
    for path in targets {
        let name = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        // Windows 重启时旧进程可能尚未释放 SQLite 等文件句柄。
        // 短暂重试；仍失败则保留标记，不允许启动后读取旧数据。
        let mut result = Ok(());
        for attempt in 0..20 {
            result = remove_memory_path(&path);
            if result.is_ok() || result.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                result = Ok(());
                break;
            }
            if attempt < 19 {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        match result {
            Ok(()) => {
                removed += 1;
                tracing::info!("[factory_reset] 已删除: {}", name);
            }
            Err(e) => {
                failed += 1;
                tracing::warn!("[factory_reset] 删除 {} 失败: {e}", name);
            }
        }
    }

    tracing::info!(
        "[factory_reset] 用户数据目录清扫完成：删除 {} 项，失败 {} 项",
        removed,
        failed
    );
    if failed > 0 {
        return Err(format!("恢复出厂清扫未完成（{failed} 项失败）。清扫标记已保留；请关闭占用数据目录的程序后重新启动。"));
    }
    std::fs::remove_file(&marker).map_err(|e| format!("删除恢复出厂清扫标记失败: {e}"))
}

#[cfg(test)]
mod reset_tests {
    use super::*;

    #[test]
    fn mixed_memory_directory_preserves_configuration_and_clears_shared_memory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["characters/nana/companion-v2/proactive/state.json",
            "characters/nana/companion-v2/proactive/config.json",
            "characters/nana/companion-v2/proactive/trigger_preferences.json",
            "companion-v2/memory/events.db", "unknown/settings.json"] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "original").unwrap();
        }
        std::fs::write(root.join(FACTORY_RESET_MARKER), "1").unwrap();
        sweep_factory_reset_dir(root).unwrap();
        assert!(!root.join("characters/nana/companion-v2/proactive/state.json").exists());
        assert!(!root.join("companion-v2/memory/events.db").exists());
        for name in ["characters/nana/companion-v2/proactive/config.json",
            "characters/nana/companion-v2/proactive/trigger_preferences.json", "unknown/settings.json"] {
            assert_eq!(std::fs::read_to_string(root.join(name)).unwrap(), "original");
        }
    }

    #[test]
    fn sweep_only_runs_with_marker_and_preserves_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("characters/nana/memory")).unwrap();
        std::fs::write(root.join("characters/nana/memory/dialogue.json"), "old dialogue").unwrap();
        std::fs::write(root.join("config.yaml"), "settings").unwrap();
        for name in ["characters/nana/sound/config.json", "characters/nana/sound/voice.yaml",
            "characters/nana/persona/persona.json", "characters/nana/diary/config.json",
            "plugins/plugin.json", "mcp/config.json", "todo/tasks.json", "notebook/note.json"] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "preserve exactly").unwrap();
        }
        std::fs::write(root.join("token_usage.json"), "42").unwrap();
        for name in ["characters/nana/companion-v2/user_model.json", "characters/vivian/user_model.json", "companion-v2/user_model.json"] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "old user knowledge").unwrap();
        }

        sweep_factory_reset_dir(root).unwrap();
        assert!(root.join("characters").exists());
        std::fs::write(root.join(FACTORY_RESET_MARKER), "1").unwrap();
        sweep_factory_reset_dir(root).unwrap();
        assert!(!root.join("characters/nana/memory").exists());
        assert!(!root.join(FACTORY_RESET_MARKER).exists());
        for name in ["characters/nana/companion-v2/user_model.json", "characters/vivian/user_model.json", "companion-v2/user_model.json"] {
            assert!(!root.join(name).exists());
        }
        assert_eq!(std::fs::read_to_string(root.join("config.yaml")).unwrap(), "settings");
        for name in ["characters/nana/sound/config.json", "characters/nana/sound/voice.yaml",
            "characters/nana/persona/persona.json", "characters/nana/diary/config.json",
            "plugins/plugin.json", "mcp/config.json", "todo/tasks.json", "notebook/note.json"] {
            assert_eq!(std::fs::read_to_string(root.join(name)).unwrap(), "preserve exactly");
        }
        assert_eq!(std::fs::read_to_string(root.join("token_usage.json")).unwrap(), "42");

    }

    #[cfg(windows)]
    #[test]
    fn sweep_retries_until_old_process_releases_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("characters/nana/memory")).unwrap();
        let file = std::fs::OpenOptions::new().write(true).create(true).share_mode(0)
            .open(root.join("characters/nana/memory/locked.db")).unwrap();
        std::fs::write(root.join(FACTORY_RESET_MARKER), "1").unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            drop(file);
        });
        sweep_factory_reset_dir(root).unwrap();
        release.join().unwrap();
        assert!(!root.join("characters/nana/memory").exists());
        assert!(!root.join(FACTORY_RESET_MARKER).exists());
    }

    #[cfg(windows)]
    #[test]
    fn locked_data_keeps_marker_until_next_successful_sweep() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("characters/nana/memory")).unwrap();
        let file = std::fs::OpenOptions::new().write(true).create(true).share_mode(0)
            .open(root.join("characters/nana/memory/locked.db")).unwrap();
        std::fs::write(root.join(FACTORY_RESET_MARKER), "1").unwrap();
        assert!(sweep_factory_reset_dir(root).is_err());
        assert!(root.join(FACTORY_RESET_MARKER).exists());
        drop(file);
        sweep_factory_reset_dir(root).unwrap();
        assert!(!root.join("characters/nana/memory").exists());
        assert!(!root.join(FACTORY_RESET_MARKER).exists());
    }
}

/// 查询 Brain 是否已初始化完成（前端据此决定是否调用依赖 Brain 的命令）
#[tauri::command]
pub fn is_initialized(state: State<'_, Arc<AppState>>) -> bool {
    state.is_initialized()
}

/// 查询启动进度快照（供启动 Toast 窗口挂载时补齐监听注册前错过的进度）
#[tauri::command]
pub fn get_startup_progress() -> Value {
    let (in_progress, state) = crate::startup::progress_snapshot();
    json!({
        "in_progress": in_progress,
        "current": state.as_ref().map(|s| s.current),
        "total": state.as_ref().map(|s| s.total),
        "stage": state.as_ref().map(|s| s.stage.clone()),
    })
}

/// 退出整个应用（通过 Tauri 正常退出流程，确保窗口销毁与资源清理）
#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    tracing::info!("[exit_app] 收到退出请求，触发 Tauri 正常退出流程");
    // 先置退出标志，光标追踪线程在下一轮循环（≤60ms）内退出
    crate::commands::window::APP_EXITING.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// 恢复出厂设置：原子化执行「锁死行为 → 停止后台任务 → 清空数据 → 重启应用」
///
/// 整个流程在后端单命令内完成，避免前端分步调用产生的时间窗口竞态：
///
/// 1. 设置 `factory_reset_in_progress = true`，所有前端定时器驱动的 tick 命令
///    （proactive_tick / psychology_micro_tick / mood_expression_tick / auto_expression_tick）
///    立即返回跳过，不再产生新行为。
/// 2. 停止所有可停止的后台子系统：proactive / scheduler / speech / activity_journal /
///    PetController（动作 + 状态机）。
/// 3. 等待 grace period（500ms），让已 spawn 的 LLM/记忆任务跑完或检测到标志。
/// 4. 执行数据清空：每个角色 clear_all_memories + 全局 clear_common_memories。
/// 5. 写入清扫标记并重启，在数据模块打开前清理明确的记忆路径。
///    保留角色配置、声音配置、插件/MCP、待办、笔记与使用统计。
#[tauri::command]
pub async fn factory_reset(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    tracing::info!("[factory_reset] 开始恢复出厂设置流程");
    // 在任何清空操作之前保证标记落盘，写入失败时不破坏当前数据。
    mark_factory_reset_sweep()?;

    // ===== 1. 锁死所有行为 =====
    state.set_factory_reset_in_progress(true);
    tracing::info!("[factory_reset] 已设置 factory_reset_in_progress 标志，所有 tick 命令将被拒绝");

    // ===== 2. 停止所有后台子系统 =====
    // 2.1 停止所有角色的主动对话 + 活动日志 + PetController
    {
        let chars = state.characters.read();
        for (id, instance) in chars.iter() {
            instance.brain.stop_proactive();
            instance.brain.proactive.activity_journal().stop();
            let _ = instance.pet_controller.stop_all_motions();
            instance.pet_controller.stop();
            tracing::info!("[factory_reset] 已停止角色 {} 的 proactive / activity_journal / pet_controller", id);
        }
    }

    // 2.2 只停止调度器，保留用户的待办和定时任务。
    state.scheduler.shutdown();
    tracing::info!("[factory_reset] 已停止 Scheduler");

    // 2.3 停止全局 SpeechPlanner（TTS 队列）
    {
        let planner = crate::speech::planner::planner().await;
        if let Err(e) = planner.stop_all().await {
            tracing::warn!("[factory_reset] 停止 SpeechPlanner 失败: {e}");
        } else {
            tracing::info!("[factory_reset] 已停止 SpeechPlanner");
        }
    }

    // ===== 3. grace period：让已 spawn 的短期任务完成 =====
    // 这些任务（inner_monologue / memory_consolidation / 夜间巩固 / 日常巩固）
    // 句柄未保存无法 abort，但 tick 命令已被拒绝，新任务不会再产生。
    // 等待 500ms 让进行中的 LLM 调用或写入完成，避免与清空操作竞态。
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // ===== 4. 执行数据清空 =====
    // 复用 clear_all_memories 的内部逻辑（不通过 Tauri 命令层），保持单一权威实现。
    tracing::info!("[factory_reset] 开始清空所有角色数据");
    let char_ids: Vec<String> = state.characters.read().keys().cloned().collect();
    for char_id in &char_ids {
        match crate::commands::memory::clear_all_memories(
            app.clone(),
            state.clone(),
            Some(char_id.clone()),
        )
        .await
        {
            Ok(()) => tracing::info!("[factory_reset] 角色 {} 数据已清空", char_id),
            Err(e) => tracing::warn!("[factory_reset] 角色 {} 数据清空失败: {e}", char_id),
        }
    }
    // 清空共同记忆（无角色归属的全局共享记忆）
    if let Err(e) = crate::commands::memory::clear_common_memories(app.clone()).await {
        tracing::warn!("[factory_reset] 清空共同记忆失败: {e}");
    } else {
        tracing::info!("[factory_reset] 共同记忆已清空");
    }

    // 清空应用解析缓存（避免历史错误映射残留影响后续 open_application 调用）
    // 应用解析配置与缓存不属于记忆，保留。

    // ===== 5. 写入清扫标记并重启应用 =====
    tracing::info!("[factory_reset] 数据清空完成，准备重启应用");
    // 重启后在 AppState 构造前只清理明确的记忆路径，保留配置与内容资产，
    // 覆盖内存清空未触达的文件（screenshots / images / discovery / 历史遗留目录等）。
    // request_restart 会触发 RunEvent::ExitRequested，由 lib.rs 执行常规清理
    // （记忆落盘 / 停光标追踪 / 卸载子类化），随后 Tauri 自动重启进程。
    // 重启后 AppState 重新构造，factory_reset_in_progress 自然恢复为 false，行为恢复。
    app.request_restart();
    Ok(())
}

/// 重新初始化 Brain / ModelRouter / MemoryManager
///
/// 用户在设置面板修改 LLM 配置（API Key / Endpoint / Model / 路由矩阵）后调用，
/// 让新配置立即生效，无需重启应用。失败时返回错误，前端可提示用户。
#[tauri::command]
pub async fn reinitialize(
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::Emitter;
    let _reinitialize_guard = state.reinitialize_lock.lock().await;
    tracing::info!("[reinitialize] 开始重新初始化 Brain / ModelRouter");
    crate::startup::begin_startup();
    // 复用启动进度 Toast 窗口展示重初始化进度（进度经快照 + 周期重发补齐）
    crate::startup::show_startup_toast();

    // 与启动流程一致：重新初始化前先确保主 LLM / 嵌入服务配置完成，
    // 本地 Ollama 场景下先启动服务并等待模型可用。
    if !crate::startup::preflight(&app, state.inner()).await {
        crate::startup::emit_progress(100, 100, "请完成配置");
        crate::startup::finish_startup();
        return Err(
            "启动预检未通过：请先完成主 LLM / 嵌入服务配置，并确保 Ollama 已启动且模型已就绪"
                .to_string(),
        );
    }

    if let Err(e) = state.initialize().await {
        tracing::error!("[reinitialize] 重新初始化失败: {e}");
        crate::startup::emit_progress(100, 100, "初始化失败，请检查配置");
        crate::startup::finish_startup();
        crate::startup::open_config_with_guide(&app);
        return Err(e.to_string());
    }
    crate::startup::emit_progress(100, 100, "启动完成 ✓");
    crate::startup::finish_startup();
    // 首次启动若因配置未完成跳过了角色窗口创建，配置保存并重试初始化后补建窗口
    crate::create_character_windows(&app, state.inner());
    // 重新注入 AppHandle（新 router 实例不携带旧 handle）
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_app_handle(app.clone());
    }
    // 热更新工具确认超时（ToolSystem 在 AppState::new() 一次性构造，reinitialize 不重建）
    let confirmation_timeout = state.config.read().get_all().tools.confirmation_timeout_secs;
    state.tool_system.update_confirmation_timeout(confirmation_timeout);
    let _ = app.emit("app:ready", ());
    tracing::info!("[reinitialize] 重新初始化完成");
    Ok(())
}

/// 获取系统信息（CPU、内存）
#[tauri::command]
pub fn get_system_info() -> Result<Value, String> {
    Ok(SYSTEM_INFO.lock().sample())
}

/// 获取正在运行的进程列表
#[tauri::command]
pub fn get_running_processes() -> Result<Vec<Value>, String> {
    let mut sys = System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::everything()));
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut processes: Vec<Value> = sys
        .processes()
        .iter()
        .map(|(pid, p)| {
            json!({
                "pid": pid.as_u32(),
                "name": p.name().to_string_lossy(),
                "cpu_usage": p.cpu_usage(),
                "memory": p.memory(),
                "command": p.cmd().iter().map(|c| c.to_string_lossy().to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();
    processes.sort_by(|a, b| {
        let a_cpu = a.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b_cpu = b.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
        b_cpu.partial_cmp(&a_cpu).unwrap_or(std::cmp::Ordering::Equal)
    });
    processes.truncate(50);
    Ok(processes)
}

/// 打开应用程序
///
/// 安全策略：
/// - 拒绝绝对路径、UNC 路径、相对路径标记（`\..`、`/..`）
/// - 拒绝含 shell 元字符（`&`、`|`、`;`、`>`、`<`、`` ` ``、`$`、`%`、`(`、`)`）的输入
/// - 仅允许纯文件名（可带 `.exe` 后缀），由系统 PATH 解析
#[tauri::command]
pub fn open_application(name: String) -> Result<(), String> {
    tracing::info!("尝试打开应用: {}", name);

    // 安全校验：拒绝路径分隔符与 shell 元字符
    const FORBIDDEN_CHARS: &[char] = &['/', '\\', '&', '|', ';', '>', '<', '`', '$', '%', '(', ')', '"', '\''];
    if name.is_empty() || name.contains("..") || name.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
        return Err(format!("拒绝打开应用：名称包含非法字符或路径分隔符: {}", name));
    }

    #[cfg(target_os = "windows")]
    {
        let cmd = if name.ends_with(".exe") {
            name.clone()
        } else {
            format!("{}.exe", name)
        };
        match crate::utils::process::silent_command(&cmd).spawn() {
            Ok(_) => return Ok(()),
            Err(e) => {
                return Err(format!("打开应用 {} 失败: {}", cmd, e));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        match crate::utils::process::silent_command("open")
            .arg(format!("-a {}", name))
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) => return Err(format!("打开应用 {} 失败: {}", name, e)),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match crate::utils::process::silent_command(&name).spawn() {
            Ok(_) => return Ok(()),
            Err(e) => return Err(format!("打开应用 {} 失败: {}", name, e)),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(format!("不支持当前操作系统打开应用: {}", name))
    }
}

/// 关闭应用程序
#[tauri::command]
pub fn close_application(name: String) -> Result<(), String> {
    tracing::info!("尝试关闭应用: {}", name);
    let target = if name.ends_with(".exe") {
        name.clone()
    } else {
        format!("{}.exe", name)
    };
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut killed = 0;
    for (_, p) in sys.processes() {
        let p_name = p.name().to_string_lossy().to_lowercase();
        if p_name == target.to_lowercase() || p_name == name.to_lowercase() {
            if p.kill() {
                killed += 1;
            }
        }
    }
    if killed > 0 {
        tracing::info!("已关闭 {} 个 {} 进程", killed, name);
        Ok(())
    } else {
        Err(format!("未找到运行中的应用: {}", name))
    }
}
