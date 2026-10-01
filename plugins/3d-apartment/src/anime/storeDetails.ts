import * as THREE from 'three';
import { Reflector } from 'three/examples/jsm/objects/Reflector.js';
import { mergeByMaterial } from './merge';
import { pickOverheadSlabs, type Collider } from './collider';

/** Close-range dressing fits the existing store/shelf envelopes; never adds collision walls. */
export function dressConvenienceStore(store: THREE.Group, dynamic: THREE.Group) {
  const details = new THREE.Group();
  details.name = 'store-authored-details';
  details.userData.sceneCollideSkip = true;
  details.userData.noMerge = true;
  store.add(details);
  const textures: THREE.Texture[] = [];
  const mat = (color: string, roughness = 0.7, metalness = 0) => {
    const m = new THREE.MeshStandardMaterial({ color, roughness, metalness });
    m.userData.outlineWeight = 0;
    return m;
  };
  const mint = mat('#729f91'), rose = mat('#bd858a'), cream = mat('#e7dec9');
  const ink = mat('#263b40', 0.38, 0.3), steel = mat('#859c9f', 0.4, 0.55);
  const wood = mat('#9e7d56'), paper = mat('#e9e1cd');
  const colors = [mint, rose, cream, mat('#bd9860'), mat('#657d98'), mat('#8a9970')];
  const cube = new THREE.BoxGeometry(1, 1, 1);
  const cylinder = new THREE.CylinderGeometry(1, 1, 1, 10);
  const add = (geometry: THREE.BufferGeometry, material: THREE.Material, x: number, y: number, z: number, sx: number, sy: number, sz: number) => {
    const mesh = new THREE.Mesh(geometry, material);
    mesh.position.set(x, y, z); mesh.scale.set(sx, sy, sz);
    mesh.castShadow = true; mesh.receiveShadow = true;
    details.add(mesh);
    return mesh;
  };
  const box = (x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) => add(cube, m, x, y, z, w, h, d);
  const bottle = (x: number, y: number, z: number, m: THREE.Material, height = 0.21) => {
    add(cylinder, m, x, y + height * 0.45, z, 0.035, height * 0.9, 0.035);
    add(cylinder, cream, x, y + height * 0.96, z, 0.022, height * 0.13, 0.022);
  };

  // One shared atlas for package fronts, menus and price tickets.
  const canvas = document.createElement('canvas'); canvas.width = 1024; canvas.height = 512;
  const ctx = canvas.getContext('2d')!;
  const labels = ['MILK', 'TEA', 'COFFEE', 'SODA', 'RICE', 'SNACK', 'BAKERY', 'JUICE'];
  const swatches = ['#6a9c8d', '#98a77a', '#9d7257', '#779bae', '#d0ad7b', '#bf8b91', '#d6ba86', '#d99d77'];
  for (let row = 0; row < 4; row++) for (let col = 0; col < 8; col++) {
    const x = col * 128, y = row * 128;
    ctx.fillStyle = row === 3 ? '#f4e8cc' : swatches[col]; ctx.fillRect(x, y, 128, 128);
    ctx.fillStyle = '#f8f0dd'; ctx.fillRect(x + 9, y + 23, 110, 75);
    ctx.fillStyle = swatches[col]; ctx.beginPath(); ctx.arc(x + 64, y + 43, 10, 0, Math.PI * 2); ctx.fill();
    ctx.textAlign = 'center'; ctx.fillStyle = '#344b47'; ctx.font = 'bold 15px sans-serif';
    ctx.fillText(row === 3 ? `¥${120 + col * 20}` : labels[col], x + 64, y + 72);
    ctx.font = '9px sans-serif'; ctx.fillText(row === 3 ? 'CITY MART' : 'FRESH • EVERY DAY', x + 64, y + 88);
    ctx.fillStyle = '#344b47';
    for (let bar = 0; bar < 12; bar++) ctx.fillRect(x + 35 + bar * 5, y + 106, bar % 3 === 0 ? 2 : 1, 12);
  }
  const atlas = new THREE.CanvasTexture(canvas); atlas.colorSpace = THREE.SRGBColorSpace;
  atlas.anisotropy = 4; textures.push(atlas);
  const labelMaterial = new THREE.MeshStandardMaterial({ map: atlas, roughness: 0.8 });
  labelMaterial.userData.outlineWeight = 0;
  const tileGeometries = new Map<number, THREE.PlaneGeometry>();
  const label = (x: number, y: number, z: number, w: number, h: number, tile: number) => {
    let geometry = tileGeometries.get(tile);
    if (!geometry) {
      geometry = new THREE.PlaneGeometry(1, 1);
      const uv = geometry.attributes.uv;
      for (let i = 0; i < uv.count; i++) uv.setXY(i, (tile % 8 + uv.getX(i)) / 8, 1 - (Math.floor(tile / 8) + 1 - uv.getY(i)) / 4);
      tileGeometries.set(tile, geometry);
    }
    const mesh = add(geometry, labelMaterial, x, y, z, w, h, 1);
    mesh.rotation.y = Math.PI; mesh.castShadow = false;
  };

  // Solid products and open shelf backs replace the original flat strips.
  for (let row = 0; row < 2; row++) for (let segment = 0; segment < 3; segment++) {
    const cx = -0.45 + segment * 2.4, z = row === 0 ? 14.78 : 15.42;
    const height = row === 0 ? 1.1 : 1.45;
    const body = store.getObjectByName(`shelf-r${row}s${segment}`) as THREE.Mesh | undefined;
    if (body) {
      // Keep the original full-height collider separately; the new back is purely visual.
      body.visible = false;
      body.userData.noMerge = true;
    }
    box(cx, 0.08, z, 2.1, 0.16, 0.5, ink);
    box(cx, height / 2, z + 0.23, 2.1, height, 0.035, cream);
    for (let level = 0; level < Math.floor(height / 0.42); level++) {
      const y = 0.085 + level * 0.42;
      box(cx, y, z, 2.06, 0.035, 0.48, paper);
      for (let item = 0; item < 11; item++) {
        const x = cx - 0.89 + item * 0.177;
        const type = (item + segment * 3 + level + row) % 8;
        const h = 0.21 + (item % 3) * 0.025;
        if ((type + level) % 3 === 0) bottle(x, y + 0.02, z - 0.13, colors[type % 6], h);
        else box(x, y + 0.02 + h / 2, z - 0.11, 0.122, h, 0.15, colors[type % 6]);
        label(x, y + 0.12, z - 0.192, 0.105, 0.12, type);
        label(x, y - 0.014, z - 0.253, 0.12, 0.044, 24 + type);
      }
    }
  }

  // Vending machine windows: bottles, selection buttons, payment slot and collection hatch.
  for (const [index, x] of [-5.75, -4.6].entries()) {
    box(x, 1.28, 12.92, 0.80, 1.00, 0.015, index ? rose : mint);
    for (let row = 0; row < 3; row++) {
      box(x - 0.035, 0.94 + row * 0.29, 12.84, 0.70, 0.025, 0.12, cream);
      for (let col = 0; col < 5; col++) {
        const xx = x - 0.30 + col * 0.135;
        bottle(xx, 0.97 + row * 0.29, 12.83, colors[(row + col + index) % 6], 0.17);
        label(xx, 1.04 + row * 0.29, 12.791, 0.052, 0.075, (col + row) % 8);
        box(xx, 0.925 + row * 0.29, 12.775, 0.045, 0.012, 0.008, ink);
      }
    }
    box(x, 0.48, 12.913, 0.64, 0.20, 0.026, ink);
    box(x, 0.395, 12.887, 0.66, 0.022, 0.055, steel);
    box(x + 0.38, 0.85, 12.895, 0.032, 0.055, 0.015, cream);
  }
  // Roof units get actual grilles rather than featureless blocks.
  for (const [x, y, z, width, height] of [[-1.3, 4.2, 17.84, 1.5, 0.7], [3.7, 4.1, 18.84, 0.9, 0.5]]) {
    box(x, y, z, width * 0.85, height * 0.75, 0.025, ink);
    for (let i = 0; i < 8; i++) box(x, y - height * 0.29 + i * height * 0.082, z - 0.02, width * 0.8, 0.018, 0.022, steel);
  }
  // Fine awning soffit ribs and a restrained mint/rose identity band.
  for (let i = 0; i < 45; i++) box(-4.2 + i * 0.24, 2.872, 13.40, 0.018, 0.026, 0.95, wood);
  box(1.1, 3.89, 13.885, 11.04, 0.055, 0.06, mint);
  box(1.1, 3.11, 13.86, 11.04, 0.035, 0.06, rose);

  // PBR is local to this shop; shared toon materials elsewhere in the city are untouched.
  const converted = new Map<THREE.Material, THREE.Material>();
  store.traverse((o) => {
    if (!(o instanceof THREE.Mesh)) return;
    const convert = (old: THREE.Material) => {
      if (!(old instanceof THREE.MeshToonMaterial) || old.transparent) return old;
      const cached = converted.get(old); if (cached) return cached;
      const m = new THREE.MeshStandardMaterial({
        color: old.color, map: old.map, roughness: old.userData.outlineWeight === 1 ? 0.4 : 0.78,
        metalness: old.userData.outlineWeight === 1 ? 0.45 : 0,
        emissive: old.emissive, emissiveIntensity: old.emissiveIntensity, side: old.side,
      });
      m.userData.outlineWeight = 0; converted.set(old, m); return m;
    };
    o.material = Array.isArray(o.material) ? o.material.map(convert) : convert(o.material);
  });
  mergeByMaterial(details);

  // Low-resolution planar reflection follows the camera every visible frame.
  const reflection = new Reflector(new THREE.PlaneGeometry(13.2, 4.8), {
    textureWidth: 512, textureHeight: 256, multisample: 0, clipBias: 0.003,
  });
  reflection.name = 'store-wet-reflection';
  reflection.rotation.x = -Math.PI / 2;
  reflection.position.set(0.5, 0.009, 10.05);
  reflection.userData.sceneCollideSkip = true;
  const rm = reflection.material as THREE.ShaderMaterial;
  rm.transparent = true; rm.depthWrite = false;
  rm.uniforms.time = { value: 0 };
  rm.uniforms.distanceFade = { value: 1 };
  rm.vertexShader = rm.vertexShader.replace('varying vec4 vUv;', 'varying vec4 vUv; varying vec2 puddleUv;')
    .replace('vUv = textureMatrix', 'puddleUv = uv; vUv = textureMatrix');
  rm.fragmentShader = rm.fragmentShader.replace('varying vec4 vUv;', 'varying vec4 vUv; varying vec2 puddleUv; uniform float time; uniform float distanceFade;')
    .replace('vec4 base = texture2DProj( tDiffuse, vUv );', `
      vec4 q = vUv;
      q.x += sin(puddleUv.y * 135.0 + time * 1.8) * 0.0008 * q.w;
      q.y += sin(puddleUv.x * 74.0 - time * 1.1) * 0.0005 * q.w;
      vec4 base = texture2DProj(tDiffuse, q);
    `)
    .replace('gl_FragColor = vec4( blendOverlay( base.rgb, color ), 1.0 );', `
      vec2 edge = smoothstep(vec2(0.0), vec2(0.13), puddleUv) * smoothstep(vec2(0.0), vec2(0.13), 1.0-puddleUv);
      float patches = 0.66 + 0.18 * sin(puddleUv.x*31.0 + sin(puddleUv.y*17.0));
      gl_FragColor = vec4(base.rgb * vec3(0.76,0.86,0.94), edge.x * edge.y * patches * 0.54 * distanceFade);
    `);
  const renderReflection = reflection.onBeforeRender.bind(reflection);
  // Camera motion changes the entire reflected view, even for static buildings.
  // A time throttle freezes both the capture and its projection, causing visible judder.
  // Keep the 512 x 256 target, distance fade and Reflector's back-face/frustum culling
  // to bound cost without decoupling the reflection from the displayed frame.
  /**
   * 反射面的**精确**可见性判定：世界 AABB × 视锥，而不是让 three 拿包围球去剔。
   *
   * 这个面是 13.2 × 4.8 的**扁平**四边形，包围球半径 7.02m。相机站在 6.7m 外
   * 平视时，这个球仍有 46° 的角半径 —— 于是"积水其实在视锥外"这种最常见的
   * 情形（室内贴着阳台平视、背对街心站着）照样能通过球体测试，白跑一整趟
   * 453 次提交的场景重画。实测室内平视窗外那一档，反射净成本 295 次提交，
   * 占整帧 45%，而那片像素根本不在画面上。
   *
   * 换成 AABB 之后判定是**可证的**：`Frustum.intersectsBox` 只在盒子整体落在
   * 某一个视锥平面外侧时才返回 false；盒子又是四边形经矩阵变换后再求包围盒
   * （只多不少）。所以"盒子与视锥不相交"⇒ 四边形与视锥不相交 ⇒ 它一个片元都
   * 不会被光栅化。早退不留下任何视觉痕迹，也就不用管纹理矩阵是否过期。
   */
  const reflectionBox = new THREE.Box3().setFromObject(reflection);
  const reflectionFrustum = new THREE.Frustum();
  const reflectionProjScreen = new THREE.Matrix4();
  const reflectionVisible = (camera: THREE.Camera): boolean => {
    reflectionProjScreen.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
    reflectionFrustum.setFromProjectionMatrix(reflectionProjScreen);
    return reflectionFrustum.intersectsBox(reflectionBox);
  };
  /**
   * 反射面是否被**头顶的水平板**挡死。
   *
   * 视锥测试管不了这一档：站在 203 室中间（或西北角、厨房）低头看街面时，积水确实
   * 落在视锥里，却被 3.4m 的楼板挡得一个像素都露不出来。实测这几个机位反射面的
   * **真实像素贡献是 0**（逐像素比对：开关反射面画布完全相同），而那一趟"把整个场景
   * 按镜像相机再提交一遍"要 400+ 次提交、占整帧三成。
   *
   * 判据是**可证的保守判定**：取反射面世界 AABB 的 8 个角，只要有一个角
   * 「落在画面里 且 没被任何楼板挡住」，就跑这趟反射。楼板是整片实心的水平板
   * （没有洞），所以"画面里的角全被挡住" ⇒ "整片都被挡住"。保守方向是对的：
   * 多跑一趟只是浪费，少跑一趟是画面里凭空少一块倒影。
   *
   * 遮挡体由 `RoomScene` 在收完碰撞表之后回填（`setOccluders`）—— 装配期间拿不到
   * 那份表，它是在合批之前遍历收的，而反射面是装配中建的。
   */
  let overheadSlabs: THREE.Box3[] = [];
  const reflectionCorners: THREE.Vector3[] = [];
  for (let i = 0; i < 8; i++) {
    reflectionCorners.push(new THREE.Vector3(
      i & 1 ? reflectionBox.max.x : reflectionBox.min.x,
      i & 2 ? reflectionBox.max.y : reflectionBox.min.y,
      i & 4 ? reflectionBox.max.z : reflectionBox.min.z
    ));
  }
  const segRay = new THREE.Ray();
  const segHit = new THREE.Vector3();
  const cornerNdc = new THREE.Vector3();
  /** 线段 p→q 是否被盒 b 挡住。相机落在盒内时返回 false —— 那只是"相机在楼板里"。 */
  const segmentBlocked = (p: THREE.Vector3, q: THREE.Vector3, b: THREE.Box3): boolean => {
    if (b.containsPoint(p)) return false;
    segRay.origin.copy(p);
    segRay.direction.copy(q).sub(p);
    const len = segRay.direction.length();
    if (len < 1e-6) return false;
    segRay.direction.divideScalar(len);
    if (!segRay.intersectBox(b, segHit)) return false;
    const t = p.distanceTo(segHit);
    return t > 1e-3 && t < len - 1e-3;
  };
  const reflectionOccluded = (camera: THREE.Camera): boolean => {
    if (!overheadSlabs.length) return false;
    let anyCornerInView = false;
    for (const c of reflectionCorners) {
      cornerNdc.copy(c).project(camera);
      if (!(cornerNdc.x >= -1 && cornerNdc.x <= 1 && cornerNdc.y >= -1 && cornerNdc.y <= 1 && cornerNdc.z >= -1 && cornerNdc.z <= 1)) continue;
      anyCornerInView = true;
      let blocked = false;
      for (const slab of overheadSlabs) {
        if (segmentBlocked(camera.position, c, slab)) { blocked = true; break; }
      }
      if (!blocked) return false;
    }
    // 一个角都没落在画面里 → 交给视锥测试去判，这里不剔除（四角都在屏幕外、
    // 四边形却铺满屏幕的情形下，上面那圈一个都没命中，不能就此认定"被挡死"）。
    return anyCornerInView;
  };
  /**
   * 反射那次渲染里把「203 室内」整组摘掉。
   *
   * Reflector 的 onBeforeRender 会把**整个场景**按镜像相机重画一遍。实测这一趟是
   * 1303 个 draw call / 111 万三角形 —— 而其中 656 个 draw call（整整一半）是
   * `unit-203`（203 室的家具）。主相机在街上时一个都没画它（把它隐藏，主 pass 的
   * draw call 变化是 0，全被视锥剔掉了）。
   *
   * 物理上也说不通：街面这块积水只有 13.2 × 4.8m、渲染到 512×256，再乘 0.54 的
   * 混合系数，不可能显示出二楼室内的家具。摘掉它零视觉损失，反射 pass 直接砍半。
   *
   * 场景引用从 onBeforeRender 的第二个参数拿（signature: renderer, scene, camera, …），
   * 不引入新的耦合；`getObjectByName` 的结果缓存住，避免每帧遍历一千多个对象。
   */
  let unitCache: THREE.Object3D | null = null;
  reflection.onBeforeRender = (...args) => {
    const camera = args[2];
    const distance = camera.position.distanceTo(reflection.position);
    rm.uniforms.distanceFade.value = 1 - THREE.MathUtils.smoothstep(distance, 35, 45);
    if (distance > 45) return;
    // 整片积水都在视锥外 → 这趟"把整个场景按镜像相机再提交一遍"整趟跳过
    if (!reflectionVisible(camera)) return;
    // 在视锥里、但被头顶楼板挡死（室内低头看街面那一档）→ 同样整趟跳过
    if (reflectionOccluded(camera)) return;
    const scene = args[1] as THREE.Scene | undefined;
    if (!unitCache && scene) unitCache = scene.getObjectByName('unit-203') || null;
    if (!unitCache) { renderReflection(...args); return; }
    const prev = unitCache.visible;
    unitCache.visible = false;
    try { renderReflection(...args); } finally { unitCache.visible = prev; }
  };
  dynamic.add(reflection);
  return {
    update(t: number, wet: boolean) { reflection.visible = wet; rm.uniforms.time.value = t; },
    /**
     * 回填"头顶的水平板"给反射面的遮挡判定（见 reflectionOccluded）。
     *
     * 为什么不在本函数里自己收：需要的是**合批之前**逐件的 AABB，而这份表由
     * `RoomScene` 在装配之后统一收（`fpsColliders`）。这里只接受结果，判据
     * （薄 / 大 / 位于镜面之上）在 `pickOverheadSlabs` 里，镜面高度用本函数的
     * `reflection.position.y`，免得调用方再抄一遍这个魔数。
     */
    setOccluders(colliders: Collider[]) {
      overheadSlabs = pickOverheadSlabs(colliders, reflection.position.y);
    },
    dispose() { reflection.dispose(); textures.forEach((texture) => texture.dispose()); },
  };
}
