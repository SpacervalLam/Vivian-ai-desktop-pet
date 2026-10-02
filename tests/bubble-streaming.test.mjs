import assert from 'node:assert/strict';
import { build } from 'esbuild';

const result = await build({
  stdin: {
    contents: `export { BubbleControllerClass } from './src/controllers/BubbleController';
      export { useAppStore } from './src/stores/useAppStore';
      export * from './src/utils/bubbleText';
      export * from './src/utils/bubbleLayout';`,
    resolveDir: process.cwd(),
  },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { BubbleControllerClass, useAppStore, computeDuration, bubbleCharacters, nextBubbleBoundary, placeBubble } =
  await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

assert.equal(nextBubbleBoundary(bubbleCharacters('短句。'), false), null);
assert.equal(nextBubbleBoundary(bubbleCharacters('一'.repeat(32) + '。”下一句'), false), 34);
assert.equal(nextBubbleBoundary(bubbleCharacters('第一段\n第二段'), false), 4);
assert.equal(nextBubbleBoundary(bubbleCharacters('🙂'.repeat(101)), false), 100);
assert.equal(computeDuration('好'), 3000);
assert.ok(computeDuration('中'.repeat(60)) > computeDuration('中'.repeat(20)));
assert.ok(computeDuration('read '.repeat(30)) > computeDuration('read '.repeat(5)));
assert.equal(computeDuration('中'.repeat(1000)), 18000);

const monitor = { x: 0, y: 0, width: 1920, height: 1080 };
const pet = { x: 600, y: 500, width: 200, height: 240 };
assert.deepEqual(placeBubble(pet, monitor, 340, 80), { x: 460, y: 420, position: 'top' });
assert.equal(placeBubble(pet, monitor, 340, 220).y, 280, 'movement uses actual stacked height');
assert.deepEqual(placeBubble({ ...pet, x: 700, y: 550 }, monitor, 340, 220), { x: 560, y: 330, position: 'top' });
assert.equal(placeBubble({ ...pet, y: 20 }, monitor, 340, 80).position, 'bottom');
assert.deepEqual(placeBubble({ ...pet, x: -1000, y: -600 },
  { x: -1920, y: -1080, width: 1920, height: 1080 }, 340, 80), { x: -1140, y: -680, position: 'top' });
assert.equal(placeBubble({ ...pet, x: 1900 }, monitor, 340, 80).x, 1576);

let now = 0;
let nextId = 0;
const frames = new Map();
const timers = new Map();
globalThis.requestAnimationFrame = (callback) => { const id = ++nextId; frames.set(id, callback); return id; };
globalThis.cancelAnimationFrame = (id) => frames.delete(id);
globalThis.setTimeout = (callback, ms) => { const id = ++nextId; timers.set(id, { callback, at: now + ms }); return id; };
globalThis.clearTimeout = (id) => timers.delete(id);
function advance(ms) {
  const until = now + ms;
  while (now < until) {
    now = Math.min(now + 16, until);
    const pending = [...frames.values()];
    frames.clear();
    for (const callback of pending) callback(now);
    for (const [id, timer] of [...timers]) {
      if (timer.at <= now) { timers.delete(id); timer.callback(); }
    }
  }
}

const controller = new BubbleControllerClass();
const completed = new Map();
let lastText = '';
const unsubscribe = useAppStore.subscribe((state) => {
  for (const bubble of state.settledBubbles) completed.set(bubble.id, bubble.text);
  if (state.currentBubble) lastText = state.currentBubble;
  if (state.currentBubble) {
    const chars = [...state.currentBubble];
    assert.ok(chars.length <= 100);
    assert.ok(!/[\uD800-\uDBFF]$/.test(state.currentBubble), 'no partial surrogate pairs');
  }
});

// A burst with multiple paragraphs must still reveal gradually and preserve every segment.
const text = '一'.repeat(36) + '。\n' + '🙂'.repeat(105) + '\n最后一句。';
controller.showStreamingBubble(text, { crossCharacter: true, listenerName: 'Nana' });
advance(320);
const beforeDone = controller.currentBubble;
assert.ok(beforeDone.length > 0 && beforeDone.length < text.length);
controller.finishStreaming(text);
assert.equal(controller.currentBubble, beforeDone, 'done must not flush the reveal queue');
advance(60000);
assert.equal([...completed.values(), lastText].join('').replace(/\s/g, ''), text.replace(/\s/g, ''));
assert.equal(controller.currentBubble, null);
assert.equal(useAppStore.getState().settledBubbles.length, 0);
assert.equal(frames.size, 0);
assert.equal(timers.size, 0);

// The final bubble gets its full reading dwell after all characters have appeared.
controller.showStreamingBubble('你好🙂');
advance(500);
assert.equal(controller.currentBubble, '你好🙂');
controller.finishStreaming('你好🙂');
advance(2900);
assert.equal(controller.currentBubble, '你好🙂');
advance(300);
assert.equal(controller.currentBubble, null);

// Replacements reset metadata, cancel the old reveal queue, and never inherit its offset.
controller.showStreamingBubble('旧消息'.repeat(30), { crossCharacter: true, listenerName: 'Nana' });
advance(100);
controller.showBubble('新消息', 1000);
advance(500);
assert.equal(controller.currentBubble, '新消息');
assert.equal(useAppStore.getState().bubbleCrossCharacter, false);
advance(1200);
assert.equal(controller.currentBubble, null);

// Incomplete action annotations stay hidden, including while the network pauses.
controller.showStreamingBubble('你好（挥');
advance(500);
assert.equal(controller.currentBubble, '你好');
controller.showStreamingBubble('你好（挥手）世界');
controller.finishStreaming('你好（挥手）世界');
advance(500);
assert.equal(controller.currentBubble, '你好世界');
controller.closeAll();
assert.equal(frames.size, 0);
assert.equal(timers.size, 0);

// A network pause at a completed sentence must not leave an idle animation loop.
const sentence = '中'.repeat(32) + '。';
controller.showStreamingBubble(sentence + '下');
advance(9000);
assert.equal(controller.currentBubble, '下');
assert.equal(frames.size, 0);
controller.showStreamingBubble(sentence + '下一句。');
controller.finishStreaming(sentence + '下一句。');
advance(500);
assert.equal(controller.currentBubble, '下一句。');
controller.closeAll();
// Standalone stickers bypass the typewriter and expire independently.
const sticker = {id:'vivian_happy_01',character_id:'vivian',version:'2',label:'开心',meaning:'喜悦'};
controller.showSticker(sticker);
assert.equal(controller.currentBubble, null);
assert.equal(frames.size, 0);
assert.equal(useAppStore.getState().settledBubbles[0].sticker, sticker);
advance(3900);
assert.equal(controller.hasActiveBubble, true);
advance(200);
assert.equal(controller.hasActiveBubble, false);

// A text attachment appears after reveal, and survives the text's shorter dwell.
controller.showBubble('你好', undefined, {sticker});
assert.equal(useAppStore.getState().settledBubbles.length, 0);
advance(500);
assert.equal(controller.currentBubble, '你好');
assert.equal(useAppStore.getState().settledBubbles[0].sticker, sticker);
advance(3000);
assert.equal(controller.currentBubble, null);
assert.equal(useAppStore.getState().settledBubbles.length, 1);
advance(1000);
assert.equal(controller.hasActiveBubble, false);

// Streaming completion carries an attachment; replacement cancels pending artwork.
controller.showStreamingBubble('🙂'.repeat(40));
controller.finishStreaming('🙂'.repeat(40), {sticker});
advance(16);
assert.equal(useAppStore.getState().settledBubbles.length, 0);
controller.showBubble('换一句');
advance(1000);
assert.equal(useAppStore.getState().settledBubbles.length, 0);
controller.closeAll();
controller.showStreamingBubble('好呀');
controller.finishStreaming('好呀', {sticker});
advance(500);
assert.equal(useAppStore.getState().settledBubbles[0].sticker, sticker);
controller.closeAll();
assert.equal(frames.size, 0);
assert.equal(timers.size, 0);
unsubscribe();
console.log('Bubble segmentation, reveal queue, adaptive dwell, movement geometry and cleanup: passed');
