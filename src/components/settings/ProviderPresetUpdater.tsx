import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import { invalidateProviderPresetsCache } from './ModelSettings';
import './ProviderPresetUpdater.css';

export interface UpdateTarget {
  id: string;
  label: string;
  labelKey: string | null;
  modelCount: number;
  verifiedAt: string | null;
  verifiedSource: string | null;
}
export type UpdateStatus = 'queued' | 'running' | 'updated' | 'verified' | 'failed' | 'stopped';
export interface UpdateItem {
  id: string; label: string; status: UpdateStatus; message: string | null;
  sessionId: string | null; source: string | null; added: number; removed: number; modelCount: number;
}
export interface UpdateJob {
  id: string; revision: number; running: boolean; stopRequested: boolean;
  startedAt: string; finishedAt: string | null; items: UpdateItem[];
}
interface UpdateInfo { providers: UpdateTarget[]; job: UpdateJob | null }

const complete = (item: UpdateItem) => item.status !== 'queued' && item.status !== 'running';
const verified = (item: UpdateItem) => item.status === 'updated' || item.status === 'verified';

export default function ProviderPresetUpdater({ charId }: { charId: string | null }) {
  const { t, i18n } = useTranslation();
  const [providers, setProviders] = useState<UpdateTarget[]>([]);
  const [job, setJob] = useState<UpdateJob | null>(null);
  const [selected, setSelected] = useState('');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const mounted = useRef(false);
  const submitting = useRef(false);
  const currentJob = useRef<UpdateJob | null>(null);
  const refreshed = useRef('');
  const fetchVersion = useRef(0);

  const acceptJob = (next: UpdateJob | null) => {
    if (!mounted.current || !next) return;
    const current = currentJob.current;
    if (current && (current.id === next.id ? current.revision > next.revision
      : Date.parse(current.startedAt) > Date.parse(next.startedAt))) return;
    currentJob.current = next;
    setJob(next);
    const successes = next.items.filter(verified);
    const revision = `${next.id}:${successes.map(item => `${item.id}:${item.status}:${item.modelCount}:${item.source}`).join('|')}`;
    if (successes.length && revision !== refreshed.current) {
      refreshed.current = revision;
      invalidateProviderPresetsCache();
      void loadInfo().catch(() => {});
    }
  };
  const loadInfo = async () => {
    const version = ++fetchVersion.current;
    const info = await invoke<UpdateInfo>('provider_preset_update_info');
    if (!mounted.current || version !== fetchVersion.current) return;
    setProviders(info.providers);
    acceptJob(info.job);
  };

  useEffect(() => {
    mounted.current = true;
    let alive = true;
    let polling = false;
    const reportError = (error: unknown) => { if (alive) setError(String(error)); };
    void loadInfo().catch(reportError);
    const timer = window.setInterval(() => {
      if (polling) return;
      polling = true;
      void invoke<UpdateJob | null>('provider_preset_update_status')
        .then(value => { if (alive) acceptJob(value); })
        .catch(reportError).finally(() => { polling = false; });
    }, 1500);
    const subscription = listen<UpdateJob>('provider-presets:update', event => {
      if (alive) acceptJob(event.payload);
    }).catch(() => undefined);
    return () => {
      alive = false; mounted.current = false; fetchVersion.current++;
      window.clearInterval(timer);
      void subscription.then(unlisten => unlisten?.());
    };
  }, []);

  const start = async (ids: string[]) => {
    if (submitting.current || currentJob.current?.running || !ids.length) return;
    submitting.current = true; setPending(true); setError('');
    try {
      const next = await invoke<UpdateJob>('start_provider_preset_update', {
        charId: charId ?? '', providerIds: ids, language: i18n.language,
      });
      acceptJob(next);
    } catch (error) {
      if (mounted.current) {
        setError(String(error));
        void loadInfo().catch(() => {});
      }
    } finally {
      submitting.current = false;
      if (mounted.current) setPending(false);
    }
  };
  const stop = async () => {
    if (!job || submitting.current || job.stopRequested) return;
    submitting.current = true; setPending(true); setError('');
    try {
      await invoke('stop_provider_preset_update', { jobId: job.id });
      const next = await invoke<UpdateJob | null>('provider_preset_update_status');
      acceptJob(next);
    } catch (error) { if (mounted.current) setError(String(error)); }
    finally { submitting.current = false; if (mounted.current) setPending(false); }
  };
  const label = (provider: UpdateTarget) => provider.labelKey ? t(provider.labelKey) : provider.label;
  const busy = pending || !!job?.running;
  const done = job?.items.filter(complete).length ?? 0;
  const successCount = job?.items.filter(verified).length ?? 0;
  const failed = job?.items.filter(item => item.status === 'failed').length ?? 0;
  const active = job?.items.find(item => item.status === 'running');
  const sourceButton = (source: string | null) => {
    if (!source) return null;
    try { const url = new URL(source); if (url.protocol !== 'https:' || url.username || url.password) return null; }
    catch { return null; }
    return <button type="button" className="provider-preset-source" title={source}
      onClick={() => { void openShell(source).catch(error => setError(String(error))); }}>{t('config.preset_update.source')}</button>;
  };

  return <section className="provider-preset-updater" aria-label={t('config.preset_update.title')} aria-busy={busy}>
    <div className="provider-preset-update-heading">{t('config.preset_update.title')}</div>
    <p className="provider-preset-update-help">{t('config.preset_update.help')}</p>
    <div className="provider-preset-update-actions">
      <button type="button" className="provider-preset-update-primary" disabled={busy || !providers.length}
        onClick={() => void start(providers.map(provider => provider.id))}>{t('config.preset_update.all')}</button>
      <select value={selected} disabled={busy || !providers.length} aria-label={t('config.preset_update.vendor')}
        onChange={event => setSelected(event.target.value)}>
        <option value="">{t('config.preset_update.choose')}</option>
        {providers.map(provider => <option key={provider.id} value={provider.id}>{label(provider)}</option>)}
      </select>
      <button type="button" disabled={busy || !providers.some(provider => provider.id === selected)}
        onClick={() => void start([selected])}>{t('config.preset_update.selected')}</button>
      {job?.running && <button type="button" disabled={pending || job.stopRequested} onClick={() => void stop()}>
        {t(job.stopRequested ? 'config.preset_update.stopping' : 'config.preset_update.stop')}
      </button>}
    </div>
    {error && <p role="alert" className="provider-preset-update-error">{error}</p>}
    {job && <div role="status" className="provider-preset-update-progress">
      <span>{t('config.preset_update.progress', { done, total: job.items.length, success: successCount, failed })}</span>
      {job.running && <progress max={job.items.length} value={done} aria-label={t('config.preset_update.progress_label')} />}
      {active && <span>{t('config.preset_update.current', { vendor: label(providers.find(provider => provider.id === active.id) ?? { id: active.id, label: active.label, labelKey: null, modelCount: 0, verifiedAt: null, verifiedSource: null }) })}</span>}
      {job.stopRequested && job.running && <span>{t('config.preset_update.stop_hint')}</span>}
      {!job.running && <span>{t(failed ? 'config.preset_update.partial' : job.stopRequested ? 'config.preset_update.stopped' : 'config.preset_update.done')}</span>}
    </div>}
    <details className="provider-preset-update-details">
      <summary>{t('config.preset_update.details')}</summary>
      <div className="provider-preset-update-table-wrap"><table>
        <thead><tr><th>{t('config.preset_update.vendor')}</th><th>{t('config.preset_update.models')}</th><th>{t('config.preset_update.status')}</th><th>{t('config.preset_update.last_check')}</th></tr></thead>
        <tbody>{providers.map(provider => {
          const item = job?.items.find(item => item.id === provider.id);
          return <tr key={provider.id}>
            <td>{label(provider)}</td><td>{item && verified(item) ? item.modelCount : provider.modelCount}
              {item && verified(item) && (item.added > 0 || item.removed > 0) && <small> +{item.added} / −{item.removed}</small>}</td>
            <td><span className={`provider-preset-update-state ${item?.status ?? 'idle'}`}>{t(`config.preset_update.${item?.status ?? 'idle'}`)}</span>
              {item?.message && <div className="provider-preset-update-message">{item.message}</div>}</td>
            <td>{provider.verifiedAt || t('config.preset_update.unverified')}{sourceButton(item?.source || provider.verifiedSource)}</td>
          </tr>;
        })}</tbody>
      </table></div>
    </details>
  </section>;
}
