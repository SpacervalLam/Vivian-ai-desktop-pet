/**
 * 自主巡游计划：把「当前可走区间 + 是否重力模式」换算成一趟行动意图。
 *
 * 重力模式下的桌宠此前只有一种行为——每隔 48–120 秒随机往某个方向挪一段。这个节奏
 * 放在会落地、会站住的桌面物理里读起来就是「一直站着不动」，靠墙/倚靠这类和边界相关
 * 的表演也基本排不上场。这里把一趟巡游拆成四种意图，让巡逻和靠墙成为正常节拍的一部分：
 *
 *   - `patrol`  沿当前支撑面走到可走区间的另一端（长距离巡逻）；
 *   - `edge`    走到最近的一侧边界，面朝边界倚靠一会儿，再回身；
 *   - `stroll`  随机方向的短途闲逛，到达后可能张望/得意；
 *   - `lookout` 原地四处张望，不做位移；
 *   - `interest` 走到本机图像分析标注出的兴趣点下方围观（图片多看一会儿，文字琢磨一下）。
 *
 * 计划只做几何与概率判断（可在 Node 里确定性测试），执行仍由舞台层
 * （`ChibiPetCanvas` 的漫步循环）负责：转身 → 走动 + 窗口滑动 → 收尾动作。
 */

export type RoamIntent = 'patrol' | 'stroll' | 'edge' | 'lookout' | 'interest';

/** 本地全屏图像分析标注出的一个兴趣点（物理屏幕坐标）。 */
export interface ScreenInterestPoint {
  x: number;
  y: number;
  width: number;
  height: number;
  score?: number;
  /** 后端标注的内容类型：`image`（彩色图片）或 `ui`（文字/控件）。 */
  kind?: string;
}

export interface RoamGesture {
  motion: string;
  durationMs: number;
}

export interface RoamPlan {
  intent: RoamIntent;
  /** 目标窗口 x（与 `x`/`minX`/`maxX` 同一坐标系）；`lookout` 时等于当前位置。 */
  targetX: number;
  /** 面向/位移方向；`lookout` 且不移动时只用于选择站姿朝向。 */
  direction: 'left' | 'right';
  /** 到达后是否贴着边界倚靠（保持面向墙）。 */
  edge: boolean;
  /** 到达（或原地）后播放的姿态；无则直接回身。 */
  gesture: RoamGesture | null;
}

export interface RoamContext {
  /** 当前窗口 x。 */
  x: number;
  /** 可走区间（窗口 x 上下界），已合并屏幕工作区边距与同伴占位。 */
  minX: number;
  maxX: number;
  /**
   * 可走区间两端是不是**屏幕边界**（而不是被同伴窗口截断的位置）。
   *
   * 缺省视为两端都是真墙，保持旧调用方的行为不变。靠墙倚靠只许发生在真墙上：
   * 隔壁桌宠的窗口边是透明的，走过去倚靠等于对着空气侧身。
   */
  walls?: { left: boolean; right: boolean };
  /** 是否重力模式。非重力模式保留原有的随机漫步节奏。 */
  gravity: boolean;
  /** 最近一次本机截图分析得到的兴趣点；空数组表示暂时没发现值得看的画面。 */
  interest?: ReadonlyArray<ScreenInterestPoint>;
  /** 可注入的随机源，测试/预览用。 */
  random?: () => number;
}

/** 一次闲逛的最短/最长距离（px）。 */
const STROLL_MIN_PX = 140;
const STROLL_MAX_PX = 700;
/** 非重力模式沿用原有的漫步距离区间。 */
const LEGACY_MAX_PX = 900;
/** 可走区间窄于此值时无法安全迈步，改为原地张望或贴边倚靠。 */
export const MIN_ROAM_TRAVEL_PX = 80;
const NARROW_LANE_PX = MIN_ROAM_TRAVEL_PX;
/** 抵达后的随机小动作池。 */
const GESTURES: ReadonlyArray<RoamGesture> = [
  { motion: 'look', durationMs: 2600 },
  { motion: 'smug', durationMs: 2400 },
  { motion: 'happy', durationMs: 2400 },
  { motion: 'think', durationMs: 2600 },
];

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const pickGesture = (random: () => number): RoamGesture =>
  GESTURES[Math.floor(random() * GESTURES.length)] ?? GESTURES[0];

/** 倚靠是持续状态：入场后停在末帧，20–120 秒后自行起身，交互可随时接管。 */
export const leanHoldMs = (random: () => number = Math.random) => Math.round(20_000 + random() * 100_000);

function facingTo(targetX: number, x: number, fallback: 'left' | 'right'): 'left' | 'right' {
  return targetX < x ? 'left' : targetX > x ? 'right' : fallback;
}

/** 目标落在边界上时，朝向由「这是哪一端」决定：左端面左、右端面右。 */
function towardBoundary(targetX: number, x: number, minX: number): 'left' | 'right' {
  if (targetX < x) return 'left';
  if (targetX > x) return 'right';
  return targetX === minX ? 'left' : 'right';
}

function lookout(x: number, random: () => number): RoamPlan {
  return {
    intent: 'lookout',
    targetX: Math.round(x),
    direction: random() < 0.5 ? 'left' : 'right',
    edge: false,
    gesture: { motion: 'look', durationMs: 2600 },
  };
}

/** 从标注结果里挑一个要去围观的兴趣点。 */
function chooseInterest(
  x: number,
  points: ReadonlyArray<ScreenInterestPoint>,
  random: () => number,
): ScreenInterestPoint {
  const ranked = [...points]
    .map(point => ({ point, distance: Math.abs(point.x + point.width / 2 - x) }))
    .sort((a, b) => a.distance - b.distance);
  // 优先「需要走过去」的点：就在脚下/头顶的点没有巡游价值。
  const informed = ranked.filter(entry => entry.distance >= STROLL_MIN_PX);
  const pool = informed.length ? informed : ranked;
  // 七成挑最近的一个（像「注意到最近的动静」），三成随机去远处看看。
  if (random() < 0.7 || pool.length === 1) return pool[0].point;
  return pool[Math.floor(random() * pool.length)].point;
}

function planInterest(
  points: ReadonlyArray<ScreenInterestPoint>,
  x: number,
  minX: number,
  maxX: number,
  random: () => number,
): RoamPlan {
  const point = chooseInterest(x, points, random);
  const center = point.x + point.width / 2;
  // 在标注格内随机取一个落脚点：每次围观站的位置略有不同，像绕着它看。
  const offset = (random() * 2 - 1) * (point.width / 3);
  const target = clamp(center + offset, minX, maxX);
  return {
    intent: 'interest',
    targetX: Math.round(target),
    direction: towardBoundary(target, x, minX),
    edge: false,
    // 彩色图片更容易让人（和桌宠）停下来多看一会儿；文字/控件则「琢磨一下」。
    gesture: point.kind === 'image'
      ? { motion: 'look', durationMs: 3200 }
      : { motion: 'think', durationMs: 3000 },
  };
}

export function planRoam(context: RoamContext): RoamPlan {
  const random = context.random ?? Math.random;
  const minX = context.minX;
  const maxX = Math.max(minX, context.maxX);
  const x = clamp(context.x, minX, maxX);
  const lane = maxX - minX;
  const walls = context.walls ?? { left: true, right: true };
  /** 这一端能不能倚靠：区间端点必须就是屏幕边界。 */
  const isWall = (target: number) => (target === minX ? walls.left : walls.right);
  /** 近侧的端点不是墙时，退到「走向另一端巡逻 / 原地张望」，绝不倚靠同伴窗口。 */
  const overPeerInstead = (blocked: number): RoamPlan => {
    const far = blocked === minX ? maxX : minX;
    if (isWall(far)) {
      return {
        intent: 'patrol',
        targetX: Math.round(far),
        direction: towardBoundary(far, x, minX),
        edge: false,
        gesture: null,
      };
    }
    return lookout(x, random);
  };

  if (lane < NARROW_LANE_PX) {
    // 站台太窄：没有安全的迈步空间。重力模式靠边倚靠，否则原地张望。
    if (!context.gravity) return lookout(x, random);
    const target = x - minX <= maxX - x ? minX : maxX;
    // 被同伴挤窄的站台没有墙可倚：倚靠只发生在屏幕边界上。
    if (!isWall(target)) return lookout(x, random);
    return {
      intent: 'edge',
      targetX: Math.round(target),
      direction: towardBoundary(target, x, minX),
      edge: true,
      gesture: { motion: 'lean', durationMs: leanHoldMs(random) },
    };
  }

  if (!context.gravity) {
    // 关闭重力时保持原有节奏：方向随机，距离在对数尺度上取样。
    let dir = random() < 0.5 ? -1 : 1;
    const distance = STROLL_MIN_PX * Math.pow(LEGACY_MAX_PX / STROLL_MIN_PX, random());
    let target = clamp(x + dir * distance, minX, maxX);
    if (Math.abs(target - x) < STROLL_MIN_PX) {
      dir = -dir;
      target = clamp(x + dir * distance, minX, maxX);
    }
    return {
      intent: 'stroll',
      targetX: Math.round(target),
      direction: facingTo(target, x, dir < 0 ? 'left' : 'right'),
      edge: false,
      gesture: null,
    };
  }

  const roll = random();
  const points = context.interest ?? [];
  // 有可围观内容时，「去兴趣点看看」排进正常节拍（约三成），其余意图按比例压缩。
  const interestEnd = points.length > 0 ? 0.30 : 0.0;
  if (points.length > 0 && roll < interestEnd) {
    return planInterest(points, x, minX, maxX, random);
  }
  const patrolEnd = interestEnd + (points.length > 0 ? 0.26 : 0.32);
  if (roll < patrolEnd) {
    // 巡逻：走向可走区间更远的一端，横穿屏幕更像在「巡场」。
    const target = x - minX >= maxX - x ? minX : maxX;
    return {
      intent: 'patrol',
      targetX: Math.round(target),
      direction: towardBoundary(target, x, minX),
      edge: false,
      gesture: random() < 0.5 ? pickGesture(random) : null,
    };
  }
  const edgeEnd = patrolEnd + (points.length > 0 ? 0.26 : 0.28);
  if (roll < edgeEnd) {
    // 靠墙：走到更近的一侧边界，面朝边界倚靠片刻。
    const target = x - minX <= maxX - x ? minX : maxX;
    // 这一侧被同伴窗口截断时没有墙可倚——别把隔壁桌宠的窗口边当成墙。
    if (!isWall(target)) return overPeerInstead(target);
    return {
      intent: 'edge',
      targetX: Math.round(target),
      direction: towardBoundary(target, x, minX),
      edge: true,
      gesture: { motion: 'lean', durationMs: leanHoldMs(random) },
    };
  }
  const strollEnd = edgeEnd + (points.length > 0 ? 0.14 : 0.25);
  if (roll < strollEnd) {
    // 闲逛：随机方向短途走动，到达后偶尔停下做个小动作。
    let dir = random() < 0.5 ? -1 : 1;
    const distance = STROLL_MIN_PX + random() * (STROLL_MAX_PX - STROLL_MIN_PX);
    let target = clamp(x + dir * distance, minX, maxX);
    if (Math.abs(target - x) < STROLL_MIN_PX) {
      dir = -dir;
      target = clamp(x + dir * distance, minX, maxX);
    }
    return {
      intent: 'stroll',
      targetX: Math.round(target),
      direction: facingTo(target, x, dir < 0 ? 'left' : 'right'),
      edge: false,
      gesture: random() < 0.45 ? pickGesture(random) : null,
    };
  }
  return lookout(x, random);
}
