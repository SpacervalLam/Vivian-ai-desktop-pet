export interface BrowserRect { x: number; y: number; width: number; height: number }
/** Intersection in CSS pixels; native clipping never changes the document size. */
export function intersectBrowserRect(a: BrowserRect, b: BrowserRect): BrowserRect {
  const x = Math.max(a.x, b.x), y = Math.max(a.y, b.y);
  return { x, y, width: Math.max(0, Math.min(a.x + a.width, b.x + b.width) - x), height: Math.max(0, Math.min(a.y + a.height, b.y + b.height) - y) };
}
export function browserViewportClip(viewport: HTMLElement): BrowserRect {
  const rect = viewport.getBoundingClientRect();
  let clip = intersectBrowserRect(rect, { x: 0, y: 0, width: innerWidth, height: innerHeight });
  for (let parent = viewport.parentElement; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    const clipsX = /^(hidden|clip|auto|scroll)$/.test(style.overflowX);
    const clipsY = /^(hidden|clip|auto|scroll)$/.test(style.overflowY);
    if (!clipsX && !clipsY) continue;
    const bounds = parent.getBoundingClientRect();
    clip = intersectBrowserRect(clip, {
      x: clipsX ? bounds.x : clip.x, y: clipsY ? bounds.y : clip.y,
      width: clipsX ? bounds.width : clip.width, height: clipsY ? bounds.height : clip.height,
    });
  }
  return clip;
}
