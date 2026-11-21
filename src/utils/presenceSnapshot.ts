interface PresenceSnapshotEnv<T> {
  ready: () => Promise<void>;
  read: () => Promise<T>;
  apply: (snapshot: T) => void;
  cancelled: () => boolean;
  revision: () => number;
  wait: () => Promise<void>;
}

/** Early pet windows must wait for their backend character and recover from a failed read. */
export async function syncPresenceSnapshot<T>(env: PresenceSnapshotEnv<T>): Promise<void> {
  await env.ready();
  while (!env.cancelled()) {
    const revision = env.revision();
    try {
      const snapshot = await env.read();
      // A state-change event received during IPC is newer than this snapshot.
      if (!env.cancelled() && revision === env.revision()) env.apply(snapshot);
      return;
    } catch {
      if (env.cancelled()) return;
      await env.wait();
    }
  }
}
