import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FileText, RefreshCw, Search } from 'lucide-react';
import { formatCount, seriesColors } from './UsageCharts';
import type { UsageInsightsCopy } from './usageInsightsCopy';

interface Snapshot { character_id: string; character_name: string; timestamp: number; total_tokens: number; total_chars: number; sections: number }
interface PromptSection { name: string; section_id: string; layer: string; token_estimate: number; char_count: number; full_content: string; present: boolean }
interface Prompt { character_id: string; timestamp: number; total_tokens: number; total_chars: number; sections: PromptSection[]; api_params: { label: string; content: string; present: boolean }[] }

export default function UsageContextPanel({ copy, locale }: { copy: UsageInsightsCopy; locale: string }) {
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [selected, setSelected] = useState('');
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [detailError, setDetailError] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState('size');
  const [revision, setRevision] = useState(0);
  const sequence = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++sequence.current;
    setLoading(true); setError(false);
    try {
      const rows = await invoke<Snapshot[]>('get_usage_prompt_snapshots');
      if (request !== sequence.current) return;
      setSnapshots(rows);
      setSelected(current => rows.some(row => row.character_id === current) ? current : rows[0]?.character_id ?? '');
      setRevision(value => value + 1);
    } catch { if (request === sequence.current) setError(true); }
    finally { if (request === sequence.current) setLoading(false); }
  }, []);
  useEffect(() => { void refresh(); return () => { sequence.current++; }; }, [refresh]);
  useEffect(() => {
    let active = true;
    setPrompt(null); setQuery(''); setDetailError(false);
    if (!selected) { setDetailLoading(false); return; }
    setDetailLoading(true);
    void invoke<Prompt | null>('get_last_prompt_breakdown', { characterId: selected })
      .then(value => { if (active) setPrompt(value); })
      .catch(() => { if (active) setDetailError(true); })
      .finally(() => { if (active) setDetailLoading(false); });
    return () => { active = false; };
  }, [selected, revision]);
  const current = snapshots.find(row => row.character_id === selected);
  const detail = prompt?.character_id === selected ? prompt : null;
  const injected = useMemo(() => detail?.sections.filter(section => section.present || section.char_count > 0) ?? [], [detail]);
  const tokens = injected.reduce((total, section) => total + section.token_estimate, 0);
  const sections = useMemo(() => {
    const words = query.toLocaleLowerCase().trim();
    const rows = injected.filter(section => `${section.name} ${section.section_id} ${section.layer} ${section.full_content}`.toLocaleLowerCase().includes(words));
    return sort === 'size' ? [...rows].sort((a, b) => b.token_estimate - a.token_estimate) : rows;
  }, [injected, query, sort]);
  return <div className="usage-context-panel" aria-busy={loading || detailLoading}>
    <div className="usage-card-heading"><h3>{copy.snapshotTitle}</h3><button type="button" className="usage-icon-button" aria-label={copy.refresh} title={copy.refresh} disabled={loading} onClick={() => void refresh()}><RefreshCw size={15} className={loading ? 'usage-spinning' : ''} /></button></div>
    <p className="usage-help">{copy.snapshotHelp}</p>
    {error && <div className="usage-error" role="alert">{copy.snapshotError}<button type="button" onClick={() => void refresh()}>{copy.retry}</button></div>}
    {loading && !snapshots.length && <p role="status">{copy.loading}</p>}
    {!loading && !error && !snapshots.length && <div className="usage-empty"><FileText size={30} /><h3>{copy.snapshotEmpty}</h3><p>{copy.snapshotEmptyHelp}</p></div>}
    <div className="usage-snapshot-cards">{snapshots.map(snapshot => <button type="button" key={snapshot.character_id} aria-pressed={selected === snapshot.character_id} onClick={() => setSelected(snapshot.character_id)}>
      <strong>{snapshot.character_name}</strong><span>≈ {formatCount(snapshot.total_tokens, locale)}<small>{copy.estimated}</small></span><small>{snapshot.sections} {copy.sections} · {new Date(snapshot.timestamp * 1000).toLocaleString(locale)}</small>
    </button>)}</div>
    {detailLoading && <p role="status">{copy.loading}</p>}
    {detailError && <div className="usage-error" role="alert">{copy.snapshotError}<button type="button" onClick={() => setRevision(value => value + 1)}>{copy.retry}</button></div>}
    {!detailLoading && !detailError && current && !detail && <p className="usage-help">{copy.snapshotEmpty}</p>}
    {detail && <section className="usage-card usage-prompt-detail">
      <div className="usage-card-heading"><h3>{current?.character_name}</h3><span>{copy.snapshotWhen} {new Date(detail.timestamp * 1000).toLocaleString(locale)}</span></div>
      <div className="usage-selected-totals"><strong>≈ {formatCount(detail.total_tokens, locale)} <small>{copy.estimated}</small></strong><span>{formatCount(detail.total_chars, locale)} {copy.characters}</span></div>
      <div className="usage-prompt-stack" aria-label={copy.snapshotTitle} role="img">{injected.map((section, index) => <span key={index} title={`${section.name}: ≈ ${section.token_estimate}`} style={{ flexGrow: section.token_estimate, background: seriesColors[index % seriesColors.length] }} />)}</div>
      <div className="usage-prompt-controls"><div className="usage-search"><Search size={14} /><input aria-label={copy.sectionSearch} placeholder={copy.sectionSearch} value={query} onChange={event => setQuery(event.target.value)} /></div><select aria-label={copy.sectionSort} value={sort} onChange={event => setSort(event.target.value)}><option value="size">{copy.size}</option><option value="original">{copy.original}</option></select></div>
      <div className="usage-prompt-sections">{sections.map((section, index) => <details key={`${section.section_id}:${index}`}>
        <summary title={copy.showContent}><span><strong>{section.name}</strong><small>{section.layer || section.section_id} · {formatCount(section.char_count, locale)} {copy.characters}</small></span><span>≈ {formatCount(section.token_estimate, locale)}<small>{tokens ? (section.token_estimate / tokens * 100).toFixed(1) : '0'}%</small></span></summary>
        <pre>{section.full_content}</pre>
      </details>)}</div>
      {!sections.length && <p className="usage-help">{copy.noSections}</p>}
      {!!detail.api_params?.some(param => param.present) && <section className="usage-prompt-api"><h4>{copy.apiParams}</h4><p className="usage-help">{copy.apiHelp}</p>{detail.api_params.filter(param => param.present).map((param, index) => <details key={index}><summary>{param.label}</summary><pre>{param.content}</pre></details>)}</section>}
    </section>}
  </div>;
}
