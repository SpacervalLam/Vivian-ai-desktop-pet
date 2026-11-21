// Run the actual outer chat-chain invocation prefix with local provider probes.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = await readFile(process.env.VIVIAN_SAMPLING_SOURCE ?? 'src-tauri/src/brain/chat_chain.rs', 'utf8');
const method = source.indexOf('async fn execute_pipeline_and_build_response(');
const start = source.indexOf('let mut config = RunnableConfig::default();', method);
const end = source.indexOf('let final_state = PipelineState::from_json(result);', start);
assert.ok(method >= 0 && start > method && end > start);
const prefix = source.slice(start, end);
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-dialogue-sampling-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="dialogue-sampling-regression"
version="0.0.0"
edition="2021"
[dependencies]
serde_json="1"
tokio={version="1",features=["rt","macros"]}
[[bin]]
name="dialogue-sampling-regression"
path="main.rs"
`);
  await writeFile(path.join(dir, 'main.rs'), `#![allow(dead_code)]
use serde_json::{json,Value};
mod providers { pub mod base { pub const TASK_COMPANION:&str="companion"; } }
#[derive(Default)] struct RunnableConfig {metadata:Value,tags:Vec<String>}
struct PipelineState;
impl PipelineState {fn to_json(&self)->Value {json!({})}}
tokio::task_local! {static TEMPERATURE:f64;}
#[derive(Default)] struct ProviderCallOptions {temperature:Option<f64>}
struct Router;
impl Router {async fn scope_call_options<F:std::future::Future>(&self,o:ProviderCallOptions,f:F)->F::Output {
    TEMPERATURE.scope(o.temperature.unwrap(),f).await
}}
struct Emotion(f64);
impl Emotion {fn to_8d_vector(&self,_:f64)->Self {Self(self.0)} fn to_temperature_default(&self)->f64 {self.0}}
struct Relationship {trust:f64}
struct Psychology {mood:f64}
impl Psychology {fn emotion(&self)->Emotion {Emotion(self.mood)} fn relationship(&self)->Relationship {Relationship{trust:0.5}}}
struct Advisor;
impl Advisor {async fn ainvoke(&self,_:Value,c:Option<RunnableConfig>)->Result<Value,String> {
    let c=c.unwrap();
    Ok(json!({"companion":TEMPERATURE.try_with(|v|*v).unwrap_or(0.7),
        "execution":TEMPERATURE.try_with(|v|*v).unwrap_or(0.2),
        "stream":c.tags.contains(&"stream".to_string()),"task":c.metadata["task_type"]}))
}}
struct Chain {router:Router,psychology:Psychology,advisor_chain:Advisor}
impl Chain {async fn run(&self,stream:bool)->Result<Value,String> {
    let state=PipelineState;
    ${prefix}
    Ok(result)
}}
#[tokio::test(flavor="current_thread")]
async fn mood_cannot_replace_configured_dialogue_or_nested_execution_sampling() {
    for mood in [0.4,1.2] {for stream in [false,true] {
        let chain=Chain{router:Router,psychology:Psychology{mood},advisor_chain:Advisor};
        let result=chain.run(stream).await.unwrap();
        assert_eq!(result["companion"],0.7,"explicit dialogue sampling was overwritten by mood");
        assert_eq!(result["execution"],0.2,"dialogue mood leaked into nested execution");
        assert_eq!(result["stream"],stream);assert_eq!(result["task"],"companion");
    }}
}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet'], {
    encoding: 'utf8', env: { ...process.env, CARGO_TARGET_DIR: path.resolve('tmp/dialogue-sampling-target') },
  });
  assert.equal(run.status, 0, run.stdout + run.stderr);
  console.log('Production outer chat invocation: configured sampling and nested task isolation survive mood changes in streaming and non-streaming turns.');
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  await rm(dir, { recursive: true, force: true });
}
