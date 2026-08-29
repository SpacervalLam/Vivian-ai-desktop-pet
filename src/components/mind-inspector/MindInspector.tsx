/**
 * Mind Inspector 壳组件
 *
 * Large Title 大标题 + 浮动胶囊侧边栏 + 页面切换动画。
 * 侧边栏导航在 8 个页面组件之间切换，激活态采用填充背景 + 顶部 accent 高亮线。
 */

import React, { useState, useMemo, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  EASE,
  DURATION,
  NAV_ITEMS,
  type NavKey,
  type NavItem,
} from './design-system';
import { NavigationProvider } from './NavigationContext';
import type { PageParams } from './NavigationContext';
import OverviewPage from './pages/OverviewPage';
import JournalPage from './pages/JournalPage';
import CodeAgentPage from './pages/CodeAgentPageNew';
import { invalidatePastelCache } from './pages/GraphPage';
import './MindInspector.css';

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

/** 侧边栏单个导航按钮（清新手账贴纸胶囊：图标 + 文字标签） */
const NavButton: React.FC<{
  item: NavItem;
  active: boolean;
  onClick: () => void;
}> = ({ item, active, onClick }) => {
  const { t } = useTranslation();
  const Icon = item.icon;
  return (
    <button
      type="button"
      title={t(item.labelKey)}
      onClick={onClick}
      className={`mind-nav-btn ${active ? 'active' : ''}`}
    >
      {active && <span className="mind-nav-active-bar" />}
      <Icon size={20} strokeWidth={active ? 2.2 : 1.8} />
      <span className="mind-nav-label">{t(item.labelKey)}</span>
    </button>
  );
};

const MindInspector: React.FC = () => {
  const { t } = useTranslation();
  const [activeNav, setActiveNav] = useState<NavKey>('overview');
  const [pageParams, setPageParams] = useState<PageParams>({});

  // 切换页面时刷新动画 key
  const [animKey, setAnimKey] = useState(0);
  useEffect(() => {
    setAnimKey((k) => k + 1);
  }, [activeNav]);

  // 挂载时刷新 pastel 主题色缓存，应对用户切换主题后重新打开 Mind Inspector 的场景
  useEffect(() => {
    invalidatePastelCache();
  }, []);

  // 合并前子视图跳转 → 合并页主键 + pageParams.sub。兼容 GraphPage → diary、MindPage → graph 等内部跳转。
  const resolveNav = (page: NavKey, params?: PageParams): { key: NavKey; params: PageParams } => {
    const base = params ?? {};
    switch (page) {
      case 'mind':
      case 'world':
      case 'graph':
      case 'profile':
        return { key: 'overview', params: { ...base, sub: page } };
      case 'diary':
      case 'notebook':
      case 'todo':
      case 'scheduler':
        return { key: 'journal', params: { ...base, sub: page } };
      default:
        return { key: page, params: base };
    }
  };

  const navigateTo = (page: NavKey, params?: PageParams) => {
    const { key, params: resolved } = resolveNav(page, params);
    setActiveNav(key);
    setPageParams(resolved);
  };

  // 读取 URL 参数（首次打开窗口时定位笔记/日记/子视图）+ 监听 memory:navigate 事件
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const params = new URLSearchParams(window.location.search);
    const nbId = params.get('nb_id');
    const nbChar = params.get('nb_char');
    const navParam = params.get('nav');

    if (nbId) {
      navigateTo('notebook', {
        notebookId: nbId,
        notebookCharacter: (nbChar as 'vivian' | 'nana') || 'vivian',
      });
    } else if (navParam && (['mind', 'world', 'graph', 'profile', 'diary', 'notebook', 'todo', 'scheduler']).includes(navParam)) {
      navigateTo(navParam as NavKey, {});
    }
    void (async () => {
      unlisten = await listen<{ page: string; notebookId?: string; notebookCharacter?: string; diaryId?: string; diaryCharacter?: string }>(
        'memory:navigate',
        (e) => {
          const p = e.payload;
          if ((p.page === 'notebook' && p.notebookId) || p.page === 'diary' || p.page === 'todo' || p.page === 'scheduler') {
            navigateTo(p.page as NavKey, {
              notebookId: p.notebookId,
              notebookCharacter: (p.notebookCharacter as 'vivian' | 'nana') || 'vivian',
              diaryId: p.diaryId,
              diaryCharacter: (p.diaryCharacter as 'vivian' | 'nana') || 'vivian',
            });
          }
        },
      );
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  const clearPageParams = () => {
    setPageParams({});
  };

  // 封面条副标题：跟随当前主导航，显示具体页面名
  const coverLabelKey = (() => {
    switch (activeNav) {
      case 'journal':
        return 'mind_inspector.nav_journal';
      case 'code':
        return 'mind_inspector.nav_code';
      default:
        return 'mind_inspector.nav_overview';
    }
  })();

  // 窗口控制（封面条右侧按钮）：本组件仅用于 MemoryWindow，操作当前窗口
  const minimizeWindow = useCallback(async () => {
    try {
      await getCurrentWindow().minimize();
    } catch {
      /* ignore */
    }
  }, []);

  const closeWindow = useCallback(async () => {
    try {
      await getCurrentWindow().close();
    } catch {
      /* ignore */
    }
  }, []);

  // 页面可注入共享标题行工具栏（切换页面时自动清空）
  const [headerExtra, setHeaderExtra] = useState<React.ReactNode>(null);
  useEffect(() => {
    setHeaderExtra(null);
  }, [activeNav]);

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
      case 'overview':
      // 兼容旧值直接命中（正常已被 resolveNav 映射，兜底）
      case 'mind':
      case 'world':
      case 'graph':
      case 'profile':
        return <OverviewPage />;
      case 'journal':
      case 'diary':
      case 'notebook':
      case 'todo':
      case 'scheduler':
        return <JournalPage />;
      case 'code':
        return <CodeAgentPage />;
      default:
        return null;
    }
  };

  return (
    <NavigationProvider value={navContext}>
      <div className="codex-root mind-scrapbook-window">
        {/* 手账本封面条（全局标题 + 窗口拖拽区 + 最小化/关闭按钮） */}
        <header className="mind-sb-cover">
          <div className="mind-sb-cover-title" data-tauri-drag-region>
            <span className="mind-sb-cover-dot" />
            Mind Scrapbook
            <span className="mind-sb-cover-divider" />
            <span className="mind-sb-cover-sub">{t(coverLabelKey)}</span>
          </div>
          <div className="mind-sb-cover-extra">
            {headerExtra}
            <span className="mind-sb-cover-seal">
              {new Date().toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })}
            </span>
            <div className="mind-sb-cover-actions">
              <button
                onClick={() => void minimizeWindow()}
                title={t('common.minimize')}
                className="mind-sb-cover-btn"
              >
                <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
                  <path
                    d="M2.5 6H9.5"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                  />
                </svg>
              </button>
              <button
                onClick={() => void closeWindow()}
                title={t('common.close')}
                className="mind-sb-cover-btn close"
              >
                <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
                  <path
                    d="M3.5 3.5L8.5 8.5M8.5 3.5L3.5 8.5"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                  />
                </svg>
              </button>
            </div>
          </div>
        </header>

        {/* 左侧贴纸导航栏 */}
        <div className="mind-sb-body">
          <aside className="mind-nav-rail">
            <div className="mind-nav-card sb-tape">
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
          <main className="mind-main">
            <div
              key={animKey}
              className="mind-page-content"
              style={{ animation: `mind-inspector-page-enter ${DURATION.slow}s ${EASE.ios}` }}
            >
              {renderPage()}
            </div>
          </main>
        </div>
      </div>
    </NavigationProvider>
  );
};

export default MindInspector;
