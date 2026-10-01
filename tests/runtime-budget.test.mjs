import assert from 'node:assert/strict';
import { SheetLoader } from '../src/chibi/sheetLoader.ts';

const created = [];
globalThis.Image = class {
  onload = null;
  onerror = null;
  src = '';
  constructor() { created.push(this); }
};
const loader = new SheetLoader();
assert.equal(created.length, 0, 'idle must not preload animation sheets');
const first = loader.load('walk-left');
assert.equal(loader.load('walk-left'), first, 'share concurrent requests');
assert.equal(created.length, 1);
created.at(-1).onload();
assert.equal(await first, true);
for (const source of ['blink', 'sleep']) {
  const pending = loader.load(source);
  created.at(-1).onload();
  assert.equal(await pending, true);
}
assert.equal(loader.isReady('walk-left'), false, 'old decoded image must be released');
assert.equal(loader.isReady('blink'), true);
assert.equal(loader.isReady('sleep'), true);
const missing = loader.load('missing');
created.at(-1).onerror();
assert.equal(await missing, false);
const count = created.length;
assert.equal(await loader.load('missing'), false);
assert.equal(created.length, count, 'missing sheets must not repeatedly download');
const cancelled = loader.load('cast');
const cancelledImage = created.at(-1);
loader.clear();
assert.equal(await cancelled, false, 'cleanup must settle waiting sequences');
assert.equal(cancelledImage.onload, null);
assert.equal(loader.isReady('sleep'), false);

console.log('Sprite loading and cleanup: passed');
