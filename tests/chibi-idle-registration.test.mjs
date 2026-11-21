import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import sharp from 'sharp';
import { bounds, faceBounds, headCenter } from '../scripts/chibi/frame-sheets/registration.mjs';

const cell = 512;
const { atlas } = JSON.parse(await readFile('src/chibi/animations.json', 'utf8'));
const neutral = ['float', 'turn', 'cast', 'happy', 'angry', 'thinking', 'think', 'smug', 'sleep', 'busy-in', 'remind', 'tired', 'music', 'look'];

function readFrame(root, url, cols, index) {
  return sharp(`${root}/assets${url}`)
    .extract({ left: index % cols * cell, top: Math.floor(index / cols) * cell, width: cell, height: cell })
    .ensureAlpha().raw().toBuffer();
}

function displayedChannel(pixels, offset, channel) {
  const alpha = pixels[offset + 3];
  return channel === 3 ? alpha : Math.round(pixels[offset + channel] * alpha / 255);
}

function sameDisplayedPixels(idle, frame, label) {
  for (let i = 0; i < idle.length; i += 4) {
    if (idle[i + 3] !== frame[i + 3]) assert.fail(`${label}: silhouette changed at pixel ${i / 4}`);
    for (let c = 0; c < 3; c++) {
      const delta = displayedChannel(idle, i, c) - displayedChannel(frame, i, c);
      if (Math.abs(delta) > 1) assert.fail(`${label}: visible drawing changed at pixel ${i / 4}`);
    }
  }
}

function nearIdleDrawing(idle, frame, label) {
  let visible = 0;
  let changed = 0;
  let squares = 0;
  for (let i = 0; i < idle.length; i += 4) {
    if (!idle[i + 3] && !frame[i + 3]) continue;
    visible++;
    let largestDelta = 0;
    for (let c = 0; c < 4; c++) {
      const delta = displayedChannel(idle, i, c) - displayedChannel(frame, i, c);
      squares += delta ** 2;
      largestDelta = Math.max(largestDelta, Math.abs(delta));
    }
    if (largestDelta > 20) changed++;
  }
  // A final eyelid in-between or a calibrated antialiased turn may differ locally.
  assert(changed / visible < .02, `${label}: substantial changes stay below 2% of the visible sprite`);
  assert(Math.sqrt(squares / (visible * 4)) < 6.5, `${label}: overall drawing stays close to idle`);
}

function displayedGeometry(pixels, [dx, dy], [scale, origin]) {
  const box = bounds(pixels, cell, cell);
  const face = faceBounds(pixels, cell, cell, box);
  const head = headCenter(pixels, cell, cell, face, box);
  const toPixels = cell / 100;
  const pivot = origin * toPixels;
  const y = value => (value - pivot) * scale + pivot + dy * toPixels;
  return { top: y(box.y0), foot: y(box.y1), head: (head - cell / 2) * scale + cell / 2 + dx * toPixels };
}

function assertStablePose(rest, pose, label) {
  assert(Math.abs(pose.head - rest.head) <= 2, `${label}: stable horizontal registration`);
  assert(Math.abs(pose.foot - rest.foot) <= 1, `${label}: stable foot baseline`);
  assert(Math.abs(pose.top - rest.top) <= 5, `${label}: stable sprite height`);
}

function assertNoDetachedMatte(pixels, label) {
  const count = cell * cell;
  const supported = new Uint8Array(count);
  const queue = new Uint32Array(count);
  let end = 0;
  for (let i = 0; i < count; i++) {
    if (pixels[i * 4 + 3] < 200) continue;
    supported[i] = 1;
    queue[end++] = i;
  }
  // Thin parts of the drawing may reach past the opaque body, but still connect to it.
  for (let start = 0; start < end; start++) {
    const x = queue[start] % cell;
    const y = Math.floor(queue[start] / cell);
    for (let yy = Math.max(0, y - 1); yy <= Math.min(cell - 1, y + 1); yy++) {
      for (let xx = Math.max(0, x - 1); xx <= Math.min(cell - 1, x + 1); xx++) {
        const i = yy * cell + xx;
        if (supported[i] || !pixels[i * 4 + 3]) continue;
        supported[i] = 1;
        queue[end++] = i;
      }
    }
  }
  for (let i = 0; i < count; i++) {
    if (!supported[i] && pixels[i * 4 + 3] > 8) assert.fail(`${label}: detached translucent matte at pixel ${i}`);
  }
}

for (const id of ['deepseek', 'claude', 'chatgpt']) {
  const root = `character-packs/${id}`;
  const pack = JSON.parse(await readFile(`${root}/character.json`, 'utf8'));
  const idleUrl = `/chibi/motion/${id}/${id}-idle-sheet.webp`;
  const atlasUrl = `/chibi/${id}-atlas.webp`;
  const idle = await readFrame(root, idleUrl, 1, 0);
  const rest = displayedGeometry(idle, [0, 0], [1, 90]);
  sameDisplayedPixels(idle, await readFrame(root, atlasUrl, atlas.cols, 0), `${id}: atlas fallback and pinned idle agree`);
  assert.deepEqual(pack.anchors[atlasUrl][0], [0, 0], `${id}: fallback has the pinned idle position`);
  assert.equal(pack.scales[atlasUrl][0][0], 1, `${id}: fallback has the pinned idle scale`);

  for (const name of neutral) {
    const spec = pack.animations[name];
    if (!spec) continue;
    for (const direction of spec.directions ?? ['left']) {
      const url = spec.sheet.replaceAll('{character}', id).replaceAll('{direction}', direction);
      const pixels = await readFrame(root, url, spec.cols, 0);
      const label = `${id}/${name}/${direction}: neutral entry`;
      const [dx, dy] = pack.anchors[url][0];
      const scale = pack.scales[url][0];
      if (name === 'turn' && (dx || dy || scale[0] !== 1)) {
        nearIdleDrawing(idle, pixels, label);
        assertStablePose(rest, displayedGeometry(pixels, [dx, dy], scale), label);
      } else {
        sameDisplayedPixels(idle, pixels, label);
        assert.deepEqual([dx, dy, scale[0]], [0, 0, 1], `${label}: pinned idle transform`);
      }
      if (name === 'turn') {
        assertNoDetachedMatte(pixels, `${id}/${name}/${direction}/0`);
        for (let i = 1; i < spec.frames; i++) {
          assertNoDetachedMatte(await readFrame(root, url, spec.cols, i), `${id}/${name}/${direction}/${i}`);
        }
      }
    }
  }

  const blink = pack.animations.blink;
  const url = blink.sheet.replaceAll('{character}', id);
  for (let i = 0; i < blink.frames; i++) {
    const pixels = await readFrame(root, url, blink.cols, i);
    const label = `${id}/blink/${i}`;
    const pose = displayedGeometry(pixels, pack.anchors[url][i], pack.scales[url][i]);
    assertStablePose(rest, pose, label);
    if (i === 0) sameDisplayedPixels(idle, pixels, `${label}: starts at pinned idle`);
    if (i === blink.frames - 1) nearIdleDrawing(idle, pixels, `${label}: returns to idle`);
  }
}
console.log('Packaged idle transitions: matching fallback and entry frames, stable blink registration and clean turn backgrounds verified.');
