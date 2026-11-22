import assert from 'node:assert/strict';
import { build } from 'esbuild';

const states = [], refs = [], effects = [], timers = new Map();
let stateIndex = 0, refIndex = 0, initial = true, clock = 0, timerId = 0;
const saved = { window: globalThis.window, performance: globalThis.performance, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout, random: Math.random };
const store = { presenceState: 'online', dockState: 'Off', currentBubble: null, settledBubbles: [] };
const fixture = globalThis.__roamPet = {
  calls: [], idle: 0, position: { x: 880, y: 200 }, listeners: new Map(), emitted: [],
  state(value) { const i = stateIndex++; if (initial) states[i] = typeof value === 'function' ? value() : value; return [states[i], v => { states[i] = typeof v === 'function' ? v(states[i]) : v; }]; },
  ref(value) { const i = refIndex++; if (initial) refs[i] = { current: value }; return refs[i]; },
  effects, store,
};
globalThis.performance = { now: () => clock };
globalThis.setTimeout = (fn, ms) => { const id = ++timerId; timers.set(id, { fn, at: clock + ms }); return id; };
globalThis.clearTimeout = id => timers.delete(id);
globalThis.window = { setTimeout, clearTimeout, addEventListener() {}, removeEventListener() {}, matchMedia: () => ({ matches: false }) };
Math.random = () => 0.4;
const flush = async () => { for (let i = 0; i < 5; i++) await new Promise(resolve => setImmediate(resolve)); };
const advance = async target => {
  await flush();
  let count = 0;
  while (true) {
    const next = [...timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
    if (!next || next[1].at > target) break;
    assert.ok(++count < 5000, 'scheduler must not spin');
    clock = next[1].at; timers.delete(next[0]); next[1].fn(); await flush();
  }
  clock = target; await flush();
};
const find = (tree, name) => {
  if (!tree || typeof tree !== 'object') return null;
  if (tree.props?.className?.includes(name)) return tree;
  for (const child of [tree.props?.children].flat(Infinity)) { const result = find(child, name); if (result) return result; }
  return null;
};
try {
  const bundle = await build({ stdin: { contents: `export { ChibiPetCanvas } from './src/components/ChibiPetCanvas'; export { positioningCoordinator } from './src/hooks/positioningCoordinator'; export { peerApproachPosition } from './src/chibi/peerApproach';`, resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm', loader: { '.css': 'empty' }, plugins: [{ name: 'roam-fixture', setup(b) {
    b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$|\/useDesktopPhysics$|\/slideTrack$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path === 'react' ? `export const forwardRef=f=>f,useState=v=>globalThis.__roamPet.state(v),useRef=v=>globalThis.__roamPet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__roamPet.effects.push(f),useImperativeHandle=()=>{};` :
      path === 'react/jsx-runtime' ? `export const jsx=(type,props)=>({type,props}),jsxs=jsx;` :
      path.endsWith('characterContext') ? `export const getCharacterId=()=> 'nana';` :
      path.endsWith('useAppStore') ? `export const useAppStore=s=>s(globalThis.__roamPet.store);useAppStore.getState=()=>globalThis.__roamPet.store;` :
      path.endsWith('sheetLoader') ? `export class SheetLoader {load(){return Promise.resolve(true)} isReady(){return true} clear(){} }` :
      path.endsWith('useDesktopPhysics') ? `export const useDesktopPhysics=()=>({airborne:false,error:false});` :
      path.endsWith('slideTrack') ? `export const runSlide=async o=>{if(o.shouldAbort())return false;await o.apply(o.toX,o.toY);return true};` :
      path.endsWith('/event') ? `export const listen=async(name,handler)=>{const listeners=globalThis.__roamPet.listeners;listeners.set(name,handler);return()=>listeners.delete(name)};` :
      path.endsWith('/window') ? `
        const fixture=globalThis.__roamPet;
        export const currentMonitor=async()=>({scaleFactor:1,position:{x:0,y:0},size:{width:1000,height:1000},workArea:{position:{x:0,y:0},size:{width:1000,height:1000}}});
        export const getCurrentWindow=()=>({label:'nana',isVisible:async()=>true,outerPosition:async()=>fixture.position,outerSize:async()=>({width:100,height:100}),
          emit:async(name,payload)=>fixture.emitted.push({name,payload,position:{...fixture.position}}),
          onMoved:async callback=>{fixture.onMoved=callback;return ()=>{fixture.onMoved=undefined}}});
        export const getAllWindows=async()=>fixture.peer?[getCurrentWindow(),{label:'vivian',isVisible:async()=>true,outerPosition:async()=>fixture.peer,outerSize:async()=>({width:100,height:100})}]:[];` :
      `export const convertFileSrc=p=>p; export const invoke=async(name,args)=>{const f=globalThis.__roamPet;f.calls.push(name);if(name==='get_system_input_idle_seconds')return f.idle;if(name==='find_screen_interest_points')return {points:[]};if(name==='set_window_position')f.position=args;};`
    }));
  } }] });
  const { ChibiPetCanvas: Pet, positioningCoordinator: coordinator, peerApproachPosition } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
  let gravity = true;
  const render = () => {
    stateIndex = refIndex = 0; effects.length = 0;
    const tree = Pet({ ambientMotionEnabled: true, desktopPhysicsEnabled: gravity }, {});
    find(tree, 'chibi-pet-stage').props.ref.current = { getBoundingClientRect: () => ({ width: 94 }) };
    initial = false;
    return tree;
  };
  const pose = () => find(render(), 'chibi-pet-stage').props.className;
  const roamEffect = () => effects.find(f => f.toString().includes('new RoamRhythm'));
  coordinator.physicsEnabled = true; coordinator.physicsLane = { minX: 0, maxX: 900 };
  render(); let cleanup = roamEffect()();
  await advance(10_000);
  assert.match(pose(), /pose-lean/);
  assert.equal(fixture.position.x, 900, 'a short approach must still reach the physical monitor edge');
  assert.match(find(render(), 'chibi-pet-stage').props.style.transform, /translateX\(\d/,
    'the right-facing hand compensates for the transparent margin');
  const heldStyle = find(render(), 'chibi-pet-sprite').props.style;
  assert.equal(coordinator.ambientMoveInFlight, false, 'holding a lean must leave physics free to observe its support');
  await advance(50_000);
  assert.match(pose(), /pose-lean/, 'entrance completion must not end the held state');
  assert.deepEqual(find(render(), 'chibi-pet-sprite').props.style, heldStyle, 'lean stays on its final frame');
  await advance(72_000);
  assert.match(pose(), /pose-idle/, 'natural expiry plays stand-up and turn-back before returning to idle');
  cleanup();

  render(); cleanup = roamEffect()(); await advance(82_000);
  assert.match(pose(), /pose-lean/);
  cleanup(); await advance(82_200);
  assert.match(pose(), /pose-idle/, 'disabling the loop releases its held pose');
  assert.equal(timers.size, 0, 'cleanup cancels holding and pending input polling');
  assert.equal(fixture.onMoved, undefined, 'a cancelled lean releases its move listener');

  render(); cleanup = roamEffect()(); await advance(92_200);
  assert.match(pose(), /pose-lean/);
  coordinator.physicsInFlight = true;
  await advance(92_400);
  assert.match(pose(), /pose-idle/, 'loss of support interrupts the held state instead of suspending physics');
  coordinator.physicsInFlight = false; cleanup();

  render(); cleanup = roamEffect()(); await advance(102_400);
  assert.match(pose(), /pose-lean/);
  const sprite = find(render(), 'chibi-pet-sprite');
  sprite.props.onMouseDown({ button: 0, ctrlKey: false });
  coordinator.dragInFlight = true;
  await advance(102_600);
  assert.match(pose(), /pose-idle/, 'user takeover cannot be overwritten by a stale lean exit');
  sprite.props.onMouseUp({ button: 0 }); coordinator.dragInFlight = false; cleanup();

  render(); cleanup = roamEffect()(); await advance(110_000);
  assert.match(pose(), /pose-lean/);
  fixture.position = { x: 860, y: 200 };
  fixture.onMoved({ payload: fixture.position });
  await advance(110_200);
  assert.match(pose(), /pose-idle/, 'moving away from the wall ends the held pose');
  cleanup();

  coordinator.physicsLane = { minX: 800, maxX: 895 };
  render(); cleanup = roamEffect()(); await advance(120_000);
  assert.doesNotMatch(pose(), /pose-lean/, 'an interior platform endpoint cannot become a wall');
  cleanup();

  // Same component: a newly idle desktop bypasses a long non-gravity startup wait.
  gravity = false; fixture.idle = 60; coordinator.physicsEnabled = false; coordinator.physicsLane = null;
  fixture.calls.length = 0;
  render(); cleanup = roamEffect()(); await advance(125_400);
  assert.ok(fixture.calls.includes('get_system_input_idle_seconds'), 'the loop reads system-wide input');
  assert.ok(fixture.calls.includes('set_window_position'), 'an idle desktop walks within seconds in non-gravity mode too');
  cleanup(); await advance(133_000);
  assert.equal(coordinator.ambientMoveInFlight, false);

  fixture.position = { x: 100, y: 310 };
  fixture.peer = { x: 560, y: 300 };
  fixture.calls.length = 0;
  const workArea = { x: 0, y: 0, width: 1000, height: 1000, scale: 1 };
  const approach = { stream_id: 'round-1', source_id: 'nana', target_id: 'vivian', work_area: workArea };
  const send = (name, payload) => fixture.listeners.get(name)({ payload });
  const body = (id, position) => ({ id, ...position, width: 100, height: 100 });
  const beside = peerApproachPosition(body('nana', fixture.position), body('vivian', fixture.peer), [], workArea);
  assert.ok(beside.x < fixture.peer.x && beside.y === fixture.peer.y);
  assert.ok(peerApproachPosition(body('nana', { x: -100, y: 310 }), body('vivian', { x: 3, y: 300 }), [], workArea).x > 3,
    'use the other side when the preferred side is outside the monitor');
  assert.ok(peerApproachPosition(body('nana', fixture.position), body('vivian', fixture.peer), [body('claude', beside)], workArea).x > fixture.peer.x,
    'leave room for other pets at the destination');

  render();
  const cleanEvents = effects.find(f => f.toString().includes('cross:approach-complete'))();
  cleanup = roamEffect()();
  await flush();
  const startedAt = clock;
  send('cross:approach', approach);
  assert.equal(coordinator.peerConversationInProgress, true);
  assert.equal(fixture.emitted.length, 0, 'speech cannot start before the walk completes');
  await advance(startedAt + 1_000);
  assert.match(pose(), /pose-walk/, 'the approach uses the actual walking pose');
  await advance(startedAt + 14_000);
  assert.deepEqual(fixture.position, beside);
  assert.deepEqual(fixture.emitted.at(-1), { name: 'cross:approach-complete', payload: { stream_id: 'round-1', character_id: 'nana' }, position: beside });
  const facingStyle = () => find(render(), 'chibi-pet-sprite').props.style;
  const sideOn = facingStyle();
  assert.match(sideOn.backgroundImage, /nana-turn-right-sheet/);
  assert.equal(sideOn.backgroundPosition, '100% 100%', 'arriving from the left holds the last right-turn frame');
  send('cross:start', { stream_id: 'round-1', speaker_id: 'nana', listener_id: 'vivian' });
  await advance(clock + 250);
  assert.match(pose(), /is-peer-speaking/);
  assert.deepEqual(facingStyle(), sideOn, 'the opening line keeps the sideways drawing');
  send('cross:chunk', { stream_id: 'round-1', speaker_id: 'vivian', listener_id: 'nana', text: '你好' });
  assert.doesNotMatch(pose(), /is-peer-speaking/);
  assert.deepEqual(facingStyle(), sideOn, 'listening keeps the same drawing');
  send('cross:done', { stream_id: 'round-1', speaker_id: 'vivian', listener_id: 'nana', should_continue: true });
  send('cross:speech-finished', { stream_id: 'round-1', character_id: 'nana' });
  fixture.calls.length = 0;
  await advance(clock + 10_000);
  assert.equal(coordinator.peerConversationInProgress, true, 'hold through the peer’s remaining audio');
  assert.ok(!fixture.calls.includes('set_window_position'), 'no autonomous position changes during the exchange');
  send('cross:speech-finished', { stream_id: 'round-1', character_id: 'vivian' });

  send('cross:approach', { ...approach, stream_id: 'round-2', source_id: 'vivian', target_id: 'nana' });
  send('cross:prepare-listener', { stream_id: 'round-2' });
  send('cross:start', { stream_id: 'round-2', speaker_id: 'vivian', listener_id: 'nana' });
  await advance(clock + 4_000);
  assert.equal(coordinator.peerConversationInProgress, true, 'listener holds through the previous round’s timeout');
  assert.deepEqual(facingStyle(), sideOn, 'continuations do not turn the same pet twice');
  assert.ok(!fixture.calls.includes('set_window_position'), 'the receiving role never approaches the source');
  send('cross:error', { stream_id: 'round-2' });
  assert.equal(coordinator.peerConversationInProgress, false);
  await advance(clock + 200);
  assert.match(pose(), /pose-idle/, 'the last exchange turns back to idle');
  await advance(clock + 14_000);
  assert.ok(fixture.calls.includes('set_window_position'), 'autonomous roaming resumes after the exchange');
  cleanup(); await advance(clock + 900);

  fixture.position = { x: 630, y: 300 };
  send('cross:approach', { ...approach, stream_id: 'listener', source_id: 'vivian', target_id: 'nana' });
  send('cross:prepare-listener', { stream_id: 'listener' });
  await advance(clock + 40);
  assert.match(pose(), /pose-think/, 'the reaction happens after arrival and before turning');
  assert.notEqual(fixture.emitted.at(-1)?.payload.stream_id, 'listener');
  await advance(clock + 2_000);
  assert.equal(fixture.emitted.at(-1)?.payload.stream_id, 'listener', 'ready is sent after the turn and reaction');
  send('cross:start', { stream_id: 'listener', speaker_id: 'vivian', listener_id: 'nana' });
  await advance(clock + 200);
  assert.match(facingStyle().backgroundImage, /nana-turn-left-sheet/, 'the pet on the right faces left');
  assert.doesNotMatch(pose(), /is-peer-speaking/);
  assert.deepEqual(fixture.position, { x: 630, y: 300 });
  send('cross:chunk', { stream_id: 'listener', speaker_id: 'nana', listener_id: 'vivian', text: '你好' });
  assert.match(pose(), /is-peer-speaking/, 'the listener replies while still facing the other pet');
  send('cross:done', { stream_id: 'listener', speaker_id: 'nana', listener_id: 'vivian' });
  send('cross:speech-finished', { stream_id: 'listener', character_id: 'nana' });
  send('cross:speech-finished', { stream_id: 'listener', character_id: 'vivian' });
  await advance(clock + 200);
  assert.match(pose(), /pose-idle/);

  Math.random = () => 0.5;
  send('cross:approach', { ...approach, stream_id: 'direct-turn', source_id: 'vivian', target_id: 'nana' });
  send('cross:prepare-listener', { stream_id: 'direct-turn' });
  await advance(clock + 40);
  assert.match(pose(), /pose-turn/, 'the other half of openings turn immediately');
  await advance(clock + 200);
  assert.equal(fixture.emitted.at(-1)?.payload.stream_id, 'direct-turn');
  send('cross:error', { stream_id: 'direct-turn' });
  await advance(clock + 200);

  send('cross:approach', { ...approach, stream_id: 'interrupted' });
  send('cross:start', { stream_id: 'interrupted', speaker_id: 'nana', listener_id: 'vivian' });
  send('cross:approach-cancelled', { stream_id: 'interrupted' });
  assert.equal(coordinator.peerConversationInProgress, false, 'cancelled approaches release movement ownership');
  await advance(clock + 250);
  assert.match(pose(), /pose-idle/, 'late geometry cannot restore a cancelled turn');
  cleanEvents();
  console.log('Pet roam execution: input sampling, fast idle startup, held final frame, physics freedom, natural exit and cleanup passed');
} finally {
  globalThis.window = saved.window; globalThis.performance = saved.performance;
  globalThis.setTimeout = saved.setTimeout; globalThis.clearTimeout = saved.clearTimeout; Math.random = saved.random;
  delete globalThis.__roamPet;
}
