/**
 * 简易导航栅格 v2：布局驱动。
 *
 * 地面是套房的 L 外接矩形，障碍 = 外墙（只留落地门洞，窗洞补回实体）
 * + 内墙（按"落地洞口"断开，door/glass/pass 都可走）+ 阳台栏杆 + 家具 AABB，
 * 全部按角色半径膨胀。
 *
 * 之所以不直接用 rapier：
 * - 地面是 11×8.5 的简单矩形，74×57 栅格完全够用
 * - 障碍是几十个 AABB，膨胀一次就行
 * - 直走 + 简单 A* 比物理引擎便宜一个数量级
 * - 项目里 rapier 依赖在但从来没用过——P0 阶段不引入新的失败面
 */

export type AABB = { minX: number; maxX: number; minZ: number; maxZ: number };
export type NavPoint = { x: number; z: number };

const CHARACTER_RADIUS = 0.18;
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
   * 只认"能走过去"的落地洞口：door（门）/ pass（门洞）/ glass（落地玻璃门）。
   * window 永远补回实体——角色不会从窗台走出去。
   *
   * 外墙以前一律不看洞口（由调用方传 allowGaps = !exterior），于是客厅 /
   * 餐厅通往阳台的落地玻璃门被当成实心墙，阳台成了一个孤立区：check-layout
   * 早已按 kind 放行，运行时却走不过去，两个校验口径不一致。
   * 现在统一由 kind 判断，外墙的门照常通行、窗照常封死。
   * 角色会不会因此跑到房子外面由栅格边界兜底：grid 只覆盖
   * bounds = 建筑 + 阳台的外廓，门外那几格是死胡同，走不出去。
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

/** 障碍栅格预计算一次。分辨率按"目标格宽 ~0.15m"从布局算出。 */
export function buildGrid(obstacles: AABB[], bounds: { x0: number; x1: number; z0: number; z1: number }) {
  const CELL_TARGET = 0.15;
  const resX = Math.max(8, Math.ceil((bounds.x1 - bounds.x0) / CELL_TARGET));
  const resZ = Math.max(8, Math.ceil((bounds.z1 - bounds.z0) / CELL_TARGET));
  const cell = Math.max((bounds.x1 - bounds.x0) / resX, (bounds.z1 - bounds.z0) / resZ);

  const inflated = obstacles.map(a => inflate(a, CHARACTER_RADIUS));
  const blocked: boolean[][] = [];
  for (let i = 0; i < resX; i++) {
    blocked[i] = [];
    for (let j = 0; j < resZ; j++) {
      const x = bounds.x0 + (i + 0.5) * cell;
      const z = bounds.z0 + (j + 0.5) * cell;
      blocked[i][j] = inflated.some(a => inAABB({ x, z }, a));
    }
  }
  return { blocked, res: resX, resZ, cell, halfW: (bounds.x1 - bounds.x0) / 2, halfD: (bounds.z1 - bounds.z0) / 2, x0: bounds.x0, z0: bounds.z0 };
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

/** A* 找路径，返回世界坐标点列表（含终点，不含起点）；不可达返回 null。 */
export function findPath(
  from: NavPoint,
  to: NavPoint,
  g: ReturnType<typeof buildGrid>
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
      const path: NavPoint[] = [];
      let p = cur;
      while (p !== key(a.i, a.j)) {
        const pi = Math.floor(p / g.resZ), pj = p % g.resZ;
        const w = gridToWorld(pi, pj, g);
        path.push({ x: w.x, z: w.z });
        const prev = came.get(p);
        if (prev === undefined) break;
        p = prev;
      }
      path.reverse();
      // 路径点直接就是终点之后的格子中心；终点用真实目标。
      path.push({ x: to.x, z: to.z });
      return path;
    }
    for (const [di, dj] of NEI) {
      const ni = ci + di, nj = cj + dj;
      if (ni < 0 || ni >= g.res || nj < 0 || nj >= g.resZ) continue;
      if (g.blocked[ni][nj]) continue;
      const nk = key(ni, nj);
      const tentG = (gScore.get(cur) ?? Infinity) + 1;
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
