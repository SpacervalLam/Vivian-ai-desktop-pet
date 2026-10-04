/**
 * Planner 页 — 待办 + 定时合并页
 *
 * 两页共用 .record-plan-* 任务行骨架，页内保留待办 / 定时切换。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ListChecks, Clock } from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';

type PlannerTab = 'todo' | 'scheduler';

const TABS: Array<{ key: PlannerTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'todo', labelKey: 'mind_inspector.nav_todo', icon: ListChecks },
  { key: 'scheduler', labelKey: 'mind_inspector.nav_scheduler', icon: Clock },
];

const PlannerPage: React.FC = () => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const sub = nav?.pageParams.sub;
  const [tab, setTab] = useState<PlannerTab>(sub === 'scheduler' ? 'scheduler' : 'todo');
  useEffect(() => {
    if (sub === 'todo' || sub === 'scheduler') setTab(sub);
  }, [sub]);

  return (
    <div style={{ minWidth: 0, display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          flexShrink: 0,
          paddingBottom: 'var(--rec-gap-3)',
          marginBottom: 'var(--rec-gap-4)',
          borderBottom: 'var(--rec-line-soft)',
        }}
      >
        <div className="rec-seg">
          {TABS.map((item) => {
            const Icon = item.icon;
            const active = tab === item.key;
            return (
              <button
                key={item.key}
                type="button"
                onClick={() => setTab(item.key)}
                className={`rec-seg-item${active ? ' is-active' : ''}`}
                aria-pressed={active}
              >
                <Icon size={14} strokeWidth={active ? 2 : 1.8} />
                {t(item.labelKey)}
              </button>
            );
          })}
        </div>
      </div>
      <div key={tab} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        {tab === 'todo' ? <TodoPage /> : <SchedulerPage />}
      </div>
    </div>
  );
};

export default PlannerPage;
