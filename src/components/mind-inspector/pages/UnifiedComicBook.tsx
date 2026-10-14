import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from 'react';
import { BookOpen, CalendarDays, ChevronLeft, ChevronRight, Maximize2, MessageCircle, Minimize2, NotebookPen, RefreshCw } from 'lucide-react';
import type { NavKey } from '../design-system';
import type { PageParams } from '../NavigationContext';
import PageErrorBoundary from '../PageErrorBoundary';
import MindPage from './MindPage';
import MemoryPage from './MemoryPage';
import DiaryPage from './DiaryPage';
import NotebookPage from './NotebookPage';
import PlannerPage from './PlannerPage';
import { bookDestination, bookSpreads, bookSlots, bookFrame, adjacentBookSpread, type ComicSource } from './comicBookStructure';
import { notebookPaperLayout, paperWritingStyle } from './paperPerspective';
import { startComicPaperLoop } from './comicPaperLoop';
import { hasRunningComicAnimation, nextTourPage } from './comicTour';
import './UnifiedComicBook.css';
import { useTranslation } from 'react-i18next';

const sources: ComicSource[] = ['memory', 'diary', 'notebook', 'planner'];
const panelLabelKeys = { memory: 'panelMemory', diary: 'panelDiary', notebook: 'panelNotebook', planner: 'panelPlanner' };
const sourceLabelKeys: Record<ComicSource, string> = { memory: 'comic_nav_memory', diary: 'comic_nav_diary', notebook: 'comic_nav_notebook', planner: 'comic_nav_planner' };
const sourceIcons = { memory: MessageCircle, diary: BookOpen, notebook: NotebookPen, planner: CalendarDays };
const readingLabelKeys = { memory: 'readingMemory', diary: 'readingDiary', notebook: 'readingNotebook', planner: 'readingPlanner' };
/* 展开态的底纹美术，按源声明而不是靠文件名拼出来：另外三个源用的是一张「信纸」
   纸质图，压在右下角当档案装饰；memory 没有信纸，改用一整张插画铺满面板当底纹。
   两者铺法不同（角标 vs 满幅），所以各自带类名。 */
const expandedArtwork: Record<ComicSource, { file: string; className: string }> = {
  memory: { file: 'memory-backdrop-v43.webp', className: 'unified-expanded-backdrop' },
  diary: { file: 'diary-paper-v35.webp', className: 'unified-expanded-decoration' },
  notebook: { file: 'notebook-paper-v35.webp', className: 'unified-expanded-decoration' },
  planner: { file: 'planner-nana-backdrop-v44.webp', className: 'unified-expanded-backdrop' },
};
const paperArtwork = { diary: 'diary-paper-v35.webp', notebook: 'notebook-paper-v35.webp', planner: 'planner-paper-v35.webp' };

export default function UnifiedComicBook({ destination, params, presentationReady = true, onTourChange }: { destination: NavKey; params: PageParams; presentationReady?: boolean; onTourChange?: (touring: boolean) => void }) {
  const { t } = useTranslation();
  const labels = Object.fromEntries(sources.map(source => [source, t(`comic.${panelLabelKeys[source]}`)])) as Record<ComicSource, string>;
  const readingLabels = Object.fromEntries(sources.map(source => [source, t(`comic.${readingLabelKeys[source]}`)])) as Record<ComicSource, string>;
  const initial = bookDestination(destination, params);
  const [page, setPage] = useState(initial.spread);
  const [expanded, setExpanded] = useState<ComicSource | null>(initial.focus);
  const [memoryRefreshToken, setMemoryRefreshToken] = useState(0);
  const [memoryLoading, setMemoryLoading] = useState(false);
  const [contentsOpen, setContentsOpen] = useState(false);
  const [touring, setTouring] = useState(false);
  const tourStartedAt = useRef(0);
  const animationFinishedAt = useRef<number | null>(null);
  const [bounds, setBounds] = useState({ left: 0, top: 0, width: 0, height: 0, mobile: false });
  const root = useRef<HTMLDivElement>(null);
  const pageRef = useRef(page);
  const expandedRef = useRef(expanded);
  pageRef.current = page;
  expandedRef.current = expanded;
  const lastInteraction = useRef(Date.now());
  const drag = useRef<{ id: number; x: number; y: number; dx: number; horizontal: boolean } | null>(null);
  // 翻页拖拽结束时浏览器还会补一个 click。存时间戳而不是布尔：万一那次 click 没来，
  // 也不会一直挂着把后面某个键盘触发的 click 吞掉。
  const suppressClick = useRef(0);
  const lastRoute = useRef('');
  const lastDestination = useRef(destination);
  const messageWheel = useRef({ delta: 0, lockedUntil: 0, last: 0 });
  const panelMotion = useRef<Animation>();
  const writingMotion = useRef<Animation>();
  const pendingPanelMotion = useRef<{ source: ComicSource; from: DOMRect } | null>(null);
  const scene = bookSpreads[page];
  useLayoutEffect(() => {
    const node = root.current;
    const stage = node?.querySelector<HTMLElement>('.mind-comic-book .comic');
    if (!node || !stage) return;
    const measure = () => {
      const frame = stage.getBoundingClientRect();
      const outer = node.getBoundingClientRect();
      if (!frame.width || !frame.height) return;
      const next = { left: frame.left - outer.left, top: frame.top - outer.top, width: frame.width, height: frame.height, mobile: stage.classList.contains('is-mobile') };
      setBounds(old => JSON.stringify(old) === JSON.stringify(next) ? old : next);
    };
    measure();
    const resize = new ResizeObserver(measure);
    resize.observe(stage); resize.observe(node);
    node.addEventListener('scroll', measure, true);
    const frame = requestAnimationFrame(measure);
    return () => { resize.disconnect(); cancelAnimationFrame(frame); node.removeEventListener('scroll', measure, true); };
  }, [page, expanded]);

  const jump = useCallback((next: number) => {
    pageRef.current = next;
    setPage(next);
    setExpanded(null);
    setContentsOpen(false);
    lastInteraction.current = Date.now();
  }, []);

  // Old URLs and cross-page jumps open their source inside this same book.
  // Clearing a consumed detail parameter must not reset the page again.
  useEffect(() => {
    const key = JSON.stringify([destination, params]);
    const changed = destination !== lastDestination.current;
    lastDestination.current = destination;
    if (key === lastRoute.current || (!changed && lastRoute.current && !Object.keys(params).length)) return;
    lastRoute.current = key;
    const target = bookDestination(destination, params);
    jump(target.spread);
    setExpanded(target.focus);
  }, [destination, params, jump]);

  const turn = useCallback((direction: number) => {
    if (expandedRef.current || root.current?.querySelector('.rec-overlay')) return;
    const next = adjacentBookSpread(pageRef.current, direction);
    if (next === pageRef.current) return;
    pageRef.current = next;
    setPage(next);
    lastInteraction.current = Date.now();
  }, []);

  /* 整本书只有一条拖拽通路，就挂在这个根节点（.unified-comic-book）上。
     画格（.mind-comic-book .comic）和四个功能面板（.unified-function-slots 里的
     article.unified-functional-panel）其实是同一张纸的两层：画格在下，面板浮在上。
     面板层原本是 pointer-events: auto 的独立命中面，按在面板上事件根本到不了画格，
     于是就成了「按在面板上拖不动整页」——第 7 页右侧那列记忆对话最明显。

     现在由根节点统一接管：只要不是按在真正的控件上，从画格起手和从面板起手走的是
     同一段拖拽。画格吃 transform，面板吃 --comic-drag-x（left 的 calc 加数，见
     UnifiedComicBook.css），两边挂同一条 .22s ease-out，拖拽中与松手回弹都严丝合缝。

     面板不能改用 transform 跟随：面板里挂着 .rec-overlay（position: fixed 的全屏弹窗），
     祖先一旦有 transform，fixed 就会改挂到那个祖先上，弹窗会缩成面板大小。 */
  const dragSettle = useRef<number>();
  const artStage = () => root.current?.querySelector<HTMLElement>('.mind-comic-book .comic') ?? null;
  const beginFollow = () => {
    window.clearTimeout(dragSettle.current);
    root.current?.querySelectorAll<HTMLElement>('.unified-functional-panel:not(.is-expanded)')
      .forEach(panel => { panel.style.transition = 'left .22s ease-out'; });
  };
  const setFollow = (offset: number) => { root.current?.style.setProperty('--comic-drag-x', `${offset}px`); };
  const endFollow = (immediate: boolean) => {
    const node = root.current;
    if (!node) return;
    node.style.removeProperty('--comic-drag-x');
    const release = () => {
      node.querySelectorAll<HTMLElement>('.unified-functional-panel')
        .forEach(panel => { panel.style.transition = ''; });
    };
    window.clearTimeout(dragSettle.current);
    // 翻页会让书重排（面板换槽位），这时不能再留过渡，否则面板会从旧槽位滑过去。
    if (immediate) release();
    else dragSettle.current = window.setTimeout(release, 240);
  };
  useEffect(() => {
    const node = root.current;
    return () => {
      window.clearTimeout(dragSettle.current);
      // 拖拽中途卸载（切页 / 关窗）：别把偏移留在书根上，否则面板会一直歪着。
      node?.style.removeProperty('--comic-drag-x');
    };
  }, []);

  const finishDrag = (event: ReactPointerEvent, cancelled = false) => {
    const state = drag.current;
    if (!state || state.id !== event.pointerId) return;
    drag.current = null;
    const node = event.currentTarget as HTMLElement;
    if (node.hasPointerCapture(event.pointerId)) node.releasePointerCapture(event.pointerId);
    const stage = artStage();
    if (stage) stage.style.transform = '';
    // 与上面清 transform 同一帧清掉 --comic-drag-x：画格与面板各自用同一条 .22s 曲线回弹。
    const turning = !cancelled && state.horizontal && Math.abs(state.dx) > Math.max(45, Math.min(100, (stage?.clientWidth ?? 0) * .08));
    endFollow(turning);
    lastInteraction.current = Date.now();
    if (turning) {
      suppressClick.current = Date.now();
      turn(state.dx < 0 ? 1 : -1);
    }
  };

  useEffect(() => {
    onTourChange?.(touring);
    return () => onTourChange?.(false);
  }, [touring, onTourChange]);

  useEffect(() => {
    if (!presentationReady) return;
    lastInteraction.current = Date.now();
    const activity = () => { lastInteraction.current = Date.now(); setTouring(false); };
    const events = ['pointermove', 'pointerdown', 'wheel', 'keydown', 'input', 'touchstart'] as const;
    events.forEach(event => window.addEventListener(event, activity, { passive: true, capture: true }));
    document.addEventListener('visibilitychange', activity);
    window.addEventListener('blur', activity);
    window.addEventListener('focus', activity);
    return () => {
      events.forEach(event => window.removeEventListener(event, activity, true));
      document.removeEventListener('visibilitychange', activity);
      window.removeEventListener('blur', activity);
      window.removeEventListener('focus', activity);
    };
  }, [presentationReady]);

  useEffect(() => { animationFinishedAt.current = null; }, [page, presentationReady, expanded]);

  useEffect(() => {
    if (!presentationReady) return;
    const timer = window.setInterval(() => {
      const node = root.current;
      const stage = node?.querySelector<HTMLElement>('.mind-comic-book .comic');
      const selection = window.getSelection();
      if (!node || !stage || document.hidden || !document.hasFocus() || expandedRef.current || drag.current ||
        node.querySelector('.is-editing, .rec-overlay, input:focus, textarea:focus, [contenteditable]:focus') ||
        (selection && !selection.isCollapsed && node.contains(selection.anchorNode))) {
        lastInteraction.current = Date.now();
        setTouring(false);
        return;
      }
      const now = performance.now();
      if (hasRunningComicAnimation(node.getAnimations({ subtree: true })) || now < Number(stage.dataset.scriptAnimationEnd ?? 0)) {
        animationFinishedAt.current = null;
      } else if (animationFinishedAt.current === null) {
        animationFinishedAt.current = now;
      }
      if (!touring) {
        if (Date.now() - lastInteraction.current >= 10000) {
          tourStartedAt.current = now;
          setContentsOpen(false);
          setTouring(true);
        }
        return;
      }
      if (animationFinishedAt.current !== null && now - Math.max(animationFinishedAt.current, tourStartedAt.current) >= 1000) {
        const next = nextTourPage(pageRef.current, bookSpreads.length);
        pageRef.current = next;
        animationFinishedAt.current = null;
        setPage(next);
      }
    }, 100);
    return () => clearInterval(timer);
  }, [presentationReady, touring]);

  const expand = (source: ComicSource | null) => {
    if (source === expandedRef.current) return;
    const moving = source ?? expandedRef.current;
    const panel = moving ? root.current?.querySelector<HTMLElement>(`.source-${moving}`) : null;
    const phone = source === 'memory' ? root.current?.querySelector<HTMLElement>('#line-phone') : null;
    pendingPanelMotion.current = panel ? { source: moving!, from: (phone ?? panel).getBoundingClientRect() } : null;
    setExpanded(source);
    lastInteraction.current = Date.now();
  };
  useLayoutEffect(() => {
    const motion = pendingPanelMotion.current;
    pendingPanelMotion.current = null;
    if (!motion || !root.current) return;
    panelMotion.current?.cancel();
    writingMotion.current?.cancel();
    const panel = root.current.querySelector<HTMLElement>(`.source-${motion.source}`);
    if (!panel || panel.hidden || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const phone = !expanded && motion.source === 'memory' ? root.current.querySelector<HTMLElement>('#line-phone') : null;
    const to = (phone ?? panel).getBoundingClientRect();
    const outer = getComputedStyle(panel).position === 'fixed' ? { left: 0, top: 0 } : root.current.getBoundingClientRect();
    const geometry = (r: DOMRect) => ({ left: `${r.left - outer.left}px`, top: `${r.top - outer.top}px`, width: `${r.width}px`, height: `${r.height}px`, right: 'auto', bottom: 'auto' });
    // Animate layout, not transform: fixed dialogs must remain window-sized.
    panelMotion.current = panel.animate([geometry(motion.from), geometry(to)], { duration: 420, easing: 'cubic-bezier(.22,.75,.25,1)' });
    const writing = panel.querySelector<HTMLElement>('.unified-paper-writing');
    if (writing) writingMotion.current = writing.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 220, delay: expanded ? 100 : 200, fill: 'backwards' });
  }, [expanded]);
  useEffect(() => () => { panelMotion.current?.cancel(); writingMotion.current?.cancel(); }, []);
  useEffect(() => {
    if (expanded || scene.sources.includes('notebook')) return;
    const source = scene.sources.find(key => key !== 'memory');
    const panel = source ? root.current?.querySelector<HTMLElement>(`.source-${source}`) : null;
    if (!panel) return;
    const selector = source === 'diary' ? '.record-diary-reader' : '.record-plan-list';
    let viewport: HTMLElement | null = null;
    let stop: (() => void) | undefined;
    const connect = () => {
      const next = panel.querySelector<HTMLElement>(selector);
      if (next === viewport) return;
      stop?.(); viewport = next;
      stop = next ? startComicPaperLoop(next) : undefined;
    };
    connect();
    const observer = new MutationObserver(connect);
    observer.observe(panel, { childList: true, subtree: true });
    return () => { observer.disconnect(); stop?.(); };
  }, [page, expanded]);
  useEffect(() => {
    const node = root.current;
    const preventSelection = (event: Event) => {
      const target = event.target instanceof Element ? event.target : (event.target as Node | null)?.parentElement;
      if (!target?.closest('input,textarea,[contenteditable="true"]')) event.preventDefault();
    };
    node?.addEventListener('selectstart', preventSelection);
    return () => node?.removeEventListener('selectstart', preventSelection);
  }, []);
  const activeSources = new Set(scene.sources ?? []);

  return <div ref={root} className={`unified-comic-book record-scope${expanded ? ' has-expanded-panel' : ''}`} data-spread={page} data-touring={touring} tabIndex={0}
    aria-label={t('comic.pageLabel', { title: t(`comic.title${scene.mind}`), page: page + 1, total: bookSpreads.length })}
    onKeyDown={event => {
      if ((event.target as Element).closest('#line-phone') && (event.key === 'Enter' || event.key === ' ')) {
        event.preventDefault(); expand('memory'); return;
      }
      if (event.key === 'Escape') { setContentsOpen(false); expand(null); return; }
      if (event.defaultPrevented || (event.target as Element).closest('input,textarea,select,button,a,[contenteditable],.unified-functional-panel')) return;
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); turn(event.key === 'ArrowLeft' ? -1 : 1); }
    }} onWheel={event => {
      // Overlay events bubble to the book just like wheel events over artwork.
      if (expanded || !(event.target as Element).closest('.comic-latest-view,.unified-panel-open')) return;
      const state = messageWheel.current;
      const now = Date.now();
      if (now < state.lockedUntil) return;
      if (now - state.last > 150) state.delta = 0;
      state.last = now;
      state.delta += Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
      if (Math.abs(state.delta) >= 36) {
        turn(state.delta > 0 ? 1 : -1);
        state.delta = 0;
        state.lockedUntil = now + 620;
      }
    }} onClick={event => {
      if (!expanded && (event.target as Element).closest('#line-phone')) expand('memory');
    }} onClickCapture={event => {
      // 翻页那次拖拽结束时浏览器还会补一个 click，别让它再点到底下的东西。
      if (Date.now() - suppressClick.current > 400) return;
      suppressClick.current = 0;
      event.preventDefault();
      event.stopPropagation();
    }} onPointerDown={event => {
      lastInteraction.current = Date.now();
      suppressClick.current = 0;
      if (!event.isPrimary || event.button !== 0 || expanded) return;
      // 画格与面板一起拖，但真正的控件、可编辑区、横向滚动条、目录抽屉和全屏弹层不吃这一下。
      if (!(event.target as Element).closest('.unified-panel-open') && (event.target as Element).closest('button,a,input,textarea,select,[contenteditable],[role="button"],[role="tab"],summary,label,.is-editing,.rec-overlay,.unified-book-contents,.record-diary-index-scroll,.record-note-index-scroll')) return;
      drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY, dx: 0, horizontal: false };
    }} onPointerMove={event => {
      const state = drag.current;
      if (!state || state.id !== event.pointerId) return;
      state.dx = event.clientX - state.x;
      if (!state.horizontal && Math.abs(state.dx) > 12 && Math.abs(state.dx) > Math.abs(event.clientY - state.y) * 1.3) {
        state.horizontal = true;
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
        beginFollow();
      }
      if (!state.horizontal) return;
      event.preventDefault();
      const offset = Math.max(-38, Math.min(38, state.dx * .15));
      const stage = artStage();
      if (stage) stage.style.transform = `translateX(${offset}px)`;
      setFollow(offset);
    }} onPointerUp={event => finishDrag(event)} onPointerCancel={event => finishDrag(event, true)}>

    <div className="unified-mind-spread mind-overview-surface" hidden={!!expanded}>
      <MindPage pageIndex={scene.mind} onTurn={turn} active={presentationReady && !expanded} functional />
    </div>

    <div className="unified-function-slots">
      {/* One mounted owner per source: flips and expansion never discard drafts,
          selection, filters or forms, and never duplicate conversation IDs. */}
      {sources.map(source => {
        const [x, y, w, h] = bookSlots[source][bounds.mobile ? 'm' : 'r'];
        // 横向位置走 CSS 变量而不是 left：拖拽时书根会写 --comic-drag-x，面板的 left 用
        // calc() 加上它，画格平移时面板（第 7 页右侧那列记忆对话最明显）才会跟着一起动。
        // 详见本文件上方「整本书只有一条拖拽通路」那段注释。
        const placement: CSSProperties | undefined = expanded === source ? undefined : { '--panel-left': `${bounds.left + bounds.width * x / 100}px`, top: bounds.top + bounds.height * y / 100, width: bounds.width * w / 100, height: bounds.height * h / 100, '--panel-width': `${bounds.width * w / 100}px` } as CSSProperties;
        const frame = bookFrame(source, bounds.mobile);
        const paperImage = source === 'notebook' ? notebookPaperLayout(bounds.width * w / 100, bounds.height * h / 100) : { x: 0, y: 0, width: 100, height: 100 };
        // Reading surfaces follow their illustrated paper; expansion restores
        // normal coordinates for calendars, dialogs and the complete editor.
        const projected = source !== 'memory' && !expanded;
        const writingStyle = source !== 'memory' && projected ? paperWritingStyle(source, bounds.width * w / 100, bounds.height * h / 100) : undefined;
        return <article key={source} style={placement} className={`unified-functional-panel claude-theme source-${source}${expanded === source ? ' is-expanded' : ''}`}
        data-source={source} hidden={expanded ? expanded !== source : !activeSources.has(source)}>
        {source !== 'memory' && <>
        <svg className="unified-panel-gutter" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <polygon className="panel-gutter" points={frame} />
        </svg>
        <svg className="unified-panel-ink" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <defs><clipPath id={`paper-cut-${source}`}><polygon points={frame} /></clipPath></defs>
          <image href={`${import.meta.env.BASE_URL}mind-comic/${paperArtwork[source]}`} {...paperImage} preserveAspectRatio={source === 'notebook' ? 'none' : 'xMidYMid slice'} clipPath={`url(#paper-cut-${source})`} />
          {/* Match ordinary shots: their polygon crop hides the outer half of the stroke. */}
          <polygon className="panel-outline illustrated-outline" points={frame} clipPath={`url(#paper-cut-${source})`} />
        </svg>
        </>}
        {expanded === source && <img className={expandedArtwork[source].className} src={`${import.meta.env.BASE_URL}mind-comic/${expandedArtwork[source].file}`} alt="" aria-hidden="true" />}
        <div className="unified-writing-mask">
        <div className={`unified-paper-writing${projected ? ' is-projected' : ''}`} style={writingStyle}>
        <header className="unified-panel-heading">
          <h2>{expanded === source && <small className="unified-reading-kicker">{readingLabels[source]}</small>}{labels[source]}</h2>
          {(source === 'memory' || expanded === source) && <>
          {source === 'memory' && <button type="button" onClick={() => setMemoryRefreshToken(value => value + 1)} disabled={memoryLoading} aria-label={t('comic.refresh')} title={t('comic.refresh')}>
            <RefreshCw size={16} className={memoryLoading ? 'spinning' : ''} />
          </button>}
          <button type="button" onClick={() => expand(expanded === source ? null : source)} aria-label={t(expanded === source ? 'comic.collapse' : 'comic.expand', { name: labels[source] })}>
            {expanded === source ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
          </button>
          </>}
        </header>
        <div className="unified-panel-content">
          <PageErrorBoundary>
            {source === 'memory' ? <MemoryPage embedded refreshToken={memoryRefreshToken} onLoadingChange={setMemoryLoading} initialLayer={params.sub === 'profile' ? 'profile' : 'recent'} latestOnly={expanded !== 'memory'} active={activeSources.has('memory') && !expanded} /> :
              source === 'diary' ? <DiaryPage embedded expanded={expanded === source} /> : source === 'notebook' ? <NotebookPage embedded compact={expanded !== source} /> : <PlannerPage embedded expanded={expanded === source} />}
          </PageErrorBoundary>
        </div>
        </div>
        </div>
        {projected && <button className="unified-panel-open" type="button" aria-label={t('comic.expand', { name: labels[source] })} onClick={() => expand(source)} />}
      </article>;
      })}
    </div>

    {!expanded && <>
      <button className="unified-turn is-back" style={{ width: bounds.left }} type="button" onClick={() => turn(-1)} aria-label={t('comic.previous')}><ChevronLeft size={23} /></button>
      <button className="unified-turn is-next" style={{ left: bounds.left + bounds.width, width: `calc(100% - ${bounds.left + bounds.width}px)` }} type="button" onClick={() => turn(1)} aria-label={t('comic.next')}><ChevronRight size={23} /></button>
    </>}

    {contentsOpen && <div className="unified-contents-backdrop" aria-hidden="true" />}
    <div aria-hidden={touring} className={`unified-menu-dock${contentsOpen ? ' is-open' : ''}${touring ? ' is-touring' : ''}`}
      onPointerDown={event => event.stopPropagation()}
      onMouseLeave={() => setContentsOpen(false)}
      onBlurCapture={event => {
        if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setContentsOpen(false);
      }}>
    <div className="unified-book-corner">
      <button type="button" disabled={touring} onMouseEnter={() => setContentsOpen(true)} onFocus={() => setContentsOpen(true)} aria-expanded={contentsOpen} aria-controls="comic-book-contents" aria-label="CONTENT"><BookOpen size={18} /></button>
    </div>
    {contentsOpen && <aside id="comic-book-contents" className="unified-book-contents" aria-label="CONTENT">
      <header><span className="unified-contents-mark" aria-hidden="true"><BookOpen size={23} strokeWidth={1.5} /></span><div><small>Vivian & Nana</small><strong>CONTENT</strong></div></header>
      <div className="unified-contents-caption">{t('comic.browse')}</div>
      <nav aria-label={t('comic.destinations')} className="unified-source-links">
        {sources.map(source => { const Icon = sourceIcons[source]; return <button key={source} type="button" onClick={() => {
          const index = bookDestination(({ memory: 'graph', diary: 'diary', notebook: 'notebook', planner: 'planner' })[source]).spread;
          jump(index); expand(source);
        }}><Icon size={15} strokeWidth={1.6} />{t(sourceLabelKeys[source])}</button>; })}
      </nav>
      <div className="unified-contents-caption">{t('comic.chapters')}</div>
      <ol>{bookSpreads.map((spread, index) => <li key={index}><button type="button" onClick={() => jump(index)} aria-current={page === index ? 'page' : undefined}><span className="unified-contents-number">{String(index + 1).padStart(2, '0')}</span><span className="unified-contents-title">{t(`comic.title${spread.mind}`)}</span><ChevronRight size={14} aria-hidden="true" /></button></li>)}</ol>
    </aside>}
    </div>
  </div>;
}
