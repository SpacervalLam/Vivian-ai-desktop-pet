//! 值守 tick：只负责「到期就唤醒一次」的应用边界。
//!
//! 与存储逻辑分开，是为了让 `work_loops` 保持零依赖可测：这里可以依赖 Tauri、
//! 会话服务与模型路由，那边只比较时间戳。

use tauri::Manager;

use super::work_loops::{global_work_loops, wake_text, TICK_SECS};

/// 启动值守 tick（应用运行期间）。到期才唤醒，其余时间只比时间戳。
pub fn spawn_tick(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            dispatch_due(&app).await;
        }
    });
}

async fn dispatch_due(app: &tauri::AppHandle) {
    let now = super::work_loops::now_secs();
    let due = global_work_loops().due(now);
    if due.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<std::sync::Arc<crate::state::AppState>>() else { return };
    let router = state.model_router.read().clone();
    let tool_system = state.tool_system.clone();
    let max_rounds = state.config.read().get_all().tools.max_coding_rounds as usize;
    drop(state);
    let Some(router) = router else {
        // 模型尚未配置：顺延，不刷日志也不丢条目。
        for loop_ in &due {
            global_work_loops().defer(&loop_.id, now);
        }
        return;
    };
    for loop_ in due {
        let agent = crate::commands::coding_agent::CODING_AGENT.clone();
        if agent.get_session(&loop_.session_id).is_none() {
            // 会话已删除：值守没有落点，停用而不是每 30 秒报错。
            let _ = global_work_loops().set_enabled(&loop_.id, false);
            continue;
        }
        let text = wake_text(&loop_);
        match agent.send_message(
            app.clone(),
            loop_.session_id.clone(),
            router.clone(),
            tool_system.clone(),
            text,
            Vec::new(),
            Vec::new(),
            max_rounds,
            false,
            false,
        ) {
            Ok(()) => global_work_loops().record_run(&loop_.id, now, "dispatched"),
            Err(error) => {
                tracing::debug!("[work_loops] {} 本次顺延：{error}", loop_.id);
                global_work_loops().defer(&loop_.id, now);
            }
        }
    }
}
