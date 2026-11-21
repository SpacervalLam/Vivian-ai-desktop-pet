//! Dependency adapters for the real context_gather orchestrator. No desktop/model services.
//! Real prompt rendering and memory ranking are not replaced in production or claimed tested here.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use serde_json::{json, Value};
use tokio::sync::Notify;

#[derive(Debug)]
pub enum VivianError { Other(String) }
impl std::fmt::Display for VivianError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Other(message) => f.write_str(message) }
    }
}
pub type VivianResult<T> = Result<T, VivianError>;

pub mod base {
    use super::*;
    #[derive(Clone)]
    pub struct RunnableConfig;
    #[async_trait::async_trait]
    pub trait Runnable: Send + Sync {
        async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value>;
    }
}
pub mod state {
    use super::*;
    #[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    pub struct PipelineState {
        pub user_input: String,
        pub resolved_user_input: String,
        pub memory_text: String,
        pub web_context: String,
        pub fast_perception: Option<String>,
        pub epistemic_assessment: Option<String>,
        pub schedule_assessment: Option<String>,
        pub metadata: Value,
        pub prompt: String,
    }
    impl PipelineState {
        pub fn from_json(value: Value) -> Self { serde_json::from_value(value).unwrap() }
        pub fn to_json(&self) -> Value { serde_json::to_value(self).unwrap() }
    }
}
use base::{Runnable, RunnableConfig};
use state::PipelineState;

#[derive(Default)]
pub struct Gates {
    base_started: Notify,
    candidates_started: Notify,
    memory_finished: AtomicBool,
    web_finished: AtomicBool,
}
pub struct MemoryRetrievalStep(pub Arc<Gates>);
impl MemoryRetrievalStep {
    pub async fn prefetch(&self, state: &mut PipelineState) -> Option<String> {
        assert_eq!(state.resolved_user_input, "rewritten current query");
        self.0.base_started.notify_one();
        // If tool/example gathering waits for completed recall, this cannot finish.
        self.0.candidates_started.notified().await;
        Some("actual base recall".into())
    }
    pub async fn finish_recall(&self, mut state: PipelineState, base: Option<String>) -> VivianResult<Value> {
        assert_eq!(base.as_deref(), Some("actual base recall"));
        assert_eq!(state.fast_perception.as_deref(), Some("current semantic labels"));
        assert_eq!(state.epistemic_assessment.as_deref(), Some("current evidence assessment"));
        assert_eq!(state.resolved_user_input, "rewritten current query");
        state.memory_text = "fresh memory evidence".into();
        state.metadata["memory_strong_hit"] = json!(true);
        state.metadata["memory_retrieval_timing"] = json!({"elapsed_ms":1});
        self.0.memory_finished.store(true, Ordering::SeqCst);
        Ok(state.to_json())
    }
}

#[derive(Clone)]
pub struct PromptBuildingStep(pub Arc<Gates>);
impl PromptBuildingStep {
    pub fn prepare_context(&self) -> &'static str { "stable character card" }
    pub async fn prepare_query_context(&self, state: &PipelineState) -> VivianResult<&'static str> {
        assert_eq!(state.fast_perception.as_deref(), Some("current semantic labels"));
        self.0.candidates_started.notify_one();
        Ok("prepared tools and examples")
    }
    pub async fn ainvoke_prepared(&self, input: Value, _: Option<RunnableConfig>,
        prepared: Option<&str>, query: Option<&str>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);
        assert!(self.0.memory_finished.load(Ordering::SeqCst));
        assert!(self.0.web_finished.load(Ordering::SeqCst));
        assert_eq!(state.metadata["memory_strong_hit"], true);
        assert_eq!(state.memory_text, "fresh memory evidence");
        assert_eq!(state.web_context, "fresh web evidence");
        assert_eq!(state.schedule_assessment.as_deref(), Some("current schedule assessment"));
        assert_eq!(state.resolved_user_input, "rewritten current query");
        assert_eq!(prepared, Some("stable character card"));
        assert_eq!(query, Some("prepared tools and examples"));
        state.prompt = "assembled only after all request-local inputs completed".into();
        Ok(state.to_json())
    }
}

enum InputStage { Rewrite, Semantic(Arc<Gates>), Web(Arc<Gates>) }
#[async_trait::async_trait]
impl Runnable for InputStage {
    async fn ainvoke(&self, input: Value, _: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);
        let stage = match self {
            Self::Rewrite => { state.resolved_user_input = "rewritten current query".into(); "query_rewrite" }
            Self::Semantic(gates) => {
                gates.base_started.notified().await;
                state.fast_perception = Some("current semantic labels".into());
                state.epistemic_assessment = Some("current evidence assessment".into());
                state.schedule_assessment = Some("current schedule assessment".into());
                "fast_semantic"
            }
            Self::Web(gates) => {
                assert_eq!(state.epistemic_assessment.as_deref(), Some("current evidence assessment"));
                // This branch still has the initial memory snapshot, which must not win at merge.
                assert_eq!(state.memory_text, "old memory snapshot");
                state.web_context = "fresh web evidence".into();
                gates.web_finished.store(true, Ordering::SeqCst);
                "web_context"
            }
        };
        state.metadata["timings"].as_array_mut().unwrap().push(json!({"stage":stage,"elapsed_ms":1,"success":true}));
        Ok(state.to_json())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_orchestrator_overlaps_gather_and_preserves_owned_fields_until_assembly() {
    let gates = Arc::new(Gates::default());
    let gather = crate::context_gather::CompanionContextGather::new(
        Box::new(InputStage::Rewrite), Some(Box::new(InputStage::Semantic(gates.clone()))),
        MemoryRetrievalStep(gates.clone()), Box::new(InputStage::Web(gates.clone())),
        PromptBuildingStep(gates));
    let state = PipelineState { user_input:"ask Nana".into(), memory_text:"old memory snapshot".into(),
        metadata:json!({"timings":[{"stage":"pre_processing","elapsed_ms":1,"success":true}]}),
        ..Default::default() };
    let output = tokio::time::timeout(std::time::Duration::from_secs(1), gather.ainvoke(state.to_json(), None))
        .await.expect("independent context branches serialized or deadlocked").unwrap();
    let state = PipelineState::from_json(output);
    assert!(!state.prompt.is_empty());
    let timings = state.metadata["timings"].as_array().unwrap();
    for expected in ["pre_processing", "query_rewrite", "fast_semantic", "web_context", "memory_retrieval", "context_gather", "prompt_building"] {
        assert_eq!(timings.iter().filter(|t| t["stage"] == expected).count(), 1, "lost or duplicated {expected}");
    }
}
