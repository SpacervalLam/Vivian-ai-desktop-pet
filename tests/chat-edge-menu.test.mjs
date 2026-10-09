import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

const effects = [], updates = [], calls = [], frames = new Map();
let frameId = 0, reducedMotion = false;
const globals = ['window', 'requestAnimationFrame', 'cancelAnimationFrame'];
const previous = Object.fromEntries(globals.map(name => [name, globalThis[name]]));
Object.assign(globalThis, {
  window: { innerWidth: 380, matchMedia: () => ({ matches: reducedMotion }) },
  requestAnimationFrame: fn => { const id = ++frameId; frames.set(id, fn); return id; },
  cancelAnimationFrame: id => frames.delete(id),
  __edgeEffects: effects, __edgeUpdates: updates, __edgeCalls: calls,
});
const bundled = await build({
  stdin: { contents: `export { default as Menu } from './src/components/ChatEdgeMenu';`, resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, platform: 'node', format: 'esm', loader: { '.css': 'empty' },
  plugins: [{ name: 'fixtures', setup(builder) {
    builder.onResolve({ filter: /^react$|^react\/jsx-runtime$|^lucide-react$|^react-i18next$|^@tauri-apps\/api\// }, args => ({ path: args.path, namespace: 'fixture' }));
    builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path === 'react' ? `export const useRef=value=>({current:value}), useState=value=>[value,v=>globalThis.__edgeUpdates.push(v)], useCallback=fn=>fn, useEffect=fn=>globalThis.__edgeEffects.push(fn), useLayoutEffect=useEffect;` :
      path === 'react/jsx-runtime' ? `export const jsx=(type,props)=>({type,props}),jsxs=jsx;` :
      path === 'lucide-react' ? `export const Dice5=()=>null,MessageCircle=()=>null,NotebookPen=()=>null,Rocket=()=>null,ScanLine=()=>null,BriefcaseBusiness=()=>null,Moon=()=>null;` :
      path === 'react-i18next' ? `export const useTranslation=()=>({i18n:{language:'zh'}});` :
      `export const invoke=async(...args)=>{globalThis.__edgeCalls.push(args);},emit=async(...args)=>{globalThis.__edgeCalls.push(args);},listen=async()=>()=>{};`
    }));
  } }],
});
try {
  const { Menu } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
  const menuState = { mode: 'menu', width: 48 };
  const fold = { mode: 'folding', width: 48 };
  let action;
  const rendered = Menu({ edge: menuState, target: { current: null }, onAction: async value => { action = value; }, onFoldEnd() {} });
  assert.equal(rendered.props.style.width, 48, 'physical 60px maps to logical 48px at 125% DPI');
  const buttons = rendered.props.children.props.children;
  for (const button of buttons) {
    button.props.onClick();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(action, button.props.title === '截屏分析' ? 'screen' : ({ 办公: 'office', 勿扰: 'dnd', 随手记: 'notes', 小游戏: 'games', 快捷启动: 'shortcuts', 打开聊天窗口: 'chat' })[button.props.title]);
  }
  assert.equal(calls.length, 0, 'the window shell owns readiness after theme initialization');
  effects.length = 0;
  let finished = 0, keyframes;
  let completeAnimation;
  const target = { current: { getBoundingClientRect: () => ({ left: 340, top: 64, width: 36, height: 36 }), animate: () => {} } };
  const folding = Menu({ edge: fold, target, onAction: async () => {}, onFoldEnd: () => finished++ });
  folding.props.children.props.ref.current = {
    getBoundingClientRect: () => ({ left: 0, top: 200, width: 48, height: 354 }),
    animate: frames => { keyframes = frames; return { finished: new Promise(resolve => { completeAnimation = resolve; }), cancel() {} }; },
  };
  const finishCleanup = effects[0]();
  assert.ok(keyframes[1].transform.includes('translate(334px, -295px)'), 'fold ends exactly at the actual three-dot button center');
  completeAnimation(); await new Promise(resolve => setImmediate(resolve));
  assert.equal(finished, 1); finishCleanup();
  reducedMotion = true; effects[0](); assert.equal(finished, 2, 'reduced motion immediately restores the full window');

  effects.length = 0;
  const active = Menu({ edge: menuState, target: { current: null }, quiet: true, onAction: async () => {}, onFoldEnd() {} });
  const activeButtons = active.props.children.props.children;
  assert.equal(activeButtons.length, 7);
  assert.deepEqual(activeButtons.filter(button => button.props.className.includes('is-active')).map(button => button.props.title), ['勿扰']);
  assert.equal(activeButtons[0].props['aria-pressed'], undefined, 'chat is an action, never a selected toggle');
  console.log('Edge menu: shortcuts, DPI width, exact fold target, reduced motion and shared screenshot flow passed');
} finally {
  for (const name of globals) { if (previous[name] === undefined) delete globalThis[name]; else globalThis[name] = previous[name]; }
  for (const name of ['__edgeEffects', '__edgeUpdates', '__edgeCalls']) delete globalThis[name];
}
