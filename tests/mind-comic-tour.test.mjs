import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { transform } from 'esbuild';

const helper = await transform(await readFile('src/components/mind-inspector/pages/comicTour.ts', 'utf8'), { loader: 'ts', format: 'esm' });
const { hasRunningComicAnimation, comicScriptDuration, nextTourPage } = await import(`data:text/javascript;base64,${Buffer.from(helper.code).toString('base64')}`);
const animation = (state, duration, pending = false) => ({ playState: state, pending, effect: { getComputedTiming: () => ({ endTime: duration }) } });
assert.equal(hasRunningComicAnimation([animation('running', 17000)]), true);
assert.equal(hasRunningComicAnimation([animation('finished', 17000)]), false);
assert.equal(hasRunningComicAnimation([animation('running', Infinity)]), false, 'perpetual ambient motion cannot block a tour');
assert.equal(hasRunningComicAnimation([animation('idle', 2000, true)]), true, 'pending entrances must finish');
assert.equal(comicScriptDuration(9, false), 5110);
assert.equal(comicScriptDuration(7, false), 11740, 'page 09 waits for the inset to settle before grip/hold/release');
assert.equal(comicScriptDuration(9, true), 0);
assert.equal(nextTourPage(8, 9), 0);

// Execute the real activity and scheduling effects with a controllable clock.
const source = (await readFile('src/components/mind-inspector/pages/UnifiedComicBook.tsx', 'utf8')).replace(/\r\n/g, '\n');
const start = source.indexOf('  useEffect(() => {\n    if (!presentationReady) return;\n    lastInteraction.current');
const end = source.indexOf('  const expand =', start);
assert.ok(start > 0 && end > start);
const effects = await transform(source.slice(start, end), { loader: 'ts' });
let now = 0, tick, busy = false, animations = [];
const listeners = new Map(), cleanups = [];
const stage = { dataset: {}, getAnimations: () => animations };
const node = { querySelector: selector => selector === '.mind-comic-book .comic' ? stage : busy ? {} : null, contains: () => true, getAnimations: () => animations };
const context = {
  presentationReady: true, touring: false, page: 0, expanded: null,
  root: { current: node }, pageRef: { current: 0 }, expandedRef: { current: null }, drag: { current: null },
  lastInteraction: { current: 0 }, tourStartedAt: { current: 0 }, animationFinishedAt: { current: null },
  bookSpreads: Array(10), hasRunningComicAnimation, nextTourPage,
  Date: { now: () => now }, performance: { now: () => now },
  window: {
    addEventListener: (name, callback) => listeners.set(name, callback), removeEventListener: name => listeners.delete(name),
    setInterval: callback => { tick = callback; return 1; }, getSelection: () => null,
  },
  document: { hidden: false, hasFocus: () => true, addEventListener() {}, removeEventListener() {} },
  useEffect: callback => { const cleanup = callback(); if (cleanup) cleanups.push(cleanup); },
  clearInterval() {},
  setTouring: value => { context.touring = value; }, setContentsOpen: value => { context.contentsOpen = value; },
  setPage: value => { context.page = value; },
};
vm.runInNewContext(effects.code, context);
now = 9999; tick(); assert.equal(context.touring, false);
now = 10000; tick(); assert.equal(context.touring, true, 'ten idle seconds enter tour');
now = 10999; tick(); assert.equal(context.page, 0);
now = 11000; tick(); assert.equal(context.page, 1, 'one second after tour entry when the page already finished');
animations = [animation('running', 17000)];
now = 12000; tick(); assert.equal(context.page, 1, 'unfinished page animation holds the page');
animations = [];
now = 20000; tick(); assert.equal(context.page, 1);
now = 20999; tick(); assert.equal(context.page, 1);
now = 21000; tick(); assert.equal(context.page, 2, 'advance exactly one second after animation completion');
stage.dataset.scriptAnimationEnd = '25000';
now = 23000; tick(); assert.equal(context.page, 2, 'RAF effects also hold the page');
now = 25000; tick();
now = 26000; tick(); assert.equal(context.page, 3);
for (const event of ['pointermove', 'pointerdown', 'wheel', 'keydown', 'input', 'touchstart']) {
  context.touring = true; listeners.get(event)();
  assert.equal(context.touring, false, `${event} exits the tour`);
  assert.equal(context.lastInteraction.current, now);
}
busy = true; now += 11000; tick(); assert.equal(context.touring, false, 'editing prevents tour entry');
busy = false; context.expandedRef.current = 'memory'; now += 11000; tick(); assert.equal(context.touring, false, 'expanded reading stays open');
context.expandedRef.current = null; context.document.hidden = true; now += 11000; tick(); assert.equal(context.touring, false, 'hidden windows do not tour');
cleanups.forEach(cleanup => cleanup()); assert.equal(listeners.size, 0);
assert.doesNotMatch(source, /checked=\{auto\}|setAuto/);
const css = await readFile('src/components/mind-inspector/pages/ComicOverview.css', 'utf8');
assert.match(css, /comic-phone-wake.*calc\(var\(--delay\) \+ 1150ms \+ 300ms\)/);
assert.match(source, /data-touring=\{touring\}/, 'the book exposes its live tour state');
assert.match(css, /\.unified-comic-book\[data-touring="true"\][^{]+#line-phone \.phone-screen-on\{animation:comic-phone-wake/, 'automatic phone wake requires tour mode');
assert.match(css, /#line-phone:hover \.phone-screen-on\{opacity:1\}/, 'manual browsing still lights the phone on hover');
console.log('Comic tour: idle entry, animation completion + one second, script effects, activity exit, editing/visibility guards, looping and phone wake passed');
