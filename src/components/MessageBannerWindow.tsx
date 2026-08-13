/**
 * 微信消息横幅窗口 - 类似 iPhone 消息横幅，在屏幕上方居中显示。
 *
 * 后端通过 `wechat:message_banner` 事件触发：
 * - payload: { character_id, preview, kind?, timestamp? }
 *
 * 横幅显示头像 + 角色昵称 + 消息预览，点击后调用 show_side_chat_animated 打开微信窗口。
 * 同一角色的多条消息合并到单个横幅，预览前加 [x条] 计数前缀；链接消息加 [Link] 前缀。
 */

import { useEffect, useState, useCallback, useRef } from 'react';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { currentMonitor } from '@tauri-apps/api/window';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

interface BannerPayload {
  character_id: string;
  preview: string;
  kind?: string;
  timestamp?: number;
}

interface BannerItem {
  id: number;
  character_id: string;
  name: string;
  avatarSrc: string;
  /** 累计消息条数（同一角色合并计数） */
  count: number;
  /** 最新一条消息的预览文本 */
  preview: string;
  /** 最新一条是否为链接卡片 */
  isLink: boolean;
}

interface CharacterInfo {
  id: string;
  name: string;
}

let nextId = 1;

const capitalizeCharId = (id: string) => {
  if (!id) return id;
  return id.charAt(0).toUpperCase() + id.slice(1);
};

let audioCtx: AudioContext | null = null;
function playNotificationSound() {
  try {
    if (!audioCtx) {
      const Ctor = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      audioCtx = new Ctor();
    }
    if (audioCtx.state === 'suspended') void audioCtx.resume();
    const ctx = audioCtx;
    const now = ctx.currentTime;
    const master = ctx.createGain();
    master.gain.value = 0.25;
    master.connect(ctx.destination);

    const playTone = (freq: number, start: number, dur: number) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.frequency.value = freq;
      osc.type = 'sine';
      gain.gain.setValueAtTime(0, start);
      gain.gain.linearRampToValueAtTime(1, start + 0.008);
      gain.gain.exponentialRampToValueAtTime(0.0001, start + dur);
      osc.connect(gain).connect(master);
      osc.start(start);
      osc.stop(start + dur);
    };

    playTone(880, now, 0.18);
    playTone(1320, now + 0.12, 0.28);
  } catch { /* ignore */ }
}

export default function MessageBannerWindow() {
  const [items, setItems] = useState<BannerItem[]>([]);
  const [charMap, setCharMap] = useState<Map<string, CharacterInfo>>(new Map());

  // 主题
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute(
        'data-theme',
        theme === 'light' || theme === 'dark' ? theme : 'system',
      );
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
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 加载角色列表
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const result = await invoke<{
          active_id: string;
          characters: Array<{ id: string; name: string; online: boolean }>;
        }>('list_characters');
        if (cancelled) return;
        const map = new Map<string, CharacterInfo>();
        for (const c of result.characters) {
          map.set(c.id, { id: c.id, name: c.name });
        }
        setCharMap(map);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const removeItem = useCallback((id: number) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
  }, []);

  // 每个角色横幅的自动消失定时器（合并新消息时重置，给最新消息完整展示时间）
  const bannerTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const scheduleRemoval = useCallback((character_id: string, id: number) => {
    const timers = bannerTimers.current;
    const existing = timers.get(character_id);
    if (existing) clearTimeout(existing);
    const timer = setTimeout(() => {
      timers.delete(character_id);
      removeItem(id);
    }, 5000);
    timers.set(character_id, timer);
  }, [removeItem]);

  // 监听 wechat:message_banner 事件
  // 同一角色的多条消息合并到单个横幅，不堆叠多个横幅
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void (async () => {
      unlisten = await listen<BannerPayload>('wechat:message_banner', (e) => {
        const p = e.payload;
        if (!p?.character_id) return;
        playNotificationSound();
        const info = charMap.get(p.character_id);
        const name = info?.name ?? p.character_id;
        const avatarSrc = `/${capitalizeCharId(p.character_id)}/icon.png`;
        const isLink = p.kind === 'link_card';
        setItems((prev) => {
          const existingIdx = prev.findIndex((it) => it.character_id === p.character_id);
          if (existingIdx >= 0) {
            // 合并：计数 +1，更新预览为最新一条，更新 isLink
            const existing = prev[existingIdx];
            const merged: BannerItem = {
              ...existing,
              count: existing.count + 1,
              preview: p.preview,
              isLink,
            };
            const next = [...prev];
            next[existingIdx] = merged;
            scheduleRemoval(p.character_id, merged.id);
            return next;
          }
          // 新建横幅
          const item: BannerItem = {
            id: nextId++,
            character_id: p.character_id,
            name,
            avatarSrc,
            count: 1,
            preview: p.preview,
            isLink,
          };
          scheduleRemoval(p.character_id, item.id);
          return [...prev, item];
        });
      });
      if (cancelled) {
        unlisten();
        unlisten = undefined;
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      // 清理所有定时器
      bannerTimers.current.forEach((t) => clearTimeout(t));
      bannerTimers.current.clear();
    };
  }, [charMap, removeItem, scheduleRemoval]);

  // 窗口可见性：有横幅时显示，全部清除后隐藏
  const hasContent = items.length > 0;
  useEffect(() => {
    const win = getCurrentWindow();
    if (hasContent) {
      void win.show().catch(() => {});
    } else {
      void win.hide().catch(() => {});
    }
  }, [hasContent]);

  // 窗口定位：屏幕上方居中
  useEffect(() => {
    void (async () => {
      try {
        const monitor = await currentMonitor();
        if (!monitor) return;
        const factor = monitor.scaleFactor;
        const screenW = monitor.size.width / factor;
        const winW = 380;
        const x = Math.max(8, (screenW - winW) / 2);
        const y = 12;
        const win = getCurrentWindow();
        await win.setPosition(new (await import('@tauri-apps/api/window')).LogicalPosition(x, y));
      } catch {
        /* ignore */
      }
    })();
  }, []);

  // 点击横幅：打开 ChatWindow 微信窗口并跳转到对应私聊
  const handleBannerClick = useCallback(async (item: BannerItem) => {
    const targetCharId = item.character_id;
    const chatLabel = 'chat';

    // 1. 如果已有该 chat 窗口：聚焦 + 发 navigate 事件切换到私聊
    let existingWin: WebviewWindow | null = null;
    try {
      existingWin = await WebviewWindow.getByLabel(chatLabel);
    } catch {
      existingWin = null;
    }

    if (existingWin) {
      try {
        await existingWin.show();
        await existingWin.setFocus();
      } catch {
        /* ignore */
      }
      // 通知 ChatWindow 跳转到指定角色私聊（emit 到全部窗口，由 ChatWindow 内部监听 chatwindow:navigate）
      void emit('chatwindow:navigate', { character_id: targetCharId, view: 'private' });
      setItems([]);
      return;
    }

    // 2. 不存在则创建 ChatWindow，URL 携带 private_char_id 自动跳转
    try {
      const params = new URLSearchParams();
      params.set('view', 'chat');
      if (targetCharId) params.set('private_char_id', targetCharId);
      const win = new WebviewWindow(chatLabel, {
        url: `/?${params.toString()}`,
        title: 'Chat',
        width: 390,
        height: 845,
        resizable: false,
        decorations: false,
        transparent: true,
        shadow: false,
        center: true,
        visible: false,
      });
      win.once('tauri://created', () => {
        void win.show().catch(() => {});
      });
    } catch {
      // 创建失败兜底：仅清空横幅
    }
    setItems([]);
  }, []);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        overflow: 'hidden',
        background: 'transparent',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'flex-start',
        padding: 0,
        gap: 8,
        pointerEvents: 'none',
      }}
    >
      {items.map((it) => (
        <div
          key={it.id}
          onClick={() => handleBannerClick(it)}
          style={{
            pointerEvents: 'auto',
            width: 360,
            background: 'var(--panel-surface, rgba(40, 40, 48, 0.95))',
            backdropFilter: 'blur(16px)',
            border: '1px solid var(--panel-border-strong, rgba(255,255,255,0.12))',
            borderRadius: 14,
            boxShadow: '0 8px 32px rgba(0,0,0,0.35)',
            padding: '10px 14px',
            display: 'flex',
            alignItems: 'center',
            gap: 12,
            cursor: 'pointer',
            color: 'var(--panel-text, #fff)',
            transition: 'transform 0.2s ease, box-shadow 0.2s ease',
            animation: 'bannerSlideIn 0.28s cubic-bezier(0.2, 0.8, 0.2, 1)',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.transform = 'translateY(-1px)';
            e.currentTarget.style.boxShadow = '0 10px 36px rgba(0,0,0,0.45)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.transform = 'translateY(0)';
            e.currentTarget.style.boxShadow = '0 8px 32px rgba(0,0,0,0.35)';
          }}
        >
          <img
            src={it.avatarSrc}
            alt={it.name}
            draggable={false}
            onError={(e) => {
              (e.currentTarget as HTMLImageElement).src = '/favicon.ico';
            }}
            style={{
              width: 38,
              height: 38,
              borderRadius: 10,
              flexShrink: 0,
              objectFit: 'cover',
            }}
          />
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
            <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--panel-text, #fff)' }}>
              {it.name}
            </span>
            <div
              style={{
                fontSize: 12,
                color: 'var(--panel-text-secondary, rgba(255,255,255,0.65))',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              {it.count > 1 && `[${it.count}条]`}
              {it.isLink && '[Link]'}
              {it.count > 1 || it.isLink ? ' ' : ''}
              {it.preview}
            </div>
          </div>
        </div>
      ))}
      <style>{`
        @keyframes bannerSlideIn {
          from { opacity: 0; transform: translateY(-12px); }
          to { opacity: 1; transform: translateY(0); }
        }
      `}</style>
    </div>
  );
}
