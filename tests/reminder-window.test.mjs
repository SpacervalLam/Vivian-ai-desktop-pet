import assert from 'node:assert/strict';
import { build } from 'esbuild';
let creations = 0, released = 0, shown = 0, restored = 0, promoted = 0, raisedEvents = 0;
let existing = null;
class Window {
  static async getByLabel() { return existing; }
  constructor(label, options) { this.label = label; this.options = options; creations++; assert.equal(options.focus, false); assert.equal(options.alwaysOnTop, false); }
  async once(name, callback) { if (name === 'tauri://created') queueMicrotask(() => callback({})); return () => { released++; }; }
  async setAlwaysOnTop(value) { if (value) promoted++; }
  async unminimize() { restored++; }
  async show() { existing = this; shown++; }
  async emit(name) { assert.equal(name, 'reminder:surface-shown'); raisedEvents++; }
}
globalThis.__ReminderWindowTest = Window;
const bundle = await build({ entryPoints: ['src/utils/reminderWindow.ts'], bundle:true, write:false, platform:'node', format:'esm', plugins:[{name:'mock-native',setup(b){
  b.onResolve({filter:/^@tauri-apps\/api\//}, a => ({path:a.path, namespace:'mock'}));
  b.onLoad({filter:/.*/,namespace:'mock'}, a=> ({contents:a.path.endsWith('/webviewWindow')
    ? 'export const WebviewWindow = globalThis.__ReminderWindowTest;'
    : 'export const currentMonitor = async () => ({position:{x:-1920,y:0},size:{width:1920,height:1080}}); export const getCurrentWindow = () => ({scaleFactor:async()=>1});'}));
}}]});
const {openReminderWindow} = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
await Promise.all([openReminderWindow(),openReminderWindow(true)]);
assert.equal(creations, 1, 'concurrent callers share a window');
assert.equal(released, 2, 'both creation event listeners released');
assert.equal(promoted, 1, 'important caller promotes shared window');
assert.equal(existing.options.x, -404, 'negative monitor coordinates preserved');
await openReminderWindow();
assert.equal(creations, 1); assert.equal(shown,3); assert.equal(restored,3); assert.equal(raisedEvents,3);
assert.equal(promoted,1,'normal reminder never overrides pending important cards');
delete globalThis.__ReminderWindowTest;
console.log('Reminder window: shared creation, listener disposal, restore and important promotion passed');
