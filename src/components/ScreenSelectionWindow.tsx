import { useCallback, useEffect, useRef, useState } from 'react';
import type { PointerEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { ChevronDown, Copy, Download, ScanLine, Sparkles } from 'lucide-react';
import { captureRegion, selectionRect } from '../utils/screenSelection';
import type { SelectionPoint, SelectionRect } from '../utils/screenSelection';
import { synchronizeWindowLanguage } from '../utils/windowLanguage';
import './ScreenSelectionWindow.css';

interface Frame { image: string; width: number; height: number }
type SelectionAction = 'analyze' | 'save' | 'save_and_analyze' | 'copy' | 'freeze' | 'cancel';
type AnalysisAction = 'analyze' | 'save_and_analyze';
type AnalysisCharacter = 'vivian' | 'nana';

export default function ScreenSelectionWindow() {
  const { i18n } = useTranslation();
  const language = i18n.language.startsWith('zh') ? 'zh' : i18n.language.startsWith('ja') ? 'ja' : 'en';
  // The hint pill is a single centred row, so `select` / `cancel` are kept short
  // and close in rendered width across languages: 6 / 14 / 7 characters for
  // `select` (CJK glyphs are ~2x a Latin glyph) and 2 / 6 / 5 for `cancel`.
  const copy = {
    zh: { freeze: '定格画面', select: '拖动选择区域', cancel: '取消', save: '保存', saveAnalyze: '保存并分析', analyze: '分析', copy: '复制到剪贴板', screen: '屏幕快照' },
    en: { freeze: 'Freeze', select: 'Drag to select', cancel: 'Cancel', save: 'Save', saveAnalyze: 'Save & analyze', analyze: 'Analyze', copy: 'Copy to clipboard', screen: 'Screen snapshot' },
    ja: { freeze: '画面を固定', select: 'ドラッグで選択', cancel: 'キャンセル', save: '保存', saveAnalyze: '保存して分析', analyze: '分析', copy: 'クリップボードにコピー', screen: '画面のスナップショット' },
  }[language];
  const [sessionId, setSessionId] = useState<string | null>(null);
  const activeSession = useRef<string | null>(null);
  const previewUrl = useRef<string | null>(null);
  const [frame, setFrame] = useState<Frame | null>(null);
  const [rect, setRect] = useState<SelectionRect | null>(null);
  const [dragging, setDragging] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const [drawer, setDrawer] = useState<{ action: AnalysisAction; left: number; top: number } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const start = useRef<SelectionPoint | null>(null);
  const selection = useRef<SelectionRect | null>(null);
  const busy = useRef(false);
  const mounted = useRef(true);

  const finish = useCallback(async (action: SelectionAction, analysisCharacter?: AnalysisCharacter) => {
    const cancel = action === 'cancel';
    if (busy.current || !sessionId || activeSession.current !== sessionId) return;
    const bounds = root.current?.getBoundingClientRect();
    const region = !cancel && frame && bounds && selection.current
      ? captureRegion(selection.current, bounds, frame) : null;
    if (!cancel && !region) return;
    busy.current = true;
    setSubmitting(true);
    setDrawer(null);
    try {
      await invoke('screen_selection_finish', { sessionId, region, action, ...(analysisCharacter ? { analysisCharacter } : {}) });
    } catch (cause) {
      if (activeSession.current !== sessionId) return;
      busy.current = false;
      if (mounted.current) { setSubmitting(false); setError(String(cause)); }
    }
  }, [sessionId, frame]);

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    const cleanups: Array<() => void> = [];
    const languageSync = synchronizeWindowLanguage(language => i18n.changeLanguage(language));
    cleanups.push(languageSync.dispose);
    const reset = () => {
      activeSession.current = null;
      if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
      previewUrl.current = null;
      busy.current = false; start.current = null; selection.current = null;
      setSessionId(null); setFrame(null); setRect(null); setDragging(false); setSubmitting(false); setError('');
      setDrawer(null);
    };
    void (async () => {
      const onStart = await listen<{ session_id: string; width: number; height: number }>('screen_selection:start', event => {
        if (disposed) return;
        reset();
        const { session_id: id, width, height } = event.payload;
        activeSession.current = id; setSessionId(id);
        void invoke<ArrayBuffer | number[]>('screen_selection_frame', { sessionId: id }).then(bytes => {
          if (disposed || activeSession.current !== id) return;
          const data = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes);
          const image = URL.createObjectURL(new Blob([data], { type: 'image/png' }));
          previewUrl.current = image;
          setFrame({ image, width, height });
        }).catch(() => {
          if (!disposed && activeSession.current === id) void invoke('screen_selection_finish', { sessionId: id, region: null, action: 'cancel' }).catch(console.warn);
        });
      });
      if (disposed) { onStart(); return; }
      cleanups.push(onStart);
      const onReset = await listen<{ session_id: string }>('screen_selection:reset', event => {
        if (!disposed && activeSession.current === event.payload.session_id) reset();
      });
      if (disposed) { onReset(); return; }
      cleanups.push(onReset);
      await languageSync.ready;
      if (disposed) return;
      await invoke('screen_selection_ready');
    })().catch(console.warn);
    return () => {
      disposed = true; mounted.current = false;
      cleanups.forEach(cleanup => cleanup());
      activeSession.current = null;
      if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
      previewUrl.current = null;
    };
  }, []);

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape' || event.key === 'Enter') {
        if (drawer) {
          if (event.key === 'Escape') { event.preventDefault(); setDrawer(null); }
          return;
        }
        if (event.key === 'Enter' && (event.target as Element | null)?.closest?.('button')) return;
        event.preventDefault();
        if (event.key === 'Escape' || !start.current) void finish(event.key === 'Escape' ? 'cancel' : 'freeze');
      }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [finish, drawer]);

  const reveal = () => {
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (mounted.current && sessionId === activeSession.current) void invoke('screen_selection_present', { sessionId }).catch(() => void finish('cancel'));
    }));
  };
  const point = (event: PointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  };
  const update = (event: PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    const next = selectionRect(start.current, point(event), event.currentTarget.getBoundingClientRect());
    selection.current = next;
    setRect(next);
  };
  const reset = () => { selection.current = null; setRect(null); setError(''); setDrawer(null); };
  const chooseLabel = language === 'zh' ? '选择分析桌宠' : language === 'ja' ? '分析するキャラクターを選択' : 'Choose who analyzes';
  const openDrawer = (action: AnalysisAction, button: HTMLButtonElement) => {
    if (drawer?.action === action) { setDrawer(null); return; }
    const buttonBounds = button.getBoundingClientRect();
    const screen = root.current?.getBoundingClientRect();
    if (!screen) return;
    const width = Math.min(280, screen.width - 16);
    const left = Math.max(8, Math.min(buttonBounds.right - screen.left - width, screen.width - width - 8));
    const below = buttonBounds.bottom - screen.top + 8;
    const top = below + 124 <= screen.height ? below : Math.max(8, buttonBounds.top - screen.top - 132);
    setDrawer({ action, left, top });
  };
  const analysisButton = (action: AnalysisAction) => <div className="selection-split-button selection-split-primary">
    <button className="selection-confirm" onClick={() => void finish(action)} disabled={submitting}>
      <Sparkles size={16} /><span>{action === 'analyze' ? copy.analyze : copy.saveAnalyze}</span>
    </button>
    <button className="selection-drawer-toggle" aria-label={`${action === 'analyze' ? copy.analyze : copy.saveAnalyze} · ${chooseLabel}`}
      aria-haspopup="menu" aria-expanded={drawer?.action === action} disabled={submitting}
      onClick={event => openDrawer(action, event.currentTarget)}><ChevronDown size={13} /></button>
  </div>;
  const bounds = root.current?.getBoundingClientRect();
  const region = rect && bounds && frame ? captureRegion(rect, bounds, frame) : null;
  const toolbarTop = rect && bounds ? Math.max(16, rect.y + rect.height + 100 < bounds.height ? rect.y + rect.height + 14 : rect.y - 100) : 0;
  const toolbarLeft = rect && bounds ? (bounds.width < 720 ? bounds.width / 2 : Math.max(360, Math.min(bounds.width - 360, rect.x + rect.width / 2))) : 0;

  return <div ref={root} className="screen-selection" role="application" aria-label={copy.select}
    onContextMenu={event => { event.preventDefault(); if (drawer) setDrawer(null); else void finish('cancel'); }}
    onPointerDown={event => {
      if (!frame || busy.current || event.button !== 0 || !event.isPrimary || (event.target as Element).closest('button')) return;
      if (drawer) { setDrawer(null); return; }
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      start.current = point(event);
      reset(); setDragging(true); update(event);
    }}
    onPointerMove={update}
    onPointerUp={event => {
      if (!start.current) return;
      update(event); start.current = null; setDragging(false);
      if (!selection.current || selection.current.width < 2 || selection.current.height < 2) reset();
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    }}
    onPointerCancel={() => { start.current = null; setDragging(false); reset(); }}>
    {frame && <img className="screen-selection-image" src={frame.image} alt={copy.screen} draggable={false} onLoad={reveal} onError={() => void finish('cancel')} />}
    {!rect && <div className="screen-selection-shade" />}
    {!rect && <div className="screen-selection-hint" onPointerDown={event => event.stopPropagation()}>
      <ScanLine size={19} /><span>{copy.select}</span><button onClick={() => void finish('cancel')}>{copy.cancel}</button>
    </div>}
    {rect && <div className="screen-selection-box" style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}>
      <i className="selection-corner corner-tl" /><i className="selection-corner corner-tr" /><i className="selection-corner corner-bl" /><i className="selection-corner corner-br" />
      {region && <span className="screen-selection-size" style={{ top: rect.y < 34 ? 10 : -30 }}>{region.width} × {region.height}</span>}
    </div>}
    {region && !dragging && <div className="screen-selection-toolbar" style={{ left: toolbarLeft, top: toolbarTop }} onPointerDown={event => event.stopPropagation()}>
      <button className="selection-text-button" onClick={() => void finish('cancel')} disabled={submitting}>{copy.cancel}</button>
      <button className="selection-text-button" onClick={() => void finish('save')} disabled={submitting}><Download size={16} /><span>{copy.save}</span></button>
      {analysisButton('analyze')}
      <button className="selection-text-button" onClick={() => void finish('copy')} disabled={submitting}><Copy size={16} /><span>{copy.copy}</span></button>
      {analysisButton('save_and_analyze')}
      <span className="selection-toolbar-divider" />
      <button className="selection-text-button" onClick={() => void finish('freeze')} disabled={submitting}><ScanLine size={16} /><span>{copy.freeze}</span><kbd>↵</kbd></button>
    </div>}
    {drawer && region && !dragging && <div className="selection-character-menu" role="menu" aria-label={chooseLabel}
      style={{ left: drawer.left, top: drawer.top }} onPointerDown={event => event.stopPropagation()}>
      {(['vivian', 'nana'] as const).map(character => {
        const name = character === 'vivian' ? 'Vivian' : 'Nana';
        const label = language === 'zh' ? `${drawer.action === 'save_and_analyze' ? '保存并' : ''}交由${name}分析`
          : language === 'ja' ? `${drawer.action === 'save_and_analyze' ? '保存して' : ''}${name}に分析を依頼`
            : `${drawer.action === 'save_and_analyze' ? 'Save & ask' : 'Ask'} ${name} to analyze`;
        return <button key={character} role="menuitem" disabled={submitting} onClick={() => void finish(drawer.action, character)}>
          <span className={`selection-character-avatar avatar-${character}`} aria-hidden="true">{name[0]}</span><span>{label}</span>
        </button>;
      })}
    </div>}
    {error && <div className="screen-selection-error" role="alert">{error}</div>}
  </div>;
}
