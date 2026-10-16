import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { TextField, ToggleField, SelectField, inputStyle, labelStyle, sectionTitleStyle } from '../settings/SettingsFields';
import { parseMcpArguments, parseMcpMap, type McpConfig } from './mcpConfig';

interface ServerView {
  id: string; name: string; enabled: boolean; transport: string; tool_count: number;
  alive: boolean; status: string; last_error: string | null; reconnect_attempts: number;
  source_plugin: string | null; tools: string[];
}
interface Editor { config: McpConfig; args: string; env: string; headers: string; existing: boolean }
const emptyConfig = (): McpConfig => ({ id: '', name: '', transport: 'stdio', command: '', args: [], env: {}, headers: {}, cwd: null, url: null, enabled: true });
const button: React.CSSProperties = { padding: '5px 10px', borderRadius: 7, border: '1px solid var(--panel-border)', background: 'var(--panel-surface)', color: 'var(--panel-text)', cursor: 'pointer' };

export default function McpConnections() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const say = (cn: string, en: string, jp = en) => zh ? cn : ja ? jp : en;
  const [servers, setServers] = useState<ServerView[]>([]);
  const [configs, setConfigs] = useState<McpConfig[]>([]);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [test, setTest] = useState<{ tool_count: number; tools: { name: string; inputSchema: unknown }[] } | null>(null);
  const alive = useRef(true);
  const refresh = useCallback(async (includeConfigs = false) => {
    const rows = await invoke<ServerView[]>('list_mcp_servers');
    if (alive.current) setServers(rows);
    if (includeConfigs) {
      const values = await invoke<McpConfig[]>('list_mcp_server_configs');
      if (alive.current) setConfigs(values);
    }
  }, []);
  useEffect(() => {
    alive.current = true;
    void refresh(true).catch(e => { if (alive.current) setError(String(e)); });
    const interval = window.setInterval(() => { void refresh().catch(e => { if (alive.current) setError(String(e)); }); }, 4000);
    return () => { alive.current = false; window.clearInterval(interval); };
  }, [refresh]);
  const act = async (work: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await work(); await refresh(true); }
    catch (e) { setError(String(e)); await refresh(true).catch(() => {}); }
    finally { if (alive.current) setBusy(false); }
  };
  const edit = (config = emptyConfig(), existing = false) => {
    setEditor({ config: { ...config }, args: JSON.stringify(config.args, null, 2), env: JSON.stringify(config.env, null, 2), headers: JSON.stringify(config.headers ?? {}, null, 2), existing });
    setTest(null); setError('');
  };
  const update = (patch: Partial<McpConfig>) => { if (editor) { setEditor({ ...editor, config: { ...editor.config, ...patch } }); setTest(null); } };
  const parsed = (): McpConfig => {
    if (!editor) throw new Error('No connection selected');
    return { ...editor.config, id: editor.config.id.trim(), name: editor.config.name.trim() || editor.config.id.trim(), args: editor.config.transport === 'stdio' ? parseMcpArguments(editor.args) : editor.config.args, env: editor.config.transport === 'stdio' ? parseMcpMap(editor.env) : editor.config.env, headers: editor.config.transport === 'http' ? parseMcpMap(editor.headers) : editor.config.headers, cwd: editor.config.cwd?.trim() || null };
  };
  const status = (value: string) => ({
    connected: say('已连接', 'Connected', '接続済み'), disabled: say('已停用', 'Disabled', '無効'),
    blocked: say('来源插件不可用', 'Plugin unavailable', 'プラグイン利用不可'),
    failed: say('重试已停止，可手动重连', 'Retries stopped; reconnect manually', '再接続が必要'),
    reconnecting: say('连接失败，等待重试', 'Waiting to retry', '再接続待ち'),
    disconnected: say('未连接', 'Disconnected', '未接続'),
  }[value] ?? value);
  const area = (label: string, field: 'args' | 'env' | 'headers', help: string) => editor && (
    <div style={{ marginBottom: 16 }}><label style={labelStyle}>{label}</label>
      <textarea aria-label={label} rows={field === 'args' ? 3 : 4} value={editor[field]} spellCheck={false} style={{ ...inputStyle, fontFamily: 'monospace', resize: 'vertical' }}
        onChange={e => { setEditor({ ...editor, [field]: e.target.value }); setTest(null); }} />
      <div style={{ fontSize: 11, color: 'var(--panel-text-secondary)' }}>{help}</div>
    </div>
  );
  return <section style={{ marginTop: 28 }}>
    <div style={sectionTitleStyle}>MCP</div>
    <p style={{ fontSize: 12, color: 'var(--panel-text-secondary)' }}>{say('连接本地工具或 Streamable HTTP 服务，工具可用于陪伴和工作会话。', 'Connect local tools or Streamable HTTP services for companion and work sessions.', 'ローカルツールまたは Streamable HTTP サービスを接続します。')}</p>
    {error && <div role="alert" style={{ color: 'var(--panel-danger, #b33)', whiteSpace: 'pre-wrap', marginBottom: 12 }}>{error}</div>}
    {servers.map(server => <div key={server.id} style={{ border: '1px solid var(--panel-border)', borderRadius: 9, padding: 12, marginBottom: 9 }}>
      <div style={{ display: 'flex', gap: 9, flexWrap: 'wrap', alignItems: 'center' }}>
        <strong>{server.name}</strong><code>{server.id}</code><span>{server.transport}</span>
        <span style={{ marginLeft: 'auto', color: server.alive ? 'var(--panel-success)' : 'var(--panel-text-secondary)' }}>{status(server.status)} · {server.tool_count} {say('个工具', 'tools', 'ツール')}</span>
      </div>
      {server.source_plugin && <small>{say('来源插件：', 'Plugin: ', 'プラグイン：')}{server.source_plugin}</small>}
      {server.last_error && <p style={{ margin: '7px 0', fontSize: 12, overflowWrap: 'anywhere' }}>{server.last_error}</p>}
      {server.reconnect_attempts > 0 && <small>{say('已重试', 'Retry attempts', '再試行回数')} {server.reconnect_attempts}/5</small>}
      {server.tools.length > 0 && <details><summary>{say('可用工具', 'Available tools', '利用可能なツール')}</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{server.tools.join('\n')}</pre></details>}
      <div style={{ display: 'flex', gap: 7, marginTop: 9, flexWrap: 'wrap' }}>
        <button type="button" style={button} disabled={busy} onClick={() => { const config = configs.find(c => c.id === server.id); if (config) edit(config, true); }}>{say('编辑', 'Edit', '編集')}</button>
        <button type="button" style={button} disabled={busy || !server.enabled || server.status === 'blocked'} onClick={() => void act(() => invoke('reconnect_mcp_server', { serverId: server.id }))}>{say('重新连接', 'Reconnect', '再接続')}</button>
        <button type="button" style={button} disabled={busy} onClick={() => void act(async () => { const config = configs.find(c => c.id === server.id); if (config) await invoke('add_mcp_server', { config: { ...config, enabled: !server.enabled } }); })}>{server.enabled ? say('停用', 'Disable', '無効化') : say('启用', 'Enable', '有効化')}</button>
        {server.source_plugin ? <button type="button" style={button} disabled={busy} onClick={() => void act(() => invoke('reset_mcp_server_override', { serverId: server.id }))}>{say('恢复插件默认', 'Restore plugin defaults', 'プラグインの既定値に戻す')}</button> : <button type="button" style={button} disabled={busy} onClick={() => void act(() => invoke('remove_mcp_server', { serverId: server.id }))}>{say('移除连接', 'Remove', '削除')}</button>}
      </div>
    </div>)}
    {editor ? <fieldset disabled={busy} style={{ border: '1px solid var(--panel-border)', borderRadius: 9, padding: 14 }}>
      <legend>{say(editor.existing ? '编辑连接' : '添加连接', editor.existing ? 'Edit connection' : 'Add connection', '接続の設定')}</legend>
      <TextField label="ID" value={editor.config.id} disabled={editor.existing} onChange={id => update({ id })} />
      <TextField label={say('名称', 'Name', '名前')} value={editor.config.name} onChange={name => update({ name })} />
      <SelectField label={say('连接方式', 'Transport', '接続方式')} value={editor.config.transport} options={[{ value: 'stdio', label: say('本地进程（stdio）', 'Local process (stdio)') }, { value: 'http', label: 'Streamable HTTP' }]} onChange={transport => update({ transport: transport as McpConfig['transport'] })} />
      {editor.config.transport === 'stdio' ? <>
        <TextField label={say('启动命令', 'Command', '起動コマンド')} value={editor.config.command} onChange={command => update({ command })} placeholder="npx" />
        {area(say('启动参数', 'Arguments', '引数'), 'args', say('支持 JSON 字符串数组或带引号的参数；含空格的路径会保持完整。', 'Use a JSON string array or quoted arguments. Paths with spaces stay intact.'))}
        {area(say('环境变量', 'Environment', '環境変数'), 'env', say('JSON 对象或每行 KEY=VALUE。凭据只保存在本机配置中。', 'JSON object or one KEY=VALUE per line. Credentials are saved in local configuration.'))}
        <TextField label={say('工作目录（可选）', 'Working directory (optional)', '作業ディレクトリ（任意）')} value={editor.config.cwd ?? ''} onChange={cwd => update({ cwd })} />
      </> : <>
        <TextField label={say('服务地址', 'Endpoint URL', 'サービス URL')} value={editor.config.url ?? ''} onChange={url => update({ url })} placeholder="https://example.com/mcp" />
        {area(say('请求头', 'HTTP headers', 'HTTP ヘッダー'), 'headers', say('JSON 对象或每行 KEY=VALUE，例如 Authorization=Bearer …。', 'JSON object or KEY=VALUE lines, e.g. Authorization=Bearer …'))}
      </>}
      <ToggleField label={say('启用连接', 'Enable connection', '接続を有効にする')} value={editor.config.enabled} onChange={enabled => update({ enabled })} />
      <div style={{ display: 'flex', gap: 8 }}>
        <button type="button" style={button} onClick={() => void act(async () => { const result = await invoke<NonNullable<typeof test>>('test_mcp_server', { config: parsed() }); if (alive.current) setTest(result); })}>{say('测试连接', 'Test connection', '接続テスト')}</button>
        <button type="button" style={button} onClick={() => void act(async () => { await invoke('add_mcp_server', { config: parsed() }); if (alive.current) setEditor(null); })}>{say('保存', 'Save', '保存')}</button>
        <button type="button" style={button} onClick={() => setEditor(null)}>{say('取消', 'Cancel', 'キャンセル')}</button>
      </div>
      {test && <div role="status" style={{ marginTop: 10 }}>{say('测试成功，发现', 'Connected, discovered', 'テスト成功：')} {test.tool_count} {say('个工具', 'tools', 'ツール')}<details><summary>{say('查看工具参数', 'Inspect tool schemas', '引数を確認')}</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{JSON.stringify(test.tools, null, 2)}</pre></details></div>}
    </fieldset> : <button type="button" style={button} disabled={busy} onClick={() => edit()}>{say('添加 MCP 连接', 'Add MCP connection', 'MCP 接続を追加')}</button>}
    {busy && <p role="status">{say('正在处理…', 'Working…', '処理中…')}</p>}
  </section>;
}
