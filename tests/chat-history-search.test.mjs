import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';

// 首页「查找聊天记录」全局搜索：后端 search_chat_history 返回 Unix 秒级时间戳，
// 前端必须过 normalizeTimestamp 转毫秒后再交给 formatSeparatorTime，
// 否则秒数被当作毫秒渲染成 1970 年。
const source = (await readFile('src/components/ChatWindow.tsx', 'utf8')).replace(/\r\n/g, '\n');
const extract = (startMarker, endMarker) => {
  const start = source.indexOf(startMarker);
  assert.ok(start >= 0, `missing marker: ${startMarker}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.ok(end > start, `missing end marker after: ${startMarker}`);
  return source.slice(start, end);
};
const code = (await transform([
  extract('const normalizeTimestamp = (raw', '\nconst toChatTextMessages'),
  // var 化后随 vm 全局暴露，供断言直接调用
  extract('const formatSeparatorTime = (', '\nconst escapeHtml').replace('const formatSeparatorTime', 'var formatSeparatorTime'),
  extract('  /** 聊天记录全局搜索（防抖 300ms） */', '\n  return ('),
].join('\n'), { loader: 'tsx', format: 'cjs' })).code;

// 2026-10-17 00:00:00 UTC 的 Unix 秒；毫秒口径为 1792281600000
const SEC_1733 = 1792281600;
const rows = [
  { id: 'm1', content: '晚上好呀，今晚打算做什么呢？', role: 'assistant', timestamp: SEC_1733, character_id: 'nana', character_name: 'Nana', source: 'private' },
  { id: 'm2', content: '晚安', role: 'user', timestamp: SEC_1733 + 60, character_id: 'vivian', character_name: 'Vivian', source: 'private' },
  { id: 'm3', content: '群里也说晚安', role: 'user', timestamp: SEC_1733 + 120, character_id: 'nana', character_name: 'Nana', source: 'group' },
  // 已是毫秒口径的值必须原样透传，不被二次放大
  { id: 'm4', content: '晚安晚安', role: 'assistant', timestamp: SEC_1733 * 1000, character_id: 'nana', character_name: 'Nana', source: 'private' },
];
const context = {
  useRef: (init) => ({ current: init }),
  useEffect: (fn) => { context.effects.push(fn); },
  effects: [], timers: [], cleared: [], invocations: [],
  searchQuery: '晚',
  setSearchResults: (value) => { context.results = typeof value === 'function' ? value(context.results) : value; },
  setSearching: (value) => { context.searching = value; },
  invoke: async (name, args) => {
    context.invocations.push([name, args]);
    if (context.fail) throw new Error('temporary search failure');
    return rows;
  },
  window: {
    setTimeout: (fn) => { context.timers.push(fn); return context.timers.length; },
    clearTimeout: (id) => { context.cleared.push(id); },
  },
  console: { warn: () => {}, log: () => {}, error: () => {} },
};
vm.createContext(context);
vm.runInContext(code, context);
const runEffect = () => context.effects[0]();

// 防抖：effect 执行只安排定时器，不同步打后端
runEffect();
assert.equal(context.invocations.length, 0, 'search must stay debounced');
assert.equal(context.timers.length, 1);
assert.equal(context.searching, true);

await context.timers[0]();
assert.equal(context.invocations.length, 1);
assert.equal(context.invocations[0][0], 'search_chat_history');
assert.equal(context.invocations[0][1].query, '晚');
assert.equal(context.searching, false);
assert.equal(context.results.length, rows.length);
// 秒 → 毫秒；1970 年渲染的根因就是这里漏了换算
assert.deepEqual(context.results.map((r) => r.timestamp), [SEC_1733 * 1000, (SEC_1733 + 60) * 1000, (SEC_1733 + 120) * 1000, SEC_1733 * 1000]);
assert.equal(new Date(context.results[0].timestamp).getFullYear(), 2026, 'seconds read as milliseconds render 1970');
assert.deepEqual(context.results.map((r) => r.role), ['assistant', 'user', 'user', 'assistant']);
assert.deepEqual(context.results.map((r) => r.source), ['private', 'private', 'group', 'private']);
const label = context.formatSeparatorTime(context.results[0].timestamp, (key) => key);
assert.ok(!label.includes('1970'), `search result time rendered as ${label}`);

// 换关键词：旧防抖定时器被取消，不会有两个在途请求
context.searchQuery = '晚安';
runEffect();
assert.deepEqual(context.cleared, [1], 'a newer query supersedes the pending debounce');

// 空查询：同步清空结果与搜索态
context.searchQuery = '   ';
runEffect();
assert.equal(context.results.length, 0);
assert.equal(context.searching, false);

// 后端失败：清空结果并解除搜索态，不残留转圈
context.searchQuery = '晚';
context.fail = true;
runEffect();
await context.timers.at(-1)();
assert.equal(context.results.length, 0);
assert.equal(context.searching, false);

console.log('Chat history search: debounce, seconds-to-milliseconds timestamps, empty query and failure reset passed.');
