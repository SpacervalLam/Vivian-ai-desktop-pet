import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowUp, Pencil, Trash2 } from 'lucide-react';
import type useWorkInputQueue from './useWorkInputQueue';
import './workInputStrings';
import './WorkInputQueue.css';

type Controller = ReturnType<typeof useWorkInputQueue>;
export default function WorkInputQueue({ controller }: { controller: Controller }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<string | null>(null);
  const [text, setText] = useState('');
  const { queue, error, busy, action } = controller;
  const run = (operation: string, id?: string, content?: string) => action(operation, id,
    content === undefined ? undefined : { content, images: [], file_refs: [] });
  if (!queue.items.length && !error) return null;
  return <div className="codex-queue work-input-queue">
    <div className="codex-queue-card">
      <div className="codex-queue-head">
        <span className="codex-queue-label">{t('workInput.title', { count: queue.items.length })}</span>
        {queue.items.length > 0 && <button type="button" disabled={busy} onClick={() => { void run(queue.paused ? 'resume' : 'pause').catch(() => {}); }}>{t(queue.paused ? 'workInput.resume' : 'workInput.pause')}</button>}
      </div>
      {queue.paused && queue.items.length > 0 && <p role="status">{t('workInput.paused')}</p>}
      <p className="work-input-hint">{t('workInput.hint')}</p>
      {error && <p role="alert">{error} <button type="button" disabled={busy} onClick={() => { void run('list').catch(() => {}); }}>{t('workInput.retry')}</button></p>}
      {queue.items.map((item, index) => <div key={item.id} className="work-input-item">
        <div className="codex-queue-row">
          <span className="codex-queue-num">#{index + 1}</span>
          <span className="codex-queue-text" title={item.payload.content}>{item.kind === 'steering' && <strong>{t('workInput.steering')} · </strong>}{item.payload.content || t('workInput.attachments')}</span>
          <span className="codex-queue-actions">
            <button type="button" className="codex-queue-btn" disabled={busy || item.kind === 'steering'} title={t('workInput.guide')} aria-label={t('workInput.guide')} onClick={() => { void run('steer', item.id).catch(() => {}); }}><ArrowUp size={14} /></button>
            <button type="button" className="codex-queue-btn" disabled={busy} title={t('workInput.edit')} aria-label={t('workInput.edit')} onClick={() => { setEditing(item.id); setText(item.payload.content); }}><Pencil size={13} /></button>
            <button type="button" className="codex-queue-btn" disabled={busy} title={t('workInput.remove')} aria-label={t('workInput.remove')} onClick={() => { void run('remove', item.id).catch(() => {}); }}><Trash2 size={13} /></button>
          </span>
        </div>
        {(item.payload.images.length > 0 || item.payload.file_refs.length > 0) && <div className="work-input-hint">{t('workInput.attachmentCount', { images: item.payload.images.length, files: item.payload.file_refs.length })}</div>}
        {editing === item.id && <div className="work-input-edit">
          <textarea value={text} maxLength={64000} aria-label={t('workInput.edit')} onChange={event => setText(event.target.value)} />
          <button type="button" disabled={busy || (!text.trim() && !item.payload.images.length && !item.payload.file_refs.length)} onClick={() => {
            void run('edit', item.id, text).then(() => setEditing(null)).catch(() => {});
          }}>{t('workInput.save')}</button>
          <button type="button" onClick={() => setEditing(null)}>{t('workInput.cancel')}</button>
        </div>}
      </div>)}
    </div>
  </div>;
}
