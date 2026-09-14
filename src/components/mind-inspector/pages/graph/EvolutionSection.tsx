/**
 * 角色成长记录区块（记忆图谱页底部）
 *
 * 数据源: invoke('get_persona_evolution', { characterId })
 * 展示人格自进化覆盖层（persona/evolution.rs）的两部分：
 * - entries:    已生效的自我调整（跨轨迹支持门槛 ≥2、最小间隔 6h 晋升）
 * - candidates: 酝酿中的候选（尚未达门槛，未注入 prompt）
 *
 * 纯展示组件：不改写任何数据；加载失败静默降级为空态。
 */

import React, { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { RefreshCw, Sparkles } from 'lucide-react';
import { TYPO, SPACING, RADIUS } from '../../design-system';
import { EmptyState } from '../../shared-components';
import type { CharacterId } from './types';

// 手账纸张视觉（与 GraphPage 的 GPAPER 契约保持同一组 CSS 变量）
const GPAPER = {
  paper: 'var(--graph-paper)',
  card: 'var(--graph-card)',
  ink: 'var(--graph-ink)',
  inkSoft: 'var(--graph-ink-soft)',
  inkFaint: 'var(--graph-ink-faint)',
  stampRed: 'var(--graph-stamp-red)',
  shadowSm: 'var(--graph-shadow-sm)',
  shadowMd: 'var(--graph-shadow-md)',
  border: 'var(--graph-border)',
} as const;

const HAND =
  '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "PingFang SC", "Microsoft YaHei", serif';

/** 已生效的自我调整（Rust EvolutionEntry，timestamp 为秒） */
interface EvolutionEntry {
  timestamp: number;
  kind: string;
  text: string;
  reason: string;
  support: number;
}

/** 酝酿中的候选调整（Rust EvolutionCandidate，first_seen 为秒） */
interface EvolutionCandidate {
  kind: string;
  text: string;
  reason: string;
  first_seen: number;
  support: number;
}

interface EvolutionPayload {
  entries: EvolutionEntry[];
  candidates: EvolutionCandidate[];
  is_empty: boolean;
  last_update: number;
}

/** 类别 → 贴纸底色（语气=天蓝 / 性格=淡紫） */
const KIND_BG: Record<string, string> = {
  tone: 'rgba(124, 168, 218, 0.32)',
  personality: 'rgba(160, 132, 208, 0.30)',
};

/** 由 id 派生确定性轻微旋转角，营造随手贴纸感（与 GraphPage 同款） */
const stickerTilt = (id: string): number => {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return ((Math.abs(h) % 100) / 100 - 0.5) * 3.2;
};

/** 秒级时间戳 → MM-DD 日期戳 */
const dateStamp = (tsSec: number): string => {
  const d = new Date(tsSec * 1000);
  if (Number.isNaN(d.getTime())) return '';
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};

/** 单条调整贴纸（正式 / 候选共用行布局，候选走虚线弱化态） */
const GrowthRow: React.FC<{
  id: string;
  kind: string;
  text: string;
  reason: string;
  tsSec: number;
  support: number;
  pending: boolean;
  requiredSupport: number;
}> = ({ id, kind, text, reason, tsSec, support, pending, requiredSupport }) => {
  const { t } = useTranslation();
  const kindLabel =
    kind === 'personality'
      ? t('mind_inspector.graph.growth_kind_personality')
      : t('mind_inspector.graph.growth_kind_tone');
  const bg = KIND_BG[kind] ?? 'rgba(150, 150, 150, 0.22)';
  const date = dateStamp(tsSec);

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'flex-start',
        gap: SPACING.sm,
        padding: '10px 12px',
        borderRadius: RADIUS.lg,
        background: GPAPER.card,
        border: pending ? `1.5px dashed ${GPAPER.border}` : `1.5px solid ${GPAPER.border}`,
        boxShadow: pending ? 'none' : GPAPER.shadowSm,
        transform: `rotate(${stickerTilt(id)}deg)`,
      }}
    >
      {/* 日期戳 */}
      {date && (
        <span
          style={{
            flexShrink: 0,
            fontFamily: HAND,
            fontSize: 13,
            color: GPAPER.inkFaint,
            transform: 'rotate(-3deg)',
            marginTop: 2,
            letterSpacing: 0.5,
          }}
        >
          {date}
        </span>
      )}

      <div style={{ flex: 1, minWidth: 0 }}>
        {/* 类别 + 印证数 */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 4, flexWrap: 'wrap' }}>
          <span
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              padding: '1px 8px',
              fontSize: 11,
              fontWeight: 600,
              color: GPAPER.ink,
              background: bg,
              borderRadius: 3,
            }}
          >
            {kindLabel}
          </span>
          {pending ? (
            <span
              style={{
                fontSize: 10.5,
                color: GPAPER.inkFaint,
                border: `1px dashed ${GPAPER.border}`,
                borderRadius: 999,
                padding: '1px 7px',
              }}
            >
              {t('mind_inspector.graph.growth_support_progress', {
                count: support,
                total: requiredSupport,
              })}
            </span>
          ) : (
            support >= 2 && (
              <span
                style={{
                  fontSize: 10.5,
                  fontWeight: 600,
                  color: '#FFF9EE',
                  background: GPAPER.stampRed,
                  borderRadius: 3,
                  padding: '1px 7px',
                  transform: 'rotate(-2deg)',
                  opacity: 0.85,
                }}
              >
                {t('mind_inspector.graph.growth_evidence', { count: support })}
              </span>
            )
          )}
        </div>

        {/* 调整内容（第一人称指令） */}
        <div
          style={{
            fontFamily: HAND,
            fontSize: 14.5,
            lineHeight: 1.55,
            color: GPAPER.ink,
            wordBreak: 'break-word',
          }}
        >
          {text}
        </div>

        {/* 调整依据 */}
        {reason && (
          <div
            style={{
              marginTop: 3,
              fontSize: 12,
              lineHeight: 1.5,
              color: GPAPER.inkSoft,
              wordBreak: 'break-word',
            }}
          >
            ↳ {reason}
          </div>
        )}
      </div>
    </div>
  );
};

const EvolutionSection: React.FC<{ character: CharacterId }> = ({ character }) => {
  const { t } = useTranslation();
  const [data, setData] = useState<EvolutionPayload | null>(null);
  const [loading, setLoading] = useState(true);
  // 旋转刷新按钮的小把戏：每次刷新 +36deg
  const [spin, setSpin] = useState(0);

  const load = useCallback(() => {
    let cancelled = false;
    setLoading(true);
    invoke<EvolutionPayload>('get_persona_evolution', { characterId: character })
      .then((payload) => {
        if (!cancelled) setData(payload);
      })
      .catch(() => {
        // 后端不可用/命令缺失时静默降级为空态，不打断图谱页
        if (!cancelled) setData({ entries: [], candidates: [], is_empty: true, last_update: 0 });
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [character]);

  useEffect(() => load(), [character]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleRefresh = () => {
    setSpin((s) => s + 1);
    load();
  };

  const entries = data?.entries ?? [];
  const candidates = data?.candidates ?? [];
  // 与后端 persona/evolution.rs 的 REQUIRED_SUPPORT 对齐（跨轨迹支持门槛）
  const requiredSupport = 2;
  const isEmpty = entries.length === 0 && candidates.length === 0;

  return (
    <div
      style={{
        borderRadius: RADIUS.xl,
        border: `1.5px solid ${GPAPER.border}`,
        background: GPAPER.paper,
        backgroundImage:
          'linear-gradient(var(--graph-grid) 1px, transparent 1px), linear-gradient(90deg, var(--graph-grid) 1px, transparent 1px)',
        backgroundSize: '24px 24px',
        boxShadow: 'var(--graph-shadow-lg)',
        padding: SPACING.md,
      }}
    >
      {/* 标题行：印章 + 标题 + 铅笔线 + 刷新 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 9, marginBottom: SPACING.sm }}>
        <span
          aria-hidden
          style={{
            width: 22,
            height: 22,
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: GPAPER.stampRed,
            color: '#FFF9EE',
            borderRadius: 4,
            transform: 'rotate(5deg)',
            boxShadow: `inset 0 0 0 1.5px rgba(255,249,238,0.55), ${GPAPER.shadowSm}`,
            fontFamily: TYPO.fontFamilyCN,
            fontSize: 13.5,
            lineHeight: 1,
          }}
        >
          长
        </span>
        <span
          style={{
            fontFamily: TYPO.fontFamilyCN,
            fontSize: 15,
            color: GPAPER.ink,
            letterSpacing: 2,
            whiteSpace: 'nowrap',
          }}
        >
          {t('mind_inspector.graph.growth_title')}
        </span>
        <span
          aria-hidden
          style={{
            flex: 1,
            borderBottom: `1px dashed ${GPAPER.border}`,
            transform: 'translateY(3px)',
          }}
        />
        <button
          type="button"
          onClick={handleRefresh}
          title={t('mind_inspector.graph.growth_refresh')}
          style={{
            width: 26,
            height: 26,
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            borderRadius: 6,
            border: 'none',
            background: 'transparent',
            cursor: 'pointer',
            color: GPAPER.inkSoft,
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = 'var(--panel-surface-hover)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = 'transparent';
          }}
        >
          <RefreshCw
            size={14}
            style={{
              transform: `rotate(${spin * 36}deg)`,
              transition: 'transform 0.5s cubic-bezier(0.3, 0.7, 0.4, 1)',
              animation: loading ? 'mind-inspector-spin 0.8s linear infinite' : undefined,
            }}
          />
        </button>
      </div>

      {/* 内容区 */}
      {isEmpty && !loading ? (
        <EmptyState
          icon={<Sparkles size={22} />}
          text={t('mind_inspector.graph.growth_empty')}
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.sm }}>
          {/* 已生效的调整（旧 → 新，与覆盖层渲染顺序一致） */}
          {entries.map((e, i) => (
            <GrowthRow
              key={`evo-${e.timestamp}-${i}`}
              id={`evo-${e.timestamp}-${e.text}`}
              kind={e.kind}
              text={e.text}
              reason={e.reason}
              tsSec={e.timestamp}
              support={e.support}
              pending={false}
              requiredSupport={requiredSupport}
            />
          ))}

          {/* 酝酿中的候选（未达跨轨迹支持门槛） */}
          {candidates.length > 0 && (
            <>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'baseline',
                  gap: 8,
                  marginTop: 2,
                }}
              >
                <span
                  style={{
                    fontSize: 12,
                    fontWeight: 600,
                    color: GPAPER.inkSoft,
                    letterSpacing: 1,
                  }}
                >
                  {t('mind_inspector.graph.growth_pending')}
                </span>
                <span style={{ fontSize: 11, color: GPAPER.inkFaint }}>
                  {t('mind_inspector.graph.growth_pending_hint')}
                </span>
              </div>
              {candidates.map((c, i) => (
                <GrowthRow
                  key={`cand-${c.first_seen}-${i}`}
                  id={`cand-${c.first_seen}-${c.text}`}
                  kind={c.kind}
                  text={c.text}
                  reason={c.reason}
                  tsSec={c.first_seen}
                  support={c.support}
                  pending
                  requiredSupport={requiredSupport}
                />
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
};

export default EvolutionSection;
