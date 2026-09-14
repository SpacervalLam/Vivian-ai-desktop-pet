/**
 * 时间比例尺（带间隙压缩）
 *
 * 把按时间排序的骨架点映射为纵向布局：相邻点间距 = clamp(间隔秒 * K, MIN_SPACING, MAX_SPACING)，
 * 累加得到断点表。同日内的密集节点取最小间距，数月的大间隙取最大间距而不产生空洞。
 * 朝向为最新在上：最新点落在 topPadding 处，最旧点在底部。
 */

export const TIME_SCALE_K = 0.02; // 每秒像素数（约 72px/小时）
export const MIN_SPACING = 56; // 相邻点最小间距（≥最大节点 36 + 标签 17，防同侧重叠）
export const MAX_SPACING = 140; // 相邻点最大间距（大间隙不产生空洞）

export interface Breakpoint {
  ts: number; // 时间戳（毫秒）
  yOff: number; // 相对最旧点的累积偏移（px），严格递增
}

export interface TimeScale {
  breakpoints: Breakpoint[];
  totalContent: number;
  svgHeight: number;
  yStart: number; // 最旧点（底部）的 svg y 坐标
  minTs: number;
  maxTs: number;
  tsToY: (ts: number) => number;
  yToTs: (y: number) => number;
  yAtIndex: (i: number) => number;
}

export interface TimeScaleOptions {
  topPadding: number;
  bottomPadding: number;
}

export function buildTimeScale(tsMs: number[], opts: TimeScaleOptions): TimeScale {
  const { topPadding, bottomPadding } = opts;
  const sorted = [...tsMs].sort((a, b) => a - b);

  const breakpoints: Breakpoint[] = [];
  let yOff = 0;
  for (let i = 0; i < sorted.length; i++) {
    if (i > 0) {
      const gapSec = Math.max(0, (sorted[i] - sorted[i - 1]) / 1000);
      const spacing = Math.min(MAX_SPACING, Math.max(MIN_SPACING, gapSec * TIME_SCALE_K));
      yOff += spacing;
    }
    breakpoints.push({ ts: sorted[i], yOff });
  }

  const totalContent = breakpoints.length === 0 ? 300 : yOff;
  const svgHeight = topPadding + totalContent + bottomPadding;
  const yStart = svgHeight - bottomPadding;
  const minTs = breakpoints.length > 0 ? breakpoints[0].ts : 0;
  const maxTs = breakpoints.length > 0 ? breakpoints[breakpoints.length - 1].ts : 0;

  // 最后一个满足 bp.ts <= ts 的断点下标
  const lowerIndex = (ts: number): number => {
    let lo = 0;
    let hi = breakpoints.length - 1;
    let ans = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (breakpoints[mid].ts <= ts) {
        ans = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    return ans;
  };

  const yOffAt = (ts: number): number => {
    if (breakpoints.length === 0) return totalContent / 2;
    if (breakpoints.length === 1) return 0;
    const i = lowerIndex(ts);
    if (i < 0) return breakpoints[0].yOff;
    if (i >= breakpoints.length - 1) return breakpoints[breakpoints.length - 1].yOff;
    const a = breakpoints[i];
    const b = breakpoints[i + 1];
    if (b.ts === a.ts) return a.yOff;
    const t = (ts - a.ts) / (b.ts - a.ts);
    return a.yOff + t * (b.yOff - a.yOff);
  };

  const tsToY = (ts: number): number => yStart - yOffAt(ts);

  const yToTs = (y: number): number => {
    if (breakpoints.length === 0) return 0;
    if (breakpoints.length === 1) return breakpoints[0].ts;
    const targetOff = yStart - y;
    if (targetOff <= 0) return breakpoints[0].ts;
    const last = breakpoints[breakpoints.length - 1];
    if (targetOff >= last.yOff) return last.ts;
    // 二分查找第一个 yOff >= targetOff 的断点
    let lo = 0;
    let hi = breakpoints.length - 1;
    let ans = breakpoints.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (breakpoints[mid].yOff >= targetOff) {
        ans = mid;
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }
    if (ans === 0) return breakpoints[0].ts;
    const a = breakpoints[ans - 1];
    const b = breakpoints[ans];
    if (b.yOff === a.yOff) return b.ts;
    const t = (targetOff - a.yOff) / (b.yOff - a.yOff);
    return a.ts + t * (b.ts - a.ts);
  };

  const yAtIndex = (i: number): number => {
    if (breakpoints.length === 0) return yStart;
    const idx = Math.max(0, Math.min(breakpoints.length - 1, i));
    return yStart - breakpoints[idx].yOff;
  };

  return {
    breakpoints,
    totalContent,
    svgHeight,
    yStart,
    minTs,
    maxTs,
    tsToY,
    yToTs,
    yAtIndex,
  };
}
