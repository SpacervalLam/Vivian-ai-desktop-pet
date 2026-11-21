import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';

const source = (await readFile('src/components/ChatWindow.tsx', 'utf8')).replace(/\r\n/g, '\n');
const extract = (name, next) => {
  const start = source.indexOf(`  const ${name} = useCallback(`);
  const end = source.indexOf(next, start);
  assert.ok(start >= 0 && end > start);
  return source.slice(start, end).replace(`const ${name}`, `var ${name}`);
};
const code = (await transform([
  extract('loadHistory', '  // 进入私聊视图'),
  extract('refreshHistory', '  const refreshHistoryRef'),
  extract('loadMore', '  const handleScroll'),
].join('\n'), { loader: 'tsx', format: 'cjs' })).code;
const data = Array.from({ length: 75 }, (_, i) => ({ id: String(i), role: 'user', content: `original ${i}`, timestamp: i + 1, metadata: { channel: 'wechat' } }));
let visible = [], more = false, fail = false, deferred;
const calls = [];
const context = {
  useCallback: callback => callback, PAGE_SIZE: 20, privateCharId: 'one', privateCharIdRef: { current: 'one' },
  historyCacheRef: { current: [] }, historyEntriesRef: { current: [] }, historyHasMoreRef: { current: false },
  historyMoreBusyRef: { current: false }, historyRefreshSeqRef: { current: 0 }, loadHistorySeqRef: { current: 0 },
  historyLoadedCountRef: { current: 0 }, listRef: { current: { scrollHeight: 1000, scrollTop: 20 } }, preserveScrollRef: { current: null },
  window: { setTimeout: () => 0, clearTimeout: () => {} }, performance: { now: () => 0 }, console: { log: () => {}, error: () => {} },
  normalizeTimestamp: value => Number(value), toChatMessages: entry => [entry],
  setMessages: value => { visible = typeof value === 'function' ? value(visible) : value; },
  setHasMore: value => { more = value; }, setHistoryLoadedCount: value => { context.historyLoadedCountRef.current = value; },
  setLoadingMore: () => {}, setInitialLoading: () => {},
  invoke: async (name, args) => {
    assert.equal(name, 'get_chat_history'); assert.equal(args.limit, 21); assert.equal(args.channel, 'wechat'); calls.push(args);
    if (fail) throw new Error('temporary read failure');
    const end = args.beforeId ? data.findIndex(entry => entry.id === args.beforeId) : data.length;
    const rows = data.slice(Math.max(0, end - args.limit), end);
    if (deferred) return new Promise(resolve => { deferred.resolve = () => resolve(rows); });
    return rows;
  },
};
vm.createContext(context); vm.runInContext(code, context);
await context.loadHistory('one'); assert.equal(visible.length, 20); assert.equal(visible[0].id, '55'); assert.equal(more, true);
await context.loadMore(); assert.equal(visible.length, 40); assert.equal(calls.at(-1).beforeId, '55');
await context.loadMore(); await context.loadMore();
assert.equal(visible.length, 75); assert.equal(more, false); assert.equal(new Set(visible.map(entry => entry.id)).size, 75);
data.push({ ...data[0], id: 'new', content: 'new during history browsing', timestamp: 100 });
await context.refreshHistory(); assert.equal(visible.length, 76); assert.equal(more, false);
assert.equal(visible[0].id, '0', 'refresh retains already opened old history');

await context.loadHistory('one'); const previous = visible.map(entry => entry.id);
fail = true; await context.loadMore(); fail = false;
assert.deepEqual(visible.map(entry => entry.id), previous, 'failed old-page reads preserve the conversation and retry cursor');
assert.equal(more, true);

deferred = {}; const stale = context.loadMore(); const resolveStale = deferred;
deferred = undefined; context.privateCharIdRef.current = 'two'; context.privateCharId = 'two';
await context.loadHistory('two'); const current = visible.map(entry => entry.id);
resolveStale.resolve(); await stale;
assert.deepEqual(visible.map(entry => entry.id), current, 'an old character response cannot prepend records into the new character');
console.log('Companion history: real cursor requests, exhaustion, refresh retention, failed reads and character switching passed.');
