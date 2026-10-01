interface Rect { x: number; y: number; width: number; height: number }

/** All inputs use one coordinate system, including monitors with negative origins. */
export function placeBubble(pet: Rect, monitor: Rect, width: number, height: number) {
  const position = pet.y - monitor.y >= height + 4 ? 'top' as const : 'bottom' as const;
  const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(value, max));
  return {
    x: Math.round(clamp(pet.x + pet.width - width, monitor.x + 4, monitor.x + monitor.width - width - 4)),
    y: Math.round(clamp(position === 'top' ? pet.y - height : pet.y + pet.height,
      monitor.y + 4, monitor.y + monitor.height - height - 4)),
    position,
  };
}
