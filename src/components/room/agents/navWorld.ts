/**
 * 跨场景导航 = 多层栅格 + 层间门户。
 *
 * 角色原来只能在 203 室内走，因为整张导航栅格就是按 203 的 bounds 建的。要让它
 * 走到外廊、一楼、街区、便利店、书店、地铁站，不能简单把 bounds 放大：室内要 0.15m
 * 精度才贴得住门框，街区 ±80m 用 0.15m 就是上百万格（构建要好几秒、启动直接卡住）。
 *
 * 所以按「层」切：每层一张栅格、各用自己的精度，层与层之间用**门户**连起来
 * （楼梯/电梯/门）。跨层寻路先过楼层图，再逐层用 A*，最后把各段拼成一条带 y 的路线。
 *
 * **栅格数据全部从碰撞表派生**——不另写一份几何。项目里「墙面渲染 / 导航栅格 /
 * FPS 碰撞同一数据源」是硬约定（collider.ts 头顶就写着），这里继续沿用：
 *   - floor 盒 → 该层的可走面（**没被任何 floor 覆盖的格子一律不可走**）
 *   - wall 盒 → 该层的障碍（按「是否挡人」的高度区间过滤）
 *   - ramp 盒 → 层间门户（自带 heightAt，上下楼沿坡面插值）
 * 手工只声明两样：**区域的切分方式**（哪几层、多大精度、裁到哪个范围）和
 * **门口的两个落脚点**（门的几何在渲染侧，导航只需知道门两侧各站哪）。
 */

import { buildGrid, findPath, type AABB, type NavGrid, type NavPoint } from './navGrid';

/** 与 collider.ts 的 Collider 同形（只用到这些字段，避免把 three 拖进纯逻辑模块）。 */
export type ColliderLike = {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
  kind?: 'wall' | 'floor' | 'ramp';
  source?: string;
  heightAt?: (x: number, z: number) => number | null;
};

export type LayerId = string;

export type NavLayer = {
  id: LayerId;
  /** 该层地面标高（角色站上去时的脚底高度）。 */
  y: number;
  bounds: { x0: number; x1: number; z0: number; z1: number };
  grid: NavGrid;
  /** 建这张栅格时用的障碍（诊断用：为什么某处走不过去，看它比看栅格快）。 */
  obstacles: AABB[];
};

/** 层间通道：stair 沿坡面走（有 heightAt），lift 走轿厢，door 是同层跨区域的门。 */
export type NavPortal = {
  id: string;
  kind: 'stair' | 'lift' | 'door';
  from: { layer: LayerId; x: number; z: number };
  to: { layer: LayerId; x: number; z: number };
  heightAt?: (x: number, z: number) => number | null;
  /** 通过代价（米）：楼梯比平地费劲、电梯要等，同距离优先走平路。 */
  cost: number;
  /** 两侧落脚点是否都落在各自层的可走区里。false = 这条边是死的（落脚点写错层的典型症状）。 */
  walkable?: boolean;
};

/** 路线的一段：在某层内从当前位置走到某个门口/终点。 */
export type RouteLeg = {
  layer: LayerId;
  y: number;
  path: NavPoint[];
  /** 走完这一段之后要穿过的门户（最后一段没有）。 */
  portal?: NavPortal;
};

export type NavWorld = {
  layers: NavLayer[];
  portals: NavPortal[];
  layer(id: LayerId): NavLayer | undefined;
  /** 从 (x,z,y) 反查所在层。 */
  layerAt(x: number, z: number, y: number): NavLayer | undefined;
  /** 跨层寻路：返回若干段，逐段走完即到达。不可达返回 null。 */
  route(req: { from: { layer: LayerId; x: number; z: number }; to: { layer: LayerId; x: number; z: number } }): RouteLeg[] | null;
};

/** 一块水平可走面。 */
type Plate = { x0: number; x1: number; z0: number; z1: number };

/** 区域切分：同标高的不同场所各建一张栅格（街区要粗、室内要细）。 */
type AreaSpec = {
  id: LayerId;
  /** 该层地面标高。 */
  y: number;
  /** 地面标高带：floor 盒顶面落在这段里才算该区域的地板。 */
  yBand: [number, number];
  /** 目标格宽（米）。 */
  cell: number;
  /** 只吃这个水平范围内的地板（同标高的室内外靠它分开）。 */
  clip?: { x0: number; x1: number; z0: number; z1: number };
  /**
   * 手工指定的可走面。
   *
   * 什么时候需要：店铺地面是用 `buildBoxColliders` 建盒子进去的，而那家伙把**所有**
   * 盒都标成 `wall`（它只负责"挡人"，不区分地板）——于是这些区域一份 floor 盒都没有，
   * 自动推导推导不出来。这类场所就用这块兜底。
   */
  plate?: Plate;
  /** 该层不用碰撞盒做障碍（改由 extraObstacles 提供），用于 203 这种「导航口径与 FPS 口径不同」的区域。 */
  noSolids?: boolean;
  /**
   * 该层的角色半径。默认用全局 CHARACTER_RADIUS；
   * 店内的货架缝、柜台前的过道是按第一人称玩家（0.15）的尺度留的，
   * 角色按 0.26 走进去会被膨胀盒整块封死，于是这些空间按玩家口径放宽。
   */
  radius?: number;
  /** 手工补的障碍。 */
  extraBlocks?: AABB[];
};

/**
 * 每层的「允许区」：只在这几块矩形里才可走。
 *
 * 没给 allow 的层用「有地板才可走」自动推导（对室内/店铺够用）；
 * 外廊那层必须给——2F 的楼板是横跨整栋楼的整片（x −31~31），自动推导会把
 * 邻居户室内一并判成可走，角色就串门去了。
 */
export type AllowMap = Record<LayerId, Plate[]>;

/**
 * 区域表。
 *
 * 标高都是场景里的真实值：公寓 2F=3.4（203 与北外廊、电梯厅、东端楼梯平台同层）、
 * 1F 大堂=0.062、街区=0（人行道 0.028 忽略，脚底 3cm 看不出来）、店铺前厅≈0.1。
 */
const AREAS: AreaSpec[] = [
  /**
   * 203 室内。**障碍不用碰撞盒**——这条是踩出来的：
   * 渲染侧那份碰撞盒是为了"第一人称不穿模"而建的（墙有厚度、门框、门槛、
   * 家具的真实外廓全都算），拿它当导航障碍，203 里的可走空间会被切碎，
   * 连客厅沙发前都站不下人。项目本来就有两套口径：导航吃 dormLayout 的标称
   * 尺寸（见 CODE_WIKI「碰撞三同源」），FPS 吃真实盒。这里沿用标称那份。
   */
  // clip 收在 203 的户型框上：2F 楼板是横跨整栋楼的整片，不收边就把邻居户也算进来
  { id: 'apt2', y: 3.4, yBand: [3.25, 3.75], cell: 0.15, noSolids: true, clip: { x0: -6.2, x1: 6.2, z0: -5.9, z1: 6.3 } },
  /**
   * 2F 公共部分：北外廊 + 电梯厅(2F) + 东端楼梯平台。
   * 与 203 分开建是因为障碍来源不同（这里必须用碰撞盒：外廊栏杆、电梯门套、
   * 楼梯平台挡板都只在渲染侧有）。两侧用 203 入户门当门户连起来。
   */
  { id: 'apt2out', y: 3.4, yBand: [3.25, 3.75], cell: 0.15, clip: { x0: -36.9, x1: 37.9, z0: -7.9, z1: 4.75 } },
  // 1F 大堂 + 电梯厅(1F) + 咖啡吧/书房区，南门通街
  { id: 'hall1', y: 0.062, yBand: [-0.05, 0.25], cell: 0.15, clip: { x0: -37, x1: 31.2, z0: -7.3, z1: 4.78 } },
  // 街区：整片路面，0.5m 粗栅格（±80m 用 0.15 就是上百万格）
  { id: 'street', y: 0, yBand: [-0.05, 0.25], cell: 0.5, clip: { x0: -80, x1: 80, z0: -78, z1: 83 } },
  // 便利店前厅（店门内 2.4m 深；地面盒被建成了 wall，所以可走面手工给）
  { id: 'cvs', y: 0.1, yBand: [0.05, 0.25], cell: 0.15, radius: 0.13, clip: { x0: -4.5, x1: 6.7, z0: 13.9, z1: 16.5 },
    plate: { x0: -4.4, x1: 6.6, z0: 14.0, z1: 16.4 } },
  // 书店店内（进深 9m，三排书架）
  { id: 'book', y: 0.02, yBand: [-0.05, 0.25], cell: 0.15, radius: 0.15, clip: { x0: 12.6, x1: 19.8, z0: 14.9, z1: 24.2 } },
];

/** 手工门户：门两侧各站哪。几何在渲染侧，导航只需要落脚点。 */
type DoorSpec = {
  id: string;
  kind?: 'door' | 'lift';
  from: { layer: LayerId; x: number; z: number };
  to: { layer: LayerId; x: number; z: number };
  cost?: number;
};

const DOORS: DoorSpec[] = [
  // 203 入户门（北墙 z=−5.9，门洞 x∈[4.5,5.4]）：203 室内那张栅格 ↔ 公共外廊那张
  { id: 'apt203-door', from: { layer: 'apt2', x: 4.95, z: -5.3 }, to: { layer: 'apt2out', x: 4.95, z: -6.5 } },
  // 公寓 1F 南门（z=4.70，门洞 x∈[-6.04,6.04]）
  { id: 'apt-south-door', from: { layer: 'hall1', x: 0, z: 4.4 }, to: { layer: 'street', x: 0, z: 5.6 } },
  // 便利店自动门（z=14.0，门洞 x∈[-0.11,1.66]）
  // 落脚点必须在门口那条薄带里：店内货架到门只有 0.5m，往深了放会被货架的膨胀盒吃掉
  { id: 'cvs-door', from: { layer: 'street', x: 0.78, z: 13.6 }, to: { layer: 'cvs', x: 0.78, z: 14.3 } },
  // 书店门（z=15.0，门洞 x∈[17.90,18.80]）
  { id: 'book-door', from: { layer: 'street', x: 18.35, z: 14.6 }, to: { layer: 'book', x: 18.35, z: 15.4 } },
  /**
   * 电梯：2F 电梯厅 ↔ 1F 电梯厅。
   * 层名必须写 **apt2out**——电梯厅在 x=−34.6，而 apt2 是 203 室内（x −6.2~6.2），
   * 挂到 apt2 上落脚点就在栅格外，这条边会静默失效（寻路永远绕楼梯）。
   */
  { id: 'lift', kind: 'lift', from: { layer: 'apt2out', x: -34.6, z: -0.3 }, to: { layer: 'hall1', x: -34.6, z: -0.3 }, cost: 19 },
];

function overlap1d(a0: number, a1: number, b0: number, b1: number) {
  return Math.min(a1, b1) - Math.max(a0, b0) > 0.01;
}

/** 从碰撞表派生整个导航世界。 */
export function buildNavWorld(colliders: ColliderLike[], opts?: {
  allow?: AllowMap;
  /** 每层的额外障碍（203 用 dormLayout 的标称尺寸）。 */
  extraObstacles?: Record<LayerId, AABB[]>;
}): NavWorld {
  const floors = colliders.filter(c => c.kind === 'floor');
  const ramps = colliders.filter(c => c.kind === 'ramp');
  const solids = colliders.filter(c => c.kind !== 'floor' && c.kind !== 'ramp');

  const layers: NavLayer[] = [];

  function pickLayer(list: NavLayer[], x: number, z: number, y: number) {
    return list.find(l =>
      x >= l.bounds.x0 - 1.5 && x <= l.bounds.x1 + 1.5 &&
      z >= l.bounds.z0 - 1.5 && z <= l.bounds.z1 + 1.5 &&
      Math.abs(l.y - y) < 1.2
    );
  }

  for (const spec of AREAS) {
    const clip = spec.clip ?? { x0: -1e4, x1: 1e4, z0: -1e4, z1: 1e4 };
    // 该区域的可走面：手工给的优先，否则从 floor 盒推导
    const plates: Plate[] = spec.plate ? [spec.plate] : floors
      .filter(f =>
        f.max.y >= spec.yBand[0] && f.max.y <= spec.yBand[1] &&
        overlap1d(f.min.x, f.max.x, clip.x0, clip.x1) &&
        overlap1d(f.min.z, f.max.z, clip.z0, clip.z1)
      )
      .map(f => ({ x0: f.min.x, x1: f.max.x, z0: f.min.z, z1: f.max.z }));
    if (!plates.length) continue;

    // 外接框 = 可走面并集，再夹到 clip
    const bounds = {
      x0: Math.max(clip.x0, Math.min(...plates.map(p => p.x0))),
      x1: Math.min(clip.x1, Math.max(...plates.map(p => p.x1))),
      z0: Math.max(clip.z0, Math.min(...plates.map(p => p.z0))),
      z1: Math.min(clip.z1, Math.max(...plates.map(p => p.z1))),
    };

    /**
     * 障碍 = 挡人的实体盒。
     * 高度过滤：与「地面往上 0.25~1.6m」相交才算挡路——门槛、地毯、半米高的台沿
     * 角色能迈过去，不该把路封死；2m 高的墙一定挡人。
     */
    const bandY0 = spec.y + 0.42;   // 迈步高度（STEP_UP=0.45）以下的不算障碍：便利店门口那道 0.39 高的门槛就是这么被误判成墙的
    const bandY1 = spec.y + 1.6;
    const obstacles: AABB[] = [...(opts?.extraObstacles?.[spec.id] ?? [])];
    for (const s of (spec.noSolids ? [] : solids)) {
      if (!overlap1d(s.min.y, s.max.y, bandY0, bandY1)) continue;
      if (!overlap1d(s.min.x, s.max.x, bounds.x0 - 1, bounds.x1 + 1)) continue;
      if (!overlap1d(s.min.z, s.max.z, bounds.z0 - 1, bounds.z1 + 1)) continue;
      obstacles.push({ minX: s.min.x, maxX: s.max.x, minZ: s.min.z, maxZ: s.max.z });
    }
    obstacles.push(...(spec.extraBlocks ?? []));

    const grid = buildGrid(obstacles, bounds, spec.radius, spec.cell);

    /**
     * 「有地板才可走」——这一步才是把角色关在合法范围内的关键。
     * 只靠障碍盒做不到：邻居单元的墙只有门洞一个开口、楼体外接框里也有大片空地，
     * 角色会走进别人家、或者飘在楼体内部。
     */
    const allow = opts?.allow?.[spec.id];
    const region = allow ?? plates;
    for (let i = 0; i < grid.res; i++) {
      for (let j = 0; j < grid.resZ; j++) {
        const x = grid.x0 + (i + 0.5) * grid.cell;
        const z = grid.z0 + (j + 0.5) * grid.cell;
        const ok = region.some(p =>
          x >= p.x0 - 0.05 && x <= p.x1 + 0.05 && z >= p.z0 - 0.05 && z <= p.z1 + 0.05
        );
        if (!ok) {
          grid.blocked[i][j] = true;
          grid.clearance[i * grid.resZ + j] = 0;
        }
      }
    }

    layers.push({ id: spec.id, y: spec.y, bounds, grid, obstacles });
  }

  /* ---------------- 门户 ---------------- */
  const portals: NavPortal[] = [];

  // 楼梯：每条 ramp 天生连接两个标高，直接用它的 heightAt。
  // 本项目的楼梯都是「沿 −x 上行」：坡底在 x 大的一侧、坡顶在 x 小的一侧。
  for (const r of ramps) {
    const midZ = (r.min.z + r.max.z) / 2;
    const botX = Math.max(r.min.x, r.max.x);
    const topX = Math.min(r.min.x, r.max.x);
    const botY = Math.min(r.min.y, r.max.y);
    const topY = Math.max(r.min.y, r.max.y);
    // 落脚点要留在坡面**之外**（坡外的平地），不然角色会站在半空
    const fromLayer = pickLayer(layers, botX + 0.5, midZ, botY + 0.1);
    const toLayer = pickLayer(layers, topX - 0.5, midZ, topY + 0.1);
    if (!fromLayer || !toLayer || fromLayer.id === toLayer.id) continue;
    portals.push({
      id: r.source ?? `stair@${botX.toFixed(1)}`,
      kind: 'stair',
      from: { layer: fromLayer.id, x: botX + 0.5, z: midZ },
      to: { layer: toLayer.id, x: topX - 0.5, z: midZ },
      heightAt: r.heightAt,
      cost: Math.abs(botX - topX) + 4 + (topY - botY),
    });
  }

  // 门 / 电梯：手工声明
  for (const d of DOORS) {
    if (!layers.find(l => l.id === d.from.layer) || !layers.find(l => l.id === d.to.layer)) continue;
    portals.push({ id: d.id, kind: d.kind ?? 'door', from: d.from, to: d.to, cost: d.cost ?? 1 });
  }

  /* ---------------- 查询 ---------------- */
  const layerById = (id: LayerId) => layers.find(l => l.id === id);

  /**
   * 门户图最短路。
   *
   * 节点 = 「每个门户的两侧点 + 起点 + 终点」，边有两种：同一门户的两侧互连
   * （权 = portal.cost，即穿过去），以及同层的任意两节点互连（权 = 直线距离，
   * 即走过去）。
   *
   * **关键是别只比门户自身的代价**：那样「19 的电梯」会输给「12.9 的楼梯」，
   * 因为纯层图看不见「走到楼梯口还得 170m」这件事——表现就是角色从二楼电梯厅
   * 出发去一楼电梯厅，却绕到东端楼梯再横穿整条街。把接近距离算进来才是真的近。
   * 节点十几个，完全图 Dijkstra 随便跑。
   */
  function planHops(
    from: { layer: LayerId; x: number; z: number },
    to: { layer: LayerId; x: number; z: number }
  ): NavPortal[] | null {
    type Node = { layer: LayerId; x: number; z: number; portal?: NavPortal };
    const nodes: Node[] = [
      { layer: from.layer, x: from.x, z: from.z },
      { layer: to.layer, x: to.x, z: to.z },
    ];
    for (const p of portals) {
      if (p.walkable === false) continue;
      nodes.push({ layer: p.from.layer, x: p.from.x, z: p.from.z, portal: p });
      nodes.push({ layer: p.to.layer, x: p.to.x, z: p.to.z, portal: p });
    }
    const n = nodes.length;
    const dist = new Array<number>(n).fill(Infinity);
    const prev = new Array<number>(n).fill(-1);
    const done = new Array<boolean>(n).fill(false);
    dist[0] = 0;
    for (let iter = 0; iter < n; iter++) {
      let u = -1, best = Infinity;
      for (let i = 0; i < n; i++) if (!done[i] && dist[i] < best) { best = dist[i]; u = i; }
      if (u < 0) break;
      done[u] = true;
      for (let v = 0; v < n; v++) {
        if (v === u || done[v]) continue;
        const pu = nodes[u].portal, pv = nodes[v].portal;
        let w = Infinity;
        if (pu && pu === pv) w = pu.cost;                        // 同一门户的两侧：穿过去
        else if (nodes[u].layer === nodes[v].layer) {            // 同层：走过去
          w = Math.hypot(nodes[u].x - nodes[v].x, nodes[u].z - nodes[v].z);
        }
        if (dist[u] + w < dist[v]) { dist[v] = dist[u] + w; prev[v] = u; }
      }
    }
    if (!Number.isFinite(dist[1])) return null;
    // 回溯：相邻两步落在同一门户的两侧 = 穿过它一次
    const chain: number[] = [];
    for (let cur = 1; cur >= 0; cur = prev[cur]) chain.push(cur);
    chain.reverse();
    const hops: NavPortal[] = [];
    for (let i = 0; i + 1 < chain.length; i++) {
      const a = nodes[chain[i]].portal, b = nodes[chain[i + 1]].portal;
      if (a && a === b) hops.push(a);
    }
    return hops;
  }

  for (const p of portals) {
    p.walkable = [p.from, p.to].every(pt => {
      const L = layers.find(l => l.id === pt.layer);
      if (!L) return false;
      const i = Math.floor((pt.x - L.grid.x0) / L.grid.cell), j = Math.floor((pt.z - L.grid.z0) / L.grid.cell);
      return !!L.grid.blocked[i] && !L.grid.blocked[i][j];
    });
  }

  return {
    layers,
    portals,
    layer: layerById,
    layerAt: (x, z, y) => pickLayer(layers, x, z, y),
    route({ from, to }) {
      const A = layerById(from.layer);
      const B = layerById(to.layer);
      if (!A || !B) return null;
      const hops = planHops(from, to);
      if (!hops) return null;

      const legs: RouteLeg[] = [];
      let curLayer = A;
      let cursor: NavPoint = { x: from.x, z: from.z };
      for (const p of hops) {
        // 当前层：走到门户在本层的那一侧
        const here = p.from.layer === curLayer.id ? p.from : p.to;
        const path = findPath(cursor, { x: here.x, z: here.z }, curLayer.grid);
        if (!path) return null;
        legs.push({ layer: curLayer.id, y: curLayer.y, path, portal: p });
        const there = p.from.layer === curLayer.id ? p.to : p.from;
        const next = layerById(there.layer);
        if (!next) return null;
        curLayer = next;
        cursor = { x: there.x, z: there.z };
      }
      const tail = findPath(cursor, { x: to.x, z: to.z }, curLayer.grid);
      if (!tail) return null;
      legs.push({ layer: curLayer.id, y: curLayer.y, path: tail });
      return legs;
    },
  };
}
