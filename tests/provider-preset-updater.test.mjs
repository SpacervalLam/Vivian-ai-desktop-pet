import assert from 'node:assert/strict';
import { build } from 'esbuild';

let slots = [], effects = [], cursor = 0, tree, handler, calls = [], invalidated = 0, unlistened = 0;
const timers = new Map();
const same = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
globalThis.__presetUI = {
  state(initial) { const i = cursor++; if (!(i in slots)) slots[i] = initial; return [slots[i], value => { slots[i] = typeof value === 'function' ? value(slots[i]) : value; }]; },
  ref(initial) { const i = cursor++; return slots[i] ??= { current: initial }; },
  effect(fn, deps) { const i = cursor++; if (!same(effects[i]?.deps, deps)) effects[i] = { fn, deps, pending: true, cleanup: effects[i]?.cleanup }; },
  invoke: (...args) => invoke(...args),
  listen: async (event, callback) => { assert.equal(event, 'provider-presets:update'); handler = callback; return () => { handler = undefined; unlistened++; }; },
  invalidate: () => { invalidated++; },
  open: async url => { calls.push({ command: 'open_source', args: url }); },
};
globalThis.window = {
  setInterval(fn) { const id = timers.size + 1; timers.set(id, fn); return id; },
  clearInterval(id) { timers.delete(id); },
};
const mocks = {
  react: `const h=globalThis.__presetUI;export const useState=h.state,useRef=h.ref,useEffect=h.effect;`,
  'react/jsx-runtime': `export const jsx=(type,props)=>({type,props});export const jsxs=jsx;`,
  'react-i18next': `export const useTranslation=()=>({i18n:{language:'zh'},t:(key,args)=>key+(args?JSON.stringify(args):'')});`,
  '@tauri-apps/api/core': `export const invoke=(...args)=>globalThis.__presetUI.invoke(...args);`,
  '@tauri-apps/api/event': `export const listen=(...args)=>globalThis.__presetUI.listen(...args);`,
  '@tauri-apps/plugin-shell': `export const open=(url)=>globalThis.__presetUI.open(url);`,
  ModelSettings: `export const invalidateProviderPresetsCache=()=>globalThis.__presetUI.invalidate();`,
};
const bundle = await build({ entryPoints: ['src/components/settings/ProviderPresetUpdater.tsx'], bundle: true,
  write: false, platform: 'node', format: 'esm', plugins: [{ name: 'preset-ui-fixtures', setup(b) {
    b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\// }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onResolve({ filter: /ModelSettings$/ }, () => ({ path: 'ModelSettings', namespace: 'fixture' }));
    b.onLoad({ filter: /\.css$/ }, () => ({ contents: '', loader: 'js' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: mocks[path] }));
  } }] });
const Component = (await import('data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64'))).default;
const walk = node => Array.isArray(node) ? node.flatMap(walk) : node && typeof node === 'object' ? [node, ...walk(node.props?.children)] : [];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node) : Array.isArray(node) ? node.map(text).join('') : text(node?.props?.children ?? '');
const find = predicate => { const node = walk(tree).find(predicate); assert.ok(node, 'Expected UI control'); return node; };
const button = key => find(n => n.type === 'button' && text(n) === 'config.preset_update.' + key);
const render = () => {
  cursor = 0; tree = Component({ charId: 'vivian' });
  for (const effect of effects.filter(Boolean)) if (effect.pending) { effect.pending = false; effect.cleanup?.(); effect.cleanup = effect.fn(); }
};
const settle = async () => { for (let i = 0; i < 30; i++) { await Promise.resolve(); render(); } };
const click = async key => { assert.equal(!!button(key).props.disabled, false); button(key).props.onClick(); await settle(); };
const emit = async job => { handler({ payload: structuredClone(job) }); await settle(); };
const unmount = async () => { for (const effect of effects.filter(Boolean)) effect.cleanup?.(); await Promise.resolve(); };
const target = id => ({ id, label: id.toUpperCase(), labelKey: null, modelCount: 1, verifiedAt: null, verifiedSource: null });
let providers = [target('openai'), target('deepseek')], job = null, failStart = false, holdStart, sequence = 0;
const makeJob = (ids, id = `job-${++sequence}`) => ({ id, revision: 1, running: true, stopRequested: false, startedAt: '2026-10-27T10:00:00Z', finishedAt: null,
  items: ids.map(id => ({ id, label: id, status: 'queued', message: null, sessionId: null, source: null, added: 0, removed: 0, modelCount: 1 })) });
async function invoke(command, args) {
  calls.push({ command, args });
  if (command === 'provider_preset_update_info') return structuredClone({ providers, job });
  if (command === 'provider_preset_update_status') return structuredClone(job);
  if (command === 'start_provider_preset_update') {
    if (failStart) throw Error('work model unavailable');
    job = makeJob(args.providerIds);
    if (holdStart) await holdStart;
    return structuredClone(job);
  }
  if (command === 'stop_provider_preset_update') { assert.equal(args.jobId, job.id); job.stopRequested = true; job.revision++; return; }
  throw Error('Unexpected command: ' + command);
}
render(); await settle();
assert.equal(button('selected').props.disabled, true);
await click('all');
assert.deepEqual(calls.find(c => c.command === 'start_provider_preset_update').args,
  { charId: 'vivian', providerIds: ['openai', 'deepseek'], language: 'zh' });
assert.equal(button('all').props.disabled, true);
assert.equal(invalidated, 0, 'Starting is not a successful update');
job.items[0] = { ...job.items[0], status: 'updated', added: 1, modelCount: 2, source: 'https://openai.com/docs' };
job.items[1].status = 'running'; job.revision++;
providers[0] = { ...providers[0], modelCount: 2, verifiedAt: '2026-10-27', verifiedSource: 'https://openai.com/docs' };
await emit(job);
assert.equal(invalidated, 1, 'A committed vendor refreshes model suggestions');
assert.ok(text(tree).includes('+1 / −0'));
await emit({ ...job, revision: 1, items: makeJob(['openai', 'deepseek']).items });
assert.ok(text(tree).includes('+1 / −0'), 'Stale events do not undo visible results');
await click('stop');
assert.ok(text(tree).includes('config.preset_update.stop_hint'));
assert.equal(button('stopping').props.disabled, true);
job.running = false; job.revision++; job.items[1].status = 'stopped';
await emit(job);
assert.ok(text(tree).includes('config.preset_update.stopped'));
assert.equal(invalidated, 1, 'Polling and final events do not repeat cache invalidation');
await click('source');
assert.ok(calls.some(c => c.command === 'open_source' && c.args === 'https://openai.com/docs'));

find(n => n.type === 'select').props.onChange({ target: { value: 'deepseek' } }); render();
let release; holdStart = new Promise(resolve => { release = resolve; });
button('selected').props.onClick(); button('selected').props.onClick(); await settle();
assert.equal(calls.filter(c => c.command === 'start_provider_preset_update').length, 2, 'Double click submits once');
assert.deepEqual(calls.filter(c => c.command === 'start_provider_preset_update').at(-1).args.providerIds, ['deepseek']);
release(); holdStart = undefined; await settle();
job.running = false; job.revision++; job.items[0].status = 'failed'; job.items[0].message = 'Official source could not be read';
await emit(job);
assert.ok(text(tree).includes('config.preset_update.partial'));
assert.ok(text(tree).includes('Official source could not be read'));
assert.equal(invalidated, 1, 'Failures never refresh as successes');
failStart = true; await click('selected');
assert.ok(text(find(n => n.props?.role === 'alert')).includes('work model unavailable'));
assert.equal(button('all').props.disabled, false);
assert.ok(!calls.some(c => ['set_config', 'save_config', 'select_work_model'].includes(c.command)), 'Preset updates never save current configuration');
await unmount(); assert.equal(timers.size, 0); assert.equal(unlistened, 1);

// Reopening Settings restores progress and can receive results without starting another job.
slots = []; effects = []; cursor = 0; failStart = false;
job = makeJob(['deepseek'], 'job-restored'); job.startedAt = '2026-10-27T11:00:00Z';
render(); await settle();
assert.equal(button('all').props.disabled, true);
job.running = false; job.revision++; job.items[0].status = 'verified'; job.items[0].source = 'https://api-docs.deepseek.com/';
for (const tick of timers.values()) tick(); await settle();
assert.ok(text(tree).includes('config.preset_update.done'));
assert.equal(invalidated, 2);
await unmount(); assert.equal(timers.size, 0); assert.equal(unlistened, 2);
console.log('Provider preset updater: all/single vendor, receipts, stale events, stop, failures, cache refresh, restore and cleanup passed.');
