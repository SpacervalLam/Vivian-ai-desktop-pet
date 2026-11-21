import { useTranslation } from 'react-i18next';
import { CheckCircle2, CircleAlert, Loader2, MinusCircle, Activity } from 'lucide-react';
import { probeIdentity, type ProbeResult, type ProbeTarget } from './llmProbe';

export default function LlmProbeResults({ targets, results, testing }: {
  targets: (ProbeTarget & { label: string })[];
  results: Record<string, ProbeResult>; testing: boolean;
}) {
  const { t } = useTranslation();
  const groups = new Map<string, typeof targets>();
  for (const target of targets) {
    if (!results[target.key]) continue;
    const id = probeIdentity(target);
    groups.set(id, [...(groups.get(id) ?? []), target]);
  }
  const entries = [...groups.values()];
  const count = (state: ProbeResult['state']) => entries.filter(g => results[g[0].key].state === state).length;
  const done = count('ok') + count('error') + count('skipped');
  return <section className="llm-probe-results" aria-label={t('config.llm_probe_title')}>
    <header className="llm-probe-header">
      <div className="llm-probe-heading"><Activity size={18} aria-hidden="true" /><div>
        <h3>{t('config.llm_probe_title')}</h3>
        <p role="status">{testing ? t('config.llm_test_testing') : t('config.llm_test_summary', { ok: count('ok'), total: entries.length })}</p>
      </div></div>
      <div className="llm-probe-counts">
        <span className="llm-probe-badge" data-state="ok">{t('config.llm_test_ok')} {count('ok')}</span>
        <span className="llm-probe-badge" data-state="error">{t('config.llm_test_failed')} {count('error')}</span>
        {count('skipped') > 0 && <span className="llm-probe-badge" data-state="skipped">{t('config.llm_test_skipped')} {count('skipped')}</span>}
      </div>
    </header>
    <div className="llm-probe-progress" role="progressbar" aria-label={t('config.llm_probe_title')} aria-valuemin={0} aria-valuemax={entries.length} aria-valuenow={done}>
      <div style={{ width: `${entries.length ? done / entries.length * 100 : 0}%` }} />
    </div>
    <div className="llm-probe-grid">{entries.map(group => {
      const target = group[0]; const r = results[target.key];
      const Icon = r.state === 'ok' ? CheckCircle2 : r.state === 'error' ? CircleAlert : r.state === 'testing' ? Loader2 : MinusCircle;
      const message = r.errorKind === 'region' ? t('config.llm_test_region')
        : r.errorKind === 'quota' ? t('config.llm_test_quota', { seconds: r.retrySeconds }) : r.error;
      return <article className="llm-probe-card" data-state={r.state} key={target.key}>
        <div className="llm-probe-card-head">
          <div className="llm-probe-model"><strong>{target.model || '—'}</strong><span>{target.providerType}{typeof r.elapsedMs === 'number' ? ` · ${r.elapsedMs} ms` : ''}</span></div>
          <span className="llm-probe-badge" data-state={r.state}><Icon size={14} className={r.state === 'testing' ? 'llm-probe-spin' : undefined} aria-hidden="true" />{t(`config.llm_test_${r.state === 'error' ? 'failed' : r.state === 'testing' ? 'testing' : r.state === 'ok' ? 'ok' : 'skipped'}`)}</span>
        </div>
        <div className="llm-probe-routes">{group.map(item => <span key={item.key}>{item.label}</span>)}</div>
        {r.state === 'error' && <p className="llm-probe-error">{r.errorKind ? message : t('config.llm_probe_failed_hint')}</p>}
        {r.state === 'skipped' && <p className="llm-probe-muted">{t('config.llm_probe_missing')}</p>}
        {r.state === 'ok' && r.reply && <p className="llm-probe-reply">{r.reply}</p>}
        {r.state === 'error' && r.error && <details className="llm-probe-details"><summary>{t('config.llm_test_details')}</summary><pre>{r.error}</pre></details>}
      </article>;
    })}</div>
    <p className="llm-probe-footnote">{t('config.llm_probe_shared')}</p>
  </section>;
}
