// Physical actions and timed conversational gestures keep their sprites during speech.
const PHYSICAL_MOTIONS = new Set(['edge', 'edge-bottom', 'lean', 'drag', 'dizzy', 'walk', 'float', 'turn', 'cast', 'sleep', 'wake', 'tend', 'tend-out', 'busy-in', 'busy-loop']);

export function speechDisplayMotion(motion: string, speaking: boolean, pressed: boolean, crossMotion: 'talk' | 'listen' | null = null): string {
  if (pressed || PHYSICAL_MOTIONS.has(motion)) return motion;
  if (speaking) return 'talk';
  return crossMotion ?? motion;
}
