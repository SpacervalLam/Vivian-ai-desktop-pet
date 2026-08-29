/**
 * Mind Inspector 共享组件库
 *
 * 基于 iOS 设计系统实现的纯展示组件。所有视觉常量从 design-system.ts 导入，
 * 全部使用内联样式。每个组件均使用 React.memo 优化并支持可选 style 覆盖。
 */

import React, { useState } from 'react';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
  SHADOW,
} from './design-system';

type CSSProperties = React.CSSProperties;

// === 关键帧注入（一次性，用于脉冲、旋转与流动动画） ===
const KEYFRAMES_ID = 'mind-inspector-keyframes';
if (typeof document !== 'undefined' && !document.getElementById(KEYFRAMES_ID)) {
  const style = document.createElement('style');
  style.id = KEYFRAMES_ID;
  style.textContent = `
@keyframes mind-inspector-pulse {
  0% { transform: scale(1); opacity: 0.5; }
  100% { transform: scale(2.6); opacity: 0; }
}
@keyframes mind-inspector-spin {
  to { transform: rotate(360deg); }
}
@keyframes mind-inspector-flow {
  0% { stroke-dashoffset: 20; }
  100% { stroke-dashoffset: 0; }
}`;
  document.head.appendChild(style);
}

// === 角色名翻译辅助 ===
const CHAR_IDS = ['vivian', 'nana'] as const;

/**
 * 将后端返回的角色 ID（'vivian'/'nana'）翻译为记忆面板专用显示名。
 * 其他值（'system'/'user'/'all'/自定义字符串）原样返回。
 */
export function charLabel(
  id: string | undefined | null,
  t: (key: string) => string,
): string {
  if (!id) return '—';
  if ((CHAR_IDS as readonly string[]).includes(id)) {
    return t(`mind_inspector.common.char_${id}`);
  }
  return id;
}

// ============================================================
// 1. Card — 基础卡片（iOS 风格：磨砂玻璃 + continuous corners）
// ============================================================
export interface CardProps {
  /** 开启 hover 效果（上浮 + 边框高亮 + 阴影增强） */
  hover?: boolean;
  /** 使用提升的卡片背景 */
  elevated?: boolean;
  style?: CSSProperties;
  children?: React.ReactNode;
  onClick?: () => void;
  /** 额外的鼠标进入回调（不会覆盖内部 hover 状态） */
  onMouseEnterExternal?: () => void;
  /** 额外的鼠标离开回调（不会覆盖内部 hover 状态） */
  onMouseLeaveExternal?: () => void;
}

const CardBase: React.FC<CardProps> = ({
  hover,
  elevated,
  style,
  children,
  onClick,
  onMouseEnterExternal,
  onMouseLeaveExternal,
}) => {
  const [hovered, setHovered] = useState(false);
  const isHover = hover && hovered;
  return (
    <div
      role={onClick ? 'button' : undefined}
      onClick={onClick}
      onMouseEnter={() => {
        setHovered(true);
        onMouseEnterExternal?.();
      }}
      onMouseLeave={() => {
        setHovered(false);
        onMouseLeaveExternal?.();
      }}
      style={{
        position: 'relative',
        borderRadius: RADIUS.lg,
        overflow: 'hidden',
        // 手账纸卡质感（清新暖纸，无磨砂玻璃）
        background: elevated
          ? 'var(--panel-elevated)'
          : 'var(--panel-surface)',
        border: `1px solid ${isHover ? 'var(--panel-border-hover)' : 'var(--panel-border-light)'}`,
        padding: SPACING.cardPadding,
        transform: isHover ? 'translateY(-2px) rotate(-0.2deg)' : 'translateY(0) rotate(0)',
        boxShadow: isHover
          ? 'var(--panel-shadow-card)'
          : elevated
            ? 'var(--panel-shadow-card)'
            : 'var(--panel-shadow-subtle)',
        transition: `transform ${DURATION.normal}s ${EASE.ios}, border-color ${DURATION.normal}s ${EASE.swift}, background ${DURATION.normal}s ${EASE.swift}, box-shadow ${DURATION.normal}s ${EASE.ios}`,
        cursor: onClick ? 'pointer' : 'default',
        ...style,
      }}>
      {/* 手账纸卡顶部细高光（低饱和纸缘） */}
      <div
        aria-hidden
        style={{
          position: 'absolute',
          top: 0,
          left: RADIUS.lg,
          right: RADIUS.lg,
          height: 1,
          background:
            'linear-gradient(90deg, transparent 0%, var(--panel-border-light) 50%, transparent 100%)',
          borderRadius: 1,
          pointerEvents: 'none',
        }}
      />
      {children}
    </div>
  );
};

export const Card = React.memo(CardBase);

// ============================================================
// 2. HeroCard — 大号卡片（清新手账：暖纸 + 顶部纸胶带彩条）
// ============================================================
export interface HeroCardProps {
  title?: React.ReactNode;
  subtitle?: React.ReactNode;
  children?: React.ReactNode;
  style?: CSSProperties;
}

const HeroCardBase: React.FC<HeroCardProps> = ({
  title,
  subtitle,
  children,
  style,
}) => {
  const hasBody = !!children;
  const hasSubtitle = !!subtitle;
  return (
    <div
      style={{
        position: 'relative',
        borderRadius: RADIUS.xl,
        background: 'var(--panel-elevated)',
        border: '1px solid var(--panel-border)',
        padding: SPACING.lg,
        overflow: 'hidden',
        boxShadow: 'var(--panel-shadow-card)',
        ...style,
      }}
    >
      {/* 顶部纸胶带彩条（低饱和双色斜贴） */}
      <div
        aria-hidden
        style={{
          position: 'absolute',
          top: -5,
          left: '50%',
          width: 120,
          height: 14,
          transform: 'translateX(-50%) rotate(-1.5deg)',
          background: 'linear-gradient(90deg, var(--sticker-pink-soft), var(--sticker-sky-soft))',
          borderRadius: 3,
          boxShadow: 'var(--panel-shadow-subtle)',
          pointerEvents: 'none',
        }}
      />
      {/* 右上角纸角折痕点缀 */}
      <div
        aria-hidden
        style={{
          position: 'absolute',
          top: 0,
          right: 0,
          width: 22,
          height: 22,
          background:
            'linear-gradient(225deg, var(--sticker-butter-soft) 0 50%, transparent 50% 100%)',
          borderBottomLeftRadius: RADIUS.sm,
          pointerEvents: 'none',
        }}
      />
      {title && (
        <div
          style={{
            ...TYPO.h1,
            color: 'var(--panel-text)',
            marginBottom: hasSubtitle || hasBody ? SPACING.xs : 0,
            position: 'relative',
          }}
        >
          {title}
        </div>
      )}
      {hasSubtitle && (
        <div
          style={{
            ...TYPO.body,
            color: 'var(--panel-text-secondary)',
            marginBottom: hasBody ? SPACING.md : 0,
            position: 'relative',
          }}
        >
          {subtitle}
        </div>
      )}
      {children}
    </div>
  );
};

export const HeroCard = React.memo(HeroCardBase);

// ============================================================
// 3. MetricBar — 指标条（iOS 风格：圆角胶囊 + 渐变填充）
// ============================================================
export interface MetricBarProps {
  /** 0-1 之间的值 */
  value: number;
  color?: string;
  trackColor?: string;
  style?: CSSProperties;
}

const MetricBarBase: React.FC<MetricBarProps> = ({
  value,
  color,
  trackColor,
  style,
}) => {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  const fillColor = color ?? 'var(--panel-accent)';
  return (
    <div
      style={{
        position: 'relative',
        height: 5,
        borderRadius: RADIUS.pill,
        background: trackColor ?? 'var(--panel-bg-hover)',
        overflow: 'hidden',
        ...style,
      }}
    >
      <div
        style={{
          width: `${pct}%`,
          height: '100%',
          borderRadius: RADIUS.pill,
          background: fillColor,
          boxShadow: `0 0 8px ${fillColor}66`,
          transition: `width ${DURATION.slow}s ${EASE.ios}`,
        }}
      />
    </div>
  );
};

export const MetricBar = React.memo(MetricBarBase);

// ============================================================
// 4. StatusDot — 状态圆点（可选脉冲）
// ============================================================
export interface StatusDotProps {
  color?: string;
  pulse?: boolean;
  style?: CSSProperties;
}

const StatusDotBase: React.FC<StatusDotProps> = ({ color, pulse, style }) => {
  const dotColor = color ?? 'var(--panel-accent)';
  return (
    <span
      style={{
        position: 'relative',
        display: 'inline-block',
        width: 7,
        height: 7,
        borderRadius: RADIUS.pill,
        background: dotColor,
        boxShadow: `0 0 6px ${dotColor}80`,
        ...style,
      }}
    >
      {pulse && (
        <span
          style={{
            position: 'absolute',
            inset: 0,
            borderRadius: RADIUS.pill,
            background: dotColor,
            opacity: 0.5,
            animation: `mind-inspector-pulse ${DURATION.slow * 2}s ${EASE.swift} infinite`,
          }}
        />
      )}
    </span>
  );
};

export const StatusDot = React.memo(StatusDotBase);

// ============================================================
// 6. EmptyState — 空状态（iOS 风格：圆角磨砂占位）
// ============================================================
export interface EmptyStateProps {
  icon?: React.ReactNode;
  text?: React.ReactNode;
  spinner?: boolean;
  style?: CSSProperties;
}

const EmptyStateBase: React.FC<EmptyStateProps> = ({
  icon,
  text,
  spinner,
  style,
}) => {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: SPACING.sm,
        padding: `${SPACING.xl}px ${SPACING.md}px`,
        color: 'var(--panel-text-tertiary)',
        ...style,
      }}
    >
      {icon && (
        <div
          style={{
            opacity: 0.5,
            padding: SPACING.md,
            borderRadius: RADIUS.lg,
            background: 'var(--panel-surface)',
            border: '1px solid var(--panel-border)',
          }}
        >
          {icon}
        </div>
      )}
      {spinner && (
        <div
          style={{
            width: 20,
            height: 20,
            border: '2px solid var(--panel-border-hover)',
            borderTopColor: 'var(--panel-accent)',
            borderRadius: RADIUS.pill,
            animation: 'mind-inspector-spin 0.8s linear infinite',
          }}
        />
      )}
      {text && (
        <div style={{ ...TYPO.body, color: 'var(--panel-text-tertiary)' }}>{text}</div>
      )}
    </div>
  );
};

export const EmptyState = React.memo(EmptyStateBase);

// ============================================================
// 7. SectionTitle — 区块标题（手账铅笔线与毛边标签）
// ============================================================
export interface SectionTitleProps {
  children?: React.ReactNode;
  style?: CSSProperties;
}

const SectionTitleBase: React.FC<SectionTitleProps> = ({ children, style }) => {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        ...TYPO.caption,
        color: 'var(--panel-accent)',
        ...style,
      }}
    >
      <span
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          padding: '2px 9px',
          borderRadius: 6,
          background: 'var(--sticker-sky-soft)',
          color: 'var(--panel-accent-bright)',
        }}
      >
        {children}
      </span>
      {/* 铅笔延长线 */}
      <span
        style={{
          flex: 1,
          height: 1,
          minWidth: 20,
          background: 'repeating-linear-gradient(90deg, var(--panel-border) 0 5px, transparent 5px 9px)',
          opacity: 0.7,
        }}
      />
    </div>
  );
};

export const SectionTitle = React.memo(SectionTitleBase);

// ============================================================
// 8. Tag — 标签胶囊（iOS Capsule 风格）
// ============================================================
export interface TagProps {
  children?: React.ReactNode;
  color?: string;
  style?: CSSProperties;
}

const TagBase: React.FC<TagProps> = ({ children, color, style }) => {
  const tagColor = color ?? 'var(--panel-accent)';
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        borderRadius: RADIUS.pill,
        padding: `3px ${SPACING.sm + 2}px`,
        ...TYPO.micro,
        fontWeight: 600,
        color: tagColor,
        background: `${tagColor}1A`,
        border: `1px solid ${tagColor}33`,
        whiteSpace: 'nowrap',
        ...style,
      }}
    >
      {children}
    </span>
  );
};

export const Tag = React.memo(TagBase);

// ============================================================
// 9. IconButton — 图标按钮（iOS 按压反馈）
// ============================================================
export interface IconButtonProps {
  children?: React.ReactNode;
  onClick?: () => void;
  title?: string;
  active?: boolean;
  disabled?: boolean;
  style?: CSSProperties;
}

const IconButtonBase: React.FC<IconButtonProps> = ({
  children,
  onClick,
  title,
  active,
  disabled,
  style,
}) => {
  const [hovered, setHovered] = useState(false);
  const [pressed, setPressed] = useState(false);
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      disabled={disabled}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => {
        setHovered(false);
        setPressed(false);
      }}
      onMouseDown={() => setPressed(true)}
      onMouseUp={() => setPressed(false)}
      style={{
        width: 32,
        height: 32,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 0,
        border: 'none',
        borderRadius: RADIUS.sm,
        background: active
          ? 'var(--panel-accent-muted)'
          : hovered && !disabled
            ? 'var(--panel-bg-hover)'
            : 'transparent',
        color: active ? 'var(--panel-accent-bright)' : 'var(--panel-text-secondary)',
        cursor: disabled ? 'not-allowed' : onClick ? 'pointer' : 'default',
        opacity: disabled ? 0.4 : 1,
        transition: `background ${DURATION.fast}s ${EASE.swift}, color ${DURATION.fast}s ${EASE.swift}, transform ${DURATION.fast}s ${EASE.spring}, opacity ${DURATION.fast}s ${EASE.swift}`,
        fontFamily: TYPO.fontFamily,
        transform: pressed && !disabled ? 'scale(0.92)' : 'scale(1)',
        ...style,
      }}
    >
      {children}
    </button>
  );
};

export const IconButton = React.memo(IconButtonBase);

// ============================================================
// 10. TwinView — 双角色并排布局容器
// ============================================================
export interface TwinViewProps {
  left: React.ReactNode;
  right: React.ReactNode;
  style?: CSSProperties;
}

const TwinViewBase: React.FC<TwinViewProps> = ({ left, right, style }) => {
  const panelStyle: CSSProperties = {
    flex: 1,
    minWidth: 0,
    overflowY: 'auto',
    paddingRight: SPACING.md,
  };
  return (
    <div
      style={{
        display: 'flex',
        gap: SPACING.cardGap,
        ...style,
      }}
    >
      <div style={panelStyle}>{left}</div>
      {/* 中间镜像轴（Vertical Mirror Axis） */}
      <div
        style={{
          width: 1,
          minWidth: 1,
          flexShrink: 0,
          margin: `${SPACING.xl}px 0`,
          background: 'linear-gradient(180deg, transparent 0%, var(--panel-border-light) 15%, var(--panel-border-light) 85%, transparent 100%)',
          borderRadius: RADIUS.pill,
        }}
      />
      <div style={panelStyle}>{right}</div>
    </div>
  );
};

export const TwinView = React.memo(TwinViewBase);
