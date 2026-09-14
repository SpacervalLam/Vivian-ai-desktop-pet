/**
 * 记忆巩固健康条 — 记忆图谱页顶部状态条（手账便签风）
 *
 * 数据源：invoke('get_memory_health', { characterId })
 * 展示记忆巩固流水线（pipeline / belief 两步）的步骤健康：
 * - 收起态：一行总览（✓ 正常 / ⚠ 连续失败 N 次 / 未启用 / 尚未运行）
 * - 展开态：每步的上次成功时间、连续失败计数、最近错误消息
 * 健康语义：连续失败 > 0 时巩固冷却退化为 30 分钟快速重试（故障自愈中）
 */

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Activity, ChevronDown, ChevronRight } from 'lucide-react';
import { SPACING, RADIUS } from '../design-system';

interface StepHealthView {
  last_success_at?: string | null;
  last_error_at?: string | null;
  last_error_msg?: string | null;
  fail_count: number;
  paused_reason?: string | null;
}

interface MemoryHealthView {
  enabled: boolean;
  healthy: boolean;
  paused_steps?: string[];
  steps: Record<string, StepHealthView>;
}

/** ISO 时间 → 本地短时间（MM-dd HH:mm），解析失败返回原文截断 */
function fmtTime(iso?: string | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso.slice(0, 16);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

const STEP_LABELS: Record<string, string> = {
  pipeline: '巩固流水线',
  belief: '信念生成',
};

const PAPER = {
  card: 'var(--graph-card)',
  ink: 'var(--graph-ink)',
  inkSoft: 'var(--graph-ink-soft)',
  inkFaint: 'var(--graph-ink-faint)',
  stampRed: 'var(--graph-stamp-red)',
  border: 'var(--graph-border)',
  line: 'var(--graph-line)',
};

const MemoryHealthStrip: React.FC<{ characterId: string }> = ({ characterId }) => {
  const [health, setHealth] = useState<MemoryHealthView | null>(null);
  const [expanded, setExpanded] = useState(false);

  const load = useCallback(async () => {
    try {
      const h = await invoke<MemoryHealthView>('get_memory_health', {
        characterId,
      });
      setHealth(h);
    } catch {
      setHealth(null);
    }
  }, [characterId]);

  useEffect(() => {
    void load();
    // 60s 轻轮询：巩固在后台（夜间/启动恢复）推进，健康状态会变化
    const id = setInterval(() => void load(), 60_000);
    return () => clearInterval(id);
  }, [load]);

  if (!health) return null;

  const steps = Object.entries(health.steps ?? {});
  const totalFails = steps.reduce((s, [, v]) => s + (v?.fail_count ?? 0), 0);
  const pausedCount = steps.filter(([, v]) => v?.paused_reason).length;

  // 总览文案
  let summaryText: string;
  let summaryColor: string;
  if (!health.enabled) {
    summaryText = '记忆巩固未启用';
    summaryColor = PAPER.inkFaint;
  } else if (steps.length === 0) {
    summaryText = '尚未运行（首次巩固后显示状态）';
    summaryColor = PAPER.inkFaint;
  } else if (pausedCount > 0) {
    summaryText = `熔断暂停 ${pausedCount} 步（连续失败，1 小时后半开重试）`;
    summaryColor = PAPER.stampRed;
  } else if (totalFails === 0) {
    summaryText = '运行正常';
    summaryColor = PAPER.ink;
  } else {
    summaryText = `自愈中 · 连续失败 ${totalFails} 次（30 分钟自动重试）`;
    summaryColor = PAPER.stampRed;
  }

  const lastSuccess = steps
    .map(([, v]) => v?.last_success_at)
    .filter(Boolean)
    .sort()
    .pop();

  return (
    <div
      style={{
        position: 'sticky',
        top: 2,
        zIndex: 3,
        margin: `${SPACING.sm}px ${SPACING.md}px 0`,
        borderRadius: RADIUS.sm,
        border: `1px dashed ${PAPER.line}`,
        background: PAPER.card,
        boxShadow: 'var(--graph-shadow-sm)',
        fontFamily: 'inherit',
        overflow: 'hidden',
      }}
    >
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        style={{
          width: '100%',
          display: 'flex',
          alignItems: 'center',
          gap: SPACING.sm,
          padding: `${SPACING.xs + 2}px ${SPACING.sm + 2}px`,
          border: 'none',
          background: 'transparent',
          cursor: 'pointer',
          color: PAPER.ink,
        }}
      >
        <Activity size={13} style={{ flexShrink: 0, color: summaryColor }} />
        <span style={{ fontSize: 12, fontWeight: 700, letterSpacing: 0.3 }}>记忆巩固</span>
        <span style={{ fontSize: 11.5, color: summaryColor, flex: 1, textAlign: 'left' }}>
          {summaryText}
        </span>
        {health.enabled && lastSuccess && (
          <span style={{ fontSize: 10.5, color: PAPER.inkFaint, flexShrink: 0 }}>
            上次成功 {fmtTime(lastSuccess)}
          </span>
        )}
        <span style={{ display: 'inline-flex', color: PAPER.inkSoft, flexShrink: 0 }}>
          {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </span>
      </button>
      {expanded && (
        <div style={{ borderTop: `1px dashed ${PAPER.line}`, padding: SPACING.sm }}>
          {steps.length === 0 ? (
            <div style={{ fontSize: 11.5, color: PAPER.inkFaint, padding: '2px 4px' }}>
              巩固在夜间（2-5 点）或长时间离场时运行；启动 45 秒后也会做一次恢复检查。
            </div>
          ) : (
            steps.map(([name, v]) => (
              <div
                key={name}
                style={{
                  display: 'flex',
                  alignItems: 'flex-start',
                  gap: SPACING.sm,
                  padding: '3px 4px',
                  fontSize: 11.5,
                  color: PAPER.ink,
                }}
              >
                <span
                  style={{
                    flexShrink: 0,
                    width: 7,
                    height: 7,
                    borderRadius: '50%',
                    marginTop: 4,
                    background:
                      (v?.fail_count ?? 0) > 0 ? PAPER.stampRed : 'var(--graph-ink-soft)',
                  }}
                />
                <span style={{ flexShrink: 0, fontWeight: 600, minWidth: 64 }}>
                  {STEP_LABELS[name] ?? name}
                </span>
                <span style={{ color: PAPER.inkSoft, flexShrink: 0, minWidth: 88 }}>
                  成功 {fmtTime(v?.last_success_at)}
                </span>
                {(v?.fail_count ?? 0) > 0 ? (
                  <span style={{ color: PAPER.stampRed, minWidth: 70 }}>
                    {v?.paused_reason ? '熔断暂停' : `失败 ×${v?.fail_count}`}
                  </span>
                ) : (
                  <span style={{ color: PAPER.inkFaint, minWidth: 70 }}>无失败</span>
                )}
                <span
                  style={{
                    color: v?.paused_reason ? PAPER.stampRed : PAPER.inkFaint,
                    flex: 1,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                  title={v?.paused_reason ?? v?.last_error_msg ?? ''}
                >
                  {v?.paused_reason ?? v?.last_error_msg ?? ''}
                </span>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
};

export default MemoryHealthStrip;
