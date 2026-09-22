import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';

/** Architectural renovation. Coordinates deliberately retain the navigable floor/door contract. */
export function rebuildApartmentArchitecture(group: THREE.Group) {
  const materials = {
    plaster: new THREE.MeshStandardMaterial({color:'#b9b4a5',roughness:.92}),
    clay: new THREE.MeshStandardMaterial({color:'#956e59',roughness:.88}),
    stone: new THREE.MeshStandardMaterial({color:'#646e69',roughness:.93}),
    coping: new THREE.MeshStandardMaterial({color:'#d1cbbb',roughness:.78}),
    metal: new THREE.MeshStandardMaterial({color:'#354b48',roughness:.52,metalness:.35}),
    wood: new THREE.MeshStandardMaterial({color:'#9c7954',roughness:.79}),
    roof: new THREE.MeshStandardMaterial({color:'#495852',roughness:.94}),
    leaf: new THREE.MeshStandardMaterial({color:'#496b56',roughness:.96}),
  };
  Object.values(materials).forEach(m=>{m.userData.outlineWeight=0;});
  const geometry = new Map<string,THREE.BufferGeometry>();
  const newParts = new THREE.Group(); newParts.name='apartment-authored-architecture';
  newParts.userData.sceneCollideSkip=true;group.add(newParts);
  const put=(parent:THREE.Group,name:string,x:number,y:number,z:number,w:number,h:number,d:number,mat:THREE.Material,round=false)=>{
    const key=`${w}/${h}/${d}/${round}`;let geo=geometry.get(key);
    if(!geo){geo=round?new RoundedBoxGeometry(w,h,d,1,Math.min(.035,Math.min(w,h,d)*.15)):new THREE.BoxGeometry(w,h,d);geometry.set(key,geo);}
    const mesh=new THREE.Mesh(geo,mat);mesh.name=name;mesh.position.set(x,y,z);mesh.castShadow=true;mesh.receiveShadow=true;parent.add(mesh);return mesh;
  };
  // Replace rather than overlay the old geometry. Other objects may share its geometry/material.
  const removed:THREE.Object3D[]=[];
  for(const child of [...group.children]){
    if(child.name==='apt-roof'||child.name.startsWith('apt-old-roof-')||child.name.startsWith('apt-old-gable-')||child.name.startsWith('apt-old-entrance-')){group.remove(child);removed.push(child);}
    if(child.name.startsWith('railing-')&&child instanceof THREE.Group){
      const bounds=new THREE.Box3().setFromObject(child);const min=bounds.min,max=bounds.max;
      const x0=min.x+.028,x1=max.x-.028,z=(min.z+max.z)/2,F=min.y;
      removed.push(...child.children);child.clear();
      const width=x1-x0;
      put(child,'balcony-top-rail',(x0+x1)/2,F+1.05,z,width,.055,.095,materials.metal,true);
      put(child,'balcony-lower-rail',(x0+x1)/2,F+.10,z,width,.055,.065,materials.metal);
      const sections=Math.ceil(width/2.05),step=width/sections;
      for(let n=0;n<=sections;n++)put(child,'balcony-post',x0+n*step,F+.54,z,.065,1.06,.085,materials.metal);
      for(let n=0;n<sections;n++){
        const start=x0+n*step+.08,end=x0+(n+1)*step-.08;
        /**
         * 木格栅：**方盒，不倒角**。
         *
         * 原本走 `round=true`（RoundedBoxGeometry），单根 108 个三角形。可它是一根
         * 0.068×0.85×0.046 的细栅，倒角半径取 `min(.035, 0.046×.15) = 6.9mm`——
         * 只有料厚的 15%；而全楼 1266 根、间距 0.14m 挤成一道密屏，6.9mm 在任何
         * 还看得清它的距离上都已经小于一个像素。
         *
         * 实测这一项占 apartment-shell 的 35%（13.7 万三角形）、整场景的 11.6%，
         * 是单个合批 mesh 里最重的一个。换回方盒：1266×12 ≈ 1.5 万三角形，
         * 省下 12.2 万（整场景 10.4%），而画面逐像素无差。
         *
         * 其余 `round=true` 的构件（顶横档 / 山墙垛 / 屋顶压顶 / 前柱）不倒角半径
         * 更小但**单件大、数量少**，倒角在近景是读得出来的，保持不动。
         */
        for(let x=start+.035;x<end;x+=.14)put(child,'cedar-balustrade',x,F+.555,z,.068,.85,.046,materials.wood,false);
      }
    }
  }
  // Gables are newly segmented solids with recessed terracotta fields, not decals on old walls.
  for(const side of [-1,1]){
    put(newParts,'gable-base',side*31.075,7.64,-.6,.15,8.48,10.6,materials.clay);
    for(const z of [-5.7,-2.2,1.3,4.55])put(newParts,'gable-pier',side*31.20,7.695,z,.18,8.53,.22,materials.coping,true);
    for(const y of [3.48,6.28,9.08,11.88])put(newParts,'gable-course',side*31.31,y+.013,-.6,.14,.145,10.6,materials.coping);
    for(let z=-5.35;z<4.4;z+=.37)put(newParts,'gable-ceramic-rib',side*31.17,7.6,z,.035,7.8,.06,materials.clay);
  }
  // Five roof pavilions replace the single 62 m slab. Joints are real 6 cm gaps.
  for(let unit=0;unit<5;unit++){
    const x=-24.8+unit*12.4;
    put(newParts,'roof-slab',x,12.00,-.6,12.34,.40,10.9,materials.coping);
    put(newParts,'roof-inset',x,12.245,-.6,11.8,.07,10.34,materials.roof);
    for(const z of [-6.0,4.8]){
      put(newParts,'roof-parapet',x,12.55,z,12.28,.70,.18,materials.plaster);
      put(newParts,'roof-coping',x,12.93,z,12.36,.09,.26,materials.metal,true);
    }
    for(const dx of [-6.05,6.05])put(newParts,'roof-party-wall',x+dx,12.49,-.6,.16,.58,10.58,materials.clay);
    const plantHeight=unit%2===0?1.1:.75;
    put(newParts,'roof-service-room',x-2,12.28+plantHeight/2,-1.7,2.5,plantHeight,2.1,materials.stone,true);
    put(newParts,'roof-service-cap',x-2,12.32+plantHeight,-1.7,2.64,.08,2.24,materials.metal);
    for(let n=0;n<8;n++)put(newParts,'roof-vent',x-2,12.45+n*.075,-.63,2.1,.022,.035,materials.metal);
    put(newParts,'roof-planter',x+3.8,12.48,2.9,2.4,.4,.7,materials.clay,true);
    for(let n=0;n<9;n++){
      const leaf=new THREE.Mesh(new THREE.SphereGeometry(.26,8,6),materials.leaf);
      leaf.position.set(x+2.85+n*.24,12.80+(n%3)*.06,2.9);leaf.scale.set(1,1.4,.9);newParts.add(leaf);
    }
  }
  // Columns sit at party walls, clear of every window and the 203 entrance.
  for(const x of [-30.88,-18.6,-6.2,6.2,18.6,30.88]){
    put(newParts,'front-pier',x,7.6,6.39,.22,8.6,.22,materials.coping,true);
    put(newParts,'rear-pier',x,7.6,-7.26,.18,8.6,.18,materials.clay,true);
  }
  for(const F of [3.4,6.2,9.0]){
    for(let unit=0;unit<5;unit++){
      const x=-24.8+unit*12.4;
      // Fascia is outside the existing deck edge by 35 mm. No duplicate horizontal surface.
      put(newParts,'balcony-fascia',x,F-.115,6.405,12.15,.19,.14,materials.coping,true);
      put(newParts,'balcony-shadow-line',x,F-.25,6.405,12.06,.04,.09,materials.metal);
      put(newParts,'rear-corridor-fascia',x,F-.10,-7.27,12.15,.16,.10,materials.clay);
      // Narrow side privacy screens at the party wall, leaving the balcony passage clear.
      for(let n=0;n<6;n++)put(newParts,'balcony-privacy-fin',x-5.97,F+1.30,5.0+n*.19,.065,2.20,.08,materials.wood,true);
    }
  }
  /**
   * 4F 阳台顶板 —— 补上顶层阳台缺的天花板。
   *
   * 2F/3F 阳台的天花板不是单独做的，就是**上一层阳台的楼板**（exterior.ts 南立面
   * 循环里那 12.32×0.14×1.6 的盒子，顶面 = 该层标高 F）。4F 上面没有楼层了，屋顶
   * 亭子的板（roof-slab，z ∈ [-6.05, 4.85]）只铺到主体量 ZF=4.7 外 0.15m —— 整个
   * 1.6m 挑出段上方是空的，4F 阳台因此露天。实测（tmp/_probe-balcony-ceiling.mjs）
   * 阳台 7 个采样点里只有最里侧 z=4.75 有命中，而那一下打在滑門上方的墙垛
   * （F+2.38=11.38）上，z=5…6.25 全部无遮挡。
   *
   * 标高照抄下层：顶面 = FLOORS[2] + LVL = 11.8（正好压在 roof-slab 的底面上，
   * 两者法线相反、各自背面剔除，不构成共面），厚 0.14。4F 阳台净高于是也是 2.66m。
   *
   * 唯一与下层不同的是**宽度收 2cm**（12.28 而不是 12.32）：12.32 时顶板端面落在
   * x = ux0+0.04 上，与 4F 窗套壁柱（unitFace 的 trim：x 从 ux0+0.04 起、0.13 宽、
   * y 到 11.744）的外侧面**同坐标、同法线**，于是在全楼 10 处户境壁柱上各多出
   * 83 cm² 的真共面（_zfight.mjs 实测：59 → 69 组）。收 2cm 后两者相距 20mm，
   * 落在检测器的 2mm 近距带之外；2cm 的收边在 11.7m 高处读不出来。
   *
   * 试过的另外两个标高都被否掉：顶面取 11.90（与前山墙柱 front-pier 柱顶齐平）会让
   * 板顶与柱顶的 AABB 面重合，_apt-surfaces.mjs 判 10 组贴面冲突；底面取 11.75 则又
   * 压回壁柱顶（11.744）以下 1cm，共面只是变小、没有消失。
   */
  const CEIL_TOP = 11.8, CEIL_TH = 0.14;   // 顶面标高 / 板厚（与各层楼板同）
  for(let unit=0;unit<5;unit++){
    const x=-24.8+unit*12.4;
    put(newParts,'balcony-top-slab',x,CEIL_TOP-CEIL_TH/2,5.5,12.28,CEIL_TH,1.6,materials.coping);
    // 外缘边梁 + 阴影线：与上面各层同规格，标高换到顶板的顶面
    put(newParts,'balcony-fascia',x,CEIL_TOP-.115,6.405,12.15,.19,.14,materials.coping,true);
    put(newParts,'balcony-shadow-line',x,CEIL_TOP-.25,6.405,12.06,.04,.09,materials.metal);
    /* 北侧 4F 外廊顶板（板体在 `exterior.ts` 北立面循环里，见那里的注释）的外缘压边。
     * 与上面 `rear-corridor-fascia` 同规格、同 z，标高换到 11.8 —— 相当于"5F 的廊板"。
     * 没有它，顶层外廊外缘会是一道光秃秃的混凝土边，而 2F/3F/4F 都有一道陶土色压边。 */
    put(newParts,'rear-corridor-fascia',x,CEIL_TOP-.10,-7.27,12.15,.16,.10,materials.clay);
  }
  // Materials are private to this instance; never modify module-level toon caches.
  const replacements=new Map<THREE.Material,THREE.Material>();
  const palette=new Map([['98a3ae',materials.plaster],['8b96a5',materials.clay],['a9b2bb',materials.coping],['8b95a0',materials.stone],['6b7481',materials.stone],['5e6878',materials.metal],['3a424c',materials.metal],['a3aab2',materials.coping],['8b929b',materials.stone],['59626e',materials.clay]]);
  group.traverse(o=>{if(!(o instanceof THREE.Mesh))return;const remap=(m:THREE.Material)=>{
    if(!(m instanceof THREE.MeshToonMaterial))return m;
    if(replacements.has(m))return replacements.get(m)!;
    const target=!m.map?palette.get(m.color.getHexString()):undefined;
    if(target){replacements.set(m,target);return target;}return m;
  };o.material=Array.isArray(o.material)?o.material.map(remap):remap(o.material);});
  // Detached unique resources must not leak; shared resources still in the scene remain alive.
  const retainedG=new Set<THREE.BufferGeometry>();const retainedM=new Set<THREE.Material>();
  group.traverse(o=>{if(o instanceof THREE.Mesh){retainedG.add(o.geometry);(Array.isArray(o.material)?o.material:[o.material]).forEach(m=>retainedM.add(m));}});
  const discardedG=new Set<THREE.BufferGeometry>();const discardedM=new Set<THREE.Material>();
  removed.forEach(o=>o.traverse(n=>{if(n instanceof THREE.Mesh){if(!retainedG.has(n.geometry))discardedG.add(n.geometry);(Array.isArray(n.material)?n.material:[n.material]).forEach(m=>{if(!retainedM.has(m)&&!m.userData.__roomCached)discardedM.add(m);});}}));
  discardedG.forEach(g=>g.dispose());discardedM.forEach(m=>m.dispose());
  group.userData.architectureVersion=2;
}
