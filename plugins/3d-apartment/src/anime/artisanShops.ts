import * as THREE from 'three';
import type { Collider } from './collider';
import { freezeStatic, mergeByMaterial } from './merge';

/** Each entry is a single address, with its own trade and separately authored display. */
export const ARTISAN_SHOPS = [
  ['拾音唱片', 'LISTENING ROOM · VINYL'], ['分针修钟所', 'WATCHMAKER · SINCE 1986'],
  ['铅字小屋', 'LETTERPRESS & EDITIONS'], ['蓝调暗房', 'FILM LAB · 35 MM'],
  ['土与火', 'POTTERY · OPEN STUDIO'], ['弦外琴坊', 'LUTHIER · REPAIRS'],
  ['雨线制伞', 'UMBRELLAS · HANDCRAFTED'], ['星野光学', 'TELESCOPES & LENSES'],
  ['慢针裁缝', 'TAILOR · MADE TO MEASURE'], ['山屿矿物', 'GEOLOGY · SPECIMENS'],
  ['折光玻璃', 'GLASSWORK · SMALL BATCH'], ['纸翼工房', 'PAPER SCULPTURE'],
  ['旧键打字社', 'TYPEWRITERS · RESTORED'], ['苔庭盆景', 'BONSAI · LIVING FORMS'],
  ['可可刻度', 'BEAN TO BAR · CHOCOLATE'], ['风切滑板', 'SKATEBOARDS · CUSTOM'],
  ['远山装订', 'BOOKBINDING · RESTORATION'], ['灯织', 'LANTERNS · PAPER & LIGHT'],
  ['白桃冰室', 'GELATO · MADE HERE'], ['航海模型室', 'SHIP MODELS · WORKSHOP'],
  ['浮标钓具', 'RODS · TACKLE · STORIES'], ['琥珀香室', 'BOTANICAL PERFUMERY'],
  ['棋间', 'CHESS · GO · TABLE GAMES'], ['海盐鱼铺', 'FRESH CATCH · DAILY'],
  ['铜管留声', 'BRASS INSTRUMENTS'], ['织物档案', 'YARN · LOOMS · NATURAL DYE'],
  ['雨燕修鞋', 'COBBLER · LEATHERWORK'], ['绿波收音机', 'RADIOS · VALVE REPAIRS'],
  ['山脊户外', 'CLIMB · CAMP · EXPLORE'], ['木马玩具社', 'WOODEN TOYS · HEIRLOOMS'],
  ['一寸制笔', 'FOUNTAIN PENS · NIBS'], ['翠羽标本室', 'BOTANICAL FIELD NOTES'],
  ['面影刻章', 'SEALS · STONE ENGRAVING'], ['长风风筝社', 'KITES · SKY STUDIES'],
  ['月相甜点', 'PATISSERIE · MOON PHASES'], ['苇岸茶器', 'TEAWARE · BAMBOO'],
  ['光谱霓虹', 'NEON · LETTERING STUDIO'], ['小岛香料', 'SPICES · BLENDS · AROMA'],
  ['晨露蜂蜜', 'LOCAL HONEY · BEESWAX'], ['虹桥万花筒', 'KALEIDOSCOPES · OPTICS'],
] as const;

const PALETTES = [
  ['#d8c9ac', '#356068', '#cc9566'], ['#a8b7ae', '#384a58', '#d5b766'],
  ['#c8a99b', '#6e4248', '#e0cba0'], ['#c5c0a9', '#496552', '#c88855'],
  ['#b3bfca', '#374b79', '#d2a475'], ['#d7c8b4', '#6b565d', '#97a9a1'],
] as const;

export function createArtisanShops() {
  const geometries = new Set<THREE.BufferGeometry>();
  const materials = new Set<THREE.Material>();
  const textures = new Set<THREE.Texture>();
  const used = new Set<number>();
  const own = <T extends THREE.BufferGeometry>(geometry: T): T => { geometries.add(geometry); return geometry; };
  const material = (color: string, roughness = .8, metalness = 0) => {
    const result = new THREE.MeshStandardMaterial({ color, roughness, metalness });
    result.userData.outlineWeight = 0; materials.add(result); return result;
  };
  const cube = own(new THREE.BoxGeometry(1, 1, 1));
  const cylinder = own(new THREE.CylinderGeometry(1, 1, 1, 16));
  const ball = own(new THREE.IcosahedronGeometry(1, 1));
  const ring = own(new THREE.TorusGeometry(1, .045, 6, 24));
  const ink = material('#293a42'), paper = material('#eee4ce'), brass = material('#b99a62', .36, .62);
  const wood = material('#805d48'), dark = material('#4a3c35'), leaf = material('#547465');
  const red = material('#ae655b'), blue = material('#708ea3'), amber = material('#d6a455');
  const glass = new THREE.MeshStandardMaterial({ color: '#a9cdd0', transparent: true, opacity: .2, roughness: .2, depthWrite: false });
  const glow = material('#ffe2ac'); glow.emissive.set('#ffcc80'); glow.emissiveIntensity = .55;
  materials.add(glass);

  // Every tile is different artwork. One atlas keeps the 40 original signs inexpensive.
  const canvas = document.createElement('canvas'); canvas.width = 2048; canvas.height = 1024;
  const context = canvas.getContext('2d')!;
  ARTISAN_SHOPS.forEach(([title, subtitle], id) => {
    const x = (id % 8) * 256, y = Math.floor(id / 8) * 128, palette = PALETTES[id % PALETTES.length];
    context.fillStyle = palette[1]; context.fillRect(x, y, 256, 128);
    context.strokeStyle = palette[2]; context.lineWidth = 2; context.strokeRect(x + 8, y + 8, 240, 112);
    context.fillStyle = '#f3e8ce'; context.textAlign = 'center'; context.textBaseline = 'middle';
    context.font = '600 34px "Microsoft YaHei", serif'; context.fillText(title, x + 128, y + 47, 222);
    context.font = '11px Georgia, serif'; context.fillText(subtitle, x + 128, y + 86, 230);
    context.font = '9px monospace'; context.fillText(`SAKURA DISTRICT / ${String(id + 1).padStart(2, '0')}`, x + 128, y + 109);
  });
  const atlas = new THREE.CanvasTexture(canvas); atlas.colorSpace = THREE.SRGBColorSpace; atlas.anisotropy = 4;
  const signMaterial = new THREE.MeshStandardMaterial({ map: atlas, roughness: .8 });
  signMaterial.emissiveMap = atlas; signMaterial.emissive.set('#ffffff'); signMaterial.emissiveIntensity = .05;
  materials.add(signMaterial);
  textures.add(atlas);
  const surfaceCanvas = document.createElement('canvas'); surfaceCanvas.width = surfaceCanvas.height = 1024;
  const paint = surfaceCanvas.getContext('2d')!;
  ARTISAN_SHOPS.forEach((_, id) => {
    const x = id % 8 * 128, y = Math.floor(id / 8) * 128;
    let state = 137 + id * 7919;
    const random = () => { state = (Math.imul(state, 1664525) + 1013904223) >>> 0; return state / 4294967296; };
    paint.fillStyle = '#eeece3'; paint.fillRect(x, y, 128, 128);
    for (let n = 0; n < 600; n++) {
      paint.fillStyle = `rgba(79,69,58,${.025 + random() * .09})`;
      paint.fillRect(x + random() * 128, y + random() * 128, id % 4 === 1 ? 1 : 1 + random() * 3, id % 4 === 1 ? 5 + random() * 19 : 1);
    }
    paint.strokeStyle = 'rgba(85,77,64,.17)'; paint.lineWidth = .8;
    if (id % 4 === 0) for (let row = 0; row < 8; row++) {
      const py = y + row * 17; paint.beginPath(); paint.moveTo(x, py); paint.lineTo(x + 128, py); paint.stroke();
      for (let col = 0; col < 5; col++) { const px = x + col * 31 + row % 2 * 15; paint.beginPath(); paint.moveTo(px, py); paint.lineTo(px, py + 17); paint.stroke(); }
    } else if (id % 4 === 1) for (let n = 0; n < 9; n++) { paint.beginPath(); paint.moveTo(x + n * 16, y); paint.lineTo(x + n * 16, y + 128); paint.stroke(); }
    const wash = paint.createLinearGradient(0, y + 88, 0, y + 128); wash.addColorStop(0, 'rgba(68,78,61,0)'); wash.addColorStop(1, `rgba(68,78,61,${.08 + random() * .07})`);
    paint.fillStyle = wash; paint.fillRect(x, y + 88, 128, 40);
  });
  const surface = new THREE.CanvasTexture(surfaceCanvas); surface.colorSpace = THREE.SRGBColorSpace; surface.anisotropy = 4; textures.add(surface);

  function build(parent: THREE.Group, x: number, z: number, height: number, id: number, colliders: Collider[]) {
    const identity = ARTISAN_SHOPS[id];
    if (!identity || used.has(id)) throw new Error(`Artisan address must have an unused identity: ${id}`);
    used.add(id);
    const group = new THREE.Group(); group.name = `artisan-${id}-${identity[0]}`;
    group.position.set(x, 0, z); group.userData.business = identity[0]; group.userData.originalDesign = id;
    group.userData.noMerge = true; parent.add(group);
    const [wallColor, accentColor, detailColor] = PALETTES[id % PALETTES.length];
    const wall = material(wallColor), accent = material(accentColor), detail = material(detailColor);
    const addressSurface = surface.clone(); addressSurface.repeat.set(124 / 1024, 124 / 1024);
    addressSurface.offset.set((id % 8 * 128 + 2) / 1024, 1 - (Math.floor(id / 8) * 128 + 126) / 1024);
    textures.add(addressSurface); wall.map = addressSurface;
    const h = height + (id % 5) * .09, w = 3.24, d = 6.6;
    function mesh(geometry: THREE.BufferGeometry, mat: THREE.Material, px: number, py: number, pz: number, sx = 1, sy = 1, sz = 1) {
      const item = new THREE.Mesh(geometry, mat); item.position.set(px, py, pz); item.scale.set(sx, sy, sz);
      item.castShadow = !mat.transparent; item.receiveShadow = true; group.add(item); return item;
    }
    const box = (px: number, py: number, pz: number, sx: number, sy: number, sz: number, mat: THREE.Material) => mesh(cube, mat, px, py, pz, sx, sy, sz);
    const sphere = (px: number, py: number, pz: number, radius: number, mat: THREE.Material) => mesh(ball, mat, px, py, pz, radius, radius, radius);
    const hoop = (px: number, py: number, pz: number, radius: number, mat = brass) => mesh(ring, mat, px, py, pz, radius, radius, radius);
    const disc = (px: number, py: number, pz: number, radius: number, mat: THREE.Material, depth = .035) => {
      const item = mesh(cylinder, mat, px, py, pz, radius, depth, radius); item.rotation.x = Math.PI / 2; return item;
    };
    function rod(a: number[], b: number[], radius: number, mat = brass) {
      const from = new THREE.Vector3(...a), direction = new THREE.Vector3(...b).sub(from);
      const item = mesh(cylinder, mat, from.x + direction.x / 2, from.y + direction.y / 2, from.z + direction.z / 2, radius, direction.length(), radius);
      item.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), direction.normalize()); return item;
    }
    function solid(name: string, px: number, py: number, pz: number, sx: number, sy: number, sz: number, mat: THREE.Material, kind: Collider['kind'] = 'wall') {
      const item = box(px, py, pz, sx, sy, sz, mat); item.name = name;
      colliders.push({ source: `artisan-${id}-${name}`, kind, min: new THREE.Vector3(x + px - sx / 2, py - sy / 2, z + pz - sz / 2), max: new THREE.Vector3(x + px + sx / 2, py + sy / 2, z + pz + sz / 2) });
    }
    solid('forecourt', 0, .032, -.55, w, .064, 1.1, detail, 'floor');
    solid('floor', 0, .065, d / 2, w, .13, d, paper, 'floor');
    solid('left-wall', -1.55, 1.58, d / 2, .14, 3.03, d, wall);
    solid('right-wall', 1.55, 1.58, d / 2, .14, 3.03, d, wall);
    solid('back-wall', 0, 1.58, d - .07, w, 3.03, .14, wall);
    solid('ceiling', 0, 3.08, d / 2, w, .16, d, wood);
    box(0, 2.982, 1.65, .82, .035, .12, glow);
    solid('upper-storey', 0, (3.16 + h) / 2, d / 2, w, h - 3.16, d, wall);
    box(0, 3.23, -.055, 3.25, .12, .16, detail);
    box(0, h - .1, -.065, 3.26, .105, .18, accent);
    for (const side of [-1, 1]) {
      box(side * 1.565, (3.3 + h) / 2, -.055, .075, h - 3.3, .13, detail);
      box(side * 1.565, 1.57, -.03, .09, 2.94, .13, accent);
    }
    // Exterior joints, timbering, tile courses or projecting ribs belong to this address.
    if (id % 4 === 0) for (let row = 0; row < 5; row++) box(0, 3.46 + row * .42, -.035, 3.12, .02, .035, detail);
    else if (id % 4 === 1) for (let n = 0; n < 7; n++) box(-1.37 + n * .455, (3.4 + h) / 2, -.045, .035, h - 3.4, .055, wood);
    else if (id % 4 === 2) { box(-1.35, 3.63, -.065, .2, .62, .15, accent); box(1.37, h - .58, -.065, .2, .7, .15, detail); }
    else for (let n = 0; n < 4; n++) box(-1.22 + n * .81, 3.52, -.065, .1, .52 + n * .035, .13, wood);

    // Different entrances and display proportions retain at least 0.95m of clear doorway.
    const door = id % 2 ? -.99 : .99, display = -door * .49;
    const displayWidth = 1.75 + (id % 3) * .08;
    solid('window-plinth', display, .37, .02, displayWidth, .48, .15, accent);
    box(display, 1.48, -.055, displayWidth, 1.7, .025, glass);
    for (const px of [display - displayWidth / 2, display + displayWidth / 2]) box(px, 1.57, -.07, .055, 2.7, .12, detail);
    box(display, 2.42, -.07, displayWidth + .06, .08, .13, accent);
    box(door, 2.6, .03, .97, .12, .15, detail);
    const hinge = door + Math.sign(door) * .47;
    box(hinge, 1.39, .5, .045, 2.4, .91, accent);
    box(hinge - Math.sign(door) * .028, 1.57, .53, .012, 1.35, .67, glass);
    rod([hinge - Math.sign(door) * .06, 1.19, .85], [hinge - Math.sign(door) * .06, 1.48, .85], .014, brass);
    solid('workbench', -.82, .64, 2.55, .94, 1.02, 2.5 + (id % 4) * .16, wood);
    box(-.82, 1.18, 2.55, 1.07, .07, 2.65 + (id % 4) * .16, dark);
    for (let shelf = 0; shelf < 2 + id % 2; shelf++) {
      box(.78, 1 + shelf * .52, 5.95, 1.24, .065, .55, detail);
      for (let item = 0; item < 3; item++) {
        const px = .34 + item * .41, py = 1.17 + shelf * .52;
        box(px, py, 5.97, .18 + (id % 3) * .025, .23, .19, (item + shelf) % 2 ? accent : paper);
        box(px, py + .025, 5.86, .11, .025, .015, brass);
      }
    }

    // Address-specific upper elevations: round, tripartite, bay and ribbon windows.
    const upperY = 3.16 + (h - 3.16) * .51;
    if (id % 4 === 0) {
      disc(-.39, upperY, -.07, .71, accent, .11); disc(-.39, upperY, -.14, .60, blue);
      hoop(-.39, upperY, -.17, .64, detail);
      rod([-1.04, upperY, -.19], [.26, upperY, -.19], .022, paper);
      for (let n = 0; n < 4; n++) box(.12 + n * .035, upperY, -.18, .018, .77 - n * .1, .025, paper);
    } else if (id % 4 === 1) {
      for (let n = 0; n < 3; n++) {
        const px = -.91 + n * .86;
        box(px, upperY, -.08, .67, 1.32, .14, accent); box(px, upperY, -.17, .53, 1.16, .035, blue);
        box(px - .17, upperY, -.198, .12, 1.06, .022, paper); box(px, upperY + .51, -.205, .49, .07, .035, wood);
      }
      box(0, upperY - .78, -.21, 2.75, .12, .42, detail);
    } else if (id % 4 === 2) {
      box(.29, upperY, -.21, 1.85, 1.25, .47, accent); box(.29, upperY, -.46, 1.65, 1.06, .035, blue);
      for (const px of [-.28, .3, .88]) box(px, upperY, -.49, .035, 1.12, .045, detail);
      for (let n = 0; n < 5; n++) box(-.42 + n * .042, upperY, -.49, .02, .94, .025, paper);
      box(-1.13, upperY, -.08, .31, 1.56, .1, wood);
    } else {
      box(0, upperY, -.08, 2.85, .86, .16, accent); box(0, upperY, -.18, 2.68, .69, .035, blue);
      for (let n = 0; n < 5; n++) box(-1.08 + n * .54, upperY, -.21, .034, .77, .045, paper);
      for (let n = 0; n < 9; n++) box(-1.21 + n * .3, upperY + .22 - n * .012, -.217, .26, .06, .018, detail);
      for (let n = 0; n < 4; n++) box(-1.18 + n * .75, upperY - .69, -.16, .43, .065, .26, detail);
    }
    // A unique, closed roof profile for every address, without cloned roof meshes.
    const profile = new THREE.Shape(), rise = .31 + id * .009;
    profile.moveTo(-1.61, h); profile.lineTo(1.61, h);
    const roofs = [
      [[1.61, h + .2], [.55, h + rise + .34], [-1.61, h + .2]],
      [[1.61, h + rise], [.43, h + rise], [.43, h + .18], [-1.61, h + .18]],
      [[1.61, h + .14], [-1.61, h + rise + .22]],
      [[1.61, h + .22], [.76, h + rise], [0, h + .18], [-.76, h + rise + .12], [-1.61, h + .22]],
      [[1.61, h + .2], [1.05, h + .2], [1.05, h + rise + .26], [-1.05, h + rise + .26], [-1.05, h + .2], [-1.61, h + .2]],
    ];
    for (const [px, py] of roofs[id % roofs.length]) profile.lineTo(px, py);
    profile.closePath();
    mesh(own(new THREE.ExtrudeGeometry(profile, { depth: d + .12, bevelEnabled: false })), accent, 0, 0, -.06);
    if (id % 3 === 0) for (let n = 0; n < 4; n++) box(-1.12 + n * .73, h + .32 + n * .022, 4.8, .48, .055, 1.05, brass).rotation.x = -.12;
    else if (id % 3 === 1) { box(.94, h + .55, 5.2, .47, .7, .51, detail); box(.94, h + .92, 5.2, .62, .09, .65, ink); }
    else { rod([-.75, h + .12, 5.2], [-.75, h + .94, 5.2], .035, ink); hoop(-.75, h + .84, 5.2, .24, brass); }

    const sign = own(new THREE.PlaneGeometry(2.82, .66));
    const uv = sign.getAttribute('uv');
    for (let n = 0; n < uv.count; n++) uv.setXY(n, ((id % 8) + uv.getX(n)) / 8, 1 - (Math.floor(id / 8) + 1 - uv.getY(n)) / 8);
    mesh(sign, signMaterial, 0, 2.78, -.16).rotation.y = Math.PI;
    // Canopies have different spans, folds and supports, rather than repeating striped awnings.
    const span = 2.65 + (id % 5) * .085;
    if (id % 3 === 0) { box(0, 2.45, -.43, span, .08, .78, accent).rotation.x = -.12; rod([-1.25, 2.93, -.03], [-1.25, 2.4, -.79], .018, brass); }
    else if (id % 3 === 1) for (let n = 0; n < 5; n++) box(-span / 2 + .1 + n * (span - .2) / 4, 2.48 + n * .013, -.42, .08, .095, .81, wood);
    else { const canopy = box(-.18, 2.48, -.45, span - .25, .055, .8, detail); canopy.rotation.z = .035; rod([1.28, 2.93, 0], [1.28, 2.5, -.77], .016, ink); }

    // Every trade has a different three-dimensional display and hanging emblem.
    const px = display, py = 1.17, pz = .18;
    switch (id) {
      case 0: // Records and the angled horn of a listening station.
        for (let n = 0; n < 4; n++) { disc(px - .59 + n * .39, py + .14, pz, .205, ink); disc(px - .59 + n * .39, py + .14, pz - .025, .068, n % 2 ? red : amber); }
        box(-.82, 1.26, 2.1, .64, .1, .64, accent); mesh(cylinder, brass, -.82, 1.56, 2.1, .23, .36, .23).rotation.z = -.6; break;
      case 1:
        disc(px, py + .21, pz, .48, wood); disc(px, py + .21, pz - .035, .42, paper);
        for (let n = 0; n < 12; n++) { const a = n * Math.PI / 6; box(px + Math.sin(a) * .34, py + .21 + Math.cos(a) * .34, pz - .064, .025, .055, .025, ink).rotation.z = -a; }
        rod([px, py + .21, pz - .09], [px - .2, py + .4, pz - .09], .018, ink); rod([px, py + .21, pz - .1], [px + .28, py + .27, pz - .1], .014, brass); break;
      case 2:
        box(px, py, pz, .83, .14, .52, ink); disc(px - .32, py + .24, pz - .07, .23, brass); disc(px + .32, py + .24, pz - .07, .23, brass);
        box(px, py + .38, pz, .57, .12, .46, accent); box(px + .13, py + .48, pz - .27, .51, .015, .38, paper).rotation.x = -.3; break;
      case 3:
        box(px, py + .1, pz, .77, .43, .34, ink); disc(px + .12, py + .1, pz - .2, .22, brass); disc(px + .12, py + .1, pz - .24, .16, blue);
        box(px - .25, py + .36, pz, .23, .13, .17, detail); for (const dz of [2.1, 2.65]) hoop(-.79, 1.33, dz, .19, ink); break;
      case 4:
        mesh(own(new THREE.LatheGeometry([new THREE.Vector2(.15, 0), new THREE.Vector2(.25, .15), new THREE.Vector2(.18, .42), new THREE.Vector2(.1, .52)], 20)), accent, px, py - .15, pz);
        mesh(cylinder, detail, -.8, 1.25, 2.15, .39, .055, .39); sphere(-.8, 1.39, 2.15, .18, red); break;
      case 5:
        sphere(px - .06, py + .13, pz, .3, wood).scale.multiply(new THREE.Vector3(1, 1.3, .33)); sphere(px - .06, py + .47, pz, .22, wood).scale.z *= .35;
        box(px - .06, py + .92, pz, .095, .74, .08, dark); for (const dx of [-.025, 0, .025]) rod([px - .06 + dx, py, pz - .11], [px - .06 + dx, py + 1.2, pz - .11], .004, brass); break;
      case 6:
        for (let n = 0; n < 3; n++) { const x0 = px - .48 + n * .48; mesh(own(new THREE.ConeGeometry(.3, .14, 8)), [red, accent, amber][n], x0, py + .46 + n * .09, pz); rod([x0, py - .17, pz], [x0, py + .48 + n * .09, pz], .015, wood); hoop(x0 + .06, py - .13, pz, .08, brass); } break;
      case 7:
        const telescope = mesh(cylinder, paper, px, py + .42, pz, .14, .92, .14); telescope.rotation.z = -.76;
        disc(px + .33, py + .75, pz, .16, blue); for (const dx of [-.3, .3]) rod([px, py + .16, pz], [px + dx, .65, pz + .15], .022, brass); break;
      case 8:
        box(px, py + .03, pz, .75, .08, .48, wood); box(px - .2, py + .29, pz, .17, .51, .24, ink); box(px + .07, py + .49, pz, .55, .15, .25, ink);
        disc(px - .34, py + .39, pz - .17, .19, brass); box(px + .23, py + .06, pz - .08, .27, .025, .39, red); break;
      case 9:
        for (let n = 0; n < 5; n++) { const stone = mesh(own(new THREE.ConeGeometry(.13 + n * .018, .34 + n * .08, 5)), [blue, accent, brass, red, paper][n], px - .52 + n * .26, py + n * .06, pz); stone.rotation.z = (n - 2) * .17; } break;
      case 10:
        for (let n = 0; n < 3; n++) { mesh(cylinder, blue, px - .43 + n * .43, py + .11 + n * .08, pz, .14, .46 + n * .12, .14); hoop(px - .43 + n * .43, py + .4 + n * .08, pz, .12, brass); } break;
      case 11:
        for (let n = 0; n < 3; n++) { const wing = box(px - .48 + n * .48, py + .23 + n * .13, pz, .43, .025, .36, [paper, amber, red][n]); wing.rotation.set(.2, n * .35, -.45); box(px - .48 + n * .48, py + .18 + n * .13, pz - .13, .1, .07, .4, accent).rotation.x = .3; } break;
      case 12:
        box(px, py + .05, pz, .85, .24, .57, accent).rotation.x = -.18; mesh(cylinder, ink, px, py + .33, pz + .1, .11, .82, .11).rotation.z = Math.PI / 2;
        for (let r = 0; r < 3; r++) for (let n = 0; n < 7; n++) sphere(px - .28 + n * .09, py + .17 - r * .04, pz - .14 - r * .085, .029, paper); break;
      case 13:
        box(px, py - .06, pz, .75, .13, .44, blue); rod([px, py, pz], [px + .1, py + .62, pz], .045, wood);
        for (const [dx, dy] of [[-.24, .4], [.27, .55], [-.07, .77]]) sphere(px + dx, py + dy, pz, .23, leaf).scale.y *= .38; break;
      case 14:
        for (let r = 0; r < 3; r++) for (let n = 0; n < 5; n++) { box(px - .44 + n * .22, py + r * .13, pz + r * .11, .18, .10, .19, r % 2 ? dark : wood); box(px - .44 + n * .22, py + r * .13 + .055, pz + r * .11, .04, .012, .05, brass); } break;
      case 15:
        for (let n = 0; n < 3; n++) { const board = box(px - .42 + n * .42, py + .4, pz, .26, 1.12 + n * .1, .06, [accent, amber, red][n]); board.rotation.z = (n - 1) * .13; for (const dy of [-.32, .32]) disc(px - .42 + n * .42, py + .4 + dy, pz - .075, .055, paper); } break;
      case 16:
        box(px, py + .17, pz, .73, .51, .49, dark); for (const dx of [-.26, .26]) rod([px + dx, py - .18, pz], [px + dx, py + .72, pz], .023, brass);
        box(px, py + .52, pz, .89, .09, .58, wood); for (let n = 0; n < 4; n++) box(px, py + .08 + n * .045, pz - .27, .64, .025, .025, paper); break;
      case 17:
        for (let n = 0; n < 3; n++) { sphere(px - .44 + n * .44, py + .34, pz, .23, [paper, red, amber][n]).scale.y *= 1.4; rod([px - .44 + n * .44, py + .62, pz], [px - .44 + n * .44, 2.15, pz], .008, brass); } break;
      case 18:
        box(px, py, pz, .93, .18, .55, paper); for (let n = 0; n < 4; n++) { mesh(own(new THREE.ConeGeometry(.085, .2, 8)), detail, px - .34 + n * .22, py + .17, pz); sphere(px - .34 + n * .22, py + .31, pz, .11, [paper, red, leaf, amber][n]); } break;
      case 19:
        mesh(own(new THREE.SphereGeometry(1, 12, 6, 0, Math.PI * 2, 0, Math.PI / 2)), wood, px, py + .07, pz, .52, .16, .21).rotation.x = Math.PI;
        rod([px, py + .05, pz], [px, py + .88, pz], .015, dark); box(px + .17, py + .56, pz, .31, .45, .025, paper); break;
      case 20:
        for (let n = 0; n < 3; n++) { rod([px - .5 + n * .5, py - .11, pz], [px - .33 + n * .42, py + .88, pz], .014, dark); for (let k = 0; k < 3; k++) hoop(px - .45 + n * .47 + k * .05, py + .13 + k * .22, pz, .035, brass); } break;
      case 21:
        for (let n = 0; n < 5; n++) { const bx = px - .48 + n * .24; mesh(cylinder, n % 2 ? amber : blue, bx, py + .09 + n * .035, pz, .075, .28 + n * .055, .075); sphere(bx, py + .3 + n * .06, pz, .05, brass); } break;
      case 22:
        box(px, py, pz, .86, .04, .66, wood); for (let r = 0; r < 5; r++) for (let n = 0; n < 5; n++) if ((r + n) % 2 === 0) box(px - .34 + n * .17, py + .024, pz - .26 + r * .13, .16, .008, .12, paper);
        for (let n = 0; n < 4; n++) { mesh(cylinder, n % 2 ? paper : ink, px - .28 + n * .19, py + .1, pz, .055, .12 + n * .035, .055); } break;
      case 23:
        box(px, py, pz, 1.05, .1, .6, blue); for (let n = 0; n < 3; n++) { sphere(px - .35 + n * .34, py + .09, pz, .17, paper).scale.multiply(new THREE.Vector3(1.5, .4, .65)); const tail = mesh(own(new THREE.ConeGeometry(.11, .18, 3)), accent, px - .54 + n * .34, py + .1, pz); tail.rotation.z = Math.PI / 2; } break;
      case 24:
        hoop(px, py + .3, pz, .33, brass); rod([px + .3, py + .3, pz], [px + .3, py + .86, pz], .046, brass);
        mesh(own(new THREE.ConeGeometry(.18, .3, 16, 1, true)), brass, px + .3, py + .93, pz).rotation.x = Math.PI; break;
      case 25:
        for (const dx of [-.44, .44]) rod([px + dx, py - .12, pz], [px + dx, py + .8, pz], .025, wood);
        box(px, py + .72, pz, .99, .08, .18, wood); for (let n = 0; n < 9; n++) rod([px - .34 + n * .085, py + .08, pz], [px - .34 + n * .085, py + .67, pz], .007, n % 2 ? red : amber); break;
      case 26:
        sphere(px - .12, py + .08, pz, .26, wood).scale.multiply(new THREE.Vector3(1.5, .6, .65)); box(px + .26, py + .13, pz, .23, .22, .29, dark);
        rod([px - .45, py + .42, pz], [px + .44, py + .42, pz], .035, brass); box(px - .45, py + .42, pz, .16, .35, .24, ink); break;
      case 27:
        box(px, py + .16, pz, .91, .57, .34, wood); for (let n = 0; n < 6; n++) box(px - .29 + n * .07, py + .16, pz - .19, .028, .38, .018, ink);
        disc(px + .29, py + .1, pz - .2, .07, brass); rod([px + .33, py + .46, pz], [px + .52, py + 1.02, pz], .009, ink); break;
      case 28:
        const mountain = mesh(own(new THREE.ConeGeometry(.46, .72, 4)), accent, px, py + .13, pz); mountain.rotation.y = Math.PI / 4;
        hoop(px + .43, py + .46, pz, .14, amber); rod([px - .49, py - .1, pz], [px - .4, py + .91, pz], .025, brass); break;
      case 29:
        sphere(px, py + .28, pz, .29, detail).scale.multiply(new THREE.Vector3(1.4, .65, .5)); sphere(px + .3, py + .54, pz, .14, wood);
        for (const dx of [-.22, .22]) rod([px + dx, py + .18, pz], [px + dx, py - .12, pz], .022, wood);
        const rocker = hoop(px, py - .04, pz, .48, wood); rocker.scale.y *= .24; break;
      case 30:
        for (let n = 0; n < 6; n++) { const pen = box(px - .4 + n * .16, py + .23, pz, .045, .58 + n * .025, .045, n % 2 ? ink : accent); pen.rotation.z = -.24; sphere(px - .4 + n * .16, py + .55 + n * .012, pz, .03, brass); } break;
      case 31:
        for (let n = 0; n < 4; n++) { const panel = box(px - .48 + n * .31, py + .31 + n * .05, pz, .26, .48, .035, paper); panel.rotation.z = (n - 2) * .08; rod([px - .48 + n * .31, py + .12, pz - .027], [px - .48 + n * .31, py + .46, pz - .027], .008, leaf); sphere(px - .48 + n * .31, py + .4, pz - .035, .09, leaf).scale.y *= .4; } break;
      case 32:
        for (let n = 0; n < 4; n++) { box(px - .4 + n * .26, py + .07 + n * .045, pz, .18, .3 + n * .09, .18, [red, wood, accent, ink][n]); box(px - .4 + n * .26, py - .1, pz, .2, .055, .2, brass); } break;
      case 33:
        const kite = box(px, py + .38, pz, .58, .58, .025, red); kite.rotation.z = Math.PI / 4;
        rod([px, py + .06, pz], [px + .16, py - .24, pz], .008, brass); for (let n = 0; n < 3; n++) box(px + .16, py - .05 - n * .12, pz, .15, .06, .025, amber).rotation.z = .5; break;
      case 34:
        for (let n = 0; n < 5; n++) { disc(px - .48 + n * .24, py + .24, pz, .11, paper); if (n) disc(px - .51 + n * .24, py + .27, pz - .035, .09, accent); } break;
      case 35:
        mesh(cylinder, accent, px, py + .12, pz, .28, .3, .28); hoop(px + .34, py + .13, pz, .17, brass); rod([px - .24, py + .1, pz], [px - .46, py + .31, pz], .055, accent); sphere(px, py + .32, pz, .09, wood); break;
      case 36:
        for (let n = 0; n < 3; n++) { const neon = hoop(px - .34 + n * .34, py + .34, pz, .21, glow); neon.scale.y *= 1.6 - n * .2; } rod([px - .6, py - .04, pz], [px + .6, py - .04, pz], .018, accent); break;
      case 37:
        for (let n = 0; n < 4; n++) { mesh(cylinder, [red, amber, wood, leaf][n], px - .4 + n * .27, py + .05, pz, .12, .22, .12); mesh(cylinder, brass, px - .4 + n * .27, py + .18, pz, .125, .025, .125); } break;
      case 38:
        for (let n = 0; n < 3; n++) { mesh(cylinder, amber, px - .32 + n * .32, py + .09, pz, .12, .3, .12); box(px - .32 + n * .32, py + .09, pz - .13, .16, .13, .015, paper); mesh(cylinder, dark, px - .32 + n * .32, py + .26, pz, .13, .04, .13); } break;
      case 39:
        for (let n = 0; n < 3; n++) { const scope = mesh(cylinder, [accent, red, brass][n], px - .4 + n * .4, py + .28, pz, .1, .72, .1); scope.rotation.z = -.2 + n * .2; hoop(px - .4 + n * .4, py + .64, pz, .12, brass); } break;
    }
    // Staggered tool rails finish the workbench wall without obstructing the doorway.
    box(-1.42, 1.8, 3.15, .06, .82 + id * .006, .91, accent);
    for (let n = 0; n < 3 + id % 5; n++) rod([-1.36, 1.58 + n * .1, 2.86], [-1.36, 1.65 + n * .1, 3.41], .012, n % 2 ? brass : paper);
    const number = mesh(own(new THREE.PlaneGeometry(.26, .13)), signMaterial, door, 2.31, -.065);
    // Address plaque samples its own sign tile, never another shop's artwork.
    const plaqueUV = number.geometry.getAttribute('uv');
    for (let n = 0; n < plaqueUV.count; n++) plaqueUV.setXY(n, (id % 8 + .33 + plaqueUV.getX(n) * .34) / 8, 1 - (Math.floor(id / 8) + .78 + (1 - plaqueUV.getY(n)) * .14) / 8);
    number.rotation.y = Math.PI;
    mergeByMaterial(group); freezeStatic(group);
    return group;
  }

  return {
    build,
    setEnvironment(period: string) {
      glow.emissiveIntensity = period === 'night' ? 1.15 : period === 'dusk' ? .7 : .18;
      signMaterial.emissiveIntensity = period === 'night' ? .45 : period === 'dusk' ? .22 : .05;
    },
    dispose() { geometries.forEach(g => g.dispose()); materials.forEach(m => m.dispose()); textures.forEach(t => t.dispose()); },
  };
}
