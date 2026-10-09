import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Search, X, Loader2, MessageSquare } from 'lucide-react';
import { indexSessions, searchSessions, type SearchableSession } from './searchIndex';
import './SessionSearch.css';

function Highlight({ text, query }: { text: string; query: string }) {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return <>{text}</>;
  const lower = text.toLocaleLowerCase();
  const parts: React.ReactNode[] = [];
  let cursor = 0;
  while (cursor < text.length) {
    const hits = terms.map(term => ({ offset: lower.indexOf(term, cursor), length: term.length })).filter(hit => hit.offset >= 0).sort((a, b) => a.offset - b.offset || b.length - a.length);
    const hit = hits[0];
    if (!hit) { parts.push(text.slice(cursor)); break; }
    parts.push(text.slice(cursor, hit.offset), <mark key={hit.offset}>{text.slice(hit.offset, hit.offset + hit.length)}</mark>);
    cursor = hit.offset + hit.length;
  }
  return <>{parts}</>;
}

export default function SessionSearch<T extends SearchableSession>({ sessions, onSelect, onClose, loadMatches }: {
  loadMatches?: (query: string) => Promise<T[]>;
  sessions: T[]; onSelect: (session: T) => void; onClose: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [matches, setMatches] = useState<T[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    setMatches([]); setError(''); setLoading(false);
    if (!query.trim() || !loadMatches) return;
    let canceled = false;
    const timer = setTimeout(() => {
      setLoading(true);
      void loadMatches(query).then(rows => { if (!canceled) setMatches(rows); })
        .catch(error => { if (!canceled) setError(String(error)); })
        .finally(() => { if (!canceled) setLoading(false); });
    }, 200);
    return () => { canceled = true; clearTimeout(timer); };
  }, [query, loadMatches]);
  const index = useMemo(() => indexSessions([...new Map([...sessions, ...matches].map(s => [s.session_id, s])).values()]), [sessions, matches]);
  const results = useMemo(() => searchSessions(index, query), [index, query]);
  const active = Math.min(selected, Math.max(0, results.length - 1));
  const choose = (position: number) => { if (results[position]) { onSelect(results[position].session); onClose(); } };

  useLayoutEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const modal = dialog.current;
    modal?.showModal();
    return () => { modal?.close(); if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => { list.current?.querySelector(`[data-result="${active}"]`)?.scrollIntoView({ block: 'nearest' }); }, [active, query]);

  return <dialog ref={dialog} className="work-search" aria-label={t('workbench.searchTitle')} onCancel={event => { event.preventDefault(); onClose(); }}
    onClick={event => { if (event.target === event.currentTarget) { const bounds = event.currentTarget.getBoundingClientRect(); if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose(); } }}
    onKeyDown={event => {
      event.stopPropagation();
      if (event.nativeEvent.isComposing) return;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setSelected(Math.max(0, Math.min(results.length - 1, active + (event.key === 'ArrowDown' ? 1 : -1)))); }
      else if (event.key === 'Enter' && event.target instanceof HTMLInputElement) { event.preventDefault(); choose(active); }
      else if (event.altKey && /^[1-9]$/.test(event.key)) { event.preventDefault(); choose(Number(event.key) - 1); }
    }}>
    <div className="work-search-input"><Search size={20} aria-hidden /><input autoFocus value={query} onChange={event => { setQuery(event.target.value); setSelected(0); }}
      role="combobox" aria-expanded="true" aria-autocomplete="list" aria-controls="work-search-results" aria-activedescendant={results.length ? `work-search-result-${active}` : undefined}
      aria-label={t('workbench.searchTitle')} placeholder={t('workbench.searchPlaceholder')} />
      <button type="button" aria-label={t('workbench.dismiss')} onClick={onClose}><X size={18} /></button></div>
    <div className="work-search-section" aria-live="polite">{t(query.trim() ? 'workbench.searchResults' : 'workbench.searchRecent', { count: results.length })}</div>
    {loading && <div role="status"><Loader2 size={15} className="codex-spin" /></div>}
    {error && <div role="alert">{error}</div>}
    <div ref={list} id="work-search-results" role="listbox" aria-label={t('workbench.searchTitle')} className="work-search-results">
      {!results.length && <div className="work-search-empty">{t(sessions.length ? 'workbench.searchNoResults' : 'workbench.searchNoSessions')}</div>}
      {results.map(({ session, snippet }, position) => <button type="button" role="option" tabIndex={-1} aria-selected={active === position}
        id={`work-search-result-${position}`} data-result={position} key={session.session_id} className={`work-search-result${active === position ? ' selected' : ''}`} onClick={() => choose(position)}>
        {session.status === 'running' ? <Loader2 size={15} className="codex-spin" aria-label={t('mind_inspector.code_working')} /> : <MessageSquare size={15} aria-hidden />}
        <span className="work-search-result-text"><span className="work-search-result-title"><Highlight text={session.title || t('mind_inspector.code_untitled')} query={query} /></span>
          {snippet && <span className="work-search-snippet"><Highlight text={snippet} query={query} /></span>}</span>
        <span className="work-search-workspace" title={session.working_directory}>{session.working_directory.split(/[\\/]/).filter(Boolean).slice(-1)[0] || t('mind_inspector.code_no_workspace')}</span>
        {position < 9 && <kbd>Alt+{position + 1}</kbd>}
      </button>)}
    </div>
    <div className="work-search-footer">{t('workbench.searchKeyboard')}<kbd>Esc</kbd></div>
  </dialog>;
}
