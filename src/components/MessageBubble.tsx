import React, { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { stripActions } from '../utils/ActionText';

export type BubblePosition = 'top' | 'bottom' | 'left' | 'right';

export interface MessageBubbleProps {
  text: string;
  duration?: number;
  onClose?: () => void;
  position?: BubblePosition;
  maxWidth?: number;
  /** 角色 ID：决定气泡配色。nana=紫+银，vivian=白+黄，其他回退默认深色 */
  characterId?: string;
  /** 跨角色对话标记：true 时切换为虚线边框样式 */
  crossCharacter?: boolean;
  /** 跨角色对话的收听人名称（显示在气泡角落） */
  listenerName?: string;
}

interface BubbleTheme {
  background: string;
  textColor: string;
  tailColor: string;
  shadow: string;
  /** 跨角色模式下的虚线边框颜色 */
  crossBorderColor: string;
}

const BUBBLE_THEMES: Record<string, BubbleTheme> = {
  // Nana：紫底银字
  nana: {
    background: 'linear-gradient(135deg, rgba(108, 92, 231, 0.95) 0%, rgba(191, 90, 242, 0.92) 100%)',
    textColor: '#EDEDF7',
    tailColor: '#6c5ce7',
    shadow: '0 6px 20px rgba(108, 92, 231, 0.38)',
    crossBorderColor: 'rgba(191, 90, 242, 0.7)',
  },
  // Vivian：奶白底 + 黄色内嵌边框（可爱风）
  vivian: {
    background: 'rgba(255, 251, 230, 0.97)',
    textColor: '#5A4515',
    tailColor: '#FFF6D4',
    // 黄色外光晕 + 2px 黄色内嵌边框（inset 不占布局空间）
    shadow: '0 6px 20px rgba(255, 214, 10, 0.38), inset 0 0 0 2px #FFD60A',
    crossBorderColor: 'rgba(255, 214, 10, 0.65)',
  },
};

const DEFAULT_THEME: BubbleTheme = {
  background: 'var(--bg-overlay)',
  textColor: 'var(--text-primary)',
  tailColor: 'rgba(30, 30, 40, 0.95)',
  shadow: '0 6px 20px rgba(0, 0, 0, 0.15)',
  crossBorderColor: 'rgba(255, 255, 255, 0.35)',
};

const getBubbleTheme = (charId?: string): BubbleTheme => {
  if (charId && BUBBLE_THEMES[charId]) return BUBBLE_THEMES[charId];
  return DEFAULT_THEME;
};

// 入场动画初始位移（与主题无关，提取为模块级常量避免每次渲染重建）
const INITIAL_TRANSFORM: Record<BubblePosition, string> = {
  top: 'translateY(8px)',
  bottom: 'translateY(-8px)',
  left: 'translateX(8px)',
  right: 'translateX(-8px)',
};

// 收信人标签静态样式（与主题无关）
const LISTENER_TAG_STYLE: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 4,
  marginBottom: 6,
  padding: '2px 7px',
  borderRadius: 999,
  color: 'inherit',
  background: 'rgba(0, 0, 0, 0.08)',
  fontSize: 10,
  fontWeight: 600,
  lineHeight: '14px',
  letterSpacing: 0.2,
};

const MessageBubble: React.FC<MessageBubbleProps> = ({
  text,
  duration = 5000,
  onClose,
  position = 'top',
  maxWidth = 300,
  characterId,
  crossCharacter,
  listenerName,
}) => {
  const [visible, setVisible] = useState(false);
  const closedRef = useRef(false);
  const theme = getBubbleTheme(characterId);
  const { t } = useTranslation();

  useEffect(() => {
    setVisible(true);
    if (duration <= 0) return;
    const hideTimer = window.setTimeout(() => setVisible(false), duration);
    const closeTimer = window.setTimeout(() => {
      if (!closedRef.current) {
        closedRef.current = true;
        onClose?.();
      }
    }, duration + 250);
    return () => {
      window.clearTimeout(hideTimer);
      window.clearTimeout(closeTimer);
    };
  }, [duration, onClose]);

  // tailStyles 依赖 theme.tailColor，按 position 构建一份即可
  const tailStyle: React.CSSProperties = (() => {
    const base = { width: 0, height: 0 } as React.CSSProperties;
    switch (position) {
      case 'top':
        return { ...base, bottom: -6, left: 24, borderTop: `8px solid ${theme.tailColor}`, borderLeft: '8px solid transparent', borderRight: '8px solid transparent' };
      case 'bottom':
        return { ...base, top: -6, left: 24, borderBottom: `8px solid ${theme.tailColor}`, borderLeft: '8px solid transparent', borderRight: '8px solid transparent' };
      case 'left':
        return { ...base, right: -6, top: 18, borderLeft: `8px solid ${theme.tailColor}`, borderTop: '8px solid transparent', borderBottom: '8px solid transparent' };
      case 'right':
        return { ...base, left: -6, top: 18, borderRight: `8px solid ${theme.tailColor}`, borderTop: '8px solid transparent', borderBottom: '8px solid transparent' };
      default:
        return base;
    }
  })();

  // 跨角色模式：用角色色描边标记对话对象，和普通对用户气泡区分
  const crossStyle: React.CSSProperties = crossCharacter ? {
    border: `1.5px solid ${theme.crossBorderColor}`,
    boxShadow: '0 4px 16px rgba(0, 0, 0, 0.08)',
  } : {};

  return (
    <div
      style={{
        position: 'relative',
        maxWidth,
        opacity: visible ? 1 : 0,
        transform: visible ? 'translate(0, 0)' : INITIAL_TRANSFORM[position],
        transition:
          'opacity 0.25s ease, transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1)',
        background: theme.background,
        color: theme.textColor,
        borderRadius: 12,
        padding: '10px 14px',
        boxShadow: theme.shadow,
        backdropFilter: 'blur(10px)',
        WebkitBackdropFilter: 'blur(10px)',
        fontFamily: 'inherit',
        fontSize: 14,
        lineHeight: 1.55,
        wordBreak: 'break-word',
        whiteSpace: 'pre-wrap',
        pointerEvents: 'auto',
        ...crossStyle,
      }}
    >
      <span style={{ position: 'absolute', ...tailStyle }} />
      {crossCharacter && listenerName && (
        <span style={LISTENER_TAG_STYLE} aria-label={t('chat.cross_character_to', { name: listenerName })}>
          <span aria-hidden="true">↗</span> {t('chat.cross_character_to', { name: listenerName })}
        </span>
      )}
      <span style={{ display: 'block' }}>{stripActions(text)}</span>
      {/* 明确标出这是说给另一位角色的话，避免与对用户发言混淆 */}
    </div>
  );
};

// React.memo：settled 气泡在 active 气泡流式更新时无需重渲染
// （settled 气泡 text/duration/position/characterId 均稳定，onClose 未传）
export default React.memo(MessageBubble);
