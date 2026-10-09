import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { Clipboard, ArrowUp, LoaderCircle } from 'lucide-react';
import { getCharacterId } from '../characterContext';
import { useTranslation } from 'react-i18next';
import { clipboardMessage } from '../utils/clipboardMessage';
import './ClipboardHint.css';
export default function ClipboardHint({ onSend }: { onSend: (message: string) => Promise<void> }) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const label = zh ? '发送剪贴板' : ja ? 'コピーを送る' : 'Send clipboard';
  const [visible, setVisible] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const sending = useRef(false), revision = useRef(0), mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    let active = true, cleanup: (() => void) | undefined;
    void listen<{ sequence: number }>('companion:clipboard-hint', () => {
      if (!active) return;
      revision.current++; setError(''); setVisible(true);
    }).then(unlisten => { if (active) cleanup = unlisten; else unlisten(); }).catch(() => {});
    return () => { mounted.current = false; active = false; cleanup?.(); };
  }, []);
  const send = async () => {
    if (sending.current) return;
    sending.current = true; setBusy(true); setError('');
    const version = revision.current;
    try {
      const value = await invoke<{ text: string }>('companion_read_clipboard');
      if (!value.text.trim()) throw new Error(zh ? '剪贴板没有文本' : ja ? 'テキストがありません' : 'Clipboard has no text');
      await onSend(clipboardMessage(value.text));
      if (mounted.current && version === revision.current) setVisible(false);
    } catch (e) { if (mounted.current) setError(String(e)); }
    finally { sending.current = false; if (mounted.current) setBusy(false); }
  };
  return visible ? <div className={`pet-clipboard ${getCharacterId() === 'nana' ? 'is-nana' : ''}`}
    onPointerDown={e => e.stopPropagation()} onMouseDown={e => e.stopPropagation()} onMouseUp={e => e.stopPropagation()}
    onContextMenu={e => { e.preventDefault(); e.stopPropagation(); }}>
    <button className="pet-clipboard-send" title={error || label} aria-label={label} disabled={busy}
      onClick={e => { e.stopPropagation(); void send(); }}>
      {busy ? <LoaderCircle size={15} className="pet-clipboard-spinner" /> : <Clipboard size={15} />}
      <span>{label}</span><ArrowUp size={13} />
    </button>
    {error && <span className="pet-clipboard-error" role="alert">{error}</span>}
  </div> : null;
}
