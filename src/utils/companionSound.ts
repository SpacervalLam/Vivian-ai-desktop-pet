let context: AudioContext | undefined;
/** A short local tone; sound preferences are checked by the caller. */
export function playCompanionTone(): void {
  try {
    context ??= new AudioContext();
    void context.resume().catch(() => {});
    const start = context.currentTime;
    for (const [index, hz] of [660, 880].entries()) {
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      const time = start + index * 0.11;
      oscillator.frequency.value = hz;
      gain.gain.setValueAtTime(0, time);
      gain.gain.linearRampToValueAtTime(0.025, time + 0.015);
      gain.gain.exponentialRampToValueAtTime(0.0001, time + 0.1);
      oscillator.connect(gain); gain.connect(context.destination);
      oscillator.onended = () => { oscillator.disconnect(); gain.disconnect(); };
      oscillator.start(time); oscillator.stop(time + 0.12);
    }
  } catch { /* Visual feedback remains available without an audio device. */ }
}
