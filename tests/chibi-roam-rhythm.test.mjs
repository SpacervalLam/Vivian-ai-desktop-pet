import assert from 'node:assert/strict';
import { build } from 'esbuild';

const result = await build({ stdin: { contents: `export { RoamRhythm } from './src/chibi/roamRhythm'; export { runSustainedPose } from './src/chibi/sustainedPose';`, resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm' });
const { RoamRhythm, runSustainedPose } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

for (const gravity of [true, false]) {
  const rhythm = new RoamRhythm(gravity, 0, () => 0);
  assert.equal(rhythm.ready(gravity ? 4000 : 10000), true);
  rhythm.schedule(10_000);
  rhythm.observeInput(14.99, 11_000);
  assert.equal(rhythm.ready(12_500), false, 'brief input pauses retain the normal cadence');
  rhythm.observeInput(15, 11_000);
  assert.equal(rhythm.ready(12_499), false);
  assert.equal(rhythm.ready(12_500), true, 'becoming idle shortens an already pending normal rest');
  rhythm.schedule(12_500);
  rhythm.observeInput(30, 13_000);
  assert.equal(rhythm.ready(14_000), true, 'repeated idle observations never postpone the next action');
  rhythm.schedule(14_000, true);
  assert.equal(rhythm.ready(14_749), false);
  assert.equal(rhythm.ready(14_750), true, 'blocked actions retry promptly without bypassing admission');
  rhythm.observeInput(0, 15_000);
  assert.equal(rhythm.ready(16_000), false, 'resuming input cancels the fast pending opportunity');
  assert.equal(rhythm.ready(15_000 + (gravity ? 9000 : 48000)), true);
  rhythm.observeInput(60, 100_000);
  rhythm.observeInput(null, 100_100);
  assert.equal(rhythm.ready(104_100), false, 'unavailable global input must not imply inactivity');
  rhythm.observeInput(NaN, 100_200);
  assert.ok(rhythm.nextCheckMs(100_200) <= 2000);
}
const slow = new RoamRhythm(false, 0, () => 1);
slow.observeInput(60, 0);
assert.equal(slow.ready(3999), false);
assert.equal(slow.ready(4000), true, 'even the longest idle rest is four seconds');

// Fake time verifies a real held phase, both transition animations, and ownership takeover.
for (const interruptAt of [null, 300, 3000]) {
  let now = 0;
  const phases = [];
  const completed = await runSustainedPose(20_000, {
    enter: async () => { phases.push('enter'); now += 1800; return true; },
    leave: async () => { phases.push('leave'); now += 1800; return true; },
    onHold: () => phases.push('hold'), onLeave: () => phases.push('release'),
    interrupted: () => interruptAt !== null && now >= interruptAt,
    now: () => now, wait: async ms => { now += ms; },
  });
  assert.equal(completed, interruptAt === null);
  if (interruptAt === null) {
    assert.deepEqual(phases, ['enter', 'hold', 'release', 'leave']);
    assert.equal(now, 23_600, 'the held state is in addition to entrance and exit playback');
  } else {
    assert.ok(!phases.includes('leave'), 'takeover must never play the old exit over its successor');
    assert.ok(now <= Math.max(1800, interruptAt + 100), 'a held pose yields within one sample');
  }
}
console.log('Roam cadence: global input idle, pending timers, resume/failure fallback; sustained pose hold and takeover passed');
