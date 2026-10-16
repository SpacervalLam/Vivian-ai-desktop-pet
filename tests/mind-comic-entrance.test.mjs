import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { transform } from 'esbuild';

// Exercise the production layout-effect lifecycle, including StrictMode replay.
const source = readFileSync('src/components/mind-inspector/pages/ComicOverview.tsx', 'utf8').replace(/\r\n/g, '\n');
const start = source.indexOf('  useLayoutEffect(() => {\n    const node = stage.current;');
const end = source.indexOf('  }, [html, page, revision, active]);', start);
assert(start > 0 && end > start);
const script = await transform(source.slice(start, end) + '  }, []);', { loader: 'ts' });
let cleanup, inkStarts = 0, inkStops = 0;
const classes = new Set();
const playback = [];
const node = { dataset: {}, style: {}, parentElement: { clientWidth: 1400, clientHeight: 800 },
  querySelectorAll: () => [], querySelector: () => null,
  classList: { remove: name => { classes.delete(name); playback.push(`remove:${name}`); },
    add: name => { classes.add(name); playback.push(`add:${name}`); },
    toggle: (name, enabled) => { if (enabled) classes.add(name); else classes.delete(name); } },
  get offsetWidth() { playback.push('layout'); return 1400; } };
const context = {
  stage: { current: node }, page: 9, revision: 0, active: true,
  animatedRevision: { current: '' }, handReady: { current: false }, mobileRef: { current: false },
  hoverFloats: { current: new Map() }, performance: { now: () => 100 },
  window: { matchMedia: () => ({ matches: false }), setTimeout, clearTimeout },
  comicScriptDuration: () => 5800, syncHand() {}, fitComicText() {},
  startMorningInkReveal: () => { inkStarts++; return { stop: () => inkStops++ }; },
  ResizeObserver: class { observe() {} disconnect() {} },
  useLayoutEffect: callback => { cleanup = callback(); },
};
const setup = () => vm.runInNewContext(script.code, context);
setup();
assert(classes.has('entering'));
assert.equal(inkStarts, 1);
cleanup(); setup();
assert(classes.has('entering'), 'StrictMode replay must preserve the entering state');
assert.equal(inkStarts, 2, 'scripted scenes restart after effect cleanup');
assert.deepEqual(playback, ['remove:entering','layout','add:entering','remove:entering','layout','add:entering']);
cleanup();
context.active = false; setup();
assert(!classes.has('entering'), 'inactive book does not run an entrance');
cleanup();
context.active = true; setup();
assert(classes.has('entering'), 'returning to the book replays the current scene');
assert.equal(inkStarts, 3);
cleanup();
assert.equal(inkStops, 3, 'leaving cleans up the previous scripted performance');
console.log('Comic entrance: StrictMode replay, inactive cleanup and returning to the book passed.');
