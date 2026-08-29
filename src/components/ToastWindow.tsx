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

import { useEffect, useRef, useState } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, currentMonitor, LogicalPosition, LogicalSize } from '@tauri-apps/api/window';
import Toast, { type ToastAction, type ToastType } from './Toast';
import ConfirmToast, {
  type AllowAlwaysScope,
  type ConfirmRiskLevel,
} from './ConfirmToast';

interface ToastItem {
  id: number;
  key?: number;
  /** 启动进度任务标识：同任务刷新定位同一 toast，任务切换换新 toast */
  taskKey?: string;
  message: string;
  type: ToastType;
  duration: number;
  progress?: number;
  /** 附带的一键操作（如主题切换确认按钮） */
  action?: ToastAction;
}

interface ToastShowPayload {
  message: string;
  type?: ToastType;
  duration?: number;
  key: number;
  character_id?: string;
  progress?: number;
  action?: ToastAction;
}

interface ConfirmItem {
  requestId: number;
  tool: string;
  reason: string;
  riskLevel: ConfirmRiskLevel;
  allowAlwaysScope: AllowAlwaysScope;
  charId: string;
  args?: unknown;
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

/** 嵌入初始化进度 toast 的固定 key */
const EMBEDDING_TOAST_KEY = 99001;
/** 启动进度应用节流窗口（毫秒）：同任务的进度刷新高频到时保持文本可读 */
const STARTUP_THROTTLE_MS = 400;
/** 任务切换间隔低于此值视为瞬时任务：旧 toast 直接移除，不做成功态收尾，避免刷屏 */
const STARTUP_FAST_SWITCH_MS = 1500;

/** 从启动进度 stage 文本提取任务标识：
 * - 取 "…" 之前的主体（"… 5/96" 这类进度后缀属同一任务的刷新，不换 toast）
 * - 剥除 "（…）" 括注（语义语料的 意图/话题/记忆/关系 维度归并为同一任务）
 * 无 "…" 的 stage（确认性消息如"已就绪"）按整串视为独立任务。
 */
function stageTaskKey(stage: string): string {
  const body = stage.split('…')[0].trim();
  return body.replace(/（[^）]*）/g, '').trim();
}

export default function ToastWindow() {
  // 子窗口身份：startup 专用窗口只展示启动进度，且内容锚定右上（与角色 toast 右下错开）
  const myCharId = new URLSearchParams(window.location.search).get('character_id') ?? '';
  const [items, setItems] = useState<ToastItem[]>([]);
  const [confirms, setConfirms] = useState<ConfirmItem[]>([]);
  const contentRef = useRef<HTMLDivElement>(null);

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
    let cancelled = false;
    const unlistens: Array<() => void> = [];

    // 启动进度节流状态：任务刷新高频时合并应用，任务切换时旧 toast 收尾。
    // - 去重：与最近已应用载荷一致的周期重发直接丢弃，不触发重渲染
    // - 节流：窗口期内只暂存最新载荷，到期一次性应用
    // - 完成态旁路：current ≥ total 立即生效，保证收尾提示及时出现
    const startupThrottle: {
      lastPayload: { current: number; total: number; stage: string } | null;
      lastAppliedAt: number;
      timer: number | null;
      pending: { current: number; total: number; stage: string } | null;
      lastStageKey: string | null;
      lastStageSwitchAt: number;
    } = { lastPayload: null, lastAppliedAt: 0, timer: null, pending: null, lastStageKey: null, lastStageSwitchAt: 0 };

    const applyStartupProgress = (p: { current: number; total: number; stage: string }) => {
      const stageKey = stageTaskKey(p.stage);
      const now = Date.now();
      startupThrottle.lastPayload = p;
      startupThrottle.lastAppliedAt = now;
      const pct = Math.round((p.current / Math.max(p.total, 1)) * 100);
      const done = p.current >= p.total;

      setItems((prev) => {
        let next = prev;
        // 任务切换：给上一个任务的 toast 收尾——瞬时任务（切换间隔极短）直接移除，
        // 长任务转成功态自动关闭，保证"每个任务有自己的 toast"且无闪烁替换
        const prevKey = startupThrottle.lastStageKey;
        if (prevKey !== null && prevKey !== stageKey) {
          const fast = now - startupThrottle.lastStageSwitchAt < STARTUP_FAST_SWITCH_MS;
          next = next
            .map((it): ToastItem | null => {
              if (it.taskKey !== prevKey) return it;
              if (fast) return null;
              return {
                ...it,
                message: '✓ 完成',
                type: 'success',
                duration: 2600,
                progress: undefined,
              };
            })
            .filter((it): it is ToastItem => it !== null);
        }
        startupThrottle.lastStageKey = stageKey;
        startupThrottle.lastStageSwitchAt = now;

        const message = done ? (p.stage || '启动完成') : `${p.stage} ${pct}%`;
        const item = {
          message,
          type: (done ? 'success' : 'info') as ToastType,
          duration: done ? 4000 : 0,
          progress: done ? undefined : pct,
        };
        const idx = next.findIndex((it) => it.taskKey === stageKey);
        if (idx >= 0) {
          const updated = [...next];
          updated[idx] = { ...updated[idx], ...item };
          return updated;
        }
        return [
          ...next,
          {
            id: nextId++,
            taskKey: stageKey,
            ...item,
          },
        ];
      });
    };

    void (async () => {
      // 并行注册所有事件监听，避免顺序 await 累积延迟拖慢挂载
      const [
        unlistenShow,
        unlistenConfirm,
        unlistenDone,
        unlistenEmbedProgress,
        unlistenStartupProgress,
      ] = await Promise.all([
        listen<ToastShowPayload>('toast:show', (e) => {
          if (myCharId === 'startup') return;
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
                action: p.action,
              },
            ];
          });
        }),
        listen<ToastConfirmPayload>('toast:confirm', (e) => {
          const p = e.payload;
          if (!p) return;
          if (myCharId === 'startup') return;
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
                charId: p.char_id ?? '',
                args: p.arguments,
              },
            ];
          });
        }),
        listen<{ request_id: number }>('toast:confirm_done', (e) => {
          const rid = e.payload?.request_id;
          if (rid == null) return;
          if (myCharId === 'startup') return;
          setConfirms((prev) => prev.filter((c) => c.requestId !== rid));
        }),
        // 嵌入初始化进度：后端每完成一批 (168条) 后 emit，前端管理持久 toast 的生命周期
        listen<{ current: number; total: number }>('embedding:progress', (e) => {
          if (myCharId === 'startup') return;
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
        }),
        // 统一启动进度：所有启动加载阶段共用一个持久 toast（去重/节流见 applyStartupProgress）
        listen<{ current: number; total: number; stage: string }>('startup:progress', (e) => {
          if (myCharId !== 'startup') return;
          const p = e.payload ?? { current: 0, total: 1, stage: '' };
          // 去重：与最近已应用载荷一致的周期重发直接丢弃
          const last = startupThrottle.lastPayload;
          if (
            last &&
            last.current === p.current &&
            last.total === p.total &&
            last.stage === p.stage
          ) {
            return;
          }
          const done = p.current >= p.total;
          const now = Date.now();
          if (done || now - startupThrottle.lastAppliedAt >= STARTUP_THROTTLE_MS) {
            if (startupThrottle.timer !== null) {
              window.clearTimeout(startupThrottle.timer);
              startupThrottle.timer = null;
              startupThrottle.pending = null;
            }
            applyStartupProgress(p);
            return;
          }
          // 节流窗口内：暂存最新载荷，到期一次性应用
          startupThrottle.pending = p;
          if (startupThrottle.timer === null) {
            startupThrottle.timer = window.setTimeout(() => {
              startupThrottle.timer = null;
              const pending = startupThrottle.pending;
              startupThrottle.pending = null;
              if (pending && !cancelled) applyStartupProgress(pending);
            }, STARTUP_THROTTLE_MS - (now - startupThrottle.lastAppliedAt));
          }
        }),
      ]);


      // 启动窗口：立即显示占位进度并拉取快照补齐。后端预检不等前端就绪
      // 直接开始（进度事件由周期重发补齐），此处快照保证挂载即可见
      if (myCharId === 'startup') {
        type StartupSnapshot = {
          in_progress: boolean;
          current: number | null;
          total: number | null;
          stage: string | null;
        };
        let snap: StartupSnapshot | null = null;
        try {
          snap = await invoke<StartupSnapshot | null>('get_startup_progress');
        } catch { /* ignore */ }
        if (!snap || snap.in_progress) {
          const pct =
            snap?.current != null && snap.total ? Math.round((snap.current / snap.total) * 100) : 0;
          const message = snap?.stage || '正在启动…';
          const stageKey = stageTaskKey(snap?.stage || '');
          // 用快照载荷初始化去重基准与当前任务：紧随其后的周期重发（与快照同源）会被丢弃
          if (snap?.current != null && snap?.total != null && snap?.stage != null) {
            startupThrottle.lastPayload = {
              current: snap.current,
              total: snap.total,
              stage: snap.stage,
            };
            startupThrottle.lastStageKey = stageKey;
            startupThrottle.lastStageSwitchAt = Date.now();
          }
          setItems((prev) => {
            if (prev.some((it) => it.taskKey === stageKey)) return prev;
            return [
              ...prev,
              {
                id: nextId++,
                taskKey: stageKey,
                message,
                type: 'info' as ToastType,
                duration: 0,
                progress: pct,
              },
            ];
          });
        } else {
          // 快照不可用（invoke 失败/后端已结束）但窗口已被后端 show：仍建一条占位
          // toast 触发 hasContent → show，避免「窗口显示了却没内容」
          setItems((prev) => {
            if (prev.some((it) => it.taskKey === '启动')) return prev;
            return [
              ...prev,
              {
                id: nextId++,
                taskKey: '启动',
                message: '正在启动…',
                type: 'info' as ToastType,
                duration: 0,
                progress: 0,
              },
            ];
          });
        }
      }

      if (cancelled) {
        await unlistenShow();
        await unlistenConfirm();
        await unlistenDone();
        await unlistenEmbedProgress();
        await unlistenStartupProgress();
        return;
      }

      unlistens.push(unlistenShow, unlistenConfirm, unlistenDone, unlistenEmbedProgress, unlistenStartupProgress);
      void emit('toast:ready', { character_id: myCharId });
    })();

    return () => {
      cancelled = true;
      if (startupThrottle.timer !== null) {
        window.clearTimeout(startupThrottle.timer);
        startupThrottle.timer = null;
      }
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
  // - 无 confirm 且无带操作按钮的 toast：窗口穿透（toast 不需点击），恢复全屏高度右缘窗口（内容底对齐）
  // - 有 confirm 或带操作按钮的 toast（如主题切换确认）：关闭穿透让按钮可点击，
  //   窗口收缩到实测内容高度并贴右下角，消除透明区域对背景的遮挡（高度可变，用 ResizeObserver 实测）
  const hasActionable = items.some((it) => it.action != null);
  const needsInteraction = confirms.length > 0 || hasActionable;
  useEffect(() => {
    const win = getCurrentWindow();
    void win.setIgnoreCursorEvents(!needsInteraction).catch(() => {});
    if (needsInteraction) {
      const apply = () => {
        const el = contentRef.current;
        if (!el) return;
        const contentH = Math.max(el.scrollHeight + 40, 120);
        void currentMonitor().then((monitor) => {
          if (!monitor) return;
          const factor = monitor.scaleFactor;
          const screenW = monitor.size.width / factor;
          const screenH = monitor.size.height / factor;
          void win.setSize(new LogicalSize(400, contentH)).catch(() => {});
          void win.setPosition(new LogicalPosition(screenW - 400, screenH - contentH)).catch(() => {});
        }).catch(() => {});
      };
      apply();
      const ro = new ResizeObserver(apply);
      if (contentRef.current) ro.observe(contentRef.current);
      return () => ro.disconnect();
    }
    // 无需交互时恢复全屏高度窗口（供普通 toast 使用）
    void currentMonitor().then((monitor) => {
      if (monitor) {
        const screenW = monitor.size.width / monitor.scaleFactor;
        const screenH = monitor.size.height / monitor.scaleFactor;
        void win.setSize(new LogicalSize(400, screenH)).catch(() => {});
        void win.setPosition(new LogicalPosition(screenW - 400, 0)).catch(() => {});
      }
    }).catch(() => {});
  }, [needsInteraction]);

  const removeItem = (id: number) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
  };

  /** toast 操作按钮：一键切换界面主题（写配置 + 持久化 + 广播所有窗口） */
  const handleToastAction = async (it: ToastItem) => {
    const action = it.action;
    if (!action || action.kind !== 'switch_theme') return;
    if (action.theme !== 'light' && action.theme !== 'dark') return;
    try {
      await invoke('set_config', { key: 'base.theme', value: action.theme });
      await invoke('save_config');
      await emit('config:theme-changed', { theme: action.theme });
      removeItem(it.id);
    } catch (e) {
      console.warn('[ToastWindow] 切换主题失败:', e);
    }
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
        // startup 窗口锚定顶部（屏幕右上角区域），角色窗口锚定底部（右下角区域），
        // 两个窗口的内容错开，预启动 toast 与角色 toast 永不互相遮盖
        justifyContent: myCharId === 'startup' ? 'flex-start' : 'flex-end',
        padding: 20,
        pointerEvents: 'none',
      }}
    >
      <div
        ref={contentRef}
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'flex-end',
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
            charId={c.charId}
            args={c.args}
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
            action={it.action}
            onAction={it.action ? () => void handleToastAction(it) : undefined}
            onClose={() => removeItem(it.id)}
          />
        ))}
      </div>
    </div>
  );
}
