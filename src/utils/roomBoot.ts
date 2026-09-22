/**
 * 房间窗口的启动过程：时间线打点 + 首屏 loading 层的收尾。
 *
 * 为什么需要它：「点启动公寓 → 画面出来」这段空白，实际由好几段性质完全不同的
 * 耗时叠加而成——
 *   1. HTML 解析（index.html 的 inline 脚本 + 首屏 loading 层）
 *   2. 主 chunk 求值（react / tauri / i18n）
 *   3. 动态 import 房间 chunk（three.js 全量，约 1 MB）
 *   4. 场景同步装配（造 mesh / 合批 / 描边 / PMREM / 碰撞体 / 导航栅格）
 *   5. 首帧渲染
 * 不分开量就只能靠猜，很容易去优化占比最小的那一段。这里每段打一个 mark，
 * 在 devtools 里 `__BOOT__` 或看控制台那行汇总即可看到真实分布。
 *
 * 这一层刻意做到「零依赖 + 可忽略开销」：它在房间窗口的关键路径上。
 */

export interface BootMark {
  /** 节点名，如 `scene:built`。 */
  name: string;
  /** 相对导航开始（performance.timeOrigin）的毫秒数。 */
  t: number;
  /** 距上一个节点的毫秒数——真正要看的增量。 */
  dt: number;
}

interface BootGlobal {
  __BOOT__?: BootMark[];
}

const g = globalThis as unknown as BootGlobal;

/** 复用 index.html inline 脚本可能已经建好的数组，别把它冲掉。 */
const marks: BootMark[] = (g.__BOOT__ = g.__BOOT__ || []);
let last = marks.length > 0 ? marks[marks.length - 1].t : 0;

/**
 * 打一个启动节点。同一名字可以打多次（会各记一条），用于「进房间 → 退房间 →
 * 再进房间」时看第二次是不是明显更快。
 */
export function bootMark(name: string): void {
  if (typeof performance === 'undefined') return;
  const t = performance.now();
  marks.push({ name, t: Math.round(t), dt: Math.round(t - last) });
  last = t;
}

/** 把时间线打到控制台一次。只在房间窗口调用，避免每个子窗口都刷一行。 */
export function logBootTimeline(): void {
  if (marks.length === 0) return;
  const lines = marks
    .map((m) => `  +${String(m.dt).padStart(5)}ms  (t=${String(m.t).padStart(6)}ms)  ${m.name}`)
    .join('\n');
  console.info(`[room boot] 启动时间线\n${lines}\n  合计 ${marks[marks.length - 1].t}ms`);
}

/**
 * loading 层至少显示这么久，避免「闪一下」——首次打开要几秒，但退房再进
 * （或将来做成 hide 复用）可能只要几十毫秒，那时闪一下比一直转着更难受。
 */
const MIN_VISIBLE_MS = 250;
/** 淡出时长，必须与 index.html 里 `#boot-loader` 的 transition 一致。 */
const FADE_MS = 360;

let dismissed = false;

/**
 * 淡出并移除首屏 loading 层。由「首帧真的画出来了」触发，而不是 React 挂载——
 * 挂载完成时 canvas 还是空的，这时撤掉 loading 层会露出一个黑屏，等于把
 * 空白从「有 loading」变成「没 loading 的黑屏」。
 *
 * 幂等：首帧、错误分支、兜底定时器都可能调它。
 */
export function dismissBootLoader(): void {
  if (dismissed) return;
  dismissed = true;
  const el = document.getElementById('boot-loader');
  if (!el) return;
  const shownAt = Number(el.dataset.t || 0);
  const elapsed = typeof performance === 'undefined' ? MIN_VISIBLE_MS : performance.now() - shownAt;
  const wait = Math.max(0, MIN_VISIBLE_MS - elapsed);
  window.setTimeout(() => {
    el.classList.add('bl-out');
    window.setTimeout(() => el.remove(), FADE_MS);
  }, wait);
}
