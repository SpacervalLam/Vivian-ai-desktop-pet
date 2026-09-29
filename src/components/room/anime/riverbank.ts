import * as THREE from 'three';
import { toon, makeRng } from './toon';
import { buildStreetLamp } from './props';
import { mergeByMaterial } from './merge';
import { createBlossomField, createLeafField, createFallenPetals, createTrunkField, plantTree, type TreeKit } from './foliage';
import { RIVER_Z_S, RIVER_Z_N, RIVER_SURFACE_Y, RIVER_BED_Y } from './river';
import { TRACK_GAP } from './districtArt';

/**
 * 小河两岸的滨水空间（黄浦江两岸那套：防汛墙 + 亲水步道 + 铸铁栏杆 + 路灯 +
 * 成排的树 + 下到水边的台阶平台）。
 *
 * 与河道本体（exterior.ts）分开成模块，理由只有一个：**河是地形，堤岸是建筑**。
 * 前者跟着世界地面走、改一次要动地面 Shape；后者是站在地面上的构件，可以单独增删。
 * 混在一起的话，调一次栏杆高度就得重新审一遍地面三角化。
 *
 * 两个硬性约束：
 *  1. **玩家到不了这里**。河道 z∈[-102,-88]，而 WALK_BOUNDS.z0 = -78（districtArt）——
 *     最近的可站立点离南岸还有 10m。所以整段不登记任何碰撞盒，也不做防坠落。
 *  2. **必须让开铁路走廊**（TRACK_GAP，districtArt）：x∈[74,106] 那一段是桥和路堤的
 *     地盘，步道和防汛墙铺过去会直接压在 203 的铁路绿化带上（z-fighting）。
 */

/** 步道宽度（从岸边往陆地一侧量）。 */
const WALK_W = 4.2;
/**
 * 石栏杆（黄浦江两岸那道花岗岩护栏）：墙身 + 压顶，立在步道**靠陆地的那一侧**。
 *
 * 位置不是随便挑的，是被视线几何逼出来的（river.ts 里那段"岸边退 1m 才许高
 * 0.17m"）：贴着岸边立任何连续的高物都会把水面挡死，退到 4.2m 之外才换来
 * 0.17×4.2 = 0.71m 的高度额度。总高取 0.62，留 9cm 余量。
 *
 * 想立更高的防汛墙就得把步道加宽——北侧做不到（那一侧 4.2m 外就是远景楼的
 * footprint），所以两岸统一按 4.2m / 0.62m 来，也是对得上的一档。
 */
const WALL_T = 0.34;
const WALL_H = 0.53;
const COPING_H = 0.09;
/**
 * 密的一段：只有 x∈[-260,340] 才排栏杆柱 / 树 / 路灯。
 *
 * 晴天雾 far=300（districtArt.setEnvironment），街区最远的视点是 (150,95)，往西北看
 * 到 x=-260 已经超过 300m —— 再往外排什么都只会沉在雾里，纯属白给顶点。
 * 防汛墙和步道本身是连续条带，不做这个截断：它们整条存在，断在雾外看不见。
 */
const DETAIL_X0 = -260;
const DETAIL_X1 = 340;

export function buildRiverbank(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'riverbank';

  const M = {
    walk: toon('#9a958c'),        // 步道铺装：暖灰花岗岩
    wall: toon('#a6a49c'),        // 防汛墙墙身
    coping: toon('#b9b6ad'),      // 压顶：比墙身亮一档，读得出"一条石带"
    shadow: toon('#7f7d76'),      // 压顶下的滴水线
    rail: toon('#4c535c', { finish: 'metal' }),   // 铸铁栏杆
    bench: toon('#7f6547'),
    edge: toon('#8f8a7f'),        // 树穴边石
    turf: toon('#79916b'),        // 树穴草皮
    bark: toon('#62594b'),        // 树皮（与街区行道树同色）
  };
  const mergeable = new THREE.Group();
  g.add(mergeable);

  const box = (x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) => {
    const o = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), m);
    o.position.set(x, y, z);
    mergeable.add(o);
    return o;
  };
  /** 一条沿 x 的连续带子。`x0/x1` 之外的部分跳过。 */
  const band = (m: THREE.Material, y: number, h: number, z0: number, z1: number, x0: number, x1: number) => {
    box((x0 + x1) / 2, y, (z0 + z1) / 2, x1 - x0, h, Math.abs(z1 - z0), m);
  };

  /* ---------------- 树的素材与画笔 ----------------
   *
   * 冠簇走 `foliage` 的实例收集器（和街区行道树、公园老樱、樱町商店街同一套
   * 贴图与材质），枝干走共用圆柱，**树干走 Blender 资产**（`foliage` 的干场，
   * 几何来自 `scripts/trees/build_trunks.py`）—— 河堤不自己造几何。
   *
   * 实例场必须**先收集、等 mergeByMaterial 之后再 build**：`mergeByMaterial` 只按
   * `isMesh` 过滤，InstancedMesh 也是 isMesh，混进去会被当成一块 2×2 面片烘掉
   * （这条坑 foliage 的注释里写着）。所以下面先把树"种"进 mergeable 的枝干里，
   * 冠簇与树干记在场上，合批之后再挂。
   *
   * 容量按实际棵数留余量：两岸各约 32 棵、其中 1/3 是樱、density 0.5 ——
   * 樱 21×7×(112+34)×0.5 ≈ 1.1 万，绿 43×5×(130+55)×0.5 ≈ 2.0 万，
   * 落瓣 21×190×0.5 ≈ 0.2 万。都取整上取整。 */
  const TREE_DENSITY = 0.5;
  const random = makeRng(20260927);
  const blossomField = createBlossomField(random, 12000);
  const leafField = createLeafField(random, 21000);
  const fallenField = createFallenPetals(random, 2400);
  const trunkField = createTrunkField();
  const branchGeo = new THREE.CylinderGeometry(0.75, 1, 1, 6);
  const upAxis = new THREE.Vector3(0, 1, 0);
  const rod = (a: number[], b: number[], r: number) => {
    const from = new THREE.Vector3(a[0], a[1], a[2]);
    const dir = new THREE.Vector3(b[0], b[1], b[2]).sub(from);
    const len = dir.length();
    const m = new THREE.Mesh(branchGeo, M.bark);
    m.position.copy(from).addScaledVector(dir, 0.5);
    m.quaternion.setFromUnitVectors(upAxis, dir.normalize());
    m.scale.set(r, len, r);
    mergeable.add(m);
  };
  const treeKit: TreeKit = {
    rod,
    trunk: trunkField,
    pit: (x, z) => { box(x, 0.14, z, 0.7, 0.2, 0.7, M.edge); box(x, 0.255, z, 0.58, 0.035, 0.58, M.turf); },
    random,
    blossom: blossomField,
    leaf: leafField,
    fallen: fallenField,
  };

  /* 两岸：`inward` 是"从岸边往陆地"的方向。南岸是 +z，北岸是 -z。
   *
   * `quay` = 这一岸砌成石砌驳岸（带石栏杆，规格见上面 WALL_* 那段）。
   *
   * **只有北岸有**，是算出来的而不是审美取舍：站在南岸看河的人，一点连续的遮挡
   * 都不能有。掠过岸边之后视线还要往前走 w·D/h 米才够得着水面，而许可高度是
   * h·s/D —— **D 越大许可高度越小**（远处的视线更平）。实测（_cdp-river.mjs 的
   * sight 取证）：0.62m 的石栏杆立在 4.2m 退界处，只有 z∈[-78,-76.5] 这 1.5m
   * 不挡水，再往南就把水面整个遮掉。
   *
   * 北岸不受这条约束：它本来就在水的另一侧。看过去是"水 → 驳岸 → 石栏杆 → 树 →
   * 远景楼"这一叠，栏杆挡的是自己身后的天，不挡水。对岸一道白石栏杆也正好是
   * 黄浦江两岸最好认的那一笔。 */
  const banks = [
    { id: 's', edge: RIVER_Z_S, inward: 1, quay: false },
    { id: 'n', edge: RIVER_Z_N, inward: -1, quay: true },
  ] as const;

  /* 连续构件按走廊切成东西两段；散点构件只在密的那一段里排。 */
  const X0 = -500, X1 = 500;
  const segs: Array<[number, number]> = [[X0, TRACK_GAP[0]], [TRACK_GAP[1], X1]];
  const detailSegs: Array<[number, number]> = [
    [DETAIL_X0, TRACK_GAP[0]],
    [TRACK_GAP[1], DETAIL_X1],
  ];

  for (const bk of banks) {
    const w0 = bk.edge, w1 = bk.edge + bk.inward * WALK_W;          // 步道：w0 临水、w1 靠陆
    let treeIndex = 0;
    /* 石栏杆立在**靠陆那一侧**（w1）。即使在北岸也不贴水：贴着岸边的构件每一米
     * 退界只换得到零点几米的高度，而驳岸本身还要站在步道外侧的地上。 */
    const g0 = w1 - bk.inward * WALL_T;
    for (const [x0, x1] of segs) {
      /* 步道铺装。顶面 0.008 而不是贴着地面：世界地面在 -0.012，两者相距 2cm ——
       * 2cm 在这个视距上完全读不出来，但足以让两层不再共面。 */
      band(M.walk, 0.002, 0.02, Math.min(w0, w1), Math.max(w0, w1), x0, x1);
      if (!bk.quay) continue;
      band(M.wall, WALL_H / 2, WALL_H, Math.min(w1, g0), Math.max(w1, g0), x0, x1);
      band(M.coping, WALL_H + COPING_H / 2, COPING_H, Math.min(w1, g0), Math.max(w1, g0), x0, x1);
      // 滴水线在临水那一侧：压顶挑出墙身 2cm，底下拖一道深色，石带才有厚度。
      band(M.shadow, WALL_H - 0.015, 0.03, g0, g0 + bk.inward * 0.02, x0, x1);
    }

    /* 行道树：每 18m 一棵，樱三棵里一棵、其余绿树 —— 全是樱会糊成一条粉带，
     * 全是绿又与"樱町"脱节。
     *
     * 树走 `foliage.plantTree`（和街区行道树、公园老樱同一套素材与构造），
     * 不在这里另起一套简单几何：`foliage.ts` 这个模块存在的理由就是消灭
     * "同一片场地里几种树各长各的"，河堤再写一棵就成了第三种。
     *
     * `density = 0.5`：河堤的树最近也在 15m 外，冠簇减半仍然是一团实的；
     * 枝干一根不动，所以剪影和近景的完全一致，只是更透。全密度的话两岸
     * 64 棵要吃掉六万多个实例，纯属给雾里的远景付账。
     *
     * 树是**断续**的（冠幅 2.5m、间距 18m），所以不受「贴岸构件每米许可 0.17m」
     * 那条限制：它挡掉的是 18m 里的一棵，树与树之间照样看得见河。 */
    const treeZ = bk.edge + bk.inward * 1.5;
    for (const [x0, x1] of detailSegs) {
      for (let x = x0 + 4; x <= x1; x += 18) {
        if (x > TRACK_GAP[0] && x < TRACK_GAP[1]) continue;
        plantTree(treeKit, x, treeZ, treeIndex % 3 === 0, treeIndex++, TREE_DENSITY);
      }
    }

    /* 面朝水的长椅：每 45m 一张，靠栏杆那一侧（离水最远），把临水那半条步道让出来。
     * 纯装饰（玩家走不到），但它把"这是给人待的地方"这一层读出来。 */
    const benchZ = bk.edge + bk.inward * (WALK_W - 0.7);
    for (const [x0, x1] of detailSegs) {
      for (let x = x0 + 22; x <= x1; x += 45) {
        if (x > TRACK_GAP[0] && x < TRACK_GAP[1]) continue;
        box(x, 0.44, benchZ, 1.7, 0.08, 0.44, M.bench);
        box(x, 0.68, benchZ - bk.inward * 0.20, 1.7, 0.46, 0.06, M.bench);
        for (const sx of [-0.7, 0.7]) box(x + sx, 0.22, benchZ, 0.07, 0.44, 0.4, M.rail);
      }
    }
  }

  /* ---------------- 亲水台阶平台 ----------------
   *
   * 三处下到水边的台阶：这是黄浦江两岸最好认的那一笔——防汛墙之外再往下探一级，
   * 人可以走到离水面很近的地方。
   *
   * 台阶落在岸坡上，所以每级都是"台阶面 + 埋进坡里的踢面"。岸坡 3m 水平投影降
   * 1.9m（exterior 的 BANK_RUN / BANK_DROP），走到 -0.5 只需 0.79m 水平距离，
   * 所以 4 级、级高 0.125、进深 0.198 —— 比坡缓，每级底下都埋进坡里，不会悬空。
   * 平台顶面 -0.5：比常水面（-0.85）高 35cm，读起来是"贴着水的台子"而不是码头。
   */
  const TERRACE_X = [-40, 30, 130];
  const LANDING_Y = -0.5;                  // 平台顶面：比常水面高 35cm
  for (const tx of TERRACE_X) {
    /* `toWater` 是"从岸边往水里"的方向。南岸的地在 z > RIVER_Z_S 一侧，所以往水是 -z；
     * 北岸反过来。搞反的话台阶会往陆地爬，读起来就是一处莫名其妙的抬升。 */
    for (const [toWater, edge] of [[-1, RIVER_Z_S], [1, RIVER_Z_N]] as const) {
      for (let k = 0; k < 4; k++) {
        const y = -0.125 * (k + 1);
        box(tx, y - 0.13, edge + toWater * (0.099 + k * 0.198), 2.8, 0.26, 0.198, M.coping);
      }
      const lz0 = edge + toWater * 0.79, lz1 = edge + toWater * 3.4;
      /* 平台做成**实心**而不是一块薄板：岸坡脚下的河床在 -2.75，悬空一块板从岸上
       * 看过去是飘着的。一直填到河床，读起来才是砌出来的台子，也顺手把坡脚收平。 */
      box(tx, (RIVER_BED_Y + LANDING_Y) / 2, (lz0 + lz1) / 2, 2.8,
        LANDING_Y - RIVER_BED_Y, Math.abs(lz1 - lz0), M.wall);
      box(tx, LANDING_Y + 0.03, (lz0 + lz1) / 2, 2.9, 0.06, Math.abs(lz1 - lz0), M.coping);
    }
  }

  mergeByMaterial(mergeable);

  /* 冠簇与树干：合批之后再挂（见上面树素材那段的说明）。落瓣铺在步道上，
   * 樱树那几段因此自带一层花瓣 —— 和樱町商店街是同一套东西。
   *
   * 树干用**河堤自己的树皮色**（M.bark），不是资产自带的材质：几何来自 Blender，
   * 上色仍归本地，于是河边这排树和街区行道树是同一族但各是各的调子。 */
  const trunks = trunkField.build('riverbank-trunks', M.bark.color);
  const blossoms = blossomField.build('riverbank-blossoms');
  const leaves = leafField.build('riverbank-leaves');
  const petals = fallenField.build('riverbank-petals');
  petals.position.y = 0.012;
  g.add(trunks, blossoms, leaves, petals);

  /* 路灯：不合批。buildStreetLamp 里有发光件，压进大 mesh 会把光晕焊死。 */
  for (const bk of banks) {
    const lampZ = bk.edge + bk.inward * (WALL_T + 0.55);
    for (const [x0, x1] of detailSegs) {
      for (let x = x0 + 12; x <= x1; x += 34) {
        if (x > TRACK_GAP[0] && x < TRACK_GAP[1]) continue;
        const lamp = buildStreetLamp([x, 0.008, lampZ], 4.6);
        lamp.name = `riverbank-lamp-${bk.id}-${x}`;
        g.add(lamp);
      }
    }
  }

  return g;
}
