import assert from 'node:assert/strict';
import { build } from 'esbuild';

const compile = async (path, plugins = []) => {
  const result = await build({ entryPoints: [path], bundle: true, write: false, platform: 'node', format: 'esm', plugins });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
};
const { gamePosition } = await compile('src/chibi/gamePosition.ts');
for (const scale of [1, 1.5, 2]) {
  const area = { x: -1920 * scale, y: -200 * scale, width: 1920 * scale, height: 1080 * scale };
  const size = { width: 213 * scale, height: 240 * scale };
  for (const cursor of [{ x: area.x, y: area.y }, { x: -500, y: 700 }, { x: -200, y: -100 }]) {
    for (const slot of [0, 1]) {
      const p = gamePosition(area, size, cursor, slot);
      assert.ok(p.x >= area.x && p.y >= area.y);
      assert.ok(p.x + size.width <= area.x + area.width && p.y + size.height <= area.y + area.height);
      assert.ok(p.x < area.x + area.width / 2 || p.y > area.y + area.height / 2, 'top-right close controls stay clear');
    }
    const a = gamePosition(area, size, cursor, 0), b = gamePosition(area, size, cursor, 1);
    assert.ok(a.x + size.width <= b.x || b.x + size.width <= a.x || a.y + size.height <= b.y || b.y + size.height <= a.y,
      'two pets get separate edge positions');
  }
}

const prior = { setTimeout, clearTimeout, dateNow: Date.now };
const timers = new Map(), effects = [], listeners = new Map();
let timerId = 0, time = 10000, game = { active: true, process: 'cs2', revision: 2 };
let position = { x: 500, y: 350 }, cursor = { x: 600, y: 450 }, visible = true, moves = [], passthrough = [];
const flush = async () => { for (let i = 0; i < 8; i++) await new Promise(resolve => setImmediate(resolve)); };
globalThis.__gameFixture = {
  state: initial => [game ?? initial, next => { game = typeof next === 'function' ? next(game) : next; }],
  effect: fn => effects.push(fn),
  listen: async (name, callback) => { listeners.set(name, callback); return () => listeners.delete(name); },
  invoke: async name => name === 'companion_status' ? { game: { active: true, process: 'cs2', revision: 1 } } : cursor,
  win: {
    outerPosition: async () => ({ ...position }), outerSize: async () => ({ width: 213, height: 240 }),
    isVisible: async () => visible, isFocused: async () => false,
    setPosition: async next => { position = { x: next.x, y: next.y }; moves.push(position); },
    setIgnoreCursorEvents: async active => { passthrough.push(active); },
  },
};
try {
  globalThis.setTimeout = (fn, ms) => { const id = ++timerId; timers.set(id, { fn, ms }); return id; };
  globalThis.clearTimeout = id => timers.delete(id);
  Date.now = () => time;
  const { useGameCompanion } = await compile('src/hooks/useGameCompanion.ts', [{ name: 'game-fixtures', setup(b) {
    b.onResolve({ filter: /^react$|^@tauri-apps\/api\/|\/characterContext$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path === 'react' ? 'export const useState=v=>globalThis.__gameFixture.state(v),useEffect=f=>globalThis.__gameFixture.effect(f);' :
      path.endsWith('characterContext') ? 'export const getCharacterId=()=>"vivian";' :
      path.endsWith('/event') ? 'export const listen=(...args)=>globalThis.__gameFixture.listen(...args);' :
      path.endsWith('/window') ? 'export const getCurrentWindow=()=>globalThis.__gameFixture.win;export const currentMonitor=async()=>({position:{x:0,y:0},size:{width:1920,height:1080}});export class PhysicalPosition{constructor(x,y){this.x=x;this.y=y;}}' :
      'export const invoke=(...args)=>globalThis.__gameFixture.invoke(...args);'
    }));
  } }]);
  useGameCompanion(true);
  const unsubscribe = effects[0]();
  await flush();
  assert.equal(game.revision, 2, 'startup query cannot overwrite a newer mode event');
  listeners.get('companion:game-mode')({ payload: { active: true, process: 'cs2', revision: 3 } });
  listeners.get('companion:game-mode')({ payload: { active: false, process: '', revision: 2 } });
  assert.equal(game.active, true, 'late events cannot turn off the current game session');
  const restoreCoordinator = effects[1](), restoreClicks = effects[2](), stopParking = effects[3]();
  await flush();
  assert.equal(passthrough.at(-1), true, 'game clicks pass through the pet window');
  assert.equal(moves.length, 1, 'game entry parks the pet at the edge');
  const first = { ...position };
  cursor = { x: position.x + 100, y: position.y + 100 };
  time += 4000;
  let timer = [...timers.entries()].at(-1); timers.delete(timer[0]); await timer[1].fn(); await flush();
  assert.notDeepEqual(position, first, 'approaching the pet moves it away from the input area');
  const count = moves.length;
  visible = false;
  timer = [...timers.entries()].at(-1); timers.delete(timer[0]); await timer[1].fn(); await flush();
  assert.equal(moves.length, count, 'offline/hidden pets are not moved');
  restoreCoordinator(); restoreClicks(); stopParking(); await flush();
  assert.deepEqual(position, { x: 500, y: 350 }, 'game exit restores the original pet position');
  assert.equal(passthrough.at(-1), false, 'normal interaction resumes');
  unsubscribe();
  assert.equal(listeners.size, 0); assert.equal(timers.size, 0, 'mode exit leaves no avoidance polling');
} finally {
  globalThis.setTimeout = prior.setTimeout; globalThis.clearTimeout = prior.clearTimeout; Date.now = prior.dateNow;
  delete globalThis.__gameFixture;
}
console.log('Game companionship: edge/close-button avoidance, peer separation, event ordering, click-through, cursor avoidance, restoration and cleanup passed');
