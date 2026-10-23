import assert from 'node:assert/strict';
import { build } from 'esbuild';

// Render the real component with lightweight hook and desktop-API adapters. All
// file dialogs and commands are fake: this suite never opens or replaces user data.
const h = globalThis.__configShareHarness = {
  state: [], cursor: 0, calls: [], importing: [], confirm: true, failImport: false,
  invoke: async (command, args) => {
    h.calls.push({ command, args });
    if (command === 'inspect_settings_share') return { characters: ['Alice', 'Bob'] };
    if (command === 'import_settings_share') {
      if (h.failImport) throw new Error('invalid settings');
      return args.includeCharacters ? 2 : 0;
    }
  },
};
globalThis.window = { confirm: () => h.confirm };
const mocks = {
  react: `export const useState = initial => { const h=globalThis.__configShareHarness; const index=h.cursor++; if (!(index in h.state)) h.state[index]=initial; return [h.state[index], value => {h.state[index]=typeof value==='function'?value(h.state[index]):value;}]; };`,
  'react/jsx-runtime': `export const jsx=(type,props)=>({type,props}); export const jsxs=jsx;`,
  '@tauri-apps/api/core': `export const invoke=(...args)=>globalThis.__configShareHarness.invoke(...args);`,
  '@tauri-apps/plugin-dialog': `export const open=async()=>'/share.altn'; export const save=async()=>'/export.altn';`,
  'react-i18next': `export const useTranslation=()=>({i18n:{language:'en'}});`,
  'lucide-react': `export const Download='download-icon'; export const Upload='upload-icon';`,
};
const bundle = await build({
  entryPoints: ['src/components/settings/ConfigTransferSettings.tsx'], bundle: true,
  write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'desktop-test-adapters', setup(build) {
    build.onResolve({ filter: /^(react(?:\/jsx-runtime)?|react-i18next|lucide-react|@tauri-apps\/api\/core|@tauri-apps\/plugin-dialog)$/ }, args => ({ path: args.path, namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ contents: mocks[args.path], loader: 'js' }));
  } }],
});
const { default: Component } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const render = () => { h.cursor = 0; return Component({ onImporting: value => h.importing.push(value) }); };
const elements = tree => !tree || typeof tree !== 'object' ? [] : Array.isArray(tree) ? tree.flatMap(elements) : [tree, ...elements(tree.props?.children)];
const text = node => node == null || typeof node === 'boolean' ? '' : typeof node !== 'object' ? String(node) : Array.isArray(node) ? node.map(text).join('') : text(node.props?.children);
const find = (type, label) => elements(render()).find(node => node.type === type && text(node) === label);
const checks = () => elements(render()).filter(node => node.type === 'input' && node.props.type === 'checkbox');
const click = async label => { const button = find('button', label); assert.ok(button, label); assert.ok(!button.props.disabled, `${label} must be enabled`); button.props.onClick(); await new Promise(setImmediate); };
const commands = name => h.calls.filter(call => call.command === name);

await click('Export .altn');
assert.equal(commands('export_settings_share').at(-1).args.includeCharacters, false);
checks()[0].props.onChange({ target: { checked: true } });
await click('Export .altn');
assert.equal(commands('export_settings_share').at(-1).args.includeCharacters, true);
await click('Choose .altn');
assert.equal(checks()[1].props.checked, false, 'Adding imported characters is optional and off by default');
assert.ok(text(render()).includes('Alice、Bob'));
h.confirm = false;
await click('Import and restart');
assert.equal(commands('import_settings_share').length, 0, 'Cancelling overwrite must not import or restart');
h.confirm = true;
checks()[1].props.onChange({ target: { checked: true } });
h.failImport = true;
await click('Import and restart');
assert.deepEqual(h.importing, [true, false], 'Failure unlocks the settings window');
assert.equal(commands('restart_after_settings_import').length, 0);
assert.ok(text(render()).includes('invalid settings'));
h.failImport = false;
await click('Import and restart');
assert.equal(commands('import_settings_share').at(-1).args.includeCharacters, true);
assert.equal(commands('restart_after_settings_import').length, 1);
assert.ok(find('button', 'Import and restart').props.disabled, 'Successful import prevents duplicate application');
assert.equal(h.importing.at(-1), true, 'Keep stale settings drafts locked until restart');
console.log('Configuration share: optional roles, overwrite cancellation, failure recovery and restart checks passed.');
delete globalThis.__configShareHarness;
delete globalThis.window;
