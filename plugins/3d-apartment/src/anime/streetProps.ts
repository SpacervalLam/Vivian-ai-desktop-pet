import * as THREE from 'three';
import type { Collider } from './collider';
import { createLeafField, type SprayField } from './foliage';

/**
 * 街道陈设零件库。
 *
 * 为什么单独开一个文件：这一轮要往街区里补的量级是"几百件小道具"（车、灯、
 * 垃圾桶、护柱、自行车、候车亭、配电箱…），而它们会同时被**站前广场**和
 * **沿街陈设**两个放置器用到。把零件库抽出来的理由和 `foliage.ts` 当初抽出来的
 * 理由完全一样：同一个东西在两个地方各写一份，两边必然慢慢长得不一样
 * —— 场景里已经有三种自动售货机的先例了（见 exterior / sakuraTown）。
 *
 * 三条实现约定：
 *
 *  1. **几何全部共用**。整个库里只有 cube / cylinder / sphere / torus 四个基元，
 *     每件道具靠 `scale` 变形。合批之后一个"车"只会贡献材质数个网格，
 *     几百辆车也不会把 draw call 拉起来。
 *  2. **材质全部共用**，且 `outlineWeight = 0`。这一代新内容（urbanInfill /
 *     civicQuarter / worldStreetLife）都不描边；描边壳是**逐材质**合批的，
 *     给它加上去会让陈设那点体量被一圈黑线吃掉，在 100m 外反而更糊。
 *  3. **细杆一律不进碰撞表**。项目既有约定：一根 0.08m 的杆子不该拦住玩家
 *     （见 Method A16b）。所以路灯 / 护柱 / 站牌 / 信号灯只出几何，不出碰撞盒；
 *     车、公交、候车亭、配电箱、垃圾桶这些"真的挡路"的才登记。
 */

/** 道具外观配色。集中在这里，免得两个放置器各挑一套。 */
const PAINT = ['#c9cdd2', '#e9e5dd', '#2f3b46', '#8d3b39', '#33556e', '#4a5a4a', '#d9b45c', '#3a3a3e'];

export interface PropKit {
  readonly root: THREE.Group;
  readonly materials: THREE.Material[];
  readonly geometries: Set<THREE.BufferGeometry>;
  /** 轿车。`paint` 取 0..7，不传就按坐标散列选一个。 */
  car(x: number, z: number, ry: number, paint?: number): void;
  /** 同上，但**不登记碰撞盒**，返回车身组。动态车流用：车每帧都在动，
   *  在静态表里留一份没有意义（它的碰撞盒由调用方每帧推进 `dynamicColliders`）。 */
  carBody(x: number, z: number, ry: number, paint?: number): THREE.Group;
  /** 厢式货车 / 小型配送车。 */
  van(x: number, z: number, ry: number, paint?: number): void;
  /** 同 `van`，不登记碰撞盒。 */
  vanBody(x: number, z: number, ry: number, paint?: number): THREE.Group;
  /** 单层公交。 */
  bus(x: number, z: number, ry: number): void;
  /** 自行车（斜靠架上的那种）。 */
  bike(x: number, z: number, ry: number, tone?: number): void;
  /** 路灯：5.6m 杆 + 悬臂 + 灯头。`ry` 是悬臂指向。 */
  lamp(x: number, z: number, ry: number, height?: number): void;
  /** 垃圾桶。 */
  bin(x: number, z: number): void;
  /** 护柱（细杆，不登记碰撞）。 */
  bollard(x: number, z: number): void;
  /** 长椅：座面 + 靠背 + 铸铁脚。 */
  bench(x: number, z: number, ry: number): void;
  /** 自行车停放架：`slots` 个卡位。 */
  rack(x: number, z: number, ry: number, slots: number): void;
  /** 花箱：石箱 + 叶簇（叶簇由调用方在合批后挂，这里只出箱体与枝条）。 */
  planter(x: number, z: number, w: number, ry: number): void;
  /** 消火栓。 */
  hydrant(x: number, z: number): void;
  /** 配电箱 / 通讯柜。 */
  cabinet(x: number, z: number, ry: number, w?: number, h?: number): void;
  /** 候车亭：`len` 长、`ry` 朝向。 */
  shelter(x: number, z: number, ry: number, len: number): void;
  /** 站牌 / 指示牌：细杆 + 牌面。 */
  signpost(x: number, z: number, ry: number, h: number, w: number, board: THREE.Material): void;
  /** 自动售货机（补在空旷人行道上，靠箱体本身挡人）。 */
  vending(x: number, z: number, ry: number): void;
  /** 屋顶杂物：水箱 + 空调外机 + 天线。给低层建筑的屋面加"生活感"。 */
  rooftop(x: number, z: number, w: number, d: number, topY: number): void;
  /** 跨街拉旗。`count` 面。 */
  banner(x0: number, z0: number, x1: number, z1: number, y: number, count: number): void;
  /** 施工围挡段（板 + 立柱），`len` 沿 x。 */
  hoarding(x: number, z: number, ry: number, len: number): void;
  /** 报刊亭 / 小卖亭：`board` 是正面招牌材质。 */
  kiosk(x: number, z: number, ry: number, board: THREE.Material): void;
  /** 绿篱段，`len` 沿 x。填缝用：便宜、不抢高度、把空地切成有界两块。 */
  hedge(x: number, z: number, ry: number, len: number): void;
  /**
   * 绿篱的叶簇场。
   *
   * 冠簇是 `InstancedMesh`，而 `mergeByMaterial` 只按 `isMesh` 过滤 —— 实例网格
   * 也是 `isMesh`，混在合批里会被当成一块几何烘掉。所以绿篱只往里 `spray()`，
   * 由调用方在**自己合批之后**调 `build()` 挂进场景（和树的冠簇同一条约束）。
   * 没有绿篱的调用方不要调，空场 `build()` 会白建一个 0 实例的网格。
   */
  readonly hedgeLeaf: SprayField;
  /** 街头球场：场地 + 围网 + 两个篮架。一件圈住 `w×d` 整块地。 */
  court(x: number, z: number, w: number, d: number, ry: number): void;
  /** 邮筒。 */
  mailbox(x: number, z: number, ry: number): void;
  /** 街钟：细杆 + 双面钟面（不登记碰撞）。 */
  clock(x: number, z: number, h: number, ry: number): void;
  /** 信号灯：竖杆 + 悬臂 + 三灯头 + 行人灯（细杆，不登记碰撞）。 */
  trafficLight(x: number, z: number, ry: number, h: number): void;
  /** 儿童游乐：滑梯台 + 斜梯 + 滑道 + 两个弹簧摇马。 */
  playUnit(x: number, z: number, ry: number): void;
}

/**
 * 造一套零件库。
 *
 * @param root      所有几何挂到这个组下面（调用方负责合批）
 * @param colliders 登记碰撞盒的数组（调用方负责并进 fpsColliders）
 */
export function createPropKit(root: THREE.Group, colliders: Collider[]): PropKit {
  const materials: THREE.Material[] = [];
  const geometries = new Set<THREE.BufferGeometry>();
  const mat = (color: string, opts: { emissive?: string; intensity?: number; rough?: number; metal?: number } = {}) => {
    const m = new THREE.MeshStandardMaterial({
      color, roughness: opts.rough ?? .85, metalness: opts.metal ?? 0,
      ...(opts.emissive ? { emissive: opts.emissive, emissiveIntensity: opts.intensity ?? .8 } : {}),
    });
    m.userData.outlineWeight = 0;
    materials.push(m);
    return m;
  };

  const M = {
    paints: PAINT.map((c) => mat(c, { rough: .42, metal: .18 })),
    glass: mat('#243842', { rough: .22, metal: .3 }),
    tyre: mat('#1d2024', { rough: .95 }),
    chrome: mat('#9aa4ab', { rough: .35, metal: .45 }),
    dark: mat('#2b3238', { rough: .8 }),
    stone: mat('#b6b5ad', { rough: .9 }),
    kerb: mat('#a4a69e', { rough: .9 }),
    wood: mat('#8a6a4e', { rough: .88 }),
    iron: mat('#3d4d55', { rough: .7, metal: .2 }),
    green: mat('#5d7a52', { rough: .95 }),
    clay: mat('#ad795d', { rough: .9 }),
    lamp: mat('#ffe6bb', { emissive: '#ffd9a0', intensity: 1.35, rough: .6 }),
    redLamp: mat('#e0553f', { emissive: '#e0553f', intensity: 1.1, rough: .6 }),
    white: mat('#efeade', { rough: .85 }),
    yellow: mat('#e3b447', { rough: .7 }),
    blue: mat('#3f6b93', { rough: .7 }),
    tarp: mat('#5d6b74', { rough: .95 }),
    sand: mat('#c7bb9c', { rough: .95 }),
    court: mat('#4d6b5d', { rough: .95 }),
  };

  /* 绿篱的叶簇走 `foliage` 的叶簇场（带叶脉的 alphaTest 面片），和行道树、公园
   * 绿树同一套素材 —— 原来这里是一串压扁的 `SphereGeometry`，连成一排就是一排气球。
   * 用**自己的** PRNG：叶簇贴图生成要抽几百次样，蹭调用方的 `random()` 会把
   * 调用点后面所有随机内容整体错位。 */
  let hedgeSeed = 5521;
  const hedgeRandom = () => { hedgeSeed = (Math.imul(hedgeSeed, 1664525) + 1013904223) >>> 0; return hedgeSeed / 4294967296; };
  // 每 0.45m 撒 4 簇；街区里绿篱在几十处量级，24k 足够且只占几百 KB 实例矩阵。
  const hedgeLeaf = createLeafField(hedgeRandom, 24000);

  const cube = new THREE.BoxGeometry(1, 1, 1);
  const cyl = new THREE.CylinderGeometry(1, 1, 1, 12);
  const cyl6 = new THREE.CylinderGeometry(1, 1, 1, 6);
  const sph = new THREE.SphereGeometry(1, 10, 7);
  const ring = new THREE.TorusGeometry(1, .1, 5, 14);
  for (const g of [cube, cyl, cyl6, sph, ring]) geometries.add(g);

  function add(geo: THREE.BufferGeometry, m: THREE.Material, x: number, y: number, z: number, sx: number, sy: number, sz: number, shadow = false) {
    const o = new THREE.Mesh(geo, m);
    o.position.set(x, y, z);
    o.scale.set(sx, sy, sz);
    o.castShadow = shadow;
    o.receiveShadow = true;
    root.add(o);
    return o;
  }

  /**
   * 道具在**本地坐标系**里造，最后整体绕 y 旋转。
   *
   * 直接在世界坐标里按 `ry` 展开每个零件要写十几处 `cos/sin`，改一次朝向就错一处；
   * 用一个临时组把朝向隔离掉之后，零件内部永远按"车头朝 +x / 面向 +z"写，
   * 想调朝向只动这一个 `rotation.y`。
   */
  function place(x: number, z: number, ry: number, build: (g: THREE.Group) => void) {
    const g = new THREE.Group();
    g.position.set(x, 0, z);
    g.rotation.y = ry;
    root.add(g);
    build(g);
    return g;
  }

  /* ---------- 车族 ---------- */

  function wheel(x: number, y: number, z: number, r: number, w: number) {
    const o = add(cyl, M.tyre, x, y, z, r, w, r, false);
    o.rotation.z = Math.PI / 2;
    add(cyl, M.chrome, x, y, z, r * .45, w * 1.05, r * .45, false).rotation.z = Math.PI / 2;
  }

  function carLocal(g: THREE.Group, paint: number) {
    const body = M.paints[paint % M.paints.length];
    const L = 4.30, W = 1.76;
    const put = (m: THREE.Material, x: number, y: number, z: number, w: number, h: number, d: number, shadow = false) => {
      const o = new THREE.Mesh(cube, m); o.position.set(x, y, z); o.scale.set(w, h, d); o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
    };
    put(body, 0, .62, 0, L, .60, W, true);                    // 车身
    put(body, -.10, 1.06, 0, 2.34, .50, W * .90, true);        // 车顶
    put(M.glass, -.10, 1.06, W * .455, 2.16, .42, .04);        // 侧窗
    put(M.glass, -.10, 1.06, -W * .455, 2.16, .42, .04);
    put(M.glass, 1.10, 1.02, 0, .06, .40, W * .80);            // 前后风挡
    put(M.glass, -1.32, 1.02, 0, .06, .40, W * .80);
    put(M.dark, 0, .36, 0, L * .96, .22, W * .96);             // 底裙
    put(M.chrome, L / 2 - .06, .50, 0, .12, .16, W * .98);     // 保险杠
    put(M.chrome, -L / 2 + .06, .50, 0, .12, .16, W * .98);
    put(M.lamp, L / 2 - .02, .68, W * .30, .05, .13, .30);     // 前照灯
    put(M.lamp, L / 2 - .02, .68, -W * .30, .05, .13, .30);
    put(M.redLamp, -L / 2 + .02, .70, W * .32, .05, .13, .28); // 尾灯
    put(M.redLamp, -L / 2 + .02, .70, -W * .32, .05, .13, .28);
    for (const sx of [1, -1]) for (const sz of [1, -1]) wheel(sx * 1.32, .32, sz * (W / 2 - .07), .32, .20);
  }

  function vanLocal(g: THREE.Group, paint: number) {
    const body = M.paints[paint % M.paints.length];
    const L = 5.20, W = 1.94, H = 2.16;
    const put = (m: THREE.Material, x: number, y: number, z: number, w: number, h: number, d: number, shadow = false) => {
      const o = new THREE.Mesh(cube, m); o.position.set(x, y, z); o.scale.set(w, h, d); o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
    };
    put(body, -.55, .62 + H / 2, 0, L - 1.6, H, W, true);       // 厢体
    put(body, 1.42, .62 + .78, 0, 1.7, 1.56, W * .98, true);    // 驾驶室
    put(M.glass, 2.24, 1.62, 0, .06, .78, W * .86);             // 前风挡
    put(M.glass, 1.42, 1.62, W * .495, 1.5, .70, .04);
    put(M.glass, 1.42, 1.62, -W * .495, 1.5, .70, .04);
    put(M.white, -.55, .62 + H - .22, 0, L - 1.7, .44, W * 1.01);// 厢体上白条
    put(M.chrome, 2.30, .52, 0, .14, .20, W * .98);
    put(M.lamp, 2.28, .86, W * .32, .05, .16, .34);
    put(M.lamp, 2.28, .86, -W * .32, .05, .16, .34);
    put(M.redLamp, -2.62, .92, W * .34, .05, .18, .32);
    put(M.redLamp, -2.62, .92, -W * .34, .05, .18, .32);
    for (const sx of [1.5, -1.9]) for (const sz of [1, -1]) wheel(sx, .36, sz * (W / 2 - .08), .36, .24);
  }

  function busLocal(g: THREE.Group) {
    const L = 11.2, W = 2.55, H = 2.60;
    const body = M.paints[3];
    const put = (m: THREE.Material, x: number, y: number, z: number, w: number, h: number, d: number, shadow = false) => {
      const o = new THREE.Mesh(cube, m); o.position.set(x, y, z); o.scale.set(w, h, d); o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
    };
    put(body, 0, .60 + H / 2, 0, L, H, W, true);
    put(M.white, 0, .60 + H - .30, 0, L * 1.002, .58, W * 1.002);   // 白色顶带
    for (const sz of [1, -1]) {                                    // 侧窗带
      put(M.glass, 0, 1.92, sz * (W / 2 + .01), L - 1.0, .92, .04);
      for (let n = 0; n < 8; n++) put(M.white, -L / 2 + 1.0 + n * 1.28, 1.92, sz * (W / 2 + .03), .07, .92, .03);
    }
    put(M.glass, L / 2 + .01, 1.92, 0, .05, 1.05, W - .30);        // 前风挡
    put(M.glass, -L / 2 - .01, 1.92, 0, .05, .95, W - .40);        // 后窗
    put(M.dark, 0, .58, 0, L * .99, .30, W * .99);
    put(M.lamp, L / 2 + .02, .78, W * .32, .05, .20, .40);
    put(M.lamp, L / 2 + .02, .78, -W * .32, .05, .20, .40);
    put(M.redLamp, -L / 2 - .02, .95, W * .34, .05, .24, .36);
    put(M.redLamp, -L / 2 - .02, .95, -W * .34, .05, .24, .36);
    for (const sx of [3.6, -3.4]) for (const sz of [1, -1]) wheel(sx, .48, sz * (W / 2 - .12), .48, .30);
    // 车门：两处，压在前轮之后
    for (const dx of [2.55, -1.05]) put(M.glass, dx, 1.85, W / 2 + .02, 1.05, 2.0, .04);
  }

  function bikeLocal(g: THREE.Group, tone: number) {
    const frame = tone % 2 ? M.iron : M.paints[3];
    const wheelR = .34;
    for (const dx of [.52, -.52]) {
      const o = new THREE.Mesh(ring, M.dark);
      o.position.set(dx, wheelR, 0); o.scale.set(wheelR, wheelR, wheelR); o.rotation.y = Math.PI / 2; g.add(o);
    }
    const rod = (a: number[], b: number[], r: number, m: THREE.Material) => {
      const v = new THREE.Vector3(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
      const o = new THREE.Mesh(cyl, m);
      o.position.set(a[0] + v.x / 2, a[1] + v.y / 2, a[2] + v.z / 2);
      o.scale.set(r, v.length(), r);
      o.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), v.normalize());
      g.add(o);
    };
    rod([-.52, wheelR, 0], [-.10, .86, 0], .028, frame);
    rod([-.10, .86, 0], [.44, .80, 0], .028, frame);
    rod([.44, .80, 0], [.52, wheelR, 0], .026, frame);
    rod([-.10, .86, 0], [.30, .42, 0], .026, frame);
    rod([.30, .42, 0], [.52, wheelR, 0], .024, frame);
    rod([.44, .80, 0], [.46, 1.02, 0], .022, M.chrome);
    rod([.46, 1.02, 0], [.46, 1.02, .42], .02, M.chrome);
    rod([.46, 1.02, 0], [.46, 1.02, -.42], .02, M.chrome);
    const seat = new THREE.Mesh(cube, M.dark); seat.position.set(-.16, 1.02, 0); seat.scale.set(.34, .07, .16); g.add(seat);
    const basket = new THREE.Mesh(cube, M.iron); basket.position.set(.62, .92, 0); basket.scale.set(.28, .22, .40); g.add(basket);
  }

  /* ---------- 独立放置 ---------- */

  function carBody(x: number, z: number, ry: number, paint?: number) {
    const p = paint ?? (Math.abs(Math.round(x * 7 + z * 13)) % PAINT.length);
    const group = place(x, z, ry, (g) => carLocal(g, p));
    group.userData.noMerge = true;
    group.userData.vehicleAsset = 'sedan';
    group.userData.vehiclePaint = p;
    return group;
  }
  function car(x: number, z: number, ry: number, paint?: number) {
    carBody(x, z, ry, paint);
    collider(`prop-car`, x, z, 4.30, 1.42, 1.76, ry);
  }
  function vanBody(x: number, z: number, ry: number, paint?: number) {
    const p = paint ?? (Math.abs(Math.round(x * 5 + z * 11)) % PAINT.length);
    const group = place(x, z, ry, (g) => vanLocal(g, p));
    group.userData.noMerge = true;
    group.userData.vehicleAsset = 'van';
    group.userData.vehiclePaint = p;
    return group;
  }
  function van(x: number, z: number, ry: number, paint?: number) {
    vanBody(x, z, ry, paint);
    collider(`prop-van`, x, z, 5.20, 2.78, 1.94, ry);
  }
  function bus(x: number, z: number, ry: number) {
    place(x, z, ry, (g) => busLocal(g));
    collider(`prop-bus`, x, z, 11.2, 3.20, 2.55, ry);
  }
  function bike(x: number, z: number, ry: number, tone = 0) {
    place(x, z, ry, (g) => bikeLocal(g, tone));
  }

  /**
   * 登记一个随 `ry` 旋转的碰撞盒。
   *
   * `ox/oz` 是盒子在**道具本地坐标系**里的中心偏移（比如候车亭要登记的只是那块
   * 背板，它在本地 z=-0.68 处）。旋转后的世界中心必须按 `rotation.y` 真算一遍，
   * 不能只转 footprint —— 否则 90° 朝向下背板会跑到侧面去。
   */
  function collider(id: string, x: number, z: number, w: number, h: number, d: number, ry: number, ox = 0, oz = 0, kind: Collider['kind'] = 'wall') {
    const c = Math.cos(ry), s = Math.sin(ry);
    const cx = x + ox * c + oz * s, cz = z - ox * s + oz * c;
    const hx = (Math.abs(c) * w + Math.abs(s) * d) / 2, hz = (Math.abs(s) * w + Math.abs(c) * d) / 2;
    colliders.push({
      source: id, kind,
      min: new THREE.Vector3(cx - hx, 0, cz - hz),
      max: new THREE.Vector3(cx + hx, h, cz + hz),
    });
  }

  function lamp(x: number, z: number, ry: number, height = 5.6) {
    place(x, z, ry, (g) => {
      const pole = new THREE.Mesh(cyl, M.iron);
      pole.position.set(0, height / 2, 0); pole.scale.set(.075, height, .075); pole.castShadow = true; g.add(pole);
      const base = new THREE.Mesh(cyl, M.dark);
      base.position.set(0, .18, 0); base.scale.set(.15, .36, .15); g.add(base);
      const arm = new THREE.Mesh(cyl, M.iron);
      arm.position.set(0, height - .18, .52); arm.scale.set(.055, 1.10, .055); arm.rotation.x = Math.PI / 2; g.add(arm);
      const head = new THREE.Mesh(cube, M.iron);
      head.position.set(0, height - .26, 1.02); head.scale.set(.26, .11, .62); g.add(head);
      const glow = new THREE.Mesh(cube, M.lamp);
      glow.position.set(0, height - .34, 1.02); glow.scale.set(.21, .05, .55); g.add(glow);
    });
  }

  function bin(x: number, z: number) {
    place(x, z, 0, (g) => {
      const b = new THREE.Mesh(cyl, M.iron);
      b.position.set(0, .46, 0); b.scale.set(.30, .92, .30); b.castShadow = true; g.add(b);
      const lid = new THREE.Mesh(cyl, M.dark);
      lid.position.set(0, .95, 0); lid.scale.set(.33, .10, .33); g.add(lid);
      const band = new THREE.Mesh(cyl, M.chrome);
      band.position.set(0, .70, 0); band.scale.set(.315, .05, .315); g.add(band);
    });
    colliders.push({ source: 'prop-bin', kind: 'wall', min: new THREE.Vector3(x - .32, 0, z - .32), max: new THREE.Vector3(x + .32, 1.0, z + .32) });
  }

  function bollard(x: number, z: number) {
    // 细杆：按项目约定不进碰撞表（0.15m 半径的圆盘挡在一根 0.11m 的柱子上，
    // 比直接走过去更烦人）。
    place(x, z, 0, (g) => {
      const p = new THREE.Mesh(cyl, M.iron);
      p.position.set(0, .40, 0); p.scale.set(.055, .80, .055); g.add(p);
      const cap = new THREE.Mesh(sph, M.chrome);
      cap.position.set(0, .82, 0); cap.scale.set(.062, .05, .062); g.add(cap);
      const ring = new THREE.Mesh(cyl, M.white);
      ring.position.set(0, .62, 0); ring.scale.set(.058, .06, .058); g.add(ring);
    });
  }

  function bench(x: number, z: number, ry: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, w: number, h: number, d: number) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(w, h, d); o.castShadow = true; g.add(o);
      };
      for (let n = 0; n < 3; n++) put(M.wood, 0, .44, -.17 + n * .17, 1.72, .05, .15);
      for (let n = 0; n < 3; n++) put(M.wood, 0, .60 + n * .15, .27, 1.72, .13, .05);
      for (const sx of [-.72, .72]) { put(M.iron, sx, .22, -.12, .07, .44, .07); put(M.iron, sx, .22, .24, .07, .44, .07); put(M.iron, sx, .55, .25, .06, .70, .06); }
    });
    // 座面 0.44m —— 低于 STEP_UP(0.45)，collidesAt 会自动当成可迈过的台阶，
    // 登记了也不会变成隐形墙（与公交站台长椅同一条约定）。
    colliders.push({ source: 'prop-bench', kind: 'wall', min: new THREE.Vector3(x - .9, 0, z - .35), max: new THREE.Vector3(x + .9, .90, z + .35) });
  }

  function rack(x: number, z: number, ry: number, slots: number) {
    place(x, z, ry, (g) => {
      const rail = new THREE.Mesh(cyl, M.chrome);
      rail.position.set(0, .34, 0); rail.scale.set(.032, slots * .62, .032); rail.rotation.z = Math.PI / 2; g.add(rail);
      for (let n = 0; n <= slots; n++) {
        const px = -slots * .31 + n * .62;
        const post = new THREE.Mesh(cyl, M.iron);
        post.position.set(px, .17, 0); post.scale.set(.028, .34, .028); g.add(post);
      }
      const back = new THREE.Mesh(cyl, M.chrome);
      back.position.set(0, .62, .34); back.scale.set(.03, slots * .62, .03); back.rotation.z = Math.PI / 2; g.add(back);
    });
  }

  function planter(x: number, z: number, w: number, ry: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = true; g.add(o);
      };
      put(M.kerb, 0, .21, 0, w, .42, .66);
      put(M.stone, 0, .43, 0, w - .10, .06, .56);
      put(M.green, 0, .50, 0, w - .16, .10, .50);
      for (let n = 0; n < Math.max(2, Math.round(w * 2)); n++) {
        const px = -w / 2 + .28 + n * ((w - .56) / Math.max(1, Math.round(w * 2) - 1));
        put(M.green, px, .62, 0, .26, .26, .30);
      }
    });
    colliders.push({ source: 'prop-planter', kind: 'wall', min: new THREE.Vector3(x - w / 2, 0, z - .34), max: new THREE.Vector3(x + w / 2, .50, z + .34) });
  }

  function hydrant(x: number, z: number) {
    place(x, z, 0, (g) => {
      const b = new THREE.Mesh(cyl, M.redLamp);
      b.position.set(0, .32, 0); b.scale.set(.11, .64, .11); b.castShadow = true; g.add(b);
      const cap = new THREE.Mesh(sph, M.redLamp);
      cap.position.set(0, .66, 0); cap.scale.set(.12, .10, .12); g.add(cap);
      for (const sz of [1, -1]) {
        const nub = new THREE.Mesh(cyl, M.redLamp);
        nub.position.set(0, .40, sz * .14); nub.scale.set(.055, .16, .055); nub.rotation.x = Math.PI / 2; g.add(nub);
      }
    });
  }

  function cabinet(x: number, z: number, ry: number, w = 1.05, h = 1.35) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = true; g.add(o);
      };
      put(M.iron, 0, h / 2, 0, w, h, .55);
      put(M.dark, 0, h + .03, 0, w + .08, .07, .63);
      put(M.chrome, 0, h * .62, .29, w * .7, .04, .02);
      put(M.yellow, w * .34, h * .30, .285, .16, .10, .02);
    });
    collider('prop-cabinet', x, z, w + .08, h + .08, .63, ry);
  }

  function shelter(x: number, z: number, ry: number, len: number) {
    const H = 2.55;
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number, shadow = false) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = shadow; g.add(o);
      };
      for (const sx of [-len / 2 + .12, len / 2 - .12]) put(M.iron, sx, H / 2, -.72, .10, H, .10, true);
      put(M.iron, 0, H - .06, 0, len, .10, 1.62, true);             // 顶棚
      put(M.glass, 0, 1.55, -.68, len - .30, 1.85, .05);            // 背板
      put(M.wood, 0, .46, -.36, len - .55, .07, .40);               // 座面
      put(M.iron, 0, .23, -.36, len - .55, .06, .36);
      put(M.lamp, 0, H - .16, -.10, len - .40, .06, .30);           // 顶棚灯带
    });
    // 只登记背板（顶棚在头顶之上，座面 0.46 低于 STEP_UP，两者都不该挡人）。
    collider('prop-shelter', x, z, len - .30, H, .05, ry, 0, -.68);
  }

  function signpost(x: number, z: number, ry: number, h: number, w: number, board: THREE.Material) {
    place(x, z, ry, (g) => {
      const pole = new THREE.Mesh(cyl, M.iron);
      pole.position.set(0, h / 2, 0); pole.scale.set(.055, h, .055); pole.castShadow = true; g.add(pole);
      const face = new THREE.Mesh(cube, board);
      face.position.set(0, h - .42, .05); face.scale.set(w, .62, .07); g.add(face);
      const cap = new THREE.Mesh(cube, M.iron);
      cap.position.set(0, h - .42, .01); cap.scale.set(w + .10, .70, .03); g.add(cap);
    });
  }

  function vending(x: number, z: number, ry: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number, shadow = false) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = shadow; g.add(o);
      };
      put(M.paints[3], 0, 1.0, 0, 1.10, 2.00, .70, true);
      put(M.white, 0, 1.42, .36, .92, 1.05, .04);
      for (let row = 0; row < 3; row++) for (let col = 0; col < 4; col++)
        put(M.paints[(row * 4 + col) % M.paints.length], -.34 + col * .23, 1.10 + row * .30, .385, .17, .24, .02);
      put(M.dark, 0, .74, .365, .92, .22, .03);
      put(M.lamp, 0, 1.94, .37, .96, .16, .03);
    });
    collider('prop-vending', x, z, 1.10, 2.0, .70, ry);
  }

  function rooftop(x: number, z: number, w: number, d: number, topY: number) {
    // 屋面杂物是"俯视时最划算的一笔"：从公寓阳台/俯瞰机位看下去，光秃秃的屋面
    // 才是真正读作"没人住"的东西。水箱 + 外机 + 天线，三件就够。
    const g = new THREE.Group();
    g.position.set(x, topY, z);
    root.add(g);
    const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number) => {
      const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = true; o.receiveShadow = true; g.add(o);
    };
    const tankR = .55;
    const tank = new THREE.Mesh(cyl, M.stone);
    tank.position.set(w * .18, .50 + .42, -d * .16); tank.scale.set(tankR, .84, tankR); tank.castShadow = true; g.add(tank);
    const legs = new THREE.Mesh(cube, M.iron);
    legs.position.set(w * .18, .24, -d * .16); legs.scale.set(tankR * 1.7, .48, tankR * 1.7); g.add(legs);
    put(M.iron, -w * .22, .36, d * .18, .78, .72, .46);           // 空调外机
    put(M.chrome, -w * .22, .74, d * .18, .82, .04, .50);
    put(M.iron, -w * .30, .68, -d * .28, .34, .60, .30);          // 通风帽
    const mast = new THREE.Mesh(cyl, M.iron);
    mast.position.set(w * .34, 1.55, d * .26); mast.scale.set(.035, 3.10, .035); g.add(mast);
    for (let n = 0; n < 3; n++) {
      const bar = new THREE.Mesh(cyl, M.iron);
      bar.position.set(w * .34, 2.10 + n * .34, d * .26);
      bar.scale.set(.016, .62 - n * .12, .016); bar.rotation.z = Math.PI / 2; g.add(bar);
    }
    put(M.tarp, w * .02, .18, -d * .40, .70, .36, .50);           // 遮雨布/杂物
  }

  function banner(x0: number, z0: number, x1: number, z1: number, y: number, count: number) {
    const dx = x1 - x0, dz = z1 - z0;
    const len = Math.hypot(dx, dz);
    const ry = Math.atan2(dz, dx);
    const cx = (x0 + x1) / 2, cz = (z0 + z1) / 2;
    /* 两端各立一根杆：原来整串旗直接悬在 4.4m 空中，两侧**没有任何挂点**，
     * 读数就是"凭空浮着的一面墙"。现在绳系在两根杆顶之间，挂点看得见。
     * 杆按项目惯例**不登记碰撞**（细杆不拦人，见 Method A16b）。 */
    for (const [px, pz] of [[x0, z0], [x1, z1]] as const) {
      const pole = new THREE.Mesh(cyl, M.iron);
      pole.position.set(px, (y + .26) / 2, pz); pole.scale.set(.052, y + .26, .052);
      pole.castShadow = true; root.add(pole);
      const base = new THREE.Mesh(cyl6, M.dark);
      base.position.set(px, .07, pz); base.scale.set(.15, .14, .15); root.add(base);
      const cap = new THREE.Mesh(cyl, M.chrome);
      cap.position.set(px, y + .28, pz); cap.scale.set(.036, .10, .036); root.add(cap);
    }
    const g = new THREE.Group();
    g.position.set(cx, y, cz);
    g.rotation.y = ry;
    root.add(g);
    const rope = new THREE.Mesh(cyl, M.iron);
    rope.position.set(0, 0, 0); rope.scale.set(.022, len, .022); rope.rotation.z = Math.PI / 2; g.add(rope);
    const colors = [M.paints[3], M.paints[4], M.yellow, M.green, M.paints[0]];
    for (let n = 0; n < count; n++) {
      const px = -len / 2 + (n + .5) * (len / count);
      const flag = new THREE.Mesh(cube, colors[n % colors.length]);
      flag.position.set(px, -.44, 0); flag.scale.set(len / count * .68, .78, .02); flag.castShadow = true; g.add(flag);
      const clip = new THREE.Mesh(cube, M.chrome);
      clip.position.set(px - len / count * .30, -.02, 0); clip.scale.set(.03, .09, .03); g.add(clip);
      const clip2 = new THREE.Mesh(cube, M.chrome);
      clip2.position.set(px + len / count * .30, -.02, 0); clip2.scale.set(.03, .09, .03); g.add(clip2);
    }
  }

  function hoarding(x: number, z: number, ry: number, len: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz); o.castShadow = true; g.add(o);
      };
      const H = 2.4;
      put(M.tarp, 0, H / 2, 0, len, H, .09);
      put(M.chrome, 0, H + .06, 0, len, .10, .16);
      for (let n = 0; n <= Math.round(len / 2.4); n++) put(M.iron, -len / 2 + n * 2.4, H / 2, .10, .10, H + .20, .12);
      put(M.yellow, 0, .30, .06, len, .22, .04);
    });
    collider('prop-hoarding', x, z, len, 2.5, .30, ry);
  }

  /* ---------- 第二轮：专门用来填大面积空地的新零件 ----------
   * 第一轮（车/灯/桶/椅）解决的是"沿街没有东西"，但它们**填不掉面积**：
   * 一辆车只占 9 m²，而站东一块空地是 6000 m²。第二轮补的这几件共同点是
   * "一件东西圈住一大块地"——球场靠围网把 30×18 整块地读作有人用，绿篱把
   * 空地切成有界的两块，报刊亭/街钟/游乐设施给场地一个"中心"。
   */

  function kiosk(x: number, z: number, ry: number, board: THREE.Material) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number, shadow = true) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz);
        o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
      };
      const W = 2.40, H = 2.35, D = 1.80;
      put(M.stone, 0, .12, 0, W + .18, .24, D + .18);
      put(M.paints[5], 0, H / 2 + .10, 0, W, H, D);
      put(M.iron, 0, H + .20, 0, W + .72, .20, D + .72);         // 顶盖
      put(M.dark, 0, H + .34, 0, W + .42, .08, D + .42);
      put(M.glass, 0, 1.52, D / 2 + .02, W - .46, 1.30, .05, false);
      put(M.glass, W / 2 + .02, 1.60, -.30, .05, .80, .90, false);
      put(M.wood, 0, .84, D / 2 + .17, W - .30, .09, .34);       // 售货台
      for (let n = 0; n < 3; n++) put(M.wood, -.62 + n * .62, .56, D / 2 + .11, .10, .56, .10);
      put(M.lamp, 0, H + .08, D / 2 + .32, W - .50, .10, .06, false);
      const b = new THREE.Mesh(cube, board);
      b.position.set(0, H - .24, D / 2 + .05); b.scale.set(W - .30, .58, .06); g.add(b);
    });
    collider('prop-kiosk', x, z, 2.40, 2.45, 1.80, ry);
  }

  function hedge(x: number, z: number, ry: number, len: number) {
    place(x, z, ry, (g) => {
      const soil = new THREE.Mesh(cube, M.wood);
      soil.position.set(0, .10, 0); soil.scale.set(len, .20, .80); soil.receiveShadow = true; g.add(soil);
      /* 芯改成**修剪成方的体块**（日本街道的绿篱基本都过剪），不再是一串压扁的球。
       * 留芯是必要的：叶簇是 alphaTest 面片、没有厚度，全靠它的话近距离侧面会看穿。
       * 芯刻意做小一圈（深绿），让外层的叶簇成为主要读数。 */
      const core = new THREE.Mesh(cube, M.green);
      core.position.set(0, .58, 0); core.scale.set(len - .12, .78, .56);
      core.castShadow = true; core.receiveShadow = true; g.add(core);
      const top = new THREE.Mesh(cube, M.green);
      top.position.set(0, .95, 0); top.scale.set(len - .50, .30, .40);
      top.castShadow = true; top.receiveShadow = true; g.add(top);
      /* 叶簇场是**全局**的（一个 InstancedMesh 铺满整片街区），所以这里要把本地
       * 坐标按 `place()` 的朝向折回世界坐标：`rotation.y = ry` ⇒ 本地 (px,·,pz)
       * 落在世界 (x + px·cos + pz·sin, ·, z − px·sin + pz·cos)。 */
      const c = Math.cos(ry), s = Math.sin(ry);
      const n = Math.max(3, Math.round(len / .45));
      for (let i = 0; i < n; i++) {
        const px = -len / 2 + (i + .5) * (len / n);
        for (let k = 0; k < 4; k++) {
          const pz = (hedgeRandom() - .5) * .62;
          const py = .42 + hedgeRandom() * .74;
          hedgeLeaf.spray(x + px * c + pz * s, py, z - px * s + pz * c, .28 + hedgeRandom() * .24);
        }
      }
    });
    collider('prop-hedge', x, z, len, .95, .85, ry);
  }

  function court(x: number, z: number, w: number, d: number, ry: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number, shadow = false) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz);
        o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
      };
      const H = 3.6, hw = w / 2, hd = d / 2;
      put(M.court, 0, .05, 0, w, .10, d);                        // 场地
      const iw = hw - 1.0, idn = hd - 1.0;                       // 界线（内缩 1m）
      put(M.white, 0, .108, idn, iw * 2, .012, .10);
      put(M.white, 0, .108, -idn, iw * 2, .012, .10);
      put(M.white, iw, .108, 0, .10, .012, idn * 2);
      put(M.white, -iw, .108, 0, .10, .012, idn * 2);
      put(M.white, 0, .108, 0, iw * 2, .012, .09);               // 中线
      const nx = Math.max(2, Math.round(w / 3.0)), nz = Math.max(2, Math.round(d / 3.0));
      for (const sx of [-hw, hw]) for (let i = 0; i <= nz; i++) put(M.iron, sx, H / 2, -hd + i * (d / nz), .09, H, .09, true);
      for (const sz of [-hd, hd]) for (let i = 0; i <= nx; i++) put(M.iron, -hw + i * (w / nx), H / 2, sz, .09, H, .09, true);
      for (const y of [1.15, 2.25, 3.35]) {                      // 横杆
        put(M.iron, 0, y, -hd, w, .05, .05); put(M.iron, 0, y, hd, w, .05, .05);
        put(M.iron, -hw, y, 0, .05, .05, d); put(M.iron, hw, y, 0, .05, .05, d);
      }
      for (let i = 1; i < nx; i++) { const px = -hw + i * (w / nx);
        put(M.iron, px, H / 2, -hd, .045, H, .045); put(M.iron, px, H / 2, hd, .045, H, .045); }
      for (let i = 1; i < nz; i++) { const pz = -hd + i * (d / nz);
        put(M.iron, -hw, H / 2, pz, .045, H, .045); put(M.iron, hw, H / 2, pz, .045, H, .045); }
      for (const sx of [-1, 1]) {                                // 篮架
        const px = sx * (hw - 1.1);
        put(M.iron, px, 1.8, 0, .16, 3.6, .16, true);
        put(M.iron, px - sx * .60, 3.42, 0, 1.20, .13, .13);
        put(M.white, px - sx * 1.20, 3.05, 0, .06, 1.05, 1.80);
        const rg = new THREE.Mesh(ring, M.redLamp);
        rg.position.set(px - sx * 1.20, 2.62, 0);
        rg.rotation.x = Math.PI / 2; rg.scale.set(.23, .23, .23);
        rg.castShadow = true; g.add(rg);
      }
    });
    collider('prop-court', x, z, w, .12, d, ry, 0, 0, 'floor');
    collider('prop-court-fence', x, z, w, 3.6, .12, ry, 0, -d / 2);
    collider('prop-court-fence', x, z, w, 3.6, .12, ry, 0, d / 2);
    collider('prop-court-fence', x, z, .12, 3.6, d, ry, -w / 2, 0);
    collider('prop-court-fence', x, z, .12, 3.6, d, ry, w / 2, 0);
  }

  function mailbox(x: number, z: number, ry: number) {
    place(x, z, ry, (g) => {
      const body = new THREE.Mesh(cyl, M.paints[3]);
      body.position.set(0, .62, 0); body.scale.set(.28, 1.10, .28); body.castShadow = true; g.add(body);
      const cap = new THREE.Mesh(sph, M.paints[3]);
      cap.position.set(0, 1.18, 0); cap.scale.set(.30, .24, .30); cap.castShadow = true; g.add(cap);
      const base = new THREE.Mesh(cyl, M.dark);
      base.position.set(0, .07, 0); base.scale.set(.32, .14, .32); g.add(base);
      const band = new THREE.Mesh(cyl, M.white);
      band.position.set(0, .48, 0); band.scale.set(.29, .10, .29); g.add(band);
      const slot = new THREE.Mesh(cube, M.dark);
      slot.position.set(0, .92, .27); slot.scale.set(.34, .09, .07); g.add(slot);
    });
    collider('prop-mailbox', x, z, .62, 1.30, .62, ry);
  }

  function clock(x: number, z: number, h: number, ry: number) {
    place(x, z, ry, (g) => {
      const base = new THREE.Mesh(cyl, M.stone);
      base.position.set(0, .22, 0); base.scale.set(.30, .44, .30); base.castShadow = true; g.add(base);
      const pole = new THREE.Mesh(cyl, M.iron);
      pole.position.set(0, h / 2, 0); pole.scale.set(.10, h, .10); pole.castShadow = true; g.add(pole);
      for (const sz of [1, -1]) {                                // 双面钟
        const face = new THREE.Mesh(cyl, M.white);
        face.position.set(0, h + .30, sz * .06); face.scale.set(.72, .16, .72);
        face.rotation.x = Math.PI / 2; face.castShadow = true; g.add(face);
        const hh = new THREE.Mesh(cube, M.dark);
        hh.position.set(0, h + .36, sz * .15); hh.scale.set(.055, .40, .02); hh.rotation.z = -.55; g.add(hh);
        const mh = new THREE.Mesh(cube, M.dark);
        mh.position.set(0, h + .40, sz * .15); mh.scale.set(.045, .56, .02); mh.rotation.z = .95; g.add(mh);
      }
      const rim = new THREE.Mesh(cyl, M.iron);
      rim.position.set(0, h + .30, 0); rim.scale.set(.80, .10, .80); rim.rotation.x = Math.PI / 2; g.add(rim);
    });
  }

  function trafficLight(x: number, z: number, ry: number, h: number) {
    place(x, z, ry, (g) => {
      const pole = new THREE.Mesh(cyl, M.iron);
      pole.position.set(0, h / 2, 0); pole.scale.set(.075, h, .075); pole.castShadow = true; g.add(pole);
      const arm = new THREE.Mesh(cyl, M.iron);
      arm.position.set(h * .28, h - .20, 0); arm.scale.set(.055, h * .56, .055);
      arm.rotation.z = Math.PI / 2; arm.castShadow = true; g.add(arm);
      const head = new THREE.Mesh(cube, M.dark);
      head.position.set(h * .54, h - .46, 0); head.scale.set(.28, .84, .30); head.castShadow = true; g.add(head);
      const lens = [M.redLamp, M.yellow, M.green];
      for (let n = 0; n < 3; n++) {
        const o = new THREE.Mesh(cyl, lens[n]);
        o.position.set(h * .54, h - .74 + n * .28, .16); o.scale.set(.085, .05, .085);
        o.rotation.x = Math.PI / 2; g.add(o);
      }
      const ped = new THREE.Mesh(cube, M.dark);
      ped.position.set(.10, h * .42, 0); ped.scale.set(.22, .60, .26); g.add(ped);
      const pl = new THREE.Mesh(cyl, M.green);
      pl.position.set(.10, h * .42, .15); pl.scale.set(.075, .05, .075); pl.rotation.x = Math.PI / 2; g.add(pl);
    });
  }

  function playUnit(x: number, z: number, ry: number) {
    place(x, z, ry, (g) => {
      const put = (m: THREE.Material, px: number, py: number, pz: number, sx: number, sy: number, sz: number, shadow = false) => {
        const o = new THREE.Mesh(cube, m); o.position.set(px, py, pz); o.scale.set(sx, sy, sz);
        o.castShadow = shadow; o.receiveShadow = true; g.add(o); return o;
      };
      const DECK = 1.20, W = 2.40, D = 2.00;
      for (const sx of [-1, 1]) for (const sz of [-1, 1])
        put(M.iron, sx * (W / 2 - .12), DECK / 2, sz * (D / 2 - .12), .12, DECK, .12, true);
      put(M.wood, 0, DECK + .06, 0, W, .12, D, true);            // 平台
      put(M.yellow, 0, DECK + 1.42, -D / 2 + .08, W, .12, .12);  // 顶横杆
      for (const sx of [-1, 1]) put(M.yellow, sx * (W / 2 - .12), DECK + .72, -D / 2 + .08, .10, 1.40, .10);
      for (const sz of [-1, 1]) put(M.yellow, 0, DECK + 1.42, sz * (D / 2 - .08), W, .10, .10);
      for (let i = 0; i < 5; i++) put(M.wood, -W / 2 - .50 - i * .30, .24 + i * .25, D / 2 - .34, .32, .08, .90);  // 斜梯
      const slide = put(M.paints[0], W / 2 + .88, .68, 0, 2.24, .08, .95);
      slide.rotation.z = -.44; slide.castShadow = true;
      for (const sz of [-1, 1]) { const r = put(M.chrome, W / 2 + .88, .84, sz * .50, 2.24, .22, .06); r.rotation.z = -.44; }
      for (const sx of [-1, 1]) {                                // 弹簧摇马
        const seat = put(M.paints[sx > 0 ? 4 : 6], sx * 2.9, .58, D / 2 + .30, .70, .22, .34, true);
        seat.rotation.x = .1;
        put(M.iron, sx * 2.9, .34, D / 2 + .30, .10, .58, .10);
        const sp = new THREE.Mesh(cyl, M.iron);
        sp.position.set(sx * 2.9, .12, D / 2 + .30); sp.scale.set(.10, .34, .10); sp.rotation.x = Math.PI / 2; g.add(sp);
      }
    });
    collider('prop-play', x, z, 3.0, 1.40, 2.20, ry);
  }

  return { root, materials, geometries, hedgeLeaf, car, carBody, van, vanBody, bus, bike, lamp, bin, bollard, bench, rack, planter, hydrant, cabinet, shelter, signpost, vending, rooftop, banner, hoarding, kiosk, hedge, court, mailbox, clock, trafficLight, playUnit };
}

/** 给站牌/路名牌造一块画布材质（文字牌面）。 */
export function propBoard(
  lines: string[], opts: { w?: number; h?: number; bg?: string; fg?: string; accent?: string } = {},
): THREE.MeshStandardMaterial {
  const W = opts.w ?? 512, H = opts.h ?? 256;
  const cv = document.createElement('canvas');
  cv.width = W; cv.height = H;
  const c = cv.getContext('2d')!;
  c.fillStyle = opts.bg ?? '#2f4a63'; c.fillRect(0, 0, W, H);
  c.strokeStyle = opts.accent ?? '#e8d9b4'; c.lineWidth = 6; c.strokeRect(10, 10, W - 20, H - 20);
  c.fillStyle = opts.fg ?? '#f4ecd8'; c.textAlign = 'center';
  const step = H / (lines.length + .6);
  lines.forEach((line, i) => {
    c.font = `${i === 0 ? '700 ' : ''}${Math.round(step * (i === 0 ? .62 : .42))}px "Microsoft YaHei",Georgia,sans-serif`;
    c.fillText(line, W / 2, step * (i + .82), W - 60);
  });
  const tex = new THREE.CanvasTexture(cv);
  tex.colorSpace = THREE.SRGBColorSpace;
  const m = new THREE.MeshStandardMaterial({ map: tex, roughness: .62, emissive: '#ffffff', emissiveMap: tex, emissiveIntensity: .35 });
  m.userData.outlineWeight = 0;
  return m;
}
