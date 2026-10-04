/**
 * 逃离落点的几何验证（纯算术，无需启动应用）。
 *
 * 这里验的是 planFlee 的三条不变式，它们都是真机上很难复现、但一错就很显眼的边界：
 *   - 落点必须「远」（≥ 下限），否则用户读成「没反应」；
 *   - 落点必须留在屏幕里（含多屏左侧那块屏的负坐标），否则桌宠被推出去半个身子；
 *   - 只往有余量的那一侧跑：贴着右边站的桌宠不能「往右逃」逃到屏幕外。
 *
 * 运行：node scripts/flee-plan.test.ts
 */

import { planFlee, type FleeGeometry } from '../src/chibi/fleePlan.ts';

let failures = 0;
function check(name: string, actual: unknown, expected: unknown): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    console.log(`  ok   ${name}`);
  } else {
    failures += 1;
    console.log(`  FAIL ${name}\n       期望 ${e}\n       实际 ${a}`);
  }
}
function checkThat(name: string, condition: boolean, detail: string): void {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    failures += 1;
    console.log(`  FAIL ${name}\n       ${detail}`);
  }
}

/** 确定性伪随机（LCG）：跑上千次取样也能复现同一批数字 */
function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1_664_525 + 1_013_904_223) >>> 0;
    return state / 0x1_0000_0000;
  };
}

const OPTIONS = { minDistancePx: 320, maxDistancePx: 900 };
/** 1920 宽的显示器、213 宽的桌宠窗口、8px 边距 */
const desktop = (fromX: number, fromY = 400): FleeGeometry => ({
  fromX,
  fromY,
  windowWidth: 213,
  monitorX: 0,
  monitorWidth: 1_920,
  marginPx: 8,
});

console.log('场景 A：屏幕正中——落点必须「远」、留在屏幕里、纵向不动');
{
  const geometry = desktop(853);
  const random = seeded(7);
  let minSeen = Number.POSITIVE_INFINITY;
  let maxSeen = 0;
  let outOfBounds = 0;
  let verticalDrift = 0;
  const directions = new Set<string>();
  for (let index = 0; index < 2_000; index += 1) {
    const plan = planFlee(geometry, { ...OPTIONS, random });
    if (!plan) {
      outOfBounds += 1;
      continue;
    }
    minSeen = Math.min(minSeen, plan.distancePx);
    maxSeen = Math.max(maxSeen, plan.distancePx);
    directions.add(plan.direction);
    if (plan.targetX < 8 || plan.targetX > 1_920 - 213 - 8) outOfBounds += 1;
    if (plan.targetY !== geometry.fromY) verticalDrift += 1;
  }
  check('从不出界', outOfBounds, 0);
  check('从不纵向漂移', verticalDrift, 0);
  checkThat('最远不超过上限', maxSeen <= OPTIONS.maxDistancePx + 1, `实测最远 ${maxSeen}`);
  checkThat('最近也不低于下限', minSeen >= OPTIONS.minDistancePx, `实测最近 ${minSeen}`);
  check('两个方向都会出现（不是永远同一侧）', [...directions].sort(), ['left', 'right']);
}

console.log('场景 B：贴着右边缘站——只能往左跑，且落点在屏幕内');
{
  const geometry = desktop(1_920 - 213 - 8); // 正好贴边
  const random = seeded(11);
  const plans = Array.from({ length: 200 }, () => planFlee(geometry, { ...OPTIONS, random }));
  checkThat('全部有落点', plans.every((plan) => plan !== null), '出现了 null');
  check('方向只有 left', [...new Set(plans.map((plan) => plan?.direction))], ['left']);
  checkThat(
    '落点都在屏幕内',
    plans.every((plan) => plan !== null && plan.targetX >= 8 && plan.targetX <= 1_920 - 213 - 8),
    `落点样本 ${plans.slice(0, 3).map((plan) => plan?.targetX).join(', ')}`,
  );
}

console.log('场景 C：贴着左边缘站——只能往右跑');
{
  const geometry = desktop(8);
  const random = seeded(13);
  const plans = Array.from({ length: 200 }, () => planFlee(geometry, { ...OPTIONS, random }));
  check('方向只有 right', [...new Set(plans.map((plan) => plan?.direction))], ['right']);
}

console.log('场景 D：两侧都凑不满下限（窄屏）——跑向更宽的那一侧，有多少跑多少');
{
  // 760 宽的屏：可站立区间 8..539，桌宠在 x=300 → 左 292、右 239，都不到 320
  const geometry: FleeGeometry = { ...desktop(300), monitorWidth: 760 };
  const plan = planFlee(geometry, OPTIONS);
  checkThat('仍然给出落点（不返回 null）', plan !== null, '返回了 null');
  check('跑向更宽的那一侧（左）', plan?.direction, 'left');
  check('落在该侧边界上（能跑多远跑多远）', plan?.targetX, 8);
  check('距离如实回报（小于下限也照实说）', plan?.distancePx, 292);
}

console.log('场景 E：横向没地方去 → null；桌宠停在边界外 → 拉回可站立区间');
{
  // 可站立区间宽度为 0：显示器 229 = 桌宠 213 + 两侧各 8
  const noRoom: FleeGeometry = { ...desktop(8), monitorWidth: 229 };
  check('屏幕比桌宠还窄 → null', planFlee(noRoom, OPTIONS), null);

  // 窗口被拖到右边缘之外（真机上拖拽可以做到）：这一趟应该把它带回屏幕内
  const outside = desktop(1_700);
  const random = seeded(23);
  let inBounds = 0;
  for (let index = 0; index < 200; index += 1) {
    const plan = planFlee(outside, { ...OPTIONS, random });
    if (plan && plan.targetX >= 8 && plan.targetX <= 1_699) inBounds += 1;
  }
  check('落点全部被夹回屏幕内', inBounds, 200);
}

console.log('场景 F：多屏左侧那块屏（显示器坐标是负的）');
{
  const geometry: FleeGeometry = {
    fromX: -1_000,
    fromY: 300,
    windowWidth: 213,
    monitorX: -1_920,
    monitorWidth: 1_920,
    marginPx: 8,
  };
  const random = seeded(17);
  let outOfBounds = 0;
  let minSeen = Number.POSITIVE_INFINITY;
  for (let index = 0; index < 500; index += 1) {
    const plan = planFlee(geometry, { ...OPTIONS, random });
    if (!plan) {
      outOfBounds += 1;
      continue;
    }
    minSeen = Math.min(minSeen, plan.distancePx);
    // 可站立区间：[-1920+8, -1920+1920-213-8] = [-1912, -221]
    if (plan.targetX < -1_912 || plan.targetX > -221) outOfBounds += 1;
  }
  check('从不出界', outOfBounds, 0);
  checkThat('落点仍然「远」', minSeen >= OPTIONS.minDistancePx, `实测最近 ${minSeen}`);
}

console.log('');
if (failures === 0) {
  console.log('全部通过：逃离落点永远「远」、永远留在屏幕里、只往有余量的那一侧跑。');
} else {
  console.log(`${failures} 项失败。`);
  process.exit(1);
}
