/** Keep only two recently used sheets alive; the browser manages the HTTP cache. */
export class SheetLoader {
  private images = new Map<string, HTMLImageElement>();
  private failed = new Set<string>();
  private pending = new Map<string, { promise: Promise<boolean>; cancel: () => void }>();
  private capacity = 2;
  private pinned = new Set<string>();

  private evict(): void {
    const disposable = [...this.images.keys()].filter(source => !this.pinned.has(source));
    while (disposable.length > this.capacity) this.images.delete(disposable.shift()!);
  }

  pin(source: string): Promise<boolean> {
    this.pinned.add(source);
    return this.load(source);
  }

  setCapacity(capacity: number): void {
    this.capacity = Math.max(1, Math.floor(capacity));
    this.evict();
  }

  isReady(source: string): boolean {
    return this.images.has(source);
  }

  load(source: string): Promise<boolean> {
    if (!source) return Promise.resolve(false);
    if (this.failed.has(source)) return Promise.resolve(false);
    const cached = this.images.get(source);
    if (cached) {
      this.images.delete(source);
      this.images.set(source, cached);
      return Promise.resolve(true);
    }
    const pending = this.pending.get(source);
    if (pending) return pending.promise;
    const image = new Image();
    let settle!: (ready: boolean) => void;
    const promise = new Promise<boolean>((resolve) => { settle = resolve; });
    const finish = (ready: boolean) => {
      image.onload = null;
      image.onerror = null;
      this.pending.delete(source);
      if (ready) {
        this.images.set(source, image);
        this.evict();
      } else {
        this.failed.add(source);
      }
      settle(ready);
    };
    this.pending.set(source, { promise, cancel: () => { finish(false); image.src = ''; } });
    image.onload = () => finish(true);
    image.onerror = () => finish(false);
    image.src = source;
    return promise;
  }

  clear(keepPinned = false): void {
    for (const [source, { cancel }] of [...this.pending]) {
      if (!keepPinned || !this.pinned.has(source)) cancel();
    }
    for (const source of [...this.images.keys()]) {
      if (!keepPinned || !this.pinned.has(source)) this.images.delete(source);
    }
    if (!keepPinned) { this.failed.clear(); this.pinned.clear(); }
  }
}
