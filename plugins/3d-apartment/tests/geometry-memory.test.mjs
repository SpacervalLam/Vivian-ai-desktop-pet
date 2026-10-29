import assert from 'node:assert/strict';
import * as THREE from 'three';
import { compactGeometry, compactGeometries } from '../src/anime/geometryMemory.ts';

function expandWords(geometry) {
  return Object.fromEntries(Object.entries(geometry.attributes).map(([name, a]) => {
    const words = new Uint32Array(a.array.buffer, a.array.byteOffset, a.array.length);
    const count = geometry.index?.count ?? a.count;
    const expanded = new Uint32Array(count * a.itemSize);
    for (let i = 0; i < count; i++) {
      const vertex = geometry.index ? geometry.index.getX(i) : i;
      expanded.set(words.subarray(vertex * a.itemSize, (vertex + 1) * a.itemSize), i * a.itemSize);
    }
    return [name, expanded];
  }));
}

// Triangle order and every attribute bit must survive, including hard normal/UV seams.
const box = new THREE.BoxGeometry(2, 3, 4).toNonIndexed();
const before = expandWords(box);
const groups = structuredClone(box.groups);
box.setDrawRange(3, 21);
assert.ok(compactGeometry(box) > 0);
assert.equal(box.attributes.position.count, 24, 'hard edges and UV seams remain distinct');
assert.deepEqual(expandWords(box), before);
assert.deepEqual(box.groups, groups);
assert.deepEqual(box.drawRange, { start: 3, count: 21 });
assert.equal(compactGeometry(box), 0, 'repeated calls do not rewrite indexed data');

// Close coordinates must not be merged by tolerance; signed zero/NaN payloads are exact too.
const precise = new THREE.BufferGeometry();
const bits = new Uint32Array([0, 0, 0, 0x80000000, 0, 0, 0x3727c5ac, 0, 0, 0x7fc00001, 0, 0, 0x7fc00002, 0, 0]);
const repeated = new Uint32Array(bits.length * 6);
for (let i = 0; i < 6; i++) repeated.set(bits, i * bits.length);
precise.setAttribute('position', new THREE.BufferAttribute(new Float32Array(repeated.buffer), 3));
const exact = expandWords(precise);
assert.ok(compactGeometry(precise) > 0);
assert.equal(precise.attributes.position.count, 5);
assert.deepEqual(expandWords(precise), exact);

const unique = new THREE.BufferGeometry().setAttribute('position', new THREE.Float32BufferAttribute([0,0,0,1,0,0,0,1,0],3));
const original = unique.attributes.position;
assert.equal(compactGeometry(unique), 0, 'index overhead must not increase memory');
assert.equal(unique.attributes.position, original);
for (const usage of [THREE.DynamicDrawUsage, THREE.StreamDrawUsage]) {
  const dynamic = new THREE.BoxGeometry().toNonIndexed();
  dynamic.attributes.position.setUsage(usage);
  assert.equal(compactGeometry(dynamic), 0, 'external dynamic attribute references remain intact');
}
const morph = new THREE.BoxGeometry().toNonIndexed();
morph.morphAttributes.position = [morph.attributes.position.clone()];
assert.equal(compactGeometry(morph), 0);
const interleaved = new THREE.BufferGeometry();
interleaved.setAttribute('position', new THREE.InterleavedBufferAttribute(new THREE.InterleavedBuffer(new Float32Array(36),3),3,0));
assert.equal(compactGeometry(interleaved), 0);

// Shared meshes retain their geometry identity and are compacted only once.
const scene = new THREE.Scene(), shared = new THREE.BoxGeometry().toNonIndexed();
scene.add(new THREE.Mesh(shared), new THREE.Mesh(shared));
assert.equal(compactGeometries(scene).compacted, 1);
assert.equal(scene.children[0].geometry, scene.children[1].geometry);
assert.equal(compactGeometries(scene).compacted, 0);

// WebGL reserves index 65535 for primitive restart. Crossing that boundary requires Uint32.
const large = new THREE.BufferGeometry(), vertices = new Float32Array(65536 * 3 * 3);
for (let i = 0; i < 65536 * 3; i++) vertices[i * 3] = i % 65536;
large.setAttribute('position', new THREE.BufferAttribute(vertices,3));
const largeBefore = expandWords(large);
assert.ok(compactGeometry(large) > 0);
assert.ok(large.index.array instanceof Uint32Array);
assert.deepEqual(expandWords(large), largeBefore);
console.log('Geometry memory: bit-exact triangles, seams, shared ownership, safety guards and index limits passed');
