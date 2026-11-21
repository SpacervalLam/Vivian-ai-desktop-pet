/**
 * Planner 页 — 待办 + 定时合并页
 *
 * 两页共用 .record-plan-* 任务行骨架，页内保留待办 / 定时切换。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ListChecks, Clock, Bookmark } from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';
import AgreementsPage from './AgreementsPage';
import { agreementWords } from './agreementPresentation';

type PlannerTab = 'todo' | 'agreements' | 'scheduler';

const TABS: Array<{ key: PlannerTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'todo', labelKey: 'mind_inspector.nav_todo', icon: ListChecks },
  { key: 'agreements', labelKey: '', icon: Bookmark },
  { key: 'scheduler', labelKey: 'mind_inspector.nav_scheduler', icon: Clock },
];

const PlannerPage: React.FC = () => {
  const { t, i18n } = useTranslation();
  const agreementTitle = agreementWords(i18n.resolvedLanguage ?? i18n.language, '关注与约定', 'Interests & agreements', '関心と約束');
  const label = (key: PlannerTab) => key === 'agreements' ? agreementTitle : t(key === 'todo' ? 'mind_inspector.nav_todo' : 'mind_inspector.nav_scheduler');
  const content = (key: PlannerTab) => key === 'todo' ? <TodoPage /> : key === 'agreements' ? <AgreementsPage /> : <SchedulerPage />;
  const nav = useNavigation();
  const sub = nav?.pageParams.sub;
  const [tab, setTab] = useState<PlannerTab>(sub === 'scheduler' || sub === 'agreements' ? sub : 'todo');
  useEffect(() => {
    if (sub === 'todo' || sub === 'scheduler' || sub === 'agreements') setTab(sub);
  }, [sub, nav?.pageParams]);

  return (
    <section className="record-page record-planner">
      <div className="rec-seg record-planner-modes">
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
              {label(item.key)}
            </button>
          );
        })}
      </div>
      <div className="record-layout-spread">

      <div key={tab} className="record-layout-content">
        {content(tab)}
      </div>
      </div>
    </section>
  );
};

export default PlannerPage;
