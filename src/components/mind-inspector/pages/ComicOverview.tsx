import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { comicMarkup } from './comicMarkup';
import { bookSpreads, adjacentBookSpread } from './comicBookStructure';
import './ComicOverview.css';
import { createPanelFloat, type PanelFloat } from './panelFloat';
import { applyComicPresentation, fitComicText, patchComicData, translateComicLabels } from './comicBind';
import { startFullPanelReveal, startInsetPanelReveal } from './comicPanelReveal';
import { startMorningInkReveal } from './morningInkReveal';
import { startRainTypewriter } from './rainTypewriter';
import { comicScriptDuration, handSceneTiming, handInsetSettledAt } from './comicTour';
import { useTranslation } from 'react-i18next';

export interface ComicCharacter {
  status?: 'loading' | 'ready'; name: string; primaryEmotion: string; presence: string; thought: string; goal: string; mood: string;
  valence: number | null; arousal: number | null; error: string | null;
  emotions: Array<[string, number | string]>;
  needs: Array<[string, number | string]>;
  relations: Array<[string, number | string]>;
}

export interface ComicOverviewControl { pageIndex?: number; onTurn?: (direction: number) => void; active?: boolean; functional?: boolean }
export default function ComicOverview({ data, pageIndex, onTurn, active = true, functional = false }: { data: Record<'vivian' | 'nana', ComicCharacter> } & ComicOverviewControl) {
  const { t, i18n } = useTranslation();
  const [localPage, setPage] = useState(bookSpreads[0].mind);
  const page = pageIndex ?? localPage;
  const [revision, setRevision] = useState(0);
  const stage = useRef<HTMLDivElement>(null);
  const current = useRef(bookSpreads[0].mind);
  const dragging = useRef<{ id: number; x: number; y: number; dx: number; horizontal: boolean } | null>(null);
  const lastManual = useRef(0);
  const suppressClick = useRef(false);
  const animatedRevision = useRef('');
  const hoverFloats = useRef(new Map<string, PanelFloat>());
  const handTimer = useRef<number>();
  const handHoldTimer = useRef<number>();
  const handStartedAt = useRef(0);
  const handState = useRef<'open' | 'gripping' | 'held' | 'releasing'>('open');
  const handReady = useRef(false);
  const mobileRef = useRef(false);
  const wheelAccum = useRef(0);
  const wheelLock = useRef(0);
  const wheelIdle = useRef<number>(0);
  /* 拖拽归谁管：functional 时这张画格是 UnifiedComicBook 的一层纸，整本书（画格 +
     四个功能面板）的拖拽统一由那边的书根接管 —— 面板浮在画格上面，按在面板上根本
     到不了这里，只有把通路提到共同祖先上才能「从面板上把整页拖走」。
     所以这里只保留非书页（独立画格）时的自带拖拽，functional 下让位。
     跟随偏移 --comic-drag-x 与面板的 left 过渡现在都写在 UnifiedComicBook.tsx。 */
  function syncHand() {
    const panel = stage.current?.querySelector<HTMLElement>('#skirt-hand');
    if (!panel) return;
    panel.classList.remove('hand-playing','hand-gripping','hand-held','hand-releasing');
    const frames = panel.querySelector<HTMLElement>('.hand-frames');
    if (frames) frames.style.animationDelay = '';
    if (handState.current === 'held') panel.classList.add('hand-held');
    if (handState.current === 'gripping' || handState.current === 'releasing') {
      if (frames) frames.style.animationDelay = `-${Math.min(719, performance.now() - handStartedAt.current)}ms`;
      panel.classList.add(`hand-${handState.current}`);
    }
  }
  function startHand(direction: 'gripping' | 'releasing') {
    if (!stage.current?.querySelector('#skirt-hand')) return;
    window.clearTimeout(handTimer.current);
    window.clearTimeout(handHoldTimer.current);
    handState.current = direction;
    handStartedAt.current = performance.now();
    syncHand();
    handTimer.current = window.setTimeout(() => {
      handState.current = direction === 'gripping' ? 'held' : 'open';
      syncHand();
      if (direction === 'gripping') handHoldTimer.current = window.setTimeout(() => startHand('releasing'), handSceneTiming.hold);
    }, direction === 'gripping' ? handSceneTiming.grip : handSceneTiming.release);
  }
  function playHand() {
    if (!handReady.current) return;
    if (handState.current === 'open') startHand('gripping');
    else if (handState.current === 'held') startHand('releasing');
  }
  // 数据只进 ref、不进 memo 依赖：分镜结构按「页 + 翻页轮次」重建，心灵数据按 5 秒
  // 节流就地 patch（见 comicBind.ts）。否则每轮刷新都会整体替换 innerHTML——入场
  // 动画重放、图片重新解码、hover 漂浮与小手状态全部丢失。
  const dataRef = useRef(data);
  dataRef.current = data;
  const html = useMemo(() => comicMarkup(dataRef.current, page, `${import.meta.env.BASE_URL}mind-comic/`, revision, functional, i18n.resolvedLanguage ?? i18n.language), [page, revision, functional]);

  // Translate labels in place so a language change never replays the page animation.
  useLayoutEffect(() => {
    translateComicLabels(stage.current, key => t(key));
  }, [html, t, i18n.resolvedLanguage]);

  // 数据刷新只改文本节点，不动 DOM 结构
  useLayoutEffect(() => {
    patchComicData(stage.current, data, revision, i18n.resolvedLanguage ?? i18n.language);
  }, [data, revision, i18n.resolvedLanguage]);

  const turn = (direction: number, automatic = false) => {
    if (onTurn) { onTurn(direction); return; }
    const index = bookSpreads.findIndex(spread => spread.mind === current.current);
    const next = bookSpreads[adjacentBookSpread(index, direction)].mind;
    if (next === current.current) return;
    current.current = next;
    setPage(current.current);
    if (!automatic) lastManual.current = Date.now();
    setRevision(v => v + 1);
  };
  const turnRef = useRef(turn);
  turnRef.current = turn;

  useEffect(() => {
    if (pageIndex !== undefined) return;
    const timer = window.setInterval(() => {
      if (!document.hidden && !dragging.current && Date.now() - lastManual.current >= 28000 && !stage.current?.querySelector('button:focus')) turnRef.current(1, true);
    }, 28000);
    const visible = () => { lastManual.current = Date.now(); };
    document.addEventListener('visibilitychange', visible);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', visible); };
  }, [pageIndex]);

  useLayoutEffect(() => {
    const node = stage.current;
    if (!node) return;
    applyComicPresentation(node);
    const animationKey = `${page}:${revision}:${active}`;
    const entering = active && animatedRevision.current !== animationKey;
    handReady.current = !entering;
    if (entering) {
      node.dataset.scriptAnimationEnd = String(performance.now() + comicScriptDuration(page));
    }
    if (entering) {
      // Returning from the office remounts the book. Reset CSS playback too,
      // including React StrictMode's cleanup/setup replay of the same mount.
      node.classList.remove('entering');
      void node.offsetWidth;
      node.classList.add('entering');
    } else node.classList.remove('entering');
    if (entering) { hoverFloats.current.forEach(float => float.stop()); hoverFloats.current.clear(); }
    animatedRevision.current = animationKey;
    hoverFloats.current.forEach((float, id) => {
      const wrap = node.querySelector<HTMLElement>(`#${id} .image-wrap`);
      if (wrap) wrap.style.translate = `${float.position.x}px ${float.position.y}px`;
    });
    syncHand();
    let fullPanelReveal: ReturnType<typeof startFullPanelReveal> | undefined;
    let insetPanelReveal: ReturnType<typeof startInsetPanelReveal> | undefined;
    const arrange = () => {
      const book = node.parentElement!;
      if (!book.clientWidth || !book.clientHeight) return;
      const mobile = book.clientWidth <= 600;
      mobileRef.current = mobile;
      // Every spread uses the first page's canvas dimensions, so turning pages
      // keeps the same width within the available window height.
      const ratio = 2469 / 1372;
      const width = mobile ? book.clientWidth : Math.min(book.clientWidth, book.clientHeight * ratio);
      node.style.width = `${width}px`;
      node.style.height = mobile && page !== 9 ? '1040px' : `${width / ratio}px`;
      node.classList.toggle('is-mobile', mobile);
      node.querySelectorAll<HTMLElement>('[data-r]').forEach(el => {
        const [x, y, w, h] = el.dataset[mobile ? 'm' : 'r']!.split(',').map(Number);
        Object.assign(el.style, { left: `${x}%`, top: `${y}%`, width: `${w}%`, height: h ? `${h}%` : 'auto' });
        if (el.classList.contains('panel-shadow')) {
          const shot = el.querySelector<HTMLElement>('.shot')!;
          if (page === 1 && shot.id === 'platform-light') {
            const fullHeight = mobile ? 63 : 100;
            el.style.setProperty('--rain-full-panel-height', `${fullHeight}%`);
            el.style.setProperty('--rain-final-panel-height', `${h}%`);
            shot.style.setProperty('--rain-image-height', `${(mobile ? 1040 : width / ratio) * fullHeight / 100}px`);
          }
          // Choose the nearest occupied side; wide bottom panels enter from below.
          const horizontal = Math.max(0, Math.abs(x + w / 2 - 50) - w / 2);
          const vertical = Math.max(0, Math.abs(y + h / 2 - 50) - h / 2);
          const fromHorizontal = horizontal > vertical || (h > w && Math.abs(x+w/2-50) > 15);
          const side = fromHorizontal ? (x+w/2 >= 50 ? 'right' : 'left') : (y+h/2 >= 50 ? 'bottom' : 'top');
          shot.dataset.enterSide = side;
          shot.style.setProperty('--enter-x', side === 'right' ? '24px' : side === 'left' ? '-24px' : '0px');
          shot.style.setProperty('--enter-y', side === 'bottom' ? '24px' : side === 'top' ? '-24px' : '0px');
          if (page === 1 && shot.id === 'rain-soaked-detail') {
            shot.style.setProperty('--enter-x', '0px');
            shot.style.setProperty('--enter-y', `${(mobile ? 1040 : width / ratio) * (mobile ? 21 : 38) / 100}px`);
          }
          if (page === 1 && shot.id === 'waiting-n') {
            shot.style.setProperty('--enter-x', '40px');
            shot.style.setProperty('--enter-y', '0px');
          }
          if (shot.id === 'skirt-hand') {
            // The 3×2 atlas contains six square 512px frames. Fit the panel,
            // never stretch the atlas; anchor its right edge at the shared corner.
            const stageHeight = mobile ? 1040 : width / ratio;
            const size = Math.min(width * w / 100, stageHeight * h / 100);
            el.style.width = `${size}px`;
            el.style.height = `${size}px`;
            el.style.left = `${width * (x + w) / 100 - size}px`;
            const mainLayout = node.querySelector<HTMLElement>('#skirt-full')?.parentElement?.dataset[mobile ? 'm' : 'r']?.split(',').map(Number);
            shot.style.setProperty('--hand-corner-width', `${width * ((x + w) - (mainLayout?.[0] ?? 30.5)) / 100}px`);
            shot.style.setProperty('--hand-corner-height', `${stageHeight * ((mainLayout?.[1] ?? 0) + (mainLayout?.[3] ?? 100) - y) / 100}px`);
          }

          const points: number[][] = mobile ? [[0, 0], [100, 0], [100, 100], [0, 100]] : JSON.parse(shot.dataset.points!);
          shot.style.clipPath = shot.classList.contains('hand-inset') ? 'none' : `polygon(${points.map(p => `${p[0]}% ${p[1]}%`).join(',')})`;
          shot.querySelector('polygon')?.setAttribute('points', points.map(p => p.join(',')).join(' '));
        }
      });
      fitComicText(node);
      fullPanelReveal?.refresh();
      insetPanelReveal?.refresh();
    };
    arrange();
    const stopTypewriter = entering && (page === 1 || page === 8) ? startRainTypewriter(node, page === 8 ? 0 : 350) : undefined;
    const inkReveal = entering && page === 9 ? startMorningInkReveal(node) : undefined;
    if (entering && page === 4 && !mobileRef.current) {
      const panel = node.querySelector<HTMLElement>('#together')?.parentElement;
      if (panel) fullPanelReveal = startFullPanelReveal(panel, () => !mobileRef.current);
    }
    if (page === 7) {
      insetPanelReveal = startInsetPanelReveal(node, () => !mobileRef.current,
        entering && !mobileRef.current,
        () => {
          const alreadyReady = handReady.current;
          handReady.current = true;
          if (active && entering && !alreadyReady) startHand('gripping');
        });
    }
    // Keep the right-panel crop locked during the entrance; hovering becomes
    // available once the inset has settled, including in automatic touring.
    const nanaPanel = node.querySelector<HTMLElement>('#hold-n');
    nanaPanel?.classList.remove('crop-ready');
    const cropReadyTimer = nanaPanel ? window.setTimeout(() => {
      nanaPanel.classList.add('crop-ready');
    }, entering ? handInsetSettledAt : 0) : undefined;
    const resize = new ResizeObserver(arrange);
    resize.observe(node.parentElement!);
    return () => {
      window.clearTimeout(cropReadyTimer);
      nanaPanel?.classList.remove('crop-ready');
      resize.disconnect(); fullPanelReveal?.stop(); insetPanelReveal?.stop(); inkReveal?.stop(); stopTypewriter?.();
      if (animatedRevision.current === animationKey) animatedRevision.current = '';
    };
  }, [html, page, revision, active]);

  useEffect(() => () => { hoverFloats.current.forEach(float => float.stop()); }, []);
  useEffect(() => { if (!active) hoverFloats.current.forEach(float => float.stop()); }, [active]);

  // 手写体加载完成后文本度量会变，补跑一次气泡适配
  useEffect(() => {
    let cancelled = false;
    void document.fonts?.ready.then(() => { if (!cancelled) fitComicText(stage.current); });
    return () => { cancelled = true; };
  }, []);
  useEffect(() => () => window.clearTimeout(wheelIdle.current), []);

  useEffect(() => {
    // Without the animated desktop crop, wait for the same story cue explicitly.
    const initial = active && page === 7 && mobileRef.current ? window.setTimeout(() => {
      handReady.current = true;
      playHand();
    }, handInsetSettledAt) : undefined;
    return () => { window.clearTimeout(initial); window.clearTimeout(handTimer.current); window.clearTimeout(handHoldTimer.current); handStartedAt.current = 0; handState.current = 'open'; };
  }, [page, revision, active]);
  const finish = (e: React.PointerEvent, cancelled = false) => {
    const drag = dragging.current;
    if (!drag || drag.id !== e.pointerId) return;
    dragging.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    (e.currentTarget as HTMLElement).style.transform = '';
    const turning = !cancelled && drag.horizontal && Math.abs(drag.dx) > Math.max(45, Math.min(100, e.currentTarget.clientWidth * .08));
    lastManual.current = Date.now();
    if (turning) {
      suppressClick.current = true;
      turn(drag.dx < 0 ? 1 : -1);
    }
  };

  return <div className="mind-comic-book">
    <div key={revision} ref={stage} className={`comic scene-${page + 1}`} data-language={i18n.resolvedLanguage ?? i18n.language} tabIndex={0} aria-label={t('comic.gesture', { title: t(`comic.title${page}`) })} onKeyDown={e => {
      if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); turn(e.key === 'ArrowLeft' ? -1 : 1); }
    }} onWheel={e => {
      // 移动布局下 .mind-comic-book 自身可纵向滚动，滚轮让位原生滚动
      if (mobileRef.current || dragging.current) return;
      const delta = Math.abs(e.deltaY) >= Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
      if (!delta) return;
      const now = Date.now();
      // 翻页冷却内的惯性事件直接丢弃，避免一 gesture 连翻多页
      if (now < wheelLock.current) { wheelAccum.current = 0; return; }
      wheelAccum.current += delta;
      window.clearTimeout(wheelIdle.current);
      wheelIdle.current = window.setTimeout(() => { wheelAccum.current = 0; }, 150);
      if (Math.abs(wheelAccum.current) >= 36) {
        const direction = wheelAccum.current > 0 ? 1 : -1;
        wheelAccum.current = 0;
        wheelLock.current = now + 620;
        turn(direction);
      }
    }} onPointerDown={e => {
      suppressClick.current = false;
      // functional：整本书的拖拽由 UnifiedComicBook 的书根统一接管，这里让位，免得两处都动。
      if (functional || !e.isPrimary || e.button !== 0 || (e.target as Element).closest('button,a')) return;
      dragging.current = { id: e.pointerId, x: e.clientX, y: e.clientY, dx: 0, horizontal: false };
    }} onPointerMove={e => {
      const drag = dragging.current;
      if (!drag || drag.id !== e.pointerId) return;
      drag.dx = e.clientX - drag.x;
      if (!drag.horizontal && Math.abs(drag.dx) > 12 && Math.abs(drag.dx) > Math.abs(e.clientY - drag.y) * 1.3) { drag.horizontal = true; e.currentTarget.setPointerCapture(e.pointerId); }
      if (drag.horizontal) {
        e.preventDefault();
        const offset = Math.max(-38, Math.min(38, drag.dx * .15));
        e.currentTarget.style.transform = `translateX(${offset}px)`;
      }
    }} onPointerUp={e => finish(e)} onPointerCancel={e => finish(e, true)} onPointerOver={e => {
      const shot = (e.target as Element).closest<HTMLElement>('.scene-3 .shot');
      if (shot && !(e.relatedTarget instanceof Node && shot.contains(e.relatedTarget))) {
        {
          let float = hoverFloats.current.get(shot.id);
          if (!float) {
            const id = shot.id;
            float = createPanelFloat((x,y) => {
              const wrap = stage.current?.querySelector<HTMLElement>(`#${id} .image-wrap`);
              if (wrap) wrap.style.translate = `${x}px ${y}px`;
            });
            hoverFloats.current.set(id,float);
          }
          const wrap = shot.querySelector<HTMLElement>('.image-wrap')!;
          const extraX = Math.max(0,(wrap.offsetWidth-shot.clientWidth)/2);
          const extraY = Math.max(0,(wrap.offsetHeight-shot.clientHeight)/2);
          float.start(Math.random()*Math.PI*2,{minX:-extraX,maxX:extraX,minY:-extraY,maxY:extraY});
        }
      }
      const panel = (e.target as Element).closest('#skirt-hand');
      if (panel && !(e.relatedTarget instanceof Node && panel.contains(e.relatedTarget))) playHand();
    }} onClick={e => {
      if (suppressClick.current) { suppressClick.current = false; return; }
      if ((e.target as Element).closest('#skirt-hand')) {
        lastManual.current = Date.now();
        playHand();
      }
    }} onPointerOut={e => {
      const shot = (e.target as Element).closest<HTMLElement>('.scene-3 .shot');
      if (shot && !(e.relatedTarget instanceof Node && shot.contains(e.relatedTarget))) {
        hoverFloats.current.get(shot.id)?.stop();
      }
    }} dangerouslySetInnerHTML={{ __html: html }} />

  </div>;
}
