import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { createMaterials } from './src/palette.js';
import { createAtmosphere } from './src/sky.js';
import { buildBuildings } from './src/buildings.js';
import { buildStreet } from './src/street.js';
import { buildNature } from './src/nature.js';
import { buildSkyline } from './src/skyline.js';
import { buildInterior } from './src/interior.js';

/* 桜町 · Tokyo Bloom —— 从三张概念图反推的独立街区原型。
 * 与 plugins/3d-apartment 完全隔离：不 import 任何旧模块、不复用任何旧素材、
 * 不用旧坐标系。全部几何与贴图在本目录内程序化生成。 */

const renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: 'high-performance' });
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 1.5));
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.outputColorSpace = THREE.SRGBColorSpace;
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.16;
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFSoftShadowMap;
document.body.appendChild(renderer.domElement);

const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(58, window.innerWidth / window.innerHeight, .05, 1400);
const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.dampingFactor = .075;
controls.maxPolarAngle = Math.PI / 2 - .015;
controls.minDistance = .4;
controls.maxDistance = 400;

const M = createMaterials();
const atmo = createAtmosphere(scene, renderer, M);
buildSkyline(scene, M, atmo);
buildBuildings(scene, M, atmo);
const street = buildStreet(scene, M, atmo);
buildNature(scene, M);
buildInterior(scene, M, atmo);

/* 四个站位对应三张图 + 一个总览。切机位同时切到那张图的时间。 */
const VIEWS = {
  // 图 1 站在横街路口斜看公寓：7m 宽的巷子里退不出能看全 12m 楼体的距离。
  panorama: { label: '① 巷口全景', hint: '图 1：低层街屋 / 木格栅公寓 / 东京天际线三层',
    pos: [-8, 2.2, 20], target: [7, 6.5, 1], fov: 58, time: 'golden' },
  entrance: { label: '② 入口夜景', hint: '图 2：樱花树影、雨后湿路反射、暖光门廊、晴空塔',
    pos: [-2.5, 1.75, 8], target: [3.5, 2.4, 0], fov: 64, time: 'dusk' },
  // 图 3 正对落地玻璃：视线几乎沿 -X，才能同时框住两把藤椅、小圆桌与天际线。
  balcony:  { label: '③ 室内阳台', hint: '图 3：木地板 → 落地玻璃 → 阳台 → 街巷 → 天际线',
    pos: [7.2, 7.75, 5.5], target: [-4, 7.1, 6.5], fov: 64, time: 'day' },
  overview: { label: '④ 俯瞰街区', hint: '整体布局与巷宽核对',
    pos: [46, 40, 56], target: [0, 3, -6], fov: 54, time: 'golden' },
};

let currentView = 'panorama';
function setView(name, { withTime = true } = {}) {
  const v = VIEWS[name] || VIEWS.panorama;
  currentView = VIEWS[name] ? name : 'panorama';
  camera.fov = v.fov;
  camera.updateProjectionMatrix();
  camera.position.set(...v.pos);
  controls.target.set(...v.target);
  controls.update();
  if (withTime) setTime(v.time);
  renderHud();
  const tip = document.getElementById('tip');
  if (tip) tip.innerHTML = `<b>${v.label}</b><br>${v.hint}`;
}
function setTime(name) { atmo.setTime(name); renderHud(); }

/* ------------------------------------------------------------------ HUD */
const viewsEl = document.getElementById('views');
const timesEl = document.getElementById('times');
for (const [key, v] of Object.entries(VIEWS)) {
  const btn = document.createElement('button');
  btn.textContent = v.label; btn.dataset.view = key;
  btn.onclick = () => setView(key);
  viewsEl.appendChild(btn);
}
for (const [key, p] of Object.entries(atmo.presets)) {
  const btn = document.createElement('button');
  btn.textContent = p.label.split(' · ')[0]; btn.dataset.time = key;
  btn.onclick = () => setTime(key);
  timesEl.appendChild(btn);
}
function renderHud() {
  viewsEl.querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.view === currentView));
  timesEl.querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.time === atmo.current));
}
window.addEventListener('keydown', e => {
  const keys = ['panorama', 'entrance', 'balcony', 'overview'];
  if (e.key >= '1' && e.key <= '4') setView(keys[+e.key - 1]);
});
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
});

setView('panorama');
const loading = document.getElementById('loading');
loading.style.opacity = '0';
setTimeout(() => loading.remove(), 700);

/* CDP 取证接口：固定机位、切时段、读渲染统计。 */
window.__SAKURA__ = {
  scene, camera, controls, renderer, THREE, M, atmo,
  setView, setTime, views: VIEWS,
  get stats() {
    let meshes = 0, tris = 0;
    scene.traverse(o => {
      if (!o.isMesh && !o.isPoints) return;
      meshes++;
      const g = o.geometry;
      const n = g?.index ? g.index.count / 3 : (g?.attributes?.position?.count || 0) / 3;
      tris += n * (o.isInstancedMesh ? o.count : 1);
    });
    return { meshes, tris: Math.round(tris), calls: renderer.info.render.calls,
      drawnTris: renderer.info.render.triangles, frame: renderer.info.render.frame,
      fog: scene.fog?.density, view: currentView, time: atmo.current };
  },
};

let last = performance.now();
function animate(now) {
  requestAnimationFrame(animate);
  const dt = Math.min((now - last) / 1000, .05);
  last = now;
  controls.update();
  street.update(dt);
  renderer.render(scene, camera);
}
requestAnimationFrame(animate);
window.__SAKURA_READY__ = true;
