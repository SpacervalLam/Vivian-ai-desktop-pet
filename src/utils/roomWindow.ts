/**
 * 3D 公寓窗口的打开入口（主窗口与心智观察器共用）
 *
 * 公寓窗口是全局单例：主窗口快捷键和心智观察器封面条都可能触发，
 * 两个入口处在不同的 WebView（各自的 JS 上下文），本地缓存互不共享，
 * 所以复用判定一律以 `WebviewWindow.getByLabel` 为准——它查的是 Rust 侧
 * 的窗口注册表，跨窗口一致。
 *
 * 窗口形态固定为「屏幕尺寸 + 无边框 + 不可缩放 + 不透明」：
 */

import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { LogicalPosition } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';

export const ROOM_WINDOW_LABEL = 'room';

/** 本上下文持有的窗口引用。仅用于防止 JS 对象被 GC，
 *  存活判定不走这里（窗口可能在别的入口被关掉）。 */
let cached: WebviewWindow | null = null;

/** 创建流程进行中标志：getByLabel 是 async 的，await 期间并发调用
 *  会双双拿到 null 然后各自 new 一个窗口（重复 label 在 Tauri 侧
 *  报 tauri://error，第二个窗口其实创建成功了，缓存短暂不一致）。
 *  同步置位后并发调用直接走"复用缓存"路径。 */
let pending = false;

/** 创建并/或聚焦公寓窗口。
 *
 * 已存在实例时只做 unminimize → show → setFocus，不重建——
 * 房间场景加载一次要几百毫秒，重复创建等于每次都重新搭一遍模型。
 */
export async function openRoomWindow(title = '公寓'): Promise<void> {
  if (pending) {
    // 并发调用：等本次创建流程收尾，走缓存复用
    await new Promise((r) => setTimeout(r, 300));
    if (cached) return openRoomWindow(title);
  }
  let win: WebviewWindow | null = cached;

  if (!win) {
    pending = true;
    try {
      win = await WebviewWindow.getByLabel(ROOM_WINDOW_LABEL);
    } catch {
      win = null;
    } finally {
      pending = false;
    }
  }

  if (win) {
    try {
      cached = win;
      await win.unminimize();
      await win.show();
      await win.setFocus();
      return;
    } catch {
      cached = null;
      win = null;
    }
  }
  const width = Math.floor(window.screen.width);
  const height = Math.floor(window.screen.height);

  try {
    const created = new WebviewWindow(ROOM_WINDOW_LABEL, {
      url: '/?view=room',
      title,
      width,
      height,
      decorations: false,
      transparent: false,
      resizable: false,
      shadow: false,
      center: true,
      visible: false,
      dragDropEnabled: false,
    });

    cached = created;

    created.once('tauri://created', () => {
      void (async () => {
        try {
          await created.setPosition(new LogicalPosition(0, 0));
          await created.show();
          await created.setFocus();
        } catch {
          /* 窗口已被关闭 */
        }
      })();
    });

    created.once('tauri://error', (e) => {
      console.warn('[roomWindow] 窗口创建失败:', e);
      cached = null;
    });
  } catch (e) {
    console.error('[roomWindow] 创建窗口异常:', e);
    cached = null;
  }
}
