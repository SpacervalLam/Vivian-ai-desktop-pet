// Tauri 事件订阅通用 hook,自动处理 unlisten 与卸载竞态

import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/**
 * 订阅 Tauri 事件的通用 hook，自动处理卸载时的 unlisten 和竞态。
 * @param eventName 事件名
 * @param handler 事件处理函数（用 ref 包裹，避免闭包过期）
 * @param deps 依赖数组，变化时重新订阅
 */
export function useTauriEvent<T = unknown>(
  eventName: string,
  handler: (payload: T) => void,
  deps: unknown[] = [],
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;

    void (async () => {
      const fn = await listen<T>(eventName, (event) => {
        if (!cancelled) {
          handlerRef.current(event.payload);
        }
      });
      if (cancelled) {
        try { fn(); } catch { /* 已卸载 */ }
      } else {
        unlisten = fn;
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) {
        try { unlisten(); } catch { /* ignore */ }
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [eventName, ...deps]);
}
