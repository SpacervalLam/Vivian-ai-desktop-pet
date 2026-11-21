import * as THREE from 'three';

/** Index identical vertex records without rounding positions, normals or UV seams. */
export function compactGeometry(geometry: THREE.BufferGeometry): number {
  // Animated and interleaved buffers have additional ownership/layout constraints.
  if (geometry.index || geometry instanceof THREE.InstancedBufferGeometry || Object.keys(geometry.morphAttributes).length) return 0;
  const attributes = Object.values(geometry.attributes);
  const count = geometry.getAttribute('position')?.count ?? 0;
  if (count < 3 || attributes.some(a =>
    !(a instanceof THREE.BufferAttribute) ||
    a instanceof THREE.InstancedBufferAttribute ||
    a.usage !== THREE.StaticDrawUsage ||
    !(a.array instanceof Float32Array) ||
    a.count !== count
  )) return 0;
  const records = attributes.map(a => ({
    attribute: a as THREE.BufferAttribute,
    words: new Uint32Array(a.array.buffer, a.array.byteOffset, a.array.length),
  }));
  const stride = attributes.reduce((sum, a) => sum + a.itemSize * 4, 0);
  let capacity = 1;
  while (capacity < count * 2) capacity *= 2;
  const table = new Uint32Array(capacity);
  const sources = new Uint32Array(count);
  const indices = new Uint32Array(count);
  let unique = 0;

  const equal = (a: number, b: number): boolean => {
    for (const { attribute, words } of records) {
      const size = attribute.itemSize;
      for (let k = 0; k < size; k++) {
        if (words[a * size + k] !== words[b * size + k]) return false;
      }
    }
    return true;
  };
  for (let vertex = 0; vertex < count; vertex++) {
    let hash = 2166136261;
    for (const { attribute, words } of records) {
      const start = vertex * attribute.itemSize;
      for (let k = 0; k < attribute.itemSize; k++) hash = Math.imul(hash ^ words[start + k], 16777619);
    }
    let slot = hash & (capacity - 1);
    while (table[slot] && !equal(vertex, sources[table[slot] - 1])) slot = (slot + 1) & (capacity - 1);
    if (!table[slot]) {
      sources[unique] = vertex;
      table[slot] = ++unique;
    }
    indices[vertex] = table[slot] - 1;
  }

  // Three reserves 65535 for primitive restart, so that value needs Uint32 too.
  const useShortIndex = unique <= 65535;
  const saved = count * stride - (unique * stride + count * (useShortIndex ? 2 : 4));
  if (saved <= 0) return 0;
  // Release any uploaded buffers before replacing their attributes in place.
  geometry.dispose();
  for (const [name, source] of Object.entries(geometry.attributes)) {
    const data = new Float32Array(unique * source.itemSize);
    const words = new Uint32Array(data.buffer);
    const original = new Uint32Array(source.array.buffer, source.array.byteOffset, source.array.length);
    for (let vertex = 0; vertex < unique; vertex++) {
      const start = sources[vertex] * source.itemSize;
      words.set(original.subarray(start, start + source.itemSize), vertex * source.itemSize);
    }
    const attribute = new THREE.BufferAttribute(data, source.itemSize, source.normalized);
    attribute.name = source.name;
    attribute.setUsage((source as THREE.BufferAttribute).usage);
    attribute.gpuType = (source as THREE.BufferAttribute).gpuType;
    geometry.setAttribute(name, attribute);
  }
  geometry.setIndex(new THREE.BufferAttribute(useShortIndex ? new Uint16Array(indices) : indices, 1));
  return saved;
}

/** Compact each shared geometry once, after static batches have been assembled. */
export function compactGeometries(root: THREE.Object3D): { compacted: number; bytes: number } {
  const visited = new Set<THREE.BufferGeometry>();
  let compacted = 0, bytes = 0;
  root.traverse(object => {
    const mesh = object as THREE.Mesh;
    if (!mesh.isMesh || (mesh as THREE.SkinnedMesh).isSkinnedMesh || visited.has(mesh.geometry)) return;
    visited.add(mesh.geometry);
    const saved = compactGeometry(mesh.geometry);
    if (saved) { compacted++; bytes += saved; }
  });
  return { compacted, bytes };
}
