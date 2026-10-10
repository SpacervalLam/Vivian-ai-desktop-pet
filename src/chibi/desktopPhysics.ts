export interface TerrainRect { id: string; x: number; y: number; width: number; height: number }
export interface TerrainWorld { windows: TerrainRect[]; floors: TerrainRect[] }
const right = (r: TerrainRect) => r.x + r.width;
const bottom = (r: TerrainRect) => r.y + r.height;
const overlapsX = (a: TerrainRect, b: TerrainRect) => a.x < right(b) && right(a) > b.x;
const contains = (box: TerrainRect, body: TerrainRect, inset = 0) =>
  body.x >= box.x + inset && right(body) <= right(box) - inset &&
  body.y >= box.y + inset && bottom(body) <= bottom(box) - inset;

/** One physical body; coordinates are physical pixels, velocities are pixels/second. */
export class DesktopPhysics {
  body: TerrainRect;
  vx = 0;
  vy = 0;
  support: string | null = null;
  enclosure: string | null = null;
  world: TerrainWorld = { windows: [], floors: [] };
  private wallVelocity = { left: 0, right: 0, top: 0, bottom: 0 };

  constructor(body: TerrainRect, readonly scale: number) { this.body = { ...body }; }

  release(vx: number, vy: number): void {
    this.vx = Number.isFinite(vx) ? vx : 0;
    this.vy = Number.isFinite(vy) ? vy : 0;
    this.support = null;
    this.enclosure = this.world.windows.find(box => contains(box, this.body, 4 * this.scale))?.id ?? null;
  }

  updateWorld(world: TerrainWorld, seconds: number): void {
    const prior = this.world.windows;
    const dt = Math.max(.016, Math.min(.5, seconds));
    const s = this.scale;
    for (const box of world.windows) {
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
    if (this.enclosure && !world.windows.some(rect => rect.id === this.enclosure && rect.width > this.body.width + 8 * s && rect.height > this.body.height + 8 * s)) {
      this.enclosure = null;
      this.support = null;
    }
    this.world = world;
  }

  step(seconds: number): { impact: number; airborne: boolean } {
    if (!Number.isFinite(seconds) || seconds <= 0) return { impact: 0, airborne: !this.support };
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
    const box = this.world.windows.find(rect => rect.id === this.enclosure);
    if (box) {
      const bounds = { ...box, x: box.x + 4 * s, y: box.y + 4 * s, width: box.width - 8 * s, height: box.height - 8 * s };
      const collide = (velocity: number, wall: number, direction: number) => {
        const relative = velocity - wall;
        impact = Math.max(impact, Math.abs(relative));
        return relative * direction > 0 ? wall - relative * .42 : velocity;
      };
      if (this.body.x < bounds.x) { this.body.x = bounds.x; this.vx = collide(this.vx, this.wallVelocity.left, -1); }
      if (right(this.body) > right(bounds)) { this.body.x = right(bounds) - this.body.width; this.vx = collide(this.vx, this.wallVelocity.right, 1); }
      if (this.body.y < bounds.y) { this.body.y = bounds.y; this.vy = collide(this.vy, this.wallVelocity.top, -1); }
      if (bottom(this.body) >= bottom(bounds)) {
        this.body.y = bottom(bounds) - this.body.height;
        this.vy = collide(this.vy, this.wallVelocity.bottom, 1);
        if (Math.abs(this.vy - this.wallVelocity.bottom) < 100 * s) { this.vy = 0; this.support = box.id; }
      }
    } else {
      const centerX = this.body.x + this.body.width / 2;
      const centerY = this.body.y + this.body.height / 2;
      const floor = [...this.world.floors].sort((a, b) => {
        const distance = (r: TerrainRect) => Math.max(r.x - centerX, 0, centerX - right(r)) ** 2 + Math.max(r.y - centerY, 0, centerY - bottom(r)) ** 2;
        return distance(a) - distance(b);
      })[0];
      if (floor) {
        if (this.body.x < floor.x) { this.body.x = floor.x; impact = Math.max(impact, Math.abs(this.vx)); this.vx = Math.abs(this.vx) * .4; }
        if (right(this.body) > right(floor)) { this.body.x = right(floor) - this.body.width; impact = Math.max(impact, Math.abs(this.vx)); this.vx = -Math.abs(this.vx) * .4; }
        if (this.body.y < floor.y) { this.body.y = floor.y; this.vy = Math.abs(this.vy) * .35; }
      }
      const roof = this.world.windows.filter((r, index, all) =>
        overlapsX(this.body, r) && bottom(before) <= r.y + 2 * s && bottom(this.body) >= r.y && this.vy >= 0 &&
        !all.slice(0, index).some(front => centerX >= front.x && centerX <= right(front) && r.y >= front.y && r.y < bottom(front)))
        .sort((a, b) => a.y - b.y)[0];
      const groundY = roof?.y ?? (floor ? bottom(floor) : Infinity);
      if (bottom(this.body) >= groundY && this.vy >= 0) {
        this.body.y = groundY - this.body.height;
        impact = Math.max(impact, this.vy);
        this.vy = this.vy > 180 * s ? -this.vy * .3 : 0;
        if (this.vy === 0) this.support = roof?.id ?? floor?.id ?? null;
      }
    }
    return { impact, airborne: !this.support || Math.abs(this.vx) > 20 * s };
  }

  get lane(): { minX: number; maxX: number; groundY: number } | null {
    if (!this.support || Math.abs(this.vx) > 20 * this.scale) return null;
    const r = [...this.world.windows, ...this.world.floors].find(rect => rect.id === this.support);
    if (!r) return null;
    return { minX: r.x + (this.enclosure ? 4 * this.scale : 0), maxX: right(r) - this.body.width - (this.enclosure ? 4 * this.scale : 0),
      groundY: this.enclosure ? bottom(r) - 4 * this.scale : this.world.windows.includes(r) ? r.y : bottom(r) };
  }
}

/** Approximate standing-body footprint using the renderer's stage size and foot baseline. */
export function petFootprint(width: number, height: number) {
  const side = Math.min(width * .96, height * .94);
  return { left: (width - side) / 2 + side * .31, top: height - side * .8, width: side * .38, height: side * .72 };
}
