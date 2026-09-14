/**
 * 室外世界层。
 *
 * 房间不再是摆在展示底座上的微缩模型，而是站在雨夜街区里——
 * 窗外看到的是真实存在的 3D 世界，不是贴在窗口的一张画。
 *
 * 分层（B+ 架构，按到房间的距离分）：
 *   世界地面  一整张湿沥青，所有层的承载者（本文件 Stage 1）
 *   近景街道  真 3D：对面楼 / 路灯 / 湿地反光（Stage 2）
 *   公寓本体  203 所在的这栋四层集合住宅，本文件最大的一块（Stage 2）
 *             单侧开放式外廊 + 东端共用外置折返楼梯的公共交通系统
 *   便利店    街对过的近景主体（Stage 2b）
 *   中景环带  低模楼群，吃雾（Stage 2）
 *   天空      由 postfx.background 纯色兜底
 *
 * 美术方向（本次整体重做的依据）：
 *   冷蓝灰环境 + 暖黄窗光。建筑一律低饱和冷灰：浅冷灰 / 蓝灰 / 灰褐 /
 *   石墨灰 / 深灰蓝，**不使用大面积米白、象牙白或暖米白**。
 *   墙体是有厚度的混凝土，窗是"墙上的洞"（有窗洞深度、窗框、玻璃、窗台、
 *   中梃、窗帘），不是贴在墙面上的一片色块；阳台是真正挑出墙外的结构
 *   （板 + 戸境板 + 栏杆竖栅 + 排水口 + 竖雨水管），不是画在墙上的线。
 *   树是有层次的简化枝叶，不是圆球低模。
 *
 * 性能约定：整层合批 + 冻结；小件共享几何实例（见 gbox）；
 * 除窗玻璃外不新增透明材质。
 */

import * as THREE from 'three';
import {
  toon, emissive, makeRng, makeCanvas, toTexture,
  asphaltTexture, sidewalkTexture, plazaStoneTexture, drainGrateTexture, puddlePatchTexture,
  curbTexture, manholeTexture, nightHorizonTexture,
  rainGlassTexture, curtainTexture,
} from './toon';
import { scaleUV, outlineProp, buildStreetLamp, buildWetGround, buildPuddleRipples, buildEaveDrips, type PoolSpec, type DripEdge } from './props';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import type { BoxColliderSpec } from './collider';

/* ============================================================================
 * 色板：冷灰蓝城市住宅
 * ========================================================================== */

/**
 * 整栋楼不许出现大面积米白 / 象牙白 / 暖黄。
 *
 * 墙体读作"被夜雨打湿的混凝土"：冷灰、蓝灰、灰褐、石墨灰、深灰蓝，
 * 全部低饱和，明度压在中低区——暖黄窗光、入口照明、便利店、路灯才是
 * 画面里唯一的高光。颜色之间靠明度分层，不靠色相（色相一多就花）。
 *
 * 七层以上不同但协调的材质：墙体 / 阳台板 / 栏杆 / 窗框 / 玻璃 / 木材 /
 * 金属 / 植物 / 织物。同层之内也刻意不共用一色（主墙与山墙差一档）。
 */
const EXT = {
  /* —— 墙 —— */
  wall: '#98a3ae',      // 主墙：浅冷灰（不白、不发黄、不高亮）
  wallAlt: '#8b96a5',   // 山墙 / 北面 / 楼梯间：蓝灰，比主墙深一档
  wallDeep: '#5e6878',  // 深灰蓝：入口凹龛 / 楼梯间竖向体量 / 底层腰线
  wallBase: '#6b7481',  // 底层基座：石墨灰，接地带潮气
  /* —— 混凝土二次构件 —— */
  slab: '#a9b2bb',      // 阳台板 / 檐口 / 走廊板：比墙浅一档的冷灰
  slabDark: '#8b95a0',  // 戸境板 / 阳台侧板 / 窗台
  /* —— 阳台 —— */
  railing: '#3f4650',   // 栏杆竖栅：炭灰
  railTop: '#4b525c',   // 扶手 / 横档：略浅的炭灰
  /* —— 窗 —— */
  frame: '#49525f',     // 窗框：深灰蓝
  glass: '#31405a',     // 玻璃：冷蓝（夜空反光的颜色）
  unlit: '#262f3e',     // 熄灯窗：比玻璃再暗一档，读得出"没人"
  /* —— 木材 —— */
  wood: '#a98a63',      // 浅橡木：阳台墙板 / 户门 / 长凳
  woodDeep: '#7f6547',  // 中木色：入口门 / 便利店木作
  /* —— 五金 —— */
  metal: '#4c535c',     // 室外机 / 表箱 / 支架
  pipe: '#59616b',      // 雨水管 / 空调配管
  dark: '#2b3340',      // 摄像机 / 门牌底 / 排水口
  /* —— 植物 —— */
  plant: '#4d6a52',     // 主叶色（低饱和、不发荧光）
  plantDeep: '#3a5442', // 背光叶 / 绿篱下部
  plantWarm: '#5c6f4a', // 少量偏黄绿的品种
  trunk: '#574e45',     // 树干：灰褐，不是暖橙
  /* —— 织物（晾晒衣物 / 窗帘）低饱和 —— */
  cloth: ['#c6ccd3', '#a7b4c1', '#b2aba0', '#8d98a6', '#b9a9a2'],
};

/**
 * 几何缓存：外景有大量同尺寸的小件（栏杆竖栅、窗框料、空调外机）。
 * 共享同一个 BufferGeometry 实例有两个好处：
 *  1. 内存：几百根竖栅只占一份顶点
 *  2. 描边：smoothNormalGeometry 按几何实例缓存（WeakMap），共享实例意味着
 *     这几百根只做一次"合并顶点 + 平均法线"的预处理，否则外景构建会慢几倍
 * 合批侧对共享几何有引用计数（见 merge.ts），不会误 dispose。
 */
const geoCache = new Map<string, THREE.BoxGeometry>();
function gbox(w: number, h: number, d: number): THREE.BoxGeometry {
  const k = `${w.toFixed(4)}|${h.toFixed(4)}|${d.toFixed(4)}`;
  let g = geoCache.get(k);
  if (!g) {
    g = new THREE.BoxGeometry(w, h, d);
    geoCache.set(k, g);
  }
  return g;
}

/**
 * 世界地面：一整张湿沥青，铺在房间地板（y=0）之下 12mm。
 *
 * 尺寸按雾远平面反推：地面边缘必须远到被雾完全吞掉（fog far=70），
 * 否则观察者拉到最远时会看到"世界的边缘"。±85m 之外的事归远景幕布管。
 *
 * UV 承载贴图尺度（repeat 恒 1 的思路和室内地板一致）：pavementTexture
 * 自身 repeat 是 [3,3]，这里把 UV 放大 11 倍 → 每张 512px 画布代表约 5m，
 * 碎石颗粒在窗口高度看密度刚好。
 */
// 地铁出入口下沉竖井的开孔（世界坐标 x,z）。buildExteriorGround 在此给世界地面挖洞，
// buildSubwayEntrance 的竖井四壁 / 底板严格与此对齐，否则地面会把井口封死、
// 看不见"挖入地下"往下走的楼梯。
const PIT_X0 = -25.0, PIT_X1 = -20.45;   // 竖井左右壁（含扶梯 + 楼梯）
const PIT_Z0 = 13.8, PIT_Z1 = 18.9;      // 竖井口（街沿侧，开口朝 -z）→ 井底后墙
const PIT_D = 2.6;                        // 站台层深度（地面以下，米）

export function buildExteriorGround(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'exterior-ground';

  const SIZE = 170;
  const HX = SIZE / 2;
  // 世界地面：一张大平面，但在地铁竖井口处挖洞——否则这层均匀沥青会把井口封死。
  const shape = new THREE.Shape();
  shape.moveTo(-HX, -HX);
  shape.lineTo(HX, -HX);
  shape.lineTo(HX, HX);
  shape.lineTo(-HX, HX);
  shape.lineTo(-HX, -HX);
  const hole = new THREE.Path();
  // shape 局部 (a,b) 经 rotateX(-90°) 映射到世界 (a, 0, -b)，故 world z = -b
  hole.moveTo(PIT_X0, -PIT_Z0);
  hole.lineTo(PIT_X1, -PIT_Z0);
  hole.lineTo(PIT_X1, -PIT_Z1);
  hole.lineTo(PIT_X0, -PIT_Z1);
  hole.lineTo(PIT_X0, -PIT_Z0);
  shape.holes.push(hole);
  const geo = new THREE.ShapeGeometry(shape);
  geo.rotateX(-Math.PI / 2);
  // UV 复刻 PlaneGeometry + scaleUV(11,11)：uv = (worldX+HX)/SIZE*11, (-worldZ+HX)/SIZE*11
  {
    const pos = geo.attributes.position;
    const uv = new Float32Array(pos.count * 2);
    for (let i = 0; i < pos.count; i++) {
      uv[i * 2] = (pos.getX(i) + HX) * 11 / SIZE;
      uv[i * 2 + 1] = (-pos.getZ(i) + HX) * 11 / SIZE;
    }
    geo.setAttribute('uv', new THREE.BufferAttribute(uv, 2));
  }

  const mat = toon('#ffffff', { map: asphaltTexture(), finish: 'wet' });
  const ground = new THREE.Mesh(geo, mat);
  ground.name = 'world-ground';
  ground.position.y = -0.012;
  ground.castShadow = false;
  ground.receiveShadow = true;
  g.add(ground);

  return g;
}

/* ============================================================================
 * 通用零件
 * ========================================================================== */

/**
 * 一棵行道树。
 *
 * 刻意不用"一个球 + 一根圆柱"：圆球低模是低模游戏场景最扎眼的特征，
 * 从任何角度看剪影都是一个圆。这里改成"干 + 3~5 团非等比压扁的低面冠簇"，
 * 冠簇之间互相错开、深浅两色交替——剪影是有起伏的一团，转起来能看到
 * 前后层次，远看仍是一棵安静的行道树。
 */
function buildTree(
  x: number,
  z: number,
  height: number,
  seed: number,
  mats: { trunk: THREE.Material; leaf: THREE.Material; leafDeep: THREE.Material; leafWarm: THREE.Material }
): THREE.Group {
  const g = new THREE.Group();
  const rnd = makeRng(seed);
  const trunkH = height * 0.52;

  const trunk = new THREE.Mesh(
    new THREE.CylinderGeometry(height * 0.028, height * 0.05, trunkH, 7),
    mats.trunk
  );
  trunk.position.set(0, trunkH / 2, 0);
  g.add(trunk);

  // 两根主枝：冠簇挂上去之后，枝是"冠从干上分出来"的唯一交代
  for (const [dx, dz, len] of [[-1, 0.35, 0.34], [0.9, -0.4, 0.28]] as Array<[number, number, number]>) {
    const br = new THREE.Mesh(new THREE.CylinderGeometry(height * 0.014, height * 0.022, len * height, 5), mats.trunk);
    br.position.set(dx * len * height * 0.32, trunkH * (0.86 + rnd() * 0.1), dz * len * height * 0.32);
    br.rotation.z = -dx * 0.75;
    br.rotation.x = dz * 0.75;
    g.add(br);
  }

  // 冠簇：低面二十面体，各自非等比压扁并错开，形成"一片片叶丛"
  const clusters: Array<[number, number, number, number, number]> = [
    [0, trunkH + height * 0.20, 0, height * 0.30, 0],
    [-height * 0.17, trunkH + height * 0.13, height * 0.05, height * 0.21, 1],
    [height * 0.16, trunkH + height * 0.15, -height * 0.04, height * 0.22, 2],
    [height * 0.02, trunkH + height * 0.31, -height * 0.09, height * 0.17, 0],
    [-height * 0.05, trunkH + height * 0.09, -height * 0.14, height * 0.15, 1],
  ];
  for (const [cx, cy, cz, r, tone] of clusters) {
    const geo = new THREE.IcosahedronGeometry(r, 0);
    // 每团自己随机压扁一点：正球一看就是"摆上去的球"
    geo.scale(1 + (rnd() - 0.5) * 0.22, 0.66 + rnd() * 0.24, 1 + (rnd() - 0.5) * 0.22);
    const leaf = new THREE.Mesh(geo, tone === 0 ? mats.leaf : tone === 1 ? mats.leafDeep : mats.leafWarm);
    leaf.position.set(cx, cy, cz);
    leaf.rotation.y = rnd() * Math.PI;
    g.add(leaf);
  }

  g.position.set(x, 0, z);
  return g;
}

/**
 * 整形绿篱 / 灌木。
 *
 * 一样不用球：一颗植株 = 一个压扁的低面块 + 顶上两三个小凸起，
 * 连成一排读作"修剪过的灌木带"，孤植读作球状灌木。
 */
function buildShrub(
  x: number,
  z: number,
  w: number,
  h: number,
  d: number,
  seed: number,
  mats: { leaf: THREE.Material; leafDeep: THREE.Material }
): THREE.Group {
  const g = new THREE.Group();
  const rnd = makeRng(seed);

  const base = new THREE.Mesh(new THREE.IcosahedronGeometry(1, 0), mats.leaf);
  base.scale.set(w / 2, h / 2, d / 2);
  base.position.y = h / 2;
  base.rotation.y = rnd() * Math.PI;
  g.add(base);

  // 顶上的凸起：让上缘不是一条规整的椭球弧
  const n = 2 + Math.floor(rnd() * 2);
  for (let i = 0; i < n; i++) {
    const bump = new THREE.Mesh(new THREE.IcosahedronGeometry(1, 0), mats.leafDeep);
    const r = Math.min(w, d) * (0.24 + rnd() * 0.16);
    bump.scale.set(r * 1.3, r * 0.8, r * 1.3);
    bump.position.set((rnd() - 0.5) * w * 0.55, h * (0.78 + rnd() * 0.22), (rnd() - 0.5) * d * 0.55);
    bump.rotation.y = rnd() * Math.PI;
    g.add(bump);
  }

  g.position.set(x, 0, z);
  return g;
}

/* ============================================================================
 * Stage 2 — 近景街道层（环带楼群 + 路灯 + 湿地）
 * ========================================================================== */

/**
 * 布点依据（世界坐标，房间 x -6.2..6.2 / z -5.9..6.3，阳台在南 z4.7..6.3）：
 * 楼群在 25~45m 环带上——它们是城市背景剪影，不是邻居。近景是公寓楼
 * 本体（buildApartmentShell）和街角便利店（Stage 2b）。环带楼高 10~21m，
 * 吃 25%~45% 的雾（fog near14/far70），正好是"隔着雨看城市"的褪色感。
 * 窗灯只铺朝房间的那一面，其余三面画了也永远看不到。
 *
 * face 是"朝房间立面"的法线方向：楼在房间的哪一侧，face 就指向反方向。
 */
type BuildingSpec = {
  /** 底面中心 [x, z]（世界坐标） */
  center: [number, number];
  /** [宽 w, 高 h, 深 d]，高从地面算起 */
  size: [number, number, number];
  color: string;
  face: 'north' | 'south' | 'east' | 'west';
  /** 亮灯窗比例，0..1 */
  lit: number;
};

/**
 * 环带楼群全部压在冷蓝灰区间内：越远越浅越蓝（空气透视），
 * 近的一档偏石墨灰。它们唯一的职责是给窗外的暖光一个冷色背景。
 */
const BUILDINGS: BuildingSpec[] = [
  // 南 —— 远背景剪影（z≥45，退到中景楼后方做纯背景）
  { center: [-14, 48], size: [9, 18, 7], color: '#26303f', face: 'north', lit: 0.34 },
  { center: [6, 53], size: [10, 22, 8], color: '#212a3a', face: 'north', lit: 0.3 },
  { center: [27, 47], size: [8, 15, 7], color: '#2a3346', face: 'north', lit: 0.36 },  // 东移让开中景食堂（18,38）
  // 东
  { center: [42, -6], size: [6, 15, 7], color: '#242d40', face: 'west', lit: 0.3 },   // 东移 2m，给共用外楼梯让位
  { center: [43, 6], size: [5, 11, 6], color: '#28324a', face: 'west', lit: 0.26 },
  // 西
  { center: [-40, -4], size: [6, 14, 7], color: '#222b3e', face: 'east', lit: 0.34 },
  { center: [-43, 7], size: [5, 10, 6], color: '#273045', face: 'east', lit: 0.26 },
  // 北
  { center: [-4, -28], size: [8, 17, 6], color: '#252e42', face: 'south', lit: 0.38 },
  { center: [7, -30], size: [7, 12, 6], color: '#202938', face: 'south', lit: 0.3 },
];

/** 楼体在哪个轴上面对房间、立面宽度取 w 还是 d、立面外偏距离。 */
function faceFrame(b: BuildingSpec): { axis: 'x' | 'z'; sign: 1 | -1; width: number; offset: number } {
  const [w, , d] = b.size;
  switch (b.face) {
    case 'north': return { axis: 'z', sign: -1, width: w, offset: d / 2 }; // 面朝 -z
    case 'south': return { axis: 'z', sign: 1, width: w, offset: d / 2 };  // 面朝 +z
    case 'east':  return { axis: 'x', sign: 1, width: d, offset: w / 2 };  // 面朝 +x
    case 'west':  return { axis: 'x', sign: -1, width: d, offset: w / 2 }; // 面朝 -x
  }
}

/** 窗灯平面共享一份几何（合批时会各自 clone，这里省的是创建成本）。 */
const WIN_GEO = new THREE.PlaneGeometry(0.55, 0.8);

function buildBuilding(b: BuildingSpec, rnd: () => number): THREE.Group {
  const g = new THREE.Group();
  const [cx, cz] = b.center;
  const [w, h, d] = b.size;

  // 主体 + 顶部女儿墙（parapet）：matte 默认描边 weight 2（主结构全厚）。
  // 不投影不收影——阴影相机只罩房间，楼在视锥外，开了也是白算。
  const bodyMat = toon(b.color);
  const body = new THREE.Mesh(gbox(w, h, d), bodyMat);
  body.position.set(cx, h / 2, cz);
  g.add(body);
  const parapet = new THREE.Mesh(gbox(w + 0.24, 0.32, d + 0.24), bodyMat);
  parapet.position.set(cx, h + 0.16, cz);
  g.add(parapet);

  // 楼顶水箱 / 空调外机：metal 自动 weight 1（次结构细线）。
  // 观察者俯视时楼顶不能是一片光板，这两个盒子就是俯视剪影的全部内容。
  const metalMat = toon('#39415a', { finish: 'metal' });
  const tank = new THREE.Mesh(gbox(1.3, 1.1, 1.3), metalMat);
  tank.position.set(cx + (rnd() - 0.5) * w * 0.4, h + 0.32 + 0.55, cz + (rnd() - 0.5) * d * 0.4);
  g.add(tank);
  const ac = new THREE.Mesh(gbox(0.9, 0.55, 0.7), metalMat);
  ac.position.set(cx + (rnd() - 0.5) * w * 0.5, h + 0.32 + 0.28, cz + (rnd() - 0.5) * d * 0.5);
  g.add(ac);

  // 高层的航空障碍灯：夜里城市天际线上最容易被记住的一点点红
  if (h > 15) {
    const beacon = new THREE.Mesh(new THREE.SphereGeometry(0.07, 6, 5), emissive('#c9564a'));
    beacon.position.set(cx, h + 0.32 + 1.18, cz);
    g.add(beacon);
  }

  // 窗灯：emissive 默认 weight 0 不描边（黑壳会闷死 bloom），亮度过 bloom
  // threshold 0.86 自然起光晕。冷暖两种色温混排，熄灯的窗不画——夜里
  // 暗窗本来就是隐形的，画出来反而糊立面。
  const ff = faceFrame(b);
  const warm = emissive('#ffcf96');
  const cool = emissive('#c9d9f2');
  const floors = Math.max(1, Math.floor((h - 1.8) / 2.9));
  const cols = Math.max(1, Math.floor((ff.width - 1.2) / 1.35));
  const rotY = ff.axis === 'z' ? (ff.sign === 1 ? 0 : Math.PI) : (ff.sign === 1 ? Math.PI / 2 : -Math.PI / 2);
  for (let f = 0; f < floors; f++) {
    for (let c = 0; c < cols; c++) {
      if (rnd() > b.lit) continue;
      const win = new THREE.Mesh(WIN_GEO, rnd() < 0.78 ? warm : cool);
      const u = cols === 1 ? 0 : (c / (cols - 1) - 0.5) * (ff.width - 1.6);
      const y = 1.9 + f * 2.9;
      if (ff.axis === 'z') win.position.set(cx + u, y, cz + ff.sign * (ff.offset + 0.012));
      else win.position.set(cx + ff.sign * (ff.offset + 0.012), y, cz + u);
      win.rotation.y = rotY;
      g.add(win);
    }
  }

  return g;
}

/**
 * 近景街道层：对面楼群 + 路灯 + 街道湿地反光。
 *
 * 这是窗外景收益的大头——六个窗四个朝向，每个朝向望出去都得有真东西。
 * 整层返回后由调用方走标准装配（合批 → 描边 → add → 冻结），不进 FPS
 * 碰撞（阳台栏杆拦着，玩家本来就出不去，碰撞盒纯浪费）。
 *
 * 路灯三盏全在南街：南街是主景观面，灯是"房间里看出去唯一的暖光"。
 * 光晕不靠 Sprite（要守住"零新增透明"预算），灯头 emissive 过 bloom
 * threshold 自然起晕；灯下湿地光斑走 buildWetGround 那套现成画法。
 */
/* ============================================================================
 * 街道断面：建筑 → 人行道 → 路缘 → 车行道 → 排水篦 → 便利店侧人行道
 * ========================================================================== */

/**
 * 街道剖面（沿 +Z，复用本文件顶部的常量与便利店 z 段，单一事实源）：
 *   APT_ZB(6.3) .. 7.6   公寓侧人行道（路灯 LAMP_Z=7.4 在这条上）
 *   7.6 .. 12.6          车行道
 *   12.6 .. 14.0         便利店侧人行道
 * 见 buildStreetscape 顶部注释里的完整剖面；这里只给本模块要用的几个边界。
 * x 取 ±16：覆盖可见视窗（灯列 ±21 之间），更远处由远处地面改成的冷灰沥青兜底。
 */
const STREET_X0 = -16;
const STREET_X1 = 16;
const ROAD_Z0 = 7.6;
const ROAD_Z1 = 12.6;
const SIDE_B_Z1 = 14.0;        // 便利店侧人行道外缘（前厅从 14.0 起）
const CURB_H = 0.14;
const SURF_Y = -0.008;          // 略高于远处地面(-0.012)，压住它

/**
 * 街道断面几何。整组标 sceneCollideSkip：玩家到不了街道（阳台栏杆拦着），
 * 且合批前收集碰撞时整组跳过，避免把街道变成一片挡人地板。
 * 全部按上面的常量落到世界坐标，公寓 / 便利店 / 相机 / 灯光一律不动。
 */
function buildStreetSurface(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'street-surface';
  g.userData.sceneCollideSkip = true;

  const roadMat = toon('#ffffff', { map: asphaltTexture(), finish: 'wet' });
  // 人行道：城市广场石砖（赛璐璐硬边铺装），读起来是人行广场而非光混凝土
  const walkMat = toon('#ffffff', { map: plazaStoneTexture(), finish: 'wet' });
  const curbMat = toon('#ffffff', { map: curbTexture(), finish: 'wet' });
  const grateMat = toon('#2a2f37', { map: drainGrateTexture() });
  const lineMat = toon('#aeb6bf');
  const manholeMat = toon('#ffffff', { map: manholeTexture(), finish: 'metal' });

  const cx = (STREET_X0 + STREET_X1) / 2;
  const w = STREET_X1 - STREET_X0;

  const flat = (ww: number, dd: number, mat: THREE.Material, x: number, z: number, y = SURF_Y) => {
    const geo = new THREE.PlaneGeometry(ww, dd);
    geo.rotateX(-Math.PI / 2);
    scaleUV(geo, ww / 6, dd / 6);
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.set(x, y, z);
    mesh.receiveShadow = true;
    g.add(mesh);
    return mesh;
  };
  const add = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, shadow: 'both' | 'cast' | 'receive' | 'none' = 'both') => {
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.set(x, y, z);
    mesh.castShadow = shadow === 'both' || shadow === 'cast';
    mesh.receiveShadow = shadow === 'both' || shadow === 'receive';
    g.add(mesh);
    return mesh;
  };
  const flatAt = (ww: number, dd: number, mat: THREE.Material, x: number, z: number, y: number) => {
    const mesh = new THREE.Mesh(new THREE.PlaneGeometry(ww, dd), mat);
    mesh.rotation.x = -Math.PI / 2;
    mesh.position.set(x, y, z);
    g.add(mesh);
    return mesh;
  };

  // 两侧人行道
  flat(w, ROAD_Z0 - APT_ZB, walkMat, cx, (APT_ZB + ROAD_Z0) / 2);
  flat(w, SIDE_B_Z1 - ROAD_Z1, walkMat, cx, (ROAD_Z1 + SIDE_B_Z1) / 2);
  // 车行道
  flat(w, ROAD_Z1 - ROAD_Z0, roadMat, cx, (ROAD_Z0 + ROAD_Z1) / 2);

  // 路缘石：两侧人行道内缘 + 车行道两条边
  for (const z of [(APT_ZB + ROAD_Z0) / 2, (ROAD_Z1 + SIDE_B_Z1) / 2, ROAD_Z0 + 0.06, ROAD_Z1 - 0.06]) {
    add(gbox(w, CURB_H, 0.18), curbMat, cx, CURB_H / 2 - 0.01, z, 'both');
  }

  // 中央虚线（沿 z 横跨车行道）
  for (let z = ROAD_Z0 + 0.6; z < ROAD_Z1; z += 1.4) {
    flatAt(0.12, 0.7, lineMat, cx, z, SURF_Y + 0.002);
  }

  // 排水沟：凹槽 + 格栅（有深度的线性排水系统）
  const drainMat = toon('#222830');
  for (const z of [ROAD_Z0 + 0.25, ROAD_Z1 - 0.25]) {
    // 凹槽底板（下沉 4cm）
    flatAt(0.40, w, drainMat, cx, z, SURF_Y - 0.04);
    // 凹槽两壁
    add(gbox(0.04, 0.045, w), curbMat, cx - 0.22, SURF_Y - 0.018, z, 'none');
    add(gbox(0.04, 0.045, w), curbMat, cx + 0.22, SURF_Y - 0.018, z, 'none');
    // 格栅盖板（半透明暗示深度）
    flatAt(0.40, w, grateMat, cx, z, SURF_Y + 0.002);
  }
  // 井盖（带凸起金属边框）
  const mhRimMat = toon('#3a4149', { finish: 'metal' });
  for (const [px, pz] of [[-5, 9.5], [6, 11], [0, 8.2]] as Array<[number, number]>) {
    // 井盖面
    const mh = new THREE.Mesh(new THREE.CircleGeometry(0.30, 24), manholeMat);
    mh.rotation.x = -Math.PI / 2;
    mh.position.set(px, SURF_Y + 0.005, pz);
    g.add(mh);
    // 金属边框环
    const rim = new THREE.Mesh(new THREE.RingGeometry(0.30, 0.36, 24), mhRimMat);
    rim.rotation.x = -Math.PI / 2;
    rim.position.set(px, SURF_Y + 0.004, pz);
    g.add(rim);
  }

  return g;
}

/**
 * 局部浅水：只铺在低洼 / 路缘旁 / 排水口附近，不规则、极低不透明度、正常混合
 * （非叠加、非发蓝）。全路面 75~85% 仍是湿沥青，水是"上面薄薄一层现象"而非主角。
 * 涟漪（buildStreetscapeRipples）只落在这几处水面上。
 */
function buildStreetPuddles(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'street-puddles';
  g.userData.sceneCollideSkip = true;
  const tex = puddlePatchTexture();
  const spots: Array<[number, number, number, number]> = [
    [-6, ROAD_Z0 + 0.7, 1.6, 1.1],
    [5.5, ROAD_Z1 - 0.55, 1.4, 1.0],
    [2.2, 8.0, 1.1, 0.9],
    [-3.5, 11.2, 1.3, 1.0],
    [0.5, 9.6, 1.0, 0.8],
  ];
  const rnd = makeRng(7151);
  for (const [x, z, sx, sz] of spots) {
    const p = new THREE.Mesh(
      new THREE.PlaneGeometry(sx, sz),
      new THREE.MeshBasicMaterial({
        map: tex,
        color: new THREE.Color('#2b333d'),
        transparent: true,
        opacity: 0.22,
        depthWrite: false,
      })
    );
    p.rotation.x = -Math.PI / 2;
    p.rotation.z = rnd() * Math.PI;
    p.position.set(x, SURF_Y + 0.006, z);
    p.renderOrder = 1;
    g.add(p);
  }
  return g;
}

/* ============================================================================
 * 露天停车场（便利店西侧那片空地）
 * ========================================================================== */

/**
 * 便利店西侧原先只是一整张世界沥青——街角往西看是一块空白，纵深也少了收头。
 * 这里做成一片 5 台位的 24h コインパーキング：铺装 + 白线车位 + 车止め +
 * 精算機 + 竖招牌 + 高杆灯 + 围栅。
 *
 * 位置（世界坐标）：
 *   x  -19.0 .. -5.6   东端离店西山墙（ST.x0=-4.4）留 1.2m：那面墙上挑出一块
 *                      竖招牌（x≈-5.45..-4.75），地界要让它挑得出去。
 *   z   14.0 .. 25.6   北接便利店侧人行道外缘（SIDE_B_Z1 = 14.0）
 *   出入口  北侧西端 x∈[-19.0,-16.0]。人行道和车行道都只铺到 x=STREET_X0(-16)，
 *           所以口子开在这里不必去动 buildStreetSurface 那几道路缘石——
 *           它们一律只铺 x≥-16，西侧本来就是空地。
 */
const LOT = {
  x0: -19.0, x1: -5.6,
  z0: 14.0, z1: 25.6,
  bayZ0: 19.8,                 // 车位前缘（回车道一侧）
  bayZ1: 24.8,                 // 车位后缘
  bayW: 2.5,
  n: 5,
  entryX0: -19.0, entryX1: -16.0,
};

let _lotSurf: THREE.Texture | null = null;
/**
 * 停车场铺装：整片地面画进一张贴图（沥青底 + 白线 + 车位号 + 回车箭头）。
 *
 * 为什么不铺一张 plane 再往上叠几十条白线：白线只有 8cm 宽，叠在铺装上就
 * 是几十根共面的细长条，24bit 深度缓冲在这个尺度上必闪。画进贴图既没有
 * 共面问题，车位号这种"只能画在地上"的信息也顺带解决了——本文件约定不
 * 新增透明材质，所以不能用一张带 alpha 的数字贴片叠上去。
 *
 * 贴图 → 世界坐标（PlaneGeometry + rotateX(-90°)，flipY 默认 true）：
 *   px = (x - x0)/(x1 - x0) * W      py = (z - z0)/(z1 - z0) * H
 * 即画布的"上"是场地北侧（z 小）、"下"是南侧。H 按场地长宽比取，
 * 于是 X / Z 两个方向的"每米像素数"相同，线宽不会被拉扁。
 */
function lotSurfaceTexture(): THREE.Texture {
  if (_lotSurf) return _lotSurf;
  const LW = LOT.x1 - LOT.x0, LD = LOT.z1 - LOT.z0;
  const W = 512;
  const H = Math.round((W * LD) / LW);
  const { canvas, ctx } = makeCanvas(W, H);
  const rnd = makeRng(9023);
  const px = (x: number) => ((x - LOT.x0) / LW) * W;
  const pz = (z: number) => ((z - LOT.z0) / LD) * H;
  const mx = (m: number) => (m / LW) * W;   // 米 → 像素（沿 X）
  const mz = (m: number) => (m / LD) * H;   // 米 → 像素（沿 Z）

  ctx.fillStyle = '#454d56';
  ctx.fillRect(0, 0, W, H);
  // 碎石颗粒：纯色块在近景会读成"没有材质"，与 world-ground 同一路数
  for (let i = 0; i < 5200; i++) {
    const s = 0.5 + rnd() * 1.7;
    ctx.fillStyle = rnd() < 0.5
      ? `rgba(28,34,41,${(0.10 + rnd() * 0.20).toFixed(3)})`
      : `rgba(152,162,174,${(0.05 + rnd() * 0.12).toFixed(3)})`;
    ctx.fillRect(rnd() * W, rnd() * H, s, s);
  }
  // 几块深浅不一的补丁：修补过的路面 / 常年渗油的位子
  for (let i = 0; i < 7; i++) {
    ctx.fillStyle = rnd() < 0.5 ? 'rgba(28,34,41,0.45)' : 'rgba(122,134,148,0.15)';
    ctx.beginPath();
    ctx.ellipse(rnd() * W, rnd() * H, mx(0.6 + rnd() * 1.4), mz(0.5 + rnd() * 1.1), rnd() * Math.PI, 0, Math.PI * 2);
    ctx.fill();
  }

  // 车位白线：n+1 条长边 + 前后两条端线，线宽 8cm
  const bx0 = LOT.x0 + (LW - LOT.n * LOT.bayW) / 2;
  const bx1 = bx0 + LOT.n * LOT.bayW;
  ctx.strokeStyle = '#c9d0d6';
  ctx.lineWidth = mx(0.08);
  ctx.beginPath();
  for (let i = 0; i <= LOT.n; i++) {
    const X = px(bx0 + i * LOT.bayW);
    ctx.moveTo(X, pz(LOT.bayZ0));
    ctx.lineTo(X, pz(LOT.bayZ1));
  }
  ctx.moveTo(px(bx0), pz(LOT.bayZ0)); ctx.lineTo(px(bx1), pz(LOT.bayZ0));
  ctx.moveTo(px(bx0), pz(LOT.bayZ1)); ctx.lineTo(px(bx1), pz(LOT.bayZ1));
  ctx.stroke();

  // 车位号：コインパーキング 最标志性的一笔——地上刷着 1 / 2 / 3 …
  ctx.fillStyle = '#c9d0d6';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.font = `bold ${Math.round(mz(0.95))}px system-ui, "Segoe UI", sans-serif`;
  for (let i = 0; i < LOT.n; i++) {
    ctx.fillText(String(i + 1), px(bx0 + (i + 0.5) * LOT.bayW), pz((LOT.bayZ0 + LOT.bayZ1) / 2 + 0.7));
  }

  // 回车道方向箭头：从西侧出入口进来往东（+X）单向
  ctx.fillStyle = '#b7c0c9';
  const ax = px(-12.4), ay = pz(16.9), alen = mx(2.8), aw2 = mz(0.30), aw1 = mz(0.62);
  ctx.beginPath();
  ctx.moveTo(ax - alen / 2, ay - aw2);
  ctx.lineTo(ax + alen / 2 - mz(0.9), ay - aw2);
  ctx.lineTo(ax + alen / 2 - mz(0.9), ay - aw1);
  ctx.lineTo(ax + alen / 2, ay);
  ctx.lineTo(ax + alen / 2 - mz(0.9), ay + aw1);
  ctx.lineTo(ax + alen / 2 - mz(0.9), ay + aw2);
  ctx.lineTo(ax - alen / 2, ay + aw2);
  ctx.closePath();
  ctx.fill();

  _lotSurf = toTexture(canvas);
  return _lotSurf;
}

let _lotSign: THREE.Texture | null = null;
/**
 * 竖招牌：深蓝底 + 白 P + 24h + 料金。
 *
 * 与便利店那套招牌同一个取舍——写罗马字不写日文：画布字体的 CJK 字形在
 * 部分环境里缺字，竖招牌那块已经踩过一次，这里不再赌。
 */
function lotSignTexture(): THREE.Texture {
  if (_lotSign) return _lotSign;
  const W = 256, H = 160;
  const { canvas, ctx } = makeCanvas(W, H);
  ctx.fillStyle = '#1f2c3e';
  ctx.fillRect(0, 0, W, H);
  ctx.fillStyle = '#e8eef4';
  ctx.fillRect(0, 0, W, 7);
  ctx.fillRect(0, H - 7, W, 7);
  // 白 P：浅底方块 + 挖空太麻烦，直接用粗体 P 压在浅底上
  ctx.fillStyle = '#eaf0f6';
  ctx.fillRect(16, 28, 92, 104);
  ctx.fillStyle = '#1f2c3e';
  ctx.font = 'bold 78px system-ui, "Segoe UI", sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('P', 62, 82);
  ctx.textAlign = 'left';
  ctx.fillStyle = '#dfe8f1';
  ctx.font = 'bold 34px system-ui, "Segoe UI", sans-serif';
  ctx.fillText('24h', 122, 58);
  ctx.fillStyle = '#9fb4c9';
  ctx.font = 'bold 21px system-ui, "Segoe UI", sans-serif';
  ctx.fillText('100yen', 122, 98);
  ctx.fillText('/ 60min', 122, 124);
  _lotSign = toTexture(canvas);
  return _lotSign;
}

/**
 * 便利店西侧的露天停车场。整组标 sceneCollideSkip：玩家被阳台栏杆拦着，
 * 到不了街上；也别让这片地变成一张挡人的地板。
 */
function buildParkingLot(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'parking-lot';
  g.userData.sceneCollideSkip = true;

  const LW = LOT.x1 - LOT.x0, LD = LOT.z1 - LOT.z0;
  const cx = (LOT.x0 + LOT.x1) / 2, cz = (LOT.z0 + LOT.z1) / 2;

  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.set(x, y, z);
    if (ry) mesh.rotation.y = ry;
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    g.add(mesh);
    return mesh;
  };
  const bx = (w: number, h: number, d: number) => gbox(w, h, d);
  const flatAt = (w: number, d: number, mat: THREE.Material, x: number, z: number, y: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.set(x, y, z);
    mesh.receiveShadow = true;
    g.add(mesh);
    return mesh;
  };

  const curbMat = toon('#ffffff', { map: curbTexture(), finish: 'wet' });
  const stopMat = toon('#b8bfc6');                       // 车止め：素混凝土
  const steelMat = toon(EXT.metal);
  const carDark = toon('#1f252e');                       // 车窗 / 保险杠
  const carTire = toon('#22272f');
  const carLamp = toon('#c8d0d8');
  const carTail = toon('#6f3630');

  /* ---- 铺装 ---- */
  flatAt(LW, LD, toon('#ffffff', { map: lotSurfaceTexture(), finish: 'wet' }), cx, cz, -0.006);

  /* ---- 出入口引道 ----
   * 车行道与人行道都止于 x=-16，那条边之外本来就是空地。这里从路口西端
   * 往南接 2.2m，读作"从街的尽端拐进来"——不必去动街道那几道路缘石。 */
  const apGeo = new THREE.PlaneGeometry(3.1, 2.2);
  apGeo.rotateX(-Math.PI / 2);
  scaleUV(apGeo, 3.1 / 6, 2.2 / 6);
  const apron = new THREE.Mesh(apGeo, toon('#ffffff', { map: asphaltTexture(), finish: 'wet' }));
  apron.position.set(-17.55, -0.006, 12.9);
  apron.receiveShadow = true;
  g.add(apron);

  /* ---- 周界路缘（北 / 西 / 南 / 东），北侧西端留出入口 ----
   * 与街上同款路缘石、同高 0.14：一眼能读出"这里是铺装边界"。 */
  put(bx(LOT.x1 - LOT.entryX1, 0.14, 0.18), curbMat, (LOT.entryX1 + LOT.x1) / 2, 0.06, LOT.z0 + 0.09);
  put(bx(0.18, 0.14, LD), curbMat, LOT.x0 + 0.09, 0.06, cz);
  put(bx(LW, 0.14, 0.18), curbMat, cx, 0.06, LOT.z1 - 0.09);
  put(bx(0.18, 0.14, LD), curbMat, LOT.x1 - 0.09, 0.06, cz);

  /* ---- 南侧 + 西侧围栅：立柱 + 两道横档 ----
   * 不做金網（要透明材质，本文件约定不新增）；两道横档 + 立柱在十几米外
   * 读起来就是围着空地的一圈栅栏。 */
  for (const sx of [-18.2, -15.2, -12.2, -9.2, -6.2]) {
    put(bx(0.06, 1.3, 0.06), steelMat, sx, 0.65, LOT.z1 - 0.18);
  }
  for (const sz of [15.0, 18.5, 22.0, 25.2]) {
    put(bx(0.06, 1.3, 0.06), steelMat, LOT.x0 + 0.18, 0.65, sz);
  }
  put(bx(LW - 0.5, 0.05, 0.05), steelMat, cx, 0.62, LOT.z1 - 0.18);
  put(bx(LW - 0.5, 0.05, 0.05), steelMat, cx, 1.18, LOT.z1 - 0.18);
  put(bx(0.05, 0.05, LD - 0.5), steelMat, LOT.x0 + 0.18, 0.62, cz);
  put(bx(0.05, 0.05, LD - 0.5), steelMat, LOT.x0 + 0.18, 1.18, cz);

  /* ---- 车止め + 车位 ---- */
  const bayX0 = LOT.x0 + (LW - LOT.n * LOT.bayW) / 2;
  const bayCx = (i: number) => bayX0 + (i + 0.5) * LOT.bayW;
  for (let i = 0; i < LOT.n; i++) {
    put(bx(1.9, 0.14, 0.16), stopMat, bayCx(i), 0.07, LOT.bayZ1 - 0.5);
  }

  /* ---- 停着的车 ----
   * 隔一条街也读得出车头车尾，才不至于把停车场摆成一排砖。车长沿 X、
   * 车头朝 +X；ry=-π/2 把车头转到 +Z（朝里泊入）。 */
  const parkedCar = (x: number, z: number, ry: number, bodyColor: string, len = 1) => {
    const L = 4.05 * len, Wd = 1.72;
    const body = toon(bodyColor, { finish: 'metal' });
    // 整台车一个 Group：零件坐标留在车的局部系里（车长沿 X、车头朝 +X），
    // 摆放方向只由 group 的 rotation.y 决定——逐件去算旋转后的世界偏移，
    // 轮子早晚要摆错地方。
    const car = new THREE.Group();
    car.position.set(x, 0, z);
    car.rotation.y = ry;
    const add = (geo: THREE.BufferGeometry, mat: THREE.Material, lx: number, ly: number, lz: number) => {
      const mesh = new THREE.Mesh(geo, mat);
      mesh.position.set(lx, ly, lz);
      mesh.castShadow = true;
      car.add(mesh);
    };
    // 车身 / 座舱（暗色玻璃带）/ 车顶 / 前后风挡 / 保险杠 / 四轮 / 灯
    add(bx(L, 0.62, Wd), body, 0, 0.60, 0);
    add(bx(L * 0.52, 0.46, Wd - 0.18), carDark, 0, 1.16, 0);
    add(bx(L * 0.46, 0.06, Wd - 0.10), body, 0, 1.41, 0);
    add(bx(0.07, 0.42, Wd - 0.26), carDark, 0, 1.14, 0);
    add(bx(0.16, 0.20, Wd + 0.02), carDark, L / 2 - 0.02, 0.44, 0);
    add(bx(0.16, 0.20, Wd + 0.02), carDark, -(L / 2 - 0.02), 0.44, 0);
    const wheel = new THREE.CylinderGeometry(0.29, 0.29, 0.20, 10);
    wheel.rotateX(Math.PI / 2);
    for (const sx of [L * 0.33, -L * 0.33]) {
      for (const sz of [Wd / 2 - 0.07, -(Wd / 2 - 0.07)]) add(wheel, carTire, sx, 0.29, sz);
    }
    add(bx(0.06, 0.11, 0.30), carLamp, L / 2 + 0.01, 0.66, Wd * 0.26);
    add(bx(0.06, 0.11, 0.30), carLamp, L / 2 + 0.01, 0.66, -Wd * 0.26);
    add(bx(0.06, 0.10, 0.26), carTail, -(L / 2 + 0.01), 0.68, Wd * 0.28);
    add(bx(0.06, 0.10, 0.26), carTail, -(L / 2 + 0.01), 0.68, -Wd * 0.28);
    g.add(car);
  };
  // 5 台位空一格：满员的停车场像样板间，空着那格才像真的在营业
  parkedCar(bayCx(0), 22.2, -Math.PI / 2, '#5f6d7c');
  parkedCar(bayCx(1), 22.2, -Math.PI / 2, '#4a5460');
  parkedCar(bayCx(3), 22.2, -Math.PI / 2, '#6d6157');
  parkedCar(bayCx(4), 22.2, -Math.PI / 2, '#55606b', 0.9);   // 軽自動車，短一截

  /* ---- 精算機（出入口东侧，朝街）---- */
  put(bx(0.70, 0.06, 0.52), steelMat, -15.3, 0.03, 14.75);
  put(bx(0.62, 1.50, 0.42), toon('#39404a'), -15.3, 0.81, 14.75);
  put(bx(0.80, 0.06, 0.60), steelMat, -15.3, 1.60, 14.75);
  {
    const scr = new THREE.Mesh(new THREE.PlaneGeometry(0.44, 0.34), emissive('#bcd8e6'));
    scr.position.set(-15.3, 1.16, 14.53);
    scr.rotation.y = Math.PI;
    g.add(scr);
  }

  /* ---- 竖招牌 ---- */
  put(new THREE.CylinderGeometry(0.05, 0.06, 2.5, 8), steelMat, -16.7, 1.25, 14.5);
  put(bx(1.42, 0.92, 0.09), toon('#26313f'), -16.7, 2.55, 14.5);
  for (const [dz, ry] of [[-0.055, Math.PI], [0.055, 0]] as Array<[number, number]>) {
    const face = new THREE.Mesh(new THREE.PlaneGeometry(1.30, 0.82), emissive('#ffffff', { map: lotSignTexture() }));
    face.position.set(-16.7, 2.55, 14.5 + dz);
    face.rotation.y = ry;
    g.add(face);
  }

  /* ---- 高杆灯 ----
   * 不挂 PointLight：街上整排已经 12 盏点光，每多一盏整层光照都跟着变贵。
   * 这里只要"夜里这片地是亮的"这个读感——自发光灯头 + 湿地光池（在
   * buildStreetscape 的 pools 里给）已经够。 */
  put(new THREE.CylinderGeometry(0.16, 0.19, 0.07, 10), steelMat, -18.2, 0.035, 16.2);
  put(new THREE.CylinderGeometry(0.055, 0.075, 6.0, 10), steelMat, -18.2, 3.0, 16.2);
  put(bx(0.52, 0.06, 0.06), steelMat, -17.94, 5.93, 16.2);
  put(bx(0.44, 0.09, 0.28), steelMat, -17.70, 5.84, 16.2);
  put(bx(0.36, 0.05, 0.24), emissive('#e6dcc4'), -17.70, 5.77, 16.2);

  /* ---- 边角植栽：南边贴栅一小段绿篱，把水泥地的冷硬压一压 ---- */
  const leafMat = toon(EXT.plant), leafDeep = toon(EXT.plantDeep);
  g.add(buildShrub(-18.1, 25.0, 1.5, 0.75, 0.9, 7711, { leaf: leafMat, leafDeep }));
  g.add(buildShrub(-16.8, 25.0, 1.2, 0.62, 0.8, 7712, { leaf: leafMat, leafDeep }));
  g.add(buildShrub(-6.4, 24.9, 1.3, 0.68, 0.85, 7713, { leaf: leafMat, leafDeep }));

  /* ---- 积水：雨夜的湿地面要有几处反光的洼 ---- */
  const pTex = puddlePatchTexture();
  for (const [px2, pz2, sx2, sz2] of [[-11.5, 17.6, 2.2, 1.4], [-8.2, 22.6, 1.6, 1.1], [-16.2, 21.4, 1.9, 1.2]] as Array<[number, number, number, number]>) {
    const p = new THREE.Mesh(
      new THREE.PlaneGeometry(sx2, sz2),
      new THREE.MeshBasicMaterial({ map: pTex, color: new THREE.Color('#2b333d'), transparent: true, opacity: 0.2, depthWrite: false })
    );
    p.rotation.x = -Math.PI / 2;
    p.rotation.z = px2 * 0.7;
    p.position.set(px2, -0.002, pz2);
    p.renderOrder = 1;
    g.add(p);
  }

  return g;
}

export function buildStreetscape(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'exterior-streetscape';

  // 远景改为一整圈环状贴图：不再用四面平面拼接，旋转到四个角落时不会出现
  // 贴图边缘错位。CylinderGeometry 的 UV 沿圆周连续展开，贴图自身横向可重复，
  // 场景任意方向都能读到同一套远处城市；它仍不承担碰撞，也不受 fog far 吞掉。
  const horizonMat = new THREE.MeshBasicMaterial({
    map: nightHorizonTexture(), transparent: true, opacity: 0.86,
    depthWrite: false, side: THREE.DoubleSide, fog: false,
  });
  const horizon = new THREE.Mesh(
    new THREE.CylinderGeometry(78, 78, 42, 128, 1, true),
    horizonMat,
  );
  horizon.name = 'night-horizon-ring';
  // 必须显式标 noCollide：它是半径 78m 的整圈幕布，AABB 是 156×42×156 的实体块。
  // 场景遍历补碰撞（buildSceneColliders）只看标记和透明度，而它 opacity=0.86 不算
  // 半透明、又不带任何跳过标记 → 会被收成一个包住整张地图的巨型盒，玩家在任何位置
  // 都判定撞墙，第一人称 WASD 全失效。
  horizon.userData.noCollide = true;
  horizon.position.set(0, 16, 0);
  horizon.renderOrder = -5;
  g.add(horizon);

  const rnd = makeRng(20260902);
  BUILDINGS.forEach((b, i) => {
    const bg = buildBuilding(b, rnd);
    // 命名：外景自检要逐栋判相交，组整体 AABB 必然互相包含，判不出东西
    bg.name = `bldg-${b.face}${i}`;
    // 远景楼群是窗外景观、玩家到不了；且合批前收集碰撞时，整组标 skip 让遍历跳过
    // （遍历只收路灯这类该挡人的实体，不放行进景楼群）。
    bg.userData.sceneCollideSkip = true;
    g.add(bg);
  });

  // 路灯：公寓侧（z7.4，阳台栏杆 z6.3 外 1.1m）+ 便利店侧（z12.9）两排，
  // 沿整条路每隔 ~7m 一盏；挑臂分别指向 +Z / -Z 悬到车行道上方，灯泡在挑臂
  // 末端 armLen=0.30 处，湿地光斑就铺在它正下方。
  const LAMP_X = [-21, -7, 0, 7, 14, 21];             // 公寓侧：整条路；x=-14 落在入口西侧自行车棚（x∈[-16,-9]）内会穿模，去掉
  const STORE_LAMP_X = [-21, -14, 0, 7, 14, 21];      // 便利店侧：整条路；避开 x∈[-12.75,-6.25] 的平行车位（不立灯杆在车上）
  const LAMP_Z = 7.4;
  const STORE_LAMP_Z = 12.9;
  const ARM = 0.3;
  /*
   * 这几处用 streak 而不是 pool：夜里站在街上看湿沥青，主角从来不是灯脚
   * 下那团圆光，而是被路面拖成一条的软光带——一团圆光晕只在灯正下方那一
   * 小块成立，画满整条街就变成"地上摆了一排圆形贴片"。
   * len 取 8~10m：路灯 2.35m 高、观察者在十几米外斜看，倒影大致这个长度。
   */
  const pools: PoolSpec[] = [
    ...LAMP_X.map((x, i) => ({
      pos: [x, LAMP_Z + ARM] as [number, number],
      size: 1.5,
      len: 8.5 + (i % 3) * 0.9,
      shape: 'streak' as const,
      color: '#ffd9a0',
      opacity: 0.20,
    })),
    ...STORE_LAMP_X.map((x, i) => ({
      pos: [x, STORE_LAMP_Z + ARM] as [number, number],
      size: 1.5,
      len: 8.5 + (i % 3) * 0.9,
      shape: 'streak' as const,
      color: '#ffd9a0',
      opacity: 0.18,
    })),
  ];
  for (const x of LAMP_X) {
    const lamp = buildStreetLamp([x, 0, LAMP_Z], 2.35, { halo: false });
    lamp.rotation.y = -Math.PI / 2;
    g.add(lamp);
  }
  for (const x of STORE_LAMP_X) {
    const lamp = buildStreetLamp([x, 0, STORE_LAMP_Z], 2.5, { halo: false });
    lamp.rotation.y = Math.PI / 2;
    g.add(lamp);
  }

  // 公寓入口檐下的暖光洒到街上：整条街唯一一团"暖"的地光，
  // 它把入口从冷蓝街景里拎出来，是观察者第一眼落点。
  pools.push(
    { pos: [0, 6.3], size: 3.4, color: '#ffdca8', opacity: 0.20, rot: 0.2 },
    { pos: [0, 6.9], size: 1.3, len: 6.0, shape: 'streak', color: '#ffcf96', opacity: 0.16 }
  );

  // 街心两道冷色长反射：对面店铺灯箱 / 楼窗的冷光在湿路面上的回应。
  // 比路灯那几条更长更淡——远处的光拖得更长、也更散。
  pools.push(
    { pos: [-1.8, 9.6], size: 2.2, len: 15.0, shape: 'streak', color: '#9fb6d8', opacity: 0.10 },
    { pos: [3.6, 10.2], size: 1.9, len: 17.0, shape: 'streak', color: '#8fa8cc', opacity: 0.085 }
  );

  // 便利店那片：灯箱 + 店内冷白光在门口铺一小片"干的"亮地，
  // 亮度比路灯高但更冷——这是全画面最亮的一块，得克制着给。
  pools.push(
    { pos: [0, 7.9], size: 6.5, color: '#cfe0f2', opacity: 0.10, rot: -0.1 },
    { pos: [-3.2, 8.4], size: 1.6, len: 11.0, shape: 'streak', color: '#c3d8ee', opacity: 0.09 },
    { pos: [3.4, 8.4], size: 1.6, len: 11.0, shape: 'streak', color: '#c3d8ee', opacity: 0.09 }
  );

  // 停车场的光：高杆灯脚下一团暖光 + 整片一层很淡的冷白。
  // 后者的存在感全靠"比周围亮半档"——没有它，空停车场在夜里是一块纯黑的洞。
  pools.push(
    { pos: [-17.7, 16.2], size: 2.8, color: '#ffd7a4', opacity: 0.20, rot: 0.15 },
    { pos: [-12.3, 20.0], size: 6.2, color: '#9fb0c6', opacity: 0.075, rot: -0.08 }
  );

  // 街道断面（人行道 / 路缘 / 车行道 / 排水篦 / 井盖）+ 局部浅水：压在远处蓝地面之上，
  // 把"一块均匀蓝地板"变成"能读出剖面层次的雨夜街道"。整组已标 sceneCollideSkip。
  g.add(buildStreetSurface());
  g.add(buildStreetPuddles());

  // 便利店西侧空地 → 露天停车场（5 台位的 コインパーキング）
  g.add(buildParkingLot());

  // 碎光点撒在街心——阳台那套 wetPools 在 JSON 里，这套是街道自己的。
  const wet = buildWetGround(pools, { sparkleCount: 70, area: 6.0, center: [0.3, 9.5] });
  g.add(wet);

  return g;
}

/* ============================================================================
 * 窗户
 * ========================================================================== */

/**
 * 亮灯窗的"窗内浅盒"贴图：一张暖底剪影顶掉整个室内。
 *
 * 每户逐户建室内不现实，隔着 8m+ 街道也根本看不清。剪影给的是
 * "这户有人生活"的信号：
 *   0 客厅  沙发 + 落地灯 + 电视
 *   1 书房  书架 + 书桌 + 显示器
 *   2 卧室  床 + 床头灯
 *   3 餐厅  吊灯 + 餐桌
 *   4 玄関  只有门厅一盏灯（很暗，读得出"刚回家"）
 * 五种轮换 + 每户随机亮度，楼立面就有了层次，不会所有亮窗长得一样。
 */
const glowCache = new Map<number, THREE.Texture>();
function roomGlowTexture(kind: number): THREE.Texture {
  const hit = glowCache.get(kind);
  if (hit) return hit;
  const { canvas, ctx } = makeCanvas(128, 128);
  // 暖底：顶亮底暗（灯从天花下来）
  const grad = ctx.createLinearGradient(0, 0, 0, 128);
  if (kind === 4) {
    grad.addColorStop(0, '#c9a678');
    grad.addColorStop(1, '#8d7351');
  } else {
    grad.addColorStop(0, '#ffe9c0');
    grad.addColorStop(1, '#d9b787');
  }
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 128, 128);
  ctx.fillStyle = 'rgba(64,52,40,0.80)';
  if (kind === 0) {
    // 客厅：窗帘缝 + 沙发 + 落地灯 + 电视
    ctx.fillRect(6, 0, 7, 128);
    ctx.fillRect(115, 0, 7, 128);
    ctx.fillRect(22, 82, 58, 30);           // 沙发
    ctx.fillRect(22, 74, 58, 10);           // 沙发背
    ctx.fillRect(96, 34, 4, 78);            // 落地灯杆
    ctx.beginPath(); ctx.arc(98, 28, 10, 0, Math.PI * 2); ctx.fill(); // 灯罩
    ctx.fillRect(52, 40, 34, 22);           // 电视（暗屏）
  } else if (kind === 1) {
    // 书房：书架 + 书桌 + 显示器（亮）+ 台灯
    ctx.fillRect(10, 18, 30, 94);           // 书架
    ctx.fillRect(48, 74, 56, 6);            // 桌面
    ctx.fillRect(52, 80, 4, 34);            // 桌腿
    ctx.fillRect(96, 80, 4, 34);
    ctx.fillRect(60, 48, 26, 20);           // 显示器（先画深色壳）
    ctx.fillRect(104, 44, 3, 30);           // 台灯杆
    ctx.beginPath(); ctx.arc(105, 40, 6, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = '#cfe2f2';
    ctx.fillRect(63, 51, 20, 14);           // 屏幕亮面
  } else if (kind === 2) {
    // 卧室：床 + 床头灯
    ctx.fillRect(14, 78, 76, 26);           // 床
    ctx.fillRect(14, 64, 14, 16);           // 床头板
    ctx.fillStyle = '#f2ead8';
    ctx.fillRect(20, 70, 22, 10);           // 枕头
    ctx.fillStyle = 'rgba(64,52,40,0.80)';
    ctx.fillRect(100, 62, 16, 42);          // 床头柜
    ctx.beginPath(); ctx.arc(108, 52, 8, 0, Math.PI * 2); ctx.fill(); // 灯罩
  } else if (kind === 3) {
    // 餐厅：吊灯 + 餐桌 + 两把椅子背
    ctx.fillRect(60, 0, 3, 40);             // 吊灯线
    ctx.beginPath(); ctx.arc(61, 46, 12, 0, Math.PI * 2); ctx.fill(); // 灯罩
    ctx.fillRect(30, 84, 62, 5);            // 桌面
    ctx.fillRect(36, 89, 4, 28);            // 桌腿
    ctx.fillRect(84, 89, 4, 28);
    ctx.fillRect(22, 74, 6, 22);            // 椅背
    ctx.fillRect(96, 74, 6, 22);
  } else {
    // 玄関：只有一盏门厅灯，其余全是暗的家具轮廓
    ctx.fillStyle = 'rgba(64,52,40,0.55)';
    ctx.fillRect(8, 0, 10, 128);            // 鞋柜
    ctx.fillRect(104, 62, 18, 46);          // 柜子
    ctx.fillStyle = '#ffdca8';
    ctx.beginPath(); ctx.arc(64, 18, 9, 0, Math.PI * 2); ctx.fill(); // 门厅灯
  }
  const tex = toTexture(canvas);
  glowCache.set(kind, tex);
  return tex;
}

/**
 * 窗玻璃贴图：一张上亮下暗的冷蓝渐变 + 两道斜向反光。
 *
 * 夜里的玻璃不是纯色——它映着比地面亮得多的夜空，所以上半部偏亮偏蓝，
 * 下半部落进街面反着的暗色。没有这个渐变，熄灯窗就是一块死板的深色矩形，
 * 也就是"把窗户做成黑色贴片"最典型的破绽。
 */
let _glass: THREE.Texture | null = null;
function glassTexture(): THREE.Texture {
  if (_glass) return _glass;
  const { canvas, ctx } = makeCanvas(64, 128);
  const grad = ctx.createLinearGradient(0, 0, 0, 128);
  grad.addColorStop(0.0, '#4a5c78');
  grad.addColorStop(0.45, '#33415a');
  grad.addColorStop(1.0, '#232c3d');
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 64, 128);
  // 两道斜向反光：玻璃是平的，但绝不"平得均匀"
  ctx.globalAlpha = 0.14;
  ctx.fillStyle = '#8fa8c8';
  ctx.beginPath();
  ctx.moveTo(0, 30); ctx.lineTo(64, 0); ctx.lineTo(64, 16); ctx.lineTo(0, 52);
  ctx.closePath(); ctx.fill();
  ctx.globalAlpha = 0.08;
  ctx.beginPath();
  ctx.moveTo(0, 96); ctx.lineTo(64, 58); ctx.lineTo(64, 70); ctx.lineTo(0, 112);
  ctx.closePath(); ctx.fill();
  ctx.globalAlpha = 1;
  _glass = toTexture(canvas);
  return _glass;
}

/**
 * 拉帘窗的「帘后透光」贴图：灰阶亮度层，颜色由材质 emissive 给。
 *
 * 拉上的窗帘绝不是一块均匀暖色——布褶让透光一条亮一条暗，顶部贴帘轨有
 * 一道阴影，帘摆底部堆在窗台上更暗。这三样缺了哪样，帘窗都是「糊一块
 * 纯色」的贴片感。画在灰阶上，同一张贴图将来配不同 emissive 色
 * 可以复用。
 */
let _curtainGlow: THREE.Texture | null = null;
function curtainGlowTexture(): THREE.Texture {
  if (_curtainGlow) return _curtainGlow;
  const { canvas, ctx } = makeCanvas(128, 256);
  const rnd = makeRng(412);

  // 1) 底：上亮下暗——灯多在上方，帘摆往下堆积
  const g = ctx.createLinearGradient(0, 0, 0, 256);
  g.addColorStop(0, '#ffffff');
  g.addColorStop(0.55, '#e9e9e9');
  g.addColorStop(1, '#c2c2c2');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 128, 256);

  // 2) 布褶漏光：竖向明暗条。每条褶的暗部只占条宽的一半——鼓起来的
  //    面透光、褶谷背光，条与条之间亮度也不等。这是帘窗「活」的关键。
  for (let x = 0; x < 128; ) {
    const w = 6 + rnd() * 10;
    ctx.fillStyle = `rgba(0,0,0,${(0.06 + rnd() * 0.16).toFixed(3)})`;
    ctx.fillRect(x, 0, w * 0.42, 256);
    x += w;
  }

  // 3) 顶部帘轨阴影 + 底部帘摆堆积
  const top = ctx.createLinearGradient(0, 0, 0, 18);
  top.addColorStop(0, 'rgba(0,0,0,0.38)');
  top.addColorStop(1, 'rgba(0,0,0,0)');
  ctx.fillStyle = top;
  ctx.fillRect(0, 0, 128, 18);
  const bot = ctx.createLinearGradient(0, 234, 0, 256);
  bot.addColorStop(0, 'rgba(0,0,0,0)');
  bot.addColorStop(1, 'rgba(0,0,0,0.30)');
  ctx.fillStyle = bot;
  ctx.fillRect(0, 234, 128, 22);

  // 4) 横向织纹：很淡的一组细线，凑近才读得出，但中景就有「这是布」的信号
  ctx.globalAlpha = 0.06;
  ctx.strokeStyle = '#000';
  ctx.lineWidth = 1;
  for (let i = 0; i < 30; i++) {
    const y = rnd() * 256;
    ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(128, y); ctx.stroke();
  }
  ctx.globalAlpha = 1;

  _curtainGlow = toTexture(canvas);
  return _curtainGlow;
}

/**
 * 1F 北面一排小店的招牌贴图。五种店各自一张，夜里从街上读得出店类。
 *
 * 招牌是店面唯一"从远处第一眼能读出来"的部件——橱窗陈列隔着玻璃只能
 * 看个朦胧，但招牌的颜色 / 字体 / 灯光方式直接告诉路人"这是家什么店"。
 * 五种分别走不同的视觉语言：
 *   0  ゲームセンター  深底 + 霓虹粉紫 + 像素字体感
 *   1  ベーカリー      暖米底 + 深棕字 + 麦穗图标
 *   2  ラーメン        暖红底 + 白字 + 碗图标（中户最显眼）
 *   3  本屋            绿底 + 白字 + 书脊图标
 *   4  クリーニング    淡蓝底 + 深蓝字 + 衣架图标
 */
/**
 * 1F 北面一排小店的招牌 atlas：一张大贴图上画 10 个招牌，每个 256×64，
 * 纵向排列成 256×640。材质只占一个桶，每家店用 UV offset 选自己的那行。
 *
 * 招牌是店面唯一"从远处第一眼能读出来"的部件。10 种店各自走不同的
 * 视觉语言，夜里从街上读得出店类：
 *   0  ゲームセンター  深紫底 + 霓虹粉/青 + 扫描线
 *   1  ベーカリー      暖米底 + 深棕字 + 麦穗
 *   2  ラーメン        暖红底 + 白字 + 碗 + 蒸汽
 *   3  本屋            深绿底 + 白字 + 书脊
 *   4  クリーニング    淡蓝底 + 深蓝字 + 衣架
 *   5  蜜雪冰城        白底 + 红字 + 雪花（奶茶 / 冰饮）
 *   6  居酒屋          暖木底 + 红灯笼 + 黑字
 *   7  花屋            淡粉底 + 绿叶 + 深粉字
 *   8  薬局            白底 + 绿十字 + 深绿字
 *   9  コンビニ        暖黄底 + 深棕字 + 条纹
 */
let _shopSignAtlas: THREE.Texture | null = null;
const SHOP_SIGN_COUNT = 10;
function shopSignAtlas(): THREE.Texture {
  if (_shopSignAtlas) return _shopSignAtlas;
  const CELL_H = 64;
  const { canvas, ctx } = makeCanvas(256, CELL_H * SHOP_SIGN_COUNT);

  /** 在第 kind 行的区域内画招牌 */
  const drawSign = (kind: number, fn: (ctx: CanvasRenderingContext2D) => void) => {
    ctx.save();
    ctx.translate(0, kind * CELL_H);
    fn(ctx);
    ctx.restore();
  };

  drawSign(0, (c) => {
    c.fillStyle = '#1a0e2e'; c.fillRect(0, 0, 256, 64);
    c.globalAlpha = 0.12; c.fillStyle = '#ff44aa';
    for (let y = 0; y < 64; y += 3) c.fillRect(0, y, 256, 1);
    c.globalAlpha = 1;
    c.font = 'bold 26px sans-serif'; c.textAlign = 'center';
    c.fillStyle = '#ff44aa'; c.fillText('GAME', 78, 42);
    c.fillStyle = '#44ddff'; c.fillText('CENTER', 178, 42);
    for (let i = 0; i < 6; i++) { c.fillStyle = ['#ff44aa','#44ddff','#ffdd44'][i%3]; c.fillRect(8+i*8,52,5,5); }
  });
  drawSign(1, (c) => {
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#f5e6cc'); g.addColorStop(1,'#e8d5b0');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    c.strokeStyle = '#a08050'; c.lineWidth = 2;
    for (const sx of [30,226]) { c.beginPath(); c.moveTo(sx,14); c.lineTo(sx,50); c.stroke();
      for (const sy of [22,32,42]) { c.beginPath(); c.moveTo(sx-6,sy); c.lineTo(sx,sy+4); c.stroke(); c.beginPath(); c.moveTo(sx+6,sy); c.lineTo(sx,sy+4); c.stroke(); } }
    c.font = 'bold 24px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#6b4a2a'; c.fillText('BAKERY',128,40);
  });
  drawSign(2, (c) => {
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#c93a2a'); g.addColorStop(1,'#a82a1a');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    c.fillStyle = '#fff'; c.beginPath(); c.arc(40,38,16,0,Math.PI,true); c.fill(); c.fillRect(24,38,32,4);
    c.strokeStyle = 'rgba(255,255,255,0.5)'; c.lineWidth = 2;
    for (const [sx,sy] of [[34,16],[44,12]]) { c.beginPath(); c.moveTo(sx,sy); c.quadraticCurveTo(sx+5,sy-6,sx,sy-12); c.stroke(); }
    c.font = 'bold 24px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#fff'; c.fillText('らーめん',150,42);
  });
  drawSign(3, (c) => {
    c.fillStyle = '#2a4a3a'; c.fillRect(0,0,256,64);
    for (let i=0;i<5;i++) { c.fillStyle = ['#8fa38c','#b3a68d','#c2a49c','#8fa38c','#b3a68d'][i]; c.fillRect(12+i*7,16,5,36); }
    c.font = 'bold 22px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#e8f0e8'; c.fillText('BOOK SHOP',160,42);
  });
  drawSign(4, (c) => {
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#d0dde8'); g.addColorStop(1,'#bccfdc');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    c.strokeStyle = '#2a5070'; c.lineWidth = 2.5;
    c.beginPath(); c.moveTo(40,18); c.lineTo(40,30); c.moveTo(30,48); c.lineTo(40,30); c.lineTo(50,48); c.stroke();
    c.beginPath(); c.arc(40,16,3,0,Math.PI*2); c.stroke();
    c.font = 'bold 20px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#2a5070'; c.fillText('CLEANING',160,42);
  });
  drawSign(5, (c) => {
    // 蜜雪冰城：白底 + 红字 + 雪花
    c.fillStyle = '#ffffff'; c.fillRect(0,0,256,64);
    // 雪花
    c.strokeStyle = '#e04040'; c.lineWidth = 1.5;
    for (const [sx,sy] of [[28,32],[228,32],[20,20],[236,46]]) {
      for (let a=0;a<6;a++) { const ang=a*Math.PI/3; c.beginPath(); c.moveTo(sx,sy); c.lineTo(sx+Math.cos(ang)*7,sy+Math.sin(ang)*7); c.stroke(); }
    }
    c.font = 'bold 28px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#e04040';
    c.fillText('蜜雪冰城',128,42);
  });
  drawSign(6, (c) => {
    // 居酒屋：暖木色底 + 红灯笼 + 黑字
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#5a3e28'); g.addColorStop(1,'#4a3220');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    // 灯笼
    c.fillStyle = '#cc3322'; c.beginPath(); c.ellipse(36,32,12,16,0,0,Math.PI*2); c.fill();
    c.fillStyle = '#222'; c.fillRect(30,16,12,3); c.fillRect(30,48,12,3);
    c.font = 'bold 26px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#f0e0c0';
    c.fillText('居酒屋',160,42);
  });
  drawSign(7, (c) => {
    // 花屋：淡粉底 + 绿叶 + 深粉字
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#f5e8ec'); g.addColorStop(1,'#ecdde4');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    // 花
    c.fillStyle = '#d4608a';
    for (const [fx,fy] of [[28,32],[222,28],[218,46]]) { c.beginPath(); c.arc(fx,fy,7,0,Math.PI*2); c.fill(); }
    c.fillStyle = '#6a9c5a';
    for (const [lx,ly] of [[36,38],[214,36],[226,40]]) { c.beginPath(); c.ellipse(lx,ly,5,3,0.5,0,Math.PI*2); c.fill(); }
    c.font = 'bold 24px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#b04068'; c.fillText('花屋',140,42);
  });
  drawSign(8, (c) => {
    // 薬局：白底 + 绿十字 + 深绿字
    c.fillStyle = '#ffffff'; c.fillRect(0,0,256,64);
    c.fillStyle = '#2a8a4a';
    c.fillRect(30,24,20,6); c.fillRect(37,17,6,20);  // 十字
    c.font = 'bold 24px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#2a6a3a';
    c.fillText('薬 局',160,42);
  });
  drawSign(9, (c) => {
    // コンビニ：暖黄底 + 深棕字 + 条纹边
    const g = c.createLinearGradient(0,0,0,64); g.addColorStop(0,'#f5d870'); g.addColorStop(1,'#e8c858');
    c.fillStyle = g; c.fillRect(0,0,256,64);
    // 上下条纹
    c.fillStyle = '#8a6a3a';
    for (let x=0;x<256;x+=12) { c.fillRect(x,0,6,4); c.fillRect(x,60,6,4); }
    c.font = 'bold 22px sans-serif'; c.textAlign = 'center'; c.fillStyle = '#5a3e1a';
    c.fillText('CONVENI',128,42);
  });

  _shopSignAtlas = toTexture(canvas);
  return _shopSignAtlas;
}

/** 给招牌 PlaneGeometry 设置 UV，让它只显示 atlas 的第 kind 行 */
function shopSignUV(geo: THREE.PlaneGeometry, kind: number) {
  const rowH = 1 / SHOP_SIGN_COUNT;
  const y0 = 1 - (kind + 1) * rowH;
  const y1 = 1 - kind * rowH;
  const uv = geo.getAttribute('uv') as THREE.BufferAttribute;
  uv.setXY(0, 0, y1); uv.setXY(1, 1, y1); uv.setXY(2, 0, y0); uv.setXY(3, 1, y0);
  uv.needsUpdate = true;
}

/** 一扇窗的状态。夜里一栋楼里这些状态必须混着出现，全亮就假了。 */
export type WinState =
  | 'dark'      // 完全熄灯
  | 'lit'       // 亮灯（暖黄剪影）
  | 'lamp'      // 只亮一盏台灯
  | 'curtain'   // 拉着窗帘，只有缝里漏光
  | 'ajar'      // 半开（一扇窗扇推开）
  | 'frost';    // 磨砂玻璃 / 浴室，只透出一团朦胧暖光

/**
 * 户号牌 / 楼层牌贴图：深灰底 + 米白字。
 *
 * 多户共同住宅的第一读法就是门牌——201/202/203/204/205、301..305、2F/3F/4F。
 * 配一层很暗的背光（emissive 半亮），夜里既读得出数字又不抢戏。
 */
const plateCache = new Map<string, THREE.Texture>();
function numberPlateTexture(label: string): THREE.Texture {
  let tex = plateCache.get(label);
  if (!tex) {
    const { canvas, ctx } = makeCanvas(96, 64);
    ctx.fillStyle = '#2b323d';
    ctx.fillRect(0, 0, 96, 64);
    ctx.fillStyle = '#3a424e';
    ctx.fillRect(3, 3, 90, 58);
    ctx.font = 'bold 32px system-ui, "Segoe UI", sans-serif';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = '#e6e0d2';
    ctx.fillText(label, 48, 34);
    tex = toTexture(canvas);
    plateCache.set(label, tex);
  }
  return tex;
}

/**
 * 户主牌（表札）贴图：奶油亚克力底 + 深棕字 + 细木框，写住户名字。
 *
 * 与户号牌同样走自发光（夜里也读得出），但配色反过来——户号牌是冷灰底亮字，
 * 表札是暖奶油底深字，两者并排时一眼能区分「门牌号」和「谁住这户」。
 * 字号随名字长度自适应：中文名偏长要缩，避免溢出。
 */
const nameCache = new Map<string, THREE.Texture>();
function namePlateTexture(name: string): THREE.Texture {
  let tex = nameCache.get(name);
  if (!tex) {
    const W = 200, H = 80;
    const { canvas, ctx } = makeCanvas(W, H);
    ctx.fillStyle = '#f3ecdc';
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = '#e9e0cb';
    ctx.fillRect(3, 3, W - 6, H - 6);
    ctx.strokeStyle = '#b9a986';
    ctx.lineWidth = 3;
    ctx.strokeRect(5, 5, W - 10, H - 10);
    let fs = 38;
    if (name.length > 7) fs = 30;
    if (name.length > 10) fs = 24;
    if (name.length > 14) fs = 19;
    ctx.font = `bold ${fs}px system-ui, "Segoe UI", "Hiragino Sans", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", sans-serif`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = '#41372b';
    ctx.fillText(name, W / 2, H / 2 + 2);
    tex = toTexture(canvas);
    nameCache.set(name, tex);
  }
  return tex;
}

/**
 * 各户住户名（表札）。
 * 203 固定为主角户 Vivian Nana（Vivian 与 Nana 同住）；其余 14 户用固定分配——
 * 12 个常见大模型名 + 2 个空串（无人居住，表札留白），正好 14 张对应 14 户，
 * 每个名字恰好出现一次、恰好两户空置。
 *
 * 早期版本用 Math.random 每构建重新洗牌，但那样「交换两个具体名字的位置」无法稳定复现、
 * 也无法按户精修。改为固定分配后，所有位置都可预测、可微调配。本版已按需求把 Kimi 与 DeepSeek 对调。
 * key 为「楼层+户序」三位串（与 door loop 里 `${fi + 2}${unitNum}` 同构）。
 */
const RESIDENT_POOL = [
  'chatGPT', 'Claude', 'Gemini', 'Grok', 'Kimi', 'Qwen',
  'DeepSeek', 'GLM', 'ERNIE', 'Mistral', 'Gemma', 'Doubao',
  '', '', // 两户空置（无人居住）
];
const RESIDENT_UNITS = [
  '201', '202', '204', '205',
  '301', '302', '303', '304', '305',
  '401', '402', '403', '404', '405',
];
const RESIDENTS: Record<string, string> = { '203': 'Vivian Nana' };
RESIDENT_POOL.forEach((name, i) => { RESIDENTS[RESIDENT_UNITS[i]] = name; });

/* ============================================================================
 * 公寓楼外壳（203 室所在的这栋四层公寓）
 * ========================================================================== */

/**
 * 东端共用外楼梯的几何常量。
 *
 * buildApartmentShell（本体）和 buildApartmentEaveDrips（滴水动效）两处共用，
 * 放模块级保证标高永不漂移。布局（俯视，东端局部）：
 *
 *        za0 ┌────────────┐── 北梯带：第一跑(1F→2F) + 第二跑(2F→3F) 上下叠合
 *            │  西侧平台   │      ↖ 两跑都向西上行，到达西侧平台
 *   妻壁→ ═══╡ lx0     lx1 │
 *            │  2F 步行廊  │── 南梯带：2F 标高的步行廊（向东去折返平台）
 *        zb1 └────────────┘
 *             wx1      ex1（东端折返平台：转身 180° 上第二跑）
 */
export const STRS = {
  lx0: 31.15,   // 西侧平台西缘（贴东妻壁外皮 31.08）
  lx1: 32.45,   // 西侧平台东缘 = 两跑梯的到达边
  wx1: 36.55,   // 2F 步行廊东缘 = 东端折返平台西缘
  ex1: 37.75,   // 东端折返平台东缘（也是整座楼梯的东界）
  f1Bot: 36.95, // 第一跑起步（地面）前缘
  za0: -7.75, za1: -6.52, // 北梯带：两跑上行梯上下叠合
  zb0: -6.52, zb1: -5.29, // 南梯带：2F 步行廊 / 楼下半开放区
};

/**
 * 公寓楼结构常量（供第一人称碰撞体构建复用，避免 RoomScene 再写死一套、
 * 与 exterior.ts 标高漂移打架）。单点真相。
 */
export const FLOORS = [3.4, 6.2, 9.0]; // 2F / 3F / 4F 世界标高（四层：1F + 2~4F）
export const APT_X0 = -31;            // 公寓西端（妻壁外皮）
export const APT_X1 = 31;             // 公寓东端（妻壁外皮；楼梯在其东侧外挑）
export const APT_ZN = -5.9;           // 北立面（外廊侧）
export const APT_ZF = 4.7;            // 主体量南立面（阳台内缘；1F 无阳台，南墙就在这条线上）
export const APT_ZB = 6.3;            // 阳台外缘（栏杆线；2F 起阳台挑出到此）
export const APT_CORRIDOR_N = -7.15;  // 北侧外廊北缘（挑出 1.25m）
/**
 * 邻居户室内空腔的进深：各住宅层的实心体量南端往北退这一段，腾出的空间
 * 由「带洞结构板 + 室盒内衬 + 低模家具」填回去。外轮廓仍收在 z=APT_ZF
 * （结构板补位），所以楼的体量与外墙碰撞盒都不跟着动。
 * 碰撞侧也是同一条：空腔南界要单独声明一道墙（见 APT_WALL_BOXES），
 * 否则玩家能从邻居家的阳台直接走进屋里。
 */
export const APT_ROOM_D = 1.0;

/**
 * 由世界 AABB 区间生成碰撞盒声明（中心 + 尺寸）——写区间比写中心好核对。
 *
 * 公寓楼里所有「该挡人」的实体（外壳四壁 / 阳台栏杆 / 外廊腰壁 / 外置楼梯栏杆 /
 * 自行车）都用它声明，再统一交给 collider.ts 的 buildBoxColliders 生成世界 AABB。
 * 不盲从 mesh 几何——否则门洞会被重新堵死、地坪高度还会生成水平盒把人钉死。
 */
export function boxSpec(
  id: string, x0: number, x1: number, y0: number, y1: number, z0: number, z1: number
): BoxColliderSpec {
  return {
    id,
    pos: [(x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2],
    size: [x1 - x0, y1 - y0, z1 - z0],
  };
}

/** 局部盒绕 Y 旋转后的世界 AABB 声明（扩盒公式与 buildFurnitureColliders 同源） */
function rotBoxSpec(
  id: string, x: number, z: number, rot: number, size: [number, number, number], y0: number
): BoxColliderSpec {
  const c = Math.abs(Math.cos(rot));
  const s = Math.abs(Math.sin(rot));
  const ex = (size[0] * c + size[2] * s) / 2;
  const ez = (size[0] * s + size[2] * c) / 2;
  return { id, pos: [x, y0 + size[1] / 2, z], size: [ex * 2, size[1], ez * 2] };
}

/**
 * 街道剖面（沿 +Z，从公寓北墙 z-5.9 往南）：
 *   -7.15 .. -5.9  北侧外廊（挑出 1.25m）
 *   -5.9  ..  4.7  主体量（进深 10.6m）
 *     4.7  ..  6.3  南侧阳台（挑出 1.6m，与 203 的阳台同一条线）
 *     6.3  ..  7.6  公寓侧人行道（三盏路灯 z7.4 在这条上）
 *     7.6  .. 12.6  车行道：斑马线、停车位
 *   12.6  .. 14.0  便利店侧人行道（贩卖机 / 自行车 / 护栏）
 *
 * 关键点：主体量只到 z4.7，阳台是真正挑出墙外的结构。
 * 旧版把阳台和栏杆埋在实体体量内部（体量一直铺到 z6.3），从街上看
 * 整栋楼就是"一整片墙 + 贴在墙上的窗和栏杆线"——这正是"白模感"的根源。
 *
 * 竖向：层高 2.8，1F（0..3.36）+ 2F（3.44..6.16）+ 3F（6.24..8.96）
 *      + 4F（9.04..11.76）+ 屋顶（11.8..12.25）。四层，中小型体量。
 * 楼层之间留 4cm 空气缝（GAP）：和 203 地板/天花共用标高会 z-fighting。
 *
 * 公共交通系统（本次补全的核心，"单侧外廊 + 一端共用外梯"）：
 *   地面 → 东端外置钢楼梯第一跑 → 2F 平台（连 2F 外廊 / 2F 各户门）
 *        → 2F 步行廊 → 2F 折返平台 → 第二跑 → 3F 平台（连 3F 外廊）
 *        → 3F 步行廊 → 3F 折返平台 → 第三跑 → 4F 平台（连 4F 外廊）
 *   各跑都在北梯带内上下叠合（净空 ~2.8m），南梯带是各层步行廊——
 *   折返关系逐层重复、真实连续，不是每层各自为政的摆设楼梯。
 *   楼梯下方（2F 平台下）是储物间 + 半开放自行车/设备位。
 */
/**
 * 公寓外壳构建结果。
 *
 * boxes 与 group 同源产出：每建一段该挡人的实体（栏杆 / 腰壁 / 自行车），
 * 就在建 mesh 的同一处声明它的碰撞盒，两条信息不会各写各的而漂移。
 * 调用方把 boxes 与 APT_WALL_BOXES 一起交给 buildBoxColliders 即可。
 */
export type ApartmentShellHandle = {
  group: THREE.Group;
  boxes: BoxColliderSpec[];
};

export function buildApartmentShell(): ApartmentShellHandle {
  const g = new THREE.Group();
  g.name = 'apartment-shell';
  /** 外壳内所有该挡人的实体：一个实体一个轴对齐盒，与家具碰撞同一套机制 */
  const boxes: BoxColliderSpec[] = [];

  /* ---------------- 材质（冷灰蓝体系，见文件头 EXT） ---------------- */
  const M = {
    wall: toon(EXT.wall),                                  // 主墙：浅冷灰
    wallAlt: toon(EXT.wallAlt),                            // 山墙 / 北面 / 楼梯间
    wallDeep: toon(EXT.wallDeep),                          // 入口凹龛 / 楼梯间竖向体量
    wallBase: toon(EXT.wallBase),                          // 底层基座
    slab: toon(EXT.slab, { finish: 'wet' }),               // 阳台板：轻微潮湿反射
    slabDry: toon(EXT.slab),                               // 檐口 / 走廊板
    slabDark: toon(EXT.slabDark),                          // 戸境板 / 窗台
    railing: toon(EXT.railing, { finish: 'metal' }),       // 栏杆竖栅：炭灰半哑光金属
    railTop: toon(EXT.railTop, { finish: 'metal' }),       // 扶手
    frame: toon(EXT.frame),                                // 窗框
    glass: toon('#ffffff', { map: glassTexture(), finish: 'glass' }),
    /* 能看进屋里的窗玻璃：finish:'glass' 只给反光和描边权重，真透明得靠
     * transparent + 低 opacity + 关 depthWrite——不关 depthWrite 的话，
     * 这块玻璃会写进深度缓冲，把它后面的室内家具整片挡掉。
     * opacity 0.3 是"看得清家具、又留一层夜空反光"的折中。 */
    winGlass: toon('#cfe0ee', {
      map: glassTexture(), finish: 'glass',
      transparent: true, opacity: 0.3, depthWrite: false,
    }),
    unlit: toon(EXT.unlit),
    wood: toon(EXT.wood),
    woodDeep: toon(EXT.woodDeep),
    metal: toon(EXT.metal, { finish: 'metal' }),
    pipe: toon(EXT.pipe, { finish: 'metal' }),
    dark: toon(EXT.dark),
    /* —— 共用外楼梯（深灰钢构 + 浅灰踏步，绝不用纯白） —— */
    stairSteel: toon('#3a424c', { finish: 'metal' }),   // 侧梁 / 立柱 / 平台框 / 栏杆立柱
    tread: toon('#a3aab2', { finish: 'wet' }),          // 踏步板：浅灰、雨夜微湿
    treadRise: toon('#8b929b'),                         // 踢面板：比踏面深一档
    walkFloor: toon('#9aa1a9', { finish: 'wet' }),      // 外廊 / 平台地面：中浅灰防滑、微湿
    gutter: toon('#2c333d'),                            // 排水沟 / 集水口
    trunkWall: toon('#59626e'),                         // 楼梯间储物墙体：深灰混凝土
    /* —— 住户门的三档差异（同一建筑语言内的"每户不一样"） —— */
    doorB: toon('#74604a'),                             // 深一档的木门
    doorC: toon('#515d6b'),                             // 蓝灰门
    leaf: toon(EXT.plant),
    leafDeep: toon(EXT.plantDeep),
    leafWarm: toon(EXT.plantWarm),
    trunk: toon(EXT.trunk),
    // 光：暖黄（室内 / 入口 / 廊下）与冷白（门禁屏 / 楼梯间）分开
    warm: emissive('#ffcf96'),
    warmSoft: emissive('#e8b57e'),
    warmPale: emissive('#ffd9a0'),
    hall: emissive('#ffe9c4', { side: THREE.DoubleSide }),
    hallDim: emissive('#d8b98c', { side: THREE.DoubleSide }), // 廊灯的"暗档"
    cool: emissive('#c9d9f2'),
    /* —— 邻居户室内（低模家具 + 内衬，不可到达、不产生碰撞盒）——
     * 只给「亮灯」那一档配材质。熄灯户一律复用上面的深色调（wallDeep /
     * woodDeep / dark）——黑屋里本来也分不出木纹和布纹，省下三种材质，
     * 对合批后的 draw call 是实打实的。
     * 亮灯档的墙/地带一点暖色 emissive：场景里没有室内点光源（十几户各一盏
     * 就得十几个光源，前向渲染扛不住），"这户开着灯"只能靠材质自发光说出来。
     * 值压得很低，够把亮灯户从一片冷蓝夜景里拎出来即可，高了会被 bloom
     * 糊成一团光斑。 */
    rmWallLit: toon('#e7dcc6', { emissive: '#2a2216' }),   // 亮室墙面：暖米白
    rmFloorLit: toon('#c6a67e', { emissive: '#241c10' }),  // 亮室地板：淡木
    rmWoodLit: toon('#a9805a'),      // 亮室家具木
    rmClothLit: toon('#8fa38c'),     // 亮室布面：柔和绿
    rmClothAlt: toon('#b3a68d'),     // 亮室布面第二档：亚麻米（每户沙发/床品二选一）
    /* —— 拉帘窗的材质组 ——
     * 帘布直接复用 203 房内那张 curtainTexture（褶皱竖条 + 亚麻织纹），
     * 同参数走 toon 缓存 = 同一个材质实例，不占新桶；帘后透光层是新桶，
     * 一整栋楼几十扇帘窗共用它。 */
    curtainCloth: toon('#ffffff', { map: curtainTexture(), side: THREE.DoubleSide }),
    curtainGlow: emissive('#e8b57e', { map: curtainGlowTexture() }),
    /* 1F 店面招牌：10 种店共用 1 张 atlas 贴图 → 1 个 emissive 桶。
     * 每家店用 shopSignUV() 设 UV offset 选自己的那行。 */
    shopSign: emissive('#ffffff', { map: shopSignAtlas() }),
  };

  /** 邻居户室内按亮灯/熄灯取用的两套材质。熄灯档不新增材质。 */
  const RM = {
    lit: { wall: M.rmWallLit, floor: M.rmFloorLit, wood: M.rmWoodLit, cloth: M.rmClothLit },
    dark: { wall: M.wallDeep, floor: M.woodDeep, wood: M.woodDeep, cloth: M.dark },
  } as const;
  type RmMats = typeof RM.lit | typeof RM.dark;

  /**
   * 室内一个分区的状态。卧室（西腰窓）和客厅（滑門）各一份——同一户里
   * "卧室亮着灯、客厅黑着"或反过来都是常态；帘拉上的分区连家具都不用建，
   * 帘后透暖光就足够读出"有人睡下了"。
   */
  type ZoneState = { lit: boolean; furnish: boolean };

  /* ---------------- 体量常量 ---------------- */
  // 楼栋加宽：每一户都双倍宽（12.4m），5 户 = 62m。
  // 203 仍居中（[-6.2, 6.2]），东西各两户对称展开。
  const X0 = APT_X0, X1 = APT_X1, W = 62;
  const ZN = APT_ZN;  // 北立面（外廊侧）
  const ZF = APT_ZF;  // 南立面（阳台内缘，与 203 南墙同一条线）
  const ZB = APT_ZB;  // 阳台外缘（栏杆线，与 203 阳台外缘同一条线）
  const F2 = 3.4, LVL = 2.8;
  // FLOORS 已提到模块级导出（见文件头），这里直接复用同一套标高
  const RF = F2 + LVL * 3;                       // 11.8 屋顶
  const GAP = 0.04;
  /** 每户的 x 边界：5 户等宽 12.4m，203 居中占中户 */
  const UNITS: Array<[number, number]> = [
    [-31, -18.6], [-18.6, -6.2], [-6.2, 6.2], [6.2, 18.6], [18.6, 31],
  ];
  /** 戸境（户与户之间的分隔）位置，含两端妻壁 */
  const PARTITIONS = [X0, -18.6, -6.2, 6.2, 18.6, X1];

  const put = (
    geo: THREE.BufferGeometry,
    mat: THREE.Material,
    x: number, y: number, z: number,
    ry = 0
  ): THREE.Mesh => {
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.set(x, y, z);
    if (ry) mesh.rotation.y = ry;
    g.add(mesh);
    return mesh;
  };
  /**
   * 朝 dir 方向的立面平板。
   *
   * 旧版有两个方向写反的函数（faceS 实际朝 -Z 却用在南立面），结果是
   * 立面上的窗/门/海报从街上看是背面剔除掉的——从房间里望出去有，绕到
   * 外面就没了。这里统一成一个函数，dir 就是法线方向，不再有"南北"歧义。
   */
  const panel = (
    w: number, h: number, mat: THREE.Material,
    x: number, y: number, z: number, dir: 1 | -1
  ): THREE.Mesh => {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(w, h), mat);
    m.position.set(x, y, z);
    if (dir < 0) m.rotation.y = Math.PI;
    g.add(m);
    return m;
  };
  /** 贴地平面（标线 / 草坪）：法线朝上 */
  const flatUp = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    return put(geo, mat, x, y, z);
  };
  /** 朝下的平面（雨棚下的灯带）——从街上抬头看的是这一面 */
  const flatDown = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(Math.PI / 2);
    return put(geo, mat, x, y, z);
  };

  const rnd = makeRng(20260903);

  /**
   * 楼层牌材质（按层缓存）。
   *
   * emissive() 每次调用都新建一个材质——同层的"2F"在外廊西端和楼梯山墙各挂
   * 一块，放任它建就是两份贴图内容一模一样、却各占一次 draw call 的材质。
   * 三层楼省下来是三次提交。户号牌 / 表札没法这么收：那两层每户内容都不同。
   */
  const floorPlateMats = new Map<string, THREE.Material>();
  const floorPlateMat = (label: string): THREE.Material => {
    let m = floorPlateMats.get(label);
    if (!m) {
      m = emissive('#cfc4b0', { map: numberPlateTexture(label) });
      floorPlateMats.set(label, m);
    }
    return m;
  };

  /**
   * 亮灯窗的材质按（剪影种类 × 亮度档）缓存。
   *
   * 两个原因，缺一不可：
   *  1. 合批：不缓存的话每扇亮窗都是一份独立材质，mergeByMaterial 按材质分组，
   *     一栋楼三十几扇亮窗就是三十几个 draw call，整层的合批直接白做。
   *  2. 亮度：真楼里没有两户的灯一样亮——有的开着主灯，有的只开玄関那盏。
   *     三档亮度配五种剪影，随机组合出来就有"每户不一样"的层次，
   *     而不是一片统一的暖黄色块。
   */
  const GLOW_LEVELS = [0.58, 0.80, 1.0];
  const glowMats = new Map<string, THREE.Material>();
  const glowMaterial = (kind: number, level: number): THREE.Material => {
    const k = `${kind}|${level}`;
    let mat = glowMats.get(k);
    if (!mat) {
      const c = new THREE.Color(1, 1, 1).multiplyScalar(GLOW_LEVELS[level]);
      mat = emissive(`#${c.getHexString()}`, { map: roomGlowTexture(kind) });
      glowMats.set(k, mat);
    }
    return mat;
  };

  /* ==========================================================================
   * 窗玻璃收集器
   *
   * mergeByMaterial 会跳过所有透明材质——透明物体得能按到相机的距离逐个排序，
   * 并成一个就排不了。一扇窗一块玻璃，一栋楼六十扇就是六十次 draw call，
   * 合批等于白做（改透明窗那次，合批数从 123 直接跳到 178）。
   *
   * 但立面上的窗玻璃全都共面、朝向一致、彼此不重叠：合并它们既不影响排序
   * 也不影响深度测试，是安全的。这里先攒着，buildApartmentShell 收尾时按
   * 材质一次性合并——六十次提交压回一次。
   * ======================================================================== */
  const paneBuf: Array<{ geo: THREE.BufferGeometry; mat: THREE.Material }> = [];
  const addPane = (
    w: number, h: number, mat: THREE.Material, x: number, y: number, z: number, dir: 1 | -1
  ) => {
    const geo = new THREE.PlaneGeometry(w, h);
    if (dir < 0) geo.rotateY(Math.PI);
    geo.translate(x, y, z);
    paneBuf.push({ geo, mat });
  };
  /** 收尾：按材质合并成尽量少的几个 mesh。合并失败就原样退回，宁可少赚一次合批 */
  const flushPanes = () => {
    const byMat = new Map<THREE.Material, THREE.BufferGeometry[]>();
    for (const p of paneBuf) {
      const arr = byMat.get(p.mat);
      if (arr) arr.push(p.geo);
      else byMat.set(p.mat, [p.geo]);
    }
    for (const [mat, geos] of byMat) {
      const merged = geos.length > 1 ? mergeGeometries(geos, false) : geos[0];
      if (merged) {
        if (merged !== geos[0]) for (const gg of geos) gg.dispose();
        g.add(new THREE.Mesh(merged, mat));
      } else {
        for (const gg of geos) g.add(new THREE.Mesh(gg, mat));
      }
    }
    paneBuf.length = 0;
  };

  /* ==========================================================================
   * 一扇窗：墙上的洞，不是贴在墙上的片
   *
   * 结构自外向内：窗框（凸出墙面 2cm）→ 玻璃（退进 6cm）→ 亮灯时窗内
   * 浅盒（退进 7cm）；洞口四周有 14cm 深的侧壁（reveal），窗台外挑。
   * dir = +1 朝 +Z（南 / 街），-1 朝 -Z（北 / 外廊）。
   * ======================================================================== */
  const windowUnit = (o: {
    x: number; y: number; z: number; dir: 1 | -1;
    w: number; h: number;
    state: WinState;
    kind?: number;
    /** 省掉窗洞侧壁：北立面高窗是背面，不值得花这份顶点 */
    simple?: boolean;
    sill?: boolean;
    /**
     * 玻璃/窗内贴片的绝对 z。默认按 14cm 墙腔内退 6cm——那要求立面后真有腔
     * （2~4F 南立面的室内空腔）。1F 实心体量与北立面的楼体面就贴在立面线后
     * 1~2.5cm，内退 6cm 的玻璃会整个埋进实体里，从外只能看到楼体素面；
     * 这两处的窗由调用方给 paneZ，把玻璃贴到楼体表面上。
     */
    paneZ?: number;
    /**
     * 玻璃后真有可看的室内（2~4F 南立面邻居户）：玻璃换透明档，亮灯不再贴
     * 发光剪影——屋里是内衬 + 低模家具，亮不亮由室内材质和吸顶灯自己说话。
     */
    seeThrough?: boolean;
    /**
     * 这扇窗装窗帘（南立面的住宅窗）。拉着的场合帘展开铺满窗洞；
     * 其他状态帘收在两侧——每扇窗都有帘轨和收拢的帘，这是"住人的屋子"
     * 最省的一笔证据。布料复用同一种材质，几十扇帘合批后还是一次提交。
     */
    curtain?: boolean;
    /**
     * 不画玻璃自带的四根外框：洞口已经有 unitFace 的覆板窗套在收边
     * （南立面邻居户）。两圈同色的 M.frame 矩形一里一外叠着，从街上
     * 看就是"窗洞外又套了一圈无厚度的框"——而 203 自家窗在立面侧
     * 只有窗套那一圈。中竖梃照常保留。
     */
    noFrame?: boolean;
  }): void => {
    const { x, y, z, dir, w, h } = o;
    const state = o.state ?? 'dark';
    const inZ = (t: number) => z - dir * t; // 从立面向墙内退 t
    const pz = o.paneZ ?? inZ(0.06);        // 玻璃落点
    const glassMat = o.seeThrough ? M.winGlass : M.glass;

    /** 一片褶皱帘布：横向正弦竖褶。收拢帘和展开帘是同一种东西，只是宽窄。
     *  褶皱用双频正弦叠加（低频大褶 + 高频小褶），单频从斜侧看是"波纹铁皮"；
     *  布纹贴图（curtainTexture）叠在几何褶上，材质与几何各自出褶，不追求
     *  逐条对齐——真实布的织纹本来就不跟褶皱位走。 */
    const drape = (cw: number, ch: number, px: number, py: number, dz: number) => {
      const geo = new THREE.PlaneGeometry(cw, ch, 12, 1);
      const p = geo.getAttribute('position') as THREE.BufferAttribute;
      for (let i = 0; i < p.count; i++) {
        const u = p.getX(i) / cw + 0.5;
        p.setZ(i, Math.sin(u * Math.PI * 4) * 0.035 + Math.sin(u * Math.PI * 9 + 1.7) * 0.012);
      }
      geo.computeVertexNormals();
      const m = new THREE.Mesh(geo, M.curtainCloth);
      m.position.set(px, py, dz);
      if (dir < 0) m.rotation.y = Math.PI;
      m.userData.noOutline = true; // 双面褶皱片描边会糊成一团
      g.add(m);
    };

    // 0) 帘轨：窗顶内侧一根细杆。装了窗帘的窗才有。
    if (o.curtain) {
      put(gbox(w + 0.12, 0.028, 0.028), M.metal, x, y + h / 2 + 0.05, inZ(0.045));
    }

    // 1) 窗洞侧壁：四圈窄面，法线朝洞口中心。
    //    这是"窗嵌在墙里"唯一的证据，也是最容易被省掉的一笔。
    //    只在真有洞的地方画（2~4F 南立面：玻璃面 z-0.06 到覆板内背面 z+0.02），
    //    且两端面刻意避开既有面——外端若落在 z=ZF 会与结构板外表面共面闪面，
    //    内端若越过玻璃面又会从斜角戳进室内。1F / 北面的 paneZ 窗贴在实心
    //    楼体表面上、没有洞，侧壁不画。
    if (!o.simple && !o.paneZ) {
      const t = 0.05;
      const rc = inZ(0.02), rd = 0.08;
      put(gbox(w + t * 2, t, rd), M.slabDark, x, y + h / 2 + t / 2, rc);   // 上
      put(gbox(w + t * 2, t, rd), M.slabDark, x, y - h / 2 - t / 2, rc);   // 下
      put(gbox(t, h, rd), M.slabDark, x - w / 2 - t / 2, y, rc);           // 左
      put(gbox(t, h, rd), M.slabDark, x + w / 2 + t / 2, y, rc);           // 右
    }

    // 2) 窗内：亮灯用剪影贴图，熄灯用深色玻璃。
    //    熄灯窗必须有玻璃（映着夜空的冷蓝渐变），不能是一块死黑。
    //    seeThrough 的窗例外：屋里是真家具，玻璃直接透明，剪影体系不进来。
    if (state === 'lit') {
      if (o.seeThrough) {
        addPane(w, h, glassMat, x, y, pz, dir);
      } else {
        const level = Math.floor(rnd() * GLOW_LEVELS.length);
        panel(w, h, glowMaterial(o.kind ?? 0, level), x, y, o.paneZ ? pz : inZ(0.07), dir);
      }
    } else if (state === 'lamp') {
      // 只亮一盏台灯：整窗仍是暗玻璃，只有靠下角一小块暖光
      addPane(w, h, glassMat, x, y, pz, dir);
      panel(w * 0.34, h * 0.30, M.warmSoft, x - w * 0.22, y - h * 0.26, inZ(0.075), dir);
    } else if (state === 'curtain') {
      // 拉着窗帘：帘后透出一层暖光。透光层带布褶竖条 + 上下渐变（见
      // curtainGlowTexture）——纯色平面是帘窗"贴图感"的最大来源。
      // 有 paneZ 的窗（1F 实心面）没有腔可退，帘和衬布都贴在楼体表面铺开
      panel(w, h, M.curtainGlow, x, y, o.paneZ ? pz : inZ(0.075), dir);
      for (const s of [-1, 1]) {
        drape(w * 0.52, h, x + s * w * 0.24, y, o.paneZ ? pz + dir * 0.02 : inZ(0.055));
      }
    } else if (state === 'frost') {
      // 磨砂玻璃（浴室 / 厕所）：一团朦胧暖光，看不出家具
      panel(w, h, M.warmSoft, x, y, pz, dir);
      panel(w, h * 0.62, M.warmPale, x, y + h * 0.16, o.paneZ ? pz - dir * 0.005 : inZ(0.065), dir);
    } else {
      addPane(w, h, glassMat, x, y, pz, dir);
    }

    // 2b) 收拢的窗帘：装了帘的窗在没拉上时，帘收在两侧各一束。
    //     拉开 / 收拢 / 半拉在楼里各不相同——"每户住着不一样的人"最便宜的
    //     一笔表达；布材质共享，几十束帘合批后仍是一次提交。
    if (o.curtain && state !== 'curtain') {
      const cw = Math.max(0.09, w * 0.13);
      for (const s of [-1, 1]) {
        drape(cw, h * 0.94, x + s * (w / 2 - cw * 0.55), y, inZ(0.05));
      }
    }

    // 3) 窗框：外框四根 + 一根中竖梃（引違い窗的两扇交界）。
    //    窗框要凸出墙面 2cm，否则从斜角看窗是"陷进去的一个洞"。
    const fw = 0.05;
    // 已有覆板窗套的洞口（unitFace trim）走 noFrame：外框四根只贴玻璃画，
    // 窗套已在洞口外圈框了一整圈——两圈同色矩形叠着就是那条"多余的框"。
    if (!o.noFrame) {
      const fz = z + dir * 0.01;
      put(gbox(w + fw * 2, fw, 0.05), M.frame, x, y + h / 2 + fw / 2, fz);
      put(gbox(w + fw * 2, fw, 0.05), M.frame, x, y - h / 2 - fw / 2, fz);
      put(gbox(fw, h, 0.05), M.frame, x - w / 2 - fw / 2, y, fz);
      put(gbox(fw, h, 0.05), M.frame, x + w / 2 + fw / 2, y, fz);
      // 上框再压一道 6mm 的深色线：真实窗框的上沿总有一道阴影缝
      put(gbox(w + fw * 2, 0.012, 0.055), M.dark, x, y + h / 2 + fw + 0.006, fz);
    }
    // 中竖梃：玻璃窗扇的分界（引違い窗两扇相叠处），不随外框开关——分扇的
    // 暗示在两圈框之争里是干净的，203 自家窗透过玻璃也能看到同款中梃。
    put(gbox(0.04, h, 0.035), M.frame, x, y, z + dir * 0.01 - dir * 0.008);

    // 4) 窗台：外挑 6cm 的混凝土台 + 下沿一道滴水（水切り）
    if (o.sill !== false) {
      put(gbox(w + fw * 2 + 0.1, 0.05, 0.16), M.slabDark, x, y - h / 2 - fw + 0.01, z + dir * 0.055);
      put(gbox(w + fw * 2 + 0.1, 0.02, 0.03), M.dark, x, y - h / 2 - fw - 0.02, z + dir * 0.125);
    }

    // 5) 半开：一扇窗扇向外推开约 12°，露出黑洞洞的缝。
    //    整栋楼只有两三扇半开，多了就像没关窗。
    if (state === 'ajar') {
      const sashW = w / 2 - 0.04;
      const sash = new THREE.Mesh(new THREE.PlaneGeometry(sashW, h - 0.06), glassMat);
      sash.position.set(x + w * 0.25, y, inZ(0.05));
      sash.rotation.y = dir > 0 ? -0.22 : Math.PI + 0.22;
      sash.translateZ(dir * 0.0);
      sash.position.x += dir * 0.0;
      g.add(sash);
      const edge = new THREE.Mesh(gbox(0.03, h - 0.06, 0.03), M.frame);
      edge.position.set(x + w * 0.25 + dir * 0.02, y, inZ(0.12));
      g.add(edge);
    }
  };

  /* ==========================================================================
   * 带洞口的立面覆板 + 203 式窗套（unitFace）
   * ======================================================================== */

  /** 带洞口的立面覆板：把 x0..x1 × y0..y1 这块墙按洞切成条带（详见 203 段用法） */
  const facadePanel = (
    x0: number, x1: number, y0: number, y1: number, z: number, thick: number,
    mat: THREE.Material, holes: Array<{ a: number; b: number; y0: number; y1: number }>
  ) => {
    const cuts = [...holes].sort((p, q) => p.a - q.a);
    let cx = x0;
    for (const h of cuts) {
      if (h.a > cx) put(gbox(h.a - cx, y1 - y0, thick), mat, (cx + h.a) / 2, (y0 + y1) / 2, z);
      if (h.y0 > y0) put(gbox(h.b - h.a, h.y0 - y0, thick), mat, (h.a + h.b) / 2, (y0 + h.y0) / 2, z);
      if (y1 > h.y1) put(gbox(h.b - h.a, y1 - h.y1, thick), mat, (h.a + h.b) / 2, (h.y1 + y1) / 2, z);
      cx = Math.max(cx, h.b);
    }
    if (x1 > cx) put(gbox(x1 - cx, y1 - y0, thick), mat, (cx + x1) / 2, (y0 + y1) / 2, z);
  };

  /**
   * 把 203 那套"混凝土覆板 + 窗套 + 窗台"原样套到任意一户，
   * 让 203 两侧的所有邻居户、以及 3F/4F 的南/北立面读成同一套外模。
   * dir=+1 南（街，trim=true）/ -1 北（外廊，trim=false，与 203 北面一致）。
   * 覆板比墙外挑 9cm（z+0.09），窗套再外挑 6cm（z+0.15），窗台再外挑 11cm（z+0.20），
   * 与 203 段 1085–1108 的写法完全一致，所以整栋楼从街上看是同一套模。
   */
  const unitFace = (o: {
    ux0: number; ux1: number; F: number;
    holes: Array<{ a: number; b: number; y0: number; y1: number; sill?: boolean }>;
    mat: THREE.Material; z: number; trim?: boolean;
    /**
     * 左右各外扩的量（默认 0）。相邻两户的覆板各自向内缩 4cm，拼起来中间就
     * 留一道 8cm 的竖缝，缝里露出后面的楼体素面。203 是玩家自己那户、外廊
     * 上看得最久，传 0.08 把两侧补满（板边到 ±6.24），与南面 203 覆板同一
     * 口径；邻居户保留留缝，读作"一块块预制板挂上去"的分缝。
     */
    bleed?: number;
  }) => {
    const { ux0, ux1, F, holes, mat, z, trim } = o;
    const y0 = F + GAP, y1 = F + LVL - GAP;
    const bleed = o.bleed ?? 0;
    facadePanel(ux0 + 0.04 - bleed, ux1 - 0.04 + bleed, y0, y1, z, 0.13, mat, holes);
    if (!trim) return;
    const tz = z + 0.06;
    for (const h of holes) {
      const hy0 = Math.max(h.y0, y0), hy1 = Math.min(h.y1, y1);
      const fw = 0.05;
      put(gbox(h.b - h.a + fw * 2, fw, 0.05), M.frame, (h.a + h.b) / 2, hy1 + fw / 2, tz);
      put(gbox(h.b - h.a + fw * 2, fw, 0.05), M.frame, (h.a + h.b) / 2, hy0 - fw / 2, tz);
      put(gbox(fw, hy1 - hy0, 0.05), M.frame, h.a - fw / 2, (hy0 + hy1) / 2, tz);
      put(gbox(fw, hy1 - hy0, 0.05), M.frame, h.b + fw / 2, (hy0 + hy1) / 2, tz);
      if (h.sill !== false && hy0 > F + 0.45) {
        put(gbox(h.b - h.a + 0.14, 0.05, 0.16), M.slabDark, (h.a + h.b) / 2, hy0 - fw + 0.01, z + 0.11);
      }
    }
  };

  /* ==========================================================================
   * 邻居户的窗后室内（不可到达：低模家具，不产生碰撞盒）
   * ======================================================================== */

  /**
   * 一户的低模家具。
   *
   * 摆位照着窗洞走——从街上透过窗看进来，视线基本只扫得到窗洞正后方那一块，
   * 摆在窗间墙背后的家具等于白给。滑門后是客厅（视野最大的一档），左腰窓后
   * 是卧室，右腰窓后是餐厨，高窓后塞个小柜。
   *
   * 每件 1~3 个盒子、几何共享；这些屋不可到达，所以一件都不产生碰撞盒。
   * 所有家具深度压在 0.84m 以内（室盒净深只有 ROOM_D 减去结构板那 0.12），
   * 否则会从玻璃面里穿出来。
   *
   * H = 室高（默认 LVL=2.8；1F 净高 3.36 由调用方传）。
   */
  const furnishUnit = (
    cx: number, F: number, zc: number, seed: number, H: number,
    liv: ZoneState, bed: ZoneState,
    mLiv: RmMats, mBed: RmMats,
    cloth: THREE.Material,
  ) => {
    const r = makeRng(seed);
    const O = (dx: number) => cx + dx;
    const at = (dx: number, dy: number, dz: number) => [O(dx), F + dy, zc + dz] as const;

    /* —— 卧室（西腰窓后）：亮暗独立于客厅 —— */
    const bedX = -4.5 + (r() - 0.5) * 0.5;
    if (bed.furnish) {
      const bedCloth = bed.lit ? cloth : mBed.cloth;   // 熄灯屋分不出床品颜色
      put(gbox(2.0, 0.24, 0.84), mBed.wood, ...at(bedX, 0.12, 0));
      put(gbox(1.94, 0.14, 0.78), bedCloth, ...at(bedX, 0.31, 0));      // 被褥
      put(gbox(0.56, 0.10, 0.26), bedCloth, ...at(bedX - 0.6, 0.42, -0.18));  // 枕头
      if (r() < 0.6) put(gbox(0.40, 0.42, 0.38), mBed.wood, ...at(bedX + 1.2, 0.21, -0.2));  // 床头柜
    }

    /* —— 客厅（玻璃滑門后）：沙发长度 / 摆位 / 茶几 / 柜子每户不同 —— */
    const sofaX = (r() - 0.5) * 0.8;
    const sofaW = 1.5 + r() * 0.6;
    const livCloth = liv.lit ? cloth : mLiv.cloth;
    if (liv.furnish) {
      put(gbox(sofaW, 0.32, 0.78), livCloth, ...at(sofaX, 0.16, -0.05));
      put(gbox(sofaW, 0.40, 0.14), livCloth, ...at(sofaX, 0.50, -0.36));   // 靠背顶着后壁
      if (r() < 0.85) put(gbox(0.9 + r() * 0.3, 0.05, 0.5), mLiv.wood, ...at(sofaX + (r() - 0.5) * 0.4, 0.36, 0.24));  // 茶几
      if (r() < 0.5) {
        put(gbox(1.5, 0.40, 0.40), mLiv.wood, ...at(1.15, 0.20, -0.2));    // 电视柜
      } else {
        put(gbox(1.1, 0.62, 0.36), mLiv.wood, ...at(1.15, 0.31, -0.2));    // 矮边柜
        if (r() < 0.6) put(gbox(0.9, 0.55, 0.05), M.dark, ...at(1.15, 0.66, -0.36));  // 柜上电视
      }
      if (r() < 0.7) put(gbox(2.0 + r() * 0.5, 0.02, 0.8), livCloth, ...at(sofaX, 0.02, 0.05));  // 地毯

      // 餐厨（右腰窓后）：桌长 / 椅数 / 冰箱
      const tblX = 2.6 + (r() - 0.5) * 0.4;
      put(gbox(0.9 + r() * 0.4, 0.05, 0.66), mLiv.wood, ...at(tblX, 0.70, 0));
      const chairAt = (dx: number) => {
        put(gbox(0.38, 0.40, 0.36), mLiv.wood, ...at(dx, 0.20, 0));
        put(gbox(0.38, 0.42, 0.05), mLiv.wood, ...at(dx, 0.60, -0.14));
      };
      const nCh = 1 + Math.floor(r() * 1.99);   // 1~2 把
      chairAt(tblX - 0.55);
      if (nCh > 1) chairAt(tblX + 0.55);
      if (r() < 0.7) put(gbox(0.60, 1.35, 0.58), mLiv.wood, ...at(3.9, 0.68, -0.12));  // 冰箱

      // 高窓后：小柜 + 一盆绿植
      if (r() < 0.8) put(gbox(0.52, 0.9, 0.36), mLiv.wood, ...at(4.9, 0.45, -0.2));
      if (r() < 0.6) {
        put(gbox(0.22, 0.26, 0.22), mLiv.wood, ...at(5.5, 0.13, 0.08));
        put(new THREE.IcosahedronGeometry(0.2, 0), M.leaf, ...at(5.5, 0.44, 0.08));
      }
    }

    /* —— 灯：两区各亮各的，组合随机——有的户只开落地灯，有的全开 —— */
    if (liv.lit) {
      if (r() < 0.75) put(gbox(0.34, 0.06, 0.34), M.warmPale, ...at(sofaX + 0.6, H - 0.35, 0));
      if (r() < 0.55) put(gbox(0.26, 0.30, 0.26), M.warmPale, ...at(sofaX - 1.2, 0.98, -0.25));
      if (r() < 0.3) put(gbox(1.3, 0.05, 0.04), M.warmSoft, ...at(1.15, 0.72, -0.37));   // 电视柜背光条
    }
    if (bed.lit) {
      if (r() < 0.7) put(gbox(0.30, 0.05, 0.30), M.warmPale, ...at(bedX, H - 0.35, 0));
      if (r() < 0.45) put(gbox(0.16, 0.20, 0.16), M.warmSoft, ...at(bedX + 1.2, 0.66, -0.2));  // 床头灯
    }
  };

  /**
   * 1F 一户的低模家具。
   *
   * 与楼上不同：1F 每户只在正中开一扇 1.15m 宽的窗（窗台压到 0.53m，
   * 一半还带防盗格栅），从街上能扫到的横向范围就是窗后那三四米。
   * 所以家具只摆窗后这一段——摆在窗间墙背后的等于白给，不如省下来。
   * 单间户型：全屋一个亮暗档（bed 区参数忽略）。
   */
  const furnishGroundUnit = (
    cx: number, F: number, zc: number, seed: number, _H: number,
    liv: ZoneState, _bed: ZoneState,
    m: RmMats, _mBed: RmMats,
    cloth: THREE.Material,
  ) => {
    void _bed; void _mBed; void _H;
    const r = makeRng(seed);
    const O = (dx: number) => cx + dx;
    const at = (dx: number, dy: number, dz: number) => [O(dx), F + dy, zc + dz] as const;
    const lc = liv.lit ? cloth : m.cloth;
    const sofaX = (r() - 0.5) * 0.5;

    // 客厅：沙发 + 茶几 + 电视柜（电视柜正对窗，从街上第一眼看到的是它）
    put(gbox(1.4 + r() * 0.6, 0.30, 0.74), lc, ...at(sofaX, 0.15, 0.18));
    put(gbox(1.4 + r() * 0.6, 0.42, 0.14), lc, ...at(sofaX, 0.48, -0.19));
    put(gbox(0.8 + r() * 0.3, 0.05, 0.46), m.wood, ...at(sofaX, 0.34, 0.2));   // 茶几
    if (r() < 0.5) {
      put(gbox(1.5, 0.34, 0.42), m.wood, ...at(0, 0.17, -0.28));   // 电视柜贴后壁
    } else {
      put(gbox(1.1, 0.6, 0.38), m.wood, ...at(0, 0.30, -0.26));    // 矮边柜
      if (r() < 0.6) put(gbox(0.85, 0.5, 0.05), M.dark, ...at(0, 0.63, -0.43));
    }
    if (r() < 0.7) put(gbox(1.9 + r() * 0.4, 0.02, 0.76), lc, ...at(sofaX, 0.02, 0.06));
    // 窗侧一角：书架或餐柜，二选一
    if (r() < 0.5) {
      put(gbox(0.5, 1.5, 0.34), m.wood, ...at(1.65, 0.75, -0.26));
    } else {
      put(gbox(1.0, 0.85, 0.40), m.wood, ...at(1.6, 0.43, -0.24));
      put(gbox(0.26, 0.3, 0.26), lc, ...at(1.6, 1.0, -0.24));     // 柜上摆件
    }
    if (r() < 0.55) put(new THREE.IcosahedronGeometry(0.22, 0), M.leaf, ...at(-1.7, 0.34, 0.1));
    if (liv.lit) {
      if (r() < 0.75) put(gbox(0.34, 0.06, 0.34), M.warmPale, ...at(0, 2.86, 0));      // 吸顶灯（1F 净高 3.36）
      if (r() < 0.5) put(gbox(0.22, 0.34, 0.22), M.warmPale, ...at(sofaX - 1.3, 1.05, -0.1));  // 落地灯
    }
  };

  /**
   * 一间能看进去的屋子：结构板 + 室盒内衬 + 低模家具。
   *
   * 上方住宅层原本是实心体量（内部没有空间）。实心块南端已退 ROOM_D 让出
   * 空腔，这里把它封成一间屋——结构板与 unitFace 用同一套洞口，但落在
   * z=ZF 这条外皮线上（窗洞位置留空，视线从这里进屋）；内衬给到后壁 / 顶 /
   * 底 / 分户隔板，法线朝屋内；家具贴后壁摆。
   * 外轮廓仍收在 z=ZF，楼的体积分毫不变，碰墙盒自然也不用跟着动。
   */
  const unitInterior = (o: {
    ux0: number; ux1: number; F: number;
    holes: Array<{ a: number; b: number; y0: number; y1: number }>;
    lit: boolean; seed: number;
    /** 卧室区亮暗（默认同客厅）。西腰窓后是卧室，可以和客厅不同步亮灯。 */
    bedLit?: boolean;
    /** 客厅区是否建家具（滑門拉帘时 false，帘后不建模） */
    livFurnish?: boolean;
    /** 卧室区是否建家具（西腰窓拉帘时 false） */
    bedFurnish?: boolean;
    /** 亮灯户的布面色（默认柔和绿；每户沙发/床品二选一，见 rmClothAlt） */
    cloth?: THREE.Material;
    /** 室高（默认 LVL=2.8；1F 净高 3.36 由调用方传） */
    h?: number;
    /** 结构板材质（默认 M.wall；1F 用基座色 M.wallBase 与体量同调） */
    facade?: THREE.Material;
    /** 家具生成器（默认标准户型；1F 只摆窗后一段） */
    furnish?: (
      cx: number, F: number, zc: number, seed: number, H: number,
      liv: ZoneState, bed: ZoneState,
      mLiv: RmMats, mBed: RmMats, cloth: THREE.Material,
    ) => void;
  }) => {
    const { ux0, ux1, F, holes, lit } = o;
    const H = o.h ?? LVL;
    const y0 = F + GAP, y1 = F + H - GAP;
    const uw = ux1 - ux0;
    const cx = (ux0 + ux1) / 2;
    const livLit = lit;
    const bedLit = o.bedLit ?? livLit;
    const mLiv = livLit ? RM.lit : RM.dark;
    const mBed = bedLit ? RM.lit : RM.dark;
    const cloth = o.cloth ?? M.rmClothLit;
    const zBack = ZF - ROOM_D;            // 空腔后壁 = 退让后实心块的南表面
    const zMid = ZF - ROOM_D / 2;
    const zCf = ZF - ROOM_D / 2 - 0.008;   // 天花板/地板中心微量后退，避免后缘与结构板前缘（ZF）共面 z-fighting
    // 卧室 / 客厅分界：落在西腰窓东缘（cx-3.72）与滑門西缘（cx-1.38）之间，
    // 卧室窗看到的是卧室那一半，滑門看到的是客厅那一半。
    const xDiv = cx - 3.4;

    // 1) 结构板：和外皮同一套洞口，厚 0.12，南面正好落在 z=ZF 上
    facadePanel(ux0 + 0.04, ux1 - 0.04, y0, y1, ZF - 0.06, 0.12, o.facade ?? M.wall, holes);

    // 2) 内衬：后壁 / 顶 / 底，各按卧室 / 客厅两段建，两段各用各的亮暗档。
    const bw = xDiv - (ux0 + 0.04);       // 卧室段宽
    const lw = (ux1 - 0.04) - xDiv;       // 客厅段宽
    const bedCx = (ux0 + 0.04 + xDiv) / 2;
    const livCx = (xDiv + ux1 - 0.04) / 2;
    put(gbox(bw, H - GAP * 2, 0.06), mBed.wall, bedCx, F + H / 2, zBack + 0.03);
    put(gbox(lw, H - GAP * 2, 0.06), mLiv.wall, livCx, F + H / 2, zBack + 0.03);
    put(gbox(bw, 0.06, ROOM_D), mBed.wall, bedCx, y1 - 0.03, zCf);    // 卧室顶
    put(gbox(lw, 0.06, ROOM_D), mLiv.wall, livCx, y1 - 0.03, zCf);    // 客厅顶
    put(gbox(bw, 0.06, ROOM_D), mBed.floor, bedCx, y0 + 0.03, zCf);   // 卧室地
    put(gbox(lw, 0.06, ROOM_D), mLiv.floor, livCx, y0 + 0.03, zCf);   // 客厅地

    // 2b) 卧室与客厅之间的隔墙：两片薄板各朝各面（gbox 六面同色，卧室亮客厅暗
    //     时总有一面颜色不对）。朝东贴客厅墙色，朝西贴卧室墙色。
    {
      const ih = H - GAP * 2;
      const mkWall = (mat: THREE.Material, face: 1 | -1) => {
        const geo = new THREE.PlaneGeometry(ROOM_D - 0.12, ih);
        geo.rotateY(face * Math.PI / 2);
        const wm = new THREE.Mesh(geo, mat);
        wm.position.set(xDiv + face * 0.005, F + GAP + ih / 2, zMid);
        g.add(wm);
      };
      mkWall(mLiv.wall, 1);
      mkWall(mBed.wall, -1);
    }

    // 3) 分户隔板：只画西侧那一道，东侧由邻户自己画（避免同一位置叠两片）；
    //    最西端 ux0 === X0 处是山墙，不用补。西侧隔板紧邻卧室，用卧室档。
    if (ux0 !== X0) {
      put(gbox(0.06, H - GAP * 2, ROOM_D), mBed.wall, ux0 + 0.03, F + H / 2, zMid);
    }

    // 4) 家具：中心略偏北，给玻璃面留出身位
    //    底面抬 GAP，与楼板 shell 同一套空气缝：不然楼上单元（如 203 正上方的 303）
    //    家具底面落在 F（= 下层天花标高），和 203 天花精确共面 → z-fighting。
    (o.furnish ?? furnishUnit)(cx, F + GAP, zMid - 0.06, o.seed, H,
      { lit: livLit, furnish: o.livFurnish ?? true },
      { lit: bedLit, furnish: o.bedFurnish ?? true },
      mLiv, mBed, cloth);
  };

  /**
   * 一户北端（外廊侧）的低模家具。
   *
   * 与南面 furnishUnit 是两套内容：北面窗后按 203 的户型是「卧室 / 洗面所 /
   * 浴室 / 玄関」这一排小间，不是客厅。家具贴后壁（zBack 侧）摆——外廊上的人
   * 是斜着往里看的，摆在窗洞正对面的才扫得到。
   *
   * 件深压在 0.62m 以内：北端空腔净深只有 ROOM_D 减去结构板那 0.12，家具从
   * 玻璃面里戳出来就穿帮了。
   */
  const furnishNorth = (
    cx: number, F: number, zc: number, seed: number, H: number,
    liv: ZoneState, bed: ZoneState,
    mLiv: RmMats, mBed: RmMats,
    cloth: THREE.Material,
  ) => {
    const r = makeRng(seed);
    const O = (dx: number) => cx + dx;
    // zc 是贴后壁的基准线，-dz 表示往窗（北）侧挪
    const at = (dx: number, dy: number, dz: number) => [O(dx), F + dy, zc - dz] as const;

    /* —— 卧室（大窗后，x≈-4.7）：床横过来放，床头床头柜各一件 ——
     * 大窗窗台只有 F+0.9，从外廊平视就能扫到床面和床头柜，这是三扇窗里
     * 唯一能真正「看进屋」的一扇，家具给足。 */
    if (bed.furnish) {
      const bedCloth = bed.lit ? cloth : mBed.cloth;
      put(gbox(1.94, 0.24, 0.78), mBed.wood, ...at(-4.7, 0.12, 0.16));      // 床箱
      put(gbox(1.88, 0.14, 0.72), bedCloth, ...at(-4.7, 0.31, 0.16));        // 被褥
      put(gbox(0.52, 0.10, 0.26), bedCloth, ...at(-5.3, 0.42, 0.26));        // 枕头
      put(gbox(0.40, 0.42, 0.36), mBed.wood, ...at(-3.5, 0.21, 0.28));       // 床头柜
    }

    /* —— 洗面所（小窗后，x≈-0.1）：洗面台 + 镜柜 ——
     * 小窗窗台 F+1.6，只有镜柜上半截和镜前灯够得着视线，台面本身是给
     * 「亮灯时瓷砖反光」作底的。 */
    if (liv.furnish) {
      put(gbox(0.78, 0.80, 0.52), mLiv.wood, ...at(-0.1, 0.40, 0.20));      // 洗面台柜体
      put(gbox(0.82, 0.06, 0.56), M.warmPale, ...at(-0.1, 0.83, 0.20));      // 台面（白瓷）
      put(gbox(0.70, 0.62, 0.06), M.dark, ...at(-0.1, 1.34, 0.40));          // 镜柜贴后壁
      // 洗衣机：洗面所的常客，一半的户有
      if (r() < 0.5) put(gbox(0.60, 0.85, 0.56), M.warmPale, ...at(0.72, 0.43, 0.16));

      /* —— 浴室（小窗后，x≈2.2）：浴缸 + 浴帘 —— */
      put(gbox(1.10, 0.55, 0.62), M.warmPale, ...at(2.2, 0.28, 0.16));       // 浴缸
      put(gbox(1.10, 0.04, 0.62), M.metal, ...at(2.2, 0.57, 0.16));          // 缸沿
      put(gbox(0.64, 0.03, 0.03), M.metal, ...at(2.2, 2.22, 0.16));          // 浴帘杆
      put(gbox(0.66, 1.50, 0.04), liv.lit ? cloth : mLiv.cloth, ...at(2.2, 1.45, 0.42)); // 浴帘

      /* —— 玄関（门后，x≈4.95）：门扇是实心看不到，只给鞋柜一个底 —— */
      put(gbox(0.90, 0.90, 0.36), mLiv.wood, ...at(4.95, 0.45, 0.22));
    }

    /* —— 灯：卧室与卫浴各亮各的 ——
     * 卫浴那盏常亮一小盏：夜里回家先开的是这一盏，是「有人住着」最便宜的证据。 */
    if (bed.lit) {
      if (r() < 0.7) put(gbox(0.30, 0.05, 0.30), M.warmPale, ...at(-4.7, H - 0.35, 0.16));
      if (r() < 0.45) put(gbox(0.15, 0.19, 0.15), M.warmSoft, ...at(-3.5, 0.63, 0.28));
    }
    if (liv.lit) {
      put(gbox(0.26, 0.05, 0.26), M.warmPale, ...at(1.0, H - 0.35, 0.16));
      if (r() < 0.6) put(gbox(0.34, 0.03, 0.05), M.warmSoft, ...at(-0.1, 1.70, 0.40));   // 镜前灯
    }
  };

  /**
   * 一间北端（外廊侧）能看进去的屋子：结构板 + 室盒内衬 + 低模家具。
   *
   * 与 unitInterior 是镜像的两套：那边在 z=ZF 朝街，这边在 z=ZN 朝外廊。
   * 住宅层实心体量北端同样退了 ROOM_D（见 DEPTHB2），这里把外皮补回 z=ZN
   * 并封成屋子——与南面同一套做法，只是里头摆的是洗面所 / 浴室 / 卧室，
   * 不是客厅。
   *
   * 洞口必须与同户北立面 unitFace 的洞口完全一致（都取 203 北墙 openings
   * 的户内相对坐标），否则覆板的洞和窗后的屋子对不上，外廊看进来就还是
   * 「窗后一片素面」。
   */
  const unitInteriorNorth = (o: {
    ux0: number; ux1: number; F: number;
    holes: Array<{ a: number; b: number; y0: number; y1: number }>;
    lit: boolean; seed: number;
    /** 卧室区亮暗（默认同卫浴区）。大窗后是卧室，可以和卫浴不同步亮灯。 */
    bedLit?: boolean;
    /** 卫浴区是否建家具（拉帘 / 磨砂的户不建） */
    livFurnish?: boolean;
    /** 卧室区是否建家具（拉帘的户不建） */
    bedFurnish?: boolean;
    /** 亮灯户的布面色 */
    cloth?: THREE.Material;
    h?: number;
  }) => {
    const { ux0, ux1, F, holes, lit } = o;
    const H = o.h ?? LVL;
    const y0 = F + GAP, y1 = F + H - GAP;
    const uw = ux1 - ux0;
    const cx = (ux0 + ux1) / 2;
    const livLit = lit;
    const bedLit = o.bedLit ?? livLit;
    const mLiv = livLit ? RM.lit : RM.dark;
    const mBed = bedLit ? RM.lit : RM.dark;
    const cloth = o.cloth ?? M.rmClothLit;
    const zBack = ZN + ROOM_D;            // 空腔后壁 = 退让后实心块的北表面
    const zMid = ZN + ROOM_D / 2;
    const zCf = ZN + ROOM_D / 2 + 0.008;   // 天花板/地板中心微量前移，避免前缘与结构板后缘（ZN）共面 z-fighting

    // 1) 结构板：外表面正好落在 z=ZN 上（与 unitFace 的覆板留 2.5cm 空气缝）
    facadePanel(ux0 + 0.04, ux1 - 0.04, y0, y1, ZN + 0.06, 0.12, M.wall, holes);

    // 2) 内衬：后壁 / 顶 / 地。北端这一段按 203 户型横跨卧室到玄関，整段一个
    //    亮暗档——分区（客厅/卧室两档）是南面那套，湿区不分区。
    put(gbox(uw - 0.08, H - GAP * 2, 0.06), mLiv.wall, cx, F + H / 2, zBack - 0.03);
    put(gbox(uw - 0.08, 0.06, ROOM_D), mLiv.wall, cx, y1 - 0.03, zCf);    // 顶
    put(gbox(uw - 0.08, 0.06, ROOM_D), mLiv.floor, cx, y0 + 0.03, zCf);   // 地

    // 3) 分户隔板：只画西侧那一道，东侧由邻户自己画；最西端 ux0 === X0 是山墙
    if (ux0 !== X0) {
      put(gbox(0.06, H - GAP * 2, ROOM_D), mBed.wall, ux0 + 0.03, F + H / 2, zMid);
    }

    // 4) 家具：贴后壁摆（底面抬 GAP，与南面同套空气缝，避开与楼板的共面闪面）
    furnishNorth(cx, F + GAP, zBack - 0.09, o.seed, H,
      { lit: livLit, furnish: o.livFurnish ?? true },
      { lit: bedLit, furnish: o.bedFurnish ?? true },
      mLiv, mBed, cloth);
  };

  /* ==========================================================================
   * 阳台：真正挑出墙外的结构
   * ======================================================================== */

  /**
   * 一段阳台栏杆。
   *
   * 与 203 自己那圈栏杆（props.ts buildRoomShell）完全同规格：高 1.05、
   * 立柱 0.055m、竖栅 0.018m / 间距 0.115m。整层楼的阳台从街上看是
   * 同一道密栅栏，而不是 203 一段 + 邻居一段两种节奏——那正是用户截图里
   * 「明显差异」的主因。
   */
  const railing = (x0: number, x1: number, F: number, z: number) => {
    const rg = new THREE.Group();
    const add = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, zz: number) => {
      const m = new THREE.Mesh(geo, mat);
      m.position.set(x, y, zz);
      rg.add(m);
      return m;
    };
    const len = x1 - x0;
    const cx = (x0 + x1) / 2;
    const RH = 1.05;
    // 上下横档
    add(gbox(len, 0.05, 0.07), M.railTop, cx, F + RH, z);
    add(gbox(len, 0.04, 0.05), M.railTop, cx, F + 0.08, z);
    // 立柱
    const nPost = Math.max(2, Math.round(len / 0.55) + 1);
    for (let i = 0; i < nPost; i++) {
      const px = x0 + (len * i) / (nPost - 1);
      add(gbox(0.055, RH, 0.055), M.railing, px, F + RH / 2, z);
    }
    // 竖栅：与 203 完全同规格（间距 0.115 / 杆径 0.018 / 杆高 RH-0.1），
    // 共享几何一次创建、整层楼几百根复用——合批后仍然是一个 draw call。
    for (let px = x0 + 0.14; px < x1 - 0.05; px += 0.115) {
      add(gbox(0.018, RH - 0.1, 0.018), M.railing, px, F + 0.06 + (RH - 0.1) / 2, z);
    }
    rg.name = `railing-${x0.toFixed(1)}_${z.toFixed(1)}`;
    g.add(rg);
    // 整段只声明一个盒（栏杆是临空边的屏障，不是几十根杆各自的障碍）：
    // 立柱底在 F、顶横档上沿 F+1.075，取 [F, F+1.10]；横档厚 0.07，取 ±0.06。
    boxes.push(boxSpec(`${rg.name}@${F.toFixed(1)}`, x0 - 0.03, x1 + 0.03, F, F + 1.1, z - 0.06, z + 0.06));
  };

  /** 空调室外机：机身 + 风扇 + 支架 + 两根配管（接回墙里） */
  const acUnit = (x: number, y: number, z: number, dir: 1 | -1) => {
    put(gbox(0.72, 0.52, 0.30), M.metal, x, y + 0.30, z);
    // 风扇：正面一个深灰圆盘 + 更亮的扇叶圈
    const fan = new THREE.Mesh(new THREE.CircleGeometry(0.15, 14), toon('#333a44'));
    fan.position.set(x, y + 0.32, z + dir * 0.152);
    if (dir < 0) fan.rotation.y = Math.PI;
    g.add(fan);
    // 支架：机器不是浮在地上的
    put(gbox(0.08, 0.16, 0.08), M.metal, x - 0.26, y + 0.08, z - dir * 0.06);
    put(gbox(0.08, 0.16, 0.08), M.metal, x + 0.26, y + 0.08, z - dir * 0.06);
    // 配管：两根细管从机器背后斜上接入墙内
    for (const dx of [-0.18, -0.08]) {
      const pipe = new THREE.Mesh(new THREE.CylinderGeometry(0.022, 0.022, 0.55, 6), M.pipe);
      pipe.position.set(x + dx, y + 0.42, z - dir * 0.22);
      pipe.rotation.x = dir * 0.5;
      g.add(pipe);
    }
  };

  /** 阳台盆栽：陶盆 + 一团低面叶簇（不是球） */
  const potted = (x: number, y: number, z: number, seed: number) => {
    put(new THREE.CylinderGeometry(0.105, 0.085, 0.16, 8), toon('#8d7159'), x, y + 0.08, z);
    const leaf = new THREE.Mesh(new THREE.IcosahedronGeometry(0.16, 0), M.leaf);
    leaf.scale.set(1, 0.85, 1);
    leaf.position.set(x, y + 0.27, z);
    leaf.rotation.y = makeRng(seed)() * Math.PI;
    g.add(leaf);
  };

  /** 折叠晾衣架 + 搭在上面的衣物 */
  const dryingRack = (x: number, y: number, z: number, seed: number) => {
    const r = makeRng(seed);
    for (const sx of [-1, 1]) {
      const leg = new THREE.Mesh(new THREE.CylinderGeometry(0.016, 0.02, 1.05, 6), M.metal);
      leg.position.set(x + sx * 0.42, y + 0.52, z);
      leg.rotation.z = sx * 0.14;
      g.add(leg);
    }
    put(gbox(0.94, 0.024, 0.024), M.metal, x, y + 1.03, z);
    put(gbox(0.90, 0.02, 0.02), M.metal, x, y + 0.72, z);
    // 衣物：两三件低饱和色小布片，各自错开高度
    const n = 1 + Math.floor(r() * 3);
    for (let i = 0; i < n; i++) {
      const cw = 0.26 + r() * 0.16;
      const ch = 0.36 + r() * 0.30;
      panel(cw, ch, toon(EXT.cloth[Math.floor(r() * EXT.cloth.length)]),
        x - 0.34 + i * 0.34 + (r() - 0.5) * 0.1, y + 1.03 - ch / 2, z + 0.03, 1);
    }
  };

  /** 塑料收纳箱 */
  const storageBox = (x: number, y: number, z: number, c: string) => {
    put(gbox(0.46, 0.32, 0.36), toon(c), x, y + 0.16, z);
    put(gbox(0.48, 0.04, 0.38), toon('#6f7885'), x, y + 0.34, z);
  };

  /** 折叠椅 + 小桌：阳台上"这户在这儿坐过"的证据 */
  const balconyChair = (x: number, y: number, z: number) => {
    put(gbox(0.40, 0.03, 0.38), M.wood, x, y + 0.42, z);           // 座面
    put(gbox(0.40, 0.44, 0.03), M.wood, x, y + 0.64, z - 0.17);    // 靠背
    for (const [dx, dz] of [[-0.16, -0.15], [0.16, -0.15], [-0.16, 0.15], [0.16, 0.15]] as Array<[number, number]>) {
      put(gbox(0.025, 0.42, 0.025), M.metal, x + dx, y + 0.21, z + dz);
    }
  };

  /** 折叠桌：和 balconyChair 配套，让那一户读作"在这儿吃过饭"而不是"堆了把椅子" */
  const balconyTable = (x: number, y: number, z: number) => {
    put(gbox(0.62, 0.03, 0.44), M.wood, x, y + 0.40, z);
    put(gbox(0.14, 0.36, 0.03), M.wood, x, y + 0.20, z - 0.16);
    put(gbox(0.14, 0.36, 0.03), M.wood, x, y + 0.20, z + 0.16);
    put(gbox(0.48, 0.025, 0.025), M.metal, x, y + 0.09, z);
  };

  /**
   * 长条花箱：防腐木箱 + 土面 + 沿长度排开的几丛叶簇。
   *
   * 203 自家阳台的两个花箱是这条立面上唯一有"体量"的绿植，邻居原先只有巴掌
   * 大的陶盆，在大阳台上根本撑不起来。花箱是补齐体量感最省的一件：一个 1m
   * 长的木箱在剪影上的分量，顶得上七八个陶盆。
   */
  const planterBox = (x: number, y: number, z: number, len: number, seed: number) => {
    const r = makeRng(seed);
    put(gbox(len, 0.30, 0.34), M.wood, x, y + 0.15, z);
    put(gbox(len + 0.04, 0.045, 0.38), M.woodDeep, x, y + 0.315, z);   // 上沿压条
    put(gbox(len - 0.07, 0.04, 0.29), M.woodDeep, x, y + 0.275, z);    // 土面：用深一档的木色读"泥炭"
    const n = Math.max(2, Math.round(len / 0.26));
    for (let i = 0; i < n; i++) {
      const rad = 0.12 + r() * 0.06;
      // 叶簇只取 M.leaf 一个桶：外景合批后 draw call 贴着上限，多一种叶色就多
      // 一个桶。层次靠每丛的半径 / 扁度 / 转角错出来，不靠色相。
      const leaf = new THREE.Mesh(new THREE.IcosahedronGeometry(rad, 0), M.leaf);
      leaf.scale.set(1, 0.8 + r() * 0.35, 1);
      leaf.position.set(
        x - len / 2 + len * ((i + 0.5) / n) + (r() - 0.5) * 0.05,
        y + 0.30 + rad * 0.85,
        z + (r() - 0.5) * 0.09
      );
      leaf.rotation.y = r() * Math.PI;
      g.add(leaf);
    }
  };

  /** 木花架：两层，上层摆三只小盆——给阳台一点竖向层次，不然东西全摊在地上。 */
  const plantStand = (x: number, y: number, z: number, seed: number) => {
    const r = makeRng(seed);
    put(gbox(0.78, 0.03, 0.30), M.wood, x, y + 0.44, z);
    put(gbox(0.78, 0.03, 0.30), M.wood, x, y + 0.16, z);
    for (const sx of [-1, 1]) {
      for (const dz of [-0.12, 0.12]) put(gbox(0.03, 0.46, 0.03), M.wood, x + sx * 0.36, y + 0.23, z + dz);
    }
    for (let i = 0; i < 3; i++) {
      const lx = x - 0.25 + i * 0.25;
      put(new THREE.CylinderGeometry(0.07, 0.058, 0.12, 8), M.woodDeep, lx, y + 0.56, z);
      const leaf = new THREE.Mesh(new THREE.IcosahedronGeometry(0.095 + r() * 0.04, 0), M.leaf);
      leaf.position.set(lx, y + 0.66, z);
      leaf.rotation.y = r() * Math.PI;
      g.add(leaf);
    }
  };

  /**
   * 洗衣机：很多日本户型把洗衣机位放在阳台，这是阳台最有说服力的一件"家电"。
   * 机壳用 slabDry 而不是纯白——夜景里纯白会跳出立面，浅冷灰反而像被打湿的塑料。
   */
  const washer = (x: number, y: number, z: number) => {
    put(gbox(0.62, 0.84, 0.60), M.slabDry, x, y + 0.44, z);
    put(gbox(0.64, 0.05, 0.62), M.metal, x, y + 0.885, z);          // 顶盖
    panel(0.36, 0.36, M.dark, x, y + 0.50, z + 0.311, 1);           // 舷窗（贴片，省一个厚度）
    put(gbox(0.05, 0.34, 0.05), M.pipe, x - 0.30, y + 1.00, z - 0.28);  // 水龙头立管
    put(gbox(0.17, 0.05, 0.05), M.pipe, x - 0.225, y + 1.16, z - 0.28); // 弯头
  };

  /**
   * 目隠しシート：挂在栏杆内侧的布围挡。
   *
   * 日本阳台最常见的"我在这里住"的招牌，也是唯一能把 12m 长的栏杆切成几段、
   * 不至于一条通长到底的东西。用薄 box 而不是 PlaneGeometry——单面板从背面
   * 会被剔除，从 203 阳台斜看邻居时它会整片消失。
   */
  const blindSheet = (x0: number, x1: number, y: number, z: number, mat: THREE.Material) => {
    const len = x1 - x0, cx = (x0 + x1) / 2;
    put(gbox(len, 0.72, 0.012), mat, cx, y + 0.46, z);
    put(gbox(len, 0.022, 0.022), M.pipe, cx, y + 0.83, z - 0.012);   // 上沿绳
    put(gbox(len, 0.022, 0.022), M.pipe, cx, y + 0.09, z - 0.012);   // 下沿绳
    for (let px = x0 + 0.6; px < x1 - 0.2; px += 1.2) {
      put(gbox(0.028, 0.80, 0.028), M.pipe, px, y + 0.46, z + 0.016);  // 绑扎杆
    }
  };

  /** 水桶 + 浇水壶：浇花那套。小件，但堆在花箱旁边立刻读出"有人打理"。 */
  const wateringKit = (x: number, y: number, z: number) => {
    put(new THREE.CylinderGeometry(0.11, 0.09, 0.20, 10), M.slabDark, x, y + 0.10, z);
    put(new THREE.CylinderGeometry(0.115, 0.115, 0.025, 10), M.pipe, x, y + 0.202, z);
    put(new THREE.CylinderGeometry(0.075, 0.085, 0.16, 10), M.metal, x + 0.22, y + 0.08, z);
    put(gbox(0.18, 0.028, 0.028), M.metal, x + 0.38, y + 0.135, z);
  };

  /** 阳台壁灯：夜景里每户一点暖光，整条立面才有"住着人"的层次。
   *  发光面走已有的 warmSoft 桶，不新增材质。 */
  const balconyLamp = (x: number, y: number, z: number) => {
    put(gbox(0.09, 0.10, 0.07), M.metal, x, y, z);
    put(gbox(0.16, 0.09, 0.11), M.metal, x, y + 0.09, z);
    flatDown(0.12, 0.07, M.warmSoft, x, y + 0.045, z + 0.02);
  };

  /** 竖雨水管（竪樋）：贴着戸境板一路下到地面 */
  const downpipe = (x: number, z: number, top: number) => {
    const pipe = new THREE.Mesh(new THREE.CylinderGeometry(0.05, 0.05, top - 0.25, 8), M.pipe);
    pipe.position.set(x, (top - 0.25) / 2 + 0.25, z);
    g.add(pipe);
    // 每层一个接头（排水管不是一根到底的光管）
    for (let y = 1.2; y < top; y += 2.8) {
      put(gbox(0.13, 0.07, 0.13), M.pipe, x, y, z);
    }
  };

  /**
   * 一段钢栏杆（外楼梯 / 平台 / 外廊尽端共用）。
   *
   * 从 (x0,y0,z0) 到 (x1,y1,z1)，可以带坡度（沿梯跑扶手）。构造与外廊
   * 竖栅同一语言：炭灰立柱 + 0.15m 间距竖栅 + 顶扶手 + 中横档——整栋楼
   * 的栏杆读作一套模，只是楼梯这套更"钢"。
   * 基点 (y0/y1) 是落脚面（踏面 / 平台面），杆件从 +0.05 起算。
   */
  const railRun = (x0: number, y0: number, z0: number, x1: number, y1: number, z1: number) => {
    const dx = x1 - x0, dy = y1 - y0, dz = z1 - z0;
    const horiz = Math.hypot(dx, dz);
    const len = Math.hypot(horiz, dy);
    if (horiz < 0.02) return;
    const RH = 0.98;
    const dir = new THREE.Vector3(dx, dy, dz).normalize();
    // 斜杆（顶扶手 / 中横档）：四元数对准段方向，最稳
    const bar = (h: number, mat: THREE.Material, s = 0.05) => {
      const m = new THREE.Mesh(gbox(len + 0.06, s, s), mat);
      m.position.set((x0 + x1) / 2, (y0 + y1) / 2 + 0.05 + h, (z0 + z1) / 2);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(1, 0, 0), dir);
      g.add(m);
    };
    bar(RH, M.railTop);
    bar(0.45, M.railTop, 0.04);
    // 立柱：段端必有一根，中间每 ~1.4m 补
    const nPost = Math.max(2, Math.round(horiz / 1.4) + 1);
    for (let i = 0; i < nPost; i++) {
      const t = i / (nPost - 1);
      put(gbox(0.05, RH, 0.05), M.stairSteel, x0 + dx * t, y0 + dy * t + 0.05 + RH / 2, z0 + dz * t);
    }
    // 竖栅：0.15m 一根，与外廊 0.115 的密栅同一节奏略放宽（钢梯更疏）
    const nBar = Math.max(1, Math.round(horiz / 0.15));
    for (let i = 0; i <= nBar; i++) {
      const t = i / nBar;
      put(gbox(0.018, RH - 0.12, 0.018), M.railing, x0 + dx * t, y0 + dy * t + 0.05 + (RH - 0.12) / 2, z0 + dz * t);
    }
    // 整段一个盒：立柱 0.05、斜杆半厚 0.025 → 两侧各留 0.05；杆件从基点 +0.05 起、
    // 顶扶手上沿在 +0.05+RH+0.025。斜跑段（y0≠y1）取两端包络，横向很薄、不会侵入踏步。
    boxes.push(boxSpec(
      `rail-${x0.toFixed(2)}_${y0.toFixed(2)}_${z0.toFixed(2)}→${x1.toFixed(2)}_${z1.toFixed(2)}`,
      Math.min(x0, x1) - 0.05, Math.max(x0, x1) + 0.05,
      Math.min(y0, y1), Math.max(y0, y1) + 0.05 + RH + 0.03,
      Math.min(z0, z1) - 0.05, Math.max(z0, z1) + 0.05
    ));
  };

  /** 雨夜积水一小片（车棚下 / 楼梯基座 / 垃圾房旁） */
  const puddle = (x: number, z: number, r: number) => {
    const geo = new THREE.CircleGeometry(r, 12);
    geo.rotateX(-Math.PI / 2);
    const m = new THREE.Mesh(geo, toon('#333d49', { finish: 'wet' }));
    m.position.set(x, 0.009, z);
    m.scale.set(1, 1, 0.62);
    m.userData.noOutline = true;
    g.add(m);
  };

  /** 一辆自行车：真车架（五通/前叉/座管/车把/车筐/脚撑），不是两个轮子一根棍 */
  const bikeFrameMats = ['#566070', '#4b5058', '#6d675c'].map((c) => toon(c, { finish: 'metal' }));
  const BIKE_WHEEL = new THREE.TorusGeometry(0.265, 0.026, 6, 14);
  const bike = (x: number, z: number, rot: number, seed: number, basket = false) => {
    const b = new THREE.Group();
    const rr = makeRng(seed);
    const fm = bikeFrameMats[Math.floor(rr() * bikeFrameMats.length)];
    const tube = (x1: number, y1: number, z1: number, x2: number, y2: number, z2: number, r = 0.02, mat = fm) => {
      const dx = x2 - x1, dy = y2 - y1, dz = z2 - z1;
      const m = new THREE.Mesh(new THREE.CylinderGeometry(r, r, Math.hypot(dx, dy, dz), 5), mat);
      m.position.set((x1 + x2) / 2, (y1 + y2) / 2, (z1 + z2) / 2);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), new THREE.Vector3(dx, dy, dz).normalize());
      b.add(m);
    };
    // 车轮（Torus 平面 XY、轴向 Z——车头朝 +X 时正好立着）
    for (const wx of [-0.42, 0.42]) {
      const w = new THREE.Mesh(BIKE_WHEEL, M.metal);
      w.position.set(wx, 0.265, 0);
      b.add(w);
    }
    // 车架管线
    tube(-0.42, 0.265, 0, 0, 0.265, 0);        // 链叉
    tube(-0.42, 0.265, 0, 0.06, 0.78, 0);      // 座叉
    tube(0, 0.265, 0, 0.06, 0.78, 0);          // 座管
    tube(0, 0.265, 0, 0.4, 0.55, 0);           // 下管
    tube(0.06, 0.78, 0, 0.38, 0.72, 0);        // 上管
    tube(0.4, 0.72, 0, 0.42, 0.265, 0);        // 前叉
    tube(0.38, 0.74, -0.17, 0.38, 0.74, 0.17, 0.016);   // 车把
    tube(0.03, 0.78, 0, 0.03, 0.84, 0, 0.014);          // 鞍座立杆
    tube(0.05, 0.3, 0, -0.1, 0.02, 0.1, 0.011);         // 脚撑
    const saddle = new THREE.Mesh(gbox(0.23, 0.05, 0.09), M.dark);
    saddle.position.set(0.03, 0.86, 0);
    b.add(saddle);
    if (basket) {
      const bk = new THREE.Mesh(gbox(0.32, 0.2, 0.26), M.metal);
      bk.position.set(0.56, 0.66, 0);
      b.add(bk);
      for (let i = 0; i < 3; i++) {
        const bar = new THREE.Mesh(gbox(0.012, 0.17, 0.012), M.dark);
        bar.position.set(0.725, 0.66, -0.08 + i * 0.08);
        b.add(bar);
      }
    }
    b.position.set(x, 0, z);
    b.rotation.y = rot;
    b.name = `bike-${seed}`;
    g.add(b);
    // 整辆车一个盒（车不是几十根管子各自的障碍）：局部外廓 = 前后轮 ±0.711、
    // 车把 ±0.186、带车筐时前伸到 0.731，鞍座顶 0.885。绕 Y 旋转后扩成世界 AABB。
    boxes.push(rotBoxSpec(b.name, x, z, rot, [1.45, 0.89, 0.38], 0));
  };

  /* ================= 主体量（1F + 2~3F + 屋顶） ================= */

  const DEPTH = ZF - ZN;      // 10.6
  const CZ = (ZN + ZF) / 2;   // -0.6
  /* 邻居户的室内空腔深度：上方住宅层的实心体量南端往北退这一段，
   * 腾出来的空间由「带洞结构板 + 室盒内衬 + 低模家具」填回去。
   * 外轮廓仍收在 z=ZF 这条线上（结构板补位），所以楼的体量分毫不变，
   * 碰撞盒（APT_WALL_BOXES 里的 apt-south-* 也按 APT_ZF 算）不需要跟着动。 */
  const ROOM_D = APT_ROOM_D;   // 模块级常量（APT_WALL_BOXES 要用同一条）
  const DEPTHB = DEPTH - ROOM_D;   // 只退南端时的进深
  const CZB = CZ - ROOM_D / 2;     // 只退南端时的新中心
  /* 住宅层（2~4F）南北两端各退 ROOM_D：外廊侧那三扇窗（卧室大窗 / 洗面所 /
   * 浴室）后面同样要是能看进去的真屋子，北端不退就只能是实心素面 + 贴片玻璃。
   * 两端退的量相同 → 实心块中心不动（仍是 CZ），只改进深。
   * 1F 北面是实心墙、没有窗，apt-ground 不跟着退（保持 DEPTHB/CZB）。 */
  const DEPTHB2 = DEPTH - ROOM_D * 2;
  const CZB2 = CZ;

  // 底层（0..3.36）：基座色。203 的地板（y3.4）是它的顶，留 GAP。
  // 南端同样退 ROOM_D：1F 的四户窗与入口门厅也做成能看进去的屋子，
  // 外轮廓由下面的结构板补回 z=ZF，楼的体量与碰撞盒都不跟着动。
  // 北端也退 ROOM_D：1F 北面是一排小店店面（电玩/面包/拉面/书店/干洗），
  // 退出来的腔由「带洞结构板 + 店内衬 + 低模陈列」填回，与 2~4F 同一套做法。
  put(gbox(W, F2 - GAP, DEPTHB2), M.wallBase, 0, (F2 - GAP) / 2, CZB2).name = 'apt-ground';

  // 2 层：203 居中（外皮归 203 自己的 shell），东西各两户是实心体量。
  // 底/顶离开 F2/F3，内侧面离开 203 山墙（x±6.2）——三处共用标高都是闪面。
  for (const [ux0, ux1] of [[X0, -6.2 - GAP], [6.2 + GAP, X1]] as Array<[number, number]>) {
    const w = ux1 - ux0;
    put(gbox(w, LVL - GAP * 2, DEPTHB2), M.wall, (ux0 + ux1) / 2, F2 + LVL / 2, CZB2)
      .name = ux0 < 0 ? 'apt-neighborw' : 'apt-neighbore';
  }

  // 3~4 层：全宽实心（203 正上方的 303/403 也只是立面，内部不可见）
  const tierNames = ['apt-f3', 'apt-f4'];
  FLOORS.slice(1).forEach((f, i) => {
    put(gbox(W, LVL - GAP * 2, DEPTHB2), M.wall, 0, f + LVL / 2, CZB2).name = tierNames[i];
  });

  /* ---- 竖向分色：两端妻壁 / 各层腰线 / 顶部檐口 ----
   * 一栋 36m 长的楼如果只有一个色号，远看就是"一整片墙"。
   * 这几笔不花什么顶点，却把体量切成有比例的块面。
   */
  for (const [wx, sx] of [[X0, -1], [X1, 1]] as Array<[number, number]>) {
    // 妻壁（山墙）压深一档：只包 2F~4F（1F 是基座色），顶部微凸出屋面读作压顶
    put(gbox(0.08, RF - F2 + 0.12, DEPTH), M.wallAlt, wx + sx * 0.04, (F2 + RF + 0.1) / 2, CZ);
  }
  for (const f of FLOORS) {
    // 各层腰线（幕板）：一条浅色混凝土带，把楼层从立面上读出来
    put(gbox(W, 0.14, 0.07), M.slabDry, 0, f - 0.07, ZF + 0.035);
    put(gbox(W, 0.14, 0.07), M.slabDry, 0, f - 0.07, ZN - 0.035);
  }

  // 屋顶：女儿墙 + 水箱 + 室外机 + 天线
  put(gbox(W + 0.3, 0.45, DEPTH + 0.3), M.slabDry, 0, RF + 0.225, CZ).name = 'apt-roof';
  put(gbox(1.7, 1.25, 1.7), M.metal, -6.5, RF + 0.45 + 0.62, -0.5);
  put(gbox(0.9, 0.55, 0.7), M.metal, 4.0, RF + 0.45 + 0.28, 1.2);
  put(gbox(0.9, 0.55, 0.7), M.metal, 5.1, RF + 0.45 + 0.28, 1.2);
  put(new THREE.CylinderGeometry(0.02, 0.03, 1.8, 6), M.metal, 8.5, RF + 1.35, -1.0);

  /* ---- 西端外廊尽端：封头栏杆 ----
   * 原先这里立着一座"假楼梯间"（实心体量 + 亮窗，没有真梯）。本次把它
   * 拆掉，换成建筑另一端的真楼梯（见东端共用外楼梯段），西端只补一段
   * 尽端封头栏杆——外廊到头了，人不能再往前走。
   */

  /* ================= 南立面：阳台 + 真窗 + 生活痕迹 ================= */

  for (let fi = 0; fi < FLOORS.length; fi++) {
    const F = FLOORS[fi];
    for (let ui = 0; ui < UNITS.length; ui++) {
      const [ux0, ux1] = UNITS[ui];
      const uw = ux1 - ux0;
      const ucx = (ux0 + ux1) / 2;
      /* —— 203（fi===0 && ui===2）：只补阳台结构底板 ——
       * 203 的阳台栏杆、落地窗、外墙覆板都由 room shell / 上面那段覆板自己出，
       * 邻户循环里唯一缺的是"阳台的板"。
       *
       * 不补的后果：203 的阳台地面是 props.ts 里一张法线朝上的单面 Plane
       * （rooms 里的 balcony，floor=jpdeck）。从街上或院子里仰视时背面被
       * 剔除，整格阳台没有底，能一眼看穿进屋里——立面正中缺一块，比邻户
       * 任何一处破绽都显眼。
       *
       * 规格照抄邻户（顶面 = 户内地板标高 F、挑出到 ZB），只把板厚从 0.14
       * 收到 0.12：木地板 plane 已经占着 y=F 这一层，板顶停在 F-0.01 避开
       * 共面闪面；板底 F-0.13 与邻户的 F-0.14 只差 1cm，仰视读不出台阶。 */
      if (fi === 0 && ui === 2) {
        put(gbox(uw, 0.12, ZB - ZF), M.slab, ucx, F - 0.07, (ZF + ZB) / 2);
        put(gbox(0.16, 0.03, 0.16), M.dark, ux1 - 0.35, F - 0.005, ZB - 0.35);
        continue;
      }

      /* —— 阳台结构 —— */
      // 板：顶面 = 户内地板标高 F，挑出 1.6m。
      put(gbox(uw, 0.14, ZB - ZF), M.slab, ucx, F - 0.07, (ZF + ZB) / 2);
      // 排水口：角落一个深色小盒 + 一小截落水管头
      put(gbox(0.16, 0.03, 0.16), M.dark, ux1 - 0.35, F - 0.005, ZB - 0.35);
      // 栏杆
      railing(ux0 + 0.05, ux1 - 0.05, F, ZB);

      /* —— 立面窗：与 203 完全同款户型（四樘洞口）——
       * 203 南面 = 腰窓(左) + 玻璃滑門(中) + 腰窓(右) + 高窓(右偏)，
       * 每一户邻居都按这套坐标整体平移 ucx 复制一遍——公寓楼的户型本就该一致。
       * 混凝土覆板 + 窗套 + 窗台（unitFace）洞里嵌真窗。 —— */
      const O = (dx: number) => ucx + dx;  // 以 203 洞口坐标为基准、按户中心平移
      // 卧室窗（西腰窓）独立于客厅滑門判定：亮暗、是否拉帘都各算各的。
      const pickState = (): WinState => {
        const r = rnd();
        return r < 0.22 ? 'dark' : r < 0.52 ? 'lit' : r < 0.62 ? 'lamp' :
               r < 0.84 ? 'curtain' : r < 0.95 ? 'frost' : 'ajar';
      };
      const winState = pickState();        // 卧室窗
      const winState2 = pickState();       // 右腰窓（餐厨）
      const highState: WinState = (() => {
        const r = rnd();
        return r < 0.5 ? 'dark' : r < 0.88 ? 'lit' : 'frost';
      })();
      const doorState = pickState();       // 玻璃滑門（客厅主窗）
      const holes = [
        { a: O(-5.28), b: O(-3.72), y0: F + 0.47, y1: F + 2.23 },  // 腰窓（左）
        { a: O(-1.38), b: O(1.38),  y0: F + 0.07, y1: F + 2.38 },  // 玻璃滑門
        { a: O(2.02),  b: O(3.18),   y0: F + 0.47, y1: F + 2.23 },  // 腰窓（右）
        { a: O(4.42),  b: O(5.38),   y0: F + 0.87, y1: F + 2.23 },  // 高窓
      ];
      unitFace({
        ux0, ux1, F, z: ZF + 0.09, mat: M.wall, trim: true,
        holes,
      });
      /* 窗后的屋子：结构板把退让掉的外皮补回 z=ZF，里头是内衬 + 低模家具。
       * 卧室（西腰窓后）与客厅（滑門后）各自亮暗——卧室亮客厅黑是常态。
       * 拉上帘的分区连家具都不建：帘后透暖光就够读出"有人睡下了"。
       * 203 那一户在上面的 continue 里就跳过了，它有自己的房间。 */
      unitInterior({
        ux0, ux1, F, holes,
        lit: doorState === 'lit',
        bedLit: winState === 'lit',
        livFurnish: doorState !== 'curtain',
        bedFurnish: winState !== 'curtain',
        cloth: rnd() < 0.5 ? M.rmClothLit : M.rmClothAlt,
        seed: 9100 + fi * 17 + ui,
      });
      /* 玻璃全走透明档：窗后就是 unitInterior 建的真室内，亮灯户看得见
       * 暖色家具和吸顶灯，熄灯户是深调的暗屋。窗帘/磨砂档自己带遮挡，
       * 仍然从窗洞读不出室内——每户的层次就从这里来。每扇窗都挂帘。 */
      windowUnit({ x: O(-4.5), y: F + 1.35,  z: ZF, dir: 1, w: 1.56, h: 1.76, state: winState,  seeThrough: true, curtain: true, noFrame: true, kind: Math.floor(rnd() * 5) });
      windowUnit({ x: O(0),    y: F + 1.225, z: ZF, dir: 1, w: 2.76, h: 2.31, sill: false, state: doorState, seeThrough: true, curtain: true, noFrame: true, kind: Math.floor(rnd() * 4) });
      windowUnit({ x: O(2.6),  y: F + 1.35,  z: ZF, dir: 1, w: 1.16, h: 1.76, state: winState2, seeThrough: true, curtain: true, noFrame: true, kind: Math.floor(rnd() * 5) });
      windowUnit({ x: O(4.9),  y: F + 1.55,  z: ZF, dir: 1, w: 0.96, h: 1.36, state: highState, seeThrough: true, curtain: true, noFrame: true, kind: Math.floor(rnd() * 5) });

      /* —— 生活痕迹：每户不同，避免复制粘贴 —— */
      acUnit(ux0 + 0.55, F, ZF + 0.55, 1);
      if (rnd() < 0.55) dryingRack(ucx + (rnd() - 0.5) * uw * 0.35, F, ZF + 1.05, 7000 + fi * 10 + ui);
      if (rnd() < 0.65) potted(ux1 - 0.5, F, ZF + 1.2, 300 + fi * 7 + ui);
      if (rnd() < 0.35) storageBox(ux0 + 1.25, F, ZF + 0.85, rnd() < 0.5 ? '#7f8a96' : '#6f6a63');
      const hasChair = rnd() < 0.22;
      if (hasChair) balconyChair(ucx + (rnd() - 0.5) * uw * 0.4, F, ZF + 1.0);
      if (rnd() < 0.18) {
        // 靠墙立着的扫帚
        const broom = new THREE.Mesh(new THREE.CylinderGeometry(0.014, 0.014, 1.15, 5), M.wood);
        broom.position.set(ux1 - 1.1, F + 0.58, ZF + 0.22);
        broom.rotation.x = -0.16;
        g.add(broom);
      }

      /* —— 阳台的铺装与生活件 ——
       *
       * 这一段的所有 rnd() 都挂在既有调用之后：makeRng 是固定种子的序列，在中间
       * 插一次调用，后面每一户的窗态 / 帘子 / 家具摆位会整体重排（heater、自行车
       * 的碰撞盒数量也跟着变）。追加在末尾，已有的 14 户一动不动。
       *
       * 差的是体量感而不是件数：203 自家阳台有铺装、有大花箱、有能坐下来的桌椅，
       * 邻居这边原先只有巴掌大的小件，摊在近 20㎡ 的板上只剩空。
       * 材质全部取 M.* 已有桶——外壳合批后 draw call 已经贴着上限，每新增一种
       * 材质就多一个桶；复用现有桶的话，加多少件几何都不涨 draw call。 */
      // 铺装：木甲板 / 防滑垫二选一，四周留一圈混凝土收边，不铺到墙根
      put(gbox(uw - 0.18, 0.012, 1.09), rnd() < 0.45 ? M.wood : M.walkFloor, ucx, F + 0.007, ZF + 0.695);
      // 外缘排水沟 + 格栅盖板：日本阳台沿外缘一条沟，雨水不顺着立面直挂下去
      put(gbox(uw - 0.20, 0.045, 0.16), M.gutter, ucx, F + 0.004, ZB - 0.24);
      put(gbox(uw - 0.24, 0.012, 0.13), M.slabDark, ucx, F + 0.016, ZB - 0.24);

      /** 203 的左右贴邻（202 / 204）：从自家阳台天天看，这两户给完整配置。 */
      const near203 = fi === 0 && (ui === 1 || ui === 3);
      // 花箱贴栏杆摆（晒得到太阳），东西两端各一个，中间留给晾衣架和桌椅
      const nPlanter = near203 ? 2 : rnd() < 0.72 ? 1 : 0;
      for (let i = 0; i < nPlanter; i++) {
        const px = i === 0 ? ux0 + 1.3 + rnd() * 0.8 : ux1 - 2.3 + rnd() * 0.8;
        planterBox(px, F, ZF + 1.0, 0.9 + rnd() * 0.5, 4100 + fi * 23 + ui * 7 + i);
      }
      if (near203 || rnd() < 0.3) washer(ux1 - 1.2, F, ZF + 0.40);
      if (near203 || rnd() < 0.45) {
        const bw = 2.6 + rnd() * 1.6;
        const bx = ucx + (rnd() - 0.5) * (uw - bw - 1.2);
        blindSheet(bx - bw / 2, bx + bw / 2, F, ZB - 0.05, rnd() < 0.5 ? M.rmClothLit : M.rmClothAlt);
      }
      if (rnd() < 0.55) flatUp(0.7, 0.42, M.dark, ucx, F + 0.02, ZF + 0.30);          // 滑门外地垫
      if (rnd() < 0.35) plantStand(ux0 + 3.2 + rnd() * 0.8, F, ZF + 0.36, 5200 + fi * 13 + ui);
      if (nPlanter > 0 && rnd() < 0.5) wateringKit(ux0 + 2.2 + rnd() * 0.6, F, ZF + 0.75);
      if (hasChair && rnd() < 0.6) balconyTable(ucx + 1.5, F, ZF + 0.95);
      if (rnd() < 0.6) balconyLamp(ucx + 1.8, F + 2.10, ZF + 0.06);
    }

    /* —— 戸境板：户与户之间的分隔板 ——
     * 它既把立面切成一格格（竖向节奏），又让阳台读起来是"每户一格"，
     * 不是一条通长走廊。203 自己有侧栏杆，那两片跳过。
     */
    for (const px of PARTITIONS) {
      if (fi === 0 && (px === -6.2 || px === 6.2)) continue;
      put(gbox(0.07, 1.15, ZB - ZF - 0.02), M.slabDark, px, F + 1.15 / 2, (ZF + ZB) / 2);
      // 顶部压条：混凝土板直接切齐会毛，真楼这儿都有一道金属收口压住
      put(gbox(0.095, 0.03, ZB - ZF - 0.02), M.metal, px, F + 1.16, (ZF + ZB) / 2);
      // 每两格挂一根竖雨水管，一路下到地面
      if (px !== -6.2 && px !== 6.2) downpipe(px + 0.09, ZF + 0.12, F + 1.15);
    }
  }

  /* ================= 北立面：公共外廊 + 户门 + 高窗 =================
   * 每层一条外廊（板挑出 1.25m），户门朝走廊开——203 的正门 x4.6..5.5
   * 推开就是这条走廊，室内外在这里闭环。
   *
   * 本次补全的公共交通逻辑：外廊东端 (x+31.15) 与共用外楼梯的西侧平台
   * 无缝相接（板直接铺过去，不留缝），西端用封头栏杆收头。地面铺浅灰
   * 防滑层（雨夜微湿）、贴墙一条通长排水沟 + 集水口；照明分两档——
   * 2F 吊在 3F 廊板底下，3F 顶层没有可吊的板，改墙面壁灯——明暗相间，
   * 不是一条亮到底的白管子。
   */
  // 外廊板/腰壁统一东延到楼梯平台西缘（STRS.lx0），与平台拼缝相接、互不叠面
  const CW = STRS.lx0 - X0;      // 外廊全长
  const CX = (STRS.lx0 + X0) / 2;
  const PLATE = 0.012;         // 地面铺装厚度（防滑层）
  for (let fi = 0; fi < FLOORS.length; fi++) {
    const F = FLOORS[fi];
    // 廊下结构板 + 栏杆（0.75 高的混凝土腰壁 + 上部竖栅）
    put(gbox(CW, 0.14, 1.25), M.slabDry, CX, F - 0.07, ZN - 0.625);
    // 外廊腰壁：0.75 高的混凝土实体 + 上部竖栅，临空边的实体屏障
    put(gbox(CW, 0.75, 0.08), M.slabDark, CX, F + 0.375, ZN - 1.25);
    put(gbox(CW, 0.05, 0.11), M.railTop, CX, F + 1.15, ZN - 1.25);
    put(gbox(CW, 0.04, 0.08), M.railTop, CX, F + 0.82, ZN - 1.25);
    for (let px = X0 + 0.14; px < STRS.lx0 - 0.06; px += 0.24) {
      put(gbox(0.022, 0.36, 0.022), M.railing, px, F + 0.97, ZN - 1.25);
    }
    // 腰壁 + 竖栅 + 扶手整段一个盒：从廊面 F 到顶扶手上沿 F+1.175，厚 0.11（腰壁 0.08）
    boxes.push(boxSpec(`corridor-rail-${F.toFixed(1)}`, X0 - 0.03, STRS.lx0 + 0.03, F, F + 1.2, ZN - 1.31, ZN - 1.19));
    // 西端封头栏杆：外廊到头了（原"假楼梯间"拆除后补的收头）
    railRun(X0 + 0.04, F, ZN - 1.22, X0 + 0.04, F, ZN + 0.02);

    // 地面铺装：中浅灰防滑层（微湿）+ 贴墙通长排水沟 + 集水口
    put(gbox(CW - 0.1, PLATE, 1.18), M.walkFloor, CX, F + PLATE / 2, ZN - 0.64);
    put(gbox(CW - 0.1, 0.012, 0.09), M.gutter, CX, F + PLATE + 0.006, ZN - 0.055);
    for (let dx = -28; dx <= 28; dx += 8) {
      put(gbox(0.36, 0.016, 0.17), M.gutter, dx, F + PLATE + 0.008, ZN - 0.055);
      for (let s = 0; s < 3; s++) put(gbox(0.28, 0.006, 0.024), M.metal, dx - 0.09 + s * 0.09, F + PLATE + 0.017, ZN - 0.055);
    }

    // 廊下照明：2F/3F 吸顶灯（上一层廊板底，交替明暗两档）；顶层(4F)无板可吊→墙面壁灯
    for (let i = 0; i <= 6; i++) {
      const lx = [-27, -18, -9, 0, 9, 18, 27][i];
      if (fi < FLOORS.length - 1) {
        // 吸顶灯 y 取「上层廊板底 - 0.04」：板上沿底在 F+LVL-0.14，灯面若同高会与之共面 z-fighting。
        // 下压 4cm 让灯面略低于天花板，读作内嵌吸顶灯、不再闪。
        flatDown(0.5, 0.16, i % 2 === 0 ? M.hall : M.hallDim, lx, F + LVL - 0.18, ZN - 0.5);
      } else {
        // 顶层壁灯：x 位避开各户窗洞（洗面 / 浴室 / 玄関）
        const sx = [-27, -18, -9, 0, 9, 18.4, 26.2][i];
        put(gbox(0.1, 0.16, 0.26), M.dark, sx, F + 2.08, ZN - 0.19);
        put(gbox(0.05, 0.1, 0.19), i % 2 === 0 ? M.warm : M.warmSoft, sx, F + 2.08, ZN - 0.245);
      }
    }

    // 楼层牌（2F / 3F / 4F）：仅保留西端一块（东端 30.55 与 X01 表札共面 z-fighting，已删）。
    // 另楼梯口平台山墙（buildApartmentStairs 内）还有一块朝东的楼层牌作侧向指示。
    for (const pxx of [-29.95]) {
      panel(0.34, 0.23, floorPlateMat(`${fi + 2}F`),
        pxx, F + 1.92, ZN - 0.165, -1);
    }

    for (let ui = 0; ui < UNITS.length; ui++) {
      const [ux0, ux1] = UNITS[ui];
      const ucx = (ux0 + ux1) / 2;
      // 203（fi===0 && ui===2）北立面：正门/窗是房间自己的，但这里不能像南立面
      // 那样直接 continue——房间北墙在 z=-5.9，落在建筑空心体量内部（外壳北面在
      // z=-6.055），外廊看过来先撞上那堵实心北面，把 203 的北窗整片盖死（就是
      // 报的"长方体盒子遮窗"+北面像贴图）。南立面能看进去是因为 203 南墙在
      // z=4.8、落在体量南面(4.7)之外。这里补一层带洞覆板 + 透明玻璃，盖住那堵
      // 实心北面、让外廊看进来是透明玻璃 + 亮着的屋里——和南立面 203 对齐。
      // 洞口坐标取 dormLayout.json 北墙 openings 的原值（ucx=0，直接用世界 x）：
      // 娜娜卧室窗 [-5.3,-4.1]、洗面所窗 [-0.5,0.3]、浴室窗 [1.8,2.6]、玄関ドア [4.5,5.4]。
      // 这里是 203 北面覆板唯一的一处——洞口必须跟着房间布局走，别在别处再建
      // 一层同标高的板（同 z 双层必然 z-fighting，且第二层的洞口一旦写成邻户
      // 那套偏移坐标，就会把窗整片糊死，看着像"北墙是贴图"）。
      if (fi === 0 && ui === 2) {
        unitFace({
          ux0, ux1, F, z: ZN - 0.09, mat: M.wallAlt, trim: false, bleed: 0.08,
          holes: [
            { a: -5.3, b: -4.1, y0: F + 0.9, y1: F + 2.35 },   // 娜娜卧室窗
            { a: -0.5, b: 0.3, y0: F + 1.6, y1: F + 2.35 },    // 洗面所窗
            { a: 1.8,  b: 2.6, y0: F + 1.6, y1: F + 2.35 },    // 浴室窗
            { a: 4.5,  b: 5.4, y0: F + 0.0, y1: F + 2.2 },     // 玄関ドア
          ],
        });
        // 三扇北窗：透明玻璃（seeThrough），屋里是真家具/吸顶灯，亮不亮由室内决定。
        // 不画外框/帘/洞口侧壁：房间外壳 buildWindow 已经给了框和帘，避免叠两套。
        windowUnit({ x: -4.7, y: F + 1.625, z: ZN, dir: -1, w: 1.2,  h: 1.45, seeThrough: true, noFrame: true, paneZ: ZN - 0.01, state: 'dark', curtain: false, kind: 2 });
        windowUnit({ x: -0.1, y: F + 1.975, z: ZN, dir: -1, w: 0.8,  h: 0.75, seeThrough: true, noFrame: true, paneZ: ZN - 0.01, state: 'dark', curtain: false, kind: 3 });
        windowUnit({ x: 2.2,  y: F + 1.975, z: ZN, dir: -1, w: 0.8,  h: 0.75, seeThrough: true, noFrame: true, paneZ: ZN - 0.01, state: 'dark', curtain: false, kind: 1 });
        continue; // 正门扇/热水器/户号牌由房间自己的 buildEntryDoor 等负责，跳过邻户那套
      }

      // 北立面：与 203 北面完全一致——同一套户型的四樘洞口（卧室大窗 / 洗面所 /
      // 浴室 / 玄関ドア），整户坐标按 ucx 平移。
      // 早先这里抄的是另一套偏移坐标（4.62/2.02/0.22）且漏了卧室那扇大窗，
      // 结果外廊上看邻户是「三扇偏移的小贴片」，和 203 那一排对不上。
      const O = (dx: number) => ucx + dx;
      // 203 北墙 openings 的户内相对坐标（dormLayout.json，单点真相）
      const NORTH_HOLES = [
        { a: O(-5.3), b: O(-4.1), y0: F + 0.9, y1: F + 2.35 },   // 卧室大窗
        { a: O(-0.5), b: O(0.3),  y0: F + 1.6, y1: F + 2.35 },   // 洗面所窗
        { a: O(1.8),  b: O(2.6),  y0: F + 1.6, y1: F + 2.35 },   // 浴室窗
        { a: O(4.5),  b: O(5.4),  y0: F + 0.0, y1: F + 2.2 },    // 玄関ドア
      ];
      unitFace({
        ux0, ux1, F, z: ZN - 0.09, mat: M.wallAlt, trim: false,
        holes: NORTH_HOLES,
      });
      const doorX = O(4.95);   // 门洞中心（洞 x 跨度 O(4.5)~O(5.4)）
      /* —— 住户层次：门色 / 户号 / 门垫 / 门口盆栽，每户有微妙不同 ——
       * 5 户从东（最靠近东侧楼梯）到西顺序编号 X01..X05：ui=4(东)→01，ui=3→02，
       * ui=2(中)→03，ui=1→04，ui=0(西)→05。2F 中户(ui=2)是玩家自己的 203，
       * 正门/窗由房间自身绘制、此处跳过，故 2F 中户无邻户牌（3F/4F 照常显示 303/403）。 */
      const doorTones = [M.woodDeep, M.doorB, M.doorC];
      const unitNum = ['05', '04', '03', '02', '01'][ui];
      // 门扇 / 门框按新门洞（宽 0.9、高 2.2）给尺寸，门牌挂到门东侧那片墙上
      put(gbox(0.86, 2.16, 0.06), doorTones[(ui + fi * 2) % 3], doorX, F + 1.08, ZN - 0.03);
      put(gbox(0.98, 2.24, 0.04), M.frame, doorX, F + 1.12, ZN - 0.005);
      panel(0.16, 0.09, M.warmPale, doorX + 0.60, F + 2.30, ZN - 0.03, -1);  // 门牌灯
      panel(0.19, 0.14, emissive('#d9cfbc', { map: numberPlateTexture(`${fi + 2}${unitNum}`) }),
        doorX + 0.60, F + 1.88, ZN - 0.165, -1);                              // 户号牌（微背光）
      // 户主牌（表札）：写住户名字——203 是主角 Vivian Nana，其余用常见大模型填充。
      // 贴在户号牌右侧的墙上（dir=-1 朝外廊），与冷灰门牌形成冷暖对比。
      const resident = RESIDENTS[`${fi + 2}${unitNum}`];
      if (resident) {
        panel(0.30, 0.12, emissive('#fff6ea', { map: namePlateTexture(resident) }),
          doorX + 0.94, F + 1.88, ZN - 0.165, -1);                           // 户主牌（亚克力表札）
      }
      const matTones = ['#3a4250', '#463f37', '#33413b', '#514639'];
      put(gbox(0.72, 0.016, 0.4), toon(matTones[(ui + fi) % 4]), doorX, F + 0.018, ZN - 0.44);  // 门垫
      if (rnd() < 0.55) potted(doorX - 0.64, F + PLATE, ZN - 0.42, 500 + fi * 9 + ui);          // 门口盆栽
      /* —— 三扇窗：玻璃全走透明档 ——
       * 住宅层实心体量北端已退 ROOM_D（DEPTHB2），窗后是真屋子（unitInteriorNorth），
       * 亮不亮由室内材质自己说话，不再贴发光剪影。
       * 三扇状态各自独立随机：整栋楼不许出现「每户都一样的亮窗」，那是最假的
       * 一种假。磨砂档给浴室（走廊侧的私密），半开档只给少数几户。 */
      const pickNorth = (allowAjar: boolean): WinState => {
        const v = rnd();
        if (allowAjar) {
          return v < 0.30 ? 'dark' : v < 0.62 ? 'lit' : v < 0.80 ? 'frost' :
                 v < 0.92 ? 'curtain' : 'ajar';
        }
        return v < 0.34 ? 'dark' : v < 0.66 ? 'lit' : v < 0.86 ? 'frost' : 'curtain';
      };
      const bedState = pickNorth(true);    // 卧室大窗：窗台低，是唯一能看进屋的一扇
      const washState = pickNorth(false);  // 洗面所小高窗
      const bathState = pickNorth(false);  // 浴室小高窗

      // 窗后的屋子：卫浴区（洗面所 + 浴室 + 玄関）与卧室区各算各的亮暗，
      // 拉帘 / 磨砂的分区连家具都不建——帘后透暖光就够读出「有人睡下了」。
      unitInteriorNorth({
        ux0, ux1, F, holes: NORTH_HOLES,
        lit: washState === 'lit' || bathState === 'lit',
        bedLit: bedState === 'lit',
        livFurnish: washState !== 'curtain' && bathState !== 'curtain',
        bedFurnish: bedState !== 'curtain',
        cloth: rnd() < 0.5 ? M.rmClothLit : M.rmClothAlt,
        seed: 3300 + fi * 17 + ui,
      });

      windowUnit({ x: O(-4.7), y: F + 1.625, z: ZN, dir: -1, w: 1.2, h: 1.45,
        seeThrough: true, paneZ: ZN - 0.01, curtain: true, state: bedState,
        kind: Math.floor(rnd() * 5) });
      // 洗面所 / 浴室是小高窗，不挂帘——那两处通常是磨砂玻璃或卷帘，
      // 挂上布帘反而把「湿区」读错。
      windowUnit({ x: O(-0.1), y: F + 1.975, z: ZN, dir: -1, w: 0.8, h: 0.75,
        seeThrough: true, paneZ: ZN - 0.01, curtain: false, state: washState,
        kind: Math.floor(rnd() * 5) });
      windowUnit({ x: O(2.2), y: F + 1.975, z: ZN, dir: -1, w: 0.8, h: 0.75,
        seeThrough: true, paneZ: ZN - 0.01, curtain: false, state: bathState,
        kind: Math.floor(rnd() * 5) });

      // 防盗格栅：外廊侧也有住户装（自行车停放区上方那几户居多）。随机给，
      // 只装在没拉帘的窗上——帘后本来就看不进去，加格栅等于白给几何。
      if (rnd() < 0.45) {
        for (const [wx, ww, wy, wh] of [
          [O(-4.7), 1.2, F + 1.625, 1.45],
          [O(2.2), 0.8, F + 1.975, 0.75],
        ] as Array<[number, number, number, number]>) {
          for (let b = 0; b < 3; b++) {
            put(gbox(0.03, wh + 0.04, 0.03), M.metal, wx - ww / 2 + 0.12 + b * (ww - 0.24) / 2, wy, ZN - 0.16);
          }
          put(gbox(ww + 0.04, 0.03, 0.03), M.metal, wx, wy + wh / 2 + 0.02, ZN - 0.16);
        }
      }

      // 给汤器（燃气热水器）：一半的户有，挂在廊下墙上（覆板外侧，保持可见）。
      // x 落在浴室窗（O(1.8)~O(2.6)）与玄関ドア（O(4.5)）之间的实墙上——原来在
      // O(2.1)，正好压在浴室窗洞上。
      if (rnd() < 0.55) {
        put(gbox(0.46, 0.66, 0.28), M.metal, ucx + 3.2, F + 1.5, ZN - 0.16);
        // 碰撞盒：与给汤器同尺寸（贴墙侧 z 略扩，避免穿模），跳过头顶/脚踝
        boxes.push(boxSpec(`heater-${fi}-${ui}`, ucx + 2.97, ucx + 3.43, F + 1.17, F + 1.83, ZN - 0.30, ZN - 0.02));
      }
      // 电表箱：每户一个，贴在分户墙上（移到覆板外侧，避免被覆板挡住）
      put(gbox(0.34, 0.44, 0.09), M.metal, ux0 + 0.3, F + 1.75, ZN - 0.13);
    }
  }

  // 外廊排水：2F 沟西端落水管一路下到地面（3F 沟向东排往楼梯平台方向）
  downpipe(X0 + 0.28, ZN - 0.07, F2 + 0.1);

  /* ================= 东端共用外置楼梯（折返式钢构，1F→2F→3F） =================
   *
   * 位置刻意放在东端——从便利店 / 默认镜头方向看正好落在楼的右手边，
   * 形成「正面阳台 → 侧边外廊 → 外置楼梯 → 地面」的连续读法。
   *
   * 垂直交通（全部真实构件，连续可走，逐层折返，不是每层一段摆设）：
   *   第一跑    地面(x36.95) → 2F 西平台(x32.45)，19 级 0.179/0.25
   *   2F 西平台   西缘与 2F 外廊拼缝相接；下方是储物间
   *   2F 步行廊   南梯带 2F 标高向东 4.1m（2F→3F 的过道）
   *   东端折返平台  转身 180° 处；其下方正好遮住第一跑的起步
   *   第二跑    折返平台(x36.55) → 3F 西平台(x32.45)，16 级，
   *             直接叠在第一跑上方（净空 ~2.8m，真实折返梯的叠合关系）
   *   3F 西平台   西缘接 3F 外廊；3F 步行廊 + 东端折返平台，再上一跑
   *   第三跑    折返平台(x36.55) → 4F 西平台(x32.45)，16 级，
   *             叠在 2F 跑上方（净空 ~2.8m）；4F 西平台顶上一挑小雨棚收头，不再向上
   *
   * 构件语言：深灰钢侧梁/立柱 + 浅灰湿踏步 + 炭灰竖栅栏杆（与外廊同套），
   * 踏面前缘压防滑条，平台面与外廊同一种防滑湿面，到达边有封板。
   */
  {
    const { lx0, lx1, wx1, ex1, f1Bot, za0, za1, zb0, zb1 } = STRS;
    const F3 = FLOORS[1];
    const F4 = FLOORS[2];

    /* ---- 一跑楼梯：踏步 + 踢面 + 防滑条 + 侧梁 + 两侧扶手 ---- */
    const stairFlight = (xBot: number, yBot: number, xTop: number, yTop: number, z0: number, z1: number) => {
      const rise = yTop - yBot;
      const run = xBot - xTop;                     // 向西上行
      const n = Math.max(2, Math.round(rise / 0.176));
      const riser = rise / n;
      const tread = run / (n - 1);
      const wid = z1 - z0;
      const cz = (z0 + z1) / 2;
      for (let i = 1; i < n; i++) {
        const ty = yBot + i * riser;               // 这一级踏面顶标高
        const tFront = xBot - (i - 1) * tread;     // 踏面前缘（东）
        const tBack = tFront - tread;
        // 踏面板：浅灰微湿，向东外突 3cm 做段鼻（防滑突缘）
        put(gbox(tread + 0.03, 0.055, wid), M.tread, (tFront + tBack) / 2 + 0.015, ty - 0.0275, cz).userData.noCollide = true;
        // 踢面板：踏面之下的竖板，楼梯从侧面看是实心的
        put(gbox(0.045, riser - 0.055, wid), M.treadRise, tFront - 0.0225, ty - 0.055 - (riser - 0.055) / 2, cz).userData.noCollide = true;
        // 防滑条：浅灰踏面上一道深色，雨夜读得出每一级的位置
        put(gbox(0.028, 0.012, wid - 0.12), M.dark, tFront - 0.038, ty + 0.006, cz).userData.noCollide = true;
      }
      // 到达边最后一级踢面（平台边缘那道竖缝，没有它梯与平台之间就是一道黑洞）
      put(gbox(0.05, riser, wid), M.treadRise, xTop + 0.025, yTop - riser / 2, cz).userData.noCollide = true;
      // 侧梁（斜梁）：0.34 高的实钢梁，踏步坐在它上面——楼梯的"真实厚度"
      const ang = Math.atan2(rise, run);
      const len = Math.hypot(rise, run) + 0.35;
      for (const sz of [z0 + 0.075, z1 - 0.075]) {
        const beam = new THREE.Mesh(gbox(len, 0.34, 0.11), M.stairSteel);
        beam.position.set((xBot + xTop) / 2 + 0.12, (yBot + yTop) / 2 - 0.23, sz);
        beam.rotation.z = -ang;                    // 东端低、西端高
        beam.userData.noCollide = true;            // 斜梁不挡第一人称（碰撞由斜坡盒接管，避免上楼一卡一卡）
        g.add(beam);
      }
      // 两翼扶手：沿坡的栏杆（railRun 支持带坡段）
      for (const rz of [z0 + 0.035, z1 - 0.035]) {
        railRun(xBot - 0.06, yBot + riser, rz, xTop + 0.04, yTop + 0.02, rz);
      }
    };

    /* ---- 平台：钢框板 + 与外廊同款防滑湿面 ---- */
    const landing = (x0: number, x1: number, y: number, z0: number, z1: number) => {
      // 平台板本身不进第一人称碰撞（碰撞由外廊/平台 floor 盒接管，避免薄板被当成空气墙）
      put(gbox(x1 - x0, 0.16, z1 - z0), M.stairSteel, (x0 + x1) / 2, y - 0.08, (z0 + z1) / 2).userData.noCollide = true;
      put(gbox(x1 - x0 - 0.06, PLATE, z1 - z0 - 0.06), M.walkFloor, (x0 + x1) / 2, y + PLATE / 2, (z0 + z1) / 2).userData.noCollide = true;
    };

    /* ---- 钢柱 + 混凝土基础：楼梯不是悬空的 ---- */
    const column = (x: number, z: number, top: number) => {
      put(gbox(0.11, top, 0.11), M.stairSteel, x, top / 2, z);
      put(gbox(0.32, 0.24, 0.32), M.trunkWall, x, 0.12, z);
    };

    /* ---- 平台 + 梯跑：逐层生成（顶层只有西侧平台） ---- */
    for (let fi = 0; fi < FLOORS.length; fi++) {
      const lvl = FLOORS[fi];
      landing(lx0, lx1, lvl, za0, zb1);                    // 每层西侧平台
      if (fi < FLOORS.length - 1) {
        landing(lx1, wx1, lvl, zb0, zb1);                  // 该层步行廊（南梯带）
        landing(wx1, ex1, lvl, za0, zb1);                  // 该层折返平台
        stairFlight(wx1, lvl, lx1, FLOORS[fi + 1], za0, za1); // 该层 → 上一层（叠在上一跑上方）
      }
    }
    stairFlight(f1Bot, 0, lx1, F2, za0, za1);            // 第一跑：地面 → 2F（自东端折返平台下方起）

    /* ---- 平台栏杆：每个临空边一段，开口留给梯跑与外廊 ---- */
    const rb = PLATE; // 栏杆基点 = 平台铺装面
    for (let fi = 0; fi < FLOORS.length; fi++) {
      const ly = FLOORS[fi];
      railRun(lx0 + 0.03, ly + rb, za0 + 0.03, lx1 - 0.03, ly + rb, za0 + 0.03);              // 西平台北缘
      railRun(lx0 + 0.03, ly + rb, zb1 - 0.03, lx1 - 0.03, ly + rb, zb1 - 0.03);              // 西平台南缘
      railRun(lx0 + 0.03, ly + rb, za0 + 0.03, lx0 + 0.03, ly + rb, ZN - 1.15);               // 西缘·北条（外廊腰壁之外）
      railRun(lx0 + 0.03, ly + rb, ZN, lx0 + 0.03, ly + rb, zb1 - 0.03);                      // 西缘·南条（山墙之外）
      if (fi === FLOORS.length - 1) railRun(lx1 - 0.03, ly + rb, zb0 + 0.03, lx1 - 0.03, ly + rb, zb1 - 0.03); // 顶层东缘南带封头
      if (fi < FLOORS.length - 1) {
        railRun(lx1 + 0.03, ly + rb, zb1 - 0.03, wx1 - 0.03, ly + rb, zb1 - 0.03);            // 步行廊南缘
        railRun(wx1 + 0.03, ly + rb, za0 + 0.03, ex1 - 0.03, ly + rb, za0 + 0.03);            // 折返平台北缘
        railRun(wx1 + 0.03, ly + rb, zb1 - 0.03, ex1 - 0.03, ly + rb, zb1 - 0.03);            // 折返平台南缘
        railRun(ex1 - 0.03, ly + rb, za0 + 0.03, ex1 - 0.03, ly + rb, zb1 - 0.03);            // 折返平台东缘（转身处）
      }
    }

    /* ---- 支撑体系 ---- */
    column(ex1 - 0.14, za0 + 0.14, F3 - 0.16);   // 折返平台东北柱（撑 2F+3F 折返平台）
    column(ex1 - 0.14, zb1 - 0.14, F3 - 0.16);   // 折返平台东南柱
    column((lx1 + wx1) / 2, zb1 - 0.14, F3 - 0.16); // 步行廊跨中柱（南缘，撑 2F+3F 步行廊）
    column(lx1 - 0.12, za0 + 0.14, F2 - 0.16);   // 2F 平台东北柱（贴山墙，在储物间内）
    column(lx1 - 0.12, zb1 - 0.14, F2 - 0.16);   // 2F 平台东南柱
    // 第一跑跨中斜梁下的一对短柱（梁到不了地面的地方补支）
    {
      const mx = (f1Bot + lx1) / 2;
      const myTop = (F2 * (f1Bot - mx)) / (f1Bot - lx1) - 0.42;
      column(mx, za0 + 0.075, myTop);
      column(mx, za1 - 0.075, myTop);
    }
    // 平台与妻壁的连接钢托（每层西平台都连到山墙；山墙只存在于 z≥-5.9，钢托落在南条内）
    for (const ly of FLOORS.map((F) => F - 0.1)) {
      put(gbox(0.42, 0.09, 0.2), M.stairSteel, 31.29, ly, -5.62);
      put(gbox(0.42, 0.09, 0.2), M.stairSteel, 31.29, ly, -5.86);
    }

    /* ---- 4F 平台小雨棚：山墙挑梁 + 两根立柱 + 微倾暗色棚面，顶层收头不上屋顶 ---- */
    put(gbox(0.85, 0.09, 0.09), M.stairSteel, 31.5, F4 + 2.3, -5.62);            // 山墙挑梁
    put(gbox(0.1, 2.4, 0.1), M.stairSteel, 32.2, F4 + 1.2, za0 + 0.15);          // 北立柱（立在 4F 平台上）
    put(gbox(0.1, 2.4, 0.1), M.stairSteel, 32.2, F4 + 1.2, zb1 - 0.15);          // 南立柱
    {
      const canopy = new THREE.Mesh(gbox(1.3, 0.05, 2.7), M.slabDark);
      canopy.position.set(31.72, F4 + 2.4, (za0 + zb1) / 2);
      canopy.rotation.z = -0.1;   // 向东微倾，雨水排离山墙
      g.add(canopy);
      put(gbox(0.05, 0.04, 2.66), M.dark, 32.3, F4 + 2.34, (za0 + zb1) / 2);   // 棚口滴水线
    }

    /* ---- 排水：折返平台集水口 + 落水管，步行廊集水口 ---- */
    put(gbox(0.16, 0.06, 0.16), M.gutter, ex1 - 0.24, F2 - 0.03, za0 + 0.24);
    put(new THREE.CylinderGeometry(0.045, 0.045, F2 - 0.2, 8), M.pipe, ex1 - 0.24, (F2 - 0.2) / 2, za0 + 0.24);
    put(gbox(0.3, 0.016, 0.16), M.gutter, (lx1 + wx1) / 2, F2 + PLATE + 0.008, zb1 - 0.14);

    /* ---- 楼梯照明：每层山墙壁灯 + 折返平台下一盏暗灯，克制偏暖 ----
     * 壁灯装在真实存在的山墙范围内（z≥-5.9）。 */
    for (const ly of FLOORS.map((F) => F + 1.32)) {
      put(gbox(0.09, 0.15, 0.2), M.dark, 31.13, ly, -5.6);
      put(gbox(0.035, 0.09, 0.13), M.warm, 31.19, ly, -5.6);
    }
    put(gbox(0.26, 0.03, 0.3), M.warmSoft, ex1 - 0.55, F2 - 0.22, -6.9);   // 起步上方暗灯
    // 平台处的楼层牌（朝东贴山墙，爬楼梯时读）
    for (let fi = 0; fi < FLOORS.length; fi++) {
      const ly = FLOORS[fi] + 1.95;
      const lbl = `${fi + 2}F`;
      const p = new THREE.Mesh(new THREE.PlaneGeometry(0.3, 0.2), floorPlateMat(lbl));
      p.position.set(31.1, ly, -5.6);
      p.rotation.y = Math.PI / 2;
      g.add(p);
    }

    /* ---- 楼梯下方：储物间（西侧平台底下）+ 半开放设备/自行车带（步行廊底下） ----
     * 不留大空洞：2F 平台下是一间真的储物间（北面开门），步行廊下是
     * 围起来的设备/自行车位（燃气表、分电盘、车锁、矮门）。
     */
    const TW = 3.16;   // 储物间净高（到平台梁底）
    // 北墙（带门）：设备间门朝北，正对楼背的通道
    put(gbox(0.34, TW, 0.08), M.trunkWall, 31.29, TW / 2, za0 + 0.03);
    put(gbox(0.2, TW, 0.08), M.trunkWall, 32.34, TW / 2, za0 + 0.03);
    put(gbox(0.86, TW - 2.06, 0.08), M.trunkWall, 31.85, 2.06 + (TW - 2.06) / 2, za0 + 0.03);
    put(gbox(0.82, 2.02, 0.05), M.metal, 31.85, 1.01, za0 + 0.03);        // 深灰钢门
    put(gbox(0.92, 2.12, 0.03), M.dark, 31.85, 1.06, za0 + 0.075);        // 门框
    put(gbox(0.14, 0.03, 0.1), M.warmSoft, 31.85, 2.24, za0 + 0.09);      // 门楣小灯
    // 南墙（带两个通风格栅）
    put(gbox(1.32, TW, 0.08), M.trunkWall, 31.78, TW / 2, zb1 + 0.04);
    for (const vx of [31.52, 32.06]) {
      put(gbox(0.28, 0.2, 0.02), M.dark, vx, 2.1, zb1 + 0.005);
      for (let s = 0; s < 3; s++) put(gbox(0.24, 0.018, 0.03), M.metal, vx, 2.04 + s * 0.06, zb1 - 0.005);
    }
    // 东墙（设备墙：燃气表一排 + 分电盘）
    put(gbox(0.08, TW, zb1 - za0 - 0.06), M.trunkWall, 32.42, TW / 2, (za0 + zb1) / 2);
    for (let i = 0; i < 4; i++) {
      put(gbox(0.26, 0.36, 0.16), M.metal, 32.53, 1.32, -6.26 + i * 0.32);   // 燃气表
      put(gbox(0.2, 0.1, 0.03), M.dark, 32.62, 1.32, -6.26 + i * 0.32);      // 表盘
    }
    put(new THREE.CylinderGeometry(0.028, 0.028, 2.3, 6), M.pipe, 32.56, 1.15, -6.95);  // 燃气立管
    put(gbox(0.14, 0.78, 0.46), M.metal, 32.55, 1.55, -7.25);                // 分电盘
    // 南侧围栏（矮坎 + 立杆 + 两道横杆），东端一扇半开的矮门
    put(gbox(5.3, 0.22, 0.1), M.trunkWall, 35.1, 0.11, zb1 - 0.07);
    for (let fx = 32.55; fx <= 36.75; fx += 1.05) {
      put(gbox(0.045, 1.28, 0.045), M.stairSteel, fx, 0.86, zb1 - 0.07);
    }
    put(gbox(4.3, 0.05, 0.05), M.railTop, 34.65, 1.5, zb1 - 0.07);
    put(gbox(4.3, 0.04, 0.04), M.railTop, 34.65, 1.02, zb1 - 0.07);
    boxes.push(boxSpec('stair-fence-rail', 32.45, 37.75, 0, 1.55, zb1 - 0.13, zb1 - 0.01));   // 南侧围栏整排（矮坎+立杆+横杆）当一道实墙收
    {
      const gate = new THREE.Group();
      const gm = (w: number, h: number, d: number, gx: number, gy: number) => {
        const m = new THREE.Mesh(gbox(w, h, d), M.stairSteel);
        m.position.set(gx, gy, 0);
        gate.add(m);
      };
      gm(0.05, 1.25, 0.05, 0, 0.86);
      gm(0.05, 1.25, 0.05, 0.85, 0.86);
      gm(0.9, 0.05, 0.05, 0.42, 1.45);
      gm(0.9, 0.05, 0.05, 0.42, 0.95);
      for (let bi = 1; bi <= 3; bi++) gm(0.04, 1.15, 0.04, bi * 0.21, 0.86);
      gate.position.set(36.82, 0.22, zb1 - 0.07);
      gate.rotation.y = -0.6;   // 半开
      g.add(gate);
      // 栅栏门碰撞：整扇门一个轴对齐盒（半开，按真实世界 AABB 收，避免漏角）
      gate.updateWorldMatrix(true, true);
      const gbox3 = new THREE.Box3().setFromObject(gate);
      boxes.push(boxSpec('stair-fence-gate', gbox3.min.x, gbox3.max.x, gbox3.min.y, gbox3.max.y, gbox3.min.z, gbox3.max.z));
    }
    // 楼梯下的监控（朝西看着车与设备）
    put(gbox(0.1, 0.07, 0.18), toon('#8f97a2'), ex1 - 0.18, F2 - 0.28, zb1 - 0.22, Math.PI / 2);
    put(gbox(0.045, 0.045, 0.03), M.dark, ex1 - 0.27, F2 - 0.28, zb1 - 0.22, Math.PI / 2);
    // 楼下自行车两辆（顶层车棚之外的溢出车位，日本公寓常态）
    bike(33.7, -6.02, 0.07, 8101, true);
    bike(34.85, -5.88, -0.09, 8102, false);
    // 混凝土护脚（楼梯整个落在这块台基上）+ 向南通往楼侧的步道
    flatUp(7.3, 2.9, toon('#79828e', { finish: 'wet' }), 34.6, 0.006, -6.55);
    flatUp(1.1, 3.1, toon('#79828e', { finish: 'wet' }), 37.65, 0.006, -3.55);
    // 少量积水（雨夜）
    puddle(35.4, -6.7, 0.55);
    puddle(33.2, -5.72, 0.4);
  }

  /* ================= 203 段的外墙覆板 =================
   *
   * 203 是自己那户的独立模型，它的外墙是米白墙纸——在整栋冷灰蓝的立面
   * 正中间留一块米白，是"这栋楼被拼接过"最刺眼的证据。这里给它贴一层与
   * 邻居同色的混凝土覆板，并按 203 的洞口开孔（孔比洞口小 2cm，让覆板
   * 自己形成一圈窗套，把米白的洞壁盖住）。
   *
   * 覆板 z 必须盖到 203 南墙的外表面（room shell 南墙内表面 z=4.8 +
   * wallThickness=0.12 = 4.92），把露到外面来的米白 reveal 断面包住；
   * 邻居的覆板中心在 ZF+0.09=4.79（他们没有 reveal 凸出），203 单独
   * 推 6.5cm，在 z=ZF+0.155=4.855，外表面与房间外墙齐平。
   *
   * 洞口/窗套坐标必须与 dormLayout.json 的 south 墙当前 openings 严格
   * 对齐（世界标高 = 室内 y + UNIT_LIFT 3.4），覆板跟着房间布局走，
   * 不要再回填老布局的数。
   *
   * y 上下各让 4cm：与 2F 楼面、3F 楼面共用标高会 z-fighting。
   */
  const CLAD_Y0 = F2 + GAP, CLAD_Y1 = F2 + LVL - GAP;

  // 南面：四樘洞口（世界标高 = 室内 y + UNIT_LIFT 3.4）。这些数必须与
  // dormLayout.json 的 south 墙 openings 一一对应，误差走房间布局，不写死猜值。
  const SOUTH_Z = ZF + 0.155;          // 4.855：覆板外表面与 203 南墙外表面齐平（4.8 + 0.12 - 0.065）
  const TRIM_Z = SOUTH_Z + 0.06;       // 窗套贴在覆板表面外
  const SILL_Z = SOUTH_Z + 0.11;       // 窗台比窗套再外挑 5cm
  const CLAD_T = 0.13;

  // 洞 = 当前 openings 各内缩 2cm（让覆板自己形成 2cm 窗套覆盖洞壁）
  // 套 = 当前 openings 原样（套料宽 fw=0.05 会再向外扩 5cm）
  facadePanel(-6.24, 6.24, CLAD_Y0, CLAD_Y1, SOUTH_Z, CLAD_T, M.wall, [
    { a: -5.38, b: -4.02, y0: 3.87, y1: 5.73 },   // 腰窗（master）
    { a: -1.18, b:  1.18, y0: 3.47, y1: 5.78 },   // 玻璃滑门（通阳台）
    { a:  2.02, b:  3.18, y0: 3.87, y1: 5.73 },   // 腰窗（dining）
    { a:  4.32, b:  5.28, y0: 4.27, y1: 5.73 },   // 高窗（kitchen）
  ]);
  // 洞口四周补一圈窗套：从街上读出"这里有一樘窗"，而不是一个裸洞
  for (const h of [
    { a: -5.40, b: -4.00, y0: 3.85, y1: 5.75 },
    { a: -1.20, b:  1.20, y0: 3.45, y1: 5.80 },
    { a:  2.00, b:  3.20, y0: 3.85, y1: 5.75 },
    { a:  4.30, b:  5.30, y0: 4.25, y1: 5.75 },
  ]) {
    const fw = 0.05;
    put(gbox(h.b - h.a + fw * 2, fw, 0.05), M.frame, (h.a + h.b) / 2, h.y1 + fw / 2, TRIM_Z);
    put(gbox(h.b - h.a + fw * 2, fw, 0.05), M.frame, (h.a + h.b) / 2, h.y0 - fw / 2, TRIM_Z);
    put(gbox(fw, h.y1 - h.y0, 0.05), M.frame, h.a - fw / 2, (h.y0 + h.y1) / 2, TRIM_Z);
    put(gbox(fw, h.y1 - h.y0, 0.05), M.frame, h.b + fw / 2, (h.y0 + h.y1) / 2, TRIM_Z);
    // 窗台（玻璃门那樘是落地，不做窗台）
    if (h.y0 > 3.6) {
      put(gbox(h.b - h.a + 0.14, 0.05, 0.16), M.slabDark, (h.a + h.b) / 2, h.y0 - fw + 0.01, SILL_Z);
    }
  }

  // 北面不在这里建覆板：203 的北面覆板由上面楼层循环里的 unitFace 一处负责
  // （z=ZN-0.09、洞口取 dormLayout 北墙 openings）。这里再建一层同 z 同厚度的板
  // 会与它 z-fighting，而且洞口一旦不是同一套坐标就把窗糊死。本段只管外廊侧的
  // 门牌这类挂件。
  // 户号牌：主角单元是 203，门楣挂 "203"（与邻户牌同款微背光，dir=-1 朝外廊）
  panel(0.19, 0.14, emissive('#d9cfbc', { map: numberPlateTexture('203') }), 5.41, F2 + 1.78, ZN - 0.165, -1);
  // 户主牌（表札）：203 是主角 Vivian Nana（与邻户牌同款、dir=-1 朝外廊，贴门牌右侧）
  panel(0.30, 0.12, emissive('#fff6ea', { map: namePlateTexture(RESIDENTS['203']) }), 5.75, F2 + 1.78, ZN - 0.165, -1);

  /* ================= 底层：入口 + 生活设施 =================
   *
   * 入口退进到 z4.7 的立面（与上层阳台内缘同一条线），上面就是 2 层的
   * 阳台板——这是日式公寓最常见的"エントランス在上层阳台的荫里"。
   * 信箱 / 快递柜 / 表箱 / 消火栓 / 监控全在入口两侧——日式公寓一楼的
   * "生活前台"全集中在这 12m 里。
   */
  // 雨棚 + 檐下暖光灯带（朝下）+ 入口上方的楼层标牌
  put(gbox(5.6, 0.14, 1.55), M.slabDry, 0, 2.74, ZF + 0.775);
  flatDown(4.8, 0.34, M.hall, 0, 2.64, ZF + 0.62);
  put(gbox(5.6, 0.05, 0.05), M.dark, 0, 2.63, ZF + 1.53);   // 雨棚滴水线
  panel(1.5, 0.26, M.wallDeep, 0, 3.05, ZF + 0.02, 1);      // 楼栋名标牌底
  panel(1.2, 0.16, M.warmPale, 0, 3.05, ZF + 0.04, 1);      // 标牌（柔光）

  // 自动玻璃双开门 + 门框 + 门禁面板
  panel(1.8, 2.2, toon('#cfe0ee', { finish: 'glass', transparent: true, opacity: 0.22, depthWrite: false }),
    0, 1.12, ZF + 0.03, 1);
  for (const dx of [-0.95, 0.95]) put(gbox(0.09, 2.34, 0.09), M.frame, dx, 1.17, ZF + 0.04);
  put(gbox(2.08, 0.12, 0.12), M.frame, 0, 2.36, ZF + 0.04);
  put(gbox(0.16, 0.24, 0.06), M.metal, 1.2, 1.35, ZF + 0.05);
  panel(0.1, 0.14, M.cool, 1.2, 1.4, ZF + 0.09, 1);
  // 监控摄像头（雨棚下，朝街）
  put(gbox(0.12, 0.08, 0.2), toon('#8f97a2'), 2.2, 2.52, ZF + 1.1);
  put(gbox(0.05, 0.05, 0.03), M.dark, 2.2, 2.5, ZF + 1.2);

  // 信箱柜：一整排格口（入口左）
  put(gbox(2.8, 1.5, 0.4), toon('#7c8794'), -4.6, 0.85, ZF + 0.22);
  boxes.push(boxSpec('entrance-mailbox', -6.0, -3.2, 0.10, 1.60, ZF + 0.02, ZF + 0.42));   // 柜体碰撞
  for (let r = 0; r < 4; r++) {
    for (let c = 0; c < 6; c++) {
      panel(0.38, 0.28, toon('#9aa5b1'), -5.75 + c * 0.46, 0.35 + r * 0.34, ZF + 0.43, 1);
    }
  }
  // 快递柜（入口右）：更大的格口 + 操作屏
  put(gbox(2.2, 1.8, 0.5), toon('#5f6b7d'), 4.6, 0.95, ZF + 0.27);
  boxes.push(boxSpec('entrance-parcel', 3.5, 5.7, 0.05, 1.85, ZF + 0.02, ZF + 0.52));       // 柜体碰撞
  for (let r = 0; r < 3; r++) {
    for (let c = 0; c < 4; c++) {
      panel(0.42, 0.42, toon('#7d8998'), 3.95 + c * 0.48, 0.45 + r * 0.52, ZF + 0.53, 1);
    }
  }
  panel(0.3, 0.22, M.cool, 5.35, 1.55, ZF + 0.53, 1);
  // 公告栏（信箱旁）+ 消火栓箱
  put(gbox(1.0, 1.15, 0.08), M.woodDeep, -6.9, 1.15, ZF + 0.04);
  boxes.push(boxSpec('entrance-bulletin', -7.4, -6.4, 0.575, 1.725, ZF, ZF + 0.08));  // 公告栏（薄板）
  for (let i = 0; i < 3; i++) panel(0.24, 0.32, toon('#c3c8ce'), -7.18 + i * 0.28, 1.18, ZF + 0.09, 1);
  put(gbox(0.5, 0.65, 0.2), toon('#9c5040'), 6.6, 0.55, ZF + 0.12);
  boxes.push(boxSpec('entrance-hydrant', 6.35, 6.85, 0.225, 0.875, ZF + 0.02, ZF + 0.22));    // 消火栓箱
  // 电表箱 / 燃气表箱：靠入口的一小排金属箱
  put(gbox(0.5, 0.6, 0.16), M.metal, -2.6, 1.5, ZF + 0.08);
  boxes.push(boxSpec('entrance-meter-1', -2.85, -2.35, 1.2, 1.8, ZF, ZF + 0.16));
  put(gbox(0.42, 0.5, 0.16), M.metal, -2.05, 1.45, ZF + 0.08);
  boxes.push(boxSpec('entrance-meter-2', -2.26, -1.84, 1.2, 1.7, ZF, ZF + 0.16));

  /* ---- 1F 立面：四户窗 + 入口门厅，窗后都是能看进去的屋子 ----
   * 1F 的实心体量南端同样退了 ROOM_D（与 2~4F 同一套做法），这里把外皮
   * 补回 z=ZF 并封成房间：四户各一扇低窗台窗（一半带防盗格栅），中间入口
   * 段是门厅。玻璃走透明档——从街上能看进屋，"白模 + 一片死玻璃"的观感
   * 就是从这里来的。这些区域不可到达，一件都不产生碰撞盒。 */
  const GF = 0;          // 1F 地面标高
  const GH = F2 - GAP;   // 1F 净高 3.36（比标准层 2.8 高一档）
  for (let ui = 0; ui < UNITS.length; ui++) {
    if (ui === 2) continue;              // 中户是入口门厅，下面单独建
    const [ux0, ux1] = UNITS[ui];
    const wx = (ux0 + ux1) / 2;          // 窗落在户中心
    const gState: WinState = (() => {
      const r = rnd();
      return r < 0.22 ? 'curtain' : r < 0.55 ? 'lit' : r < 0.8 ? 'dark' : 'lamp';
    })();
    unitInterior({
      ux0, ux1, F: GF, h: GH,
      holes: [{ a: wx - 0.575, b: wx + 0.575, y0: 0.53, y1: 2.03 }],
      lit: gState === 'lit',
      livFurnish: gState !== 'curtain',
      cloth: rnd() < 0.5 ? M.rmClothLit : M.rmClothAlt,
      seed: 7700 + ui,
      facade: M.wallBase,                // 1F 结构板跟基座同色，读作底层
      furnish: furnishGroundUnit,        // 1F 只摆窗后那一段
    });
    windowUnit({
      x: wx, y: 1.28, z: ZF, dir: 1, w: 1.15, h: 1.5,
      state: gState,
      seeThrough: true, curtain: true,
    });
    // 防盗格栅：一半的户有（1 层住户的痕迹），凸出墙面 16cm
    if (ui % 2 === 0) {
      for (let b = 0; b < 4; b++) put(gbox(0.03, 1.52, 0.03), M.metal, wx - 0.42 + b * 0.28, 1.28, ZF + 0.16);
      put(gbox(1.2, 0.03, 0.03), M.metal, wx, 2.0, ZF + 0.16);
    }
  }

  /* ---- 入口门厅：整栋楼唯一一处能看进去的公共空间 ----
   * 自动双开门是透明玻璃（opacity 0.22），后面如果是一整片素墙，从街上
   * 看就是"门上糊了一块底色"。这里封成一间真的门厅：瓷砖地 + 电梯 +
   * 楼梯口 + 常明灯 + 消火栓，进出楼的动作才有落点。
   * 净深只有 0.88m，够放电梯门和一段暗示性的楼梯起步，不需要真楼梯。 */
  {
    const ex0 = -6.2, ex1 = 6.2;
    const ew = ex1 - ex0;
    const zBack = ZF - ROOM_D, zMid = ZF - ROOM_D / 2;
    const HM = RM.lit;                   // 门厅恒亮：公共空间不该是黑屋
    // 门厅地面直接复用外廊那档防滑层：公共空间同一种铺装，雨夜带水的鞋底
    // 踩上去本来就该是同一块微湿的浅灰（顺带省下一个材质 / 一次 draw call）。
    const tile = M.walkFloor;
    // 结构板：外皮补回 z=ZF，中间留自动门的洞（x∈[-1,1]，高 2.22）
    facadePanel(ex0 + 0.04, ex1 - 0.04, GF + GAP, GH - GAP, ZF - 0.06, 0.12, M.wallBase,
      [{ a: -1.0, b: 1.0, y0: GF + GAP, y1: 2.22 }]);
    // 内衬：后壁 / 顶 / 地 / 西侧隔板（东侧那道由 ui=3 的 unitInterior 补）
    put(gbox(ew - 0.08, GH - GAP * 2, 0.06), HM.wall, 0, GH / 2, zBack + 0.03);
    put(gbox(ew - 0.08, 0.06, ROOM_D), HM.wall, 0, GH - GAP - 0.03, zMid);
    put(gbox(ew - 0.08, 0.06, ROOM_D), tile, 0, GF + GAP + 0.03, zMid);
    put(gbox(0.06, GH - GAP * 2, ROOM_D), HM.wall, ex0 + 0.03, GH / 2, zMid);

    // 电梯：不锈钢双开门 + 门套 + 呼叫面板 + 门上楼层指示灯
    put(gbox(1.9, 2.5, 0.05), M.frame, -3.0, 1.25, zBack + 0.06);
    put(gbox(1.6, 2.2, 0.04), M.metal, -3.0, 1.12, zBack + 0.09);
    put(gbox(0.02, 2.2, 0.02), M.dark, -3.0, 1.12, zBack + 0.115);      // 中缝
    put(gbox(0.11, 0.26, 0.03), M.metal, -2.0, 1.4, zBack + 0.115);     // 呼叫面板
    panel(0.26, 0.09, M.cool, -3.0, 2.52, zBack + 0.12, 1);             // 楼层指示灯

    // 楼梯口：四级起步台阶从门口（南，低）往后壁（北，高）抬起 + 单侧扶手。
    // 净深只有 0.88m，够暗示"上楼的起步"就够了，不做真梯。
    for (let s = 0; s < 4; s++) {
      put(gbox(1.5, 0.18, 0.2), tile, 3.4, 0.19 + s * 0.18, zBack + 0.75 - s * 0.2);
    }
    put(gbox(1.5, 0.06, 0.18), tile, 3.4, 0.79, zBack + 0.11);          // 接住第四级的休息平台
    {
      const rise = 0.72, run = 0.6;                                     // 四级总起高 / 总进深
      const hand = new THREE.Mesh(gbox(0.045, 0.045, Math.hypot(rise, run)), M.metal);
      hand.position.set(2.62, 1.45, zBack + 0.45);
      hand.rotation.x = Math.atan2(rise, run);                          // +z 端（靠门）低
      g.add(hand);
      put(gbox(0.05, 1.18, 0.05), M.metal, 2.62, 0.59, zBack + 0.75);   // 南立柱
      put(gbox(0.05, 1.72, 0.05), M.metal, 2.62, 0.86, zBack + 0.15);   // 北立柱
    }

    // 常明吸顶灯两盏 + 门内地垫：公共空间永远有人管
    put(gbox(0.5, 0.06, 0.24), M.warmPale, -1.6, GH - 0.35, zMid + 0.1);
    put(gbox(0.5, 0.06, 0.24), M.warmPale, 2.2, GH - 0.35, zMid + 0.1);
    put(gbox(2.0, 0.02, 0.6), M.dark, 0, GAP + 0.07, ZF - 0.7);         // 门内地垫
    // 门厅消火栓 + 一块通告牌：日式公寓一进门那点"管理感"
    put(gbox(0.5, 0.65, 0.2), toon('#9c5040'), -5.4, 0.36, zBack + 0.14);
    put(gbox(0.7, 0.5, 0.03), M.slabDry, 5.2, 1.6, zBack + 0.12);
  }
  // 102/104 两户的小庭院：低围栏 + 半开小门，把 1F 与楼上的阳台区分开
  for (const fcx of [-12.4, 12.4]) {
    const fz = ZF + 1.25;
    for (let fx = fcx - 1.15; fx <= fcx + 1.15; fx += 0.575) {
      if (Math.abs(fx - fcx) < 0.5) continue;   // 门位
      put(gbox(0.045, 0.55, 0.045), M.stairSteel, fx, 0.275, fz);
    }
    put(gbox(2.3, 0.04, 0.04), M.railTop, fcx, 0.5, fz);
    put(gbox(2.3, 0.04, 0.04), M.railTop, fcx, 0.27, fz);
    // 半开的庭院小门
    const ygate = new THREE.Group();
    for (const gy of [0.06, 0.28, 0.5]) {
      const bar = new THREE.Mesh(gbox(0.04, 0.04, 0.85), M.stairSteel);
      bar.position.set(0, gy, 0.42);
      ygate.add(bar);
    }
    const ybar = new THREE.Mesh(gbox(0.04, 0.55, 0.04), M.stairSteel);
    ybar.position.set(0, 0.28, 0.82);
    ygate.add(ybar);
    ygate.position.set(fcx - 0.45, 0, fz);
    ygate.rotation.y = -0.7;
    g.add(ygate);
  }

  /* ================= 1F 北面：一排小店店面（10 间） =================
   *
   * 日本公寓楼下沿街一面常是一排小店——每户 12.4m 切成东西两间、各约 6.2m 宽，
   * 全楼 5 户 = 10 间店面。夜里雨夜从街上走过，橱窗透出来的暖光和招牌是街景
   * 里最有人气的那一层。1F 体量北端已退 ROOM_D，这里把外皮补回 z=ZN 并封成
   * 店面：每间一樘大橱窗 + 一扇店门，里头是低模陈列。
   *
   * 招牌走 atlas（1 张贴图 10 行），每间用 UV offset 选自己的那行 → 只占 1 个桶。 */
  const HALF_W = (UNITS[0][1] - UNITS[0][0]) / 2;   // 6.2m：每户半宽 = 一间店面
  const SHOPS: Array<{
    kind: number; cx: number; uw: number;
    winW: number; winY0: number; winY1: number;
    doorW: number; doorH: number;
    lit: boolean; cloth: THREE.Material; seed: number;
  }> = (() => {
    const arr: Array<typeof SHOPS[number]> = [];
    // 10 间店面沿 x 均匀排列，店类交错排布让相邻店视觉差异最大化
    const KINDS = [0, 5, 1, 6, 2, 7, 3, 8, 4, 9];  // 电玩→蜜雪→面包→居酒→拉面→花屋→书店→薬局→干洗→便利
    const LIT    = [true, true, true, true, true, false, false, true, true, true];
    const CLOTHS = [M.rmClothLit, M.rmClothAlt, M.rmClothLit, M.rmClothAlt, M.rmClothLit,
                    M.rmClothAlt, M.rmClothLit, M.rmClothAlt, M.rmClothLit, M.rmClothAlt];
    let si = 0;
    for (let ui = 0; ui < UNITS.length; ui++) {
      const [ux0, ux1] = UNITS[ui];
      for (const half of [0, 1]) {
        const cx = half === 0 ? (ux0 + ux0 + HALF_W) / 2 : (ux0 + HALF_W + ux1) / 2;
        arr.push({
          kind: KINDS[si], cx,
          uw: HALF_W - 1.20,   // 店面缩进，每侧留 0.60m 做柱/墙缝（日本小店节奏：~5m 店 + ~1.2m 间距）
          winW: HALF_W - 2.60,   // 橱窗 = 店面宽 - 门宽 - 边距
          winY0: 0.45, winY1: 2.40,
          doorW: 0.9, doorH: 2.30,
          lit: LIT[si], cloth: CLOTHS[si], seed: 10000 + si * 37,
        });
        si++;
      }
    }
    return arr;
  })();
  const SIGN_H = 0.36;
  const SIGN_Y = GH - SIGN_H / 2 - 0.04;
  const AWN_Y = GH - 0.06;
  const zBack = ZN + ROOM_D;
  const zMidN = ZN + ROOM_D / 2;
  const zCfN = ZN + ROOM_D / 2 + 0.008;   // 店面天花板/地板微量前移，避免与结构板共面
  const shopMats = M;

  for (const s of SHOPS) {
    const cx = s.cx;
    const uw = s.uw;
    const r = makeRng(s.seed);
    const HM = s.lit ? RM.lit : RM.dark;
    // 橱窗居中偏左、门在右
    const winCx = cx - (s.winW + s.doorW) / 2 + s.winW / 2;
    const doorCx = cx + (s.winW + s.doorW) / 2 - s.doorW / 2;
    const ux0 = cx - uw / 2, ux1 = cx + uw / 2;
    const holes = [
      { a: winCx - s.winW / 2, b: winCx + s.winW / 2, y0: s.winY0, y1: s.winY1 },
      { a: doorCx - s.doorW / 2, b: doorCx + s.doorW / 2, y0: 0, y1: s.doorH },
    ];
    // 1) 结构板
    facadePanel(ux0 + 0.04, ux1 - 0.04, GF + GAP, GH - GAP, ZN + 0.06, 0.12, M.wallBase, holes);
    // 2) 内衬
    put(gbox(uw - 0.08, GH - GAP * 2, 0.06), HM.wall, cx, GF + GH / 2, zBack - 0.03);
    put(gbox(uw - 0.08, 0.06, ROOM_D), HM.wall, cx, GF + GH - GAP - 0.03, zCfN);
    put(gbox(uw - 0.08, 0.06, ROOM_D), HM.floor, cx, GF + GAP + 0.03, zCfN);
    // 3) 分户隔板（最西端是山墙不画）
    if (ux0 !== X0) {
      put(gbox(0.06, GH - GAP * 2, ROOM_D), HM.wall, ux0 + 0.03, GF + GH / 2, zCfN);
    }
    // 4) 店内陈列：10 种店各有特色，全复用 M.* 现有桶
    const atN = (dx: number, dy: number, dz: number) => [cx + dx, GF + dy, zBack - 0.09 - dz] as const;
    if (s.kind === 0) {
      // 电玩店：两台街机 + 投币台
      for (const ax of [-0.8, 0.8]) {
        put(gbox(0.46, 1.50, 0.56), shopMats.dark, ...atN(ax, 0.75, 0.10));
        panel(0.32, 0.24, shopMats.cool, cx + ax, GF + 1.50, zBack - 0.10, -1);
      }
      put(gbox(1.0, 0.36, 0.44), shopMats.metal, ...atN(0, 0.18, 0.30));
    } else if (s.kind === 1) {
      // 面包店：展示柜 + 面包 + 货架
      put(gbox(s.winW - 0.6, 0.80, 0.46), shopMats.wood, ...atN(-0.3, 0.40, 0.12));
      put(gbox(s.winW - 0.6, 0.06, 0.48), shopMats.warmPale, ...atN(-0.3, 0.83, 0.12));
      for (let i = 0; i < 3; i++) put(gbox(0.26, 0.07, 0.20), shopMats.warmPale, ...atN(-0.8 + i * 0.5, 0.86, 0.18));
      put(gbox(1.2, 1.6, 0.38), shopMats.wood, ...atN(1.6, 0.80, 0.08));
    } else if (s.kind === 2) {
      // 拉面店：吧台 + 吧凳 + 食券机 + 暖帘
      put(gbox(3.0, 0.85, 0.46), shopMats.wood, ...atN(-0.3, 0.43, 0.12));
      put(gbox(3.0, 0.06, 0.48), shopMats.warmPale, ...atN(-0.3, 0.88, 0.12));
      for (let i = 0; i < 3; i++) put(gbox(0.34, 0.42, 0.34), shopMats.wood, ...atN(-1.0 + i * 0.7, 0.21, 0.36));
      put(gbox(0.44, 1.40, 0.28), shopMats.metal, ...atN(1.5, 0.70, 0.08));
      for (let i = 0; i < 2; i++) put(gbox(0.50, 0.50, 0.03), s.lit ? s.cloth : shopMats.dark, cx - 0.5 + i * 0.55, GF + s.winY0 + 0.28, ZN + 0.10);
    } else if (s.kind === 3) {
      // 书店：书架 + 书脊 + 阅读桌
      put(gbox(2.0, 1.6, 0.32), shopMats.wood, ...atN(-0.3, 0.80, 0.08));
      const bookMats = [shopMats.rmClothLit, shopMats.rmClothAlt, shopMats.wood, shopMats.woodDeep, shopMats.dark];
      for (let row = 0; row < 3; row++) for (let col = 0; col < 4; col++)
        put(gbox(0.32, 0.04, 0.26), bookMats[(row*4+col)%5], ...atN(-1.2 + col*0.45, 0.28 + row*0.45, 0.14));
      put(gbox(0.8, 0.68, 0.54), shopMats.wood, ...atN(1.6, 0.34, 0.20));
    } else if (s.kind === 4) {
      // 干洗店：旋筒 + 挂衣 + 柜台
      put(gbox(0.06, 1.6, 1.6), shopMats.metal, ...atN(-0.3, 0.80, 0.08));
      for (let i = 0; i < 4; i++) put(gbox(0.04, 0.02, 0.36), shopMats.metal, ...atN(-0.3 + (i-1.5)*0.20, 1.55, 0.08));
      put(gbox(0.8, 0.80, 0.46), shopMats.wood, ...atN(1.6, 0.40, 0.12));
      for (let i = 0; i < 3; i++) put(gbox(0.24, 0.50, 0.04), s.lit ? s.cloth : shopMats.dark, ...atN(-0.3 + (i-1)*0.20, 1.25, 0.15));
    } else if (s.kind === 5) {
      // 蜜雪冰城：制冰机 + 操作台 + 菜单灯箱 + 桌椅
      put(gbox(0.70, 1.20, 0.50), shopMats.metal, ...atN(-1.2, 0.60, 0.10));    // 制冰机
      put(gbox(1.6, 0.85, 0.46), shopMats.wood, ...atN(0.2, 0.43, 0.12));        // 操作台
      put(gbox(1.6, 0.06, 0.48), shopMats.warmPale, ...atN(0.2, 0.88, 0.12));
      panel(0.80, 0.50, shopMats.warmSoft, cx + 1.5, GF + 1.70, ZN + 0.12, -1); // 菜单灯箱
      // 两张小桌 + 椅
      for (const tx of [-0.5, 0.8]) { put(gbox(0.50, 0.68, 0.46), shopMats.wood, ...atN(tx, 0.34, 0.32)); put(gbox(0.30, 0.40, 0.30), shopMats.wood, ...atN(tx, 0.20, 0.52)); }
    } else if (s.kind === 6) {
      // 居酒屋：吧台 + 酒瓶架 + 红灯笼 + 暖帘
      put(gbox(2.8, 0.85, 0.46), shopMats.wood, ...atN(-0.2, 0.43, 0.12));
      put(gbox(2.8, 0.06, 0.48), shopMats.warmPale, ...atN(-0.2, 0.88, 0.12));
      for (let i = 0; i < 5; i++) put(gbox(0.10, 0.30, 0.10), [shopMats.dark, shopMats.woodDeep, shopMats.rmClothAlt][i%3], ...atN(-1.0 + i*0.45, 1.30, 0.06)); // 酒瓶
      // 红灯笼
      put(gbox(0.16, 0.22, 0.16), shopMats.warmSoft, cx - 1.5, GF + 2.00, ZN + 0.14);
      put(gbox(0.16, 0.22, 0.16), shopMats.warmSoft, cx + 1.5, GF + 2.00, ZN + 0.14);
      // 暖帘
      for (let i = 0; i < 2; i++) put(gbox(0.80, 0.60, 0.03), s.lit ? s.cloth : shopMats.dark, cx - 0.4 + i * 0.84, GF + s.winY0 + 0.32, ZN + 0.10);
    } else if (s.kind === 7) {
      // 花屋：花桶 + 货架 + 鲜花
      for (let i = 0; i < 4; i++) {
        put(gbox(0.22, 0.50, 0.22), shopMats.wood, ...atN(-1.2 + i * 0.55, 0.25, 0.18));
        put(gbox(0.18, 0.30, 0.18), [shopMats.rmClothLit, shopMats.rmClothAlt, shopMats.leaf, shopMats.leafWarm][i%4], ...atN(-1.2 + i * 0.55, 0.58, 0.18));
      }
      put(gbox(1.2, 1.0, 0.36), shopMats.wood, ...atN(1.4, 0.50, 0.08));
      for (let i = 0; i < 3; i++) put(gbox(0.20, 0.20, 0.20), [shopMats.leaf, shopMats.leafDeep, shopMats.leafWarm][i], ...atN(1.0 + i*0.3, 1.10, 0.10));
    } else if (s.kind === 8) {
      // 薬局：药品架 + 柜台 + 绿十字灯
      put(gbox(2.0, 1.8, 0.32), shopMats.wood, ...atN(-0.2, 0.90, 0.08));
      for (let row = 0; row < 3; row++) for (let col = 0; col < 4; col++)
        put(gbox(0.28, 0.08, 0.24), shopMats.warmPale, ...atN(-1.0 + col*0.45, 0.30 + row*0.50, 0.12));
      put(gbox(0.9, 0.80, 0.46), shopMats.wood, ...atN(1.6, 0.40, 0.12));
      panel(0.20, 0.20, shopMats.cool, cx + 1.5, GF + 2.10, ZN + 0.12, -1);    // 绿十字灯
    } else {
      // コンビニ：货架 + 冷柜 + 收银台
      put(gbox(1.6, 1.8, 0.36), shopMats.wood, ...atN(-0.6, 0.90, 0.08));      // 货架
      put(gbox(0.70, 1.6, 0.50), shopMats.metal, ...atN(0.8, 0.80, 0.08));     // 冷柜
      put(gbox(0.80, 0.85, 0.46), shopMats.wood, ...atN(1.8, 0.43, 0.12));     // 收银台
      put(gbox(0.26, 0.36, 0.26), shopMats.warmPale, ...atN(-0.6, 0.36, 0.32)); // 杂货堆
    }
    // 店内吸顶灯
    if (s.lit) {
      put(gbox(0.36, 0.06, 0.36), shopMats.warmPale, cx, GF + GH - GAP - 0.08, zMidN);
      if (r() < 0.5) put(gbox(0.22, 0.06, 0.22), shopMats.warmPale, cx - 1.0, GF + GH - GAP - 0.08, zMidN);
    }

    // 5) 招牌：atlas 贴图 + UV offset 选第 kind 行
    {
      const signGeo = new THREE.PlaneGeometry(uw - 0.16, SIGN_H);
      shopSignUV(signGeo, s.kind);
      const sign = new THREE.Mesh(signGeo, shopMats.shopSign);
      sign.position.set(cx, SIGN_Y, ZN - 0.02);
      sign.rotation.y = Math.PI;   // 朝 -Z（外廊 / 街面）
      g.add(sign);
    }
    // 6) 雨棚
    {
      const awn = new THREE.Mesh(gbox(uw - 0.08, 0.05, 0.70), shopMats.slabDark);
      awn.position.set(cx, AWN_Y, ZN - 0.34);
      awn.rotation.x = 0.06;
      g.add(awn);
      flatDown(uw - 0.16, 0.20, shopMats.hall, cx, AWN_Y - 0.02, ZN - 0.60);
    }
    // 7) 橱窗玻璃
    windowUnit({
      x: winCx, y: (s.winY0 + s.winY1) / 2, z: ZN, dir: -1,
      w: s.winW, h: s.winY1 - s.winY0,
      seeThrough: true, curtain: false, noFrame: false,
      paneZ: ZN - 0.01, state: s.lit ? 'lit' : 'dark', kind: s.kind,
    });
    // 8) 店门
    put(gbox(s.doorW - 0.04, s.doorH - 0.04, 0.05), shopMats.woodDeep, doorCx, GF + s.doorH / 2, ZN - 0.02);
    put(gbox(s.doorW + 0.04, 0.06, 0.06), shopMats.frame, doorCx, GF + s.doorH, ZN - 0.02);
    for (const dx of [-s.doorW / 2 - 0.03, s.doorW / 2 + 0.03]) {
      put(gbox(0.06, s.doorH, 0.06), shopMats.frame, doorCx + dx, GF + s.doorH / 2, ZN - 0.02);
    }
    put(gbox(0.04, 0.04, 0.03), shopMats.metal, doorCx + s.doorW / 2 - 0.10, GF + 1.05, ZN - 0.04);
    if (s.lit) panel(0.10, 0.10, shopMats.warmSoft, doorCx, GF + s.doorH + 0.10, ZN - 0.03, -1);
  }

  /* ---------------- 自行车棚（入口西侧） ----------------
   * 平顶棚 + 立柱 + 车轮锁。自行车"整齐但不完全对齐"——每辆的位置、
   * 朝向、有没有车筐都带一点随机，是住户口味的痕迹。
   */
  put(gbox(7.0, 0.08, 2.0), toon('#79828e'), -12.5, 2.0, 7.1);
  put(gbox(7.1, 0.06, 0.06), M.dark, -12.5, 1.95, 6.12);   // 棚口滴水线
  for (const px of [-15.8, -12.5, -9.2]) {
    put(gbox(0.08, 2.0, 0.08), M.metal, px, 1.0, 6.35);
    put(gbox(0.08, 2.0, 0.08), M.metal, px, 1.0, 7.85);
  }
  for (let i = 0; i < 5; i++) {
    const bxp = -15.0 + i * 1.3 + (makeRng(9100 + i)() - 0.5) * 0.22;
    const rot = (makeRng(9200 + i)() - 0.5) * 0.24 + (i % 2 === 0 ? 0.04 : -0.05);
    bike(bxp, 7.06 + (makeRng(9300 + i)() - 0.5) * 0.14, rot, 9400 + i, i % 3 === 0);
    put(gbox(0.1, 0.05, 0.36), M.pipe, bxp - 0.1, 0.02, 7.32);   // 地面轮挡（锁位）
  }
  // 车棚下的监控（朝东看着车）+ 少量积水
  put(gbox(0.1, 0.07, 0.18), toon('#8f97a2'), -15.6, 1.82, 6.5, Math.PI / 2);
  put(gbox(0.045, 0.045, 0.03), M.dark, -15.69, 1.82, 6.5, Math.PI / 2);
  puddle(-13.9, 7.05, 0.75);
  puddle(-11.1, 7.35, 0.5);
  puddle(-14.6, 7.5, 0.45);

  /* ---------------- 垃圾分类区（入口东侧：顶棚 + 金属网罩 + 分类桶） ----------------
   * 体量刻意小、不正对入口：四根钢柱撑一片向内微倾的暗色顶棚，三面
   * 金属网（细杆读作网），南面敞开做人行。桶按可燃/不可燃/资源分类。
   */
  {
    const gx0 = 10.3, gx1 = 12.9, gz0 = 6.5, gz1 = 8.05;
    flatUp(gx1 - gx0 + 0.3, gz1 - gz0 + 0.3, toon('#79828e', { finish: 'wet' }), (gx0 + gx1) / 2, 0.006, (gz0 + gz1) / 2);
    for (const [px, pz] of [[gx0, gz0], [gx1, gz0], [gx0, gz1], [gx1, gz1]] as Array<[number, number]>) {
      put(gbox(0.09, 2.1, 0.09), M.stairSteel, px, 1.05, pz);
    }
    // 顶棚：向内（北）微倾，雨水排进楼侧而不是人行道
    const roof = new THREE.Mesh(gbox(gx1 - gx0 + 0.5, 0.05, gz1 - gz0 + 0.45), M.slabDark);
    roof.position.set((gx0 + gx1) / 2, 2.18, (gz0 + gz1) / 2);
    roof.rotation.x = -0.08;
    g.add(roof);
    put(gbox(gx1 - gx0 + 0.5, 0.05, 0.05), M.dark, (gx0 + gx1) / 2, 2.13, gz1 + 0.2);   // 棚口滴水线
    // 北面（背面）：混凝土矮墙 + 上部金属网
    put(gbox(gx1 - gx0, 1.05, 0.08), M.trunkWall, (gx0 + gx1) / 2, 0.525, gz0);
    for (let bx = gx0 + 0.1; bx <= gx1 - 0.1; bx += 0.17) {
      put(gbox(0.016, 1.0, 0.016), M.stairSteel, bx, 1.6, gz0);
    }
    put(gbox(gx1 - gx0, 0.035, 0.035), M.stairSteel, (gx0 + gx1) / 2, 2.08, gz0);
    put(gbox(gx1 - gx0, 0.035, 0.035), M.stairSteel, (gx0 + gx1) / 2, 1.12, gz0);
    // 东西两面金属网
    for (const px of [gx0, gx1]) {
      for (let bz = gz0 + 0.17; bz <= gz1 - 0.1; bz += 0.17) {
        put(gbox(0.016, 2.05, 0.016), M.stairSteel, px, 1.025, bz);
      }
      put(gbox(0.035, 0.035, gz1 - gz0), M.stairSteel, px, 2.0, (gz0 + gz1) / 2);
      put(gbox(0.035, 0.035, gz1 - gz0), M.stairSteel, px, 0.35, (gz0 + gz1) / 2);
    }
    // 分类桶：可燃 / 不可燃 / 资源——颜色压暗，与整体低饱和一致
    const binTones = ['#41505f', '#465446', '#4a4a52'];
    for (let i = 0; i < 3; i++) {
      const bx = 10.78 + i * 0.68;
      put(gbox(0.56, 0.8, 0.55), toon(binTones[i]), bx, 0.4, 6.98);
      put(gbox(0.58, 0.1, 0.57), toon('#5d6570'), bx, 0.85, 6.98);   // 桶盖
      put(gbox(0.3, 0.05, 0.06), M.dark, bx, 0.92, 6.72);           // 盖把手
    }
    // 资源回收筐（开口网筐）+ 两袋收好的垃圾 + 立牌
    put(gbox(0.52, 0.34, 0.42), toon('#79828c'), 12.5, 0.17, 7.5);
    put(gbox(0.5, 0.06, 0.4), M.metal, 12.5, 0.4, 7.5);
    for (const [bxp, c] of [[11.0, '#b9bcc0'], [11.7, '#a8aba6']] as Array<[number, string]>) {
      const bag = new THREE.Mesh(new THREE.SphereGeometry(0.28, 8, 6), toon(c));
      bag.scale.set(1, 0.82, 1);
      bag.position.set(bxp, 0.22, 7.55);
      g.add(bag);
    }
    put(gbox(0.06, 1.1, 0.06), M.metal, 12.75, 0.55, 6.7);
    put(gbox(0.5, 0.36, 0.03), M.woodDeep, 12.75, 1.0, 6.72);
    panel(0.36, 0.24, toon('#c3c8ce'), 12.75, 1.02, 6.745, 1);   // 告示纸
    puddle(11.6, 7.75, 0.5);
    // 垃圾分类区碰撞：背面矮墙 + 三分类桶（含盖）+ 资源回收筐 + 立牌，各收一个盒
    boxes.push(boxSpec('trash-backwall', 10.3, 12.9, 0, 1.05, 6.42, 6.58));
    boxes.push(boxSpec('trash-bins', 10.49, 12.43, 0, 0.95, 6.70, 7.27));
    boxes.push(boxSpec('trash-basket', 12.24, 12.76, 0, 0.46, 7.29, 7.71));
    boxes.push(boxSpec('trash-sign', 12.72, 13.0, 0, 1.18, 6.67, 6.74));
    // 东 / 西两侧 mesh 栅栏：竖杆 + 顶轨，南北向两道，之前漏了碰撞盒
    boxes.push(boxSpec('trash-fence-w', 10.3 - 0.04, 10.3 + 0.04, 0, 2.1, 6.5, 8.05));
    boxes.push(boxSpec('trash-fence-e', 12.9 - 0.04, 12.9 + 0.04, 0, 2.1, 6.5, 8.05));
  }

  /* ---------------- 绿化 ----------------
   * 全是低饱和的深绿，且刻意不亮：夜里的植物是被路灯和窗光打亮的暗绿块，
   * 鲜绿会把整个夜景的色温拽跑。
   */
  for (const hx of [-7.8, -4.0, 4.0, 7.8]) {
    g.add(buildShrub(hx, ZB + 0.55, 1.0, 0.62, 0.8, 900 + hx, M));
  }
  // 沿 1 层墙面的花壇：一条窄绿化带 + 三丛矮灌木
  flatUp(9.0, 0.9, toon('#414f3c'), -13.0, 0.006, ZF + 0.55);
  flatUp(9.0, 0.9, toon('#414f3c'), 13.0, 0.006, ZF + 0.55);
  for (const hx of [-15.4, -12.6, -10.2]) g.add(buildShrub(hx, ZF + 0.5, 0.7, 0.5, 0.6, 1300 + hx, M));
  for (const hx of [11.0, 13.2, 15.0]) g.add(buildShrub(hx, ZF + 0.5, 0.7, 0.5, 0.6, 1700 + hx, M));
  // 两棵小乔木：一棵在自行车棚东，一棵在垃圾区西
  g.add(buildTree(-8.6, 7.0, 2.9, 4111, M));
  g.add(buildTree(9.0, 7.3, 2.4, 4222, M));
  g.add(buildTree(-16.6, 7.2, 2.2, 4333, M));
  // 楼东侧（山墙与楼梯之间的窄带）：两丛灌木，别让地基直接裸着
  g.add(buildShrub(31.75, 1.4, 0.8, 0.55, 0.7, 4501, M));
  g.add(buildShrub(31.7, 3.9, 0.7, 0.5, 0.6, 4502, M));
  // 草坪条
  flatUp(3.0, 1.1, toon('#3f5039'), -6.0, 0.006, ZB + 0.5);
  flatUp(3.0, 1.1, toon('#3f5039'), 6.0, 0.006, ZB + 0.5);

  /* ---------------- 电线杆 + 跨街电线 ----------------
   * 日式街区最便宜也最有效的标志物。电线用二次贝塞尔下垂，
   * TubeGeometry 半径 12mm，远看是一道细剪影。
   */
  const poleMat = toon('#2c3446');
  put(new THREE.CylinderGeometry(0.09, 0.12, 7.2, 8), poleMat, -8.2, 3.6, 7.1);
  put(gbox(1.6, 0.09, 0.09), poleMat, -8.2, 6.6, 7.1); // 横担
  put(gbox(1.1, 0.08, 0.08), poleMat, -8.2, 6.1, 7.1); // 第二层横担
  const wireMat = toon('#151a26');
  const wire = (a: [number, number, number], b: [number, number, number], sag: number) => {
    const mid = new THREE.Vector3((a[0] + b[0]) / 2, Math.min(a[1], b[1]) - sag, (a[2] + b[2]) / 2);
    const curve = new THREE.QuadraticBezierCurve3(new THREE.Vector3(...a), mid, new THREE.Vector3(...b));
    g.add(new THREE.Mesh(new THREE.TubeGeometry(curve, 14, 0.012, 4), wireMat));
  };
  // 电线杆 → 公寓山墙两根（入户线）；沿街向西一根，走出画外交代街道纵深。
  wire([-8.9, 6.6, 7.1], [X0 + 0.1, 6.9, 2.0], 0.5);
  wire([-7.5, 6.6, 7.1], [X0 + 0.1, 6.5, -2.5], 0.6);
  wire([-8.2, 6.9, 7.1], [-21.0, 6.2, 7.4], 0.9);
  wire([-8.2, 6.1, 7.1], [-21.0, 5.6, 7.6], 1.0);

  // 窗玻璃收尾合并：所有 pane 到这里才落进 group（见上方 paneBuf 的说明）
  flushPanes();

  return { group: g, boxes };
}

/* ============================================================================
 * Stage 2b — 街角便利店
 * ========================================================================== */

/**
 * 街道剖面（沿 +Z，从公寓南墙往南）：
 *   6.3 .. 7.6   公寓侧人行道（三盏路灯 z7.4 在这条上）
 *   7.6 .. 12.6  车行道：斑马线、停车位
 *   12.6 .. 14.0 便利店侧人行道（贩卖机 / 自行车 / 垃圾桶 / 护栏）
 *   14.0 .. 21.0 便利店。14.0..16.4 是玻璃前厅，16.4 之后是实体体量
 *   14.0 .. 21.0 居酒屋在便利店正东侧（x 7.5..11.7），门面与便利店同一条街沿线
 *   13.5 .. 27.0 居酒屋东侧 11.7..14.3 是小巷，巷东一栋楼封住纵深
 *
 * 机位：阳台栏杆 z6.3 / 高 4.4（俯视街角），客厅 z0 / 高 4.4（平视街对面）。
 * 店门 z14 距阳台 7.7m —— 这是"站在二楼看街角"该有的距离，再远就失去
 * 街角的包裹感，再近就从客厅看糊脸。
 */
const ST = {
  x0: -4.4, x1: 6.6,      // 沿街宽度 11.0
  zFront: 14.0, zBack: 21.0,
  zSolid: 16.4,           // 前厅 / 实体分界：店内浅景深就截在这里
  eave: 3.5,              // 檐口
  parapet: 3.85,
  ceil: 2.9,              // 前厅吊顶
};
const ST_CX = (ST.x0 + ST.x1) / 2; // 1.1
const ST_W = ST.x1 - ST.x0;        // 11.0

/**
 * 居酒屋：搬到便利店正东侧，与便利店并排在同一条街沿线上（门面都朝北对着马路）。
 *   x 7.5 .. 11.7   西墙离便利店东山墙(ST.x1=6.6)留 0.9m 检修缝，店宽 W=4.2
 *   z 14.0 .. 17.8  北墙与便利店店门线 zFront=14.0 齐平，店深 D=3.8
 * 原「便利店东侧 6.6..9.2」的小巷整条东移到 11.7..14.3（宽 2.6 不变），
 * 巷东楼 alley-block 与巷口设施（壁灯 / 护栏 / 路牌 / 红绿灯 / 横跨电线）
 * 同步东移 5.1。居酒屋东侧的入口 recess 正好开向新小巷。
 */
const IZ = { x0: 7.5, x1: 11.7 };
const ALLEY = { x0: 11.7, x1: 14.3 };

/**
 * 货架商品带：一条 256×32 的低饱和包装条带，一条顶几十件商品。
 *
 * 隔着 8m 街道 + 雨幕 + 玻璃，逐件建模和一条纹理肉眼无差，顶点数差两个
 * 量级。色板压在低饱和区——高饱和糖果色会把整个夜景的色温拽跑。
 */
const goodsCache = new Map<number, THREE.Texture>();
function goodsStripTexture(seed: number): THREE.Texture {
  const hit = goodsCache.get(seed);
  if (hit) return hit;
  const { canvas, ctx } = makeCanvas(256, 32);
  const rnd = makeRng(seed);
  ctx.fillStyle = '#e7e3da';
  ctx.fillRect(0, 0, 256, 32);
  const palette = ['#a86a58', '#5f86ab', '#779a68', '#b59a63', '#84798f', '#4f9188', '#a5838f', '#6d7c90', '#b8a376'];
  let x = 4;
  while (x < 252) {
    const w = 8 + Math.floor(rnd() * 8);
    const h = 17 + Math.floor(rnd() * 12);
    ctx.fillStyle = palette[Math.floor(rnd() * palette.length)];
    ctx.fillRect(x, 29 - h, w, h);
    x += w + 3;
  }
  const tex = toTexture(canvas);
  goodsCache.set(seed, tex);
  return tex;
}

/**
 * 招牌灯箱贴图：深青绿底 + 白字。
 *
 * 日式便利店 VI 的经典结构是一条贯穿正面的横向灯箱。这里不做真实品牌，
 * 用深青绿 / 白 / 浅蓝三色带 + 罗马字——青绿是整条街唯一的冷色发光体，
 * 与建筑墙体的冷灰同族但明度高一档，既跳得出来又不刺眼。
 */
let _sign: THREE.Texture | null = null;
function signTexture(): THREE.Texture {
  if (_sign) return _sign;
  const { canvas, ctx } = makeCanvas(768, 64);
  ctx.fillStyle = '#2f7f6b';
  ctx.fillRect(0, 0, 768, 64);
  ctx.fillStyle = '#eaf3ef';
  ctx.fillRect(0, 0, 768, 5);
  ctx.fillRect(0, 59, 768, 5);
  ctx.font = 'bold 40px system-ui, "Segoe UI", sans-serif';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = '#eef5f2';
  ctx.fillText('CITY MART', 26, 32);
  ctx.font = '18px system-ui, "Segoe UI", sans-serif';
  ctx.fillStyle = '#a9d3c4';
  ctx.fillText('24h', 660, 32);
  _sign = toTexture(canvas);
  return _sign;
}

/** 平滑起停：开门/关门都用它，避免线性运动的机械感。 */
function smoothstep(x: number): number {
  const t = Math.min(1, Math.max(0, x));
  return t * t * (3 - 2 * t);
}

/**
 * 公寓楼体四壁的碰撞盒（外壳 mesh 之外的那部分实心体块）。
 *
 * 与室内家具同源同机制：都是「数据声明若干个轴对齐长方体 → 交给 collider.ts 的
 * buildBoxColliders 生成世界 AABB」，外壳直接写在世界坐标（跨 1F~4F）。
 * 楼体内部的栏杆 / 腰壁 / 自行车由 buildApartmentShell 的 boxes 一并交出，
 * 与这里拼成同一张表，不再有任何按标记遍历场景的专用收集器。
 * 不盲从 mesh 几何——否则门洞会被重新堵死、地坪高度还会生成水平盒把人钉死。
 *
 * 几何边界（与外壳 mesh 同源）：主体量南墙 z=APT_ZF(4.7)、北墙 z=APT_ZN(-5.9)、
 * 东西山墙 x=APT_X0/APT_X1。2F 起南侧阳台挑出到 APT_ZB(6.3)，阳台外缘屏障建在那条线上。
 * 外廊 / 外楼梯在楼体之外（z≈-8.4、x>31.15），不被这四壁卡住。
 */

const SHELL_RF = 3.4 + 2.8 * 3 + 0.7;  // 首层 3.4 + 3×2.8 层高 + 女儿墙余量
const SHELL_T = 0.12;                    // 墙厚余量：碰撞盒略厚，避免贴脸穿模

export const APT_WALL_BOXES: BoxColliderSpec[] = [
  /* —— 南立面 —— */
  // 1F：主体量南墙在 z=4.7。1F 没有阳台（z=6.3 那条线在 1F 是入口前的人行道），
  // 所以这道墙建在真实墙面 APT_ZF，而不是阳台外缘。入口自动玻璃双开门
  // x∈[-1.0, 1.0]（门扇 1.8m 宽、门框立柱在 ±0.95）处留洞，否则进不了楼。
  boxSpec('apt-south-1f-w', APT_X0 - SHELL_T, -1.0, 0, FLOORS[0], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-south-1f-e', 1.0, APT_X1 + SHELL_T, 0, FLOORS[0], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
  // 2F~4F：阳台外缘屏障在 z=6.3（栏杆线），挡住从阳台走出掉楼。
  // 只从 2F 楼面 FLOORS[0] 起——1F 无阳台，这道屏障不该压到 1F。
  boxSpec('apt-south-up', APT_X0 - SHELL_T, APT_X1 + SHELL_T, FLOORS[0], SHELL_RF, APT_ZB - SHELL_T, APT_ZB + SHELL_T),

  /* —— 邻居户室内空腔的南界（z=APT_ZF）——
   * 各住宅层的实心体量南端退了 APT_ROOM_D，窗后是有家具的真屋子，
   * 但那些屋子不可进入——这道墙就是"看得见、进不去"的那一层玻璃。
   * 没有它的话，玩家能从邻居家的阳台一路走进别人客厅里（楼体本身不是
   * 碰撞体，碰撞全靠这里声明）。
   * 2F 的 203 段（x∈[-6.2, 6.2]）跳过：203 的南墙由房间自己的 shell 提供，
   * 那里是一道能推开的玻璃滑门，玩家要能进出阳台。 */
  boxSpec('apt-room-south-2f-w', APT_X0 - SHELL_T, -6.2 + SHELL_T, FLOORS[0], FLOORS[1], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-room-south-2f-e', 6.2 - SHELL_T, APT_X1 + SHELL_T, FLOORS[0], FLOORS[1], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-room-south-up', APT_X0 - SHELL_T, APT_X1 + SHELL_T, FLOORS[1], SHELL_RF, APT_ZF - SHELL_T, APT_ZF + SHELL_T),

  /* —— 1F 入口门厅：能从自动门进去，但只是个 0.88m 深的浅门厅 ——
   * 后壁（空腔北界）挡住往里走，两端挡住往邻居家横向走。
   * 电梯 / 楼梯起步 / 消火栓都是低模陈设，一件都不该能被穿过。 */
  boxSpec('apt-entry-back', -6.2 + SHELL_T, 6.2 - SHELL_T, 0, FLOORS[0],
    APT_ZF - APT_ROOM_D - SHELL_T, APT_ZF - APT_ROOM_D + SHELL_T),
  boxSpec('apt-entry-w', -6.2 - SHELL_T, -6.2 + SHELL_T, 0, FLOORS[0],
    APT_ZF - APT_ROOM_D - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-entry-e', 6.2 - SHELL_T, 6.2 + SHELL_T, 0, FLOORS[0],
    APT_ZF - APT_ROOM_D - SHELL_T, APT_ZF + SHELL_T),
  // 门厅里玩家真能走到跟前的三件，得挡得住；其余（地垫 / 通告牌 / 扶手立柱）
  // 一律不进碰撞——它们要么贴地要么细过 5cm，撞上去的观感损失远小于多加的盒。
  boxSpec('apt-entry-elevator', -3.95, -2.05, 0, 2.5,
    APT_ZF - APT_ROOM_D, APT_ZF - APT_ROOM_D + 0.14),
  boxSpec('apt-entry-stair', 2.65, 4.15, 0, 0.85,
    APT_ZF - APT_ROOM_D + 0.05, APT_ZF - APT_ROOM_D + 0.85),
  boxSpec('apt-entry-hydrant', -5.65, -5.15, 0, 0.68,
    APT_ZF - APT_ROOM_D + 0.04, APT_ZF - APT_ROOM_D + 0.24),

  /* —— 北立面 z=-5.9（外廊侧楼背）：挡住从外廊朝楼内走、掉进单元空腔 —— */
  // 203 主门 x∈[4.6, 5.5]（shell.walls.north 的 door 开口）处留洞，整面实心会把主门堵死。
  // 洞只在 2F 那一层：以前按整楼高留洞，是因为 3F/4F 是实心块、钻进去也只是
  // 卡在混凝土里；现在各层南侧都挖了室内空腔，整楼高的洞等于给 303/403 开了
  // 一条从外廊钻进邻居客厅的通道。1F 同样实心——背面只有设备间小门。
  boxSpec('apt-north-1f', APT_X0 - SHELL_T, APT_X1 + SHELL_T, 0, FLOORS[0], APT_ZN - SHELL_T, APT_ZN + SHELL_T),
  boxSpec('apt-north-2f-w', APT_X0 - SHELL_T, 4.6 + SHELL_T, FLOORS[0], FLOORS[1], APT_ZN - SHELL_T, APT_ZN + SHELL_T),
  boxSpec('apt-north-2f-e', 5.5 - SHELL_T, APT_X1 + SHELL_T, FLOORS[0], FLOORS[1], APT_ZN - SHELL_T, APT_ZN + SHELL_T),
  boxSpec('apt-north-up', APT_X0 - SHELL_T, APT_X1 + SHELL_T, FLOORS[1], SHELL_RF, APT_ZN - SHELL_T, APT_ZN + SHELL_T),

  /* —— 东西山墙：1F 基座段只到 z=APT_ZF（与 apt-ground 体量一致，1F 无阳台，
   *   南端就是 z=4.7，再往南是入口前人行道/空地，不该有墙）；2F~4F 段含阳台进深，
   *   延伸到 APT_ZB（阳台东/西端收头，挡住从阳台东端走出掉楼）。
   *   旧版整段压到 6.3，导致 1F 东南/西南角多出一截空气墙。 —— */
  boxSpec('apt-west-1f', APT_X0 - SHELL_T, APT_X0 + SHELL_T, 0, FLOORS[0], APT_ZN - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-west-up',  APT_X0 - SHELL_T, APT_X0 + SHELL_T, FLOORS[0], SHELL_RF, APT_ZN - SHELL_T, APT_ZB + SHELL_T),
  boxSpec('apt-east-1f',  APT_X1 - SHELL_T, APT_X1 + SHELL_T, 0, FLOORS[0], APT_ZN - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-east-up',  APT_X1 - SHELL_T, APT_X1 + SHELL_T, FLOORS[0], SHELL_RF, APT_ZN - SHELL_T, APT_ZB + SHELL_T),
];

export type StoreHandle = {
  /** 静态部分：调用方走标准装配（合批 → 描边 → add → 冻结） */
  group: THREE.Group;
  /** 动效部分：已各自描边，add 即可。绝不能冻结——门要滑、灯要闪 */
  dynamic: THREE.Group;
  /** 每帧更新，t 是从开局累计的秒数（与浮尘同一时基） */
  update: (t: number) => void;
};

/* ============================================================================
 * Stage 2b — 便利店附属贴图
 *
 * 招牌 / 灯箱 / 商品带之外，街头陈设需要的几张小贴图集中放在这里，避免
 * buildConvenienceStore 里散落一堆闭包构造 canvas。
 * ========================================================================== */

/**
 * 竖式招牌（突き出し看板）。
 *
 * 日式便利店最强的街道识别符号之一：侧墙上伸出来的薄长方形灯箱，
 * 文字沿长边竖排。从街上看是「-Z 立面横招牌 + ±X 侧招牌」组合，
 * 顺着街道走两个方向都能读到店名，比单一条横招牌辨识度高出一档。
 */
let _vSign: THREE.Texture | null = null;
function verticalSignTexture(): THREE.Texture {
  if (_vSign) return _vSign;
  const W = 96, H = 384;
  const { canvas, ctx } = makeCanvas(W, H);
  // 背景：与横招牌同色，但底色压一档亮度，远处不会被横招牌"吃"掉
  ctx.fillStyle = '#2a6f5d';
  ctx.fillRect(0, 0, W, H);
  // 顶/底白色窄边，与横招牌呼应
  ctx.fillStyle = '#dde8e3';
  ctx.fillRect(0, 0, W, 4);
  ctx.fillRect(0, H - 4, W, 4);
  // 文字竖排：日文与罗马字双行，靠字重区分主次
  ctx.fillStyle = '#eaf3ef';
  ctx.font = 'bold 38px system-ui, "Segoe UI", sans-serif';
  ctx.textBaseline = 'top';
  // 每个字符单独写一行，确保竖排不溢出
  const chars = ['C', 'I', 'T', 'Y', 'M', 'A', 'R', 'T'];
  let y = 18;
  for (const ch of chars) {
    ctx.fillText(ch, 22, y);
    y += 42;
  }
  // 24h 在底部独立成行
  ctx.font = 'bold 22px system-ui, "Segoe UI", sans-serif';
  ctx.fillStyle = '#ffd6b0';
  ctx.fillText('24h', 26, y + 4);
  _vSign = toTexture(canvas);
  return _vSign;
}

/**
 * 立式看板（立て看板，A 字招牌）。
 *
 * 雨夜便利店门口最常见的木制 A 字招牌：
 * - 底框深木色，面板暖白
 * - 一行日文「おすすめ」 + 三件商品简笔
 * - 下沿一溜雨水渍（刚被雨打湿的边缘读得最准）
 */
let _sandwichBoard: THREE.Texture | null = null;
function sandwichBoardTexture(): THREE.Texture {
  if (_sandwichBoard) return _sandwichBoard;
  const W = 256, H = 320;
  const { canvas, ctx } = makeCanvas(W, H);
  // 底：暖白面板
  ctx.fillStyle = '#ece4d2';
  ctx.fillRect(0, 0, W, H);
  // 木框：四边深木色，留 12px
  ctx.fillStyle = '#5a4a36';
  ctx.fillRect(0, 0, W, 14);
  ctx.fillRect(0, H - 14, W, 14);
  ctx.fillRect(0, 0, 14, H);
  ctx.fillRect(W - 14, 0, 14, H);
  // 标题：おすすめ（推荐）
  ctx.fillStyle = '#3a2a18';
  ctx.font = 'bold 56px "Yu Mincho", "Hiragino Mincho ProN", serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'top';
  ctx.fillText('おすすめ', W / 2, 32);
  // 副标题：手写感的细字
  ctx.font = '20px "Yu Mincho", "Hiragino Mincho ProN", serif';
  ctx.fillStyle = '#6a4a30';
  ctx.fillText('本日のおすすめ商品', W / 2, 108);
  // 三件商品简笔：便当 / 饭团 / 咖啡杯，每个用色块 + 文字
  const items = [
    { y: 152, color: '#c4835a', name: '弁当', sub: '390円' },
    { y: 206, color: '#6b8f5e', name: 'おにぎり', sub: '130円' },
    { y: 260, color: '#7a5a3c', name: 'コーヒー', sub: '120円' },
  ];
  for (const it of items) {
    ctx.fillStyle = it.color;
    ctx.fillRect(28, it.y, 28, 28);
    ctx.fillStyle = '#3a2a18';
    ctx.font = 'bold 28px "Yu Mincho", "Hiragino Mincho ProN", serif';
    ctx.textAlign = 'left';
    ctx.fillText(it.name, 72, it.y - 2);
    ctx.font = '18px system-ui';
    ctx.fillStyle = '#6a4a30';
    ctx.fillText(it.sub, 72, it.y + 28);
  }
  // 下沿水渍：从下往上 14px 范围内做一道由深到浅的水痕
  const wet = ctx.createLinearGradient(0, H - 14, 0, H - 40);
  wet.addColorStop(0, 'rgba(80,100,130,0.42)');
  wet.addColorStop(1, 'rgba(80,100,130,0)');
  ctx.fillStyle = wet;
  ctx.fillRect(14, H - 40, W - 28, 26);
  _sandwichBoard = toTexture(canvas);
  return _sandwichBoard;
}

/**
 * 掲示板（社区公告栏）。
 *
 * 玻璃门 + 内贴几张通知：上半区是社区活动海报，下半区是几张小纸条
 * （错落贴、有重叠）。雨夜里玻璃上会反光——但贴图层面已经把玻璃高光画
 * 进去了，不需要额外的法线贴图。
 */
let _bulletinBoard: THREE.Texture | null = null;
function bulletinBoardTexture(): THREE.Texture {
  if (_bulletinBoard) return _bulletinBoard;
  const W = 192, H = 256;
  const { canvas, ctx } = makeCanvas(W, H);
  // 底：浅米色内壁
  ctx.fillStyle = '#c8c0aa';
  ctx.fillRect(0, 0, W, H);
  // 主海报：上方一张「地域のお知らせ」
  ctx.fillStyle = '#f3e8d0';
  ctx.fillRect(14, 14, W - 28, 110);
  ctx.fillStyle = '#9a6f3a';
  ctx.fillRect(14, 14, W - 28, 6);
  ctx.font = 'bold 22px "Yu Mincho", "Hiragino Mincho ProN", serif';
  ctx.fillStyle = '#3a2818';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'top';
  ctx.fillText('地域のお知らせ', W / 2, 30);
  ctx.font = '14px system-ui';
  ctx.fillStyle = '#5a4a36';
  // 三行伪正文
  for (let i = 0; i < 3; i++) {
    const y = 68 + i * 14;
    ctx.fillRect(28, y, W - 56, 2);
  }
  // 小纸条：错落贴在主海报右下和下半区
  ctx.save();
  ctx.translate(124, 110);
  ctx.rotate(0.08);
  ctx.fillStyle = '#fff4d8';
  ctx.fillRect(0, 0, 60, 44);
  ctx.fillStyle = '#6a4a30';
  ctx.font = 'bold 14px "Yu Mincho", serif';
  ctx.textAlign = 'left';
  ctx.fillText(' lost cat ', 6, 8);
  ctx.fillText('  ', 6, 24);
  ctx.font = '10px system-ui';
  ctx.fillText('近所を捜しています', 6, 36);
  ctx.restore();
  // 第二张小纸条（横贴）
  ctx.save();
  ctx.translate(20, 150);
  ctx.rotate(-0.05);
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, 80, 38);
  ctx.fillStyle = '#444';
  ctx.font = 'bold 13px system-ui';
  ctx.fillText('マンション総会', 6, 6);
  ctx.font = '11px system-ui';
  ctx.fillText('11/15 (土) 19:00', 6, 22);
  ctx.restore();
  // 玻璃高光：左上斜向一道亮条
  const gl = ctx.createLinearGradient(0, 0, W * 0.7, H * 0.6);
  gl.addColorStop(0, 'rgba(255,255,255,0.18)');
  gl.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = gl;
  ctx.fillRect(0, 0, W, H);
  // 雨滴：稀疏几个亮点
  for (let i = 0; i < 6; i++) {
    const x = 20 + ((i * 37) % (W - 40));
    const y = 20 + ((i * 53) % (H - 40));
    ctx.fillStyle = 'rgba(220,235,255,0.55)';
    ctx.beginPath();
    ctx.arc(x, y, 1.6, 0, Math.PI * 2);
    ctx.fill();
  }
  _bulletinBoard = toTexture(canvas);
  return _bulletinBoard;
}

/**
 * 吊り下げ POP（悬挂促销旗）。
 *
 * 三角小旗，暖红底 + 白字「セール」。三角下摆剪一下，再做一道阴影折痕
 * 让它读得出"飘着"。
 */
let _popBanner: THREE.Texture | null = null;
function popBannerTexture(): THREE.Texture {
  if (_popBanner) return _popBanner;
  const W = 128, H = 96;
  const { canvas, ctx } = makeCanvas(W, H);
  // 顶边整条
  ctx.fillStyle = '#c64a3a';
  ctx.fillRect(0, 0, W, H);
  // 三角下摆：底部 12px 切掉两个三角（左右各一个凹）
  ctx.fillStyle = '#c64a3a';
  ctx.beginPath();
  ctx.moveTo(0, H);
  ctx.lineTo(W / 2 - 12, H - 12);
  ctx.lineTo(W / 2 + 12, H - 12);
  ctx.lineTo(W, H);
  ctx.closePath();
  ctx.fill();
  // 中线折痕
  ctx.fillStyle = 'rgba(0,0,0,0.16)';
  ctx.fillRect(W / 2 - 1, 8, 2, H - 16);
  // 文字
  ctx.fillStyle = '#fff4d8';
  ctx.font = 'bold 28px "Yu Gothic", "Hiragino Kaku Gothic ProN", sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('SALE', W / 2 - 28, H / 2 - 2);
  ctx.fillText('セール', W / 2 + 28, H / 2 - 2);
  _popBanner = toTexture(canvas);
  return _popBanner;
}

/**
 * 店内灯箱海报（宽幅，比 posterMat 多一张插画）：暖红「ポイント 5倍」，
 * 让玻璃后面除了货架还多一处暖色焦点。
 */
let _posterB: THREE.Texture | null = null;
function posterBTexture(): THREE.Texture {
  if (_posterB) return _posterB;
  const W = 256, H = 384;
  const { canvas, ctx } = makeCanvas(W, H);
  // 底
  ctx.fillStyle = '#f1e4c6';
  ctx.fillRect(0, 0, W, H);
  // 顶带
  ctx.fillStyle = '#c64a3a';
  ctx.fillRect(0, 0, W, 56);
  ctx.fillStyle = '#fff4d8';
  ctx.font = 'bold 30px "Yu Gothic", "Hiragino Kaku Gothic ProN", sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('ポイント 5倍', W / 2, 28);
  // 中段插画占位：暖色块 + 文字
  ctx.fillStyle = '#e7b46a';
  ctx.fillRect(20, 80, W - 40, 130);
  ctx.fillStyle = '#7a3a1c';
  ctx.font = 'bold 22px "Yu Gothic", "Hiragino Kaku Gothic ProN", sans-serif';
  ctx.textBaseline = 'top';
  ctx.fillText('毎週 金・土・日', 30, 92);
  ctx.font = '14px system-ui';
  ctx.fillText('※ 一部対象外', 30, 122);
  // 下段小字
  ctx.fillStyle = '#3a2818';
  ctx.font = '16px "Yu Mincho", "Hiragino Mincho ProN", serif';
  for (let i = 0; i < 6; i++) {
    ctx.fillRect(20, 240 + i * 18, W - 40, 2);
  }
  _posterB = toTexture(canvas);
  return _posterB;
}

/**
 * 营业中灯牌：店门右侧一张竖长小灯牌，「営業中」字样 + 红绿底色块。
 * 暖底 + 强对比，远看就是店门口那个亮起来的"欢迎光临"焦点。
 */
let _openSign: THREE.Texture | null = null;
function openSignTexture(): THREE.Texture {
  if (_openSign) return _openSign;
  const W = 96, H = 192;
  const { canvas, ctx } = makeCanvas(W, H);
  ctx.fillStyle = '#1e6b48';
  ctx.fillRect(0, 0, W, H);
  // 上下窄边
  ctx.fillStyle = '#eaf3ef';
  ctx.fillRect(0, 0, W, 4);
  ctx.fillRect(0, H - 4, W, 4);
  // 文字
  ctx.fillStyle = '#fff4d8';
  ctx.font = 'bold 32px "Yu Gothic", "Hiragino Kaku Gothic ProN", sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('営', W / 2, 50);
  ctx.fillText('業', W / 2, 86);
  ctx.fillText('中', W / 2, 122);
  // 下沿 OPEN
  ctx.font = 'bold 22px system-ui';
  ctx.fillText('OPEN', W / 2, 162);
  _openSign = toTexture(canvas);
  return _openSign;
}

/** おにぎり冷藏陈列（玻璃门 + 三排饭团三角剪影）。 */
let _onigiriCooler: THREE.Texture | null = null;
function onigiriCoolerTexture(): THREE.Texture {
  if (_onigiriCooler) return _onigiriCooler;
  const W = 256, H = 192;
  const { canvas, ctx } = makeCanvas(W, H);
  // 底：冷白内胆
  ctx.fillStyle = '#dfeaf2';
  ctx.fillRect(0, 0, W, H);
  // 三排：每排 14 个饭团
  const wrappings = ['#c64a3a', '#3a6e9a', '#e7b46a', '#5e8a4a', '#7a4a8c', '#dde1e4'];
  for (let row = 0; row < 3; row++) {
    const y = 24 + row * 56;
    for (let i = 0; i < 14; i++) {
      const x = 14 + i * 17;
      const c = wrappings[(i + row * 3) % wrappings.length];
      // 三角形饭团
      ctx.fillStyle = c;
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(x + 14, y);
      ctx.lineTo(x + 7, y + 26);
      ctx.closePath();
      ctx.fill();
      // 海苔条
      ctx.fillStyle = '#1a1410';
      ctx.fillRect(x + 5, y + 8, 4, 14);
    }
  }
  // 玻璃反光：斜向一道
  const gl = ctx.createLinearGradient(0, 0, W * 0.5, H);
  gl.addColorStop(0, 'rgba(255,255,255,0.22)');
  gl.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = gl;
  ctx.fillRect(0, 0, W, H);
  _onigiriCooler = toTexture(canvas);
  return _onigiriCooler;
}

/** 弁当陈列保温柜（玻璃门 + 多层便当盒轮廓）。 */
let _bentoWarmer: THREE.Texture | null = null;
function bentoWarmerTexture(): THREE.Texture {
  if (_bentoWarmer) return _bentoWarmer;
  const W = 256, H = 256;
  const { canvas, ctx } = makeCanvas(W, H);
  // 底：暖黄内胆
  ctx.fillStyle = '#f1d99a';
  ctx.fillRect(0, 0, W, H);
  // 三层架：每层排 8 个便当
  const bentos = ['#c4835a', '#7a5a3c', '#e7b46a', '#a87a4a', '#5a4a36', '#c0986a'];
  for (let row = 0; row < 3; row++) {
    const y = 18 + row * 78;
    for (let i = 0; i < 8; i++) {
      const x = 14 + i * 30;
      const c = bentos[(i + row * 2) % bentos.length];
      // 便当盒
      ctx.fillStyle = '#3a2818';
      ctx.fillRect(x, y, 24, 36);
      ctx.fillStyle = c;
      ctx.fillRect(x + 2, y + 2, 20, 32);
      // 饭团半圆
      ctx.fillStyle = '#fff4d8';
      ctx.beginPath();
      ctx.arc(x + 12, y + 26, 8, Math.PI, 0);
      ctx.fill();
    }
  }
  // 暖光高光
  const gl = ctx.createLinearGradient(0, 0, W * 0.5, H);
  gl.addColorStop(0, 'rgba(255,240,210,0.32)');
  gl.addColorStop(1, 'rgba(255,240,210,0)');
  ctx.fillStyle = gl;
  ctx.fillRect(0, 0, W, H);
  _bentoWarmer = toTexture(canvas);
  return _bentoWarmer;
}

/** アイス冷凍柜（玻璃上盖 + 冰淇淋色块阵列）。 */
let _freezer: THREE.Texture | null = null;
function freezerTexture(): THREE.Texture {
  if (_freezer) return _freezer;
  const W = 256, H = 128;
  const { canvas, ctx } = makeCanvas(W, H);
  ctx.fillStyle = '#dde8f2';
  ctx.fillRect(0, 0, W, H);
  // 上沿"ICE"标
  ctx.fillStyle = '#4a6f8f';
  ctx.fillRect(0, 0, W, 24);
  ctx.fillStyle = '#eaf3ef';
  ctx.font = 'bold 18px system-ui';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('ICE CREAM', W / 2, 12);
  // 两排冰淇淋盒
  const ices = ['#e7b46a', '#c64a3a', '#5e8a4a', '#dde1e4', '#e2a4c4', '#a87a4a', '#3a6e9a', '#c0986a'];
  for (let row = 0; row < 2; row++) {
    const y = 36 + row * 44;
    for (let i = 0; i < 16; i++) {
      const x = 8 + i * 15;
      ctx.fillStyle = ices[(i + row * 3) % ices.length];
      ctx.fillRect(x, y, 12, 36);
      ctx.fillStyle = 'rgba(0,0,0,0.18)';
      ctx.fillRect(x, y + 28, 12, 8);
    }
  }
  // 上沿玻璃高光
  const gl = ctx.createLinearGradient(0, 24, 0, 50);
  gl.addColorStop(0, 'rgba(255,255,255,0.32)');
  gl.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = gl;
  ctx.fillRect(0, 24, W, 26);
  _freezer = toTexture(canvas);
  return _freezer;
}

/**
 * 径向渐变贴图：暖色光晕用。中心不透明暖色 → 边缘全透明，贴到平面上就是
 * 一团柔光（招牌辉光 / 地面暖光池）。W 控制分辨率。
 *
 * 原本是便利店里的局部函数，居酒屋也要用同一套"暖光外溢"，提到模块级共用。
 */
const radial = (inner: string, outer: string, W = 128): THREE.Texture => {
  const { canvas, ctx } = makeCanvas(W, W);
  const grad = ctx.createRadialGradient(W / 2, W / 2, 0, W / 2, W / 2, W / 2);
  grad.addColorStop(0, inner);
  grad.addColorStop(1, outer);
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, W, W);
  return toTexture(canvas);
};

/**
 * 街角便利店：近景主体，203 阳台和客厅望出去的主景观。
 *
 * 店内是**浅景深橱窗盒**——只建 z14.0..16.4 这 2.4m，后面用冷饮柜和后墙
 * 截断。玩家永远走不进去（隔着街道和雨），多花的工时只有在观察者模式怼脸
 * 看店时才兑现，不划算。隔着 8m + 雨幕 + 玻璃，和全建模肉眼无差。
 *
 * 亮着的店是这张图的情绪核心：整条街只有它是暖白，其余全是湿冷蓝灰。
 * 所以店体本身必须是冷灰 / 深灰蓝 / 浅灰 / 深木色——如果店也是米白，
 * "冷环境里唯一一处暖光"这条关系就没了。
 */
export function buildConvenienceStore(): StoreHandle {
  const g = new THREE.Group();
  g.name = 'convenience-store';
  const dyn = new THREE.Group();
  dyn.name = 'convenience-dynamic';

  /* ---------------- 材质：冷灰主体 + 深青绿 VI ---------------- */
  const wall = toon('#aab3bc');            // 外墙涂料：冷灰（matte → 描边 weight 2）
  const wallDark = toon('#8d97a2');        // 侧墙/背墙，背光侧压一档
  const wallDeep = toon('#4c5a6b');        // 腰壁 / 柱 / 雨棚端板：深灰蓝
  const trim = toon('#c1c8ce');            // 檐口 / 女儿墙：浅灰
  const viGreen = toon('#2f7f6b');         // VI 色带：深青绿
  const viBlue = toon('#4a6f8f');          // VI 辅色：深蓝
  const woodTrim = toon(EXT.woodDeep);     // 木质装饰（门框侧板 / 长凳）
  const metal = toon('#3b424e', { finish: 'metal' });   // 框/栏/机壳 → weight 1
  const glassMat = toon('#cfe0ee', {       // 橱窗玻璃：finish glass → weight 0
    finish: 'glass', transparent: true, opacity: 0.14, depthWrite: false,
  });
  const glassCold = toon('#dce9f2', {      // 冷饮柜玻璃门
    finish: 'glass', transparent: true, opacity: 0.2, depthWrite: false,
  });
  const floorTile = toon('#b6bbc1', { finish: 'soft' });
  const ceilMat = toon('#dfe3e7');
  const backWall = toon('#dfe4e8');        // 店内后墙：比外墙更亮，读得出"室内"
  const shelf = toon('#dcdfe2');           // 货架：浅灰（不是米白）
  const shelfMetal = toon('#9aa2aa', { finish: 'metal' }); // 货架立柱 / 层板边
  const counter = toon(EXT.wood);          // 收银台木面
  const counterTop = toon('#c2a184');
  const guideLine = emissive('#d8b869');   // 地面导视：黄色虚线
  const innerLight = emissive('#fff4e2');  // 店内灯箱 / 门头（暖白）
  const downLight = emissive('#ffe9c4', { side: THREE.DoubleSide }); // 雨棚下照灯带
  const coldLight = emissive('#dceaf5');   // 冷饮柜内胆（冷白）
  const posterMat = toon('#e6e9ec');

  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const bx = (w: number, h: number, d: number) => gbox(w, h, d);
  /** 贴地平面（标线 / 地垫）：法线朝上 */
  const flat = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    return put(geo, mat, x, y, z);
  };
  /**
   * 朝 -Z 的立面平板（玻璃 / 海报 / 招牌面板）：对着街道，也就是对着公寓。
   * PlaneGeometry 法线是 +Z，绕 Y 转 π 之后指向 -Z。
   */
  const faceS = (w: number, h: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(w, h), mat);
    m.position.set(x, y, z);
    m.rotation.y = Math.PI;
    g.add(m);
    return m;
  };

  /* ================= 建筑本体 ================= */

  // 实体体量：z16.4..21.0，前厅靠它的前表面当后墙
  const storeMass = put(bx(ST_W, ST.eave, ST.zBack - ST.zSolid), wall, ST_CX, ST.eave / 2, (ST.zSolid + ST.zBack) / 2);
  storeMass.name = 'store-mass';
  // 便利店主体体量（z16.4..21 后段实心块）参与 buildSceneColliders 遍历：玩家能到对街，
  // 需被整块挡住（防从后巷/侧边穿进楼体）。店门在 z14，不在此块范围内不会被堵死；
  // 前厅地面/地垫是脚踝级薄板，collidesAt 自动跳过，不影响临街人行道。
  // 女儿墙 + 屋顶设备：屋顶和墙不是一个明度，结构才分得开
  put(bx(ST_W + 0.24, 0.35, 7.2), trim, ST_CX, ST.parapet - 0.175, 17.5);
  put(bx(1.5, 0.7, 1.5), metal, ST_CX - 2.4, ST.parapet + 0.35, 18.6);
  put(bx(0.9, 0.5, 0.7), metal, ST_CX + 2.6, ST.parapet + 0.25, 19.2);
  // 东山墙临巷，加一道深灰蓝分格压边 + 腰壁
  put(bx(0.08, ST.eave, 7.0), wallDark, ST.x1 + 0.02, ST.eave / 2, 17.5);
  put(bx(ST_W, 0.5, 0.06), wallDeep, ST_CX, 0.25, ST.zSolid + 0.02);

  // 前厅：地板 / 吊顶 / 东西侧墙。前厅比实体矮一截（吊顶 2.9 vs 檐口 3.5），
  // 侧面看得出"玻璃盒子嵌在楼里"的层次。
  const FH = ST.zSolid - ST.zFront; // 2.4
  const FZ = (ST.zFront + ST.zSolid) / 2; // 15.2
  put(bx(ST_W, 0.1, FH), floorTile, ST_CX, 0.05, FZ);
  put(bx(ST_W, 0.1, FH), ceilMat, ST_CX, ST.ceil + 0.05, FZ);
  put(bx(0.14, ST.ceil, FH), wallDark, ST.x0 + 0.07, ST.ceil / 2, FZ);
  put(bx(0.14, ST.ceil, FH), wallDark, ST.x1 - 0.07, ST.ceil / 2, FZ);

  // 吊顶灯带：三条暖白长条，水平朝下（用 downLight 的 DoubleSide）。
  // 这是"店里很亮"的全部来源——夜景里只要天花板亮，隔着玻璃也读得出室内。
  for (const z of [14.7, 15.2, 15.7]) flat(8.6, 0.3, downLight, ST_CX, ST.ceil - 0.02, z);

  /* ================= 店内陈设 ================= */

  // 后墙（= 实体体量的前表面）贴一层更亮的室内墙，把浅盒截断得干净
  faceS(ST_W - 0.3, ST.ceil - 0.1, backWall, ST_CX, ST.ceil / 2, ST.zSolid - 0.03);

  // 货架：两排 × 三段，长边平行玻璃。从街上望进去就是一层层商品带，
  // 这是便利店最强的辨识符号。排 1 矮、排 2 高，纵深读出层次。
  // 层板边和立柱走金属色：全货架一个米白色号会糊成一片白块。
  const SEG_W = 2.1, SEG_GAP = 0.3, SEG_X0 = -1.5;
  for (let row = 0; row < 2; row++) {
    const depth = row === 0 ? 1.10 : 1.45;
    const z = row === 0 ? 14.78 : 15.42;
    for (let s = 0; s < 3; s++) {
      const cx = SEG_X0 + SEG_W / 2 + s * (SEG_W + SEG_GAP);
      put(bx(SEG_W, depth, 0.5), shelf, cx, depth / 2, z).name = `shelf-r${row}s${s}`;
      // 端头挡板（货架侧面那块板，从斜角看能读出体块）
      put(bx(0.04, depth + 0.06, 0.54), shelfMetal, cx - SEG_W / 2, depth / 2, z);
      put(bx(0.04, depth + 0.06, 0.54), shelfMetal, cx + SEG_W / 2, depth / 2, z);
      // 商品带：每层一条，层数按架高走
      const layers = Math.floor(depth / 0.42);
      for (let L = 0; L < layers; L++) {
        const y = 0.24 + L * 0.42;
        const tex = goodsStripTexture(row * 17 + s * 5 + L);
        const strip = new THREE.Mesh(
          new THREE.PlaneGeometry(SEG_W - 0.08, 0.3),
          toon('#ffffff', { map: tex })
        );
        strip.position.set(cx, y, z - 0.256);
        strip.rotation.y = Math.PI;
        g.add(strip);
        // 层板前沿的金属边（商品带下沿一道亮线）
        put(bx(SEG_W - 0.04, 0.02, 0.03), shelfMetal, cx, y - 0.16, z - 0.27);
      }
    }
  }

  // 冷饮柜：贴后墙横贯，柜内冷白 + 四扇玻璃门。它的高度（2.05）压过两排
  // 货架，是浅盒的"背景板"——纵深就在这里被截断。
  // 左端收在 x-2.6：再往左就把后场门和咖啡机整扇挡死了。
  const CZ0 = 15.96;
  put(bx(8.25, 2.05, 0.42), toon('#7f93a4'), 1.525, 1.025, CZ0 + 0.21).name = 'cooler';
  for (let i = 0; i < 4; i++) {
    const cx = -1.575 + i * 2.05;
    faceS(1.85, 1.5, coldLight, cx, 1.15, CZ0 + 0.02);
    faceS(1.85, 1.5, glassCold, cx, 1.15, CZ0 - 0.01);
  }
  for (let i = 0; i < 5; i++) put(bx(0.07, 1.62, 0.05), metal, -2.5 + i * 2.0, 1.15, CZ0 - 0.03);

  // 收银台（西侧靠墙）+ 收银机。z 夹在关东煮柜和冷饮柜之间，前后各留缝。
  // 台面与台体同深：台面比台体宽出檐就会往冷饮柜那侧多伸 6cm、扎进柜体。
  put(bx(1.8, 0.92, 0.7), counter, -3.1, 0.46, 15.58).name = 'counter';
  put(bx(1.8, 0.06, 0.7), counterTop, -3.1, 0.95, 15.58).name = 'counter-top';
  put(bx(0.42, 0.3, 0.36), metal, -3.4, 1.28, 15.58);
  // 关东煮柜台：暖光小方块，日式便利店的灵魂摊位
  put(bx(1.5, 0.88, 0.66), counter, -3.1, 0.44, 14.85).name = 'oden';
  put(bx(1.3, 0.05, 0.5), innerLight, -3.1, 0.9, 14.85);
  // 咖啡机：坐在收银台面左侧（y 从 0.95 起，不与台体相交）
  put(bx(0.44, 0.62, 0.46), metal, -3.75, 1.26, 16.1).name = 'coffee';
  put(bx(0.3, 0.06, 0.3), innerLight, -3.75, 1.0, 16.06);

  // 杂志架（东侧靠玻璃）：浅灰架体 + 木色层板
  put(bx(0.62, 1.32, 1.15), shelf, 5.9, 0.66, 15.1);
  for (let i = 0; i < 3; i++) put(bx(0.5, 0.03, 1.1), woodTrim, 5.9, 0.5 + i * 0.4, 15.1);

  // 后场门（后墙上一道深色门 + 门上小窗）。放在冷饮柜左侧的空档里，
  // 冷饮柜横贯整个后墙，门挪到中间就被挡死。
  faceS(0.9, 2.0, toon('#6f7883'), -3.5, 1.0, ST.zSolid - 0.05).name = 'backdoor';
  faceS(0.5, 0.32, innerLight, -3.5, 1.62, ST.zSolid - 0.07);

  // 后墙挂钟：圆形暖白面 + 指针（10:10），隔着玻璃多一分"正在营业"的生活感。
  const clockTex = (() => {
    const { canvas, ctx } = makeCanvas(128, 128);
    ctx.fillStyle = '#f4ecd8';
    ctx.beginPath(); ctx.arc(64, 64, 60, 0, Math.PI * 2); ctx.fill();
    ctx.strokeStyle = '#2a2018'; ctx.lineWidth = 5;
    ctx.beginPath(); ctx.arc(64, 64, 60, 0, Math.PI * 2); ctx.stroke();
    for (let i = 0; i < 12; i++) {
      const a = (i / 12) * Math.PI * 2;
      ctx.fillStyle = '#2a2018';
      ctx.beginPath(); ctx.arc(64 + Math.sin(a) * 48, 64 - Math.cos(a) * 48, 3, 0, Math.PI * 2); ctx.fill();
    }
    ctx.strokeStyle = '#2a2018'; ctx.lineCap = 'round';
    ctx.lineWidth = 5; ctx.beginPath(); ctx.moveTo(64, 64); ctx.lineTo(64 - 24, 64 - 14); ctx.stroke();
    ctx.lineWidth = 3; ctx.beginPath(); ctx.moveTo(64, 64); ctx.lineTo(64 + 32, 64 - 11); ctx.stroke();
    ctx.fillStyle = '#c64a3a'; ctx.beginPath(); ctx.arc(64, 64, 4, 0, Math.PI * 2); ctx.fill();
    return toTexture(canvas);
  })();
  faceS(0.5, 0.5, toon('#ffffff', { map: clockTex }), 3.1, 2.3, ST.zSolid - 0.06).name = 'wall-clock';

  // 关东煮上方吊一盏暖色小吊灯：操作区的发光焦点，配合下方补光把这一角
  // 烘成店里最暖的一处。无动画，进 g 一起合批冻结。
  {
    const cord = new THREE.Mesh(new THREE.CylinderGeometry(0.012, 0.012, 0.34, 5), toon('#2a2018'));
    cord.position.y = 2.78;
    const shade = new THREE.Mesh(new THREE.ConeGeometry(0.22, 0.26, 16, 1, true), toon('#46505c'));
    shade.position.y = 2.58;
    const bulb = new THREE.Mesh(new THREE.CircleGeometry(0.17, 16), emissive('#fff0cf'));
    bulb.rotation.x = Math.PI / 2; // 法线朝 -Y（朝下，从街上看得到）
    bulb.position.y = 2.46;
    const pg = new THREE.Group();
    pg.add(cord, shade, bulb);
    pg.position.set(-3.1, 0, 14.85);
    g.add(pg);
  }

  // 地面导视：黄色虚线，从门口往里。y 要压在店内地砖（0..0.1）之上，
  // 贴到 0.015 会整条埋进地板里。
  for (let i = 0; i < 5; i++) flat(0.12, 0.34, guideLine, -0.6, 0.105, 14.5 + i * 0.5);

  // 玻璃内侧海报两张（贴在玻璃后面，朝外）
  faceS(0.6, 0.85, posterMat, -2.9, 1.75, 14.06);
  faceS(0.6, 0.85, posterMat, 4.9, 1.75, 14.06);

  /* ================= 正面：玻璃 / 雨棚 / 招牌 ================= */

  // 橱窗玻璃：门在 x0..1.6，左右两片。
  // 用薄板不用 box：玻璃在 8m 外，厚度读不出来，但平面省一半顶点。
  faceS(3.4, 2.19, glassMat, -1.85, 1.445, ST.zFront - 0.01);
  faceS(4.0, 2.19, glassMat, 3.7, 1.445, ST.zFront - 0.01);
  // 竖向分格 + 下墙裙（墙裙用深灰蓝，和上面的冷灰墙拉开明度）
  for (const x of [-3.55, -0.15, 1.7, 5.7]) put(bx(0.08, 2.19, 0.08), metal, x, 1.445, ST.zFront - 0.04);
  put(bx(ST_W, 0.35, 0.12), wallDeep, ST_CX, 0.175, ST.zFront - 0.03);
  // 立柱（左右角柱，撑起雨棚）
  put(bx(0.22, ST.eave, 0.22), metal, ST.x0 + 0.02, ST.eave / 2, ST.zFront - 0.06);
  put(bx(0.22, ST.eave, 0.22), metal, ST.x1 - 0.02, ST.eave / 2, ST.zFront - 0.06);

  // VI 色带：横贯正面的深青绿 + 一条深蓝，压住玻璃顶边
  put(bx(ST_W, 0.34, 0.1), viGreen, ST_CX, 2.72, ST.zFront - 0.05);
  put(bx(ST_W, 0.07, 0.12), viBlue, ST_CX, 2.51, ST.zFront - 0.05);

  // 屋檐雨棚：挑出 1.25m，下沿藏一条暖光灯带（雨夜屋檐的暖光是招牌动作）。
  // 灯带朝下：PlaneGeometry 转到水平后法线朝 +Y，从街上抬头看是背面，
  // 必须 DoubleSide 才渲染——Downlight 的可见性全在这一面。
  put(bx(ST_W + 0.1, 0.14, 1.25), trim, ST_CX, 2.95, 13.35);
  flat(ST_W - 0.4, 0.2, downLight, ST_CX, 2.86, 13.02);

  // 门口地垫
  flat(1.9, 0.7, toon('#5c6673'), 0.8, 0.008, 13.72);

  /* ================= 街边设施 ================= */

  // 自动贩卖机（店西侧人行道）：街角最容易被读出来的标志物。
  // 两台并排，色温岔开；机身是深灰蓝，不是高饱和的霓虹箱。
  const vending = (x: number, panel: string, strip: string) => {
    put(bx(1.05, 1.9, 0.72), toon('#2c3a4e'), x, 0.95, 13.3);
    put(bx(1.09, 0.09, 0.76), toon('#1f2a38'), x, 1.94, 13.3);   // 顶盖阴影缝
    faceS(0.88, 1.06, emissive(panel), x, 1.28, 12.93);
    faceS(0.88, 0.3, emissive(strip), x, 0.5, 12.93);
    put(bx(0.16, 0.2, 0.06), metal, x + 0.36, 1.0, 12.95);       // 投币口 / 操作板
  };
  vending(-5.75, '#d2dfeb', '#dd9a63');
  vending(-4.6, '#dde8f1', '#5fae9f');

  // 垃圾桶（带盖的圆柱）+ 雨伞架 + 自行车
  put(new THREE.CylinderGeometry(0.28, 0.25, 0.82, 10), toon('#454e5b'), -3.35, 0.41, 13.2);
  put(new THREE.CylinderGeometry(0.3, 0.3, 0.06, 10), metal, -3.35, 0.85, 13.2);
  put(bx(0.5, 0.55, 0.34), metal, 2.6, 0.28, 13.25);
  for (let i = 0; i < 3; i++) put(new THREE.CylinderGeometry(0.02, 0.02, 0.85, 5), metal, 2.48 + i * 0.12, 0.72, 13.25);
  for (const bxp of [4.3, 5.35]) {
    const wheel = new THREE.TorusGeometry(0.27, 0.028, 6, 14);
    put(wheel, metal, bxp - 0.33, 0.28, 13.3);
    put(wheel, metal, bxp + 0.33, 0.28, 13.3);
    put(bx(0.72, 0.05, 0.05), metal, bxp, 0.42, 13.3);
    put(bx(0.05, 0.3, 0.05), metal, bxp + 0.3, 0.55, 13.3);
  }

  // 空调外机：东山墙临巷两台（背面那两台永远看不到，不建）
  put(bx(0.78, 0.56, 0.34), metal, ST.x1 + 0.18, 0.62, 18.2);
  put(bx(0.78, 0.56, 0.34), metal, ST.x1 + 0.18, 0.62, 19.6);
  // 巷口壁灯：把巷子点亮一点，巷子全黑会像一条缝。
  // 一盏朝街（-Z）招徕，一盏朝巷子（+X）照亮巷壁——朝巷那盏得单独转 +π/2。
  // 巷子东移后，巷子的西壁换成居酒屋东山墙，所以这灯挂在 IZ.x1 上（仍属本函数绘制）。
  put(bx(0.2, 0.14, 0.1), metal, IZ.x1 + 0.12, 2.4, 15.4);
  faceS(0.16, 0.1, innerLight, IZ.x1 + 0.18, 2.34, 15.42);
  const laneLamp = new THREE.Mesh(new THREE.PlaneGeometry(0.16, 0.1), innerLight);
  laneLamp.position.set(IZ.x1 + 0.24, 2.34, 15.4);
  laneLamp.rotation.y = Math.PI / 2;
  g.add(laneLamp);

  // 街角护栏：东北角人行道转角，两根横管 + 四根立柱。
  // 随巷子东移，转角现在在 ALLEY.x0 处（原 7.0 = 旧巷西壁 6.6 + 0.4）。
  for (let i = 0; i < 2; i++) put(bx(0.06, 0.06, 1.3), metal, ALLEY.x0 + 0.4, 0.5 + i * 0.36, 13.35);
  for (let i = 0; i < 4; i++) put(bx(0.07, 0.78, 0.07), metal, ALLEY.x0 + 0.4, 0.39, 12.78 + i * 0.38);

  // 路牌（深灰蓝底白条，巷口标识）；同样跟着巷口东移（原 6.95）
  put(new THREE.CylinderGeometry(0.035, 0.035, 2.0, 6), metal, ALLEY.x0 + 0.35, 1.0, 13.0);
  faceS(0.46, 0.62, toon('#3f5a78'), ALLEY.x0 + 0.35, 2.28, 13.02);
  faceS(0.36, 0.1, posterMat, ALLEY.x0 + 0.35, 2.36, 12.99);
  faceS(0.36, 0.1, posterMat, ALLEY.x0 + 0.35, 2.2, 12.99);

  // 排水沟：车行道两侧的深色窄条，打破大片沥青的单调
  flat(44, 0.24, toon('#2b3242'), 0, 0.003, 7.68);
  flat(44, 0.24, toon('#2b3242'), 0, 0.003, 12.52);

  // 斑马线：正对店门（x≈1.1），从便利店横穿整个车行道到公寓楼一侧。
  //   正确方向 = 整组旋转 90°：原版白条是"沿 Z 的细条、沿 X 排开"，俯视被读成
  //   "沿街走"；这里把白条改为沿 X（顺街方向）的长条、沿 Z（店→公寓的过街
  //   方向）排开。条数保持 7 条不变（只旋转、不增减）；每条 4.6m(沿X)×0.45m(沿Z)，
  //   沿 Z 间距 0.9m 横跨整条车行道（z 7.6..12.6）。标线压浅灰蓝更贴夜景。
  for (let i = 1; i < 6; i++) flat(4.6, 0.45, toon('#c3cad1'), 1.1, 0.004, 10.1 + (i - 3) * 0.9);

  // 停车位：便利店一侧路缘的平行车位（沿 X 顺路边停，不再横在马路中间）。
  //   车行道 z 7.6..12.6；路缘在 z=12.6。车位贴着路缘、车身朝 +X 顺街排，
  //   占 z≈12.6..13.84 的便道，空出整条车行道。白框画成顺路边的小矩形。
  //   放到西侧 x≈-11/-8，避开店门斑马线(x≈-0.7..2.8)与自动贩卖机(x≈-6.3..-4.0)。
  const CZ = 13.34;                       // 车身中心 z：路缘侧(12.6)贴边，车身伸向店前
  for (const cx of [-11.0, -8.0]) {
    flat(3.5, 0.05, toon('#c3cad1'), cx, 0.004, 13.86);    // 近店侧线（沿 X）
    flat(3.5, 0.05, toon('#c3cad1'), cx, 0.004, 12.58);    // 近路缘线（沿 X，贴路缘）
    flat(0.05, 1.36, toon('#c3cad1'), cx - 1.75, 0.004, 13.22);  // 端线
    flat(0.05, 1.36, toon('#c3cad1'), cx + 1.75, 0.004, 13.22);  // 端线
  }
  const carMat = toon('#6f8296');
  const CxP = -11.0;                       // 第一格停一台车
  put(bx(3.3, 0.66, 1.48), carMat, CxP, 0.63, CZ);         // 车身长轴沿 X（顺路边）
  put(bx(1.9, 0.54, 1.34), carMat, CxP + 0.2, 1.18, CZ);   // 车顶（驾驶舱略偏 +X）
  // 车轮：圆柱轴默认沿 Y，绕 X 转 90° 后轴沿 Z（=车宽方向，车身顺 X 停）
  for (const [wx, wz] of [[CxP + 1.3, CZ - 0.74], [CxP + 1.3, CZ + 0.74], [CxP - 1.3, CZ - 0.74], [CxP - 1.3, CZ + 0.74]]) {
    const wheel = new THREE.CylinderGeometry(0.27, 0.27, 0.17, 10);
    wheel.rotateX(Math.PI / 2);
    put(wheel, toon('#232936'), wx, 0.27, wz);
  }
  put(bx(0.06, 0.5, 1.3), toon('#2b3446'), CxP + 1.55, 1.15, CZ);  // 前挡风（车头朝 +X）
  // 车头朝 +X：暖白前灯在 +X 端，红尾灯在 -X 端
  put(bx(0.05, 0.08, 0.3), emissive('#ffd7a8'), CxP + 1.66, 0.66, CZ - 0.4);
  put(bx(0.05, 0.08, 0.3), emissive('#ffd7a8'), CxP + 1.66, 0.66, CZ + 0.4);
  put(bx(0.05, 0.07, 0.26), emissive('#d1503f'), CxP - 1.66, 0.68, CZ - 0.4);
  put(bx(0.05, 0.07, 0.26), emissive('#d1503f'), CxP - 1.66, 0.68, CZ + 0.4);

  /* ---------------- 小巷：居酒屋东侧 ALLEY.x0..ALLEY.x1 ----------------
   *
   * 巷东原来有一栋 5.4×11.5×13.5 的低模 alley-block（深蓝 #2b3346）封住
   * 纵深。后来书店 buildStreetBookStore()（CX=16.2, W=7.0 ⇒ 西墙 x=12.7）
   * 坐到了同一个位置上，从 203 阳台望过去那栋深蓝方塔会透过书店看得到
   * —— 就是用户报的"居酒屋东侧深色长方体"。整块删掉，巷东的墙由
   * 书店西墙接手；横跨电线一并改挂到书店西墙上。
   *
   * 巷子随之收窄到 1.0m（IZ.x1=11.7 到书店西墙 12.7），电线跨距也短了，
   * sag 等比缩小（0.35/0.40 → 0.14/0.16）才不会垂到地面。
   */
  const wireMat = toon('#151a26');
  const awire = (a: [number, number, number], b: [number, number, number], sag: number) => {
    const mid = new THREE.Vector3((a[0] + b[0]) / 2, Math.min(a[1], b[1]) - sag, (a[2] + b[2]) / 2);
    const curve = new THREE.QuadraticBezierCurve3(new THREE.Vector3(...a), mid, new THREE.Vector3(...b));
    g.add(new THREE.Mesh(new THREE.TubeGeometry(curve, 10, 0.012, 4), wireMat));
  };
  // 两根电线：起点居酒屋檐口 3.3（居酒屋比便利店矮，eave 3.5 那套用不上），
  // 终点改挂书店西墙（x=12.7），y 收到 4.2/4.0 避开书店二层窗（y≈3.4 顶）。
  // z 仍取 15.0/15.8：落在居酒屋东山墙的 z 范围 [14.02,16.11] 内。
  awire([IZ.x1, 3.3, 15.0], [12.7, 4.2, 15.0], 0.14);
  awire([IZ.x1, 3.3, 15.8], [12.7, 4.0, 15.8], 0.16);

  // 便利店这一侧的路灯整排已上移到 buildStreetscape（STORE_LAMP_X，z12.9，
  // 挑臂 -Z 伸到车行道上方），与公寓侧两排一起沿整条路串亮，这里不再单独放。

  /* ================= 美术增厚（屋顶 / 招牌 / 街头陈设 / 店内充实） =================
   *
   * 原始版只有「横招牌 + 玻璃 + 货架 + 冷饮柜 + 关东煮」四件大事，贴着玻璃看
   * 进去会有点空。日式便利店真正的辨识是密度——屋顶满满的设备、侧墙伸出的
   * 突き出し看板、门口的立て看板和掲示板、店内便当 / 饭团 / 冷冻柜各自的发光
   * 玻璃柜。这里在不破坏浅盒纵深截断（z16.4 不被穿出）的前提下把密度补上。
   *
   * 全部进 sceneCollideSkip=false 的静态组，跟着 store.group 一起合批 → 描边 →
   * 冻结。屋顶设备和街头小件另起 sub-group 标 noCollide，避免给玩家凭空多出
   * 一堆挡路的薄板。
   */
  {
    /* ---------- 屋顶设备 ---------- */
    // 屋顶楼梯间（便利店二楼 / 仓库的出入口）：典型日式商业楼顶都有
    // 这个小方盒子 + 一扇小铁门。位置：东南角。
    put(bx(1.6, 1.4, 1.8), trim, ST_CX + 3.7, ST.parapet + 0.7, 17.6);
    faceS(0.7, 1.2, toon('#3a4250'), ST_CX + 3.7 + 0.81, ST.parapet + 0.7, 17.5);  // 铁门
    put(bx(0.6, 0.04, 1.4), metal, ST_CX + 3.7, ST.parapet + 1.42, 17.6);          // 门顶雨棚

    // 排气口 / 通风机组：横排 3 个，在楼梯间西侧
    for (let i = 0; i < 3; i++) {
      const x = ST_CX - 2.5 + i * 1.6;
      put(bx(0.9, 0.32, 0.7), toon('#9aa2aa'), x, ST.parapet + 0.16, 17.5);
      put(bx(0.7, 0.06, 0.5), toon('#454e5b'), x, ST.parapet + 0.34, 17.5);          // 顶盖
      put(bx(0.04, 0.18, 0.5), metal, x, ST.parapet + 0.16, 17.5);                   // 立柱
    }
    // 太阳能热水器筒（街角日式建筑常见的圆筒水箱）：西端
    put(new THREE.CylinderGeometry(0.28, 0.28, 0.7, 12), toon('#5a6470'), ST_CX - 4.6, ST.parapet + 0.35, 18.6);
    put(new THREE.CylinderGeometry(0.32, 0.32, 0.04, 12), metal, ST_CX - 4.6, ST.parapet + 0.72, 18.6);

    /* ---------- 侧墙突き出し看板（竖招牌）---------- */
    // 从西墙（x=ST.x0=-4.4）伸出一根金属托臂，末端挂一块竖长灯箱。
    // 灯箱两面（朝 +Z 街 / 朝 -Z 公寓）独立贴同一张贴图——从街上沿 X 走
    // 任何方向都能读到店名，比单条横招牌辨识度高一档。
    // 托臂从墙伸到 x=-5.3，留 0.9m 间隙不挡自动贩卖机（贩卖机 x=-5.75）。
    put(bx(0.05, 0.05, 0.9), metal, -4.55, 3.45, 13.95);                              // 上托臂
    put(bx(0.05, 0.05, 0.9), metal, -4.55, 2.65, 13.95);                              // 下托臂
    const vSignW = 0.7, vSignH = 1.6;
    put(bx(0.04, vSignH, 0.5), viGreen, -5.1, 3.05, 13.7);                              // 侧条（顶底）
    const vSign = new THREE.Mesh(new THREE.PlaneGeometry(vSignW, vSignH), emissive('#ffffff', { map: verticalSignTexture() }));
    vSign.position.set(-5.1, 3.05, 13.7);
    vSign.rotation.y = Math.PI;
    g.add(vSign);
    // 朝公寓那一面：单独建一个 mesh（法线朝 +Z），rotation.y = 0 即可
    const vSignB = new THREE.Mesh(new THREE.PlaneGeometry(vSignW, vSignH), emissive('#ffffff', { map: verticalSignTexture() }));
    vSignB.position.set(-5.1, 3.05, 14.2);
    vSignB.rotation.y = 0;
    g.add(vSignB);
    // 灯箱顶底各一小灯顶 + 下沿一道暖白小灯条
    put(bx(0.78, 0.06, 0.56), emissive('#fff1dc'), -5.1, 3.93, 13.95);
    put(bx(0.78, 0.06, 0.56), emissive('#fff1dc'), -5.1, 2.17, 13.95);

    /* ---------- 街头陈设：立て看板 + 掲示板 ---------- */
    // 立て看板（A 字招牌）：雨里立在门口偏东、靠便道边。底框深木、面
    // 板暖白贴「おすすめ」海报。一腿深一腿浅（透视的便宜做法）。
    const sb = new THREE.Group();
    sb.name = 'sandwich-board';
    const sbMat = toon('#ffffff', { map: sandwichBoardTexture() });
    const sbPanel = new THREE.Mesh(new THREE.PlaneGeometry(0.6, 0.75), sbMat);
    sbPanel.position.set(0, 0.85, 0);
    sb.add(sbPanel);
    // 木框（4 边）
    for (const [w, h, x, y] of [[0.6, 0.04, 0, 1.225], [0.6, 0.04, 0, 0.485], [0.04, 0.76, -0.28, 0.85], [0.04, 0.76, 0.28, 0.85]] as const) {
      const f = new THREE.Mesh(new THREE.BoxGeometry(w, h, 0.02), toon('#5a4a36'));
      f.position.set(x, y, 0.01);
      sb.add(f);
    }
    // A 形支架：两根斜撑
    const legGeo = new THREE.CylinderGeometry(0.012, 0.012, 0.85, 5);
    const legL = new THREE.Mesh(legGeo, toon('#3a2a18'));
    legL.position.set(-0.22, 0.42, -0.05);
    legL.rotation.z = 0.18;
    sb.add(legL);
    const legR = new THREE.Mesh(legGeo, toon('#3a2a18'));
    legR.position.set(0.22, 0.42, -0.05);
    legR.rotation.z = -0.18;
    sb.add(legR);
    sb.position.set(2.5, 0.01, 13.85);
    sb.rotation.y = Math.PI;          // 面板朝街（-Z）
    sb.userData.sceneCollideSkip = true;
    g.add(sb);

    // 掲示板（玻璃门公告栏）：立在西侧人行道、靠近自行车一侧。
    // 单柱 + 玻璃门柜，内贴几张通知+雨滴玻璃反光。
    const bb = new THREE.Group();
    bb.name = 'bulletin-board';
    const bbPost = new THREE.Mesh(new THREE.CylinderGeometry(0.02, 0.02, 1.8, 5), toon('#454e5b'));
    bbPost.position.set(-3.4, 0.9, 13.85);
    bb.add(bbPost);
    const bbPanel = new THREE.Mesh(new THREE.PlaneGeometry(0.86, 1.16), toon('#ffffff', { map: bulletinBoardTexture() }));
    bbPanel.position.set(-3.4, 1.55, 13.85);
    bbPanel.rotation.y = Math.PI;
    bb.add(bbPanel);
    // 玻璃门：薄板，比面板大 2cm，给一圈金属包边
    const bbGlass = new THREE.Mesh(new THREE.PlaneGeometry(0.9, 1.2), toon('#dfeaf2', { finish: 'glass', transparent: true, opacity: 0.12, depthWrite: false }));
    bbGlass.position.set(-3.4, 1.55, 13.83);
    bbGlass.rotation.y = Math.PI;
    bb.add(bbGlass);
    // 顶盖
    const bbCap = new THREE.Mesh(new THREE.BoxGeometry(0.95, 0.04, 0.18), toon('#3a4250'));
    bbCap.position.set(-3.4, 2.16, 13.85);
    bb.add(bbCap);
    bb.userData.sceneCollideSkip = true;
    g.add(bb);

    /* ---------- 店内充实 ---------- */
    // おにぎり冷藏陈列（进门右手第一件）：低矮的玻璃柜，三排饭团清晰可见。
    // 位置：门内东侧（x 1.8..3.0），紧贴 shelf row 0 前方。
    const OG_X0 = 1.85, OG_W = 1.15, OG_Z0 = 14.22;
    put(bx(OG_W, 0.95, 0.5), toon('#7f93a4'), OG_X0 + OG_W / 2, 0.475, OG_Z0 + 0.25).name = 'onigiri-cooler';
    // 玻璃门：冷白内胆 + 贴饭团贴图
    const ogInner = new THREE.Mesh(new THREE.PlaneGeometry(OG_W - 0.06, 0.62), emissive('#dfeaf2', { map: onigiriCoolerTexture() }));
    ogInner.position.set(OG_X0 + OG_W / 2, 0.65, OG_Z0 + 0.03);
    ogInner.rotation.y = Math.PI;
    g.add(ogInner);
    // 玻璃门（覆盖一层）
    const ogGlass = new THREE.Mesh(new THREE.PlaneGeometry(OG_W - 0.06, 0.62), toon('#dce9f2', { finish: 'glass', transparent: true, opacity: 0.2, depthWrite: false }));
    ogGlass.position.set(OG_X0 + OG_W / 2, 0.65, OG_Z0 + 0.01);
    ogGlass.rotation.y = Math.PI;
    g.add(ogGlass);
    // 顶沿一道金属亮边
    put(bx(OG_W, 0.02, 0.04), shelfMetal, OG_X0 + OG_W / 2, 0.97, OG_Z0 + 0.04);

    // 弁当保温陈列（收银台与关东煮之间）：暖黄内胆让便利店更"温暖"。
    // 位置：x -2.35..-1.5，z 14.4..14.85（夹在 oden 和 shelf row 0 之间）。
    const BW_X0 = -2.3, BW_W = 0.85, BW_Z0 = 14.42;
    put(bx(BW_W, 0.95, 0.45), toon('#a87a4a'), BW_X0 + BW_W / 2, 0.475, BW_Z0 + 0.225).name = 'bento-warmer';
    const bwInner = new THREE.Mesh(new THREE.PlaneGeometry(BW_W - 0.06, 0.65), emissive('#f1d99a', { map: bentoWarmerTexture() }));
    bwInner.position.set(BW_X0 + BW_W / 2, 0.62, BW_Z0 + 0.03);
    bwInner.rotation.y = Math.PI;
    g.add(bwInner);
    const bwGlass = new THREE.Mesh(new THREE.PlaneGeometry(BW_W - 0.06, 0.65), toon('#f6e4be', { finish: 'glass', transparent: true, opacity: 0.16, depthWrite: false }));
    bwGlass.position.set(BW_X0 + BW_W / 2, 0.62, BW_Z0 + 0.01);
    bwGlass.rotation.y = Math.PI;
    g.add(bwGlass);
    put(bx(BW_W, 0.02, 0.04), shelfMetal, BW_X0 + BW_W / 2, 0.97, BW_Z0 + 0.04);

    // アイス冷冻柜（杂志架前方的卧式冷柜）：玻璃上盖 + 冰淇淋阵列。
    // 位置：x 5.4..6.2，z 14.3..14.7（杂志架前的过道）。
    const FZ1 = 14.32;
    put(bx(0.8, 0.7, 0.5), toon('#5a6470'), 5.8, 0.35, FZ1 + 0.25).name = 'freezer';
    const fzLid = new THREE.Mesh(new THREE.PlaneGeometry(0.78, 0.46), emissive('#dfe8f2', { map: freezerTexture() }));
    fzLid.position.set(5.8, 0.71, FZ1 + 0.03);
    fzLid.rotation.x = -Math.PI / 2;
    fzLid.position.y = 0.72;
    g.add(fzLid);
    // 上沿"ICE"贴一条（玻璃上的标）
    put(bx(0.82, 0.05, 0.04), shelfMetal, 5.8, 0.74, FZ1 + 0.02);

    // 宅配ボックス（小型包裹自提柜）：后墙东侧空档里。
    // 位置：x 5.6..6.4，z 15.85..16.35。3×2 共 6 格小柜，每格都能开。
    const LX0 = 5.62, LY0 = 0.0, LZ0 = 15.88;
    put(bx(0.84, 1.4, 0.52), toon('#9aa2aa'), LX0 + 0.42, 0.7, LZ0 + 0.26).name = 'locker';
    for (let r = 0; r < 2; r++) {
      for (let c = 0; c < 3; c++) {
        const cx = LX0 + 0.16 + c * 0.26;
        const cy = LY0 + 1.05 - r * 0.42;
        put(bx(0.22, 0.36, 0.04), toon('#c1c8ce'), cx, cy, LZ0 + 0.02);
        // 把手
        put(bx(0.02, 0.04, 0.02), metal, cx + 0.07, cy - 0.12, LZ0 + 0.04);
        // 编号小标（一条深色短线）
        put(bx(0.08, 0.02, 0.005), toon('#3a4250'), cx - 0.04, cy + 0.1, LZ0 + 0.025);
      }
    }

    /* ---------- 吊り下げ POP（天花板挂的小促销旗）---------- */
    // 沿进门到后墙的天花板挂一排三角小旗，z 14.5（过道上方），y 2.55。
    const popMat = toon('#ffffff', { map: popBannerTexture(), side: THREE.DoubleSide });
    for (const [x, z] of [[-2.2, 14.5], [0.2, 14.55], [2.4, 14.5], [4.5, 14.55]] as const) {
      const pop = new THREE.Mesh(new THREE.PlaneGeometry(0.42, 0.32), popMat);
      pop.position.set(x, 2.6, z);
      g.add(pop);
      // 旗顶小绳
      put(new THREE.CylinderGeometry(0.004, 0.004, 0.3, 4), toon('#1a1410'), x, 2.76, z);
      pop.userData.sceneCollideSkip = true;
    }

    /* ---------- 店内玻璃后面的补充海报（再贴两张，错落）---------- */
    // 玻璃后面再贴一张「ポイント 5倍」（暖红，比现有的米白更有存在感）。
    faceS(0.7, 1.0, toon('#ffffff', { map: posterBTexture() }), 0.6, 1.55, 14.06);
    faceS(0.55, 0.75, posterMat, -3.9, 1.85, 14.06);
    faceS(0.55, 0.75, posterMat, 5.4, 1.7, 14.06);
  }

  /* ================= 动效件 =================
   * 三组：招牌灯箱 / 自动门 / 红绿灯。都不能进合批（材质要单独改），
   * 也都不能冻结（每帧改矩阵或颜色）。描边在这里各自加好。
   */

  // 1) 招牌灯箱。用 emissive 而不是 toon：灯箱自己就是光源，不该吃场景光，
  //    而且改 color 就是改亮度——toon 材质改的是漫反射色，改不出"灯在闪"。
  //    亮度压到 0.92 基准：灯箱自己会起光晕，再亮就过曝成一整条白。
  const signMat = emissive('#ffffff', { map: signTexture() });
  const SIGN_BASE = new THREE.Color(0.92, 0.92, 0.92);
  const sign = new THREE.Mesh(new THREE.PlaneGeometry(9.2, 0.76), signMat);
  sign.position.set(ST_CX + 0.1, 3.5, ST.zFront - 0.12);
  sign.rotation.y = Math.PI;
  sign.name = 'store-sign';
  dyn.add(sign);

  // 2) 自动门：两扇玻璃推拉门 + 门头灯箱
  const DOOR_LX = 0.4, DOOR_RX = 1.2, DOOR_OPEN = 0.74;
  const leafMat = toon('#d7e5ef', { finish: 'glass', transparent: true, opacity: 0.22, depthWrite: false });
  const mkLeaf = (cx: number) => {
    const leaf = new THREE.Group();
    leaf.name = 'store-door-leaf';
    const glass = new THREE.Mesh(new THREE.PlaneGeometry(0.76, 2.18), leafMat);
    glass.rotation.y = Math.PI;
    leaf.add(glass);
    for (const dx of [-0.38, 0.38]) {
      const bar = new THREE.Mesh(bx(0.05, 2.2, 0.05), metal);
      bar.position.set(dx, 0, 0.02);
      leaf.add(bar);
    }
    const rail = new THREE.Mesh(bx(0.78, 0.06, 0.06), metal);
    rail.position.set(0, 1.09, 0.02);
    leaf.add(rail);
    leaf.position.set(cx, 1.15, ST.zFront - 0.05);
    return leaf;
  };
  const leafL = mkLeaf(DOOR_LX);
  const leafR = mkLeaf(DOOR_RX);
  // 门扇必须自己当一次描边的 root：壳是挂在 root 上的，挂在门框组下就会被
  // 烘焙成固定几何，门一滑开描边留在原地。门框另起一组，两边互不 traverse。
  outlineProp(leafL);
  outlineProp(leafR);
  dyn.add(leafL, leafR);

  // 门框 + 门头灯箱（门楣那一道亮边，把入口从玻璃面里拎出来）
  const entryMat = emissive('#fff1dc');
  const ENTRY_BASE = entryMat.color.clone();
  const doorFrame = new THREE.Group();
  doorFrame.name = 'store-door-frame';
  for (const dx of [-0.06, 1.66]) {
    const jamb = new THREE.Mesh(bx(0.09, 2.42, 0.09), metal);
    jamb.position.set(dx, 1.21, ST.zFront - 0.04);
    doorFrame.add(jamb);
  }
  const head = new THREE.Mesh(new THREE.PlaneGeometry(1.72, 0.2), entryMat);
  head.position.set(0.8, 2.36, ST.zFront - 0.06);
  head.rotation.y = Math.PI;
  doorFrame.add(head);
  dyn.add(doorFrame);

  // 3) 红绿灯：悬臂杆 + 三色灯。挂在小巷口上方，朝 -Z（对着公寓）
  const sigGroup = new THREE.Group();
  sigGroup.name = 'traffic-signal';
  const pole = new THREE.Mesh(new THREE.CylinderGeometry(0.07, 0.09, 4.4, 8), metal);
  // 巷口东移后整组跟着走（原 8.1 / 7.15 / 6.42，相对旧巷西壁 6.6 的偏移保持不变）
  pole.position.set(ALLEY.x0 + 1.5, 2.2, 12.95);
  sigGroup.add(pole);
  const arm = new THREE.Mesh(bx(1.9, 0.08, 0.08), metal);
  arm.position.set(ALLEY.x0 + 0.55, 4.32, 12.95);
  sigGroup.add(arm);
  const boxMat = toon('#2a3244');
  const housing = new THREE.Mesh(bx(0.3, 0.82, 0.22), boxMat);
  housing.position.set(ALLEY.x0 - 0.18, 3.9, 12.95);
  sigGroup.add(housing);
  const LIGHT_ON = [new THREE.Color('#d1483c'), new THREE.Color('#dcb853'), new THREE.Color('#48c081')];
  const LIGHT_OFF = [new THREE.Color('#472a2a'), new THREE.Color('#453e27'), new THREE.Color('#274034')];
  // 灯泡走 emissive：它自动带 outlineWeight 0，描边那一步不会给发光体糊黑壳
  const bulbMats = [0, 1, 2].map((i) => emissive(LIGHT_OFF[i]));
  [0, 1, 2].forEach((i) => {
    const m = new THREE.Mesh(new THREE.CircleGeometry(0.085, 12), bulbMats[i]);
    m.position.set(ALLEY.x0 - 0.18, 4.15 - i * 0.27, 12.83);
    m.rotation.y = Math.PI;
    sigGroup.add(m);
  });
  dyn.add(sigGroup);

  // 描边：门框和信号灯各自成组（门扇在上面已经单独描过）。
  // 发光体和透明面由 addOutline 自己跳过，这里不用额外标记。
  outlineProp(doorFrame);
  outlineProp(sigGroup);

  /* ---------------- 美术增厚 · 动效 ----------------
   *
   * 这三件不能进合批（材质 / UV 每帧改），也都不进 store.group 合批。
   * 它们各自描边、走 dyn。
   */

  // 4) 玻璃上的雨水（UV 慢速往下滚 + 偶发水痕）。复用 toon.ts 的 rainGlassTexture()。
  //    两片玻璃各一片，独立滚 UV 让接缝不齐。
  const rainMats = [-1, 1].map((i) => {
    const mat = emissive('#dfeaf2', { map: rainGlassTexture(), transparent: true, opacity: 0.22, side: THREE.DoubleSide });
    mat.map!.wrapS = mat.map!.wrapT = THREE.RepeatWrapping;
    mat.map!.repeat.set(1, 1.05);
    mat.map!.offset.set(0, i * 0.5);
    mat.userData.outlineWeight = 0;
    return mat;
  });
  const rainSpecs: Array<{ w: number; cx: number }> = [
    { w: 3.4, cx: -1.85 },
    { w: 4.0, cx: 3.7 },
  ];
  const rainMeshes = rainSpecs.map((s, i) => {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(s.w, 2.19), rainMats[i]);
    m.position.set(s.cx, 1.445, ST.zFront - 0.06);
    m.rotation.y = Math.PI;
    m.userData.sceneCollideSkip = true;
    return m;
  });
  for (const m of rainMeshes) dyn.add(m);

  // 5) 便利店屋檐滴水：8 滴挂在雨棚前缘 (y≈2.86, z=13.0)。与公寓屋檐共用
  //    rainGlassTexture 作水珠纹理，颜色压冷蓝低透明度。缓变 sprite scale
  //    像水珠欲滴未滴。
  const dripGroup = new THREE.Group();
  dripGroup.name = 'store-eave-drips';
  const dripTex = rainGlassTexture();
  const dripColor = new THREE.Color('#bcd2ee');
  type Drip = { sprite: THREE.Sprite; baseH: number; phase: number; freq: number };
  const drips: Drip[] = [];
  const DRIP_X0 = ST.x0 + 0.5, DRIP_X1 = ST.x1 - 0.5, DRIP_Y = 2.83, DRIP_Z = 13.0;
  const DRIP_N = 8;
  for (let i = 0; i < DRIP_N; i++) {
    const t = (i + 0.5) / DRIP_N;
    const x = DRIP_X0 + t * (DRIP_X1 - DRIP_X0);
    const w = 0.05 + (i % 3) * 0.012;
    const h = 0.22 + (i % 4) * 0.05;
    const mat = new THREE.SpriteMaterial({
      map: dripTex,
      color: dripColor,
      transparent: true,
      opacity: 0.45,
      depthWrite: false,
      blending: THREE.NormalBlending,
    });
    const s = new THREE.Sprite(mat);
    s.position.set(x, DRIP_Y - h * 0.4, DRIP_Z);
    s.scale.set(w, h, 1);
    s.renderOrder = 4;
    s.userData.sceneCollideSkip = true;
    dripGroup.add(s);
    drips.push({ sprite: s, baseH: h, phase: i * 0.93, freq: 0.55 + (i % 3) * 0.25 });
  }
  dyn.add(dripGroup);

  // 6) 营业中（OPEN）小灯牌：门右侧贴一张 0.32×0.64 的小灯牌，
  //    暖底 + 偶发微抖（与主招牌不同步，避免抢节奏）。
  const openMat = emissive('#ffffff', { map: openSignTexture(), side: THREE.DoubleSide });
  const OPEN_BASE = new THREE.Color(0.92, 0.92, 0.92);
  const openSign = new THREE.Mesh(new THREE.PlaneGeometry(0.32, 0.64), openMat);
  openSign.position.set(1.78, 1.85, ST.zFront - 0.07);
  openSign.rotation.y = Math.PI;
  openSign.userData.sceneCollideSkip = true;
  dyn.add(openSign);

  /* ---------------- 美术增厚 · 店内补光 + 暖光外溢 ----------------
   *
   * 雨夜封闭前厅方向光几乎进不来，店内表面本来只有自发光面在"假亮"。
   * 补两盏暖色点光做真实填充，让货架/地砖/饭团是被照亮的，隔着玻璃读
   * 得出"店里开着灯"；冷柜再补一盏冷光拉温度差。最后沿门口把暖光泼到
   * 湿地面上形成积水反光，并给招牌叠一层柔光晕。全部进 dyn（不合并 /
   * 不冻结，点光不投阴影所以很便宜）。
   */
  const inLight = new THREE.PointLight(0xffd6a0, 10, 16, 2);
  inLight.position.set(ST_CX, 2.5, 15.1);
  dyn.add(inLight);
  const inLight2 = new THREE.PointLight(0xffe2b4, 6, 11, 2);
  inLight2.position.set(-3.0, 2.0, 15.0);
  dyn.add(inLight2);
  const inCool = new THREE.PointLight(0xcfe2f2, 4, 9, 2);
  inCool.position.set(1.5, 1.3, 15.9);
  dyn.add(inCool);

  // 暖光外溢到湿地面：门口正前方的暖色光池（积水反光）。加法混合的径向
  // 渐变，贴地、不投阴影、不吃碰撞。颜色与店内一致，远处看像店里的光淌到路上。
  const poolMat = emissive('#ffe6bf', { map: radial('#ffe9c2', 'rgba(255,210,150,0)'), transparent: true, opacity: 0.55, side: THREE.DoubleSide });
  poolMat.blending = THREE.AdditiveBlending;
  poolMat.depthWrite = false;
  const pool = new THREE.Mesh(new THREE.PlaneGeometry(9.5, 4.2), poolMat);
  pool.geometry.rotateX(-Math.PI / 2);
  pool.position.set(ST_CX, 0.014, 13.2);
  pool.renderOrder = 2;
  pool.userData.sceneCollideSkip = true;
  dyn.add(pool);

  // 招牌暖晕：横招牌前方叠一层柔光，雨夜里像灯箱在发烫的辉光。
  const haloMat = emissive('#ffd9a0', { map: radial('rgba(255,225,180,0.9)', 'rgba(255,200,150,0)'), transparent: true, opacity: 0.5, side: THREE.DoubleSide });
  haloMat.blending = THREE.AdditiveBlending;
  haloMat.depthWrite = false;
  const halo = new THREE.Mesh(new THREE.PlaneGeometry(11.6, 1.7), haloMat);
  halo.position.set(ST_CX + 0.1, 3.5, ST.zFront - 0.2);
  halo.renderOrder = 3;
  halo.userData.sceneCollideSkip = true;
  dyn.add(halo);

  /* ---------------- 每帧更新 ---------------- */
  const update = (t: number) => {
    // 招牌：0.955~1.0 慢呼吸 + 每 11.3s 一次 0.28s 的启辉抖动。
    // 抖动的频率（61rad/s）故意远离呼吸频率，两者不共振才像真的老灯管。
    const breathe = 0.955 + 0.045 * Math.sin(t * 1.7);
    const glitch = t % 11.3 < 0.28 ? 0.62 + 0.3 * Math.sin(t * 61) : 1;
    signMat.color.copy(SIGN_BASE).multiplyScalar(breathe * glitch);
    entryMat.color.copy(ENTRY_BASE).multiplyScalar(0.94 + 0.06 * Math.sin(t * 2.3 + 1.1));

    // 自动门：21s 一轮，进 0.9s / 保持 2.3s / 退 0.9s。
    // 纯相位驱动、不存状态——暂停再恢复也不会卡在半开。
    const ph = t % 21;
    let open = 0;
    if (ph < 0.9) open = smoothstep(ph / 0.9);
    else if (ph < 3.2) open = 1;
    else if (ph < 4.1) open = 1 - smoothstep((ph - 3.2) / 0.9);
    leafL.position.x = DOOR_LX - open * DOOR_OPEN;
    leafR.position.x = DOOR_RX + open * DOOR_OPEN;

    // 红绿灯：红 11s → 绿 9s → 黄 2s → 黄 1s（23s 一轮）
    const sp = t % 23;
    const on = sp < 11 ? 0 : sp < 20 ? 2 : 1;
    for (let i = 0; i < 3; i++) bulbMats[i].color.copy(i === on ? LIGHT_ON[i] : LIGHT_OFF[i]);

    // 玻璃雨水：UV 慢滚（每 10s 滚一轮 1.0），两片不同步
    for (let i = 0; i < rainMats.length; i++) {
      const m = rainMats[i].map!;
      m.offset.y = ((i * 0.5) - t * 0.1) % 1.0;
    }

    // 屋檐滴水：水珠欲滴未滴
    for (const d of drips) {
      const k = 0.5 + 0.5 * Math.sin(t * d.freq + d.phase);
      d.sprite.scale.y = d.baseH * (0.8 + 0.5 * k);
      d.sprite.material.opacity = 0.32 + 0.3 * k;
    }

    // 营业中：0.88 基准 + 0.08 的微呼吸（与主招牌不同步）
    const ob = 0.88 + 0.08 * Math.sin(t * 2.7 + 0.6);
    openMat.color.copy(OPEN_BASE).multiplyScalar(ob);

    // 店内暖光：极轻呼吸 + 偶发 0.4s 灯丝抖动（与招牌不同步，像老灯管），
    // 让隔着玻璃看进去的"店里开着灯"不是死亮，而是有生命的小幅明灭。
    const inB = 0.94 + 0.06 * Math.sin(t * 1.3 + 0.4);
    const inG = t % 7.3 < 0.4 ? 0.82 + 0.12 * Math.sin(t * 45) : 1;
    inLight.intensity = 10 * inB * inG;
    inLight2.intensity = 6 * (0.96 + 0.04 * Math.sin(t * 1.1 + 2.0));
    inCool.intensity = 4 * (0.95 + 0.05 * Math.sin(t * 0.9 + 1.3));
  };

  return { group: g, dynamic: dyn, update };
}

/* ============================================================================
 * Stage 2c — 近景城市节点：地铁出入口 / 居酒屋 / 小公园
 * ========================================================================== */

/**
 * 高模街边地铁出入口——不是车站站房，只是街沿上那个"往下走的口子"。
 *
 * 位置与朝向：贴街沿布置。街沿线 z=14（居酒屋北墙那条），主街在 z=6.3~14，
 * 所以地铁口开口朝 -z（面向马路），人从人行道直接踏进去。
 * 体量 x[-28,-20] / z[14,19.5]，8m 宽 —— 比公寓窄，不抢主角位，但细节给足。
 *
 * 构件：双坡玻璃雨棚 + 落水管 + 8 级下行楼梯（含扶手）+ 玻璃电梯井 +
 *       线路色站名牌 + 出入口编号 + 导向立牌 + 雨棚下照明 + 地面通风口 +
 *       入口护栏 + 柱身广告灯箱 + 自行车 + 垃圾桶 + 路灯。
 */
export function buildSubwayEntrance(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'subway-entrance';
  const r = makeRng(40000);

  // 局部工具（与 buildConvenienceStore 同模式）
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  // 带任意欧拉旋转的版本（斜杆、坡面用）
  const putR = (geo: THREE.BufferGeometry, mat: THREE.Material,
                x: number, y: number, z: number, rx = 0, ry = 0, rz = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    m.rotation.set(rx, ry, rz);
    g.add(m);
    return m;
  };
  const flatDown = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    return put(geo, mat, x, y, z);
  };
  const LM = {
    // 雨棚专用：比建筑玻璃更实一点（0.4 太透，远看只剩骨架没有"顶"）
    glass: toon('#eef4f8', { map: glassTexture(), transparent: true, opacity: 0.6 }),
    metal: toon('#8a9099', { finish: 'metal' }),
    frame: toon('#d0d4d8'),
    wallBase: toon('#6b7481'),
    dark: toon('#2a2f38'),
    slabDark: toon('#3a3f48'),
    cool: emissive('#dceaf5'),
    warmPale: toon('#fff0d4'),
    hall: emissive('#ffe4b5'),
    wood: toon('#c4a882'),
    woodDeep: toon('#8b7355'),
    // 本函数专用（近景高模，值得多开几个桶）
    tile: toon('#a9b0b6'),           // 楼梯踏面瓷砖
    conc: toon('#5f6771'),           // 混凝土侧墙
    line: emissive('#e8703a'),       // 线路色（橙）——站名牌色条 + 电梯门楣
  };

  // 位置：街沿南侧建筑带，入口北面正对主街。
  // 东缘 x=-20 与停车场（LOT x0=-19）留 1m，西侧 x=-28 是空地/人行道。
  // 203 阳台（x0,z5.5）→ 本口的视线走廊 z=5.5-0.4375x，沿线上只有平地停车场，畅通。
  const CX = -24.0, CZ = 16.5;
  const ENTRY_Z = 14.0;          // 入口线（街沿），开口朝 -z
  const W = 8.0;                 // 整口宽度

  // ---- 1) 双坡玻璃雨棚（钢框架 + 透明顶） ----
  const cy = 3.30;               // 雨棚檐口高
  const cw = W, cd = 5.0, pitch = 0.12;   // 坡度约 7°
  const cz0 = ENTRY_Z - 0.4, cz1 = ENTRY_Z + cd - 0.4;
  // 前后两片斜面
  for (const s of [-1, 1]) {
    const panel = putR(gbox(cw, 0.08, cd / 2), LM.glass,
      CX, cy + 0.30, (cz0 + cz1) / 2 + s * cd / 4, s * pitch, 0, 0);
    panel.userData.noMerge = true;
  }
  // 屋脊横梁
  put(gbox(cw + 0.12, 0.14, 0.16), LM.metal, CX, cy + 0.62, (cz0 + cz1) / 2);
  // 檐口横梁（前后各一根）
  for (const z of [cz0, cz1]) {
    put(gbox(cw + 0.12, 0.12, 0.12), LM.metal, CX, cy + 0.02, z);
  }
  // 纵向次梁（3 根，分在两坡上）
  for (const dx of [-cw / 2 + 2.0, 0, cw / 2 - 2.0]) {
    for (const s of [-1, 1]) {
      putR(gbox(0.08, 0.08, cd / 2), LM.metal,
        CX + dx, cy + 0.30, (cz0 + cz1) / 2 + s * cd / 4, s * pitch, 0, 0);
    }
  }
  // 四根立柱（落在入口两侧与后端）
  for (const dx of [-cw / 2 + 0.35, cw / 2 - 0.35]) {
    for (const z of [cz0 + 0.25, cz1 - 0.25]) {
      put(gbox(0.13, cy, 0.13), LM.frame, CX + dx, cy / 2, z);
    }
  }
  // 落水管（两根，从檐口垂到地面）
  for (const dx of [-cw / 2 + 0.35, cw / 2 - 0.35]) {
    put(gbox(0.07, cy, 0.07), LM.metal, CX + dx + 0.12, cy / 2, cz1 - 0.25);
  }

  // ---- 2) 下沉竖井（挖入地下）：四壁 + 底板 + 后墙 + 站厅层 ----
  // 世界地面已在 PIT_X0..PIT_X1 / PIT_Z0..PIT_Z1 处开洞（buildExteriorGround），
  // 这里用混凝土井壁把洞口兜成一口真正的"往下走的竖井"，楼梯 / 扶梯通入井底站厅层。
  const pitX0 = PIT_X0, pitX1 = PIT_X1, pitZ0 = PIT_Z0, pitZ1 = PIT_Z1, pitD = PIT_D;
  const pitW = pitX1 - pitX0, pitL = pitZ1 - pitZ0;
  const pitCx = (pitX0 + pitX1) / 2, pitCz = (pitZ0 + pitZ1) / 2;
  const pitTopY = 0.10;          // 井壁顶略高出地面，读作收口的路缘
  // 底板（站厅层地面，顶面贴 -pitD）
  put(gbox(pitW + 0.1, 0.30, pitL + 0.1), LM.slabDark, pitCx, -pitD - 0.15, pitCz);
  // 左右井壁
  for (const s of [-1, 1]) {
    put(gbox(0.16, pitD + pitTopY + 0.05, pitL + 0.1), LM.conc,
      pitCx + s * (pitW / 2 + 0.08), (-pitD + pitTopY) / 2, pitCz);
  }
  // 后墙（井底，朝 -z 一面是通往站厅层的开口）
  put(gbox(pitW + 0.1, pitD + pitTopY + 0.05, 0.16), LM.conc, pitCx, (-pitD + pitTopY) / 2, pitZ1 + 0.08);
  // 站厅层开口（后墙底部一段暖光，读作"地下站台"在那头亮着）
  put(gbox(pitW - 0.7, 1.5, 0.06), LM.hall, pitCx, -pitD + 0.95, pitZ1 + 0.02);

  // ---- 2a) 楼梯：从街沿（y=0）下行到站厅层（y=-pitD） ----
  // 左电梯井(CX-2.6) / 中扶梯(CX+0.4) / 右楼梯(CX+2.6) 三者并排，全部落入竖井内。
  const stairX = CX + 2.6, stairW = 1.9;
  const steps = 15, riser = pitD / steps, tread = (pitL - 0.3) / steps;
  const stairTopZ = pitZ0 + 0.15;
  // 台阶
  for (let i = 0; i < steps; i++) {
    const z = stairTopZ + i * tread + tread / 2;
    const y = -i * riser;
    put(gbox(stairW, riser, tread), LM.tile, stairX, y - riser / 2, z);
  }
  // 扶手：两侧斜杆 + 立杆。
  // rx 取 +slope：绕 +X 正角让杆件远端(+z，朝井底)下沉，与梯级下行同向；
  // 取负角会朝天上翘。立杆高度恒定 0.9，从踏面插到扶手底。
  const slope = Math.atan2(steps * riser, steps * tread);
  for (const s of [-1, 1]) {
    const hx = stairX + s * (stairW / 2 - 0.05);
    putR(gbox(0.06, 0.06, steps * tread + 0.5), LM.frame,
      hx, 0.90 - (steps * riser) / 2, stairTopZ + (steps * tread) / 2, slope, 0, 0);
    for (let i = 0; i <= 3; i++) {
      const t = i / 3;
      put(gbox(0.04, 0.90, 0.04), LM.frame,
        hx, -t * steps * riser + 0.45, stairTopZ + t * steps * tread);
    }
  }

  // ---- 2b) 下行自动扶梯（站厅层 ↔ 地面，斜度 ≈30°）----
  const escX = CX + 0.4, escW = 2.3;
  const escTopZ = pitZ0 + 0.15, escRun = pitL - 0.3, escDrop = pitD;
  const escLen = Math.hypot(escRun, escDrop);
  const escAng = Math.atan2(escDrop, escRun);
  // 两侧金属裙板（斜梁）：中心在梯级线中点上方 0.42，rx=+escAng 随梯级下行
  // （中心放 +escDrop/2 会整条浮出地面、且负角反翘，读作朝上走的护栏）。
  const balu = 0.42;
  for (const s of [-1, 1]) {
    putR(gbox(0.10, 0.46, escLen), LM.metal,
      escX + s * (escW / 2), -escDrop / 2 + balu, escTopZ + escRun / 2, escAng, 0, 0);
    // 扶手带（沿斜梁顶）
    putR(gbox(0.07, 0.07, escLen), LM.dark,
      escX + s * (escW / 2 - 0.04), -escDrop / 2 + balu + 0.28, escTopZ + escRun / 2, escAng, 0, 0);
  }
  // 连续梯级（斜板 + 横向齿，读出"会动的台阶"；+escAng 让相邻梯级沿斜面连续铺排）
  const nEsc = 20;
  for (let i = 0; i < nEsc; i++) {
    const t = i / (nEsc - 1);
    const z = escTopZ + t * escRun;
    const y = -t * escDrop + 0.06;
    putR(gbox(escW - 0.12, 0.05, (escRun / nEsc) * 0.92), LM.tile, escX, y, z, escAng, 0, 0);
  }
  // 桁架底肋（梯级板下方悬挂的斜撑）
  for (let i = 1; i < nEsc; i += 3) {
    const t = i / (nEsc - 1);
    putR(gbox(0.05, 0.26, 0.05), LM.frame,
      escX, -t * escDrop - 0.10, escTopZ + t * escRun, escAng, 0, 0);
  }
  // 上/下端站房（梳齿板 + 端盖）+ 下行橙色箭头
  for (const [ez, ey, s] of [[escTopZ, 0, 1], [escTopZ + escRun, -escDrop, -1]] as [number, number, number][]) {
    put(gbox(escW + 0.18, 0.28, 0.34), LM.metal, escX, ey + 0.14, ez + s * 0.17);
    put(gbox(escW - 0.1, 0.05, 0.10), LM.dark, escX, ey + 0.30, ez + s * 0.18);
  }
  put(gbox(escW - 0.2, 0.10, 0.04), LM.line, escX, 0.42, escTopZ + 0.22);

  // ---- 3) 玻璃电梯井（无障碍口，独立于楼梯） ----
  const liftX = CX - 2.6, liftW = 2.1, liftD = 2.1, liftH = 4.4;
  // 井道玻璃四壁
  for (const [ox, oz, w, d] of [
    [0, -liftD / 2, liftW, 0.06], [0, liftD / 2, liftW, 0.06],
    [-liftW / 2, 0, 0.06, liftD], [liftW / 2, 0, 0.06, liftD],
  ] as Array<[number, number, number, number]>) {
    const p = put(gbox(w, liftH - 0.5, d), LM.glass, liftX + ox, (liftH - 0.5) / 2, ENTRY_Z + 1.1 + oz);
    p.userData.noMerge = true;
  }
  // 井道角柱
  for (const ox of [-liftW / 2, liftW / 2]) {
    for (const oz of [-liftD / 2, liftD / 2]) {
      put(gbox(0.10, liftH, 0.10), LM.frame, liftX + ox, liftH / 2, ENTRY_Z + 1.1 + oz);
    }
  }
  // 顶部机房
  put(gbox(liftW + 0.25, 0.5, liftD + 0.25), LM.metal, liftX, liftH + 0.2, ENTRY_Z + 1.1);
  // 门楣线路色条（橙色，一眼认出是地铁）
  put(gbox(liftW - 0.1, 0.16, 0.08), LM.line, liftX, 2.35, ENTRY_Z + 1.1 - liftD / 2 - 0.04);
  // 呼叫按钮柱
  put(gbox(0.10, 1.0, 0.08), LM.metal, liftX + liftW / 2 + 0.25, 0.5, ENTRY_Z + 0.1);

  // ---- 4) 站名牌（蓝底白字 + 线路色条 + 编号） ----
  const signCanvas = (() => {
    const { canvas, ctx } = makeCanvas(256, 76);
    ctx.fillStyle = '#0e4d8c';
    ctx.fillRect(0, 0, 256, 76);
    // 顶部线路色条
    ctx.fillStyle = '#e8703a';
    ctx.fillRect(0, 0, 256, 10);
    ctx.fillStyle = '#ffffff';
    ctx.font = 'bold 26px sans-serif';
    ctx.textAlign = 'left';
    ctx.fillText('西園寺前', 52, 48);
    // 地铁圆标
    ctx.beginPath(); ctx.arc(28, 38, 15, 0, Math.PI * 2);
    ctx.fillStyle = '#0e4d8c'; ctx.fill();
    ctx.strokeStyle = '#ffffff'; ctx.lineWidth = 3; ctx.stroke();
    ctx.fillStyle = '#ffffff';
    ctx.font = 'bold 17px sans-serif'; ctx.textAlign = 'center';
    ctx.fillText('M', 28, 44);
    // 编号
    ctx.font = '15px sans-serif'; ctx.textAlign = 'right';
    ctx.fillText('出口 2  Exit 2', 244, 70);
    return toTexture(canvas);
  })();
  const signMat = emissive('#0e4d8c', { map: signCanvas });
  const signMesh = new THREE.Mesh(new THREE.PlaneGeometry(2.0, 0.6), signMat);
  signMesh.position.set(CX, 2.80, ENTRY_Z - 0.12);
  signMesh.rotation.y = Math.PI;   // 面朝 -z（马路方向）
  g.add(signMesh);
  // 站名牌背板（贴图面更靠南/更近马路，背板在其北侧收厚度；顺序反了贴图会被自己挡住）
  put(gbox(2.2, 0.76, 0.07), LM.metal, CX, 2.80, ENTRY_Z - 0.02);

  // ---- 5) 出入口编号牌（独立小牌，柱身上） ----
  const numCanvas = (() => {
    const { canvas, ctx } = makeCanvas(64, 64);
    ctx.fillStyle = '#e8703a'; ctx.fillRect(0, 0, 64, 64);
    ctx.fillStyle = '#ffffff';
    ctx.font = 'bold 44px sans-serif'; ctx.textAlign = 'center';
    ctx.fillText('2', 32, 48);
    return toTexture(canvas);
  })();
  const numMat = emissive('#e8703a', { map: numCanvas });
  const numMesh = new THREE.Mesh(new THREE.PlaneGeometry(0.42, 0.42), numMat);
  numMesh.position.set(CX - cw / 2 + 0.35, 2.10, ENTRY_Z - 0.19);
  numMesh.rotation.y = Math.PI;
  g.add(numMesh);

  // ---- 6) 柱身广告灯箱（冷白，雨夜里一块亮面） ----
  const adCanvas = (() => {
    const { canvas, ctx } = makeCanvas(96, 160);
    ctx.fillStyle = '#f2f6fa'; ctx.fillRect(0, 0, 96, 160);
    ctx.fillStyle = '#1d3f66';
    ctx.fillRect(8, 14, 80, 6);
    ctx.fillRect(8, 28, 56, 5);
    ctx.fillRect(8, 130, 80, 4);
    ctx.fillStyle = '#c8d8e6'; ctx.fillRect(8, 44, 80, 76);
    ctx.fillStyle = '#8fa8bd';
    ctx.beginPath(); ctx.arc(48, 82, 22, 0, Math.PI * 2); ctx.fill();
    return toTexture(canvas);
  })();
  const adMat = emissive('#f2f6fa', { map: adCanvas });
  const adMesh = new THREE.Mesh(new THREE.PlaneGeometry(0.5, 0.85), adMat);
  adMesh.position.set(CX + cw / 2 - 0.35, 1.85, ENTRY_Z - 0.13);
  adMesh.rotation.y = Math.PI;
  g.add(adMesh);
  // 背框在贴图面北侧（贴图面朝 -z 朝马路，框只做厚度收边）
  put(gbox(0.62, 0.98, 0.08), LM.metal, CX + cw / 2 - 0.35, 1.85, ENTRY_Z - 0.02);

  // ---- 7) 导向立牌（人行道上的立式指示） ----
  const dirX = CX + cw / 2 + 0.75, dirZ = ENTRY_Z - 0.5;
  put(gbox(0.07, 1.9, 0.07), LM.metal, dirX, 0.95, dirZ);            // 立杆
  put(gbox(0.72, 0.52, 0.06), LM.warmPale, dirX, 2.05, dirZ);        // 牌面
  putR(gbox(0.30, 0.05, 0.02), LM.line, dirX, 2.05, dirZ - 0.04, 0, 0, 0);  // 箭头色条
  putR(gbox(0.20, 0.05, 0.02), LM.line, dirX + 0.06, 2.05, dirZ - 0.04, 0, 0, -Math.PI / 4);

  // ---- 8) 照明：雨棚下 3 盏冷白 + 入口地灯 ----
  for (const dx of [-2.4, 0, 2.4]) {
    put(gbox(0.44, 0.07, 0.30), LM.cool, CX + dx, cy + 0.14, ENTRY_Z + 1.4);
  }
  // 灯具吊杆
  for (const dx of [-2.4, 0, 2.4]) {
    put(gbox(0.03, 0.30, 0.03), LM.metal, CX + dx, cy + 0.30, ENTRY_Z + 1.4);
  }
  // 街沿路灯
  put(gbox(0.09, 3.2, 0.09), LM.metal, CX - cw / 2 - 0.9, 1.60, ENTRY_Z - 0.35);
  const lampHead = new THREE.Mesh(new THREE.SphereGeometry(0.15, 8, 8), LM.cool);
  lampHead.position.set(CX - cw / 2 - 0.9, 3.30, ENTRY_Z - 0.35);
  g.add(lampHead);

  // ---- 9) 入口两侧护栏（把人流导向楼梯口） ----
  for (const s of [-1, 1]) {
    const gx = CX + s * (cw / 2 - 0.35);
    put(gbox(0.08, 0.9, 1.6), LM.frame, gx, 0.45, ENTRY_Z + 0.3);
    put(gbox(0.10, 0.06, 1.6), LM.metal, gx, 0.90, ENTRY_Z + 0.3);
  }

  // ---- 10) 地面通风口（百叶格栅，带边框）—— 放在竖井口前侧街沿外，避开开洞区 ----
  const ventX = CX + 0.5, ventZ = PIT_Z0 - 0.6;
  put(gbox(1.9, 0.10, 1.2), LM.slabDark, ventX, 0.05, ventZ);
  for (let i = 0; i < 8; i++) {
    put(gbox(1.7, 0.04, 0.07), LM.frame, ventX, 0.12, ventZ - 0.5 + i * 0.14);
  }

  // ---- 11) 街边杂物：自行车 ×2 + 垃圾桶 ----
  const bike = (bx: number, bz: number, ry: number) => {
    put(gbox(0.05, 0.68, 0.05), LM.dark, bx - 0.28, 0.34, bz, ry);   // 前轮
    put(gbox(0.05, 0.68, 0.05), LM.dark, bx + 0.28, 0.34, bz, ry);   // 后轮
    put(gbox(0.62, 0.05, 0.05), LM.metal, bx, 0.72, bz, ry);         // 横梁
    put(gbox(0.05, 0.42, 0.05), LM.metal, bx - 0.10, 0.58, bz, ry);  // 座管
    put(gbox(0.30, 0.04, 0.04), LM.metal, bx + 0.20, 0.86, bz, ry);  // 车把
  };
  bike(CX - cw / 2 - 1.3, ENTRY_Z + 1.0, 0.25);
  bike(CX - cw / 2 - 1.0, ENTRY_Z + 1.9, -0.15);
  put(gbox(0.40, 0.62, 0.40), LM.slabDark, CX - cw / 2 - 1.6, 0.31, ENTRY_Z + 2.9);
  put(gbox(0.44, 0.05, 0.44), LM.metal, CX - cw / 2 - 1.6, 0.64, ENTRY_Z + 2.9);

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 街角居酒屋——紧凑日式小酒馆。木格栅门面 + 暖黄灯笼 + 小招牌 +
 * 门帘 + 玻璃门 + 雨棚 + 立式菜单牌 + 空调外机 + 饮料箱。
 * 位置：便利店正东侧（x 7.5..11.7, z 14.0..17.8），与便利店并排在同一条街沿线上，
 *   门面朝北对着马路/公寓；东侧入口 recess 开向便利店东边的小巷（见 IZ / ALLEY）。
 */
export function buildIzakaya(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'izakaya';
  const r = makeRng(41000);

  // 局部工具与材质
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const flatDown = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    return put(geo, mat, x, y, z);
  };
  const panel = (w: number, h: number, mat: THREE.Material, x: number, y: number, z: number, dir: number) => {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(w, h), mat);
    m.position.set(x, y, z);
    if (dir === -1) m.rotation.y = Math.PI; else if (dir === 1) m.rotation.y = 0;
    else if (dir === 0.5) m.rotation.y = Math.PI / 2; else m.rotation.y = -Math.PI / 2;
    g.add(m);
    return m;
  };
  const IM = {
    glass: toon('#ffffff', { map: glassTexture(), transparent: true, opacity: 0.45 }),
    metal: toon('#8a9099', { finish: 'metal' }),
    frame: toon('#d0d4d8'),
    wood: toon('#c4a882'),
    woodDeep: toon('#8b7355'),
    slabDark: toon('#3a3f48'),
    dark: toon('#2a2f38'),
    cool: emissive('#dceaf5'),
    warmPale: toon('#fff0d4'),
    hall: emissive('#ffe4b5'),
    floor: toon('#d4c4a8'),
    rmClothLit: toon('#ffffff', { map: curtainTexture() }),
    rmClothAlt: toon('#e8ddd0'),
    // 店内小物：短冊/紙 = paper，器皿 = white，酒瓶 = green / amber，赤提灯 = redLamp
    paper: toon('#f2e4cc'),
    white: toon('#e9edf1'),
    green: toon('#3f6b4a'),
    amber: toon('#a9712f'),
    red: toon('#b2382e'),
    redLamp: emissive('#ff7a3c'),
  };

  // 便利店正东侧（IZ.x0..IZ.x1），朝北面向马路与公寓。
  // 北墙 CZ-D/2=14.0 与便利店店门线 ST.zFront=14.0 齐平，两店成同一排街沿。
  const CX = (IZ.x0 + IZ.x1) / 2, CZ = 14.0 + 3.8 / 2;  // 9.6, 15.9
  const W = 4.2, D = 3.8, H = 3.2;  // 店面尺寸

  // ---- 1) 结构：三面墙（西/南/东）+ 北面敞开（朝向公寓） ----
  // 后壁（南侧）
  put(gbox(W, H, 0.12), IM.wood, CX, H / 2, CZ + D / 2);
  // 左侧壁（西侧）
  put(gbox(0.10, H, D), IM.woodDeep, CX - W / 2, H / 2, CZ);
  // 右侧壁（东侧，缩进做入口）
  put(gbox(0.10, H, D * 0.55), IM.woodDeep, CX + W / 2, H / 2, CZ - D * 0.22);
  // 天花板
  put(gbox(W, 0.08, D), IM.wood, CX, H - 0.04, CZ);
  // 地板
  put(gbox(W, 0.06, D), IM.floor, CX, 0.03, CZ);

  // ---- 2) 木格栅门面（北面，朝公寓方向） ----
  for (let i = 0; i < 8; i++) {
    put(gbox(0.04, H * 0.75, 0.04), IM.woodDeep,
      CX - W / 2 + 0.15 + i * 0.48, H * 0.42, CZ - D / 2 + 0.02);
  }

  // ---- 3) 玻璃门（右侧开口处） ----
  const doorW = 1.0, doorH = 2.2;
  const doorCx = CX + W / 2 - 0.6;
  put(gbox(doorW - 0.04, doorH - 0.04, 0.03), IM.glass, doorCx, doorH / 2, CZ - D / 2 + 0.02);
  // 门框
  put(gbox(doorW + 0.06, 0.06, 0.05), IM.frame, doorCx, doorH, CZ - D / 2 - 0.01);
  put(gbox(0.05, doorH, 0.05), IM.frame, doorCx - doorW / 2 - 0.02, doorH / 2, CZ - D / 2 - 0.01);
  // 门把手
  put(gbox(0.04, 0.04, 0.03), IM.metal, doorCx + doorW / 2 - 0.12, 1.0, CZ - D / 2);

  // ---- 4) 暖帘（noren，挂在北面门口）----
  // 下沿抬到 1.43（原来只有 0.89）：暖帘下面让出完整一条"吧台视窗"，
  // 从马路平视进来就能看到吧凳 / 吧台 / 台面小物，不然店里补再多也白补。
  // 上沿 2.01，门洞（x 10.6..11.6）不挡。
  for (let i = 0; i < 3; i++) {
    put(gbox(0.55, 0.58, 0.03), IM.rmClothLit,
      CX - 0.8 + i * 0.58, 1.72, CZ - D / 2 + 0.06);
  }

  // ---- 5) 灯笼：改成从天井垂下来的竖向吊提灯 ----
  const lanternGeo = new THREE.CylinderGeometry(0.16, 0.18, 0.28, 8);
  const lanternMat = emissive('#ffaa33', {});
  for (const [lx, lz] of [[-1.4, 0.3], [1.4, 0.3]] as Array<[number, number]>) {
    put(lanternGeo, lanternMat, CX + lx, H - 0.42, CZ + lz);
    // 吊绳（天井 3.12 到灯顶 2.92）
    put(gbox(0.012, 0.20, 0.012), IM.dark, CX + lx, 3.02, CZ + lz);
  }
  // 门口小提灯：吊在门楣（y=2.2）下
  {
    put(gbox(0.012, 0.10, 0.012), IM.dark, doorCx, 2.12, CZ - D / 2 + 0.05);
    put(new THREE.CylinderGeometry(0.10, 0.12, 0.20, 8), lanternMat,
      doorCx, 1.97, CZ - D / 2 + 0.05);
  }

  // ---- 6) 招牌 ----
  const izakayaSign = (() => {
    const { canvas, ctx } = makeCanvas(200, 56);
    // 深色木底
    const grd = ctx.createLinearGradient(0, 0, 0, 56);
    grd.addColorStop(0, '#3a2418');
    grd.addColorStop(1, '#2a1808');
    ctx.fillStyle = grd;
    ctx.fillRect(0, 0, 200, 56);
    // 字
    ctx.fillStyle = '#ffcc44';
    ctx.font = 'bold 26px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('らーめん や', 100, 36);
    ctx.font = '14px sans-serif';
    ctx.fillStyle = '#ffdd88';
    ctx.fillText('営業中', 160, 20);
    return toTexture(canvas);
  })();
  const izSignMat = emissive('#ffcc44', { map: izakayaSign });
  const izSign = new THREE.Mesh(new THREE.PlaneGeometry(1.6, 0.45), izSignMat);
  izSign.position.set(CX, H + 0.25, CZ - D / 2 + 0.02);
  // 不旋转：法线默认 +Z，朝北面向公寓
  g.add(izSign);

  // ---- 7) 雨棚 ----
  const awn = new THREE.Mesh(gbox(W + 0.3, 0.05, 0.65), IM.slabDark);
  awn.position.set(CX, H + 0.02, CZ + D / 2 + 0.30);
  awn.rotation.x = 0.08;
  g.add(awn);
  // 雨棚下灯带
  flatDown(W + 0.1, 0.18, IM.hall, CX, H - 0.02, CZ + D / 2 + 0.55);

  // ---- 8) 立式菜单牌 ----
  {
    const boardH = 0.85, boardW = 0.45;
    const board = new THREE.Mesh(
      new THREE.PlaneGeometry(boardW, boardH),
      IM.warmPale,
    );
    board.position.set(CX + W / 2 + 0.6, boardH / 2, CZ + D / 2 - 0.8);
    board.rotation.y = -Math.PI * 0.2;
    g.add(board);
    // 支架
    put(gbox(0.03, boardH, 0.03), IM.wood, CX + W / 2 + 0.4, boardH / 2, CZ + D / 2 - 0.8);
  }

  /* ---- 9) 店内陈列（透过玻璃/格栅可见）----
   *
   * 室内体积 x 7.55..11.65 / z 14.06..17.74 / y 0.06..3.12。视角决定摆法：
   * 阳台俯视看到的是"地板 + 南墙"，马路平视看到的是"暖帘以下那一条"，
   * 所以纵深分四层：土間 → 吧凳 → 客席吧台 → 厨房，南墙整面做酒棚当背景墙。
   * 原来这里只有一个吧台 + 3 张方凳 + 一个酒瓶架（还堵在门洞上），太空。
   */
  const FZ = CZ - D / 2;          // 14.00 店面（北）开口
  const BZ = CZ + D / 2 - 0.06;   // 17.74 后墙内表面
  const FL = 0.06;                // 地板面
  const cyl = (rt: number, rb: number, h: number, seg = 8) =>
    new THREE.CylinderGeometry(rt, rb, h, seg);
  const BTL = [IM.dark, IM.green, IM.amber, IM.woodDeep, IM.white, IM.rmClothAlt];
  /** 酒瓶：瓶身 + 瓶颈，k 决定颜色 */
  const bottle = (x: number, y: number, z: number, h: number, r: number, k: number) => {
    put(cyl(r, r, h, 7), BTL[k % BTL.length], x, y + h / 2, z);
    put(cyl(r * 0.34, r * 0.40, h * 0.30, 6), BTL[k % BTL.length], x, y + h * 1.15, z);
  };

  // 9-1 入口土間：水泥地 + 上がり框 + 傘立て（门洞在 x 10.6..11.6，别堵）
  put(gbox(1.30, 0.02, 1.05), IM.slabDark, 11.00, FL + 0.01, FZ + 0.58);
  put(gbox(0.05, 0.10, 1.05), IM.woodDeep, 10.35, FL + 0.05, FZ + 0.58);
  put(cyl(0.085, 0.09, 0.42, 10), IM.slabDark, 11.44, 0.27, FZ + 0.30);
  put(cyl(0.017, 0.017, 0.60, 5), IM.dark, 11.39, 0.60, FZ + 0.30);
  put(cyl(0.017, 0.017, 0.56, 5), IM.woodDeep, 11.49, 0.58, FZ + 0.32);

  // 9-2 客席吧台（L 型：正面一段 + 西端往里折一段）
  const CT_Z = FZ + 1.15;         // 15.15
  const CT_TOP = FL + 0.95;       // 1.01 天板中心
  put(gbox(2.30, 0.92, 0.55), IM.wood, 8.90, FL + 0.46, CT_Z);
  put(gbox(2.46, 0.05, 0.70), IM.warmPale, 8.90, CT_TOP, CT_Z);
  for (let i = 0; i < 9; i++) {   // 台面下的竖木条腰板
    put(gbox(0.025, 0.80, 0.02), IM.woodDeep, 7.80 + i * 0.275, FL + 0.43, CT_Z - 0.29);
  }
  put(gbox(0.55, 0.92, 0.92), IM.wood, 7.85, FL + 0.46, CT_Z + 0.71);
  put(gbox(0.72, 0.05, 1.00), IM.warmPale, 7.85, CT_TOP + 0.02, CT_Z + 0.73);

  // 9-3 吧凳 × 3（座面 + 支柱 + 底盘 + 脚踏圈）
  for (let i = 0; i < 3; i++) {
    const sx = 8.20 + i * 0.75, sz = FZ + 0.62;
    put(cyl(0.16, 0.16, 0.05, 10), IM.wood, sx, 0.635, sz);
    put(cyl(0.028, 0.032, 0.58, 6), IM.metal, sx, 0.35, sz);
    put(cyl(0.13, 0.14, 0.03, 10), IM.metal, sx, 0.10, sz);
    put(cyl(0.115, 0.115, 0.018, 10), IM.metal, sx, 0.24, sz);
  }

  // 9-4 台面上的一桌份：小皿 / 湯呑 / 徳利 / 箸置き+箸（中间那份加个啤酒杯）
  for (let i = 0; i < 3; i++) {
    const px = 8.20 + i * 0.75, pz = CT_Z + 0.05, ty = 1.035;
    put(cyl(0.075, 0.068, 0.014, 10), IM.white, px - 0.10, ty + 0.008, pz);
    put(cyl(0.036, 0.032, 0.062, 8), IM.paper, px + 0.01, ty + 0.031, pz - 0.03);
    put(cyl(0.038, 0.040, 0.13, 8), IM.green, px + 0.15, ty + 0.065, pz - 0.05);
    put(gbox(0.022, 0.012, 0.10), IM.woodDeep, px - 0.02, ty + 0.006, pz + 0.15);
    put(gbox(0.008, 0.008, 0.19), IM.wood, px - 0.02, ty + 0.015, pz + 0.15);
    if (i === 1) {
      put(cyl(0.048, 0.044, 0.135, 10), IM.amber, px - 0.26, ty + 0.068, pz - 0.02);
      put(cyl(0.049, 0.049, 0.026, 10), IM.white, px - 0.26, ty + 0.148, pz - 0.02);
    }
  }

  // 9-5 台面杂物：レジ / おしぼり桶 / 小鉢の重ね
  put(gbox(0.22, 0.14, 0.26), IM.dark, 9.88, 1.105, CT_Z - 0.02);
  put(gbox(0.20, 0.04, 0.24), IM.metal, 9.88, 1.195, CT_Z - 0.02);
  put(cyl(0.075, 0.070, 0.07, 8), IM.metal, 7.86, 1.07, CT_Z + 0.30);
  put(cyl(0.055, 0.050, 0.045, 8), IM.white, 8.52, 1.058, CT_Z - 0.14);
  put(cyl(0.050, 0.045, 0.040, 8), IM.paper, 8.52, 1.100, CT_Z - 0.14);

  // 9-6 厨房（东侧里间）：调理台 + シンク + コンロ + 鍋/やかん + 換気扇 + 吊り棚
  {
    const KZ = 16.35;
    put(gbox(0.52, 0.85, 1.90), IM.wood, 11.36, FL + 0.425, KZ);
    put(gbox(0.56, 0.04, 1.94), IM.metal, 11.36, FL + 0.87, KZ);
    put(gbox(0.40, 0.10, 0.44), IM.metal, 11.34, FL + 0.83, KZ - 0.60);   // シンク
    put(gbox(0.34, 0.03, 0.34), IM.dark, 11.34, FL + 0.87, KZ + 0.28);    // コンロ
    put(cyl(0.075, 0.075, 0.015, 10), IM.dark, 11.34, FL + 0.90, KZ + 0.28);
    put(cyl(0.11, 0.10, 0.11, 10), IM.metal, 11.34, FL + 0.945, KZ + 0.28); // 鍋
    put(cyl(0.07, 0.075, 0.13, 10), IM.metal, 11.46, FL + 0.955, KZ - 0.05); // やかん
    put(gbox(0.30, 0.02, 0.22), IM.wood, 11.34, FL + 0.90, KZ - 0.85);    // まな板
    put(gbox(0.40, 0.26, 0.52), IM.metal, 11.30, 1.86, KZ + 0.28);        // 換気扇
    put(gbox(1.00, 0.03, 0.28), IM.wood, 11.15, 2.32, KZ - 0.10);         // 吊り棚
    for (let i = 0; i < 4; i++) {                                          // 棚の器
      put(cyl(0.075, 0.060, 0.05, 8), IM.white, 10.75 + i * 0.28, 2.36, KZ - 0.10);
    }
  }

  // 9-7 南墙酒棚：地柜 + 两层瓶架 + 一升瓶 + 小瓶（俯视时是正对镜头的背景墙）
  {
    const BB_Z = BZ - 0.16;       // 17.58
    put(gbox(2.90, 0.85, 0.34), IM.wood, 9.80, FL + 0.425, BB_Z);
    put(gbox(3.00, 0.04, 0.40), IM.warmPale, 9.80, FL + 0.87, BB_Z);
    put(gbox(2.90, 0.03, 0.20), IM.wood, 9.80, 1.42, BZ - 0.10);
    put(gbox(2.90, 0.03, 0.20), IM.wood, 9.80, 1.86, BZ - 0.10);
    const BB = BZ - 0.14;         // 17.60 瓶子所在的 z
    for (let i = 0; i < 3; i++) bottle(8.58 + i * 0.40, 0.95, BB, 0.34, 0.085, i);            // 一升瓶
    for (let i = 0; i < 5; i++) bottle(9.95 + i * 0.30, 0.95, BB, 0.20 + (i % 2) * 0.05, 0.042, i + 2);
    for (let i = 0; i < 8; i++) bottle(8.45 + i * 0.33, 1.435, BB, 0.18 + (i % 3) * 0.04, 0.038, i + 1);
    for (let i = 0; i < 8; i++) bottle(8.45 + i * 0.33, 1.875, BB, 0.16 + (i % 2) * 0.05, 0.036, i + 3);
  }

  // 9-8 南墙上部：短冊菜单 + 小电视
  for (let i = 0; i < 7; i++) {
    panel(0.13, 0.48, [IM.paper, IM.red, IM.paper, IM.white, IM.paper, IM.green, IM.paper][i],
      8.52 + i * 0.34, 2.42, BZ - 0.02, -1);
  }
  panel(0.46, 0.32, IM.cool, 11.02, 2.42, BZ - 0.02, -1);

  // 9-9 冷蔵庫 + 酒樽 + ビールケース（西南角，俯视时压住画面左下）
  put(gbox(0.50, 1.50, 0.52), IM.frame, 7.88, 0.81, 17.40);
  put(gbox(0.42, 1.38, 0.02), IM.metal, 7.88, 0.81, 17.13);        // 扉
  put(gbox(0.03, 0.24, 0.03), IM.metal, 8.06, 0.86, 17.11);        // 把手
  put(cyl(0.22, 0.22, 0.50, 12), IM.wood, 8.60, 0.37, 16.80);      // 酒樽
  put(cyl(0.226, 0.226, 0.04, 12), IM.woodDeep, 8.60, 0.20, 16.80);
  put(cyl(0.226, 0.226, 0.04, 12), IM.woodDeep, 8.60, 0.56, 16.80);
  for (let i = 0; i < 2; i++) {                                     // ビールケース
    put(gbox(0.34, 0.24, 0.26), IM.red, 10.55, 0.18 + i * 0.26, 16.85);
  }

  // 9-10 天井：木梁两道 + 吧台上方的吊提灯 + 吊り短冊菜单
  put(gbox(W - 0.1, 0.07, 0.09), IM.woodDeep, CX, 3.08, FZ + 1.35);
  put(gbox(W - 0.1, 0.07, 0.09), IM.woodDeep, CX, 3.08, CZ + 1.0);
  {
    const lampMats = [lanternMat, IM.redLamp, lanternMat];
    for (let i = 0; i < 3; i++) {
      const lx = 8.25 + i * 0.90;
      put(gbox(0.012, 0.58, 0.012), IM.dark, lx, 2.84, CT_Z);       // 吊绳
      put(cyl(0.115, 0.13, 0.26, 10), lampMats[i], lx, 2.42, CT_Z); // 提灯
    }
  }
  // 短冊挂在 z=16.0：再往前来会挡住俯视时看酒棚的视线（算过，见下）
  put(gbox(2.20, 0.04, 0.04), IM.woodDeep, 9.55, 2.86, 16.00);
  for (let i = 0; i < 7; i++) {
    panel(0.10, 0.42, [IM.paper, IM.red, IM.paper, IM.green, IM.paper, IM.white, IM.paper][i],
      8.53 + i * 0.34, 2.62, 16.00, -1);
  }

  // ---- 10) 外部细节 ----
  // 空调外机
  put(gbox(0.60, 0.42, 0.48), IM.slabDark, CX - W / 2 - 0.35, 0.21, CZ - D / 2 + 0.8);
  // 饮料箱
  for (let i = 0; i < 2; i++) {
    put(gbox(0.28, 0.20, 0.22), IM.cool, CX + W / 2 + 0.25, 0.10, CZ + 0.5 + i * 0.30);
  }
  // 垃圾桶
  put(gbox(0.30, 0.50, 0.30), IM.slabDark, CX + W / 2 + 0.5, 0.25, CZ + D / 2 - 0.6);
  // 店先の酒樽 + 植木鉢（居酒屋的门面记号）
  put(cyl(0.20, 0.20, 0.50, 12), IM.wood, 8.35, 0.31, FZ - 0.42);
  put(cyl(0.206, 0.206, 0.05, 12), IM.woodDeep, 8.35, 0.20, FZ - 0.42);
  put(cyl(0.206, 0.206, 0.05, 12), IM.woodDeep, 8.35, 0.46, FZ - 0.42);
  put(cyl(0.13, 0.16, 0.24, 8), IM.woodDeep, 7.70, 0.12, FZ - 0.30);
  put(cyl(0.20, 0.20, 0.06, 8), IM.green, 7.70, 0.30, FZ - 0.30);

  /* ---- 11) 店内补光 + 暖光外溢 ----
   * 跟便利店同一套做法：店内两盏暖点光（不投阴影，很便宜），把吧台/酒棚
   * 真正照亮，隔着玻璃读得出"店里开着灯"；门口再铺一层加色混合的暖光池，
   * 雨夜里像店里的光淌到湿地面上；招牌前叠一层柔光晕。
   */
  const izWarm = new THREE.PointLight(0xffc98a, 7, 9, 2);
  izWarm.position.set(CX + 0.1, 2.3, CZ + 0.1);
  g.add(izWarm);
  const izWarm2 = new THREE.PointLight(0xffb265, 4, 6, 2);
  izWarm2.position.set(CX - 0.5, 1.9, FZ + 1.1);
  g.add(izWarm2);

  const izPoolMat = emissive('#ffe0b0', {
    map: radial('#ffdcb0', 'rgba(255,190,120,0)'),
    transparent: true, opacity: 0.5, side: THREE.DoubleSide,
  });
  izPoolMat.blending = THREE.AdditiveBlending;
  izPoolMat.depthWrite = false;
  const izPool = new THREE.Mesh(new THREE.PlaneGeometry(5.4, 2.6), izPoolMat);
  izPool.geometry.rotateX(-Math.PI / 2);
  izPool.position.set(CX, 0.014, FZ - 1.5);
  izPool.renderOrder = 2;
  izPool.userData.sceneCollideSkip = true;
  g.add(izPool);

  const izHaloMat = emissive('#ffcf90', {
    map: radial('rgba(255,215,165,0.85)', 'rgba(255,190,140,0)'),
    transparent: true, opacity: 0.45, side: THREE.DoubleSide,
  });
  izHaloMat.blending = THREE.AdditiveBlending;
  izHaloMat.depthWrite = false;
  const izHalo = new THREE.Mesh(new THREE.PlaneGeometry(2.9, 1.1), izHaloMat);
  izHalo.position.set(CX, H + 0.25, FZ - 0.12);
  izHalo.renderOrder = 3;
  izHalo.userData.sceneCollideSkip = true;
  g.add(izHalo);

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 二层书店——近景高模临街铺，就位在居酒屋东侧的巷位（中景楼后撤后，
 * 东区最西一栋原让出的位置正好由它接上，居酒屋往东第一家就是它）。
 * 学园都市（原型多摩市）式学生街一层店面：大玻璃橱窗 + 遮阳篷 + 发光店招，
 * 二层是宿舍感的亮窗。与地铁站/便利店/居酒屋一起构成公寓这条街的高模临街排面。
 */
export function buildStreetBookStore(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'street-bookstore';
  const r = makeRng(43000);

  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const BM = {
    wall: toon('#d8d2c6'),                       // 米灰外墙
    trim: toon('#8f8779'),                       // 檐口/压顶
    dark: toon('#2a2f38'),
    frame: toon('#4a4640'),                      // 橱窗框
    glass: toon('#ffffff', { map: glassTexture(), transparent: true, opacity: 0.5 }),  // 基色白：夜里透出店内暖光板，不发灰
    cool: emissive('#dceaf5'),
    warm: emissive('#ffe4b5'),                   // 店内暖光
  };

  // 位置与体量：面宽 7m、进深 6m、两层 6.6m；北面（-z）临街。
  // (16.2,21)：西缘 x=12.7 与居酒屋菜单牌(12.3)留 0.4m，雨篷北挑 z≈16.95 不碰路灯(13,16)
  const CX = 16.2, CZ = 21.0, W = 7.0, D = 6.0, H1 = 3.2, H2 = 3.4;
  const zN = CZ - D / 2;   // 临街面

  // 结构：主身体 + 楼层缝 + 平屋顶
  put(gbox(W, H1, D), BM.wall, CX, H1 / 2, CZ);
  put(gbox(W, H2, D), BM.wall, CX, H1 + H2 / 2, CZ);
  put(gbox(W + 0.06, 0.10, D + 0.06), BM.trim, CX, H1 + 0.05, CZ);     // 楼层腰线
  put(gbox(W + 0.4, 0.22, D + 0.4), BM.trim, CX, H1 + H2 + 0.11, CZ);  // 屋檐
  put(gbox(W - 0.3, 0.55, D - 0.3), BM.dark, CX, H1 + H2 + 0.28 + 0.27, CZ); // 屋顶压顶围墙

  // 1F 临街：大玻璃橱窗（左 2/3）+ 门（右）。
  // 剖面由外到内：框(0.10) → 玻璃(0.08) → 暖光板(0.04) → 墙面(0)——
  // 暖光板必须嵌在玻璃与墙之间，塞进楼体盒内部会被实心墙整个埋掉。
  put(gbox(W * 0.55, 2.3, 0.06), BM.glass, CX - W * 0.18, 1.55, zN - 0.08);
  for (const dx of [-W * 0.18 - W * 0.275, -W * 0.18 + W * 0.275]) {
    put(gbox(0.09, 2.3, 0.10), BM.frame, CX + dx, 1.55, zN - 0.10);
  }
  put(gbox(0.85, 2.1, 0.08), BM.frame, CX + W * 0.30, 1.35, zN - 0.06);   // 门
  put(gbox(0.70, 0.10, 0.10), BM.cool, CX + W * 0.30, 2.45, zN - 0.04);   // 门楣灯

  // 店内暖光透出（橱窗后面一块亮板，读作书架灯）
  put(gbox(W * 0.5, 1.9, 0.04), BM.warm, CX - W * 0.18, 1.55, zN - 0.04);

  // 遮阳篷（临街，米色，微斜）+ 支架
  put(gbox(W * 0.8, 0.06, 1.1), BM.trim, CX, H1 - 0.18, zN - 0.5).rotateX(0.10);
  for (const dx of [-W * 0.36, W * 0.36]) {
    put(gbox(0.05, 0.55, 0.05), BM.dark, CX + dx, H1 - 0.45, zN - 0.06);
  }

  // 店招（发光面板 + 深色底板，挂二层檐下）
  const signCanvas = (() => {
    const { canvas, ctx } = makeCanvas(192, 48);
    ctx.fillStyle = '#233240';
    ctx.fillRect(0, 0, 192, 48);
    ctx.fillStyle = '#f2ede2';
    ctx.font = 'bold 26px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('書泉堂書店', 96, 34);
    return toTexture(canvas);
  })();
  const signMat = emissive('#ffffff', { map: signCanvas });   // 基色走白，颜色全在贴图（基色会乘进贴图，深底色会压暗字）
  const sign = new THREE.Mesh(new THREE.PlaneGeometry(2.6, 0.65), signMat);
  sign.position.set(CX, H1 + 0.55, zN - 0.10);
  sign.rotation.y = Math.PI;   // 牌面朝 -z（临街），默认 +z 会转向楼内只剩背板
  g.add(sign);
  put(gbox(2.75, 0.75, 0.06), BM.frame, CX, H1 + 0.55, zN - 0.02);

  // 2F 窗：临街三扇（两亮一暗）+ 侧墙各一扇
  for (let i = 0; i < 3; i++) {
    const lit = i !== 1;
    put(gbox(1.15, 1.5, 0.06), lit ? BM.warm : BM.glass,
      CX - W * 0.3 + i * W * 0.3, H1 + 1.7, zN - 0.02);
    put(gbox(1.25, 1.6, 0.05), BM.frame, CX - W * 0.3 + i * W * 0.3, H1 + 1.7, zN - 0.015);
  }
  for (const s of [-1, 1] as number[]) {
    put(gbox(0.05, 1.3, 1.1), r() > 0.5 ? BM.warm : BM.glass,
      CX + s * (W / 2 + 0.01), H1 + 1.7, CZ + s * 0.8);
  }

  // 空调外机 + 落水管
  put(gbox(0.6, 0.4, 0.3), BM.dark, CX - W / 2 - 0.2, H1 + 1.2, CZ + 1.6);
  put(gbox(0.07, H1 + H2, 0.07), BM.dark, CX + W / 2 + 0.06, (H1 + H2) / 2, zN + 0.15);

  // 门口台阶 + 单车（单车放东侧，西侧贴居酒屋菜单牌/饮料箱）
  put(gbox(1.1, 0.12, 0.8), BM.trim, CX + W * 0.30, 0.06, zN - 0.4);
  put(gbox(0.05, 0.62, 0.05), BM.dark, CX + W * 0.40, 0.31, zN - 1.4, 0.3);
  put(gbox(0.05, 0.62, 0.05), BM.dark, CX + W * 0.40 + 0.56, 0.31, zN - 1.35, -0.2);
  put(gbox(0.56, 0.05, 0.05), BM.frame, CX + W * 0.40 + 0.28, 0.62, zN - 1.38, 0);

  // 西侧竖招牌（縦看板，正对居酒屋巷口——从居酒屋往东第一家，招牌必须朝西读得到）
  const sideCanvas = (() => {
    const { canvas, ctx } = makeCanvas(64, 192);
    ctx.fillStyle = '#1d3a4a';
    ctx.fillRect(0, 0, 64, 192);
    ctx.fillStyle = '#f2ede2';
    ctx.font = 'bold 36px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('書', 32, 54);
    ctx.fillText('泉', 32, 112);
    ctx.fillText('堂', 32, 170);
    return toTexture(canvas);
  })();
  const sideSignMat = emissive('#ffffff', { map: sideCanvas });
  const sideSign = new THREE.Mesh(new THREE.PlaneGeometry(0.5, 1.6), sideSignMat);
  sideSign.position.set(CX - W / 2 - 0.06, H1 - 1.15, zN + 0.35);
  sideSign.rotation.y = -Math.PI / 2;   // 朝 -x（居酒屋方向）
  g.add(sideSign);
  put(gbox(0.05, 1.75, 0.62), BM.frame, CX - W / 2 - 0.045, H1 - 1.15, zN + 0.35); // 背板

  // 西墙一层贴报栏（西侧不是全 blank 的山墙）
  put(gbox(0.06, 1.1, 0.8), BM.frame, CX - W / 2 - 0.02, 1.75, CZ + 1.2);
  put(gbox(0.04, 0.9, 0.6), BM.cool, CX - W / 2 - 0.05, 1.75, CZ + 1.2);

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 小型社区公园——公寓西侧的呼吸空间。
 * 2-3 棵树 + 灌木 + 草坪 + 长椅 + 小滑梯 + 路灯 + 低矮围栏 + 花坛。
 * 雨夜：湿润地面 + 路灯照长椅/树，安静治愈感。
 */
export function buildSmallPark(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'small-park';
  const r = makeRng(42000);

  // 局部工具与材质
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const flatDown = (w: number, d: number, mat: THREE.Material, x: number, y: number, z: number) => {
    const geo = new THREE.PlaneGeometry(w, d);
    geo.rotateX(-Math.PI / 2);
    return put(geo, mat, x, y, z);
  };
  const PM = {
    leaf: toon('#7ab86c'),
    leafWarm: toon('#a8c478'),
    leafDeep: toon('#5a9054'),
    wood: toon('#c4a882'),
    woodDeep: toon('#8b7355'),
    slabDark: toon('#3a3f48'),
    dark: toon('#2a2f38'),
    cool: emissive('#dceaf5'),
    warmPale: toon('#fff0d4'),
    hall: emissive('#ffe4b5'),
    floor: toon('#d4c4a8'),
    rmClothLit: toon('#ffffff', { map: curtainTexture() }),
    rmClothAlt: toon('#e8ddd0'),
    frame: toon('#d0d4d8'),
    metal: toon('#8a9099', { finish: 'metal' }),
  };

  // 公园位置：公寓西侧（x=-24~-16, z=2~7），与街道有矮围栏分隔
  const PX0 = -24.5, PX1 = -15.5;   // 公园 x 范围
  const PZ0 = 1.5, PZ1 = 7.5;       // 公园 z 范围
  const PCX = (PX0 + PX1) / 2;      // 公园中心 x
  const PCZ = (PZ0 + PZ1) / 2;      // 公园中心 z
  const PW = PX1 - PX0, PD = PZ1 - PZ0;

  // ---- 1) 地面（草坪）----
  const parkGround = new THREE.Mesh(
    new THREE.PlaneGeometry(PW, PD),
    PM.leaf,
  );
  parkGround.rotation.x = -Math.PI / 2;
  parkGround.position.set(PCX, 0.005, PCZ);
  parkGround.receiveShadow = true;
  g.add(parkGround);

  // ---- 2) 低矮围栏（公园边界）----
  const fenceH = 0.45, fenceSeg = 0.6;
  // 北边（z=PZ1）
  for (let fx = PX0; fx < PX1; fx += fenceSeg) {
    const sw = Math.min(fenceSeg, PX1 - fx);
    if (sw > 0.1) put(gbox(sw, fenceH, 0.04), PM.wood, fx + sw / 2, fenceH / 2, PZ1);
  }
  // 西边（x=PX0）
  for (let fz = PZ0; fz < PZ1; fz += fenceSeg) {
    const sd = Math.min(fenceSeg, PZ1 - fz);
    if (sd > 0.1) put(gbox(0.04, fenceH, sd), PM.wood, PX0, fenceH / 2, fz + sd / 2);
  }
  // 围栏入口（南边中间留 2m 口子）
  const gateW = 2.0, gateCx = PCX;
  for (let fx = PX0; fx < gateCx - gateW / 2; fx += fenceSeg) {
    const sw = Math.min(fenceSeg, gateCx - gateW / 2 - fx);
    if (sw > 0.1) put(gbox(sw, fenceH, 0.04), PM.wood, fx + sw / 2, fenceH / 2, PZ0);
  }
  for (let fx = gateCx + gateW / 2; fx < PX1; fx += fenceSeg) {
    const sw = Math.min(fenceSeg, PX1 - fx);
    if (sw > 0.1) put(gbox(sw, fenceH, 0.04), PM.wood, fx + sw / 2, fenceH / 2, PZ0);
  }

  // ---- 3) 树木（3 棵，复用 buildTree）----
  const treePositions: Array<[number, number]> = [
    [PCX - 4, PCZ - 1.5],
    [PCX + 3, PCZ + 2],
    [PCX - 1, PCZ + 1],
  ];
  for (const [tx, tz] of treePositions) {
    const tree = buildTree(tx, tz, 4.5 + r() * 1.5, r() * 10000 | 0, { trunk: PM.woodDeep, leaf: PM.leaf, leafDeep: PM.leafDeep, leafWarm: PM.leafWarm });
    g.add(tree);
  }

  // ---- 4) 灌木丛（沿围边）----
  const shrubSpots = [
    [PX0 + 1, PZ1 - 0.6], [PX1 - 1.5, PZ1 - 0.8],
    [PX0 + 0.8, PZ0 + 0.8], [PX1 - 1, PZ0 + 0.6],
    [PX0 + 0.6, PCZ], [PX1 - 0.8, PCZ - 1],
  ];
  for (const [sx, sz] of shrubSpots) {
    const shrub = buildShrub(sx, sz, 0.5 + r() * 0.3, 0.45 + r() * 0.25, 0.5 + r() * 0.3, r() * 10000 | 0, { leaf: PM.leaf, leafDeep: PM.leafDeep });
    g.add(shrub);
  }

  // ---- 5) 长椅（2 张）----
  for (const [bx, bz, by] of [[PCX - 2, PCZ + 2.5, 0], [PCX + 3.5, PCZ - 2, 0]] as Array<[number, number, number]>) {
    // 座板
    put(gbox(1.2, 0.06, 0.42), PM.wood, bx, 0.42, bz);
    // 靠背
    put(gbox(1.2, 0.38, 0.04), PM.wood, bx, 0.72, bz - 0.19);
    // 腿 × 4
    for (const dx of [-0.5, 0.5]) for (const dz of [-0.17, 0.17]) {
      put(gbox(0.05, 0.36, 0.05), PM.woodDeep, bx + dx, 0.22, bz + dz);
    }
  }

  // ---- 6) 小型儿童滑梯 ----
  {
    const slideCX = PCX + 1, slideCZ = PZ1 - 1.8;
    // 滑梯支架
    put(gbox(0.08, 1.2, 0.08), PM.metal, slideCX - 0.5, 0.6, slideCZ - 0.6);
    put(gbox(0.08, 0.6, 0.08), PM.metal, slideCX + 0.5, 0.3, slideCZ + 0.3);
    // 滑梯面（倾斜平面用窄 box 堆叠模拟）
    for (let i = 0; i < 6; i++) {
      const t = i / 5;
      const sx = slideCX - 0.5 + t * 1.1;
      const sy = 1.15 - t * 0.85;
      const sz = slideCZ - 0.55 + t * 0.9;
      put(gbox(0.38, 0.04, 0.18 - i * 0.01), PM.warmPale, sx, sy, sz);
    }
    // 小平台
    put(gbox(0.6, 0.06, 0.5), PM.wood, slideCX - 0.5, 1.15, slideCZ - 0.6);
  }

  // ---- 7) 花坛（2 个）----
  for (const [fbx, fbz] of [[PX0 + 2, PZ1 - 1.5], [PX1 - 2.5, PZ0 + 1.5]] as Array<[number, number]>) {
    // 花坛框
    put(gbox(1.0, 0.22, 0.7), PM.slabDark, fbx, 0.11, fbz);
    // 花（彩色小球）
    for (let fi = 0; fi < 6; fi++) {
      const fangle = (fi / 6) * Math.PI * 2;
      const fr = 0.2 + r() * 0.2;
      put(gbox(0.10, 0.12, 0.10),
        [PM.leafWarm, PM.rmClothLit, PM.leaf][fi % 3],
        fbx + Math.cos(fangle) * fr, 0.26, fbz + Math.sin(fangle) * fr * 0.7);
    }
  }

  // ---- 8) 公园路灯（暖黄，比街灯更柔和）----
  {
    const lampX = PCX, lampZ = PZ0 + 1.2;
    put(gbox(0.06, 2.2, 0.06), PM.woodDeep, lampX, 1.1, lampZ);  // 灯杆
    const lampHead = new THREE.Mesh(
      new THREE.SphereGeometry(0.14, 8, 8),
      emissive('#ffdd88', {}),
    );
    lampHead.position.set(lampX, 2.35, lampZ);
    g.add(lampHead);
    // 地面光斑
    flatDown(0.8, 0.6, PM.hall, lampX, 0.01, lampZ + 0.3);
  }

  // ---- 9) 自动饮水机 ----
  put(gbox(0.30, 0.95, 0.28), PM.metal, PCX - 5, 0.48, PZ0 + 0.8);
  // 饮水机按钮/出水口暗示
  put(gbox(0.06, 0.06, 0.03), PM.cool, PCX - 5, 0.75, PZ0 + 0.68);

  // ---- 10) 垃圾桶 ----
  put(gbox(0.32, 0.48, 0.32), PM.slabDark, PCX + 5, 0.24, PZ1 - 0.8);

  g.userData.sceneCollideSkip = true;
  return g;
}

/* ============================================================================
 * 中景城市肌理：商住混合街区（4-6 层）
 * ============================================================================ */

/**
 * 中景商住混合建筑群——5~6 栋 4~6 层商住楼，填补公寓与远景之间的中景层次。
 * 一楼商铺（咖啡/餐馆/药店/理发/便利店），二楼以上住宅。
 * 高密度小间距，形成日式住宅区 × 上海里弄密度的城市肌理。
 * 控制 draw call：全楼共享材质桶，合批后 ≤25 call。
 */
export function buildMidRiseBlock(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'mid-rise-block';
  const r = makeRng(52000);

  // ---- 局部工具与共享材质桶 ----
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  // 扁平盒子
  const flat = (w: number, h: number, d: number, mat: THREE.Material) => new THREE.Mesh(
    gbox(w, h, d), mat,
  );
  const MM = {
    wall:    toon('#c8c4bc'),   // 主墙：暖灰白
    wall2:   toon('#b8b0a8'),   // 次墙：稍深灰
    dark:    toon('#4a4a4a'),   // 深色构件
    accent:  toon('#9ebfcf'),   // 点缀：淡蓝灰（一栋用）
    brick:   toon('#c49178'),   // 砖红（一栋用）
    glass:   toon('#2a3444', { transparent: true, opacity: 0.6 }),
    glassLit: emissive('#ffddaa'), // 亮窗
    roof:    toon('#5a5858'),   // 屋顶深灰
    awning:  toon('#e8dfd3'),  // 遮阳篷米色
    shopFront: toon('#3d3530'),// 店面深色框
  };

  // ---- 建筑定义数组 ----
  interface BldgDef {
    cx: number;      // 中心 X
    cz: number;      // 中心 Z
    w: number;       // 宽度 X
    d: number;       // 深度 Z
    floors: number;  // 楼层数
    floorH: number;  // 层高（默认 3.0）
    matWall: THREE.Material; // 墙材质（可覆盖）
    shopName: string; // 一楼店名
    hasAwning: boolean;
    accentColor?: string; // 特殊点缀色
  }

  const bldgs: BldgDef[] = [
    // 两条硬约束：
    //  A. 203 阳台（约 x0,z5.5）看向地铁口是一条西南向视线走廊，两侧各留 6m 净空，
    //     所以商住楼只落东区（x>10）和南区（z≥24，在地铁口身后）。
    //  B. 居酒屋占 x7.5~11.7 / z14~17.8，东区最西一栋必须让开，中间留巷子。
    // 层次约定（学园都市/多模参考）：公寓这条街（z≤30）是高模街区（便利店/居酒屋/
    // 地铁口/书店/公交站），低模商住楼整体退到 z≥29 的中景带，临街近景不留低模。

    // ① 东区 A 栋：6F 最宽，小餐馆（西缘 x=14，与居酒屋隔 2.3m 巷）
    { cx: 18, cz: 38, w: 8, d: 10, floors: 6, floorH: 3.0,
      matWall: MM.brick, shopName: '食堂', hasAwning: true },
    // ② 东区 B 栋：5F，便利店（① 东侧，隔 2.25m 小巷）
    { cx: 27, cz: 36, w: 5.5, d: 8, floors: 5, floorH: 3.0,
      matWall: MM.wall, shopName: 'STORE', hasAwning: true },
    // ③ 东区 C 栋：5F，小型办公室（最东端，隔 2m 小巷）
    { cx: 35, cz: 33, w: 6.5, d: 7.5, floors: 5, floorH: 2.95,
      matWall: MM.wall2, shopName: 'OFFICE', hasAwning: false },
    // ④ 南区 A 栋：4F 小型，理发店（南区最东，与东区隔路口）
    { cx: 8, cz: 39, w: 5, d: 7, floors: 4, floorH: 2.85,
      matWall: MM.accent, shopName: 'BARBER', hasAwning: true },
    // ⑤ 南区 B 栋：5F 窄长，咖啡店
    { cx: -14, cz: 38, w: 5.5, d: 9, floors: 5, floorH: 3.0,
      matWall: MM.wall, shopName: 'COFFEE', hasAwning: true },
    // ⑥ 南区 C 栋：4F 方正，药店（南区最西，再往西就空着）
    { cx: -24, cz: 37, w: 6, d: 8, floors: 4, floorH: 3.0,
      matWall: MM.wall2, shopName: '薬局', hasAwning: false },
  ];

  // ---- 逐栋建造 ----
  for (let bi = 0; bi < bldgs.length; bi++) {
    const b = bldgs[bi];
    const totalH = b.floors * b.floorH;
    const baseY = 0;

    // === 主体墙体（整栋一个 box，后面挖窗） ===
    const mainBody = new THREE.Mesh(
      gbox(b.w, totalH, b.d),
      b.matWall,
    );
    mainBody.position.set(b.cx, baseY + totalH / 2, b.cz);
    g.add(mainBody);

    // === 屋顶（稍微出檐） ===
    put(gbox(b.w + 0.3, 0.25, b.d + 0.3), MM.roof, b.cx, totalH + 0.125, b.cz);

    // === 一楼店面处理 ===
    // 店面朝向：中景楼群在主街南侧，街和 203 视点都在它们的北边——
    // 店面玻璃/门/雨棚/店招/满排窗必须朝 -z（北）才读得出"临街商铺"，
    // 朝 +z 就是全街区背对着观众。F=-1 为临街面。
    const shopH = b.floorH;
    const shopY = baseY + shopH / 2;
    const F = -1;

    // 店面大玻璃（临街面 + 背面各一扇）
    const frontGlassW = b.w * 0.65;
    const frontGlassH = shopH * 0.55;
    put(gbox(frontGlassW, frontGlassH, 0.05), MM.glass, b.cx, shopY + 0.15, b.cz + F * (b.d / 2 + 0.01));
    put(gbox(frontGlassW * 0.7, frontGlassH * 0.85, 0.05), MM.glass, b.cx, shopY + 0.1, b.cz - F * (b.d / 2 + 0.01));

    // 店面门（偏右）
    const doorW = 0.85;
    const doorH = shopH * 0.75;
    put(gbox(doorW, doorH, 0.08), MM.dark, b.cx + b.w * 0.25, shopY + doorH / 2 - 0.15, b.cz + F * (b.d / 2 + 0.02));

    // 遮阳篷
    if (b.hasAwning) {
      put(gbox(b.w + 0.4, 0.06, 1.2), MM.awning, b.cx, shopH + 0.03, b.cz + F * (b.d / 2 + 0.55));
      // 篷支架
      put(gbox(0.06, shopH * 0.35, 0.06), MM.dark, b.cx - b.w * 0.35, shopH + 0.2, b.cz + F * (b.d / 2 + 0.1));
      put(gbox(0.06, shopH * 0.35, 0.06), MM.dark, b.cx + b.w * 0.35, shopH + 0.2, b.cz + F * (b.d / 2 + 0.1));
    }

    // 店招（小牌子）
    const signW = Math.min(b.w * 0.7, 2.5);
    put(gbox(signW, 0.45, 0.08), MM.shopFront, b.cx, totalH * 0.55, b.cz + F * (b.d / 2 + 0.08));

    // === 二楼以上窗户 ===
    const winCols = Math.max(2, Math.floor(b.w / 2.2));  // 每排窗数
    const winRows = b.floors - 1;                         // 二楼起
    const winW = 0.85;
    const winH = 1.3;
    const winSpacingX = b.w / (winCols + 0.5);
    const winStartX = b.cx - b.w / 2 + winSpacingX * 0.7;

    for (let row = 0; row < winRows; row++) {
      const floorY = baseY + b.floorH + row * b.floorH + b.floorH / 2;
      for (let col = 0; col < winCols; col++) {
        const wx = winStartX + col * winSpacingX;
        // 随机决定这扇窗是否亮着（基于建筑索引+行列的伪随机）
        const seedVal = r() ?? 0.5;
        const isLit = seedVal > 0.55;
        const winMat = isLit ? MM.glassLit : MM.glass;

        // 临街面窗（满排）
        put(gbox(winW, winH, 0.05), winMat, wx, floorY, b.cz + F * (b.d / 2 + 0.01));
        // 背面窗（少一些）
        if (col % 2 === 0) {
          put(gbox(winW * 0.8, winH * 0.85, 0.05), winMat, wx, floorY, b.cz - F * (b.d / 2 + 0.01));
        }
      }
      // 侧面窗（仅宽建筑）
      if (b.w > 6) {
        const sideWinX = (bi % 2 === 0) ? b.cx + b.w / 2 + 0.01 : b.cx - b.w / 2 - 0.01;
        if ((row + bi) % 3 !== 0) {  // 不是每层都画侧窗
          const sideLit = (r() ?? 0.5) > 0.6;
          put(gbox(0.05, winH, winW * 0.75), sideLit ? MM.glassLit : MM.glass,
              sideWinX, floorY, b.cz);
        }
      }
    }

    // === 空调外机（随机出现在某些楼层外墙上） ===
    for (let fl = 1; fl < b.floors; fl++) {
      if ((r() ?? 0.5) > 0.6) {
        const acy = baseY + (fl + 0.5) * b.floorH;
        const acSide = (bi + fl) % 2 === 0 ? 1 : -1;
        const acx = b.cx + acSide * (b.w / 2 + 0.18);
        put(gbox(0.55, 0.35, 0.4), MM.dark, acx, acy, b.cz + (r() ?? 0.5) * b.d * 0.3 - b.d * 0.15);
      }
    }

    // === 阳台栏杆（部分楼层） ===
    for (let fl = 1; fl < b.floors; fl++) {
      if ((r() ?? 0.5) > 0.5 && b.floors >= 5) {
        const by = baseY + fl * b.floorH;
        // 临街面栏杆段
        const railLen = b.w * 0.5;
        const railX = b.cx + (bi % 2 === 0 ? -1 : 1) * b.w * 0.15;
        put(gbox(railLen, 0.9, 0.06), MM.dark, railX, by + 0.45, b.cz + F * (b.d / 2 + 0.08));
        // 栏杆竖条
        for (let ri = 0; ri < 3; ri++) {
          const rx = railX + (ri - 1) * (railLen / 3.5);
          put(gbox(0.04, 0.9, 0.04), MM.dark, rx, by + 0.45, b.cz + F * (b.d / 2 + 0.08));
        }
      }
    }
  }

  // ---- 建筑之间的小巷元素 ----
  // 路灯（几盏散布在建筑间隙）
  // 灯位贴街沿与巷口；(-25,21) 那盏是地铁口的引导灯，排在视线走廊南侧让开净空
  const lampPos: Array<[number, number]> = [
    [13, 16], [20, 20], [28, 21], [2, 22], [-19, 24], [-25, 21],
  ];
  for (const [lx, lz] of lampPos) {
    // 灯杆
    put(gbox(0.07, 3.5, 0.07), MM.dark, lx, 1.75, lz);
    // 灯头
    const lampHead = new THREE.Mesh(
      new THREE.SphereGeometry(0.18, 6, 6),
      emissive('#ffeecc', {}),
    );
    lampHead.position.set(lx, 3.55, lz);
    g.add(lampHead);
  }

  // 电线杆 / 配电箱（增加城市细节）
  put(gbox(0.25, 2.2, 0.25), MM.dark, -10.5, 1.1, 27);  // 电线杆
  put(gbox(0.6, 0.7, 0.4), MM.dark, -10.5, 0.35, 27.2);  // 配电箱

  g.userData.sceneCollideSkip = true;
  return g;
}

/* ============================================================================
 * 远景城市骨架：高架道路 + 商业综合体
 * ========================================================================== */

/**
 * 远景高架桥——一条从城市深处斜穿而过的城市快速路。
 * 极低模：桥面 + 桥墩 + 护栏 + 路灯 + 车灯光。
 * 桥体沿直线两端延伸出雾距（fog far=115）之外，两端隐入雾色；
 * 灯光只布在可见段，桥灯是 emissive 仍能透出雾，是"远处那条光带"。
 */
// 高架线参数（模块级）：远处电杆排沿这条线平行走位，读作"电线随高架铺设"
const VD_X0 = -126, VD_Z0 = 47.2, VD_X1 = 106.3, VD_Z1 = 82.6;

export function buildViaduct(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'viaduct';
  const r = makeRng(50000);

  // 局部工具与材质
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const VM = {
    slabDark: toon('#3a3f48'),
    wallBase: toon('#6b7481'),
    dark: toon('#2a2f38'),
    cool: emissive('#dceaf5'),
  };

  // 高架走向：从左前（西北）向右后（东南），斜穿画面。
  // 两端沿同一直线延伸到雾距（fog far=115）之外——距原点约 135m，
  // 桥体两端隐入雾色，读作"快速路一头扎进城市深处"，而不是断在半空的半截桥。
  // 中段 z≈60~74 仍须落在环带楼（z≤57）之后，否则桥面 y=14 会扎进 15~22m 高的楼体。
  const V_Z0 = VD_Z0, V_Z1 = VD_Z1;
  const V_X0 = VD_X0, V_X1 = VD_X1;
  const V_Y = 14;           // 高架高度（中远景层次）
  const V_W = 10;            // 桥面宽

  // ---- 1) 桥面（长条 box）----
  const vLen = Math.sqrt((V_X1 - V_X0) ** 2 + (V_Z1 - V_Z0) ** 2);
  const vAngle = Math.atan2(V_Z1 - V_Z0, V_X1 - V_X0);
  const deck = new THREE.Mesh(
    gbox(V_W, 0.4, vLen),
    VM.slabDark,
  );
  deck.position.set((V_X0 + V_X1) / 2, V_Y, (V_Z0 + V_Z1) / 2);
  deck.rotation.y = -vAngle;
  g.add(deck);

  // ---- 2) 桥墩（沿线每 20m 一根，H 型双柱）----
  const vdx = (V_X1 - V_X0) / vLen, vdz = (V_Z1 - V_Z0) / vLen;   // 沿线单位向量
  for (let d = 0; d <= vLen; d += 20) {
    const px = V_X0 + vdx * d;
    const pz = V_Z0 + vdz * d;
    // 双柱
    put(gbox(1.2, V_Y - 1, 0.8), VM.wallBase, px - 2.5, (V_Y - 1) / 2, pz);
    put(gbox(1.2, V_Y - 1, 0.8), VM.wallBase, px + 2.5, (V_Y - 1) / 2, pz);
    // 横梁
    put(gbox(6.5, 0.8, 0.6), VM.wallBase, px, V_Y - 0.6, pz);
  }

  // ---- 3) 护栏（两侧矮墙）----
  for (const side of [-1, 1] as number[]) {
    const rail = new THREE.Mesh(
      gbox(0.15, 0.9, vLen),
      VM.dark,
    );
    rail.position.set((V_X0 + V_X1) / 2 + side * (V_W / 2 - 0.2), V_Y + 0.45, (V_Z0 + V_Z1) / 2);
    rail.rotation.y = -vAngle;
    g.add(rail);
    // 护栏反光条（每 3m 一段）
    for (let i = 0; i < Math.floor(vLen / 3); i++) {
      const rt = i / Math.floor(vLen / 3);
      const rx = V_X0 + rt * (V_X1 - V_X0) + side * (V_W / 2 - 0.2);
      const rz = V_Z0 + rt * (V_Z1 - V_Z0);
      put(gbox(0.08, 0.12, 0.4), VM.cool, rx, V_Y + 0.35, rz);
    }
  }

  // ---- 4) 路灯（按沿线距离布点，只落在可见段，雾外不浪费）----
  for (const d of [30, 75, 120, 165]) {
    const lx = V_X0 + vdx * d;
    const lz = V_Z0 + vdz * d;
    // 灯杆（短，贴护栏）
    put(gbox(0.06, 1.8, 0.06), VM.dark, lx + V_W / 2 - 0.5, V_Y + 0.9, lz);
    // 灯头（暖白）
    const vlamp = new THREE.Mesh(
      new THREE.SphereGeometry(0.2, 6, 6),
      emissive('#ffeecc', {}),
    );
    vlamp.position.set(lx + V_W / 2 - 0.5, V_Y + 1.85, lz);
    g.add(vlamp);
  }

  // ---- 5) 车灯光（几团静态暖光点，按"沿线距离 + 横向车道偏移"落在桥面上）----
  const carLights: Array<[number, number]> = [
    [45, 3.2], [80, -3.2], [115, 3.2], [150, -3.2], [185, 0],
  ];
  for (const [d, off] of carLights) {
    const clx = V_X0 + vdx * d - vdz * off;
    const clz = V_Z0 + vdz * d + vdx * off;
    const cly = V_Y + 0.25;
    // 前灯（白/微黄）
    const headLight = new THREE.Mesh(
      new THREE.SphereGeometry(0.12, 6, 6),
      emissive('#ffffee', {}),
    );
    headLight.position.set(clx, cly, clz);
    g.add(headLight);
    // 尾灯（红）
    const tailLight = new THREE.Mesh(
      new THREE.SphereGeometry(0.08, 6, 6),
      emissive('#ff3322', {}),
    );
    tailLight.position.set(clx - 2.5, cly, clz + 1.5);
    g.add(tailLight);
  }

  // （原东端匝道已随两端延长进雾而移除——桥体两端都隐入雾色，
  //   匝道落在 |p|≈127 的全雾区里永远不可见，纯浪费几何。）

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 远景商业综合体——8-12 层玻璃幕墙商场。
 * 极低模：大轮廓 + 玻璃幕墙 + 大型发光招牌 + 屋顶设备。
 * 在雾气中呈现为远处一个"亮起来的城市节点"。
 */
export function buildShoppingMall(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'shopping-mall';
  const r = makeRng(51000);

  // 局部工具与材质
  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };
  const MM = {
    dark: toon('#2a2f38'),
  };

  // 商场位置：远景右侧（x=50~70, z=100~130）
  // 东侧中远景（x=55, z=38）：从203阳台往东南看，越过中景楼东侧边缘可见
  // 不被中景楼群(z=18~33)遮挡，在雾内(far=115)
  const MX = 55, MZ = 38;
  const MW = 28, MD = 18, MH = 42;   // 主体尺寸

  // ---- 1) 主体量（玻璃幕墙感：深色底 + 窗格线）----
  const body = new THREE.Mesh(
    gbox(MW, MH, MD),
    toon('#1a2030', { unique: true }),
  );
  body.position.set(MX, MH / 2, MZ);
  g.add(body);

  // ---- 2) 窗格线（横竖线条，模拟幕墙分格）----
  const gridMat = toon('#2a3545', { unique: true });
  // 水平线（每 3.5m 一层）
  for (let floor = 1; floor < MH / 3.5; floor++) {
    const fy = floor * 3.5;
    const hLine = new THREE.Mesh(gbox(MW + 0.02, 0.08, MD + 0.02), gridMat);
    hLine.position.set(MX, fy, MZ);
    g.add(hLine);
  }
  // 垂直线（每 4m 一列）
  for (let col = -3; col <= 3; col++) {
    const vx = MX + col * 4;
    if (vx > MX - MW/2 && vx < MX + MW/2) {
      const vLine = new THREE.Mesh(gbox(0.06, MH, MD + 0.02), gridMat);
      vLine.position.set(vx, MH / 2, MZ);
      g.add(vLine);
    }
  }

  // ---- 3) 大型发光招牌（立面中部）----
  const mallSignCanvas = (() => {
    const { canvas, ctx } = makeCanvas(256, 96);
    ctx.fillStyle = '#0a1628';
    ctx.fillRect(0, 0, 256, 96);
    // 发光字
    ctx.fillStyle = '#44aaff';
    ctx.font = 'bold 36px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('CITY PLAZA', 128, 55);
    ctx.font = '16px sans-serif';
    ctx.fillStyle = '#88ccff';
    ctx.fillText('Shopping & Dining', 128, 78);
    return toTexture(canvas);
  })();
  const mallSignMat = emissive('#4488ff', { map: mallSignCanvas });
  const mallSign = new THREE.Mesh(new THREE.PlaneGeometry(8, 3), mallSignMat);
  mallSign.position.set(MX, MH * 0.6, MZ - MD / 2 - 0.05);
  mallSign.rotation.y = Math.PI;
  g.add(mallSign);

  // ---- 4) 顶部发光结构 ----
  // 屋顶设备间轮廓
  put(gbox(8, 4, 6), MM.dark, MX + 5, MH + 2, MZ - 2);
  put(gbox(6, 3, 4), MM.dark, MX - 6, MH + 1.5, MZ + 3);
  // 屋顶发光标识（红色航空障碍灯）
  const obsLight = new THREE.Mesh(
    new THREE.SphereGeometry(0.4, 8, 8),
    emissive('#ff3333', {}),
  );
  obsLight.position.set(MX, MH + 4.5, MZ);
  g.add(obsLight);

  // ---- 5) 入口灯光 ----
  // 底部入口区域亮起来
  const entranceGlow = new THREE.Mesh(
    new THREE.PlaneGeometry(10, 6),
    emissive('#ffddaa', { opacity: 0.4, transparent: true }),
  );
  entranceGlow.position.set(MX, 3, MZ - MD / 2 - 0.1);
  entranceGlow.rotation.y = Math.PI;
  g.add(entranceGlow);

  // ---- 6) 随机窗光（少量亮着的窗户）----
  const winMat = emissive('#ffffcc', {});
  for (let i = 0; i < 20; i++) {
    if (r() > 0.4) continue;  // 只亮 ~40%
    const wx = MX + (r() - 0.5) * (MW - 2);
    const wy = 2 + r() * (MH - 4);
    const wz = MZ + (r() - 0.5) * (MD - 0.5);
    const win = new THREE.Mesh(new THREE.PlaneGeometry(1.2, 1.8), winMat);
    win.position.set(wx, wy, wz);
    // 随机朝向四个立面之一
    const face = Math.floor(r() * 4);
    if (face === 0) { win.rotation.y = Math.PI; win.position.z = MZ - MD / 2 - 0.01; }
    else if (face === 1) { win.rotation.y = 0; win.position.z = MZ + MD / 2 + 0.01; }
    else if (face === 2) { win.rotation.y = Math.PI / 2; win.position.x = MX + MW / 2 + 0.01; }
    else { win.rotation.y = -Math.PI / 2; win.position.x = MX - MW / 2 - 0.01; }
    g.add(win);
  }

  // ---- 7) 周围低层商业裙房 ----
  const podium = new THREE.Mesh(
    gbox(MW + 8, 8, MD + 6),
    toon('#252e3d', { unique: true }),
  );
  podium.position.set(MX, 4, MZ);
  g.add(podium);

  g.userData.sceneCollideSkip = true;
  return g;
}

/* ============================================================================
 * 城市基础设施：电线杆 + 架空电线 + 街道设施
 * ========================================================================== */

/**
 * 电线杆与架空电线——日式住宅区 × 上海里弄密度最标志性的一笔。
 *
 * 杆身/横担/绝缘子/挂箱变压器全部共用一个水泥灰桶，合批后整片杆子只占 1 个
 * draw call；所有下垂跨距（纵向沿街、横向跨街、接户支线）汇成**一条**
 * LineSegments（1 个 draw call），不吃不透明合批、也不新增材质。
 * 控制后本函数仅 2 个 draw call。
 */
export function buildUtilityPoles(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'utility-poles';

  const UP = {
    wood: toon('#5b5650'),   // 水泥杆/横担/绝缘子/变压器共一桶
  };

  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };

  // 单根杆：杆身 + 双横担 + 绝缘子 + （可选）挂箱式变压器
  const pole = (x: number, z: number, transformer = false) => {
    put(gbox(0.20, 7.0, 0.20), UP.wood, x, 3.5, z);
    for (const cy of [6.4, 6.0]) put(gbox(1.4, 0.10, 0.10), UP.wood, x, cy, z);
    for (const dx of [-0.6, 0.6]) put(gbox(0.08, 0.16, 0.08), UP.wood, x + dx, 6.3, z);
    if (transformer) {
      put(gbox(0.70, 0.08, 0.30), UP.wood, x, 4.1, z);
      put(gbox(0.55, 0.95, 0.55), UP.wood, x, 4.6, z);
    }
  };

  // 杆位：近景街道只留西段两根（地铁口 / 停车场一带）。
  // 远处列改两支：①西侧空地一条南北向短列（x=-33，地铁口/停车场/小公园以西）；
  // ②沿高架平行的斜列——电线随高架铺（多摩式动线），立在中景楼群与环带楼身后。
  // 中景低模楼已退到 z 33~44，原 z=39 的东西向直列会穿楼，故取消。
  const ROW_A_Z = 13.3;              // 商铺侧人行道（近景仅存 2 根）
  const FAR_WEST_X = -33.0;          // 远处南北列
  const A_X = [-26, -16];            // 近景：地铁口西侧 + 停车场东侧各一根
  const F2_Z = [10, 18, 26, 34];                          // 远处南北列
  for (const x of A_X) pole(x, ROW_A_Z, x === -26);
  for (const z of F2_Z) pole(FAR_WEST_X, z, z === 18);

  // 高架平行斜列：沿线参数取可见段（|p|≲105），横向偏 8m 落在高架南侧
  const vdxAll = VD_X1 - VD_X0, vdzAll = VD_Z1 - VD_Z0;
  const vLenAll = Math.hypot(vdxAll, vdzAll);
  const udx = vdxAll / vLenAll, udz = vdzAll / vLenAll;
  const pdx = -udz, pdz = udx;             // 南侧法向（+z 分量为正）
  const F1_D = [70, 110, 150, 190];
  const F1: Array<[number, number]> = F1_D.map(
    (d) => [VD_X0 + udx * d + pdx * 8, VD_Z0 + udz * d + pdz * 8] as [number, number],
  );
  for (let i = 0; i < F1.length; i++) pole(F1[i][0], F1[i][1], i === 1);

  // 电线：所有下垂跨距合进一条 LineSegments（1 draw call，不吃不透明合批）
  const PH = 6.3;
  const wirePts: number[] = [];
  const sagOf = (ax: number, az: number, bx: number, bz: number) => {
    const d = Math.hypot(bx - ax, bz - az);
    return Math.min(0.6, 0.18 + d * 0.05);
  };
  const span = (ax: number, az: number, bx: number, bz: number, ya = PH, yb = PH) => {
    const s = sagOf(ax, az, bx, bz);
    const N = 10;
    let prev: [number, number, number] | null = null;
    for (let i = 0; i <= N; i++) {
      const t = i / N;
      const x = ax + (bx - ax) * t;
      const z = az + (bz - az) * t;
      const y = ya + (yb - ya) * t - s * 4 * t * (1 - t);
      const cur: [number, number, number] = [x, y, z];
      if (prev) wirePts.push(prev[0], prev[1], prev[2], cur[0], cur[1], cur[2]);
      prev = cur;
    }
  };
  // 近景：沿街一段 + 接户支线（地铁口 / 停车场高杆灯）+ 西向馈线（跨地铁口雨棚上空进远处列）
  span(A_X[0], ROW_A_Z, A_X[1], ROW_A_Z);
  span(-26, ROW_A_Z, -24, 15.5, PH, 3.5);
  span(-16, ROW_A_Z, -17.7, 16.2, PH, 4.6);
  span(-16, ROW_A_Z, FAR_WEST_X, F2_Z[0]);
  // 高架平行斜列：双线（主干 + 附属线）
  for (let i = 0; i < F1.length - 1; i++) {
    span(F1[i][0], F1[i][1], F1[i + 1][0], F1[i + 1][1]);
    span(F1[i][0], F1[i][1], F1[i + 1][0], F1[i + 1][1], PH - 0.45, PH - 0.45);
  }
  // 远处南北列：双线（主干 + 附属线），雾中读出"一排杆一束线"
  for (let i = 0; i < F2_Z.length - 1; i++) {
    span(FAR_WEST_X, F2_Z[i], FAR_WEST_X, F2_Z[i + 1]);
    span(FAR_WEST_X, F2_Z[i], FAR_WEST_X, F2_Z[i + 1], PH - 0.45, PH - 0.45);
  }

  const wireGeo = new THREE.BufferGeometry();
  wireGeo.setAttribute('position', new THREE.Float32BufferAttribute(wirePts, 3));
  const wires = new THREE.LineSegments(wireGeo, new THREE.LineBasicMaterial({ color: 0x14181f, fog: true }));
  wires.name = 'power-lines';
  g.add(wires);

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 街道设施：自动售货机 / 快递柜 / 路牌。
 *
 * 机柜、灯杆、招牌柱共用一个金属桶；所有发光显示面（售货机商品窗、取货口、
 * 快递柜屏、路牌牌面）共用一个冷白发光桶——合批后仅占 2 个 draw call。
 * 全部 sceneCollideSkip（纯视觉，不参与 FPS 碰撞）。
 */
export function buildStreetFurniture(): THREE.Group {
  const g = new THREE.Group();
  g.name = 'street-furniture';

  const SF = {
    metal: toon('#7c828b', { finish: 'metal' }),
    panel: emissive('#cfe6ff'),     // 发光显示面（无贴图，远处读作亮起的招牌/屏幕）
  };

  const put = (geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number, ry = 0) => {
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    if (ry) m.rotation.y = ry;
    g.add(m);
    return m;
  };

  // 自动售货机：柜体 + 商品展示发光面 + 取货口 + 操作屏（正面朝 -z 对街道/相机）
  const vending = (x: number, z: number) => {
    put(gbox(1.1, 2.0, 0.7), SF.metal, x, 1.0, z);
    put(gbox(0.92, 1.35, 0.05), SF.panel, x, 1.35, z - 0.36);
    put(gbox(0.5, 0.35, 0.05), SF.panel, x + 0.25, 0.45, z - 0.36);
    put(gbox(0.18, 0.5, 0.05), SF.panel, x - 0.35, 0.55, z - 0.36);
  };
  vending(-13.5, 13.0);
  vending(11.5, 13.0);

  // 快递柜：多格机柜 + 操作屏
  const parcel = (x: number, z: number) => {
    put(gbox(1.7, 2.0, 0.6), SF.metal, x, 1.0, z);
    for (const dy of [-0.6, -0.2, 0.2, 0.6]) put(gbox(1.5, 0.04, 0.04), SF.metal, x, 1.0 + dy, z - 0.30);
    put(gbox(0.35, 0.5, 0.05), SF.panel, x - 0.5, 1.2, z - 0.31);
  };
  parcel(-18.0, 14.5);
  parcel(16.5, 14.0);

  // 路牌 / 指引牌：柱 + 发光牌面
  const signpost = (x: number, z: number, w: number, h: number, ry = 0) => {
    put(gbox(0.08, 2.4, 0.08), SF.metal, x, 1.2, z, ry);
    put(gbox(w, h, 0.05), SF.panel, x, 2.1, z - 0.06, ry);
  };
  signpost(0, 13.0, 1.0, 0.55);
  signpost(10, 13.0, 0.7, 0.5);

  // 公交站牌（学园都市味：站名以学区设施命名）——柱 + 灯箱牌面 + 座位条凳
  const busCanvas = (() => {
    const { canvas, ctx } = makeCanvas(160, 96);
    ctx.fillStyle = '#eef2f6';
    ctx.fillRect(0, 0, 160, 96);
    ctx.fillStyle = '#1d6e4f';
    ctx.fillRect(0, 0, 160, 22);
    ctx.fillStyle = '#ffffff';
    ctx.font = 'bold 14px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('第七学区 行', 80, 16);
    ctx.fillStyle = '#243040';
    ctx.font = 'bold 17px sans-serif';
    ctx.fillText('学生寮前', 80, 52);
    ctx.font = '12px sans-serif';
    ctx.fillStyle = '#5a6472';
    ctx.fillText('／ 柵川中學', 80, 76);
    return toTexture(canvas);
  })();
  const busMat = emissive('#eef2f6', { map: busCanvas });
  put(gbox(0.07, 2.5, 0.07), SF.metal, -6.5, 1.25, 13.0);                 // 立柱
  const busSign = new THREE.Mesh(new THREE.PlaneGeometry(0.85, 0.5), busMat);
  busSign.position.set(-6.5, 2.1, 12.96);
  busSign.rotation.y = Math.PI;   // 牌面朝 -z 对人行道来向，默认 +z 只见背板
  g.add(busSign);
  put(gbox(0.9, 0.55, 0.04), SF.metal, -6.5, 2.1, 13.0);                  // 牌背板
  put(gbox(1.4, 0.06, 0.45), SF.metal, -6.5, 0.45, 13.35);                // 条凳

  g.userData.sceneCollideSkip = true;
  return g;
}

/* ============================================================================
 * 湿地涟漪 + 屋檐滴水（动效层，单独返回，由 RoomScene 加进未冻结的组）
 * ========================================================================== */

/**
 * 街道湿地面上的雨打涟漪。
 *
 * 参数固定：街心湿地面（center [0.3, 9.5]、半边长 5.2）覆盖路灯倒影、
 * 公寓入口暖光、便利店门口那片湿地。返回的对象**必须加进未冻结的组**——
 * 涟漪每帧改 scale，freezeStatic 把 matrixAutoUpdate 关掉后会定格不动。
 */
export function buildStreetscapeRipples(): { object: THREE.Group; update: (t: number) => void } {
  // 涟漪只落在局部浅水（buildStreetPuddles）那几处水面上，面积收小、数量减半、
  // 颜色压淡——不再是满地乱冒的同心环，而是"这几汪水里还在落雨"。
  return buildPuddleRipples({ area: 2.2, center: [0.0, 9.5], count: 7, y: 0.016, color: '#8fa4be', seed: 98831 });
}

/**
 * 公寓每层每户阳台外缘垂下的一排屋檐滴水（203 自家阳台除外——那段归房间
 * 自己的 shell，由 RainScene 的 wetPools / 阳台湿地处理）。
 *
 * 四层楼（2F/3F/4F 阳台）+ 两处新增檐口：北面公共外廊的外缘（整条），
 * 以及共用外楼梯的各层平台（步行廊 / 折返平台 / 4F 西平台）——雨夜里
 * 楼梯边挂水是"这楼梯真的在淋雨"的最便宜证据。
 *
 * 阳台板底面 = 楼层标高 F - 0.14，外缘在 z = ZB(6.3)。每 ~1.4m 一滴，
 * 冷蓝低透明度，轻微伸缩起伏像欲滴未滴。
 */
export function buildApartmentEaveDrips(): { object: THREE.Group; update: (t: number) => void } {
  const ZB = 6.3;
  const F2 = 3.4, LVL = 2.8;
  const FLOORS = [F2, F2 + LVL, F2 + LVL * 2]; // 2F 3.4 / 3F 6.2 / 4F 9.0（四层）
  const UNITS: Array<[number, number]> = [
    [-31, -18.6], [-18.6, -6.2], [-6.2, 6.2], [6.2, 18.6], [18.6, 31],
  ];
  const edges: DripEdge[] = [];
  FLOORS.forEach((F, fi) => {
    UNITS.forEach(([ux0, ux1], ui) => {
      if (fi === 0 && ui === 2) return; // 203 自家阳台，跳过
      edges.push({ x0: ux0 + 0.08, x1: ux1 - 0.08, z: ZB, y: F - 0.14 });
    });
    // 公共外廊外缘（z=-7.19，腰壁之外）
    edges.push({ x0: -30.9, x1: STRS.lx0 - 0.1, z: -7.21, y: F - 0.14 });
  });
  // 共用外楼梯：逐层西平台南北檐口 + (非顶层) 步行廊/折返平台檐口
  FLOORS.forEach((F, fi) => {
    edges.push({ x0: STRS.lx0 + 0.06, x1: STRS.lx1 - 0.06, z: STRS.za0 - 0.02, y: F - 0.16 });  // 西平台北缘
    edges.push({ x0: STRS.lx0 + 0.06, x1: STRS.lx1 - 0.06, z: STRS.zb1 + 0.02, y: F - 0.16 });  // 西平台南缘
    if (fi < FLOORS.length - 1) {
      edges.push({ x0: STRS.lx1 + 0.06, x1: STRS.wx1 - 0.06, z: STRS.zb1 + 0.02, y: F - 0.16 });  // 步行廊南缘
      edges.push({ x0: STRS.wx1 + 0.06, x1: STRS.ex1 - 0.06, z: STRS.za0 - 0.02, y: F - 0.16 });  // 折返平台北缘
      edges.push({ x0: STRS.wx1 + 0.06, x1: STRS.ex1 - 0.06, z: STRS.zb1 + 0.02, y: F - 0.16 });  // 折返平台南缘
    }
  });
  return buildEaveDrips(edges);
}
