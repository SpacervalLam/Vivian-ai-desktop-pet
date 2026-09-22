import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import type { BoxColliderSpec } from './collider';

/**
 * Runtime handle for the south entrance's motion-sensing door.
 *
 * Only geometry is built here; every opening decision stays with the caller —
 * the trigger radius has to account for the player, the agents and the scene
 * assembly order, none of which belong in a pure builder.
 */
export type PodiumAutoDoor = {
  /** Whole group (two leaves + lintel sensor + status strip). Add as-is; never merge or freeze. */
  group: THREE.Group;
  /** The two leaves, left then right. Sliding only writes position.x. */
  leaves: [THREE.Object3D, THREE.Object3D];
  /** Leaf centre x when closed, matching `leaves` order */
  closeX: [number, number];
  /** Leaf centre x when fully open */
  openX: [number, number];
  /** Portal centre (world xz) and clear height band: used to gate the trigger */
  x: number;
  z: number;
  y0: number;
  y1: number;
  /** Trigger radius in metres: a walker inside this horizontal distance opens the door */
  senseRadius: number;
  /** Status strip material: lit while the door is passing someone through */
  indicator: THREE.MeshStandardMaterial;
};

/** A continuous residential podium: stone portals, bronze glazing and sheltered gardens.
 * All coordinates are world coordinates. Upper-floor slabs start at 3.4 m.
 * The south entrance is a pair of motion-sensing sliding leaves; glazing and
 * service rooms remain non-traversable. */
export function buildApartmentPodium() {
  const group = new THREE.Group(); group.name = 'apartment-residential-podium';
  group.userData.sceneCollideSkip = true;
  const boxes: BoxColliderSpec[] = [];
  let seed=1923;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  const canvas=document.createElement('canvas');canvas.width=canvas.height=256;
  const ctx=canvas.getContext('2d')!;ctx.fillStyle='#e1d8c8';ctx.fillRect(0,0,256,256);
  for(let i=0;i<6000;i++){ctx.fillStyle=`rgba(105,91,68,${random()*.09})`;ctx.fillRect(random()*256,random()*256,1+random()*2,.5+random());}
  for(let y=0;y<256;y+=9){ctx.fillStyle=`rgba(151,129,100,${.02+random()*.03})`;ctx.fillRect(0,y,256,.5);}
  const stoneMap=new THREE.CanvasTexture(canvas);stoneMap.colorSpace=THREE.SRGBColorSpace;stoneMap.anisotropy=4;
  const mat=(name:string,color:string,roughness=.8,metalness=0)=>{const m=new THREE.MeshStandardMaterial({color,roughness,metalness});m.name=`podium/${name}`;m.userData.outlineWeight=0;return m;};
  const stone=mat('honed limestone','#ffffff');stone.map=stoneMap;stone.bumpMap=stoneMap;stone.bumpScale=.003;
  const dark=mat('basalt','#535550'),bronze=mat('brushed bronze','#6d6252',.4,.65),wood=mat('smoked oak','#92704d');
  const plaster=mat('lime plaster','#ddd5c5',.96),linen=mat('boucle','#c9bc9f',1),sage=mat('sage upholstery','#778776',.98);
  const soil=mat('gravel','#78786a'),leaves=mat('evergreen','#455e42',.95);
  const rug=mat('wool rug','#6b6154',.99);
  /* 橡木饰面（北墙整面用）。
   * 横向细纹 + 稀疏的深色年轮线，暖中棕底。做成横向而不是竖向：北墙是一条 62m
   * 的长墙，横纹把它读成"一片连续的木板墙"；竖纹会把它切成无数条窄板，反而更碎。
   * 用独立随机流，免得污染 stoneMap 之后那条主序列（灌木的位置与大小还跟着它走）。 */
  let oakSeed=4211;const oakRnd=()=>{oakSeed=(Math.imul(oakSeed,1664525)+1013904223)>>>0;return oakSeed/4294967296;};
  const oakCanvas=document.createElement('canvas');oakCanvas.width=oakCanvas.height=256;
  const ox=oakCanvas.getContext('2d')!;
  ox.fillStyle='#b08a62';ox.fillRect(0,0,256,256);
  for(let i=0;i<240;i++){
    ox.fillStyle=`rgba(96,68,42,${(.05+oakRnd()*.15).toFixed(3)})`;
    ox.fillRect(0,oakRnd()*256,256,.6+oakRnd()*2.4);
  }
  for(let i=0;i<900;i++){
    ox.fillStyle=`rgba(${oakRnd()<.5?'150,116,82':'84,58,34'},${(.03+oakRnd()*.07).toFixed(3)})`;
    ox.fillRect(oakRnd()*256,oakRnd()*256,2+oakRnd()*14,1);
  }
  const oakMap=new THREE.CanvasTexture(oakCanvas);oakMap.colorSpace=THREE.SRGBColorSpace;oakMap.anisotropy=4;
  const oak=mat('oak veneer','#ffffff',.72);oak.map=oakMap;oak.bumpMap=oakMap;oak.bumpScale=.002;
  const glass=mat('clear glazing','#bfd4cd',.16,.12);glass.transparent=true;glass.opacity=.14;glass.depthWrite=false;glass.side=THREE.DoubleSide;
  const warm=mat('warm diffusers','#ffdfa6',.8);warm.emissive.set('#ffd199');warm.emissiveIntensity=1.2;
  const geometry=new Map<string,THREE.BufferGeometry>();
  function box(name:string,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material,solid=false,round=false){
    const key=`${w}/${h}/${d}/${round}`;let geo=geometry.get(key);
    if(!geo){geo=round?new RoundedBoxGeometry(w,h,d,2,Math.min(.06,w*.12,h*.12,d*.12)):new THREE.BoxGeometry(w,h,d);geometry.set(key,geo);}
    const mesh=new THREE.Mesh(geo,m);mesh.name=`podium-${name}`;mesh.position.set(x,y,z);mesh.castShadow=true;mesh.receiveShadow=true;mesh.userData.noOutline=true;group.add(mesh);
    if(solid)boxes.push({id:mesh.name,pos:[x,y,z],size:[w,h,d]});return mesh;
  }
  function pane(x:number,y:number,z:number,w:number,h:number){const mesh=new THREE.Mesh(new THREE.PlaneGeometry(w,h),glass);mesh.name='podium-glass';mesh.position.set(x,y,z);mesh.userData.noOutline=true;group.add(mesh);}
  function label(text:string,x:number,y:number,z:number,w:number,h:number,back=false){
    const c=document.createElement('canvas');c.width=1024;c.height=128;const t=c.getContext('2d')!;
    t.fillStyle='#514d44';t.fillRect(0,0,1024,128);t.fillStyle='#e9d9b8';t.font='500 52px Georgia, serif';t.textAlign='center';t.textBaseline='middle';t.fillText(text,512,65,950);
    const map=new THREE.CanvasTexture(c);map.colorSpace=THREE.SRGBColorSpace;
    const m=mat('wayfinding','#ffffff');m.map=map;m.emissive.set('#c9b18b');m.emissiveMap=map;m.emissiveIntensity=.18;
    const mesh=new THREE.Mesh(new THREE.PlaneGeometry(w,h),m);mesh.position.set(x,y,z);if(back)mesh.rotation.y=Math.PI;mesh.name='podium-sign';group.add(mesh);
  }
  /**
   * 一张软座（扶手椅 / 小沙发）。
   *
   * `dir` 是**靠背所在的那一侧**：dir=1 靠背在 -z（人朝 +z），dir=-1 靠背在 +z（人朝 -z）。
   * 别小看这个参数——茶几/餐桌摆在座位的哪一侧，靠背就得朝另一边，
   * 否则整组座位是"背对桌子"坐的（咖啡座区原来就是这个毛病）。
   */
  function seat(x:number,z:number,width=1.8,upholstery:THREE.Material=linen,collision=false,dir:1|-1=1){
    box('seat-base',x,.26,z,width-.12,.32,.64,wood,collision,true);
    box('seat-cushion',x,.49,z,width,.14,.72,upholstery,false,true);
    box('seat-back',x,.82,z-dir*.31,width-.06,.48,.16,upholstery,collision,true);
    for(const s of [-1,1])box('seat-arm',x+s*(width/2-.04),.67,z,.09,.28,.66,wood,false,true);
    // 靠枕：软性元素是"这里能坐下来待一会儿"的第一眼信号，也是这类空间最缺的东西
    for(const s of [-1,1])box('seat-pillow',x+s*(width*.30),.605,z-dir*.19,.36,.26,.13,sage,false,true);
  }
  const leafGeo=new THREE.SphereGeometry(1,9,7);
  function planter(x:number,z:number,w:number,d=.68){
    box('planter',x,.27,z,w,.46,d,dark,true,true);
    box('gravel',x,.51,z,w-.12,.016,d-.12,soil);
    for(let i=0;i<Math.ceil(w/.3);i++){
      const mesh=new THREE.Mesh(leafGeo,leaves);mesh.position.set(x-w/2+.18+i*.29,.66+random()*.12,z+(random()-.5)*.12);mesh.scale.set(.24,.25+random()*.12,.22);mesh.castShadow=true;mesh.name='podium-shrub';group.add(mesh);
    }
  }
  function sconce(x:number,z:number,back=false){
    const s=back?-1:1;
    box('sconce-body',x,1.95,z,.13,.52,.10,bronze);
    box('sconce-diffuser',x,1.95,z+s*.065,.072,.40,.025,warm);
  }
  // Four generously glazed communal bays. New wall cavities are 2.4 m deep.
  for(const x of [-24.8,-12.4,12.4,24.8]){
    // 12 mm masonry joints are physical gaps, not coplanar line decals.
    for(const dx of [-5.17,5.17])for(let row=0;row<4;row++)
      box('stone-pier',x+dx,.405+row*.705,4.70,2.02,.693,.24,stone);
    box('stone-lintel',x,3.025,4.70,12.30,.31,.24,stone);
    box('stone-plinth',x,.16,4.70,8.26,.22,.24,dark);
    box('bronze-head',x,2.822,4.60,8.22,.056,.09,bronze);
    box('bronze-sill',x,.314,4.60,8.22,.056,.09,bronze);
    for(let i=0;i<5;i++)box('window-mullion',x-4.08+i*2.04,1.568,4.60,.044,2.452,.09,bronze);
    for(let i=0;i<4;i++)pane(x-3.06+i*2.04,1.568,4.586,1.996,2.452);
    sconce(x+5.17,4.875);
  }
  // Central entry: a real 4.3 m deep lobby and a 2.4 m clear passage.
  // A single continuous ground floor, independent of the five apartments above.
  box('lobby-floor',0,.037,-.55,62,.05,10.54,stone,true);
  box('lobby-ceiling',0,3.265,-.55,61.98,.07,10.48,plaster);
  /* —— 木条格栅吊顶：铺满整个一层 ——
   * 抬头看到的应该是木头，而不是一整片白光石膏——这是"这家店装修过"
   * 和"这是商场预留空间"的分界。条沿 z 向（垂直于 62m 的长向）等距排开、背后压
   * 一层深色背板，缝里透出影，比一整片平木顶有层次。
   * 条底 3.155，比原石膏顶低约 7cm，1F 净高仍有余量。
   * 铺满之后不再需要端部收边——原来那两条铜边是给"半截吊顶"收口的。 */
  for(let x=-30.9;x<=30.9;x+=.45){
    box('lobby-ceiling-slat',x,3.19,-.55,.30,.07,10.3,oak);
  }
  box('lobby-ceiling-back',0,3.22,-.55,62,.02,10.4,dark);
  box('entrance-header',0,2.985,4.70,12.3,.39,.24,stone);
  for(const x of [-6.04,-1.25,1.25,6.04])box('entry-jamb',x,1.43,4.63,.07,2.72,.13,bronze);
  for(const s of [-1,1]){
    box('entry-glass-head',s*3.645,2.755,4.63,4.72,.055,.10,bronze);
    box('entry-glass-foot',s*3.645,.105,4.63,4.72,.055,.10,bronze);
    pane(s*3.645,1.43,4.609,4.72,2.595);
    box('portal-cheek',s*5.86,1.46,5.11,.22,2.78,.78,stone,true);
  }
  // Head track the leaves run on: one rail spanning the portal and the parked
  // positions, so an open leaf still reads as "riding on something".
  box('entry-track',0,2.715,4.49,5.30,.09,.15,bronze);
  // The two leaves are built outside `group`: they move every frame, and the
  // caller merges + freezes the shell, which would weld them shut.
  const doorW=1.22,doorH=2.58,doorZ=4.49,doorCloseX=.61,doorOpenX=1.83;
  const autoDoorGroup=new THREE.Group();autoDoorGroup.name='apartment-auto-door';
  const leafGlass=new THREE.BoxGeometry(doorW,doorH,.024);
  const leafStile=new THREE.BoxGeometry(.05,doorH,.055);
  const leafRail=new THREE.BoxGeometry(doorW,.05,.055);
  const leafPull=new THREE.BoxGeometry(.035,.72,.075);
  const doorLeaves:[THREE.Object3D,THREE.Object3D]=[] as unknown as [THREE.Object3D,THREE.Object3D];
  for(const s of [-1,1] as const){
    const leaf=new THREE.Group();leaf.name='podium-auto-door-leaf';
    const glazing=new THREE.Mesh(leafGlass,glass);glazing.position.y=doorH/2;glazing.userData.noOutline=true;leaf.add(glazing);
    for(const dx of [-1,1]){
      const stile=new THREE.Mesh(leafStile,bronze);stile.position.set(dx*(doorW/2-.025),doorH/2,0);stile.userData.noOutline=true;leaf.add(stile);
    }
    for(const dy of [.025,doorH-.025]){
      const rail=new THREE.Mesh(leafRail,bronze);rail.position.set(0,dy,0);rail.userData.noOutline=true;leaf.add(rail);
    }
    // Pull bar on the meeting edge: the side that closes against the other leaf.
    const pull=new THREE.Mesh(leafPull,bronze);pull.position.set(-s*(doorW/2-.075),1.15,.05);pull.userData.noOutline=true;leaf.add(pull);
    leaf.position.set(s*doorCloseX,0,doorZ);
    doorLeaves[s<0?0:1]=leaf;
    autoDoorGroup.add(leaf);
  }
  // Lintel sensor housing plus the status strip that lights up while passing.
  // Both sit below the head track (y 2.67..2.76) so nothing interpenetrates.
  const sensorLamp=mat('sensor lamp','#7fae8c',.5,.2);sensorLamp.emissive.set('#2c6644');sensorLamp.emissiveIntensity=.35;
  {
    const housing=new THREE.Mesh(new THREE.BoxGeometry(.62,.10,.13),bronze);housing.position.set(0,2.56,4.42);housing.userData.noOutline=true;autoDoorGroup.add(housing);
    const strip=new THREE.Mesh(new THREE.BoxGeometry(2.42,.028,.05),sensorLamp);strip.position.set(0,2.535,4.325);strip.userData.noOutline=true;autoDoorGroup.add(strip);
  }
  const autoDoor:PodiumAutoDoor={
    group:autoDoorGroup,
    leaves:doorLeaves,
    closeX:[-doorCloseX,doorCloseX],
    openX:[-doorOpenX,doorOpenX],
    x:0,z:doorZ,y0:0,y1:2.79,
    senseRadius:2.9,
    indicator:sensorLamp,
  };
  box('canopy',0,3.11,5.55,11.96,.15,1.68,bronze);
  for(let x=-5.78;x<5.8;x+=.16)box('canopy-oak',x,2.995,5.55,.10,.055,1.55,wood);
  for(const x of [-4.6,4.6])box('canopy-light',x,2.948,5.55,.055,.018,1.40,warm);
  // 雨棚檐口原来挂着一块「V I V I A N / R E S I D E N C E S」牌匾（z=6.401，贴在
  // canopy 外皮 6.39 之外 11mm）。用户要求去掉，檐口恢复成一道干净的铜色压边。
  // 牌匾是零厚度 PlaneGeometry、不参与碰撞，删掉不牵连任何碰撞盒或立面构件。
  // 仍要写字的另外四处（POST/PARCEL、RESIDENT STAIRS、STARBUCKS、LIFT 1—4）
  // 继续用同一个 label()。
  // Elevator, recessed mail cabinetry and a concierge counter behind the glazing.
  for(let col=0;col<6;col++)for(let row=0;row<4;row++){
    const x=-5.35+col*.49,y=.70+row*.37;
    box('mail-front',x,y,-5.58,.472,.35,.15,wood,true);
    box('mail-slot',x,y+.09,-5.495,.23,.014,.016,bronze);
  }
  label('POST  /  PARCEL',-4.12,2.47,-5.635,2.6,.18);
  box('concierge-desk',3.63,.57,1.85,2.48,1.02,.65,wood,true,true);
  box('concierge-top',3.63,1.10,1.85,2.56,.045,.72,stone,false,true);
  box('desk-screen',4.21,1.28,1.80,.34,.29,.05,bronze);
  for(let i=0;i<22;i++)box('desk-flute',2.47+i*.11,.58,2.192,.045,.84,.025,wood);
  seat(-3.84,3.23,2.15,linen,true);
  for(const x of [-3.4,3.4]){
    box('lobby-light',x,3.20,2.28,3.75,.025,.08,warm);
    const light=new THREE.PointLight('#ffe0b0',8,7,2);light.position.set(x,2.75,2.45);group.add(light);
  }
  // A level entry apron and landscaped pockets keep the public sidewalk unobstructed.
  box('entry-apron',0,.026,5.63,11.4,.035,1.50,stone,true);
  for(const x of [-8.0,8.0,-20.5,20.5])planter(x,5.44,2.8);
  for(const x of [-15.5,15.5]){
    box('outdoor-bench-base',x,.22,5.42,2.3,.35,.56,dark,true);
    for(let i=0;i<5;i++)box('outdoor-bench-slat',x,.425,5.18+i*.12,2.4,.05,.09,wood);
  }
  // Two small multi-stem garden trees sit within the entrance planting pockets.
  const branchGeo=new THREE.CylinderGeometry(.022,.035,1,7);
  for(const x of [-8,8]){
    for(let i=0;i<3;i++){
      const a=new THREE.Vector3(x,.52,5.44),b=new THREE.Vector3(x+(i-1)*.30,1.80+i*.13,5.44+(i%2-.5)*.28);
      const stem=new THREE.Mesh(branchGeo,wood);stem.position.copy(a).add(b).multiplyScalar(.5);stem.scale.y=a.distanceTo(b);stem.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),b.clone().sub(a).normalize());stem.name='podium-garden-stem';group.add(stem);
      for(let k=0;k<4;k++){
        const crown=new THREE.Mesh(leafGeo,leaves);crown.position.set(b.x+Math.cos(k*2.4)*.26,b.y+.13+Math.sin(k*2.4)*.16,b.z+Math.sin(k*2.4)*.23);crown.scale.set(.34,.18,.30);crown.castShadow=true;crown.name='podium-garden-canopy';group.add(crown);
      }
    }
  }
  // Quiet rear elevation: a continuous oak-faced wall with high privacy windows.
  // These shallow cavities are not furnished or advertised as full amenity rooms.
  const privacy=mat('reeded privacy glass','#91a49b',.56,.08);
  const paving=mat('rear sandstone pavers','#b6b1a2',.94);
  function rearWall(x0:number,x1:number,y0:number,y1:number){
    const cols=Math.ceil((x1-x0)/1.55),rows=Math.ceil((y1-y0)/.72);
    const w=(x1-x0)/cols,h=(y1-y0)/rows;
    for(let c=0;c<cols;c++)for(let r=0;r<rows;r++)box('rear-oak-panel',x0+(c+.5)*w,y0+(r+.5)*h,-5.88,w-.012,h-.012,.20,oak);
  }
  for(let unit=0;unit<5;unit++){
    const x=-24.8+unit*12.4,left=x-6.19,right=x+6.19;

    if(unit===1||unit===3){
      // High-window bays keep their two lights — they are the only openings in this wall.
      const openings=[x-3.15,x+3.15];
      let from=left;
      for(const wx of openings){
        rearWall(from,wx-1.37,.29,3.18);
        rearWall(wx-1.37,wx+1.37,.29,1.62);
        rearWall(wx-1.37,wx+1.37,2.61,3.18);
        for(const xx of [wx-1.337,wx,wx+1.337])box('rear-high-window-frame',xx,2.115,-5.835,.045,.896,.09,bronze);
        for(const yy of [1.642,2.588])box('rear-high-window-edge',wx,yy,-5.835,2.72,.045,.09,bronze);
        for(const sign of [-1,1])box('rear-privacy-pane',wx+sign*.6685,2.115,-5.819,1.292,.90,.014,privacy);
        box('rear-window-sill',wx,1.589,-5.925,2.83,.038,.22,dark);
        from=wx+1.37;
      }
      rearWall(from,right,.29,3.18);
    }else{
      /* —— 其余三个开间：整面橡木实墙 ——
       * 原先这里画的是"带百叶的闭门 + 门框 + 门槛 + 门楣雨庇"，但那是**假门**：
       * 它不通任何地方，进不去也出不来，唯一的效果是把北墙读成一排服务门。
       * 现在整段封成实墙——北墙回归成一面完整的橡木背景墙，咖啡吧才有背景可挂。 */
      rearWall(left,right,.29,3.18);
      // 壁灯保留：这三个开间不再有门，墙面照明只剩它
      sconce(left+.70,-6.045,true);
      sconce(right-.70,-6.045,true);
    }
    // 勒脚通长，不再被"门洞"打断
    box('rear-base',x,.16,-5.90,12.36,.22,.22,dark);
  }
  /* 北墙上唯一那个洞口（原 NORTH ENTRY，x∈[-1.2,1.2]）随这次封墙一并堵上。
   * 楼体碰撞表（APT_WALL_BOXES 的 apt-north-1f-*）在那一段是留空的，不补的话
   * 玩家会从"看上去是墙"的地方直接穿出楼外。这里只补碰撞盒、不建 mesh——
   * 墙面本身已经由上面的 rearWall 铺满了。 */
  boxes.push({id:'north-seal',pos:[0,1.65,-5.88],size:[2.54,3.30,.24]});
  box('rear-top-shadow',0,3.255,-5.90,61.98,.138,.19,dark);
  box('rear-cavity-ceiling',0,3.285,-5.42,61.90,.06,.92,plaster);
  // A continuous, level pedestrian strip distinguishes the rear access from asphalt.
  for(let col=0;col<62;col++)for(let row=0;row<3;row++)box('rear-paving',-30.5+col,.022,-6.43-row*.61,.986,.025,.595,paving);
  box('rear-path-edge',0,.022,-8.02,61.98,.025,.12,dark);
  for(const x of [-19,-6.2,6.2,19])planter(x,-6.39,2.5,.50);
  // A discreet stair direction marker points to the existing eastern external stair.
  label('←  RESIDENT STAIRS',28.7,1.50,-5.995,1.54,.17,true);
  /* 东端墙：玄武岩勒脚 + 橡木墙板，与北墙同一套饰面。
   * 只做 s=1——西端那一段被电梯塔体量完全挡住，用不着；
   * 旧注释写的 "both end elevations" 与代码不符，已改掉。
   * 北墙的 rearWall() 是沿 x 铺板的，端墙得沿 z 铺，所以这里单独铺一份。 */
  for(const s of [1]){
    box('end-plinth',s*31.10,.16,-.60,.16,.22,10.78,dark);
    const z0=-5.99,z1=4.79,y0=.29,y1=3.18;
    const cols=Math.ceil((z1-z0)/1.55),rows=Math.ceil((y1-y0)/.72);
    const d=(z1-z0)/cols,h=(y1-y0)/rows;
    for(let c=0;c<cols;c++)for(let r=0;r<rows;r++){
      box('end-oak-panel',s*31.08,y0+(r+.5)*h,z0+(c+.5)*d,.18,h-.012,d-.012,oak);
    }
  }
  // Slender structural columns preserve a broad, continuous east-west route at z=0.
  for(const x of [-24,-12,12,24])for(const z of [-3.9,3.6])box('lobby-column',x,1.65,z,.26,3.15,.26,stone,true,true);
  // West coffee bar: a working counter, back bar, espresso machine and open seating.
  box('cafe-counter',-14,.58,-3.08,8.2,1.05,.88,wood,true,true);
  box('cafe-stone-top',-14,1.135,-3.08,8.34,.06,1.02,stone);
  box('cafe-back-cabinet',-14,.52,-5.21,8.2,.91,.61,wood,true);
  box('cafe-back-top',-14,1.01,-5.21,8.3,.045,.68,stone);
  for(const y of [1.65,2.25]){
    box('cafe-shelf',-14,y,-5.40,7.8,.045,.37,wood);
    for(let n=0;n<14;n++)box('cafe-canister',-17.45+n*.53,y+.13,-5.42,.15,.21,.16,n%3===0?bronze:linen);
  }
  label('S T A R B U C K S',-14,2.80,-5.62,4.4,.28);
  box('espresso-machine',-15.8,1.38,-3.18,1.12,.43,.49,bronze,false,true);
  box('espresso-face',-15.8,1.38,-2.925,.98,.29,.025,dark);
  for(const x of [-16.1,-15.55]){
    box('espresso-spout',x,1.24,-2.85,.045,.12,.13,bronze);
    box('coffee-cup',x,1.21,-2.77,.11,.10,.11,linen,false,true);
  }
  box('pastry-case',-12,1.37,-3.05,1.6,.39,.53,bronze);
  box('pastry-display',-12,1.38,-2.772,1.48,.28,.016,linen);
  for(const x of [-17,-14,-11]){
    box('barstool-base',x,.39,-1.95,.13,.66,.13,bronze,true);
    box('barstool-seat',x,.75,-1.95,.43,.075,.43,sage,false,true);
    box('cafe-pendant',x,2.70,-3.0,.43,.16,.43,warm,false,true);
    box('pendant-wire',x,2.99,-3.0,.012,.42,.012,bronze);
  }
  /* —— 咖啡座区（西段）——
   * 咖啡吧要读成"能坐下来待一会儿的地方"，而不是"一排柜台 + 几张孤零零的椅子"：
   *   · 座位组压到 4.4m 间距（原来 6m，走廊感太重）；
   *   · 每张桌子**两侧对坐**，靠背一律朝桌子（seat 的 dir）——
   *     原来沙发全摆在桌子的同一侧、还都背对桌子，坐下就是面壁；
   *   · 铺一块地毯把座位圈出来，压掉大片素色地砖的"过道感"；
   *   · 每组一盏低垂暖吊灯。咖啡座的氛围靠"局部亮、整体暗"，
   *     一排等高的长条灯带把整个空间照匀，那是商场中庭不是咖啡吧。
   */
  for(const x of [-25,-20.6,-16.2,-11.8]){
    box('cafe-table-leg',x,.36,1.65,.14,.62,.14,bronze,true);
    box('cafe-table',x,.72,1.65,1.20,.065,.85,stone,true,true);
    seat(x,2.72,1.7,linen,true,-1);   // 南侧：背朝 +z，面向桌子
    seat(x,.62,1.7,linen,true,1);     // 北侧：背朝 -z，面向桌子
    box('cafe-pendant',x,2.62,1.65,.46,.17,.46,warm);
    box('pendant-wire',x,2.955,1.65,.012,.49,.012,bronze);
  }
  box('cafe-rug',-18.4,.065,1.75,17.6,.006,3.5,rug);   // 抬到地砖面(0.062)之上，否则整块埋进地板里
  // 长向空间两端各收一次边：一株绿植 + 一盏落地灯
  for(const x of [-27.8,-9.0]){
    planter(x,3.05,1.5,.72);
    /* 落地灯必须自带碰撞盒：灯罩落在 y 1.49~1.79，正好卡在站立眼高 1.6 上，
     * 而它离最近的盒子（花池）有 0.2m 以上 —— 不补的话玩家能走到灯罩里面去。 */
    box('floor-lamp-pole',x,.78,3.62,.05,1.56,.05,bronze,true);
    box('floor-lamp-shade',x,1.64,3.62,.44,.30,.44,warm);
  }
  /* —— 东侧休息 / 阅览区：同一套朝向规则，座位组也压密 —— */
  for(const x of [11,16.5,22,27.5]){
    box('lounge-table',x,.37,1.65,1.4,.10,.7,wood,true,true);
    box('lounge-table-foot',x,.19,1.65,.72,.25,.43,dark);
    seat(x,2.95,2.6,sage,true,-1);
    seat(x,.62,2.6,sage,true,1);
    box('cafe-pendant',x,2.62,1.65,.46,.17,.46,warm);
    box('pendant-wire',x,2.955,1.65,.012,.49,.012,bronze);
  }
  box('lounge-rug',19.25,.065,1.75,19.2,.006,3.5,rug);
  /* —— 沿街面的高脚吧台 ——
   * 独坐的客人得有个不用拼桌的位置；顺带把临窗那一整排从"空着的玻璃幕"变成
   * "看得见有人坐"的界面。台面贴在前厅玻璃内侧，凳面 0.75 配台面 1.06。 */
  box('window-bar',-18,.985,4.16,9.6,.055,.42,wood,true,true);
  for(const x of [-22,-20,-18,-16,-14])box('window-bar-leg',x,.49,4.16,.10,.98,.10,bronze,true);
  for(const x of [-21.2,-19.2,-17.2,-15.2,-13.4]){
    box('window-barstool-base',x,.39,3.60,.13,.66,.13,bronze,true);
    box('window-barstool-seat',x,.75,3.60,.43,.075,.43,sage,false,true);
  }
  box('library-table',18,.76,-2.9,7.8,.075,1.15,wood,true,true);
  for(const x of [14.6,21.4])box('library-leg',x,.39,-2.9,.13,.68,.9,bronze,true);
  for(const x of [15,17,19,21])seat(x,-4.05,.72,linen,true);
  for(let n=0;n<5;n++){
    box('bookcase',12+n*3.1,1.27,-5.45,2.8,2.38,.43,wood,true);
    for(let row=0;row<4;row++)for(let book=0;book<9;book++)box('book-spine',10.85+n*3.1+book*.27,.42+row*.5,-5.22,.15,.30+(book%3)*.035,.025,book%2?sage:linen);
  }
  // 四条长条灯带都落在木格栅里：压到条底(3.155)之下才看得到光
  /* —— 门厅北侧：共享书房 / 书吧 ——
   * 这一段原先是 62m 大堂里最大的一片空白。做成"复合功能"：靠墙是书架墙，
   * 中间一张大长桌（白天共享办公、晚上棋牌或手工），两侧对坐，底下压一块地毯
   * 把它从穿行流线里圈出来。日式的低矮书架与英式的长桌、扶手椅放在同一个屋檐
   * 下——公寓一层本来就该是个"谁都能待一会儿"的地方，不是一条走道。 */
  for(const x of [-1.2,1.6,4.4]){
    box('study-shelf',x,1.30,-5.55,2.6,2.32,.44,wood,true);
    for(let row=0;row<4;row++)for(let b=0;b<8;b++){
      box('study-book-spine',x-1.15+b*.26,.44+row*.49,-5.31,.17,.28+(b%3)*.03,.022,b%2?sage:linen);
    }
  }
  box('study-table',0,.74,-2.55,7.6,.08,1.30,wood,true,true);
  for(const x of [-3.2,3.2])box('study-table-leg',x,.36,-2.55,.16,.74,.50,bronze,true);
  for(const x of [-2.6,-1.3,1.3,2.6]){
    seat(x,-1.45,1.7,linen,true,-1);   // 南侧：背朝 +z，面向长桌
    seat(x,-3.60,1.7,linen,true,1);    // 北侧：背朝 -z，面向长桌
  }
  for(const x of [-2.4,2.4]){
    box('study-lamp-base',x,.86,-2.55,.20,.03,.20,dark);
    box('study-lamp-pole',x,1.05,-2.55,.03,.38,.03,bronze);
    box('study-lamp-shade',x,1.29,-2.55,.34,.20,.34,warm);
  }
  box('study-rug',0,.065,-2.55,9.2,.006,4.4,rug);
  /* —— 自助水吧（咖啡吧与书房之间）——
   * 咖啡吧的"自助端"：杯子、热水、糖奶在这边自取，吧台那边只管出杯。
   * 顺带把西段与书房之间那截空墙接上，不再是一段素墙。 */
  box('water-bar',-6.2,.52,-5.44,6.4,.90,.62,wood,true,true);
  box('water-bar-top',-6.2,1.01,-5.44,6.5,.045,.68,stone);
  for(const x of [-8.4,-6.9,-5.4,-3.9])box('water-cup-stack',x,1.12,-5.36,.22,.16,.22,linen,false,true);
  /* 饮水机是唯一一个**探出吧台**的眼高摆件（x 到 -2.77、z 到 -5.10，比 water-bar 的
   * -3.00 / -5.13 还外凸 23cm），所以必须自带碰撞盒：否则第一人称走到它跟前时，
   * 相机能直接站进机壳里，近裁面就会把它切开、露出内部。其余摆件（咖啡机、点心柜、
   * 前台屏）都完整落在各自的台面碰撞盒投影内，加了也是冗余。 */
  box('water-dispenser',-3.0,1.38,-5.30,.46,.62,.40,bronze,true,true);
  box('water-dispenser-face',-3.0,1.38,-5.11,.34,.34,.02,dark);
  /* —— 包裹柜 + 社区公告 ——
   * 英式公寓一层的门房职能：收快递、贴通知。柜子 + 软木板一组，
   * 把前台到休息区之间那截墙也用起来。 */
  box('parcel-cabinet',8.2,.98,-5.52,5.6,1.72,.52,bronze,true,true);
  for(let col=0;col<5;col++)for(let row=0;row<3;row++){
    box('parcel-door',6.0+col*1.1,.55+row*.55,-5.25,.98,.50,.04,row%2?wood:linen,true);
    box('parcel-handle',6.85+col*1.1,.55+row*.55,-5.22,.22,.035,.03,dark);
  }
  box('notice-board',8.2,2.45,-5.24,4.2,1.10,.05,dark);
  for(let n=0;n<9;n++){
    box('notice-paper',6.35+(n%5)*.95,2.62-Math.floor(n/5)*.55,-5.20,.62,.42,.012,linen);
  }
  /* —— 玄关：换鞋凳 + 伞架 ——
   * 日式公寓进门那一套：鞋脱在门口的落尘区、伞立在门内侧滴水，不把大堂弄湿。
   * 放在门厅西侧的空地上，不占中央那条从大门到电梯的通道。 */
  box('genkan-bench',-8.5,.42,3.30,2.4,.16,.52,wood,true,true);
  for(const x of [-9.4,-7.6])box('genkan-bench-leg',x,.20,3.30,.14,.44,.42,bronze,true);
  box('umbrella-stand',-6.4,.30,3.62,.42,.60,.42,dark,true);
  for(let n=0;n<4;n++)box('umbrella',-6.4+n*.09,.78,3.62,.035,1.02,.035,n%2?sage:linen);
  for(const x of [-21,-12,12,21])box('hall-light',x,3.128,0,7,.018,.085,warm);
  label('←  LIFT  /  1—4',-28,2.65,4.435,2.7,.22,true);
  group.userData.podiumVersion=3;
  return {group,boxes,autoDoor};
}
