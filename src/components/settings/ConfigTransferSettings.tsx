import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { Download, Upload } from 'lucide-react';

export default function ConfigTransferSettings({ disabled = false, onImporting }: { disabled?: boolean; onImporting: (value: boolean) => void }) {
  const { i18n } = useTranslation();
  const say = (zh: string, en: string, ja: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [exportCharacters, setExportCharacters] = useState(false);
  const [importCharacters, setImportCharacters] = useState(false);
  const [source, setSource] = useState<{ path: string; characters: string[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('');
  const [imported, setImported] = useState(false);
  const locked = busy || disabled || imported;
  const act = async (run: () => Promise<void>) => {
    setBusy(true); setError(''); setStatus('');
    try { await run(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const filters = [{ name: say('配置分享', 'Settings share', '設定共有'), extensions: ['altn'] }];

  return <section aria-label={say('配置迁移与分享', 'Migrate and share settings', '設定の移行と共有')}>
    <h3>{say('配置迁移与分享', 'Migrate and share settings', '設定の移行と共有')}</h3>
    <p className="settings-data-help">{say(
      '导出已保存的全部设置（含路由矩阵、各角色语音与日记开关、MCP 连接及插件开关）为 .altn 文件。API key 等密钥、记忆、聊天记录和日记内容均不包含。请先保存尚未保存的修改。',
      'Export all saved preferences, including routing, per-character speech and diary settings, MCP connections and plugin switches, to .altn. Credentials, memories, chat history and diary entries are excluded. Save pending edits first.',
      '保存済みの全設定（ルーティング、キャラクター別の音声・日記設定、MCP 接続、プラグイン切替）を .altn に書き出します。認証情報、記憶、会話履歴、日記本文は含みません。変更を先に保存してください。')}</p>
    <div className="settings-data-row">
      <span className="settings-data-icon"><Download size={18} aria-hidden="true" /></span>
      <div className="settings-data-copy">
        <h3>{say('导出配置', 'Export settings', '設定をエクスポート')}</h3>
        <label><input type="checkbox" checked={exportCharacters} disabled={locked} onChange={e => setExportCharacters(e.target.checked)} /> {say('包含聊天角色定义（人设、头像和模型配置）', 'Include chat character definitions (persona, avatar and model)', 'チャットキャラクター定義（人物設定・画像・モデル）を含める')}</label>
      </div>
      <button type="button" className="settings-data-button" disabled={locked} onClick={() => void act(async () => {
        const path = await save({ defaultPath: `vivian_settings_${new Date().toISOString().slice(0, 10)}.altn`, filters });
        if (!path) return;
        await invoke('export_settings_share', { path, includeCharacters: exportCharacters });
        setStatus(say('配置已导出：', 'Settings exported: ', '設定をエクスポートしました：') + path);
      })}>{say('导出 .altn', 'Export .altn', '.altn を保存')}</button>
    </div>
    <div className="settings-data-row">
      <span className="settings-data-icon"><Upload size={18} aria-hidden="true" /></span>
      <div className="settings-data-copy">
        <h3>{say('导入配置', 'Import settings', '設定をインポート')}</h3>
        <p>{say('覆盖所有设置并重启应用。已有角色和记忆保留；同名导入角色自动添加后缀。服务匹配时保留本机密钥，否则需要重新填写。',
          'Replaces settings and restarts the app. Existing characters and memories are kept; imported names receive a suffix on conflict. Local credentials are kept for matching services; otherwise enter them again.',
          '全設定を置き換えて再起動します。既存キャラクターと記憶は保持され、同名には接尾辞が付きます。同じサービスのローカル認証情報は保持されます。それ以外は再入力が必要です。')}</p>
      </div>
      <button type="button" className="settings-data-button" disabled={locked} onClick={() => void act(async () => {
        const path = await open({ multiple: false, filters });
        if (!path || Array.isArray(path)) return;
        setSource(null); setImportCharacters(false);
        const preview = await invoke<{ characters: string[] }>('inspect_settings_share', { path });
        setSource({ path, characters: preview.characters });
      })}>{say('选择 .altn', 'Choose .altn', '.altn を選択')}</button>
    </div>
    {source && <div className="settings-data-help">
      <p style={{ overflowWrap: 'anywhere' }}>{source.path}</p>
      {source.characters.length > 0 ? <label><input type="checkbox" disabled={locked} checked={importCharacters} onChange={e => setImportCharacters(e.target.checked)} />
        {say(`增加文件中的 ${source.characters.length} 个聊天角色：`, `Add ${source.characters.length} chat characters: `, `${source.characters.length} 人のチャットキャラクターを追加：`)}{source.characters.join('、')}</label>
        : <p>{say('文件未包含聊天角色定义。', 'This file contains no chat characters.', 'キャラクター定義は含まれていません。')}</p>}
      <div className="settings-data-actions">
        <button type="button" className="settings-data-button settings-data-button-primary" disabled={locked} onClick={() => {
          if (!window.confirm(say('确定覆盖所有已保存及未保存的设置并重启吗？记忆不会改变。', 'Replace all saved and unsaved settings and restart? Memories will remain unchanged.', '保存済み・未保存の設定を置き換えて再起動しますか？記憶は変更されません。'))) return;
          void act(async () => {
            onImporting(true);
            let count: number;
            try {
              count = await invoke<number>('import_settings_share', { path: source.path, includeCharacters: importCharacters });
            } catch (e) { onImporting(false); throw e; }
            setImported(true);
            setStatus(say(`配置已导入，新增 ${count} 个角色，正在重启…`, `Settings imported; added ${count} characters. Restarting…`, `設定をインポートし、${count} 人を追加しました。再起動中…`));
            await invoke('restart_after_settings_import');
          });
        }}>{say('导入并重启', 'Import and restart', 'インポートして再起動')}</button>
        <button type="button" className="settings-data-button" disabled={locked} onClick={() => setSource(null)}>{say('取消', 'Cancel', 'キャンセル')}</button>
      </div>
    </div>}
    {imported && <button type="button" className="settings-data-button" onClick={() => void act(async () => { await invoke('restart_after_settings_import'); })}>{say('重新启动应用', 'Restart app', '再起動')}</button>}
    {status && <p role="status" className="settings-data-help" style={{ overflowWrap: 'anywhere' }}>{status}</p>}
    {error && <p role="alert" className="settings-storage-error">{error}</p>}
  </section>;
}
