//! Gather prompt inputs by dependency, then assemble once before generation.
use super::{memory::MemoryRetrievalStep, prompt::PromptBuildingStep};
use crate::error::{VivianError, VivianResult};
use crate::pipeline::{
    base::{Runnable, RunnableConfig},
    state::PipelineState,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{time::Instant, sync::Arc};
use futures::FutureExt;
use super::generation::AIResponseGenerationRunnable;
type TurnResolver = (Arc<crate::providers::ModelRouter>, String, Arc<parking_lot::Mutex<crate::pipeline::task_continuity::TaskTracker>>);

pub struct CompanionContextGather {
    rewrite: Box<dyn Runnable>,
    semantic: Option<Box<dyn Runnable>>,
    memory: MemoryRetrievalStep,
    web: Box<dyn Runnable>,
    prompt: PromptBuildingStep,
    resolver: Option<TurnResolver>,
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Step { wait: Option<Arc<tokio::sync::Notify>>, done: Option<Arc<tokio::sync::Notify>> }
    #[async_trait]
    impl Runnable for Step {
        async fn ainvoke(&self, input: Value, _: Option<RunnableConfig>) -> VivianResult<Value> {
            if let Some(wait) = &self.wait { wait.notified().await; }
            if let Some(done) = &self.done { done.notify_one(); }
            Ok(input)
        }
    }
    fn pass() -> Box<dyn Runnable> { Box::new(Step { wait: None, done: None }) }

    #[tokio::test]
    async fn intent_and_semantics_overlap_and_resolved_task_reaches_final_prompt() {
        use axum::{routing::post, Json, Router};
        let model_started = Arc::new(tokio::sync::Notify::new());
        let semantic_done = Arc::new(tokio::sync::Notify::new());
        let notice = model_started.clone(); let dependency = semantic_done.clone();
        let app = Router::new().route("/v1/chat/completions", post(move || {
            let notice = notice.clone(); let dependency = dependency.clone();
            async move {
                notice.notify_one();
                dependency.notified().await;
                Json(json!({"choices":[{"message":{"role":"assistant","content":
                    r#"{"action":true,"continues_task":false,"needs_clarification":false,"task":"制作指定文档"}"#},"finish_reason":"stop"}]}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = crate::config::AppConfig::default();
        config.enable_routing_matrix = false; config.ai.provider = "chat_completions".into();
        config.ai.endpoint = Some(format!("http://{}/v1", listener.local_addr().unwrap()));
        config.ai.model = uuid::Uuid::new_v4().to_string(); config.ai.api_key = Some("local-test-key".into());
        config.network.proxy_mode = "direct".into();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let memory = Arc::new(crate::memory::MemoryManager::new(&config, &format!("gather-test-{}", uuid::Uuid::new_v4())).unwrap());
        let gather = CompanionContextGather::new(pass(), Some(Box::new(Step { wait:Some(model_started), done:Some(semantic_done) })),
            MemoryRetrievalStep::new(memory), pass(), PromptBuildingStep::new())
            .with_turn_resolution(Arc::new(crate::providers::ModelRouter::new(&config).unwrap()), "test".into(), Arc::new(Default::default()));
        let mut state = PipelineState { user_input:"帮我制作指定文档".into(), ..Default::default() };
        state.metadata["resolve_contextual_intent"] = json!(true);
        state.metadata["skip_memory_retrieval"] = json!(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(3), gather.ainvoke(state.to_json(), None)).await.unwrap().unwrap();
        let output = PipelineState::from_json(output);
        assert_eq!(output.metadata["contextual_intent"]["task"], "制作指定文档");
        assert_eq!(output.metadata["task_turn_revision"], 1);
        assert!(AIResponseGenerationRunnable::requests_execution(&output));
        server.abort();
    }
}

impl CompanionContextGather {
    pub fn new(
        rewrite: Box<dyn Runnable>,
        semantic: Option<Box<dyn Runnable>>,
        memory: MemoryRetrievalStep,
        web: Box<dyn Runnable>,
        prompt: PromptBuildingStep,
    ) -> Self {
        Self {
            rewrite,
            semantic,
            memory,
            web,
            prompt,
            resolver: None,
        }
    }

    pub(crate) fn with_turn_resolution(mut self, router: Arc<crate::providers::ModelRouter>, character: String,
        tracker: Arc<parking_lot::Mutex<crate::pipeline::task_continuity::TaskTracker>>) -> Self {
        self.resolver = Some((router, character, tracker)); self
    }

}

fn timings_len(state: &PipelineState) -> usize {
    state.metadata["timings"].as_array().map_or(0, Vec::len)
}
fn append_timings(state: &mut PipelineState, source: &PipelineState, skip: usize) {
    let Some(timings) = source.metadata["timings"].as_array() else {
        return;
    };
    if !state.metadata["timings"].is_array() {
        state.metadata["timings"] = json!([]);
    }
    state.metadata["timings"]
        .as_array_mut()
        .unwrap()
        .extend(timings.iter().skip(skip).cloned());
}

#[async_trait]
impl Runnable for CompanionContextGather {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let started = Instant::now();
        let initial = PipelineState::from_json(input.clone());
        let existing_timings = timings_len(&initial);
        // Intent interpretation depends on history, not optional recall or prompt preparation.
        let mut turn_state = initial.clone();
        if let Some(original) = initial.metadata["api_turn_text"].as_str() { turn_state.user_input = original.into(); }
        let resolution = async {
            let mut metadata = serde_json::Map::new();
            if turn_state.metadata["resolve_contextual_intent"] != true { return Ok::<_, String>(metadata); }
            let Some((router, character, tracker)) = &self.resolver else { return Ok(metadata); };
            let start = Instant::now();
            let tracked = tracker.lock().snapshot(crate::memory::types::current_timestamp());
            let resolved = AIResponseGenerationRunnable::resolve_contextual_intent(router, &turn_state, character, tracked)
                .await.map_err(|error| error.to_string())?;
            crate::voice_diagnostics::mark_chat("intent_resolved");
            metadata.insert("contextual_intent".into(), json!(resolved));
            let mut interpreted = turn_state.clone();
            interpreted.metadata["contextual_intent"] = json!(resolved);
            let request = AIResponseGenerationRunnable::requests_execution(&interpreted);
            let (revision, continuation) = tracker.lock().begin_resolved(&turn_state.user_input, request,
                !turn_state.messages.iter().any(|message| matches!(message.role.as_str(), "user" | "assistant")),
                crate::memory::types::current_timestamp(), Some(&resolved));
            metadata.insert("task_turn_revision".into(), json!(revision));
            metadata.insert("contextual_intent_ms".into(), json!(start.elapsed().as_millis() as u64));
            if let Some(context) = continuation { metadata.insert("task_continuation".into(), json!(context)); }
            Ok(metadata)
        }.boxed().shared();
        let query_resolution = resolution.clone();
        let semantic = async {
            let result = match self.semantic.as_ref() {
                Some(semantic) => semantic.ainvoke(input.clone(), config.clone()).await,
                None => Ok(input.clone()),
            };
            // The shared result is cloned, not the embedding vectors (which remain Arc-backed).
            result
                .map(PipelineState::from_json)
                .map_err(|error| error.to_string())
        };
        let base_recall = async {
            let rewritten = self.rewrite.ainvoke(input.clone(), config.clone()).await?;
            let mut state = PipelineState::from_json(rewritten);
            let base = self.memory.prefetch(&mut state).await;
            Ok::<_, VivianError>((state, base))
        };
        let query_config = config.clone();
        let gathered = crate::pipeline::gather::gather(
            semantic,
            base_recall,
            |semantic| async move {
                let mut state = semantic.map_err(VivianError::Other)?;
                let metadata = query_resolution.await.map_err(VivianError::Other)?;
                state.metadata.as_object_mut().unwrap().extend(metadata);
                let semantic_timings = timings_len(&state);
                let (web, query) = tokio::join!(
                    self.web.ainvoke(state.to_json(), query_config),
                    self.prompt.prepare_query_context(&state),
                );
                Ok::<_, VivianError>((PipelineState::from_json(web?), query?, semantic_timings))
            },
            |semantic, base| async move {
                let semantic = semantic.map_err(VivianError::Other)?;
                let (mut state, base) = base?;
                // Copy only semantic-owned fields; never overwrite rewrite or recall outputs
                // with an older cloned PipelineState.
                state.fast_perception = semantic.fast_perception.clone();
                state.epistemic_assessment = semantic.epistemic_assessment.clone();
                state.schedule_assessment = semantic.schedule_assessment.clone();
                append_timings(&mut state, &semantic, existing_timings);
                let start = Instant::now();
                let mut output = self.memory.finish_recall(state, base).await?;
                output["metadata"]["memory_completion_ms"] = json!(start.elapsed().as_millis());
                Ok::<_, VivianError>(PipelineState::from_json(output))
            },
        );
        let preparation = async {
            let prompt = self.prompt.clone();
            let start = Instant::now();
            let prepared = tokio::task::spawn_blocking(move || prompt.prepare_context())
                .await
                .map_err(|error| {
                    VivianError::Other(format!("Prompt preparation failed: {error}"))
                })?;
            Ok::<_, VivianError>((prepared, start.elapsed().as_millis() as u64))
        };
        let ((query, recall), prepared, resolved) = tokio::join!(gathered, preparation, resolution);
        let mut state = recall?;
        state.metadata.as_object_mut().unwrap().extend(resolved.map_err(VivianError::Other)?);
        let (web, query, semantic_timings) = query?;
        let (prepared, preparation_ms) = prepared?;
        // Web gathering owns only external evidence; its input snapshot has no new memories.
        state.web_context = web.web_context.clone();
        append_timings(&mut state, &web, semantic_timings);
        let recall_ms = state.metadata["memory_retrieval_timing"]["elapsed_ms"]
            .as_u64()
            .unwrap_or(0);
        let gathered_ms = started.elapsed().as_millis() as u64;
        state.metadata["prompt_gather"] = json!({"elapsed_ms":gathered_ms,"mode":"parallel"});
        let start = Instant::now();
        let mut output = self
            .prompt
            .ainvoke_prepared(state.to_json(), config, Some(prepared), Some(query))
            .await?;
        let finalize_ms = start.elapsed().as_millis() as u64;
        let timings = &mut output["metadata"]["timings"];
        if !timings.is_array() {
            *timings = json!([]);
        }
        timings.as_array_mut().unwrap().extend([
            json!({"stage":"memory_retrieval","elapsed_ms":recall_ms,"success":true}),
            json!({"stage":"prompt_preparation","elapsed_ms":preparation_ms,"success":true}),
            json!({"stage":"context_gather","elapsed_ms":gathered_ms,"success":true}),
            json!({"stage":"prompt_building","elapsed_ms":finalize_ms,"success":true}),
        ]);
        crate::voice_diagnostics::mark_chat("context_ready");
        tracing::info!(
            gathered_ms,
            preparation_ms,
            finalize_ms,
            "Prompt inputs gathered concurrently; ready for generation"
        );
        Ok(output)
    }
}
