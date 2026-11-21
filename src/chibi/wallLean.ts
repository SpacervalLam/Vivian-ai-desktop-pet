import { defaultSizeTransform } from './defaultSize';
import { frameStyle, type ChibiAnimationSpec, type ChibiDirection } from './motionRegistry';

export function leanWallPositions(monitor: { position: { x: number }; size: { width: number } }, width: number) {
  return { left: monitor.position.x, right: monitor.position.x + monitor.size.width - width };
}

/** Project the marked contact through the same reflection, scale and registration as the sprite. */
export function leanContactX(spec: ChibiAnimationSpec, character: string, side: ChibiDirection): number | null {
  const point = (spec.wallContact ?? spec.palmContact)?.[side];
  if (!Array.isArray(point)) return null;
  const [x, y] = point;
  if (!(x >= 0 && x <= 512 && y >= 0 && y <= 512)) return null;
  return projectContactX(spec, character, side, spec.frames - 1, x);
}

function projectContactX(spec: ChibiAnimationSpec, character: string, side: ChibiDirection, frame: number, x: number) {
  const style = frameStyle(spec, frame, character, side);
  const scale = Number.parseFloat(style.scale ?? '1');
  const offset = Number.parseFloat(style.translate ?? '0');
  return 50 + (offset + (x * 100 / 512 - 50) * scale) * (defaultSizeTransform(character)?.scale ?? 1);
}

/** Approach the earliest palm mark, then keep each marked drawing flush with the wall. */
export function wallLeanOffsets(spec: ChibiAnimationSpec, character: string, side: ChibiDirection, target: number): number[] | null {
  const final = leanContactX(spec, character, side);
  if (final === null) return null;
  const marks = spec.wallContactFrames;
  const from = marks?.from ?? spec.frames - 1;
  const xs = marks?.[side] ?? (spec.directions === null ? marks?.right ?? marks?.left : undefined);
  if (!Array.isArray(xs) || !Number.isInteger(from) || from < 0 || from >= spec.frames
    || xs.length !== spec.frames - from || !xs.every(x => Number.isFinite(x) && x >= 0 && x <= 512)) {
    return Array.from({length: spec.frames}, (_, frame) =>
      (target - final) * (spec.frames === 1 ? 1 : frame / (spec.frames - 1)));
  }
  const firstOffset = target - projectContactX(spec, character, side, from, xs[0]);
  return Array.from({length: spec.frames}, (_, frame) => frame < from
    ? firstOffset * frame / from
    : target - projectContactX(spec, character, side, frame, xs[frame - from]));
}
