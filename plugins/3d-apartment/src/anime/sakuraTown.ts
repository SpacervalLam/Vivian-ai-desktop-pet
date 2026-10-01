import * as THREE from 'three';
import { mergeByMaterial, freezeStatic } from './merge';
import { createBlossomField, createFallenPetals, createLeafField, createTrunkField } from './foliage';
import type { Collider } from './collider';
import { CITY_LOTS, CITY_INBLOCK_TOWER_LOTS } from './urbanStreets';

/** Human-scale foreground. All shop dimensions and collision boxes share local coordinates. */
export function createSakuraTown(scene: THREE.Scene) {
  const root = new THREE.Group();
  root.name = 'sakura-shotengai';
  root.userData.sceneCollideSkip = true;
  scene.add(root);
  const colliders: Collider[] = [];
  const materials: THREE.Material[] = [];
  const textures: THREE.Texture[] = [];
  const shopLights: THREE.PointLight[] = [];
  const geometries = new Set<THREE.BufferGeometry>();
  const mat = (color: string, emissive = false) => {
    const m = new THREE.MeshStandardMaterial({ color, roughness: .86,
      ...(emissive ? { emissive: color, emissiveIntensity: .45 } : {}) });
    m.userData.outlineWeight = 0; materials.push(m); return m;
  };
  const M = {
    plaster: mat('#d6cfbd'), sage: mat('#92a79a'), blue: mat('#a0b2bf'), pink: mat('#c7afa5'),
    wood: mat('#785342'), darkWood: mat('#493b36'), roof: mat('#4d5764'), roofEdge: mat('#6f7d86'),
    iron: mat('#394b52'), cream: mat('#f3e3c8'), red: mat('#ae534d'), teal: mat('#547d77'),
    brass: mat('#b99360'), concrete: mat('#a5a6a0'), soil: mat('#514744'), leaf: mat('#547755'),
    leafLight: mat('#829b64'), flower: mat('#e797b2'), blueBook: mat('#728ca9'), yellow: mat('#dec178'),
    paper: mat('#ebe4d5'), lamp: mat('#ffe1ae', true), redLamp: mat('#df7663',true), interior: mat('#e0c7a1'), black: mat('#273239'),
    signBlue: mat('#3f6ea6'), signRed: mat('#c14a42'),
  };
  const glass = new THREE.MeshStandardMaterial({color:'#b4d1d8',transparent:true,opacity:.12,roughness:.22,metalness:.1,depthWrite:false});
  materials.push(glass);
  /* 神社专项材质（2026-09-25 按参考图还原门前街景）。
   * 鸟居不复用 M.red(#ae534d 偏砖红)：那个材质同时是食堂/茶房的布帘色，改它会连带改帘子。
   * 参考图的鸟居是鲜朱色 + 近黑的笠木，所以单开两味。 */
  const shrineRed = mat('#bd4a37');
  const shrineCap = mat('#2b2b30');
  const granite   = mat('#b7b5ae');   // 社号标的花岗岩
  const poleGrey  = mat('#c3c0b7');   // 门前电线杆的混凝土
  let seed = 62871;
  const random = () => { seed = (Math.imul(seed,1664525)+1013904223)>>>0; return seed/4294967296; };
  function surface(kind:'wood'|'plaster') {
    const canvas=document.createElement('canvas');canvas.width=canvas.height=256;const c=canvas.getContext('2d')!;
    c.fillStyle='#f0eee8';c.fillRect(0,0,256,256);
    for(let n=0;n<(kind==='wood'?400:6500);n++){
      const x=random()*256,y=random()*256;c.fillStyle=`rgba(68,58,49,${random()*(kind==='wood'?.13:.08)})`;
      c.fillRect(x,y,kind==='wood'?12+random()*110:1,kind==='wood'?.6:1);
    }
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;textures.push(t);return t;
  }
  const woodGrain=surface('wood'),stucco=surface('plaster');
  for(const m of [M.wood,M.darkWood])m.map=woodGrain;
  for(const m of [M.plaster,M.sage,M.blue,M.pink])m.map=stucco;
  const geo = <T extends THREE.BufferGeometry>(g: T): T => { geometries.add(g); return g; };
  const cube = geo(new THREE.BoxGeometry(1,1,1));
  const sphere = geo(new THREE.IcosahedronGeometry(1,1));
  const cylinder = geo(new THREE.CylinderGeometry(1,1,1,10));
  const ring = geo(new THREE.TorusGeometry(1,.08,6,20));
  const bowl = geo(new THREE.LatheGeometry([[0,0],[.072,.005],[.11,.03],[.166,.108],[.16,.12],[.146,.108],[.10,.037],[.065,.018],[0,.018]].map(([r,y])=>new THREE.Vector2(r,y)),20));
  const up = new THREE.Vector3(0,1,0);
  function mesh(parent: THREE.Group, geometry: THREE.BufferGeometry, material: THREE.Material,
    x: number,y: number,z: number,sx=1,sy=1,sz=1) {
    const m = new THREE.Mesh(geometry,material);m.position.set(x,y,z);m.scale.set(sx,sy,sz);
    m.castShadow = !material.transparent;m.receiveShadow = true;parent.add(m);return m;
  }
  const box = (g:THREE.Group,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material) => mesh(g,cube,m,x,y,z,w,h,d);
  const ball = (g:THREE.Group,x:number,y:number,z:number,s:number,m:THREE.Material) => mesh(g,sphere,m,x,y,z,s,s,s);
  function rod(g:THREE.Group,a:number[],b:number[],r:number,m:THREE.Material) {
    const from = new THREE.Vector3(...a), to = new THREE.Vector3(...b);
    const v=to.clone().sub(from), mid=from.add(to).multiplyScalar(.5);
    const o=mesh(g,cylinder,m,mid.x,mid.y,mid.z,r,v.length(),r);
    o.quaternion.setFromUnitVectors(up,v.normalize());return o;
  }
  function solid(g:THREE.Group,name:string,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material,kind:Collider['kind']='wall') {
    box(g,x,y,z,w,h,d,m);
    colliders.push({source:`sakura-${name}`,kind,
      min:new THREE.Vector3(g.position.x+x-w/2,y-h/2,g.position.z+z-d/2),
      max:new THREE.Vector3(g.position.x+x+w/2,y+h/2,g.position.z+z+d/2)});
  }
  // Text is painted once; fine rules, prices, stamps and paper margins read at street level.
  function label(g:THREE.Group,title:string,sub:string,x:number,y:number,z:number,w:number,h:number,bg='#f3e7cf',ink='#4d4c49') {
    const canvas=document.createElement('canvas');canvas.width=768;canvas.height=Math.min(1024,Math.max(128,Math.round(768*h/w)));
    const c=canvas.getContext('2d')!, W=canvas.width,H=canvas.height;
    c.fillStyle=bg;c.fillRect(0,0,W,H);c.strokeStyle=ink;c.lineWidth=4;c.strokeRect(13,13,W-26,H-26);
    c.textAlign='center';c.textBaseline='middle';c.fillStyle=ink;
    c.font=`600 ${Math.min(H*.47,W/(title.length+1))}px "Yu Mincho", "Microsoft YaHei", serif`;
    c.fillText(title,W/2,H*.4,W*.88);c.font=`${Math.min(H*.17,36)}px sans-serif`;c.fillText(sub,W/2,H*.78,W*.86);
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;textures.push(t);
    const material=new THREE.MeshStandardMaterial({map:t,roughness:.9});materials.push(material);
    const p=mesh(g,geo(new THREE.PlaneGeometry(w,h)),material,x,y,z);p.rotation.y=Math.PI;return p;
  }
  /* 竖排文字牌（社号标「桜町稲荷神社」/ 电线杆上的「消火栓」「駅前東2」）。
   * label() 是"横排标题 + 副题"，这几块都是竖排，逐字居中另画一张。
   * 同样是一块 PlaneGeometry 转 y=π 朝 -z（与 label 一致）。 */
  function vlabel(g:THREE.Group,text:string,x:number,y:number,z:number,w:number,h:number,bg:string,ink:string) {
    const canvas=document.createElement('canvas');canvas.width=160;canvas.height=Math.round(160*h/w);
    const c=canvas.getContext('2d')!, W=canvas.width,H=canvas.height;
    c.fillStyle=bg;c.fillRect(0,0,W,H);
    c.textAlign='center';c.textBaseline='middle';c.fillStyle=ink;
    const n=text.length, cell=H/n;
    c.font=`700 ${Math.min(cell*.84,W*.8)}px "Yu Mincho", "Microsoft YaHei", serif`;
    for(let i=0;i<n;i++)c.fillText(text[i],W/2,cell*(i+.5));
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;textures.push(t);
    const material=new THREE.MeshStandardMaterial({map:t,roughness:.9});materials.push(material);
    const p=mesh(g,geo(new THREE.PlaneGeometry(w,h)),material,x,y,z);p.rotation.y=Math.PI;return p;
  }
  /* 电线杆根部的黄黑斜纹警示套（参考图那根杆最抓眼的细节）。
   * 45° 斜条画在一张 128² canvas 上，绕杆一圈用 repeat 铺开。
   * repeat 要按"世界尺寸正方形"取：杆周长 2π*0.137≈0.861m、套高 1.9m，
   * 一个 tile 取 0.36m → x 2.4 次 / y 5.3 次，斜纹才不会拉成长条。 */
  function hazardBand() {
    const canvas=document.createElement('canvas');canvas.width=canvas.height=128;
    const c=canvas.getContext('2d')!;
    c.fillStyle='#e6b32a';c.fillRect(0,0,128,128);
    c.strokeStyle='#23232b';c.lineWidth=30;
    for(let i=-128;i<=256;i+=64){c.beginPath();c.moveTo(i,-4);c.lineTo(i+136,132);c.stroke();}
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;
    t.wrapS=t.wrapT=THREE.RepeatWrapping;t.repeat.set(2.4,5.3);textures.push(t);
    const m=new THREE.MeshStandardMaterial({map:t,roughness:.7,metalness:.05});
    m.userData.outlineWeight=0;materials.push(m);return m;
  }
  function pot(g:THREE.Group,x:number,y:number,z:number,s=.25,flowers=false) {
    mesh(g,geo(new THREE.CylinderGeometry(s,s*.7,s*1.35,9)),M.wood,x,y+s*.68,z);
    mesh(g,cylinder,M.soil,x,y+s*1.37,z,s*.89,.025,s*.89);
    for(let n=0;n<8;n++){
      const a=n*2.4,dx=Math.cos(a)*s*.65,dz=Math.sin(a)*s*.65,top=y+s*(1.8+random()*1.1);
      rod(g,[x,y+s,z],[x+dx,top,z+dz],.012,M.leaf);
      const leaf=ball(g,x+dx,top-.06,z+dz,s*.43,n%2?M.leaf:M.leafLight);leaf.scale.y*=1.7;
      if(flowers)ball(g,x+dx,top+.09,z+dz,s*.24,n%2?M.flower:M.cream);
    }
  }
  function lantern(g:THREE.Group,x:number,y:number,z:number,color:THREE.Material) {
    rod(g,[x,y+.48,z],[x,y+.24,z],.014,M.iron);
    mesh(g,geo(new THREE.SphereGeometry(1,12,10)),color===M.red?M.redLamp:color,x,y,z,.17,.25,.17);
    for(const dy of [-.24,.24])mesh(g,cylinder,M.iron,x,y+dy,z,.10,.04,.10);
    for(let n=-3;n<=3;n++){
      const r=.174*Math.sqrt(1-(n/4.2)**2);
      const rib=mesh(g,ring,M.cream,x,y+n*.055,z,r,r,r);rib.rotation.x=Math.PI/2;
    }
  }
  function bicycle(g:THREE.Group,x:number,z:number,color:THREE.Material) {
    const y=.39;
    for(const dx of [-.48,.48]){
      mesh(g,ring,M.black,x+dx,y,z,.33,.33,.33);
      const rim=mesh(g,ring,M.concrete,x+dx,y,z,.295,.295,.295);rim.scale.z=.22;
      for(let k=0;k<10;k++){const a=k*Math.PI/5;rod(g,[x+dx,y,z],[x+dx+Math.cos(a)*.29,y+Math.sin(a)*.29,z],.007,M.concrete);}
    }
    const a=[x-.48,y,z],b=[x-.12,.84,z],c=[x+.02,.4,z],d=[x+.35,.91,z],e=[x+.48,y,z];
    for(const [p,q] of [[a,b],[b,c],[c,a],[c,d],[b,d],[d,e]])rod(g,p,q,.023,color);
    box(g,x-.14,.92,z,.24,.055,.15,M.black);
    rod(g,[x+.35,.9,z],[x+.35,1.12,z],.02,M.iron);rod(g,[x+.35,1.12,z-.17],[x+.35,1.12,z+.17],.018,M.iron);
    box(g,x+.49,.9,z,.29,.20,.29,M.concrete);
    for(let n=0;n<4;n++)box(g,x+.49,.83+n*.05,z-.155,.3,.012,.015,M.iron);
    rod(g,[x,.36,z],[x-.12,.07,z-.19],.014,M.iron);
    colliders.push({source:'sakura-bicycle',min:new THREE.Vector3(x-.84,0,z-.2),max:new THREE.Vector3(x+.84,1.1,z+.2)});
  }
  function vending(g:THREE.Group,x:number,z:number) {
    solid(g,'vending',x,.96,z,.88,1.9,.68,M.red);
    box(g,x,1.24,z-.355,.70,1.02,.025,M.cream);
    box(g,x,.42,z-.36,.62,.19,.035,M.black);
    for(let row=0;row<3;row++)for(let col=0;col<5;col++){
      const bx=x-.26+col*.13,by=.96+row*.29;
      mesh(g,cylinder,[M.teal,M.yellow,M.blueBook,M.flower,M.paper][col],bx,by,z-.39,.044,.16,.044);
      box(g,bx,by-.1,z-.39,.068,.025,.022,M.lamp);
    }
    label(g,'つめた〜い','DRINKS · ¥120',x,1.77,z-.36,.72,.17,'#f5ead5','#b44d4d');
    /* 商品窗玻璃罩（2026-09-25：用户报「两种自动售货机都缺少玻璃罩」，随后要求去掉
     * 门框、又说"你只盖了正面，侧面没包住"）。现在是**正面 + 左右 + 顶面**的罩子。
     *
     * 罐子凸在柜面(z-.34)之外、最前到 z-.434，所以正面玻璃压到 z-.445 才算罩住商品。
     * 罩子顶边停在 y1.68 —— 上面 1.685..1.855 是 'つめた〜い' 那块招牌（一块 Plane，
     * 就贴在 z-.36），让它留在玻璃外的机身上，不然招牌下沿会糊在玻璃里面。
     * 侧板 x±0.38：陈列层最宽是那块 0.70 宽的商品窗(x±0.35)，让出 0.03；内面 ±0.37
     * 也正好让开招牌的 ±0.36。进深 z −0.455..−0.345，柜面在 −0.34，留 5mm 不贴上。
     * 顶板宽取 0.74 = 正面玻璃宽度，正好落在两侧板**内面**上、深 0.065 停在 z-.37
     * （招牌在 z-.36 之前 0.01），与既有几何**零体积相交**。
     * 复用本文件既有的 glass（橱窗玻璃同款），只做视觉，不另登记碰撞盒。 */
    box(g,x,1.19,z-.445,.74,.98,.02,glass);                            // 正面
    box(g,x-.38,1.19,z-.40,.02,.98,.11,glass);                         // 左侧
    box(g,x+.38,1.19,z-.40,.02,.98,.11,glass);                         // 右侧
    box(g,x,1.67,z-.4025,.74,.02,.065,glass);                          // 顶面
  }
  const shopSpecs: {x:number;z?:number;name:string;sub:string;wall:THREE.MeshStandardMaterial;cloth:THREE.MeshStandardMaterial;height:number;kind:number}[] = [
    {x:21.82,name:'さくら食堂',sub:'手打ち麺  /  RAMEN',wall:M.sage,cloth:M.red,height:6.25,kind:0},
    {x:25.18,name:'こむぎ日和',sub:'BAKERY  /  焼きたてパン',wall:M.plaster,cloth:M.teal,height:5.8,kind:1},
    {x:29.32,name:'花と暮らし',sub:'FLOWERS  /  花束・鉢植え',wall:M.blue,cloth:M.cream,height:6.1,kind:2},
    {x:32.68,name:'喫茶 月と猫',sub:'COFFEE  /  自家焙煎',wall:M.pink,cloth:M.teal,height:6.5,kind:3},
    {x:-17.2,name:'サイクル丸山',sub:'自転車  /  修理・販売',wall:M.blue,cloth:M.teal,height:5.65,kind:4},
    {x:-13.84,name:'茶房 春風',sub:'抹茶と和菓子  /  TEA ROOM',wall:M.plaster,cloth:M.red,height:6.15,kind:3},
  ];
  // Replace the generic apartment blocks with human-scale courtyard shop houses.
  const titles=['桜月堂','暮らしの花','小春喫茶','こむぎ工房','春風食堂','ひだまり堂'];
  // 街区里由 districtArt 就地盖高楼的地块必须跳过：一块地只能有一个主人，
  // 两边都盖就是楼与店屋互穿。集合由 urbanStreets 统一给出（见 CITY_INBLOCK_TOWER_LOTS）。
  CITY_LOTS.filter(lot=>!lot.id.endsWith('-19.6')&&!CITY_INBLOCK_TOWER_LOTS.has(lot.id)).forEach((lot,i)=>{
    shopSpecs.push({x:lot.x,z:lot.z-3.3,name:titles[i%titles.length],sub:'桜町商店街  /  SAKURA TOWN',wall:[M.plaster,M.sage,M.blue,M.pink][i%4],cloth:i%2?M.teal:M.cream,height:5.5+(i%4)*.25,kind:i%4});
  });
  // Continuous frontage links the original shops to the railway approach.
  for(const [i,x] of [49,52.5,56,59.5,68,71.5,75].entries())shopSpecs.push({x,z:16.7,name:titles[i%6],sub:'駅前通り  /  STATION STREET',wall:[M.plaster,M.sage,M.blue][i%3],cloth:i%2?M.teal:M.red,height:5.6+(i%3)*.3,kind:i%4});
  shopSpecs.forEach((spec,shopNumber)=>{
    const index=spec.kind;
    const g=new THREE.Group();g.name=`sakura-shop-${shopNumber}`;g.position.set(spec.x,0,spec.z??15.8);root.add(g);
    if(shopNumber<6){const light=new THREE.PointLight('#ffcc92',9,5.4,2);light.position.set(.1,2.55,2.7);g.add(light);shopLights.push(light);}
    const w=3.24,d=6.6,h=spec.height;
    if(shopNumber>=6)solid(g,'shop-entry-paving',0,.025,-1.2,3.24,.05,2.4,M.concrete,'floor');
    solid(g,'floor',0,.055,d/2,w,.11,d,M.concrete,'floor');
    solid(g,'west-wall',-w/2+.07,1.58,d/2,.14,3.05,d,spec.wall);
    solid(g,'east-wall',w/2-.07,1.58,d/2,.14,3.05,d,spec.wall);
    solid(g,'back-wall',0,1.58,d-.07,w,3.05,.14,spec.wall);
    solid(g,'ceiling',0,3.1,d/2,w,.18,d,M.wood);
    solid(g,'upper-house',0,(3.19+h)/2,d/2,w,h-3.19,d,spec.wall);
    // Shop window at left; open doorway on the right has a clear 0.91m opening.
    solid(g,'display-sill',-.48,.36,.02,2.03,.58,.16,M.darkWood);
    solid(g,'display-glass',-.48,1.43,.045,2.03,1.56,.045,glass);
    for(const x of [-1.5,.53,1.51])box(g,x,1.48,-.07,.09,2.9,.15,M.darkWood);
    box(g,-.48,2.23,-.09,2.08,.07,.12,M.darkWood);
    box(g,-.48,1.4,-.08,.045,1.65,.06,M.wood);
    box(g,1.0,.135,-.08,.87,.05,.40,M.concrete);
    // Deep shop interior: tile floor, exposed beams, illuminated shelves, counter and stools.
    for(let z=.4;z<6.4;z+=.44)for(let x=-1.3;x<1.4;x+=.44)
      box(g,x,.116,z,.425,.014,.425,(Math.round(z*10)+Math.round(x*10))%3===0?M.interior:M.paper);
    for(const z of [.45,2.8,5.5])box(g,0,2.94,z,3.1,.15,.14,M.darkWood);
    solid(g,'counter',-.67,.57,2.5,.88,.94,3.45,M.wood);
    box(g,-.67,1.075,2.5,1.03,.10,3.57,M.darkWood);
    for(let k=0;k<4;k++){
      const z=1.22+k*.78;
      if(index===0||index===3){
        colliders.push({source:'sakura-stool',min:new THREE.Vector3(spec.x-.08,.1,g.position.z+z-.24),max:new THREE.Vector3(spec.x+.40,.86,g.position.z+z+.24)});
        mesh(g,cylinder,M.iron,.16,.44,z,.044,.66,.044);
        mesh(g,cylinder,M.iron,.16,.15,z,.23,.045,.23);
        const footRing=mesh(g,ring,M.concrete,.16,.38,z,.17,.17,.17);footRing.rotation.x=Math.PI/2;
        mesh(g,cylinder,index===0?M.red:M.teal,.16,.81,z,.24,.09,.24);
        mesh(g,bowl,M.paper,-.43,1.13,z);
        mesh(g,cylinder,index===0?M.brass:M.leaf,-.43,1.207,z,.138,.008,.138);
        const bowlRim=mesh(g,ring,M.red,-.43,1.244,z,.156,.156,.08);bowlRim.rotation.x=Math.PI/2;
        if(index===0){
          for(let noodle=0;noodle<4;noodle++){const loop=mesh(g,ring,M.cream,-.47+noodle*.024,1.216,z,.044,.036,.044);loop.rotation.x=Math.PI/2;}
          const egg=ball(g,-.39,1.229,z+.061,.043,M.paper);egg.scale.y=.021;
          const yolk=ball(g,-.39,1.249,z+.061,.021,M.yellow);yolk.scale.y=.009;
          for(let spring=0;spring<3;spring++)box(g,-.47+spring*.024,1.224,z-.065,.011,.012,.03,M.leaf);
        }
        rod(g,[-.7,1.135,z-.09],[-.4,1.15,z+.02],.009,M.wood);
        // Condiments, napkins and chopstick holders have their own silhouette.
        mesh(g,cylinder,M.red,-.99,1.19,z,.045,.18,.045);
        mesh(g,cylinder,M.cream,-.99,1.295,z,.032,.025,.032);
        box(g,-.93,1.17,z+.15,.13,.08,.10,M.paper);
        mesh(g,cylinder,M.darkWood,-.85,1.20,z+.29,.05,.20,.05);
        for(let chop=0;chop<5;chop++)rod(g,[-.88+chop*.014,1.19,z+.29],[-.87+chop*.014,1.49,z+.3],.005,M.brass);
      }else if(index===1){
        for(let shelf=0;shelf<2;shelf++){
          const y=1.12+shelf*.41;box(g,-.65,y,z,.78,.045,.62,M.brass);
          for(let loaf=0;loaf<4;loaf++){
            const x=-.91+(loaf%2)*.34,pz=z-.15+Math.floor(loaf/2)*.30;
            const bread=ball(g,x,y+.11,pz,.13,M.interior);bread.scale.set(.14,.095,.115);
            for(let cut=0;cut<3;cut++)box(g,x-.06+cut*.05,y+.20,pz,.009,.008,.10,M.cream);
          }
        }
        box(g,-.17,1.43,z,.025,.64,.68,glass);box(g,-.65,1.77,z,.98,.025,.69,glass);
      }else if(index===2){
        for(let n=0;n<3;n++)pot(g,-.96+n*.27,1.13,z,.14,true);
        pot(g,-1.30,.12,z+.2,.15,k%2===0);
      }else{
        // Cycle repair bench, pegboard tools, spare rims and stacked tire tubes.
        mesh(g,ring,M.black,-1.47,1.65,z,.30,.30,.30).rotation.y=Math.PI/2;
        box(g,-.69,1.16,z,.32,.08,.18,M.red);
        rod(g,[-.72,1.22,z-.18],[-.72,1.22,z+.15],.018,M.concrete);
        for(let n=0;n<3;n++)rod(g,[-1.47,2.16,z-.16+n*.12],[-1.47,1.84,z-.16+n*.12],.021,M.concrete);
      }
      lantern(g,-.4,2.45,z,index===0?M.red:M.lamp);
    }
    for(const z of [2.0,4.0]){
      box(g,1.47,1.92,z,.07,.96,1.34,M.darkWood);
      const wallMenu=label(g,index===0?'季節のらーめん':index===4?'自転車点検':'桜のある暮らし',index===0?'醤油 680円   味噌 750円':index===4?'修理 · 空気入れ · パンク':'SAKURAMACHI · SPRING',1.424,1.92,z,1.23,.84,'#ecdfbf','#5f6455');wallMenu.rotation.y=-Math.PI/2;
    }
    if(index===0){
      solid(g,'kitchen',-.48,.59,5.18,1.56,.98,.75,M.concrete);
      for(const x of [-.93,-.27]){
        mesh(g,cylinder,M.iron,x,1.12,5.17,.26,.07,.26);
        mesh(g,cylinder,M.concrete,x,1.31,5.17,.21,.32,.21);
        mesh(g,cylinder,M.paper,x,1.48,5.17,.22,.025,.22);
        box(g,x,1.51,5.17,.10,.04,.065,M.black);
      }
      box(g,-.48,2.5,5.18,1.71,.26,.95,M.concrete);
      box(g,-.48,2.22,5.64,1.76,.26,.08,M.iron);
      for(let n=0;n<4;n++)label(g,['醤油','塩','味噌','冷麺'][n],`¥${680+n*70}`,-1.08+n*.68,2.33,6.15,.45,.7);
    }else if(index===3){
      box(g,-.73,1.37,4.0,.65,.46,.43,M.concrete);
      for(const x of [-.9,-.58]){mesh(g,cylinder,M.black,x,1.11,3.73,.04,.24,.04);box(g,x,1.36,3.7,.035,.07,.16,M.iron);}
      box(g,-.73,1.42,3.77,.51,.17,.028,M.iron);
      for(let n=0;n<3;n++)mesh(g,cylinder,M.paper,-.96+n*.20,1.69,4,.07,.16,.07);
      label(g,shopNumber===5?'おしながき':'COFFEE MENU',shopNumber===5?'抹茶 ¥450  /  桜もち ¥280':'BLEND ¥450  /  LATTE ¥520',0,2.43,6.12,1.65,.69,'#344b46','#f2dec1');
    }
    for(let shelf=0;shelf<4;shelf++){
      const y=.53+shelf*.48;
      box(g,0,y,6.26,2.78,.065,.36,M.wood);
      for(let item=0;item<13;item++){
        const x=-1.25+item*.205;
        if(index===0){mesh(g,cylinder,item%2?M.paper:M.red,x,y+.13,6.25,.063,.23,.063);}
        // 面包要"略长"，所以是**乘**不是赋值：ball() 已经写成 (.095,.095,.095)，
        // 直接 `scale.z=1.6` 会把它变成一根 3.2m 的长梭，从背墙（局部 z 6.46~6.60）
        // 里捅出去 1.2m —— 那排"尖刺"就是它。
        else if(index===1){const bread=ball(g,x,y+.105,6.23,.095,item%2?M.interior:M.brass);bread.scale.z*=1.6;}
        else if(index===2)pot(g,x,y+.04,6.24,.07,true);
        else if(index===4){mesh(g,cylinder,item%2?M.red:M.teal,x,y+.15,6.24,.054,.25,.054);}
        else box(g,x,y+.17,6.24,.14,.27+random()*.07,.19,[M.red,M.blueBook,M.paper,M.teal][item%4]);
      }
    }
    // A display immediately behind the front glass makes the shop legible from the road.
    box(g,-.47,.78,.45,1.77,.10,.52,M.wood);
    for(let n=0;n<7;n++){
      const x=-1.2+n*.24;
      if(index===2)pot(g,x,.84,.45,.10,true);
      else if(index===1){const bun=ball(g,x,.94,.42,.115,M.brass);bun.scale.set(.13,.09,.16);}
      else {mesh(g,cylinder,n%2?M.paper:M.teal,x,.98,.45,.075,.25,.075);box(g,x,.99,.37,.08,.07,.014,M.paper);}
    }
    // Awning has real scalloped valance and rods, separate from the timber sign fascia.
    box(g,0,2.83,-.12,3.16,.63,.19,M.darkWood);
    label(g,spec.name,spec.sub,0,2.84,-.225,2.92,.48,index===0?'#84423d':'#efe5ce',index===0?'#f3e8ce':'#394e50');
    box(g,1.28,3.65,-.24,.33,.95,.18,M.darkWood);
    label(g,['麺','麦','花',shopNumber===5?'茶':'珈琲','輪'][index],['食堂','パン','花屋','喫茶','修理'][index],1.28,3.65,-.34,.28,.84,index===0?'#a9534c':'#eee2c8',index===0?'#fff0d0':'#476d62');
    for(let panel=0;panel<12;panel++){
      const x=-1.47+panel*.268;
      const aw=box(g,x,2.41,-.49,.268,.05,.93,panel%3===0?M.cream:spec.cloth);aw.rotation.x=-.17;
      box(g,x,2.27,-.93,.268,.19,.055,panel%3===0?M.cream:spec.cloth);
    }
    for(const x of [-1.47,1.47])rod(g,[x,1.99,-.03],[x,2.36,-.89],.016,M.iron);
    for(let n=0;n<3;n++)box(g,.7+n*.29,2.05,-.04,.27,.40,.026,spec.cloth);
    lantern(g,-1.31,2.02,-.4,index%2?M.lamp:M.red);
    // Upper story: clapboards, sash windows, curtains, sliding shutters and balcony.
    for(let y=3.35;y<h-.15;y+=.19)box(g,0,y,-.01,w,.018,.018,index%2?M.cream:M.roofEdge);
    for(const x of [-.78,.77]){
      box(g,x,4.51,-.07,1.20,1.52,.11,M.darkWood);
      box(g,x,4.51,-.135,1.07,1.37,.03,M.blueBook);
      for(const dx of [-.35,.35]){
        box(g,x+dx,4.51,-.156,.23,1.29,.018,M.paper);
        for(let fold=0;fold<4;fold++)box(g,x+dx-.08+fold*.045,4.51,-.17,.009,1.29,.015,M.plaster);
      }
      box(g,x,4.51,-.19,.045,1.45,.04,M.cream);box(g,x,4.53,-.19,1.14,.04,.04,M.cream);
      box(g,x,3.7,-.28,1.30,.11,.45,M.concrete);
      box(g,x,4.13,-.47,1.26,.045,.045,M.iron);
      for(let k=0;k<9;k++)box(g,x-.57+k*.143,3.94,-.47,.026,.37,.026,M.iron);
      if(x<0)for(let k=0;k<3;k++)pot(g,x-.36+k*.34,3.77,-.24,.095,k%2===0);
    }
    // Tiled gable roof: staggered rows, curved tile rolls, ridge caps and deep eaves.
    //
    // 侧向不做挑檐。最紧的两间只隔 3.36m（w=3.24 → 半宽 1.62），原来 half=w/2+.24=1.86，
    // 再加檐口圆管（r .058）外延到 1.918，相邻两间的屋顶在 x 上重叠 **0.41m** ——
    // 这就是"楼房互相穿模"（实测店铺组 AABB 相交 ~29m³）。
    // 收到 half=w/2-.02=1.60 后最大外延 1.658，相邻净距 3.36-2×1.658 = 0.044m：
    // 读起来是连成一排的町屋，但几何互不相交。
    // 前后挑檐（z 向 slab d+.65 / 脊 d+.37）保留 —— 那才是街道上看得到的那道檐。
    // 山墙三角仍按 w/2 建：屋面外沿到 1.628 > 1.62，正好把山墙角盖住，不会露缝。
    const rise=.85,half=w/2-.02,angle=Math.atan2(rise,half),slope=Math.hypot(rise,half);
    const gableShape=new THREE.Shape();gableShape.moveTo(-w/2,0);gableShape.lineTo(w/2,0);gableShape.lineTo(0,rise);gableShape.closePath();
    const gableGeo=geo(new THREE.ShapeGeometry(gableShape));
    const frontGable=mesh(g,gableGeo,spec.wall,0,h,-.025);frontGable.rotation.y=Math.PI;
    mesh(g,gableGeo,spec.wall,0,h,d+.025);
    for(let vent=0;vent<4;vent++)box(g,0,h+.15+vent*.095,-.042,.47-vent*.06,.027,.018,M.darkWood);
    for(const side of [-1,1]){
      const roof=box(g,side*half/2,h+rise/2,d/2,slope,.12,d+.65,M.roof);roof.rotation.z=-side*angle;
      for(let row=0;row<6;row++)for(let col=0;col<25;col++){
        const t=(row+.5)/6, x=side*half*t,z=-.27+col*.294;
        const tile=box(g,x,h+rise*(1-t)+.085,z,slope/6+.025,.035,.274,(col+row)%4===0?M.roofEdge:M.roof);tile.rotation.z=-side*angle;
      }
      rod(g,[side*half,h-.04,-.33],[side*half,h-.04,d+.33],.058,M.iron);
    }
    rod(g,[0,h+rise+.08,-.37],[0,h+rise+.08,d+.37],.10,M.roofEdge);
    rod(g,[w/2-.04,.17,.16],[w/2-.04,h,.16],.045,M.iron);
    for(const y of [1.1,3.2,5.2])box(g,w/2-.04,y,.12,.13,.06,.11,M.brass);
    // Air conditioner: actual circular grille, fins, brackets and insulated pipe.
    box(g,1.08,5.63,-.20,.82,.43,.34,M.paper);
    mesh(g,ring,M.iron,.96,5.64,-.39,.15,.15,.07);
    for(let k=0;k<5;k++)box(g,1.34,5.49+k*.065,-.38,.20,.022,.025,M.roofEdge);
    for(let k=0;k<8;k++){const a=k*Math.PI/4;rod(g,[.96,5.64,-.39],[.96+Math.cos(a)*.13,5.64+Math.sin(a)*.13,-.39],.008,M.iron);}
    rod(g,[1.47,5.63,-.22],[1.52,4.92,-.22],.026,M.cream);
    rod(g,[-.3,h+.7,5.1],[-.3,h+2.0,5.1],.015,M.iron);
    rod(g,[-.85,h+1.75,5.1],[.25,h+1.75,5.1],.014,M.iron);
    for(let k=0;k<5;k++)rod(g,[-.78+k*.24,h+1.75,4.86],[-.78+k*.24,h+1.75,5.34],.009,M.iron);
    // Shop-side goods stay in the shallow forecourt, leaving the main sidewalk continuous.
    if(index===2){for(const x of [-1.1,-.55,0])pot(g,x,.08,-.57,.20,true);}
    else {
      solid(g,'display-crate',-1.13,.25,-.49,.61,.40,.53,M.wood);
      for(let n=0;n<5;n++)box(g,-1.37+n*.12,.25,-.77,.07,.36,.025,M.interior);
      for(let n=0;n<8;n++)ball(g,-1.32+random()*.38,.50,-.64+random()*.27,.065,index===1?M.brass:M.leafLight);
      label(g,index===1?'焼きたて':'本日のおすすめ',index===1?'食パン  ¥280':'季節の味  ¥680',-.18,.63,-.49,.52,.76,'#324b47','#f0e0b9');
      rod(g,[-.49,.08,-.51],[-.49,1.1,-.36],.025,M.wood);rod(g,[.13,.08,-.51],[.13,1.1,-.36],.025,M.wood);
      colliders.push({source:'sakura-menu',min:new THREE.Vector3(spec.x-.51,0,g.position.z-.68),max:new THREE.Vector3(spec.x+.16,1.1,g.position.z-.28)});
    }
    label(g,'営業中','OPEN',1.40,1.55,-.14,.18,.45);
    label(g,'桜まつり','4.01 — 4.14',-1.39,1.3,-.16,.24,.34,'#f0d5d8','#915b66');
    mergeByMaterial(g);freezeStatic(g);
  });
  // Street furniture is authored independently so its collisions are not giant merged AABBs.
  const street=new THREE.Group();street.name='sakura-street-props';root.add(street);
  vending(street,35.45,15.66);
  bicycle(street,21.5,14.61,M.teal);bicycle(street,31.6,14.61,M.red);
  bicycle(street,-17.55,14.65,M.red);bicycle(street,-17.60,20.70,M.teal);
  for(const x of [20.05,27.24,36.8]){
    solid(street,'utility-pole',x,3.75,13.0,.17,7.5,.17,M.concrete);
    for(let band=0;band<8;band++)box(street,x,.55+band*.11,12.902,.19,.055,.014,band%2?M.black:M.yellow);
    box(street,x,6.84,13,1.24,.075,.12,M.iron);
    for(const dx of [-.48,0,.48])mesh(street,cylinder,M.paper,x+dx,6.99,13,.055,.20,.055);
    box(street,x,5.8,13,.45,.64,.40,M.roofEdge);
    label(street,'桜町','SAKURAMACHI',x,2.45,12.90,.14,.57,'#426882','#e4eaf0');
    // 禁停圆牌挂在东西两端电线杆的路面侧（参考图路牌位），中间一根留干净。
    if(x!==27.24){
      const rim=mesh(street,cylinder,M.signRed,x,2.72,12.895,.215,.03,.215);rim.rotation.x=Math.PI/2;
      const disc=mesh(street,cylinder,M.signBlue,x,2.72,12.868,.172,.026,.172);disc.rotation.x=Math.PI/2;
    }
  }
  for(const [a,b] of [[20.05,27.24],[27.24,36.8]])for(let wire=0;wire<3;wire++){
    const points=[];
    for(let n=0;n<=20;n++){const t=n/20;points.push(new THREE.Vector3(a+(b-a)*t,6.94-wire*.12-Math.sin(t*Math.PI)*.42,13+wire*.16));}
    mesh(street,geo(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(points),20,.012,4,false)),M.iron,0,0,0);
  }
  for(const x of [27.2,36.2]){
    solid(street,'bin',x,.37,15.6,.42,.72,.42,M.teal);
    box(street,x,.75,15.6,.46,.07,.46,M.iron);box(street,x,.61,15.37,.19,.09,.022,M.black);
  }
  // 立式告示板：参考图书店旁那块贴满传单的木框告示板。书店体量（x 12.7~19.7，
  // 立面顶到路缘）把西段人行道吃掉了，所以放在面包房前的空档：西邻自行车
  // 21.5（占到 22.34）、脚下是面包房外摆（z≥15.04），14.25~14.85 这条带是空的。
  {
    const bx=24.4,bz=14.5;
    for(const dx of [-.52,.52])rod(street,[bx+dx,.06,bz],[bx+dx,1.18,bz],.035,M.darkWood);
    box(street,bx,1.24,bz,1.26,.96,.08,M.wood);
    box(street,bx,1.78,bz,1.42,.07,.3,M.roof);
    box(street,bx,1.74,bz-.1,1.42,.05,.12,M.roofEdge);
    label(street,'掲示板','桜町 1丁目 町内会',bx,1.56,bz-.055,.86,.15,'#7a6a4f','#f0e6cf');
    label(street,'桜まつり','4.01 — 4.14',bx-.4,1.16,bz-.055,.34,.6,'#f2dcdd','#9c5f6a');
    label(street,'お知らせ','町内会 · 清掃の日',bx,1.16,bz-.055,.36,.6,'#e6ead4','#4a6657');
    label(street,'募集','店頭スタッフ',bx+.4,1.16,bz-.055,.34,.6,'#dfe5ef','#4c628c');
    colliders.push({source:'sakura-noticeboard-west',min:new THREE.Vector3(bx-.72,0,bz-.3),max:new THREE.Vector3(bx+.72,1.85,bz+.3)});
  }
  // 商店街导览牌：蓝底白字立杆（参考图路口的蓝色导览牌）。西段被书店/居酒屋
  // 占满，立在便利店门前斑马线（x≈1.1）东侧的人行道上——路口标牌的标准位，
  // 主视角可见。
  {
    const px=2.3,pz=13.3;
    solid(street,'guide-pole',px,1.05,pz,.09,2.1,.09,M.iron);
    label(street,'桜町商店街','SAKURA-SHOTENGAI',px,2.12,pz-.075,1.0,.42,'#3f6ea6','#eef2f6');
  }
  // Bookshop street display: individual illustrated covers, wire retainers and a chalkboard.
  function magazineRack(x:number,z:number){
    for(const dx of [-.65,.65])rod(street,[x+dx,.08,z+.25],[x+dx,1.48,z],.025,M.iron);
    for(let row=0;row<3;row++){
      const y=.36+row*.37,pz=z+.16-row*.07;
      box(street,x,y-.04,pz,1.4,.055,.23,M.paper);
      for(let col=0;col<4;col++){
        const px=x-.5+col*.335;
        label(street,['暮らし','花日和','旅の本','喫茶案内'][(row+col)%4],['SPRING 04','桜のある街','散歩日記'][row],px,y+.13,pz-.08,.30,.33,['#ecd8d9','#dbe7d8','#e3dcc7','#d7e1eb'][col],'#495b61');
        // Coloured illustration and printed rules give each cover a distinct composition.
        box(street,px,y+.14,pz-.089,.12,.075,.008,[M.teal,M.red,M.yellow,M.blueBook][(col+row)%4]);
      }
      rod(street,[x-.7,y+.045,pz-.11],[x+.7,y+.045,pz-.11],.012,M.iron);
    }
    label(street,'週刊誌・月刊誌','BOOKS & MAGAZINES',x,1.55,z-.11,1.45,.24,'#365e52','#f2eadb');
    colliders.push({source:'sakura-magazine-rack',kind:'wall',min:new THREE.Vector3(x-.74,0,z-.18),max:new THREE.Vector3(x+.74,1.7,z+.4)});
  }
  magazineRack(15.2,14.45);magazineRack(51.6,15.92);
  for(const x of [47.5,57.8,66.2,73.9]){
    // Planter boxes stay against the storefront; the curbside walking strip remains open.
    solid(street,'flower-box',x,.28,15.7,.72,.5,.42,M.wood);
    for(let slat=0;slat<5;slat++)box(street,x-.3+slat*.15,.3,15.475,.035,.38,.018,M.darkWood);
    for(let n=0;n<4;n++)pot(street,x-.24+n*.16,.54,15.7,.075,true);
    for(let crate=0;crate<2;crate++){
      const px=x+.7,py=.2+crate*.33;
      for(const dz of [-.18,.18]){box(street,px,py,15.7+dz,.48,.04,.035,M.teal);box(street,px,py+.24,15.7+dz,.48,.04,.035,M.teal);for(let j=0;j<5;j++)box(street,px-.22+j*.11,py+.12,15.7+dz,.025,.25,.025,M.teal);}
      for(const dx of [-.23,.23])box(street,px+dx,py+.12,15.7,.025,.26,.38,M.teal);
    }
  }
  for(const x of [47.5,61.8,76.7]){
    solid(street,'east-wire-pole',x,3.5,13.2,.16,7,.16,M.concrete);
    box(street,x,6.6,13.2,1.3,.08,.1,M.iron);
    for(let j=0;j<7;j++)box(street,x,.28+j*.14,13.105,.19,.08,.02,j%2?M.black:M.yellow);
    label(street,'桜まつり','SAKURA WALK',x,2.45,13.08,.25,.82,'#f4e7d9','#aa5767');
    box(street,x-.36,5.9,13.2,.4,.48,.32,M.roofEdge);
  }
  for(const [a,b] of [[36.8,47.5],[47.5,61.8],[61.8,76.7]])for(let i=0;i<3;i++){
    const points=Array.from({length:17},(_,n)=>{const t=n/16;return new THREE.Vector3(a+(b-a)*t,6.7-i*.13-Math.sin(t*Math.PI)*.45,13.2+i*.12);});
    mesh(street,geo(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(points),16,.01,4,false)),M.iron,0,0,0);
  }
  for(const x of [17.9,46.5,62,77]){
    box(street,x,.08,12.92,.64,.026,.24,M.iron);
    for(let k=0;k<7;k++)box(street,x-.27+k*.09,.096,12.92,.027,.014,.24,M.concrete);
  }
  mergeByMaterial(street);freezeStatic(street);

  /* 灌木的叶簇场（神社绿篱 / 庭院植栽）。
   * 用**自己的** PRNG：叶簇贴图生成要抽几百次样，蹭本模块的 `random()` 会把后面
   * 所有随机内容整体错位 —— 这条街的每一处落位都挂在同一条序列上。 */
  let shrubSeed=4409;const shrubRandom=()=>{shrubSeed=(Math.imul(shrubSeed,1664525)+1013904223)>>>0;return shrubSeed/4294967296;};
  const shrubs=createLeafField(shrubRandom,4000);

  // The former car park becomes a small neighborhood shrine, with an open central approach.
  const garden=new THREE.Group();garden.name='sakura-shrine-courtyard';root.add(garden);
  solid(garden,'garden-ground',-8.9,.015,20.25,6.1,.09,10.0,M.concrete,'floor');
  box(garden,-8.9,.068,20.25,5.9,.022,9.8,M.paper);
  for(let n=0;n<13;n++){
    const z=15.55+n*.56;
    box(garden,-9.1,.09,z,1.56,.05,.50,n%2?M.concrete:M.plaster);
    box(garden,-9.1,.12,z,.023,.015,.49,M.roofEdge);
  }
  for(const x of [-11.85,-5.96]){
    solid(garden,'garden-wall',x,.43,21.1,.17,.82,8.2,M.concrete);
    box(garden,x,.88,21.1,.26,.10,8.34,M.roof);
    for(let z=17.2;z<25;z+=.7)box(garden,x,.5,z,.20,.021,.017,M.roofEdge);
  }
  solid(garden,'garden-back',-8.9,.52,25.16,6.05,1,.18,M.concrete);
  /* 鸟居（2026-09-25 按参考图换色）：参考图是**鲜朱色 + 近黑笠木**，
   * 原来的 M.red(#ae534d) 偏砖红、M.darkWood 偏棕，门前主体不够跳。
   * 结构一个数没动（柱/贯/岛木/笠木/黑根卷），只换材质。 */
  for(const x of [-10.43,-7.77]){
    solid(garden,'torii-post',x,1.60,16.73,.24,3.08,.26,shrineRed);
    mesh(garden,cylinder,M.black,x,.28,16.73,.15,.42,.15);
    mesh(garden,cylinder,M.black,x,3.03,16.73,.18,.13,.18);
  }
  box(garden,-9.1,2.64,16.73,3.25,.15,.21,shrineRed);
  box(garden,-9.1,3.22,16.73,3.62,.23,.37,shrineRed);
  box(garden,-9.1,3.40,16.73,3.93,.16,.49,shrineCap);
  for(const s of [-1,1]){const cap=box(garden,-9.1+s*1.98,3.46,16.73,.35,.15,.49,shrineCap);cap.rotation.z=s*.19;}
  box(garden,-9.1,2.99,16.50,.37,.52,.08,M.brass);
  label(garden,'桜町','稲荷神社',-9.1,2.99,16.449,.31,.45,'#433d3c','#e5c481');
  for(let n=0;n<18;n++){
    const x=-10.25+n*.135,t=n/17;
    rod(garden,[x,2.58-Math.sin(t*Math.PI)*.24,16.53],[x+.13,2.58-Math.sin((n+1)/17*Math.PI)*.24,16.53],.028,M.brass);
  }
  for(const x of [-9.8,-9.35,-8.9,-8.45])for(let n=0;n<3;n++){
    const shide=box(garden,x+(n%2)*.055,2.24-n*.09,16.52,.075,.12,.014,M.paper);shide.rotation.z=n%2?.4:-.4;
  }
  // Tiny sanctuary with timber lattice, tiled canopy, offertory box and hanging bell rope.
  solid(garden,'shrine-base',-9.1,.24,23.0,2.64,.4,2.65,M.concrete);
  solid(garden,'sanctuary',-9.1,1.31,23.24,2.12,1.72,1.65,M.darkWood);
  for(let n=0;n<13;n++)box(garden,-10.01+n*.152,1.45,22.38,.032,1.19,.045,M.brass);
  for(const y of [.88,1.95])box(garden,-9.1,y,22.34,2.02,.065,.07,M.wood);
  for(const s of [-1,1]){
    const r=box(garden,-9.1+s*.66,2.39,23.15,1.58,.12,2.85,M.teal);r.rotation.z=-s*.38;
    for(let n=0;n<13;n++)rod(garden,[-9.1,2.7,21.79+n*.224],[-9.1+s*1.42,2.15,21.79+n*.224],.045,M.roofEdge);
  }
  rod(garden,[-9.1,2.76,21.7],[-9.1,2.76,24.62],.065,M.roof);
  solid(garden,'offering-box',-9.1,.67,21.99,1.11,.63,.48,M.wood);
  for(let n=0;n<9;n++)box(garden,-9.57+n*.117,1.01,21.99,.066,.035,.43,M.brass);
  rod(garden,[-9.1,2.55,22.08],[-9.1,1.16,22.08],.037,M.cream);ball(garden,-9.1,2.47,22.08,.12,M.brass);
  label(garden,'奉納','桜町 稲荷',-9.1,.71,21.739,.43,.31,'#785342','#f4ddb2');
  for(const x of [-10.77,-7.43])for(const z of [19.05,21.35]){
    solid(garden,'stone-lantern',x,.58,z,.22,1.03,.22,M.concrete);
    box(garden,x,.14,z,.50,.21,.5,M.concrete);box(garden,x,1.16,z,.43,.14,.43,M.plaster);
    box(garden,x,1.36,z,.27,.29,.27,M.lamp);
    for(const dx of [-.19,.19])for(const dz of [-.19,.19])box(garden,x+dx,1.36,z+dz,.065,.32,.065,M.concrete);
    const cap=mesh(garden,geo(new THREE.ConeGeometry(.43,.22,4)),M.roofEdge,x,1.65,z);cap.rotation.y=Math.PI/4;
    ball(garden,x,1.84,z,.09,M.concrete);
  }
  for(const x of [-11.2,-6.5])for(const z of [18.8,23.8])pot(garden,x,.09,z,.26,true);
  /* ── 门前街景（2026-09-25 按用户给的参考图还原）──────────────────────────
   * 参考图（楢ヶ丘稲荷神社）门前自西向东是：町内会公告板 → 玉垣矮墙 → 社号标石柱 →
   * 鸟居 → 玉垣 → 黄黑斜纹电线杆（挂消火栓/駅前東2 牌）→ 邻家。
   * 原场景只有东西两道**侧**墙（z 17..25.2），正面 z15.25 整条敞开、没有玉垣，
   * 没有社号标，门前也没有那根杆。下面按这个次序补齐。
   *
   * 三条不动的约束（都是本仓库既有的）：
   *   1) 便利店那两台售货机的碰撞盒是 `tmp/_verify-vending-colliders.mjs` 钉死的
   *      （断言 A 恰好 4 个 streetfurn-*、B2 柜面外 0.149m 才被挡）——新加的东西
   *      一律不许碰到 x -6.29..-4.05 / z 12.92..13.68 这块。
   *   2) 预览页自检沿 x=-9.1 走 z15.25..21.3 查参道通不通，另外 6 间店铺各走一条；
   *      新加的碰撞盒不能落在这 7 条线上。
   *   3) 「细杆件不登记碰撞盒」是本仓库惯例（路牌柱 / 公交牌柱都这样），门前电线杆照办。
   */
  // ① 玉垣：沿庭院正面 z15.18 的两段矮墙，中间留出参道口（鸟居柱 x-10.55 / -7.65 之间）。
  const fenceZ=15.18;
  for(const [fx0,fx1] of [[-11.9,-11.02],[-7.5,-5.95]]){
    box(garden,(fx0+fx1)/2,.17,fenceZ,fx1-fx0,.30,.16,M.concrete);
    box(garden,(fx0+fx1)/2,.36,fenceZ,fx1-fx0,.08,.22,M.plaster);
    for(let px=fx0+.16;px<fx1-.06;px+=.62){
      box(garden,px,.32,fenceZ,.20,.62,.20,M.concrete);
      box(garden,px,.65,fenceZ,.24,.06,.24,M.plaster);
    }
    colliders.push({source:'sakura-front-fence',kind:'wall',
      min:new THREE.Vector3(fx0,0,fenceZ-.11),max:new THREE.Vector3(fx1,.66,fenceZ+.11)});
  }
  // ② 社号标：鸟居西柱外那根花岗岩石柱（参考图「楢ヶ丘稲荷神社」那块）。
  //    本町叫桜町，字沿用鸟居铜牌上的「桜町稲荷神社」，不另起一个地名。
  box(garden,-10.78,.17,15.42,.52,.18,.38,M.concrete);
  box(garden,-10.78,.97,15.42,.34,1.42,.24,granite);
  box(garden,-10.78,1.72,15.42,.40,.08,.28,M.concrete);
  vlabel(garden,'桜町稲荷神社',-10.78,.97,15.298,.26,1.16,'#b7b5ae','#3a3a38');
  colliders.push({source:'sakura-name-post',kind:'wall',
    min:new THREE.Vector3(-11.04,0,15.23),max:new THREE.Vector3(-10.52,1.76,15.61)});
  // ③ 参道口的两级踏石（人行道 y0.028 → 庭院 y0.08）。矮，登记成 floor。
  solid(garden,'shrine-step',-9.1,.025,15.28,2.44,.05,.44,M.concrete,'floor');
  box(garden,-9.1,.075,15.44,2.44,.05,.30,M.concrete);
  // ④ 门内东侧那道修剪过的绿篱（参考图鸟居右手边那丛深绿）。
  //    冠簇走叶簇场：原来顶上是两颗 `IcosahedronGeometry` 球，一丛四个球连起来
  //    读作"一排绿气球"。芯（那个方箱）留着 —— 面片没有厚度，全靠它会看穿。
  for(let n=0;n<4;n++){
    const hz=17.0+n*.44;
    box(garden,-6.7,.42,hz,1.0,.68,.42,M.leaf);
    for(let k=0;k<10;k++){
      const px=-7.24+shrubRandom()*1.10,pz=hz+(shrubRandom()-.5)*.42;
      shrubs.spray(px,.54+shrubRandom()*.52,pz,.19+shrubRandom()*.13);
    }
  }
  colliders.push({source:'sakura-hedge',kind:'wall',
    min:new THREE.Vector3(-7.22,0,16.88),max:new THREE.Vector3(-6.18,.78,18.42)});
  // ⑤ 门内那面红底白字的幟（参考图右侧那条竖幡）。
  rod(garden,[-7.9,.08,20.5],[-7.9,2.72,20.5],.035,M.iron);
  rod(garden,[-7.9,2.68,20.5],[-7.9,2.68,20.22],.018,M.iron);
  vlabel(garden,'稲荷神社',-7.9,1.84,20.45,.30,1.42,'#c0392b','#f6efe0');
  // ⑥ 门前电线杆：混凝土杆 + 根部黄黑斜纹警示套 + 消火栓 / 取扱注意 / 駅前東2 三块牌。
  //    立在神社东侧人行道 x-6.6 / z14.1 —— 参考图里杆就在门前右手边、离镜头最近。
  //    位置核对：售货机占 x-6.29..-4.05 / z12.92..13.68（本杆 x-6.73..-6.47、z14.1
  //    与它 z 向差 0.42m，完全避开）；公交牌柱在 (-7.8,13)，隔 1.63m；庭院东墙
  //    x-6.04..-5.88 只在 z17 之后。按惯例**不登记碰撞盒**。
  const POLE_X=-6.6,POLE_Z=14.1;
  mesh(garden,cylinder,poleGrey,POLE_X,4.6,POLE_Z,.13,9.2,.13);
  mesh(garden,cylinder,poleGrey,POLE_X,.1,POLE_Z,.18,.2,.18);
  mesh(garden,cylinder,hazardBand(),POLE_X,1.1,POLE_Z,.137,1.9,.137);
  vlabel(garden,'消火栓',POLE_X,3.20,POLE_Z-.134,.30,.76,'#c0392b','#fbf6ef');
  label(garden,'マンホール','取扱注意',POLE_X,2.66,POLE_Z-.134,.30,.26,'#f2ece2','#b8382f');
  vlabel(garden,'駅前東2',POLE_X,2.18,POLE_Z-.134,.24,.62,'#f4f2ec','#2b3a44');
  /* ⑦ 町内会公告板（原有，2026-09-25 改动）：
   *    从 z15.43 **前移到 z14.78** —— 参考图里公告板站在玉垣**前面**（靠街一侧），
   *    原来的位置在庭院边缘、被新加的玉垣挡在身后。
   *    同时从 x-11.2 **西移到 x-11.55**：社号标立在 x-10.78，参考图机位下两者只差
   *    2°，板子会盖掉石柱右半边；西移 0.35m 后两者分开 1.2° 以上。
   *    另外按参考图补到 4 张告示 + 顶部那条「桜ヶ丘町内会 掲示板」标题条。 */
  for(const x of [-12.05,-11.05])rod(garden,[x,.06,14.78],[x,2.1,14.78],.05,M.darkWood);
  box(garden,-11.55,1.6,14.78,1.15,.94,.10,M.wood);
  label(garden,'桜まつり','4月 · 町内会',-11.76,1.66,14.719,.49,.56,'#f5dfdf','#ac646f');
  label(garden,'お知らせ','清掃の日',-11.23,1.66,14.719,.43,.56,'#e4e8d3','#46665b');
  label(garden,'夏まつり','7月 · 盆踊り',-11.76,1.24,14.719,.46,.22,'#eaf3e6','#4e7a55');
  label(garden,'当番表','8月 · 回覧',-11.23,1.24,14.719,.40,.22,'#f7f2e6','#8a7a52');
  label(garden,'桜ヶ丘町内会','掲示板',-11.55,2.00,14.719,.62,.13,'#f6efe0','#6a5a48');
  box(garden,-11.55,2.14,14.75,1.36,.08,.42,M.roof);
  colliders.push({source:'sakura-noticeboard',min:new THREE.Vector3(-12.17,0,14.54),max:new THREE.Vector3(-10.90,2.18,14.99)});
  mergeByMaterial(garden);freezeStatic(garden);

  // Branching cherry trees. Hundreds of small, irregular blossoms replace smooth pink blobs.
  const treePositions: Array<[number,number]>=[[-11.6,13.4],[19.55,13.45],[27.20,14.0],[36.9,15.1],[-6.85,22.8],[48.2,13.5],[57,13.5],[66.6,13.5],[75.9,13.5],[54.3,36.5],[31,36.5]];
  // Cutout flower sprays give the canopy fine, airy edges instead of faceted pink boulders.
  // 贴图与实例收集器来自 foliage —— 社区绿地与市政行道树用的是同一套
  // （见 urbanStreets），免得几处树冠各长各的、从阳台望出去一中景就露馅。
  const field=createBlossomField(random,treePositions.length*640);
  materials.push(field.material);textures.push(field.texture);geometries.add(field.geometry);
  /* 树干走 Blender 资产（`foliage` 的干场），和河堤/行道树/公园老樱同一套几何。
   * 商店街的 11 棵是**近景**，玩家在街上走的时候就在头顶上，所以这里的树干比
   * 别处更值得换成有板根和树皮棱沟的模型 —— 原来那根是缩放的圆柱。
   * 干场最后统一 build：每棵树自己 mergeByMaterial 之后没法再往里塞实例网格。 */
  const trunks=createTrunkField();
  treePositions.forEach(([x,z],treeIndex)=>{
    const tree=new THREE.Group();tree.name=`sakura-tree-${treeIndex}`;root.add(tree);
    solid(tree,'tree-bed',x,.15,z,1.1,.3,1.0,M.concrete);box(tree,x,.307,z,.94,.025,.84,M.soil);
    // 原来是 `rod(tree,[x,.2,z],[x+.16,2.75,z+.04],.14,M.darkWood)`：起讫点原样
    // 搬过来（h = Δy、lean/leanDir = 水平偏移），所以树形与枝条着生点没动。
    trunks.add({x,z,y0:.2,h:2.55,r:.14,variant:'sakura',
      lean:Math.hypot(.16,.04),leanDir:Math.atan2(.04,.16),seed:treeIndex});
    colliders.push({source:'sakura-tree-trunk',min:new THREE.Vector3(x-.18,.2,z-.18),max:new THREE.Vector3(x+.32,2.9,z+.22)});
    for(let branch=0;branch<8;branch++){
      const a=branch*2.399+treeIndex,reach=1.05+random()*.7;
      const tip=[x+Math.cos(a)*reach,3.6+random()*1.4,z+Math.sin(a)*reach];
      const joint=[x+Math.cos(a)*.6,2.8+random()*.6,z+Math.sin(a)*.6];
      rod(tree,[x+.16,1.6+branch*.12,z+.04],joint,.065,M.darkWood);rod(tree,joint,tip,.032,M.darkWood);
      for(let twig=0;twig<3;twig++)rod(tree,tip,[tip[0]+Math.sin(twig*2+a)*.6,tip[1]+.40,tip[2]+Math.cos(twig*2+a)*.6],.012,M.darkWood);
      for(let n=0;n<80;n++){
        const theta=random()*Math.PI*2,r=Math.sqrt(random())*.99;
        field.spray(tip[0]+Math.cos(theta)*r,tip[1]+(random()-.5)*.83,tip[2]+Math.sin(theta)*r,.18+random()*.20);
      }
    }
    mergeByMaterial(tree);freezeStatic(tree);
  });
  root.add(trunks.build('sakura-tree-trunks',M.darkWood.color));
  root.add(field.build('sakura-blossom-clusters'));
  const fallenField=createFallenPetals(random,treePositions.length*550);
  materials.push(fallenField.material);geometries.add(fallenField.geometry);
  treePositions.forEach(([x,z])=>{fallenField.scatter(x,z,.082,2.6,350);fallenField.scatter(x+.9,z-1.1,.083,1.15,200);});
  root.add(fallenField.build('sakura-fallen-petals'));
  // 灌木叶簇同样要等合批之后再挂（InstancedMesh 会被 mergeByMaterial 烘掉）。
  root.add(shrubs.build('sakura-shrub-leaves'));
  // 风中飘落的花瓣与地面落花共用同一张花瓣几何/材质（原实现也是共享的）。
  const drifting=new THREE.InstancedMesh(fallenField.geometry,fallenField.material,120);
  drifting.name='sakura-wind-petals';drifting.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
  drifting.frustumCulled=false;drifting.userData.noMerge=true;drifting.userData.sceneCollideSkip=true;root.add(drifting);
  const dummy=new THREE.Object3D();
  let wind=1;
  return {
    colliders,
    setEnvironment(period:string,weather:string){
      M.lamp.emissiveIntensity=period==='night'?1.3:period==='dusk'?.8:.35;
      M.redLamp.emissiveIntensity=period==='night'?.65:.20;
      for(const light of shopLights)light.intensity=period==='night'?13:period==='dusk'?10:7;
      wind=weather==='storm'?2.4:1;
    },
    update(t:number){
      for(let i=0;i<120;i++){
        const [x,z]=treePositions[i%treePositions.length],phase=i*2.399+t*.7*wind;
        const y=.12+((i*.731-t*.35*wind)%4.8+4.8)%4.8;
        dummy.position.set(x+Math.sin(phase*.33)*2.25,y,z+Math.cos(phase*.27)*1.7);
        dummy.rotation.set(phase,phase*.73,phase*.48);dummy.scale.setScalar(.06+(i%4)*.018);dummy.updateMatrix();drifting.setMatrixAt(i,dummy.matrix);
      }drifting.instanceMatrix.needsUpdate=true;
    },
    dispose(){
      root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);if(o instanceof THREE.InstancedMesh)o.dispose();});
      for(const g of geometries)g.dispose();materials.forEach(m=>m.dispose());textures.forEach(t=>t.dispose());
      shrubs.material.dispose();shrubs.texture.dispose();root.removeFromParent();
    },
  };
}
