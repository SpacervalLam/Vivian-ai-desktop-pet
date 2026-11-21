/** System-wide input inactivity changes the next opportunity, including an already pending rest. */
export class RoamRhythm {
  private inputIdle = false;
  private dueAt: number;

  constructor(private gravity: boolean, now: number, private random = Math.random) {
    this.dueAt = now + (gravity ? this.between(4_000, 9_000) : this.between(10_000, 25_000));
  }

  private between(min: number, max: number): number {
    return min + this.random() * (max - min);
  }

  observeInput(idleSeconds: number | null, now: number): void {
    const idle = idleSeconds !== null && Number.isFinite(idleSeconds) && idleSeconds >= 15;
    if (idle === this.inputIdle) return;
    this.inputIdle = idle;
    // No accumulated opportunities: resuming input starts a fresh normal rest.
    const next = now + this.restDelay();
    this.dueAt = idle ? Math.min(this.dueAt, next) : next;
  }

  private restDelay(): number {
    if (this.inputIdle) return this.between(1_500, 4_000);
    return this.gravity ? this.between(9_000, 24_000) : this.between(48_000, 120_000);
  }

  schedule(now: number, retry = false): void {
    this.dueAt = now + (retry
      ? this.inputIdle ? this.between(750, 1_500) : this.between(8_000, 14_000)
      : this.restDelay());
  }

  ready(now: number): boolean { return now >= this.dueAt; }

  /** Poll cheaply during a long rest so becoming idle takes effect promptly. */
  nextCheckMs(now: number): number { return Math.max(0, Math.min(2_000, this.dueAt - now)); }
}
