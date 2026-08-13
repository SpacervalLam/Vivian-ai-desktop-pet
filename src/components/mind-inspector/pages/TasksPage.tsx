/**
 * Tasks 页 — 待办事件 + 定时任务（合并）
 *
 * 顶部二级 tab 切换「待办」和「定时任务」，各自保留原有的筛选 tab 和表单逻辑。
 */

import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { COLORS, TYPO, SPACING, RADIUS, EASE, DURATION } from '../design-system';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';

type SubTab = 'todo' | 'scheduler';

const TasksPage: React.FC = () => {
  const { t } = useTranslation();
  const [subTab, setSubTab] = useState<SubTab>('todo');

  const tabs: { key: SubTab; label: string }[] = [
    { key: 'todo', label: t('mind_inspector.nav_todo') },
    { key: 'scheduler', label: t('mind_inspector.nav_scheduler') },
  ];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      {/* 二级 tab 切换（iOS 胶囊容器 + 半透明激活填充） */}
      <div
        style={{
          display: 'inline-flex',
          gap: 4,
          padding: 4,
          alignSelf: 'flex-start',
          marginBottom: SPACING.md,
          flexShrink: 0,
          borderRadius: RADIUS.pill,
          background: COLORS.subtleBg,
          border: `1px solid ${COLORS.subtleBorder}`,
        }}
      >
        {tabs.map((tb) => {
          const active = subTab === tb.key;
          return (
            <button
              key={tb.key}
              onClick={() => setSubTab(tb.key)}
              style={{
                padding: '6px 20px',
                border: 'none',
                borderRadius: RADIUS.pill,
                background: active ? `${COLORS.accent}22` : 'transparent',
                color: active ? COLORS.accent : COLORS.textSecondary,
                fontSize: 14,
                fontWeight: active ? 600 : 500,
                cursor: 'pointer',
                transition: `background ${DURATION.normal}s ${EASE.swift}, color ${DURATION.normal}s ${EASE.swift}, transform ${DURATION.fast}s ${EASE.spring}`,
                fontFamily: TYPO.fontFamily,
              }}
            >
              {tb.label}
            </button>
          );
        })}
      </div>

      {/* 内容区 */}
      <div style={{ flex: 1, minHeight: 0, overflow: 'hidden' }}>
        {subTab === 'todo' ? <TodoPage /> : <SchedulerPage />}
      </div>
    </div>
  );
};

export default TasksPage;
