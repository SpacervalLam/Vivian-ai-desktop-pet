// 隔离验证 search_chat_history 的渠道过滤/去重/分类逻辑：从生产源码切片提取
// 真实函数体，桩掉 Tauri/AppState 边界后离线编译运行（整 crate check 在 16GB
// 机器上会 OOM，且本逻辑不依赖应用边界）。生产代码若漂移，切片标记找不到即失败。
import { mkdir, writeFile, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'chat-history-search-scope');
await mkdir(path.join(dir, 'src'), { recursive: true });

const source = (await readFile(path.join(root, 'src-tauri', 'src', 'commands', 'history.rs'), 'utf8')).replace(/\r\n/g, '\n');
const slice = (start, end, from = 0) => {
  const s = source.indexOf(start, from);
  if (s < 0) throw new Error(`production marker missing: ${start}`);
  const e = source.indexOf(end, s + start.length);
  if (e < 0) throw new Error(`production end marker missing after: ${start}`);
  return source.slice(s, e + end.length);
};
const dedent = (code, spaces) => code.split('\n').map((line) => (line.startsWith(' '.repeat(spaces)) ? line.slice(spaces) : line)).join('\n');

// 渠道可见性谓词（含本次新增的 visible_in_chat_search）逐字提取；去掉尾随的 tauri 属性行
const predicates = slice('// Mode transitions remain in the pets', '#[tauri::command]')
  .split('\n').filter((line) => line.trim() !== '#[tauri::command]').join('\n').trimEnd();
// 命令前置：查询规范化 + 结果上限
const prelude = dedent(slice('    let q = query.trim().to_lowercase();', '    const MAX_RESULTS: usize = 50;'), 4);
// 搜索主体：过滤/去重/渠道键/来源分类/排序
const body = dedent(slice('        let mut groups:', '        Ok(result)'), 8);

if (!body.includes('visible_in_chat_search(&e.metadata)')) {
  throw new Error('search body lost the channel whitelist filter');
}
if (!body.includes('let key = format!("{}|{}|{}", channel, content, ts);')) {
  throw new Error('search body lost the channel-aware dedup key');
}

const main = [
  '// 由 tests/chat-history-search-scope.test.mjs 生成：逻辑本体提取自 src-tauri/src/commands/history.rs',
  'use serde_json::{json, Value};',
  '',
  '// 与 crate::dialogue::HistoryEntry 同构',
  '#[derive(Clone)]',
  'struct HistoryEntry { id: String, role: String, content: String, timestamp: f64, metadata: Value }',
  '',
  '// 桩：替代 DialogueManager，只保留搜索用到的读取面',
  'struct DialogueManager { entries: Vec<HistoryEntry> }',
  'impl DialogueManager {',
  '    fn get_all_history(&self) -> Result<Vec<HistoryEntry>, String> { Ok(self.entries.clone()) }',
  '}',
  '',
  predicates,
  '',
  'fn search_chat_history(chars: Vec<(DialogueManager, String, String)>, query: String) -> Result<Vec<Value>, String> {',
  prelude,
  body,
  '}',
  '',
  'fn entry(id: &str, role: &str, content: &str, ts: f64, metadata: Value) -> HistoryEntry {',
  '    HistoryEntry { id: id.into(), role: role.into(), content: content.into(), timestamp: ts, metadata }',
  '}',
  '',
  'fn texts(results: &[Value]) -> Vec<String> {',
  '    results.iter().map(|r| r["content"].as_str().unwrap().to_string()).collect()',
  '}',
  '',
  'fn sources(results: &[Value]) -> Vec<String> {',
  '    results.iter().map(|r| r["source"].as_str().unwrap().to_string()).collect()',
  '}',
  '',
  'fn main() {',
  '    // 渠道白名单：聊天窗口的两个会话入口 + 旧数据兼容',
  '    let searchable = |c: Option<&str>| {',
  '        let metadata = match c {',
  '            Some(c) => json!({ "channel": c }),',
  '            None => json!({ "utterance_id": "legacy" }),',
  '        };',
  '        visible_in_chat_search(&metadata)',
  '    };',
  '    assert!(searchable(Some("wechat")) && searchable(Some("wechat_group")) && searchable(None));',
  '    for hidden in ["proactive", "presence", "direct", "cross_character", "inner", "broadcast"] {',
  '        assert!(!searchable(Some(hidden)), "{hidden} must not surface in chat search");',
  '    }',
  '',
  '    let nana = DialogueManager { entries: vec![',
  '        entry("n1", "user", "晚上好呀", 100.0, json!({ "channel": "wechat" })),',
  '        entry("n2", "assistant", "晚安", 101.0, json!({ "channel": "wechat" })),',
  '        entry("n3", "assistant", "桌宠气泡也说晚安", 102.0, json!({ "channel": "proactive" })),',
  '        entry("n4", "assistant", "presence 频道晚安", 103.0, json!({ "channel": "presence" })),',
  '        entry("n5", "system", "系统提示晚安", 104.0, json!({ "channel": "wechat" })),',
  '        entry("n6", "assistant", "legacy 晚安", 105.0, json!({})),',
  '        entry("n7", "assistant", "quiet_control 晚安", 106.0, json!({ "source": "quiet_control", "channel": "wechat" })),',
  '        entry("n8", "assistant", "同内容不同渠道", 300.0, json!({ "channel": "wechat" })),',
  '        entry("g1", "user", "群里大家晚安", 200.0, json!({ "channel": "wechat_group" })),',
  '    ]};',
  '    let vivian = DialogueManager { entries: vec![',
  '        entry("v1", "assistant", "晚安呀", 201.0, json!({ "channel": "wechat" })),',
  '        entry("v2", "assistant", "cross 角色晚安", 202.0, json!({ "channel": "cross_character" })),',
  '        entry("v3", "assistant", "direct 晚安", 203.0, json!({ "channel": "direct" })),',
  '        entry("v4", "assistant", "inner 晚安", 204.0, json!({ "channel": "inner" })),',
  '        entry("v5", "assistant", "broadcast 晚安", 205.0, json!({ "channel": "broadcast" })),',
  '        entry("g1", "user", "群里大家晚安", 200.0, json!({ "channel": "wechat_group" })),',
  '        entry("g2", "assistant", "同内容不同渠道", 300.0, json!({ "channel": "wechat_group" })),',
  '    ]};',
  '    let chars = vec![',
  '        (nana, "nana".to_string(), "Nana".to_string()),',
  '        (vivian, "vivian".to_string(), "Vivian".to_string()),',
  '    ];',
  '',
  '    let results = search_chat_history(chars, "晚安".to_string()).unwrap();',
  '    assert_eq!(texts(&results), vec!["晚安呀", "群里大家晚安", "legacy 晚安", "晚安"],',
  '        "only wechat/wechat_group/legacy entries survive, newest first");',
  '    assert_eq!(sources(&results), vec!["private", "group", "private", "private"]);',
  '    for banned in ["桌宠气泡", "presence", "系统提示", "quiet_control", "cross", "direct", "inner", "broadcast"] {',
  '        assert!(!texts(&results).iter().any(|c| c.contains(banned)), "{banned} leaked into search");',
  '    }',
  '    for r in &results {',
  '        assert!(r.get("matched_chars").is_none(), "matched_chars must be stripped from the payload");',
  '    }',
  '    let timestamps: Vec<f64> = results.iter().map(|r| r["timestamp"].as_f64().unwrap()).collect();',
  '    assert_eq!(timestamps, vec![201.0, 200.0, 105.0, 101.0]);',
  '    let group = results.iter().find(|r| r["content"] == "群里大家晚安").unwrap();',
  '    assert_eq!(group["source"], "group");',
  '    assert_eq!(group["character_id"], "nana", "first-seen character owns the merged group hit");',
  '',
  '    // 渠道进入去重键：同内容同时戳的私聊/群聊不再互相淹没',
  '    let mixed = search_chat_history(',
  '        vec![',
  '            (DialogueManager { entries: vec![entry("m1", "user", "同内容不同渠道", 300.0, json!({ "channel": "wechat" }))] }, "nana".into(), "Nana".into()),',
  '            (DialogueManager { entries: vec![entry("m2", "assistant", "同内容不同渠道", 300.0, json!({ "channel": "wechat_group" }))] }, "vivian".into(), "Vivian".into()),',
  '        ],',
  '        "同内容".to_string(),',
  '    ).unwrap();',
  '    assert_eq!(texts(&mixed), vec!["同内容不同渠道", "同内容不同渠道"], "cross-channel collisions must not merge");',
  '    // 同时间戳稳定排序保持插入序（nana 先于 vivian 处理）；关键是两条都在且渠道各自独立',
  '    assert_eq!(sources(&mixed), vec!["private", "group"], "explicit channel wins over merge order");',
  '',
  '    assert!(search_chat_history(Vec::new(), "   ".to_string()).unwrap().is_empty());',
  '    println!("search_chat_history scope: channel whitelist, group dedup, cross-channel keys, ordering and payload shape passed.");',
  '}',
].join('\n');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name = "chat-history-search-scope"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
serde_json = "1"
`);
await writeFile(path.join(dir, 'src', 'main.rs'), main + '\n');

// debuginfo=2 会让 rustc 在本机栈溢出，关掉；离线走 registry 缓存
const result = spawnSync('cargo', ['run', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
