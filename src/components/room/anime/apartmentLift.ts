import * as THREE from 'three';
import { buildBoxColliders, type BoxColliderSpec, type Collider } from './collider';
import type { FPSControls } from './fpsControls';

export const LIFT_LEVELS = [0, 3.4, 6.2, 9.0] as const;
export const LIFT_CABIN = { x: -34.6, z: -2.4 };
/** 轿厢门扇所在的 z（门洞平面）：门外侧是 z 大于它的那一侧 */
const DOOR_Z = -0.92;
export const liftTravelMs = (from: number, to: number) => 1200 + Math.abs(to-from)*1500;

/** Four matching landings with a timed, camera-teleport lift. World vertical axis is Y. */
export function createApartmentLift(container: HTMLElement, camera: THREE.PerspectiveCamera, fps: FPSControls) {
  const group=new THREE.Group();group.name='apartment-lift-tower';group.userData.sceneCollideSkip=true;
  const dynamic=new THREE.Group();dynamic.name='lift-moving-doors';dynamic.userData.sceneCollideSkip=true;
  const specs:BoxColliderSpec[]=[], floors:Collider[]=[];

  /* ============================ 材质 ============================
   * 旧版五种材质全是**无贴图的纯色** MeshStandardMaterial：一个深棕盒子、一块
   * 灰米色板，白天看过去整个轿厢就是一片平涂。这里补三张程序化贴图
   * （石材斑驳 / 橡木直纹 / 青铜细颗粒），并把色板收拢成
   * 「暖木 + 冷石 + 青铜」三档，和 podium 的材质语言对齐。
   *
   * 用 canvas 而不是外部贴图：这个文件本来就不引资源，四层轿厢共用同一批
   * 纹理，程序化生成既省体积也能保证四层完全一致。
   */
  let seed=19770921;
  const rnd=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  function canvasTex(w:number,h:number,paint:(g:CanvasRenderingContext2D)=>void){
    const c=document.createElement('canvas');c.width=w;c.height=h;paint(c.getContext('2d')!);
    const t=new THREE.CanvasTexture(c);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;return t;
  }
  /** 细粒磨光石灰石：暖白底 + 密而淡的深色颗粒 + 几条水平锯痕。 */
  const stoneMap=canvasTex(256,256,g=>{
    g.fillStyle='#ddd6c8';g.fillRect(0,0,256,256);
    for(let i=0;i<5200;i++){g.fillStyle=`rgba(96,86,68,${(rnd()*.10).toFixed(3)})`;g.fillRect(rnd()*256,rnd()*256,1+rnd()*2,.5+rnd());}
    for(let y=0;y<256;y+=11){g.fillStyle=`rgba(150,132,104,${(.02+rnd()*.03).toFixed(3)})`;g.fillRect(0,y,256,.6);}
  });
  /** 烟熏橡木直纹：长纹 + 短打断纹。只有长纹会退化成等距竖条，必须混着画。
   *  对比度刻意压低 —— 第一版把年轮线画到 alpha .20，贴到 0.9m 宽的板子上就是
   *  一片橙红硬条纹，读起来像廉价木纹纸。 */
  const oakMap=canvasTex(192,512,g=>{
    g.fillStyle='#8a7255';g.fillRect(0,0,192,512);
    for(let i=0;i<150;i++){
      g.fillStyle=`rgba(${rnd()<.5?'62,46,30':'152,132,106'},${(.03+rnd()*.09).toFixed(3)})`;
      g.fillRect(rnd()*192,0,.8+rnd()*2.6,512);
    }
    for(let i=0;i<520;i++){
      g.fillStyle=`rgba(${rnd()<.5?'58,42,26':'162,144,118'},${(.025+rnd()*.06).toFixed(3)})`;
      g.fillRect(rnd()*192,rnd()*512,1+rnd()*1.6,5+rnd()*48);
    }
  });
  /** 青铜：细颗粒噪点。**刻意不做方向性拉丝** —— 拉丝贴图贴到 7.5m 长的
   *  灯槽或 12m 长的顶板上会被拉成一道道横条，比纯色更难看。 */
  const bronzeMap=canvasTex(128,128,g=>{
    g.fillStyle='#8b7a5e';g.fillRect(0,0,128,128);
    for(let i=0;i<2600;i++){
      g.fillStyle=`rgba(${rnd()<.5?'54,44,30':'200,180,146'},${(.03+rnd()*.10).toFixed(3)})`;
      g.fillRect(rnd()*128,rnd()*128,1,1);
    }
  });
  /** 轿厢地面：深色磨石 + 一圈青铜镶边。镶边**画进贴图**，不另外摆四条细线
   *  —— 共面的细长条在 24bit 深度缓冲下必 z-fighting。 */
  const floorMap=canvasTex(512,512,g=>{
    g.fillStyle='#4a443a';g.fillRect(0,0,512,512);
    for(let i=0;i<9000;i++){g.fillStyle=`rgba(${rnd()<.5?'30,26,21':'136,126,108'},${(.05+rnd()*.16).toFixed(3)})`;g.fillRect(rnd()*512,rnd()*512,1+rnd()*2.4,1+rnd()*2.4);}
    g.strokeStyle='#a8875a';g.lineWidth=9;g.strokeRect(14,14,484,484);
    g.strokeStyle='rgba(24,21,17,.55)';g.lineWidth=3;g.strokeRect(26,26,460,460);
  });
  /** 门楣名牌：轿厢侧唯一的文字，只做一张、四层共用。
   *
   *  文字是「WELCOME」。原来写的是「VIVIAN RESIDENCES」——16 个字母铺满画布 66% 宽，
   *  换成 7 个字母后同样 22px 只占 27%，在 1.20m 宽的名牌中间缩成一小撮。
   *  所以字号抬到 30px（画布只有 48px 高，再大就顶到边），字距仍是「每个字母间一个空格」
   *  的 tracked caps 风格，与原来的排版一致。 */
  const nameMap=canvasTex(512,48,g=>{
    g.fillStyle='#4a4133';g.fillRect(0,0,512,48);
    g.fillStyle='#d8c49a';g.font='600 30px sans-serif';g.textAlign='center';g.textBaseline='middle';
    g.fillText('W E L C O M E',256,24);
  });

  const stone=new THREE.MeshStandardMaterial({color:'#ffffff',roughness:.86,map:stoneMap,bumpMap:stoneMap,bumpScale:.003});
  const wood=new THREE.MeshStandardMaterial({color:'#ffffff',roughness:.72,map:oakMap,bumpMap:oakMap,bumpScale:.002});
  /* 青铜。**metalness 必须压在 0.45 附近**：这个场景的 IBL（PMREM 家具环境）
   * 本身很暗，metalness 一上 0.6 金属面就只剩镜面项、直接黑掉 —— 第一版门扇
   * 在正午下渲染成一块近黑的深棕，就是这条。 */
  const metal=new THREE.MeshStandardMaterial({color:'#b09a72',roughness:.44,metalness:.42,map:bronzeMap});
  /** 门扇单独一档：轿厢里最大的一块面，要比线脚再亮一点，否则整面门读成黑洞。 */
  const doorMat=new THREE.MeshStandardMaterial({color:'#c6b592',roughness:.38,metalness:.32,map:bronzeMap});
  /** 深色收口（踢脚 / 扶手凹带 / 门扇踢脚板 / 入口垫）。
   *  色相偏暖 —— 第一版用的 #4b4f48 是冷绿灰，压在暖木墙上像一道发霉的缝。 */
  const dark=new THREE.MeshStandardMaterial({color:'#4a443a',roughness:.68});
  const light=new THREE.MeshStandardMaterial({color:'#f6ddaf',emissive:'#ffdc9c',emissiveIntensity:.95});
  /** 后壁整高缎面背漆玻璃。
   *
   *  **不要再把它做成高光泽镜面。** 第一版是 roughness .10 + metalness .72 +
   *  envMapIntensity 2.4 的「烟熏镜」。问题不在于好不好看，而在于这个场景**没有
   *  真反射** —— 既没有 Reflector 也没有 CubeCamera，高光面只能靠 IBL 的镜面项
   *  假装。于是轿厢里任何一盏点光都会在它上面烧出一个正圆亮斑，从轿厢里看就是
   *  「后壁正中嵌了一盏灯」（用户反馈的突兀光源就是这个）。
   *  实测把光源挪到吊顶两侧也治不了：斜视时两侧灯的高光一样落到这块板上，因为
   *  平面镜的反射点由相机与光源的连线决定，躲不开。
   *  根治办法只有把高光瓣摊平：roughness 抬到 .38、metalness 收到 .30、
   *  envMapIntensity 只留 1.15。现在它仍比两侧木饰面亮、仍是块有分界的独立板材，
   *  但再也形不成能读成灯具的亮点。 */
  const satinGlass=new THREE.MeshStandardMaterial({color:'#d9d1bb',roughness:.38,metalness:.30,envMapIntensity:1.15});
  const floorStone=new THREE.MeshStandardMaterial({color:'#ffffff',roughness:.42,map:floorMap});
  const nameplate=new THREE.MeshStandardMaterial({color:'#ffffff',roughness:.44,metalness:.35,map:nameMap});
  const mats=[stone,wood,metal,doorMat,dark,light,satinGlass,floorStone,nameplate];mats.forEach(m=>m.userData.outlineWeight=0);

  /* 按钮盘面：每个「层号 × 亮/暗」一张小图，**按需缓存**。
   *  旧版是每层每键现造一张贴图（4 层 × 4 键 = 16 份材质 + 16 个透明面片，
   *  透明材质不参与合批 ⇒ 16 次 draw call 画一堆看不见的小数字）。 */
  const discMats=new Map<string,THREE.MeshStandardMaterial>();
  function discMat(key:string,paint:(g:CanvasRenderingContext2D)=>void,lit=false){
    const hit=discMats.get(key);if(hit)return hit;
    const c=document.createElement('canvas');c.width=c.height=64;paint(c.getContext('2d')!);
    const t=new THREE.CanvasTexture(c);t.colorSpace=THREE.SRGBColorSpace;
    const m=new THREE.MeshStandardMaterial({map:t,roughness:.5,emissive:lit?'#ffd9a0':'#000000',emissiveMap:lit?t:null,emissiveIntensity:.7});
    m.userData.outlineWeight=0;discMats.set(key,m);return m;
  }
  function numberMat(n:number,lit:boolean){
    return discMat(`n${n}${lit?'L':'D'}`,g=>{
      g.fillStyle=lit?'#f4e0b6':'#3d423d';g.fillRect(0,0,64,64);
      g.fillStyle=lit?'#3a423b':'#d9c9a6';
      g.font='bold 42px sans-serif';g.textAlign='center';g.textBaseline='middle';g.fillText(String(n+1),32,35);
    },lit);
  }
  function glyphMat(kind:'door'|'bell'|'up'|'down'){
    return discMat(kind,g=>{
      g.fillStyle='#3d423d';g.fillRect(0,0,64,64);g.fillStyle='#d9c9a6';
      if(kind==='door'){
        g.beginPath();g.moveTo(27,16);g.lineTo(27,48);g.lineTo(11,32);g.closePath();g.fill();
        g.beginPath();g.moveTo(37,16);g.lineTo(37,48);g.lineTo(53,32);g.closePath();g.fill();
      }else if(kind==='bell'){
        g.beginPath();g.arc(32,28,17,Math.PI,0);g.closePath();g.fill();
        g.fillRect(13,28,38,5);g.fillRect(26,44,12,5);
      }else if(kind==='up'){
        g.beginPath();g.moveTo(32,12);g.lineTo(50,42);g.lineTo(14,42);g.closePath();g.fill();
      }else{
        g.beginPath();g.moveTo(32,52);g.lineTo(14,22);g.lineTo(50,22);g.closePath();g.fill();
      }
    });
  }

  const geometries=new Map<string,THREE.BoxGeometry>();
  function box(name:string,x:number,y:number,z:number,w:number,h:number,d:number,mat:THREE.Material,solid=false,parent:THREE.Object3D=group){
    const key=[w,h,d].join('/');let geo=geometries.get(key);if(!geo){geo=new THREE.BoxGeometry(w,h,d);geometries.set(key,geo);}
    const mesh=new THREE.Mesh(geo,mat);mesh.name='lift-'+name;mesh.position.set(x,y,z);mesh.castShadow=parent===group;mesh.receiveShadow=true;mesh.userData.noOutline=true;parent.add(mesh);
    if(solid)specs.push({id:mesh.name,pos:[x,y,z],size:[w,h,d]});return mesh;
  }
  /** 圆片（层号 / 呼叫箭头 / 功能键）：CylinderGeometry 默认轴就是 Y，平放不用转。
   *  faceRotY 决定盘面朝向：轿厢内的操纵盘朝 −z（π），电梯厅的呼叫盘朝 +z（0）。 */
  function disc(name:string,x:number,y:number,z:number,r:number,h:number,mat:THREE.Material,faceZ?:number,faceMat?:THREE.Material,faceRotY=Math.PI){
    const body=new THREE.Mesh(new THREE.CylinderGeometry(r,r,h,20),mat);
    body.name='lift-'+name;body.rotation.x=Math.PI/2;body.position.set(x,y,z);
    body.castShadow=false;body.receiveShadow=true;body.userData.noOutline=true;group.add(body);
    if(faceMat!==undefined){
      const face=new THREE.Mesh(new THREE.CircleGeometry(r*.94,20),faceMat);
      face.name='lift-'+name+'-face';face.position.set(x,y,faceZ!);face.rotation.y=faceRotY;
      face.userData.noOutline=true;group.add(face);
    }
  }
  box('west-wall',-36.74,6.08,-1.25,.12,12.16,11.90,stone,true);
  box('north-wall',-33.9,6.08,-7.20,5.56,12.16,.12,stone,true);
  box('south-wall',-33.9,6.08,4.73,5.56,12.16,.12,stone,true);
  box('roof',-33.9,12.25,-1.25,5.94,.18,12.18,metal);
  // Close the construction joint without covering either existing wall face.
  box('south-joint',-31.06,6.075,4.73,.20,12.11,.10,stone,true);
  box('north-joint',-31.06,6.075,-7.20,.20,12.11,.10,stone,true);
  // A flush portal joins the taller cafe ceiling to the lower lift-hall ceiling.
  box('lobby-portal-head',-31.045,2.99,-.60,.29,.50,10.60,stone,true);
  box('lobby-portal-north',-31.045,1.39,-5.8175,.29,2.70,.235,stone,true);
  box('lobby-portal-south',-31.045,1.39,4.59,.29,2.70,.20,stone,true);
  box('lobby-floor-joint',-31.01,.032,-.55,.02,.06,10.54,stone);
  // Ground floor only: fixed glazing prevents entering from the rear pedestrian path.
  // Upper floors retain their open connection to the north access galleries.
  const glazing=new THREE.MeshStandardMaterial({color:'#9ebeb7',roughness:.18,metalness:.12,transparent:true,opacity:.27,depthWrite:false});
  glazing.userData.outlineWeight=0;
  for(const z of [-6.8325,-6.2475])box('ground-rear-glass',-31.025,1.38,z,.018,2.60,.545,glazing,true).castShadow=false;
  for(const z of [-7.125,-6.54,-5.955])box('ground-rear-mullion',-31.025,1.38,z,.072,2.60,.04,metal,true);
  for(const y of [.062,2.698])box('ground-rear-transom',-31.025,y,-6.54,.072,.036,1.21,metal,true);
  box('ground-rear-spandrel',-31.025,3.015,-6.54,.09,.58,1.21,stone,true);
  const leaves:Array<[THREE.Mesh,THREE.Mesh]>=[];
  /* ======================= 四层轿厢内饰 =======================
   * 轿厢内净空（世界坐标，四层完全一致）：
   *   x −36.10 … −33.10   z −3.90 … −0.96
   *   地面完成面 F+0.062   吊顶底 F+2.685
   * 所有饰面都从这三个「墙完成面」往外长，**凸出量封顶 0.036**：
   * 玩家碰撞半径 0.15、相机 near = 0.1 ⇒ 贴墙站定时相机离墙正好 0.15。
   * 饰面一旦凸出超过 0.05 就会被 near 面切开，近处直接出现一个洞。
   * 扶手 / 操纵盘 / 门套都按这条线卡过尺寸，改的时候别随手加厚。
   */
  for(let floor=0;floor<4;floor++){
    const F=LIFT_LEVELS[floor],top=F+.062;
    box('landing-floor',-33.9,F+.032,-1.25,5.76,.06,11.86,stone);
    floors.push({min:new THREE.Vector3(-36.8,F-.02,-7.18),max:new THREE.Vector3(-31,top,4.68),kind:'floor',source:`lift-floor-${floor+1}`});
    box('landing-ceiling',-33.9,F+2.72,-1.25,5.56,.07,11.66,stone);
    /* 结构壳体：四壁 + 门楣 + 门洞上方补板。这些是碰撞体，位置尺寸原样保留。 */
    box('cabin-back',-34.6,F+1.40,-3.95,3.1,2.60,.10,wood,true);
    for(const x of [-36.15,-33.05])box('cabin-side',x,F+1.40,-2.43,.10,2.60,2.94,wood,true);
    for(const x of [-36.10,-33.10])box('cabin-front',x,F+1.40,-.90,1.10,2.60,.12,stone,true);
    box('door-header',-34.6,F+2.48,-.85,2.08,.24,.18,metal);
    /* 石过梁比前壁（cabin-front，z ∈ [−0.96,−0.84]）**两面各多凸 5mm**。
     * 原来它和 cabin-front 同厚同 z，前后两个面都共面重叠 —— 舱内侧一条、厅侧一条。 */
    box('cabin-lintel',-34.6,F+2.648,-.90,2.08,.074,.13,stone);
    box('landing-cove',-32.25,F+2.655,-1.0,.22,.07,7.50,metal);
    box('landing-light',-32.25,F+2.612,-1.0,.09,.020,7.30,light);

    /* —— 地面：深色磨石 + 青铜镶边（镶边画在贴图里）。
     *    比电梯厅的米色石灰石深两档，一进门就有「轿厢」与「厅」的分界。 —— */
    const cabFloor=new THREE.Mesh(new THREE.PlaneGeometry(3.12,3.06),floorStone);
    cabFloor.name='lift-cabin-floor';cabFloor.rotation.x=-Math.PI/2;
    cabFloor.position.set(-34.60,F+.0665,-2.43);cabFloor.receiveShadow=true;
    cabFloor.userData.noOutline=true;group.add(cabFloor);

    /* —— 后壁：两片橡木 + 中间整高缎面背漆玻璃（青铜框）。
     *
     *    **端梃要比侧壁端梃后退 2mm**：两侧端梃在阴角处都收在 x = −36.08 这个平面上，
     *    共面重叠就是一条 2.44m 高的竖向闪纹（检测脚本报出的面积第二大的一处）。
     *    后退后由侧壁端梃盖住转角，视觉上仍是同一个阴角。
     *    同理，踢脚与顶冠都比侧壁那两条**短 160mm**、正好顶在侧壁构件上 ——
     *    拉满全宽时它们的端面与侧壁构件共面，也会闪。 —— */
    box('cabin-skirt-back',-34.60,F+.117,-3.885,2.94,.11,.030,dark);
    box('cabin-back-stile-w',-36.117,F+1.391,-3.890,.07,2.438,.020,wood);
    box('cabin-back-panel-w',-35.620,F+1.391,-3.894,.92,2.438,.012,wood);
    box('cabin-back-frame-w',-35.130,F+1.391,-3.890,.06,2.438,.020,metal);
    box('cabin-back-glass',-34.600,F+1.391,-3.895,1.00,2.438,.010,satinGlass);
    box('cabin-back-frame-e',-34.070,F+1.391,-3.890,.06,2.438,.020,metal);
    box('cabin-back-panel-e',-33.580,F+1.391,-3.894,.92,2.438,.012,wood);
    box('cabin-back-stile-e',-33.083,F+1.391,-3.890,.07,2.438,.020,wood);
    box('cabin-back-crown',-34.60,F+2.6475,-3.888,2.952,.075,.024,metal);

    /* —— 两侧壁：三片橡木 + 两道青铜凹缝。凹缝是「比木饰面多凸 6mm 的窄条」，
     *    没有布尔运算，这是最省事的做法。缝宽刻意只给 32mm：第一版 50mm 配深墨绿，
     *    在 2.9m 的墙上变成两条黑杠，把整面墙切碎了。 —— */
    box('cabin-skirt-west',-36.085,F+.117,-2.43,.030,.11,2.94,dark);
    for(const z of [-3.865,-0.995])box('cabin-west-stile',-36.090,F+1.391,z,.020,2.438,.07,wood);
    for(const z of [-2.902,-1.958])box('cabin-west-reveal',-36.089,F+1.391,z,.018,2.438,.032,metal);
    for(const z of [-3.374,-2.430,-1.486])box('cabin-west-panel',-36.094,F+1.391,z,.012,2.438,.912,wood);
    box('cabin-west-crown',-36.088,F+2.6475,-2.43,.024,.075,2.94,metal);
    box('cabin-skirt-east',-33.115,F+.117,-2.43,.030,.11,2.94,dark);
    for(const z of [-3.865,-0.995])box('cabin-east-stile',-33.110,F+1.391,z,.020,2.438,.07,wood);
    for(const z of [-2.902,-1.958])box('cabin-east-reveal',-33.111,F+1.391,z,.018,2.438,.032,metal);
    for(const z of [-3.374,-2.430,-1.486])box('cabin-east-panel',-33.106,F+1.391,z,.012,2.438,.912,wood);
    box('cabin-east-crown',-33.112,F+2.6475,-2.43,.024,.075,2.94,metal);

    /* —— 扶手：贴墙细青铜杆 + 背后的深色凹带。旧版是一根 4.5cm 方杆直接糊在后壁上，
     *    离墙只有 1.4cm，读起来是「墙上一条黑线」而不是扶手；现在三面都做，
     *    并且靠「深色凹带 + 支架」把它从墙面上拎出来。
     *
     *    **凹带的进深是被两侧夹出来的，不能随手改**：木饰面面在 −36.088、竖梃/凹缝面在
     *    −36.080，中间只有 8mm；凹带必须落在中间（取 −36.085）才既不埋进木板、也不与
     *    竖梃共面。原来直接取 −36.080，和竖梃、凹缝、后壁端梃三处共面 —— 三处闪纹。
     *    支架比凹带矮 2mm，同理：等高时两者的上下端面也是共面的。 —— */
    box('cabin-rail-band-back',-34.60,F+.96,-3.889,2.970,.105,.010,dark);
    box('cabin-rail-back',-34.60,F+.96,-3.885,2.70,.062,.030,metal);
    for(const x of [-35.60,-34.60,-33.60])box('cabin-rail-bracket-back',x,F+.96,-3.888,.045,.101,.024,metal);
    for(const [sfx,bx] of [['west',-36.085],['east',-33.115]] as const){
      box(`cabin-rail-band-${sfx}`,sfx==='west'?-36.090:-33.110,F+.96,-2.43,.010,.105,2.94,dark);
      box(`cabin-rail-${sfx}`,bx,F+.96,-2.43,.030,.062,2.62,metal);
      for(const z of [-3.50,-2.43,-1.36])box(`cabin-rail-bracket-${sfx}`,sfx==='west'?-36.088:-33.112,F+.96,z,.024,.101,.045,metal);
    }

    /* —— 吊顶：石质跌级 + 青铜收边 + 中央发光软膜 + 四角筒灯。
     *    旧版是一整块 2.56×2.45 的自发光板贴在顶板上，亮得发白、没有层次。
     *
     *    跌级是一个**环**，四块必须**对缝拼接、互不重叠**。原来四块在转角互相叠了
     *    0.30×0.15，同高度的两块底面就成了共面重叠 —— 检测脚本报出的面积最大的一处
     *    （0.276×0.126）。做法：东西两块跑满整个进深，南北两块夹在它们之间。
     *    青铜收边比石质跌级**多凸 5mm**：原来它和跌级的内侧面共面，是两条 2.1m 长的
     *    闪纹。四根收边同样对缝拼接（东西两根跑满、南北两根夹在中间），转角不重叠。 —— */
    for(const [x,z,w,d] of [[-34.60,-3.60,2.10,.30],[-34.60,-1.26,2.10,.30],[-35.80,-2.43,.30,2.64],[-33.40,-2.43,.30,2.64]] as const)
      box('cabin-soffit',x,F+2.635,z,w,.10,d,stone);
    box('cabin-cove-n',-34.60,F+2.600,-3.465,2.09,.05,.04,metal);
    box('cabin-cove-s',-34.60,F+2.600,-1.395,2.09,.05,.04,metal);
    box('cabin-cove-w',-35.665,F+2.600,-2.43,.04,.05,2.11,metal);
    box('cabin-cove-e',-33.535,F+2.600,-2.43,.04,.05,2.11,metal);
    box('cabin-light',-34.60,F+2.674,-2.43,2.06,.02,2.00,light);
    for(const x of [-35.80,-33.40])for(const z of [-3.60,-1.26]){
      const spot=new THREE.Mesh(new THREE.CylinderGeometry(.055,.055,.014,18),light);
      spot.name='lift-downlight';spot.position.set(x,F+2.578,z);
      spot.userData.noOutline=true;group.add(spot);
    }
    /* 轿厢自己的真实光源。发光软膜只是 emissive 材质，**不照亮任何东西** —— 这间
     * 轿厢原先一盏真实光源都没有，四壁全靠环境光；门扇这种带 metalness 的面只剩
     * 镜面项，会渲染成一块近黑的深棕（实测：把这几盏灯全关掉，门扇立刻退回暗橄榄
     * 棕）。所以必须留着。
     *
     * 位置刻意避开后壁中心：原先只在正中放一盏 (-34.60, F+2.40, -2.43)，它的 x 与
     * 后壁完全对齐，镜像光斑正落在板心，读起来就是「北墙上有一盏灯」——用户反馈
     * 的突兀光源就是它。现在拆成吊顶两侧各一盏，和四角筒灯同一条纵线上。
     * 但挪位置只是减弱：斜视时两侧灯的高光一样能落到后壁上，真正的根治在上面的
     * satinGlass 把高光瓣摊平了。
     * 这两盏灯进的是 RoomScene 的点光池，池子只在装配完成后收一次；玩家不在电梯
     * 附近时它们自然排不进前 16 盏，所以不会白付片元成本。 */
    for(const x of [-35.80,-33.40]){
      const cabLight=new THREE.PointLight('#ffe4bc',3.6,5.0,2);
      cabLight.position.set(x,F+2.50,-2.43);group.add(cabLight);
    }

    /* —— 门洞：轿厢侧青铜门套 + 门楣名牌；门洞上方补一块石过梁。 —— */
    box('cabin-jamb-w',-35.600,F+1.262,-.972,.10,2.40,.024,metal);
    box('cabin-jamb-e',-33.600,F+1.262,-.972,.10,2.40,.024,metal);
    box('cabin-door-head',-34.60,F+2.520,-.950,1.90,.18,.020,metal);
    box('cabin-door-plate',-34.60,F+2.520,-.966,1.20,.085,.012,nameplate);

    /* —— 操纵盘：贴在门右侧回转壁上（真实电梯的位置），盘面朝 −z。
     *    旧版钉在东侧壁上、离地 1.35m，且数字是 16 张透明小图 —— 又暗又糊。 —— */
    box('cop-plate',-33.375,F+1.320,-.972,.20,.86,.024,metal);
    for(let n=0;n<4;n++){
      const by=F+1.16+n*.13;
      disc('floor-button',-33.375,by,-.989,.030,.014,n===floor?light:dark,-.9975,numberMat(n,n===floor));
    }
    disc('cop-door',-33.425,F+1.00,-.989,.026,.012,dark,-.9975,glyphMat('door'));
    disc('cop-bell',-33.325,F+1.00,-.989,.026,.012,dark,-.9975,glyphMat('bell'));

    /* —— 门扇：加一条竖向青铜嵌条 + 底部踢脚板（挂在门扇下面，跟着一起滑）。 —— */
    const door=(sign:number)=>{
      const leaf=box('sliding-door',-34.6+sign*1.47,F+1.25,DOOR_Z,.95,2.34,.045,doorMat,false,dynamic);
      /* 嵌条贴在**前缘**（合拢时两扇相接的那条边，局部 x = −sign·0.44）：
       * 贴在尾缘的话开门时它跟着滑进回转壁后面，等于白做。 */
      box('door-inlay',-sign*.44,0,-.0295,.05,2.28,.014,metal,false,leaf);
      box('door-kick',0,-1.06,-.029,.88,.10,.012,dark,false,leaf);
      return leaf;
    };
    const pair:[THREE.Mesh,THREE.Mesh]=[door(-1),door(1)];
    leaves.push(pair);
    const c=document.createElement('canvas');c.width=256;c.height=128;const ctx=c.getContext('2d')!;
    ctx.fillStyle='#313b39';ctx.fillRect(0,0,256,128);ctx.fillStyle='#f0d6a5';ctx.font='48px sans-serif';ctx.textAlign='center';ctx.fillText(`${floor+1} F`,128,61);ctx.font='18px sans-serif';ctx.fillText(floor===0?'LOBBY / COFFEE':'RESIDENTS',128,102);
    const texture=new THREE.CanvasTexture(c);texture.colorSpace=THREE.SRGBColorSpace;
    const display=new THREE.MeshStandardMaterial({map:texture,emissive:'#d9c7a4',emissiveMap:texture,emissiveIntensity:.45});display.userData.outlineWeight=0;
    box('floor-display',-34.6,F+2.50,-.744,.59,.22,.015,display);

    /* —— 电梯厅侧：青铜门套 + 呼梯面板 + 入口垫。门套/面板的凸出量同样卡在 0.036。
     *    门套只做 130mm 宽：第一版 200mm 配旧版的深青铜，在门两侧压出两条黑边，
     *    和同样发暗的门扇连成一大块。 —— */
    box('hall-jamb-w',-35.615,F+1.282,-.825,.13,2.44,.030,metal);
    box('hall-jamb-e',-33.585,F+1.282,-.825,.13,2.44,.030,metal);
    /* 横楣顶端压在 2.594 而不是 2.60：门楣（door-header）的顶面正好在 2.60，
     * 等高时两条 2.04m 长的顶面共面重叠（这是唯一一处不在舱内、却仍然可见的）。 */
    box('hall-jamb-head',-34.60,F+2.542,-.825,2.30,.104,.030,metal);
    box('hall-call-plate',-35.850,F+1.180,-.828,.15,.36,.024,metal);
    disc('call-up',-35.850,F+1.262,-.810,.028,.012,dark,-.8015,glyphMat('up'),0);
    disc('call-down',-35.850,F+1.098,-.810,.028,.012,dark,-.8015,glyphMat('down'),0);
    box('hall-mat',-34.60,F+.068,.05,2.30,.012,1.10,dark);

    // Articulated tower facade, distinct from the apartments rather than a sixth unit.
    box('facade-band',-33.9,F+2.97,4.815,5.60,.18,.045,metal);
    for(let n=0;n<15;n++)box('facade-fin',-36.35+n*.35,F+1.45,4.817,.07,2.55,.055,wood);
  }
  const colliders=buildBoxColliders(specs).concat(floors);
  const hud=document.createElement('div');hud.dataset.roomLift='true';
  Object.assign(hud.style,{position:'absolute',left:'50%',bottom:'100px',transform:'translateX(-50%)',padding:'18px 24px',borderRadius:'12px',background:'rgba(24,31,30,.94)',color:'#f4ead8',fontFamily:'sans-serif',fontSize:'16px',textAlign:'center',pointerEvents:'none',display:'none',zIndex:'30',boxShadow:'0 8px 30px #0005',whiteSpace:'pre-line'});
  container.appendChild(hud);
  let enabled=false,activeFloor=-1,trip:{from:number;to:number;start:number;duration:number}|null=null,disposed=false;
  /**
   * 每层门是否被「叫开」。常驻闭合：这个标记为假时门就是合拢的。
   * F 键切换；到站自动置真（否则玩家出不去）；玩家离开该层、或电梯起身时收回。
   */
  const doorOpen=[false,false,false,false];
  const floorAt=()=>LIFT_LEVELS.findIndex(f=>Math.abs(camera.position.y-(f+.062+1.6))<.40);
  const inside=()=>Math.abs(camera.position.x-LIFT_CABIN.x)<1.30&&camera.position.z>-3.70&&camera.position.z<-1.02;
  /**
   * 玩家够得着哪一层的电梯门；−1 = 都够不着。
   *
   * 轿厢内也算：真实电梯的轿厢里有开门键，这里同样允许按 F 开关本层门——
   * 少了这一条，玩家在轿厢里退出公寓再进来就只能被困住（门常驻闭合，
   * 而门外侧的判定够不到）。
   */
  const nearFloor=()=>{
    const f=floorAt();
    if(f<0)return -1;
    if(inside())return f;
    const dx=camera.position.x-LIFT_CABIN.x,dz=camera.position.z-DOOR_Z;
    return Math.abs(dx)<1.45&&dz>-.05&&dz<2.60?f:-1;
  };
  function selectFloor(to:number,now=performance.now()){
    const from=floorAt();
    if(disposed||!enabled||trip||!inside()||from<0||!Number.isInteger(to)||to<0||to>3||to===from)return false;
    // 只清运动残留。不掰朝向、不把玩家拉进轿厢中心——起身这一刻玩家该怎么站就怎么站，
    // 位置的唯一变化发生在到站时（见 update 里的垂直平移）。
    fps.stopMotion();
    trip={from,to,start:now,duration:liftTravelMs(from,to)};return true;
  }
  function cancel(){
    if(trip){fps.stopMotion();trip=null;}
    doorOpen[0]=doorOpen[1]=doorOpen[2]=doorOpen[3]=false;
    hud.style.display='none';
  }
  const onKey=(e:KeyboardEvent)=>{
    if(e.repeat||!enabled)return;
    // F：开关够得着的那层门。运行中一律不受理——电梯在动，门必须合拢。
    if(e.code==='KeyF'){
      if(trip)return;
      const f=nearFloor();
      if(f>=0){e.preventDefault();doorOpen[f]=!doorOpen[f];}
      return;
    }
    if(!inside()||floorAt()<0)return;
    const n=/^(?:Digit|Numpad)([1-4])$/.exec(e.code);if(n){e.preventDefault();selectFloor(Number(n[1])-1);}
  };
  window.addEventListener('keydown',onKey);
  const doorClosure=[1,1,1,1];let previousUpdate=performance.now();
  /**
   * 门扇的挡人盒（合拢到位时才有）。两个扇各自一个盒，随开度移动。
   * 池化复用：每帧只调整 doorBlockers 的长度，不 new 对象。
   */
  const doorBlockerPool:Collider[]=[
    {min:new THREE.Vector3(),max:new THREE.Vector3()},
    {min:new THREE.Vector3(),max:new THREE.Vector3()},
  ];
  const doorBlockers:Collider[]=[];
  function update(now:number,firstPerson:boolean){
    const doorStep=Math.min(1,Math.max(0,now-previousUpdate)/450);previousUpdate=now;
    enabled=firstPerson;
    if(!enabled){cancel();activeFloor=-1;}else activeFloor=floorAt();
    if(trip){
      const elapsed=now-trip.start;
      if(elapsed>=trip.duration){
        const destination=trip.to;
        fps.stopMotion();
        // 到站只做一次垂直平移：水平位置取玩家自己的（不拉进轿厢中心），朝向根本不碰。
        // 运行期间同样不碰这两样——玩家在轿厢里转身张望是自由的。
        // 水平位置用 fps.getPosition() 而不是 camera.position：后者含步行 bob 偏移。
        const p=fps.getPosition();
        fps.setPosition(p.x,LIFT_LEVELS[destination]+.062+1.6,p.z);
        doorOpen[destination]=true;   // 到站开门，否则玩家被困在合拢的门后面
        activeFloor=destination;trip=null;
      }
    }
    // 够不着的那几层一律收回：门是"叫开"的，没人站在跟前就该合拢。
    const nf=nearFloor();
    for(let f=0;f<doorOpen.length;f++)if(doorOpen[f]&&f!==nf)doorOpen[f]=false;
    for(let f=0;f<leaves.length;f++){
      // 常驻闭合：只有「被叫开」且电梯没在跑时才开；运行时整排门都合拢。
      const wantOpen=trip===null&&doorOpen[f];
      doorClosure[f]=THREE.MathUtils.clamp(doorClosure[f]+(wantOpen?-doorStep:doorStep),0,1);
      const closeAmount=doorClosure[f];
      leaves[f].forEach((mesh,i)=>{mesh.position.x=LIFT_CABIN.x+(i===0?-1:1)*(1.47-closeAmount*.995);});
    }
    // 门合拢到一定程度才挡人：既不让人穿门而过，也不在门正在开合时把人夹住。
    doorBlockers.length=0;
    if(nf>=0&&doorClosure[nf]>.55){
      const base=LIFT_LEVELS[nf]+.062;
      for(let i=0;i<2;i++){
        const c=doorBlockerPool[doorBlockers.length];
        const cx=leaves[nf][i].position.x;
        c.min.set(cx-.475,base+.08,DOOR_Z-.03);
        c.max.set(cx+.475,base+2.42,DOOR_Z+.03);
        doorBlockers.push(c);
      }
    }
    if(!enabled||nf<0)hud.style.display='none';
    else{
      hud.style.display='block';
      const doorLine=doorOpen[nf]?'门已开':'门已关  ·  按 F 开门';
      hud.textContent=trip
        ?`电梯运行中  ${trip.from+1}F → ${trip.to+1}F\n约 ${Math.max(1,Math.ceil((trip.duration-(now-trip.start))/1000))} 秒后到达`
        :inside()
          ?`${nf+1}F  ·  ${nf===0?'大厅 / 咖啡吧':'住户层'}\n${doorLine}\n按 1 — 4 选择楼层`
          :`${nf+1}F  ·  电梯门前\n${doorLine}`;
    }
    return trip!==null;
  }
  return {group,dynamic,colliders,update,selectFloor,cancel,doorBlockers,getState:()=>({floor:activeFloor,busy:!!trip,doors:[...doorOpen],trip:trip?{...trip}:null}),dispose(){disposed=true;cancel();window.removeEventListener('keydown',onKey);hud.remove();}};
}
