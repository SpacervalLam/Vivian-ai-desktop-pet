import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const source = readFileSync(new URL('../src/components/mind-inspector/pages/WorkspaceFileTree.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const start = source.indexOf('  useEffect(() => {\n    if (!root) return;');
assert.ok(start > 0);
const end = source.indexOf('  const input =', start);
const code = ts.transpileModule(source.slice(start, end), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
const tick = () => new Promise(resolve => setImmediate(resolve));

function mount({ read = async () => [], register } = {}) {
  const state = { roots: null, error: '', loading: new Set(), expanded: new Set(['/workspace', '/workspace/src']) };
  const depthCache = { '/workspace/closed': [{ name: 'stale' }] };
  const listeners = new Map();
  const calls = [];
  let timer;
  let cleanup;
  let channel;
  const window = {
    setTimeout(callback, delay) { assert.equal(delay, 100); timer = callback; return 1; },
    clearTimeout() { timer = undefined; },
    addEventListener(name, callback) { listeners.set(name, callback); },
    removeEventListener(name) { listeners.delete(name); },
  };
  const document = { ...window, visibilityState: 'visible' };
  class Channel { constructor() { channel = this; } }
  const invoke = async (command, args) => {
    calls.push({ command, ...args });
    if (command === 'coding_watch_directories') return register ? register() : 'watch-id';
    if (command === 'coding_unwatch_directories') return;
    return read(args.directory);
  };
  const run = new Function('useEffect', 'root', 'expanded', 'depthCache', 'invoke', 'setRoots', 'setError', 'setLoading', 'window', 'document', 'Channel', 'setExpanded', code);
  run(effect => { cleanup = effect(); }, '/workspace', new Set(['/workspace', '/workspace/src']), depthCache, invoke,
    value => { state.roots = value; }, value => { state.error = value; }, value => { state.loading = value; }, window, document, Channel, update => { state.expanded = update(state.expanded); });
  return { state, depthCache, listeners, document, calls,
    event: path => channel.onmessage(path),
    flush: () => { const callback = timer; timer = undefined; callback?.(); },
    hasTimer: () => timer !== undefined,
    cleanup: () => cleanup() };
}

let files = [{ name: 'old' }];
const tree = mount({ read: async () => files });
await tick();
assert.equal(tree.calls[0].command, 'coding_watch_directories');
assert.deepEqual(tree.calls.filter(x => x.command === 'coding_list_dir_tree').map(x => x.directory), ['/workspace', '/workspace/src']);
assert.equal(tree.depthCache['/workspace/closed'], undefined);
assert.equal(tree.hasTimer(), false, 'idle tree must not schedule reads');
files = [{ name: 'renamed' }, { name: 'new' }];
const before = tree.calls.length;
for (let i = 0; i < 20; i++) tree.event('/workspace/src');
tree.flush();
await tick();
assert.equal(tree.calls.length, before + 1, 'burst must refresh only the changed directory once');
assert.deepEqual(tree.depthCache['/workspace/src'], files);
assert.deepEqual(tree.state.roots, [{ name: 'old' }]);
tree.document.visibilityState = 'hidden';
tree.event('/workspace');
tree.flush();
await tick();
assert.equal(tree.calls.length, before + 1);
tree.document.visibilityState = 'visible';
files = [];
tree.listeners.get('visibilitychange')();
tree.flush();
await tick();
assert.deepEqual(tree.state.roots, []);
const idle = tree.calls.length;
tree.listeners.get('focus')();
tree.flush();
assert.equal(tree.calls.length, idle, 'focus without pending changes must not read');
tree.event('');
tree.flush();
await tick();
assert.equal(tree.calls.length, idle + 2, 'rescan notification refreshes all watched directories');
tree.cleanup();
assert.equal(tree.listeners.size, 0);
assert.equal(tree.calls.at(-1).command, 'coding_unwatch_directories');

const pending = [];
const stale = mount({ read: () => new Promise(resolve => pending.push(resolve)) });
await tick();
stale.event('/workspace');
stale.flush();
assert.equal(pending.length, 2, 'refreshes must not overlap');
stale.cleanup();
pending.forEach(resolve => resolve([{ name: 'old-workspace-result' }]));
await tick();
assert.equal(stale.state.roots, null);
assert.equal(stale.depthCache['/workspace/src'], undefined);
const queued = [];
const busy = mount({ read: () => new Promise(resolve => queued.push(resolve)) });
await tick();
busy.event('/workspace/src');
busy.flush();
queued.splice(0).forEach(resolve => resolve([]));
await tick();
assert.equal(busy.hasTimer(), true, 'events arriving during a read must be retained');
busy.flush();
assert.equal(queued.length, 1);
queued.splice(0).forEach(resolve => resolve([]));
await tick();
busy.cleanup();
const deleted = mount({ read: async () => [] });
await tick();
assert.deepEqual([...deleted.state.expanded], ['/workspace'], 'deleted expanded directories must release their expansion/watch state');
deleted.cleanup();
let resolveRegistration;
const late = mount({ register: () => new Promise(resolve => { resolveRegistration = resolve; }) });
late.cleanup();
resolveRegistration('late-watch');
await tick();
assert.equal(late.calls.at(-1).command, 'coding_unwatch_directories');
assert.equal(late.calls.at(-1).id, 'late-watch');
console.log('Workspace tree event refresh tests passed');
