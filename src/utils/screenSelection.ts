export interface SelectionPoint { x: number; y: number }
export interface SelectionRect extends SelectionPoint { width: number; height: number }
export interface SelectionSize { width: number; height: number }
const clamp = (value: number, max: number) => Math.max(0, Math.min(value, max));

export function selectionRect(start: SelectionPoint, end: SelectionPoint, viewport: SelectionSize): SelectionRect {
  const x1 = clamp(start.x, viewport.width), y1 = clamp(start.y, viewport.height);
  const x2 = clamp(end.x, viewport.width), y2 = clamp(end.y, viewport.height);
  return { x: Math.min(x1, x2), y: Math.min(y1, y2), width: Math.abs(x2 - x1), height: Math.abs(y2 - y1) };
}

/** Map CSS coordinates to the frozen desktop's physical pixels, including mixed DPI monitors. */
export function captureRegion(rect: SelectionRect, viewport: SelectionSize, frame: SelectionSize): SelectionRect | null {
  if (viewport.width <= 0 || viewport.height <= 0 || rect.width < 2 || rect.height < 2) return null;
  const x = Math.floor(clamp(rect.x, viewport.width) * frame.width / viewport.width);
  const y = Math.floor(clamp(rect.y, viewport.height) * frame.height / viewport.height);
  const right = Math.ceil(clamp(rect.x + rect.width, viewport.width) * frame.width / viewport.width);
  const bottom = Math.ceil(clamp(rect.y + rect.height, viewport.height) * frame.height / viewport.height);
  if (right - x < 2 || bottom - y < 2) return null;
  return { x, y, width: right - x, height: bottom - y };
}
