export interface BrowserOverlayFrame { key: string; image: string }

/** Capture once per overlay, discarding late results after navigation/close/unmount. */
export class BrowserOverlayCapture {
  private key: string | null = null;
  private generation = 0;
  private pending = false;
  private frame: BrowserOverlayFrame | null = null;
  private loaded = false;
  private failed = false;
  constructor(private commit: (frame: BrowserOverlayFrame | null) => void) {}
  request(key: string, capture: () => Promise<string>) {
    if (this.key !== key) { this.clear(); this.key = key; }
    if (this.pending || this.frame || this.failed) return;
    this.pending = true;
    const generation = this.generation;
    void Promise.resolve().then(capture).then(image => {
      if (generation !== this.generation) return;
      if (!/^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(image)) throw new Error('Invalid viewport image');
      this.pending = false;
      this.frame = { key, image };
      this.commit(this.frame);
    }).catch(() => { if (generation === this.generation) this.fail(key); });
  }
  markReady(key: string, frame?: BrowserOverlayFrame) { if (this.frame?.key === key && (!frame || frame === this.frame)) this.loaded = true; }
  ready(key: string) { return this.key === key && this.loaded; }
  active() { return this.key !== null; }
  hasFailed(key: string) { return this.key === key && this.failed; }
  fail(key: string) {
    if (this.key !== key) return;
    this.generation++; this.pending = false; this.failed = true; this.loaded = false;
    if (this.frame) { this.frame = null; this.commit(null); }
  }
  clear() {
    this.generation++; this.key = null; this.pending = false; this.loaded = false; this.failed = false;
    if (this.frame) { this.frame = null; this.commit(null); }
  }
}

export interface BrowserRect { x: number; y: number; width: number; height: number }
/** Capture failure fallback: keep a live, unobscured portion instead of blanking the page. */
export function unobscuredBrowserRect(viewport: BrowserRect, overlays: BrowserRect[]): BrowserRect {
  let parts = [viewport];
  for (const overlay of overlays) parts = parts.flatMap(part => {
    const left = Math.max(part.x, overlay.x), top = Math.max(part.y, overlay.y);
    const right = Math.min(part.x + part.width, overlay.x + overlay.width), bottom = Math.min(part.y + part.height, overlay.y + overlay.height);
    if (right <= left || bottom <= top) return [part];
    return [
      { x: part.x, y: part.y, width: part.width, height: top - part.y },
      { x: part.x, y: bottom, width: part.width, height: part.y + part.height - bottom },
      { x: part.x, y: top, width: left - part.x, height: bottom - top },
      { x: right, y: top, width: part.x + part.width - right, height: bottom - top },
    ].filter(rect => rect.width > 0 && rect.height > 0);
  });
  return parts.reduce((best, part) => part.width * part.height > best.width * best.height ? part : best, { ...viewport, width: 0, height: 0 });
}
