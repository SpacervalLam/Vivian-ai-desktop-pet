import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as choose } from '@tauri-apps/plugin-dialog';
import { open as openLink } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import { getCharacterId } from '../../characterContext';
import { invalidateProviderPresetsCache } from '../settings/ModelSettings';

interface Gap { id: string; need: string; evidence: string[]; lastSearch: number; searchError: string | null; resolved: boolean }
interface Preview { fingerprint: string; name: string; version: string; missing_requirements: string[]; [key: string]: unknown }
interface Suggestion { id: string; gapId: string; name: string; source: string; revision: string | null; reason: string; status: string; error: string | null; preview: Preview | null; testedTools: string[] }
interface AdvisorState { automatic: boolean; searching: boolean; gaps: Gap[]; suggestions: Suggestion[] }
export default function PluginSuggestionsPanel({ onInstalled }: { onInstalled: () => Promise<void> }) {
  const { i18n } = useTranslation();
  const say = (zh: string, en: string, ja = en) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [state, setState] = useState<AdvisorState | null>(null);
  const [need, setNeed] = useState(''); const [selected, setSelected] = useState(''); const [source, setSource] = useState('');
  const [preview, setPreview] = useState<Suggestion | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [message, setMessage] = useState('');
  const mounted = useRef(false), pending = useRef(false), generation = useRef(0);
  const charId = getCharacterId();
  const load = async () => {
    const version = generation.current;
    const data = await invoke<AdvisorState>('plugin_advisor_state', { charId });
    if (mounted.current && version === generation.current) setState(data);
  };
  useEffect(() => {
    mounted.current = true; let polling = false;
    void load().catch(error => { if (mounted.current) setError(String(error)); });
    const timer = window.setInterval(() => {
      if (pending.current || polling) return;
      polling = true; void load().catch(() => {}).finally(() => { polling = false; });
    }, 3000);
    return () => { mounted.current = false; generation.current++; window.clearInterval(timer); };
  }, [charId]);
  const action = async (action: string, input: Record<string, unknown>) => {
    if (pending.current) return;
    pending.current = true; generation.current++; setBusy(true); setError(''); setMessage('');
    try {
      const result = await invoke<AdvisorState | { state: AdvisorState; candidate?: Suggestion; sessionId?: string }>('plugin_advisor_action', { charId, action, input });
      if (!mounted.current) return;
      setState('state' in result ? result.state : result);
      if ('candidate' in result && result.candidate) setPreview(result.candidate);
      if (action === 'record') setNeed('');
      if (action === 'resolve') { if (selected === input.gapId) setSelected(''); if (preview?.gapId === input.gapId) setPreview(null); }
      if (action === 'dismiss' && preview?.id === input.recommendationId) setPreview(null);
      if (action === 'draft') setMessage(say('已交给工作侧设计，可在工作页查看；创建插件前仍需审核确认。', 'Design started in Work. Plugin creation still requires a concrete review.', '作業側で設計を開始しました。作成前に内容の確認が必要です。'));
      if (action === 'install') {
        setPreview(null); invalidateProviderPresetsCache(); await onInstalled();
        setMessage(say('插件已装载，实际业务能力待真实调用验证。', 'Plugin loaded. Business behavior still needs a real invocation.', 'ロードしました。実際の動作確認は未実施です。'));
      }
    } catch (error) {
      if (mounted.current) { setError(String(error)); await load().catch(() => {}); }
    } finally { pending.current = false; if (mounted.current) setBusy(false); }
  };
  const selectPackage = async (directory: boolean) => {
    try {
      const path = await choose({ directory, multiple: false, ...(directory ? {} : { filters: [{ name: 'ZIP', extensions: ['zip'] }] }) });
      if (typeof path === 'string' && mounted.current) await action('inspect', { gapId: selected, source: path });
    } catch (error) { if (mounted.current) setError(String(error)); }
  };
  const button = { padding: '5px 10px', border: '1px solid var(--panel-border)', borderRadius: 6, background: 'var(--panel-surface)', color: 'var(--panel-text)', cursor: 'pointer', font: 'inherit', fontSize: 12 };
  const inputStyle = { ...button, cursor: 'text', minWidth: 160, flex: 1 };
  const activeGaps = state?.gaps.filter(gap => !gap.resolved) ?? [];
  const disabled = busy || !!state?.searching;
  const labels: Record<string,string> = {
    discovered:say('已核对清单，待检查文件', 'Manifest checked; files not reviewed', '宣言確認済み・ファイル未確認'),
    reviewed:say('待审核安装', 'Ready for review', '承認待ち'),build:say('建议设计', 'Design suggested', '設計を提案'),
    installed:say('已安装，未装载', 'Installed, not loaded', 'インストール済み・未ロード'),loaded:say('已装载，业务待验证', 'Loaded; behavior unverified', 'ロード済み・動作未確認'),
    verified:say('已有成功工具调用', 'Successful tool invocation recorded', 'ツール実行成功を記録'),failed:say('装载失败', 'Loading failed', 'ロード失敗'),dismissed:say('已忽略', 'Dismissed', '非表示'),
  };
  return <section aria-label={say('能力发现与插件建议', 'Capability discovery and plugin suggestions', '機能の発見とプラグイン提案')} style={{ border: '1px solid var(--panel-border)', borderRadius: 9, padding: 12, marginBottom: 16 }}>
    <strong>{say('能力发现与插件建议', 'Capability discovery and plugin suggestions', '機能の発見とプラグイン提案')}</strong>
    <p style={{ fontSize: 12, color: 'var(--panel-text-secondary)', lineHeight: 1.6 }}>{say('记录重复的能力缺口，检查公开 GitHub 仓库根目录的 Vivian 插件清单。自动发现每天最多一次，使用联网搜索额度，只发送通用能力描述；不会安装或启动候选代码。安装前审核来源、固定版本、依赖和完整代码。', 'Tracks recurring capability gaps and checks Vivian manifests at public GitHub repository roots. Automatic discovery runs at most once daily, uses search credits and sends only generic capability descriptions. Candidates run only after package-specific approval.', '繰り返す機能不足から公開 GitHub の Vivian 宣言を確認します。自動検索は全体で1日1回まで。検索枠を使用し、一般的な機能説明のみを送信します。インストールには内容ごとの承認が必要です。')}</p>
    <label style={{ fontSize: 12 }}><input type="checkbox" checked={state?.automatic ?? false} disabled={disabled || !state} onChange={event => void action('automatic', { enabled: event.target.checked })} /> {say('自动发现（全部角色）', 'Automatic discovery (all characters)', '自動検索（全キャラクター）')}</label>
    <div style={{ display: 'flex', gap: 8, marginTop: 10, flexWrap: 'wrap' }}>
      <input aria-label={say('能力描述', 'Capability description', '機能の説明')} value={need} maxLength={120} onChange={event => setNeed(event.target.value)} style={inputStyle} placeholder={say('例如：批量导出日历；不要填写账号、路径或密钥', 'For example: batch calendar export; no accounts, paths or keys', '例：カレンダー一括出力。個人情報やキーは含めないでください')} />
      <button type="button" style={button} disabled={disabled || need.trim().length < 3} onClick={() => void action('record', { need })}>{say('记录能力缺口', 'Record gap', '機能不足を記録')}</button>
    </div>
    <div style={{ display: 'flex', gap: 8, marginTop: 10, flexWrap: 'wrap' }}>
      <select aria-label={say('关联能力缺口', 'Related capability gap', '関連する機能不足')} value={selected} disabled={disabled} onChange={event => setSelected(event.target.value)} style={button}><option value="">{say('选择能力缺口', 'Choose a gap', '機能不足を選択')}</option>{activeGaps.map(gap => <option key={gap.id} value={gap.id}>{gap.need}</option>)}</select>
      <input aria-label={say('候选仓库', 'Candidate repository', '候補リポジトリ')} value={source} onChange={event => setSource(event.target.value)} style={inputStyle} placeholder="https://github.com/owner/repo" />
      <button type="button" style={button} disabled={disabled || !selected || !source.trim()} onClick={() => void action('inspect', { gapId: selected, source })}>{say('检查仓库', 'Inspect repository', 'リポジトリを確認')}</button>
      <button type="button" style={button} disabled={disabled || !selected} onClick={() => void selectPackage(false)}>{say('检查本地 ZIP', 'Inspect local ZIP', 'ローカル ZIP を確認')}</button>
      <button type="button" style={button} disabled={disabled || !selected} onClick={() => void selectPackage(true)}>{say('检查本地目录', 'Inspect local folder', 'ローカルフォルダーを確認')}</button>
    </div>
    {disabled && <p role="status">{say('正在检查或搜索…', 'Inspecting or searching…', '確認・検索中…')}</p>}
    {error && <p role="alert" style={{ color: 'var(--panel-danger, #b33)', overflowWrap: 'anywhere' }}>{error}</p>}
    {message && <p role="status">{message}</p>}
    {!activeGaps.length && state && <p style={{ fontSize: 12 }}>{say('尚无能力缺口记录。', 'No capability gaps recorded.', '機能不足の記録はありません。')}</p>}
    {activeGaps.map(gap => <div key={gap.id} style={{ borderTop: '1px solid var(--panel-border)', marginTop: 12, paddingTop: 10 }}>
      <strong>{gap.need}</strong> <small>({gap.evidence.length})</small>
      <div style={{ display: 'flex', gap: 8, margin: '8px 0', flexWrap: 'wrap' }}>
        <button type="button" style={button} disabled={disabled} onClick={() => void action('search', { gapId: gap.id })}>{say('查找兼容候选', 'Find candidates', '候補を検索')}</button>
        <button type="button" style={button} disabled={disabled} onClick={() => void action('draft', { gapId: gap.id })}>{say('交给工作侧设计', 'Design in Work', '作業側で設計')}</button>
        <button type="button" style={button} disabled={disabled} onClick={() => void action('resolve', { gapId: gap.id })}>{say('关闭此记录', 'Close gap', '記録を終了')}</button>
      </div>
      {!!gap.lastSearch && <small>{say('上次搜索：', 'Last searched: ', '最終検索：')}{new Date(gap.lastSearch * 1000).toLocaleString()}</small>}
      {gap.searchError && <p role="alert" style={{ fontSize: 12, overflowWrap: 'anywhere' }}>{gap.searchError}</p>}
      {state?.suggestions.filter(item => item.gapId === gap.id && item.status !== 'dismissed').map(item => <div key={item.id} style={{ padding: 10, marginTop: 8, background: 'var(--panel-surface)', borderRadius: 6 }}>
        <strong>{item.name}</strong> · {labels[item.status] || item.status}
        <p style={{ fontSize: 12 }}>{item.reason}</p>
        {item.source.startsWith('https://github.com/') && <button type="button" style={button} onClick={() => void openLink(item.source).catch(error => setError(String(error)))}>{say('查看来源', 'View source', '出典を開く')}</button>}
        {item.revision && <p style={{ fontSize: 11, overflowWrap: 'anywhere' }}>{say('固定提交：', 'Pinned commit: ', '固定コミット：')}{item.revision}</p>}
        {item.error && <p role="alert">{item.error}</p>}
        {!!item.testedTools.length && <p>{say('成功调用：', 'Successful calls: ', '成功した呼び出し：')}{item.testedTools.join(', ')}</p>}
        {['discovered','reviewed','failed','installed'].includes(item.status) && <button type="button" style={button} disabled={disabled} onClick={() => void action('inspect', { gapId: gap.id, recommendationId: item.id })}>{say('下载并检查 / 重新检查', 'Download and inspect / recheck', 'ダウンロードして確認・再確認')}</button>}
        <button type="button" style={{ ...button, marginLeft: 8 }} disabled={disabled} onClick={() => void action('dismiss', { recommendationId: item.id })}>{say('忽略此建议', 'Dismiss', 'この提案を非表示')}</button>
      </div>)}
    </div>)}
    {preview?.preview && <div role="region" aria-label={say('安装前审核', 'Review before installation', 'インストール前の確認')} style={{ marginTop: 14, borderTop: '1px solid var(--panel-border)', paddingTop: 10 }}>
      <strong>{preview.preview.name} · {preview.preview.version}</strong><p>{say('确认后将安装并信任本次内容，可能执行脚本或启动 MCP 服务。检查仅验证包结构，实际业务效果尚未验证。', 'Approval installs and trusts this exact package, which may run scripts or start MCP services. Inspection checks structure, not business behavior.', '承認するとこの内容をインストールして信頼します。スクリプトや MCP が実行される場合があります。動作はまだ未確認です。')}</p>
      {!!preview.preview.missing_requirements.length && <p role="alert">{say('请先补齐依赖：', 'Resolve dependencies first: ', '先に依存関係を解決してください：')}{preview.preview.missing_requirements.join('; ')}</p>}
      <details open><summary>{say('完整声明与代码', 'Full declarations and code', '宣言とコードの全文')}</summary><pre style={{ maxHeight: 320, overflow: 'auto', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 11 }}>{JSON.stringify({ source: preview.source, revision: preview.revision, ...preview.preview }, null, 2)}</pre></details>
      <div style={{ display: 'flex', gap: 8, marginTop: 8 }}><button type="button" style={button} disabled={disabled || !!preview.preview.missing_requirements.length} onClick={() => void action('install', { recommendationId: preview.id, fingerprint: preview.preview!.fingerprint })}>{say('确认安装并信任', 'Install and trust this package', 'この内容をインストールして信頼')}</button><button type="button" style={button} disabled={busy} onClick={() => setPreview(null)}>{say('取消', 'Cancel', 'キャンセル')}</button></div>
    </div>}
  </section>;
}
