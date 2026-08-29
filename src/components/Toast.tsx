import React, { useEffect, useRef, useState } from 'react';

export type ToastType = 'info' | 'success' | 'error' | 'warning';

/** toast 附带的一键操作（如主题切换确认按钮），后端随 toast:show 下发 */
export interface ToastAction {
  kind: 'switch_theme';
  theme: 'light' | 'dark';
  /** 按钮文案（后端按界面语言本地化） */
  label: string;
}

export interface ToastProps {
  message: string;
  type?: ToastType;
  duration?: number;
  /** 0-100 进度百分比，提供时在 toast 底部渲染进度条 */
  progress?: number;
  /** 提供时渲染操作按钮，点击触发 onAction */
  action?: ToastAction;
  onAction?: () => void;
  onClose?: () => void;
}

const palette: Record<ToastType, { accent: string; icon: string }> = {
  info: { accent: '#2196F3', icon: 'ℹ' },
  success: { accent: '#4CAF50', icon: '✓' },
  error: { accent: '#E53935', icon: '✕' },
  warning: { accent: '#FF9800', icon: '!' },
};

/**
 * 单个 Toast 视觉单元。定位由父容器决定，组件本身只负责
 * 一条 toast 的外观、入场/出场动画与自动关闭计时。
 */
const Toast: React.FC<ToastProps> = ({
  message,
  type = 'info',
  duration = 3000,
  progress,
  action,
  onAction,
  onClose,
}) => {
  const [visible, setVisible] = useState(false);
  // onClose 存 ref：父组件每次渲染都传入新的箭头函数，若作为 effect 依赖会导致
  // 每次内容更新都重跑 effect——自动关闭计时器被反复重置，高频更新下的 toast 永不消失
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    setVisible(true);
    // duration <= 0 表示持久 toast（如后台任务进度）：不启动自动关闭计时器，
    // 由外部通过同 key 更新 duration>0 或直接移除来关闭。
    if (duration <= 0) return;
    const hideTimer = window.setTimeout(() => setVisible(false), duration);
    const closeTimer = window.setTimeout(() => onCloseRef.current?.(), duration + 250);
    return () => {
      window.clearTimeout(hideTimer);
      window.clearTimeout(closeTimer);
    };
  }, [duration]);

  const colors = palette[type];

  return (
    <div
      style={{
        position: 'relative',
        overflow: 'hidden',
        transform: visible ? 'translateX(0)' : 'translateX(120%)',
        opacity: visible ? 1 : 0,
        transition: 'transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1), opacity 0.25s ease',
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        minWidth: 220,
        maxWidth: 360,
        padding: '10px 14px',
        borderRadius: 10,
        background: 'var(--panel-surface)',
        color: 'var(--panel-text)',
        border: '1.5px solid var(--panel-border-strong)',
        boxShadow: 'var(--panel-shadow-elevated)',
        pointerEvents: 'auto',
        fontFamily: 'inherit',
        fontSize: 13,
        lineHeight: 1.5,
      }}
    >
      <span
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: 20,
          height: 20,
          borderRadius: '50%',
          background: colors.accent,
          color: '#fff',
          fontSize: 12,
          fontWeight: 700,
          flexShrink: 0,
        }}
      >
        {colors.icon}
      </span>
      <span style={{ flex: 1, wordBreak: 'break-word', whiteSpace: 'pre-wrap' }}>{message}</span>
      {action && onAction && (
        <button
          type="button"
          onClick={onAction}
          style={{
            flexShrink: 0,
            padding: '5px 12px',
            borderRadius: 8,
            fontSize: 12,
            fontWeight: 600,
            fontFamily: 'inherit',
            cursor: 'pointer',
            border: '1.5px solid var(--panel-border-strong)',
            background: 'var(--panel-selected-bg)',
            color: 'var(--panel-selected-text)',
            transition: 'opacity 0.15s ease',
          }}
        >
          {action.label}
        </button>
      )}
      {progress != null && progress >= 0 && (
        <div
          style={{
            position: 'absolute',
            bottom: 0,
            left: 0,
            height: 3,
            width: `${Math.min(100, Math.max(0, progress))}%`,
            background: colors.accent,
            borderRadius: '0 0 10px 10px',
            transition: 'width 0.3s ease',
          }}
        />
      )}
    </div>
  );
};

export default Toast;
