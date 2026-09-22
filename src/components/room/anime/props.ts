/**
 * 日式宿舍房间的一整套道具。
 *
 * 每一件都是"组合体"而不是单个 box：床有床头板/褥子/被子/枕头，书桌有显示器/
 * 键盘/台灯/马克杯，书架里塞满书。全部程序化生成，所以改配色只需要动调色板。
 *
 * 约定：
 *  - 每个 buildXxx(spec) 返回一个 Group，group 的原点 = 家具在地面的落脚中心
 *  - spec.pos = [x, y, z]，spec.size = [宽X, 高Y, 深Z]
 *  - 家具自身的局部坐标里，y=0 就是地板
 */

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import {
  toon,
  emissive,
  addOutline,
  makeRng,
  woodFloorTexture,
  wallTexture,
  tatamiTexture,
  tileTexture,
  deckTexture,
  furnitureWoodTexture,
  rugTexture,
  rainStreakTexture,
  rainBlobTexture,
  doorGrainMap,
  rainGlassTexture,
  puddleRippleTexture,
  dripTexture,
  curtainTexture,
  posterTexture,
  screenTexture,
  softDotTexture,
  beamTexture,
  lightPoolTexture,
  wetStreakTexture,
  marbleTexture,
  velvetTexture,
  jpWoodFloorTexture,
  jpKitchenFloorTexture,
  jpWoodGrainTexture,
  jpFabricTexture,
  jpRugTexture,
  jpShojiPaperTexture,
  jpCeramicTexture,
  jpMetalTexture,
  jpPaperBookTexture,
  jpCounterTopTexture,
  jpFridgePanelTexture,
  jpDeckWoodTexture,
  jpFutonTexture,
  jpPosterTexture,
  jpClockFaceTexture,
  jpBathTileTexture,
} from './toon';

/**
 * 住户入户门的三档轮换色（**外廊侧**）。
 *
 * 三档都压到低明度、彼此拉开色相。原先的门色里有两档几乎同色（woodDeep 与 doorB
 * 都是中明度暖棕），暖光再把墙（#8b96a5 蓝灰）染暖之后，门和墙、门和门之间全糊在
 * 一起，一排看过去像同一块板。整栋楼的门共用这一套：`exterior.ts` 的 M.doorA/B/C
 * 与这里的 buildEntryDoor 都从这里取，别再各写一份。
 *
 * 轮换规则是 `(ui + fi * 2) % 3`——203 是 2F 中户（ui=2、fi=0），落在第 3 档。
 *
 * 三档的色相都收在中性区（棕 25° / 无彩 / 橄榄 78°），明度也都在 18~27%：
 * 外廊是公共面，一排门要读成"同一套门的不同批次"，而不是三个彩色样板。
 * 第 3 档尤其别往"绿"走——它在暖棕的廊道里最容易被衬出来。
 */
export const UNIT_DOOR_TONES = ['#4a3a2c', '#2a2d30', '#3f4238'] as const;

/** 203 在轮换表里的档位（2F 中户：(2 + 0 * 2) % 3 = 2）。 */
const OWN_UNIT_TONE = 2;

/* ============================================================================
 * 模块级风格状态
 * ========================================================================== */

type ArtStyle = { outlineColor: string; outlineWidth: number; night?: boolean };

let PAL: Record<string, string> = {};
let STYLE: ArtStyle = { outlineColor: '#3d2f3a', outlineWidth: 0.0035, night: true };

/** 夜景开关。决定窗外贴图、玻璃水痕、地面色温这一整组分支。 */
let NIGHT = true;

export function setArtStyle(palette: Record<string, string>, style?: Partial<ArtStyle>): void {
  PAL = palette ?? {};
  STYLE = { outlineColor: '#3d2f3a', outlineWidth: 0.0035, night: true, ...(style ?? {}) };
  NIGHT = STYLE.night !== false;
}

function C(key: string, fallback: string): string {
  return PAL[key] ?? fallback;
}

/** 给一件家具描边。scale 用于抵消父级缩放对描边厚度的影响。 */
export function outlineProp(g: THREE.Object3D, scale = 1): THREE.Object3D {
  return addOutline(g, STYLE.outlineWidth / Math.max(0.0001, scale), STYLE.outlineColor);
}

/* ============================================================================
 * 小工具
 * ========================================================================== */

type ShadowMode = 'both' | 'cast' | 'receive' | 'none';

function m(
  geo: THREE.BufferGeometry,
  mat: THREE.Material,
  pos: [number, number, number] = [0, 0, 0],
  shadow: ShadowMode = 'both'
): THREE.Mesh {
  const mesh = new THREE.Mesh(geo, mat);
  mesh.position.set(pos[0], pos[1], pos[2]);
  mesh.castShadow = shadow === 'both' || shadow === 'cast';
  mesh.receiveShadow = shadow === 'both' || shadow === 'receive';
  return mesh;
}

/**
 * 方块基元统一带倒角。真实家具没有数学意义上的锐边——光打到锐边上没有任何
 * 过渡，赛璐璐色阶下边缘是一条生硬的明暗跳变，这是"糙"的主要来源之一。
 * 倒角给边缘一小段圆弧，色阶在弧上滚出一档过渡色，剪影也柔。
 *
 * 半径 8mm：5.9m 默认机位下亚像素，只贡献明暗过渡不改变剪影；第一人称
 * 贴脸时刚好能读出"这是有厚度的板件"。薄件由 RoundedBoxGeometry 内部
 * 把半径钳到 min(边长)/2，不会撑爆。
 *
 * 代价：segments=2 时每 box 约 300 三角形（锐边盒 12 个），全屋 260 处
 * 合计约 8 万三角形，对 GPU 无感；几何是非索引的，合批侧见 merge.ts 的
 * 索引归一处理。
 */
const BEVEL_RADIUS = 0.008;
const box = (w: number, h: number, d: number) =>
  new RoundedBoxGeometry(w, h, d, 2, BEVEL_RADIUS);
const cyl = (rt: number, rb: number, h: number, seg = 16, openEnded = false) =>
  new THREE.CylinderGeometry(rt, rb, h, seg, 1, openEnded);
const sph = (r: number, w = 12, h = 10) => new THREE.SphereGeometry(r, w, h);

/**
 * 把几何的 UV 按世界尺寸缩放，让贴图的物理尺度脱离物体大小。
 *
 * 贴图 repeat 写死成常量时，同一张木地板铺在 4.8m 主卧与 10.2m 走廊上板条尺寸差好几倍，
 * 且长宽比被非等比拉歪。改为 repeat 恒为 1、由 UV 承载尺寸（uv *= 尺寸 / 贴图代表的米数），
 * 全屋木纹 / 瓷砖 / 墙纸的图案尺度即锁死。
 *
 * 附带好处：UV 不同的地板可共用同一材质实例，同材质地板即可合批。
 */
export function scaleUV(geo: THREE.BufferGeometry, sx: number, sy: number): THREE.BufferGeometry {
  const uv = geo.getAttribute('uv') as THREE.BufferAttribute | undefined;
  if (!uv) return geo;
  for (let i = 0; i < uv.count; i++) uv.setXY(i, uv.getX(i) * sx, uv.getY(i) * sy);
  uv.needsUpdate = true;
  return geo;
}

/**
 * 一张贴图代表多大的一块面（米）。
 * 木工/瓦工的现实尺寸：地板条长 1.6m、宽 0.2m；地砖 0.45m 见方；
 * 榻榻米半叠 0.9×1.8m（贴图是正方形铺满，所以 XY 不相等）；防腐木板窄一些。
 */
const FLOOR_TILE: Record<RoomDef['floor'], [number, number]> = {
  wood: [1.6, 1.6],
  tile: [1.8, 1.8],
  tatami: [0.9, 1.8],
  deck: [1.2, 1.2],
  jpwood: [1.6, 1.6],
  jptile: [2.0, 2.0],
  jpdeck: [1.2, 1.2],
};

/** 墙纸：一张贴图代表 1.6m 见方，樱花簇的间距由此固定。 */
const WALL_TILE = 1.6;

/* ============================================================================
 * 房间外壳：地板 / 墙 / 天花 / 踢脚线
 * ========================================================================== */

export type WallOpening = {
  /** door=室内门洞（落地，门扇另做家具）；window=窗洞；glass=玻璃滑门；pass=纯通道洞口 */
  kind: 'door' | 'window' | 'glass' | 'pass';
  /** 沿墙方向的洞口范围（世界坐标，a < b） */
  a: number;
  b: number;
  /** 竖向范围；door/pass 是 0 ~ 门楣高 */
  y0: number;
  y1: number;
};

export type WallSeg = {
  id: string;
  /** 法线轴：'z' = 墙沿 X 铺、法线指 ±Z；'x' = 墙沿 Z 铺、法线指 ±X */
  axis: 'x' | 'z';
  /** 墙所在的坐标（如 axis 'z' → 墙在 z = at） */
  at: number;
  /** 沿墙方向的范围 */
  from: number;
  to: number;
  /** 外墙朝屋内的法线方向；内墙（exterior=false）恒为双面，忽略此值 */
  dir?: 1 | -1;
  exterior?: boolean;
  openings?: WallOpening[];
};

export type RoomDef = {
  id: string;
  x0: number; x1: number; z0: number; z1: number;
  floor: 'wood' | 'tile' | 'tatami' | 'deck' | 'jpwood' | 'jptile' | 'jpdeck';
  /** 阳台这类露天区不铺天花 */
  ceiling?: boolean;
};

export type ShellConfig = {
  room: { bounds: { x0: number; x1: number; z0: number; z1: number }; height: number };
  shell: {
    wallThickness: number;
    rooms: RoomDef[];
    walls: WallSeg[];
    /** 阳台栏杆：沿边一圈，不挡视线也不参与剖面剔除 */
    railing?: Array<{ axis: 'x' | 'z'; at: number; from: number; to: number }>;
  };
};

/** 一个"墙面 + 该侧的厚装饰（门框/玻璃门/窗帘）"。装饰组按相机在墙哪一侧显隐。 */
export type WallSide = {
  id: string;
  axis: 'x' | 'z';
  at: number;
  dir: 1 | -1;
  /** 贴墙装饰（窗框/门框）。跟随墙显隐。 */
  decor: THREE.Group;
  // 注意：decor 和 roomSideDecor 在观察者模式下**都**跟随墙显隐（见 RoomScene 渲染循环）。
  // 早期注释写的是「roomSideDecor 永远可见」，早已不成立——墙被视线剔除时凸入室内的
  // 窗帘/窗台也得一起隐，否则剖切面上会吊着一层无墙的装饰。
  /** 凸入室内的部分（窗台/窗帘/盆栽）。 */
  roomSideDecor: THREE.Group;
  /**
   * 该侧的洞口断面（窗台/门楣/门套侧壁）。跟随墙显隐。
   * 内墙的断面两侧房间都看得见，统一走 `ShellResult.interiorReveals` 恒显示，
   * 那种墙的这个组是空的。
   */
  reveals: THREE.Group;
};

export type ShellResult = {
  group: THREE.Group;
  wallSides: WallSide[];
  wallMeshes: THREE.Mesh[];
  ceilingMeshes: THREE.Mesh[];
  /** 内墙的洞口断面：两侧房间都看得见，不参与逐侧剔除。 */
  interiorReveals: THREE.Group;
};

/**
 * 房间外壳 v3：多房间 L 形套房（第一人称双面模式）。
 *
 * 墙体全部 DoubleSide：第一人称在房间内部，从任何角度都能看到墙的正面。
 * 不再是剖面娃娃屋——相机进入房间后需要看到完整的室内空间。
 *
 * 地板按房间分铺：木地板/瓷砖/榻榻米/防腐木。天花同理，阳台露天不铺。
 *
 * 每面墙的每一侧登记一个 WallSide：厚装饰（窗框窗帘、门扇、玻璃滑门）由
 * RoomScene 按布局里的 wall 字段挂进对应 decor 组。
 *
 * 墙/天花/侧壁全部投影且 shadowSide=DoubleSide：太阳是房间外的平行光，
 * 直射光只允许从窗洞进来。
 */
export function buildRoomShell(layout: ShellConfig): ShellResult {
  const g = new THREE.Group();
  const H = layout.room.height;
  const t = layout.shell.wallThickness;

  /** 收集所有墙板 mesh，供观察者模式做视线剔除（背对相机的墙隐藏） */
  const wallMeshes: THREE.Mesh[] = [];
  const ceilingMeshes: THREE.Mesh[] = [];

  const floorMat = toon(C(NIGHT ? 'floorTintNight' : 'floorTint', '#ffffff'), { map: woodFloorTexture() });
  // 这两份要就地改 shadowSide，必须独占，免得改动顺着共享缓存流到别处。
  // 第一人称模式：墙体全部双面（从房间内部看需要看到墙的正面）
  const wallMat = toon(C(NIGHT ? 'wallTintNight' : 'wallTint', '#ffffff'), { map: wallTexture(), unique: true, side: THREE.DoubleSide });
  const ceilMat = toon(C('ceiling', '#fdf7ec'), { unique: true, side: THREE.DoubleSide });
  /**
   * 洞口侧壁（窗台/门楣/两侧门套的那一圈窄面）是墙的**断面**，不是墙面。
   * 断面不贴墙纸——现实里它是抹灰见光，而且这圈面只有 12cm 宽，
   * 铺一张 1.6m 见方的墙纸上去等于把图案放大十几倍，近看是一坨糊色。
   */
  const revealMat = toon(C('reveal', '#efe6d6'), { unique: true });
  /**
   * 墙/天花是朝内的单面平面，而 three 的阴影 pass 对 FrontSide 材质只渲染背面
   * （shadowSide 默认映射 FrontSide→BackSide）。朝光的那一面是正面的墙会被整个
   * 剔除掉，直射光就从墙里漏进来了。显式设 DoubleSide 让墙真正挡光。
   */
  wallMat.shadowSide = THREE.DoubleSide;
  ceilMat.shadowSide = THREE.DoubleSide;
  revealMat.shadowSide = THREE.DoubleSide;
  /**
   * 天花板板面 y=H 与墙片 y1=H 在房间边界（外墙整圈 + 所有内墙分隔线）精确共线，
   * 24-bit 非线性深度缓冲在该线两侧数 cm 范围内分不出先后 → 沿交接线沿天花板肋纹闪。
   * polygonOffset 把墙平面沿相机方向推走 1 个最小深度单位，让天花板稳赢；墙 plane
   * 是唯一用 wallMat 的几何，挂件/挂钟/海报各自独立材质不受影响，仍叠在墙前面。
   */
  wallMat.polygonOffset = true;
  wallMat.polygonOffsetFactor = 2;
  wallMat.polygonOffsetUnits = 2;
  // 地板/天花板推后，让墙在所有接缝处（顶/底/侧）都稳赢
  floorMat.polygonOffset = true;
  floorMat.polygonOffsetFactor = -2;
  floorMat.polygonOffsetUnits = -2;
  ceilMat.polygonOffset = true;
  ceilMat.polygonOffsetFactor = -2;
  ceilMat.polygonOffsetUnits = -2;

  const wallSides: WallSide[] = [];
  const decorFor = (id: string, axis: 'x' | 'z', at: number, dir: 1 | -1): WallSide => {
    const decor = new THREE.Group();
    const roomSideDecor = new THREE.Group();
    const reveals = new THREE.Group();
    g.add(decor);
    g.add(roomSideDecor);
    g.add(reveals);
    const ws: WallSide = { id, axis, at, dir, decor, roomSideDecor, reveals };
    wallSides.push(ws);
    return ws;
  };

  /* ---------------- 地板 / 天花（按房间） ---------------- */

  /**
   * 地面材质按铺装种类共享一份实例 —— 各房间的尺寸差异已经由 UV 缩放承担，
   * 不再需要为每个房间 clone 一份材质去改 repeat。共享是后面合批的前提。
   */
  const floorMats: Record<RoomDef['floor'], THREE.Material> = {
    wood: floorMat,
    tatami: toon('#ffffff', { map: tatamiTexture() }),
    tile: toon('#ffffff', { map: tileTexture() }),
    // 阳台木地板带色偏而不是纯白：纯白会让木材贴图原样输出暖橙，
    // 在冷灰立面正中横着断开一条（邻居户全是湿混凝土）。
    deck: toon(C('deck', '#9a8a70'), { map: deckTexture(), finish: 'wet' }),
    jpwood: toon('#ffffff', { map: jpWoodFloorTexture() }),
    jptile: toon('#ffffff', { map: jpKitchenFloorTexture() }),
    jpdeck: toon('#ffffff', { map: jpDeckWoodTexture(), finish: 'wet' }),
  };
  // 所有地板材质统一推后，避免与墙共面 z-fighting
  for (const fm of Object.values(floorMats)) {
    fm.polygonOffset = true;
    fm.polygonOffsetFactor = -2;
    fm.polygonOffsetUnits = -2;
  }

  for (const r of layout.shell.rooms) {
    const w = r.x1 - r.x0, d = r.z1 - r.z0;
    const cx = (r.x0 + r.x1) / 2, cz = (r.z0 + r.z1) / 2;
    const [tx, tz] = FLOOR_TILE[r.floor];
    const fgeo = scaleUV(new THREE.PlaneGeometry(w, d), w / tx, d / tz);
    const fl = m(fgeo, floorMats[r.floor], [cx, 0, cz], 'receive');
    fl.rotation.x = -Math.PI / 2;
    fl.name = 'floor-' + r.id;
    fl.userData.floorType = r.floor;
    g.add(fl);
    if (r.ceiling !== false) {
      const ce = m(new THREE.PlaneGeometry(w, d), ceilMat, [cx, H, cz], 'both');
      ce.rotation.x = Math.PI / 2;
      ce.userData.noMerge = true;
      ce.userData.isCeiling = true;
      ceilingMeshes.push(ce);
      g.add(ce);
    }
  }

  /* ---------------- 墙 ---------------- */

  /** 一块墙板：axis='z' → 板在 XY 面、法线指 dir*Z；axis='x' → 法线指 dir*X。
   *  uOff = 这段沿墙的米制起点 / WALL_TILE——同一面墙的多段板（门洞两侧/上方）
   *  用连续寻址，墙纸条纹才能跨缝对齐；各自从 0 起铺会在每条拼缝上断纹。 */
  const piece = (axis: 'x' | 'z', at: number, a0: number, a1: number, y0: number, y1: number, dir: 1 | -1, uOff = 0) => {
    const len = a1 - a0, cy = (y0 + y1) / 2, c = (a0 + a1) / 2;
    if (len < 0.01 || y1 - y0 < 0.01) return;
    // 墙纸按米制寻址：墙有多长多高，UV 就铺几张贴图，墙纸图案尺度与墙段尺寸无关。
    const pgeo = scaleUV(new THREE.PlaneGeometry(len, y1 - y0), len / WALL_TILE, (y1 - y0) / WALL_TILE);
    if (uOff) {
      const uv = pgeo.attributes.uv;
      for (let i = 0; i < uv.count; i++) uv.setX(i, uv.getX(i) + uOff);
    }
    let mesh: THREE.Mesh;
    if (axis === 'z') {
      mesh = m(pgeo, wallMat, [c, cy, at], 'both');
      if (dir < 0) mesh.rotation.y = Math.PI;
    } else {
      mesh = m(pgeo, wallMat, [at, cy, c], 'both');
      mesh.rotation.y = dir > 0 ? Math.PI / 2 : -Math.PI / 2;
    }
    // 标记法线方向：世界空间中这面墙"朝向"哪一侧
    mesh.userData.isWallPiece = true;
    mesh.userData.wallAxis = axis;
    mesh.userData.wallAt = at;       // 墙所在轴线坐标
    mesh.userData.wallDir = dir;     // 法线方向（+1/-1）
    mesh.userData.noMerge = true;    // 墙板不参与合批，保留独立 visible 控制
    wallMeshes.push(mesh);
    g.add(mesh);
  };

  /**
   * 洞口侧壁：四圈窄面，法线朝洞口中心。mid = 侧壁所在平面（墙厚的中间）。
   * target 为 null 表示这一侧不生成断面（内墙两侧只需生成一次，见下方调用处）。
   */
  const reveal = (axis: 'x' | 'z', mid: number, op: WallOpening, target: THREE.Group | null) => {
    if (!target) return;
    const ow = op.b - op.a, oh = op.y1 - op.y0;
    const ca = (op.a + op.b) / 2;
    const mk = (geo: THREE.PlaneGeometry, pos: [number, number, number], rx: number, ry: number) => {
      const mesh = m(geo, revealMat, pos, 'both');
      if (rx) mesh.rotation.x = rx;
      if (ry) mesh.rotation.y = ry;
      target.add(mesh);
    };
    if (axis === 'z') {
      if (op.y0 > 0.01) mk(new THREE.PlaneGeometry(ow, t), [ca, op.y0, mid], -Math.PI / 2, 0);   // 窗台下侧壁
      if (H - op.y1 > 0.01) mk(new THREE.PlaneGeometry(ow, t), [ca, op.y1, mid], Math.PI / 2, 0); // 门楣下侧壁
      mk(new THREE.PlaneGeometry(t, oh), [op.a, (op.y0 + op.y1) / 2, mid], 0, Math.PI / 2);
      mk(new THREE.PlaneGeometry(t, oh), [op.b, (op.y0 + op.y1) / 2, mid], 0, -Math.PI / 2);
    } else {
      if (op.y0 > 0.01) mk(new THREE.PlaneGeometry(ow, t), [mid, op.y0, ca], -Math.PI / 2, 0);
      if (H - op.y1 > 0.01) mk(new THREE.PlaneGeometry(ow, t), [mid, op.y1, ca], Math.PI / 2, 0);
      mk(new THREE.PlaneGeometry(t, oh), [mid, (op.y0 + op.y1) / 2, op.a], 0, Math.PI / 2);
      mk(new THREE.PlaneGeometry(t, oh), [mid, (op.y0 + op.y1) / 2, op.b], 0, -Math.PI / 2);
    }
  };

  /** 沿墙切洞：返回全高段 + 处理下坎/门楣/侧壁。uOff 传每段的米制起点，
   *  让同一条 run 的所有墙段墙纸连续寻址（拼接缝处图案不断纹）。 */
  const buildWallRun = (
    axis: 'x' | 'z',
    at: number,
    dir: 1 | -1,
    from: number,
    to: number,
    ops: WallOpening[],
    revealMid: number,
    revealTarget: THREE.Group | null
  ) => {
    const sorted = [...(ops ?? [])].sort((p, q) => p.a - q.a);
    let cursor = from;
    for (const op of sorted) {
      if (op.a > cursor) piece(axis, at, cursor, op.a, 0, H, dir, cursor / WALL_TILE);
      if (op.y0 > 0.01) piece(axis, at, op.a, op.b, 0, op.y0, dir, op.a / WALL_TILE);
      if (H - op.y1 > 0.01) piece(axis, at, op.a, op.b, op.y1, H, dir, op.a / WALL_TILE);
      reveal(axis, revealMid, op, revealTarget);
      cursor = Math.max(cursor, op.b);
    }
    if (to > cursor) piece(axis, at, cursor, to, 0, H, dir, cursor / WALL_TILE);
  };

  /** 内墙的洞口断面：两侧房间都看得见，不能跟着任何一侧剔除，所以单独一个组。 */
  const interiorReveals = new THREE.Group();
  g.add(interiorReveals);

  for (const w of layout.shell.walls) {
    const ops = w.openings ?? [];
    if (w.exterior) {
      const dir = (w.dir ?? 1) as 1 | -1;
      const ws = decorFor(w.id, w.axis, w.at, dir);
      // 外墙：名义线就是内表面，洞口侧壁向外侧探出 t/2；断面归这一侧，跟着墙剔除
      buildWallRun(w.axis, w.at, dir, w.from, w.to, ops, w.at - dir * t / 2, ws.reveals);
    } else {
      // 内墙：名义线两侧各让出 t/2，各朝一侧房间；洞口侧壁铺满中间的墙厚。
      // 两侧的 run 传进来的 revealMid 都是 w.at，断面位置完全重合——
      // 生成两份就是精确共面的重复面（z-fighting），只让第一份生成。
      buildWallRun(w.axis, w.at - t / 2, -1, w.from, w.to, ops, w.at, interiorReveals);
      buildWallRun(w.axis, w.at + t / 2, 1, w.from, w.to, ops, w.at, null);
      decorFor(`${w.id}:a`, w.axis, w.at - t / 2, -1);
      decorFor(`${w.id}:b`, w.axis, w.at + t / 2, 1);
    }
  }

  /* ---------------- 墙角密封柱 ---------------- */

  /**
   * 墙体是零厚度平面：外墙角只是两块板的线接触，内墙角靠 ±t/2 的互咬腔封住。
   * 前者在掠射角会沿缝看穿（对角线漏一丝"外面"），后者顺缝瞥进墙腔读成黑缝。
   * 在每条墙线交点立一根密封柱：
   *  - 双内墙交叉：柱边 = 墙厚 + 4mm，柱面比墙片 proud 2mm，不与任何墙片共面，
   *    一根填实互咬腔；
   *  - 任一方是外墙（外墙只有一张贴线平面）：柱边缩到 3cm，柱面最多 proud 1.5cm，
   *    刚好封住线接触的对角缝又不读出柱体。
   * 柱面用同一份墙纸材质 + 米制 UV，与墙片同花纹。
   */
  {
    const seen = new Set<string>();
    const postGeoInterior = scaleUV(new THREE.BoxGeometry(t + 0.004, H, t + 0.004), (t + 0.004) / WALL_TILE, H / WALL_TILE);
    const postGeoEdge = scaleUV(new THREE.BoxGeometry(0.03, H, 0.03), 0.03 / WALL_TILE, H / WALL_TILE);
    for (const w of layout.shell.walls) {
      for (const v of layout.shell.walls) {
        if (w.axis === v.axis) continue;
        const px = w.axis === 'z' ? v.at : w.at;
        const pz = w.axis === 'z' ? w.at : v.at;
        const wCovers = px >= Math.min(w.from, w.to) - 0.01 && px <= Math.max(w.from, w.to) + 0.01;
        const vCovers = pz >= Math.min(v.from, v.to) - 0.01 && pz <= Math.max(v.from, v.to) + 0.01;
        if (!wCovers || !vCovers) continue;
        const key = `${px.toFixed(2)}|${pz.toFixed(2)}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const edge = w.exterior || v.exterior;
        const post = m(edge ? postGeoEdge : postGeoInterior, wallMat, [px, H / 2, pz], 'none');
        post.userData.noMerge = true;
        g.add(post);
      }
    }
  }

  /* ---------------- 阳台栏杆 ---------------- */

  if (layout.shell.railing) {
    // 203 的阳台栏杆和邻居户是同一条阳台线，从街上看必须连成一道——
    // 早先它是米白（#f4f0e8 / #e8e2d6），在冷灰立面中间断成两截亮白，
    // 是"立面被切成两种材质"的直接来源。这里跟 exterior.ts 的 EXT.railing
    // 取同一组炭灰金属色，palette 里可用 railing / railTop 覆盖。
    const railMat = toon('#354b48', { finish: 'metal' });
    const slatMat = toon('#9c7954');
    const postMat = toon('#354b48', { finish: 'metal' });
    const rh = 1.05; // 栏杆高度
    for (const r of layout.shell.railing) {
      const len = r.to - r.from;
      const c = (r.from + r.to) / 2;
      // 立柱间距 ~0.55m
      const n = Math.max(2, Math.ceil(len / 2.05) + 1);
      for (let i = 0; i < n; i++) {
        const p = r.from + (len * i) / (n - 1);
        const post = m(box(0.055, rh, 0.055), postMat,
          r.axis === 'z' ? [p, rh / 2, r.at] : [r.at, rh / 2, p], 'cast');
        g.add(post);
      }
      // 顶扶手 + 底横档 + 竖栅
      const rail = m(box(r.axis === 'z' ? len : 0.07, 0.05, r.axis === 'z' ? 0.07 : len), railMat,
        r.axis === 'z' ? [c, rh, r.at] : [r.at, rh, c], 'cast');
      g.add(rail);
      g.add(m(box(r.axis === 'z' ? len : 0.05, 0.04, r.axis === 'z' ? 0.05 : len), postMat,
        r.axis === 'z' ? [c, 0.08, r.at] : [r.at, 0.08, c], 'none'));
      for (let p = r.from + 0.14; p < r.to - 0.05; p += 0.14) {
        g.add(m(box(r.axis === 'z' ? 0.068 : 0.046, rh - 0.2, r.axis === 'z' ? 0.046 : 0.068), slatMat,
          r.axis === 'z' ? [p, (rh - 0.1) / 2 + 0.06, r.at] : [r.at, (rh - 0.1) / 2 + 0.06, p], 'none'));
      }
    }
  }

  return { group: g, wallSides, wallMeshes, ceilingMeshes, interiorReveals };
}

/* ============================================================================
 * 雨
 * ========================================================================== */

export type RainConfig = {
  count: number;
  area: { x: [number, number]; y: [number, number]; z: [number, number] };
  /** 雨不能落进房间里，这个盒子里的点会被丢掉重摇。 */
  exclude?: { min: [number, number, number]; max: [number, number, number] };
  /** 雨区跟随焦点：area.x / area.z 改解为「相对焦点的偏移」（通常对称，如 [-12, 12]），
   *  group 每帧平移到焦点的水平位置，雨滴用等量反向位移补偿，于是雨滴的**世界坐标**
   *  保持连续——雨不会"黏"在屏幕上跟着镜头走，只有飘出局部盒的才回绕补充。
   *  这样不必铺满整个室外场景，实例数可以砍掉一大截。
   *  焦点由调用方每帧给：第一人称是玩家自身，观察者模式是注视点——俯视时相机
   *  在几十米外的高空，雨要落在被看的房子周围，而不是落在相机脚下。
   *  默认 false = 固定世界区域。 */
  followCamera?: boolean;
  /** 雨滴下落速度（米/秒）。 */
  speed: number;
  /** 水平风速（米/秒），与 speed 一起决定雨丝倾角 ≈ atan2(wind, speed)。
   *  默认 1.5，配合 speed 6.0 给出约 14° 的斜雨。日式动画常用 10–20°。 */
  wind?: number;
  /** 中景雨丝基础长度（米）。近层 ×nearLenScale / 远层 ×farLenScale。默认 0.13。 */
  streakLen?: number;
  /** 中景雨丝基础宽度（米）。默认 0.014。 */
  streakWidth?: number;
  /** 纵深分层数，默认 3。 */
  layers?: number;
  /** 最远档长度缩放，默认 0.40。 */
  farLenScale?: number;
  /** 最远档宽度缩放，默认 0.55。 */
  farWidScale?: number;
  /** 最远档亮度缩放（颜色倍率），默认 0.30。 */
  farBrightScale?: number;
  /** 最近档长度缩放（相对中景），默认 2.2。 */
  nearLenScale?: number;
  /** 最近档宽度缩放，默认 3.0。 */
  nearWidScale?: number;
  /** 最近档亮度缩放，默认 1.15。 */
  nearBrightScale?: number;

  /** 兼容字段（旧版 size / opacity / farSize / farOpacity / wind=横向漂移）的别名映射，
   *  让旧配置不用改也能工作。 */
  /** @deprecated 用 streakLen */
  size?: number;
  /** @deprecated 用 nearBrightScale */
  opacity?: number;
  /** @deprecated 用 farLenScale */
  farSize?: number;
  /** @deprecated 用 farBrightScale */
  farOpacity?: number;
};

type RainLayer = {
  mesh: THREE.InstancedMesh;
  count: number;
  /** 每实例的世界坐标（每帧更新） */
  px: Float32Array; py: Float32Array; pz: Float32Array;
  /** 每实例的当前下落速度（米/秒），决定倾角和帧位移 */
  spd: Float32Array;
  /** 每实例的长/宽缩放（在基础矩形尺寸上的倍率，制造"长长短短"的差异） */
  lenScale: Float32Array;
  widScale: Float32Array;
  /** 每实例的亮度（已经预乘到 instanceColor 里） */
  bright: Float32Array;
  /** z 带（每实例落回顶端时只在带内重摇） */
  band: [number, number];
  /** x 范围（落回顶端时在范围内重摇） */
  areaX: [number, number];
  yRange: [number, number];
  /** 水平风速（米/秒，整层共享） */
  wind: number;
};

/**
 * 一场雨。
 *
 * 用 InstancedMesh 而不是 Points：Points 的方块永远正对相机，纹理再竖也是
 * 屏幕上永远竖的——做不出斜雨。InstancedMesh 给每根雨丝一个独立的旋转矩阵，
 * 跟着风速倾斜，就有了日式动画里那种"被风斜吹过来的雨"。
 *
 * 三个 z 带 × 三种外观：
 *  - 最近（z 最大那一段）：软椭圆纹理 + 大尺寸 + 较亮 → "景前大雨滴"，营造
 *    "摄像机正站在雨里"的近距虚化感。这是参考图里最显眼的层次。
 *  - 中景（中间）：细条纹理 + 中等长度 → 雨幕主体。
 *  - 远景（z 最小那一段）：细条 + 短 + 暗 + 数量多 → "雨幕"空气透视。
 *
 * 倾角：rotation.z = atan2(wind, spd[i])，每实例随自身速度略有差异（≈ 11–16°），
 * 自然不整齐。
 *
 * 长度变化：每实例 lenScale ∈ [0.75, 1.25]，制造"长雨丝 + 中等 + 小雨丝"混合。
 */
export function buildRain(cfg: RainConfig): { object: THREE.Group; update: (dt: number) => void } {
  const [x0, x1] = cfg.area.x;
  const [y0, y1] = cfg.area.y;
  const [z0, z1] = cfg.area.z;
  const ex = cfg.exclude;
  /** 落入"有顶房间"并集包围盒内（室内/楼下实心体量）——这些地方不该有雨。
   *  注意：判定是整盒（含 x/z 柱 + y 区间），所以檐上(y 在盒顶之上)的雨仍会
   *  落、只是被不透明楼体挡住看不见；真正要清掉的是盒内那段（玩家站在 203
   *  里能看到的那截）。 */
  const inExclude = (x: number, y: number, z: number): boolean =>
    !!ex &&
    x > ex.min[0] && x < ex.max[0] &&
    y > ex.min[1] && y < ex.max[1] &&
    z > ex.min[2] && z < ex.max[2];
  const speed = cfg.speed;
  // 兼容映射：旧的 size/opacity/farSize/farOpacity 也吃进来
  const baseLen   = cfg.streakLen   ?? cfg.size     ?? 0.13;
  const baseWid   = cfg.streakWidth               ?? 0.014;
  const nearLenK  = cfg.nearLenScale              ?? 2.2;
  const nearWidK  = cfg.nearWidScale              ?? 3.0;
  const nearBriK  = cfg.nearBrightScale           ?? 1.15;
  const farLenK   = cfg.farLenScale    ?? cfg.farSize     ?? 0.40;
  const farWidK   = cfg.farWidScale                ?? 0.55;
  const farBriK   = cfg.farBrightScale ?? cfg.farOpacity  ?? 0.30;
  const baseBri   = cfg.nearBrightScale ? 1.0 : (cfg.opacity ?? 0.28);
  // 真正用的亮度基准 = 中景亮度 = near / nearBriK；远层 ×farBriK；近层 ×nearBriK
  const midBri    = baseBri / nearBriK;
  const wind      = cfg.wind ?? 1.5;
  const layers    = Math.max(1, Math.floor(cfg.layers ?? 3));

  /** 层权重：近档密一些、远档也密（远景靠数量撑"雨幕"）。
   *  参考：近 18% / 中 50% / 远 32%。 */
  const weights: number[] = [];
  let wSum = 0;
  if (layers === 1) {
    weights.push(1); wSum = 1;
  } else if (layers === 3) {
    weights.push(0.18, 0.50, 0.32); wSum = 1;
  } else {
    for (let L = 0; L < layers; L++) {
      const t = L / (layers - 1);
      weights.push(1.4 - 0.7 * t);
      wSum += weights[L];
    }
  }

  const group = new THREE.Group();
  group.name = 'rain';
  const built: RainLayer[] = [];
  const band = (z1 - z0) / layers;
  const rnd = makeRng(20260831);

  const texStreak = rainStreakTexture();
  const texBlob = rainBlobTexture();

  const m = new THREE.Matrix4();
  const sV = new THREE.Vector3();
  const pV = new THREE.Vector3();
  const c = new THREE.Color();
  const baseCol = new THREE.Color().setRGB(0.82, 0.88, 0.94);   // 冷蓝偏白

  for (let L = 0; L < layers; L++) {
    const t = layers === 1 ? 0 : L / (layers - 1);
    // 长度 / 宽度 / 亮度：插值在 mid → far（t=1）或 mid → near（t=0）之间
    let quadH: number, quadW: number, briK: number;
    if (L === 0) {              // 最近
      quadH = baseLen * nearLenK;
      quadW = baseWid * nearWidK;
      briK = midBri * nearBriK;
    } else if (L === layers - 1) {  // 最远
      quadH = baseLen * farLenK;
      quadW = baseWid * farWidK;
      briK = midBri * farBriK;
    } else {                    // 中间
      // 3 层时只有 L=0/1/2；多于 3 层时按 t 线性
      const tt = layers === 1 ? 0 : (L / (layers - 1));
      quadH = baseLen * (nearLenK + (1 - nearLenK) * tt);
      quadW = baseWid * (nearWidK + (1 - nearWidK) * tt);
      briK = midBri * (nearBriK + (1 - nearBriK) * tt);
    }
    // 修正：layers=3 时 L=1 应是中景 base 长度，不是上面那公式的奇怪值
    if (layers === 3 && L === 1) {
      quadH = baseLen;
      quadW = baseWid;
      briK = midBri;
    }

    // L=0 占 z 最大那一段（相机在 +z），越远越往 -z
    const bz1 = z1 - (z1 - z0) * (L / layers);
    const bz0 = bz1 - band;
    const n = L === layers - 1
      ? cfg.count - built.reduce((s, l) => s + l.count, 0)
      : Math.round((cfg.count * weights[L]) / wSum);
    if (n <= 0) continue;

    const isNear = L === 0;
    const tex = isNear ? texBlob : texStreak;
    const geo = new THREE.PlaneGeometry(quadW, quadH);
    // 把矩形锚点挪到底边中点：drop 位置 = 矩形底边中点，雨丝沿 +Y 延伸（斜向上 = 上风方向）
    geo.translate(0, quadH / 2, 0);

    const mat = new THREE.MeshBasicMaterial({
      map: tex,
      color: 0xffffff,
      transparent: true,
      depthWrite: false,
      blending: THREE.NormalBlending,    // 透明度纹理别用 Additive——会烧白
      side: THREE.DoubleSide,
    });

    const mesh = new THREE.InstancedMesh(geo, mat, n);
    mesh.frustumCulled = false;
    mesh.renderOrder = 10 + (layers - L);   // 近层最后画（盖在远层上）
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);

    // instanceColor：亮度预乘进去（material.color=white，纹理已经是冷蓝）
    const colorArr = new Float32Array(n * 3);
    mesh.instanceColor = new THREE.InstancedBufferAttribute(colorArr, 3);
    mesh.instanceColor.setUsage(THREE.DynamicDrawUsage);

    const px = new Float32Array(n), py = new Float32Array(n), pz = new Float32Array(n);
    const spd = new Float32Array(n);
    const lenScale = new Float32Array(n), widScale = new Float32Array(n);
    const bright = new Float32Array(n);

    for (let i = 0; i < n; i++) {
      let x = 0, y = 0, z = 0;
      // 落在房间里的重摇，最多试 8 次，试不出来就认了
      for (let k = 0; k < 8; k++) {
        x = x0 + rnd() * (x1 - x0);
        y = y0 + rnd() * (y1 - y0);
        z = bz0 + rnd() * (bz1 - bz0);
        if (!ex) break;
        const inside =
          x > ex.min[0] && x < ex.max[0] &&
          y > ex.min[1] && y < ex.max[1] &&
          z > ex.min[2] && z < ex.max[2];
        if (!inside) break;
      }
      px[i] = x; py[i] = y; pz[i] = z;
      // 远档略压慢，制造"远处雨感觉在飘"的层次
      spd[i] = speed * (1 - 0.16 * t) * (0.85 + rnd() * 0.30);
      // 长度 0.75..1.25，宽度 0.85..1.15：避免"一整屏雨丝长得一样"
      lenScale[i] = 0.75 + rnd() * 0.50;
      widScale[i] = 0.85 + rnd() * 0.30;
      bright[i] = briK * (0.70 + rnd() * 0.55);

      const tilt = Math.atan2(wind, spd[i]);
      const qTilt = _qEulerZ.setFromAxisAngle(_zAxis, tilt);
      sV.set(widScale[i], lenScale[i], 1);
      pV.set(x, y, z);
      m.compose(pV, qTilt, sV);
      mesh.setMatrixAt(i, m);
      c.copy(baseCol).multiplyScalar(bright[i]);
      mesh.setColorAt(i, c);
    }
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;

    group.add(mesh);
    built.push({
      mesh, count: n, px, py, pz, spd, lenScale, widScale, bright,
      band: [bz0, bz1], areaX: [x0, x1], yRange: [y0, y1], wind,
    });
  }

  /* 跟随模式下 group 的水平平移量（= 焦点 xz）。不跟随时恒为 0，
   * px/py/pz 直接就是世界坐标。 */
  let originX = 0;
  let originZ = 0;
  let focusSeen = false;
  const follow = !!cfg.followCamera;
  const spanXFull = x1 - x0;
  const spanZFull = z1 - z0;

  const update = (dt: number, focus?: { x: number; z: number }): void => {
    /* —— 焦点跟随 ——
     * group 平移到焦点的水平位置，雨滴的局部坐标反向补偿同样的位移，于是雨滴的
     * **世界坐标**保持连续：雨不会"黏"在屏幕上跟着镜头走，只有飘出局部盒的才
     * 回绕到盒内补充。首帧、以及位移超过一整个盒宽（切模式瞬移 / 从室内恢复）
     * 时直接落位不补偿——那时要跨过整段距离回绕，等于把整场雨重洗一遍，
     * 不如让它原地套在焦点上。 */
    let shiftX = 0;
    let shiftZ = 0;
    if (follow && focus) {
      const dx = focus.x - originX;
      const dz = focus.z - originZ;
      if (focusSeen && Math.abs(dx) <= spanXFull && Math.abs(dz) <= spanZFull) {
        shiftX = dx;
        shiftZ = dz;
      }
      focusSeen = true;
      originX = focus.x;
      originZ = focus.z;
      group.position.set(originX, 0, originZ);
    }

    const span = y1 - y0;
    for (const L of built) {
      const attr = L.mesh.instanceMatrix;
      const arr = attr.array as Float32Array;
      const spanX = L.areaX[1] - L.areaX[0];
      const spanZ = L.band[1] - L.band[0];
      for (let i = 0; i < L.count; i++) {
        // 抵消 group 的平移，让雨滴在世界空间里延续原来的轨迹
        L.px[i] -= shiftX;
        L.pz[i] -= shiftZ;

        // 位置更新：水平风恒速，下落速度按每实例
        L.px[i] += L.wind * dt;
        L.py[i] -= L.spd[i] * dt;
        if (L.py[i] < L.yRange[0]) {
          L.py[i] += span;
          L.px[i] = L.areaX[0] + Math.random() * spanX;
          L.pz[i] = L.band[0] + Math.random() * spanZ;
        }
        // 局部盒内回绕（取模，任意大小的位移都能正确绕回，不怕瞬移）
        if (L.px[i] < L.areaX[0] || L.px[i] > L.areaX[1]) {
          L.px[i] = L.areaX[0] + (((L.px[i] - L.areaX[0]) % spanX) + spanX) % spanX;
        }
        if (L.pz[i] < L.band[0] || L.pz[i] > L.band[1]) {
          L.pz[i] = L.band[0] + (((L.pz[i] - L.band[0]) % spanZ) + spanZ) % spanZ;
        }

        // 室内/实心体量禁区：每帧复核。px/pz 是相对焦点的局部坐标，判定要还原
        // 成世界坐标（group 只做水平平移，加回 origin 即可）。落进盒内的雨
        // （从檐上落下、或横向风飘进来的）立刻重摇到盒外，保证"室内不是下雨
        // 区"——否则玩家站在 203 里会看到雨丝穿堂而过。
        if (inExclude(L.px[i] + originX, L.py[i], L.pz[i] + originZ)) {
          for (let k = 0; k < 8; k++) {
            L.px[i] = L.areaX[0] + Math.random() * spanX;
            L.py[i] = L.yRange[0] + Math.random() * span;
            L.pz[i] = L.band[0] + Math.random() * spanZ;
            if (!inExclude(L.px[i] + originX, L.py[i], L.pz[i] + originZ)) break;
          }
        }

        // 重新合成矩阵（位置 + 倾角 + 缩放）
        const tilt = Math.atan2(L.wind, L.spd[i]);
        _qEulerZ.setFromAxisAngle(_zAxis, tilt);
        sV.set(L.widScale[i], L.lenScale[i], 1);
        pV.set(L.px[i], L.py[i], L.pz[i]);
        m.compose(pV, _qEulerZ, sV);
        m.toArray(arr, i * 16);
      }
      attr.needsUpdate = true;
    }
  };

  return { object: group, update };
}

// 共享给所有帧的临时对象，省得每实例每帧 new Quaternion
const _zAxis = new THREE.Vector3(0, 0, 1);
const _qEulerZ = new THREE.Quaternion();

/* ============================================================================
 * 湿地面反光
 * ========================================================================== */

export type PoolSpec = {
  pos: [number, number];
  size: number;
  color: string;
  opacity: number;
  /** 沿 Y 轴的旋转，让光斑不是清一色的正圆。 */
  rot?: number;
  /**
   * 'pool'（默认）= 灯脚下一团圆光晕；
   * 'streak' = 湿地上被拉长的倒影，沿 z 方向拖出一条软光带。
   * 夜里站在街上看，湿路面的主角是后者，所以沿街那几处都用 streak。
   */
  shape?: 'pool' | 'streak';
  /** streak 的长度（米）。省略时按 size * 5 估一个。 */
  len?: number;
};

/**
 * 湿地面的反光。
 *
 * 这里不做真正的镜面反射（要再渲一遍整个场景，代价太大），做的是动画背景里
 * 真正被用的那种画法：在每个光源正下方的地面上铺一团软光晕，再撒一层会闪的
 * 碎光点。物理上"不对"，但视觉上这就是湿沥青该有的样子，而且从任何角度看都成立
 * ——真镜面反而只有在特定角度才对。
 */
export function buildWetGround(
  pools: PoolSpec[],
  opts: {
    sparkleCount?: number;
    area?: number;
    /**
     * 碎光点的铺开中心，默认原点。
     *
     * 必须能指定：这套房间只有阳台（z 4.7..6.3）是露天的，而碎光点画在
     * y=0.008——比室内地板（y=0）高 8mm。不挪中心的话，它们会一路洒进客厅、
     * 主卧的地板上，变成"室内地上浮着一层会闪的蓝点"。
     */
    center?: [number, number];
  } = {}
): THREE.Group {
  const g = new THREE.Group();

  for (const p of pools) {
    const streak = p.shape === 'streak';
    const len = p.len ?? p.size * 5;
    const pool = new THREE.Mesh(
      // 平面先转到水平：局部 height 落在世界 z 上，所以拉长的是第二个参数
      new THREE.PlaneGeometry(p.size, streak ? len : p.size),
      new THREE.MeshBasicMaterial({
        map: streak ? wetStreakTexture() : lightPoolTexture(),
        color: new THREE.Color(p.color),
        transparent: true,
        opacity: p.opacity,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      })
    );
    pool.rotation.x = -Math.PI / 2;
    pool.rotation.z = p.rot ?? 0;
    // 拉长倒影从光源脚下朝观察者（+z）拖出去：几何中心要往 +z 挪半个长度
    pool.position.set(p.pos[0], 0.006, p.pos[1] + (streak ? len / 2 : 0));
    pool.renderOrder = 2;
    g.add(pool);
  }

  // 碎光点：湿地面上那些会随视角变化的小亮点，用固定点云近似
  const count = opts.sparkleCount ?? 0;
  const area = opts.area ?? 0;
  if (count > 0 && area > 0) {
    const [scx, scz] = opts.center ?? [0, 0];
    const pos = new Float32Array(count * 3);
    const rnd = makeRng(60214);
    for (let i = 0; i < count; i++) {
      pos[i * 3] = scx + (rnd() - 0.5) * area * 2;
      pos[i * 3 + 1] = 0.008;
      pos[i * 3 + 2] = scz + (rnd() - 0.5) * area * 2;
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    const sparkles = new THREE.Points(
      geo,
      new THREE.PointsMaterial({
        size: 0.035,
        map: softDotTexture(),
        color: new THREE.Color(C('sparkle', '#dde4ea')),
        transparent: true,
        opacity: 0.5,
        depthWrite: false,
        blending: THREE.AdditiveBlending,
        sizeAttenuation: true,
      })
    );
    sparkles.frustumCulled = false;
    g.add(sparkles);
  }

  return g;
}

/* ============================================================================
 * 公告板实例集（billboard instance set）
 * ========================================================================== */

/**
 * 一堆「各自要独立尺寸 / 独立透明度、且永远正对相机的小四边形」收成**一次提交**。
 *
 * 为什么不能继续用 Sprite：`SpriteMaterial.opacity` 是**材质级**的。要让 N 个
 * Sprite 各自淡入淡出，就必须给它们各建一份材质 —— 于是 N 个 Sprite 换来
 * N 次 draw call、N 次材质切换、N 个透明队列条目。实测全场景 309 个 Sprite
 * 全是这个来源（屋檐滴水 294 + 涟漪 7 + 店面滴水 8），是「对象数远多于
 * 三角形数」的典型：这 309 次提交一共只画 0 个三角形。
 *
 * 为什么不用 Points：`gl_PointSize` 有驱动上限（部分实现只到 64px），近景一滴
 * 水就能顶到上限被裁成方块；而且 Points 只能是正方形，做不出"细长水线"。
 *
 * 做法：一个手写的单位四边形装进 `InstancedBufferGeometry`，每实例三个属性：
 *   iPos   vec3   位置（组内局部坐标，随组变换）
 *   iSize  vec2   世界尺寸（宽, 高）
 *   iAlpha float  0..1 透明度
 * 顶点着色器在**视图空间**里把四边形摆开 —— 所以永远正对相机、纵轴对齐屏幕上方，
 * 与 Sprite 的朝向完全一致，但只占 1 次提交。
 *
 * 颜色/雾/色调映射与 `SpriteMaterial` 对齐：纹理乘 `uColor` 乘 `iAlpha`，再依次过
 * `<tonemapping_fragment>` / `<colorspace_fragment>` / `<fog_fragment>`——顺序与
 * three 的 sprite_frag 一致，所以换过来之后逐像素不变。
 *
 * **必须 `sceneCollideSkip`**：合批后的 mesh 横跨整栋楼，遍历收碰撞会收出一个
 * 罩住半条街的隐形墙。描边也不参与（材质 `transparent` 直接被 addOutline 跳过）。
 */
type BillboardSet = {
  mesh: THREE.Mesh;
  /** 写第 i 个实例的位置与尺寸（世界尺寸，米）。 */
  set(i: number, x: number, y: number, z: number, w: number, h: number): void;
  /** 只改尺寸（位置不变时用这个，省掉每帧重传位置缓冲）。 */
  setSize(i: number, w: number, h: number): void;
  /** 写第 i 个实例的透明度。 */
  setAlpha(i: number, a: number): void;
  /** 把改动过的实例属性缓冲标记为脏 —— 每帧改完调一次。 */
  flush(): void;
  /** 实例都摆好之后收一次包围球，让视锥剔除继续有效。 */
  finish(): void;
  dispose(): void;
};

function makeBillboardSet(
  count: number,
  map: THREE.Texture,
  color: THREE.ColorRepresentation,
  renderOrder: number
): BillboardSet {
  const geo = new THREE.InstancedBufferGeometry();
  /* 手写四边形而不是复用 PlaneGeometry：后者是带索引的 4 顶点网格，直接搬属性会
     和它的 dispose 语义纠缠（搬走属性后 dispose 会释放还在用的那份）。 */
  geo.setAttribute('position', new THREE.Float32BufferAttribute([
    -0.5, -0.5, 0, 0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0,
  ], 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute([0, 0, 1, 0, 1, 1, 0, 1], 2));
  geo.setIndex([0, 1, 2, 0, 2, 3]);

  const iPos = new THREE.InstancedBufferAttribute(new Float32Array(count * 3), 3);
  const iSize = new THREE.InstancedBufferAttribute(new Float32Array(count * 2), 2);
  const iAlpha = new THREE.InstancedBufferAttribute(new Float32Array(count), 1);
  iPos.setUsage(THREE.DynamicDrawUsage);
  iSize.setUsage(THREE.DynamicDrawUsage);
  iAlpha.setUsage(THREE.DynamicDrawUsage);
  geo.setAttribute('iPos', iPos);
  geo.setAttribute('iSize', iSize);
  geo.setAttribute('iAlpha', iAlpha);
  geo.instanceCount = count;

  const mat = new THREE.ShaderMaterial({
    uniforms: {
      uMap: { value: map },
      uColor: { value: new THREE.Color(color) },
      // fog 相关的 uniform 由 three 的 refreshFogUniforms 每帧写；这里只需占位，
      // 否则 material.fog === true 时 setProgram 找不到 uniform 会报错
      fogColor: { value: new THREE.Color(0xffffff) },
      fogNear: { value: 1 },
      fogFar: { value: 100 },
    },
    vertexShader: /* glsl */`
      attribute vec3 iPos;
      attribute vec2 iSize;
      attribute float iAlpha;
      varying vec2 vUv;
      varying float vAlpha;
      #include <common>
      #include <fog_pars_vertex>
      void main() {
        vUv = uv;
        vAlpha = iAlpha;
        // 视图空间里直接铺开：mv.xy 就是屏幕右/上两个方向，纵轴天然朝上
        vec4 mv = modelViewMatrix * vec4(iPos, 1.0);
        mv.xy += position.xy * iSize;
        vec4 mvPosition = mv;
        gl_Position = projectionMatrix * mvPosition;
        #include <fog_vertex>
      }
    `,
    fragmentShader: /* glsl */`
      uniform sampler2D uMap;
      uniform vec3 uColor;
      varying vec2 vUv;
      varying float vAlpha;
      #include <common>
      #include <fog_pars_fragment>
      void main() {
        vec4 tex = texture2D(uMap, vUv);
        gl_FragColor = vec4(uColor * tex.rgb, tex.a * vAlpha);
        // 全透明片元直接丢：省掉无谓的混合，也免得在深度预剔除之后还写一遍
        if (gl_FragColor.a < 0.004) discard;
        #include <tonemapping_fragment>
        #include <colorspace_fragment>
        #include <fog_fragment>
      }
    `,
    transparent: true,
    depthWrite: false,
    depthTest: true,
    blending: THREE.NormalBlending,
    fog: true,
  });
  mat.userData.outlineWeight = 0;

  const mesh = new THREE.Mesh(geo, mat);
  mesh.name = '__billboards';
  mesh.renderOrder = renderOrder;
  mesh.castShadow = false;
  mesh.receiveShadow = false;
  // 合批后横跨整栋楼，绝不能进碰撞表
  mesh.userData.sceneCollideSkip = true;
  mesh.userData.noMerge = true;
  // 视锥剔除不能靠 three 自己算：InstancedBufferGeometry 的 computeBoundingSphere
  // 只看基础四边形（半径 0.7），会把整批在屏幕外时误剔掉。finish() 里手工收球。
  mesh.frustumCulled = true;

  let posDirty = false, sizeDirty = false, alphaDirty = false;
  return {
    mesh,
    set(i, x, y, z, w, h) {
      iPos.setXYZ(i, x, y, z);
      iSize.setXY(i, w, h);
      posDirty = sizeDirty = true;
    },
    setSize(i, w, h) { iSize.setXY(i, w, h); sizeDirty = true; },
    setAlpha(i, a) { iAlpha.setX(i, a); alphaDirty = true; },
    flush() {
      if (posDirty) { iPos.needsUpdate = true; posDirty = false; }
      if (sizeDirty) { iSize.needsUpdate = true; sizeDirty = false; }
      if (alphaDirty) { iAlpha.needsUpdate = true; alphaDirty = false; }
    },
    finish() {
      iPos.needsUpdate = true; iSize.needsUpdate = true; iAlpha.needsUpdate = true;
      posDirty = sizeDirty = alphaDirty = false;
      /* 包围球：中心取位置均值，半径取「最远位置到中心 + 最大半尺寸」。
         偏保守没关系——多提交一次远好过在屏幕边缘把整批剔掉。 */
      const c = new THREE.Vector3();
      for (let i = 0; i < count; i++) c.x += iPos.getX(i), c.y += iPos.getY(i), c.z += iPos.getZ(i);
      c.divideScalar(Math.max(1, count));
      let r2 = 0, maxHalf = 0;
      for (let i = 0; i < count; i++) {
        const dx = iPos.getX(i) - c.x, dy = iPos.getY(i) - c.y, dz = iPos.getZ(i) - c.z;
        r2 = Math.max(r2, dx * dx + dy * dy + dz * dz);
        maxHalf = Math.max(maxHalf, iSize.getX(i), iSize.getY(i));
      }
      geo.boundingSphere = new THREE.Sphere(c, Math.sqrt(r2) + maxHalf * 0.5);
    },
    dispose() { geo.dispose(); mat.dispose(); },
  };
}

/* ============================================================================
 * 湿地涟漪（雨打积水）
 * ========================================================================== */

export type PuddleRippleOpts = {
  /** 撒开半边长（世界 x/z），默认 5.2 */
  area?: number;
  /** 撒开中心 [x, z]，默认 [0.3, 9.5]（街心湿地面） */
  center?: [number, number];
  /** 涟漪数量，默认 28 */
  count?: number;
  /** 离地高度（米），默认 0.014——刚好浮在湿地光斑之上 */
  y?: number;
  /** 固定随机种子，默认 98831 */
  seed?: number;
  /** 涟漪颜色，默认冷蓝白 '#a9c2e6' */
  color?: string;
};

/**
 * 湿地面上一圈圈"刚被雨点敲过"的同心环。
 *
 * 和 buildWetGround 那套静态光斑不同：涟漪是**动的**——每圈从一点冒出、
 * 向外扩张、同时淡出，然后再从别处冒出。正是这点连续的运动，让"地面是湿的、
 * 且雨还在下"这件事被读出来（静态光斑只能说明"曾经湿"）。
 *
 * 逐圈独立淡入淡出，所以每个涟漪要有**自己的透明度**——这正是过去必须「一个
 * Sprite 一份 SpriteMaterial」的原因，几十个涟漪就是几十次提交。现在改走
 * makeBillboardSet：透明度是逐实例属性，整片只占 1 次提交，视觉逐像素不变。
 *
 * 注意：返回对象必须加进**未冻结**的组（见 exterior.ts 的 buildStreetscapeRipples）。
 * 虽然现在改的是实例属性而不是矩阵、冻结了也不会定格，但保持这个约定——
 * 将来有人再往这里加"真的在动"的零件时不会踩坑。
 */
export function buildPuddleRipples(opts: PuddleRippleOpts = {}): { object: THREE.Group; update: (t: number) => void } {
  const area = opts.area ?? 5.2;
  const [cx, cz] = opts.center ?? [0.3, 9.5];
  const count = opts.count ?? 28;
  const y = opts.y ?? 0.014;
  const seed = opts.seed ?? 98831;
  const tex = puddleRippleTexture();

  const g = new THREE.Group();
  g.name = 'puddle-ripples';
  const rnd = makeRng(seed);

  /* 先按原来的随机数调用顺序把参数抽完，再建实例缓冲——顺序一乱，涟漪位置就整体变样 */
  type Ripple = { x: number; z: number; base: number; phase: number; period: number };
  const ripples: Ripple[] = [];
  for (let i = 0; i < count; i++) {
    const x = cx + (rnd() - 0.5) * area * 2;
    const z = cz + (rnd() - 0.5) * area * 2;
    const base = 0.42 + rnd() * 0.55; // 最终直径（米）
    ripples.push({
      x, z, base,
      phase: rnd(),                 // 错开起始相位
      period: 1.8 + rnd() * 2.4,    // 每圈 1.8~4.2s 一个生命周期
    });
  }

  const set = makeBillboardSet(count, tex, opts.color ?? '#a9c2e6', 3);
  for (let i = 0; i < count; i++) {
    const r = ripples[i];
    // 起始尺寸按 update 在 p=0 时的取值填，避免首帧闪一下
    set.set(i, r.x, y, r.z, r.base * 0.5, r.base * 0.5);
    set.setAlpha(i, 0);
  }
  set.finish();
  g.add(set.mesh);

  const update = (t: number) => {
    for (let i = 0; i < ripples.length; i++) {
      const r = ripples[i];
      // 生命周期进度 [0,1)，到头即重生（相位错开，不会整片同步冒）
      const p = (t / r.period + r.phase) % 1;
      // 扩张：从 0.5 倍长到 1.0 倍
      const grow = 0.5 + p * 0.5;
      const sc = r.base * grow;
      set.setSize(i, sc, sc);
      // 淡入淡出：中段最亮，首尾几乎不可见——像一圈圈水波涌起又平复
      set.setAlpha(i, Math.sin(p * Math.PI) * 0.5);
    }
    set.flush();
  };

  return { object: g, update };
}

/* ============================================================================
 * 屋檐滴水
 * ========================================================================== */

export type DripEdge = {
  /** 沿 x 的滴水段范围（世界坐标） */
  x0: number;
  x1: number;
  /** 外缘 z（滴水垂到这条线外侧一点点） */
  z: number;
  /** 檐口底标高（滴水从这里往下挂），世界 y */
  y: number;
  /** 这一段的滴水数量，省略按长度估算（每 ~1.4m 一滴） */
  count?: number;
};

/**
 * 阳台/雨棚外缘垂下来的一排细短水线 + 底端水珠。
 *
 * 屋檐滴水是雨夜最容易被忽略、缺了却不对劲的细节：没有它，阳台栏杆下沿是
 * 干的，整栋楼少了"雨刚顺着檐口淌下去"的那一下。这里把 dripTexture 贴到一排
 * 竖直四边形上，垂在阳台外缘正下方，并让它们轻微地伸缩/明暗起伏，
 * 像水珠在汇聚、欲滴未滴。
 *
 * 四边形在视图空间里铺开，纵轴天然对齐屏幕上方，所以无论相机怎么绕，水线都
 * 读成"竖直下垂"——与 Sprite 的朝向一致。同样走 NormalBlending + 低透明度，
 * 冷蓝不烧白。
 *
 * **这一处曾经是全场景提交量最大的单项之一。** 原先一滴水一个 Sprite，而
 * SpriteMaterial 的 opacity 是材质级的，要各自明暗起伏就只能各建一份材质：
 * 整栋楼 294 滴 = 294 次 draw call + 294 次材质切换 + 294 个透明队列条目，
 * 而它们合计只画 0 个三角形。改走 makeBillboardSet 之后整批 1 次提交，
 * 透明度走逐实例属性 —— 画面逐像素不变（颜色/雾/色调映射链与 sprite_frag 同序）。
 */
export function buildEaveDrips(edges: DripEdge[]): { object: THREE.Group; update: (t: number) => void } {
  const g = new THREE.Group();
  g.name = 'eave-drips';
  const tex = dripTexture();
  const rnd = makeRng(70711);

  /* 先按原来的随机数调用顺序把每一滴的参数抽完（顺序一乱，整排滴水就换位置），
     再按最终数量建实例缓冲。 */
  type Drip = { x: number; y: number; z: number; baseW: number; baseH: number; phase: number; freq: number };
  const drips: Drip[] = [];

  for (const e of edges) {
    const n = e.count ?? Math.max(3, Math.round((e.x1 - e.x0) / 1.4));
    for (let i = 0; i < n; i++) {
      const x = e.x0 + (i + 0.5 + (rnd() - 0.5) * 0.3) * (e.x1 - e.x0) / n;
      const w = 0.05 + rnd() * 0.04;
      const h = 0.18 + rnd() * 0.16;
      drips.push({
        // 顶点落在檐口底（e.y），整条往下挂
        x, y: e.y - h * 0.4, z: e.z,
        baseW: w, baseH: h,
        phase: rnd() * Math.PI * 2,
        freq: 0.6 + rnd() * 0.8,
      });
    }
  }

  const set = makeBillboardSet(drips.length, tex, '#bcd2ee', 4);
  for (let i = 0; i < drips.length; i++) {
    const d = drips[i];
    // 起始尺寸/透明度取 update 在 k 中位时的值，避免首帧闪一下
    set.set(i, d.x, d.y, d.z, d.baseW, d.baseH);
    set.setAlpha(i, 0.49);
  }
  set.finish();
  g.add(set.mesh);

  const update = (t: number) => {
    for (let i = 0; i < drips.length; i++) {
      const d = drips[i];
      const k = 0.5 + 0.5 * Math.sin(t * d.freq + d.phase);
      // 水珠汇聚时略伸长、变亮；将滴未滴时缩回、变淡
      set.setSize(i, d.baseW, d.baseH * (0.8 + 0.5 * k));
      set.setAlpha(i, 0.34 + 0.3 * k);
    }
    set.flush();
  };

  return { object: g, update };
}

/* ============================================================================
 * 路灯
 * ========================================================================== */

/**
 * 街边那盏灯。它是整个场景里最冷区域中唯一的一处暖光，
 * 和房间里透出来的暖光隔着雨呼应，是"孤独但依然温暖"这句情绪的具体载体。
 */
export function buildStreetLamp(
  pos: [number, number, number],
  height = 2.35,
  opts: { halo?: boolean } = {}
): THREE.Group {
  const g = new THREE.Group();
  const poleMat = toon(C('lampPole', '#4a5163'));

  g.add(m(cyl(0.115, 0.14, 0.05, 14), poleMat, [0, 0.025, 0], 'both'));
  g.add(m(cyl(0.038, 0.052, height, 12), poleMat, [0, height / 2, 0], 'cast'));
  g.add(m(cyl(0.056, 0.056, 0.035, 12), poleMat, [0, height * 0.06, 0], 'cast'));

  // 横挑臂 + 灯头
  const armLen = 0.30;
  const arm = m(cyl(0.026, 0.030, armLen, 10), poleMat, [armLen / 2, height - 0.03, 0], 'cast');
  arm.rotation.z = Math.PI / 2;
  g.add(arm);
  g.add(m(box(0.24, 0.075, 0.19), poleMat, [armLen, height - 0.075, 0], 'cast'));
  g.add(m(box(0.20, 0.030, 0.15), toon('#e3e8ee'), [armLen, height - 0.125, 0], 'cast'));

  // 灯泡：自发光本体 + 一圈光晕。
  // 光晕用 Sprite 而不是 Plane —— 场景是可以绕着转的，Plane 转到侧面就变成一条线了，
  // Sprite 永远正对相机，从哪个角度看都是一团光。
  // halo:false 时省掉这圈 Sprite（透明叠加件）——灯泡本体亮度过 bloom threshold
  // 后由泛光给光晕，适合对透明材质数量有预算的外景层。
  const bulbY = height - 0.145;
  g.add(m(sph(0.045, 12, 10), emissive('#e3d3b4'), [armLen, bulbY, 0], 'none'));
  if (opts.halo !== false) {
    const glow = new THREE.Sprite(
      new THREE.SpriteMaterial({
        map: lightPoolTexture(),
        color: new THREE.Color('#dcc79c'),
        transparent: true,
        opacity: 0.55,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      })
    );
    glow.scale.setScalar(0.62);
    glow.position.set(armLen, bulbY, 0);
    g.add(glow);
  }

  const light = new THREE.PointLight(0xffc887, 1.5, 6.0, 2);
  light.position.set(armLen, bulbY - 0.05, 0);
  g.add(light);

  // 灯下地面那一团湿反光。交给 buildWetGround 统一铺，这里只标位置
  g.userData.poolAt = [pos[0] + armLen, pos[2]];

  g.position.set(pos[0], 0, pos[2]);
  return g;
}

/* ============================================================================
 * 窗
 * ========================================================================== */

/**
 * 窗口。spec.size 在这里的含义和其他家具不同：
 *   size = [洞口宽, 窗台高, 窗顶高]  —— 因为窗口是"墙上的洞"，高度是绝对标高而不是尺寸
 */
export function buildWindow(
  spec: { pos: [number, number, number]; size: [number, number, number] }
): THREE.Group {
  const g = new THREE.Group();
  const wallSurface = new THREE.Group(); wallSurface.name = 'window-wall';
  const roomSide = new THREE.Group(); roomSide.name = 'room-side';
  g.add(wallSurface, roomSide);
  const [w, y0, y1] = spec.size, h = y1-y0, cy = (y0+y1)/2;
  // Local +Z faces indoors. The trim starts 12 mm ahead of the wall;
  // glass, sash and fabric occupy separate depths, including their folds.
  const frame = new THREE.MeshStandardMaterial({color:'#8b897e',roughness:.42,metalness:.3});
  const seal = new THREE.MeshStandardMaterial({color:'#494d48',roughness:.85});
  const sill = new THREE.MeshStandardMaterial({color:'#c9b596',roughness:.7});
  const cloth = new THREE.MeshStandardMaterial({color:'#faf5e9',map:curtainTexture(),roughness:1,side:THREE.DoubleSide});
  const glass = new THREE.MeshStandardMaterial({color:'#b9d7d8',roughness:.14,metalness:.12,transparent:true,opacity:.085,depthWrite:false,side:THREE.DoubleSide});
  for (const material of [frame,seal,sill,cloth,glass]) { material.name='203-window'; material.userData.outlineWeight=0; }
  const bar=(ww:number,hh:number,dd:number,x:number,y:number,z:number,mat:THREE.Material=frame,parent=wallSurface)=>{
    const mesh=m(box(ww,hh,dd),mat,[x,y,z],'both');mesh.userData.noOutline=true;parent.add(mesh);return mesh;
  };
  // Slim perimeter: horizontal pieces meet verticals without intersecting.
  const fw=.036;
  for(const sign of [-1,1]) {
    bar(w+fw*2,fw,.065,0,sign<0?y0-fw/2:y1+fw/2,.0445);
    bar(fw,h,.065,sign*(w/2+fw/2),cy,.0445);
    bar(.012,h-.024,.023,sign*(w/2-.006),cy,.017,seal);
  }
  for(const yy of [y0+.006,y1-.006])bar(w,.012,.023,0,yy,.017,seal);
  const split = w*.12;
  bar(.025,h-.024,.043,split,cy,.048);
  // A fixed picture pane and a narrower operable sash; clear view at eye level.
  for(const [left,right] of [[-w/2+.012,split-.014],[split+.014,w/2-.012]]){
    const pane=m(new THREE.PlaneGeometry(right-left,h-.026),glass,[(left+right)/2,cy,-.009],'none');
    pane.userData.noOutline=true;wallSurface.add(pane);
  }
  bar(.009,.105,.025,split+.035,cy-.1,.091,seal);
  bar(w+.14,.038,.225,0,y0-.06,.082,sill);
  const top=y1+.145;
  const roller=y0>=.8;
  if(roller){
    bar(w+.10,.047,.055,0,top,.165,sill,roomSide);
    const drop=y0>1.4?.21:.14;
    bar(w-.02,drop,.009,0,top-.035-drop/2,.169,cloth,roomSide);
    bar(w-.012,.015,.017,0,top-.043-drop,.17,frame,roomSide);
  }else{
    bar(w+.46,.035,.055,0,top+.018,.235,sill,roomSide);
    const bottom=Math.max(.17,y0-.16),height=top-bottom;
    for(const sign of [-1,1]){
      const width=.29;
      const geo=new THREE.PlaneGeometry(width,height,24,16);
      const p=geo.getAttribute('position') as THREE.BufferAttribute;
      for(let i=0;i<p.count;i++){
        const u=p.getX(i)/width+.5,v=(p.getY(i)+height/2)/height;
        // Soft tapered pleats, with a gently relaxed hem.
        p.setX(i,p.getX(i)*(1+.12*(1-v)));
        p.setZ(i,Math.sin(u*Math.PI*8)*(.018+.009*(1-v)));
        p.setY(i,p.getY(i)+Math.cos(u*Math.PI*8)*.007*(1-v));
      }
      geo.computeVertexNormals();
      const drape=m(geo,cloth,[sign*(w/2+.035),(top+bottom)/2,.23],'both');
      drape.userData.noOutline=true;roomSide.add(drape);
    }
  }
  return g;
}

/* ============================================================================
 * 书桌
 * ========================================================================== */

export function buildDesk(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const topY = H;

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const legMat = toon(C('deskLeg', '#b09070'));
  const panelMat = toon(C('deskPanel', '#b39377'));
  const darkMat = toon(C('dark', '#43434f'));
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });

  // 桌面 + 四周一圈稍深的封边
  g.add(m(box(W + 0.04, 0.045, D + 0.02), woodMat, [0, topY - 0.022, 0], 'both'));
  g.add(m(box(W + 0.05, 0.016, D + 0.03), panelMat, [0, topY - 0.05, 0], 'cast'));

  // 四条腿
  for (const sx of [-1, 1]) {
    for (const sz of [-1, 1]) {
      g.add(m(box(0.06, topY - 0.06, 0.06), legMat, [sx * (W / 2 - 0.06), (topY - 0.06) / 2, sz * (D / 2 - 0.06)], 'cast'));
    }
  }
  // 后挡板
  g.add(m(box(W - 0.14, 0.42, 0.03), panelMat, [0, topY - 0.30, -D / 2 + 0.03], 'cast'));

  // 右侧抽屉柜
  const dwW = 0.5, dwH = topY - 0.10;
  g.add(m(box(dwW, dwH, D - 0.08), panelMat, [W / 2 - dwW / 2 - 0.06, dwH / 2, 0], 'both'));
  for (let i = 0; i < 3; i++) {
    const dy = dwH * (0.18 + i * 0.29);
    g.add(m(box(dwW - 0.05, dwH * 0.24, 0.02), woodMat, [W / 2 - dwW / 2 - 0.06, dy, D / 2 - 0.02], 'cast'));
    g.add(m(cyl(0.012, 0.012, 0.12, 8), metalMat, [W / 2 - dwW / 2 - 0.06, dy, D / 2 + 0.03], 'cast').rotateZ(Math.PI / 2));
  }

  /* ---- 桌面上的东西 ---- */

  // 显示器
  const mon = new THREE.Group();
  mon.add(m(cyl(0.115, 0.13, 0.018, 20), darkMat, [0, 0.009, 0], 'cast'));
  // 立柱高度要和屏体下沿接上：屏体中心 y=0.45、自身高 0.43，绕 X 俯仰 0.05rad 后
  // 下沿落在 y≈0.235，同时往 -Z 缩 11mm（仍在立柱 50mm 的进深范围内）。
  // 原来只有 0.20 高（顶端 0.21），屏体底下空着 25mm，看着是断开的。
  mon.add(m(box(0.05, 0.24, 0.05), darkMat, [0, 0.13, 0], 'cast'));
  const panel = new THREE.Group();
  panel.add(m(box(0.70, 0.43, 0.03), darkMat, [0, 0, 0], 'cast'));
  const screen = m(new THREE.PlaneGeometry(0.63, 0.36), emissive('#ffffff', { map: screenTexture() }), [0, 0, 0.019], 'none');
  panel.add(screen);
  panel.position.set(0, 0.45, 0);
  panel.rotation.x = 0.05;
  mon.add(panel);
  mon.position.set(-0.05, topY, -0.10);
  g.add(mon);

  // 桌垫 + 键盘 + 鼠标
  g.add(m(box(0.66, 0.005, 0.26), toon('#e8d9c2'), [-0.05, topY + 0.003, 0.11], 'none'));
  g.add(m(box(0.46, 0.018, 0.15), toon('#f2efe6'), [-0.05, topY + 0.014, 0.11], 'cast'));
  g.add(m(box(0.055, 0.028, 0.085), toon('#f2efe6'), [0.30, topY + 0.019, 0.11], 'cast'));

  // 台灯：底座 + 两节灯臂 + 灯罩 + 亮着的灯泡
  const lampX = -W / 2 + 0.22;
  const lamp = new THREE.Group();
  lamp.add(m(cyl(0.075, 0.085, 0.022, 18), toon(C('accent', '#c2a49c')), [0, 0.011, 0], 'cast'));
  const arm1 = m(cyl(0.012, 0.012, 0.26, 8), toon(C('accent', '#c2a49c')), [0, 0.14, 0], 'cast');
  arm1.rotation.x = -0.22;
  lamp.add(arm1);
  const arm2 = m(cyl(0.011, 0.011, 0.20, 8), toon(C('accent', '#c2a49c')), [0.09, 0.28, 0], 'cast');
  arm2.rotation.z = 1.1;
  lamp.add(arm2);
  const shade = m(cyl(0.035, 0.082, 0.09, 14, true), toon('#fdf3e0', { side: THREE.DoubleSide }), [0.18, 0.31, 0.02], 'cast');
  shade.rotation.z = -0.55;
  lamp.add(shade);
  lamp.add(m(sph(0.026, 10, 8), emissive('#e8d9b8'), [0.20, 0.285, 0.03], 'none'));
  lamp.position.set(lampX, topY, -0.02);
  g.add(lamp);
  const lampLight = new THREE.PointLight(0xffd9a0, 0.35, 1.6, 2);
  lampLight.position.set(lampX + 0.20, topY + 0.30, 0.03);
  g.add(lampLight);

  // 马克杯
  const mug = new THREE.Group();
  mug.add(m(cyl(0.037, 0.033, 0.09, 16), toon('#fdf6f0'), [0, 0.045, 0], 'cast'));
  const handle = m(new THREE.TorusGeometry(0.024, 0.006, 6, 14), toon('#fdf6f0'), [0.042, 0.046, 0], 'cast');
  handle.rotation.y = Math.PI / 2;
  mug.add(handle);
  mug.add(m(cyl(0.033, 0.033, 0.004, 16), toon('#7a6552'), [0, 0.088, 0], 'none'));
  mug.position.set(0.44, topY, 0.02);
  g.add(mug);

  // 一摞书：整摞放进自己的 group，方便整体挪位置
  const stackRoot = new THREE.Group();
  const rnd = makeRng(1207);
  let by = 0;
  for (let i = 0; i < 4; i++) {
    const th = 0.022 + rnd() * 0.016;
    const col = ['#a86b5c', '#5c7488', '#6a9c78', '#b98f63'][i];
    const bk = m(box(0.19, th, 0.14), toon(col), [(rnd() - 0.5) * 0.03, by + th / 2, (rnd() - 0.5) * 0.03], 'cast');
    bk.rotation.y = (rnd() - 0.5) * 0.22;
    stackRoot.add(bk);
    by += th;
  }
  stackRoot.position.set(-W / 2 + 0.28, topY, 0.14);
  g.add(stackRoot);

  // 笔筒
  const cup = new THREE.Group();
  cup.add(m(cyl(0.036, 0.032, 0.095, 14), toon('#a4b8c8'), [0, 0.0475, 0], 'cast'));
  for (let i = 0; i < 3; i++) {
    const a = (i / 3) * Math.PI * 2;
    cup.add(m(cyl(0.005, 0.005, 0.14, 6), toon(['#c2a49c', '#ddcba8', '#6a9c78'][i]),
      [Math.cos(a) * 0.014, 0.085, Math.sin(a) * 0.014], 'none'));
  }
  cup.position.set(W / 2 - 0.72, topY, -0.02);
  g.add(cup);

  // 便签纸
  const paper = m(box(0.15, 0.006, 0.21), toon('#fffdf6'), [W / 2 - 0.45, topY + 0.004, 0.06], 'none');
  paper.rotation.y = -0.24;
  g.add(paper);

  return g;
}

/* ============================================================================
 * 椅子
 * ========================================================================== */

export function buildChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0];
  const seatY = 0.44;

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const cushionMat = toon(C('accent', '#c2a49c'));

  g.add(m(box(W, 0.055, W), woodMat, [0, seatY, 0], 'both'));
  g.add(m(box(W - 0.04, 0.05, W - 0.04), cushionMat, [0, seatY + 0.05, 0], 'cast'));

  const back = m(box(W - 0.02, 0.44, 0.05), woodMat, [0, seatY + 0.30, -W / 2 + 0.03], 'cast');
  back.rotation.x = 0.09;
  g.add(back);
  g.add(m(box(W - 0.10, 0.05, 0.06), woodMat, [0, seatY + 0.16, -W / 2 + 0.05], 'cast'));

  for (const sx of [-1, 1]) {
    for (const sz of [-1, 1]) {
      g.add(m(box(0.045, seatY, 0.045), woodMat, [sx * (W / 2 - 0.05), seatY / 2, sz * (W / 2 - 0.05)], 'cast'));
    }
  }
  // 横撑
  for (const sz of [-1, 1]) {
    g.add(m(box(W - 0.10, 0.03, 0.03), woodMat, [0, 0.14, sz * (W / 2 - 0.05)], 'cast'));
  }
  g.add(m(box(0.03, 0.03, W - 0.10), woodMat, [-(W / 2 - 0.05), 0.14, 0], 'cast'));

  return g;
}

/* ============================================================================
 * 床
 * ========================================================================== */

export function buildBed(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0];      // 沿 X 的宽
  const L = spec.size[2];      // 沿 Z 的长（床头在 -Z 端）

  const frameMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const sheetMat = toon(C('mattress', '#fdf8f0'));
  const blanketMat = toon(C('blanket', '#a4b8c8'));
  const pillowMat = toon(C('pillow', '#f6f1ea'));

  // 床架
  g.add(m(box(W, 0.24, L), frameMat, [0, 0.20, 0], 'both'));
  for (const sx of [-1, 1]) {
    for (const sz of [-1, 1]) {
      g.add(m(box(0.09, 0.08, 0.09), frameMat, [sx * (W / 2 - 0.07), 0.04, sz * (L / 2 - 0.07)], 'cast'));
    }
  }
  // 床头板 / 床尾板
  g.add(m(box(W, 0.72, 0.07), frameMat, [0, 0.52, -L / 2 - 0.02], 'both'));
  g.add(m(box(W - 0.06, 0.40, 0.06), frameMat, [0, 0.44, L / 2 + 0.02], 'both'));

  // 褥子
  g.add(m(box(W - 0.05, 0.16, L - 0.06), sheetMat, [0, 0.40, 0], 'both'));

  // 枕头（略微歪一点，别太对称，太对称会像样板间）
  const pillow = m(box(0.58, 0.14, 0.30), pillowMat, [0.02, 0.55, -L / 2 + 0.28], 'cast');
  pillow.rotation.y = 0.09;
  g.add(pillow);
  const pillow2 = m(box(0.34, 0.11, 0.24), toon('#f1ece3'), [-0.30, 0.53, -L / 2 + 0.30], 'cast');
  pillow2.rotation.y = -0.22;
  g.add(pillow2);

  // 被子（下半身盖着，上沿翻折一层）
  const bTop = 0.43;
  g.add(m(box(W + 0.01, 0.11, 1.16), blanketMat, [0, bTop + 0.055, -L / 2 + 0.92], 'both'));
  g.add(m(box(W + 0.015, 0.10, 0.13), toon('#f2ece4'), [0, bTop + 0.06, -L / 2 + 0.30], 'cast'));

  // 猫咪抱枕
  const plush = new THREE.Group();
  plush.add(m(sph(0.11, 12, 10), toon('#fdf6ee'), [0, 0.09, 0], 'cast'));
  const head = m(sph(0.085, 12, 10), toon('#fdf6ee'), [0, 0.215, 0.01], 'cast');
  plush.add(head);
  for (const s of [-1, 1]) {
    const ear = m(new THREE.ConeGeometry(0.032, 0.055, 8), toon('#f0e9e0'), [s * 0.05, 0.285, 0.01], 'cast');
    ear.rotation.z = s * 0.25;
    plush.add(ear);
  }
  for (const s of [-1, 1]) {
    plush.add(m(sph(0.012, 8, 6), toon('#544c4e'), [s * 0.032, 0.222, 0.078], 'none'));
  }
  plush.add(m(sph(0.010, 8, 6), toon('#c49a92'), [0, 0.198, 0.086], 'none'));
  const tail = m(cyl(0.014, 0.02, 0.16, 8), toon('#fdf6ee'), [0, 0.07, -0.10], 'cast');
  tail.rotation.x = 0.9;
  plush.add(tail);
  plush.position.set(-0.26, bTop + 0.10, -L / 2 + 0.42);
  plush.rotation.y = 0.5;
  g.add(plush);

  return g;
}

/* ============================================================================
 * 书架
 * ========================================================================== */

/** 一排竖着的书（沿 Z 排开，书脊朝 +X）。 */
function buildBookRow(len: number, depth: number, seed: number): THREE.Group {
  const g = new THREE.Group();
  const rnd = makeRng(seed);
  const colors = ['#a86b5c', '#5c7488', '#b98f63', '#6a9c78', '#7d7590', '#b08a80', '#4f8f95'];

  let z = -len / 2 + 0.02;
  while (z < len / 2 - 0.05) {
    const th = 0.026 + rnd() * 0.038;
    const hh = 0.16 + rnd() * 0.13;
    if (z + th > len / 2) break;
    const col = colors[Math.floor(rnd() * colors.length)];
    const bk = m(box(depth, hh, th), toon(col), [0, hh / 2, z + th / 2], 'cast');
    // 书脊上的烫金细线
    const band = m(box(depth + 0.002, 0.008, th * 0.9), toon('#dcc79c'), [0, hh * 0.82, z + th / 2], 'none');
    g.add(bk, band);
    z += th + 0.004;
  }

  // 末尾歪着靠一本
  if (rnd() > 0.4) {
    const lean = m(box(depth, 0.22, 0.045), toon(colors[Math.floor(rnd() * colors.length)]), [0, 0.11, len / 2 - 0.04], 'cast');
    lean.rotation.x = 0.30;
    g.add(lean);
  }

  return g;
}

export function buildShelf(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0];      // 进深（X）
  const H = spec.size[1];      // 高
  const Wz = spec.size[2];     // 宽（沿 Z）

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const backMat = toon(C('shelfBack', '#b09070'));

  // 背板 + 两侧板
  g.add(m(box(0.02, H, Wz), backMat, [-D / 2 + 0.01, H / 2, 0], 'both'));
  for (const s of [-1, 1]) {
    g.add(m(box(D, H, 0.026), woodMat, [0, H / 2, s * (Wz / 2 - 0.013)], 'both'));
  }

  // 层板：5 层
  const shelfYs = [0.02, 0.47, 0.92, 1.37, 1.80];
  for (const y of shelfYs) {
    g.add(m(box(D - 0.01, 0.026, Wz - 0.05), woodMat, [0.005, y, 0], 'both'));
  }

  // 第 1、2 层：书
  const r1 = buildBookRow(Wz - 0.10, D - 0.07, 21);
  r1.position.set(0.015, shelfYs[0] + 0.013, 0);
  g.add(r1);
  const r2 = buildBookRow(Wz - 0.10, D - 0.07, 88);
  r2.position.set(0.015, shelfYs[1] + 0.013, 0);
  g.add(r2);

  // 第 3 层：漫画合订本 + 一个小摆件
  const rnd = makeRng(404);
  let z = -Wz / 2 + 0.10;
  for (let i = 0; i < 5; i++) {
    const w = 0.05 + rnd() * 0.03;
    const col = ['#c2a49c', '#a4b8c8', '#ddcba8', '#a8b8ac', '#a89aa8'][i];
    g.add(m(box(D - 0.08, 0.20, w), toon(col), [0.015, shelfYs[2] + 0.113, z], 'cast'));
    z += w + 0.008;
  }
  /**
   * 一个小相框。
   *
   * 两层方向都要对：框体是 X 方向 12mm 的薄板，正面法线朝 ±X；而 PlaneGeometry
   * 默认躺在 XY 面、法线 +Z，直接塞进去会和框面互相垂直（照片横穿出框外）。
   * 所以照片自己要先绕 Y 转 90°，躺进框面所在的 YZ 平面、法线转到 +X。
   *
   * 整框朝 +X（书架开口方向，也就是朝着房间）立在第 3 层，并略微后仰靠住层板。
   * 底下要正好落在层板面上：层板顶面在 shelfYs[2]+0.013，框高 0.14，故中心取
   * shelfYs[2]+0.013+0.07（后仰角极小，cos 带来的高度损失可忽略）。
   */
  const fr = new THREE.Group();
  fr.add(m(box(0.012, 0.14, 0.11), toon('#fdf6ea'), [0, 0, 0], 'cast'));
  const photo = m(new THREE.PlaneGeometry(0.088, 0.118), emissive('#ffffff', { map: posterTexture() }), [0.008, 0, 0], 'none');
  photo.rotation.y = Math.PI / 2;
  fr.add(photo);
  fr.rotation.z = 0.09; // 绕 Z 正转 = 顶边往 -X 倒，也就是向后靠着层板
  fr.position.set(D / 2 - 0.05, shelfYs[2] + 0.083, Wz / 2 - 0.18);
  g.add(fr);

  // 第 4 层：收纳盒 + 杂志
  g.add(m(box(D - 0.09, 0.16, 0.30), toon('#a8b8ac'), [0.01, shelfYs[3] + 0.093, -Wz / 2 + 0.24], 'cast'));
  g.add(m(box(D - 0.09, 0.16, 0.30), toon('#ddcba8'), [0.01, shelfYs[3] + 0.093, -Wz / 2 + 0.58], 'cast'));
  for (let i = 0; i < 3; i++) {
    const mg = m(box(D - 0.10, 0.012, 0.21), toon(['#c2a49c', '#a4b8c8', '#f1ece3'][i]), [0.015, shelfYs[3] + 0.02 + i * 0.014, Wz / 2 - 0.20], 'cast');
    mg.rotation.x = i === 2 ? 0.04 : 0;
    g.add(mg);
  }

  // 顶上：绿植 + 一个小手办盒
  const plant = buildPlant(1.0);
  plant.position.set(0.02, H + 0.013, -Wz / 2 + 0.22);
  g.add(plant);

  const figure = new THREE.Group();
  figure.add(m(box(0.14, 0.14, 0.11), toon('#fdf6ea', { transparent: true, opacity: 0.55 }), [0, 0.07, 0], 'cast'));
  figure.add(m(box(0.11, 0.02, 0.09), toon('#c2a49c'), [0, 0.01, 0], 'cast'));
  figure.add(m(sph(0.035, 10, 8), toon('#f0e9e0'), [0, 0.055, 0], 'cast'));
  figure.add(m(cyl(0.022, 0.03, 0.06, 10), toon('#a4b8c8'), [0, 0.10, 0], 'cast'));
  figure.position.set(0.02, H + 0.013, Wz / 2 - 0.20);
  g.add(figure);

  return g;
}

/* ============================================================================
 * 冰箱
 * ========================================================================== */

export function buildFridge(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];

  const bodyMat = toon(C('fridge', '#f4f1e8'));
  const doorMat = toon(C('fridgeDoor', '#eae5d8'));
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });

  g.add(m(box(W - 0.02, H, D), bodyMat, [-0.01, H / 2, 0], 'both'));
  // 门（朝 +X，也就是朝房间）
  g.add(m(box(0.026, H - 0.06, D - 0.03), doorMat, [W / 2, H / 2, 0], 'both'));
  // 上下门分缝
  g.add(m(box(0.03, 0.012, D - 0.04), toon('#cfc7b6'), [W / 2 + 0.005, H * 0.66, 0], 'none'));
  // 把手
  for (const y of [H * 0.80, H * 0.52]) {
    g.add(m(box(0.03, 0.17, 0.028), metalMat, [W / 2 + 0.026, y, D / 2 - 0.10], 'cast'));
  }
  // 冰箱贴 & 便签
  const notes: Array<[number, number, string]> = [
    [H * 0.86, -0.10, '#f1ece3'], [H * 0.78, 0.06, '#dfe6dc'],
    [H * 0.70, -0.02, '#e2d8c2'], [H * 0.40, 0.08, '#e3e8ee'], [H * 0.32, -0.09, '#f1ece3'],
  ];
  for (const [y, z, col] of notes) {
    g.add(m(box(0.006, 0.075, 0.075), toon(col), [W / 2 + 0.014, y, z], 'none'));
  }

  // 顶上放一盆小绿植
  const plant = buildPlant(0.8);
  plant.position.set(0, H, -0.05);
  g.add(plant);

  return g;
}

/* ============================================================================
 * 地毯 / 矮桌 / 座布団
 * ========================================================================== */

export function buildRug(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];

  // 底下压一层深色，等于给地毯加了一圈"厚度"
  g.add(m(new THREE.PlaneGeometry(W + 0.06, D + 0.06), toon('#bda49e'), [0, 0.004, 0], 'receive').rotateX(-Math.PI / 2));
  const top = m(new THREE.PlaneGeometry(W, D), toon('#ffffff', { map: rugTexture() }), [0, 0.008, 0], 'receive');
  top.rotation.x = -Math.PI / 2;
  g.add(top);
  return g;
}

export function buildLowTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const r = spec.size[0] / 2;
  const H = spec.size[1];

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });

  // 底盘与立柱的半径跟着桌面走。原来底盘写死 0.26m：0.45m 的床头柜
  // 底盘和桌面一样大，0.4m 的边几底盘比桌面还大一圈，既难看又被顶进墙里。
  const baseR = Math.min(0.26, r * 0.72);
  const colR = Math.max(0.032, r * 0.16);

  g.add(m(cyl(r, r, 0.045, 26), woodMat, [0, H - 0.022, 0], 'both'));
  g.add(m(cyl(colR, colR * 1.25, H - 0.05, 12), woodMat, [0, (H - 0.05) / 2, 0], 'cast'));
  g.add(m(cyl(baseR * 0.92, baseR, 0.03, 18), woodMat, [0, 0.015, 0], 'cast'));

  // 茶壶
  const pot = new THREE.Group();
  const body = m(sph(0.078, 14, 12), toon('#a4b8c8'), [0, 0.065, 0], 'cast');
  body.scale.set(1, 0.85, 1);
  pot.add(body);
  pot.add(m(cyl(0.026, 0.03, 0.018, 12), toon('#93aac0'), [0, 0.123, 0], 'cast'));
  pot.add(m(sph(0.016, 8, 6), toon('#ddcba8'), [0, 0.136, 0], 'cast'));
  const spout = m(cyl(0.011, 0.016, 0.07, 8), toon('#a4b8c8'), [0.075, 0.078, 0], 'cast');
  spout.rotation.z = -0.85;
  pot.add(spout);
  const handle = m(new THREE.TorusGeometry(0.032, 0.007, 6, 14), toon('#a4b8c8'), [-0.078, 0.075, 0], 'cast');
  handle.rotation.y = Math.PI / 2;
  pot.add(handle);
  pot.position.set(0, H, 0);
  g.add(pot);

  // 两只茶杯
  for (const [cx, cz, col] of [[-r * 0.62, r * 0.34, '#fdf6f0'], [r * 0.55, -r * 0.38, '#f1ece3']] as Array<[number, number, string]>) {
    g.add(m(cyl(0.033, 0.028, 0.05, 14), toon(col), [cx, H + 0.025, cz], 'cast'));
    g.add(m(cyl(0.028, 0.028, 0.004, 14), toon('#b09070'), [cx, H + 0.048, cz], 'none'));
  }

  return g;
}

export function buildCushion(pos: [number, number, number], color = '#f1ece3', rot = 0): THREE.Group {
  const g = new THREE.Group();
  g.add(m(box(0.46, 0.085, 0.46), toon(color), [0, 0.045, 0], 'both'));
  // 中间一个凹口
  g.add(m(sph(0.022, 8, 6), toon('#cbbbb4'), [0, 0.083, 0], 'none'));
  // 四角的小流苏
  for (const sx of [-1, 1]) {
    for (const sz of [-1, 1]) {
      g.add(m(sph(0.016, 8, 6), toon('#cbbbb4'), [sx * 0.215, 0.03, sz * 0.215], 'none'));
    }
  }
  g.position.set(pos[0], pos[1], pos[2]);
  g.rotation.y = rot;
  return g;
}

/* ============================================================================
 * 套房扩展：客厅 / 厨房 / 卫浴 / 和室 / 电竞 / 玄关 / 门 / 阳台
 * ========================================================================== */

/** 客厅三人沙发。size = [进深X(默认朝 +X), 高, 长Z]，面朝 +X。 */
export function buildSofa(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  const seatH = 0.40;

  const bodyMat = toon('#e8e0d2');
  const cushionMat = toon(C('sofa', '#bfa49c'));
  const legMat = toon(C('wood', '#b09070'));

  // 底箱 + 两侧扶手 + 靠背（靠背在 -X 侧，面朝 +X）
  g.add(m(box(D, seatH, L), bodyMat, [0, seatH / 2, 0], 'both'));
  g.add(m(box(D, H - seatH, 0.16), bodyMat, [0, seatH + (H - seatH) / 2, L / 2 - 0.08], 'cast'));
  g.add(m(box(D, H - seatH, 0.16), bodyMat, [0, seatH + (H - seatH) / 2, -L / 2 + 0.08], 'cast'));
  g.add(m(box(0.18, H, L), bodyMat, [-D / 2 + 0.09, H / 2, 0], 'both'));
  // 四条小短腿
  for (const sz of [-1, 1]) {
    g.add(m(box(0.05, 0.10, 0.05), legMat, [sz * (D / 2 - 0.08), 0.05, L / 2 - 0.10], 'cast'));
    g.add(m(box(0.05, 0.10, 0.05), legMat, [sz * (D / 2 - 0.08), 0.05, -L / 2 + 0.10], 'cast'));
  }
  // 三个坐垫 + 两个靠枕，坐出"陷进去"的感觉
  for (const z of [-L / 2 + 0.55, 0, L / 2 - 0.55]) {
    const c = m(box(D - 0.30, 0.13, 0.52), cushionMat, [0.02, seatH + 0.065, z], 'cast');
    g.add(c);
  }
  for (const [z, col, ry] of [[-L / 2 + 0.35, '#bda49e', 0.18], [L / 2 - 0.35, '#a8b8ac', -0.15]] as Array<[number, string, number]>) {
    const p = m(box(0.13, 0.30, 0.30), toon(col), [-D / 2 + 0.20, seatH + 0.20, z], 'cast');
    p.rotation.z = -0.16;
    p.rotation.y = ry;
    g.add(p);
  }
  return g;
}

/** 电视柜 + 电视。size = [柜深, 柜高, 柜长]，屏幕面朝 +X。 */
export function buildTV(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const darkMat = toon(C('dark', '#43434f'));

  g.add(m(box(D, H, L), woodMat, [0, H / 2, 0], 'both'));
  // 抽屉缝 + 拉手
  for (const z of [-L / 4, L / 4]) {
    g.add(m(box(D + 0.01, H * 0.62, 0.012), toon('#b39377'), [0, H / 2, z], 'none'));
    g.add(m(box(0.02, 0.02, 0.14), toon(C('metal', '#b9bcc4'), { finish: 'metal' }), [D / 2 + 0.01, H * 0.62, z], 'none'));
  }
  // 电视：底座 + 支架 + 屏幕
  g.add(m(box(0.16, 0.015, 0.42), darkMat, [0, H + 0.008, 0], 'cast'));
  g.add(m(box(0.03, 0.16, 0.05), darkMat, [0, H + 0.09, 0], 'cast'));
  g.add(m(box(0.035, 0.62, 1.10), darkMat, [0, H + 0.44, 0], 'cast'));
  // 屏幕面朝 +X（贴在 box +X 面上）；PlaneGeometry 默认法线 +Z，绕 Y 转 +π/2 后法线指向 +X；凸出 0.5mm 避免和框面 z-fight
  const screen = m(new THREE.PlaneGeometry(1.04, 0.56), emissive('#e0e8ec', { map: screenTexture() }), [0.018, H + 0.44, 0], 'none');
  screen.rotation.y = Math.PI / 2;
  g.add(screen);
  return g;
}

/** 落地灯。 */
export function buildFloorLamp(spec: { pos: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  g.add(m(cyl(0.11, 0.13, 0.02, 16), metalMat, [0, 0.01, 0], 'cast'));
  g.add(m(cyl(0.014, 0.014, 1.32, 8), metalMat, [0, 0.68, 0], 'cast'));
  const shade = m(cyl(0.14, 0.19, 0.26, 16, true), toon('#fdf3e0', { side: THREE.DoubleSide }), [0, 1.42, 0], 'cast');
  g.add(shade);
  g.add(m(sph(0.045, 10, 8), emissive('#e8d9b8'), [0, 1.36, 0], 'none'));
  const light = new THREE.PointLight(0xffd9a0, 0.45, 3.2, 2);
  light.position.set(0, 1.38, 0);
  g.add(light);
  return g;
}

/**
 * 厨房：操作台 + 水槽 + 灶台 + 吊柜 + 台面小物。
 * size = [长X, 台高, 进深Z]，紧贴 -Z 墙（背面在 -Z）。
 */
export function buildKitchen(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const L = spec.size[0], H = spec.size[1], D = spec.size[2];

  const counterMat = toon('#f2ede2');
  const cabinetMat = toon('#d3c9b6');
  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const darkMat = toon(C('dark', '#43434f'));

  // 地柜 + 台面 + 踢脚
  g.add(m(box(L, H - 0.06, D), cabinetMat, [0, (H - 0.06) / 2, 0], 'both'));
  // 台面只在"前脸"出檐 15mm，背面与地柜背板齐平。原来前后各出 10mm，
  // 背面那 10mm 正好啃进 120mm 厚的隔墙里。
  g.add(m(box(L + 0.02, 0.05, D + 0.02), counterMat, [0, H - 0.03, 0.015], 'both'));
  g.add(m(box(L - 0.08, 0.08, D - 0.06), toon('#8a8378'), [0, 0.04, 0.01], 'none'));
  // 柜门缝
  const nDoors = Math.round(L / 0.5);
  for (let i = 1; i < nDoors; i++) {
    g.add(m(box(0.012, H - 0.14, 0.012), toon('#b8ac93'), [-L / 2 + (L * i) / nDoors, (H - 0.06) / 2, D / 2], 'none'));
  }

  // 水槽（左侧内嵌）+ 龙头
  const sinkX = -L / 2 + 0.45;
  g.add(m(box(0.46, 0.02, 0.34), metalMat, [sinkX, H - 0.045, 0], 'none'));
  const faucet = m(cyl(0.013, 0.013, 0.30, 8), metalMat, [sinkX, H + 0.15, -D / 2 + 0.08], 'cast');
  g.add(faucet);
  const spout = m(cyl(0.011, 0.011, 0.16, 8), metalMat, [sinkX, H + 0.28, -D / 2 + 0.16], 'cast');
  spout.rotation.x = Math.PI / 2;
  g.add(spout);
  // 洗好的盘子立在沥水架里
  for (let i = 0; i < 3; i++) {
    const plate = m(cyl(0.09, 0.09, 0.012, 14), toon('#fdf6f0'), [sinkX - 0.16 + i * 0.05, H + 0.10, -0.05], 'cast');
    plate.rotation.z = Math.PI / 2 - 0.35;
    plate.rotation.y = 0.2;
    g.add(plate);
  }

  // 灶台（右侧）：面板 + 两个炉眼 + 小锅
  const stoveX = L / 2 - 0.42;
  g.add(m(box(0.52, 0.015, 0.40), darkMat, [stoveX, H + 0.008, 0], 'none'));
  for (const dz of [-0.09, 0.09]) {
    g.add(m(new THREE.TorusGeometry(0.085, 0.012, 6, 18), toon('#5c5c68'), [stoveX - 0.09, H + 0.02, dz], 'cast'));
    g.add(m(new THREE.TorusGeometry(0.085, 0.012, 6, 18), toon('#5c5c68'), [stoveX + 0.09, H + 0.02, dz], 'cast'));
  }
  const pot = m(cyl(0.10, 0.09, 0.10, 14), toon('#c9c4bb'), [stoveX - 0.09, H + 0.06, -0.09], 'cast');
  g.add(pot);
  g.add(m(new THREE.TorusGeometry(0.045, 0.008, 6, 14), metalMat, [stoveX - 0.09, H + 0.06, -0.09], 'none'));

  /**
   * 吊柜：贴在 -Z 侧上方（台面正上方的墙面）。
   * 柜深 0.33，中心必须放在 -D/2 + 0.185，柜背才和地柜背面（-D/2）齐平。
   * 原来写 -D/2 + 0.02，柜体会往墙里再探出 145mm，直接戳穿 120mm 厚的墙——
   * 厨房靠墙摆放时的穿模就是这么来的。
   */
  const upY0 = H + 0.55, upY1 = 2.18;
  const upZ = -D / 2 + 0.185;
  g.add(m(box(L - 0.25, upY1 - upY0, 0.33), cabinetMat, [0, (upY0 + upY1) / 2, upZ], 'both'));
  g.add(m(box(L - 0.25, 0.02, 0.35), counterMat, [0, upY1 + 0.01, upZ + 0.015], 'none'));
  for (let i = 1; i < 3; i++) {
    g.add(m(box(0.012, upY1 - upY0 - 0.04, 0.012), toon('#b8ac93'), [-L / 2 + 0.125 + ((L - 0.25) * i) / 3, (upY0 + upY1) / 2, -D / 2 + 0.19], 'none'));
  }

  // 台面小物：砧板 + 调味罐
  const board = m(box(0.28, 0.018, 0.20), woodMat, [-0.25, H + 0.034, 0.02], 'cast');
  board.rotation.y = 0.12;
  g.add(board);
  for (const [dx, col] of [[-0.08, '#c2a49c'], [0.0, '#a4b8c8'], [0.08, '#ddcba8']] as Array<[number, string]>) {
    g.add(m(cyl(0.030, 0.030, 0.11, 10), toon(col), [L / 2 - 0.75 + dx, H + 0.10, D / 2 - 0.12], 'cast'));
  }
  return g;
}

/** 餐桌 + 两把餐椅。size = [桌面宽X, 桌高, 桌深Z]。 */
export function buildDining(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const legMat = toon(C('deskLeg', '#b09070'));

  g.add(m(box(W, 0.045, D), woodMat, [0, H - 0.022, 0], 'both'));
  for (const sx of [-1, 1]) {
    for (const sz of [-1, 1]) {
      g.add(m(box(0.055, H - 0.05, 0.055), legMat, [sx * (W / 2 - 0.06), (H - 0.05) / 2, sz * (D / 2 - 0.06)], 'cast'));
    }
  }
  // 两把餐椅分坐两端（±X，位置不变），座面朝向桌面，椅背贴在外侧
  for (const sx of [-1, 1]) {
    const chair = new THREE.Group();
    chair.add(m(box(0.38, 0.04, 0.38), woodMat, [0, 0.44, 0], 'both'));
    // 靠背板：厚度沿 X、宽度沿 Z，这样椅背能贴到椅子 ±X 外侧并面向桌面
    const back = m(box(0.045, 0.42, 0.38), woodMat, [sx * 0.165, 0.66, 0], 'cast');
    back.rotation.z = -sx * 0.06; // 靠背略向后仰（远离桌面）
    chair.add(back);
    for (const cx of [-1, 1]) {
      for (const cz of [-1, 1]) {
        chair.add(m(box(0.035, 0.44, 0.035), legMat, [cx * 0.15, 0.22, cz * 0.15], 'cast'));
      }
    }
    chair.position.set(sx * (W / 2 + 0.30), 0, 0); // 位置保持餐桌两端，不改动
    chair.rotation.y = 0; // 西侧椅子还原为原朝向；东侧椅子保持改对状态（背朝房间外侧）
    g.add(chair);
  }
  // 桌上一只小水壶 + 两个杯子
  g.add(m(cyl(0.055, 0.065, 0.22, 12), toon('#a8b8ac'), [0, H + 0.11, 0], 'cast'));
  for (const [x, z, col] of [[-0.16, 0.10, '#fdf6f0'], [0.14, -0.12, '#f1ece3']] as Array<[number, number, string]>) {
    g.add(m(cyl(0.032, 0.027, 0.055, 12), toon(col), [x, H + 0.03, z], 'cast'));
  }
  return g;
}

/**
 * 卫生间三件套：浴缸 + 马桶 + 洗手台（含镜柜）。
 * 布局假定：浴缸沿 -X 墙（进深方向），马桶贴 -Z 墙，洗手台贴 +Z 墙。
 * size 参数给 [房间宽X, 不用, 房间深Z]，家具按房间相对定位。
 */
export function buildBathroom(spec: { pos: [number, number, number]; size: [number, number, number]; toilet?: boolean }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];
  const xL = -W / 2, zB = -D / 2, zF = D / 2;

  /**
   * 洁具位置原来是写死的偏移量，只在 3.2×2.4 那种宽卫生间里才不打架：
   * 房间一窄，浴缸（贴 -X 墙、长 1.70）就会横向撞上洗手台（宽 0.90、固定在 x=-0.35），
   * 纵向又会撞上洗手台的 Z 区间——这就是卫生间里那堆穿模。
   * 现在全部改成按房间实际尺寸推：缸长自适应、洗手台在"浴缸右缘 ~ +X 墙"之间居中并限宽。
   */
  const tubW = 0.75;
  // 缸长上限 = 房间进深 - 0.90（给两端各留 0.45 检修缝），保证和洗手台在 Z 上不重叠
  const tubHalfLen = Math.min(1.70, Math.max(0.90, D - 0.90)) / 2;
  const tubX = xL + 0.045 + tubW / 2;
  const vanRight = W / 2 - 0.05;
  const vanLeft = tubX + tubW / 2 + 0.08;
  const vanW = Math.max(0.50, Math.min(0.90, vanRight - vanLeft));
  const vanX = (vanLeft + vanRight) / 2;

  const porcelainMat = toon('#f7f8f6');
  const tileMat = toon('#dfe8ea');
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });

  /* 浴缸：沿 -X 墙，长边贴墙 */
  const tub = new THREE.Group();
  const tubLen = tubHalfLen * 2;
  tub.add(m(box(tubW, 0.52, tubLen), porcelainMat, [0, 0.26, 0], 'both'));
  // 内膛（深色水面 + 一圈内壁）
  tub.add(m(box(tubW - 0.15, 0.03, tubLen - 0.15), toon('#cbd8d6'), [0.03, 0.50, 0], 'none'));
  tub.add(m(box(tubW - 0.12, 0.36, tubLen - 0.12), tileMat, [0.02, 0.32, 0], 'none'));
  // 缸沿龙头
  const tf = m(cyl(0.013, 0.013, 0.26, 8), metalMat, [-0.28, 0.60, 0], 'cast');
  tub.add(tf);
  const tspout = m(cyl(0.011, 0.011, 0.14, 8), metalMat, [-0.21, 0.71, 0], 'cast');
  tspout.rotation.z = Math.PI / 2;
  tub.add(tspout);
  tub.position.set(tubX, 0, 0);
  g.add(tub);

  /* 马桶：贴 -Z 墙，朝 +Z（spec.toilet === false 时跳过，独立トイレ由 buildToilet 负责） */
  if (spec.toilet !== false) {
    const toilet = new THREE.Group();
    toilet.add(m(box(0.36, 0.20, 0.50), porcelainMat, [0, 0.10, 0], 'both'));
    const bowl = m(cyl(0.19, 0.16, 0.14, 16), porcelainMat, [0, 0.24, 0.05], 'cast');
    bowl.scale.set(1, 1, 1.25);
    toilet.add(bowl);
    toilet.add(m(cyl(0.17, 0.17, 0.012, 16), toon('#eef0ee'), [0, 0.315, 0.05], 'none'));
    // 水箱贴墙
    toilet.add(m(box(0.36, 0.42, 0.16), porcelainMat, [0, 0.32, -0.20], 'both'));
    toilet.add(m(box(0.14, 0.025, 0.04), metalMat, [0, 0.46, -0.12], 'cast'));
    toilet.position.set(W / 2 - 0.55, 0, zB + 0.42);
    g.add(toilet);
  }

  /* 洗手台 + 镜柜：贴 +Z 墙，朝 -Z */
  const van = new THREE.Group();
  van.add(m(box(vanW, 0.50, 0.30), woodMat, [0, 0.70, 0], 'both'));
  van.add(m(box(vanW + 0.04, 0.04, 0.34), porcelainMat, [0, 0.96, 0], 'both'));
  const basin = m(cyl(0.16, 0.13, 0.10, 16), porcelainMat, [0, 1.00, -0.01], 'cast');
  van.add(basin);
  const vf = m(cyl(0.011, 0.011, 0.22, 8), metalMat, [0, 1.10, 0.12], 'cast');
  van.add(vf);
  const vsp = m(cyl(0.009, 0.009, 0.12, 8), metalMat, [0, 1.19, 0.06], 'cast');
  vsp.rotation.x = Math.PI / 2;
  van.add(vsp);
  // 镜柜（emissive 镜面 + 木框）
  const mirrorW = Math.max(0.42, vanW - 0.20);
  van.add(m(box(mirrorW, 0.65, 0.05), woodMat, [0, 1.65, 0.10], 'both'));
  const mirror = m(new THREE.PlaneGeometry(mirrorW - 0.10, 0.55), emissive('#e4eaec'), [0, 1.65, 0.072], 'none');
  van.add(mirror);
  // 一条小毛巾
  van.add(m(box(Math.min(0.30, vanW * 0.34), 0.02, 0.34), toon('#f1ece3'), [vanW / 2 - 0.22, 0.985, 0], 'cast'));
  van.position.set(vanX, 0, zF - 0.20);
  van.rotation.y = Math.PI;
  g.add(van);

  return g;
}

/** 和室小物件：挂轴（床之间墙上）+ 矮桌上不需要，桌复用 buildLowTable。 */
export function buildScroll(spec: { pos: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  // 挂轴：面朝 +X（挂 -X 墙）
  g.add(m(box(0.03, 0.72, 0.30), toon('#f2ecdc'), [0, 0, 0], 'cast'));
  g.add(m(box(0.032, 0.035, 0.34), toon('#544c4e'), [0, 0.375, 0], 'cast'));
  g.add(m(box(0.032, 0.035, 0.34), toon('#544c4e'), [0, -0.375, 0], 'cast'));
  // 轴心画面：远山 + 一轮月
  g.add(m(new THREE.PlaneGeometry(0.20, 0.50), emissive('#f7f2e4'), [0.017, 0, 0], 'none'));
  g.add(m(sph(0.035, 10, 8), toon('#e2dac6'), [0.017, 0.14, -0.04], 'none'));
  g.add(m(new THREE.CircleGeometry(0.075, 16), toon('#a9c4b2'), [0.018, -0.10, 0.04], 'none'));
  return g;
}

/**
 * 电竞房：双人位战斗桌 + 双屏 + 主机(RGB) + 电竞椅 + 墙面灯带。
 * size = [桌宽X, 桌高, 桌深Z]，桌面沿 -Z 墙，面朝 +Z。
 */
export function buildGameStation(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];

  const darkMat = toon(C('dark', '#43434f'));
  const blackMat = toon('#2f2f37');
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });

  // 桌板 + 桌腿（两侧整块侧板，电竞桌的常见样式）
  g.add(m(box(W, 0.04, D), blackMat, [0, H - 0.02, 0], 'both'));
  for (const sx of [-1, 1]) {
    g.add(m(box(0.05, H - 0.04, D - 0.06), blackMat, [sx * (W / 2 - 0.04), (H - 0.04) / 2, 0], 'cast'));
  }
  // 桌面理线架
  g.add(m(box(W - 0.3, 0.02, 0.06), metalMat, [0, H + 0.09, -D / 2 + 0.08], 'none'));

  /* 主显示器（大曲面感：宽面板 + 轻微弯折用两块拼） */
  const mon = new THREE.Group();
  mon.add(m(box(0.05, 0.28, 0.05), darkMat, [0, 0.14, 0], 'cast'));
  const scr = new THREE.Group();
  scr.add(m(box(0.05, 0.44, 0.98), darkMat, [0, 0, 0], 'cast'));
  for (const dz of [-0.245, 0.245]) {
    const wing = m(box(0.05, 0.44, 0.24), darkMat, [0, 0, dz * 1.04], 'cast');
    wing.rotation.y = dz > 0 ? -0.14 : 0.14;
    scr.add(wing);
  }
  const c = new THREE.Color('#b0c4d0');
  const s1 = m(new THREE.PlaneGeometry(0.42, 0.38), emissive('#b0c4d0'), [0.027, 0.02, -0.36], 'none');
  const s2 = m(new THREE.PlaneGeometry(0.42, 0.38), emissive(c.offsetHSL(0.05, 0, 0).getStyle()), [0.027, 0.02, 0.36], 'none');
  scr.add(s1, s2);
  scr.position.set(0, 0.50, 0);
  mon.add(scr);
  mon.position.set(-0.35, H, -0.10);
  g.add(mon);

  /* 副屏（竖屏，右侧） */
  const sub = new THREE.Group();
  sub.add(m(box(0.04, 0.20, 0.04), darkMat, [0, 0.10, 0], 'cast'));
  sub.add(m(box(0.03, 0.40, 0.24), darkMat, [0, 0.40, 0], 'cast'));
  sub.add(m(new THREE.PlaneGeometry(0.20, 0.34), emissive('#8fae96'), [0.017, 0.40, 0], 'none'));
  sub.position.set(W / 2 - 0.45, H, -0.08);
  g.add(sub);

  /* 主机：桌下右侧，正面朝 +Z，RGB 三条灯带 */
  const tower = new THREE.Group();
  tower.add(m(box(0.22, 0.46, 0.46), blackMat, [0, 0.23, 0], 'both'));
  tower.add(m(new THREE.PlaneGeometry(0.16, 0.38), emissive('#34343c'), [0, 0.24, 0.231], 'none'));
  for (const [y, col] of [[0.38, '#b0705f'], [0.24, '#b0c4d0'], [0.10, '#8fae96']] as Array<[number, string]>) {
    tower.add(m(box(0.012, 0.025, 0.40), emissive(col), [0.115, y, 0], 'none'));
  }
  tower.position.set(W / 2 - 0.18, 0, 0.02);
  g.add(tower);

  /* 键盘 + 鼠标 + 鼠标垫 + 耳机挂架 */
  g.add(m(box(0.40, 0.018, 0.15), toon('#3a3a42'), [-0.35, H + 0.01, 0.22], 'cast'));
  g.add(m(box(0.30, 0.002, 0.22), toon('#43434f'), [-0.35, H + 0.003, 0.22], 'none'));
  g.add(m(box(0.06, 0.026, 0.10), toon('#3a3a42'), [0.12, H + 0.015, 0.24], 'cast'));
  const hook = m(new THREE.TorusGeometry(0.035, 0.008, 6, 12, Math.PI), metalMat, [W / 2 - 0.32, H + 0.12, -D / 2 + 0.10], 'cast');
  g.add(hook);
  const headset = m(box(0.16, 0.05, 0.10), toon('#3a3a42'), [W / 2 - 0.32, H + 0.15, -D / 2 + 0.10], 'cast');
  g.add(headset);

  /* 电竞椅：高背 + 头枕 + 五爪底座 */
  const chair = new THREE.Group();
  chair.add(m(cyl(0.03, 0.03, 0.30, 8), metalMat, [0, 0.20, 0], 'cast'));
  for (let i = 0; i < 5; i++) {
    const a = (i / 5) * Math.PI * 2;
    const leg = m(box(0.28, 0.03, 0.05), blackMat, [Math.cos(a) * 0.14, 0.045, Math.sin(a) * 0.14], 'cast');
    leg.rotation.y = -a;
    chair.add(leg);
    chair.add(m(sph(0.028, 8, 6), toon('#43434f'), [Math.cos(a) * 0.27, 0.028, Math.sin(a) * 0.27], 'none'));
  }
  chair.add(m(box(0.48, 0.08, 0.46), blackMat, [0, 0.38, 0], 'both'));
  chair.add(m(box(0.46, 0.10, 0.42), toon('#565063'), [0, 0.45, 0], 'cast'));
  const backrest = m(box(0.46, 0.72, 0.10), blackMat, [0, 0.86, -0.20], 'both');
  backrest.rotation.x = 0.10;
  chair.add(backrest);
  const backCushion = m(box(0.40, 0.60, 0.08), toon('#565063'), [0, 0.88, -0.15], 'cast');
  backCushion.rotation.x = 0.10;
  chair.add(backCushion);
  chair.add(m(box(0.26, 0.12, 0.09), toon('#565063'), [0, 1.30, -0.24], 'cast'));
  chair.position.set(-0.35, 0, 0.62);
  g.add(chair);

  /* 墙面 RGB 灯带（-Z 墙上方，横贯桌宽）+ 一张海报 */
  for (const [y, col] of [[2.35, '#b0705f'], [2.28, '#b0c4d0']] as Array<[number, string]>) {
    g.add(m(box(W - 0.4, 0.025, 0.015), emissive(col), [0, y, -D / 2 + 0.12], 'none'));
  }
  const poster = m(new THREE.PlaneGeometry(0.36, 0.50), toon('#a89aa8'), [W / 2 - 0.55, 1.55, -D / 2 + 0.121], 'none');
  poster.userData.noOutline = true;
  g.add(poster);

  return g;
}

/** 玄关：鞋柜 + 地垫 + 换鞋凳。size = [柜宽X, 柜高, 柜深Z]，柜贴 +Z 墙。 */
export function buildEntrySet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });

  // 鞋柜（上下两段，中间留格放钥匙）
  g.add(m(box(W, H, D), woodMat, [0, H / 2, 0], 'both'));
  g.add(m(box(W + 0.01, 0.02, D + 0.01), toon('#b39377'), [0, H * 0.62, 0], 'none'));
  g.add(m(box(W - 0.06, 0.26, 0.01), toon('#f2ede2'), [0, H * 0.62 + 0.16, -D / 2 - 0.002], 'none'));
  for (const s of [-1, 1]) {
    g.add(m(cyl(0.012, 0.012, 0.10, 8), toon(C('metal', '#b9bcc4'), { finish: 'metal' }), [s * W / 4, H * 0.35, -D / 2 - 0.012], 'cast').rotateZ(Math.PI / 2));
  }
  // 地垫（welcome mat）：跟着柜体正面走，别再写死 z=-1.05（柜深一变就浮到墙外）
  g.add(m(box(0.60, 0.012, 0.40), toon('#bda49e'), [0, 0.006, -D / 2 - 0.78], 'none'));
  // 换鞋小凳
  const bench = new THREE.Group();
  bench.add(m(box(0.40, 0.04, 0.26), woodMat, [0, 0.26, 0], 'both'));
  for (const sx of [-1, 1]) bench.add(m(box(0.04, 0.24, 0.22), toon('#b09070'), [sx * 0.16, 0.12, 0], 'cast'));
  /**
   * 换鞋凳：必须待在柜体正面（-Z 侧）的地面上，且 X 落在柜宽之内。
   * 原来放在 -W/2 - 0.30，凳子（宽 0.40）会整体伸到柜体外 0.50m，
   * 玄关柜一排到墙就必然插进墙里。
   */
  bench.position.set(W / 2 - 0.30, 0, -D / 2 - 0.18);
  g.add(bench);
  return g;
}

/** 门扇款式。同一户里的门本来就不是同一款，差异全在门扇和五金上。 */
export type DoorVariant = 'bedroom' | 'toilet' | 'wash' | 'bath' | 'closet';

/**
 * 室内门：门框（洞口内）+ 门套线（贴墙一圈）+ 门扇 + 五金。
 * size = [门宽, 门高, 0]，pos = 洞口中心（地面），rot = 门扇开合基准。
 * variant 决定门扇款式，不写 = 卧室门（三段平板 + 腰线 + 杆把手）。
 * 门扇默认朝 +X 开（挂在 -X 端铰链上），通过 rot 摆到各个墙面。
 *
 * 门读起来"像门"靠的是四层嵌套的收口，不是门扇本身：套线凸出墙面 → 门框填满
 * 墙厚 → 一圈 1cm 门缝 → 门扇凹在框里。素板门之所以廉价，是因为门扇和墙之间
 * 没有任何过渡，看着就是墙上贴了块板；补上套线和门缝，门才成为"装在洞里的构件"。
 *
 * 【款式差异的根据】都是这家人在用的门，区别只在使用场景：卫生间门下部带通风
 * 格栅（ガラリ）和表示锁；浴室门是单元浴室那种树脂门，格栅在中段、配竖把手；
 * 洗面所门上段嵌磨砂玻璃给走廊借光；收纳门最素，一块板配一个圆钮。
 */
export function buildDoor(spec: {
  pos: [number, number, number];
  size: [number, number, number];
  rot?: number;
  /** 款式名，写错按卧室门处理。取值见 DoorVariant。 */
  variant?: string;
}): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1];
  const variant = (spec.variant ?? 'bedroom') as DoorVariant;

  const JAMB_W = 0.03;      // 门框料宽（占进洞口，洞口净宽因此是 W-0.06）
  const JAMB_D = 0.126;     // 框进深 = 墙厚 0.12 + 两面各凸 3mm，凸出的这一点让框面读得出来
  const CASING_W = 0.055;   // 套线宽
  const CASING_D = 0.144;   // 套线进深：贯穿墙厚、两面各凸出 1.2cm
  const SW = W - 0.08;      // 门扇宽 = 净洞两侧再各让 1cm 门缝
  const SH = H - 0.03;      // 门扇高：上下各留 1.5cm 缝
  const T = 0.040;          // 门扇厚
  const Y0 = 0.015;         // 门扇底缝
  const CX = SW / 2;        // 门扇板局部 x 中心（铰链在 x=0）

  const frameMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const doorMat = toon(C('door', '#e9dcc8'));
  const railMat = toon(C('doorRail', '#cfc6b6'));
  const louvreMat = toon(C('doorLouvre', '#b3ada0'));
  const bathDoorMat = toon(C('bathDoor', '#e6ecee'));
  // 磨砂玻璃走不透明 + glass finish：真透明面要关 depthWrite、且不能描边（描边壳
  // 会从透明面后整片透上来把玻璃填成实心板），为一块采光窗新增透明预算不划算。
  // glass finish 给的是冷色边缘亮和高光——磨砂玻璃在赛璐璐下就是"发白的亮面"。
  const frostedMat = toon(C('doorGlass', '#e8eef2'), { finish: 'glass' });
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const lockMat = toon(C('doorLock', '#6b7078'), { finish: 'metal' });

  // 门框：两侧梃 + 上槛。包进 door-frame 组，供 RoomScene 单独描边（静态）
  const frame = new THREE.Group();
  frame.name = 'door-frame';
  for (const s of [-1, 1]) {
    frame.add(m(box(JAMB_W, H, JAMB_D), frameMat, [s * (W / 2 - JAMB_W / 2), H / 2, 0], 'cast'));
  }
  frame.add(m(box(W, 0.05, JAMB_D), frameMat, [0, H + 0.025, 0], 'cast'));
  // 套线：贴墙一圈。进深做成贯穿墙厚的一整块——中间那段被墙挡住，所以一个 box
  // 就同时喂到了门的两面，比两面各贴一片省一半 draw call
  for (const s of [-1, 1]) {
    frame.add(m(box(CASING_W, H + 0.05, CASING_D), frameMat, [s * (W / 2 + 0.015), (H + 0.05) / 2, 0], 'cast'));
  }
  // 横头套线压在两侧竖条之上（外缘齐平），门套因此有明确的"上收口"
  frame.add(m(box(W + 0.085, CASING_W, CASING_D), frameMat, [0, H + 0.0775, 0], 'cast'));
  g.add(frame);

  // 门扇：铰链在 -X 端，挂在独立 pivot（door-leaf）上，开关动画由 RoomScene 按距离驱动
  const leaf = new THREE.Group();
  leaf.name = 'door-leaf';
  const hx = SW - 0.075, hy = H * 0.47;   // 五金位：距门扇自由边 7.5cm

  /** 通风格栅（ガラリ）：一块凹进 8mm 的背板 + 若干道贯穿门扇的横条。
   *  横条贯穿整块门扇厚度，两面都能读到——和套线同一个"一块顶两面"的做法。 */
  const addLouvre = (yBottom: number, height: number, slats: number) => {
    const gw = SW - 0.10;
    leaf.add(m(box(gw, height, T - 0.016), louvreMat, [CX, yBottom + height / 2, 0], 'cast'));
    for (let i = 0; i < slats; i++) {
      const y = yBottom + height * ((i + 0.5) / slats);
      leaf.add(m(box(gw, 0.022, T - 0.004), louvreMat, [CX, y, 0], 'cast'));
    }
  };
  /** 两面各一套的杆把手 + 锁芯。 */
  const addLeverSet = (withLock: boolean) => {
    for (const s of [-1, 1]) {
      leaf.add(m(cyl(0.024, 0.024, 0.026, 12), metalMat, [hx, hy, s * (T / 2 + 0.013)], 'cast').rotateX(Math.PI / 2));
      leaf.add(m(box(0.095, 0.016, 0.016), metalMat, [hx - 0.0475, hy, s * (T / 2 + 0.031)], 'cast'));
      if (withLock) {
        leaf.add(m(cyl(0.011, 0.011, 0.014, 10), lockMat, [hx, hy - 0.058, s * (T / 2 + 0.007)], 'none').rotateX(Math.PI / 2));
      }
    }
  };

  if (variant === 'closet') {
    // 收纳门：整块素板，不分段——收纳只需要一个能拉开的面，腰线在这里是多余信息。
    // 把手是日式家具那种圆钮（つまみ），不是杆把手，一眼和卧室门区分开。
    leaf.add(m(box(SW, SH, T), doorMat, [CX, Y0 + SH / 2, 0], 'cast'));
    for (const s of [-1, 1]) {
      leaf.add(m(cyl(0.015, 0.015, 0.032, 12), metalMat, [SW - 0.06, hy, s * (T / 2 + 0.016)], 'cast').rotateX(Math.PI / 2));
    }
  } else if (variant === 'toilet') {
    // 卫生间门：下部通风格栅（卫生间没窗，换气靠门下这排ガラリ）+ 把手上的表示锁。
    // 格栅压得很低（离地 11~33cm），这是日式トイレドア最认得出的一个特征。
    const louvreY = Y0 + 0.10, louvreH = 0.22;
    leaf.add(m(box(SW, 0.10, T), doorMat, [CX, Y0 + 0.05, 0], 'cast'));
    addLouvre(louvreY, louvreH, 3);
    leaf.add(m(box(SW, SH - 0.32, T), doorMat, [CX, Y0 + 0.32 + (SH - 0.32) / 2, 0], 'cast'));
    addLeverSet(false);
    for (const s of [-1, 1]) {
      leaf.add(m(box(0.05, 0.035, 0.012), lockMat, [hx, hy + 0.052, s * (T / 2 + 0.006)], 'none'));
    }
  } else if (variant === 'bath') {
    // 浴室门：单元浴室的树脂门，格栅收在腰高（湿气往上走，排气口不在门底——卫生间
    // 门那排ガラリ是贴地面的，两者位置岔开才不会看成同一款），把手是竖向长条：
    // 手湿的时候从任何高度都能抓住，杆把手反而不好用。
    const louvreH = 0.26, lowerH = 1.125 - Y0 - louvreH, upperH = SH - louvreH - lowerH;
    leaf.add(m(box(SW, lowerH, T), bathDoorMat, [CX, Y0 + lowerH / 2, 0], 'cast'));
    addLouvre(Y0 + lowerH, louvreH, 4);
    leaf.add(m(box(SW, upperH, T), bathDoorMat, [CX, Y0 + lowerH + louvreH + upperH / 2, 0], 'cast'));
    for (const s of [-1, 1]) {
      leaf.add(m(box(0.024, 0.15, 0.024), metalMat, [SW - 0.07, H * 0.5, s * (T / 2 + 0.012)], 'cast'));
    }
  } else if (variant === 'wash') {
    // 洗面所门：上段嵌一块磨砂玻璃，给无窗的走廊借一点光。门扇上段因此拆成
    // 四条边梃围出来的框，玻璃薄 1.2cm 嵌在中间，两面都陷进去一点。
    const lowerH = SH * 0.30, railH = 0.042;
    const upY0 = Y0 + lowerH + railH, upH = SH - lowerH - railH;
    const upMid = upY0 + upH / 2;
    const gw = SW - 0.22, gh = upH - 0.34;   // 玻璃占上段的七成，上下各留一段实板才像门
    leaf.add(m(box(SW, lowerH, T), doorMat, [CX, Y0 + lowerH / 2, 0], 'cast'));
    leaf.add(m(box(SW, railH, T - 0.010), railMat, [CX, Y0 + lowerH + railH / 2, 0], 'cast'));
    const sideW = (SW - gw) / 2, railBarH = (upH - gh) / 2;
    for (const s of [-1, 1]) {
      leaf.add(m(box(sideW, upH, T), doorMat, [s > 0 ? SW - sideW / 2 : sideW / 2, upMid, 0], 'cast'));
    }
    leaf.add(m(box(gw, railBarH, T), doorMat, [CX, upMid + gh / 2 + railBarH / 2, 0], 'cast'));
    leaf.add(m(box(gw, railBarH, T), doorMat, [CX, upMid - gh / 2 - railBarH / 2, 0], 'cast'));
    leaf.add(m(box(gw, gh, T - 0.012), frostedMat, [CX, upMid, 0], 'cast'));
    addLeverSet(false);
  } else {
    // 卧室门：三段平板，中间那道横档比门扇本体薄 1cm 形成凹槽——平板门的全部造型
    // 就是这条槽在赛璐璐色阶下压出的明暗跳变，比贴线条省事，且两面都成立。
    const lowerH = SH * 0.34, railH = 0.042;
    const upperH = SH - lowerH - railH;
    leaf.add(m(box(SW, lowerH, T), doorMat, [CX, Y0 + lowerH / 2, 0], 'cast'));
    leaf.add(m(box(SW, railH, T - 0.010), railMat, [CX, Y0 + lowerH + railH / 2, 0], 'cast'));
    leaf.add(m(box(SW, upperH, T), doorMat, [CX, Y0 + lowerH + railH + upperH / 2, 0], 'cast'));
    addLeverSet(true);
  }

  leaf.position.set(-W / 2 + 0.04, 0, 0);
  leaf.rotation.y = 0; // 默认关闭；RoomScene 按角色/玩家距离在 [0, -1.82] 之间驱动
  g.add(leaf);

  if (spec.rot) g.rotation.y = spec.rot;
  g.position.set(spec.pos[0], spec.pos[1], spec.pos[2]);
  return g;
}

/** 入户门：外墙面上的实体防盗门（关着）。size = [门宽, 门高, 0]。 */
export function buildEntryDoor(spec: { pos: [number, number, number]; size: [number, number, number]; rot?: number }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1];

  // 门色：与邻户共用同一套轮换色（203 是 2F 中户，落在第 3 档）。
  // 外廊是公共面，整排门必须是一套语言；内外两侧同一个色——主角户的区分交给
  // 门牌 / 表札那一套挂件，不再做在门扇上。
  const doorMat = toon(UNIT_DOOR_TONES[OWN_UNIT_TONE], { map: doorGrainMap() });
  const frameMat = toon('#39414c');   // 与邻户户门套同色：整排看过去是一套门的语言
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const grooveMat = toon('#6b7a86');  // 门面压条：与邻户同一支中蓝灰，压在深门上读得出来

  // 门框：两侧梃 + 上槛。包进 door-frame 组，供 RoomScene 单独描边（静态）
  // 进深 0.24 且整体向室外侧偏 0.05：外墙面那块覆板厚 0.13、外表面在 ZN-0.155，
  // 门框必须一路伸到墙面外（外表面落到 -5.9-0.17），否则洞壁露的是覆板断面、
  // 洞口没有收口。室内侧维持原来的 0.07，不动房间那面墙。
  //
  // 高度上刻意让开洞口上沿（洞顶在 H，门框顶落在 H+0.06 / 上槛落在 H+0.03±0.025）：
  // 覆板被洞口切开后，上面那块板的**底面**和房间外壳的洞顶断面都正好在 y=H，
  // 三者都是朝下的面——门框若也从 H 起，就是三层同向共面在一起闪。上槛底面压到
  // H+0.005、顶面 H+0.055，立梃顶 H+0.06，谁都不与它们同面。
  const frame = new THREE.Group();
  frame.name = 'door-frame';
  for (const s of [-1, 1]) {
    frame.add(m(box(0.05, H + 0.06, 0.24), frameMat, [s * (W / 2 + 0.02), (H + 0.06) / 2, -0.05], 'cast'));
  }
  frame.add(m(box(W + 0.14, 0.05, 0.24), frameMat, [0, H + 0.03, -0.05], 'cast'));
  g.add(frame);

  // 门扇：铰链在 -X 端，挂在独立 pivot（door-leaf）上，开关动画由 RoomScene 按距离驱动
  const leaf = new THREE.Group();
  leaf.name = 'door-leaf';
  // 门扇比洞口小一圈（每侧 7mm）：spec.size 给的是**洞口**尺寸，门扇照抄的话四个
  // 侧面会与洞口断面完全共面（断面就贴在 x=±W/2、y=H 上）。留出的这一圈同时也是
  // 真实门缝，门扇底仍贴地，只缩顶边和两侧。
  const leafW = W - 0.014, leafH = H - 0.014;
  leaf.add(m(box(leafW, leafH, 0.06), doorMat, [W / 2, leafH / 2, 0], 'both'));
  /* 门面分格：两道竖压条，内外各一套。
   * 门是双面构件——室内外两侧都要，只做 +z 那面的话从外廊看就是一整块素板
   * （旧版正是如此：凹槽/猫眼/圆把手全在 +z 侧，背对外廊）。 */
  for (const s of [-1, 1]) {
    for (const zz of [-0.034, 0.034]) {
      leaf.add(m(box(0.018, H - 0.26, 0.012), grooveMat, [W / 2 + s * W * 0.2, H * 0.52, zz], 'none'));
    }
  }
  // 猫眼只在外侧（-z 朝外廊）：它是从里往外看的
  leaf.add(m(cyl(0.022, 0.022, 0.015, 10), metalMat, [W / 2, H * 0.74, -0.036], 'none').rotateX(Math.PI / 2));
  // 杠杆把手：内外各一套，z 完全对称。旧版是个圆球，和邻户的杠杆把手对不上
  for (const s of [-1, 1]) {
    leaf.add(m(box(0.048, 0.15, 0.022), metalMat, [W * 0.86, H * 0.46, s * 0.040], 'cast'));
    leaf.add(m(box(0.145, 0.032, 0.028), metalMat, [W * 0.86 - 0.085, H * 0.46, s * 0.054], 'cast'));
  }
  leaf.position.set(-W / 2, 0, 0);
  leaf.rotation.y = 0; // 默认关闭
  g.add(leaf);

  if (spec.rot) g.rotation.y = spec.rot;
  g.position.set(spec.pos[0], spec.pos[1], spec.pos[2]);
  return g;
}

/**
 * 玻璃滑门（LDK↔阳台 / 和室↔阳台）：门框 + 双轨 + 两扇玻璃，一扇半开。
 * size = [总宽, 总高, 0]，pos = 洞口中心。两扇都吃进墙厚里。
 */
export function buildGlassDoor(spec: { pos: [number, number, number]; size: [number, number, number]; rot?: number }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1];

  const frameMat = toon(C('windowFrame', '#fdfaf5'));
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  // 玻璃：半透明，但 depthWrite 必须关 —— 否则透明面写入深度缓冲后会把背后的
  // 另一扇玻璃 / 阳台景物提前剔除，看起来就像一扇实心白门（透明排序伪影）。
  //
  // finish:'glass' 给玻璃一层冷色边缘亮 + 高光。透明面拿不到描边（描边壳是不透明的
  // BackSide 外扩壳，铺满轮廓之后会从玻璃后面整片透上来，把玻璃填成实心板），
  // 形状全靠这层高光读出来，所以它是必需的而不是锦上添花。
  const glassMat = toon('#dfeaf0', {
    transparent: true,
    opacity: 0.18,
    depthWrite: false,
    side: THREE.DoubleSide,
    finish: 'glass',
  });

  // 上框 + 下轨 + 中梃
  g.add(m(box(W, 0.07, 0.12), frameMat, [0, H - 0.035, 0], 'cast'));
  g.add(m(box(W, 0.05, 0.14), frameMat, [0, 0.025, 0], 'cast'));
  g.add(m(box(0.05, H, 0.12), frameMat, [-W / 2 + 0.025, H / 2, 0], 'cast'));
  g.add(m(box(0.05, H, 0.12), frameMat, [W / 2 - 0.025, H / 2, 0], 'cast'));

  // 两扇玻璃门：左半扇固定（door-leaf），右半扇（door-leaf2）可沿局部 X 向左滑到
  // 与左半扇重合，右半门洞因此打开——和 fusuma 同一套推拉逻辑，由 RoomScene 按
  // 「路径穿过 + 靠近」驱动。关着时两扇并拢盖住整面门洞（current=0 即闭合态）。
  const pw = W / 2;
  const mkPane = (handleSide: 1 | -1): THREE.Group => {
    const p = new THREE.Group();
    p.add(m(box(pw - 0.02, H - 0.16, 0.03), glassMat, [0, H / 2, 0], 'none'));
    // 每扇自己的细框 + 竖把手；把手必须位于两扇相接的那一侧（门洞中央），
    // 因此左半扇用 +x、右半扇用 -x。
    p.add(m(box(pw - 0.02, 0.045, 0.045), frameMat, [0, H - 0.10, 0], 'cast'));
    p.add(m(box(pw - 0.02, 0.045, 0.045), frameMat, [0, 0.10, 0], 'cast'));
    p.add(m(box(0.04, H - 0.20, 0.045), frameMat, [-pw / 2 + 0.01, H / 2, 0], 'cast'));
    p.add(m(box(0.04, H - 0.20, 0.045), frameMat, [pw / 2 - 0.01, H / 2, 0], 'cast'));
    p.add(m(box(0.035, 0.22, 0.05), metalMat, [handleSide * (pw / 2 - 0.05), H * 0.48, 0.01], 'cast'));
    return p;
  };
  const leaf = mkPane(1);
  leaf.name = 'door-leaf';
  leaf.position.set(-pw / 2, 0, 0);   // 固定半扇：盖住左半门洞
  g.add(leaf);
  const leaf2 = mkPane(-1);
  leaf2.name = 'door-leaf2';
  leaf2.position.set(pw / 2, 0, 0.03); // 动半扇：初始盖右半；开门向左滑与左半重合（z 略前避免 z-fighting）
  g.add(leaf2);

  if (spec.rot) g.rotation.y = spec.rot;
  g.position.set(spec.pos[0], spec.pos[1], spec.pos[2]);
  return g;
}

/**
 * 日式推拉门（障子·fusuma）：墙洞里的固定外框 + 两扇可滑动的障子面板，
 * 关着时并拢盖住整个门洞；开门时把右半扇（leaf2）向左滑到与左半扇（leaf）重合——
 * 只有「其中一扇推到和另一扇重合」，右半门洞因此打开，而非两扇向两侧分开。
 * size = [总宽, 总高, 0]，pos = 洞口中心。两扇都吃进墙厚里。
 * 门扇分组名 door-leaf / door-leaf2，由 RoomScene 按「路径穿过 + 靠近」滑动。
 */
export function buildFusuma(spec: { pos: [number, number, number]; size: [number, number, number]; rot?: number }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1];

  const frameMat = toon('#3f342a');  // 深木色框
  const paperMat = toon('#f2ead8', { transparent: true, opacity: 0.9, side: THREE.DoubleSide }); // 障子纸：暖白半透
  const latticeMat = toon('#4a3b2e'); // 格栅：深木色

  // 固定外框（墙厚内）：上槛 + 下槛 + 两侧梃，供 RoomScene 单独描边（静态）
  const frame = new THREE.Group();
  frame.name = 'door-frame';
  frame.add(m(box(W + 0.06, 0.09, 0.10), frameMat, [0, H - 0.045, 0], 'cast'));
  frame.add(m(box(W + 0.06, 0.09, 0.10), frameMat, [0, 0.045, 0], 'cast'));
  for (const s of [-1, 1]) {
    frame.add(m(box(0.07, H, 0.10), frameMat, [s * (W / 2 + 0.03), H / 2, 0], 'cast'));
  }
  g.add(frame);

  // 两扇障子面板（各宽 W/2），名称 door-leaf / door-leaf2，供 RoomScene 滑动动画
  const pw = W / 2;
  const mkPanel = (): THREE.Group => {
    const p = new THREE.Group();
    // 纸面
    p.add(m(box(pw - 0.02, H - 0.14, 0.03), paperMat, [0, H / 2, 0], 'none'));
    // 面板外框
    p.add(m(box(pw - 0.02, 0.05, 0.05), frameMat, [0, H - 0.08, 0], 'cast'));
    p.add(m(box(pw - 0.02, 0.05, 0.05), frameMat, [0, 0.08, 0], 'cast'));
    p.add(m(box(0.05, H - 0.16, 0.05), frameMat, [-pw / 2 + 0.02, H / 2, 0], 'cast'));
    p.add(m(box(0.05, H - 0.16, 0.05), frameMat, [pw / 2 - 0.02, H / 2, 0], 'cast'));
    // 障子格：横 3 道 + 竖 2 道（细格栅，只描边不投影）
    for (let i = 1; i <= 3; i++) {
      p.add(m(box(pw - 0.06, 0.022, 0.05), latticeMat, [0, (H - 0.16) * (i / 4) + 0.10, 0], 'none'));
    }
    for (let i = 1; i <= 2; i++) {
      p.add(m(box(0.022, H - 0.20, 0.05), latticeMat, [(i - 1.5) * (pw / 3), H / 2, 0], 'none'));
    }
    return p;
  };
  const leaf = mkPanel();
  leaf.name = 'door-leaf';
  leaf.position.set(-pw / 2, 0, 0);
  g.add(leaf);
  const leaf2 = mkPanel();
  leaf2.name = 'door-leaf2';
  // 右半扇略向前（+z）一点：开门滑到与左半扇重合时不会和左半扇 z-fighting
  leaf2.position.set(pw / 2, 0, 0.04);
  g.add(leaf2);

  if (spec.rot) g.rotation.y = spec.rot;
  g.position.set(spec.pos[0], spec.pos[1], spec.pos[2]);
  return g;
}

/** 阳台小件：晾衣杆 + 晾着的衣物 + 花箱 + 空调外机 + 咖啡桌椅。 */
export function buildBalconyProps(spec: { pos: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const metalMat = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });

  /* 晾衣杆：两根立杆 + 横杆 + 晾着一条毛巾一件 T 恤 */
  const pole = new THREE.Group();
  for (const sx of [-1, 1]) {
    pole.add(m(cyl(0.022, 0.026, 1.55, 8), toon('#d8d2c4'), [sx * 1.1, 0.775, 0], 'cast'));
  }
  const bar = m(cyl(0.014, 0.014, 2.3, 8), metalMat, [0, 1.55, 0], 'cast');
  bar.rotation.z = Math.PI / 2;
  pole.add(bar);
  // 毛巾（搭下来）+ T 恤（带肩线）
  const towel = m(box(0.32, 0.55, 0.012), toon('#f1ece3'), [-0.55, 1.26, 0], 'cast');
  pole.add(towel);
  const shirt = m(box(0.40, 0.48, 0.012), toon('#a8b8ac'), [0.25, 1.29, 0], 'cast');
  pole.add(shirt);
  pole.add(m(box(0.42, 0.06, 0.014), toon('#a4b8c8'), [0.25, 1.52, 0], 'cast'));
  pole.position.set(0, 0, 0);
  g.add(pole);

  /* 花箱 ×2：沿栏杆一排小花 */
  for (const [x, z, cols] of [
    [-0.6, 1.05, ['#c2a49c', '#ddcba8', '#c2a49c']],
    [0.9, 1.05, ['#a8b8ac', '#f1ece3', '#a89aa8']],
  ] as Array<[number, number, string[]]>) {
    const boxG = new THREE.Group();
    boxG.add(m(box(0.68, 0.16, 0.24), woodMat, [0, 0.08, 0], 'cast'));
    boxG.add(m(box(0.62, 0.04, 0.20), toon('#6b584a'), [0, 0.17, 0], 'none'));
    for (let i = 0; i < 3; i++) {
      boxG.add(m(sph(0.045, 8, 6), toon(cols[i]), [-0.20 + i * 0.20, 0.24, 0], 'cast'));
      boxG.add(m(box(0.012, 0.10, 0.012), toon('#6a9c78'), [-0.20 + i * 0.20, 0.19, 0], 'none'));
    }
    boxG.position.set(x, 0, z);
    g.add(boxG);
  }

  /* 空调外机：贴玄关外墙（+Z 侧墙面），带栅格和风扇网 */
  const ac = new THREE.Group();
  ac.add(m(box(0.72, 0.52, 0.28), toon('#d9dde0'), [0, 0.30, 0], 'both'));
  ac.add(m(box(0.74, 0.03, 0.30), toon('#b8bec4'), [0, 0.045, 0], 'cast'));
  for (let i = 0; i < 5; i++) {
    ac.add(m(box(0.64, 0.008, 0.008), toon('#aab0b6'), [0, 0.24 + i * 0.045, 0.142], 'none'));
  }
  ac.add(m(new THREE.CircleGeometry(0.16, 18), toon('#c4c9ce'), [-0.16, 0.30, 0.141], 'none'));
  for (let i = 0; i < 4; i++) {
    const spoke = m(box(0.26, 0.010, 0.006), toon('#aab0b6'), [-0.16, 0.30, 0.143], 'none');
    spoke.rotation.z = (i / 4) * Math.PI;
    ac.add(spoke);
  }
  // 支架
  for (const sx of [-1, 1]) {
    ac.add(m(box(0.04, 0.24, 0.04), toon('#8a8f95'), [sx * 0.26, 0.12, -0.14], 'cast'));
  }
  ac.position.set(2.55, 0, -0.42);
  g.add(ac);

  /* 一套小咖啡桌椅 */
  const cafe = new THREE.Group();
  cafe.add(m(cyl(0.20, 0.20, 0.035, 16), woodMat, [0, 0.42, 0], 'both'));
  cafe.add(m(cyl(0.03, 0.04, 0.40, 8), metalMat, [0, 0.20, 0], 'cast'));
  const stool = new THREE.Group();
  stool.add(m(cyl(0.15, 0.15, 0.03, 12), woodMat, [0, 0.42, 0], 'both'));
  stool.add(m(cyl(0.025, 0.035, 0.40, 8), metalMat, [0, 0.20, 0], 'cast'));
  stool.position.set(0.55, 0, 0.28);
  cafe.add(stool);
  // 桌上一杯东西
  cafe.add(m(cyl(0.035, 0.028, 0.09, 10), toon('#fdf6f0'), [0, 0.48, 0.04], 'cast'));
  cafe.position.set(1.75, 0, 0.35);
  g.add(cafe);

  return g;
}

/** 简洁吸顶灯（卧室/和室/卫浴/玄关）。半球罩 + 小夜灯点光。 */
export function buildCeilLamp(spec: { pos: [number, number, number]; ceilingH: number }): THREE.Group {
  const g = new THREE.Group();
  const y = spec.ceilingH;
  g.add(m(cyl(0.10, 0.13, 0.025, 14), toon('#e8e2d6'), [0, y - 0.012, 0], 'cast'));
  const dome = m(sph(0.115, 14, 8), toon('#fdf6ea', { emissive: '#e8dcbe', emissiveIntensity: 0.55, side: THREE.DoubleSide }), [0, y - 0.045, 0], 'cast');
  g.add(dome);
  const light = new THREE.PointLight(0xffe9c8, 0.38, 3.4, 2);
  light.position.set(0, y - 0.12, 0);
  g.add(light);
  g.position.set(spec.pos[0], 0, spec.pos[2]);
  return g;
}

/**
 * 有生活逻辑的零散物件：拖鞋/雨伞在玄关，纸箱在鞋柜边，垃圾桶在卧室书桌旁，
 * 帆布包靠 LDK 墙根。这些物件不参与导航，只负责把样板间变成"有人长期住过"的空间。
 */
/**
 * 零散生活物件的摆放位置。
 *
 * 这些坐标原来写死在函数里，户型一改它们就全部落到错误的位置——
 * 一半插进墙体、一半悬在半空，是"穿模"投诉里最难排查的一类。
 * 现在全部由 dormLayout.json 的 lifestyle 条目驱动，函数只负责造形。
 */
export type LifestyleConfig = {
  /** 拖鞋：每项 [x, z, 朝向弧度] */
  slippers?: Array<[number, number, number]>;
  /** 靠墙的折叠伞 [x, z]（伞顶朝 -X 倒） */
  umbrella?: [number, number];
  /** 帆布包 [x, z, 朝向弧度] */
  bag?: [number, number, number];
  /** 快递纸箱的落点 [x, z]，两只叠着放 */
  parcels?: [number, number];
  /** 垃圾桶 [x, z] */
  bin?: [number, number];
  /** 贴墙的拍立得/便签：[x, y, z, 绕Y弧度, 颜色] */
  cards?: Array<[number, number, number, number, string]>;
};

const LIFESTYLE_DEFAULTS: Required<LifestyleConfig> = {
  slippers: [[4.62, 0.55, -0.20], [4.36, 0.62, 0.12]],
  umbrella: [2.42, 0.25],
  bag: [-1.58, -0.45, -0.18],
  parcels: [4.72, 1.12],
  bin: [-3.12, -3.75],
  cards: [
    [-5.488, 1.62, -3.85, Math.PI / 2, '#e6dad4'],
    [-5.488, 1.38, -3.55, Math.PI / 2, '#c8d4da'],
    [-5.488, 1.10, -3.80, Math.PI / 2, '#ddcba8'],
  ],
};

export function buildLifestyleDetails(cfg?: LifestyleConfig): THREE.Group {
  const g = new THREE.Group();
  const wood = toon('#ad9276');
  const pale = toon('#f6eee3');
  const blue = toon('#a8bcc8');
  const dark = toon('#54525c');
  const paper = toon('#fffaf0');

  const D = { ...LIFESTYLE_DEFAULTS, ...(cfg ?? {}) };
  const slipColors = ['#c0a49c', '#a8bcc8'];

  // 玄关拖鞋：成双但不完全平行，保留刚脱下来的随意感。
  for (const [i, [x, z, rot]] of D.slippers.entries()) {
    const col = slipColors[i % slipColors.length];
    const slipper = m(box(0.13, 0.045, 0.31), toon(col), [x, 0.025, z], 'cast');
    slipper.rotation.y = rot;
    g.add(slipper);
    const strap = m(new THREE.TorusGeometry(0.052, 0.012, 6, 12, Math.PI), toon('#f3ede5'), [x, 0.065, z - 0.035], 'cast');
    strap.rotation.x = Math.PI / 2;
    strap.rotation.z = rot;
    g.add(strap);
  }

  // 玄关墙边的折叠伞：靠玄关西墙（X=2.2，开口 Z∈[0.55,1.35] 以南的实墙段）。
  // 弯柄接口对齐见 buildLifestyleDetails 内注释：环心放 (0, 0.78, R)。
  const umbrella = new THREE.Group();
  umbrella.add(m(cyl(0.010, 0.010, 0.78, 8), dark, [0, 0.39, 0], 'cast'));
  const canopy = m(new THREE.ConeGeometry(0.105, 0.42, 12, 1, true), blue, [0, 0.34, 0], 'cast');
  canopy.rotation.x = Math.PI;
  umbrella.add(canopy);
  const handle = m(new THREE.TorusGeometry(0.050, 0.010, 6, 12, Math.PI * 1.25), dark, [0, 0.78, 0.05], 'cast');
  handle.rotation.y = Math.PI / 2;
  umbrella.add(handle);
  // 倾角 +0.10 = 顶往 -X 倒（靠向墙）；伞冠 z 占 [0.115,0.335]，避开门口动线
  umbrella.position.set(D.umbrella[0], 0, D.umbrella[1]);
  umbrella.rotation.z = 0.10;
  g.add(umbrella);

  // LDK 墙根的帆布包（靠卧室东墙，餐厅旁）。
  const bag = new THREE.Group();
  bag.add(m(box(0.34, 0.34, 0.13), pale, [0, 0.17, 0], 'cast'));
  for (const sx of [-1, 1]) {
    const h = m(new THREE.TorusGeometry(0.085, 0.012, 6, 14, Math.PI), wood, [sx * 0.085, 0.35, 0], 'cast');
    h.rotation.x = Math.PI / 2;
    bag.add(h);
  }
  bag.position.set(D.bag[0], 0, D.bag[1]);
  bag.rotation.y = D.bag[2];
  g.add(bag);

  // 鞋柜旁的两只快递纸箱。
  const parcelA = m(box(0.34, 0.18, 0.28), toon('#bda283'), [D.parcels[0], 0.09, D.parcels[1]], 'cast');
  parcelA.rotation.y = 0.08;
  g.add(parcelA);
  const parcelB = m(box(0.25, 0.14, 0.23), toon('#bda283'), [D.parcels[0] - 0.06, 0.25, D.parcels[1] - 0.02], 'cast');
  parcelB.rotation.y = -0.10;
  g.add(parcelB);

  // 书桌旁的小垃圾桶和一团废纸。
  const bin = m(cyl(0.12, 0.095, 0.28, 14, true), dark, [D.bin[0], 0.14, D.bin[1]], 'cast');
  g.add(bin);
  g.add(m(sph(0.035, 7, 6), paper, [D.bin[0] - 0.16, 0.035, D.bin[1] + 0.17], 'cast'));

  /**
   * 墙面生活痕迹：卧室西墙的小照片/便签，均贴墙不投影。
   *
   * 贴墙的东西必须和墙一样是"朝内"的单面平面，不能用薄 box：box 朝向房间外
   * 那面是正面会被渲染出来，于是墙让开视线时它们还悬在半空。改成法线朝房间内
   * 的 Plane 之后，它们和墙一起被背面剔除，视觉上等于跟墙长在一起。
   */
  for (const [x, y, z, ry, col] of D.cards) {
    const card = m(new THREE.PlaneGeometry(0.24, 0.18), toon(col), [x, y, z], 'none');
    card.userData.noOutline = true;
    card.rotation.y = ry;
    g.add(card);
  }

  return g;
}

/* ============================================================================
 * 灯具 / 墙面装饰
 * ========================================================================== */

/** 和风纸灯笼（吊灯）。 */
export function buildCeilingLantern(pos: [number, number, number], ceilingH: number): THREE.Group {
  const g = new THREE.Group();
  const hangY = ceilingH - 0.42;

  const cordMat = toon('#544c4e');
  g.add(m(cyl(0.006, 0.006, 0.42, 6), cordMat, [0, hangY + 0.21, 0], 'none'));

  // 提灯那种鼓肚子的轮廓
  const profile: THREE.Vector2[] = [
    new THREE.Vector2(0.001, -0.19),
    new THREE.Vector2(0.095, -0.175),
    new THREE.Vector2(0.17, -0.10),
    new THREE.Vector2(0.195, 0.0),
    new THREE.Vector2(0.17, 0.10),
    new THREE.Vector2(0.095, 0.175),
    new THREE.Vector2(0.001, 0.19),
  ];
  const lanternMat = toon('#fff6e2', { emissive: '#dcc79c', emissiveIntensity: 0.9, side: THREE.DoubleSide });
  g.add(m(new THREE.LatheGeometry(profile, 20), lanternMat, [0, hangY, 0], 'cast'));

  // 上下木盖
  g.add(m(cyl(0.05, 0.05, 0.022, 12), toon('#a08066'), [0, hangY + 0.20, 0], 'cast'));
  g.add(m(cyl(0.05, 0.045, 0.022, 12), toon('#a08066'), [0, hangY - 0.20, 0], 'cast'));
  // 竹骨横箍
  for (const [y, r] of [[-0.13, 0.152], [-0.05, 0.192], [0.05, 0.192], [0.13, 0.152]] as Array<[number, number]>) {
    const ring = m(new THREE.TorusGeometry(r, 0.004, 6, 22), toon('#d6c3a2'), [0, hangY + y, 0], 'none');
    ring.rotation.x = Math.PI / 2;
    g.add(ring);
  }

  const light = new THREE.PointLight(0xffd9a0, 0.5, 4.6, 2);
  light.position.set(0, hangY - 0.1, 0);
  g.add(light);

  g.position.set(pos[0], 0, pos[2]);
  return g;
}

export function buildWallClock(pos: [number, number, number], rot = 0): THREE.Group {
  const g = new THREE.Group();
  const caseMat = toon('#fdf6ea');
  const rimMat = toon(C('accent', '#c2a49c'));

  const body = m(cyl(0.165, 0.165, 0.045, 26), caseMat, [0, 0, 0], 'cast');
  body.rotation.x = Math.PI / 2;
  g.add(body);

  const rim = m(new THREE.TorusGeometry(0.162, 0.014, 8, 30), rimMat, [0, 0, 0.024], 'cast');
  g.add(rim);

  const face = m(new THREE.CircleGeometry(0.148, 28), toon('#fffdf7'), [0, 0, 0.024], 'none');
  g.add(face);

  // 12 个刻度
  for (let i = 0; i < 12; i++) {
    const a = (i / 12) * Math.PI * 2;
    const tick = m(box(0.008, i % 3 === 0 ? 0.028 : 0.018, 0.004), toon('#6b5a5f'),
      [Math.sin(a) * 0.122, Math.cos(a) * 0.122, 0.027], 'none');
    tick.rotation.z = -a;
    g.add(tick);
  }

  // 指针：几何体先平移，让旋转轴落在中心
  const hourGeo = box(0.013, 0.082, 0.004).translate(0, 0.032, 0);
  const hour = m(hourGeo, toon('#544c4e'), [0, 0, 0.028], 'none');
  hour.rotation.z = -2.2;
  g.add(hour);
  const minGeo = box(0.009, 0.122, 0.004).translate(0, 0.050, 0);
  const min = m(minGeo, toon('#544c4e'), [0, 0, 0.030], 'none');
  min.rotation.z = 0.6;
  g.add(min);
  g.add(m(cyl(0.012, 0.012, 0.012, 10), toon(C('accent', '#c2a49c')), [0, 0, 0.032], 'none').rotateX(Math.PI / 2));

  g.position.set(pos[0], pos[1], pos[2]);
  g.rotation.y = rot;
  return g;
}

export function buildPoster(
  pos: [number, number, number],
  size: [number, number],
  /** 绕 Y 旋转：默认朝 +Z，传 -π/2 让法线指向 +X（挂在西墙朝室内） */
  rot = 0
): THREE.Group {
  const g = new THREE.Group();
  g.add(m(box(size[0] + 0.05, size[1] + 0.05, 0.018), toon('#fdf8ef'), [0, 0, 0], 'cast'));
  // 画面要比框面再往里 5mm，不然两个面几乎共面会 z-fighting
  g.add(m(new THREE.PlaneGeometry(size[0], size[1]), emissive('#ffffff', { map: posterTexture() }), [0, 0, 0.014], 'none'));
  g.position.set(pos[0], pos[1], pos[2]);
  g.rotation.y = rot;
  return g;
}

/** 一串小彩灯，沿悬链线垂在窗前。 */
export function buildStringLights(from: [number, number, number], to: [number, number, number], sag = 0.14): THREE.Group {
  const g = new THREE.Group();
  const N = 40;
  const pts: THREE.Vector3[] = [];
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    const x = from[0] + (to[0] - from[0]) * t;
    const z = from[2] + (to[2] - from[2]) * t;
    const y = from[1] + (to[1] - from[1]) * t - Math.sin(t * Math.PI) * sag;
    pts.push(new THREE.Vector3(x, y, z));
  }

  const curve = new THREE.CatmullRomCurve3(pts);
  g.add(m(new THREE.TubeGeometry(curve, 48, 0.005, 5, false), toon('#5a4a50'), [0, 0, 0], 'none'));

  const bulbColors = ['#efe6d0', '#f2e8e4', '#cfe6ff', '#efe6d0', '#e2eae0'];
  for (let i = 2; i < N; i += 3) {
    const p = curve.getPointAt(i / N);
    g.add(m(sph(0.023, 10, 8), emissive(bulbColors[i % bulbColors.length]), [p.x, p.y - 0.032, p.z], 'none'));
    const cap = m(cyl(0.008, 0.010, 0.014, 8), toon('#6b5a5f'), [p.x, p.y - 0.014, p.z], 'none');
    g.add(cap);
  }

  const mid = curve.getPointAt(0.5);
  const glow = new THREE.PointLight(0xffe0b0, 0.35, 2.6, 2);
  glow.position.set(mid.x, mid.y - 0.1, mid.z);
  g.add(glow);

  return g;
}

/* ============================================================================
 * 通用小物件
 * ========================================================================== */

/** 一盆观叶植物。scale=1 时高约 0.3m。 */
export function buildPlant(scale = 1): THREE.Group {
  const g = new THREE.Group();
  const potMat = toon('#c1745a');
  const soilMat = toon('#6b584a');
  const leafMat = toon('#6a9c78');

  g.add(m(cyl(0.075, 0.058, 0.095, 14), potMat, [0, 0.0475, 0], 'cast'));
  g.add(m(cyl(0.080, 0.080, 0.016, 14), toon('#b0664f'), [0, 0.096, 0], 'cast'));
  g.add(m(cyl(0.068, 0.068, 0.012, 14), soilMat, [0, 0.101, 0], 'none'));

  const leafGeo = new THREE.SphereGeometry(0.05, 8, 6);
  leafGeo.scale(0.5, 0.16, 1.0);
  const rnd = makeRng(6161);
  for (let i = 0; i < 8; i++) {
    const a = (i / 8) * Math.PI * 2 + rnd() * 0.3;
    const len = 0.8 + rnd() * 0.55;
    const leaf = m(leafGeo, leafMat, [0, 0, 0], 'cast');
    leaf.scale.set(1, 1, len);
    leaf.position.set(Math.cos(a) * 0.05 * len, 0.11 + rnd() * 0.03, Math.sin(a) * 0.05 * len);
    leaf.rotation.y = -a;
    leaf.rotation.x = -0.55 - rnd() * 0.25;
    g.add(leaf);
  }
  g.add(m(sph(0.026, 8, 6), toon('#7fb08a'), [0, 0.135, 0], 'cast'));

  g.scale.setScalar(scale);
  return g;
}

/* ============================================================================
 * 窗口光束 + 地面光斑
 * ========================================================================== */

export type BeamConfig = {
  x0: number; x1: number;   // 窗口左右（世界 X）
  y0: number; y1: number;   // 窗口上下（世界 Y）
  z: number;                // 窗口所在平面（世界 Z）
  dir: [number, number, number]; // 光线前进方向
  sheets?: number;
  opacity?: number;
  /**
   * 光束来自哪面墙（shell.walls 的 id）。RoomScene 靠它把光束挂进该墙的
   * decor 组——光束是从窗口一直拉到地面的大四边形，脱离墙独立挂 scene 的话，
   * 观察者模式把墙剔掉之后它就变成几片悬在半空的白条。
   */
  wall?: string;
};

/**
 * 窗户漏进来的一道阳光。做法：沿光线方向切几张"薄片"，
 * 每片从窗口竖边一直拉到地面，叠加混合后就成了一道有体积感的光柱。
 */
export function buildSunbeam(cfg: BeamConfig): THREE.Group {
  const g = new THREE.Group();
  const dir = new THREE.Vector3(...cfg.dir).normalize();
  if (dir.y >= -0.05) return g; // 方向不对就别画了

  const tex = beamTexture();
  const mat = new THREE.MeshBasicMaterial({
    map: tex,
    transparent: true,
    opacity: cfg.opacity ?? 0.11,
    blending: THREE.AdditiveBlending,
    depthWrite: false,
    side: THREE.DoubleSide,
  });

  const n = cfg.sheets ?? 3;
  for (let i = 0; i < n; i++) {
    const t = n === 1 ? 0.5 : i / (n - 1);
    const x = cfg.x0 + (cfg.x1 - cfg.x0) * (0.12 + t * 0.76);

    // 窗口上的竖边两点 → 沿光线投到地面
    const p0 = new THREE.Vector3(x, cfg.y1, cfg.z);
    const p1 = new THREE.Vector3(x, cfg.y0, cfg.z);
    const t0 = p0.y / -dir.y;
    const t1 = p1.y / -dir.y;
    const p3 = p0.clone().addScaledVector(dir, t0);
    const p2 = p1.clone().addScaledVector(dir, t1);

    const geo = new THREE.BufferGeometry();
    // 顶点顺序：p0(窗上) p1(窗下) p2(地-下沿) p3(地-上沿)
    geo.setAttribute('position', new THREE.Float32BufferAttribute([
      p0.x, p0.y, p0.z, p1.x, p1.y, p1.z, p2.x, p2.y, p2.z, p3.x, p3.y, p3.z,
    ], 3));
    // u = 从窗口往地面的推进度，v = 高度
    geo.setAttribute('uv', new THREE.Float32BufferAttribute([0, 1, 0, 0, 1, 0, 1, 1], 2));
    geo.setIndex([0, 1, 2, 0, 2, 3]);
    geo.computeVertexNormals();

    const sheet = new THREE.Mesh(geo, mat);
    sheet.renderOrder = 3;
    g.add(sheet);
  }

  // 地面上的那块亮斑
  const proj: THREE.Vector3[] = [];
  for (const [x, y] of [[cfg.x0, cfg.y0], [cfg.x1, cfg.y0], [cfg.x1, cfg.y1], [cfg.x0, cfg.y1]] as Array<[number, number]>) {
    const p = new THREE.Vector3(x, y, cfg.z);
    proj.push(p.clone().addScaledVector(dir, p.y / -dir.y));
  }
  const cx = (proj[0].x + proj[2].x) / 2;
  const cz = (proj[0].z + proj[2].z) / 2;
  // 只比投影轮廓大一圈，太大就会铺到墙外去（虽然被墙挡住看不见，但会盖到家具底下）
  const spanX = Math.abs(proj[1].x - proj[0].x) + 0.8;
  const spanZ = Math.abs(proj[2].z - proj[0].z) + 0.8;
  const pool = new THREE.Mesh(
    new THREE.PlaneGeometry(spanX, spanZ),
    new THREE.MeshBasicMaterial({
      map: lightPoolTexture(), transparent: true, opacity: 0.34,
      blending: THREE.AdditiveBlending, depthWrite: false,
    })
  );
  pool.rotation.x = -Math.PI / 2;
  pool.position.set(cx, 0.012, cz);
  pool.renderOrder = 2;
  g.add(pool);

  return g;
}

/* ============================================================================
 * 浮尘
 * ========================================================================== */

export type MotesConfig = {
  count: number;
  bounds: { x: [number, number]; y: [number, number]; z: [number, number] };
};

/**
 * 空气里飘的灰尘。逆光时特别明显，是日式动画背景的标志性细节。
 * 返回 update(t) 交给渲染循环调用。
 */
export function buildDustMotes(cfg: MotesConfig): { object: THREE.Points; update: (t: number) => void } {
  const n = cfg.count;
  const pos = new Float32Array(n * 3);
  const seed = new Float32Array(n);
  const rnd = makeRng(31415);

  for (let i = 0; i < n; i++) {
    pos[i * 3] = cfg.bounds.x[0] + rnd() * (cfg.bounds.x[1] - cfg.bounds.x[0]);
    pos[i * 3 + 1] = cfg.bounds.y[0] + rnd() * (cfg.bounds.y[1] - cfg.bounds.y[0]);
    pos[i * 3 + 2] = cfg.bounds.z[0] + rnd() * (cfg.bounds.z[1] - cfg.bounds.z[0]);
    seed[i] = rnd() * 100;
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));

  const mat = new THREE.PointsMaterial({
    size: 0.032,
    map: softDotTexture(),
    transparent: true,
    opacity: 0.62,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    sizeAttenuation: true,
  });

  const points = new THREE.Points(geo, mat);
  points.frustumCulled = false;

  const base = pos.slice();
  const y0 = cfg.bounds.y[0];
  const y1 = cfg.bounds.y[1];

  const update = (t: number) => {
    const attr = geo.getAttribute('position') as THREE.BufferAttribute;
    const arr = attr.array as Float32Array;
    for (let i = 0; i < n; i++) {
      const s = seed[i];
      const rise = (t * 0.035 + s * 0.37) % 1;
      arr[i * 3] = base[i * 3] + Math.sin(t * 0.28 + s) * 0.09;
      arr[i * 3 + 1] = y0 + rise * (y1 - y0);
      arr[i * 3 + 2] = base[i * 3 + 2] + Math.cos(t * 0.22 + s * 1.7) * 0.07;
    }
    attr.needsUpdate = true;
  };

  return { object: points, update };
}

/* ============================================================================
 * 豪华公寓 · 新素材 builder
 *
 * 设计原则：
 *  - 每空间 1-2 件陈述件（软包床头/弧形沙发/大理石茶几/整墙书柜/嵌入式衣柜）
 *  - 材质表现靠纹理与形状，不靠饱和度：大理石 veining、天鹅绒刷痕、亚麻垂坠
 *  - 灯光分层：吊灯 + LED 灯带 + 壁灯 + 落地灯
 *
 * 【配色基线以 dormLayout.json 的 palette 为准，别信下面的 fallback 值】
 * 这一节 builder 里的 C('walnut', …) / C('brass', …) 是按早年「欧式豪宅」方向
 * 写的，现 palette 已把 walnut / brass 覆盖成浅木（#C9B79A）和灰金属
 * （#B0AEA6），与 2026-09-01 定的北欧日式基线一致。文件里的 fallback hex
 * 只是键缺失时的兜底，运行时不会生效 —— 改配色请改 palette，别改这里。
 * ========================================================================== */

/* ---------------- 软包床（主卧陈述件） ---------------- */

/**
 * 软包床头是豪华主卧的标志：织物包面 + 黄铜收边，比光秃秃的木床头
 * 多一层"酒店套房"的质感。床体胡桃木，床品奶油白+淡绿被。
 */
export function buildUpholsteredBed(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const linen = toon(C('linen', '#E8E2D5'), { map: velvetTexture('#E8E2D5') });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });
  const white = toon('#FAF7F0');
  const blanket = toon(C('blanket', '#C4D0C0'));

  g.add(m(box(w, h * 0.55, d), walnut, [0, h * 0.275, 0], 'both'));
  g.add(m(box(w - 0.1, h * 0.45, d - 0.1), white, [0, h * 0.55 + h * 0.225, 0], 'both'));

  const headboardH = 1.0;
  g.add(m(box(w + 0.1, headboardH, 0.15), linen, [0, h * 0.5 + headboardH / 2, -d / 2 - 0.075], 'both'));
  g.add(m(box(w + 0.1, 0.03, 0.03), brass, [0, h * 0.5 + headboardH, -d / 2 - 0.075], 'both'));
  g.add(m(box(0.04, headboardH, 0.04), brass, [-w / 2 - 0.03, h * 0.5 + headboardH / 2, -d / 2 - 0.075], 'both'));
  g.add(m(box(0.04, headboardH, 0.04), brass, [w / 2 + 0.03, h * 0.5 + headboardH / 2, -d / 2 - 0.075], 'both'));

  const blanketD = d * 0.6;
  g.add(m(box(w - 0.15, 0.1, blanketD), blanket, [0, h * 0.8, d / 2 - blanketD / 2], 'both'));
  const pillowW = w / 2 - 0.15;
  g.add(m(box(pillowW, 0.12, 0.25), white, [-w / 4, h * 0.86, -d / 2 + 0.2], 'both'));
  g.add(m(box(pillowW, 0.12, 0.25), white, [w / 4, h * 0.86, -d / 2 + 0.2], 'both'));

  return g;
}

/* ---------------- 铁艺床（次卧/客房） ---------------- */

/**
 * 铁艺床头是"青年公寓/客房"的标志：竖栅比软包更轻盈，
 * 适合次卧这种不需要陈述件的房间。
 *
 * 结构：四根铸铁立柱撑起床箱 → 床箱上铺床垫 → 床头竖栅 + 床尾矮栅。
 * 旧版没有床腿，床垫整块悬在 0.2m 高的空中；床头/床尾的"横杆"又把
 * cyl(半径, 半径, w) 当横杆用，结果是一根 1.1m 长的铁棍竖直插在床上、
 * 床尾那根还一路戳到地面下 0.05m。横杆必须绕 Z 转 90° 让圆柱沿 X 躺倒。
 */
export function buildMetalBed(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const metal = toon(C('dark', '#3a3630'));
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const white = toon('#FAF7F0');
  const blanket = toon(C('blanket', '#C4D0C0'));

  // 床箱顶面就是 spec 高度 h；下面 55% 让给床腿，床才不会"浮"着
  const legH = h * 0.55;
  const boxH = h - legH;
  const lx = w / 2 - 0.045, lz = d / 2 - 0.06;

  g.add(m(box(w - 0.12, boxH, d - 0.12), walnut, [0, legH + boxH / 2, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.020, 0.024, legH, 8), metal, [sx * lx, legH / 2, sz * lz], 'cast'));
  }
  // 落地横撑：两条沿 Z、一条沿 X，把四条腿连成框
  for (const sx of [-1, 1]) {
    g.add(m(cyl(0.011, 0.011, d - 0.14, 8), metal, [sx * lx, 0.06, 0], 'cast').rotateX(Math.PI / 2));
  }
  g.add(m(cyl(0.011, 0.011, w - 0.11, 8), metal, [0, 0.06, 0], 'cast').rotateZ(Math.PI / 2));

  // 床头竖栅 + 横杆（横杆绕 Z 转 90°，圆柱轴 Y → X）
  const railH = 0.62, nBars = 7;
  for (let i = 0; i < nBars; i++) {
    const x = -w / 2 + 0.08 + ((w - 0.16) * i) / (nBars - 1);
    g.add(m(cyl(0.016, 0.016, railH, 8), metal, [x, h + railH / 2, -d / 2 + 0.03], 'cast'));
  }
  g.add(m(cyl(0.024, 0.024, w - 0.10, 8), metal, [0, h + railH, -d / 2 + 0.03], 'cast').rotateZ(Math.PI / 2));

  // 床尾矮栅 + 横杆
  const footH = 0.30;
  for (let i = 0; i < nBars; i++) {
    const x = -w / 2 + 0.08 + ((w - 0.16) * i) / (nBars - 1);
    g.add(m(cyl(0.014, 0.014, footH, 8), metal, [x, h + footH / 2, d / 2 - 0.03], 'cast'));
  }
  g.add(m(cyl(0.020, 0.020, w - 0.10, 8), metal, [0, h + footH, d / 2 - 0.03], 'cast').rotateZ(Math.PI / 2));

  // 床垫 + 被子 + 双枕
  const matH = 0.22;
  g.add(m(box(w - 0.10, matH, d - 0.10), white, [0, h + matH / 2, 0], 'both'));
  g.add(m(box(w - 0.15, 0.10, d * 0.6), blanket, [0, h + matH + 0.05, d * 0.2], 'both'));
  const pillowW = w / 2 - 0.15;
  g.add(m(box(pillowW, 0.12, 0.25), white, [-w / 4, h + matH + 0.06, -d / 2 + 0.2], 'both'));
  g.add(m(box(pillowW, 0.12, 0.25), white, [w / 4, h + matH + 0.06, -d / 2 + 0.2], 'both'));

  return g;
}

/* ---------------- 嵌入式衣柜（衣帽间陈述件） ---------------- */

/**
 * 通顶嵌入式衣柜：玻璃门 + LED 灯带 + 内部挂杆/隔板可见。
 * 和普通书架的区别就在"通顶 + 玻璃 + 灯"这三件套上。
 */
export function buildWardrobe(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });
  const glass = new THREE.MeshPhysicalMaterial({ color: 0xffffff, transparent: true, opacity: 0.15, roughness: 0.05, metalness: 0, side: THREE.DoubleSide });
  const led = emissive('#FFF8E0');

  g.add(m(box(w, h, d), walnut, [0, h / 2, 0], 'both'));

  const doorW = w / 2 - 0.02;
  g.add(m(new THREE.PlaneGeometry(doorW, h - 0.1), glass, [-w / 4, h / 2, d / 2 + 0.01], 'none'));
  g.add(m(new THREE.PlaneGeometry(doorW, h - 0.1), glass, [w / 4, h / 2, d / 2 + 0.01], 'none'));

  g.add(m(box(0.02, h - 0.1, 0.02), brass, [-w / 2 + 0.01, h / 2, d / 2], 'both'));
  g.add(m(box(0.02, h - 0.1, 0.02), brass, [w / 2 - 0.01, h / 2, d / 2], 'both'));
  g.add(m(box(0.02, h - 0.1, 0.02), brass, [0, h / 2, d / 2], 'both'));

  g.add(m(box(w - 0.1, 0.02, 0.05), led, [0, h - 0.05, 0], 'none'));

  // 挂衣杆：绕 Z 转 90° 让圆柱沿 X 躺倒。不转的话 cyl 的轴是 Y，
  // 1.9m 长的杆子会变成两根竖棍——上一根顶到 2.56m，下一根插进地板 3cm。
  g.add(m(cyl(0.015, 0.015, w - 0.1, 8), brass, [0, h * 0.7, 0], 'none').rotateZ(Math.PI / 2));
  g.add(m(cyl(0.015, 0.015, w - 0.1, 8), brass, [0, h * 0.4, 0], 'none').rotateZ(Math.PI / 2));

  g.add(m(box(w - 0.1, 0.02, d - 0.1), walnut, [0, h * 0.25, 0], 'none'));
  g.add(m(box(w - 0.1, 0.02, d - 0.1), walnut, [0, h * 0.55, 0], 'none'));

  return g;
}

/* ---------------- 展示柜（客厅） ---------------- */

/**
 * 客厅展示柜：玻璃门 + LED + 内部陈列（花瓶/雕塑/装饰），
 * 和普通书架拉开差距的关键是"展示品本身"。
 */
export function buildDisplayCabinet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });
  const glass = new THREE.MeshPhysicalMaterial({ color: 0xffffff, transparent: true, opacity: 0.12, roughness: 0.05, side: THREE.DoubleSide });

  g.add(m(box(w, h, d), walnut, [0, h / 2, 0], 'both'));
  g.add(m(new THREE.PlaneGeometry(w - 0.05, h - 0.1), glass, [0, h / 2, d / 2 + 0.01], 'none'));
  g.add(m(box(w - 0.1, 0.02, 0.05), emissive('#FFF8E0'), [0, h - 0.05, 0], 'none'));

  for (const y of [h * 0.3, h * 0.55, h * 0.8]) {
    g.add(m(box(w - 0.1, 0.02, d - 0.1), walnut, [0, y, 0], 'none'));
  }

  const terracotta = toon(C('terracotta', '#B86B4A'));
  g.add(m(cyl(0.05, 0.08, 0.15, 12), terracotta, [-w / 4, h * 0.3 + 0.075, 0], 'none'));
  g.add(m(sph(0.05, 12, 10), brass, [w / 4, h * 0.55 + 0.05, 0], 'none'));
  g.add(m(box(0.08, 0.12, 0.08), brass, [0, h * 0.8 + 0.06, 0], 'none'));

  return g;
}

/* ---------------- 酒柜（餐厅） ---------------- */

/**
 * 餐厅酒柜：玻璃门 + LED + 斜放酒瓶架。斜放的酒瓶是酒柜
 * 和普通柜子的唯一视觉差异，必须做出来。
 */
export function buildWineCabinet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const glass = new THREE.MeshPhysicalMaterial({ color: 0xffffff, transparent: true, opacity: 0.12, roughness: 0.05, side: THREE.DoubleSide });
  const bottle = toon('#3a4a3a');
  const bottleCap = toon(C('brass', '#b3aea2'), { finish: 'metal' });

  g.add(m(box(w, h, d), walnut, [0, h / 2, 0], 'both'));
  g.add(m(new THREE.PlaneGeometry(w - 0.05, h - 0.1), glass, [0, h / 2, d / 2 + 0.01], 'none'));
  g.add(m(box(w - 0.1, 0.02, 0.05), emissive('#FFF8E0'), [0, h - 0.05, 0], 'none'));

  const rows = 4, cols = 3;
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const x = -w / 2 + (c + 0.5) * (w / cols);
      const y = h * 0.12 + r * (h * 0.18);
      const bg = new THREE.Group();
      bg.position.set(x, y, 0);
      bg.rotation.z = Math.PI / 4;
      bg.add(m(cyl(0.03, 0.03, 0.18, 8), bottle, [0, 0, 0], 'none'));
      bg.add(m(cyl(0.012, 0.012, 0.05, 8), bottleCap, [0, 0.115, 0], 'none'));
      g.add(bg);
    }
  }

  return g;
}

/* ---------------- 大理石岛台（厨房陈述件） ---------------- */

/**
 * 厨房岛台：大理石台面（边缘外挑）+ 胡桃木柜体 + 黄铜五金。
 * 和普通厨房台面的差距全在"大理石 + 黄铜"上。
 */
export function buildIslandKitchen(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const marble = toon('#ffffff', { map: marbleTexture() });
  const walnut = toon(C('walnut', '#a8875f'), { map: furnitureWoodTexture() });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });

  g.add(m(box(w + 0.06, 0.04, d + 0.06), marble, [0, h - 0.02, 0], 'both'));
  g.add(m(box(w, h - 0.04, d), walnut, [0, (h - 0.04) / 2, 0], 'both'));
  g.add(m(box(w, 0.02, d), brass, [0, 0.01, 0], 'none'));
  for (let i = 0; i < 4; i++) {
    g.add(m(box(0.15, 0.02, 0.02), brass, [-w / 2 + 0.3 + (i * (w - 0.6)) / 3, h * 0.7, d / 2], 'both'));
  }

  return g;
}

/* ---------------- 电竞桌（电竞房陈述件） ---------------- */

/**
 * 电竞桌：黑色桌面 + 双屏（主+竖屏副）+ 机械键盘背光 + RGB 灯条。
 * 和普通书桌的差距在"双屏 + 背光 + RGB"这三件上。
 */
export function buildGamingDesk(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const dark = toon(C('dark', '#3a3630'));
  const metal = toon(C('brass', '#b3aea2'), { finish: 'metal' });
  const screen = emissive('#1a2332', { map: screenTexture() });

  g.add(m(box(w, 0.05, d), dark, [0, h - 0.025, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.05, h - 0.05, 0.05), metal, [sx * (w / 2 - 0.1), (h - 0.05) / 2, sz * (d / 2 - 0.1)], 'both'));
  }

  const monW = 0.5, monH = 0.3;
  const mon1 = new THREE.Group();
  mon1.position.set(-0.15, h + 0.15, -d / 2 + 0.1);
  mon1.add(m(box(monW, monH, 0.03), dark, [0, 0, 0], 'both'));
  mon1.add(m(new THREE.PlaneGeometry(monW - 0.04, monH - 0.04), screen, [0, 0, 0.02], 'none'));
  g.add(mon1);

  const mon2 = new THREE.Group();
  mon2.position.set(0.32, h + 0.15, -d / 2 + 0.1);
  mon2.rotation.y = -0.3;
  mon2.add(m(box(0.25, 0.45, 0.03), dark, [0, 0, 0], 'both'));
  mon2.add(m(new THREE.PlaneGeometry(0.21, 0.41), screen, [0, 0, 0.02], 'none'));
  g.add(mon2);

  g.add(m(box(0.4, 0.03, 0.15), emissive('#4a5568'), [-0.1, h + 0.015, 0.1], 'none'));
  g.add(m(box(0.06, 0.03, 0.1), dark, [0.25, h + 0.015, 0.1], 'both'));

  const rgbColors = ['#b5705e', '#d8c08a', '#7fa88a', '#9dbdd0', '#8f8fa8'];
  for (let i = 0; i < 5; i++) {
    g.add(m(box(w / 5 - 0.02, 0.02, 0.02), emissive(rgbColors[i]), [-w / 2 + (i + 0.5) * (w / 5), h - 0.06, d / 2 + 0.01], 'none'));
  }

  return g;
}

/* ---------------- 大理石茶几（客厅陈述件） ---------------- */

/**
 * 大理石茶几：大理石台面 + 黄铜细腿。和普通木茶几的差距
 * 全在"大理石 + 细腿"——细腿让茶几看起来是"飘"在地毯上的。
 */
export function buildMarbleTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const marble = toon('#ffffff', { map: marbleTexture() });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });

  g.add(m(box(w, 0.05, d), marble, [0, h - 0.025, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.025, 0.025, h - 0.05, 12), brass, [sx * (w / 2 - 0.1), (h - 0.05) / 2, sz * (d / 2 - 0.1)], 'both'));
  }

  return g;
}

/**
 * 专用家具（替换此前被复用的 shelf / fridge / cushionItem）。
 *
 * 背景：之前"穿衣镜""鞋柜""洗衣机""吧台凳"都是拿书架、冰箱、座布団顶替的，
 * 于是镜子里长出一排书、洗衣机上贴着冰箱贴、吧台凳是个带流苏的粉坐垫。
 * 这些错位的根源是 kind 复用，只能靠补专用件解决，改配色救不了。
 *
 * 造型原则（北欧日式）：细腿、浅木、圆角、无装饰线脚，靠比例而不是花纹。
 */

/** 书柜：柜体 + 层板 + 书。面朝 +Z（背板贴 -Z 墙）。size = [宽X, 高, 深Z]。 */
export function buildBookcase(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];

  const woodMat = toon('#ffffff', { map: furnitureWoodTexture() });
  const backMat = toon(C('shelfBack', '#b09070'));

  // 背板 + 两侧板
  g.add(m(box(W, H, 0.02), backMat, [0, H / 2, -D / 2 + 0.01], 'both'));
  for (const s of [-1, 1]) {
    g.add(m(box(0.026, H, D), woodMat, [s * (W / 2 - 0.013), H / 2, 0], 'both'));
  }

  // 层板：0.42m 层距，顶格留给绿植
  const nShelf = Math.max(1, Math.floor((H - 0.5) / 0.42));
  const shelfYs: number[] = [0.03];
  for (let i = 1; i <= nShelf; i++) shelfYs.push(Math.min(H - 0.45, i * 0.42));
  for (const y of shelfYs) {
    g.add(m(box(W - 0.05, 0.026, D - 0.01), woodMat, [0, y, 0], 'both'));
  }

  // 每层一排书。满墙的书是压迫感，随机留空格才是书柜该有的样子
  const bookCols = ['#c2a49c', '#a4b8c8', '#ddcba8', '#a8b8ac', '#d6c3a2', '#bda49e'];
  for (let li = 0; li < shelfYs.length; li++) {
    const y0 = shelfYs[li];
    const y1 = li + 1 < shelfYs.length ? shelfYs[li + 1] : H - 0.06;
    const bookH = Math.min(0.30, (y1 - y0) * 0.82);
    if (bookH < 0.12) continue;
    const rnd = makeRng(900 + li * 31);
    let x = -W / 2 + 0.05;
    while (x < W / 2 - 0.16) {
      const bw = 0.03 + rnd() * 0.025;
      if (rnd() > 0.24) {
        const tilt = rnd() > 0.85 ? (rnd() - 0.5) * 0.2 : 0;
        // 书是「厚 bw 沿 X（书脊并排的方向）、高 bookH 沿 Y、进深沿 Z」。
        // 旧代码把 X/Z 写反了：每本书在 X 上占 0.22m 却按 5cm 步进排，
        // 整排书互相叠成一坨，Z 向上又薄成 3cm 的纸片。
        const bk = m(box(bw, bookH, (D - 0.06) * 0.78), toon(bookCols[Math.floor(rnd() * bookCols.length)]),
          [x + bw / 2, y0 + 0.013 + bookH / 2, 0], 'cast');
        bk.rotation.z = tilt;
        g.add(bk);
      }
      x += bw + 0.004;
    }
  }

  // 顶格一盆绿植，仅此一件
  const plant = buildPlant(0.85);
  plant.position.set(W / 2 - 0.14, H - 0.45 + 0.013, 0);
  g.add(plant);

  return g;
}

/**
 * 细腿：日式家具的典型特征，腿径 30mm 左右，末端微收。
 *
 * 网格原点在腿的「顶端」——也就是与座板 / 柜底相接的那一头，柱体向下延伸到
 * y = -h。这样调用方把腿摆到接缝高度再旋转，旋转中心就正好是接缝点：
 * 腿脚只会向外撇开，绝不会绕腿中点旋转而把半截腿转到地面以下。
 *（旧实现把原点留在腿中点，调用方又用 position.set(x, 0, z) 把 y 覆盖回 0，
 *  结果每条腿都有一半埋进地板。）
 */
function taperLeg(h: number, r = 0.018, mat: THREE.Material) {
  const geo = cyl(r * 0.82, r, h, 10);
  geo.translate(0, -h / 2, 0);
  return m(geo, mat, [0, 0, 0], 'cast');
}

/** 穿衣镜：全身镜。size = [厚X, 高Y, 宽Z]，镜面朝 +X。 */
export function buildMirror(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [, H, W] = spec.size;
  const frameMat = toon('ffffff', { map: furnitureWoodTexture() });
  // 镜面用低饱和的冷灰：三渲二里没有真实反射，靠"比周围暗一档 + 冷色相"
  // 让眼睛把它读成玻璃，而不是一块灰板。
  const glassMat = toon('#b8c2c4');

  // 外框（细木框，40mm 见方）
  const fw = 0.04;
  for (const s of [-1, 1]) {
    g.add(m(box(0.06, H, fw), frameMat, [0, H / 2, s * (W / 2 - fw / 2)], 'both'));
  }
  g.add(m(box(0.06, fw, W - fw * 2), frameMat, [0, H - fw / 2, 0], 'both'));
  g.add(m(box(0.06, fw, W - fw * 2), frameMat, [0, fw / 2, 0], 'both'));

  // 镜面本体
  g.add(m(box(0.02, H - fw * 2, W - fw * 2), glassMat, [0.005, H / 2, 0], 'receive'));

  // 一道斜向的高光带：静态贴图模拟反光，避免整片死板。
  //
  // 镜面是 YZ 平面、法线朝 +X，高光片必须平贴在镜面上。旧代码写成
  // rotation.set(π/2, 0, 0.32)，把面片翻成了水平——一片 0.17×1.31m 的板子
  // 横插在镜子外面，既穿墙又让包围盒虚胖。这里改用两级变换：先在自身平面内
  // 绕法线斜 0.32rad，再由 pivot 把整片面转到朝 +X，方向不会歧义。
  const sheenPivot = new THREE.Group();
  sheenPivot.position.set(0.017, H / 2, -W * 0.12);
  sheenPivot.rotation.y = Math.PI / 2; // 面片法线 +Z 转到 +X，与镜面同向
  const sheen = m(
    new THREE.PlaneGeometry(W * 0.34, (H - fw * 2) * 0.86),
    toon('#d4dadd', { transparent: true, opacity: 0.5 }),
    [0, 0, 0], 'none'
  );
  sheen.rotation.z = 0.32; // 绕自身法线斜过来
  sheenPivot.add(sheen);
  g.add(sheenPivot);

  // 落地的斜撑脚：让镜子有明确的支撑关系，不再像一块贴地的板
  for (const s of [-1, 1]) {
    const foot = m(box(0.05, 0.06, 0.05), frameMat, [s * 0.005, 0.03, s * (W / 2 - fw / 2)], 'cast');
    g.add(foot);
  }

  return g;
}

/** 玄关鞋柜：下半封闭柜门 + 中段开放格 + 木质台面。size = [进深X, 高Y, 宽Z] */
export function buildShoeCabinet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], W = spec.size[2];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const panelMat = toon(C('door', '#EDE8DE'));
  const pullMat = toon(C('metal', '#B0AEA6'), { finish: 'metal' });

  // 柜体落地。原本做成悬空 80mm，但没有支撑件就是"漂浮物体"，
  // 改成底部踢脚内收 60mm——视觉上仍有轻盈的阴影缝，但柜子是实打实落在地上的。
  const kick = 0.06;
  g.add(m(box(D, H - kick, W), panelMat, [0, kick + (H - kick) / 2, 0], 'both'));
  g.add(m(box(D - 0.08, kick, W - 0.06), toon('#D6D2CA'), [0.02, kick / 2, 0], 'cast'));

  // 两扇柜门 + 中缝
  for (const s of [-1, 1]) {
    g.add(m(box(0.02, H - kick - 0.32, W / 2 - 0.012), woodMat,
      [D / 2 + 0.005, kick + (H - kick + 0.32) / 2, s * (W / 4)], 'both'));
  }
  // 隐形拉手：一条横向的凹槽色块，比凸出的把手更克制
  for (const s of [-1, 1]) {
    g.add(m(box(0.012, 0.016, 0.16), pullMat,
      [D / 2 + 0.016, H - 0.30, s * (W / 4) + s * 0.06], 'none'));
  }

  // 顶部台面（略微出檐）
  g.add(m(box(D + 0.04, 0.035, W + 0.04), woodMat, [0.01, H + 0.017, 0], 'both'));

  // 台面上的生活痕迹：一个小托盘 + 一只马克杯，仅此而已
  g.add(m(box(D * 0.5, 0.014, 0.16), toon(C('terracotta', '#B5836E')), [0.01, H + 0.041, -W * 0.22], 'cast'));
  const cup = m(cyl(0.032, 0.028, 0.085, 14), toon('#EFE8DC'), [0.01, H + 0.077, W * 0.18], 'cast');
  g.add(cup);

  return g;
}

/** 换鞋凳：细腿木质长凳。size = [进深X, 座高Y, 长Z] */
export function buildBench(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], W = spec.size[2];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const cushionMat = toon(C('linen', '#EDE8DE'));

  // 座板
  g.add(m(box(D, 0.05, W), woodMat, [0, H - 0.025, 0], 'both'));
  // 薄坐垫（只盖住座面，不包边）
  g.add(m(box(D - 0.03, 0.035, W - 0.05), cushionMat, [0, H + 0.017, 0], 'both'));

  // 四根细腿，微微外撇：腿顶钉在座板底面（H-0.05），绕顶端旋转让脚向外张开
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    const leg = taperLeg(H - 0.05, 0.019, woodMat);
    leg.position.set(sx * (D / 2 - 0.05), H - 0.05, sz * (W / 2 - 0.06));
    leg.rotation.z = sx * 0.04;
    leg.rotation.x = -sz * 0.04;
    g.add(leg);
  }
  // 横撑
  g.add(m(box(0.024, 0.024, W - 0.12), woodMat, [0, H * 0.32, 0], 'cast'));

  return g;
}

/** 滚筒洗衣机 / 烘干机。size = [宽X, 高Y, 深Z]，正面（舱门）朝 +X。 */
export function buildWasher(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const body = toon(C('fridge', '#EDEAE3'));
  const doorMat = toon('#c3c8cc');
  const glassMat = toon('#8e9aa0');
  const trim = toon(C('metal', '#B0AEA6'), { finish: 'metal' });

  g.add(m(box(W, H, D), body, [0, H / 2, 0], 'both'));

  // 顶部控制条（正面 +X 面）
  g.add(m(box(W - 0.04, 0.075, D - 0.05), toon('#DFDBD2'), [0, H - 0.055, 0], 'both'));
  // 旋钮 + 两个按键：都放在正面（+X 面），与滚筒（玻璃视窗）同一面
  const knob = m(cyl(0.032, 0.032, 0.022, 16), trim, [W / 2 + 0.011, H - 0.038, -0.10], 'cast');
  knob.rotation.z = Math.PI / 2; // 轴沿 X：钮凸出正面，与滚筒同面
  g.add(knob);
  for (let i = 0; i < 2; i++) {
    g.add(m(box(0.012, 0.026, 0.03), trim, [W / 2 + 0.006, H - 0.038, 0.0 + i * 0.09], 'none'));
  }

  // 舱门：外圈金属环 + 内嵌玻璃视窗
  const ring = m(cyl(0.20, 0.20, 0.03, 28), doorMat, [0, 0, 0], 'cast');
  ring.rotation.z = Math.PI / 2;
  ring.position.set(W / 2 + 0.012, H * 0.55, 0);
  g.add(ring);
  const glass = m(cyl(0.155, 0.155, 0.024, 26), glassMat, [0, 0, 0], 'cast');
  glass.rotation.z = Math.PI / 2;
  glass.position.set(W / 2 + 0.026, H * 0.55, 0);
  g.add(glass);

  // 底座踢脚
  g.add(m(box(W - 0.02, 0.06, D - 0.02), toon('#D6D2CA'), [0, 0.03, 0], 'cast'));

  return g;
}

/** 餐边柜：矮柜 + 台面摆件。size = [进深X, 高Y, 宽Z] */
export function buildSideboard(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], W = spec.size[2];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const panelMat = toon(C('door', '#EDE8DE'));

  // 柜体悬空 0.10 由细腿撑起，不做整柜贴地——矮柜离开地面才有轻盈感
  const legH = 0.10;
  g.add(m(box(D, H - legH, W), panelMat, [0, legH + (H - legH) / 2, 0], 'both'));
  // 两扇对开门 + 细缝
  for (const s of [-1, 1]) {
    g.add(m(box(0.018, H - legH - 0.10, W / 2 - 0.008), woodMat,
      [D / 2 + 0.004, legH + (H - legH) / 2, s * (W / 4)], 'both'));
  }
  // 台面出檐
  g.add(m(box(D + 0.05, 0.032, W + 0.05), woodMat, [0.012, H + 0.016, 0], 'both'));

  // 台面：一个花瓶 + 一叠书 + 一只陶碗——生活感靠三件小物带出，不再堆
  const vase = m(cyl(0.045, 0.058, 0.20, 16), toon(C('accent', '#8FA98D')), [0.012, H + 0.132, -W * 0.26], 'cast');
  g.add(vase);
  for (let i = 0; i < 3; i++) {
    g.add(m(box(D * 0.55, 0.022, 0.16), toon(['#C6CFCE', '#D8CDB8', '#BFA98A'][i]),
      [0.012, H + 0.043 + i * 0.024, W * 0.18], 'cast'));
  }
  const bowl = m(sph(0.075, 16, 10), toon(C('terracotta', '#B5836E')), [0.012, H + 0.048, -W * 0.02], 'cast');
  bowl.scale.set(1, 0.5, 1);
  g.add(bowl);

  // 细腿：从地面撑到柜底。腿顶钉在柜体底面 y = legH，绕顶端旋转微微外撇
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    const leg = taperLeg(legH, 0.016, woodMat);
    leg.position.set(sx * (D / 2 - 0.05), legH, sz * (W / 2 - 0.07));
    leg.rotation.z = sx * 0.035;
    leg.rotation.x = -sz * 0.035;
    g.add(leg);
  }
  return g;
}

/** 浴室洗手台柜：台上盆 + 龙头 + 柜体 + 镜柜。size = [宽X, 高Y, 深Z]，正面朝 +X。 */
export function buildVanity(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const stoneMat = toon(C('marble', '#F2F0EA'));
  const metalMat = toon(C('metal', '#B0AEA6'), { finish: 'metal' });

  // 柜体（悬空，底部离地 0.15）
  g.add(m(box(W, H - 0.15, D), woodMat, [0, 0.15 + (H - 0.15) / 2, 0], 'both'));
  // 台面
  g.add(m(box(W + 0.03, 0.04, D + 0.03), stoneMat, [0, H + 0.02, 0], 'both'));

  // 台上盆（椭圆柱 + 内凹）
  const basin = m(cyl(0.21, 0.18, 0.13, 24), stoneMat, [0, H + 0.105, 0], 'both');
  g.add(basin);
  g.add(m(cyl(0.175, 0.165, 0.03, 22), toon('#DCD8CE'), [0, H + 0.155, 0], 'receive'));

  // 龙头：竖管 + 弯嘴
  g.add(m(cyl(0.018, 0.018, 0.22, 12), metalMat, [0, H + 0.15, -D / 2 + 0.07], 'cast'));
  const spout = m(cyl(0.014, 0.014, 0.13, 10), metalMat, [0, H + 0.255, -D / 2 + 0.13], 'cast');
  spout.rotation.x = Math.PI / 2;
  g.add(spout);

  // 镜柜：贴墙竖在台面上方，照明用自发光条
  const mw = W - 0.08, mh = 0.72;
  g.add(m(box(0.06, mh, mw), woodMat, [-D / 2 + 0.03, H + 0.30 + mh / 2, 0], 'both'));
  g.add(m(box(0.012, mh - 0.06, mw - 0.06), toon('#b8c2c4'),
    [-D / 2 + 0.062, H + 0.30 + mh / 2, 0], 'receive'));
  // 镜前灯带
  g.add(m(box(0.02, 0.018, mw - 0.10), emissive('#FFF3DC'),
    [-D / 2 + 0.068, H + 0.30 + mh + 0.02, 0], 'none'));

  // 台面上的小物：一只皂碟
  g.add(m(box(0.09, 0.014, 0.07), toon('#EFE8DC'), [W / 2 - 0.10, H + 0.047, D / 2 - 0.10], 'cast'));

  return g;
}

/** 边几：细腿小圆几。size = [直径, 高, -] */
export function buildSideTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const R = spec.size[0] / 2, H = spec.size[1];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });

  g.add(m(cyl(R, R, 0.03, 28), woodMat, [0, H - 0.015, 0], 'both'));
  // 三根细腿，比四根更透气
  for (let i = 0; i < 3; i++) {
    const a = (i / 3) * Math.PI * 2 + 0.4;
    const leg = taperLeg(H - 0.03, 0.015, woodMat);
    leg.position.set(Math.cos(a) * R * 0.68, H - 0.03, Math.sin(a) * R * 0.68);
    leg.rotation.z = Math.cos(a) * 0.05;
    leg.rotation.x = -Math.sin(a) * 0.05;
    g.add(leg);
  }
  // 桌上一只马克杯
  g.add(m(cyl(0.033, 0.029, 0.088, 14), toon('#EFE8DC'), [R * 0.3, H + 0.044, -R * 0.2], 'cast'));
  return g;
}

/** 吧台凳：细腿圆凳。size = [座面直径, 座高, -] */
export function buildStool(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const R = spec.size[0] / 2, H = spec.size[1];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const seatMat = toon(C('linen', '#EDE8DE'));

  g.add(m(cyl(R, R, 0.045, 24), woodMat, [0, H - 0.022, 0], 'both'));
  g.add(m(cyl(R - 0.008, R - 0.008, 0.028, 24), seatMat, [0, H + 0.014, 0], 'both'));
  for (let i = 0; i < 4; i++) {
    const a = (i / 4) * Math.PI * 2 + 0.6;
    const leg = taperLeg(H - 0.045, 0.014, woodMat);
    leg.position.set(Math.cos(a) * R * 0.72, H - 0.045, Math.sin(a) * R * 0.72);
    leg.rotation.z = Math.cos(a) * 0.07;
    leg.rotation.x = -Math.sin(a) * 0.07;
    g.add(leg);
  }
  // 脚踏圈
  const ringR = R * 0.78, ringH = H * 0.30;
  const ring = m(new THREE.TorusGeometry(ringR, 0.009, 6, 20), woodMat, [0, ringH, 0], 'cast');
  ring.rotation.x = Math.PI / 2;
  g.add(ring);
  return g;
}

/** 休闲椅：一把简洁的单椅（木框 + 软座 + 软靠）。size = [宽X, 高Y, 深Z] */
export function buildLoungeChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const woodMat = toon('ffffff', { map: furnitureWoodTexture() });
  const fabMat = toon(C('sofa', '#B8B2A4'));

  const seatY = H * 0.44;
  // 座面 + 靠背（靠背略微后仰）
  g.add(m(box(W, 0.10, D * 0.86), fabMat, [0, seatY, 0], 'both'));
  const back = m(box(W, H * 0.46, 0.09), fabMat, [0, seatY + H * 0.23, -D / 2 + 0.05], 'both');
  back.rotation.x = -0.12;
  g.add(back);
  // 一根软垫搭在靠背上，带出"有人坐过"的松弛感
  const pad = m(box(W * 0.62, 0.20, 0.07), toon(C('linen', '#EDE8DE')), [0, seatY + 0.18, -D / 2 + 0.11], 'both');
  pad.rotation.x = -0.16;
  g.add(pad);
  // 木框：两条扶手 + 四腿
  for (const s of [-1, 1]) {
    g.add(m(box(0.035, 0.06, D * 0.86), woodMat, [s * (W / 2 - 0.018), seatY + 0.10, 0], 'both'));
  }
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    const leg = taperLeg(seatY - 0.05, 0.018, woodMat);
    leg.position.set(sx * (W / 2 - 0.05), seatY - 0.05, sz * (D / 2 - 0.09));
    leg.rotation.z = sx * 0.05;
    leg.rotation.x = -sz * 0.05;
    g.add(leg);
  }
  return g;
}

/* ---------------- 独立トイレ（马桶间专用件） ---------------- */

/**
 * 独立马桶间：马桶 + 角落小洗手台 + 毛巾杆。日式住宅把トイレ从浴室里分出来，
 * 这间只有 1.2×2.0m，洁具沿 -Z 墙排、洗手台塞在 +X 角。
 */
export function buildToilet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, , d] = spec.size;
  const porcelain = toon('#f7f8f6');
  const metal = toon(C('metal', '#b9bcc4'), { finish: 'metal' });
  const wood = toon('#ffffff', { map: furnitureWoodTexture() });

  /* 马桶：水箱贴 -Z，朝 +Z */
  const toilet = new THREE.Group();
  toilet.add(m(box(0.36, 0.20, 0.50), porcelain, [0, 0.10, 0], 'both'));
  const bowl = m(cyl(0.19, 0.16, 0.14, 16), porcelain, [0, 0.24, 0.05], 'cast');
  bowl.scale.set(1, 1, 1.25);
  toilet.add(bowl);
  toilet.add(m(cyl(0.17, 0.17, 0.012, 16), toon('#eef0ee'), [0, 0.315, 0.05], 'none'));
  toilet.add(m(box(0.36, 0.42, 0.16), porcelain, [0, 0.32, -0.20], 'both'));
  toilet.add(m(box(0.14, 0.025, 0.04), metal, [0, 0.46, -0.12], 'cast'));
  toilet.position.set(-w / 2 + 0.28, 0, -d / 2 + 0.30);
  g.add(toilet);

  /* 角落洗手台：贴 +X 墙 */
  const sink = new THREE.Group();
  sink.add(m(box(0.34, 0.50, 0.30), wood, [0, 0.55, 0], 'both'));
  sink.add(m(box(0.38, 0.04, 0.34), porcelain, [0, 0.82, 0], 'both'));
  sink.add(m(cyl(0.12, 0.10, 0.08, 16), porcelain, [0, 0.86, 0], 'cast'));
  sink.add(m(cyl(0.010, 0.010, 0.18, 8), metal, [0, 0.95, 0.10], 'cast'));
  sink.position.set(w / 2 - 0.20, 0, d / 2 - 0.22);
  g.add(sink);

  /* 毛巾杆：贴 -Z 墙，横杆绕 Z 转 90° 让圆柱沿 X 躺倒 */
  g.add(m(cyl(0.012, 0.012, 0.36, 8), metal, [w / 2 - 0.24, 0.95, -d / 2 + 0.06], 'cast').rotateZ(Math.PI / 2));

  return g;
}

/* ---------------- 弧形沙发（客厅陈述件） ---------------- */

/**
 * 弧形沙发：三段拼弧 + 绒面 + 金属脚。弧线是客厅的构图陈述件，
 * 面料色随 palette.accent（当前为柔和绿），不再是固定的深绿天鹅绒。
 */
export function buildCurvedSofa(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const [w, h, d] = spec.size;
  const velvet = toon(C('accent', '#5A6E54'), { map: velvetTexture(C('accent', '#5A6E54')) });
  const brass = toon(C('brass', '#b3aea2'), { finish: 'metal' });

  const segW = w / 3;
  const seatH = h * 0.55;
  const backH = h * 0.45;

  g.add(m(box(segW, seatH, d * 0.7), velvet, [0, seatH / 2, 0], 'both'));
  g.add(m(box(segW, backH, 0.2), velvet, [0, seatH + backH / 2, -d * 0.25], 'both'));

  const left = new THREE.Group();
  left.position.set(-segW, 0, 0.05);
  left.rotation.y = 0.26;
  left.add(m(box(segW, seatH, d * 0.7), velvet, [0, seatH / 2, 0], 'both'));
  left.add(m(box(segW, backH, 0.2), velvet, [0, seatH + backH / 2, -d * 0.25], 'both'));
  g.add(left);

  const right = new THREE.Group();
  right.position.set(segW, 0, 0.05);
  right.rotation.y = -0.26;
  right.add(m(box(segW, seatH, d * 0.7), velvet, [0, seatH / 2, 0], 'both'));
  right.add(m(box(segW, backH, 0.2), velvet, [0, seatH + backH / 2, -d * 0.25], 'both'));
  g.add(right);

  const cushionW = segW - 0.1;
  g.add(m(box(cushionW, 0.15, 0.2), velvet, [-segW, seatH + 0.075, 0.05], 'both'));
  g.add(m(box(cushionW, 0.15, 0.2), velvet, [0, seatH + 0.075, 0], 'both'));
  g.add(m(box(cushionW, 0.15, 0.2), velvet, [segW, seatH + 0.075, 0.05], 'both'));

  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.03, 0.03, 0.05, 8), brass, [sx * (w / 2 - 0.1), 0.025, sz * (d * 0.35 - 0.1)], 'cast'));
  }

  return g;
}

/* ============================================================================
 * 7. 日式公寓 LDK 重设计家具（《你的名字》泷的日常公寓风）
 *    全部新几何 + 新 jp* 纹理 + 新 jp* palette，不复用上面任何 buildXxx。
 *    约定：原点 = 地面落脚中心，局部 y=0 = 地板；size 语义逐函数注明。
 * ========================================================================== */

/* --- 共享材质（toon 内部按 key 缓存，重复调用零成本） -------------------- */
const jpWood = () => toon('#ffffff', { map: jpWoodGrainTexture() });
const jpOakSolid = () => toon(C('jpOak', '#c8a87c'));
const jpOakDark = () => toon(C('jpOakDark', '#9a7b52'));
const jpCream = () => toon(C('jpCream', '#f0e9dc'));
const jpIndigo = () => toon(C('jpIndigo', '#3e5c76'), { map: jpFabricTexture() });
const jpIndigoDeep = () => toon(C('jpIndigoDeep', '#2c4257'));
const jpGrayFabric = () => toon(C('jpGrayWarm', '#b9b2a6'), { map: jpFabricTexture() });
const jpMetal = () => toon(C('jpMetalGray', '#a9adb3'), { map: jpMetalTexture(), finish: 'metal' });
const jpBlack = () => toon(C('jpBlack', '#2b2e33'));
const jpCeramic = () => toon('#ffffff', { map: jpCeramicTexture() });
const jpCeramicBlue = () => toon(C('jpCeramicBlue', '#7c99ac'), { map: jpCeramicTexture() });
const jpCounter = () => toon('#ffffff', { map: jpCounterTopTexture() });
const jpBook = () => toon('#ffffff', { map: jpPaperBookTexture() });
const jpShoji = () => toon('#ffffff', { map: jpShojiPaperTexture(), side: THREE.DoubleSide });
const jpPlantGreen = () => toon(C('jpPlantGreen', '#6e8b5e'));
const jpPlantGreenDark = () => toon('#556b48');
const jpTerra = () => toon(C('jpTerra', '#b5715a'));
const jpTileWarm = () => toon(C('jpTileWarm', '#cfc7b8'));

/* ---------------- 客厅 ---------------- */

/** 开放书架。size = [进深X, 高Y, 长Z]，开口朝 +X（背靠 -X 墙）。 */
export function buildJpBookRack(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  const t = 0.028;
  const wood = jpWood();
  const innerL = L - t * 2 - 0.02;

  // 背板 + 两侧板 + 顶底板
  g.add(m(box(0.018, H - t, L - t), jpOakSolid(), [-D / 2 + 0.009, H / 2, 0], 'receive'));
  for (const sz of [-1, 1]) g.add(m(box(D, H, t), wood, [0, H / 2, sz * (L / 2 - t / 2)], 'both'));
  g.add(m(box(D, t, L), wood, [0, t / 2, 0], 'both'));
  g.add(m(box(D, t, L), wood, [0, H - t / 2, 0], 'both'));

  // 隔板
  const boards = [H * 0.27, H * 0.52, H * 0.76];
  for (const y of boards) g.add(m(box(D - 0.02, t * 0.8, L - t * 2), wood, [0.006, y, 0], 'both'));

  // 每格塞书 / 杂物
  const rng = makeRng(4401);
  const levels = [t / 2, ...boards, H - t / 2];
  for (let i = 0; i < levels.length - 1; i++) {
    const yBot = levels[i] + t * 0.5;
    const yTop = levels[i + 1] - t * 0.5;
    const compH = Math.max(0.14, yTop - yBot);
    const bd = D - 0.12;
    const bx = -D / 2 + 0.04 + bd / 2;
    if (i === 2) {
      // 中层留白：陶罐 + 一叠横放的书
      g.add(m(cyl(0.055, 0.045, 0.13, 14), jpCeramicBlue(), [bx, yBot + 0.065, -innerL * 0.24], 'cast'));
      g.add(m(box(bd, 0.028, 0.19), jpBook(), [bx, yBot + 0.014, innerL * 0.2], 'cast'));
      g.add(m(box(bd * 0.9, 0.026, 0.17), jpIndigoDeep(), [bx, yBot + 0.041, innerL * 0.2], 'cast'));
      continue;
    }
    const blockLen = innerL * (0.55 + rng() * 0.34);
    const bh = compH * (0.80 + rng() * 0.16);
    const startZ = -innerL / 2 + 0.01;
    g.add(m(box(bd, bh, blockLen), jpBook(), [bx, yBot + bh / 2, startZ + blockLen / 2], 'cast'));
    const rest = innerL - blockLen - 0.03;
    if (rest > 0.07 && rng() > 0.45) {
      const lean = m(box(bd * 0.9, compH * 0.78, 0.028), jpBook(), [bx, yBot + compH * 0.39, startZ + blockLen + rest * 0.5], 'cast');
      lean.rotation.x = 0.16;
      g.add(lean);
    }
  }
  return g;
}

/** 低电视柜 + 电视。size = [进深X, 柜高Y, 长Z]，屏幕朝 +X。 */
export function buildJpTvBoard(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  const wood = jpWood();

  // 柜体 + 四条矮腿
  g.add(m(box(D, H - 0.06, L), wood, [0, 0.06 + (H - 0.06) / 2, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.022, 0.018, 0.06, 8), jpOakDark(), [sx * (D / 2 - 0.07), 0.03, sz * (L / 2 - 0.12)], 'cast'));
  }
  // 两扇推拉门缝 + 长条木拉手（朝 +X）
  g.add(m(box(0.012, H - 0.14, 0.012), jpOakDark(), [D / 2 + 0.001, 0.06 + (H - 0.06) / 2, 0], 'none'));
  for (const sz of [-1, 1]) {
    g.add(m(box(0.02, 0.03, L * 0.28), jpMetal(), [D / 2 + 0.012, 0.06 + (H - 0.06) * 0.62, sz * L * 0.24], 'none'));
  }

  // 电视：底座 + 支架 + 边框 + 暗屏（夜间关机，冷灰反光）
  const tvY = H + 0.02;
  g.add(m(box(0.2, 0.018, 0.44), jpBlack(), [0, tvY + 0.009, 0], 'cast'));
  g.add(m(box(0.05, 0.14, 0.07), jpBlack(), [0, tvY + 0.08, 0], 'cast'));
  const scrH = 0.60, scrL = Math.min(1.06, L - 0.5);
  g.add(m(box(0.045, scrH, scrL), jpBlack(), [0, tvY + 0.15 + scrH / 2, 0], 'cast'));
  const screen = m(new THREE.PlaneGeometry(scrL - 0.05, scrH - 0.05), toon('#3a4250', { finish: 'glass' }), [0.024, tvY + 0.15 + scrH / 2, 0], 'none');
  screen.rotation.y = Math.PI / 2;
  g.add(screen);

  // 柜面小物：一只音箱 + 一叠影碟
  g.add(m(box(0.1, 0.16, 0.1), jpOakDark(), [0, H + 0.08, L / 2 - 0.16], 'cast'));
  g.add(m(box(0.16, 0.02, 0.14), jpCeramic(), [0.02, H + 0.01, -L / 2 + 0.2], 'cast'));
  return g;
}

/** 靛蓝低绒地毯。size = [宽X, 厚Y, 长Z]。装饰件，nav:false。 */
export function buildJpRug(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];
  g.add(m(new THREE.PlaneGeometry(W + 0.04, D + 0.04), jpIndigoDeep(), [0, 0.004, 0], 'receive').rotateX(-Math.PI / 2));
  const top = m(new THREE.PlaneGeometry(W, D), toon('#ffffff', { map: jpRugTexture() }), [0, 0.009, 0], 'receive');
  top.rotation.x = -Math.PI / 2;
  g.add(top);
  return g;
}

/** 矮布艺沙发。size = [进深X, 高Y, 长Z]，默认面朝 +X。 */
export function buildJpLowSofa(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  const seatH = 0.30;
  const fabric = jpIndigo();
  const frame = jpGrayFabric();

  // 底座 + 坐垫
  g.add(m(box(D, seatH, L), frame, [0, seatH / 2, 0], 'both'));
  const cushN = L > 1.4 ? 2 : 1;
  const cw = (L - 0.08) / cushN;
  for (let i = 0; i < cushN; i++) {
    const z = -L / 2 + 0.04 + cw * (i + 0.5);
    g.add(m(box(D - 0.24, 0.13, cw - 0.04), fabric, [0.04, seatH + 0.065, z], 'cast'));
  }
  // 靠背（-X 侧）
  g.add(m(box(0.18, H - seatH, L), frame, [-D / 2 + 0.09, seatH + (H - seatH) / 2, 0], 'both'));
  // 两个靠枕
  for (const sz of [-1, 1]) {
    const p = m(box(0.12, 0.30, 0.30), fabric, [-D / 2 + 0.22, seatH + 0.18, sz * (L / 2 - 0.34)], 'cast');
    p.rotation.z = -0.14;
    g.add(p);
  }
  // 矮腿
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.026, 0.022, 0.05, 8), jpOakDark(), [sx * (D / 2 - 0.09), 0.025, sz * (L / 2 - 0.12)], 'cast'));
  }
  return g;
}

/** 矮桌（含桌面杂物）。size = [宽X, 高Y, 深Z]。 */
export function buildJpCenterTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  g.add(m(box(W, 0.04, D), wood, [0, H - 0.02, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.05, H - 0.04, 0.05), jpOakDark(), [sx * (W / 2 - 0.08), (H - 0.04) / 2, sz * (D / 2 - 0.08)], 'cast'));
  }
  g.add(m(box(W - 0.24, 0.02, D - 0.18), wood, [0, 0.12, 0], 'receive'));

  // 桌面：马克杯 + 小钵 + 一本摊开的书 + 杯垫
  g.add(m(cyl(0.036, 0.03, 0.085, 14), jpCeramicBlue(), [W * 0.24, H + 0.042, -D * 0.18], 'cast'));
  const handle = m(new THREE.TorusGeometry(0.024, 0.006, 6, 12), jpCeramicBlue(), [W * 0.24 + 0.045, H + 0.045, -D * 0.18], 'cast');
  handle.rotation.y = Math.PI / 2;
  g.add(handle);
  const bowl = m(sph(0.07, 14, 10), jpCeramic(), [-W * 0.2, H + 0.02, D * 0.12], 'cast');
  bowl.scale.set(1, 0.5, 1);
  g.add(bowl);
  const bookFlat = m(box(0.2, 0.022, 0.15), jpBook(), [W * 0.02, H + 0.011, D * 0.05], 'cast');
  bookFlat.rotation.y = 0.2;
  g.add(bookFlat);
  g.add(m(cyl(0.06, 0.06, 0.006, 14), jpTerra(), [W * 0.24, H + 0.003, -D * 0.18], 'none'));
  return g;
}

/** 座布団。size = [宽X, 厚Y, 深Z]。装饰件，nav:false。 */
export function buildJpZabuton(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], t = spec.size[1], D = spec.size[2];
  g.add(m(box(W, t, D), jpIndigo(), [0, t / 2, 0], 'both'));
  g.add(m(sph(0.02, 8, 6), jpCream(), [0, t - 0.005, 0], 'none'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(sph(0.015, 8, 6), jpCream(), [sx * (W / 2 - 0.03), t * 0.4, sz * (D / 2 - 0.03)], 'none'));
  }
  return g;
}

/** 落地灯（障子纸罩，内置暖光）。 */
export function buildJpFloorLamp(_spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const metal = jpMetal();
  g.add(m(cyl(0.13, 0.15, 0.022, 18), jpOakDark(), [0, 0.011, 0], 'cast'));
  g.add(m(cyl(0.015, 0.015, 1.24, 8), metal, [0, 0.63, 0], 'cast'));
  // 三段障子纸罩
  const shade = m(cyl(0.15, 0.17, 0.42, 18, true), jpShoji(), [0, 1.42, 0], 'cast');
  g.add(shade);
  g.add(m(cyl(0.155, 0.155, 0.012, 18), jpOakDark(), [0, 1.21, 0], 'cast'));
  g.add(m(cyl(0.175, 0.175, 0.012, 18), jpOakDark(), [0, 1.63, 0], 'cast'));
  g.add(m(sph(0.05, 10, 8), emissive('#ffe9c8'), [0, 1.42, 0], 'none'));
  const light = new THREE.PointLight(0xffd9a0, 0.55, 3.6, 2);
  light.position.set(0, 1.42, 0);
  g.add(light);
  return g;
}

/** 盆栽。size[1] 忽略，固定株型。装饰件，nav:false。 */
export function buildJpPlant(_spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const rng = makeRng(6161);
  // 陶盆
  g.add(m(cyl(0.13, 0.10, 0.20, 16), jpTerra(), [0, 0.10, 0], 'cast'));
  g.add(m(cyl(0.135, 0.135, 0.022, 16), jpTerra(), [0, 0.20, 0], 'cast'));
  g.add(m(cyl(0.115, 0.115, 0.02, 14), toon('#4a3b30'), [0, 0.195, 0], 'none'));
  // 叶簇：若干扁球 + 细茎
  for (let i = 0; i < 9; i++) {
    const a = (i / 9) * Math.PI * 2 + rng() * 0.4;
    const r = 0.06 + rng() * 0.12;
    const y = 0.30 + rng() * 0.30;
    const leaf = m(sph(0.055 + rng() * 0.04, 10, 8), i % 2 ? jpPlantGreen() : jpPlantGreenDark(), [Math.cos(a) * r, y, Math.sin(a) * r], 'cast');
    leaf.scale.set(1, 0.55, 1.4);
    leaf.rotation.y = a;
    g.add(leaf);
    g.add(m(cyl(0.006, 0.006, y - 0.18, 5), jpPlantGreenDark(), [Math.cos(a) * r * 0.5, 0.18 + (y - 0.18) / 2, Math.sin(a) * r * 0.5], 'none'));
  }
  g.add(m(sph(0.07, 10, 8), jpPlantGreen(), [0, 0.52, 0], 'cast'));
  return g;
}

/** 地面杂物（书堆 / 杂志 / 玻璃瓶）。装饰件，nav:false。 */
export function buildJpFloorClutter(_spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const rng = makeRng(3131);
  // 一摞横放的书
  let y = 0.014;
  for (let i = 0; i < 4; i++) {
    const b = m(box(0.24 - i * 0.012, 0.028, 0.18 - i * 0.01), i % 2 ? jpBook() : jpIndigoDeep(), [0, y, 0], 'cast');
    b.rotation.y = (rng() - 0.5) * 0.4;
    g.add(b);
    y += 0.03;
  }
  // 一本竖靠的杂志
  const mag = m(box(0.02, 0.26, 0.19), jpBook(), [0.2, 0.12, 0.02], 'cast');
  mag.rotation.z = 0.22;
  g.add(mag);
  // 一只玻璃瓶 + 一个马克杯
  g.add(m(cyl(0.035, 0.04, 0.18, 12), toon(C('jpCeramicBlue', '#7c99ac'), { finish: 'glass', opacity: 0.85, transparent: true }), [-0.22, 0.09, 0.06], 'cast'));
  g.add(m(cyl(0.014, 0.014, 0.05, 10), toon(C('jpCeramicBlue', '#7c99ac'), { finish: 'glass', opacity: 0.85, transparent: true }), [-0.22, 0.20, 0.06], 'cast'));
  g.add(m(cyl(0.036, 0.03, 0.085, 14), jpCeramic(), [-0.08, 0.042, -0.16], 'cast'));
  return g;
}

/** 木格栅电视背景墙。size = [厚X, 高Y, 长Z]，贴 -X 墙。装饰件，nav:false。 */
export function buildJpSlatWall(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  g.add(m(box(0.012, H, L), jpOakDark(), [-D / 2 + 0.006, H / 2, 0], 'none'));
  const n = 15, gap = L / n;
  for (let i = 0; i < n; i++) {
    const z = -L / 2 + gap * (i + 0.5);
    g.add(m(box(D - 0.012, H, gap * 0.54), jpWood(), [0.006, H / 2, z], 'cast'));
  }
  return g;
}

/* ---------------- 餐厅 ---------------- */

/** 橡木餐桌（含餐具）。size = [宽X, 高Y, 深Z]。 */
export function buildJpDiningTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  g.add(m(box(W, 0.045, D), wood, [0, H - 0.022, 0], 'both'));
  g.add(m(box(W - 0.1, 0.06, 0.04), jpOakDark(), [0, H - 0.075, D / 2 - 0.08], 'none'));
  g.add(m(box(W - 0.1, 0.06, 0.04), jpOakDark(), [0, H - 0.075, -D / 2 + 0.08], 'none'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.06, H - 0.045, 0.06), jpOakDark(), [sx * (W / 2 - 0.1), (H - 0.045) / 2, sz * (D / 2 - 0.1)], 'cast'));
  }

  // 两套餐具 + 中央小花瓶
  for (const sx of [-1, 1]) {
    g.add(m(cyl(0.11, 0.1, 0.014, 18), jpCeramic(), [sx * W * 0.26, H + 0.007, 0], 'cast'));
    g.add(m(cyl(0.038, 0.032, 0.09, 14), jpCeramicBlue(), [sx * W * 0.26, H + 0.045, D * 0.24], 'cast'));
    g.add(m(box(0.02, 0.006, 0.22), jpOakDark(), [sx * W * 0.26 + 0.14, H + 0.003, -D * 0.1], 'none'));
  }
  const vase = m(cyl(0.04, 0.05, 0.16, 14), jpCeramic(), [0, H + 0.08, 0], 'cast');
  g.add(vase);
  for (let i = 0; i < 3; i++) {
    const a = (i / 3) * Math.PI * 2;
    const stem = m(cyl(0.005, 0.005, 0.2, 5), jpPlantGreenDark(), [Math.cos(a) * 0.02, H + 0.24, Math.sin(a) * 0.02], 'none');
    stem.rotation.z = Math.cos(a) * 0.3;
    stem.rotation.x = -Math.sin(a) * 0.3;
    g.add(stem);
    g.add(m(sph(0.028, 8, 6), i === 1 ? jpTerra() : jpCream(), [Math.cos(a) * 0.05, H + 0.33, Math.sin(a) * 0.05], 'cast'));
  }
  return g;
}

/** 餐椅。size = [宽X, 高Y, 深Z]，默认面朝 +Z（靠背在 -Z）。 */
export function buildJpDiningChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const seatH = 0.42;
  const wood = jpWood();

  // 座面 + 坐垫
  g.add(m(box(W, 0.04, D), wood, [0, seatH, 0], 'both'));
  g.add(m(box(W - 0.06, 0.05, D - 0.06), jpGrayFabric(), [0, seatH + 0.045, 0], 'cast'));
  // 四腿
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.04, seatH, 0.04), jpOakDark(), [sx * (W / 2 - 0.05), seatH / 2, sz * (D / 2 - 0.05)], 'cast'));
  }
  // 靠背（-Z 侧）：两根立柱 + 两条横板
  for (const sx of [-1, 1]) {
    g.add(m(box(0.04, H - seatH, 0.04), jpOakDark(), [sx * (W / 2 - 0.05), seatH + (H - seatH) / 2, -D / 2 + 0.05], 'cast'));
  }
  g.add(m(box(W - 0.06, 0.09, 0.03), wood, [0, H - 0.10, -D / 2 + 0.05], 'cast'));
  g.add(m(box(W - 0.06, 0.06, 0.025), wood, [0, H - 0.26, -D / 2 + 0.05], 'cast'));
  return g;
}

/** 吊灯（障子纸罩，内置暖光）。从天花 2.8m 垂下。装饰件，nav:false。 */
export function buildJpPendant(_spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const ceil = 2.8, shadeTop = 1.92, shadeBot = 1.60;
  // 吊线 + 天花座
  g.add(m(cyl(0.05, 0.05, 0.02, 12), jpOakDark(), [0, ceil - 0.01, 0], 'none'));
  g.add(m(cyl(0.005, 0.005, ceil - shadeTop, 6), jpBlack(), [0, shadeTop + (ceil - shadeTop) / 2, 0], 'none'));
  // 纸罩（鼓形）
  const shade = m(cyl(0.17, 0.15, shadeTop - shadeBot, 20, true), jpShoji(), [0, (shadeTop + shadeBot) / 2, 0], 'cast');
  g.add(shade);
  g.add(m(cyl(0.175, 0.175, 0.014, 20), jpOakDark(), [0, shadeTop, 0], 'cast'));
  g.add(m(cyl(0.155, 0.155, 0.014, 20), jpOakDark(), [0, shadeBot, 0], 'cast'));
  g.add(m(sph(0.045, 10, 8), emissive('#ffe9c8'), [0, (shadeTop + shadeBot) / 2, 0], 'none'));
  const light = new THREE.PointLight(0xffdca8, 0.95, 4.6, 2);
  light.position.set(0, shadeBot - 0.02, 0);
  g.add(light);
  return g;
}

/** 餐区地毯。size = [宽X, 厚Y, 长Z]。装饰件，nav:false。 */
export function buildJpDiningRug(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];
  const top = m(new THREE.PlaneGeometry(W, D), toon('#ffffff', { map: jpRugTexture() }), [0, 0.008, 0], 'receive');
  top.rotation.x = -Math.PI / 2;
  g.add(top);
  return g;
}

/* ---------------- 厨房 ---------------- */

/** 系统厨房台面。size = [进深X, 台高Y, 长Z]，背靠 +X 墙、操作面朝 -X。 */
export function buildJpKitchenCounter(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], L = spec.size[2];
  const cabinet = jpWood();
  const metal = jpMetal();

  // 踢脚 + 地柜 + 台面（朝 -X 出檐）
  g.add(m(box(D - 0.12, 0.09, L - 0.06), jpOakDark(), [0.03, 0.045, 0], 'none'));
  g.add(m(box(D, H - 0.11, L), cabinet, [0, 0.09 + (H - 0.11) / 2, 0], 'both'));
  g.add(m(box(D + 0.03, 0.05, L + 0.02), jpCounter(), [-0.015, H - 0.025, 0], 'both'));

  // 柜门（朝 -X）：三门 + 长条拉手
  const doorN = 3, dw = (L - 0.06) / doorN;
  for (let i = 0; i < doorN; i++) {
    const z = -L / 2 + 0.03 + dw * (i + 0.5);
    g.add(m(box(0.014, H - 0.24, dw - 0.03), jpCream(), [-D / 2 - 0.001, 0.09 + (H - 0.24) / 2 + 0.02, z], 'none'));
    g.add(m(box(0.02, 0.022, dw * 0.5), metal, [-D / 2 - 0.012, H * 0.62, z], 'none'));
  }

  // 水槽（靠北端）+ 龙头
  const sinkZ = -L / 2 + L * 0.22;
  g.add(m(box(D * 0.6, 0.06, 0.5), metal, [-0.01, H - 0.05, sinkZ], 'none'));
  g.add(m(box(D * 0.54, 0.05, 0.44), toon('#8f959c', { finish: 'metal' }), [-0.01, H - 0.055, sinkZ], 'none'));
  g.add(m(cyl(0.018, 0.02, 0.24, 10), metal, [D / 2 - 0.14, H + 0.12, sinkZ], 'cast'));
  const spout = m(cyl(0.014, 0.014, 0.18, 8), metal, [D / 2 - 0.22, H + 0.23, sinkZ], 'cast');
  spout.rotation.z = Math.PI / 2;
  g.add(spout);

  // 灶台（中段）：两个灶眼 + 一口锅
  // 面板顶面必须高出台面：原来中心 H-0.006 时顶面恰与台面顶面(y=H)共面 → z-fighting，
  // 现在中心抬到 H，顶面高出台面 6mm、底面仍嵌进台面里，无任何共面对。
  const stoveZ = L * 0.06;
  g.add(m(box(D * 0.62, 0.012, 0.62), jpBlack(), [-0.01, H, stoveZ], 'none'));
  for (const dz of [-0.15, 0.15]) {
    g.add(m(new THREE.TorusGeometry(0.075, 0.012, 6, 18), jpBlack(), [-0.01, H + 0.004, stoveZ + dz], 'none').rotateX(-Math.PI / 2));
  }
  const pot = m(cyl(0.085, 0.08, 0.1, 16), toon('#6e737a', { finish: 'metal' }), [-0.01, H + 0.05, stoveZ - 0.15], 'cast');
  g.add(pot);
  g.add(m(cyl(0.088, 0.088, 0.012, 16), toon('#565b62', { finish: 'metal' }), [-0.01, H + 0.105, stoveZ - 0.15], 'cast'));

  // 南端：沥水架 + 水壶 + 电饭煲（托盘底面抬离台面 1mm，避免与台面顶面共面闪烁）
  const southZ = L / 2 - L * 0.2;
  g.add(m(box(D * 0.5, 0.02, 0.34), metal, [-0.02, H + 0.011, southZ - 0.05], 'none'));
  for (let i = 0; i < 4; i++) {
    g.add(m(cyl(0.05, 0.045, 0.01, 14), jpCeramic(), [-0.02, H + 0.03, southZ - 0.18 + i * 0.09], 'none'));
  }
  // 水壶
  const kettle = m(cyl(0.07, 0.085, 0.16, 16), toon('#c9ced4', { finish: 'metal' }), [-0.05, H + 0.08, southZ + 0.24], 'cast');
  g.add(kettle);
  g.add(m(cyl(0.012, 0.02, 0.09, 8), toon('#c9ced4', { finish: 'metal' }), [-0.13, H + 0.14, southZ + 0.24], 'cast'));
  // 电饭煲
  g.add(m(box(0.24, 0.18, 0.24), jpCream(), [0.02, H + 0.09, southZ + 0.24], 'cast'));
  g.add(m(cyl(0.09, 0.09, 0.02, 14), jpBlack(), [0.02, H + 0.19, southZ + 0.24], 'none'));
  return g;
}

/** 台面之上的开放吊柜。size = [进深X, 高Y, 长Z]，内部抬到 y≈1.5。装饰件，nav:false。 */
export function buildJpKitchenShelf(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], L = spec.size[2];
  const y0 = 1.50;
  const wood = jpWood();
  // 两层板 + 靠墙背衬
  g.add(m(box(0.016, 0.62, L), jpOakDark(), [D / 2 - 0.008, y0 + 0.31, 0], 'none'));
  for (const yy of [y0, y0 + 0.34]) {
    g.add(m(box(D, 0.026, L), wood, [0, yy, 0], 'both'));
  }
  // 托架
  for (const sz of [-1, 1]) {
    g.add(m(box(D * 0.8, 0.02, 0.03), jpMetal(), [0, y0 - 0.05, sz * (L / 2 - 0.12)], 'none'));
  }
  // 架上物：玻璃罐 / 杯 / 小盆栽 / 一叠盘
  const rng = makeRng(7071);
  const items = 7;
  for (let i = 0; i < items; i++) {
    const z = -L / 2 + 0.16 + (L - 0.32) * (i / (items - 1));
    const layer = i % 2 === 0 ? y0 : y0 + 0.34;
    const kind = Math.floor(rng() * 3);
    if (kind === 0) {
      g.add(m(cyl(0.05, 0.05, 0.13, 12), toon(C('jpCeramicBlue', '#7c99ac'), { finish: 'glass', opacity: 0.8, transparent: true }), [0, layer + 0.078, z], 'cast'));
      g.add(m(cyl(0.035, 0.035, 0.02, 10), jpOakDark(), [0, layer + 0.15, z], 'cast'));
    } else if (kind === 1) {
      g.add(m(cyl(0.04, 0.034, 0.09, 12), jpCeramic(), [0, layer + 0.058, z], 'cast'));
    } else {
      g.add(m(box(0.16, 0.014, 0.16), jpCeramic(), [0, layer + 0.02, z], 'cast'));
      g.add(m(box(0.15, 0.014, 0.15), jpCeramicBlue(), [0, layer + 0.035, z], 'cast'));
    }
  }
  return g;
}

/** 新冰箱。size = [进深X, 高Y, 宽Z]，门朝 -X（背靠 +X 墙）。 */
export function buildJpFridge(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const D = spec.size[0], H = spec.size[1], W = spec.size[2];
  const panel = toon('#ffffff', { map: jpFridgePanelTexture() });
  const metal = jpMetal();

  // 箱体
  g.add(m(box(D, H, W), panel, [0, H / 2, 0], 'both'));
  // 冷冻/冷藏分缝（上门占 1/3）
  const seamY = H * 0.66;
  g.add(m(box(0.012, 0.014, W - 0.04), jpOakDark(), [-D / 2 - 0.001, seamY, 0], 'none'));
  // 门（朝 -X）+ 两条竖拉手
  g.add(m(box(0.024, H - 0.05, W - 0.03), panel, [-D / 2 - 0.002, H / 2, 0], 'both'));
  for (const [yy, hh] of [[seamY + (H - seamY) * 0.5, (H - seamY) * 0.5], [seamY * 0.5, seamY * 0.42]] as Array<[number, number]>) {
    g.add(m(box(0.028, hh, 0.03), metal, [-D / 2 - 0.02, yy, W / 2 - 0.12], 'cast'));
  }
  // 顶上放一只收纳筐
  g.add(m(box(W * 0.5, 0.14, W * 0.6), jpTileWarm(), [0, H + 0.07, 0], 'cast'));
  return g;
}

/* ============================================================================
 * 8. 日式公寓 · 全屋扩展（卧室 / 玄关 / 卫浴 / 阳台 / 灯具 / 杂物 / 墙饰）
 *    与第 7 节同一套 jp* 纹理 + palette，全部新几何，不复用上面任何 buildXxx。
 *    约定：原点 = 地面落脚中心，局部 y=0 = 地板；size 语义逐函数注明。
 * ========================================================================== */

/* --- 第 8 节新增共享材质 ------------------------------------------------- */
const jpFuton = () => toon('#ffffff', { map: jpFutonTexture() });
const jpDeck = () => toon('#ffffff', { map: jpDeckWoodTexture() });
const jpFixture = () => toon(C('jpFixtureWhite', '#eef0ee'), { finish: 'soft' });
const jpBathTile = () => toon('#ffffff', { map: jpBathTileTexture() });
const jpPosterMat = () => toon('#ffffff', { map: jpPosterTexture() });
const jpClockMat = () => toon('#ffffff', { map: jpClockFaceTexture() });

/* ---------------- 卧室 ---------------- */

/**
 * 日式低平台床（布団）。size = [长X(头→脚), 高Y, 宽Z]，床头板在 -X。
 * 橡木矮平台 + 絣织布団 + 枕头 + 靛蓝被褥。
 */
export function buildJpBed(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const L = spec.size[0], H = spec.size[1], W = spec.size[2];
  const wood = jpWood();

  // 矮平台：底部内缩的暗色基座（留阴影缝）+ 橡木台面
  g.add(m(box(L - 0.12, 0.07, W - 0.12), jpOakDark(), [0, 0.035, 0], 'both'));
  g.add(m(box(L, 0.12, W), wood, [0, 0.07 + 0.06, 0], 'both'));
  const deckTop = 0.19;

  // 床头板：贴 -X，略高于床垫
  g.add(m(box(0.05, H + 0.16, W), wood, [-L / 2 + 0.025, (H + 0.16) / 2, 0], 'both'));

  // 布団床垫
  const matH = 0.16;
  g.add(m(box(L - 0.1, matH, W - 0.08), jpFuton(), [0.02, deckTop + matH / 2, 0], 'both'));
  const matTop = deckTop + matH;

  // 枕头 ×2（靠床头 -X）
  for (const sz of [-1, 1]) {
    const pil = m(box(0.3, 0.09, W * 0.32), jpCream(), [-L / 2 + 0.32, matTop + 0.045, sz * W * 0.2], 'cast');
    pil.rotation.z = -0.04;
    g.add(pil);
  }

  // 被褥：盖住脚侧 2/3，靛蓝
  const duvetL = L * 0.58;
  g.add(m(box(duvetL, 0.11, W - 0.06), jpIndigo(), [L / 2 - duvetL / 2 - 0.04, matTop + 0.055, 0], 'both'));
  // 被口翻折：床头侧一道浅色折边
  g.add(m(box(0.14, 0.07, W - 0.08), jpCream(), [L / 2 - duvetL - 0.0, matTop + 0.075, 0], 'cast'));
  // 床尾搭一条折叠毯
  g.add(m(box(0.3, 0.06, W * 0.7), jpIndigoDeep(), [L / 2 - 0.2, matTop + 0.12, 0], 'cast'));
  return g;
}

/** 床头柜。size = [宽X, 高Y, 深Z]，单抽屉 + 台面小物。 */
export function buildJpNightstand(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  // 四条矮腿 + 柜体
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(cyl(0.02, 0.016, 0.08, 8), jpOakDark(), [sx * (W / 2 - 0.06), 0.04, sz * (D / 2 - 0.06)], 'cast'));
  }
  g.add(m(box(W, H - 0.08, D), wood, [0, 0.08 + (H - 0.08) / 2, 0], 'both'));
  // 抽屉面 + 木拉手（朝 +Z）
  g.add(m(box(W - 0.06, H * 0.42, 0.014), jpOakSolid(), [0, 0.08 + (H - 0.08) * 0.5, D / 2 + 0.002], 'none'));
  g.add(m(box(W * 0.3, 0.022, 0.024), jpOakDark(), [0, 0.08 + (H - 0.08) * 0.5, D / 2 + 0.014], 'none'));
  // 台面：一只陶杯 + 一本搁着的书
  g.add(m(cyl(0.035, 0.03, 0.08, 12), jpCeramicBlue(), [W * 0.18, H + 0.04, -D * 0.1], 'cast'));
  g.add(m(box(W * 0.5, 0.025, D * 0.6), jpBook(), [-W * 0.12, H + 0.012, D * 0.05], 'cast'));
  return g;
}

/**
 * 日式推拉门衣柜。size = [宽X, 高Y, 深Z]，门朝 +Z。
 * 橡木框 + 两扇推拉门（一扇障子纸感、一扇木纹）+ 细金属拉手。
 */
export function buildJpWardrobe(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  // 柜体 + 顶线 + 踢脚
  g.add(m(box(W, H, D), wood, [0, H / 2, 0], 'both'));
  g.add(m(box(W + 0.03, 0.05, D + 0.02), jpOakDark(), [0, H - 0.025, 0], 'cast'));
  g.add(m(box(W - 0.04, 0.06, D - 0.04), jpOakDark(), [0, 0.03, 0], 'none'));

  // 两扇推拉门（+Z 面）：左木纹、右障子纸格栅感
  const doorH = H - 0.14, doorY = 0.07 + doorH / 2;
  g.add(m(box(W * 0.5 - 0.015, doorH, 0.022), jpOakSolid(), [-W * 0.25, doorY, D / 2 + 0.004], 'cast'));
  g.add(m(box(W * 0.5 - 0.015, doorH, 0.022), jpShoji(), [W * 0.25, doorY, D / 2 + 0.012], 'cast'));
  // 障子门细格栅
  for (const fx of [-0.16, 0, 0.16]) {
    g.add(m(box(0.012, doorH - 0.04, 0.008), jpOakDark(), [W * 0.25 + fx * W, doorY, D / 2 + 0.026], 'none'));
  }
  // 两条竖向金属拉手
  for (const sx of [-1, 1]) {
    g.add(m(box(0.018, doorH * 0.5, 0.02), jpMetal(), [sx * 0.012, doorY, D / 2 + 0.03], 'none'));
  }
  return g;
}

/**
 * 书桌。size = [宽X, 高Y, 深Z]，背靠 -Z、人坐 +Z 侧。
 * 橡木板 + 两侧板腿 + 单侧抽屉柜 + 台面文具。
 */
export function buildJpDesk(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();
  const topY = H - 0.02;

  // 桌面
  g.add(m(box(W, 0.04, D), wood, [0, topY, 0], 'both'));
  // 两侧板腿
  for (const sx of [-1, 1]) {
    g.add(m(box(0.04, H - 0.04, D - 0.06), wood, [sx * (W / 2 - 0.05), (H - 0.04) / 2, 0], 'both'));
  }
  // 背板挡片（-Z）
  g.add(m(box(W - 0.12, 0.18, 0.02), jpOakSolid(), [0, topY - 0.11, -D / 2 + 0.04], 'none'));
  // 右侧抽屉柜
  const dw = W * 0.3;
  g.add(m(box(dw, H - 0.1, D - 0.1), wood, [W / 2 - dw / 2 - 0.06, (H - 0.1) / 2, 0], 'both'));
  for (const yy of [H * 0.3, H * 0.58]) {
    g.add(m(box(dw - 0.04, 0.012, 0.012), jpOakDark(), [W / 2 - dw / 2 - 0.06, yy, D / 2 - 0.04], 'none'));
    g.add(m(box(0.08, 0.018, 0.018), jpMetal(), [W / 2 - dw / 2 - 0.06, yy + 0.05, D / 2 - 0.03], 'none'));
  }

  // 台面：小台灯 + 马克杯 + 一摞书 + 笔筒
  const lampX = -W / 2 + 0.22;
  g.add(m(cyl(0.06, 0.07, 0.02, 12), jpOakDark(), [lampX, topY + 0.03, -D * 0.18], 'cast'));
  g.add(m(cyl(0.012, 0.012, 0.3, 8), jpMetal(), [lampX, topY + 0.17, -D * 0.18], 'cast'));
  const shade = m(cyl(0.05, 0.09, 0.12, 14, true), jpShoji(), [lampX + 0.06, topY + 0.34, -D * 0.18], 'cast');
  shade.rotation.z = 0.5;
  g.add(shade);
  g.add(m(cyl(0.035, 0.03, 0.09, 12), jpCeramic(), [W * 0.05, topY + 0.045, D * 0.12], 'cast'));
  g.add(m(box(0.22, 0.03, 0.16), jpBook(), [W * 0.12, topY + 0.035, -D * 0.1], 'cast'));
  g.add(m(box(0.2, 0.028, 0.15), jpIndigoDeep(), [W * 0.12, topY + 0.064, -D * 0.1], 'cast'));
  g.add(m(cyl(0.035, 0.03, 0.1, 10), jpCeramicBlue(), [-W * 0.05, topY + 0.05, D * 0.16], 'cast'));
  return g;
}

/** 书桌椅。size = [宽X, 高Y, 深Z]，面朝 +Z（靠背在 -Z），布垫坐面。 */
export function buildJpDeskChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();
  const seatH = 0.42;

  // 四腿
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.04, seatH, 0.04), wood, [sx * (W / 2 - 0.06), seatH / 2, sz * (D / 2 - 0.06)], 'cast'));
  }
  // 坐面框 + 布垫
  g.add(m(box(W, 0.04, D), wood, [0, seatH + 0.02, 0], 'both'));
  g.add(m(box(W - 0.08, 0.06, D - 0.08), jpGrayFabric(), [0, seatH + 0.07, 0], 'cast'));
  // 靠背（-Z）：两根竖杆 + 横向靠背板
  for (const sx of [-1, 1]) {
    g.add(m(box(0.04, H - seatH, 0.04), wood, [sx * (W / 2 - 0.06), seatH + (H - seatH) / 2, -D / 2 + 0.06], 'cast'));
  }
  g.add(m(box(W - 0.08, 0.16, 0.035), jpOakSolid(), [0, H - 0.16, -D / 2 + 0.06], 'cast'));
  g.add(m(box(W - 0.08, 0.1, 0.03), jpGrayFabric(), [0, H - 0.34, -D / 2 + 0.05], 'cast'));
  return g;
}

/** 休闲椅（低矮单座）。size = [宽X, 高Y, 深Z]，面朝 +Z，橡木框 + 靛蓝软垫。 */
export function buildJpLoungeChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();
  const seatH = 0.3;

  // 四腿（前腿短、后腿高延伸成靠背柱）
  for (const sx of [-1, 1]) {
    g.add(m(box(0.05, seatH, 0.05), wood, [sx * (W / 2 - 0.07), seatH / 2, D / 2 - 0.07], 'cast'));
    g.add(m(box(0.05, H, 0.05), wood, [sx * (W / 2 - 0.07), H / 2, -D / 2 + 0.07], 'cast'));
  }
  // 坐面框 + 厚软垫
  g.add(m(box(W, 0.04, D), wood, [0, seatH + 0.02, 0], 'both'));
  g.add(m(box(W - 0.1, 0.12, D - 0.1), jpIndigo(), [0, seatH + 0.1, 0.02], 'cast'));
  // 靠背软垫（倾斜）
  const back = m(box(W - 0.1, 0.4, 0.1), jpIndigo(), [0, seatH + 0.34, -D / 2 + 0.12], 'cast');
  back.rotation.x = -0.16;
  g.add(back);
  // 扶手
  for (const sx of [-1, 1]) {
    g.add(m(box(0.05, 0.04, D - 0.16), wood, [sx * (W / 2 - 0.05), seatH + 0.24, 0.02], 'cast'));
  }
  return g;
}

/** 挂墙镜。size = [厚X, 高Y, 宽Z]，镜面朝 +X（rot=π 则朝 -X）。 */
export function buildJpMirror(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const T = spec.size[0], H = spec.size[1], W = spec.size[2];

  // 橡木框
  g.add(m(box(T, H, W), jpOakSolid(), [0, H / 2 + 0.02, 0], 'cast'));
  // 镜面：朝 +X 的平面，冷灰蓝玻璃感
  const glass = m(new THREE.PlaneGeometry(H - 0.08, W - 0.08), toon('#cdd8de', { finish: 'glass' }), [T / 2 + 0.004, H / 2 + 0.02, 0], 'none');
  glass.rotation.y = Math.PI / 2;
  g.add(glass);
  return g;
}

/* ---------------- 玄关 / 收纳 ---------------- */

/** 玄关鞋柜。size = [宽X, 高Y, 深Z]，门朝 +Z。矮柜 + 台面托盘 + 斜插鞋格暗示。 */
export function buildJpShoeCabinet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  // 柜体 + 踢脚
  g.add(m(box(W - 0.04, 0.05, D - 0.04), jpOakDark(), [0, 0.025, 0], 'none'));
  g.add(m(box(W, H - 0.05, D), wood, [0, 0.05 + (H - 0.05) / 2, 0], 'both'));
  // 两扇门缝 + 长条木拉手（朝 +Z）
  g.add(m(box(0.012, H - 0.14, 0.012), jpOakDark(), [0, 0.05 + (H - 0.05) / 2, D / 2 + 0.001], 'none'));
  for (const sx of [-1, 1]) {
    g.add(m(box(0.02, H * 0.3, 0.022), jpOakDark(), [sx * 0.05, 0.05 + (H - 0.05) * 0.62, D / 2 + 0.012], 'none'));
  }
  // 台面：一只钥匙陶碟 + 一个小插枝陶瓶
  g.add(m(cyl(0.06, 0.05, 0.025, 14), jpCeramic(), [-W * 0.22, H + 0.012, 0], 'cast'));
  g.add(m(cyl(0.04, 0.05, 0.14, 12), jpCeramicBlue(), [W * 0.2, H + 0.07, 0], 'cast'));
  g.add(m(cyl(0.004, 0.004, 0.16, 5), jpPlantGreen(), [W * 0.2, H + 0.2, 0.01], 'cast'));
  return g;
}

/** 换鞋凳。size = [宽X, 高Y, 深Z]，橡木矮凳 + 编织坐面。 */
export function buildJpBench(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();

  // 四腿 + 两根横撑
  for (const sx of [-1, 1]) for (const sz of [-1, 1]) {
    g.add(m(box(0.05, H - 0.05, 0.05), wood, [sx * (W / 2 - 0.07), (H - 0.05) / 2, sz * (D / 2 - 0.07)], 'cast'));
  }
  for (const sx of [-1, 1]) {
    g.add(m(box(0.03, 0.03, D - 0.16), jpOakDark(), [sx * (W / 2 - 0.07), H * 0.35, 0], 'none'));
  }
  // 坐面：木板 + 一层靛蓝织垫
  g.add(m(box(W, 0.05, D), wood, [0, H - 0.025, 0], 'both'));
  g.add(m(box(W - 0.1, 0.04, D - 0.1), jpGrayFabric(), [0, H + 0.02, 0], 'cast'));
  return g;
}

/** 收纳开放架（収納间）。size = [宽X, 高Y, 深Z]，朝 +Z，多层 + 收纳箱/篮。 */
export function buildJpClosetShelf(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();
  const t = 0.03;

  // 两侧板 + 背板
  for (const sx of [-1, 1]) g.add(m(box(t, H, D), wood, [sx * (W / 2 - t / 2), H / 2, 0], 'both'));
  g.add(m(box(W, H, 0.016), jpOakSolid(), [0, H / 2, -D / 2 + 0.008], 'receive'));
  // 隔板（4 层）
  const levels = [0.04, H * 0.28, H * 0.52, H * 0.76, H - t];
  for (const y of levels) g.add(m(box(W - t * 2, t, D), wood, [0, y, 0], 'both'));

  // 每层塞收纳箱 / 藤篮 / 叠被
  const rng = makeRng(5150);
  const innerW = W - t * 2 - 0.04;
  for (let i = 0; i < levels.length - 1; i++) {
    const yBot = levels[i] + t / 2;
    const compH = levels[i + 1] - levels[i] - t;
    if (compH < 0.12) continue;
    const nBoxes = 1 + Math.floor(rng() * 2);
    let zCursor = -innerW / 2;
    for (let b = 0; b < nBoxes; b++) {
      const bw = (innerW / nBoxes) * (0.7 + rng() * 0.25);
      const bh = compH * (0.7 + rng() * 0.22);
      const mat = rng() > 0.5 ? jpTileWarm() : jpGrayFabric();
      g.add(m(box(D - 0.08, bh, bw), mat, [0.02, yBot + bh / 2, zCursor + bw / 2], 'cast'));
      zCursor += bw + 0.03;
      if (zCursor > innerW / 2 - 0.1) break;
    }
  }
  return g;
}

/* ================= 卫浴（トイレ / ユニットバス / 洗面所 / 洗濯） ================= */

/**
 * 独立トイレ。size = [宽X, 高Y, 深Z]，水箱贴 -Z 墙、便器朝 +Z。
 * 白瓷便器 + 水箱上手洗 + 侧墙遥控面板 + 角落毛巾环。
 */
export function buildJpToilet(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];
  const white = jpFixture();
  const metal = jpMetal();

  // 便器主体（贴 -Z）：底座 + 便 bowl + 座圈 + 盖
  const tw = Math.min(0.40, W * 0.8);
  const tz = -D / 2 + 0.30;
  g.add(m(box(tw, 0.20, 0.48), white, [0, 0.10, tz], 'both'));
  const bowl = m(cyl(0.18, 0.15, 0.15, 20), white, [0, 0.24, tz + 0.05], 'cast');
  bowl.scale.set(1, 1, 1.22);
  g.add(bowl);
  g.add(m(cyl(0.17, 0.17, 0.014, 20), toon('#e7eae7'), [0, 0.32, tz + 0.05], 'none')); // 座圈
  g.add(m(cyl(0.165, 0.165, 0.012, 20), white, [0, 0.335, tz + 0.05], 'cast')); // 盖
  // 水箱（贴 -Z 墙）+ 顶部手洗盆 + 龙头
  g.add(m(box(tw, 0.40, 0.18), white, [0, 0.40, -D / 2 + 0.10], 'both'));
  g.add(m(box(tw - 0.06, 0.05, 0.14), toon('#e7eae7'), [0, 0.62, -D / 2 + 0.10], 'receive'));
  const tf = m(cyl(0.011, 0.011, 0.10, 8), metal, [0, 0.66, -D / 2 + 0.10], 'cast');
  g.add(tf);
  const tsp = m(cyl(0.009, 0.009, 0.07, 8), metal, [0, 0.70, -D / 2 + 0.14], 'cast');
  tsp.rotation.x = Math.PI / 2;
  g.add(tsp);
  // 侧墙遥控面板（+X 侧）
  g.add(m(box(0.02, 0.10, 0.18), toon('#dfe3e0'), [W / 2 - 0.05, 0.72, tz - 0.02], 'cast'));
  g.add(m(box(0.006, 0.02, 0.02), jpBlack(), [W / 2 - 0.038, 0.75, tz - 0.02], 'none'));
  // 卷纸器（-X 侧）
  const holder = m(cyl(0.012, 0.012, 0.10, 8), metal, [-W / 2 + 0.10, 0.68, tz + 0.02], 'cast');
  holder.rotation.z = Math.PI / 2;
  g.add(holder);
  const roll = m(cyl(0.055, 0.055, 0.09, 16), toon('#f2efe8'), [-W / 2 + 0.10, 0.68, tz + 0.02], 'cast');
  roll.rotation.z = Math.PI / 2;
  g.add(roll);
  return g;
}

/**
 * ユニットバス（整体浴室）。size = [宽X, 高Y, 深Z]，H 可为 0（用 2.2 兜底）。
 * 沿 -X 墙深浴缸 + 前区洗浴站（矮凳/水桶/花洒/小镜）。房间墙体由 shell 提供，这里只摆洁具。
 */
export function buildJpBath(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], D = spec.size[2];
  const H = spec.size[1] > 0.1 ? spec.size[1] : 2.2;
  const white = jpFixture();
  const tile = jpBathTile();
  const metal = jpMetal();
  const xL = -W / 2, zB = -D / 2;

  // 浴缸：沿 -X 墙，长边贴墙，缸长自适应房间进深
  const tubW = 0.78;
  const tubLen = Math.min(1.75, Math.max(1.0, D - 0.7));
  const tubX = xL + 0.05 + tubW / 2;
  g.add(m(box(tubW, 0.56, tubLen), white, [tubX, 0.28, 0], 'both'));
  g.add(m(box(tubW - 0.14, 0.03, tubLen - 0.14), toon('#cdd8d6'), [tubX + 0.02, 0.545, 0], 'none')); // 水面
  g.add(m(box(tubW - 0.10, 0.40, tubLen - 0.10), tile, [tubX + 0.02, 0.34, 0], 'none')); // 内膛
  // 缸沿龙头
  const btf = m(cyl(0.014, 0.014, 0.24, 8), metal, [tubX - tubW / 2 + 0.12, 0.66, -tubLen / 2 + 0.16], 'cast');
  g.add(btf);
  const btsp = m(cyl(0.011, 0.011, 0.13, 8), metal, [tubX - tubW / 2 + 0.19, 0.76, -tubLen / 2 + 0.16], 'cast');
  btsp.rotation.z = Math.PI / 2;
  g.add(btsp);

  // 洗浴站（+X 侧）：矮凳 + 水桶 + 墙面花洒 + 小镜
  const stX = xL + tubW + 0.10 + (W - tubW - 0.20) / 2;
  // 矮凳
  const stool = new THREE.Group();
  stool.add(m(box(0.32, 0.04, 0.24), white, [0, 0.22, 0], 'both'));
  for (const sx of [-1, 1]) for (const sz of [-1, 1])
    stool.add(m(box(0.03, 0.20, 0.03), white, [sx * 0.13, 0.10, sz * 0.09], 'cast'));
  stool.position.set(stX, 0, 0.35);
  g.add(stool);
  // 水桶
  const bucket = m(cyl(0.11, 0.09, 0.20, 18), toon('#cdd5cf'), [stX + 0.02, 0.10, 0.75], 'both');
  g.add(bucket);
  // 墙面花洒（贴 +X 墙，柱 + 喷头 + 混水阀）
  const shX = W / 2 - 0.10;
  g.add(m(box(0.06, 0.9, 0.06), metal, [shX, 1.25, -0.2], 'cast'));
  const head = m(cyl(0.075, 0.05, 0.05, 18), metal, [shX - 0.02, 1.68, -0.2], 'cast');
  head.rotation.z = Math.PI / 2.4;
  g.add(head);
  g.add(m(box(0.08, 0.16, 0.06), white, [shX, 1.05, -0.2], 'cast')); // 混水阀
  // 小镜（贴 -Z 墙，洗浴站上方）
  g.add(m(box(0.40, 0.50, 0.03), jpOakSolid(), [stX, 1.35, zB + 0.06], 'cast'));
  g.add(m(new THREE.PlaneGeometry(0.34, 0.44), toon('#cdd8de', { finish: 'glass' }), [stX, 1.35, zB + 0.078], 'none'));
  // 地漏（前区中央）
  g.add(m(cyl(0.07, 0.07, 0.012, 16), metal, [stX, 0.006, -0.15], 'none'));
  // 一条挂在缸沿的毛巾
  g.add(m(box(0.30, 0.02, 0.30), jpGrayFabric(), [tubX + tubW / 2 - 0.02, 0.575, tubLen / 2 - 0.2], 'cast'));
  return g;
}

/**
 * 洗面台（洗面所）。size = [宽X, 高Y, 深Z]，背面贴 -Z 墙、正面朝 +Z。
 * 橡木柜 + 白瓷台上盆 + 龙头 + 上方镜柜 + 毛巾/漱口杯。
 */
export function buildJpVanity(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const wood = jpWood();
  const white = jpFixture();
  const metal = jpMetal();
  const zBack = -D / 2;

  // 柜体（悬空，底部离地 0.12）+ 两扇对开门
  g.add(m(box(W, H - 0.12, D), wood, [0, 0.12 + (H - 0.12) / 2, 0], 'both'));
  for (const sx of [-1, 1])
    g.add(m(box(W / 2 - 0.01, H - 0.24, 0.018), jpOakSolid(), [sx * (W / 4), 0.12 + (H - 0.12) / 2, D / 2 + 0.002], 'cast'));
  g.add(m(box(0.02, 0.10, 0.02), metal, [-0.04, H * 0.55, D / 2 + 0.02], 'cast'));
  g.add(m(box(0.02, 0.10, 0.02), metal, [0.04, H * 0.55, D / 2 + 0.02], 'cast'));
  // 台面
  g.add(m(box(W + 0.03, 0.045, D + 0.03), white, [0, H + 0.022, 0], 'both'));
  // 台上盆（椭圆）+ 内凹
  const basin = m(cyl(0.20, 0.17, 0.12, 24), white, [0, H + 0.10, 0.02], 'both');
  basin.scale.set(1.15, 1, 0.8);
  g.add(basin);
  g.add(m(cyl(0.165, 0.155, 0.02, 22), toon('#dfe3e0'), [0, H + 0.155, 0.02], 'receive'));
  // 龙头（贴台面后缘）
  const vf = m(cyl(0.012, 0.012, 0.20, 8), metal, [0, H + 0.14, zBack + 0.14], 'cast');
  g.add(vf);
  const vsp = m(cyl(0.010, 0.010, 0.12, 8), metal, [0, H + 0.23, zBack + 0.20], 'cast');
  vsp.rotation.x = Math.PI / 2;
  g.add(vsp);
  // 镜柜（台面上方，贴 -Z 墙）
  g.add(m(box(Math.min(W, 0.75), 0.60, 0.06), jpOakSolid(), [0, H + 0.62, zBack + 0.05], 'both'));
  g.add(m(new THREE.PlaneGeometry(Math.min(W, 0.75) - 0.10, 0.50), toon('#cdd8de', { finish: 'glass' }), [0, H + 0.62, zBack + 0.082], 'none'));
  // 漱口杯 + 牙刷 + 一条毛巾
  g.add(m(cyl(0.035, 0.032, 0.09, 14), jpCeramicBlue(), [W / 2 - 0.14, H + 0.09, 0.02], 'cast'));
  g.add(m(box(0.012, 0.12, 0.012), jpCeramic(), [W / 2 - 0.14, H + 0.14, 0.02], 'cast'));
  g.add(m(box(0.26, 0.02, D * 0.5), toon('#f2efe8'), [-W / 2 + 0.16, H + 0.055, 0], 'cast'));
  return g;
}

/**
 * 洗濯机（洗面所）。size = [宽X, 高Y, 深Z]，正面朝 +Z。
 * 白色机身 + 前开门玻璃视窗 + 顶部控制条 + 洗涤剂抽屉。
 */
export function buildJpWasher(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const body = jpFixture();
  const metal = jpMetal();
  const zF = D / 2;

  g.add(m(box(W, H, D), body, [0, H / 2, 0], 'both'));
  // 顶部控制条
  g.add(m(box(W - 0.03, 0.07, D - 0.04), toon('#dfe3e0'), [0, H - 0.05, 0], 'both'));
  // 旋钮 + 两枚按键（正面 +Z 顶部）
  const knob = m(cyl(0.030, 0.030, 0.02, 16), metal, [W / 2 - 0.10, H - 0.045, zF + 0.005], 'cast');
  knob.rotation.x = Math.PI / 2;
  g.add(knob);
  for (let i = 0; i < 2; i++)
    g.add(m(box(0.03, 0.012, 0.012), jpBlack(), [-W / 2 + 0.10 + i * 0.07, H - 0.045, zF + 0.004], 'none'));
  // 前开门：金属圈 + 玻璃视窗
  const ring = m(cyl(0.19, 0.19, 0.03, 28), toon('#c3c8cc'), [0, H * 0.48, zF + 0.006], 'cast');
  ring.rotation.x = Math.PI / 2;
  g.add(ring);
  const glass = m(cyl(0.145, 0.145, 0.026, 26), toon('#8e9aa0', { finish: 'glass' }), [0, H * 0.48, zF + 0.02], 'cast');
  glass.rotation.x = Math.PI / 2;
  g.add(glass);
  // 洗涤剂抽屉（正面下部）
  g.add(m(box(W * 0.5, 0.07, 0.02), toon('#dfe3e0'), [0, H * 0.16, zF + 0.004], 'cast'));
  // 底座踢脚
  g.add(m(box(W - 0.02, 0.05, D - 0.02), toon('#d6dad7'), [0, 0.025, 0], 'cast'));
  return g;
}

/* ================= 阳台（ベランダ） ================= */

/** 阳台折叠木桌。size = [宽X, 高Y, 深Z]，防腐木条面 + 折叠腿。 */
export function buildJpBalconyTable(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const deck = jpDeck();
  const metal = jpMetal();
  // 桌面：3 条防腐木板
  const slats = 3;
  for (let i = 0; i < slats; i++) {
    const z = -D / 2 + (D / slats) * (i + 0.5);
    g.add(m(box(W, 0.028, D / slats - 0.012), deck, [0, H - 0.014, z], 'both'));
  }
  // 折叠 X 腿
  for (const sx of [-1, 1]) {
    for (const s of [-1, 1]) {
      const leg = m(box(0.03, H, 0.03), metal, [sx * (W / 2 - 0.06), H / 2, s * 0.06], 'cast');
      leg.rotation.x = s * 0.16;
      g.add(leg);
    }
    g.add(m(box(0.03, 0.03, D - 0.10), metal, [sx * (W / 2 - 0.06), H * 0.42, 0], 'cast'));
  }
  // 台面一只陶杯
  g.add(m(cyl(0.035, 0.032, 0.08, 14), jpCeramicBlue(), [W * 0.18, H + 0.04, 0], 'cast'));
  return g;
}

/** 阳台折叠木椅。size = [宽X, 高Y, 深Z]，座面朝 +Z，防腐木 + 靛蓝坐垫。 */
export function buildJpBalconyChair(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const deck = jpDeck();
  const seatH = Math.min(0.42, H * 0.5);
  // 座板（2 条）
  for (let i = 0; i < 2; i++)
    g.add(m(box(W, 0.026, D / 2 - 0.02), deck, [0, seatH, -D / 4 + i * (D / 2)], 'both'));
  // 靛蓝坐垫
  g.add(m(box(W - 0.06, 0.05, D - 0.06), jpIndigo(), [0, seatH + 0.038, 0], 'cast'));
  // 靠背（贴 -Z，微微后倾）
  const back = m(box(W, H - seatH, 0.026), deck, [0, seatH + (H - seatH) / 2, -D / 2 + 0.03], 'both');
  back.rotation.x = -0.12;
  g.add(back);
  // 四腿
  for (const sx of [-1, 1]) for (const sz of [-1, 1])
    g.add(m(box(0.032, seatH, 0.032), deck, [sx * (W / 2 - 0.05), seatH / 2, sz * (D / 2 - 0.05)], 'cast'));
  return g;
}

/** 阳台花箱（プランター）。size = [宽X, 高Y, 深Z]，防腐木箱 + 土 + 绿植/小花。装饰件 nav:false。 */
export function buildJpPlanter(spec: { pos: [number, number, number]; size: [number, number, number] }): THREE.Group {
  const g = new THREE.Group();
  const W = spec.size[0], H = spec.size[1], D = spec.size[2];
  const deck = jpDeck();
  // 木箱（四壁 + 底）
  for (const sx of [-1, 1]) g.add(m(box(0.03, H, D), deck, [sx * (W / 2 - 0.015), H / 2, 0], 'both'));
  for (const sz of [-1, 1]) g.add(m(box(W, H, 0.03), deck, [0, H / 2, sz * (D / 2 - 0.015)], 'both'));
  g.add(m(box(W - 0.04, 0.02, D - 0.04), deck, [0, 0.01, 0], 'receive'));
  // 土
  g.add(m(box(W - 0.06, 0.04, D - 0.06), toon('#5d4c3c'), [0, H - 0.04, 0], 'receive'));
  // 绿植丛 + 几点小花
  const rng = makeRng(7781);
  const n = 7;
  for (let i = 0; i < n; i++) {
    const t = i / (n - 1);
    const x = -W / 2 + 0.10 + (W - 0.20) * t;
    const z = -D / 2 + 0.08 + rng() * (D - 0.16);
    const hgt = 0.16 + rng() * 0.16;
    const bush = m(sph(0.07 + rng() * 0.04, 10, 8), rng() > 0.4 ? jpPlantGreen() : jpPlantGreenDark(), [x, H + hgt * 0.5, z], 'cast');
    bush.scale.set(1, hgt / 0.14, 1);
    g.add(bush);
    if (rng() > 0.55)
      g.add(m(sph(0.022, 8, 6), rng() > 0.5 ? jpTerra() : jpCream(), [x + (rng() - 0.5) * 0.05, H + hgt, z + (rng() - 0.5) * 0.05], 'cast'));
  }
  return g;
}

/* ================= 灯具 / 生活杂物 / 墙饰 ================= */

/** 和风障子纸吸顶灯。与 buildCeilLamp 同签名（{pos, ceilingH}），内置暖光 PointLight。 */
export function buildJpCeilLamp(spec: { pos: [number, number, number]; ceilingH: number }): THREE.Group {
  const g = new THREE.Group();
  const y = spec.ceilingH;
  const oak = jpOakSolid();
  const paper = toon('#fff6e6', { emissive: '#e6d3ac', emissiveIntensity: 0.7, side: THREE.DoubleSide });
  // 底座 + 方形障子纸罩 + 木格栅
  g.add(m(cyl(0.07, 0.09, 0.03, 14), oak, [0, y - 0.015, 0], 'cast'));
  g.add(m(box(0.42, 0.10, 0.42), paper, [0, y - 0.10, 0], 'cast'));
  for (const s of [-1, 1]) {
    g.add(m(box(0.44, 0.02, 0.02), oak, [0, y - 0.055, s * 0.20], 'cast'));
    g.add(m(box(0.02, 0.02, 0.44), oak, [s * 0.20, y - 0.055, 0], 'cast'));
    g.add(m(box(0.44, 0.02, 0.02), oak, [0, y - 0.145, s * 0.20], 'cast'));
    g.add(m(box(0.02, 0.02, 0.44), oak, [s * 0.20, y - 0.145, 0], 'cast'));
  }
  g.add(m(box(0.02, 0.10, 0.44), oak, [0, y - 0.10, 0], 'none')); // 中骨
  g.add(m(box(0.44, 0.10, 0.02), oak, [0, y - 0.10, 0], 'none'));
  const light = new THREE.PointLight(0xffe6c0, 0.46, 4.0, 2);
  light.position.set(0, y - 0.20, 0);
  g.add(light);
  g.position.set(spec.pos[0], 0, spec.pos[2]);
  return g;
}

/**
 * 日式生活杂物：玄关拖鞋/折伞、風呂敷包、纸箱、垃圾桶、墙面明信片。
 * 与 buildLifestyleDetails 同一 LifestyleConfig 结构，由 dormLayout.json 的 lifestyle 条目驱动。
 */
export function buildJpLifestyleDetails(cfg?: LifestyleConfig): THREE.Group {
  const g = new THREE.Group();
  const D = { ...LIFESTYLE_DEFAULTS, ...(cfg ?? {}) };
  const deck = jpDeck();
  const indigo = jpIndigo();
  const cream = jpCream();
  const black = jpBlack();
  const paper = toon('#f4efe4');
  const slipColors = ['#c8b6a0', '#7c99ac'];

  // 玄关拖鞋（室内スリッパ）：成双、微随意
  for (const [i, [x, z, rot]] of D.slippers.entries()) {
    const slipper = m(box(0.12, 0.04, 0.29), toon(slipColors[i % slipColors.length]), [x, 0.02, z], 'cast');
    slipper.rotation.y = rot;
    g.add(slipper);
    const strap = m(new THREE.TorusGeometry(0.048, 0.011, 6, 12, Math.PI), cream, [x, 0.058, z - 0.03], 'cast');
    strap.rotation.x = Math.PI / 2;
    strap.rotation.z = rot;
    g.add(strap);
  }

  // 玄关折伞：靛蓝伞面 + 木柄
  const umbrella = new THREE.Group();
  umbrella.add(m(cyl(0.010, 0.010, 0.76, 8), jpOakDark(), [0, 0.38, 0], 'cast'));
  const canopy = m(new THREE.ConeGeometry(0.10, 0.40, 12, 1, true), indigo, [0, 0.33, 0], 'cast');
  canopy.rotation.x = Math.PI;
  umbrella.add(canopy);
  const handle = m(new THREE.TorusGeometry(0.048, 0.010, 6, 12, Math.PI * 1.25), jpOakDark(), [0, 0.76, 0.05], 'cast');
  handle.rotation.y = Math.PI / 2;
  umbrella.add(handle);
  umbrella.position.set(D.umbrella[0], 0, D.umbrella[1]);
  umbrella.rotation.z = 0.10;
  g.add(umbrella);

  // 風呂敷包（帆布包位置）：靛蓝布包 + 打结
  const bag = new THREE.Group();
  bag.add(m(box(0.32, 0.30, 0.14), indigo, [0, 0.15, 0], 'cast'));
  bag.add(m(box(0.34, 0.05, 0.16), cream, [0, 0.30, 0], 'cast'));
  const knot = m(sph(0.045, 10, 8), indigo, [0, 0.35, 0], 'cast');
  knot.scale.set(1, 0.7, 1);
  bag.add(knot);
  bag.position.set(D.bag[0], 0, D.bag[1]);
  bag.rotation.y = D.bag[2];
  g.add(bag);

  // 纸箱（瓦楞 + 胶带）
  const parcelA = m(box(0.34, 0.18, 0.28), toon('#c2a882'), [D.parcels[0], 0.09, D.parcels[1]], 'cast');
  parcelA.rotation.y = 0.08;
  g.add(parcelA);
  g.add(m(box(0.34, 0.02, 0.05), cream, [D.parcels[0], 0.185, D.parcels[1]], 'none').rotateY(0.08));
  const parcelB = m(box(0.25, 0.14, 0.23), toon('#c2a882'), [D.parcels[0] - 0.06, 0.25, D.parcels[1] - 0.02], 'cast');
  parcelB.rotation.y = -0.10;
  g.add(parcelB);

  // 垃圾桶（木条 + 内衬）+ 一团废纸
  const bin = m(cyl(0.12, 0.095, 0.28, 14, true), deck, [D.bin[0], 0.14, D.bin[1]], 'cast');
  g.add(bin);
  g.add(m(cyl(0.115, 0.09, 0.02, 14), black, [D.bin[0], 0.275, D.bin[1]], 'none'));
  g.add(m(sph(0.033, 7, 6), paper, [D.bin[0] - 0.16, 0.033, D.bin[1] + 0.17], 'cast'));

  // 墙面明信片/便签（单面朝内平面，贴墙不投影）
  for (const [x, y, z, ry, col] of D.cards) {
    const card = m(new THREE.PlaneGeometry(0.22, 0.16), toon(col), [x, y, z], 'none');
    card.userData.noOutline = true;
    card.rotation.y = ry;
    g.add(card);
  }
  return g;
}

/** 和风挂钟：用 jpClockFace 表盘纹理 + 橡木圈 + 指针。签名同 buildWallClock。 */
export function buildJpWallClock(pos: [number, number, number], rot = 0): THREE.Group {
  const g = new THREE.Group();
  const oak = jpOakSolid();
  // 表体 + 橡木圈
  const body = m(cyl(0.16, 0.16, 0.04, 26), toon('#f3eee4'), [0, 0, 0], 'cast');
  body.rotation.x = Math.PI / 2;
  g.add(body);
  g.add(m(new THREE.TorusGeometry(0.158, 0.016, 8, 30), oak, [0, 0, 0.02], 'cast'));
  // 表盘（纹理已含刻度）
  g.add(m(new THREE.CircleGeometry(0.145, 28), jpClockMat(), [0, 0, 0.023], 'none'));
  // 指针
  const hourGeo = box(0.012, 0.078, 0.004).translate(0, 0.030, 0);
  const hour = m(hourGeo, jpBlack(), [0, 0, 0.027], 'none');
  hour.rotation.z = -2.2;
  g.add(hour);
  const minGeo = box(0.008, 0.116, 0.004).translate(0, 0.048, 0);
  const min = m(minGeo, jpBlack(), [0, 0, 0.029], 'none');
  min.rotation.z = 0.6;
  g.add(min);
  g.add(m(cyl(0.011, 0.011, 0.012, 10), jpTerra(), [0, 0, 0.031], 'none').rotateX(Math.PI / 2));
  g.position.set(pos[0], pos[1], pos[2]);
  g.rotation.y = rot;
  return g;
}

/** 《你的名字》彗星夜空海报：橡木框 + jpPoster 画面。签名同 buildPoster。 */
export function buildJpPoster(
  pos: [number, number, number],
  size: [number, number],
  rot = 0
): THREE.Group {
  const g = new THREE.Group();
  g.add(m(box(size[0] + 0.05, size[1] + 0.05, 0.02), jpOakSolid(), [0, 0, 0], 'cast'));
  g.add(m(new THREE.PlaneGeometry(size[0], size[1]), jpPosterMat(), [0, 0, 0.015], 'none'));
  g.position.set(pos[0], pos[1], pos[2]);
  g.rotation.y = rot;
  return g;
}
