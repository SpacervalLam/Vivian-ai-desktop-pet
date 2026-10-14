/**
 * Planner 页 — 待办 + 定时合并页
 *
 * 两页共用 .record-plan-* 任务行骨架，页内保留待办 / 定时切换。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ListChecks, Clock, ArrowLeft, ArrowRight } from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';
import { ComicChapterHeading, ComicChapterInset } from './ComicChapter';

type PlannerTab = 'todo' | 'scheduler';

const TABS: Array<{ key: PlannerTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'todo', labelKey: 'mind_inspector.nav_todo', icon: ListChecks },
  { key: 'scheduler', labelKey: 'mind_inspector.nav_scheduler', icon: Clock },
];

const PlannerPage: React.FC<{ embedded?: boolean; expanded?: boolean }> = ({ embedded = false, expanded = false }) => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const sub = nav?.pageParams.sub;
  const [tab, setTab] = useState<PlannerTab>(sub === 'scheduler' ? 'scheduler' : 'todo');
  useEffect(() => {
    if (sub === 'todo' || sub === 'scheduler') setTab(sub);
  }, [sub]);

  if (expanded) {
    const next = tab === 'todo' ? 'scheduler' : 'todo';
    const Icon = tab === 'todo' ? ListChecks : Clock;
    const Direction = tab === 'todo' ? ArrowRight : ArrowLeft;
    return <section className="comic-chapter comic-planner planner-sliding" data-mode={tab}>
      <button type="button" className="planner-switch-space" onClick={() => setTab(next)}
        aria-label={t(next === 'scheduler' ? 'comic.plannerSwitchScheduler' : 'comic.plannerSwitchTodo')}>
        <span><Direction size={20} /><span>{t(next === 'scheduler' ? 'comic.plannerSwitchScheduler' : 'comic.plannerSwitchTodo')}</span></span>
      </button>
      <div className="planner-sliding-card">
        <div className="planner-mode-title"><Icon size={19} /><span>{t(tab === 'todo' ? 'mind_inspector.nav_todo' : 'mind_inspector.nav_scheduler')}</span></div>
        <div className="planner-morph-content">
          {TABS.map(item => <div key={item.key} className={`planner-mode-content${tab === item.key ? ' is-active' : ''}`}
            aria-hidden={tab !== item.key} ref={node => { if (node) node.inert = tab !== item.key; }}>
            {item.key === 'todo' ? <TodoPage /> : <SchedulerPage />}
          </div>)}
        </div>
      </div>
    </section>;
  }

  return (
    <section className="comic-chapter comic-planner">
      {!embedded && <ComicChapterHeading number="04" title={tab === 'todo' ? '约好下一件事' : '到了那时，提醒我'}
        caption={tab === 'todo' ? '平常的日子里，还有想一起做的事。' : '别让小小的约定，悄悄错过。'}
        image={tab === 'todo' ? 'after-rain-street-v19' : 'line-phone-on-v33'} position="50% 50%">
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
      </ComicChapterHeading>}
      {embedded && <div className="rec-seg comic-planner-modes">{TABS.map(item => <button key={item.key} type="button" onClick={() => setTab(item.key)} className={`rec-seg-item${tab === item.key ? ' is-active' : ''}`} aria-pressed={tab === item.key}><item.icon size={14} />{t(item.labelKey)}</button>)}</div>}
      <div className="comic-plan-spread">
        {!embedded && <aside className="comic-plan-aside" aria-label={t('comic.chapterArt')}>
          <ComicChapterInset image={tab === 'todo' ? 'vivian-platform-light-v25' : 'nana-door-smile-v23'}
            caption={tab === 'todo' ? '想做的事，就从这里开始。' : '到了时间，再叫你一声。'} />
          <ComicChapterInset image={tab === 'todo' ? 'shared-handle-edit-v14a' : 'umbrella-drip-v01'}
            caption={tab === 'todo' ? '一件一件，慢慢完成。' : '有些事，值得惦记。'} position="50% 50%" />
        </aside>}
      <div key={tab} className="comic-plan-content">
        {tab === 'todo' ? <TodoPage /> : <SchedulerPage />}
      </div>
      </div>
    </section>
  );
};

export default PlannerPage;
