import assert from 'node:assert/strict';
import { build } from 'esbuild';

globalThis.__quickInput = { state: null, effects: [], callbacks: {}, calls: [], cleanups: 0 };
const previousDocument = globalThis.document;
globalThis.document = { documentElement: { dataset: {} } };
const built = await build({
  stdin: { contents: "export { default as Window } from './src/components/QuickInputWindow.tsx';", resolveDir: process.cwd(), loader: 'tsx' },
  bundle: true, write: false, format: 'esm', jsx: 'automatic',
  plugins: [{ name: 'window-fixture', setup(builder) {
    builder.onResolve({ filter: /^(react|react\/jsx-runtime|@tauri-apps\/api\/.*)$|\.\/InputDialog$/ }, args => ({ path: args.path, namespace: 'fixture' }));
    builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path === 'react' ? 'export const useState=()=>[globalThis.__quickInput.state,value=>globalThis.__quickInput.state=value],useCallback=callback=>callback,useEffect=callback=>globalThis.__quickInput.effects.push(callback);' :
      path === 'react/jsx-runtime' ? 'export const jsx=(type,props,key)=>({type,props,key}),jsxs=jsx;' :
      path === './InputDialog' ? 'export default function InputDialog(){}' :
      path.endsWith('/core') ? 'export const invoke=async name=>{globalThis.__quickInput.calls.push(name)};' :
      path.endsWith('/event') ? 'export const emit=async(name,payload)=>globalThis.__quickInput.calls.push({name,payload}),listen=async(name,callback)=>{globalThis.__quickInput.callbacks[name]=callback;return()=>globalThis.__quickInput.cleanups++};' :
      'export const getCurrentWindow=()=>({hide:async()=>globalThis.__quickInput.calls.push("hide"),onFocusChanged:async callback=>{globalThis.__quickInput.callbacks.focus=callback;return()=>globalThis.__quickInput.cleanups++}});'
    }));
  } }],
});
try {
  const { Window } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);
  const fixture = globalThis.__quickInput;
  assert.equal(Window(), null);
  const cleanup = fixture.effects[0]();
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(fixture.callbacks['quick_input:configure']);
  assert.ok(fixture.calls.includes('quick_input_ready'));
  const configure = config => fixture.callbacks['quick_input:configure']({ payload: config });
  configure({ character_id: 'nana', broadcast: false, auto_voice: false });
  const privateInput = Window();
  assert.equal(privateInput.props.characterId, 'nana');
  privateInput.props.onSend('hello', true);
  assert.deepEqual(fixture.calls.at(-1), { name: 'quick_input:send_message', payload: { text: 'hello', whisper: true, character_id: 'nana' } });
  configure({ character_id: 'nana', broadcast: false, auto_voice: true });
  const voiceInput = Window();
  assert.equal(voiceInput.key, privateInput.key, 'long press upgrades without losing the draft');
  assert.equal(voiceInput.props.autoStartVoice, true);
  configure({ character_id: 'vivian', broadcast: true, auto_voice: false });
  assert.equal(Window().props.broadcast, true);
  fixture.callbacks.focus({ payload: false });
  assert.equal(fixture.state, null);
  assert.equal(fixture.calls.at(-1), 'hide');
  cleanup();
  assert.equal(fixture.cleanups, 3);
  console.log('Quick input readiness, private routing, whisper, voice upgrade, broadcast and blur cleanup passed.');
} finally {
  delete globalThis.__quickInput;
  if (previousDocument === undefined) delete globalThis.document;
  else globalThis.document = previousDocument;
}
