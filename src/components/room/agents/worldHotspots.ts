/**
 * 203 之外的热点：公寓公共区（2F 外廊 / 电梯厅 / 1F 大堂）与街区店铺。
 *
 * 和室内热点分开维护：室内那份是 `dormLayout.json` 的一部分（它同时是户型、
 * 家具、灯光的单一真相），室外这些只跟场景几何有关，混进户型文件反而难找。
 *
 * **必须带 `layer`**：同一个 (x,z) 在不同楼层都能站（电梯厅、楼梯平台、
 * 街面），不带楼层信息的话寻路会挑错那一层。
 */

export type WorldHotspot = {
  id: string;
  /** 所在导航层，取值见 navWorld 的 AREAS。 */
  layer: string;
  pos: [number, number, number];
  facing: number;
  animation?: 'sit';
};

/** 公共区与街区的落脚点。坐标按各场景的墙/门位置推，改几何时要跟着改。 */
export const WORLD_HOTSPOTS: WorldHotspot[] = [
  /* ---- 2F 公共区（apt2out）---- */
  // 北外廊东段：靠着栏杆看街
  { id: 'corridor-view', layer: 'apt2out', pos: [26, 3.4, -6.6], facing: 3.14159 },
  // 北外廊中段（203 门口往西）
  { id: 'corridor-west', layer: 'apt2out', pos: [-12, 3.4, -6.6], facing: 0 },
  // 电梯厅（2F）：面朝轿厢门等梯
  { id: 'lift-lobby-2f', layer: 'apt2out', pos: [-34.6, 3.4, -2.2], facing: 0 },
  // 东端楼梯平台（2F）
  { id: 'stair-landing-2f', layer: 'apt2out', pos: [33.5, 3.4, -6.4], facing: 3.14159 },

  /* ---- 1F 大堂（hall1）---- */
  // 电梯厅（1F）
  { id: 'lift-lobby-1f', layer: 'hall1', pos: [-34.6, 0.062, -2.2], facing: 0 },
  // 大堂中庭
  { id: 'lobby-hall', layer: 'hall1', pos: [-8, 0.062, 0.5], facing: 0 },
  // 南门内侧（出门前）
  { id: 'lobby-south', layer: 'hall1', pos: [0, 0.062, 3.4], facing: 0 },

  /* ---- 街区（street）---- */
  // 公寓门口人行道
  { id: 'street-door', layer: 'street', pos: [0, 0, 6.6], facing: 0 },
  // 便利店门前
  { id: 'cvs-front', layer: 'street', pos: [0.78, 0, 12.9], facing: 0 },
  // 书店门前
  { id: 'book-front', layer: 'street', pos: [18.35, 0, 13.4], facing: 0 },
  // 地铁口前
  { id: 'subway-front', layer: 'street', pos: [-24, 0, 12.6], facing: 0 },
];
