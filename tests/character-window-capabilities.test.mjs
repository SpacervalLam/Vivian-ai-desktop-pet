import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';

const capability = JSON.parse(await readFile('src-tauri/capabilities/default.json', 'utf8'));
const ids = ['vivian', 'nana'];
for (const entry of await readdir('character-packs', { withFileTypes: true })) {
  if (!entry.isDirectory()) continue;
  const source = await readFile(`character-packs/${entry.name}/character.json`, 'utf8').catch(error => {
    if (error.code === 'ENOENT') return null; // Unfinished character artwork has no importable package yet.
    throw error;
  });
  if (source === null) continue;
  const manifest = JSON.parse(source);
  ids.push(manifest.id);
}
for (const id of ids) {
  for (const suffix of ['', '_status', '_bubble', '_toast']) {
    assert.ok(capability.windows.includes(id + suffix), `${id + suffix} must receive the local app capability before listening for events`);
  }
}
assert.ok(capability.permissions.includes('core:event:default'));
assert.ok(!capability.windows.includes('*'), 'character permissions must not grant access to every window');
console.log(`Character window capabilities: ${ids.join(', ')} and their overlays are covered`);
