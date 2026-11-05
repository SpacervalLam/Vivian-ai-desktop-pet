import * as THREE from 'three';

/* 桜町原型的全部表面语言。零外部资源：贴图一律 canvas 现画，材质一律 PBR。
 * 旧场景那种「所有表面在光下响应一样」的问题，根因是 Toon 材质没有
 * roughness / metalness；这里每类材质都显式给一组物理参数。 */

export const C = {
  plaster:    '#e9dfcd',
  plasterCool:'#d9d4ca',
  wood:       '#a97248',
  woodDark:   '#6b452c',
  woodFloor:  '#bd8654',
  tile:       '#3b444e',
  asphalt:    '#393a40',
  stone:      '#b9b2a4',
  metal:      '#2f3439',
  glass:      '#8fb2bd',
  sakura:     '#f8c9d9',
  sakuraLight:'#ffe6ee',
  leaf:       '#5f7d4e',
  warm:       '#ffb765',
};

function canvas(w, h) {
  const c = document.createElement('canvas');
  c.width = w; c.height = h;
  return [c, c.getContext('2d')];
}

function texture(c, repeat = [1, 1]) {
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.repeat.set(repeat[0], repeat[1]);
  t.anisotropy = 8;
  return t;
}

function rnd(seed) {
  let s = seed >>> 0;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
}

/** 细颗粒灰泥：暖米白底 + 低频云斑 + 高频砂点。 */
export function plasterTexture(base = C.plaster) {
  const [c, x] = canvas(256, 256);
  const r = rnd(7717);
  x.fillStyle = base; x.fillRect(0, 0, 256, 256);
  for (let i = 0; i < 90; i++) {
    const px = r() * 256, py = r() * 256, rad = 18 + r() * 54;
    const g = x.createRadialGradient(px, py, 0, px, py, rad);
    g.addColorStop(0, `rgba(${r() > .5 ? '255,246,230' : '150,138,120'},${.025 + r() * .045})`);
    g.addColorStop(1, 'rgba(0,0,0,0)');
    x.fillStyle = g; x.beginPath(); x.arc(px, py, rad, 0, 7); x.fill();
  }
  for (let i = 0; i < 9000; i++) {
    const v = r();
    x.fillStyle = v > .5 ? 'rgba(255,255,255,.055)' : 'rgba(90,80,66,.05)';
    x.fillRect(r() * 256, r() * 256, 1, 1);
  }
  return texture(c, [2, 2]);
}

/** 竖向木纹。木格栅与室内护墙板共用，靠 repeat 控制密度。 */
export function woodTexture(vertical = true, base = C.wood) {
  const [c, x] = canvas(256, 256);
  const r = rnd(3301);
  x.fillStyle = base; x.fillRect(0, 0, 256, 256);
  for (let i = 0; i < 240; i++) {
    const p = r() * 256;
    x.strokeStyle = `rgba(${r() > .55 ? '255,222,180' : '70,40,22'},${.03 + r() * .13})`;
    x.lineWidth = .4 + r() * 2.2;
    x.beginPath();
    if (vertical) { x.moveTo(p, 0); x.bezierCurveTo(p + (r() - .5) * 5, 85, p + (r() - .5) * 5, 170, p + (r() - .5) * 3, 256); }
    else { x.moveTo(0, p); x.bezierCurveTo(85, p + (r() - .5) * 5, 170, p + (r() - .5) * 5, 256, p); }
    x.stroke();
  }
  for (let i = 0; i < 5; i++) {
    const p = r() * 256;
    x.strokeStyle = 'rgba(64,38,22,.16)'; x.lineWidth = 1.2;
    x.beginPath();
    if (vertical) { x.moveTo(p, 0); x.lineTo(p, 256); } else { x.moveTo(0, p); x.lineTo(256, p); }
    x.stroke();
  }
  return texture(c, [1, 1]);
}

/** 和瓦：一排排半圆筒错缝，深灰蓝。 */
export function tileTexture() {
  const [c, x] = canvas(256, 256);
  const r = rnd(9182);
  x.fillStyle = '#2e3740'; x.fillRect(0, 0, 256, 256);
  const rows = 8, cols = 10, rh = 256 / rows, cw = 256 / cols;
  for (let j = 0; j < rows; j++) {
    for (let i = -1; i <= cols; i++) {
      const px = i * cw + (j % 2 ? cw / 2 : 0), py = j * rh;
      const g = x.createLinearGradient(px, 0, px + cw, 0);
      g.addColorStop(0, '#242c34'); g.addColorStop(.42, '#4b5661');
      g.addColorStop(.75, '#3a444e'); g.addColorStop(1, '#212830');
      x.fillStyle = g;
      x.beginPath(); x.roundRect(px + 1, py + 1, cw - 2, rh - 2, [cw * .42, cw * .42, 2, 2]); x.fill();
      x.strokeStyle = 'rgba(12,16,20,.5)'; x.lineWidth = 1;
      x.beginPath(); x.moveTo(px + cw - 1, py); x.lineTo(px + cw - 1, py + rh); x.stroke();
    }
  }
  for (let i = 0; i < 2600; i++) {
    x.fillStyle = r() > .5 ? 'rgba(255,255,255,.03)' : 'rgba(0,0,0,.06)';
    x.fillRect(r() * 256, r() * 256, 1, 1);
  }
  return texture(c, [1, 1]);
}

/** 沥青：细骨料 + 补丁 + 零星落瓣。 */
export function asphaltTexture() {
  const [c, x] = canvas(512, 512);
  const r = rnd(4421);
  x.fillStyle = C.asphalt; x.fillRect(0, 0, 512, 512);
  for (let i = 0; i < 80; i++) {
    x.fillStyle = `rgba(${r() > .5 ? '255,250,240' : '15,17,22'},${.012 + r() * .02})`;
    x.beginPath(); x.ellipse(r() * 512, r() * 512, 12 + r() * 60, 8 + r() * 40, r() * 3, 0, 7); x.fill();
  }
  for (let i = 0; i < 26000; i++) {
    const v = r();
    x.fillStyle = v > .5 ? 'rgba(255,255,255,.05)' : 'rgba(0,0,0,.08)';
    x.fillRect(r() * 512, r() * 512, 1, 1);
  }
  for (let i = 0; i < 130; i++) {
    x.fillStyle = `rgba(248,201,217,${.25 + r() * .4})`;
    x.save(); x.translate(r() * 512, r() * 512); x.rotate(r() * 6.28);
    x.beginPath(); x.ellipse(0, 0, 1.5 + r() * 2.5, .8 + r() * 1.2, 0, 0, 7); x.fill(); x.restore();
  }
  return texture(c, [3, 14]);
}

/** 湿石板（图 2 入口铺装）：大块石 + 缝 + 水光。 */
export function stonePavingTexture() {
  const [c, x] = canvas(512, 512);
  const r = rnd(8827);
  x.fillStyle = '#6e6e72'; x.fillRect(0, 0, 512, 512);
  const n = 6, s = 512 / n;
  for (let j = 0; j < n; j++) for (let i = 0; i < n; i++) {
    const v = r();
    x.fillStyle = `rgb(${104 + v * 26 | 0},${102 + v * 25 | 0},${104 + v * 24 | 0})`;
    x.fillRect(i * s + 2, j * s + 2, s - 4, s - 4);
    x.strokeStyle = 'rgba(255,255,255,.045)'; x.lineWidth = 1;
    x.strokeRect(i * s + 2.5, j * s + 2.5, s - 5, s - 5);
  }
  for (let i = 0; i < 14000; i++) {
    x.fillStyle = r() > .5 ? 'rgba(255,255,255,.035)' : 'rgba(0,0,0,.055)';
    x.fillRect(r() * 512, r() * 512, 1, 1);
  }
  return texture(c, [2, 2]);
}

/** 亮着灯的窗：暖橙渐变 + 家具/植物剪影 + 窗帘。夜景靠 emissiveMap 发光。 */
export function litWindowTexture() {
  const [c, x] = canvas(128, 192);
  const r = rnd(2291);
  const g = x.createLinearGradient(0, 0, 0, 192);
  g.addColorStop(0, '#ffe0a8'); g.addColorStop(.52, '#f0b46a'); g.addColorStop(1, '#9a6238');
  x.fillStyle = g; x.fillRect(0, 0, 128, 192);
  x.fillStyle = 'rgba(60,38,24,.5)';
  x.fillRect(0, 150, 128, 42);
  x.fillRect(12, 118, 26, 34);
  x.fillStyle = 'rgba(52,66,44,.45)';
  for (let i = 0; i < 9; i++) {
    x.beginPath(); x.ellipse(96 + (r() - .5) * 22, 122 + (r() - .5) * 34, 5 + r() * 8, 8 + r() * 12, r(), 0, 7); x.fill();
  }
  x.fillStyle = 'rgba(255,246,226,.16)';
  x.fillRect(0, 0, 11, 192); x.fillRect(117, 0, 11, 192);
  x.fillStyle = 'rgba(40,28,20,.28)'; x.fillRect(0, 0, 128, 5);
  return texture(c);
}

/** 招牌：木底 + 竖排/横排假名汉字，交给 canvas 字体。 */
export function signTexture(text, opts = {}) {
  const [c, x] = canvas(512, 128);
  x.fillStyle = opts.bg || '#3d2c20'; x.fillRect(0, 0, 512, 128);
  const g = x.createLinearGradient(0, 0, 0, 128);
  g.addColorStop(0, 'rgba(255,220,170,.14)'); g.addColorStop(1, 'rgba(0,0,0,.2)');
  x.fillStyle = g; x.fillRect(0, 0, 512, 128);
  x.strokeStyle = opts.border || '#c9a273'; x.lineWidth = 3; x.strokeRect(7, 7, 498, 114);
  x.fillStyle = opts.fg || '#f4e3c4';
  x.font = `600 ${opts.size || 62}px "Yu Mincho","MS Mincho","Songti SC",serif`;
  x.textAlign = 'center'; x.textBaseline = 'middle';
  x.fillText(text, 256, 68);
  return texture(c);
}

/** 湿路面的灯影：一条竖向拉长的暖光带，additive 贴花用。 */
export function glowTexture() {
  const [c, x] = canvas(128, 256);
  const g = x.createRadialGradient(64, 128, 0, 64, 128, 120);
  g.addColorStop(0, 'rgba(255,196,120,.95)');
  g.addColorStop(.28, 'rgba(255,168,92,.42)');
  g.addColorStop(.65, 'rgba(255,150,80,.1)');
  g.addColorStop(1, 'rgba(255,140,70,0)');
  x.fillStyle = g; x.fillRect(0, 0, 128, 256);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

/** 单瓣樱花：带 alpha 的软边椭圆，落樱 Points 用。 */
export function petalTexture() {
  const [c, x] = canvas(64, 64);
  const g = x.createRadialGradient(32, 30, 2, 32, 32, 28);
  g.addColorStop(0, 'rgba(255,240,246,1)');
  g.addColorStop(.55, 'rgba(250,196,216,.95)');
  g.addColorStop(1, 'rgba(246,178,204,0)');
  x.fillStyle = g;
  x.save(); x.translate(32, 32); x.scale(1, .68); x.beginPath(); x.arc(0, 0, 27, 0, 7); x.fill(); x.restore();
  const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

/* ---------------------------------------------------------------- 材质库 */
const mat = (name, params) => {
  const m = new THREE.MeshStandardMaterial(params);
  m.name = name;
  return m;
};

export function createMaterials() {
  const plaster = plasterTexture();
  const plasterCool = plasterTexture(C.plasterCool);
  const woodV = woodTexture(true);
  const woodH = woodTexture(false, C.woodFloor);
  const tiles = tileTexture();
  const road = asphaltTexture();
  const paving = stonePavingTexture();
  const win = litWindowTexture();

  const M = {
    plaster:    mat('plaster',    { color: '#fffaf0', map: plaster, roughness: .94 }),
    plasterSide:mat('plasterSide',{ color: '#efe7d8', map: plasterCool, roughness: .95 }),
    concrete:   mat('concrete',   { color: '#cfc7b8', roughness: .88 }),
    stone:      mat('stone',      { color: C.stone, roughness: .93, map: paving }),
    wood:       mat('wood',       { color: '#f2d7b6', map: woodV, roughness: .68 }),
    woodDark:   mat('woodDark',   { color: '#8d6444', roughness: .74 }),
    floor:      mat('floor',      { color: '#f0dcc0', map: woodH, roughness: .52 }),
    tile:       mat('tile',       { color: '#cfd6dd', map: tiles, roughness: .82 }),
    asphalt:    mat('asphalt',    { color: '#ffffff', map: road, roughness: .78, metalness: .04 }),
    metal:      mat('metal',      { color: C.metal, roughness: .42, metalness: .72 }),
    blackMetal: mat('blackMetal', { color: '#22262a', roughness: .5, metalness: .55 }),
    glass:      mat('glass',      { color: '#b9d3d8', roughness: .08, metalness: .12, transparent: true, opacity: .3, depthWrite: false, side: THREE.DoubleSide }),
    windowLit:  mat('windowLit',  { color: '#8a6a4a', map: win, emissive: '#ffb765', emissiveMap: win, emissiveIntensity: .95, roughness: .28 }),
    windowDark: mat('windowDark', { color: '#28313a', roughness: .15, metalness: .35 }),
    curtain:    mat('curtain',    { color: '#eadfc9', roughness: .96, side: THREE.DoubleSide, transparent: true, opacity: .94 }),
    plasterWarm:mat('plasterWarm',{ color: '#f6e6cc', roughness: .95 }),
    leaf:       mat('leaf',       { color: C.leaf, roughness: .92, side: THREE.DoubleSide }),
    bark:       mat('bark',       { color: '#5b4c46', roughness: .98 }),
    trunkDark:  mat('trunkDark',  { color: '#3d3432', roughness: 1 }),
    blossomA:   mat('blossomA',   { color: '#ffe3ec', roughness: .9, emissive: '#f7b7ce', emissiveIntensity: .16 }),
    blossomB:   mat('blossomB',   { color: '#f9c4d6', roughness: .9, emissive: '#f2a6c2', emissiveIntensity: .14 }),
    blossomC:   mat('blossomC',   { color: '#fff2f5', roughness: .9, emissive: '#ffd6e3', emissiveIntensity: .14 }),
    pot:        mat('pot',        { color: '#b9a68f', roughness: .9 }),
    soil:       mat('soil',       { color: '#4a3c31', roughness: 1 }),
    flowerPink: mat('flowerPink', { color: '#e88fae', roughness: .88 }),
    flowerWhite:mat('flowerWhite',{ color: '#f3eae0', roughness: .88 }),
    shrub:      mat('shrub',      { color: '#54734d', roughness: .95 }),
    cherryRed:  mat('cherryRed',  { color: '#c8453f', roughness: .55, metalness: .35 }),
    train:      mat('train',      { color: '#8fae86', roughness: .5, metalness: .3 }),
    tower:      mat('tower',      { color: '#c8ccd2', roughness: .55, metalness: .35 }),
    towerLit:   mat('towerLit',   { color: '#8fa5c8', roughness: .35, emissive: '#ffd08a', emissiveIntensity: .5 }),
  };
  // 夜景把路面变成镜面；白天还原。main.js 的 setTime 调这两个方法。
  M.setWet = (wet) => {
    M.asphalt.roughness = wet ? .13 : .78;
    M.asphalt.metalness = wet ? .62 : .04;
    M.stone.roughness   = wet ? .18 : .93;
    M.stone.metalness   = wet ? .35 : 0;
    M.asphalt.needsUpdate = M.stone.needsUpdate = true;
  };
  return M;
}

/** 小工具：一次性盒子，避免到处 new BoxGeometry。 */
const boxCache = new Map();
export function boxGeo(w, h, d) {
  const k = `${w.toFixed(3)}/${h.toFixed(3)}/${d.toFixed(3)}`;
  let g = boxCache.get(k);
  if (!g) { g = new THREE.BoxGeometry(w, h, d); boxCache.set(k, g); }
  return g;
}

export function box(parent, M, name, x, y, z, w, h, d, material, opts = {}) {
  const m = new THREE.Mesh(boxGeo(w, h, d), material);
  m.name = name;
  m.position.set(x, y, z);
  m.castShadow = opts.cast !== false;
  m.receiveShadow = opts.receive !== false;
  if (opts.rotY) m.rotation.y = opts.rotY;
  parent.add(m);
  return m;
}
