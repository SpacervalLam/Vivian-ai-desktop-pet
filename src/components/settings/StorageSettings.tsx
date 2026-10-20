import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { FolderOpen, RefreshCw } from 'lucide-react';
import ClearConfirmDialog from '../ClearConfirmDialog';

export interface StorageUsage {
  path: string;
  total_bytes: number;
  categories: { id: string; bytes: number }[];
  skipped_entries: number;
}

export function formatStorageBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${Number((bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1))} ${units[index]}`;
}

const colors: Record<string, string> = {
  chats: '#efbf43', work: '#b384ef', memory: '#ed8875', media: '#56bda2', cache: '#73a9da', other: '#929292',
};

export default function StorageSettings({ onManageData }: { onManageData: () => void }) {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('');
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(false);
  const requestId = useRef(0);

  const refresh = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true); setError('');
    try {
      const result = await invoke<StorageUsage>('get_storage_usage');
      if (mounted.current && id === requestId.current) setUsage(result);
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
    setBusy(true); setError(''); setStatus('');
    try { await run(); }
    catch (e) { if (mounted.current) setError(t('storageSettings.actionError', { error: String(e) })); }
    finally { if (mounted.current) setBusy(false); }
  };

  const manageWork = async () => {
    await invoke('edge_menu_action', { action: 'office' });
  };

  const clearCache = () => action(async () => {
    try {
      await invoke('clear_storage_cache');
      if (mounted.current) { setConfirm(false); setStatus(t('storageSettings.cleared')); }
    } finally {
      if (mounted.current) { setConfirm(false); await refresh(); }
    }
  });

  return <div className="settings-storage">
    <h3>{t('storageSettings.device')}</h3>
    <p className="settings-data-help">{t('storageSettings.description')}</p>
    <div className="settings-storage-summary" aria-live="polite">
      <strong>{usage ? t('storageSettings.used', { size: formatStorageBytes(usage.total_bytes) })
        : t(loading ? 'storageSettings.loading' : 'storageSettings.error')}</strong>
      <button type="button" className="settings-data-button" disabled={loading || busy} onClick={() => { setStatus(''); void refresh(); }}>
        <RefreshCw size={14} className={loading ? 'llm-probe-spin' : undefined} />{t('storageSettings.refresh')}
      </button>
    </div>
    {usage && <>
      <div className="settings-storage-bar" role="img" aria-label={usage.categories.map(c => `${t(`storageSettings.${c.id}`)} ${formatStorageBytes(c.bytes)}`).join(', ')}>
        {usage.categories.filter(c => c.bytes > 0).map(c => <span key={c.id} style={{ width: `${usage.total_bytes ? c.bytes / usage.total_bytes * 100 : 0}%`, background: colors[c.id] }} title={`${t(`storageSettings.${c.id}`)} ${formatStorageBytes(c.bytes)}`} />)}
      </div>
      {usage.total_bytes === 0 && <p className="settings-storage-note">{t('storageSettings.empty')}</p>}
      <dl className="settings-storage-legend">{usage.categories.map(c => <div key={c.id}>
        <dt><span style={{ background: colors[c.id] }} />{t(`storageSettings.${c.id}`)}</dt><dd>{formatStorageBytes(c.bytes)}</dd>
      </div>)}</dl>
      <button type="button" className="settings-storage-path" disabled={busy} onClick={() => void action(() => invoke('open_storage_folder'))} title={t('storageSettings.folder')}>
        <FolderOpen size={14} /><span>{usage.path}</span>
      </button>
      <p className="settings-storage-note">{t('storageSettings.scope')}</p>
      {usage.skipped_entries > 0 && <p className="settings-storage-note">{t('storageSettings.partial', { count: usage.skipped_entries })}</p>}
    </>}
    {error && <p className="settings-storage-error" role="alert">{!usage && `${t('storageSettings.error')}：`}{error}</p>}
    {status && <p className="settings-storage-note" role="status">{status}</p>}
    <div className="settings-storage-actions">
      {[
        { title: 'manageChats', help: 'chatsHelp', label: 'view', run: () => action(() => invoke('open_chat_window')) },
        { title: 'manageWork', help: 'workHelp', label: 'manage', run: () => action(manageWork) },
        { title: 'manageData', help: 'dataHelp', label: 'manage', run: onManageData },
        { title: 'clearCache', help: 'cacheHelp', label: 'clear', run: () => setConfirm(true) },
      ].map(row => <div className="settings-data-row" key={row.title}>
        <div className="settings-data-copy"><h3>{t(`storageSettings.${row.title}`)}</h3><p>{t(`storageSettings.${row.help}`)}</p></div>
        <button type="button" className="settings-data-button" disabled={busy || loading} onClick={() => void row.run()}>{t(`storageSettings.${row.label}`)}</button>
      </div>)}
    </div>
    <ClearConfirmDialog open={confirm} loading={busy} onCancel={() => { if (!busy) setConfirm(false); }}
      onConfirm={() => void clearCache()} title={t('storageSettings.confirmTitle')} message={t('storageSettings.confirmMessage')}
      confirmLabel={t('storageSettings.clear')} loadingLabel={t('storageSettings.clearing')} />
  </div>;
}
