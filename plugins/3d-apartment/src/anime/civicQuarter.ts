import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';
import { createBlossomField, createLeafField, createTrunkField, plantTree, type TreeKit } from './foliage';
import { createModernMall } from './modernMall';
import { groundTexture } from './groundTextures';
import { buildCityRoutes, SOUTH_ROUTES } from './cityRoutes';

/** South civic quarter. Coordinates deliberately avoid the existing homes, river and railway. */
export function createCivicQuarter(scene:THREE.Scene){
  const root=new THREE.Group();root.name='south-civic-quarter';root.userData.sceneCollideSkip=true;scene.add(root);
  const colliders:Collider[]=[],materials:THREE.Material[]=[],textures:THREE.Texture[]=[];
  const geometries=new Set<THREE.BufferGeometry>();
  /* `emissive` 允许传字符串（自发光色 ≠ 基色），传 `true` 时退回"自发光=基色"的老行为。
   * `extra` 用来挂贴图等 —— 必须放在展开的最后，否则会被默认值覆盖。 */
  const material=(color:string,emissive:boolean|string=false,extra:Partial<THREE.MeshStandardMaterialParameters>={})=>{const em=emissive===true?color:(emissive||null);const m=new THREE.MeshStandardMaterial({color,roughness:.72,...(em?{emissive:em,emissiveIntensity:.65}:{}),...extra});m.userData.outlineWeight=0;materials.push(m);return m;};
  /** 地面贴图登记进 `textures`，模块 dispose 时才释放得掉。 */
  const tex=(t:THREE.Texture)=>{textures.push(t);return t;};
  const M={stone:material('#c5c6bf'),ivory:material('#e1d9c7'),dark:material('#253947'),steel:material('#6a858d'),glass:material('#83b9c5'),wood:material('#82604e'),grass:material('#829773'),road:material('#586675'),white:material('#efe9dc'),pink:material('#eaa9c5'),yellow:material('#e5c776'),track:material('#ae7868'),cyan:material('#54d6dc',true),amber:material('#ffcd7c',true),rose:material('#f57ba9',true),
    // Garage openings and the existing exterior link retain their night lighting.
    litCore:material('#e8dcc2','#ffc987'),
    litCool:material('#bcd7de','#cfe6f2'),
  };
  for(const m of [M.litCore,M.litCool])m.emissiveIntensity=0;
  /* 地面材质**单独一套**，不复用 `M.stone` / `M.grass` —— 那两色同时用于门柱、
   * 围栏底座、雨花园草块等建筑构件，给它们贴砖缝/草丛是错的（立面砖缝会读作
   * "墙纸"）。地面贴图见 `groundTextures`：贴图只出明暗，颜色仍由这里的色相乘。
   * repeat 按各自地面的尺度给：草地 152×80 ⇒ 26（约 6m 一个 tile）；
   * 广场铺装 78×13 ⇒ 10；车行道 ⇒ 8。 */
  const G={
    grass:material('#829773',false,{map:tex(groundTexture('grass',26)),roughness:.95}),
    paving:material('#c5c6bf',false,{map:tex(groundTexture('paving',10)),roughness:.93}),
    road:material('#586675',false,{map:tex(groundTexture('asphalt',8)),roughness:.95}),
    soil:material('#514739',false,{map:tex(groundTexture('soil',6)),roughness:.96}),
  };
  const cube=new THREE.BoxGeometry(1,1,1),cylinder=new THREE.CylinderGeometry(1,1,1,12);geometries.add(cube);geometries.add(cylinder);
  function box(x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material){const o=new THREE.Mesh(cube,m);o.position.set(x,y,z);o.scale.set(w,h,d);o.castShadow=h>.2;o.receiveShadow=true;root.add(o);return o;}
  function block(name:string,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material,kind:Collider['kind']='wall'){box(x,y,z,w,h,d,m);colliders.push({source:`civic-${name}`,kind,min:new THREE.Vector3(x-w/2,y-h/2,z-d/2),max:new THREE.Vector3(x+w/2,y+h/2,z+d/2)});}
  function rod(a:number[],b:number[],r:number,m:THREE.Material){const from=new THREE.Vector3(...a),v=new THREE.Vector3(...b).sub(from);const o=new THREE.Mesh(cylinder,m);o.position.copy(from.addScaledVector(v,.5));o.scale.set(r,v.length(),r);o.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),v.normalize());o.castShadow=true;root.add(o);}
  function board(title:string,subtitle:string,x:number,y:number,z:number,w:number,h:number,style='navy',glow=false){
    const cv=document.createElement('canvas');cv.width=1024;cv.height=Math.max(256,Math.round(1024*h/w));const c=cv.getContext('2d')!,H=cv.height;
    const gradient=c.createLinearGradient(0,0,1024,H);gradient.addColorStop(0,style==='rose'?'#9d245e':style==='mint'?'#087a83':'#101f40');gradient.addColorStop(1,style==='rose'?'#ff967b':style==='mint'?'#77d6bc':'#3a4999');c.fillStyle=gradient;c.fillRect(0,0,1024,H);
    c.globalAlpha=.2;c.strokeStyle='#ffffff';c.lineWidth=3;for(let n=0;n<7;n++){c.beginPath();c.arc(900,H*.25,60+n*55,0,Math.PI*2);c.stroke();}c.globalAlpha=1;
    c.fillStyle='#faf3df';c.font=`700 ${Math.min(115,H*.28)}px "Microsoft YaHei",sans-serif`;c.fillText(title,55,H*.46,920);c.font=`${Math.min(42,H*.12)}px sans-serif`;c.fillText(subtitle,58,H*.72,905);c.fillStyle='#f3c16c';c.fillRect(58,H*.81,150,6);
    const texture=new THREE.CanvasTexture(cv);texture.colorSpace=THREE.SRGBColorSpace;textures.push(texture);
    const m=new THREE.MeshStandardMaterial({map:texture,roughness:.6,...(glow?{emissive:'#ffffff',emissiveMap:texture,emissiveIntensity:.75}:{})});m.userData.outlineWeight=0;materials.push(m);
    const geo=new THREE.PlaneGeometry(w,h);geometries.add(geo);const mesh=new THREE.Mesh(geo,m);mesh.position.set(x,y,z);mesh.rotation.y=Math.PI;root.add(mesh);return m;
  }
  // Continuous ground, extension of the two existing north-south streets and their sidewalks.
  block('district-ground',-4,-.04,123,152,.1,80,G.grass,'floor');
  for(const g of buildCityRoutes(root,colliders,SOUTH_ROUTES,{road:G.road,paving:G.paving,paint:M.white},.08))geometries.add(g);
  // LUMINA retains its x12..72 / z122..154 site and west parking garage.
  // The hollow atrium, shop galleries and gardens live in modernMall.ts.
  /* ---- 北广场：入口广场 + 落客区。x 从 12 西延到 -6，让停车楼北立面也front 到
   * 铺装上（否则它面对的是一片草地，停车楼会读成"站在荒地里"）。西界 -6 是贴着
   * 校区铺装（`campus-paving` 到 x=-7.5）留出来的，两者不相交。 ---- */
  block('mall-plaza',33,.055,116,78,.11,13,G.paving,'floor');
  /* 东侧广场与停车场地坪的**东界一律收到 x=102**，z 界互相错开，为的是给
   * `globalSquare.ts` 的世界广场（x 102…150, z 116…164，顶面同为 y=0.10）让出
   * 一块**只相切、不重叠**的场地。
   *
   * 为什么必须收：两块地坪如果同高又重叠，就是共面 —— 会整片 z-fighting 闪烁。
   * 为什么必须是 0.10 这个标高：`globalSquare` 那侧的顶面也是 0.10，同高才能在
   * x=102 处拼成一条无缝的接缝；差 1cm 在 150m 外就已经低于深度缓冲的分辨率
   * （near=0.1 / far≈400 的 24bit 深度，150m 处只能分辨约 1.3cm）。
   *
   * 分区（顶面都是 0.10，彼此只共边）：
   *   mall-plaza        x -6…72  z 109.5…122.5  （顶面 0.11，主广场，西延到停车楼前）
   *   mall-plaza-east   x 72…102 z 109.5…122.5
   *   mall-east-ground  x 68…102 z 122.5…163    （商场东侧的铺装前场）
   *   global-square     x 102…150 z 116…164     （见 globalSquare.ts） */
  block('mall-plaza-east',87,.05,116,30,.1,13,G.paving,'floor');
  /* 东侧前场。它横跨铁路走廊（x 74…106），但 `kind='floor'` ⇒ 不挡车、也不进
   * 「压轨清单」。顶面 0.10 夹在铁路草皮（0.005）与道砟顶面（0.12）之间：在轨道
   * 范围内盖住草皮（露出来的还是道砟 + 轨枕 + 钢轨），两侧就是一片铺装。
   * 留着它的唯一理由：不在 x 72…102 之间开一个 30×40 的洞（洞会直接看到天空）。 */
  block('mall-east-ground',85,.05,142.75,34,.1,40.5,G.paving,'floor');

  const mall=createModernMall(scene);
  colliders.push(...mall.colliders);

  /* ---- 自带停车楼（**西侧**，与西翼共一堵山墙）----
   *
   * **为什么不建在东侧**（上一版的位置 x 76…100，26×24，天桥接东翼）：
   * 那里整片落在**铁路走廊**里。轨道在 x=86 / x=91，轨枕每 0.72m 从 z=-440 一直
   * 铺到 +440，接触网门型架每 18m 一座、同样铺满 —— 所以那一版是把两条轨连枕木
   * 带接触网一起**包在楼里**：列车从北立面开进去、从南立面开出来，轨枕和门型架
   * 在楼体内部。`districtArt.TRACK_GAP = [74,106]` 就是为这条设的退界（远景内外
   * 两圈、`riverbank`、`cityDressing` 三处都在让），停车楼是唯一没让的那一个。
   * 判据：`_probe-park-site.mjs` 的「压轨清单」——实体盒 footprint 含 x=86 或
   * x=91 即为命中，改前 `civic-park-deck` 命中、改后应为 0。
   *
   * **为什么是这里**：核账量过（同一探针的「候选场地占用」）——
   *   x -6…12 / z 118…152  只与 1 条绿篱 + 3 棵树相交（都是 `cityDressing` 之后
   *                        才落的，装配顺序对了它们自然让开）
   *   x 104…150 / z 86…114 撞 101 个（球场、停车划线、行道树）
   *   x 12…72  / z 94…122  撞 157 个（`mall-boulevard` 43 + 步行道 76）
   *   x 12…72  / z 154…166 只有 11m 深，而且压着南边界墙
   * ⇒ 西侧是这一带**唯一**放得下一栋 19×30 楼的位置。
   *
   * 楼体东面贴在**西翼西山墙**（x=12）上：两者共墙，这是"自带停车场"最直接的
   * 读法 —— 商场 → 停车楼是一条动线，不需要天桥。共墙处两面都朝内、互相遮挡，
   * 所以不存在共面闪烁（西翼西山墙那排窗在 x=11.89…11.99，整个陷在楼体内部，
   * 不会被渲染出来）。 */
  const PARK_H=17.4,PARK_W=19,PARK_D=30,PARK_XC=2.5,PARK_ZC=137;
  block('park-deck',PARK_XC,PARK_H/2,PARK_ZC,PARK_W,PARK_H,PARK_D,M.dark);
  for(let f=0;f<6;f++){
    const y=1.7+f*2.9;
    box(PARK_XC,y,PARK_ZC-PARK_D/2-.1,PARK_W-1.6,1.5,.1,M.litCool);  // 北开口（朝广场）
    box(PARK_XC,y,PARK_ZC+PARK_D/2+.1,PARK_W-1.6,1.5,.1,M.litCool);  // 南开口
    box(PARK_XC-PARK_W/2-.1,y,PARK_ZC,.1,1.5,PARK_D-2,M.litCool);    // 西开口（朝校区）
    /* 东开口不画：那一面就是西翼的西山墙，做了也是埋在两个盒子中间，看不见。 */
    box(PARK_XC,y+1.15,PARK_ZC-PARK_D/2-.15,PARK_W-1.4,.14,.08,M.amber);  // 层间灯带
  }
  box(PARK_XC,PARK_H+.35,PARK_ZC,PARK_W+.8,.7,PARK_D+.8,M.ivory);
  board('P  PARKING','LUMINA  /  24H  /  1200 CARS',PARK_XC,15.0,PARK_ZC-PARK_D/2-.6,13,3.2,'navy',true);
  /* 外挂坡道（南侧直坡）—— 只做形体，不做可行驶面。放南面而不是北面：
   * 北面正对入口广场，一条钢坡道压在落客区上会把主入口的读法打乱。 */
  const RAMP_Z=PARK_ZC+PARK_D/2+1.9;
  const ramp=box(PARK_XC-4,PARK_H/2-1,RAMP_Z,10,.4,2.6,M.steel);ramp.rotation.z=-Math.atan2(PARK_H-2,10);
  for(const dx of [-5,5]){const rail=box(PARK_XC-4+dx,PARK_H/2-.3,RAMP_Z,.14,.14,2.6,M.steel);rail.rotation.z=-Math.atan2(PARK_H-2,10);}
  /* 2F 连廊：西翼 ↔ 停车楼。两栋共墙 ⇒ 连廊必须从**北立面挑出来**才看得见；
   * 贴着墙面做是埋在两个盒子里，什么都看不到。跨过 x=12 那道缝，夜里亮成一条。
   * y 取 5.35…6.75：正好落在停车楼第 2、3 层开口之间那道**实墙带**上（开口在
   * y = 1.7 / 4.6 / 7.5 …，各高 1.5），既压不住开口，上沿又对上西翼 2F 楼板 6.7。 */
  box(6,6.05,121.3,14,1.4,1.8,M.litCore);
  /* 地面停车位：停车楼北侧一排划线，压在广场铺装顶面之上（铺装顶面 0.11）。 */
  for(let i=0;i<4;i++)box(-3+i*3.6,.118,115,3.2,.02,5,M.white);
  // Red amphitheatre steps and café tables, kept out of the straight approach to the lobby.
  for(let i=0;i<5;i++)block('plaza-seat',20+i*.48,.17+i*.16,115, .48,.34+i*.32,5.4,M.stone);
  for(const x of [55,59])for(const z of [113,117]){rod([x,.05,z],[x,.78,z],.055,M.steel);box(x,.8,z,1,.08,1,M.wood);for(const dx of [-.8,.8]){block('cafe-chair',x+dx,.42,z,.42,.84,.45,M.wood);}rod([x,.8,z],[x,2.8,z],.025,M.steel);box(x,2.8,z,2.4,.07,2.4,M.ivory);}
  // High school: three-floor classroom bar with side gym, enclosed grounds and an open gate.
  block('campus-paving',-35,.03,132,55,.06,43,G.paving,'floor');
  block('classroom-building',-34,5.1,124,43,10.2,9,M.ivory);
  for(let f=0;f<3;f++){
    const y=1.85+f*3.2;box(-34,y-1.55,119.35,43.4,.16,.38,M.stone);
    for(let col=0;col<15;col++){const x=-53.6+col*2.8;box(x,y,119.42,2.3,1.95,.08,M.steel);box(x,y,119.36,2.12,1.78,.035,M.glass);box(x,y,119.31,.04,1.8,.035,M.ivory);box(x,y-.96,119.25,2.45,.08,.28,M.stone);}
  }
  box(-34,10.4,124,44,.3,9.8,M.stone);for(let x=-54;x<-12;x+=2)box(x,10.85,119.3,.045,.7,.045,M.steel);box(-34,11.2,119.3,43,.05,.05,M.steel);
  for(let f=0;f<3;f++){
    for(let col=0;col<15;col++){const x=-53.6+col*2.8,y=1.85+f*3.2;box(x,y,128.55,2.3,1.95,.06,M.steel);box(x,y,128.6,2.12,1.78,.03,M.glass);box(x,y,128.63,.04,1.8,.025,M.ivory);}
    for(const x of [-55.55,-12.45])for(const z of [121.3,124,126.7])box(x,1.85+f*3.2,z,.05,1.8,1.55,M.glass);
  }
  box(-34,1.4,119.2,4,2.8,.14,M.dark);for(const x of [-35,-33])box(x,1.35,119.1,1.75,2.5,.06,M.glass);
  board('桜ヶ丘高等学校','SAKURAGAOKA HIGH SCHOOL',-34,9,119.15,13,1.2,'navy');
  for(const x of [-42,-26]){block('school-gatepost',x,1.15,112, .65,2.3,.65,M.stone);}
  board('桜ヶ丘高校','正門  /  MAIN GATE',-41.98,1.45,111.65,.55,1.25);
  function fence(a:number,b:number,fixed:number,alongX:boolean){
    const x=alongX?(a+b)/2:fixed,z=alongX?fixed:(a+b)/2;
    // Collision follows the open rail silhouette without rendering a solid metal wall.
    colliders.push({source:'civic-campus-fence',kind:'wall',min:new THREE.Vector3(alongX?a:fixed-.06,0,alongX?fixed-.06:a),max:new THREE.Vector3(alongX?b:fixed+.06,1.45,alongX?fixed+.06:b)});
    box(x,.13,z,alongX?b-a:.16,.26,alongX?.16:b-a,M.stone);
    for(const y of [.4,1.4])box(x,y,z,alongX?b-a:.045,.045,alongX?.045:b-a,M.steel);
    for(let p=a;p<b;p+=.25)box(alongX?p:fixed,.87,alongX?fixed:p,.024,1.1,.024,M.steel);
  }
  for(const [a,b] of [[-62,-42],[-26,-8]])fence(a,b,112,true);
  for(const x of [-62,-8])fence(112,153,x,false);
  fence(-62,-8,153,true);
  block('gymnasium',-15,3.4,140,9,6.8,17,M.ivory);
  for(const side of [-1,1]){const panel=box(-15+side*2.3,7.2,140,4.8,.18,17.8,M.steel);panel.rotation.z=-side*.18;}
  for(let z=134;z<148;z+=2.5)box(-19.56,5.1,z,.06,1.25,1.8,M.glass);
  board('体育館','GYMNASIUM',-15,4.3,131.43,5,.8);
  box(-15,1.3,131.42,3.4,2.6,.08,M.dark);
  // Compact multipurpose sports court with line work, goals and basketball hoops.
  block('sports-court',-36,.075,141,30,.05,17,M.track,'floor');
  for(const x of [-50.5,-21.5])box(x,.108,141,.07,.01,16,M.white);
  for(const z of [133,149])box(-36,.108,z,29,.01,.07,M.white);box(-36,.109,141,.07,.01,16,M.white);
  const circle=new THREE.TorusGeometry(2,.035,4,48);geometries.add(circle);const centre=new THREE.Mesh(circle,M.white);centre.rotation.x=Math.PI/2;centre.position.set(-36,.12,141);root.add(centre);
  for(const x of [-50,-22]){rod([x,0,141],[x,3.1,141],.07,M.steel);box(x,3,141,.1,1,1.7,M.white);for(const z of [139,143])rod([x,.1,z],[x,1.9,z],.04,M.white);rod([x,1.9,139],[x,1.9,143],.04,M.white);}
  const hoopGeometry=new THREE.TorusGeometry(.24,.022,5,20);geometries.add(hoopGeometry);
  for(const x of [-49.65,-22.35]){const hoop=new THREE.Mesh(hoopGeometry,M.rose);hoop.rotation.x=Math.PI/2;hoop.position.set(x,2.7,141);root.add(hoop);for(let n=0;n<8;n++){const a=n*Math.PI/4;rod([x+Math.cos(a)*.24,2.7,141+Math.sin(a)*.24],[x+Math.cos(a)*.14,2.35,141+Math.sin(a)*.14],.008,M.white);}}
  // Bicycle shelter with racks and simple spoke wheels.
  box(-56,2.35,139,5,.13,12,M.glass);for(const z of [134,144])for(const x of [-58,-54])rod([x,0,z],[x,2.35,z],.045,M.steel);
  const wheel=new THREE.TorusGeometry(.3,.024,5,18);geometries.add(wheel);
  for(let n=0;n<7;n++){const z=134+n*1.5;for(const x of [-56.65,-55.45]){const o=new THREE.Mesh(wheel,M.dark);o.rotation.y=Math.PI/2;o.position.set(x,.38,z);root.add(o);}rod([-56.65,.38,z],[-56,1,z],.025,M.steel);rod([-56,1,z],[-55.45,.38,z],.025,M.steel);rod([-56.65,.38,z],[-55.45,.38,z],.025,M.steel);box(-56.1,1.05,z,.35,.08,.18,M.wood);}
  // Small inhabited edges along the greenway connect the destinations into one civic landscape.
  for(const [x,z] of [[9,88],[-12,91],[5,110]]){
    block('greenway-seat',x,.42,z,2.2,.84,.55,M.wood);
    block('rain-garden',x+2.5,.16,z,2.2,.32,1.4,M.stone);
    for(let n=0;n<6;n++)box(x+1.65+n*.32,.49,z,.23,.38,.6,G.grass);
  }
  board('CIVIC WALK','SCHOOL ←  ·  PARK ↑  ·  LUMINA →',5,1.7,110,2,.65,'mint');
  // Shared avenue furniture and mature cherry trees soften both large developments.
  let seed=9821;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  /* 冠簇容量按 `plantTree` 的实际消耗估：每棵 7×(112+34) ≈ 1022 簇，13 棵 ≈ 13300。
   * 上一版每棵只挂 5×35 = 175 簇（3000 就够），换实现后不同步扩容会**静默截断** ——
   * `spray()` 满了直接 return，树冠从内圈开始秃，而且没有任何报错。 */
  const blossoms=createBlossomField(random,16000);
  /* 整棵树（干 + 分叉侧枝 + 两级冠簇）走 `foliage.plantTree`，和街区行道树、公园
   * 老樱、河堤行道樱同一套实现。
   *
   * 上一版只把**主干**换成了 Blender 资产，侧枝仍是 5 根 `rod` 圆柱、冠簇只在枝头
   * 挂一层（内圈是空的，从下面看是个"甜甜圈"）—— 换干不换枝就是半套。这一版连
   * 侧枝一起换掉：7 组分叉、枝头 + 枝条中段两层冠簇。
   * 树穴（含碰撞盒）改由 `pit` 画，位置与尺寸一个没动。 */
  const trunks=createTrunkField();
  /* 叶簇场：樱树那批用不上，但下面广场树阵（绿树）和花坛植栽要用。
   * 容量按「3 棵绿树 × 925 簇 + 两列花坛约 500 簇」估 —— 小了会被 `spray()`
   * 静默截断（满了直接 return），表现是冠从内圈开始秃。 */
  const civicLeaf=createLeafField(random,8000);
  const treeKit:TreeKit={
    blossom:blossoms,leaf:civicLeaf,
    random,trunk:trunks,
    rod(a,b,r){rod(a,b,r,M.wood);},
    pit(x,z){block('tree-bed',x,.18,z,1.7,.36,1.7,M.wood);},
  };
  [[-64,109.8],[-53,109.8],[-20,109.8],[-9,109.8],[8,115],[16,115],[64,115],[9,75],[-3,82],[5,90],[-8,95],[8,104],[20,108]].forEach(([x,z],i)=>plantTree(treeKit,x,z,true,i));
  /* x=8 那一对（路灯 + 长椅）原来压在 `civic-shared-street` 的**车行道**里：
   * 那条协同街在 x 3~15 之间向南拐到 z≈98，路半宽 2.25 ⇒ 沥青面覆盖 z 96.2~100.7，
   * 而路灯在 z=100、长椅在 z=99 —— 长椅几乎骑在中线上。
   * 静态布景看不出来，动态车流一上去就撞（`tmp/_probe-dressing.mjs` 的车流②）。
   * 东移到 x=30：那里是 `mall-boulevard` 北侧人行道，沥青面到 z≈98.4、铺装到 100.2，
   * 两件家具落在铺装上、离车行道 1.5m 以上。 */
  for(const x of [-65,-18,30,65]){block('streetlamp',x,2.5,100,.1,5,.1,M.steel);box(x,5,100,1,.1,.6,M.amber);block('public-bench',x+2,.45,99,2,.9,.6,M.wood);}
  board('LUMINA →','SHOPPING & CINEMA',43,2.3,98,3,.8);board('← 桜ヶ丘高校','SCHOOL / CAMPUS',-46,2.3,98,3,.8);

  /* ---- 入口广场的空场补完 ----
   *
   * `mall-plaza` 是 x -6..72 / z 109.5..122.5 的 78×13 铺装。此前广场上只有
   * 两处水景（x28 / x48）、四个花池（x24/33/51/69 @ z119）和门口三棵树，
   * **西半边（x -6..14）整片是空铺装** —— 玩家从西侧走过来，先看到 20m 空地
   * 才到商场门口。这里补三样：一座廊架（有顶、有座，是"能待一会儿"的读法）、
   * 两列花坛、东段一排树阵。
   *
   * 落位全部避开既有实体：水景 x25.3..30.7 / x45.3..50.7 @ z112.15..115.25；
   * 花池 x22.85..25.15 / 31.85..34.15 / 49.85..52.15 / 67.85..70.15 @ z117.95..120.05；
   * 树穴 x7.15..8.85 / 15.15..16.85 / 63.15..64.85 @ z114.15..115.85；
   * 门前台阶 x33..51 @ z117..122（入口前庭，一律不碰）。 */
  {
    // 廊架：木柱 + 双梁 + 格栅顶 + 底下的长椅。8.4×3.6 的体量刚好把西半边的
    // 空旷压住，又不挡住从西侧看商场主入口的视线（它在 x -2.7..5.7，门口在 x42）。
    const PX = 1.5, PZ = 114.2, PW = 8.4, PD = 3.6, PH = 2.85;
    for(const dx of [-1,1])for(const dz of [-1,1]){
      block('plaza-pergola',PX+dx*PW/2,PH/2,PZ+dz*PD/2,.18,PH,.18,M.wood);
      box(PX+dx*PW/2,PH+.02,PZ+dz*PD/2,.30,.10,.30,M.stone);   // 柱础
    }
    for(const dz of [-1,1])box(PX,PH+.14,PZ+dz*PD/2,PW+.5,.16,.26,M.wood);   // 前后梁
    for(let x=PX-PW/2;x<=PX+PW/2;x+=.62)box(x,PH+.30,PZ,PW/9,.06,PD+.1,M.wood); // 格栅顶
    for(const dx of [-1,1])box(PX+dx*(PW/2-.5),.46,PZ,2.6,.10,.5,M.wood);      // 两条长椅
    for(const dx of [-1,1])for(const bx of [dx*(PW/2-.5)-1.1,dx*(PW/2-.5)+1.1])
      box(bx,.22,PZ,.12,.44,.44,M.steel);
  }
  /* 花坛：西段一列、东段一列。`kind='wall'`（0.48m 高确实挡人），
   * 坛底压回铺装面，不做成浮在地面上的盒子。 */
  for(const [fx,fz,fw] of [[12.4,111.8,6.4],[59,110.4,5.2]] as Array<[number,number,number]>){
    block('plaza-flowerbed',fx,.24,fz,fw,.48,1.3,M.stone);
    box(fx,.49,fz,fw-.24,.04,1.06,G.soil);
    // 植栽走叶簇场（和树的冠同一张贴图），沿花坛长向密排
    const n=Math.round(fw*9);
    for(let i=0;i<n;i++){
      const t=(i+.5)/n;
      civicLeaf.spray(fx-fw/2+.16+t*(fw-.32),.58+random()*.30,fz+(random()-.5)*.86,.15+random()*.13);
    }
  }
  /* 东段树阵：广场东半边 x54..72 此前只有一块水景和两个花池，从东侧看过去
   * 是一排铺装＋远处的西翼山墙。三棵绿树把纵深感补上（绿树，别和广场已有的
   * 三棵樱撞色 —— 樱花集中在 x8/16/64 @ z115）。 */
  [[56,112],[62,112],[68,112]].forEach(([x,z],i)=>plantTree(treeKit,x,z,false,100+i,.8));

  /* ---- 校区前庭（校门内 x -60..-10 / z 113..119）----
   *
   * 这块 50×6 的铺装在**校门和教学楼之间**：门在 z112、教学楼北墙在 z119.35，
   * 中间整条是空的 —— 玩家从南侧走过来先看到校门，推门进去是一片 50m 宽的
   * 空铺装，教学楼像被推到远处。补一排树阵 + 两条花坛，把"进校门"这个动作
   * 交代出来。（不补得更多：预留出来的是集会/疏散用的场地，铺满反而不像学校。） */
  [[-46,116.5],[-38,116.5],[-30,116.5],[-22,116.5],[-14,116.5]].forEach(([x,z],i)=>plantTree(treeKit,x,z,false,200+i,.7));
  for(const [fx,fw] of [[-42,7.0],[-26,7.0]] as Array<[number,number]>){
    block('campus-flowerbed',fx,.22,114.2,fw,.44,1.2,M.stone);
    box(fx,.45,114.2,fw-.22,.04,.98,G.soil);
    const n=Math.round(fw*8);
    for(let i=0;i<n;i++){
      const t=(i+.5)/n;
      civicLeaf.spray(fx-fw/2+.16+t*(fw-.32),.54+random()*.28,114.2+(random()-.5)*.78,.14+random()*.12);
    }
  }

  mergeByMaterial(root);root.add(blossoms.build('civic-sakura-canopy'));
  /* 干场和冠簇一样，必须等 mergeByMaterial 之后再挂：merge 只按 isMesh 过滤，
   * InstancedMesh 也是 isMesh，混进去会被当成一块几何烘掉。 */
  const trunkGroup=trunks.build('civic-sakura-trunks',M.wood.color);root.add(trunkGroup);
  // 干场材质是 build 内部新建的（toon + 树皮贴图），没进 materials，得登记后才能随模块释放。
  trunkGroup.traverse(o=>{const m=o as THREE.InstancedMesh;if(m.isInstancedMesh)materials.push(m.material as THREE.Material);});
  // 广场树阵的冠与花坛植栽共用这一场，同样等合批之后再挂。
  root.add(civicLeaf.build('civic-street-leaves'));
  root.userData.destinations={mall:[40,115],school:[-34,115]};
  // Keep the garage and the new galleria on the same environment clock.
  function applyPeriod(period:string){
    const night=period==='night',dusk=period==='dusk';
    M.litCore.emissiveIntensity=night?1.4:dusk?.7:.06;
    M.litCool.emissiveIntensity=night?1:dusk?.45:.03;
    mall.setEnvironment(period);
  }
  applyPeriod('day');
  return {colliders,setEnvironment(period:string){applyPeriod(period);},update(_t:number){},dispose(){mall.dispose();root.removeFromParent();root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);if(o instanceof THREE.InstancedMesh)o.dispose();});for(const g of geometries)g.dispose();for(const m of materials)m.dispose();for(const t of textures)t.dispose();blossoms.material.dispose();blossoms.texture.dispose();civicLeaf.material.dispose();civicLeaf.texture.dispose();}};
}
