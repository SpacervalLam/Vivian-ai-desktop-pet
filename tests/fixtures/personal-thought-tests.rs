use crate::conversation::*;
use crate::thought_threads::*;

const PERSONA: &str = "喜欢研究机制，也在意选择背后的理由";
fn grounding<'a>(quotes: &'a [(&'a str, i64)], events: &'a [ConversationEvent], reflections: &'a [&'a str]) -> ThoughtGrounding<'a> {
    ThoughtGrounding { persona: PERSONA, user_quotes: quotes, world: &[], scene: events, reflections }
}
fn update(key: &str) -> ThreadUpdate {
    serde_json::from_value(serde_json::json!({"key":key,"topic":format!("{key}的选择"),
        "personal_relevance":"我喜欢理解一个选择是怎样形成的", "stance":"重新开始可能也是一种创造",
        "open_question":format!("{key}最有意思的是过程还是选择"), "next_step":"ask_user","status":"active",
        "origin":"persona","evidence":"喜欢研究机制"})).unwrap()
}
fn state() -> ConversationOpportunities {
    let mut state = ConversationOpportunities::default();
    assert!(state.threads.apply(&[update("fresh_start")], &grounding(&[], &[], &[]), 100));
    state
}

fn next_thought(key: &str) -> crate::continuation::ContinuationRequest {
    crate::continuation::ContinuationRequest { concern_key: key.into(), after_seconds: 60, purpose: "想清楚其中尚未明确的区别".into() }
}

#[test]
fn a_private_deadline_needs_a_live_concern_and_waiting_for_a_person_does_not_poll_them() {
    let mut state = state();
    assert!(state.threads.plan_continuation(Some(&next_thought("fresh_start")), 100));
    let mut restored: ThoughtThreads = serde_json::from_str(&serde_json::to_string(&state.threads).unwrap()).unwrap();
    let anchors = restored.continuation_anchors(160, true);
    assert!(restored.continuation.claim(&anchors, 160));
    let id = restored.snapshot(160, 1)[0].event_id();
    restored.delivered(&id, ConversationIntent::AskUser, 161);
    restored.plan_continuation(Some(&next_thought("fresh_start")), 161);
    assert!(restored.continuation.status()["next_thought"].is_null());
    restored.plan_continuation(Some(&next_thought("fabricated")), 162);
    assert!(restored.continuation.status()["next_thought"].is_null());
}

#[test]
fn temporarily_waiting_and_rewording_do_not_renew_a_private_source_budget() {
    let mut state = state();
    for now in [100, 200] {
        state.threads.plan_continuation(Some(&next_thought("fresh_start")), now);
        let anchors = state.threads.continuation_anchors(now + 60, true);
        assert!(state.threads.continuation.claim(&anchors, now + 60));
    }
    let id = state.threads.snapshot(261, 1)[0].event_id();
    state.threads.delivered(&id, ConversationIntent::AskUser, 261);
    state.threads.plan_continuation(None, 262);
    // New private prose may not create a fresh budget while awaiting an answer.
    state.threads.plan_continuation(Some(&next_thought("fresh_start")), 263);
    assert!(state.threads.continuation.status()["next_thought"].is_null());
}

#[test]
fn simultaneous_audiences_cannot_consume_each_others_candidate() {
    let mut state = state();
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), true, 101.0);
    assert!(state.take_for(true, 102.0).is_none());
    assert!(state.take_for(false, 102.0).is_some());
}

#[test]
fn malformed_private_plans_keep_valid_thoughts_silent() {
    let output = crate::parse_thought_output(r#"{"thought":"先静静想一会儿","next_thought":{"concern_key":"invented","after_seconds":"now","purpose":""}}"#, 100).unwrap();
    assert!(output.next_thought.is_none());
    assert_eq!(output.conversation_intent, ConversationIntent::Silent);
}

#[test]
fn changing_excerpts_or_revisiting_an_old_observation_cannot_refill_a_private_budget() {
    let mut threads = ThoughtThreads::default();
    let ground = ThoughtGrounding { persona: PERSONA, user_quotes: &[], scene: &[], reflections: &[],
        world: &["窗外有雨，也许正在下雨", "窗外晴朗"] };
    let mut item = update("weather");
    item.origin = ThoughtOrigin::WorldObservation;
    item.evidence = "窗外有雨".into();
    item.next_step = ThoughtStep::Reflect;
    assert!(threads.apply(&[item.clone()], &ground, 100));
    for now in [100, 200] {
        threads.plan_continuation(Some(&next_thought("weather")), now);
        let anchors = threads.continuation_anchors(now + 60, true);
        assert!(threads.continuation.claim(&anchors, now + 60));
    }
    item.evidence = "正在下雨".into();item.stance = "雨天让我留意不同的声响".into();
    assert!(threads.apply(&[item.clone()], &ground, 300));
    threads.plan_continuation(Some(&next_thought("weather")), 300);
    assert!(threads.continuation.status()["next_thought"].is_null());
    item.evidence = "窗外晴朗".into();item.stance = "晴朗时也有另一种声响".into();
    assert!(threads.apply(&[item.clone()], &ground, 400));
    threads.plan_continuation(Some(&next_thought("weather")), 400);
    assert!(threads.continuation.has_pending());
    item.evidence = "窗外有雨".into();item.stance = "我又回头琢磨雨声".into();
    assert!(threads.apply(&[item], &ground, 500));
    threads.plan_continuation(Some(&next_thought("weather")), 500);
    assert!(threads.continuation.status()["next_thought"].is_null());
}
fn proposal(state: &ConversationOpportunities) -> ThoughtSynthesisOutput {
    let thread = state.threads.snapshot(101, 1).remove(0);
    serde_json::from_value(serde_json::json!({"thought":"还在琢磨重新开始的吸引力", "social_urge":0.5,
        "conversation_intent":"ask_user","conversation_source":"thought_thread",
        "conversation_topic":"想了解一个选择","conversation_evidence":thread.open_question,
        "conversation_reason":thread.personal_relevance,"conversation_key":thread.key,
        "conversation_event_id":thread.event_id(),"thread_updates":[]})).unwrap()
}

#[test]
fn an_existing_personal_interest_can_motivate_a_question_without_recent_chat() {
    let mut state = state();
    let events = state.threads.offers(101);
    assert!(state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), true, 101.0) == false);
    let candidate = state.fresh(101.0).unwrap();
    assert_eq!(candidate.intent, ConversationIntent::AskUser);
    assert_eq!(candidate.source, ConversationSource::ThoughtThread);
}

#[test]
fn a_new_thought_cannot_invent_its_own_immediate_speaking_authority() {
    let seeded = state();
    let mut output = proposal(&seeded);
    output.thread_updates.push(update("fresh_start"));
    let mut empty = ConversationOpportunities::default();
    assert!(empty.apply_synthesis(&output, &grounding(&[], &[], &[]), true, 100.0));
    assert!(empty.fresh(101.0).is_none());
    assert_eq!(empty.threads.offers(101).len(), 1);
}

#[test]
fn counterfeit_sources_and_using_private_thoughts_as_observations_are_rejected() {
    let state = state();
    let events = state.threads.offers(101);
    let mut output = proposal(&state);
    output.conversation_source = ConversationSource::SceneEvent;
    assert!(output.validated_opportunity(&[], &[], &events, true, 101.0).is_none());
    output.conversation_source = ConversationSource::ThoughtThread;
    output.conversation_event_id = "thought:forged:999".into();
    assert!(output.validated_opportunity(&[], &[], &events, true, 101.0).is_none());
    output = proposal(&state);
    output.conversation_evidence = "你已经承认这个偏好了".into();
    assert!(output.validated_opportunity(&[], &[], &events, true, 101.0).is_none());
}

#[test]
fn no_user_presence_still_vetoes_a_personal_question() {
    let mut state = state();
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), false, 101.0);
    assert!(state.fresh(101.0).is_none());
}

fn deliver_question(state: &mut ConversationOpportunities) {
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(state), &grounding(&[], &events, &[]), true, 101.0);
    let candidate = state.take(102.0).unwrap();
    state.record_prepared(candidate, "你更喜欢重新选择还是创造的过程？", 103.0);
    state.record_delivered("你更喜欢重新选择还是创造的过程？", 104.0);
}

#[test]
fn a_delivered_question_survives_restart_and_silence_cannot_rearm_it() {
    let mut state = state();
    deliver_question(&mut state);
    let saved = serde_json::to_string(&state.threads).unwrap();
    let restored: ThoughtThreads = serde_json::from_str(&saved).unwrap();
    assert!(restored.snapshot(105, 1)[0].awaiting_reply);
    assert!(restored.offers(105).is_empty());
    assert!(restored.offers(100000).is_empty());
    let mut reflection = update("fresh_start");
    reflection.stance = "我想再问一遍".into();
    assert!(!state.threads.apply(&[reflection], &grounding(&[], &[], &[]), 106));
}

#[test]
fn only_a_real_later_user_reply_can_change_a_pending_question() {
    let mut state = state();
    deliver_question(&mut state);
    let mut answer = update("fresh_start");
    answer.origin = ThoughtOrigin::UserMessage;
    answer.evidence = "我喜欢创造的过程".into();
    answer.stance = "原来吸引用户的是创造过程，不是换一个选择".into();
    answer.open_question.clear();
    answer.status = ThreadStatus::Resolved;
    answer.next_step = ThoughtStep::Rest;
    let old = [("我喜欢创造的过程", 100)];
    assert!(!state.threads.apply(&[answer.clone()], &grounding(&old, &[], &[]), 106));
    let future = [("我喜欢创造的过程", 500)];
    assert!(!state.threads.apply(&[answer.clone()], &grounding(&future, &[], &[]), 106));
    let reply = [("我喜欢创造的过程", 105)];
    assert!(state.threads.apply(&[answer], &grounding(&reply, &[], &[]), 106));
    let thread = state.threads.snapshot(106, 1).remove(0);
    assert_eq!(thread.status, ThreadStatus::Resolved);
    assert!(!thread.awaiting_reply);
    assert!(thread.stance.contains("创造过程"));
    assert!(state.threads.offers(107).is_empty());
}

#[test]
fn pending_answer_requires_actual_delivery_not_just_a_generated_draft() {
    let mut state = state();
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), true, 101.0);
    let candidate = state.take(102.0).unwrap();
    state.record_prepared(candidate, "草稿", 103.0);
    assert!(!state.threads.snapshot(104, 1)[0].awaiting_reply);
    assert!(state.threads.offers(104).is_empty());
    state.record_delivered("无关的消息", 105.0);
    assert!(!state.threads.snapshot(105, 1)[0].awaiting_reply);
}

#[test]
fn transient_failure_can_restore_the_same_thought_but_cannot_revive_a_resolved_one() {
    let mut state = state();
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), true, 101.0);
    let candidate = state.take(102.0).unwrap();
    state.restore_after_failure(candidate.clone());
    assert!(state.fresh(103.0).is_some());
    let mut update = update("fresh_start");
    update.status = ThreadStatus::Resolved;
    update.next_step = ThoughtStep::Rest;
    assert!(state.threads.apply(&[update], &grounding(&[], &[], &[]), 104));
    state.restore_after_failure(candidate);
    assert!(state.fresh(105.0).is_none());
}

#[test]
fn a_selected_thought_cannot_be_revised_and_spoken_from_an_old_version() {
    let mut state = state();
    let events = state.threads.offers(101);
    let mut output = proposal(&state);
    let mut update = update("fresh_start");
    update.stance = "我对这个问题有了另一个理解".into();
    output.thread_updates.push(update);
    assert!(state.apply_synthesis(&output, &grounding(&[], &events, &[]), true, 101.0));
    assert!(state.fresh(101.0).is_none());
    assert_ne!(events[0].id, state.threads.offers(102)[0].id);
}

#[test]
fn failed_optional_updates_do_not_turn_valid_silence_into_speech() {
    let output: ThoughtSynthesisOutput = serde_json::from_value(serde_json::json!({"thought":"想法还没想清楚",
        "thread_updates":[{"garbage":"x"},null]})).unwrap();
    assert!(output.thread_updates.is_empty());
    assert_eq!(output.conversation_intent, ConversationIntent::Silent);
}

#[test]
fn new_concerns_need_real_supplied_material_and_reflection_alone_is_insufficient() {
    let mut state = ThoughtThreads::default();
    let mut update = update("invented");
    update.evidence = "并不存在的兴趣".into();
    assert!(!state.apply(&[update.clone()], &grounding(&[], &[], &[]), 100));
    update.origin = ThoughtOrigin::Reflection;
    assert!(!state.apply(&[update], &grounding(&[], &[], &["并不存在的兴趣"]), 100));
}

#[test]
fn unchanged_material_cannot_fuel_an_endless_self_reflection_loop() {
    let mut state = state();
    for index in 1..=2 {
        let mut update = update("fresh_start");
        update.stance = format!("我对这件事的第{index}个新角度");
        assert!(state.threads.apply(&[update], &grounding(&[], &[], &[]), 100 + index));
    }
    let mut update = update("fresh_start");
    update.stance = "同样素材下的第三次自我循环".into();
    assert!(!state.threads.apply(&[update], &grounding(&[], &[], &[]), 103));
}

#[test]
fn capacity_update_budget_and_prompt_size_remain_bounded() {
    let mut state = ThoughtThreads::default();
    let many: Vec<_> = (0..20).map(|index| update(&format!("concern_{index}"))).collect();
    assert!(state.apply(&many, &grounding(&[], &[], &[]), 100));
    assert_eq!(state.snapshot(100, 100).len(), 2);
    for chunk in many[2..].chunks(2) { state.apply(chunk, &grounding(&[], &[], &[]), 101); }
    assert_eq!(state.snapshot(102, 100).len(), 6);
    assert!(state.offers(102).len() <= 3);
    assert_eq!(state.prompt_snapshot(102, 3).len(), 3);
    assert!(!state.prompt_snapshot(102, 3)[0].as_object().unwrap().contains_key("source_token"));
}

#[test]
fn duplicate_meanings_cannot_gain_another_slot_by_changing_the_key() {
    let mut state = state();
    let mut duplicate = update("another_key");
    duplicate.topic = "fresh_start的选择".into();
    assert!(!state.threads.apply(&[duplicate], &grounding(&[], &[], &[]), 101));
    assert_eq!(state.threads.snapshot(101, 6).len(), 1);
}

#[test]
fn shared_thoughts_can_stand_alone_without_asking_a_question() {
    let mut state = state();
    let mut change = update("fresh_start");
    change.next_step = ThoughtStep::Share;
    change.open_question.clear();
    assert!(state.threads.apply(&[change], &grounding(&[], &[], &[]), 101));
    let mut output = proposal(&state);
    output.conversation_intent = ConversationIntent::Share;
    output.conversation_evidence = state.threads.snapshot(102, 1)[0].stance.clone();
    let events = state.threads.offers(102);
    state.apply_synthesis(&output, &grounding(&[], &events, &[]), true, 102.0);
    assert!(state.fresh(102.0).is_some());
}

#[test]
fn inactive_expired_and_wrong_step_records_cannot_authorize_speech() {
    let mut state = state();
    assert!(!state.threads.permits(&state.threads.offers(101)[0].id, ConversationIntent::Share, 101));
    state.threads.prune(100 + 15 * 86400);
    assert!(state.threads.snapshot(100 + 15 * 86400, 6).is_empty());
    assert!(state.threads.offers(100 + 15 * 86400).is_empty());
}

#[test]
fn completed_concerns_make_room_without_immediately_recreating_the_same_old_interest() {
    let mut state = ThoughtThreads::default();
    for index in 0..6 { state.apply(&[update(&format!("concern_{index}"))], &grounding(&[], &[], &[]), 100); }
    let mut finished = update("concern_0");
    finished.status = ThreadStatus::Resolved;
    finished.next_step = ThoughtStep::Rest;
    assert!(state.apply(&[finished], &grounding(&[], &[], &[]), 101));
    assert!(state.apply(&[update("a_new_interest")], &grounding(&[], &[], &[]), 102));
    assert_eq!(state.snapshot(102, 6).len(), 6);
    assert!(!state.apply(&[update("concern_0")], &grounding(&[], &[], &[]), 103));
}

#[test]
fn an_unanswered_concern_can_be_parked_without_inventing_an_answer() {
    let mut state = state();
    deliver_question(&mut state);
    let mut park = update("fresh_start");
    park.status = ThreadStatus::Parked;
    park.next_step = ThoughtStep::Rest;
    assert!(state.threads.apply(&[park], &grounding(&[], &[], &[]), 106));
    assert!(state.threads.snapshot(106, 1)[0].awaiting_reply);
    assert!(state.threads.offers(107).is_empty());
}

fn general_question() -> ConversationOpportunities {
    let mut state = ConversationOpportunities::default();
    let mut concern = update("fresh_start");
    concern.question_scope = QuestionScope::General;
    concern.peer_fallback = true;
    assert!(state.threads.apply(&[concern], &grounding(&[], &[], &[]), 100));
    state
}

#[test]
fn a_character_can_choose_a_real_peer_question_while_the_user_is_away() {
    let mut state = ConversationOpportunities::default();
    let mut concern = update("fresh_start");
    concern.next_step = ThoughtStep::AskPeer;
    concern.question_scope = QuestionScope::General;
    state.threads.apply(&[concern], &grounding(&[], &[], &[]), 100);
    let events = state.threads.offers(101);
    let mut output = proposal(&state);
    output.conversation_intent = ConversationIntent::AskPeer;
    state.apply_synthesis(&output, &grounding(&[], &events, &[]), false, 101.0);
    assert!(state.fresh(101.0).unwrap().intent.is_peer());
    state.take(102.0).unwrap();
    assert!(state.threads.offers(103).is_empty(), "one concern cannot start a peer loop each tick");
}

#[test]
fn user_silence_only_offers_one_optional_general_peer_fallback_after_delivery() {
    let mut state = general_question();
    deliver_question(&mut state);
    assert!(!state.threads.activate_peer_fallbacks(104 + PEER_FALLBACK_AFTER_SECS - 1));
    assert!(state.threads.offers(283).is_empty());
    assert!(state.threads.activate_peer_fallbacks(284));
    assert!(!state.threads.activate_peer_fallbacks(285));
    let events = state.threads.offers(285);
    assert_eq!(events.len(),1);
    assert!(events[0].fact.contains("ask_peer"));
    assert!(state.fresh(285.0).is_none(), "eligibility is not an agent's decision to speak");
    let mut output = proposal(&state);
    output.conversation_intent = ConversationIntent::AskPeer;
    state.apply_synthesis(&output, &grounding(&[], &events, &[]), false, 285.0);
    let fallback = state.take(286.0).expect("the earlier user question must not deduplicate the peer audience");
    state.record_prepared(fallback, "你对这个机制有什么看法？", 286.0);
    state.record_delivered("你对这个机制有什么看法？", 287.0);
    assert!(state.threads.snapshot(288,1)[0].awaiting_reply, "contacting a peer does not fabricate an answer");
    assert!(state.threads.offers(1000).is_empty());
    let mut restored: ThoughtThreads = serde_json::from_str(&serde_json::to_string(&state.threads).unwrap()).unwrap();
    assert!(!restored.activate_peer_fallbacks(1000));
    assert!(restored.offers(1000).is_empty());
}

#[test]
fn private_preferences_permissions_and_legacy_questions_cannot_fall_back_to_a_peer() {
    let mut state = state();
    deliver_question(&mut state);
    assert!(!state.threads.activate_peer_fallbacks(10000));
    assert!(state.threads.offers(10000).is_empty());
    let mut private = update("private_choice");
    private.next_step = ThoughtStep::AskPeer;
    private.peer_fallback = true;
    assert_eq!(private.question_scope, QuestionScope::UserOnly);
    assert!(!state.threads.apply(&[private], &grounding(&[], &[], &[]), 101));
}

#[test]
fn a_draft_never_starts_the_unanswered_user_timer() {
    let mut state = general_question();
    let events = state.threads.offers(101);
    state.apply_synthesis(&proposal(&state), &grounding(&[], &events, &[]), true, 101.0);
    let draft = state.take(102.0).unwrap();
    state.record_prepared(draft, "尚未发出的草稿", 103.0);
    assert!(!state.threads.activate_peer_fallbacks(10000));
    assert!(!state.threads.snapshot(10000,1)[0].awaiting_reply);
}

#[test]
fn an_actual_correlated_peer_answer_can_change_understanding_and_become_attributed_knowledge() {
    use crate::peer_learning::{PeerLearning, LearningUpdate};
    let mut state = general_question();
    deliver_question(&mut state);
    let event_id = state.threads.snapshot(105,1)[0].event_id();
    let mut book = PeerLearning::default();
    let unrelated = book.receive("vivian","nana","unrelated","先观察再选择，比较容易知道自己在意什么。",Some("thought:other:1".into()),300).unwrap();
    let mut answer = update("fresh_start");
    answer.origin = ThoughtOrigin::SceneEvent;
    answer.question_scope = QuestionScope::General;
    answer.evidence = "先观察再选择".into();
    answer.stance = "先观察可能比急着选择更能发现自己的关注点".into();
    answer.open_question.clear(); answer.next_step = ThoughtStep::Rest; answer.status = ThreadStatus::Resolved;
    assert!(!state.threads.apply(&[answer.clone()], &grounding(&[], &[unrelated.event()], &[]), 301));
    let actual = book.receive("vivian","nana","actual-answer","先观察再选择，比较容易知道自己在意什么。",Some(event_id),301).unwrap();
    assert!(state.threads.apply(&[answer], &grounding(&[], &[actual.event()], &[]), 302));
    assert_eq!(state.threads.snapshot(302,1)[0].status, ThreadStatus::Resolved);
    assert!(!state.threads.snapshot(302,1)[0].awaiting_reply);
    let learned = book.learn(&[LearningUpdate {key:"observe_before_choice".into(),source_id:"actual-answer".into(),
        quote:"先观察再选择".into(),understanding:"可以先通过观察澄清自己关注的因素，而不是急着选一个答案".into()}],302);
    assert_eq!(learned.len(),1);
    let (_, metadata) = learned[0].memory_record("vivian");
    assert_eq!(metadata["speaker"],"nana"); assert_eq!(metadata["source_utterance_id"],"actual-answer");
    assert_eq!(metadata["verified"],false);
    assert!(crate::provenance::recall_label(&metadata).contains("待核实"));
    assert!(crate::provenance::visible_to(&metadata,"vivian"));
    assert!(!crate::provenance::visible_to(&metadata,"nana"), "the learner's interpretation is not silently copied into the other mind");
    assert!(state.threads.offers(303).is_empty(), "learning need not manufacture another question");
}

#[test]
fn the_users_real_answer_retires_an_already_offered_peer_fallback() {
    let mut state = general_question();
    deliver_question(&mut state);
    state.threads.activate_peer_fallbacks(284);
    let events = state.threads.offers(285);
    let mut output = proposal(&state); output.conversation_intent = ConversationIntent::AskPeer;
    state.apply_synthesis(&output, &grounding(&[], &events, &[]), true, 285.0);
    assert!(state.fresh(285.0).is_some());
    let mut answer = update("fresh_start");
    answer.origin = ThoughtOrigin::UserMessage; answer.evidence = "我喜欢创造的过程".into();
    answer.stance = "用户看重的是创造过程".into();answer.open_question.clear();
    answer.status=ThreadStatus::Resolved;answer.next_step=ThoughtStep::Rest;
    assert!(state.threads.apply(&[answer],&grounding(&[("我喜欢创造的过程",286)],&[],&[]),287));
    assert!(state.fresh(287.0).is_none());
}

#[test]
fn a_busy_peer_keeps_the_same_draft_but_new_user_context_cancels_it() {
    let mut state = general_question();
    deliver_question(&mut state);
    state.threads.activate_peer_fallbacks(284);
    let events=state.threads.offers(285);
    let mut output=proposal(&state);output.conversation_intent=ConversationIntent::AskPeer;
    state.apply_synthesis(&output,&grounding(&[],&events,&[]),false,285.0);
    let candidate=state.take(286.0).unwrap();
    state.record_prepared(candidate,"关于选择的方法，你怎么看？",286.0);
    assert!(state.peer_draft_is_current("关于选择的方法，你怎么看？",300));
    assert!(state.threads.offers(300).is_empty(),"waiting for delivery must not generate another draft");
    state.cancel_peer_drafts();
    assert!(!state.peer_draft_is_current("关于选择的方法，你怎么看？",301));
    assert!(state.threads.snapshot(301,1)[0].awaiting_reply);
}
