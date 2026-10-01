/**
 * 角色行动轨迹验收哨兵 —— 改热点 / 家具 / 墙 / 角色半径之后必跑。
 *
 * 之所以要有它：角色是「数据驱动」的，热点写错（比如坐点落在沙发背后）在编辑
 * JSON 时看不出来，只有跑起来盯着屏幕才知道。这个脚本把能在离线算清楚的三件事
 * 全部钉死：
 *   1. 热点站立落点必须真的可走（A* 到得了、不落在膨胀后的障碍里）；
 *   2. 坐类热点的坐点必须压在指定家具的坐面上，且抬升量和坐面高度对得上；
 *   3. 任意两热点之间的路径，沿途净空不得低于角色硬半径。
 *   4. 两个角色头对头相遇时不得互穿（跑一段无渲染的状态机仿真，统计最小间距）。
 *
 * 跑法：npm run check:agent-track
 */
import layoutData from '../../../../plugins/3d-apartment/src/dormLayout.json';
import {
  buildObstacles,
  buildGrid,
  findPath,
  clearanceAt,
  CHARACTER_RADIUS,
  SOFT_CLEARANCE,
} from '../../../../plugins/3d-apartment/src/agents/navGrid';
import { HOTSPOTS } from '../../../../plugins/3d-apartment/src/agents/hotspots';

const layout = layoutData as any;
const obstacles = buildObstacles(layout);
const grid = buildGrid(obstacles, layout.room.bounds);

/**
 * 坐姿 clip 自带坐高，按身高归一。
 *
 * 由 GLB 量得：身高 1.45m 时坐定髋高 0.361(Vivian)/0.398(Nana)，减去髋到坐面的
 * 0.07~0.10 ⇒ clip 相当于坐在 0.28~0.30 的矮凳上，取 0.29。模型是按身高线性缩放的，
 * 所以坐高按 height/1.45 等比换算——**改了 height 却没改坐点抬升量，这里会直接报错**。
 * 换角色模型/重导出 GLB 要重测这个基准值。
 */
const SIT_CLIP_SEAT_Y_AT = { base: 0.29, atHeight: 1.45 };
const clipSeatY = (height: number) => (SIT_CLIP_SEAT_Y_AT.base * height) / SIT_CLIP_SEAT_Y_AT.atHeight;

let bad = 0;
const fail = (msg: string) => { console.log(`FAIL  ${msg}`); bad++; };
const ok = (msg: string) => console.log(`ok    ${msg}`);

/* ---------- 1. 门洞净宽 ⇒ 角色半径上限 ---------- */
let narrowest = { id: '', width: Infinity };
for (const w of layout.shell.walls ?? []) {
  for (const op of w.openings ?? []) {
    if (op.kind === 'window') continue;
    // navCut 的推拉门只有半边能走
    const width = (op.b - op.a) / (op.navCut ? 2 : 1);
    if (width < narrowest.width) narrowest = { id: `${w.id}@${op.a}~${op.b}`, width };
  }
}
console.log(`\n— 门洞 —\n  最窄可走门洞 ${narrowest.id} 净宽 ${narrowest.width.toFixed(2)}m`);
console.log(`  当前角色半径 ${CHARACTER_RADIUS} ⇒ 门洞剩余 ${(narrowest.width - 2 * CHARACTER_RADIUS).toFixed(2)}m（需 ≥ 一格 ${grid.cell.toFixed(2)}）`);
if (narrowest.width - 2 * CHARACTER_RADIUS < grid.cell) {
  fail(`角色半径 ${CHARACTER_RADIUS} 超过最窄门洞 ${narrowest.id} 的承受范围，房间会被走成孤岛`);
}

/* ---------- 2. 热点落点 ---------- */
// characters 里混着 _height_note 这类说明字段，按「有 startPos」挑真人
const starts: Array<{ id: string; pos: [number, number, number] }> =
  Object.entries(layout.characters ?? {})
    .filter(([, c]: [string, any]) => Array.isArray(c?.startPos))
    .map(([id, c]: [string, any]) => ({ id, pos: c.startPos }));

/** 两个角色共用一套坐点，身高取均值（当前两者相同）——决定 clip 自带坐高。 */
const charHeights = Object.values(layout.characters ?? {})
  .filter((c: any) => typeof c?.height === 'number')
  .map((c: any) => c.height as number);
const charHeight = charHeights.reduce((a, b) => a + b, 0) / charHeights.length;
const clipSeat = clipSeatY(charHeight);
console.log(`\n— 角色 —\n  身高 ${charHeight.toFixed(2)}m ⇒ 坐姿 clip 自带坐高 ${clipSeat.toFixed(3)}m（基准 ${SIT_CLIP_SEAT_Y_AT.base}@${SIT_CLIP_SEAT_Y_AT.atHeight}m）`);

const furnitureById = new Map<string, any>((layout.furniture ?? []).map((f: any) => [f.id, f]));
/** 家具旋转后的轴对齐占地——和 navGrid.buildObstacles 同一套算法，这里独立算一遍做交叉校验。 */
function footprint(f: any) {
  const [x, , z] = f.pos;
  const [w, , d] = f.size;
  const r = f.rot ?? 0;
  const c = Math.abs(Math.cos(r)), s = Math.abs(Math.sin(r));
  return { x0: x - (w * c + d * s) / 2, x1: x + (w * c + d * s) / 2, z0: z - (w * s + d * c) / 2, z1: z + (w * s + d * c) / 2 };
}

console.log('\n— 出生点 —');
for (const s of starts) {
  const clear = clearanceAt(s.pos[0], s.pos[2], grid);
  if (clear <= CHARACTER_RADIUS) fail(`角色 ${s.id} 出生点 (${s.pos[0]}, ${s.pos[2]}) 落在障碍里（净空 ${clear.toFixed(3)}m）`);
  else ok(`${s.id.padEnd(14)} 出生点净空 ${clear.toFixed(2)}m`);
}

console.log('\n— 热点 —');
/**
 * 本哨兵只验得了 203 那一层：它手里的栅格是室内那张（node 里没有场景装配，
 * 建不出 navWorld 的多层栅格）。跨层热点（外廊/大堂/街区/店铺）的落脚点与
 * 可达性由浏览器版的 plugins/3d-apartment/tools/room/check-world-route.mjs 验。
 */
const INDOOR_LAYER = 'apt2';
const indoor = HOTSPOTS.filter(h => (h.layer ?? INDOOR_LAYER) === INDOOR_LAYER);
const worldCount = HOTSPOTS.length - indoor.length;
console.log();
const reachable: Array<{ id: string; pos: [number, number, number] }> = [];
for (const h of indoor) {
  const clear = clearanceAt(h.pos[0], h.pos[2], grid);   // A* 实际用的口径
  const clearExact = exactClearance(h.pos[0], h.pos[2]); // 真实净空，用来看「贴不贴家具」
  const cellFree = clear > CHARACTER_RADIUS;
  const reach = starts.map(s => !!findPath({ x: s.pos[0], z: s.pos[2] }, { x: h.pos[0], z: h.pos[2] }, grid));
  if (!cellFree) fail(`热点 ${h.id} 落点 (${h.pos[0]}, ${h.pos[2]}) 净空仅 ${clear.toFixed(3)}m < 半径 ${CHARACTER_RADIUS}`);
  starts.forEach((s, i) => { if (!reach[i]) fail(`热点 ${h.id} 从 ${s.id} 出生点走不到`); });
  let seatNote = '';
  if (h.animation === 'sit') {
    const seat = h.seat;
    if (!seat) { fail(`坐类热点 ${h.id} 缺 seat 落点`); continue; }
    const f = furnitureById.get(seat.on);
    if (!f) { fail(`坐类热点 ${h.id} 的 seat.on="${seat.on}" 在家具表里找不到`); continue; }
    const box = footprint(f);
    const inside = seat.pos[0] >= box.x0 && seat.pos[0] <= box.x1 && seat.pos[2] >= box.z0 && seat.pos[2] <= box.z1;
    const wantLift = seat.surfaceY - clipSeat;
    const liftErr = Math.abs(seat.pos[1] - wantLift);
    if (!inside) fail(`热点 ${h.id} 坐点 (${seat.pos[0]}, ${seat.pos[2]}) 不在 ${seat.on} 的占地 [${box.x0.toFixed(2)},${box.x1.toFixed(2)}]×[${box.z0.toFixed(2)},${box.z1.toFixed(2)}] 内`);
    if (liftErr > 0.03) fail(`热点 ${h.id} 抬升 ${seat.pos[1]} 与坐面 ${seat.surfaceY} − clip 坐高 ${clipSeat.toFixed(3)} = ${wantLift.toFixed(3)} 不符`);
    seatNote = ` 坐点(${seat.pos[0]}, ${seat.pos[1]}, ${seat.pos[2]})@${seat.on}${inside ? ' 压在坐面上' : ''}`;
    if (inside && liftErr <= 0.03) ok(`${h.id.padEnd(14)} 净空 ${clearExact.toFixed(2)}m${seatNote}`);
    continue;
  }
  if (cellFree && reach.every(Boolean)) {
    ok(`${h.id.padEnd(14)} 净空 ${clearExact.toFixed(2)}m${clearExact < SOFT_CLEARANCE ? ' (偏窄，走过去会贴家具)' : ''}`);
  }
  reachable.push({ id: h.id, pos: h.pos });
}

/* ---------- 3. 路径净空 ---------- */
/**
 * 点到最近障碍的真实距离（不经过栅格）。
 * 栅格净空是「最近格中心」的值，误差可达半格（0.075m）——拿它当验收口径会把
 * 好路线判成贴墙，所以这里独立按 AABB 精确算一遍。
 */
function exactClearance(x: number, z: number): number {
  let best = Infinity;
  for (const a of obstacles) {
    const dx = Math.max(a.minX - x, 0, x - a.maxX);
    const dz = Math.max(a.minZ - z, 0, z - a.maxZ);
    const d = Math.hypot(dx, dz);
    if (d < best) best = d;
  }
  return best;
}

/** 沿折线采样最小净空；最后一段（走到精确落点）单独算，落点是布局数据，允许贴一点家具。 */
function pathClearance(pts: Array<{ x: number; z: number }>) {
  let minBody = Infinity;
  const legs = pts.slice(0, -1);
  for (let i = 0; i < legs.length - 1; i++) {
    const a = legs[i], b = legs[i + 1];
    const d = Math.hypot(b.x - a.x, b.z - a.z);
    // 与 navGrid.segmentClear 同量级（cell/7）：擦角漏检会在这里被抓出来
    const steps = Math.max(1, Math.ceil(d / (grid.cell / 7)));
    for (let s = 0; s <= steps; s++) {
      const t = s / steps;
      minBody = Math.min(minBody, exactClearance(a.x + (b.x - a.x) * t, a.z + (b.z - a.z) * t));
    }
  }
  const last = pts[pts.length - 1];
  return { body: minBody, end: exactClearance(last.x, last.z), turns: Math.max(0, pts.length - 2) };
}

const nodes = [...starts.map(s => ({ id: s.id, pos: s.pos })), ...reachable];
console.log('\n— 路径 —');
const stats: Array<{ route: string; body: number; end: number; turns: number; len: number }> = [];
for (const from of nodes) {
  for (const to of reachable) {
    if (from.id === to.id) continue;
    const path = findPath({ x: from.pos[0], z: from.pos[2] }, { x: to.pos[0], z: to.pos[2] }, grid);
    if (!path) { fail(`${from.id} → ${to.id} 不可达`); continue; }
    const pts = [{ x: from.pos[0], z: from.pos[2] }, ...path];
    const c = pathClearance(pts);
    let len = 0;
    for (let i = 0; i < pts.length - 1; i++) len += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].z - pts[i].z);
    stats.push({ route: `${from.id} → ${to.id}`, ...c, len });
    if (c.body < CHARACTER_RADIUS - 1e-6) fail(`${from.id} → ${to.id} 途中净空 ${c.body.toFixed(3)}m 小于角色半径`);
  }
}
stats.sort((a, b) => a.body - b.body);
const tight = stats.filter(s => s.body < SOFT_CLEARANCE).length;
console.log(`  ${stats.length} 条路线：途中最小净空 ${stats[0]?.body.toFixed(3)}m，中位 ${stats[Math.floor(stats.length / 2)]?.body.toFixed(3)}m`);
console.log(`  挤在软净空 ${SOFT_CLEARANCE}m 以内的路线：${tight}/${stats.length}`);
console.log(`  平均拐点数 ${(stats.reduce((s, x) => s + x.turns, 0) / stats.length).toFixed(1)}（拉绳平滑后）`);
console.log('  最挤的 5 条：');
for (const s of stats.slice(0, 5)) console.log(`    ${s.route.padEnd(28)} 净空 ${s.body.toFixed(3)}m  落点 ${s.end.toFixed(3)}m  长 ${s.len.toFixed(2)}m  拐点 ${s.turns}`);

/* ---------- 4. 两个角色头对头：不许互穿 ---------- */
/**
 * 跑一段无渲染的状态机仿真。
 *
 * 「路径净空达标」只能证明单个角色不蹭墙，证明不了两个角色不撞——那是运行时
 * 才暴露的问题（规划层绕人 + 执行层让行 + 侧移让路三件事凑一起才成立）。
 * 这里把 PetAgent 直接跑起来，逐 tick 记两者距离，用最小间距当判据。
 */
const { PetAgent, SEPARATION } = await import('../../../../plugins/3d-apartment/src/agents/usePetAgent');
const bodyR = CHARACTER_RADIUS;

/** 让两个角色从各自起点对头走到对方一侧，返回全过程最小间距与让行次数。 */
function headOn(fromA: string, toA: string, fromB: string, toB: string) {
  const hs = HOTSPOTS;
  const pick = (id: string) => {
    const s = starts.find(x => x.id === id);
    if (s) return { pos: s.pos, id };
    const h = hs.find(x => x.id === id)!;
    return { pos: h.pos, id };
  };
  const A = pick(fromA), B = pick(fromB);
  const a = new PetAgent(LA, [A.pos[0], 0, A.pos[2]], 0);
  const b = new PetAgent(LB, [B.pos[0], 0, B.pos[2]], 0);
  PetAgent.bindPeers([a, b]);
  a.goTo(toA); b.goTo(toB);
  let min = Infinity, minAt = '';
  let yieldTicks = 0;
  const dt = 0.05;
  for (let i = 0; i < 2400; i++) {          // 120s 上限
    a.tick(dt); b.tick(dt);
    PetAgent.separate([a, b]);
    const d = Math.hypot(a.pos.x - b.pos.x, a.pos.z - b.pos.z);
    if (d < min) { min = d; minAt = `${a.stateLabel} / ${b.stateLabel} @(${((a.pos.x + b.pos.x) / 2).toFixed(2)}, ${((a.pos.z + b.pos.z) / 2).toFixed(2)})`; }
    if (a.state === 'yield' || b.state === 'yield') yieldTicks++;
    if (a.state === 'stay' && b.state === 'stay') break;
  }
  return { route: `${fromA}→${toA} vs ${fromB}→${toB}`, min, minAt, yieldSec: (yieldTicks * dt).toFixed(1) };
}
const LA = 'Vivian', LB = 'Nana';
const encounters = [
  headOn('Nana', 'genkan', 'genkan', 'nana'),        // 走廊对头（主场景）
  headOn('Nana', 'balcony', 'balcony', 'nana'),      // 走廊 + 客厅
  headOn('Nana', 'washroom', 'washroom', 'nana'),
];
console.log('\n— 对头相遇（仿真） —');
/**
 * 门槛 = 分离约束的硬保证值：躯干直径（0.32）+ 余量。
 * 两个角色各 0.26 硬半径 ⇒ 0.52 是「连裙摆/头发都不交叠」，而 1.2m 走廊的可用带宽
 * 只有 0.68m、两人臂展各 0.82m —— 那个目标在数学上不成立，不是算法能补的。
 */
const TOUCH_LIMIT = SEPARATION;
for (const e of encounters) {
  const pass = e.min >= TOUCH_LIMIT - 0.02;
  console.log(`  ${pass ? 'ok  ' : 'FAIL'} ${e.route.padEnd(34)} 最小间距 ${e.min.toFixed(3)}m（需 ≥ ${TOUCH_LIMIT.toFixed(2)}）  让行 ${e.yieldSec}s  ${e.minAt}`);
  if (!pass) fail(`对头相遇时两者最近只差 ${e.min.toFixed(3)}m，躯干会重叠`);
}

console.log(bad === 0 ? '\nALL PASS' : `\n${bad} FAILED`);
process.exit(bad ? 1 : 0);
