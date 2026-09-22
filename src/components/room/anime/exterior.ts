import { buildApartmentPodium, type PodiumAutoDoor } from './apartmentPodium';
import { rebuildApartmentArchitecture } from './apartmentArchitecture';
/**
 * 室外世界层（B+ 架构，按到房间的距离分层）。
 *
 * 分层：
 *   世界地面  一整张湿沥青，所有层的承载者（Stage 1）
 *   近景街道  真 3D：对面楼 / 路灯 / 湿地反光（Stage 2）
 *   公寓本体  四层集合住宅（Stage 2，单侧外廊 + 东端折返楼梯）
 *   便利店    街对过近景主体（Stage 2b）
 *   中景环带  低模楼群，吃雾（Stage 2）
 *   天空      由 postfx.background 纯色兜底
 *
 * 美术方向：冷蓝灰环境 + 暖黄窗光。建筑一律低饱和冷灰，
 * 不使用大面积米白 / 象牙白 / 暖米白。墙是有厚度的混凝土；窗是"墙上的洞"
 * （窗洞深度 + 窗框 + 玻璃 + 窗台 + 中梃 + 窗帘）；阳台是真正挑出墙外的结构；
 * 树是有层次的简化枝叶，不是圆球低模。
 *
 * 性能约定：整层合批 + 冻结；小件共享几何实例（见 gbox）；
 * 便利店湿地反射使用独立低分辨率逐帧通道；装饰按材质合批。
 */

import * as THREE from 'three';
import {
  toon, emissive, makeRng, makeCanvas, toTexture,
  asphaltTexture, curbTexture, puddlePatchTexture,
  rainGlassTexture, curtainTexture, doorGrainMap,
} from './toon';
import { scaleUV, outlineProp, buildStreetLamp, buildWetGround, buildPuddleRipples, buildEaveDrips, UNIT_DOOR_TONES, type PoolSpec, type DripEdge } from './props';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import type { BoxColliderSpec, Collider } from './collider';
import { dressConvenienceStore } from './storeDetails';

/* ============================================================================
 * 色板：冷灰蓝城市住宅
 * ========================================================================== */

/**
 * 整栋楼不许出现大面积米白 / 象牙白 / 暖黄；建筑一律低饱和冷灰
 * （浅冷灰 / 蓝灰 / 灰褐 / 石墨灰 / 深灰蓝），明度压在中低区——
 * 暖黄窗光、入口照明、便利店、路灯才是画面里唯一的高光，颜色靠明度分层而非色相。
 *
 * 不同部位用不同但协调的材质：墙体 / 阳台板 / 栏杆 / 窗框 / 玻璃 / 木材 /
 * 金属 / 植物 / 织物；同层之内也不共用一色（主墙与山墙差一档）。
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

  const SIZE = 360;
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
 * Stage 2 — 近景街道层（路灯 + 湿地）
 * ========================================================================== */

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
 * 街道断面常量：建筑 → 人行道 → 路缘 → 车行道 → 排水篦 → 便利店侧人行道
 * 断面几何已由程序化街区（urbanStreets）铺出；这里只保留本模块共用的边界值。
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
 *           所以口子开在这里不必去动那几道路缘石——它们一律只铺 x≥-16，
 *           西侧本来就是空地。
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

  // 局部浅水：压在远处蓝地面之上，把"一块均匀蓝地板"变成"能读出水面层次的雨夜街道"。
  // （街道断面本身已由程序化街区 urbanStreets 铺出，这里只补低洼处的积水。）
  // 整组已标 sceneCollideSkip。
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
  /**
   * 南侧入口的自动感应门。它挂在 `group` 之外——这一整棵树会被合批 + 冻结，
   * 门扇进去就被焊死了。调用方单独 add、单独驱动，且不要描边（与门厅其他
   * 构件同属 noOutline 的一套）。
   */
  autoDoor: PodiumAutoDoor;
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
    frame: new THREE.MeshStandardMaterial({color: '#8b897e', roughness: .42, metalness: .3}),                                // 窗框
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
    /* —— 住户门的三档（同一建筑语言内的"每户不一样"） ——
     * 色值取自 props.ts 的 UNIT_DOOR_TONES：三档都压到低明度、彼此拉开色相。
     * 原先用的是 woodDeep / doorB 两个几乎同色的木棕，暖光把墙（wallAlt 蓝灰）
     * 也染暖之后，门和墙、门和门之间全糊在一起，一排看过去像同一块板。
     * 纹理走 doorGrainMap()（白底细纹），三档共用一张图、靠 color 相乘上色。 */
    doorA: toon(UNIT_DOOR_TONES[0], { map: doorGrainMap() }),  // 胡桃棕
    doorB: toon(UNIT_DOOR_TONES[1], { map: doorGrainMap() }),  // 炭
    doorC: toon(UNIT_DOOR_TONES[2], { map: doorGrainMap() }),  // 灰橄榄
    doorJamb: toon('#39414c'),                          // 户门门套：比窗框再深一档，压在三种门色上都不糊
    doorTrim: toon('#6b7a86'),                          // 门面压条：三种深色门上都读得出来的中蓝灰
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
    const fw = 0.036;
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
    put(gbox(0.025, h, 0.035), M.frame, x + w * 0.12, y, z + dir * 0.01 - dir * 0.008);

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
  /**
   * 覆板切口相对洞口外放的值（米）。
   *
   * 覆板不是"一块带洞的板"，是被洞口切成若干实心块拼出来的——每块的**边缘面**
   * 原本正好落在洞口边界（x=h.a/h.b、y=h.y0/h.y1）上。而房间外壳在同一个位置
   * 还有一层洞口断面（reveal），两个面**同向、共面**，于是洞口侧壁会 z-fighting：
   * 203 北窗最明显（那边 `noFrame`，没有窗框挡着断面）。
   *
   * 让切口比洞口外放 3mm，覆板洞壁就退到断面外侧、把它整个盖住——洞口的侧壁
   * 改由覆板呈现，断面只在缝里露 3mm。洞口大 6mm，肉眼无感。
   */
  const HOLE_CLEAR = 0.003;
  const facadePanel = (
    x0: number, x1: number, y0: number, y1: number, z: number, thick: number,
    mat: THREE.Material, holes: Array<{ a: number; b: number; y0: number; y1: number }>
  ) => {
    const cuts = [...holes].sort((p, q) => p.a - q.a);
    let cx = x0;
    for (const h of cuts) {
      const a = h.a - HOLE_CLEAR, b = h.b + HOLE_CLEAR;
      const hy0 = h.y0 - HOLE_CLEAR, hy1 = h.y1 + HOLE_CLEAR;
      if (a > cx) put(gbox(a - cx, y1 - y0, thick), mat, (cx + a) / 2, (y0 + y1) / 2, z);
      if (hy0 > y0) put(gbox(b - a, hy0 - y0, thick), mat, (a + b) / 2, (y0 + hy0) / 2, z);
      if (y1 > hy1) put(gbox(b - a, y1 - hy1, thick), mat, (a + b) / 2, (hy1 + y1) / 2, z);
      cx = Math.max(cx, b);
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
      const fw = 0.036;
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
      // 204 的西侧隔板不能与 203 独立东墙 x=6.2 共面；南北窗后空腔都退开 4cm。
      const room203Gap = Math.abs(F - F2) < 0.001 && Math.abs(ux0 - 6.2) < 0.001 ? GAP : 0;
      put(gbox(0.06, H - GAP * 2, ROOM_D), mBed.wall, ux0 + 0.03 + room203Gap, F + H / 2, zMid);
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
      // 204 的西侧隔板不能与 203 独立东墙 x=6.2 共面；南北窗后空腔都退开 4cm。
      const room203Gap = Math.abs(F - F2) < 0.001 && Math.abs(ux0 - 6.2) < 0.001 ? GAP : 0;
      put(gbox(0.06, H - GAP * 2, ROOM_D), mBed.wall, ux0 + 0.03 + room203Gap, F + H / 2, zMid);
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
   *
   * **注意：这里的几何最终会被丢掉。** `apartmentArchitecture.rebuildApartmentArchitecture`
   * 会遍历所有 `railing-*` 组，用 `Box3.setFromObject` 量出它的位置和尺寸，
   * 然后 `child.clear()` 清空、按量出来的尺寸用木格栅重建一遍。所以这里建出来的
   * 竖栅只用于**定义包围盒**，一个三角形都不会进最终场景——想省三角形得改
   * apartmentArchitecture.ts 那边（见 `cedar-balustrade` 的注释），改这里没用。
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
    // 共享几何一次创建、整层楼几百根复用。
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
  // Ground floor is an open lobby; no solid apartment-core infill.

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
    put(gbox(0.08, RF - F2 + 0.12, DEPTH), M.wallAlt, wx + sx * 0.04, (F2 + RF + 0.1) / 2, CZ).name = 'apt-old-gable-' + sx;
  }
  for (const f of FLOORS) {
    // 各层腰线（幕板）：一条浅色混凝土带，把楼层从立面上读出来
    put(gbox(W, 0.14, 0.07), M.slabDry, 0, f - 0.07, ZF + 0.035);
    put(gbox(W, 0.14, 0.07), M.slabDry, 0, f - 0.07, ZN - 0.035);
  }

  // 屋顶：女儿墙 + 水箱 + 室外机 + 天线
  put(gbox(W + 0.3, 0.45, DEPTH + 0.3), M.slabDry, 0, RF + 0.225, CZ).name = 'apt-roof';
  put(gbox(1.7, 1.25, 1.7), M.metal, -6.5, RF + 0.45 + 0.62, -0.5).name='apt-old-roof-tank';
  put(gbox(0.9, 0.55, 0.7), M.metal, 4.0, RF + 0.45 + 0.28, 1.2).name='apt-old-roof-ac1';
  put(gbox(0.9, 0.55, 0.7), M.metal, 5.1, RF + 0.45 + 0.28, 1.2).name='apt-old-roof-ac2';
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
        put(gbox(uw - GAP * 2, 0.12, ZB - ZF), M.slab, ucx, F - 0.07, (ZF + ZB) / 2);
        put(gbox(0.16, 0.03, 0.16), M.dark, ux1 - 0.35, F - 0.005, ZB - 0.35);
        continue;
      }

      /* —— 阳台结构 —— */
      // 板：顶面 = 户内地板标高 F，挑出 1.6m。
      put(gbox(uw - GAP * 2, 0.14, ZB - ZF), M.slab, ucx, F - 0.07, (ZF + ZB) / 2);
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
      // 亮灯档（lit + lamp，能看进屋）从 40% 提到 62%：窗后的低模家具和暖色内衬
      // 本来就是为"从外面看进去"做的，压在 30% 的亮灯率下等于白做。磨砂 / 拉帘
      // 仍然保留四分之一——整栋楼不许出现「每户都一样的亮窗」，那是最假的一种假。
      const pickState = (): WinState => {
        const r = rnd();
        return r < 0.14 ? 'dark' : r < 0.66 ? 'lit' : r < 0.76 ? 'lamp' :
               r < 0.90 ? 'curtain' : r < 0.96 ? 'frost' : 'ajar';
      };
      const winState = pickState();        // 卧室窗
      const winState2 = pickState();       // 右腰窓（餐厨）
      // 高窓（窗台高、只看得到上半截）：亮灯档同样提到 56%
      const highState: WinState = (() => {
        const r = rnd();
        return r < 0.30 ? 'dark' : r < 0.86 ? 'lit' : 'frost';
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
    /* 4F 外廊顶板 —— 补上顶层外廊缺的天花板。
     *
     * 2F/3F 外廊的天花不是单独做的，就是**上一层这块廊板**（板底 F+LVL-0.14）。
     * 顶层上面没有楼层了，而屋顶亭子的板（`apartmentArchitecture.ts` 的
     * `roof-slab`，z ∈ [-6.05, 4.85]）只盖到主体量 ZF=4.7 外一点点 —— 外廊
     * z −7.15..−5.9 里只有最南 0.15m 有遮挡，其余 1.1m 露天。
     * 实测（`tmp/_probe-balcony-ceiling.mjs`，向北撒竖直射线）4F 五户 **0/7 命中**，
     * 而 2F/3F 是 7/7。
     *
     * 规格照抄各层廊板：CW × 0.14 × 1.25，z 中心 ZN−0.625。标高取 4F + 层高
     * = 11.8 —— 顶面正好压在 roof-slab 的底面上（两者法线相反、各自背面剔除，
     * 不构成共面），板底 11.66 ⇒ 4F 外廊净高 2.66m，与 2F/3F 完全同构。
     * 与南侧阳台顶板（`balcony-top-slab`，顶面同样取 11.8）是同一套标高口径。
     * 外缘压边见 `apartmentArchitecture.ts` 里新增的 11.8 那道 `rear-corridor-fascia`。 */
    if (fi === FLOORS.length - 1) {
      put(gbox(CW, 0.14, 1.25), M.slabDry, CX, F + LVL - 0.07, ZN - 0.625);
    }
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
    // West corridor end is open to the new lift tower.

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
        /* 门扇与门套由房间自己的 buildEntryDoor 画（props.ts），但门旁这一圈配件
         * 属于外廊的公共视觉语言——这里补齐。少了它，整排 15 扇门看过去只有 203
         * 这户没有对讲机 / 雨庇 / 门垫，一眼就认出是"没做完的玩家户"。 */
        {
          const D203 = 4.95, FACE203 = ZN - 0.155;   // 与邻户同一套坐标口径
          put(gbox(0.11, 0.17, 0.022), M.tread, D203 + 0.62, F + 1.30, FACE203 + 0.005);   // 对讲机面板
          put(gbox(0.075, 0.05, 0.008), M.dark, D203 + 0.62, F + 1.355, FACE203 - 0.008);  // 屏幕
          put(gbox(0.05, 0.016, 0.008), M.frame, D203 + 0.62, F + 1.255, FACE203 - 0.008); // 按键
          put(gbox(0.98, 0.05, 0.34), M.frame, D203, F + 2.30, FACE203 - 0.175);           // 门楣雨庇
          put(gbox(0.72, 0.016, 0.4), toon('#3a4250'), D203, F + 0.018, ZN - 0.44);        // 门垫
          if (rnd() < 0.55) potted(D203 - 0.64, F + PLATE, ZN - 0.42, 500 + fi * 9 + ui);  // 门口盆栽
        }
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
      const doorTones = [M.doorA, M.doorB, M.doorC];
      const unitNum = ['05', '04', '03', '02', '01'][ui];
      /* —— 玄関ドア ——
       * 覆板厚 0.13、外表面在 ZN-0.155，门洞 0.9×2.2 就开在这块板里。旧做法是
       * 往洞底贴一块 0.86×2.16 的素板、再垫一块更大的板当"门框"——可那块门框
       * 落在覆板**内侧**，从外廊根本看不见，于是整扇门读起来就是"墙上开了个洞、
       * 洞底贴了张纸"：没有门套进深、没有五金、门牌灯还被 13cm 的板埋了。
       * 这里补成三段结构：
       *   ① 门套：两侧立梃 + 上槛，外表面比墙面凸 8mm、内表面比覆板内表面再深
       *      7mm，把洞口侧壁整个包住，给洞口一道看得见的收口；
       *   ② 门扇：比净开口小 1.2cm 嵌在门套里，门面上加两道竖向压条分成三块
       *      （玄関ドア的典型分格），再补猫眼与杠杆把手；
       *   ③ 门旁对讲机 + 门楣雨庇：日式集合住宅的识别性细节，把"这是一户人家"
       *      和"这是一排预制板"区分开。
       * 所有贴面件的 z 都按"覆板外表面"起算——忘了这条就会被板埋掉。 */
      const HOLE_W = 0.9, HOLE_H = 2.2;      // 与 unitFace 的洞口一致
      const JAMB = 0.055;                    // 门套立梃宽
      const WALL_FACE = ZN - 0.155;          // 覆板外表面（板厚 0.13、中心 ZN-0.09）
      const TRIM_FACE = WALL_FACE - 0.008;   // 门套外表面：比墙面凸 8mm，免与覆板共面
      const SET_IN = 0.145;                  // 门套进深：比覆板厚 1.5cm，洞壁整个盖住
      const jambHalf = HOLE_W / 2 - JAMB / 2 + 0.005;   // 立梃中心到门心
      const leafW = HOLE_W - JAMB * 2 + 0.005 - 0.012;  // 门扇宽：净开口再留 1.2cm 缝
      const leafMat = doorTones[(ui + fi * 2) % 3];
      const leafZ = WALL_FACE + 0.125;       // 门扇外表面：凹进门套 13.3cm
      const leafTop = HOLE_H - 0.06;
      // ① 门套：两侧立梃 + 上槛
      for (const s of [-1, 1]) {
        put(gbox(JAMB, HOLE_H + 0.06, SET_IN), M.doorJamb,
          doorX + s * jambHalf, F + (HOLE_H + 0.06) / 2, TRIM_FACE + SET_IN / 2);
      }
      put(gbox(HOLE_W + 0.02, JAMB, SET_IN), M.doorJamb,
        doorX, F + HOLE_H - JAMB / 2, TRIM_FACE + SET_IN / 2);
      // ② 门扇本体 + 门面分格 + 五金
      put(gbox(leafW, leafTop, 0.06), leafMat, doorX, F + leafTop / 2, leafZ + 0.03);
      for (const s of [-1, 1]) {
        put(gbox(0.02, leafTop - 0.24, 0.012), M.doorTrim, doorX + s * leafW / 5, F + leafTop / 2, leafZ - 0.004);
      }
      put(gbox(0.05, 0.05, 0.018), M.metal, doorX, F + 1.62, leafZ - 0.010);              // 猫眼
      put(gbox(0.048, 0.15, 0.022), M.metal, doorX + leafW / 2 - 0.055, F + 1.02, leafZ - 0.012);  // 把手底座
      put(gbox(0.145, 0.032, 0.028), M.metal, doorX + leafW / 2 - 0.145, F + 1.02, leafZ - 0.026); // 杠杆把手
      // ③ 对讲机 + 雨庇
      put(gbox(0.11, 0.17, 0.022), M.tread, doorX + 0.62, F + 1.30, WALL_FACE + 0.005);
      put(gbox(0.075, 0.05, 0.008), M.dark, doorX + 0.62, F + 1.355, WALL_FACE - 0.008);
      put(gbox(0.05, 0.016, 0.008), M.frame, doorX + 0.62, F + 1.255, WALL_FACE - 0.008);
      put(gbox(0.98, 0.05, 0.34), M.frame, doorX, F + 2.30, WALL_FACE - 0.175);
      // 门牌灯 / 户号牌 / 户主牌：一律贴到覆板外表面之外（旧代码把门牌灯放在
      // ZN-0.03，正好埋在 13cm 厚的板里，外廊上从来就没亮过）
      panel(0.16, 0.09, M.warmPale, doorX + 0.60, F + 2.30, WALL_FACE - 0.006, -1);  // 门牌灯
      panel(0.19, 0.14, emissive('#d9cfbc', { map: numberPlateTexture(`${fi + 2}${unitNum}`) }),
        doorX + 0.60, F + 1.88, WALL_FACE - 0.012, -1);                              // 户号牌（微背光）
      // 户主牌（表札）：写住户名字——203 是主角 Vivian Nana，其余用常见大模型填充。
      // 贴在户号牌右侧的墙上（dir=-1 朝外廊），与冷灰门牌形成冷暖对比。
      const resident = RESIDENTS[`${fi + 2}${unitNum}`];
      if (resident) {
        panel(0.30, 0.12, emissive('#fff6ea', { map: namePlateTexture(resident) }),
          doorX + 0.94, F + 1.88, WALL_FACE - 0.012, -1);                           // 户主牌（亚克力表札）
      }
      const matTones = ['#3a4250', '#463f37', '#33413b', '#514639'];
      put(gbox(0.72, 0.016, 0.4), toon(matTones[(ui + fi) % 4]), doorX, F + 0.018, ZN - 0.44);  // 门垫
      if (rnd() < 0.55) potted(doorX - 0.64, F + PLATE, ZN - 0.42, 500 + fi * 9 + ui);          // 门口盆栽
      /* —— 三扇窗：玻璃全走透明档 ——
       * 住宅层实心体量北端已退 ROOM_D（DEPTHB2），窗后是真屋子（unitInteriorNorth），
       * 亮不亮由室内材质自己说话，不再贴发光剪影。
       * 三扇状态各自独立随机：整栋楼不许出现「每户都一样的亮窗」，那是最假的
       * 一种假。磨砂档给浴室（走廊侧的私密），半开档只给少数几户。 */
      /* 北面窗的状态分布。北面是**外廊侧**——玩家每天从这条廊进出，窗里有没有人
       * 是全楼唯一能被看清的"住人感"来源，所以亮灯档给得比南面还重：lit ≈ 56%。
       * 剩下的暗 / 磨砂 / 拉帘合计四成多，留出"整栋楼不是复印出来的"那点参差。 */
      const pickNorth = (allowAjar: boolean): WinState => {
        const v = rnd();
        if (allowAjar) {
          return v < 0.16 ? 'dark' : v < 0.72 ? 'lit' : v < 0.86 ? 'frost' :
                 v < 0.96 ? 'curtain' : 'ajar';
        }
        return v < 0.18 ? 'dark' : v < 0.74 ? 'lit' : v < 0.90 ? 'frost' : 'curtain';
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
    // 配电室北墙碰撞：三段墙 + 闭着的钢门（无开门机制）整面收成一道实墙，禁止穿墙进配电室
    boxes.push(boxSpec('stair-store-north', 31.12, 32.44, 0, TW, za0 + 0.03 - 0.04, za0 + 0.03 + 0.04));
    // 南墙（带两个通风格栅）
    put(gbox(1.32, TW, 0.08), M.trunkWall, 31.78, TW / 2, zb1 + 0.04);
    for (const vx of [31.52, 32.06]) {
      put(gbox(0.28, 0.2, 0.02), M.dark, vx, 2.1, zb1 + 0.005);
      for (let s = 0; s < 3; s++) put(gbox(0.24, 0.018, 0.03), M.metal, vx, 2.04 + s * 0.06, zb1 - 0.005);
    }
    boxes.push(boxSpec('stair-store-south', 31.12, 32.44, 0, TW, zb1 + 0.04 - 0.04, zb1 + 0.04 + 0.04));
    // 东墙（设备墙：燃气表一排 + 分电盘）
    put(gbox(0.08, TW, zb1 - za0 - 0.06), M.trunkWall, 32.42, TW / 2, (za0 + zb1) / 2);
    boxes.push(boxSpec('stair-store-east', 32.42 - 0.04, 32.42 + 0.04, 0, TW, za0 + 0.03, zb1 - 0.03));
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
  const SOUTH_Z = ZF + 0.180;          // 外表面 4.945，比房间外墙 4.92 向外错开 25mm，避免共面
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
    const fw = 0.036;
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
  // 位置对齐邻户那一套（门心 +0.60 / +0.94、高度 +1.88）：203 的门在 x=4.95，
  // 于是落在 5.55 / 5.89。旧值 5.41 / 5.75 是照老门洞写的，外廊上一排看过去会错位。
  panel(0.19, 0.14, emissive('#d9cfbc', { map: numberPlateTexture('203') }), 5.55, F2 + 1.88, ZN - 0.165, -1);
  // 户主牌（表札）：203 是主角 Vivian Nana（与邻户牌同款、dir=-1 朝外廊，贴门牌右侧）
  panel(0.30, 0.12, emissive('#fff6ea', { map: namePlateTexture(RESIDENTS['203']) }), 5.89, F2 + 1.88, ZN - 0.165, -1);
  // 门牌灯：邻户户门上方都有这一盏，203 之前漏了
  panel(0.16, 0.09, M.warmPale, 5.55, F2 + 2.30, ZN - 0.161, -1);

  // Replace the entire ground-floor frontage and its clutter with a residential podium.
  const podium = buildApartmentPodium();
  g.add(podium.group);
  boxes.push(...podium.boxes);

  // 窗玻璃收尾合并：所有 pane 到这里才落进 group（见上方 paneBuf 的说明）
  flushPanes();

  rebuildApartmentArchitecture(g);
  return { group: g, boxes, autoDoor: podium.autoDoor };
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
  const { canvas, ctx } = makeCanvas(1536, 128);
  ctx.fillStyle = '#f1e9d3'; ctx.fillRect(0, 0, 1536, 128);
  ctx.fillStyle = '#79b3a1'; ctx.fillRect(0, 0, 1536, 10);
  ctx.fillStyle = '#be8e96'; ctx.fillRect(0, 120, 1536, 8);
  ctx.strokeStyle = '#426f62'; ctx.lineWidth = 4;
  ctx.beginPath(); ctx.arc(78, 63, 32, 0, Math.PI * 2); ctx.stroke();
  ctx.beginPath(); ctx.ellipse(76, 61, 12, 22, -0.5, 0, Math.PI * 2); ctx.stroke();
  ctx.fillStyle = '#34584f'; ctx.textBaseline = 'middle';
  ctx.font = 'bold 66px system-ui, sans-serif'; ctx.fillText('CITY MART', 150, 58);
  ctx.font = '16px system-ui, sans-serif'; ctx.fillText('FRESH FOOD  ·  COFFEE  ·  EVERY DAY', 155, 101);
  ctx.font = '27px "Yu Gothic", "Meiryo", sans-serif'; ctx.fillText('街角の、ひと休み。', 795, 66);
  ctx.fillStyle = '#4f8374'; ctx.fillRect(1328, 25, 158, 77);
  ctx.fillStyle = '#fff4dd'; ctx.font = 'bold 48px system-ui, sans-serif'; ctx.fillText('24h', 1356, 64);
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
  // 新首层中央 x∈[-1.2, 1.2] 留出 2.4m 入口，门扇停放在两侧。
  boxSpec('apt-south-1f-w', APT_X0 - SHELL_T, -1.2, 0, FLOORS[0], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-south-1f-e', 1.2, APT_X1 + SHELL_T, 0, FLOORS[0], APT_ZF - SHELL_T, APT_ZF + SHELL_T),
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

  // Ground-floor lobby walls and furniture are supplied by buildApartmentPodium.

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
  // Ground-floor west wall opens into the added lift hall.
  boxSpec('apt-west-up',  APT_X0 - SHELL_T, APT_X0 + SHELL_T, FLOORS[0], SHELL_RF, APT_ZN - SHELL_T, APT_ZB + SHELL_T),
  boxSpec('apt-east-1f',  APT_X1 - SHELL_T, APT_X1 + SHELL_T, 0, FLOORS[0], APT_ZN - SHELL_T, APT_ZF + SHELL_T),
  boxSpec('apt-east-up',  APT_X1 - SHELL_T, APT_X1 + SHELL_T, FLOORS[0], SHELL_RF, APT_ZN - SHELL_T, APT_ZB + SHELL_T),
];

export type StoreHandle = {
  /** 静态部分：调用方走标准装配（合批 → 描边 → add → 冻结） */
  group: THREE.Group;
  /** 动效部分：已各自描边，add 即可。绝不能冻结——门要滑、灯要闪 */
  dynamic: THREE.Group;
  /** 门扇的动态碰撞盒（每扇一个，随门滑动实时更新 min/max）。
   *  门扇的金属杆件在几何上标了 `userData.noCollide`，`buildSceneColliders` 会跳过它们，
   *  所以**调用方必须把这个数组 concat 进碰撞表** —— 否则门洞一点碰撞都没有，
   *  门关着玩家也能直接穿过去。 */
  doorColliders: Collider[];
  /** 店面的**声明式**碰撞盒：橱窗玻璃那一整面，门洞处断开。
   *
   *  为什么必须声明而不能靠遍历：玻璃是 opacity 0.075 的透明材质，
   *  `buildSceneColliders` 的「透明跳过」规则会把两片橱窗（共 7.4m 宽）整片放过，
   *  玩家能从街上直接穿玻璃进店；而玻璃又不能改成不透明材质。
   *  调用方 `buildBoxColliders(store.boxes)` 接进碰撞表。 */
  boxes: BoxColliderSpec[];
  /** 每帧更新，t 是从开局累计的秒数（与浮尘同一时基）。
   *  playerPos 是玩家（第一人称）或观察者相机的世界坐标，用来驱动感应门；
   *  不传时按「没人」处理，门保持关闭。 */
  update: (t: number, wet: boolean, playerPos?: THREE.Vector3) => void;
  /** 回填碰撞表给街面积水反射面做**遮挡**剔除（见 storeDetails 的 setOccluders）。
   *
   *  必须在装配末尾、`fpsColliders` 定稿之后调用：它靠的是**合批之前**逐件收的
   *  AABB —— 并完之后楼板的 AABB 会跨多层或横跨整条街，认不出"薄板"，判据就废了。
   *  不调用只是少一层优化（室内看街面时白跑一趟反射），不影响正确性。 */
  setWetOccluders: (colliders: Collider[]) => void;
  dispose: () => void;
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
    finish: 'glass', transparent: true, opacity: 0.075, depthWrite: false,
  });
  const glassCold = toon('#dce9f2', {      // 冷饮柜玻璃门
    finish: 'glass', transparent: true, opacity: 0.10, depthWrite: false,
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
  /* 地板 / 吊顶比店宽窄 0.28（= 两侧 0.14 侧墙的厚度），正好落在两墙内表面之间。
   *
   * 不能做满 ST_W：两侧墙的外表面就在 x0 / x1 上，地板做满宽的话它的 x0 端面与
   * 西侧墙的外表面**精确共面**，在店面西立面上叠出一条 0.083×2.37 的共面区
   * （东侧被东山墙的 0.08 厚压边盖住，探针判为不可见，所以只有西侧暴露）。
   * 收进侧墙厚度之后，地板端面与侧墙内表面背靠背（法线相反），是贴合面不是暴露面。
   * 吊顶不做同样处理：它 y 2.9..3.0，与侧墙（y 0..2.9）在 y 上刚好相接、没有重叠。 */
  put(bx(ST_W - 0.28, 0.1, FH), floorTile, ST_CX, 0.05, FZ);
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

  /* 玻璃内侧海报两张（贴在玻璃后面，朝外）。
   *
   * **每张都要标 sceneCollideSkip。** 海报是零厚度平板，本身不该挡人，但
   * `buildSceneColliders` 只认 AABB、不认厚度：一张 0.6×0.85 的海报会生成一个
   * 加了 0.04 余量的实心盒。玻璃没有碰撞的时候它们还看不出问题（正好堵着玻璃），
   * 一旦玻璃的碰撞由下面 boxes 显式声明，这些盒就纯属多余；而门洞正中那张
   * （见「店内玻璃后面的补充海报」）更是直接变成一堵看不见的墙——门开着也进不去。 */
  const posterW1 = faceS(0.6, 0.85, posterMat, -2.9, 1.75, 14.06);
  posterW1.name = 'store-poster-w1';
  posterW1.userData.sceneCollideSkip = true;
  /* x 从 4.9 挪到 4.55：原来与右侧那张（5.4，占 5.125..5.675）在 5.125..5.2 上
   * 叠了 7.5cm —— 两张同材质、同在 z=14.06 的共面平板，重叠区是 0.056 m² 的共面区。
   * 挪开后 4.25..4.85 与 5.125..5.675 之间留 27.5cm。 */
  const posterE1 = faceS(0.6, 0.85, posterMat, 4.55, 1.75, 14.06);
  posterE1.name = 'store-poster-e1';
  posterE1.userData.sceneCollideSkip = true;

  /* ================= 正面：玻璃 / 雨棚 / 招牌 ================= */

  // 橱窗玻璃：门在 x0..1.6，左右两片。
  // 用薄板不用 box：玻璃在 8m 外，厚度读不出来，但平面省一半顶点。
  faceS(3.4, 2.19, glassMat, -1.85, 1.445, ST.zFront - 0.01).name = 'store-glass-w';
  faceS(4.0, 2.19, glassMat, 3.7, 1.445, ST.zFront - 0.01).name = 'store-glass-e';
  /* 两端再各补一片。分格竖框只到 −3.55 / 5.7，再往外到角柱（−4.38 / 6.58）
   * 之间原本是 0.85 / 0.90 宽的**空档**——没玻璃也没墙，从街上斜看能直接看进
   * 前厅，人也能走进去。补上之后正面才是一层完整的皮，
   * 下面那两个声明式碰撞盒（store-front-w/e）也才有对应的实体可挡，不至于是空气墙。 */
  faceS(0.85, 2.19, glassMat, -3.975, 1.445, ST.zFront - 0.01).name = 'store-glass-w-end';
  faceS(0.9, 2.19, glassMat, 6.15, 1.445, ST.zFront - 0.01).name = 'store-glass-e-end';
  // 竖向分格 + 下墙裙（墙裙用深灰蓝，和上面的冷灰墙拉开明度）
  for (const x of [-3.55, -0.15, 1.7, 5.7]) put(bx(0.08, 2.19, 0.08), metal, x, 1.445, ST.zFront - 0.04);
  /* 墙裙**在门洞处断开**：门洞净宽 1.77（x −0.11..1.66），横贯整面的话会在门口横一道
   * 0.35m 高的墙，门扇下半截埋在里面、玩家进门还要跨过去。左右两段各自伸进竖框 1cm
   * 收头（端面退到 −0.20 / 1.75，避开竖框的 −0.19 / 1.74 两个面，免得端面共面闪）。 */
  put(bx(-0.20 - ST.x0, 0.35, 0.12), wallDeep, (ST.x0 - 0.20) / 2, 0.175, ST.zFront - 0.03);
  put(bx(ST.x1 - 1.75, 0.35, 0.12), wallDeep, (1.75 + ST.x1) / 2, 0.175, ST.zFront - 0.03);
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
  //
  /* 2026-09-22：这两台**原来一个碰撞盒都没有** —— 1.9m 高的实心柜，玩家直接穿过去
   * （与居酒屋北侧 / 停车场北侧那两台同一个坑：`buildConvenienceStore` 走的是
   * 声明式 `boxes`，而这台的登记一直没人写）。
   *
   * 盒取「机身 ∪ 顶盖」的并集，两处数字都由下面这组常量派生，不另抄一遍：
   *   机身 bx(1.05, 1.9, 0.72) @ (x, 0.95, 13.3) ⇒ x±0.525 / y 0..1.9   / z 12.94..13.66
   *   顶盖 bx(1.09, 0.09, 0.76) @ (x, 1.94, 13.3) ⇒ x±0.545 / y 1.895..1.985 / z 12.92..13.68
   * 投币口 bx(0.16,0.2,0.06) @ (x+0.36, 1.0, 12.95) 前探 2cm，落在并集之内，不必单列。
   * 逐顶点扫过柜体盒（`tmp/_probe-furniture-spot.mjs`）：里面只有它自己的机身/顶盖/
   * 投币口和贴面的发光屏，没有别的构件。 */
  const VEND_HX = 0.545, VEND_TOP = 1.99, VEND_Z0 = 12.92, VEND_Z1 = 13.68;
  let vendN = 0;
  const vending = (x: number, panel: string, strip: string): BoxColliderSpec => {
    put(bx(1.05, 1.9, 0.72), toon('#2c3a4e'), x, 0.95, 13.3);
    put(bx(1.09, 0.09, 0.76), toon('#1f2a38'), x, 1.94, 13.3);   // 顶盖阴影缝
    faceS(0.88, 1.06, emissive(panel), x, 1.28, 12.93);
    faceS(0.88, 0.3, emissive(strip), x, 0.5, 12.93);
    put(bx(0.16, 0.2, 0.06), metal, x + 0.36, 1.0, 12.95);       // 投币口 / 操作板
    return boxSpec(`store-vending-${++vendN}`, x - VEND_HX, x + VEND_HX, 0, VEND_TOP, VEND_Z0, VEND_Z1);
  };
  const vendingBoxes: BoxColliderSpec[] = [
    vending(-5.75, '#d2dfeb', '#dd9a63'),
    vending(-4.6, '#dde8f1', '#5fae9f'),
  ];

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
  // 终点挂书店西墙。**书店 2026-09 扩建后（北墙 18.0 → 15.0、西墙 z 15..26.1）
  // 这两根线才真的挂上墙** —— 扩建前书店西墙从 z=18 才起，端点是悬空的。
  //   端点 x 收 1cm（12.7 → 12.67）：TubeGeometry 管壁半径 0.012，写 12.7 会让
  //   管头扎进墙面 0.002，逐顶点侵入检测会报一条。
  //   z 从 15.0/15.8 挪到 15.6/16.4：15.0 正好落在书店西北角棱上，往南挪 0.6m
  //   才读成「挂在墙上」；两处都在居酒屋东山墙（z 14.0..17.8）范围内。
  //   y 收到 4.2/4.0：书店二层窗从 y 3.4 起（1F 层高 3.4），窗外沿 4.35，够不着。
  awire([IZ.x1, 3.3, 15.6], [12.67, 4.2, 15.6], 0.14);
  awire([IZ.x1, 3.3, 16.4], [12.67, 4.0, 16.4], 0.16);

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
    const ogGlass = new THREE.Mesh(new THREE.PlaneGeometry(OG_W - 0.06, 0.62), toon('#dce9f2', { finish: 'glass', transparent: true, opacity: 0.10, depthWrite: false }));
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
    /* 玻璃后面再贴一张「ポイント 5倍」（暖红，比现有的米白更有存在感）。
     *
     * **x 原本是 0.6，正好落在门洞正中**（门洞 −0.11..1.66，中心 0.775）：
     * 视觉上是悬在门口半空的一块板，碰撞上是把门洞从 1.77 堵到只剩 0.63。
     * 感应门装好之后，"门明明开了却还是走不进去"就是这个原因。
     * 挪到门西侧第一格橱窗（−1.55..−0.85，与 −2.9 那张不重叠）。 */
    const posterW0 = faceS(0.7, 1.0, toon('#ffffff', { map: posterBTexture() }), -1.2, 1.55, 14.06);
    posterW0.name = 'store-poster-w0';
    posterW0.userData.sceneCollideSkip = true;
    const posterW2 = faceS(0.55, 0.75, posterMat, -3.9, 1.85, 14.06);
    posterW2.name = 'store-poster-w2';
    posterW2.userData.sceneCollideSkip = true;
    const posterE2 = faceS(0.55, 0.75, posterMat, 5.4, 1.7, 14.06);
    posterE2.name = 'store-poster-e2';
    posterE2.userData.sceneCollideSkip = true;
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

  /* 2) 感应门：两扇玻璃推拉门 + 门头灯箱
   *
   * 门洞净宽 = 玻璃竖框内边之间 = 1.66 − (−0.11) = 1.77，两扇各 0.88、中缝 10mm。
   * 门扇比早先宽（0.76 → 0.88）：原来的 0.76 是按那对多余门柱的间距（1.63）配的，
   * 门柱删掉之后沿用 0.76 会在门洞两边各留 8~13cm 的缝，直接看见洞口侧壁。
   * DOOR_OPEN 同步加大到 0.90，保证全开时门扇内缘退到竖框之外、完全让开门洞。 */
  const DOOR_SPAN = 1.77, DOOR_CX = 0.775;      // 门洞净宽 / 门洞中心
  const DOOR_W = 0.88, DOOR_HALF = 0.44;        // 单扇宽 / 门扇半宽（竖杆位置）
  const DOOR_LX = DOOR_CX - DOOR_W / 2 - 0.005, DOOR_RX = DOOR_CX + DOOR_W / 2 + 0.005;
  const DOOR_OPEN = 0.90;
  /* 门洞两侧的橱窗玻璃碰撞（声明式，见 StoreHandle.boxes）。
   *
   * 玻璃是透明材质，`buildSceneColliders` 会整片跳过 —— 从街上往店里走，
   * 除了门洞那一格，其余 7.4m 宽的橱窗原来一点碰撞都没有，直接穿玻璃进屋。
   * 这里把「除门洞外的整面」封成两段：左段 ST.x0..门洞西边、右段 门洞东边..ST.x1。
   * 门洞两侧的竖框自己还有碰撞盒（x −0.19..−0.11 / 1.66..1.74），正好接上。
   *
   * y 取 0.35..2.54：0.35 以下是与玻璃同层的墙裙（0.35 高，属于「可跨矮台」、
   * collidesAt 本来就不拦），2.54 是玻璃顶边。
   * z 取 13.95..14.03：玻璃面在 13.99，盒体跨在玻璃两侧 4cm，玩家（半径 0.15）
   * 被挡在 z≈13.80，刚好贴在玻璃外面，不会隔着 10cm 就被拦住。 */
  const DOOR_X0 = DOOR_CX - DOOR_SPAN / 2, DOOR_X1 = DOOR_CX + DOOR_SPAN / 2;
  const boxes: BoxColliderSpec[] = [
    boxSpec('store-front-w', ST.x0, DOOR_X0, 0.35, 2.54, 13.95, 14.03),
    boxSpec('store-front-e', DOOR_X1, ST.x1, 0.35, 2.54, 13.95, 14.03),
    /* 店西侧人行道那两台自动售货机（见上面 `vending()` 的注释）：机身 ∪ 顶盖，
     * x±0.545 / y 0..1.99 / z 12.92..13.68，由函数自己返回、这里只摊平。 */
    ...vendingBoxes,
  ];
  const leafMat = toon('#d7e5ef', { finish: 'glass', transparent: true, opacity: 0.10, depthWrite: false });
  const mkLeaf = (cx: number) => {
    const leaf = new THREE.Group();
    leaf.name = 'store-door-leaf';
    const glass = new THREE.Mesh(new THREE.PlaneGeometry(DOOR_W, 2.18), leafMat);
    glass.rotation.y = Math.PI;
    leaf.add(glass);
    /* 门扇的金属杆件**必须标 noCollide**：`buildSceneColliders` 只跳过 opacity<0.5 的
     * 材质，玻璃门扇会被跳过、但这些不透明的杆子不会 —— 每扇 3 根杆会变成 3 个静态
     * 碰撞盒杵在门洞里（RoomScene 那句「透明玻璃门扇也自然跳过」的注释就是这么失效的）。
     * 门扇的碰撞由下面每扇一个、随门滑动的动态盒承担。 */
    for (const dx of [-DOOR_HALF, DOOR_HALF]) {
      const bar = new THREE.Mesh(bx(0.05, 2.2, 0.05), metal);
      bar.position.set(dx, 0, 0.02);
      bar.userData.noCollide = true;
      leaf.add(bar);
    }
    const rail = new THREE.Mesh(bx(DOOR_W + 0.02, 0.06, 0.06), metal);
    rail.position.set(0, 1.09, 0.02);
    rail.userData.noCollide = true;
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

  /* 门扇的碰撞盒：每扇一个，**随门滑动**。
   *
   * 不能靠 buildSceneColliders 自动收：那是一次性静态遍历，门滑开后盒子还杵在门洞里 ——
   * 实测门 position.x 已经到 −0.34 / 1.94，自动收出来的盒子 min.x 仍停在 −0.045 / 0.715
   * 那一组全关位置，玩家会撞空气。所以这里自己建两个盒，交给调用方 concat 进碰撞表
   * （见 StoreHandle.doorColliders），update 里逐帧同步。
   * 高度取门扇实际范围：贴地到 2.26。 */
  const DOOR_BASE_Y = 0, DOOR_TOP_Y = 2.26;
  const doorColliders: Collider[] = [leafL, leafR].map((leaf, i) => ({
    min: new THREE.Vector3(leaf.position.x - DOOR_HALF, DOOR_BASE_Y, ST.zFront - 0.075),
    max: new THREE.Vector3(leaf.position.x + DOOR_HALF, DOOR_TOP_Y, ST.zFront - 0.025),
    source: i === 0 ? 'store-door-leaf-w' : 'store-door-leaf-e',
  }));
  const syncDoorColliders = () => {
    for (let i = 0; i < 2; i++) {
      const cx = (i === 0 ? leafL : leafR).position.x;
      doorColliders[i].min.x = cx - DOOR_HALF;
      doorColliders[i].max.x = cx + DOOR_HALF;
    }
  };
  syncDoorColliders();

  /* 感应门的三个状态量。唯一真状态是 doorOpenAmt（0..1），纯插值推进 ——
   * 暂停再恢复不会卡在半开。doorHold 是「人已离开、门还没关」的余量，防阈值抖动。 */
  let doorOpenAmt = 0, doorHold = 0, doorLastT = 0;
  const DOOR_SENSE = 1.8;   // 感应半径（米）：门中心到玩家的水平距离

  /* 门头灯箱（门楣那一道亮边，把入口从玻璃面里拎出来）。
   *
   * **不要再加一对门柱。** 门洞两侧本来就有橱窗分格的竖框（x=−0.15 / 1.7，见上面
   * `for (const x of [-3.55, -0.15, 1.7, 5.7])`），它们就是门洞的边框。早先这里又补了
   * 一对 0.09 宽的门柱（x=−0.06 / 1.66）：左侧和竖框并排、只差 5mm，右侧直接和竖框
   * 叠了 45mm，而且两者高度还不一样（竖框 0.31..2.58、门柱 −0.04..2.46）——从街上看，
   * 门洞两边各立着两根高低不齐的柱子，读作「双重的门」。删掉门柱、把灯箱宽度对齐
   * 竖框内边即可，门洞边框由竖框单独承担。 */
  const entryMat = emissive('#fff1dc');
  const ENTRY_BASE = entryMat.color.clone();
  const doorFrame = new THREE.Group();
  doorFrame.name = 'store-door-frame';
  const head = new THREE.Mesh(new THREE.PlaneGeometry(DOOR_SPAN, 0.2), entryMat);
  head.position.set(DOOR_CX, 2.36, ST.zFront - 0.06);
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
    const mat = emissive('#dfeaf2', { map: rainGlassTexture(), transparent: true, opacity: 0.075, side: THREE.DoubleSide });
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
  const inLight = new THREE.PointLight(0xffe2b8, 14, 16, 2);
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
  const poolMat = emissive('#ffe6bf', { map: radial('#ffe9c2', 'rgba(255,210,150,0)'), transparent: true, opacity: 0.24, side: THREE.DoubleSide });
  poolMat.blending = THREE.AdditiveBlending;
  poolMat.depthWrite = false;
  const pool = new THREE.Mesh(new THREE.PlaneGeometry(9.5, 4.2), poolMat);
  pool.geometry.rotateX(-Math.PI / 2);
  // y 必须压在铺装面之上：程序化街区的人行道面在 0.028、盲道条在 0.035。
  // 光池是贴地的加法平面，深度测试照常生效——低过铺装就会被自己的地面吃掉一半
  // （旧值 0.014 时，z∈[12.6,15] 那段整块看不见）。
  pool.position.set(ST_CX, 0.036, 13.2);
  pool.renderOrder = 2;
  pool.name = 'store-light-pool';
  pool.userData.sceneCollideSkip = true;
  dyn.add(pool);

  // 招牌暖晕：横招牌前方叠一层柔光，雨夜里像灯箱在发烫的辉光。
  const haloMat = emissive('#ffd9a0', { map: radial('rgba(255,225,180,0.9)', 'rgba(255,200,150,0)'), transparent: true, opacity: 0.20, side: THREE.DoubleSide });
  haloMat.blending = THREE.AdditiveBlending;
  haloMat.depthWrite = false;
  const halo = new THREE.Mesh(new THREE.PlaneGeometry(11.6, 1.7), haloMat);
  halo.position.set(ST_CX + 0.1, 3.5, ST.zFront - 0.2);
  halo.renderOrder = 3;
  halo.userData.sceneCollideSkip = true;
  dyn.add(halo);

  /* ---------------- 每帧更新 ---------------- */
  const dressing = dressConvenienceStore(g, dyn);
  const update = (t: number, wet: boolean, playerPos?: THREE.Vector3) => {
    dressing.update(t, wet);
    dripGroup.visible = wet;
    pool.visible = wet;
    for (const glassRain of rainMeshes) glassRain.visible = wet;
    // 招牌：0.955~1.0 慢呼吸 + 每 11.3s 一次 0.28s 的启辉抖动。
    // 抖动的频率（61rad/s）故意远离呼吸频率，两者不共振才像真的老灯管。
    const breathe = 0.955 + 0.045 * Math.sin(t * 1.7);
    const glitch = t % 11.3 < 0.28 ? 0.62 + 0.3 * Math.sin(t * 61) : 1;
    signMat.color.copy(SIGN_BASE).multiplyScalar(breathe * glitch);
    entryMat.color.copy(ENTRY_BASE).multiplyScalar(0.94 + 0.06 * Math.sin(t * 2.3 + 1.1));

    /* 感应门：玩家（或观察者相机）走到门口一定范围内就开，走开后延时关闭。
     *
     * 原来这里是纯时间驱动（21s 一轮无条件开合）：站在门口它照样关、人走开了它照样开，
     * 跟玩家没有任何关系，读起来不像感应门。改成距离触发 —— 开门即时、关门滞后
     * （doorHold 余量），开合本身走插值，不会瞬间跳。 */
    /* dt 夹到 0.25（4fps）而不是更小：夹太紧的话低帧率下门会开得极慢
     * （headless 下 rAF 被节流到 ~1fps，夹 0.1 时 0.55s 的行程要 5.5 帧 = 5.5 秒）。
     * 上界仍然必要 —— 标签页切回来时 t 会跳一大截，不夹就会瞬间弹开。 */
    const dt = Math.min(0.25, Math.max(0, t - doorLastT));
    doorLastT = t;
    const pdx = playerPos ? playerPos.x - DOOR_CX : 1e3;
    const pdz = playerPos ? playerPos.z - (ST.zFront - 0.05) : 1e3;
    const nearDoor = pdx * pdx + pdz * pdz < DOOR_SENSE * DOOR_SENSE;
    doorHold = nearDoor ? 1.2 : Math.max(0, doorHold - dt);
    const wantOpen = nearDoor || doorHold > 0 ? 1 : 0;
    /* 开 0.55s、关 0.85s 全行程：感应门开得快、关得慢一点，像还在等传感器 */
    const dStep = dt / (wantOpen ? 0.55 : 0.85);
    const dDiff = wantOpen - doorOpenAmt;
    if (Math.abs(dDiff) <= dStep) doorOpenAmt = wantOpen;
    else doorOpenAmt += Math.sign(dDiff) * dStep;
    leafL.position.x = DOOR_LX - doorOpenAmt * DOOR_OPEN;
    leafR.position.x = DOOR_RX + doorOpenAmt * DOOR_OPEN;
    syncDoorColliders();

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
    inLight.intensity = 14 * inB * inG;
    inLight2.intensity = 6 * (0.96 + 0.04 * Math.sin(t * 1.1 + 2.0));
    inCool.intensity = 4 * (0.95 + 0.05 * Math.sin(t * 0.9 + 1.3));
  };

  return { group: g, dynamic: dyn, doorColliders, boxes, update, setWetOccluders: dressing.setOccluders, dispose: dressing.dispose };
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

  // ---- 12) 出口光池：站厅的暖光从楼梯口淌到湿人行道上 ----
  // 这是全街区唯一一处「光从地底下冒出来」的地方，此前完全没有地面响应，
  // 雨夜看过去只剩一个黑洞口。两片叠加：宽的一片是雨棚下的漫射，窄的一条
  // 是楼梯间正对洞口的那道集中亮带（口朝 -z，所以整片铺在 ENTRY_Z 以北）。
  // y=0.036 压住人行道面(0.028)与盲道条(0.035)。
  const spillMat = emissive('#ffe4b5', {
    map: radial('#ffe7bd', 'rgba(255,215,160,0)'),
    transparent: true, opacity: 0.28, side: THREE.DoubleSide,
  });
  spillMat.blending = THREE.AdditiveBlending;
  spillMat.depthWrite = false;
  const spill = flatDown(9.6, 3.4, spillMat, CX, 0.036, 13.1);
  spill.name = 'subway-light-spill';
  spill.renderOrder = 2;
  spill.userData.sceneCollideSkip = true;

  const throatMat = emissive('#fff0d4', {
    map: radial('rgba(255,240,212,0.9)', 'rgba(255,220,170,0)'),
    transparent: true, opacity: 0.32, side: THREE.DoubleSide,
  });
  throatMat.blending = THREE.AdditiveBlending;
  throatMat.depthWrite = false;
  const throat = flatDown(7.2, 1.5, throatMat, CX, 0.038, 13.7);
  throat.name = 'subway-light-throat';
  throat.renderOrder = 3;
  throat.userData.sceneCollideSkip = true;

  g.userData.sceneCollideSkip = true;
  return g;
}

/**
 * 街角居酒屋——紧凑日式小酒馆。木格栅门面 + 暖黄灯笼 + 小招牌 +
 * 门帘 + 玻璃门 + 雨棚 + 立式菜单牌 + 空调外机 + 饮料箱。
 * 位置：便利店正东侧（x 7.5..11.7, z 14.0..17.8），与便利店并排在同一条街沿线上，
 *   门面朝北对着马路/公寓；东侧入口 recess 开向便利店东边的小巷（见 IZ / ALLEY）。
 */
export function buildIzakaya(): {
  group: THREE.Group;
  /** 感应门扇所在的动效组：每帧滑，不合批、不冻结 */
  dynamic: THREE.Group;
  boxes: BoxColliderSpec[];
  /** 门扇的动态碰撞盒（随门滑动，调用方必须 concat 进 fpsColliders） */
  doorColliders: Collider[];
  /** 每帧调一次：按玩家位置驱动门的开合，并同步门扇碰撞盒 */
  update: (t: number, playerPos?: THREE.Vector3) => void;
} {
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

  /* ---- 0) 碰撞：**声明式**盒子（跟便利店 / 书店同一条路） ----
   *
   * 整组在末尾标了 `sceneCollideSkip`，所以 `buildSceneColliders` 的遍历一个盒都
   * 不收；就算不标，`districtArt.prepare` + `mergeByMaterial` 之后 mesh 已经并成
   * 「跨整店的同材质巨块」、名字也没了，按 AABB 收集只会得到一堆废盒。所以这里的
   * 碰撞必须**与几何同一处声明**，由调用方 `buildBoxColliders(boxes)` 转成世界盒。
   *
   * 两条硬约束（踩过坑，见便利店）：
   *   1. 零厚度面板（暖帘 / 短冊 / 招牌 / 光池）**绝不能**按 mesh 登记 —— 一张海报
   *      会变成加了 0.04 余量的实心盒，正好把门洞堵死。这里一律不登记它们。
   *   2. 玻璃门是 opacity 0.45 的透明材质，按「透明即跳过」的规矩它自己不会挡人，
   *      但**这扇门是关着的**（实测 z=14.005 一整片），所以门洞由下面第 1 条
   *      「北立面」整体封住，不另开门扇盒。
   *
   * 登记规则：一个「玩家会撞到的东西」一个盒，写世界区间（boxSpec）而不是中心+
   * 尺寸 —— 区间可以直接跟几何对照，中心+尺寸要心算。区间取该物的 AABB 并集。
   * 矮件（空调外机 0.42 / 饮料箱 0.20 / 植木鉢 0.33 / 傘立て 0.42）照样登记，
   * 让 fpsControls 的 STEP_UP=0.45 去决定"迈得过去" —— 门槛写在引擎里一处就够。
   */
  const boxes: BoxColliderSpec[] = [];
  /** 声明一个碰撞盒（世界区间 x0..x1 / y0..y1 / z0..z1） */
  const solid = (id: string, x0: number, x1: number, y0: number, y1: number, z0: number, z1: number) =>
    boxes.push(boxSpec(id, x0, x1, y0, y1, z0, z1));
  /** 单块实体：建几何 + 同参数登记碰撞盒（不给两处各写一份数字的机会） */
  const mass = (id: string, x: number, y: number, z: number,
                sx: number, sy: number, sz: number, mat: THREE.Material) => {
    put(gbox(sx, sy, sz), mat, x, y, z);
    solid(id, x - sx / 2, x + sx / 2, y - sy / 2, y + sy / 2, z - sz / 2, z + sz / 2);
  };

  // 便利店正东侧（IZ.x0..IZ.x1），朝北面向马路与公寓。
  // 北墙 CZ-D/2=14.0 与便利店店门线 ST.zFront=14.0 齐平，两店成同一排街沿。
  const CX = (IZ.x0 + IZ.x1) / 2, CZ = 14.0 + 3.8 / 2;  // 9.6, 15.9
  const W = 4.2, D = 3.8, H = 3.2;  // 店面尺寸
  /* 三面墙的**顶面比天花板低 3cm**。
   * 天花板 `gbox(W,0.08,D)` 中心 H-0.04 ⇒ 顶面正好 y=H=3.2；三面墙原来也顶到 H，
   * 于是南墙顶、东西墙顶、天花板顶**四个面在 y=3.2 同轴同向共面**。共面扫描实测：
   * 南墙×天花板 0.150 m² ×2 + 东西墙×天花板 0.107 m² ×2 = **0.51 m² 显著共面**，
   * 从公寓 4F 阳台俯看居酒屋屋顶会整片闪。
   * 压低 3cm 后墙顶（3.17）埋进天花板（3.12..3.20）里，可见性探针判为「不可见」，
   * 屋顶只剩天花板一块干净平板。墙高只影响自身碰撞盒顶，玩家头顶 1.7 碰不到。 */
  const WH = H - 0.03;              // 墙高（顶面 3.17，埋在天花板板厚之内）

  // ---- 1) 结构：三面墙（西/南/东）+ 北面（格栅 + 玻璃门） ----
  // 后壁（南侧）
  mass('izakaya-wall-south', CX, WH / 2, CZ + D / 2, W, WH, 0.12, IM.wood);
  // 左侧壁（西侧）
  mass('izakaya-wall-west', CX - W / 2, WH / 2, CZ, 0.10, WH, D, IM.woodDeep);
  /* 右侧壁（东侧）：**必须是整进深**。
   * 原来这里只有 `D * 0.55`（z 14.02..16.11），南边 1.69m 是敞的 —— 水平射线实测
   * 从巷子 (13.6, 1.5, 17.0) 朝西打能一路穿到店内西墙 x=7.55，也就是玩家可以从
   * 巷子直接走进店里；而 9-6 的厨房调理台（x 11.10..11.64）正是贴着这面墙摆的，
   * 厨房对公众巷子敞开显然不是设计意图。补满到整进深。 */
  mass('izakaya-wall-east', CX + W / 2, WH / 2, CZ, 0.10, WH, D, IM.woodDeep);
  // 天花板（在头顶之上，不登记）
  put(gbox(W, 0.08, D), IM.wood, CX, H - 0.04, CZ);
  // 地板（顶面 0.06，低于脚踝门槛 footY+0.12，登记了也会被跳过 —— 干脆不登记）
  put(gbox(W, 0.06, D), IM.floor, CX, 0.03, CZ);

  /* 北立面**分三段**登记，中间给门洞让路。
   *
   * 这里不是墙而是 8 根木格栅（x = 7.65 + 0.48i，宽 0.04）+ 右端一扇门：
   *   ・格栅之间是 0.44m 的缝，玩家直径 0.30 < 0.44 —— 侧着身能挤进店里。
   *     实测：从 z=12.80 朝 +Z 打，x=8.4 / 10.3 那两枪直接穿到南墙 z=17.74。
   *   ・所以除门洞外的整面必须封死（连 0.44m 的缝一起）。
   *
   * 原来这里是**一整面**（连门洞一起封），结果玻璃门永远打不开、玩家永远进不去
   * —— 2026-09-21 用户报「居酒屋没有门无法进入」。现在拆成「门洞西 / 门洞东 /
   * 门楣」三条，门洞 x 10.60..11.60 净空（玩家半径 0.15 ⇒ 有 0.70m 可通行宽度）。
   *
   * z 取 13.96..14.08：盖住最靠外的门框外皮(13.965)与最靠里的暖帘背面(14.075)。 */
  const doorW = 1.0, doorH = 2.2;
  const doorCx = CX + W / 2 - 0.6;                  // 11.10
  const DOOR_X0 = doorCx - doorW / 2;               // 10.60
  const DOOR_X1 = doorCx + doorW / 2;               // 11.60
  const FN_Z0 = CZ - D / 2 - 0.04, FN_Z1 = CZ - D / 2 + 0.08;   // 13.96..14.08
  solid('izakaya-facade-north-w', CX - W / 2 - 0.05, DOOR_X0, 0, H, FN_Z0, FN_Z1);
  solid('izakaya-facade-north-e', DOOR_X1, CX + W / 2 + 0.05, 0, H, FN_Z0, FN_Z1);
  // 门楣：门洞上方照旧封住（玩家头顶 1.7 < doorH 2.2，不影响通行）
  solid('izakaya-facade-north-head', DOOR_X0, DOOR_X1, doorH, H, FN_Z0, FN_Z1);

  // ---- 2) 木格栅门面（北面，朝公寓方向） ----
  // 门洞那一格要跳过：i=7 落在 x=11.01，正好杵在门洞正中（原来是一堵墙看不出来，
  // 现在门洞开了，那根木条会变成门中间的一根柱子）。
  for (let i = 0; i < 8; i++) {
    const bx = CX - W / 2 + 0.15 + i * 0.48;
    if (bx > DOOR_X0 - 0.06 && bx < DOOR_X1 + 0.06) continue;
    put(gbox(0.04, H * 0.75, 0.04), IM.woodDeep, bx, H * 0.42, CZ - D / 2 + 0.02);
  }

  /* ---- 3) 感应推拉门（门扇走**外轨**）----
   *
   * 门扇滑轨放在 z 13.92（格栅外皮 14.00 之外 8cm）。为什么不走内轨：门扇要向西
   * 滑 1.06m 让开门洞，而内轨上依次横着格栅（z 14.00..14.04）与暖帘（z 14.045..
   * 14.075），滑过去必然穿模；外轨前面是空的。
   *
   * **原来的静态玻璃片删掉了**：留着就是「双重的门」（便利店那次踩过同样的坑）。
   * 现在门洞只有这一扇会动的门扇，由调用方 `izakaya.update()` 按玩家距离驱动。
   *
   * 门扇上的不透明杆件一律标 `noCollide`：`buildSceneColliders` 只跳过 opacity<0.5
   * 的材质，玻璃会跳过但这些框条不会 —— 每根框条会变成一根杵在门洞里的静态盒。
   * 门扇的碰撞由随门滑动的动态盒（doorColliders）承担。
   */
  const dyn = new THREE.Group();
  dyn.name = 'izakaya-dynamic';
  const doorLeaf = new THREE.Group();
  doorLeaf.name = 'izakaya-door-leaf';
  const LEAF_W = 1.04, LEAF_Z = CZ - D / 2 - 0.08;   // 13.92
  {
    const glass = new THREE.Mesh(gbox(LEAF_W - 0.10, doorH - 0.14, 0.02), IM.glass);
    glass.position.set(0, doorH / 2, 0);
    doorLeaf.add(glass);
    for (const dx of [-LEAF_W / 2 + 0.03, LEAF_W / 2 - 0.03]) {
      const bar = new THREE.Mesh(gbox(0.05, doorH, 0.05), IM.frame);
      bar.position.set(dx, doorH / 2, 0);
      bar.userData.noCollide = true;
      doorLeaf.add(bar);
    }
    for (const dy of [0.05, doorH - 0.05]) {
      const rail = new THREE.Mesh(gbox(LEAF_W, 0.06, 0.05), IM.frame);
      rail.position.set(0, dy, 0);
      rail.userData.noCollide = true;
      doorLeaf.add(rail);
    }
    const handle = new THREE.Mesh(gbox(0.04, 0.5, 0.04), IM.metal);
    handle.position.set(LEAF_W / 2 - 0.14, 1.05, -0.045);
    handle.userData.noCollide = true;
    doorLeaf.add(handle);
  }
  doorLeaf.position.set(doorCx, 0, LEAF_Z);
  /* 门扇自己当一次描边的 root：它挂在 dyn 上、不参与合批，挂在门框组下会被
   * 烘焙成固定几何，门一滑开描边留在原地（便利店那两个 leaf 就是这么处理的）。 */
  outlineProp(doorLeaf);
  dyn.add(doorLeaf);

  // 门框（门楣 + 西侧门樘；东侧由东墙内表面充当门樘）
  put(gbox(doorW + 0.06, 0.06, 0.05), IM.frame, doorCx, doorH, CZ - D / 2 - 0.01);
  put(gbox(0.05, doorH, 0.05), IM.frame, doorCx - doorW / 2 - 0.02, doorH / 2, CZ - D / 2 - 0.01);

  /* 门扇的动态碰撞盒：随门滑动，必须由调用方 concat 进 fpsColliders。
   * y 取 0..doorH（贴地到门扇顶），z 取轨面 ±3.5cm。 */
  const doorColliders: Collider[] = [{
    min: new THREE.Vector3(doorCx - LEAF_W / 2, 0, LEAF_Z - 0.035),
    max: new THREE.Vector3(doorCx + LEAF_W / 2, doorH, LEAF_Z + 0.035),
    kind: 'wall',
    source: 'izakaya-door-leaf',
  }];

  /* 感应门状态量。唯一真状态是 doorOpenAmt（0..1），纯插值推进 —— 暂停再恢复不会
   * 卡在半开；doorHold 是「人已离开、门还没关」的余量，防阈值抖动。
   * 四条与便利店同源：①位置驱动 ②关门滞后 ③dt 夹 0.25 ④动态盒每帧同步。 */
  let doorOpenAmt = 0, doorHold = 0, doorLastT = 0;
  const DOOR_SENSE = 1.8;    // 感应半径（米）：门中心到玩家的水平距离
  const DOOR_SLIDE = 1.06;   // 全开时的西移量（门洞净宽 1.0 + 6cm 重叠）
  const syncDoorCollider = () => {
    const cx = doorLeaf.position.x;
    doorColliders[0].min.x = cx - LEAF_W / 2;
    doorColliders[0].max.x = cx + LEAF_W / 2;
  };
  syncDoorCollider();

  const update = (t: number, playerPos?: THREE.Vector3) => {
    /* dt 夹到 0.25（4fps）而不是更小：夹太紧的话低帧率下门会开得极慢
     * （headless 下 rAF 被节流到 ~1fps）。上界仍然必要 —— 标签页切回来时 t 会跳
     * 一大截，不夹就会瞬间弹开。 */
    const dt = Math.min(0.25, Math.max(0, t - doorLastT));
    doorLastT = t;
    const pdx = playerPos ? playerPos.x - doorCx : 1e3;
    const pdz = playerPos ? playerPos.z - LEAF_Z : 1e3;
    const nearDoor = pdx * pdx + pdz * pdz < DOOR_SENSE * DOOR_SENSE;
    doorHold = nearDoor ? 1.2 : Math.max(0, doorHold - dt);
    const wantOpen = nearDoor || doorHold > 0 ? 1 : 0;
    // 开 0.55s、关 0.85s 全行程：开得快、关得慢，像还在等传感器
    const dStep = dt / (wantOpen ? 0.55 : 0.85);
    const dDiff = wantOpen - doorOpenAmt;
    if (Math.abs(dDiff) <= dStep) doorOpenAmt = wantOpen;
    else doorOpenAmt += Math.sign(dDiff) * dStep;
    doorLeaf.position.x = doorCx - doorOpenAmt * DOOR_SLIDE;
    syncDoorCollider();
  };

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
  izSign.position.set(CX, H + 0.25, CZ - D / 2 - 0.03);
  /* 牌面必须朝 **-Z**（朝街、朝公寓方向）。
   * 原来这里没旋转，注释还写着「法线默认 +Z，朝北面向公寓」—— 但本项目 +Z 是**南**
   * （见公寓剖面注释「+Z 北→南」），所以那块牌实际朝店内；而 `emissive()` 默认
   * `FrontSide` ⇒ 从街上根本看不见（背面剔除），只有绕到南侧巷子里才读得到字。
   * 2026-09-21 用户报「居酒屋的牌匾的南北朝向反了」。 */
  izSign.rotation.y = Math.PI;
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
    /* 立式菜单牌：板 0.45×0.85 绕 Y 转 -0.2π，水平投影 0.364×0.264，
     * 中心 (12.30, 17.00) → x 12.118..12.482 / z 16.868..17.132；支架在
     * (12.10, 17.00)±0.015。取并集 x 12.08..12.49 / z 16.86..17.14。 */
    solid('izakaya-menu-board', 12.08, 12.49, 0, boardH + 0.02, 16.86, 17.14);
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
  // 傘立て（0.42 高，登记真高 —— 迈不迈得过去交给 STEP_UP=0.45 判）
  solid('izakaya-umbrella-stand', 11.44 - 0.09, 11.44 + 0.09, 0.06, 0.48,
    FZ + 0.30 - 0.09, FZ + 0.30 + 0.09);

  // 9-2 客席吧台（L 型：正面一段 + 西端往里折一段）
  const CT_Z = FZ + 1.15;         // 15.15
  const CT_TOP = FL + 0.95;       // 1.01 天板中心
  mass('izakaya-counter-front', 8.90, FL + 0.46, CT_Z, 2.30, 0.92, 0.55, IM.wood);
  mass('izakaya-counter-front-top', 8.90, CT_TOP, CT_Z, 2.46, 0.05, 0.70, IM.warmPale);
  for (let i = 0; i < 9; i++) {   // 台面下的竖木条腰板
    put(gbox(0.025, 0.80, 0.02), IM.woodDeep, 7.80 + i * 0.275, FL + 0.43, CT_Z - 0.29);
  }
  mass('izakaya-counter-west', 7.85, FL + 0.46, CT_Z + 0.71, 0.55, 0.92, 0.92, IM.wood);
  mass('izakaya-counter-west-top', 7.85, CT_TOP + 0.02, CT_Z + 0.73, 0.72, 0.05, 1.00, IM.warmPale);

  // 9-3 吧凳 × 3（座面 + 支柱 + 底盘 + 脚踏圈）
  for (let i = 0; i < 3; i++) {
    const sx = 8.20 + i * 0.75, sz = FZ + 0.62;
    put(cyl(0.16, 0.16, 0.05, 10), IM.wood, sx, 0.635, sz);
    put(cyl(0.028, 0.032, 0.58, 6), IM.metal, sx, 0.35, sz);
    put(cyl(0.13, 0.14, 0.03, 10), IM.metal, sx, 0.10, sz);
    put(cyl(0.115, 0.115, 0.018, 10), IM.metal, sx, 0.24, sz);
    // 座面 φ0.32 / 底盘 φ0.28 / 支柱 —— 一件一个外接盒（y 0.085..0.66，0.575 > STEP_UP，挡人）
    solid(`izakaya-stool-${i + 1}`, sx - 0.16, sx + 0.16, 0.085, 0.66, sz - 0.16, sz + 0.16);
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
    mass('izakaya-kitchen-counter', 11.36, FL + 0.425, KZ, 0.52, 0.85, 1.90, IM.wood);
    mass('izakaya-kitchen-counter-top', 11.36, FL + 0.87, KZ, 0.56, 0.04, 1.94, IM.metal);
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
    mass('izakaya-liquor-cabinet', 9.80, FL + 0.425, BB_Z, 2.90, 0.85, 0.34, IM.wood);
    mass('izakaya-liquor-cabinet-top', 9.80, FL + 0.87, BB_Z, 3.00, 0.04, 0.40, IM.warmPale);
    // 上面两层瓶架（y 1.42 / 1.86）在头高，但地柜把玩家挡在 z≤17.23，够不到 —— 不登记
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
  mass('izakaya-fridge', 7.88, 0.81, 17.40, 0.50, 1.50, 0.52, IM.frame);
  mass('izakaya-fridge-door', 7.88, 0.81, 17.13, 0.42, 1.38, 0.02, IM.metal);  // 扉
  put(gbox(0.03, 0.24, 0.03), IM.metal, 8.06, 0.86, 17.11);        // 把手（3cm，在门的盒内）
  // 酒樽：樽身 φ0.44（y 0.12..0.62）+ 上下两道箍 φ0.452 —— 一件一个外接盒
  put(cyl(0.22, 0.22, 0.50, 12), IM.wood, 8.60, 0.37, 16.80);
  put(cyl(0.226, 0.226, 0.04, 12), IM.woodDeep, 8.60, 0.20, 16.80);
  put(cyl(0.226, 0.226, 0.04, 12), IM.woodDeep, 8.60, 0.56, 16.80);
  solid('izakaya-barrel-indoor', 8.60 - 0.226, 8.60 + 0.226, 0.12, 0.62,
    16.80 - 0.226, 16.80 + 0.226);
  for (let i = 0; i < 2; i++) {                                     // ビールケース
    put(gbox(0.34, 0.24, 0.26), IM.red, 10.55, 0.18 + i * 0.26, 16.85);
  }
  // 两层叠起来 y 0.06..0.56（0.50 > STEP_UP，挡人）
  solid('izakaya-beer-cases', 10.55 - 0.17, 10.55 + 0.17, 0.06, 0.56, 16.85 - 0.13, 16.85 + 0.13);

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
  // 空调外机（0.42 高：登记真高，迈得过去）
  mass('izakaya-ac-unit', CX - W / 2 - 0.35, 0.21, CZ - D / 2 + 0.8, 0.60, 0.42, 0.48, IM.slabDark);
  // 饮料箱（0.20 高，两个沿 z 排）
  for (let i = 0; i < 2; i++) {
    put(gbox(0.28, 0.20, 0.22), IM.cool, CX + W / 2 + 0.25, 0.10, CZ + 0.5 + i * 0.30);
  }
  solid('izakaya-drink-crates', CX + W / 2 + 0.11, CX + W / 2 + 0.39, 0, 0.20, CZ + 0.39, CZ + 0.91);
  // 垃圾桶（0.50 > STEP_UP，挡人）
  mass('izakaya-trash-bin', CX + W / 2 + 0.5, 0.25, CZ + D / 2 - 0.6, 0.30, 0.50, 0.30, IM.slabDark);
  // 店先の酒樽 + 植木鉢（居酒屋的门面记号）
  put(cyl(0.20, 0.20, 0.50, 12), IM.wood, 8.35, 0.31, FZ - 0.42);
  put(cyl(0.206, 0.206, 0.05, 12), IM.woodDeep, 8.35, 0.20, FZ - 0.42);
  put(cyl(0.206, 0.206, 0.05, 12), IM.woodDeep, 8.35, 0.46, FZ - 0.42);
  // 樽身 φ0.40（y 0.06..0.56）+ 两道箍 φ0.412 —— 0.50 > STEP_UP，挡人
  solid('izakaya-barrel-front', 8.35 - 0.206, 8.35 + 0.206, 0.06, 0.56,
    FZ - 0.42 - 0.206, FZ - 0.42 + 0.206);
  put(cyl(0.13, 0.16, 0.24, 8), IM.woodDeep, 7.70, 0.12, FZ - 0.30);
  put(cyl(0.20, 0.20, 0.06, 8), IM.green, 7.70, 0.30, FZ - 0.30);
  // 植木鉢：鉢 φ0.32 + 植栽盘 φ0.40 → 外接盒 r 0.20 / y 0..0.33（0.33 ≤ STEP_UP，迈得过去）
  solid('izakaya-planter', 7.70 - 0.20, 7.70 + 0.20, 0, 0.33, FZ - 0.30 - 0.20, FZ - 0.30 + 0.20);

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
  // 同便利店：抬到 0.036 压住人行道面(0.028)与盲道条(0.035)，否则一半埋在路面下
  izPool.position.set(CX, 0.036, FZ - 1.5);
  izPool.renderOrder = 2;
  izPool.name = 'izakaya-light-pool';
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

  /* 整组仍标 sceneCollideSkip：碰撞是**声明式**的（见函数开头的 boxes），遍历本来
   * 就一个盒都不该收 —— 暖帘 / 短冊 / 招牌 / 光池都是零厚度平板，按 mesh 收进来会
   * 生成加余量的实心盒，正好把门洞堵死。与书店（buildStreetBookStore）同一套做法。 */
  g.userData.sceneCollideSkip = true;
  /* dynamic：感应门扇。每帧滑，所以**不能**合批（会被烘焙成固定几何）也不能冻结。
   * doorColliders：门扇的动态盒，随门滑动，调用方必须 concat 进 fpsColliders
   * 并每帧调 update()，否则门开了玩家还是撞空气（便利店那条路一模一样）。 */
  return { group: g, dynamic: dyn, boxes, doorColliders, update };
}

/* ============================================================================
 * 书店内饰用的两张贴图（书脊带 / 海报）
 * ========================================================================== */

/**
 * 书脊带：一格一格是密排的竖书脊。
 *
 * 和便利店 goodsStripTexture 同一个道理——隔着玻璃看，逐本建模和一条纹理
 * 肉眼无差，顶点数差两个量级。但书的排布规律和商品**相反**：商品是宽窄不一、
 * 高低不齐的包装块；书脊是**等宽等高**的密排竖条，只有少量横放的平装本打破
 * 节奏。照商品那样画，出来是一排货架、不是一架子书。
 *
 * **为什么拼成图集**：书架一共 20 条书脊带。一条一个 256×32 纹理就是 20 个
 * 材质、20 次 draw call，而且 mergeByMaterial 是按**材质引用**分桶的——材质
 * 各不相同，20 条一条都合不了。全部画进同一张 256×768 之后 20 条共享一个
 * 材质，合批直接收成 1 个 mesh。像素总量不变（20×256×32 = 256×640），代价
 * 只是 UV 里多一次换算。
 */
const BOOK_SPINE_ROW_PX = 32;
/**
 * 图集行数。当前用到 20 行（两排双向书架各 8 条 + 后墙一排 4 条），留 4 行余量。
 * 真超了按行号取模复用：多出来的书架跟别人共用一条书脊花纹，隔着玻璃看不出来，
 * 总好过整个书店渲染不出来。
 */
const BOOK_SPINE_ROWS = 24;

let bookSpineTex: THREE.CanvasTexture | null = null;
let bookSpineCtx: CanvasRenderingContext2D | null = null;
const bookSpineRowOf = new Map<number, number>();

function bookSpineAtlas(): THREE.CanvasTexture {
  if (!bookSpineTex || !bookSpineCtx) {
    const { canvas, ctx } = makeCanvas(256, BOOK_SPINE_ROW_PX * BOOK_SPINE_ROWS);
    bookSpineCtx = ctx;
    bookSpineTex = toTexture(canvas);
  }
  return bookSpineTex;
}

/** 把第 row 行的书脊带画进图集（内部按 0..32 的局部坐标画，靠 translate 落行）。 */
function drawBookSpineRow(ctx: CanvasRenderingContext2D, row: number, seed: number) {
  const rnd = makeRng(seed);
  ctx.save();
  ctx.translate(0, row * BOOK_SPINE_ROW_PX);
  ctx.fillStyle = '#241f1b';                    // 架内阴影：近黑，书脊才跳得出来
  ctx.fillRect(0, 0, 256, BOOK_SPINE_ROW_PX);
  // 旧书：低饱和，但比便利店包装亮一档（书脊是纸与布，不是塑料）
  const palette = ['#b8a98e', '#8f7f6a', '#a8998a', '#7d8b96', '#9c8f7e', '#b0a392',
    '#6f7d84', '#a3937c', '#8a8f7a', '#c2b6a0', '#7a6f66', '#93a0a6'];
  let x = 1;
  while (x < 255) {
    const w = 3 + Math.floor(rnd() * 4);        // 3~6px：等宽书脊
    const h = 26 + Math.floor(rnd() * 4);       // 高度基本齐平
    ctx.fillStyle = palette[Math.floor(rnd() * palette.length)];
    ctx.fillRect(x, 32 - h, w, h);
    if (rnd() > 0.4) {                          // 书脊上的烫印 / 书名带
      ctx.fillStyle = 'rgba(255,255,255,0.28)';
      ctx.fillRect(x, 32 - h + 5 + Math.floor(rnd() * 4), w, 1);
    }
    x += w + 1;
  }
  for (let n = 0; n < 3; n++) {                 // 几本横放的平装本
    const w = 14 + Math.floor(rnd() * 10);
    ctx.fillStyle = palette[Math.floor(rnd() * palette.length)];
    ctx.fillRect(Math.floor(rnd() * (250 - w)), 30, w, 2);
  }
  ctx.restore();
}

/** 取（必要时新建）某条 seed 的书脊在图集里的行号，并把这一行画出来。 */
function bookSpineRow(seed: number): number {
  const hit = bookSpineRowOf.get(seed);
  if (hit !== undefined) return hit;
  const atlas = bookSpineAtlas();
  const row = bookSpineRowOf.size % BOOK_SPINE_ROWS;
  drawBookSpineRow(bookSpineCtx!, row, seed);
  // 整本书店在首帧之前就画完，这一次标记足够；重复标记也无害。
  atlas.needsUpdate = true;
  bookSpineRowOf.set(seed, row);
  return row;
}

/** 书脊材质：整本书店只有一份，20 条书脊带共用 → 合批成 1 个 mesh。 */
let bookSpineMat: THREE.MeshToonMaterial | null = null;
function bookSpineMaterial(): THREE.MeshToonMaterial {
  if (!bookSpineMat) {
    bookSpineMat = toon('#ffffff', {
      map: bookSpineAtlas(),
      emissive: '#5a4a3a', emissiveIntensity: 0.45,   // 顶灯下的纸面，别让层板间死黑
    });
  }
  return bookSpineMat;
}

/**
 * 一条书脊带的平面：UV 的 v 轴压到图集的第 row 行。
 *
 * 材质是共享的，行信息只能烘进几何——一条就 4 个顶点，代价可以忽略。
 * 每次都新建（不缓存）：合批收尾会 dispose 掉源几何，缓存下来会留一批
 * 已经释放的实例给下一次重建。
 */
function bookStripGeometry(row: number): THREE.BufferGeometry {
  const geo = new THREE.PlaneGeometry(1, 1);
  const uv = geo.attributes.uv as THREE.BufferAttribute;
  // toTexture 出来的贴图 flipY=true：canvas 顶行 → v=1。第 row 行占
  // v ∈ [(rows-1-row)/rows, (rows-row)/rows]，所以局部 v=0 是这行的**底**边。
  for (let i = 0; i < uv.count; i++) {
    uv.setY(i, (BOOK_SPINE_ROWS - 1 - row + uv.getY(i)) / BOOK_SPINE_ROWS);
  }
  uv.needsUpdate = true;
  return geo;
}

/**
 * 店内海报：米白纸 + 朱色标题条 + 几行细字。
 * 细字只用短横线示意——隔着 8m 街 + 雨幕，有字和没字才是差别。
 */
const posterCache = new Map<string, THREE.Texture>();
function bookPosterTexture(title: string, seed: number): THREE.Texture {
  const hit = posterCache.get(title);
  if (hit) return hit;
  const { canvas, ctx } = makeCanvas(96, 132);
  const rnd = makeRng(seed);
  ctx.fillStyle = '#efe8d8';
  ctx.fillRect(0, 0, 96, 132);
  ctx.fillStyle = '#7a2f28';                    // 朱色标题条
  ctx.fillRect(0, 0, 96, 34);
  ctx.fillStyle = '#f2ede2';
  ctx.font = 'bold 22px sans-serif';
  ctx.textAlign = 'center';
  ctx.fillText(title, 48, 25);
  ctx.fillStyle = 'rgba(70,60,50,0.55)';
  for (let i = 0; i < 7; i++) {
    ctx.fillRect(12, 48 + i * 11, 30 + Math.floor(rnd() * 42), 3);
  }
  ctx.fillStyle = '#3a4a5a';
  ctx.fillRect(12, 118, 72, 2);
  const tex = toTexture(canvas);
  posterCache.set(title, tex);
  return tex;
}

/**
 * 四层书店「書泉堂書店」——近景高模临街铺，就位在居酒屋东侧的巷位（中景楼后撤后，
 * 东区最西一栋原让出的位置正好由它接上，居酒屋往东第一家就是它）。
 * 学园都市（原型多摩市）式学生街一层店面：大玻璃橱窗 + 遮阳篷 + 发光店招，
 * 上部三层是宿舍感的亮窗。与地铁站/便利店/居酒屋一起构成公寓这条街的高模临街排面。
 *
 * 1F 是**可进入的**真营业厅（书架/陈列台/柜台/敞开的大门），碰撞盒随几何一并
 * 交出（声明式，见函数内的 boxes）——玩家能从街面走进来。
 *
 * 体量：7.0（面宽）× 11.1（进深）× 12.4m（4 层）。四面退让依据写在
 * 「位置与体量」那段注释里（北推人行道南缘、南推到路缘石前、东被东邻卡死 0.32m、
 * 西让居酒屋 1.0m 巷）。**改这里任何一个数之前先读那段注释。**
 */
export function buildStreetBookStore(): { group: THREE.Group; boxes: BoxColliderSpec[] } {
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
    /**
     * 橱窗玻璃：近全透明（便利店橱窗同款 0.085 + depthWrite false）。
     * 通用 glassTexture 是深蓝渐变，当年配「玻璃后面一块假暖光板」正好；
     * 现在后面是真书架，同一层幕会把书脊压成一团灰。
     */
    shopGlass: toon('#cfe0ee', { finish: 'glass', transparent: true, opacity: 0.085, depthWrite: false }),
    cool: emissive('#dceaf5'),
    warm: emissive('#ffe4b5'),                   // 店内暖光
  };
  /** 封面 / 书脊的几种布面与纸面颜色（低饱和，别把夜景色温拽跑）。 */
  const COVER = ['#8f3b34', '#2f4f6d', '#4a6b4a', '#a8763a', '#6b4a6d', '#3f5f66', '#8a7a4e', '#b0a08c'];

  /* ================= 位置与体量（2026-09 扩建）=================
   *
   * 旧值：面宽 7.0 / 进深 6.0 / 两层 6.6m，北墙 z=18.0。四个面都量过之后
   * 改成 **7.0 × 11.1 / 四层 12.4m**，北墙 z=15.0。每一面的依据：
   *
   *   北 z=15.0   人行道（pavement 12.6..15.0）的南边线。旧值 18.0 把 3m 的人行
   *               道前场空着、店面缩在街后 4m —— 从街上一眼就读成"夹在缝里的小屋"。
   *               北面 2.1m 外是路灯 (14, 12.9)（STORE_LAMP_X 那排），雨篷北挑到
   *               z=13.95 也够不着它。
   *   南 z=26.1   路缘石（planned-street-network，z=26.62）以北 0.52m。旧值 24.0
   *               白扔 2.5m 进深。
   *   西 x=12.7   居酒屋东墙 IZ.x1=11.7，留 1.0m 巷子。扩建后巷子只剩 z 14..15
   *               的一个 1.0×1.0 凹口 —— 两栋楼由此读作"贴建"，是密集商业街常态。
   *   东 x=19.7   东邻 parcel-23.5-19.6（city-block-41）西墙在 x=20.02..20.10，
   *               只有 0.32m。**东向不可扩**（逐顶点侵入量过）。
   *   高 12.4m    1F 营业厅 3.4（吊顶 3.0）+ 3×3.0。旧值 2 层 6.6m 比同排的
   *               4 层 11.2m / 5 层 14.0m 矮一大截，这才是"太小"的主因；
   *               向上 40m 内零遮挡（量过），加层是零风险方向。
   *
   * 占地 42.0 → 77.7 m²（+85%），营业厅 29.6 → 60.5 m²（×2.0）。
   *
   * 顺带修掉两件旧账：
   *   1. 旧北墙 z=18.0 正好压在人行道（12.6..15.0）与车道（7.6..12.6）的分界线上，
   *      本身就是个 bug；现在落在人行道南缘。
   *   2. 便利店拉过巷子的两根电线（exterior.ts awire，终点写死 x=12.7）在旧体量下
   *      其实是**悬空**的（旧西墙从 z=18 才起）。现在西墙 z 15..26.1，电线真挂上了。
   */
  const CX = 16.2, W = 7.0;
  const zN = 15.0;                    // 临街面（北）
  const D = 11.1;                     // 进深 → z 15.0..26.1
  const CZ = zN + D / 2;              // 20.55
  const FLOORS = 4;                   // 层数（含 1F）
  const H1 = 3.4;                     // 1F 营业厅
  const HF = 3.0;                     // 标准层高
  const H2 = HF * (FLOORS - 1);       // 上部总高 9.0
  const HT = H1 + H2;                 // 12.4

  // 结构：每层腰线 + 平屋顶（1F 不再是实心盒，见下）
  put(gbox(W, H2, D), BM.wall, CX, H1 + H2 / 2, CZ);
  for (let f = 1; f < FLOORS; f++) {
    put(gbox(W + 0.06, 0.10, D + 0.06), BM.trim, CX, H1 + (f - 1) * HF + 0.05, CZ);
  }
  put(gbox(W + 0.4, 0.22, D + 0.4), BM.trim, CX, HT + 0.11, CZ);        // 屋檐
  put(gbox(W - 0.3, 0.55, D - 0.3), BM.dark, CX, HT + 0.28 + 0.27, CZ); // 屋顶压顶围墙

  /* 屋顶设备：12.4m 的顶若只剩一块 7×11 的死板，从街上一眼看出是"纸盒"。
   * 楼梯间出屋面 + 两台冷凝器 + 一根排气管，只改轮廓线，不碰任何邻居
   * （屋顶压顶顶面 13.23，往上 40m 量过是空的）。 */
  const roofY = HT + 0.28 + 0.55;                                       // 13.23
  put(gbox(2.2, 1.5, 2.0), BM.wall, CX - W * 0.22, roofY + 0.75, CZ + D * 0.20);
  put(gbox(2.34, 0.12, 2.14), BM.trim, CX - W * 0.22, roofY + 1.56, CZ + D * 0.20);
  for (const dz of [-D * 0.26, -D * 0.10]) {
    put(gbox(0.9, 0.7, 0.5), BM.dark, CX + W * 0.26, roofY + 0.35, CZ + dz);
  }
  put(new THREE.CylinderGeometry(0.12, 0.12, 0.9, 8), BM.dark, CX + W * 0.04, roofY + 0.45, CZ + D * 0.32);

  /* ================= 1F 剖面：临街面 → 营业厅 → 后段实心 =================
   *
   * 旧版 1F 是**整块实心体量**（`gbox(W,H1,D)`），玻璃后面贴一块暖光板冒充店内。
   * 玩家能从街上走到这里（外置钢梯下到街面，街面是一整张 floor 碰撞盒），所以
   * 这里挖成真营业厅：地板 / 吊顶 / 两侧内墙 / 前墙三段墙垛，中间 4.4m 进深摆
   * 书架、陈列台、柜台。只做「看得见的那一面」的话，一进门就是穿帮。
   *
   * 前墙不另起一层皮：三段墙垛 + 窗楣 + 门楣正好把立面补满，窗与门两个洞留在
   * 原位（沿用旧值 zF = zN - 0.08，橱窗本来就是微微挑出楼线的）。
   *
   * 碰撞走**声明式盒子**（与几何同一行同步登记，两边不许各写一份数字），不走
   * buildSceneColliders 遍历：橱窗玻璃是 0.085 透明度的近全透明材质，遍历会按
   * 「transparent && opacity < 0.5」整块跳过，橱窗就挡不住人了。
   */
  const ID = 9.0;                   // 营业厅进深（z 15.0 → 24.0，占掉 11.1m 里的 9.0）
  const IH = 3.0;                   // 吊顶高（H1=3.4，顶上 0.4 是 2F 楼板）
  const FW = 0.14;                  // 内墙厚
  const FP = 0.12;                  // 临街前墙厚
  const zB = zN + ID;               // 店内后墙 24.0
  // 前墙中心与 2F 楼体齐平（zN）。玻璃 / 门窗框另给一个 zG：嵌在墙厚之内、
  // 比墙外表面凹 5cm——窗框和门框再各自往外挑一点，立面才有层次。
  const zW = zN;
  const zG = zN - 0.01;
  const WX0 = CX - W / 2, WX1 = CX + W / 2;      // 12.7 / 19.7
  // 洞口不再按 W 的百分比推：7m 面宽下百分比会把西端垛压到 0.315m（太薄，
  // 从街上读不出"柱"），改直接写死，三段墙垛 0.45 / 0.85 / 0.90 更匀。
  const GX0 = 13.15, GX1 = 17.05;   // 橱窗洞口 3.90m
  const DX0 = 17.90, DX1 = 18.80;   // 门洞 0.90m
  const GY1 = 2.90, DY1 = 2.45;     // 窗顶 / 门顶（1F 加高到 3.4，门窗跟着抬高）
  const boxes: BoxColliderSpec[] = [];
  const solid = (id: string, x: number, y: number, z: number, sx: number, sy: number, sz: number) =>
    boxes.push({ id, pos: [x, y, z], size: [sx, sy, sz] });
  /** 实体块：建几何 + 同步登记碰撞盒。 */
  const mass = (id: string, x: number, y: number, z: number, sx: number, sy: number, sz: number, mat: THREE.Material) => {
    put(gbox(sx, sy, sz), mat, x, y, z);
    solid(id, x, y, z, sx, sy, sz);
  };

  const IM = {
    wall: toon('#cfc7b6'),                        // 店内墙：比外墙暖一档的米白
    ceil: toon('#c6bdac'),
    floor: toon('#8a6f52', { finish: 'soft' }),   // 木地板
    wood: toon('#7d6247'),                        // 书架木作
    woodDark: toon('#5b4634'),
    board: toon('#6b543d'),
    metal: toon('#7a828c', { finish: 'metal' }),
    dark: toon('#2f2a26'),
    ceilLight: emissive('#fff2d8', { side: THREE.DoubleSide }),   // 吊顶灯带（DoubleSide：横放的平面要能两面读）
  };

  // 后段实心体量：撑住 2F，同时就是店内的后墙
  mass('bookstore-back-mass', CX, H1 / 2, (zB + CZ + D / 2) / 2, W, H1, CZ + D / 2 - zB, BM.wall);
  /* 吊顶（= 2F 楼板底面）与两侧内墙的 z 从**前墙内表面**（zIn = zN + FP/2）起，
   * 不从前墙中心起。旧版取 `zN + ID/2 - 0.04`（z 14.92..24.0），于是西内墙的外皮、
   * 吊顶的外皮与前墙垛的外皮**都在 x=12.70、法线同向**，重叠 0.272 m² —— 从巷子里
   * 看西墙会闪（东墙同理，x=19.70）。改成与前墙垛对接（butt joint），共面面积归零。 */
  const zIn = zN + FP / 2;          // 前墙内表面 15.06
  put(gbox(W, H1 - IH, zB - zIn), IM.ceil, CX, IH + (H1 - IH) / 2, (zIn + zB) / 2);
  mass('bookstore-wall-w', WX0 + FW / 2, IH / 2, (zIn + zB) / 2, FW, IH, zB - zIn, IM.wall);
  mass('bookstore-wall-e', WX1 - FW / 2, IH / 2, (zIn + zB) / 2, FW, IH, zB - zIn, IM.wall);
  // 临街前墙：三段墙垛（窗与门之间那块是墙，不是玻璃）
  mass('bookstore-pier-w', (WX0 + GX0) / 2, H1 / 2, zW, GX0 - WX0, H1, FP, BM.wall);
  mass('bookstore-pier-m', (GX1 + DX0) / 2, H1 / 2, zW, DX0 - GX1, H1, FP, BM.wall);
  mass('bookstore-pier-e', (DX1 + WX1) / 2, H1 / 2, zW, WX1 - DX1, H1, FP, BM.wall);
  put(gbox(GX1 - GX0, H1 - GY1, FP), BM.wall, (GX0 + GX1) / 2, (GY1 + H1) / 2, zW);  // 窗楣
  put(gbox(DX1 - DX0, H1 - DY1, FP), BM.wall, (DX0 + DX1) / 2, (DY1 + H1) / 2, zW);  // 门楣
  // 地板：贴地薄**面**而不是厚盒。厚盒会生成一个 0.13m 高的碰撞盒，而玩家的落脚面
  // 仍在街道 floor(y=0) 上——进店就陷进地板 12cm；平面的 AABB 高度为 0，
  // collidesAt 按「脚踝以下」整块跳过，脚与地面齐平。
  const floorGeo = new THREE.PlaneGeometry(W - FW * 2, ID + 0.08);
  floorGeo.rotateX(-Math.PI / 2);
  const shopFloor = new THREE.Mesh(floorGeo, IM.floor);
  shopFloor.position.set(CX, 0.02, zN + ID / 2 - 0.04);
  shopFloor.receiveShadow = true;
  g.add(shopFloor);

  /* ---- 橱窗（左 2/3）+ 敞开的门（右）---- */
  // 玻璃跟便利店橱窗同一档：finish glass + 低透明度 + depthWrite false，
  // 隔着它看店内几乎无衰减（深蓝的通用玻璃贴图是给「假内饰」用的，会把书架压成灰）。
  put(gbox(GX1 - GX0, GY1 - 0.4, 0.06), BM.shopGlass, (GX0 + GX1) / 2, (0.4 + GY1) / 2, zG);
  put(gbox(GX1 - GX0, 0.4, 0.14), BM.wall, (GX0 + GX1) / 2, 0.2, zG);            // 窗下矮墙
  put(gbox(GX1 - GX0, 0.09, 0.12), BM.frame, (GX0 + GX1) / 2, GY1 - 0.045, zG);  // 窗顶横档
  for (const mx of [GX0, CX - W * 0.18, GX1]) {                                  // 两根边梃 + 一根中梃
    put(gbox(0.09, GY1, 0.10), BM.frame, mx, GY1 / 2, zG - 0.03);
  }
  solid('bookstore-shopfront', (GX0 + GX1) / 2, (0.4 + GY1) / 2, zG, GX1 - GX0, GY1 - 0.4, 0.24);
  // 门框 + 门楣灯 + 敞开的那扇
  /* 三根 BM.frame 的收口关系（旧版三根都顶到 DY1、门槛也整宽，四组共面）：
   *   ① 竖梃**缩到门楣下沿 2.37**、门楣横贯整个门洞 17.90..18.80 压在竖梃上。
   *      旧版竖梃外侧面（x=17.90/18.80）与门楣端面同在 17.90/18.80 且法线同向，
   *      共面 0.061×2.05 ×2 组；竖梃顶面 y=2.45 又与门楣顶面同轴同向，再叠
   *      0.06×0.12 ×2 组。现在门楣底面与竖梃顶面只是 butt joint（法线相反）。
   *   ② 门槛铜条**缩到两竖梃内侧**（17.97..18.73）。旧版整宽 17.90..18.80，
   *      端面又与竖梃外侧面共面 0.04×0.14 ×2 组。
   * 改完共面面积归零。 */
  put(gbox(0.07, DY1 - 0.08, 0.14), BM.frame, DX0 + 0.035, (DY1 - 0.08) / 2, zG);
  put(gbox(0.07, DY1 - 0.08, 0.14), BM.frame, DX1 - 0.035, (DY1 - 0.08) / 2, zG);
  put(gbox(DX1 - DX0, 0.08, 0.14), BM.frame, (DX0 + DX1) / 2, DY1 - 0.04, zG);
  put(gbox(0.90, 0.09, 0.08), BM.cool, (DX0 + DX1) / 2, DY1 + 0.09, zW - FP / 2 - 0.05);
  // 门扇转 180° 贴到门洞西侧中垛的店内面（比前墙内表面再进 3cm）。
  // 不转开的话，斜着门扇的 AABB 会切进洞口，0.85m 的洞只剩 ~0.4m 能过。
  put(gbox(0.80, DY1 - 0.08, 0.045), BM.shopGlass, DX0 - 0.42, (DY1 - 0.08) / 2, zW + FP / 2 + 0.03, Math.PI);
  put(gbox(0.03, 0.42, 0.03), IM.metal, DX0 - 0.10, 1.02, zW + FP / 2 - 0.01);   // 门把手
  put(gbox(DX1 - DX0 - 0.14, 0.04, 0.16), BM.trim, (DX0 + DX1) / 2, 0.02, zG);   // 门槛铜条

  /* ================= 店内陈设 ================= */

  const bookStripMat = bookSpineMaterial();
  /**
   * 书架：木框 + 四层层板，每层贴一条书脊带。
   *
   * `faces` 给书脊贴哪几面（-1 = 朝街 / +1 = 朝店内）。中间两排双向都贴，
   * 后墙那排只贴朝街的一面——朝墙那面贴了没人看得见，白加顶点。
   */
  const shelf = (id: string, x: number, z: number, len: number, dep: number, hgt: number, seed: number, faces: number[]) => {
    const rows = 4;
    put(gbox(len, 0.07, dep), IM.wood, x, hgt - 0.035, z);                     // 顶板
    put(gbox(len, 0.09, dep), IM.woodDark, x, 0.045, z);                       // 踢脚
    /* 两侧立板**外皮缩 4mm、顶面降 1cm**：旧版立板外皮与顶板外皮同在 ±len/2、
     * 顶面与顶板顶面同在 y=hgt —— 两处共面且**法线同向**，木色 #7d6247 与深木
     * #5b4634 会闪（共面扫描实测 x 向 0.034 m² + y 向 0.029 m²，各 6 组）。
     * 缩 4mm 而不是 1mm：扫描器的「靠太近」带是 0.5~2mm，1mm 会被收进那条
     * 噪声表里；4mm 既在安全距离外，又让顶板天然成为一圈 4mm 的压边。 */
    put(gbox(0.06, hgt - 0.01, dep), IM.woodDark, x - len / 2 + 0.034, (hgt - 0.01) / 2, z);
    put(gbox(0.06, hgt - 0.01, dep), IM.woodDark, x + len / 2 - 0.034, (hgt - 0.01) / 2, z);
    for (let i = 1; i <= rows; i++) {
      const y = (hgt / (rows + 1)) * i;
      put(gbox(len - 0.14, 0.045, dep - 0.03), IM.board, x, y, z);
      const bh = (hgt / (rows + 1)) * 0.74;
      for (const s of faces) {
        const stripSeed = seed * 37 + i * 11 + (s > 0 ? 0 : 5);
        const strip = new THREE.Mesh(bookStripGeometry(bookSpineRow(stripSeed)), bookStripMat);
        strip.scale.set(len - 0.2, bh, 1);
        strip.position.set(x, y + 0.023 + bh / 2, z + s * (dep / 2 - 0.012));
        if (s < 0) strip.rotation.y = Math.PI;              // 平面法线朝 +z，朝街那面要翻过来
        g.add(strip);
      }
    }
    solid(id, x, hgt / 2, z, len, hgt, dep);
  };

  /** 一摞平放的书：3~5 本，每本略转一点角度。 */
  const pile = (x: number, z: number, n: number, w: number, d: number, seed: number) => {
    const rnd = makeRng(seed);
    let y = 0.02;
    for (let i = 0; i < n; i++) {
      const h = 0.026 + rnd() * 0.02;
      put(gbox(w * (0.88 + rnd() * 0.24), h, d * (0.88 + rnd() * 0.24)),
        toon(COVER[i % COVER.length]), x, y + h / 2, z, (rnd() - 0.5) * 0.42);
      y += h;
    }
  };

  /* 陈设按新的 6.72 × 9.0 营业厅重排（旧版是 6.72 × 4.4，三排书架就塞满了）。
   * 动线：门在西偏东（x 17.90..18.80）→ 进店就是东侧主走道（x 17.90..19.56，
   * 宽 1.66m，一直通到后墙柜台）→ 想看书往西拐进三条横走道。
   * 书架一律靠西端顶死（西端 x=12.90，内墙面 12.84），把宽度让给主走道。 */

  // 三排双向书架：长边平行橱窗，从街上望进去就是一排排正对的书脊
  shelf('bookstore-shelf-a', 15.40, 17.90, 5.00, 0.48, 1.85, 5, [-1, 1]);
  shelf('bookstore-shelf-b', 15.40, 19.30, 5.00, 0.48, 1.85, 7, [-1, 1]);
  shelf('bookstore-shelf-c', 15.40, 20.70, 5.00, 0.48, 1.85, 13, [-1, 1]);
  // 后墙一排（员工门以西，朝街），把后墙从一块平板变成书架
  shelf('bookstore-shelf-back', 14.55, 23.70, 3.30, 0.42, 2.10, 11, [-1]);

  // 陈列台：正对橱窗，从街上第一眼看到的就是它——平摊的新刊
  put(gbox(1.70, 0.06, 0.74), IM.wood, 15.75, 0.74, 16.15);
  put(gbox(1.44, 0.68, 0.48), IM.woodDark, 15.75, 0.34, 16.15);
  solid('bookstore-table', 15.75, 0.37, 16.15, 1.70, 0.74, 0.74);
  for (let i = 0; i < 4; i++) pile(15.05 + i * 0.42, 16.15, 2 + (i % 2), 0.34, 0.26, 900 + i);

  // 杂志架：斜面矮柜，封面朝上斜着摆（橱窗西端，街上看得见）
  put(gbox(1.15, 0.72, 0.42), IM.wood, 13.60, 0.36, 15.85);
  put(gbox(1.15, 0.05, 0.50), IM.woodDark, 13.60, 0.755, 15.82).rotateX(-0.22);
  solid('bookstore-rack', 13.60, 0.39, 15.85, 1.15, 0.78, 0.50);
  for (let i = 0; i < 3; i++) {
    put(gbox(0.30, 0.02, 0.40), toon(COVER[(i * 3) % COVER.length]), 13.06 + i * 0.36, 0.80, 15.80).rotateX(-0.22);
  }

  // 柜台：后墙东段（东侧主走道尽头），收银 + 一摞待上架的书
  put(gbox(1.30, 0.92, 0.45), IM.wood, 18.55, 0.46, 23.70);
  put(gbox(1.40, 0.07, 0.52), IM.woodDark, 18.55, 0.955, 23.70);
  solid('bookstore-counter', 18.55, 0.49, 23.70, 1.40, 0.99, 0.52);
  put(gbox(0.40, 0.28, 0.32), IM.dark, 18.95, 1.13, 23.70);        // 收银机
  put(gbox(0.42, 0.02, 0.34), BM.cool, 18.95, 1.28, 23.70);
  pile(17.95, 23.68, 4, 0.32, 0.24, 7711);

  // 后墙：员工门（关着——楼上仓库的入口，夹在后墙书架与柜台之间）
  /* 门板 z 中心 zB-0.04（前皮 23.925），两侧金属门套必须**比门板再凸出 5mm**：
   * 旧版门套 z 中心 zB-0.05（前皮也是 23.925），与门板前皮同轴同向共面，
   * 共面扫描实测两组 0.03×2.05 = 0.0615 m² —— 门套横条会在门板上闪。 */
  put(gbox(0.85, 2.05, 0.07), IM.woodDark, 17.00, 1.025, zB - 0.04);
  for (const jx of [16.575, 17.425]) put(gbox(0.06, 2.05, 0.05), IM.metal, jx, 1.025, zB - 0.055);
  put(gbox(0.03, 0.42, 0.03), IM.metal, 16.66, 1.02, zB - 0.08);   // 门把手
  put(gbox(0.70, 0.20, 0.03), BM.cool, 17.00, 2.32, zB - 0.06);    // 「従業員口」灯牌

  /* 两幅海报 + 一个空框：**挂东内墙**，不挂后墙。
   * 旧版挂后墙是因为旧营业厅只有 4.4m 深、东墙只有 4.4m 长，一眼看完；
   * 现在东墙 9.0m 长且正对主走道，是店里最大的一块空白墙面。
   * 后墙则被 2.10m 高的书架和柜台占满了，挂上去全被挡住。 */
  for (const [pz, label, sd] of [[18.20, '新刊', 91], [19.60, '文庫', 92]] as [number, string, number][]) {
    const poster = new THREE.Mesh(new THREE.PlaneGeometry(0.62, 0.86), toon('#ffffff', { map: bookPosterTexture(label, sd) }));
    poster.position.set(WX1 - FW - 0.02, 2.10, pz);
    poster.rotation.y = -Math.PI / 2;                               // 牌面朝店内（-x 方向）
    g.add(poster);
  }
  put(gbox(0.03, 0.86, 0.62), IM.woodDark, WX1 - FW - 0.015, 2.10, 21.00); // 东墙空框

  // 吊顶灯带：五条暖白长条，是「店里亮着」的全部来源（9m 进深，三条盖不住）
  const ceilStripGeo = new THREE.PlaneGeometry(6.2, 0.32);
  ceilStripGeo.rotateX(-Math.PI / 2);
  for (const z of [16.60, 18.40, 20.20, 22.00, 23.50]) {
    const strip = new THREE.Mesh(ceilStripGeo, IM.ceilLight);
    strip.position.set(16.20, IH - 0.02, z);
    g.add(strip);
  }
  // 两盏暖色点光做真实填充：雨夜方向光进不来，只靠自发光面的话书架全是死的。
  // 不投阴影，很便宜（便利店前厅同款做法）。9m 进深一盏照不到两头。
  for (const z of [18.00, 22.00]) {
    const inLight = new THREE.PointLight(0xffe6c0, 10, 13, 2);
    inLight.position.set(16.20, IH - 0.35, z);
    g.add(inLight);
  }

  // 角落的几摞书（不上架的新刊/退货）
  pile(13.10, 22.40, 5, 0.36, 0.28, 9101);
  pile(19.05, 22.30, 3, 0.32, 0.24, 9102);
  pile(13.10, 17.20, 4, 0.30, 0.24, 9103);

  // 遮阳篷（临街，米色，微斜）+ 支架。1F 加高到 3.4 后篷底抬到 3.22，北挑
  // 1.1m 落在 z 13.95..15.05 —— 北面 1.05m 外就是路灯 (14, 12.9)，够不着。
  put(gbox(W * 0.8, 0.06, 1.1), BM.trim, CX, H1 - 0.18, zN - 0.5).rotateX(0.10);
  for (const dx of [-W * 0.36, W * 0.36]) {
    put(gbox(0.05, 0.55, 0.05), BM.dark, CX + dx, H1 - 0.45, zN - 0.06);
  }

  // 门侧海报框（中間柱 x 17.05..17.90，0.85m 宽）：临街面唯一能贴纸的地方——
  // 西边整片是玻璃、东边是门洞，中间这根垛不挂点东西就只剩一条白墙。
  put(gbox(0.68, 0.92, 0.05), BM.frame, (GX1 + DX0) / 2, 1.72, zN - 0.08);
  const doorPoster = new THREE.Mesh(new THREE.PlaneGeometry(0.58, 0.82),
    toon('#ffffff', { map: bookPosterTexture('話題', 97) }));
  doorPoster.position.set((GX1 + DX0) / 2, 1.72, zN - 0.11);
  doorPoster.rotation.y = Math.PI;   // 牌面朝 -z（临街）
  g.add(doorPoster);

  // 店招（发光面板 + 深色底板，挂 1F 腰线上方）。
  // 楼体从 6.6m 长到 12.4m，2.6m 的牌子在立面上会缩成一条，放到 3.2m。
  const signCanvas = (() => {
    const { canvas, ctx } = makeCanvas(256, 64);
    ctx.fillStyle = '#233240';
    ctx.fillRect(0, 0, 256, 64);
    ctx.fillStyle = '#f2ede2';
    ctx.font = 'bold 34px sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('書泉堂書店', 128, 45);
    return toTexture(canvas);
  })();
  const signMat = emissive('#ffffff', { map: signCanvas });   // 基色走白，颜色全在贴图（基色会乘进贴图，深底色会压暗字）
  /* 店招中心 y = H1 + 0.50（3.90）：底板高 0.90 ⇒ 顶边正好 4.35，**等于 2F 窗洞下沿**
   * （窗心 fy+1.70=5.10，高 1.5 ⇒ 4.35..5.85）。旧版取 H1+0.62（4.02）顶边 4.47，
   * 与窗洞下沿重叠 0.12m —— 底板与窗玻璃同在 z 14.95..15.01，正面共面 0.138 m²，
   * 从街上正视立面时会闪。顶边对齐后两者只是对接（法线相反），共面面积归零。 */
  const sign = new THREE.Mesh(new THREE.PlaneGeometry(3.2, 0.80), signMat);
  sign.position.set(CX, H1 + 0.50, zN - 0.10);
  sign.rotation.y = Math.PI;   // 牌面朝 -z（临街），默认 +z 会转向楼内只剩背板
  g.add(sign);
  put(gbox(3.35, 0.90, 0.06), BM.frame, CX, H1 + 0.50, zN - 0.02);

  // 上部三层窗：临街每层三扇 + 两侧山墙各一扇（侧窗逐层错位，不然是一条竖井）
  for (let f = 1; f < FLOORS; f++) {
    const fy = H1 + (f - 1) * HF;
    for (let i = 0; i < 3; i++) {
      const lit = (i + f) % 3 !== 1;   // 每层暗一扇，且暗的位置逐层挪
      const wx = CX - W * 0.3 + i * W * 0.3;
      put(gbox(1.15, 1.5, 0.06), lit ? BM.warm : BM.glass, wx, fy + 1.70, zN - 0.02);
      put(gbox(1.25, 1.6, 0.05), BM.frame, wx, fy + 1.70, zN - 0.015);
    }
    for (const s of [-1, 1] as number[]) {
      put(gbox(0.05, 1.3, 1.1), r() > 0.5 ? BM.warm : BM.glass,
        CX + s * (W / 2 + 0.01), fy + 1.70, CZ + s * (0.6 + (f - 1) * 1.4));
    }
  }

  // 空调外机（二层临街西端，避开窗与店招）+ 落水管（东山墙，通到新檐口 12.4m）
  put(gbox(0.6, 0.4, 0.3), BM.dark, CX - W / 2 + 0.45, H1 + 0.50, zN - 0.22);
  put(gbox(0.07, HT, 0.07), BM.dark, CX + W / 2 + 0.06, HT / 2, zN + 0.15);

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

  // 西墙一层贴报栏（西侧不是全 blank 的山墙；这段墙朝西对着居酒屋与便利店
  // 之间的空地，203 阳台斜看得到）
  put(gbox(0.06, 1.1, 0.8), BM.frame, CX - W / 2 - 0.02, 1.75, CZ + 1.2);
  put(gbox(0.04, 0.9, 0.6), BM.cool, CX - W / 2 - 0.05, 1.75, CZ + 1.2);

  // 橱窗暖光洒到湿人行道上。书店是这一排里唯一「橱窗比门大」的店——
  // 暖光板 BM.warm 铺了 W*0.55 宽，但地面此前毫无回应，夜里橱窗像悬空的灯箱。
  // 口朝 -z（zN 是临街面），所以光池铺在 zN 以北；y=0.036 压住铺装面。
  const bookPoolMat = emissive('#ffe4b5', {
    map: radial('#ffe8c0', 'rgba(255,215,165,0)'),
    transparent: true, opacity: 0.26, side: THREE.DoubleSide,
  });
  bookPoolMat.blending = THREE.AdditiveBlending;
  bookPoolMat.depthWrite = false;
  // 进深 2.4m：北墙落到人行道南缘（15.0）之后，光池正好铺满 12.6..15.0 整条带，
  // 再深就洒到车行道上了（旧值 2.8 / zN-1.4 是配旧北墙 18.0 的）。
  const bookPool = new THREE.Mesh(new THREE.PlaneGeometry(8.0, 2.4), bookPoolMat);
  bookPool.geometry.rotateX(-Math.PI / 2);
  bookPool.position.set(CX, 0.036, zN - 1.2);
  bookPool.renderOrder = 2;
  bookPool.name = 'bookstore-light-pool';
  bookPool.userData.sceneCollideSkip = true;
  g.add(bookPool);

  // 整组仍标 sceneCollideSkip：书店的碰撞是**声明式**的（见函数开头的 boxes），
  // 不走 buildSceneColliders 遍历——橱窗玻璃近全透明，遍历会整块跳过它，
  // 玩家就能从橱窗穿进店里。
  g.userData.sceneCollideSkip = true;
  return { group: g, boxes };
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

  /*
   * 公园位置：公寓西侧外廊之外（x -40.4~-31.4，z -13.3~-7.3）。
   *
   * 旧版写的是 x -24.5~-15.5 / z 1.5~7.5，那个位置**根本不存在**：公寓一层南墙
   * 在 z=4.7，路缘在 7.6，中间只有 2.9m，而公园进深 6m。结果 13 个构件里 10 个
   * 埋进公寓一层实心体量（实测最深 2.52m，树在 z=3.0、长椅在 z=2.5，都在楼里）。
   * check-urban-plan.mjs 抓不到——它只校验程序化平面，手工件不在它的视野内。
   *
   * 现在整块落在公寓外壳（x≥-31.38）以西：x 上完全没有交集，z 也就无所谓了。
   * 这是这一带唯一一块真正空着、又能被 203 的西侧视线看到的场地。
   */
  const PX0 = -40.4, PX1 = -31.4;   // 公园 x 范围（东边界卡在公寓西皮之外）
  const PZ0 = -13.3, PZ1 = -7.3;    // 公园 z 范围（PZ1 那侧贴公寓电梯井，见下）
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
  /* ⚠️ 这里两个方向的注释早先写反了：本项目 **+Z 是南**（见公寓剖面注释「+Z 北→南」），
   * 所以 z 更大的 PZ1(-7.3) 是**南**边（贴公寓电梯井那侧）、PZ0(-13.3) 是**北**边。
   * 下面按实际方位写。 */
  const fenceH = 0.45, fenceSeg = 0.6;
  // 南边（z=PZ1，这一排贴着公寓电梯井北皮 z=-7.34）
  for (let fx = PX0; fx < PX1; fx += fenceSeg) {
    const sw = Math.min(fenceSeg, PX1 - fx);
    if (sw > 0.1) put(gbox(sw, fenceH, 0.04), PM.wood, fx + sw / 2, fenceH / 2, PZ1);
  }
  // 西边（x=PX0）
  for (let fz = PZ0; fz < PZ1; fz += fenceSeg) {
    const sd = Math.min(fenceSeg, PZ1 - fz);
    if (sd > 0.1) put(gbox(0.04, fenceH, sd), PM.wood, PX0, fenceH / 2, fz + sd / 2);
  }
  // 围栏入口（北边中间留 2m 口子）
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
  // 第 2 棵从 PCX+3 收到 PCX+1.6：树冠半径约 2m，站在 x=-32.9 时树冠东缘到 -30.92，
  // 而公寓西皮在 -31.38 —— 会穿墙 0.46m（check-district-overlaps 报
  // apartment-shell ∩ small-park）。西侧那两棵冠幅探出围栏是正常的（外面是空地），
  // 东侧不行：那边是公寓实体。
  //
  /* 2026-09-21 用户报「小公园的树穿模进公寓，将树北移少许」。
   * 实测（`tmp/_probe-park-trees.mjs`，顶点云按树心聚类）真正的元凶是**公寓电梯井**：
   * `apartment-lift-tower` 的世界 AABB 是 x[-36.87,-30.9] / z[-7.34,4.84] —— 它比
   * 公寓外壳（APT_X0=-31）还往西探 5.9m，所以「公园在 x 上完全避开公寓外壳」这条
   * 早先的论证对电梯井并不成立。三棵树的世界 AABB：
   *   树1 (-39.9,-11.8) x[-41.96,-38.04] z[-13.45,-10.15]  → x 上离电梯井还差 1.17m，安全
   *   树2 (-34.3, -8.3) x[-35.72,-32.32] z[ -9.95, -6.79]  → 树冠南缘 -6.79 越过井北皮
   *                                                            -7.34 达 **0.551m**，实打实穿模
   *   树3 (-36.9, -9.3) x[-39.04,-35.11] z[-12.24, -7.63]  → 只剩 0.292m 余量，太薄
   * 北移 = z 减小（本项目 +Z 由北向南，见上方公寓剖面注释）。树2 北移 0.9 ⇒ 南缘
   * -7.69（余 0.35m）；树3 北移 0.4 ⇒ 南缘 -8.03（余 0.69m）。树1 不动。 */
  const treePositions: Array<[number, number]> = [
    [PCX - 4, PCZ - 1.5],
    [PCX + 1.6, PCZ + 1.1],     // 原 PCZ+2，北移 0.9
    [PCX - 1, PCZ + 0.6],       // 原 PCZ+1，北移 0.4
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
  // 取 ±4.2 而不是 ±5：公园半宽只有 4.5，±5 会伸到围栏外面去（旧版场地宽 9m
  // 时就已经越界 0.5m，只是没人量过）。现在公园贴着公寓西皮，越界就等于穿墙。
  put(gbox(0.30, 0.95, 0.28), PM.metal, PCX - 4.2, 0.48, PZ0 + 0.8);
  // 饮水机按钮/出水口暗示
  put(gbox(0.06, 0.06, 0.03), PM.cool, PCX - 4.2, 0.75, PZ0 + 0.68);

  // ---- 10) 垃圾桶 ----
  put(gbox(0.32, 0.48, 0.32), PM.slabDark, PCX + 4.2, 0.24, PZ1 - 0.8);

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
  // 远处列只留西侧空地一条南北向短列（x=-33，地铁口/停车场/小公园以西）。
  // 原「沿高架平行的斜列」已删：那座高架从未接线（导出已删），那条斜线本身穿过 z=73
  // 一排楼——1 根杆埋进 11 层楼体内、跨距电线从楼中穿过，且 4 根杆全在 74~103m 重雾带。
  // 中景低模楼已退到 z 33~44，原 z=39 的东西向直列会穿楼，故取消。
  const ROW_A_Z = 13.3;              // 商铺侧人行道（近景仅存 2 根）
  const FAR_WEST_X = -33.0;          // 远处南北列
  const A_X = [-26, -16];            // 近景：地铁口西侧 + 停车场东侧各一根
  const F2_Z = [10, 18, 26, 34];                          // 远处南北列
  for (const x of A_X) pole(x, ROW_A_Z, x === -26);
  for (const z of F2_Z) pole(FAR_WEST_X, z, z === 18);

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
 *
 * 整组仍标 `sceneCollideSkip`（`districtArt.prepare` + `mergeByMaterial` 之后
 * mesh 已并成同材质巨块、名字也没了，`buildSceneColliders` 的 AABB 遍历收不到
 * 有用的东西），**但柜体必须自己声明碰撞盒**：`{ group, boxes }` 由调用方
 * `buildBoxColliders(boxes)` 转成世界 AABB，跟便利店 / 书店 / 居酒屋同一条路。
 *
 * 2026-09-21：用户报「居酒屋北侧和停车场北侧各有一台相同的自动售货机缺碰撞箱」
 * ——原来整组一个盒都没有，2m 高的柜子能直接穿过去。现在 2 台售货机 + 2 台快递柜
 * （都是 2.0m 高的实心柜）登记碰撞盒。
 *
 * **细杆件（路牌柱 / 公交站牌柱）与条凳刻意不登记**：
 *   - 杆件直径 0.07~0.08m，跟 `buildUtilityPoles` 的电线杆（0.20m）一样，本项目
 *     的既有约定是"杆不挡人"——只在人行道中间插一根 0.15m 半径的圆柱，比穿模更烦人。
 *   - 条凳面在 y=0.42~0.48（浮在 0.42 之上的 6cm 厚板），`STEP_UP=0.45` 会判它
 *     "迈不过去"而变成一道 6cm 高的隐形墙，比让它可跨更怪。
 *   要改这两条先跟用户确认。
 */
export function buildStreetFurniture(): { group: THREE.Group; boxes: BoxColliderSpec[] } {
  const g = new THREE.Group();
  g.name = 'street-furniture';

  const boxes: BoxColliderSpec[] = [];
  /** 单块实体：建几何 + 同参数登记碰撞盒（不给两处各写一份数字的机会） */
  const mass = (id: string, x: number, y: number, z: number,
                sx: number, sy: number, sz: number, mat: THREE.Material) => {
    put(gbox(sx, sy, sz), mat, x, y, z);
    boxes.push(boxSpec(id, x - sx / 2, x + sx / 2, y - sy / 2, y + sy / 2, z - sz / 2, z + sz / 2));
  };

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
  let vendingN = 0;
  const vending = (x: number, z: number) => {
    // 柜体走 mass()：2.0m 高实心柜，玩家不该穿过去。三块发光面是 0.05m 薄贴片、
    // 贴在柜体正面之外，本身不构成阻挡（柜体盒已覆盖），故仍用 put()。
    mass(`streetfurn-vending-${++vendingN}`, x, 1.0, z, 1.1, 2.0, 0.7, SF.metal);
    put(gbox(0.92, 1.35, 0.05), SF.panel, x, 1.35, z - 0.36);
    put(gbox(0.5, 0.35, 0.05), SF.panel, x + 0.25, 0.45, z - 0.36);
    put(gbox(0.18, 0.5, 0.05), SF.panel, x - 0.35, 0.55, z - 0.36);
  };
  /* 停车场北侧这台原来在 x=-13.5 —— 而便利店侧路灯列 `STORE_LAMP_X` 里有一根正好
   * 立在 x=-14 / z=12.9：机柜 x −14.05..−12.95 把 0.112m 粗的灯柱和它 0.28m 的
   * 底座整个吞进去 0.106 / 0.19m，灯柱从柜子里长出来、灯头还高出柜顶 0.5m。
   * 2026-09-22 实测（`tmp/_probe-furniture-spot.mjs`：逐顶点扫柜体盒，报肇事 mesh）：
   * 侵入的全是 `exterior-streetscape` 的圆柱（灯柱 + 底座），没有别的。
   * 修法取**挪柜不挪灯**：灯列 −21/−14/0/7/14/21 是 7m 等距的节奏，抽掉 −14 会让
   * −21 到 0 之间空出 21m 的黑段；柜子西移到 −15.3（机柜 x −15.85..−14.75）后，
   * 离灯柱 0.694m、离底座 0.61m，且该处逐顶点扫描 0 侵入。 */
  vending(-15.3, 13.0);   // 停车场北侧（LOT 地块内，面朝人行道）
  /* 居酒屋北侧那台原来在 x=11.5 —— 而居酒屋的感应门门洞是 x 10.60..11.60
   * （doorCx = CX + W/2 - 0.6 = 11.10），机柜 x 10.95..12.05 正好堵住门洞东侧
   * 0.65m，玩家只剩 x∈[10.75,10.80] 这 5cm 的一条缝能挤进门。
   * 2026-09-21 给居酒屋开门时实测到（`tmp/_verify-izakaya-door.mjs` 推进到
   * z=12.65 被 streetfurn-vending-2 拦住），西移到 x=8.4 让开门洞：
   * 机柜 x 7.85..8.95，离门洞西缘 1.65m，与 (10,13) 的路牌柱也留出 1.0m。
   * 仍在同一段人行道、同一排街沿上，只是挪到店面前窗前面。 */
  vending(8.4, 13.0);     // 居酒屋北侧

  // 快递柜：多格机柜 + 操作屏
  let parcelN = 0;
  const parcel = (x: number, z: number) => {
    mass(`streetfurn-parcel-${++parcelN}`, x, 1.0, z, 1.7, 2.0, 0.6, SF.metal);
    for (const dy of [-0.6, -0.2, 0.2, 0.6]) put(gbox(1.5, 0.04, 0.04), SF.metal, x, 1.0 + dy, z - 0.30);
    put(gbox(0.35, 0.5, 0.05), SF.panel, x - 0.5, 1.2, z - 0.31);
  };
  parcel(-18.0, 14.5);
  // 书店那台从 (16.5, 14.0) 挪到 (21.0, 14.0)：书店 2026-09 扩建把北墙从 z=18.0
  // 推到人行道南缘 z=15.0，这台 2.0m 高的柜子就正好杵在橱窗（x 13.15..17.05）
  // 前面 0.7m 处，把橱窗东半截挡死。挪到东邻 parcel-23.5-19.6 门前的人行道上
  // （已量：该处 z 13.6..14.4 内 0 顶点侵入，且落在 sidewalk 条带 12.6..15.0 上）。
  parcel(21.0, 14.0);

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
  /* 整簇（柱 / 牌面 / 背板 / 条凳）原来都在 x=-6.5，而便利店自己的两台自动售货机
   * 摆在 x=-5.75 / -4.6 —— 1 号机柜 x −6.295..−5.205 把**条凳东端**（x −7.2..−5.8）
   * 吞掉 0.475m、把**灯箱背板东端**（x −6.95..−6.05）吞掉 0.225m，条凳看起来
   * 是一截插进售货机里断掉的。
   * 2026-09-22 逐顶点扫（`tmp/_probe-furniture-spot.mjs`）确认这是第三个穿模，
   * 且两簇的中心距只有 0.75m，而「条凳半宽 0.7 + 机柜半宽 0.545 = 1.245m」
   * ⇒ 必须拉开 1.245m 以上。柜子东移会撞上垃圾桶（x=-3.35）和店面，故**把公交站
   * 整簇西移 1.3m 到 x=-7.8**（两簇中心距 2.05m，留 0.8m 余量）。
   * 新位置 x −8.55..−7.05 / z 12.90..13.70 逐顶点扫描 **0 侵入**。 */
  const busX = -7.8;
  put(gbox(0.07, 2.5, 0.07), SF.metal, busX, 1.25, 13.0);                 // 立柱
  const busSign = new THREE.Mesh(new THREE.PlaneGeometry(0.85, 0.5), busMat);
  busSign.position.set(busX, 2.1, 12.96);
  busSign.rotation.y = Math.PI;   // 牌面朝 -z 对人行道来向，默认 +z 只见背板
  g.add(busSign);
  put(gbox(0.9, 0.55, 0.04), SF.metal, busX, 2.1, 13.0);                  // 牌背板
  put(gbox(1.4, 0.06, 0.45), SF.metal, busX, 0.45, 13.35);                // 条凳

  // 整组仍标 sceneCollideSkip：柜体的碰撞是**声明式**的（见函数开头的 boxes），
  // 遍历本来也收不到有用的 AABB。调用方必须 buildBoxColliders(streetFurniture.boxes)。
  g.userData.sceneCollideSkip = true;
  return { group: g, boxes };
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
