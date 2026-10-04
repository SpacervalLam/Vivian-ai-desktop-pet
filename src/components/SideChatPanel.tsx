import { stripActions } from '../utils/ActionText';
import React, { useEffect, useState, useRef, useCallback } from 'react';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import InputDialog from './InputDialog';

interface ChatEntry {
  id: number;
  role: 'user' | 'assistant';
  text: string;
  timestamp: number;
  characterId?: string;
  /** 流式进行中：文本持续追加，done 到达后定型 */
  streaming?: boolean;
  /** 关联的流式会话 ID，用于 chunk/done 匹配 */
  streamId?: string;
}

const MAX_ENTRIES = 12;

const CHAR_NAMES: Record<string, string> = {
  vivian: 'Vivian',
  nana: 'Nana',
};

function charName(id?: string): string {
  if (!id) return '';
  return CHAR_NAMES[id] ?? id.charAt(0).toUpperCase() + id.slice(1);
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, '0');
  const mm = d.getMinutes().toString().padStart(2, '0');
  return `${hh}:${mm}`;
}

export default function SideChatPanel() {
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [inputVisible, setInputVisible] = useState(false);
  const [autoStartVoice, setAutoStartVoice] = useState(false);
  const [activeChar, setActiveChar] = useState<string | null>(null);
  const [broadcast, setBroadcast] = useState(false);
  const [locked, setLocked] = useState(false);
  const [overflowed, setOverflowed] = useState(false);
  const idCounterRef = useRef(0);
  const activeCharRef = useRef<string | null>(null);
  const inputRegionRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  // 跟踪用户是否处于底部附近（用于新消息到达时判断是否自动跟随）
  // column-reverse 容器中 scrollTop=0 即最底部，向上翻阅历史时值增大
  const atBottomRef = useRef(true);

  // 输入态与输入区域同步给 Rust；区域外继续使用原生鼠标穿透。
  useEffect(() => {
    void invoke('set_side_chat_input_open', { open: inputVisible, label: 'side_chat' }).catch(() => {});
  }, [inputVisible]);

  useEffect(() => {
    const region = inputRegionRef.current;
    const update = () => {
      const rect = region?.getBoundingClientRect();
      void invoke<boolean>('set_side_chat_input_region', {
        rect: rect ? [rect.left, rect.top, rect.right, rect.bottom] : null,
      }).then(setLocked).catch(() => {});
    };
    update();
    if (!region) return;
    const observer = new ResizeObserver(update);
    observer.observe(region);
    if (contentRef.current) observer.observe(contentRef.current);
    scrollRef.current?.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    return () => {
      observer.disconnect();
      scrollRef.current?.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
      void invoke<boolean>('set_side_chat_input_region', { rect: null }).catch(() => {});
    };
  }, [inputVisible, overflowed]);

  // 原生线程处理悬浮快捷键；阻止输入焦点下 Tab 的默认导航和 Esc 的旧关闭语义。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Tab' || event.key === 'Escape') event.preventDefault();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  // 检测消息内容是否超出列表高度：未超出时整组内容垂直居中，超出时锚定底部
  useEffect(() => {
    const scrollEl = scrollRef.current;
    const contentEl = contentRef.current;
    if (!scrollEl || !contentEl) return;
    const update = () => {
      setOverflowed(contentEl.scrollHeight > scrollEl.clientHeight + 1);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(scrollEl);
    ro.observe(contentEl);
    return () => ro.disconnect();
  }, []);

  // 新消息到达 / 流式追加时自动跟随到底部
  // column-reverse 容器的滚动原点在最底部，scrollTop=0 即展示最新消息；
  // 仅当用户未向上翻阅历史时跟随，避免打断阅读（与 ChatWindow 策略一致）
  useEffect(() => {
    if (!atBottomRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [entries, overflowed]);

  const addEntry = useCallback((role: 'user' | 'assistant', text: string, characterId?: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    setEntries((prev) => {
      // 去重：同一角色 3 秒内相同内容只保留一条
      // 防止后端 chat:assistant_message 与前端非流式路径 emit 同时到达导致重复
      const now = Date.now();
      const last = prev[prev.length - 1];
      if (
        last &&
        last.role === role &&
        last.characterId === characterId &&
        last.text === trimmed &&
        now - last.timestamp < 3000
      ) {
        return prev;
      }
      const newEntry: ChatEntry = {
        id: now + idCounterRef.current++,
        role,
        text: trimmed,
        timestamp: now,
        characterId,
      };
      const updated = [...prev, newEntry];
      return updated.slice(-MAX_ENTRIES);
    });
  }, []);

  useEffect(() => {
    const unlistens: UnlistenFn[] = [];
    let cancelled = false;

    const setupListeners = async () => {
      // 接收所有角色的助手消息（侧边栏不显示用户消息）
      // 微信渠道(channel=wechat)的消息由 ChatWindow 独占显示，不在此重复
      // 流式消息（带 stream_id）由 chat:chunk / chat:done 处理，此处跳过避免重复
      const un2 = await listen<{ content: string; character_id?: string; channel?: string; stream_id?: string }>(
        'chat:assistant_message',
        (e) => {
          if (!e.payload) return;
          if (e.payload.channel === 'wechat') return;
          if (e.payload.stream_id) return;
          addEntry('assistant', e.payload.content, e.payload.character_id);
        }
      );
      if (cancelled) { un2(); return; }
      unlistens.push(un2);

      // chat:chunk：流式累积文本到对应 stream_id 的条目
      // 与 ChatWindow 的分段多气泡策略不同，侧边栏在单条气泡内逐步追加文本
      const unChunk = await listen<{ text: string; stream_id?: string; character_id?: string; channel?: string }>(
        'chat:chunk',
        (e) => {
          if (!e.payload) return;
          if (e.payload.channel === 'wechat') return;
          const sid = e.payload.stream_id;
          if (!sid) return;
          const chunk = e.payload.text;
          const cid = e.payload.character_id;
          setEntries((prev) => {
            const idx = prev.findIndex((en) => en.streamId === sid);
            if (idx >= 0) {
              const updated = [...prev];
              updated[idx] = { ...updated[idx], text: updated[idx].text + chunk };
              return updated;
            }
            const newEntry: ChatEntry = {
              id: Date.now() + idCounterRef.current++,
              role: 'assistant',
              text: chunk,
              timestamp: Date.now(),
              characterId: cid,
              streaming: true,
              streamId: sid,
            };
            const updated = [...prev, newEntry];
            return updated.slice(-MAX_ENTRIES);
          });
        }
      );
      if (cancelled) { unChunk(); return; }
      unlistens.push(unChunk);

      // chat:done：定型对应 stream_id 的流式条目
      const unDone = await listen<{ text: string; stream_id?: string; character_id?: string; channel?: string }>(
        'chat:done',
        (e) => {
          if (!e.payload) return;
          if (e.payload.channel === 'wechat') return;
          const sid = e.payload.stream_id;
          if (!sid) return;
          const finalText = (e.payload.text || '').trim();
          const cid = e.payload.character_id;
          setEntries((prev) => {
            const idx = prev.findIndex((en) => en.streamId === sid);
            if (idx < 0) {
              // 流式条目缺失（chunk 未到达就 done），直接创建定型条目
              if (!finalText) return prev;
              const newEntry: ChatEntry = {
                id: Date.now() + idCounterRef.current++,
                role: 'assistant',
                text: finalText,
                timestamp: Date.now(),
                characterId: cid,
              };
              const updated = [...prev, newEntry];
              return updated.slice(-MAX_ENTRIES);
            }
            const updated = [...prev];
            updated[idx] = {
              ...updated[idx],
              text: finalText || updated[idx].text,
              streaming: false,
              streamId: undefined,
            };
            return updated;
          });
        }
      );
      if (cancelled) { unDone(); return; }
      unlistens.push(unDone);

      // 显示输入框（携带角色 ID 或广播标志）
      const un3 = await listen<{ character_id?: string; auto_start_voice?: boolean; broadcast?: boolean }>(
        'sidechat:show_input',
        (e) => {
          if (e.payload?.broadcast) {
            setBroadcast(true);
          } else {
            setBroadcast(false);
            if (e.payload?.character_id) {
              activeCharRef.current = e.payload.character_id;
              setActiveChar(e.payload.character_id);
            }
          }
          setAutoStartVoice(e.payload?.auto_start_voice ?? false);
          setInputVisible(true);
        }
      );
      if (cancelled) { un3(); return; }
      unlistens.push(un3);

      // 锁定状态同步（Rust 边缘线程呼出时复位 / Tab 或快捷键设置）
      const un4 = await listen<{ locked: boolean; label?: string }>(
        'sidechat:lock_changed',
        (e) => {
          if (e.payload?.label !== 'side_chat') return;
          setLocked(!!e.payload?.locked);
        }
      );
      if (cancelled) { un4(); return; }
      unlistens.push(un4);

      // 边缘呼出时清理上次残留的输入态（Rust 线程在 show 后广播）
      const un5 = await listen<{ label?: string }>('sidechat:input_reset', (e) => {
        if (e.payload?.label !== 'side_chat') return;
        setInputVisible(false);
        setAutoStartVoice(false);
      });
      if (cancelled) { un5(); return; }
      unlistens.push(un5);

      const params = new URLSearchParams(window.location.search);
      const initChar = params.get('active_character');
      if (initChar) {
        activeCharRef.current = initChar;
        setActiveChar(initChar);
      }
      if (params.get('show_input') === '1') {
        setAutoStartVoice(params.get('auto_voice') === '1');
        setInputVisible(true);
      }
    };

    void setupListeners();

    return () => {
      cancelled = true;
      unlistens.forEach((fn) => fn());
    };
  }, [addEntry]);

  const handleSend = useCallback(async (text: string, whisper?: boolean) => {
    try {
      void emit('sidechat:send_message', {
        text,
        character_id: broadcast ? undefined : (activeCharRef.current ?? undefined),
        whisper: whisper ?? false,
      });
    } catch {
      /* ignore */
    }
  }, [broadcast]);

  const handleCloseInput = useCallback(() => {
    setInputVisible(false);
    setAutoStartVoice(false);
  }, []);

  return (
    <div
      title={locked ? '已锁定 · 悬浮按 Tab 解锁 · Esc 收起' : '已解锁 · 悬浮按 Tab 锁定 · Esc 收起'}
      style={{
        width: '100%',
        height: '100vh',
        display: 'flex',
        flexDirection: 'column',
        backgroundColor: 'transparent',
        padding: '12px 10px 12px 22px',
        boxSizing: 'border-box',
        position: 'relative',
        pointerEvents: 'none',
      }}
    >
      <style>{`@keyframes sidechat-enter { 0% { opacity: 0; transform: translateY(10px) scale(0.95); } 60% { opacity: 1; transform: translateY(0) scale(1.025); } 100% { opacity: 1; transform: translateY(0) scale(1); } } .sidechat-scroll::-webkit-scrollbar { display: none; }`}</style>
      <span aria-live="polite" style={{ position: 'absolute', top: 8, right: 10, fontSize: 10, color: 'rgba(255,255,255,0.5)' }}>
        {locked ? '已锁定' : '已解锁'} · Tab 切换 · Esc 收起
      </span>

      <div
        ref={scrollRef}
        className="sidechat-scroll"
        onScroll={(e) => {
          atBottomRef.current = e.currentTarget.scrollTop <= 24;
        }}
        style={{
          flex: 1,
          minHeight: 0,
          overflowY: 'auto',
          overflowX: 'hidden',
          scrollbarWidth: 'none',
          // 禁用滚动锚定：新消息/流式追加时防止浏览器把视口固定在旧消息上
          overflowAnchor: 'none',
          display: 'flex',
          flexDirection: 'column-reverse',
          justifyContent: overflowed ? 'flex-start' : 'center',
          paddingRight: 2,
          // 顶部渐变遮罩：靠近上缘逐渐透明，直到边缘完全透明
          WebkitMaskImage:
            'linear-gradient(to bottom, transparent 0, rgba(0,0,0,0.45) 28px, #000 56px)',
          maskImage:
            'linear-gradient(to bottom, transparent 0, rgba(0,0,0,0.45) 28px, #000 56px)',
        }}
      >
        <div
          ref={contentRef}
          style={{ display: 'flex', flexDirection: 'column-reverse', gap: 10 }}
        >
          {inputVisible && (
            <div ref={inputRegionRef} style={{ marginBottom: 6, pointerEvents: 'auto' }}>
              <InputDialog
                sideChat
                broadcast={broadcast}
                characterId={activeChar ?? undefined}
                onSend={handleSend}
                onClose={handleCloseInput}
                visible={inputVisible}
                autoStartVoice={autoStartVoice}
              />
            </div>
          )}
          {[...entries].reverse().filter((entry) => entry.role === 'user' || stripActions(entry.text).trim()).map((entry) => (
          <div
            key={entry.id}
            style={{
              alignSelf: 'flex-start',
              maxWidth: '92%',
              transformOrigin: 'left bottom',
              animation: 'sidechat-enter 0.26s cubic-bezier(0.22, 1, 0.36, 1)',
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'baseline',
                gap: 6,
                marginBottom: 2,
                paddingLeft: 6,
              }}
            >
              <span style={{ fontSize: 10, color: 'rgba(255, 255, 255, 0.5)' }}>
                {charName(entry.characterId)}
              </span>
              <span style={{ fontSize: 10, color: 'rgba(255, 255, 255, 0.35)' }}>
                {formatTime(entry.timestamp)}
              </span>
            </div>
            <div
              style={{
                backgroundColor: 'rgba(45, 45, 55, 0.75)',
                color: '#fff',
                borderRadius: '4px 18px 18px 18px',
                padding: '10px 14px',
                fontSize: 13,
                lineHeight: 1.5,
                wordBreak: 'break-word',
                boxShadow: '0 2px 8px rgba(0, 0, 0, 0.25)',
                backdropFilter: 'blur(4px)',
                WebkitBackdropFilter: 'blur(4px)',
              }}
            >
              {entry.role === 'user' ? entry.text : stripActions(entry.text)}
            </div>
          </div>
        ))}
        </div>
      </div>
    </div>
  );
}
