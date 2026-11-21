export const FREEZE_EDGES = ['n', 'ne', 'e', 'se', 's', 'sw', 'w', 'nw'] as const;
export type FreezeEdge = typeof FREEZE_EDGES[number];

export function edgeResizeScale(edge: FreezeEdge, width: number, height: number, scale: number, dx: number, dy: number): number {
  const horizontal = edge.includes('e') ? dx / width : edge.includes('w') ? -dx / width : 0;
  const vertical = edge.includes('s') ? dy / height : edge.includes('n') ? -dy / height : 0;
  const delta = edge.length === 1 ? horizontal + vertical : Math.abs(horizontal) > Math.abs(vertical) ? horizontal : vertical;
  return Math.max(0.1, Math.min(8, scale * (1 + delta)));
}
