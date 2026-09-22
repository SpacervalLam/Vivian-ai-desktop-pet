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
import { BookOpen, House, LayoutGrid } from 'lucide-react';
import { openRoomWindow } from '../../utils/roomWindow';
import { reportInspectorNav } from '../../utils/inspectorAttention';
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
import PageErrorBoundary from './PageErrorBoundary';
import './MindInspector.css';
import './MindInspectorThemes.css';

type InspectorUiStyle = 'scrapbook' | 'minimal';

const UI_STYLE_STORAGE_KEY = 'vivian-mind-inspector-ui-style';

function readUiStyle(): InspectorUiStyle {
  try {
    return localStorage.getItem(UI_STYLE_STORAGE_KEY) === 'minimal' ? 'minimal' : 'scrapbook';
  } catch {
    return 'scrapbook';
  }
}

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
  const [navRevealed, setNavRevealed] = useState(false);
  const [pageParams, setPageParams] = useState<PageParams>({});
  const [uiStyle, setUiStyle] = useState<InspectorUiStyle>(readUiStyle);

  /* 「开关偏好」与「生效主题」是两个值，必须分开 —— 这是产品约束，不是样式细节：
     **极简只在工作页生效，其他页永远是手账。**

     理由：极简是给「工作」那一页的密集信息界面（代码 / 轨迹 / 对话）准备的，
     记忆 / 世界 / 画像三页是浏览型页面，保持手账本的手写纸感。

       · `uiStyle`          —— 用户的**偏好**。要持久化、要驱动开关自己的高亮、
                               在非工作页也不能被改写（否则切走一次偏好就没了）。
       · `effectiveUiStyle` —— 当下**真正渲染**的主题。只喂给三处 `data-ui-style` 标记
                               （窗口根 / `.mind-main` / `<body>`），别的地方一律不用它。

     所以「在记忆页看到的是手账」并不代表开关被改成了手账 —— 回到工作页，开关仍然是
     用户上次选的那一档。 */
  const isWorkPage = activeNav === 'code';
  const effectiveUiStyle: InspectorUiStyle = isWorkPage ? uiStyle : 'scrapbook';

  useEffect(() => {
    try {
      localStorage.setItem(UI_STYLE_STORAGE_KEY, uiStyle);
    } catch {
      /* localStorage may be unavailable in hardened webviews */
    }
  }, [uiStyle]);

  /* 把主题标记镜像到 <body> 上。
     原因：本模块里有一批弹层是 **portal 到 document.body** 的（选区浮卡、就地改写卡、
     输入框气泡、页签右键菜单、会话右键菜单 / 弹窗），它们逃出了 .mind-main ——
     而主题标记 data-ui-style 只挂在 .mind-main 上。结果这些弹层在极简模式下拿到的
     仍是 .codex-theme 的默认调色板（手账暖纸），表现为「页面是灰白的、浮卡却是奶油纸」。
     body 是内联内容与 portal 的唯一共同祖先，所以标记只能挂这里；
     MindInspectorThemes.css 里对应的选择器写成 body[data-ui-style="minimal"] :is(...)。

     仍然由**一处**状态驱动（下面三处标记全部读 effectiveUiStyle），不会漂移；
     卸载时清掉，避免属性泄漏到别的窗口（MemoryWindow 也是 .codex-theme，
     不该吃到心智观察器的主题）。

     注意这里要跟着 `effectiveUiStyle` 走而不是 `uiStyle`：这批 portal 弹层
     （选区浮卡 / 就地改写卡 / 输入气泡 / 页签右键菜单 / 会话菜单）全都长在工作页上，
     工作页之外它们根本不会挂载，所以两者的实际取值一致；但写成 effectiveUiStyle
     才能保证「body 上的标记 = 页面上的标记」这条不变量**永远**成立 —— 否则哪天有
     弹层挪到别的页，它就会单独穿成极简。 */
  useEffect(() => {
    document.body.setAttribute('data-ui-style', effectiveUiStyle);
    return () => document.body.removeAttribute('data-ui-style');
  }, [effectiveUiStyle]);

  // 切换页面时刷新动画 key
  const [animKey, setAnimKey] = useState(0);
  useEffect(() => {
    setAnimKey((k) => k + 1);
  }, [activeNav]);

  // 把当前页签上报给后端：工作智能体卡在等用户拍板时，靠它判断用户看不看得见
  // 那条提问——用户停在别的页签时，工作页的 coding:question 监听器压根没挂载。
  // 这里必须挂在窗口级组件上（而非工作页组件），否则离开工作页就再也没人上报了。
  useEffect(() => {
    reportInspectorNav(activeNav);
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

  // 3D 公寓入口：独立全屏窗口，与主窗口快捷键走同一入口、复用同一实例。
  //
  // 心智观察器是全屏置顶窗口，房间窗口不是——它只要还在屏上，房间就会被整个
  // 压在下面，首屏 loading 层一帧都露不出来。所以这里不是隐藏而是直接关掉本
  // 窗口，退出公寓后也不再恢复。
  //
  // openRoomWindow 内部保证「关闭自己是最后一步」：本窗口销毁后，这个 JS 上下文
  // 里未返回的 IPC 会全部丢失，顺序反了房间窗口就再也 show 不出来。
  const openApartment = useCallback(() => {
    void openRoomWindow(t('room.title', { defaultValue: '公寓' }), {
      closeInspector: true,
    });
  }, [t]);

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
      {/* 主题标记挂**两处**：窗口根（下面这个 data-ui-style）与 .mind-main（再往下几行）。
          根上这一处是给「窗口外壳」用的 —— 封面条、贴纸导航卡、页内 Tab 栏都长在
          .mind-main **外面**（前两个甚至是它的**祖先**），而 CSS 的后代选择器只能向下
          找，够不着祖先。所以外壳的极简样式只能靠挂在根上的标记来选，
          见 MindInspectorThemes.css 的「极简主题：窗口外壳」一节。
          两处同源同值（都来自 effectiveUiStyle），不会漂移。

          注意喂进去的是 **effectiveUiStyle** 而不是 uiStyle：极简只在工作页生效，
          其他页这三处标记一律写回 "scrapbook"（含 `<body>` 那处，共三处标记），
          于是整窗（外壳 + 内容 + 弹层）一起回到手账，不存在「一半极简一半手账」。 */}
      <div
        className={`codex-theme mind-inspector-root mind-scrapbook-window${isWorkPage ? ' is-work-page' : ''}`}
        data-ui-style={effectiveUiStyle}
        data-nav-revealed={navRevealed ? 'true' : 'false'}
      >
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
            {/* 主题开关本身也只在工作页出现 —— 与「极简只在工作页生效」是同一条约束的两面：
                开关就是「这一页的显示方式」，放在别的页上既没用又会让人以为切换失灵。
                高亮读的是 **uiStyle**（偏好），不是 effectiveUiStyle：开关只在工作页存在，
                而工作页上两者恒等；读偏好能保证「切走再切回来」时开关状态不丢。 */}
            {isWorkPage && (
              <div
                className="mind-ui-style-switch"
                role="group"
                aria-label={t('mind_inspector.ui_style_label')}
              >
                <button
                  type="button"
                  className={uiStyle === 'scrapbook' ? 'active' : ''}
                  aria-pressed={uiStyle === 'scrapbook'}
                  title={t('mind_inspector.ui_style_scrapbook')}
                  onClick={() => setUiStyle('scrapbook')}
                >
                  <BookOpen size={13} strokeWidth={1.9} />
                  <span>{t('mind_inspector.ui_style_scrapbook')}</span>
                </button>
                <button
                  type="button"
                  className={uiStyle === 'minimal' ? 'active' : ''}
                  aria-pressed={uiStyle === 'minimal'}
                  title={t('mind_inspector.ui_style_minimal')}
                  onClick={() => setUiStyle('minimal')}
                >
                  <LayoutGrid size={13} strokeWidth={1.9} />
                  <span>{t('mind_inspector.ui_style_minimal')}</span>
                </button>
              </div>
            )}
            <button
              type="button"
              onClick={openApartment}
              title={t('mind_inspector.action_apartment')}
              className="mind-sb-cover-apartment"
            >
              <House size={14} strokeWidth={2.1} />
              <span>{t('mind_inspector.action_apartment')}</span>
            </button>
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
          <div
            className={`mind-nav-dock${navRevealed ? ' is-revealed' : ''}`}
            onMouseEnter={() => setNavRevealed(true)}
            onMouseLeave={() => setNavRevealed(false)}
            onFocusCapture={() => setNavRevealed(true)}
            onBlurCapture={(event) => {
              const next = event.relatedTarget;
              if (!(next instanceof Node) || !event.currentTarget.contains(next)) {
                setNavRevealed(false);
              }
            }}
          >
            <div className="mind-nav-hotspot" aria-hidden="true" />
            <aside className="mind-nav-rail">
              <div className="mind-nav-card">
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
          </div>

          {/* 右侧内容区 */}
          <main
            className={`mind-main${isWorkPage ? ' is-work-page' : ''}`}
            data-ui-style={effectiveUiStyle}
          >
              <div
                key={animKey}
                className="mind-page-content"
                style={{ animation: `mind-inspector-page-enter ${DURATION.slow}s ${EASE.ios}` }}
              >
                {/* 页面级错误边界：单页渲染崩溃时只显示兜底，不让整窗透明空白
                    （见 PageErrorBoundary）。key 随主导航变化，切页即重置崩溃态。 */}
                <PageErrorBoundary key={activeNav}>
                  {renderPage()}
                </PageErrorBoundary>
              </div>
          </main>
        </div>

      </div>
    </NavigationProvider>
  );
};

export default MindInspector;
