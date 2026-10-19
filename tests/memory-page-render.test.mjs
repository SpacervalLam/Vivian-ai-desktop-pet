// Render loaded MemoryPage branches with production JSX; no Tauri window or app data needed.
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { comicTranslations } from '../src/i18n/comic.js';
globalThis.__memoryLanguage = 'zh-CN';
globalThis.__memoryTranslate = (key, options = {}) => {
  const value = comicTranslations[globalThis.__memoryLanguage][key.replace(/^comic\./, '')] ?? options.defaultValue ?? key;
  return value.replace(/\{\{(\w+)\}\}/g, (_, name) => String(options[name] ?? ''));
};
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.join(root, 'tmp', 'memory-page-render');
await mkdir(out, { recursive: true });
await build({
  stdin: { contents: `import React from 'react-original'; import {renderToStaticMarkup} from 'react-dom/server';
    import Page from './src/components/mind-inspector/pages/MemoryPage';
    export function render(states,props={}) {globalThis.__memoryStates=[...states]; return renderToStaticMarkup(React.createElement(Page,props));}`,
    resolveDir: root, loader: 'tsx' },
  outfile: path.join(out, 'page.mjs'), bundle: true, platform: 'node', format: 'esm',
  external: ['react-original', 'react-dom/server', 'react/jsx-runtime'], loader: { '.css': 'empty' },
  plugins: [{ name: 'loaded-page-fixtures', setup(b) {
    b.onResolve({ filter: /^react-i18next$/ }, () => ({ path: 'translation', namespace: 'translation' }));
    b.onLoad({ filter: /.*/, namespace: 'translation' }, () => ({ contents: `export const useTranslation=()=>({t:globalThis.__memoryTranslate,i18n:{language:globalThis.__memoryLanguage,resolvedLanguage:globalThis.__memoryLanguage}});` }));
    b.onResolve({ filter: /^react$/ }, () => ({ path: 'react', namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: `import original from 'react-original';
      export default original; export const {createElement,Fragment,memo,forwardRef,createContext,useContext,useReducer,useLayoutEffect,useId}=original;
      export const useState=initial=>[globalThis.__memoryStates.length?globalThis.__memoryStates.shift():initial,()=>{}];
      export const useMemo=fn=>fn();export const useCallback=fn=>fn;export const useRef=value=>({current:value});export const useEffect=()=>{};` }));
    b.onResolve({ filter: /^react-original$/ }, () => ({ path: pathToFileURL(path.join(root, 'node_modules/react/index.js')).href, external: true }));
    b.onResolve({ filter: /^@tauri-apps\/api\// }, (args) => ({ path: args.path, namespace: 'tauri' }));
    b.onLoad({ filter: /.*/, namespace: 'tauri' }, () => ({ contents: `export const invoke=()=>Promise.resolve([]);export const listen=()=>Promise.resolve(()=>{});export const convertFileSrc=x=>x;export const emit=()=>{};export const getCurrentWindow=()=>({});` }));
    b.onResolve({ filter: /UserProfilePage$/ }, () => ({ path: 'profile', namespace: 'profile' }));
    b.onLoad({ filter: /.*/, namespace: 'profile' }, () => ({ contents: 'export default function Profile(){return null}' }));
    b.onResolve({ filter: /StickerImage$/ }, () => ({ path: 'sticker', namespace: 'sticker' }));
    b.onLoad({ filter: /.*/, namespace: 'sticker' }, () => ({ contents: 'export default function Sticker(){return null}' }));
  } }],
});
const { render } = await import(pathToFileURL(path.join(out, 'page.mjs')).href);
const records = [{ id: 'conversation-one', title: '与你的对话', started_at: 1000, ended_at: 1010,
  participants: ['user', 'vivian'], channels: ['direct'], turns: [
    { id: 'user-original', speaker: 'user', listener: 'vivian', text: '明天见（挥手）', timestamp: 1000 },
    { id: 'pet-original', speaker: 'vivian', listener: 'user', text: '好呀（点头）', timestamp: 1010 },
  ] }];
const base = { importance: .7, created_at: 1000, tags: [] };
const items = [
  { ...base, id: 'fact', memory_type: 'long_term', content: '约定明天见（下午）', metadata: { record_kind: 'fact', source_quote: '明天见', conversation_id: 'conversation-one' } },
  { ...base, id: 'summary', memory_type: 'session_summary', content: '约好明天再见（时间待定）', metadata: { record_kind: 'session_summary', conversation_id: 'conversation-one', title: '下次见面', summary_status: 'complete' } },
  { ...base, id: 'legacy', memory_type: 'casual_conversation', tags: ['cross_character', 'dialogue', 'topic_summary'], content: 'Vivian 和我聊天：她说：你好；我回复她：你好', metadata: { record_kind: 'internal' } },
  { ...base, id: 'empty-checkpoint', memory_type: 'session_summary', content: '', metadata: { record_kind: 'session_summary', summary_status: 'no_content' } },
];
const page = (layer) => render([layer, '', items, records, [], new Set(), '', '', '', false, '', new Set()]);
const recent = page('recent');
assert.match(recent, /明天见/); assert.match(recent, /memory-action-text/);
assert.match(recent, /查看会话摘要/); assert.match(recent, /更新摘要/);
assert.equal((recent.match(/class="memory-thread"/g) ?? []).length, 1);
for (const layer of ['facts']) {
  const html = page(layer);
  assert.equal(html.includes('memory-action-text'), false);
  assert.equal(html.includes('Vivian 和我聊天'), false);
  assert.match(html, /查看原始会话/);
  assert.equal((html.match(/class="memory-entry memory-entry-/g) ?? []).length, 1);
}
assert.equal(page('episodes').includes('约好明天再见（时间待定）'), false);
console.log('loaded MemoryPage: original dialogue, facts and session summaries render correctly');

const secondRecord = { ...records[0], id: 'conversation-two', turns: [
  { ...records[0].turns[0], id: 'second-original', text: '另一张卡片的原话' },
] };
const secondSummary = { ...items[1], id: 'summary-two', content: '另一张卡片的摘要',
  metadata: { ...items[1].metadata, conversation_id: 'conversation-two' } };
const localView = (layer, key) => render([layer, '', [...items, secondSummary],
  [...records, secondRecord], [], new Set(), '', '', '', false, '', new Set([key])]);
const switchedRecent = localView('recent', 'vivian:thread:conversation-one');
assert.match(switchedRecent, /约好明天再见（时间待定）/);
assert.match(switchedRecent, /另一张卡片的原话/);
assert.equal(switchedRecent.includes('另一张卡片的摘要'), false);
assert.equal(switchedRecent.includes('memory-action-text'), false);
assert.equal((switchedRecent.match(/class="memory-thread"/g) ?? []).length, 2);
const eventItems = [ { ...items[1], metadata: { ...items[1].metadata, event_schema_version: 1, summary_parts: [{ events: [
  { id: 'meeting', title: '约定再见', phase: 'planned', detail: '约好明天见面（地点待定）', source_message_id: 'user-original', source_quote: '明天见（挥手）' },
] }] } }, { ...secondSummary, metadata: { ...secondSummary.metadata, event_schema_version: 1, summary_parts: [{ events: [
  { id: 'meeting', title: '约定再见', phase: 'progressed', detail: '后来确认了见面安排', source_message_id: 'second-original', source_quote: '另一张卡片的原话' },
] }] } } ];
const timeline = render(['episodes', '', eventItems, [...records, secondRecord], [], new Set(), '', '', '', false, '', new Set()]);
assert.equal((timeline.match(/class="memory-event-card"/g) ?? []).length, 1);
assert.match(timeline, /2 次会话 · 2 条进展/);
assert.match(timeline, /约好明天见面（地点待定）/);
assert.match(timeline, /查看来源会话/);
assert.equal(timeline.includes('memory-action-text'), false);
assert.equal(timeline.includes('约好明天再见（时间待定）'), false);
console.log('independent conversation views and cross-conversation event timeline render correctly');

const emptyHistory = render(['recent', '', [...items, { ...base, id: 'obsolete-dialogue', memory_type: 'short_term', content: '旧对白不得回填', tags: ['dialogue_turn'], metadata: { speaker: 'user' } }], [], [], new Set(), '', '', '', false, '', new Set()]);
assert.equal(emptyHistory.includes('旧对白不得回填'), false);
assert.equal(emptyHistory.includes('class="memory-thread"'), false);
const latest = render(['facts', 'saved filter', items, records,
  [{...secondRecord, ended_at: 2000, turns: [{...secondRecord.turns[0], id:'nana-latest', speaker:'nana', text:'最新的一句（轻轻点头）'}]}], new Set(), '', '', '', false, '', new Set()],
  { embedded: true, latestOnly: true });
assert.match(latest, /最新的一句/);
assert.match(latest, /data-message-id="nana-latest"/);
assert.doesNotMatch(latest, /memory-action-text|轻轻点头|（|）/);
const latestStream = latest.split('<div class="memory-history-view"')[0];
assert.equal(latestStream.includes('class="memory-thread"'), false);
assert.equal(latestStream.includes('约定明天见'), false, 'latest stream does not reuse a fact or summary');
assert.equal(latestStream.includes('明天见（挥手）'), false, 'older conversations stay in history');
assert.match(latest, /class="memory-history-view" hidden=""/, 'complete original history stays mounted for draft/filter retention');
console.log('compact manga: latest original exchange across both characters, stable IDs and dialogue markup passed');

const combined = render(['recent', '', items, records,
  [{ ...secondRecord, id: 'nana:conversation-two', title: 'Nana 的对话', ended_at: 2000,
    turns: [{ ...secondRecord.turns[0], speaker: 'nana', text: 'Nana 的原话' }] }],
  new Set(), '', '', '', false, '', new Set()], { embedded: true });
assert.match(combined, /Nana 的原话/);
assert.match(combined, /明天见/);
assert.equal((combined.match(/class="memory-thread"/g) ?? []).length, 2);
assert.ok(combined.indexOf('Nana 的对话') < combined.indexOf('你与Vivian的对话'));
assert.equal(combined.includes('memory-character-switch'), false);
console.log('unified history: both characters render together in time order without a character switch');
for (const [language, label] of [['en', 'View Summary'], ['ja', '会話の要約を見る']]) {
  globalThis.__memoryLanguage = language;
  const localized = page('recent');
  assert(localized.includes(label));
  assert(localized.includes('明天见'), 'original conversation content stays intact');
  assert(!localized.includes('查看会话摘要'), 'built-in memory actions follow the selected language');
  assert(localized.includes(language === 'en' ? 'Conversation between you and Vivian' : 'あなたとVivianの会話'));
  assert(!localized.includes('与你的对话'), 'server-generated conversation titles are localized too');
}
globalThis.__memoryLanguage = 'zh-CN';
const petRecord = owner => ({ id: `${owner}:pet-session`, source_session_id: 'shared-session',
  title: owner === 'vivian' ? '与Nana的对话' : '与Vivian的对话', participants: ['vivian', 'nana'],
  channels: ['cross_character'], started_at: 1000, ended_at: 1010, turns: [
    { id: `${owner}-opening`, utterance_id: 'opening', speaker: 'nana', listener: 'vivian', text: '一起喝茶吧', timestamp: 1000, channel: 'cross_character' },
    { id: `${owner}-answer`, utterance_id: 'answer', speaker: 'vivian', listener: 'nana', text: '我来准备茶杯', timestamp: 1010, channel: 'cross_character' },
  ] });
const mirrorSummary = { ...items[1], metadata: { ...items[1].metadata, conversation_id: 'vivian:pet-session' } };
const mergedHistory = render(['recent', '', [mirrorSummary], [petRecord('vivian')], [petRecord('nana')],
  new Set(), '', '', '', false, '', new Set()]);
assert.equal((mergedHistory.match(/class="memory-thread"/g) ?? []).length, 1);
assert.match(mergedHistory, /Nana与Vivian的对话/);
assert.equal((mergedHistory.match(/一起喝茶吧/g) ?? []).length, 1);
assert.equal((mergedHistory.match(/我来准备茶杯/g) ?? []).length, 1);
assert.match(mergedHistory, /2 条消息/);
assert.match(mergedHistory, /查看会话摘要/, 'summary from the other history remains available');
const mirrorFact = { ...items[0], metadata: { ...items[0].metadata, conversation_id: 'vivian:pet-session' } };
const mergedFacts = render(['facts', '', [mirrorFact], [petRecord('vivian')], [petRecord('nana')],
  new Set(), '', '', '', false, '', new Set()]);
assert.match(mergedFacts, /查看原始会话/, 'evidence keeps its original-history link after merging');
const titleSearch = render(['recent', 'Nana与Vivian', [], [petRecord('vivian')], [petRecord('nana')],
  new Set(), '', '', '', false, '', new Set()]);
assert.match(titleSearch, /Nana与Vivian的对话/);
console.log('merged pet history: initiator title, unique turns, summary aliases, evidence links and title search passed');
