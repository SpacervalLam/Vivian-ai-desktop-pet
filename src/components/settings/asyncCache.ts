/** Concurrent readers share a load. Invalidated loads cannot overwrite newer data. */
export function createAsyncCache<T>(load: () => Promise<T>) {
  let generation = 0;
  let cached: { value: T } | undefined;
  let pending: Promise<T> | undefined;
  const listeners = new Set<() => void>();
  const get = (): Promise<T> => {
    if (cached) return Promise.resolve(cached.value);
    if (pending) return pending;
    const current = generation;
    const request = Promise.resolve().then(load).then(value => {
      if (current !== generation) return get();
      cached = { value };
      return value;
    }, error => {
      if (current !== generation) return get();
      throw error;
    }).finally(() => {
      if (pending === request) pending = undefined;
    });
    pending = request;
    return request;
  };
  return {
    get,
    invalidate() {
      generation++;
      cached = undefined;
      pending = undefined;
      for (const listener of listeners) listener();
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
  };
}
