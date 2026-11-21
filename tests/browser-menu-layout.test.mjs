import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
const { outputFiles } = buildSync({ entryPoints: ['src/components/mind-inspector/pages/browserMenuLayout.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { browserMenuLayout, browserSubmenuDelay } = await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].text).toString('base64')}`);
assert.equal(browserSubmenuDelay, 200);
for (const width of [320, 480, 800, 1280, 1920]) {
  for (const right of [280, width - 16]) {
    const layout = browserMenuLayout({ width, height: 760 }, { right, bottom: 92 }, true, 660);
    const { main, sub } = layout;
    assert.ok(main.width > 0 && sub.width > 0);
    assert.ok(main.left >= 8 && sub.left >= 8);
    assert.ok(main.left + main.width <= width - 8 && sub.left + sub.width <= width - 8);
    assert.ok(sub.left + sub.width <= main.left - 8 || main.left + main.width <= sub.left - 8, 'flyouts must never cover the primary card');
    assert.ok(sub.top + sub.maxHeight <= 752);
  }
}
const wide = browserMenuLayout({ width: 1280, height: 800 }, { right: 1250, bottom: 100 }, true, 350);
assert.equal(wide.side, 'left');
assert.equal(wide.sub.top, 350);
console.log('Browser menu: adjacent flyouts remain inside viewport at narrow and wide sizes; hover delay is 200ms.');
