/**
 * GraphOverviewBar — 记忆图谱顶部工作台
 *
 * 整合（替代原"底部统计贴纸"和滚动区内的健康条）：
 * - 统计行：各类型计数（可点击的筛选 Chip，选中类型高亮）
 * - 记忆巩固健康条（内嵌，不再 sticky 在滚动区里）
 * - 时间搜索：日期输入 → 定位到最近记忆
 * - 图例说明（手账贴纸风格，与页面 GPAPER 主题一致）
 */

import React, { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Search } from 'lucide-react';
import { SPACING, RADIUS } from '../../design-system';
import type { CharacterId, NodeType } from './types';
import MemoryHealthStrip from '../MemoryHealthStrip';

const GPAPER = {
  card: 'var(--graph-card)',
  ink: 'var(--graph-ink)',
  inkSoft: 'var(--graph-ink-soft)',
  inkFaint: 'var(--graph-ink-faint)',
  border: 'var(--graph-border)',
  shadowSm: 'var(--graph-shadow-sm)',
} as const;

const HAND =
  '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "PingFang SC", "Microsoft YaHei", serif';

/** 筛选 Chip 定义（类型 → 显示色取自 classifyMemory 的分类色，由调用方传入） */
export interface TypeChipDef {
  type: NodeType;
  label: string;
  color: string;
  count: number;
}

interface GraphOverviewBarProps {
  character: CharacterId;
  chips: TypeChipDef[];
  /** 当前选中的筛选类型（null = 全部显示） */
  activeFilter: NodeType | null;
  onFilterChange: (type: NodeType | null) => void;
  /** 日期搜索：定位到该日期最近的节点 */
  onDateSearch: (dateStr: string) => void;
}

/** 由节点 id 生成确定性轻微旋转角（贴纸感，与 GraphPage 同规则） */
const chipTilt = (key: string): number => {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
  return ((Math.abs(h) % 100) / 100 - 0.5) * 4;
};

const GraphOverviewBar: React.FC<GraphOverviewBarProps> = ({
  character,
  chips,
  activeFilter,
  onFilterChange,
  onDateSearch,
}) => {
  const { t } = useTranslation();
  const [dateInput, setDateInput] = useState('');
  const [showLegend, setShowLegend] = useState(false);

  const totalCount = useMemo(
    () => chips.reduce((sum, c) => sum + c.count, 0),
    [chips],
  );

  const submitSearch = () => {
    const trimmed = dateInput.trim();
    if (!trimmed) return;
    onDateSearch(trimmed);
  };

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.sm,
        padding: `${SPACING.md}px ${SPACING.md + 2}px`,
        borderRadius: RADIUS.xl,
        border: `1.5px solid ${GPAPER.border}`,
        background: GPAPER.card,
        boxShadow: 'var(--graph-shadow-md)',
      }}
    >
      {/* 行 1：统计 Chip（可点筛选）+ 时间搜索 + 图例开关 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: SPACING.sm,
          flexWrap: 'wrap',
        }}
      >
        {/* 全部 chip */}
        <button
          type="button"
          onClick={() => onFilterChange(null)}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 5,
            padding: '3px 10px',
            border: `1.5px solid ${activeFilter === null ? GPAPER.ink : 'transparent'}`,
            borderRadius: 4,
            background: 'transparent',
            cursor: 'pointer',
            fontFamily: HAND,
            fontSize: 13,
            color: GPAPER.ink,
            transform: `rotate(${chipTilt('all')}deg)`,
            transition: 'border-color 0.15s ease',
          }}
        >
          {t('mind_inspector.graph.filter_all')} · {totalCount}
        </button>

        {/* 类型 chip */}
        {chips.map((c) => {
          const active = activeFilter === c.type;
          return (
            <button
              key={c.type}
              type="button"
              onClick={() => onFilterChange(active ? null : c.type)}
              title={active ? t('mind_inspector.graph.filter_click_clear') : t('mind_inspector.graph.filter_click_hint')}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 5,
                padding: '3px 10px',
                border: `1.5px solid ${active ? c.color : 'transparent'}`,
                borderRadius: 4,
                background: active ? `${c.color}33` : `${c.color}1A`,
                cursor: 'pointer',
                fontFamily: HAND,
                fontSize: 13,
                color: GPAPER.ink,
                transform: `rotate(${chipTilt(c.type)}deg)`,
                transition: 'border-color 0.15s ease, background 0.15s ease',
              }}
            >
              <span
                aria-hidden
                style={{ width: 8, height: 8, borderRadius: '50%', background: c.color }}
              />
              {c.label} · {c.count}
            </button>
          );
        })}

        {/* 弹性占位 */}
        <div style={{ flex: 1 }} />

        {/* 时间搜索 */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 6,
            padding: '2px 4px 2px 8px',
            border: `1px solid ${GPAPER.border}`,
            borderRadius: 6,
            background: 'var(--graph-paper)',
          }}
        >
          <Search size={13} style={{ color: GPAPER.inkSoft, flexShrink: 0 }} />
          <input
            type="text"
            value={dateInput}
            onChange={(e) => setDateInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submitSearch();
            }}
            placeholder={t('mind_inspector.graph.search_date_placeholder')}
            style={{
              width: 130,
              border: 'none',
              background: 'transparent',
              outline: 'none',
              color: GPAPER.ink,
              fontFamily: 'system-ui, sans-serif',
              fontSize: 12,
            }}
          />
          {dateInput.trim() && (
            <button
              type="button"
              onClick={submitSearch}
              style={{
                border: 'none',
                background: GPAPER.ink,
                color: 'var(--graph-card)',
                borderRadius: 4,
                padding: '3px 10px',
                fontSize: 12,
                cursor: 'pointer',
                fontFamily: 'inherit',
              }}
            >
              {t('mind_inspector.graph.search_go')}
            </button>
          )}
        </div>

        {/* 图例开关 */}
        <button
          type="button"
          onClick={() => setShowLegend((v) => !v)}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 4,
            padding: '3px 10px',
            border: `1px dashed ${GPAPER.inkFaint}`,
            borderRadius: 4,
            background: 'transparent',
            cursor: 'pointer',
            fontFamily: HAND,
            fontSize: 12.5,
            color: GPAPER.inkSoft,
          }}
        >
          {showLegend ? t('mind_inspector.graph.legend_hide') : t('mind_inspector.graph.legend_show')}
        </button>
      </div>

      {/* 行 2：记忆巩固健康条（内嵌） */}
      <MemoryHealthStrip characterId={character} />

      {/* 行 3（可折叠）：图例 */}
      {showLegend && (
        <div
          style={{
            display: 'flex',
            gap: SPACING.sm,
            flexWrap: 'wrap',
            paddingTop: SPACING.xs,
            borderTop: `1px dashed ${GPAPER.border}`,
          }}
        >
          <span style={{ fontFamily: HAND, fontSize: 12.5, color: GPAPER.inkSoft, alignSelf: 'center' }}>
            {t('mind_inspector.graph.legend_title')}：
          </span>
          {[
            t('mind_inspector.graph.legend_session_circle'),
            t('mind_inspector.graph.legend_reply_arrow'),
            t('mind_inspector.graph.legend_presence_color'),
            t('mind_inspector.graph.legend_drag'),
            t('mind_inspector.graph.legend_summary_expand'),
          ].map((tip, i) => (
            <span
              key={i}
              style={{
                fontSize: 12,
                color: GPAPER.inkSoft,
                fontFamily: 'system-ui, sans-serif',
                padding: '2px 8px',
                background: 'var(--graph-paper)',
                borderRadius: 3,
              }}
            >
              {tip}
            </span>
          ))}
        </div>
      )}
    </div>
  );
};

export default GraphOverviewBar;
