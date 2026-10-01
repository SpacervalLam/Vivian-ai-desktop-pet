import { performance } from 'node:perf_hooks';
import assert from 'node:assert/strict';
import { PetAgent } from '../src/agents/usePetAgent';
import { FixedStepClock } from '../src/agents/fixedStepClock';
import layout from '../src/dormLayout.json';

function run(fixed: boolean, fps: number, stationary = false) {
  let seed = 42;
  const originalRandom = Math.random;
  Math.random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 4294967296; };
  try {
    PetAgent.resetClaims();
    PetAgent.bindWorld(null);
    const agents = Object.entries(layout.characters)
      .filter(([, value]) => Array.isArray((value as any)?.startPos))
      .map(([id, value]) => new PetAgent(id, (value as any).startPos, (value as any).startFacing ?? 0));
    PetAgent.bindPeers(agents);
    if (stationary) for (const agent of agents) {
      agent.state = 'stay';
      (agent as any).stayTimer = 1e9;
    }
    const previous = agents.map((agent) => ({ ...agent.pos, facing: agent.facing }));
    const rendered = agents.map((agent) => ({ ...agent.pos, facing: agent.facing }));
    const clock = new FixedStepClock(0.05);
    let steps = 0;
    const start = performance.now();
    for (let frame = 0; frame < fps * 180; frame++) {
      const count = fixed ? clock.advance(1 / fps) : 1;
      for (let i = 0; i < count; i++) {
        if (fixed) agents.forEach((agent, index) => Object.assign(previous[index], agent.pos, { facing: agent.facing }));
        for (const agent of agents) agent.tick(fixed ? 0.05 : 1 / fps);
        PetAgent.separate(agents);
        steps++;
      }
      // Include the new render interpolation work in the comparison.
      for (let index = 0; index < agents.length; index++) {
        const agent = agents[index], prev = previous[index], pose = rendered[index];
        if (fixed) {
          const alpha = clock.alpha;
          const delta = Math.atan2(Math.sin(agent.facing - prev.facing), Math.cos(agent.facing - prev.facing));
          pose.facing = prev.facing + delta * alpha;
          pose.x = prev.x + (agent.pos.x - prev.x) * alpha;
          pose.y = prev.y + (agent.pos.y - prev.y) * alpha;
          pose.z = prev.z + (agent.pos.z - prev.z) * alpha;
        } else {
          pose.facing = agent.facing;
          pose.x = agent.pos.x; pose.y = agent.pos.y; pose.z = agent.pos.z;
        }
      }
    }
    const ms = performance.now() - start;
    for (const pose of rendered) for (const value of Object.values(pose)) assert.ok(Number.isFinite(value));
    return { ms, steps };
  } finally { Math.random = originalRandom; }
}

for (const stationary of [true, false]) for (const fps of [60, 144]) {
  run(false, fps, stationary); run(true, fps, stationary); // same warm-up
  const before: number[] = [], after: number[] = [];
  for (let repeat = 0; repeat < 7; repeat++) {
    const order = repeat % 2 ? [true, false] : [false, true];
    for (const fixed of order) (fixed ? after : before).push(run(fixed, fps, stationary).ms);
  }
  const median = (values: number[]) => [...values].sort((a, b) => a - b)[3];
  console.log(JSON.stringify({ scenario: stationary ? 'stationary' : 'autonomous', fps, simulatedSeconds: 180, beforeMs: before, afterMs: after,
    beforeMedianMs: median(before), afterMedianMs: median(after),
    reductionPct: (1 - median(after) / median(before)) * 100 }));
}
