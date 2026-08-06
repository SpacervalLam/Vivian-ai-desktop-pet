/**
 * 时间窗口加载完整性判断
 *
 * 用已加载区间集合（有序、合并）判断某个时间窗口是否已完整加载，
 * 并返回尚未加载的子区间，供按需拉取。
 */

export interface Interval {
  start: number;
  end: number;
}

/** 返回 [start, end) 中尚未被 loaded 覆盖的子区间 */
export function missingRanges(start: number, end: number, loaded: Interval[]): Interval[] {
  if (end <= start) return [];
  const result: Interval[] = [];
  let cursor = start;
  for (const iv of loaded) {
    if (iv.end <= cursor) continue;
    if (iv.start > end) break;
    if (iv.start > cursor) {
      result.push({ start: cursor, end: Math.min(iv.start, end) });
    }
    cursor = Math.max(cursor, iv.end);
    if (cursor >= end) break;
  }
  if (cursor < end) {
    result.push({ start: cursor, end });
  }
  return result;
}

/** 把 [start, end) 插入 loaded 并合并重叠/相邻区间，返回新的有序数组 */
export function insertRange(start: number, end: number, loaded: Interval[]): Interval[] {
  const merged: Interval[] = [];
  let newStart = start;
  let newEnd = end;
  for (const iv of loaded) {
    if (iv.end < newStart) {
      merged.push(iv);
    } else if (iv.start > newEnd) {
      merged.push({ start: newStart, end: newEnd });
      newStart = -1;
      merged.push(iv);
    } else {
      newStart = Math.min(newStart, iv.start);
      newEnd = Math.max(newEnd, iv.end);
    }
  }
  if (newStart !== -1) {
    merged.push({ start: newStart, end: newEnd });
  }
  return merged;
}
