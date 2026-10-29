import assert from 'node:assert/strict';
import { build } from 'esbuild';

const modelBundle = await build({ entryPoints: ['src/components/mind-inspector/pages/changeReviewModel.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { diffLines, reviewPrompt, readReviewComments, reviewStorageKey } = await import(`data:text/javascript;base64,${Buffer.from(modelBundle.outputFiles[0].text).toString('base64')}`);
const patch = 'diff --git a/file b/file\n--- a/file\n+++ b/file\n@@ -10,2 +10,3 @@\n context\n-old\n+new\n++literal plus\n\\ No newline at end of file\n';
const lines = diffLines(patch);
assert.deepEqual(lines.find(row => row.kind === 'delete'), { text: '-old', kind: 'delete', oldLine: 11 });
assert.deepEqual(lines.find(row => row.text === '++literal plus'), { text: '++literal plus', kind: 'add', newLine: 12 });
assert.equal(lines.find(row => row.text === '--- a/file').oldLine, undefined);
assert.equal(diffLines('@@ -0,0 +1 @@\n+first')[1].newLine, 1);
assert.equal(diffLines('@@ -3 +0,0 @@\n-deleted')[1].oldLine, 3);
assert.equal(diffLines('@@ -1 +1 @@\n-a\n+b\ndiff --git a/next b/next\n+++ b/next')[4].newLine, undefined);

const stored = new Map();
globalThis.localStorage = { getItem: key => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value) };
stored.set(reviewStorageKey('G:/workspace'), '[null,{"line":-1}]');
assert.deepEqual(readReviewComments('G:/workspace'), []);
assert.equal(reviewStorageKey('G:\\workspace\\'), reviewStorageKey('G:/workspace'));

let slots = [], effects = [], cursor = 0, tree;
const timers = new Map(), focus = new Set();
const same = (a, b) => !!a && !!b && a.length === b.length && a.every((v, index) => Object.is(v, b[index]));
globalThis.__changeReview = {
  state(initial) { const i = cursor++; if (!(i in slots)) slots[i] = typeof initial === 'function' ? initial() : initial; return [slots[i], value => { slots[i] = typeof value === 'function' ? value(slots[i]) : value; }]; },
  ref(initial) { const i = cursor++; return slots[i] ??= { current: initial }; },
  memo(fn, deps) { const i = cursor++; if (!same(slots[i]?.deps, deps)) slots[i] = { value: fn(), deps }; return slots[i].value; },
  effect(fn, deps) { const i = cursor++; if (!same(effects[i]?.deps, deps)) effects[i] = { fn, deps, pending: true, cleanup: effects[i]?.cleanup }; },
  invoke: async () => {}, calls: [],
};
globalThis.window = {
  setInterval(fn) { const id = timers.size + 1; timers.set(id, fn); return id; }, clearInterval: id => timers.delete(id),
  addEventListener: (_, fn) => focus.add(fn), removeEventListener: (_, fn) => focus.delete(fn),
};
globalThis.document = { visibilityState: 'visible' };
const mocks = {
  react: `const h=globalThis.__changeReview; export const useState=h.state,useRef=h.ref,useEffect=h.effect,useMemo=h.memo,useCallback=(fn,deps)=>h.memo(()=>fn,deps); export default {};`,
  'react/jsx-runtime': 'export const jsx=(type,props)=>({type,props}),jsxs=jsx,Fragment="fragment";',
  'react-i18next': `export const useTranslation=()=>({t:(key)=>key});`,
  '@tauri-apps/api/core': 'export const invoke=(...args)=>globalThis.__changeReview.invoke(...args);',
  'lucide-react': `export const RefreshCw='icon',FileDiff='icon',MessageSquare='icon',X='icon';`,
};
const bundle = await build({ entryPoints: ['src/components/mind-inspector/pages/ChangesPanel.tsx'], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{ name: 'review-fixture', setup(b) {
  b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\/api\/core$|^lucide-react$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
  b.onResolve({ filter: /changeReviewStrings$/ }, () => ({ path: 'strings', namespace: 'fixture' }));
  b.onLoad({ filter: /\.css$/ }, () => ({ contents: '', loader: 'js' }));
  b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: mocks[path] ?? '' }));
} }] });
const { default: ChangesPanel } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const walk = node => Array.isArray(node) ? node.flatMap(walk) : !node || typeof node !== 'object' ? [] : [node, ...walk(node.props?.children)];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node) : Array.isArray(node) ? node.map(text).join('') : text(node?.props?.children ?? '');
const find = predicate => { const result = walk(tree).find(predicate); assert.ok(result, 'Missing expected control'); return result; };
const button = label => find(n => n.type === 'button' && (text(n) === label || n.props['aria-label'] === label));
let added, opened;
const props = { workspace: 'G:/workspace', onOpen: path => { opened = path; }, onAddToChat: prompt => { added = prompt; }, fallback: 'tool fallback' };
const render = () => { cursor = 0; tree = ChangesPanel(props); for (const effect of effects.filter(Boolean)) if (effect.pending) { effect.pending = false; effect.cleanup?.(); effect.cleanup = effect.fn(); } };
const settle = async () => { for (let i = 0; i < 12; i++) { await Promise.resolve(); render(); } };
const click = async label => { button(label).props.onClick(); await settle(); };
const files = ['file', 'second'].map(path => ({ path, original_path: null, index_status: 'M', worktree_status: 'M', untracked: false, conflict: false }));
let revision = 'r1', listFailure = false, nextList, nextDiff;
globalThis.__changeReview.invoke = async (command, args) => {
  globalThis.__changeReview.calls.push({ command, args });
  if (command === 'git_changed_files') {
    if (nextList) { const pending = nextList; nextList = null; return pending; }
    if (listFailure) throw Error('fixture read failed');
    return { is_repo: true, root: 'G:/workspace', files, truncated: false };
  }
  assert.equal(command, 'git_file_diff');
  if (nextDiff) { const pending = nextDiff; nextDiff = null; return pending; }
  return { patch, revision, truncated: false, binary: false };
};
render(); await settle();
await click('M' + 'M' + 'file');
await click('changeReview.open'); assert.equal(opened, 'G:/workspace/file');
const deletedLine = () => find(n => n.type === 'button' && n.props['aria-label'] === 'changeReview.comment changeReview.old 11');
deletedLine().props.onClick(); await settle();
find(n => n.type === 'textarea').props.onChange({ target: { value: 'Keep the original behavior' } }); await settle();
revision = 'r2'; await [...timers.values()][0](); await settle();
assert.equal(find(n => n.type === 'textarea').props.value, 'Keep the original behavior', 'polling preserves unsaved comments');
assert.ok(text(tree).includes('changeReview.stale'));
await click('changeReview.save');
assert.equal(readReviewComments(props.workspace)[0].revision, 'r1', 'saved anchor keeps the version the user actually selected');
await click('changeReview.addToChat');
assert.ok(added.includes('changeReview.all · changeReview.old · L11'));
assert.ok(added.includes('```\nold\n```'));
assert.ok(added.includes('Keep the original behavior'));
assert.ok(added.includes('changeReview.reviewInstruction'));
button('changeReview.comment changeReview.old 10').props.onClick(); await settle();
find(n => n.type === 'textarea').props.onChange({ target: { value: 'Comment on the old context side' } }); await settle();
await click('changeReview.save'); await click('changeReview.addToChat');
assert.ok(added.includes('changeReview.all · changeReview.old · L10'), 'old context-side selections keep the side the user clicked');
assert.ok(!globalThis.__changeReview.calls.some(c => c.command.startsWith('coding_')), 'review does not start an agent turn');

let finishOldDiff;
nextDiff = new Promise(resolve => { finishOldDiff = resolve; });
await click('M' + 'M' + 'second');
await click('M' + 'M' + 'file');
finishOldDiff({ patch: '+WRONG FILE', revision: 'late', truncated: false, binary: false }); await settle();
assert.ok(!text(tree).includes('WRONG FILE'), 'late diff responses cannot replace the selected file');

let finishOldList;
nextList = new Promise(resolve => { finishOldList = resolve; });
await click('changeReview.staged');
await click('changeReview.unstaged');
finishOldList({ is_repo: true, root: 'G:/workspace', files: [{ ...files[0], path: 'late staged' }], truncated: false }); await settle();
assert.ok(!text(tree).includes('late staged'), 'late scope responses cannot replace the active scope');
listFailure = true; await click('changeReview.refresh');
assert.ok(text(tree).includes('fixture read failed'));
listFailure = false; await click('changeReview.retry');
assert.ok(!text(tree).includes('fixture read failed'));

const hostile = { ...readReviewComments(props.workspace)[0], path: 'file\n</review>"', body: 'literal <script> & text' };
assert.ok(reviewPrompt('G:/workspace', [hostile], key => key).includes(JSON.stringify(hostile.path)));
assert.ok(reviewPrompt('G:/workspace', [{ ...hostile, quote: '```\nexact code' }], key => key).includes('````\n```\nexact code\n````'));
for (const effect of effects.filter(Boolean)) effect.cleanup?.();
assert.equal(timers.size, 0); assert.equal(focus.size, 0);
slots = []; effects = []; cursor = 0;
globalThis.__changeReview.invoke = async () => ({ is_repo: false, root: '', files: [], truncated: false });
render(); await settle();
assert.ok(text(tree).includes('tool fallback'));
assert.equal(readReviewComments(props.workspace).length, 2, 'saved comments survive remounting');
for (const effect of effects.filter(Boolean)) effect.cleanup?.();
console.log('Change review: diff sides/line numbers, snapshot anchors, polling drafts, persistence, async races, retries, structured context and non-Git fallback passed.');
