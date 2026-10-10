import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Clipboard, ArrowUp, LoaderCircle } from 'lucide-react';
import { getCharacterId } from '../characterContext';
import { useTranslation } from 'react-i18next';
import { clipboardMessage } from '../utils/clipboardMessage';
import './ClipboardHint.css';
export default function ClipboardHint({ onSend }: { onSend: (message: string) => Promise<void> }) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const label = zh ? '分享剪贴板' : ja ? 'コピーを共有' : 'Share clipboard';
  const privacy = zh ? '点击才读取当前剪贴板文本并发送给桌宠；不点击不读取、不提交。'
    : ja ? 'クリックした時だけ現在のクリップボードのテキストを読み取り、ペットに送ります。'
    : 'Only clicking reads and sends the current clipboard text to your pet.';
  const [visible, setVisible] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const sending = useRef(false), revision = useRef(0), mounted = useRef(false);
  const sendButton = useRef<HTMLButtonElement>(null);
  const expiry = useRef<ReturnType<typeof setTimeout>>();
  const close = () => { clearTimeout(expiry.current); setVisible(false); setError(''); };
  useEffect(() => {
    mounted.current = true;
    let active = true;
    const cleanups: Array<() => void> = [];
    const subscribe = <T,>(name: string, callback: (event: { payload: T }) => void) => {
      void listen<T>(name, event => { if (active) callback(event); }, { target: getCurrentWindow().label })
        .then(unlisten => { if (active) cleanups.push(unlisten); else unlisten(); }).catch(() => {});
    };
    subscribe<{ sequence: number }>('companion:clipboard-hint', () => {
      revision.current++; setError(''); setVisible(true);
      clearTimeout(expiry.current);
      expiry.current = setTimeout(close, 60_000);
    });
    subscribe('companion:clipboard-outside-press', close);
    const outsidePress = (event: PointerEvent) => {
      if (event.target instanceof Node && sendButton.current?.contains(event.target)) return;
      close();
    };
    // Capture also sees presses whose handlers stop propagation (pet dragging, menus).
    document.addEventListener('pointerdown', outsidePress, true);
    void invoke('start_chat_outside_click_hook').catch(() => {});
    return () => {
      mounted.current = false; active = false; clearTimeout(expiry.current);
      document.removeEventListener('pointerdown', outsidePress, true);
      cleanups.forEach(unlisten => unlisten());
    };
  }, []);
  const send = async () => {
    if (sending.current) return;
    sending.current = true; setBusy(true); setError('');
    const version = revision.current;
    try {
      const value = await invoke<{ text: string }>('companion_read_clipboard');
      if (!value.text.trim()) throw new Error(zh ? '剪贴板没有文本' : ja ? 'テキストがありません' : 'Clipboard has no text');
      await onSend(clipboardMessage(value.text));
      if (mounted.current && version === revision.current) close();
    } catch (e) { if (mounted.current) setError(String(e)); }
    finally { sending.current = false; if (mounted.current) setBusy(false); }
  };
  return visible ? <div className={`pet-clipboard ${getCharacterId() === 'nana' ? 'is-nana' : ''}`}
    onPointerDown={e => e.stopPropagation()} onMouseDown={e => e.stopPropagation()} onMouseUp={e => e.stopPropagation()}
    onContextMenu={e => { e.preventDefault(); e.stopPropagation(); }}>
    <button ref={sendButton} className="pet-clipboard-send" title={error || privacy} aria-label={label} disabled={busy}
      onClick={e => { e.stopPropagation(); void send(); }}>
      {busy ? <LoaderCircle size={15} className="pet-clipboard-spinner" /> : <Clipboard size={15} />}
      <span>{label}</span><ArrowUp size={13} />
    </button>
    {error && <span className="pet-clipboard-error" role="alert">{error}</span>}
  </div> : null;
}
