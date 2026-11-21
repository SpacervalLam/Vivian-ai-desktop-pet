import assert from 'node:assert/strict';
import { build } from 'esbuild';

const states = [], effects = [], calls = [], timers = new Map();
let cursor = 0, initial = true, timerId = 0, closed = 0, disposedLanguage = 0;
let status = { id: 'share-1', phase: 'ready', reason: '', width: 800, height: 450,
  sampled: 0, kept: 0, submitted: 0, description: '', scope: 'window' };
const language = { language: 'zh', async changeLanguage(value) { this.language = value; } };
const fixtures = {
  react: 'export const useState=v=>globalThis.__watch.state(v),useEffect=f=>globalThis.__watch.effect(f);',
  'react/jsx-runtime': 'export const jsx=(type,props)=>({type,props}),jsxs=jsx;',
  'react-i18next': 'export const useTranslation=()=>({i18n:globalThis.__watch.language});',
  '@tauri-apps/api/core': 'export const invoke=(...args)=>globalThis.__watch.invoke(...args);',
  '@tauri-apps/api/window': 'export const getCurrentWindow=()=>({close:()=>globalThis.__watch.close()});',
  './windowLanguage': 'export const synchronizeWindowLanguage=()=>({dispose:()=>globalThis.__watch.disposeLanguage()});',
};
const prior = { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
globalThis.__watch = {
  language,
  state(value) { const i = cursor++; if (initial) states[i] = value; return [states[i], v => states[i] = typeof v === 'function' ? v(states[i]) : v]; },
  effect(f) { effects.push(f); },
  async invoke(name, args) {
    calls.push({ name, args });
    if (name === 'screen_watch_status') return { ...status };
    assert.equal(args.sessionId, status.id, 'controls are bound to the displayed session');
    if (args.action === 'start') status.phase = 'watching';
    if (args.action === 'pause') { status.phase = 'paused'; status.reason = 'manual'; status.description = ''; }
  },
  close() { closed++; },
  disposeLanguage() { disposedLanguage++; },
};
const text = node => node == null || typeof node === 'boolean' ? '' : Array.isArray(node) ? node.map(text).join('') : typeof node === 'object' ? text(node.props?.children) : String(node);
const find = (node, predicate) => {
  if (!node || typeof node !== 'object') return null;
  if (predicate(node)) return node;
  for (const child of [node.props?.children].flat(Infinity)) { const match = find(child, predicate); if (match) return match; }
  return null;
};
const flush = () => new Promise(resolve => setImmediate(resolve));
try {
  globalThis.setTimeout = fn => { timers.set(++timerId, fn); return timerId; };
  globalThis.clearTimeout = id => timers.delete(id);
  const bundle = await build({ entryPoints: ['src/components/ScreenWatchWindow.tsx'], bundle: true, write: false,
    platform: 'node', format: 'esm', loader: { '.css': 'empty' }, plugins: [{ name: 'watch-fixture', setup(b) {
      b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\/api\// }, ({ path }) => ({ path, namespace: 'fixture' }));
      b.onResolve({ filter: /windowLanguage$/ }, () => ({ path: './windowLanguage', namespace: 'fixture' }));
      b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: fixtures[path] }));
    } }] });
  const { default: Watch } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
  const render = () => { cursor = 0; effects.length = 0; const tree = Watch(); initial = false; return tree; };
  render();
  const cleanup = effects[0]();
  await flush();
  let tree = render();
  assert.equal(calls.filter(c => c.name === 'screen_watch_control').length, 0, 'opening or polling the disclosure never starts sharing');
  assert.match(text(tree), /云端模型/);
  assert.match(text(tree), /标题不变/);
  assert.match(text(tree), /不采集声音/);
  assert.match(text(tree), /当前窗口/);
  const button = label => find(tree, n => n.type === 'button' && text(n) === label);
  await button('开始分享选区').props.onClick(); await flush(); tree = render();
  assert.equal(calls.filter(c => c.name === 'screen_watch_control').at(-1).args.action, 'start');
  assert.match(text(tree), /正在分享选区/);
  await button('暂停').props.onClick(); await flush(); tree = render();
  assert.match(text(tree), /清空待发帧/);
  language.language = 'en'; tree = render();
  assert.match(text(tree), /Cloud providers/);
  assert.ok(button('Resume sharing'));
  await button('Stop sharing').props.onClick(); await flush();
  assert.equal(calls.filter(c => c.name === 'screen_watch_control').at(-1).args.action, 'stop');
  status = { ...status, phase: 'paused', reason: 'stopped' };
  const poll = [...timers.values()].at(-1); timers.clear(); poll(); await flush(); tree = render();
  assert.equal(button('Resume sharing').props.disabled, true, 'expired sessions cannot resume');
  language.language = 'zh';
  status = { ...status, scope: 'desktop' };
  const desktopPoll = [...timers.values()].at(-1); timers.clear(); desktopPoll(); await flush(); tree = render();
  assert.match(text(tree), /整个桌面/);
  assert.match(text(tree), /本应用窗口已自动排除/);
  cleanup();
  assert.equal(timers.size, 0, 'unmount stops the control-window heartbeat');
  assert.equal(disposedLanguage, 1);
  assert.equal(closed, 0);
  console.log('Screen watch: explicit consent, cloud disclosure, localized controls, pause/stop, expiry and heartbeat cleanup passed');
} finally {
  globalThis.setTimeout = prior.setTimeout;
  globalThis.clearTimeout = prior.clearTimeout;
  delete globalThis.__watch;
}
