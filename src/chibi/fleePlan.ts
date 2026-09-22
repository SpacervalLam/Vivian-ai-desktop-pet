/**
 * 逃离落点的几何：把「桌宠当前在哪 + 显示器多大」换算成一个「足够远」的随机落点。
 *
 * 单独成模块，是因为这里是纯算术、也是最容易出边界 bug 的地方：桌宠贴着屏幕边、
 * 屏幕比桌宠还窄、多屏左/上方那块屏的显示器坐标是负的——这些在真机上很难复现，
 * 但都能用一组数字直接算出来验证。
 *
 * 只算横向落点：走动图集是侧向的，纵向位移没有对应的腿（见 walkPlan 的 D 条约束），
 * 所以「跑开」只发生在水平方向上，纵向保持不变。
 */

import type { ChibiDirection } from './motionRegistry';

export interface FleeGeometry {
  /** 桌宠窗口左上角（物理像素，与显示器同一坐标系）。 */
  fromX: number;
  fromY: number;
  /** 窗口宽度：可站立的横向范围要先扣掉它。 */
  windowWidth: number;
  /** 当前显示器左上角与宽度。 */
  monitorX: number;
  monitorWidth: number;
  /** 与屏幕边缘保持的间隙：贴着边站会显得被裁掉一半。 */
  marginPx: number;
}

export interface FleePlanOptions {
  /** 「远离」的下限（px）：低于它的落点只是原地蹭一下，会被读成「没反应」。 */
  minDistancePx: number;
  /** 单趟逃离的距离上限（px）：逃离是一口气的冲刺，不是横跨整个桌面。 */
  maxDistancePx: number;
  /** 取样函数：默认 Math.random，测试里注入确定性随机。 */
  random?: () => number;
}

export interface FleePlan {
  targetX: number;
  targetY: number;
  /** 实际跑出的距离（按截断后的落点重算，不是抽签时的名义值）。 */
  distancePx: number;
  direction: ChibiDirection;
}

/**
 * 抽一个落点。返回 null 表示横向根本没有可去的空间（屏幕比桌宠还窄、桌宠已在边界外），
 * 此时调用方只生气、不挪窝。
 *
 * 落点先选边、再在「[下限, 上限] 与这一侧余量的交集」上均匀取样——不按两侧余量加权，
 * 是因为「往哪边跑」和「跑多远」该是两件独立的事：加权会让贴着右边站的桌宠几乎永远
 * 往左跑，用户看到的是「总是同一个方向」。
 */
export function planFlee(geometry: FleeGeometry, options: FleePlanOptions): FleePlan | null {
  const random = options.random ?? Math.random;
  const minX = geometry.monitorX + geometry.marginPx;
  const maxX =
    geometry.monitorX + geometry.monitorWidth - geometry.windowWidth - geometry.marginPx;
  // 横向连站的地方都没有：硬挪只会把桌宠推出屏幕
  if (maxX <= minX) return null;

  const roomLeft = geometry.fromX - minX;
  const roomRight = maxX - geometry.fromX;

  const sides: ChibiDirection[] = [];
  if (roomLeft >= options.minDistancePx) sides.push('left');
  if (roomRight >= options.minDistancePx) sides.push('right');

  let direction: ChibiDirection;
  let distance: number;
  if (sides.length > 0) {
    direction = sides.length === 1 ? sides[0] : random() < 0.5 ? 'left' : 'right';
    const room = direction === 'left' ? roomLeft : roomRight;
    const reach = Math.min(room, Math.max(options.maxDistancePx, options.minDistancePx));
    distance = options.minDistancePx + random() * (reach - options.minDistancePx);
  } else {
    // 两侧都凑不满「远离」的下限（窄屏、贴边）：跑向更宽的那一侧边缘，有多少跑多少。
    // 总比站在原地生气强——用户至少能读出「它躲开了」。
    direction = roomRight >= roomLeft ? 'right' : 'left';
    distance = Math.max(roomLeft, roomRight);
    if (distance <= 0) return null;
  }

  const signed = direction === 'left' ? -distance : distance;
  const targetX = Math.round(Math.min(maxX, Math.max(minX, geometry.fromX + signed)));
  return {
    targetX,
    targetY: geometry.fromY,
    distancePx: Math.abs(targetX - geometry.fromX),
    direction,
  };
}
