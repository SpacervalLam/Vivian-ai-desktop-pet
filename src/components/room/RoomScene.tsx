import { createCharacterAnimation } from './characterAnimation';
import { createApartmentLift } from './anime/apartmentLift';
import { createInteriorDesign } from './anime/interiorDesign';
import { createDistrictArt } from './anime/districtArt';
/**
 * 房间 3D 场景——日式公寓：Blender PBR 家具 + 赛璐璐建筑与角色。
 *
 * 视觉管线：
 *   程序化贴图 → MeshToonMaterial（4 级色阶）→ 反面外扩描边 → 三点布光 + 软阴影
 *   → 窗口光束 / 浮尘 / 自发光小物件（屏幕、灯泡、串灯）
 *   → 场景雾 → 后处理链（线性 HDR 泛光 → 色调映射 → sRGB）
 *
 * 雾和泛光的参数都在 dormLayout.json 的 postfx 里，改 JSON 就能调，不用重建。
 * 按 P 可以整段关掉雾 + 泛光 + 色调映射做 A/B 对照。
 *
 * 程序化道具走描边；Blender 家具保留 PBR / 顶点 AO，角色不描边。
 * 窗口隐藏时整条管线暂停。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { bootMark, logBootTimeline, dismissBootLoader } from '../../utils/roomBoot';
import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { EffectComposer } from 'three/examples/jsm/postprocessing/EffectComposer.js';
import { RenderPass } from 'three/examples/jsm/postprocessing/RenderPass.js';
import { UnrealBloomPass } from 'three/examples/jsm/postprocessing/UnrealBloomPass.js';
import { OutputPass } from 'three/examples/jsm/postprocessing/OutputPass.js';
import { FPSControls, Collider, FPS_NEAR_MIN, FPS_NEAR_MAX } from './anime/fpsControls';
import { buildFurnitureColliders, buildWallColliders, buildSceneColliders, buildBoxColliders, pickOverheadSlabs } from './anime/collider';
import { PetAgent } from './agents/usePetAgent';
import { buildNavWorld } from './agents/navWorld';
import { buildObstacles } from './agents/navGrid';
import layoutData from './dormLayout.json';
import { blenderFurnitureIds, loadBlenderFurniture, type FurnitureSlot } from './blenderFurniture';

import {
  describeEnvironment, environmentFromLocalClock, resolveEnvironment,
  PERIOD_LABELS, WEATHER_LABELS,
  type DayPeriod, type EnvironmentInput, type WeatherKind,
} from './worldEnvironment';
import type { WorldSnapshotResponse } from '../../types';
import { setOutlineDistanceScale, setToonKeyLight, toonGradient, makeRng } from './anime/toon';
import { mergeByMaterial, freezeStatic, dedupeGeometries } from './anime/merge';
import { buildExteriorGround, buildStreetscape, buildApartmentShell, buildConvenienceStore, buildStreetscapeRipples, buildApartmentEaveDrips, buildSubwayEntrance, buildIzakaya, buildSmallPark, buildUtilityPoles, buildStreetFurniture, buildStreetBookStore, STRS, FLOORS, APT_X0, APT_X1, APT_ZN, APT_ZB, APT_WALL_BOXES, APT_CORRIDOR_N } from './anime/exterior';
import {
  setArtStyle,
  outlineProp,
  buildRoomShell,
  buildWindow,
  buildRain,
  buildWetGround,
  buildDesk,
  buildChair,
  buildBed,
  buildShelf,
  buildFridge,
  buildRug,
  buildLowTable,
  buildCushion,
  buildLifestyleDetails,
  type LifestyleConfig,
  type RainConfig,
  type PoolSpec,
  buildCeilingLantern,
  buildStringLights,
  buildSunbeam,
  type BeamConfig,
  buildDustMotes,
  buildSofa,
  buildTV,
  buildFloorLamp,
  buildKitchen,
  buildDining,
  buildBathroom,
  buildScroll,
  buildGameStation,
  buildEntrySet,
  buildDoor,
  buildEntryDoor,
  buildGlassDoor,
  buildFusuma,
  buildBalconyProps,
  buildUpholsteredBed,
  buildMetalBed,
  buildWardrobe,
  buildDisplayCabinet,
  buildWineCabinet,
  buildIslandKitchen,
  buildGamingDesk,
  buildMarbleTable,
  buildCurvedSofa,
  // 专用件：替换此前用 shelf/fridge/cushionItem 顶替的镜子、鞋柜、洗衣机等
  buildMirror,
  buildShoeCabinet,
  buildBench,
  buildWasher,
  buildSideboard,
  buildVanity,
  buildSideTable,
  buildStool,
  buildLoungeChair,
  buildBookcase,
  buildToilet,
  buildPlant,
  buildJpBookRack,
  buildJpTvBoard,
  buildJpRug,
  buildJpLowSofa,
  buildJpCenterTable,
  buildJpZabuton,
  buildJpFloorLamp,
  buildJpPlant,
  buildJpFloorClutter,
  buildJpSlatWall,
  buildJpDiningTable,
  buildJpDiningChair,
  buildJpPendant,
  buildJpDiningRug,
  buildJpKitchenCounter,
  buildJpKitchenShelf,
  buildJpFridge,
  // 日式公寓 · 全屋扩展：卧室 / 玄关收纳 / 卫浴 / 阳台 / 灯具 / 杂物 / 墙饰
  buildJpBed,
  buildJpNightstand,
  buildJpWardrobe,
  buildJpDesk,
  buildJpDeskChair,
  buildJpLoungeChair,
  buildJpMirror,
  buildJpShoeCabinet,
  buildJpBench,
  buildJpClosetShelf,
  buildJpToilet,
  buildJpBath,
  buildJpVanity,
  buildJpWasher,
  buildJpBalconyTable,
  buildJpBalconyChair,
  buildJpPlanter,
  buildJpCeilLamp,
  buildJpLifestyleDetails,
  buildJpWallClock,
  buildJpPoster,
} from './anime/props';

const TICK_DT = 0.05; // 20Hz 逻辑

/**
 * 第一人称上次退出的位置 + 朝向（模块级，跨组件重挂存活）。
 * 进入第一人称时直接加载，实现"在地图内自由活动后，下次进来还在老地方、面朝老方向"。
 */
type SavedFps = { x: number; y: number; z: number; yaw: number; pitch: number };
let savedFpsState: SavedFps | null = null;


type FurnitureSpec = {
  id: string;
  kind: string;
  nav?: boolean;
  /** 绕 Y 的朝向弧度。不写 = 0，即默认"背靠 -Z 墙、面朝 +Z"。 */
  rot?: number;
  /** 墙侧 id：厚装饰挂进对应 decor 组，相机绕到墙外侧时跟着墙一起让开。 */
  wall?: string;
  /** 个别家具的附加色（座布団等）。 */
  color?: string;
  /** 款式：同一 kind 下的形态分支（如 buildDoor 的 variant）。 */
  variant?: string;
  pos: [number, number, number];
  size: [number, number, number];
};

/**
 * 保留逐件 mesh、不参与合批的散点摆件 kind（见装配处注释）：
 * lifestyle 一件道具的零件散落房间各处，合批会把跨房间的同材质件熔成一个
 * mesh，导致遍历补碰撞只能按整体 AABB 收盒（隐形碰撞墙）。
 */
const PIECE_KEEP_RAW = new Set(['lifestyle', 'jpLifestyle']);

const FURNITURE_BUILDERS: Record<string, (spec: FurnitureSpec) => THREE.Object3D> = {
  window: buildWindow,
  desk: buildDesk,
  chair: buildChair,
  bed: buildBed,
  shelf: buildShelf,
  fridge: buildFridge,
  rug: buildRug,
  table: buildLowTable,
  cushionItem: (spec) => buildCushion(spec.pos, spec.color),
  sofa: buildSofa,
  tv: buildTV,
  floorLamp: buildFloorLamp,
  kitchen: buildKitchen,
  dining: buildDining,
  bath: buildBathroom,
  scroll: buildScroll,
  game: buildGameStation,
  entrySet: buildEntrySet,
  door: buildDoor,
  entryDoor: buildEntryDoor,
  glassDoor: buildGlassDoor,
  fusuma: buildFusuma,
  balcony: buildBalconyProps,
  upholsteredBed: buildUpholsteredBed,
  metalBed: buildMetalBed,
  wardrobe: buildWardrobe,
  displayCabinet: buildDisplayCabinet,
  wineCabinet: buildWineCabinet,
  islandKitchen: buildIslandKitchen,
  gamingDesk: buildGamingDesk,
  marbleTable: buildMarbleTable,
  curvedSofa: buildCurvedSofa,
  // 专用件：此前镜子/鞋柜/洗衣机/吧台凳都是拿 shelf/fridge/cushionItem 顶替的，
  // 镜子里长书、洗衣机贴冰箱贴。这些错位只能靠补专用件解决，改配色救不了。
  mirror: buildMirror,
  shoeCabinet: buildShoeCabinet,
  bench: buildBench,
  washer: buildWasher,
  sideboard: buildSideboard,
  vanity: buildVanity,
  sideTable: buildSideTable,
  stool: buildStool,
  loungeChair: buildLoungeChair,
  bookcase: buildBookcase,
  toilet: buildToilet,
  plant: (spec) => buildPlant((spec as unknown as { scale?: number }).scale ?? 1),
  // lifestyle 是唯一不走 size 的 kind：拖鞋/伞/快递箱等摆件坐标直接挂在同一条目上，
  // 所以在这里收口做一次转换，别把 any 散到调用处。
  lifestyle: (spec) => buildLifestyleDetails(spec as unknown as LifestyleConfig),
  // 日式公寓 LDK 重设计：全部新几何 + 新纹理
  jpBookRack: buildJpBookRack,
  jpTvBoard: buildJpTvBoard,
  jpRug: buildJpRug,
  jpLowSofa: buildJpLowSofa,
  jpCenterTable: buildJpCenterTable,
  jpZabuton: buildJpZabuton,
  jpFloorLamp: buildJpFloorLamp,
  jpPlant: buildJpPlant,
  jpFloorClutter: buildJpFloorClutter,
  jpSlatWall: buildJpSlatWall,
  jpDiningTable: buildJpDiningTable,
  jpDiningChair: buildJpDiningChair,
  jpPendant: buildJpPendant,
  jpDiningRug: buildJpDiningRug,
  jpKitchenCounter: buildJpKitchenCounter,
  jpKitchenShelf: buildJpKitchenShelf,
  jpFridge: buildJpFridge,
  // 日式公寓 · 全屋扩展
  jpBed: buildJpBed,
  jpNightstand: buildJpNightstand,
  jpWardrobe: buildJpWardrobe,
  jpDesk: buildJpDesk,
  jpDeskChair: buildJpDeskChair,
  jpLoungeChair: buildJpLoungeChair,
  jpMirror: buildJpMirror,
  jpShoeCabinet: buildJpShoeCabinet,
  jpBench: buildJpBench,
  jpClosetShelf: buildJpClosetShelf,
  jpToilet: buildJpToilet,
  jpBath: buildJpBath,
  jpVanity: buildJpVanity,
  jpWasher: buildJpWasher,
  jpBalconyTable: buildJpBalconyTable,
  jpBalconyChair: buildJpBalconyChair,
  jpPlanter: buildJpPlanter,
  jpLifestyle: (spec) => buildJpLifestyleDetails(spec as unknown as LifestyleConfig),
};

/**
 * 角色贴图的边长上限。
 *
 * 角色 GLB 已由 scripts/room/repack_character_textures.py 原地重打包为
 * **1024² JPEG** 内嵌贴图（原 Tripo 导出的 4096² 在部分 WebView2/显卡环境下
 * createImageBitmap 解码失败,GLTFLoader 容忍式加载会把角色渲染成白模）。
 * 现在贴图尺寸已 ≤ 上限,shrinkTexture 恒为 no-op,此上限保留作为防线:
 * 将来若再换大贴图,自动缩到 1024² 兜底,避免显存与解码风险回潮。
 */
const MODEL_MAP_MAX = 1024;

/**
 * 把超过 max 的贴图缩到 max——**就地**换掉 `texture.image`。
 *
 * 就地改而不是新建 Texture：`sink` 里登记的是这个对象，换掉会让卸载时的
 * dispose 落空，那 21 MB 就永远收不回来了。
 *
 * 用 canvas 而不是 ImageBitmap：GLTFLoader 在支持的浏览器上给的是 ImageBitmap，
 * canvas 的 drawImage 对两者都收，而且 canvas 本身就能被 three 当 image 直接上传。
 */
function shrinkTexture(tex: THREE.Texture, max: number): void {
  const img = tex.image as (CanvasImageSource & { width?: number; height?: number }) | null;
  const w = img?.width ?? 0, h = img?.height ?? 0;
  if (!img || w <= max || h <= max) return;
  const scale = max / Math.max(w, h);
  const tw = Math.max(1, Math.round(w * scale));
  const th = Math.max(1, Math.round(h * scale));
  const canvas = document.createElement('canvas');
  canvas.width = tw;
  canvas.height = th;
  const ctx = canvas.getContext('2d');
  // 拿不到 2d 上下文就原样留着：宁可多占显存，也不能把角色贴图弄丢
  if (!ctx) return;
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(img, 0, 0, tw, th);
  tex.image = canvas;
  tex.needsUpdate = true;
}

/**
 * 把 GLB 里的 PBR 材质换成卡通材质。
 * 扫描出来的 Q 版模型带的金属度/粗糙度在赛璐璐风格下全是噪音，
 * 但 color / map / emissive 要原样保留，否则角色会变成一坨白。
 *
 * GLB 自带的贴图（map / normalMap）推进 sink，供卸载时精确 dispose——
 * 程序化贴图是模块级单例，不能进统一清理。
 */
function toonifyModel(root: THREE.Object3D, sink: THREE.Texture[]): void {
  const gradientMap = toonGradient();
  // 转换后材质会丢弃的贴图槽位，全部推进 sink 供卸载时 dispose——
  // 只收 map/normalMap 的话，其余槽位的 GPU 纹理在卸载后无人释放。
  const TEXTURE_SLOTS: Array<keyof THREE.MeshStandardMaterial> = [
    'map', 'normalMap', 'emissiveMap', 'metalnessMap', 'roughnessMap',
    'aoMap', 'alphaMap', 'bumpMap', 'lightMap', 'displacementMap',
  ];
  root.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh) return;

    const src = mesh.material as THREE.Material | THREE.Material[];
    const convert = (mat: THREE.Material): THREE.Material => {
      if ((mat as THREE.MeshToonMaterial).isMeshToonMaterial) return mat;
      const std = mat as THREE.MeshStandardMaterial;
      for (const slot of TEXTURE_SLOTS) {
        const tex = std[slot] as THREE.Texture | undefined;
        if (tex && !sink.includes(tex)) sink.push(tex);
      }
      const next = new THREE.MeshToonMaterial({
        color: std.color ? std.color.clone() : new THREE.Color(0xffffff),
        map: std.map ?? null,
        gradientMap,
        transparent: std.transparent ?? false,
        opacity: std.opacity ?? 1,
        side: std.side ?? THREE.FrontSide,
        alphaTest: std.alphaTest ?? 0,
        vertexColors: std.vertexColors ?? false,
      });
      if (std.emissive) {
        next.emissive = std.emissive.clone();
        next.emissiveIntensity = std.emissiveIntensity ?? 1;
      }
      if (std.normalMap) next.normalMap = std.normalMap;
      mat.dispose();
      return next;
    };

    const out = Array.isArray(src) ? src.map(convert) : convert(src);
    mesh.material = out;
    // GLB 内嵌贴图通常已由资源脚本压到 1024²；若将来资源变大，再缩到上限。
    // 放在材质换完之后：转换会丢掉一部分贴图槽位，只缩真正留下来会进显存的那张。
    for (const m of Array.isArray(out) ? out : [out]) {
      const map = (m as THREE.MeshToonMaterial).map;
      if (map) shrinkTexture(map, MODEL_MAP_MAX);
    }
    mesh.castShadow = true;
    mesh.receiveShadow = true;
  });
}

/**
 * 每帧的点光预算。
 *
 * three **不做逐物体剔除**：每盏点光都会进片元着色器的光照循环
 * （`NUM_POINT_LIGHTS` 是编译期常量，循环被展开），场景里 34 盏就是每个受光
 * 片元跑 34 遍。实测把 34 盏**全部**关掉，观察者机位下也只有 2.7% 的像素发生
 * 变化——绝大多数灯在当前视角里根本没照到东西，白算。
 *
 * 所以每帧只让"最可能影响画面"的 BUDGET 盏亮着，排序分两级：
 *   1. 光球（半径 = 灯的 distance，也就是它的有效作用半径）与相机视锥相交的
 *      优先——只有它们可能照到屏幕上的东西；
 *   2. 同组内按 `intensity / d²` 降序（d = 到相机距离，下限 1m）。这是"看起来
 *      有多亮"的代理量：远处全场入画时所有 d 相近，退化成按 intensity 排，
 *      挑出真正亮的那几盏；第一人称贴着店面时近处的灯 d 小、分数暴涨，挑出的
 *      就是身边这几盏。
 *
 *      **不能用纯距离排**。观察者机位离场景 40m，全部灯都在视锥里，按距离排
 *      只会把镜头这一侧的灯全留下、把街对面那排店面（美术真正要的）全砍掉。
 *
 * **可见数量必须恒定**。three 的着色器程序按 `lights.point.length` 进缓存键
 * （three.cjs:20845 取值、20976 进 key），数量一变就要重编译全部材质——实测
 * 一次几百毫秒的卡顿，比省下来的光照还贵。所以永远留满 BUDGET 盏、只换是哪
 * 几盏：光源 uniform 每帧重传（几十个 float，可忽略），程序不重编。
 *
 * 留 16 盏的依据（逐档扫描，每档都跟"全开 34 盏"比，用同一会话同一机位，
 * 并取"该状态自身帧间抖动"当噪声底）：
 *
 *   机位              留 8          留 12         留 16         留 20
 *   观察者 px>24      121           103           74            37
 *   观察者 Δ          0.261         0.255         0.132         0.091
 *   （该机位噪声底 0.073~0.239）
 *   第一人称 px>24    ~55           ~58           ~57           ~55
 *   第一人称 Δ        0.016         0.136         0.019         0.016
 *
 * 第一人称对预算几乎完全不敏感——各档 px>24 都卡在 55 上下（那 55 个像素是
 * 固定差异，不是光照），Δ 全在噪声里。真正的约束来自观察者机位：12 盏时
 * Δ 0.255 是该机位噪声底 0.073 的 3.5 倍，已经算"看得出一点点"；16 盏时
 * Δ 0.132 落在噪声底（0.174）之下，三个机位全部报"看不出来"。
 * 所以取 16：这是**每个机位都测不出差异**的最小档，相对 34 盏把每个受光片元
 * 的光照循环砍掉一半以上。
 */
const POINT_LIGHT_BUDGET = 16;
const _ltFrustum = new THREE.Frustum();
const _ltProjScreen = new THREE.Matrix4();
const _ltSphere = new THREE.Sphere();
const _ltWorld = new THREE.Vector3();

/** 把池子里的点光收进预算：前 BUDGET 盏 `visible = true`，其余关掉。 */
function budgetPointLights(camera: THREE.Camera, pool: THREE.PointLight[], budget: number): void {
  if (pool.length <= budget) {
    for (const l of pool) l.visible = true;
    return;
  }
  camera.updateMatrixWorld();
  _ltProjScreen.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
  _ltFrustum.setFromProjectionMatrix(_ltProjScreen);

  const scored: Array<{ l: THREE.PointLight; rank: number; score: number }> = [];
  for (const l of pool) {
    l.getWorldPosition(_ltWorld);
    _ltSphere.center.copy(_ltWorld);
    // distance = 0 在 three 里表示"不衰减"，那种灯没有作用半径，给个大球让它
    // 永远落进第一组——它一定是要紧的。
    _ltSphere.radius = l.distance > 0 ? l.distance : 1e3;
    const d = Math.max(camera.position.distanceTo(_ltWorld), 1);
    scored.push({
      l,
      rank: _ltFrustum.intersectsSphere(_ltSphere) ? 0 : 1,
      score: l.intensity / (d * d),
    });
  }
  scored.sort((a, b) => (a.rank !== b.rank ? a.rank - b.rank : b.score - a.score));
  for (let i = 0; i < scored.length; i++) scored[i].l.visible = i < budget;
}

/** 帮助面板的命令条目（/help 呼出）。group 决定归属分区：时间 / 天气 / 系统。 */
type CmdEntry = { cmd: string; desc: string; group: 'time' | 'weather' | 'other' };

const HELP_ENTRIES: CmdEntry[] = [
  { cmd: '/day', desc: '白天（正午日光）', group: 'time' },
  { cmd: '/morning', desc: '早晨', group: 'time' },
  { cmd: '/noon', desc: '正午', group: 'time' },
  { cmd: '/dusk', desc: '黄昏', group: 'time' },
  { cmd: '/night', desc: '深夜', group: 'time' },
  { cmd: '/clear', desc: '晴天', group: 'weather' },
  { cmd: '/rain', desc: '小雨', group: 'weather' },
  { cmd: '/storm', desc: '暴雨', group: 'weather' },
  { cmd: '/snow', desc: '降雪', group: 'weather' },
  { cmd: '/reset', desc: '恢复真实世界时段与天气', group: 'other' },
  { cmd: '/help', desc: '显示本帮助', group: 'other' },
];

/** 帮助面板的分区顺序与标签（赛博朋克双语：英文走 Orbitron）。 */
const CMD_GROUPS: { key: CmdEntry['group']; label: string }[] = [
  { key: 'time', label: 'TIME // 时间' },
  { key: 'weather', label: 'WEATHER // 天气' },
  { key: 'other', label: 'SYSTEM // 系统' },
];

/** 去掉命令前导斜杠，联想/执行都拿"纯字母名"做前缀匹配。 */
const normCmdName = (c: string) => (c.startsWith('/') ? c.slice(1) : c);

export function RoomScene() {
  const containerRef = useRef<HTMLDivElement>(null);
  const hudStatsRef = useRef<HTMLDivElement>(null);
  const hudAgentsRef = useRef<HTMLDivElement>(null);
  const fpsDebugRef = useRef<HTMLDivElement>(null);
  const hudModeRef = useRef<HTMLDivElement>(null);
  const hudCrosshairRef = useRef<HTMLDivElement>(null);
  const hudDoorPromptRef = useRef<HTMLDivElement>(null);
  const hudSpeedRef = useRef<HTMLDivElement>(null);
  const hudEnvRef = useRef<HTMLDivElement>(null);
  // 环境应用器。真实世界感知在正常运行时是唯一调用方，但它必须能被 polling effect
  // 和 __ROOM__ 测试注入口够到，所以经 ref 传出 scene effect 的作用域。
  const environmentApplyRef = useRef<((period: DayPeriod, weather: WeatherKind) => void) | null>(null);
  /**
   * 当前环境。初值直接按本地时钟推导，而不是写死 night/drizzle ——
   * 否则真实感知首次回调到达前，房间会先闪一帧错的时段。
   */
  const environmentRef = useRef<{ period: DayPeriod; weather: WeatherKind }>(
    resolveEnvironment(environmentFromLocalClock())
  );
  /** 当前环境来源，只给 HUD 摘要用（"Open-Meteo" / "本地时钟" / "测试注入"）。 */
  const environmentSourceRef = useRef<string>('本地时钟');
  /** 测试注入用的环境源覆盖；null = 跟随真实世界感知。 */
  const environmentOverrideRef = useRef<Partial<EnvironmentInput> | null>(null);
  /** 让 __ROOM__.setEnvironmentSource 触发一次即时重算，不必等下一个轮询周期。 */
  const environmentTickRef = useRef<(() => void) | null>(null);

  /* ---------------- 命令面板（/ 呼出，Minecraft 风格） ---------------- */
  const [cmdOpen, setCmdOpen] = useState(false);
  const cmdOpenRef = useRef(false);
  const cmdInputRef = useRef<HTMLInputElement>(null);
  /** 命令输入框当前内容（受控，纯字母）。联想卡片按它过滤。 */
  const [cmdInput, setCmdInput] = useState('');
  /** 联想卡片当前高亮行索引（↑↓ 导航用），-1 = 无高亮。 */
  const [cmdActiveIdx, setCmdActiveIdx] = useState(-1);
  /** 帮助面板（/help 呼出）：与命令输入面板互斥，ESC 优先关它。 */
  const [cmdHelpOpen, setCmdHelpOpen] = useState(false);
  const cmdHelpRef = useRef(false);
  /** 帮助面板容器引用：外部点击关闭用它判定「点在面板外」。 */
  const cmdHelpPanelRef = useRef<HTMLDivElement>(null);
  /** 命令执行结果 toast（null = 不显示）。错误保留面板让玩家改，成功才收起。 */
  const [cmdFeedback, setCmdFeedbackState] = useState<{ text: string; ok: boolean } | null>(null);
  const cmdFeedbackTimerRef = useRef<number>(0);
  /** 命令手动覆盖的环境（null = 跟随真实世界感知）。比 environmentOverrideRef 更直接：
   * 它存的是最终的 period/weather，而不是感知层 input —— 命令无需回推一个能映射出
   * 目标时段的小时数。 */
  const manualEnvironmentRef = useRef<{ period: DayPeriod; weather: WeatherKind } | null>(null);

  /** 同步「命令浮层整体是否开着」到 Rust 硬件 ESC 看护与 RoomWindow：
   * 任一浮层（输入面板/帮助面板）打开时，ESC 归前端收浮层，不让看护关窗口。 */
  const syncCmdOverlay = useCallback(() => {
    const open = cmdOpenRef.current || cmdHelpRef.current;
    void invoke('set_room_escape_suppressed', { suppressed: open }).catch((e) =>
      console.warn('[room] set_room_escape_suppressed 失败', e)
    );
    const room = (window as any).__ROOM__;
    if (room) room.cmdPanelOpen = open;
  }, []);

  const setCmdOpenState = useCallback((open: boolean) => {
    cmdOpenRef.current = open;
    setCmdOpen(open);
    syncCmdOverlay();
    if (!open) {
      // 收面板时清空输入与联想高亮，下次呼出是干净的。
      setCmdInput('');
      setCmdActiveIdx(-1);
    }
    const room = (window as any).__ROOM__;
    if (open) {
      // 第一人称下呼出：先退出指针锁定，否则焦点进不了输入框（也没法打字）。
      if (room?.fps?.locked) {
        try { document.exitPointerLock(); } catch { /* ignore */ }
      }
      requestAnimationFrame(() => cmdInputRef.current?.focus());
    }
  }, [syncCmdOverlay]);

  const setCmdHelpState = useCallback((open: boolean) => {
    cmdHelpRef.current = open;
    setCmdHelpOpen(open);
    syncCmdOverlay();
    if (open) {
      // 帮助面板是纯展示，无需持锁状态：第一人称下呼出同样先退出指针锁定。
      const room = (window as any).__ROOM__;
      if (room?.fps?.locked) {
        try { document.exitPointerLock(); } catch { /* ignore */ }
      }
    }
  }, [syncCmdOverlay]);

  /** 联想匹配：空输入不联想；精确 > 前缀 > 包含，最多 6 条（搜索引擎式排序）。 */
  const cmdMatches = useMemo<{ cmd: string; desc: string }[]>(() => {
    const q = cmdInput.trim().toLowerCase();
    if (!q) return [];
    const exact: { cmd: string; desc: string }[] = [];
    const prefix: { cmd: string; desc: string }[] = [];
    const contains: { cmd: string; desc: string }[] = [];
    for (const it of HELP_ENTRIES) {
      const n = normCmdName(it.cmd).toLowerCase();
      if (n === q) { exact.push(it); continue; }
      if (n.startsWith(q)) { prefix.push(it); continue; }
      if (n.includes(q)) contains.push(it);
    }
    return [...exact, ...prefix, ...contains].slice(0, 6);
  }, [cmdInput]);

  /** 联想行内的 cmd 高亮：命中段提亮 + 辉光，其余部分正常描。 */
  const renderCmdName = useCallback((cmd: string) => {
    const n = normCmdName(cmd);
    const q = cmdInput.trim().toLowerCase();
    if (!q) return <span>{cmd}</span>;
    const qi = n.toLowerCase().indexOf(q);
    if (qi < 0) return <span>{cmd}</span>;
    const pre = n.slice(0, qi);
    const hit = n.slice(qi, qi + q.length);
    const post = n.slice(qi + q.length);
    // 命中的字母提绿 + 辉光，未命中保持白色（与原语义相反的颜色处理：绿=命中）。
    const white = { color: 'rgba(236, 246, 255, 0.92)' } as const;
    return (
      <span>
        <span style={white}>{'/'}{pre}</span>
        <span style={{ color: '#7cf2a8', textShadow: '0 0 9px rgba(124,242,168,0.95)' }}>{hit}</span>
        <span style={white}>{post}</span>
      </span>
    );
  }, [cmdInput]);

  const showCmdFeedback = useCallback((text: string, ok: boolean) => {
    setCmdFeedbackState({ text, ok });
    window.clearTimeout(cmdFeedbackTimerRef.current);
    cmdFeedbackTimerRef.current = window.setTimeout(() => setCmdFeedbackState(null), 3500);
  }, []);

  /** 解析并执行命令。返回成功与否：成功收起面板，失败/帮助保留面板方便玩家改。 */
  const executeCommand = useCallback((raw: string) => {
    const line = raw.replace(/^\/+/, '').trim();
    if (!line) return;
    const parts = line.split(/\s+/);
    const [c0, c1, c2] = parts;
    const c = (c0 ?? '').toLowerCase();
    const arg = (c1 ?? '').toLowerCase();

    const periodAliases: Record<string, DayPeriod> = {
      day: 'noon', morning: 'morning', noon: 'noon', dusk: 'dusk', night: 'night',
    };
    const weatherAliases: Record<string, WeatherKind> = {
      clear: 'clear', sunny: 'clear', rain: 'drizzle', drizzle: 'drizzle',
      storm: 'storm', thunder: 'storm', thunderstorm: 'storm', snow: 'snow',
    };

    if (c === 'help' || c === '?') {
      // 帮助走独立面板（赛博朋克风格），收起输入面板再弹帮助；ESC / × 关闭。
      setCmdOpenState(false);
      setCmdHelpState(true);
      return;
    }
    if (c === 'reset' || c === 'realtime' || c === 'auto') {
      manualEnvironmentRef.current = null;
      environmentTickRef.current?.();
      showCmdFeedback('已恢复跟随真实世界时段/天气', true);
      setCmdOpenState(false);
      return;
    }

    let targetPeriod: DayPeriod | null = null;
    let targetWeather: WeatherKind | null = null;
    if (periodAliases[c]) {
      targetPeriod = periodAliases[c];
    } else if (c === 'time') {
      // 兼容 Minecraft 风格：/time set day、/time day
      const t = (c1 === 'set' ? (c2 ?? '') : (c1 ?? '')).toLowerCase();
      targetPeriod = periodAliases[t] ?? null;
    }
    if (weatherAliases[c]) {
      targetWeather = weatherAliases[c];
    } else if (c === 'weather') {
      // 兼容 Minecraft 风格：/weather rain
      targetWeather = weatherAliases[arg] ?? null;
    }

    if (!targetPeriod && !targetWeather) {
      showCmdFeedback(`未知命令：/${line}（输入 /help 查看可用命令）`, false);
      return;
    }

    const cur = manualEnvironmentRef.current ?? environmentRef.current;
    const next: { period: DayPeriod; weather: WeatherKind } = {
      period: targetPeriod ?? cur.period,
      weather: targetWeather ?? cur.weather,
    };
    manualEnvironmentRef.current = next;
    environmentTickRef.current?.();
    showCmdFeedback(
      targetPeriod
        ? `已设置时间为${PERIOD_LABELS[targetPeriod]}`
        : `已设置天气为${WEATHER_LABELS[targetWeather as WeatherKind]}`,
      true
    );
    setCmdOpenState(false);
  }, [showCmdFeedback, setCmdOpenState, setCmdHelpState]);
  const [hudVisible, setHudVisible] = useState(false);
  // 观察者模式（默认：OrbitControls 自由视角 + 单向透视墙）↔ 第一人称（PointerLock）
  const [mode, setMode] = useState<'observe' | 'firstPerson'>('observe');

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // 场景装配开始。它与 'room:chunk-done' 之间的差值就是 React 挂载 + 本函数
    // 之前那段的开销；从这里到 'scene:built' 则是**一整段同步阻塞主线程**的装配
    // 时间——首屏 loading 层的 CSS 动画必须能在这一段里继续转，否则会看起来卡死。
    bootMark('scene:build-start');

    const layout = layoutData as any;
    setArtStyle(layout.palette, layout.style);
    /**
     * 主光方向喂给材质分档。必须在这里调（任何 builder 之前）：finish 的高光方向
     * 是烘焙进 shader 的常量，材质一建就定型，事后改不回来。
     * 取 key.position 归一化 = 表面指向光源的方向（DirectionalLight 默认 target 在原点）。
     */
    const KP = layout.lighting.key.position as [number, number, number];
    setToonKeyLight(KP[0], KP[1], KP[2]);

    let alive = true;

    /* ---------------- Renderer / Scene / Camera ---------------- */

    const scene = new THREE.Scene();
    // 窗口不透明：场景铺底色，全屏房间视觉完整（透明窗口 + WebGL 在 Windows
    // 是 GPU 崩溃高危组合，已改回不透明）。底色本身在下方 FX 解析后设置。

    /* ---------------- 后处理配置（可在 dormLayout.json 的 postfx 里热调） ---------------- */

    const FX = (layout.postfx ?? {}) as {
      background?: string;
      toneMapping?: string;
      exposure?: number;
      bloom?: { strength?: number; radius?: number; threshold?: number };
      fog?: { color?: string; near?: number; far?: number } | null;
    };
    const BLOOM = FX.bloom ?? {};
    type EnvironmentProfile = {
      background: string; fog: string; ambient: string; hemiSky: string; hemiGround: string;
      key: string; fill: string; rim: string; exposure: number; keyMul: number; fillMul: number; rimMul: number;
    };
    const PERIODS: Record<DayPeriod, EnvironmentProfile> = {
      morning: { background: '#a9c8dc', fog: '#c2d8df', ambient: '#f7e8cf', hemiSky: '#b9d4e4', hemiGround: '#68776e', key: '#ffe2b5', fill: '#c6dce4', rim: '#f2d6b6', exposure: 1.04, keyMul: 0.82, fillMul: 0.75, rimMul: 0.55 },
      noon: { background: '#8fc7ee', fog: '#c5e1ef', ambient: '#fff8e8', hemiSky: '#b9dcf5', hemiGround: '#6f806f', key: '#fff5d2', fill: '#dcecf4', rim: '#fff3d1', exposure: 1.14, keyMul: 1.35, fillMul: 0.58, rimMul: 0.42 },
      dusk: { background: '#d88f76', fog: '#d9b6a3', ambient: '#f0c3a2', hemiSky: '#c68e8d', hemiGround: '#514b50', key: '#ffbd76', fill: '#a79ab1', rim: '#f4a978', exposure: 1.0, keyMul: 0.68, fillMul: 0.52, rimMul: 0.85 },
      night: { background: FX.background ?? '#1b2230', fog: FX.fog?.color ?? '#131e2c', ambient: '#9FB4D2', hemiSky: '#93AACE', hemiGround: '#262A33', key: '#FFDCA8', fill: '#93ACD6', rim: '#AEC4E8', exposure: FX.exposure ?? 1, keyMul: 1, fillMul: 1, rimMul: 1 },
    };
    const WEATHER_TUNING: Record<WeatherKind, {
      tint: string; tintAmount: number; exposure: number; keyMul: number; fillMul: number; rimMul: number;
    }> = {
      clear: { tint: '#ffffff', tintAmount: 0, exposure: 1, keyMul: 1, fillMul: 1, rimMul: 1 },
      drizzle: { tint: '#8398aa', tintAmount: 0.12, exposure: 0.95, keyMul: 0.86, fillMul: 1.08, rimMul: 0.9 },
      storm: { tint: '#465464', tintAmount: 0.34, exposure: 0.76, keyMul: 0.58, fillMul: 0.88, rimMul: 0.7 },
      snow: { tint: '#d8e5ea', tintAmount: 0.18, exposure: 1.03, keyMul: 0.92, fillMul: 1.06, rimMul: 0.92 },
    };

    /**
     * 世界底色 = 雨夜夜空。微缩底座时代这里是暖米色 #e7d8c4（模型摆台的衬底）；
     * 拆底座进街区之后，底色就是环绕整个世界的夜空——窗外、楼顶之上、
     * 街道尽头都是它。必须和雾色同族：远处物体是"融进夜空"，不是蒙了层灰。
     */
    scene.background = new THREE.Color(FX.background ?? '#1b2230');

    /**
     * 场景雾：雨夜的空气。
     *
     * 近端约 14m——相机距焦点约 15m，室内基本不吃雾，只有房间最远角略褪；
     * 远端 70m——中景楼群（25~45m）褪 20%~80%，世界地面边缘（±85m）被雾
     * 完全吞掉，看不到"世界的边缘"。远景幕布和窗外贴图类材质走 noFog，
     * 它们的空气透视是画出来的，不吃场景雾。
     */
    // 存一份引用：按 P 做 A/B 对照时要能整段摘掉雾
    let roomFog: THREE.Fog | null = null;
    if (FX.fog) {
      // 这里在 B 声明之前，直接用 layout.room.bounds，别引用 B（会踩 TDZ）
      const fb = layout.room.bounds;
      const fr = Math.hypot((fb.x1 - fb.x0) / 2, (fb.z1 - fb.z0) / 2);
      roomFog = new THREE.Fog(
        new THREE.Color(FX.fog.color ?? '#e0d3c2'),
        FX.fog.near ?? fr * 2.0,
        FX.fog.far ?? fr * 6.5
      );
      scene.fog = roomFog;
    }

    const cam = layout.camera;
    /**
     * 单元抬高层高：这户是公寓楼的 203 室，整户（外壳/家具/角色/阳台湿地）
     * 挂在 unitGroup 下抬到二楼。楼下是 101-103 外墙（exterior.ts），
     * 世界地面留在 y=0。所有吃世界 Y 的硬编码（相机、FPS 眼高、雨禁区、
     * 碰撞盒、天花剔除）都从这一个常量派生，改层高只动这里。
     */
    const UNIT_LIFT = 3.4;
    // far 平面要罩住世界地面的最远角：地面 ±85m，相机最远拉到 ~25m，
    // 角上相距可达 ~140m——裁掉的话被雾吞掉的地面边缘会露出背景色的"世界裂缝"
    const camera = new THREE.PerspectiveCamera(cam.fov, 1, 0.1, 420);
    camera.position.set(cam.position[0], cam.position[1] + UNIT_LIFT, cam.position[2]);
    camera.lookAt(cam.target[0], cam.target[1] + UNIT_LIFT, cam.target[2]);

    /**
     * 锁定水平视野而不是竖直视野。
     * 布局里的 fov 是按 16:9 调的，房间窗口一旦更方（4:3、竖屏），
     * 按竖直 fov 走就会把房间左右两边切掉，所以窄窗口时要反向张开竖直 fov。
     */
    let isFirstPerson = false; // 提前声明，供 applyProjection 按模式选视野
    const BASE_FOV = cam.fov;  // 观察者模式视野（偏窄，构图用）
    const FPS_FOV = 60;        // 稳定纵向视野，室内兼顾周边视野与透视比例
    const BASE_ASPECT = 1.78;
    const applyProjection = (w: number, h: number) => {
      const aspect = Math.max(1, w) / Math.max(1, h);
      camera.aspect = aspect;
      const halfH = isFirstPerson
        // 第一人称不套用全景构图补偿；超宽屏水平视野封顶 100°。
        ? Math.min(Math.tan(THREE.MathUtils.degToRad(FPS_FOV) / 2), Math.tan(THREE.MathUtils.degToRad(100) / 2) / aspect)
        : Math.tan(THREE.MathUtils.degToRad(BASE_FOV) / 2) * Math.max(1, BASE_ASPECT / aspect);
      camera.fov = THREE.MathUtils.radToDeg(2 * Math.atan(halfH));
      camera.updateProjectionMatrix();
    };
    applyProjection(container.clientWidth, container.clientHeight);

    // 不透明渲染：不设 alpha（默认不透明画布），也不用 clearAlpha 0
    const renderer = new THREE.WebGLRenderer({
      antialias: true,
      // 提示 WebView2 用独显渲染：双显卡机器默认可能落到集显，集显驱动下
      // 全屏 WebGL 容易 context lost / GPU 进程崩溃
      powerPreference: 'high-performance',
    });
    // [TEMP-DEBUG] 几何排查用，验证完即删
    (window as any).__ROOM__ = { scene, camera, renderer, THREE, cmdPanelOpen: false };
    renderer.setSize(container.clientWidth, Math.max(1, container.clientHeight));
    // 全屏渲染负载高：像素比封顶 1.5，避免 2x 的 4 倍像素把 GPU/显存逼到崩溃
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 1.5));
    renderer.shadowMap.enabled = true;
    renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    // One prefiltered environment supplies broad, soft highlights to the Blender PBR assets.
    // Generated once, without a network HDRI or additional per-frame lighting passes.
    const studio = new RoomEnvironment();
    const pmrem = new THREE.PMREMGenerator(renderer);
    const furnitureEnvironment = pmrem.fromScene(studio, 0.06);
    scene.environment = furnitureEnvironment.texture;
    scene.environmentIntensity = 0.24;
    studio.dispose();
    pmrem.dispose();
    // PMREM（IBL 预滤波）是装配期里少数几处**纯 GPU** 开销，单独打点：它在集显上
    // 可能是装配段的大头，而它只影响 PBR 家具的反射，理论上可以推迟到首帧之后再算。
    bootMark('scene:pmrem');
    /**
     * 色调映射。
     *
     * 原先是 NoToneMapping，理由是"ACES 会把饱和的赛璐璐色压灰"——这个理由对 ACES
     * 成立（它连中间调一起压），但对 Neutral 不成立：Neutral 只滚降高光
     * （阈值在 0.8 以上才开始压缩），中间调原样保留，赛璐璐的平涂色不会变灰。
     *
     * 换掉它的实际原因是高光在切：环境光 0.62 + 半球 0.72 + 主光 1.3 叠下来，
     * 受光面早就超过 1.0 被 clamp 成死白，墙面细节在亮部全丢。Neutral 把这些
     * 收回来，同时让超过 1.0 的部分成为泛光的合法输入（见下面的 composer）。
     * 想退回原样就把 postfx.toneMapping 写成 "none"。
     */
    const TONE_MAPPINGS: Record<string, THREE.ToneMapping> = {
      none: THREE.NoToneMapping,
      linear: THREE.LinearToneMapping,
      aces: THREE.ACESFilmicToneMapping,
      agx: THREE.AgXToneMapping,
      neutral: THREE.NeutralToneMapping,
    };
    const baseToneMapping = TONE_MAPPINGS[FX.toneMapping ?? 'neutral'] ?? THREE.NeutralToneMapping;
    renderer.toneMapping = baseToneMapping;
    renderer.toneMappingExposure = FX.exposure ?? 1;
    // three 在 shadow pass 之后才 reset 统计，默认读数不含阴影开销。
    // 关掉自动重置、改为每帧开头手动重置，draw call 才是整帧的真实数字。
    renderer.info.autoReset = false;
    container.appendChild(renderer.domElement);

    /* ---------------- 后处理链 ----------------
     *
     * RenderPass（线性 HDR，HalfFloat 不截断）
     *   → UnrealBloomPass（只对超过阈值的亮部起雾，屏幕/灯泡/霓虹才会发光）
     *   → OutputPass（最后一步才做色调映射 + sRGB 转换）
     *
     * 顺序不能颠倒：泛光必须作用在**色调映射之前**的线性 HDR 上。反过来的话
     * 亮部已经被压回 1.0 以内，"哪些地方过曝了"这个信息就没了，泛光会变成
     * 一层糊在整幅画面上的白纱。
     */

    // 自己建 RT 而不是用 composer 的默认 RT：默认的没有 MSAA，
    // 而渲染器上的 antialias:true 只作用于默认帧缓冲，一旦走 composer 就失效，
    // 描边和家具棱角会立刻开始闪锯齿。samples=4 是给 composer 补回抗锯齿。
    const dpr = renderer.getPixelRatio();
    const fxTarget = new THREE.WebGLRenderTarget(
      Math.max(1, Math.floor(container.clientWidth * dpr)),
      Math.max(1, Math.floor(container.clientHeight * dpr)),
      { type: THREE.HalfFloatType, samples: 4 }
    );
    const composer = new EffectComposer(renderer, fxTarget);
    composer.setPixelRatio(dpr);
    composer.setSize(container.clientWidth, Math.max(1, container.clientHeight));

    composer.addPass(new RenderPass(scene, camera));
    const bloomPass = new UnrealBloomPass(
      new THREE.Vector2(container.clientWidth, Math.max(1, container.clientHeight)),
      BLOOM.strength ?? 0.34,
      BLOOM.radius ?? 0.62,
      BLOOM.threshold ?? 0.86
    );
    composer.addPass(bloomPass);
    composer.addPass(new OutputPass());

    /* ---------------- 泛光降分辨率 ----------------
     *
     * UnrealBloomPass 自己先把 mip0 取半（w/2 × h/2），再逐级降到 1/32；一轮
     * 是 5 级 mip × 2 次分离模糊 + 高通 + 合成，像素量约等于整屏的 1/3。
     * 而泛光本来就是低频信息——再降一半（mip0 = 屏幕 1/4）肉眼几乎看不出，
     * 代价却直接少 4×。这是引擎里的常规做法（1/4 或 1/8 分辨率泛光）。
     *
     * 实现上包装 bloomPass.setSize 而不是在调用点手算：composer.setSize 会把
     * **有效像素尺寸**转发给每个 pass，包装住它，初始化 / 切倍率 / 首帧补尺寸 /
     * 窗口 resize 四条路径就都自动生效，将来新增调用点也不会漏。
     */
    let bloomScale = 0.5;
    const bloomSetSize = bloomPass.setSize.bind(bloomPass);
    bloomPass.setSize = (w: number, h: number): void => {
      bloomSetSize(
        Math.max(1, Math.floor(w * bloomScale)),
        Math.max(1, Math.floor(h * bloomScale))
      );
    };
    // 上面的包装晚于首帧那次 composer.setSize，按当前尺寸补一次
    composer.setSize(container.clientWidth, Math.max(1, container.clientHeight));
    // 调试/验证用：A/B 对照泛光分辨率（1 = 关掉这项优化）
    (window as any).__ROOM__.setBloomScale = (v: number): void => {
      bloomScale = Math.max(0.25, Math.min(1, v));
      composer.setSize(container.clientWidth, Math.max(1, container.clientHeight));
    };
    (window as any).__ROOM__.getBloomScale = () => ({
      scale: bloomScale,
      // 泛光 mip0 的 RT 尺寸——自检用：应恒为 composer RT 的 bloomScale 倍
      bloomRT: [bloomPass.renderTargetsHorizontal[0].width, bloomPass.renderTargetsHorizontal[0].height],
    });

    /* ---------------- 自适应渲染倍率（动态分辨率） ----------------
     *
     * 这个场景是「全屏填充 + 全屏后处理」型：主 pass 之后还有泛光的 5 级 mip、
     * MSAA 4× 的解析、OutputPass，而 MSAA RT 本身是 HalfFloat——**像素数几乎
     * 直接决定帧时**。集显上真正的瓶颈就在这里，不在 draw call（实测第一人称
     * 664 个，比观察者模式还少）。
     *
     * 所以按实测帧时自动升降渲染倍率：跑得动就一直是满倍率，跑不动才逐级降，
     * 有余量了再升回来。降的是渲染分辨率、不是 CSS 尺寸，所以构图 / 视野 /
     * 鼠标手感 / 交互都不变，只是画面软一点。最低 0.7×——再低就明显糊了。
     *
     * 两个防抖措施，缺一不可：
     *  - 升档要求「连续 8 个采样窗都很快」+「距上次降档 ≥20s」，否则帧时刚好
     *    卡在阈值附近时会来回抖，比一直低一档更难受；
     *  - 帧时 ≥120ms 时**不做任何判断**：那是切后台、断点、或软件光栅，不是
     *    持续负载，跟着降档只会把画质白白降下去（headless 验证时也正是靠这条
     *    保证探针不会把倍率降下去）。
     */
    const SCALE_STEPS = [1, 0.9, 0.8, 0.7];
    let scaleIdx = 0;
    let scaleHold = 0;      // 切档后的冷却秒数
    let slowStreak = 0;
    let fastStreak = 0;
    let lastDownAt = -1e9;

    const applyRenderScale = (idx: number): void => {
      if (idx === scaleIdx) return;
      scaleIdx = idx;
      const next = dpr * SCALE_STEPS[idx];
      const w = container.clientWidth;
      const h = Math.max(1, container.clientHeight);
      renderer.setPixelRatio(next);
      renderer.setSize(w, h);
      // composer 自己持有一套 RT（含传进去的 MSAA target），必须同步重设——
      // 否则后处理仍按旧分辨率画，画面会糊成一块
      composer.setPixelRatio(next);
      composer.setSize(w, h);
      console.info(`[room] 渲染倍率 → ${SCALE_STEPS[idx]}×（dpr ${next.toFixed(2)}）`);
    };
    // 调试用：手工切档，验证重设尺寸不会破坏渲染
    (window as any).__ROOM__.setRenderScale = (idx: number) =>
      applyRenderScale(Math.max(0, Math.min(SCALE_STEPS.length - 1, Math.floor(idx))));
    (window as any).__ROOM__.getRenderScale = () => ({
      idx: scaleIdx, factor: SCALE_STEPS[scaleIdx], dpr: renderer.getPixelRatio(),
      canvas: [renderer.domElement.width, renderer.domElement.height],
      // composer 的 RT（就是传进去的 fxTarget）必须跟着变，否则后处理仍按旧
      // 分辨率画——这个字段就是给自检用的
      composerRT: [composer.renderTarget1.width, composer.renderTarget1.height],
    });

    // A/B 对照开关：按 P 在"整条后处理链"和"直出"之间切，用来判断这一步到底
    // 带来了多少变化，不用改代码重启。
    let fxOn = true;

    /* ---------------- 场景范围（阴影相机 / 底座定位反推，不写死） ----------------
     * 相机/阴影的活动范围全部从 layout.room.bounds 反推，不写死。
     * 之前这些数字是按 11×8.5 的老户型硬编码的，户型一放大，
     * 远端房间直接掉出阴影相机，出现"家具悬空没影子/影子被切一半"。
     */
    const B = layout.room.bounds;
    const sceneRadius = Math.hypot((B.x1 - B.x0) / 2, (B.z1 - B.z0) / 2);

    /* ---------------- Observer mode (OrbitControls) ---------------- */

    // 默认观察者模式：自由旋转 + 缩放 + 平移，墙面单向透视（剖面娃娃屋）。
    // 按 Enter 进入第一人称。观察者视角沿用 layout.camera 的构图。
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.set(cam.target[0], cam.target[1] + UNIT_LIFT, cam.target[2]);
    controls.enableDamping = true;
    controls.dampingFactor = 0.07;
    controls.enableRotate = true;
    controls.enablePan = true;
    controls.screenSpacePanning = true;
    controls.enableZoom = true;
    controls.zoomSpeed = 0.85;
    controls.rotateSpeed = 0.65;
    controls.panSpeed = 0.65;
    controls.minDistance = 3.5;
    // 拉远上限按"整栋楼进画"定，不按房间尺度：默认构图是 30~45° 俯视的
    // 微缩全景（相机离目标约 38m），sceneRadius*2.8 只有 24m，会把默认机位
    // 一进场就夹回来，整栋楼被切成一半。
    controls.maxDistance = 180;
    controls.minPolarAngle = THREE.MathUtils.degToRad(20);
    controls.maxPolarAngle = THREE.MathUtils.degToRad(82);
    // 视线目标同样要能抬到楼层中部（默认看向 y≈5.5 世界标高，约二层半），
    // 原来只允许 UNIT_LIFT+1.45 以内的贴地范围。
    controls.target.y = THREE.MathUtils.clamp(controls.target.y, UNIT_LIFT + 0.55, UNIT_LIFT + 3.2);
    controls.update();
    (window as any).__ROOM__.controls = controls;

    /**
     * 首帧后补一次投影 + 控制器更新。页面刚加载时容器（Tauri 窗口）可能尚未完成
     * 首次布局，applyProjection / setSize 当时拿到的是 0 或错误尺寸，观察者相机投影
     * 会退化（fov/aspect 算错），表现为"首帧构图和之后不一样"。下一帧布局已就绪，
     * 这里用真实尺寸重算一次，让观察者相机从第一帧起就是正确构图。
     */
    requestAnimationFrame(() => {
      if (!alive) return;
      applyProjection(container.clientWidth, container.clientHeight);
      renderer.setSize(container.clientWidth, Math.max(1, container.clientHeight));
      composer.setSize(container.clientWidth, Math.max(1, container.clientHeight));
      controls.update();
    });

    /* ---------------- First-person controls ---------------- */

    const fps = new FPSControls(camera, renderer.domElement);
    // floorY 只作为「最低地面」兜底（街道 y=0）。上层楼板/外廊/楼梯平台/斜坡
    // 由动态支撑面 supportY 实时给出，所以第一人称能在整张地图（含 2F/3F/4F）自由上下。
    fps.floorY = 0;
    // 初始位置：客厅中央，眼睛高度 = 地板（UNIT_LIFT）+ 1.6m
    const spawnPos = layout.characters?.Vivian?.startPos ?? layout.characters?.Nana?.startPos ?? [0, 0, 4];
    fps.setPosition(spawnPos[0], UNIT_LIFT + 1.8, spawnPos[2]);
    // 初始朝向：朝南（朝向房间深处）
    fps.setRotation(0, 0);
    camera.position.set(24, 20, 34);
    controls.target.set(0, 5, 7);
    controls.update();

    let colliders: Collider[] = [];

    // 观察者模式 WASD/方向键/Space/Shift 按键状态（仅非第一人称时生效）
    const obsKeys: Record<string, boolean> = {};
    const _obsDir = new THREE.Vector3();
    const _obsRight = new THREE.Vector3();
    // 飞行时 OrbitControls.target 允许活动的整座微缩场景范围（留余量）。
    // 公寓外壳 AABB ≈ [-31.15,-0.47,-8]~[38.25,14.05,8.28]，便利店到 z≈27；
    // 放宽到下面这个范围，既能飞到街区/便利店自由探索，又不会把模型拖丢。
    const OBS_TGT_MIN = new THREE.Vector3(-78, 0.2, -76);
    const OBS_TGT_MAX = new THREE.Vector3(78, 35, 81);

    // 门洞列表：决定 buildWallColliders 在哪些墙段挖开口。
    // 必须与观察者模式的 nav 栅格（navGrid.wallAABBs）保持同一套「可走洞口」口径——
    // 凡是「落地、非窗」的洞口（pass 拱门 / glass 落地玻璃门 / door / entryDoor）都该挖开，
    // 否则第一人称走到那里会被实心墙挡住，而观察者模式的宠物却走得过去（两个校验口径不一致）。
    const furnitureDoors = (layout.furniture as FurnitureSpec[])
      .filter((f) => f.kind === 'door' || f.kind === 'entryDoor')
      .map((f) => {
        const alongX = Math.abs(Math.cos(f.rot ?? 0)) > 0.707;
        return { x: f.pos[0], z: f.pos[2], width: f.size[0], alongX, kind: f.kind };
      });
    const wallDoorways = (
      layout.shell.walls as Array<{
        axis: 'x' | 'z';
        at: number;
        from: number;
        to: number;
        openings?: Array<{ kind: string; a: number; b: number; y0?: number; navCut?: 'east' | 'west' }>;
      }>
    ).flatMap((w) =>
      (w.openings ?? [])
        // 与 navGrid 同口径：落地（y0<0.1）且非窗才算可走洞口
        .filter((op) => op.kind !== 'window' && (op.y0 ?? 0) < 0.1)
        .map((op) => {
          const center = (op.a + op.b) / 2;
          // navCut：视觉开口取整段（a..b，门框渲染用），但可通过的门洞只切指定半边，
          // 另一半恒为实心——日式推拉门西半固定扇不可过，仅东半可推拉通过。
          const gapA = op.navCut === 'east' ? center : op.a;
          const gapB = op.navCut === 'west' ? center : op.b;
          return w.axis === 'z'
            ? { x: (gapA + gapB) / 2, z: w.at, width: gapB - gapA, alongX: true }
            : { x: w.at, z: (gapA + gapB) / 2, width: gapB - gapA, alongX: false };
        })
    );
    // 只挖「两侧都是房间」的洞口。通向室外的洞口（东墙 door-entry 是入户门、
    // 洞外就是楼道/虚空）必须保持实心，否则玩家能直接走出楼外；而通向阳台的
    // 落地玻璃门两侧分别是客厅/餐厅与 balcony 房间，照常放行。
    // 用「两侧是否落在房间内」判定，比按 exterior 标记过滤更可靠——南外墙既有
    // 通阳台的玻璃门（要挖），也有窗（不挖），单看 exterior 会一刀切错。
    const roomRects = layout.shell.rooms as Array<{ x0: number; x1: number; z0: number; z1: number }>;
    const insideRoom = (x: number, z: number) =>
      roomRects.some((r) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1);
    const doorways = [...furnitureDoors, ...wallDoorways].filter((d) => {
      // 入户门（entryDoor）朝公共外廊开，必须能进出：门外是挑出的外廊防滑地面 + 腰壁栏杆，
      // 不会掉楼。豁免「两侧都在房间内」过滤，否则北墙门洞被实心墙堵死、按 F 开了门也过不去。
      if ((d as { kind?: string }).kind === 'entryDoor') return true;
      const off = 0.3; // 探到墙两侧 30cm，跨过墙厚（12cm）
      return d.alongX
        ? insideRoom(d.x, d.z - off) && insideRoom(d.x, d.z + off)
        : insideRoom(d.x - off, d.z) && insideRoom(d.x + off, d.z);
    });

    // 门开关动画：构建循环里填充 leaf 引用，渲染循环按「路径穿过 + 靠近」驱动开合
    const doors: Array<{
      leaf: THREE.Group;
      leaf2?: THREE.Group;
      x: number;
      z: number;
      alongX: boolean;
      width: number;
      height: number;
      /** 门洞净高的上下沿（世界 y）。用于判定"人和门是不是同一层"—— */
      y0: number;
      y1: number;
      openAngle: number;
      current: number;
      slide: boolean;
      openOffset: number;
      openHold: number;
      heldTarget: number;
      manualOpen: boolean;
      cosR: number;
      sinR: number;
      collider: Collider;
      colliderFixed?: Collider; // 推拉门固定半扇盒（恒挡门洞一侧）
      colliderSlide?: Collider; // 推拉门动半扇盒（随开度滑移，让出另一侧）
    }> = [];

    // 第一人称门碰撞缓冲：渲染循环里复用，避免每帧 new 数组（GC 压力）
    const blockerBuf: Collider[] = [];

    /* 公寓南侧入口的自动感应门：开度 0（合拢）..1（全开）。
     * 与 doors 分开驱动——那道门没有可手动推的门扇（所以也没有 "F 打开" 提示），
     * 也不来自 layout，唯一的输入就是"门口有没有人"。 */
    let aptDoorOpen = 0;
    /** 两个门扇的碰撞盒：每帧随开度重写，预分配复用避免 GC */
    const aptDoorBlockers: Collider[] = [
      { min: new THREE.Vector3(), max: new THREE.Vector3() },
      { min: new THREE.Vector3(), max: new THREE.Vector3() },
    ];
    /** 门扇半宽 / 碰撞盒半厚：与 apartmentPodium 里 1.22 宽的门扇对应 */
    const APT_DOOR_HALF_W = 0.61;
    const APT_DOOR_HALF_T = 0.06;

    // 第一人称下，当前最近、且处于触发范围内的门（供 F 键开门 + "F 打开" 提示显示）。
    // 观察者模式下恒为 null。
    let nearDoor: typeof doors[number] | null = null;

    const setEnvironment = (nextPeriod: DayPeriod, nextWeather: WeatherKind) => {
      const profile = PERIODS[nextPeriod];
      const tuning = WEATHER_TUNING[nextWeather];
      const precipitation = nextWeather === 'drizzle' || nextWeather === 'storm';
      const wet = nextWeather === 'drizzle' || nextWeather === 'storm';
      const storm = nextWeather === 'storm';
      const tint = new THREE.Color(tuning.tint);
      const mixColor = (hex: string) => new THREE.Color(hex).lerp(tint, tuning.tintAmount);
      environmentRef.current = { period: nextPeriod, weather: nextWeather };
      scene.background = new THREE.Color(profile.background).lerp(tint, tuning.tintAmount * 0.38);
      scene.fog = new THREE.Fog(new THREE.Color(profile.fog).lerp(tint, tuning.tintAmount * 0.5), FX.fog?.near ?? sceneRadius * 2.0, FX.fog?.far ?? sceneRadius * 6.5);
      renderer.toneMappingExposure = profile.exposure * tuning.exposure;
      scene.environmentIntensity = (nextPeriod === 'night' ? 0.20 : nextPeriod === 'dusk' ? 0.28 : 0.36) * tuning.exposure;
      ambient.color.copy(mixColor(profile.ambient));
      hemi.color.copy(mixColor(profile.hemiSky));
      hemi.groundColor.copy(mixColor(profile.hemiGround));
      fill.color.copy(mixColor(profile.fill));
      rim.color.copy(mixColor(profile.rim));
      key.color.copy(mixColor(profile.key));
      key.intensity = L.key.intensity * profile.keyMul * tuning.keyMul;
      fill.intensity = L.fill.intensity * profile.fillMul * tuning.fillMul;
      rim.intensity = L.rim.intensity * profile.rimMul * tuning.rimMul;
      if (rainObj) rainObj.visible = precipitation;
      if (snowObj) snowObj.visible = nextWeather === 'snow';
      for (const o of weatherFxObjects) o.visible = wet && o !== rainObj;
      if (wetGroundObj) wetGroundObj.visible = wet;
      if (rainObj) rainObj.scale.setScalar(storm ? 1.18 : 0.78);
      if (rainObj) rainObj.userData.weatherIntensity = storm ? '暴雨' : '小雨';
      if (snowObj) snowObj.userData.weatherIntensity = nextWeather === 'snow' ? '降雪' : '隐藏';
      districtArt.setEnvironment(nextPeriod, nextWeather);
      // HUD 摘要走 ref 直接改 DOM：环境每变一次才动一次，没必要为此重渲染整棵子树。
      if (hudEnvRef.current) {
        hudEnvRef.current.textContent = describeEnvironment(environmentRef.current, environmentSourceRef.current);
      }
    };
    environmentApplyRef.current = setEnvironment;
    /**
     * 测试注入口。传 input 片段（如 { hour: 12, weatherCode: 95 }）即切换环境，
     * 传 null 恢复跟随真实世界感知。
     *
     * 这里给的是感知层的 input 而不是最终的 period/weather —— 让无头验证脚本
     * 和真实运行走同一条映射路径，映射本身才被测得到。
     */
    (window as any).__ROOM__.setEnvironmentSource = (src: Partial<EnvironmentInput> | null) => {
      environmentOverrideRef.current = src;
      environmentTickRef.current?.();
    };

    // 构建碰撞体列表（墙体 + 栏杆 + nav 家具），全部由 dormLayout.json 驱动。
    // 与 navGrid 同源：nav 走得过去的地方，第一人称也必须走得过去。
    colliders = [
      ...buildFurnitureColliders(layout.furniture),
      // 墙体只认 shell.walls（真正建了墙面的那几面），门洞处开口。
      // 不能用 shell.rooms 的矩形边界推——开放式 LDK 的房间分界线上没有墙，
      // 按房间推会造出看不见却过不去的「空气墙」。
      ...buildWallColliders(
        layout.shell.walls,
        layout.room.height,
        layout.shell.wallThickness,
        doorways,
        layout.shell.railing
      ),
    ];
    // 碰撞盒生成器以地板 y=0 为基准，整户（203 室）抬高后垂直方向全部偏移
    // （2D 水平碰撞不受影响，但跳跃/蹲起的 3D 检测要用对的 y 区间）
    for (const c of colliders) {
      c.min.y += UNIT_LIFT;
      c.max.y += UNIT_LIFT;
    }

    /* ---- 地图级结构碰撞体（世界坐标，不随整户抬高）----
     * 让第一人称能走出 203 室、在街道/便利店/公寓各层外廊与折返平台间自由行动，
     * 并经由东端外置钢楼梯（斜坡）上下 2F/3F/4F，而不会掉进虚空或被单一地平线卡死。
     *   - floor：可站立薄板（街道地面 / 各层楼板 / 北侧外廊 / 楼梯平台），只作落地支撑
     *   - ramp ：三跑外楼梯，用 heightAt 取 (x,z) 处表面高度，平滑无抖动
     * 这些与 unitColliders 拼接后一起传给 fps.update；楼梯/平台网格已在 exterior.ts
     * 标了 noCollide，不会和这里的 floor/ramp 盒重复成空气墙。 */
    const worldColliders: Collider[] = [];
    const addFloor = (x0: number, x1: number, z0: number, z1: number, y: number, src: string) => {
      worldColliders.push({
        min: new THREE.Vector3(x0, y - 0.02, z0),
        max: new THREE.Vector3(x1, y + 0.02, z1),
        kind: 'floor',
        source: src,
      });
    };
    // 街道 / 便利店地面（一层，世界 y=0）——也作为「掉出楼板后的兜底落点」
    addFloor(-80, 80, -78, 83, 0, 'street');
    // 公寓一楼（地面层）主体楼板：楼体 footprint 正下方的正式地板，堵死
    // 「穿透二楼后落到 street 板、又被公寓外壳围墙关在一楼盒子里」的陷阱。
    addFloor(APT_X0, APT_X1, APT_ZN, APT_ZB, 0, 'apt-floor-0');
    // 公寓各层：主体楼板（覆盖 203 等室内）+ 北侧外廊（挑出 1.25m）+ 东端三块楼梯平台
    for (const lvl of FLOORS) {
      addFloor(APT_X0, APT_X1, APT_ZN, APT_ZB, lvl, `apt-floor-${lvl}`);          // 主体楼板
      addFloor(APT_X0, STRS.lx0, APT_CORRIDOR_N, APT_ZN, lvl, `corridor-${lvl}`); // 北侧外廊
      addFloor(STRS.lx0, STRS.lx1, STRS.za0, STRS.zb1, lvl, `wp-${lvl}`);         // 西平台
      addFloor(STRS.lx1, STRS.wx1, STRS.zb0, STRS.zb1, lvl, `walk-${lvl}`);       // 步行廊
      addFloor(STRS.wx1, STRS.ex1, STRS.za0, STRS.zb1, lvl, `tp-${lvl}`);         // 折返平台
    }
    // 斜坡：三跑外楼梯（heightAt 取表面高度；不阻挡水平/垂直移动）
    const addRamp = (
      xBot: number, yBot: number, xTop: number, yTop: number, z0: number, z1: number, src: string
    ) => {
      const dx = xBot - xTop;
      worldColliders.push({
        min: new THREE.Vector3(xTop, yBot, z0),
        max: new THREE.Vector3(xBot, yTop, z1),
        kind: 'ramp',
        source: src,
        heightAt: (x: number, z: number) => {
          if (z < z0 || z > z1) return null;
          if (x < xTop || x > xBot) return null;
          const t = (xBot - x) / dx; // 1 在 xBot，0 在 xTop
          return yBot + (yTop - yBot) * t;
        },
      });
    };
    addRamp(STRS.f1Bot, 0, STRS.lx1, FLOORS[0], STRS.za0, STRS.za1, 'stair-1'); // 地面 → 2F
    addRamp(STRS.wx1, FLOORS[0], STRS.lx1, FLOORS[1], STRS.za0, STRS.za1, 'stair-2'); // 2F → 3F
    addRamp(STRS.wx1, FLOORS[1], STRS.lx1, FLOORS[2], STRS.za0, STRS.za1, 'stair-3'); // 3F → 4F

    // 第一人称碰撞体 = 单元内（墙/家具，已抬高）+ 地图级结构（地板/斜坡）
    // 用 let：公寓外壳建好后再追加楼梯/平台/外廊栏杆盒（见下方 buildApartmentShell 之后）
    let fpsColliders = colliders.concat(worldColliders);

    // 第一人称落点：吸附到脚下最近的合法楼层楼板，绝不悬空 / 掉楼。
    // 存档坐标只有在「脚下有二楼及以上支撑」时才恢复（保留楼层书签），
    // 否则强制回落二楼出生点；出生点也先吸附到二楼楼板顶面，避免卡在楼层夹层。
    const STEP_UP = 0.45;
    const snapAt = (x: number, z: number, footRef: number): number | null => {
      let best: number | null = null;
      for (const c of fpsColliders) {
        if (c.kind !== 'floor' && c.kind !== 'ramp') continue;
        const cx = Math.max(c.min.x, Math.min(x, c.max.x));
        const cz = Math.max(c.min.z, Math.min(z, c.max.z));
        const dx = x - cx, dz = z - cz;
        if (dx * dx + dz * dz > 0.0225) continue;
        let top: number | null = null;
        if (c.kind === 'ramp') { const h = c.heightAt?.(x, z); if (h == null) continue; top = h; }
        else top = c.max.y;
        if (top <= footRef + STEP_UP && (best == null || top > best)) best = top;
      }
      return best;
    };

    // 观察者模式 ↔ 第一人称。默认观察者，按 Enter 进入第一人称——
    // 观察者模式需要鼠标旋转/缩放，不能用点击画面（会跟 OrbitControls 冲突），
    // 键盘事件同样满足 requestPointerLock 的用户手势要求。
    // （isFirstPerson 已在上方投影计算处声明；进入/退出第一人称时切换并复算视野）
    fps.init();
    fps.onLock = () => {
      if (!alive) return;
      isFirstPerson = true;
      /* 近裁面先复位到区间下限：第一帧还没跑过 fps.update，用观察者的 0.1 会
       * 在出生点贴墙的情况下切一帧。之后由 FPSControls 每帧自适应放大。
       * 刻意不放进 applyProjection —— 那样窗口 resize 也会把 near 打回下限，
       * 明明是站在空地上却要白丢一段深度精度。 */
      camera.near = FPS_NEAR_MIN;
      applyProjection(container.clientWidth, container.clientHeight); // 第一人称切广角
      controls.enabled = false;
      // 吸附到脚下最近合法楼层；存档只在脚下有二楼层支撑时才恢复，否则回落出生点。
      const eye = 1.6;
      let placed = false;
      if (savedFpsState) {
        const g = snapAt(savedFpsState.x, savedFpsState.z, savedFpsState.y - eye);
        if (g != null && g >= -0.1) {
          fps.setPosition(savedFpsState.x, g + eye, savedFpsState.z);
          fps.setRotation(savedFpsState.yaw, savedFpsState.pitch);
          placed = true;
        }
      }
      if (!placed) {
        const g = snapAt(spawnPos[0], spawnPos[2], UNIT_LIFT);
        // 首次进入（无存档）：恒落在二楼楼板（UNIT_LIFT）。即使 snapAt 因碰撞体
        // 尚未就绪返回了更低楼层，也强制抬到二楼，杜绝"出生即掉到一楼"。
        const floorY = g == null ? UNIT_LIFT : Math.max(g, UNIT_LIFT);
        fps.setPosition(spawnPos[0], floorY + eye, spawnPos[2]);
        fps.setRotation(0, 0);

      }
      setMode('firstPerson');
      // ESC 一律关闭公寓窗口（看护线程在指针锁定下也能收到），不只是退出第一人称
      if (hudModeRef.current) hudModeRef.current.textContent = '第一人称 · ESC 退出公寓';
      if (hudCrosshairRef.current) hudCrosshairRef.current.style.opacity = '1';
    };
    fps.onUnlock = () => {
      if (!alive) return;
      // 退出第一人称：先记录当前位置 + 面朝方向，下次进入直接加载（地图内自由活动后的"书签"）
      const p = fps.getPosition();
      const r = fps.getRotation();
      savedFpsState = { x: p.x, y: p.y, z: p.z, yaw: r.yaw, pitch: r.pitch };
      // 意外解锁（ESC 退出锁定 / Alt+Tab / 锁定请求失败）→ 回到观察者模式，
      // 并复位到观察者初始构图（全景概览），避免停在第一人称的房间内部角度。
      isFirstPerson = false;
      camera.near = FPS_NEAR_MAX; // 观察者模式距离目标 ≥3.5m，恢复常规近裁面
      applyProjection(container.clientWidth, container.clientHeight); // 观察者退回原视野
      controls.enabled = true;
      // 退出第一人称时清掉飞行按键状态，避免观察者在 FPS 期间按住的键"卡住"继续飞
      for (const k in obsKeys) obsKeys[k] = false;
      camera.position.set(cam.position[0], cam.position[1] + UNIT_LIFT, cam.position[2]);
      controls.target.set(cam.target[0], cam.target[1] + UNIT_LIFT, cam.target[2]);
      controls.update();
      setMode('observe');
      if (hudModeRef.current) hudModeRef.current.textContent = '观察者模式 · WASD飞行 / Space升 Shift降 / Enter进入第一人称';
      if (hudCrosshairRef.current) hudCrosshairRef.current.style.opacity = '0';
    };

    // Enter 键：观察者模式下进入第一人称
    const onEnterKey = (e: KeyboardEvent) => {
      if (e.key !== 'Enter' || isFirstPerson) return;
      // 命令面板输入框里的回车是执行命令，不是进入第一人称。
      // 除了 ref 判定还要看事件源：命令成功会同步关面板、cmdOpenRef 提前翻 false，
      // 这条 keydown 冒泡到 window 时 ref 已不可信，按 DOM 源最稳。
      const t = e.target as HTMLElement | null;
      if (t && t.tagName === 'INPUT') return;
      if (cmdOpenRef.current) return; // 命令面板打开时 Enter 是执行命令，不进第一人称
      e.preventDefault();
      fps.requestLock();
    };
    window.addEventListener('keydown', onEnterKey);

    // 观察者模式飞行：WASD/方向键 水平飞行，Space 上升、Shift 下降。
    // 仅非第一人称时响应——第一人称下这些键交给 FPSControls 处理，互不冲突。
    const onObsKeyDown = (e: KeyboardEvent) => {
      if (isFirstPerson) return;
      if (cmdOpenRef.current) return; // 命令面板输入时这些键是打字，不触发飞行
      const k = e.key.toLowerCase();
      if (k === 'w' || k === 'a' || k === 's' || k === 'd' ||
          k === 'arrowup' || k === 'arrowdown' || k === 'arrowleft' || k === 'arrowright' ||
          k === ' ' || k === 'shift') {
        obsKeys[k === ' ' ? 'space' : k] = true;
        // 空格/方向键默认会滚动页面，吃掉避免观察者飞行时页面跟着滚
        if (k === ' ' || k.startsWith('arrow')) e.preventDefault();
      }
    };
    const onObsKeyUp = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      obsKeys[k === ' ' ? 'space' : k] = false;
    };
    window.addEventListener('keydown', onObsKeyDown);
    window.addEventListener('keyup', onObsKeyUp);

    /* ---------------- Lighting ---------------- */

    const L = layout.lighting;
    const ambient = new THREE.AmbientLight(new THREE.Color(L.ambient.color), L.ambient.intensity);
    scene.add(ambient);
    const hemi = new THREE.HemisphereLight(new THREE.Color(L.hemi.sky), new THREE.Color(L.hemi.ground), L.hemi.intensity);
    scene.add(hemi);

    const key = new THREE.DirectionalLight(new THREE.Color(L.key.color), L.key.intensity);
    key.position.set(-18, 32, 16);
    key.castShadow = true;
    // 软阴影下 1024 对十几米的场景已够（~1cm/texel），2048 显存和阴影 pass 都翻倍
    key.shadow.mapSize.set(2048, 2048);
    // 正交阴影相机罩住套房的包围球（留 5% 余量），户型再怎么扩建都不会漏
    const shadowR = 34;
    key.shadow.camera.near = 0.5;
    key.shadow.camera.far = 110;
    key.shadow.camera.left = -shadowR;
    key.shadow.camera.right = shadowR;
    key.shadow.camera.top = shadowR;
    key.shadow.camera.bottom = -shadowR;
    key.shadow.bias = -0.0006;
    key.shadow.normalBias = 0.022;
    // 主光默认 target 在原点——整户抬高后阴影正交框会偏到楼下。
    // 显式指到单元中心，阴影范围始终罩住 203 室本体。
    key.target.position.set((B.x0 + B.x1) / 2, UNIT_LIFT + 1.0, (B.z0 + B.z1) / 2);
    scene.add(key.target);
    /**
     * 阴影不再逐帧重画。太阳不动、家具全冻结，唯一会变的只有两个角色的影子。
     * 改成"角色的水平位移超过阈值才重画一次"：站着不动时阴影 pass 完全跳过，
     * 走动时才按需刷新。角色只占画面很小一块，隔一帧刷一次的延迟看不出来。
     */
    key.shadow.autoUpdate = false;
    key.shadow.needsUpdate = true;
    scene.add(key);

    const fill = new THREE.DirectionalLight(new THREE.Color(L.fill.color), L.fill.intensity);
    fill.position.set(L.fill.position[0], L.fill.position[1], L.fill.position[2]);
    scene.add(fill);

    // 逆光勾边：从窗口方向打过来，给角色和家具边缘补一道暖色轮廓
    const rim = new THREE.DirectionalLight(new THREE.Color(L.rim.color), L.rim.intensity);
    rim.position.set(L.rim.position[0], L.rim.position[1], L.rim.position[2]);
    scene.add(rim);

    /* ---------------- 世界地面（室外层 Stage 1） ----------------
     *
     * 微缩底座已拆：房间不再摆在展示底座上，而是站在雨夜街区里。
     * 地面只有一张（湿沥青，170m 见方，边缘被雾吞掉），居中跟着房间
     * 包围盒走——户型一旦扩建，地面不会一侧贴墙、另一侧空出一块。
     */
    const districtArt = createDistrictArt(scene);
    fpsColliders = fpsColliders.concat(districtArt.colliders);
    /* 近景那一排（公寓正对面）单独挂描边并冻结。
     *
     * 街区整体是刻意不描边的（远景描边会在雾里变成网格，且 48 栋的壳太贵），
     * 但只描最近这 4 栋就能把"隔着一条街的楼是纯色块、身后公寓有清晰线稿"
     * 这个质感断点接上。必须在 createDistrictArt 内部合批之后做——描边要从
     * 合并后的少数几个 mesh 上长出来，逐 mesh 描会退化成几千个壳。
     */
    outlineProp(districtArt.frontage);
    freezeStatic(districtArt.frontage);
    const exteriorGround = buildExteriorGround();
    districtArt.prepare(exteriorGround);
    exteriorGround.position.set((B.x0 + B.x1) / 2, 0, (B.z0 + B.z1) / 2);
    scene.add(exteriorGround);
    freezeStatic(exteriorGround);

    /* ---------------- 近景街道层（室外层 Stage 2） ----------------
     *
     * 路灯 + 街道湿地 + 停车场。全部用世界绝对坐标，不跟房间包围盒居中
     * ——户型扩建时街道不该跟着挪。整层走标准装配：合批 → 描边 → add →
     * 冻结；不进 FPS 碰撞（阳台栏杆拦着，玩家出不去）。
     */
    const streetscape = buildStreetscape();

    // 合批前收集碰撞：合批会把子组合并进大 mesh、丢掉其 sceneCollideSkip 标记，
    // 且合并出的大 mesh 直接挂根下会被收成横跨整条街的巨型盒。这里先收，只对路灯
    // 这类该挡人的实体生成碰撞盒。
    const streetSceneColliders = buildSceneColliders(streetscape);
    districtArt.prepare(streetscape);
    mergeByMaterial(streetscape);
    outlineProp(streetscape);
    scene.add(streetscape);
    freezeStatic(streetscape);

    /* ---------------- 公寓楼外壳（203 所在的这栋三层） ----------------
     *
     * 地面层（101-103 门脸 + 楼道入口 + 贩卖机 + 自行车）、同层邻居 202/203、
     * 三楼 301-303、屋顶。203 那一段的外皮是房间自己的 shell，这里不重复建。
     *
     * 用世界绝对坐标，挂 scene 而不是 unitGroup —— 它包含楼下和邻居，
     * 只有 203 那一户才跟着 UNIT_LIFT 抬高。走标准装配，不进 FPS 碰撞。
     */
    // 公寓楼全部走声明式碰撞：楼体四壁（APT_WALL_BOXES）与阳台栏杆 / 外廊腰壁 /
    // 外置楼梯栏杆 / 自行车（shell.boxes）拼成同一张表，统一交给 buildBoxColliders。
    // 好处是不再有「按 userData 标记遍历场景」的专用收集器，也不必赶在合批前收集。
    const apartmentShell = buildApartmentShell();
    fpsColliders = fpsColliders.concat(
      buildBoxColliders([...APT_WALL_BOXES, ...apartmentShell.boxes])
    );
    districtArt.prepare(apartmentShell.group);
    mergeByMaterial(apartmentShell.group);
    outlineProp(apartmentShell.group);
    scene.add(apartmentShell.group);
    freezeStatic(apartmentShell.group);
    /* 南侧入口的自动感应门。挂在 shell 树之外是必须的：外壳整体走
     * mergeByMaterial + freezeStatic，门扇进去就被合批焊死、矩阵也不再更新。
     * 这里只 add，不描边（门厅其他构件一律 noOutline，保持一致）。 */
    const aptAutoDoor = apartmentShell.autoDoor;
    scene.add(aptAutoDoor.group);
    const apartmentLift=createApartmentLift(container,camera,fps);
    fpsColliders=fpsColliders.concat(apartmentLift.colliders);
    districtArt.prepare(apartmentLift.group);
    mergeByMaterial(apartmentLift.group);
    scene.add(apartmentLift.group,apartmentLift.dynamic);
    freezeStatic(apartmentLift.group);
    (window as any).__ROOM__.lift=apartmentLift;
    (window as any).__ROOM__.fps=fps;


    /* ---------------- 湿地涟漪 + 屋檐滴水（动效层，不得冻结） ----------------
     *
     * 这两层每帧改 scale/opacity，必须加进未冻结的组——freezeStatic 会把
     * matrixAutoUpdate 关掉，挂进公寓/街道组就定格不动了。所以它们各自返回
     * { object, update }，单独挂 scene，更新收进 sceneFx 统一驱动。
     */
    const sceneFx: Array<(t: number) => void> = [];
    const streetRipples = buildStreetscapeRipples();
    scene.add(streetRipples.object);
    sceneFx.push(streetRipples.update);
    const aptDrips = buildApartmentEaveDrips();
    scene.add(aptDrips.object);
    sceneFx.push(aptDrips.update);
    const weatherFxObjects: THREE.Object3D[] = [streetRipples.object, aptDrips.object];

    /* ---------------- 街角便利店（街对过，近景主体） ----------------
     *
     * 便利店是 203 阳台和客厅的主景观：店门在 z14，距阳台栏杆 7.7m。
     * 静态部分走标准装配；动效部分（招牌灯箱 / 自动门 / 红绿灯）单独挂在
     * scene 下，不能合批（材质每帧改）也不能冻结（门每帧滑）。
     */
    const store = buildConvenienceStore();
    /* 碰撞表三路拼：
     *   1) 遍历 store.group —— 合批前收，货架/吧台/冷饮柜/墙裙/竖框/角柱这类实体各自
     *      还是独立小 mesh（合批之后就并成跨整店的巨盒了，必须赶在这之前）。
     *      ⚠️ 「自动门玻璃门扇透明，遍历里也自然跳过」这句只对玻璃成立：门扇上的
     *      金属竖杆/横杆是不透明的，遍历会照样收，所以它们各自标了 noCollide
     *      （见 mkLeaf），门扇的碰撞一律由第 3 路给。
     *   2) 遍历 store.dynamic —— 动效件（招牌灯箱 / 门头灯箱 / 红绿灯）的实体盒。
     *   3) 显式声明 + 门扇动态盒 —— 橱窗玻璃是 opacity 0.075 的透明材质，遍历整片放过，
     *      必须由 store.boxes 声明；门扇盒随门滑动，必须每帧更新，两者都不能靠遍历。 */
    const storeSceneColliders = buildSceneColliders(store.group)
      .concat(buildSceneColliders(store.dynamic))
      .concat(buildBoxColliders(store.boxes))
      .concat(store.doorColliders);
    districtArt.prepare(store.group);
    mergeByMaterial(store.group);
    outlineProp(store.group);
    scene.add(store.group);
    freezeStatic(store.group);
    scene.add(store.dynamic);

    /* ---------------- 近景城市节点（地铁 / 居酒屋 / 小公园） ---------------- */
    const subwayEntrance = buildSubwayEntrance();
    districtArt.prepare(subwayEntrance);
    mergeByMaterial(subwayEntrance);
    outlineProp(subwayEntrance);
    scene.add(subwayEntrance);
    freezeStatic(subwayEntrance);

    /* 居酒屋：碰撞随几何一并交出（声明式，与便利店 / 书店同一条路）。
     * 整组标了 sceneCollideSkip，遍历一个盒都不收；而且北立面是「木格栅 + 门」，
     * 格栅缝 0.44m 比玩家直径 0.30 宽，靠遍历也挡不住 —— 必须由 izakaya.boxes 声明。
     * 北立面已按门洞拆成「西 / 东 / 门楣」三条，门洞净空可进（原来整面封死，
     * 用户报「居酒屋没有门无法进入」）。
     * 第 3 路 doorColliders：门扇的动态盒，随门滑动，必须每帧由 izakaya.update() 同步。 */
    const izakaya = buildIzakaya();
    fpsColliders = fpsColliders.concat(buildBoxColliders(izakaya.boxes))
                              .concat(izakaya.doorColliders);
    districtArt.prepare(izakaya.group);
    mergeByMaterial(izakaya.group);
    outlineProp(izakaya.group);
    scene.add(izakaya.group);
    freezeStatic(izakaya.group);
    // 感应门扇：每帧滑，既不合批也不冻结，单独挂
    scene.add(izakaya.dynamic);

    const smallPark = buildSmallPark();
    districtArt.prepare(smallPark);
    mergeByMaterial(smallPark);
    outlineProp(smallPark);
    scene.add(smallPark);
    freezeStatic(smallPark);

    /* ---------------- 近景高模临街书店（学园都市式学生街排面） ----------------
     *
     * 1F 是可进入的真营业厅，碰撞盒随几何一并交出（声明式，与公寓楼同一条路）。
     * 不能走 buildSceneColliders 遍历：橱窗那片玻璃是 0.085 透明度的近全透明
     * 材质，遍历按「transparent && opacity < 0.5」整块跳过，玩家就能从橱窗穿进去。
     */
    const bookStore = buildStreetBookStore();
    fpsColliders = fpsColliders.concat(buildBoxColliders(bookStore.boxes));
    districtArt.prepare(bookStore.group);
    mergeByMaterial(bookStore.group);
    outlineProp(bookStore.group);
    scene.add(bookStore.group);
    freezeStatic(bookStore.group);

    /* ---------------- 中景商住混合街区 ---------------- */
    // The authored district replaces the old repeated mid-rise, mall and viaduct masses.

    // 城市基础设施：电线杆+架空电线 / 街道设施（售货机·快递柜·路牌）
    const utilityPoles = buildUtilityPoles();
    districtArt.prepare(utilityPoles);
    mergeByMaterial(utilityPoles);
    scene.add(utilityPoles);
    freezeStatic(utilityPoles);

    /* 街道设施（售货机 / 快递柜 / 路牌）：柜体的碰撞随几何一并交出（声明式，
     * 与便利店 / 书店 / 居酒屋同一条路）。整组标了 sceneCollideSkip，遍历一个盒
     * 都不收，2.0m 高的售货机与快递柜原来能直接穿过去。
     * 路牌柱 / 公交牌柱 / 条凳**刻意不在 boxes 里**（细杆与可跨条凳，见 exterior.ts
     * buildStreetFurniture 的注释）。 */
    const streetFurniture = buildStreetFurniture();
    fpsColliders = fpsColliders.concat(buildBoxColliders(streetFurniture.boxes));
    districtArt.prepare(streetFurniture.group);
    mergeByMaterial(streetFurniture.group);
    scene.add(streetFurniture.group);
    freezeStatic(streetFurniture.group);

    /* ---------------- 单元组（203 室，整户抬高到二楼） ----------------
     *
     * 这户是公寓楼的一个单元：shell/家具/角色/阳台湿地/浮尘全部挂进
     * unitGroup，组本身抬高 UNIT_LIFT。楼下地面层和邻居外墙归 exterior.ts
     * 管，世界地面留在 y=0。组创建后立刻手动算一次矩阵——后面每个子树的
     * freezeStatic 都会用父级 matrixWorld 烘焙，不先算准就会冻出少 3.4m
     * 的错位。
     */
    const unitGroup = new THREE.Group();
    unitGroup.name = 'unit-203';
    unitGroup.position.y = UNIT_LIFT;
    scene.add(unitGroup);
    unitGroup.updateMatrixWorld(true);

    /* ---------------- Room shell ---------------- */

    /**
     * 外壳 v2：多房间 + 墙段 + 洞口。每个墙侧登记一个 decor 组，
     * 相机绕到那一侧的墙外时，挂在该墙上的厚装饰（窗框窗帘、门扇、玻璃滑门框）
     * 跟着墙一起让开——墙平面本身靠 FrontSide 背面剔除自动消失。
     */
    const interiorDesign = createInteriorDesign();
    const shell = buildRoomShell(layout);
    console.log('[room] shell 构建完成, wallSides:', shell.wallSides.length, 'wallMeshes:', shell.wallMeshes.length, 'ceilingMeshes:', shell.ceilingMeshes.length);
    /** 观察者模式：墙板/天花板/墙装饰 视线剔除 */
    const wallMeshes = shell.wallMeshes;
    const ceilingMeshes = shell.ceilingMeshes;
    /**
     * 装饰组不参与整体合并：它们是空壳，家具随后才按墙侧挂进去。
     * 标 noMerge 让合并只处理墙/地板/天花/栏杆本体。
     *
     * 洞口断面同理要躲开整体合并，但理由不同：所有断面共用一份 revealMat，
     * 一旦参与整体合并就会**跨墙压成一个 mesh**，观察者模式再也没法按墙剔除它
     * ——墙隐了、窗洞那一圈断面还框在半空（看着像窗户的外侧轮廓没跟着消失）。
     * 躲开之后再按墙侧各自合一次，合并后每侧只剩 1 个 mesh，剔除能力也保住了。
     */
    for (const ws of shell.wallSides) {
      ws.decor.userData.noMerge = true;
      ws.roomSideDecor.userData.noMerge = true;
      ws.reveals.userData.noMerge = true;
    }
    shell.interiorReveals.userData.noMerge = true;
    districtArt.prepare(shell.group);
    interiorDesign.prepareShell(shell);
    mergeByMaterial(shell.group);
    // noMerge 只对"父级发起的合批"生效，拿它当 root 单独调仍然照常合并
    for (const ws of shell.wallSides) mergeByMaterial(ws.reveals);
    mergeByMaterial(shell.interiorReveals);
    // 房间外壳整体不参与遍历补碰撞：地板/天花板 mesh 若被遍历收进去，会在地面高度生成
    // 水平阻挡盒把玩家钉死在地板；墙体碰撞只认 buildWallColliders（门洞在 layout 层挖）。
    shell.group.userData.sceneCollideSkip = true;
    unitGroup.add(shell.group);
    const wallDecor = new Map<string, THREE.Group>();
    const roomSideDecor = new Map<string, THREE.Group>();
    for (const ws of shell.wallSides) {
      wallDecor.set(ws.id, ws.decor);
      roomSideDecor.set(ws.id, ws.roomSideDecor);
    }

    /* ---------------- Furniture ---------------- */

    // 已被 buildFurnitureColliders 覆盖的家具 id（nav!==false 且有正尺寸）。
    // 这些组的 mesh 标 furnitureRoot=true，遍历补碰撞时跳过，避免重复成更大盒子造成夹点；
    // 门/窗/地毯/浴室（nav:false）以及 lifestyle/落地灯等无尺寸摆件不标，落到遍历里补碰撞。
    const navFurnitureIds = new Set(
      (layout.furniture as FurnitureSpec[])
        .filter((f) => f.nav !== false && Array.isArray(f.size) && (f.size as number[])[0] > 0 && (f.size as number[])[2] > 0)
        .map((f) => f.id)
    );

    const blenderSlots: FurnitureSlot[] = [];
    console.log('[room] 开始构建家具, 总数:', (layout.furniture as FurnitureSpec[]).length);
    for (const it of layout.furniture as FurnitureSpec[]) {
      try {
      const build = FURNITURE_BUILDERS[it.kind];
      if (!build) {
        console.warn(`[room] 未知道具类型 kind=${it.kind}（id=${it.id}），跳过`);
        continue;
      }
      const obj = ['jpRug', 'jpDiningRug'].includes(it.kind) ? interiorDesign.buildRug(it.size) : build(it);
      interiorDesign.prepareFurniture(obj, it.id);
      obj.userData.roomFurnitureId = it.id;
      if (blenderFurnitureIds.has(it.id) && !['jpRug', 'jpDiningRug'].includes(it.kind)) blenderSlots.push({ root: obj, spec: it });
      // 标 furnitureRoot 的家具组在 buildSceneColliders 遍历里被跳过（碰撞已由 layout 尺寸盒覆盖）。
      // 仅「有尺寸的 nav 家具 + 门/窗/地毯/浴室」标 true；lifestyle/落地灯等无尺寸摆件留 false，
      // 让遍历给它们补实体碰撞。
      obj.userData.furnitureRoot =
        navFurnitureIds.has(it.id) ||
        it.kind === 'door' || it.kind === 'entryDoor' || it.kind === 'glassDoor' || it.kind === 'fusuma' ||
        it.kind === 'window' || it.kind === 'rug' || it.kind === 'bath' || it.kind === 'toilet' ||
        it.kind === 'jpRug' || it.kind === 'jpDiningRug' || it.kind === 'jpZabuton' ||
        it.kind === 'jpPlant' || it.kind === 'jpFloorClutter' || it.kind === 'jpSlatWall' ||
        it.kind === 'jpKitchenShelf' || it.kind === 'jpPendant' ||
        it.kind === 'jpBath' || it.kind === 'jpToilet' || it.kind === 'jpPlanter';
      obj.position.set(it.pos[0], it.pos[1], it.pos[2]);
      // rot：布局里声明的朝向修正（比如桌前椅子要背对桌沿、面向桌子）
      if (typeof it.rot === 'number') obj.rotation.y = it.rot;

      // 门：不做合批/冻结——门扇要独立旋转做开关动画，合批会把门扇并进静态
      // mesh、冻结会锁死 matrix。门框（door-frame）静态描边，门扇（door-leaf）描边壳
      // 挂 leaf 下跟随旋转。
      if (it.kind === 'door' || it.kind === 'entryDoor' || it.kind === 'fusuma' || it.kind === 'glassDoor') {
        let frame: THREE.Group | null = null;
        let leaf: THREE.Group | null = null;
        let leaf2: THREE.Group | null = null;
        obj.traverse((o) => {
          if (!frame && o.name === 'door-frame') frame = o as THREE.Group;
          if (!leaf && o.name === 'door-leaf') leaf = o as THREE.Group;
          if (!leaf2 && o.name === 'door-leaf2') leaf2 = o as THREE.Group;
        });
        // 描边在 add 之前：此时 obj 还没挂进 scene，matrixWorld == 本地矩阵，烘焙最稳
        if (frame) outlineProp(frame);
        if (leaf) outlineProp(leaf);
        if (leaf2) outlineProp(leaf2);
        const decorGroup = it.wall ? wallDecor.get(it.wall) : undefined;
        (decorGroup ?? unitGroup).add(obj);
        if (leaf) {
          const rot = it.rot ?? 0;
          const cosR = Math.cos(rot);
          const sinR = Math.sin(rot);
          const alongX = Math.abs(cosR) > 0.707;
          const w = it.size[0];
          const h = it.size[1];
          const halfT = 0.05;
          const slide = it.kind === 'fusuma' || it.kind === 'glassDoor';
          const y0 = UNIT_LIFT + (it.pos[1] ?? 0);
          const y1 = y0 + h;
          // 推拉门：固定半扇（leaf）恒挡门洞一侧，动半扇（leaf2）随开度滑向固定半扇、让出另一侧。
          // 两半扇盒始终参与碰撞——关门时合盖整门洞、开门时仅留固定半扇那侧挡，被推开的半边才放行。
          // 盒的世界半边由门朝向（rot）决定：局部 +x 轴世界投影 = (cosR, 0, -sinR)，固定半扇局部中心 x=-w/4。
          let colliderFixed: Collider | undefined;
          let colliderSlide: Collider | undefined;
          if (slide) {
            if (alongX) {
              const fx = it.pos[0] + cosR * (-w / 4);
              colliderFixed = { min: new THREE.Vector3(fx - w / 4, y0, it.pos[2] - halfT), max: new THREE.Vector3(fx + w / 4, y1, it.pos[2] + halfT) };
              const sx0 = it.pos[0] + cosR * (w / 4); // current=0：动半扇在右半
              colliderSlide = { min: new THREE.Vector3(sx0 - w / 4, y0, it.pos[2] - halfT), max: new THREE.Vector3(sx0 + w / 4, y1, it.pos[2] + halfT) };
            } else {
              const fz = it.pos[2] - sinR * (-w / 4);
              colliderFixed = { min: new THREE.Vector3(it.pos[0] - halfT, y0, fz - w / 4), max: new THREE.Vector3(it.pos[0] + halfT, y1, fz + w / 4) };
              const sz0 = it.pos[2] - sinR * (w / 4); // current=0：动半扇在右半
              colliderSlide = { min: new THREE.Vector3(it.pos[0] - halfT, y0, sz0 - w / 4), max: new THREE.Vector3(it.pos[0] + halfT, y1, sz0 + w / 4) };
            }
          }
          doors.push({
            leaf,
            leaf2: leaf2 ?? undefined,
            x: it.pos[0],
            z: it.pos[2],
            alongX,
            width: w,
            height: h,
            y0,
            y1,
            openAngle: -1.82,
            current: 0,
            slide,
            openOffset: slide ? w / 2 : 0,
            openHold: 0,
            heldTarget: 0,
            manualOpen: false,
            cosR,
            sinR,
            collider: alongX
              ? {
                  min: new THREE.Vector3(it.pos[0] - w / 2, y0, it.pos[2] - halfT),
                  max: new THREE.Vector3(it.pos[0] + w / 2, y1, it.pos[2] + halfT),
                }
              : {
                  min: new THREE.Vector3(it.pos[0] - halfT, y0, it.pos[2] - w / 2),
                  max: new THREE.Vector3(it.pos[0] + halfT, y1, it.pos[2] + w / 2),
                },
            colliderFixed,
            colliderSlide,
          });
        }
        continue;
      }

      // 窗：先按 window-wall / room-side 两个子组分别合批再挂组。
      // 必须先拆再合批——mergeByMaterial 会把整棵子树按材质压平、抽空命名子组，
      // 若先合批再拆，真实几何会全落到 obj 根节点，丢失"窗框跟墙隐 / 窗台永远可见"的归属。
      if (it.kind === 'window') {
        // 窗是贴在墙面上的薄构件，不参与地面投影，否则会糊出一片脏影子
        obj.traverse((o) => { (o as THREE.Mesh).castShadow = false; });
        const decorGroup = it.wall ? wallDecor.get(it.wall) : undefined;
        const rsd = it.wall ? roomSideDecor.get(it.wall) : undefined;
        const wallG = obj.getObjectByName('window-wall') as THREE.Group | null;
        const roomG = obj.getObjectByName('room-side') as THREE.Group | null;
        // 摘出子组后 obj 不再进场景——必须把 obj 的墙位/朝向继承给子组，
        // 否则窗框脱离 obj 会丢位置（跑到装饰组原点）并丢失旋转（朝向错乱=“水平前后”）。
        if (wallG) {
          wallG.position.copy(obj.position);
          wallG.rotation.copy(obj.rotation);
          wallG.scale.copy(obj.scale);
          wallG.userData.furnitureRoot = true; // 窗框随墙走，碰撞由 buildWallColliders 接管，遍历跳过
          mergeByMaterial(wallG);
          outlineProp(wallG);
          (decorGroup ?? unitGroup).add(wallG); // 贴墙：跟墙一起隐
          freezeStatic(wallG);
        }
        if (roomG) {
          roomG.position.copy(obj.position);
          roomG.rotation.copy(obj.rotation);
          roomG.scale.copy(obj.scale);
          roomG.userData.furnitureRoot = true; // 窗台凸入室内，碰撞由 buildWallColliders 接管，遍历跳过
          mergeByMaterial(roomG);
          outlineProp(roomG);
          (rsd ?? unitGroup).add(roomG); // 凸入室内：永远可见
          freezeStatic(roomG);
        }
        if (!wallG && !roomG) (decorGroup ?? unitGroup).add(obj);
        continue;
      }

      // 地毯这类贴地的东西不要投影，否则会糊出一片脏影子
      if (it.kind === 'rug' || it.kind === 'jpRug' || it.kind === 'jpDiningRug') obj.traverse((o) => { (o as THREE.Mesh).castShadow = false; });
      // 散点小摆件（lifestyle 拖鞋/伞/布包/纸箱/垃圾桶…）保留逐件 mesh，不参与合批：
      // 这类道具的零件散落在房间各处，合批会把「同材质、相距数米」的件熔成一个 mesh，
      // 遍历补碰撞（buildSceneColliders）只能按合并后的整体 AABB 收盒——玄关的拖鞋/伞
      // 和客厅的布包同材质就会并出横跨半套房的隐形碰撞墙（实测 5.3m×8.9m 大盒），
      // 视觉上没家具却走不过去。逐件保留后每件自己收小盒；件数约 20，合批收益本就有限。
      if (!PIECE_KEEP_RAW.has(it.kind)) mergeByMaterial(obj);
      // 先合批再描边：描边长在合并后的少数几个 mesh 上，反过来会把几千个壳也卷进来
      // （散点摆件不合并也照常描边——addOutline 按描边分级把整件道具并成 1~2 个壳）
      outlineProp(obj);
      // 挂墙的厚装饰进对应墙侧的 decor 组；其余（含玻璃滑门——两侧都看得见）直接进场景
      const decorGroup = it.wall ? wallDecor.get(it.wall) : undefined;
      (decorGroup ?? unitGroup).add(obj);
      // 家具位置再也不会变，冻结掉逐帧的矩阵重算
      freezeStatic(obj);
      } catch (e) {
        console.error(`[room] 家具构建异常 id=${it.id} kind=${it.kind}:`, e);
      }
    }

    /* ---------------- Props ---------------- */

    const P = layout.props;

    /**
     * 天花板挂件组：天花板也是朝内的单面（法线朝下），相机升到天花板上方向下
     * 俯视时它被背面剔除，吊灯/吸顶灯这种实体必须跟着一起让开。阳台露天无天花，
     * 不受影响。
     */
    const ceilingY = layout.room.height;
    const ceilingDecor = new THREE.Group();
    unitGroup.add(ceilingDecor);

    /** 摆件的统一收口：合批 → 描边 → 挂进父级 → 冻结。顺序不能乱。 */
    const placeProp = (obj: THREE.Object3D, parent: THREE.Object3D, outline = true) => {
      mergeByMaterial(obj);
      if (outline) outlineProp(obj);
      parent.add(obj);
      freezeStatic(obj);
    };

    if (P.lantern) placeProp(buildCeilingLantern(P.lantern.pos, layout.room.height), ceilingDecor);

    for (const c of P.ceilLamps ?? []) {
      placeProp(buildJpCeilLamp({ pos: c.pos, ceilingH: layout.room.height }), ceilingDecor);
    }

    /**
     * 挂墙装饰（默认朝 +Z）按 wall 朝内法线方向旋转。
     * ws.dir 是外墙的朝室内法线（dir=+1 朝 +X / +Z；dir=-1 朝 -X / -Z）。
     * 绕 Y 的映射（实证值，别凭直觉推）：rot=0→+Z，+π/2→+X，π→-Z，-π/2→-X。
     *   axis='x' dir=+1 → 朝 +X（西墙），rot=+π/2；dir=-1 → 朝 -X（东墙），rot=-π/2
     *   axis='z' dir=+1 → 朝 +Z（北墙），rot=0；   dir=-1 → 朝 -Z（南墙），rot=π
     * 不转会侧立（box 0.018 厚面朝室内）；转反则正脸朝墙里、室内只看到背板。
     */
    const wallRotY = (wallId: string | undefined): number => {
      if (!wallId) return 0;
      const ws = shell.wallSides.find((w) => w.id === wallId);
      if (!ws) return 0;
      if (ws.axis === 'x') return ws.dir > 0 ? Math.PI / 2 : -Math.PI / 2;
      return ws.dir > 0 ? 0 : Math.PI;
    };

    placeProp(buildJpWallClock(P.clock.pos, wallRotY(P.clock.wall)), wallDecor.get(P.clock.wall) ?? scene);
    placeProp(buildJpPoster(P.poster.pos, P.poster.size, wallRotY(P.poster.wall)), wallDecor.get(P.poster.wall) ?? scene);
    if (P.stringLights) {
      const sl = buildStringLights(P.stringLights.from, P.stringLights.to, P.stringLights.sag);
      // 灯串是贴顶的细管，遍历收进去会沿天花板拉出一排薄碰撞盒（玩家本就走不到）；
      // 它不参与第一人称碰撞，直接跳过。
      sl.userData.sceneCollideSkip = true;
      placeProp(sl, wallDecor.get(P.stringLights.wall) ?? scene);
    }

    // 光束（每扇进光的窗/玻璃门一条）和浮尘是纯叠加混合，不能描边。
    // 光束本身是静止的，照样冻结；浮尘每帧改写顶点，保持鲜活。
    for (const sb of (P.sunbeams ?? []) as BeamConfig[]) {
      const beam = buildSunbeam(sb);
      // 挂进所属墙的 decor 组，不能挂 scene：光束是从窗口拉到地面的大四边形，
      // 脱离墙之后，观察者模式把墙剔掉的瞬间它就变成几片悬空的白条（看着像窗棂）。
      // 走 decor 而不是 roomSideDecor —— decor 的语义就是「跟墙同进退」。
      const parent = sb.wall ? wallDecor.get(sb.wall) : undefined;
      // 没写 wall 或写错 id 都会静默退回 scene，等于把这个 bug 原样放回去，所以必须喊出来
      if (!parent) console.warn(`[room] 光束未绑定到有效墙侧（wall=${sb.wall ?? '未填'}），观察者模式下不会随墙隐藏`);
      (parent ?? unitGroup).add(beam);
      freezeStatic(beam);
    }

    const motes = buildDustMotes(P.motes);
    unitGroup.add(motes.object);

    /* ---------------- 雨夜层：雨丝 + 阳台湿地反光 ----------------
     *
     * 这套户型里只有阳台（ceiling:false）是露天的，也是唯一"雨真的会落到的
     * 室外地面"。所以 JSON 里这套湿地反光只铺阳台：房间里淋不到雨，铺了
     * 就是撒谎。街道层面的湿地/路灯已随室外层 Stage 2 接走（见上方
     * streetscape 装配），雨区 margin 扩到盖住南街。
     */

    const W = (layout.weather ?? {}) as {
      rain?: {
        count: number; speed: number; size: number; opacity: number; wind?: number;
        /** 跟随模式下雨区的水平半径（米，以焦点为心）。 */
        radius?: number;
        /** 雨区是否跟随焦点，默认 true。 */
        follow?: boolean;
        top?: number; layers?: number; farSize?: number; farOpacity?: number;
      };
      wetPools?: PoolSpec[];
      sparkle?: { count?: number; area?: number; center?: [number, number] };
    };

    /** 雨的对象 / 每帧更新（null = 这局没下雨）。update 的 focus = 雨区跟随的焦点。 */
    let rainObj: THREE.Group | null = null;
    let rainUpdate: ((dt: number, focus?: { x: number; z: number }) => void) | null = null;
    let wetGroundObj: THREE.Object3D | null = null;
    let snowObj: THREE.Points | null = null;
    let snowUpdate: ((dt: number, focus?: { x: number; z: number }) => void) | null = null;
    /** 相机是否正处在有顶室内——室内一滴雨都不该有。 */
    let rainIndoors: ((x: number, y: number, z: number) => boolean) | null = null;

    if (W.rain && W.rain.count > 0) {
      const R = W.rain;
      // 雨区跟随焦点（见 buildRain 的 followCamera）：半径只要罩住焦点四周的
      // 可见范围，不必像固定区域那样一路外扩到盖住南街。
      const radius = R.radius ?? 12;
      const top = R.top ?? 14;
      const area = {
        x: [-radius, radius],
        y: [0, top],
        z: [-radius, radius],
      } as RainConfig['area'];
      // 落雨禁区 = 有顶房间的并集包围盒（+UNIT_LIFT：整户在二楼）。
      // ceiling:false 的阳台不算禁区，雨要落进阳台；禁区上方（屋顶之上）不拦，
      // 观察者模式俯视时能看到檐外落雨。邻居/地面层的外壳不用禁——它们内部
      // 不可见，雨丝穿进去也被不透明立面挡住。
      const roofed = (layout.shell.rooms as Array<{ x0: number; x1: number; z0: number; z1: number; ceiling?: boolean }>).filter((r) => r.ceiling !== false);
      const exclude = roofed.length
        ? {
            min: [Math.min(...roofed.map((r) => r.x0)), UNIT_LIFT - 0.2, Math.min(...roofed.map((r) => r.z0))] as [number, number, number],
            max: [Math.max(...roofed.map((r) => r.x1)), UNIT_LIFT + layout.room.height + 0.1, Math.max(...roofed.map((r) => r.z1))] as [number, number, number],
          }
        : undefined;
      // 「相机是否在有顶房间里」逐间判，而不是套用上面那个并集盒：并集盒是
      // 矩形，L 形户型里房间之间的凹口会被误判成室内。阳台 ceiling:false 不在
      // roofed 里，所以站阳台不算室内、照常淋雨。
      const yLo = UNIT_LIFT - 0.2;
      const yHi = UNIT_LIFT + layout.room.height + 0.1;
      rainIndoors = (x, y, z) =>
        (x > -31 && x < 31 && z > -5.9 && z < 4.7 && y > 0 && y < 3.3) ||
        (x > -36.8 && x < -31 && z > -7.2 && z < 4.7 && y > 0 && y < 12.2) ||
        (y > yLo && y < yHi && roofed.some((r) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1));
      const rain = buildRain({
        count: R.count,
        area,
        exclude,
        followCamera: R.follow !== false,
        speed: R.speed,
        size: R.size,
        opacity: R.opacity,
        wind: R.wind,
        layers: R.layers,
        farSize: R.farSize,
        farOpacity: R.farOpacity,
      });
      scene.add(rain.object);
      rainObj = rain.object;
      rainUpdate = rain.update;
    }

    // 雪：用柔和白色点粒子表现慢速飘雪，和雨丝分开，切换天气时只改可见性。
    {
      const snowCount = 520;
      const snowPos = new Float32Array(snowCount * 3);
      const snowSpeed = new Float32Array(snowCount);
      const snowRnd = makeRng(20260906);
      const snowRadius = W.rain?.radius ?? 15;
      const snowTop = W.rain?.top ?? 14;
      for (let i = 0; i < snowCount; i++) {
        snowPos[i * 3] = (snowRnd() - 0.5) * snowRadius * 2;
        snowPos[i * 3 + 1] = snowRnd() * snowTop;
        snowPos[i * 3 + 2] = (snowRnd() - 0.5) * snowRadius * 2;
        snowSpeed[i] = 0.28 + snowRnd() * 0.42;
      }
      const snowGeo = new THREE.BufferGeometry();
      snowGeo.setAttribute('position', new THREE.BufferAttribute(snowPos, 3));
      const snow = new THREE.Points(snowGeo, new THREE.PointsMaterial({
        color: '#fffaf0', size: 0.09, transparent: true, opacity: 0.82,
        depthWrite: false, sizeAttenuation: true,
      }));
      snow.name = 'snow';
      snow.frustumCulled = false;
      snow.visible = false;
      scene.add(snow);
      snowObj = snow;
      let snowOriginX = 0;
      let snowOriginZ = 0;
      snowUpdate = (dt, focus) => {
        if (focus) {
          snowOriginX = focus.x;
          snowOriginZ = focus.z;
          snow.position.set(snowOriginX, 0, snowOriginZ);
        }
        const attr = snowGeo.getAttribute('position') as THREE.BufferAttribute;
        const arr = attr.array as Float32Array;
        for (let i = 0; i < snowCount; i++) {
          const yIndex = i * 3 + 1;
          arr[yIndex] -= snowSpeed[i] * dt;
          arr[i * 3] += Math.sin((clockT + i) * 0.7) * 0.0015;
          arr[i * 3 + 2] += Math.cos((clockT + i) * 0.55) * 0.0012;
          if (arr[yIndex] < 0) {
            arr[yIndex] = snowTop;
            arr[i * 3] = (snowRnd() - 0.5) * snowRadius * 2;
            arr[i * 3 + 2] = (snowRnd() - 0.5) * snowRadius * 2;
          }
        }
        attr.needsUpdate = true;
      };
    }

    if (W.wetPools?.length) {
      const wet = buildWetGround(W.wetPools, {
        sparkleCount: W.sparkle?.count,
        area: W.sparkle?.area,
        center: W.sparkle?.center,
      });
      unitGroup.add(wet);
      wetGroundObj = wet;
      weatherFxObjects.push(wet);
      // 光斑和碎光点是叠加混合的透明件，不能参与合批（透明要按距离排序），
      // 也不描边 —— 描边壳会把光晕框出一圈实线。
      freezeStatic(wet);
    }

    // 外墙、地板、天花连同已经挂进去的装饰一起冻结。必须等家具都挂完再调，
    // 否则 decor 组里的家具会拿到还没算过的父级矩阵。
    freezeStatic(shell.group);

    /* ---------------- 几何体去重（放在全部合批之后） ----------------
     * 合批会把同材质的零件并成少数几个几何体，但**合不动的**那些还留着：
     * 透明件（玻璃、光晕）、noMerge 子树、单件即独占一个材质的构件。它们里面
     * 有 301 个几何体是逐字节重复的（合计约 5.4 MB 顶点数据，CPU 与 GPU 各一份），
     * 全部来自「循环里 new 出来的同尺寸几何」——构建期各 new 各的，缓存没覆盖到。
     *
     * 必须放在所有 mergeByMaterial 之后：合批会 dispose 源几何，先去重等于白干。
     */
    const dedup = dedupeGeometries(scene);
    if (dedup.removed) {
      console.info(`[room] 几何去重：${dedup.groups} 组、收掉 ${dedup.removed} 个重复几何体，省 ${(dedup.bytes / 1048576).toFixed(2)} MB`);
    }

    /* ---- 场景遍历全量补碰撞 ----
     * 给所有未被 layout 驱动碰撞（家具尺寸盒/墙体门洞盒/栏杆/floor/ramp）覆盖的实体几何
     * 补碰撞盒：室内落地灯/lifestyle 摆件（拖鞋/伞/快递箱）/阳台件/吊灯，室外路灯/便利店
     * 货架/吧台/冷饮柜等——第一人称原本能直接穿过去，现在都会挡人。
     * 见 collider.ts buildSceneColliders 的跳过规则：房间外壳（地板/墙/天花）整体标了
     * sceneCollideSkip、有尺寸的 nav 家具标了 furnitureRoot、门/窗/地毯/浴室也跳过——
     * 它们各自已有更准的碰撞盒，不会被重复收、也不会把门堵死。
     *
     * 室外（街道楼群/便利店）的碰撞在「合批前」收集：合批会把 bldg-* 与 store-mass 子组合并
     * 进大 mesh、丢掉其 sceneCollideSkip 标记，合并出的大 mesh 直接挂根下会被收成横跨整片
     * 楼群的巨型盒；合批前收集则每根路灯/每个货架还是独立小 mesh，只标了 skip 的楼群主体/
     * 便利店主体被跳过，不会把入户门/店门重新堵死。
     * ⚠️ 「透明 = 自然跳过」只对纯玻璃板成立，别拿它当"门洞一定通"的保证：
     *    - 便利店门扇上的金属竖杆/横杆是不透明的，遍历会照样收成盒杵在门洞里 → 标 noCollide；
     *    - 橱窗玻璃整片透明 → 遍历完全不收，等于没有碰撞 → 声明式盒（store.boxes）补上；
     *    - 海报/贴纸这类零厚度装饰板 → 遍历按 AABB 生成实心盒（门洞正中那张就是隐形墙）
     *      → 标 sceneCollideSkip。
     * 公寓外壳（203 所在楼体）整体不进遍历——它的主体是实心整块、门洞只在 layout 层挖，
     * 遍历会重新把入户门堵死；楼体与栏杆/自行车走声明式盒（见 buildBoxColliders），
     * 各层落脚面由 worldColliders 的 floor / ramp 盒接管。 */
    fpsColliders = fpsColliders.concat(
      buildSceneColliders(unitGroup),
      streetSceneColliders,
      storeSceneColliders
    );
    // 自检钩子：把第一人称碰撞体挂到调试面。门外景那些「能不能走进去 / 会不会
    // 被空气墙挡住」的问题，只看源码是判不出来的——必须拿真实的盒表去跑
    // collidesAt 那一套规则（scripts/room/verify-bookstore.mjs 就是这么做的）。
    (window as any).__ROOM__.colliders = fpsColliders;

    /**
     * 跨场景导航：把定稿的碰撞表派生成「多层栅格 + 层间门户」。
     *
     * 位置必须在 fpsColliders 定稿之后——它吃的是同一份盒表（导航与碰撞同源）。
     * 分层的理由见 navWorld.ts：室内要 0.15m 精度，街区 ±80m 用同样精度就是上百万格。
     */
    const navWorld = buildNavWorld(fpsColliders, {
      // 2F 那层横跨整栋楼、楼板是一整片：只放开「北外廊 + 电梯厅 + 东端楼梯平台」，
      // 其余（邻居户室内）一律不可走。坐标全部取自 exterior.ts 的常量，不在这里写第二份。
      allow: {
        apt2out: [
          { x0: APT_X0, x1: STRS.lx0, z0: APT_CORRIDOR_N, z1: APT_ZN },                 // 北侧外廊
          { x0: -36.8, x1: APT_X0, z0: -7.18, z1: 4.68 },                              // 2F 电梯厅
          { x0: STRS.lx0, x1: STRS.ex1, z0: STRS.za0, z1: STRS.zb1 },                   // 东端楼梯平台
        ],
      },
      // 203 室内用 dormLayout 的标称尺寸当障碍（导航口径），不吃渲染侧那份为
      // 「第一人称不穿模」而建的精细碰撞盒——否则客厅连沙发前都站不下人。
      extraObstacles: { apt2: buildObstacles(layout as never) },
    });
    (window as any).__ROOM__.navWorld = navWorld;
    /**
     * 街面积水反射面的**遮挡**剔除：把"头顶的水平板"回填给它（见 pickOverheadSlabs）。
     *
     * 位置很讲究：必须在 fpsColliders 定稿之后。它靠的是**合批之前**逐件收的 AABB，
     * 而 mergeByMaterial 早在上面的 store.group 装配时就跑过了 —— 所以只能拿这里这份
     * 定稿表，不能让它自己遍历场景去认楼板（并完的板 AABB 跨多层/横跨整条街，认不准）。
     * 反射面自己知道镜面高度，筛板的三条判据都在 pickOverheadSlabs 里。
     */
    store.setWetOccluders(fpsColliders);
    /** 调试钩子：传空表即关掉那层遮挡剔除。
     *
     *  它是可证回归的唯一入口 —— 「剔除开 / 关」两帧必须**逐像素相同**（剔除只在
     *  积水一个像素都露不出来的机位触发；真触发了不该触发的机位，两帧就会分叉）。
     *  见 scripts/room/verify-wet-reflection.mjs。 */
    (window as any).__ROOM__.setWetOccluders = (c: Collider[]) => store.setWetOccluders(c);
    // 门表也挂上（只取纯数据）。排查「某处莫名弹出 F 提示」时，直接拿相机高度
    // 和每道门的净高区间对照，比顺着源码猜快得多——门口弹提示的根因就在这两
    // 个数是否同层。THREE 对象不入表，避免调试面里出现无法序列化的引用。
    (window as any).__ROOM__.doors = doors.map((d) => ({
      x: d.x, z: d.z, y0: d.y0, y1: d.y1, width: d.width, slide: d.slide,
    }));

    console.log('[room] 初始化完成, 进入渲染循环');

    /**
     * 第一人称碰撞体缓冲。
     *
     * fps.update 每帧要拿到「静态碰撞体 + 当前门扇碰撞盒」。静态那 700+ 个盒
     * 装配完就再也不会变（fpsColliders 的全部赋值都在本行之前），门扇盒只有
     * 个位数、每帧变。之前每帧 `fpsColliders.concat(blockerBuf)` 会新造一个
     * 700+ 元素的数组——这是渲染循环里唯一一处稳定的每帧垃圾，而第一人称恰好
     * 是最需要稳帧的时候。改成预分配缓冲：静态部分拷一次，之后每帧只重写尾部。
     */
    const fpsColliderBuf: Collider[] = fpsColliders.slice();
    const FPS_STATIC_COLLIDER_N = fpsColliderBuf.length;

    /* ---------------- Characters ---------------- */

    /**
     * 阴影的重画请求。装配时置 true 保证首帧一定画一次；
     * GLB 是异步加载的，角色落进场景时也置一次，否则要等它走一步才有影子。
     */
    let shadowPending = true;
    /**
     * 阴影贴图重画的最小间隔（秒），以及距离上次重画的计时。
     * 见渲染循环里的「阴影节流」说明。
     */
    const SHADOW_MIN_INTERVAL = 0.12;
    let shadowCooldown = 0;
    /** 点光池（每帧按视角收进预算，见 budgetPointLights）。 */
    let pointPool: THREE.PointLight[] | null = null;
    const disposeBlenderFurniture = loadBlenderFurniture(blenderSlots, () => {
      shadowPending = true;
      // 家具 GLB 是异步落进场景的，上面那次去重跑在它之前——补跑一次。
      // 函数本身幂等：没有新的重复项就什么都不做。
      dedupeGeometries(scene);
    }, interiorDesign.prepareFurniture);

    const loader = new GLTFLoader();
    // WebView2 may resolve an embedded image through createImageBitmap without rejecting
    // the GLB load, leaving a valid mesh with a null map. Use the browser's image decoder
    // for these small embedded character textures so texture failures don't silently turn
    // the characters into white models.
    loader.register((parser) => {
      const textureLoader = new THREE.TextureLoader(parser.options.manager);
      textureLoader.setCrossOrigin(parser.options.crossOrigin);
      textureLoader.setRequestHeader(parser.options.requestHeader);
      parser.textureLoader = textureLoader;
      return { name: 'room-character-texture-loader' };
    });
    const agents: PetAgent[] = [];
    const bodies: THREE.Object3D[] = [];
    const characterAnimations: Array<ReturnType<typeof createCharacterAnimation>> = [];
    // GLB 自带贴图的登记表：卸载时精确 dispose，不碰程序化贴图单例
    const gltfTextures: THREE.Texture[] = [];

    // characters 里混着 _height_note 之类的说明字段，按「有没有 startPos」挑真人——
    // 直接把说明字段当角色解析会在 placeholder.position.set(undefined[0]…) 上炸掉整个场景装配。
    const charEntries = Object.entries(
      layout.characters as Record<string, { model: string; height: number; startPos: [number, number, number]; startFacing: number; yaw?: number }>
    ).filter(([, cfg]) => Array.isArray(cfg?.startPos));
    // 角色占用的热点表是静态的，跨场景挂载存活；重建前先清，否则新场景会继承旧占用。
    PetAgent.resetClaims();

    charEntries.forEach(([id, cfg], idx) => {
      // placeholder 管位移和朝向，body 管上下浮动和走路时的左右晃
      const placeholder = new THREE.Object3D();
      placeholder.position.set(cfg.startPos[0], 0, cfg.startPos[2]);
      const body = new THREE.Object3D();
      placeholder.add(body);
      // 角色挂 scene（世界坐标）而不是 unitGroup（那一组把 203 整体抬高 3.4）。
      // 现在角色会走出 203 去外廊/街区，跨场景的坐标必须统一到世界系：
      // 挂在一个被抬高的组下面，navWorld 给的世界标高会整体多抬 3.4m。
      scene.add(placeholder);
      bodies[idx] = body;
      agents[idx] = new PetAgent(id, cfg.startPos, cfg.startFacing);
      // 203 在 2F：初始层与世界标高都按导航层的定义来（startPos 是相对楼板的）
      agents[idx].setLayer('apt2');

      loader.load(
        cfg.model,
        (gltf) => {
          if (!alive) {
            gltf.scene.traverse((o: any) => { o.geometry?.dispose?.(); o.material?.dispose?.(); });
            gltf.scene.traverse((o: any) => {
              const mats = Array.isArray(o.material) ? o.material : [o.material];
              for (const m of mats ?? []) {
                m?.map?.dispose?.();
                m?.normalMap?.dispose?.();
              }
            });
            return;
          }
          const model = gltf.scene;
          // 按包围盒把角色 normalize 到 layout 里指定的目标身高（米）。
          // GLB 生成器的输出尺度每次可能不同，写死 scale 是这次尺寸错配的根因。
          const bbox = new THREE.Box3().setFromObject(model);
          const nativeHeight = bbox.max.y - bbox.min.y;
          if (nativeHeight > 0) {
            model.scale.setScalar(cfg.height / nativeHeight);
          }
          // 模型自带朝向与房间约定不符时，给一个基准偏航角（绕 Y，俯视顺时针为负）。
          // 只动模型自身，不进 facing —— facing 由 agents 驱动导航，二者在 world 里叠加。
          model.rotation.y = cfg.yaw ?? 0;
          toonifyModel(model, gltfTextures);
          // 角色不加描边：卡通描边线在偏写实的 Q 版角色身上反而像描边画报，
          // 与三渲二家具的风格拼不到一起。让角色只靠色阶和软阴影"立"起来。
          body.add(model);
          const animation=createCharacterAnimation(model,gltf.animations);
          characterAnimations[idx]=animation;
          if(animation)agents[idx].sitDuration=animation.sitDuration;
          // 模型是异步落进场景的，此时它的影子还没进过 shadow map
          shadowPending = true;
        },
        undefined,
        (err) => console.error('[room] GLB load failed for', id, err)
      );
    });

    // 互绑同伴：查路绕人 + 头对头让行都挂在它上面，漏绑这一场景就等于没有避让。
    PetAgent.bindPeers(agents);
    // 跨场景导航世界：角色靠它走外廊/楼梯/街区；不注入就退回只走 203 那张单层栅格
    PetAgent.bindWorld(navWorld);

    // agents 建完后补挂到 __ROOM__：验收脚本要读角色坐标/状态/当前热点，
    // 只有 scene 的话就只能靠肉眼看截图，改一次布局就得人工盯一次。
    const roomDbg = (window as any).__ROOM__;
    if (roomDbg) { roomDbg.agents = agents; roomDbg.navGrid = PetAgent.navGrid; }

    /* ---------------- Resize / Visibility ---------------- */

    const onResize = () => {
      const w = container.clientWidth;
      const h = Math.max(1, container.clientHeight);
      renderer.setSize(w, h);
      // composer 有自己的一套 RT（含我们传进去的 MSAA target），不跟着 setSize
      // 的话后处理会一直按首帧的分辨率画，窗口一变就拉伸模糊
      composer.setSize(w, h);
      applyProjection(w, h);
    };
    window.addEventListener('resize', onResize);

    let running = true;
    const onVisibility = () => { running = !document.hidden; };
    document.addEventListener('visibilitychange', onVisibility);

    /**
     * P：整段第 ① 步的 A/B 对照。
     *
     * 三项一起切才是有效对照——泛光在 composer 里、雾在 scene 上、色调映射在
     * renderer 上，只切其中一个，看到的差别会被另外两项盖住，等于白切。
     * 关掉时走 renderer.render 直出：此时 three 会自己应用 renderer.toneMapping，
     * 所以必须显式退回 NoToneMapping，否则"关掉"之后色调映射还在生效。
     *
     * 切换 scene.fog 会触发全场材质重编译（USE_FOG 定义变了），第一下会卡一帧，
     * 这是调试开关，可以接受。
     */
    const onFxKey = (e: KeyboardEvent) => {
      if (e.key !== 'p' && e.key !== 'P') return;
      if (cmdOpenRef.current) return; // 命令面板输入时 p 是打字，不切调试开关
      fxOn = !fxOn;
      scene.fog = fxOn ? roomFog : null;
      renderer.toneMapping = fxOn ? baseToneMapping : THREE.NoToneMapping;
    };
    window.addEventListener('keydown', onFxKey);

    // 第一人称：靠近门时按 F 开/关门（manualOpen 切换）。nearDoor 由渲染循环每帧刷新，
    // 指向触发范围内最近的一道门；观察者模式下 nearDoor 恒为 null，此处理器直接返回。
    const onDoorKey = (e: KeyboardEvent) => {
      if (!isFirstPerson || !nearDoor) return;
      if (cmdOpenRef.current) return; // 命令面板输入时 f 是打字，不开门
      if (e.key === 'f' || e.key === 'F') {
        nearDoor.manualOpen = !nearDoor.manualOpen;
      }
    };
    window.addEventListener('keydown', onDoorKey);

    /* ---------------- Render loop ---------------- */

    let lastT = performance.now();
    let frameCount = 0;
    let fpsT = lastT;
    /** 首帧是否已经真的画出来了——首屏 loading 层只撤一次，靠它去重。 */
    let firstFrameDone = false;
    setEnvironment(environmentRef.current.period, environmentRef.current.weather);
    let clockT = 0;
    let raf = 0;

    // 上一帧记录的角色位置，用来判断影子是否需要重画
    const prevAgentPos = agents.map((a) => ({ x: a.pos.x, z: a.pos.z }));
    const SHADOW_MOVE_EPS = 0.0015;

    // 上一帧相机位置（第一人称下用作玩家移动方向，判定是否正在穿门）
    const prevCamPos = { x: camera.position.x, z: camera.position.z };

    // 上一帧显示的速度挡位：挡位标签只在变化时写 DOM，不必每帧重写样式
    let lastSpeedTier = 'walk';

    /**
     * 判断一段折线（角色 A* 路径 / 第一人称移动前瞻）是否穿过某道门洞，
     * 并返回开门旋转方向符号：门从「穿入侧」往「穿出侧」甩，避免门扇扫到角色。
     * - alongX：门洞沿 X 布置，穿过方向是 Z；返回 -sign(Δz)
     * - 否则： 门洞沿 Z 布置，穿过方向是 X；返回 -sign(Δx)
     */
    function pathCrossesDoor(
      pts: { x: number; z: number }[],
      d: { x: number; z: number; alongX: boolean; width: number }
    ): { hit: boolean; sign: number } {
      const half = d.width / 2 + 0.25;
      for (let i = 0; i < pts.length - 1; i++) {
        const a = pts[i];
        const b = pts[i + 1];
        if (d.alongX) {
          const dz0 = a.z - d.z;
          const dz1 = b.z - d.z;
          if (dz0 === 0 || dz1 === 0 || (dz0 < 0) !== (dz1 < 0)) {
            const t = dz0 === dz1 ? 0 : (d.z - a.z) / (b.z - a.z);
            const xc = a.x + t * (b.x - a.x);
            if (Math.abs(xc - d.x) <= half) return { hit: true, sign: -Math.sign(b.z - a.z) || 1 };
          }
        } else {
          const dx0 = a.x - d.x;
          const dx1 = b.x - d.x;
          if (dx0 === 0 || dx1 === 0 || (dx0 < 0) !== (dx1 < 0)) {
            const t = dx0 === dx1 ? 0 : (d.x - a.x) / (b.x - a.x);
            const zc = a.z + t * (b.z - a.z);
            if (Math.abs(zc - d.z) <= half) return { hit: true, sign: -Math.sign(b.x - a.x) || 1 };
          }
        }
      }
      return { hit: false, sign: 0 };
    }

    /**
     * 取角色 A* 路径中「当前位置往前 maxDist 米」以内的折线段。
     * 避免把整条长路径都拿去判定——否则角色只是远远路过、最终会穿过这道门，
     * 门就会提前开。
     */
    function lookaheadPath(
      a: { pos: { x: number; z: number }; path: { x: number; z: number }[]; pathIdx: number },
      maxDist: number
    ): { x: number; z: number }[] {
      const pts = [{ x: a.pos.x, z: a.pos.z }];
      let acc = 0;
      for (let i = a.pathIdx; i < a.path.length; i++) {
        const p = a.path[i];
        const last = pts[pts.length - 1];
        const seg = Math.hypot(p.x - last.x, p.z - last.z);
        if (acc + seg > maxDist) {
          const t = (maxDist - acc) / seg;
          pts.push({ x: last.x + (p.x - last.x) * t, z: last.z + (p.z - last.z) * t });
          break;
        }
        pts.push({ x: p.x, z: p.z });
        acc += seg;
      }
      return pts;
    }

    const render = () => {
      raf = requestAnimationFrame(render);
      if (!running) { lastT = performance.now(); return; }

      const now = performance.now();
      const elapsed = Math.min((now - lastT) / 1000, 0.25);
      lastT = now;
      clockT += elapsed;
      districtArt.update(camera);
      renderer.info.reset();

      // 逻辑 tick（20Hz 固定步长）
      let acc = elapsed;
      while (acc > 0) {
        const step = Math.min(acc, TICK_DT);
        for (const a of agents) a.tick(step);
        // 每步之后解一次角色间的分离约束：让行是「尽量不错身」，它才是「绝不互相插入」
        PetAgent.separate(agents);
        acc -= step;
      }

      for (let i = 0; i < bodies.length; i++) {
        const body = bodies[i];
        const a = agents[i];
        if (!body || !a) continue;
        const parent = body.parent!;
        parent.position.x = a.pos.x;
        // 坐在沙发/椅子上时根节点抬到坐面高度：坐姿 clip 的 Root 位移恒为 0，
        // 不抬的话人是「站在地板上做坐姿」，屁股埋在坐垫里、腿从坐面里穿出来。
        parent.position.y = a.pos.y;
        parent.position.z = a.pos.z;
        parent.rotation.y = a.facing;

        const animation=characterAnimations[i];
        if(animation){
          body.position.y=0;body.rotation.z=0;
          animation.update(elapsed,a.state,a.currentHotspotId,a.walkSpeed);
        }else{
          // Models without embedded clips retain the existing procedural fallback.
          const walking=a.state==='walk';
          const phase=clockT*(walking?9:1.6)+i*1.7;
          body.position.y=Math.abs(Math.sin(phase))*(walking?.032:.011);
          body.rotation.z=walking?Math.sin(phase)*.045:0;
        }
      }

      motes.update(clockT);
      const weatherFocus = isFirstPerson ? camera.position : controls.target;
      if (rainUpdate && rainObj) {
        // 相机在有顶房间里 → 整场雨收起来并停更（省掉每实例的矩阵合成）。
        const indoors = rainIndoors
          ? rainIndoors(camera.position.x, camera.position.y, camera.position.z)
          : false;
          const activeRain = environmentRef.current.weather === 'drizzle' || environmentRef.current.weather === 'storm';
        rainObj.visible = activeRain && !indoors;
        if (activeRain && !indoors) rainUpdate(elapsed, weatherFocus);
      }
      if (snowUpdate && snowObj) {
        if (environmentRef.current.weather === 'snow') {
          snowObj.visible = true;
          snowUpdate(elapsed, weatherFocus);
        } else {
          snowObj.visible = false;
        }
      }
      // 湿地涟漪 / 屋檐滴水：与浮尘同时间基（clockT），错开相位各自循环
      for (const fx of sceneFx) fx(clockT);
      // 街角动效：招牌呼吸 / 感应门开合 / 红绿灯换色。门的开合由**玩家位置**驱动
      // （camera 在第一人称下就是玩家、观察者模式下就是观察点），不再是定时开合。
      store.update(
        clockT,
        environmentRef.current.weather === 'drizzle' || environmentRef.current.weather === 'storm',
        camera.position
      );
      // 居酒屋感应推拉门：同一条路（位置驱动 + doorHold 滞后 + dt 夹 0.25 + 动态盒同步）
      izakaya.update(clockT, camera.position);

      // 只有角色挪动了才重画阴影（首帧的 needsUpdate 已在装配时置好）
      let shadowDirty = false;
      for (let i = 0; i < agents.length; i++) {
        const a = agents[i];
        const p = prevAgentPos[i];
        if (!a || !p) continue;
        if (Math.abs(a.pos.x - p.x) > SHADOW_MOVE_EPS || Math.abs(a.pos.z - p.z) > SHADOW_MOVE_EPS) {
          p.x = a.pos.x;
          p.z = a.pos.z;
          shadowDirty = true;
        }
      }
      // 注意：这里只记「想不想重画」，真正决定这一帧画不画在门扇逻辑之后（要合并
      // 门扇的请求），并且要过一道节流——见下方「阴影节流」。
      let shadowWanted = shadowDirty || shadowPending;

      // 门开关动画：由「角色路径需要穿过门洞」驱动，但必须「靠近门洞」才开——
      // 有经过意图也不能提前开（满足"不要在角色靠近/远离时就自动开/关"：仅靠近不够，
      // 还要有穿过意图；有意图但还远也不开）。
      // - 观察者模式：PetAgent 有 A* 路径，路径穿过门洞「且」角色已靠近才开。
      // - 第一人称：相机即角色、没有规划路径，用「移动方向是否穿过门洞」判定，
      //   且同样要已靠近；站着不动即使贴着门也不开（除非已站在门洞里）。
      let doorMoved = false;
      /* ---- 公寓南侧入口：自动感应门 ----
       * 触发判据只有两条：门口有没有人（水平距离）、人在不在门洞那一层。
       * 高度那一条不能省——二楼阳台在 xz 上正压着门洞，只看水平距离的话，
       * 站在阳台上会把一楼的门打开。
       * 快开慢关，与真实感应门的观感一致。 */
      {
        const pd = aptAutoDoor;
        const near =
          camera.position.y < pd.y1 + 1.1 &&
          Math.hypot(camera.position.x - pd.x, camera.position.z - pd.z) < pd.senseRadius;
        const target = near ? 1 : 0;
        const diff = target - aptDoorOpen;
        if (Math.abs(diff) > 1e-4) {
          const step = (diff > 0 ? 2.6 : 1.4) * elapsed;
          aptDoorOpen = diff > 0
            ? Math.min(target, aptDoorOpen + step)
            : Math.max(target, aptDoorOpen - step);
          for (let i = 0; i < 2; i++) {
            const leaf = pd.leaves[i];
            leaf.position.x = pd.closeX[i] + (pd.openX[i] - pd.closeX[i]) * aptDoorOpen;
          }
          pd.indicator.emissiveIntensity = 0.35 + 1.6 * aptDoorOpen;
        }
      }
      const DOOR_NEAR = 1.3;  // 角色到门洞中心的最近距离阈值：超过此距离即使有穿过意图也不开
      const PATH_LOOKAHEAD = 0.9; // 观察者模式只前瞻路径前方这么远——远处路过的门不会提前开
      const DOOR_HOLD = 1.4;  // 门保持开启的宽限秒数：玩家穿过/走远后避免猛关
      // 第一人称：先找出触发范围内最近的一道门（用于显示 "F 打开" 提示 + 接收 F 键）
      nearDoor = null;
      if (isFirstPerson) {
        let bestD = Infinity;
        for (const d of doors) {
          // 高度过滤：门洞必须落在相机身边这一层，人在门洞净高之内才算"够得着"。
          // 少了这一条，站在公寓一楼南侧入口（水平投影正对楼上 203 的阳台滑门，
          // 两者 xz 几乎重合）也会弹「F 打开」——那扇门在头顶 3.4m 处，是楼上的门。
          if (camera.position.y < d.y0 + 0.35 || camera.position.y > d.y1 + 1.2) continue;
          const dc = Math.hypot(camera.position.x - d.x, camera.position.z - d.z);
          if (dc < DOOR_NEAR && dc < bestD) { bestD = dc; nearDoor = d; }
        }
      }
      for (const d of doors) {
        // PetAgent（角色）到门洞中心的最近距离——用于 PetAgent 自动开关门（两种模式都生效）
        let agentMin = Infinity;
        for (const a of agents) {
          const dist = Math.hypot(a.pos.x - d.x, a.pos.z - d.z);
          if (dist < agentMin) agentMin = dist;
        }
        // 第一人称相机到门洞中心的最近距离——仅用于"靠近弹 F 提示 + 手动开"
        let playerMin = Infinity;
        if (isFirstPerson) {
          playerMin = Math.hypot(camera.position.x - d.x, camera.position.z - d.z);
        }
        // 综合最近距离（reset manualOpen 用）
        const minDist = Math.min(agentMin, playerMin);

        let wantOpen = false;
        let openVal = 0; // 期望开门量（带符号：平开门含甩向，推拉门为正 openOffset）

        // PetAgent 自动开关门：两种模式都生效（修复"进第一人称后角色走到门前门不开"）。
        // 路径穿过门洞「且」角色已靠近才开——与观察者模式原逻辑一致。第一人称模式下这一支
        // 原先被整个关在 else 里，导致角色自动开关门在第一人称下彻底失效。
        if (agentMin < DOOR_NEAR) {
          for (const a of agents) {
            if (a.state !== 'walk' || a.path.length === 0) continue;
            const pts = lookaheadPath(a, PATH_LOOKAHEAD);
            const r = pathCrossesDoor(pts, d);
            if (r.hit) {
              wantOpen = true;
              openVal = d.slide ? d.openOffset : d.openAngle;
              break;
            }
          }
          // 安全：角色已站在门洞里也保持开启，避免门扇扫到角色
          if (!wantOpen) {
            for (const a of agents) {
              const half = d.width / 2 + 0.12;
              if (d.alongX) {
                if (Math.abs(a.pos.z - d.z) < 0.45 && Math.abs(a.pos.x - d.x) < half) {
                  wantOpen = true;
                  openVal = d.slide ? d.openOffset : d.openAngle;
                }
              } else {
                if (Math.abs(a.pos.x - d.x) < 0.45 && Math.abs(a.pos.z - d.z) < half) {
                  wantOpen = true;
                  openVal = d.slide ? d.openOffset : d.openAngle;
                }
              }
            }
          }
        }

        // 第一人称手动触发（F 键）叠加：靠近门 + 按 F 才开，玩家走远自动收回。
        // 这里只处理玩家主动开门，"玩家靠近自动开"仍不做（靠近只弹提示），符合既有需求。
        if (isFirstPerson && d.manualOpen) {
          wantOpen = true;
          openVal = d.slide ? d.openOffset : d.openAngle;
          if (minDist > DOOR_NEAR * 1.8) d.manualOpen = false;
        }

        // 宽限：角色刚穿过门、wantOpen 立刻变 false 时，保持开门 DOOR_HOLD 秒，让其走远再关
        if (wantOpen) {
          d.openHold = DOOR_HOLD;
          d.heldTarget = openVal;
        } else if (d.openHold > 0) {
          d.openHold = Math.max(0, d.openHold - elapsed);
        }
        const target = wantOpen || d.openHold > 0 ? d.heldTarget : 0;

        const rate = 4; // 开合角速度 rad/s
        const before = d.current;
        if (target < d.current) {
          d.current = Math.max(target, d.current - rate * elapsed);
        } else {
          d.current = Math.min(target, d.current + rate * elapsed);
        }
        // 只有角度/位移实际变化才刷新矩阵——门静止时跳过 updateMatrixWorld 的整棵子树重算
        if (Math.abs(d.current - before) > 1e-4) {
          doorMoved = true;
          if (d.slide) {
            // 推拉门（fusuma）：只把其中一扇推到与另一扇重合——左半扇固定不动，
            // 右半扇（leaf2）沿局部 X 向左滑 width/2 与左半扇重叠，右半门洞因此打开。
            d.leaf.position.x = -d.width / 4;            // 固定半扇：始终盖住左半门洞
            if (d.leaf2) d.leaf2.position.x = d.width / 4 - d.current; // 动半扇：current=0 在右半，current=width/2 与左半重合
            d.leaf.updateMatrix();
            d.leaf.updateMatrixWorld(true);
            if (d.leaf2) {
              d.leaf2.updateMatrix();
              d.leaf2.updateMatrixWorld(true);
            }
          } else {
            d.leaf.rotation.y = d.current;
            // 门挂在被 freeze 的 decor 组下，matrixAutoUpdate 已关：手动重算本地矩阵，
            // 再强制刷新世界矩阵并递归子物体（门扇面板/把手/描边壳跟着转）
            d.leaf.updateMatrix();
            d.leaf.updateMatrixWorld(true);
          }
        }
      }
      // 门扇动了也要重渲阴影（静态阴影 autoUpdate 关着，门扇投影要跟着门转）
      if (doorMoved) shadowWanted = true;

      /**
       * 阴影贴图节流。
       *
       * 静态阴影（autoUpdate=false）本来是"有东西动了才重画"，但角色几乎一直在走、
       * 门开合时更是连续动，于是这个 pass 实际上每帧都在跑。它的代价不小：
       * 2048² 的贴图，要把全场投影体重新提交一遍——实测一次阴影更新是
       * **+463 draw call（第一人称）/ +1257（观察者视角）**，比主 pass 本体
       * （第一人称 684 / 观察者约 99）还多。原注释里的"484 calls / 34 万三角形"
       * 是更早一版的数，已按当前场景重量。
       *
       * 角色和门扇在画面里都很小，影子晚 0.12 秒跟上肉眼看不出来，但省掉的是
       * 接近一半的每帧提交量。装配完成 / GLB 落位走 shadowPending，不受节流影响，
       * 保证首帧和角色入场那一帧一定画出影子。
       *
       * **不要再想"把 2048² 降到 1024²"**：省的是 12.6 MB 显存与一次 4.2M 片元的
       * 深度填充（现代 GPU 上不到 1ms），而 463~1257 次提交一个都不会少（提交量
       * 才是这个 pass 的大头）；代价却是主光的 6cm/texel 采样密度，桌面/椅子的
       * 投影会开始出阶梯。收益与代价不成比例。
       */
      shadowCooldown -= elapsed;
      if (shadowPending || (shadowWanted && shadowCooldown <= 0)) {
        key.shadow.needsUpdate = true;
        shadowPending = false;
        shadowCooldown = SHADOW_MIN_INTERVAL;
      } else {
        key.shadow.needsUpdate = false;
      }
      prevCamPos.x = camera.position.x;
      prevCamPos.z = camera.position.z;

      // 电梯每帧只更新一次，且必须早于碰撞盒装配——门扇盒由这次更新产生。
      // 返回 true 表示它正在跑，这一帧由它接管玩家（见下方 fps.update 的跳过）。
      const liftBusy = apartmentLift.update(now, isFirstPerson);

      if (isFirstPerson) {
        // 第一人称：WASD 移动 + 碰撞检测。关着的门（> -0.5 rad ≈ 28°）挡路。
        // blockers 缓冲在 effect 顶层预分配复用，避免渲染循环每帧 new 数组
        blockerBuf.length = 0;
        for (const d of doors) {
          if (d.slide) {
            // 推拉门：固定半扇恒挡 + 动半扇随开度滑移，二者并集即「仍被遮挡的部分」。
            // 不再用整门盒按阈值全放/全挡——否则开门时两半扇一起放行，固定半扇那侧也被穿过。
            if (d.colliderFixed) blockerBuf.push(d.colliderFixed);
            if (d.colliderSlide) {
              const off = d.width / 4 - d.current; // 动半扇局部中心 x：关门=+w/4（右半），全开=−w/4（与固定半扇重合）
              if (d.alongX) {
                const cx = d.x + d.cosR * off;
                d.colliderSlide.min.x = cx - d.width / 4;
                d.colliderSlide.max.x = cx + d.width / 4;
              } else {
                const cz = d.z - d.sinR * off;
                d.colliderSlide.min.z = cz - d.width / 4;
                d.colliderSlide.max.z = cz + d.width / 4;
              }
              blockerBuf.push(d.colliderSlide);
            }
          } else {
            // 平开门：整扇旋转，开到足够角度后整扇离开门洞，按阈值放行
            const blockThresh = 0.5;
            if (Math.abs(d.current) < blockThresh) blockerBuf.push(d.collider);
          }
        }
        /* 感应门：门扇还没让开时才挡路。开度过 40%（洞口已让出约 0.85m）就放行，
         * 免得门正在开、人已经走到门面上时被夹住——感应门的存在感来自"它会开"，
         * 不是来自"它会挡"。 */
        if (aptDoorOpen < 0.4) {
          for (let i = 0; i < 2; i++) {
            const c = aptDoorBlockers[i];
            const cx = aptAutoDoor.leaves[i].position.x;
            c.min.set(cx - APT_DOOR_HALF_W, aptAutoDoor.y0, aptAutoDoor.z - APT_DOOR_HALF_T);
            c.max.set(cx + APT_DOOR_HALF_W, aptAutoDoor.y1, aptAutoDoor.z + APT_DOOR_HALF_T);
            blockerBuf.push(c);
          }
        }
        /* 电梯门：常驻闭合，只在合拢到位时挡人（盒由 apartmentLift.update 每帧重写） */
        for (const c of apartmentLift.doorBlockers) blockerBuf.push(c);
        // 静态碰撞体拷一次就够（装配完不再变），每帧只重写尾部那几个门扇盒——
        // 不再每帧 concat 出一个 700+ 元素的新数组。见 fpsColliderBuf 的说明。
        fpsColliderBuf.length = FPS_STATIC_COLLIDER_N;
        for (let i = 0; i < blockerBuf.length; i++) fpsColliderBuf.push(blockerBuf[i]);
        if (!liftBusy) fps.update(elapsed, fpsColliderBuf);
        setOutlineDistanceScale(0); // 第一人称描边距离固定
        // 第一人称下墙/顶/墙装饰全部可见
        for (let i = 0; i < wallMeshes.length; i++) wallMeshes[i].visible = true;
        for (let i = 0; i < ceilingMeshes.length; i++) ceilingMeshes[i].visible = true;
        ceilingDecor.visible = true;
        for (const ws of shell.wallSides) {
          ws.decor.visible = true;
          ws.roomSideDecor.visible = true;
          ws.reveals.visible = true;
        }
      } else {
        // 观察者模式：WASD/方向键 水平飞行 + Space/Shift 垂直飞行。
        // 鼠标旋转/缩放仍由 OrbitControls 处理（controls.update 在下方调用）。
        // 关键：飞行时相机和 OrbitControls.target 必须同步平移。否则只动相机、target
        // 不动，OrbitControls 会把"相机相对 target 的偏移变化"当成绕 target 旋转/缩放——
        // 结果 A/D 看起来像在转视角而不是横移（用户反馈的 bug）。
        if (obsKeys['w'] || obsKeys['s'] || obsKeys['a'] || obsKeys['d'] ||
            obsKeys['arrowup'] || obsKeys['arrowdown'] || obsKeys['arrowleft'] || obsKeys['arrowright'] ||
            obsKeys['space'] || obsKeys['shift']) {
          const obsSpeed = 14; // 飞行速度 m/s
          const obsDist = obsSpeed * elapsed;
          // 相机前向的水平投影（XZ 平面）
          camera.getWorldDirection(_obsDir);
          _obsDir.y = 0;
          if (_obsDir.lengthSq() < 1e-6) _obsDir.set(0, 0, -1);
          _obsDir.normalize();
          // 右向量 = 前 × 上（A/D 横移用这个，不是转向）
          _obsRight.crossVectors(_obsDir, camera.up).normalize();
          let mx = 0, mz = 0;
          if (obsKeys['w'] || obsKeys['arrowup']) { mx += _obsDir.x; mz += _obsDir.z; }
          if (obsKeys['s'] || obsKeys['arrowdown']) { mx -= _obsDir.x; mz -= _obsDir.z; }
          if (obsKeys['d'] || obsKeys['arrowright']) { mx += _obsRight.x; mz += _obsRight.z; }
          if (obsKeys['a'] || obsKeys['arrowleft']) { mx -= _obsRight.x; mz -= _obsRight.z; }
          const mlen = Math.hypot(mx, mz);
          const dx = mlen > 1e-6 ? (mx / mlen) * obsDist : 0;
          const dz = mlen > 1e-6 ? (mz / mlen) * obsDist : 0;
          let vy = 0;
          if (obsKeys['space']) vy += 1;
          if (obsKeys['shift']) vy -= 1;
          const dy = vy * obsDist;
          if (dx !== 0 || dz !== 0 || dy !== 0) {
            const nx = camera.position.x + dx;
            const ny = camera.position.y + dy;
            const nz = camera.position.z + dz;
            // 相机与 target 一起平移：相对偏移 (camera-target) 不变，
            // OrbitControls 不会把它解读成旋转/缩放，A/D 才是纯横移。
            // 观察者模式不做碰撞（用户要求），飞行可自由穿过模型。
            camera.position.set(nx, ny, nz);
            controls.target.x += dx;
            controls.target.y += dy;
            controls.target.z += dz;
          }
        }
        // 观察者模式：OrbitControls.target 限制在整座微缩场景范围内（留余量，见
        // OBS_TGT_MIN/MAX），既能飞到街区/便利店自由探索，又不会把模型拖丢。
        // 若钳制改变了 target，相机同步同样的位移——保持 (camera-target) 偏移不变，
        // 飞行/平移后不会突然"转一下视角"。
        const prevTx = controls.target.x, prevTy = controls.target.y, prevTz = controls.target.z;
        controls.target.x = THREE.MathUtils.clamp(controls.target.x, OBS_TGT_MIN.x, OBS_TGT_MAX.x);
        controls.target.y = THREE.MathUtils.clamp(controls.target.y, OBS_TGT_MIN.y, OBS_TGT_MAX.y);
        controls.target.z = THREE.MathUtils.clamp(controls.target.z, OBS_TGT_MIN.z, OBS_TGT_MAX.z);
        camera.position.x += controls.target.x - prevTx;
        camera.position.y += controls.target.y - prevTy;
        camera.position.z += controls.target.z - prevTz;
        controls.update();
        setOutlineDistanceScale(camera.position.distanceTo(controls.target));

        // 天花板俯视剖切（dollhouse 视角）：相机高过屋顶就把顶拿掉，
        // 从上面看进去是剖面，平视时屋顶照常。
        const camY = camera.position.y;
        const showCeiling = camY <= UNIT_LIFT + layout.room.height + 0.3;
        for (let i = 0; i < ceilingMeshes.length; i++) {
          ceilingMeshes[i].visible = showCeiling;
        }
        ceilingDecor.visible = showCeiling;

        // 墙板/墙装饰/洞口断面不再做相机侧显隐：203 的外墙后面现在是
        // 真实的公寓楼和街区（不是虚空），从街上看就该看到这户的外墙皮——
        // 它本来就是"公寓二楼亮着灯的那户"的立面。材质本就 DoubleSide，
        // 墙两面都画，外侧看到的是墙皮背面（米白，夜里读作公寓外墙）。
        for (let i = 0; i < wallMeshes.length; i++) wallMeshes[i].visible = true;
        for (const ws of shell.wallSides) {
          ws.decor.visible = true;
          ws.roomSideDecor.visible = true;
          ws.reveals.visible = true;
        }
      }
      // 点光预算（判据见 budgetPointLights 的注释）。池子只在装配完成后收一次；
      // 之后没有任何路径会新增点光（角色与家具的 GLB 都刻意不带灯），所以只有
      // 发现灯被摘走（父级没了）才重收。
      if (!pointPool || pointPool.some((l) => !l.parent)) {
        pointPool = [];
        scene.traverse((o) => {
          if ((o as THREE.PointLight).isPointLight) pointPool!.push(o as THREE.PointLight);
        });
      }
      budgetPointLights(camera, pointPool, POINT_LIGHT_BUDGET);

      if (fxOn) composer.render();
      else renderer.render(scene, camera);

      // 首帧真的画出来了：撤掉首屏 loading 层，并打印一次启动时间线。
      // 挂在「渲染完成之后」而不是 React 挂载处——挂载时 canvas 还是空的，
      // 那时撤掉 loading 层只是把空白从「有 loading 转圈」换成「没 loading 的黑屏」，
      // 观感反而更差。
      if (!firstFrameDone) {
        firstFrameDone = true;
        bootMark('scene:first-frame');
        dismissBootLoader();
        logBootTimeline();
      }

      /* 速度挡位标签：只在挡位变化时写 DOM——每帧写会白白触发样式重算。
       * 走路是默认态，不挂标签；只有跑 / 冲刺才显示，否则常驻一个字反而没信息量。
       * 挡位取值直接来自 fps.getSpeedTier()，与 update 里选 maxSpeed 的那套分支同源，
       * 不会出现"显示冲刺、实际按跑步速度走"这种漂移。 */
      {
        const tier = isFirstPerson ? fps.getSpeedTier() : 'walk';
        if (tier !== lastSpeedTier) {
          lastSpeedTier = tier;
          const el = hudSpeedRef.current;
          if (el) {
            if (tier === 'boost') {
              el.textContent = '冲刺';
              el.style.color = '#ffd9a0';
              el.style.opacity = '1';
            } else if (tier === 'sprint') {
              el.textContent = '跑';
              el.style.color = '#cfe6c4';
              el.style.opacity = '1';
            } else {
              el.style.opacity = '0';
            }
          }
        }
      }

      frameCount++;
      if (now - fpsT > 250) {
        const fpsVal = Math.round((frameCount * 1000) / (now - fpsT));
        const info = renderer.info.render;
        // 直接写 DOM，不走 React state：HUD 每 250ms 刷新一次，
        // 用 setState 会把整个组件（连带这个 effect 的闭包）每 250ms 重建一遍。
        if (hudStatsRef.current) {
          // 降过倍率就在读数后面标出来，否则"画质变软了"会被当成 bug
          const scaleTag = scaleIdx > 0 ? ` · ×${SCALE_STEPS[scaleIdx]}` : '';
          hudStatsRef.current.textContent =
            `${fpsVal} fps · ${info.calls} draw · ${Math.round(info.triangles / 1000)}k tri${scaleTag}`;
        }
        if (hudAgentsRef.current) {
          hudAgentsRef.current.textContent = agents
            .map((a) => `${a.id}  ${a.stateLabel}  ${a.pos.x.toFixed(1)},${a.pos.z.toFixed(1)}`)
            .join('\n');
        }
        if (fpsDebugRef.current) {
          fpsDebugRef.current.textContent = isFirstPerson
            ? `FPS y=${fps.dbgPlayerY.toFixed(2)} ground=${fps.dbgGroundY.toFixed(2)} ${fps.dbgGrounded ? 'GND' : 'AIR'}`
            : '';
        }
        if (hudDoorPromptRef.current) {
          // 仅关门且靠近时提示「F 打开」；开门后不再显示「F 关闭」按钮，
          // 玩家走远后门自动收回 manualOpen 并经 DOOR_HOLD 宽限自行关闭。
          if (isFirstPerson && nearDoor && nearDoor.current <= 0.5) {
            hudDoorPromptRef.current.textContent = 'F 打开';
            hudDoorPromptRef.current.style.opacity = '1';
          } else {
            hudDoorPromptRef.current.style.opacity = '0';
          }
        }
        // ---- 自适应渲染倍率：用刚结束的这个 250ms 统计窗的平均帧时判断 ----
        {
          const avgMs = (now - fpsT) / Math.max(1, frameCount);
          if (scaleHold > 0) scaleHold -= (now - fpsT) / 1000;
          // ≥120ms 一律不判：切后台 / 断点 / 软件光栅，不是持续负载
          if (scaleHold <= 0 && avgMs < 120) {
            if (avgMs > 33) { slowStreak++; fastStreak = 0; }
            else if (avgMs < 13) { fastStreak++; slowStreak = 0; }
            else { slowStreak = 0; fastStreak = 0; }
            if (slowStreak >= 4 && scaleIdx < SCALE_STEPS.length - 1) {
              applyRenderScale(scaleIdx + 1);
              lastDownAt = now;
              slowStreak = 0;
              scaleHold = 1.5;
            } else if (fastStreak >= 8 && scaleIdx > 0 && now - lastDownAt > 20000) {
              applyRenderScale(scaleIdx - 1);
              fastStreak = 0;
              scaleHold = 2.5;
            }
          }
        }
        frameCount = 0;
        fpsT = now;
      }
    };
    // 装配结束、循环即将启动。到这一刻为止主线程一直被占着，首屏 loading 层
    // 是屏幕上唯一动过的东西。'scene:built' → 'scene:first-frame' 之间是首帧
    // 渲染（含全部着色器变体的首次编译），通常是整条链里最尖的一根刺。
    bootMark('scene:built');
    render();

    /* ---------------- Cleanup ---------------- */

    return () => {
      alive = false;
      characterAnimations.forEach(animation=>animation?.dispose());
      disposeBlenderFurniture();
      apartmentLift.dispose();
      store.dispose();
      districtArt.dispose();
      interiorDesign.dispose();
      scene.environment = null;
      furnitureEnvironment.dispose();
      cancelAnimationFrame(raf);
      controls.dispose();
      fps.dispose();
      // composer 的 RT 是它自己建的，renderer.dispose() 不管。漏掉会留一对
      // HalfFloat + MSAA 的 renderbuffer——这两个是显存大户，StrictMode 双挂载
      // 就是两份
      composer.dispose();
      window.removeEventListener('keydown', onEnterKey);
      window.removeEventListener('keydown', onFxKey);
      window.removeEventListener('keydown', onDoorKey);
      window.removeEventListener('keydown', onObsKeyDown);
      window.removeEventListener('keyup', onObsKeyUp);
      window.removeEventListener('resize', onResize);
      document.removeEventListener('visibilitychange', onVisibility);
      // 阴影贴图是惰性创建的 WebGLRenderTarget，renderer.dispose() 不会释放，
      // 必须显式 dispose 灯光的 shadow.map，否则孤儿 RT 持续占显存
      scene.traverse((o: any) => {
        if (o.isLight && o.shadow?.map) o.shadow.map.dispose();
      });
      scene.traverse((o: any) => {
        if (o.geometry) o.geometry.dispose();
        if (o.material) {
          if (Array.isArray(o.material)) {
            o.material.forEach((mm: any) => {
              // 模块级缓存材质（toonCache / outlineMatCache）是共享单例，
              // 卸载时 dispose 会让缓存里的材质失效（program 反复重建），跳过
              if (!mm.userData?.__roomCached) mm.dispose();
            });
          } else if (!o.material.userData?.__roomCached) {
            o.material.dispose();
          }
        }
      });
      // GLB 贴图是每次加载的独立实例，随场景一起释放；
      // 程序化贴图 / gradientMap 是模块级单例，刻意不 dispose
      // （StrictMode 会挂载两次，第一次销毁贴图，第二次就拿到废图了）
      for (const t of gltfTextures) t.dispose();
      // 先 forceContextLoss 再 dispose：renderer.dispose() 只清 three 内部缓存，
      // 不归还 GL 上下文——某些驱动上（WebView2 集显）要等 GC/Chromium 逐出，
      // StrictMode 双挂载会留下孤儿 context
      renderer.forceContextLoss();
      renderer.dispose();
      if (renderer.domElement.parentNode === container) {
        container.removeChild(renderer.domElement);
      }
      // 注意：程序化贴图是模块级单例，这里刻意不 dispose ——
      // StrictMode 会挂载两次，第一次卸载时销毁贴图，第二次就拿到废图了。
      environmentApplyRef.current = null;
      (window as any).__ROOM__.setEnvironmentSource = undefined;
    };
  }, []);

  /* ---------------- 真实世界感知 → 房间环境 ---------------- */

  useEffect(() => {
    let cancelled = false;
    let timer = 0;
    // 只有推导结果与上一次不同才真正搬到场景上。这个 command 每次会整包回传
    // research / behaviors / beliefs，所以轮询要够省；而房间按时挂满几小时是
    // 常态，间隔又要够密才能让跨时段（尤其 dusk）及时跟上。
    let lastKey = `${environmentRef.current.period}|${environmentRef.current.weather}`;

    const tick = async () => {
      // 命令面板设置的手动环境优先于真实感知与测试注入，且每次轮询都保持。
      const manual = manualEnvironmentRef.current;
      if (manual) {
        environmentSourceRef.current = '命令';
        const key = `${manual.period}|${manual.weather}`;
        if (key !== lastKey) {
          lastKey = key;
          environmentApplyRef.current?.(manual.period, manual.weather);
        }
        // HUD 节点只在打开后才存在，故像常规路径一样无条件重刷文本。
        if (hudEnvRef.current) {
          hudEnvRef.current.textContent = describeEnvironment(manual, '命令');
        }
        return;
      }
      const override = environmentOverrideRef.current;
      let input: EnvironmentInput;
      let source: string;

      if (override) {
        // 测试注入优先，且拿不到网络也不该被覆盖。
        input = { ...environmentFromLocalClock(), ...override };
        source = '测试注入';
      } else {
        try {
          const res = await invoke<WorldSnapshotResponse>('get_world_snapshot');
          if (cancelled) return;
          const s = res?.snapshot;
          input = s
            ? {
                hour: s.hour,
                // 后端优先用天气 API 的日字段，缺两份都空时映射层会退到固定日照。
                sunriseHour: s.sunrise_sunset?.sunrise_hour ?? s.weather?.sunrise_hour ?? null,
                sunsetHour: s.sunrise_sunset?.sunset_hour ?? s.weather?.sunset_hour ?? null,
                weatherCode: s.weather?.weather_code ?? null,
              }
            : environmentFromLocalClock();
          source = s?.weather ? 'Open-Meteo' : '本地时钟';
        } catch {
          // 无 Tauri 上下文（room-preview.html）：没有后台可用，退到本地时钟。
          if (cancelled) return;
          input = environmentFromLocalClock();
          source = '本地时钟';
        }
      }

      environmentSourceRef.current = source;

      const env = resolveEnvironment(input);
      const key = `${env.period}|${env.weather}`;
      if (key !== lastKey) {
        lastKey = key;
        environmentApplyRef.current?.(env.period, env.weather);
      }
      // HUD 节点只在 HUD 打开后才存在（挂在 {hudVisible && ...} 里），所以每次
      // 都无条件重刷文本 —— 环境没变时只是重写同一个字符串，成本可忽略。
      if (hudEnvRef.current) {
        hudEnvRef.current.textContent = describeEnvironment(env, source);
      }
    };

    environmentTickRef.current = () => void tick();
    void tick();
    timer = window.setInterval(() => void tick(), 120_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      environmentTickRef.current = null;
    };
  }, []);

  /* ---------------- HUD ---------------- */

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'h' || e.key === 'H') {
        if (cmdOpenRef.current) return; // 命令面板输入时 h 是打字，不切 HUD
        setHudVisible((v) => !v);
        // 摘要节点随 HUD 才挂载，打开时补一次，否则第一帧是空行。
        environmentTickRef.current?.();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  /* ---------------- 命令面板：/ 呼出，ESC 关闭 ---------------- */

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const inInput = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);

      if (e.key === '/') {
        // 正在输入框里打字时不抢键；输入面板已开也不重复呼出。
        if (inInput || cmdOpenRef.current) return;
        e.preventDefault();
        // 帮助面板开着时按 /：关掉帮助、直接换到命令输入。
        if (cmdHelpRef.current) setCmdHelpState(false);
        setCmdOpenState(true);
        return;
      }
      if (e.key === 'Escape') {
        // 浮层开着时 ESC 只收浮层：先帮助面板、再输入面板，都不会关窗口。
        if (cmdHelpRef.current) {
          e.preventDefault();
          setCmdHelpState(false);
          return;
        }
        if (cmdOpenRef.current) {
          e.preventDefault();
          setCmdOpenState(false);
        }
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.clearTimeout(cmdFeedbackTimerRef.current);
      // 组件卸载兜底：面板可能正开着就卸载（StrictMode 双挂载 / 切走），
      // 残留的抑制会让下一次进房间 ESC 失效；watch_room_escape 起步也会复位，
      // 这里再补一次，保证两处都干净。
      void invoke('set_room_escape_suppressed', { suppressed: false }).catch(() => {});
    };
  }, [setCmdOpenState, setCmdHelpState]);

  /* ---------------- 帮助面板：外部点击关闭 ---------------- */

  useEffect(() => {
    if (!cmdHelpOpen) return;
    const onMouseDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      const el = cmdHelpPanelRef.current;
      if (el && t && !el.contains(t)) setCmdHelpState(false);
    };
    // 延迟一帧再挂监听：若这次打开是由「点击联想行执行 /help」触发的，这一记点击的
    // 事件流尚未结束，立即挂上会把同一个点击误判成「面板外点击」而秒关。rAF 后挂可避。
    const raf = requestAnimationFrame(() => window.addEventListener('mousedown', onMouseDown));
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('mousedown', onMouseDown);
    };
  }, [cmdHelpOpen, setCmdHelpState]);

  return (
    <div style={{ position: 'absolute', inset: 0, background: '#e7d8c4' }}>
      <div ref={containerRef} style={{ position: 'absolute', inset: 0 }} />

      {/* 时段/天气默认跟随真实世界感知；按 / 打开命令面板可手动切换（/day /rain 等） */}

      {hudVisible && (
        <div
          style={{
            position: 'absolute', top: 12, right: 12, padding: '10px 14px',
            background: 'rgba(255,252,246,0.86)', color: '#6b4f5e',
            border: '1px solid rgba(107,79,94,0.25)', borderRadius: 12,
            boxShadow: '0 4px 14px rgba(107,79,94,0.15)',
            fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
            fontSize: 12, lineHeight: 1.7, pointerEvents: 'none', minWidth: 168,
          }}
        >
          <div ref={hudStatsRef} style={{ fontWeight: 700, letterSpacing: 0.5 }} />
          <div style={{ height: 1, background: 'rgba(107,79,94,0.18)', margin: '6px 0' }} />
          <div ref={hudAgentsRef} style={{ whiteSpace: 'pre-line' }} />
          <div ref={fpsDebugRef} style={{ marginTop: 6, color: '#7fd1ff', fontFamily: 'ui-monospace, monospace' }} />
          <div ref={hudModeRef} style={{ marginTop: 6, color: '#C9A961', fontWeight: 600 }}>观察者模式 · Enter 进入</div>
          <div ref={hudEnvRef} style={{ marginTop: 4, color: '#8fa5c4', fontWeight: 600 }} />
          <div ref={hudCrosshairRef} style={{
            position: 'fixed', top: '50%', left: '50%',
            transform: 'translate(-50%, -50%)',
            pointerEvents: 'none', opacity: 0,
            transition: 'opacity 0.2s ease',
            zIndex: 10,
          }}>
            {/* 十字准星 */}
            <svg width="24" height="24" viewBox="0 0 24 24" style={{ display: 'block' }}>
              <line x1="12" y1="4" x2="12" y2="9" stroke="#C9A961" strokeWidth="1.5" strokeLinecap="round" opacity="0.85"/>
              <line x1="12" y1="15" x2="12" y2="20" stroke="#C9A961" strokeWidth="1.5" strokeLinecap="round" opacity="0.85"/>
              <line x1="4" y1="12" x2="9" y2="12" stroke="#C9A961" strokeWidth="1.5" strokeLinecap="round" opacity="0.85"/>
              <line x1="15" y1="12" x2="20" y2="12" stroke="#C9A961" strokeWidth="1.5" strokeLinecap="round" opacity="0.85"/>
              {/* 中心点 */}
              <circle cx="12" cy="12" r="1" fill="#C9A961" opacity="0.6"/>
            </svg>
          </div>
          <div style={{ marginTop: 6, opacity: 0.55, fontSize: 11 }}>
            WASD 移动 · 鼠标视角 · Ctrl/右键 跑 · 双击右键按住 冲刺
          </div>
          <div style={{ marginTop: 6, opacity: 0.55, fontSize: 11 }}>Space 跳 · Shift 蹲 · ESC 退出公寓 · 按 H 隐藏</div>
        </div>
      )}

      {/* 第一人称靠近门时的 "F 打开" 提示：固定位置，由渲染循环按 nearDoor 控制显隐 */}
      <div
        ref={hudDoorPromptRef}
        style={{
          position: 'fixed', left: '50%', bottom: 96,
          transform: 'translateX(-50%)',
          padding: '8px 16px',
          background: 'rgba(40, 30, 28, 0.72)',
          color: '#fff',
          borderRadius: 10,
          fontSize: 14, fontWeight: 700, letterSpacing: 1,
          pointerEvents: 'none', userSelect: 'none',
          zIndex: 16, whiteSpace: 'nowrap',
          opacity: 0, transition: 'opacity 0.15s ease',
        }}
      >
        F 打开
      </div>

      {/* 速度挡位标签。走路是默认态、不挂标签，只有跑 / 冲刺时由渲染循环点亮。
          没有它的话，双击右键这个手势是"隐形"的——玩家分不出自己到底进没进冲刺挡。 */}
      <div
        ref={hudSpeedRef}
        style={{
          position: 'fixed', right: 24, bottom: 96,
          padding: '5px 12px',
          background: 'rgba(40, 30, 28, 0.6)',
          color: '#ffd9a0',
          borderRadius: 8,
          fontSize: 12, fontWeight: 700, letterSpacing: 1.5,
          pointerEvents: 'none', userSelect: 'none',
          zIndex: 16, whiteSpace: 'nowrap',
          opacity: 0, transition: 'opacity 0.12s ease',
        }}
      />

      {/* 观察者模式右下角提示：复用速度挡位标签的同款样式，提示按 Enter 进入第一人称。
          第一人称下它不渲染，让位给渲染循环点亮的 跑 / 冲刺 标签（同一位置、互不重叠）。 */}
      {mode === 'observe' && (
        <div
          style={{
            position: 'fixed', right: 24, bottom: 96,
            padding: '5px 12px',
            background: 'rgba(40, 30, 28, 0.6)',
            color: '#ffd9a0',
            borderRadius: 8,
            fontSize: 12, fontWeight: 700, letterSpacing: 1.5,
            pointerEvents: 'none', userSelect: 'none',
            zIndex: 16, whiteSpace: 'nowrap',
          }}
        >
          ENTER键进入第一人称
        </div>
      )}

      {/* 命令执行结果 toast（终端风格，成功/失败都会弹，几秒后自动消失） */}
      {cmdFeedback && (
        <div
          style={{
            position: 'fixed', top: 16, left: '50%', transform: 'translateX(-50%)',
            padding: '8px 16px',
            background: 'rgba(8, 8, 12, 0.9)',
            color: cmdFeedback.ok ? '#7cf2a8' : '#ff8f7a',
            border: `1px solid ${cmdFeedback.ok ? 'rgba(124,242,168,0.5)' : 'rgba(255,143,122,0.5)'}`,
            borderRadius: 4,
            fontSize: 13,
            fontWeight: 600,
            letterSpacing: 1,
            fontFamily: "'Orbitron', 'Microsoft YaHei', 'Segoe UI', sans-serif",
            textShadow: cmdFeedback.ok
              ? '0 0 8px rgba(124,242,168,0.6)'
              : '0 0 8px rgba(255,143,122,0.6)',
            boxShadow: '0 0 12px rgba(0,0,0,0.4)',
            pointerEvents: 'none', userSelect: 'none',
            zIndex: 21, whiteSpace: 'nowrap',
            maxWidth: '80vw', overflow: 'hidden', textOverflow: 'ellipsis',
          }}
        >
          {cmdFeedback.text}
        </div>
      )}

      {/* /help 帮助面板：赛博朋克终端风格。/ 或 ESC 或点击面板外部关闭。
          与输入面板互斥；开着时同样抑制硬件 ESC，不会关掉房间窗口。 */}
      {cmdHelpOpen && (
        <div
          ref={cmdHelpPanelRef}
          style={{
            position: 'fixed', left: '50%', top: '10%', transform: 'translateX(-50%)',
            width: 420, maxWidth: 'calc(100vw - 48px)',
            maxHeight: '78vh', overflowY: 'auto',
            padding: '16px 18px 12px',
            background:
              'repeating-linear-gradient(0deg, rgba(124,242,168,0.035) 0 1px, transparent 1px 3px), rgba(8, 10, 14, 0.94)',
            border: '1px solid rgba(124, 242, 168, 0.45)',
            boxShadow: '0 0 24px rgba(124,242,168,0.22), inset 0 0 18px rgba(124,242,168,0.06)',
            borderRadius: 4,
            color: '#b8ffd9',
            fontFamily: "'Orbitron', 'Microsoft YaHei', 'Segoe UI', sans-serif",
            userSelect: 'none',
            zIndex: 22,
          }}
        >
          {/* 顶栏：标题 + 右侧装饰短线（无关闭按钮，靠外部点击 / ESC / / 关闭） */}
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, marginBottom: 12 }}>
            <div style={{ fontSize: 16, fontWeight: 700, letterSpacing: 3, color: '#7cf2a8',
              textShadow: '0 0 10px rgba(124,242,168,0.85)' }}>
              COMMAND&nbsp;//&nbsp;HELP
            </div>
            <div style={{
              display: 'flex', alignItems: 'center', gap: 3, opacity: 0.7,
              fontSize: 10, letterSpacing: 1.5, color: 'rgba(124,242,168,0.9)',
            }}>
              <span style={{ width: 14, height: 1, background: 'rgba(124,242,168,0.5)', boxShadow: '0 0 6px rgba(124,242,168,0.8)' }} />
              <span style={{ width: 6, height: 1, background: 'rgba(124,242,168,0.35)' }} />
              <span style={{ width: 3, height: 1, background: 'rgba(124,242,168,0.2)' }} />
            </div>
          </div>

          {/* 命令列表：按 时间/天气/系统 分区展示，组与组之间用霓虹分隔线隔开 */}
          {CMD_GROUPS.map((g, gi) => {
            const items = HELP_ENTRIES.filter((it) => it.group === g.key);
            if (items.length === 0) return null;
            return (
              <div key={g.key}>
                {/* 分区标题：短亮条 + 标签 + 通栏细线 */}
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 8,
                  marginTop: gi === 0 ? 2 : 14, marginBottom: 3,
                }}>
                  <span style={{
                    width: 12, height: 1,
                    background: 'rgba(124,242,168,0.55)',
                    boxShadow: '0 0 6px rgba(124,242,168,0.8)',
                  }} />
                  <span style={{
                    fontSize: 10, letterSpacing: 2.5, fontWeight: 700,
                    color: 'rgba(124,242,168,0.95)',
                    textShadow: '0 0 6px rgba(124,242,168,0.5)',
                  }}>
                    {g.label}
                  </span>
                  <span style={{ flex: 1, height: 1, background: 'rgba(124,242,168,0.16)' }} />
                </div>
                {items.map((it) => (
                  <div
                    key={it.cmd}
                    style={{
                      display: 'flex', alignItems: 'baseline', justifyContent: 'space-between',
                      gap: 12, padding: '5px 2px',
                      borderBottom: '1px solid rgba(124,242,168,0.08)',
                    }}
                  >
                    <span style={{ fontWeight: 700, letterSpacing: 1.5, fontSize: 13,
                      color: '#7cf2a8', textShadow: '0 0 7px rgba(124,242,168,0.6)', whiteSpace: 'nowrap' }}>
                      {it.cmd}
                    </span>
                    <span style={{ fontSize: 12, opacity: 0.72, textAlign: 'right' }}>{it.desc}</span>
                  </div>
                ))}
              </div>
            );
          })}

          {/* 底部提示 */}
          <div style={{ marginTop: 10, fontSize: 10.5, letterSpacing: 1.5, opacity: 0.5,
            textAlign: 'right' }}>
            点击面板外部 / ESC 关闭&nbsp;&nbsp;·&nbsp;&nbsp;按 / 重新呼出
          </div>
        </div>
      )}

      {/* Minecraft 风格命令面板：左下角小黑框，/ 呼出，Enter 执行，ESC 关闭。
          输入框无自动前缀、无提示文本；字母走赛博朋克字形。上方上拉联想卡片
          随输入实时过滤命令，↑↓ 选择、ENTER 执行选中项。 */}
      {cmdOpen && (
        <div style={{ position: 'fixed', left: 24, bottom: 28, zIndex: 20 }}>
          {/* 上拉联想卡片（搜索引擎式）：有匹配才出现，置于输入框正上方 */}
          {cmdMatches.length > 0 && (
            <div
              style={{
                position: 'absolute', left: 0, bottom: '100%', marginBottom: 8,
                width: 340, maxWidth: 'calc(100vw - 48px)',
                background:
                  'repeating-linear-gradient(0deg, rgba(124,242,168,0.03) 0 1px, transparent 1px 3px), rgba(8,10,14,0.95)',
                border: '1px solid rgba(124,242,168,0.4)',
                boxShadow: '0 0 18px rgba(124,242,168,0.18)',
                borderRadius: 4,
                overflow: 'hidden',
                fontFamily: "'Orbitron', 'Microsoft YaHei', 'Segoe UI', sans-serif",
                userSelect: 'none',
              }}
            >
              <div style={{ padding: '5px 10px', fontSize: 10, letterSpacing: 2, opacity: 0.5,
                borderBottom: '1px solid rgba(124,242,168,0.15)' }}>
                MATCH&nbsp;//&nbsp;命令联想
              </div>
              {cmdMatches.map((it, i) => (
                <div
                  key={it.cmd}
                  onMouseEnter={() => { setCmdActiveIdx(i); }}
                  onClick={() => { executeCommand(it.cmd); }}
                  style={{
                    display: 'flex', alignItems: 'baseline', justifyContent: 'space-between',
                    gap: 12, padding: '7px 10px', cursor: 'pointer',
                    borderLeft: `3px solid ${i === cmdActiveIdx ? '#7cf2a8' : 'transparent'}`,
                    background: i === cmdActiveIdx ? 'rgba(124,242,168,0.12)' : 'transparent',
                    transition: 'background 0.08s ease',
                  }}
                >
                  <span style={{ fontWeight: 700, letterSpacing: 1.5, fontSize: 13,
                    color: 'rgba(236, 246, 255, 0.92)', whiteSpace: 'nowrap' }}>
                    {renderCmdName(it.cmd)}
                  </span>
                  <span style={{ fontSize: 11, opacity: 0.65, textAlign: 'right', whiteSpace: 'nowrap' }}>
                    {it.desc}
                  </span>
                </div>
              ))}
              <div style={{ padding: '4px 10px', fontSize: 9.5, letterSpacing: 1, opacity: 0.4,
                borderTop: '1px solid rgba(124,242,168,0.12)' }}>
                ↑↓ 选择&nbsp;&nbsp;·&nbsp;&nbsp;ENTER 执行选中&nbsp;&nbsp;·&nbsp;&nbsp;ESC 关闭
              </div>
            </div>
          )}

          {/* 输入小黑框本体 */}
          <div
            style={{
              width: 340, maxWidth: 'calc(100vw - 48px)',
              padding: '10px 12px',
              background: 'rgba(8, 8, 12, 0.92)',
              border: '1px solid rgba(120, 240, 160, 0.35)',
              boxShadow: '0 0 14px rgba(120, 240, 160, 0.18), inset 0 0 10px rgba(120, 240, 160, 0.05)',
              borderRadius: 4,
              fontFamily: "'Orbitron', 'Share Tech Mono', 'Consolas', monospace",
              color: '#b8ffd9',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <span style={{ color: '#5cf2a6', fontWeight: 700, fontSize: 14, textShadow: '0 0 6px rgba(92,242,166,0.8)' }}>&gt;</span>
              {/* 区块光标层：输入框本身文字/原生光标设为透明，由这层幽灵文本 + 块状光标
                  代绘（Minecraft 终端风格）。字体样式与下层 input 完全一致才能对齐。 */}
              <div style={{ position: 'relative', flex: 1, minWidth: 0, height: 20 }}>
                <div style={{
                  position: 'absolute', left: 0, right: 0, top: 0, bottom: 0,
                  display: 'flex', alignItems: 'center',
                  fontFamily: 'inherit', fontSize: 14, fontWeight: 700, letterSpacing: 2,
                  color: '#b8ffd9', textShadow: '0 0 8px rgba(120,240,160,0.7)',
                  overflow: 'hidden', whiteSpace: 'nowrap', pointerEvents: 'none',
                }}>
                  <span>{cmdInput}</span>
                  <span style={{
                    color: '#7cf2a8',
                    textShadow: '0 0 9px rgba(92,242,166,0.95)',
                    animation: 'cmdBlockCursor 1s steps(2, start) infinite',
                    marginLeft: 1,
                  }}>▮</span>
                </div>
                <input
                  ref={cmdInputRef}
                  autoFocus
                  value={cmdInput}
                  spellCheck={false}
                  autoComplete="off"
                  maxLength={20}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      // 有联想且已高亮 → 执行高亮项；否则执行原样输入。
                      const exec = cmdMatches.length > 0 && cmdActiveIdx >= 0
                        ? cmdMatches[cmdActiveIdx].cmd
                        : cmdInput;
                      // 必须同时 stopPropagation：这条 keydown 还要继续冒泡到 window
                      // 上的 onEnterKey（进入第一人称）。命令成功会同步收起面板、把
                      // cmdOpenRef 翻成 false，没挡住的话同一记回车就顺手锁了指针。
                      e.preventDefault();
                      e.stopPropagation();
                      executeCommand(exec);
                      return;
                    }
                    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                      if (cmdMatches.length === 0) return;
                      e.preventDefault();
                      setCmdActiveIdx((i) => {
                        const last = cmdMatches.length - 1;
                        if (e.key === 'ArrowDown') return i >= last ? 0 : i + 1;
                        return i <= 0 ? last : i - 1;
                      });
                      return;
                    }
                    // 严格白名单：单字符按键只放行 26 个英文字母（含大小写）。
                    // 其余字符（含 /、空格、数字、符号）一律屏蔽；方向键/退格/
                    // 删除等编辑键（e.key.length>1）放行。
                    if (e.key.length === 1 && !/[a-zA-Z]/.test(e.key)) {
                      e.preventDefault();
                    }
                  }}
                  onBeforeInput={(e) => {
                    // 兜底：IME 组词、拖拽插入会绕过 keydown，直接挡掉非字母的插入内容
                    const data = (e.nativeEvent as InputEvent).data;
                    if (typeof data === 'string' && data !== '' && /[^a-zA-Z]/.test(data)) {
                      e.preventDefault();
                    }
                  }}
                  onChange={(e) => {
                    // 兜底第二层：粘贴可能整段进来，这里把非字母再滤一遍（纯字母输入）。
                    // 受控输入，清理后立即驱动联想重算；输入变化时清掉旧高亮。
                    const clean = e.target.value.replace(/[^a-zA-Z]/g, '');
                    setCmdInput(clean);
                    setCmdActiveIdx(-1);
                  }}
                  style={{
                    position: 'absolute', left: 0, right: 0, top: 0, bottom: 0, width: '100%',
                    background: 'transparent', border: 'none', outline: 'none',
                    // 文字与本机光标都藏掉，由幽灵层代绘；font 与幽灵层同款
                    color: 'transparent', caretColor: 'transparent',
                    fontFamily: 'inherit', fontSize: 14, fontWeight: 700, letterSpacing: 2,
                  }}
                />
              </div>
            </div>
            {/* 区块光标闪烁动画 + 联想卡片同上（面板内联注入，面板关掉即失效） */}
            <style>{`@keyframes cmdBlockCursor { 0%,49% { opacity: 1 } 50%,100% { opacity: 0 } }`}</style>
          </div>
        </div>
      )}
    </div>
  );
}
