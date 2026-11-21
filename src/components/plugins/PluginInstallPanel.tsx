import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';

interface Preview { key: string; name: string; version: string; description: string; previous_version: string | null; fingerprint: string; missing_requirements: string[] }
export default function PluginInstallPanel({ onInstalled }: { onInstalled: () => Promise<void> }) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const say = (cn: string, en: string, jp = en) => zh ? cn : ja ? jp : en;
  const [source, setSource] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const button: React.CSSProperties = { padding: '5px 12px', border: '1px solid var(--panel-border)', borderRadius: 7, background: 'var(--panel-surface)', color: 'var(--panel-text)', cursor: 'pointer' };
  const select = async (directory: boolean) => {
    setError(''); setMessage('');
    try {
      const path = await open({ directory, multiple: false, ...(directory ? {} : { filters: [{ name: 'Plugin ZIP', extensions: ['zip'] }] }) });
      if (typeof path !== 'string') return;
      setBusy(true); setPreview(null); setSource(path);
      setPreview(await invoke<Preview>('inspect_plugin_package', { source: path }));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const install = async () => {
    if (!preview) return;
    setBusy(true); setError('');
    try {
      await invoke('install_plugin_package', { source, fingerprint: preview.fingerprint });
      setPreview(null);
      setMessage(say('已安装。请在下方检查插件并信任，然后启用；更新后的内容需要重新信任。', 'Installed. Review and trust the plugin below before enabling it. Changed packages require renewed trust.', 'インストール完了。下の一覧で内容を確認し、信頼してから有効にしてください。'));
      await onInstalled();
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <div style={{ border: '1px solid var(--panel-border)', borderRadius: 9, padding: 12 }}>
    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
      <button type="button" disabled={busy} style={button} onClick={() => void select(false)}>{say('导入 / 更新 ZIP', 'Import / update ZIP', 'ZIP をインポート・更新')}</button>
      <button type="button" disabled={busy} style={button} onClick={() => void select(true)}>{say('从目录导入', 'Import folder', 'フォルダーからインポート')}</button>
    </div>
    {busy && <p role="status">{say('正在检查或安装…', 'Inspecting or installing…', '確認・インストール中…')}</p>}
    {error && <p role="alert" style={{ color: 'var(--panel-danger, #b33)' }}>{error}</p>}
    {message && <p role="status">{message}</p>}
    {preview && <div style={{ marginTop: 12 }}>
      <strong>{preview.name} · {preview.version}</strong>
      {preview.previous_version && <p>{say('将替换已安装版本：', 'Replaces installed version: ', '置き換えるバージョン：')}{preview.previous_version}</p>}
      <p>{preview.description}</p>
      {preview.missing_requirements.length > 0 && <div role="alert">{say('安装后需补齐依赖才能加载：', 'Install these requirements before loading: ', 'ロード前に必要な依存関係：')}<ul>{preview.missing_requirements.map(item => <li key={item}>{item}</li>)}</ul></div>}
      <details><summary>{say('查看包声明', 'Inspect package manifest', 'パッケージ宣言を確認')}</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 12 }}>{JSON.stringify(preview, null, 2)}</pre></details>
      <div style={{ display: 'flex', gap: 8, marginTop: 9 }}><button type="button" disabled={busy} style={button} onClick={() => void install()}>{say('确认安装', 'Install package', 'インストール')}</button><button type="button" disabled={busy} style={button} onClick={() => setPreview(null)}>{say('取消', 'Cancel', 'キャンセル')}</button></div>
    </div>}
  </div>;
}
