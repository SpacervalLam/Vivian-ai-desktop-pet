/**
 * Planner 页 — 待办 + 定时合并页
 *
 * 两页共用 .record-plan-* 任务行骨架，页内保留待办 / 定时切换。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ListChecks, Clock, Bookmark, ArrowLeft, ArrowRight } from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import TodoPage from './TodoPage';
import SchedulerPage from './SchedulerPage';
import AgreementsPage from './AgreementsPage';
import { agreementWords } from './agreementPresentation';
import { ComicChapterHeading, ComicChapterInset } from './ComicChapter';

type PlannerTab = 'todo' | 'agreements' | 'scheduler';

const TABS: Array<{ key: PlannerTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'todo', labelKey: 'mind_inspector.nav_todo', icon: ListChecks },
  { key: 'agreements', labelKey: '', icon: Bookmark },
  { key: 'scheduler', labelKey: 'mind_inspector.nav_scheduler', icon: Clock },
];

const PlannerPage: React.FC<{ embedded?: boolean; expanded?: boolean }> = ({ embedded = false, expanded = false }) => {
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

  if (expanded) {
    const next = TABS[(TABS.findIndex(item => item.key === tab) + 1) % TABS.length].key;
    const Icon = TABS.find(item => item.key === tab)!.icon;
    const Direction = tab === 'todo' ? ArrowRight : ArrowLeft;
    return <section className="comic-chapter comic-planner planner-sliding" data-mode={tab}>
      <button type="button" className="planner-switch-space" onClick={() => setTab(next)}
        aria-label={label(next)}>
        <span><Direction size={20} /><span>{label(next)}</span></span>
      </button>
      <div className="planner-sliding-card">
        <div className="planner-mode-title"><Icon size={19} /><span>{label(tab)}</span></div>
        <div className="rec-seg comic-planner-modes">{TABS.map(item => <button key={item.key} type="button" onClick={() => setTab(item.key)} className={`rec-seg-item${tab === item.key ? ' is-active' : ''}`} aria-pressed={tab === item.key}><item.icon size={14} />{label(item.key)}</button>)}</div>
        <div className="planner-morph-content">
          {TABS.map(item => <div key={item.key} className={`planner-mode-content${tab === item.key ? ' is-active' : ''}`}
            aria-hidden={tab !== item.key} ref={node => { if (node) node.inert = tab !== item.key; }}>
            {content(item.key)}
          </div>)}
        </div>
      </div>
    </section>;
  }

  return (
    <section className="comic-chapter comic-planner">
      {!embedded && <ComicChapterHeading number="04" title={tab === 'agreements' ? agreementTitle : tab === 'todo' ? '约好下一件事' : '到了那时，提醒我'}
        caption={tab === 'agreements' ? '' : tab === 'todo' ? '平常的日子里，还有想一起做的事。' : '别让小小的约定，悄悄错过。'}
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
                {label(item.key)}
              </button>
            );
          })}
        </div>
      </ComicChapterHeading>}
      {embedded && <div className="rec-seg comic-planner-modes">{TABS.map(item => <button key={item.key} type="button" onClick={() => setTab(item.key)} className={`rec-seg-item${tab === item.key ? ' is-active' : ''}`} aria-pressed={tab === item.key}><item.icon size={14} />{label(item.key)}</button>)}</div>}
      <div className="comic-plan-spread">
        {!embedded && <aside className="comic-plan-aside" aria-label={t('comic.chapterArt')}>
          <ComicChapterInset image={tab === 'todo' ? 'vivian-platform-light-v25' : 'nana-door-smile-v23'}
            caption={tab === 'todo' ? '想做的事，就从这里开始。' : '到了时间，再叫你一声。'} />
          <ComicChapterInset image={tab === 'todo' ? 'shared-handle-edit-v14a' : 'umbrella-drip-v01'}
            caption={tab === 'todo' ? '一件一件，慢慢完成。' : '有些事，值得惦记。'} position="50% 50%" />
        </aside>}
      <div key={tab} className="comic-plan-content">
        {content(tab)}
      </div>
      </div>
    </section>
  );
};

export default PlannerPage;
