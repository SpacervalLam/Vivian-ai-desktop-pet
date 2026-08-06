/**
 * Toast 子窗口 - 在屏幕右下角的独立透明窗口中渲染 Toast 通知与工具确认卡片。
 *
 * 主窗口通过 Tauri 事件驱动本窗口：
 * - `toast:show` 显示一条 toast（payload: { message, type?, duration?, key }）
 * - `toast:confirm` 显示一个工具执行确认卡片（三按钮：拒绝/放行一次/始终允许）
 * - `toast:confirm_done` 某确认已被响应，移除同 request_id 的卡片（覆盖多窗口场景）
 *
 * 本窗口自身透明、无边框、跳过任务栏、始终置顶，默认点击穿透；
 * 存在确认卡片时关闭点击穿透以接收按钮点击，卡片全部清除后恢复穿透。
 * 窗口内有 toast 或确认卡片时显示，全部关闭后隐藏窗口以彻底让出屏幕。
 */

import { useEffect, useState } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, currentMonitor, LogicalPosition, LogicalSize } from '@tauri-apps/api/window';
import Toast, { type ToastType } from './Toast';
import ConfirmToast, {
  type AllowAlwaysScope,
  type ConfirmRiskLevel,
} from './ConfirmToast';

interface ToastItem {
  id: number;
  key?: number;
  message: string;
  type: ToastType;
  duration: number;
  progress?: number;
}

interface ToastShowPayload {
  message: string;
  type?: ToastType;
  duration?: number;
  key: number;
  character_id?: string;
  progress?: number;
}

interface ConfirmItem {
  requestId: number;
  tool: string;
  reason: string;
  riskLevel: ConfirmRiskLevel;
  allowAlwaysScope: AllowAlwaysScope;
}

interface ToastConfirmPayload {
  request_id: number;
  tool: string;
  arguments: unknown;
  reason: string;
  risk_level: ConfirmRiskLevel;
  char_id: string;
  allow_always_scope: AllowAlwaysScope;
}

let nextId = 1;

export default function ToastWindow() {
  const [items, setItems] = useState<ToastItem[]>([]);
  const [confirms, setConfirms] = useState<ConfirmItem[]>([]);

  // 主题：读取 base.theme 配置设置根节点 data-theme，并监听实时变更
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
    };
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const theme = await invoke<string | null>('get_config', { key: 'base.theme' });
        if (!cancelled) applyTheme(theme);
        unlisten = await listen<{ theme: string }>('config:theme-changed', (e) => {
          applyTheme(e.payload?.theme);
        });
        if (cancelled) unlisten();
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const myCharId = params.get('character_id') ?? '';

    let cancelled = false;
    const unlistens: Array<() => void> = [];

    void (async () => {
      const unlistenShow = await listen<ToastShowPayload>('toast:show', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        const p = e.payload;
        setItems((prev) => {
          // 同 key 原地更新：用于持久进度 toast 的刷新与收尾（duration 0 → >0 触发自动关闭）
          if (p.key != null) {
            const idx = prev.findIndex((it) => it.key === p.key);
            if (idx >= 0) {
              const next = [...prev];
              next[idx] = {
                ...next[idx],
                message: p.message,
                type: p.type ?? next[idx].type,
                duration: p.duration ?? next[idx].duration,
                progress: p.progress ?? next[idx].progress,
              };
              return next;
            }
          }
          return [
            ...prev,
            {
              id: nextId++,
              key: p.key,
              message: p.message,
              type: p.type ?? 'info',
              duration: p.duration ?? 3000,
              progress: p.progress,
            },
          ];
        });
      });

      const unlistenConfirm = await listen<ToastConfirmPayload>('toast:confirm', (e) => {
        const p = e.payload;
        if (!p) return;
        // char_id 为空表示不区分角色，所有窗口显示（先响应者生效）
        if (p.char_id && p.char_id !== myCharId) return;
        setConfirms((prev) => {
          if (prev.some((c) => c.requestId === p.request_id)) return prev;
          return [
            ...prev,
            {
              requestId: p.request_id,
              tool: p.tool,
              reason: p.reason,
              riskLevel: p.risk_level,
              allowAlwaysScope: p.allow_always_scope,
            },
          ];
        });
      });

      const unlistenDone = await listen<{ request_id: number }>('toast:confirm_done', (e) => {
        const rid = e.payload?.request_id;
        if (rid == null) return;
        setConfirms((prev) => prev.filter((c) => c.requestId !== rid));
      });

      // 嵌入初始化进度：后端每完成一批 (168条) 后 emit，前端管理持久 toast 的生命周期
      const EMBEDDING_TOAST_KEY = 99001;
      const unlistenEmbedProgress = await listen<{ current: number; total: number }>(
        'embedding:progress',
        (e) => {
          const { current, total } = e.payload ?? { current: 0, total: 1 };
          const pct = Math.round((current / Math.max(total, 1)) * 100);
          if (current >= total) {
            // 完成：切换为 success + 自动关闭
            setItems((prev) => {
              const idx = prev.findIndex((it) => it.key === EMBEDDING_TOAST_KEY);
              if (idx >= 0) {
                const next = [...prev];
                next[idx] = {
                  ...next[idx],
                  message: '情绪感知就绪 ✓',
                  type: 'success',
                  duration: 4000,
                  progress: undefined,
                };
                return next;
              }
              return prev;
            });
          } else {
            // 进度中：创建或更新持久 toast
            setItems((prev) => {
              const idx = prev.findIndex((it) => it.key === EMBEDDING_TOAST_KEY);
              if (idx >= 0) {
                const next = [...prev];
                next[idx] = {
                  ...next[idx],
                  message: `情绪感知初始化中… ${pct}%`,
                  progress: pct,
                };
                return next;
              }
              return [
                ...prev,
                {
                  id: nextId++,
                  key: EMBEDDING_TOAST_KEY,
                  message: `情绪感知初始化中… ${pct}%`,
                  type: 'info' as ToastType,
                  duration: 0,
                  progress: pct,
                },
              ];
            });
          }
        },
      );

      if (cancelled) {
        await unlistenShow();
        await unlistenConfirm();
        await unlistenDone();
        await unlistenEmbedProgress();
        return;
      }

      unlistens.push(unlistenShow, unlistenConfirm, unlistenDone, unlistenEmbedProgress);
      void emit('toast:ready', { character_id: myCharId });
    })();

    return () => {
      cancelled = true;
      for (const un of unlistens) un();
    };
  }, []);

  // 窗口可见性：有 toast 或确认卡片时显示，全部清除后隐藏
  const hasContent = items.length > 0 || confirms.length > 0;
  useEffect(() => {
    const win = getCurrentWindow();
    if (hasContent) {
      void win.show().catch(() => {});
    } else {
      void win.hide().catch(() => {});
    }
  }, [hasContent]);

  // 点击穿透 + 窗口尺寸策略：
  // - 无 confirm：窗口穿透（toast 不需点击），透明区域不挡背景
  // - 有 confirm：关闭穿透让卡片按钮可点击，但把窗口缩小到仅覆盖卡片区域
  //   （右下角），消除透明区域对背景的遮挡
  useEffect(() => {
    const win = getCurrentWindow();
    void win.setIgnoreCursorEvents(confirms.length === 0).catch(() => {});
    // 有 confirm 时缩小窗口到卡片区域：宽度=400（padding+卡片），高度按卡片数量
    if (confirms.length > 0) {
      const cardWidth = 400;
      const cardHeight = 180; // 单个 confirm 卡片估算高度（含 padding+gap）
      const totalHeight = 40 + confirms.length * cardHeight;
      void win.setSize(new LogicalSize(cardWidth, totalHeight)).catch(() => {});
    } else {
      // 无 confirm 时恢复全屏高度窗口（供普通 toast 使用）
      void currentMonitor().then((monitor) => {
        if (monitor) {
          const screenW = monitor.size.width / monitor.scaleFactor;
          const screenH = monitor.size.height / monitor.scaleFactor;
          void win.setSize(new LogicalSize(400, screenH)).catch(() => {});
          void win.setPosition(new LogicalPosition(screenW - 400, 0)).catch(() => {});
        }
      }).catch(() => {});
    }
  }, [confirms.length]);

  const removeItem = (id: number) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
  };

  const removeConfirm = (requestId: number) => {
    setConfirms((prev) => prev.filter((c) => c.requestId !== requestId));
  };

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        overflow: 'hidden',
        background: 'transparent',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-end',
        justifyContent: 'flex-end',
        padding: 20,
        gap: 10,
        pointerEvents: 'none',
      }}
    >
      {confirms.map((c) => (
        <ConfirmToast
          key={c.requestId}
          requestId={c.requestId}
          tool={c.tool}
          reason={c.reason}
          riskLevel={c.riskLevel}
          allowAlwaysScope={c.allowAlwaysScope}
          onDone={() => removeConfirm(c.requestId)}
        />
      ))}
      {items.map((it) => (
        <Toast
          key={it.id}
          message={it.message}
          type={it.type}
          duration={it.duration}
          progress={it.progress}
          onClose={() => removeItem(it.id)}
        />
      ))}
    </div>
  );
}
