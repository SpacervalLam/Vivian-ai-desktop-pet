import assert from 'node:assert/strict';
import { build } from 'esbuild';

const states = [], refs = [], effects = [], listeners = new Map(), timers = new Map();
let stateIndex = 0, refIndex = 0, initial = true, timerId = 0, handle, clock = 0;
const store = { currentBubble: null, settledBubbles: [], dockState: 'Off' };
const saved = Object.fromEntries(['window', 'setTimeout', 'clearTimeout', 'performance'].map(key => [key, globalThis[key]]));
globalThis.__thinkingPet = {
  state(value) { const index = stateIndex++; if (initial) states[index] = typeof value === 'function' ? value() : value; return [states[index], value => { states[index] = typeof value === 'function' ? value(states[index]) : value; }]; },
  ref(value) { const index = refIndex++; if (initial) refs[index] = { current: value }; return refs[index]; },
  effect(fn) { effects.push(fn); }, store,
  handle(value) { handle = value; },
  async listen(name, callback) { listeners.set(name, callback); return () => listeners.delete(name); },
};
globalThis.setTimeout = (fn, ms) => { const id = ++timerId; timers.set(id, { fn, ms }); return id; };
globalThis.clearTimeout = id => timers.delete(id);
globalThis.performance = { now: () => clock };
globalThis.window = { setTimeout, clearTimeout, innerWidth: 300, innerHeight: 300, matchMedia: () => ({ matches: false }) };
const flush = () => new Promise(resolve => setImmediate(resolve));
const find = (tree, predicate) => {
  if (!tree || typeof tree !== 'object') return null;
  if (predicate(tree)) return tree;
  for (const child of [tree.props?.children].flat(Infinity)) { const result = find(child, predicate); if (result) return result; }
  return null;
};
try {
  const bundle = await build({ entryPoints: ['src/components/ChibiPetCanvas.tsx'], bundle: true, write: false, platform: 'node', format: 'esm', loader: { '.css': 'empty' }, plugins: [{ name: 'thinking-fixtures', setup(b) {
    b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$|\/useDesktopPhysics$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path === 'react' ? `export const forwardRef=f=>f,useState=v=>globalThis.__thinkingPet.state(v),useRef=v=>globalThis.__thinkingPet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__thinkingPet.effect(f),useImperativeHandle=(ref,create)=>globalThis.__thinkingPet.handle(create());` :
      path === 'react/jsx-runtime' ? `export const jsx=(type,props)=>({type,props}),jsxs=jsx;` :
      path.endsWith('characterContext') ? `export const getCharacterId=()=>globalThis.__thinkingPet.character;` :
      path.endsWith('useAppStore') ? `export const useAppStore=f=>f(globalThis.__thinkingPet.store);useAppStore.getState=()=>globalThis.__thinkingPet.store;` :
      path.endsWith('useDesktopPhysics') ? `export const useDesktopPhysics=()=>({airborne:false,error:false});` :
      path.endsWith('sheetLoader') ? `export class SheetLoader{load(){return Promise.resolve(true)}isReady(){return true}clear(){}}` :
      path.endsWith('/event') ? `export const listen=(...args)=>globalThis.__thinkingPet.listen(...args);` :
      path.endsWith('/window') ? `export const currentMonitor=async()=>null,getCurrentWindow=()=>({}),getAllWindows=async()=>[];` :
      `export const convertFileSrc=p=>p;export const invoke=async()=>null;`
    }));
  } }] });
  const { ChibiPetCanvas: Pet } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
  for (const character of ['vivian', 'nana']) {
  globalThis.__thinkingPet.character = character;
  states.length = refs.length = effects.length = 0;
  initial = true;
  const render = () => { stateIndex = refIndex = 0; effects.length = 0; const tree = Pet({ ambientMotionEnabled: false }, {}); initial = false; return tree; };
  const sprite = tree => find(tree, node => node.props?.className === 'chibi-pet-sprite');
  const pose = tree => find(tree, node => node.props?.className?.includes('chibi-pet-stage')).props.className;
  const frame = () => { const style = sprite(render()).props.style; const rows = parseFloat(style.backgroundSize.split(' ')[1]) / 100; const [x, y] = style.backgroundPosition.split(' ').map(parseFloat); return Math.round(y * (rows - 1) / 100) * 4 + Math.round(x / (100 / 3)); };
  const send = (name, payload) => listeners.get(name)({ payload: { character_id: character, ...payload } });
  const tick = () => {
    const entry = [...timers.entries()].find(([, timer]) => timer.ms < 1000);
    assert.ok(entry, 'animation has a pending frame');
    timers.delete(entry[0]); clock += entry[1].ms; entry[1].fn();
  };
  render();
  handle.startBusy();
  await flush();
  // Work enters through all twelve phone-taking frames before the loop.
  for (let i = 0; i < 12; i++) { tick(); await flush(); }
  render();
  const busyCleanup = effects.find(fn => fn.toString().includes('targetFrames'))();
  const busyFrames = [frame()];
  const busyDelays = [];
  for (let i = 0; i < 28; i++) {
    busyDelays.push([...timers.values()].find(timer => timer.ms < 1000).ms);
    tick(); busyFrames.push(frame());
  }
  const rising = Array.from({ length: 16 }, (_, i) => i);
  assert.deepEqual(busyFrames, [...rising, ...rising.slice(2, -1).reverse()], 'busy loops reverse smoothly without duplicating the endpoint');
  assert.deepEqual(busyDelays.slice(0, 16), [210,210,120,120,100,100,90,90,90,90,100,100,120,120,210,210], 'busy slows down and pauses at each turn');
  busyCleanup();
  assert.ok([...timers.values()].every(timer => timer.ms >= 1000), 'leaving busy cancels its pending frame');
  handle.stopBusy();
  await flush();
  for (let i = 0; i < 12; i++) { tick(); await flush(); }
  handle.resetExpression();
  render();
  const eventCleanup = effects.find(fn => fn.toString().includes('tts:started'))();
  await flush();

  const activity = (channel, character_id = character) => send('chat:channel_activity', { channel, character_id });
  const phoneTimers = () => [...timers.entries()].filter(([, timer]) => timer.ms === 8000);
  const enterPhone = async () => {
    activity('wechat');
    await flush();
    assert.match(pose(render()), /pose-busy-in/);
    for (let i = 0; i < 12; i++) { tick(); await flush(); }
    assert.match(pose(render()), /pose-busy-loop/);
  };
  const otherCharacter = character === 'nana' ? 'vivian' : 'nana';
  store.presenceState = 'edge';
  assert.match(pose(render()), /pose-idle/, 'busy presence alone does not start the phone animation');
  activity('wechat', otherCharacter);
  assert.equal(phoneTimers().length, 0);
  await enterPhone();
  const [[firstPhoneTimer]] = phoneTimers();
  activity('direct', otherCharacter);
  assert.ok(timers.has(firstPhoneTimer));
  activity('wechat');
  assert.equal(phoneTimers().length, 1);
  assert.ok(!timers.has(firstPhoneTimer), 'a newer message replaces the phone timeout');
  send('chat:waiting', { channel: 'wechat', stream_id: 'phone' });
  send('chat:start', { channel: 'wechat', stream_id: 'phone' });
  send('chat:done', { channel: 'wechat', stream_id: 'phone' });
  store.currentBubble = '手机消息';
  assert.match(pose(render()), /pose-busy-loop/, 'reply presentation keeps the phone sprite');
  store.currentBubble = null;
  activity('direct');
  assert.match(pose(render()), /pose-idle/, 'direct input immediately replaces the phone sprite');
  assert.equal(phoneTimers().length, 0);
  assert.equal(store.presenceState, 'edge', 'interrupting the phone animation preserves busy presence');
  send('chat:start', { channel: 'wechat', stream_id: 'late-phone-reply' });
  assert.match(pose(render()), /pose-idle/, 'late reply events cannot restore phone activity');

  store.presenceState = 'online';
  render();
  activity('wechat');
  activity('broadcast');
  await flush();
  assert.match(pose(render()), /pose-idle/, 'public speech also cancels a pending phone entry');
  await enterPhone();
  const [[phoneTimer, timeout]] = phoneTimers();
  timers.delete(phoneTimer);
  clock += timeout.ms;
  timeout.fn();
  await flush();
  assert.match(pose(render()), /pose-busy-in/, 'expiry starts the phone outro');
  for (let i = 0; i < 12; i++) { tick(); await flush(); }
  assert.match(pose(render()), /pose-idle/);
  assert.equal(store.presenceState, 'online', 'playing the phone animation preserves online presence');

  const start = id => {
    send('chat:waiting', { stream_id: id }); render();
    const cleanup = effects.find(fn => fn.toString().includes('targetFrames'))();
    assert.ok(sprite(render()).props.style.backgroundImage.includes(`${character}-think-sheet.webp`));
    return cleanup;
  };
  let cleanup = start('first');
  const frames = [frame()];
  for (let i = 0; i < 17; i++) { tick(); frames.push(frame()); }
  assert.deepEqual(frames, character === 'vivian'
    ? [0, 1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 4, 3, 4, 5, 6, 7]
    : [0, 1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 4, 3, 2, 3, 4, 5]);
  const loopStart = character === 'nana' ? 2 : 3;
  for (let i = 0; i < 23; i++) { tick(); assert.ok(frame() >= loopStart && frame() <= 8, 'thinking never reaches the outro before its end signal'); }
  for (let i = 0; frame() !== 8 && i < 12; i++) tick();
  assert.equal(frame(), 8, 'the loop reaches its upper endpoint');
  tick(); tick();
  const from = frame();
  assert.equal(from, 6, 'stop during the reverse half of the ping-pong loop');
  send('chat:waiting-ended', { stream_id: 'first' });
  assert.equal(frame(), from, 'ending does not jump to a different frame');
  store.currentBubble = '开始回复'; send('tts:started', {});
  for (let expected = from + 1; expected <= 11; expected++) {
    tick(); assert.equal(frame(), expected);
    assert.ok(sprite(render()).props.style.backgroundImage.includes(`${character}-think-sheet.webp`), 'speech cannot cover the outro');
  }
  tick(); assert.doesNotMatch(pose(render()), /pose-thinking/);
  cleanup(); store.currentBubble = null; send('presentation:stop', { speaker_id: character });

  cleanup = start('old'); tick();
  send('chat:error', { stream_id: 'old' }); tick();
  send('chat:waiting', { stream_id: 'new' });
  for (let i = 0; i < 25; i++) { tick(); assert.ok(frame() <= 8, 'a newer request cancels the pending outro'); }
  send('chat:done', { stream_id: 'old' });
  tick(); assert.match(pose(render()), /pose-thinking/, 'late old completion cannot end a new thought');
  send('chat:chunk', { stream_id: 'new', text: '回复' });
  for (let i = frame(); i < 12; i++) tick();
  assert.doesNotMatch(pose(render()), /pose-thinking/, 'first text also ends the loop naturally');
  cleanup();
  send('chat:waiting-ended', { stream_id: 'new' });

  cleanup = start('drag');
  handle.setExpression('drag');
  assert.match(pose(render()), /pose-drag/, 'dragging still takes over immediately');
  cleanup(); eventCleanup();
  assert.equal(listeners.size, 0);
  assert.equal(timers.size, 0, 'cleanup removes frame timers and the waiting watchdog');
  handle.resetExpression();
  const stopVertical = await handle.startVerticalMove();
  assert.equal(typeof stopVertical, 'function');
  render();
  const floatCleanup = effects.find(fn => fn.toString().includes('targetFrames'))();
  const positions = new Set();
  store.currentBubble = '升降时也可以说话';
  for (let i = 0; i < 8; i++) {
    const style = sprite(render()).props.style;
    assert.ok(style.backgroundImage.includes(`${character}-float-sheet.webp`), 'vertical movement keeps its own sheet during speech');
    positions.add(style.backgroundPosition);
    tick();
  }
  assert.equal(positions.size, 8, 'vertical movement plays all eight sprite frames');
  stopVertical(); floatCleanup(); store.currentBubble = null;
  assert.match(pose(render()), /pose-idle/, 'finishing vertical movement returns to idle');
  const staleStop = await handle.startVerticalMove();
  handle.setExpression('drag');
  staleStop();
  assert.match(pose(render()), /pose-drag/, 'an old movement cleanup cannot overwrite a user drag');
  console.log(`${character}: thinking playback, phone activity, speech protection, new-stream takeover and cleanup passed.`);
  }
} finally {
  for (const [key, value] of Object.entries(saved)) { if (value === undefined) delete globalThis[key]; else globalThis[key] = value; }
  delete globalThis.__thinkingPet;
}
