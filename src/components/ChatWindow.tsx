import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import DOMPurify from 'dompurify';
import { useVirtualizer } from '@tanstack/react-virtual';
import i18n, { changeLanguage } from '../i18n';
import LoadingSpinner from './LoadingSpinner';
import { ChatController } from '../controllers/ChatController';
import { resolveAvatarUrl } from '../characterContext';
import RealtimeCallOverlay, { RealtimeCallBubble } from './RealtimeCallWindow';
import ImageViewer from './ImageViewer';
import { useAppStore } from '../stores/useAppStore';
import { useExtractFileText, type FileTextResult } from '../hooks/useTauriCommands';
import { stripActions } from '../utils/ActionText';
import type { AiResponse } from '../types';

type Role = 'user' | 'assistant';

interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  timestamp: number;
  /** 流式生成中：气泡显示打字动画/光标 */
  streaming?: boolean;
  /** 生成出错：气泡显示错误样式 */
  error?: boolean;
  /** 图片消息：立即可用的 data URL（刚发送时由事件携带） */
  imageDataUrl?: string;
  /** 图片消息：相对用户数据目录的图片路径（历史记录加载时据此懒加载 data URL） */
  imagePath?: string;
  /** 表情包消息：表情包名称（非空时渲染为表情包图片，不渲染文本气泡） */
  sticker?: string;
  /** 群聊视图：AI 消息来源角色 ID（用于显示发送者名称） */
  character_id?: string;
  /** 链接卡片：微信风格链接分享 */
  linkCard?: {
    url: string;
    title: string;
    description?: string;
    source?: string;
  };
  /** 文件消息：用户上传的文本/PDF 文件，内容可能很长，需折叠展示 */
  fileMeta?: {
    fileName: string;
    fileType: string;
    truncated: boolean;
    originalCharCount: number;
  };
  /** 语音消息：原始音频用于聊天界面播放（发送给 LLM 的仍是 ASR 转写文本） */
  voice?: {
    /** 相对用户数据目录的音频路径（历史记录加载时据此懒加载 data URL） */
    audioPath?: string;
    /** 立即可用的 data URL（刚发送时由前端录制直接生成） */
    audioDataUrl?: string;
    /** 语音时长（秒） */
    duration: number;
  };
}

/** 图片 data URL LRU 缓存（按 imagePath 索引），避免历史刷新时重复读取。
 *  限制 50 条，超出时淘汰最久未访问的条目，防止长时间使用后内存无限增长。 */
const IMAGE_CACHE_MAX = 50;
const imageCache = new Map<string, string>();
function imageCacheGet(key: string): string | undefined {
  const v = imageCache.get(key);
  if (v !== undefined) {
    // 重新插入到末尾，标记为最近使用
    imageCache.delete(key);
    imageCache.set(key, v);
  }
  return v;
}
function imageCacheSet(key: string, value: string): void {
  if (imageCache.has(key)) imageCache.delete(key);
  imageCache.set(key, value);
  while (imageCache.size > IMAGE_CACHE_MAX) {
    const oldest = imageCache.keys().next().value;
    if (oldest === undefined) break;
    imageCache.delete(oldest);
  }
}

/** 语音 data URL LRU 缓存（按 audioPath 索引），避免历史刷新时重复读取。
 *  限制 20 条（音频 data URL 体积大于图片），超出时淘汰最久未访问的条目。 */
const AUDIO_CACHE_MAX = 20;
const audioCache = new Map<string, string>();
function audioCacheGet(key: string): string | undefined {
  const v = audioCache.get(key);
  if (v !== undefined) {
    audioCache.delete(key);
    audioCache.set(key, v);
  }
  return v;
}
function audioCacheSet(key: string, value: string): void {
  if (audioCache.has(key)) audioCache.delete(key);
  audioCache.set(key, value);
  while (audioCache.size > AUDIO_CACHE_MAX) {
    const oldest = audioCache.keys().next().value;
    if (oldest === undefined) break;
    audioCache.delete(oldest);
  }
}

/** 联动卡片消息：薇薇安推送的待办/定时任务提醒 */
interface CardMessage {
  id: string;
  cardType: 'todo' | 'scheduler';
  action: string;
  title: string;
  subtitle: string;
  timestamp: number;
}

interface HistoryEntry {
  id: string;
  role: string;
  content: string;
  timestamp: number | string;
  session_id?: string;
  metadata?: Record<string, unknown>;
}

const PAGE_SIZE = 20;
const SCROLL_LOAD_THRESHOLD = 100;
const SCROLL_DEBOUNCE_MS = 200;
const TIME_GAP_MS = 5 * 60 * 1000;

let idCounter = 0;
const nextId = () => `m-${Date.now()}-${idCounter++}`;

/**
 * 把录制的音频 Blob 解码并重采样到 16kHz 单声道 f32 PCM，返回 base64 编码的字节流。
 * 用于非 WinRT ASR 引擎的文件转写路径（transcribe_audio 命令接收 base64 f32 PCM）。
 */
async function audioBlobToBase64F32(blob: Blob): Promise<string> {
  const arrayBuffer = await blob.arrayBuffer();
  const tmpCtx = new AudioContext();
  try {
    const decoded = await tmpCtx.decodeAudioData(arrayBuffer);
    const targetRate = 16000;
    const offlineCtx = new OfflineAudioContext(
      1,
      Math.max(1, Math.ceil(decoded.duration * targetRate)),
      targetRate,
    );
    const src = offlineCtx.createBufferSource();
    src.buffer = decoded;
    src.connect(offlineCtx.destination);
    src.start();
    const rendered = await offlineCtx.startRendering();
    const f32 = rendered.getChannelData(0);
    const bytes = new Uint8Array(f32.buffer);
    let binary = '';
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + chunk)) as unknown as number[]);
    }
    return btoa(binary);
  } finally {
    tmpCtx.close();
  }
}

const normalizeTimestamp = (raw: number | string): number => {
  if (typeof raw === 'number') return raw < 1e12 ? raw * 1000 : raw;
  const t = Date.parse(raw);
  return Number.isNaN(t) ? Date.now() : t;
};

const toChatMessages = (e: HistoryEntry): ChatMessage[] => {
  const isImage = e.metadata?.kind === 'image';
  const imagePath = typeof e.metadata?.image_path === 'string' ? e.metadata.image_path : undefined;
  const role = (e.role === 'user' ? 'user' : 'assistant') as Role;
  const ts = normalizeTimestamp(e.timestamp);

  // 链接卡片消息：metadata 中有 link_card 字段
  const linkCard = e.metadata?.link_card as { url: string; title: string; description?: string; source?: string } | undefined;
  if (linkCard && linkCard.url && linkCard.title) {
    return [{
      id: e.id,
      role,
      content: '',
      timestamp: ts,
      linkCard: {
        url: linkCard.url,
        title: linkCard.title,
        description: linkCard.description || '',
        source: linkCard.source || '',
      },
    }];
  }

  if (isImage) {
    return [{
      id: e.id,
      role,
      content: e.content,
      timestamp: ts,
      imagePath,
      imageDataUrl: imageCacheGet(imagePath || ''),
    }];
  }

  // 语音消息：metadata.kind === 'voice'，渲染为微信风格语音气泡，不按行拆分
  const isVoice = e.metadata?.kind === 'voice';
  if (isVoice) {
    const audioPath = typeof e.metadata?.audio_path === 'string' ? e.metadata.audio_path : undefined;
    const duration = typeof e.metadata?.duration === 'number' ? e.metadata.duration : 0;
    return [{
      id: e.id,
      role,
      content: e.content,
      timestamp: ts,
      voice: {
        audioPath,
        duration,
      },
    }];
  }

  // 文件消息：内容可能为长文本（PDF/文本文件提取），不按行拆分，
  // 作为单条消息渲染并由 Bubble 折叠展示，避免拆成大量 ChatMessage 导致渲染卡顿。
  const isFile = e.metadata?.kind === 'file';
  if (isFile) {
    return [{
      id: e.id,
      role,
      content: e.content,
      timestamp: ts,
      fileMeta: {
        fileName: typeof e.metadata?.file_name === 'string' ? e.metadata.file_name : '',
        fileType: typeof e.metadata?.file_type === 'string' ? e.metadata.file_type : '',
        truncated: e.metadata?.truncated === true,
        originalCharCount: typeof e.metadata?.original_char_count === 'number' ? e.metadata.original_char_count : 0,
      },
    }];
  }

  const lines = e.content.split('\n').map((l) => l.trim()).filter((l) => l.length > 0);
  // 后端持久化的 sticker：从 metadata 读取，生成独立表情包气泡追加在文本之后
  // timestamp +0.5 使其在同一条消息的文本段落之后排序（不与文本段落的 ts+i 冲突）
  const stickerFromMeta = typeof e.metadata?.sticker === 'string' && e.metadata.sticker
    ? e.metadata.sticker
    : '';
  if (lines.length <= 1) {
    const result: ChatMessage[] = [{
      id: e.id,
      role,
      content: e.content,
      timestamp: ts,
    }];
    if (stickerFromMeta) {
      result.push({
        id: `${e.id}#sticker`,
        role,
        content: '',
        timestamp: ts + 0.5,
        sticker: stickerFromMeta,
      });
    }
    return result;
  }

  const segs: ChatMessage[] = lines.map((line, i) => ({
    id: i === 0 ? e.id : `${e.id}#seg${i}`,
    role,
    content: line,
    timestamp: ts + i,
  }));
  if (stickerFromMeta) {
    segs.push({
      id: `${e.id}#sticker`,
      role,
      content: '',
      timestamp: ts + lines.length,
      sticker: stickerFromMeta,
    });
  }
  return segs;
};

const formatSeparatorTime = (
  ts: number,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string => {
  const d = new Date(ts);
  const now = new Date();
  const hh = d.getHours().toString().padStart(2, '0');
  const mm = d.getMinutes().toString().padStart(2, '0');
  const time = `${hh}:${mm}`;
  const isSameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate();
  if (isSameDay(d, now)) return time;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (isSameDay(d, yesterday)) return t('chat.time_yesterday', { time });
  const beforeYesterday = new Date(now);
  beforeYesterday.setDate(now.getDate() - 2);
  if (isSameDay(d, beforeYesterday)) return t('chat.time_before_yesterday', { time });
  const month = (d.getMonth() + 1).toString().padStart(2, '0');
  const day = d.getDate().toString().padStart(2, '0');
  if (d.getFullYear() === now.getFullYear()) return t('chat.time_md', { month, day, time });
  return t('chat.time_ymd', { year: d.getFullYear(), month, day, time });
};

const escapeHtml = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');

const renderMarkdown = (text: string): string => {
  const filteredText = stripActions(text);
  // 超长文本快速路径：跳过 markdown 解析和 DOMPurify（对大字符串耗时显著），
  // 直接转义并转换换行，避免阻塞主线程导致界面卡顿/未响应。
  const MAX_MARKDOWN_LEN = 2000;
  if (filteredText.length > MAX_MARKDOWN_LEN) {
    return escapeHtml(filteredText).replace(/\n/g, '<br/>');
  }
  const codeBlocks = new Map<string, string>();
  let working = filteredText.replace(/```([\s\S]*?)```/g, (_m, code: string) => {
    const key = `\u0000CB${codeBlocks.size}\u0000`;
    const lines = code.replace(/^\n/, '').replace(/\n$/, '');
    codeBlocks.set(key, `<pre class="md-code-block"><code>${escapeHtml(lines)}</code></pre>`);
    return key;
  });
  working = escapeHtml(working);
  working = working.replace(/`([^`\n]+)`/g, (_m, c: string) => `<code class="md-code-inline">${c}</code>`);
  working = working.replace(/\*\*([^*]+)\*\*/g, '$1');
  working = working.replace(/(^|[^*])\*([^*]+)\*/g, '$1$2');
  working = working.replace(/__/g, '');
  working = working.replace(/\n/g, '<br/>');
  codeBlocks.forEach((html, key) => { working = working.split(key).join(html); });
  // 最终 sanitization：白名单只允许 markdown 生成的标签，class 属性默认放行用于样式
  return DOMPurify.sanitize(working, {
    ALLOWED_TAGS: ['pre', 'code', 'br'],
    ALLOWED_ATTR: ['class'],
  });
};

const ThinkingDots: React.FC = () => (
  <div style={{ display: 'inline-flex', gap: 5, alignItems: 'center', padding: '6px 2px' }}>
    {[0, 1, 2].map((i) => (
      <span key={i} style={{
        width: 7, height: 7, borderRadius: '50%', background: 'var(--wx-thinking)', display: 'inline-block',
        animation: `vivian-bounce 1.2s ${i * 0.18}s infinite ease-in-out`,
      }} />
    ))}
    <style>{`@keyframes vivian-bounce { 0%, 60%, 100% { transform: translateY(0); opacity: 0.4; } 30% { transform: translateY(-5px); opacity: 1; } }`}</style>
  </div>
);

/** 用户头像操作（上传 / 清除），供头像右键菜单与三点菜单复用 */
const useUserAvatarActions = () => {
  const { t } = useTranslation();
  const setUserAvatarUrl = useAppStore((s) => s.setUserAvatarUrl);

  const uploadAvatar = useCallback(async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: t('chat.avatar_image_filter'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'] }],
      });
      if (!selected || Array.isArray(selected)) return;
      const dataUrl = await invoke<string | null>('save_user_avatar', { sourcePath: selected });
      setUserAvatarUrl(dataUrl ?? null);
    } catch (e) {
      console.warn('[UserAvatar] 上传头像失败:', e);
    }
  }, [t, setUserAvatarUrl]);

  const clearAvatar = useCallback(async () => {
    try {
      await invoke('clear_user_avatar');
      setUserAvatarUrl(null);
    } catch (e) {
      console.warn('[UserAvatar] 清除头像失败:', e);
    }
  }, [setUserAvatarUrl]);

  return { uploadAvatar, clearAvatar };
};

/** 用户头像（支持自定义：右键单击可上传图片） */
const UserAvatar: React.FC<{ size: number }> = ({ size }) => {
  const { t } = useTranslation();
  const userAvatarUrl = useAppStore((s) => s.userAvatarUrl);
  const { uploadAvatar, clearAvatar } = useUserAvatarActions();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY });
  }, []);

  const closeMenu = useCallback(() => setMenu(null), []);

  const handleUpload = useCallback(async () => {
    closeMenu();
    await uploadAvatar();
  }, [closeMenu, uploadAvatar]);

  const handleClear = useCallback(async () => {
    closeMenu();
    await clearAvatar();
  }, [closeMenu, clearAvatar]);

  // 右键菜单关闭：点击外部 / Escape / 滚动
  useEffect(() => {
    if (!menu) return;
    const onDown = (ev: MouseEvent) => {
      if (ev.button !== 2) setMenu(null);
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') setMenu(null);
    };
    const onScroll = () => setMenu(null);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [menu]);

  const imgStyle: React.CSSProperties = {
    width: size, height: size, borderRadius: 8, flexShrink: 0,
    objectFit: 'cover', cursor: 'context-menu',
  };

  return (
    <>
      {userAvatarUrl ? (
        <img
          src={userAvatarUrl}
          alt={t('chat.user_avatar')}
          style={imgStyle}
          onContextMenu={handleContextMenu}
          draggable={false}
        />
      ) : (
        <div
          style={{
            width: size, height: size, borderRadius: 8, flexShrink: 0,
            background: 'linear-gradient(135deg, #5ac8fa, #007aff)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            color: '#fff', fontSize: size * 0.45, fontWeight: 600,
            cursor: 'context-menu',
          }}
          onContextMenu={handleContextMenu}
        >{t('chat.user_avatar')}</div>
      )}
      {menu && (
        <div
          style={{
            position: 'fixed',
            left: Math.min(menu.x, window.innerWidth - 160),
            top: Math.min(menu.y, window.innerHeight - 90),
            zIndex: 9999,
            minWidth: 140,
            padding: '4px',
            borderRadius: 10,
            background: 'var(--wx-menu-bg)',
            backdropFilter: 'blur(12px)',
            WebkitBackdropFilter: 'blur(12px)',
            border: '1px solid var(--wx-border)',
            boxShadow: '0 8px 24px var(--wx-menu-shadow)',
          }}
          onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            onClick={handleUpload}
            style={menuItemStyle}
            onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--wx-bg-active)'; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
          >{t('chat.avatar_upload')}</button>
          {userAvatarUrl && (
            <button
              onClick={handleClear}
              style={{ ...menuItemStyle, color: '#FF6B6B' }}
              onMouseEnter={(e) => { e.currentTarget.style.background = 'rgba(255,107,107,0.15)'; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
            >{t('chat.avatar_clear')}</button>
          )}
        </div>
      )}
    </>
  );
};

const menuItemStyle: React.CSSProperties = {
  display: 'block',
  width: '100%',
  padding: '8px 12px',
  border: 'none',
  background: 'transparent',
  color: 'var(--wx-text)',
  fontSize: 13,
  textAlign: 'left',
  cursor: 'pointer',
  borderRadius: 6,
  transition: 'background 0.15s ease',
};

/**
 * AI 头像（按角色加载对应 icon.png，URL 需按 dev/release 解析，
 * release 下 dist 已移除明文目录，只能走 model 协议；解析失败回退 favicon）
 */
const AiAvatar: React.FC<{ size: number; characterId?: string }> = ({ size, characterId }) => {
  const [src, setSrc] = useState<string>('/favicon.ico');
  useEffect(() => {
    let cancelled = false;
    if (!characterId) {
      setSrc('/favicon.ico');
      return;
    }
    void resolveAvatarUrl(characterId).then((url) => {
      if (!cancelled) setSrc(url);
    });
    return () => {
      cancelled = true;
    };
  }, [characterId]);
  return (
    <img
      src={src}
      alt={characterId ?? 'Vivian'}
      draggable={false}
      style={{ width: size, height: size, borderRadius: 8, flexShrink: 0, objectFit: 'cover' }}
    />
  );
};

/** 联系人列表头像（同 AiAvatar，单独组件以便列表项复用解析结果） */
const ContactAvatar: React.FC<{ characterId: string; name: string; size?: number }> = ({
  characterId,
  name,
  size = 44,
}) => {
  const [src, setSrc] = useState<string>('/favicon.ico');
  useEffect(() => {
    let cancelled = false;
    void resolveAvatarUrl(characterId).then((url) => {
      if (!cancelled) setSrc(url);
    });
    return () => {
      cancelled = true;
    };
  }, [characterId]);
  return (
    <img
      src={src}
      alt={name}
      draggable={false}
      style={{ width: size, height: size, borderRadius: 8, objectFit: 'cover' }}
    />
  );
};

/** 未读消息角标（微信风格：头像右上角红色圆点内白色数字，双位数变药丸，超 99 显示 99+） */
const UnreadBadge: React.FC<{ count: number }> = ({ count }) => {
  if (count <= 0) return null;
  return (
    <span style={{
      position: 'absolute', top: -5, right: -5, zIndex: 1,
      minWidth: 18, height: 18, padding: count > 9 ? '0 5px' : '0',
      borderRadius: 9, background: '#FA5151',
      color: '#fff', fontSize: 11, fontWeight: 600, lineHeight: '18px',
      textAlign: 'center', fontVariantNumeric: 'tabular-nums',
      boxShadow: '0 1px 3px rgba(0,0,0,0.25)',
      pointerEvents: 'none',
    }}>{count > 99 ? '99+' : count}</span>
  );
};

interface BubbleProps {
  message: ChatMessage;
  onOpenImage?: (src: string) => void;
}

/** 图片缩略图气泡（微信风格）：懒加载历史图片 data URL，点击查看大图 */
const ImageThumb = React.memo(function ImageThumb({ message, onOpen }: { message: ChatMessage; onOpen?: (src: string) => void }) {
  // 缓存读取放到 useEffect 中，避免在渲染阶段修改模块级 Map（LRU 会 delete+re-insert）
  const [src, setSrc] = useState<string | null>(message.imageDataUrl ?? null);
  useEffect(() => {
    if (src) return;
    if (!message.imagePath) return;
    // 优先读缓存（命中则直接 setSrc，不发 IPC）
    const cached = imageCacheGet(message.imagePath);
    if (cached) {
      setSrc(cached);
      return;
    }
    let cancelled = false;
    void invoke<string | null>('get_image_data_url', { imagePath: message.imagePath }).then((url) => {
      if (!cancelled && url) {
        imageCacheSet(message.imagePath!, url);
        setSrc(url);
      }
    }).catch(() => { /* ignore */ });
    return () => { cancelled = true; };
  }, [src, message.imagePath]);

  if (!src) {
    return (
      <div style={{
        width: 180, height: 140, borderRadius: 8, background: 'var(--wx-bg-active)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
      }}>
        <LoadingSpinner size={18} color="var(--wx-icon)" thickness={1.5} />
      </div>
    );
  }
  return (
    <img
      src={src}
      alt={message.content || '图片'}
      onClick={() => onOpen?.(src)}
      style={{
        maxWidth: 220, maxHeight: 280, borderRadius: 8, cursor: 'zoom-in',
        display: 'block', objectFit: 'cover',
        boxShadow: '0 1px 2px var(--wx-shadow)',
        userSelect: 'none', WebkitUserDrag: 'none',
      } as React.CSSProperties}
    />
  );
});

/** 语音消息气泡（微信风格）：懒加载历史音频 data URL，点击播放/暂停 */
const VoiceBubble = React.memo(function VoiceBubble({ message, isUser }: { message: ChatMessage; isUser: boolean }) {
  const voice = message.voice;
  const [src, setSrc] = useState<string | null>(voice?.audioDataUrl ?? null);
  const [playing, setPlaying] = useState(false);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const { t } = useTranslation();

  // 懒加载历史音频 data URL
  useEffect(() => {
    if (src) return;
    if (!voice?.audioPath) return;
    const cached = audioCacheGet(voice.audioPath);
    if (cached) {
      setSrc(cached);
      return;
    }
    let cancelled = false;
    void invoke<string | null>('get_audio_data_url', { audioPath: voice.audioPath }).then((url) => {
      if (!cancelled && url) {
        audioCacheSet(voice.audioPath!, url);
        setSrc(url);
      }
    }).catch(() => { /* ignore */ });
    return () => { cancelled = true; };
  }, [src, voice?.audioPath]);

  const duration = voice?.duration ?? 0;
  // 气泡宽度：基础 70px + 每秒 6px，限制 70~220px
  const bubbleWidth = Math.max(70, Math.min(220, 70 + duration * 6));

  const togglePlay = useCallback(() => {
    if (!src) return;
    if (!audioRef.current) {
      audioRef.current = new Audio(src);
      audioRef.current.addEventListener('ended', () => setPlaying(false));
      audioRef.current.addEventListener('error', () => setPlaying(false));
    }
    if (playing) {
      audioRef.current.pause();
      setPlaying(false);
    } else {
      void audioRef.current.play().then(() => setPlaying(true)).catch(() => setPlaying(false));
    }
  }, [src, playing]);

  // 卸载时停止播放
  useEffect(() => {
    return () => {
      if (audioRef.current) {
        audioRef.current.pause();
        audioRef.current = null;
      }
    };
  }, []);

  return (
    <div
      onClick={togglePlay}
      title={src ? t('chat.voice_play_hint', { defaultValue: '点击播放' }) : t('chat.voice_loading', { defaultValue: '加载中…' })}
      style={{
        width: bubbleWidth,
        padding: '10px 14px',
        borderRadius: isUser ? '18px 4px 18px 18px' : '4px 18px 18px 18px',
        background: isUser ? 'var(--wx-bubble-user)' : 'var(--wx-bubble-ai)',
        color: isUser ? 'var(--wx-bubble-user-text)' : 'var(--wx-bubble-ai-text)',
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        cursor: src ? 'pointer' : 'default',
        boxShadow: '0 1px 1px var(--wx-shadow)',
        position: 'relative',
        userSelect: 'none',
      }}
    >
      {/* 气泡尖角 */}
      <span style={{
        position: 'absolute', top: 8,
        ...(isUser ? { right: -5 } : { left: -5 }),
        width: 0, height: 0,
        borderTop: '5px solid transparent',
        borderBottom: '5px solid transparent',
        ...(isUser
          ? { borderLeft: `6px solid ${isUser ? 'var(--wx-bubble-user)' : 'var(--wx-bubble-ai)'}` }
          : { borderRight: `6px solid ${isUser ? 'var(--wx-bubble-user)' : 'var(--wx-bubble-ai)'}` }),
      }} />
      {/* 用户消息：时长在左，图标在右；AI 消息：图标在左，时长在右 */}
      {isUser && (
        <span style={{ fontSize: 13, fontVariantNumeric: 'tabular-nums', flex: 1, textAlign: 'right' }}>
          {Math.round(duration)}″
        </span>
      )}
      {/* 声波/播放图标 */}
      {!src ? (
        <LoadingSpinner size={16} color="currentColor" thickness={1.5} />
      ) : playing ? (
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" style={{ flexShrink: 0 }}>
          <rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor" />
          <rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor" />
        </svg>
      ) : (
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" style={{ flexShrink: 0 }}>
          <path d="M8 5v14l11-7z" fill="currentColor" />
        </svg>
      )}
      {/* 用户消息：右侧多一条声波条装饰；AI 消息：左侧 */}
      <span style={{
        width: 3, height: 14, borderRadius: 2,
        background: 'currentColor', opacity: 0.35, flexShrink: 0,
      }} />
      {!isUser && (
        <span style={{ fontSize: 13, fontVariantNumeric: 'tabular-nums', flex: 1 }}>
          {Math.round(duration)}″
        </span>
      )}
    </div>
  );
});

const Bubble = React.memo(function Bubble({ message, onOpenImage, senderName, characterId }: BubbleProps & { senderName?: string; characterId?: string }) {
  const isUser = message.role === 'user';
  const avatarSize = 38;
  const isStreaming = !!message.streaming;
  const isEmpty = isStreaming && !message.content;
  const hasImage = !!message.imageDataUrl || !!message.imagePath;
  const hasSticker = !!message.sticker;
  const hasLinkCard = !!message.linkCard;
  const hasFile = !!message.fileMeta;
  const hasVoice = !!message.voice;
  const [fileExpanded, setFileExpanded] = useState(false);
  const bubbleBg = message.error ? 'var(--wx-bubble-error)' : isUser ? 'var(--wx-bubble-user)' : 'var(--wx-bubble-ai)';

  const handleOpenLink = useCallback(async (url: string) => {
    if (url.startsWith('vivian://notebook/')) {
      const parts = url.split('/');
      const charId = parts[3];
      const noteId = parts[4];
      if (charId && noteId) {
        try {
          const existing = await WebviewWindow.getByLabel('memory');
          if (existing) {
            await emit('memory:navigate', { page: 'notebook', notebookId: noteId, notebookCharacter: charId });
            await existing.setFocus();
            return;
          }
          new WebviewWindow('memory', {
            url: `/?view=memory&nb_id=${encodeURIComponent(noteId)}&nb_char=${encodeURIComponent(charId)}`,
            title: i18n.t('memory.title'),
            width: window.screen.width,
            height: window.screen.height,
            resizable: true,
            decorations: false,
            transparent: false,
            shadow: true,
            minWidth: 1260,
            minHeight: 896,
            visible: false,
            dragDropEnabled: true,
          });
        } catch (e) {
          console.error('打开笔记本失败:', e);
        }
      }
      return;
    }
    try {
      await openShell(url);
    } catch {
      window.open(url, '_blank', 'noopener,noreferrer');
    }
  }, []);

  return (
    <div style={{
      display: 'flex', flexDirection: isUser ? 'row-reverse' : 'row',
      alignItems: 'flex-start', gap: 8, marginBottom: 16,
    }}>
      {isUser ? <UserAvatar size={avatarSize} /> : <AiAvatar size={avatarSize} characterId={characterId ?? message.character_id} />}
      <div style={{
        display: 'flex', flexDirection: 'column',
        alignItems: isUser ? 'flex-end' : 'flex-start', maxWidth: '68%',
      }}>
        {!isUser && senderName && (
          <span style={{ fontSize: 11, color: 'var(--wx-icon)', marginBottom: 2, marginLeft: 4 }}>{senderName}</span>
        )}
        {hasSticker ? (
          <img
            src={`/expression/${message.sticker}.webp`}
            alt={message.sticker}
            style={{ width: 120, height: 'auto', borderRadius: 8, objectFit: 'contain' }}
          />
        ) : hasImage ? (
          <ImageThumb message={message} onOpen={onOpenImage} />
        ) : hasVoice ? (
          <VoiceBubble message={message} isUser={isUser} />
        ) : hasLinkCard ? (
          <div
            onClick={() => handleOpenLink(message.linkCard!.url)}
            onMouseEnter={(e) => { e.currentTarget.style.boxShadow = '0 2px 12px var(--wx-shadow)'; }}
            onMouseLeave={(e) => { e.currentTarget.style.boxShadow = '0 1px 3px var(--wx-shadow)'; }}
            style={{
              width: 280,
              borderRadius: 8,
              background: '#fff',
              border: '0.5px solid var(--wx-border)',
              boxShadow: '0 1px 3px var(--wx-shadow)',
              cursor: 'pointer',
              overflow: 'hidden',
              position: 'relative',
            }}
          >
            <div style={{
              padding: '12px 14px 10px',
              borderBottom: '0.5px solid var(--wx-border)',
              display: 'flex',
              alignItems: 'flex-start',
              gap: 10,
            }}>
              <div style={{
                width: 52,
                height: 52,
                borderRadius: 6,
                background: 'linear-gradient(135deg, #f0f7ff 0%, #e6f0fa 100%)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                flexShrink: 0,
              }}>
                <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#5b8def" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                  <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                </svg>
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{
                  fontSize: 14,
                  fontWeight: 500,
                  color: 'var(--wx-text)',
                  lineHeight: 1.4,
                  display: '-webkit-box',
                  WebkitLineClamp: 2,
                  WebkitBoxOrient: 'vertical',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  wordBreak: 'break-all',
                  marginBottom: message.linkCard!.description ? 4 : 0,
                }}>
                  {message.linkCard!.title}
                </div>
                {message.linkCard!.description && (
                  <div style={{
                    fontSize: 12,
                    color: 'var(--wx-icon)',
                    lineHeight: 1.4,
                    display: '-webkit-box',
                    WebkitLineClamp: 2,
                    WebkitBoxOrient: 'vertical',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    wordBreak: 'break-all',
                  }}>
                    {message.linkCard!.description}
                  </div>
                )}
              </div>
            </div>
            <div style={{
              padding: '6px 14px',
              display: 'flex',
              alignItems: 'center',
              gap: 4,
              fontSize: 11,
              color: 'var(--wx-icon)',
            }}>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="10" />
                <line x1="2" y1="12" x2="22" y2="12" />
                <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
              </svg>
              <span style={{
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
                flex: 1,
              }}>
                {message.linkCard!.source || (() => {
                  try { return new URL(message.linkCard!.url).hostname; } catch { return message.linkCard!.url; }
                })()}
              </span>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="9 18 15 12 9 6" />
              </svg>
            </div>
          </div>
        ) : hasFile ? (
          <div style={{
            borderRadius: isUser ? '18px 4px 18px 18px' : '4px 18px 18px 18px',
            background: bubbleBg,
            color: isUser ? 'var(--wx-bubble-user-text)' : 'var(--wx-bubble-ai-text)',
            boxShadow: '0 1px 1px var(--wx-shadow)',
            overflow: 'hidden',
            minWidth: 240, maxWidth: 420,
          }}>
            {/* 文件头部：图标 + 文件名 + 字符数 */}
            <div
              onClick={() => setFileExpanded((v) => !v)}
              style={{
                display: 'flex', alignItems: 'center', gap: 10,
                padding: '10px 14px', cursor: 'pointer',
                userSelect: 'none',
              }}
            >
              <div style={{
                width: 36, height: 36, borderRadius: 6,
                background: 'rgba(91, 141, 239, 0.12)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                flexShrink: 0,
              }}>
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#5b8def" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                  <polyline points="14 2 14 8 20 8" />
                </svg>
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{
                  fontSize: 14, fontWeight: 500,
                  overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                }}>
                  {message.fileMeta!.fileName || '未命名文件'}
                </div>
                <div style={{ fontSize: 11, color: 'var(--wx-icon)', marginTop: 2 }}>
                  {message.fileMeta!.fileType.toUpperCase() || 'FILE'}
                  {message.fileMeta!.originalCharCount > 0 && (
                    <span> · {message.fileMeta!.originalCharCount} 字符</span>
                  )}
                  {message.fileMeta!.truncated && <span> · 已截断</span>}
                </div>
              </div>
              <svg
                width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
                style={{
                  transform: fileExpanded ? 'rotate(180deg)' : 'none',
                  transition: 'transform 0.2s', flexShrink: 0, opacity: 0.6,
                }}
              >
                <polyline points="6 9 12 15 18 9" />
              </svg>
            </div>
            {/* 文件内容：默认折叠，避免长文本一次性渲染导致卡顿 */}
            {fileExpanded && (
              <div style={{
                borderTop: '0.5px solid var(--wx-border)',
                padding: '10px 14px',
                maxHeight: 320, overflowY: 'auto',
                fontSize: 13, lineHeight: 1.5, wordBreak: 'break-word',
                whiteSpace: 'pre-wrap',
                fontFamily: 'monospace',
              }}>
                {message.content}
              </div>
            )}
          </div>
        ) : (
          <div style={{
            padding: '10px 14px',
            borderRadius: isUser ? '18px 4px 18px 18px' : '4px 18px 18px 18px',
            background: bubbleBg,
            color: isUser ? 'var(--wx-bubble-user-text)' : 'var(--wx-bubble-ai-text)',
            fontSize: 15, lineHeight: 1.5, wordBreak: 'break-word',
            fontFamily: 'inherit', position: 'relative',
            boxShadow: '0 1px 1px var(--wx-shadow)',
          }}>
            {/* 气泡尖角（微信风格：顶部指向头像的小三角） */}
            <span style={{
              position: 'absolute', top: 8,
              ...(isUser ? { right: -5 } : { left: -5 }),
              width: 0, height: 0,
              borderTop: '5px solid transparent',
              borderBottom: '5px solid transparent',
              ...(isUser
                ? { borderLeft: `6px solid ${bubbleBg}` }
                : { borderRight: `6px solid ${bubbleBg}` }),
            }} />
            {isUser ? (
              <span style={{ whiteSpace: 'pre-wrap' }}>{message.content}</span>
            ) : isEmpty ? (
              <ThinkingDots />
            ) : (
              <div className="vivian-md" dangerouslySetInnerHTML={{ __html: renderMarkdown(message.content) }} />
            )}
            {isStreaming && !isEmpty && (
              <span className="vivian-cursor" style={{
                display: 'inline-block', width: 2, height: 16, background: 'var(--wx-cursor)',
                marginLeft: 2, verticalAlign: 'text-bottom',
                animation: 'vivian-blink 1s steps(2) infinite',
              }} />
            )}
          </div>
        )}
      </div>
    </div>
  );
});

/** 联动卡片 */
const LinkageCard: React.FC<{ card: CardMessage; t: (k: string) => string; characterId?: string }> = ({ card, t, characterId }) => {
  const [hover, setHover] = useState(false);
  const onClick = useCallback(async () => {
    const page = card.cardType; // 'todo' | 'scheduler'
    try {
      const existing = await WebviewWindow.getByLabel('memory');
      if (existing) {
        try {
          if (await existing.isVisible()) {
            await emit('memory:navigate', { page });
            await existing.setFocus();
            return;
          }
        } catch { /* 陈旧引用，继续创建新窗口 */ }
      }
      new WebviewWindow('memory', {
        url: `/?view=memory&nav=${page}`,
        title: i18n.t('memory.title'),
        width: window.screen.width, height: window.screen.height, resizable: true, decorations: false, transparent: false, shadow: true,
        minWidth: 1260, minHeight: 896,
        visible: false,
        dragDropEnabled: true,
      });
    } catch (e) { console.error('打开管理窗口失败:', e); }
  }, [card]);

  const accentColor = card.cardType === 'todo' ? '#07C160' : '#FF9500';
  const icon = card.cardType === 'todo' ? '📝' : '⏰';
  return (
    <div style={{
      display: 'flex', flexDirection: 'row', alignItems: 'flex-start',
      gap: 8, marginBottom: 16,
    }}>
      <AiAvatar size={38} characterId={characterId} />
      <div style={{ maxWidth: '78%' }}>
        <div
          onClick={onClick}
          onMouseEnter={() => setHover(true)}
          onMouseLeave={() => setHover(false)}
          style={{
            cursor: 'pointer', padding: '12px 14px',
            borderRadius: '18px 18px 18px 4px',
            background: hover ? 'var(--wx-bubble-ai-hover)' : 'var(--wx-bubble-ai)',
            border: `0.5px solid ${accentColor}55`,
            boxShadow: '0 1px 2px var(--wx-shadow)',
            transition: 'background 0.15s ease, border 0.15s ease',
            display: 'flex', flexDirection: 'column', gap: 6,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 18 }}>{icon}</span>
            <span style={{ fontSize: 14, fontWeight: 600, color: accentColor }}>{card.title}</span>
          </div>
          {card.subtitle && (
            <div style={{
              fontSize: 13, color: 'var(--wx-bubble-ai-text)', lineHeight: 1.4,
              whiteSpace: 'pre-wrap', wordBreak: 'break-word',
            }}>{card.subtitle}</div>
          )}
          <div style={{
            fontSize: 11, color: 'var(--wx-text-secondary)', marginTop: 2,
            display: 'flex', alignItems: 'center', gap: 4,
          }}>
            <span style={{ color: accentColor }}>›</span>
            {card.cardType === 'todo' ? t('chat.card_todo_view') : t('chat.card_scheduler_view')}
          </div>
        </div>
      </div>
    </div>
  );
};

/* ===== iOS 状态栏图标 ===== */
const SignalIcon: React.FC = () => (
  <svg width="17" height="12" viewBox="0 0 17 12" fill="none">
    <rect x="0" y="9" width="3" height="3" rx="0.5" fill="currentColor" />
    <rect x="4.5" y="6" width="3" height="6" rx="0.5" fill="currentColor" />
    <rect x="9" y="3" width="3" height="9" rx="0.5" fill="currentColor" />
    <rect x="13.5" y="0" width="3" height="12" rx="0.5" fill="currentColor" />
  </svg>
);

const WifiIcon: React.FC = () => (
  <svg width="16" height="12" viewBox="0 0 16 12" fill="none">
    <path d="M8 10.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3z" fill="currentColor" transform="translate(0,-2)" />
    <path d="M4.7 8.3a4.8 4.8 0 0 1 6.6 0" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" transform="translate(0,-1)" />
    <path d="M2.1 5.7a8 8 0 0 1 11.8 0" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" transform="translate(0,-1)" />
    <path d="M0 3.2a11 11 0 0 1 16 0" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" transform="translate(0,-1)" />
  </svg>
);

const BatteryIcon: React.FC<{ level?: number }> = ({ level = 100 }) => {
  const width = 24;
  const fillWidth = Math.max(1, (width - 4) * (level / 100));
  return (
    <svg width={width + 2} height="12" viewBox="0 0 26 12" fill="none">
      <rect x="0.5" y="1" width="22" height="10" rx="2" stroke="currentColor" strokeWidth="1" opacity="0.5" />
      <rect x="23" y="4" width="2" height="4" rx="1" fill="currentColor" opacity="0.5" />
      <rect x="2" y="3" width={fillWidth} height="6" rx="1" fill="currentColor" />
    </svg>
  );
};

/* ===== 底部工具栏 SVG 图标 ===== */
const MicIcon: React.FC<{ recording: boolean; size?: number }> = ({ recording, size = 24 }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <circle cx="12" cy="12" r="10" stroke={recording ? '#FF453A' : 'currentColor'} strokeWidth="1.5" />
    <g
      transform="translate(12 12) rotate(90) scale(0.58) translate(-12 -12)"
      stroke={recording ? '#FF453A' : 'currentColor'}
      strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"
    >
      <path d="M5 12.55a11 11 0 0 1 14.08 0" />
      <path d="M8.53 16.11a6 6 0 0 1 6.95 0" />
      <line x1="12" y1="20" x2="12.01" y2="20" />
    </g>
    {recording && (
      <circle cx="12" cy="12" r="11" stroke="#FF453A" strokeWidth="1" opacity="0.5">
        <animate attributeName="r" values="11;13;11" dur="1.2s" repeatCount="indefinite" />
        <animate attributeName="opacity" values="0.5;0;0.5" dur="1.2s" repeatCount="indefinite" />
      </circle>
    )}
  </svg>
);

const SmileIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <circle cx="12" cy="12" r="9.5" stroke="currentColor" strokeWidth="1.5" />
    <circle cx="9" cy="10.5" r="1.2" fill="currentColor" />
    <circle cx="15" cy="10.5" r="1.2" fill="currentColor" />
    <path d="M8.5 14.5c.8 1.5 2.2 2.5 3.5 2.5s2.7-1 3.5-2.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
  </svg>
);

const PlusIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <circle cx="12" cy="12" r="9.5" stroke="currentColor" strokeWidth="1.5" />
    <path d="M12 7.5v9M7.5 12h9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

const PhoneIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <path
      d="M6.6 4h2.2l1.2 3-1.4 1.4a12 12 0 0 0 5 5l1.4-1.4 3 1.2v2.2c0 .9-.7 1.6-1.6 1.6A13.4 13.4 0 0 1 5 6.6C5 5.7 5.7 5 6.6 5z"
      stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round"
    />
  </svg>
);

const ImageIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" stroke="currentColor" strokeWidth="1.5" />
    <circle cx="9" cy="10" r="1.6" fill="currentColor" />
    <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

const FileIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <path d="M6 3h7l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    <path d="M13 3v5h5" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    <path d="M8.5 14h7M8.5 17h5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

const CameraIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    <path d="M4 8.5A1.5 1.5 0 0 1 5.5 7h2L9 5h6l1.5 2h2A1.5 1.5 0 0 1 20 8.5v9A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5v-9z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    <circle cx="12" cy="13" r="3.5" stroke="currentColor" strokeWidth="1.5" />
  </svg>
);

const AudioMessageIcon: React.FC = () => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
    {/* 扬声器 */}
    <path d="M3 10v4h3l4 4V6L6 10H3z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    {/* 声波弧线 */}
    <path d="M14 9.5a3 3 0 0 1 0 5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    <path d="M17 7.5a6 6 0 0 1 0 9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

/* ===== 摄像头拍摄模态组件 ===== */
interface CameraCaptureProps {
  open: boolean;
  onClose: () => void;
  onCaptured: (base64Data: string, mime: string) => void;
}

/**
 * 摄像头拍摄模态：
 * - open 时通过 getUserMedia 启动摄像头视频流
 * - 支持拍照（canvas 抓帧）、预览、重拍、使用照片
 * - 支持前后摄像头切换（移动端/多摄像头设备）
 * - 关闭/卸载时自动停止所有 track，释放摄像头
 */
const CameraCapture: React.FC<CameraCaptureProps> = ({ open, onClose, onCaptured }) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [capturedDataUrl, setCapturedDataUrl] = useState<string | null>(null);
  const [facingMode, setFacingMode] = useState<'user' | 'environment'>('environment');
  const { t } = useTranslation();

  const startCamera = useCallback(async (mode: 'user' | 'environment') => {
    setError(null);
    setCapturedDataUrl(null);
    if (streamRef.current) {
      streamRef.current.getTracks().forEach((tr) => tr.stop());
      streamRef.current = null;
    }
    if (!navigator.mediaDevices?.getUserMedia) {
      setError(t('chat.camera_no_device', { defaultValue: '未检测到摄像头设备' }));
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: mode, width: { ideal: 1920 }, height: { ideal: 1080 } },
        audio: false,
      });
      streamRef.current = stream;
      if (videoRef.current) {
        videoRef.current.srcObject = stream;
        await videoRef.current.play().catch(() => {});
      }
    } catch (e) {
      const errName = (e as DOMException)?.name ?? '';
      if (errName === 'NotAllowedError' || errName === 'SecurityError') {
        setError(t('chat.camera_denied', { defaultValue: '无法访问摄像头，请检查系统权限设置' }));
      } else if (errName === 'NotFoundError' || errName === 'OverconstrainedError') {
        setError(t('chat.camera_no_device', { defaultValue: '未检测到摄像头设备' }));
      } else {
        setError(t('chat.camera_failed', { error: String(e), defaultValue: '摄像头错误：{{error}}' }));
      }
    }
  }, [t]);

  useEffect(() => {
    if (open) {
      void startCamera(facingMode);
    }
    return () => {
      if (streamRef.current) {
        streamRef.current.getTracks().forEach((tr) => tr.stop());
        streamRef.current = null;
      }
    };
  }, [open, facingMode, startCamera]);

  const handleCapture = useCallback(() => {
    const video = videoRef.current;
    if (!video || !video.videoWidth) return;
    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
    setCapturedDataUrl(canvas.toDataURL('image/png'));
  }, []);

  const handleUse = useCallback(() => {
    if (!capturedDataUrl) return;
    const match = capturedDataUrl.match(/^data:([^;]+);base64,(.+)$/);
    if (!match) return;
    onCaptured(match[2], match[1]);
    onClose();
  }, [capturedDataUrl, onCaptured, onClose]);

  const handleSwitch = useCallback(() => {
    setFacingMode((m) => (m === 'user' ? 'environment' : 'user'));
  }, []);

  if (!open) return null;

  // 拍照后的预览界面
  if (capturedDataUrl) {
    return (
      <div style={{
        position: 'fixed', inset: 0, zIndex: 10000,
        background: '#000',
        display: 'flex', flexDirection: 'column',
      }}>
        {/* 照片预览区 */}
        <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', overflow: 'hidden' }}>
          <img src={capturedDataUrl} alt="captured" style={{
            maxWidth: '100%', maxHeight: '100%', objectFit: 'contain',
          }} />
        </div>
        {/* 底部操作栏 */}
        <div style={{
          display: 'flex', alignItems: 'center', justifyContent: 'space-around',
          padding: '20px 40px 40px',
          background: 'linear-gradient(transparent, rgba(0,0,0,0.8))',
        }}>
          <button onClick={() => setCapturedDataUrl(null)} style={{
            padding: '12px 28px', borderRadius: 24,
            border: '1.5px solid rgba(255,255,255,0.4)',
            background: 'rgba(255,255,255,0.1)', color: '#fff', fontSize: 15, cursor: 'pointer',
            backdropFilter: 'blur(8px)',
          }}>{t('chat.capture_retake', { defaultValue: '重拍' })}</button>
          <button onClick={handleUse} style={{
            padding: '12px 36px', borderRadius: 24, border: 'none',
            background: '#07c160', color: '#fff', fontSize: 15, cursor: 'pointer', fontWeight: 600,
          }}>{t('chat.capture_use', { defaultValue: '使用' })}</button>
        </div>
      </div>
    );
  }

  // 相机取景界面
  return (
    <div style={{
      position: 'fixed', inset: 0, zIndex: 10000,
      background: '#000',
      display: 'flex', flexDirection: 'column',
    }}>
      {/* 视频预览区 — 全屏铺满 */}
      <div style={{ flex: 1, position: 'relative', overflow: 'hidden' }}>
        {error ? (
          <div style={{
            position: 'absolute', inset: 0,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            color: '#ff6b6b', fontSize: 14, padding: '0 32px', textAlign: 'center',
          }}>{error}</div>
        ) : (
          <video
            ref={videoRef}
            autoPlay
            playsInline
            muted
            style={{
              width: '100%', height: '100%',
              objectFit: 'cover',
              transform: facingMode === 'user' ? 'scaleX(-1)' : 'none',
            }}
          />
        )}
        {/* 顶部工具栏 */}
        <div style={{
          position: 'absolute', top: 0, left: 0, right: 0,
          display: 'flex', alignItems: 'center', justifyContent: 'space-between',
          padding: '12px 16px',
          background: 'linear-gradient(rgba(0,0,0,0.5), transparent)',
        }}>
          <button onClick={onClose} style={{
            width: 36, height: 36, borderRadius: '50%',
            background: 'rgba(0,0,0,0.4)', border: 'none',
            color: '#fff', fontSize: 18, cursor: 'pointer',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            backdropFilter: 'blur(8px)',
          }} aria-label={t('chat.capture_close', { defaultValue: '关闭' })}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round"><path d="M18 6L6 18M6 6l12 12"/></svg>
          </button>
          <button onClick={handleSwitch} style={{
            width: 36, height: 36, borderRadius: '50%',
            background: 'rgba(0,0,0,0.4)', border: 'none',
            color: '#fff', cursor: 'pointer',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            backdropFilter: 'blur(8px)',
          }} aria-label={t('chat.capture_switch', { defaultValue: '切换摄像头' })}>
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M11 19H4a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h5"/><path d="M13 5h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-5"/>
              <circle cx="12" cy="12" r="3"/><path d="m18 22-3-3 3-3"/><path d="m6 2 3 3-3 3"/>
            </svg>
          </button>
        </div>
      </div>

      {/* 底部拍照控制栏 */}
      <div style={{
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        padding: '24px 0 48px',
        background: 'linear-gradient(transparent, rgba(0,0,0,0.6))',
      }}>
        <button onClick={handleCapture} style={{
          width: 72, height: 72, borderRadius: '50%',
          border: '4px solid #fff',
          background: 'rgba(255,255,255,0.15)',
          cursor: 'pointer',
          transition: 'transform 0.1s ease, background 0.15s ease',
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }}
        onMouseDown={(e) => { e.currentTarget.style.transform = 'scale(0.9)'; e.currentTarget.style.background = 'rgba(255,255,255,0.3)'; }}
        onMouseUp={(e) => { e.currentTarget.style.transform = 'scale(1)'; e.currentTarget.style.background = 'rgba(255,255,255,0.15)'; }}
        onMouseLeave={(e) => { e.currentTarget.style.transform = 'scale(1)'; e.currentTarget.style.background = 'rgba(255,255,255,0.15)'; }}
        aria-label={t('chat.capture_capture', { defaultValue: '拍照' })}>
          <span style={{
            width: 56, height: 56, borderRadius: '50%',
            background: '#fff',
          }} />
        </button>
      </div>
    </div>
  );
};

/* ===== Emoji 数据 ===== */
// 积极/消极/中性情绪交替排布，避免前面全是笑脸
const EMOJI_LIST = [
  // 笑脸与情绪（积极/消极/中性交替）
  '😀', '😢', '😂', '😠', '😍', '😨', '🤣', '😩', '😊', '😭',
  '😎', '😰', '🥰', '😖', '😅', '😱', '😘', '😤', '🤔', '🤯',
  '😏', '🥺', '😐', '😵', '🙃', '😟', '🤩', '😬', '😌', '🤐',
  '🥳', '😶', '😋', '🙁', '🤪', '😮', '😛', '😯', '🤨', '🥱',
  '🤗', '🥶', '🤑', '🤒', '🤠', '🤕', '🤭', '🤢', '🤫', '🤮',
  '🥵', '😪', '🥴', '🤤', '😇', '😔', '😥', '😴', '😷', '🙄',
  '😒', '🤥', '😡', '🤬', '😑', '😝', '😁', '😆', '😃', '🙂',
  // 手势与身体
  '👍', '👎', '👏', '🙏', '👋', '🤝', '🙌', '🤞', '💪', '👊',
  '✌️', '🤟', '👌', '🤙', '👈', '👉', '👆', '👇', '✋', '🤚',
  // 心与情感
  '❤️', '💔', '💕', '🖤', '💖', '💗', '💘', '💝', '💞', '❣️',
  // 自然与天气
  '🌹', '🌸', '🌺', '🌻', '🌷', '🌼', '🌈', '☀️', '⛅', '☁️',
  '🌧️', '⛈️', '❄️', '🔥', '⚡', '💧',
  // 庆祝与物品
  '🎉', '🎊', '✨', '🎈', '🎁', '🎂', '🍰', '🍾',
  // 符号与状态
  '💯', '✅', '❌', '⭐', '🌟', '💫', '💥', '💢', '❓', '❗',
  '💬', '💤', '🚫', '🔍', '💡',
];

type RenderItem =
  | { kind: 'time'; key: string; text: string }
  | { kind: 'msg'; key: string; msg: ChatMessage }
  | { kind: 'card'; key: string; card: CardMessage; timestamp: number };

const ChatWindow: React.FC = () => {
  const { t } = useTranslation();
  // ===== 三视图状态 =====
  /** 当前视图：home（角色选择）/ private（单角色私聊）/ group（群聊）/ details（聊天详情） */
  const [view, setView] = useState<'home' | 'private' | 'group' | 'details'>('home');
  /** 在线角色列表 */
  const [characters, setCharacters] = useState<Array<{ id: string; name: string; online: boolean }>>([]);
  // characters 的 ref 镜像，供 setInterval 等闭包读取最新值，避免在 state 更新器内执行副作用
  const charactersRef = useRef<Array<{ id: string; name: string; online: boolean }>>([]);
  useEffect(() => { charactersRef.current = characters; }, [characters]);
  /** 各角色在场状态（presence）：online / busy / rest / offline，独立于窗口 online 标志 */
  const [presenceStates, setPresenceStates] = useState<Record<string, string>>({});
  /** 会话最新消息预览（key: charId 或 'group'），用于主页列表项展示 */
  const [lastPreviews, setLastPreviews] = useState<Record<string, { content: string; timestamp: number; role: string; sticker?: string; imagePath?: string; characterId?: string }>>({});
  /** 聊天记录搜索：关键词、结果列表、是否在搜索中 */
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Array<{
    id: string;
    content: string;
    role: string;
    timestamp: number;
    character_id: string;
    character_name: string;
    source: 'private' | 'group';
  }>>([]);
  const [searching, setSearching] = useState(false);
  /** 当前私聊的角色 ID（仅 private 视图使用） */
  const [privateCharId, setPrivateCharId] = useState<string | null>(null);
  /** 群聊消息列表（独立于私聊 messages） */
  const [groupMessages, setGroupMessages] = useState<ChatMessage[]>([]);
  /** 群聊流式缓冲：按 `charId:streamId` 索引累积未换行文本 */
  const groupStreamBuffersRef = useRef<Map<string, string>>(new Map());
  /** 群聊 stream_id → charId 映射，用于事件路由 */
  const groupStreamCharMapRef = useRef<Map<string, string>>(new Map());
  /** 群聊视图是否正在流式生成（至少一个角色在生成） */
  const [groupStreaming, setGroupStreaming] = useState(false);
  /** 私聊视图是否正在流式生成（用于标题显示"对方正在输入..."） */
  const [privateTyping, setPrivateTyping] = useState(false);

  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [cards, setCards] = useState<CardMessage[]>([]);
  const [input, setInput] = useState('');
  /** 图片草稿（本窗口选择/粘贴，含 base64，发送前可预览/取消） */
  const [draftImages, setDraftImages] = useState<{ id: string; dataUrl: string; name: string; mime: string }[]>([]);
  const draftSeqRef = useRef(0);
  /** @ 提及菜单状态（仅群聊视图） */
  const [mentionState, setMentionState] = useState<{
    active: boolean;
    query: string;
    startIndex: number;
    selectedIndex: number;
  }>({ active: false, query: '', startIndex: -1, selectedIndex: 0 });
  const mentionStateRef = useRef(mentionState);
  useEffect(() => { mentionStateRef.current = mentionState; }, [mentionState]);
  /** 当前 @ 筛选匹配到的在线角色列表 */
  const mentionList = useMemo(() => {
    if (!mentionState.active) return [];
    const q = mentionState.query.toLowerCase();
    return characters.filter((c) => c.online && c.name.toLowerCase().startsWith(q));
  }, [mentionState.active, mentionState.query, characters]);
  const [recording, setRecording] = useState(false);
  // 用 ref 跟踪 recording 最新值，供卸载清理和 toggleRecording 闭包读取
  const recordingRef = useRef(false);
  useEffect(() => { recordingRef.current = recording; }, [recording]);
  // ===== 语音消息录制（按住说话）=====
  // 与 recording（ASR 转文字写入输入框）不同：voiceRecording 把原始音频存为文件并显示为语音气泡，
  // 同时启动 ASR 获取转写文本发送给 LLM。两者不能同时进行。
  const [voiceRecording, setVoiceRecording] = useState(false);
  const voiceRecordingRef = useRef(false);
  useEffect(() => { voiceRecordingRef.current = voiceRecording; }, [voiceRecording]);
  /** 语音消息录制期间累积的 ASR final 文本 */
  const voiceAsrTextRef = useRef('');
  /** 本次录音是否走实时 ASR（WinRT 共享麦克风）；false=文件转写模式（cpal 类后端独占麦克风） */
  const voiceRealtimeAsrRef = useRef(true);
  /** MediaRecorder 实例（语音消息录制用） */
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  /** 录制启动时刻（计算时长用） */
  const voiceStartTimeRef = useRef(0);
  /** 录制计时器（驱动 UI 显示秒数） */
  const voiceTimerRef = useRef<number | null>(null);
  /** 录制期间累计的音频 chunks */
  const voiceChunksRef = useRef<Blob[]>([]);
  /** 录制选中的 MIME（保存音频时用） */
  const voiceMimeRef = useRef('audio/webm');
  /** 录制开始的 pointerId，用于释放 pointer capture */
  const voicePointerIdRef = useRef<number | null>(null);
  /** 录制时长（秒，UI 实时显示） */
  const [voiceDuration, setVoiceDuration] = useState(0);
  /** 语音消息发送后跳过 chat:user_message 的文本气泡（避免与本地语音气泡重复） */
  const skipNextUserMessageRef = useRef(false);
  const [initialLoading, setInitialLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [historyLoadedCount, setHistoryLoadedCount] = useState(0);
  const historyLoadedCountRef = useRef(0);
  useEffect(() => { historyLoadedCountRef.current = historyLoadedCount; }, [historyLoadedCount]);
  // ===== 聊天详情（details 视图）：备注名 / 聊天背景，localStorage 持久化 =====
  const [charRemarks, setCharRemarks] = useState<Record<string, string>>(() => {
    try { return JSON.parse(localStorage.getItem('vivian_char_remarks') || '{}'); }
    catch { return {}; }
  });
  const [chatBackgrounds, setChatBackgrounds] = useState<Record<string, string>>(() => {
    try { return JSON.parse(localStorage.getItem('vivian_chat_backgrounds') || '{}'); }
    catch { return {}; }
  });
  useEffect(() => {
    try { localStorage.setItem('vivian_char_remarks', JSON.stringify(charRemarks)); } catch { /* ignore */ }
  }, [charRemarks]);
  useEffect(() => {
    try { localStorage.setItem('vivian_chat_backgrounds', JSON.stringify(chatBackgrounds)); } catch { /* ignore */ }
  }, [chatBackgrounds]);
  // details 视图子界面：'main'（详情主页）/ 'search'（查找聊天内容）
  const [detailsSubView, setDetailsSubView] = useState<'main' | 'search'>('main');
  const [detailsSearchQuery, setDetailsSearchQuery] = useState('');
  const [detailsSearchResults, setDetailsSearchResults] = useState<Array<{ id: string; content: string; role: string; timestamp: number; character_id: string }>>([]);
  const [detailsSearching, setDetailsSearching] = useState(false);
  // 备注编辑
  const [editingRemark, setEditingRemark] = useState(false);
  const [remarkInput, setRemarkInput] = useState('');
  const setUserAvatarUrl = useAppStore((s) => s.setUserAvatarUrl);
  /** 底部面板：'none' | 'emoji' | 'media' */
  const [bottomPanel, setBottomPanel] = useState<'none' | 'emoji' | 'media'>('none');
  /** 最近使用的 emoji（持久化到 localStorage，最多保留 16 个） */
  const [recentEmojis, setRecentEmojis] = useState<string[]>(() => {
    try {
      const stored = localStorage.getItem('vivian-recent-emojis');
      return stored ? JSON.parse(stored) : [];
    } catch { return []; }
  });
  useEffect(() => {
    try { localStorage.setItem('vivian-recent-emojis', JSON.stringify(recentEmojis)); } catch { /* ignore */ }
  }, [recentEmojis]);
  /** 通话视图：'none' | 'full' | 'minimized' */
  const [callView, setCallView] = useState<'none' | 'full' | 'minimized'>('none');
  /** 图片大图查看器：src 非 null 时展示 */
  const [imageViewerSrc, setImageViewerSrc] = useState<string | null>(null);
  const bottomPanelTriggersRef = useRef<HTMLDivElement>(null);
  const bottomPanelDrawerRef = useRef<HTMLDivElement>(null);
  const [isClosing, setIsClosing] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  /** 各会话未读消息数（键：角色 ID 或 'group'） */
  const [unreadCounts, setUnreadCounts] = useState<Record<string, number>>({});
  /** 各会话已读水位线（最新消息时间戳），新于它的 assistant 消息计为未读 */
  const lastSeenRef = useRef<Record<string, number>>(
    (() => {
      try {
        const stored = localStorage.getItem('vivian_chat_last_seen');
        return stored ? JSON.parse(stored) : {};
      } catch {
        return {};
      }
    })(),
  );
  /** 首次加载是否已完成"全部已读"快照；有持久化水位线时直接为 true，按真实水位线统计未读 */
  const unreadInitRef = useRef(Object.keys(lastSeenRef.current).length > 0);
  /** 当前查看的会话（角色 ID 或 'group'），home 视图为 null —— 用 ref 避免 refreshLastPreviews 依赖视图状态 */
  const viewingRef = useRef<string | null>(null);

  // ChatWindow 是独立窗口，有独立的 store 实例，需要自行加载头像
  useEffect(() => {
    void (async () => {
      try {
        const dataUrl = await invoke<string | null>('get_user_avatar_data_url');
        setUserAvatarUrl(dataUrl ?? null);
      } catch { /* ignore */ }
    })();
  }, [setUserAvatarUrl]);

  // 主题：读取 base.theme 配置设置根节点 data-theme（system/light/dark），并监听设置窗口实时变更
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
    };
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
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

  /** 将已读水位线持久化到 localStorage，使窗口关闭重开后仍能正确统计未读 */
  const persistLastSeen = useCallback(() => {
    try {
      localStorage.setItem('vivian_chat_last_seen', JSON.stringify(lastSeenRef.current));
    } catch { /* ignore */ }
  }, []);

  /** 标记会话已读：推进已读水位线（防止下次刷新重计）并清零该会话未读角标 */
  const markConversationRead = useCallback((convId: string) => {
    lastSeenRef.current[convId] = Date.now();
    persistLastSeen();
    setUnreadCounts((prev) => (prev[convId] ? { ...prev, [convId]: 0 } : prev));
  }, [persistLastSeen]);

  // 加载会话最新消息预览（主页列表用），并监听 dialogue:changed / chat:history-cleared 实时刷新
  // 使用 get_latest_previews 轻量命令：Rust 端完成聚合过滤，只返回每个会话最新一条 + 未读计数
  const refreshLastPreviews = useCallback(async () => {
    try {
      const viewing = viewingRef.current;
      // 首次加载：不传 last_seen，让后端返回全量未读，前端再以最新时间戳初始化水位线
      const lastSeenParam = unreadInitRef.current ? lastSeenRef.current : undefined;
      const result = await invoke<{
        previews: Array<{
          id: string;
          role: string;
          content: string;
          timestamp: number | string;
          character_id: string;
          character_name: string;
          metadata?: Record<string, unknown>;
        }>;
        unread: Record<string, number>;
      }>('get_latest_previews', { lastSeen: lastSeenParam });

      type Preview = { content: string; timestamp: number; role: string; sticker?: string; imagePath?: string; characterId?: string };
      const map: Record<string, Preview> = {};
      for (const e of result.previews) {
        const ch = typeof e.metadata?.channel === 'string' ? (e.metadata.channel as string) : undefined;
        const ts = typeof e.timestamp === 'number' ? e.timestamp : Number(e.timestamp) || 0;
        const sticker = typeof e.metadata?.sticker === 'string' ? (e.metadata.sticker as string) : undefined;
        const imagePath = typeof e.metadata?.image_path === 'string' ? (e.metadata.image_path as string) : undefined;
        const entry: Preview = { content: e.content, timestamp: ts, role: e.role, sticker, imagePath, characterId: e.character_id };
        if (ch === 'wechat') {
          map[e.character_id] = entry;
        } else if (ch === 'wechat_group') {
          map['group'] = entry;
        }
      }
      setLastPreviews(map);

      // ── 未读计数 ──
      // 首次加载：后端 last_seen=None 时返回空 unread（历史全部视为已读），
      // 此处仅初始化水位线，后续调用据此计算未读
      if (!unreadInitRef.current) {
        unreadInitRef.current = true;
        for (const [conv, p] of Object.entries(map)) lastSeenRef.current[conv] = p.timestamp;
        persistLastSeen();
      }
      // 正在查看的会话：消息实时可见，推进水位线，不计未读
      if (viewing && map[viewing] && map[viewing].timestamp > (lastSeenRef.current[viewing] ?? 0)) {
        lastSeenRef.current[viewing] = map[viewing].timestamp;
        persistLastSeen();
      }
      const counts: Record<string, number> = { ...result.unread };
      // 正在查看的会话不计未读
      if (viewing && counts[viewing]) {
        delete counts[viewing];
      }
      setUnreadCounts((prev) => {
        const next: Record<string, number> = {};
        for (const conv of Object.keys(prev)) next[conv] = 0;
        for (const [conv, n] of Object.entries(counts)) next[conv] = n;
        // 浅比较：若所有值都相同则返回 prev，避免不必要重渲染
        const prevKeys = Object.keys(prev);
        const nextKeys = Object.keys(next);
        if (prevKeys.length === nextKeys.length) {
          let same = true;
          for (const k of nextKeys) {
            if (prev[k] !== next[k]) { same = false; break; }
          }
          if (same) return prev;
        }
        return next;
      });
    } catch { /* ignore */ }
  }, []);

  // 初始化：加载预览 + 读取 URL 参数（横幅点击等外部打开场景会传入 initial_private 跳转指定私聊）
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        await refreshLastPreviews();
      } catch { /* ignore */ }
      if (cancelled) return;
      // dialogue:changed / chat:history-cleared 由下方 refreshHistory 的监听器统一 debounce 处理（同时刷新预览+历史）
    })();
    // 读取 URL 参数，支持外部直接跳转到指定私聊
    try {
      const params = new URLSearchParams(window.location.search);
      const initPrivate = params.get('private_char_id');
      const initView = params.get('view_mode');
      if (initPrivate) {
        privateCharIdRef.current = initPrivate;
        setPrivateCharId(initPrivate);
        setView('private');
        markConversationRead(initPrivate);
      } else if (initView === 'group') {
        setView('group');
      }
    } catch { /* ignore */ }
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听 chatwindow:navigate 事件（横幅点击 / 外部调用等场景），运行时动态跳转指定私聊
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string; view?: 'home' | 'private' | 'group' }>(
          'chatwindow:navigate',
          (e) => {
            if (cancelled || !e.payload) return;
            const p = e.payload;
            if (p.view === 'group') {
              setView('group');
              return;
            }
            if (p.view === 'home' || !p.character_id) {
              setView('home');
              return;
            }
            // 跳转到指定角色私聊
            privateCharIdRef.current = p.character_id;
            setPrivateCharId(p.character_id);
            setView('private');
            // 标记该会话已读
            markConversationRead(p.character_id);
          },
        );
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 同步"当前查看的会话"标记，供 refreshLastPreviews 判断哪个会话的消息实时可见
  useEffect(() => {
    viewingRef.current = view === 'private' ? privateCharId : view === 'group' ? 'group' : null;
  }, [view, privateCharId]);

  // 窗口聚焦/可见时清除当前查看会话的未读并刷新预览
  // 用户切回 ChatWindow 时，正在查看的会话消息已可见，红点应立即清除
  // 同时处理「退出后再次显示」：复位 isClosing 并重播进入动画，
  // 否则退出动画 fill:forwards 残留 opacity:0，重开后窗口不可见（三态异常）
  const isClosingRef = useRef(false);
  useEffect(() => { isClosingRef.current = isClosing; }, [isClosing]);
  useEffect(() => {
    const handleVisible = () => {
      const viewing = viewingRef.current;
      if (viewing) {
        markConversationRead(viewing);
      }
      void refreshLastPreviews();
      // 从隐藏 → 可见（被边缘看护/横幅再次显示）：复位退出态
      if (isClosingRef.current) {
        setIsClosing(false);
        playEnter();
      }
    };
    window.addEventListener('focus', handleVisible);
    document.addEventListener('visibilitychange', handleVisible);
    return () => {
      window.removeEventListener('focus', handleVisible);
      document.removeEventListener('visibilitychange', handleVisible);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [markConversationRead, refreshLastPreviews]);

  // 加载在线角色列表（挂载时 + 角色上下线事件时刷新）
  // 失败时指数退避重试（最多 4 次），避免偶发 IPC 失败导致私聊入口永久缺失
  const refreshCharacters = useCallback(async () => {
    const maxRetries = 4;
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        const result = await invoke<{ active_id: string; characters: Array<{ id: string; name: string; online: boolean }> }>('list_characters');
        setCharacters(result.characters ?? []);
        return;
      } catch (e) {
        if (attempt === maxRetries) {
          console.warn('[ChatWindow] 加载角色列表失败（已重试 4 次）:', e);
          return;
        }
        // 指数退避：200ms / 400ms / 800ms / 1600ms
        await new Promise((r) => setTimeout(r, 200 * Math.pow(2, attempt)));
      }
    }
  }, []);

  useEffect(() => {
    void refreshCharacters();
    let unlisten: UnlistenFn | undefined;
    // 角色列表为空时定期重试（窗口可能在角色初始化完成前打开，IPC 成功但返回空列表）
    // 使用 ref 读取最新 characters，避免在 state 更新器内执行副作用（React 18 StrictMode 会双调用更新器）
    let retryTimer: ReturnType<typeof setInterval> | undefined;
    const startRetryIfEmpty = () => {
      if (retryTimer) return;
      retryTimer = setInterval(() => {
        const hasOnline = charactersRef.current.some((c) => c.online);
        if (hasOnline) {
          // 已有在线角色，停止轮询
          if (retryTimer) { clearInterval(retryTimer); retryTimer = undefined; }
          return;
        }
        void refreshCharacters();
      }, 3000);
    };
    startRetryIfEmpty();
    void (async () => {
      try {
        unlisten = await listen('character:online_changed', () => {
          void refreshCharacters();
          // 角色上下线时刷新预览：在线角色集合变化后 get_chat_history_all 返回内容也会变
          void refreshLastPreviewsRef.current();
        });
      } catch { /* ignore */ }
    })();
    return () => { unlisten?.(); if (retryTimer) clearInterval(retryTimer); };
  }, [refreshCharacters]);

  // 加载各角色在场状态（presence），并监听 presence:changed 实时更新
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        const states = await invoke<Array<{ character_id: string; state: string }>>('get_all_presence_states');
        if (cancelled) return;
        const map: Record<string, string> = {};
        for (const s of states) map[s.character_id] = s.state;
        setPresenceStates(map);
      } catch { /* ignore */ }
      try {
        unlisten = await listen<{ character_id: string; to: string }>('presence:changed', (e) => {
          if (!e.payload?.character_id) return;
          const newState = e.payload.to;
          setPresenceStates((prev) => ({ ...prev, [e.payload.character_id]: newState }));
          // 角色从忙碌恢复为在线/休息时，发送暂存的待发消息
          if (newState !== 'busy' && newState !== 'offline') {
            const pending = pendingMessagesRef.current.filter((m) => m.charId === e.payload.character_id);
            if (pending.length > 0) {
              pendingMessagesRef.current = pendingMessagesRef.current.filter((m) => m.charId !== e.payload.character_id);
              for (const m of pending) {
                void ChatController.sendMessage(m.text, m.charId, 'wechat');
              }
            }
          }
        });
      } catch { /* ignore */ }
      if (cancelled) { unlisten?.(); }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  /** 状态栏时间 */
  const [statusTime, setStatusTime] = useState(() => {
    const d = new Date();
    return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
  });

  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const historyCacheRef = useRef<ChatMessage[]>([]);
  /** 竞态保护：记录最新一次 loadHistory 请求的 charId，丢弃过期的异步结果 */
  const loadHistorySeqRef = useRef(0);
  const preserveScrollRef = useRef<{ oldScrollHeight: number; oldScrollTop: number } | null>(null);
  const scrollDebounceRef = useRef<number | null>(null);
  const hasStreamingRef = useRef(false);
  const pendingRefreshRef = useRef(false);
  const refreshTimerRef = useRef<number | null>(null);
  /** typing 安全超时：chat:start 后 60s 无 chunk/done/error 自动清除"对方正在输入" */
  const typingSafetyTimerRef = useRef<number | null>(null);
  /** typing 延迟显示定时器：chat:start 后随机延迟 1-1.5s 再显示"对方正在输入"，避免突兀 */
  const typingDelayTimerRef = useRef<number | null>(null);
  /** 待发送消息队列：私聊对象忙碌时暂存消息，待状态恢复在线后发送 */
  const pendingMessagesRef = useRef<Array<{ charId: string; text: string }>>([]);
  /** 流式缓冲区：按 stream_id 累积未遇到换行符的文本 */
  const streamBuffersRef = useRef<Map<string, string>>(new Map());
  /** stream_id 到段落气泡 id 列表的映射，用于 chat:done 时清理 */
  const streamSegmentIdsRef = useRef<Map<string, string[]>>(new Map());
  /** stream_id 到当前正在流式输出的消息气泡 id（逐 chunk 更新） */
  const streamActiveIdRef = useRef<Map<string, string>>(new Map());
  /** 群聊 stream_id 到当前正在流式输出的消息气泡 id */
  const groupStreamActiveIdRef = useRef<Map<string, string>>(new Map());
  /** 视图状态 ref（供事件监听器读取最新值，避免重注册） */
  const viewRef = useRef<typeof view>('home');
  useEffect(() => { viewRef.current = view; }, [view]);
  /** 当前私聊角色 ID ref */
  const privateCharIdRef = useRef<string | null>(null);
  useEffect(() => { privateCharIdRef.current = privateCharId; }, [privateCharId]);

  /* 状态栏时钟 */
  useEffect(() => {
    const tick = () => {
      const d = new Date();
      setStatusTime(`${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`);
    };
    tick();
    const id = setInterval(tick, 15_000);
    return () => clearInterval(id);
  }, []);

  // 跟踪用户是否处于底部附近（用于流式期间判断是否自动跟随）
  const isAtBottomRef = useRef(true);
  // rAF 合并：同一帧内多次 scrollToBottom 只执行一次，避免 layout thrashing
  const scrollRafRef = useRef<number | null>(null);

  const scrollToBottom = useCallback(() => {
    if (scrollRafRef.current !== null) return;
    scrollRafRef.current = window.requestAnimationFrame(() => {
      scrollRafRef.current = null;
      const el = listRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }, []);

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (preserveScrollRef.current) {
      const { oldScrollHeight, oldScrollTop } = preserveScrollRef.current;
      el.scrollTop = oldScrollTop + (el.scrollHeight - oldScrollHeight);
      preserveScrollRef.current = null;
    } else if (isAtBottomRef.current) {
      // 仅当用户已处于底部时自动跟随，避免打断用户向上翻阅历史
      scrollToBottom();
    }
  }, [messages, cards, scrollToBottom]);

  // 清理未执行的 rAF
  useEffect(() => {
    return () => {
      if (scrollRafRef.current !== null) {
        window.cancelAnimationFrame(scrollRafRef.current);
        scrollRafRef.current = null;
      }
    };
  }, []);

  const loadHistory = useCallback(async (charId: string | null) => {
    if (!charId) {
      setMessages([]);
      historyCacheRef.current = [];
      setHistoryLoadedCount(0);
      setHasMore(false);
      return;
    }
    const seq = ++loadHistorySeqRef.current;
    setInitialLoading(true);
    // 安全兜底：如果 invoke 挂起超过 5s，强制关闭加载指示器
    const safetyTimer = window.setTimeout(() => {
      console.warn('[loadHistory] 安全超时（5s），强制关闭加载指示器');
      setInitialLoading(false);
    }, 5000);
    try {
      const t0 = performance.now();
      const entries = await invoke<HistoryEntry[]>('get_chat_history', { characterId: charId });
      const elapsed = performance.now() - t0;
      // 竞态保护：如果在 await 期间又触发了新的 loadHistory，丢弃本次过期结果
      if (loadHistorySeqRef.current !== seq) return;
      const filtered = entries.filter((e) => {
        if (e.role === 'system') return false;
        const ch = e.metadata?.channel;
        return ch === 'wechat' || ch === undefined;
      }).flatMap(toChatMessages).sort((a, b) => a.timestamp - b.timestamp);
      console.log(`[loadHistory] charId=${charId} invoke=${elapsed.toFixed(0)}ms total=${entries.length} filtered=${filtered.length}`);
      historyCacheRef.current = filtered;
      const count = Math.min(PAGE_SIZE, filtered.length);
      setMessages(filtered.slice(filtered.length - count));
      setHistoryLoadedCount(count);
      setHasMore(count < filtered.length);
    } catch (e) {
      if (loadHistorySeqRef.current !== seq) return;
      console.error('加载历史消息失败:', e);
      historyCacheRef.current = [];
      setMessages([]);
      setHistoryLoadedCount(0);
      setHasMore(false);
    } finally {
      window.clearTimeout(safetyTimer);
      setInitialLoading(false);
    }
  }, []);

  // 进入私聊视图时加载该角色历史；切换角色时重新加载
  useEffect(() => {
    if (view === 'private' && privateCharId) {
      // 切换角色时清理流式缓冲区，防止上一角色的残留 stream_id
      // 导致其 chunk/done 事件被错误追加到新角色的消息列表
      streamBuffersRef.current.clear();
      streamSegmentIdsRef.current.clear();
      streamActiveIdRef.current.clear();
      hasStreamingRef.current = false;
      pendingRefreshRef.current = false;
      setPrivateTyping(false);
      void loadHistory(privateCharId);
    } else if (view !== 'private') {
      // 离开私聊视图时清空，避免下次进入时闪现旧消息
      setMessages([]);
      historyCacheRef.current = [];
      setHistoryLoadedCount(0);
      setHasMore(false);
    }
  }, [view, privateCharId, loadHistory]);

  // 对话历史变更时静默刷新（不触发 initialLoading，保留当前已加载条数）
  // 使用 ref 访问 historyLoadedCount，避免将其作为 useCallback 依赖，
  // 否则每次条数变化都会重建 refreshHistory → 重新注册 dialogue:changed 监听器
  const refreshHistory = useCallback(async () => {
    if (!privateCharId) return;
    try {
      const entries = await invoke<HistoryEntry[]>('get_chat_history', { characterId: privateCharId });
      const filtered = entries.filter((e) => {
        if (e.role === 'system') return false;
        const ch = e.metadata?.channel;
        return ch === 'wechat' || ch === undefined;
      }).flatMap(toChatMessages).sort((a, b) => a.timestamp - b.timestamp);
      historyCacheRef.current = filtered;
      const currentCount = Math.min(Math.max(historyLoadedCountRef.current, PAGE_SIZE), filtered.length);
      const historySlice = filtered.slice(filtered.length - currentCount);

      setMessages((prev) => {
        // 保留当前显示中比历史最新记录更新的消息（如刚通过事件追加的用户消息和分段气泡）
        // 这些消息可能还没写入历史，refreshHistory 不能丢弃
        const lastHistoryTs = historySlice.length > 0
          ? historySlice[historySlice.length - 1].timestamp
          : 0;
        const newerMsgs = prev.filter((m) => {
          if (m.timestamp <= lastHistoryTs) return false;
          // 流式分段消息去重：后端 dialogue 存完整文本，前端流式按行分段追加
          // timestamp 时序竞争（后端 T1 < 前端 Date.now() T2）导致分段被保留
          // 检查历史中是否已有包含此内容的 assistant 消息，有则丢弃流式副本
          if (m.role === 'assistant' && m.content) {
            const dupInHistory = historySlice.some((h) =>
              h.role === 'assistant' && h.content && (
                h.content === m.content ||
                h.content.includes(m.content) ||
                m.content.includes(h.content)
              )
            );
            if (dupInHistory) return false;
          }
          // 用户消息去重：Busy 延后等场景下前端乐观添加的 user 消息可能与后端写入的重复
          if (m.role === 'user' && m.content) {
            const dupUserInHistory = historySlice.some((h) =>
              h.role === 'user' && h.content === m.content
            );
            if (dupUserInHistory) return false;
          }
          // 表情包消息去重：后端持久化的 sticker 在 toChatMessages 中已生成独立气泡，
          // 流式期间 chat:done 事件追加的 sticker 气泡内容相同，丢弃流式副本
          if (m.role === 'assistant' && m.sticker) {
            const dupStickerInHistory = historySlice.some((h) =>
              h.role === 'assistant' && h.sticker === m.sticker
            );
            if (dupStickerInHistory) return false;
          }
          return true;
        });
        if (newerMsgs.length === 0) {
          return historySlice;
        }
        return [...historySlice, ...newerMsgs];
      });
      setHistoryLoadedCount(currentCount);
      setHasMore(currentCount < filtered.length);
    } catch (e) {
      console.error('刷新历史消息失败:', e);
    }
  }, [privateCharId]);

  // 用 ref 包装 refreshHistory / refreshLastPreviews，使事件监听器 useEffect 依赖项变为 []，
  // 监听器只在挂载时注册一次，避免切换角色时重注册导致卡顿和事件丢失窗口
  const refreshHistoryRef = useRef(refreshHistory);
  const refreshLastPreviewsRef = useRef(refreshLastPreviews);
  useEffect(() => { refreshHistoryRef.current = refreshHistory; }, [refreshHistory]);
  useEffect(() => { refreshLastPreviewsRef.current = refreshLastPreviews; }, [refreshLastPreviews]);

  // 窗口恢复可见时静默刷新：隐藏期间 WebView 被挂起（freeze_webview 省内存），
  // 此期间的 dialogue:changed 等事件不会投递给 JS，恢复后补拉一次历史与预览
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState === 'visible') {
        void refreshHistoryRef.current();
        void refreshLastPreviewsRef.current?.();
      }
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

  // dialogue:changed 事件：后端 add_message 时 emit，debounce 500ms 后同时刷新预览和历史
  // 合并原两个独立监听器（分别刷新预览/历史），共享同一个 debounce 定时器
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    let unlistenCleared: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string }>('dialogue:changed', (event) => {
          if (cancelled) return;
          // 多角色过滤：仅在私聊视图下，非当前对象的历史刷新才跳过；
          // 主页预览、其他视图都需要响应所有角色事件，否则首页/其他角色私聊预览不刷新
          const curView = viewRef.current;
          const curPrivate = privateCharIdRef.current;
          if (curView === 'private' && curPrivate && event.payload?.character_id && event.payload.character_id !== curPrivate) return;
          if (refreshTimerRef.current !== null) {
            window.clearTimeout(refreshTimerRef.current);
          }
          // 流式生成期间暂缓刷新，标记待刷新，流式结束后立即执行
          if (hasStreamingRef.current) {
            pendingRefreshRef.current = true;
            return;
          }
          // 非流式期间：debounce 500ms 后刷新，确保后端 add_message 已完成
          refreshTimerRef.current = window.setTimeout(() => {
            refreshTimerRef.current = null;
            pendingRefreshRef.current = false;
            void refreshHistoryRef.current();
            void refreshLastPreviewsRef.current();
          }, 500);
        });
        if (cancelled) { unlisten(); return; }
      } catch { /* ignore */ }
      // 合并 chat:history-cleared：清空本地消息 + 刷新预览
      try {
        unlistenCleared = await listen<{ character_id?: string }>('chat:history-cleared', (event) => {
          if (cancelled) return;
          // 仅私聊视图下，非当前对象的清空事件才跳过；主页预览刷新始终执行
          if (viewRef.current === 'private' && event.payload?.character_id && privateCharIdRef.current && event.payload.character_id !== privateCharIdRef.current) return;
          historyCacheRef.current = [];
          setMessages([]);
          setHistoryLoadedCount(0);
          setHasMore(false);
          void refreshLastPreviewsRef.current();
        });
        if (cancelled) { unlistenCleared(); return; }
      } catch { /* ignore */ }
    })();
    return () => {
      cancelled = true;
      if (refreshTimerRef.current !== null) {
        window.clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = null;
      }
      unlisten?.();
      unlistenCleared?.();
    };
  }, []);

  // 挂载时同步后端录音状态：如果其他窗口（如 InputDialog）之前启动了录音但未停止，
  // 后端 is_recording 仍为 true。此处同步本地状态，避免 UI 与后端不一致。
  // 同时主动停止残留录音，确保 ChatWindow 从干净状态开始。
  useEffect(() => {
    void (async () => {
      try {
        const isRecording = await invoke<boolean>('get_recognition_status');
        if (isRecording) {
          await invoke('stop_recognition');
        }
      } catch { /* ignore */ }
    })();
  }, []);

  // 组件卸载时自动停止录音，防止 AsrManager.is_recording 状态泄漏
  useEffect(() => {
    return () => {
      if (recordingRef.current) {
        void invoke('stop_recognition').catch(() => {});
      }
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ language: string }>('config:language-changed', (e) => {
          if (e.payload?.language) void changeLanguage(e.payload.language);
        });
        if (cancelled) { unlisten(); return; }
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 用 ref 包装 t，避免 t 引用变化导致事件监听器重注册（i18next 在语言切换时会改变 t 引用）
  const tRef = useRef(t);
  useEffect(() => { tRef.current = t; }, [t]);

  useEffect(() => {
    let cancelled = false;
    let unTodo: UnlistenFn | undefined;
    let unSched: UnlistenFn | undefined;
    let unLinkCard: UnlistenFn | undefined;
    let unAssistantMsg: UnlistenFn | undefined;
    let unAssistantImage: UnlistenFn | undefined;
    void (async () => {
      try {
        unTodo = await listen<{ action: string; item: { title?: string; id?: string } }>('todo:changed', (event) => {
          const { action, item } = event.payload;
          let title = '';
          let subtitle = '';
          switch (action) {
            case 'added': case 'updated': case 'completed':
              title = tRef.current('chat.card_todo_title');
              subtitle = item.title || '';
              break;
            default: return;
          }
          setCards((prev) => [...prev, { id: nextId(), cardType: 'todo', action, title, subtitle, timestamp: Date.now() }]);
        });
        if (cancelled) { unTodo(); return; }
        unSched = await listen<{ action: string; task: { message?: string; id?: string } }>('scheduler:changed', (event) => {
          const { action, task } = event.payload;
          let title = '';
          let subtitle = '';
          switch (action) {
            case 'added': case 'triggered':
              title = tRef.current('chat.card_scheduler_title');
              subtitle = task.message || '';
              break;
            default: return;
          }
          setCards((prev) => [...prev, { id: nextId(), cardType: 'scheduler', action, title, subtitle, timestamp: Date.now() }]);
        });
        if (cancelled) { unSched(); return; }
        // chat:link_card：AI 分享链接卡片（微信风格）
        unLinkCard = await listen<{ url: string; title: string; description?: string; source?: string; timestamp?: string; character_id?: string; channel?: string; follow_up?: string }>('chat:link_card', (event) => {
          if (cancelled || !event.payload) return;
          const cid = event.payload.character_id;
          const ch = event.payload.channel;
          const ts = event.payload.timestamp ? normalizeTimestamp(event.payload.timestamp) : Date.now();
          const linkCard = {
            url: event.payload.url,
            title: event.payload.title,
            description: event.payload.description || '',
            source: event.payload.source || '',
          };
          if (viewRef.current === 'group') {
            // 群聊视图：追加到群聊消息列表
            if (ch && ch !== 'wechat_group') return;
            setGroupMessages((prev) => [...prev, {
              id: nextId(),
              role: 'assistant',
              content: '',
              timestamp: ts,
              linkCard,
              character_id: cid,
            }]);
            return;
          }
          // 私聊视图：只处理当前私聊角色的卡片，且只处理 wechat 频道
          if (viewRef.current === 'private') {
            if (ch && ch !== 'wechat') return;
            if (cid && privateCharIdRef.current && cid !== privateCharIdRef.current) return;
          }
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: '',
            timestamp: ts,
            linkCard,
          }]);
        });
        if (cancelled) { unLinkCard(); return; }
        // chat:assistant_message：外部触发的助手消息（无 stream_id 的非流式消息，
        // 如定时提醒、share_link 跟进评论等。正常对话走 chunk/done 流式通道，此处避免重复）
        unAssistantMsg = await listen<{ content: string; timestamp?: string; character_id?: string; channel?: string; stream_id?: string }>('chat:assistant_message', (event) => {
          if (cancelled || !event.payload) return;
          // 有 stream_id 的是流式消息（ChatController 正常对话），已通过 chunk/done 处理，跳过
          if (event.payload.stream_id) return;
          const ch = event.payload.channel;
          const cid = event.payload.character_id;
          const text = event.payload.content?.trim();
          if (!text) return;
          const ts = event.payload.timestamp ? normalizeTimestamp(event.payload.timestamp) : Date.now();

          if (viewRef.current === 'group') {
            // 群聊模式：追加到群聊消息列表
            if (ch && ch !== 'wechat_group') return;
            setGroupMessages((prev) => [...prev, {
              id: nextId(),
              role: 'assistant',
              content: text,
              timestamp: ts,
              character_id: cid,
            }]);
            return;
          }
          // 群聊频道消息但用户不在群聊视图：增加群聊未读计数
          if (ch === 'wechat_group') {
            setUnreadCounts((prev) => ({ ...prev, group: (prev.group ?? 0) + 1 }));
            return;
          }
          // 私聊视图：只处理 wechat 频道消息
          if (ch && ch !== 'wechat') return;
          if (viewRef.current === 'private' && cid && privateCharIdRef.current && cid !== privateCharIdRef.current) return;
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: text,
            timestamp: ts,
          }]);
          // 立即刷新主面板私聊预览（乐观更新，不等 dialogue:changed debounce）
          if (cid) {
            setLastPreviews((prev) => ({
              ...prev,
              [cid]: { content: text, timestamp: ts, role: 'assistant' },
            }));
          }
          // 即时未读计数：不在对应私聊视图时，红点立即+1（不依赖 dialogue:changed debounce）
          {
            const convId = cid ?? '';
            const isViewing = viewRef.current === 'private' && privateCharIdRef.current === convId;
            if (convId && !isViewing) {
              setUnreadCounts((prev) => ({ ...prev, [convId]: (prev[convId] ?? 0) + 1 }));
            }
          }
        });
        if (cancelled) { unAssistantMsg(); return; }
        // chat:assistant_image：AI 发送图片（send_image 工具），立即在聊天列表中插入 AI 图片气泡。
        // 历史重载时由 metadata.kind=image 渲染（toChatMessages 已支持 assistant 角色）。
        unAssistantImage = await listen<{ data_url: string; image_path: string; timestamp?: number | string; character_id?: string; channel?: string }>('chat:assistant_image', (event) => {
          if (cancelled || !event.payload) return;
          const ch = event.payload.channel;
          const cid = event.payload.character_id;
          const ts = event.payload.timestamp ? normalizeTimestamp(event.payload.timestamp) : Date.now();
          // 群聊视图不展示微信频道图片（与 chat:assistant_message 行为一致）
          if (viewRef.current === 'group') return;
          if (ch && ch !== 'wechat') return;
          if (viewRef.current === 'private' && cid && privateCharIdRef.current && cid !== privateCharIdRef.current) return;
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: '',
            timestamp: ts,
            imageDataUrl: event.payload.data_url,
            imagePath: event.payload.image_path,
          }]);
          // 立即刷新主面板私聊预览（图片消息预览为 [图片]）
          if (cid) {
            setLastPreviews((prev) => ({
              ...prev,
              [cid]: { content: '', timestamp: ts, role: 'assistant', imagePath: event.payload.image_path, characterId: cid },
            }));
          }
          // 即时未读计数：不在对应私聊视图时，红点立即+1
          {
            const isViewing = viewRef.current === 'private' && privateCharIdRef.current === cid;
            if (cid && !isViewing) {
              setUnreadCounts((prev) => ({ ...prev, [cid]: (prev[cid] ?? 0) + 1 }));
            }
          }
        });
        if (cancelled) { unAssistantImage(); return; }
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unTodo?.(); unSched?.(); unLinkCard?.(); unAssistantMsg?.(); unAssistantImage?.(); };
  }, []);

  /** 抽屉打开时，点击非抽屉与非触发按钮区域即收起 */
  useEffect(() => {
    if (bottomPanel === 'none') return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      const inDrawer = bottomPanelDrawerRef.current?.contains(target);
      const inTrigger = bottomPanelTriggersRef.current?.contains(target);
      if (!inDrawer && !inTrigger) setBottomPanel('none');
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [bottomPanel]);

  useEffect(() => {
    let cancelled = false;
    let unlistenUser: UnlistenFn | undefined;
    let unlistenUserImage: UnlistenFn | undefined;
    let unlistenStart: UnlistenFn | undefined;
    let unlistenChunk: UnlistenFn | undefined;
    let unlistenDone: UnlistenFn | undefined;
    let unlistenError: UnlistenFn | undefined;
    let unlistenCancelled: UnlistenFn | undefined;
    let unlistenYielded: UnlistenFn | undefined;
    void (async () => {
      unlistenUser = await listen<{ content: string; timestamp: string; character_id?: string; channel?: string }>('chat:user_message', (event) => {
        const ch = event.payload.channel;
        if (ch && ch !== 'wechat') return;
        // 语音消息：本地已添加语音气泡，跳过此处重复的文本气泡
        if (skipNextUserMessageRef.current) {
          skipNextUserMessageRef.current = false;
          return;
        }
        // 群聊视图：用户消息由 handleSend 直接添加，跳过（避免每个角色各发一次导致重复）
        if (viewRef.current === 'group') return;
        // 私聊视图：只处理当前私聊角色的消息
        if (viewRef.current === 'private' && event.payload.character_id && event.payload.character_id !== privateCharIdRef.current) return;
        setMessages((prev) => [...prev, { id: nextId(), role: 'user', content: event.payload.content, timestamp: normalizeTimestamp(event.payload.timestamp) }]);
      });
      if (cancelled) { unlistenUser(); return; }
      // chat:user_image：用户发送图片，立即在聊天列表中插入图片气泡
      unlistenUserImage = await listen<{ data_url: string; image_path: string; timestamp?: number | string; character_id?: string; channel?: string }>('chat:user_image', (event) => {
        const ch = event.payload.channel;
        if (ch && ch !== 'wechat') return;
        if (viewRef.current === 'group') return;
        if (viewRef.current === 'private' && event.payload.character_id && event.payload.character_id !== privateCharIdRef.current) return;
        setMessages((prev) => [...prev, {
          id: nextId(), role: 'user', content: '',
          timestamp: event.payload.timestamp ? normalizeTimestamp(event.payload.timestamp) : Date.now(),
          imageDataUrl: event.payload.data_url,
          imagePath: event.payload.image_path,
        }]);
      });
      if (cancelled) { unlistenUserImage(); return; }
      // chat:start：初始化流式缓冲区，不创建占位气泡，标题显示"对方正在输入..."
      unlistenStart = await listen<{ message: string; stream_id: string; character_id?: string; channel?: string }>('chat:start', (event) => {
        const sid = event.payload.stream_id;
        if (!sid) return;
        const cid = event.payload.character_id ?? '';
        const ch = event.payload.channel;
        // 群聊流：stream_id 在 groupStreamCharMapRef 中预注册（由 handleSend 群发时写入）
        if (groupStreamCharMapRef.current.has(sid)) {
          if (ch && ch !== 'wechat_group') return;
          groupStreamBuffersRef.current.set(sid, '');
          setGroupStreaming(true);
          return;
        }
        // 私聊流：仅处理当前私聊角色，且 channel 必须是 wechat（或未设置，兼容旧数据）
        if (ch && ch !== 'wechat') return;
        if (viewRef.current !== 'private') return;
        if (cid && cid !== privateCharIdRef.current) return;
        hasStreamingRef.current = true;
        // 随机延迟 1-1.5s 再显示"对方正在输入"，避免发送后立刻显示的突兀感
        if (typingDelayTimerRef.current !== null) window.clearTimeout(typingDelayTimerRef.current);
        const typingDelay = 1000 + Math.random() * 500;
        typingDelayTimerRef.current = window.setTimeout(() => {
          typingDelayTimerRef.current = null;
          if (hasStreamingRef.current) setPrivateTyping(true);
        }, typingDelay);
        // 安全超时：60s 后自动清除 typing（防止 chat:done 事件丢失导致指示器卡死）
        if (typingSafetyTimerRef.current !== null) window.clearTimeout(typingSafetyTimerRef.current);
        typingSafetyTimerRef.current = window.setTimeout(() => {
          typingSafetyTimerRef.current = null;
          if (typingDelayTimerRef.current !== null) { window.clearTimeout(typingDelayTimerRef.current); typingDelayTimerRef.current = null; }
          if (hasStreamingRef.current) {
            console.warn('[chat:start] typing 安全超时：自动清除对方正在输入');
            hasStreamingRef.current = false;
            setPrivateTyping(false);
            pendingRefreshRef.current = true;
            void refreshHistoryRef.current();
            void refreshLastPreviewsRef.current();
          }
        }, 60000);
        streamBuffersRef.current.set(sid, '');
        streamSegmentIdsRef.current.set(sid, []);
      });
      if (cancelled) { unlistenStart(); return; }
      // chat:chunk：静默累积到缓冲区，遇到换行符时输出完整段落气泡（无流式占位气泡）
      unlistenChunk = await listen<{ text: string; stream_id?: string; character_id?: string; channel?: string }>('chat:chunk', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!sid) return;
        const chunk = event.payload.text;
        const ch = event.payload.channel;
        // 群聊流路由
        if (groupStreamCharMapRef.current.has(sid)) {
          if (ch && ch !== 'wechat_group') return;
          const cid = groupStreamCharMapRef.current.get(sid) ?? '';
          const buf = (groupStreamBuffersRef.current.get(sid) ?? '') + chunk;
          const parts = buf.split('\n');
          if (parts.length > 1) {
            // 有换行符：除最后一段外都是完整段落，各自输出为已结算气泡
            const completeParts = parts.slice(0, -1).map((p) => p.trim()).filter((p) => p.length > 0);
            const remaining = parts[parts.length - 1];
            groupStreamBuffersRef.current.set(sid, remaining);
            if (completeParts.length > 0) {
              const newMsgs: ChatMessage[] = completeParts.map((part) => ({
                id: nextId(),
                role: 'assistant',
                content: part,
                timestamp: Date.now(),
                streaming: false,
                character_id: cid,
              }));
              setGroupMessages((prev) => [...prev, ...newMsgs]);
            }
          } else {
            // 无换行符：仅累积，不输出气泡
            groupStreamBuffersRef.current.set(sid, buf);
          }
          return;
        }
        // 私聊流路由：校验 channel 和 character_id 防止跨渠道/跨角色消息污染
        if (ch && ch !== 'wechat') return;
        const chunkCid = event.payload.character_id;
        if (chunkCid && privateCharIdRef.current && chunkCid !== privateCharIdRef.current) return;
        if (!streamBuffersRef.current.has(sid)) return;
        const buf = (streamBuffersRef.current.get(sid) ?? '') + chunk;
        const parts = buf.split('\n');
        if (parts.length > 1) {
          // 有换行符：除最后一段外都是完整段落，各自输出为已结算气泡
          const completeParts = parts.slice(0, -1).map((p) => p.trim()).filter((p) => p.length > 0);
          const remaining = parts[parts.length - 1];
          streamBuffersRef.current.set(sid, remaining);
          if (completeParts.length > 0) {
            const newMsgs: ChatMessage[] = completeParts.map((part) => ({
              id: nextId(),
              role: 'assistant',
              content: part,
              timestamp: Date.now(),
              streaming: false,
            }));
            setMessages((prev) => [...prev, ...newMsgs]);
          }
        } else {
          // 无换行符：仅累积，不输出气泡
          streamBuffersRef.current.set(sid, buf);
        }
      });
      if (cancelled) { unlistenChunk(); return; }
      // chat:done：输出缓冲区剩余文本作为最终气泡，清理流式状态
      unlistenDone = await listen<{ text: string; stream_id?: string; sticker?: string; character_id?: string; channel?: string; voice_message?: boolean; voice_audio_path?: string | null; voice_duration?: number | null }>('chat:done', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!sid) return;
        const finalText = event.payload.text || '';
        const sticker = event.payload.sticker ?? '';
        const isVoiceMessage = !!event.payload.voice_message && !!event.payload.voice_audio_path;
        const ch = event.payload.channel;
        // 群聊流路由
        if (groupStreamCharMapRef.current.has(sid)) {
          if (ch && ch !== 'wechat_group') return;
          const cid = groupStreamCharMapRef.current.get(sid) ?? '';
          const buf = groupStreamBuffersRef.current.get(sid) ?? '';
          // 输出缓冲区剩余文本（未遇到换行符的最后一段）
          const trimmedBuf = buf.trim();
          // 合并文本+表情为一次 setGroupMessages，避免双倍渲染开销
          const newMsgs: ChatMessage[] = [];
          if (isVoiceMessage) {
            // 语音消息：不显示文本，以语音气泡发出
            newMsgs.push({
              id: nextId(),
              role: 'assistant',
              content: '',
              timestamp: Date.now(),
              streaming: false,
              character_id: cid,
              voice: {
                audioPath: event.payload.voice_audio_path ?? undefined,
                duration: event.payload.voice_duration ?? 0,
              },
            });
          } else if (trimmedBuf) {
            newMsgs.push({
              id: nextId(),
              role: 'assistant',
              content: trimmedBuf,
              timestamp: Date.now(),
              streaming: false,
              character_id: cid,
            });
          }
          if (sticker) {
            newMsgs.push({
              id: nextId(),
              role: 'assistant',
              content: '',
              timestamp: Date.now() + 1,
              streaming: false,
              sticker,
              character_id: cid,
            });
          }
          if (newMsgs.length > 0) {
            setGroupMessages((prev) => [...prev, ...newMsgs]);
          }
          // 立即刷新主面板群聊预览（乐观更新）
          const previewText = isVoiceMessage ? '[语音]' : (finalText.trim() || buf.trim());
          if (previewText) {
            setLastPreviews((prev) => ({
              ...prev,
              group: { content: previewText, timestamp: Date.now(), role: 'assistant', characterId: cid },
            }));
          }
          groupStreamBuffersRef.current.delete(sid);
          groupStreamCharMapRef.current.delete(sid);
          groupStreamActiveIdRef.current.delete(sid);
          if (groupStreamCharMapRef.current.size === 0) setGroupStreaming(false);
          // 用前端渲染时刻回传后端，覆盖持久化时使用的后端构造时刻，避免 refreshHistory 合并重复
          void invoke('update_last_assistant_timestamp', { characterId: cid, timestampMs: Date.now() }).catch(() => {});
          return;
        }
        // 私聊流路由：校验 channel 和 character_id 防止跨渠道/跨角色消息污染
        if (ch && ch !== 'wechat') return;
        const doneCid = event.payload.character_id;
        if (doneCid && privateCharIdRef.current && doneCid !== privateCharIdRef.current) return;
        if (!streamBuffersRef.current.has(sid)) return;
        const buf = streamBuffersRef.current.get(sid) ?? '';
        // 输出缓冲区剩余文本（未遇到换行符的最后一段）
        const trimmedBuf = buf.trim();
        if (isVoiceMessage) {
          // 语音消息：不显示流式文本，以语音气泡发出
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: '',
            timestamp: Date.now(),
            streaming: false,
            voice: {
              audioPath: event.payload.voice_audio_path ?? undefined,
              duration: event.payload.voice_duration ?? 0,
            },
          }]);
        } else if (trimmedBuf) {
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: trimmedBuf,
            timestamp: Date.now(),
            streaming: false,
          }]);
        }
        // 表情包：作为独立消息追加在文本之后
        if (sticker) {
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: '',
            timestamp: Date.now() + 1,
            streaming: false,
            sticker,
          }]);
        }
        // 立即刷新主面板私聊预览（乐观更新）
        const privatePreviewText = isVoiceMessage ? '[语音]' : finalText.trim();
        const previewCharId = privateCharIdRef.current;
        if (privatePreviewText && previewCharId) {
          setLastPreviews((prev) => ({
            ...prev,
            [previewCharId]: { content: privatePreviewText, timestamp: Date.now(), role: 'assistant' },
          }));
        }
        streamBuffersRef.current.delete(sid);
        streamSegmentIdsRef.current.delete(sid);
        streamActiveIdRef.current.delete(sid);
        if (typingDelayTimerRef.current !== null) { window.clearTimeout(typingDelayTimerRef.current); typingDelayTimerRef.current = null; }
        if (typingSafetyTimerRef.current !== null) { window.clearTimeout(typingSafetyTimerRef.current); typingSafetyTimerRef.current = null; }
        hasStreamingRef.current = false;
        setPrivateTyping(false);
        // 用前端渲染时刻回传后端，覆盖持久化时使用的后端构造时刻，避免 refreshHistory 合并重复
        const assistantCharId = privateCharIdRef.current ?? doneCid;
        if (assistantCharId) {
          void invoke('update_last_assistant_timestamp', { characterId: assistantCharId, timestampMs: Date.now() }).catch(() => {});
        }
        // 安全兜底：流式结束后延迟刷新历史，确保 AI 回复已写入 dialogue
        // 正常路径由 dialogue:changed 触发刷新，此处作为保底
        pendingRefreshRef.current = true;
        if (refreshTimerRef.current !== null) window.clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = window.setTimeout(() => {
          refreshTimerRef.current = null;
          if (pendingRefreshRef.current) {
            pendingRefreshRef.current = false;
            void refreshHistoryRef.current();
            void refreshLastPreviewsRef.current();
          }
        }, 800);
      });
      if (cancelled) { unlistenDone(); return; }
      // chat:error：清理流式状态，输出残余文本（如有）
      unlistenError = await listen<{ error: string; stream_id?: string; character_id?: string; channel?: string }>('chat:error', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!sid) return;
        const ch = event.payload.channel;
        // 群聊流路由
        if (groupStreamCharMapRef.current.has(sid)) {
          if (ch && ch !== 'wechat_group') return;
          const cid = groupStreamCharMapRef.current.get(sid) ?? '';
          const buf = groupStreamBuffersRef.current.get(sid) ?? '';
          const activeId = groupStreamActiveIdRef.current.get(sid);
          if (activeId) {
            setGroupMessages((prev) => prev.map((m) =>
              m.id === activeId ? { ...m, content: buf || m.content, streaming: false, error: true } : m
            ));
          } else if (buf.trim()) {
            setGroupMessages((prev) => [...prev, {
              id: nextId(),
              role: 'assistant',
              content: buf,
              timestamp: Date.now(),
              streaming: false,
              error: true,
              character_id: cid,
            }]);
          }
          groupStreamBuffersRef.current.delete(sid);
          groupStreamCharMapRef.current.delete(sid);
          groupStreamActiveIdRef.current.delete(sid);
          if (groupStreamCharMapRef.current.size === 0) setGroupStreaming(false);
          return;
        }
        // 私聊流路由：校验 channel 和 character_id
        if (ch && ch !== 'wechat') return;
        const errCid = event.payload.character_id;
        if (errCid && privateCharIdRef.current && errCid !== privateCharIdRef.current) return;
        if (!streamBuffersRef.current.has(sid)) return;
        const buf = streamBuffersRef.current.get(sid) ?? '';
        const activeId = streamActiveIdRef.current.get(sid);
        if (activeId) {
          setMessages((prev) => prev.map((m) =>
            m.id === activeId ? { ...m, content: buf || m.content, streaming: false, error: true } : m
          ));
        } else if (buf.trim()) {
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: buf,
            timestamp: Date.now(),
            streaming: false,
            error: true,
          }]);
        }
        streamBuffersRef.current.delete(sid);
        streamSegmentIdsRef.current.delete(sid);
        streamActiveIdRef.current.delete(sid);
        if (typingDelayTimerRef.current !== null) { window.clearTimeout(typingDelayTimerRef.current); typingDelayTimerRef.current = null; }
        if (typingSafetyTimerRef.current !== null) { window.clearTimeout(typingSafetyTimerRef.current); typingSafetyTimerRef.current = null; }
        hasStreamingRef.current = false;
        setPrivateTyping(false);
        pendingRefreshRef.current = true;
        if (refreshTimerRef.current !== null) window.clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = window.setTimeout(() => {
          refreshTimerRef.current = null;
          if (pendingRefreshRef.current) {
            pendingRefreshRef.current = false;
            void refreshHistoryRef.current();
            void refreshLastPreviewsRef.current();
          }
        }, 800);
      });
      if (cancelled) { unlistenError(); return; }
      // chat:cancelled：清理流式状态
      unlistenCancelled = await listen<{ stream_id?: string; character_id?: string; channel?: string }>('chat:cancelled', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!sid) return;
        const ch = event.payload.channel;
        // 群聊流路由
        if (groupStreamCharMapRef.current.has(sid)) {
          if (ch && ch !== 'wechat_group') return;
          const cid = groupStreamCharMapRef.current.get(sid) ?? '';
          const buf = groupStreamBuffersRef.current.get(sid) ?? '';
          const activeId = groupStreamActiveIdRef.current.get(sid);
          if (activeId) {
            setGroupMessages((prev) => prev.map((m) =>
              m.id === activeId ? { ...m, content: buf || m.content, streaming: false } : m
            ));
          } else if (buf.trim()) {
            setGroupMessages((prev) => [...prev, {
              id: nextId(),
              role: 'assistant',
              content: buf,
              timestamp: Date.now(),
              streaming: false,
              character_id: cid,
            }]);
          }
          groupStreamBuffersRef.current.delete(sid);
          groupStreamCharMapRef.current.delete(sid);
          groupStreamActiveIdRef.current.delete(sid);
          if (groupStreamCharMapRef.current.size === 0) setGroupStreaming(false);
          return;
        }
        // 私聊流路由：校验 channel 和 character_id
        if (ch && ch !== 'wechat') return;
        const cancelCid = event.payload.character_id;
        if (cancelCid && privateCharIdRef.current && cancelCid !== privateCharIdRef.current) return;
        if (!streamBuffersRef.current.has(sid)) return;
        const buf = streamBuffersRef.current.get(sid) ?? '';
        const activeId = streamActiveIdRef.current.get(sid);
        if (activeId) {
          setMessages((prev) => prev.map((m) =>
            m.id === activeId ? { ...m, content: buf || m.content, streaming: false } : m
          ));
        } else if (buf.trim()) {
          setMessages((prev) => [...prev, {
            id: nextId(),
            role: 'assistant',
            content: buf,
            timestamp: Date.now(),
            streaming: false,
          }]);
        }
        streamBuffersRef.current.delete(sid);
        streamSegmentIdsRef.current.delete(sid);
        streamActiveIdRef.current.delete(sid);
        if (typingDelayTimerRef.current !== null) { window.clearTimeout(typingDelayTimerRef.current); typingDelayTimerRef.current = null; }
        if (typingSafetyTimerRef.current !== null) { window.clearTimeout(typingSafetyTimerRef.current); typingSafetyTimerRef.current = null; }
        hasStreamingRef.current = false;
        setPrivateTyping(false);
        pendingRefreshRef.current = true;
        if (refreshTimerRef.current !== null) window.clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = window.setTimeout(() => {
          refreshTimerRef.current = null;
          if (pendingRefreshRef.current) {
            pendingRefreshRef.current = false;
            void refreshHistoryRef.current();
            void refreshLastPreviewsRef.current();
          }
        }, 800);
      });
      if (cancelled) { unlistenCancelled(); return; }
      // chat:yielded：群聊让位协议——角色未被点名选择不回复，静默清理流式状态（不产生消息气泡）
      unlistenYielded = await listen<{ stream_id?: string; character_id?: string; channel?: string }>('chat:yielded', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!sid) return;
        if (!groupStreamCharMapRef.current.has(sid)) return;
        groupStreamBuffersRef.current.delete(sid);
        groupStreamCharMapRef.current.delete(sid);
        groupStreamActiveIdRef.current.delete(sid);
        if (groupStreamCharMapRef.current.size === 0) setGroupStreaming(false);
      });
      if (cancelled) { unlistenYielded(); return; }
    })();
    return () => { cancelled = true; unlistenUser?.(); unlistenUserImage?.(); unlistenStart?.(); unlistenChunk?.(); unlistenDone?.(); unlistenError?.(); unlistenCancelled?.(); unlistenYielded?.(); if (typingDelayTimerRef.current !== null) { window.clearTimeout(typingDelayTimerRef.current); typingDelayTimerRef.current = null; } if (typingSafetyTimerRef.current !== null) { window.clearTimeout(typingSafetyTimerRef.current); typingSafetyTimerRef.current = null; } };
  }, []);

  const loadMore = useCallback(() => {
    const cache = historyCacheRef.current;
    if (!cache.length || loadingMore || !hasMore) return;
    const el = listRef.current;
    if (el) preserveScrollRef.current = { oldScrollHeight: el.scrollHeight, oldScrollTop: el.scrollTop };
    const prevCount = historyLoadedCount;
    const newCount = Math.min(prevCount + PAGE_SIZE, cache.length);
    const olderSlice = cache.slice(Math.max(0, cache.length - newCount), Math.max(0, cache.length - prevCount));
    setLoadingMore(true);
    window.setTimeout(() => {
      setMessages((prev) => [...olderSlice, ...prev]);
      setHistoryLoadedCount(newCount);
      setHasMore(newCount < cache.length);
      setLoadingMore(false);
    }, 0);
  }, [historyLoadedCount, hasMore, loadingMore]);

  const handleScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    // 跟踪用户是否处于底部附近（距底 < 80px 视为在底部），用于流式期间自动跟随判断
    isAtBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (el.scrollTop <= SCROLL_LOAD_THRESHOLD && hasMore && !loadingMore && !initialLoading) {
      if (scrollDebounceRef.current !== null) clearTimeout(scrollDebounceRef.current);
      scrollDebounceRef.current = window.setTimeout(() => { scrollDebounceRef.current = null; loadMore(); }, SCROLL_DEBOUNCE_MS);
    }
  }, [hasMore, loadingMore, initialLoading, loadMore]);

  useEffect(() => { return () => { if (scrollDebounceRef.current !== null) clearTimeout(scrollDebounceRef.current); }; }, []);

  const items = useMemo<RenderItem[]>(() => {
    type Merged =
      | { kind: 'msg'; msg: ChatMessage; ts: number }
      | { kind: 'card'; card: CardMessage; ts: number };
    const merged: Merged[] = [
      ...messages.map((m) => ({ kind: 'msg' as const, msg: m, ts: m.timestamp })),
      ...cards.map((c) => ({ kind: 'card' as const, card: c, ts: c.timestamp })),
    ];
    merged.sort((a, b) => a.ts - b.ts);
    const result: RenderItem[] = [];
    let prevTs: number | null = null;
    for (const it of merged) {
      if (prevTs === null || it.ts - prevTs > TIME_GAP_MS) {
        const key = it.kind === 'msg' ? `t-${it.msg.id}` : `t-${it.card.id}`;
        result.push({ kind: 'time', key, text: formatSeparatorTime(it.ts, t) });
      }
      if (it.kind === 'msg') {
        result.push({ kind: 'msg', key: it.msg.id, msg: it.msg });
      } else {
        result.push({ kind: 'card', key: it.card.id, card: it.card, timestamp: it.ts });
      }
      prevTs = it.ts;
    }
    return result;
  }, [messages, cards, t]);

  // 长列表虚拟化
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 80,
    overscan: 5,
    getItemKey: (i) => items[i].key,
  });

  /** 从文本中解析被 @ 提及的在线角色 ID 集合 */
  const parseMentionedCharIds = useCallback((text: string) => {
    const mentioned = new Set<string>();
    for (const c of characters) {
      if (!c.online) continue;
      const regex = new RegExp(`@${c.name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`, 'i');
      if (regex.test(text)) mentioned.add(c.id);
    }
    return mentioned;
  }, [characters]);

  /** 添加图片到草稿（base64 dataUrl + mime） */
  const addDraftImages = useCallback((files: File[]) => {
    if (files.length === 0) return;
    const results: { id: string; dataUrl: string; name: string; mime: string }[] = [];
    let done = 0;
    const total = files.length;
    for (const file of files) {
      if (!file.type.startsWith('image/')) continue;
      const reader = new FileReader();
      reader.onload = () => {
        draftSeqRef.current += 1;
        results.push({
          id: `draft-${draftSeqRef.current}`,
          dataUrl: String(reader.result ?? ''),
          name: file.name || '图片',
          mime: file.type,
        });
        done += 1;
        if (done === total) setDraftImages((prev) => [...prev, ...results]);
      };
      reader.onerror = () => { done += 1; };
      reader.readAsDataURL(file);
    }
  }, []);

  /** 移除一张草稿图片 */
  const removeDraftImage = useCallback((id: string) => {
    setDraftImages((prev) => prev.filter((d) => d.id !== id));
  }, []);

  /** 发送草稿图片（逐张走 send_image_message），群聊遍历在线角色 */
  const sendDraftImages = useCallback(async (draft: typeof draftImages): Promise<{ failed: typeof draftImages }> => {
    const failed: typeof draftImages = [];
    const targetCharIds = view === 'private' && privateCharId
      ? [privateCharId]
      : view === 'group'
        ? characters.filter((c) => c.online).map((c) => c.id)
        : [];
    const channel = view === 'group' ? 'wechat_group' : 'wechat';
    if (targetCharIds.length === 0) return { failed: draft };
    for (let i = 0; i < draft.length; i += 1) {
      const img = draft[i];
      try {
        // base64 dataUrl → 临时文件供后端读取（复用现有保存管道）
        const base64 = img.dataUrl.split(',')[1] ?? '';
        if (!base64) { failed.push(img); continue; }
        const tmpPath = await invoke<string>('save_temp_image', { base64Data: base64, mime: img.mime });
        for (const cid of targetCharIds) {
          await invoke('send_image_message', { sourcePath: tmpPath, characterId: cid, channel });
        }
      } catch (e) {
        console.warn('[ChatWindow] 草稿图片发送失败:', e);
        failed.push(img);
      }
    }
    return { failed };
  }, [view, privateCharId, characters]);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    const hasImages = draftImages.length > 0;
    if (!text && !hasImages) return;
    // 发送成功后才清空；失败恢复草稿
    const pendingDrafts = draftImages;
    setInput('');
    setMentionState({ active: false, query: '', startIndex: -1, selectedIndex: 0 });

    if (view === 'group') {
      // 群聊：用户消息只添加一次，然后发给目标角色
      const userTs = Date.now();
      setGroupMessages((prev) => [...prev, { id: nextId(), role: 'user', content: text, timestamp: userTs }]);
      // 立即刷新主面板预览（乐观更新，不等 dialogue:changed 的 500ms debounce）
      setLastPreviews((prev) => ({
        ...prev,
        group: { content: text, timestamp: userTs, role: 'user' },
      }));
      // @ 提及路由：有 @ 标记时只发给被提及的在线角色，未被 @ 的角色不回应；
      // 无 @ 标记时群发给所有在线角色
      const mentionedIds = parseMentionedCharIds(text);
      const onlineChars = mentionedIds.size > 0
        ? characters.filter((c) => c.online && mentionedIds.has(c.id))
        : characters.filter((c) => c.online);
      // 为每个目标角色生成独立 stream_id 并预注册映射，后端会串行处理
      for (const c of onlineChars) {
        const sid = (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function')
          ? crypto.randomUUID()
          : `s-${Date.now()}-${Math.random().toString(36).slice(2)}`;
        groupStreamCharMapRef.current.set(sid, c.id);
        groupStreamBuffersRef.current.set(sid, '');
        void invoke('send_message_stream', { message: text, streamId: sid, characterId: c.id, channel: 'wechat_group' }).catch((err) => {
          console.warn(`[群发] 角色 ${c.id} 发送失败:`, err);
          groupStreamCharMapRef.current.delete(sid);
          groupStreamBuffersRef.current.delete(sid);
        });
      }
      if (onlineChars.length > 0) setGroupStreaming(true);
    }

    if (view === 'private' && privateCharId) {
      // 立即刷新主面板预览（乐观更新，不等 dialogue:changed 的 500ms debounce）
      setLastPreviews((prev) => ({
        ...prev,
        [privateCharId]: { content: text, timestamp: Date.now(), role: 'user' },
      }));
      // 忙碌状态下暂存消息，等状态恢复在线后再发送
      if (presenceStates[privateCharId] === 'busy') {
        // 立即在聊天列表显示用户消息（不经过 ChatController，不会触发 chat:user_message 事件）
        setMessages((prev) => [...prev, { id: nextId(), role: 'user', content: text, timestamp: Date.now() }]);
        pendingMessagesRef.current.push({ charId: privateCharId, text });
      } else if (text) {
        void ChatController.sendMessage(text, privateCharId, 'wechat');
      }
    }

    // 发送草稿图片（群聊已在上方群发文本，私聊已发文本）；失败保留为草稿供重试
    if (hasImages) {
      setDraftImages([]);
      const { failed } = await sendDraftImages(pendingDrafts);
      if (failed.length > 0) {
        setDraftImages(failed);
        void emit('toast:show', {
          message: `${failed.length} 张图片发送失败，已保留供重试`, type: 'warning', duration: 4000, key: Date.now(),
        });
      }
    }
    // home 视图：有图片时也可发送（发到当前角色）
    if (!view || (view as string) === '') {
      // no-op
    }
  }, [input, view, characters, privateCharId, parseMentionedCharIds, presenceStates, draftImages, sendDraftImages]);

  const toggleRecording = useCallback(async () => {
    const isRecording = recordingRef.current;
    try {
      if (isRecording) {
        await invoke('stop_recognition');
        setRecording(false);
      } else {
        asrBaseLenRef.current = inputRef.current?.value.length ?? 0;
        await invoke('start_recognition', { characterId: privateCharId ?? undefined });
        setRecording(true);
      }
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e));
      console.warn('语音识别切换失败:', e);
      void emit('toast:show', { message: msg, type: 'warning', duration: 8000, key: Date.now() });
    }
  }, [privateCharId]);

  // 录音期间监听 ASR 事件，把识别结果实时写入输入框
  // - final_result：追加到已确认文本
  // - partial_result：替换尾部未确认片段
  const asrPartialRef = useRef('');
  // 本次录音开始时输入框已有文本长度（润色只处理 ASR 追加的尾部）
  const asrBaseLenRef = useRef(0);
  useEffect(() => {
    if (!recording) { asrPartialRef.current = ''; return; }
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    (async () => {
      unlisten = await listen<{
        type: string;
        text?: string;
        confidence?: number;
        message?: string;
      }>('asr:event', (e) => {
        const { type, text } = e.payload;
        if (type === 'final_result' && text) {
          setInput((prev) => {
            // WinRT 在 ResultGenerated 前会先发一条文本几乎相同的 HypothesisGenerated(partial)。
            // final 是对 partial 的确认/修正，必须先从 prev 中移除已显示的 partial 尾部，
            // 再追加 final 文本，否则同一段文字会被写两遍。
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = '';
            const separator = base === '' || base.endsWith(' ') ? '' : ' ';
            return base + separator + text;
          });
        } else if (type === 'partial_result' && text) {
          setInput((prev) => {
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = text;
            return base + text;
          });
        } else if (type === 'error') {
          console.warn('ASR 错误:', e.payload.message);
        } else if (type === 'stopped') {
          setRecording(false);
        }
      });
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      asrPartialRef.current = '';
    };
  }, [recording]);

  // 录音结束后（手动/静音停止），对输入框中 ASR 追加的尾部文本做 LLM 润色
  // 延迟 300ms 等待尾部 final_result 写入；用户已手动编辑或润色失败时保留原文
  const prevRecordingRef = useRef(false);
  useEffect(() => {
    const was = prevRecordingRef.current;
    prevRecordingRef.current = recording;
    if (!was || recording) return;
    const timer = window.setTimeout(() => {
      void (async () => {
        const el = inputRef.current;
        if (!el) return;
        const current = el.value;
        const baseLen = asrBaseLenRef.current;
        if (baseLen > current.length) return;
        const base = current.slice(0, baseLen);
        const asrPart = current.slice(baseLen).trim();
        if (asrPart.length < 2) return;
        try {
          const polished = await invoke<string>('polish_asr_text', { text: asrPart });
          const trimmed = (polished ?? '').trim();
          if (!trimmed || inputRef.current?.value !== current) return;
          setInput(base.trim() ? `${base.replace(/\s+$/, '')} ${trimmed}` : trimmed);
        } catch {
          // 润色失败保留原文
        }
      })();
    }, 300);
    return () => window.clearTimeout(timer);
  }, [recording]);

  // 语音消息录制期间监听 ASR 事件，把识别结果累积到 voiceAsrTextRef（不写入输入框）
  // 与 recording 模式互斥：同一时刻只会有一处订阅 asr:event
  const voiceAsrPartialRef = useRef('');
  useEffect(() => {
    if (!voiceRecording) { voiceAsrPartialRef.current = ''; return; }
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    (async () => {
      unlisten = await listen<{
        type: string;
        text?: string;
        confidence?: number;
        message?: string;
      }>('asr:event', (e) => {
        const { type, text } = e.payload;
        if (type === 'final_result' && text) {
          // 移除已累积的 partial 尾部，再追加 final（与 recording 模式同样的去重逻辑）
          const base = voiceAsrTextRef.current.slice(0, voiceAsrTextRef.current.length - voiceAsrPartialRef.current.length);
          voiceAsrPartialRef.current = '';
          const separator = base === '' || base.endsWith(' ') ? '' : ' ';
          voiceAsrTextRef.current = base + separator + text;
        } else if (type === 'partial_result' && text) {
          const base = voiceAsrTextRef.current.slice(0, voiceAsrTextRef.current.length - voiceAsrPartialRef.current.length);
          voiceAsrPartialRef.current = text;
          voiceAsrTextRef.current = base + text;
        } else if (type === 'error') {
          console.warn('[voice] ASR 错误:', e.payload.message);
        }
        // 注意：不处理 stopped 事件——voiceRecording 的停止由 handleVoiceStop 控制
      });
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      voiceAsrPartialRef.current = '';
    };
  }, [voiceRecording]);

  // 录音期间 ASR 写入文本后，光标和滚动位置跟随到末尾
  useLayoutEffect(() => {
    if (!recording) return;
    const el = inputRef.current;
    if (!el) return;
    const len = el.value.length;
    el.setSelectionRange(len, len);
    el.scrollTop = el.scrollHeight;
    el.scrollLeft = el.scrollWidth;
  }, [input, recording]);

  /** 选中 @ 候选角色：把输入框中的 `@query` 替换为 `@角色名 ` */
  const selectMention = useCallback((charId: string) => {
    const char = characters.find((c) => c.id === charId);
    if (!char) return;
    const ms = mentionStateRef.current;
    if (ms.startIndex < 0) return;
    const before = input.slice(0, ms.startIndex);
    const after = input.slice(ms.startIndex + 1 + ms.query.length);
    const insert = `@${char.name} `;
    const newValue = before + insert + after;
    setInput(newValue);
    setMentionState({ active: false, query: '', startIndex: -1, selectedIndex: 0 });
    const newPos = before.length + insert.length;
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(newPos, newPos);
    });
  }, [input, characters]);

  /** 检测输入框光标位置是否处于 @ 触发状态，更新 mentionState */
  const detectMention = useCallback((value: string, cursorPos: number) => {
    if (view !== 'group') {
      if (mentionStateRef.current.active) {
        setMentionState({ active: false, query: '', startIndex: -1, selectedIndex: 0 });
      }
      return;
    }
    const beforeCursor = value.slice(0, cursorPos);
    const match = beforeCursor.match(/(?:^|\s)@([\w]*)$/);
    if (match) {
      const atStart = (match.index ?? 0) + (match[0].startsWith(' ') ? 1 : 0);
      const query = match[1];
      setMentionState((prev) => ({
        active: true,
        query,
        startIndex: atStart,
        selectedIndex: prev.query === query ? prev.selectedIndex : 0,
      }));
    } else if (mentionStateRef.current.active) {
      setMentionState({ active: false, query: '', startIndex: -1, selectedIndex: 0 });
    }
  }, [view]);

  const handleInputChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.target.value;
    setInput(value);
    detectMention(value, e.target.selectionStart ?? value.length);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // @ 提及菜单激活时优先处理导航/选择
    const ms = mentionStateRef.current;
    if (ms.active && mentionList.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setMentionState((prev) => ({
          ...prev,
          selectedIndex: (prev.selectedIndex + 1) % mentionList.length,
        }));
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setMentionState((prev) => ({
          ...prev,
          selectedIndex: (prev.selectedIndex - 1 + mentionList.length) % mentionList.length,
        }));
        return;
      }
      if (e.key === 'Enter' && !e.ctrlKey && !e.shiftKey && !e.metaKey) {
        e.preventDefault();
        const target = mentionList[ms.selectedIndex] ?? mentionList[0];
        if (target) selectMention(target.id);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setMentionState({ active: false, query: '', startIndex: -1, selectedIndex: 0 });
        return;
      }
    }
    // Enter 发送，Ctrl+Enter 换行
    if (e.key === 'Enter' && !e.ctrlKey && !e.shiftKey && !e.metaKey) {
      e.preventDefault();
      void handleSend();
    }
    // Ctrl+Enter 或 Shift+Enter 换行：textarea 默认行为即可插入换行，无需拦截
  };

  const closeWindow = useCallback(async () => {
    if (isClosing) return;
    setIsClosing(true);
    const el = rootRef.current;
    if (el) {
      el.animate(
        [
          { opacity: 1, transform: 'translateY(0) scale(1)' },
          { opacity: 0, transform: 'translateY(80px) scale(0.78)' },
        ],
        { duration: 400, easing: 'cubic-bezier(0.65, 0, 1, 1)', fill: 'forwards' },
      );
    }
    // 微信窗口为右缘抽屉：点击退出 → 动画收回屏幕右侧并隐藏（保留窗口复用）
    void invoke('collapse_side_chat', { label: 'chat' }).catch(() => {});
  }, [isClosing]);

  useEffect(() => {
    let cancelled = false;
    let started = false;
    const startEnter = () => {
      if (cancelled || started) return;
      started = true;
      playEnter();
    };
    window.addEventListener('window-shown', startEnter, { once: true });
    const timer = setTimeout(startEnter, 500);
    return () => {
      cancelled = true;
      window.removeEventListener('window-shown', startEnter);
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 播放进入动画（初次显示与退出后再次显示复用）：取消旧动画并重置根节点样式后淡入 */
  const playEnter = useCallback(() => {
    const el = rootRef.current;
    if (!el) return;
    try { el.getAnimations().forEach((a) => a.cancel()); } catch { /* ignore */ }
    el.style.opacity = '';
    el.style.transform = '';
    el.animate(
      [
        { opacity: 0, transform: 'translateY(40px) scale(0.85)' },
        { opacity: 1, transform: 'translateY(-6px) scale(1.02)', offset: 0.5 },
        { opacity: 1, transform: 'translateY(2px) scale(0.995)', offset: 0.7 },
        { opacity: 1, transform: 'translateY(0) scale(1)' },
      ],
      { duration: 600, easing: 'cubic-bezier(0.34, 1.56, 0.64, 1)', fill: 'forwards' },
    );
  }, []);

  /** 切换底部面板（emoji/media），点击同一按钮则收起 */
  const togglePanel = useCallback((panel: 'emoji' | 'media') => {
    setBottomPanel((prev) => prev === panel ? 'none' : panel);
  }, []);

  /** 点击 emoji：插入到输入框并记录到最近使用 */
  const handleEmojiClick = useCallback((emoji: string) => {
    setInput((prev) => prev + emoji);
    inputRef.current?.focus();
    setRecentEmojis((prev) => {
      const filtered = prev.filter((e) => e !== emoji);
      return [emoji, ...filtered].slice(0, 16);
    });
  }, []);

  /** 清理语音录制资源（MediaRecorder / 计时器 / ASR），供 handleVoiceStop 和异常路径复用 */
  const cleanupVoiceRecording = useCallback(() => {
    // 手动重置 ref，避免依赖异步 useEffect 同步导致竞态（handleVoiceStart await 期间被取消时 ref 仍为 true）
    voiceRecordingRef.current = false;
    if (voiceTimerRef.current !== null) {
      window.clearInterval(voiceTimerRef.current);
      voiceTimerRef.current = null;
    }
    const mr = mediaRecorderRef.current;
    if (mr && mr.state !== 'inactive') {
      try { mr.stop(); } catch { /* ignore */ }
    }
    mediaRecorderRef.current = null;
    voiceChunksRef.current = [];
    voiceAsrTextRef.current = '';
    voiceAsrPartialRef.current = '';
    voicePointerIdRef.current = null;
    setVoiceRecording(false);
    setVoiceDuration(0);
    // 停止 ASR（幂等，未启动时后端直接返回 Ok）
    void invoke('stop_recognition').catch(() => { /* ignore */ });
  }, []);

  // handleVoiceStopRef 在 handleVoiceStart 之前声明，供 60s 超时回调读取最新值
  // 初始值为 no-op，handleVoiceStop 定义后通过 useEffect 更新
  const handleVoiceStopRef = useRef<() => void>(() => {});

  /**
   * 按下语音按钮：启动 MediaRecorder + ASR
   * - 同时打开麦克风（MediaRecorder）和后端流式 ASR（start_recognition）
   * - ASR final 文本累积在 voiceAsrTextRef，松开后发送给 LLM
   * - 原始音频保存为文件，显示为微信风格语音气泡
   */
  const handleVoiceStart = useCallback(async () => {
    // home 视图不发送
    if (view === 'home') return;
    // 已在录音或 ASR 转文字模式中，不重复启动
    if (voiceRecordingRef.current || recordingRef.current) return;
    // 立即标记为录制中，防止 async 等待期间重复触发
    voiceRecordingRef.current = true;

    voiceAsrTextRef.current = '';
    voiceAsrPartialRef.current = '';
    voiceChunksRef.current = [];
    voiceStartTimeRef.current = Date.now();
    setVoiceDuration(0);

    // 读取 ASR 引擎类型：仅 WinRT 能与 MediaRecorder 共享麦克风；其他引擎（Azure/Aliyun/Whisper/OpenAI）
    // 用 cpal 独占麦克风，需走"先录音再文件转写"路径避免麦克风冲突
    let asrEngine = 'winrt';
    try {
      asrEngine = (await invoke<string>('get_config', { key: 'speech_recognition.engine' })) || 'winrt';
    } catch { /* 默认 winrt */ }
    const useRealtimeAsr = asrEngine === 'winrt';
    voiceRealtimeAsrRef.current = useRealtimeAsr;

    // setVoiceRecording(true) 先触发 useEffect 注册 asr:event 监听器，确保事件不丢失
    setVoiceRecording(true);
    let asrStarted = false;
    if (useRealtimeAsr) {
      // WinRT：先启动后端 ASR（必须在 getUserMedia 之前：前端开麦克风会占用音频会话，
      //   导致 WinRT SpeechRecognizer 创建失败 [0x800455A0]。WinRT 先开则两者可共享麦克风）
      try {
        await invoke('start_recognition', { characterId: privateCharId ?? undefined });
        asrStarted = true;
      } catch (err) {
        // ASR 启动失败不影响音频录制，语音气泡仍可发送（只是没有转写文本）
        console.warn('[voice] ASR 启动失败:', err);
        void emit('toast:show', {
          message: t('chat.voice_asr_failed', { defaultValue: '语音识别启动失败，仅保存语音消息' }),
          type: 'warning', duration: 4000, key: Date.now(),
        });
      }
    } else {
      // 非 WinRT：停 TTS（避免扬声器声音被录进麦克风），不启动实时 ASR，转写改在录音结束后进行
      try {
        await invoke('stop_speaking', { characterId: privateCharId ?? undefined });
      } catch { /* ignore */ }
    }

    // 2. 启动 MediaRecorder（捕获原始音频用于语音气泡播放，与 WinRT 共享麦克风）
    let stream: MediaStream | null = null;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      voiceRecordingRef.current = false;
      setVoiceRecording(false);
      if (asrStarted) { void invoke('stop_recognition').catch(() => {}); }
      const errName = (err as DOMException)?.name ?? '';
      const msg = errName === 'NotAllowedError' || errName === 'SecurityError'
        ? t('chat.voice_mic_denied', { defaultValue: '无法访问麦克风，请检查系统权限设置' })
        : t('chat.voice_mic_failed', { error: String(err), defaultValue: '麦克风启动失败：{{error}}' });
      void emit('toast:show', { message: msg, type: 'warning', duration: 5000, key: Date.now() });
      return;
    }

    // 选择浏览器支持的 MIME（webm 优先，兼容 ogg/mp4）
    const candidates = ['audio/webm;codecs=opus', 'audio/webm', 'audio/ogg;codecs=opus', 'audio/ogg', 'audio/mp4'];
    const mime = candidates.find((m) => {
      try { return MediaRecorder.isTypeSupported(m); } catch { return false; }
    }) ?? '';
    voiceMimeRef.current = mime || 'audio/webm';

    let recorder: MediaRecorder;
    try {
      recorder = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    } catch (err) {
      voiceRecordingRef.current = false;
      setVoiceRecording(false);
      if (asrStarted) { void invoke('stop_recognition').catch(() => {}); }
      stream.getTracks().forEach((tr) => tr.stop());
      void emit('toast:show', { message: t('chat.voice_mic_failed', { error: String(err), defaultValue: '麦克风启动失败：{{error}}' }), type: 'warning', duration: 5000, key: Date.now() });
      return;
    }
    recorder.ondataavailable = (ev) => {
      if (ev.data && ev.data.size > 0) voiceChunksRef.current.push(ev.data);
    };
    recorder.start(100); // 每 100ms 采集一个 chunk
    // 检查是否在 async 等待期间被取消（用户快速双击）
    if (!voiceRecordingRef.current) {
      recorder.stream.getTracks().forEach((tr) => tr.stop());
      try { recorder.stop(); } catch { /* ignore */ }
      if (asrStarted) { void invoke('stop_recognition').catch(() => {}); }
      return;
    }
    mediaRecorderRef.current = recorder;

    // 3. 启动计时器
    voiceTimerRef.current = window.setInterval(() => {
      const elapsed = (Date.now() - voiceStartTimeRef.current) / 1000;
      setVoiceDuration(elapsed);
      // 最长 60 秒自动停止
      if (elapsed >= 60) {
        handleVoiceStopRef.current();
      }
    }, 100);
  }, [view, privateCharId, t]);

  /**
   * 松开语音按钮：停止录制，保存音频，发送语音气泡 + ASR 转写文本给 LLM
   */
  const handleVoiceStop = useCallback(() => {
    if (!voiceRecordingRef.current) return;

    // 清理计时器
    if (voiceTimerRef.current !== null) {
      window.clearInterval(voiceTimerRef.current);
      voiceTimerRef.current = null;
    }

    const durationSec = (Date.now() - voiceStartTimeRef.current) / 1000;
    const elapsedMs = Date.now() - voiceStartTimeRef.current;

    const recorder = mediaRecorderRef.current;
    const chunks = voiceChunksRef.current;
    const mime = voiceMimeRef.current;
    const targetCharId = privateCharId;
    const targetView = view;

    // 太短（< 500ms）视为误触，丢弃
    if (elapsedMs < 500) {
      cleanupVoiceRecording();
      return;
    }

    // 停止 ASR（仅实时模式启动了 ASR；文件转写模式在录音结束后才调 transcribe_audio）
    const useRealtimeAsr = voiceRealtimeAsrRef.current;
    if (useRealtimeAsr) {
      void invoke('stop_recognition').catch(() => { /* ignore */ });
    }

    // 等待 ASR final 文本到达（stop_recognition 后后端会产生 final_result 事件）
    const waitForAsr = useRealtimeAsr
      ? new Promise<void>((resolve) => {
          // 给后端 600ms 把 final_result 事件送回来（IPC + WinRT StopAsync + 事件转发）
          setTimeout(resolve, 600);
        })
      : Promise.resolve();

    if (!recorder || recorder.state === 'inactive') {
      cleanupVoiceRecording();
      return;
    }

    recorder.onstop = () => {
      // 停止所有音频轨道，释放麦克风
      try {
        recorder.stream.getTracks().forEach((tr) => tr.stop());
      } catch { /* ignore */ }

      const blob = new Blob(chunks, { type: mime || 'audio/webm' });
      voiceChunksRef.current = [];

      if (blob.size === 0) {
        cleanupVoiceRecording();
        return;
      }

      const reader = new FileReader();
      reader.onloadend = async () => {
        const dataUrl = reader.result as string;
        // 提取纯 base64（去掉 data:<mime>;base64, 前缀）
        const match = dataUrl.match(/^data:[^;]+;base64,(.+)$/);
        if (!match) {
          cleanupVoiceRecording();
          return;
        }
        const base64Data = match[1];
        const finalMime = mime || 'audio/webm';

        // 保存音频文件到用户数据目录
        let audioPath: string | undefined;
        try {
          audioPath = await invoke<string>('save_voice_audio', { base64Data, mime: finalMime });
        } catch (err) {
          console.warn('[voice] 保存音频失败:', err);
        }

        // 获取转写文本：实时模式等待 ASR 事件累积；文件转写模式解码音频后调 transcribe_audio
        let transcribedText = '';
        if (useRealtimeAsr) {
          await waitForAsr;
          transcribedText = voiceAsrTextRef.current.trim();
        } else {
          try {
            const samplesB64 = await audioBlobToBase64F32(blob);
            transcribedText = ((await invoke<string>('transcribe_audio', { samplesB64 })) || '').trim();
          } catch (err) {
            console.warn('[voice] 文件转写失败:', err);
            void emit('toast:show', {
              message: t('chat.voice_asr_failed', { defaultValue: '语音识别启动失败，仅保存语音消息' }),
              type: 'warning', duration: 4000, key: Date.now(),
            });
          }
        }

        // 清理录制状态
        voiceAsrTextRef.current = '';
        voiceAsrPartialRef.current = '';
        mediaRecorderRef.current = null;
        voicePointerIdRef.current = null;
        setVoiceRecording(false);
        setVoiceDuration(0);

        // 构造语音消息 metadata（持久化到对话历史）
        const voiceMeta = {
          kind: 'voice',
          audio_path: audioPath,
          duration: Math.round(durationSec * 10) / 10,
        };
        // ASR 无转写文本时仍显示语音气泡，仅不发送给 LLM
        const messageText = transcribedText;

        if (targetView === 'group') {
          // 群聊：本地添加语音气泡，然后群发给在线角色
          const userTs = Date.now();
          const voiceMsg: ChatMessage = {
            id: nextId(),
            role: 'user',
            content: '',
            timestamp: userTs,
            voice: { audioPath, audioDataUrl: dataUrl, duration: durationSec },
          };
          setGroupMessages((prev) => [...prev, voiceMsg]);
          setLastPreviews((prev) => ({
            ...prev,
            group: { content: messageText || '[语音]', timestamp: userTs, role: 'user' },
          }));

          if (!messageText) {
            void emit('toast:show', {
              message: t('chat.voice_empty_transcript', { defaultValue: '未识别到语音内容，仅保存语音消息' }),
              type: 'warning', duration: 3000, key: Date.now(),
            });
            return;
          }
          const onlineChars = characters.filter((c) => c.online);
          for (const c of onlineChars) {
            const sid = (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function')
              ? crypto.randomUUID()
              : `s-${Date.now()}-${Math.random().toString(36).slice(2)}`;
            groupStreamCharMapRef.current.set(sid, c.id);
            groupStreamBuffersRef.current.set(sid, '');
            void invoke('send_message_stream', {
              message: messageText,
              streamId: sid,
              characterId: c.id,
              channel: 'wechat_group',
              whisper: false,
              fileMetadata: voiceMeta,
            }).catch((err) => {
              console.warn(`[语音群发] 角色 ${c.id} 发送失败:`, err);
              groupStreamCharMapRef.current.delete(sid);
              groupStreamBuffersRef.current.delete(sid);
            });
          }
          if (onlineChars.length > 0) setGroupStreaming(true);
          return;
        }

        if (targetView === 'private' && targetCharId) {
          // 私聊：本地添加语音气泡，然后发送转写文本给 LLM
          const userTs = Date.now();
          const voiceMsg: ChatMessage = {
            id: nextId(),
            role: 'user',
            content: '',
            timestamp: userTs,
            voice: { audioPath, audioDataUrl: dataUrl, duration: durationSec },
          };
          setMessages((prev) => [...prev, voiceMsg]);
          setLastPreviews((prev) => ({
            ...prev,
            [targetCharId]: { content: messageText || '[语音]', timestamp: userTs, role: 'user' },
          }));

          if (!messageText) {
            void emit('toast:show', {
              message: t('chat.voice_empty_transcript', { defaultValue: '未识别到语音内容，仅保存语音消息' }),
              type: 'warning', duration: 3000, key: Date.now(),
            });
            return;
          }

          // 忙碌状态下暂存
          if (presenceStates[targetCharId] === 'busy') {
            pendingMessagesRef.current.push({ charId: targetCharId, text: messageText });
            return;
          }
          // 标记跳过 chat:user_message 的文本气泡（本地已显示语音气泡）
          skipNextUserMessageRef.current = true;
          // 安全超时：2 秒后未匹配到 chat:user_message 则清除标记，避免卡住后续消息
          setTimeout(() => { skipNextUserMessageRef.current = false; }, 2000);
          void ChatController.sendMessage(messageText, targetCharId, 'wechat', undefined, voiceMeta);
        }
      };
      reader.readAsDataURL(blob);
    };

    try {
      recorder.stop();
    } catch {
      cleanupVoiceRecording();
    }
  }, [view, privateCharId, characters, presenceStates, t, cleanupVoiceRecording]);

  // 用 ref 保存 handleVoiceStop 最新值，供 handleVoiceStart 中的 60s 超时回调读取
  // handleVoiceStopRef 已在 handleVoiceStart 之前声明（初始为 no-op），这里只更新值
  useEffect(() => { handleVoiceStopRef.current = handleVoiceStop; }, [handleVoiceStop]);

  // 组件卸载时清理语音录制资源
  useEffect(() => {
    return () => {
      if (voiceRecordingRef.current) {
        if (voiceTimerRef.current !== null) {
          window.clearInterval(voiceTimerRef.current);
          voiceTimerRef.current = null;
        }
        const mr = mediaRecorderRef.current;
        if (mr && mr.state !== 'inactive') {
          try { mr.stop(); } catch { /* ignore */ }
        }
        if (mr) {
          try { mr.stream.getTracks().forEach((tr) => tr.stop()); } catch { /* ignore */ }
        }
        void invoke('stop_recognition').catch(() => { /* ignore */ });
      }
    };
  }, []);

  /** 选择本地图片进入草稿（多选，预览后可随文本一起发送） */
  const handleSendImage = useCallback(async () => {
    try {
      const selected = await openDialog({
        multiple: true,
        filters: [{ name: t('chat.image_filter'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'] }],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      if (paths.length === 0) return;
      setBottomPanel('none');
      // 路径 → File → 草稿（经 asset 协议读取 blob）
      const files: File[] = [];
      for (const p of paths) {
        try {
          const resp = await fetch(convertFileSrc(p)).catch(() => null);
          if (!resp?.ok) continue;
          const blob = await resp.blob();
          files.push(new File([blob], p.split(/[\\/]/).pop() || 'image', { type: blob.type || 'image/png' }));
        } catch { /* 跳过不可读 */ }
      }
      if (files.length === 0) {
        void emit('toast:show', { message: t('chat.image_send_failed'), type: 'warning', duration: 3000, key: Date.now() });
        return;
      }
      addDraftImages(files);
    } catch (e) {
      void emit('toast:show', { message: String(e), type: 'error', duration: 3000, key: Date.now() });
    }
  }, [t, addDraftImages]);

  // ── 文件拖放：通过 Tauri 原生 onDragDropEvent 获取文件路径 ──
  const extractFileText = useExtractFileText();
  const [isDragOver, setIsDragOver] = useState(false);

  // Drop 逻辑用 ref 保存最新闭包，避免 onDragDropEvent 监听器持有过时状态
  const handleFileDropRef = useRef<(paths: string[]) => void>(() => {});
  handleFileDropRef.current = (paths: string[]) => {
    if (paths.length === 0) return;

    if (view === 'home') {
      void emit('toast:show', {
        message: t('chat.drag_file_home_hint', { defaultValue: '请先进入对话再发送文件' }),
        type: 'info', duration: 3000, key: Date.now(),
      });
      return;
    }

    const targetCharIds = view === 'private' && privateCharId
      ? [privateCharId]
      : view === 'group'
        ? characters.filter((c) => c.online).map((c) => c.id)
        : [];
    if (targetCharIds.length === 0) return;

    const channel = view === 'group' ? 'wechat_group' : 'wechat';

    void (async () => {
      for (const filePath of paths) {
        try {
          const result: FileTextResult = await extractFileText(filePath);

          if (result.file_type === 'image') {
            // 图片进入草稿（预览后可随文本一起发送），不再直接发送
            try {
              const blob = await (await fetch(convertFileSrc(filePath))).blob();
              addDraftImages([new File([blob], result.filename || 'image', { type: blob.type || 'image/png' })]);
            } catch {
              void emit('toast:show', {
                message: t('toast.file_extract_failed', { error: 'read image', defaultValue: '读取图片失败' }),
                type: 'error', duration: 4000, key: Date.now(),
              });
            }
          } else if (result.file_type === 'unsupported') {
            void emit('toast:show', {
              message: t('toast.file_unsupported', {
                filename: result.filename,
                defaultValue: '不支持的文件类型：{{filename}}',
              }),
              type: 'warning', duration: 4000, key: Date.now(),
            });
          } else {
            const truncatedHint = result.truncated
              ? t('toast.file_truncated', {
                  count: result.original_char_count,
                  defaultValue: `（文件过长，已截断，原始 ${result.original_char_count} 字符）`,
                })
              : '';
            const message = `[文件：${result.filename}]\n${result.text}${truncatedHint}`;
            const fileMetadata = {
              kind: 'file',
              file_name: result.filename,
              file_type: result.file_type,
              truncated: result.truncated,
              original_char_count: result.original_char_count,
            };
            for (const cid of targetCharIds) {
              void ChatController.sendMessage(message, cid, channel, undefined, fileMetadata);
            }
          }
        } catch (err) {
          void emit('toast:show', {
            message: t('toast.file_extract_failed', {
              error: String(err),
              defaultValue: '文件处理失败：{{error}}',
            }),
            type: 'error', duration: 5000, key: Date.now(),
          });
        }
      }
    })();
  };

  // 注册原生拖放事件监听（仅一次）
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      unlisten = await getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === 'enter') {
          setIsDragOver(true);
        } else if (payload.type === 'leave') {
          setIsDragOver(false);
        } else if (payload.type === 'drop') {
          setIsDragOver(false);
          handleFileDropRef.current(payload.paths);
        }
      });
    })();
    return () => { unlisten?.(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 发送本地文件：弹出文件选择对话框，提取文本后发送给当前对话角色 */
  const handleSendFile = useCallback(async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [
          { name: t('chat.file_filter'), extensions: [
            'txt', 'md', 'markdown', 'log', 'csv', 'tsv', 'rtf', 'pdf',
            'json', 'yaml', 'yml', 'xml', 'toml', 'ini', 'conf', 'cfg', 'properties',
            'rs', 'py', 'js', 'ts', 'tsx', 'jsx', 'mjs', 'cjs',
            'go', 'java', 'c', 'cpp', 'cc', 'cxx', 'h', 'hpp', 'cs', 'rb', 'php',
            'swift', 'kt', 'sh', 'bash', 'zsh', 'ps1', 'bat', 'cmd',
            'sql', 'r', 'lua', 'pl', 'dart', 'html', 'htm', 'css', 'scss', 'less', 'svg',
          ] },
          { name: t('chat.image_filter'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'] },
        ],
      });
      if (!selected || Array.isArray(selected)) return;
      setBottomPanel('none');

      const filePath = selected as string;
      const charId = privateCharId;
      if (!charId) return;

      const result: FileTextResult = await extractFileText(filePath);

      if (result.file_type === 'image') {
        // 用户在文件选择器中选了图片：走多模态图片发送流程
        await invoke('send_image_message', { sourcePath: filePath, characterId: charId, channel: 'wechat' });
      } else if (result.file_type === 'unsupported') {
        void emit('toast:show', {
          message: t('toast.file_unsupported', { filename: result.filename, defaultValue: '不支持的文件类型：{{filename}}' }),
          type: 'warning', duration: 4000, key: Date.now(),
        });
      } else {
        const truncatedHint = result.truncated
          ? t('toast.file_truncated', { count: result.original_char_count, defaultValue: `（文件过长，已截断，原始 ${result.original_char_count} 字符）` })
          : '';
        const message = `[文件：${result.filename}]\n${result.text}${truncatedHint}`;
        const fileMetadata = {
          kind: 'file',
          file_name: result.filename,
          file_type: result.file_type,
          truncated: result.truncated,
          original_char_count: result.original_char_count,
        };
        void ChatController.sendMessage(message, charId, 'wechat', undefined, fileMetadata);
      }
    } catch (e) {
      const errMsg = String(e);
      void emit('toast:show', {
        message: t('toast.file_extract_failed', { error: errMsg, defaultValue: '文件处理失败：{{error}}' }),
        type: 'error', duration: 5000, key: Date.now(),
      });
    }
  }, [t, privateCharId, extractFileText]);

  // ── 摄像头拍摄：打开拍摄模态，拍照后保存为临时文件并走 send_image_message ──
  const [cameraOpen, setCameraOpen] = useState(false);

  const handleCapturePhoto = useCallback(() => {
    setBottomPanel('none');
    setCameraOpen(true);
  }, []);

  /** 拍照完成回调：base64 → save_temp_image → send_image_message */
  const handlePhotoCaptured = useCallback(async (base64Data: string, mime: string) => {
    const charId = privateCharId;
    if (!charId) return;
    try {
      const tempPath = await invoke<string>('save_temp_image', { base64Data, mime });
      await invoke('send_image_message', { sourcePath: tempPath, characterId: charId, channel: 'wechat' });
    } catch (e) {
      const errMsg = String(e);
      void emit('toast:show', {
        message: t('chat.capture_save_failed', { error: errMsg, defaultValue: '照片保存失败：{{error}}' }),
        type: 'error', duration: 5000, key: Date.now(),
      });
    }
  }, [privateCharId, t]);

  const mdStyles = useMemo(() => (
    <style>{`
      .vivian-md .md-code-block { background: var(--wx-code-bg); border-radius: 10px; padding: 10px 12px; overflow-x: auto; margin: 6px 0; font-family: 'SF Mono', 'Consolas', 'Monaco', monospace; font-size: 13px; line-height: 1.5; border: 1px solid var(--wx-border); }
      .vivian-md .md-code-block code { background: transparent; padding: 0; color: var(--wx-text); }
      .vivian-md .md-code-inline { background: var(--wx-bg-active); padding: 1px 5px; border-radius: 4px; font-family: 'SF Mono', 'Consolas', 'Monaco', monospace; font-size: 13px; color: var(--wx-code-inline); }
      .vivian-md strong { font-weight: 700; }
      .vivian-md em { font-style: italic; }
      .vivian-scroll::-webkit-scrollbar { width: 0; display: none; }
      .vivian-scroll { scrollbar-width: none; -ms-overflow-style: none; }
      .vivian-chat-input::placeholder { color: var(--wx-placeholder); }
      @keyframes vivian-blink { 0%, 50% { opacity: 1; } 50.01%, 100% { opacity: 0; } }
      @keyframes vivian-slide-up {
        0% { opacity: 0; transform: translateY(40px) scale(0.85) rotateX(15deg) rotateY(-8deg); }
        25% { opacity: 1; }
        50% { transform: translateY(-6px) scale(1.02) rotateX(-2deg) rotateY(2deg); }
        70% { transform: translateY(2px) scale(0.995) rotateX(1deg) rotateY(-0.5deg); }
        100% { opacity: 1; transform: translateY(0) scale(1) rotateX(0) rotateY(0); }
      }
      @keyframes vivian-slide-down {
        0% { opacity: 1; transform: translateY(0) scale(1) rotateX(0) rotateY(0); }
        20% { transform: translateY(4px) scale(1.005) rotateX(0.5deg) rotateY(0); }
        50% { opacity: 1; transform: translateY(20px) scale(0.94) rotateX(-3deg) rotateY(2deg); }
        100% { opacity: 0; transform: translateY(80px) scale(0.78) rotateX(20deg) rotateY(-8deg); }
      }
      .vivian-enter-animation { animation: vivian-slide-up 0.6s cubic-bezier(0.34, 1.56, 0.64, 1) forwards; transform-origin: 50% 50%; perspective: 1200px; }
      .vivian-exit-animation { animation: vivian-slide-down 0.4s cubic-bezier(0.65, 0, 1, 1) forwards; transform-origin: 50% 50%; perspective: 1200px; }
    `}</style>
  ), []);

  // 群聊消息渲染项（用户消息 + AI 消息，按时间排序）
  const groupItems = useMemo<RenderItem[]>(() => {
    const result: RenderItem[] = [];
    let prevTs: number | null = null;
    for (const m of groupMessages) {
      if (prevTs === null || m.timestamp - prevTs > TIME_GAP_MS) {
        result.push({ kind: 'time', key: `t-${m.id}`, text: formatSeparatorTime(m.timestamp, t) });
      }
      result.push({ kind: 'msg', key: m.id, msg: m });
      prevTs = m.timestamp;
    }
    return result;
  }, [groupMessages, t]);

  // 群聊视图独立的滚动容器 ref + virtualizer + 自动滚动跟踪
  const groupListRef = useRef<HTMLDivElement>(null);
  const groupIsAtBottomRef = useRef(true);
  const groupScrollRafRef = useRef<number | null>(null);
  const groupVirtualizer = useVirtualizer({
    count: groupItems.length,
    getScrollElement: () => groupListRef.current,
    estimateSize: () => 80,
    overscan: 5,
    getItemKey: (i) => groupItems[i].key,
  });

  const groupScrollToBottom = useCallback(() => {
    if (groupScrollRafRef.current !== null) return;
    groupScrollRafRef.current = window.requestAnimationFrame(() => {
      groupScrollRafRef.current = null;
      const el = groupListRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }, []);

  // 群聊消息变化时自动滚动到底部（仅当用户已处于底部）
  useEffect(() => {
    if (groupIsAtBottomRef.current) groupScrollToBottom();
  }, [groupMessages, groupScrollToBottom]);

  // 清理群聊滚动 rAF
  useEffect(() => {
    return () => {
      if (groupScrollRafRef.current !== null) {
        window.cancelAnimationFrame(groupScrollRafRef.current);
        groupScrollRafRef.current = null;
      }
    };
  }, []);

  const handleGroupScroll = useCallback(() => {
    const el = groupListRef.current;
    if (!el) return;
    groupIsAtBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }, []);

  // 当前私聊角色名称
  const privateCharName = useMemo(() => {
    if (!privateCharId) return '';
    return characters.find((c) => c.id === privateCharId)?.name ?? privateCharId;
  }, [privateCharId, characters]);

  /** details 视图：搜索当前私聊角色的聊天历史 */
  const handleDetailsSearch = useCallback(async (query: string) => {
    const q = query.trim();
    setDetailsSearchQuery(query);
    if (!q || !privateCharId) { setDetailsSearchResults([]); return; }
    setDetailsSearching(true);
    try {
      const entries = await invoke<HistoryEntry[]>('get_chat_history', { characterId: privateCharId });
      const lower = q.toLowerCase();
      const results = entries.filter((e) => {
        if (e.role === 'system') return false;
        const ch = e.metadata?.channel;
        if (ch !== 'wechat' && ch !== undefined) return false;
        return e.content.toLowerCase().includes(lower);
      }).map((e) => ({
        id: e.id,
        content: stripActions(e.content),
        role: e.role,
        timestamp: normalizeTimestamp(e.timestamp),
        character_id: privateCharId,
      }));
      setDetailsSearchResults(results);
    } catch (e) {
      console.error('搜索聊天记录失败:', e);
      setDetailsSearchResults([]);
    } finally {
      setDetailsSearching(false);
    }
  }, [privateCharId]);

  /** details 视图：保存备注名 */
  const handleSaveRemark = useCallback(() => {
    if (!privateCharId) return;
    const trimmed = remarkInput.trim();
    setCharRemarks((prev) => {
      const next = { ...prev };
      if (trimmed) next[privateCharId] = trimmed;
      else delete next[privateCharId];
      return next;
    });
    setEditingRemark(false);
  }, [privateCharId, remarkInput]);

  /** details 视图：选择并设置聊天背景图 */
  const handleSetBackground = useCallback(async () => {
    if (!privateCharId) return;
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: t('chat.avatar_image_filter'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'] }],
      });
      if (!selected || Array.isArray(selected)) return;
      setChatBackgrounds((prev) => ({ ...prev, [privateCharId]: selected }));
    } catch (e) {
      console.warn('设置聊天背景失败:', e);
    }
  }, [privateCharId, t]);

  /** details 视图：清除聊天背景 */
  const handleClearBackground = useCallback(() => {
    if (!privateCharId) return;
    setChatBackgrounds((prev) => {
      const next = { ...prev };
      delete next[privateCharId];
      return next;
    });
  }, [privateCharId]);

  /** 在场状态 → 显示文本与颜色（在线绿 / 忙碌红 / 休息黄 / 离线灰） */
  const presenceDisplay = useCallback((charId: string): { label: string; color: string } => {
    const state = presenceStates[charId] ?? 'online';
    switch (state) {
      case 'busy': return { label: t('chat.status_busy'), color: '#FF453A' };
      case 'rest': return { label: t('chat.status_rest'), color: '#FFCC00' };
      case 'offline': return { label: t('chat.status_offline'), color: '#8C8C8C' };
      default: return { label: t('chat.status_online'), color: '#34C759' };
    }
  }, [presenceStates, t]);

  /**
   * 主页列表项的会话预览文本（类似微信）。
   * - 图片消息 → `[图片]`
   * - 表情包 → `[表情]`
   * - 群聊 AI 消息 → `角色名: <文本>`；私聊/用户消息直接显示文本
   * - 无消息 → `暂无消息`
   * 文本超长截断到 30 字符。
   */
  const formatPreview = useCallback((key: string): string => {
    const p = lastPreviews[key];
    if (!p) return t('chat.preview_empty');
    let body: string;
    if (p.imagePath) {
      body = t('chat.preview_image');
    } else if (p.sticker) {
      body = t('chat.preview_sticker');
    } else {
      const raw = (p.content || '').replace(/\n/g, ' ').trim();
      body = raw.length > 30 ? `${raw.slice(0, 30)}…` : raw;
    }
    if (key === 'group' && p.characterId && p.role !== 'user') {
      const name = characters.find((c) => c.id === p.characterId)?.name ?? p.characterId;
      return `${name}: ${body}`;
    }
    return body || t('chat.preview_empty');
  }, [lastPreviews, characters, t]);

  // 角色名称查找（群聊用）
  const charNameMap = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of characters) m.set(c.id, c.name);
    return m;
  }, [characters]);

  /** 所有会话未读数之和（聊天视图返回按钮旁的灰色气泡） */
  const totalUnread = useMemo(
    () => Object.values(unreadCounts).reduce((a, b) => a + b, 0),
    [unreadCounts],
  );

  const onlineCharacters = useMemo(() => characters.filter((c) => c.online), [characters]);

  /** 聊天记录全局搜索（防抖 300ms） */
  const searchTimerRef = useRef<number | null>(null);
  useEffect(() => {
    if (searchTimerRef.current) window.clearTimeout(searchTimerRef.current);
    const q = searchQuery.trim();
    if (!q) {
      setSearchResults([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    searchTimerRef.current = window.setTimeout(async () => {
      try {
        // 后端完成搜索+去重+排序，只返回最多 50 条匹配结果，避免全量历史 IPC 传输
        const result = await invoke<Array<{
          id: string;
          content: string;
          role: string;
          timestamp: number;
          character_id: string;
          character_name: string;
          source: 'private' | 'group';
        }>>('search_chat_history', { query: q });
        setSearchResults(result);
      } catch (err) {
        console.warn('[ChatWindow] 搜索聊天记录失败:', err);
        setSearchResults([]);
      } finally {
        setSearching(false);
      }
    }, 300);
    return () => {
      if (searchTimerRef.current) window.clearTimeout(searchTimerRef.current);
    };
  }, [searchQuery]);

  return (
    <div ref={rootRef} style={{
      display: 'flex', flexDirection: 'column', height: '100vh',
      background: 'var(--wx-bg)', overflow: 'hidden',
      borderRadius: 44,
      border: '1px solid var(--wx-phone-border)',
      fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
      color: 'var(--wx-text)',
      position: 'relative',
      ...(!isClosing ? { opacity: 0, transform: 'translateY(40px) scale(0.85)' } : {}),
    }}
    >
      {mdStyles}

      {/* ===== 拖拽文件高亮覆盖层 ===== */}
      {isDragOver && (
        <div style={{
          position: 'absolute', inset: 0, zIndex: 9999,
          background: 'rgba(0, 0, 0, 0.35)',
          display: 'flex', alignItems: 'center', justifyContent: 'center',
          borderRadius: 44, pointerEvents: 'none',
        }}>
          <div style={{
            padding: '20px 32px', borderRadius: 16,
            background: 'var(--wx-bg-surface)', color: 'var(--wx-text)',
            fontSize: 16, fontWeight: 500,
            border: '2px dashed var(--wx-border)',
          }}>
            {t('chat.drag_file_hint', { defaultValue: '松开以发送文件' })}
          </div>
        </div>
      )}

      {/* ===== Dynamic Island（始终黑色，不随主题变化） ===== */}
      <div style={{
        position: 'absolute', top: 10, left: '50%', transform: 'translateX(-50%)',
        width: 126, height: 36, borderRadius: 20,
        background: '#000',
        zIndex: 100,
        pointerEvents: 'none',
      }} />

      {/* ===== 第一行：iOS 状态栏 ===== */}
      <div data-tauri-drag-region style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        padding: '0 36px', flexShrink: 0, userSelect: 'none', height: 54,
        background: 'var(--wx-bg)',
      }}>
        <span style={{ fontSize: 14, fontWeight: 600, letterSpacing: 0.2, color: 'var(--wx-text)', fontVariantNumeric: 'tabular-nums' }}>
          {statusTime}
        </span>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, color: 'var(--wx-text)' }}>
          <SignalIcon />
          <WifiIcon />
          <BatteryIcon />
        </div>
      </div>

      {/* ===== 第二行：导航栏 ===== */}
      <div style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        padding: '6px 8px 8px', flexShrink: 0, userSelect: 'none',
        background: 'var(--wx-bg)',
        borderBottom: '0.5px solid var(--wx-border)',
      }}>
        {/* 左：返回按钮（home 关闭窗口，details 返回 private，private/group 返回 home）+ 未读总数气泡 */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          <button
            onClick={() => {
              if (view === 'home') void closeWindow();
              else if (view === 'details') { setView('private'); setDetailsSubView('main'); }
              else setView('home');
            }}
            title={view === 'home' ? t('chat.btn_back') : t('chat.back_to_home')}
            style={navBtn}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
              <path d="M15 19l-7-7 7-7" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
          {view !== 'home' && totalUnread > 0 && (
            <span style={{
              padding: '1px 7px', borderRadius: 9,
              background: 'var(--wx-pill-bg)', color: 'var(--wx-pill-text)',
              fontSize: 11, fontWeight: 600, lineHeight: '16px',
              fontVariantNumeric: 'tabular-nums', userSelect: 'none',
            }}>
              {totalUnread > 99 ? '99+' : totalUnread}
            </span>
          )}
        </div>

        {/* 中间标题 */}
        <div style={{
          position: 'absolute', left: '50%', transform: 'translateX(-50%)',
          display: 'flex', flexDirection: 'column', alignItems: 'center',
        }}>
          <span style={{
            fontSize: 17, fontWeight: 600, letterSpacing: -0.2, color: 'var(--wx-text)',
          }}>
            {view === 'home' ? t('chat.home_title')
              : view === 'group' ? t('chat.group_title')
              : view === 'details' ? t('chat.details_title')
              : (privateTyping ? t('chat.typing') : (charRemarks[privateCharId ?? ''] || privateCharName || t('chat.title')))}
          </span>
        </div>

        {/* 右：三点按钮（仅 private 视图显示，点击进入 details 聊天详情界面） */}
        {view === 'private' && (
          <button
            onClick={() => setView('details')}
            title={t('chat.btn_more')}
            style={navBtn}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            <svg width="22" height="22" viewBox="0 0 24 24">
              <circle cx="6" cy="12" r="1.8" fill="currentColor" />
              <circle cx="12" cy="12" r="1.8" fill="currentColor" />
              <circle cx="18" cy="12" r="1.8" fill="currentColor" />
            </svg>
          </button>
        )}
        {view !== 'private' && <div style={{ width: 36 }} />}
      </div>

      {/* ===== Home 视图：微信风格聊天列表 + 搜索栏 ===== */}
      {view === 'home' && (
        <>
          {/* 搜索栏 */}
          <div style={{
            flexShrink: 0,
            padding: '6px 12px 8px',
            background: 'var(--wx-bg)',
            borderBottom: '0.5px solid var(--wx-border-light)',
          }}>
            <div style={{
              display: 'flex', alignItems: 'center', gap: 6,
              height: 32,
              padding: '0 10px',
              borderRadius: 8,
              background: searchQuery.trim() ? 'var(--wx-bg-active)' : 'var(--wx-search-bg)',
              border: '0.5px solid var(--wx-border-light)',
              transition: 'background 0.2s ease, border-color 0.2s ease',
            }}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" style={{ flexShrink: 0, opacity: 0.5, color: 'var(--wx-text)' }}>
                <circle cx="11" cy="11" r="7" stroke="currentColor" strokeWidth="2" />
                <path d="M21 21l-4.5-4.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
              </svg>
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder={t('chat.search_placeholder')}
                style={{
                  flex: 1, minWidth: 0,
                  background: 'transparent', border: 'none', outline: 'none',
                  color: 'var(--wx-text)', fontSize: 13, fontFamily: 'inherit',
                }}
              />
              {searching && (
                <LoadingSpinner size={12} color="var(--wx-icon)" thickness={1.5} />
              )}
              {searchQuery.trim() && !searching && (
                <button
                  onClick={() => setSearchQuery('')}
                  title={t('chat.search_clear')}
                  style={{
                    flexShrink: 0, width: 16, height: 16, padding: 0,
                    border: 'none', background: 'var(--wx-bg-active)', borderRadius: '50%',
                    cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center',
                    color: 'var(--wx-text-secondary)',
                  }}
                >
                  <svg width="8" height="8" viewBox="0 0 24 24" fill="none">
                    <path d="M6 6l12 12M18 6l-12 12" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
                  </svg>
                </button>
              )}
            </div>
          </div>

          {/* 列表 / 搜索结果 */}
          <div className="vivian-scroll" style={{
            flex: 1, overflowY: 'auto',
            background: 'var(--wx-bg)',
          }}>
            {searchQuery.trim() ? (
              /* 搜索结果列表 */
              <div style={{ padding: '4px 0' }}>
                {searchResults.length === 0 && !searching && (
                  <div style={{
                    textAlign: 'center', color: 'var(--wx-text-tertiary)', fontSize: 13,
                    padding: '40px 20px',
                  }}>
                    {t('chat.search_no_results')}
                  </div>
                )}
                {searchResults.length === 0 && searching && (
                  <div style={{
                    textAlign: 'center', color: 'var(--wx-text-tertiary)', fontSize: 13,
                    padding: '40px 20px',
                  }}>
                    {t('chat.loading_more')}
                  </div>
                )}
                {searchResults.map((r) => {
                  const isUser = r.role === 'user';
                  const sourceLabel = r.source === 'group'
                    ? t('chat.search_source_group')
                    : r.character_name;
                  const sourceColor = r.source === 'group'
                    ? '#5ac8fa'
                    : presenceDisplay(r.character_id).color;
                  return (
                    <button
                      key={`${r.id}-${r.character_id}`}
                      onClick={() => {
                        if (r.source === 'group') {
                          markConversationRead('group');
                          setView('group');
                          setGroupMessages([]);
                        } else {
                          markConversationRead(r.character_id);
                          setPrivateCharId(r.character_id);
                          setView('private');
                        }
                        setSearchQuery('');
                      }}
                      style={{
                        display: 'flex', alignItems: 'flex-start', gap: 10, width: '100%',
                        padding: '10px 14px',
                        background: 'transparent',
                        borderBottom: '0.5px solid var(--wx-border-light)',
                        border: 'none',
                        cursor: 'pointer',
                        color: 'var(--wx-text)', fontFamily: 'inherit', textAlign: 'left',
                        transition: 'background 0.15s ease',
                      }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-hover)')}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                    >
                      {/* 来源标识圆形头像 */}
                      <div style={{
                        width: 40, height: 40, borderRadius: 8, flexShrink: 0,
                        display: 'flex', alignItems: 'center', justifyContent: 'center',
                        fontSize: 16, fontWeight: 700, color: '#fff',
                        background: r.source === 'group'
                          ? 'linear-gradient(135deg, #5ac8fa, #007aff)'
                          : `linear-gradient(135deg, ${sourceColor}cc, ${sourceColor}88)`,
                      }}>
                        {r.source === 'group' ? (
                          <svg width="20" height="20" viewBox="0 0 24 24" fill="none">
                            <circle cx="9" cy="9" r="3.5" stroke="#fff" strokeWidth="1.8" />
                            <circle cx="16" cy="10" r="2.8" stroke="#fff" strokeWidth="1.8" />
                            <path d="M3 19c0-3 3-5 6-5s6 2 6 5" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" />
                            <path d="M14 17c1-2 3-3 5-3s4 1 4 4" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" />
                          </svg>
                        ) : (
                          <span style={{ fontSize: 14 }}>{r.character_name.slice(0, 1)}</span>
                        )}
                      </div>
                      {/* 内容区 */}
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 3 }}>
                          <span style={{
                            fontSize: 12, fontWeight: 600,
                            color: sourceColor,
                          }}>
                            {sourceLabel}
                          </span>
                          <span style={{
                            fontSize: 9, color: 'var(--wx-text-tertiary)',
                            padding: '1px 5px', borderRadius: 3,
                            background: 'var(--wx-search-bg)',
                          }}>
                            {isUser ? t('chat.role_user_badge') : t('chat.role_ai_badge')}
                          </span>
                          <span style={{
                            fontSize: 10, color: 'var(--wx-text-tertiary)',
                            marginLeft: 'auto', flexShrink: 0,
                            fontVariantNumeric: 'tabular-nums',
                          }}>
                            {formatSeparatorTime(r.timestamp, t)}
                          </span>
                        </div>
                        <div style={{
                          fontSize: 12, lineHeight: 1.4,
                          color: 'var(--wx-text-secondary)',
                          overflow: 'hidden', textOverflow: 'ellipsis',
                          display: '-webkit-box',
                          WebkitLineClamp: 2, WebkitBoxOrient: 'vertical',
                          wordBreak: 'break-word',
                        }}>
                          {r.content}
                        </div>
                      </div>
                    </button>
                  );
                })}
              </div>
            ) : (
              /* 聊天列表（默认） */
              <div style={{ padding: '4px 0' }}>
                {/* 群聊入口 */}
                <button
                  onClick={() => { markConversationRead('group'); setView('group'); setGroupMessages([]); }}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 12, width: '100%',
                    padding: '10px 14px',
                    background: 'transparent',
                    borderBottom: '0.5px solid var(--wx-border-light)',
                    border: 'none',
                    cursor: 'pointer',
                    color: 'var(--wx-text)', fontFamily: 'inherit', textAlign: 'left',
                    transition: 'background 0.15s ease',
                  }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-hover)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                >
                  <div style={{
                    position: 'relative',
                    width: 44, height: 44, borderRadius: 8, flexShrink: 0,
                    background: 'linear-gradient(135deg, #5ac8fa, #007aff)',
                    display: 'flex', alignItems: 'center', justifyContent: 'center',
                  }}>
                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
                      <circle cx="9" cy="9" r="3.5" stroke="#fff" strokeWidth="1.8" />
                      <circle cx="16" cy="10" r="2.8" stroke="#fff" strokeWidth="1.8" />
                      <path d="M3 19c0-3 3-5 6-5s6 2 6 5" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" />
                      <path d="M14 17c1-2 3-3 5-3s4 1 4 4" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" />
                    </svg>
                    <UnreadBadge count={unreadCounts['group'] ?? 0} />
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 15, fontWeight: 500, color: 'var(--wx-text)' }}>{t('chat.home_group_entry')}</div>
                    <div style={{
                      fontSize: 12, color: 'var(--wx-text-secondary)', marginTop: 2,
                      overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                    }}>
                      {formatPreview('group')}
                    </div>
                  </div>
                </button>

                {/* 在线角色列表 */}
                {onlineCharacters.map((c) => {
                  return (
                    <button
                      key={c.id}
                      onClick={() => { markConversationRead(c.id); setPrivateCharId(c.id); setView('private'); }}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 12, width: '100%',
                        padding: '10px 14px',
                        background: 'transparent',
                        borderBottom: '0.5px solid var(--wx-border-light)',
                        border: 'none',
                        cursor: 'pointer',
                        color: 'var(--wx-text)', fontFamily: 'inherit', textAlign: 'left',
                        transition: 'background 0.15s ease',
                      }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-hover)')}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                    >
                      <div style={{ position: 'relative', flexShrink: 0 }}>
                        <ContactAvatar characterId={c.id} name={c.name} />
                        <UnreadBadge count={unreadCounts[c.id] ?? 0} />
                      </div>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 15, fontWeight: 500, color: 'var(--wx-text)' }}>{c.name}</div>
                        <div style={{
                          fontSize: 12, color: 'var(--wx-text-secondary)', marginTop: 2,
                          overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                        }}>
                          {formatPreview(c.id)}
                        </div>
                      </div>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </>
      )}

      {/* ===== Private 视图：私聊消息列表 ===== */}
      {view === 'private' && (
        <div ref={listRef} className="vivian-scroll" onScroll={handleScroll} style={{
          flex: 1, overflowY: 'auto', padding: '12px 14px 8px',
          background: chatBackgrounds[privateCharId ?? '']
            ? `url("${convertFileSrc(chatBackgrounds[privateCharId!])}") center / cover no-repeat fixed, var(--wx-bg)`
            : 'var(--wx-bg)',
        }}>
          {initialLoading ? (
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8, color: 'var(--wx-icon)', fontSize: 13, marginTop: 40 }}>
              <LoadingSpinner size={16} color="var(--wx-icon)" thickness={1.5} /> {t('chat.loading_history')}
            </div>
          ) : (
            <>
              {!loadingMore && messages.length === 0 && (
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--wx-icon)', fontSize: 13, marginTop: 40, opacity: 0.7 }}>
                  {t('chat.empty_chat')}
                </div>
              )}
              {messages.length > 0 && (loadingMore || !hasMore) && (
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 6, padding: '8px 0', color: 'var(--wx-icon)', fontSize: 12 }}>
                  {loadingMore ? (<><LoadingSpinner size={14} color="var(--wx-icon)" thickness={1.5} /> {t('chat.loading_more')}</>) : t('chat.no_more')}
                </div>
              )}
              <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
                {virtualizer.getVirtualItems().map((vi) => {
                  const item = items[vi.index];
                  if (!item) return null;
                  return (
                    <div
                      key={vi.key}
                      data-index={vi.index}
                      ref={virtualizer.measureElement}
                      style={{
                        position: 'absolute',
                        top: 0,
                        left: 0,
                        width: '100%',
                        transform: `translateY(${vi.start}px)`,
                      }}
                    >
                      {item.kind === 'time' ? (
                        <div style={{ display: 'flex', justifyContent: 'center', margin: '12px 0' }}>
                          <span style={{
                            background: 'var(--wx-bg-active)', color: 'var(--wx-icon)', fontSize: 11,
                            padding: '3px 12px', borderRadius: 6,
                          }}>{item.text}</span>
                        </div>
                      ) : item.kind === 'msg' ? (
                        <Bubble message={item.msg} onOpenImage={setImageViewerSrc} senderName={privateCharName} characterId={privateCharId ?? undefined} />
                      ) : (
                        <LinkageCard card={item.card} t={t} characterId={privateCharId ?? undefined} />
                      )}
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      )}

      {/* ===== Details 视图：聊天详情（类似微信聊天信息页） ===== */}
      {view === 'details' && privateCharId && (
        <div className="vivian-scroll" style={{
          flex: 1, overflowY: 'auto',
          background: 'var(--wx-bg)',
        }}>
          {detailsSubView === 'main' ? (
            <>
              {/* 头像 */}
              <div style={{
                display: 'flex', flexDirection: 'column', alignItems: 'center',
                padding: '36px 0 24px', gap: 10,
              }}>
                <AiAvatar size={72} characterId={privateCharId} />
                <span style={{
                  fontSize: 17, fontWeight: 600, color: 'var(--wx-text)',
                }}>
                  {charRemarks[privateCharId] || privateCharName}
                </span>
              </div>

              {/* 设置项列表 */}
              <div style={{
                margin: '0 8px', borderRadius: 8, overflow: 'hidden',
                background: 'var(--wx-bg-surface)',
                borderTop: '0.5px solid var(--wx-border-light)',
                borderBottom: '0.5px solid var(--wx-border-light)',
              }}>
                {/* 编辑备注 */}
                {editingRemark ? (
                  <div style={{
                    padding: '12px 16px',
                    borderBottom: '0.5px solid var(--wx-border-light)',
                  }}>
                    <input
                      autoFocus
                      value={remarkInput}
                      onChange={(e) => setRemarkInput(e.target.value)}
                      onKeyDown={(e) => { if (e.key === 'Enter') handleSaveRemark(); if (e.key === 'Escape') setEditingRemark(false); }}
                      placeholder={t('chat.details_remark_placeholder')}
                      style={{
                        width: '100%', padding: '6px 10px', fontSize: 15,
                        background: 'var(--wx-input-bg)', color: 'var(--wx-text)',
                        border: '0.5px solid var(--wx-border)', borderRadius: 6,
                        outline: 'none', boxSizing: 'border-box',
                      }}
                    />
                    <div style={{ display: 'flex', gap: 12, marginTop: 8, justifyContent: 'flex-end' }}>
                      <button
                        onClick={() => setEditingRemark(false)}
                        style={{
                          padding: '4px 16px', fontSize: 14, borderRadius: 4,
                          background: 'var(--wx-bg-active)', color: 'var(--wx-text)',
                          border: 'none', cursor: 'pointer',
                        }}
                      >{t('cancel')}</button>
                      <button
                        onClick={handleSaveRemark}
                        style={{
                          padding: '4px 16px', fontSize: 14, borderRadius: 4,
                          background: 'var(--wx-accent, #576b95)', color: '#fff',
                          border: 'none', cursor: 'pointer',
                        }}
                      >{t('save')}</button>
                    </div>
                  </div>
                ) : (
                  <button
                    onClick={() => { setRemarkInput(charRemarks[privateCharId] ?? ''); setEditingRemark(true); }}
                    style={detailsRowStyle}
                    onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                    onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                  >
                    <span style={detailsLabelStyle}>{t('chat.details_edit_remark')}</span>
                    <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <span style={{ fontSize: 14, color: 'var(--wx-icon)' }}>
                        {charRemarks[privateCharId] || ''}
                      </span>
                      <ChevronRight />
                    </span>
                  </button>
                )}

                {/* 查找聊天内容 */}
                <button
                  onClick={() => { setDetailsSubView('search'); setDetailsSearchQuery(''); setDetailsSearchResults([]); }}
                  style={detailsRowStyle}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                >
                  <span style={detailsLabelStyle}>{t('chat.details_search_chat')}</span>
                  <ChevronRight />
                </button>

                {/* 设置当前聊天背景 */}
                <button
                  onClick={handleSetBackground}
                  style={{ ...detailsRowStyle, borderBottom: 'none' }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                >
                  <span style={detailsLabelStyle}>{t('chat.details_set_background')}</span>
                  <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                    {chatBackgrounds[privateCharId] && (
                      <img
                        src={convertFileSrc(chatBackgrounds[privateCharId])}
                        alt=""
                        style={{ width: 40, height: 40, borderRadius: 6, objectFit: 'cover' }}
                      />
                    )}
                    <ChevronRight />
                  </span>
                </button>
              </div>

              {/* 清除背景（仅在已设置时显示） */}
              {chatBackgrounds[privateCharId] && (
                <div style={{ margin: '12px 8px 0' }}>
                  <button
                    onClick={handleClearBackground}
                    style={{
                      width: '100%', padding: '12px', fontSize: 15,
                      background: 'var(--wx-bg-surface)', color: 'var(--wx-danger, #FA5151)',
                      border: 'none', borderRadius: 8, cursor: 'pointer',
                      borderTop: '0.5px solid var(--wx-border-light)',
                      borderBottom: '0.5px solid var(--wx-border-light)',
                    }}
                    onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                    onMouseLeave={(e) => (e.currentTarget.style.background = 'var(--wx-bg-surface)')}
                  >
                    {t('chat.details_clear_background')}
                  </button>
                </div>
              )}
            </>
          ) : (
            <>
              {/* 搜索子视图 */}
              <div style={{
                padding: '8px 12px', flexShrink: 0,
                background: 'var(--wx-bg-surface)',
                borderBottom: '0.5px solid var(--wx-border-light)',
              }}>
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 8,
                }}>
                  <button
                    onClick={() => { setDetailsSubView('main'); }}
                    style={{
                      border: 'none', background: 'transparent', cursor: 'pointer',
                      color: 'var(--wx-icon)', padding: 4, display: 'flex',
                    }}
                  >
                    <svg width="20" height="20" viewBox="0 0 24 24" fill="none">
                      <path d="M15 19l-7-7 7-7" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
                    </svg>
                  </button>
                  <input
                    autoFocus
                    value={detailsSearchQuery}
                    onChange={(e) => handleDetailsSearch(e.target.value)}
                    placeholder={t('chat.details_search_placeholder')}
                    style={{
                      flex: 1, padding: '6px 12px', fontSize: 14,
                      background: 'var(--wx-input-bg)', color: 'var(--wx-text)',
                      border: '0.5px solid var(--wx-border)', borderRadius: 6,
                      outline: 'none',
                    }}
                  />
                </div>
              </div>
              <div style={{ padding: '8px 12px' }}>
                {!detailsSearchQuery.trim() ? (
                  <div style={{
                    textAlign: 'center', color: 'var(--wx-icon)', fontSize: 13,
                    marginTop: 40, opacity: 0.7,
                  }}>
                    {t('chat.details_search_hint')}
                  </div>
                ) : detailsSearching ? (
                  <div style={{
                    display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8,
                    color: 'var(--wx-icon)', fontSize: 13, marginTop: 40,
                  }}>
                    <LoadingSpinner size={14} color="var(--wx-icon)" thickness={1.5} />
                  </div>
                ) : detailsSearchResults.length === 0 ? (
                  <div style={{
                    textAlign: 'center', color: 'var(--wx-icon)', fontSize: 13,
                    marginTop: 40, opacity: 0.7,
                  }}>
                    {t('chat.details_search_empty')}
                  </div>
                ) : (
                  <div>
                    {detailsSearchResults.map((r) => (
                      <div key={r.id} style={{
                        padding: '12px 14px', marginBottom: 4,
                        background: 'var(--wx-bg-surface)', borderRadius: 8,
                      }}>
                        <div style={{
                          fontSize: 11, color: 'var(--wx-icon)', marginBottom: 4,
                        }}>
                          {r.role === 'user' ? t('chat.role_user_badge') : privateCharName}
                          {' · '}
                          {new Date(r.timestamp).toLocaleString()}
                        </div>
                        <div style={{
                          fontSize: 14, color: 'var(--wx-text)',
                          whiteSpace: 'pre-wrap', wordBreak: 'break-word',
                          maxHeight: 60, overflow: 'hidden',
                        }}>
                          {r.content}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      )}

      {/* ===== Group 视图：群聊消息列表（虚拟滚动） ===== */}
      {view === 'group' && (
        <div ref={groupListRef} className="vivian-scroll" onScroll={handleGroupScroll} style={{
          flex: 1, overflowY: 'auto', padding: '12px 14px 8px',
          background: 'var(--wx-bg)',
        }}>
          {groupMessages.length === 0 && !groupStreaming && (
            <div style={{ height: 1 }} />
          )}
          <div style={{ height: groupVirtualizer.getTotalSize(), position: 'relative' }}>
            {groupVirtualizer.getVirtualItems().map((vi) => {
              const item = groupItems[vi.index];
              if (!item) return null;
              return (
                <div
                  key={vi.key}
                  data-index={vi.index}
                  ref={groupVirtualizer.measureElement}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  {item.kind === 'time' ? (
                    <div style={{ display: 'flex', justifyContent: 'center', margin: '12px 0' }}>
                      <span style={{
                        background: 'var(--wx-bg-active)', color: 'var(--wx-icon)', fontSize: 11,
                        padding: '3px 12px', borderRadius: 6,
                      }}>{item.text}</span>
                    </div>
                  ) : item.kind === 'msg' ? (
                    <Bubble
                      message={item.msg}
                      onOpenImage={setImageViewerSrc}
                      senderName={item.msg.character_id ? charNameMap.get(item.msg.character_id) : undefined}
                    />
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* ===== 底部输入区域（仅 private/group 视图） ===== */}
      {(view === 'private' || view === 'group') && (
        <div style={{
          flexShrink: 0, background: 'var(--wx-bg-surface)',
          borderTop: '0.5px solid var(--wx-border)',
        }}>
          {/* 图片草稿栏（粘贴/选图后先预览，可取消；与文本一起发送） */}
          {draftImages.length > 0 && (
            <div style={{
              display: 'flex', gap: 8, padding: '8px 24px 2px',
              overflowX: 'auto', scrollbarWidth: 'none', msOverflowStyle: 'none',
            }}>
              {draftImages.map((img) => (
                <div key={img.id} style={{ position: 'relative', flexShrink: 0, width: 56, height: 56 }}>
                  <img
                    src={img.dataUrl}
                    alt={img.name}
                    style={{ width: '100%', height: '100%', objectFit: 'cover', borderRadius: 6, border: '0.5px solid var(--wx-border)' }}
                  />
                  <button
                    type="button"
                    title="移除"
                    onClick={() => removeDraftImage(img.id)}
                    style={{
                      position: 'absolute', top: -5, right: -5,
                      width: 16, height: 16, borderRadius: '50%',
                      border: 'none', background: 'rgba(0,0,0,0.6)', color: '#fff',
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                      cursor: 'pointer', fontSize: 11, lineHeight: 1, padding: 0,
                    }}
                  >✕</button>
                </div>
              ))}
            </div>
          )}
          {/* 输入行 */}
          <div style={{
            display: 'flex', alignItems: 'center', gap: 6,
            padding: '8px 24px',
            position: 'relative',
          }}>
            {/* @ 提及上拉菜单（仅群聊视图） */}
            {view === 'group' && mentionState.active && mentionList.length > 0 && (
              <div style={{
                position: 'absolute',
                bottom: '100%',
                left: 24,
                right: 24,
                marginBottom: 4,
                background: 'var(--wx-bg-surface)',
                border: '0.5px solid var(--wx-border)',
                borderRadius: 8,
                boxShadow: '0 -4px 16px rgba(0,0,0,0.12)',
                overflow: 'hidden',
                zIndex: 50,
              }}>
                {mentionList.map((c, i) => (
                  <div
                    key={c.id}
                    onMouseDown={(e) => { e.preventDefault(); selectMention(c.id); }}
                    onMouseEnter={() => setMentionState((prev) => ({ ...prev, selectedIndex: i }))}
                    style={{
                      padding: '8px 14px',
                      cursor: 'pointer',
                      fontSize: 14,
                      color: 'var(--wx-text)',
                      background: i === mentionState.selectedIndex ? 'var(--wx-bg-active)' : 'transparent',
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                    }}
                  >
                    <span style={{ color: 'var(--wx-accent, #576b95)', fontWeight: 500 }}>@</span>
                    <span>{c.name}</span>
                  </div>
                ))}
              </div>
            )}
            <button
              onClick={() => { voiceRecordingRef.current ? handleVoiceStop() : handleVoiceStart(); }}
              disabled={recording}
              title={t('chat.btn_voice_input')}
              style={{
                ...toolBtn,
                background: voiceRecording ? 'var(--wx-recording-bg, rgba(255,69,58,0.12))' : 'transparent',
                color: voiceRecording ? '#FF453A' : 'var(--wx-icon)',
                cursor: recording ? 'not-allowed' : 'pointer',
                opacity: recording ? 0.4 : 1,
                userSelect: 'none',
                WebkitUserSelect: 'none',
                touchAction: 'none',
              }}
            >
              {voiceRecording ? (
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 12, fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>
                  <span style={{
                    width: 8, height: 8, borderRadius: '50%', background: '#FF453A',
                    animation: 'vivian-blink 1s steps(2) infinite',
                  }} />
                  {Math.ceil(voiceDuration)}″
                </span>
              ) : (
                <AudioMessageIcon />
              )}
            </button>

            <div style={{
              flex: 1, display: 'flex', alignItems: 'center',
              background: 'var(--wx-input-bg)', borderRadius: 20,
              border: '0.5px solid var(--wx-border)',
              padding: '0 4px 0 14px', minHeight: 36,
            }}>
              <textarea
                ref={inputRef} value={input} onChange={handleInputChange}
                onKeyDown={handleKeyDown}
                onPaste={(e) => {
                  const files = Array.from(e.clipboardData?.files ?? []);
                  const imgs = files.filter((f) => f.type.startsWith('image/'));
                  if (imgs.length > 0) {
                    e.preventDefault();
                    addDraftImages(imgs);
                  }
                }}
                onSelect={(e) => {
                  const el = e.currentTarget;
                  detectMention(el.value, el.selectionStart ?? el.value.length);
                }}
                onKeyUp={(e) => {
                  const el = e.currentTarget;
                  detectMention(el.value, el.selectionStart ?? el.value.length);
                }}
                onFocus={() => void invoke('set_side_chat_input_open', { open: true, label: 'chat' }).catch(() => {})}
                onBlur={() => void invoke('set_side_chat_input_open', { open: false, label: 'chat' }).catch(() => {})}
                placeholder={view === 'group' ? t('chat.group_input_placeholder') : t('chat.input_placeholder')}
                rows={1}
                className="vivian-chat-input"
                style={{
                  flex: 1, border: 'none', outline: 'none', resize: 'none', background: 'transparent',
                  color: 'var(--wx-text)', fontSize: 15, fontFamily: 'inherit',
                  lineHeight: 1.4, maxHeight: 80, padding: '8px 0',
                }}
              />
              <button
                onClick={toggleRecording}
                title={recording ? t('chat.btn_stop_recording') : t('chat.btn_asr_input')}
                style={{
                  width: 28, height: 28, flexShrink: 0, display: 'inline-flex',
                  alignItems: 'center', justifyContent: 'center',
                  border: 'none', borderRadius: '50%',
                  background: recording ? 'var(--wx-recording-bg)' : 'transparent',
                  cursor: 'pointer', marginRight: 2,
                  color: 'var(--wx-icon)',
                }}
                onMouseEnter={(e) => (e.currentTarget.style.background = recording ? 'var(--wx-recording-bg-hover)' : 'var(--wx-bg-active)')}
                onMouseLeave={(e) => (e.currentTarget.style.background = recording ? 'var(--wx-recording-bg)' : 'transparent')}
              >
                <MicIcon recording={recording} size={18} />
              </button>
            </div>

            <div ref={bottomPanelTriggersRef} style={{ display: 'contents' }}>
              <button
                onClick={() => togglePanel('emoji')}
                title={t('chat.btn_emoji')}
                style={toolBtn}
                onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <SmileIcon />
              </button>
              {view === 'private' && (
                <button
                  onClick={() => togglePanel('media')}
                  title={t('chat.btn_media')}
                  style={toolBtn}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                >
                  <PlusIcon />
                </button>
              )}
            </div>
          </div>

          {/* 底部面板：Emoji / Media */}
          <div ref={bottomPanelDrawerRef}>
            <div style={{
              maxHeight: bottomPanel === 'emoji' ? 160 : 0,
              opacity: bottomPanel === 'emoji' ? 1 : 0,
              pointerEvents: bottomPanel === 'emoji' ? 'auto' : 'none',
              overflow: 'hidden',
              background: 'var(--wx-bg-surface)',
              transition: 'max-height 200ms ease-out, opacity 180ms ease-out',
            }}>
              <div
                className="vivian-scroll"
                style={{
                  height: 160, padding: '4px 10px 8px',
                  borderTop: '0.5px solid var(--wx-border-light)',
                  overflowY: 'auto', overflowX: 'hidden',
                }}
              >
                {recentEmojis.length > 0 && (
                  <>
                    <div style={{ fontSize: 10, color: 'var(--wx-icon)', padding: '4px 2px 2px', letterSpacing: 0.3 }}>
                      {t('chat.emoji_recent')}
                    </div>
                    <div style={{ display: 'flex', flexWrap: 'wrap' }}>
                      {recentEmojis.map((emoji, i) => (
                        <button
                          key={`r-${i}`}
                          onClick={() => handleEmojiClick(emoji)}
                          style={{
                            width: 32, height: 32, flexShrink: 0, border: 'none', background: 'transparent',
                            fontSize: 20, cursor: 'pointer', borderRadius: 6,
                            display: 'flex', alignItems: 'center', justifyContent: 'center',
                          }}
                          onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                        >
                          {emoji}
                        </button>
                      ))}
                    </div>
                  </>
                )}
                <div style={{ fontSize: 10, color: 'var(--wx-icon)', padding: '4px 2px 2px', letterSpacing: 0.3 }}>
                  {t('chat.emoji_all')}
                </div>
                <div style={{ display: 'flex', flexWrap: 'wrap' }}>
                  {EMOJI_LIST.map((emoji, i) => (
                    <button
                      key={`a-${i}`}
                      onClick={() => handleEmojiClick(emoji)}
                      style={{
                        width: 32, height: 32, flexShrink: 0, border: 'none', background: 'transparent',
                        fontSize: 20, cursor: 'pointer', borderRadius: 6,
                        display: 'flex', alignItems: 'center', justifyContent: 'center',
                      }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--wx-bg-active)')}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                    >
                      {emoji}
                    </button>
                  ))}
                </div>
              </div>
            </div>

            {view === 'private' && (
              <div style={{
                maxHeight: bottomPanel === 'media' ? 200 : 0,
                opacity: bottomPanel === 'media' ? 1 : 0,
                pointerEvents: bottomPanel === 'media' ? 'auto' : 'none',
                overflow: 'hidden',
                background: 'var(--wx-bg-surface)',
                transition: 'max-height 200ms ease-out, opacity 180ms ease-out',
              }}>
                <div style={{
                  height: 200, padding: '16px 12px',
                  borderTop: '0.5px solid var(--wx-border-light)',
                  display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 12,
                  alignContent: 'center', justifyItems: 'center',
                }}>
                  <button
                    onClick={() => { setBottomPanel('none'); void emit('toast:show', { message: t('chat.voice_call_disabled'), type: 'info', duration: 3000, key: Date.now() }); }}
                    style={{
                      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 6,
                      background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--wx-text)',
                    }}
                  >
                    <div style={{
                      width: 48, height: 48, borderRadius: 14,
                      background: 'var(--wx-bg-active)',
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                      color: 'var(--wx-icon)',
                    }}><PhoneIcon /></div>
                    <span style={{ fontSize: 11 }}>{t('chat.realtime_call_entry')}</span>
                  </button>
                  <button
                    onClick={handleSendImage}
                    style={{
                      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 6,
                      background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--wx-text)',
                    }}
                  >
                    <div style={{
                      width: 48, height: 48, borderRadius: 14,
                      background: 'var(--wx-bg-active)',
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                      color: 'var(--wx-icon)',
                    }}><ImageIcon /></div>
                    <span style={{ fontSize: 11 }}>{t('chat.send_image_entry')}</span>
                  </button>
                  <button
                    onClick={handleSendFile}
                    style={{
                      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 6,
                      background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--wx-text)',
                    }}
                  >
                    <div style={{
                      width: 48, height: 48, borderRadius: 14,
                      background: 'var(--wx-bg-active)',
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                      color: 'var(--wx-icon)',
                    }}><FileIcon /></div>
                    <span style={{ fontSize: 11 }}>{t('chat.send_file_entry')}</span>
                  </button>
                  <button
                    onClick={handleCapturePhoto}
                    style={{
                      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 6,
                      background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--wx-text)',
                    }}
                  >
                    <div style={{
                      width: 48, height: 48, borderRadius: 14,
                      background: 'var(--wx-bg-active)',
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                      color: 'var(--wx-icon)',
                    }}><CameraIcon /></div>
                    <span style={{ fontSize: 11 }}>{t('chat.capture_photo_entry')}</span>
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* ===== Home Indicator（底部居中圆角条） ===== */}
      <div style={{
        flexShrink: 0, height: 24,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: view !== 'home' ? 'var(--wx-bg-surface)' : 'var(--wx-bg)',
      }}>
        <div style={{
          width: 134, height: 5, borderRadius: 3,
          background: 'var(--wx-home-indicator)',
        }} />
      </div>

      {callView !== 'none' && (
        <div style={{ display: callView === 'full' ? 'block' : 'none', position: 'absolute', inset: 0 }}>
          <RealtimeCallOverlay
            onClose={() => setCallView('none')}
            onMinimize={() => setCallView('minimized')}
          />
        </div>
      )}
      {callView === 'minimized' && (
        <RealtimeCallBubble
          onExpand={() => setCallView('full')}
          onClose={() => setCallView('none')}
        />
      )}
      <ImageViewer src={imageViewerSrc} onClose={() => setImageViewerSrc(null)} />
      <CameraCapture
        open={cameraOpen}
        onClose={() => setCameraOpen(false)}
        onCaptured={handlePhotoCaptured}
      />
    </div>
  );
};

/** 导航栏按钮基础样式 */
const navBtn: React.CSSProperties = {
  width: 36, height: 36, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
  border: 'none', background: 'transparent', borderRadius: 10, cursor: 'pointer', transition: 'background 0.15s ease',
  color: 'var(--wx-text)',
};

/** 底部工具栏按钮 */
const toolBtn: React.CSSProperties = {
  width: 36, height: 36, flexShrink: 0, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
  border: 'none', background: 'transparent', borderRadius: 10, cursor: 'pointer', transition: 'background 0.15s ease',
  color: 'var(--wx-toolbar-icon)',
};

/** details 视图行样式（微信设置页风格） */
const detailsRowStyle: React.CSSProperties = {
  width: '100%', display: 'flex', alignItems: 'center', justifyContent: 'space-between',
  padding: '14px 16px', border: 'none', background: 'transparent',
  cursor: 'pointer', textAlign: 'left',
  borderBottom: '0.5px solid var(--wx-border-light)',
  transition: 'background 0.15s ease',
};

/** details 视图行标签样式 */
const detailsLabelStyle: React.CSSProperties = {
  fontSize: 16, color: 'var(--wx-text)',
};

/** 右箭头图标（微信风格 chevron） */
const ChevronRight: React.FC = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" style={{ flexShrink: 0 }}>
    <path d="M9 6l6 6-6 6" stroke="var(--wx-icon)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

export default ChatWindow;
