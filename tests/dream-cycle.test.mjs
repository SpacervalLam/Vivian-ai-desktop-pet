// Execute the production dream/diary coordinator and prompt builder with isolated I/O and workers.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dir = path.join(root, 'tmp/dream-cycle-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
const source = async file => (await readFile(path.join(root, file), 'utf8')).replaceAll('\r\n', '\n');
const rustPath = file => path.join(root, file).replaceAll('\\', '/');
const [diary, consolidation, generator, modules, proactive, commands, environment, diaryTool, dreamAdapter] = await Promise.all([
  source('src-tauri/src/diary/mod.rs'), source('src-tauri/src/memory/consolidation.rs'),
  source('src-tauri/src/diary/intelligent_generator.rs'), source('src-tauri/src/pipeline/prompt_modules.rs'),
  source('src-tauri/src/commands/proactive.rs'), source('src-tauri/src/commands/diary.rs'),
  source('src-tauri/src/world/environment_agent.rs'), source('src-tauri/src/tools/builtin/diary_tools.rs'),
  source('src-tauri/src/diary/dream.rs'),
]);
function between(text, first, end) {
  const a = text.indexOf(first), b = text.indexOf(end, a);
  assert.ok(a >= 0 && b > a, `missing source: ${first}`);return text.slice(a, b);
}
function item(text, marker) { const start=text.indexOf(marker);assert.ok(start>=0,marker);const open=text.indexOf('{',start);let n=1,i=open+1;for(;n&&i<text.length;i++){if(text[i]==='{')n++;else if(text[i]==='}')n--;}return text.slice(start,i); }
// Only an accepted environment decision dispatches automatic daily work.
assert.ok(!proactive.includes('spawn_auto_dream_check('));
assert.ok(!proactive.includes('spawn_auto_diary_check('));
assert.ok(environment.includes('super::environment_policy::planning_prompt('));
assert.ok(environment.includes('diary_candidate.as_ref()'));
assert.ok(environment.includes('decision.private_activity.and_then('));
assert.ok(environment.includes('selection.matches(candidate)'));
assert.ok(environment.indexOf('if let Some(candidate) = selected_cycle') < environment.indexOf('crate::diary::dream::spawn_selected('));
assert.ok(diaryTool.includes('environment.request_diary_check('));
assert.ok(!diaryTool.includes('generate_intelligent_diary(&brain'));
assert.ok(between(commands,'pub async fn generate_diary(', '/// 智能日记生成').includes('generate_diary_intelligent('));
assert.ok(item(diary, 'async fn catch_up_one_day(').includes('.request(date)'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-dream-cycle-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono={version="0.4",features=["serde"]}
async-trait="0.1"
thiserror="2"
once_cell="1"
parking_lot="0.12"
tokio={version="1",features=["macros","rt-multi-thread","sync","time","test-util"]}
tempfile="3"
tracing="0.1"
uuid={version="1",features=["v4"]}
`);
const diaryTypes = diary.slice(diary.lastIndexOf('#[derive', diary.indexOf('pub struct DiaryEntry')), diary.lastIndexOf('#[derive', diary.indexOf('pub struct DiaryConfig')));
const context = between(generator, '#[derive(Debug, Clone, Default)]\npub struct DailyContext', 'impl InteractionRecord {');
const prompt = between(generator, 'fn build_prompt(', '/// 解析 LLM 返回的日记 JSON');
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
pub mod error {
    #[derive(Debug,thiserror::Error)] pub enum VivianError {
        #[error("{0}")] Memory(String),
        #[error(transparent)] Io(#[from]std::io::Error),
        #[error(transparent)] Json(#[from]serde_json::Error),
    }
    pub type VivianResult<T>=Result<T,VivianError>;
}
pub mod utils { pub mod fs {
    pub fn load_json_or_backup<T:serde::de::DeserializeOwned>(p:&std::path::Path)->Option<T>{std::fs::read_to_string(p).ok().and_then(|s|serde_json::from_str(&s).ok())}
    pub fn write_atomic(p:&std::path::Path,text:&str)->std::io::Result<()>{std::fs::write(p,text)}
} }
pub mod memory {
    #[path="${rustPath('src-tauri/src/memory/types.rs')}"] pub mod types;
    #[path="${rustPath('src-tauri/src/memory/kinds.rs')}"] pub mod kinds;
    pub mod consolidation {
    #[derive(Debug,Clone,Default,serde::Serialize,serde::Deserialize)] ${item(consolidation,'pub struct DreamReport {')}
} }
pub mod pipeline { pub mod prompt_modules { ${item(modules,'pub fn normalize_lang(')} } }
pub mod presence { #[path="${rustPath('src-tauri/src/presence/activity_budget.rs')}"] pub mod activity_budget; }
pub mod world { pub mod environment_policy {
    #[derive(Debug,Clone)] ${item(await source('src-tauri/src/world/environment_policy.rs'),'pub struct DiaryCandidate {')}
} }
pub mod brain {
    pub struct Material { pub count:std::sync::atomic::AtomicUsize,pub scans:std::sync::atomic::AtomicUsize }
    impl Material { pub fn count_diary_material(&self,_:f64,_:f64,limit:usize)->usize{self.scans.fetch_add(1,std::sync::atomic::Ordering::Relaxed);self.count.load(std::sync::atomic::Ordering::Relaxed).min(limit)} }
    pub struct Brain {pub char_id:String,pub memory:Material}
}
pub mod diary {
    use serde::{Deserialize,Serialize};
    ${diaryTypes}
    pub static ROOTS:once_cell::sync::Lazy<parking_lot::Mutex<std::collections::HashMap<String,std::path::PathBuf>>>=once_cell::sync::Lazy::new(||parking_lot::Mutex::new(Default::default()));
    pub static CONFIGS:once_cell::sync::Lazy<parking_lot::Mutex<std::collections::HashMap<String,DiaryConfig>>>=once_cell::sync::Lazy::new(||parking_lot::Mutex::new(Default::default()));
    #[derive(Debug,Clone)] ${item(diary,'pub struct DiaryConfig {')}
    ${item(diary,'impl Default for DiaryConfig {')}
    pub fn diary_dir(id:&str)->std::path::PathBuf{ROOTS.lock().get(id).expect("tests supply isolated roots").clone()}
    pub fn get_config(id:&str)->crate::error::VivianResult<DiaryConfig>{Ok(CONFIGS.lock().get(id).cloned().unwrap_or_default())}
    pub fn get_entries(id:&str,date:Option<&str>)->crate::error::VivianResult<Vec<DiaryEntry>>{
        let path=diary_dir(id).join("test-entries.json");if !path.exists(){return Ok(Vec::new());}
        let entries:Vec<DiaryEntry>=serde_json::from_str(&std::fs::read_to_string(path)?)?;
        Ok(entries.into_iter().filter(|entry|date.is_none_or(|date|entry.date==date)).collect())
    }
    #[path="${rustPath('src-tauri/src/diary/cycle.rs')}"] pub mod cycle;
    pub mod dream {
        use chrono::NaiveDate;use crate::{brain::Brain,error::VivianResult};
        use super::cycle::day_bounds;
        ${item(dreamAdapter,'pub fn candidate(')}
    }
    pub mod ongoing_stories { #[derive(Debug,Clone)] pub struct OngoingStory {pub title:String,pub status:String,pub summary:String} }
    pub mod intelligent_generator {
        use serde_json::Value;
        use chrono::NaiveDate;
        use super::DiaryEntry;
        use crate::memory::types::MemoryItem;
        use super::{MoodSample,RelationshipDelta};
        ${context}
        ${prompt}
        ${between(generator, 'fn extract_user_dialogue(', '/// 收集今日情绪 / 关系状态摘要')}
        ${item(generator, 'pub(crate) fn preceding_diaries(')}
        pub fn test_prompt(ctx:&DailyContext,lang:&str)->(String,String){build_prompt(ctx,lang)}
        pub fn test_original_dialogue(memories:Vec<MemoryItem>)->Vec<InteractionRecord>{extract_user_dialogue(memories)}
    }
}
#[path="${rustPath('src-tauri/src/mind/belief_sources.rs')}"] pub mod belief_sources;
#[cfg(test)] #[path="${rustPath('tests/fixtures/dream-cycle-tests.rs')}"] mod regressions;
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'],
  { cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0' } });
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
