import * as THREE from 'three';
import { Batch, louvers, railing } from './batch.js';

/* 建筑体量。坐标是全新的，和旧场景没有任何关系：
 *   巷子沿 Z，中心 x=0，路面 x∈[-3.5,3.5]，近端 z=+45 / 远端 z=-110
 *   主公寓在右侧 x∈[3.5,15.5]、z∈[-9,9]，主立面朝 -X（面朝巷子）
 *   两侧低层瓦顶街屋夹出 7m 连续街墙
 * 图 1 的公寓是这个体量的斜前视角，图 2 是它的 1F 入口，图 3 在它 3F 阳台内。
 */
export const ALLEY = { half: 3.5, near: 45, far: -110 };
export const APT = { x0: 3.5, x1: 15.5, z0: -9, z1: 9, floors: [0, 3.1, 6.2, 9.3], top: 12.4 };

/* ------------------------------------------------------------------ 工具 */
function windowPanel(b, x, y, z, w, h, facing = -1, lit = true) {
  // 薄盒当窗扇，法线朝巷子；材质自带 emissiveMap，夜里会亮
  b.box(lit ? 'windowLit' : 'windowDark', x, y, z, .035, h, w);
  const fx = x + facing * .035;
  const t = .045;
  b.box('blackMetal', fx, y + h / 2, z, .055, t, w + .08);
  b.box('blackMetal', fx, y - h / 2, z, .055, t, w + .08);
  b.box('blackMetal', fx, y, z - w / 2, .055, h, t);
  b.box('blackMetal', fx, y, z + w / 2, .055, h, t);
  b.box('blackMetal', fx, y, z, .05, h, .038);
}

function planter(b, x, z, w = 1.6, d = .7, h = .52, y = 0) {
  b.box('pot', x, y + h / 2, z, w, h, d);
  b.box('soil', x, y + h + .012, z, w - .13, .024, d - .13);
}

function shrubClump(parent, M, x, y, z, r, seed) {
  const g = new THREE.IcosahedronGeometry(r, 1);
  const m = new THREE.Mesh(g, M.shrub);
  m.position.set(x, y, z);
  m.scale.set(1, .78 + (seed % 5) * .05, 1);
  m.castShadow = true;
  parent.add(m);
  return m;
}

function flowerDots(parent, M, x, y, z, rx, rz, n, seed) {
  const geo = new THREE.IcosahedronGeometry(.075, 0);
  const inst = new THREE.InstancedMesh(geo, M.flowerPink, n);
  const dummy = new THREE.Object3D();
  let s = seed * 7919 + 13;
  const rnd = () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
  for (let i = 0; i < n; i++) {
    dummy.position.set(x + (rnd() - .5) * rx * 2, y + rnd() * .18, z + (rnd() - .5) * rz * 2);
    const k = .7 + rnd() * .7; dummy.scale.setScalar(k);
    dummy.updateMatrix(); inst.setMatrixAt(i, dummy.matrix);
  }
  inst.castShadow = true;
  parent.add(inst);
}

/* ------------------------------------------------------------- 主公寓 */
function buildApartment(scene, M, atmo) {
  const b = new Batch(M, 'sakura-apartment');
  const { x0, x1, z0, z1, floors, top } = APT;
  const cx = (x0 + x1) / 2, cz = (z0 + z1) / 2;
  const depth = x1 - x0, width = z1 - z0;
  const face = x0;             // 朝 -X 的主立面

  // 主体不是实心盒：3F 的 z∈[2,9] 留成真空腔，作为图 3 的「室内 → 阳台 → 街巷」
  // 观景单元。实心盒会直接把室内机位糊死，这是三张图里最容易漏掉的结构约束。
  const unitZ0 = 2.0;
  const segA = (z0 + unitZ0) / 2, segALen = unitZ0 - z0;
  const segB = (unitZ0 + z1) / 2, segBLen = z1 - unitZ0;
  b.box('plaster', cx, top / 2, segA, depth, top, segALen);
  b.box('plaster', cx, floors[2] / 2, segB, depth, floors[2], segBLen);   // 1F + 2F
  b.box('plaster', cx, (floors[3] + top) / 2, segB, depth, top - floors[3], segBLen); // 4F
  // 空腔内壁：背墙、两端隔墙、顶棚底面（主立面留给落地玻璃）
  b.box('plasterWarm', x1 - .11, (floors[2] + floors[3]) / 2, segB, .22, floors[3] - floors[2], segBLen);
  b.box('plasterWarm', cx, (floors[2] + floors[3]) / 2, unitZ0 + .11, depth, floors[3] - floors[2], .22);
  b.box('plasterWarm', cx, (floors[2] + floors[3]) / 2, z1 - .11, depth, floors[3] - floors[2], .22);
  b.box('woodDark', cx, floors[3] - .09, segB, depth, .18, segBLen);

  b.box('plasterSide', x1 + .02, top / 2, cz, .05, top, width);
  b.box('plasterSide', cx, top / 2, z0 - .02, depth, top, .05);
  b.box('plasterSide', cx, top / 2, z1 + .02, depth, top, .05);
  b.box('concrete', cx, .16, cz, depth + .3, .32, width + .3);

  // 屋顶挑檐 + 顶层压顶
  b.box('concrete', cx, top + .12, cz, depth + .8, .24, width + .8);
  b.box('woodDark', cx, top + .28, cz, depth + .5, .1, width + .5);
  for (const z of [z0 - .3, z1 + .3]) b.box('concrete', cx, top + .45, z, depth + .8, .34, .12);
  b.box('concrete', cx, top + .45, cz, .12, .34, width + .8);

  // 竖向木格栅竖井：图 1/2 最醒目的识别特征，从 1F 一直贯到屋顶
  const louZ0 = 5.7, louZ1 = 8.7;
  louvers(b, 'wood', 'z', louZ0, louZ1, .5, top - .15, face - .09, .12, .085);
  b.box('woodDark', face - .13, top / 2, louZ0 - .08, .1, top, .12);
  b.box('woodDark', face - .13, top / 2, louZ1 + .08, .1, top, .12);
  b.box('woodDark', face - .13, .45, (louZ0 + louZ1) / 2, .12, .18, louZ1 - louZ0);

  // 各层：落地窗 + 外挑阳台板 + 黑细栏杆 + 木格栅户间隔屏 + 盆栽
  const balcX0 = face - 1.55;
  const balcZ0 = z0 + .35, balcZ1 = louZ0 - .25;
  for (let f = 1; f <= 3; f++) {
    const F = floors[f];
    // 楼板
    b.box('concrete', (balcX0 + face) / 2, F - .1, (balcZ0 + balcZ1) / 2,
      face - balcX0 + .1, .2, balcZ1 - balcZ0);
    b.box('concrete', (balcX0 + face) / 2, F - .22, (balcZ0 + balcZ1) / 2,
      face - balcX0, .06, balcZ1 - balcZ0 - .1);
    // 窗（每层三开间，避开入口正上方留一段实墙）
    for (const wz of [-6.4, -2.3, 1.8]) {
      windowPanel(b, face - .03, F + 1.42, wz, 2.75, 2.12, -1, true);
      // 窗帘（半透米色，压在窗内侧形成层次）
      b.box('curtain', face - .005, F + 1.42, wz - 1.18, .012, 2.1, .42);
      b.box('curtain', face - .005, F + 1.42, wz + 1.18, .012, 2.1, .42);
      // 窗前空调/花槽小件
      if (f === 2 && wz === -6.4) b.box('concrete', face - .3, F + .35, wz, .5, .3, .7);
    }
    // 栏杆沿外缘
    railing(b, 'z', balcZ0, balcZ1, balcX0, F, 1.06);
    railing(b, 'x', balcX0, face, balcZ0, F, 1.06);
    railing(b, 'x', balcX0, face, balcZ1, F, 1.06);
    // 其中两段用木格栅栏板（图 1 阳台是木格栅 + 玻璃混着）
    for (const [a, zz] of [[-6.4, -5.1], [-2.3, -1.0]]) {
      const zA = a - .85, zB = zz + .85;
      if (zB > zA) louvers(b, 'wood', 'z', zA, zB, F + .12, F + 1.02, balcX0 - .03, .1, .09);
    }
    // 户间隔屏：沿 x 方向一片实心木格栅板，隔开相邻两户的阳台
    for (const pz of [-4.35, -.25]) {
      const seg = 9;
      for (let k = 0; k < seg; k++) {
        const px = balcX0 + (face - balcX0) * (k + .5) / seg;
        b.box('wood', px, F + .82, pz, (face - balcX0) / seg * .78, 1.4, .075);
      }
    }
    // 阳台花池（图 1 每个阳台都塞满植物）
    planter(b, balcX0 + .42, -7.7, .9, .55, .42, F);
    planter(b, balcX0 + .42, 3.9, 1.1, .55, .42, F);
  }

  // 1F：内凹入口 + 深挑檐 + 玻璃门 + 木格栅门 + 两侧石花池
  const entZ0 = -2.5, entZ1 = 2.5;
  // 挖凹：两侧做实墙，中间留给门洞（用两块侧墙 + 上方过梁表达）
  b.box('plasterWarm', face + .02, 1.55, (z0 + entZ0) / 2, .14, 3.1, entZ0 - z0);
  b.box('plasterWarm', face + .02, 1.55, (entZ1 + louZ0) / 2, .14, 3.1, louZ0 - entZ1);
  b.box('concrete', face + .02, 2.92, (entZ0 + entZ1) / 2, .18, .36, entZ1 - entZ0);
  // 门洞内退 1.2m
  const doorX = face + 1.2;
  b.box('woodDark', doorX, 1.5, (entZ0 + entZ1) / 2, .12, 3.0, entZ1 - entZ0);
  // 木格栅门扇（左右两片）
  louvers(b, 'wood', 'z', entZ0 + .25, -.15, .15, 2.75, doorX - .12, .09, .075);
  louvers(b, 'wood', 'z', .15, entZ1 - .25, .15, 2.75, doorX - .12, .09, .075);
  // 玻璃门 + 内厅暖光（透出图 2 那种橙光）
  b.box('windowLit', doorX - .28, 1.5, 0, .03, 2.7, 3.4);
  b.box('glass', face + .08, 1.5, 0, .03, 2.7, entZ1 - entZ0 - .2);
  b.box('blackMetal', face + .06, 1.5, 0, .06, 2.8, .05);
  // 深挑檐（图 2 那块黑色雨棚）
  b.box('blackMetal', face - 1.25, 3.05, 0, 2.9, .22, 7.6);
  b.box('woodDark', face - 1.25, 2.92, 0, 2.7, .06, 7.4);
  for (const zz of [entZ0 - .55, entZ1 + .55]) {
    b.box('concrete', face - 2.35, 1.45, zz, .34, 2.9, .34);
  }
  // 1F 两侧窗
  for (const wz of [-5.6, 3.9]) windowPanel(b, face - .03, 1.68, wz, 2.1, 1.7, -1, true);
  // 入口平台与两级台阶
  b.box('stone', face - 1.55, .1, 0, 3.4, .2, 7.4);
  b.box('concrete', face - 3.0, .055, 0, .55, .11, 5.0);
  b.box('concrete', face - 3.45, .028, 0, .5, .056, 4.6);
  // 入口石花池（图 2 左右各一，种绣球与灌木）
  planter(b, face - 2.0, -4.4, 2.5, .95, .55, 0);
  planter(b, face - 2.0, 4.4, 2.5, .95, .55, 0);
  // 壁灯柱
  for (const zz of [-3.35, 3.35]) {
    b.box('concrete', face - .12, .95, zz, .18, 1.9, .18);
    b.box('windowLit', face - .23, 1.72, zz, .05, .42, .16);
  }
  b.flush(scene);

  // 独立的暖光点光：入口两盏 + 门厅一盏（交给 atmosphere 按时段调）
  for (const [x, y, z, power] of [
    [face - .5, 2.0, -3.35, 9], [face - .5, 2.0, 3.35, 9], [face - 2.2, 2.2, 0, 12],
  ]) {
    const l = new THREE.PointLight('#ffb066', 1, 13, 2);
    l.position.set(x, y, z);
    scene.add(l);
    atmo.register(l, { day: power * .12, golden: power * .45, dusk: power });
  }

  // 阳台植物：花簇与灌木用真实几何（会投影到墙上，是图 1 的斑驳光影来源）
  const plants = new THREE.Group(); plants.name = 'apartment-planting'; scene.add(plants);
  for (let f = 1; f <= 3; f++) {
    const F = floors[f];
    for (const [px, pz, s] of [[balcX0 + .42, -7.7, 1], [balcX0 + .42, 3.9, 2], [balcX0 + .9, .6, 3]]) {
      shrubClump(plants, M, px, F + .72, pz, .32, s);
      flowerDots(plants, M, px, F + .86, pz, .42, .24, 12, s + f);
    }
  }
  for (const [px, pz, s] of [[face - 2.0, -4.4, 5], [face - 2.0, 4.4, 7]]) {
    shrubClump(plants, M, px - .6, .88, pz, .55, s);
    shrubClump(plants, M, px + .55, .82, pz + .1, .44, s + 2);
    flowerDots(plants, M, px, .94, pz, 1.0, .38, 26, s);
  }

  return { face, balcX0, balcZ0, balcZ1, floors };
}

/* ------------------------------------------------------------- 低层街屋 */
function roof(b, x0, x1, z0, z1, eaveY, ridgeY) {
  // 双坡和瓦：两片薄斜板，屋脊沿 Z，坡向 ±X
  const mid = (x0 + x1) / 2;
  const half = (x1 - x0) / 2 + .55;
  const rise = ridgeY - eaveY;
  const slope = Math.atan2(rise, half);
  const len = Math.hypot(half, rise);
  const depth = z1 - z0 + 1.0;
  for (const s of [-1, 1]) {
    const g = new THREE.BoxGeometry(len + .12, .14, depth);
    g.rotateZ(s * slope);
    g.translate(mid + s * half / 2, eaveY + rise / 2, (z0 + z1) / 2);
    b.geo('tile', g);
    g.dispose();
  }
  // 屋脊与檐口
  b.box('tile', mid, ridgeY + .05, (z0 + z1) / 2, .3, .18, depth);
  for (const xx of [x0 - .5, x1 + .5]) b.box('woodDark', xx, eaveY - .06, (z0 + z1) / 2, .12, .16, depth);
  for (const zz of [z0 - .5, z1 + .5]) b.box('woodDark', mid, eaveY + rise / 2, zz, (x1 - x0) + 1, .12, .1);
}

function shopHouse(b, M, scene, cfg, atmo) {
  const { x0, x1, z0, z1, kind, seed, facing } = cfg;
  const cx = (x0 + x1) / 2, cz = (z0 + z1) / 2;
  const H1 = 3.05, H2 = 5.75, ridge = 7.15;
  b.box('plasterSide', cx, H2 / 2, cz, x1 - x0, H2, z1 - z0);
  b.box('concrete', cx, .14, cz, x1 - x0 + .18, .28, z1 - z0 + .18);
  roof(b, x0, x1, z0, z1, H2, ridge);
  // 面向巷子的立面
  const fx = facing > 0 ? x1 : x0;
  const nx = fx + (facing > 0 ? .035 : -.035);
  // 1F：木格栅门 / 暖光橱窗
  const span = z1 - z0;
  if (kind === 'cafe') {
    // 大面积暖光玻璃 + 木框
    b.box('windowLit', nx, 1.42, cz, .04, 2.35, span * .72);
    for (const dz of [-span * .36, 0, span * .36]) b.box('woodDark', nx + facing * .03, 1.42, cz + dz, .07, 2.45, .09);
    b.box('woodDark', nx + facing * .03, 2.66, cz, .08, .12, span * .76);
    // 粉色遮阳篷（图 1 左下那家）
    const shade = new THREE.BoxGeometry(1.65, .09, span * .82);
    shade.rotateZ(facing > 0 ? -.14 : .14);
    shade.translate(nx + facing * .78, 2.78, cz);
    b.geo('flowerPink', shade);
    b.box('woodDark', nx + facing * 1.5, 1.35, cz - span * .38, .07, 2.7, .07);
    b.box('woodDark', nx + facing * 1.5, 1.35, cz + span * .38, .07, 2.7, .07);
  } else if (kind === 'shop') {
    b.box('windowLit', nx, 1.35, cz - span * .12, .04, 1.9, span * .48);
    louvers(b, 'wood', 'z', cz + span * .12, cz + span * .44, .25, 2.55, nx + facing * .02, .1, .08);
    b.box('woodDark', nx + facing * .01, 2.68, cz, .1, .14, span * .9);
  } else {
    // 住宅：木格栅门 + 两侧小窗
    louvers(b, 'wood', 'z', cz - .85, cz + .25, .12, 2.5, nx + facing * .01, .1, .075);
    for (const dz of [-span * .32, span * .34]) windowPanel(b, nx, 1.5, cz + dz, 1.15, 1.35, facing > 0 ? 1 : -1, true);
  }
  // 2F 小窗（木框 + 暖光）
  for (const dz of [-span * .24, span * .24]) {
    b.box('windowLit', nx, H1 + 1.28, cz + dz, .04, 1.25, 1.35);
    b.box('woodDark', nx + facing * .03, H1 + 1.28, cz + dz - .7, .07, 1.35, .08);
    b.box('woodDark', nx + facing * .03, H1 + 1.28, cz + dz + .7, .07, 1.35, .08);
    b.box('woodDark', nx + facing * .03, H1 + 1.94, cz + dz, .07, .1, 1.5);
    b.box('woodDark', nx + facing * .03, H1 + .62, cz + dz, .12, .1, 1.6);
  }
  // 腰线 / 檐下木带
  b.box('woodDark', nx + facing * .01, H1, cz, .1, .12, span);
  b.box('woodDark', nx + facing * .01, H2 - .12, cz, .16, .2, span + .2);

  // 院墙 + 花池，贴巷子一侧
  if (kind !== 'cafe') {
    const wx = fx + facing * .95;
    b.box('pot', wx, .42, cz - span * .42, .22, .84, span * .14);
    b.box('pot', wx, .42, cz + span * .42, .22, .84, span * .14);
    b.box('concrete', wx, .9, cz, .12, .1, span * .9);
  }
  return { fx, cx, cz };
}

export function buildStreetHouses(scene, M, atmo) {
  const b = new Batch(M, 'sakura-lowrise');
  const houses = [];
  // 左侧（x<0，立面朝 +X）
  let z = 42;
  let i = 0;
  while (z > -76) {
    const w = 6.2 + (i % 3) * 1.1;
    // z∈[1,25] 是横街路口 + 入口前广场：图 1 的相机站在这里斜看公寓。
    // 7m 宽的巷子里退不出能看全 12m 楼体的距离，且近端街屋会整片挡住前景。
    const opensToCrossStreet = z > 1 && z - w < 25;
    if (!opensToCrossStreet) {
      const kind = i === 7 ? 'cafe' : (i % 3 === 0 ? 'shop' : 'house');
      const cfg = { x0: -3.5 - (8.2 + (i % 2)), x1: -3.5 - .05, z0: z - w, z1: z, kind, seed: i, facing: 1 };
      houses.push(shopHouse(b, M, scene, cfg, atmo));
    }
    z -= w + .35; i++;
  }
  // 右侧近端（z > -12）刻意不排街屋：图 1 的巷口右侧第一栋就是公寓本身。
  // 第一版在这里排了街屋，从巷口看过去一整片瓦顶把公寓完全挡住，构图直接废掉。
  // 近端留出的空地做成入口前场，也让图 2 沿巷回望时能读到入口与纵深。
  z = -12; i = 0;
  while (z > -76) {
    const w = 6.4 + (i % 3) * 1.0;
    const kind = i % 4 === 2 ? 'shop' : 'house';
    houses.push(shopHouse(b, M, scene, { x0: 3.55, x1: 3.55 + 8.0 + (i % 2), z0: z - w, z1: z, kind, seed: 20 + i, facing: -1 }, atmo));
    z -= w + .35; i++;
  }
  b.flush(scene);

  // 咖啡店户外桌椅（图 1 左下的生活细节）
  const cafe = houses[1];
  if (cafe) {
    const t = new Batch(M, 'cafe-terrace');
    for (const [dx, dz] of [[2.1, -1.5], [2.1, 1.2], [3.5, -.2]]) {
      t.box('woodDark', cafe.fx + dx, .72, cafe.cz + dz, .78, .07, .78);
      for (const sx of [-.3, .3]) for (const sz of [-.3, .3])
        t.box('woodDark', cafe.fx + dx + sx, .36, cafe.cz + dz + sz, .06, .72, .06);
      for (const sx of [-.34, .34]) t.box('woodDark', cafe.fx + dx + sx, .95, cafe.cz + dz, .06, .5, .7);
    }
    t.flush(scene);
  }
  return houses;
}

export function buildBuildings(scene, M, atmo) {
  const apt = buildApartment(scene, M, atmo);
  const houses = buildStreetHouses(scene, M, atmo);
  return { apt, houses };
}
