export interface GameRect { x: number; y: number; width: number; height: number }

/** Keep the center and the usual top-right close button clear. All values are physical pixels. */
export function gamePosition(area: GameRect, size: { width: number; height: number }, cursor: { x: number; y: number }, slot = 0) {
  const margin = 12;
  const left = area.x + margin;
  const right = Math.max(left, area.x + area.width - size.width - margin);
  const top = area.y + margin;
  const bottom = Math.max(top, area.y + area.height - size.height - margin);
  const offset = Math.max(0, slot) * (size.width + margin);
  const positions = [{ x: Math.min(right, left + offset), y: bottom },
    { x: Math.max(left, right - offset), y: bottom }, { x: left, y: Math.min(bottom, top + slot * size.height) }];
  return positions.sort((a, b) => distance(b) - distance(a))[0];
  function distance(p: { x: number; y: number }) {
    return Math.hypot(cursor.x - p.x - size.width / 2, cursor.y - p.y - size.height / 2);
  }
}
