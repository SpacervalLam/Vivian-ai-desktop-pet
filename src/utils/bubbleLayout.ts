interface Rect { x: number; y: number; width: number; height: number }

/** All inputs use one coordinate system, including monitors with negative origins. */
export function placeBubble(pet: Rect, monitor: Rect, width: number, height: number) {
  const position = pet.y - monitor.y >= height + 4 ? 'top' as const : 'bottom' as const;
  const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(value, max));
  const anchor = pet.x + pet.width / 2;
  const x = Math.round(clamp(anchor - width / 2, monitor.x + 4, monitor.x + monitor.width - width - 4));
  return {
    x,
    y: Math.round(clamp(position === 'top' ? pet.y - height : pet.y + pet.height,
      monitor.y + 4, monitor.y + monitor.height - height - 4)),
    position,
    anchorX: anchor - x,
  };
}

/** Keep each bubble in its row while aiming its tail at the pet's window center. */
export function placeBubbleContent(anchorX: number, width: number, availableWidth: number) {
  const inset = Math.min(20, width / 2);
  const left = Math.max(0, Math.min(anchorX - width / 2, availableWidth - width));
  return { left, tailX: Math.max(inset, Math.min(anchorX - left, width - inset)) };
}
