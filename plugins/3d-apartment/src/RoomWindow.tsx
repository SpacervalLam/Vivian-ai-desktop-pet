/**
 * 房间窗口外壳。挂载 RoomScene，处理进/出房间时桌面桌宠的显隐联动。
 *
 * 进房间：Rust 端 set_room_mode(true) 批量隐藏角色窗口与心智观察器并冻结 WebView
 *         （桌宠渲染循环停止，不再空转 GPU/CPU）
 * 离开房间：set_room_mode(false) 解冻并恢复显示
 *
 * ESC 关闭走 Rust 侧 watch_room_escape 看护线程，而不是前端 keydown：
 * 第一人称 PointerLock 状态下浏览器把 ESC 保留用于退出锁定、不派发 keydown，
 * 前端永远收不到；Rust 用 GetAsyncKeyState 轮询硬件按键状态，不受 PointerLock 影响。
 * 线程仅在 room 窗口是前台窗口时响应 ESC 下降沿，防止误关。
 *
 * 这里的 cleanup 只覆盖「前端主动卸载」的常规路径。用户按 ESC、点系统关闭、
 * 进程退出等路径下 React 可能来不及跑 invoke，所以 Rust 侧还监听了 room 窗口的
 * CloseRequested / Destroyed 事件做兜底，两侧都调同一个幂等的 set_room_mode(false)。
 *
 * hide→freeze / thaw→show 的时序由 Rust 命令在主线程队列里保证，
 * 前端不要绕过它直接操作角色窗口。
 */

import { useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { RoomScene } from './RoomScene';

export default function RoomWindow() {
  // 两条关闭路径（前端 keydown 与 Rust 看护线程）都会响应同一次按键，
  // 用这个标志挡掉第二次，避免对已经销毁的窗口重复 close。
  const closingRef = useRef(false);
  // 场景装配完成后交来的「立即释放 GPU 资源」函数，见 RoomSceneProps.onDisposeReady。
  const disposeSceneRef = useRef<(() => void) | null>(null);

  const closeRoom = useCallback(() => {
    if (closingRef.current) return;
    closingRef.current = true;
    void (async () => {
      // 关窗**之前**先把显存还掉。
      //
      // 房间窗口是 close 掉的，WebView2 上下文随进程一起没了，React 的 unmount
      // cleanup 一次都跑不到——那套 dispose（geometry / material / texture /
      // shadow.map / composer RT / forceContextLoss）全部落空。而 Tauri v2 的
      // 所有窗口共享同一个 WebView2 environment，关掉房间窗口并不拆 GPU 环境，
      // 这份显存要等 WebView2 自己逐出，时机不确定。主动调一次，释放才是确定性的。
      try {
        disposeSceneRef.current?.();
      } catch (err) {
        console.warn('[room] 释放场景资源失败，仍继续关窗', err);
      }
      try {
        await getCurrentWindow().close();
      } catch (err) {
        console.warn('[room] 关闭窗口失败', err);
      }
    })();
  }, []);

  const handleDisposeReady = useCallback((dispose: () => void) => {
    disposeSceneRef.current = dispose;
  }, []);

  useEffect(() => {
    // 进入房间：隐藏并冻结桌面桌宠与心智观察器；启动 ESC 看护
    //
    // 让位其实在 utils/roomWindow 里、窗口显形那一刻就做过了（那是用户真正
    // 看见首屏 loading 层的前提）。这里留着是兜底：从别处直接导航到 ?view=room
    // （开发调试、或将来别的入口）时没人替它调。命令幂等，重复调用不会把
    // 记档覆盖成 false。
    void invoke('set_room_mode', { active: true }).catch((e) =>
      console.warn('[room] set_room_mode(true) 失败', e)
    );
    void invoke('watch_room_escape').catch((e) =>
      console.warn('[room] watch_room_escape 失败', e)
    );

    return () => {
      // 离开房间：解冻并恢复桌面桌宠与心智观察器；停止 ESC 看护
      void invoke('set_room_mode', { active: false }).catch((e) =>
        console.warn('[room] set_room_mode(false) 失败', e)
      );
      void invoke('stop_room_escape_watcher').catch((e) =>
        console.warn('[room] stop_room_escape_watcher 失败', e)
      );
    };
  }, []);

  // 观察者模式（非指针锁定）下按 ESC 关闭窗口。
  // 指针锁定中 ESC 被浏览器吞掉、不派发 keydown，由 Rust 侧 watch_room_escape
  // 看护线程关闭；这里兜底处理「已解锁、显示观察者模式」时按 ESC 的情况，
  // 也让看护线程万一没起来时 ESC 仍然可用。
  // 用 capture 阶段 + e.code，确保先于其他监听捕获、不受键盘布局影响。
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.code !== 'Escape') return;
      // 命令面板打开时 ESC 由面板自己收（关闭面板而不是关掉整个房间窗口）
      if ((window as any).__ROOM__?.cmdPanelOpen) return;
      e.preventDefault();
      closeRoom();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [closeRoom]);

  // 兜底释放：任何关闭路径都会走到这里。
  //
  // 上一条 closeRoom 只覆盖前端 ESC；ESC 看护线程那条路是 **Rust 直接
  // `win.close()`**（apartment_host.rs 的 20ms 硬件轮询），根本不经过前端
  // 的 closeRoom，挂在那儿就漏了。所以真正的兜底必须挂在 close-requested
  // 事件上——无论谁发起的关闭，事件都会派发到 JS。
  //
  // 同步调用，不 await：three 的 dispose 全是同步的，而 close-requested 的
  // 派发不会等我们的异步工作跑完（不等就等于没做）。disposeScene 幂等，
  // 和 closeRoom 里那次调用重复执行无副作用。
  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const u = await win.onCloseRequested(() => {
        try {
          disposeSceneRef.current?.();
        } catch (err) {
          console.warn('[room] close-requested 释放资源失败', err);
        }
      });
      if (cancelled) u();
      else unlisten = u;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return <RoomScene onDisposeReady={handleDisposeReady} />;
}
