import * as THREE from 'three';
import type { Collider } from './collider';
import { CITY_ROUTES, SOUTH_ROUTES } from './cityRoutes';
import { freezeStatic, mergeByMaterial } from './merge';

export const STREET_STORIES = [
  { id: 'letterpress-yard', title: '街角印刷日', sub: 'PRINT SOMETHING THAT MATTERS', x: -60, z: 25, w: 3.8, d: 3.0 },
  { id: 'weather-observatory', title: '今日的风', sub: 'NEIGHBOURHOOD WEATHER STATION', x: -33, z: -64, w: 3.2, d: 2.8 },
  { id: 'public-orrery', title: '抬头看星星', sub: 'A SMALL SOLAR SYSTEM', x: 13, z: 52, w: 3.6, d: 3.2 },
  { id: 'buskers-rehearsal', title: '傍晚排练', sub: 'ONE SONG BEFORE YOU GO', x: 55, z: 26, w: 4.1, d: 3.2 },
  { id: 'mapmakers-table', title: '走一条新路', sub: 'THE CITY IS A FIELD NOTE', x: 123, z: 7, w: 3.8, d: 3.0 },
  { id: 'stained-glass-court', title: '收集一束光', sub: 'GLASS · COLOR · AFTERNOON', x: 31, z: 55, w: 3.4, d: 2.8 },
  { id: 'open-air-cinema', title: '星期六放映室', sub: 'TONIGHT / THE MOON EXPRESS', x: 42, z: 117, w: 4.4, d: 3.5 },
  { id: 'stone-game-garden', title: '下一手', sub: 'TAKE A SEAT · TAKE YOUR TIME', x: -56, z: 73, w: 3.5, d: 3.2 },
  { id: 'sculptors-worksite', title: '石头的来信', sub: 'WORK IN PROGRESS · PLEASE LOOK', x: -64, z: -37, w: 3.8, d: 3.1 },
  { id: 'wind-paper-mobile', title: '风写的句子', sub: 'A KINETIC PAPER SCULPTURE', x: 19, z: 86, w: 3.3, d: 2.9 },
  { id: 'copper-water-point', title: '沿途补给', sub: 'DRINKING WATER · REFILL HERE', x: 111, z: 82, w: 3.0, d: 2.6 },
  { id: 'community-seed-library', title: '把春天带回家', sub: 'SEEDS · SWAP · GROW', x: -24, z: 52, w: 3.6, d: 2.8 },
] as const;

type Bounds = { x0: number; x1: number; z0: number; z1: number };
const intersects = (a: Bounds, b: Bounds) => a.x0 < b.x1 && a.x1 > b.x0 && a.z0 < b.z1 && a.z1 > b.z0;

/** One-off street scenes. Placement uses the completed district's actual occupancy. */
export function createStreetStories(scene: THREE.Scene, existing: Collider[]) {
  const root = new THREE.Group(); root.name = 'one-off-street-stories'; root.userData.sceneCollideSkip = true; scene.add(root);
  const colliders: Collider[] = [], geometries = new Set<THREE.BufferGeometry>(), materials = new Set<THREE.Material>();
  const moving: Array<{ object: THREE.Object3D; axis: 'y' | 'z'; speed: number; initial: number; sway: boolean }> = [];
  const nightOnly: THREE.Object3D[] = [];
  const plan: Array<{ id: string; title: string; x: number; z: number; angle: number; bounds: Bounds }> = [];
  const rejected: string[] = [];
  const own = <T extends THREE.BufferGeometry>(g: T): T => { geometries.add(g); return g; };
  const mat = (color: string, roughness = .82, metalness = 0) => {
    const m = new THREE.MeshStandardMaterial({ color, roughness, metalness }); m.userData.outlineWeight = 0; materials.add(m); return m;
  };
  const M = { ink: mat('#283d49'), cream: mat('#e5dcc5'), wood: mat('#89644c'), copper: mat('#b37d55', .4, .65),
    blue: mat('#678b9d'), red: mat('#a85f58'), green: mat('#537a69'), yellow: mat('#dbb86d'), stone: mat('#a8aaa0'), white: mat('#ece8dc') };
  const luminous = mat('#ffe3b1'); luminous.emissive.set('#ffd099');
  const glass = new THREE.MeshStandardMaterial({ color: '#a9d3da', transparent: true, opacity: .24, roughness: .18, depthWrite: false }); materials.add(glass);
  const cube = own(new THREE.BoxGeometry(1, 1, 1)), cylinder = own(new THREE.CylinderGeometry(1, 1, 1, 18));
  const sphere = own(new THREE.IcosahedronGeometry(1, 1)), torus = own(new THREE.TorusGeometry(1, .045, 6, 32));
  const occupancy: Bounds[] = existing.filter(c => c.max.y > .24 && c.min.y < 3.5 && c.kind !== 'floor' && c.kind !== 'ramp')
    .map(c => ({ x0: c.min.x - .22, x1: c.max.x + .22, z0: c.min.z - .22, z1: c.max.z + .22 }));
  occupancy.push(
    { x0: -40, x1: 40, z0: -10, z1: 9 }, { x0: -40, x1: 40, z0: -40, z1: -22 },
    { x0: -17.2, x1: 34.6, z0: 14.5, z1: 21 }, { x0: -9.9, x1: -8.3, z0: 14.6, z1: 22 },
    { x0: 78, x1: 103, z0: -78, z1: 165 },
  );
  const roads = [...CITY_ROUTES, ...SOUTH_ROUTES].flatMap(route => {
    const curve = new THREE.CatmullRomCurve3(route.points.map(([x, z]) => new THREE.Vector3(x, 0, z)), false, 'centripetal');
    return curve.getSpacedPoints(Math.ceil(curve.getLength() / .65)).map(point => ({ point, clear: route.width / 2 + (route.walk ? .7 : 1.3) }));
  });
  function locate(spec: typeof STREET_STORIES[number]) {
    for (let radius = 0; radius <= 14; radius += 2) for (let direction = 0; direction < (radius ? 12 : 1); direction++) {
      const x = spec.x + Math.cos(direction * Math.PI / 6) * radius, z = spec.z + Math.sin(direction * Math.PI / 6) * radius;
      let nearest = roads[0], distance = Infinity;
      for (const road of roads) {
        const squared = (road.point.x - x) ** 2 + (road.point.z - z) ** 2;
        if (squared < distance) { nearest = road; distance = squared; }
      }
      const angle = Math.atan2(x - nearest.point.x, z - nearest.point.z);
      const hw = Math.abs(Math.cos(angle)) * spec.w / 2 + Math.abs(Math.sin(angle)) * spec.d / 2;
      const hd = Math.abs(Math.sin(angle)) * spec.w / 2 + Math.abs(Math.cos(angle)) * spec.d / 2;
      const bounds = { x0: x - hw, x1: x + hw, z0: z - hd, z1: z + hd };
      if (bounds.x0 < -78 || bounds.x1 > 148 || bounds.z0 < -77 || bounds.z1 > 163 || occupancy.some(b => intersects(bounds, b))) continue;
      if (roads.some(({ point, clear }) => point.x > bounds.x0 - clear && point.x < bounds.x1 + clear && point.z > bounds.z0 - clear && point.z < bounds.z1 + clear)) continue;
      occupancy.push(bounds); return { x, z, angle, bounds };
    }
    return null;
  }

  // Original lettering, a folded street map and the cinema poster occupy distinct atlas tiles.
  const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1024;
  const ctx = canvas.getContext('2d')!;
  STREET_STORIES.forEach((spec, id) => {
    const x = (id % 4) * 256, y = Math.floor(id / 4) * 256;
    ctx.fillStyle = ['#324c56', '#775c4e', '#4d665a', '#734e56'][id % 4]; ctx.fillRect(x, y, 256, 256);
    ctx.strokeStyle = '#ceae79'; ctx.lineWidth = 3; ctx.strokeRect(x + 10, y + 10, 236, 236);
    ctx.fillStyle = '#f2e3c6'; ctx.textAlign = 'center'; ctx.textBaseline = 'middle'; ctx.font = '600 32px "Microsoft YaHei", serif';
    ctx.fillText(spec.title, x + 128, y + 88, 226); ctx.font = '11px Georgia,serif'; ctx.fillText(spec.sub, x + 128, y + 138, 230);
    ctx.font = '12px monospace'; ctx.fillText(`STREET NOTE ${String(id + 1).padStart(2, '0')}`, x + 128, y + 198);
  });
  // Tile 12: contour map, waterways, a route and a hand-drawn compass.
  ctx.fillStyle = '#e7d7b4'; ctx.fillRect(0, 768, 256, 256); ctx.strokeStyle = '#a89673'; ctx.lineWidth = 1;
  for (let n = 0; n < 8; n++) { ctx.beginPath(); ctx.ellipse(48 + n * 9, 884, 25 + n * 16, 17 + n * 12, -.3, 0, Math.PI * 2); ctx.stroke(); }
  ctx.strokeStyle = '#719ca8'; ctx.lineWidth = 8; ctx.beginPath(); ctx.moveTo(4, 804); ctx.bezierCurveTo(78, 890, 159, 793, 251, 870); ctx.stroke();
  ctx.strokeStyle = '#ad5d4c'; ctx.lineWidth = 3; ctx.setLineDash([5, 3]); ctx.beginPath(); ctx.moveTo(27, 990); ctx.lineTo(98, 918); ctx.lineTo(191, 952); ctx.lineTo(219, 872); ctx.stroke(); ctx.setLineDash([]);
  ctx.fillStyle = '#344e58'; ctx.font = 'bold 16px Georgia'; ctx.fillText('N', 213, 805); ctx.beginPath(); ctx.moveTo(213, 814); ctx.lineTo(203, 842); ctx.lineTo(213, 835); ctx.lineTo(223, 842); ctx.closePath(); ctx.fill();
  // Tile 13: a one-off travel-film poster, moon, tram and illustrated hills.
  ctx.fillStyle = '#182f4c'; ctx.fillRect(256, 768, 256, 256); ctx.fillStyle = '#e6cf90'; ctx.beginPath(); ctx.arc(435, 821, 31, 0, Math.PI * 2); ctx.fill();
  for (let n = 0; n < 32; n++) { ctx.fillStyle = '#c4d4d6'; ctx.fillRect(267 + ((n * 71) % 230), 781 + ((n * 43) % 116), 2, 2); }
  ctx.fillStyle = '#406369'; ctx.beginPath(); ctx.moveTo(256, 959); ctx.lineTo(321, 879); ctx.lineTo(372, 939); ctx.lineTo(443, 874); ctx.lineTo(512, 962); ctx.closePath(); ctx.fill();
  ctx.fillStyle = '#d3a16c'; ctx.fillRect(318, 919, 116, 45); ctx.fillStyle = '#f5dc99'; for (let n = 0; n < 5; n++) ctx.fillRect(325 + n * 21, 925, 15, 18);
  ctx.fillStyle = '#eee3c2'; ctx.font = 'bold 17px Georgia'; ctx.fillText('THE MOON EXPRESS', 384, 990, 234);
  const atlas = new THREE.CanvasTexture(canvas); atlas.colorSpace = THREE.SRGBColorSpace; atlas.anisotropy = 4;
  const atlasMaterial = new THREE.MeshStandardMaterial({ map: atlas, roughness: .8 }); materials.add(atlasMaterial);
  atlasMaterial.emissiveMap = atlas; atlasMaterial.emissive.set('#ffffff'); atlasMaterial.emissiveIntensity = .03;

  STREET_STORIES.forEach((spec, id) => {
    const position = locate(spec);
    if (!position) { rejected.push(spec.id); return; }
    plan.push({ id: spec.id, title: spec.title, ...position });
    const group = new THREE.Group(); group.name = `street-story-${spec.id}`; group.userData.noMerge = true;
    group.userData.originalDesign = spec.id; group.position.set(position.x, 0, position.z); group.rotation.y = position.angle; root.add(group);
    function mesh(g: THREE.BufferGeometry, m: THREE.Material, x: number, y: number, z: number, a = 1, b = 1, c = 1, parent: THREE.Object3D = group) {
      const result = new THREE.Mesh(g, m); result.position.set(x, y, z); result.scale.set(a, b, c); result.castShadow = !m.transparent; result.receiveShadow = true; parent.add(result); return result;
    }
    const box = (x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) => mesh(cube, m, x, y, z, w, h, d);
    const ball = (x: number, y: number, z: number, r: number, m: THREE.Material) => mesh(sphere, m, x, y, z, r, r, r);
    const hoop = (x: number, y: number, z: number, r: number, m = M.copper) => mesh(torus, m, x, y, z, r, r, r);
    function rod(a: number[], b: number[], radius: number, m = M.ink) {
      const v = new THREE.Vector3(...b).sub(new THREE.Vector3(...a));
      const item = mesh(cylinder, m, a[0] + v.x / 2, a[1] + v.y / 2, a[2] + v.z / 2, radius, v.length(), radius);
      item.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), v.normalize()); return item;
    }
    function solid(x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) {
      const item = box(x, y, z, w, h, d, m); item.updateWorldMatrix(true, false);
      const bounds = new THREE.Box3().setFromObject(item);
      colliders.push({ source: `story-${spec.id}`, kind: 'wall', min: bounds.min, max: bounds.max }); return item;
    }
    function art(tile: number, x: number, y: number, z: number, w: number, h: number) {
      const g = own(new THREE.PlaneGeometry(w, h)), uv = g.getAttribute('uv');
      for (let n = 0; n < uv.count; n++) uv.setXY(n, (tile % 4 + uv.getX(n)) / 4, 1 - (Math.floor(tile / 4) + 1 - uv.getY(n)) / 4);
      const item = mesh(g, atlasMaterial, x, y, z); item.rotation.y = Math.PI; return item;
    }
    const pad = box(0, .035, 0, spec.w - .1, .07, spec.d - .1, id % 2 ? M.stone : M.cream);
    pad.updateWorldMatrix(true, false); const bounds = new THREE.Box3().setFromObject(pad);
    colliders.push({ source: `story-${spec.id}-floor`, kind: 'floor', min: bounds.min, max: bounds.max });
    switch (id) {
      case 0: {
        solid(-.25, .55, .1, 1.6, .96, 1.05, M.wood); box(-.25, 1.07, .1, 1.78, .11, 1.2, M.ink);
        for (const x of [-.85, .35]) { rod([x, 1.1, .1], [x, 2.17, .1], .048, M.copper); hoop(x, 1.8, -.2, .22); }
        box(-.25, 1.72, .1, 1.3, .13, .56, M.ink); box(-.25, 1.27, -.19, .96, .04, .59, M.white);
        for (let row = 0; row < 4; row++) for (let col = 0; col < 7; col++) box(-.61 + col * .12, 1.31, -.36 + row * .1, .07, .028, .06, M.ink);
        solid(1.15, .4, .45, .67, .65, .76, M.red); for (let n = 0; n < 5; n++) box(1.15, .76 + n * .035, .45, .58, .025, .62, M.cream);
        for (const x of [-1.6, 1.6]) rod([x, .08, .85], [x, 2.65, .85], .04, M.ink);
        box(0, 2.7, .42, 3.4, .07, 1.37, M.red).rotation.x = -.08;
        break;
      }
      case 1: {
        solid(0, .66, .27, .54, 1.2, .52, M.ink); rod([0, 1.1, .27], [0, 3.2, .27], .055, M.copper);
        for (const [x, y, r] of [[-.49, 1.82, .32], [.48, 2.1, .24]]) {
          const face = mesh(cylinder, M.white, x, y, .18, r, .05, r); face.rotation.x = Math.PI / 2; hoop(x, y, .14, r);
          for (let n = 0; n < 10; n++) { const a = n * Math.PI * .16 - Math.PI * .8; box(x + Math.sin(a) * r * .77, y + Math.cos(a) * r * .77, .145, .015, r * .16, .01, M.ink).rotation.z = -a; }
          rod([x, y, .1], [x + r * .46, y + r * .33, .1], .014, M.red);
        }
        for (let n = 0; n < 7; n++) box(-.2, 1.25 + n * .06, .27, .8, .03, .69, M.white);
        const wind = new THREE.Group(); wind.position.set(0, 3.17, .27); wind.userData.noMerge = true; group.add(wind);
        mesh(cube, M.copper, .42, 0, 0, 1.12, .025, .025, wind); mesh(own(new THREE.ConeGeometry(.19, .48, 3)), M.red, -.45, 0, 0, 1, 1, 1, wind).rotation.z = Math.PI / 2;
        moving.push({ object: wind, axis: 'y', speed: .23, initial: .1, sway: true });
        const gauge = mesh(cylinder, glass, .96, .73, .35, .16, 1.1, .16); gauge.castShadow = false;
        solid(.96, .13, .35, .48, .2, .48, M.stone); hoop(.96, 1.29, .35, .23).rotation.x = Math.PI / 2;
        break;
      }
      case 2: {
        solid(0, .56, .2, .77, .98, .77, M.stone); mesh(cylinder, M.copper, 0, 1.09, .2, 1.17, .11, 1.17);
        const planets = new THREE.Group(); planets.position.set(0, 1.22, .2); planets.userData.noMerge = true; group.add(planets);
        mesh(sphere, luminous, 0, .1, 0, .2, .2, .2, planets);
        for (let n = 0; n < 5; n++) {
          const r = .35 + n * .15, a = n * 2.1;
          const orbit = mesh(torus, M.copper, 0, .1 + n * .06, 0, r, r, r, planets); orbit.rotation.x = Math.PI / 2;
          mesh(sphere, [M.blue, M.red, M.green, M.yellow, M.white][n], Math.cos(a) * r, .1 + n * .06, Math.sin(a) * r, .06 + n * .014, .06 + n * .014, .06 + n * .014, planets);
        }
        moving.push({ object: planets, axis: 'y', speed: .08, initial: 0, sway: false });
        const scope = mesh(cylinder, M.cream, 1.18, 1.73, -.53, .12, .81, .12); scope.rotation.z = -.7;
        for (const dx of [-.24, .24]) rod([1.18, 1.42, -.53], [1.18 + dx, .08, -.53 + dx], .025, M.copper);
        break;
      }
      case 3: {
        solid(-.46, .2, .12, 2.4, .26, 1.85, M.wood);
        const body = ball(-.88, .86, .23, .39, M.copper); body.scale.multiply(new THREE.Vector3(.85, 1.4, .34));
        ball(-.88, 1.35, .23, .27, M.wood).scale.z *= .48; rod([-.88, .67, .23], [-.88, 2.37, .23], .04, M.ink);
        for (let n = 0; n < 4; n++) rod([-.94 + n * .034, .63, .08], [-.94 + n * .034, 2.3, .08], .004, M.cream);
        box(-.88, .92, .069, .21, .06, .035, M.cream); rod([-.88, .33, .23], [-.88, .12, .23], .018, M.ink);
        for (const y of [2.25, 2.34]) for (const side of [-1, 1]) { rod([-.88, y, .23], [-.88 + side * .085, y, .23], .009, M.copper); ball(-.88 + side * .09, y, .23, .024, M.copper); }
        solid(.55, .64, .25, .9, .9, .65, M.ink); for (let n = 0; n < 8; n++) box(.2 + n * .1, 1.14, .25, .085, .045, .59 - n * .035, n % 2 ? M.copper : M.yellow);
        for (let n = 0; n < 7; n++) mesh(cylinder, M.copper, .24 + n * .1, .89 + n * .012, -.13, .025, .36 - n * .025, .025);
        const music = art(id, .42, 1.66, -.19, .74, .55); music.rotation.x = -.23; rod([.42, .08, -.06], [.42, 1.56, -.06], .024);
        hoop(-.2, .47, -.75, .36, M.ink).rotation.x = Math.PI / 2;
        break;
      }
      case 4: {
        for (const x of [-1.08, 1.08]) for (const z of [-.61, .61]) rod([x, .08, z], [x * .83, 1.14, z * .8], .035, M.wood);
        solid(0, 1.13, 0, 2.45, .11, 1.62, M.wood);
        const map = art(12, -.24, 1.2, -.03, 1.65, 1.3); map.rotation.set(-Math.PI / 2, 0, 0);
        for (let n = 0; n < 3; n++) mesh(cylinder, M.cream, .98, 1.24 + n * .11, .36, .07, 1.1, .07).rotation.z = Math.PI / 2;
        hoop(.72, 1.23, -.48, .15).rotation.x = Math.PI / 2;
        box(-.89, 1.25, .51, .44, .09, .28, M.red).rotation.y = -.2;
        break;
      }
      case 5: {
        solid(0, .19, .4, 2.4, .25, .65, M.stone);
        for (const x of [-1.09, 1.09]) rod([x, .3, .4], [x, 2.75, .4], .045, M.copper);
        const arch = hoop(0, 2.13, .4, 1.1); arch.scale.y *= .56;
        for (let n = 0; n < 7; n++) { const x = -.9 + n * .3, h = 1.38 + .37 * Math.sin(n * .8); box(x, 1.28, .4, .25, h, .045, [M.blue, M.green, M.red, M.yellow, M.cream, M.copper, M.white][n]); rod([x, .6, .36], [x + .19, 1.9, .36], .009, M.copper); }
        const pane = box(0, 1.4, .34, 2.12, 1.9, .015, glass); pane.castShadow = false;
        solid(-.77, .39, -.82, .48, .62, .47, M.blue); box(-.77, .75, -.82, .39, .08, .38, M.white);
        break;
      }
      case 6: {
        for (const x of [-1.51, 1.51]) rod([x, .08, .94], [x, 2.68, .94], .05, M.ink);
        solid(0, 1.74, .94, 2.94, 1.81, .09, M.cream); art(13, 0, 1.74, .884, 2.7, 1.58);
        solid(-1.1, .55, -.61, .64, .96, .73, M.wood); box(-1.1, 1.12, -.61, .64, .29, .44, M.ink);
        for (const dy of [1.38, 1.68]) hoop(-1.1, dy, -.61, .2, M.copper);
        mesh(cylinder, M.copper, -1.1, 1.13, -.25, .1, .23, .1).rotation.x = Math.PI / 2;
        const beamMaterial = new THREE.MeshBasicMaterial({ color: '#d9cda9', transparent: true, opacity: .045, depthWrite: false, side: THREE.DoubleSide }); materials.add(beamMaterial);
        const beam = mesh(own(new THREE.ConeGeometry(.6, 1.7, 12, 1, true)), beamMaterial, -1.1, 1.19, .45); beam.rotation.x = -Math.PI / 2; nightOnly.push(beam);
        for (let n = 0; n < 3; n++) { solid(.05 + n * .61, .28, -.7, .49, .39, .47, [M.red, M.blue, M.green][n]); box(.05 + n * .61, .63, -.48, .49, .37, .045, M.wood); }
        break;
      }
      case 7: {
        solid(0, .5, .16, .55, .86, .55, M.stone); mesh(cylinder, M.stone, 0, .99, .16, 1.02, .15, 1.02);
        box(0, 1.073, .16, 1.26, .015, 1.26, M.wood);
        for (let n = 0; n < 9; n++) { box(-.56 + n * .14, 1.084, .16, .009, .005, 1.13, M.ink); box(0, 1.084, -.4 + n * .14, 1.13, .005, .009, M.ink); }
        for (const [x, z, color] of [[-.28, .16, M.ink], [0, .3, M.white], [.14, .02, M.ink], [-.14, -.12, M.white], [.28, .44, M.ink]] as const) ball(x, 1.107, z, .055, color).scale.y *= .4;
        for (const x of [-1.25, 1.25]) { solid(x, .3, .13, .5, .45, .62, M.copper); mesh(cylinder, M.wood, x, .56, .13, .35, .08, .35); }
        break;
      }
      case 8: {
        const stone = mesh(own(new THREE.DodecahedronGeometry(.77, 0)), M.stone, -.21, .96, .16, 1, 1.28, .85); stone.rotation.set(.2, -.32, .12);
        solid(-.21, .28, .16, 1.75, .43, 1.43, M.wood);
        for (let n = 0; n < 7; n++) { const chip = mesh(own(new THREE.TetrahedronGeometry(.07 + n * .012)), M.cream, -.9 + n * .26, .14, -.69 + Math.sin(n) * .17); chip.rotation.z = n * .6; }
        solid(1.12, .59, .25, .63, 1.04, .74, M.red); box(1.12, 1.15, .25, .76, .08, .84, M.wood);
        rod([.88, 1.23, -.02], [1.34, 1.23, .42], .024, M.copper); box(.88, 1.23, -.02, .16, .12, .34, M.ink).rotation.y = -.5;
        break;
      }
      case 9: {
        for (const x of [-1.24, 1.24]) rod([x, .07, .4], [x, 3.35, .4], .035, M.copper);
        rod([-1.24, 3.35, .4], [1.24, 3.35, .4], .035, M.copper);
        const mobile = new THREE.Group(); mobile.position.set(0, 2.55, .4); mobile.userData.noMerge = true; group.add(mobile);
        for (let n = 0; n < 5; n++) {
          const x = -.91 + n * .46;
          mesh(cylinder, M.copper, x, .37, 0, .006, .75 + n * .07, .006, mobile);
          const shape = new THREE.Shape(); shape.moveTo(-.18, 0); shape.lineTo(0, .27 + n * .025); shape.lineTo(.2, 0); shape.lineTo(0, -.33); shape.closePath();
          const paper = mesh(own(new THREE.ShapeGeometry(shape)), [M.red, M.cream, M.blue, M.yellow, M.green][n], x, -.16 - n * .08, 0, 1, 1, 1, mobile); paper.material = paper.material.clone(); (paper.material as THREE.Material).side = THREE.DoubleSide; materials.add(paper.material as THREE.Material);
        }
        moving.push({ object: mobile, axis: 'y', speed: .16, initial: -.3, sway: true });
        solid(0, .19, .4, 2.6, .25, .56, M.stone);
        break;
      }
      case 10: {
        solid(0, .28, .1, 1.26, .43, .96, M.stone);
        mesh(cylinder, M.copper, -.12, .63, .09, .51, .24, .51); mesh(cylinder, M.blue, -.12, .76, .09, .44, .015, .44);
        rod([.44, .32, .32], [.44, 1.52, .32], .045, M.copper); rod([.44, 1.52, .32], [-.1, 1.52, .32], .045, M.copper);
        rod([-.1, 1.52, .32], [-.1, 1.37, .32], .038, M.copper);
        const water = rod([-.1, 1.37, .32], [-.1, .79, .32], .012, M.blue); water.castShadow = false;
        box(.44, 1.25, .27, .14, .055, .13, M.yellow); solid(.86, .48, -.59, .38, .82, .38, M.green);
        hoop(.86, .93, -.59, .18).rotation.x = Math.PI / 2;
        break;
      }
      case 11: {
        solid(0, .8, .37, 2.6, 1.45, .68, M.wood); box(0, 1.58, .37, 2.8, .11, .84, M.green);
        for (let row = 0; row < 3; row++) for (let col = 0; col < 6; col++) {
          const x = -1.06 + col * .42, y = .33 + row * .4;
          box(x, y, -.002, .38, .35, .055, (row + col) % 3 ? M.cream : M.blue); rod([x - .06, y, -.05], [x + .06, y, -.05], .012, M.copper);
          box(x, y + .08, -.036, .18, .08, .012, M.white);
        }
        for (let n = 0; n < 4; n++) {
          const x = -.83 + n * .55;
          mesh(cylinder, glass, x, 1.79, .36, .14, .31, .14); mesh(cylinder, M.copper, x, 1.97, .36, .145, .035, .145);
          for (let seed = 0; seed < 13; seed++) ball(x + Math.sin(seed * 2.4 + n) * .075, 1.66 + Math.floor(seed / 4) * .028, .36 + Math.cos(seed * 2.4 + n) * .075, .016 + seed % 3 * .003, n % 2 ? M.green : M.yellow);
          box(x, 1.8, .211, .15, .09, .012, M.cream);
        }
        solid(-.85, .24, -.8, .62, .34, .54, M.red); box(-.85, .43, -.8, .48, .03, .4, M.wood);
        break;
      }
    }
    // Each story has a locally authored title plaque; it sits within its reserved footprint.
    rod([-spec.w / 2 + .2, .07, -spec.d / 2 + .35], [-spec.w / 2 + .2, 1.15, -spec.d / 2 + .35], .018, M.copper);
    const plaque = art(id, -spec.w / 2 + .43, 1.18, -spec.d / 2 + .31, .78, .52); plaque.rotation.x = -.15;
    const animated = moving.filter(item => item.object.parent === group);
    if (animated.length) {
      const dynamic = new THREE.Group(); dynamic.name = `${group.name}-moving`; dynamic.userData.noMerge = true;
      dynamic.position.copy(group.position); dynamic.rotation.copy(group.rotation); root.add(dynamic);
      for (const item of animated) dynamic.add(item.object);
    }
    mergeByMaterial(group); freezeStatic(group);
  });
  root.userData.plan = { installed: plan, rejected };
  return {
    colliders,
    setEnvironment(period: string) {
      luminous.emissiveIntensity = period === 'night' ? 1.1 : .18;
      atlasMaterial.emissiveIntensity = period === 'night' ? .3 : .03;
      nightOnly.forEach(object => { object.visible = period === 'night' || period === 'dusk'; });
    },
    update(time: number) {
      for (const item of moving) item.object.rotation[item.axis] = item.initial + (item.sway ? Math.sin(time * item.speed) * .6 : time * item.speed % (Math.PI * 2));
    },
    dispose() { root.removeFromParent(); root.traverse(object => { if (object instanceof THREE.Mesh) geometries.add(object.geometry); }); geometries.forEach(g => g.dispose()); materials.forEach(m => m.dispose()); atlas.dispose(); moving.length = 0; nightOnly.length = 0; },
  };
}
