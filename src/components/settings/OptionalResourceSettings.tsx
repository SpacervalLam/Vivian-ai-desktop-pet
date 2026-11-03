import './OptionalResourceSettings.css';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import {
  installOptionalResource, optionalResourcePacks, optionalResourceStatus, refreshOptionalResources,
  type PackDefinition, type PackStatus, type ResourcePackId, type PackInstallProgress,
} from '../../utils/optionalResources';

/** Packs carry a Chinese label for logs; the settings page localises the same rows. */
const LABELS: Record<string, [string, string]> = {
  'browser-runtime': ['浏览器自动化运行时', 'Browser automation runtime'],
};

/**
 * One manifest, two surfaces. Packs the installer embeds stay in the general page; the
 * separately published ones install on demand and belong next to the plugins, where users
 * already look for something they can add.
 */
export default function OptionalResourceSettings({ scope = 'bundled' }: { scope?: 'bundled' | 'published' }) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const label = (pack: PackDefinition) => {
    const entry = LABELS[pack.id];
    return entry ? (zh ? entry[0] : entry[1]) : pack.label;
  };
  const rows = optionalResourcePacks.filter(pack => scope === 'published' ? Boolean(pack.download) : !pack.download);
  const heading = scope === 'published' && rows.length === 1
    ? label(rows[0])
    : (zh ? '可选资源包' : 'Optional resources');
  const [resources, setResources] = useState<PackStatus[]>(() => optionalResourceStatus());
  const [busy, setBusy] = useState<ResourcePackId | null>(null);
  const [progress, setProgress] = useState<PackInstallProgress | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    listen<PackInstallProgress>('optional-resource:install-progress', event => {
      if (!cancelled) setProgress(event.payload);
    }).then(unlisten => {
      if (cancelled) unlisten(); else stop = unlisten;
    }).catch(() => { /* Browser-only preview has no event bridge. */ });
    refreshOptionalResources().then(() => {
      if (!cancelled) setResources(optionalResourceStatus());
    }).catch(() => {});
    return () => { cancelled = true; stop?.(); };
  }, []);
  const installed = (id: ResourcePackId) => resources.some(pack => pack.id === id && pack.installed);
  const install = async (id: ResourcePackId, source?: string) => {
    setBusy(id); setError(''); setProgress(null);
    try {
      const ok = await installOptionalResource(id, source);
      setResources(optionalResourceStatus());
      if (!ok) setError(zh ? '安装后资源仍不可用，请查看日志。' : 'The pack is still unavailable after installation.');
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  };
  const chooseZip = async (id: ResourcePackId) => {
    try {
      const selected = await open({ multiple: false, filters: [{ name: 'ZIP', extensions: ['zip'] }] });
      if (typeof selected === 'string') await install(id, selected);
    } catch (reason) {
      setError(String(reason));
    }
  };
  const percent = progress?.phase === 'downloading' && progress.total
    ? Math.min(100, Math.round(progress.downloaded * 100 / progress.total)) : null;
  const megs = progress?.phase === 'downloading' ? progress.downloaded / 1048576 : 0;
  const published = scope === 'published';
  return <section className={published ? "optional-resources optional-resources--published" : "optional-resources"} aria-label={heading} style={{ padding: published ? 0 : '12px 0' }}>
    {!published && <strong>{heading}</strong>}
    {rows.map(pack => <div key={pack.id} className={published ? "optional-resource-card" : undefined} style={published
      ? undefined
      : { padding: '8px 0', borderTop: '1px solid rgba(128,128,128,.25)', marginTop: 8 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, justifyContent: 'flex-start', flexWrap: 'wrap' }}>
        <div style={published ? { fontSize: 13, fontWeight: 600, color: 'var(--panel-text)' } : undefined}>
          {published ? label(pack) : `${label(pack)}：`}
        </div>
        {published && <span className="optional-resource-version">v{pack.version}</span>}
        {published && <span className="optional-resource-kind">{zh ? '运行环境' : 'Runtime'}</span>}
        <div className={published ? `optional-resource-badge${installed(pack.id) ? ' is-installed' : ''}` : undefined} style={published ? { fontSize: 11, color: 'var(--panel-text-secondary, #B4B2A9)' } : undefined}>
          {installed(pack.id) ? (zh ? '已安装' : 'Installed') : (zh ? '未安装' : 'Not installed')}
        </div>
      </div>
      {pack.download && (
        <div className="optional-resource-actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 12 }}>
          <button type="button" disabled={busy !== null} onClick={() => void install(pack.id)}>
            {installed(pack.id) ? (zh ? '重新下载安装' : 'Reinstall from download') : (zh ? '下载并安装' : 'Download and install')}
          </button>
          <button type="button" disabled={busy !== null} onClick={() => void chooseZip(pack.id)}>
            {zh ? '从本地 ZIP 安装…' : 'Install from local ZIP…'}
          </button>
        </div>
      )}
      {pack.download && <>
        <p className="optional-resource-description" style={{ fontSize: 12, margin: '6px 0' }}>{zh
          ? `为浏览器自动化提供 Playwright 运行环境，支持执行网页任务与自动操作。按需下载约 ${pack.download.approx_mb} MB。`
          : `Playwright runtime for automated browser tasks and web interactions. Download size: about ${pack.download.approx_mb} MB.`}</p>
        {busy === pack.id && <p role="status" aria-live="polite" className="optional-resource-progress" style={{ fontSize: 12, margin: '10px 0 0' }}>
          {progress?.phase === 'verifying' ? (zh ? '校验并安装…' : 'Verifying and installing…')
            : progress?.phase === 'downloading'
              ? (zh ? `下载中 ${percent !== null ? `${percent}%` : `${megs.toFixed(1)} MB`}`
                : `Downloading ${percent !== null ? `${percent}%` : `${megs.toFixed(1)} MB`}`)
              : (zh ? '准备下载…' : 'Preparing…')}
        </p>}
        {busy === pack.id && percent !== null && <progress className="optional-resource-meter" max={100} value={percent} aria-label={zh ? '下载进度' : 'Download progress'} />}
      </>}
      {!pack.download && !installed(pack.id) && <p style={{ opacity: .7, fontSize: 12, margin: '6px 0' }}>{zh
        ? '可通过安装程序添加。'
        : 'Can be added through Setup.'}</p>}
    </div>)}
    {error && <p className="optional-resource-error" role="alert" style={{ color: '#b3261e', fontSize: 12, margin: '6px 0' }}>{error}</p>}
    {!published && <p style={{ opacity: .7, fontSize: 12 }}>{zh
      ? '字体和贴纸 ZIP 可放在安装程序旁并在安装时勾选。安装后无需重启。'
      : 'Font and sticker ZIPs can sit beside Setup and be selected during installation. No restart is needed.'}</p>}
  </section>;
}
