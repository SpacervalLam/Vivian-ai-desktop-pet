import { useEffect, useRef, useState } from 'react';
import type { PointerEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { Download, Sparkles, X } from 'lucide-react';
import { synchronizeWindowLanguage } from '../utils/windowLanguage';
import { edgeResizeScale, FREEZE_EDGES, type FreezeEdge } from '../utils/screenFreezeResize';
import './ScreenFreezeWindow.css';

export default function ScreenFreezeWindow() {
  const { i18n } = useTranslation();
  const copy = i18n.language.startsWith('zh')
    ? { save: '保存', close: '关闭', analyze: '分析', saved: '已保存至', analyzing: '正在分析…', done: '已发送分析结果', title: '定格画面：滚轮缩放，拖动移动，右键打开菜单' }
    : i18n.language.startsWith('ja')
      ? { save: '保存', close: '閉じる', analyze: '分析', saved: '保存先', analyzing: '分析中…', done: '分析結果を送信しました', title: '固定画面：ホイールで拡大縮小、ドラッグで移動、右クリックでメニュー' }
      : { save: 'Save', close: 'Close', analyze: 'Analyze', saved: 'Saved to', analyzing: 'Analyzing…', done: 'Analysis sent', title: 'Snapshot: scroll to zoom, drag to move, right-click for menu' };
  const [image, setImage] = useState('');
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const root = useRef<HTMLDivElement>(null);
  const scale = useRef(1);
  const ready = useRef(false);
  const active = useRef(false);
  const running = useRef(false);
  const operation = useRef(false);
  const resizeEdge = useRef<FreezeEdge | null>(null);
  const edgeDrag = useRef<{ edge: FreezeEdge; pointer: number; x: number; y: number; width: number; height: number; scale: number } | null>(null);

  useEffect(() => {
    active.current = true;
    let disposed = false;
    let url: string | undefined;
    const language = synchronizeWindowLanguage(value => i18n.changeLanguage(value));
    void invoke<ArrayBuffer | number[]>('screen_freeze_frame').then(bytes => {
      if (disposed) return;
      url = URL.createObjectURL(new Blob([bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes)], { type: 'image/png' }));
      setImage(url);
    }).catch(error => {
      if (!disposed) void invoke('screen_freeze_present', { error: String(error) }).catch(console.warn);
    });
    return () => {
      disposed = true; active.current = false; ready.current = false;
      language.dispose();
      if (url) URL.revokeObjectURL(url);
    };
  }, []);

  const present = () => {
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (!active.current || ready.current) return;
      void invoke('screen_freeze_present', { error: null }).then(() => {
        if (active.current) ready.current = true;
      }).catch(error => { if (active.current) setMessage(String(error)); });
    }));
  };

  // Serialize native resizes, coalescing wheel events that arrive during an IPC call.
  const resize = async () => {
    if (running.current || !active.current) return;
    running.current = true;
    try {
      let applied: number;
      do {
        applied = scale.current;
        const effective = await invoke<number>('screen_freeze_resize', { scale: applied, edge: resizeEdge.current });
        if (applied === scale.current) {
          scale.current = effective;
          break;
        }
      } while (active.current && applied !== scale.current);
    } catch (error) {
      if (active.current) setMessage(String(error));
    } finally { running.current = false; }
  };

  useEffect(() => {
    const element = root.current;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      if (!ready.current || !event.deltaY) return;
      if (edgeDrag.current) return;
      setMenu(null);
      resizeEdge.current = null;
      scale.current = Math.max(0.1, Math.min(8, scale.current * Math.exp(-Math.max(-100, Math.min(100, event.deltaY)) * 0.002)));
      void resize();
    };
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape') setMenu(null); };
    element?.addEventListener('wheel', wheel, { passive: false });
    window.addEventListener('keydown', key);
    return () => { element?.removeEventListener('wheel', wheel); window.removeEventListener('keydown', key); };
  }, []);

  const moveEdge = (event: PointerEvent<HTMLDivElement>) => {
    const drag = edgeDrag.current;
    if (!drag || drag.pointer !== event.pointerId) return;
    scale.current = edgeResizeScale(drag.edge, drag.width, drag.height, drag.scale, event.screenX - drag.x, event.screenY - drag.y);
    resizeEdge.current = drag.edge;
    void resize();
  };

  const perform = async (action: 'save' | 'analyze') => {
    if (operation.current) return;
    operation.current = true; setBusy(true); setMenu(null);
    setMessage(action === 'analyze' ? copy.analyzing : '');
    try {
      if (action === 'save') {
        const path = await invoke<string>('screen_freeze_save');
        if (active.current) setMessage(`${copy.saved} ${path}`);
      } else {
        await invoke('screen_freeze_analyze');
        if (active.current) setMessage(copy.done);
      }
    } catch (error) { if (active.current) setMessage(String(error)); }
    finally { operation.current = false; if (active.current) setBusy(false); }
  };

  return <div ref={root} className="screen-freeze" aria-label={copy.title}
    onPointerDown={event => {
      if (event.button !== 0) return;
      setMenu(null); setMessage('');
      void getCurrentWindow().startDragging().catch(error => setMessage(String(error)));
    }}
    onContextMenu={event => {
      event.preventDefault();
      setMenu({ x: Math.max(6, Math.min(event.clientX, window.innerWidth - 154)), y: Math.max(6, Math.min(event.clientY, window.innerHeight - 114)) });
    }}>
    {image && <img src={image} alt={copy.title} draggable={false} onLoad={present}
      onError={() => void invoke('screen_freeze_present', { error: '无法加载定格快照' }).catch(console.warn)} />}
    {FREEZE_EDGES.map(edge => <div key={edge} className={`screen-freeze-edge edge-${edge}`} aria-hidden="true"
      onPointerDown={event => {
        event.stopPropagation();
        if (event.button !== 0 || !ready.current || !event.isPrimary) return;
        event.preventDefault();
        setMenu(null); setMessage('');
        event.currentTarget.setPointerCapture(event.pointerId);
        edgeDrag.current = { edge, pointer: event.pointerId, x: event.screenX, y: event.screenY, width: window.innerWidth, height: window.innerHeight, scale: scale.current };
      }}
      onPointerMove={moveEdge}
      onPointerUp={event => {
        moveEdge(event);
        if (edgeDrag.current?.pointer !== event.pointerId) return;
        edgeDrag.current = null;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      }}
      onPointerCancel={() => { edgeDrag.current = null; }}
      onLostPointerCapture={() => { edgeDrag.current = null; }}
    />)}
    {menu && <div className="screen-freeze-menu" role="menu" aria-label={copy.title} style={{ left: menu.x, top: menu.y }} onPointerDown={event => event.stopPropagation()}>
      <button role="menuitem" disabled={busy} onClick={() => void perform('save')}><Download size={15} />{copy.save}</button>
      <button role="menuitem" onClick={() => void getCurrentWindow().close().catch(error => setMessage(String(error)))}><X size={15} />{copy.close}</button>
      <button role="menuitem" disabled={busy} onClick={() => void perform('analyze')}><Sparkles size={15} />{copy.analyze}</button>
    </div>}
    {message && <div className="screen-freeze-message" role="status" onPointerDown={event => event.stopPropagation()}>{message}</div>}
  </div>;
}
