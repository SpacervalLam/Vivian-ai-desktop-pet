/**
 * Move the window and feet on the same clock. Horizontal travel selects whole
 * gait cycles, and near-vertical movement uses hovering instead. Completing a
 * cycle returns to the shared turn/walk stance before turning back to idle.
 * Cadence remains within the authored walking range; very short walks take at
 * least one cycle rather than cutting a raised leg directly to a standing pose.
 */

import { animation, type ChibiDirection } from './motionRegistry';

/** 走满一个图集周期时，角色在屏幕上应有的水平前进距离（px）。 */
// Two short chibi steps, rather than a full character-width glide per cycle.
const CYCLE_TRAVEL_PX = 140;

/** Nana's right-facing sheet uses measured, low-lift steps. */
export function walkCycleTravelPx(character: string | null | undefined, direction: ChibiDirection): number {
  return character === 'nana' && direction === 'right' ? 100 : CYCLE_TRAVEL_PX;
}

const WALK = animation('walk');
const WALK_FRAMES = WALK.frames;

/**
 * 图集 walk 的固有平均帧间隔（ms）：14 帧时长之和 ÷ 14。
 * 所有「原生节奏」的判定都以它为基准，不从别处抄常数。
 */
const NATIVE_FRAME_MS = WALK.durations.reduce((sum, d) => sum + d, 0) / WALK_FRAMES;

/**
 * 帧间隔安全带（A 的兜底区间），围绕原生节奏对称展开：
 *   A_LO = 0.75 × 原生 ≈ 58ms（再快就是超速碎步，根因①）
 *   A_HI = 1.35 × 原生 ≈ 105ms（再慢就读不出在走）
 */
const A_LO = Math.round(0.75 * NATIVE_FRAME_MS);
const A_HI = Math.round(1.35 * NATIVE_FRAME_MS);

/**
 * 图集自己的地面速度（px/ms）：走满一个周期前进 CYCLE_TRAVEL_PX、耗时整整一个周期。
 * 「腿摆一格、地面就该挪多少」的标准答案，自主漫步直接锚在它上面。
 */
const NATIVE_TRAVEL_PX_PER_MS = CYCLE_TRAVEL_PX / WALK.durations.reduce((sum, d) => sum + d, 0);

/** 窗口滑动速度上限（px/ms）：横跨屏幕的挪动也要在约一秒内到位（B）。 */
const MAX_SLIDE_SPEED_PX_PER_MS = 0.6;
/** 位移时长下限（ms）：太短的挪动也得让人看清起步与收尾（B）。 */
const MIN_DURATION_MS = 400;
/** 悬浮滑动的时长上限；走路还需按步幅延长，防止长距离滑步。 */
const MAX_DURATION_MS = 1_400;
/** 水平位移低于此值时不播走动——腿的摆动表达不了这次挪动（D：纵向不走路）。 */
const MIN_HORIZONTAL_PX = 40;

/**
 * 自主漫步的步速抖动幅度（相对原生步速）。
 *
 * 每一趟换一个略快/略慢的步频：固定步频会让长距离漫步变成节拍器，越远越明显。
 * 抖动幅度压在原生节奏安全带 [A_LO, A_HI] 以内，所以抖动不会触发限幅——
 * 帧间隔始终是「原生节奏本身」，只是每趟的原生节奏略有不同。
 */
export const AMBIENT_SPEED_JITTER = 0.15;

export interface SmartMovePlan {
  /** 本次挪动是否播放走动动画。纵向为主的位移不播。 */
  walking: boolean;
  direction: ChibiDirection;
  /** Whole gait cycles; unused when walking is false. */
  frames: number;
  /** 走动帧间隔（ms）；walking 为 false 时无意义。恒在 [A_LO, A_HI] 原生节奏带内。 */
  frameDelayMs: number;
  /** Shared duration for both window movement and walking frames. */
  durationMs: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * 把「位移 + 权威时长」收敛成一份自洽的计划。
 *
 * 两个入口的差别只在 slideMs 怎么来（避让按速度上限推、漫步按原生步速推），
 * 从这一步往后完全一样——步数、帧间隔、总时长的收敛规则只写在这里一份。
 */
function composeWalkPlan(dx: number, _dy: number, slideMs: number, cycleTravelPx = CYCLE_TRAVEL_PX): SmartMovePlan {
  const direction: ChibiDirection = dx <= 0 ? 'left' : 'right';
  const horizontal = Math.abs(dx);

  // 纵向为主的位移不播走动：腿是侧向的，表达不了纵向挪动（D，根因④）。
  if (horizontal < MIN_HORIZONTAL_PX) {
    return { walking: false, direction, frames: 0, frameDelayMs: 0, durationMs: slideMs };
  }

  // Finish a whole gait cycle at the turn's shared side-facing stance. A short
  // move may take one complete step rather than cutting a raised leg straight
  // to the turn sheet. Both the window and feet use the adjusted duration.
  const minCycles = Math.max(1, Math.ceil(slideMs / (WALK_FRAMES * A_HI)));
  const maxCycles = Math.max(minCycles, Math.floor(slideMs / (WALK_FRAMES * A_LO)));
  const cycles = clamp(Math.round(horizontal / cycleTravelPx), minCycles, maxCycles);
  const frames = cycles * WALK_FRAMES;
  const frameDelayMs = clamp(Math.round(slideMs / frames), A_LO, A_HI);
  return {
    walking: true,
    direction,
    frames,
    frameDelayMs,
    durationMs: frames * frameDelayMs,
  };
}

/** 把一次窗口位移换算成「走不走、走几帧、每帧多久、总共多久」（智能避让用）。 */
export function planSmartMove(dx: number, dy: number, cycleTravelPx = CYCLE_TRAVEL_PX): SmartMovePlan {
  // 先计算悬浮滑动时长；水平走路还要满足步幅与步频约束。
  const slideMs = clamp(
    Math.hypot(dx, dy) / MAX_SLIDE_SPEED_PX_PER_MS,
    MIN_DURATION_MS,
    MAX_DURATION_MS,
  );
  // A long avoidance move must not squeeze a single gait cycle into the slide
  // time cap. Allow a brisk cadence while keeping travel proportional to steps.
  const walkingMs = Math.abs(dx) / (NATIVE_TRAVEL_PX_PER_MS * cycleTravelPx / CYCLE_TRAVEL_PX * 1.25);
  return composeWalkPlan(dx, dy, Math.max(slideMs, walkingMs), cycleTravelPx);
}

/**
 * 把一次自主漫步换算成同样形状的计划。
 *
 * 与避让的差别只有速度锚点：避让的 0.6 px/ms 是「尽快到位」，而漫步锚在图集自己的
 * 约 0.128 px/ms 上——于是时长由距离决定（走多远就花多久），且由于帧间隔天然落在
 * 原生节奏上，不会被限幅改写，步幅与地面位移严格一一对应（腿不滑步）。
 *
 * `speedScale` 是每趟的步频微抖（见 AMBIENT_SPEED_JITTER）：默认每次调用抽一个
 * [1-jitter, 1+jitter] 的比例，需要确定性时（测试/预览）可以显式传入。
 */
export function planAmbientWalk(
  dx: number,
  speedScale: number = 1 + (Math.random() * 2 - 1) * AMBIENT_SPEED_JITTER,
  cycleTravelPx: number = CYCLE_TRAVEL_PX,
): SmartMovePlan {
  const distance = Math.abs(dx);
  const speed = NATIVE_TRAVEL_PX_PER_MS * cycleTravelPx / CYCLE_TRAVEL_PX * clamp(speedScale, 1 - AMBIENT_SPEED_JITTER, 1 + AMBIENT_SPEED_JITTER);
  return composeWalkPlan(dx, 0, distance / speed, cycleTravelPx);
}
