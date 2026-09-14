import React, { useCallback, useEffect, useState } from 'react';

/**
 * 通用确认对话框
 *
 * 用法一（受控）：
 *   <ConfirmDialog open={open} title="确认" message="..." onConfirm={...} onCancel={...} />
 *
 * 用法二（命令式 Promise）：
 *   const ok = await ConfirmDialog.confirm({ title: '确认', message: '...' });
 *   if (!ok) return;
 */

export type ConfirmIconType = 'question' | 'success' | 'error' | 'warning' | 'info';

export interface ConfirmDialogProps {
  /** 是否显示 */
  open?: boolean;
  /** 标题 */
  title?: string;
  /** 消息内容 */
  message?: React.ReactNode;
  /** 确认按钮文本 */
  confirmText?: string;
  /** 取消按钮文本（为空字符串则隐藏取消按钮） */
  cancelText?: string;
  /** 图标类型，决定图标与确认按钮主色 */
  iconType?: ConfirmIconType;
  /** 确认按钮是否处于 loading 状态（异步操作期间禁用按钮） */
  loading?: boolean;
  /** 是否允许点击遮罩关闭（默认 true） */
  dismissOnOverlayClick?: boolean;
  /** 确认回调 */
  onConfirm?: () => void;
  /** 取消/关闭回调 */
  onCancel?: () => void;
}

// 图标与主色映射
const ICON_META: Record<ConfirmIconType, { icon: string; color: string }> = {
  question: { icon: '❓', color: 'var(--accent)' },
  info: { icon: 'ℹ️', color: 'var(--accent)' },
  success: { icon: '✅', color: 'var(--success)' },
  error: { icon: '⛔', color: 'var(--error)' },
  warning: { icon: '⚠️', color: '#ff9f1c' },
};

const ConfirmDialogComponent: React.FC<ConfirmDialogProps> = ({
  open = true,
  title = '确认',
  message = '确定要执行此操作吗？',
  confirmText = '确定',
  cancelText = '取消',
  iconType = 'question',
  loading = false,
  dismissOnOverlayClick = true,
  onConfirm,
  onCancel,
}) => {
  const [show, setShow] = useState(open);

  useEffect(() => {
    setShow(open);
  }, [open]);

  // Escape 键关闭
  useEffect(() => {
    if (!show) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !loading) {
        e.preventDefault();
        handleCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [show, loading]);

  const handleConfirm = useCallback(() => {
    if (loading) return;
    onConfirm?.();
  }, [loading, onConfirm]);

  const handleCancel = useCallback(() => {
    if (loading) return;
    setShow(false);
    // 关闭动画结束后再触发回调
    window.setTimeout(() => onCancel?.(), 160);
  }, [loading, onCancel]);

  if (!show) return null;

  const meta = ICON_META[iconType] ?? ICON_META.question;

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 10000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0,0,0,0.55)',
        backdropFilter: 'blur(4px)',
        WebkitBackdropFilter: 'blur(4px)',
        animation: 'vivian-confirm-fade 0.18s ease',
      }}
      onClick={() => dismissOnOverlayClick && !loading && handleCancel()}
    >
      <style>{`
        @keyframes vivian-confirm-fade {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes vivian-confirm-rise {
          from { opacity: 0; transform: translateY(10px) scale(0.96); }
          to { opacity: 1; transform: translateY(0) scale(1); }
        }
      `}</style>
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 380,
          maxWidth: '90vw',
          padding: '24px 24px 20px',
          borderRadius: 16,
          background: 'var(--bg-overlay)',
          backdropFilter: 'blur(20px) saturate(180%)',
          WebkitBackdropFilter: 'blur(20px) saturate(180%)',
          border: '1px solid var(--separator)',
          boxShadow: '0 16px 48px rgba(0, 0, 0, 0.36)',
          fontFamily: 'inherit',
          animation: 'vivian-confirm-rise 0.22s cubic-bezier(0.2, 0.8, 0.2, 1)',
        }}
      >
        {/* 标题栏（可拖动） */}
        <div
          data-tauri-drag-region
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            marginBottom: 14,
          }}
        >
          <span style={{ fontSize: 22, lineHeight: 1 }}>{meta.icon}</span>
          <span
            style={{
              fontSize: 16,
              fontWeight: 600,
              color: 'var(--text-primary)',
              flex: 1,
            }}
          >
            {title}
          </span>
        </div>

        {/* 消息内容 */}
        <div
          style={{
            fontSize: 13,
            lineHeight: 1.7,
            color: 'var(--text-secondary)',
            marginBottom: 20,
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
          }}
        >
          {message}
        </div>

        {/* 按钮区 */}
        <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end' }}>
          {cancelText && (
            <button
              onClick={handleCancel}
              disabled={loading}
              style={{
                padding: '8px 20px',
                borderRadius: 8,
                fontSize: 13,
                color: 'var(--text-secondary)',
                border: '1px solid var(--separator)',
                background: 'transparent',
                cursor: loading ? 'not-allowed' : 'pointer',
                opacity: loading ? 0.6 : 1,
                fontFamily: 'inherit',
              }}
            >
              {cancelText}
            </button>
          )}
          <button
            onClick={handleConfirm}
            disabled={loading}
            style={{
              padding: '8px 20px',
              borderRadius: 8,
              fontSize: 13,
              fontWeight: 600,
              color: '#fff',
              border: 'none',
              background: loading ? 'var(--separator)' : meta.color,
              cursor: loading ? 'not-allowed' : 'pointer',
              opacity: loading ? 0.7 : 1,
              fontFamily: 'inherit',
              transition: 'opacity 0.2s ease',
            }}
          >
            {loading ? '处理中…' : confirmText}
          </button>
        </div>
      </div>
    </div>
  );
};

// ============ 命令式调用支持 ============

interface ConfirmOptions extends Omit<ConfirmDialogProps, 'open' | 'onConfirm' | 'onCancel'> {}

/** 命令式调用：返回 Promise<boolean>，true 表示用户确认 */
const confirm = (options: ConfirmOptions): Promise<boolean> => {
  return new Promise((resolve) => {
    const holder: { unmount?: () => void } = {};

    const container = document.createElement('div');
    document.body.appendChild(container);

    const cleanup = () => {
      holder.unmount?.();
      // 延迟移除容器以等待关闭动画
      window.setTimeout(() => {
        if (container.parentNode) container.parentNode.removeChild(container);
      }, 220);
    };

    const handleConfirm = () => {
      resolve(true);
      cleanup();
    };

    const handleCancel = () => {
      resolve(false);
      cleanup();
    };

    // 动态导入 React 以渲染组件
    void import('react-dom/client').then(({ createRoot }) => {
      const root = createRoot(container);
      holder.unmount = () => root.unmount();
      root.render(
        <ConfirmDialogComponent
          open
          onConfirm={handleConfirm}
          onCancel={handleCancel}
          {...options}
        />
      );
    });
  });
};

// 通过 Object.assign 合并组件与静态方法，使 `ConfirmDialog.confirm(...)` 可用
const ConfirmDialog = Object.assign(ConfirmDialogComponent, { confirm });

export default ConfirmDialog;
