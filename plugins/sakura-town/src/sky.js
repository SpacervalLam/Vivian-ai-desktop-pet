import * as THREE from 'three';

/* 大气与光照的单一所有者。
 * 三张图其实是同一条视轴的三种时间：图 1 暖橙逆光、图 2 蓝紫暮色、
 * 图 3 晴朗白天。改时段时天空、雾、曝光、窗光、点光、湿路面一起走，
 * 不允许各模块自己写一份全局状态。 */

const PRESETS = {
  golden: {
    label: '黄昏 · 暖橙逆光',
    top: '#6f9ec9', mid: '#b9b7cc', horizon: '#f7c9a3',
    fog: '#e0bda6', fogDensity: .00165,
    hemiSky: '#cfe0ee', hemiGround: '#8c6b52', hemi: .9,
    key: '#ffcf94', keyI: 2.5, keyPos: [-34, 19, 30],
    fill: '#8fb6dc', fillI: .55,
    ambient: '#6a5e58', ambientI: .28,
    exposure: 1.16, wet: false,
    sun: '#ffd9a8', sunI: .75,
    window: .55, point: .35, cloud: .85,
  },
  dusk: {
    label: '蓝紫暮色 · 雨后',
    top: '#1e2756', mid: '#5c4c85', horizon: '#c58b88',
    fog: '#4b4870', fogDensity: .0021,
    hemiSky: '#6a72b4', hemiGround: '#2a2634', hemi: .72,
    key: '#9a92d8', keyI: .8, keyPos: [-30, 16, 24],
    fill: '#5a7ab8', fillI: .42,
    ambient: '#3c3a58', ambientI: .42,
    exposure: 1.34, wet: true,
    sun: '#d29a9a', sunI: .18,
    window: 1.85, point: 1.0, cloud: .35,
  },
  day: {
    label: '晴天 · 通透',
    top: '#6ba9de', mid: '#b8d4e8', horizon: '#eef0e9',
    fog: '#cdd9df', fogDensity: .0014,
    hemiSky: '#dbeaf5', hemiGround: '#9a8c78', hemi: 1.05,
    key: '#fff3dc', keyI: 2.2, keyPos: [-30, 34, 26],
    fill: '#b7d3ea', fillI: .7,
    ambient: '#8c8f94', ambientI: .3,
    exposure: 1.08, wet: false,
    sun: '#fff6e2', sunI: .5,
    window: .18, point: .12, cloud: .55,
  },
};

const SKY_VERT = /* glsl */`
varying vec3 vWorld;
void main(){
  vWorld = (modelMatrix * vec4(position, 1.0)).xyz;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}`;

const SKY_FRAG = /* glsl */`
uniform vec3 topColor, midColor, horizonColor, sunColor;
uniform vec3 sunDir;
uniform float sunStrength, cloudAmount;
varying vec3 vWorld;
void main(){
  vec3 dir = normalize(vWorld);
  float h = dir.y;
  vec3 col = mix(horizonColor, topColor, smoothstep(-0.06, 0.82, h));
  // 中层色带压在 0.05..0.45，给暮色一段紫 / 给黄昏一段粉
  float band = smoothstep(0.01, 0.30, h) * (1.0 - smoothstep(0.34, 0.72, h));
  col = mix(col, midColor, band * 0.62);
  // 两层低频云带，只在中低空出现
  float n = sin(dir.x * 26.0 + dir.z * 11.0) * sin(dir.z * 19.0 - dir.x * 7.0)
          + 0.5 * sin(dir.x * 53.0 - dir.z * 31.0);
  float cloud = smoothstep(0.35, 1.25, n) * smoothstep(0.0, 0.20, h) * (1.0 - smoothstep(0.30, 0.66, h));
  col += vec3(0.14, 0.085, 0.075) * cloud * cloudAmount;
  // 低角度太阳 + 大范围辉光，做图 1 的逆光
  float sd = max(dot(dir, normalize(sunDir)), 0.0);
  col += sunColor * pow(sd, 46.0) * sunStrength * 1.4;
  col += sunColor * pow(sd, 5.0) * sunStrength * 0.20;
  gl_FragColor = vec4(col, 1.0);
  #include <tonemapping_fragment>
  #include <colorspace_fragment>
}`;

export function createAtmosphere(scene, renderer, M) {
  const uniforms = {
    topColor:     { value: new THREE.Color() },
    midColor:     { value: new THREE.Color() },
    horizonColor: { value: new THREE.Color() },
    sunColor:     { value: new THREE.Color() },
    sunDir:       { value: new THREE.Vector3(-.55, .18, .62) },
    sunStrength:  { value: .6 },
    cloudAmount:  { value: .6 },
  };
  const skyMat = new THREE.ShaderMaterial({
    uniforms, vertexShader: SKY_VERT, fragmentShader: SKY_FRAG,
    side: THREE.BackSide, depthWrite: false, fog: false,
  });
  const sky = new THREE.Mesh(new THREE.SphereGeometry(900, 32, 20), skyMat);
  sky.name = 'sakura-sky';
  sky.frustumCulled = false;
  scene.add(sky);

  const hemi = new THREE.HemisphereLight('#ffffff', '#887766', 1);
  const key = new THREE.DirectionalLight('#ffffff', 2);
  key.castShadow = true;
  key.shadow.mapSize.set(2048, 2048);
  key.shadow.camera.left = -60; key.shadow.camera.right = 60;
  key.shadow.camera.top = 60;  key.shadow.camera.bottom = -60;
  key.shadow.camera.near = 1;  key.shadow.camera.far = 180;
  key.shadow.bias = -0.0004;
  key.shadow.normalBias = 0.02;
  key.target.position.set(0, 3, 0);
  const fill = new THREE.DirectionalLight('#aaccff', .5);
  fill.position.set(40, 26, -50);
  const ambient = new THREE.AmbientLight('#888888', .3);
  scene.add(hemi, key, key.target, fill, ambient);

  scene.fog = new THREE.FogExp2('#cccccc', .0018);

  /** 各模块把暖色点光 / 湿反光贴花登记进来，时段切换时统一调强度。 */
  const registered = [];
  function register(object, opts = {}) {
    registered.push({ object, opts });
    return object;
  }

  let current = 'golden';
  function setTime(name) {
    const p = PRESETS[name] || PRESETS.golden;
    current = PRESETS[name] ? name : 'golden';
    uniforms.topColor.value.set(p.top);
    uniforms.midColor.value.set(p.mid);
    uniforms.horizonColor.value.set(p.horizon);
    uniforms.sunColor.value.set(p.sun);
    uniforms.sunStrength.value = p.sunI;
    uniforms.cloudAmount.value = p.cloud;
    uniforms.sunDir.value.set(-p.keyPos[0], -p.keyPos[1] * .55, -p.keyPos[2]).normalize();

    hemi.color.set(p.hemiSky); hemi.groundColor.set(p.hemiGround); hemi.intensity = p.hemi;
    key.color.set(p.key); key.intensity = p.keyI; key.position.set(...p.keyPos);
    fill.color.set(p.fill); fill.intensity = p.fillI;
    ambient.color.set(p.ambient); ambient.intensity = p.ambientI;
    scene.fog.color.set(p.fog); scene.fog.density = p.fogDensity;
    renderer.toneMappingExposure = p.exposure;
    M.setWet?.(p.wet);
    M.windowLit.emissiveIntensity = p.window;
    M.blossomA.emissiveIntensity = M.blossomB.emissiveIntensity = M.blossomC.emissiveIntensity =
      name === 'dusk' ? .34 : .14;

    for (const { object, opts: o } of registered) {
      if (object.isLight) object.intensity = (o[name] ?? o.base ?? 1) * p.point;
      else if (object.material) {
        object.visible = p.wet || !!o.always;
        if (object.material.opacity !== undefined) {
          object.material.opacity = (o.opacity ?? .85) * (p.wet ? 1 : (o.always ? .45 : 0));
        }
      }
    }
    return p;
  }

  setTime('golden');
  return { setTime, register, get current() { return current; }, presets: PRESETS, key, sky };
}
