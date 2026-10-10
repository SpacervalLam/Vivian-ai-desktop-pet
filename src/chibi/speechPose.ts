// Physical actions and timed conversational gestures keep their sprites during speech.
const PHYSICAL_MOTIONS = new Set(['drag', 'walk', 'turn', 'cast', 'sleep', 'wake', 'tend', 'tend-out', 'busy-in', 'busy-out']);
export const CONVERSATION_GESTURES = new Set(['rps-paper', 'rps-rock', 'rps-scissors']);

export function speechDisplayMotion(motion: string, speaking: boolean, pressed: boolean, crossMotion: 'talk' | 'listen' | null = null): string {
  if (pressed || PHYSICAL_MOTIONS.has(motion) || CONVERSATION_GESTURES.has(motion)) return motion;
  if (speaking) return 'talk';
  return crossMotion ?? motion;
}
