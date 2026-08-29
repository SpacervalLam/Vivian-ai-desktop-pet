/**
 * Overview 页 — 综合视图（子 tab 切换四个原页面）
 *
 * 合并 心智(MindPage) / 世界(WorldPage) / 记忆(GraphPage) / 用户画像(UserProfilePage)
 * 为一个侧边栏入口，页内用顶部胶囊子 tab 切换。子 tab 可被内部跳转覆盖：
 * 例如 GraphPage → navigateTo('graph') 会通过 MindInspector 映射为 sub='graph'，
 * 本组件收到后自动切到对应子视图。
 */

import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Brain,
  Globe,
  Network,
  UserCircle,
} from 'lucide-react';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
} from '../design-system';
import { useNavigation } from '../NavigationContext';
import MindPage from './MindPage';
import WorldPage from './WorldPage';
import GraphPage from './GraphPage';
import UserProfilePage from './UserProfilePage';

type OverviewTab = 'mind' | 'world' | 'graph' | 'profile';

const TABS: Array<{ key: OverviewTab; labelKey: string; icon: React.ElementType; order: number }> = [
  { key: 'mind', labelKey: 'mind_inspector.nav_mind', icon: Brain, order: 0 },
  { key: 'world', labelKey: 'mind_inspector.nav_world', icon: Globe, order: 1 },
  { key: 'graph', labelKey: 'mind_inspector.nav_graph', icon: Network, order: 2 },
  { key: 'profile', labelKey: 'mind_inspector.nav_profile', icon: UserCircle, order: 3 },
];

const isTab = (v: string | undefined): v is OverviewTab =>
  v === 'mind' || v === 'world' || v === 'graph' || v === 'profile';

/** 重挂后仍记住上次子 tab（避免切出再切回时丢失） */
let cachedTab: OverviewTab | null = null;

const OverviewPage: React.FC = () => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const sub = nav?.pageParams?.sub;
  const initial: OverviewTab = (cachedTab ?? (isTab(sub as string) ? (sub as OverviewTab) : 'mind'));
  const [tab, setTab] = useState<OverviewTab>(initial);

  // 外部跳转请求（sub 生效）时跟随切换
  useEffect(() => {
    if (isTab(sub as string)) {
      setTab(sub as OverviewTab);
    }
  }, [sub]);

  useEffect(() => {
    cachedTab = tab;
  }, [tab]);

  const render = (): React.ReactNode => {
    switch (tab) {
      case 'mind':
        return <MindPage />;
      case 'world':
        return <WorldPage />;
      case 'graph':
        return <GraphPage />;
      case 'profile':
        return <UserProfilePage />;
    }
  };

  return (
    <div style={{ minWidth: 0 }}>
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
          {t('mind_inspector.nav_overview')}
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
          {t(tab === 'graph' ? 'mind_inspector.nav_graph' : tab === 'profile' ? 'mind_inspector.nav_profile' : tab === 'world' ? 'mind_inspector.nav_world' : 'mind_inspector.nav_mind')}
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

      {/* 当前子视图（切换时重挂，保留各页自身布局） */}
      <div key={tab}>{render()}</div>
    </div>
  );
};

export default OverviewPage;