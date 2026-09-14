/**
 * Planner 页 — 待办 + 定时合并页
 *
 * 将原 TodoPage 与 SchedulerPage 合并为一个页面，内部通过手账 Tab 切换。
 */

import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ListChecks, Clock } from 'lucide-react';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';

type PlannerTab = 'todo' | 'scheduler';

const TABS: Array<{ key: PlannerTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'todo', labelKey: 'mind_inspector.nav_todo', icon: ListChecks },
  { key: 'scheduler', labelKey: 'mind_inspector.nav_scheduler', icon: Clock },
];

const PlannerPage: React.FC = () => {
  const { t } = useTranslation();
  const [tab, setTab] = useState<PlannerTab>('todo');

  return (
    <div style={{ minWidth: 0, display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      <div className="mind-tabs">
        {TABS.map((item) => {
          const Icon = item.icon;
          const active = tab === item.key;
          return (
            <button
              key={item.key}
              type="button"
              onClick={() => setTab(item.key)}
              className={`mind-tab ${active ? 'active' : ''}`}
            >
              <Icon size={15} strokeWidth={active ? 2.2 : 1.8} />
              {t(item.labelKey)}
            </button>
          );
        })}
      </div>
      <div key={tab} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        {tab === 'todo' ? <TodoPage /> : <SchedulerPage />}
      </div>
    </div>
  );
};

export default PlannerPage;
