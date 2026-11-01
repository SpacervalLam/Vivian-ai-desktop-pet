//! One native cognition actor per character. Webviews present speech; they do not
//! own the character's lifetime, private state, or self-scheduled next thought.
use crate::{presence::PresenceState, proactive::TickContext, state::AppState};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    sync::{Arc, LazyLock, Weak},
    time::{Duration, Instant},
};

#[derive(Default, Serialize)]
struct ActorStatus {
    heartbeat_at: i64,
    private_tick_at: i64,
    pulses: u64,
    restarts: u64,
    phase: String,
}
static ACTORS: LazyLock<Mutex<super::resident_tasks::ResidentTasks<Arc<Mutex<ActorStatus>>>>> =
    LazyLock::new(Default::default);

/// Called by Scheduler's native heartbeat. A still-running actor is never replaced.
pub(crate) fn ensure_started(app: &tauri::AppHandle, state: &Arc<AppState>) {
    if crate::utils::cancel_token::cancel_token().is_cancelled() {
        return;
    }
    let ids: Vec<String> = state.characters.read().keys().cloned().collect();
    let mut actors = ACTORS.lock();
    actors.retain(&ids);
    for id in ids {
        actors.ensure(
            &id,
            |previous| {
                previous
                    .map(|status| {
                        status.lock().restarts += 1;
                        tracing::warn!(character=%id, "常驻认知任务已退出，重新启动");
                        status
                    })
                    .unwrap_or_else(|| Arc::new(Mutex::new(ActorStatus::default())))
            },
            |status| tokio::spawn(run(app.clone(), Arc::downgrade(state), id.clone(), status)),
        );
    }
}

pub(crate) fn status(id: &str) -> serde_json::Value {
    let actors = ACTORS.lock();
    actors
        .get(id)
        .map(|actor| {
            serde_json::json!({
                "running": !actor.task.is_finished(), "state": *actor.state.lock(),
                "owner": "native", "private_tick_seconds": 10,
            })
        })
        .unwrap_or_else(|| serde_json::json!({"running":false,"owner":"native"}))
}

struct PeerJob(Option<tokio::task::JoinHandle<()>>);
impl Drop for PeerJob {
    fn drop(&mut self) {
        if let Some(job) = &self.0 {
            job.abort();
        }
    }
}

async fn run(
    app: tauri::AppHandle,
    state: Weak<AppState>,
    id: String,
    status: Arc<Mutex<ActorStatus>>,
) {
    let cancel = crate::utils::cancel_token::cancel_token();
    let mut identity = 0usize;
    let mut last_pulse = Instant::now() - Duration::from_secs(10);
    let mut last_maintenance = Instant::now() - Duration::from_secs(60);
    let mut wake_requested = true;
    let mut peer_job = PeerJob(None);
    let mut inbox_job = PeerJob(None);
    loop {
        let Some(state) = state.upgrade() else {
            return;
        };
        if cancel.is_cancelled() {
            return;
        }
        let instance = state.get_character(Some(&id)).ok();
        let wake = instance
            .as_ref()
            .map(|instance| instance.brain.mind.cognition_wake.clone());
        let now = chrono::Utc::now().timestamp();
        status.lock().heartbeat_at = now;
        let enabled = state.initialized.load(std::sync::atomic::Ordering::Acquire)
            && !state.is_factory_reset_in_progress()
            && !state.is_rebuild_in_progress();
        if let Some(instance) = instance.filter(|instance| enabled && *instance.online.read()) {
            let brain = &instance.brain;
            let current = Arc::as_ptr(&brain.cognitive_tick) as usize;
            if current != identity {
                if let Some(job) = inbox_job.0.take() { job.abort(); }
                if let Some(job) = peer_job.0.take() { job.abort(); }
                identity = current;
                if let Err(error) = brain.mind.inbox.interrupt_peer_processing() { tracing::warn!("收件工作恢复失败: {error}"); }
                // Reinitialize creates new instances: never keep reasoning with an old Brain.
                if brain.config.proactive.enabled {
                    brain.start_proactive();
                }
                // Preserve a restored continuation; cold-start evaluation is not new evidence.
                if !brain
                    .mind
                    .conversation_opportunities
                    .read()
                    .threads
                    .continuation
                    .has_pending()
                {
                    brain
                        .mind
                        .thought_refresh_requested
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                wake_requested = true;
            }
            if wake_requested || last_pulse.elapsed() >= Duration::from_secs(10) {
                wake_requested = false;
                last_pulse = Instant::now();
                match brain.mind.personal.review_due(now) {
                    Ok(true)=>{brain.mind.thought_refresh_requested.store(true,std::sync::atomic::Ordering::Release);},
                    Err(error)=>tracing::warn!(character=%id,"事务跟进状态保存失败: {error}"),
                    _=>{}
                }
                let active_user = state.session_coordinator.any_user_turn_active();
                let quiet = crate::companion_runtime::quiet_reason(&state).is_some();
                let presence = brain.presence.current();
                let idle = crate::utils::get_system_idle_seconds().unwrap_or(3600.0);
                let context = TickContext {
                    now: now as f64,
                    idle_seconds: idle,
                    user_present: idle <= brain.config.proactive.away_threshold_seconds as f64,
                    character_presence: presence,
                    user_contact_blocked: state
                        .world_provider
                        .environment
                        .contact_suppressed(now as f64),
                    is_user_chatting: active_user,
                    is_speaking_leader: false,
                    lay_low: quiet
                        || active_user
                        || presence != PresenceState::Online
                        || !brain.proactive.is_running()
                        || !brain.config.proactive.enabled,
                    active_window: state
                        .world_provider
                        .environment
                        .external_foreground()
                        .map(|window| window.title)
                        .unwrap_or_default(),
                    ..Default::default()
                };
                brain.mind.log_contended_locks(context.now);
                super::interaction_runtime::ingest(brain);
                brain.cognitive_tick.run_background(brain, &context);
                if last_maintenance.elapsed() >= Duration::from_secs(60) {
                    last_maintenance = Instant::now();
                    brain.check_memory_maintenance();
                }
                {
                    let mut status = status.lock();
                    status.private_tick_at = now;
                    status.pulses += 1;
                    status.phase = if brain
                        .mind
                        .thought_refresh_in_flight
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        "thinking"
                    } else if presence == PresenceState::Rest {
                        "resting"
                    } else {
                        "waiting"
                    }
                    .into();
                }
                let can_dispatch = !context.lay_low;
                if can_dispatch && peer_job.0.as_ref().is_none_or(|job| job.is_finished()) {
                    let brain = brain.clone();
                    let state = state.clone();
                    let app = app.clone();
                    let id = id.clone();
                    peer_job.0 = Some(tokio::spawn(async move {
                        brain
                            .proactive
                            .fire_character_peer_opportunity(&context, context.now);
                        if !state.get_character(Some(&id)).is_ok_and(|current| {
                            Arc::ptr_eq(&current.brain.cognitive_tick, &brain.cognitive_tick)
                        }) {
                            return;
                        }
                        let messages = brain.proactive.drain_peer_messages();
                        crate::commands::proactive::deliver_cross_character_messages(
                            &app, &state, &id, messages,
                        )
                        .await;
                    }));
                }
                if can_dispatch && inbox_job.0.as_ref().is_none_or(|job| job.is_finished()) {
                    inbox_job.0 = Some(tokio::spawn(super::interaction_runtime::process_peer(app.clone(), state.clone(), id.clone())));
                }
            }
        } else {
            status.lock().phase = "offline".into();
        }
        // Notify wakes a character promptly; the one-second delay coalesces event bursts.
        // A completed synthesis only wakes delivery; it does not set refresh_requested.
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            _ = async { match wake { Some(wake) => wake.notified().await, None => std::future::pending::<()>().await } } => {
                wake_requested = true;
                tokio::select! { _ = cancel.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            },
        }
    }
}
