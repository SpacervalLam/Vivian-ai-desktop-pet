/**
 * Journal 页 — 创作视图（子 tab 切换日记 / 笔记 / 待办 / 定时）
 *
 * 日记(DiaryPage)、笔记(NotebookPage)、待办(TodoPage)、定时(SchedulerPage) 为同一级子视图，
 * 页内用顶部胶囊子 tab 切换。
 * 兼容内部跳转：外部 navigateTo('diary'/'notebook'/'todo'/'scheduler') 会带上 pageParams.sub，
 * 本组件收到后自动切到对应子视图，同时 diaryId / notebookId 等参数经 NavigationContext 透传给子页面定位。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BookHeart, NotebookPen, CalendarClock } from 'lucide-react';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
} from '../design-system';
import { useNavigation } from '../NavigationContext';
import DiaryPage from './DiaryPage';
import NotebookPage from './NotebookPage';
import PlannerPage from './PlannerPage';

type JournalTab = 'diary' | 'notebook' | 'planner';

const TABS: Array<{ key: JournalTab; labelKey: string; icon: React.ElementType }> = [
  { key: 'diary', labelKey: 'mind_inspector.nav_diary', icon: BookHeart },
  { key: 'notebook', labelKey: 'mind_inspector.nav_notebook', icon: NotebookPen },
  { key: 'planner', labelKey: 'mind_inspector.nav_planner', icon: CalendarClock },
];

/** 重挂后仍记住上次子 tab */
let cachedTab: JournalTab | null = null;

const JournalPage: React.FC = () => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const p = nav?.pageParams ?? {};
  const sub = p.sub as string | undefined;
  // 初始 tab：外部带定位参数（笔记 ID / 日记 ID / sub）时优先跟随
  const initial: JournalTab =
    cachedTab ??
    (sub === 'notebook' || p.notebookId
      ? 'notebook'
      : sub === 'todo' || sub === 'scheduler'
        ? 'planner'
        : sub === 'diary' || p.diaryId
          ? 'diary'
          : 'diary');
  const [tab, setTab] = useState<JournalTab>(initial);

  useEffect(() => {
    if (sub === 'diary' || sub === 'notebook') {
      setTab(sub);
    } else if (sub === 'todo' || sub === 'scheduler') {
      setTab('planner');
    }
  }, [sub]);

  useEffect(() => {
    cachedTab = tab;
  }, [tab]);

  return (
    <div style={{ minWidth: 0, display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      {/* 手账页头（大标题 + 铅笔线） */}
      <div
        style={{
          display: 'flex',
          alignItems: 'baseline',
          gap: 10,
          marginBottom: SPACING.sm,
          padding: '0 2px',
        }}
      >
        <span
          style={{
            fontSize: 20,
            fontWeight: 700,
            letterSpacing: 0.3,
            color: 'var(--panel-text)',
            fontFamily: TYPO.fontFamilyCN,
          }}
        >
          {t('mind_inspector.nav_journal')}
        </span>
        <span style={{ flex: 1, height: 1, minWidth: 24, background: 'repeating-linear-gradient(90deg, var(--panel-border) 0 6px, transparent 6px 11px)', opacity: 0.7 }} />
        <span
          style={{
            fontSize: 11,
            fontWeight: 600,
            letterSpacing: 1,
            color: 'var(--panel-accent)',
            background: 'var(--sticker-sky-soft)',
            padding: '2px 10px',
            borderRadius: 999,
          }}
        >
          {t(tab === 'diary' ? 'mind_inspector.nav_diary' : tab === 'notebook' ? 'mind_inspector.nav_notebook' : 'mind_inspector.nav_planner')}
        </span>
      </div>

      {/* 子 tab 栏 */}
      <div className="mind-tabs">
        {TABS.map((tItem) => {
          const Icon = tItem.icon;
          const active = tab === tItem.key;
          return (
            <button
              key={tItem.key}
              type="button"
              onClick={() => setTab(tItem.key)}
              className={`mind-tab ${active ? 'active' : ''}`}
            >
              <Icon size={15} strokeWidth={active ? 2.2 : 1.8} />
              {t(tItem.labelKey)}
            </button>
          );
        })}
      </div>

      {/* 当前子视图 */}
      <div key={tab} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        {tab === 'diary' ? (
          <DiaryPage />
        ) : tab === 'notebook' ? (
          <NotebookPage />
        ) : (
          <PlannerPage />
        )}
      </div>
    </div>
  );
};

export default JournalPage;
