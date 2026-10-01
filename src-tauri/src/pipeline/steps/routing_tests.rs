use super::*;
use crate::emotion::embedding_classifier::EmbeddingEmotionClassifier;
use crate::memory::embedding::{HashingMemoryEmbedding, MemoryEmbeddingProvider};
use crate::pipeline::base::RunnableLambda;
use crate::pipeline::steps::prompt::{PreparedPromptPipeline, PromptBuildingStep};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Local fixture only. No configured embedding/LLM endpoints or credentials are read.
struct ControlledEmbedding {
    gate_next: AtomicBool,
    timed_out: AtomicBool,
    calls: AtomicUsize,
    delay_ms: AtomicUsize,
    started: Arc<tokio::sync::Notify>,
    release: parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
    remote: bool,
}

impl MemoryEmbeddingProvider for ControlledEmbedding {
    fn dimension(&self) -> usize { 256 }
    fn model_id(&self) -> &str { "local-routing-test" }
    fn is_remote(&self) -> bool { self.remote }
    fn embed(&self, text: &str) -> crate::error::VivianResult<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.gate_next.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            if self.release.lock().recv_timeout(Duration::from_secs(2)).is_err() {
                self.timed_out.store(true, Ordering::SeqCst);
            }
        }
        let delay = self.delay_ms.load(Ordering::Relaxed);
        if delay != 0 {
            self.started.notify_one();
            std::thread::sleep(Duration::from_millis(delay as u64));
        }
        HashingMemoryEmbedding::new(256).embed(text)
    }
}

fn controlled_provider(remote: bool) -> (Arc<ControlledEmbedding>, std::sync::mpsc::Sender<()>) {
    let (release, receiver) = std::sync::mpsc::channel();
    (Arc::new(ControlledEmbedding {
        gate_next: AtomicBool::new(false), timed_out: AtomicBool::new(false),
        calls: AtomicUsize::new(0), delay_ms: AtomicUsize::new(0),
        started: Arc::new(tokio::sync::Notify::new()), release: parking_lot::Mutex::new(receiver), remote,
    }), release)
}

fn warm_analyzer(provider: Arc<ControlledEmbedding>) -> Arc<FastSemanticAnalyzer> {
    let emotion = Arc::new(EmbeddingEmotionClassifier::new(provider.clone(), "zh".into()));
    emotion.preload().unwrap();
    let analyzer = Arc::new(FastSemanticAnalyzer::new(emotion, provider, "zh".into()));
    analyzer.preload().unwrap();
    analyzer
}

#[tokio::test(flavor = "current_thread")]
async fn sync_embedding_does_not_block_peer_and_assessments_survive_merge() {
    let (provider, release) = controlled_provider(false);
    let analyzer = warm_analyzer(provider.clone());
    provider.gate_next.store(true, Ordering::SeqCst);
    let started = provider.started.clone();
    let peer = RunnableLambda::new(|input| input).with_async(move |input| {
        let started = started.clone(); let release = release.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(3), started.notified()).await.unwrap();
            release.send(()).unwrap();
            input
        }
    });
    let parallel = ParallelStep::new(Box::new(peer), Box::new(FastSemanticStep::new(analyzer, "vivian".into())));
    let state = PipelineState { user_input: "分析异步任务和同步调用的机制724F2".into(), ..Default::default() };
    let result = PipelineState::from_json(parallel.ainvoke(state.to_json(), None).await.unwrap());
    assert!(!provider.timed_out.load(Ordering::SeqCst), "peer could not run while embed blocked");
    assert!(result.fast_perception.is_some());
    assert!(result.epistemic_assessment.is_some());
    assert!(result.schedule_assessment.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_preparation_overlaps_context_and_finalizes_with_current_results() {
    let (provider, release) = controlled_provider(true);
    provider.gate_next.store(true, Ordering::SeqCst);
    let started = provider.started.clone();
    let context = RunnableLambda::new(|input| input).with_async(move |input| {
        let started = started.clone(); let release = release.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(3), started.notified()).await.unwrap();
            release.send(()).unwrap();
            let mut state = PipelineState::from_json(input);
            state.memory_text = "unique_retrieved_evidence_724F2".into();
            let perception = crate::emotion::FastPerceptionResult {
                guidance: "unique_semantic_guidance_724F2".into(),
                query_embedding: Arc::new(vec![1.0; 256]),
                ..Default::default()
            };
            state.fast_perception = Some(perception);
            state.to_json()
        }
    });
    let injector = Arc::new(crate::persona::ToneInjector::with_embedding("vivian", provider.clone()));
    let prompt = PromptBuildingStep::new().with_tone_injector(injector);
    let pipeline = PreparedPromptPipeline::new(Box::new(context), prompt);
    let state = PipelineState { user_input: "parallel routing test 724F2".into(), ..Default::default() };
    let result = PipelineState::from_json(pipeline.ainvoke(state.to_json(), None).await.unwrap());
    assert!(!provider.timed_out.load(Ordering::SeqCst), "preparation blocked context progress");
    assert!(result.system_prompt.contains("unique_retrieved_evidence_724F2"));
    assert!(result.system_prompt.contains("unique_semantic_guidance_724F2"));
    let timings = result.metadata["timings"].as_array().unwrap();
    assert!(timings.iter().any(|timing| timing["stage"] == "prompt_preparation"));
    assert!(timings.iter().any(|timing| timing["stage"] == "prompt_building"));
}

#[tokio::test(flavor = "current_thread")]
async fn non_user_messages_skip_embedding_and_parallel_merge_preserves_existing_context() {
    let (provider, _) = controlled_provider(false);
    let analyzer = warm_analyzer(provider.clone());
    provider.calls.store(0, Ordering::Relaxed);
    let step = FastSemanticStep::new(analyzer, "nana".into());
    let state = PipelineState { user_input: "不要把角色来话当成用户信号".into(), current_channel: "cross_character".into(), ..Default::default() };
    let result = PipelineState::from_json(step.ainvoke(state.to_json(), None).await.unwrap());
    assert!(result.fast_perception.is_none());
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

struct BlockingSemantic(Arc<FastSemanticAnalyzer>);

#[tokio::test]
async fn retrieved_examples_are_dynamic_and_absent_without_relevant_scene() {
    let injector = Arc::new(crate::persona::ToneInjector::new("nana"));
    let prompt = PromptBuildingStep::new().with_tone_injector(injector);
    let messages = vec![serde_json::from_value(serde_json::json!({
        "role": "assistant", "content": "英语演讲准备讲多久？"
    })).unwrap()];
    let state = PipelineState { user_input: "8分钟左右".into(), messages, ..Default::default() };
    let result = PipelineState::from_json(prompt.ainvoke(state.to_json(), None).await.unwrap());
    let position = result.system_prompt.find("[RETRIEVED EXAMPLES").unwrap();
    let boundary = result.system_prompt.find(crate::pipeline::prompt_modules::SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
    assert!(position > boundary);
    assert!(result.system_prompt.contains("presentation-duration"));
    assert!(result.metadata["retrieved_example_chars"].as_u64().unwrap() <= 1800);
    let state = PipelineState { user_input: "怎么安装软件".into(), ..Default::default() };
    let result = PipelineState::from_json(prompt.ainvoke(state.to_json(), None).await.unwrap());
    assert!(!result.system_prompt.contains("[RETRIEVED EXAMPLES"));
}

#[test]
fn tone_matching_reuses_the_query_vector_without_embedding_it_again() {
    let (provider, _) = controlled_provider(true);
    let injector = crate::persona::ToneInjector::with_embedding("vivian", provider.clone());
    injector.preload();
    provider.calls.store(0, Ordering::Relaxed);
    let text = "xxq724F2";
    let embedding = HashingMemoryEmbedding::new(256).embed(text).unwrap();
    injector.build_tone_injection_with_embedding(text, "zh", &[], Some(&embedding));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
    injector.build_tone_injection_growing(text, "zh", &[]);
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[async_trait]
impl Runnable for BlockingSemantic {
    async fn ainvoke(&self, input: Value, _: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);
        state.fast_perception = Some(self.0.analyze(&state.user_input).unwrap());
        Ok(state.to_json())
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "controlled scheduling benchmark; run explicitly with --ignored --nocapture"]
async fn benchmark_blocking_vs_offloaded_semantic_work() {
    let (provider, _) = controlled_provider(false);
    let analyzer = warm_analyzer(provider.clone());
    provider.delay_ms.store(80, Ordering::Relaxed);
    let mut old = Vec::new(); let mut new = Vec::new();
    for round in 0..7 {
        for offloaded in if round % 2 == 0 { [false, true] } else { [true, false] } {
            let started = provider.started.clone();
            let peer = RunnableLambda::new(|input| input).with_async(move |input| {
                let started = started.clone();
                async move {
                    started.notified().await;
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    input
                }
            });
            let semantic: Box<dyn Runnable> = if offloaded {
                Box::new(FastSemanticStep::new(analyzer.clone(), "vivian".into()))
            } else { Box::new(BlockingSemantic(analyzer.clone())) };
            let pipeline = ParallelStep::new(Box::new(peer), semantic);
            let state = PipelineState { user_input: format!("独立调度基准消息724F2_{round}_{offloaded}"), ..Default::default() };
            let start = Instant::now();
            pipeline.ainvoke(state.to_json(), None).await.unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if offloaded { new.push(elapsed); } else { old.push(elapsed); }
        }
    }
    old.sort_by(f64::total_cmp); new.sort_by(f64::total_cmp);
    eprintln!("controlled scheduling: blocking median={:.2}ms; offloaded median={:.2}ms; 80ms local embed + 80ms async peer; no network", old[3], new[3]);
}
