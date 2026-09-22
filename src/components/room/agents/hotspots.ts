/**
 * 房间热点定义——每个热点是一个"有意义的位置 + 朝向"，
 * 角色站在这里代表在做某事（坐书桌、躺床、面朝窗外等）。
 *
 * 数据直接来自 dormLayout.json 的 hotspots 字段，避免两处维护。
 *
 * 坐类热点（animation:'sit'）拆成两个坐标：
 *   pos  = 站立落点，必须落在可走格上（A* 的目标）；
 *   seat = 坐定后的落点，允许落在家具里（沙发坐垫上），并带 y 抬升。
 * 之所以拆开：导航栅格把家具整块挖掉了，直接把坐点当 A* 目标会被判成不可达；
 * 而把"站在沙发边"当坐点，角色就会在家具背后播坐下动画（沙发没定位准的那个 bug）。
 */

import layoutData from '../dormLayout.json';
import { WORLD_HOTSPOTS } from './worldHotspots';

export type Hotspot = {
  id: string;
  animation?: 'sit';
  pos: [number, number, number];
  facing: number; // Y 轴旋转弧度，0 = 沿 +Z 看，π = 沿 -Z，+π/2 = 沿 +X
  /** 所在导航层（navWorld 的 AREAS）。室内热点省略即 'apt2'。 */
  layer?: string;
  /** 坐/躺的落点。只有 animation:'sit' 的热点需要。 */
  seat?: {
    /** 坐面所在家具 id，仅用于验收哨兵核对落点确实压在该家具坐面上。 */
    on: string;
    /** 坐定后根节点位置；y = 相对地面的抬升量（把坐姿的坐面垫到家具坐面高度）。 */
    pos: [number, number, number];
    /** 坐定后朝向；省略则沿用站立朝向。 */
    facing?: number;
    /** 家具坐面高度（米），哨兵用它核对抬升量。 */
    surfaceY: number;
  };
};

// 室内热点省略 layer ⇒ 视为 2F 那一层（203 与 2F 公共区同标高，但导航上是两张栅格）
const INDOOR: Hotspot[] = ((layoutData as any).hotspots ?? []).map((h: Hotspot) => ({ ...h, layer: h.layer ?? 'apt2' }));

/** 室内 + 公寓公共区 + 街区店铺，寻路与选点都只看这一份。 */
export const HOTSPOTS: Hotspot[] = [...INDOOR, ...(WORLD_HOTSPOTS as Hotspot[])];

export function findHotspot(id: string): Hotspot | undefined {
  return HOTSPOTS.find(h => h.id === id);
}
