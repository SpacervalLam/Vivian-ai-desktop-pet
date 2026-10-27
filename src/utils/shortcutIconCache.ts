type Entry = { promise: Promise<string | null>; expiresAt: number };

/** Share pending requests and briefly remember failures so panel remounts cannot retry-loop. */
export class ShortcutIconCache {
  private entries = new Map<string, Entry>();

  constructor(
    private now = Date.now,
    private failureTtlMs = 60_000,
    private capacity = 60,
  ) {}

  get(key: string, load: () => Promise<string | null>): Promise<string | null> {
    const cached = this.entries.get(key);
    if (cached && this.now() < cached.expiresAt) return cached.promise;
    this.entries.delete(key);
    const entry: Entry = { promise: Promise.resolve(null), expiresAt: Infinity };
    entry.promise = Promise.resolve().then(load).catch(() => null).then(value => {
      entry.expiresAt = value ? Infinity : this.now() + this.failureTtlMs;
      return value;
    });
    if (this.entries.size >= this.capacity) this.entries.delete(this.entries.keys().next().value!);
    this.entries.set(key, entry);
    return entry.promise;
  }
}
