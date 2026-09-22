/**
 * 3D 公寓窗口的打开入口（主窗口与心智观察器共用）
 *
 * 公寓窗口是全局单例：主窗口快捷键和心智观察器封面条都可能触发，
 * 两个入口处在不同的 WebView（各自的 JS 上下文），本地缓存互不共享，
 * 所以复用判定一律以 `WebviewWindow.getByLabel` 为准——它查的是 Rust 侧
 * 的窗口注册表，跨窗口一致。
 *
 * 窗口形态固定为「屏幕尺寸 + 无边框 + 不可缩放 + 不透明」：
 *
 * 显形与让位是两件事，必须都发生在窗口这一侧，不能交给房间页面自己：
 * 房间的首屏是 index.html 里的静态 loading 层，点开就该立刻看见它转圈，
 * 而真正盖住它的其实是「还杵在屏幕上的心智观察器/桌宠」——见 stepAside。
 *
 * 两个入口对心智观察器的处置不同：
 *  - 心智观察器自己的「进入公寓」按钮：`closeInspector: true`，把心智观察器
 *    直接关掉，退出公寓也不再恢复（用户是主动关的）。
 *  - 主窗口快捷键 / 托盘：心智观察器随房间模式一起让位，退出公寓时恢复显示。
 */

import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { LogicalPosition } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';

export const ROOM_WINDOW_LABEL = 'room';

/** 心智观察器窗口 label，与 Rust 侧 commands::inspector::INSPECTOR_WINDOW_LABEL
 *  以及 App.tsx 的 openWindow('memory', ...) 一致。 */
const INSPECTOR_WINDOW_LABEL = 'memory';

/** openRoomWindow 的可选项 */
export interface OpenRoomOptions {
  /** 进公寓时把心智观察器窗口整个关掉（而不是隐藏后退出再恢复）。
   *  心智观察器的「进入公寓」按钮走这条：它的窗口被销毁后不会自动回来。 */
  closeInspector?: boolean;
}

/** 本上下文持有的窗口引用。仅用于防止 JS 对象被 GC，
 *  存活判定不走这里（窗口可能在别的入口被关掉）。 */
let cached: WebviewWindow | null = null;

/** 创建流程进行中标志：getByLabel 是 async 的，await 期间并发调用
 *  会双双拿到 null 然后各自 new 一个窗口（重复 label 在 Tauri 侧
 *  报 tauri://error，第二个窗口其实创建成功了，缓存短暂不一致）。
 *  同步置位后并发调用直接走"复用缓存"路径。 */
let pending = false;

/**
 * 让位：隐藏桌宠与心智观察器（`set_room_mode`）。
 *
 * 必须在窗口显形那一刻就做，不能留给房间页面自己。两条理由：
 *
 * 1. React 的 effect 是「子先于父」：`RoomWindow` 的 effect 排在 `RoomScene`
 *    那个大 effect 之后，而后者是**一整段同步阻塞主线程**的场景装配。交给它，
 *    让位就会晚整整一个装配周期——对首开来说就是几秒。
 * 2. 心智观察器是全屏**置顶**窗口（raiseWindow 给它 setAlwaysOnTop(true)，
 *    失焦才降回）。它不让开，房间窗口就算已经 show 了也整个压在下面，用户
 *    看到的就是「心智观察器继续杵着 → 几秒后公寓直接出现」，首屏 loading
 *    层一帧都露不出来。
 *
 * Rust 侧命令幂等（ROOM_MODE_ACTIVE 只在真翻转时执行一次），所以房间页面
 * 挂载后再调一次 true 不会重复记档，这里是安全的提前量。
 */
function stepAside(active: boolean): void {
  void invoke('set_room_mode', { active }).catch((e) =>
    console.warn(`[roomWindow] set_room_mode(${active}) 失败`, e)
  );
}

/** 让位的逆向操作，只在「房间窗口确实没建起来」时执行。
 *
 * 并发点开会让第二个 new WebviewWindow 收到 tauri://error，但第一个可能已经
 * 建好了。不加判断地回滚会把桌宠与心智观察器放回到房间上面——正好是这次要
 * 修的那个观感。以 Rust 侧窗口注册表为准，查得到就说明房间在，别动。 */
async function rollbackStepAside(): Promise<void> {
  try {
    if (await WebviewWindow.getByLabel(ROOM_WINDOW_LABEL)) return;
  } catch {
    /* 查不到 → 确实没建起来，继续回滚 */
  }
  stepAside(false);
}

/** 关掉心智观察器窗口。
 *
 * 调用它的往往是心智观察器自己，所以有一条硬约束：**必须是流程的最后一步**。
 * 窗口销毁后，这个 JS 上下文里所有还没返回的 IPC 全部丢失——包括发给房间窗口
 * 的 show / setPosition / setFocus。所以要么等这些调用都 await 完（复用分支），
 * 要么把房间的落地整个交给它自己（创建分支走 main.tsx 那条路）。 */
async function closeInspectorWindow(): Promise<void> {
  try {
    const inspector = await WebviewWindow.getByLabel(INSPECTOR_WINDOW_LABEL);
    await inspector?.close();
  } catch (e) {
    console.warn('[roomWindow] 关闭心智观察器失败', e);
  }
}

/** 创建并/或聚焦公寓窗口。
 *
 * 已存在实例时只做 unminimize → show → setFocus，不重建——
 * 房间场景加载一次要几百毫秒，重复创建等于每次都重新搭一遍模型。
 */
export async function openRoomWindow(
  title = '公寓',
  opts: OpenRoomOptions = {},
): Promise<void> {
  if (pending) {
    // 并发调用：等本次创建流程收尾，走缓存复用
    await new Promise((r) => setTimeout(r, 300));
    if (cached) return openRoomWindow(title, opts);
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
      // 复用路径也要让位：房间模式可能因为上一次异常退出而没激活，
      // 那时桌宠会浮在已加载好的房间上面。
      stepAside(true);
      await win.setFocus();
      // 房间窗口已经处理完，关自己才是安全的——本窗口销毁后未返回的 IPC 全丢
      if (opts.closeInspector) await closeInspectorWindow();
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
        // 位置先定再显形：创建时是居中的，show 之后移动会看到整窗跳一下。
        // 但这一步绝不能拖死显形——它失败时窗口会一直停在 visible:false，
        // 只能等房间页面自己的兜底 show，而那段排在 React 渲染之后，
        // 观感就退化成「等场景加载完才出窗口」。
        try {
          await created.setPosition(new LogicalPosition(0, 0));
        } catch (e) {
          console.warn('[roomWindow] setPosition 失败，继续显形:', e);
        }
        try {
          await created.show();
        } catch (e) {
          console.warn('[roomWindow] show 失败:', e);
        }
        // 显形后立刻让位：此刻窗口里画的是 index.html 的静态 loading 层，
        // 场景还没开始装配，让位完用户看到的就是「转圈」而不是「上一屏的残留」。
        stepAside(true);
        try {
          await created.setFocus();
        } catch {
          /* 窗口已被关闭 */
        }
      })();
    });

    created.once('tauri://error', (e) => {
      console.warn('[roomWindow] 窗口创建失败:', e);
      cached = null;
      // 窗口没建起来，就不能把桌宠和心智观察器留在隐藏状态——
      // 它们没有 CloseRequested 兜底可走。
      void rollbackStepAside();
    });

    if (opts.closeInspector) {
      // 心智观察器要「立刻」消失，所以不等 tauri://created——等它等于把关闭
      // 拖到窗口创建完成之后（首开是几百毫秒，人眼看得出来）。
      //
      // 代价是本窗口随即销毁，created 回调大概率收不到，房间窗口的定位/显形/
      // 聚焦/让位全部改由它自己的 main.tsx 完成（那条路是必需的，不是兜底）。
      //
      // 这里**故意不调 stepAside**：本窗口销毁后无法确认房间是否真的建起来了，
      // 万一创建失败，隐藏掉的桌宠就再也没有恢复路径（房间不存在，也就没有
      // CloseRequested 兜底）。让位交给房间窗口自己——它存在才会调，不存在就
      // 什么都不动，桌宠照常留在桌面上。心智观察器关了、公寓没出来，用户看得
      // 见、也能重开；桌宠凭空消失则是没法自愈的。
      void closeInspectorWindow();
    }
  } catch (e) {
    console.error('[roomWindow] 创建窗口异常:', e);
    cached = null;
    void rollbackStepAside();
  }
}
