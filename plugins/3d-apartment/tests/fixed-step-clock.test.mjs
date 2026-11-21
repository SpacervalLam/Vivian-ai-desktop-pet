import assert from 'node:assert/strict';
import { FixedStepClock } from '../src/agents/fixedStepClock.ts';

for (const fps of [30, 60, 120, 144]) {
  const clock = new FixedStepClock(0.05);
  let ticks = 0;
  for (let frame = 0; frame < fps * 60; frame++) {
    ticks += clock.advance(1 / fps);
    assert.ok(clock.alpha >= 0 && clock.alpha < 1 + 1e-8);
  }
  assert.equal(ticks, 1200, `${fps} Hz: exactly 20 simulation steps/sec`);
  console.log(`${fps} Hz, 60s: old=${fps * 60} simulation steps; new=${ticks}`);
}
const clock = new FixedStepClock(0.05);
assert.equal(clock.advance(0.01), 0);
clock.reset();
assert.equal(clock.advance(0.04), 0, 'hidden time must not carry into resume');
assert.equal(clock.advance(60), 5, 'long stalls must not cause an unbounded catch-up loop');
console.log('Fixed logic frequency and interpolation bounds: passed');
