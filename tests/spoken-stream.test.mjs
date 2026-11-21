import assert from 'node:assert/strict';
import { build } from 'esbuild';

globalThis.window = globalThis;
globalThis.__speech = { events: {}, calls: [], bubbles: [], thinking: [], completed: [] };
const bundle = await build({ entryPoints: ['src/controllers/ChatController.ts'], bundle: true, write: false,
  platform: 'node', format: 'esm', plugins: [{ name: 'spoken-stream-fixture', setup(b) {
    b.onResolve({ filter: /@tauri-apps\/api|useAppStore|BubbleController|TtsStreamQueue|characterContext|\/i18n$|companionLatency/ }, args => ({ path: args.path, namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents:
      path.includes('/core') ? `export const invoke=async(name,args)=>{globalThis.__speech.calls.push({name,args});return [{reply:'reply'}]};` :
      path.includes('/event') ? `export const emit=async()=>{},listen=async(name,callback)=>{globalThis.__speech.events[name]=callback;return()=>{}};` :
      path.includes('BubbleController') ? `export const BubbleController=new Proxy({}, {get:(_,name)=>(...args)=>{globalThis.__speech.bubbles.push({name,args});return()=>{}}});` :
      path.includes('TtsStreamQueue') ? `export const TtsStreamQueue=new Proxy({}, {get:(_,name)=>name==='isEnabled'?()=>false:()=>{}});` :
      path.includes('useAppStore') ? `export const useAppStore={getState:()=>({setLastUserEmotion:()=>{}})};` :
      path.includes('characterContext') ? `export const getCharacterId=()=>'nana';` :
      path.includes('companionLatency') ? `export const beginCompanionTurn=()=>{};` :
      `export default {t:key=>key};`
    }));
  } }] });
const { ChatController } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const fixture = globalThis.__speech;
const event = async (name, payload) => fixture.events[name]({ payload });
try {
  await ChatController.init();
  ChatController.setHandlers({ onThinkingStarted: id => fixture.thinking.push(id), onResponseReceived: response => fixture.completed.push(response.text) });
  await ChatController.sendMessage('hello', 'vivian', 'direct');
  assert.deepEqual(fixture.calls, [{ name: 'send_spoken_message', args: { message: 'hello', fileMetadata: undefined } }]);
  assert.equal(ChatController.isStreaming, false, 'the sender does not create a duplicate local stream');
  await event('chat:start', { source: 'spoken', stream_id: 'other', character_id: 'claude', channel: 'broadcast' });
  assert.equal(ChatController.isStreaming, false, 'only the speaking character owns its bubble and TTS');
  await event('chat:start', { source: 'spoken', stream_id: 'ours', character_id: 'nana', channel: 'broadcast' });
  assert.equal(ChatController.isStreaming, true);
  await event('chat:start', { stream_id: 'ours', character_id: 'nana', channel: 'broadcast' });
  assert.deepEqual(fixture.thinking, ['ours'], 'backend start is idempotent');
  await event('chat:done', { stream_id: 'ours', text: 'A voluntary answer' });
  assert.equal(ChatController.isStreaming, false);
  assert(fixture.bubbles.some(call => call.name === 'showBubble' && call.args[0] === 'A voluntary answer'));
  await event('chat:start', { source: 'spoken', stream_id: 'quiet', character_id: 'nana', channel: 'broadcast' });
  await event('chat:done', { stream_id: 'quiet', text: '' });
  assert.equal(ChatController.isStreaming, false, 'silence clears the thinking state');
  assert.deepEqual(fixture.completed, ['A voluntary answer', '']);
  console.log('Public speech streams: one dispatch, character routing, final-only bubbles and silence settlement passed.');
} finally {
  ChatController.cleanup();
  delete globalThis.__speech;
}
