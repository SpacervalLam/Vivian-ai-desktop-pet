/**
 * 房间热点定义——每个热点是一个"有意义的位置 + 朝向"，
 * 角色站在这里代表在做某事（坐书桌、躺床、面朝窗外等）。
 *
 * 数据直接来自 dormLayout.json 的 hotspots 字段，避免两处维护。
 */

import layoutData from '../dormLayout.json';

export type Hotspot = {
  id: string;
  pos: [number, number, number];
  facing: number; // Y 轴旋转弧度，0 = 沿 +Z 看，π = 沿 -Z，±π/2 = ∓X
};

export const HOTSPOTS: Hotspot[] = (layoutData as any).hotspots ?? [];

export function findHotspot(id: string): Hotspot | undefined {
  return HOTSPOTS.find(h => h.id === id);
}
