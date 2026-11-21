import assert from 'node:assert/strict';
import { build } from 'esbuild';
const built = await build({ stdin: { contents: `export {ambientLane} from './src/chibi/ambientLane'; export {petFootprint} from './src/chibi/desktopPhysics'; export {planRoam,MIN_ROAM_TRAVEL_PX} from './src/chibi/roamPlan';`, resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm' });
const { ambientLane, petFootprint, planRoam, MIN_ROAM_TRAVEL_PX } = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].text).toString('base64'));
const pets = ['vivian', 'nana', 'chatgpt'].map((id, i) => ({ id, x: i * 315, y: 200, width: 300, height: 300 }));
for (const pet of pets) {
  const lane = ambientLane(pet, pets, 8, 1600, 8);
  const plan = planRoam({ x: pet.x, ...lane, gravity: true, random: () => 0 });
  assert.ok(Math.abs(plan.targetX - pet.x) >= MIN_ROAM_TRAVEL_PX, `${pet.id} can patrol even when created before the imported pet`);
  const footprint = petFootprint(pet.width, pet.height);
  for (const peer of pets.filter(p => p !== pet)) {
    const other = petFootprint(peer.width, peer.height);
    const left = plan.targetX + footprint.left, right = left + footprint.width;
    const peerLeft = peer.x + other.left, peerRight = peerLeft + other.width;
    assert.ok(right + 8 <= peerLeft + 1 || left >= peerRight + 8 - 1, 'the visible bodies retain their collision gap within rounded window coordinates');
  }
}
const pet = pets[1];
assert.deepEqual(ambientLane(pet, [{ ...pets[0], y: -500 }], 8, 1600, 8),
  { minX: 8, maxX: 1600, walls: { left: true, right: true } },
  'pets on a different level do not restrict horizontal walking');

// 同伴窗口挤出来的端点不是墙：靠墙倚靠只许发生在屏幕边界上。
const crowded = ambientLane(pet, [pets[0], pets[2]], 8, 1600, 8);
assert.equal(crowded.walls.left, false, 'a peer on the left replaces the screen edge');
assert.equal(crowded.walls.right, false, 'a peer on the right replaces the screen edge');
// random() = 0.4 命中靠墙分支；两侧都是同伴窗口时不得返回 lean。
const crowdedPlan = planRoam({ x: pet.x, ...crowded, gravity: true, random: () => 0.4 });
assert.notEqual(crowdedPlan.intent, 'edge', 'never lean on a peer window edge');
assert.notEqual(crowdedPlan.gesture?.motion, 'lean', 'no lean gesture towards a peer');
assert.equal(crowdedPlan.intent, 'lookout', 'with peers on both sides there is no wall to walk to, so it looks around in place');
// 一端真墙、一端同伴：仍然只往真墙那边靠。
const halfOpen = ambientLane(pets[0], [pets[1]], 8, 1600, 8);
assert.equal(halfOpen.walls.left, true, 'the screen edge on the left is still a wall');
assert.equal(halfOpen.walls.right, false, 'the neighbour on the right is not');
const halfOpenPlan = planRoam({ x: pets[0].x, ...halfOpen, gravity: true, random: () => 0.4 });
assert.equal(halfOpenPlan.intent, 'edge');
assert.equal(halfOpenPlan.targetX, halfOpen.minX, 'leans on the screen edge, not on the neighbour');
// 站台被两侧同伴挤得迈不开步：原地张望，而不是对着同伴侧身。
const squeezed = ambientLane({ id: 'nana', x: 300, y: 200, width: 300, height: 300 }, [pets[0], pets[2]], 8, 1600, 8);
const squeezedPlan = planRoam({ x: 300, ...squeezed, gravity: true, random: () => 0.4 });
if (squeezed.maxX - squeezed.minX < MIN_ROAM_TRAVEL_PX) {
  assert.equal(squeezedPlan.intent, 'lookout', 'a lane pinched by peers has no wall to lean on');
}
console.log('Ambient lanes: walkable ends are flagged as walls; peers never become lean targets');
