import {readFile, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import sharp from 'sharp';

const json = async path => JSON.parse(await readFile(path, 'utf8'));
const save = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + '\n');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
/**
 * 丢掉重复帧后，把它占的时间并给保留下来的那一帧。
 *
 * 重复帧不是白占的：眨眼素材里那几拍是「眼睛闭着停住」，删掉画面但保留时长，
 * 整段才还是原来那个节奏。曾经只删帧不补时长，414ms 的眨眼被压成 281ms，
 * 短到看上去像没有眨眼。
 */
const mergeDurations = (durations, kept, total) => kept.map((index, order) => {
  const next = order + 1 < kept.length ? kept[order + 1] : total;
  let merged = 0;
  for (let i = index; i < next; i++) merged += durations[i] ?? 0;
  return merged;
});
const vocab = await json('src/chibi/animations.json');
const blink = vocab.animations.find(spec => spec.name === 'blink');
const anchors = await json('src/chibi/frameAnchors.json');
const scales = await json('src/chibi/frameScales.json');
let builtinChanged = false;
for (const id of ['vivian', 'nana', 'deepseek', 'claude', 'chatgpt']) {
  const builtin = ['vivian', 'nana'].includes(id);
  const manifestPath = `character-packs/${id}/character.json`;
  const manifest = builtin ? null : await json(manifestPath);
  const spec = manifest ? manifest.animations.blink : {...blink, ...blink.characterOverrides?.[id]};
  const url = spec.sheet.replaceAll('{character}', id);
  const path = (builtin ? 'public' : `character-packs/${id}/assets`) + url;
  const seen = new Set(), kept = [], cells = [];
  for (let i = 0; i < spec.frames; i++) {
    const cell = await sharp(path).extract({left: i % spec.cols * 512,
      top: Math.floor(i / spec.cols) * 512, width: 512, height: 512}).ensureAlpha().raw().toBuffer();
    // Invisible RGB values do not distinguish two rendered frames.
    for (let p = 0; p < cell.length; p += 4) if (!cell[p + 3]) cell.fill(0, p, p + 3);
    const key = hash(cell);
    if (seen.has(key)) continue;
    seen.add(key); kept.push(i); cells.push(cell);
  }
  console.log(`${id}: ${spec.frames} -> ${kept.length} unique blink frames`);
  if (kept.length === spec.frames) continue;
  const rows = Math.ceil(kept.length / spec.cols);
  const encoded = await sharp({create: {width: spec.cols * 512, height: rows * 512,
    channels: 4, background: '#00000000'}}).composite(cells.map((input, i) => ({input,
    raw: {width: 512, height: 512, channels: 4}, left: i % spec.cols * 512,
    top: Math.floor(i / spec.cols) * 512}))).webp({lossless: true, effort: 4}).toBuffer();
  await writeFile(path, encoded);
  const updated = {...spec, rows, frames: kept.length, durations: mergeDurations(spec.durations, kept, spec.frames)};
  if (builtin) {
    blink.characterOverrides ??= {};
    blink.characterOverrides[id] = {rows, frames: updated.frames, durations: updated.durations};
  } else manifest.animations.blink = updated;
  // Frame calibration must follow the same old-to-new index mapping as pixels.
  for (const [kind, data] of [['anchors', anchors], ['scales', scales]]) {
    const source = builtin ? data : manifest[kind];
    if (source?.[url]) source[url] = kept.map(i => source[url][i]);
  }
  if (builtin) builtinChanged = true;
  else {
    manifest.files['assets' + url] = hash(encoded);
    await save(manifestPath, manifest);
  }
}
await save('src/chibi/animations.json', vocab);
if (builtinChanged) {
  await save('src/chibi/frameAnchors.json', anchors);
  await save('src/chibi/frameScales.json', scales);
}
