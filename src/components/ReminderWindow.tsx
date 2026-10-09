import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { playCompanionTone } from '../utils/companionSound';
import './ReminderWindow.css';

interface Notice {
  id: string; task_id: string; character_id: string; content: string; delivery_id: string;
  scheduled_time: number; created_at: number; important: boolean;
}
export default function ReminderWindow() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const words = zh ? ['日程提醒', '知道了', '5 分钟后提醒', '打开日程', '收起', '暂无待确认的提醒']
    : ja ? ['リマインダー', '確認した', '5分後に再通知', '予定を開く', '閉じる', '未確認のリマインダーはありません']
      : ['Reminders', 'Got it', 'Remind in 5 min', 'Open schedule', 'Hide', 'No reminders awaiting confirmation'];
  const [notices, setNotices] = useState<Notice[]>([]);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const sounded = useRef(new Set<string>());
  const generation = useRef(0);
  const alive = useRef(false);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    const next = await invoke<Notice[]>('pending_reminder_notices');
    if (!alive.current || request !== generation.current) return;
    setNotices(next);
    await getCurrentWindow().setAlwaysOnTop(next.some(n => n.important));
    const win = getCurrentWindow();
    if (!(await win.isVisible()) || await win.isMinimized()) return;
    // The surface accepting a card acknowledges transport, not reading.
    for (const notice of next) {
      if (!alive.current || request !== generation.current) return;
      const accepted = await invoke<boolean>('acknowledge_reminder_delivery', { deliveryId: notice.delivery_id });
      if (accepted && !sounded.current.has(notice.id)) {
        sounded.current.add(notice.id);
        const sound = await invoke<boolean>('get_config', { key: 'companion.sound' });
        if (alive.current && sound !== false) playCompanionTone();
      }
    }
  }, []);
  useEffect(() => {
    alive.current = true;
    let cancelled = false;
    const cleanup: UnlistenFn[] = [];
    const update = () => { if (!cancelled) void refresh().catch(e => { if (!cancelled) setError(String(e)); }); };
    void (async () => {
      for (const name of ['reminder:deliver', 'reminder:changed', 'reminder:surface-shown']) {
        const unlisten = await listen(name, update);
        if (cancelled) unlisten(); else cleanup.push(unlisten);
      }
      update();
    })().catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; alive.current = false; ++generation.current; cleanup.forEach(unlisten => unlisten()); };
  }, [refresh]);
  const acknowledge = async (id: string, snooze: boolean) => {
    if (busy) return;
    setBusy(id); setError('');
    try {
      await invoke('acknowledge_reminder_notice', { noticeId: id, snooze });
      await refresh();
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  return <main className="reminder-window">
    <header data-tauri-drag-region><strong data-tauri-drag-region>{words[0]}</strong><button onClick={() => void getCurrentWindow().hide()}>{words[4]}</button></header>
    {error && <p role="alert">{error}</p>}
    <section aria-live="polite">
      {!notices.length && <p className="reminder-empty">{words[5]}</p>}
      {notices.map(notice => <article key={notice.id}>
        <small>{notice.character_id === 'nana' ? 'Nana' : 'Vivian'} · {new Date(notice.scheduled_time * 1000).toLocaleString(i18n.language)}</small>
        <p>{notice.content}</p>
        <footer>
          <button disabled={busy !== null} onClick={() => void acknowledge(notice.id, false)}>{words[1]}</button>
          <button disabled={busy !== null} onClick={() => void acknowledge(notice.id, true)}>{words[2]}</button>
        </footer>
      </article>)}
    </section>
    <button className="reminder-open" onClick={() => void emitTo('vivian', 'companion:open-scheduler').catch(e => setError(String(e)))}>{words[3]}</button>
  </main>;
}
