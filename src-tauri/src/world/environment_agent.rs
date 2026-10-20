//! Non-character environment agent: one observation stream and one bounded planning budget.
//! It reports observations and selects an opportunity. Character brains retain all speech.
use super::environment_input::EnvironmentInput;
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
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
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
}

#[derive(Default)]
pub struct EnvironmentAgent {
    observations: Mutex<Observations>,
    planning: Arc<tokio::sync::Mutex<()>>,
    input: EnvironmentInput,
    priority: OnceLock<Arc<BackgroundPriority>>,
    last_sample: AtomicI64,
    sampling: AtomicBool,
    suppressed_until: AtomicU64,
}

struct SamplingGuard<'a>(&'a AtomicBool);
impl Drop for SamplingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
            return;
        }
        let Some(idle) = crate::utils::get_system_idle_seconds() else {
            self.observations.lock().hint.clear();
            return;
        };
        let roles = participants(state);
        let user_available = idle <= config.proactive.away_threshold_seconds as f64;
        let chatting = state.session_coordinator.any_user_turn_active()
            || (crate::conversation::CONVERSATION_MANAGER.is_any_user_session_active()
                && idle < 90.0);
        let snap = state.world_provider.snapshot(None);
        let (events, user_turns, current_intent, epoch, input_revision) = {
            let mut inputs = self.input.drain();
            inputs.sort_by_key(|input| input.revision);
            let mut obs = self.observations.lock();
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
            observe(&mut obs, &snap, &roles, idle, now);
            obs.policy.reconcile(&roles, user_available, chatting, now);
            if !obs.policy.begin_plan(now, &roles) {
                return;
            }
            (
                obs.policy.events(now),
                obs.policy.recent_user_turns(now),
                obs.policy.current_intent(now).cloned(),
                obs.epoch,
                obs.processed_input_revision,
            )
        };
        let Ok(permit) = self.planning.clone().try_lock_owned() else {
            return;
        };
        let router = {
            let characters = state.characters.read();
            roles
                .iter()
                .find(|r| r.available)
                .and_then(|r| characters.get(&r.id))
                .map(|c| c.brain.router.clone())
        };
        let Some(router) = router else {
            return;
        };
        let Some(priority) = self.background_priority() else {
            return;
        };
        let scene = self.observations.lock().hint.clone();
        let agent = self.clone();
        let state = state.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = permit;
            let messages = vec![
                ChatMessage::system(include_str!(
                    "../../prompts/framework/environment_agent.en.md"
                )),
                ChatMessage::user(
                    json!({"scene":scene,"user_available":user_available,
                    "user_chatting":chatting,"participants":roles,"events":events,
                    "recent_user_turns":user_turns,"current_intent":current_intent})
                    .to_string(),
                ),
            ];
            let request = LLMRequest::new("environment_judge", messages)
                .without_framework_instructions()
                .with_temperature(0.0)
                .with_max_tokens(512);
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
            let currently_chatting = state.session_coordinator.any_user_turn_active()
                || (crate::conversation::CONVERSATION_MANAGER.is_any_user_session_active()
                    && current_idle.is_none_or(|idle| idle < 90.0));
            let permitted = !state.session_coordinator.any_user_turn_active()
                && cfg.proactive.enabled
                && crate::companion_runtime::quiet_reason(&state).is_none()
                && !state.is_factory_reset_in_progress()
                && !state.is_rebuild_in_progress();
            let mut obs = agent.observations.lock();
            if let Some(decision) = decision.filter(|_| {
                permitted && obs.epoch == epoch && agent.input.revision() == input_revision
            }) {
                if let Some(intent) = decision.user_intent {
                    obs.policy
                        .apply_intent(intent, chrono::Local::now().timestamp() as f64);
                }
                let accepted = decision.opportunity.is_some_and(|selection| {
                    obs.policy.accept(
                        selection,
                        &current_roles,
                        current_idle
                            .is_some_and(|i| i <= cfg.proactive.away_threshold_seconds as f64),
                        currently_chatting,
                        chrono::Local::now().timestamp() as f64,
                    )
                });
                agent
                    .suppressed_until
                    .store(obs.policy.suppressed_until().to_bits(), Ordering::Release);
                tracing::debug!(accepted, "[Environment] control-flow decision evaluated");
            }
            // Abstention/failure is terminal for these events, never a per-tick retry storm.
            obs.policy
                .finish_evaluation(&events.iter().map(|e| e.id.clone()).collect::<Vec<_>>());
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
        self.observations.lock().policy.take(id, now)
    }
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
