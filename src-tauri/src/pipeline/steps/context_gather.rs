//! Gather prompt inputs by dependency, then assemble once before generation.
use super::{memory::MemoryRetrievalStep, prompt::PromptBuildingStep};
use crate::error::{VivianError, VivianResult};
use crate::pipeline::{
    base::{Runnable, RunnableConfig},
    state::PipelineState,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Instant;

pub struct CompanionContextGather {
    rewrite: Box<dyn Runnable>,
    semantic: Option<Box<dyn Runnable>>,
    memory: MemoryRetrievalStep,
    web: Box<dyn Runnable>,
    prompt: PromptBuildingStep,
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
        }
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
                let state = semantic.map_err(VivianError::Other)?;
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
        let ((query, recall), prepared) = tokio::join!(gathered, preparation);
        let mut state = recall?;
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
        tracing::info!(
            gathered_ms,
            preparation_ms,
            finalize_ms,
            "Prompt inputs gathered concurrently; ready for generation"
        );
        Ok(output)
    }
}
