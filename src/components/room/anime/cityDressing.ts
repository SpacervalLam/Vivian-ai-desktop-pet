import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';
import { createPropKit, propBoard } from './streetProps';
import { groundTexture } from './groundTextures';
import { buildCityRoutes, CITY_ROUTES, SOUTH_ROUTES, type CityRoute } from './cityRoutes';
import { createBlossomField, createLeafField, createTrunkField, plantTree, type TreeKit } from './foliage';
/* 公寓占位常量。只为了派生 `PROTECTED` 里那两条保护区 —— 公寓的碰撞盒不在
 * `districtArt.ts` 的碰撞盒表里（见 PROTECTED 的注释），只能这样把它圈出来。 */
import { APT_X0, APT_X1, APT_ZN, APT_ZF, APT_TWIN_DZ } from './exterior';

/**
 * 街区陈设：把"空地太多、看起来很空旷"这件事按**区域**补上。
 *
 * 先量再补。用 `tmp/_probe-void.mjs`（4m 网格，判据是"离最近实体 ≥12m"）扫了
 * 一遍街区，改前合计 **16672 m²** 的地面离任何东西都超过 12m，前两名：
 *
 *   ① 9136 m²  x[100,168] z[-82,118]  最大净距 64m —— 站东
 *   ② 4512 m²  x[-100,-68] z[-78,118] 最大净距 44m —— 街区西缘到远景墙之间那条空带
 *
 * **改图前先读占位图。** 第一版把 ① 当成"站体东侧那片大草坪"来设计站前广场，
 * 结果 `_probe-dressing.mjs` 的东区占位图直接打脸：`sakuraStation` 早在
 * **x 104~150 / z -17..62** 就建了整片住宅区 —— 地面是 `east-ground` 那块
 * 70×140 的草地（x 80..150 / z -45..95），`east-lane` 在 x=132，`house()` 落在
 * [108,-7]/[120,-7]/[140,10]/[116,40]/[116,57]/[140,53]（每栋带 `garden-wall`
 * 与 `garden-side` 院墙），`shop()` 在 x108/119 z=21，`utility-pole` 在 x126.5/137.5。
 * 所以 ① 里**真正成片的空只有四块**：
 *
 *   北段 z -78..-46（44m 宽整条空）／中带 z 0..14／东段 x 128..148／南带 z 92..118
 *
 * 站前广场只能**填在房子之间**——这就是 `pave()` 要分块铺的由来（整块 36×60 的
 * 铺装会从人家院子里穿过去，第一版就是这么报了 10 处重叠）。
 *
 * 这两块是"俯视一眼就看见"的洞。除此之外还有一类更伤人的空：**沿街没有东西**。
 * 东西向街道（z = -69/-44/-19/10.1/30/61）此前一根路灯都没有（路灯只立在南北向
 * 林荫道上），站在街心看过去，路面、人行道、天际线之间什么都没有 —— 那才是
 * "空旷"这个词真正的来源。第一版靠"沿街排满停放的车"来治，**治错了**（见下）。
 *
 * 分区（各自独立可核账）：
 *
 *   A.  **站前东区**（x 98..150 / z -44..94）—— 商务楼 + 内部车道 + 公交站场 +
 *       出租车乘车点 + 自行车棚 + 露天停车场 + 口袋绿地。
 *   A5. **站东北段**（x 100..148 / z -78..-44）—— 第二轮，填重扫后的 TOP1。
 *   A6. **站东中带**（x 100..132 / z 0..14）—— 报刊亭 + 绿篱 + 行道树。
 *   A7. **站东段**（x 124..168 / z 34..118）—— 街头球场 + 儿童游乐场 + 绿篱 +
 *       边界墙外的东缘服务带。
 *   A8. **站东南带**（z 92..118）—— 人行道 + 树阵 + 绿篱。
 *   B.  **沿街陈设** —— 沿 CITY_ROUTES / SOUTH_ROUTES 两侧成对铺路灯、行道树与
 *       零散家具（垃圾桶 / 长椅 / 花箱 / 配电箱 / 消火栓 / 自行车架 / 拉旗 /
 *       信号灯）。**不放停放车辆。**
 *   C.  **西缘服务带**（x -102..-86）—— 低层仓储/工坊 + 配送货车 + 路灯。
 *
 * **第一轮的教训：车和灯填不掉面积。** 一辆车 9 m²，而站东一块空是 6000 m²。
 * 第二轮补的零件（`streetProps.ts` 的 `court / hedge / kiosk / playUnit / clock /
 * mailbox / trafficLight`）共同点是"**一件东西圈住一大块地**"：球场靠围网把
 * 20×14 整块读作有人用，绿篱把空地切成有界两块，报刊亭/街钟给场地一个中心。
 *
 * **第三轮的教训（本轮返工）：车填不掉"在用"，只能填掉"面积"。** 第一版在 B 段
 * 沿 18 条街两侧每 12.4m 停一辆车，合计 122 辆静止汽车；用户直接否掉了 ——
 * 「不要再道路上放一堆密集静止的汽车，你可以做少量动态的汽车」。判断是对的：
 * 一排等距的静止车把空地表按下去了，却把街道做成了停车场，而且越整齐越像库存。
 * 现在的分工是：**本模块负责"街上有东西"（灯/树/家具/店面），
 * `streetTraffic.ts` 负责"街上有车在开"（2 圈环路、6 辆）。**
 * 静止车辆只剩三类有语义的：出租车乘车点 3 辆、露天停车场 6 辆、公交站场 3 辆、
 * 西缘配送货车 6 辆 —— 没有一类是"沿街排成一列"。
 *
 * 放置纪律：
 *
 *  - **不碰既有东西**：所有落位先过 `free()`（对既非 floor 也非 ramp 的碰撞盒做
 *    footprint 相交测试，外扩 0.45m）。重复陈设、插进楼里、压到樱花树上都不可能
 *    发生 —— 判据是几何，不是我的记忆。
 *  - **放不下就挪，不缩**：`tryAt()` 先试理想位，不行按 4m 一圈圈往外挪。大件
 *    一律走它 —— 站东的房子散在住宅区的缝里，按图纸挑的坐标十有八九会被顶掉。
 *  - **不碰保护区**：`PROTECTED` 里两条是预览页的自检走线（**验收契约**，扫到
 *    0.46~1.72m 高度有东西就报错），第三条是玩家边界墙（见下方注释）。
 */

/** `districtArt.ts` 的 `WALK_BOUNDS` 手工同步副本（那边没导出，导出会成环）。 */
const WALK = { x0: -80, x1: 150, z0: -78, z1: 165 } as const;

/** 不许落位的矩形（x0,x1,z0,z1）。 */
const PROTECTED: readonly [number, number, number, number][] = [
  // 预览页自检走线。`tmp/sakura-street-preview.tsx` 沿这两条扫 0.46~1.72m 高度，
  // 扫到东西就报错 —— 它是**验收契约**，不是可选检查。
  [-17.2, 34.6, 14.5, 21.0],
  [-9.9, -8.3, 14.6, 22.0],
  /* 玩家公寓。**这是最贵的一条教训。**
   *
   * `free()` 的 `keepOut` 是从 `districtArt.ts:69` 那个数组算出来的，它只含
   * streets / sakuraTown / sakuraStation / civicQuarter / worldStreetLife。
   * 玩家公寓（`buildApartmentShell` / `buildApartmentTwin` / `apartmentPodium`）
   * 的碰撞盒**根本不在这份表里** —— 而且它在 `RoomScene.tsx` 里比
   * `createDistrictArt` 还晚建（1439 行 vs 1513 行），想传进来也传不了。
   *
   * 前几轮没事，是因为手写坐标都在"沿街"和"站东"，从来没往楼里插。这一轮加了
   * 全域扫描的**扫街绿化**，它一视同仁地扫到了公寓头上：13 处冲突里 9 处是
   * `apt-twin-mass`、1 处 `apt-twin-lift-mass`、1 处 `podium-bookcase`。
   *
   * 修法：从 `exterior.ts` 导出的常量**派生**，不写死数字 —— 公寓尺寸一改，
   * 这里自己跟着走。主楼与孪生楼各一条（孪生楼整体平移 `APT_TWIN_DZ`）；
   * 电梯塔楼贴在西山墙外侧（x 到 `APT_X0-5.87`），所以西界多留 6.5m；
   * 北界留 1.6m 是给裙房雨棚（z=5.55）与入口花池的。
   */
  [APT_X0 - 6.5, APT_X1 + 1.5, APT_ZN - 1.6, APT_ZF + 2.0],
  [APT_X0 - 6.5, APT_X1 + 1.5, APT_ZN - 1.6 + APT_TWIN_DZ, APT_ZF + 2.0 + APT_TWIN_DZ],

  /* 玩家边界墙（不可见，1m 厚，4m 高，环本身没有几何）。**这条也是被真实冲突
   * 逼出来的**：边界墙在 `districtArt.ts:553` 创建，晚于本模块的
   * `createCityDressing`（305 行），所以它进不了 `keepOut`。上一版 `tryAt` 就把
   * 一块球场挪到了 x 136..156 —— 正好骑在 x=150 那道墙上。
   * 与其把本模块挪到边界墙之后（依赖调用顺序，下次有人插一行就又坏了），
   * 不如把这条判据写死在这里。
   *
   * 四个数是 `districtArt.ts` 里 `WALK_BOUNDS` 的手工同步副本（那边没导出，
   * 导出会成环）。**改 WALK_BOUNDS 时这里要一起改。**
   */
  [WALK.x0 - 2, WALK.x0 + 1, WALK.z0 - 2, WALK.z1 + 2],
  [WALK.x1 - 1, WALK.x1 + 2, WALK.z0 - 2, WALK.z1 + 2],
  [WALK.x0 - 2, WALK.x1 + 2, WALK.z0 - 2, WALK.z0 + 1],
  [WALK.x0 - 2, WALK.x1 + 2, WALK.z1 - 1, WALK.z1 + 2],
];

/** 铺装/落位的可用范围。z0 取到 -94：北侧边界墙外还有一条 16m 空带
 *  （`_probe-dressing.mjs` 的空地表里 z=-98 那几块就是它），要从街区里往外看
 *  接得上，得允许在北墙外种树。 */
const BOUNDS = { x0: -106, x1: 168, z0: -94, z1: 164 } as const;

export interface CityDressingPlan {
  stationBuildings: number; stationCars: number; lamps: number;
  trees: number; furniture: number; westBlocks: number;
  /** 西缘服务带的配送货车（静止）。**上一版的 `cars` 已删除** —— 沿街那一百多辆
   *  路缘停车整段拆掉了，现在**本模块一辆"沿街停的车"都不放**，街上的车全部来自
   *  `streetTraffic.ts`（动态）。计数留着 `stationCars`（出租车/公交/停车场）
   *  与这里的 `westVans`，是为了核账时能一眼看出静止车辆到底还剩多少。 */
  westVans: number;
  /** 第二轮：站东北段低层街区。 */
  northBlocks: number;
  /** 第二轮：边界墙外东缘那一排低层体量。 */
  eastBlocks: number;
  /** 第二轮：街头球场 / 报刊亭 / 绿篱段 / 儿童游乐。 */
  courts: number; kiosks: number; hedges: number; plays: number;
  /** 分块铺装实际铺下去的板块数（跳过的是压在既有实体上的）。 */
  pavingTiles: number;
  /** 收尾"扫街绿化"实际种下的点数（树 + 绿篱）。 */
  sweep: number;
}

export interface CityDressingHandle {
  colliders: Collider[];
  /** 落位统计，给核账脚本读。 */
  plan: CityDressingPlan;
  dispose(): void;
}

export function createCityDressing(scene: THREE.Scene, existing: Collider[]): CityDressingHandle {
  const root = new THREE.Group();
  root.name = 'city-dressing';
  root.userData.sceneCollideSkip = true;
  scene.add(root);

  const colliders: Collider[] = [];
  const kit = createPropKit(root, colliders);
  const materials: THREE.Material[] = [...kit.materials];
  const textures: THREE.Texture[] = [];
  const geometries = new Set<THREE.BufferGeometry>(kit.geometries);

  const mat = (color: string, rough = .88, extra: Partial<THREE.MeshStandardMaterialParameters> = {}) => {
    const m = new THREE.MeshStandardMaterial({ color, roughness: rough, ...extra });
    m.userData.outlineWeight = 0;
    materials.push(m);
    return m;
  };
  /** 地面贴图登记进 `textures`，模块 dispose 时才释放得掉。 */
  const tex = (t: THREE.Texture) => { textures.push(t); return t; };
  const M = {
    /* 铺装分块铺（`pave()` 的 tile = 4m），所以 repeat = 1 正好让一个 tile 落在
     * 一块 4×4 的板上 ⇒ 单砖 0.5m，是人行道砖的真实尺度。
     * 原来这两色都是纯色：站东那片铺装一张就几十米见方，俯视是一块灰板。 */
    paving: mat('#b2b2a6', .93, { map: tex(groundTexture('paving', 1)) }),
    road: mat('#586675', .95, { map: tex(groundTexture('asphalt', 3)) }),
    paint: mat('#e6e2d5', .8),
    facade: ['#cfc6b4', '#b9a894', '#a9b3ae', '#c8bda9', '#9fa9b2', '#bfae9e'].map((c) => mat(c)),
    shop: mat('#3d5158', .6),
    glass: mat('#3a5a68', .25, { metalness: .25 }),
    roof: mat('#6d7b7c', .95),
    steel: mat('#7d8b93', .6, { metalness: .3 }),
    green: mat('#5f7a53', .95),
    kerb: mat('#a4a69e', .9),
  };

  const cube = new THREE.BoxGeometry(1, 1, 1);
  geometries.add(cube);
  const box = (parent: THREE.Object3D, m: THREE.Material, x: number, y: number, z: number, w: number, h: number, d: number, shadow = true) => {
    const o = new THREE.Mesh(cube, m);
    o.position.set(x, y, z); o.scale.set(w, h, d);
    o.castShadow = shadow; o.receiveShadow = true;
    parent.add(o);
    return o;
  };
  const solid = (id: string, parent: THREE.Object3D, m: THREE.Material, x: number, y: number, z: number, w: number, h: number, d: number, kind: Collider['kind'] = 'wall') => {
    box(parent, m, x, y, z, w, h, d);
    colliders.push({ source: id, kind, min: new THREE.Vector3(x - w / 2, y - h / 2, z - d / 2), max: new THREE.Vector3(x + w / 2, y + h / 2, z + d / 2) });
  };

  /* 铺装。**必须分块铺**：站东 x 108~145 是 sakuraStation 的既有住宅区，
   * 一整块 36×60 的广场会从人家院子、花坛、院墙里穿过去（上一轮就这么报了
   * 10 处重叠）。这里按 4m 见方的板块铺，板块与既有实体（非 floor/ramp）
   * 相交就跳过 —— 于是铺装自然长成"填在房子之间"的样子，这才是站前广场
   * 该有的形态。板块内收 0.10m 再判交，让铺装能贴到墙根而不触发判交。 */
  const floorFree = (x0: number, x1: number, z0: number, z1: number) => {
    for (const k of keepOut) if (x0 < k.x1 && x1 > k.x0 && z0 < k.z1 && z1 > k.z0) return false;
    return true;
  };
  const pave = (id: string, x: number, z: number, w: number, d: number, y: number, tile = 4, m: THREE.Material = M.paving) => {
    const nx = Math.max(1, Math.round(w / tile)), nz = Math.max(1, Math.round(d / tile));
    const tw = w / nx, td = d / nz;
    let laid = 0;
    for (let i = 0; i < nx; i++) for (let j = 0; j < nz; j++) {
      const cx = x - w / 2 + tw * (i + .5), cz = z - d / 2 + td * (j + .5);
      if (!floorFree(cx - tw / 2 + .10, cx + tw / 2 - .10, cz - td / 2 + .10, cz + td / 2 - .10)) continue;
      box(root, m, cx, y, cz, tw, .08, td, false);
      // 地面碰撞盒跟着板块走：既有铺装是 floor，不挡人，只负责"脚下有多高"
      colliders.push({ source: id, kind: 'floor', min: new THREE.Vector3(cx - tw / 2, y - .04, cz - td / 2), max: new THREE.Vector3(cx + tw / 2, y + .04, cz + td / 2) });
      laid++;
    }
    return laid;
  };

  /* ---------------- 落位过滤 ---------------- */

  const keepOut = existing
    .filter((c) => c.kind !== 'floor' && c.kind !== 'ramp')
    .map((c) => ({ x0: c.min.x - .45, x1: c.max.x + .45, z0: c.min.z - .45, z1: c.max.z + .45 }));
  // 自己刚放下的东西也要挡后面的：车与车不能叠在一起。
  const own: { x0: number; x1: number; z0: number; z1: number }[] = [];
  const free = (x: number, z: number, hw: number, hd: number) => {
    const x0 = x - hw, x1 = x + hw, z0 = z - hd, z1 = z + hd;
    // 西界 -106：远景墙内圈在 x=-114，留 8m 净距；东界 168：站东区之外还没做。
    if (x0 < BOUNDS.x0 || x1 > BOUNDS.x1 || z0 < BOUNDS.z0 || z1 > BOUNDS.z1) return false;
    for (const p of PROTECTED) if (x0 < p[1] && x1 > p[0] && z0 < p[3] && z1 > p[2]) return false;
    for (const k of keepOut) if (x0 < k.x1 && x1 > k.x0 && z0 < k.z1 && z1 > k.z0) return false;
    for (const k of own) if (x0 < k.x1 && x1 > k.x0 && z0 < k.z1 && z1 > k.z0) return false;
    return true;
  };
  const claim = (x: number, z: number, hw: number, hd: number) => own.push({ x0: x - hw, x1: x + hw, z0: z - hd, z1: z + hd });

  /**
   * "把这件东西放在这附近"。`free()` 只回答能不能放，回答不了"那放哪"——
   * 站东的房子是散在住宅区缝里的，我按图纸挑的坐标十有八九被既有实体顶掉。
   * 所以大件（楼、球场、游乐场、报刊亭）一律走这里：先试理想位，不行就按
   * 4m 一圈一圈往外挪，最多挪 4 圈（16m）。挪不动才放弃。
   *
   * 注意它**只挪位置，不缩体量**。缩体量会把"一栋 14×12 的楼"变成一堆碎块，
   * 从街面看反而更空。
   */
  const tryAt = (x: number, z: number, hw: number, hd: number, fn: (px: number, pz: number) => void, step = 4, rings = 4) => {
    if (free(x, z, hw, hd)) { fn(x, z); return true; }
    for (let r = step; r <= step * rings; r += step) {
      for (const [dx, dz] of [[r, 0], [-r, 0], [0, r], [0, -r], [r, r], [-r, r], [r, -r], [-r, -r]]) {
        if (free(x + dx, z + dz, hw, hd)) { fn(x + dx, z + dz); return true; }
      }
    }
    return false;
  };

  /* ---------------- 树木（与全场景共用一套冠簇） ---------------- */

  const seedState = { s: 9173 };
  const random = () => { seedState.s = (Math.imul(seedState.s, 1664525) + 1013904223) >>> 0; return seedState.s / 4294967296; };
  /* 容量是按"约 70 棵 × 密度 0.4"估的：plantTree 每棵消耗 7×(112+34)+5×(130+55) 次
   * 抽样，乘密度后 ≈ 780。给到 65000 是留一倍余量，又不会让 InstancedMesh 的
   * instanceMatrix 变成几十 MB —— 每个实例 16 个 float，65000 个就是 4.2MB。 */
  const blossom = createBlossomField(random, 65000);
  const leaf = createLeafField(random, 65000);
  const trunkMat = mat('#7a5c45', .9);
  const pitMat = mat('#8d9483', .95);
  const kitTree: TreeKit = {
    blossom, leaf, random,
    // 树干走 Blender 资产（foliage 的干场）。**干不再走 `rod`** —— 原来那根是
    // 缩放的立方体，正是"过于简陋"的那一类。合批之后再挂，见下面 mergeByMaterial。
    trunk: createTrunkField(),
    rod(a, b, r) {
      const v = new THREE.Vector3(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
      const o = new THREE.Mesh(cube, trunkMat);
      o.position.set(a[0] + v.x / 2, a[1] + v.y / 2, a[2] + v.z / 2);
      o.scale.set(r * 1.6, v.length(), r * 1.6);
      o.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), v.normalize());
      o.castShadow = true;
      root.add(o);
    },
    pit(x, z) {
      box(root, pitMat, x, .035, z, 1.5, .07, 1.5, false);
      box(root, M.kerb, x, .06, z, 1.7, .05, 1.7, false);
    },
  };
  /* ---------------- 屋顶绿化 / 立面爬藤 ----------------
   *
   * 低层建筑的屋顶是这个场景里**唯一还平着的面**：从俯视机位（预览页的
   * 「城市布局全景」）看过去，十几块深灰楼板铺成一片。`kit.rooftop` 补的水箱、
   * 空调外机、天线解决的是"有人在用"，解决不了"平"。这里补的是绿化：
   * 女儿墙内侧一圈种植槽 + 叶簇；再给实墙补爬藤 —— 街景里另一块"什么都没有"
   * 的地方就是那几面 7~13m 高的实墙。
   *
   * 用**独立的**叶簇场和 PRNG。共享 `leaf` 场看着省事，但 `spray()` 内部要用
   * 注入的 `random()` 摇朝向 —— 那会把主序列推进，后面所有行道树、家具的落位
   * 跟着整体洗一遍（这个模块的随机流是被 `free()/claim()` 序列依赖的）。 */
  let roofSeed = 60611;
  const roofRandom = () => { roofSeed = (Math.imul(roofSeed, 1664525) + 1013904223) >>> 0; return roofSeed / 4294967296; };
  const roofLeaf = createLeafField(roofRandom, 12000);
  /** 屋顶绿化：沿女儿墙内侧铺一圈种植槽 + 叶簇。`topY` 是屋面标高。 */
  function roofGarden(px: number, pz: number, w: number, d: number, topY: number) {
    const alongX = w >= d;
    const len = (alongX ? w : d) - 2.2;
    const n = Math.max(2, Math.round(len / 2.5));
    for (let i = 0; i < n; i++) {
      const t = -len / 2 + (i + .5) * (len / n);
      const gx = alongX ? px + t : px, gz = alongX ? pz : pz + t;
      const gw = alongX ? 2.1 : 1.05, gd = alongX ? 1.05 : 2.1;
      box(root, M.kerb, gx, topY + .13, gz, gw, .26, gd, false);
      box(root, M.green, gx, topY + .285, gz, gw - .20, .09, gd - .20, false);
      for (let k = 0; k < 8; k++) {
        roofLeaf.spray(
          gx + (roofRandom() - .5) * (gw - .34),
          topY + .34 + roofRandom() * .40,
          gz + (roofRandom() - .5) * (gd - .34),
          .16 + roofRandom() * .13,
        );
      }
    }
  }
  /**
   * 立面爬藤：沿墙撒一串叶簇，自墙脚爬起、高度随株变化（不是一条齐平的绿带）。
   * `dx/dz` 是**沿墙方向**的单位向量，用向量而不是角度：调用点写 `(1,0)`（沿 x）
   * 或 `(0,1)`（沿 z），比推一遍 `cos/sin` 不容易错。
   */
  function wallVine(x: number, z: number, dx: number, dz: number, w: number, topY: number) {
    const n = Math.max(3, Math.round(w / .34));
    for (let i = 0; i < n; i++) {
      const t = -w / 2 + (i + .5) * (w / n);
      const px = x + dx * t, pz = z + dz * t;
      const h = topY * (.45 + roofRandom() * .55);
      for (let k = 0; k < 4; k++) {
        roofLeaf.spray(px + (roofRandom() - .5) * .42, .55 + (k + roofRandom() * .8) / 4 * h,
          pz + (roofRandom() - .5) * .42, .14 + roofRandom() * .12);
      }
    }
  }

  let treeIndex = 0;
  const plan: CityDressingPlan = {
    stationBuildings: 0, stationCars: 0, lamps: 0, trees: 0, furniture: 0, westBlocks: 0, westVans: 0,
    northBlocks: 0, eastBlocks: 0, courts: 0, kiosks: 0, hedges: 0, plays: 0, pavingTiles: 0, sweep: 0,
  };
  /** 本轮铺过的所有路线。收尾的"扫街绿化"要用它当路网掩膜，免得把树种到车道中间。 */
  const allRoutes: CityRoute[] = [];
  function tree(x: number, z: number, bloom: boolean, density = .40) {
    if (!free(x, z, 1.1, 1.1)) return false;
    plantTree(kitTree, x, z, bloom, treeIndex++, density);
    claim(x, z, 1.0, 1.0);
    colliders.push({ source: 'dress-tree', kind: 'wall', min: new THREE.Vector3(x - .3, 0, z - .3), max: new THREE.Vector3(x + .3, 2.4, z + .3) });
    plan.trees++;
    return true;
  }

  /* ================================================================
   * A. 站前东区：填掉 ① 里"房子之间"的缝
   * ================================================================
   * 站体与站台在 x 82..95。站东地面是 `sakuraStation` 的 `east-ground`
   * （x 80..150 / z -45..95 的草地），但**它并不空**：x 104~150 / z -17..62
   * 已经是一整片住宅区（见文件头注释）。所以这一段不是"铺满一块草坪"，而是
   * **绕开住宅区、填进缝里**：
   *
   *   z -44..8   商务段：两排临街楼（x=108 / x=140）+ 东西向内部车道
   *   z 12..72   站前广场：公交站场、出租车排队区、自行车棚、行道树 ——
   *              铺装分块，只铺在既有院落与花坛之外
   *   z 76..92   停车与绿地：露天停车场 + 口袋绿地
   */

  /* ---- A1 内部路网：先铺路，楼再骑在路的两侧 ---- */
  const innerRoutes: CityRoute[] = [
    { name: 'dress-station-east-lane', points: [[100, 6], [116, 6], [132, 6], [146, 6], [152, 6]], width: 4.6 },
    { name: 'dress-station-north-cross', points: [[124, -42], [124, -26], [124, -10], [124, 4]], width: 5.2 },
    { name: 'dress-station-forecourt-walk', points: [[100, 14], [100, 34], [100, 54], [100, 72]], width: 3.4, walk: true },
    { name: 'dress-station-forecourt-east', points: [[134, 16], [134, 36], [134, 56], [134, 74]], width: 4.2 },
    { name: 'dress-station-south-drive', points: [[100, 74], [120, 76], [140, 78], [150, 80]], width: 4.4 },
    { name: 'dress-station-west-link', points: [[98, 40], [98, 60], [98, 74]], width: 4.2 },
  ];
  allRoutes.push(...innerRoutes);
  for (const g of buildCityRoutes(root, colliders, innerRoutes, { road: M.road, paving: M.paving, paint: M.paint }, .07)) geometries.add(g);

  /* ---- A2 商务段：两排临街楼，屋面带杂物 ---- */
  const stationLots: [number, number, number, number, number][] = [
    // x, z, w, d, 层数
    [108, -36, 14, 12, 4], [108, -20, 14, 12, 5],
    [140, -36, 14, 12, 5], [140, -20, 14, 12, 4],
    [108, -2, 14, 12, 4], [140, -2, 14, 12, 5],
  ];
  stationLots.forEach(([x, z, w, d, floors], i) => {
    if (!free(x, z, w / 2, d / 2)) return;
    const h = 3.4 + (floors - 1) * 2.8;
    const face = M.facade[i % M.facade.length];
    solid(`dress-station-block-${i}`, root, face, x, h / 2, z, w, h, d);
    box(root, M.roof, x, h + .16, z, w + .34, .32, d + .34);
    // 首层店面：朝 +z 的一面开一列橱窗与雨棚
    for (let n = 0; n < 3; n++) {
      const px = x - w / 2 + 2.4 + n * ((w - 4.8) / 2);
      box(root, M.glass, px, 1.75, z + d / 2 + .04, w / 4.4, 2.4, .06, false);
      box(root, M.shop, px, 3.05, z + d / 2 + .05, w / 4.0, .28, .10, false);
      box(root, M.steel, px, 2.95, z + d / 2 + .55, w / 4.0, .10, 1.10, false);
    }
    // 上部窗带
    for (let f = 1; f < floors; f++) {
      const fy = 3.4 + (f - 1) * 2.8 + 1.5;
      for (let n = 0; n < 4; n++) box(root, M.glass, x - w / 2 + 1.9 + n * ((w - 3.8) / 3), fy, z + d / 2 + .04, 1.5, 1.55, .06, false);
      box(root, M.steel, x, fy - 1.02, z + d / 2 + .06, w - .6, .10, .16, false);
    }
    // 屋面杂物：从公寓阳台/俯瞰机位看下去，屋面才是读作"有人在用"的地方
    kit.rooftop(x, z, w, d, h + .32);
    // 店面招牌
    const board = propBoard([['STATION EAST', '桜町駅前 · 中央通り'][0], ['OFFICE · SHOP · CLINIC', 'BAKERY & COFFEE', 'DAILY GOODS', 'CITY CLINIC'][i % 4]], { w: 512, h: 176 });
    materials.push(board);
    if (board.map) textures.push(board.map);
    const signGeo = new THREE.PlaneGeometry(w * .82, .86);
    geometries.add(signGeo);
    const sign = new THREE.Mesh(signGeo, board);
    sign.position.set(x, 3.66, z + d / 2 + .10);
    root.add(sign);
    claim(x, z, w / 2 + 1.2, d / 2 + 1.2);
    plan.stationBuildings++;
  });

  /* ---- A3 站前广场：公交站场 + 出租车 + 自行车 + 行道树 ---- */
  // 广场铺装（比草坪高 8cm，与内部车道同标高）。分块铺，让开既有住宅与花坛。
  plan.pavingTiles += pave('dress-station-paving', 116, 43, 36, 60, .04);
  const BUS_Z = [22, 36, 50];
  BUS_Z.forEach((z, i) => {
    if (free(124, z, 5.8, 1.4)) {
      kit.bus(124, z, Math.PI / 2);
      claim(124, z, 6.0, 1.6);
      plan.stationCars++;
    }
    if (free(129.6, z, 3.6, 1.0)) {
      kit.shelter(129.6, z, -Math.PI / 2, 6.4);
      claim(129.6, z, 3.8, 1.4);
    }
    const board = propBoard([['BUS 01 · 駅前循環', 'BUS 02 · 大学病院', 'BUS 03 · 港湾地区'][i], 'EVERY 12 MIN'], { w: 384, h: 192 });
    materials.push(board);
    if (board.map) textures.push(board.map);
    kit.signpost(128.4, z - 4.4, -Math.PI / 2, 2.6, 1.5, board);
  });
  /* 出租车排队区：x=112 一列朝北。**只留 3 辆**。
   * 原来排 6 辆（间距 5.4m，占 27m），读起来是一条静止的车龙；3 辆（间距 9m）才是
   * "站前有一个出租车乘车点"。出租车排队这件事本身没错，错的是密度。 */
  for (let n = 0; n < 3; n++) {
    const z = 22 + n * 9;
    if (!free(112, z, 1.2, 2.3)) continue;
    kit.car(112, z, Math.PI / 2, 6);
    claim(112, z, 1.3, 2.4);
    plan.stationCars++;
  }
  /* 短时停靠区（x=103.5 一列 8 辆）已整段拆除 —— 那是站前广场上最长的一条静止
   * 车队，正是用户点名的"一堆密集静止的汽车"。广场的"在用"读数改由
   * 出租车 + 公交站场 + 自行车棚 + 长椅花箱承担。 */
  // 自行车棚：四组，每组 5 个卡位 + 车
  for (let n = 0; n < 4; n++) {
    const x = 107.5, z = 30 + n * 9.5;
    if (!free(x, z, 2.2, .8)) continue;
    kit.rack(x, z, 0, 5);
    claim(x, z, 2.3, .9);
    for (let b = 0; b < 4; b++) kit.bike(x - 1.2 + b * .8, z + .30, Math.PI / 2, (n + b) % 2);
  }
  // 广场家具：行道树 + 路灯 + 长椅 + 垃圾桶 + 花箱
  for (let z = 16; z <= 72; z += 8) {
    tree(99.5, z, (z / 8) % 2 === 0, .40);
    if ((z / 8) % 3 === 0) tree(119, z, false, .40);
    if (free(101.5, z, .8, .8)) { kit.lamp(101.5, z, -Math.PI / 2, 5.6); claim(101.5, z, .8, .8); plan.lamps++; }
    if (free(131.5, z, .8, .8)) { kit.lamp(131.5, z, Math.PI / 2, 5.6); claim(131.5, z, .8, .8); plan.lamps++; }
  }
  for (let n = 0; n < 7; n++) {
    const z = 18 + n * 8.6;
    if (free(116.5, z, 1.0, .4)) { kit.bench(116.5, z, Math.PI); claim(116.5, z, 1.0, .5); plan.furniture++; }
    if (free(121.5, z + 3, .4, .4)) { kit.bin(121.5, z + 3); claim(121.5, z + 3, .4, .4); plan.furniture++; }
    if (free(113.5, z + 4, 1.2, .5)) { kit.planter(113.5, z + 4, 2.2, 0); claim(113.5, z + 4, 1.2, .5); plan.furniture++; }
  }
  for (const x of [102.5, 108.5, 114.5, 120.5, 126.5, 132.5]) kit.bollard(x, 13.4);
  // 站前大牌（面向广场西侧，从站台出来第一眼看到）
  {
    const b = propBoard(['SAKURA STATION', '桜町駅 東口 · EAST GATE'], { w: 768, h: 256, bg: '#2b4a5e' });
    materials.push(b);
    if (b.map) textures.push(b.map);
    const g = new THREE.PlaneGeometry(6.4, 2.1);
    geometries.add(g);
    const m = new THREE.Mesh(g, b);
    m.position.set(99.2, 3.4, 43); m.rotation.y = -Math.PI / 2;
    root.add(m);
    for (const dz of [-2.4, 2.4]) box(root, M.steel, 99.0, 1.7, 43 + dz, .22, 3.4, .22);
  }

  /* ---- A4 南段：露天停车场 + 口袋绿地 ----
   * 车位**画满 7 个**（划线是场地的一部分），但只停 3 辆 —— 空着的车位反而更像
   * 一个真在用的停车场：满位读作"库存"，半空读作"有人来有人走"。 */
  plan.pavingTiles += pave('dress-station-park-lot', 118, 84, 30, 14, .045, 4, M.road);
  for (let row = 0; row < 2; row++) for (let col = 0; col < 7; col++) {
    const x = 106 + col * 3.4, z = 79.5 + row * 8.0;
    box(root, M.paint, x, .10, z + 2.2, .10, .01, 4.2, false);
    if (col > 2 || !free(x, z, 1.2, 2.3)) continue;
    kit.car(x, z, Math.PI / 2, (row * 3 + col) % 8);   // 车头朝里，与 3.4m 柱距的车位对得上
    claim(x, z, 1.3, 2.4);
    plan.stationCars++;
  }
  for (let n = 0; n < 4; n++) {
    const x = 106 + n * 10;
    if (free(x, 92, 2.0, 1.0)) { kit.planter(x, 92, 3.0, 0); claim(x, 92, 2.0, 1.0); plan.furniture++; }
    tree(x + 4.6, 92.5, n % 2 === 1, .40);
  }
  for (let n = 0; n < 3; n++) if (free(140, 80 + n * 6, .8, .8)) { kit.lamp(140, 80 + n * 6, Math.PI, 5.6); claim(140, 80 + n * 6, .8, .8); plan.lamps++; }

  /* ================================================================
   * A5. 站东北段（x 100..148, z -78..-46）—— 第二轮，填重扫后的 TOP1
   * ================================================================
   * 第一轮做完，重扫空地仍然是 `6736 m² x[100,168] z[-82,118] 最大净距 52m`。
   * 东区占位图把这一坨拆开看，真正成片的空只剩四处，北段是最大的一块：
   * **z -52..-48 那一整行 x 100..144 全空**，往上到边界 z=-78 都没有东西。
   * 补法照旧——先铺一条东西向车道，楼骑在两侧，车道两侧停车种树。
   */
  const northRoutes: CityRoute[] = [
    { name: 'dress-north-lane', points: [[100, -60], [116, -60], [132, -60], [150, -60]], width: 5.0 },
  ];
  allRoutes.push(...northRoutes);
  for (const g of buildCityRoutes(root, colliders, northRoutes, { road: M.road, paving: M.paving, paint: M.paint }, .07)) geometries.add(g);

  /* 横断面是按实际余量配的，不是拍脑袋：
   *   北排楼  z -76..-66   (d=10, 中心 -71)
   *   北侧绿化带 -65.4..-62.5   ← 行道树 2.2m 宽刚好塞得下
   *   车道    z -62.5..-57.5   (w=5.0, 中心 -60)
   *   南侧绿化带 -57.5..-54.6
   *   南排楼  z -54..-44   (d=10, 中心 -49)
   * 楼体 claim 只外扩 0.6（不是 1.2）—— 雨棚在 2.95m 高，不占地面，外扩 1.2 会把
   * 两侧绿化带整条吃掉，树和车就全被 free() 顶掉了。 */
  for (let i = 0; i < 6; i++) {
    const x = 106 + (i % 3) * 18, z = i < 3 ? -71 : -49;
    const w = 12, d = 10;
    tryAt(x, z, w / 2, d / 2, (px, pz) => {
      const floors = 3 + (i % 3);
      const h = 3.4 + (floors - 1) * 2.8;
      solid(`dress-north-block-${i}`, root, M.facade[(i + 2) % M.facade.length], px, h / 2, pz, w, h, d);
      box(root, M.roof, px, h + .16, pz, w + .34, .32, d + .34);
      const face = z < -60 ? 1 : -1;                       // 店面朝车道
      for (let k = 0; k < 3; k++) {
        const qx = px - w / 2 + 2.1 + k * ((w - 4.2) / 2);
        box(root, M.glass, qx, 1.75, pz + face * (d / 2 + .04), w / 4.4, 2.4, .06, false);
        box(root, M.shop, qx, 3.05, pz + face * (d / 2 + .05), w / 4.0, .28, .10, false);
        box(root, M.steel, qx, 2.95, pz + face * (d / 2 + .55), w / 4.0, .10, 1.10, false);
      }
      for (let f = 1; f < floors; f++) {
        const fy = 3.4 + (f - 1) * 2.8 + 1.5;
        for (let k = 0; k < 4; k++) box(root, M.glass, px - w / 2 + 1.7 + k * ((w - 3.4) / 3), fy, pz + face * (d / 2 + .04), 1.4, 1.55, .06, false);
      }
      kit.rooftop(px, pz, w, d, h + .32);
      roofGarden(px, pz, w, d, h + .32);
      // 爬藤走**山墙**（东西两侧），不挡朝车道的店面那一面。
      wallVine(px - w / 2 - .10, pz, 0, 1, d - 2.6, h);
      wallVine(px + w / 2 + .10, pz, 0, 1, d - 2.6, h);
      claim(px, pz, w / 2 + .6, d / 2 + .6);
      plan.northBlocks++;
    });
  }
  for (let x = 102; x <= 146; x += 6.2) {
    tree(x, -63.9, (x % 12.4) < 6.2, .34);                 // 北侧绿化带
    tree(x + 3.1, -56.1, (x % 12.4) >= 6.2, .34);          // 南侧绿化带
    if (free(x + 3.1, -63.9, .7, .7)) { kit.lamp(x + 3.1, -63.9, Math.PI, 5.6); claim(x + 3.1, -63.9, .7, .7); plan.lamps++; }
    if (free(x, -56.1, .7, .7)) { kit.lamp(x, -56.1, 0, 5.6); claim(x, -56.1, .7, .7); plan.lamps++; }
    /* 这里原本沿车道每 6.2m 停一辆车（8 辆一列）。同 B 段，已拆 ——
     * 车道两侧留树与路灯就够读作"有人在维护"，一排静止车只会读作停车场。 */
  }
  for (const x of [115, 133]) if (free(x, -71, .5, 5.0)) { kit.hedge(x, -71, Math.PI / 2, 10.0); claim(x, -71, .5, 5.0); plan.hedges++; }

  /* ================================================================
   * A6. 站东中带（x 100..132, z 0..14）—— 第二轮
   * ================================================================
   * 占位图里 z 0..12 那一整行 x 100..132 也是全空：夹在商务段（z ≤ -2）与
   * 站前广场（z ≥ 16）之间的一条横缝。这里不放大体量（会把广场和商务段
   * 糊成一片），只放**有中心的东西**：报刊亭一排 + 绿篱 + 行道树。
   */
  for (let n = 0; n < 3; n++) {
    const board = propBoard(['KIOSK · 売店', ['NEWS & DRINK', 'COFFEE · BREAD', 'DAILY GOODS'][n], 'OPEN 7:00 - 21:00'], { w: 384, h: 200 });
    materials.push(board);
    if (board.map) textures.push(board.map);
    tryAt(106 + n * 9, 11.6, 1.3, 1.0, (px, pz) => {
      kit.kiosk(px, pz, Math.PI, board);
      claim(px, pz, 1.5, 1.1);
      plan.kiosks++;
    });
  }
  for (const x of [104, 116, 128]) {
    if (free(x, 1.8, 4.0, .5)) { kit.hedge(x, 1.8, 0, 8.0); claim(x, 1.8, 4.0, .5); plan.hedges++; }
  }
  for (let x = 102; x <= 130; x += 5.6) {
    if (free(x, 13.4, .4, .4)) { kit.bin(x, 13.4); claim(x, 13.4, .4, .4); plan.furniture++; }
    if (free(x + 2.8, 0, .4, .4)) { kit.mailbox(x + 2.8, 0, 0); claim(x + 2.8, 0, .4, .4); plan.furniture++; }
  }

  /* ================================================================
   * A7. 站东段：球场 + 游乐场 + 东缘服务带（x 124..168, z 34..118）—— 第二轮
   * ================================================================
   * 第一轮往这里塞的是车和灯 —— 它们填不掉面积：一辆车 9 m²，而这里成片的
   * 空是 20×20 起步。这一轮换成"一件圈一块地"的东西：**街头球场**（围网把
   * 20×14 整块读作有人用）、**儿童游乐场**、**绿篱**（把空地切成有界两块）。
   */
  // 注意：旋转 90° 的那两件，`tryAt` 的 hw/hd 必须**跟着转**（球场 20×14 转成
  // 世界坐标 14×20）。用未旋转的尺寸去判交会漏掉长边那一截，冲突扫描就会抓到。
  tryAt(138, 44, 8.4, 6.4, (px, pz) => { kit.court(px, pz, 16, 12, 0); claim(px, pz, 8.4, 6.4); plan.courts++; });
  tryAt(136, 66, 4.0, 4.0, (px, pz) => { kit.playUnit(px, pz, 0); claim(px, pz, 4.0, 4.0); plan.plays++; });
  tryAt(136, 100, 6.4, 8.4, (px, pz) => { kit.court(px, pz, 16, 12, Math.PI / 2); claim(px, pz, 6.4, 8.4); plan.courts++; });
  tryAt(116, 100, 4.0, 4.0, (px, pz) => { kit.playUnit(px, pz, Math.PI / 2); claim(px, pz, 4.0, 4.0); plan.plays++; });
  // 口袋公园的边界：绿篱把球场/游乐场与道路隔开
  for (const [hx, hz, hr, hl] of [
    [138, 34.5, 0, 20], [138, 55, 0, 20], [136, 76, 0, 18], [136, 112, 0, 18],
  ] as [number, number, number, number][]) {
    if (free(hx, hz, hl / 2, .5)) { kit.hedge(hx, hz, hr, hl); claim(hx, hz, hl / 2, .5); plan.hedges++; }
  }
  for (let z = 36; z <= 112; z += 7.6) {
    tree(126.4, z, (z % 15.2) < 7.6, .36);
    if (free(147.5, z, .8, .8)) { kit.lamp(147.5, z, -Math.PI / 2, 5.6); claim(147.5, z, .8, .8); plan.lamps++; }
    if (free(129.5, z + 3, 1.0, .5)) { kit.bench(129.5, z + 3, -Math.PI / 2); claim(129.5, z + 3, 1.0, .5); plan.furniture++; }
  }
  for (const x of [128.5, 143.5]) for (const z of [40, 62, 86, 104]) {
    if (free(x, z, .4, .4)) { kit.bin(x, z); claim(x, z, .4, .4); plan.furniture++; }
  }
  // 东缘：x 150 是**不可见**的玩家边界墙（1m 厚，无几何），墙外 x 152..168 是一片
  // 纯空地 —— 玩家走不过去，但从街区里往外看、以及俯瞰机位都看得见。放一排低层
  // 体量把天际线接上。x=160 / w=14 ⇒ 153..167，离边界墙 keepOut（149.05）留 4m。
  // 步长 22、进深 18/14 交替：上一版步长 24 进深 20 会在 z 14..46 和 z 114..118
  // 留出两条 256/144 m² 的缝（空地表里最大的两块残块）。
  for (let z = -72, i = 0; z <= 152; z += 22, i++) {
    const d = i % 3 === 1 ? 14 : 18;
    const h = 7.6 + (i % 4) * 1.7;
    tryAt(160, z, 7.0, d / 2, (px, pz) => {
      solid('dress-east-block', root, M.facade[i % M.facade.length], px, h / 2, pz, 14, h, d);
      box(root, M.roof, px, h + .16, pz, 14.4, .32, d + .4);
      for (let f = 0; f < Math.floor(h / 2.8); f++) {           // 朝西（朝街区）的窗带
        const fy = 1.9 + f * 2.8;
        for (let k = 0; k < 4; k++) box(root, M.glass, px - 7.05, fy, pz - d / 2 + 2.6 + k * ((d - 5.2) / 3), .06, 1.35, 1.3, false);
      }
      kit.rooftop(px, pz, 14, d, h + .32);
      claim(px, pz, 8.0, d / 2 + 1.0);
      plan.eastBlocks++;
    });
  }

  /* 北侧边界墙外（z -94..-80）—— 第二轮收尾补的一条绿化带。
   * 空地表里 `x[-92,-84] / [-72,-64] / [-36,-28] / [0,8] / [36,44] / [100,104]
   * z[-98,-94]` 那六块 48~64 m² 的洞都在这里：北边界墙（z -79..-78）外面一条
   * 16m 宽的空带。玩家走不过去，但从街区里往北看、以及俯瞰机位都看得见。
   * 这里**不放楼**（墙外起楼会把远景环的层次搞乱），只种树 + 立灯 + 绿篱。 */
  for (let x = -96; x <= 140; x += 7.4) {
    tree(x, -86.5, (x % 14.8) < 7.4, .34);
    if (free(x + 3.7, -90.5, .7, .7)) { kit.lamp(x + 3.7, -90.5, 0, 5.2); claim(x + 3.7, -90.5, .7, .7); plan.lamps++; }
    /* 绿篱要让开**铁路走廊**（`districtArt.TRACK_GAP` = x 74…106）。每段 7.4m 长，
     * x=89 那一段占 x 85.3…92.7 —— 正好把两条轨（x=86 / 91）一起罩住，列车会从
     * 绿篱里穿过去。
     *
     * **`free()` 看不见它**：枕木、道砟、草皮、钢轨全都是用**不带碰撞盒的 `box()`**
     * 画的，所以任何基于 `colliders` 的冲突扫描都不可能报出来。判据只能用几何：
     * 「实体盒 footprint 含轨道中心线 x=86 或 x=91」——见 `tmp/_probe-park-site.mjs`
     * 的「压轨清单」。 */
    const hedgeOnTrack = x + 3.7 > 74 && x - 3.7 < 106;
    if (!hedgeOnTrack && free(x, -81.6, 3.7, .4)) { kit.hedge(x, -81.6, 0, 7.4); claim(x, -81.6, 3.7, .4); plan.hedges++; }
  }

  /* ================================================================
   * A8. 站东南带（z 92..118）—— 第二轮
   * ================================================================
   * 露天停车场南侧到边界之间还有一条 26m 深的横带。放低密度绿地：绿篱 + 树 +
   * 一条人行道，别放楼 —— 这里已经是街区的南端，放楼会把南区市政厅的轮廓压掉。
   */
  {
    const southWalk: CityRoute[] = [
      { name: 'dress-south-walk', points: [[98, 114], [116, 114], [134, 114], [150, 114]], width: 3.2, walk: true },
    ];
    allRoutes.push(...southWalk);
    for (const g of buildCityRoutes(root, colliders, southWalk, { road: M.road, paving: M.paving, paint: M.paint }, .07)) geometries.add(g);
  }
  for (let x = 100; x <= 148; x += 5.2) {
    tree(x, 96.5, (x % 10.4) < 5.2, .34);
    tree(x + 2.6, 105.5, (x % 10.4) >= 5.2, .34);
    if (free(x, 111.5, 4.0, .5)) { kit.hedge(x, 111.5, 0, 8.0); claim(x, 111.5, 4.0, .5); plan.hedges++; }
  }

  /* ================================================================
   * B. 沿街陈设：让每条街都"有东西"
   * ================================================================
   * 采样按 6.2m 一步（与 urbanInfill 同节奏），两侧各有两种落位：
   *   - 路缘侧（offset = 路宽/2 - 1.05）：**已经不放车了**，见下方注释
   *   - 人行道侧（offset = 路宽/2 + 1.6）：路灯 / 行道树 / 零散家具
   * 全部先过 `free()`，已经种了树、立了灯的地方会自动跳过。
   *
   * **路缘停车整段拆掉（第二轮返工）。** 第一版在这里每 12.4m 停一辆、两侧都停，
   * 18 条街下来 122 辆静止汽车。用户的原话是「不要再道路上放一堆密集静止的汽车，
   * 你可以做少量动态的汽车」。这个判断是对的：一排等距的静止车只把空地表填下去，
   * 街道被做成了停车场 —— 而"这条街在用"这个读数靠的是**有车在开**，
   * 那部分现在在 `streetTraffic.ts`（两圈环路、6 辆真在跑）。
   * 拆的时候注意：`kit.car()` 不消耗 `random()`，删掉它不会让后面所有树的随机
   * 序列整体错位（这一点先核过才敢删）。
   */
  const routes: CityRoute[] = [...CITY_ROUTES, ...SOUTH_ROUTES];
  allRoutes.push(...routes);
  const streetTrees: { x: number; z: number; bloom: boolean }[] = [];
  /* 沿街指示牌**共用一个牌面材质**：几十根牌子各建一张 canvas 是纯浪费，
   * 而且牌子内容本来就该是重复的（同一条街的路名）。 */
  const streetSignBoard = propBoard(['桜町中央通り', 'SAKURAMACHI CHUO ST'], { w: 512, h: 176, bg: '#2b4a5e' });
  for (const route of routes) {
    const curve = new THREE.CatmullRomCurve3(route.points.map(([x, z]) => new THREE.Vector3(x, 0, z)), false, 'centripetal');
    const length = curve.getLength();
    for (let at = 6; at < length - 5; at += 6.2) {
      const t = at / length;
      const p = curve.getPointAt(t);
      const tan = curve.getTangentAt(t);
      const nx = -tan.z, nz = tan.x;
      const walkOff = route.width / 2 + 1.6;
      const step = Math.round(at / 6.2);
      for (const side of [-1, 1]) {
        const wx = p.x + nx * side * walkOff, wz = p.z + nz * side * walkOff;
        // 路灯：每 4 步（24.8m）一盏，悬臂朝车道
        if (step % 4 === 1 && free(wx, wz, .7, .7)) {
          kit.lamp(wx, wz, Math.atan2(-nx * side, -nz * side), 5.6);
          claim(wx, wz, .7, .7);
          plan.lamps++;
        }
        // 行道树：每 9 步（56m）一棵，左右交替，樱花与绿树轮换
        if (step % 9 === 3 && (side > 0) === (step % 18 === 3)) streetTrees.push({ x: wx, z: wz, bloom: (step / 9) % 2 === 0 });
        /* 零散家具：每 5 步一件（31m），按 `step/5` 轮换 7 种。
         *
         * 原来每 7 步（43.4m）一件，而且判据写成 `((step + side) / 7) % 5` ——
         * 那是**浮点取模**，`pick === 0/1/2/3` 几乎永远不成立，实际绝大多数落到
         * 最后的 `else` ⇒ 整条街的"零散家具"其实是清一色的消火栓（一个箱子都
         * 没摆出来）。改成整数取模后 7 种才真正轮得起来。
         * 多出来的两种是自动售货机和指示牌 —— 日本街道密度最高的两样东西。 */
        if (step % 5 === 2) {
          const pick = (Math.floor(step / 5) + (side > 0 ? 1 : 0)) % 7;
          if (pick === 0 && free(wx, wz, .4, .4)) { kit.bin(wx, wz); claim(wx, wz, .4, .4); plan.furniture++; }
          else if (pick === 1 && free(wx, wz, 1.0, .4)) { kit.bench(wx, wz, Math.atan2(nx * side, nz * side)); claim(wx, wz, 1.0, .5); plan.furniture++; }
          else if (pick === 2 && free(wx, wz, .7, .5)) { kit.cabinet(wx, wz, Math.atan2(tan.x, tan.z)); claim(wx, wz, .7, .5); plan.furniture++; }
          else if (pick === 3 && free(wx, wz, 1.1, .5)) { kit.planter(wx, wz, 2.0, Math.atan2(tan.x, tan.z)); claim(wx, wz, 1.1, .5); plan.furniture++; }
          else if (pick === 4 && free(wx, wz, .95, .75)) { kit.vending(wx, wz, Math.atan2(nx * side, nz * side)); claim(wx, wz, .95, .75); plan.furniture++; }
          else if (pick === 5 && free(wx, wz, .5, .5)) { kit.signpost(wx, wz, Math.atan2(nx * side, nz * side), 2.7, .95, streetSignBoard); claim(wx, wz, .5, .5); plan.furniture++; }
          else if (free(wx, wz, .3, .3)) { kit.hydrant(wx, wz); claim(wx, wz, .3, .3); plan.furniture++; }
        }
        /* 护柱：沿人行道每 2.4m 一根，4 根一小段。细杆不登记碰撞（见
         * `streetProps` 文件头第 3 条：一根 0.08m 的杆子不该拦住玩家），
         * 但占位要 `claim` 掉，否则后面的家具会插进柱阵里。 */
        if (step % 11 === 6) {
          for (let b = 0; b < 4; b++) {
            const bx = wx + tan.x * (b - 1.5) * 2.4, bz = wz + tan.z * (b - 1.5) * 2.4;
            if (free(bx, bz, .22, .22)) { kit.bollard(bx, bz); claim(bx, bz, .22, .22); }
          }
        }
        // 自行车架：每 13 步一组
        if (step % 13 === 5) {
          const ry = Math.atan2(tan.x, tan.z);
          if (free(wx, wz, 1.9, .8)) {
            kit.rack(wx, wz, ry, 5);
            claim(wx, wz, 2.0, .9);
            for (let b = 0; b < 4; b++) {
              const off = -1.2 + b * .8;
              kit.bike(wx + Math.cos(ry) * off, wz - Math.sin(ry) * off, ry + Math.PI / 2, b % 2);
            }
            plan.furniture++;
          }
        }
      }
      /* 跨街拉旗：每 18 步（111.6m）一道。原来每 9 步（55.8m）一道 —— 一条街上
       * 隔五十米就来一串，密到把"节庆"读成了"一路布景"，减半。
       * 另外 `banner` 现在自带两端立杆：原来那串旗悬在 4.4m 空中、两侧没有挂点。 */
      if (step % 18 === 4) {
        const dir = new THREE.Vector3(nx, 0, nz);
        const a = p.clone().addScaledVector(dir, walkOff + .3);
        const b = p.clone().addScaledVector(dir, -(walkOff + .3));
        // 杆要落地：落点上已经有树 / 灯 / 家具就跳过，否则杆会从树冠里穿出来。
        if (free(a.x, a.z, .45, .45) && free(b.x, b.z, .45, .45)) kit.banner(a.x, a.z, b.x, b.z, 4.4, 5);
      }
    }
  }
  for (const t of streetTrees) tree(t.x, t.z, t.bloom, .40);

  /* 信号灯：南北大道（x=-46 / 43）与东西向街道的路口。细杆不登记碰撞，
   * 纯粹是"这里有路口"的读数 —— 一条街上没有路口标识，纵有路灯也还是像布景。 */
  for (const [tx, tz] of [[-46, -19], [-46, 10.1], [-46, 30], [-46, 61], [43, -19], [43, 10.1], [43, 30], [43, 61]]) {
    const off = tx < 0 ? 4.8 : -4.8;
    if (free(tx + off, tz - 4.4, .6, .6)) kit.trafficLight(tx + off, tz - 4.4, tx < 0 ? 0 : Math.PI, 5.8);
  }

  /* ================================================================
   * C. 西缘服务带：填掉 4512 m² 的西侧空带
   * ================================================================
   * 街区活动范围西界是 x=-80，远景墙内圈在 x=-114，中间 34m 全是空沥青。
   * 而这一段是**玩家真的会站到边上**的位置（西侧几条街的尽头）。补一条低层
   * 服务带：仓储/工坊 + 货车 + 路灯，高度压在 8~11m，不抢街区内公寓的天际线。
   * 体量放在 x=-94（footprint -102..-86），离远景墙内圈 8m 净距。
   */
  const westLane: CityRoute[] = [{ name: 'dress-west-lane', points: [[-88, -72], [-88, -20], [-88, 40], [-88, 100], [-88, 156]], width: 4.4 }];
  allRoutes.push(...westLane);
  for (const g of buildCityRoutes(root, colliders, westLane, { road: M.road, paving: M.paving, paint: M.paint }, .06)) geometries.add(g);
  for (let z = -68; z <= 152; z += 22) {
    const w = 16, d = 14, x = -94;
    if (!free(x, z, w / 2, d / 2)) continue;
    const idx = Math.abs(Math.round(z / 22));
    const h = 7.4 + (idx % 3) * 1.6;
    solid('dress-west-block', root, M.facade[idx % M.facade.length], x, h / 2, z, w, h, d);
    box(root, M.roof, x, h + .14, z, w + .4, .28, d + .4);
    // 卷帘门 + 雨棚，朝东（朝街区）
    for (let n = 0; n < 3; n++) {
      const pz = z - d / 2 + 2.6 + n * ((d - 5.2) / 2);
      box(root, M.shop, x + w / 2 + .05, 1.6, pz, .08, 3.2, d / 4.2, false);
      box(root, M.steel, x + w / 2 + .55, 3.35, pz, 1.10, .10, d / 3.8, false);
    }
    kit.rooftop(x, z, w, d, h + .28);
    roofGarden(x, z, w, d, h + .28);
    // 卷帘门那一面朝东（朝街区），爬藤放南北山墙。
    wallVine(x, z - d / 2 - .10, 1, 0, w - 2.8, h);
    wallVine(x, z + d / 2 + .10, 1, 0, w - 2.8, h);
    claim(x, z, w / 2 + 1.4, d / 2 + 1.4);
    plan.westBlocks++;
    /* 货车：**隔一个街区一辆**。原来每个街区两辆（11 个街区 22 辆），沿西缘
     * 连成一条静止的车列 —— 那是同一类问题（车列 ≠ 在用）。留 6 辆配送车，
     * 稀疏到读起来是"每个工坊各自有一辆"，而不是"这里排了一队"。 */
    if (idx % 2 === 0 && free(-83.5, z - 4, 1.2, 2.8)) {
      kit.van(-83.5, z - 4, Math.PI / 2, idx % 8);
      claim(-83.5, z - 4, 1.3, 2.9);
      plan.westVans++;
    }
  }
  for (let z = -70; z <= 154; z += 12) {
    if (free(-83.2, z, .7, .7)) { kit.lamp(-83.2, z, Math.PI / 2, 5.2); claim(-83.2, z, .7, .7); plan.lamps++; }
  }
  for (let z = -66; z <= 150; z += 16) tree(-84.6, z, false, .35);

  /* ================================================================
   * D. 扫街绿化：把"仍然空旷"的点自动种上（"不断填充"的最后一道网）
   * ================================================================
   * 分区落位总会在边角留下几十平米的碎缝。第二轮做完，`_probe-dressing.mjs`
   * 的空地表里还剩 944 m² / 12 块，全部 ≤96 m²、净距 12~16m —— 手写坐标一个个
   * 去补是补不完的（补掉一块，旁边又冒出一块）。
   *
   * 所以改成**用判据驱动**：按 8m 网格扫全域，凡是
   *   ① `free(x, z, 4, 4)` —— 放得下一个 8×8 的空位（等价于"离最近实体还有
   *      4m 以上"，比空地表那条"≥12m 净距"更严），
   *   ② `roadClear(x, z)` —— 不落在任何一条路的路面上，
   *   ③ 不在铁路走廊里，
   * 就种一棵树（樱花/绿树交替），每第三棵改成一段绿篱。
   *
   * 这一遍是**幂等**的：空地越多它种得越多，填完重扫自然就没有 ≥12m 的洞了。
   * 以后再加内容也不用重新调它 —— 它读的是同一套 `free()` 判据。
   */
  {
    // 路网掩膜：沿每条路线按 3m 采样，记下圆心与半径（半宽 + 2.2m 人行道）。
    const roadPts: { x: number; z: number; r: number }[] = [];
    for (const route of allRoutes) {
      const curve = new THREE.CatmullRomCurve3(route.points.map(([x, z]) => new THREE.Vector3(x, 0, z)), false, 'centripetal');
      const len = curve.getLength();
      for (let at = 0; at <= len; at += 3) {
        const p = curve.getPointAt(Math.min(1, at / len));
        roadPts.push({ x: p.x, z: p.z, r: route.width / 2 + 2.2 });
      }
    }
    const roadClear = (x: number, z: number) => {
      for (const p of roadPts) { const dx = x - p.x, dz = z - p.z; if (dx * dx + dz * dz < p.r * p.r) return false; }
      return true;
    };
    /* 铁路走廊 x 74..106（`districtArt.ts` 的 TRACK_GAP）—— 道床、轨枕、接触网
     * 都在那儿，绿化带退到走廊之外。这里硬编码而不 import：districtArt 反过来
     * import 本模块，import 常量会成环。 */
    const inTrack = (x: number) => x > 74 && x < 106;
    /* 抖动。8m 网格种出来的树是**等距方阵**，从俯瞰机位一眼就露馅（取证帧 V12 里
     * 那排整齐的深绿点）。用坐标散列做一个 ±1.4m 的确定性抖动 —— 不用 `random()`，
     * 免得挪动 `plantTree` 的调用位置把后面所有随机内容整体错位。
     * 抖动后仍要过一遍 `free(…, 3.6, 3.6)`：一件绿篱最长 4.8m（半长 2.4），
     * 加 1.4m 抖动正好落在原来那 8×8 净空里，过不了就退回格点。 */
    const jitter = (a: number, b: number) => ((((a * 73856093) ^ (b * 19349663)) >>> 0) % 1000) / 1000 - .5;
    let planted = 0;
    for (let x = -104; x <= 166 && planted < 140; x += 8) {
      if (inTrack(x)) continue;
      for (let z = -92; z <= 162 && planted < 140; z += 8) {
        if (!roadClear(x, z)) continue;
        if (!free(x, z, 4, 4)) continue;             // 8×8 净空 ⇒ 这里确实还空着
        const jx = jitter(x, z) * 2.8, jz = jitter(z, x) * 2.8;
        const jOk = free(x + jx, z + jz, 3.6, 3.6);
        const px = jOk ? x + jx : x;
        const pz = jOk ? z + jz : z;
        if (planted % 3 === 2) {
          const alongX = ((x / 8) | 0) % 2 === 0;
          kit.hedge(px, pz, alongX ? 0 : Math.PI / 2, 4.8);
          claim(px, pz, alongX ? 2.4 : .5, alongX ? .5 : 2.4);
          plan.hedges++;
        } else {
          tree(px, pz, ((x + z) / 8) % 2 === 0, .30);
        }
        planted++;
      }
    }
    plan.sweep = planted;
  }

  /* ---------------- 合批 + 冠簇 + 树干 ---------------- */
  mergeByMaterial(root);
  root.add(kitTree.trunk.build('dressing-trunks', trunkMat.color));
  root.add(blossom.build('dressing-blossom'));
  root.add(leaf.build('dressing-leaves'));
  // 绿篱的叶簇同理：合批之后才能挂，否则会被 mergeByMaterial 当成一块几何烘掉。
  root.add(kit.hedgeLeaf.build('dressing-hedge-leaves'));
  // 屋顶绿化 + 立面爬藤共用一场，同样等合批之后。
  root.add(roofLeaf.build('dressing-roof-planting'));
  root.userData.plan = plan;

  return {
    colliders,
    plan,
    dispose() {
      root.removeFromParent();
      root.traverse((o) => { if (o instanceof THREE.Mesh) geometries.add(o.geometry); if (o instanceof THREE.InstancedMesh) o.dispose(); });
      for (const g of geometries) g.dispose();
      for (const m of materials) m.dispose();
      for (const t of textures) t.dispose();
      blossom.material.dispose(); blossom.texture.dispose();
      leaf.material.dispose(); leaf.texture.dispose();
      kit.hedgeLeaf.material.dispose(); kit.hedgeLeaf.texture.dispose();
      roofLeaf.material.dispose(); roofLeaf.texture.dispose();
    },
  };
}