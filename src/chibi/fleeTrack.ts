import { planFlee, type FleeGeometry } from './fleePlan';

/**
 * 逃离的滑行速度（px/ms）。
 *
 * 比智能避让的 0.6 快一倍：避让是「让开」，逃离是「窜出去」。整段位移 0.3~0.7s，
 * 一口气跑完；慢吞吞地挪过去读起来不像害怕，像散步。
 */
const FLEE_SPEED_PX_PER_MS = 1.2;
/** 时长下限（ms）：再近的落点也要有这么多帧定位，否则看上去是瞬移。 */
const FLEE_MIN_DURATION_MS = 280;
/** 时长上限（ms）：再远也别拖——逃离是冲刺，不是长途。 */
const FLEE_MAX_DURATION_MS = 900;

/**
 * 「按住多久才算把窗口抢回来」（ms）。
 *
 * 逃离的中止条件与「拖动会话要不要延后开启」共用这一个数，两侧各写一份必然漂移
 * ——一侧放宽、另一侧还收紧，窗口就会在半路被拽一下。
 */
export const FLEE_TAKEOVER_HOLD_MS = 250;

/**
 * 一次逃离要滑多久：距离 ÷ 逃离速度，再限幅。
 *
 * 刻意不挂走动节奏（`planSmartMove` 那套步数/帧间隔）：这一趟没有腿，帧率与步幅都
 * 没有承载者，只有「多久到位」有意义。
 */
function fleeDurationMs(distancePx: number): number {
  return Math.min(
    FLEE_MAX_DURATION_MS,
    Math.max(FLEE_MIN_DURATION_MS, distancePx / FLEE_SPEED_PX_PER_MS),
  );
}

/** 环境侧：只有真机才有的东西（窗口几何、滑动、计时）。 */
export interface FleeEnv {
  /** 读窗口与显示器几何；拿不到（非 Tauri 环境、取不到显示器）时返回 null。 */
  readGeometry: () => Promise<FleeGeometry | null>;
  /** 把窗口从 from 滑到 to；返回是否走完全程（被中止时窗口停在半途）。 */
  slide: (
    from: { x: number; y: number },
    to: { x: number; y: number },
    durationMs: number,
    shouldAbort: () => boolean,
  ) => Promise<boolean>;
  wait: (durationMs: number) => Promise<void>;
}

export interface FleeOptions {
  minDistancePx: number;
  maxDistancePx: number;
  /**
   * 生气脸先亮多久再起步（ms）。
   *
   * 这不是延迟——生气脸是调用方**当场**切上去的，这一拍只是让它被看见：窗口一动起来，
   * 视线就跟到位移上去了，先瞪一眼再窜出去，读起来是「有情绪」而不是「被弹开」。
   */
  angerBeatMs: number;
  /**
   * 中止判据：窗口被用户按住够久（抢回）、或组件已卸载 → 当帧停下。
   *
   * 用「按住」而非「手在窗口上」：连点时手一直停在窗口上，而本条件每 32ms 采样一次、
   * 真手按一下约 60~120ms——若按「手在不在」判定，位移会在第一帧就被掐死。
   */
  shouldAbort?: () => boolean;
  random?: () => number;
}

export type FleeOutcome =
  /** 滑到了落点。 */
  | 'fled'
  /** 横向放不下一个「远离」的落点 → 只生气，不挪窝。 */
  | 'no-room'
  /** 拿不到窗口几何，或亮脸/滑动途中被用户按住抢走。 */
  | 'interrupted';

export async function runFlee(env: FleeEnv, options: FleeOptions): Promise<FleeOutcome> {
  const geometry = await env.readGeometry();
  if (!geometry) return 'interrupted';
  if (options.shouldAbort?.()) return 'interrupted';

  const plan = planFlee(geometry, options);
  if (!plan) return 'no-room';

  // 生气脸此刻已经在脸上（调用方切好了），这一拍只是让它被看见。
  await env.wait(options.angerBeatMs);
  if (options.shouldAbort?.()) return 'interrupted';

  // 起步前重读一次位置：别人的位移（自主漫步 / 智能避让）可能刚被我们按停、窗口停在
  // 半途，按旧位置起滑会先往回跳一下。
  const from = (await env.readGeometry()) ?? geometry;
  const distance = Math.hypot(plan.targetX - from.fromX, plan.targetY - from.fromY);

  const slid = await env.slide(
    { x: from.fromX, y: from.fromY },
    { x: plan.targetX, y: plan.targetY },
    fleeDurationMs(distance),
    () => options.shouldAbort?.() ?? false,
  );
  return slid ? 'fled' : 'interrupted';
}
