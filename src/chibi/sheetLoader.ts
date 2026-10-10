/** Keep only two recently used sheets alive; the browser manages the HTTP cache. */
export class SheetLoader {
  private images = new Map<string, HTMLImageElement>();
  private failed = new Set<string>();
  private pending = new Map<string, { promise: Promise<boolean>; cancel: () => void }>();

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
        while (this.images.size > 2) this.images.delete(this.images.keys().next().value!);
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

  clear(): void {
    for (const { cancel } of [...this.pending.values()]) cancel();
    this.images.clear();
    this.failed.clear();
  }
}
