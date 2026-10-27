use crate::{
    diary::{
        cycle::{CycleWork, DiaryCycle, DreamContext},
        DiaryEntry,
    },
    error::{VivianError, VivianResult},
};
use chrono::NaiveDate;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
struct Work {
    log: parking_lot::Mutex<Vec<String>>,
    saved: parking_lot::Mutex<Option<DiaryEntry>>,
    fail_dream: AtomicBool,
    fail_diary: AtomicBool,
    fail_after_save: AtomicBool,
    block: AtomicBool,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
}
fn entry(day: NaiveDate, dream: &DreamContext) -> DiaryEntry {
    serde_json::from_value(serde_json::json!({"id":"saved-diary","date":day.to_string(),"start_time":1,"end_time":2,
        "content":"原始互动后的日记","key_events":[],"mood_average":{},"word_count":9,"interaction_count":3,
        "trigger_type":"manual","trigger_score":30,"mood_tag":"neutral","created_at":3,"version":2,"dream":dream})).unwrap()
}
#[async_trait::async_trait]
impl CycleWork for Work {
    fn existing(&self, day: NaiveDate) -> VivianResult<Option<DiaryEntry>> {
        Ok(self
            .saved
            .lock()
            .clone()
            .filter(|e| e.date == day.to_string()))
    }
    async fn dream(&self, day: NaiveDate) -> VivianResult<DreamContext> {
        self.log.lock().push(format!("dream:{day}"));
        self.entered.notify_one();
        if self.block.load(Ordering::Relaxed) {
            self.resume.notified().await;
        }
        if self.fail_dream.load(Ordering::Relaxed) {
            return Err(VivianError::Memory("dream failed".into()));
        }
        Ok(DreamContext {
            report: Default::default(),
            material: vec![serde_json::json!({"kind":"interpretation","text":"有来源的联想"})],
        })
    }
    async fn diary(
        &self,
        day: NaiveDate,
        _: &str,
        dream: &DreamContext,
    ) -> VivianResult<DiaryEntry> {
        self.log.lock().push(format!("diary:{day}"));
        assert_eq!(dream.material[0]["text"], "有来源的联想");
        if self.fail_diary.load(Ordering::Relaxed) {
            return Err(VivianError::Memory("diary failed".into()));
        }
        let entry = entry(day, dream);
        *self.saved.lock() = Some(entry.clone());
        if self.fail_after_save.swap(false, Ordering::Relaxed) {
            return Err(VivianError::Memory("interrupted after save".into()));
        }
        Ok(entry)
    }
}

#[tokio::test]
async fn dream_precedes_diary_and_one_date_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cycle = DiaryCycle::new(dir.path().into());
    let work = Work::default();
    let result = cycle.run(date(), "manual", &work).await.unwrap();
    assert_eq!(result.date, date().to_string());
    assert!(result.dream.is_some());
    assert_eq!(*work.log.lock(), ["dream:2026-10-26", "diary:2026-10-26"]);
    cycle.run(date(), "tool", &work).await.unwrap();
    assert_eq!(work.log.lock().len(), 2);
    assert!(cycle.pending_date(i64::MAX).unwrap().is_none());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "completed cycles do not accumulate scan files"
    );
}

#[tokio::test]
async fn dream_failure_never_writes_diary_and_can_resume() {
    let dir = tempfile::tempdir().unwrap();
    let cycle = DiaryCycle::new(dir.path().into());
    let work = Work::default();
    work.fail_dream.store(true, Ordering::Relaxed);
    assert!(cycle.run(date(), "manual", &work).await.is_err());
    assert_eq!(work.log.lock().len(), 1);
    assert!(work.saved.lock().is_none());
    work.fail_dream.store(false, Ordering::Relaxed);
    cycle.run(date(), "manual", &work).await.unwrap();
    assert_eq!(work.log.lock().len(), 3);
}

#[tokio::test]
async fn diary_failure_survives_restart_without_redreaming() {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::default();
    work.fail_diary.store(true, Ordering::Relaxed);
    assert!(DiaryCycle::new(dir.path().into())
        .run(date(), "manual", &work)
        .await
        .is_err());
    work.fail_diary.store(false, Ordering::Relaxed);
    DiaryCycle::new(dir.path().into())
        .run(date(), "manual", &work)
        .await
        .unwrap();
    assert_eq!(
        *work.log.lock(),
        ["dream:2026-10-26", "diary:2026-10-26", "diary:2026-10-26"]
    );
}

#[tokio::test]
async fn saved_diary_recovers_a_crash_before_checkpoint_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::default();
    work.fail_after_save.store(true, Ordering::Relaxed);
    let cycle = DiaryCycle::new(dir.path().into());
    assert!(cycle.run(date(), "manual", &work).await.is_err());
    cycle.run(date(), "auto", &work).await.unwrap();
    assert_eq!(work.log.lock().len(), 2);
    assert!(cycle.pending_date(i64::MAX).unwrap().is_none());
}

#[tokio::test]
async fn automatic_retry_respects_backoff_manual_retry_can_resume() {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::default();
    work.fail_diary.store(true, Ordering::Relaxed);
    let cycle = DiaryCycle::new(dir.path().into());
    assert!(cycle.run(date(), "auto", &work).await.is_err());
    assert!(!cycle.ready(date(), chrono::Utc::now().timestamp()).unwrap());
    assert!(cycle.ready(date(), i64::MAX).unwrap());
    assert!(cycle.run(date(), "auto", &work).await.is_err());
    assert_eq!(work.log.lock().len(), 2);
    work.fail_diary.store(false, Ordering::Relaxed);
    cycle.run(date(), "manual", &work).await.unwrap();
    assert_eq!(work.log.lock().len(), 3);
}

#[tokio::test]
async fn abort_releases_the_character_gate_and_other_characters_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let work = Arc::new(Work::default());
    work.block.store(true, Ordering::Relaxed);
    let spawned_work = work.clone();
    let task = tokio::spawn(async move {
        DiaryCycle::new(path)
            .run(date(), "manual", &*spawned_work)
            .await
    });
    work.entered.notified().await;
    assert!(DiaryCycle::new(dir.path().into())
        .run(date(), "manual", &*work)
        .await
        .is_err());
    let other = tempfile::tempdir().unwrap();
    DiaryCycle::new(other.path().into())
        .run(date(), "manual", &Work::default())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    work.block.store(false, Ordering::Relaxed);
    DiaryCycle::new(dir.path().into())
        .run(date(), "manual", &*work)
        .await
        .unwrap();
    assert_eq!(work.log.lock().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_dream_timeout_retains_progress_and_releases_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::default();
    work.block.store(true, Ordering::Relaxed);
    let cycle = DiaryCycle::new(dir.path().into());
    let error = cycle.run(date(), "manual", &work).await.unwrap_err();
    assert!(error.to_string().contains("超时"));
    assert!(work.saved.lock().is_none());
    assert!(cycle.pending_date(i64::MAX).unwrap().is_some());
    work.block.store(false, Ordering::Relaxed);
    cycle.run(date(), "manual", &work).await.unwrap();
}

#[tokio::test]
async fn unreadable_progress_is_preserved_without_model_calls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{}.json", date()));
    std::fs::write(&path, "invalid json").unwrap();
    let work = Work::default();
    assert!(DiaryCycle::new(dir.path().into())
        .run(date(), "manual", &work)
        .await
        .is_err());
    assert!(work.log.lock().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid json");
}

#[tokio::test]
async fn pending_cycles_resume_their_original_date_after_midnight() {
    let dir = tempfile::tempdir().unwrap();
    let cycle = DiaryCycle::new(dir.path().into());
    let work = Work::default();
    work.fail_diary.store(true, Ordering::Relaxed);
    assert!(cycle.run(date(), "manual", &work).await.is_err());
    assert!(cycle
        .run(date().succ_opt().unwrap(), "manual", &work)
        .await
        .is_err());
    assert_eq!(cycle.pending_date(i64::MAX).unwrap(), Some(date()));
    work.fail_diary.store(false, Ordering::Relaxed);
    let entry = cycle.run(date(), "manual", &work).await.unwrap();
    assert_eq!(entry.date, date().to_string());
    assert_eq!(cycle.pending_date(i64::MAX).unwrap(), date().succ_opt());
}

#[test]
fn requesting_catch_up_only_creates_a_candidate_without_writing_a_diary() {
    let dir = tempfile::tempdir().unwrap();
    let cycle = DiaryCycle::new(dir.path().into());
    cycle.request(date()).unwrap();
    cycle.request(date()).unwrap();
    assert_eq!(
        cycle.pending_date(chrono::Utc::now().timestamp()).unwrap(),
        Some(date())
    );
    let checkpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(format!("{}.json", date()))).unwrap(),
    )
    .unwrap();
    assert!(checkpoint["dream"].is_null());
    assert_eq!(checkpoint["phase"], "dreaming");
}

#[test]
fn corrupt_old_progress_does_not_starve_a_healthy_newer_request() {
    let dir = tempfile::tempdir().unwrap();
    let cycle = DiaryCycle::new(dir.path().into());
    std::fs::write(dir.path().join(format!("{}.json", date())), "bad json").unwrap();
    cycle.request(date().succ_opt().unwrap()).unwrap();
    assert_eq!(cycle.pending_date(i64::MAX).unwrap(), date().succ_opt());
}

fn gate_fixture(material: usize) -> (tempfile::TempDir, crate::brain::Brain) {
    let dir = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    crate::diary::ROOTS
        .lock()
        .insert(id.clone(), dir.path().into());
    (
        dir,
        crate::brain::Brain {
            char_id: id,
            memory: crate::brain::Material {
                count: material.into(),
                scans: 0.into(),
            },
        },
    )
}
fn local_time(hour: u32) -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone;
    chrono::Local
        .from_local_datetime(&date().and_hms_opt(hour, 0, 0).unwrap())
        .earliest()
        .unwrap()
}

#[test]
fn program_gate_precedes_material_scans_and_semantic_admission() {
    let (_dir, brain) = gate_fixture(100);
    assert!(crate::diary::dream::candidate(&brain, local_time(19))
        .unwrap()
        .is_none());
    assert_eq!(brain.memory.scans.load(Ordering::Relaxed), 0);
    brain.memory.count.store(19, Ordering::Relaxed);
    assert!(crate::diary::dream::candidate(&brain, local_time(20))
        .unwrap()
        .is_none());
    brain.memory.count.store(20, Ordering::Relaxed);
    let candidate = crate::diary::dream::candidate(&brain, local_time(20))
        .unwrap()
        .unwrap();
    assert_eq!(candidate.material_count_at_least, 20);
    assert_eq!(candidate.material_threshold, 20);
    assert_eq!(candidate.character_id, brain.char_id);
    assert!(crate::diary::dream::candidate(&brain, local_time(0))
        .unwrap()
        .is_none());
}

#[test]
fn pending_resume_obeys_the_same_material_and_time_gates() {
    let (dir, brain) = gate_fixture(20);
    let cycle = DiaryCycle::new(dir.path().join("cycles"));
    let previous = date().pred_opt().unwrap();
    cycle.request(previous).unwrap();
    assert!(crate::diary::dream::candidate(&brain, local_time(19))
        .unwrap()
        .is_none());
    brain.memory.count.store(19, Ordering::Relaxed);
    assert!(crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .is_none());
    brain.memory.count.store(20, Ordering::Relaxed);
    let candidate = crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .unwrap();
    assert!(candidate.resume);
    assert_eq!(candidate.date, previous.to_string());
}

#[test]
fn disabled_existing_and_active_cycles_never_enter_semantic_planning() {
    let (dir, brain) = gate_fixture(100);
    let mut config = crate::diary::DiaryConfig::default();
    config.enable_auto_diary = false;
    crate::diary::CONFIGS
        .lock()
        .insert(brain.char_id.clone(), config.clone());
    assert!(crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .is_none());
    assert_eq!(brain.memory.scans.load(Ordering::Relaxed), 0);
    config.enable_auto_diary = true;
    config.min_interaction_threshold = 101;
    crate::diary::CONFIGS
        .lock()
        .insert(brain.char_id.clone(), config.clone());
    assert!(crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .is_none());
    config.min_interaction_threshold = 20;
    crate::diary::CONFIGS
        .lock()
        .insert(brain.char_id.clone(), config);
    let permit = crate::presence::activity_budget::try_acquire(&brain.char_id).unwrap();
    assert!(crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .is_none());
    drop(permit);
    std::fs::write(
        dir.path().join("test-entries.json"),
        serde_json::to_string(&vec![entry(date(), &DreamContext::default())]).unwrap(),
    )
    .unwrap();
    assert!(crate::diary::dream::candidate(&brain, local_time(21))
        .unwrap()
        .is_none());
}

#[test]
fn judgment_window_boundaries_and_empty_threshold_are_explicit() {
    use crate::diary::cycle::may_offer_to_planner;
    for hour in [0, 1, 19, 24, 25] {
        assert!(!may_offer_to_planner(hour, 100, 20));
    }
    assert!(may_offer_to_planner(20, 20, 20));
    assert!(may_offer_to_planner(23, 21, 20));
    assert!(!may_offer_to_planner(20, 19, 20));
    assert!(!may_offer_to_planner(21, 0, 0));
}

#[test]
fn local_day_bounds_and_old_diary_schema_remain_compatible() {
    use chrono::{Datelike, TimeZone};
    let (start, end) = crate::diary::cycle::day_bounds(date()).unwrap();
    assert!(end > start);
    assert_eq!(
        chrono::Local.timestamp_opt(start, 0).unwrap().date_naive(),
        date()
    );
    assert_eq!(chrono::Local.timestamp_opt(end, 0).unwrap().day(), 27);
    let mut value = serde_json::to_value(entry(date(), &DreamContext::default())).unwrap();
    value.as_object_mut().unwrap().remove("dream");
    assert!(serde_json::from_value::<DiaryEntry>(value)
        .unwrap()
        .dream
        .is_none());
}

#[test]
fn associative_beliefs_must_reference_the_provided_memories() {
    let ids = ["real".to_string()].into_iter().collect();
    assert!(crate::belief_sources::valid_sources(&["real".into()], &ids));
    assert!(!crate::belief_sources::valid_sources(&[], &ids));
    assert!(!crate::belief_sources::valid_sources(
        &["real".into(), "invented".into()],
        &ids
    ));
}

#[test]
fn diary_prompt_labels_associations_as_interpretations_in_all_languages() {
    use crate::diary::intelligent_generator::{test_prompt, DailyContext};
    let mut ctx = DailyContext::default();
    ctx.char_id = "vivian".into();
    ctx.char_name = "Vivian".into();
    ctx.dream = Some(DreamContext {
        report: Default::default(),
        material: vec![
            serde_json::json!({"kind":"interpretation","source_memory_ids":["real"],"text":"可能想要陪伴"}),
        ],
    });
    for (lang, warning) in [
        ("zh", "不能把联想写成用户事实"),
        ("en", "never turn an association into a user fact"),
        ("ja", "連想をユーザーの事実に変え"),
    ] {
        let (system, user) = test_prompt(&ctx, lang);
        assert!(system.contains(warning));
        assert!(user.contains("interpretation"));
        assert!(user.contains("real"));
    }
}

#[test]
fn facts_summaries_and_private_associations_never_count_as_new_dialogue() {
    use crate::memory::{
        kinds,
        types::{Granularity, MemoryItem},
    };
    let records: Vec<_> = [
        "short_term",
        "user",
        "session_summary",
        "insight",
        "knowledge",
    ]
    .iter()
    .map(|kind| {
        let mut item = MemoryItem::new("同一条原文".into(), Granularity::Summary, 0.7);
        item.memory_type = (*kind).into();
        item.metadata =
            serde_json::json!({"channel":"direct","speaker":"user","listener":"vivian"});
        kinds::initialize_record(&mut item);
        item
    })
    .collect();
    let result = crate::diary::intelligent_generator::test_original_dialogue(records);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[0].content, "同一条原文");
}

#[test]
fn catch_up_continuity_never_uses_a_later_or_same_day_diary() {
    let day = date();
    let dream = DreamContext::default();
    let entries = vec![
        entry(day.succ_opt().unwrap(), &dream),
        entry(day, &dream),
        entry(day.pred_opt().unwrap(), &dream),
    ];
    let previous = crate::diary::intelligent_generator::preceding_diaries(entries, day);
    assert_eq!(previous.len(), 1);
    assert_eq!(previous[0].date, day.pred_opt().unwrap().to_string());
}
