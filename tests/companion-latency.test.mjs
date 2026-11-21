import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { transformSync } from 'esbuild';

const source = readFileSync('src/utils/companionLatency.ts', 'utf8').replace(/^import .*\n/, '');
const code = transformSync(source, { loader: 'ts', format: 'cjs', target: 'es2020' }).code;
let now = 10;
const requests = [];
const frames = [];
const context = { module: { exports: {} }, performance: { now: () => now },
  invoke: (command, args) => { requests.push({ command, ...args }); return Promise.resolve(); },
  document: { visibilityState: 'visible' }, innerHeight: 800,
  requestAnimationFrame: callback => frames.push(callback),
};
vm.runInNewContext(code, context);
const { beginCompanionTurn, companionParagraphId, companionParagraphCommitted } = context.module.exports;
beginCompanionTurn('a');
now = 110;
assert.equal(companionParagraphId('a', 'paragraph-a'), 'paragraph-a');
companionParagraphId('a', 'paragraph-a2');
companionParagraphId('other-window', 'paragraph-external');
assert.equal(requests.length, 1);
assert.equal(requests[0].stage, 'paragraph_ready');
assert.equal(requests[0].elapsedMs, 100);
const element = { isConnected: true, getBoundingClientRect: () => ({ width: 300, height: 60, top: 10, bottom: 70 }) };
companionParagraphCommitted('paragraph-a', element);
assert.equal(requests.length, 1, 'a state update is not a paint');
now = 126; frames.shift()();
assert.equal(requests.length, 1);
now = 142; frames.shift()();
assert.equal(requests[1].stage, 'paragraph_painted');
assert.equal(requests[1].elapsedMs, 132);
companionParagraphCommitted('paragraph-a', element);
assert.equal(frames.length, 0, 'rerenders never duplicate a sample');
beginCompanionTurn('hidden');
companionParagraphId('hidden', 'hidden-paragraph');
context.document.visibilityState = 'hidden';
companionParagraphCommitted('hidden-paragraph', element);
assert.equal(frames.length, 0, 'hidden windows cannot claim a visible first paragraph');

// Instrumentation must not change the product's complete-paragraph display contract.
const windowSource = readFileSync('src/components/ChatWindow.tsx', 'utf8');
assert.match(windowSource, /const parts = buf\.split\('\\n'\)/);
assert.match(windowSource, /无换行符：仅累积，不输出气泡/);
assert.equal(requests.some(request => 'text' in request || 'message' in request), false);
console.log('Companion latency: monotonic clock, actual commit, deduplication and paragraph display preserved.');
