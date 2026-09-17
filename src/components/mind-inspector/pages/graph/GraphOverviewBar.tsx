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

/** 筛选 Chip 定义：一个 chip 可覆盖多个底层节点类型（如"聊天"= dialogue + wechat） */
export interface TypeChipDef {
  id: string;
  types: NodeType[];
  label: string;
  color: string;
  count: number;
}

interface GraphOverviewBarProps {
  character: CharacterId;
  chips: TypeChipDef[];
  /** 当前激活的节点类型集合（空集 = 全部显示，支持多选） */
  activeTypes: Set<NodeType>;
  /** 切换一组类型的选中状态（多选取并集；再次点击已全选的组则整组取消） */
  onToggleTypes: (types: NodeType[]) => void;
  /** 清空所有筛选（回到全部显示） */
  onClear: () => void;
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
  activeTypes,
  onToggleTypes,
  onClear,
  onDateSearch,
}) => {
  const { t } = useTranslation();
  const [dateInput, setDateInput] = useState('');

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
          onClick={onClear}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 5,
            padding: '3px 10px',
            border: `1.5px solid ${activeTypes.size === 0 ? GPAPER.ink : 'transparent'}`,
            borderRadius: 4,
            background: activeTypes.size === 0 ? GPAPER.ink : 'transparent',
            color: activeTypes.size === 0 ? GPAPER.card : GPAPER.ink,
            cursor: 'pointer',
            fontFamily: HAND,
            fontSize: 13,
            transform: `rotate(${chipTilt('all')}deg)`,
            transition: 'border-color 0.15s ease, background 0.15s ease, color 0.15s ease',
            boxShadow: activeTypes.size === 0 ? '0 2px 6px rgba(0,0,0,0.18)' : 'none',
          }}
        >
          {t('mind_inspector.graph.filter_all')} · {totalCount}
        </button>

        {/* 类型 chip（多选：点击切换该组类型） */}
        {chips.map((c) => {
          const active = c.types.every((tp) => activeTypes.has(tp));
          return (
            <button
              key={c.id}
              type="button"
              onClick={() => onToggleTypes(c.types)}
              title={active ? t('mind_inspector.graph.filter_click_clear') : t('mind_inspector.graph.filter_click_hint')}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 5,
                padding: '3px 10px',
                border: `1.5px solid ${active ? c.color : `${c.color}40`}`,
                borderRadius: 4,
                background: active ? c.color : `${c.color}14`,
                cursor: 'pointer',
                fontFamily: HAND,
                fontSize: 13,
                color: active ? '#fff' : GPAPER.ink,
                transform: `rotate(${chipTilt(c.id)}deg)`,
                transition: 'border-color 0.15s ease, background 0.15s ease, color 0.15s ease',
                boxShadow: active ? `0 2px 6px ${c.color}66` : 'none',
              }}
            >
              <span
                aria-hidden
                style={{
                  width: 8,
                  height: 8,
                  borderRadius: '50%',
                  background: active ? '#fff' : c.color,
                  transition: 'background 0.15s ease',
                }}
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

        {/* 图例开关已移除 */}
      </div>

      {/* 行 2：记忆巩固健康条（内嵌） */}
      <MemoryHealthStrip characterId={character} />
    </div>
  );
};

export default GraphOverviewBar;
