/**
 * 「窗口从桌宠矩形扩展到全屏」的入场动画。
 *
 * 若逐帧改窗口几何（SetWindowPos 反复改 x/y/w/h），WebView2 每帧重建交换链、
 * 整棵 DOM 重新排版，必然掉帧且布局跳动。故改用纯合成层：窗口一次性按最终（全屏）
 * 尺寸创建、内容布局只算一次；入场时给根节点整体做 transform（translate + scale）
 * 从「桌宠矩形」插值到「整屏」，全程只改 GPU 合成矩阵、不触发重排，即 60fps 丝滑缩放。
 *
 * 前提：窗口必须 `transparent: true`，否则缩放时其余部分露出不透明底色。
 * 坐标系：调用方传入的桌宠矩形是物理像素（Tauri `outerPosition` / `outerSize`），
 * 本模块用当前窗口 `scaleFactor` 折算为 CSS px，跨不同缩放比例的显示器也不会错位。
 *
 * 两条播放路径按「窗口此刻在不在屏上」二选一：
 * - `playPetReveal`：窗口尚未上屏时（新建 / 最小化 / hide），先把内容压成桌宠矩形再显形。
 * - `replayPetReveal`：窗口已上屏时，先沿同一条变换路径收拢回桌宠矩形再展开，
 *   两段观感对齐（差别只在有没有那段收拢）。窗口最小化时若直接 replay 会先还原一次、
 *   再收拢展开，视觉上呼出两回——故必须二选一。
 *
 * 此外本模块还承载「预热」的握手协议（`buildPrewarmQuery` / `parsePrewarmSession` /
 * `emitPrewarmReady`）：长按桌宠 0.1s 就把窗口建出来加载，但一律不上屏；等长按成立
 * 才发 `pet:reveal` 让它长出来。三条路径共用同一套桌宠矩形与显形语义。
 */

import { getCurrentWindow } from '@tauri-apps/api/window';
import { emit, emitTo } from '@tauri-apps/api/event';

/** 入场动画时长（毫秒） */
export const PET_REVEAL_DURATION_MS = 340;
/** 重播时的收拢段时长（毫秒）：明显短于展开段，避免整段播放拖沓 */
const PET_COLLAPSE_DURATION_MS = 180;
/** 缓动：起手快、收尾稳，长距离位移最「丝滑」（近似 ease-out-quint） */
const PET_REVEAL_EASING = 'cubic-bezier(0.22, 1, 0.36, 1)';
/** 收拢段缓动：起步慢、收尾快，像内容被吸回桌宠 */
const PET_COLLAPSE_EASING = 'cubic-bezier(0.4, 0, 1, 1)';
/** 等 `transitionend` 的兜底余量（毫秒）：窗口失焦/被挂起时该事件可能不来 */
const PET_REVEAL_SETTLE_SLACK_MS = 220;

/** 桌宠 → 子窗口：通知重播入场动画，payload 为 `PetRect`（物理像素） */
export const PET_REVEAL_EVENT = 'pet:reveal';

/** 桌宠窗口矩形（物理像素） */
export interface PetRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 随子窗口 URL 携带桌宠矩形的 query 键（物理像素） */
export const PET_RECT_QUERY = {
  x: 'pet_x',
  y: 'pet_y',
  w: 'pet_w',
  h: 'pet_h',
} as const;

/** 由桌宠矩形拼出 query 片段，供 `openWindow` 的 `extraQuery` 使用 */
export function buildPetRectQuery(rect: PetRect): string {
  const p = new URLSearchParams();
  setPetRectParams(p, rect);
  return p.toString();
}

/** 把桌宠矩形写进已有的 query 参数表 */
function setPetRectParams(p: URLSearchParams, rect: PetRect): void {
  p.set(PET_RECT_QUERY.x, String(Math.round(rect.x)));
  p.set(PET_RECT_QUERY.y, String(Math.round(rect.y)));
  p.set(PET_RECT_QUERY.w, String(Math.round(rect.w)));
  p.set(PET_RECT_QUERY.h, String(Math.round(rect.h)));
}

/** 预热会话号随子窗口 URL 携带的 query 键 */
export const PET_PREWARM_QUERY = 'prewarm';

/**
 * 拼出「预热窗口」的 query：桌宠矩形 + 预热会话号 + `hidden=1`。
 *
 * 预热窗口是长按 0.1s 时就建出来、但**不允许自己上屏**的窗口：加载可以提前，
 * 显形必须等到长按成立。所以除了随 URL 带过去的桌宠矩形（入场动画的起点），
 * 还要两样东西：
 * - 会话号：子窗口就绪后按它回执，桌宠据此确认「这一下是我等的那一次」
 *   （memory 是共享窗口，两个角色桌宠可能同时预热，回执只该被发起者认领）；
 * - `hidden=1`：main.tsx 对子窗口的「渲染完两帧就 show」兜底必须跳过，
 *   否则预热窗口会在长按还没成立时自己冒出来。显形改由 `pet:reveal` 驱动。
 */
export function buildPrewarmQuery(rect: PetRect | null, session: number): string {
  const p = new URLSearchParams();
  if (rect) setPetRectParams(p, rect);
  p.set(PET_PREWARM_QUERY, String(session));
  p.set('hidden', '1');
  return p.toString();
}

/** 从 `location.search` 解析预热会话号；不是预热窗口（没带这个参数）返回 `null` */
export function parsePrewarmSession(search: string): number | null {
  const raw = new URLSearchParams(search).get(PET_PREWARM_QUERY);
  if (raw === null || raw === '') return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

/** 子窗口 → 桌宠：预热窗口已就绪（`pet:reveal` 监听已挂好） */
export const PET_PREWARM_READY_EVENT = 'pet:prewarm_ready';

export interface PetPrewarmReady {
  /** 发起预热的会话号，原样回执 */
  session: number;
}

/**
 * 子窗口侧：宣告「预热就绪，随时可以接显形事件」。
 *
 * 为什么需要这个回执：`pet:reveal` 是事件，子窗口的监听挂上之前发出去就丢了，
 * 而预热正是把「建窗口」提前到了长按成立之前——建完不等于加载完，两者之间
 * 隔着一整个页面冷启。没有回执，长按成立时发的事件可能打在空处，窗口只能靠
 * 兜底定时器迟到显形。
 *
 * 走全局 `emit` 而非 `emitTo`：子窗口并不知道桌宠窗口的 label（多角色下是
 * vivian/nana 等），而这条回执本身极轻；桌宠侧按会话号比对，非本会话的丢弃。
 */
export async function emitPrewarmReady(session: number): Promise<void> {
  const payload: PetPrewarmReady = { session };
  try {
    await emit(PET_PREWARM_READY_EVENT, payload);
  } catch {
    /* IPC 失败：桌宠侧有等待上限兜底，退化为「长按成立后照常打开」 */
  }
}

/**
 * 从 `location.search` 解析桌宠矩形。
 *
 * 四个键缺一不可，且宽高必须为正；任一不满足都返回 `null`
 * （表示「这次不是从桌宠打开的」，调用方应跳过动画、按常态显示）。
 */
export function parsePetRect(search: string): PetRect | null {
  const p = new URLSearchParams(search);
  const num = (key: string): number | null => {
    const raw = p.get(key);
    if (raw === null || raw === '') return null;
    const n = Number(raw);
    return Number.isFinite(n) ? n : null;
  };
  const x = num(PET_RECT_QUERY.x);
  const y = num(PET_RECT_QUERY.y);
  const w = num(PET_RECT_QUERY.w);
  const h = num(PET_RECT_QUERY.h);
  if (x === null || y === null || w === null || h === null) return null;
  if (w <= 0 || h <= 0) return null;
  return { x, y, w, h };
}

/** 等两帧：确保起手帧已经真正提交给合成器，再开始 transition */
function nextFrame(): Promise<void> {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

/** 系统是否开启了「减弱动态效果」 */
function prefersReducedMotion(): boolean {
  return !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
}

/** 「桌宠矩形」折算到本窗口 CSS px 后的一帧变换 */
interface RevealFrame {
  x: number;
  y: number;
  sx: number;
  sy: number;
}

/** 变换的常态值（无位移、无缩放）。与 root 的默认样式同义，写出来是为了让
 *  收拢/展开两段有对称的起终点——transform 缺失时浏览器无法插值（会跳变）。 */
const IDENTITY_TRANSFORM = 'translate3d(0, 0, 0) scale(1, 1)';

const toTransform = (f: RevealFrame): string =>
  `translate3d(${f.x}px, ${f.y}px, 0) scale(${f.sx}, ${f.sy})`;

/**
 * 算出「内容压成桌宠矩形」这一帧的 CSS 变换。
 *
 * 返回 `null` 表示这次没法安全摆frame（取不到窗口位置、窗口尺寸异常等），
 * 调用方据此退化：首次显形路径直接 show，重播路径直接放弃动画。
 */
async function computeRevealFrame(root: HTMLElement, pet: PetRect): Promise<RevealFrame | null> {
  let winX = 0;
  let winY = 0;
  let scale = 1;
  try {
    const win = getCurrentWindow();
    const [pos, sf] = await Promise.all([win.outerPosition(), win.scaleFactor()]);
    winX = pos.x;
    winY = pos.y;
    scale = sf > 0 ? sf : 1;
  } catch {
    // 取不到窗口位置 → 不冒错位的风险
    return null;
  }

  const vw = root.clientWidth || window.innerWidth;
  const vh = root.clientHeight || window.innerHeight;
  if (vw <= 0 || vh <= 0) return null;

  // 物理像素 → 本窗口 CSS px
  const x = (pet.x - winX) / scale;
  const y = (pet.y - winY) / scale;
  const sx = pet.w / scale / vw;
  const sy = pet.h / scale / vh;
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  if (!(sx > 0) || !(sy > 0)) return null;
  return { x, y, sx, sy };
}

/**
 * 立刻把根节点摆到指定 transform（无过渡）。
 *
 * 必须强制提交一次样式：否则紧随其后写入的 transition 会与这个值合并成
 * 一次样式计算，浏览器直接落到终点，看不到任何动画。
 */
function setTransformImmediately(root: HTMLElement, css: string): void {
  root.style.transition = '';
  root.style.transformOrigin = '0 0';
  root.style.willChange = 'transform';
  root.style.transform = css;
  void root.offsetWidth;
}

/** 播放一段 transform 过渡，直到真正结束（带超时兜底：窗口失焦时不派发 `transitionend`） */
function runTransform(root: HTMLElement, target: string, durationMs: number, easing: string): Promise<void> {
  return new Promise<void>((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      root.removeEventListener('transitionend', onTransitionEnd);
      window.clearTimeout(timer);
      resolve();
    };
    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target === root && e.propertyName === 'transform') finish();
    };
    root.addEventListener('transitionend', onTransitionEnd);
    const timer = window.setTimeout(finish, durationMs + PET_REVEAL_SETTLE_SLACK_MS);
    root.style.transition = `transform ${durationMs}ms ${easing}`;
    root.style.transform = target;
  });
}

/** 收尾：清除动画留下的全部内联痕迹，让根节点回到常态 */
function resetTransformStyle(root: HTMLElement): void {
  root.style.transition = '';
  root.style.transform = '';
  root.style.transformOrigin = '';
  root.style.willChange = '';
}

/**
 * 把窗口放上屏幕：最小化的先还原。
 *
 * 还原本身就会显示窗口，所以**只能在首帧已经摆好之后**调用——否则用户先看到的
 * 是复位后的整屏窗口，紧接着才看到它缩回桌宠重新长出来，视觉上等于呼出了两次。
 * 顺序反过来的话（先还原再摆帧）这个闪现在任何情况下都无法避免。
 *
 * `unminimize()` 对未最小化的窗口是无操作（标志没变化就不发 Win32 调用），
 * 所以可以无条件调用，不必先查状态。
 */
async function bringWindowOnScreen(): Promise<void> {
  const win = getCurrentWindow();
  await win.unminimize().catch(() => {});
  await win.show().catch(() => {});
}

/**
 * 播放入场动画，并在「卡片仍是桌宠大小」的首帧才把窗口显形。
 *
 * 调用约定：
 * - 调用方**不要**自行 `show()` / `unminimize()`。本函数负责还原与显形时机，
 *   否则会先闪一帧全屏大图（最小化时更糟：会先整屏复位一次，再播一次入场）。
 * - 窗口可以是被最小化或被 hide 的——正是本路径要接的场景。
 * - 调用方可在 await 之前把根节点 `opacity` 置 0（在首次绘制前同步执行，
 *   例如放在 `useLayoutEffect` 里）。本函数在动画起手时清掉它——
 *   这样在等待 Tauri 返回窗口位置的那几毫秒里不会漏出全屏内容。
 * - 无论成功、提前返回还是异常，本函数都保证根节点最终可见、
 *   且回到「无 transform」的常态（`position: fixed` 等定位语义不受污染）。
 *
 * 取不到窗口位置、窗口尺寸异常、或系统开启了「减弱动态效果」时，
 * 都退化为直接显形（不做缩放），不会把窗口卡在不可见状态。
 */
export async function playPetReveal(root: HTMLElement, pet: PetRect): Promise<void> {
  const clearGuard = () => {
    root.style.opacity = '';
  };

  try {
    // 尊重系统「减弱动态效果」偏好：直接显形
    if (prefersReducedMotion()) {
      await bringWindowOnScreen();
      return;
    }

    const frame = await computeRevealFrame(root, pet);
    if (!frame) {
      await bringWindowOnScreen();
      return;
    }

    // 起手帧：整屏内容压成桌宠矩形，锚在左上角
    setTransformImmediately(root, toTransform(frame));

    // 摆好首帧再显形：用户看到的第一眼就是桌宠大小的卡片
    await bringWindowOnScreen();
    await nextFrame();

    // 起手瞬间显形（清掉调用方的 opacity 守卫）
    clearGuard();

    await runTransform(root, IDENTITY_TRANSFORM, PET_REVEAL_DURATION_MS, PET_REVEAL_EASING);
  } finally {
    // 任何路径收尾都回到常态：可见、无 transform
    clearGuard();
    resetTransformStyle(root);
  }
}

/**
 * 重播入场动画——用于窗口**已经显示在屏幕上**时再次长按桌宠的场景。
 *
 * 为什么不是直接压到起手帧：此时内容是全屏常态，直接摆 frame 会是一次肉眼可见的
 * 「全屏啪地塌成小卡片」跳变。所以先沿同一条变换路径播一段较短的收拢
 * （`PET_COLLAPSE_DURATION_MS`），再原路展开——展开段与首次打开逐帧一致，
 * 唯一差别是多了那段收拢，用户感知到的仍然是「从桌宠位置长出来」。
 *
 * 调用约定：
 * - 窗口必须**已经在屏上**（可见且未最小化，由调用方判断）。本函数不碰可见性——
 *   重复的 hide/show 会让窗口闪一下并抖动 Z 序。
 * - 不在屏上时（被最小化 / 被 hide）不要走这里，走 `playPetReveal`：
 *   否则得先还原窗口才看得见内容，那次还原本身就是一次呼出，随后这段收拢展开
 *   就是第二次，合起来像呼出了两回。
 * - 根节点当前必须处于常态（无残留 transform），否则首帧基准就是错的。
 * - 失败时不抛异常，最坏情况是这次没有动画，不影响窗口可用性。
 */
export async function replayPetReveal(root: HTMLElement, pet: PetRect): Promise<void> {
  try {
    if (prefersReducedMotion()) return;

    const frame = await computeRevealFrame(root, pet);
    if (!frame) return;

    // 先把根节点钉到常态：浏览器需要这个明确的起点才能插值出收拢过程
    setTransformImmediately(root, IDENTITY_TRANSFORM);
    await nextFrame();

    // 收拢 → 展开。两段合起来等价于「内容回到桌宠位置再长回来」
    await runTransform(root, toTransform(frame), PET_COLLAPSE_DURATION_MS, PET_COLLAPSE_EASING);
    await runTransform(root, IDENTITY_TRANSFORM, PET_REVEAL_DURATION_MS, PET_REVEAL_EASING);
  } finally {
    resetTransformStyle(root);
  }
}

/**
 * 桌宠侧：通知指定 label 的窗口重播入场动画。
 *
 * 走得是 Tauri 事件而非 URL 参数——复用已有窗口不能再 navigate 一次，
 * 那会把整个页面 reload 掉、丢掉当前页签与未保存的输入。
 */
export async function emitPetReveal(label: string, rect: PetRect): Promise<void> {
  try {
    await emitTo(label, PET_REVEAL_EVENT, rect);
  } catch {
    // 目标窗口此刻不存在 / IPC 失败：静默降级为「只聚焦，不动画」
  }
}
