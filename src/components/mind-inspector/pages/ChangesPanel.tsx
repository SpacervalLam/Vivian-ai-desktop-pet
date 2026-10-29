import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { RefreshCw, FileDiff as FileDiffIcon, MessageSquare, X } from 'lucide-react';
import { diffLines, readReviewComments, reviewPrompt, reviewStorageKey,
  type ChangesSnapshot, type DiffLine, type DiffScope, type FileDiff, type ReviewComment } from './changeReviewModel';
import './changeReviewStrings';
import './ChangesPanel.css';

interface Props {
  workspace: string;
  onOpen: (path: string, line?: number) => void;
  onAddToChat: (text: string) => void;
  fallback: React.ReactNode;
}

export default function ChangesPanel({ workspace, onOpen, onAddToChat, fallback }: Props) {
  const { t } = useTranslation();
  const [scope, setScope] = useState<DiffScope>('all');
  const [snapshot, setSnapshot] = useState<ChangesSnapshot | null>(null);
  const [selected, setSelected] = useState('');
  const [filter, setFilter] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [diffError, setDiffError] = useState('');
  const [diffLoading, setDiffLoading] = useState(false);
  const [revision, setRevision] = useState(0);
  const [refreshTick, setRefreshTick] = useState(0);
  const [anchor, setAnchor] = useState<{ row: DiffLine; revision: string; side: 'old' | 'new' } | null>(null);
  const [body, setBody] = useState('');
  const [comments, setComments] = useState<ReviewComment[]>(() => readReviewComments(workspace));
  const [storageError, setStorageError] = useState(false);
  const listRequest = useRef(0);
  const diffRequest = useRef(0);
  const pendingDiffs = useRef(0);
  const refresh = useCallback(() => setRefreshTick(v => v + 1), []);

  useEffect(() => {
    if (!workspace) return;
    let disposed = false, busy = false;
    const load = async () => {
      if (busy) return;
      busy = true;
      const request = ++listRequest.current;
      setLoading(true);
      try {
        const next = await invoke<ChangesSnapshot>('git_changed_files', { workingDirectory: workspace, scope });
        if (disposed || request !== listRequest.current) return;
        setSnapshot(next); setError('');
        setSelected(current => next.files.some(file => file.path === current) ? current : '');
        setRevision(value => value + 1);
      } catch (e) {
        if (!disposed && request === listRequest.current) setError(String(e));
      } finally {
        busy = false;
        if (!disposed && request === listRequest.current) setLoading(false);
      }
    };
    void load();
    const backgroundLoad = () => { if (!pendingDiffs.current && document.visibilityState === 'visible') void load(); };
    const timer = window.setInterval(backgroundLoad, 5000);
    window.addEventListener('focus', backgroundLoad);
    return () => { disposed = true; window.clearInterval(timer); window.removeEventListener('focus', backgroundLoad); };
  }, [workspace, scope, refreshTick]);

  useEffect(() => {
    const request = ++diffRequest.current;
    setDiffError('');
    if (!selected || !snapshot?.is_repo || error) { setDiff(null); setDiffLoading(false); return; }
    let disposed = false;
    setDiffLoading(true);
    pendingDiffs.current++;
    invoke<FileDiff>('git_file_diff', { workingDirectory: workspace, path: selected, scope })
      .then(next => { if (!disposed && request === diffRequest.current) setDiff(current => current?.revision === next.revision ? current : next); })
      .catch(e => { if (!disposed && request === diffRequest.current) setDiffError(String(e)); })
      .finally(() => { pendingDiffs.current--; if (!disposed && request === diffRequest.current) setDiffLoading(false); });
    return () => { disposed = true; };
  }, [workspace, selected, scope, revision, snapshot?.is_repo, error]);

  useEffect(() => {
    if (!workspace) return;
    try { localStorage.setItem(reviewStorageKey(workspace), JSON.stringify(comments)); setStorageError(false); }
    catch { setStorageError(true); }
  }, [workspace, comments]);

  const file = snapshot?.files.find(item => item.path === selected);
  const rows = useMemo(() => diff ? diffLines(diff.patch) : [], [diff]);
  const selectFile = (path: string) => {
    if (path === selected) return;
    setSelected(path); setDiff(null); setAnchor(null); setBody('');
  };
  const saveComment = () => {
    if (!anchor || !diff || !body.trim() || comments.length >= 100) return;
    const line = anchor.side === 'old' ? anchor.row.oldLine : anchor.row.newLine;
    if (!line) return;
    setComments(previous => [...previous, { id: crypto.randomUUID(), path: selected, scope,
      revision: anchor.revision, side: anchor.side, line,
      quote: anchor.row.text.slice(1), body: body.trim() }]);
    setAnchor(null); setBody('');
  };

  if (!workspace) return <div className="codex-changes-empty">{t('changeReview.noWorkspace')}</div>;
  return <section className="change-review">
    <div className="change-review-heading"><FileDiffIcon size={14} /><strong>{t('changeReview.title')}</strong>
      <button type="button" onClick={refresh} disabled={loading || diffLoading} title={t('changeReview.refresh')} aria-label={t('changeReview.refresh')}><RefreshCw size={14} /></button>
    </div>
    <div className="change-review-scopes" role="group" aria-label={t('changeReview.title')}>
      {(['all', 'staged', 'unstaged'] as const).map(value => <button key={value} type="button" aria-pressed={scope === value}
        onClick={() => { if (scope !== value) { setSnapshot(null); selectFile(''); setScope(value); } }}>{t(`changeReview.${value}`)}</button>)}
    </div>
    {error && <div className="change-review-error" role="alert">{error}<button type="button" onClick={refresh}>{t('changeReview.retry')}</button></div>}
    {loading && !snapshot && <p role="status">{t('changeReview.loading')}</p>}
    {snapshot && !snapshot.is_repo ? <><p>{t('changeReview.notRepo')}</p>{fallback}</> : snapshot && <>
      <input className="change-review-filter" value={filter} onChange={e => setFilter(e.target.value)} placeholder={t('changeReview.search')} aria-label={t('changeReview.search')} />
      {snapshot.truncated && <p role="status">{t('changeReview.truncated')}</p>}
      {!snapshot.files.length && <p>{t('changeReview.empty')}</p>}
      <div className="change-review-files">
        {snapshot.files.filter(item => `${item.path} ${item.original_path ?? ''}`.toLowerCase().includes(filter.toLowerCase())).map(item =>
          <button key={item.path} type="button" className={selected === item.path ? 'selected' : ''} aria-pressed={selected === item.path} title={item.path} onClick={() => selectFile(item.path)}>
            <span className={item.conflict ? 'change-review-conflict' : 'change-review-status'}>{item.index_status}{item.worktree_status}</span>
            <span>{item.original_path ? `${item.original_path} → ${item.path}` : item.path}</span>
            {item.conflict && <span>{t('changeReview.conflict')}</span>}
          </button>)}
      </div>
      {selected ? <div className="change-review-detail">
        <div className="change-review-path"><span>{selected}</span>{file?.worktree_status !== 'D' && file?.index_status !== 'D' &&
          <button type="button" onClick={() => onOpen(`${snapshot.root}/${selected}`)}>{t('changeReview.open')}</button>}</div>
        {diffLoading && <p role="status">{t('changeReview.loading')}</p>}
        {diffError && <p className="change-review-error" role="alert">{diffError}</p>}
        {diff?.binary ? <p>{t('changeReview.binary')}</p> : diff && <>
          <p className="change-review-hint">{t('changeReview.commentHint')}</p>
          <div className="change-review-stats"><span>+{rows.filter(row => row.kind === 'add').length}</span><span>−{rows.filter(row => row.kind === 'delete').length}</span></div>
          {diff.patch ? <div className="change-review-diff" aria-label={selected}>
            {rows.map((row, index) => <div key={index} className={`change-review-line ${row.kind}`}>
              <span className="change-review-gutter">{(['old', 'new'] as const).map(side => {
                const line = side === 'old' ? row.oldLine : row.newLine;
                return line === undefined ? <span key={side} /> : <button key={side} type="button"
                  title={`${t('changeReview.comment')} · ${t(`changeReview.${side}`)} ${line}`}
                  aria-label={`${t('changeReview.comment')} ${t(`changeReview.${side}`)} ${line}`}
                  aria-pressed={anchor?.row === row && anchor.side === side}
                  onClick={() => { setAnchor({ row, revision: diff.revision, side }); setBody(''); }}>{line}</button>;
              })}</span>
              <code>{row.text || ' '}</code>
            </div>)}
          </div> : <p>{t('changeReview.noDiff')}</p>}
        </>}
        {diff?.truncated && <p role="status">{t('changeReview.truncated')}</p>}
        {anchor && <div className="change-review-editor">
          <strong>{t(`changeReview.${anchor.side}`)} : {anchor.side === 'old' ? anchor.row.oldLine : anchor.row.newLine}</strong>
          <pre>{anchor.row.text.slice(1)}</pre>
          {diff && diff.revision !== anchor.revision && <p role="status">{t('changeReview.stale')}</p>}
          <textarea autoFocus value={body} onChange={e => setBody(e.target.value)} maxLength={8000} placeholder={t('changeReview.commentPlaceholder')} aria-label={t('changeReview.commentPlaceholder')} />
          <div><button type="button" disabled={!body.trim() || comments.length >= 100} onClick={saveComment}>{t('changeReview.save')}</button>
            <button type="button" onClick={() => { setAnchor(null); setBody(''); }}>{t('changeReview.cancel')}</button></div>
          {comments.length >= 100 && <p>{t('changeReview.limit')}</p>}
        </div>}
      </div> : snapshot.files.length > 0 && <p>{t('changeReview.choose')}</p>}
    </>}
    {comments.length > 0 && <div className="change-review-comments">
      <strong><MessageSquare size={13} /> {t('changeReview.comments', { count: comments.length })}</strong>
      {comments.map(comment => <article key={comment.id}>
        <div><span>{comment.path} · {t(`changeReview.${comment.scope}`)} · {t(`changeReview.${comment.side}`)}:{comment.line}</span>
          <button type="button" title={t('changeReview.remove')} aria-label={t('changeReview.remove')} onClick={() => setComments(previous => previous.filter(c => c.id !== comment.id))}><X size={13} /></button></div>
        <blockquote>{comment.quote}</blockquote><p>{comment.body}</p>
        {selected === comment.path && scope === comment.scope && diff && diff.revision !== comment.revision && <p role="status">{t('changeReview.stale')}</p>}
      </article>)}
      <p className="change-review-hint">{t('changeReview.captured')}</p>
      {storageError && <p role="alert">{t('changeReview.storageError')}</p>}
      <button type="button" onClick={() => onAddToChat(reviewPrompt(snapshot?.root || workspace, comments, t))}>{t('changeReview.addToChat')}</button>
    </div>}
  </section>;
}
