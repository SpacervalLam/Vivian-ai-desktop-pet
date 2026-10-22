import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { Copy, FolderOpen, HardDrive, RefreshCw } from 'lucide-react';

interface StorageCategory {
  id: string;
  bytes: number;
  subcategories?: StorageCategory[];
}

export interface StorageUsage {
  path: string;
  total_bytes: number;
  categories: StorageCategory[];
  skipped_entries: number;
}

export function formatStorageBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const index = Math.max(0, Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1));
  return `${Number((bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1))} ${units[index]}`;
}

const colors: Record<string, string> = {
  chats: '#efbf43', work: '#b384ef', memory: '#ed8875', media: '#56bda2', other: '#929292',
  diary: '#db8eaa', notes: '#64a5ca', tasks: '#c9a55e', social: '#73b7a3', companion: '#b19ac9',
  appCache: '#73a9da', browserCache: '#80b7cf', browserData: '#7c91cf', extensions: '#b2ad70',
  runtime: '#9c9fa9', logs: '#bf9981', settings: '#8fa798',
};

export default function StorageSettings() {
  const { t, i18n } = useTranslation();
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [actionError, setActionError] = useState('');
  const [status, setStatus] = useState('');
  const [updatedAt, setUpdatedAt] = useState<Date | null>(null);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(false);
  const requestId = useRef(0);

  const refresh = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true); setError('');
    try {
      const result = await invoke<StorageUsage>('get_storage_usage');
      if (mounted.current && id === requestId.current) {
        setUsage(result);
        setUpdatedAt(new Date());
      }
    } catch (e) {
      if (mounted.current && id === requestId.current) setError(String(e));
    } finally {
      if (mounted.current && id === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; requestId.current++; };
  }, [refresh]);

  const action = async (run: () => Promise<unknown>) => {
    setBusy(true); setActionError(''); setStatus('');
    try { await run(); }
    catch (e) { if (mounted.current) setActionError(t('storageSettings.actionError', { error: String(e) })); }
    finally { if (mounted.current) setBusy(false); }
  };

  const categories = usage ? usage.categories.filter(c => c.bytes > 0).sort((a, b) => b.bytes - a.bytes) : [];
  const percent = (bytes: number) => usage?.total_bytes ? bytes / usage.total_bytes * 100 : 0;
  const formatPercent = (bytes: number) => new Intl.NumberFormat(i18n.language, { style: 'percent', maximumFractionDigits: 1 }).format(percent(bytes) / 100);

  return <div className="settings-storage">
    <div className="settings-storage-location">
      <div className="settings-storage-location-heading">
        <span className="settings-storage-icon"><FolderOpen size={22} /></span>
        <div><h3>{t('storageSettings.location')}</h3><p>{t('storageSettings.folderHelp')}</p></div>
      </div>
      <div className="settings-storage-location-actions">
        <button type="button" className="settings-data-button settings-data-button-primary" disabled={busy}
          onClick={() => void action(() => invoke('open_storage_folder'))}>
          <FolderOpen size={17} />{t('storageSettings.folder')}
        </button>
        {usage && <button type="button" className="settings-data-button" disabled={busy} onClick={() => void action(async () => {
          await navigator.clipboard.writeText(usage.path);
          if (mounted.current) setStatus(t('storageSettings.copied'));
        })}><Copy size={15} />{t('storageSettings.copyPath')}</button>}
      </div>
      {usage && <code className="settings-storage-path">{usage.path}</code>}
      {actionError && <p className="settings-storage-error" role="alert">{actionError}</p>}
      {status && <p className="settings-storage-note" role="status">{status}</p>}
    </div>
    <div className="settings-storage-summary" aria-busy={loading}>
      <div className="settings-storage-total" aria-live="polite">
        <span><HardDrive size={15} />{t('storageSettings.totalLabel')}</span>
        <strong>{usage ? formatStorageBytes(usage.total_bytes) : '—'}</strong>
        <span>{loading ? t('storageSettings.loading') : updatedAt && t('storageSettings.updated', {
          time: updatedAt.toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
        })}</span>
      </div>
      <button type="button" className="settings-data-button" disabled={loading} onClick={() => void refresh()}>
        <RefreshCw size={14} className={loading ? 'llm-probe-spin' : undefined} />{t(loading ? 'storageSettings.refreshing' : 'storageSettings.refresh')}
      </button>
    </div>
    {usage && <>
      <div className="settings-storage-bar" role="img" aria-label={categories.length ? categories.map(c => `${t(`storageSettings.${c.id}`)} ${formatStorageBytes(c.bytes)}`).join(', ') : t('storageSettings.empty')}>
        {categories.filter(c => c.bytes > 0).map(c => <span key={c.id} style={{ width: `${percent(c.bytes)}%`, background: colors[c.id] }} title={`${t(`storageSettings.${c.id}`)} ${formatStorageBytes(c.bytes)} · ${formatPercent(c.bytes)}`} />)}
      </div>
      {usage.total_bytes === 0 && <p className="settings-storage-note">{t('storageSettings.empty')}</p>}
      <dl className="settings-storage-legend">{categories.map(c => <div key={c.id}>
        <dt><span style={{ background: colors[c.id] }} />{t(`storageSettings.${c.id}`)}</dt>
        <dd>
          <div className="settings-storage-category-size"><strong>{formatStorageBytes(c.bytes)}</strong><span>{formatPercent(c.bytes)}</span></div>
          <p className="settings-storage-category-description">{t(`storageSettings.${c.id}Help`)}</p>
          {!!c.subcategories?.some(child => child.bytes > 0) && <ul className="settings-storage-breakdown">
            {c.subcategories.filter(child => child.bytes > 0).sort((a, b) => b.bytes - a.bytes).map(child => <li key={child.id}>
              <span>{t(`storageSettings.${child.id}`)}</span><strong>{formatStorageBytes(child.bytes)}</strong>
            </li>)}
          </ul>}
        </dd>
      </div>)}</dl>
      <p className="settings-storage-note">{t('storageSettings.scope')}</p>
      {usage.skipped_entries > 0 && <p className="settings-storage-note">{t('storageSettings.partial', { count: usage.skipped_entries })}</p>}
    </>}
    {error && <p className="settings-storage-error" role="alert">{t('storageSettings.error')}：{error}</p>}
  </div>;
}
