import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundle = await build({ entryPoints: ['src/components/mind-inspector/pages/comicPanelReveal.ts'], bundle: true, write: false, format: 'esm' });
const { startInsetPanelReveal } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
let now = 0;
let nextId = 0;
const pending = new Map();
Object.defineProperty(globalThis, 'performance', { configurable: true, value: { now: () => now } });
globalThis.requestAnimationFrame = callback => { pending.set(++nextId, callback); return nextId; };
globalThis.cancelAnimationFrame = id => pending.delete(id);
globalThis.getComputedStyle = () => ({ getPropertyValue: () => '9px' });
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
const tick = time => { now = time; const callbacks = [...pending.values()]; pending.clear(); callbacks.forEach(callback => callback()); };

let mainRect = { left: 305, top: 0, width: 365, height: 600, bottom: 600 };
let insetRect = { left: 160, top: 390, right: 370, width: 210, height: 210 };
const mainPanel = { getBoundingClientRect: () => mainRect };
const insetPanel = { style: {}, getBoundingClientRect: () => insetRect };
const edge = { setAttribute: (_, value) => { edge.points = value.split(' ').map(point => point.split(',').map(Number)); } };
const main = { style: {}, parentElement: mainPanel, dataset: { points: '[[0,0],[100,0],[91.429,100],[0,100]]' }, querySelector: () => edge };
const inset = { parentElement: insetPanel };
const classes = new Set();
let noteRect = { left: 0, top: 0, width: 290, height: 600 };
const notePolygons = Array.from({ length: 3 }, () => ({ setAttribute(_, value) { this.points = value.split(' ').map(point => point.split(',').map(Number)); } }));
const notebook = {
  style: { setProperty(key, value) { this[key] = value; }, removeProperty(key) { delete this[key]; } },
  classList: { contains: () => false }, getBoundingClientRect: () => noteRect, querySelectorAll: () => notePolygons,
};
const stage = { closest: () => ({ querySelector: () => notebook }), querySelector: id => id === '#skirt-full' ? main : inset, classList: { add: cls => classes.add(cls), remove: cls => classes.delete(cls) } };
let desktop = true;
let settled = 0;
const reveal = startInsetPanelReveal(stage, () => desktop, true, () => {
  settled++;
  assert.equal(insetPanel.style.opacity, '1', 'hand playback starts after the final fade frame');
  assert.equal(insetPanel.style.translate, '0 0px', 'hand playback starts after the final movement frame');
});
assert.equal(insetPanel.style.visibility, 'hidden', 'the inset waits for its scene cue');
assert.equal(insetPanel.style.opacity, '0', 'the inset starts transparent');
assert.equal(edge.points[4][1], 100, 'the main panel is intact before the inset arrives');
tick(3150);
assert.equal(insetPanel.style.translate, '0 219px');
tick(3725);
const offset = Number(insetPanel.style.translate.split(' ')[1].slice(0, -2));
assert.equal(settled, 0, 'hand playback cannot start while the inset is moving');
assert(offset > 0 && offset < 219, 'the inset travels upward without scaling its artwork');
assert(Number(insetPanel.style.opacity) > 0 && Number(insetPanel.style.opacity) < 1, 'the inset fades in during its push');
assert(Math.abs(Number(insetPanel.style.opacity) + offset / 219 - 1) < 1e-6, 'movement and opacity share one eased clock');
assert(Math.abs(edge.points[4][1] / 100 * mainRect.height - (insetRect.top - 9 + offset)) < 1e-6,
  'the moving crop remains exactly at the inset paper gutter');
assert.equal(insetPanel.style.width, undefined, 'entrance never stretches the square hand atlas');
assert(Math.abs(notePolygons[0].points[3][0] / 100 * noteRect.width - (insetRect.left - 9)) < 1e-6,
  'the notebook notch follows the inset left gutter');
assert(Math.abs(notePolygons[0].points[3][1] / 100 * noteRect.height - (insetRect.top - 9 + offset)) < 1e-6,
  'the notebook is cut by the same moving top gutter as the main panel');
assert.deepEqual(notePolygons[0].points, notePolygons[1].points, 'notebook artwork and outline share the cut');
assert(notebook.style['--notebook-crop'].includes('polygon('), 'live content and hit target receive the same mask');

// Resizing mid-entrance must preserve time and remeasure the crop in pixels.
mainRect = { left: 610, top: 0, width: 730, height: 1200, bottom: 1200 };
insetRect = { left: 320, top: 780, right: 740, width: 420, height: 420 };
noteRect = { left: 0, top: 0, width: 580, height: 1200 };
reveal.refresh();
tick(4300);
assert.equal(settled, 1, 'hand playback begins once the inset settles');
reveal.refresh();
assert.equal(settled, 1, 'resizing a settled inset does not replay the hand animation');
assert.equal(insetPanel.style.translate, '0 0px');
assert.equal(insetPanel.style.opacity, '1', 'the settled inset is fully visible');
assert(Math.abs(edge.points[3][0] / 100 * 730 + 610 - 749) < 1e-6);
assert(Math.abs(edge.points[4][1] / 100 * 1200 - 771) < 1e-6);
assert.equal(pending.size, 0, 'the animation stops scheduling frames after settlement');
desktop = false;
reveal.refresh();
assert.equal(classes.has('inset-crop'), false, 'mobile keeps its non-overlapping layout');
assert.equal(insetPanel.style.translate, '');
assert.equal(insetPanel.style.opacity, '', 'mobile clears the desktop fade');
assert.equal(notebook.style['--notebook-crop'], undefined, 'mobile clears the desktop notebook notch');
reveal.stop();
desktop = true;
const still = startInsetPanelReveal(stage, () => desktop, false);
assert.equal(insetPanel.style.translate, '0 0px', 'reduced motion and revisits show the final crop immediately');
assert.equal(pending.size, 0);
still.stop();
now = 0;
const cancelled = startInsetPanelReveal(stage, () => desktop, true);
cancelled.stop();
assert.equal(pending.size, 0, 'turning away cancels the pending animation frame');
assert.equal(insetPanel.style.visibility, '');
console.log('Inset crop: synchronized gutter, stable artwork, resizing, mobile, reduced motion and cancellation passed.');
