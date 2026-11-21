/** Keep sampled colors across icon remounts and application restarts. */
export class ShortcutBackgroundCache {
  private colors = new Map<string, string>();
  private storage: () => Pick<Storage, 'getItem' | 'setItem'>;
  constructor(storage: () => Pick<Storage, 'getItem' | 'setItem'> = () => localStorage) { this.storage = storage; }

  get(key: string, sample: () => string): string {
    const memory = this.colors.get(key);
    if (memory) return memory;
    const storageKey = `phone-icon-background:v1:${key}`;
    try {
      const saved = this.storage().getItem(storageKey);
      if (saved && /^(#[\da-f]{6}|rgb\(\d{1,3}, \d{1,3}, \d{1,3}\))$/i.test(saved)) {
        this.colors.set(key, saved);
        return saved;
      }
    } catch { /* Memory caching still works when storage is unavailable. */ }
    const color = sample();
    this.colors.set(key, color);
    try { this.storage().setItem(storageKey, color); } catch { /* Keep the session cache. */ }
    return color;
  }
}
