import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { bounds } from '../../scripts/chibi/frame-sheets/registration.mjs';
export function characterAssetPath(input) {
  const path=String(input).replaceAll('\\','/');
  if(existsSync(path))return path;
  const prompt=/^src-tauri\/prompts\/characters\/([^/]+)\/(.*)$/.exec(path);
  if(prompt)return resolve('character-packs',prompt[1],'prompts',prompt[2]);
  if(path.startsWith('public/')) {
    const relative=path.slice(7),name=relative.split('/').at(-1);
    for(const id of ['deepseek','claude','chatgpt'])if(name.startsWith(id+'-')||name.startsWith(id+'_')||name===id+'.webp')return resolve('character-packs',id,'assets',relative);
  }
  return path;
}

export function clearTransparentRgb(pixels) {
  for (let i = 0; i < pixels.length; i += 4) {
    if (pixels[i + 3] === 0) pixels.fill(0, i, i + 3);
  }
  return pixels;
}

export function assertSpriteSeam(first, second, label) {
  assert.equal(first.length, second.length, label);
  for (let i = 0; i < first.length; i += 4) {
    const alpha = first[i + 3];
    if (alpha !== second[i + 3]) assert.fail(`${label}: silhouette changes at pixel ${i / 4}`);
    if (alpha === 0) continue;
    for (let channel = 0; channel < 3; channel++) {
      // A two-code-value RGB difference is allowed for independently encoded sprites.
      if (Math.abs(first[i + channel] - second[i + channel]) * alpha > 2 * 255) {
        assert.fail(`${label}: visible drawing changes at pixel ${i / 4}`);
      }
    }
  }
}

export function assertBottomDockFrame(frame, idle, previousHeight, label) {
  const cell = 512;
  const stride = cell * 4;
  const box = bounds(frame, cell, cell);
  const offset = box.y0 - bounds(idle, cell, cell).y0;
  assert.ok(offset >= 0 && offset <= 8, `${label}: head settles within eight pixels of idle`);
  assertSpriteSeam(frame.subarray(offset * stride, (offset + 200) * stride),
    idle.subarray(0, 200 * stride), `${label}: original head drawing`);
  assert.ok(box.height <= previousHeight + 2, `${label}: hidden body stays behind the edge`);
  for (let y = 120; y < box.y1; y++) {
    const center = frame.subarray(y * stride + 140 * 4, y * stride + 370 * 4);
    assert.ok(center.some((value, i) => i % 4 === 3 && value > 128),
      `${label}: separated drawing at row ${y}`);
  }
  return box.height;
}
