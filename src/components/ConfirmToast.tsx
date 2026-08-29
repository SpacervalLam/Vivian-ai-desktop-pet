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
  /** 发起请求的角色 ID（用于卡片上标注是哪个智能体在请求） */
  charId?: string;
  /** 工具参数（渲染为参数明细，让用户看到具体操作对象） */
  args?: unknown;
  onDone: () => void;
}

/** 将参数对象格式化为多行 key: value 预览（单值截断，超限折叠） */
function formatArgs(args: unknown): string {
  if (!args || typeof args !== 'object' || Array.isArray(args)) return '';
  const entries = Object.entries(args as Record<string, unknown>);
  if (entries.length === 0) return '';
  const lines = entries.map(([k, v]) => {
    let val: string;
    if (v == null) val = String(v);
    else if (typeof v === 'string') val = v;
    else val = JSON.stringify(v);
    val = val.replace(/\s+/g, ' ').trim();
    if (val.length > 80) val = val.slice(0, 80) + '…';
    return `${k}: ${val}`;
  });
  const shown = lines.slice(0, 6).join('\n');
  return lines.length > 6 ? `${shown}\n… +${lines.length - 6} 项` : shown;
}

/** 角色 ID → 显示名（未收录的 ID 首字母大写兜底） */
function charName(id?: string): string {
  if (!id) return '';
  const map: Record<string, string> = { vivian: 'Vivian', nana: 'Nana' };
  return map[id] ?? id.charAt(0).toUpperCase() + id.slice(1);
}

/** create_tool 请求的参数结构（预览卡片渲染用） */
interface CreateToolArgs {
  name?: string;
  description?: string;
  parameters?: unknown;
  script?: string;
  deferred?: boolean;
}

/** 预览卡片字段标签样式 */
const previewLabel: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  color: '#8A8A8A',
  marginBottom: 2,
};

/** 预览卡片内容块样式 */
const previewValue: React.CSSProperties = {
  fontSize: 12,
  lineHeight: 1.5,
  color: 'var(--panel-text)',
  wordBreak: 'break-word',
  whiteSpace: 'pre-wrap',
  marginBottom: 8,
};

/** 脚本/schema 等长文本的滚动预览样式 */
const previewCode: React.CSSProperties = {
  ...previewValue,
  fontFamily: 'Consolas, "Courier New", monospace',
  fontSize: 11,
  background: 'rgba(44, 44, 44, 0.06)',
  borderRadius: 6,
  padding: '6px 8px',
  maxHeight: 150,
  overflowY: 'auto',
  whiteSpace: 'pre',
  wordBreak: 'break-all',
};

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
  charId,
  args,
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
  const isCreateTool = tool === 'create_tool';
  const createArgs: CreateToolArgs = isCreateTool && args && typeof args === 'object'
    ? (args as CreateToolArgs)
    : {};
  const argsText = isCreateTool ? '' : formatArgs(args);
  const alwaysLabel = isCreateTool
    ? t('tool_confirm.allow_session_create')
    : allowAlwaysScope === 'persistent'
      ? t('tool_confirm.trust_app')
      : t('tool_confirm.allow_session');
  const onceLabel = isCreateTool ? t('tool_confirm.create_once') : t('tool_confirm.allow_once');

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
        <span style={{ fontWeight: 700, fontSize: 13 }}>
          {isCreateTool ? t('tool_confirm.create_title') : t('tool_confirm.title')}
        </span>
        {charId && (
          <span
            style={{
              padding: '1px 8px',
              borderRadius: 999,
              background: accent,
              color: '#fff',
              fontSize: 11,
              fontWeight: 600,
              flexShrink: 0,
              whiteSpace: 'nowrap',
            }}
          >
            {charName(charId)}
          </span>
        )}
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
      <div style={{ fontSize: 11, color: '#8A8A8A', marginBottom: 8 }}>{tool}</div>
      {isCreateTool ? (
        <div style={{ marginBottom: 10 }}>
          <div style={previewLabel}>{t('tool_confirm.field_name')}</div>
          <div style={{ ...previewValue, fontFamily: 'Consolas, "Courier New", monospace' }}>
            {createArgs.name || '?'}
          </div>

          <div style={previewLabel}>{t('tool_confirm.field_description')}</div>
          <div style={previewValue}>{createArgs.description || ''}</div>

          <div style={previewLabel}>{t('tool_confirm.field_params')}</div>
          <div style={previewCode}>
            {createArgs.parameters && Object.keys(createArgs.parameters as object).length > 0
              ? JSON.stringify(createArgs.parameters, null, 2)
              : t('tool_confirm.no_params')}
          </div>

          <div style={previewLabel}>{t('tool_confirm.field_script')}</div>
          <div style={previewCode}>{createArgs.script || ''}</div>

          <div style={previewLabel}>{t('tool_confirm.field_risk')}</div>
          <div style={previewValue}>{t('tool_confirm.risk_shell')}</div>

          <div style={previewLabel}>{t('tool_confirm.field_injection')}</div>
          <div style={previewValue}>
            {createArgs.deferred
              ? t('tool_confirm.injection_deferred')
              : t('tool_confirm.injection_always')}
          </div>
        </div>
      ) : (
        argsText && (
          <div
            style={{
              fontSize: 11,
              lineHeight: 1.45,
              color: '#8A8A8A',
              background: 'rgba(44, 44, 44, 0.06)',
              borderRadius: 6,
              padding: '6px 8px',
              marginBottom: 10,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
              fontFamily: 'Consolas, "Courier New", monospace',
            }}
          >
            {argsText}
          </div>
        )
      )}

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
          {onceLabel}
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
