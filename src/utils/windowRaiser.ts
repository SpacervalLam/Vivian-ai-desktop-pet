/**
 * 子窗口 Z-order 提升工具（App.tsx 与桌宠三击等入口共用）
 *
 * 桌宠本体窗口始终 topmost；心智观察器始终在桌宠之下。
 * raiseWindow 恢复窗口并聚焦；config 可临时置顶，memory 始终保持普通层级。
 */
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

/** 普通层级子窗口：聚焦时临时 topmost（突破桌宠遮挡），失焦自动降回普通层级。
 *  与始终 topmost 的桌宠/气泡/输入框不同，这些窗口的 Z-order 行为与普通应用窗口一致。 */
export const NORMAL_TIER_WINDOWS = new Set(['config']);

/** 追踪 raiseWindow 注册的 onFocusChanged 监听器的卸载函数，防止累积 */
export const RAISE_UNLISTEN = new Map<string, () => void>();

/** 等恢复落定的轮询：步长与上限（0.01s × 12 = 120ms 封顶） */
const RESTORE_WAIT_STEP_MS = 10;
const RESTORE_WAIT_STEPS = 12;

/** 只要能查可见性与最小化状态的窗口句柄——`Window` 与 `WebviewWindow` 都满足，
 *  调用方（子窗口自身用 `getCurrentWindow()`、父窗口用 `WebviewWindow`）不必统一到一种类型 */
type VisibilityProbe = {
  isVisible(): Promise<boolean>;
  isMinimized(): Promise<boolean>;
};

/** 窗口此刻是不是「已经摆在屏幕上」（可见且未最小化）。
 *
 *  必须两个都查：最小化的窗口在 tao 的可见标志里仍然是 visible，
 *  只看 `isVisible()` 会把最小化误判成在屏上。取不到状态一律当作不在屏上，
 *  调用方据此走更安全的那条路（先摆好首帧再显形）。 */
export async function isWindowOnScreen(win: VisibilityProbe): Promise<boolean> {
  try {
    return (await win.isVisible()) && !(await win.isMinimized());
  } catch {
    return false;
  }
}

/** 将已存在的子窗口提升到 Z-order 顶层。
 *
 *  config 临时设 topmost，失焦时自动降回 non-topmost：
 *  Alt+Tab 切换、点击外部失焦、不永久置顶。
 *
 *  memory 始终 non-topmost，保持在桌宠下方。其他窗口保持 topmost。
 *
 *  `selfReveal`：显形时机归窗口自己（它要用「先摆好首帧再出现」的入场动画）。
 *  此时这里只做 Z 序与焦点，碰可见性会替它先把窗口摆上屏——最小化时尤其明显：
 *  用户先看到整屏复位一次，再看到它缩回桌宠长出来，等于呼出了两回。 */
export async function raiseWindow(
  win: WebviewWindow,
  label?: string,
  selfReveal = false,
) {
  // 先卸载上一次注册的 onFocusChanged 监听器，防止累积导致多个监听器竞争 setAlwaysOnTop(false)
  if (label) {
    const prev = RAISE_UNLISTEN.get(label);
    if (prev) {
      prev();
      RAISE_UNLISTEN.delete(label);
    }
  }

  if (!selfReveal) {
    // 「可见且未最小化」是后面两步的前提——对最小化或隐藏的窗口，
    // setFocus 内部会直接跳过抢前台那一整段（它只对「可见、非最小化、且不在前台」的窗口下手）。
    //
    // 而 show/unminimize 都是投递到主线程 FIFO 队列的即发即忘操作：await 返回时窗口状态还没变，
    // 偏偏 setFocus 是**同步**读窗口自己那份影子标志来决策的。恢复最小化后不等状态落定，
    // 这里读到的仍是「最小化」，激活就被整段跳过，窗口回到屏幕上却停在后排。
    // 轮询以标志位为准（标志与 ShowWindow 在同一段里更新），代价是一次多余的 IPC。
    await win.show();
    if (await win.isMinimized().catch(() => false)) {
      await win.unminimize();
      for (
        let i = 0;
        i < RESTORE_WAIT_STEPS && (await win.isMinimized().catch(() => false));
        i++
      ) {
        await new Promise((resolve) => window.setTimeout(resolve, RESTORE_WAIT_STEP_MS));
      }
    }
  }

  // 心智观察器是全屏窗口，但桌宠必须始终绘制在它上面。
  // 最小化时 setFocus 可能被 tao 跳过，入场动画的显形会继续激活窗口。
  await win.setAlwaysOnTop(label !== 'memory');
  await win.setFocus();

  // 普通层级窗口：失焦时自动降回 non-topmost
  if (label && NORMAL_TIER_WINDOWS.has(label)) {
    const unlisten = await win.onFocusChanged(({ payload: focused }) => {
      if (!focused) {
        void win.setAlwaysOnTop(false);
        // 自清理：失焦回调触发后即卸载，下次 raise 会重新注册
        const u = RAISE_UNLISTEN.get(label);
        if (u) { u(); RAISE_UNLISTEN.delete(label); }
      }
    });
    RAISE_UNLISTEN.set(label, unlisten);
  }
}
