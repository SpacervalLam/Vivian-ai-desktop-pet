import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { boxGeo } from './palette.js';

/* 同材质的静态构件一律进合批器。
 * 木格栅一根根算下来上千个盒子，逐个 Mesh 会让 swiftshader 直接卡死；
 * 合批后每个材质只剩一个 draw call。需要独立动画/透明/点光的对象不进这里。 */
export class Batch {
  constructor(materials, name = 'batch') {
    this.materials = materials;
    this.group = new THREE.Group();
    this.group.name = name;
    this.buckets = new Map();
  }

  /** 加一个轴对齐盒子（可绕 Y 旋转）。 */
  box(key, x, y, z, w, h, d, rotY = 0) {
    const g = boxGeo(w, h, d).clone();
    if (rotY) g.applyMatrix4(new THREE.Matrix4().makeRotationY(rotY));
    g.translate(x, y, z);
    this._push(key, g);
    return this;
  }

  /** 加任意几何（已自带世界变换）。 */
  geo(key, geometry, matrix) {
    const g = geometry.clone();
    if (matrix) g.applyMatrix4(matrix);
    this._push(key, g);
    return this;
  }

  _push(key, g) {
    if (!this.buckets.has(key)) this.buckets.set(key, []);
    this.buckets.get(key).push(g);
  }

  flush(parent, { cast = true, receive = true } = {}) {
    for (const [key, list] of this.buckets) {
      const merged = list.length === 1 ? list[0] : mergeGeometries(list, false);
      if (!merged) { console.warn('[batch] merge failed for', key); continue; }
      const mesh = new THREE.Mesh(merged, this.materials[key] || this.materials.plaster);
      mesh.name = `${this.group.name}:${key}`;
      mesh.castShadow = cast;
      mesh.receiveShadow = receive;
      this.group.add(mesh);
      if (list.length > 1) list.forEach(g => g.dispose());
    }
    this.buckets.clear();
    parent.add(this.group);
    return this.group;
  }
}

/** 竖条木格栅：一根根进 batch，间距/宽度按开间自动分配。 */
export function louvers(b, key, axis, from, to, y0, y1, depthAt, thick = .085, gap = .075, material = 'wood') {
  const span = to - from;
  const pitch = thick + gap;
  const n = Math.max(2, Math.floor(span / pitch));
  const step = span / n;
  for (let i = 0; i < n; i++) {
    const p = from + (i + .5) * step;
    if (axis === 'z') b.box(material, depthAt, (y0 + y1) / 2, p, thick, y1 - y0, step * .82);
    else b.box(material, p, (y0 + y1) / 2, depthAt, step * .82, y1 - y0, thick);
  }
}

/** 细金属栏杆：上下横杆 + 均匀竖柱。 */
export function railing(b, axis, from, to, at, y0, height, material = 'blackMetal') {
  const length = to - from;
  const mid = (from + to) / 2;
  const bar = (y) => axis === 'z'
    ? b.box(material, at, y, mid, .045, .045, length)
    : b.box(material, mid, y, at, length, .045, .045);
  bar(y0 + height);
  bar(y0 + height * .52);
  const n = Math.max(3, Math.round(length / .92));
  for (let i = 0; i <= n; i++) {
    const p = from + (i / n) * length;
    if (axis === 'z') b.box(material, at, y0 + height / 2, p, .035, height, .035);
    else b.box(material, p, y0 + height / 2, at, .035, height, .035);
  }
}
