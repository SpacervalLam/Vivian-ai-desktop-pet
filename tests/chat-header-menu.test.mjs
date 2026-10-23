import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';

// —— 1. 「截屏分析」动作本身 ——
// 动作被抽到 src/utils/companionScreenAnalyze.ts：ChatWindow 有 5000+ 行，
// 埋在 JSX 上方的话这段逻辑没有任何测试入口。
const source = await readFile('src/utils/companionScreenAnalyze.ts', 'utf8');
const stubbed = source
  .replace("import { invoke } from '@tauri-apps/api/core';", 'const invoke = (...a) => globalThis.invoke(...a);')
  .replace("import { emit } from '@tauri-apps/api/event';", 'const emit = (...a) => globalThis.emit(...a);');
assert.notEqual(stubbed, source, 'the tauri imports must be swappable for the harness');
const { code } = await transform(stubbed, { loader: 'ts', format: 'esm' });
const calls = [];
globalThis.invoke = async (name, args) => {
  calls.push({ name, args });
  if (globalThis.__fails) throw new Error('vision unavailable');
  return globalThis.__result;
};
globalThis.emit = async (...args) => { calls.push({ name: 'emit', args }); };
const { analyzeScreen } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const messages = { saved: '已保存至系统截图目录', copied: '已复制到剪贴板' };
const toasts = () => calls.filter(c => c.name === 'emit' && c.args[0] === 'toast:show').map(c => c.args[1]);
const named = name => calls.filter(c => c.name === name);

globalThis.__result = { cancelled: true };
calls.length = 0;
assert.deepEqual(await analyzeScreen(messages), { cancelled: true });
assert.deepEqual(named('companion_screen_analyze').length, 1);
assert.equal(named('companion_screen_analyze')[0].args, undefined, 'character attribution is resolved by the backend, not guessed by the chat window');
assert.equal(toasts().length, 0, 'a cancelled selection never produces a toast');

globalThis.__result = { saved_path: 'C:/Users/Test/Pictures/Screenshots/selected.png', copied: true };
calls.length = 0;
await analyzeScreen(messages);
assert.equal(toasts().length, 1, 'a plain save confirms where the screenshot went');
assert.equal(toasts()[0].message, messages.saved);
assert.equal(toasts()[0].type, 'success');
assert.equal(toasts()[0].duration, 2500);
assert.ok(Number.isFinite(toasts()[0].key), 'toasts are keyed so repeats do not stack');
assert.equal(named('chat:assistant_message').length, 0, 'a plain save never fabricates an AI chat reply');

globalThis.__result = { copied: true };
calls.length = 0;
await analyzeScreen(messages);
assert.equal(toasts().length, 1);
assert.equal(toasts()[0].message, messages.copied);

globalThis.__result = { handled: true, saved_path: 'C:/Users/Test/Pictures/Screenshots/selected.png' };
calls.length = 0;
await analyzeScreen(messages);
assert.equal(toasts().length, 0, 'an analysed screenshot is relayed by the pet, never toasted — even when it was also saved');
assert.equal(named('companion:interaction').length, 0, 'the frontend must not relay the analysis itself; that is the backend pet path');

globalThis.__fails = true;
calls.length = 0;
assert.equal(await analyzeScreen(messages), null);
assert.equal(toasts().length, 1);
assert.equal(toasts()[0].type, 'error');
assert.equal(toasts()[0].duration, 4000);
assert.match(toasts()[0].message, /vision unavailable/);
delete globalThis.__fails;
delete globalThis.__result;

// —— 2. 头部下拉的接线 ——
const chat = await readFile('src/components/ChatWindow.tsx', 'utf8');
assert.match(chat, /import \{ analyzeScreen \} from '\.\.\/utils\/companionScreenAnalyze'/, 'the dropdown reuses the extracted action');
assert.doesNotMatch(chat, /invoke<\{ cancelled\?: boolean/, 'the action is not duplicated inline in ChatWindow');
assert.match(chat, /t\('chat\.screen_saved'/, 'the saved hint goes through i18n');

// —— 3. 分析结果由桌宠转述：后端 emit 气泡，前端不插手 ——
const assistant = await readFile('src-tauri/src/commands/desktop_assistant.rs', 'utf8');
assert.match(assistant, /let analysis = analyze_snapshot\(app, state, analysis_character, selected\.png\)\.await;[\s\S]*?let description = analysis\.map_err\([\s\S]*?relay_visual_description\(app, state, analysis_character, description\);/, 'the selected pet receives the screenshot analysis after error handling');
assert.match(assistant, /let description = analyze_snapshot\(app, state, character_id, png\)\.await\?;\s*relay_visual_description\(app, state, character_id, description\);/, 'the frozen screenshot analysis is handed to the same pet');
assert.match(assistant, /emit\("companion:interaction"/, 'the relay rides the existing bubble channel');
assert.match(assistant, /"motion": "happy"/, 'the relay carries an expression so the pet visibly reacts');
assert.match(assistant, /"replace": true/, 'the relay replaces the in-flight image bubble instead of being swallowed');
assert.match(assistant, /tauri::async_runtime::spawn/, 'the relay must not be awaited on the screenshot path');
const feedback = await readFile('src/hooks/useCompanionFeedback.ts', 'utf8');
assert.match(feedback, /if \(p\.replace\)/, 'the pet honours the replace flag');
assert.match(feedback, /BubbleController\.showBubble\(p\.content\)/, 'the relay ends up as a pet bubble');
const reaction = await readFile('src-tauri/src/commands/pet_reaction.rs', 'utf8');
assert.match(reaction, /pub async fn relay_screen_analysis/, 'the relay reuses the lightweight reaction module');
assert.match(reaction, /fn clean_relay\(/, 'the relay has its own cleaner that keeps multiple sentences');
assert.match(reaction, /VISUAL_RELAY_MAX_CHARS: usize = 140/, 'the relay gets a wider budget than a 60-char touch reaction');

// —— 4. 下拉文案三语齐全 ——
const i18n = await readFile('src/i18n/index.ts', 'utf8');
for (const key of ['tool_notes', 'tool_shortcuts', 'tool_screen', 'tool_settings', 'screen_saved', 'screen_copied']) {
  assert.equal((i18n.match(new RegExp(`\\b${key}:`, 'g')) ?? []).length, 3, `${key} must exist in zh / en / ja`);
}
// 转述走的是 pet_reaction 档位，设置面板的说明必须说清它现在还承担什么。
assert.equal((i18n.match(/routing_pet_reaction_help:/g) ?? []).length, 3, 'the routing help must exist in zh / en / ja');
for (const help of [/转述/, /relaying screen-analysis/, /言い換え/]) {
  assert.match(i18n, help, 'each language explains the relay role');
}

delete globalThis.invoke;
delete globalThis.emit;
console.log('Header menu: screen analysis is relayed by the pet, plain save/copy still toasts, dropdown copy is trilingual');
