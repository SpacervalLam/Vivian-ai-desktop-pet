/** Accumulate render time so simulation frequency is independent of monitor refresh rate. */
export class FixedStepClock {
  private remainder = 0;
  private readonly step: number;
  constructor(step: number) { this.step = step; }

  get alpha(): number { return this.remainder / this.step; }

  advance(elapsed: number): number {
    this.remainder += Math.max(0, Math.min(elapsed, 0.25));
    const steps = Math.floor((this.remainder + 1e-10) / this.step);
    this.remainder = Math.max(0, this.remainder - steps * this.step);
    return steps;
  }

  reset(): void { this.remainder = 0; }
}
