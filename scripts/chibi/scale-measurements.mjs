import sharp from 'sharp';
import { bounds, faceBounds } from './frame-sheets/registration.mjs';

export const CELL_SIZE = 512;
export const ORIGIN_PERCENT = 90;
export const PERCENT_PX = CELL_SIZE / 100;
export const SPREAD_LIMIT = 0.08;
export const COVERAGE = 0.6;
export const DEAD_ZONE = 0.03;

export function defaultIdleSheet(character) {
  if (character === 'nana') {
    return { url: `/chibi/${character}-atlas.webp`, columns: 3 };
  }
  return { url: `/chibi/motion/${character}/${character}-idle-sheet.webp`, columns: 1 };
}

export function readSpriteFrame(file, columns, frameIndex) {
  return sharp(file)
    .extract({
      left: (frameIndex % columns) * CELL_SIZE,
      top: Math.floor(frameIndex / columns) * CELL_SIZE,
      width: CELL_SIZE,
      height: CELL_SIZE,
    })
    .ensureAlpha()
    .raw()
    .toBuffer();
}

export async function measureSpriteFrame(file, columns, frameIndex) {
  const pixels = await readSpriteFrame(file, columns, frameIndex);
  const box = bounds(pixels, CELL_SIZE, CELL_SIZE);
  let face;
  try {
    face = faceBounds(pixels, CELL_SIZE, CELL_SIZE, box);
  } catch {
    return null;
  }

  return {
    top: box.y0,
    foot: box.y1,
    height: box.height,
    faceW: face.width,
    faceCx: (face.x0 + face.x1) / 2,
    chin: face.y1,
  };
}

export function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

export function scaleSignal(frame, reference, referenceHeight) {
  const ratios = [
    reference.faceW / frame.faceW,
    referenceHeight / (frame.foot - frame.top),
    (reference.foot - reference.chin) / (frame.foot - frame.chin),
  ];
  const ratio = median(ratios);
  const spread = (Math.max(...ratios) - Math.min(...ratios)) / ratio;
  return { ratio, spread };
}
