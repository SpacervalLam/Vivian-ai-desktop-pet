export const browserSubmenuDelay = 200;

/** Fixed popovers escape the inspector's clipping boundary, staying side by side. */
export function browserMenuLayout(viewport: { width: number; height: number }, anchor: { right: number; bottom: number }, secondary: boolean, rowTop?: number) {
  const edge = 8, gap = 8;
  const available = Math.max(0, viewport.width - edge * 2);
  const mainWidth = Math.min(280, secondary ? (available - gap) / 2 : available);
  const mainLeft = Math.max(edge, Math.min(anchor.right - mainWidth, viewport.width - edge - mainWidth));
  const top = Math.max(edge, Math.min(anchor.bottom + gap, viewport.height - 100));
  const rightRoom = viewport.width - edge - (mainLeft + mainWidth + gap);
  const leftRoom = mainLeft - gap - edge;
  const side = rightRoom >= 300 || rightRoom >= leftRoom ? 'right' : 'left';
  const width = Math.max(0, Math.min(340, side === 'right' ? rightRoom : leftRoom));
  const subTop = Math.max(edge, Math.min(rowTop ?? top, viewport.height - 260));
  return { main: { left: mainLeft, top, width: mainWidth, maxHeight: Math.max(80, viewport.height - top - edge) },
    sub: { left: side === 'right' ? mainLeft + mainWidth + gap : mainLeft - gap - width, top: subTop, width, maxHeight: Math.max(80, viewport.height - subTop - edge) }, side };
}
