/** CSS completion includes delayed entrances, but excludes perpetual ambience. */
export const handSceneTiming = { insetDelay: 4300, insetDuration: 1150, grip: 720, hold: 6000, release: 720 };
export const handInsetSettledAt = handSceneTiming.insetDelay + handSceneTiming.insetDuration;

export function hasRunningComicAnimation(animations: Animation[]): boolean {
  return animations.some(animation =>
    (animation.playState === 'running' || animation.pending) &&
    Number.isFinite(Number(animation.effect?.getComputedTiming().endTime)));
}

/** Effects driven by RAF/timers are not exposed by Element.getAnimations(). */
export function comicScriptDuration(page: number): number {
  if (page === 7) return handInsetSettledAt + handSceneTiming.grip + handSceneTiming.hold + handSceneTiming.release;
  if (page === 9) return 5800; // Morning ink front.
  if (page === 4) return 1100; // Full-page panel closes into place.
  return 0;
}

export function nextTourPage(page: number, total: number): number {
  return (page + 1) % total;
}
