// Physical actions keep their own sprites; spoken content overrides idle/emotion poses.
const PHYSICAL_MOTIONS = new Set(['drag', 'walk', 'turn', 'cast', 'sleep', 'wake', 'tend', 'tend-out', 'busy-in', 'busy-out']);

export function speechDisplayMotion(motion: string, speaking: boolean, pressed: boolean): string {
  return speaking && !pressed && !PHYSICAL_MOTIONS.has(motion) ? 'talk' : motion;
}
