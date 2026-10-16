import assert from 'node:assert/strict';
import { build } from 'esbuild';

let slots = [], effects = [], cursor = 0, tree;
const timers = new Map();
const same = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
globalThis.__extensionUI = {
  state(initial) { const i = cursor++; if (!(i in slots)) slots[i] = initial; return [slots[i], value => { slots[i] = typeof value === 'function' ? value(slots[i]) : value; }]; },
  ref(initial) { const i = cursor++; return slots[i] ??= { current: initial }; },
  callback(fn, deps) { const i = cursor++; if (!same(slots[i]?.deps, deps)) slots[i] = { fn, deps }; return slots[i].fn; },
  effect(fn, deps) { const i = cursor++; if (!same(effects[i]?.deps, deps)) effects[i] = { fn, deps, pending: true, cleanup: effects[i]?.cleanup }; },
  calls: [], invoke: async () => {}, selected: null,
};
globalThis.window = {
  setInterval(fn) { const id = timers.size + 1; timers.set(id, fn); return id; },
  clearInterval(id) { timers.delete(id); },
};
const mocks = {
  react: `const h=globalThis.__extensionUI;export const useState=h.state,useRef=h.ref,useEffect=h.effect,useCallback=h.callback;export default {createElement:(type,props,...children)=>({type,props:{...props,children}})};`,
  'react/jsx-runtime': `export const jsx=(type,props)=>({type,props});export const jsxs=jsx;export const Fragment='fragment';`,
  'react-i18next': `export const useTranslation=()=>({i18n:{language:'zh'},t:key=>key});`,
  '@tauri-apps/api/core': `export const invoke=(...args)=>globalThis.__extensionUI.invoke(...args);`,
  '@tauri-apps/plugin-dialog': `export const open=async()=>globalThis.__extensionUI.selected;`,
  'lucide-react': `export const Globe='icon',Plug='icon',Unplug='icon',Loader='icon';`,
  fields: `export const TextField='text-field',ToggleField='toggle-field',SelectField='select-field';export const inputStyle={},labelStyle={},sectionTitleStyle={};`,
};
async function load(entry) {
  const bundle = await build({ entryPoints: [entry], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{ name: 'ui-fixtures', setup(b) {
    b.onResolve({ filter: /^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\/|^lucide-react$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onResolve({ filter: /SettingsFields$/ }, () => ({ path: 'fields', namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: mocks[path] }));
  } }] });
  return (await import('data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64'))).default;
}
const walk = node => {
  if (Array.isArray(node)) return node.flatMap(walk);
  if (!node || typeof node !== 'object') return [];
  return [node, ...walk(node.props?.children)];
};
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node) : Array.isArray(node) ? node.map(text).join('') : text(node?.props?.children ?? '');
const find = predicate => { const node = walk(tree).find(predicate); assert.ok(node, 'Expected UI control'); return node; };
const button = label => find(n => n.type === 'button' && text(n) === label);
const field = label => find(n => n.props?.label === label || n.props?.['aria-label'] === label);
let Component, props = {};
const render = () => {
  cursor = 0; tree = Component(props);
  for (const effect of effects.filter(Boolean)) if (effect.pending) { effect.pending = false; effect.cleanup?.(); effect.cleanup = effect.fn(); }
};
const settle = async () => { for (let i = 0; i < 20; i++) { await Promise.resolve(); render(); } };
const click = async label => { button(label).props.onClick(); await settle(); };
function reset() {
  for (const effect of effects.filter(Boolean)) effect.cleanup?.();
  slots = []; effects = []; cursor = 0; props = {}; globalThis.__extensionUI.calls = [];
}

Component = await load('src/components/connections/McpConnections.tsx');
let config = { id: 'test', name: 'Test', transport: 'stdio', command: 'node', args: [], env: {}, cwd: null, enabled: true };
let failed = false;
globalThis.__extensionUI.invoke = async (command, args) => {
  globalThis.__extensionUI.calls.push({ command, args });
  if (command === 'list_mcp_servers') return [{ ...config, alive: false, status: 'reconnecting', tool_count: 0, last_error: 'fixture failure', reconnect_attempts: 1, source_plugin: null, tools: [] }];
  if (command === 'list_mcp_server_configs') return [config];
  if (command === 'test_mcp_server') return { tool_count: 1, tools: [{ name: 'read', inputSchema: { type: 'object' } }] };
  if (command === 'add_mcp_server') { config = args.config; if (failed) throw Error('connection rejected'); return []; }
  throw Error('Unexpected command: ' + command);
};
render(); await settle(); await click('编辑');
field('启动参数').props.onChange({ target: { value: '-y server "G:\\My Files" ""' } }); render();
field('环境变量').props.onChange({ target: { value: 'TOKEN=a=b==\nEMPTY=' } }); render();
field('工作目录（可选）').props.onChange('G:/My Files'); render();
await click('测试连接');
let submitted = globalThis.__extensionUI.calls.find(c => c.command === 'test_mcp_server').args.config;
assert.deepEqual(submitted.args, ['-y', 'server', 'G:\\My Files', '']);
assert.deepEqual(submitted.env, { TOKEN: 'a=b==', EMPTY: '' });
assert.equal(submitted.cwd, 'G:/My Files');
assert.ok(!globalThis.__extensionUI.calls.some(c => c.command === 'add_mcp_server'), 'Testing does not save');
await click('保存'); assert.ok(button('添加 MCP 连接'));
await click('编辑'); failed = true; await click('保存');
assert.ok(text(find(n => n.props?.role === 'alert')).includes('connection rejected'));
assert.ok(field('启动参数'), 'Failed saves retain the editor and persisted connection');
await click('取消'); await click('编辑');
field('连接方式').props.onChange('http'); render();
field('服务地址').props.onChange('https://example.invalid/mcp'); render();
field('请求头').props.onChange({ target: { value: 'Authorization=Bearer synthetic' } }); render();
await click('测试连接');
submitted = globalThis.__extensionUI.calls.filter(c => c.command === 'test_mcp_server').at(-1).args.config;
assert.equal(submitted.transport, 'http'); assert.equal(submitted.url, 'https://example.invalid/mcp');
assert.deepEqual(submitted.headers, { Authorization: 'Bearer synthetic' });
reset(); assert.equal(timers.size, 0, 'Polling stops on unmount');

Component = await load('src/components/plugins/PluginInstallPanel.tsx');
let installed = 0; props = { onInstalled: async () => { installed++; } };
globalThis.__extensionUI.selected = 'G:/sample.zip';
globalThis.__extensionUI.invoke = async (command, args) => {
  globalThis.__extensionUI.calls.push({ command, args });
  if (command === 'inspect_plugin_package') return { key: 'sample', name: 'Sample', version: '2', previous_version: '1', fingerprint: 'reviewed-package', missing_requirements: ['missing runtime'] };
  if (command === 'install_plugin_package') return {};
  throw Error('Unexpected command: ' + command);
};
render(); await click('导入 / 更新 ZIP');
assert.ok(text(tree).includes('missing runtime'));
assert.ok(!globalThis.__extensionUI.calls.some(c => c.command === 'install_plugin_package'));
await click('确认安装');
assert.deepEqual(globalThis.__extensionUI.calls.find(c => c.command === 'install_plugin_package').args, { source: 'G:/sample.zip', fingerprint: 'reviewed-package' });
assert.equal(installed, 1);
assert.ok(!globalThis.__extensionUI.calls.some(c => c.command === 'trust_plugin'), 'Installing never silently trusts code');
reset();
Component = await load('src/components/ConnectionsPanel.tsx');
let browserReady=true;
globalThis.__extensionUI.invoke=async(command,args)=>{
  globalThis.__extensionUI.calls.push({command,args});
  if(command==='get_browser_platforms')return {connected:browserReady,platforms:[{platform:'bilibili',logged_in:false}],reported_at_ms:100,error:browserReady?null:'fixture engine error'};
  if(command==='list_tools')return {tools:[{name:'mcp__browser__snapshot',description:'Read embedded page'}]};
  if(command==='open_embedded_browser')return;
  if(command==='verify_browser_page')return 'Live account: fixture-user';
  throw Error('Unexpected command: '+command);
};
render();await settle();await click('browser.open_browser');await click('browser.go_login');await click('browser.verify_page');
assert.ok(globalThis.__extensionUI.calls.some(c=>c.command==='open_embedded_browser'&&c.args?.url==='https://passport.bilibili.com/login'));
assert.ok(text(tree).includes('Live account: fixture-user'));
browserReady=false;for(const tick of timers.values())tick();await settle();
assert.ok(text(find(n=>n.props?.role==='alert')).includes('fixture engine error'));
reset();assert.equal(timers.size,0,'Browser status polling stops on unmount');
console.log('Extension panels: MCP editing, reviewed ZIP install, native browser login/read/error and polling cleanup passed.');
