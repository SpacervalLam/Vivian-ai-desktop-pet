// Compile production startup observations and prompt builders without app data or model calls.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const root = process.cwd();
const dir = path.join(root, 'tmp', 'startup-greeting-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
const read = async file => (await readFile(path.join(root, file), 'utf8')).replaceAll('\r\n', '\n');
const brain = await read('src-tauri/src/brain/brain.rs');
const agent = await read('src-tauri/src/world/environment_agent.rs');
const modules = await read('src-tauri/src/pipeline/prompt_modules.rs');
const method = agent.match(/    pub fn startup_context\([\s\S]*?\n    \}/)?.[0];
const greeting = brain.match(/fn first_contact_prompt\([\s\S]*?\n\}/)?.[0];
const language = brain.match(/fn language_code_to_name\([\s\S]*?\n\}/)?.[0];
const normalize = modules.match(/pub fn normalize_lang\([\s\S]*?\n\}/)?.[0];
const dispatchStart = modules.indexOf('pub fn tone_rule(');
const dispatchEnd = modules.indexOf('// ========== 模块 1', dispatchStart);
assert.ok(method && greeting && language && normalize && dispatchEnd > dispatchStart);
const dispatch = modules.slice(dispatchStart, dispatchEnd)
  .replaceAll('../../prompts/framework/', path.join(root, 'src-tauri/prompts/framework').replaceAll('\\', '/') + '/');
assert.doesNotMatch(brain, /fn fixed_startup_greeting\(/);
assert.match(brain, /self\.generate_llm_greeting\(first_meeting\)\.await/);
assert.match(brain, /environment\.startup_context\(&world_snapshot, first_meeting\)/);
assert.match(brain, /ainvoke_greeting\(&greeting_prompt, first_meeting\)/);

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-startup-greeting-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
`);
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
use serde_json::json;
mod pipeline { pub mod prompt_modules { ${normalize}\n${dispatch} } }
struct Weather {description:String,temperature:f64,cached_at:i64}
struct Location {city:Option<String>}
struct Foreground {process:String}
struct WorldSnapshot {timestamp:i64,local_time:String,weather:Option<Weather>,location:Option<Location>,foreground_window:Option<Foreground>}
struct EnvironmentAgent;
impl EnvironmentAgent { ${method} }
${language}
${greeting}
${brain.slice(brain.indexOf('#[cfg(test)]\nmod first_contact_tests'))}
#[cfg(test)] mod startup_observations {
use super::*;
fn snapshot()->WorldSnapshot { WorldSnapshot{timestamp:1000,local_time:"2026-10-21 09:12".into(),weather:None,location:None,foreground_window:None} }
#[test] fn missing_observations_remain_unknown_and_returning_startup_is_not_first_contact() {
    let agent=EnvironmentAgent; let snap=snapshot();
    let first=agent.startup_context(&snap,true);
    assert_eq!(first["first_contact"],true);
    assert_eq!(first["event"],"first_contact_in_this_installation");
    for key in ["weather","city","foreground_app"] { assert!(first[key].is_null()); }
    let returning=agent.startup_context(&snap,false);
    assert_eq!(returning["first_contact"],false);
    assert_eq!(returning["event"],"returning_startup");
    assert_eq!(returning["user_identity"],"unknown_from_startup");
}
#[test] fn fresh_weather_and_time_reach_the_character_without_environment_authored_speech() {
    let agent=EnvironmentAgent; let mut snap=snapshot();
    snap.weather=Some(Weather{description:"小雨".into(),temperature:18.0,cached_at:950});
    snap.location=Some(Location{city:Some("上海".into())});
    snap.foreground_window=Some(Foreground{process:"editor.exe".into()});
    let first=agent.startup_context(&snap,true);
    assert_eq!(first["weather"]["observed_at"],950);
    assert_eq!(first["city"],"上海"); assert_eq!(first["foreground_app"],"editor.exe");
    assert!(first.get("greeting").is_none());
    let prompt=first_contact_prompt(&first,"zh-CN");
    for fact in ["小雨","18.0","上海","editor.exe"] {assert!(prompt.contains(fact));}
    snap.timestamp=2000; snap.local_time="2026-10-21 10:12".into(); snap.weather=None;
    let next=agent.startup_context(&snap,true);
    assert_eq!(next["observed_at"],2000); assert!(next["weather"].is_null());
    assert_ne!(first_contact_prompt(&next,"zh-CN"),prompt);
}
}
`);
const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'], {
  stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0' },
});
if (run.error) throw run.error;
process.exitCode = run.status ?? 1;
