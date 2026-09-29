import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import type { Collider } from './collider';
import { createPropKit } from './streetProps';

/**
 * 街区**动态**车流。
 *
 * 存在的理由是一次返工。上一轮为了治"空地太多、看起来很空旷"，沿 18 条街两侧
 * 每 12.4m 停一辆车，一共 122 辆**静止**汽车。空地表是填下去了，但街道被做成了
 * 停车场 —— 用户的原话是「不要再道路上放一堆密集静止的汽车，你可以做少量动态
 * 的汽车」。静止的车只贡献占位面积，不贡献"这条街在用"这个读数。
 *
 * 所以这一版换一条完全不同的路子：**少量车、真在开**。三条约束，每条都是踩出来的：
 *
 *  1. **必须闭环，不能折返。** 折返车开到端点要原地掉头，一眼假；走到界外再传送
 *     回起点更假。所以走线只能取**路网里真实存在的环路** —— 这也是为什么只有
 *     两圈：把 `cityRoutes.ts` 的 CITY_ROUTES / SOUTH_ROUTES 连起来能闭合的只有
 *     这两条（其余全是 T 字头的断头路）。
 *  2. **靠左行驶（左侧通行）。** 这一条**被反复核过两遍**，因为场景里的证据是冲突的：
 *
 *     **场景自己编码了"靠哪一边"的两处，都是右行** ——
 *       · 站内两轨：`trainSchedule.ts` 西轨 x=86 只向 +z（自北向南）、东轨 x=91 只向 -z；
 *         面向 +z 时右手边是 -x ⇒ 南向那列走西轨 = 靠右（两侧站台 82.47/94.55 也是
 *         标准右侧通行的布置）。
 *       · 公交站台：`cityDressing.ts` 公交车在 x=124 朝北（ry=π/2 ⇒ 世界 -z），
 *         候车亭在 x=129.6 —— 车头朝北时那是**右手边** ⇒ 靠右停靠、从右侧上车。
 *
 *     所以第二版按"右行"改过一次，**改完核账③立刻从 2.2m 掉到 0.98m** —— 两辆车在
 *     共用的 90° 转角上撞在一起。原因不是数字没调好，是**右行的左转必然横穿对向车道**：
 *       北圈在 (-46,10.1) 要从"沿 x=-46 南行"转到"沿 z=10.1 东行"，这是左转。
 *       倒角后中心线顶点 = P + (c/4)(d_out-d_in) = (-44.125, 8.225)，
 *       偏移顶点 = 中心线顶点 + LANE·外法线：
 *         · 右行 ⇒ (-43.347, 9.003)；对向（大圈北行）车道在 x=-44.90 ⇒ **同侧**，
 *           横向只差 1.553m，两条走线交叉 ⇒ 实测 0.98m；
 *         · 左行 ⇒ (-44.903, 7.447)；对向车道在 x=-47.10 ⇒ 分居中线两侧，
 *           横向 2.197m = 2·LANE ✓。
 *     左转"留在本侧"还是"横穿对向"完全取决于靠哪边 —— 这是右行在现实里的已知代价，
 *     现实靠信号灯与让行错开，而这里的走线必须是**时间的纯函数**（否则 `seek` 失效、
 *     全部无头取证作废），排不了队、也没有跟驰。
 *
 *     ⇒ **取左行。** 代价是与站内两轨、公交站台方向相反；但那两处离车流 40m 以上、
 *     中间隔着楼，同框都做不到，是既有的、看不出来的分歧。而"两辆车在路口互相穿过去"
 *     是看得出来的 —— 两害相权取其轻。
 *     两圈共用 x=-46 / x=43 两条大道与 z=-19 那条横街，**环流方向相反** ⇒ 同一段路
 *     正好是一来一去两条车道，横向间距恒为 2·LANE（核账③盯着）。
 *     判据：核账 **①c 带符号叉积** `s = dx*(z-cz) - dz*(x-cx)` —— 左行恒为 −，
 *     符号混号就是"有车在逆行车道"。① 只量距离，量不出靠哪边，看不到这一类错误。
 *  3. **不许插进既有陈设里。** 车道偏移 1.10m、车宽 1.76m ⇒ 车身占中线两侧
 *     0.22~1.98m，最窄的路（riverside-lane 4.4m，半宽 2.2）也塞得下；主街南侧
 *     路缘线上那排 2.35m 高的立杆（z=7.70）留 0.42m 净距。转角按二次贝塞尔倒角
 *     （切回 6.0m ⇒ 顶点半径约 4.2m），内外偏移点都留在路面内。
 *     这三个数字**都是核账逼出来的**，注释写在 `LANE` / `SAMPLE` / `roundCorners`
 *     上 —— 别凭感觉调，改完要重跑 `tmp/_probe-dressing.mjs` 的车流一节。
 *
 * 与电车保持同一套验收接口（`userData.live` / `userData.seek`），理由见
 * `sakuraStation.ts`：这个场景在 headless swiftshader 下只有 0.4fps、update 里的
 * dt 被夹到 0.1s ⇒ 时钟每秒只推进 0.04s，没有 seek 就没法在验收里守到"某辆车
 * 正好开进某个路口"那个瞬间。
 */

interface Pt { x: number; z: number }

interface LoopSpec {
  name: string;
  /** 走线（沿既有道路中心线的折点，单位 m）。**必须闭合**，首尾自动相连。 */
  points: [number, number][];
  /** 这一圈几辆车。 */
  cars: number;
  /** 巡航速度 m/s（约 30~34 km/h）。 */
  speed: number;
  /** 车型序列，按车序循环取。 */
  fleet: ('car' | 'van')[];
}

/**
 * 两圈环路。折点全部照抄 `cityRoutes.ts` 里对应路线的真实采样点 ——
 * 车必须压在铺装中线上，凭印象写坐标会立刻骑到人行道上去。
 */
const LOOPS: LoopSpec[] = [
  {
    /* 北街区：西大道 → 站前大街 → 东大道 → 公寓北街。
     * 这是玩家公寓（z -6..4.8）门口那一圈，也是"站在街心能看见车"的地方。 */
    name: 'traffic-north-block',
    points: [[-46, -19], [-46, 10.1], [43, 10.1], [43, -19]],
    cars: 2, speed: 8.2, fleet: ['car', 'car'],
  },
  {
    /* 大环：公寓北街 → 东大道 → 商场大道 → 市政共享街 → 校区引道 → 西大道。
     * 四条路线在端点上是**真的接得上**的（逐个交点核过）：
     *   (43,-19) 东大道起点 · (43,83) 商场大道起点 · (24,103) 商场大道 ∩ 市政街
     *   (-34,106) 市政街 ∩ 校区引道 · (-46,83) 校区引道起点 · (-46,-19) 西大道。
     * 商场大道在 (24,103) 之后是往东南的支路、市政街在 (-34,106) 之后是往西的
     * 支路，都要**截断**，否则车会拐进死胡同。 */
    name: 'traffic-grand-loop',
    points: [
      [-46, -19], [0, -19], [43, -19],                          // apartment-north-street
      [43, 10], [43, 30], [45, 46], [42, 63], [43, 83],         // east-station-avenue
      [38, 91], [29, 96], [24, 103],                            // mall-boulevard
      [15, 99], [3, 98], [-15, 103], [-34, 106],                // civic-shared-street（反向）
      [-38, 99], [-43, 91], [-46, 83],                          // campus-approach（反向）
      [-46, 65], [-44, 47], [-47, 30], [-46, 10],               // west-neighbourhood-avenue（反向）
    ],
    cars: 4, speed: 9.4, fleet: ['car', 'van', 'car', 'car'],
  },
];

/**
 * 车道偏移：中心线到车身中线。
 *
 * **1.10 是被一次真实命中逼出来的，不是随手取的。** 第一版取 1.30，核账
 * （`tmp/_probe-dressing.mjs` 的②）在主街上扫到 17 处命中 —— 那排东西是
 * 0.26×0.32m、高 2.35m 的立杆，就在站前大街南侧**路缘线**上（z=7.70，而车道
 * 中线在 z=10.1、路半宽 2.5）。1.30 的偏移让车身外沿落到 z=7.92，离杆子只剩
 * 6cm —— 判据是"探针外扩 0.95 之后仍然零命中"，6cm 过不了。
 *
 * 1.10 之后：车身占中线两侧 0.22~1.98m，路缘在 2.4m ⇒ 留 0.42m。
 * 最窄的路（riverside-lane 4.4m，半宽 2.2）也塞得下。
 * 代价是两圈共用走廊的横向间距从 2.60 降到 2.20m —— 车宽 1.76，会车净距 0.44m，
 * 仍然过得去（核账③的最小中心距会盯住这一项）。
 */
const LANE = 1.10;
/**
 * 走线重采样步长。
 *
 * **0.45 而不是 1.1，也是被数字逼出来的。** 第一版 1.1m 时，核账③量到
 * traffic-car-0/1 的"车头朝向 vs 行驶方向"最大差到 **44°** —— 看起来像车在
 * 转角上横着走。原因不是朝向算法，是**折线本身**：内侧车道在 90° 转角处的
 * 有效半径只有 ~1.9m，1.1m 一步就转 21°，两段相邻弦（朝向看 2m、速度看 1.64m）
 * 落在不同的段上，方向就能差出 40° 去。加密到 0.45m 之后单段转角降到 8~16°，
 * 弦向平均掉了大部分折线化误差。折点疏密不均，先归一化再取法线，偏移才不会抖。
 */
const SAMPLE = 0.45;

/**
 * 转角倒角：每个折点换成一小段二次贝塞尔（控制点就是折点本身），切回距离 `cut`。
 *
 * 直接拿折点串插值的话，车到 90° 转角会**瞬间**掉头。倒角之后折点中点的偏移量是
 * `cut·|dout-din|/4`，顶点处的曲率半径是 `cut/√2 ≈ 0.707·cut`：
 *   cut=4.5 ⇒ R≈3.2m（内侧减掉车道偏移只剩 1.9m，单段转角 21°，太急）
 *   cut=7.5 ⇒ R≈5.3m（内侧 4.2m，单段转角 12° 上下）
 * 切回距离同时被压到相邻段长的 45% 以内 —— 两头的倒角加起来 ≤ 段长的 90%，
 * 不会撞在一起（这是上限，不能取 0.5）。
 * 上限还有个副作用：环路南圈在 (24,103)/(-34,106) 两处相邻段只有 8~9m，
 * `cut` 被压到 3.2~3.9，那两处的单段转角是整条走线里最大的（核账①b 盯着它）。
 */
function roundCorners(points: [number, number][], cut = 7.5, samples = 10): Pt[] {
  const n = points.length, out: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const vx = points[i][0], vz = points[i][1];
    const px = points[(i - 1 + n) % n][0], pz = points[(i - 1 + n) % n][1];
    const qx = points[(i + 1) % n][0], qz = points[(i + 1) % n][1];
    const lp = Math.hypot(vx - px, vz - pz), lq = Math.hypot(qx - vx, qz - vz);
    if (lp < 1e-6 || lq < 1e-6) { out.push({ x: vx, z: vz }); continue; }
    const c = Math.min(cut, lp * .45, lq * .45);
    const ax = vx - (vx - px) / lp * c, az = vz - (vz - pz) / lp * c;
    const bx = vx + (qx - vx) / lq * c, bz = vz + (qz - vz) / lq * c;
    for (let k = 0; k <= samples; k++) {
      const t = k / samples, u = 1 - t;
      out.push({ x: u * u * ax + 2 * u * t * vx + t * t * bx, z: u * u * az + 2 * u * t * vz + t * t * bz });
    }
  }
  return out;
}

/** 按弧长等间距重采样（闭合折线）。 */
function resample(pts: Pt[], step: number): Pt[] {
  const n = pts.length, out: Pt[] = [];
  let carry = 0;
  for (let i = 0; i < n; i++) {
    const a = pts[i], b = pts[(i + 1) % n];
    const d = Math.hypot(b.x - a.x, b.z - a.z);
    if (d < 1e-6) continue;
    let t = carry;
    while (t < d) { out.push({ x: a.x + (b.x - a.x) * t / d, z: a.z + (b.z - a.z) * t / d }); t += step; }
    carry = t - d;
  }
  return out;
}

/** 沿折线**左侧**偏移 `lane`（靠左行驶）。左法线 = (dz, -dx)。
 *
 * 行进方向 `d=(dx,dz)`、上方向 `u=(0,1,0)` ⇒ 右手边 = `d × u`（在 (x,z) 平面上是
 * `(-dz, dx)`），左法线取反。为什么是左行而不是右行，见文件头第 2 条 —— 那是核过
 * 两遍、并且被核账③逼出来的结论，不是风格选择。 */
function offsetLeft(pts: Pt[], lane: number): Pt[] {
  const n = pts.length, out: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const a = pts[(i - 1 + n) % n], b = pts[(i + 1) % n];
    const dx = b.x - a.x, dz = b.z - a.z;
    const l = Math.hypot(dx, dz) || 1;
    out.push({ x: pts[i].x + dz / l * lane, z: pts[i].z - dx / l * lane });
  }
  return out;
}

const scratchA: Pt = { x: 0, z: 0 }, scratchB: Pt = { x: 0, z: 0 };

/** 闭合走线：弧长 ↔ 位置。 */
class Track {
  readonly pts: Pt[];
  /** cum[i] = 从起点到 pts[i] 的累计弧长；cum 末位 = 总长。 */
  readonly cum: number[];
  readonly total: number;
  constructor(pts: Pt[]) {
    this.pts = pts;
    this.cum = [0];
    let s = 0;
    for (let i = 0; i < pts.length; i++) {
      const a = pts[i], b = pts[(i + 1) % pts.length];
      s += Math.hypot(b.x - a.x, b.z - a.z);
      this.cum.push(s);
    }
    this.total = s;
  }
  /** 弧长 → 位置（闭合，自动取模）。二分找段：百来个点，比线性扫省且不会错边界。 */
  at(s: number, out: Pt) {
    const total = this.total;
    s = ((s % total) + total) % total;
    const cum = this.cum;
    let lo = 0, hi = cum.length - 1;
    while (lo + 1 < hi) { const mid = (lo + hi) >> 1; if (cum[mid] <= s) lo = mid; else hi = mid; }
    const a = this.pts[lo], b = this.pts[(lo + 1) % this.pts.length];
    const seg = cum[lo + 1] - cum[lo] || 1, t = (s - cum[lo]) / seg;
    out.x = a.x + (b.x - a.x) * t;
    out.z = a.z + (b.z - a.z) * t;
    return out;
  }
  /**
   * 朝前看 `ahead` 米算朝向。
   *
   * 车身几何的车头朝**本地 +x**（见 `streetProps.ts` 的 `carLocal`：前风挡在
   * x=+1.10、前保险杠在 +2.30、尾灯在 -2.62）。绕 y 转 `ry` 之后本地 +x 落到
   * `(cos ry, 0, -sin ry)`，所以 `ry = atan2(-dz, dx)` —— 写成 `atan2(dx, dz)`
   * 车会横着开。
   *
   * 用"朝前看点"而不是相邻折点：转角处折点方向是跳变的，看点得到的朝向是平滑的。
   * 看点距离取 1.4m —— 弦向 ≈ 弦中点处的切向，等于把折线化误差按 `ahead/2/SAMPLE`
   * 段做了一次滑动平均（1.4/2/0.45 ≈ 1.6 段），同时只引入 `ahead/2/R` 的"提前转向"
   * 量。`ahead` 调大更平滑但转得更早（看起来像提前拐），调小则折线感回来。
   */
  heading(s: number, ahead = 1.4) {
    this.at(s, scratchA);
    this.at(s + ahead, scratchB);
    return Math.atan2(-(scratchB.z - scratchA.z), scratchB.x - scratchA.x);
  }
}

export interface TrafficPlan {
  /** 环路名。 */
  loops: string[];
  /** 环路总长（m）。 */
  lengths: number[];
  cars: number;
  vans: number;
  lane: number;
  hand: 'left';
}

export interface StreetTrafficHandle {
  /** 恒为空数组：动态车流**不进**静态碰撞表（车每帧都在动，烘进静态表没有意义）。
   *  留着这个字段是为了和别的分区模块返回形状一致，调用方可以无条件展开。 */
  colliders: Collider[];
  /** 每帧重建的车身碰撞盒，供 `districtArt` 合成后交给 RoomScene。 */
  dynamicColliders: Collider[];
  plan: TrafficPlan;
  update(dt: number, camera: THREE.Camera): void;
  dispose(): void;
}

interface Car {
  group: THREE.Group;
  track: Track;
  loop: number;
  kind: 'car' | 'van';
  /** 起点弧长：均匀铺在这一圈上。 */
  s0: number;
  /** 巡航速度 m/s。 */
  speed: number;
  /** 当前弧长 = s0 + speed × clock（clock 的纯函数 ⇒ seek 才是确定性的）。 */
  s: number;
  len: number;
  wid: number;
  hgt: number;
  box: Collider;
}

/**
 * 把一辆车内部按材质合批（**一件道具内部合、不跨车合** —— 与 `merge.ts` 的边界一致）。
 *
 * 不用 `merge.ts` 的 `mergeByMaterial`，两个原因都是硬的：
 *  1. 它按"本次调用内这个几何被引用几次"决定释放。零件库的 `cube / cyl / sph / ring`
 *     是六辆车**共用**的实例 —— 逐辆调用时第一辆合完就把共享几何 dispose 了，
 *     后面五辆跟着变空白。
 *  2. 六辆一起调又会把结果挂到 root 上（收尾是 `root.add(mesh)`），每辆车就不再是
 *     一个能单独移动的组，车流当场失效。
 * 所以这里只做"去索引 + 烘局部变换 + 合并 + 摘掉原件"，**一个几何都不 dispose**；
 * 合并出来的几何收进 `sink`，最终由 `dispose()` 统一释放。
 *
 * 一辆车从 21 个 mesh 降到 7 个（每种材质一个）。这个场景的瓶颈是提交次数而不是
 * 填充率（见 `merge.ts` 文件头），六辆就是省下 84 次 draw call。
 */
function mergeVehicle(group: THREE.Group, sink: THREE.BufferGeometry[]) {
  const buckets = new Map<THREE.Material, THREE.Mesh[]>();
  for (const child of [...group.children]) {
    const mesh = child as THREE.Mesh;
    if (!mesh.isMesh || !mesh.geometry) continue;
    if (Array.isArray(mesh.material) || mesh.material.transparent) continue;
    const list = buckets.get(mesh.material);
    if (list) list.push(mesh); else buckets.set(mesh.material, [mesh]);
  }
  for (const [material, meshes] of buckets) {
    if (meshes.length < 2) continue;
    // 索引态必须统一：倒角/圆柱是索引几何，混着合会被 mergeGeometries 直接拒掉。
    const geos = meshes.map((m) => {
      m.updateMatrix();
      const g = m.geometry.index ? m.geometry.toNonIndexed() : m.geometry.clone();
      return g.applyMatrix4(m.matrix);
    });
    const combined = mergeGeometries(geos, false);
    for (const g of geos) g.dispose();
    if (!combined) continue;
    const out = new THREE.Mesh(combined, material);
    out.castShadow = meshes[0].castShadow;
    out.receiveShadow = meshes[0].receiveShadow;
    out.name = `merged:${group.name}`;
    for (const m of meshes) group.remove(m);
    group.add(out);
    sink.push(combined);
  }
}

export function createStreetTraffic(scene: THREE.Scene): StreetTrafficHandle {
  const root = new THREE.Group();
  root.name = 'street-traffic';
  scene.add(root);

  /* 车身几何走**自己的**零件库实例：`createPropKit` 的材质与几何归调用方 dispose，
   * 而这一批永远不能进 `mergeByMaterial`（车每帧都在动，变换烘不进顶点）。
   * 碰撞盒数组丢给一个空壳 —— 静态表里不该有会动的东西。 */
  const kit = createPropKit(root, []);

  const tracks: Track[] = [];
  /** 未偏移的中心线，只留给核账脚本对账用（"偏移量到底是不是 lane"）。 */
  const centers: Pt[][] = [];
  /** 逐辆车合批出来的几何。它们不在 `kit.geometries` 里，dispose 时要单独放。 */
  const mergedGeos: THREE.BufferGeometry[] = [];
  const cars: Car[] = [];
  const PAINTS = 8;

  LOOPS.forEach((spec, li) => {
    const center = resample(roundCorners(spec.points), SAMPLE);
    const track = new Track(offsetLeft(center, LANE));
    centers.push(center);
    tracks.push(track);
    for (let k = 0; k < spec.cars; k++) {
      const kind = spec.fleet[k % spec.fleet.length];
      const paint = (li * 3 + k) % PAINTS;
      const group = kind === 'van' ? kit.vanBody(0, 0, 0, paint) : kit.carBody(0, 0, 0, paint);
      group.name = `traffic-${kind}-${cars.length}`;
      mergeVehicle(group, mergedGeos);
      const isVan = kind === 'van';
      cars.push({
        group, track, loop: li, kind,
        s0: track.total * (k / spec.cars),
        /* **同圈等速**，这一条是被算出来的、不是图省事：
         *
         * 同圈车只要速度不同，快的那辆就一定会追上慢的那辆 —— 相对速度 5%（0.4 m/s）
         * 追 118m 的间距只要 ~5 分钟，追上之后两辆车就会**叠在一起开**。而"追上前车
         * 就减速"那种跟驰逻辑会让 `s` 不再是 clock 的纯函数，`seek` 当场失效。
         *
         * 等速的代价是整圈车距固定，但 2 辆车隔 118m、4 辆车隔 110m，玩家看到的是
         * "过十来秒来一辆"，看不出链条。不同**圈**之间速度是差的（8.2 vs 9.4）。 */
        speed: spec.speed,
        s: 0,
        len: isVan ? 5.20 : 4.30, wid: isVan ? 1.94 : 1.76, hgt: isVan ? 2.78 : 1.42,
        box: { source: `traffic-${kind}`, kind: 'wall', min: new THREE.Vector3(), max: new THREE.Vector3() },
      });
    }
  });

  const dynamicColliders: Collider[] = [];
  const scratch: Pt = { x: 0, z: 0 };

  function pose(c: Car) {
    c.track.at(c.s, scratch);
    const ry = c.track.heading(c.s);
    c.group.position.set(scratch.x, 0, scratch.z);
    c.group.rotation.y = ry;
    /* 车身 4.3m 长，不能只转 footprint：按 ry 真算一次世界 AABB。
     * 与 `streetProps.collider()` 同一套算法（w 沿本地 x、d 沿本地 z），免得两处对不上。 */
    const co = Math.abs(Math.cos(ry)), si = Math.abs(Math.sin(ry));
    const hx = (co * c.len + si * c.wid) / 2, hz = (si * c.len + co * c.wid) / 2;
    c.box.min.set(scratch.x - hx, 0, scratch.z - hz);
    c.box.max.set(scratch.x + hx, c.hgt, scratch.z + hz);
  }

  const plan: TrafficPlan = {
    loops: LOOPS.map((l) => l.name),
    lengths: tracks.map((t) => +t.total.toFixed(1)),
    cars: cars.length,
    vans: cars.filter((c) => c.kind === 'van').length,
    lane: LANE,
    hand: 'left',
  };
  root.userData.plan = plan;
  /* 走线发一份到 userData：核账脚本要拿它判"车有没有压在路面上"。
   * 车身组的名字是 `traffic-<kind>-<i>`，脚本按名寻址读位置。
   * `center` 是未偏移的中心线 —— 有它脚本才能核"偏移量确实等于 lane"。 */
  root.userData.paths = tracks.map((t, i) => ({
    name: LOOPS[i].name, total: +t.total.toFixed(1), lane: LANE,
    points: t.pts.map((p) => [+p.x.toFixed(2), +p.z.toFixed(2)]),
    center: centers[i].map((p) => [+p.x.toFixed(2), +p.z.toFixed(2)]),
  }));
  /* 当前状态的**只读快照**（同一个对象每帧改字段，不重新分配）。 */
  const live = { cars: cars.length, clock: 0, blocked: false, moving: cars.length };
  root.userData.live = live;
  let clock = 0;

  /* 相位跳转，只给验收脚本用。理由同 sakuraStation：headless 下时钟每秒只推进
   * 0.04s，想守到"某辆车正好开进某个路口"要等十几分钟。有了 seek，"t 时刻车在哪"
   * 变成确定性、瞬时的 —— 每辆车的弧长就是 `s0 + speed × clock`。 */
  root.userData.seek = (t: number) => {
    clock = t;
    for (const c of cars) { c.s = c.s0 + c.speed * clock; pose(c); }
  };

  const blockedByLoop: boolean[] = LOOPS.map(() => false);

  function update(dt: number, camera: THREE.Camera) {
    const p = camera.position;
    for (let i = 0; i < blockedByLoop.length; i++) blockedByLoop[i] = false;
    /* 行人礼让：玩家站在车头前方 9m、横向 1.9m 的走廊里，**这一圈**的车全部停下。
     *
     * 停一整圈而不是只停挡路那一辆，是因为同圈的车互不知情 —— 只停一辆的话后面
     * 那辆会径直穿过去。电车遇到玩家占轨也是整张时刻表暂停，同一套取舍。
     * 只看"地面上的相机"：观察者模式能飞，y>3 不算挡路。 */
    if (p.y < 3) {
      for (const c of cars) {
        if (blockedByLoop[c.loop]) continue;
        const dx = p.x - c.group.position.x, dz = p.z - c.group.position.z;
        const fx = Math.cos(c.group.rotation.y), fz = -Math.sin(c.group.rotation.y);
        const fwd = fx * dx + fz * dz, lat = Math.abs(-fz * dx + fx * dz);
        if (fwd > -1.5 && fwd < 9 && lat < 1.9) blockedByLoop[c.loop] = true;
      }
    }
    let any = false;
    for (let i = 0; i < blockedByLoop.length; i++) if (blockedByLoop[i]) any = true;
    /* `userData.freeze` 是给无头取证的第三个钩子（前两个是 `seek` 与场景里的
     * `__PIN`）：这个场景 headless 只有 0.4fps，一张图要等十几秒，等出来车早开出
     * 画框了。冻住时钟之后 `seek(t)` 定住的位置就一直定在那里。
     * 注意是冻**时钟**而不是跳过 pose —— pose 每帧照跑，只是弧长不变（幂等）。 */
    if (!any && root.userData.freeze !== true) clock += Math.min(dt, .1);
    dynamicColliders.length = 0;
    for (const c of cars) {
      c.s = c.s0 + c.speed * clock;
      pose(c);
      dynamicColliders.push(c.box);
    }
    live.clock = clock; live.blocked = any; live.moving = any ? 0 : cars.length;
  }

  /* 开局先摆一次，载入画面里车就在路上而不是全挤在原点。 */
  for (const c of cars) { c.s = c.s0; pose(c); }

  return {
    colliders: [],
    dynamicColliders,
    plan,
    update,
    dispose() {
      scene.remove(root);
      for (const g of kit.geometries) g.dispose();
      for (const g of mergedGeos) g.dispose();
      for (const m of kit.materials) m.dispose();
    },
  };
}
