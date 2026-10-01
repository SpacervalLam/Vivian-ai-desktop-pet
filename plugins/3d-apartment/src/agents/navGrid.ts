/**
 * 简易导航栅格 v2：布局驱动。
 *
 * 地面是套房的 L 外接矩形，障碍 = 外墙（只留落地门洞，窗洞补回实体）
 * + 内墙（按"落地洞口"断开，door/glass/pass 都可走）+ 阳台栏杆 + 家具 AABB，
 * 全部按角色半径膨胀。
 *
 * 三层防护，缺一层就会贴墙穿模：
 *   1. 硬膨胀（CHARACTER_RADIUS）：路径点离障碍不会比它更近，过窄的门洞直接判定不可走；
 *   2. 软净空（SOFT_CLEARANCE）：A* 对贴障碍的格子加价，开阔处路线自动走通道中线，
 *      而不是像纯栅格 A* 那样贴着障碍边缘找最短路；
 *   3. 拉绳平滑：把逐格折线压成直线段，去掉锯齿，拐角不再把身体甩进墙里。
 *
 * 之所以不直接用 rapier：
 * - 地面是 11×8.5 的简单矩形，74×57 栅格完全够用
 * - 障碍是几十个 AABB，膨胀一次就行
 * - 直走 + 简单 A* 比物理引擎便宜一个数量级
 * - 项目里 rapier 依赖在但从来没用过——P0 阶段不引入新的失败面
 */

export type AABB = { minX: number; maxX: number; minZ: number; maxZ: number };
export type NavPoint = { x: number; z: number };
/**
 * 动态排斥圆（另一个角色）。
 *
 * 角色是活的障碍：栅格是静态预计算的，塞不进会动的东西。所以把「对方现在站哪」
 * 当软代价在查路时现算——绕开人，而不是把人钉进栅格。只加价不封死，
 * 否则两头顶死时双方都算不出路，直接互相锁死。
 */
export type AvoidCircle = { x: number; z: number; r: number };

/**
 * 角色硬半径（米）——障碍按它膨胀，路径绝不会比它更贴。
 *
 * 0.18 是按「人」的直觉拍的，实测对不上模型。角色归一化后站立姿态前后 0.70、
 * 左右 0.91（含头发/裙摆/尾巴），0.18 意味着这些部分常年埋在墙里。
 *
 * 上限不是角色尺寸而是门洞：最窄的卫生间门净宽 0.72m，两侧各膨胀 R 后还要
 * 剩下至少一个格宽（0.15）才连得通 ⇒ R ≤ 0.285。取 0.26 留一格余量。
 * 剩下的差值靠 SOFT_CLEARANCE 把路线拉回通道中线 + 角色整体缩到 1.30m 抹平。
 */
export const CHARACTER_RADIUS = 0.26;
/**
 * 软净空：A* 对「离障碍不足这个距离」的格子加价，路就会主动走通道中间。
 * 只加价不封死——门洞里没得选时照样能挤过去，不会像硬膨胀那样把房间走成孤岛。
 */
export const SOFT_CLEARANCE = 0.45;
/** 软净空单价：不足部分每米折算的额外步数。 */
const CLEARANCE_WEIGHT = 4;
/** 绕人的单价：比贴墙更贵——撞人比蹭墙显眼。 */
const PEER_WEIGHT = 6;
/** 墙的导航厚度：比视觉墙厚（0.12）略宽，门洞两侧留出贴墙余量。 */
const WALL_NAV_HALF = 0.09;

type WallLike = {
  axis: 'x' | 'z';
  at: number;
  from: number;
  to: number;
  exterior?: boolean;
  openings?: Array<{ kind: string; a: number; b: number; y0: number; y1: number; navCut?: 'east' | 'west' }>;
};

type LayoutLike = {
  room: { bounds: { x0: number; x1: number; z0: number; z1: number } };
  shell: { walls: WallLike[]; railing?: Array<{ axis: 'x' | 'z'; at: number; from: number; to: number }> };
  furniture: Array<{ nav?: boolean; pos: [number, number, number]; size: [number, number, number]; rot?: number }>;
};

/** 一段墙（或栏杆）沿墙方向切成 AABB，落地洞口处留缺口。 */
function wallAABBs(w: WallLike, allowGaps: boolean): AABB[] {
  /**
   * 只认"能走过去"的落地洞口：door / pass / glass；window 永远补回实体。
   *
   * 统一按 kind 判断洞口是否可走。此前外墙不看洞口（allowGaps = !exterior），
   * 落地玻璃门被当成实心墙、阳台成了孤立区——check-layout 放行但运行时走不过去，
   * 两个校验口径不一致。越界由栅格边界（建筑 + 阳台外廓）兜底，门外是死胡同。
   */
  const walkableKind = (op: { kind: string }) => op.kind !== 'window';
  const ops = (allowGaps ? (w.openings ?? []) : [])
    .filter(op => op.y0 < 0.1 && walkableKind(op))   // 只有落地洞口可走（窗洞不算）
    .sort((p, q) => p.a - q.a);
  const out: AABB[] = [];
  const mk = (a0: number, a1: number) => {
    if (a1 - a0 < 0.02) return;
    if (w.axis === 'z') {
      out.push({ minX: a0, maxX: a1, minZ: w.at - WALL_NAV_HALF, maxZ: w.at + WALL_NAV_HALF });
    } else {
      out.push({ minX: w.at - WALL_NAV_HALF, maxX: w.at + WALL_NAV_HALF, minZ: a0, maxZ: a1 });
    }
  };
  let cursor = w.from;
  for (const op of ops) {
    const c = (op.a + op.b) / 2;
    // navCut：视觉开口取整段（a..b，门框渲染用），但宠物可走的门洞只留其中半边，
    // 另半边恒为实心固定扇。固定扇恒在局部 -x，绕 Y 旋转 rot 后落到世界 +x 还是 -x
    // 取决于门朝向：glass-living 设 rot=π，固定扇翻到世界 +x（东半），让出的是西半，
    // 故配 navCut:'west'（西半可走、东半实心）。若写反，宠物会从固定扇占着的半边穿过去。
    const gapA = op.navCut === 'east' ? c : op.a;
    const gapB = op.navCut === 'west' ? c : op.b;
    mk(cursor, gapA);
    cursor = Math.max(cursor, gapB);
  }
  mk(cursor, w.to);
  return out;
}

/** 墙（外墙实心 / 内墙留门洞）+ 栏杆 + 家具的 AABB 列表。 */
export function buildObstacles(layout: LayoutLike): AABB[] {
  const walls: AABB[] = [];
  for (const w of layout.shell.walls) {
    walls.push(...wallAABBs(w, true));
  }
  for (const r of layout.shell.railing ?? []) {
    walls.push(...wallAABBs(r, false));
  }
  const solids = layout.furniture.filter(it => it.nav !== false && Array.isArray(it.size) && it.size[0] > 0 && it.size[2] > 0);
  const f: AABB[] = solids.map(it => {
    const [x, , z] = it.pos;
    const [w, , d] = it.size;
    /**
     * 必须算"旋转后的轴对齐外接盒"。
     * 早期版本直接拿 center ± size/2，等于把转过 90° 的床/沙发当成没转，
     * 导航占地和实际占地错开整整一个长宽差：角色会走进家具里，
     * 而家具的视觉本体又压在墙里——这就是之前穿模的一大来源。
     */
    const r = it.rot ?? 0;
    const c = Math.abs(Math.cos(r)), s = Math.abs(Math.sin(r));
    const ex = (w * c + d * s) / 2;
    const ez = (w * s + d * c) / 2;
    return { minX: x - ex, maxX: x + ex, minZ: z - ez, maxZ: z + ez };
  });
  return [...walls, ...f];
}

/** 角色半径膨胀后的 AABB（让路径不会贴着墙走）。 */
function inflate(a: AABB, by: number): AABB {
  return { minX: a.minX - by, maxX: a.maxX + by, minZ: a.minZ - by, maxZ: a.maxZ + by };
}

function inAABB(p: NavPoint, a: AABB): boolean {
  return p.x >= a.minX && p.x <= a.maxX && p.z >= a.minZ && p.z <= a.maxZ;
}

/** 点到 AABB 的水平距离（在盒内为 0）。 */
function rectDistance(x: number, z: number, a: AABB): number {
  const dx = Math.max(a.minX - x, 0, x - a.maxX);
  const dz = Math.max(a.minZ - z, 0, z - a.maxZ);
  return Math.hypot(dx, dz);
}

export type NavGrid = ReturnType<typeof buildGrid>;

/** 障碍栅格预计算一次。分辨率按"目标格宽 ~0.15m"从布局算出。 */
export function buildGrid(
  obstacles: AABB[],
  bounds: { x0: number; x1: number; z0: number; z1: number },
  radius: number = CHARACTER_RADIUS,   // 窄空间（店内）可以放宽，见 navWorld 的 AreaSpec.radius
  /** 目标格宽（米）。室内 0.15 才贴得住门框；街区 ±80m 用 0.5，否则格子数上百万。 */
  cellTarget = 0.15
) {
  const CELL_TARGET = cellTarget;
  const resX = Math.max(8, Math.ceil((bounds.x1 - bounds.x0) / CELL_TARGET));
  const resZ = Math.max(8, Math.ceil((bounds.z1 - bounds.z0) / CELL_TARGET));
  const cell = Math.max((bounds.x1 - bounds.x0) / resX, (bounds.z1 - bounds.z0) / resZ);

  const blocked: boolean[][] = [];
  for (let i = 0; i < resX; i++) blocked[i] = new Array(resZ).fill(false);
  /**
   * 净空距离场，**按障碍刷格**而不是按格遍历障碍。
   *
   * 语义不变但快了一个量级：街区那层 10 万格 × 200 多个障碍 = 两千万次距离运算，
   * 街机一样卡启动；改成"每个障碍只刷它周围 CAP 范围内的格子"之后，
   * 总工作量取决于障碍周长而不是格数。代价是远处的净空被截断成 CAP——
   * 不影响 A*（代价 = max(0, SOFT_CLEARANCE − clearance)，超过 SOFT_CLEARANCE 就是 0），
   * 也不影响平滑（那里只判断"够不够 radius"）。
   */
  const CLEARANCE_CAP = SOFT_CLEARANCE + cell * 2;
  const clearance = new Float32Array(resX * resZ).fill(CLEARANCE_CAP);
  const clampI = (v: number, hi: number) => Math.max(0, Math.min(hi, v));
  const inflated = obstacles.map(a => inflate(a, radius));
  for (let k = 0; k < obstacles.length; k++) {
    const raw = obstacles[k];
    const inf = inflated[k];
    // blocked：格中心落在膨胀盒里就封死（与旧的 inflated.some(inAABB) 等价）
    const bi0 = clampI(Math.floor((inf.minX - bounds.x0) / cell), resX - 1);
    const bi1 = clampI(Math.ceil((inf.maxX - bounds.x0) / cell), resX - 1);
    const bj0 = clampI(Math.floor((inf.minZ - bounds.z0) / cell), resZ - 1);
    const bj1 = clampI(Math.ceil((inf.maxZ - bounds.z0) / cell), resZ - 1);
    for (let i = bi0; i <= bi1; i++) {
      const x = bounds.x0 + (i + 0.5) * cell;
      for (let j = bj0; j <= bj1; j++) {
        const z = bounds.z0 + (j + 0.5) * cell;
        if (inAABB({ x, z }, inf)) blocked[i][j] = true;
      }
    }
    // clearance：只刷 CAP 邻域
    const ci0 = clampI(Math.floor((raw.minX - CLEARANCE_CAP - bounds.x0) / cell), resX - 1);
    const ci1 = clampI(Math.ceil((raw.maxX + CLEARANCE_CAP - bounds.x0) / cell), resX - 1);
    const cj0 = clampI(Math.floor((raw.minZ - CLEARANCE_CAP - bounds.z0) / cell), resZ - 1);
    const cj1 = clampI(Math.ceil((raw.maxZ + CLEARANCE_CAP - bounds.z0) / cell), resZ - 1);
    for (let i = ci0; i <= ci1; i++) {
      const x = bounds.x0 + (i + 0.5) * cell;
      for (let j = cj0; j <= cj1; j++) {
        const at = i * resZ + j;
        if (clearance[at] === 0) continue;   // 已经是 0 了，不用再算
        const z = bounds.z0 + (j + 0.5) * cell;
        const d = rectDistance(x, z, raw);
        if (d < clearance[at]) clearance[at] = d;
      }
    }
  }
  return { blocked, clearance, obstacles, radius, res: resX, resZ, cell, halfW: (bounds.x1 - bounds.x0) / 2, halfD: (bounds.z1 - bounds.z0) / 2, x0: bounds.x0, z0: bounds.z0 };
}

/**
 * 任意点的精确净空（按 AABB 真算）。
 *
 * 判断"这个点能不能站人"必须用它而不是 clearanceAt：后者取最近格中心的值，
 * 误差可达半格（0.075m）——让行侧移是靠它筛落点的，误判的结果是角色贴着墙 5cm 站。
 */
export function clearanceExactAt(x: number, z: number, g: NavGrid): number {
  let best = Infinity;
  for (const a of g.obstacles) {
    const d = rectDistance(x, z, a);
    if (d < best) best = d;
  }
  return best;
}

/** 任意点的净空（取最近格，误差 ≤ 半格；够判断"能不能贴着走过去"）。 */
export function clearanceAt(x: number, z: number, g: NavGrid): number {
  const i = Math.max(0, Math.min(g.res - 1, Math.floor((x - g.x0) / g.cell)));
  const j = Math.max(0, Math.min(g.resZ - 1, Math.floor((z - g.z0) / g.cell)));
  return g.clearance[i * g.resZ + j];
}

/**
 * 采样点的净空，阈值附近走精确值。
 *
 * 栅格值取的是「最近格中心」的距离，对真正的采样点最多乐观半格对角线（≈0.1m）。
 * 远离阈值时用栅格值即可（便宜，且近远关系不会翻转）；一旦落在阈值附近这条
 * 0.1m 的模糊带里，就按 AABB 真算——否则拉绳会放出「擦着墙角过去」的直线。
 */
function sampleClearance(x: number, z: number, g: NavGrid, minClear: number): number {
  const gc = clearanceAt(x, z, g);
  if (gc > minClear + g.cell) return gc;
  let best = Infinity;
  for (const a of g.obstacles) {
    const d = rectDistance(x, z, a);
    if (d < best) best = d;
  }
  return best;
}

/**
 * 两点之间沿直线走是否处处够净空。
 *
 * 拉绳平滑靠它判定"能不能抄近道"：只在不会把角色推到障碍跟前时才抄，
 * 否则宁可保留栅格折线——平滑的目的是去掉锯齿，不是换取更贴墙的路线。
 */
function segmentClear(
  ax: number, az: number, bx: number, bz: number,
  g: NavGrid, minClear: number, avoid: AvoidCircle[] = []
): boolean {
  const d = Math.hypot(bx - ax, bz - az);
  // 步长取 cell/7：折线可能只是「擦过」某个被占格的角，采样太稀会漏掉这种擦角，
  // 平滑出来的直线就会真的切进墙里——实测 cell*0.4 的步长漏过门洞旁的墙角格。
  const steps = Math.max(1, Math.ceil(d / (g.cell / 7)));
  for (let s = 1; s < steps; s++) {
    const t = s / steps;
    const x = ax + (bx - ax) * t, z = az + (bz - az) * t;
    if (sampleClearance(x, z, g, minClear) < minClear) return false;
    /**
     * 其他角色也要算进"不能穿"：拉绳只认静态障碍的话，会把好不容易绕开人的
     * 折线又拉成一条直线从对方身上穿过去（A* 的绕行代价等于白算）。
     * 门槛取 0.8×排斥半径 = 0.52m，正好是两个角色硬半径之和。
     */
    for (const o of avoid) {
      if (Math.hypot(x - o.x, z - o.z) < o.r * 0.8) return false;
    }
  }
  return true;
}

/**
 * 拉绳平滑：把一串栅格点压成尽量少的直线段。
 *
 * 栅格 A* 出来的是逐格拐点的折线，照着走会一路「一格一抖」；压平后角色是真沿直线走，
 * 也就不会在拐角处把身体甩进墙里。窗口 40 格封顶，避免长路径退化成 O(n²)。
 */
function stringPull(pts: NavPoint[], g: NavGrid, avoid: AvoidCircle[] = []): NavPoint[] {
  if (pts.length <= 2) return pts;
  const out: NavPoint[] = [pts[0]];
  let i = 0;
  while (i < pts.length - 1) {
    const limit = Math.min(pts.length - 1, i + 40);
    let j = limit;
    for (; j > i + 1; j--) {
      if (segmentClear(pts[i].x, pts[i].z, pts[j].x, pts[j].z, g, g.radius, avoid)) break;
    }
    out.push(pts[j]);
    i = j;
  }
  return out;
}

function worldToGrid(x: number, z: number, g: ReturnType<typeof buildGrid>) {
  const i = Math.floor((x - g.x0) / g.cell);
  const j = Math.floor((z - g.z0) / g.cell);
  return { i: Math.max(0, Math.min(g.res - 1, i)), j: Math.max(0, Math.min(g.resZ - 1, j)) };
}

function gridToWorld(i: number, j: number, g: ReturnType<typeof buildGrid>) {
  return {
    x: g.x0 + (i + 0.5) * g.cell,
    z: g.z0 + (j + 0.5) * g.cell
  };
}

const NEI = [[1, 0], [-1, 0], [0, 1], [0, -1]] as const;

/**
 * A* 找路径，返回世界坐标点列表（含终点，不含起点）；不可达返回 null。
 *
 * avoid = 其他角色的动态排斥圆：越靠近代价越高，于是路会主动绕开人。
 * 绕不开时（1.2m 走廊里两个人头对头，几何上就错不开）A* 照样给一条路，
 * 由上层状态机的让行规则来裁决谁先过——规划层不负责解决死锁。
 */
export function findPath(
  from: NavPoint,
  to: NavPoint,
  g: ReturnType<typeof buildGrid>,
  avoid: AvoidCircle[] = []
): NavPoint[] | null {
  const a = worldToGrid(from.x, from.z, g);
  const b = worldToGrid(to.x, to.z, g);
  if (g.blocked[b.i][b.j]) return null;
  if (g.blocked[a.i][a.j] && (a.i !== b.i || a.j !== b.j)) {
    // 起点在障碍里：先尝试往最近可走格走一步
    for (const [di, dj] of NEI) {
      const ni = a.i + di, nj = a.j + dj;
      if (ni >= 0 && ni < g.res && nj >= 0 && nj < g.resZ && !g.blocked[ni][nj]) {
        a.i = ni; a.j = nj;
        break;
      }
    }
  }
  const key = (i: number, j: number) => i * g.resZ + j;
  /** 一步的代价 = 1 格 + 净空不足的罚金 + 靠近其他角色的罚金；启发式仍按 1 估（下界）。 */
  const stepCost = (i: number, j: number) => {
    let c = 1 + CLEARANCE_WEIGHT * Math.max(0, SOFT_CLEARANCE - g.clearance[key(i, j)]);
    if (avoid.length) {
      const w = gridToWorld(i, j, g);
      for (const o of avoid) {
        const d = Math.hypot(w.x - o.x, w.z - o.z);
        if (d < o.r) c += PEER_WEIGHT * (o.r - d);
      }
    }
    return c;
  };
  const open: number[] = [key(a.i, a.j)];
  const came: Map<number, number> = new Map();
  const gScore: Map<number, number> = new Map();
  gScore.set(key(a.i, a.j), 0);
  const fScore: Map<number, number> = new Map();
  fScore.set(key(a.i, a.j), Math.abs(a.i - b.i) + Math.abs(a.j - b.j));
  while (open.length) {
    let bestIdx = 0;
    let bestF = Infinity;
    for (let k = 0; k < open.length; k++) {
      const f = fScore.get(open[k]) ?? Infinity;
      if (f < bestF) { bestF = f; bestIdx = k; }
    }
    const cur = open.splice(bestIdx, 1)[0];
    const ci = Math.floor(cur / g.resZ), cj = cur % g.resZ;
    if (ci === b.i && cj === b.j) {
      const cells: NavPoint[] = [];
      let p = cur;
      while (p !== key(a.i, a.j)) {
        const pi = Math.floor(p / g.resZ), pj = p % g.resZ;
        const w = gridToWorld(pi, pj, g);
        cells.push({ x: w.x, z: w.z });
        const prev = came.get(p);
        if (prev === undefined) break;
        p = prev;
      }
      cells.reverse();
      // 拉绳只压栅格段：终点是布局给的真实坐标（可能贴着家具），不参与平滑判定。
      const smoothed = stringPull([{ x: from.x, z: from.z }, ...cells], g, avoid);
      smoothed.push({ x: to.x, z: to.z });
      return smoothed;
    }
    for (const [di, dj] of NEI) {
      const ni = ci + di, nj = cj + dj;
      if (ni < 0 || ni >= g.res || nj < 0 || nj >= g.resZ) continue;
      if (g.blocked[ni][nj]) continue;
      const nk = key(ni, nj);
      const tentG = (gScore.get(cur) ?? Infinity) + stepCost(ni, nj);
      if (tentG < (gScore.get(nk) ?? Infinity)) {
        came.set(nk, cur);
        gScore.set(nk, tentG);
        fScore.set(nk, tentG + Math.abs(ni - b.i) + Math.abs(nj - b.j));
        if (!open.includes(nk)) open.push(nk);
      }
    }
  }
  return null;
}
