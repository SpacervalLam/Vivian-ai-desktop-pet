import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { NotebookPen, Trash2 } from 'lucide-react';
import './DesktopAssistantPanel.css';

const showErrorToast = (error: unknown) => {
  void emit('toast:show', { message: error instanceof Error ? error.message : String(error), type: 'error', duration: 5000, key: Date.now() });
};

/** Notes live inside the phone. Launch shortcuts now belong to PhoneHomeScreen. */
export default function DesktopAssistantPanel(_props: { initialCharacter?: string; panel?: string } = {}) {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  useEffect(() => {
    void invoke('ensure_chat_interactive').catch(() => {});
    return () => { void invoke('ensure_chat_interactive').catch(() => {}); };
  }, []);
  return <main data-panel="notes" className="desktop-assistant"><div className="assistant-scroll"><div className="assistant-page"><div className="assistant-body">
    <div className="assistant-intro"><NotebookPen size={19} /><div><h2>{label('留住此刻的想法', '今のひらめきを残そう', 'A little space for your thoughts')}</h2><p>{label('灵感、琐事，或一句想记住的话。', 'ひらめきも、日々の小さなことも。', 'Ideas, little things, and words to keep.')}</p></div></div>
    <section className="assistant-card assistant-editor"><textarea aria-label={label('随手记', 'メモ', 'Note')} placeholder={label('想到什么就写下来，不用整理。', '思いついたことをそのまま書いて。', 'Write it down as it comes.')} value={note} maxLength={10_000} onChange={e => setNote(e.target.value)} /><div className="assistant-editor-footer"><span className="assistant-count">{[...note].length}<b> / 10,000</b></span><button className="assistant-primary" disabled={busy || !note.trim()} onClick={() => {
      if (busyRef.current) return;
      busyRef.current = true; setBusy(true);
      void invoke('user_quick_notes_save', { content: note.trim() }).then(() => setNote('')).catch(showErrorToast).finally(() => { busyRef.current = false; setBusy(false); });
    }}>{label('保存', '保存', 'Save')}</button></div></section>
    <section className="assistant-card assistant-notes-archive"><p className="assistant-card-title">{label('已记录', '保存したメモ', 'Saved notes')}</p><UserQuickNotes /></section>
  </div></div></div></main>;
}

function UserQuickNotes() {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [notes, setNotes] = useState<Array<{ id: string; title: string; content: string; created_at: number }>>([]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true, revision = 0;
    let cleanup: (() => void) | undefined;
    const update = () => {
      const request = ++revision;
      void invoke<typeof notes>('user_quick_notes_list')
        .then(next => { if (active && request === revision) setNotes(next); }).catch(e => { if (active && request === revision) showErrorToast(e); });
    };
    void listen('user_quick_notes:changed', update).then(unlisten => { if (active) { cleanup = unlisten; update(); } else unlisten(); }).catch(e => { if (active) showErrorToast(e); });
    return () => { active = false; cleanup?.(); };
  }, []);
  return <div className="assistant-notes-list">
    {!notes.length && <p className="assistant-muted">{label('暂无随手记', 'メモはまだありません', 'No notes yet')}</p>}
    {notes.map(note => <details className="assistant-details" key={note.id}>
      <summary>{note.title}</summary>
      <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{note.content}</p>
      <button className="assistant-icon-button" aria-label={label('删除随手记', 'メモを削除', 'Delete note')} disabled={busy} onClick={() => {
        setBusy(true);
        void invoke('user_quick_notes_delete', { id: note.id }).then(() => setNotes(previous => previous.filter(item => item.id !== note.id)))
          .catch(showErrorToast).finally(() => setBusy(false));
      }}><Trash2 size={16} /></button>
    </details>)}
  </div>;
}
