import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

export type ConfirmRiskLevel = 'low' | 'medium' | 'high';
export type AllowAlwaysScope = 'persistent' | 'session';

export type ConfirmAction = 'deny' | 'allow_once' | 'allow_always';

export interface ConfirmToastProps {
  requestId: number;
  tool: string;
  reason: string;
  riskLevel: ConfirmRiskLevel;
  allowAlwaysScope: AllowAlwaysScope;
  onDone: () => void;
}

/** 无操作自动视为拒绝的倒计时秒数 */
const COUNTDOWN_SECONDS = 30;

const riskAccent: Record<ConfirmRiskLevel, string> = {
  low: '#2196F3',
  medium: '#FF9800',
  high: '#E53935',
};

/**
 * 工具执行确认卡片：拒绝 / 放行一次 / 始终允许（信任应用或本次运行允许）三按钮，
 * COUNTDOWN_SECONDS 秒内无操作自动按拒绝处理，回传后通知父级移除自身。
 */
const ConfirmToast: React.FC<ConfirmToastProps> = ({
  requestId,
  tool,
  reason,
  riskLevel,
  allowAlwaysScope,
  onDone,
}) => {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const [remaining, setRemaining] = useState(COUNTDOWN_SECONDS);
  const respondedRef = useRef(false);

  const respond = useCallback(
    (action: ConfirmAction) => {
      if (respondedRef.current) return;
      respondedRef.current = true;
      void invoke('confirm_tool_execution', { requestId, action }).catch((err) => {
        console.warn('[ConfirmToast] 确认结果回传失败:', err);
      });
      void emit('toast:confirm_done', { request_id: requestId });
      setVisible(false);
      window.setTimeout(onDone, 250);
    },
    [requestId, onDone],
  );

  useEffect(() => {
    setVisible(true);
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => setRemaining((r) => r - 1), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (remaining <= 0) respond('deny');
  }, [remaining, respond]);

  const accent = riskAccent[riskLevel];
  const alwaysLabel =
    allowAlwaysScope === 'persistent'
      ? t('tool_confirm.trust_app')
      : t('tool_confirm.allow_session');

  const buttonBase: React.CSSProperties = {
    flex: 1,
    padding: '7px 0',
    borderRadius: 8,
    fontSize: 12.5,
    fontWeight: 600,
    fontFamily: 'inherit',
    cursor: 'pointer',
    border: '1.5px solid var(--panel-border-strong)',
    transition: 'opacity 0.15s ease, transform 0.1s ease',
  };

  return (
    <div
      style={{
        transform: visible ? 'translateX(0)' : 'translateX(120%)',
        opacity: visible ? 1 : 0,
        transition: 'transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1), opacity 0.25s ease',
        width: '100%',
        boxSizing: 'border-box',
        padding: '12px 14px',
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
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: 20,
            height: 20,
            borderRadius: '50%',
            background: accent,
            color: '#fff',
            fontSize: 12,
            fontWeight: 700,
            flexShrink: 0,
          }}
        >
          ?
        </span>
        <span style={{ fontWeight: 700, fontSize: 13 }}>{t('tool_confirm.title')}</span>
        <span
          style={{
            marginLeft: 'auto',
            fontSize: 11,
            color: '#8A8A8A',
            fontVariantNumeric: 'tabular-nums',
          }}
        >
          {Math.max(remaining, 0)}s
        </span>
      </div>

      <div style={{ wordBreak: 'break-word', whiteSpace: 'pre-wrap', marginBottom: 4 }}>
        {reason}
      </div>
      <div style={{ fontSize: 11, color: '#8A8A8A', marginBottom: 10 }}>{tool}</div>

      <div
        style={{
          height: 3,
          borderRadius: 2,
          background: 'rgba(44, 44, 44, 0.12)',
          overflow: 'hidden',
          marginBottom: 10,
        }}
      >
        <div
          style={{
            height: '100%',
            width: `${(Math.max(remaining, 0) / COUNTDOWN_SECONDS) * 100}%`,
            background: accent,
            transition: 'width 1s linear',
          }}
        />
      </div>

      <div style={{ display: 'flex', gap: 8 }}>
        <button
          type="button"
          style={{ ...buttonBase, background: 'transparent', color: 'var(--panel-text)' }}
          onClick={() => respond('deny')}
        >
          {t('tool_confirm.deny')}
        </button>
        <button
          type="button"
          style={{ ...buttonBase, background: 'var(--panel-bg-active)', color: 'var(--panel-text)' }}
          onClick={() => respond('allow_once')}
        >
          {t('tool_confirm.allow_once')}
        </button>
        <button
          type="button"
          style={{ ...buttonBase, background: 'var(--panel-selected-bg)', color: 'var(--panel-selected-text)' }}
          onClick={() => respond('allow_always')}
        >
          {alwaysLabel}
        </button>
      </div>
    </div>
  );
};

export default ConfirmToast;
