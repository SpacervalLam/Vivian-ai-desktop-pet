import assert from 'node:assert/strict';
import { build } from 'esbuild';
const built = await build({
  stdin: { contents: `export { ReplyWaiting } from './src/chibi/replyWaiting';
    export { getMotion } from './src/chibi/motionRegistry';`, resolveDir: process.cwd() },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { ReplyWaiting, getMotion } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);
const state = new ReplyWaiting();
assert.equal(state.thinking, false);
state.start('a');
assert.equal(state.thinking, true);
assert.equal(state.text('a', ' \n'), false);
assert.equal(state.thinking, true);
state.text('a', '你');
assert.equal(state.thinking, false);
state.start('a'); // delayed backend start must not restart thinking after first text
assert.equal(state.thinking, false);
state.start('b');
assert.equal(state.thinking, false); // speaking has priority over queued replies
state.finish('a');
assert.equal(state.thinking, true);
state.finish('unrelated');
assert.equal(state.thinking, true);
state.finish('b'); // done, failure, empty response and cancel all end waiting
assert.equal(state.thinking, false);
for (const id of ['failed', 'cancelled', 'empty', 'config-error']) {
  state.start(id); assert.equal(state.thinking, true);
  state.finish(id); assert.equal(state.thinking, false);
}
const thinking = getMotion('thinking');
const think = getMotion('think');
assert.equal(thinking.kind, 'animation');
assert.equal(thinking.loop, true);
assert.equal(thinking.promptable, false);
assert.equal(thinking.sheet, think.sheet);
assert.equal(think.loop, false); // emotion remains a one-shot animation
console.log('Reply waiting lifecycle and motion vocabulary checks passed');
