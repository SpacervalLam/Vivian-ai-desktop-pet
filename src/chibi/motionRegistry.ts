/**
 * Q 版桌宠动作词汇表。
 *
 * `animations.json` 是动作的唯一真源：图集分格、每帧时长、循环语义、情绪映射与
 * 可供 LLM 选择的动作名都在那里声明。本模块把它归一化成渲染器可直接消费的结构，
 * 新增或调整动作只需要改 JSON，不必再同步散落各处的常量。
 *
 * 同一个词表也被后端读取（构建期嵌入），保证 prompt 里列出的动作名与前端能播的
 * 动作完全一致——曾经两边的词汇是各自硬编码的，后端列动作、前端只有
 * Q 版图集，导致动作永远选不中。
 */

import vocab from './animations.json';

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
  /** 图集路径模板，`{character}` 与 `{direction}` 由调用方填充。 */
  sheet: string;
  /** 有方向的动作为 ['left','right']，无方向为 null。 */
  directions: ChibiDirection[] | null;
  cols: number;
  rows: number;
  frames: number;
  loop: boolean;
  durations: number[];
}

export type ChibiMotionSpec = ChibiPoseSpec | ChibiAnimationSpec;

export const ATLAS_COLS: number = vocab.atlas.cols;
export const ATLAS_ROWS: number = vocab.atlas.rows;

const POSE_SPECS: ChibiPoseSpec[] = vocab.atlas.poses.map((pose, slot) => ({
  kind: 'pose' as const,
  name: pose.name,
  label: pose.label,
  category: pose.category,
  promptable: pose.promptable,
  slot,
}));

const ANIMATION_SPECS: ChibiAnimationSpec[] = vocab.animations.map((animation) => ({
  kind: 'animation' as const,
  name: animation.name,
  label: animation.label,
  category: animation.category,
  promptable: animation.promptable,
  sheet: animation.sheet,
  directions: (animation.directions as ChibiDirection[] | undefined) ?? null,
  cols: animation.cols,
  rows: animation.rows,
  frames: animation.frames,
  loop: animation.loop,
  durations: [...animation.durations],
}));

/** 帧序列优先：同名的图集格位（如 happy）让位给表现更完整的帧序列。 */
const BY_NAME = new Map<string, ChibiMotionSpec>();
for (const spec of [...ANIMATION_SPECS, ...POSE_SPECS]) {
  if (!BY_NAME.has(spec.name)) BY_NAME.set(spec.name, spec);
}

const ALIASES: Record<string, string> = vocab.expression_aliases;

/** 面向前端的动作名清单（含别名），供调试面板与验收脚本使用。 */
export const ANIMATION_NAMES: string[] = ANIMATION_SPECS.map((spec) => spec.name);
export const POSE_NAMES: string[] = POSE_SPECS.map((spec) => spec.name);
export const PROMPTABLE_NAMES: string[] = [...ANIMATION_SPECS, ...POSE_SPECS]
  .filter((spec) => spec.promptable)
  .map((spec) => spec.name);
export const DIRECTIONAL_NAMES: string[] = ANIMATION_SPECS
  .filter((spec) => spec.directions !== null)
  .map((spec) => spec.name);

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
  if (/dizzy|sweat|blank|tear|sad|cry|sleep|晕|困/.test(value)) return BY_NAME.get('dizzy')!;
  if (/drag|dark|reach|grab|拎/.test(value)) return BY_NAME.get('drag')!;
  if (/talk|speak|mouth|说/.test(value)) return BY_NAME.get('talk')!;
  if (/listen|focus|look|听/.test(value)) return BY_NAME.get('listen')!;
  return BY_NAME.get('idle')!;
}

/** 解析动作名：精确名 → 别名表 → 旧词汇模糊兜底。永远有结果（兜底为 idle）。 */
export function resolveMotion(raw: string | undefined | null): ChibiMotionSpec {
  const name = (raw ?? '').trim().toLowerCase();
  if (!name) return BY_NAME.get('idle')!;
  const direct = BY_NAME.get(name);
  if (direct) return direct;
  const aliased = ALIASES[name];
  if (aliased) return BY_NAME.get(aliased) ?? fuzzyResolve(aliased);
  return fuzzyResolve(name);
}

export function getMotion(name: string): ChibiMotionSpec | null {
  return BY_NAME.get(name) ?? null;
}

/**
 * 按名取帧序列规格。
 *
 * 调用方传入的都是词表里写死的名字，缺失只可能是 JSON 被改坏，故直接抛错而不是
 * 静默降级——静默降级会让动作悄悄播不出来，比构建期报错难查得多。
 */
export function animation(name: string): ChibiAnimationSpec {
  const spec = BY_NAME.get(name);
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

/** 该动作是否需要方向（决定调用方是否要记录 left/right）。 */
export function isDirectional(spec: ChibiMotionSpec): spec is ChibiAnimationSpec {
  return spec.kind === 'animation' && spec.directions !== null;
}

export function totalDurationMs(spec: ChibiAnimationSpec): number {
  return spec.durations.reduce((total, duration) => total + duration, 0);
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
  return fillTemplate(spec.sheet, character, direction);
}

/**
 * 主状态图集地址。
 *
 * 与 `ChibiPetCanvas.css` 的 `--atlas-url` 必须指向同一份文件：这里是 JS 侧的
 * 唯一出处，供预取与「帧序列图集加载失败」时的回落使用。
 */
export function atlasUrl(character: string): string {
  return `/chibi/${character}-atlas.webp`;
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
    backgroundSize: string;
    backgroundPosition: string;
  } = {
    backgroundSize: `${cols * 100}% ${rows * 100}%`,
    backgroundPosition: `${x}% ${y}%`,
  };
  if (spec.kind === 'animation') {
    style.backgroundImage = `url('${sheetUrl(spec, character, direction)}')`;
  }
  return style;
}

/**
 * 动作运行时实际需要的图集地址清单（用于首帧前预取）。
 *
 * 有方向的动作用到几张图就预取几张：只预取 left 的话，环境走路的第一次「向右走」
 * 会在图集下载完成前先渲染一帧空背景，看起来像桌宠闪了一下。
 */
export function prefetchUrls(character: string): string[] {
  const urls = new Set<string>([atlasUrl(character)]);
  for (const spec of ANIMATION_SPECS) {
    for (const direction of spec.directions ?? (['left'] as ChibiDirection[])) {
      urls.add(sheetUrl(spec, character, direction));
    }
  }
  return [...urls];
}
