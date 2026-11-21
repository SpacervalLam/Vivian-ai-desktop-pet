import defaultSize from './defaultSize.json';

export interface TerrainRect {
  id: string; x: number; y: number; width: number; height: number; landable?: boolean;
  /** 其它桌宠窗口的角色 label；外部应用窗口没有该字段。 */
  pet?: string;
}
export interface TerrainWorld { windows: TerrainRect[]; floors: TerrainRect[] }
const right = (r: TerrainRect) => r.x + r.width;
const bottom = (r: TerrainRect) => r.y + r.height;
const overlapsX = (a: TerrainRect, b: TerrainRect) => a.x < right(b) && right(a) > b.x;
const contains = (box: TerrainRect, body: TerrainRect, inset = 0) =>
  body.x >= box.x + inset && right(body) <= right(box) - inset &&
  body.y >= box.y + inset && bottom(body) <= bottom(box) - inset;

/**
 * 桌宠之间的碰撞恢复系数：0 = 完全非弹性（同速），1 = 完全弹性（速度交换）。
 * 等质量下双方按 (1∓e)/2 分配动量；取 0.3 是「撞方明显减速、被撞方被推走」，
 * 而不是 e≈1 那种撞方停住、被撞方飞出去。
 */
const PET_RESTITUTION = 0.3;

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
  /** 其它桌宠的水平速度估计（physical px/s），由两次地形采样之间的位移差分得到。 */
  private petVx = new Map<string, number>();
  /**
   * 上一帧仍在接触的其它桌宠。
   *
   * 动量交换只允许发生在**接触开始的那一帧**：接触期间每帧再算一次的话，
   * 撞方速度会按 (1-e)/2 逐帧衰减，三帧就归零，被撞方也只拿到残值——
   * 表现就是「一碰，两只一起僵住」。
   */
  private touching = new Set<string>();

  constructor(body: TerrainRect, readonly scale: number, readonly windowTopOffset = 0) { this.body = { ...body }; }

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
    // 对方的速度只能从两次地形采样的位移差里估：每只桌宠各跑各的物理，
    // 不开额外通道也能拿到「它正往哪走、多快」，碰撞才能传递动量。
    this.petVx.clear();
    for (const box of world.windows) {
      if (!box.pet) continue;
      const old = prior.find(rect => rect.id === box.id);
      if (old) this.petVx.set(box.pet, Math.max(-1500 * s, Math.min(1500 * s, (box.x - old.x) / dt)));
    }
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
    // Let the transparent margin below the soles extend past the screen edge.
    const floorY = floor ? bottom(floor) : Infinity;
    let box = this.world.windows.find(rect => rect.landable !== false && rect.id === this.enclosure);
    if (box && floor && Math.min(bottom(box) - 4 * s, floorY) -
        Math.max(box.y + 4 * s, floor.y + this.windowTopOffset) < this.body.height) {
      this.enclosure = null; box = undefined;
    }
    if (box) {
      const top = Math.max(box.y + 4 * s, floor ? floor.y + this.windowTopOffset : -Infinity);
      const base = Math.min(bottom(box) - 4 * s, floorY);
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
        overlapsX(this.body, r) && !!this.roofLane(r) && r.y <= floorY && (!floor || r.y - this.body.height - this.windowTopOffset >= floor.y) &&
        bottom(before) <= r.y + 2 * s && bottom(this.body) >= r.y && this.vy >= 0)
        .sort((a, b) => a.y - b.y)[0];
      const groundY = roof?.y ?? floorY;
      if (bottom(this.body) >= groundY && this.vy >= 0) {
        this.body.y = groundY - this.body.height;
        impact = Math.max(impact, this.vy);
        this.vy = this.vy > 180 * s ? -this.vy * .3 : 0;
        if (this.vy === 0) this.support = roof?.id ?? floor?.id ?? null;
      }
    }
    this.resolvePetOverlap();
    return { impact, ceilingImpact, airborne: !this.support || Math.abs(this.vx) > 20 * s };
  }

  /**
   * 桌宠之间是实体关系：和另一只桌宠的身体矩形重叠时，把自己挪到刚好不重叠。
   * 只做水平分离而不用最小平移轴——两者通常站在同一条地面线上，垂直推挤会把
   * 一只顶到另一只头上（或来回弹跳），水平推开后各自仍然站得住。
   *
   * 关键是**不给反向推力**：旧实现按重叠速度额外加 90~500px/s 的 shove，
   * 两只相遇会越推越快、来回抖；实体接触只需要挡住，剩下的交给地面摩擦。
   */
  private resolvePetOverlap(): void {
    const s = this.scale;
    // 接触容差：撞方每帧会把自己复位到「刚好不重叠再留一点间隙」，被撞方算出的
    // 重叠量恒为负值。只认严格重叠的话，速度交换那一支永远进不来——实机上就是
    // 两只互相顶住、动量一点没传出去，看起来像「一碰就静止」。
    const contact = 2 * s;
    // 只有双脚基本落在同一平面才算「并排的实体接触」。垂直方向只重叠几个像素、
    // 脚底却差着一大截时（一只在上、一只在下擦过），水平推开就是隔空撞空气。
    const sameLevel = 12 * s;
    const floor = this.screenFloor;
    const stillTouching = new Set<string>();
    for (const other of this.world.windows) {
      if (!other.pet) continue;
      const overlapX = Math.min(right(this.body), right(other)) - Math.max(this.body.x, other.x);
      const overlapY = Math.min(bottom(this.body), bottom(other)) - Math.max(this.body.y, other.y);
      if (overlapX <= -contact || overlapY <= 0) continue;
      if (Math.abs(bottom(this.body) - bottom(other)) > sameLevel) continue;
      stillTouching.add(other.pet);
      const selfCenter = this.body.x + this.body.width / 2, otherCenter = other.x + other.width / 2;
      // 中心完全重合时按 label 定序分向，两只桌宠才不会朝同一侧一起滑动。
      const toLeft = selfCenter < otherCenter
        || (selfCenter === otherCenter && this.body.id < (other.pet ?? ''));
      // 分离约束只往外推、不往里拉：已经迈开的一帧不能被重新吸附回接触位置，
      // 否则被撞方刚拿到的速度会被这一步原地按住，同样是「碰到就静止」。
      if (overlapX > 0) {
        this.body.x = toLeft
          ? Math.min(this.body.x, other.x - this.body.width - s)
          : Math.max(this.body.x, right(other) + s);
      }
      // 等质量碰撞：撞方减速、被撞方被推走。只有相对速度朝对方时才算撞击；
      // 已经在分离（或只是被位置修正）时保留原速度，避免把它们「粘」在一起。
      const otherVx = this.petVx.get(other.pet ?? '') ?? 0;
      const approaching = toLeft ? this.vx > otherVx : this.vx < otherVx;
      if (approaching && !this.touching.has(other.pet)) {
        // 对方在受力方向上已经贴着屏幕边缘：它没有地方可退，冲量只能从墙传回来。
        // 按「无限质量」处理，撞方以 -e 原速弹回；照等质量公式分摊会让撞方
        // 只是减速停住，手感像撞进棉花，而实际撞的是堵墙。
        const walled = !!floor && (toLeft
          ? right(other) >= right(floor) - .5
          : other.x <= floor.x + .5);
        this.vx = walled
          ? -PET_RESTITUTION * this.vx
          : (1 - PET_RESTITUTION) / 2 * this.vx + (1 + PET_RESTITUTION) / 2 * otherVx;
      }
    }
    this.touching = stillTouching;
    // 被墙挡住的桌宠吸收冲击：它拿到的是朝墙的速度，屏幕边界不会让它飞出去，
    // 只会让窗口越界一帧再被拉回（视觉上闪一下）。这里直接把这份速度吃掉，
    // 「撞到靠墙的桌宠」就该是推不动，而不是把对方顶出屏幕再弹回来。
    if (floor) {
      // 位置也要收：分离约束按对方矩形推出 s 间隙，贴着墙的一只会被挤出屏幕 1px，
      // 而窗口位置每帧都真的写出去，1px 也是可见的越界。
      if (this.body.x < floor.x) this.body.x = floor.x;
      if (right(this.body) > right(floor)) this.body.x = right(floor) - this.body.width;
      if (this.body.x <= floor.x + .5 && this.vx < 0) this.vx = 0;
      if (right(this.body) >= right(floor) - .5 && this.vx > 0) this.vx = 0;
    }
    // 被推离平台/遮挡区间后不能再假装自己还站在上面。
    if (this.support && !this.hasVisibleSupport()) this.support = null;
  }

  get lane(): { minX: number; maxX: number; groundY: number } | null {
    if (!this.support || Math.abs(this.vx) > 20 * this.scale) return null;
    const r = [...this.world.windows, ...this.world.floors].find(rect => rect.id === this.support);
    if (!r) return null;
    const floor = this.screenFloor;
    const lane = !this.enclosure && this.world.windows.includes(r) ? this.roofLane(r) : null;
    if (!this.enclosure && this.world.windows.includes(r) && !lane) return null;
    return { minX: lane?.minX ?? r.x + (this.enclosure ? 4 * this.scale : 0), maxX: lane?.maxX ?? right(r) - this.body.width - (this.enclosure ? 4 * this.scale : 0),
      groundY: this.enclosure ? Math.min(bottom(r) - 4 * this.scale, floor ? bottom(floor) : Infinity) : this.world.windows.includes(r) ? r.y : bottom(r) };
  }
}

/**
 * 站立身体的碰撞盒，按渲染层的真实几何推算，而不是猜比例。
 *
 * 舞台是 `min(96vw, 94vh)` 的正方形、底部对齐窗口，512 格素材铺满舞台；
 * `defaultSize` 提供待机素材共用的头顶与脚底基线，碰撞宽度取可见轮廓的代表值。
 */
const SPRITE_CELLS = 512;
const BODY_WIDTH_CELL = 331;

export function petFootprint(width: number, height: number) {
  const stage = Math.min(width * .96, height * .94);
  const perCell = stage / SPRITE_CELLS;
  const bodyWidth = BODY_WIDTH_CELL * perCell;
  return {
    left: (width - bodyWidth) / 2,
    top: height - stage + defaultSize.target.top * perCell,
    width: bodyWidth,
    height: (defaultSize.target.foot - defaultSize.target.top) * perCell,
  };
}

/**
 * 把另一只桌宠的整窗矩形换算成它的身体矩形（同样的窗口尺寸基准）。
 * 直接拿整窗做碰撞会把透明留白也算进去，两只桌宠会隔着一大段空气停下。
 */
export function petBody(rect: TerrainRect): TerrainRect {
  const footprint = petFootprint(rect.width, rect.height);
  return { ...rect, x: rect.x + footprint.left, y: rect.y + footprint.top,
    width: footprint.width, height: footprint.height, landable: false };
}
