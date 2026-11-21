import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { Cable, ChevronRight, Globe, Loader, Package, Pencil, Plus, Power, RotateCcw, Terminal, Trash2, Check } from 'lucide-react';
import { TextField, ToggleField, SelectField, inputStyle, labelStyle } from '../settings/SettingsFields';
import { parseMcpArguments, parseMcpMap, type McpConfig } from './mcpConfig';

interface ServerView {
  id: string; name: string; enabled: boolean; transport: string; tool_count: number;
  alive: boolean; status: string; last_error: string | null; reconnect_attempts: number;
  source_plugin: string | null; tools: string[];
}
interface Editor { config: McpConfig; args: string; env: string; headers: string; existing: boolean }
const emptyConfig = (): McpConfig => ({ id: '', name: '', transport: 'stdio', command: '', args: [], env: {}, headers: {}, cwd: null, url: null, enabled: true });

export default function McpConnections() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const say = (cn: string, en: string, jp = en) => zh ? cn : ja ? jp : en;
  const [servers, setServers] = useState<ServerView[]>([]);
  const [configs, setConfigs] = useState<McpConfig[]>([]);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
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
    void refresh(true).catch(e => { if (alive.current) setError(String(e)); }).finally(() => { if (alive.current) setLoading(false); });
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
  return <section className="mcp-connections">
    <div className="mcp-section-heading">
      <div className="connection-section-heading">
        <div className="connection-title-row"><h3>{say('MCP 服务', 'MCP services', 'MCP サービス')}</h3>
          {servers.length > 0 && <span className="connection-count">{servers.filter(server => server.alive).length}/{servers.length} {say('已连接', 'connected', '接続済み')}</span>}
        </div>
        <p>{say('连接本地工具或远程服务，为陪伴和工作会话扩展能力。', 'Connect local tools or remote services for companion and work sessions.', 'ローカルツールやリモートサービスで会話の機能を拡張します。')}</p>
      </div>
      {!editor && <button type="button" className="connection-button connection-button-primary" disabled={busy} onClick={() => edit()}><Plus size={15} />{say('添加 MCP 连接', 'Add MCP connection', 'MCP 接続を追加')}</button>}
    </div>
    {error && <div role="alert" className="connection-alert">{error}</div>}
    {loading ? <div className="connection-empty-inline" role="status"><Loader size={17} className="connection-spin" />{say('正在加载服务…', 'Loading services…', 'サービスを読み込み中…')}</div>
      : servers.length === 0 && !editor ? <div className="mcp-empty">
        <span className="mcp-empty-icon"><Cable size={26} /></span>
        <strong>{say('还没有 MCP 服务', 'No MCP services yet', 'MCP サービスはまだありません')}</strong>
        <p>{say('添加一个连接，让常用工具在会话中随时可用。', 'Add a connection to make your tools available in conversations.', '接続を追加すると、会話でツールを利用できます。')}</p>
      </div> : null}
    <div className="mcp-server-list">
      {servers.map(server => {
        const config = configs.find(c => c.id === server.id);
        const endpoint = server.transport === 'http' ? config?.url : config?.command;
        return <article key={server.id} className="mcp-server-card">
          <div className="mcp-server-header">
            <span className="connection-service-icon">{server.transport === 'http' ? <Globe size={20} /> : <Terminal size={20} />}</span>
            <div className="connection-service-title">
              <div className="connection-title-row"><h4>{server.name}</h4>
                <span className={`connection-status${server.alive ? ' is-connected' : server.status === 'failed' ? ' is-failed' : ''}`}><span className="connection-status-dot" />{status(server.status)}</span>
              </div>
              <div className="mcp-server-meta">
                <span>{server.transport === 'http' ? 'Streamable HTTP' : say('本地进程', 'Local process', 'ローカルプロセス')}</span>
                <span className="mcp-meta-separator" />
                <span>{server.tool_count} {say('个工具', 'tools', 'ツール')}</span>
                {server.source_plugin && <span className="mcp-plugin-source"><Package size={12} />{server.source_plugin}</span>}
              </div>
            </div>
          </div>
          <div className="mcp-server-identity"><code title={server.id}>{server.id}</code>{endpoint && <span title={endpoint}>{endpoint}</span>}</div>
          {server.last_error && <div className="mcp-server-error" role="status">{server.last_error}
            {server.reconnect_attempts > 0 && <span>{say('已重试', 'Retry attempts', '再試行回数')} {server.reconnect_attempts}/5</span>}
          </div>}
          {server.tools.length > 0 && <details className="mcp-tool-details"><summary><ChevronRight size={14} />{say('可用工具', 'Available tools', '利用可能なツール')}<span>{server.tools.length}</span></summary><div className="mcp-tool-tags">{server.tools.map(tool => <code key={tool}>{tool}</code>)}</div></details>}
          <div className="mcp-server-actions">
            <button type="button" className="connection-button" disabled={busy || !config} onClick={() => { if (config) edit(config, true); }}><Pencil size={13} />{say('编辑', 'Edit', '編集')}</button>
            <button type="button" className="connection-button" disabled={busy || !server.enabled || server.status === 'blocked'} onClick={() => void act(() => invoke('reconnect_mcp_server', { serverId: server.id }))}><RotateCcw size={13} />{say('重新连接', 'Reconnect', '再接続')}</button>
            <button type="button" className="connection-button" disabled={busy || !config} onClick={() => void act(async () => { if (config) await invoke('add_mcp_server', { config: { ...config, enabled: !server.enabled } }); })}><Power size={13} />{server.enabled ? say('停用', 'Disable', '無効化') : say('启用', 'Enable', '有効化')}</button>
            {server.source_plugin ? <button type="button" className="connection-button mcp-remove" disabled={busy} onClick={() => void act(() => invoke('reset_mcp_server_override', { serverId: server.id }))}><RotateCcw size={13} />{say('恢复插件默认', 'Restore plugin defaults', 'プラグインの既定値に戻す')}</button>
              : <button type="button" className="connection-button connection-button-danger mcp-remove" disabled={busy} onClick={() => void act(() => invoke('remove_mcp_server', { serverId: server.id }))}><Trash2 size={13} />{say('移除连接', 'Remove', '削除')}</button>}
          </div>
        </article>;
      })}
    </div>
    {editor ? <fieldset disabled={busy} className="mcp-editor">
      <legend>{say(editor.existing ? '编辑连接' : '添加连接', editor.existing ? 'Edit connection' : 'Add connection', '接続の設定')}</legend>
      <div className="mcp-editor-grid">
      <TextField label="ID" value={editor.config.id} disabled={editor.existing} onChange={id => update({ id })} />
      <TextField label={say('名称', 'Name', '名前')} value={editor.config.name} onChange={name => update({ name })} />
      </div>
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
      <div className="mcp-editor-actions">
        <button type="button" className="connection-button" onClick={() => void act(async () => { const result = await invoke<NonNullable<typeof test>>('test_mcp_server', { config: parsed() }); if (alive.current) setTest(result); })}><Cable size={14} />{say('测试连接', 'Test connection', '接続テスト')}</button>
        <button type="button" className="connection-button connection-button-primary" onClick={() => void act(async () => { await invoke('add_mcp_server', { config: parsed() }); if (alive.current) setEditor(null); })}><Check size={14} />{say('保存', 'Save', '保存')}</button>
        <button type="button" className="connection-button" onClick={() => setEditor(null)}>{say('取消', 'Cancel', 'キャンセル')}</button>
      </div>
      {test && <div role="status" className="mcp-test-result">{say('测试成功，发现', 'Connected, discovered', 'テスト成功：')} {test.tool_count} {say('个工具', 'tools', 'ツール')}<details><summary>{say('查看工具参数', 'Inspect tool schemas', '引数を確認')}</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{JSON.stringify(test.tools, null, 2)}</pre></details></div>}
    </fieldset> : null}
    {busy && <div className="mcp-busy" role="status"><Loader size={14} className="connection-spin" />{say('正在处理…', 'Working…', '処理中…')}</div>}
  </section>;
}
