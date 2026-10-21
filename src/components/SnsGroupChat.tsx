import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import type { SnsGroup, SnsPerson } from './SnsManager';
import './Sns.css';
interface Message { id: string; speaker: string; role: string; content: string; timestamp: number; metadata: Record<string, unknown> }
interface Stream { speaker: string; text: string }
function SharedImage({ path }: { path: string }) {
  const [src, setSrc] = useState('');
  useEffect(() => { let disposed = false; void invoke<string | null>('get_image_data_url', { imagePath: path }).then(src => { if (!disposed) setSrc(src || ''); }).catch(() => {}); return () => { disposed = true; }; }, [path]);
  return src ? <a href={src} target="_blank" rel="noreferrer"><img src={src} alt="Shared image" /></a> : <span>📷</span>;
}
export default function SnsGroupChat({ group, people, onClose, onSettings }: { group: SnsGroup; people: SnsPerson[]; onClose: () => void; onSettings: () => void }) {
  const { t } = useTranslation();
  const [messages, setMessages] = useState<Message[]>([]); const [streams, setStreams] = useState<Record<string, Stream>>({});
  const [input, setInput] = useState(''); const [image, setImage] = useState<string | null>(null); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [more, setMore] = useState(false); const [loadingOlder, setLoadingOlder] = useState(false);
  const channel = `wechat_group:${group.id}`; const list = useRef<HTMLDivElement>(null); const alive = useRef(true); const nearBottom = useRef(true); const first = useRef<string | null>(null);
  const refresh = useCallback(async () => {
    const rows = await invoke<Message[]>('sns_group_history', { id: group.id, limit: 81 });
    if (!alive.current) return;
    setMore(previous => first.current ? previous : rows.length > 80);
    const page = rows.slice(-80); if (!first.current) first.current = page[0]?.id ?? null;
    setMessages(previous => [...new Map([...previous, ...page].map(m => [m.id, m])).values()].sort((a, b) => a.timestamp - b.timestamp));
  }, [group.id]);
  useEffect(() => {
    alive.current = true; let stopped = false; const cleanup: UnlistenFn[] = [];
    void (async () => {
      try {
        for (const name of ['chat:start', 'chat:chunk', 'chat:done', 'chat:error', 'chat:cancelled']) {
          const unlisten = await listen<{ channel?: string; stream_id?: string; character_id?: string; text?: string; error?: string }>(name, event => {
            const p = event.payload; if (stopped || p.channel !== channel || !p.stream_id) return;
            const id = p.stream_id;
            if (name === 'chat:start') setStreams(previous => ({ ...previous, [id]: { speaker: p.character_id || '', text: '' } }));
            else if (name === 'chat:chunk') setStreams(previous => ({ ...previous, [id]: { speaker: p.character_id || previous[id]?.speaker || '', text: (previous[id]?.text || '') + (p.text || '') } }));
            else {
              if (p.error) setError(p.error);
              void refresh().catch(e => setError(String(e))).finally(() => { if (alive.current) setStreams(previous => { const next = { ...previous }; delete next[id]; return next; }); });
            }
          }); if (stopped) unlisten(); else cleanup.push(unlisten);
        }
        const unlisten = await listen<{ deleted_group?: string; group_id?: string }>('sns:changed', event => {
          if (event.payload.deleted_group === group.id) onClose();
          else if (event.payload.group_id === group.id) void refresh().catch(e => setError(String(e)));
        }); if (stopped) unlisten(); else cleanup.push(unlisten);
        await refresh();
      } catch (e) { if (!stopped) setError(String(e)); }
    })();
    return () => { stopped = true; alive.current = false; cleanup.forEach(fn => fn()); };
  }, [channel, group.id, refresh]);
  useEffect(() => { if (nearBottom.current) list.current?.scrollTo({ top: list.current.scrollHeight, behavior: 'auto' }); }, [messages, streams]);
  const send = async () => {
    if (busy || (!input.trim() && !image)) return;
    const message = input.trim(); const attachment = image; setInput(''); setImage(null); setBusy(true); setError(''); nearBottom.current = true;
    try { await invoke('sns_send_group_message', { id: group.id, message, requestId: crypto.randomUUID(), imagePath: attachment }); await refresh(); }
    catch (e) { if (alive.current) { setError(String(e)); setInput(previous => previous || message); setImage(previous => previous || attachment); } }
    finally { if (alive.current) setBusy(false); }
  };
  const older = async () => {
    if (!messages[0] || loadingOlder) return; setLoadingOlder(true); nearBottom.current = false;
    const height = list.current?.scrollHeight || 0; const top = list.current?.scrollTop || 0;
    try { const rows = await invoke<Message[]>('sns_group_history', { id: group.id, before: messages[0].id, limit: 81 }); if (!alive.current) return;
      setMore(rows.length > 80); setMessages(previous => [...rows.slice(-80), ...previous]); requestAnimationFrame(() => { if (list.current) list.current.scrollTop = top + list.current.scrollHeight - height; });
    } catch (e) { setError(String(e)); } finally { if (alive.current) setLoadingOlder(false); }
  };
  const chooseImage = async () => { try { const path = await open({ multiple: false, filters: [{ name: t('chat.image_filter'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'] }] }); if (typeof path === 'string') setImage(path); } catch (e) { setError(String(e)); } };
  const bubble = (message: Message, streaming = false) => {
    const person = people.find(p => p.id === message.speaker); const user = message.role === 'user';
    const name = user ? t('chat.sns_you') : String(message.metadata.speaker_name || person?.name || message.speaker);
    const avatar = String(message.metadata.speaker_avatar || person?.avatar || (person ? `/icons/charaters/${person.id}.webp` : '/favicon.ico'));
    return <article className={`sns-message ${user ? 'user' : ''}`} key={message.id}>
      {!user && <img className="sns-speaker-avatar" src={avatar} alt="" onError={e => { e.currentTarget.src = '/favicon.ico'; }} />}
      <div><small>{name} · {new Date(message.timestamp * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}{!user && !person ? ` · ${t('chat.sns_historical_contact')}` : ''}</small>
        <div className="sns-bubble">{typeof message.metadata.image_path === 'string' && <SharedImage path={message.metadata.image_path} />}{message.content !== '📷 [图片]' ? message.content : ''}{streaming && !message.content ? t('chat.typing') : ''}</div>
      </div></article>;
  };
  return <section className="sns-page" aria-label={group.name}>
    <header className="sns-header"><button onClick={onClose}>‹ {t('chat.sns_back')}</button><strong>{group.name} ({group.members.length + 1})</strong><button aria-label={t('chat.sns_group_info')} onClick={onSettings}>•••</button></header>
    <div className="sns-transcript" ref={list} onScroll={() => { const el = list.current; if (el) nearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 90; }}>
      {more && <button onClick={() => void older()} disabled={loadingOlder}>{t('chat.sns_older')}</button>}
      {messages.map(m => bubble(m))}
      {Object.entries(streams).map(([id, stream]) => bubble({ id, speaker: stream.speaker, role: 'assistant', content: stream.text, timestamp: Date.now() / 1000, metadata: {} }, true))}
      {!messages.length && !Object.keys(streams).length && <p className="sns-hint">{t('chat.sns_start_group')}</p>}
    </div>
    {error && <p className="sns-error" role="alert">{error}</p>}
    <div className="sns-input">
      {image && <div className="sns-image-draft"><span>📷 {image.split(/[\\/]/).pop()}</span><button onClick={() => setImage(null)}>×</button></div>}
      <div className="sns-input-row"><button title={t('chat.send_image_entry')} onClick={() => void chooseImage()} disabled={busy}>＋</button>
        <textarea rows={2} value={input} onChange={e => setInput(e.target.value)} placeholder={t('chat.sns_group_placeholder')} onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); void send(); } }} />
        <button className="sns-primary" onClick={() => void send()} disabled={busy || (!input.trim() && !image) || !group.members.length}>{busy ? '…' : t('chat.sns_send')}</button>
      </div>
    </div>
  </section>;
}
