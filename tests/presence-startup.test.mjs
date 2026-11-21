import assert from 'node:assert/strict';
import { build } from 'esbuild';
const built = await build({ entryPoints: ['src/utils/presenceSnapshot.ts'], bundle: true, write: false, platform: 'node', format: 'esm' });
const { syncPresenceSnapshot } = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].text).toString('base64'));

// Vivian/Nana mount before startup finishes. No presence event is required to enable roaming.
let releaseReady, reads = 0, retries = 0, state = null;
const ready = new Promise(resolve => { releaseReady = resolve; });
const pending = syncPresenceSnapshot({
  ready: () => ready,
  read: async () => { if (++reads === 1) throw Error('character not initialized'); return { state: 'online' }; },
  apply: value => { state = value.state; }, cancelled: () => false, revision: () => 0,
  wait: async () => { retries++; },
});
await Promise.resolve(); assert.equal(reads, 0, 'early windows wait for backend readiness');
releaseReady(); await pending;
assert.equal(state, 'online', 'a transient startup failure cannot leave ambientMotionEnabled false forever');
assert.equal(reads, 2); assert.equal(retries, 1);

// A late imported window takes the same path without retrying.
state = null;
await syncPresenceSnapshot({ ready: async () => {}, read: async () => ({ state: 'online' }),
  apply: value => { state = value.state; }, cancelled: () => false, revision: () => 0,
  wait: async () => { assert.fail('initialized imported character must not retry'); },
});
assert.equal(state, 'online');

let revision = 0;
state = 'rest';
await syncPresenceSnapshot({ ready: async () => {}, read: async () => { revision++; return { state: 'online' }; },
  apply: value => { state = value.state; }, cancelled: () => false, revision: () => revision, wait: async () => {},
});
assert.equal(state, 'rest', 'a newer event cannot be overwritten by an in-flight snapshot');

let cancelled = false;
await syncPresenceSnapshot({ ready: async () => {}, read: async () => { throw Error('not ready'); },
  apply: () => assert.fail('unmounted windows must not update state'), cancelled: () => cancelled,
  revision: () => 0, wait: async () => { cancelled = true; },
});
console.log('Presence startup: early built-in windows, transient failure recovery, late imported windows, event ordering and cancellation passed');
