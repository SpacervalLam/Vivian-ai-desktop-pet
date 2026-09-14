/**
 * 日式赛璐璐（Cel-shading）渲染工具箱。
 *
 * 三件事：
 *  1. 阶梯渐变图 —— MeshToonMaterial 靠它把连续光照压成 2~4 级色阶，这是"动画感"的根
 *  2. 描边 —— 反面外扩（inverted hull）。先合并顶点、平均法线，否则立方体的角上
 *     法线是分裂的，外扩后会在每条棱上裂开缝。描边分两级：主结构全厚度、
 *     次结构细线，分级打在材质上（userData.outlineWeight），发光体不描
 *  3. 程序化贴图 —— 全部用 canvas 现画。不引入任何二进制美术资源，包体零增长，
 *     且风格统一（日式动画的背景本来就偏"手绘平涂"，程序化反而比照片贴图更贴）
 *
 * 所有贴图都是模块级单例，重复调用不重复生成。
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

/* ============================================================================
 * 1. 色阶渐变图
 * ========================================================================== */

let gradientTex: THREE.DataTexture | null = null;

/**
 * 4 级色阶。暗部刻意不压到 0 —— 日式动画的阴影是有色相的"第二层颜色"，
 * 死黑阴影会让整个画面变成廉价 3D 感。
 *
 * 描边变细之后，色阶自身的对比就是形体边界的主要承载者，所以暗部压得稍深、中间
 * 调拉宽一点点，立体感更清楚。
 */
export function toonGradient(): THREE.DataTexture {
  if (gradientTex) return gradientTex;
  /**
   * 色阶的暗端决定整张画面的"硬"程度。
   *
   * 0.50 起步意味着背光面直接砍掉一半亮度，明暗交界是一条突变的线——
   * 这是赛璐璐动画的做法，但本次要的是日本动画背景美术那种柔和分层。
   * 暗端提到 0.66、并把中间两档的间距压匀，阴影就变成"压暗但仍透光"，
   * 物体体积还在，硬边消失。
   */
  const ramp = [0.66, 0.81, 0.93, 1.0];
  const data = new Uint8Array(ramp.map((v) => Math.round(v * 255)));
  const tex = new THREE.DataTexture(data, ramp.length, 1, THREE.RedFormat);
  tex.minFilter = THREE.NearestFilter;
  tex.magFilter = THREE.NearestFilter;
  tex.generateMipmaps = false;
  tex.needsUpdate = true;
  gradientTex = tex;
  return tex;
}

export type ToonOpts = {
  map?: THREE.Texture | null;
  emissive?: THREE.ColorRepresentation;
  emissiveIntensity?: number;
  transparent?: boolean;
  opacity?: number;
  side?: THREE.Side;
  depthWrite?: boolean;
  /**
   * 材质分档：决定这块表面"怎么反光"。
   *
   * MeshToonMaterial 本体只有色阶，没有粗糙度/金属度——所有表面在光下的响应
   * 是同一个，这正是画面"看不出材质差别"的根因。finish 往 shader 里注入一段
   * 风格化的 specular + rim，让不同材质拿到不同的高光响应：
   *   matte  不注入（默认，木头/布/墙/纸，行为与从前完全一致，零风险）
   *   soft   极弱边缘亮（塑料、陶瓷、漆面）
   *   metal  硬高光 + 中等亮边（五金、支脚、栏杆、灯杆）
   *   glass  强亮边 + 中高光（玻璃门、酒柜玻璃）
   *   wet    软高光 + 冷色亮边（湿面，备用）
   *
   * 注入是纯 GLSL 常量烘焙（见 FINISH_PRESETS），不带自定义 uniform ——
   * three 对缓存命中的材质不会重跑 onBeforeCompile，走 uniform 的话同一档位里
   * 只有第一个材质的值会生效，其余全静默漏掉。烘焙成常量后同档共享 program
   * 是安全的：program 即档位，没有可变状态。
   */
  finish?: ToonFinish;
  /**
   * 描边分级（0/1/2）。不填则按 finish 推导（见 defaultOutlineWeight）：
   *   2 主结构/外轮廓——全厚度（默认，木/布/墙/漆面）
   *   1 次结构——细线（五金、湿面这类小件桶）
   *   0 不描——发光体（屏幕/灯带/灯泡）。黑壳会把 bloom 的光晕整个闷在
   *     轮廓里，发光物描了边就永远不会"溢出来"。
   * 分级打在材质上而不是零件上：合批按材质并桶，桶的材质引用合批后还在，
   * 所以 addOutline 按材质分桶建壳即可，实体合批完全不用动。
   */
  outlineWeight?: OutlineWeight;
  /**
   * 独占一份材质，不走共享缓存。
   * 拿到的实例如果要就地改参数（改 shadowSide、改 map.repeat 之类），必须设这个，
   * 否则改动会顺着缓存流到所有同参数的物体上。
   */
  unique?: boolean;
};

/* ============================================================================
 * 1.5 材质分档（finish）
 * ========================================================================== */

export type ToonFinish = 'matte' | 'soft' | 'metal' | 'glass' | 'wet';

/** 描边分级：0 不描 / 1 次结构细线 / 2 主结构全厚度。 */
export type OutlineWeight = 0 | 1 | 2;

/**
 * finish → 默认描边分级。五金/湿面是"小零件"语义，细线；玻璃靠自身
 * rim 读形状（且玻璃面基本都是透明的，透明面本来就不描）；其余全厚度。
 */
function defaultOutlineWeight(finish?: ToonFinish): OutlineWeight {
  switch (finish) {
    case 'metal':
    case 'wet':
      return 1;
    case 'glass':
      return 0;
    default:
      return 2;
  }
}

/**
 * 主光方向（世界空间，指向光源）。
 *
 * specular 的半程向量需要"表面指向光源"的方向。主光位置来自 layout.lighting.key，
 * 是静态的（本次不做昼夜变化），所以直接烘焙成 shader 常量，不搞 uniform。
 * 必须在**任何带 finish 的 toon() 调用之前**设好——RoomScene 在 setArtStyle 之后、
 * 造任何家具之前调用 setToonKeyLight。
 */
const KEY_LIGHT_DIR = new THREE.Vector3(0.42, 0.72, 0.55).normalize();

export function setToonKeyLight(x: number, y: number, z: number): void {
  KEY_LIGHT_DIR.set(x, y, z).normalize();
}

type FinishPreset = {
  rimLow: number;
  rimHigh: number;
  rimStrength: number;
  rimColor: string;
  /** 半程向量与法线点积的硬切阈值，越接近 1 高光越小越锐。 */
  specCut: number;
  specStrength: number;
  specColor: string;
};

const FINISH_PRESETS: Record<Exclude<ToonFinish, 'matte'>, FinishPreset> = {
  soft: { rimLow: 0.66, rimHigh: 0.94, rimStrength: 0.2, rimColor: '#fff6e8', specCut: 0.992, specStrength: 0.0, specColor: '#ffffff' },
  metal: { rimLow: 0.5, rimHigh: 0.88, rimStrength: 0.4, rimColor: '#fff1d8', specCut: 0.968, specStrength: 0.55, specColor: '#fff4e0' },
  glass: { rimLow: 0.38, rimHigh: 0.8, rimStrength: 0.58, rimColor: '#cfe2ff', specCut: 0.978, specStrength: 0.35, specColor: '#e8f2ff' },
  wet: { rimLow: 0.52, rimHigh: 0.9, rimStrength: 0.34, rimColor: '#dbe8ff', specCut: 0.958, specStrength: 0.28, specColor: '#e6efff' },
};

/** hex → GLSL vec3 字面量（linear 感知无所谓：这里注入的是光上加色，按 sRGB 直取已够用）。 */
function glslVec3(hex: string): string {
  const c = new THREE.Color(hex);
  return `vec3(${c.r.toFixed(5)}, ${c.g.toFixed(5)}, ${c.b.toFixed(5)})`;
}

/**
 * 拼出这一档的注入代码。两处落点：
 *  - 常量声明插在 `#include <common>` 之后（全局作用域，main 之前）
 *  - 计算插在 `#include <opaque_fragment>` 之前（此时 outgoingLight 刚拼完，
 *    还没做色调映射和 sRGB 转换——高光必须加在线性空间，加了顺序就错了）
 *
 * 可用变量（都由 toon 模板保证存在）：normal（view space，normal_fragment_begin
 * 定义的）、vViewPosition（lights_toon_pars_fragment 定义的 = -mvPosition.xyz，
 * 所以 normalize 后就是"表面指向相机"）、viewMatrix（three 片元内建 uniform）。
 */
function finishShaderPatch(finish: Exclude<ToonFinish, 'matte'>): { decl: string; code: string } {
  const p = FINISH_PRESETS[finish];
  const decl = `
  // —— finish: ${finish}（风格化 specular + rim，常量烘焙，无 uniform）——
  const vec3 finishKeyDirW = vec3(${KEY_LIGHT_DIR.x.toFixed(5)}, ${KEY_LIGHT_DIR.y.toFixed(5)}, ${KEY_LIGHT_DIR.z.toFixed(5)});
  const vec3 finishSpecColor = ${glslVec3(p.specColor)};
  const vec3 finishRimColor = ${glslVec3(p.rimColor)};
  const float finishRimLow = ${p.rimLow.toFixed(4)};
  const float finishRimHigh = ${p.rimHigh.toFixed(4)};
  const float finishRimStrength = ${p.rimStrength.toFixed(4)};
  const float finishSpecCut = ${p.specCut.toFixed(4)};
  const float finishSpecStrength = ${p.specStrength.toFixed(4)};
`;
  const code = `
  {
    vec3 finishViewDir = normalize( vViewPosition );
    vec3 finishLightDir = normalize( ( viewMatrix * vec4( finishKeyDirW, 0.0 ) ).xyz );
    float finishNdh = saturate( dot( normal, normalize( finishViewDir + finishLightDir ) ) );
    float finishSpec = step( finishSpecCut, finishNdh );
    float finishFres = 1.0 - saturate( dot( normal, finishViewDir ) );
    float finishRim = smoothstep( finishRimLow, finishRimHigh, finishFres );
    outgoingLight += finishSpecColor * ( finishSpec * finishSpecStrength )
                   + finishRimColor * ( finishRim * finishRimStrength );
  }
`;
  return { decl, code };
}

function createToon(color: THREE.Color, opts: ToonOpts): THREE.MeshToonMaterial {
  const m = new THREE.MeshToonMaterial({
    color,
    gradientMap: toonGradient(),
    map: opts.map ?? null,
    transparent: opts.transparent ?? false,
    opacity: opts.opacity ?? 1,
    side: opts.side ?? THREE.FrontSide,
    depthWrite: opts.depthWrite ?? true,
  });
  if (opts.emissive !== undefined) {
    m.emissive = new THREE.Color(opts.emissive);
    m.emissiveIntensity = opts.emissiveIntensity ?? 1;
  }
  // 分级跟着材质走：合批后实体 mesh 只留材质引用，weight 必须从材质上读回来
  m.userData.outlineWeight = opts.outlineWeight ?? defaultOutlineWeight(opts.finish);
  if (opts.finish && opts.finish !== 'matte') {
    const patch = finishShaderPatch(opts.finish);
    m.onBeforeCompile = (shader) => {
      shader.fragmentShader = shader.fragmentShader
        .replace('#include <common>', '#include <common>\n' + patch.decl)
        .replace('#include <opaque_fragment>', patch.code + '\n\t#include <opaque_fragment>');
    };
    /**
     * 档位必须进 program 缓存键。three 默认拿 onBeforeCompile.toString() 当键，
     * 而各档位的 onBeforeCompile 是同一个函数——不区分的话不同档位会复用同一个
     * program，先编译的那档的注入会顶掉其他档。
     */
    m.customProgramCacheKey = () => `room-toon-${opts.finish}`;
  }
  return m;
}

/**
 * 同参数的材质共享一个实例。
 *
 * 房间里的实体 mesh 有两千多个，而"木头米色""布料粉""金属灰"这些颜色组合
 * 拢共几十种。逐个 new 出来的材质在渲染时既无法合批，又让渲染队列的排序
 * （按材质 id 分组）退化成两千多个单元素组，这部分 CPU 开销比 GPU 还贵。
 */
const toonCache = new Map<string, THREE.MeshToonMaterial>();

/** 统一入口：房间里的所有实体表面都走这里，保证 gradientMap 一定挂上。 */
export function toon(color: THREE.ColorRepresentation, opts: ToonOpts = {}): THREE.MeshToonMaterial {
  const c = new THREE.Color(color);
  if (opts.unique) return createToon(c, opts);

  const key = [
    c.getHexString(),
    opts.map?.uuid ?? '-',
    opts.emissive === undefined ? '-' : new THREE.Color(opts.emissive).getHexString(),
    opts.emissiveIntensity ?? 1,
    opts.opacity ?? 1,
    opts.transparent ? 1 : 0,
    opts.side ?? THREE.FrontSide,
    opts.depthWrite === false ? 0 : 1,
    // finish 改的是注入的 shader 源码，不同档位绝不能共用材质实例
    opts.finish ?? 'matte',
    // 显式覆盖分级的材质不能和默认分级的同参数材质共享实例，否则先建的那方赢
    opts.outlineWeight ?? '-',
  ].join('|');

  let mat = toonCache.get(key);
  if (!mat) {
    mat = createToon(c, opts);
    // 标记共享缓存材质：场景卸载做 dispose 遍历时必须跳过，
    // 否则缓存单例被销毁后仍留在 Map 里，下次重建白白浪费 program 重建
    mat.userData.__roomCached = true;
    toonCache.set(key, mat);
  }
  return mat;
}

/**
 * 自发光平面（屏幕、灯泡、天空）：不吃光照，永远亮。
 *
 * noFog：窗外那张远景（晴天/夜景）是画在墙后的一张"背景画"，它自己在画面里
 * 就已经带着空气透视。再让场景雾罩一层，等于给远景叠两次雾，窗外会糊成一片
 * 和背景同色的灰。这类材质要显式退出雾。
 */
export function emissive(
  color: THREE.ColorRepresentation,
  opts: {
    map?: THREE.Texture;
    opacity?: number;
    transparent?: boolean;
    side?: THREE.Side;
    noFog?: boolean;
    /** 发光体默认不描边（weight 0），个别需要轮廓的可以显式拉回 1/2。 */
    outlineWeight?: OutlineWeight;
  } = {}
): THREE.MeshBasicMaterial {
  const mat = new THREE.MeshBasicMaterial({
    color,
    map: opts.map ?? null,
    transparent: opts.transparent ?? false,
    opacity: opts.opacity ?? 1,
    side: opts.side ?? THREE.FrontSide,
  });
  if (opts.noFog) mat.fog = false;
  mat.userData.outlineWeight = opts.outlineWeight ?? 0;
  return mat;
}

/* ============================================================================
 * 2. 描边
 * ========================================================================== */

/**
 * 所有描边材质共享同一个 uniform 对象引用 —— 改一次，全场生效。
 *
 * 为什么需要：外扩是在**世界空间**里按固定长度做的，相机拉近时描边会跟着变粗、
 * 拉远时变细到看不见。微缩模型是要让人凑近看的，这个不一致特别扎眼。
 * 每帧按"相机到焦点的距离 / 参考距离"写一次 uDist，描边就恒定占同样的像素数。
 */
const OUTLINE_DIST = { value: 1 };

/** 参考距离：等于构图默认机位到焦点的距离（见 dormLayout.camera）。 */
const OUTLINE_REF_DIST = 5.9;

export function setOutlineDistanceScale(dist: number): void {
  OUTLINE_DIST.value = THREE.MathUtils.clamp(dist / OUTLINE_REF_DIST, 0.45, 2.6);
}

/**
 * 描边也吃雾。
 *
 * 描边是自定义 ShaderMaterial，three 不会自动给它注入雾。不接的话会出现一个很
 * 难形容但一眼能看出的破绽：远处的家具表面褪向雾色，描边却还是原样的深褐，
 * 物体越远轮廓线越"跳"，像贴了一圈黑边。所以这里手动把 fog chunk 引进来，
 * 让描边和它所属的表面一起褪。
 *
 * 变量名叫 mvPosition 不是随意起的 —— fog_vertex 这个 chunk 里写的是
 * `vFogDepth = - mvPosition.z`，必须有一个同名变量在作用域里，否则编译不过。
 */
const OUTLINE_VS = /* glsl */ `
  #include <common>
  #include <fog_pars_vertex>
  uniform float uThickness;
  uniform float uDist;
  void main() {
    vec3 inflated = position + normalize(normal) * uThickness * uDist;
    vec4 mvPosition = modelViewMatrix * vec4(inflated, 1.0);
    gl_Position = projectionMatrix * mvPosition;
    #include <fog_vertex>
  }
`;

const OUTLINE_FS = /* glsl */ `
  #include <common>
  #include <fog_pars_fragment>
  uniform vec3 uColor;
  void main() {
    gl_FragColor = vec4(uColor, 1.0);
    #include <fog_fragment>
  }
`;

/**
 * 把顶点按位置合并、法线求平均。
 * BoxGeometry 每面法线独立，直接在它的 8 个角上外扩会得到 3 个互相错开的角，
 * 描边就散了。合并之后每个顶点只有一个"外扩方向"，角上才会连成一条完整的线。
 */
const smoothCache = new WeakMap<THREE.BufferGeometry, THREE.BufferGeometry>();

export function smoothNormalGeometry(src: THREE.BufferGeometry): THREE.BufferGeometry {
  const cached = smoothCache.get(src);
  if (cached) return cached;

  const g = src.index ? src.toNonIndexed() : src.clone();
  const pos = g.getAttribute('position');
  const nor = g.getAttribute('normal');
  if (!pos || !nor) {
    smoothCache.set(src, g);
    return g;
  }

  const count = pos.count;
  const lookup = new Map<string, number>();
  const positions: number[] = [];
  const accum: Array<[number, number, number]> = [];
  const remap = new Array<number>(count);

  for (let i = 0; i < count; i++) {
    const x = pos.getX(i);
    const y = pos.getY(i);
    const z = pos.getZ(i);
    const key = `${x.toFixed(4)}|${y.toFixed(4)}|${z.toFixed(4)}`;
    let idx = lookup.get(key);
    if (idx === undefined) {
      idx = positions.length / 3;
      lookup.set(key, idx);
      positions.push(x, y, z);
      accum.push([0, 0, 0]);
    }
    remap[i] = idx;
    const a = accum[idx];
    a[0] += nor.getX(i);
    a[1] += nor.getY(i);
    a[2] += nor.getZ(i);
  }

  const normals: number[] = [];
  for (const a of accum) {
    const len = Math.hypot(a[0], a[1], a[2]) || 1;
    normals.push(a[0] / len, a[1] / len, a[2] / len);
  }

  const out = new THREE.BufferGeometry();
  out.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3));
  out.setAttribute('normal', new THREE.Float32BufferAttribute(normals, 3));
  out.setIndex(remap);

  if (src.index) g.dispose();
  smoothCache.set(src, out);
  return out;
}

export function outlineMaterial(thickness: number, color: THREE.ColorRepresentation): THREE.ShaderMaterial {
  /**
   * uniforms 只 clone 雾的那几个，自定义的三个手动挂上去。
   *
   * 不能用 UniformsUtils.merge 一把梭 —— merge 会对每个 uniform 做深拷贝，
   * uDist 那个共享引用会被复制成一份孤立的 { value: 1 }，
   * "改一次全场描边跟着变"的机制就断了。uDist 必须是同一个对象引用。
   */
  const uniforms = THREE.UniformsUtils.clone(THREE.UniformsLib.fog) as Record<string, THREE.IUniform>;
  uniforms.uThickness = { value: thickness };
  uniforms.uColor = { value: new THREE.Color(color) };
  uniforms.uDist = OUTLINE_DIST;

  return new THREE.ShaderMaterial({
    uniforms,
    vertexShader: OUTLINE_VS,
    fragmentShader: OUTLINE_FS,
    side: THREE.BackSide,
    // ShaderMaterial 默认 fog=false，不开这个 three 就不会定义 USE_FOG，
    // 上面引的 chunk 会整段被预处理掉，也不会来刷 fogColor/fogNear/fogFar
    fog: true,
  });
}

/** 同参数的描边材质共享一个实例：全场就厚度和颜色两种组合，没必要一人一份。 */
const outlineMatCache = new Map<string, THREE.ShaderMaterial>();

function sharedOutlineMaterial(thickness: number, color: THREE.ColorRepresentation): THREE.ShaderMaterial {
  const key = `${thickness.toFixed(6)}|${new THREE.Color(color).getHexString()}`;
  let mat = outlineMatCache.get(key);
  if (!mat) {
    mat = outlineMaterial(thickness, color);
    mat.userData.__roomCached = true;
    outlineMatCache.set(key, mat);
  }
  return mat;
}

/**
 * 描边分级的厚度系数。weight 2 是全厚度（调用方给的 thickness），
 * weight 1 收细——次结构线太粗会跟外轮廓抢戏，画面就回到"所有边一样粗"
 * 的老样子。weight 0 在这里没有系数：根本不建壳。
 */
const OUTLINE_WEIGHT_FACTOR: Record<1 | 2, number> = { 1: 0.55, 2: 1 };

/** 从 mesh 的材质上读描边分级。材质没带标记（比如直接 new 出来的）按全厚度处理。 */
function outlineWeightOf(mesh: THREE.Mesh): OutlineWeight {
  const mat = mesh.material as THREE.Material | THREE.Material[] | undefined;
  const m0 = Array.isArray(mat) ? mat[0] : mat;
  const w = m0?.userData?.outlineWeight;
  return w === 0 || w === 1 ? w : 2;
}

/**
 * 给 root 下所有实体挂描边。
 *
 * 一件道具的所有描边按**分级**各合成一个几何挂在 root 下（weight 1 一个、
 * weight 2 一个），而不是逐个挂在各自的实体 mesh 下面。两种挂法渲染结果
 * 一致（描边本来就是 renderOrder=-1 的反面外扩壳，先于实体绘制），但
 * draw call 从"实体数"降到"分级数" —— 之前一个书架要 130 次描边提交，
 * 现在最多两次（主结构 + 次结构）。
 *
 * 分级依据是材质上的 userData.outlineWeight（toon/emissive 出厂时打好），
 * 不是零件身份：合批之后零件身份已经丢了，但桶的材质引用还在，按材质
 * 分桶是唯一不拆实体合批的分级方式。weight 0（发光体）直接不建壳。
 *
 * 代价：变换要烘进顶点，所以描边不再跟随单个 mesh 动。房间里的家具全是
 * 钉死的，用不上跟随；角色本来就不描边，也不受影响。
 */
export function addOutline(
  root: THREE.Object3D,
  thickness = 0.012,
  color: THREE.ColorRepresentation = 0x5b4250
): THREE.Object3D {
  const targets: THREE.Mesh[] = [];
  root.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.name === '__outline') return;
    if (m.userData?.noOutline) return;
    // 透明面不能描边。描边是不透明的 BackSide 外扩壳，它铺满整个轮廓：不透明物体
    // 会把壳盖住、只留下一圈边，透明物体盖不住——壳会从透明面后面整片透上来。
    // 0.12 的玻璃门因此看着就是一块实心深紫板，完全不是透明材质该有的样子。
    // 透明面改由自身的高光/边缘亮（finish:'glass'）读出形状，不需要描边。
    const mat = m.material as THREE.Material | THREE.Material[] | undefined;
    if (Array.isArray(mat) ? mat.some((mm) => mm.transparent) : mat?.transparent) return;
    targets.push(m);
  });
  if (!targets.length) return root;

  // 变换烘焙：描边合并成一个后没有各自的父级可继承，只能写进顶点
  root.updateMatrixWorld(true);
  const invRoot = root.matrixWorld.clone().invert();
  const rel = new THREE.Matrix4();

  // 按分级分桶：同一级一个壳，各用各的厚度
  const buckets = new Map<1 | 2, THREE.BufferGeometry[]>();
  for (const mesh of targets) {
    const w = outlineWeightOf(mesh);
    if (w === 0) continue; // 发光体不描——黑壳会把 bloom 的光晕闷死在轮廓里
    const geo = smoothNormalGeometry(mesh.geometry).clone();
    rel.multiplyMatrices(invRoot, mesh.matrixWorld);
    geo.applyMatrix4(rel);
    const list = buckets.get(w);
    if (list) list.push(geo);
    else buckets.set(w, [geo]);
  }

  for (const [w, geos] of buckets) {
    const material = sharedOutlineMaterial(thickness * OUTLINE_WEIGHT_FACTOR[w], color);
    const shellOf = (geo: THREE.BufferGeometry) => {
      const shell = new THREE.Mesh(geo, material);
      shell.name = '__outline';
      shell.castShadow = false;
      shell.receiveShadow = false;
      shell.renderOrder = -1;
      return shell;
    };

    const combined = geos.length > 1 ? mergeGeometries(geos, false) : null;
    if (combined) {
      for (const g of geos) g.dispose();
      root.add(shellOf(combined));
    } else {
      // 只有一件、或合并失败：退回逐个挂，宁可慢也不能整件没有描边
      for (const g of geos) root.add(shellOf(g));
    }
  }
  return root;
}

/* ============================================================================
 * 3. 程序化贴图
 * ========================================================================== */

export function makeCanvas(w: number, h: number) {
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('[room] 2d context 获取失败');
  return { canvas, ctx };
}

export function toTexture(canvas: HTMLCanvasElement, repeat: [number, number] = [1, 1]): THREE.CanvasTexture {
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = THREE.RepeatWrapping;
  t.wrapT = THREE.RepeatWrapping;
  t.repeat.set(repeat[0], repeat[1]);
  t.anisotropy = 4;
  return t;
}

/** 固定种子随机，保证每次刷新纹理一模一样。 */
export function makeRng(seed: number) {
  let s = (seed >>> 0) || 1;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

/* ---------------- 夜间远景天空 / 城市灯带 ---------------- */
let _nightHorizon: THREE.Texture | null = null;
/**
 * 远景贴图只承担"地平线之后的空气与城市灯带"，不画完整天空，也不替代近景 3D 楼群。
 * 上部留深色云夜，下部用低对比雾化楼影和少量窗光，避免贴图像一堵完整背景墙。
 */
export function nightHorizonTexture(): THREE.Texture {
  if (_nightHorizon) return _nightHorizon;
  const { canvas, ctx } = makeCanvas(1024, 256);
  const rnd = makeRng(90402);
  const sky = ctx.createLinearGradient(0, 0, 0, 256);
  sky.addColorStop(0, '#0b1425');
  sky.addColorStop(0.48, '#101d31');
  sky.addColorStop(0.78, '#23364d');
  sky.addColorStop(1, '#52677a');
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, 1024, 256);

  // 薄云层：低对比、宽笔触，只压在天空上部，不抢近景楼群。
  for (let i = 0; i < 26; i++) {
    const x = rnd() * 1024;
    const y = 20 + rnd() * 105;
    const w = 70 + rnd() * 180;
    ctx.fillStyle = `rgba(70, 88, 112, ${0.035 + rnd() * 0.055})`;
    ctx.beginPath();
    ctx.ellipse(x, y, w, 10 + rnd() * 13, 0, 0, Math.PI * 2);
    ctx.fill();
  }

  // 远处城市剪影：只放在底部 40% 区域，模糊/低对比，配合雾融入背景。
  const skylineBase = 256;
  const skylineTop = 138;
  let x = -8;
  while (x < 1030) {
    const w = 12 + rnd() * 31;
    const h = 18 + rnd() * 73;
    const y = skylineBase - h;
    ctx.fillStyle = `rgba(14, 25, 40, ${0.28 + rnd() * 0.18})`;
    ctx.fillRect(x, Math.max(skylineTop, y), w, skylineBase - Math.max(skylineTop, y));
    if (w > 22 && rnd() > 0.62) {
      ctx.fillStyle = 'rgba(98, 122, 145, 0.22)';
      ctx.fillRect(x + w * 0.42, Math.max(skylineTop, y) - 5, 2, 5);
    }
    // 极少量暖窗，遵循参考图的"远处星点"而不是一整片亮窗。
    for (let wy = Math.max(skylineTop + 8, y + 12); wy < skylineBase - 7; wy += 12 + rnd() * 8) {
      if (rnd() > 0.78) {
        ctx.fillStyle = rnd() > 0.8 ? 'rgba(255, 203, 125, 0.52)' : 'rgba(180, 205, 220, 0.38)';
        ctx.fillRect(x + 3 + rnd() * Math.max(2, w - 6), wy, 1.5, 2);
      }
    }
    x += w + 3 + rnd() * 9;
  }

  // 地平线雾带：把贴图底缘压进场景雾，减少"硬贴上去"的边界。
  const haze = ctx.createLinearGradient(0, 160, 0, 256);
  haze.addColorStop(0, 'rgba(130, 157, 177, 0)');
  haze.addColorStop(1, 'rgba(155, 177, 189, 0.32)');
  ctx.fillStyle = haze;
  ctx.fillRect(0, 145, 1024, 111);

  _nightHorizon = toTexture(canvas, [1, 1]);
  return _nightHorizon;
}

function petal(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, rot: number, fill: string) {
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(rot);
  ctx.beginPath();
  ctx.moveTo(0, -r);
  ctx.quadraticCurveTo(r * 0.9, -r * 0.15, 0, r);
  ctx.quadraticCurveTo(-r * 0.9, -r * 0.15, 0, -r);
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.restore();
}

/* ---------------- 地板：暖色木地板 ---------------- */

let _floor: THREE.Texture | null = null;
export function woodFloorTexture(): THREE.Texture {
  if (_floor) return _floor;
  const { canvas, ctx } = makeCanvas(512, 512);
  /**
   * 白橡/白蜡木的浅木色。
   * 原先的 #e6c69c 饱和偏高、色相偏橙，铺满 200㎡ 之后整屋泛着一层暖棕，
   * 是"豪宅感"的主要来源。改成低饱和、略偏灰的浅木后，木地板退成背景，
   * 家具和光影才立得起来。木纹也同步从深棕降到浅灰褐——真实浅色木材的
   * 纹理本来就是比底色稍深一点的灰褐，不是深棕色。
   */
  ctx.fillStyle = '#d8c9b3';
  ctx.fillRect(0, 0, 512, 512);

  const PL = 64;
  for (let r = 0; r < 8; r++) {
    const y = r * PL;
    const rnd = makeRng(1000 + r * 77);
    // 逐板色差压到 ±2.5%：真实铺装的地板确实有板材色差，但超过这个值
    // 远看就是一片花，与"干净、统一"的要求冲突
    const c = new THREE.Color('#d8c9b3').offsetHSL(0, (rnd() - 0.5) * 0.022, (rnd() - 0.5) * 0.03);
    ctx.fillStyle = `#${c.getHexString()}`;
    ctx.fillRect(0, y, 512, PL);

    // 木纹
    ctx.globalAlpha = 0.075;
    ctx.strokeStyle = '#a89780';
    for (let k = 0; k < 8; k++) {
      const gy = y + 4 + rnd() * (PL - 8);
      ctx.beginPath();
      ctx.moveTo(0, gy);
      for (let x = 0; x <= 512; x += 24) {
        ctx.lineTo(x, gy + Math.sin((x + r * 47 + k * 13) * 0.018) * 2.6);
      }
      ctx.lineWidth = 0.6 + rnd() * 1.5;
      ctx.stroke();
    }
    // 结疤：浅色木材的结疤很淡，再降一档存在感
    if (rnd() > 0.6) {
      ctx.globalAlpha = 0.09;
      ctx.beginPath();
      ctx.ellipse(rnd() * 512, y + PL / 2, 5, 3, rnd() * 3, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;

    // 板缝：浅木地板的缝不该是深色沟，压到刚好能看出分格的程度
    ctx.fillStyle = 'rgba(150,133,112,0.30)';
    ctx.fillRect(0, y, 512, 2);
    const seamX = (r % 2 === 0 ? 0 : 256) + (r % 3) * 46;
    ctx.fillRect(seamX, y, 2, PL);
  }

  // repeat 恒为 1：贴图代表多大的一块地面由几何 UV 决定（见 scaleUV / FLOOR_TILE）。
  // 512px 里 8 块板（PL=64），配 FLOOR_TILE.wood=1.6m → 板条 1.6m × 0.2m。
  _floor = toTexture(canvas, [1, 1]);
  return _floor;
}

/* ---------------- 榻榻米：草席面 + 黑色包边 ---------------- */

let _tatami: THREE.Texture | null = null;
export function tatamiTexture(): THREE.Texture {
  if (_tatami) return _tatami;
  const { canvas, ctx } = makeCanvas(512, 512);
  // 一整张半叠榻榻米（约 0.9×1.8m），repeat 时按房间尺寸铺
  ctx.fillStyle = '#cfe0a8';
  ctx.fillRect(0, 0, 512, 512);

  // 草席的经纬织纹：细密横线 + 更淡的竖线
  ctx.strokeStyle = 'rgba(150,175,105,0.30)';
  ctx.lineWidth = 1;
  for (let y = 0; y < 512; y += 5) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(512, y);
    ctx.stroke();
  }
  ctx.strokeStyle = 'rgba(190,210,150,0.35)';
  for (let x = 0; x < 512; x += 11) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, 512);
    ctx.stroke();
  }
  // 缘侧（黑色布包边）：沿纹理上边缘一条
  ctx.fillStyle = '#2e3038';
  ctx.fillRect(0, 0, 512, 26);
  ctx.fillStyle = 'rgba(255,255,255,0.10)';
  ctx.fillRect(0, 26, 512, 3);

  _tatami = toTexture(canvas, [1, 1]);
  return _tatami;
}

/* ---------------- 卫生间/玄关瓷砖：浅灰 + 缝 ---------------- */

let _tile: THREE.Texture | null = null;
export function tileTexture(): THREE.Texture {
  if (_tile) return _tile;
  const { canvas, ctx } = makeCanvas(512, 512);
  ctx.fillStyle = '#e3ebee';
  ctx.fillRect(0, 0, 512, 512);

  const T = 128; // 一张 512 = 4×4 块砖，repeat 后约 0.45m 一块
  const rnd = makeRng(77);
  for (let r = 0; r < 4; r++) {
    for (let c = 0; c < 4; c++) {
      const v = (rnd() - 0.5) * 0.035;
      ctx.fillStyle = `rgb(${227 + v * 60},${235 + v * 55},${238 + v * 50})`;
      ctx.fillRect(c * T + 2, r * T + 2, T - 4, T - 4);
    }
  }
  // 缝
  ctx.strokeStyle = 'rgba(150,160,168,0.55)';
  ctx.lineWidth = 3;
  for (let i = 0; i <= 4; i++) {
    ctx.beginPath(); ctx.moveTo(i * T, 0); ctx.lineTo(i * T, 512); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, i * T); ctx.lineTo(512, i * T); ctx.stroke();
  }

  // 512px = 4×4 块砖，配 FLOOR_TILE.tile=1.8m → 每块 0.45m 见方。
  _tile = toTexture(canvas, [1, 1]);
  return _tile;
}

/* ---------------- 阳台防腐木地板 ---------------- */

let _deck: THREE.Texture | null = null;
export function deckTexture(): THREE.Texture {
  if (_deck) return _deck;
  const { canvas, ctx } = makeCanvas(512, 512);
  ctx.fillStyle = '#b98d63';
  ctx.fillRect(0, 0, 512, 512);

  const PL = 64;
  const rnd = makeRng(31);
  for (let r = 0; r < 8; r++) {
    const y = r * PL;
    const c = new THREE.Color('#b98d63').offsetHSL(0, (rnd() - 0.5) * 0.05, (rnd() - 0.5) * 0.07);
    ctx.fillStyle = `#${c.getHexString()}`;
    ctx.fillRect(0, y, 512, PL);
    // 木纹（比室内地板粗糙）
    ctx.globalAlpha = 0.16;
    ctx.strokeStyle = '#7a5636';
    for (let k = 0; k < 5; k++) {
      const gy = y + 5 + rnd() * (PL - 10);
      ctx.beginPath();
      ctx.moveTo(0, gy);
      for (let x = 0; x <= 512; x += 32) ctx.lineTo(x, gy + Math.sin((x + r * 31) * 0.02) * 3.4);
      ctx.lineWidth = 1 + rnd() * 1.6;
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    // 板缝（室外板缝宽）
    ctx.fillStyle = 'rgba(90,62,40,0.6)';
    ctx.fillRect(0, y, 512, 4);
  }

  _deck = toTexture(canvas, [1, 1]);
  return _deck;
}

/* ---------------- 墙纸：米白 + 细颗粒 + 极淡竖纹 ---------------- */

let _wall: THREE.Texture | null = null;
export function wallTexture(): THREE.Texture {
  if (_wall) return _wall;
  /**
   * 原先的墙纸撒了粉色樱花瓣，是"日式和风"的直白符号。本次要的是现代日本
   * 城市住宅，不是传统和风住宅——樱花一出现，整个空间立刻被读成料亭或民宿。
   *
   * 换成米白底 + 细颗粒：颗粒让墙面在近看时有涂料的质感，远看仍是干净的一块色，
   * 这正是"细腻、均匀、略带颗粒感，而不是完全纯色"要的效果。
   */
  const { canvas, ctx } = makeCanvas(512, 512);
  ctx.fillStyle = '#f4efe6';
  ctx.fillRect(0, 0, 512, 512);

  // 细颗粒：单点噪声，密度高、对比低。故意不做成规则点阵，
  // 规则点阵在 repeat 之后会连成明显的网格纹。
  const rnd = makeRng(20260831);
  for (let i = 0; i < 9000; i++) {
    const v = rnd();
    ctx.fillStyle = v > 0.5
      ? `rgba(255,255,255,${0.05 + rnd() * 0.07})`
      : `rgba(196,186,170,${0.04 + rnd() * 0.06})`;
    ctx.fillRect(rnd() * 512, rnd() * 512, 1.6, 1.6);
  }

  // 极淡的竖条：给大面积墙面一点方向感，避免整片死平
  ctx.fillStyle = 'rgba(228,220,206,0.30)';
  for (let x = 0; x < 512; x += 24) ctx.fillRect(x, 0, 2, 512);

  _wall = toTexture(canvas, [1, 1]);
  return _wall;
}

/* ---------------- 家具木纹：比地板更深 ---------------- */

let _wood: THREE.Texture | null = null;
export function furnitureWoodTexture(): THREE.Texture {
  if (_wood) return _wood;
  /**
   * 家具用材同样走浅木（白橡/桦木）。
   * 原先 #c8945f 是胡桃色，配合金色的黄铜件，整体往"欧式豪宅"偏。
   */
  const { canvas, ctx } = makeCanvas(256, 256);
  ctx.fillStyle = '#d3bf9f';
  ctx.fillRect(0, 0, 256, 256);
  const rnd = makeRng(4242);
  ctx.strokeStyle = 'rgba(150,126,94,0.26)';
  for (let k = 0; k < 26; k++) {
    const y = rnd() * 256;
    ctx.beginPath();
    ctx.moveTo(0, y);
    for (let x = 0; x <= 256; x += 16) {
      ctx.lineTo(x, y + Math.sin((x + k * 31) * 0.03) * 3.2);
    }
    ctx.lineWidth = 0.5 + rnd() * 1.4;
    ctx.stroke();
  }
  _wood = toTexture(canvas, [2, 2]);
  return _wood;
}

/* ---------------- 地毯：暖灰底 + 低饱和几何 ---------------- */

let _rug: THREE.Texture | null = null;
export function rugTexture(): THREE.Texture {
  if (_rug) return _rug;
  /**
   * 原先是粉底 + 玫红/青/黄三色菱形，饱和度在整屋里最高，缩略图一眼就先看到地毯。
   * 地毯在构图上应当属于"近景的材质层"，负责把家具圈起来，不是视觉主角。
   * 改成暖灰底 + 灰绿/灰蓝两色，几何结构保留，但对比压到刚好能辨形的程度。
   */
  const { canvas, ctx } = makeCanvas(512, 512);
  ctx.fillStyle = '#e8e2d7';
  ctx.fillRect(0, 0, 512, 512);

  // 三层边框
  ctx.strokeStyle = '#b9b2a2';
  ctx.lineWidth = 16;
  ctx.strokeRect(16, 16, 480, 480);
  ctx.strokeStyle = '#e8e2d7';
  ctx.lineWidth = 7;
  ctx.strokeRect(34, 34, 444, 444);
  ctx.strokeStyle = '#9aa89c';
  ctx.lineWidth = 4;
  ctx.strokeRect(45, 45, 422, 422);

  // 中央菱形阵列
  const diamond = (cx: number, cy: number, r: number, fill: string) => {
    ctx.beginPath();
    ctx.moveTo(cx, cy - r);
    ctx.lineTo(cx + r * 0.72, cy);
    ctx.lineTo(cx, cy + r);
    ctx.lineTo(cx - r * 0.72, cy);
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.fill();
  };
  const colors = ['#b9b2a2', '#9aa89c', '#a8b5c4'];
  let ci = 0;
  for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
      diamond(128 + i * 128, 128 + j * 128, 40, colors[ci % 3]);
      ci++;
      diamond(128 + i * 128, 128 + j * 128, 18, '#fdf6f2');
    }
  }
  // 边角球纹
  for (const [cx, cy] of [[70, 70], [442, 70], [70, 442], [442, 442]]) {
    diamond(cx, cy, 16, '#a8b5c4');
  }

  _rug = toTexture(canvas, [1, 1]);
  return _rug;
}

/* ---------------- 窗外：动漫蓝天 + 积云 + 远山 ---------------- */

let _sky: THREE.Texture | null = null;
export function skyTexture(): THREE.Texture {
  if (_sky) return _sky;
  const { canvas, ctx } = makeCanvas(512, 512);

  const g = ctx.createLinearGradient(0, 0, 0, 512);
  g.addColorStop(0.0, '#63b8e8');
  g.addColorStop(0.45, '#a8dcf5');
  g.addColorStop(0.78, '#dff0f7');
  g.addColorStop(1.0, '#ffe6c8');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 512, 512);

  // 太阳光晕
  const sun = ctx.createRadialGradient(370, 150, 8, 370, 150, 130);
  sun.addColorStop(0, 'rgba(255,252,232,0.95)');
  sun.addColorStop(0.4, 'rgba(255,244,205,0.42)');
  sun.addColorStop(1, 'rgba(255,240,200,0)');
  ctx.fillStyle = sun;
  ctx.fillRect(0, 0, 512, 512);

  // 积云：几个圆叠出蓬松轮廓，底部压一层偏蓝的暗面
  const cloud = (x: number, y: number, s: number, alpha: number) => {
    ctx.fillStyle = `rgba(255,255,255,${alpha})`;
    ctx.beginPath();
    ctx.arc(x, y, 30 * s, 0, Math.PI * 2);
    ctx.arc(x + 34 * s, y + 10 * s, 23 * s, 0, Math.PI * 2);
    ctx.arc(x - 32 * s, y + 12 * s, 20 * s, 0, Math.PI * 2);
    ctx.arc(x + 10 * s, y - 20 * s, 24 * s, 0, Math.PI * 2);
    ctx.arc(x - 12 * s, y - 10 * s, 20 * s, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = `rgba(196,222,240,${alpha * 0.75})`;
    ctx.beginPath();
    ctx.ellipse(x, y + 26 * s, 54 * s, 9 * s, 0, 0, Math.PI * 2);
    ctx.fill();
  };
  cloud(120, 170, 1.15, 0.95);
  cloud(330, 250, 0.85, 0.9);
  cloud(230, 330, 0.7, 0.8);
  cloud(430, 400, 1.0, 0.85);
  cloud(60, 420, 0.75, 0.7);

  // 远山剪影
  ctx.fillStyle = 'rgba(150,186,168,0.75)';
  ctx.beginPath();
  ctx.moveTo(0, 512);
  ctx.lineTo(0, 452);
  ctx.quadraticCurveTo(90, 392, 190, 448);
  ctx.quadraticCurveTo(280, 402, 380, 452);
  ctx.quadraticCurveTo(455, 424, 512, 462);
  ctx.lineTo(512, 512);
  ctx.closePath();
  ctx.fill();

  // 一层薄雾
  const haze = ctx.createLinearGradient(0, 400, 0, 512);
  haze.addColorStop(0, 'rgba(255,248,235,0)');
  haze.addColorStop(1, 'rgba(255,250,238,0.85)');
  ctx.fillStyle = haze;
  ctx.fillRect(0, 400, 512, 112);

  _sky = toTexture(canvas, [1, 1]);
  return _sky;
}

/* ---------------- 雨夜城市：窗外远景 ---------------- */

/**
 * 夜里的街景。和晴天那张的区别不只是"调暗"：
 *  - 光源从"一个太阳"变成"无数个小窗 + 霓虹"，所以亮部是碎的、点状的
 *  - 雨会让远处的对比度整体塌掉（空气里全是水），所以越远越灰蓝、越糊
 *  - 地面是湿的，霓虹和窗光会在沥青上拖出竖直的倒影条 —— 这是夜景最出效果的一笔
 */
let _nightCity: THREE.Texture | null = null;
export function nightCityTexture(): THREE.Texture {
  if (_nightCity) return _nightCity;
  const W = 512, H = 384;
  const { canvas, ctx } = makeCanvas(W, H);
  const rnd = makeRng(20260831);

  // 夜空：顶上近黑，靠近地平线被城市灯光染成灰蓝
  const sky = ctx.createLinearGradient(0, 0, 0, H);
  sky.addColorStop(0.00, '#0a1026');
  sky.addColorStop(0.30, '#141d3c');
  sky.addColorStop(0.56, '#26355c');
  sky.addColorStop(0.74, '#42567f');
  sky.addColorStop(0.86, '#5d6f95');
  sky.addColorStop(1.00, '#2b3550');
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, W, H);

  // 星（越靠地平线越被灯光吃掉）
  for (let i = 0; i < 80; i++) {
    const y = rnd() * 170;
    ctx.globalAlpha = (0.12 + rnd() * 0.55) * Math.max(0, 1 - y / 210);
    ctx.fillStyle = '#e2eaff';
    ctx.fillRect(rnd() * W, y, 1.5, 1.5);
  }
  ctx.globalAlpha = 1;

  // 月亮：被云压住大半，只漏一点边 —— 满月挂在天上就太"素材感"了
  const mx = 392, my = 66;
  const halo = ctx.createRadialGradient(mx, my, 3, mx, my, 70);
  halo.addColorStop(0, 'rgba(224,233,255,0.62)');
  halo.addColorStop(0.45, 'rgba(180,200,250,0.16)');
  halo.addColorStop(1, 'rgba(160,185,240,0)');
  ctx.fillStyle = halo;
  ctx.fillRect(mx - 70, my - 70, 140, 140);
  ctx.fillStyle = '#e9eeff';
  ctx.beginPath();
  ctx.arc(mx, my, 21, 0, Math.PI * 2);
  ctx.fill();
  // 遮月的云带
  ctx.fillStyle = 'rgba(24,32,60,0.62)';
  for (const [cx, cy, rx, ry] of [[352, 74, 78, 11], [430, 88, 66, 9], [380, 50, 54, 8]] as Array<[number, number, number, number]>) {
    ctx.beginPath();
    ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
    ctx.fill();
  }

  // 雨雾：斜向的极淡亮条，两层交错
  ctx.save();
  for (let i = 0; i < 90; i++) {
    const x = rnd() * W;
    const y = rnd() * 250;
    const len = 18 + rnd() * 46;
    ctx.globalAlpha = 0.03 + rnd() * 0.05;
    ctx.strokeStyle = '#cfe0ff';
    ctx.lineWidth = 0.8 + rnd() * 1.2;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x - len * 0.18, y + len);
    ctx.stroke();
  }
  ctx.restore();
  ctx.globalAlpha = 1;

  /* ---- 楼群：三层，越远越浅越糊 ---- */

  type Block = { x: number; w: number; top: number };

  const layer = (blocks: Block[], fill: string, alpha: number) => {
    ctx.globalAlpha = alpha;
    ctx.fillStyle = fill;
    for (const b of blocks) ctx.fillRect(b.x, b.top, b.w, H - b.top);
    ctx.globalAlpha = 1;
  };

  const row = (n: number, y0: number, y1: number, wMin: number, wMax: number, seed: number): Block[] => {
    const r = makeRng(seed);
    const out: Block[] = [];
    let x = -10;
    for (let i = 0; i < n; i++) {
      const w = wMin + r() * (wMax - wMin);
      out.push({ x, w: w + 2, top: y0 + r() * (y1 - y0) });
      x += w + 1;
      if (x > W + 10) break;
    }
    return out;
  };

  const far = row(14, 196, 244, 26, 54, 11);
  const mid = row(11, 214, 278, 34, 74, 22);
  const near = row(8, 244, 318, 52, 104, 33);

  layer(far, '#1b2544', 0.72);
  layer(mid, '#121a34', 0.92);
  layer(near, '#080d1c', 1.0);

  // 窗灯：warm 为主、零星 cool，近层最亮最密
  const windows = (blocks: Block[], cell: number, gap: number, lit: number, bright: number, seed: number) => {
    const r = makeRng(seed);
    for (const b of blocks) {
      for (let wy = b.top + gap; wy < H - 8; wy += cell) {
        for (let wx = b.x + gap; wx < b.x + b.w - 5; wx += cell) {
          if (r() > lit) continue;
          const warm = r() > 0.26;
          ctx.globalAlpha = bright * (0.55 + r() * 0.45);
          ctx.fillStyle = warm ? '#ffcf87' : '#b9dcff';
          ctx.fillRect(wx, wy, 3.4, 4.6);
        }
      }
    }
    ctx.globalAlpha = 1;
  };
  windows(far, 14, 5, 0.20, 0.42, 101);
  windows(mid, 15, 5, 0.26, 0.62, 202);
  windows(near, 17, 6, 0.30, 0.92, 303);

  // 霓虹招牌：竖排灯箱 + 一条横招牌，带外发光
  const neon = (x: number, y: number, w: number, h: number, col: string) => {
    ctx.save();
    ctx.shadowColor = col;
    ctx.shadowBlur = 14;
    ctx.globalAlpha = 0.9;
    ctx.fillStyle = col;
    ctx.fillRect(x, y, w, h);
    ctx.globalAlpha = 1;
    ctx.fillStyle = 'rgba(255,255,255,0.75)';
    ctx.fillRect(x + w * 0.25, y + h * 0.12, w * 0.5, h * 0.76);
    ctx.restore();
  };
  neon(58, 236, 9, 62, '#ff6fae');
  neon(70, 250, 7, 40, '#6fe3ff');
  neon(214, 262, 10, 74, '#ffd36f');
  neon(324, 226, 8, 52, '#6fe3ff');
  neon(452, 268, 46, 11, '#ff6fae');

  // 顶层航空障碍灯
  for (const b of [...mid, ...near]) {
    if (rnd() > 0.55) continue;
    ctx.globalAlpha = 0.85;
    ctx.fillStyle = '#ff8a7a';
    ctx.fillRect(b.x + b.w * 0.5, b.top - 3, 2.4, 2.4);
  }
  ctx.globalAlpha = 1;

  /* ---- 湿沥青路面 ---- */

  const roadY = 322;
  const road = ctx.createLinearGradient(0, roadY, 0, H);
  road.addColorStop(0, '#141b30');
  road.addColorStop(0.45, '#0d1324');
  road.addColorStop(1, '#080c18');
  ctx.fillStyle = road;
  ctx.fillRect(0, roadY, W, H - roadY);

  // 倒影：把楼里的亮色往下拖成竖直渐隐的条。这是"地面是湿的"最直接的证据
  ctx.save();
  ctx.globalCompositeOperation = 'lighter';
  const smear = (x: number, w: number, col: string, a: number) => {
    const g = ctx.createLinearGradient(0, roadY, 0, H);
    g.addColorStop(0, col);
    g.addColorStop(1, 'rgba(0,0,0,0)');
    ctx.globalAlpha = a;
    ctx.fillStyle = g;
    ctx.fillRect(x, roadY, w, H - roadY);
  };
  for (let i = 0; i < 26; i++) {
    const x = rnd() * W;
    const w = 3 + rnd() * 12;
    const warm = rnd() > 0.3;
    smear(x, w, warm ? 'rgba(255,205,135,0.85)' : 'rgba(150,205,255,0.85)', 0.10 + rnd() * 0.16);
  }
  smear(50, 34, 'rgba(255,111,174,0.9)', 0.30);   // 粉色霓虹的倒影
  smear(210, 26, 'rgba(255,211,111,0.9)', 0.28);
  smear(320, 22, 'rgba(111,227,255,0.9)', 0.24);
  smear(444, 52, 'rgba(255,111,174,0.9)', 0.22);
  ctx.restore();
  ctx.globalAlpha = 1;

  // 路灯光斑 + 它在水面上的竖直倒影
  for (const lx of [96, 268, 404]) {
    const ly = roadY + 12;
    const g = ctx.createRadialGradient(lx, ly, 1, lx, ly, 26);
    g.addColorStop(0, 'rgba(255,226,168,0.95)');
    g.addColorStop(1, 'rgba(255,214,150,0)');
    ctx.fillStyle = g;
    ctx.fillRect(lx - 26, ly - 26, 52, 52);
    ctx.save();
    ctx.globalCompositeOperation = 'lighter';
    const s = ctx.createLinearGradient(0, ly, 0, H);
    s.addColorStop(0, 'rgba(255,216,150,0.55)');
    s.addColorStop(1, 'rgba(255,216,150,0)');
    ctx.fillStyle = s;
    ctx.fillRect(lx - 2.5, ly, 5, H - ly);
    ctx.restore();
  }

  // 地平线上一层水汽，把楼根"泡"进去
  const mist = ctx.createLinearGradient(0, roadY - 40, 0, roadY + 16);
  mist.addColorStop(0, 'rgba(120,145,190,0)');
  mist.addColorStop(1, 'rgba(130,155,200,0.36)');
  ctx.fillStyle = mist;
  ctx.fillRect(0, roadY - 40, W, 56);

  // 整体压一层冷调，统一色温
  ctx.globalCompositeOperation = 'multiply';
  ctx.fillStyle = 'rgba(186,205,240,0.16)';
  ctx.fillRect(0, 0, W, H);
  ctx.globalCompositeOperation = 'source-over';

  _nightCity = toTexture(canvas, [1, 1]);
  return _nightCity;
}

/* ---------------- 雨丝 ---------------- */

/**
 * 一缕雨。画在 24×128 的竖长条里，中间一条细亮线、两端渐隐。
 *
 * 关键在"别画成白线"：雨丝本身不发光，它只是把环境冷光折了一点进眼睛。
 * 早先这图峰值用到 rgba(236,245,255,0.98) 再走叠加混合，落到画面上就是
 * 一根根烧白的粗线——典型的廉价 3D 雨。现在压到 0.6 峰值、整体偏冷蓝，
 * 配 0.3 上下的不透明度，出来的是"看得见雨、但雨不抢戏"的细雨。
 *
 * 这张纹理现在给 InstancedMesh 的雨丝矩形用：矩形本身有 rotation，所以
 * 纹理是"竖"的、矩形是"斜"的，组合出屏幕上斜向下飞的雨丝。
 */
let _rain: THREE.Texture | null = null;
export function rainStreakTexture(): THREE.Texture {
  if (_rain) return _rain;
  const { canvas, ctx } = makeCanvas(24, 128);

  const g = ctx.createLinearGradient(0, 0, 0, 128);
  g.addColorStop(0.00, 'rgba(172,200,244,0)');
  g.addColorStop(0.28, 'rgba(182,208,248,0.22)');
  g.addColorStop(0.70, 'rgba(200,222,252,0.50)');
  g.addColorStop(0.92, 'rgba(212,231,255,0.60)');
  g.addColorStop(1.00, 'rgba(212,231,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(11, 0, 2, 128);

  // 两侧各补一条更淡的，雨丝看起来才有粗细层次
  ctx.globalAlpha = 0.26;
  ctx.fillRect(9, 0, 2, 128);
  ctx.fillRect(13, 0, 2, 128);
  ctx.globalAlpha = 1;

  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _rain = t;
  return t;
}

/* ---------------- 前景虚大雨滴 ---------------- */

/**
 * 镜头前面那一层"虚化的雨"——不是一根根线段，是一团团软光斑。
 *
 * 参考图里这种"景前雨丝"是真实感的来源：它不是更多雨，而是更大更虚更亮、
 * 离镜头更近的一层。中/远层是细雨丝（rainStreakTexture），前景这一层是
 * 软椭圆（rainBlobTexture）。两者叠在一起才有"摄像机正站在雨里"的感觉。
 *
 * 纹理：竖椭圆，高斯型双向衰减。冷蓝偏白。
 */
let _rainBlob: THREE.Texture | null = null;
export function rainBlobTexture(): THREE.Texture {
  if (_rainBlob) return _rainBlob;
  const W = 48, H = 160;
  const { canvas, ctx } = makeCanvas(W, H);
  const cx = W / 2, cy = H / 2;
  const img = ctx.createImageData(W, H);
  for (let y = 0; y < H; y++) {
    const ty = (y - cy) / (H / 2);
    const lenFall = Math.exp(-ty * ty * 1.6);
    for (let x = 0; x < W; x++) {
      const tx = (x - cx) / (W / 2);
      const widFall = Math.exp(-tx * tx * 2.6);
      const a = lenFall * widFall * 0.62;
      const idx = (y * W + x) * 4;
      img.data[idx]     = Math.round(214 * a);
      img.data[idx + 1] = Math.round(228 * a);
      img.data[idx + 2] = Math.round(248 * a);
      img.data[idx + 3] = Math.round(a * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _rainBlob = t;
  return t;
}

/* ---------------- 窗玻璃上的雨水 ---------------- */

/**
 * 贴在窗玻璃上的一层水膜：往下淌的水痕 + 挂在玻璃上的水珠。
 * 叠加混合，本身不发光，靠后面夜景的亮度把它"点亮"。
 */
let _rainGlass: THREE.Texture | null = null;
export function rainGlassTexture(): THREE.Texture {
  if (_rainGlass) return _rainGlass;
  const W = 256, H = 384;
  const { canvas, ctx } = makeCanvas(W, H);
  const rnd = makeRng(70707);

  // 底：整片玻璃上一层极淡的水雾
  ctx.fillStyle = 'rgba(150,180,225,0.10)';
  ctx.fillRect(0, 0, W, H);

  // 水痕：从某个水珠开始往下拖，越往下越细越淡
  for (let i = 0; i < 34; i++) {
    const x = rnd() * W;
    const y0 = rnd() * H * 0.75;
    const len = 24 + rnd() * 150;
    const w = 1.2 + rnd() * 2.2;
    const g = ctx.createLinearGradient(0, y0, 0, y0 + len);
    g.addColorStop(0, 'rgba(226,238,255,0.42)');
    g.addColorStop(0.55, 'rgba(210,228,255,0.20)');
    g.addColorStop(1, 'rgba(200,220,255,0)');
    ctx.fillStyle = g;
    // 水痕会蛇形往下走，不是笔直的
    ctx.beginPath();
    ctx.moveTo(x, y0);
    const amp = 2 + rnd() * 5;
    for (let s = 0; s <= len; s += 8) {
      ctx.lineTo(x + Math.sin(s * 0.06 + i) * amp, y0 + s);
    }
    ctx.lineWidth = w;
    ctx.strokeStyle = g;
    ctx.stroke();
  }

  // 水珠：亮边 + 更亮的高光点 + 下方一点折射亮
  for (let i = 0; i < 120; i++) {
    const x = rnd() * W;
    const y = rnd() * H;
    const r = 1.4 + rnd() * 3.6;
    ctx.globalAlpha = 0.30 + rnd() * 0.35;
    ctx.fillStyle = 'rgba(214,232,255,0.9)';
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.globalAlpha = 0.55 + rnd() * 0.4;
    ctx.fillStyle = 'rgba(255,255,255,0.95)';
    ctx.beginPath();
    ctx.arc(x - r * 0.3, y - r * 0.35, r * 0.34, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  // 玻璃四边积得更厚
  const edge = ctx.createLinearGradient(0, 0, 0, H);
  edge.addColorStop(0, 'rgba(180,205,245,0.34)');
  edge.addColorStop(0.18, 'rgba(180,205,245,0)');
  edge.addColorStop(0.82, 'rgba(180,205,245,0)');
  edge.addColorStop(1, 'rgba(180,205,245,0.30)');
  ctx.fillStyle = edge;
  ctx.fillRect(0, 0, W, H);

  _rainGlass = toTexture(canvas, [1, 1]);
  return _rainGlass;
}

/* ---------------- 湿润路面（世界地面） ---------------- */

/**
 * 湿沥青：深蓝灰底 + 碎石颗粒 + 几摊更亮的水光。
 * 刻意做成低频、低对比 —— 地面是外景的承载者，抢了建筑的戏就本末倒置了。
 */
let _pave: THREE.Texture | null = null;
export function pavementTexture(): THREE.Texture {
  if (_pave) return _pave;
  const { canvas, ctx } = makeCanvas(512, 512);
  const rnd = makeRng(5150);

  ctx.fillStyle = '#2a3247';
  ctx.fillRect(0, 0, 512, 512);

  // 大块的深浅不均
  for (let i = 0; i < 90; i++) {
    const c = new THREE.Color('#2a3247').offsetHSL((rnd() - 0.5) * 0.05, 0, (rnd() - 0.5) * 0.075);
    ctx.globalAlpha = 0.5;
    ctx.fillStyle = `#${c.getHexString()}`;
    ctx.beginPath();
    ctx.ellipse(rnd() * 512, rnd() * 512, 30 + rnd() * 90, 22 + rnd() * 70, rnd() * 3, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  // 碎石颗粒
  for (let i = 0; i < 2600; i++) {
    ctx.globalAlpha = 0.05 + rnd() * 0.16;
    ctx.fillStyle = rnd() > 0.55 ? '#4a5a78' : '#161c2c';
    ctx.fillRect(rnd() * 512, rnd() * 512, 1 + rnd() * 2, 1 + rnd() * 2);
  }
  ctx.globalAlpha = 1;

  // 几摊积水：边缘一圈更暗的水线，中间整片偏亮偏蓝
  for (let i = 0; i < 7; i++) {
    const x = rnd() * 512, y = rnd() * 512;
    const rx = 26 + rnd() * 62, ry = 18 + rnd() * 40;
    ctx.globalAlpha = 0.30;
    ctx.fillStyle = '#5f78a4';
    ctx.beginPath();
    ctx.ellipse(x, y, rx, ry, rnd() * 3, 0, Math.PI * 2);
    ctx.fill();
    ctx.globalAlpha = 0.35;
    ctx.strokeStyle = '#8fa8cc';
    ctx.lineWidth = 1.4;
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  _pave = toTexture(canvas, [3, 3]);
  return _pave;
}

/* ---------------- 雨夜沥青车行道 ---------------- */

let _asphalt: THREE.Texture | null = null;
export function asphaltTexture(): THREE.Texture {
  if (_asphalt) return _asphalt;
  const { canvas, ctx } = makeCanvas(512, 512);
  const rnd = makeRng(9173);

  // 平涂底色：中性黑灰沥青（赛璐璐靠贴图分层，不靠颗粒堆质感；去蓝，仅留极轻冷偏）
  ctx.fillStyle = '#3f4041';
  ctx.fillRect(0, 0, 512, 512);

  // —— 大块硬边色块：2~3 档明度分层（无渐变、无透明叠加）——
  // 较亮的"湿反光区"：硬边多边形
  const lightPatches: Array<[number, number, number]> = [
    [150, 150, 120], [380, 360, 140], [90, 400, 95],
  ];
  for (const [cx, cy, r] of lightPatches) {
    ctx.fillStyle = '#4d4e4f';
    ctx.beginPath();
    const n = 7;
    for (let i = 0; i <= n; i++) {
      const a = (i / n) * Math.PI * 2;
      const rr = r * (0.7 + rnd() * 0.5);
      const x = cx + Math.cos(a) * rr;
      const y = cy + Math.sin(a) * rr * 0.7;
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.closePath();
    ctx.fill();
  }
  // 较暗的"老旧 / 磨损区"：同样硬边
  const darkPatches: Array<[number, number, number]> = [
    [300, 110, 95], [430, 95, 72], [210, 425, 80],
  ];
  for (const [cx, cy, r] of darkPatches) {
    ctx.fillStyle = '#373839';
    ctx.beginPath();
    const n = 6;
    for (let i = 0; i <= n; i++) {
      const a = (i / n) * Math.PI * 2;
      const rr = r * (0.7 + rnd() * 0.5);
      const x = cx + Math.cos(a) * rr;
      const y = cy + Math.sin(a) * rr * 0.75;
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.closePath();
    ctx.fill();
  }

  // 轮胎磨损带：两条干净硬边暗带（赛璐璐不做渐变，直接实色块）
  for (const bandX of [160, 352]) {
    ctx.fillStyle = '#333435';
    ctx.fillRect(bandX - 20, 0, 40, 512);
  }

  // 横向接缝（切割缝 ~1.25m 间距）：干净硬边线
  ctx.strokeStyle = '#2a2b2c';
  ctx.lineWidth = 2;
  for (let y = 128; y < 512; y += 128) {
    const yOff = y + (rnd() - 0.5) * 4;
    ctx.beginPath();
    ctx.moveTo(0, yOff);
    ctx.lineTo(170, yOff + (rnd() - 0.5) * 2);
    ctx.lineTo(340, yOff + (rnd() - 0.5) * 2);
    ctx.lineTo(512, yOff + (rnd() - 0.5) * 3);
    ctx.stroke();
  }
  // 纵向施工缝
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(256, 0); ctx.lineTo(256, 512);
  ctx.stroke();

  // —— 细骨料颗粒（赛璐璐分 4 档明度，参考 PBR 预览图的碎石密度）——
  // 浅色碎石（抛光骨料反射，最亮档）
  for (let i = 0; i < 520; i++) {
    const s = 1 + rnd() * 2;
    ctx.fillStyle = '#606162';
    ctx.fillRect(rnd() * 512, rnd() * 512, s, s);
  }
  // 中浅碎石
  for (let i = 0; i < 680; i++) {
    const s = 1 + rnd() * 1.5;
    ctx.fillStyle = '#4b4c4d';
    ctx.fillRect(rnd() * 512, rnd() * 512, s, s);
  }
  // 中深碎石
  for (let i = 0; i < 480; i++) {
    const s = 1 + rnd() * 1.5;
    ctx.fillStyle = '#363738';
    ctx.fillRect(rnd() * 512, rnd() * 512, s, s);
  }
  // 深色碎石（最暗档）
  for (let i = 0; i < 350; i++) {
    const s = 1 + rnd() * 1.2;
    ctx.fillStyle = '#252627';
    ctx.fillRect(rnd() * 512, rnd() * 512, s, s);
  }
  // 轮胎带内补少量抛光点
  for (const bandX of [160, 352]) {
    ctx.fillStyle = '#545556';
    for (let j = 0; j < 90; j++) {
      ctx.fillRect(bandX - 20 + rnd() * 40, rnd() * 512, 1, 1);
    }
  }

  // —— 少量裂纹（赛璐璐硬边线，3~4 条不规则短裂纹）——
  ctx.strokeStyle = '#1d1e1f';
  ctx.lineWidth = 1;
  for (let i = 0; i < 4; i++) {
    ctx.beginPath();
    let x = rnd() * 512, y = rnd() * 512;
    ctx.moveTo(x, y);
    for (let seg = 0; seg < 6 + Math.floor(rnd() * 4); seg++) {
      x += (rnd() - 0.5) * 28;
      y += (rnd() - 0.5) * 28;
      ctx.lineTo(x, y);
    }
    ctx.stroke();
  }

  _asphalt = toTexture(canvas, [3, 3]);
  return _asphalt;
}

/* ---------------- 城市广场石砖 ---------------- */

let _plazaStone: THREE.Texture | null = null;
/**
 * 城市广场石砖：工字错缝铺的方形石板，读作"二次元赛璐璐"的硬边铺装。
 *
 * 与 sidewalkTexture（照片级混凝土）相反——这里不堆噪点、不画气孔，只靠
 * 3 档冷灰石材 + 干净勾缝 + 每块上方一道受光亮边做出"扁平但有体积"的铺地。
 * 6×6 网格 → 6m UV repeat 下每砖 ≈ 1m，正好是一块广场石材的尺度。
 */
export function plazaStoneTexture(): THREE.Texture {
  if (_plazaStone) return _plazaStone;
  const { canvas, ctx } = makeCanvas(512, 512);
  const rnd = makeRng(5501);

  // 勾缝（砂浆）底色：比石材深一档的冷灰
  ctx.fillStyle = '#4c545f';
  ctx.fillRect(0, 0, 512, 512);

  // 5 档冷灰石材（参考 PBR 预览：块与块之间明度变化更自然）
  const tones = ['#8c96a2', '#7e8895', '#939da9', '#828c98', '#8a949f'];
  const TILE = 512 / 6;     // 6×6 网格
  const MORTAR = 8;         // 勾缝加宽（参考图勾缝明显）

  for (let row = 0; row < 6; row++) {
    const offset = (row % 2) * (TILE / 2);   // 工字错缝
    for (let col = -1; col < 7; col++) {
      const x = col * TILE + offset;
      const y = row * TILE;
      // 块面：5 档里挑一档（赛璐璐仍不做连续噪点，只在档间切换）
      const tone = tones[Math.floor(rnd() * 5)];
      ctx.fillStyle = tone;
      ctx.fillRect(x + MORTAR / 2, y + MORTAR / 2, TILE - MORTAR, TILE - MORTAR);

      // 顶部受光亮边（赛璐璐：每块上方一道更亮的边，读作"光从上方来"）
      ctx.globalAlpha = 0.4;
      ctx.fillStyle = '#aab3bf';
      ctx.fillRect(x + MORTAR / 2, y + MORTAR / 2, TILE - MORTAR, 4);
      // 底部暗边
      ctx.fillStyle = '#5a626e';
      ctx.fillRect(x + MORTAR / 2, y + TILE - MORTAR / 2 - 4, TILE - MORTAR, 4);
      ctx.globalAlpha = 1;

      // —— 每块内部纹理（参考 PBR 预览：每块有自己的矿物斑点/纹路）——
      // 浅色矿物斑点（石英/长石）
      for (let k = 0; k < 12; k++) {
        const c = rnd() > 0.5 ? '#9aa4b0' : '#a8b2be';
        ctx.fillStyle = c;
        const px = x + MORTAR + rnd() * (TILE - MORTAR);
        const py = y + MORTAR + rnd() * (TILE - MORTAR);
        const s = 1 + rnd() * 1.8;
        ctx.fillRect(px, py, s, s);
      }
      // 深色斑点（暗色矿物/气孔）
      for (let k = 0; k < 8; k++) {
        ctx.fillStyle = '#6b7380';
        const px = x + MORTAR + rnd() * (TILE - MORTAR);
        const py = y + MORTAR + rnd() * (TILE - MORTAR);
        const s = 1 + rnd() * 1.4;
        ctx.fillRect(px, py, s, s);
      }
      // 偶尔一条细纹（石材纹理走向）
      if (rnd() > 0.55) {
        ctx.globalAlpha = 0.22;
        ctx.strokeStyle = '#6b7380';
        ctx.lineWidth = 0.8;
        ctx.beginPath();
        const sx = x + MORTAR + rnd() * (TILE - MORTAR);
        const sy = y + MORTAR + rnd() * (TILE - MORTAR);
        ctx.moveTo(sx, sy);
        ctx.lineTo(sx + (rnd() - 0.5) * 35, sy + (rnd() - 0.5) * 18);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
    }
  }

  // 勾缝：干净硬边暗线（盖在块间留白上，强调网格）
  ctx.globalAlpha = 1;
  ctx.strokeStyle = '#3c434e';
  ctx.lineWidth = MORTAR;
  for (let row = 0; row <= 6; row++) {
    ctx.beginPath(); ctx.moveTo(0, row * TILE); ctx.lineTo(512, row * TILE); ctx.stroke();
  }
  for (let row = 0; row < 6; row++) {
    const offset = (row % 2) * (TILE / 2);
    for (let col = 0; col <= 6; col++) {
      const x = col * TILE + offset;
      ctx.beginPath(); ctx.moveTo(x, row * TILE); ctx.lineTo(x, (row + 1) * TILE); ctx.stroke();
    }
  }

  _plazaStone = toTexture(canvas, [1, 1]);
  return _plazaStone;
}

/* ---------------- 人行道混凝土 ---------------- */

let _sidewalk: THREE.Texture | null = null;
export function sidewalkTexture(): THREE.Texture {
  if (_sidewalk) return _sidewalk;
  const { canvas, ctx } = makeCanvas(512, 512);
  const rnd = makeRng(4421);

  // 底色：中浅灰混凝土
  ctx.fillStyle = '#6c7480';
  ctx.fillRect(0, 0, 512, 512);

  // 逐板色差：4x4 网格，每块独立微调明度
  const SLAB = 128;
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 4; col++) {
      const dl = (rnd() - 0.5) * 0.025;
      const c = new THREE.Color('#6c7480').offsetHSL((rnd() - 0.5) * 0.008, 0, dl);
      ctx.globalAlpha = 0.7;
      ctx.fillStyle = `#${c.getHexString()}`;
      ctx.fillRect(col * SLAB + 2, row * SLAB + 2, SLAB - 4, SLAB - 4);
    }
  }
  ctx.globalAlpha = 1;

  // 伸缩缝：凹槽效果（暗线 + 两侧亮边模拟倒角）
  ctx.lineWidth = 3;
  for (let i = SLAB; i < 512; i += SLAB) {
    // 凹槽暗线
    ctx.globalAlpha = 0.55;
    ctx.strokeStyle = '#4a5260';
    ctx.beginPath(); ctx.moveTo(i, 0); ctx.lineTo(i, 512); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, i); ctx.lineTo(512, i); ctx.stroke();
    // 倒角亮边（缝右侧/下侧 1px 亮线）
    ctx.globalAlpha = 0.15;
    ctx.strokeStyle = '#8a929e';
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(i + 2, 0); ctx.lineTo(i + 2, 512); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, i + 2); ctx.lineTo(512, i + 2); ctx.stroke();
    ctx.lineWidth = 3;
  }
  ctx.globalAlpha = 1;

  // 模板印痕：每块板中央一个大椭圆浅色斑（浇筑模板留下的光面）
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 4; col++) {
      const cx = col * SLAB + SLAB / 2 + (rnd() - 0.5) * 12;
      const cy = row * SLAB + SLAB / 2 + (rnd() - 0.5) * 12;
      ctx.globalAlpha = 0.06 + rnd() * 0.04;
      ctx.fillStyle = '#7d8694';
      ctx.beginPath();
      ctx.ellipse(cx, cy, 30 + rnd() * 18, 26 + rnd() * 16, rnd() * 0.5, 0, Math.PI * 2);
      ctx.fill();
    }
  }
  ctx.globalAlpha = 1;

  // 气孔：混凝土表面小坑（暗点 + 旁边亮点 = 凹陷感）
  for (let i = 0; i < 400; i++) {
    const px = rnd() * 512, py = rnd() * 512;
    const r = 0.8 + rnd() * 1.5;
    ctx.globalAlpha = 0.12 + rnd() * 0.1;
    ctx.fillStyle = '#4e5764';
    ctx.beginPath(); ctx.arc(px, py, r, 0, Math.PI * 2); ctx.fill();
    ctx.globalAlpha = 0.06;
    ctx.fillStyle = '#8a939f';
    ctx.beginPath(); ctx.arc(px + 0.5, py - 0.5, r * 0.6, 0, Math.PI * 2); ctx.fill();
  }
  ctx.globalAlpha = 1;

  // 细颗粒噪点
  for (let i = 0; i < 2000; i++) {
    ctx.globalAlpha = 0.03 + rnd() * 0.06;
    ctx.fillStyle = rnd() > 0.5 ? '#7c8490' : '#5a626d';
    ctx.fillRect(rnd() * 512, rnd() * 512, 1, 1);
  }
  ctx.globalAlpha = 1;

  // 边缘磨损：板的四角稍微暗一点（踩踏集中区）
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 4; col++) {
      const corners = [
        [col * SLAB, row * SLAB],
        [(col + 1) * SLAB, row * SLAB],
        [col * SLAB, (row + 1) * SLAB],
        [(col + 1) * SLAB, (row + 1) * SLAB],
      ];
      for (const [cx2, cy2] of corners) {
        ctx.globalAlpha = 0.07;
        ctx.fillStyle = '#545d69';
        ctx.beginPath(); ctx.arc(cx2, cy2, 10 + rnd() * 6, 0, Math.PI * 2); ctx.fill();
      }
    }
  }
  ctx.globalAlpha = 1;

  _sidewalk = toTexture(canvas, [1, 1]);
  return _sidewalk;
}

/* ---------------- 路缘石混凝土 ---------------- */

let _curb: THREE.Texture | null = null;
export function curbTexture(): THREE.Texture {
  if (_curb) return _curb;
  const { canvas, ctx } = makeCanvas(128, 256);
  const rnd = makeRng(6637);

  // 底色：灰色混凝土
  ctx.fillStyle = '#5b626d';
  ctx.fillRect(0, 0, 128, 256);

  // 纵向色差
  for (let i = 0; i < 20; i++) {
    ctx.globalAlpha = 0.12;
    ctx.fillStyle = rnd() > 0.5 ? '#666e7a' : '#4e5661';
    ctx.fillRect(rnd() * 128, 0, 4 + rnd() * 12, 256);
  }
  ctx.globalAlpha = 1;

  // 顶部磨损带（上 20% 更亮 = 被踩/被车轮磨）
  const topGrad = ctx.createLinearGradient(0, 0, 0, 50);
  topGrad.addColorStop(0, 'rgba(110,120,132,0.3)');
  topGrad.addColorStop(1, 'rgba(110,120,132,0)');
  ctx.fillStyle = topGrad;
  ctx.fillRect(0, 0, 128, 50);

  // 底部水渍痕（雨水长期流过的深色条纹）
  for (let i = 0; i < 8; i++) {
    ctx.globalAlpha = 0.12 + rnd() * 0.08;
    ctx.fillStyle = '#3a4250';
    const sx = rnd() * 128;
    ctx.fillRect(sx, 180 + rnd() * 40, 2 + rnd() * 5, 36 + rnd() * 40);
  }
  ctx.globalAlpha = 1;

  // 混凝土颗粒
  for (let i = 0; i < 800; i++) {
    ctx.globalAlpha = 0.04 + rnd() * 0.08;
    ctx.fillStyle = rnd() > 0.5 ? '#6e7783' : '#484f5a';
    ctx.fillRect(rnd() * 128, rnd() * 256, 1 + rnd(), 1 + rnd());
  }
  ctx.globalAlpha = 1;

  // 横向接缝（每块路缘石的接头）
  ctx.globalAlpha = 0.4;
  ctx.strokeStyle = '#424a55';
  ctx.lineWidth = 2;
  ctx.beginPath(); ctx.moveTo(0, 128); ctx.lineTo(128, 128); ctx.stroke();
  ctx.globalAlpha = 1;

  _curb = toTexture(canvas, [1, 1]);
  return _curb;
}

/* ---------------- 井盖金属 ---------------- */

let _manhole: THREE.Texture | null = null;
export function manholeTexture(): THREE.Texture {
  if (_manhole) return _manhole;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const rnd = makeRng(8812);

  // 底色：深灰金属
  ctx.fillStyle = '#2f353d';
  ctx.fillRect(0, 0, S, S);

  const cx = S / 2, cy = S / 2;

  // 外圈边框
  ctx.globalAlpha = 0.6;
  ctx.strokeStyle = '#4a525d';
  ctx.lineWidth = 8;
  ctx.beginPath(); ctx.arc(cx, cy, 110, 0, Math.PI * 2); ctx.stroke();
  ctx.globalAlpha = 1;

  // 内圈
  ctx.globalAlpha = 0.4;
  ctx.strokeStyle = '#3d4550';
  ctx.lineWidth = 4;
  ctx.beginPath(); ctx.arc(cx, cy, 88, 0, Math.PI * 2); ctx.stroke();
  ctx.globalAlpha = 1;

  // 浮雕花纹：同心环 + 径向条纹（日本标准下水道井盖样式）
  for (let r = 20; r < 85; r += 14) {
    ctx.globalAlpha = 0.2;
    ctx.strokeStyle = rnd() > 0.5 ? '#555e6a' : '#252b33';
    ctx.lineWidth = 3;
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, Math.PI * 2); ctx.stroke();
  }
  // 径向条纹
  for (let i = 0; i < 12; i++) {
    const a = (i / 12) * Math.PI * 2;
    ctx.globalAlpha = 0.15;
    ctx.strokeStyle = '#555e6a';
    ctx.lineWidth = 2.5;
    ctx.beginPath();
    ctx.moveTo(cx + Math.cos(a) * 22, cy + Math.sin(a) * 22);
    ctx.lineTo(cx + Math.cos(a) * 84, cy + Math.sin(a) * 84);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  // 中心标记（小型十字或圆点）
  ctx.globalAlpha = 0.35;
  ctx.fillStyle = '#5a636e';
  ctx.beginPath(); ctx.arc(cx, cy, 14, 0, Math.PI * 2); ctx.fill();
  ctx.globalAlpha = 1;

  // 金属磨损：随机亮点（使用抛光）
  for (let i = 0; i < 300; i++) {
    const a = rnd() * Math.PI * 2;
    const r = rnd() * 105;
    ctx.globalAlpha = 0.04 + rnd() * 0.06;
    ctx.fillStyle = rnd() > 0.6 ? '#6a737e' : '#22282f';
    ctx.fillRect(cx + Math.cos(a) * r, cy + Math.sin(a) * r, 1 + rnd() * 2, 1 + rnd());
  }
  ctx.globalAlpha = 1;

  // 锈迹（少量棕色斑点）
  for (let i = 0; i < 12; i++) {
    const a = rnd() * Math.PI * 2;
    const r = 30 + rnd() * 70;
    ctx.globalAlpha = 0.06 + rnd() * 0.05;
    ctx.fillStyle = '#5c4a3a';
    ctx.beginPath();
    ctx.arc(cx + Math.cos(a) * r, cy + Math.sin(a) * r, 2 + rnd() * 5, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  _manhole = toTexture(canvas, [1, 1]);
  return _manhole;
}

/* ---------------- 排水篦 ---------------- */

let _grate: THREE.Texture | null = null;
export function drainGrateTexture(): THREE.Texture {
  if (_grate) return _grate;
  const { canvas, ctx } = makeCanvas(128, 256);
  const rnd = makeRng(7734);

  // 外框：深灰金属边框
  ctx.fillStyle = '#3a4149';
  ctx.fillRect(0, 0, 128, 256);

  // 内凹区域（比边框暗 = 深度感）
  ctx.fillStyle = '#1a1f26';
  ctx.fillRect(8, 8, 112, 240);

  // 横向栅条
  ctx.globalAlpha = 0.85;
  for (let y = 14; y < 244; y += 16) {
    // 栅条主体
    ctx.fillStyle = '#4a525d';
    ctx.fillRect(10, y, 108, 7);
    // 栅条顶部高光（金属反光）
    ctx.globalAlpha = 0.25;
    ctx.fillStyle = '#6a737e';
    ctx.fillRect(10, y, 108, 2);
    ctx.globalAlpha = 0.85;
    // 栅条间暗缝（深度）
    ctx.fillStyle = '#111619';
    ctx.fillRect(10, y + 8, 108, 7);
  }
  ctx.globalAlpha = 1;

  // 边框高光（顶边和左边亮一点 = 光源方向）
  ctx.globalAlpha = 0.2;
  ctx.fillStyle = '#7a838e';
  ctx.fillRect(0, 0, 128, 3);
  ctx.fillRect(0, 0, 3, 256);
  ctx.globalAlpha = 1;

  // 锈迹和水垢
  for (let i = 0; i < 20; i++) {
    ctx.globalAlpha = 0.06 + rnd() * 0.06;
    ctx.fillStyle = rnd() > 0.5 ? '#5c4a3a' : '#3a4a42';
    ctx.fillRect(rnd() * 128, rnd() * 256, 2 + rnd() * 8, 2 + rnd() * 6);
  }
  ctx.globalAlpha = 1;

  _grate = toTexture(canvas, [1, 1]);
  return _grate;
}

/* ---------------- 局部浅水斑块 ---------------- */

/**
 * 局部浅水：不规则软斑（非正圆、非规则椭圆、非蓝色），只铺在低洼 / 路缘旁 /
 * 排水口附近。白色径向底用材质色 tint 成深冷灰，正常混合低不透明度，读作
 * "沥青上的一汪浅水"而非"一块发光蓝片"。涟漪（buildPuddleRipples）只落在这几处。
 */
let _puddlePatch: THREE.Texture | null = null;
export function puddlePatchTexture(): THREE.Texture {
  if (_puddlePatch) return _puddlePatch;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const g = ctx.createRadialGradient(64, 64, 6, 64, 64, 60);
  g.addColorStop(0.0, 'rgba(255,255,255,0.95)');
  g.addColorStop(0.55, 'rgba(255,255,255,0.45)');
  g.addColorStop(1.0, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.beginPath();
  const rnd = makeRng(3322);
  const pts = 14;
  for (let i = 0; i <= pts; i++) {
    const a = (i / pts) * Math.PI * 2;
    const r = 48 + (rnd() - 0.5) * 22;
    const x = 64 + Math.cos(a) * r;
    const y = 64 + Math.sin(a) * r * (0.7 + rnd() * 0.3);
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  }
  ctx.closePath();
  ctx.fill();
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _puddlePatch = t;
  return t;
}

/* ---------------- 窗帘：竖褶 + 顶部渐暗 ---------------- */

let _curtain: THREE.Texture | null = null;
export function curtainTexture(): THREE.Texture {
  if (_curtain) return _curtain;
  const { canvas, ctx } = makeCanvas(128, 256);

  const g = ctx.createLinearGradient(0, 0, 128, 0);
  g.addColorStop(0.0, '#cfc7b8');
  g.addColorStop(0.35, '#f6f2e9');
  g.addColorStop(0.5, '#ebe5d8');
  g.addColorStop(0.68, '#f6f2e9');
  g.addColorStop(1.0, '#cfc7b8');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 128, 256);

  // 褶皱的竖向明暗
  ctx.globalAlpha = 0.35;
  for (let x = 0; x < 128; x += 16) {
    ctx.fillStyle = 'rgba(150,140,124,0.6)';
    ctx.fillRect(x, 0, 2, 256);
  }
  ctx.globalAlpha = 1;

  // 上下压暗，让窗帘有垂坠体积
  const v = ctx.createLinearGradient(0, 0, 0, 256);
  v.addColorStop(0, 'rgba(120,110,96,0.35)');
  v.addColorStop(0.3, 'rgba(120,110,96,0)');
  v.addColorStop(1, 'rgba(120,110,96,0.28)');
  ctx.fillStyle = v;
  ctx.fillRect(0, 0, 128, 256);

  // 亚麻的织纹：横竖交错的细线，替代原先的小碎花。
  // 碎花会把窗帘变成"图案"，而这里需要的是"材质"。
  const rnd = makeRng(77);
  ctx.globalAlpha = 0.16;
  ctx.strokeStyle = '#a89e8c';
  ctx.lineWidth = 1;
  for (let i = 0; i < 42; i++) {
    const y = rnd() * 256;
    ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(128, y); ctx.stroke();
  }
  for (let i = 0; i < 26; i++) {
    const x = rnd() * 128;
    ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, 256); ctx.stroke();
  }
  ctx.globalAlpha = 1;

  _curtain = toTexture(canvas, [1, 1]);
  return _curtain;
}

/* ---------------- 墙上装饰画：低饱和抽象风景 ---------------- */

let _poster: THREE.Texture | null = null;
export function posterTexture(): THREE.Texture {
  if (_poster) return _poster;
  /**
   * 原先是紫粉渐变黄昏 + 流星 + 城市剪影，饱和度全屋最高，一件装饰画把眼睛
   * 全抢走了。装饰画的正确位置是"墙面上的一点变化"，不是画面主角。
   *
   * 换成低饱和的远景山峦：天空、远山、近坡三层，每层一个灰调色块，
   * 保留可辨识的图像内容，但整体退到墙面的明度附近。
   */
  const { canvas, ctx } = makeCanvas(256, 384);

  // 天空：上淡下暖的米色渐变
  const g = ctx.createLinearGradient(0, 0, 0, 384);
  g.addColorStop(0.0, '#dfe6ea');
  g.addColorStop(0.55, '#eae4db');
  g.addColorStop(1.0, '#f2ece1');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 256, 384);

  // 日轮：低对比的一轮淡色，给画面一个焦点但不刺眼
  ctx.fillStyle = '#f0dfc8';
  ctx.beginPath();
  ctx.arc(170, 132, 34, 0, Math.PI * 2);
  ctx.fill();

  // 远山：两层，越远越淡（空气透视）
  const ridge = (baseY: number, amp: number, color: string, seed: number) => {
    const rnd = makeRng(seed);
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(0, 384);
    ctx.lineTo(0, baseY);
    for (let x = 0; x <= 256; x += 16) {
      ctx.lineTo(x, baseY - Math.sin(x * 0.02 + seed) * amp - rnd() * amp * 0.5);
    }
    ctx.lineTo(256, 384);
    ctx.closePath();
    ctx.fill();
  };
  ridge(238, 22, '#b9c3c6', 11);   // 最远的脊线，灰青
  ridge(284, 26, '#9aa89c', 23);   // 中景，灰绿
  ridge(330, 20, '#8d9a82', 37);   // 近坡，稍深的灰绿

  // 留白边框：画本身要有边，不然像直接印在墙上
  ctx.strokeStyle = '#f7f3ea';
  ctx.lineWidth = 14;
  ctx.strokeRect(7, 7, 242, 370);

  _poster = toTexture(canvas, [1, 1]);
  return _poster;
}

/* ---------------- 显示器画面 ---------------- */

let _screen: THREE.Texture | null = null;
export function screenTexture(): THREE.Texture {
  if (_screen) return _screen;
  const { canvas, ctx } = makeCanvas(256, 160);
  ctx.fillStyle = '#161b26';
  ctx.fillRect(0, 0, 256, 160);
  ctx.fillStyle = '#232a3a';
  ctx.fillRect(0, 0, 256, 18);

  // 标题栏圆点
  for (const [x, c] of [[10, '#ff6b81'], [24, '#ffd166'], [38, '#5fd6a4']] as Array<[number, string]>) {
    ctx.fillStyle = c;
    ctx.beginPath();
    ctx.arc(x, 9, 4, 0, Math.PI * 2);
    ctx.fill();
  }

  // 代码行
  const rnd = makeRng(555);
  const colors = ['#7fd8c8', '#e7a3c0', '#9fb8ff', '#f2d18a'];
  let y = 30;
  while (y < 152) {
    const indent = 12 + Math.floor(rnd() * 3) * 12;
    const len = 40 + rnd() * 150;
    ctx.fillStyle = colors[Math.floor(rnd() * colors.length)];
    ctx.globalAlpha = 0.85;
    ctx.fillRect(indent, y, Math.min(len, 244 - indent), 5);
    ctx.globalAlpha = 1;
    y += 13;
  }
  // 光标
  ctx.fillStyle = '#ffffff';
  ctx.fillRect(12, 146, 9, 6);

  _screen = toTexture(canvas, [1, 1]);
  return _screen;
}

/* ---------------- 柔光圆点（浮尘） ---------------- */

let _dot: THREE.Texture | null = null;
export function softDotTexture(): THREE.Texture {
  if (_dot) return _dot;
  const { canvas, ctx } = makeCanvas(64, 64);
  const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
  g.addColorStop(0, 'rgba(255,250,230,1)');
  g.addColorStop(0.35, 'rgba(255,244,210,0.55)');
  g.addColorStop(1, 'rgba(255,240,200,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  _dot = t;
  return t;
}

/* ---------------- 光束：沿长度衰减 + 两侧羽化 ---------------- */

let _beam: THREE.Texture | null = null;
export function beamTexture(): THREE.Texture {
  if (_beam) return _beam;
  const { canvas, ctx } = makeCanvas(64, 64);
  const img = ctx.createImageData(64, 64);
  for (let y = 0; y < 64; y++) {
    for (let x = 0; x < 64; x++) {
      const u = x / 63;
      const v = y / 63;
      const along = Math.pow(1 - u, 1.35);
      const across = Math.pow(Math.sin(v * Math.PI), 0.7);
      const edge = Math.min(1, Math.min(v, 1 - v) / 0.16);
      const a = Math.max(0, along * across * edge);
      const i = (y * 64 + x) * 4;
      img.data[i] = 255;
      img.data[i + 1] = 248;
      img.data[i + 2] = 226;
      img.data[i + 3] = Math.round(a * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _beam = t;
  return t;
}

/* ---------------- 大理石（Calacatta 白底浅金灰纹） ---------------- */

/**
 * 豪华公寓的台面/茶几/岛台用。白底上蜿蜒的浅色脉络——
 * 脉络是"从某点出发、方向不断微扰的曲线"，不是直线也不是规则网格，
 * 这样远看像天然大理石，近看不会像贴图重复。
 */
let _marble: THREE.Texture | null = null;
export function marbleTexture(): THREE.Texture {
  if (_marble) return _marble;
  const { canvas, ctx } = makeCanvas(512, 512);
  ctx.fillStyle = '#F5F3EE';
  ctx.fillRect(0, 0, 512, 512);

  // 底色的温润不均（大理石不是死白，是暖白里带着极淡的云絮）
  const rnd = makeRng(20260901);
  for (let i = 0; i < 40; i++) {
    ctx.globalAlpha = 0.03 + rnd() * 0.04;
    ctx.fillStyle = rnd() > 0.5 ? '#EAE6DC' : '#FFFFFF';
    ctx.beginPath();
    ctx.ellipse(rnd() * 512, rnd() * 512, 40 + rnd() * 100, 30 + rnd() * 80, rnd() * 3, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  /**
   * 一条脉络：从 (x0, y0) 出发沿 angle 走，方向每步微扰。
   * 浅金的粗脉少而醒目，浅灰的细脉多而淡——层次靠这个拉开。
   */
  const vein = (x0: number, y0: number, angle: number, length: number, color: string, alpha: number, width: number) => {
    let x = x0, y = y0, a = angle;
    ctx.strokeStyle = color;
    ctx.globalAlpha = alpha;
    ctx.lineWidth = width;
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';
    ctx.beginPath();
    ctx.moveTo(x, y);
    const steps = 24;
    for (let s = 0; s < steps; s++) {
      a += (rnd() - 0.5) * 0.7;
      const stepLen = length / steps;
      x += Math.cos(a) * stepLen;
      y += Math.sin(a) * stepLen;
      ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  };

  // 主脉（浅金，3-5 条，粗且醒目）
  for (let i = 0; i < 4; i++) {
    vein(rnd() * 512, rnd() * 512, rnd() * Math.PI * 2, 220 + rnd() * 200, '#C9A961', 0.22 + rnd() * 0.14, 1.2 + rnd() * 1.0);
  }
  // 次脉（浅灰，10-14 条，细且淡）
  for (let i = 0; i < 12; i++) {
    vein(rnd() * 512, rnd() * 512, rnd() * Math.PI * 2, 140 + rnd() * 160, '#B8B0A4', 0.10 + rnd() * 0.12, 0.5 + rnd() * 0.6);
  }

  _marble = toTexture(canvas, [1, 1]);
  return _marble;
}

/* ---------------- 天鹅绒（软包/沙发面料） ---------------- */

/**
 * 软包床头、弧形沙发、陈述椅的面料。深色底 + 极细的竖向刷痕，
 * 远看是纯色的哑光织物，近看有"绒毛被手抚过"的方向感。
 */
let _velvet: THREE.Texture | null = null;
export function velvetTexture(base = '#5A6E54'): THREE.Texture {
  if (_velvet) return _velvet;
  const { canvas, ctx } = makeCanvas(256, 256);
  ctx.fillStyle = base;
  ctx.fillRect(0, 0, 256, 256);

  const rnd = makeRng(7788);
  // 竖向刷痕：明暗交错的细竖条，模拟绒毛倒向
  for (let x = 0; x < 256; x += 2) {
    const v = (rnd() - 0.5) * 0.06;
    ctx.globalAlpha = 0.5;
    ctx.fillStyle = v > 0 ? `rgba(255,255,255,${v})` : `rgba(0,0,0,${-v})`;
    ctx.fillRect(x, 0, 1, 256);
  }
  ctx.globalAlpha = 1;

  // 局部高光团（坐过/靠过的地方绒毛倒向不同）
  for (let i = 0; i < 8; i++) {
    ctx.globalAlpha = 0.04 + rnd() * 0.05;
    ctx.fillStyle = rnd() > 0.5 ? '#FFFFFF' : '#000000';
    ctx.beginPath();
    ctx.ellipse(rnd() * 256, rnd() * 256, 20 + rnd() * 50, 15 + rnd() * 40, rnd() * 3, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  _velvet = toTexture(canvas, [2, 2]);
  return _velvet;
}

/* ---------------- 地面光斑 ---------------- */

let _pool: THREE.Texture | null = null;
export function lightPoolTexture(): THREE.Texture {
  if (_pool) return _pool;
  const { canvas, ctx } = makeCanvas(128, 128);
  const g = ctx.createRadialGradient(64, 64, 4, 64, 64, 64);
  g.addColorStop(0, 'rgba(255,247,225,0.95)');
  g.addColorStop(0.5, 'rgba(255,240,208,0.42)');
  g.addColorStop(1, 'rgba(255,236,200,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 128, 128);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  _pool = t;
  return t;
}

/* ---------------- 湿地上的拉长倒影 ---------------- */

/**
 * 湿地面把路灯、店铺灯箱、亮着的窗户拉成一条竖着的软光带。
 *
 * 和上面那团圆光晕的区别是形状：光晕是"灯正下方有一摊亮"，拉长倒影是
 * "亮处朝观察者方向拖出一条"。夜里站在街上看，后者才是湿路面的主要特征
 * ——整条街都是被拖长的光，一团圆光晕反而只在灯脚下那一小块成立。
 *
 * 纹理里 V 方向 = 世界 Z（拖长方向）：顶端（贴着光源那头）实，尾巴散开消失；
 * U 方向两边羽化，免得光带两侧是一条硬边。
 */
let _wetStreak: THREE.Texture | null = null;
export function wetStreakTexture(): THREE.Texture {
  if (_wetStreak) return _wetStreak;
  const W = 64, H = 256;
  const { canvas, ctx } = makeCanvas(W, H);

  const v = ctx.createLinearGradient(0, 0, 0, H);
  v.addColorStop(0.00, 'rgba(255,255,255,0)');
  v.addColorStop(0.14, 'rgba(255,255,255,0.88)');
  v.addColorStop(0.42, 'rgba(255,255,255,0.40)');
  v.addColorStop(0.72, 'rgba(255,255,255,0.14)');
  v.addColorStop(1.00, 'rgba(255,255,255,0)');
  ctx.fillStyle = v;
  ctx.fillRect(0, 0, W, H);

  ctx.globalCompositeOperation = 'destination-in';
  const h = ctx.createLinearGradient(0, 0, W, 0);
  h.addColorStop(0.00, 'rgba(0,0,0,0)');
  h.addColorStop(0.30, 'rgba(0,0,0,0.85)');
  h.addColorStop(0.50, 'rgba(0,0,0,1)');
  h.addColorStop(0.70, 'rgba(0,0,0,0.85)');
  h.addColorStop(1.00, 'rgba(0,0,0,0)');
  ctx.fillStyle = h;
  ctx.fillRect(0, 0, W, H);
  ctx.globalCompositeOperation = 'source-over';

  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _wetStreak = t;
  return t;
}

/* ---------------- 雨水涟漪 ---------------- */

/**
 * 湿地面上"雨打积水"形成的一圈圈同心环。
 *
 * 参考图里便利店前那段湿地面，能看到一圈圈淡淡的圆环——那是动画背景里
 * 表现"雨持续落在湿地上"的惯用画法。三圈同心软环 + 中心一个小亮点，
 * 随机透明度叠在一起形成"这片地方刚被雨敲过"的视觉信号。
 *
 * 用法：在 buildWetGround 之后，调用 buildPuddleRipples(area, count) 撒一片。
 */
let _puddleRipple: THREE.Texture | null = null;
export function puddleRippleTexture(): THREE.Texture {
  if (_puddleRipple) return _puddleRipple;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const cx = S / 2, cy = S / 2;
  const img = ctx.createImageData(S, S);
  // 三圈同心软环，半径依次为 0.18 / 0.40 / 0.62（占纹理归一化半径）
  const rings = [0.18, 0.40, 0.62];
  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const dx = (x - cx) / (S / 2), dy = (y - cy) / (S / 2);
      const r = Math.sqrt(dx * dx + dy * dy);
      let a = 0;
      for (const rr of rings) {
        const w = 0.05;
        a += Math.exp(-((r - rr) ** 2) / (w * w)) * 0.32;
      }
      // 中心点：刚才那一滴落下来的位置
      a += Math.exp(-r * r * 36) * 0.55;
      if (a > 0.95) a = 0.95;
      const idx = (y * S + x) * 4;
      img.data[idx]     = Math.round(196 * a);
      img.data[idx + 1] = Math.round(214 * a);
      img.data[idx + 2] = Math.round(236 * a);
      img.data[idx + 3] = Math.round(a * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _puddleRipple = t;
  return t;
}

/* ---------------- 屋檐滴水 ---------------- */

/**
 * 阳台/雨棚外缘悬下来的一条短水线，底部挂着一颗水珠。
 *
 * 屋檐滴水是雨夜里最容易被忽略、但缺了就不对劲的细节——没有它，
 * 阳台栏杆下沿是干的，少了"雨刚刚顺着檐口滴下去"的那一下。
 *
 * 纹理：上半段一条细亮线（流下来的水痕），下半段一个圆点（汇聚的
 * 水珠）。竖向构图，贴到一个略宽于雨丝的矩形上，下沿刚好落在阳台
 * 外缘下方一点。
 */
let _drip: THREE.Texture | null = null;
export function dripTexture(): THREE.Texture {
  if (_drip) return _drip;
  const W = 18, H = 90;
  const { canvas, ctx } = makeCanvas(W, H);
  const cx = W / 2;
  const img = ctx.createImageData(W, H);

  // 水线：从顶端往下，颜色先实后淡
  const lineAlpha = (y: number) => {
    const t = y / H;
    if (t < 0.65) return 0.55 * (1 - t * 0.6);
    return 0.22 * (1 - (t - 0.65) / 0.35);
  };
  // 水珠：底部约 1/4 处一个圆点
  const dotCenter = { y: H * 0.82, r: 4.2 };

  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const dx = (x - cx) / (W / 2);
      // 线：高斯型横向衰减
      const lineFall = Math.exp(-dx * dx * 6);
      const aLine = lineAlpha(y) * lineFall;
      // 珠
      const ddy = y - dotCenter.y, ddx = x - cx;
      const d = Math.sqrt(ddx * ddx + ddy * ddy);
      const aDot = Math.exp(-(d * d) / (dotCenter.r * dotCenter.r * 0.5)) * 0.85;
      const a = Math.max(aLine, aDot);
      const idx = (y * W + x) * 4;
      img.data[idx]     = Math.round(210 * a);
      img.data[idx + 1] = Math.round(224 * a);
      img.data[idx + 2] = Math.round(244 * a);
      img.data[idx + 3] = Math.round(a * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  const t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.ClampToEdgeWrapping;
  _drip = t;
  return t;
}

/* ============================================================================
 * 4. 日式公寓（LDK 重设计）程序化贴图
 *    《你的名字》泷的日常公寓风：浅橡木 / 靛蓝 / 米白 / 暖灰 / 陶土。
 *    全部自绘，模块级单例，不调用第 3 节任何已有纹理函数。
 * ========================================================================== */

/** 本区专用的 #rrggbb → [r,g,b] 解析，独立于既有代码。 */
function jpHex(hex: string): [number, number, number] {
  const h = hex.replace('#', '');
  const n = parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** 在 ImageData 上叠一个带随机抖动的像素，越界自动忽略。 */
function jpPut(img: ImageData, x: number, y: number, r: number, g: number, b: number, a = 255) {
  if (x < 0 || y < 0 || x >= img.width || y >= img.height) return;
  const idx = (y * img.width + x) * 4;
  img.data[idx] = r;
  img.data[idx + 1] = g;
  img.data[idx + 2] = b;
  img.data[idx + 3] = a;
}

/* --- 1. 浅橡木宽板地板（living / dining） ------------------------------- */
let _jpWoodFloor: THREE.Texture | null = null;
export function jpWoodFloorTexture(): THREE.Texture {
  if (_jpWoodFloor) return _jpWoodFloor;
  const S = 512;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#dcc39c');
  const dark = jpHex('#b89a6d');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(4101);
  const plankH = Math.round(S / 6); // 6 条横向宽板

  for (let y = 0; y < S; y++) {
    const row = Math.floor(y / plankH);
    // 每块板一个轻微的整体明度偏移
    const rowTone = (rng() - 0.5) * 0; // 预置，避免逐行重掷导致纹理跳变
    void rowTone;
    const stagger = (row % 2) * (S / 2);
    for (let x = 0; x < S; x++) {
      // 木纹：沿板长方向的细密正弦 + 噪声
      const grain = Math.sin((x + stagger) * 0.09 + row * 2.3) * 0.5
        + Math.sin((x + stagger) * 0.021) * 0.3
        + (rng() - 0.5) * 0.5;
      let k = 0.5 + grain * 0.10;
      // 板缝（横向）：每 plankH 一条深色线
      const seamY = y % plankH;
      if (seamY < 2) k *= 0.72;
      // 板缝（纵向）：错缝拼接
      const seamX = (x + stagger) % (S / 2);
      if (seamX < 2) k *= 0.78;
      const rr = Math.max(0, Math.min(255, base[0] * k - (1 - k) * 8 + (dark[0] - base[0]) * (1 - k) * 0.4));
      const gg = Math.max(0, Math.min(255, base[1] * k + (dark[1] - base[1]) * (1 - k) * 0.4));
      const bb = Math.max(0, Math.min(255, base[2] * k + (dark[2] - base[2]) * (1 - k) * 0.4));
      jpPut(img, x, y, rr, gg, bb);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpWoodFloor = toTexture(canvas, [1, 1]);
  return _jpWoodFloor;
}

/* --- 2. 暖灰小方砖地板（kitchen） --------------------------------------- */
let _jpKitchenFloor: THREE.Texture | null = null;
export function jpKitchenFloorTexture(): THREE.Texture {
  if (_jpKitchenFloor) return _jpKitchenFloor;
  const S = 512;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#c9c2b4');
  const grout = jpHex('#a49b8b');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(7712);
  const T = S / 4; // 4×4 砖

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const gx = x % T, gy = y % T;
      const onGrout = gx < 3 || gy < 3 || gx > T - 3 || gy > T - 3;
      const speck = (rng() - 0.5) * 10;
      const c = onGrout ? grout : base;
      const k = onGrout ? 1 : 1 + speck * 0.01;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, c[0] * k)),
        Math.max(0, Math.min(255, c[1] * k)),
        Math.max(0, Math.min(255, c[2] * k)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpKitchenFloor = toTexture(canvas, [1, 1]);
  return _jpKitchenFloor;
}

/* --- 3. 家具橡木纹（桌 / 柜 / 架 / 台面门板） --------------------------- */
let _jpWoodGrain: THREE.Texture | null = null;
export function jpWoodGrainTexture(): THREE.Texture {
  if (_jpWoodGrain) return _jpWoodGrain;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#c8a87c');
  const dark = jpHex('#9a7b52');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(2093);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      // 竖向木纹（家具立面）：低频起伏 + 细纹
      const warp = Math.sin(y * 0.035 + Math.sin(x * 0.02) * 1.6) * 0.5 + 0.5;
      const fine = Math.sin(y * 0.6 + x * 0.05) * 0.08 + (rng() - 0.5) * 0.10;
      const k = 0.82 + warp * 0.20 + fine;
      const t = 1 - k;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, base[0] * k + dark[0] * t * 0.6)),
        Math.max(0, Math.min(255, base[1] * k + dark[1] * t * 0.6)),
        Math.max(0, Math.min(255, base[2] * k + dark[2] * t * 0.6)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpWoodGrain = toTexture(canvas, [2, 2]);
  return _jpWoodGrain;
}

/* --- 4. 棉麻织纹（沙发 / 座垫 / 椅面） ---------------------------------- */
let _jpFabric: THREE.Texture | null = null;
export function jpFabricTexture(): THREE.Texture {
  if (_jpFabric) return _jpFabric;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#e8e4dc');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(5150);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      // 平纹编织：经纬交替的明暗格
      const warp = ((x >> 1) + (y >> 1)) % 2 === 0 ? 1 : -1;
      const k = 0.90 + warp * 0.06 + (rng() - 0.5) * 0.10;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, base[0] * k)),
        Math.max(0, Math.min(255, base[1] * k)),
        Math.max(0, Math.min(255, base[2] * k)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpFabric = toTexture(canvas, [3, 3]);
  return _jpFabric;
}

/* --- 5. 靛蓝低绒地毯（客厅 / 餐区，麻叶 + 絣菱格） ---------------------- */
let _jpRug: THREE.Texture | null = null;
export function jpRugTexture(): THREE.Texture {
  if (_jpRug) return _jpRug;
  const S = 512;
  const { canvas, ctx } = makeCanvas(S, S);
  const cream = jpHex('#ece5d8');
  const indigo = jpHex('#3e5c76');
  const terra = jpHex('#b5715a');
  const rng = makeRng(9021);

  // 底色绒面
  ctx.fillStyle = `rgb(${cream[0]},${cream[1]},${cream[2]})`;
  ctx.fillRect(0, 0, S, S);
  // 绒毛噪点
  const img = ctx.getImageData(0, 0, S, S);
  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const n = (rng() - 0.5) * 14;
      const idx = (y * S + x) * 4;
      img.data[idx] = Math.max(0, Math.min(255, img.data[idx] + n));
      img.data[idx + 1] = Math.max(0, Math.min(255, img.data[idx + 1] + n));
      img.data[idx + 2] = Math.max(0, Math.min(255, img.data[idx + 2] + n));
    }
  }
  ctx.putImageData(img, 0, 0);

  // 靛蓝边框
  const bw = 26;
  ctx.strokeStyle = `rgb(${indigo[0]},${indigo[1]},${indigo[2]})`;
  ctx.lineWidth = bw;
  ctx.strokeRect(bw / 2, bw / 2, S - bw, S - bw);
  // 内细边
  ctx.lineWidth = 4;
  ctx.strokeStyle = `rgb(${terra[0]},${terra[1]},${terra[2]})`;
  ctx.strokeRect(bw + 12, bw + 12, S - 2 * (bw + 12), S - 2 * (bw + 12));

  // 中央絣菱格 3×3
  const cell = (S - 2 * (bw + 34)) / 3;
  const ox = bw + 34, oy = bw + 34;
  for (let gy = 0; gy < 3; gy++) {
    for (let gx = 0; gx < 3; gx++) {
      const cx = ox + cell * (gx + 0.5);
      const cy = oy + cell * (gy + 0.5);
      const r = cell * 0.30;
      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(Math.PI / 4);
      ctx.strokeStyle = `rgb(${indigo[0]},${indigo[1]},${indigo[2]})`;
      ctx.lineWidth = 5;
      ctx.strokeRect(-r, -r, r * 2, r * 2);
      ctx.strokeStyle = `rgba(${terra[0]},${terra[1]},${terra[2]},0.85)`;
      ctx.lineWidth = 3;
      ctx.strokeRect(-r * 0.5, -r * 0.5, r, r);
      ctx.restore();
    }
  }
  _jpRug = toTexture(canvas, [1, 1]);
  _jpRug.wrapS = _jpRug.wrapT = THREE.ClampToEdgeWrapping;
  return _jpRug;
}

/* --- 6. 障子纸 + 组子格栅（吊灯灯罩） ----------------------------------- */
let _jpShoji: THREE.Texture | null = null;
export function jpShojiPaperTexture(): THREE.Texture {
  if (_jpShoji) return _jpShoji;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const paper = jpHex('#f6efe0');
  const kumiko = jpHex('#b08e62');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(3388);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const fiber = (rng() - 0.5) * 12 + Math.sin(y * 0.8) * 3;
      const k = 1 + fiber * 0.01;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, paper[0] * k)),
        Math.max(0, Math.min(255, paper[1] * k)),
        Math.max(0, Math.min(255, paper[2] * k)));
    }
  }
  ctx.putImageData(img, 0, 0);
  // 组子格栅：2×3
  ctx.strokeStyle = `rgb(${kumiko[0]},${kumiko[1]},${kumiko[2]})`;
  ctx.lineWidth = 6;
  for (let i = 1; i < 2; i++) {
    ctx.beginPath(); ctx.moveTo((S / 2) * i, 0); ctx.lineTo((S / 2) * i, S); ctx.stroke();
  }
  for (let i = 1; i < 3; i++) {
    ctx.beginPath(); ctx.moveTo(0, (S / 3) * i); ctx.lineTo(S, (S / 3) * i); ctx.stroke();
  }
  ctx.lineWidth = 8;
  ctx.strokeRect(4, 4, S - 8, S - 8);
  _jpShoji = toTexture(canvas, [1, 1]);
  return _jpShoji;
}

/* --- 7. 上釉陶瓷（马克杯 / 碗 / 钵 / 花瓶 / 餐具） ---------------------- */
let _jpCeramic: THREE.Texture | null = null;
export function jpCeramicTexture(): THREE.Texture {
  if (_jpCeramic) return _jpCeramic;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#ede8e0');
  const speck = jpHex('#7c99ac');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(6407);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const n = rng();
      let r = base[0], g = base[1], b = base[2];
      const k = 1 + (rng() - 0.5) * 0.05;
      r *= k; g *= k; b *= k;
      if (n > 0.985) { // 稀疏釉点
        r = speck[0]; g = speck[1]; b = speck[2];
      }
      jpPut(img, x, y, r | 0, g | 0, b | 0);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpCeramic = toTexture(canvas, [1, 1]);
  return _jpCeramic;
}

/* --- 8. 拉丝金属（水槽 / 龙头 / 灯杆 / 把手 / 电视边框） ---------------- */
let _jpMetal: THREE.Texture | null = null;
export function jpMetalTexture(): THREE.Texture {
  if (_jpMetal) return _jpMetal;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#a9adb3');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(1187);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      // 横向拉丝：沿 x 的低频条带 + 细噪
      const brush = Math.sin(y * 1.7) * 0.5 + (rng() - 0.5) * 0.6;
      const k = 1 + brush * 0.08;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, base[0] * k)),
        Math.max(0, Math.min(255, base[1] * k)),
        Math.max(0, Math.min(255, base[2] * k)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpMetal = toTexture(canvas, [2, 2]);
  return _jpMetal;
}

/* --- 9. 书脊 / 漫画封面（书架 + 地面杂物） ------------------------------ */
let _jpBook: THREE.Texture | null = null;
export function jpPaperBookTexture(): THREE.Texture {
  if (_jpBook) return _jpBook;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const rng = makeRng(8834);
  const spines = ['#3e5c76', '#b5715a', '#6e8b5e', '#2c4257', '#c8a87c', '#7c99ac', '#f0e9dc', '#9a7b52'];

  ctx.fillStyle = '#f0e9dc';
  ctx.fillRect(0, 0, S, S);
  // 竖向书脊条带
  let x = 0;
  while (x < S) {
    const w = 12 + Math.floor(rng() * 20);
    const col = jpHex(spines[Math.floor(rng() * spines.length)]);
    ctx.fillStyle = `rgb(${col[0]},${col[1]},${col[2]})`;
    ctx.fillRect(x, 0, w, S);
    // 书脊上的两道金线
    ctx.fillStyle = 'rgba(240,233,220,0.55)';
    ctx.fillRect(x + 2, S * 0.22, w - 4, 3);
    ctx.fillRect(x + 2, S * 0.72, w - 4, 3);
    // 分隔暗线
    ctx.fillStyle = 'rgba(43,46,51,0.35)';
    ctx.fillRect(x + w - 1, 0, 1, S);
    x += w;
  }
  _jpBook = toTexture(canvas, [1, 1]);
  return _jpBook;
}

/* --- 10. 厨房台面（哑光石 / 复合板） ------------------------------------ */
let _jpCounterTop: THREE.Texture | null = null;
export function jpCounterTopTexture(): THREE.Texture {
  if (_jpCounterTop) return _jpCounterTop;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#e4ded2');
  const fleck = jpHex('#b9b2a6');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(2266);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const k = 1 + (rng() - 0.5) * 0.04;
      let r = base[0] * k, g = base[1] * k, b = base[2] * k;
      if (rng() > 0.97) { r = fleck[0]; g = fleck[1]; b = fleck[2]; }
      jpPut(img, x, y, r | 0, g | 0, b | 0);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpCounterTop = toTexture(canvas, [2, 2]);
  return _jpCounterTop;
}

/* --- 11. 冰箱哑光门板 --------------------------------------------------- */
let _jpFridgePanel: THREE.Texture | null = null;
export function jpFridgePanelTexture(): THREE.Texture {
  if (_jpFridgePanel) return _jpFridgePanel;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const top = jpHex('#f4f1e8');
  const bot = jpHex('#ddd8cc');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(5599);

  for (let y = 0; y < S; y++) {
    const t = y / S; // 竖向渐变，模拟哑光面板的柔光
    for (let x = 0; x < S; x++) {
      const k = 1 + (rng() - 0.5) * 0.03;
      const r = (top[0] * (1 - t) + bot[0] * t) * k;
      const g = (top[1] * (1 - t) + bot[1] * t) * k;
      const b = (top[2] * (1 - t) + bot[2] * t) * k;
      jpPut(img, x, y, r | 0, g | 0, b | 0);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpFridgePanel = toTexture(canvas, [1, 1]);
  return _jpFridgePanel;
}

/* --- 12. 阳台防腐木（balcony deck） ------------------------------------- */
let _jpDeckWood: THREE.Texture | null = null;
export function jpDeckWoodTexture(): THREE.Texture {
  if (_jpDeckWood) return _jpDeckWood;
  const S = 512;
  const { canvas, ctx } = makeCanvas(S, S);
  const base = jpHex('#9a8869');
  const dark = jpHex('#6f6049');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(8123);
  const plankW = Math.round(S / 7); // 7 条竖向板

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const col = Math.floor(x / plankW);
      // 风化木纹：沿板长的粗犷条纹 + 噪声，整体偏灰
      const grain = Math.sin(y * 0.06 + col * 3.1) * 0.5
        + Math.sin(y * 0.017 + col) * 0.35
        + (rng() - 0.5) * 0.7;
      let k = 0.52 + grain * 0.12;
      // 板缝（竖向）：每条板之间一道深色缝，缝内更暗
      const seamX = x % plankW;
      if (seamX < 2) k *= 0.55;
      // 端头横缝：错缝拼接
      const stagger = (col % 2) * (S / 2);
      const seamY = (y + stagger) % (S / 2);
      if (seamY < 2) k *= 0.7;
      const rr = Math.max(0, Math.min(255, base[0] * k + (dark[0] - base[0]) * (1 - k) * 0.5));
      const gg = Math.max(0, Math.min(255, base[1] * k + (dark[1] - base[1]) * (1 - k) * 0.5));
      const bb = Math.max(0, Math.min(255, base[2] * k + (dark[2] - base[2]) * (1 - k) * 0.5));
      jpPut(img, x, y, rr, gg, bb);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpDeckWood = toTexture(canvas, [1, 1]);
  return _jpDeckWood;
}

/* --- 13. 布団床品（靛蓝 + 米白 kasuri 条纹） ----------------------------- */
let _jpFuton: THREE.Texture | null = null;
export function jpFutonTexture(): THREE.Texture {
  if (_jpFuton) return _jpFuton;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const cream = jpHex('#ece5d8');
  const indigo = jpHex('#3e5c76');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(6021);

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const n = (rng() - 0.5) * 10; // 织物抖动
      // 宽窄相间的靛蓝条纹（kasuri 风格），边缘略带晕染
      const band = Math.floor(y / 16);
      const inStripe = band % 4 === 0 || band % 4 === 1;
      const edge = (y % 16) < 1 || (y % 16) > 14;
      let c = inStripe ? indigo : cream;
      if (inStripe && edge) c = cream; // 条纹边缘留米白细线，模拟絣织
      // 纵向织纹
      const weave = (x % 3 === 0) ? -6 : 0;
      jpPut(img, x, y,
        Math.max(0, Math.min(255, c[0] + n + weave)),
        Math.max(0, Math.min(255, c[1] + n + weave)),
        Math.max(0, Math.min(255, c[2] + n + weave)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpFuton = toTexture(canvas, [2, 2]);
  return _jpFuton;
}

/* --- 14. 日式海报（《你的名字》彗星夜空母题） --------------------------- */
let _jpPoster: THREE.Texture | null = null;
export function jpPosterTexture(): THREE.Texture {
  if (_jpPoster) return _jpPoster;
  const W = 256, H = 384;
  const { canvas, ctx } = makeCanvas(W, H);
  const img = ctx.createImageData(W, H);
  const rng = makeRng(3307);
  const skyTop = jpHex('#16233c');
  const skyBot = jpHex('#3c5a86');
  const warm = jpHex('#e8b06a');

  for (let y = 0; y < H; y++) {
    const t = y / H;
    for (let x = 0; x < W; x++) {
      // 夜空竖向渐变
      let r = skyTop[0] * (1 - t) + skyBot[0] * t;
      let g = skyTop[1] * (1 - t) + skyBot[1] * t;
      let b = skyTop[2] * (1 - t) + skyBot[2] * t;
      // 星点
      if (rng() > 0.9975 && t < 0.7) { r += 90; g += 90; b += 80; }
      // 彗星：一道自右上向左下的暖色亮带
      const dx = x - (W * 0.78 - t * W * 0.5);
      const dy = y - (H * 0.12 + t * H * 0.5);
      const d = Math.abs(dx * 0.6 + dy * 0.8) / 26;
      if (d < 1) {
        const fall = (1 - d) * (1 - Math.abs(dy) / (H * 0.6));
        if (fall > 0) { r += warm[0] * fall * 0.8; g += warm[1] * fall * 0.7; b += warm[2] * fall * 0.4; }
      }
      // 底部山影
      const ridge = H * 0.82 + Math.sin(x * 0.05) * 10;
      if (y > ridge) { r *= 0.35; g *= 0.38; b *= 0.5; }
      jpPut(img, x, y,
        Math.max(0, Math.min(255, r | 0)),
        Math.max(0, Math.min(255, g | 0)),
        Math.max(0, Math.min(255, b | 0)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpPoster = toTexture(canvas, [1, 1]);
  return _jpPoster;
}

/* --- 15. 钟面（极简刻度盘） --------------------------------------------- */
let _jpClockFace: THREE.Texture | null = null;
export function jpClockFaceTexture(): THREE.Texture {
  if (_jpClockFace) return _jpClockFace;
  const S = 128;
  const { canvas, ctx } = makeCanvas(S, S);
  const img = ctx.createImageData(S, S);
  const face = jpHex('#f3eee4');
  const ink = jpHex('#3a3f47');
  const cx = S / 2, cy = S / 2, R = S / 2;

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const dx = x - cx, dy = y - cy;
      const dist = Math.sqrt(dx * dx + dy * dy);
      let c = face;
      if (dist > R - 2) c = ink; // 外圈描边
      // 12 个刻度：每逢 30° 一道短墨线
      const ang = Math.atan2(dy, dx);
      const tick = Math.abs(((ang + Math.PI) % (Math.PI / 6)) - Math.PI / 12);
      if (dist > R - 14 && dist < R - 4 && tick < 0.06) c = ink;
      jpPut(img, x, y, c[0], c[1], c[2]);
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpClockFace = toTexture(canvas, [1, 1]);
  return _jpClockFace;
}

/* --- 16. 卫浴釉面白砖（ユニットバス 内壁） ------------------------------ */
let _jpBathTile: THREE.Texture | null = null;
export function jpBathTileTexture(): THREE.Texture {
  if (_jpBathTile) return _jpBathTile;
  const S = 256;
  const { canvas, ctx } = makeCanvas(S, S);
  const tile = jpHex('#eef0ee');
  const grout = jpHex('#c3c8c6');
  const img = ctx.createImageData(S, S);
  const rng = makeRng(9044);
  const TW = S / 4, TH = S / 8; // 地铁砖：宽 1/4，高 1/8，错缝

  for (let y = 0; y < S; y++) {
    const row = Math.floor(y / TH);
    const stagger = (row % 2) * (TW / 2);
    for (let x = 0; x < S; x++) {
      const gx = (x + stagger) % TW;
      const gy = y % TH;
      const isGrout = gx < 2 || gy < 2;
      const c = isGrout ? grout : tile;
      const n = (rng() - 0.5) * (isGrout ? 8 : 5);
      // 釉面：砖心略亮，靠边微暗
      const sh = isGrout ? 0 : (1 - Math.min(gx, TW - gx) / TW * 0.06);
      jpPut(img, x, y,
        Math.max(0, Math.min(255, c[0] * sh + n)),
        Math.max(0, Math.min(255, c[1] * sh + n)),
        Math.max(0, Math.min(255, c[2] * sh + n)));
    }
  }
  ctx.putImageData(img, 0, 0);
  _jpBathTile = toTexture(canvas, [2, 2]);
  return _jpBathTile;
}
