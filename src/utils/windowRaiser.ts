/**
 * 子窗口 Z-order 提升工具（App.tsx 与桌宠三击等入口共用）
 *
 * 桌宠本体窗口始终 topmost，普通层级子窗口（config/memory）默认在桌宠之下。
 * 需要将某个子窗口"置于屏幕顶端"时调用 raiseWindow：
 *  - 临时设 topmost 突破桌宠遮挡、unminimize + show + focus；
 *  - 普通层级窗口失焦时自动降回 non-topmost（恢复普通应用窗口层级行为）。
 */
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

/** 普通层级子窗口：聚焦时临时 topmost（突破桌宠遮挡），失焦自动降回普通层级。
 *  与始终 topmost 的桌宠/气泡/输入框不同，这些窗口的 Z-order 行为与普通应用窗口一致。 */
export const NORMAL_TIER_WINDOWS = new Set(['config', 'memory']);

/** 追踪 raiseWindow 注册的 onFocusChanged 监听器的卸载函数，防止累积 */
export const RAISE_UNLISTEN = new Map<string, () => void>();

/** 将已存在的子窗口提升到 Z-order 顶层。
 *
 *  普通层级窗口（config/memory）：临时设 topmost 突破桌宠遮挡，
 *  失焦时自动降回 non-topmost，实现与普通应用窗口一致的层级行为：
 *  Alt+Tab 切换、点击外部失焦、不永久置顶。
 *
 *  始终 topmost 的窗口（chat/bubble/toast/input 等）：保持 topmost
 *  直到关闭，确保不被桌宠覆盖。 */
export async function raiseWindow(win: WebviewWindow, label?: string) {
  // 先卸载上一次注册的 onFocusChanged 监听器，防止累积导致多个监听器竞争 setAlwaysOnTop(false)
  if (label) {
    const prev = RAISE_UNLISTEN.get(label);
    if (prev) {
      prev();
      RAISE_UNLISTEN.delete(label);
    }
  }

  await win.unminimize();
  await win.show();
  await win.setAlwaysOnTop(true);
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
