import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { getCharacterId } from '../characterContext';

type CallState = 'idle' | 'connecting' | 'active' | 'closing' | 'error';

interface RealtimeEvent {
  type: string;
  state?: CallState;
  dialog_id?: string;
  text?: string;
  seconds?: number;
  message?: string;
  input_text_tokens?: number;
  input_audio_tokens?: number;
  output_text_tokens?: number;
  output_audio_tokens?: number;
}

function formatDuration(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

interface RealtimeCallOverlayProps {
  onClose: () => void;
  onMinimize: () => void;
}

/* ===== iOS 风格 SVG 图标 ===== */
const MicOnIcon: React.FC<{ size?: number; color?: string }> = ({ size = 24, color = '#fff' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <rect x="9" y="3" width="6" height="11" rx="3" stroke={color} strokeWidth="1.6" />
    <path d="M5 11a7 7 0 0 0 14 0M12 18v3" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
  </svg>
);

const MicOffIcon: React.FC<{ size?: number; color?: string }> = ({ size = 24, color = '#fff' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <path d="M9 9V6a3 3 0 0 1 6 0v5" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
    <path d="M15 12.5a3 3 0 0 1-4.8-2.4" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
    <path d="M5 11a7 7 0 0 0 11.3 5.5M17.7 14.5A7 7 0 0 0 19 11" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
    <path d="M12 18v3M9 21h6" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
    <line x1="4" y1="4" x2="20" y2="20" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
  </svg>
);

const SpeakerOnIcon: React.FC<{ size?: number; color?: string }> = ({ size = 24, color = '#fff' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <path d="M11 5L6.5 8.5H4v7h2.5L11 19V5z" stroke={color} strokeWidth="1.6" strokeLinejoin="round" />
    <path d="M15.5 8.5a5 5 0 0 1 0 7M18 5.5a9 9 0 0 1 0 13" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
  </svg>
);

const SpeakerOffIcon: React.FC<{ size?: number; color?: string }> = ({ size = 24, color = '#fff' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <path d="M11 5L6.5 8.5H4v7h2.5L11 19V5z" stroke={color} strokeWidth="1.6" strokeLinejoin="round" />
    <line x1="15" y1="9" x2="20" y2="14" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
    <line x1="20" y1="9" x2="15" y2="14" stroke={color} strokeWidth="1.6" strokeLinecap="round" />
  </svg>
);

const HangupIcon: React.FC<{ size?: number; color?: string }> = ({ size = 28, color = '#fff' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <path
      d="M4.5 9.5c0-1 .8-1.8 1.8-1.8h.4c1 0 1.8.8 1.8 1.8v1c0 .4.3.8.8.8h.4c.5 0 .8-.4.8-.8v-1c0-1 .8-1.8 1.8-1.8h1c1 0 1.8.8 1.8 1.8v1c0 .4.3.8.8.8h.4c.5 0 .8-.4.8-.8v-1c0-1 .8-1.8 1.8-1.8h.4c1 0 1.8.8 1.8 1.8v2c0 2-1.6 3.6-3.6 3.6h-.2c-4.4 0-8.5-2.7-10.4-6.6-.3-.6-.4-1.2-.4-1.8v-.2z"
      fill={color}
    />
  </svg>
);

const ChevronDownIcon: React.FC<{ size?: number; color?: string }> = ({ size = 20, color = 'rgba(255,255,255,0.7)' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none">
    <path d="M6 9l6 6 6-6" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

const AvatarIcon: React.FC<{ size?: number }> = ({ size = 80 }) => (
  <svg width={size} height={size} viewBox="0 0 80 80" fill="none">
    <circle cx="40" cy="40" r="40" fill="url(#avatarGrad)" />
    <circle cx="40" cy="32" r="13" fill="rgba(255,255,255,0.9)" />
    <path d="M16 68c4-13 15-20 24-20s20 7 24 20" fill="rgba(255,255,255,0.9)" />
    <defs>
      <linearGradient id="avatarGrad" x1="0" y1="0" x2="80" y2="80">
        <stop offset="0%" stopColor="#667eea" />
        <stop offset="100%" stopColor="#764ba2" />
      </linearGradient>
    </defs>
  </svg>
);

/** 实时语音通话覆盖层 — 内嵌在 ChatWindow 中，全屏覆盖聊天界面 */
export default function RealtimeCallOverlay({ onClose, onMinimize }: RealtimeCallOverlayProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<CallState>('connecting');
  const [duration, setDuration] = useState(0);
  const [asrText, setAsrText] = useState('');
  const [aiText, setAiText] = useState('');
  const [errorMessage, setErrorMessage] = useState('');
  const [muted, setMuted] = useState(false);
  const [speakerOn, setSpeakerOn] = useState(true);
  const [aiSpeaking, setAiSpeaking] = useState(false);
  const [usage, setUsage] = useState<{ in: number; out: number } | null>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const stoppedRef = useRef(false);
  const wasActiveRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      unlistenRef.current = await listen<RealtimeEvent>('realtime:event', (e) => {
        const ev = e.payload;
        switch (ev.type) {
          case 'state_changed':
            if (ev.state) {
              setState(ev.state);
              if (ev.state === 'active') wasActiveRef.current = true;
              if (ev.state === 'idle') {
                setAiSpeaking(false);
                if (wasActiveRef.current && !stoppedRef.current) {
                  stoppedRef.current = true;
                  setTimeout(() => onClose(), 500);
                }
              }
              if (ev.state === 'error') setAiSpeaking(false);
            }
            break;
          case 'asr_partial':
          case 'asr_final':
            if (ev.text) setAsrText(ev.text);
            break;
          case 'ai_text_delta':
            if (ev.text) setAiText((prev) => prev + ev.text);
            break;
          case 'ai_text_done':
            if (ev.text) setAiText(ev.text);
            break;
          case 'ai_audio_started':
            setAiSpeaking(true);
            break;
          case 'ai_audio_finished':
            setAiSpeaking(false);
            break;
          case 'duration_tick':
            if (typeof ev.seconds === 'number') setDuration(ev.seconds);
            break;
          case 'usage':
            setUsage({
              in: (ev.input_text_tokens ?? 0) + (ev.input_audio_tokens ?? 0),
              out: (ev.output_text_tokens ?? 0) + (ev.output_audio_tokens ?? 0),
            });
            break;
          case 'error':
            setErrorMessage(ev.message ?? t('realtime.status_error'));
            setState('error');
            break;
        }
      });

      if (cancelled) return;
      try {
        const status = await invoke<{ state: CallState }>('get_realtime_status', { characterId: getCharacterId() ?? undefined });
        if (cancelled) return;
        if (status.state && status.state !== 'idle') {
          setState(status.state);
        } else {
          setState('connecting');
          try {
            await invoke('start_realtime_call', { characterId: getCharacterId() ?? undefined });
          } catch (err) {
            if (cancelled) return;
            setErrorMessage(String(err));
            setState('error');
          }
        }
      } catch {
        // 忽略
      }
    })();

    return () => {
      cancelled = true;
      if (unlistenRef.current) unlistenRef.current();
    };
  }, [t]);

  const handleHangup = useCallback(async () => {
    if (stoppedRef.current) return;
    stoppedRef.current = true;
    setState('closing');
    try {
      await invoke('stop_realtime_call', { characterId: getCharacterId() ?? undefined });
    } catch {
      // 忽略
    }
    onClose();
  }, [onClose]);

  const handleRetry = useCallback(async () => {
    setErrorMessage('');
    setState('connecting');
    try {
      await invoke('stop_realtime_call', { characterId: getCharacterId() ?? undefined });
      await new Promise((r) => setTimeout(r, 300));
      await invoke('start_realtime_call', { characterId: getCharacterId() ?? undefined });
    } catch (err) {
      setErrorMessage(String(err));
      setState('error');
    }
  }, []);

  const toggleMute = useCallback(() => {
    setMuted((m) => !m);
  }, []);

  const toggleSpeaker = useCallback(() => {
    setSpeakerOn((s) => !s);
  }, []);

  const isActive = state === 'active';
  const isConnecting = state === 'connecting';
  const isError = state === 'error';

  const statusText = (() => {
    switch (state) {
      case 'idle': return t('realtime.status_idle');
      case 'connecting': return t('realtime.status_connecting');
      case 'active': return aiSpeaking ? t('realtime.ai_speaking') : t('realtime.listening');
      case 'closing': return t('realtime.status_closing');
      case 'error': return t('realtime.status_error');
    }
  })();

  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 500,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '52px 28px 40px',
        background: 'linear-gradient(180deg, #1C1C1E 0%, #1C1C1E 40%, #2C2C2E 100%)',
        color: '#fff',
        userSelect: 'none',
        overflow: 'hidden',
        borderRadius: 'inherit',
      }}
    >
      {/* 顶部导航栏：返回 + 名称/状态 */}
      <div style={{ width: '100%', display: 'flex', alignItems: 'center', justifyContent: 'space-between', position: 'relative' }}>
        <button
          onClick={onMinimize}
          title={t('realtime.minimize')}
          style={{
            width: 36, height: 36,
            borderRadius: '50%',
            border: 'none',
            background: 'rgba(255,255,255,0.1)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            cursor: 'pointer',
            transition: 'background 0.15s',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.2)')}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
        >
          <ChevronDownIcon />
        </button>
        <div style={{ textAlign: 'center', position: 'absolute', left: '50%', transform: 'translateX(-50%)' }}>
          <div style={{ fontSize: 17, fontWeight: 600, letterSpacing: 0.3 }}>
            {t('realtime.contact_name')}
          </div>
          <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.5)', marginTop: 2, height: 16 }}>
            {isConnecting || isError ? statusText : formatDuration(duration)}
          </div>
        </div>
        <div style={{ width: 36 }} />
      </div>

      {/* 中部：头像 + 声波 + 字幕 */}
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 28, flex: 1, justifyContent: 'center', width: '100%' }}>
        <div style={{ position: 'relative' }}>
          <div
            style={{
              width: 120,
              height: 120,
              borderRadius: '50%',
              overflow: 'hidden',
              boxShadow: aiSpeaking
                ? '0 0 50px rgba(102, 126, 234, 0.5), 0 0 100px rgba(118, 75, 162, 0.2)'
                : '0 8px 32px rgba(0,0,0,0.4)',
              transition: 'box-shadow 0.4s',
              animation: aiSpeaking ? 'realtime-pulse 1.5s ease-in-out infinite' : 'none',
            }}
          >
            <AvatarIcon size={120} />
          </div>
          {isActive && (
            <div
              style={{
                position: 'absolute',
                bottom: -2,
                right: -2,
                width: 28,
                height: 28,
                borderRadius: '50%',
                background: '#34C759',
                border: '3px solid #1C1C1E',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
              }}
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
                <path d="M5 12l5 5L20 7" stroke="#fff" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </div>
          )}
        </div>

        {/* 声波条 */}
        <div style={{ display: 'flex', gap: 4, alignItems: 'center', height: 36 }}>
          {Array.from({ length: 9 }).map((_, i) => (
            // 静态条，用 index 作 key
            <div
              key={i}
              style={{
                width: 3,
                borderRadius: 2,
                background: isActive
                  ? (aiSpeaking ? '#667eea' : 'rgba(255,255,255,0.3)')
                  : 'rgba(255,255,255,0.15)',
                animation: (isActive && aiSpeaking) ? `realtime-wave 0.9s ease-in-out ${i * 0.08}s infinite alternate` : 'none',
                height: aiSpeaking ? undefined : (isConnecting ? 4 : 6),
                transition: 'height 0.3s',
              }}
            />
          ))}
        </div>

        {/* 实时字幕 */}
        <div
          style={{
            minHeight: 56,
            maxWidth: 320,
            textAlign: 'center',
            fontSize: 14,
            lineHeight: 1.6,
            color: 'rgba(255,255,255,0.8)',
          }}
        >
          {asrText && (
            <div style={{ marginBottom: 8, color: 'rgba(255,255,255,0.45)' }}>
              <span style={{ marginRight: 6, color: 'rgba(255,255,255,0.3)' }}>{t('realtime.you')}:</span>
              {asrText}
            </div>
          )}
          {aiText && (
            <div>
              <span style={{ marginRight: 6, color: 'rgba(255,255,255,0.3)' }}>{t('realtime.contact_name')}:</span>
              {aiText}
            </div>
          )}
          {!asrText && !aiText && isActive && (
            <div style={{ color: 'rgba(255,255,255,0.3)', fontSize: 13 }}>{statusText}</div>
          )}
        </div>

        {errorMessage && (
          <div
            style={{
              fontSize: 13,
              color: '#FF453A',
              textAlign: 'center',
              maxWidth: 300,
              padding: '10px 16px',
              background: 'rgba(255, 69, 58, 0.12)',
              borderRadius: 12,
              border: '1px solid rgba(255, 69, 58, 0.2)',
            }}
          >
            {errorMessage}
          </div>
        )}

        {usage && (
          <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.25)' }}>
            tokens: ↑{usage.in} ↓{usage.out}
          </div>
        )}
      </div>

      {/* 底部：控制按钮 */}
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 20, width: '100%' }}>
        <div style={{ display: 'flex', gap: 48, alignItems: 'center', justifyContent: 'center' }}>
          <CallControlButton
            label={muted ? t('realtime.unmute') : t('realtime.mute')}
            active={muted}
            disabled={!isActive}
            onClick={toggleMute}
          >
            {muted ? <MicOffIcon size={22} /> : <MicOnIcon size={22} />}
          </CallControlButton>

          <button
            onClick={handleHangup}
            style={{
              width: 68,
              height: 68,
              borderRadius: '50%',
              border: 'none',
              background: '#FF3B30',
              color: '#fff',
              cursor: 'pointer',
              boxShadow: '0 6px 24px rgba(255, 59, 48, 0.4)',
              transition: 'transform 0.15s, box-shadow 0.15s',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.transform = 'scale(1.08)';
              e.currentTarget.style.boxShadow = '0 8px 32px rgba(255, 59, 48, 0.55)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.transform = 'scale(1)';
              e.currentTarget.style.boxShadow = '0 6px 24px rgba(255, 59, 48, 0.4)';
            }}
          >
            <HangupIcon size={28} />
          </button>

          <CallControlButton
            label={speakerOn ? t('realtime.speaker_off') : t('realtime.speaker_on')}
            active={!speakerOn}
            disabled={!isActive}
            onClick={toggleSpeaker}
          >
            {speakerOn ? <SpeakerOnIcon size={22} /> : <SpeakerOffIcon size={22} />}
          </CallControlButton>
        </div>

        {isError && (
          <button
            onClick={handleRetry}
            style={{
              padding: '10px 32px',
              borderRadius: 24,
              border: '1px solid rgba(255,255,255,0.2)',
              background: 'rgba(255,255,255,0.08)',
              color: '#fff',
              fontSize: 14,
              fontWeight: 500,
              cursor: 'pointer',
              transition: 'background 0.15s',
            }}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.16)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.08)')}
          >
            {t('realtime.retry')}
          </button>
        )}
      </div>

      <style>{`
        @keyframes realtime-pulse {
          0%, 100% { transform: scale(1); }
          50% { transform: scale(1.06); }
        }
        @keyframes realtime-wave {
          0% { height: 4px; }
          100% { height: 28px; }
        }
      `}</style>
    </div>
  );
}

interface CallControlButtonProps {
  label: string;
  active?: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}

function CallControlButton({ label, active, disabled, onClick, children }: CallControlButtonProps) {
  const [hover, setHover] = useState(false);
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 8,
        background: 'transparent',
        border: 'none',
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.35 : 1,
      }}
    >
      <div
        style={{
          width: 56,
          height: 56,
          borderRadius: '50%',
          background: active
            ? '#fff'
            : (hover ? 'rgba(255,255,255,0.2)' : 'rgba(255,255,255,0.12)'),
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          transition: 'background 0.2s, transform 0.15s',
          transform: hover && !disabled ? 'scale(1.08)' : 'scale(1)',
        }}
        onMouseEnter={() => !disabled && setHover(true)}
        onMouseLeave={() => setHover(false)}
      >
        <div style={{ color: active ? '#1C1C1E' : '#fff', display: 'flex' }}>{children}</div>
      </div>
      <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.55)' }}>{label}</span>
    </button>
  );
}

/* ===== 悬浮小窗：通话最小化时显示，可拖动并吸附边缘 ===== */
interface RealtimeCallBubbleProps {
  onExpand: () => void;
  onClose: () => void;
}

const BUBBLE_SIZE = 56;
const BUBBLE_MARGIN = 12;
const BUBBLE_TOP_MIN = 60;

export function RealtimeCallBubble({ onExpand, onClose }: RealtimeCallBubbleProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<CallState>('connecting');
  const [duration, setDuration] = useState(0);
  const [aiSpeaking, setAiSpeaking] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    startX: number; startY: number;
    origX: number; origY: number;
    moved: boolean;
    rafId: number | null;
    x: number; y: number;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      unlistenRef.current = await listen<RealtimeEvent>('realtime:event', (e) => {
        const ev = e.payload;
        switch (ev.type) {
          case 'state_changed':
            if (ev.state) {
              setState(ev.state);
              if (ev.state === 'idle') break;
              if (ev.state === 'error') setAiSpeaking(false);
            }
            break;
          case 'ai_audio_started':
            setAiSpeaking(true);
            break;
          case 'ai_audio_finished':
            setAiSpeaking(false);
            break;
          case 'duration_tick':
            if (typeof ev.seconds === 'number') setDuration(ev.seconds);
            break;
        }
      });

      if (cancelled) return;
      try {
        const status = await invoke<{ state: CallState }>('get_realtime_status', { characterId: getCharacterId() ?? undefined });
        if (!cancelled && status.state) setState(status.state);
      } catch { /* 忽略 */ }
    })();

    return () => {
      cancelled = true;
      if (unlistenRef.current) unlistenRef.current();
    };
  }, []);

  useEffect(() => {
    if (state === 'idle') onClose();
  }, [state, onClose]);

  // 初始定位到右上角
  useEffect(() => {
    if (pos !== null) return;
    const el = containerRef.current;
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;
    const rect = parent.getBoundingClientRect();
    const x = rect.width - BUBBLE_SIZE - BUBBLE_MARGIN;
    const y = BUBBLE_TOP_MIN;
    setPos({ x, y });
  }, [pos]);

  const handlePointerDown = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const el = containerRef.current;
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;

    const parentRect = parent.getBoundingClientRect();
    const currentX = pos?.x ?? parentRect.width - BUBBLE_SIZE - BUBBLE_MARGIN;
    const currentY = pos?.y ?? BUBBLE_TOP_MIN;

    dragRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      origX: currentX,
      origY: currentY,
      moved: false,
      rafId: null,
      x: currentX,
      y: currentY,
    };

    setDragging(true);
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }, [pos]);

  const handlePointerMove = useCallback((e: React.PointerEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const dx = e.clientX - drag.startX;
    const dy = e.clientY - drag.startY;

    if (!drag.moved && Math.hypot(dx, dy) > 4) {
      drag.moved = true;
    }

    const el = containerRef.current;
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;
    const parentRect = parent.getBoundingClientRect();

    let nx = drag.origX + dx;
    let ny = drag.origY + dy;

    nx = Math.max(BUBBLE_MARGIN, Math.min(nx, parentRect.width - BUBBLE_SIZE - BUBBLE_MARGIN));
    ny = Math.max(BUBBLE_TOP_MIN, Math.min(ny, parentRect.height - BUBBLE_SIZE - BUBBLE_MARGIN));

    drag.x = nx;
    drag.y = ny;

    if (drag.rafId === null) {
      drag.rafId = requestAnimationFrame(() => {
        if (dragRef.current) {
          setPos({ x: dragRef.current.x, y: dragRef.current.y });
          dragRef.current.rafId = null;
        }
      });
    }
  }, []);

  const handlePointerUp = useCallback((e: React.PointerEvent) => {
    const drag = dragRef.current;
    if (!drag) return;

    if (drag.rafId !== null) {
      cancelAnimationFrame(drag.rafId);
      drag.rafId = null;
    }

    const wasDragging = drag.moved;
    setDragging(false);

    const el = containerRef.current;
    const parent = el?.parentElement;
    if (!el || !parent) {
      dragRef.current = null;
      if (!wasDragging) onExpand();
      return;
    }

    // 吸附到最近的左右边缘
    const parentRect = parent.getBoundingClientRect();
    const centerX = drag.x + BUBBLE_SIZE / 2;
    const snapX = centerX < parentRect.width / 2
      ? BUBBLE_MARGIN
      : parentRect.width - BUBBLE_SIZE - BUBBLE_MARGIN;
    const snapY = Math.max(BUBBLE_TOP_MIN, Math.min(drag.y, parentRect.height - BUBBLE_SIZE - BUBBLE_MARGIN));

    setPos({ x: snapX, y: snapY });
    dragRef.current = null;

    (e.target as HTMLElement).releasePointerCapture(e.pointerId);

    if (!wasDragging) {
      onExpand();
    }
  }, [onExpand]);

  const isActive = state === 'active';
  const isConnecting = state === 'connecting';

  if (pos === null) {
    return <div ref={containerRef} style={{ position: 'absolute', top: 0, left: 0, width: BUBBLE_SIZE, height: BUBBLE_SIZE, opacity: 0 }} />;
  }

  return (
    <div
      ref={containerRef}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      style={{
        position: 'absolute',
        left: pos.x,
        top: pos.y,
        zIndex: 400,
        width: BUBBLE_SIZE,
        height: BUBBLE_SIZE,
        borderRadius: '50%',
        background: dragging ? 'linear-gradient(135deg, #7c8ff0 0%, #8a5fba 100%)' : 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
        boxShadow: dragging
          ? '0 8px 32px rgba(102, 126, 234, 0.7), 0 4px 16px rgba(0,0,0,0.4)'
          : '0 4px 20px rgba(102, 126, 234, 0.5), 0 2px 8px rgba(0,0,0,0.3)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        cursor: dragging ? 'grabbing' : 'grab',
        touchAction: 'none',
        userSelect: 'none',
        transition: dragging ? 'none' : 'left 0.25s cubic-bezier(0.34, 1.56, 0.64, 1), top 0.25s cubic-bezier(0.34, 1.56, 0.64, 1), box-shadow 0.2s, background 0.2s',
        overflow: 'visible',
        transform: dragging ? 'scale(1.05)' : 'scale(1)',
      }}
      title={t('realtime.contact_name')}
    >
      {/* 脉冲环 */}
      {(isActive || isConnecting) && !dragging && (
        <>
          <div style={{
            position: 'absolute',
            inset: -4,
            borderRadius: '50%',
            border: '2px solid rgba(102, 126, 234, 0.4)',
            animation: 'bubble-pulse 1.8s ease-out infinite',
          }} />
          <div style={{
            position: 'absolute',
            inset: -4,
            borderRadius: '50%',
            border: '2px solid rgba(102, 126, 234, 0.3)',
            animation: 'bubble-pulse 1.8s ease-out 0.6s infinite',
          }} />
        </>
      )}

      {/* 图标/文字 */}
      {isActive ? (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="9" y="3" width="6" height="11" rx="3" fill="#fff" />
          <path d="M5 11a7 7 0 0 0 14 0M12 18v3" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" opacity="0.9" />
        </svg>
      ) : isConnecting ? (
        <div style={{ display: 'flex', gap: 3 }}>
          {[0, 1, 2].map((i) => (
            // 3 个静态连接动画圆点，数量固定且无业务 ID，使用 index 作为 key
            <div key={i} style={{
              width: 5, height: 5, borderRadius: '50%', background: '#fff',
              animation: `bubble-dot 1s ease-in-out ${i * 0.15}s infinite`,
            }} />
          ))}
        </div>
      ) : (
        <span style={{ fontSize: 20, fontWeight: 700, color: '#fff' }}>!</span>
      )}

      {/* 通话时长标签 */}
      {isActive && !dragging && (
        <div style={{
          position: 'absolute',
          bottom: -20,
          left: '50%',
          transform: 'translateX(-50%)',
          fontSize: 10,
          color: 'rgba(255,255,255,0.7)',
          whiteSpace: 'nowrap',
          background: 'rgba(0,0,0,0.5)',
          padding: '1px 6px',
          borderRadius: 8,
          pointerEvents: 'none',
        }}>
          {formatDuration(duration)}
        </div>
      )}

      {/* 语音状态指示点 */}
      {isActive && aiSpeaking && !dragging && (
        <div style={{
          position: 'absolute',
          bottom: 2,
          right: 2,
          width: 12,
          height: 12,
          borderRadius: '50%',
          background: '#34C759',
          border: '2px solid #764ba2',
        }} />
      )}

      <style>{`
        @keyframes bubble-pulse {
          0% { transform: scale(1); opacity: 0.8; }
          100% { transform: scale(1.5); opacity: 0; }
        }
        @keyframes bubble-dot {
          0%, 80%, 100% { transform: scale(0.6); opacity: 0.4; }
          40% { transform: scale(1); opacity: 1; }
        }
      `}</style>
    </div>
  );
}
