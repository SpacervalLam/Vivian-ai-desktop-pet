import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';
import { createBlossomField, createTrunkField } from './foliage';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
/* 行车时刻表是纯函数，单独一个模块（见 trainSchedule.ts 的文件头注释：
 * 这个场景在 headless 下只有 0.4fps，整周期的性质只能在 Node 里核账）。 */
import { CAR_CENTRES, CAR_HL, CONSIST_HL, STOP_C, TRACK_HALF, RUN_HALF, DWELL, CYCLE, PHASE_E, INITIAL_PHASE,
  LEG_IN, LEG_OUT, westC, eastC, westWin, eastWin, closureAt, isDwelling, wrap } from './trainSchedule';
import { BRIDGE_Z_N, BRIDGE_Z_S, BRIDGE_X0, BRIDGE_X1, BRIDGE_DECK_Y, BRIDGE_DECK_THICK, BRIDGE_GIRDER_Y,
  inBridgeSpan } from './river';

/** Eastern extension, phase one: authored streets, crossing and a parked local train. */
export function createSakuraStation(scene: THREE.Scene) {
  const root = new THREE.Group(); root.name='sakura-station-district';
  root.userData.sceneCollideSkip=true; scene.add(root);
  const colliders: Collider[]=[];
  const dynamicColliders: Collider[]=[];
  const barriers: {group:THREE.Group;dir:number;block:Collider}[]=[];
  const signals:THREE.MeshStandardMaterial[]=[];
  const materials:THREE.Material[]=[]; const textures:THREE.Texture[]=[];
  const geometries=new Set<THREE.BufferGeometry>();
  const mat=(color:string,glow=false)=>{const m=new THREE.MeshStandardMaterial({color,roughness:.82,...(glow?{emissive:color,emissiveIntensity:.5}:{})});m.userData.outlineWeight=0;materials.push(m);return m;};
  const M={cream:mat('#eee4d1'),steel:mat('#53636b'),dark:mat('#303c47'),stone:mat('#b7b9ae'),road:mat('#5b6477'),wood:mat('#85604a'),pink:mat('#d98fa8'),mint:mat('#7aa999'),yellow:mat('#e7c25b'),white:mat('#f2eadb'),glass:mat('#7b9aa7'),grass:mat('#79916b'),soil:mat('#77716d'),red:mat('#b64948'),light:mat('#fff0c9',true)};
  // Fine surface grain breaks up large uniform slabs without adding draw calls.
  let grainSeed=1977;
  const grainRandom=()=>{grainSeed=(Math.imul(grainSeed,1664525)+1013904223)>>>0;return grainSeed/4294967296;};
  for(const material of [M.stone,M.road,M.soil]){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=256;
    const c=canvas.getContext('2d')!;c.fillStyle='#dddddd';c.fillRect(0,0,256,256);
    for(let i=0;i<9000;i++){const g=130+Math.floor(grainRandom()*120);c.fillStyle=`rgb(${g},${g},${g})`;const s=material===M.soil?2+grainRandom()*6:1;c.fillRect(grainRandom()*256,grainRandom()*256,s,s*.65);}
    const texture=new THREE.CanvasTexture(canvas);texture.wrapS=texture.wrapT=THREE.RepeatWrapping;texture.repeat.set(5,5);texture.colorSpace=THREE.SRGBColorSpace;textures.push(texture);material.map=texture;
  }
  const cube=new THREE.BoxGeometry(1,1,1),cyl=new THREE.CylinderGeometry(1,1,1,10);geometries.add(cube);geometries.add(cyl);
  function box(x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material){const o=new THREE.Mesh(cube,m);o.position.set(x,y,z);o.scale.set(w,h,d);o.castShadow=h>.15;o.receiveShadow=true;root.add(o);return o;}
  function solid(name:string,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material,kind:Collider['kind']='wall'){box(x,y,z,w,h,d,m);colliders.push({source:`station-${name}`,kind,min:new THREE.Vector3(x-w/2,y-h/2,z-d/2),max:new THREE.Vector3(x+w/2,y+h/2,z+d/2)});}
  function rod(a:number[],b:number[],r:number,m:THREE.Material){const va=new THREE.Vector3(...a),vb=new THREE.Vector3(...b),v=vb.clone().sub(va);const o=new THREE.Mesh(cyl,m);o.position.copy(va.add(vb).multiplyScalar(.5));o.scale.set(r,v.length(),r);o.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),v.normalize());o.castShadow=true;root.add(o);}
  function sign(text:string,sub:string,x:number,y:number,z:number,w:number,h:number,angle=0,bg='#354e60'){
    const canvas=document.createElement('canvas');canvas.width=768;canvas.height=256;
    const c=canvas.getContext('2d')!;c.fillStyle=bg;c.fillRect(0,0,768,256);c.strokeStyle='#e8cfaf';c.lineWidth=6;c.strokeRect(12,12,744,232);c.fillStyle='#fff5e4';c.textAlign='center';c.font='bold 65px "Microsoft YaHei", sans-serif';c.fillText(text,384,112,710);c.font='27px sans-serif';c.fillText(sub,384,191,710);
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;textures.push(t);
    const m=new THREE.MeshStandardMaterial({map:t,roughness:.85,side:THREE.DoubleSide});m.userData.outlineWeight=0;materials.push(m);
    const g=new THREE.PlaneGeometry(w,h);geometries.add(g);const o=new THREE.Mesh(g,m);o.position.set(x,y,z);o.rotation.y=angle;root.add(o);
  }
  // Only the main street crosses the railway. Other old roads remain in the original district.
  solid('east-ground',115,-.045,25,70,.1,140,M.grass,'floor');
  solid('main-street',108,.025,10.1,56,.08,6.4,M.road,'floor');
  for(const z of [5.55,14.65])solid('sidewalk',108,.06,z,56,.12,2.5,M.stone,'floor');
  solid('station-approach',76.8,.035,39,5.6,.07,44,M.stone,'floor');
  solid('station-plaza',73,.04,49,10,.08,23,M.stone,'floor');
  for(let x=80;x<136;x+=2){box(x,.13,4.35,1.94,.16,.22,M.cream);box(x,.13,15.95,1.94,.16,.22,M.cream);}
  for(let x=81;x<136;x+=4)box(x,.071,10.1,2,.018,.09,M.yellow);
  for(const x of [81,99]){box(x,.08,10.1,.18,.02,5.2,M.white);for(let k=0;k<6;k++)box(x-1.3,.081,7.8+k*.85,1,.02,.38,M.white);}
  for(const z of [6.1,14.1])for(let x=80;x<137;x+=.45)box(x,.145,z,.32,.04,.38,M.yellow);
  for(let z=18;z<62;z+=2)for(const x of [74.5,77])box(x,.081,z,.025,.01,1.97,M.dark);
  /* ---------------- 轨道在 z 向铺多长 ----------------
   *
   * 原版两条轨道都只铺 z ∈ [-45, 95]：北端在 z=-45 就断了，而玩家的可活动范围是
   * z ∈ [-78, 95]（见 districtArt 的 WALK_BOUNDS）——也就是说**轨道的北端断在活动
   * 范围以内 33m 处**。站在道口往北看，轨道连着草地一起在眼前凭空截断，后面是一张
   * 空沥青（取证见 tmp/ring-shots/R5-track-north.jpg）。顺带：列车北端折返点是
   * z≈-40.5，离断头只剩 4.5m。
   *
   * 这里拉到 ±TRACK_HALF。取值不是拍脑袋：晴天雾是 near 85 / far 300
   * （districtArt.setEnvironment），要玩家**在任何可站立点都看不到断头**，
   * 断头就必须离每个可站立点 ≥300m。最苛刻的点是街面东北角 (80,-78)
   * ——东侧车场地坪的北界只到 z=-45，够不到更北——它到轨道断头 (88,-H) 的距离是
   * √(8² + (H-78)²) ≥ 300 → H ≥ 377.9。取 440，留 62m 余量。
   *
   * 世界地面（exterior.ts 的 SIZE=360 → ±180）比这短，同步扩到 1000：地面边缘到
   * 最远的可站立点 (150,95) 有 350m，也在雾里消失 —— 否则轨道还没断，地面先断了。
   */
  /* 轨道半长现在是 trainSchedule 的单点真相（行程端点由它派生，见那里的注释），
   * 这里只 import，铺轨 / 接触网 / 栅栏延段全用它。 */
  /* 铁路绿化带。原版东侧草地（east-ground）只到 z ∈ [-45,95]，轨道一延长，北面那段
   * 就变成"轨道铺在沥青上"。这两条把绿化带补到与轨道同长（20m 宽，把 x=80.2/97
   * 两道栅栏和 84.1~92.9 的道床都包在里面）。
   * 刻意与 east-ground **对接而不是盖上去**（在 z=-45 / 95 处留接缝）：
   * 两块草地的顶面都在 y=0.005，重叠就是同面 z-fighting。
   * 南北端各**比道砟短 1m**（±439 对 ±440）：两端面同为 z 法向、同向、又都在
   * x[84.1,92.9] 上重叠，齐平就是一对共面片（z-fight 扫描实测 0.34m²×2）。
   * 与围栏延段"短 1m"是同一条理由。反正那 1m 在 440m 外、雾 far=300 之内看不到。
   *
   * 加北侧小河之后，北段那条 -439..-45 又被**桥**切成两截：BRIDGE_Z_N / BRIDGE_Z_S
   * 之间的开口下面不是土而是河谷，草皮悬在水面上比什么都显眼，必须让它跟着断。 */
  const GRASS_HALF = TRACK_HALF - 1;   // 上面那条"比道砟短 1m"的约定，写成常量便于推算
  for(const [cz,cd] of [
    [(-GRASS_HALF + BRIDGE_Z_N)/2, BRIDGE_Z_N + GRASS_HALF],   // 北岸之外
    [(BRIDGE_Z_S - 45)/2, -45 - BRIDGE_Z_S],                    // 桥台到原东侧草地北界
    [267,344],
  ] as const)box(90,-.045,cz,20,.1,cd,M.grass);
  // Ballast and two detailed track beds.
  for(const x of [86,91]){
    /* 道砟同样在桥跨处断开。
     *
     * 反过来想更清楚：两条道砟各宽 3.8m，把 x∈[84.1,92.9] 几乎铺满。它要是一路铺过
     * 河，从侧面看就是"土台子横穿水面"，桥面板、梁腹、桥下一整跨的空气全被埋掉 ——
     * 这条河等于白挖。断开，桥才读得出来。
     *
     * 枕木不跟着断：桥面板顶面就做在道砟顶面那个标高上，枕木在桥上和在区间里一样
     * 半嵌着，这是最常见的有砟桥面做法，也省掉一整套"桥枕"零件。 */
    const BALLAST_TOP=.12;
    for(const [z0,z1] of [[-TRACK_HALF,BRIDGE_Z_N],[BRIDGE_Z_S,TRACK_HALF]] as const)
      box(x,BALLAST_TOP-.08,(z0+z1)/2,3.8,.16,z1-z0,M.soil);
    /* 枕木一直铺到轨道尽头，但**只有街区里那段带扣件**（每根枕木上 4 个小垫板 /
     * 轨座）。0.3m 的扣件在 150m 外只占 1 像素，铺满 880m 要多花 2/3 的顶点；
     * 断点取 150m，是因为那里扣件已经小到看不出"消失"。
     * 起点取 -TRACK_HALF 而不是 -TRACK_HALF+1：440-44 = 396 = 550×0.72，
     * 所以 -440 与原版起点 -44 **同相**，站内（含道口）那些枕木一根都不挪位。 */
    for(let z=-TRACK_HALF;z<TRACK_HALF;z+=.72){
      box(x,.17,z,2.65,.18,.24,M.wood);
      if(Math.abs(z)<150)for(const dx of [-.74,.74]){box(x+dx,.29,z,.3,.07,.26,M.steel);box(x+dx,.37,z,.075,.17,.18,M.dark);}
    }
    for(const dx of [-.74,.74]){box(x+dx,.27,0,.18,.08,TRACK_HALF*2,M.steel);box(x+dx,.37,0,.07,.19,TRACK_HALF*2,M.steel);box(x+dx,.48,0,.12,.06,TRACK_HALF*2,M.steel);}
    // Flush crossing deck, rail flangeways remain visible.
    solid('crossing-deck',x,.19,10.1,1.3,.38,7.2,M.stone,'floor');
    for(const dx of [-1.35,1.35])solid('crossing-deck-edge',x+dx,.19,10.1,1,.38,7.2,M.stone,'floor');
  }
  solid('crossing-middle',88.5,.19,10.1,1.2,.38,7.2,M.stone,'floor');
  // Sloped approaches eliminate an abrupt step up to the railhead.
  for(const [a,b,reverse] of [[80,83.8,false],[93.2,100,true]] as const){
    const slope=(x:number)=>.07+(reverse?(b-x):(x-a))/(b-a)*.31;
    colliders.push({source:'station-crossing-ramp',kind:'ramp',min:new THREE.Vector3(a,.05,6.5),max:new THREE.Vector3(b,.4,13.7),heightAt:(x,z)=>x>=a&&x<=b&&z>=6.5&&z<=13.7?slope(x):null});
    const o=box((a+b)/2,.185,10.1,b-a,.07,7.2,M.road);o.rotation.z=(reverse?-1:1)*Math.atan(.31/(b-a));
  }
  /* 接触网门型架：与轨道同长。间距仍是 18m，起点取 -432 = -24×18 —— 原有的
   * -36 / -18 / 0 / … / 90 全在这组 18 的倍数上，站内那几座位置一个都不动。
   * 只有 z ∈ [-100,100] 那几座登记碰撞盒（玩家走得到它们）；再往外的都在边界环之外，
   * 登记了也永远命中不了，白白占掉第一人称每帧的遍历和 navWorld 的栅格化。 */
  for(let z=-432;z<TRACK_HALF;z+=18){
    /* 门型架落在桥跨里就跳过。18m 的整数倍里正好有一个撞上了：-432 + 19×18 = -90，
     * 而 x=81 / 96 两根柱子都在桥面宽度之外 —— 它们会直接戳在河谷的空气上。
     * 接触线本身是那两根通长的 `rod(...±TRACK_HALF)`，跳一座门型架不会让接触网
     * 断成一截，视觉上只是这一跨头顶空一座。 */
    if(inBridgeSpan(z))continue;
    const near=Math.abs(z)<=100;
    for(const x of [81,96]){
      if(near)solid('mast',x,3.9,z,.2,7.8,.2,M.steel);else box(x,3.9,z,.2,7.8,.2,M.steel);
      box(x,.15,z,.65,.3,.65,M.stone);
    }
    rod([81,7.6,z],[96,7.6,z],.075,M.steel);rod([81,7,z],[96,7,z],.055,M.steel);
    for(let x=81;x<96;x+=1.5)rod([x,7,z],[x+1.5,7.6,z],.027,M.steel);
    for(const x of [86,91]){rod([x,7.6,z],[x,6.05,z],.035,M.steel);for(let y=6.6;y<7.1;y+=.09)box(x,y,z,.18,.04,.18,M.cream);}
  }
  for(const x of [86,91]){rod([x,6.05,-TRACK_HALF],[x,6.05,TRACK_HALF],.014,M.dark);rod([x,6.6,-TRACK_HALF],[x,6.6,TRACK_HALF],.014,M.dark);}
  /** 栅栏。`base` 是它脚下的地面标高——站台栏杆要坐在台面上（见 PT）。
   *  `detail=false` 给边界环之外那两段：柱距从 2m 放到 6m、省掉细栏条，也不登记
   *  碰撞盒（玩家走不到，登记了只是永远命中不了的盒）。6m 柱距在 150m 外约 1 像素。 */
  function fence(x:number,z0:number,z1:number,base=0,detail=true){
    if(detail){
      solid('track-fence',x,base+.8,(z0+z1)/2,.12,1.6,z1-z0,M.mint);
      // Thin horizontal collision rail; visual mesh is open slats.
      root.remove(root.children[root.children.length-1]);
    }
    for(let z=z0;z<=z1;z+=detail?2:6){
      box(x,base+.8,z,.09,1.6,.09,M.mint);
      if(detail)for(let q=0;q<2;q+=.25)box(x,base+.8,z+q,.025,1.35,.025,M.steel);
    }
    for(const y of [.18,1.5])box(x,base+y,(z0+z1)/2,.06,.06,z1-z0,M.mint);
  }
  for(const x of [80.2,97]){
    /* 北/南延段刻意与街区内那三段**留 1m 空档**（-46 对 -45、96 对 95）：
     * 首尾两根柱子落在同一个 z 上就是两个完全重合的 0.09m 方柱（z-fighting）。
     * 同一条理由，北延段被桥切开后新生的两个端头也各让 1m —— 它们将来是接
     * 桥栏杆的，齐了就会并排立两根柱子。 */
    fence(x,-TRACK_HALF,BRIDGE_Z_N-1,0,false);   // 桥北（边界环外）
    fence(x,BRIDGE_Z_S+1,-46,0,false);           // 桥南到道口
    fence(x,-45,6);                     // 原样：道口北侧
    fence(x,14.5,33);                   // 原样：站台前
    fence(x,85,95);                     // 原样：站台南端
    fence(x,96,TRACK_HALF,0,false);     // 南延段（边界环外）
  }
  /* ---------------- 跨河桥 ----------------
   *
   * 21m 单跨，不设河中桥墩：河只有 14m 宽、水深 1.2m，往水里立墩子的收益远不如留一
   * 跨干净的腹下空间 —— 站在岸边看过去，"这么宽一段没有支撑居然跨过去了"正是桥该有
   * 的读数。
   *
   * 结构从上往下三层：钢筋混凝土桥面板 0.35m → 两条主梁各 0.4m 深、正落在两条走行轨
   * 底下 → 腹下净空到水面 0.92m。
   */
  {
    const spanZ = BRIDGE_Z_S - BRIDGE_Z_N;
    const czB = (BRIDGE_Z_S + BRIDGE_Z_N) / 2;
    const cxB = (BRIDGE_X0 + BRIDGE_X1) / 2;
    const widthB = BRIDGE_X1 - BRIDGE_X0;
    const deckBottom = BRIDGE_DECK_Y - BRIDGE_DECK_THICK;
    // 桥面板。顶面标高与道砟顶面一致，枕木在桥上和区间里保持同一高度。
    box(cxB,(BRIDGE_DECK_Y+deckBottom)/2,czB,widthB,BRIDGE_DECK_THICK,spanZ,M.stone);
    // 两条主梁，各自对准一条走行轨（x=86 / 91），这是荷载真正走的路。
    for(const gx of [86,91])
      box(gx,(deckBottom+BRIDGE_GIRDER_Y)/2,czB,2.2,deckBottom-BRIDGE_GIRDER_Y,spanZ,M.stone);
    // 端部横隔板：把面板和两条主梁在桥台处箍住，免得从河里看上去是三片孤零零的板。
    for(const ezz of [BRIDGE_Z_N+.6,BRIDGE_Z_S-.6])
      box(cxB,(deckBottom+BRIDGE_GIRDER_Y)/2,ezz,widthB,deckBottom-BRIDGE_GIRDER_Y,1.2,M.stone);
    /* 桥台。往下多做 1.6m 而不是刚好做到地面标高为止：世界地面只有一张面，底下是空的，
     * 桥台做浅了，从河谷方向平看过去就是一块悬在半空里的板。
     * 翼墙向两侧张开，把桥台背后那捧土收住。 */
    for(const [az,dir] of [[BRIDGE_Z_N,1],[BRIDGE_Z_S,-1]] as const){
      box(cxB,(BRIDGE_DECK_Y-1.6)/2,az+dir*.9,widthB+1.6,BRIDGE_DECK_Y+1.6,1.8,M.stone);
      for(const sx of [-1,1])
        box(cxB+sx*(widthB/2+.6),(BRIDGE_DECK_Y-1.6)/2,az+dir*.9,1.2,BRIDGE_DECK_Y+1.6,1.8,M.stone);
    }
    // 缘石：道砟在桥上是断的，两侧需要一道矮边把桥面边缘收干净。
    for(const kx of [BRIDGE_X0+.25,BRIDGE_X1-.25])
      box(kx,BRIDGE_DECK_Y+.17,czB,.5,.34,spanZ,M.stone);
    /* 桥栏杆。刻意不用 fence()：那是 1.6m 高的绿色网片，装在桥上把腹下的空间整个
     * 糊住；这里用 1.05m 的钢管扶手，横杆只有两根，站在岸上能直接看穿到水面。 */
    for(const rx of [BRIDGE_X0,BRIDGE_X1]){
      for(const y of [.55,1.05])box(rx,BRIDGE_DECK_Y+y,czB,.07,.07,spanZ,M.steel);
      for(let z=BRIDGE_Z_N;z<=BRIDGE_Z_S+.01;z+=2.6)
        box(rx,BRIDGE_DECK_Y+.52,z,.08,1.05,.08,M.steel);
    }
  }

  // Each barrier remains a separate pivot so static merging cannot bake its motion.
  for(const [x,z,dir] of [[81,6.4,1],[96,13.8,-1]]){
    solid('signal-base',x,.35,z,.7,.7,.7,M.stone);
    for(let j=0;j<10;j++)box(x,.8+j*.27,z,.17,.27,.17,j%2?M.dark:M.yellow);
    box(x,2.7,z,.95,.15,.16,M.dark);
    for(const dx of [-.31,.31]){box(x+dx,2.7,z-.1,.28,.3,.22,M.dark);const lens=mat('#d4473f',true);signals.push(lens);box(x+dx,2.7,z-.23,.18,.18,.025,lens);box(x+dx,2.89,z-.26,.32,.065,.35,M.dark);}
    solid('barrier-motor',x, .65,z+dir*.8,.5,1.3,.5,M.cream);
    const pivot=new THREE.Group();pivot.name='crossing-barrier';pivot.userData.noMerge=true;pivot.position.set(x,.95,z+dir*.8);root.add(pivot);
    for(let j=0;j<26;j++){const segment=box(0,.125+j*.25,0,.1,.25,.1,j%2?M.dark:M.yellow);pivot.add(segment);}
    const block:Collider={source:'station-moving-barrier',kind:'wall',min:new THREE.Vector3(),max:new THREE.Vector3()};
    barriers.push({group:pivot,dir,block});
    sign('踏切 注意','STOP · LOOK · LISTEN',x,1.9,z-.15,.75,.48,Math.PI);
  }
  /** 站台长椅。`base` 同 fence：座面高度是相对台面的。 */
  function bench(x:number,z:number,base=0){solid('bench',x,base+1.54,z,.65,1.05,2.7,M.wood);root.remove(root.children[root.children.length-1]);for(let j=0;j<4;j++)box(x-.23+j*.15,base+1.34,z,.12,.08,2.7,M.wood);for(let j=0;j<3;j++)box(x+.28,base+1.62+j*.15,z,.06,.11,2.7,M.wood);for(const dz of [-1,1])box(x,base+1.09,z+dz,.5,.5,.07,M.steel);}

  /* ---------------- 站台标高 / 宽度 ----------------
   *
   * `PT_RAISE` 是为了电车整体抬高 0.40 而同步抬高的台面量（缘由见下面电车段的
   * 长注释）。原版台面 0.84、轨面 0.51 —— 只差 0.33，同时车体底 0.625 只比轨面
   * 高 0.115：轮对、转向架、床下机器全部没有可展示的竖向空间，"简陋"有一半
   * 出在这个压缩上。抬完之后：
   *   轨面 0.51 → 车体底 1.025（0.515 的床下空间）
   *   台面 1.24 → 车内地板 1.45（0.21 的踏步，与真车一致）
   *
   * `PLAT_W` 顺带修掉第二个破绽：原版台缘在 83.6，而车体侧面在 84.625 ——
   * 列车看起来停在站台外一米。站台由 3.2 加宽到 4.1（外侧边线不动，只往轨道方向长），
   * 台缘按「车体最外皮 + 0.105」定：车体侧面 84.625、门框外皮 84.644、
   * 门玻璃 84.583，取 84.52 —— 留出 10cm 空隙。原值 84.5 看着没错，但站台块
   * 中心在 82.55、半宽 2.05 → 实际台缘是 84.6，比门框还靠里 4cm，等于站台切进车里；
   * 黄色点字ブロック 也随之从 84.5 内缩到 84.22（原值比台缘还外 7.5cm，整条悬空）。
   * 2 号站台的 `edge` 同理：台缘在**西**侧 92.5，原值 92.5 的条带有一半悬空 → 92.80。
   */
  const PT_RAISE=.40, PT=.84+PT_RAISE, PLAT_W=4.1, PLAT_HW=PLAT_W/2;
  // Platforms are independently accessible from their north ends via shallow ramps.
  for(const [x,edge] of [[82.47,84.22],[94.55,92.80]]){
    solid('platform',x,PT/2,59,PLAT_W,PT,50,M.stone,'floor');
    colliders.push({source:'station-platform-ramp',kind:'ramp',min:new THREE.Vector3(x-PLAT_HW,0,28),max:new THREE.Vector3(x+PLAT_HW,PT,34),heightAt:(px,pz)=>px>=x-PLAT_HW&&px<=x+PLAT_HW&&pz>=28&&pz<=34?(pz-28)/6*PT:null});
    const ramp=box(x,PT/2,31,PLAT_W,.08,6.06,M.stone);ramp.rotation.x=-Math.atan(PT/6);
    for(let z=35;z<84;z+=.5){box(edge,PT+.025,z,.35,.035,.4,M.yellow);for(const dx of [-.1,0,.1])box(edge+dx,PT+.052,z,.025,.02,.3,M.yellow);}
    for(let z=38;z<82;z+=7){solid('canopy-column',x,PT+1.9,z,.13,3.8,.13,M.steel);box(x,PT+3.81,z,PLAT_W-.3,.15,.17,M.steel);rod([x,PT+2.66,z],[x-1.9,PT+3.81,z],.045,M.steel);rod([x,PT+2.66,z],[x+1.9,PT+3.81,z],.045,M.steel);box(x,PT+3.71,z+1.5,.12,.07,1.8,M.light);}
    box(x,PT+3.99,59,PLAT_W-.1,.15,46,M.cream);for(const dx of [-2.0,2.0])box(x+dx,PT+3.98,59,.1,.24,46,M.steel);
    for(const z of [44,64,77])bench(x, z, PT_RAISE);
    sign('桜町駅','SAKURAMACHI  /  さくらまち',x,PT+2.71,40,2.7,.78,Math.PI);
    sign(x<90?'1  春日野方面':'2  山桜方面','LOCAL LINE · SAKURA',x,PT+2.66,61,2.6,.7,Math.PI);
    fence(x<90?80.45:96.65,35,84,PT_RAISE);
  }
  sign('桜町駅  →','STATION · 80 m',73,2.1,14.5,3,.8,Math.PI);
  /* ================= 通勤型電車（2 両編成） =================
   *
   * 原版是「两个奶油色盒子 + 一块平板车顶」，六个破绽叠在一起：
   *   ① 没有车头 —— 两辆车都在 -z 端画了玻璃和头灯，于是两个"车头"在 z≈57
   *      面对面，而编组真正的两端是平的；
   *   ② 车门（dz ±4.5、宽 1.25）和车窗（dz -4.35 / 4.25、宽 1.7）在 z 上重叠，
   *      门里套着半扇窗；
   *   ③ 转向架只有四个悬空的深色圆盘，没有侧架 / 摇枕 / 轴箱 / 制动；
   *   ④ 床下什么都没有（真实通勤车的床下机器是最有信息量的一层）；
   *   ⑤ 车体是硬边盒子，没有肩部（cant rail）、没有圆角、没有雨檐；
   *   ⑥ 编组两端没有貫通幌、没有連結器、没有ジャンパ線。
   *
   * 竖向还有一个更根本的问题：车体底 0.625 只比轨面（0.51）高 0.115 ——
   * 轮对无论怎么摆都只能露出 11cm，站台上根本看不见转向架。所以这一段的前提是
   * **把电车整体抬高 PT_RAISE**（站台同步抬，见上面的 PT）：
   *   轨面 0.51 → 车体底 1.025（0.515 的床下空间，轮对露出 68%）
   *   台面 1.24 → 车内地板 1.45（0.21 的踏步）
   *
   * 几何只用 box / 圆角盒 / 轴向圆柱三种原型，共用 cube / cyl 几何，合批后不涨
   * draw call；只有 4 张 canvas 贴片（方向幕 ×1、侧面行先 ×1、车号 ×2）是独立材质。
   *
   * 编组：A 车 z=48（驾驶台在 -z 端）、B 车 z=65（驾驶台在 +z 端），
   * 中间 z 56..57 是貫通幌。这样两端各有一个真车头 ——
   * 也因此这套几何**天生是双向的**，东轨那列直接复用，不需要镜像。
   */
  /* 两条轨的中心线。西轨 86 / 东轨 91，站台分别在 82.47 / 94.55，一轨一站台。
   * 两列编组各占一条轨、各走一个方向（时刻表见 update 里那段长注释）。 */
  const TRKX=86, TRKX_E=91, CAR_HW=1.375, CAR_H=3.25;
  const CAR_Y0=.625+PT_RAISE, CAR_CY=CAR_Y0+CAR_H/2, FLR=CAR_Y0+.425, ROOF=CAR_Y0+CAR_H+.29;
  /** 车轮半径 / 中心高：轮心 = 轨面 0.51 + 半径，轮对正好坐在钢轨上。 */
  const WHL_R=.38, WHL_Y=.51+WHL_R, BOGIE_DZ=5.4;

  // —— 电车专用材质。每多一桶就多一个 draw call，只加必要的那几桶 ——
  /* 颜色仍然是"颜色"，贴图只是**调制层**（灰底 0.93 上下浮动）：map 是与
   * color 相乘的，如果把花纹直接画成材质色，结果会变成颜色的平方、整车发暗。
   * 所以下面每张 canvas 都画在中性灰上，污渍用暖灰 → 乘出来才是"脏"而不是
   * "变色"。顺带好处：材质 hex 仍然可读，z-fighting / 审计脚本照旧能报出
   * #3f4952 这样的身份，不会全变成 #ffffff。 */
  const M2={
    body:mat('#eee4d1'),     // 车体奶油色（与 M.cream 同色，但带拉丝 + 板缝）
    roof:mat('#9aa3a7'),     // 不锈钢车顶
    band:mat('#232c34'),     // 黑色窗带 / 门套 / 前脸 / 雨檐
    skirt:mat('#3f4952'),    // 裙板
    frame:mat('#4d575e'),    // 床下枠 / 床下机器 / 連結器
    bogie:mat('#525d66'),    // 转向架（比裙板亮一档，否则和轮对糊成一团黑）
    wheel:mat('#262c31'),    // 车轮
    panto:mat('#8d9599'),    // 受电弓
    blind:mat('#e3d9c4'),    // 卷帘
    hand:mat('#cdd2ce'),     // 手すり（不锈钢）
    rubber:mat('#1b2228'),   // 门窗胶条 / 幌
  };
  const glassDim=mat('#3a525d');                        // 未点灯的窗玻璃
  /* 点灯窗：底色调成冷灰蓝、自发光降到 0.45 —— 原来是 0.9 的饱和琥珀，
   * 白天看着像一排灯箱；真车白天也开灯，但只是"窗里亮着"，不该比车身还抢眼。 */
  const glassLit=new THREE.MeshStandardMaterial({color:'#4e5a60',emissive:'#ffd9a6',emissiveIntensity:.45,roughness:.55});
  glassLit.userData.outlineWeight=0;materials.push(glassLit);
  const lampTail=mat('#c2372f',true);                   // 尾灯

  /* ================= 程序化材质贴图 =================
   * 车体侧面是一整块 16m × 3.25m 的平面。几何细节做完之后，"简陋感"最后剩下
   * 的那一半全在这里：纯色渲染出来就是一块塑料板。挂 map 的代价是**零个
   * draw call** —— 这辆车最后会被 mergeByMaterial 按材质合成十几桶，多一张
   * 贴图不多一桶。
   *
   * 四个必须记住的坑：
   *   ① BoxGeometry 的 UV 是**每面 0..1**，不随世界尺寸走。同一材质被长度差
   *      100 倍的零件共用（窗带 15.1m / 冷房散热片 5cm），repeat 只能照顾主面。
   *      所以花纹一律做成"细到看不出尺度"的密排线或噪声，绝不能做稀疏大图案。
   *   ② map 与 color 相乘。花纹必须画在中性灰上，否则整车颜色被平方、发暗。
   *   ③ 贴图必须用 seeded 的 grainRandom 画。Math.random 会让每次载入的污渍
   *      位置都不同，截图前后对比就没法做了。
   *   ④ 所有金属件 metalness 压在 0.45 以下。这个场景的 IBL 是 PMREM 家具环境
   *      （scene.environmentIntensity 只有 0.24~0.36），metalness 一高就只剩
   *      镜面项、直接黑掉 —— apartmentLift 的青铜门已经踩过一次。
   */
  function skin(m:THREE.MeshStandardMaterial,rx:number,ry:number,
                draw:(c:CanvasRenderingContext2D,s:number)=>void,bump=.005){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=256;
    draw(canvas.getContext('2d')!,256);
    const t=new THREE.CanvasTexture(canvas);t.wrapS=t.wrapT=THREE.RepeatWrapping;
    t.repeat.set(rx,ry);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;
    textures.push(t);m.map=t;m.bumpMap=t;m.bumpScale=bump;m.needsUpdate=true;return m;
  }
  /** 中性灰底（默认 0.95 白）。 */
  const ground=(c:CanvasRenderingContext2D,v=242)=>{c.fillStyle=`rgb(${v},${v},${v})`;c.fillRect(0,0,256,256);};
  /** 拉丝：每 2px 一条明暗线。"金属"与"塑料"的分界几乎全靠这一手。
   *
   *  **线不能画成 1px。** 第一版是 256 条 1px、alpha .40、bumpScale .003，
   *  渲染出来车体上浮出一层 ~30cm 周期的宽横条，像刷了木纹 —— 原因不是
   *  颜色贴图，是 bumpMap：1px 的线在 2.7m 见方的一张图里等于 1cm 一个
   *  台阶，bumpScale .003 就是 3mm/1cm（17° 斜率），缩小时这些逐像素法线
   *  扰动被 mip 平均成低频斑块，看上去就是宽条。现在线宽 2px、alpha 减半、
   *  bumpScale 一律 ≤ .0016（见各材质），降到"只有掠射角才看得见"的强度。 */
  const brush=(c:CanvasRenderingContext2D,vertical:boolean,lo:number,hi:number,alpha=.26)=>{
    for(let i=0;i<256;i+=2){
      const v=Math.floor(lo+grainRandom()*(hi-lo));
      c.fillStyle=`rgba(${v},${v},${v},${alpha})`;
      if(vertical)c.fillRect(i,0,2,256);else c.fillRect(0,i,256,2);
    }
  };
  /** 低频软斑：大块的明暗起伏（风化 / 落尘）。高频纹理缩小后会消失，
   *  真正让大平面在远处"有东西"的其实是这一层。方一点、别做成长条 ——
   *  长条会读成"分带"，方块才读成"脏"。 */
  const blotch=(c:CanvasRenderingContext2D,n:number,lo:number,hi:number,alpha:number)=>{
    for(let i=0;i<n;i++){
      const v=Math.floor(lo+grainRandom()*(hi-lo));
      c.fillStyle=`rgba(${v},${v},${v},${alpha})`;
      c.fillRect(grainRandom()*256-30,grainRandom()*256-24,30+grainRandom()*96,20+grainRandom()*62);
    }
  };
  /** 斑点：灰尘 / 油污 / 锈点。alpha 很低，只在斜射光下才显形。
   *  `warm` 把点染成暖灰 —— 乘上去是"脏"，而不是"变色"。 */
  const specks=(c:CanvasRenderingContext2D,n:number,lo:number,hi:number,sz:number,alpha:number,warm=0)=>{
    for(let i=0;i<n;i++){
      const v=Math.floor(lo+grainRandom()*(hi-lo));
      const r=Math.min(255,v+warm),g=v,b=Math.max(0,v-Math.round(warm*.7));
      c.fillStyle=`rgba(${r},${g},${b},${alpha})`;
      c.fillRect(grainRandom()*256,grainRandom()*256,.6+grainRandom()*sz,.6+grainRandom()*sz);
    }
  };

  /* 车体奶油色：拉丝 + 板缝 + 大块风化 + 落尘。
   *
   * **repeat 的取法**：BoxGeometry 每面 UV 0..1，所以
   *   车体侧面 ±x 面 16.0m 长 / U repeat 8 → 板缝 2.0m 一道；
   *   车头端面 ±z 面 2.69m 宽 / U repeat 8 → 板缝 34cm 一道。
   * 34cm 的竖缝在车头上读作"分块面板"，是对的；但**不能同时给 V 方向也
   * 加缝**，否则车头会变成一张方格纸 —— 所以 canvas 里只有一条竖缝。 */
  M2.body=skin(M2.body,8,2,(c)=>{
    ground(c,242);
    brush(c,false,208,254,.30);
    blotch(c,22,226,255,.22);
    c.fillStyle='rgba(176,176,176,.62)';c.fillRect(0,0,3,256);
    c.fillStyle='rgba(255,255,255,.42)';c.fillRect(4,0,1,256);
    specks(c,1400,196,240,2.6,.11,10);
  },.0006);

  /* 车顶不锈钢：**repeat 取 (2,1)**，这是这一组里唯一需要算的。
   *   顶面 ±y 面 = 2.22m(x) × 15.90m(z)；U 沿 x、V 沿 z。
   *   U repeat 2 → 一图 1.11m（板缝 2 道横跨车顶，合理）；
   *   V repeat 1 → 一图铺满 15.9m！所以 canvas 里**竖直**方向的细节会变成
   *   沿车长方向的长条 —— 车顶的积灰正好就该是这个走向。
   * 第一版把 repeat 写成 (7,1)，等于在 2.22m 上塞 7 道缝（32cm 一道）。 */
  M2.roof=skin(M2.roof,2,1,(c)=>{
    ground(c,236);
    brush(c,false,168,252,.42);
    blotch(c,30,148,244,.26);
    c.fillStyle='rgba(150,150,150,.52)';c.fillRect(0,0,3,256);
    for(let i=0;i<140;i++){                       // 沿车长的灰道
      const v=Math.floor(140+grainRandom()*80);
      c.fillStyle=`rgba(${v},${v},${v},.18)`;
      c.fillRect(grainRandom()*256,0,1+grainRandom()*3,256);
    }
    specks(c,900,150,232,2.4,.16,10);
  },.0008);

  /* 黑色窗带 / 门套：大块纯黑最容易读成一块死黑。给它一层极淡的横向拉丝，
   * 掠射角下就有一条一条的高光，面积感立刻出来。 */
  M2.band=skin(M2.band,8,1,(c)=>{
    ground(c,244);
    brush(c,false,212,255,.30);
    blotch(c,14,232,255,.20);
    specks(c,600,202,246,2.2,.11);
  },.0006);

  /* 裙板：竖向筋条。±x 面 = 8.1m(z) × 0.42m(y)，U 沿 z、V 沿 y。
   * canvas 里的**竖线** → 世界的竖筋 ✓；筋距 = 8.1m / repeat 4 / 16 条 ≈ 12.6cm。
   * 筋条是**结构**不是噪声，宽度按 3px 给足，缩小时才不会退化成灰糊。 */
  M2.skirt=skin(M2.skirt,4,1,(c)=>{
    ground(c,240);
    for(let i=0;i<16;i++){
      const x=i*16;
      c.fillStyle='rgba(136,136,136,.66)';c.fillRect(x,0,3,256);
      c.fillStyle='rgba(255,255,255,.42)';c.fillRect(x+3,0,2,256);
    }
    blotch(c,16,166,248,.20);
    specks(c,1300,170,236,2.4,.17,16);
  },.0012);

  /* 床下枠 / 床下机器 / 連結器：不锈钢拉丝，比车顶粗，油污更重。 */
  M2.frame=skin(M2.frame,5,1,(c)=>{
    ground(c,238);
    brush(c,false,176,252,.40);
    blotch(c,22,152,246,.24);
    specks(c,1300,158,232,2.6,.20,18);
  },.0008);

  /* 转向架：铸钢件。粗颗粒 + 油污，这一带是全车最脏的，浓度给足。
   * 转向架的观察距离最近（玩家在站台俯看就是 1~2m），所以 bump 给到 .0016
   * 也安全 —— 高频只有在**大平面 + 远距离**才会变成宽条。 */
  M2.bogie=skin(M2.bogie,3,1,(c)=>{
    ground(c,240);
    specks(c,3400,150,254,4.0,.28,12);
    specks(c,800,116,190,5.5,.26,22);
    for(let i=0;i<80;i++){c.fillStyle='rgba(146,142,136,.20)';
      c.fillRect(grainRandom()*256,grainRandom()*256,2+grainRandom()*11,.8+grainRandom()*3);}
  },.0016);

  /* 车轮：踏面磨亮、辐板暗锈。整只轮子一档深钢 + 锈点。 */
  M2.wheel=skin(M2.wheel,3,1,(c)=>{
    ground(c,238);
    specks(c,1800,142,252,3.2,.26);
    specks(c,420,148,218,2.6,.26,50);
  },.0010);

  /* 受电弓 / 手すり：亮不锈钢，拉丝最细。 */
  M2.panto=skin(M2.panto,6,1,(c)=>{ground(c,244);brush(c,false,198,255,.42);},.0006);
  M2.hand =skin(M2.hand ,6,1,(c)=>{ground(c,248);brush(c,false,214,255,.38);},.0005);

  /* 卷帘：布纹（横竖两个方向的细密织纹），无金属。 */
  M2.blind=skin(M2.blind,3,1,(c)=>{
    ground(c,244);
    for(let i=0;i<256;i+=4){c.fillStyle='rgba(198,198,198,.34)';c.fillRect(0,i,256,1);}
    for(let i=0;i<256;i+=3){c.fillStyle='rgba(255,255,255,.26)';c.fillRect(i,0,1,256);}
    specks(c,500,206,242,1.8,.10);
  },.0008);

  /* 胶条 / 幌：橡胶，哑光，细颗粒。 */
  M2.rubber=skin(M2.rubber,10,2,(c)=>{
    ground(c,240);
    specks(c,4200,204,254,2.0,.20);
  },.0010);

  /* 粗糙度 / 金属度。金属度全部 ≤ .45（见上面 ④）。 */
  M2.body.roughness=.52;   M2.body.metalness=.08;
  M2.roof.roughness=.40;   M2.roof.metalness=.40;
  M2.band.roughness=.56;   M2.band.metalness=.18;
  M2.skirt.roughness=.56;  M2.skirt.metalness=.30;
  M2.frame.roughness=.46;  M2.frame.metalness=.36;
  M2.bogie.roughness=.70;  M2.bogie.metalness=.28;
  M2.wheel.roughness=.54;  M2.wheel.metalness=.32;
  M2.panto.roughness=.34;  M2.panto.metalness=.45;
  M2.hand.roughness=.30;   M2.hand.metalness=.42;
  M2.blind.roughness=.92;  M2.blind.metalness=0;
  M2.rubber.roughness=.94; M2.rubber.metalness=0;

  /** 圆角盒。低模里"有没有倒角"比"多几个面"重要得多，车体尤其。 */
  function rbox(w:number,h:number,d:number,r:number,m:THREE.Material,x:number,y:number,z:number){
    const o=new THREE.Mesh(new RoundedBoxGeometry(w,h,d,2,Math.min(r,Math.min(w,h,d)*.42)),m);
    o.position.set(x,y,z);o.castShadow=h>.15;o.receiveShadow=true;root.add(o);return o;
  }
  /** 轴向 X 的圆柱 / 圆盘：车轮、空気ばね、主電動機、がいし。 */
  function disc(x:number,y:number,z:number,r:number,t:number,m:THREE.Material){
    const o=new THREE.Mesh(cyl,m);o.position.set(x,y,z);o.scale.set(r,t,r);o.rotation.z=Math.PI/2;
    o.castShadow=true;o.receiveShadow=true;root.add(o);return o;
  }
  /** canvas 贴片材质缓存：同一段文字在两侧 / 两辆车上共用一份，别各建一张。 */
  const decalMats=new Map<string,THREE.MeshStandardMaterial>();
  function decal(key:string,cw:number,ch:number,w:number,h:number,x:number,y:number,z:number,ry:number,glow:number,
                 draw:(c:CanvasRenderingContext2D,w:number,h:number)=>void){
    let m=decalMats.get(key);
    if(!m){
      const canvas=document.createElement('canvas');canvas.width=cw;canvas.height=ch;
      draw(canvas.getContext('2d')!,cw,ch);
      const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;textures.push(t);
      m=new THREE.MeshStandardMaterial({map:t,roughness:.55,side:THREE.DoubleSide,emissive:'#ffffff',emissiveMap:t,emissiveIntensity:glow});
      m.userData.outlineWeight=0;materials.push(m);decalMats.set(key,m);
    }
    const g=new THREE.PlaneGeometry(w,h);geometries.add(g);
    const o=new THREE.Mesh(g,m);o.position.set(x,y,z);o.rotation.y=ry;root.add(o);return o;
  }

  /** 一辆车。`cabAt` = ±1：驾驶台在那一端，另一端是貫通端。
   *  `trackX` = 走行轨中心（86 = 西轨 / 91 = 东轨）。两条轨各有一列，车体不能再写死 86。 */
  function trainCar(trackX:number,cz:number,cabAt:number,num:string){
    const zc=cz+cabAt*CAR_HL;      // 车头端面
    const zg=cz-cabAt*CAR_HL;      // 貫通端面
    /** 按 (车, 窗位) 定死的伪随机：同一扇窗两侧状态一致，且每次载入不变。 */
    const pick=(k:number)=>(Math.imul(Math.round((cz+k)*100)|0,2654435761)>>>17)/32768;

    /* ---- ① 车体：圆角箱 ----
     * 车体 / 车肩 / 前脸面罩 / 貫通端面罩 全部走 M2.body。它们必须同材质：
     * M2.body 带一张拉丝贴图，平均亮度约 0.93，和纯色的 M.cream 摆在一起会
     * 差半档，接缝处能看出一条分界线。 */
    rbox(2.75,CAR_H,16,.13,M2.body,trackX,CAR_CY,cz);
    /* ---- ② 车顶：肩部（cant rail）+ 弧顶 + 雨檐 ---- */
    rbox(2.51,.22,15.96,.09,M2.body,trackX,CAR_Y0+CAR_H+.02,cz);
    rbox(2.22,.18,15.90,.08,M2.roof,trackX,CAR_Y0+CAR_H+.20,cz);
    for(const sx of [-1,1])box(trackX+sx*1.30,CAR_Y0+CAR_H+.04,cz,.09,.10,16.02,M2.band);

    /* ---- ③ 腰线：桜町色（粉 + 薄荷）+ 下缘细黑线 ---- */
    for(const sx of [-1,1]){
      const x=trackX+sx*(CAR_HW+.014);
      box(x,CAR_Y0+.60,cz,.03,.32,15.60,M.pink);
      box(x,CAR_Y0+.825,cz,.03,.06,15.60,M.mint);
      box(x,CAR_Y0+.20,cz,.03,.04,15.60,M2.band);
    }
    /* ---- ④ 黑色窗带：贯穿全长，车窗与车门都嵌在里面 ---- */
    for(const sx of [-1,1])box(trackX+sx*(CAR_HW+.004),FLR+.85,cz,.025,1.34,15.10,M2.band);

    /* ---- ⑤ 侧面：4 樘车窗 + 3 樘车门 + 驾驶台侧窗（方向幕） ---- */
    for(const sx of [-1,1]){
      const xg=trackX+sx*(CAR_HW+.010), xb=trackX+sx*(CAR_HW+.006), xd=trackX+sx*(CAR_HW+.020);
      for(const dz of [-3.6,-1.8,1.8,3.6]){
        const v=pick(dz), lit=v<.26, blind=!lit&&v<.44;
        /* 窗框板比玻璃大一圈（左右各 3cm、上下各 2cm），玻璃再外凸 5.5mm ——
         * 原来只有一块深色玻璃贴在黑窗带上，对比度太低，"窗户"读不出来。
         * 高度 0.90 而不是 0.94：0.94 时框板顶面正好落在 2.97，与窗带顶面
         * （FLR+.85+.67 = 2.97）**完全共面** —— 实测 52 对三角形、0.45m² 的
         * z-fighting。改 0.90 后：框顶 2.95 / 玻璃顶 2.93 / 窗带顶 2.97，
         * 三层依次错开 2cm，远看仍是"框包玻璃"，近看不再闪。 */
        box(xg,FLR+1.05,cz+dz,.025,.90,1.50,M2.frame);             // 窗框板
        box(xg+sx*.008,FLR+1.05,cz+dz,.02,.86,1.44,lit?glassLit:glassDim);  // 玻璃
        box(xg+sx*.012,FLR+1.05,cz+dz,.02,.90,.05,M.cream);        // 窗中挺
        box(xg+sx*.014,FLR+.56,cz+dz,.03,.05,1.52,M2.rubber);      // 窗台胶条
        if(blind)box(xg+sx*.016,FLR+1.25,cz+dz,.03,.42,1.42,M2.blind);   // 半降卷帘
      }
      /* 両開き拉门。门套是黑的（与窗带同色 → 窗带视觉连续），门扇是奶油色
       * （把窗带断开），中央缝 + 门边胶条 + 手すり + 乗降ステップ 一个不少。 */
      for(const dz of [-5.4,0,5.4]){
        const czd=cz+dz;
        box(xb,FLR+.90,czd,.05,1.80,1.40,M2.band);                 // 门套
        for(const s of [-1,1]){
          box(xd,FLR+.90,czd+s*.325,.03,1.72,.58,M.cream);         // 门扇
          box(xd+sx*.012,FLR+1.12,czd+s*.325,.02,.78,.44,glassDim); // 门玻璃
          box(xd+sx*.018,FLR+1.12,czd+s*.325,.015,.82,.04,M.cream);
          /* 踢脚比门扇再往外 1cm：原来两者 x 区间完全相同（84.590..84.620），
           * 两面朝车外的面就共面了 —— 实测 4 处、每处 2.28m² 的 z-fighting。 */
          box(xd+sx*.010,FLR+.22,czd+s*.325,.03,.34,.56,M2.band);  // 踢脚（外凸 1cm）
        }
        box(xd,FLR+.90,czd,.03,1.72,.035,M2.rubber);               // 中央缝
        for(const s of [-1,1])box(xd,FLR+.90,czd+s*.645,.03,1.72,.035,M2.rubber);
        for(const s of [-1,1]){                                     // 手すり
          rod([trackX+sx*(CAR_HW+.10),FLR+.02,czd+s*.76],[trackX+sx*(CAR_HW+.10),FLR+1.66,czd+s*.76],.022,M2.hand);
          rod([trackX+sx*(CAR_HW+.10),FLR+1.66,czd+s*.76],[trackX+sx*(CAR_HW+.10),FLR+1.66,czd+s*.30],.022,M2.hand);
        }
      }
      /* 驾驶台侧窗 = 侧面行先表示（方向幕），发光板。x 取 +0.022：窗带外皮在
       * +0.0165，贴片再往外 5.5mm，免得两块面共面闪。 */
      decal('roll',384,160,1.16,.86,trackX+sx*(CAR_HW+.026),FLR+1.05,cz+cabAt*7.0,sx>0?Math.PI/2:-Math.PI/2,.5,(c,w,h)=>{
        c.fillStyle='#121a21';c.fillRect(0,0,w,h);
        c.fillStyle='#ffe9b8';c.textAlign='center';
        c.font='bold 58px "Microsoft YaHei", sans-serif';c.fillText('各駅停車',w/2,68,w-16);
        c.font='34px "Microsoft YaHei", sans-serif';c.fillText('春日野',w/2,124,w-16);
      });
      /* 车号：贴在腰线下方。x 必须**比腰线外皮再往外**：三条腰线（黑细线 /
       * 粉带 / 薄荷带）都画在 CAR_HW+.014 处，外皮 84.604；车号原来也在
       * +.014，等于埋在线条实体里面，数字被黑线横切一刀。改 +.034 后车号
       * 浮在线条外 5mm，完整可读。 */
      decal('num-'+num,256,80,.66,.20,trackX+sx*(CAR_HW+.034),CAR_Y0+.20,cz+cabAt*6.6,sx>0?Math.PI/2:-Math.PI/2,.06,(c,w,h)=>{
        c.fillStyle='#2b3339';c.textAlign='center';c.font='bold 46px "Microsoft YaHei", sans-serif';
        c.fillText(num,w/2,56,w-8);
      });
    }

    /* ---- 乗降ステップ：**故意不画** ----
     * 原来这里有一个 `for(const dz of [-5.4,0,5.4]) box(trackX,CAR_Y0+.28,cz+dz,2.85,.10,1.36,M2.band)`，
     * 是整辆车最大的 z-fighting 来源，两个独立的错：
     *   ① 踏み板是以 trackX 为中心的**一个**箱体，却写在了 `for(const sx of [-1,1])`
     *      循环内部 —— 左右两侧各画一次，两个箱体 6 个面全部重合。实测 6 处共面
     *      22.8m²（#232c34 对 #eee4d1），连侧面/端面一共 11 处。教训：以车体
     *      中线为轴的对称零件必须放在 sx 循环**外面**。
     *   ② 更根本的是它本来就不该存在：本段站台面 1.24m、车地板 1.45m，只差 21cm，
     *      低床ホーム用的踏み板在这里读不出任何东西；而它的外缘 84.575 距站台边
     *      84.52 只有 5.5cm，看上去倒像车体蹭着站台。
     * 省下的面数已经加到窗/门的细节上，视觉收益更大。 */

    /* ---- ⑥ 车头：前脸 + 前窗 + 方向幕 + 头尾灯 + 排障器 + 連結器 ---- */
    {
      box(trackX,CAR_CY,zc+cabAt*.02,2.69,CAR_H-.06,.06,M2.body);              // 前脸面罩
      for(const sx of [-1,1]){
        const w=new THREE.Mesh(cube,glassDim);                                // 前窗（后倾 8.6°）
        w.position.set(trackX+sx*.60,CAR_Y0+1.98,zc+cabAt*.05);
        w.scale.set(1.00,.90,.05);w.rotation.x=-cabAt*.15;
        w.castShadow=true;w.receiveShadow=true;root.add(w);
        box(trackX+sx*.60,CAR_Y0+1.49,zc+cabAt*.045,1.06,.07,.08,M2.band);
        box(trackX+sx*.60,CAR_Y0+2.48,zc+cabAt*.085,1.06,.07,.08,M2.band);
        box(trackX+sx*1.13,CAR_Y0+1.98,zc+cabAt*.060,.08,1.00,.08,M2.band);
        rod([trackX+sx*.32,CAR_Y0+1.62,zc+cabAt*.10],[trackX+sx*.80,CAR_Y0+2.34,zc+cabAt*.10],.014,M2.hand);   // 雨刷
      }
      box(trackX,CAR_Y0+1.98,zc+cabAt*.07,.13,1.02,.09,M2.body);                // 中央立柱
      box(trackX,CAR_Y0+2.78,zc+cabAt*.05,1.42,.36,.06,M2.band);                // 方向幕灯箱
      decal('dest',512,128,1.30,.30,trackX,CAR_Y0+2.78,zc+cabAt*.085,cabAt>0?Math.PI:0,.55,(c,w,h)=>{
        c.fillStyle='#101820';c.fillRect(0,0,w,h);
        c.fillStyle='#ffe9b8';c.textAlign='center';
        c.font='bold 58px "Microsoft YaHei", sans-serif';c.fillText('各駅停車',w/2,62,w-20);
        c.font='32px "Microsoft YaHei", sans-serif';c.fillText('春日野  ·  LOCAL',w/2,110,w-20);
      });
      /* 前后灯：一个灯箱里并排两枚 —— 外侧前照灯（暖白），内侧尾灯（红）。
       * 高度必须在前窗上框（CAR_Y0+2.48±0.035）之上，否则灯箱会切进窗框。 */
      for(const sx of [-1,1]){
        box(trackX+sx*1.02,CAR_Y0+2.78,zc+cabAt*.045,.62,.32,.06,M2.band);      // 共用灯箱
        box(trackX+sx*1.19,CAR_Y0+2.78,zc+cabAt*.078,.24,.18,.03,M.light);      // 前照灯
        box(trackX+sx*0.86,CAR_Y0+2.78,zc+cabAt*.078,.20,.14,.03,lampTail);     // 尾灯
      }
      box(trackX,CAR_Y0-.30,zc+cabAt*.10,2.30,.36,.09,M2.band);                 // 排障器（下缘 0.545，恰在轨面上方）
      box(trackX,CAR_Y0-.34,zc+cabAt*.22,.32,.22,.44,M2.frame);                // 連結器
      rod([trackX-.17,CAR_Y0-.34,zc+cabAt*.30],[trackX+.17,CAR_Y0-.34,zc+cabAt*.30],.05,M2.frame);
    }

    /* ---- ⑦ 貫通端：貫通路 + 貫通扉窓 + 手すり ---- */
    {
      const zn=zg-cabAt*.02;
      box(trackX,CAR_CY,zn,2.69,CAR_H-.06,.06,M2.body);
      /* 貫通扉用不锈钢灰而不是纯黑：纯黑在车端读成"一个洞"，而这一面本来就
       * 在阴影里，再压黑就没有层次了。窗另加一圈奶油色窗套。 */
      box(trackX,CAR_Y0+1.42,zn-cabAt*.03,1.46,2.42,.06,M2.frame);
      box(trackX,CAR_Y0+1.74,zn-cabAt*.05,.90,1.02,.04,M.cream);
      box(trackX,CAR_Y0+1.74,zn-cabAt*.06,.74,.86,.04,glassDim);
      for(const s of [-1,1])rod([trackX+s*.90,CAR_Y0+.05,zn-cabAt*.07],[trackX+s*.90,CAR_Y0+2.60,zn-cabAt*.07],.022,M2.hand);
    }

    /* ---- ⑧ 床下枠 / 裙板 / 床下机器 / 转向架 ---- */
    box(trackX,CAR_Y0-.06,cz,2.54,.10,15.20,M2.frame);                          // 床下枠
    // 裙板在转向架处断开 —— 真车的裙板本来就有缺口，缺口里才看得见轮对。
    // 宽 2.79 > 车体 2.75：裙板必须比车体**鼓出来**才看得见，等宽就等于埋进车体里。
    /* 高度 CAR_Y0+.14（0.955..1.375）而不是 +.21（1.025..1.445）：后者下沿正好
     * 压在车体底面上，两块朝下的面完全共面 —— 实测 **48.3m²**，是全车最大的
     * 一处 z-fighting（#3f4952 对 #eee4d1）。改 +.14 后裙板从车底再垂下来 7cm，
     * 上沿 1.375 埋进车体内部、下沿 0.955 高于床下枠底 0.915，与谁都不同面。 */
    for(const [z0,z1] of [[cz-7.55,cz-6.75],[cz-4.05,cz+4.05],[cz+6.75,cz+7.55]] as const)
      box(trackX,CAR_Y0+.14,(z0+z1)/2,2.79,.42,z1-z0,M2.skirt);
    // 床下机器：挂在床下枠下面，左右错开（真车也是左右不等宽）
    for(const [dz,w,h,d,ox] of [[-3.5,1.36,.36,1.95,-.14],[-1.3,1.06,.32,1.30,.14],
                                [.9,1.44,.34,2.05,-.10],[3.0,.92,.30,1.15,.16]] as const)
      box(trackX+ox,CAR_Y0-.11-h/2,cz+dz,w,h,d,M2.frame);
    for(const bz of [-BOGIE_DZ,BOGIE_DZ]){
      const bzz=cz+bz;
      for(const sx of [-1,1])box(trackX+sx*1.00,CAR_Y0-.22,bzz,.15,.46,2.72,M2.bogie);   // 侧架
      box(trackX,CAR_Y0-.24,bzz,1.86,.26,.48,M2.bogie);                                   // 横梁
      box(trackX,CAR_Y0-.05,bzz,1.28,.20,.62,M2.bogie);                                   // 揺枕
      for(const sx of [-1,1])disc(trackX+sx*.56,CAR_Y0-.06,bzz,.21,.24,M2.band);          // 空気ばね
      disc(trackX,WHL_Y-.06,bzz,.24,1.06,M2.bogie);                                       // 主電動機（吊在轴高，不得低于轨面）
      for(const sx of [-1,1])for(const dz of [-.95,.95]){
        disc(trackX+sx*.74,WHL_Y,bzz+dz,WHL_R,.11,M2.wheel);                              // 车轮
        disc(trackX+sx*.74,WHL_Y,bzz+dz,.14,.13,M2.bogie);                                // 轮心
        box(trackX+sx*.90,WHL_Y,bzz+dz,.26,.24,.26,M2.bogie);                             // 軸箱
        box(trackX+sx*.62,.62,bzz+dz+(dz>0?.21:-.21),.16,.16,.09,M2.band);                // 制動子
      }
    }

    /* ---- ⑨ 车顶设备：冷房装置 / 高圧配線 / 通風器 / 受電弓 ---- */
    for(const dz of [-5.2,0,5.2]){
      rbox(1.62,.30,2.30,.06,M.cream,trackX,ROOF+.15,cz+dz);                    // 集中冷房装置
      for(let i=0;i<7;i++)box(trackX-.60+i*.20,ROOF+.325,cz+dz,.05,.05,2.16,M2.band);   // 顶部散热片
      for(const s of [-1,1])box(trackX,ROOF+.325,cz+dz+s*1.16,.30,.10,.05,M2.band);
    }
    for(const sx of [-1,1])box(trackX+sx*.86,ROOF+.06,cz,.06,.06,15.00,M2.band);   // 高圧配線
    for(const dz of [-3.2,3.2])box(trackX,ROOF+.09,cz+dz,1.10,.12,.36,M2.roof);    // 通風器
    if(cabAt<0){
      // シングルアーム受電弓：滑板顶面正好落在接触网 6.05 上
      box(trackX,ROOF+.06,cz+1.6,1.24,.10,1.90,M2.panto);
      for(const sx of [-1,1])for(const dz of [-.75,.75])
        disc(trackX+sx*.50,ROOF+.24,cz+1.6+dz,.075,.20,M.cream);
      rod([trackX-.34,ROOF+.34,cz+1.05],[trackX,ROOF+1.05,cz+1.95],.035,M2.panto);
      rod([trackX+.34,ROOF+.34,cz+1.05],[trackX,ROOF+1.05,cz+1.95],.035,M2.panto);
      rod([trackX,ROOF+1.05,cz+1.95],[trackX,ROOF+1.42,cz+1.15],.032,M2.panto);
      box(trackX,ROOF+1.435,cz+1.15,1.70,.06,.16,M2.panto);                     // 集電舟
      box(trackX,ROOF+1.472,cz+1.15,1.46,.03,.07,M2.band);                      // 滑板（顶面 6.05 = 接触网）
      for(const sx of [-1,1])rod([trackX+sx*.85,ROOF+1.45,cz+1.15],[trackX+sx*1.02,ROOF+1.28,cz+1.15],.028,M2.panto);
      rod([trackX,ROOF+.06,cz-4.2],[trackX,ROOF+.70,cz-4.2],.022,M2.band);        // 天线
      disc(trackX,ROOF+.74,cz-4.2,.22,.03,M2.band);
    }
  }

  /* ================= 一列编组 =================
   *
   * 原版这里只有一段顺序代码，建完就合批成 'sakura-local-train' 一个组。现在要
   * 两列（一条轨一列），所以抽成工厂：建两辆车 + 貫通幌 → 切片取出这期间挂到
   * `root` 上的所有网格 → 装进自己的组 → 各自 mergeByMaterial。
   *
   * 为什么必须**各自合批**：mergeByMaterial 是把整棵子树按材质压平的，两列合在
   * 一个组里，就再也没有"两列分别剔除"这回事了 —— 站在站台上朝北看，南边那一列
   * 本来可以被视锥整列剔掉，合在一起就得连顶点一起提交。
   *
   * 碰撞盒同理：`colliders.splice(boxStart)` 只切走本次新建的那几个，两列互不干扰。
   * 每次 update 里按当前 offset 平移的也是各自那一份 base。
   */
  function consist(trackX:number,name:string,numbers:readonly[string,string]){
    const meshStart=root.children.length,boxStart=colliders.length;
    /* 两辆车的中心 z 取自 trainSchedule 的 CAR_CENTRES —— 那里同时用它们推
     * 编组中心 / 半长（STOP_C / CONSIST_HL），车长改了时刻表会自己跟着走。 */
    for(const [cz,cabAt,num] of [[CAR_CENTRES[0],-1,numbers[0]],[CAR_CENTRES[1],1,numbers[1]]] as const){
      trainCar(trackX,cz,cabAt,num);
      /* 碰撞盒一个车一个。min.y 压到轨面：轮对 / 转向架 / 床下机器全在盒里，
       * 玩家既不能从侧面挤进去，也不能从车底钻过去。z 方向多给 0.45 ——
       * 連結器与排障器伸在车体端面之外。 */
      colliders.push({source:'station-parked-train',kind:'wall',
        min:new THREE.Vector3(trackX-1.44,.51,cz-CAR_HL-.45),
        max:new THREE.Vector3(trackX+1.44,4.45,cz+CAR_HL+.45)});
    }
    /* ---- ⑩ 車間：貫通幌 / 連結器 / ジャンパ線 ---- */
    {
      const z0=56,z1=57;
    /* 幌芯要**封住整张端面**，不能只做门口那一块。
     * 第一版芯只有 1.50×2.50（y 1.045..3.545），而车体高 3.25（1.025..4.275）——
     * 上方 73cm、两侧各 62cm 全是敞的，从站台正对着看就是一个贯通编组的洞，
     * 背后直接是樱花树（F4-gangway 那一机位一眼就看出来了）。
     * 真车的貫通幌本来就是盖住整个端面的大方块（只有連結器和ジャンパ線露在
     * 下面），所以直接放大：宽 2.60（端面罩 2.69）、高到 4.15（车顶 4.275）。
     * 剩下的缝是两侧各 4.5cm、上方 12.5cm，已经窄到读不出"洞"。
     * 蛇腹褶同步加宽到 2.64/2.72（比芯鼓 2~6cm，才看得见褶），高度 2.66 收在
     * 芯的范围内 —— 褶只应该在 x 方向凸出来。
     *
     * 进深（z）也必须三块各不相同：这一带挤着 8 个 z 平面 —— 1 号车端面罩
     * 55.99..56.05、貫通扉 56.02..56.08、扉窓套 56.05..56.09、扉窓ガラス
     * 56.06..56.10；2 号车镜像 56.90..56.94 / 56.91..56.95 / 56.92..56.98 /
     * 56.95..57.01。原来芯 .96（56.02/56.98）、渡り板 1.00（56.00/57.00）、
     * 褶子起点 56.10 —— 与扉窓ガラス、貫通扉各撞一次，实测 4 处 1.43m²。
     * 现在芯 .92（56.04/56.96，两端埋进对方端面罩里，端面根本露不出来）、
     * 渡り板 .86（56.07/56.93）、褶子 56.16 起步距 .17（56.11..56.89），
     * 彼此至少隔 1cm。改这几行之前先把 tmp/_zfight-train.mjs 跑一遍。 */
      box(trackX,CAR_Y0+1.5725,(z0+z1)/2,2.60,3.105,.92,M2.band);
      box(trackX,FLR-.02,(z0+z1)/2,1.30,.06,.86,M2.frame);
      for(let i=0;i<5;i++)box(trackX,CAR_Y0+1.42,z0+.16+i*.17,i%2?2.72:2.64,2.66,.10,i%2?M2.band:M2.rubber);
      for(const dx of [-.30,.30])rod([trackX+dx,CAR_Y0-.36,z0],[trackX+dx,CAR_Y0-.36,z1],.05,M2.frame);
      box(trackX,CAR_Y0-.36,(z0+z1)/2,.70,.22,.30,M2.frame);
      for(const dx of [-.95,.95]){                                 // ジャンパ線（略下垂）
        rod([trackX+dx,CAR_Y0-.30,z0+.02],[trackX+dx,CAR_Y0-.40,(z0+z1)/2],.022,M2.band);
        rod([trackX+dx,CAR_Y0-.40,(z0+z1)/2],[trackX+dx,CAR_Y0-.30,z1-.02],.022,M2.band);
      }
    }
    const group=new THREE.Group();group.name=name;group.userData.noMerge=true;
    for(const child of root.children.slice(meshStart))group.add(child);
    root.add(group);mergeByMaterial(group);
    const boxes=colliders.splice(boxStart);
    return {group,boxes,base:boxes.map(c=>({min:c.min.clone(),max:c.max.clone()}))};
  }
  /* 西轨那列保留原名 'sakura-local-train' —— tmp/_zfight-train.mjs 的 ZF_TARGET
   * 默认值就是它，改名等于把那条验收线悄悄停掉。东轨那列另起一个名字。 */
  const trains=[
    consist(TRKX,  'sakura-local-train',      ['クハ2001','モハ2002']),
    consist(TRKX_E,'sakura-local-train-east', ['クハ2003','モハ2004']),
  ];
  // Street-facing station annex and an inhabited florist frontage.
  function shop(x:number,z:number,title:string,color:THREE.Material){
    solid('shop-shell',x,3.1,z,7,6.2,6,color);box(x,6.3,z,7.6,.2,6.5,M.dark);
    box(x,1.6,z-3.06,6.5,2.8,.06,M.glass);
    for(const dx of [-3.2,-1.1,1.1,3.2])box(x+dx,1.6,z-3.15,.09,2.9,.12,M.cream);
    for(const dx of [-2,1.9]){box(x+dx,4.65,z-3.06,1.8,1.5,.09,M.dark);box(x+dx,4.65,z-3.13,1.6,1.3,.04,M.glass);box(x+dx,4.65,z-3.18,.06,1.3,.035,M.cream);}
    sign(title,'SAKURA TOWN · OPEN 09:00–19:00',x,3.4,z-3.22,6.6,.8,Math.PI);
    const awning=box(x,2.92,z-3.65,7,.1,1.3,M.mint);awning.rotation.x=-.13;
    for(let i=0;i<7;i++)box(x-3+i,2.78,z-4.25,.5,.3,.05,M.cream);
    rod([x+3.4,5.9,z-3.2],[x+3.4,.1,z-3.2],.055,M.steel);
    for(let j=0;j<6;j++)for(let row=0;row<2;row++){
      const px=x-2.6+j*.62,pz=z-4.2-row*.5;
      box(px,.34,pz,.38,.45,.35,M.cream);for(let k=0;k<3;k++){rod([px,.5,pz],[px+(k-1)*.12,.95+k*.12,pz],.018,M.grass);box(px+(k-1)*.12,.95+k*.12,pz,.19,.14,.17,k%2?M.yellow:M.pink);}
    }
    solid('flower-display',x-.9,.5,z-4.45,4.1,1,.95,M.wood);root.remove(root.children[root.children.length-1]);
  }
  shop(108,21,'花屋 はなのわ',M.cream);shop(119,21,'こはる文具店',M.mint);
  // A continuous residential lane turns the road end into a inhabited neighbourhood.
  solid('east-lane',132,.03,34,5,.06,70,M.road,'floor');
  for(const x of [128.5,135.5])solid('lane-footpath',x,.055,34,2,.11,70,M.stone,'floor');
  function house(x:number,z:number,index:number){
    const facade=[M.cream,M.mint,M.stone][index%3];
    solid('house',x,2.8,z,7.4,5.6,7.2,facade);
    // Two sloping roof planes, ridge cap, eaves, fascia and gutter.
    for(const side of [-1,1]){const roof=box(x+side*2,6.05,z,4.45,.16,8,M.dark);roof.rotation.z=-side*.29;box(x+side*4,5.46,z,.12,.18,8,M.steel);}
    box(x,6.7,z,.18,.16,8.05,M.steel);
    for(const dx of [-2.25,.2,2.3])for(const y of [1.75,4.15]){
      box(x+dx,y,z-3.64,1.55,1.4,.09,M.steel);box(x+dx,y,z-3.7,1.38,1.23,.04,M.glass);
      box(x+dx,y,z-3.74,.05,1.23,.03,M.cream);box(x+dx,y-.76,z-3.76,1.7,.08,.3,M.cream);
      if(y>3){box(x+dx,y-.82,z-4.05,1.8,.12,.8,M.stone);for(let k=0;k<8;k++)box(x+dx-.8+k*.23,y-.42,z-4.45,.027,.8,.035,M.steel);box(x+dx,y,z-4.45,1.8,.05,.05,M.steel);}
    }
    box(x-2.3,1.25,z-3.79,1.12,2.5,.14,M.wood);box(x-1.93,1.2,z-3.89,.045,.32,.04,M.steel);
    box(x-2.3,2.65,z-4,1.6,.12,.9,M.dark);box(x-2.3,.12,z-4,1.5,.24,.8,M.stone);
    rod([x+3.77,5.5,z-3.65],[x+3.77,.15,z-3.65],.047,M.steel);
    box(x+3.82,1.65,z-2,.16,.5,.34,M.steel);
    box(x+2,.46,z-3.98,.9,.65,.36,M.cream);for(let v=0;v<7;v++)box(x+1.63+v*.12,.46,z-4.18,.04,.46,.025,M.steel);
    solid('garden-wall',x+1.5,.47,z-5.9,4.5,.94,.18,M.stone);
    solid('garden-side',x+4.15,.47,z-1,.18,.94,9.8,M.stone);
    for(let n=0;n<4;n++){box(x+.2+n*.7,.24,z-5.1,.4,.48,.4,M.wood);rod([x+.2+n*.7,.45,z-5.1],[x+.2+n*.7,1,z-5.1],.025,M.grass);box(x+.2+n*.7,.9,z-5.1,.45,.3,.4,n%2?M.mint:M.pink);}
    box(x-3.3,.9,z-5.9,.5,.32,.25,M.red);rod([x-3.3,0,z-5.9],[x-3.3,.85,z-5.9],.04,M.steel);
    sign(`桜町 ${index+1}丁目`,'SAKURA RESIDENCE',x-2.3,2.1,z-3.9,.8,.25,Math.PI);
  }
  for(const [x,z,i] of [[108,-7,0],[120,-7,1],[140,10,2],[116,40,3],[116,57,4],[140,53,5]])house(x,z,i);
  for(const x of [126.5,137.5])for(const z of [-3,28,60]){
    solid('utility-pole',x,4,z,.2,8,.2,M.stone);box(x,7.3,z,2.4,.12,.12,M.steel);
    for(const dx of [-.8,0,.8]){rod([x+dx,7.4,z],[x+dx,7.4,z+28],.014,M.dark);box(x+dx,7.45,z,.12,.23,.12,M.cream);}
    box(x,.8,z,.23,1.6,.23,M.yellow);for(let j=0;j<4;j++)box(x,.25+j*.4,z-.125,.24,.15,.02,M.dark);
  }
  // Vending machine beside the plaza, with individually modelled products and controls.
  for(const x of [70,71.4]){
    solid('vending',x,1.05,44,1.15,2.1,.7,x===70?M.mint:M.cream);box(x,1.35,43.63,.87,1.04,.04,M.dark);
    for(let row=0;row<3;row++)for(let col=0;col<5;col++){box(x-.33+col*.165,1.03+row*.3,43.59,.105,.2,.04,[M.pink,M.yellow,M.white][col%3]);box(x-.33+col*.165,.89+row*.3,43.57,.09,.035,.02,M.light);}
    box(x,.35,43.62,.72,.22,.04,M.dark);box(x+.44,.72,43.61,.08,.2,.03,M.steel);
  }
  // Station forecourt: tiled entrance canopy, timetable, clock and drainage.
  for(const x of [69,75])solid('entrance-post',x,1.9,38,.18,3.8,.18,M.steel);
  box(72,3.9,38,7,.18,3.8,M.cream);
  sign('桜町駅','SAKURAMACHI STATION',72,3.25,36.08,4.8,.85,Math.PI);
  solid('timetable-case',69,.95,39,.18,1.9,1.6,M.steel);
  sign('時刻表  /  ご案内','06:15  06:35  06:55  ·  LOCAL',68.89,1.35,39,1.4,.9,-Math.PI/2);
  const clockCanvas=document.createElement('canvas');clockCanvas.width=clockCanvas.height=256;
  const cc=clockCanvas.getContext('2d')!;cc.fillStyle='#efe9da';cc.fillRect(0,0,256,256);
  cc.strokeStyle='#3d4d58';cc.lineWidth=9;cc.beginPath();cc.arc(128,128,116,0,Math.PI*2);cc.stroke();
  for(let n=0;n<12;n++){const a=n*Math.PI/6;cc.beginPath();cc.moveTo(128+Math.sin(a)*96,128-Math.cos(a)*96);cc.lineTo(128+Math.sin(a)*108,128-Math.cos(a)*108);cc.stroke();}
  cc.lineWidth=7;cc.beginPath();cc.moveTo(128,128);cc.lineTo(175,153);cc.moveTo(128,128);cc.lineTo(128,49);cc.stroke();
  const clockTexture=new THREE.CanvasTexture(clockCanvas);clockTexture.colorSpace=THREE.SRGBColorSpace;textures.push(clockTexture);
  const clockMaterial=new THREE.MeshStandardMaterial({map:clockTexture,roughness:.8,side:THREE.DoubleSide});clockMaterial.userData.outlineWeight=0;materials.push(clockMaterial);
  const clockGeometry=new THREE.CircleGeometry(.43,32);geometries.add(clockGeometry);
  for(const x of [82.47,94.55]){rod([x,PT+3.76,52],[x,PT+2.86,52],.025,M.steel);const clock=new THREE.Mesh(clockGeometry,clockMaterial);clock.position.set(x,PT+2.51,52);root.add(clock);}
  for(let x=101;x<132;x+=4){box(x,.135,15.65,.7,.025,.24,M.dark);for(let i=0;i<7;i++)box(x-.3+i*.1,.152,15.65,.025,.01,.24,M.stone);}
  for(const x of [79.6,97.6])for(let z=-30;z<92;z+=1.7){
    if(z>4&&z<17)continue;
    for(let j=0;j<3;j++){const px=x+(j-1)*.17;rod([px,.05,z],[px,.45+j*.08,z+.08],.014,M.grass);box(px,.45+j*.08,z+.08,.12,.09,.12,M.yellow);}
  }
  // Blossom trees use the same fine alpha-tested foliage as the original town.
  let seed=815;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  const blossoms=createBlossomField(random,2400);
  /* 树干走 Blender 资产（`foliage` 的干场），和河堤/行道树/商店街同一套几何。
   *
   * 这里原来是 `solid('tree-trunk',x,1.4,z,.48,2.8,.48,M.wood)` —— 一根 0.48m 见方的
   * **方柱**。碰撞盒尺寸（0.48 × 2.8）**原样保留**，只把视觉换成带板根和树皮棱沟的
   * 模型；视觉半径取 0.19，比碰撞盒细一圈，这一排站台樱读起来才像树。
   * 干场统一 build：合批之后再挂（见 mergeByMaterial 那行）。 */
  const trunks=createTrunkField();
  for(const x of [77.5,100])for(const z of [-22,-8,22,39,56,73,90]){
    colliders.push({source:'station-tree-trunk',kind:'wall',
      min:new THREE.Vector3(x-.24,0,z-.24),max:new THREE.Vector3(x+.24,2.8,z+.24)});
    trunks.add({x,z,y0:0,h:2.8,r:.19,variant:'sakura',
      lean:.05,leanDir:x>88?1.1:-1.1,seed:(x|0)*7+(z|0)});
    for(let b=0;b<5;b++){const a=b*1.256,tx=x+Math.cos(a)*1.7,tz=z+Math.sin(a)*1.7;rod([x,1.7,z],[tx,4.2,tz],.1,M.wood);
      for(let n=0;n<26;n++)blossoms.spray(tx+(random()-.5)*2.5,4.3+random()*1.9,tz+(random()-.5)*2.5,.4+random()*.45);
    }
    box(x,.11,z,1.5,.2,1.5,M.soil);
  }
  mergeByMaterial(root);
  root.add(trunks.build('station-tree-trunks',M.wood.color));
  root.add(blossoms.build('station-cherry-blossoms'));
  root.userData.plan={phase:2,crossing:[88.5,10.1],platforms:[[82.47,59],[94.55,59]],train:'one-way-pair'};
  /* ---------------- 行车参数 ----------------
   *
   * 编组几何 / 行程端点 / 加减速 / 停站时长 / 道口窗口**全部**在 trainSchedule.ts 里，
   * 这里只 import。理由写在那个文件头：这张时刻表的每条性质都要整周期对账，而这个
   * 场景在 headless 下只有 0.4fps，跑一遍周期要 40 分钟 —— 抽成纯函数之后 Node 里
   * 毫秒级全量核账（tmp/_check-schedule.ts），浏览器只验"接线对不对"。
   *
   * 落地到这里的只有一件事：编组是按**绝对坐标**建的（中心落在 STOP_C），所以每帧
   * 要写进 group.position.z 的是"目标中心 - STOP_C"。
   */
  /* 时刻表发一份到 userData，给核账脚本对表。 */
  root.userData.schedule={cycle:CYCLE,dwell:DWELL,stopC:STOP_C,consistHL:CONSIST_HL,
    runHalf:RUN_HALF,phaseEast:PHASE_E,legIn:LEG_IN.total,legOut:LEG_OUT.total,
    initialPhase:INITIAL_PHASE,westWin,eastWin,
    westTrack:TRKX,eastTrack:TRKX_E,westDir:'+z（自北向南）',eastDir:'-z（自南向北）'};
  /* 当前相位 / 栏杆放下量的**只读快照**（同一个对象每帧改字段，不重新分配）。
   * 有了它，核账脚本才能把"编组位置"和"时刻表相位"对上账 —— 否则从 group.position
   * 里读出来的数只能看出"它在动"，看不出"它走到哪一段了"、也判不了停站。 */
  root.userData.live={west:0,east:0,closure:0};
  /* 开局就停在站台上（两列同时），载入画面与改前一致，自检行读到的也是「列车停站」。 */
  let time=INITIAL_PHASE, closurePrev=0;
  /* 相位跳转，只给验收脚本用。存在的理由很实在：这个场景在 headless swiftshader 下
   * 只有 0.4fps，而 update 里 dt 被夹到 0.1s —— 于是时刻表每秒只推进 0.04s，
   * 想守到"道口栏杆落下且编组正压在上面"那个瞬间要等十几分钟（实测相位 65→28.9
   * 需 ~20 分钟）。有了 seek，"某个相位长什么样"变成确定性的、瞬时的。
   * closurePrev 一并对齐，免得跳转后第一帧把状态字判成错误的闭合/开启沿。 */
  root.userData.seek=(t:number)=>{ time=wrap(t,CYCLE);
    closurePrev=closureAt(wrap(time,CYCLE),wrap(time+PHASE_E,CYCLE)); };
  function update(dt:number,camera:THREE.Camera){
    /* 玩家站在轨道上 → 时刻表暂停。这是唯一一处"安全优先于演出"。
     *
     * 判据从"整条轨床"改成"编组附近"：原版编组只在 z∈[-80,73] 之间折返，所以
     * 那个盒子（x 83.9~88.1 / z -43~75）实际上等于"编组会扫过的范围"。现在单向
     * 行车扫的是整条 ±440 的轨，照搬成 z∈[-78,95] 会把站台边缘也算进去 ——
     * 点字ブロック 就在 x=84.22，而站台上看车恰恰是玩家最常站的位置，
     * 于是整个动画会一直冻着。那是把安全做成了故障。
     * 改成：玩家在轨床上 **且** 某列编组在 45m 以内（≈2s 行程）才暂停；
     * 道口铺面里无条件暂停（原样保留）。 */
    const p=camera.position;
    const onRails=p.y<5&&p.x>83.9&&p.x<93.1&&p.z>-78&&p.z<95;
    const inCrossing=p.y<5&&p.x>80&&p.x<97&&p.z>5&&p.z<15;
    const nearTrain=trains.some(tr=>Math.abs(tr.group.position.z+STOP_C-p.z)<CONSIST_HL+45);
    const occupied=inCrossing||(onRails&&nearTrain);
    if(!occupied)time=(time+Math.min(dt,.1))%CYCLE;
    const tW=time,tE=wrap(time+PHASE_E,CYCLE);
    /* 编组是按**绝对坐标**建的（中心落在 STOP_C），所以位移量是"目标中心 - STOP_C"。 */
    trains[0].group.position.z=westC(tW)-STOP_C;
    trains[1].group.position.z=eastC(tE)-STOP_C;
    dynamicColliders.length=0;
    for(const tr of trains){
      const off=tr.group.position.z;tr.group.updateMatrix();
      for(let i=0;i<tr.boxes.length;i++){
        const c=tr.boxes[i],base=tr.base[i];
        c.min.copy(base.min);c.max.copy(base.max);c.min.z+=off;c.max.z+=off;
        dynamicColliders.push(c);
      }
    }
    const closure=closureAt(tW,tE);
    for(const b of barriers){b.group.rotation.x=b.dir*closure*Math.PI/2;b.group.updateMatrix();if(closure>.15){const z=b.group.position.z,end=z+b.dir*6.5*Math.sin(closure*Math.PI/2);b.block.min.set(b.group.position.x-.14,.4,Math.min(z,end)-.12);b.block.max.set(b.group.position.x+.14,2,Math.max(z,end)+.12);dynamicColliders.push(b.block);}}
    const warning=closure>.02;signals.forEach((m,i)=>{m.emissiveIntensity=warning&&Math.floor(time*3)%2===i%2?3:.03;});
    const dwelling=isDwelling(tW,tE);
    /* 状态优先级：玩家占轨 > 道口在动作 > 有车停站 > 区间运行。
     * 'outbound' / 'inbound' / 'north-stop' 三个旧状态随折返一起取消 ——
     * 单向行车之后"驶出/进站"已经由道口窗口和停站窗口分别表达，
     * 而"北端折返等待"描述的那个动作本身就不存在了。 */
    root.userData.operatingState=occupied?'waiting-for-clearance'
      :closure>.02?(closure>closurePrev?'crossing-closing':closure<closurePrev?'crossing-opening':'crossing-running')
      :dwelling?'station-stop':'in-section';
    const live=root.userData.live;live.west=tW;live.east=tE;live.closure=closure;
    closurePrev=closure;
  }
  return {colliders,dynamicColliders,update,dispose(){scene.remove(root);root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);});for(const g of geometries)g.dispose();for(const m of materials)m.dispose();for(const t of textures)t.dispose();blossoms.material.dispose();blossoms.texture.dispose();}};
}
