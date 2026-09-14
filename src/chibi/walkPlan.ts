/**
 * 智能避让的移动节奏规划。
 *
 * 走动是逐帧图集，播放速度必须落在人眼读得出「腿在摆」的区间里。此前的算法把位移
 * 时长固定成 700ms、帧数正比于距离，帧间隔 = 时长 / 帧数 就反比于距离——屏幕纵向
 * 挪动轻易就是数百像素，帧间隔被压到几毫秒，走动退化成闪烁抖动；固定时长还意味着
 * 瞬时速度随距离线性膨胀，同一只角色在远近两次挪动中的步速能差一个数量级。
 *
 * 这里改成三段式推导，让帧率与速度各自有界，且互不放大：
 *   1. 步数由距离分档——每 CYCLE_TRAVEL_PX 走满一个图集周期；
 *   2. 总时长由距离与滑动速度上限推出，限制单次挪动的耗时；
 *   3. 帧间隔由「在总时长里播完这些帧」反推，再限幅到可辨识区间。
 * 算完把时长按帧时间轴回写一次，让走动收尾与窗口到位同时发生。
 *
 * 腿是侧向的：纵向位移无法由摆动表达，因此水平分量过小时不播走动，交给窗口滑动。
 */

import { animation, type ChibiDirection } from './motionRegistry';

/** 走满一个图集周期时，角色在屏幕上应有的水平前进距离（px）。 */
const CYCLE_TRAVEL_PX = 300;
/** 帧间隔下限 ≈ 30fps。再快只剩抖动，读不出摆腿。 */
const MIN_FRAME_DELAY_MS = 33;
/** 帧间隔上限：比图集固有节奏再慢一档，再慢就拖沓了。 */
const MAX_FRAME_DELAY_MS = 98;
/** 窗口滑动速度上限（px/ms）：横跨屏幕的挪动也要在约一秒内到位。 */
const MAX_SLIDE_SPEED_PX_PER_MS = 0.6;
const MIN_DURATION_MS = 450;
/** 只约束「按距离推时间」这一步；走动时以帧时间轴为准，可能更长，见 planSmartMove。 */
const MAX_DURATION_MS = 1_000;
/** 水平位移低于此值时不播走动——腿的摆动表达不了这次挪动。 */
const MIN_HORIZONTAL_PX = 40;

const WALK_FRAMES = animation('walk').frames;

export interface SmartMovePlan {
  /** 本次挪动是否播放走动动画。纵向为主的位移不播。 */
  walking: boolean;
  direction: ChibiDirection;
  /** 走动总帧数，恒为图集周期的整数倍，播满即停在第一格。 */
  frames: number;
  /** 走动帧间隔（ms）；walking 为 false 时无意义。 */
  frameDelayMs: number;
  /** 窗口滑动总时长（ms），走动时与帧时间轴严格一致。 */
  durationMs: number;
}

/** 把一次窗口位移换算成「走不走、走几帧、每帧多久、总共多久」。 */
export function planSmartMove(dx: number, dy: number): SmartMovePlan {
  const direction: ChibiDirection = dx <= 0 ? 'left' : 'right';
  const horizontal = Math.abs(dx);
  const travel = Math.hypot(dx, dy);
  const slideMs = Math.min(
    MAX_DURATION_MS,
    Math.max(MIN_DURATION_MS, travel / MAX_SLIDE_SPEED_PX_PER_MS),
  );

  if (horizontal < MIN_HORIZONTAL_PX) {
    return { walking: false, direction, frames: 0, frameDelayMs: 0, durationMs: slideMs };
  }

  // 步数按水平距离分档：走得越远，摆腿的周期数越多，而不是把同样的步数播得更快。
  // 向上取整而非就近取整——就近取整会让 400px 比 300px 更"打滑"（同样是走一个周期，
  // 却要在同样的时间里多挪一段），距离与周期数就不再单调。
  const cycles = Math.max(1, Math.ceil(horizontal / CYCLE_TRAVEL_PX));
  const frames = cycles * WALK_FRAMES;
  // 取整：帧间隔会一路传给定时器，整数才能让「帧数 × 帧间隔」与窗口滑动时长严格相等。
  const frameDelayMs = Math.round(Math.min(
    MAX_FRAME_DELAY_MS,
    Math.max(MIN_FRAME_DELAY_MS, slideMs / frames),
  ));

  return {
    walking: true,
    direction,
    frames,
    frameDelayMs,
    // 以帧时间轴回写时长，让走动收尾和窗口到位同时发生。帧间隔有下限，所以帧数多时
    // 这里的时长会超过 MAX_DURATION_MS——宁可让横跨屏幕的挪动多花一点时间，
    // 也不要把帧率拉回抖动区间。
    durationMs: Math.round(frames * frameDelayMs),
  };
}
