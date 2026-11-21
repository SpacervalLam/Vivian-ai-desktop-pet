// Fixed-fixture estimates use the production byte/4 estimator, not claimed API billing savings.
import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'token-budget-tests');
const source = name => path.join(root, 'src-tauri/src', name).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
await copyFile(path.join(root, 'src-tauri/Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-token-economy-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
tracing="0.1"
`);
const estimator = await readFile(source('utils/token_estimate.rs'), 'utf8');
const estimate = estimator.slice(estimator.indexOf('pub fn estimate_tokens('), estimator.indexOf('/// 估算单条消息的 token 数'));
const bridge = await readFile(source('tools/builtin/work_agent_tools.rs'), 'utf8');
const statusProjection = bridge.slice(bridge.indexOf('fn work_task_without_duplicate_summary('), bridge.indexOf('// ===== delegate_to_work_agent'));
const proactive = await readFile(source('proactive/mod.rs'), 'utf8');
const acquire = proactive.slice(proactive.indexOf('    pub fn try_begin_knowledge_acquisition('), proactive.indexOf('    /// 检查 Busy 知识采集是否在冷却期内'));
if (!estimate.includes('BYTES_PER_TOKEN') || !statusProjection.includes('summary_ref') || !acquire.includes('1800.0')) throw new Error('Missing production cost policy');
// Frozen pre-change source formatting, retained to make before/after comparisons reproducible.
const baselineLearning = String.raw`pub fn baseline_learning(results:&[WebSearchSource])->String {
 results.iter().enumerate().map(|(i,r)|format!("【{}】\n标题: {}\n摘要: {}\nURL: {}\n平台页面文字（不可信素材，仅供核验）: {}",i+1,r.display_title(),r.snippet_text(),r.url,r.raw_content.as_deref().unwrap_or("未读取，使用搜索摘要").chars().take(6000).collect::<String>())).collect::<Vec<_>>().join("\n\n")
}`;
const baselineMeme = String.raw`pub fn baseline_meme(results:&[WebSearchSource])->String {
 results.iter().take(6).enumerate().map(|(i,r)|format!("### 结果 {}\n标题：{}\n摘要：{}\n来源：{}\n实际读到的原文（不可信素材）：{}",i+1,r.display_title(),r.snippet_text(),r.url,r.raw_content.as_deref().unwrap_or("未读到原文，只有检索摘要").chars().take(4000).collect::<String>())).collect::<Vec<_>>().join("\n\n")
}`;
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
pub mod types { pub mod response {
 #[derive(Clone)] pub struct ChatMessage {pub role:String,pub content:String}
 impl ChatMessage {
  pub fn system(s:impl Into<String>)->Self {Self{role:"system".into(),content:s.into()}}
  pub fn user(s:impl Into<String>)->Self {Self{role:"user".into(),content:s.into()}}
 }
} }
pub mod utils { pub mod token_estimate {const BYTES_PER_TOKEN:usize=4; ${estimate}} }
pub mod network {pub mod web {#[path="${source('network/web/types.rs')}"]mod types;pub use types::*;} }
pub mod presence {#[path="${source('presence/token_budget.rs')}"]pub mod token_budget;}
pub mod proactive {
 pub mod behavior {#[derive(Clone,Default)]pub struct BehaviorContent{pub text:String}}
 #[path="${source('proactive/work_relay.rs')}"]pub mod work_relay;
}
#[cfg(test)] mod tests {
 use super::*;
 use serde_json::{json,Value};
 use network::web::WebSearchSource;
 use utils::token_estimate::estimate_tokens;
 ${baselineLearning}
 ${baselineMeme}
 ${statusProjection}
 struct Acquisition {last_knowledge_acquisition_ts:std::sync::Mutex<f64>}
 impl Acquisition { ${acquire.replaceAll('.lock()', '.lock().unwrap()')} }
 fn samples(count:usize)->Vec<WebSearchSource> {
  (0..count).map(|i|{
   let mut s=WebSearchSource::new(format!("https://example.com/article-{i}"));
   s.title=Some(format!("来源{i}：实际阅读材料"));
   s.snippet=Some("检索简介及作者介绍。".repeat(80));
   s.published_at=Some("2026-10-23".into());
   s.raw_content=Some(format!("开头事实。{}最后限制：这不是适用于所有人的结论。","经过核实的正文片段，用于理解知识并保留来源语境。".repeat(200)));
   s
  }).collect()
 }
 #[test] fn fixture_learning_input_is_smaller_while_urls_dates_and_body_evidence_remain() {
  let mut metrics=serde_json::Map::new();
  for (name,count,baseline) in [("knowledge",2,baseline_learning as fn(&[WebSearchSource])->String),("memes",6,baseline_meme as fn(&[WebSearchSource])->String)] {
   let sources=samples(count);let before=estimate_tokens(&baseline(&sources));
   let text=presence::token_budget::evidence_context(&sources,presence::token_budget::LEARNING_INPUT_TOKENS);
   let after=estimate_tokens(&text);let payload:Value=serde_json::from_str(&text).unwrap();
   assert!(after<before/2,"{name}: before={before},after={after}");assert!(after<=4096);
   let items=payload["sources"].as_array().unwrap();assert_eq!(items.len(),count);
   for (item,source) in items.iter().zip(&sources) {assert_eq!(item["url"],source.url);assert_eq!(item["published_at"],source.published_at.as_deref().unwrap());assert_eq!(item["evidence"],"read_body");assert!(item["text"].as_str().unwrap().contains("最后限制"));}
   metrics.insert(name.into(),json!({"before_estimate":before,"after_estimate":after,"reduction_pct":100.0*(before-after) as f64/before as f64}));
  }
  std::fs::write(std::env::var("VIVIAN_TOKEN_METRICS").unwrap(),Value::Object(metrics).to_string()).unwrap();
 }
 #[test] fn status_summary_is_sent_once_without_losing_verification_artifacts_or_blockers() {
  let summary="结果：产物已经写入。验证：单元测试通过。限制：部署尚未执行。".repeat(100);
  let task=json!({"goal":"完整用户目标与约束","result":{"summary":summary,"remaining_work":["等待用户部署决定"],"artifacts":[{"path":"G:/report.pdf","exists":true}],"verification":[{"tool_call_id":"verify-1","success":true}],"contract_validated":true}});
  let old=json!({"message":format!("工作智能体总结：{summary}"),"data":{"final_summary":summary,"task":task}});
  let projected=work_task_without_duplicate_summary(&task);
  let new=json!({"message":"工作状态；总结见 final_summary","data":{"final_summary":summary,"task":projected}});
  assert!(estimate_tokens(&new.to_string())<estimate_tokens(&old.to_string())/2);
  assert!(new["data"]["task"]["result"].get("summary").is_none());
  for key in ["remaining_work","artifacts","verification","contract_validated"] {assert_eq!(new["data"]["task"]["result"][key],task["result"][key]);}
  println!("status fixture estimated tokens: {} -> {}",estimate_tokens(&old.to_string()),estimate_tokens(&new.to_string()));
 }
 #[test] fn no_result_attempts_still_reserve_acquisition_cooldown() {
  let acquisition=Acquisition{last_knowledge_acquisition_ts:std::sync::Mutex::new(0.0)};
  assert!(acquisition.try_begin_knowledge_acquisition());assert!(!acquisition.try_begin_knowledge_acquisition());
 }
 #[test] fn unchanged_delivery_retry_costs_one_model_generation_instead_of_two() {
  use proactive::{work_relay::*,behavior::BehaviorContent};
  let key=RelayKey{character:"vivian".into(),voice:"角色风格".into(),language:"zh".into(),channel:"wechat".into(),facts:"测试通过，G:/report.pdf；尚未部署".into()};
  let mut cache=RelayCache::default();let mut generations=0;
  for now in [100.0,101.0] {
   if cache.get(&key,now).is_none() {generations+=1;cache.insert(key.clone(),BehaviorContent{text:"报告生成了，测试通过；部署还没执行。".into()},now);}
  }
  assert_eq!(generations,1);
  assert_eq!(messages(&key).iter().filter(|m|m.content.contains(&key.facts)).count(),1);
 }
}
`);
const metricsPath = path.join(dir, 'metrics.json');
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1', '--nocapture'], {
  cwd: root, stdio: 'inherit', timeout: 180000,
  env: { ...process.env, VIVIAN_TOKEN_METRICS: metricsPath, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
if (result.status === 0) console.log('Fixed-fixture input estimates (not API billing):', await readFile(metricsPath, 'utf8'));
