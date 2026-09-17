/**
 * 移动节奏规划：把一段窗口位移换算成「走不走、走几帧、每帧多久、总共多久」。
 *
 * 走动是逐帧图集，播放必须满足三条硬约束，对应「上下移动时走路过快」这条根因：
 *   1. **时长随距离缩放**（B）：窗口滑动时长 = 距离 ÷ 速度上限，限幅到可接受区间。
 *      不再有固定的 700ms——位移越远 / 挪得越快，时长越短，而不是帧率去追位移。
 *   2. **腿只表达水平速度**（D）：步数由「水平位移 |dx|」推，纵向挪动交给窗口滑动本身。
 *      此前步数按 hypot(dx,dy) 推，于是纵向为主的位移也会播一通与移动方向无关的碎步。
 *   3. **帧间隔限幅到图集固有节奏**（A，兜底）：帧间隔被夹在原生节奏的
 *      [0.75, 1.35]× 区间（≈58–105ms）。下沿防止「用放大帧率去追一个远超步行能力的
 *      位移」（超速碎步，根因①）；上沿防止读不出「腿在摆」。限幅命中时改**步数**
 *      （步幅变长）而不是改时长，所以走动收尾仍与窗口到位同时发生，腿不滑步。
 *
 * 推导链（顺序关键）：
 *   1. slideMs = 权威时间轴（B）：距离 ÷ 速度，再限幅到可接受区间；
 *   2. strideFrames = round( |dx| / PX_PER_FRAME ) —— 步数由水平位移推（D）；
 *   3. frameDelay = slideMs / strideFrames，再把 frameDelay 限幅到 [A_LO, A_HI]；
 *      限幅命中时按「保持时长不变」反推步数：frameDelay 偏小就少迈几帧（步幅变长），
 *      frameDelay 偏大就多迈几帧（步幅变短），时长始终 ≈ slideMs；
 *   4. durationMs = frames × frameDelay（≈ slideMs，限幅命中时最多差一个帧间隔）。
 *
 * 为什么 strideFrames 用 |dx| 而不是 hypot：腿是侧向的，一帧只表达水平前进；纵向位移
 * 由窗口滑动表达。若按全距离推步数，纵向为主的斜向挪动会让腿摆得比真实水平速度还快，
 * 看上去像「在往上飘的时候疯狂倒腾腿」——正是这次要修的语义错位（根因④）。
 *
 * 这套模型与上一个版本（步数按整周期分档、帧间隔 clamp 33–98ms）相比：33ms 下限在
 * 约 270px 以上全部命中、且固定 700ms 把时长绑死在帧率上，两条都废了。现在时长是权威
 * 值，帧率只在原生节奏带内浮动。
 *
 * ---------------------------------------------------------------------------
 * 两个消费方共用这套模型，只有「速度锚点」不同：
 *   - 智能避让 `planSmartMove` —— 尽快让开，速度上限 MAX_SLIDE_SPEED_PX_PER_MS；
 *   - 自主漫步 `planAmbientWalk` —— 闲逛不该冲出画面，速度锚在图集自己的步速上。
 * 共用一个 core 而不是各写一份，是因为「腿不滑步」这条不变式必须在两处都成立：
 * 此前漫步那段自己定时长、腿按原生节奏空转，走 100px 也摆满三个周期，
 * 于是既没有距离感（每次都像同一小步），腿也在地上打滑。
 */

import { animation, type ChibiDirection } from './motionRegistry';

/** 走满一个图集周期时，角色在屏幕上应有的水平前进距离（px）。 */
const CYCLE_TRAVEL_PX = 300;

const WALK = animation('walk');
const WALK_FRAMES = WALK.frames;

/**
 * 每前进这么多像素推进一格走动帧（步幅）。
 *
 * 锚点是图集自己的节奏：walk 一个周期 14 帧、共 1090ms，按原速播完恰好覆盖
 * CYCLE_TRAVEL_PX——也就是约 0.275 px/ms 的「不滑步」速度。
 */
const PX_PER_FRAME = CYCLE_TRAVEL_PX / WALK_FRAMES;

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
/** 位移时长上限（ms）：更快的挪动靠提高帧率表达，而不是继续拖时间（B）。 */
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
  /** 走动总帧数；walking 为 false 时无意义。种子来自 |dx|/PX_PER_FRAME，限幅可能微调。 */
  frames: number;
  /** 走动帧间隔（ms）；walking 为 false 时无意义。恒在 [A_LO, A_HI] 原生节奏带内。 */
  frameDelayMs: number;
  /** 走动总时长（ms）。恒在 ≈ slideMs 的一个帧间隔之内，与窗口滑动收尾同时发生。 */
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
function composeWalkPlan(dx: number, dy: number, slideMs: number): SmartMovePlan {
  const direction: ChibiDirection = dx <= 0 ? 'left' : 'right';
  const horizontal = Math.abs(dx);

  // 纵向为主的位移不播走动：腿是侧向的，表达不了纵向挪动（D，根因④）。
  if (horizontal < MIN_HORIZONTAL_PX) {
    return { walking: false, direction, frames: 0, frameDelayMs: 0, durationMs: slideMs };
  }

  // 步数由「水平位移」推：每前进 PX_PER_FRAME 推一格。纵向成分不计入，于是
  // 斜向挪动的腿只摆得和真实水平速度匹配，不会因纵向距离被放大（D）。
  let frames = Math.max(1, Math.round(horizontal / PX_PER_FRAME));

  // 先把权威时长按步数均分，得到无滑步的目标帧间隔（≈ 步幅 ÷ 窗口水平速度）；
  // 再把帧间隔限幅到原生节奏带 [A_LO, A_HI]（A）。
  let frameDelayMs = slideMs / frames;
  if (frameDelayMs < A_LO) {
    // 不能比原生还快（杜绝超速碎步）：少迈几帧，步幅变长，时长不变。
    frames = Math.max(1, Math.floor(slideMs / A_LO));
    frameDelayMs = slideMs / frames;
  } else if (frameDelayMs > A_HI) {
    // 不能太慢（否则读不出在走）：多迈几帧，步幅变短，时长不变。
    frames = Math.max(1, Math.ceil(slideMs / A_HI));
    frameDelayMs = slideMs / frames;
  }

  // 取整到整数毫秒；时长按整帧回写，与窗口滑动收尾同时发生（误差 ≤ 半个帧间隔）。
  frameDelayMs = Math.round(frameDelayMs);
  return {
    walking: true,
    direction,
    frames,
    frameDelayMs,
    durationMs: frames * frameDelayMs,
  };
}

/** 把一次窗口位移换算成「走不走、走几帧、每帧多久、总共多久」（智能避让用）。 */
export function planSmartMove(dx: number, dy: number): SmartMovePlan {
  // 窗口滑动时长：按距离与速度上限推，限幅到可接受区间。它是本次挪动的权威时间轴，
  // 后面的收敛只是把走动挂到这条时间轴上（B）。
  const slideMs = clamp(
    Math.hypot(dx, dy) / MAX_SLIDE_SPEED_PX_PER_MS,
    MIN_DURATION_MS,
    MAX_DURATION_MS,
  );
  return composeWalkPlan(dx, dy, slideMs);
}

/**
 * 把一次自主漫步换算成同样形状的计划。
 *
 * 与避让的差别只有速度锚点：避让的 0.6 px/ms 是「尽快到位」，而漫步锚在图集自己的
 * 0.275 px/ms 上——于是时长完全由距离决定（走多远就花多久），且由于帧间隔天然落在
 * 原生节奏上，不会被限幅改写，步幅与地面位移严格一一对应（腿不滑步）。
 *
 * `speedScale` 是每趟的步频微抖（见 AMBIENT_SPEED_JITTER）：默认每次调用抽一个
 * [1-jitter, 1+jitter] 的比例，需要确定性时（测试/预览）可以显式传入。
 */
export function planAmbientWalk(
  dx: number,
  speedScale: number = 1 + (Math.random() * 2 - 1) * AMBIENT_SPEED_JITTER,
): SmartMovePlan {
  const distance = Math.abs(dx);
  const speed = NATIVE_TRAVEL_PX_PER_MS * clamp(speedScale, 1 - AMBIENT_SPEED_JITTER, 1 + AMBIENT_SPEED_JITTER);
  return composeWalkPlan(dx, 0, distance / speed);
}
