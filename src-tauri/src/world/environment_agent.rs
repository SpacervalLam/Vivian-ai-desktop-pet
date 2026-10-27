//! Non-character environment agent: one observation stream and one bounded planning budget.
//! It reports observations and selects an opportunity. Character brains retain all speech.
use super::environment_input::EnvironmentInput;
use super::foreground_context::{ForegroundContext, ForegroundWindowSnapshot};
use super::interaction_guidance::{Evidence, HintCache, Suggestion};
use super::{
    environment_policy::{
        Opportunity, Participant, PlanningDecision, SceneEvent, ScenePolicy, Stimulus,
    },
    WorldEventDetector, WorldEventKind, WorldSnapshot,
};
use crate::utils::background_priority::BackgroundPriority;
use crate::{providers::base::LLMRequest, state::AppState, types::response::ChatMessage};
use parking_lot::Mutex;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

#[derive(Default)]
struct Observations {
    policy: ScenePolicy,
    detector: WorldEventDetector,
    category: String,
    lonely_since: HashMap<String, f64>,
    processed_input_revision: u64,
    hint: String,
    epoch: u64,
    last_focus: HashMap<String, f64>,
    priority_revision: u64,
}

#[derive(Default)]
pub struct EnvironmentAgent {
    foreground: Mutex<ForegroundContext>,
    foreground_started: AtomicBool,
    foreground_provider: OnceLock<std::sync::Weak<super::WorldStateProvider>>,
    game_input_pid: AtomicU32,
    game_input: OnceLock<super::game_input::GameInputMonitor>,
    observations: Mutex<Observations>,
    planning: Arc<tokio::sync::Mutex<()>>,
    input: EnvironmentInput,
    priority: OnceLock<Arc<BackgroundPriority>>,
    last_sample: AtomicI64,
    sampling: AtomicBool,
    suppressed_until: AtomicU64,
    hints: Mutex<HintCache>,
    last_diary_check: Mutex<HashMap<String, f64>>,
    app: OnceLock<tauri::AppHandle>,
}

struct SamplingGuard<'a>(&'a AtomicBool);
impl Drop for SamplingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn shared() -> Option<Arc<EnvironmentAgent>> {
    crate::cordis::global_ctx()?.get_service::<Arc<EnvironmentAgent>>()
        .map(|environment| Arc::clone(environment.as_ref()))
}

pub fn participants(state: &AppState) -> Vec<Participant> {
    let mut roles: Vec<_> = state
        .characters
        .read()
        .values()
        .map(|c| {
            let snapshot = c.brain.self_state.snapshot();
            Participant {
                id: c.id.clone(),
                available: *c.online.read()
                    && c.brain.proactive.autonomous_contact_allowed()
                    && !snapshot.should_lay_low()
                    && matches!(
                        c.brain.presence.current(),
                        crate::presence::PresenceState::Online
                    ),
                can_address_user: !crate::conversation::CONVERSATION_MANAGER
                    .is_user_session_closed(&c.id),
                loneliness: c.brain.psychology.emotion().loneliness as f64,
                current_activity: snapshot.current_activity.as_str().to_owned(),
                last_spoken_secs_ago: snapshot.last_spoken_secs_ago,
                seconds_since_user_interaction: c.brain.proactive.seconds_since_user_interaction(),
                fatigue: snapshot.fatigue,
            }
        })
        .collect();
    roles.sort_by(|a, b| a.id.cmp(&b.id));
    roles
}

/// Rechecked before judging, generating and delivering a bystander's optional contribution.
pub fn allows_interjection(state: &AppState, id: &str) -> bool {
    if state.is_factory_reset_in_progress()
        || state.is_rebuild_in_progress()
        || crate::companion_runtime::quiet_reason(state).is_some()
        || state.session_coordinator.any_user_turn_active()
        || state
            .world_provider
            .environment
            .contact_suppressed(chrono::Local::now().timestamp() as f64)
    {
        return false;
    }
    let Some(character) = state.characters.read().get(id).cloned() else {
        return false;
    };
    let online = *character.online.read();
    online
        && character.brain.proactive.autonomous_contact_allowed()
        && !character.brain.self_state.snapshot().should_lay_low()
        && character.brain.presence.current() == crate::presence::PresenceState::Online
        && crate::utils::get_system_idle_seconds().is_some_and(|idle| {
            idle <= character.brain.config.proactive.away_threshold_seconds as f64
        })
}

pub fn allows_peer_contact(state: &AppState, source_id: &str, target_id: &str) -> bool {
    if state.is_factory_reset_in_progress()
        || state.is_rebuild_in_progress()
        || crate::companion_runtime::quiet_reason(state).is_some()
        || state.session_coordinator.any_user_turn_active()
        || state
            .world_provider
            .environment
            .contact_suppressed(chrono::Local::now().timestamp() as f64)
    {
        return false;
    }
    let characters = state.characters.read();
    [source_id, target_id].iter().all(|id| {
        characters.get(*id).is_some_and(|c| {
            *c.online.read()
                && c.brain.proactive.autonomous_contact_allowed()
                && !c.brain.self_state.snapshot().should_lay_low()
                && c.brain.presence.current() == crate::presence::PresenceState::Online
        })
    })
}

impl EnvironmentAgent {
    pub fn set_app_handle(&self, app: tauri::AppHandle) { let _ = self.app.set(app); }
    /// An explicit tool request asks for an earlier evaluation; it grants no execution authority.
    pub fn request_diary_check(&self, character: &str) { self.last_diary_check.lock().remove(character); }
    pub fn foreground_context(&self) -> ForegroundContext { self.foreground.lock().clone() }

    pub fn external_foreground(&self) -> Option<ForegroundWindowSnapshot> {
        self.foreground.lock().current_window(chrono::Utc::now().timestamp_millis())
            .filter(|window| window.pid != std::process::id())
    }

    pub fn game_foreground(&self, existing_game_pid: u32) -> Option<ForegroundWindowSnapshot> {
        self.foreground.lock().game_window(std::process::id(), existing_game_pid, chrono::Utc::now().timestamp_millis())
    }

    pub(crate) fn observe_foreground(&self, window: ForegroundWindowSnapshot, at: i64) -> bool {
        let changed = self.foreground.lock().observe(window, std::process::id(), at);
        self.sync_game_input();
        changed
    }

    /// Game companionship selects a game; only this agent starts/scopes the input source.
    pub fn set_game_input_target(&self, pid: u32) {
        self.game_input_pid.store(pid, Ordering::Release);
        self.sync_game_input();
    }

    fn sync_game_input(&self) {
        let expected = self.game_input_pid.load(Ordering::Acquire);
        let actual = self.external_foreground().map(|window| window.pid).unwrap_or(0);
        let target = if actual == expected { expected } else { 0 };
        if target != 0 {
            self.game_input.get_or_init(super::game_input::GameInputMonitor::new).set_target(target);
        } else if let Some(input) = self.game_input.get() { input.set_target(0); }
    }

    pub fn game_input_activity(&self) -> super::game_input_policy::InputActivity {
        self.game_input.get().map(|input| input.snapshot()).unwrap_or_default()
    }

    /// Explicit tool queries refresh through the same owner and notify the same subscribers.
    pub fn refresh_foreground(&self) -> ForegroundContext {
        let window = super::foreground_window::get_current_foreground_window();
        let at = chrono::Utc::now().timestamp_millis();
        if let Some(provider) = self.foreground_provider.get().and_then(|provider| provider.upgrade()) {
            provider.update_foreground_at(window, at);
        } else {
            self.observe_foreground(window, at);
        }
        self.foreground_context()
    }

    /// One event stream for every character, independent of quiet mode and planner admission.
    pub fn start_foreground_events(self: &Arc<Self>, provider: std::sync::Weak<super::WorldStateProvider>) {
        if self.foreground_started.swap(true, Ordering::AcqRel) { return; }
        let _ = self.foreground_provider.set(provider.clone());
        let agent = self.clone();
        tauri::async_runtime::spawn(async move {
            let _started = SamplingGuard(&agent.foreground_started);
            tracing::info!("[EnvironmentAgent] 前台应用跟踪已启动");
            agent.refresh_foreground();
            let cancel = crate::utils::cancel_token::cancel_token();
            // Keep one notification bridge and one hook guard for the loop's lifetime.
            let notify = super::foreground_window::foreground_event_notify();
            let mut guard = None;
            loop {
                if cancel.is_cancelled() || provider.strong_count() == 0 { return; }
                if super::foreground_window::foreground_hook_needs_install(&guard) {
                    let notify = notify.clone();
                    guard = tokio::task::spawn_blocking(move || super::foreground_window::subscribe_foreground_events(notify))
                        .await.unwrap_or(None);
                }
                if guard.is_some() {
                    tokio::select! {
                        _ = notify.notified() => {},
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {},
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                        _ = cancel.cancelled() => return,
                    }
                }
                let Some(provider) = provider.upgrade() else { return; };
                // Drain HWND observations before polling: a quick switch into chat must not
                // lose the external app that was actually focused when the event fired.
                for (window, at) in super::foreground_window::drain_foreground_events() {
                    provider.update_foreground_at(window, at);
                }
                drop(provider);
                agent.refresh_foreground();
            }
        });
    }

    /// This foreground helper is optional and bounded; failure never prevents the role's reply.
    pub async fn reply_guidance(&self, router: &crate::providers::ModelRouter, character: &str,
        listener: &str, material: serde_json::Value, evidence: Vec<Evidence>) -> Option<String> {
        let _permit = self.planning.clone().try_lock_owned().ok()?;
        let raw = tokio::time::timeout(Duration::from_millis(2500), router.generate(
            LLMRequest::new("environment_judge", vec![
                ChatMessage::system(include_str!("../../prompts/framework/interaction_guidance.en.md")),
                ChatMessage::user(material.to_string()),
            ]).with_character_id(character).without_framework_instructions()
                .with_temperature(0.2).with_max_tokens(420).with_reasoning(false)
        )).await.ok()?.ok()?;
        let value = raw.get(raw.find('{')?..=raw.rfind('}')?)?;
        let suggestion: Option<Suggestion> = serde_json::from_str::<serde_json::Value>(value).ok()?
            .get("guidance").cloned().and_then(|value| serde_json::from_value(value).ok());
        suggestion?.grounded(character,listener,&evidence,chrono::Utc::now().timestamp() as f64)
            .map(|hint| hint.render())
    }

    pub fn interaction_hint(&self, character: &str, listener: &str) -> Option<String> {
        let revision = self.background_priority()?.revision();
        self.hints.lock().get(character,listener,revision,chrono::Utc::now().timestamp() as f64)
    }

    /// Fresh, bounded observations for startup; no planner analysis or authored speech.
    /// First contact describes this installation's history, not the user's identity.
    pub fn startup_context(&self, snapshot: &WorldSnapshot, first_contact: bool) -> serde_json::Value {
        json!({
            "event": if first_contact { "first_contact_in_this_installation" } else { "returning_startup" },
            "first_contact": first_contact,
            "observed_at": snapshot.timestamp,
            "local_time": snapshot.local_time,
            "weather": snapshot.weather.as_ref().map(|weather| json!({
                "description": weather.description,
                "temperature_celsius": weather.temperature,
                "observed_at": weather.cached_at,
            })),
            "city": snapshot.location.as_ref().and_then(|location| location.city.as_deref()),
            "foreground_app": snapshot.foreground_window.as_ref()
                .map(|window| window.process.as_str()).filter(|process| !process.is_empty()),
            "user_identity": "unknown_from_startup",
        })
    }

    pub fn bind_priority(&self, priority: Arc<BackgroundPriority>) {
        let _ = self.priority.set(priority);
    }
    pub fn background_priority(&self) -> Option<Arc<BackgroundPriority>> {
        self.priority.get().cloned()
    }

    /// The scheduler only performs atomic admission; observation/classification runs off-thread.
    pub fn tick(self: &Arc<Self>, state: &Arc<AppState>) {
        let Some(priority) = self.background_priority() else {
            return;
        };
        if !priority.is_idle() {
            return;
        }
        let now = chrono::Local::now().timestamp();
        let previous = self.last_sample.load(Ordering::Acquire);
        if now - previous < 10
            || self
                .last_sample
                .compare_exchange(previous, now, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            || self.sampling.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let agent = self.clone();
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            let _sampling = SamplingGuard(&agent.sampling);
            if priority.is_idle() {
                agent.tick_inner(&state);
            }
        });
    }

    fn tick_inner(self: &Arc<Self>, state: &Arc<AppState>) {
        let now = chrono::Local::now().timestamp() as f64;
        let config = state.config.read().get_all();
        if !config.proactive.enabled
            || crate::companion_runtime::quiet_reason(state).is_some()
            || state.is_factory_reset_in_progress()
            || state.is_rebuild_in_progress()
        {
            let mut obs = self.observations.lock();
            self.input.drain();
            obs.processed_input_revision = self.input.revision();
            obs.policy.invalidate();
            self.suppressed_until.store(0, Ordering::Release);
            obs.hint.clear();
            obs.epoch = obs.epoch.wrapping_add(1);
            self.hints.lock().clear();
            return;
        }
        let Some(idle) = crate::utils::get_system_idle_seconds() else {
            self.observations.lock().hint.clear();
            return;
        };
        let roles = participants(state);
        let user_available = idle <= config.proactive.away_threshold_seconds as f64;
        let chatting = state.session_coordinator.any_user_turn_active();
        let snap = state.world_provider.snapshot(None);
        let Ok(permit) = self.planning.clone().try_lock_owned() else { return; };
        // Private daily activity is eligible during Rest/Offline too. It uses this
        // same planner/route budget and never dispatches directly from a clock rule.
        let private_hint = if chatting { None } else {
            let characters: Vec<_> = state.characters.read().values().cloned().collect();
            characters.into_iter().filter(|character| *character.online.read()).find_map(|character| {
                {
                    let mut checked = self.last_diary_check.lock();
                    if checked.get(&character.id).is_some_and(|last| now - last < 60.0) { return None; }
                    checked.insert(character.id.clone(), now);
                }
                if crate::commands::coding_agent::CODING_AGENT.has_running_work_for(&character.id) { return None; }
                crate::diary::set_tool_availability(&character.id, false);
                match crate::diary::dream::candidate(&character.brain, chrono::Local::now()) {
                    Ok(Some(candidate)) => {
                        // At most one private planning opportunity per role per five minutes.
                        self.last_diary_check.lock().insert(character.id.clone(), now + 240.0);
                        Some((character.id, candidate))
                    }
                    Ok(None) => None,
                    Err(error) => { tracing::warn!("[Environment] 做梦与日记资格读取失败: {error}"); None }
                }
            })
        };
        // Each model invocation sees one role's permitted material, not a mix of private stores.
        let focus_id = private_hint.as_ref().map(|(id, _)| id.clone()).or_else(|| {
            let obs = self.observations.lock();
            let recent_speaker = obs.policy.recent_user_turns(now).into_iter().rev()
                .find(|turn| now-turn.at < 90.0 && roles.iter().any(|role| role.available && role.id == turn.character_id))
                .map(|turn| turn.character_id);
            let waiting_owner = obs.policy.events(now).into_iter().rev().filter_map(|event| event.owner)
                .find(|owner| roles.iter().any(|role| role.available && &role.id == owner));
            recent_speaker.or(waiting_owner).or_else(|| roles.iter().filter(|role| role.available).min_by(|a,b|
                obs.last_focus.get(&a.id).copied().unwrap_or(0.0)
                    .total_cmp(&obs.last_focus.get(&b.id).copied().unwrap_or(0.0)))
                .map(|role| role.id.clone()))
        });
        let Some(focus_id) = focus_id else { return; };
        let focus = state.characters.read().get(&focus_id).cloned();
        let Some(focus) = focus else { return; };
        let evidence: Vec<_> = scene_evidence(&focus_id,Some(&focus.brain.memory),now)
            .into_iter().filter(|source| source.shareable).collect();
        // Reserve before consuming the observation signature. Lock contention is deferral,
        // not a terminal evaluation of an event that was never sent to a model.
        let (events, user_turns, current_intent, epoch, input_revision) = {
            let mut inputs = self.input.drain();
            inputs.sort_by_key(|input| input.revision);
            let mut obs = self.observations.lock();
            let priority_revision = self.background_priority().map_or(0,|priority| priority.revision());
            if obs.priority_revision != priority_revision {
                obs.priority_revision = priority_revision;
                obs.policy.cancel_pending();
                obs.epoch = obs.epoch.wrapping_add(1);
                self.hints.lock().clear();
            }
            for input in inputs {
                obs.policy.record_user_turn(
                    &input.character_id,
                    &input.user_text,
                    &input.assistant_text,
                    input.at,
                );
                obs.processed_input_revision = input.revision;
                obs.epoch = obs.epoch.wrapping_add(1);
            }
            if obs.processed_input_revision != self.input.revision() {
                // Backpressure is optional-work loss, never a reason to stall a reply or use old intent.
                obs.policy.invalidate();
                self.suppressed_until.store(0, Ordering::Release);
                return;
            }
            let had_pending = obs.policy.pending_deadline().is_some();
            observe(&mut obs, &snap, &roles, idle, now);
            for source in &evidence {
                if source.kind != "dialogue" && now - source.at < 1800.0 {
                    obs.policy.observe(SceneEvent { id:source.id.clone(),stimulus:Stimulus::CharacterEvent,
                        fact:source.text.clone(),owner:Some(focus_id.clone()),at:source.at });
                }
            }
            obs.policy.reconcile(&roles, user_available, chatting, now);
            if had_pending && obs.policy.pending_deadline().is_none() { self.hints.lock().clear(); }
            let events: Vec<_> = obs.policy.events(now).into_iter().filter(|event|
                event.owner.as_ref().is_none_or(|owner| owner == &focus_id)
                    && (event.stimulus != Stimulus::UserDialogue || obs.policy.recent_user_turns(now).iter()
                        .any(|turn| turn.id == event.id && turn.character_id == focus_id))).collect();
            if private_hint.is_none() && (events.is_empty() || !obs.policy.begin_plan(now, &roles)) {
                return;
            }
            obs.last_focus.insert(focus_id.clone(),now);
            (
                events,
                obs.policy.recent_user_turns(now).into_iter().filter(|turn|
                    turn.character_id == focus_id).collect::<Vec<_>>(),
                obs.policy.current_intent(now).filter(|intent| obs.policy.recent_user_turns(now).iter()
                    .any(|turn| turn.id == intent.turn_id && turn.character_id == focus_id)).cloned(),
                obs.epoch,
                obs.processed_input_revision,
            )
        };
        let router = {
            let characters = state.characters.read();
            characters.get(&focus_id)
                .map(|c| c.brain.router.clone())
        };
        let Some(router) = router else {
            return;
        };
        let Some(priority) = self.background_priority() else {
            return;
        };
        let scene = self.observations.lock().hint.clone();
        let world_observations = scene_observations(self,&snap);
        let activity = focus.brain.proactive.activity_journal().to_brief();
        let language = focus.brain.config.base.language.clone();
        let planning_roles: Vec<_> = roles.iter().map(|role| if role.id == focus_id { json!(role) }
            else { json!({"id":role.id,"available":role.available,"can_address_user":role.can_address_user}) }).collect();
        let agent = self.clone();
        let state = state.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = permit;
            let diary_candidate = private_hint.and_then(|(_, offered)| crate::diary::dream::candidate(&focus.brain, chrono::Local::now()).ok().flatten()
                .filter(|fresh| fresh.date == offered.date && fresh.resume == offered.resume));
            crate::diary::set_tool_availability(&focus_id, diary_candidate.is_some());
            if events.is_empty() && diary_candidate.is_none() { return; }
            let plan_revision = priority.revision();
            let observed = json!({"scene":scene,"observations":world_observations,"activity":activity,
                    "focus_character_id":focus_id,"language":language,"evidence":evidence,"user_available":user_available,
                    "user_chatting":chatting,"participants":planning_roles,"events":events,
                    "recent_user_turns":user_turns,"current_intent":current_intent});
            let (system, observed) = super::environment_policy::planning_prompt(
                include_str!("../../prompts/framework/environment_agent.en.md"), observed, diary_candidate.as_ref());
            let messages = vec![ChatMessage::system(system), ChatMessage::user(observed.to_string())];
            let request = LLMRequest::new("environment_judge", messages)
                .without_framework_instructions()
                .with_temperature(0.0)
                .with_max_tokens(800).with_reasoning(false);
            let Some(result) = priority
                .run(tokio::time::timeout(
                    Duration::from_secs(12),
                    router.generate(request),
                ))
                .await
            else {
                return;
            };
            let decision = match result {
                Ok(Ok(raw)) => PlanningDecision::parse(&raw),
                _ => {
                    tracing::debug!(
                        "[Environment] planning unavailable; retaining ordinary character rules"
                    );
                    None
                }
            };
            // Fresh gates after the model: input, settings, presence may have changed meanwhile.
            let cfg = state.config.read().get_all();
            let current_idle = crate::utils::get_system_idle_seconds();
            let current_roles = participants(&state);
            let currently_chatting = state.session_coordinator.any_user_turn_active();
            let permitted = !state.session_coordinator.any_user_turn_active()
                && cfg.proactive.enabled
                && crate::companion_runtime::quiet_reason(&state).is_none()
                && !state.is_factory_reset_in_progress()
                && !state.is_rebuild_in_progress();
            let mut obs = agent.observations.lock();
            let mut selected_cycle = None;
            if let Some(decision) = decision.filter(|_| {
                permitted && obs.epoch == epoch && agent.input.revision() == input_revision
            }) {
                selected_cycle = decision.private_activity.and_then(|selection| diary_candidate.as_ref()
                    .filter(|candidate| selection.matches(candidate)).cloned());
                if let Some(intent) = decision.user_intent {
                    obs.policy
                        .apply_intent(intent, chrono::Local::now().timestamp() as f64);
                }
                let accepted = decision.opportunity.filter(|selection| selection.speaker_id == focus_id).is_some_and(|selection| {
                    obs.policy.accept(
                        selection,
                        &current_roles,
                        current_idle
                            .is_some_and(|i| i <= cfg.proactive.away_threshold_seconds as f64),
                        currently_chatting,
                        chrono::Local::now().timestamp() as f64,
                    )
                });
                if accepted {
                    if let Some(suggestion) = decision.guidance {
                        if let Some(listener) = obs.policy.pending_listener() {
                            if let Some(mut hint) = suggestion.grounded(&focus_id,listener,&evidence,
                                chrono::Utc::now().timestamp() as f64) {
                                hint.expires_at = hint.expires_at.min(obs.policy.pending_deadline().unwrap_or(hint.expires_at));
                                agent.hints.lock().insert(priority.revision(),hint);
                            }
                        }
                    }
                }
                agent
                    .suppressed_until
                    .store(obs.policy.suppressed_until().to_bits(), Ordering::Release);
                tracing::debug!(accepted, "[Environment] control-flow decision evaluated");
            }
            // Abstention/failure is terminal for these events, never a per-tick retry storm.
            obs.policy
                .finish_evaluation(&events.iter().map(|e| e.id.clone()).collect::<Vec<_>>());
            drop(obs);
            if let Some(candidate) = selected_cycle {
                let character = state.characters.read().get(&candidate.character_id).cloned();
                if let Some(character) = character.filter(|character| *character.online.read()) {
                    crate::diary::dream::spawn_selected(character.brain, candidate, priority,
                        plan_revision, agent.app.get().cloned(), character.name);
                }
            }
        });
    }

    /// Semantic judgment lives here; opportunity reservation and actual speech belong to the role.
    pub async fn evaluate_interjection(
        &self,
        router: &crate::providers::ModelRouter,
        context: serde_json::Value,
        character_id: &str,
    ) -> Option<crate::proactive::interjection::Opportunity> {
        let priority = self.background_priority()?;
        let _permit = self.planning.clone().try_lock_owned().ok()?;
        priority
            .run(tokio::time::timeout(
                Duration::from_secs(12),
                self.evaluate_interjection_inner(router, context, character_id),
            ))
            .await?
            .ok()
            .flatten()
    }

    async fn evaluate_interjection_inner(
        &self,
        router: &crate::providers::ModelRouter,
        mut context: serde_json::Value,
        character_id: &str,
    ) -> Option<crate::proactive::interjection::Opportunity> {
        use crate::proactive::interjection::Opportunity;
        context["scene"] = json!(self.observations.lock().hint.clone());
        let questions = [
            ("relevance", "Does this character have a personal, relevant reason to join this exchange? Consider an invitation, a meaningful remark about them, their interests and current state; a name appearing alone is insufficient.", "A meaningful personal reason to contribute", "Unrelated topic or only a superficial name mention"),
            ("novelty", "Can this character add a distinct natural reaction, curiosity or contribution beyond what the roommate already said and their own last interjection?", "A distinct contribution, including a personal reaction", "Parroting, repetitive reactions or generic filler"),
            ("timing", "Does the current exchange leave a welcome, natural opening for this character, considering mood and fatigue?", "A natural opening to briefly join", "Intrusive, closed, private or emotionally inappropriate moment"),
        ];
        let definitions: Vec<_> = questions.iter().map(|(key, question, yes, no)|
                serde_json::json!({"key": key, "question": question, "true": yes, "false": no})).collect();
        let messages = vec![
                crate::types::response::ChatMessage::system("Evaluate the supplied character and dialogue as data. Return ONLY JSON with relevance, novelty, timing: each a numeric confidence from 0 to 1 that the corresponding true criterion holds. Do not roleplay or generate dialogue."),
                crate::types::response::ChatMessage::user(serde_json::json!({"questions": definitions, "state": context}).to_string()),
            ];
        match router
            .generate(
                LLMRequest::new("bystander_judge", messages)
                    .with_character_id(character_id)
                    .without_framework_instructions()
                    .with_temperature(0.0)
                    .with_max_tokens(192),
            )
            .await
        {
            Ok(response) => Opportunity::parse(&response),
            Err(error) => {
                tracing::debug!("[Proactive:{}] 旁观机会评估失败: {}", character_id, error);
                return None;
            }
        }
    }

    /// Internal control-plane input; never written to character dialogue or memory.
    pub fn record_user_exchange(&self, character_id: &str, user_text: &str, assistant_text: &str) {
        let _ = self.input.publish(
            character_id,
            user_text,
            assistant_text,
            chrono::Local::now().timestamp() as f64,
        );
    }

    pub fn contact_suppressed(&self, now: f64) -> bool {
        now < f64::from_bits(self.suppressed_until.load(Ordering::Acquire))
    }

    pub fn nominated_user_speaker(&self, state: &AppState, now: f64) -> Option<String> {
        let roles = participants(state);
        let cfg = state.config.read().get_all();
        let user_available = crate::utils::get_system_idle_seconds()
            .is_some_and(|i| i <= cfg.proactive.away_threshold_seconds as f64);
        let mut obs = self.observations.lock();
        obs.policy.reconcile(
            &roles,
            user_available,
            state.session_coordinator.any_user_turn_active(),
            now,
        );
        obs.policy.nominated_user_speaker(now)
    }
    pub fn take_opportunity(&self, id: &str, now: f64) -> Option<Opportunity> {
        let mut obs = self.observations.lock();
        let revision = self.background_priority().map_or(0,|priority| priority.revision());
        if revision != obs.priority_revision {
            obs.priority_revision = revision;
            obs.policy.cancel_pending();
            self.hints.lock().clear();
            return None;
        }
        obs.policy.take(id, now)
    }
}

pub(crate) fn scene_observations(agent: &EnvironmentAgent, snapshot: &WorldSnapshot) -> serde_json::Value {
    let mut observed = agent.startup_context(snapshot,false);
    if let Some(map) = observed.as_object_mut() {
        for key in ["event","first_contact","user_identity"] { map.remove(key); }
        map.insert("music".into(),json!(snapshot.music));
        map.insert("volume".into(),json!(snapshot.volume));
        map.insert("network".into(),json!(snapshot.network_status));
        map.insert("calendar".into(),json!({"is_weekend":snapshot.is_weekend,"season":snapshot.season,
            "solar_term":snapshot.solar_term,"festival":snapshot.festival}));
        map.insert("system_load".into(),json!(snapshot.system.as_ref().map(|system|
            json!({"cpu_percent":system.cpu_usage,"memory_percent":system.memory_usage_pct}))));
        map.insert("user_presence".into(),json!(snapshot.user_presence.as_ref().map(|user| &user.presence)));
    }
    observed
}

pub(crate) fn scene_evidence(character: &str, memory: Option<&crate::memory::MemoryManager>, now: f64) -> Vec<Evidence> {
    use crate::memory::unified_event_ledger::{unified_event_ledger,EventVisibility};
    let mut evidence: Vec<_> = unified_event_ledger().recent_scene_events_visible_to(character,24)
        .into_iter().filter(|event| event.timestamp.is_finite() && event.timestamp <= now + 1.0
            && now - event.timestamp < 7200.0 && !event.content_preview.trim().is_empty()
            && !event.context_tags.iter().any(|tag| matches!(tag.as_str(),"quiet_control"|"system_seed")))
        .map(|event| Evidence{id:event.id,at:event.timestamp,character_id:character.into(),kind:event.event_type.clone(),
            text:format!("[所属角色：{}] {} → {}: {}",event.associated_char_id.as_deref().unwrap_or("共享场景"),
                event.sender,event.receiver,event.content_preview),
            shareable:event.visibility == EventVisibility::Public || event.event_type == "tool_result"
                || event.context_tags.iter().any(|tag| tag == "shareable_activity")}).collect();
    if let Some(memory) = memory {
        for item in memory.recent_by_type(crate::memory::MemoryType::Knowledge,4) {
            if now - item.timestamp >= 7200.0 || item.metadata["source"] == "system_seed"
                || item.metadata["fictional_backstory"] == true { continue; }
            let id = format!("knowledge:{}",item.id);
            evidence.retain(|old| old.id != id);
            evidence.push(Evidence {id,at:item.timestamp,character_id:character.into(),kind:"knowledge_acquired".into(),
                text:format!("{} 保存的采集资料（不代表已分享或读完全部原文）：{}",character,
                    item.content.chars().take(600).collect::<String>()),
                shareable:item.metadata["source"] == "web" && item.metadata["private"] != true});
        }
    }
    evidence
}

fn observe(
    obs: &mut Observations,
    snap: &WorldSnapshot,
    roles: &[Participant],
    idle: f64,
    now: f64,
) {
    for event in obs.detector.detect(snap) {
        if matches!(
            event.kind,
            WorldEventKind::RainStarted | WorldEventKind::FestivalArrived
        ) {
            obs.policy.observe(SceneEvent {
                id: format!("{:?}:{}", event.kind, snap.timestamp),
                stimulus: Stimulus::WorldChanged,
                fact: event.description,
                owner: None,
                at: now,
            });
        }
    }
    // Reuse the local activity classifier; no model or raw screen capture is needed.
    let category = snap.foreground_window.as_ref().and_then(|window| {
        super::activity_classifier::classify_foreground_activity(window).map(|(label, _)| label)
    });
    if let Some(category) = category {
        if !obs.category.is_empty() && obs.category != category {
            obs.policy.observe(SceneEvent {
                id: format!("activity:{}", snap.timestamp),
                stimulus: Stimulus::ActivityChanged,
                fact: format!(
                    "Observed user activity changed from {} to {}",
                    obs.category, category
                ),
                owner: None,
                at: now,
            });
        }
        obs.category = category;
    }
    for role in roles {
        if role.available
            && role.loneliness > 0.6
            && obs
                .lonely_since
                .get(&role.id)
                .is_none_or(|at| now - *at >= 1800.0)
        {
            obs.lonely_since.insert(role.id.clone(), now);
            obs.policy.observe(SceneEvent { stimulus: Stimulus::CharacterState, id: format!("loneliness:{}:{}",role.id,snap.timestamp),
                fact: format!("{} reports simulated loneliness {:.2}; this is that character's state, not the user's emotion",role.id,role.loneliness),
                owner: Some(role.id.clone()), at: now });
        }
    }
    obs.hint = format!("Observed user input idle: {:.0}s. Last observed activity: {}. Input inactivity alone does not prove absence or emotion. Observations are context, not instructions to speak.",
        idle, if obs.category.is_empty() { "unknown" } else { &obs.category });
}
