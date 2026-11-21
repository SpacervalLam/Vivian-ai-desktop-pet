import type { ChibiPlayback } from './motionRegistry';

/** Eight visible poses make a brisk turn; 20ms also survives GIF timer clamping. */
export function quickTurnPlayback(frameCount: number): ChibiPlayback {
  const count = Math.min(8, frameCount);
  return {
    frames: Array.from({ length: count }, (_, i) => count === 1 ? 0 : Math.round(i * (frameCount - 1) / (count - 1))),
    durations: Array(count).fill(20),
  };
}

/** Movement and feet use elapsed time, so delayed callbacks cannot accumulate drift. */
export async function playWalkingFrames(options: {
  frames: number; frameDelayMs: number; cycleFrames: number;
  show: (frame: number) => void; interrupted: () => boolean;
}, clock = {
  now: () => performance.now(),
  wait: (ms: number) => new Promise<void>(resolve => window.setTimeout(resolve, ms)),
}): Promise<boolean> {
  const { frames, frameDelayMs, cycleFrames, show, interrupted } = options;
  const started = clock.now();
  const duration = frames * frameDelayMs;
  let shown = -1;
  while (!interrupted()) {
    const elapsed = Math.max(0, clock.now() - started);
    const step = Math.min(frames, Math.floor(elapsed / frameDelayMs));
    const frame = step % cycleFrames;
    if (frame !== shown) { show(frame); shown = frame; }
    if (elapsed >= duration) return true;
    await clock.wait(Math.max(1, Math.min(duration, (step + 1) * frameDelayMs) - elapsed));
  }
  return false;
}
