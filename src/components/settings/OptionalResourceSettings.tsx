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
  return <section aria-label={heading} style={{ padding: published ? 0 : '12px 0' }}>
    {!published && <strong>{heading}</strong>}
    {rows.map(pack => <div key={pack.id} style={published
      ? { paddingBottom: 8 }
      : { padding: '8px 0', borderTop: '1px solid rgba(128,128,128,.25)', marginTop: 8 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, justifyContent: published ? 'space-between' : 'flex-start' }}>
        <div style={published ? { fontSize: 14, fontWeight: 700, color: 'var(--panel-text)' } : undefined}>
          {published ? label(pack) : `${label(pack)}：`}
        </div>
        <div style={published ? { fontSize: 12, color: 'var(--panel-text-secondary, #B4B2A9)' } : undefined}>
          {installed(pack.id) ? (zh ? '已安装' : 'Installed') : (zh ? '未安装' : 'Not installed')}
        </div>
      </div>
      {pack.download && <>
        <p style={{ opacity: .7, fontSize: 12, margin: '6px 0' }}>{zh
          ? `为浏览器自动化（browser_execute）提供 Playwright 执行环境，按需下载约 ${pack.download.approx_mb} MB。未安装时该能力不可用，浏览器其余操作与其它功能不受影响。`
          : `Provides the Playwright execution environment behind browser_execute (~${pack.download.approx_mb} MB on demand). That capability stays unavailable until installed; the rest of browsing and everything else keep working.`}</p>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 8 }}>
          <button type="button" disabled={busy !== null} onClick={() => void install(pack.id)}>
            {installed(pack.id) ? (zh ? '重新下载安装' : 'Reinstall from download') : (zh ? '下载并安装' : 'Download and install')}
          </button>
          <button type="button" disabled={busy !== null} onClick={() => void chooseZip(pack.id)}>
            {zh ? '从本地 ZIP 安装…' : 'Install from local ZIP…'}
          </button>
        </div>
        {busy === pack.id && <p style={{ fontSize: 12, margin: '6px 0' }}>
          {progress?.phase === 'verifying' ? (zh ? '校验并安装…' : 'Verifying and installing…')
            : progress?.phase === 'downloading'
              ? (zh ? `下载中 ${percent !== null ? `${percent}%` : `${megs.toFixed(1)} MB`}`
                : `Downloading ${percent !== null ? `${percent}%` : `${megs.toFixed(1)} MB`}`)
              : (zh ? '准备下载…' : 'Preparing…')}
        </p>}
      </>}
      {!pack.download && !installed(pack.id) && <p style={{ opacity: .7, fontSize: 12, margin: '6px 0' }}>{zh
        ? '可通过安装程序添加。'
        : 'Can be added through Setup.'}</p>}
    </div>)}
    {error && <p role="alert" style={{ color: '#b3261e', fontSize: 12, margin: '6px 0' }}>{error}</p>}
    {!published && <p style={{ opacity: .7, fontSize: 12 }}>{zh
      ? '字体和贴纸 ZIP 可放在安装程序旁并在安装时勾选。安装后无需重启。'
      : 'Font and sticker ZIPs can sit beside Setup and be selected during installation. No restart is needed.'}</p>}
  </section>;
}
