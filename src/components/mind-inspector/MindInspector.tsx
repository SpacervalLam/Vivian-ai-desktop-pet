/**
 * Mind Inspector 壳组件
 *
 * Large Title 大标题 + 浮动胶囊侧边栏 + 页面切换动画。
 * 侧边栏导航在 8 个页面组件之间切换，激活态采用填充背景 + 顶部 accent 高亮线。
 */

import React, { useState, useMemo, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import {
  COLORS,
  TYPO,
  SPACING,
  SIDEBAR,
  EASE,
  DURATION,
  RADIUS,
  NAV_ITEMS,
  SHADOW,
  type NavKey,
  type NavItem,
} from './design-system';
import { NavigationProvider } from './NavigationContext';
import type { PageParams } from './NavigationContext';
import MindPage from './pages/MindPage';
import WorldPage from './pages/WorldPage';
import GraphPage, { invalidatePastelCache } from './pages/GraphPage';
import DiaryPage from './pages/DiaryPage';
import UserProfilePage from './pages/UserProfilePage';

// === 关键帧（页面切换动画） ===
const KEYFRAMES_ID = 'mind-inspector-shell-keyframes';
if (typeof document !== 'undefined' && !document.getElementById(KEYFRAMES_ID)) {
  const style = document.createElement('style');
  style.id = KEYFRAMES_ID;
  style.textContent = `
@keyframes mind-inspector-page-enter {
  0% { opacity: 0; transform: translateY(8px) scale(0.99); }
  100% { opacity: 1; transform: translateY(0) scale(1); }
}`;
  document.head.appendChild(style);
}

/** 侧边栏单个导航按钮（iOS 风格浮动胶囊） */
const NavButton: React.FC<{
  item: NavItem;
  active: boolean;
  onClick: () => void;
}> = ({ item, active, onClick }) => {
  const { t } = useTranslation();
  const [hovered, setHovered] = useState(false);
  const Icon = item.icon;
  return (
    <button
      type="button"
      title={t(item.labelKey)}
      onClick={onClick}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        position: 'relative',
        width: 48,
        height: 48,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 0,
        border: 'none',
        borderRadius: RADIUS.md,
        background: active
          ? COLORS.accentMuted
          : hovered
            ? COLORS.bgHover
            : 'transparent',
        color: active ? COLORS.accentBright : COLORS.textSecondary,
        cursor: 'pointer',
        transition: `background ${DURATION.normal}s ${EASE.swift}, color ${DURATION.normal}s ${EASE.swift}, transform ${DURATION.fast}s ${EASE.spring}`,
        fontFamily: TYPO.fontFamily,
        transform: hovered && !active ? 'scale(1.06)' : 'scale(1)',
      }}
    >
      {/* 激活态左侧高亮竖线 */}
      {active && (
        <span
          style={{
            position: 'absolute',
            left: -SIDEBAR.margin / 2,
            top: '50%',
            transform: 'translateY(-50%)',
            width: 3,
            height: 20,
            borderRadius: RADIUS.pill,
            background: COLORS.accent,
            boxShadow: `0 0 8px ${COLORS.accentGlow}`,
          }}
        />
      )}
      <Icon size={22} strokeWidth={active ? 2.2 : 1.8} />
    </button>
  );
};

const MindInspector: React.FC = () => {
  const { t } = useTranslation();
  const [activeNav, setActiveNav] = useState<NavKey>('mind');
  const [pageParams, setPageParams] = useState<PageParams>({});
  const [headerExtra, setHeaderExtra] = useState<React.ReactNode>(null);

  // 切换页面时刷新动画 key
  const [animKey, setAnimKey] = useState(0);
  useEffect(() => {
    setAnimKey((k) => k + 1);
  }, [activeNav]);

  // 挂载时刷新 pastel 主题色缓存，应对用户切换主题后重新打开 Mind Inspector 的场景
  useEffect(() => {
    invalidatePastelCache();
  }, []);

  // 切换页面时清空标题行工具栏，避免上一页的控件残留
  useEffect(() => {
    setHeaderExtra(null);
  }, [activeNav]);

  const navigateTo = (page: NavKey, params?: PageParams) => {
    setActiveNav(page);
    setPageParams(params ?? {});
  };

  const clearPageParams = () => {
    setPageParams({});
  };

  const navContext = useMemo(
    () => ({
      navigateTo,
      activePage: activeNav,
      pageParams,
      clearPageParams,
      headerExtra,
      setHeaderExtra,
    }),
    [activeNav, pageParams, headerExtra],
  );

  const renderPage = (): React.ReactNode => {
    switch (activeNav) {
      case 'mind':
        return <MindPage />;
      case 'world':
        return <WorldPage />;
      case 'graph':
        return <GraphPage />;
      case 'diary':
        return <DiaryPage />;
      case 'profile':
        return <UserProfilePage />;
      default:
        return null;
    }
  };

  const activeItem = NAV_ITEMS.find((n) => n.key === activeNav);

  return (
    <NavigationProvider value={navContext}>
      <div
        style={{
          display: 'flex',
          height: '100%',
          background: 'transparent',
          color: COLORS.textPrimary,
          fontFamily: TYPO.fontFamily,
          overflow: 'hidden',
        }}
      >
        {/* 左侧浮动胶囊侧边栏（远离边缘，类似 iOS Control Center） */}
        <aside
          style={{
            width: SIDEBAR.widthCollapsed,
            flexShrink: 0,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            gap: SPACING.xs,
            padding: `${SIDEBAR.margin}px 0`,
            pointerEvents: 'none',
          }}
        >
          {/* 内部浮动容器（手帐风格） */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              gap: SPACING.xs,
              padding: `${SPACING.sm}px 0`,
              borderRadius: RADIUS.xl,
              background: COLORS.sidebarBg,
              border: `1.5px solid ${COLORS.borderStrong}`,
              backdropFilter: 'blur(16px) saturate(150%)',
              WebkitBackdropFilter: 'blur(16px) saturate(150%)',
              boxShadow: SHADOW.sidebar,
              pointerEvents: 'auto',
            }}
          >
            {NAV_ITEMS.map((item) => (
              <NavButton
                key={item.key}
                item={item}
                active={item.key === activeNav}
                onClick={() => setActiveNav(item.key)}
              />
            ))}
          </div>
        </aside>

        {/* 右侧内容区 */}
        <main
          style={{
            flex: 1,
            minWidth: 0,
            display: 'flex',
            flexDirection: 'column',
            overflow: 'hidden',
          }}
        >
          {/* Large Title 大标题区（右侧可注入页面工具栏） */}
          <header
            style={{
              flexShrink: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: SPACING.md,
              flexWrap: 'wrap',
              padding: `${SPACING.md + 4}px ${SPACING.lg}px ${SPACING.md}px`,
            }}
          >
            <h1
              style={{
                ...TYPO.largeTitle,
                color: COLORS.textPrimary,
                margin: 0,
              }}
            >
              {activeItem ? t(activeItem.labelKey) : ''}
            </h1>
            {headerExtra}
          </header>

          {/* 页面内容（可滚动，带切换动画） */}
          <div
            key={animKey}
            style={{
              flex: 1,
              minWidth: 0,
              overflow: 'auto',
              padding: `0 ${SPACING.lg}px ${SPACING.lg}px`,
              animation: `mind-inspector-page-enter ${DURATION.slow}s ${EASE.ios}`,
            }}
          >
            {renderPage()}
          </div>
        </main>
      </div>
    </NavigationProvider>
  );
};

export default MindInspector;
