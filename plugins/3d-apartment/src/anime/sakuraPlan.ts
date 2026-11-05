/**
 * 樱町主视轴的坐标系契约（R2 第一块）。
 *
 * 概念图 1 的三层是「近景低层街屋 → 中景木格栅公寓 → 远景东京天际线」。
 * 旧场景的 40 家手作店散在 37 个地块上，公寓正南 z≈8..30 是一整片空地，
 * 站在门口只有一面 62m 长墙，读不出街巷纵深。这里先把主视轴上的街墙坐标
 * 收成唯一真源，由 sakuraTown 消费；旧坐标（exterior.ts 的 APT_X0/X1、
 * urbanStreets.ts 的 CITY_LOTS）暂时留作 legacy，逐块迁移、每轮用四机位
 * 基线 A/B，全部迁完再清理。
 *
 * 只放有真实消费者的量：契约里不留"以后会用到"的常量。
 */
export interface Frontage {
  x: number;
  z: number;
  height: number;
}

export const MAIN_FRONTAGE = {
  /** 街墙基准线：公寓南面 z=6.3，往外约 21.7m。 */
  z: 28.0,
  /** 店屋面宽 3.24m，留 0.16m 净缝，成连续街墙而不是一排孤立小房子。 */
  pitch: 3.4,
  originX: 0,
  count: 8,
} as const;

/**
 * 主视轴上的 8 个门面。每个坐标都按店屋 AABB
 * （[x-1.72, x+1.72] × [z-0.62, z+6.7]）对场景既有 1729 个碰撞盒核过账，
 * 零相交。店屋正面朝 -z，也就是朝公寓：从门口和阳台能读到招牌与橱窗。
 */
export const FRONTAGES: readonly Frontage[] = Array.from(
  { length: MAIN_FRONTAGE.count },
  (_, i) => ({
    x: MAIN_FRONTAGE.originX + i * MAIN_FRONTAGE.pitch,
    z: MAIN_FRONTAGE.z,
    height: 5.55 + (i % 4) * 0.22,
  }),
);

/**
 * 街墙前的行道樱。按树床 1.1×1.0m 的 AABB 对既有碰撞盒核过账（零相交），
 * 刻意避开书店后墙那一段（x≈13..19 全被占）。由 sakuraTown 的 treePositions
 * 消费：树冠实例场、落花场、Blender 干场都按数组长度自动扩容，不用另接线。
 */
export const ALLEY_TREES: ReadonlyArray<readonly [number, number]> = [
  [1.7, 24.5], [8.5, 24.5], [11.5, 24.5], [22.1, 24.5],
];

/* ============================================================================
 * 公寓体量（R2 整体重排的坐标唯一真源）
 * ========================================================================== */

/**
 * 住宅体量已收到 37.2m / 3 户（203 仍是中户、12 室与家具热点一点没动）。
 * UNITS / PARTITIONS / W / APT_WALL_BOXES、STRS 楼梯与楼梯区 30 处细节、
 * liftMass、电梯塔（整塔随 x0 平移）、公寓公园、203 索引、户号、表札、
 * cityDressing 的 keepOut，以及 apartmentArchitecture 的山墙/屋顶亭/柱子
 * 全部从这里派生。
 *
 * **1F 商业裙房刻意保留 62m**：apartmentPodium 是一整片按 62m 铺满的大堂
 * （咖啡吧、图书馆、水吧、62 列铺装），家具都是固定尺寸，等比压缩会互相
 * 穿插。住宅收窄、裙房不收，正好形成「低层商业基座 + 收窄木格栅住宅」的
 * 退台，也贴概念图 1 的近景低层 / 中景公寓层次。露出的两段屋面在
 * apartmentArchitecture 里做成屋顶庭园，不是空白残骸。
 */
export const APARTMENT_PLAN = {
  x0: -18.6,
  x1: 18.6,
  /** 每层户数（含 203 中户），等宽划分。 */
  unitCount: 3,
  /** 203 在 UNITS 里的下标；改成 3 户时居中 = 1。 */
  ownUnitIndex: 1,
  /** 203 自家户的 x 范围，必须与 dormLayout.room.bounds 一致。 */
  ownUnitX0: -6.2,
  ownUnitX1: 6.2,
} as const;

export const APARTMENT_WIDTH = APARTMENT_PLAN.x1 - APARTMENT_PLAN.x0;

/** 1F 商业裙房半宽（保留 62m 基座，不随住宅收缩）。屋顶庭园的落位边界。 */
export const PODIUM_HALF_WIDTH = 31;

/** 电梯塔几何是按 62m 时代的绝对坐标写的，平移量以旧西端为基准。 */
export const LEGACY_APT_X0 = -31;

/**
 * 户边界由面宽与户数等分派生。当前 62/5=12.4，中户恰好落在
 * [-6.2, 6.2] —— 与 203 的房间壳完全对齐，不是巧合，是当初按 203 宽度
 * 反推的面宽。收缩时若仍保持 5 户，中户会变窄并和 203 撞上，所以下面
 * 那条断言会在第一轮收缩时就喊出来，逼着一起改 unitCount / 203 布局。
 */
export function apartmentUnits(): Array<[number, number]> {
  const { x0, x1, unitCount } = APARTMENT_PLAN;
  const step = (x1 - x0) / unitCount;
  // 必须抹掉浮点尾巴：-31 + 2*12.4 会得到 -6.199999999999999，
  // 而 exterior.ts 里有 `px === -6.2` 这类严格相等判断（203 两侧戸境板跳过），
  // 差 1e-15 就会静默失效、多建两片隔板。1e-6 足够覆盖这套图纸精度。
  const snap = (v: number) => Math.round(v * 1e6) / 1e6;
  return Array.from({ length: unitCount }, (_, i) => [snap(x0 + i * step), snap(x0 + (i + 1) * step)]);
}

export function apartmentPartitions(): number[] {
  return [APARTMENT_PLAN.x0, ...apartmentUnits().map(([, right]) => right)];
}

/**
 * 户号（两位字符串）：ui 自西向东递增，门牌号自东向西递减，203 恒落在
 * ownUnitIndex 上。5 户 → 05/04/03/02/01（与旧版逐值相同），3 户 → 04/03/02。
 * 门牌、表札、住户名映射必须都走这里，否则收缩户数后三处会各说各话。
 */
export function unitNumber(ui: number): string {
  return String(3 + (APARTMENT_PLAN.ownUnitIndex - ui)).padStart(2, '0');
}

/** 结构不变量：203 的房间壳必须与派生出来的中户严丝合缝。 */
export function assertApartmentPlan(): void {
  const units = apartmentUnits();
  const middle = units[APARTMENT_PLAN.ownUnitIndex];
  if (!middle || Math.abs(middle[0] - APARTMENT_PLAN.ownUnitX0) > 1e-6 ||
      Math.abs(middle[1] - APARTMENT_PLAN.ownUnitX1) > 1e-6) {
    throw new Error(
      `Apartment plan mismatch: middle unit [${middle?.[0]}, ${middle?.[1]}] ` +
      `vs 203 [${APARTMENT_PLAN.ownUnitX0}, ${APARTMENT_PLAN.ownUnitX1}]. ` +
      `收缩面宽时必须同步改 unitCount 或 203 的房间布局。`
    );
  }
}
