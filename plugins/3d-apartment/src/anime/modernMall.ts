import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';
import { createLeafField, createTrunkField, plantTree, type TreeKit } from './foliage';

/** LUMINA: the existing 60 × 32 m site, with an open six-storey atrium.
 * The parking garage and the railway setbacks are owned by civicQuarter. */
export function createModernMall(scene: THREE.Scene) {
  const root = new THREE.Group();
  root.name = 'lumina-galleria';
  root.userData.sceneCollideSkip = true;
  scene.add(root);
  const colliders: Collider[] = [];
  const geometries = new Set<THREE.BufferGeometry>();
  const materials = new Set<THREE.Material>();
  const textures = new Set<THREE.Texture>();
  const geometry = <T extends THREE.BufferGeometry>(g: T) => { geometries.add(g); return g; };
  function mat(color: string, options: THREE.MeshStandardMaterialParameters = {}) {
    const m = new THREE.MeshStandardMaterial({ color, roughness: .6, ...options });
    m.userData.outlineWeight = 0; materials.add(m); return m;
  }
  const stone = mat('#e5dfd2'), ivory = mat('#f6eee0'), bronze = mat('#75634c', { metalness: .65, roughness: .3 });
  const ink = mat('#293632'), wood = mat('#ac805b'), earth = mat('#514739');
  const warm = mat('#ffdfaa', { emissive: '#ffcf85', emissiveIntensity: .5 });
  const display = mat('#e9d7b7', { emissive: '#ffe3b5', emissiveIntensity: .12 });
  const glass = mat('#bdd9df', { transparent: true, opacity: .15, depthWrite: false, roughness: .12, metalness: .12, side: THREE.DoubleSide });
  const water = mat('#719f9c', { roughness: .13, metalness: .3 });
  const fashion = ['#b77b61', '#597d7e', '#e9c9a2', '#52576a', '#bd9d77'].map(c => mat(c));
  const cube = geometry(new THREE.BoxGeometry(1, 1, 1));
  const tube = geometry(new THREE.CylinderGeometry(1, 1, 1, 10));
  const sphere = geometry(new THREE.SphereGeometry(1, 10, 7));
  function box(x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) {
    const mesh = new THREE.Mesh(cube, m); mesh.position.set(x, y, z); mesh.scale.set(w, h, d);
    mesh.castShadow = h > .15 && m !== glass; mesh.receiveShadow = m !== glass; root.add(mesh); return mesh;
  }
  function collision(name: string, x: number, y: number, z: number, w: number, h: number, d: number, kind: Collider['kind'] = 'wall') {
    colliders.push({ source: `lumina-${name}`, kind, min: new THREE.Vector3(x-w/2,y-h/2,z-d/2), max: new THREE.Vector3(x+w/2,y+h/2,z+d/2) });
  }
  function block(name: string, x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material, kind: Collider['kind'] = 'wall') {
    box(x,y,z,w,h,d,m); collision(name,x,y,z,w,h,d,kind);
  }
  function rod(a: number[], b: number[], radius: number, m: THREE.Material) {
    const from = new THREE.Vector3(...a), delta = new THREE.Vector3(...b).sub(from);
    const mesh = new THREE.Mesh(tube,m); mesh.position.copy(from.addScaledVector(delta,.5));
    mesh.scale.set(radius,delta.length(),radius); mesh.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),delta.normalize());
    mesh.castShadow = true; root.add(mesh); return mesh;
  }
  // A single sign atlas keeps the dozens of individual storefronts inexpensive.
  const names = ['L U M I N A', 'SHOP · DINE · DISCOVER', 'ATELIER / 01', 'MAISON & CO.', 'FORM STUDIO', 'BOTANICA', 'COMMON GROUND', 'THE EDIT', 'NOIR BEAUTY', 'PAPER & POETRY', 'TERRA HOME', 'STUDIO SPORT', 'SKY DINING', 'CINEMA SIX', 'WEST WALK', 'EAST GALLERY', 'FASHION  /  BEAUTY', 'DINING  /  CINEMA', 'WELCOME', 'CONCIERGE', '↑ TERRACE    •    CINEMA →', 'LUMINA / NEW SEASON', 'THE ART OF EVERYDAY', 'GARDEN LEVEL'];
  const atlas = document.createElement('canvas'); atlas.width = 2048; atlas.height = 2048;
  const ctx = atlas.getContext('2d')!;
  for (let i=0;i<names.length;i++) {
    const x=(i%4)*512,y=Math.floor(i/4)*256;
    ctx.fillStyle=i<2?'#e4ddcd':'#273a35'; ctx.fillRect(x,y,512,256);
    ctx.fillStyle=i<2?'#66533b':'#f6e9cf'; ctx.textAlign='center'; ctx.font=`${i===0?68:44}px Georgia, serif`;
    ctx.fillText(names[i],x+256,y+150,480);
    ctx.fillStyle='#b29a72';ctx.fillRect(x+204,y+186,104,2);
  }
  const atlasTexture = new THREE.CanvasTexture(atlas); atlasTexture.colorSpace=THREE.SRGBColorSpace; textures.add(atlasTexture);
  const lettering=mat('#ffffff',{map:atlasTexture,emissiveMap:atlasTexture,emissive:'#ffffff',emissiveIntensity:.2});
  function sign(index:number,x:number,y:number,z:number,w:number,h:number,angle=Math.PI) {
    const g=geometry(new THREE.PlaneGeometry(w,h)),uv=g.getAttribute('uv');
    const col=index%4,row=Math.floor(index/4);
    for(let i=0;i<uv.count;i++)uv.setXY(i,(col+uv.getX(i))/4,1-(row*256+70+(1-uv.getY(i))*140)/2048);
    const mesh=new THREE.Mesh(g,lettering);mesh.position.set(x,y,z);mesh.rotation.y=angle;root.add(mesh);
  }
  // Subtle limestone veining, with physical joints modelled separately.
  const marble=document.createElement('canvas');marble.width=marble.height=512;
  const mc=marble.getContext('2d')!;mc.fillStyle='#d8d7cc';mc.fillRect(0,0,512,512);
  let seed=91823;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  for(let n=0;n<110;n++){mc.strokeStyle=`rgba(117,131,125,${.015+random()*.045})`;mc.lineWidth=.3+random()*3;mc.beginPath();let y=random()*700-100;mc.moveTo(0,y);for(let x=0;x<=512;x+=32){y+=random()*26-8;mc.lineTo(x,y);}mc.stroke();}
  const marbleTexture=new THREE.CanvasTexture(marble);marbleTexture.colorSpace=THREE.SRGBColorSpace;marbleTexture.wrapS=marbleTexture.wrapT=THREE.RepeatWrapping;marbleTexture.repeat.set(8,5);textures.add(marbleTexture);
  const floor=mat('#ffffff',{map:marbleTexture,roughness:.24,metalness:.1});
  /* 容量按 `plantTree` 的实际消耗估：场内共 24 棵花池树，每棵 5×(130+55)×0.6 ≈ 555
   * 簇 ⇒ 约 13300，再加上各池铺的草 ≈ 1550。原来 9500 是给"每棵 120 簇"那版定的，
   * 换实现后不同步扩容会被 `spray()` 静默截断 —— 满了就 return，冠从内圈开始秃。 */
  const leaves=createLeafField(random,26000);
  const trunks=createTrunkField();
  let treeIndex=0;
  /**
   * `plantTree` 的坐标以"树脚 y=0"为基准，而花池种的树基准是**池底 `base`**。
   * 所以 `rod` / 树干 / 冠簇的 y 一律要抬 `base` —— 直接调用会把树种到地下。
   */
  function treeKit(base:number):TreeKit{
    return {
      blossom:leaves,leaf:leaves,random,
      trunk:{add:(s)=>trunks.add({...s,y0:s.y0+base}),build:trunks.build,install:trunks.install,wants:trunks.wants,get count(){return trunks.count;}},
      rod(a,b,r){rod([a[0],a[1]+base,a[2]],[b[0],b[1]+base,b[2]],r,wood);},
      pit(){/* 花池本体（含碰撞）已经在 planter() 里画好，这里不再重复 */},
    };
  }
  function planter(x:number,y:number,z:number,w:number,d:number,tree=false) {
    block('planter',x,y+.27,z,w,.54,d,stone);box(x,y+.55,z,w-.14,.035,d-.14,earth);
    for(let i=0;i<Math.ceil(w*d*9);i++)leaves.spray(x+(random()-.5)*(w-.18),y+.7+random()*.32,z+(random()-.5)*(d-.18),.24+random()*.16);
    /* 原来是一根 `rod` 圆柱干 + 5 根 `rod` 圆柱枝 + 120 簇叶。现在整棵（干 + 分叉
     * 侧枝 + 枝头/中段两层冠簇）走 `foliage.plantTree`，和街区行道树、公园老樱
     * 同一套实现 —— 干是 Blender 资产，不再是一根管子。
     * base = y+.3：干底落在 y+.5，与原来的干底同高，池面（y+.54）刚好盖住干脚。 */
    if(tree)plantTree(treeKit(y+.3),x,z,false,treeIndex++,.6);
  }
  function rail(x:number,y:number,z:number,w:number,d:number) {
    block('balustrade',x,y+.56,z,w,1.12,d,glass);
    box(x,y+1.13,z,w+.02,.045,d+.02,bronze);
    const alongX=w>d,len=alongX?w:d;
    for(let n=0;n<=Math.floor(len/2);n++){const v=-len/2+n*2;box(x+(alongX?v:0),y+.57,z+(alongX?0:v),.045,1.14,.045,bronze);}
  }
  // An actual hollow shell: side rooms open onto galleries, no solid wing blocks.
  block('ground',42,.06,138,60,.12,32,floor,'floor');
  block('west-shell',12.12,12.6,138,.24,25.2,32,stone);
  block('east-shell-glass',71.88,12.6,138,.08,25.2,32,glass);
  block('rear-shell',42,12.6,153.88,60,25.2,.24,stone);
  for(let level=0;level<6;level++) {
    const y=.12+level*4.2;
    if(level>0){
      for(const x of [24,60])block('gallery-floor',x,y-.14,138,24,.28,32,floor,'floor');
      block('front-gallery',42,y-.14,124.6,12,.28,5.2,floor,'floor');
      block('rear-gallery',42,y-.14,150.4,12,.28,7.2,floor,'floor');
      block('atrium-bridge',42,y-.14,137,12,.28,2,floor,'floor');
      /* 桥的**北侧**栏杆同样拆三段：前半中庭的扶梯要从这里落到桥面上。
       * 南侧（z138）早就是三段，正是给后半中庭的扶梯留的口 —— 两侧现在对称。 */
      for(const [rx,rw] of [[36.7,1.4],[42,1.9],[47.3,1.4]])rail(rx,y,136,rw,.075);
      box(42,y-.18,135.92,12,.065,.065,warm);
      // The void is x36..48, z127.2..146.8. Escalator landings meet these edges.
      rail(36,y,137,.08,19.6);rail(48,y,137,.08,19.6);
      /* 前缘栏杆拆成三段（与后缘 z146.8 同一套分法），给**前半中庭**的扶梯让出
       * 两个口 —— 见下面扶梯的错层布置。原来是一整条 12m 通栏，扶梯要穿过去的话
       * 只能从玻璃里插出来。 */
      for(const [rx,rw] of [[36.7,1.4],[42,1.9],[47.3,1.4]])rail(rx,y,127.2,rw,.075);
      for(const x of [36.7,42,47.3]){
        const width=x===42?1.9:1.4;
        rail(x,y,146.8,width,.075);
        rail(x,y,138,width,.075);
      }
      box(35.93,y-.18,137,.07,.065,19.6,warm);box(48.07,y-.18,137,.07,.065,19.6,warm);
      box(42,y-.18,127.12,12,.065,.06,warm);box(42,y-.18,146.88,12,.065,.06,warm);
    }
    // Four different stores on each wing, fronting the central promenade.
    for(const side of [-1,1])for(let bay=0;bay<4;bay++){
      const z=125.7+bay*7.7,shopX=side<0?21.7:62.3,frontX=side<0?30:54;
      block('shop-partition',shopX,y+1.9,z+3.75,16.3,3.8,.12,ivory);
      box(shopX,y+3.87,z,16.3,.12,7.6,ivory);
      for(const dz of [-2.65,2.65]){block('shop-window',frontX,y+1.55,z+dz,.055,2.95,2.1,glass);box(frontX,y+1.55,z+dz-1.04,.08,3,.055,bronze);}
      box(frontX,y+3.3,z,.22,.65,7.45,ink);
      sign(2+(bay+level*3+(side>0?4:0))%12,frontX-side*.13,y+3.3,z,6,.56,side<0?Math.PI/2:-Math.PI/2);
      box(shopX,y+3.78,z,11,.045,.12,warm);
      // Merchandise islands and garment rails are visible through the glass.
      for(let k=0;k<3;k++){
        const px=shopX+(k-1)*3.4;
        block('display-island',px,y+.4,z,1.5,.8,1.25,wood);
        for(let j=0;j<4;j++)box(px-.5+j*.34,y+.88,z,.25,.16,.65,fashion[(j+k+bay)%fashion.length]);
        rod([px-.8,y+.1,z+2],[px-.8,y+2.1,z+2],.023,bronze);rod([px+.8,y+.1,z+2],[px+.8,y+2.1,z+2],.023,bronze);rod([px-.8,y+2.1,z+2],[px+.8,y+2.1,z+2],.025,bronze);
        for(let j=0;j<5;j++){box(px-.6+j*.3,y+1.48,z+2,.19,.8,.33,fashion[(j+level)%5]);rod([px-.6+j*.3,y+1.85,z+2],[px-.6+j*.3,y+2.08,z+2],.009,bronze);}
      }
      // Lit display niche in the rear of each store.
      box(side<0?12.5:71.5,y+1.7,z,.12,2.8,4.8,display);
    }
    for(const x of [33,51])for(const z of [126,136,146,152])block('column',x,y+2.05,z,.4,4.1,.4,stone);
    for(const x of [33.4,50.6])for(const z of [129,144])planter(x,y,z,1.1,1.7,level===0);
    sign(level===5?12:14,42,y+2.9,151.9,7,.8);
    // Slender front glazing, warm ceiling strips and stone floor edges.
    for(const x of [23,61]){
      box(x,y+4.03,121.85,22,.25,.65,stone);box(x,y+3.87,122.08,21,.055,.08,warm);
      for(let k=0;k<7;k++){const px=x-9.3+k*3.1;block('front-wing-glass',px,y+1.9,122.06,3.04,3.8,.05,glass);box(px-1.52,y+1.9,121.97,.07,3.8,.16,bronze);}
    }
    box(72,y+4.03,138,.55,.26,32,stone);
    box(72.03,y+3.85,138,.08,.055,31.5,warm);
    for(let z=123;z<154;z+=2.6)box(71.96,y+1.95,z,.15,3.9,.055,bronze);
  }
  for(const z of [122.25,130,138,146,153.75]){
    box(72.04,12.6,z,.5,25.2,.46,stone);
    box(72.31,12.6,z,.045,24.8,.08,bronze);
  }
  /* Paired escalators on each level, **staggered between levels**.
   *
   * 原来 5 段全部叠在 z137.6..147.6 / x39.2 & 44.8 同一个位置：从下往上看是一根
   * 五层高的竖井，动线也不成立 —— 真实商场是"上到一层、横穿中庭、再续乘下一
   * 段"。现在相邻层段在平面上错开：
   *   偶数段（f 0/2/4）走中庭**后半**：桥 z137.6 → 后廊 z147.6
   *   奇数段（f 1/3）  走中庭**前半**：前廊 z126.6 → 桥 z136.6
   * 两端都落在实楼板上（桥 z136..138 / 前廊 z122..127.2 / 后廊 z146.8..154），
   * 没有悬空端。前缘栏杆已按后缘的分法拆成三段留出穿口。
   *
   * 同层两部**一上一下**（x39.2 上行 / x44.8 下行）。原来两部同向（整段统一
   * `up = f%2===0`），等于一条只上不下的单行道。 */
  for(let f=0;f<5;f++){
    const y=.12+f*4.2,back=f%2===0,z0=back?137.6:126.6,z1=back?147.6:136.6,run=z1-z0;
    for(const [x,up] of [[39.2,true],[44.8,false]] as const) {
    const startY=y+(up?0:4.2),endY=y+(up?4.2:0);
    for(let i=0;i<64;i++){const t=(i+.5)/64,z=z0+t*run,top=startY+(endY-startY)*t;box(x,top-.12,z,1.72,.24,run/64+.012,ink);box(x,top+.006,z-run/128,1.7,.016,.025,bronze);}
    for(const dx of [-.98,.98]){
      rod([x+dx,startY+.95,z0],[x+dx,endY+.95,z1],.055,ink);
      rod([x+dx,startY+.28,z0],[x+dx,endY+.28,z1],.08,bronze);
      const side=box(x+dx,(startY+endY)/2+.6,(z0+z1)/2,.045,.63,Math.hypot(run,4.2),glass);side.rotation.x=-Math.atan2(endY-startY,run);
    }
    colliders.push({source:'lumina-escalator',kind:'ramp',min:new THREE.Vector3(x-.87,y-.05,z0),max:new THREE.Vector3(x+.87,y+4.25,z1),heightAt:(px,z)=>Math.abs(px-x)>.87||z<z0||z>z1?null:startY+(endY-startY)*(z-z0)/run});
    for(const z of [z0,z1])box(x,z===z0?startY+.01:endY+.01,z,1.72,.035,1.4,bronze);
    }
  }
  // Tall central curtain wall: an unobstructed 6 m wide ground-floor doorway.
  for(const x of [36.5,47.5])block('entry-side-glass',x,2,121.84,5,3.8,.08,glass);
  box(42,14.6,121.84,16,21.2,.08,glass);
  box(42,26.1,121.84,16,1.8,.08,glass);
  for(const x of [34,36,38,40,42,44,46,48,50])box(x,26.1,121.71,.085,1.8,.16,bronze);
  collision('upper-front-glass',42,14.6,121.84,16,21.2,.08);
  for(let x=34;x<=50;x+=2){const overDoor=x>38&&x<46;box(x,overDoor?14.6:12.6,121.71,.085,overDoor?21.2:25.2,.16,bronze);}
  for(let y=4.2;y<25.3;y+=4.2)box(42,y,121.69,16,.1,.17,bronze);
  for(const x of [38.8,45.2]){box(x,1.9,121.65,.09,3.8,.12,bronze);rod([x,.95,121.45],[x,1.9,121.45],.025,bronze);}
  sign(0,42,16.1,121.53,13,2.0);sign(1,42,14.7,121.52,10,.8);
  box(42,4.1,119.5,18,.25,5,bronze);box(42,3.94,119.5,17.8,.045,4.8,ivory);
  for(const x of [34,50])rod([x,.12,117.6],[x,4,117.6],.09,bronze);
  for(const x of [35,38,41,44,47,49])box(x,3.9,118,.12,.045,.12,warm);
  // Front piers, fluted cladding and original large-format art panels.
  const artCanvas=document.createElement('canvas');artCanvas.width=1024;artCanvas.height=1024;
  const ac=artCanvas.getContext('2d')!;
  for(let i=0;i<4;i++){
    const x=i*256;ac.fillStyle=['#b2bab0','#d9b794','#9baca7','#c5c2b7'][i];ac.fillRect(x,0,256,1024);
    ac.fillStyle=['#456558','#543f37','#e4dfcd','#596b76'][i];
    ac.beginPath();ac.ellipse(x+128,520,105,270,-.2+i*.12,0,Math.PI*2);ac.fill();
    ac.strokeStyle='#efe1c6';ac.lineWidth=3;for(let k=0;k<7;k++){ac.beginPath();ac.arc(x+128,500,65+k*20,.3,5.3);ac.stroke();}
    ac.fillStyle='#f8f1df';ac.textAlign='center';ac.font='23px Georgia';ac.fillText(['THE EDIT','SLOW LIVING','IN BLOOM','NEW FORMS'][i],x+128,115);ac.font='13px sans-serif';ac.fillText('L U M I N A   /   2 0 2 6',x+128,900);
  }
  const artTexture=new THREE.CanvasTexture(artCanvas);artTexture.colorSpace=THREE.SRGBColorSpace;textures.add(artTexture);
  const art=mat('#ffffff',{map:artTexture,emissiveMap:artTexture,emissive:'#fff1d8',emissiveIntensity:.14});
  for(const [i,x] of [17.1,27.9,56.1,66.9].entries()){
    box(x,14.1,121.5,8.6,17.8,.65,stone);
    for(let dx=-4;dx<=4;dx+=.28)box(x+dx,14.1,121.13,.055,17.5,.08,ivory);
    box(x,14.1,120.99,6.6,15.9,.15,bronze);
    const g=geometry(new THREE.PlaneGeometry(6.35,15.6)),uv=g.getAttribute('uv');for(let j=0;j<uv.count;j++)uv.setX(j,(i+uv.getX(j))/4);
    const mesh=new THREE.Mesh(g,art);mesh.position.set(x,14.1,120.89);mesh.rotation.y=Math.PI;root.add(mesh);
    for(const dx of [-4.24,4.24])box(x+dx,14.1,121.01,.055,17.55,.055,warm);
    sign(2+i,x,3.65,121.73,8,.8);
  }
  // Wing roofs become gardens; the centre gets a broad, gently bowed canopy.
  for(const x of [23,61]){
    block('roof',x,25.22,138,22,.28,32,stone,'floor');
    const gardenXs=x<42?[16,22,28]:[56,62,68];
    for(const z of [123.4,151.8])for(const px of gardenXs)planter(px,25.36,z,4.8,1.6,true);
    rail(x,25.36,122.4,21,.065);rail(x,25.36,153.4,21,.065);
    for(const px of [x-6,x+6]){box(px,25.85,137,3,.13,1.1,wood);for(const dz of [-.35,.35])box(px,25.57,137+dz,2.6,.48,.1,bronze);}
    for(let px=x-8;px<=x+8;px+=2)box(px,28.1,143,.1,.14,7,wood);
    for(const px of [x-8,x+8])for(const z of [139.5,146.5])rod([px,25.35,z],[px,28.1,z],.055,bronze);
  }
  const roofShape=new THREE.Shape();roofShape.moveTo(30,119);roofShape.quadraticCurveTo(42,114.8,54,119);roofShape.lineTo(54,155);roofShape.lineTo(30,155);roofShape.closePath();
  // A ring shape leaves the skylight open rather than covering it with a slab.
  const hole=new THREE.Path();hole.moveTo(35,125);hole.lineTo(35,151);hole.lineTo(49,151);hole.lineTo(49,125);hole.closePath();roofShape.holes.push(hole);
  const roofGeo=geometry(new THREE.ExtrudeGeometry(roofShape,{depth:.42,bevelEnabled:false,curveSegments:24}));roofGeo.rotateX(Math.PI/2);roofGeo.translate(0,27.4,0);
  const canopy=new THREE.Mesh(roofGeo,ivory);canopy.castShadow=true;canopy.receiveShadow=true;root.add(canopy);
  box(42,27.13,138,14,.05,26,glass);
  for(let x=35;x<=49;x+=2)box(x,27.16,138,.1,.18,26,ivory);
  for(let z=125;z<=151;z+=2.6)box(42,27.16,z,14,.18,.11,ivory);
  for(const x of [34,50])for(const z of [123,133,143,153])rod([x,21.1,z],[x,27.05,z],.08,bronze);
  for(let x=32;x<=52;x+=2)box(x,26.95,120,.1,.055,.1,warm);
  // Concierge behind the entrance, with a clear central route to the escalators.
  block('concierge',34.2,.72,124.7,2.2,1.2,1.1,wood);sign(19,34.2,1.03,124.11,2,.55);
  for(const x of [37,47]){block('directory',x,1.17,124,.55,2.1,.24,bronze);sign(20,x,1.45,123.87,.5,1.35);}
  for(const x of [38.5,45.5]){planter(x,.12,151,2.5,1.8,true);block('interior-seat',x,.4,149.8,2.6,.56,.6,wood);}
  for(const x of [39,45]){planter(x,.12,132,1.5,2.2,true);block('atrium-lounge',x,.4,130.5,2.1,.56,.65,wood);}
  // Small mannequins give the street-level windows a readable human scale.
  const mannequin=mat('#ede1ca');
  for(const x of [15.8,18.1,26.8,29.1,54.8,57.1,65.8,68.1]){
    box(x,.24,123.5,1.2,.24,1.1,ivory);
    for(const dx of [-.12,.12])rod([x+dx,.36,123.5],[x+dx,1.08,123.5],.065,mannequin);
    const torso=box(x,1.35,123.5,.43,.57,.23,fashion[Math.floor(x)%5]);torso.rotation.z=.03;
    const head=new THREE.Mesh(sphere,mannequin);head.position.set(x,1.86,123.5);head.scale.set(.13,.17,.13);root.add(head);
    for(const dx of [-.28,.28])rod([x+dx,1.56,123.5],[x+dx*1.2,1.02,123.45],.044,mannequin);
  }
  // Plaza edges: shallow water gardens flank an accessible central approach.
  for(const x of [28,48]){
    block('water-garden',x,.25,113.7,5.4,.28,3.1,stone);box(x,.404,113.7,5.05,.025,2.75,water);
    const ringGeo=geometry(new THREE.TorusGeometry(1.15,.09,10,56));
    for(let i=0;i<2;i++){const sculpture=new THREE.Mesh(ringGeo,bronze);sculpture.position.set(x,1.72,113.7);sculpture.rotation.set(.3+i*.7,i*.9,.35);root.add(sculpture);}
    collision('sculpture',x,1.5,113.7,2.5,2.8,2.5);
  }
  for(const x of [24,33,51,69]){planter(x,.11,119,2.3,2.1,true);block('plaza-bench',x,.42,117.5,2.5,.62,.5,wood);}
  for(const x of [25,31,47,53,66])for(const z of [111,120]){box(x,.62,z,.1,1,.1,bronze);box(x,1.13,z,.16,.09,.16,warm);}
  // Paving joints and inset strips lead from the street to the entrance.
  for(let x=35;x<=45;x+=2)box(x,.125,116,.018,.015,10.6,bronze);
  for(let z=111;z<122;z+=2)box(40,.126,z,10,.015,.018,bronze);
  for(const x of [35,45])box(x,.13,116,.045,.015,10.5,warm);
  const lights=[new THREE.PointLight('#ffd6a0',3,26,2),new THREE.PointLight('#ffd6a0',3,22,2)];
  lights[0].position.set(42,9,137);lights[1].position.set(42,4,119);lights.forEach(l=>root.add(l));
  /* 干场和冠簇一样，必须等 mergeByMaterial 之后再挂：merge 只按 isMesh 过滤，
   * InstancedMesh 也是 isMesh，混进去会被当成一块几何烘掉。 */
  mergeByMaterial(root);root.add(leaves.build('lumina-garden-foliage'));root.add(trunks.build('lumina-tree-trunks',wood.color));
  function setEnvironment(period:string){const n=period==='night',d=period==='dusk';warm.emissiveIntensity=n?1.3:d?.8:.35;display.emissiveIntensity=n?.65:d?.35:.12;lettering.emissiveIntensity=n?.65:d?.35:.16;art.emissiveIntensity=n?.55:d?.3:.12;for(const l of lights)l.intensity=n?12:d?8:3;}
  setEnvironment('day');
  return {colliders,setEnvironment,dispose(){root.removeFromParent();root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);if(o instanceof THREE.InstancedMesh)o.dispose();});geometries.add(leaves.geometry);for(const g of geometries)g.dispose();for(const m of materials)m.dispose();for(const t of textures)t.dispose();leaves.material.dispose();leaves.texture.dispose();}};
}
