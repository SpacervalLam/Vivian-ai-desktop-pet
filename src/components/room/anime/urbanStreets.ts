import * as THREE from 'three';
import { mergeByMaterial } from './merge';
import { createBlossomField, createFallenPetals, createLeafField, createTrunkField, plantTree, type TreeKit } from './foliage';
import type { Collider } from './collider';
import { buildCityRoutes, CITY_ROUTES } from './cityRoutes';

export const CITY_ROADS = {
  eastWest: [-69, -44, -19, 10.1, 30, 61].map(z=>({z,width:z===10.1?5:6.6})),
  northSouth: [{x:-46,width:8},{x:43,width:8}],
  bounds:{x0:-80,x1:80,z0:-78,z1:83},
};
export type CityLot={id:string;x:number;z:number;w:number;d:number;floors:number};

/**
 * 地块表。
 *
 * 这里修掉了旧版三个互相叠加、最后让街区读起来"空 + 平"的问题：
 *
 *  1. **高度场是 f(x) 而不是 f(x,z)**。旧规则 `x>50 ? 7+(|z|%3) : (z===44?4:5)`
 *     让 x=-68/-58/-32/-18/18/31 六列的层数序列**完全相同**（都是 5,5,4,5）——
 *     同一栋楼沿 x 复制六遍。从阳台望出去，天际线是一块 14m 高的平板，唯一
 *     起伏来自冠部随机（在 14m 楼上只有 16% 波动，破不了平台）。
 *     现在改成以公寓为原点的径向簇 + 两处偏心次高簇 + 每栋 seeded 抖动。
 *  2. **中央格 81m 宽只放 4 栋 9m 面宽**：行 0/1/4 各留 27m 空档，行 2/3 更是
 *     0 覆盖。站在街上看到的是"铺装广场上立着几栋楼"，不是街墙；阳台正南
 *     因此是一条 27m 宽、70m 长、直通天空的空走廊。
 *  3. 补齐行 4 中央会压到社区绿地，所以绿地同步从 ±10 收窄到 ±9。
 *
 * 顺带：正对阳台的那一跨（便利店所在）**刻意不补**。它是这条街唯一的暖光，
 * 也是主视轴的地面落点；补上就把"街角看店"的构图堵死了。而它西侧是停车场用地，
 * 同样放不下楼——所以这一排真正要收的空档只有东侧那一段。
 */
/* --- 地块表开始（scripts/room/analyze-district-plan.mjs 依赖这个标记做源码解析） --- */
export const CITY_LOTS:CityLot[]=[];
/**
 * 确定性散列。用乘法散列的高位——低位没被混匀，直接取模会退化成等差数列。
 * （踩过的坑：乘数取 40503 = 3×13501，任何倍数都能被 3 整除，`%3` 恒为 0，
 * 抖动永远等于 -1，四栋近景楼全部掉到最低档。）
 */
const hash=(i:number,salt:number)=>(Math.imul(i+salt,2654435761)>>>8);

/**
 * 高度场：径向簇 + 两处偏心次高簇 + 每栋抖动，夹在 4~12 层。
 *
 * 近景刻意压低（4~6 层）：公寓正对的那排必须矮，否则 8.6m 外的街道变成天井。
 * 远景逐级抬高，天际线才有前中后三层而不是一块平板。两处偏心簇（东南
 * (34,-46)、西北 (-52,30)）是故意的——只有径向簇的话等高线是同心圆，从
 * 阳台上看仍然是"一圈一圈"的假。
 */
function floorsAt(x:number,z:number,index:number):number{
  const r=Math.hypot(x,z);
  const base=r<26?5:r<48?6:r<70?7:9;
  const boost=(Math.hypot(x-34,z+46)<26?2:0)+(Math.hypot(x+52,z-30)<22?1:0);
  const jitter=r<26?hash(index,7)%3-1:hash(index,1)%5-2;
  return Math.max(4,Math.min(12,base+boost+jitter));
}
function lot(id:string,x:number,z:number,w:number,d:number,floors?:number){
  CITY_LOTS.push({id,x,z,w,d,floors:floors??floorsAt(x,z,CITY_LOTS.length)});
}

// 东西向四行，每行 8 列
for(const z of [-57,-31,44,73])for(const x of [-68,-58,-32,-18,18,31,56,69])lot(`parcel-${x}-${z}`,x,z,9,z===73?8:10);
// 中央格行 0/1：旧版 x∈[-13.5,13.5] 这 27m 整段空着，按 9m 模数补满成街墙
for(const z of [-57,-31])for(const x of [-9,0,9])lot(`parcel-${x}-${z}`,x,z,9,10);
// 行 4 中央：绿地占 ±9，两侧各补一栋窄的，把 27m 空档收掉
for(const x of [-11.25,11.25])lot(`parcel-${x}-44`,x,44,4.5,10);
// 公寓对街保留便利店与书店；东段两地块由 sakuraTown 重建为四间低层商店。
// 西侧原停车场改为自行车铺、茶房和神社庭院，同样由 sakuraTown 提供几何与碰撞。
// These two lots are a low-rise shopping street, with furnished interiors in sakuraTown.
lot('parcel-23.5-19.6',23.5,19.6,6.8,7.6,2);
lot('parcel-31-19.6',31,19.6,6.8,7.6,2);
// 侧地块
for(const x of [-66,64])for(const z of [-5,20])lot(`side-${x}-${z}`,x,z,13,z===20?7.4:11);
// Reserve the station approach and plaza; keep the existing road network intact.
for(let i=CITY_LOTS.length-1;i>=0;i--){const p=CITY_LOTS[i];if(p.x===69 && (p.z===44 || p.z===73))CITY_LOTS.splice(i,1);}
// 北侧孪生公寓的用地（落位见 exterior.ts 的 APT_TWIN_DZ）。
// 新楼实测横占 x -31.38..38.25、纵占 z -37.98..-23.4，正好压在 z=-31 这一排上；
// 这一排里 x -32..31 的地块整块落在楼体里，不清掉就是楼与低层店面互穿。
// 只清这一排：z=-57 那排在楼北面之外，而 z=-19 那条是街、本来就没有地块。
for(let i=CITY_LOTS.length-1;i>=0;i--){const p=CITY_LOTS[i];if(p.z===-31&&p.x>=-33&&p.x<=38)CITY_LOTS.splice(i,1);}

/**
 * 街区里哪些地块盖**高楼**（而不是 sakuraTown 那批低层店屋）。
 *
 * 背景：第一阶段把整片街区的楼都挪去了远景当低模天际线，街区里只剩下
 * sakuraTown 那批 3.24m 面宽、5.5~6.5m 高的店屋——每一排都是同一种房子换
 * 名字换颜色，走一趟就腻。所以按确定性散列挑约三成地块把高楼请回街区里，
 * 和店屋交错，天际线才有前中后三层。
 *
 * 两条硬约束，都在下面这个循环里：
 *
 *  1. **一块地只能有一个主人。** districtArt 与 sakuraTown 都按这张地块表盖房子，
 *     同一块地两边都盖就是楼与店屋互穿。所以这里导出唯一一份集合，两边都读它
 *     （districtArt 就地盖高楼，sakuraTown 跳过）。
 *
 *  2. **临街正对段必须留低。** 见地块表上方「近景刻意压低」那段：公寓南北那条街
 *     （z ∈ [-22,30] 且 |x| < 45）是公寓出门看到的街景，塔楼杵在这儿会把 6.6m 宽的
 *     街变成天井；而 x > 45 且 z ∈ [10,30] 是駅前通り那 7 间店屋，塔楼会直接插进
 *     它们的几何里。这两段之外才允许放高楼。
 *
 * 散列盐取 71 是挑过的：出 10 栋，铺在 z = -57 / -31 / -5 / 44 / 73 五行上，
 * 行内互不相邻（相邻地块 9m 间距，塔楼 footprint 再内缩 0.8m，见 districtArt 的
 * TOWER_INSET），既不撞在一起也不排成一堵连续长墙。
 */
export const CITY_INBLOCK_TOWER_LOTS=new Set<string>();
for(let i=0;i<CITY_LOTS.length;i++){
  const p=CITY_LOTS[i];
  if(p.id.endsWith('-19.6'))continue;                              // 这两块是 sakuraTown 的入户店
  if(p.z>=-22&&p.z<=30&&Math.abs(p.x)<45)continue;                 // 公寓门前的街景
  if(p.z>=10&&p.z<=30&&p.x>45)continue;                            // 駅前通り的店屋
  if(hash(i,71)%3===0)CITY_INBLOCK_TOWER_LOTS.add(p.id);
}
// Slightly stagger southern parcels around the curving lane; IDs and authored building details stay stable.
for(const p of CITY_LOTS){if(p.z===73)p.z+=2.8*Math.sin(p.x/19);if(p.z===44&&Math.abs(p.x)>14)p.z+=1.6*Math.sin(p.x/23);}
/* --- 地块表结束 --- */

export function buildUrbanStreets(scene:THREE.Scene){
  const group=new THREE.Group();group.name='planned-street-network';group.userData.sceneCollideSkip=true;scene.add(group);
  const textures:THREE.Texture[]=[];const materials:THREE.Material[]=[];const colliders:Collider[]=[];
  /** 合批之后才挂进 group 的实例网格几何（mergeByMaterial 看不到，需自行释放）。 */
  const extraGeometries:THREE.BufferGeometry[]=[];
  let seed=441;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  /** 细粒噪声图：bump / roughness 走逐像素噪声，平铺不可见，无所谓对齐。 */
  function noiseMap(kind:'bump'|'rough'){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=512;const c=canvas.getContext('2d')!;const data=c.createImageData(512,512);
    for(let y=0;y<512;y++)for(let x=0;x<512;x++){
      const i=(y*512+x)*4,n=random(),macro=Math.sin(x*Math.PI/256)*Math.cos(y*Math.PI/256)*1.2;
      const value=kind==='bump'?100+n*55:155+macro*4+n*20;
      data.data[i]=value;data.data[i+1]=value;data.data[i+2]=value;data.data[i+3]=255;
    }c.putImageData(data,0,0);
    const t=new THREE.CanvasTexture(canvas);t.wrapS=t.wrapT=THREE.RepeatWrapping;t.anisotropy=8;t.colorSpace=THREE.NoColorSpace;textures.push(t);return t;
  }
  /**
   * 动画风沥青：参考街景的路面是**中明度冷灰蓝、大面平整**，只有淡淡的
   * 补丁/雨渍色斑 —— 旧版 53 基准 + 强逐像素噪声读起来像砂纸，把整条街拉暗。
   * 色斑用低透明大圆斑叠出来，亮度扰动压在 ±8 以内，2m 平铺一次才不显形。
   * 有意不做轮胎磨痕条纹：uv 按世界坐标 /2 映射，各条路的 z 不落在同一
   * 2m 网格上，贴图里的条纹会错位 —— 对齐敏感的线条全部用几何 rect 画。
   */
  function asphaltMap(){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=512;const c=canvas.getContext('2d')!;
    c.fillStyle='#575e6b';c.fillRect(0,0,512,512);
    for(let n=0;n<64;n++){
      const x=random()*512,y=random()*512,r=30+random()*90;
      const tone=random()<.5?'rgba(70,77,89,':'rgba(98,105,118,';
      const g=c.createRadialGradient(x,y,0,x,y,r);
      g.addColorStop(0,tone+(.05+random()*.07).toFixed(3)+')');g.addColorStop(1,tone+'0)');
      c.fillStyle=g;c.beginPath();c.arc(x,y,r,0,Math.PI*2);c.fill();
    }
    const data=c.getImageData(0,0,512,512);
    for(let i=0;i<data.data.length;i+=4){const n=(random()-.5)*9;data.data[i]+=n;data.data[i+1]+=n;data.data[i+2]+=n+2;}
    c.putImageData(data,0,0);
    const t=new THREE.CanvasTexture(canvas);t.wrapS=t.wrapT=THREE.RepeatWrapping;t.anisotropy=8;t.colorSpace=THREE.SRGBColorSpace;textures.push(t);return t;
  }
  /**
   * 石砖步行道：参考图两侧是**暖米色大石板**（约 0.5m 一格），砖缝清晰、
   * 每砖自带明度色差。uv 按世界坐标 /2 映射，512px 里画 4×4 格 → 每格
   * 0.5m，砖缝自动落在世界 0.5m 网格上，相邻 rect 的砖格能对上。
   */
  function paverMap(){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=512;const c=canvas.getContext('2d')!;
    const cell=128;
    for(let ty=0;ty<4;ty++)for(let tx=0;tx<4;tx++){
      const v=196+(random()-.5)*24;
      c.fillStyle=`rgb(${v},${v-6},${v-22})`;
      c.fillRect(tx*cell,ty*cell,cell,cell);
      for(let n=0;n<26;n++){
        c.fillStyle=`rgba(122,116,98,${(random()*.09).toFixed(3)})`;
        c.fillRect(tx*cell+random()*cell,ty*cell+random()*cell,1.6,1.6);
      }
    }
    c.fillStyle='#8b8577';
    for(let k=0;k<=4;k++){c.fillRect(k*cell-2,0,4,512);c.fillRect(0,k*cell-2,512,4);}
    const t=new THREE.CanvasTexture(canvas);t.wrapS=t.wrapT=THREE.RepeatWrapping;t.anisotropy=8;t.colorSpace=THREE.SRGBColorSpace;textures.push(t);return t;
  }
  const bump=noiseMap('bump'),rough=noiseMap('rough');
  const asphalt=new THREE.MeshStandardMaterial({map:asphaltMap(),roughnessMap:rough,bumpMap:bump,bumpScale:.006,roughness:.94,metalness:0});
  const paving=new THREE.MeshStandardMaterial({map:paverMap(),bumpMap:bump,bumpScale:.002,roughness:.9});
  const stone=new THREE.MeshStandardMaterial({color:'#a4a69e',roughness:.86});
  const paint=new THREE.MeshStandardMaterial({color:'#ddd9c5',roughness:.85});
  const paintYellow=new THREE.MeshStandardMaterial({color:'#d9a93f',roughness:.85});
  const tactile=new THREE.MeshStandardMaterial({color:'#c9b04c',roughness:.85});
  const gutter=new THREE.MeshStandardMaterial({color:'#484e59',roughness:.92});
  const iron=new THREE.MeshStandardMaterial({color:'#313e40',roughness:.6,metalness:.35});
  const grass=new THREE.MeshStandardMaterial({color:'#536e52',roughness:1});
  const courtyard=new THREE.MeshStandardMaterial({color:'#80877c',roughness:.97});
  const wood=new THREE.MeshStandardMaterial({color:'#877052',roughness:.85});
  const bark=new THREE.MeshStandardMaterial({color:'#62594b',roughness:.96});
  const leaves=new THREE.MeshStandardMaterial({color:'#507763',roughness:.96});
  materials.push(asphalt,paving,stone,paint,paintYellow,tactile,gutter,iron,grass,wood,bark,leaves,courtyard);
  materials.forEach(m=>m.userData.outlineWeight=0);
  const surfaces:Array<{name:string;x0:number;x1:number;z0:number;z1:number;y:number}>=[];
  const cube=new THREE.BoxGeometry(1,1,1);
  const rect=(name:string,x0:number,x1:number,z0:number,z1:number,y:number,material:THREE.Material)=>{
    surfaces.push({name,x0,x1,z0,z1,y});
    const geo=new THREE.PlaneGeometry(x1-x0,z1-z0);geo.rotateX(-Math.PI/2);
    const pos=geo.attributes.position,uv=geo.attributes.uv;for(let i=0;i<pos.count;i++)uv.setXY(i,(pos.getX(i)+(x0+x1)/2)/2,(pos.getZ(i)+(z0+z1)/2)/2);
    const mesh=new THREE.Mesh(geo,material);mesh.name=name;mesh.position.set((x0+x1)/2,name.includes('crosswalk')?Math.max(y,.027):y,(z0+z1)/2);mesh.receiveShadow=true;group.add(mesh);return mesh;
  };
  const box=(x:number,y:number,z:number,w:number,h:number,d:number,material:THREE.Material)=>{const m=new THREE.Mesh(cube,material);m.position.set(x,y,z);m.scale.set(w,h,d);m.castShadow=true;m.receiveShadow=true;group.add(m);return m;};
  function pavement(x0:number,x1:number,z0:number,z1:number){
    // 退化条带直接丢：地块入口落客带是按「路缘外沿 → 楼体正面」算的，某些
    // 进深下两者会重合（长度 0），零面积的 surface 会污染平面重叠自检。
    if(x1-x0<=.001||z1-z0<=.001)return;
    rect('sidewalk',x0,x1,z0,z1,.028,paving);
    colliders.push({kind:'floor',source:'planned-sidewalk',min:new THREE.Vector3(x0,.008,z0),max:new THREE.Vector3(x1,.048,z1)});
  }
  // Land is no longer an asphalt grid: vehicle routes and garden walks have separate hierarchy.
  rect('neighbourhood-landscape',-80,80,-78,83,-.006,courtyard);
  extraGeometries.push(...buildCityRoutes(group,colliders,CITY_ROUTES,{road:asphalt,paving,paint}));
  // Short entry paths join each retained parcel to its nearest new route.
  for(const lot of CITY_LOTS){
    let best:THREE.Vector3|undefined,dist=Infinity;
    const door=new THREE.Vector3(lot.x,0,lot.z-lot.d/2);
    for(const route of CITY_ROUTES){const curve=new THREE.CatmullRomCurve3(route.points.map(([x,z])=>new THREE.Vector3(x,0,z)),false,'centripetal');for(const p of curve.getSpacedPoints(100)){const d=p.distanceToSquared(door);if(d<dist){dist=d;best=p;}}}
    if(best&&dist<225)extraGeometries.push(...buildCityRoutes(group,colliders,[{name:`entry-${lot.id}`,points:[[best.x,best.z],[door.x,door.z]],width:1.6,walk:true}],{road:asphalt,paving,paint},.019));
  }
  // Main street retains its familiar dashed line; neighbourhood lanes have no centre stripe.
  for(const z of [10.1,-19])for(let x=-78;x<78;x+=4.5)if(Math.abs(x+46)>7&&Math.abs(x-43)>7)rect('lane-mark',x,x+2,z-.045,z+.045,.025,paintYellow);
  // The station keeps a direct crossing on the existing slow street. The shop entrance
  // is already served by the store-aligned crossing built in exterior.ts (4.6 m wide
  // bars, centred x≈1.1) — a second, narrower 2.4 m set used to sit on top of it here.
  // 2026-09-25 按参考图在**神社参道口**补一道过街斑马线：x=-9.1 与鸟居中线正对，
  // 和车站那道（x=-24）相隔 15m，是两道各自独立的过街口，不是叠在一起。
  // 顺带给这道口子的两侧路缘铺点字砖——参考图前景那条黄色盲道正是过街口的标准配置。
  for(let z=7.95;z<12.4;z+=.75)rect('frontage-crosswalk',-25.2,-22.8,z,z+.38,.007,paint);   // 车站口（原样）
  /* 神社口起步取 **7.55 而不是 7.95**：0.75m 步距下 7.95 起步会把黄色虚线
   * （z 10.055..10.145）正好落在两条白条的**缝**里（条带是 …9.45..9.83 / 10.20..10.58），
   * 俯视能看到黄线从斑马线中间穿出来。7.55 起步则有一条白条 9.80..10.18 正好压住黄线，
   * 与用户既有要求"黄色中心线要被白色斑马线盖住"一致。 */
  for(let z=7.55;z<12.4;z+=.75)rect('frontage-crosswalk',-10.3,-7.9,z,z+.38,.007,paint);
  for(const edge of [-1,1])rect('tactile-threshold',-10.25,-7.95,10.1+edge*(5/2+.4)-.16,10.1+edge*(5/2+.4)+.16,.035,tactile);
  // 路面落花漂积：樱花沿街几段路面上积出浅浅一层花瓣（参考图路面中央的
  // 粉色花痕）。scatter 圆心走在行车道范围内，y 压在标线上方 —— 花瓣盖线。
  const roadPetals=createFallenPetals(random,700);
  materials.push(roadPetals.material);extraGeometries.push(roadPetals.geometry);
  for(const [px,pz] of [[-24,10.1],[2,10.1],[15,10.35],[22,9.9],[29,10.2],[36,10.1],[-8,30],[6,30],[18,29.7],[-32,-19],[12,-19]])
    roadPetals.scatter(px+(random()-.5)*2.4,pz+(random()-.5)*.9,.02,1.8,44);
  // ── 路口蓝底路名牌（参考图路口的蓝色标志牌）──
  // 独立 canvas 牌面 + 细立杆；杆子细，但人在街上会撞，给一个小碰撞盒。
  function plateMat(title:string,sub:string,bg:string,ink:string){
    const canvas=document.createElement('canvas');canvas.width=768;canvas.height=320;
    const c=canvas.getContext('2d')!;c.fillStyle=bg;c.fillRect(0,0,768,320);
    c.strokeStyle=ink;c.lineWidth=10;c.strokeRect(14,14,740,292);
    c.textAlign='center';c.textBaseline='middle';c.fillStyle=ink;
    c.font='600 118px "Yu Mincho","Microsoft YaHei",serif';c.fillText(title,384,124,690);
    c.font='54px sans-serif';c.fillText(sub,384,238,680);
    const t=new THREE.CanvasTexture(canvas);t.colorSpace=THREE.SRGBColorSpace;t.anisotropy=4;textures.push(t);
    const m=new THREE.MeshStandardMaterial({map:t,roughness:.85});materials.push(m);return m;
  }
  function roadSign(x:number,z:number,rotY:number,title:string,sub:string){
    box(x,1.02,z,.08,2.05,.08,iron);
    const p=new THREE.Mesh(new THREE.PlaneGeometry(1.04,.43),plateMat(title,sub,'#3d6396','#eef2f6'));
    p.position.set(x,2.02,z);p.rotation.y=rotY;p.castShadow=true;group.add(p);extraGeometries.push(p.geometry);
    colliders.push({source:'street-sign',min:new THREE.Vector3(x-.12,0,z-.12),max:new THREE.Vector3(x+.12,2.1,z+.12)});
  }
  roadSign(-51.5,34.4,Math.PI/2,'桜町通り','SAKURAMACHI 1-CHOME');
  roadSign(48.7,-39.1,-Math.PI/2,'春日通り','KASUGA 2-CHOME');
  for(const road of CITY_ROADS.eastWest.filter(r=>r.z===10.1||r.z===-19))for(let x=-73;x<78;x+=12){
    if(CITY_ROADS.northSouth.some(c=>Math.abs(x-c.x)<7))continue;
    for(const side of [-1,1]){const z=road.z+side*(road.width/2-.22);box(x,.012,z,.7,.022,.3,iron);for(let n=0;n<7;n++)box(x-.28+n*.09,.027,z,.023,.012,.25,stone);}
  }
  // Neighborhood green: public path connects the service road to the southern street.
  // 绿地收窄到 ±9：行 4 中央的 27m 空档两侧各补了一栋 4.5m 面宽的窄楼
  // （x=±11.25），绿地再占 ±10 就会被压到楼底下。
  rect('community-green',-9,9,36,55,.015,grass);pavement(-1.4,1.4,35.7,55.3);pavement(-9,-1.4,44.2,46.2);pavement(1.4,9,44.2,46.2);
  // 冠簇几何全部由 foliage 提供，旧的 crown/trunk 球体已退役。
  // 公园老樱与近景商店街共用同一套花簇贴图/实例收集器（见 foliage）。
  // 旧版是 11 个二十面体球 —— 从公寓阳台正南望出去，中景正好是那几团**粉色气球**，
  // 和两侧街道的樱花明显不是同一个世界。
  const parkBlossom=createBlossomField(random,4300);
  const parkFallen=createFallenPetals(random,900);
  // 绿树（市政行道树）也换成冠簇系统，只是贴图换成叶簇 —— 旧版是**5 个一模一样的
  // 平滑球**，单色，剪影从任何角度看都是一个圆，和樱花那边同样是"气球"。
  const leafField=createLeafField(random,16000);
  materials.push(parkBlossom.material,parkFallen.material,leafField.material);
  textures.push(parkBlossom.texture,leafField.texture);
  extraGeometries.push(parkBlossom.geometry,parkFallen.geometry,leafField.geometry);
  // 单位圆柱 + 四元数定向：全场枝条共用一个几何，省掉每根枝一个 CylinderGeometry。
  // mergeByMaterial 按引用计数释放，合批后引用归零才真 dispose，所以共用是安全的。
  const branchGeo=new THREE.CylinderGeometry(.75,1,1,6);
  const upAxis=new THREE.Vector3(0,1,0);
  function rod(a:number[],b:number[],r:number){
    const from=new THREE.Vector3(a[0],a[1],a[2]),to=new THREE.Vector3(b[0],b[1],b[2]);
    const dir=new THREE.Vector3().subVectors(to,from),len=dir.length();
    const m=new THREE.Mesh(branchGeo,bark);
    m.position.copy(from).addScaledVector(dir,.5);
    m.quaternion.setFromUnitVectors(upAxis,dir.normalize());
    m.scale.set(r,len,r);m.castShadow=true;group.add(m);return m;
  }
  /**
   * 行道树 / 公园老樱。
   *
   * 树的构造已经提到 `foliage.plantTree`（河堤也要用同一套，否则河边会出现第三种
   * 树）。这里只留两件本地的事：树穴材质（本地的 stone/grass）与碰撞盒登记。
   */
  const treeKit: TreeKit = {
    rod,
    // 树干走 Blender 资产（foliage 的干场）。几何在 scripts/trees/build_trunks.py，
    // 合批之后再挂 —— 见下面 mergeByMaterial 那段的说明。
    trunk: createTrunkField(),
    pit: (x, z) => { box(x, .12, z, .7, .20, .7, stone); box(x, .235, z, .58, .035, .58, grass); },
    random,
    blossom: parkBlossom,
    leaf: leafField,
    fallen: parkFallen,
  };
  function tree(x:number,z:number,bloom=false,treeIndex=0){
    plantTree(treeKit, x, z, bloom, treeIndex);
    colliders.push({source:'street-tree',min:new THREE.Vector3(x-.35,0,z-.35),max:new THREE.Vector3(x+.35,.9,z+.35)});
  }
  // x 从 ±7 收到 ±5.8：树冠最大外延 1.9+1.02≈2.9m，x=±7 时会穿进 x∈[-13.5,-9]
  // 那两栋 parcel-±11.25-44（z 39~49）的东/西墙。收到 ±5.8 后外延 ≤8.7 <9，留在绿地内。
  for(const x of [-5.8,5.8])for(const z of [38.5,51.5])tree(x,z,true,(x>0?2:0)+(z>45?1:0));
  // A small neighborhood shrine gate frames the park path, visible from the street.
  const vermilion=new THREE.MeshStandardMaterial({color:'#c95049',roughness:.72});
  const shrineCap=new THREE.MeshStandardMaterial({color:'#49394a',roughness:.74});
  const rope=new THREE.MeshStandardMaterial({color:'#f0d9a7',roughness:.95});
  materials.push(vermilion,shrineCap,rope);
  for(const m of [vermilion,shrineCap,rope])m.userData.outlineWeight=1.4;
  const gateZ=36.2;
  for(const x of [-1.45,1.45]){
    box(x,1.48,gateZ,.24,2.96,.25,vermilion);
    box(x,2.94,gateZ,.40,.22,.39,shrineCap);
  }
  box(0,3.16,gateZ,3.85,.28,.48,vermilion);
  box(0,3.34,gateZ,4.28,.2,.64,shrineCap);
  // 注连绳：横绳贴着下梁底面（梁底 y=3.02），纸垂从横绳上垂下来。
  // 原来只有 x=0 处一个 0.12m 的小结，10 个纸垂却挂在 y 2.43~2.65 —— 离梁底
  // 0.37m，两头都不挨着，正面看就是一排凭空悬着的木片。
  box(0,2.95,gateZ,3.0,.14,.14,rope);
  for(let x=-1.18;x<=1.19;x+=.24)box(x,2.77,gateZ,.055,.22,.055,rope);
  colliders.push({source:'shrine-gate-post',min:new THREE.Vector3(-1.62,0,gateZ-.2),max:new THREE.Vector3(-1.28,3,gateZ+.2)});
  colliders.push({source:'shrine-gate-post',min:new THREE.Vector3(1.28,0,gateZ-.2),max:new THREE.Vector3(1.62,3,gateZ+.2)});
  const avenueSamples=CITY_ROUTES.filter(r=>r.name==='west-neighbourhood-avenue'||r.name==='east-station-avenue').map(r=>new THREE.CatmullRomCurve3(r.points.map(([x,z])=>new THREE.Vector3(x,0,z)),false,'centripetal').getSpacedPoints(160));
  function avenueX(x:number,z:number){return avenueSamples[x<0?0:1].reduce((a,b)=>Math.abs(a.z-z)<Math.abs(b.z-z)?a:b).x;}
  let greenTree=0;
  for(const r of CITY_ROADS.northSouth)for(let z=-61;z<78;z+=12)if(CITY_ROADS.eastWest.every(c=>Math.abs(z-c.z)>7))for(const s of [-1,1])tree(avenueX(r.x,z)+s*5.4,z,false,greenTree++);
  for(const x of [-5,5])for(const z of [42,49]){box(x,.47,z,2,.09,.5,wood);box(x,.85,z+.22,2,.65,.06,wood);for(const dx of [-.7,.7])box(x+dx,.24,z,.08,.48,.42,iron);colliders.push({source:'park-bench',min:new THREE.Vector3(x-1,0,z-.25),max:new THREE.Vector3(x+1,1.18,z+.26)});}
  const lantern=new THREE.MeshStandardMaterial({color:'#ffe2b1',emissive:'#ffd299',emissiveIntensity:1.3});materials.push(lantern);
  for(const road of CITY_ROADS.northSouth)for(let z=-61;z<79;z+=18){
    if(CITY_ROADS.eastWest.some(r=>Math.abs(z-r.z)<7))continue;
    for(const side of [-1,1]){const x=avenueX(road.x,z)+side*5.2;box(x,2.1,z,.085,4.2,.085,iron);box(x-side*.4,4.16,z,.88,.08,.10,iron);box(x-side*.78,4.07,z,.32,.09,.18,lantern);}
  }
  // Low bollards define park entries while leaving the central path clear.
  for(const z of [34.1,56.9])for(const x of [-1.1,1.1])box(x,.38,z,.12,.76,.12,iron);
  group.userData.plan={roads:CITY_ROUTES,lots:CITY_LOTS,surfaces};
  mergeByMaterial(group);
  // 花簇/落花是 InstancedMesh，而 mergeByMaterial 只按 isMesh 过滤 —— 它也是 isMesh，
  // 混在 group 里会被当成一块 2×2 面片烘掉。所以合批之后再挂。
  group.add(parkBlossom.build('park-blossom-clusters'));
  group.add(parkFallen.build('park-fallen-petals'));
  group.add(leafField.build('street-leaf-clusters'));
  /* 树干场同理：合批之后才挂。几何得自己登记进 `extraGeometries` —— 本模块的
   * dispose 只认那个数组（不像 sakuraTown/sakuraStation 是遍历 root 收几何）。 */
  const trunkGroup = treeKit.trunk.build('street-trunks', bark.color);
  trunkGroup.traverse((o) => { const m = o as THREE.Mesh; if (m.isMesh) extraGeometries.push(m.geometry); });
  group.add(trunkGroup);
  group.add(roadPetals.build('road-petal-drifts'));
  return{asphalt,paving,stone,colliders,
    setWet(wet:boolean){asphalt.roughness=wet?.48:.94;paving.roughness=wet?.63:.9;},
    dispose(){textures.forEach(t=>t.dispose());materials.forEach(m=>m.dispose());extraGeometries.forEach(g=>g.dispose());}
  };
}
