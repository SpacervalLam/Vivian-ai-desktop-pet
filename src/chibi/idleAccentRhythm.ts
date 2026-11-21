/** Quiet-time hysteresis plus a fresh cooldown prevents clockwork or bursty gestures. */
export class IdleAccentRhythm {
  private dueAt: number;
  private quietSince: number | null = null;

  constructor(private intervalMs: number, now: number, private random = Math.random) {
    this.dueAt = now + this.delay();
  }

  private delay(): number {
    return Math.max(12_000, this.intervalMs * (0.8 + this.random() * 0.6));
  }

  ready(now: number, free: boolean): boolean {
    if (!free) { this.quietSince = null; return false; }
    this.quietSince ??= now;
    return now >= this.dueAt && now - this.quietSince >= 2_000;
  }

  played(now: number, durationMs: number): void {
    this.dueAt = now + durationMs + this.delay();
    this.quietSince = null;
  }
}
