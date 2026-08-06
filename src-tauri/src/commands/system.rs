//! 系统命令 - 系统信息、进程管理与应用控制

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tauri::State;

use crate::state::AppState;

static SYSTEM_REFRESH: AtomicBool = AtomicBool::new(false);

/// 查询 Brain 是否已初始化完成（前端据此决定是否调用依赖 Brain 的命令）
#[tauri::command]
pub fn is_initialized(state: State<'_, Arc<AppState>>) -> bool {
    state.is_initialized()
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
/// 5. 调用 `tauri_plugin_process::process::restart` 重启应用，重启后所有内存状态归零，
///    行为自然恢复。
#[tauri::command]
pub async fn factory_reset(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    tracing::info!("[factory_reset] 开始恢复出厂设置流程");

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

    // 2.2 清空并停止全局 Scheduler（定时任务调度器）
    // 先清空待办（会取消关联的 reminder），再清空定时任务，最后停止调度循环
    crate::tools::builtin::todo_tools::clear_all_todos();
    tracing::info!("[factory_reset] 已清空所有待办");
    state.scheduler.clear_all_tasks();
    tracing::info!("[factory_reset] 已清空所有定时任务");
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
    crate::tools::builtin::system_ops::clear_app_registry();

    // ===== 5. 重启应用 =====
    tracing::info!("[factory_reset] 数据清空完成，准备重启应用");
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
    tracing::info!("[reinitialize] 开始重新初始化 Brain / ModelRouter");
    state.initialize().await.map_err(|e| {
        tracing::error!("[reinitialize] 重新初始化失败: {e}");
        e.to_string()
    })?;
    // 重新注入 AppHandle（新 router 实例不携带旧 handle）
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_app_handle(app.clone());
    }
    let _ = app.emit("app:ready", ());
    tracing::info!("[reinitialize] 重新初始化完成");
    Ok(())
}

/// 获取系统信息（CPU、内存）
#[tauri::command]
pub fn get_system_info() -> Result<Value, String> {
    SYSTEM_REFRESH.store(true, Ordering::SeqCst);
    let mut sys = System::new_with_specifics(RefreshKind::everything());
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    SYSTEM_REFRESH.store(false, Ordering::SeqCst);

    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let available_memory = total_memory.saturating_sub(used_memory);
    let memory_usage_pct = if total_memory > 0 {
        (used_memory as f64 / total_memory as f64) * 100.0
    } else {
        0.0
    };

    Ok(json!({
        "cpu_usage": sys.global_cpu_usage(),
        "cpu_count": sys.cpus().len(),
        "total_memory": total_memory,
        "used_memory": used_memory,
        "available_memory": available_memory,
        "memory_usage_pct": memory_usage_pct,
        "uptime": System::uptime(),
        "host_name": System::host_name().unwrap_or_default(),
        "os_name": System::name().unwrap_or_default(),
        "os_version": System::os_version().unwrap_or_default(),
    }))
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
