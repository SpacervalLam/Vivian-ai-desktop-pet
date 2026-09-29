import * as THREE from 'three';
import { toon } from './toon';

/**
 * 树冠外观的共享实现（樱花 / 绿叶）。
 *
 * 这里解决的是**同一片场地里几种树各长各的**的问题。历史上有三套互不相干的树冠：
 *
 *  - 近景商店街（sakuraTown）：五瓣小花贴图 + alphaTest 面片，细碎通透；
 *  - 社区绿地（urbanStreets 的 bloom 分支）：11 个二十面体球；
 *  - 市政行道树（urbanStreets 的普通分支）：**5 个一模一样的平滑球**，单色 ——
 *    低模场景里最扎眼的"气球"，从任何角度看剪影都是一个圆。
 *
 * 现在贴图与实例收集器统一到这里，枝干走位仍留在各自调用点
 * （行道树瘦高、公园老樱矮壮、绿树更圆更密）。
 */

/** 花簇配色。白色占多数，粉色只作点缀 —— 全粉会糊成一块。 */
export const BLOSSOM_PALETTE = ['#ffffff', '#fff1f5', '#f4c9de'] as const;

/** 叶簇配色。三档明暗交替，避免整棵树读成一个色块。 */
export const LEAF_PALETTE = ['#7d9c62', '#5f7f4d', '#93ae74'] as const;

/**
 * 五瓣小花簇贴图：细碎、边缘通透。
 *
 * 随机数由调用方注入，保证调用方的 PRNG 序列不被打乱（贴图会消耗大量抽样，
 * 若内部自建 PRNG，调用点之后的随机内容会整体错位）。
 */
export function blossomSprayTexture(random: () => number, count = 95): THREE.CanvasTexture {
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 256;
  const c = canvas.getContext('2d')!;
  for (let n = 0; n < count; n++) {
    const a = random() * Math.PI * 2, r = Math.sqrt(random()) * 103;
    const x = 128 + Math.cos(a) * r, y = 128 + Math.sin(a) * r, size = 5 + random() * 8;
    c.fillStyle = ['#fff1f6', '#ffd7e6', '#f2b3cd', '#ffe4ed'][n % 4];
    for (let petal = 0; petal < 5; petal++) {
      const pa = petal * Math.PI * .4;
      c.beginPath();
      c.ellipse(x + Math.cos(pa) * size * .55, y + Math.sin(pa) * size * .55,
        size * .62, size * .40, pa, 0, Math.PI * 2);
      c.fill();
    }
    c.fillStyle = '#d888ab'; c.beginPath(); c.arc(x, y, 1.3, 0, Math.PI * 2); c.fill();
  }
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = 4;
  return texture;
}

/**
 * 叶簇贴图：一把朝向杂乱的小叶 + 主脉。
 *
 * 关键是**每片叶各自旋转、疏密不均**，alphaTest 裁掉缝隙 —— 于是轮廓是碎的、
 * 有空气感的，而不是一个实心圆。叶脉那道深色细线是让近景不糊成色块的关键。
 */
export function leafSprayTexture(random: () => number, count = 300): THREE.CanvasTexture {
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 256;
  const c = canvas.getContext('2d')!;
  for (let n = 0; n < count; n++) {
    const a = random() * Math.PI * 2, r = Math.sqrt(random()) * 110;
    const x = 128 + Math.cos(a) * r, y = 128 + Math.sin(a) * r;
    const len = 6.4 + random() * 7.2, wid = len * (0.34 + random() * 0.16), rot = random() * Math.PI;
    c.fillStyle = ['#7fa063', '#618150', '#96b077', '#6d8f57'][n % 4];
    c.beginPath(); c.ellipse(x, y, len, wid, rot, 0, Math.PI * 2); c.fill();
    c.strokeStyle = 'rgba(48,68,40,.45)'; c.lineWidth = .9;
    c.beginPath();
    c.moveTo(x - Math.cos(rot) * len, y - Math.sin(rot) * len);
    c.lineTo(x + Math.cos(rot) * len, y + Math.sin(rot) * len);
    c.stroke();
  }
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = 4;
  return texture;
}

export interface SprayField {
  /** 在枝头挂一簇。面片基准尺寸 2×2，故 scale=0.5 即约 1m 见方。 */
  spray(x: number, y: number, z: number, scale: number): void;
  /** 所有树登记完后一次性产出；**必须在 mergeByMaterial 之后**再挂进场景。 */
  build(name: string): THREE.InstancedMesh;
  readonly material: THREE.MeshStandardMaterial;
  readonly geometry: THREE.PlaneGeometry;
  readonly texture: THREE.Texture;
  readonly count: number;
}

/**
 * 冠簇实例收集器。
 *
 * 为什么不直接建 InstancedMesh：树的枝干要先 mergeByMaterial 合批，而
 * `mergeByMaterial` 只按 `isMesh` 过滤 —— **InstancedMesh 也是 isMesh**，
 * 混在里面会被当成一块 2×2 的面片烘掉。所以先收集、合批之后再 build。
 */
export function createSprayField(opts: {
  random: () => number; capacity: number; texture: THREE.Texture;
  palette?: readonly string[]; alphaTest?: number;
}): SprayField {
  const { random, capacity, texture } = opts;
  const colors = (opts.palette ?? BLOSSOM_PALETTE).map((c) => new THREE.Color(c));
  const material = new THREE.MeshStandardMaterial({
    map: texture, alphaTest: opts.alphaTest ?? .38, side: THREE.DoubleSide, roughness: 1,
  });
  // 描边默认是 2（见 toon.ts outlineWeightOf）—— 冠簇是 alphaTest 面片，
  // 套一层外扩壳会直接变成实心方块，必须显式归零。
  material.userData.outlineWeight = 0;
  const geometry = new THREE.PlaneGeometry(2, 2);
  const pos: number[] = [], rot: number[] = [], scl: number[] = [], tone: number[] = [];
  const dummy = new THREE.Object3D();
  return {
    material, geometry, texture,
    get count() { return scl.length; },
    spray(x, y, z, scale) {
      if (scl.length >= capacity) return;
      pos.push(x, y, z);
      rot.push(random() * 3, random() * 3, random() * 3);
      scl.push(scale);
      tone.push(scl.length % colors.length);
    },
    build(name) {
      const n = scl.length;
      const mesh = new THREE.InstancedMesh(geometry, material, n);
      mesh.name = name;
      mesh.castShadow = true;
      for (let i = 0; i < n; i++) {
        dummy.position.set(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
        dummy.rotation.set(rot[i * 3], rot[i * 3 + 1], rot[i * 3 + 2]);
        dummy.scale.setScalar(scl[i]);
        dummy.updateMatrix();
        mesh.setMatrixAt(i, dummy.matrix);
        mesh.setColorAt(i, colors[tone[i]]);
      }
      mesh.instanceMatrix.needsUpdate = true;
      if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
      // 合批之后挂进来，但仍要挡两道：实例网格跨整片场地，
      // 遍历式碰撞收集会给街道横上一堵隐形墙。
      mesh.userData.noMerge = true;
      mesh.userData.sceneCollideSkip = true;
      return mesh;
    },
  };
}

/** 花簇场（樱花）。贴图在这里生成 —— 顺序与历史实现一致，不移动调用点的 PRNG 序列。 */
export function createBlossomField(random: () => number, capacity: number,
  palette: readonly string[] = BLOSSOM_PALETTE): SprayField {
  return createSprayField({ random, capacity, texture: blossomSprayTexture(random), palette });
}

/** 叶簇场（绿树）。 */
export function createLeafField(random: () => number, capacity: number,
  palette: readonly string[] = LEAF_PALETTE): SprayField {
  return createSprayField({ random, capacity, texture: leafSprayTexture(random), palette });
}

/* ============================================================================
 * 树干
 *
 * 树干曾经是"一根缩放的圆柱"（`cityDressing` 里甚至是一根缩放的**立方体**），
 * 读起来就是一根管子：没有根部外扩、没有树皮起伏、没有弯。几何现在由 Blender
 * 出（`scripts/trees/build_trunks.py` → `public/room/models/trees/*.glb`），三个
 * "像树"的线索都烘在网格里：板根（**离散的根鳍**，不是均匀的喇叭口 —— 均匀喇叭口
 * 读作花瓶，第一版就是这么翻车的）、树皮棱沟（|sin| 截面，沟底是尖的）、微弯的轴线。
 *
 * 这里管的是**铺实例**，不是造几何：
 *   - `add()` 在 plantTree 里逐棵登记；`build()` 在 mergeByMaterial **之后**调用，
 *     每个变体产出 1 个 InstancedMesh（全场干只占 1~2 次 draw call）。
 *   - GLB 到位前用程序化回退（`unitTrunkGeometry`：带锥度 + 根部外扩的旋转体），
 *     到位后 `install()` 换几何 —— 同一变体的实例共用一份几何，换一次就全变。
 *
 * **材质不来自 GLB**：场景是 toon 渲染，GLB 的 MeshStandardMaterial 会格格不入。
 * 取几何 + UV，材质换成 `toon(色, {map: trunkBarkTexture()})`，于是描边、色阶、
 * 以及各模块**自己的树皮色**都保住了，而树皮细节只有一份。
 *
 * 登记**不消耗 `kit.random()`**：`plantTree` 的随机序列被调用点依赖（见它的注释），
 * 干的位置/朝向一律从 `seed` 推导，免得挪一根干就把后面的冠簇整体错位。
 * ========================================================================== */

export type TrunkVariant = 'sakura' | 'green';

/** 一棵树的干。坐标是树脚；干顶 = 树脚 + (lean·cos(leanDir), h, lean·sin(leanDir))。 */
export interface TrunkSpec {
  x: number;
  z: number;
  /** 干底高度。下方那段"裙摆"埋进树穴里，所以它略低于穴面。 */
  y0: number;
  /** 干高。 */
  h: number;
  /** 基部半径。 */
  r: number;
  /** 干顶相对树脚的水平偏移量。 */
  lean: number;
  /** 偏移方向（弧度）。 */
  leanDir: number;
  variant: TrunkVariant;
  /** 决定朝向与个体差异的确定性种子（**不要**用 kit.random()）。 */
  seed: number;
}

/**
 * 树皮贴图：**灰度**，由各模块自己的树皮色相乘上色。
 *
 * 一份贴图服务河堤（#62594b）、街区（#7a5c45）、商店街（darkWood）等所有配色，
 * 细节只有一份。值域压在 0.6~1.0：`map` 与材质色**相乘**，超过 1 提不亮，
 * 低于 0.6 就把彩色树皮压成黑条。
 *
 * 竖纹必须**随高度摆动**，否则远看是一排等距直线，比没有贴图还假。
 */
let _barkTexture: THREE.CanvasTexture | null = null;
export function trunkBarkTexture(): THREE.CanvasTexture {
  if (_barkTexture) return _barkTexture;
  const canvas = document.createElement('canvas');
  canvas.width = 256; canvas.height = 512;
  const c = canvas.getContext('2d')!;
  let s = 24601;
  const rnd = () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 4294967296; };
  c.fillStyle = '#e4e0da'; c.fillRect(0, 0, 256, 512);
  // 竖纹：一条沟 = 一道暗线 + 紧挨着的一道亮线（棱脊受光）
  const ridges = 26;
  for (let i = 0; i < ridges; i++) {
    const x0 = (i + 0.5) * 256 / ridges, wob = 1.5 + rnd() * 3.5, phase = rnd() * 6.28;
    const dark = 0.26 + rnd() * 0.22, w = 1.6 + rnd() * 4.4;
    const lift = 0.10 + rnd() * 0.16;
    const strokes: Array<[number, string, number]> = [
      [0, `rgba(38,30,24,${dark})`, w],
      [w * 0.9, `rgba(255,252,246,${lift})`, w * 0.7],
    ];
    for (const [dx, style, lw] of strokes) {
      c.strokeStyle = style; c.lineWidth = lw; c.beginPath();
      for (let y = -8; y <= 520; y += 8) {
        const x = x0 + dx + Math.sin(y * 0.021 + phase) * wob + Math.sin(y * 0.071 + phase * 2) * wob * 0.4;
        if (y <= -8) c.moveTo(x, y); else c.lineTo(x, y);
      }
      c.stroke();
    }
  }
  // 横向皮孔/裂缝：短促、随机，打断竖纹的"条形码"感
  for (let i = 0; i < 150; i++) {
    const y = rnd() * 512, x = rnd() * 256, w = 5 + rnd() * 20;
    c.strokeStyle = `rgba(32,25,20,${0.16 + rnd() * 0.22})`; c.lineWidth = 1 + rnd() * 1.6;
    c.beginPath(); c.moveTo(x, y); c.lineTo(x + w, y + (rnd() - 0.5) * 3); c.stroke();
  }
  // 细颗粒：让平涂的 toon 面不至于像塑料
  for (let i = 0; i < 2600; i++) {
    const v = rnd();
    c.fillStyle = v > 0.5 ? `rgba(255,250,240,${(v - 0.5) * 0.30})` : `rgba(30,22,16,${(0.5 - v) * 0.34})`;
    c.fillRect(rnd() * 256, rnd() * 512, 1.4, 1.4);
  }
  const tex = new THREE.CanvasTexture(canvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
  tex.anisotropy = 4;
  _barkTexture = tex;
  return tex;
}

/**
 * 程序化回退：**单位尺度**（高 1、基部半径 1）的旋转体。
 *
 * 关键是它和 GLB 用同一套缩放约定 —— 一律 `scale(r, len, r)` —— 于是 GLB 到位
 * 前后形状一致，只是细节变多。锥度用 t**1.5 而不是线性，否则读作一个圆锥。
 */
let _unitTrunk: THREE.LatheGeometry | null = null;
function unitTrunkGeometry(): THREE.LatheGeometry {
  if (_unitTrunk) return _unitTrunk;
  const pts: THREE.Vector2[] = [];
  const N = 16;
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    const flare = 1 + 0.55 * Math.max(0, 1 - t / 0.22) ** 2.2;
    const prof = 1 - 0.42 * t ** 1.5;
    pts.push(new THREE.Vector2(Math.max(0.02, prof * flare), t));
  }
  _unitTrunk = new THREE.LatheGeometry(pts, 14);
  return _unitTrunk;
}

export interface TrunkField {
  add(spec: TrunkSpec): void;
  /**
   * 产出实例网格。**必须在 mergeByMaterial 之后**再挂进场景（同冠簇场：
   * mergeByMaterial 只按 isMesh 过滤，InstancedMesh 也是 isMesh，混进去会被
   * 当成一块几何烘掉）。返回一个 Group，每个变体一个 InstancedMesh。
   */
  build(name: string, color: THREE.ColorRepresentation): THREE.Group;
  /** GLB 到位后换几何。同一变体的所有实例共用一份几何，换一次就全变。 */
  install(variant: TrunkVariant, geometry: THREE.BufferGeometry): void;
  /** 这一场里有没有用到某个变体（用来决定要不要去取那个 GLB）。 */
  wants(variant: TrunkVariant): boolean;
  readonly count: number;
}

/** 所有已创建的干场。`blenderTrunks` 靠它把 GLB 派发下去。 */
export const trunkFields: TrunkField[] = [];

const TRUNK_UP = new THREE.Vector3(0, 1, 0);
function trunkHash(seed: number, salt: number): number {
  let h = (Math.imul((seed + 1) | 0, 2654435761) + Math.imul(salt | 0, 40503)) >>> 0;
  h = Math.imul(h ^ (h >>> 15), 2246822519) >>> 0;
  return ((h ^ (h >>> 13)) >>> 0) / 4294967296;
}

export function createTrunkField(): TrunkField {
  const specs: TrunkSpec[] = [];
  const meshes = new Map<TrunkVariant, THREE.InstancedMesh>();
  // 记下已经到位的 GLB 几何：万一张贴快过建场，build() 也能直接取到。
  const arrived = new Map<TrunkVariant, THREE.BufferGeometry>();
  const field: TrunkField = {
    get count() { return specs.length; },
    wants: (variant) => specs.some((s) => s.variant === variant),
    add(spec) { specs.push(spec); },
    build(name, color) {
      const group = new THREE.Group();
      group.name = name;
      const material = toon(color, { map: trunkBarkTexture() });
      for (const variant of ['sakura', 'green'] as const) {
        const list = specs.filter((s) => s.variant === variant);
        if (!list.length) continue;
        // **每个场一份几何克隆**：`sakuraTown` / `cityDressing` 的 dispose 会遍历
        // root 把所有 mesh 几何收走释放。若各场共用同一份 GLB 几何，先卸载的那个
        // 模块就会把还活着的场一起打空（merge.ts 的引用计数坑是同一类问题）。
        const source: THREE.BufferGeometry = (arrived.get(variant) ?? unitTrunkGeometry()).clone();
        const mesh: THREE.InstancedMesh = new THREE.InstancedMesh(source, material, list.length);
        mesh.name = `${name}/${variant}`;
        mesh.castShadow = true;
        const dummy = new THREE.Object3D();
        const q = new THREE.Quaternion();
        list.forEach((spec, i) => {
          // 干顶方向 = 树脚 → 树脚 + (lean·cos, h, lean·sin)
          const dir = new THREE.Vector3(spec.lean * Math.cos(spec.leanDir), spec.h,
            spec.lean * Math.sin(spec.leanDir));
          const len = dir.length();
          // 先绕自身轴转（定树皮棱沟的相位），再整体倾斜
          q.setFromUnitVectors(TRUNK_UP, dir.normalize());
          dummy.quaternion.copy(q).multiply(
            new THREE.Quaternion().setFromAxisAngle(TRUNK_UP, trunkHash(spec.seed, 7) * 6.283));
          dummy.position.set(spec.x, spec.y0, spec.z);
          // 个体差异：高 ±8%、粗 ±6%。干顶因此浮动 ~0.2m，但枝条着生点在 1.6~1.9m、
          // 离干顶还有半米，甩不出去。
          const hs = 0.92 + trunkHash(spec.seed, 11) * 0.16;
          const rs = 0.94 + trunkHash(spec.seed, 23) * 0.12;
          dummy.scale.set(spec.r * rs, len * hs, spec.r * rs);
          dummy.updateMatrix();
          mesh.setMatrixAt(i, dummy.matrix);
        });
        mesh.instanceMatrix.needsUpdate = true;
        // 同冠簇场：实例网格跨整片场地，遍历式碰撞收集会给街道横上一堵隐形墙。
        // 树干本来就逐棵登记了碰撞盒（sakura-tree-trunk / street-tree / dress-tree）。
        mesh.userData.noMerge = true;
        mesh.userData.sceneCollideSkip = true;
        // 核账用：几何到底换没换成 Blender 资产，从外面只看顶点数猜不出来。
        // （和 blenderVehicles 的 `blenderVehicleLoaded` 是同一套路。）
        mesh.userData.trunkAsset = false;
        mesh.userData.trunkVariant = variant;
        mesh.userData.trunkCount = list.length;
        meshes.set(variant, mesh);
        group.add(mesh);
      }
      return group;
    },
    install(variant, geometry) {
      arrived.set(variant, geometry);
      const mesh = meshes.get(variant);
      if (!mesh) return;
      mesh.geometry.dispose();
      mesh.geometry = geometry.clone();
      mesh.userData.trunkAsset = true;
    },
  };
  trunkFields.push(field);
  return field;
}

/**
 * 种一棵树要用的"画笔"。
 *
 * 枝条和树穴的落位**留在调用点**（行道树瘦高、公园老樱矮壮、河堤的行道樱又略
 * 不同），所以这里只注入构造手段，不接管几何。
 */
export interface TreeKit {
  /** 一段枝条（起点、终点、半径）。 */
  rod(a: number[], b: number[], r: number): void;
  /** 树穴：脚下那块方形的土/草。 */
  pit(x: number, z: number): void;
  /**
   * 树干场。**干不走 `rod`** —— 干是 Blender 资产，走实例网格（见上面「树干」那段）。
   * 必填而不是可选：漏掉一处就会在原地留下一根光秃秃的圆柱，而那种退化在
   * 满屏树冠里根本看不出来。
   */
  trunk: TrunkField;
  random(): number;
  /** 花簇场（樱花）。 */
  blossom: SprayField;
  /** 叶簇场（绿树）。 */
  leaf: SprayField;
  /** 落瓣场。不传就只种树、不铺落花。 */
  fallen?: { scatter(x: number, z: number, scale: number, radius: number, count: number): void };
}

/**
 * 一棵完整的树：干 + 分叉 + 两级冠簇（枝头 + 枝条中段）。
 *
 * 从 `urbanStreets` 里的私有 `tree()` 提出来，变成全场景共用的一处实现。动机是
 * 同一个老毛病又冒了一次：给河堤种树时，如果就地再写一棵"干 + 几个压扁的球"，
 * 河边立刻会出现第三种树 —— 而 `foliage.ts` 这个模块存在的理由，正是消灭
 * "同一片场地里几种树各长各的"（见文件头）。
 *
 * `density` 缩放**灌进冠簇的簇数**（不动枝干）。近景街道树用 1，远处成排的
 * 背景树可以降到 0.5 左右：冠幅不变、只是变透，比减少棵数自然。
 *
 * ⚠ 这个函数会按固定顺序消耗 `kit.random()`。调用点一旦挪动它的位置，之后的
 * 随机内容会整体错位 —— 这正是它保持"逐字照搬原实现"的原因。
 */
export function plantTree(
  kit: TreeKit, x: number, z: number, bloom = false, treeIndex = 0, density = 1,
): void {
  const { rod, random, blossom, leaf, trunk } = kit;
  const n = (base: number) => Math.max(1, Math.round(base * density));
  kit.pit(x, z);
  if (bloom) {
    // 枝干比行道树矮壮、枝展更大，读作"公园里的老樱"。
    // 干不再是一根圆柱：几何由 Blender 出（见本文件「树干」那段）。原来那根
    // `rod([x,.2,z],[x+.14,2.62,z+.05],.15)` 的**起讫点与半径原样搬过来** ——
    // h 取 Δy、lean/leanDir 取水平偏移，于是树形、枝条着生点、碰撞盒一个都没动。
    trunk.add({
      x, z, y0: .2, h: 2.42, r: .15, variant: 'sakura',
      lean: Math.hypot(.14, .05), leanDir: Math.atan2(.05, .14), seed: treeIndex,
    });
    for (let i = 0; i < 7; i++) {
      const a = i * 2.399 + treeIndex * .8, reach = 1.35 + random() * .55;
      const joint = [x + Math.cos(a) * .72, 2.78 + random() * .42, z + Math.sin(a) * .72];
      const tip = [x + Math.cos(a) * reach, 4.18 + random() * 1.02, z + Math.sin(a) * reach];
      rod([x + .14, 1.7 + i * .12, z + .05], joint, .075); rod(joint, tip, .038);
      for (let twig = 0; twig < 2; twig++) rod(tip, [tip[0] + Math.sin(twig * 2 + a) * .68, tip[1] + .44, tip[2] + Math.cos(twig * 2 + a) * .68], .014);
      for (let k = 0; k < n(112); k++) {
        const theta = random() * Math.PI * 2, r = Math.sqrt(random()) * 1.02;
        blossom.spray(tip[0] + Math.cos(theta) * r, tip[1] + (random() - .5) * .66, tip[2] + Math.sin(theta) * r, .22 + random() * .24);
      }
      // 只在枝头挂花会留出一圈空档：树冠读起来像个"环"。沿枝条中段补一层，
      // 把内圈填满，近看才是完整的一团而不是甜甜圈。
      const mid = [(joint[0] + tip[0]) / 2, (joint[1] + tip[1]) / 2, (joint[2] + tip[2]) / 2];
      for (let k = 0; k < n(34); k++) {
        const theta = random() * Math.PI * 2, r = Math.sqrt(random()) * .86;
        blossom.spray(mid[0] + Math.cos(theta) * r, mid[1] + (random() - .5) * .72, mid[2] + Math.sin(theta) * r, .20 + random() * .20);
      }
    }
    kit.fallen?.scatter(x, z, .032, 3.4, n(190));
  } else {
    // 绿树：同样是"干 + 分叉 + 冠簇"，但比老樱紧凑、比行道樱圆。
    // treeIndex 一定要传 —— 旧版 16 棵全部走默认 0，于是**棵棵完全同形**，
    // 连起来看就是一排复制粘贴的球。
    // 密度对齐老樱：冠幅差不多时，簇数少了就"透光"，远看像一把稀疏的扫帚。
    // 同上：`rod([x,.2,z],[x+.1,2.35,z+.03],.13)` 的起讫点原样搬到干场。
    trunk.add({
      x, z, y0: .2, h: 2.15, r: .13, variant: 'green',
      lean: Math.hypot(.1, .03), leanDir: Math.atan2(.03, .1), seed: treeIndex,
    });
    for (let i = 0; i < 5; i++) {
      const a = i * 1.7 + treeIndex * .9, reach = 1.15 + random() * .55;
      const joint = [x + Math.cos(a) * .55, 2.45 + random() * .30, z + Math.sin(a) * .55];
      const tip = [x + Math.cos(a) * reach, 3.35 + random() * .75, z + Math.sin(a) * reach];
      rod([x + .1, 1.9 + i * .10, z + .03], joint, .07); rod(joint, tip, .034);
      for (let twig = 0; twig < 2; twig++) rod(tip, [tip[0] + Math.sin(twig * 2 + a) * .55, tip[1] + .36, tip[2] + Math.cos(twig * 2 + a) * .55], .013);
      for (let k = 0; k < n(130); k++) {
        const theta = random() * Math.PI * 2, r = Math.sqrt(random()) * .92;
        leaf.spray(tip[0] + Math.cos(theta) * r, tip[1] + (random() - .5) * .62, tip[2] + Math.sin(theta) * r, .21 + random() * .24);
      }
      const mid = [(joint[0] + tip[0]) / 2, (joint[1] + tip[1]) / 2, (joint[2] + tip[2]) / 2];
      for (let k = 0; k < n(55); k++) {
        const theta = random() * Math.PI * 2, r = Math.sqrt(random()) * .78;
        leaf.spray(mid[0] + Math.cos(theta) * r, mid[1] + (random() - .5) * .66, mid[2] + Math.sin(theta) * r, .19 + random() * .19);
      }
    }
  }
}

export interface PetalField {
  /** 在 (x,z) 周围半径 radius 内铺 count 片落花，y 为铺设高度。 */
  scatter(x: number, z: number, y: number, radius: number, count: number): void;
  build(name: string): THREE.InstancedMesh;
  readonly material: THREE.MeshStandardMaterial;
  readonly geometry: THREE.ShapeGeometry;
}

/** 落花。与树上花簇同一张花瓣形状，铺在地面形成"花筏"。 */
export function createFallenPetals(random: () => number, capacity: number,
  color = '#f5c1d5'): PetalField {
  const shape = new THREE.Shape();
  shape.moveTo(0, -.5);
  shape.quadraticCurveTo(.62, -.04, .16, .46);
  shape.lineTo(0, .33);
  shape.lineTo(-.16, .46);
  shape.quadraticCurveTo(-.62, -.04, 0, -.5);
  const geometry = new THREE.ShapeGeometry(shape);
  const material = new THREE.MeshStandardMaterial({ color, side: THREE.DoubleSide, roughness: 1 });
  material.userData.outlineWeight = 0;
  const matrices: THREE.Matrix4[] = [];
  const dummy = new THREE.Object3D();
  return {
    material, geometry,
    scatter(x, z, y, radius, count) {
      for (let i = 0; i < count && matrices.length < capacity; i++) {
        const a = random() * Math.PI * 2, r = Math.sqrt(random()) * radius;
        dummy.position.set(x + Math.cos(a) * r, y, z + Math.sin(a) * r);
        dummy.rotation.set(-Math.PI / 2, 0, random() * 6.28);
        dummy.scale.setScalar(.035 + random() * .075);
        dummy.updateMatrix();
        matrices.push(dummy.matrix.clone());
      }
    },
    build(name) {
      const mesh = new THREE.InstancedMesh(geometry, material, matrices.length);
      mesh.name = name;
      matrices.forEach((m, i) => mesh.setMatrixAt(i, m));
      mesh.instanceMatrix.needsUpdate = true;
      mesh.userData.noMerge = true;
      mesh.userData.sceneCollideSkip = true;
      return mesh;
    },
  };
}
