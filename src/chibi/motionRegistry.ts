import { CHARACTER_IDS } from '../characters';
import { characterAssetSource, characterFrameData, characterAnimation } from '../characterPackages';
/**
 * Q 版桌宠动作词汇表。
 *
 * `animations.json` 声明通用动作语义与内置角色的播放配置；导入角色的图集分格、
 * 每帧时长和循环方式由角色包的 `animations` 声明。本模块按角色选取配置，
 * 再归一化成渲染器可直接消费的结构。
 *
 * 同一个词表也被后端读取（构建期嵌入），保证 prompt 里列出的动作名与前端能播的
 * 动作完全一致——曾经两边的词汇是各自硬编码的，后端列动作、前端只有
 * Q 版图集，导致动作永远选不中。
 */

import vocab from './animations.json';
import frameAnchors from './frameAnchors.json';
import frameScales from './frameScales.json';
import { optionalAssetSource } from '../utils/optionalResources';

export const CHIBI_CHARACTER_IDS: readonly string[] = CHARACTER_IDS;

export type ChibiDirection = 'left' | 'right';

/** 图集格位：可持续的定格姿态，需要外部给定持续时长。 */
export interface ChibiPoseSpec {
  kind: 'pose';
  name: string;
  label: string;
  category: string;
  promptable: boolean;
  /** 图集内的格位序号（按行优先）。 */
  slot: number;
}

/** 帧序列：自带节奏的一次性动作，播完自动回到 pose。 */
export interface ChibiAnimationSpec {
  kind: 'animation';
  name: string;
  label: string;
  category: string;
  promptable: boolean;
  /** 省略时两个角色通用，否则只供这些角色使用。 */
  characters?: string[];
  /** Characters that must not play the motion, including from older installed packs. */
  excludedCharacters?: string[];
  /** 图集路径模板，`{character}` 与 `{direction}` 由调用方填充。 */
  sheet: string;
  /** 有方向的动作为 ['left','right']，无方向为 null。 */
  directions: ChibiDirection[] | null;
  /** Both directions share a left-facing sheet; render right by horizontal reflection. */
  mirrorRight?: boolean;
  /** Overrides the legacy left-side reflection for edge and lean. */
  mirrorLeft?: boolean;
  /** Optional palm contact on the final lean frame, by playback direction, in 512-cell source coordinates. */
  palmContact?: Partial<Record<ChibiDirection, readonly [x: number, y: number]>>;
  /** Contact on the final frame by playback direction, in 512-cell source coordinates before mirroring. */
  wallContact?: Partial<Record<ChibiDirection, readonly [x: number, y: number]>>;
  /** Horizontal contact marks in 512-cell coordinates, starting from the first frame touching the wall. */
  wallContactFrames?: { from: number; left?: number[]; right?: number[] };
  /** Zero-based index of the first dock frame that requires the window to be at the edge. */
  dockedFrame?: number;
  cols: number;
  rows: number;
  frames: number;
  characterOverrides?: Record<string, Partial<Pick<ChibiAnimationSpec,
    'sheet' | 'directions' | 'mirrorLeft' | 'mirrorRight' | 'wallContact' | 'wallContactFrames' | 'dockedFrame' | 'cols' | 'rows' | 'frames' | 'durations'>>>;
  loop: boolean;
  /** 播完停在最后一帧，不自动回到 idle（如睡觉：躺平后保持入睡姿态）。 */
  hold?: boolean;
  durations: number[];
}

export type ChibiMotionSpec = ChibiPoseSpec | ChibiAnimationSpec;

export const ATLAS_COLS: number = vocab.atlas.cols;
export const ATLAS_ROWS: number = vocab.atlas.rows;

/**
 * 待机点缀：静息期间周期性插播的小动作。
 *
 * 后端的空闲阶段机只在 Active→Short→Medium→Long→Asleep 的跃迁上出手，天然做不出
 * 「每十几秒动一下」的节奏，也感知不到桌宠此刻是否被按住 / 拖动 / 正在漫步；这类点缀
 * 因此改由前端推进器消费（见 ChibiPetCanvas），与后端 `idle_triggers` 分开命名，
 * 免得一个键被两边各按各的语义解读。
 */
export interface IdleAccentSpec {
  motion: string;
  intervalSecs: number;
  probability: number;
  durationMs: number;
}

export const IDLE_ACCENT: IdleAccentSpec | null = (() => {
  const raw = (vocab as unknown as { idle_accent?: {
    motion?: string; interval_secs?: number; probability?: number; duration_ms?: number;
  } }).idle_accent;
  if (!raw?.motion) return null;
  return {
    motion: raw.motion,
    intervalSecs: Math.max(4, raw.interval_secs ?? 12),
    probability: Math.min(1, Math.max(0, raw.probability ?? 0.7)),
    durationMs: Math.max(400, raw.duration_ms ?? 2600),
  };
})();

const POSE_SPECS: ChibiPoseSpec[] = vocab.atlas.poses.map((pose, slot) => ({
  kind: 'pose' as const,
  name: pose.name,
  label: pose.label,
  category: pose.category,
  promptable: pose.promptable,
  slot,
}));

/** 格位按名索引：帧序列被角色门挡下时，用它回落到同名神态。 */
const POSE_BY_NAME = new Map<string, ChibiPoseSpec>(POSE_SPECS.map((spec) => [spec.name, spec]));

const ANIMATION_SPECS: ChibiAnimationSpec[] = vocab.animations.map((animation) => ({
  kind: 'animation' as const,
  name: animation.name,
  label: animation.label,
  category: animation.category,
  promptable: animation.promptable,
  characters: animation.characters ? [...animation.characters] : undefined,
  excludedCharacters: animation.excludedCharacters ? [...animation.excludedCharacters] : undefined,
  sheet: animation.sheet,
  directions: (animation.directions as ChibiDirection[] | undefined) ?? null,
  mirrorRight: 'mirrorRight' in animation && animation.mirrorRight === true,
  cols: animation.cols,
  rows: animation.rows,
  frames: animation.frames,
  characterOverrides: (animation as { characterOverrides?: ChibiAnimationSpec["characterOverrides"] }).characterOverrides,
  loop: animation.loop,
  hold: animation.hold,
  durations: [...animation.durations],
}));

/** 帧序列优先：同名的图集格位（如 happy）让位给表现更完整的帧序列。 */
const BY_NAME = new Map<string, ChibiMotionSpec>();
for (const spec of [...ANIMATION_SPECS, ...POSE_SPECS]) {
  if (!BY_NAME.has(spec.name)) BY_NAME.set(spec.name, spec);
}

const ALIASES: Record<string, string> = vocab.expression_aliases;

/**
 * 后端可能仍在发旧词汇（历史语义名或自由文本），此处做最后一道兜底。
 * 新词汇命中规则越少说明该收敛了，不要在这里继续加语义。
 */
function fuzzyResolve(raw: string): ChibiMotionSpec {
  const value = raw.toLowerCase();
  // 先判动作词：形如 walk_left / turn-right 的带方向名字必须先命中，
  // 否则会落到下面某个表情分支上
  if (/walk|move|pace|march|走/.test(value)) return BY_NAME.get('walk')!;
  if (/turn|rotate|spin|转身/.test(value)) return BY_NAME.get('turn')!;
  if (/blink|wink|眨眼/.test(value)) return BY_NAME.get('blink')!;
  if (/cast|summon|magic|施法|召唤/.test(value)) return BY_NAME.get('cast')!;
  if (/angry|annoy|furious|huff|生气|气鼓鼓|跺脚/.test(value)) return BY_NAME.get('angry')!;
  if (/smug|proud|scheming|得意|傲娇|叉腰/.test(value)) return BY_NAME.get('smug')!;
  if (/think|ponder|confus|wonder|思考|托腮|疑惑/.test(value)) return BY_NAME.get('think')!;
  if (/happy|joy|love|star|smile|开心|高兴/.test(value)) return BY_NAME.get('happy')!;
  // 只把「明确在晕」的信号归到 dizzy：dizzy/晕/blank(翻白眼)/tear(泪)/cry(哭)/sweat(冷汗)。
  // 早期这里还收了 sad/sleep/困，但这些词太常见（"困了""想睡""有点难过"常出现在心情文本里），
  // 一旦被自由文本命中就误挂晕眩脸——它们应回落到 idle 或走专门的情绪分支，不该统一当成眩晕。
  if (/dizzy|晕|blank|tear|cry|sweat/.test(value)) return BY_NAME.get('dizzy')!;
  if (/drag|dark|reach|grab|拎/.test(value)) return BY_NAME.get('drag')!;
  if (/talk|speak|mouth|说/.test(value)) return BY_NAME.get('talk')!;
  if (/listen|focus|look|听/.test(value)) return BY_NAME.get('listen')!;
  return BY_NAME.get('idle')!;
}

/** 解析动作名：精确名 → 别名表 → 旧词汇模糊兜底。永远有结果（兜底为 idle）。 */
export function resolveMotion(raw: string | undefined | null, character?: string): ChibiMotionSpec {
  const name = (raw ?? '').trim().toLowerCase();
  if (!name) return BY_NAME.get('idle')!;
  const direct = BY_NAME.get(name);
  const aliased = ALIASES[name];
  const spec = direct ?? (aliased ? BY_NAME.get(aliased) ?? fuzzyResolve(aliased) : fuzzyResolve(name));
  const resolved = forCharacter(spec, character);
  if (!resolved || (resolved === spec && character && spec.kind === 'animation' && spec.characters
    && !spec.characters.includes(character) && !characterAssetSource(fillTemplate(spec.sheet, character, 'left')))) {
    return POSE_BY_NAME.get(spec.name) ?? BY_NAME.get('idle')!;
  }
  return resolved;
}

function forCharacter(spec: ChibiMotionSpec, character?: string): ChibiMotionSpec | null {
  if (!character || spec.kind !== 'animation') return spec;
  if (spec.excludedCharacters?.includes(character)) return null;
  const packaged = characterAnimation(character, spec.name);
  if (packaged) return { ...packaged, kind: 'animation', name: spec.name };
  const override = spec.characterOverrides?.[character];
  return override ? { ...spec, ...override } : spec;
}

export function getMotion(name: string, character?: string): ChibiMotionSpec | null {
  const spec = BY_NAME.get(name);
  return spec ? forCharacter(spec, character) : null;
}

/**
 * 按名取帧序列规格。
 *
 * 调用方传入的都是词表里写死的名字，缺失只可能是 JSON 被改坏，故直接抛错而不是
 * 静默降级——静默降级会让动作悄悄播不出来，比构建期报错难查得多。
 */
export function animation(name: string, character?: string): ChibiAnimationSpec {
  const spec = getMotion(name, character);
  if (!spec || spec.kind !== 'animation') {
    throw new Error(`动作词汇表中不存在帧序列 "${name}"`);
  }
  return spec;
}

/** 按名取图集格位规格，缺失同样抛错。 */
export function pose(name: string): ChibiPoseSpec {
  const spec = BY_NAME.get(name);
  if (!spec || spec.kind !== 'pose') {
    throw new Error(`动作词汇表中不存在图集格位 "${name}"`);
  }
  return spec;
}

export function totalDurationMs(spec: ChibiAnimationSpec): number {
  return spec.durations.reduce((total, duration) => total + duration, 0);
}

/** 一段可播放的帧序列：帧序号与逐帧时长一一对应。 */
export interface ChibiPlayback {
  frames: number[];
  durations: number[];
}

/**
 * 倒放一段帧序列。
 *
 * 收场动作通常就是入场动作的倒放——图集里没有第二份「收起」素材，也不需要：
 * 倒着播天然从当前定格回到起始姿态，不会多出一份对不上的图。
 * 帧与帧时长**一起**倒序，得到的是原节奏的镜像；只倒帧不倒时长，会让慢帧配上快拍，
 * 看着像卡顿而不再像倒放。
 */
export function reversePlayback(spec: ChibiAnimationSpec): ChibiPlayback {
  return {
    frames: Array.from({ length: spec.frames }, (_, index) => index).reverse(),
    durations: [...spec.durations].reverse(),
  };
}

export function frameDurationMs(spec: ChibiAnimationSpec, index: number): number {
  return spec.durations[index] ?? spec.durations[spec.durations.length - 1] ?? 80;
}

/**
 * 填充路径模板里的全部占位符。
 *
 * 必须替换**所有**出现位置：`sheet` 模板里 `{character}` 在目录段与文件名段各出现
 * 一次（`/chibi/motion/{character}/{character}-blink-sheet.webp`），而
 * `String.prototype.replace(pattern, str)` 只替换首个匹配。曾用链式 replace 拼接，
 * 结果每个帧序列都请求到带字面量 `{character}` 的地址，全部 404 —— 背景图加载失败后
 * 精灵透明，桌宠在播放动作期间整个消失，且不报任何错。
 * 这里用 split/join 而非 replaceAll：tsconfig 的 target/lib 停在 ES2020。
 */
function fillTemplate(template: string, character: string, direction: ChibiDirection): string {
  return template.split('{character}').join(character).split('{direction}').join(direction);
}

/** 帧序列的图集地址；无方向的动作用默认方向占位（路径模板里没有 {direction}）。 */
export function sheetUrl(
  spec: ChibiAnimationSpec,
  character: string,
  direction: ChibiDirection,
): string {
  const source = fillTemplate(spec.sheet, character, direction);
  return optionalAssetSource(source) ?? '';
}

/**
 * 把「第 frame 帧」换算成图集定位，UI 侧无需再手算百分比。
 *
 * 图集格位只产出定位，背景图仍由 `.chibi-pet-sprite` 的 `--atlas-url` 提供；
 * 帧序列则一并给出图集地址，覆盖掉类上的背景图。
 */
export function frameStyle(
  spec: ChibiMotionSpec,
  frame: number,
  character: string,
  direction: ChibiDirection = 'left',
): {
  backgroundImage?: string;
  translate?: string;
  scale?: string;
  transformOrigin?: string;
  backgroundSize: string;
  backgroundPosition: string;
} {
  const cols = spec.kind === 'animation' ? spec.cols : ATLAS_COLS;
  const rows = spec.kind === 'animation' ? spec.rows : ATLAS_ROWS;
  const safeFrame = Math.max(0, frame);
  const col = safeFrame % cols;
  const row = Math.floor(safeFrame / cols) % rows;
  const x = cols > 1 ? (col * 100) / (cols - 1) : 0;
  const y = rows > 1 ? (row * 100) / (rows - 1) : 0;
  const style: {
    backgroundImage?: string;
    translate?: string;
    scale?: string;
    transformOrigin?: string;
    backgroundSize: string;
    backgroundPosition: string;
  } = {
    backgroundSize: `${cols * 100}% ${rows * 100}%`,
    backgroundPosition: `${x}% ${y}%`,
  };
  const mirroredRight = spec.kind === 'animation' && spec.mirrorRight && direction === 'right';
  const mirroredLeft = spec.kind === 'animation' && direction === 'left'
    && (spec.mirrorLeft ?? (spec.name === 'edge' || spec.name === 'lean'));
  const mirrored = mirroredRight || mirroredLeft;
  if (mirrored) style.scale = '-1 1';
  style.backgroundImage = `url('${spec.kind === 'animation' ? sheetUrl(spec, character, direction) : optionalAssetSource(`/chibi/${character}-atlas.webp`) ?? ''}')`;
  const path = spec.kind === 'animation' ? fillTemplate(spec.sheet, character, direction) : `/chibi/${character}-atlas.webp`;
  const anchor = (characterFrameData(path, 'anchors') ?? (frameAnchors as Record<string, number[][]>)[path])?.[safeFrame];
  // Independent translate preserves breathing/drag transforms and scales with the cell.
  if (anchor) style.translate = `${mirroredRight ? -anchor[0] : anchor[0]}% ${anchor[1]}%`;
  // Dock frames keep the default character scale and the optional reflection.
  if (spec.name !== 'edge' && spec.name !== 'edge-bottom') {
    const calibration = (characterFrameData(path, 'scales') ?? (frameScales as Record<string, number[][]>)[path])?.[safeFrame];
    if (calibration) {
      style.scale = mirrored ? `${-calibration[0]} ${calibration[0]}` : String(calibration[0]);
      style.transformOrigin = `50% ${calibration[1]}%`;
    }
  }
  return style;
}
