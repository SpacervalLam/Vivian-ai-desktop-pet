import assert from 'node:assert/strict';
import { build } from 'esbuild';
const listeners = new Map(), events = [], bubbles = [], responses = [];
globalThis.__imageChatFixture = { listeners, events, bubbles };
const originalWindow = globalThis.window;
globalThis.window = { setTimeout: () => 1, clearTimeout: () => {} };
const bundle = await build({ entryPoints: ['src/controllers/ChatController.ts'], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{ name: 'image-stream-fixture', setup(b) {
  b.onResolve({filter:/^@tauri-apps\/api\/|\/useAppStore$|\.\/BubbleController$|\.\/TtsStreamQueue$|\.\/StreamController$|\/characterContext$|\/i18n$/}, args => ({path:args.path, namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'}, ({path}) => ({contents:
    path.includes('/api/event') ? `export const listen=async(name,callback)=>{globalThis.__imageChatFixture.listeners.set(name,callback);return()=>{};};export const emit=async(name,args)=>{globalThis.__imageChatFixture.events.push({name,args});};` :
    path.includes('/api/core') ? `export const invoke=async()=>({emotion:'neutral',intensity:0});` :
    path.endsWith('useAppStore') ? `export const useAppStore={getState:()=>({setLastUserEmotion:()=>{}})};` :
    path.endsWith('characterContext') ? `export const getCharacterId=()=> 'nana';` :
    path.endsWith('i18n') ? `export default {t:key=>key};` :
    path.endsWith('StreamController') ? `export class StreamController {feed(){} reset(){}}` :
    path.endsWith('BubbleController') ? `export const BubbleController=new Proxy({}, {get:(_,key)=>(...args)=>globalThis.__imageChatFixture.bubbles.push({key,args})});` :
    `export const TtsStreamQueue=new Proxy({}, {get:(_,key)=>key==='isEnabled'?()=>false:()=>{}});`
  }));
}}] });
const { ChatController } = await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'));
ChatController.setHandlers({onResponseReceived:(response,id)=>responses.push({response,id})});
await ChatController.init();
const dispatch=(name,payload)=>listeners.get(name)({payload});
dispatch('chat:start',{source:'image',stream_id:'other',character_id:'vivian',channel:'wechat'});
assert.equal(ChatController.isStreaming,false,'other characters cannot adopt an image stream');
dispatch('chat:start',{source:'text',stream_id:'text',character_id:'nana',channel:'wechat'});
assert.equal(ChatController.isStreaming,false,'ordinary backend starts cannot create duplicate sessions');
dispatch('chat:start',{source:'image',stream_id:'picture',character_id:'nana',channel:'wechat'});
assert.equal(ChatController.isStreaming,true);
dispatch('chat:meta',{stream_id:'picture',character_id:'nana',expression:'happy',motion:'listen'});
dispatch('chat:chunk',{stream_id:'picture',text:'桌宠主对话回复'});
dispatch('chat:done',{stream_id:'picture',text:'桌宠主对话回复',motion:'listen',expression:'happy',emotion_score:1});
assert.equal(ChatController.isStreaming,false,'image stream is settled by the normal terminal event');
assert.equal(responses.length,1);
assert.equal(responses[0].response.text,'桌宠主对话回复');
assert.ok(bubbles.some(call=>call.key==='showStreamingBubble'&&call.args[0]==='桌宠主对话回复'));
assert.equal(events.filter(e=>e.name==='chat:assistant_message').length,1,'only the main reply is delivered once');
dispatch('chat:start',{source:'image',stream_id:'cancel',character_id:'nana'});
dispatch('chat:cancelled',{stream_id:'cancel'});
assert.equal(ChatController.isStreaming,false,'cancelling vision clears the adopted image turn');
ChatController.cleanup();
globalThis.window=originalWindow;
delete globalThis.__imageChatFixture;
console.log('Image chat streams: role isolation, primary reply, expression/bubble lifecycle and cancellation passed');
