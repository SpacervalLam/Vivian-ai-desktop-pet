import assert from 'node:assert/strict';
import { build } from 'esbuild';

const compile = async path => {
  const result = await build({ entryPoints: [path], bundle: true, write: false, platform: 'node', format: 'esm' });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
};
const { CrossPresentation } = await compile('src/chibi/crossPresentation.ts');
const { speechDisplayMotion } = await compile('src/chibi/speechPose.ts');
const { ambientLane } = await compile('src/chibi/ambientLane.ts');
const { runSlide } = await compile('src/chibi/slideTrack.ts');

const cross = new CrossPresentation('nana');
const event = (id, speaker = 'vivian', listener = 'nana', text = 'hello') =>
  ({ stream_id: id, speaker_id: speaker, listener_id: listener, text });
assert.equal(cross.update(event('a')), true);
assert.equal(cross.motion, 'listen');
assert.equal(cross.update(event('a', 'nana', 'vivian'), true), true);
assert.equal(cross.motion, 'talk', 'reply reverses speaker and listener on the same stream');
cross.update(event('b'));
assert.equal(cross.finish('a'), true);
assert.equal(cross.motion, 'listen', 'older completion preserves the newer stream');
assert.equal(cross.update(event('a'), true), false, 'late completed chunk cannot revive a stream');
assert.equal(cross.update(event('foreign', 'alice', 'bob')), false);
assert.equal(cross.update(event('b', 'nana', 'vivian', '   '), true), false);
assert.equal(cross.motion, 'listen');
cross.removePeer('VIVIAN');
assert.equal(cross.motion, null);
cross.finish('missed-start');
assert.equal(cross.update(event('missed-start'), true), false);
cross.update(event('c'));
cross.clear();
assert.equal(cross.update(event('c'), true), false, 'user takeover invalidates interrupted peer speech');
for (const physical of ['drag', 'walk', 'turn', 'cast', 'sleep', 'wake', 'tend', 'busy-in', 'busy-out']) {
  assert.equal(speechDisplayMotion(physical, false, false, 'listen'), physical);
}
assert.equal(speechDisplayMotion('idle', false, false, 'listen'), 'listen');
assert.equal(speechDisplayMotion('idle', true, false, 'listen'), 'talk', 'own audio wins over peer listening');
assert.equal(speechDisplayMotion('idle', false, true, 'talk'), 'idle', 'user press wins');

const pet = { id: 'nana', x: -1300, y: 300, width: 200, height: 200 };
const peer = { id: 'vivian', x: -900, y: 310, width: 200, height: 200 };
assert.deepEqual(ambientLane(pet, [peer], -1900, -220, 16), { minX: -1900, maxX: -1116 });
assert.deepEqual(ambientLane(pet, [{ ...peer, y: 700 }], -1900, -220, 16), { minX: -1900, maxX: -220 });
assert.deepEqual(ambientLane(pet, [{ ...peer, x: -1600 }], -1900, -220, 16), { minX: -1384, maxX: -220 });
assert.deepEqual(ambientLane(pet, [{ ...peer, x: -1200 }], -1900, -220, 16), { minX: -1900, maxX: pet.x }, 'overlap can separate to the left');
const colocated = { ...peer, x: pet.x, y: pet.y };
assert.equal(ambientLane(pet, [colocated], -1900, -220, 16).maxX, pet.x);
assert.equal(ambientLane(colocated, [pet], -1900, -220, 16).minX, pet.x, 'exact overlap separates deterministically');
const scaled = rect => ({ ...rect, x: rect.x * 2, y: rect.y * 2, width: rect.width * 2, height: rect.height * 2 });
assert.deepEqual(ambientLane(scaled(pet), [scaled(peer)], -3800, -440, 32), { minX: -3800, maxX: -2232 });

const oldWindow = globalThis.window;
const oldPerformance = globalThis.performance;
let clock = 0, timerLag = 0;
globalThis.window = { setTimeout: (callback, ms) => { clock += ms + timerLag; queueMicrotask(callback); } };
globalThis.performance = { now: () => clock };
try {
  const points = [];
  const base = { fromX: -100, fromY: 100, toX: 500, toY: 200, durationMs: 320 };
  assert.equal(await runSlide({ ...base, apply: (x, y) => points.push([x, y, clock]) }), true);
  assert.deepEqual(points.at(-1), [500, 200, 320], 'end point is reached at the planned end time');
  assert.ok(points.every((p, i) => !i || p[0] >= points[i - 1][0]));
  clock = 0; timerLag = 90; points.length = 0;
  assert.equal(await runSlide({ ...base, apply: (x, y) => points.push([x, y, clock]) }), true);
  assert.ok(clock < 420 && points.length < 10, 'busy timers skip missed samples instead of stretching each step');
  clock = 0; timerLag = 0; let inFlight = 0, maxInFlight = 0;
  assert.equal(await runSlide({ ...base, apply: async () => {
    maxInFlight = Math.max(maxInFlight, ++inFlight);
    await Promise.resolve(); clock += 70; inFlight--;
  } }), true);
  assert.equal(maxInFlight, 1, 'native position requests are serialized');
  clock = 0; let cancelled = false, calls = 0;
  assert.equal(await runSlide({ ...base, shouldAbort: () => cancelled, apply: async () => {
    calls++; cancelled = true; await Promise.resolve();
  } }), false);
  assert.equal(calls, 1, 'cancellation during IPC stops all further samples');
  calls = 0;
  assert.equal(await runSlide({ ...base, shouldAbort: () => true, apply: () => calls++ }), false);
  assert.equal(calls, 0);
  points.length = 0;
  assert.equal(await runSlide({ ...base, durationMs: 0, apply: (x, y) => points.push([x, y]) }), true);
  assert.deepEqual(points, [[500, 200]]);
  await assert.rejects(runSlide({ ...base, durationMs: NaN, apply: () => {} }), RangeError);
  await assert.rejects(runSlide({ ...base, apply: async () => { throw new Error('native failed'); } }), /native failed/);
} finally {
  if (oldWindow === undefined) delete globalThis.window; else globalThis.window = oldWindow;
  globalThis.performance = oldPerformance;
}
console.log('Pet coordination: stream ownership, user priority, peer spacing, DPI/negative coordinates, elapsed movement, slow IPC and cancellation passed');
