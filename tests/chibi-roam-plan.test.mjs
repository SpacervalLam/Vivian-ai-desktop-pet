import assert from 'node:assert/strict';
import { build } from 'esbuild';

const result = await build({ entryPoints: ['src/chibi/roamPlan.ts'], bundle: true, write: false, platform: 'node', format: 'esm' });
const { planRoam } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

/** 依序吐出给定数值的随机源，让每趟意图都可以被精确复现。 */
const sequence = values => {
  let index = 0;
  return () => values[index++ % values.length];
};
const within = (value, min, max) => value >= min - 1 && value <= max + 1;

// 重力模式：巡逻走向可走区间更远的一端。
let plan = planRoam({ x: 500, minX: 100, maxX: 900, gravity: true, random: sequence([0.1, 0.9]) });
assert.equal(plan.intent, 'patrol');
assert.equal(plan.targetX, 100, 'patrol heads for the farther end of the lane');
assert.equal(plan.direction, 'left');
assert.equal(plan.edge, false);
assert.equal(plan.gesture, null, 'a roll above 0.5 skips the arrival gesture');
plan = planRoam({ x: 500, minX: 100, maxX: 900, gravity: true, random: sequence([0.1, 0.2]) });
assert.ok(plan.gesture && ['look', 'smug', 'happy', 'think'].includes(plan.gesture.motion), 'patrol may end in a small gesture');

// 重力模式：靠墙走向最近的一侧，并带回 lean 倚靠时长。
plan = planRoam({ x: 850, minX: 100, maxX: 900, gravity: true, random: sequence([0.4, 0]) });
assert.equal(plan.intent, 'edge');
assert.equal(plan.targetX, 900, 'edge heads for the nearer boundary');
assert.equal(plan.direction, 'right');
assert.equal(plan.edge, true);
assert.equal(plan.gesture.motion, 'lean');
assert.equal(plan.gesture.durationMs, 20_000);
plan = planRoam({ x: 150, minX: 100, maxX: 900, gravity: true, random: sequence([0.4, 0.999]) });
assert.equal(plan.targetX, 100);
assert.equal(plan.direction, 'left');
assert.equal(plan.gesture.durationMs, 119_900, 'lean is a sustained state lasting up to two minutes');

// 重力模式：闲逛是短途位移，目标不会越过可走区间。
plan = planRoam({ x: 500, minX: 100, maxX: 900, gravity: true, random: sequence([0.7, 0, 0.5, 0.2]) });
assert.equal(plan.intent, 'stroll');
assert.ok(within(plan.targetX, 100, 900));
assert.ok(Math.abs(plan.targetX - 500) >= 140 && Math.abs(plan.targetX - 500) <= 700, 'stroll distance stays in the short-walk range');
plan = planRoam({ x: 880, minX: 100, maxX: 900, gravity: true, random: sequence([0.7, 0.9, 0.9, 0.2]) });
assert.ok(within(plan.targetX, 100, 900), 'a stroll near a boundary is clamped into the lane');

// 重力模式：原地张望不做位移。
plan = planRoam({ x: 500, minX: 100, maxX: 900, gravity: true, random: sequence([0.95, 0.3]) });
assert.equal(plan.intent, 'lookout');
assert.equal(plan.targetX, 500);
assert.equal(plan.gesture.motion, 'look');

// 重力模式 + 兴趣点：走到标注点附近围观，并选一个和内容类型匹配的收尾动作。
const interest = [
  { x: 120, y: 200, width: 160, height: 160, kind: 'ui' },
  { x: 700, y: 300, width: 160, height: 160, kind: 'image' },
];
plan = planRoam({ x: 480, minX: 100, maxX: 900, gravity: true, interest, random: sequence([0.1, 0.9, 0.9]) });
assert.equal(plan.intent, 'interest');
assert.equal(plan.edge, false);
assert.ok(within(plan.targetX, 100, 900));
assert.equal(plan.gesture.motion, 'look', 'a colorful picture earns a longer look');
plan = planRoam({ x: 480, minX: 100, maxX: 900, gravity: true, interest: [interest[0]], random: sequence([0.1, 0.1]) });
assert.equal(plan.intent, 'interest');
assert.ok(Math.abs(plan.targetX - 200) <= 60, 'the visit lands within the annotated cell around it');
assert.equal(plan.gesture.motion, 'think', 'text/UI is pondered rather than simply looked at');
// 兴趣点在可走区间之外时把目标夹回边界，仍然算一次围观。
plan = planRoam({ x: 480, minX: 400, maxX: 900, gravity: true, interest: [interest[0]], random: sequence([0.1, 0.1]) });
assert.equal(plan.intent, 'interest');
assert.equal(plan.targetX, 400);

// 可走区间太窄：重力模式下靠边倚靠，否则原地张望。
plan = planRoam({ x: 225, minX: 200, maxX: 250, gravity: true, random: sequence([0, 0.25]) });
assert.equal(plan.intent, 'edge');
assert.equal(plan.targetX, 200);
assert.equal(plan.edge, true);
plan = planRoam({ x: 225, minX: 200, maxX: 250, gravity: false, random: sequence([0.2, 0.4]) });
assert.equal(plan.intent, 'lookout');
assert.equal(plan.targetX, 225);

// 关闭重力：保留原有的随机漫步节奏，不再附带收尾动作。
plan = planRoam({ x: 500, minX: 100, maxX: 900, gravity: false, random: sequence([0.3, 0.5]) });
assert.equal(plan.intent, 'stroll');
assert.equal(plan.gesture, null);
assert.ok(within(plan.targetX, 100, 900));
assert.equal(plan.edge, false);

// 边界容错：x 超出可走区间会被夹回去，靠墙时朝向跟着所在的那一端。
plan = planRoam({ x: 2000, minX: 100, maxX: 900, gravity: true, random: sequence([0.4, 0.1]) });
assert.equal(plan.targetX, 900);
assert.equal(plan.direction, 'right');
plan = planRoam({ x: -200, minX: 100, maxX: 900, gravity: true, random: sequence([0.4, 0.1]) });
assert.equal(plan.targetX, 100);
assert.equal(plan.direction, 'left');

console.log('Chibi roam plan: patrol/edge/stroll/lookout intents, lean holds, narrow lanes and legacy pacing passed');
