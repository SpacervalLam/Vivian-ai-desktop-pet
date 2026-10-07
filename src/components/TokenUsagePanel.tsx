import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { Activity, ArrowUpRight, BarChart3, Check, ChevronRight, Cpu, Info, Layers3, RefreshCw, Search, Waypoints, X } from 'lucide-react';
import { usageCopy } from './usage/usageCopy';
import { compactCount as compactUsageCount, CompositionChart, DailyUsageChart, formatCount as formatUsageCount, TokenBreakdown } from './usage/UsageCharts';
import { dimensionRows, LEGACY_KEY, metricValue, routeLabelKeys, selectedDays, sumUsage, tokenTotal, type UsageMetric, type UsageReport, type UsageSelection, type UsageView } from './usage/usageData';
import './usage/TokenUsagePanel.css';

const views = [{ key: 'trend', icon: BarChart3 }, { key: 'models', icon: Cpu }, { key: 'routes', icon: Waypoints }, { key: 'tasks', icon: Layers3 }] as const;

export default function TokenUsagePanel() {
  const { t, i18n } = useTranslation();
  const copy = usageCopy[i18n.language.startsWith('zh') ? 'zh' : i18n.language.startsWith('ja') ? 'ja' : 'en'];
  const formatCount = (value: number) => formatUsageCount(value, copy.locale);
  const compactCount = (value: number) => compactUsageCount(value, copy.locale);
  const [period, setPeriod] = useState(7);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [loadedPeriod, setLoadedPeriod] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [updated, setUpdated] = useState<Date | null>(null);
  const [view, setView] = useState<UsageView>('trend');
  const [metric, setMetric] = useState<UsageMetric>('tokens');
  const [query, setQuery] = useState('');
  const [selection, setSelection] = useState<UsageSelection | null>(null);
  const sequence = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++sequence.current;
    setLoading(true); setError(false);
    try {
      const next = await invoke<UsageReport>('get_token_usage', { days: period });
      if (next.read_error) throw new Error(next.read_error);
      if (request !== sequence.current) return;
      setReport(next); setLoadedPeriod(period); setUpdated(new Date());
    } catch {
      if (request === sequence.current) setError(true);
    } finally {
      if (request === sequence.current) setLoading(false);
    }
  }, [period]);
  useEffect(() => { void refresh(); return () => { sequence.current++; }; }, [refresh]);
  useEffect(() => { setSelection(null); setQuery(''); }, [view, period]);
  // A response for an old period must never appear under a new range label.
  const data = loadedPeriod === period ? report : null;
  const totals = useMemo(() => sumUsage(data?.days ?? []), [data]);
  const total = tokenTotal(totals);
  const hasRecords = !!(total || totals.requests);
  const inputTotal = totals.input + totals.hit + totals.cache_creation;
  const modelCount = data?.models.filter((model) => model.model.trim() && !['unattributed', '未归类'].includes(model.model) && (model.requests || tokenTotal(model))).length ?? 0;
  const routeCalls = data?.routes?.filter((route) => route.route !== 'unattributed').reduce((sum, route) => sum + route.requests, 0) ?? 0;
  const taskCalls = data?.tasks.filter((task) => task.task !== 'unattributed').reduce((sum, task) => sum + task.requests, 0) ?? 0;
  const rowLabel = (key: string, dimension: UsageSelection['dimension']): string => {
    if (key === LEGACY_KEY) return copy.legacy;
    if (key === 'unattributed' || key === '未归类') return dimension === 'models' ? copy.unknownModel : dimension === 'tasks' ? copy.unknownTask : copy.unknownRoute;
    if (dimension === 'models') return key;
    if (routeLabelKeys[key]) return t(routeLabelKeys[key]);
    return copy.purpose[key] ?? key;
  };
  const rows = data && view !== 'trend' ? dimensionRows(data, view, metric) : [];
  const filteredRows = rows.filter((row) => `${row.key} ${rowLabel(row.key, view === 'trend' ? 'models' : view)}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const matrixRoutes = data?.routes?.filter((route) => route.route !== 'unattributed' && `${route.route} ${rowLabel(route.route, 'routes')}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())) ?? [];
  const matrixModels = [...new Set(matrixRoutes.flatMap((route) => route.models.map((model) => model.model)))].sort();
  const matrixMax = Math.max(1, ...matrixRoutes.flatMap((route) => route.models.map((model) => metricValue(model, metric))));
  const detailDays = data && selection ? selectedDays(data, selection) : [];
  const detailTotals = sumUsage(detailDays);
  const changeView = (next: UsageView) => { setSelection(null); setView(next); };
  const searchLabel = view === 'models' ? copy.searchModels : view === 'routes' ? copy.searchRoutes : copy.searchTasks;
  const toggleSelection = (next: UsageSelection) => setSelection((current) =>
    current?.dimension === next.dimension && current.key === next.key && current.model === next.model ? null : next);
  const inlineDetail = selection && <section id="usage-inline-detail" className="usage-selection" aria-label={copy.detail}>
    <div className="usage-selection-heading"><h3>{rowLabel(selection.key, selection.dimension)}{selection.model && <span> / {selection.model}</span>}</h3></div>
    <div className="usage-selected-totals"><strong>{formatCount(tokenTotal(detailTotals))} {copy.tokenUnit}</strong><span>{formatCount(detailTotals.requests)} {copy.calls}</span></div>
    <TokenBreakdown usage={detailTotals} copy={copy} />
    <DailyUsageChart key={`${selection.dimension}/${selection.key}/${selection.model ?? ''}`} days={detailDays} metric={metric} copy={copy} />
  </section>;

  return <section className="usage-dashboard" aria-label={copy.title} aria-busy={loading}>
    <div className="usage-toolbar">
      <div><span className="usage-source"><Check size={12} />{copy.source}</span><p>{copy.subtitle}</p></div>
      <div className="usage-toolbar-controls">
        <select aria-label={copy.date} value={period} onChange={(event) => setPeriod(Number(event.target.value))}>
          {[1, 7, 30, 90].map((days) => <option key={days} value={days}>{days === 1 ? copy.today : `${days} ${copy.days}`}</option>)}
        </select>
        <button type="button" className="usage-icon-button" onClick={() => void refresh()} disabled={loading} aria-label={copy.refresh} title={copy.refresh}>
          <RefreshCw size={15} className={loading ? 'usage-spinning' : ''} />
        </button>
      </div>
    </div>
    <div className="usage-source-note"><Info size={14} /><span>{copy.scopeHelp}</span></div>
    {loading && !data && <div className="usage-empty" role="status"><RefreshCw className="usage-spinning" size={24} /><h3>{copy.loading}</h3></div>}
    {error && <div className="usage-error" role="alert"><Info size={17} /><div><strong>{data ? copy.stale : copy.error}</strong><p>{copy.errorHelp}</p></div><button type="button" onClick={() => void refresh()} disabled={loading}>{copy.retry}</button></div>}
    {!loading && !error && data && !hasRecords && <div className="usage-empty"><Activity size={32} /><h3>{copy.empty}</h3><p>{copy.emptyHelp}</p></div>}
    {data && hasRecords && <>
      <div className="usage-kpis">
        {[
          { label: copy.total, value: compactCount(total), exact: formatCount(total), note: copy.tokenNote, icon: BarChart3 },
          { label: copy.calls, value: formatCount(totals.requests), note: copy.callsNote, icon: Activity },
          { label: copy.cacheShare, value: inputTotal ? `${(totals.hit / inputTotal * 100).toFixed(1)}%` : '—', note: copy.cacheNote, icon: Layers3 },
          { label: copy.activeModels, value: formatCount(modelCount), note: copy.modelNote, icon: Cpu },
        ].map((stat) => <div className="usage-kpi" key={stat.label}><div><span>{stat.label}</span><stat.icon size={15} /></div><strong title={stat.exact}>{stat.value}</strong><small>{stat.note}</small></div>)}
      </div>
      <div className="usage-view-controls">
        <div className="usage-view-tabs" role="tablist" aria-label={copy.title}>
          {views.map(({ key, icon: Icon }) => <button type="button" key={key} role="tab" tabIndex={view === key ? 0 : -1} id={`usage-tab-${key}`} aria-controls="usage-view-panel" aria-selected={view === key}
            onClick={() => changeView(key)} onKeyDown={(event) => {
              const direction = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
              if (!direction) return;
              event.preventDefault();
              const next = views[(views.findIndex((item) => item.key === view) + direction + views.length) % views.length].key;
              changeView(next); document.getElementById(`usage-tab-${next}`)?.focus();
            }}><Icon size={14} />{copy[key]}</button>)}
        </div>
        <div className="usage-metric-switch" aria-label={copy.ranking}>
          {(['tokens', 'requests'] as const).map((key) => <button type="button" key={key} aria-pressed={metric === key} onClick={() => setMetric(key)}>{copy[key]}</button>)}
        </div>
      </div>
      <div id="usage-view-panel" role="tabpanel" aria-labelledby={`usage-tab-${view}`}>
        {view === 'trend' ? <div className="usage-overview"><DailyUsageChart days={data.days} metric={metric} copy={copy} /><CompositionChart usage={totals} copy={copy} /></div> : <>
          <section className="usage-card">
            <div className="usage-card-heading"><h3>{copy[view]} · {copy.ranking}</h3><span>{copy.choose}</span></div>
            {view === 'models' && <p className="usage-help">{copy.modelHelp}</p>}
            {view === 'routes' && <p className="usage-help">{copy.routeHelp}</p>}
            {view !== 'models' && <div className="usage-coverage">
              <div><span>{view === 'routes' ? copy.coverage : copy.taskCoverage}</span><strong>{totals.requests ? ((view === 'routes' ? routeCalls : taskCalls) / totals.requests * 100).toFixed(1) : '0'}%</strong></div>
              <progress max={Math.max(1, totals.requests)} value={view === 'routes' ? routeCalls : taskCalls} />
              <p>{view === 'routes' ? copy.coverageHelp : copy.taskCoverageHelp}</p>
            </div>}
            <div className="usage-search"><Search size={14} /><input aria-label={searchLabel} placeholder={searchLabel} value={query} onChange={(event) => setQuery(event.target.value)} />{query && <button type="button" onClick={() => setQuery('')} aria-label={copy.clearSearch}><X size={13} /></button>}</div>
            <div className="usage-ranking">
              {filteredRows.map((row) => {
                const value = metricValue(row, metric);
                const all = metricValue(totals, metric);
                const share = all ? value / all * 100 : 0;
                const expanded = selection?.dimension === view && selection.key === row.key;
                return <div key={row.key} className="usage-rank-item">
                  <button type="button" className="usage-rank-row" aria-expanded={expanded} aria-controls={expanded ? 'usage-inline-detail' : undefined} onClick={() => toggleSelection({ dimension: view, key: row.key })}>
                  <span className="usage-rank-title"><span title={row.key}>{rowLabel(row.key, view)}</span><strong>{formatCount(value)} <small>{metric === 'tokens' ? copy.tokenUnit : copy.calls}</small></strong><ChevronRight size={14} /></span>
                  <span className="usage-rank-track"><span style={{ width: `${Math.min(100, share)}%` }} /></span>
                  <span className="usage-rank-meta"><span>{metric === 'tokens' ? `${formatCount(row.requests)} ${copy.calls}` : `${formatCount(tokenTotal(row))} ${copy.tokenUnit}`}</span><span>{share.toFixed(1)}% {copy.share}</span></span>
                  </button>
                  {expanded && inlineDetail}
                </div>;
              })}
              {!filteredRows.length && <p className="usage-help">{copy.noMatch}</p>}
            </div>
          </section>
          {view === 'routes' && <section className="usage-card">
            <div className="usage-card-heading"><h3>{copy.matrix}</h3><ArrowUpRight size={16} /></div><p className="usage-help">{copy.matrixHelp}</p>
            {matrixRoutes.length && matrixModels.length ? <div className="usage-table-scroll"><table className="usage-matrix"><thead><tr><th scope="col">{copy.routes}</th>{matrixModels.map((model) => <th scope="col" key={model} title={model}>{model}</th>)}</tr></thead><tbody>
              {matrixRoutes.map((route) => <Fragment key={route.route}><tr><th scope="row">{rowLabel(route.route, 'routes')}<small>{route.route}</small></th>{matrixModels.map((model) => {
                const observed = route.models.find((row) => row.model === model);
                const value = observed ? metricValue(observed, metric) : null;
                return <td key={model}>{value !== null ? <button type="button" style={{ background: `color-mix(in srgb, var(--usage-input) ${12 + value / matrixMax * 50}%, var(--panel-surface))` }}
                  aria-label={`${rowLabel(route.route, 'routes')} / ${model}: ${formatCount(value)} ${metric === 'tokens' ? copy.tokenUnit : copy.calls}`} aria-expanded={selection?.dimension === 'route-model' && selection.key === route.route && selection.model === model}
                  aria-controls={selection?.dimension === 'route-model' && selection.key === route.route && selection.model === model ? 'usage-inline-detail' : undefined}
                  onClick={() => toggleSelection({ dimension: 'route-model', key: route.route, model })}>{compactCount(value)}</button> : <span className="usage-no-cell">—</span>}</td>;
              })}</tr>{selection?.dimension === 'route-model' && selection.key === route.route && <tr className="usage-matrix-detail"><td colSpan={matrixModels.length + 1}>{inlineDetail}</td></tr>}</Fragment>)}
            </tbody></table></div> : <p className="usage-help">{query ? copy.noMatch : copy.noMatrix}</p>}
          </section>}
          <section className="usage-card">
            <div className="usage-card-heading"><h3>{copy.detail}</h3><span>{copy.totalColumn} = {copy.input} + {copy.output} + {copy.hit} + {copy.cache_creation}</span></div>
            <div className="usage-table-scroll"><table className="usage-detail-table"><thead><tr><th scope="col">{copy[view]}</th><th scope="col">{copy.requestColumn}</th>{(['input', 'output', 'hit', 'cache_creation'] as const).map((key) => <th scope="col" key={key}>{copy[key]}</th>)}<th scope="col">{copy.totalColumn}</th></tr></thead><tbody>
              {filteredRows.map((row) => <tr key={row.key}><th scope="row">{rowLabel(row.key, view)}</th><td>{formatCount(row.requests)}</td><td>{formatCount(row.input)}</td><td>{formatCount(row.output)}</td><td>{formatCount(row.hit)}</td><td>{formatCount(row.cache_creation)}</td><td><strong>{formatCount(tokenTotal(row))}</strong></td></tr>)}
            </tbody></table></div>
          </section>
        </>}
      </div>
    </>}
    {updated && data && <div className="usage-updated">{copy.updated} {updated.toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</div>}
  </section>;
}
