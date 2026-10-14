export interface TerrainRect { id: string; x: number; y: number; width: number; height: number; landable?: boolean }
export interface TerrainWorld { windows: TerrainRect[]; floors: TerrainRect[] }
const right = (r: TerrainRect) => r.x + r.width;
const bottom = (r: TerrainRect) => r.y + r.height;
const overlapsX = (a: TerrainRect, b: TerrainRect) => a.x < right(b) && right(a) > b.x;
const contains = (box: TerrainRect, body: TerrainRect, inset = 0) =>
  body.x >= box.x + inset && right(body) <= right(box) - inset &&
  body.y >= box.y + inset && bottom(body) <= bottom(box) - inset;

/** Upward normal impact in physical px/s, normalized for display scaling. */
export function ceilingDizzyMs(impact: number, scale: number): number | null {
  const speed = impact / scale;
  if (!Number.isFinite(speed) || !Number.isFinite(scale) || scale <= 0 || speed < 600) return null;
  return 1400 + Math.min(1000, Math.floor((speed - 600) * 1.2));
}

/** One physical body; coordinates are physical pixels, velocities are pixels/second. */
export class DesktopPhysics {
  body: TerrainRect;
  vx = 0;
  vy = 0;
  support: string | null = null;
  enclosure: string | null = null;
  world: TerrainWorld = { windows: [], floors: [] };
  private wallVelocity = { left: 0, right: 0, top: 0, bottom: 0 };

  constructor(body: TerrainRect, readonly scale: number, readonly windowTopOffset = 0, readonly windowBottomOffset = 0) { this.body = { ...body }; }

  private get screenFloor(): TerrainRect | undefined {
    const x = this.body.x + this.body.width / 2, y = this.body.y + this.body.height / 2;
    const distance = (r: TerrainRect) => Math.max(r.x - x, 0, x - right(r)) ** 2 + Math.max(r.y - y, 0, y - bottom(r)) ** 2;
    return [...this.world.floors].sort((a, b) => distance(a) - distance(b))[0];
  }

  /** Windows arrive in front-to-back Z order. Subtract every covered roof interval. */
  private roofLane(roof: TerrainRect, world = this.world, bodyX = this.body.x): { minX: number; maxX: number } | null {
    if (roof.landable === false) return null;
    const index = world.windows.findIndex(r => r.id === roof.id);
    if (index < 0) return null;
    let spans = [{ left: roof.x, right: right(roof) }];
    for (const front of world.windows.slice(0, index)) {
      if (front.y > roof.y || bottom(front) <= roof.y) continue;
      spans = spans.flatMap(span => {
        if (front.x >= span.right || right(front) <= span.left) return [span];
        const result = [];
        if (front.x > span.left) result.push({ left: span.left, right: front.x });
        if (right(front) < span.right) result.push({ left: right(front), right: span.right });
        return result;
      });
    }
    const span = spans.find(span => bodyX >= span.left && bodyX + this.body.width <= span.right);
    return span ? { minX: span.left, maxX: span.right - this.body.width } : null;
  }

  hasVisibleSupport(world = this.world, bodyX = this.body.x): boolean {
    if (!this.support) return false;
    if (this.enclosure) return world.windows.some(r => r.id === this.support);
    if (world.floors.some(r => r.id === this.support)) return true;
    const roof = world.windows.find(r => r.id === this.support);
    return !!roof && !!this.roofLane(roof, world, bodyX);
  }

  release(vx: number, vy: number): void {
    this.vx = Number.isFinite(vx) ? vx : 0;
    this.vy = Number.isFinite(vy) ? vy : 0;
    this.support = null;
    this.enclosure = this.world.windows.find(box => box.landable !== false && contains(box, this.body, 4 * this.scale))?.id ?? null;
  }

  updateWorld(world: TerrainWorld, seconds: number): void {
    const prior = this.world.windows;
    const dt = Math.max(.016, Math.min(.5, seconds));
    const s = this.scale;
    for (const box of world.windows) {
      if (box.landable === false) continue;
      const old = prior.find(rect => rect.id === box.id);
      if (!old) continue; // A newly appearing window is not evidence of a sweep.
      if (this.support === box.id && !this.enclosure) {
        this.body.x += box.x - old.x;
        this.body.y += box.y - old.y;
      }
      if (this.enclosure === box.id) {
        const limit = (n: number) => Math.max(-1200 * s, Math.min(1200 * s, n / dt));
        this.wallVelocity = { left: limit(box.x - old.x), right: limit(right(box) - right(old)),
          top: limit(box.y - old.y), bottom: limit(bottom(box) - bottom(old)) };
        continue;
      }
      if (this.enclosure || this.support === box.id || bottom(this.body) <= Math.min(box.y, old.y) + 3 * s) continue;
      if (Math.min(bottom(box), bottom(this.body)) - Math.max(box.y, this.body.y) <= 4 * s) continue;
      const hitRight = right(box) > right(old) && right(old) <= this.body.x + 2 * s && right(box) >= this.body.x;
      const hitLeft = box.x < old.x && old.x >= right(this.body) - 2 * s && box.x <= right(this.body);
      if (hitRight || hitLeft) {
        const speed = Math.min(1500 * s, Math.abs((hitRight ? right(box) - right(old) : box.x - old.x) / dt));
        if (speed < 25 * s) continue;
        this.body.x = hitRight ? right(box) + 2 * s : box.x - this.body.width - 2 * s;
        this.vx = (hitRight ? 1 : -1) * Math.max(240 * s, speed * .9);
        this.vy = -Math.min(400 * s, speed * .25);
        this.support = null;
        break;
      }
    }
    if (this.support && ![...world.windows, ...world.floors].some(rect => rect.id === this.support)) this.support = null;
    if (this.enclosure && !world.windows.some(rect => rect.landable !== false && rect.id === this.enclosure && rect.width > this.body.width + 8 * s && rect.height > this.body.height + 8 * s)) {
      this.enclosure = null;
      this.support = null;
    }
    this.world = world;
    if (this.support && !this.hasVisibleSupport()) this.support = null;
  }

  step(seconds: number): { impact: number; ceilingImpact: number; airborne: boolean } {
    if (!Number.isFinite(seconds) || seconds <= 0) return { impact: 0, ceilingImpact: 0, airborne: !this.support };
    // Bound integration after sleep or a suspended WebView; no huge catch-up impulse.
    const dt = Math.min(.05, seconds);
    const s = this.scale;
    const before = { ...this.body };
    this.vx *= Math.exp(-(this.support ? 7 : .35) * dt);
    this.vy = Math.min(1500 * s, this.vy + 1700 * s * dt);
    this.body.x += this.vx * dt;
    this.body.y += this.vy * dt;
    this.support = null;
    let impact = 0;
    let ceilingImpact = 0;
    const floor = this.screenFloor;
    let box = this.world.windows.find(rect => rect.landable !== false && rect.id === this.enclosure);
    if (box && floor && Math.min(bottom(box) - 4 * s, bottom(floor) - this.windowBottomOffset) -
        Math.max(box.y + 4 * s, floor.y + this.windowTopOffset) < this.body.height) {
      this.enclosure = null; box = undefined;
    }
    if (box) {
      const top = Math.max(box.y + 4 * s, floor ? floor.y + this.windowTopOffset : -Infinity);
      const base = Math.min(bottom(box) - 4 * s, floor ? bottom(floor) - this.windowBottomOffset : Infinity);
      const bounds = { ...box, x: box.x + 4 * s, y: top, width: box.width - 8 * s, height: base - top };
      const collide = (velocity: number, wall: number, direction: number) => {
        const relative = velocity - wall;
        impact = Math.max(impact, Math.abs(relative));
        return relative * direction > 0 ? wall - relative * .42 : velocity;
      };
      if (this.body.x < bounds.x) { this.body.x = bounds.x; this.vx = collide(this.vx, this.wallVelocity.left, -1); }
      if (right(this.body) > right(bounds)) { this.body.x = right(bounds) - this.body.width; this.vx = collide(this.vx, this.wallVelocity.right, 1); }
      if (this.body.y < bounds.y) {
        if (before.y > bounds.y && this.vy < 0) ceilingImpact = Math.max(0, this.wallVelocity.top - this.vy);
        this.body.y = bounds.y; this.vy = collide(this.vy, this.wallVelocity.top, -1);
      }
      if (bottom(this.body) >= bottom(bounds)) {
        this.body.y = bottom(bounds) - this.body.height;
        this.vy = collide(this.vy, this.wallVelocity.bottom, 1);
        if (Math.abs(this.vy - this.wallVelocity.bottom) < 100 * s) { this.vy = 0; this.support = box.id; }
      }
    } else {
      if (floor) {
        if (this.body.x < floor.x) { this.body.x = floor.x; impact = Math.max(impact, Math.abs(this.vx)); this.vx = Math.abs(this.vx) * .4; }
        if (right(this.body) > right(floor)) { this.body.x = right(floor) - this.body.width; impact = Math.max(impact, Math.abs(this.vx)); this.vx = -Math.abs(this.vx) * .4; }
        // Keep the complete pet window above its body footprint inside the work area.
        const ceilingY = floor.y + this.windowTopOffset;
        if (this.body.y < ceilingY) {
          if (before.y > ceilingY && this.vy < 0) ceilingImpact = -this.vy;
          impact = Math.max(impact, ceilingImpact);
          this.body.y = ceilingY; this.vy = Math.abs(this.vy) * .35;
        }
      }
      const roof = this.world.windows.filter(r =>
        overlapsX(this.body, r) && !!this.roofLane(r) && (!floor || (r.y - this.body.height - this.windowTopOffset >= floor.y && r.y <= bottom(floor) - this.windowBottomOffset)) &&
        bottom(before) <= r.y + 2 * s && bottom(this.body) >= r.y && this.vy >= 0)
        .sort((a, b) => a.y - b.y)[0];
      const groundY = roof?.y ?? (floor ? bottom(floor) - this.windowBottomOffset : Infinity);
      if (bottom(this.body) >= groundY && this.vy >= 0) {
        this.body.y = groundY - this.body.height;
        impact = Math.max(impact, this.vy);
        this.vy = this.vy > 180 * s ? -this.vy * .3 : 0;
        if (this.vy === 0) this.support = roof?.id ?? floor?.id ?? null;
      }
    }
    return { impact, ceilingImpact, airborne: !this.support || Math.abs(this.vx) > 20 * s };
  }

  get lane(): { minX: number; maxX: number; groundY: number } | null {
    if (!this.support || Math.abs(this.vx) > 20 * this.scale) return null;
    const r = [...this.world.windows, ...this.world.floors].find(rect => rect.id === this.support);
    if (!r) return null;
    const floor = this.screenFloor;
    const lane = !this.enclosure && this.world.windows.includes(r) ? this.roofLane(r) : null;
    if (!this.enclosure && this.world.windows.includes(r) && !lane) return null;
    return { minX: lane?.minX ?? r.x + (this.enclosure ? 4 * this.scale : 0), maxX: lane?.maxX ?? right(r) - this.body.width - (this.enclosure ? 4 * this.scale : 0),
      groundY: this.enclosure ? Math.min(bottom(r) - 4 * this.scale, floor ? bottom(floor) - this.windowBottomOffset : Infinity) : this.world.windows.includes(r) ? r.y : bottom(r) - this.windowBottomOffset };
  }
}

/** Approximate standing-body footprint using the renderer's stage size and foot baseline. */
export function petFootprint(width: number, height: number) {
  const side = Math.min(width * .96, height * .94);
  return { left: (width - side) / 2 + side * .31, top: height - side * .8, width: side * .38, height: side * .72 };
}
