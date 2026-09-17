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

/** create_plugin 请求的参数结构（预览卡片渲染用） */
interface CreatePluginArgs {
  name?: string;
  version?: string;
  description?: string;
  skills?: Array<{ filename?: string; content?: string }>;
  tools?: Array<{ name?: string; description?: string; script?: string }>;
  mcpServers?: Array<{ id?: string; name?: string; command?: string; args?: string[] }>;
  providers?: Array<{ id?: string }>;
}

/** 预览卡片字段标签样式 */
const previewLabel: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  color: 'var(--panel-text-tertiary)',
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

/**
 * 脚本/schema 等长文本的滚动预览样式。
 *
 * 底色必须走主题变量：原先写死 `rgba(44, 44, 44, 0.06)`（深灰 6%），
 * 在深色暖调主题的 #373028 底上几乎不可见，代码块看着像"没有背景"。
 * `--panel-bg-surface` 三套主题各自定义（深色为浅色 5% 叠加），才是对的做法。
 */
const previewCode: React.CSSProperties = {
  ...previewValue,
  fontFamily: 'Consolas, "Courier New", monospace',
  fontSize: 11,
  background: 'var(--panel-bg-surface)',
  borderRadius: 6,
  padding: '6px 8px',
  maxHeight: 150,
  overflowY: 'auto',
  whiteSpace: 'pre',
  wordBreak: 'break-all',
};

/** 无操作自动视为拒绝的倒计时秒数 */
const COUNTDOWN_SECONDS = 30;

/**
 * 风险等级配色。
 *
 * 取主题语义色而非固定 Material 色：设置面板是暖纸调，硬编码的
 * #2196F3 蓝 / #FF9800 橙 在深色暖底上很跳，且不随明暗主题变化。
 */
const riskAccent: Record<ConfirmRiskLevel, string> = {
  low: 'var(--panel-info)',
  medium: 'var(--panel-warning)',
  high: 'var(--panel-danger)',
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
  const isCreatePlugin = tool === 'create_plugin';
  const isCapabilityCreate = isCreateTool || isCreatePlugin;
  const createArgs: CreateToolArgs = isCreateTool && args && typeof args === 'object'
    ? (args as CreateToolArgs)
    : {};
  const pluginArgs: CreatePluginArgs = isCreatePlugin && args && typeof args === 'object'
    ? (args as CreatePluginArgs)
    : {};
  const argsText = isCapabilityCreate ? '' : formatArgs(args);
  const alwaysLabel = isCapabilityCreate
    ? t('tool_confirm.allow_session_create')
    : allowAlwaysScope === 'persistent'
      ? t('tool_confirm.trust_app')
      : t('tool_confirm.allow_session');
  const onceLabel = isCapabilityCreate ? t('tool_confirm.create_once') : t('tool_confirm.allow_once');

  const buttonBase: React.CSSProperties = {
    flex: 1,
    padding: '7px 0',
    borderRadius: 8,
    fontSize: 12.5,
    fontWeight: 600,
    fontFamily: 'inherit',
    cursor: 'pointer',
    // 1px 而非 1.5px：与卡片统一到细线层级，靠背景色区分三个按钮而非靠重边框
    border: '1px solid var(--panel-border-strong)',
    transition: 'opacity 0.15s ease, transform 0.1s ease',
  };

  return (
    <div
      style={{
        position: 'relative',
        overflow: 'hidden',
        transform: visible ? 'translateX(0) scale(1)' : 'translateX(28px) scale(0.94)',
        opacity: visible ? 1 : 0,
        transition:
          'transform 320ms cubic-bezier(0.16, 1, 0.3, 1), opacity 200ms ease',
        width: '100%',
        boxSizing: 'border-box',
        // 左侧多留 3px 给风险色条
        padding: '12px 14px 12px 17px',
        borderRadius: 12,
        background: 'var(--panel-elevated)',
        color: 'var(--panel-text)',
        border: '1px solid var(--panel-border)',
        // 与 Toast 同一套层次：外层投影 + 内顶高光
        boxShadow:
          'var(--panel-shadow-elevated), inset 0 1px 0 rgba(255, 255, 255, 0.05)',
        pointerEvents: 'auto',
        fontFamily: 'inherit',
        fontSize: 13,
        lineHeight: 1.55,
      }}
    >
      {/* 左侧风险色条：与 Toast 同一套视觉语言，等级一眼可辨 */}
      <span
        style={{
          position: 'absolute',
          left: 0,
          top: 0,
          bottom: 0,
          width: 3,
          background: accent,
        }}
      />
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
        <svg
          width={19}
          height={19}
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden
          style={{ color: accent, flexShrink: 0 }}
        >
          <circle cx={12} cy={12} r={10} fill="currentColor" opacity={0.16} />
          <path
            d="M9.6 9.5a2.5 2.5 0 1 1 3.4 2.3c-.8.3-1.2 1-1.2 1.8M11.8 16.6h.01"
            stroke="currentColor"
            strokeWidth={2.1}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
        <span style={{ fontWeight: 700, fontSize: 13 }}>
          {isCreatePlugin
            ? t('tool_confirm.create_plugin_title')
            : isCreateTool
              ? t('tool_confirm.create_title')
              : t('tool_confirm.title')}
        </span>
        {charId && (
          <span
            style={{
              padding: '1px 8px',
              borderRadius: 999,
              // 描边式而非实心：accent 在强制浅色主题下是 #2196F3，
              // 实心底配白字对比度不足；描边 + accent 文字在三套主题下都成立。
              background: 'var(--panel-bg-surface-elevated)',
              color: accent,
              border: `1px solid ${accent}`,
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
            color: 'var(--panel-text-tertiary)',
            fontVariantNumeric: 'tabular-nums',
          }}
        >
          {Math.max(remaining, 0)}s
        </span>
      </div>

      <div style={{ wordBreak: 'break-word', whiteSpace: 'pre-wrap', marginBottom: 4 }}>
        {reason}
      </div>
      <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginBottom: 8 }}>{tool}</div>
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
      ) : isCreatePlugin ? (
        <div style={{ marginBottom: 10 }}>
          <div style={previewLabel}>{t('tool_confirm.field_name')}</div>
          <div style={{ ...previewValue, fontFamily: 'Consolas, "Courier New", monospace' }}>
            {pluginArgs.name || '?'} <span style={{ color: 'var(--panel-text-tertiary)' }}>v{pluginArgs.version || ''}</span>
          </div>

          <div style={previewLabel}>{t('tool_confirm.field_description')}</div>
          <div style={previewValue}>{pluginArgs.description || ''}</div>

          <div style={previewLabel}>{t('tool_confirm.field_skills')}</div>
          <div style={previewValue}>
            {pluginArgs.skills && pluginArgs.skills.length > 0
              ? pluginArgs.skills.map((s) => s.filename).join('、')
              : t('tool_confirm.plugin_none')}
          </div>

          <div style={previewLabel}>{t('tool_confirm.field_tools')}</div>
          {pluginArgs.tools && pluginArgs.tools.length > 0 ? (
            pluginArgs.tools.map((toolDef, i) => (
              <div key={i} style={{ marginBottom: 6 }}>
                <div style={{ ...previewValue, fontWeight: 600, marginBottom: 0 }}>{toolDef.name}</div>
                <div style={previewCode}>{toolDef.script || ''}</div>
              </div>
            ))
          ) : (
            <div style={previewValue}>{t('tool_confirm.plugin_none')}</div>
          )}

          <div style={previewLabel}>{t('tool_confirm.field_mcp')}</div>
          {pluginArgs.mcpServers && pluginArgs.mcpServers.length > 0 ? (
            pluginArgs.mcpServers.map((m, i) => (
              <div key={i} style={previewCode}>
                {m.command}{m.args && m.args.length > 0 ? ` ${m.args.join(' ')}` : ''}
                {'  '}（{m.id}）
              </div>
            ))
          ) : (
            <div style={previewValue}>{t('tool_confirm.plugin_none')}</div>
          )}

          <div style={previewLabel}>{t('tool_confirm.field_providers')}</div>
          <div style={previewValue}>
            {pluginArgs.providers && pluginArgs.providers.length > 0
              ? pluginArgs.providers.map((p) => p.id).join('、')
              : t('tool_confirm.plugin_none')}
          </div>
        </div>
      ) : (
        argsText && (
          <div
            style={{
              fontSize: 11,
              lineHeight: 1.45,
              color: 'var(--panel-text-tertiary)',
              background: 'var(--panel-bg-surface)',
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
          background: 'var(--panel-bg-active)',
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
