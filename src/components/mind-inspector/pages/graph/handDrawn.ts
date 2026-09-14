/**
 * 手绘线条路径生成器：会话圈、回应箭头等手帐风格装饰线。
 *
 * 所有抖动由种子化 PRNG 派生，同一 seed 生成结果恒定，
 * 滚动/重渲染时线条形状不发生变化。
 */

/** FNV-1a 字符串哈希，生成 32 位无符号整数种子 */
export function hashString(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** mulberry32 种子化伪随机数生成器，返回 [0,1) */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const fmt = (n: number): string => (Math.round(n * 100) / 100).toString();

interface Pt {
  x: number;
  y: number;
}

/** 二次贝塞尔中点法平滑：M p0，逐点 Q p[i] mid(p[i],p[i+1])，末点 L 收尾 */
function smoothPath(pts: Pt[]): string {
  if (pts.length < 3) {
    return pts.map((p, i) => `${i === 0 ? 'M' : 'L'} ${fmt(p.x)} ${fmt(p.y)}`).join(' ');
  }
  let d = `M ${fmt(pts[0].x)} ${fmt(pts[0].y)}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const midX = (pts[i].x + pts[i + 1].x) / 2;
    const midY = (pts[i].y + pts[i + 1].y) / 2;
    d += ` Q ${fmt(pts[i].x)} ${fmt(pts[i].y)} ${fmt(midX)} ${fmt(midY)}`;
  }
  const last = pts[pts.length - 1];
  d += ` L ${fmt(last.x)} ${fmt(last.y)}`;
  return d;
}

/**
 * 手绘椭圆圈路径：采样点带径向抖动，首尾过冲交叉，模拟手画圈。
 *
 * @param seed 决定开口位置、倾斜角、过冲量与抖动形态
 */
export function handDrawnLoopPath(
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  seed: string,
): string {
  const rng = mulberry32(hashString(seed));
  const start = rng() * Math.PI * 2;
  const tilt = (rng() - 0.5) * 0.04;
  const sweep = Math.PI * 2 + 0.5 + rng() * 0.25;
  const cosT = Math.cos(tilt);
  const sinT = Math.sin(tilt);

  const N = 26;
  const pts: Pt[] = [];
  for (let i = 0; i < N; i++) {
    const a = start + (sweep * i) / (N - 1);
    const jitter = 1 + (rng() - 0.5) * 0.08;
    const ex = rx * jitter * Math.cos(a);
    const ey = ry * jitter * Math.sin(a);
    pts.push({
      x: cx + ex * cosT - ey * sinT,
      y: cy + ex * sinT + ey * cosT,
    });
  }
  return smoothPath(pts);
}

/**
 * 手绘箭头路径：微弯曲线 + 不对称倒刺箭头，曲线与箭头合并为单个 d。
 *
 * @param bow     弯曲幅度（垂线偏移基准像素）
 * @param trimA   起点沿弦裁剪距离（裁到图形边缘）
 * @param trimB   终点沿弦裁剪距离
 * @param seed    决定弯曲量、抖动与倒刺形态
 */
export function handDrawnArrowPath(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  seed: string,
  bow: number,
  trimA: number,
  trimB: number,
  fanOffset: number = 0,
): string {
  const rng = mulberry32(hashString(seed));
  const dx = x2 - x1;
  const dy = y2 - y1;
  const chord = Math.hypot(dx, dy);
  if (chord < 1) return '';

  // 垂线方向取 x 正向，使箭头背离左侧时间轴弯曲
  let px = -dy / chord;
  let py = dx / chord;
  if (px < 0) {
    px = -px;
    py = -py;
  }

  // fanOffset 使同目标箭头组的控制点沿垂直方向阶梯偏移，形成扇形展开
  const fanShift = bow * 0.35 * fanOffset;
  const cx = (x1 + x2) / 2 + px * (bow * (0.85 + rng() * 0.3) + fanShift);
  const cy = (y1 + y2) / 2 + py * (bow * (0.85 + rng() * 0.3) + fanShift);

  const tStart = Math.min(0.4, Math.max(0, trimA / chord));
  const tEnd = Math.max(0.6, Math.min(1, 1 - trimB / chord));

  const bez = (t: number): Pt => ({
    x: (1 - t) * (1 - t) * x1 + 2 * (1 - t) * t * cx + t * t * x2,
    y: (1 - t) * (1 - t) * y1 + 2 * (1 - t) * t * cy + t * t * y2,
  });
  const tangent = (t: number): Pt => ({
    x: 2 * (1 - t) * (cx - x1) + 2 * t * (x2 - cx),
    y: 2 * (1 - t) * (cy - y1) + 2 * t * (y2 - cy),
  });

  const N = 10;
  const pts: Pt[] = [];
  for (let i = 0; i < N; i++) {
    const t = tStart + ((tEnd - tStart) * i) / (N - 1);
    const p = bez(t);
    if (i > 0 && i < N - 1) {
      const tg = tangent(t);
      const tl = Math.hypot(tg.x, tg.y) || 1;
      const off = (rng() - 0.5) * 2.4;
      p.x += (-tg.y / tl) * off;
      p.y += (tg.x / tl) * off;
    }
    pts.push(p);
  }

  const end = bez(tEnd);
  const tg = tangent(tEnd);
  const phi = Math.atan2(tg.y, tg.x);
  const spread1 = 0.45 + rng() * 0.15;
  const spread2 = 0.55 + rng() * 0.15;
  const len1 = 8 + rng() * 3;
  const len2 = len1 * (0.8 + rng() * 0.3);
  const b1: Pt = {
    x: end.x + Math.cos(phi + Math.PI - spread1) * len1,
    y: end.y + Math.sin(phi + Math.PI - spread1) * len1,
  };
  const b2: Pt = {
    x: end.x + Math.cos(phi + Math.PI + spread2) * len2,
    y: end.y + Math.sin(phi + Math.PI + spread2) * len2,
  };

  return (
    `${smoothPath(pts)} ` +
    `M ${fmt(b1.x)} ${fmt(b1.y)} L ${fmt(end.x)} ${fmt(end.y)} L ${fmt(b2.x)} ${fmt(b2.y)}`
  );
}
