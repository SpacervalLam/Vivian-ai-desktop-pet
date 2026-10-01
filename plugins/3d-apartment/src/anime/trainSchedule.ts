/**
 * 桜町駅两条轨的行车时刻表 —— **纯函数**，不 import three、不碰 DOM。
 *
 * 为什么单独成文件：这条时刻表决定四件事，而每一件都得逐帧对账才有意义 ——
 *   ① 两列编组的位置（单向行车、到雾外重置）；
 *   ② 道口栏杆的关闭窗口；
 *   ③ 动态碰撞盒的位置；
 *   ④ `operatingState` 给预览页读的状态字。
 * 把它们留在 sakuraStation 里，就只能靠"跑一遍浏览器、把 group.position 采下来"
 * 来验 —— 而这个场景在 headless swiftshader 下只有 **0.4 fps**（实测：261s 采到 103
 * 帧），一个 84.7s 的周期要跑 40 分钟。抽出来之后，整周期的性质（单调性、跳变量、
 * 停站重合、编组不出轨、栏杆窗口）可以在 Node 里毫秒级全量核账，
 * 浏览器那边只需要验"接线对不对"（见 tmp/_check-schedule.ts 与 tmp/_probe-trainpair.mjs）。
 *
 * 几何量从车体布局派生，不写死：两辆车中心在 z=48 / 65，每辆半长 CAR_HL，
 * 两端再各留 COUPLER 给連結器与排障器。改车长只会动这一处。
 */

/* ---------------- 编组几何 ---------------- */
export const CAR_CENTRES = [48, 65] as const;
export const CAR_HL = 8;
export const COUPLER = 0.45;
/** 编组中心（停站时它落在站台正中偏北，与原版 offset=0 的位置逐位相同）。 */
export const CONSIST_C = (CAR_CENTRES[0] + CAR_CENTRES[1]) / 2;
/** 编组半长（含連結器外伸）。 */
export const CONSIST_HL = (CAR_CENTRES[1] - CAR_CENTRES[0]) / 2 + CAR_HL + COUPLER;
/** 停站时编组中心的 z。 */
export const STOP_C = CONSIST_C;

/* ---------------- 行车参数 ---------------- */
/**
 * 轨道半长（米）。**单点真相**：sakuraStation 铺轨（道砟 / 枕木 / 钢轨 / 接触网 /
 * 栅栏）用它，行程端点也由它派生。
 *
 * 取值理由见 sakuraStation 里那段长注释：晴天雾 near 85 / far 300，要玩家在任何
 * 可站立点都看不到断头，断头就得离每个可站立点 ≥300m。最苛刻的点是街面东北角
 * (80,-78)，代入得 H ≥ 377.9。取 440，留 62m 余量。
 */
export const TRACK_HALF = 440;
/**
 * 行程端点（编组**中心**的 |z| 上限）。
 *
 * 两个约束把夹在中间：
 *   · **下界（雾）**：跳变必须发生在全雾里，否则会看到编组凭空消失 / 凭空出现。
 *     编组最近的端面在 |z| = RUN_HALF - CONSIST_HL，它到活动范围最近点
 *     （z=95 与 z=-78）的距离必须 > 雾 far=300。
 *   · **上界（轨）**：编组整列都得压在钢轨上，否则会看到车头悬在轨道尽头之外。
 *     即 RUN_HALF + CONSIST_HL ≤ TRACK_HALF。
 * 取等号的上界：RUN_HALF = 440 - 16.95 = 423.05，此时最近端面在 |z|=406.1，
 * 到 z=95 有 311.1m、到 z=-78 有 328.1m，都 > 300 —— 两条约束同时满足，
 * 且没有任何余量浪费在"车头伸到轨外"上。
 * （改前是写死的 430：车头伸出轨端 6.95m。虽然那个位置在 318m 外、全雾里，
 *   肉眼绝对看不见，但"车头悬在轨道外"是个不该存在的状态。）
 */
export const RUN_HALF = TRACK_HALF - CONSIST_HL;
export const V_MAX = 22;    // m/s（79km/h）
export const ACC = 1.2;     // m/s²
export const DWELL = 9;     // 停站秒数

/* ---------------- 道口 ---------------- */
/** crossing-deck 的 z 区间（10.1 ∓ 3.6）。 */
export const ZX0 = 6.5, ZX1 = 13.7;
export const LEAD = 9;      // 预警提前量（秒）
export const CLEAR = 3.5;   // 尾部出清后延迟抬杆（秒）

/** 相位（秒）回卷到 [0, CYCLE)。 */
export const wrap = (t: number, cycle: number) => ((t % cycle) + cycle) % cycle;

/**
 * 从静止出发、走完 d 再停住的「时间 → 已走距离」曲线，附带反函数。
 * 够长是梯形（加速—匀速—减速），不够长自动退化成三角形（没有匀速段）。
 */
export function profile(d: number, vmax: number, acc: number) {
  const trapezoid = d >= (vmax * vmax) / acc;
  const ta = trapezoid ? vmax / acc : Math.sqrt(d / acc);
  const da = trapezoid ? (vmax * vmax) / (2 * acc) : d / 2;
  const vp = trapezoid ? vmax : acc * ta;
  const tc = trapezoid ? (d - 2 * da) / vmax : 0;
  const total = 2 * ta + tc;
  const at = (t: number) => {
    if (t <= 0) return 0;
    if (t >= total) return d;
    if (t < ta) return 0.5 * acc * t * t;
    if (t < ta + tc) return da + vp * (t - ta);
    const r = total - t;
    return d - 0.5 * acc * r * r;
  };
  /** at 的反函数：走完 s 米要多少秒。at 单调，逐段解即可。 */
  const inv = (s: number) => {
    if (s <= 0) return 0;
    if (s >= d) return total;
    if (s <= da) return Math.sqrt((2 * s) / acc);
    if (s <= d - da) return ta + (s - da) / vp;
    return total - Math.sqrt((2 * (d - s)) / acc);
  };
  return { d, vmax, acc, total, at, inv, trapezoid, peak: vp };
}

export type Leg = ReturnType<typeof profile>;

/** 北端雾外 → 站台。 */
export const LEG_IN: Leg = profile(STOP_C + RUN_HALF, V_MAX, ACC);
/** 站台 → 南端雾外。 */
export const LEG_OUT: Leg = profile(RUN_HALF - STOP_C, V_MAX, ACC);
export const CYCLE = LEG_IN.total + DWELL + LEG_OUT.total;
/**
 * 东轨那列**相对"同相停站"**的错开量（秒）。
 *
 * 前身是"两列同相停站"（偏移 = LEG_OUT.total - LEG_IN.total），注释给的两条理由
 * 是"交会站就该两列同时进站"和"错开半周期栏杆几乎全程不放（算过 92%）"。
 * **第二条是错的**：把 PHASE_E 当自变量整周期扫描（tmp/_phase-sweep.mjs）之后，
 * 错开半周期（42.04s）的关闭占比是 38.5%，与同相**基本一样**，不存在 92%。
 * 那条注释把一个没算过的数字当成了约束，于是"两列同时到站"被锁死在设计里。
 *
 * 用户要求两列不要同时到站，于是重新扫了一遍。自变量是 **PHASE_E 本身**（不是增量），
 * 三项指标：停站重叠 / 到达间隔 / 道口关闭占比 ——
 *    δ =  0.000（同相）  重叠 9.000s  间隔  0.000s  关闭 39.25%（放杆 2 次）
 *    δ = 21.000          重叠 0.000s  间隔 21.000s  关闭 26.7%   压停站 13.4%  ← 差
 *    δ = 26.127          重叠 0.000s  间隔 26.127s  关闭 19.62%（放杆 1 次）  ← 取它
 *    δ = 42.026          重叠 0.000s  间隔 42.026s  关闭 38.53%（放杆 1 次）
 *
 * 为什么是 26.127 而不是别的：两次道口窗口的长度都是 16.5s，**两个窗口在时间上重合时
 * 并集最短**（16.5s 而不是 33s）—— 一次放杆正好覆盖两条轨的两次穿越，关闭占比从
 * 39.25% 砍到 19.62%，玩家有 80% 的时间可以过道口。
 * 窗口重合 ⇔ 两次穿越时刻相同 ⇔ PHASE_E = eastWin[0] - westWin[0] = 20.951。
 * 这是个**平台不是刀尖**：PHASE_E 偏 ±0.5s 以内关闭占比仍 ≈20%（并集 = 16.5 + |偏差|）。
 * 核账脚本（tmp/_check-schedule.ts ②）会断言两个窗口确实重合 —— 改 LEAD / 道口几何
 * 之后要重新取这个值。
 * δ=21 之所以不能要：两个窗口只是**部分**重叠，并集反而被拉长到 22.4s，而且并集尾部
 * 正好压在另一列的停站上（压停站 13.4%，而 δ=26.127 只有 2.9%）。
 *
 * 残留的"压停站 2.9%"（≈2.44s）是结构性的、且不是缺陷：2.44s = LEAD(9s) − 从站台
 * 出发到车头抵道口的时间(6.564s)，即栏杆比**发车**早 2.44s 落下。这 2.44s 里另一列
 * 正停在站台上 —— 栏杆是为"即将通过的那一列"而落，站台上那列本来就要开走，
 * 观感上是路口提前预警，不是白拦。（"白放"是另一回事：栏杆放下时若两列都离道口
 * >200m 才算白放，实测 0。）
 */
const STAGGER = 26.127;
/** 错开量。核账脚本（tmp/_check-schedule.ts）要用它印日志，所以导出。 */
export { STAGGER };
/**
 * 东轨那列的相位偏移 = "让两列同相停站"的量 + 错开量。
 * 西轨那列的停站相位是 LEG_IN.total，东轨那列是 LEG_OUT.total，差即同相量。
 */
export const PHASE_E = LEG_OUT.total - LEG_IN.total + STAGGER;
/**
 * 载入时的相位：西轨那列停在站台上（东轨那列此刻在区间里跑），
 * 所以自检行读到的第一眼仍然是「列车停站」。
 */
export const INITIAL_PHASE = LEG_IN.total;

/** 西轨（x=86）编组中心 z：**只向 +z 走**（自北向南）。 */
export const westC = (t: number) => t < LEG_IN.total ? -RUN_HALF + LEG_IN.at(t)
  : t < LEG_IN.total + DWELL ? STOP_C
  : STOP_C + LEG_OUT.at(t - LEG_IN.total - DWELL);
/** 东轨（x=91）编组中心 z：**只向 -z 走**（自南向北）。同一张表的镜像。 */
export const eastC = (t: number) => t < LEG_OUT.total ? RUN_HALF - LEG_OUT.at(t)
  : t < LEG_OUT.total + DWELL ? STOP_C
  : STOP_C - LEG_IN.at(t - LEG_OUT.total - DWELL);

/**
 * 道口栏杆的关闭窗口，按**相位**给。
 *
 * 不按"列车离道口还有多远"给：停站位置离道口铺面净空只有 24.85m，几何上
 * "已经很近"会一直成立，栏杆就会在停站那 9 秒里全程压着；改成"还有多少秒"又会被
 * 停站本身拖长。相位窗口是这张时刻表的直接推论，没有这个歧义。
 *
 * 西轨：车头面 = C + CONSIST_HL，先碰到 ZX0；车尾面 = C - CONSIST_HL，最后离开 ZX1。
 */
export const westWin: [number, number] = [
  LEG_IN.inv(ZX0 - CONSIST_HL + RUN_HALF) - LEAD,
  LEG_IN.inv(ZX1 + CONSIST_HL + RUN_HALF) + CLEAR,
];
/**
 * 东轨反向：车头面（= C - CONSIST_HL）先碰到 ZX1，车尾面最后离开 ZX0。
 * 两次穿越都在**停站之后**那条腿（LEG_IN）上，所以相位要加 LEG_OUT.total + DWELL。
 */
export const eastWin: [number, number] = [
  LEG_OUT.total + DWELL + LEG_IN.inv(STOP_C - ZX1 - CONSIST_HL) - LEAD,
  LEG_OUT.total + DWELL + LEG_IN.inv(STOP_C - ZX0 + CONSIST_HL) + CLEAR,
];

const smooth = (v: number) => { v = Math.min(1, Math.max(0, v)); return v * v * (3 - 2 * v); };
/** 相位 t 落在（允许跨 0 的）窗口 [a,b] 里的放下量 0..1；窗口之外返回 0。
 *  两端各留 1.6s 平滑段，栏杆不会瞬移。 */
export const windowWeight = (t: number, a: number, b: number) => {
  const span = wrap(b - a, CYCLE);
  const rel = wrap(t - a, CYCLE);
  if (rel > span) return 0;
  const edge = Math.min(1.6, span / 2);
  return Math.min(smooth(rel / edge), smooth((span - rel) / edge));
};
/** 两列合起来的栏杆放下量。 */
export const closureAt = (tW: number, tE: number) =>
  Math.max(windowWeight(tW, westWin[0], westWin[1]), windowWeight(tE, eastWin[0], eastWin[1]));

/** 某一列是否在停站（相位在它自己那条停站平台上）。 */
export const isDwelling = (tW: number, tE: number) =>
  (tW >= LEG_IN.total && tW < LEG_IN.total + DWELL) ||
  (tE >= LEG_OUT.total && tE < LEG_OUT.total + DWELL);
