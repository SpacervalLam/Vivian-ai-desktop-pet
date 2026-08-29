/**
 * 清空确认对话框（iOS 弹窗风格）
 *
 * 通用确认弹窗，用于清空记忆等危险操作的二次确认。
 */

import React from 'react';
import { useTranslation } from 'react-i18next';

const ClearConfirmDialog: React.FC<{
  open: boolean;
  loading: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  /** 覆盖默认标题（如恢复备份确认） */
  title?: string;
  /** 覆盖默认说明文案 */
  message?: string;
  /** 覆盖默认确认按钮文案 */
  confirmLabel?: string;
  /** 覆盖默认执行中按钮文案 */
  loadingLabel?: string;
}> = ({ open, loading, onConfirm, onCancel, title, message, confirmLabel, loadingLabel }) => {
  const { t } = useTranslation();
  if (!open) return null;
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--panel-overlay)',
        backdropFilter: 'blur(12px) saturate(120%)',
        WebkitBackdropFilter: 'blur(12px) saturate(120%)',
      }}
      onClick={onCancel}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 340,
          maxWidth: '90vw',
          borderRadius: 18,
          overflow: 'hidden',
          background: 'var(--panel-surface)',
          border: '1.5px solid var(--panel-border)',
          boxShadow: 'var(--panel-shadow-elevated)',
        }}
      >
        <div style={{ padding: '22px 22px 16px', textAlign: 'center' }}>
          <div
            style={{
              width: 44,
              height: 44,
              borderRadius: '50%',
              background: 'rgba(229, 57, 53, 0.10)',
              border: '1.5px solid rgba(229, 57, 53, 0.25)',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              marginBottom: 12,
            }}
          >
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
              <path
                d="M12 8v5M12 16.5v.5"
                stroke="#E53935"
                strokeWidth="2"
                strokeLinecap="round"
              />
              <circle cx="12" cy="12" r="9.5" stroke="#E53935" strokeWidth="1.6" opacity="0.6" />
            </svg>
          </div>
          <div style={{ fontSize: 16, fontWeight: 700, color: 'var(--panel-text)', marginBottom: 6 }}>
            {title ?? t('memory.clear_title')}
          </div>
          <div
            style={{
              fontSize: 13,
              lineHeight: 1.6,
              color: 'var(--panel-text-secondary)',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
            }}
          >
            {message ?? t('memory.clear_message')}
          </div>
        </div>
        <div style={{ height: 1, background: 'var(--panel-border)' }} />
        <div style={{ display: 'flex' }}>
          <button
            onClick={onCancel}
            disabled={loading}
            style={{
              flex: 1,
              padding: '13px 0',
              fontSize: 15,
              fontWeight: 500,
              color: 'var(--panel-text)',
              background: 'transparent',
              cursor: loading ? 'not-allowed' : 'pointer',
              opacity: loading ? 0.5 : 1,
              borderRight: '1.5px solid var(--panel-border)',
            }}
          >
            {t('memory.clear_btn_cancel')}
          </button>
          <button
            onClick={onConfirm}
            disabled={loading}
            style={{
              flex: 1,
              padding: '13px 0',
              fontSize: 15,
              fontWeight: 600,
              color: '#E53935',
              background: 'transparent',
              cursor: loading ? 'not-allowed' : 'pointer',
              opacity: loading ? 0.6 : 1,
            }}
          >
            {loading ? (loadingLabel ?? t('memory.clear_btn_clearing')) : (confirmLabel ?? t('memory.clear_btn_confirm'))}
          </button>
        </div>
      </div>
    </div>
  );
};

export default ClearConfirmDialog;
