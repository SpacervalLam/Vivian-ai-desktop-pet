import * as THREE from 'three';
import { mergeByMaterial } from './merge';
import type { Collider } from './collider';

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
// 行 3 中央（公寓正对面这条街）只补**东段**。
//
// 西段 x∈[-19,-4.4] 是手工件的低层用地：露天停车场（LOT，x -19.0~-5.6、
// z 14.0~25.6）铺满整个块深（12.6~26.7），便利店紧贴其东（西山墙 x=-4.4）。
// 那里没有放楼的余地——早先按「西侧留 14.8m 洞」补过两栋，结果一栋整栋站在
// 停车场里、另一栋啃掉便利店西墙 0.1m。所以西侧刻意留白：街墙在这里断一口，
// 正好把停车场的暖光露给阳台。
// 东段 x∈[19.9,39] 是 19.1m 空档，补两栋把街道收成一条 ~11m 宽的峡谷。
// 高度单独锁在 4~5 层：它离阳台只有 8.6m，是全场唯一「高度直接等于压迫感」
// 的位置——径向高度场在这里给到 6 层（16.8m / 50° 仰角）会变成天井，给到 3 层
// 又框不住。11.2~14m 落在 33°~43°，是看得到对街、又还留着一线天的区间。
lot('parcel-23.5-19.6',23.5,19.6,6.8,7.6,4);
lot('parcel-31-19.6',31,19.6,6.8,7.6,5);
// 侧地块
for(const x of [-66,64])for(const z of [-5,20])lot(`side-${x}-${z}`,x,z,13,z===20?7.4:11);
/* --- 地块表结束 --- */

export function buildUrbanStreets(scene:THREE.Scene){
  const group=new THREE.Group();group.name='planned-street-network';group.userData.sceneCollideSkip=true;scene.add(group);
  const textures:THREE.Texture[]=[];const materials:THREE.Material[]=[];const colliders:Collider[]=[];
  let seed=441;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  function map(kind:'asphalt'|'paver'|'bump'|'rough'){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=512;const c=canvas.getContext('2d')!;const data=c.createImageData(512,512);
    for(let y=0;y<512;y++)for(let x=0;x<512;x++){
      const i=(y*512+x)*4,n=random(),macro=Math.sin(x*Math.PI/256)*Math.cos(y*Math.PI/256)*1.2;
      let value=kind==='asphalt'?53+macro+n*15:kind==='paver'?151+macro+n*12:kind==='bump'?100+n*55:155+macro*4+n*20;
      if(kind==='paver'&&(x%128<2||y%128<2))value=105;
      data.data[i]=value;data.data[i+1]=value+(kind==='asphalt'?3:0);data.data[i+2]=value+(kind==='asphalt'?5:kind==='paver'?-5:0);data.data[i+3]=255;
    }c.putImageData(data,0,0);
    const t=new THREE.CanvasTexture(canvas);t.wrapS=t.wrapT=THREE.RepeatWrapping;t.anisotropy=8;t.colorSpace=kind==='asphalt'||kind==='paver'?THREE.SRGBColorSpace:THREE.NoColorSpace;textures.push(t);return t;
  }
  const bump=map('bump'),rough=map('rough');
  const asphalt=new THREE.MeshStandardMaterial({map:map('asphalt'),roughnessMap:rough,bumpMap:bump,bumpScale:.008,roughness:.94,metalness:0});
  const paving=new THREE.MeshStandardMaterial({map:map('paver'),bumpMap:bump,bumpScale:.002,roughness:.9});
  const stone=new THREE.MeshStandardMaterial({color:'#a4a69e',roughness:.86});
  const paint=new THREE.MeshStandardMaterial({color:'#ddd9c5',roughness:.85});
  const iron=new THREE.MeshStandardMaterial({color:'#313e40',roughness:.6,metalness:.35});
  const grass=new THREE.MeshStandardMaterial({color:'#536e52',roughness:1});
  const courtyard=new THREE.MeshStandardMaterial({color:'#80877c',roughness:.97});
  const wood=new THREE.MeshStandardMaterial({color:'#877052',roughness:.85});
  const bark=new THREE.MeshStandardMaterial({color:'#62594b',roughness:.96});
  const leaves=new THREE.MeshStandardMaterial({color:'#507763',roughness:.96});
  materials.push(asphalt,paving,stone,paint,iron,grass,wood,bark,leaves,courtyard);
  materials.forEach(m=>m.userData.outlineWeight=0);
  const surfaces:Array<{name:string;x0:number;x1:number;z0:number;z1:number;y:number}>=[];
  const cube=new THREE.BoxGeometry(1,1,1);
  const rect=(name:string,x0:number,x1:number,z0:number,z1:number,y:number,material:THREE.Material)=>{
    surfaces.push({name,x0,x1,z0,z1,y});
    const geo=new THREE.PlaneGeometry(x1-x0,z1-z0);geo.rotateX(-Math.PI/2);
    const pos=geo.attributes.position,uv=geo.attributes.uv;for(let i=0;i<pos.count;i++)uv.setXY(i,(pos.getX(i)+(x0+x1)/2)/2,(pos.getZ(i)+(z0+z1)/2)/2);
    const mesh=new THREE.Mesh(geo,material);mesh.name=name;mesh.position.set((x0+x1)/2,y,(z0+z1)/2);mesh.receiveShadow=true;group.add(mesh);return mesh;
  };
  const box=(x:number,y:number,z:number,w:number,h:number,d:number,material:THREE.Material)=>{const m=new THREE.Mesh(cube,material);m.position.set(x,y,z);m.scale.set(w,h,d);m.castShadow=true;m.receiveShadow=true;group.add(m);return m;};
  function pavement(x0:number,x1:number,z0:number,z1:number){
    // 退化条带直接丢：地块入口落客带是按「路缘外沿 → 楼体正面」算的，某些
    // 进深下两者会重合（长度 0），零面积的 surface 会污染平面重叠自检。
    if(x1-x0<=.001||z1-z0<=.001)return;
    rect('sidewalk',x0,x1,z0,z1,.028,paving);
    colliders.push({kind:'floor',source:'planned-sidewalk',min:new THREE.Vector3(x0,.008,z0),max:new THREE.Vector3(x1,.048,z1)});
  }
  // Horizontal roads own all junctions; vertical segments end exactly at their boundaries.
  for(const r of CITY_ROADS.eastWest)rect('east-west-road',-80,80,r.z-r.width/2,r.z+r.width/2,0,asphalt);
  for(const r of CITY_ROADS.northSouth){let z0=-78;for(const cross of CITY_ROADS.eastWest){rect('north-south-road',r.x-r.width/2,r.x+r.width/2,z0,cross.z-cross.width/2,0,asphalt);z0=cross.z+cross.width/2;}rect('north-south-road',r.x-r.width/2,r.x+r.width/2,z0,83,0,asphalt);}
  const xCells=[[-80,-50],[-42,39],[47,80]];
  for(let row=0;row<CITY_ROADS.eastWest.length-1;row++){
    const a=CITY_ROADS.eastWest[row],b=CITY_ROADS.eastWest[row+1],z0=a.z+a.width/2,z1=b.z-b.width/2;
    for(let col=0;col<xCells.length;col++){
      const[x0,x1]=xCells[col];const central=col===1&&(row===2||row===3);
      // Existing station pit, apartment and shop entrances retain their exact levels and openings.
      if(central){
        if(row===2)pavement(x0+2.4,x1-2.4,6.3,7.6);
        // 行 3 中央的前场步道要和两侧格对齐成同一条 2.4m 带（12.6~15.0）：
        // 新补的四栋临街体量从 z=15.8 起，它们到路缘的落客带正好接在这条带上。
        if(row===3)pavement(x0+2.4,x1-2.4,12.6,15);
        pavement(x0,x0+2.4,z0,z1);pavement(x1-2.4,x1,z0,z1);
        if(row===2)pavement(x0+2.4,x1-2.4,z0,z0+2.4);
        if(row===3)pavement(x0+2.4,x1-2.4,z1-2.4,z1);
      }else{
        rect('block-courtyard',x0+2.4,x1-2.4,z0+2.4,z1-2.4,-.003,courtyard);
        pavement(x0,x1,z0,z0+2.4);pavement(x0,x1,z1-2.4,z1);
        pavement(x0,x0+2.4,z0+2.4,z1-2.4);pavement(x1-2.4,x1,z0+2.4,z1-2.4);
      }
      // Kerbs stop 4 m before junctions, leaving flush accessible crossing entries.
      for(const z of [z0,z1])for(let x=x0+4;x<x1-4;x+=1.05)box(x,.068,z,.99,.12,.16,stone);
    }
  }
  for(const [x0,x1] of xCells){pavement(x0,x1,64.3,66.7);pavement(x0,x0+2.4,66.7,83);pavement(x1-2.4,x1,66.7,83);}
  for(const lot of CITY_LOTS){
    const road=[...CITY_ROADS.eastWest].sort((a,b)=>Math.abs(a.z-lot.z)-Math.abs(b.z-lot.z))[0];
    const sign=Math.sign(lot.z-road.z),streetEdge=road.z+sign*(road.width/2+2.4),entry=lot.z-sign*lot.d/2;
    pavement(lot.x-.8,lot.x+.8,Math.min(streetEdge,entry),Math.max(streetEdge,entry));
  }
  // Crossings and centerlines follow the street axis, with no paint laid through junction centers.
  for(const r of CITY_ROADS.eastWest){
    for(let x=-77;x<79;x+=4.5)if(CITY_ROADS.northSouth.every(c=>Math.abs(x-c.x)>8))rect('lane-mark',x,x+2,r.z-.045,r.z+.045,.006,paint);
    for(const cross of CITY_ROADS.northSouth)for(const side of [-1,1]){
      const cx=cross.x+side*7;
      for(let z=r.z-r.width/2+.35;z<r.z+r.width/2-.25;z+=.75)rect('crosswalk',cx-1.2,cx+1.2,z,z+.38,.007,paint);
      for(const edge of [-1,1])rect('tactile-threshold',cx-1.15,cx+1.15,r.z+edge*(r.width/2+.4)-.16,r.z+edge*(r.width/2+.4)+.16,.035,stone);
    }
  }
  for(const road of CITY_ROADS.northSouth){
    for(let z=-76;z<81;z+=4.5)if(CITY_ROADS.eastWest.every(c=>Math.abs(z-c.z)>c.width/2+4))rect('lane-mark',road.x-.045,road.x+.045,z,z+2,.006,paint);
    for(const cross of CITY_ROADS.eastWest)for(const side of [-1,1])for(let x=road.x-3.6;x<road.x+3.5;x+=.75){const z=cross.z+side*(cross.width/2+2);rect('crosswalk',x,x+.38,z-1.2,z+1.2,.007,paint);}
  }
  // Minor-street stop lines sit behind the zebra crossings, in each approaching lane.
  for(const street of CITY_ROADS.eastWest)for(const avenue of CITY_ROADS.northSouth){
    rect('stop-line',avenue.x-9.1,avenue.x-8.85,street.z-street.width/2+.2,street.z-.15,.008,paint);
    rect('stop-line',avenue.x+8.85,avenue.x+9.1,street.z+.15,street.z+street.width/2-.2,.008,paint);
  }
  // The shop and station get direct crossings on the existing slow street.
  for(const x of [0,-24])for(let z=7.95;z<12.4;z+=.75)rect('frontage-crosswalk',x-1.2,x+1.2,z,z+.38,.007,paint);
  for(const road of CITY_ROADS.eastWest)for(let x=-73;x<78;x+=12){
    if(CITY_ROADS.northSouth.some(c=>Math.abs(x-c.x)<7))continue;
    for(const side of [-1,1]){const z=road.z+side*(road.width/2-.22);box(x,.012,z,.7,.022,.3,iron);for(let n=0;n<7;n++)box(x-.28+n*.09,.027,z,.023,.012,.25,stone);}
  }
  // Neighborhood green: public path connects the service road to the southern street.
  // 绿地收窄到 ±9：行 4 中央的 27m 空档两侧各补了一栋 4.5m 面宽的窄楼
  // （x=±11.25），绿地再占 ±10 就会被压到楼底下。
  rect('community-green',-9,9,36,55,.015,grass);pavement(-1.4,1.4,35.7,55.3);pavement(-9,-1.4,44.2,46.2);pavement(1.4,9,44.2,46.2);
  const crown=new THREE.SphereGeometry(1,10,7),trunk=new THREE.CylinderGeometry(.09,.14,2,8);
  function tree(x:number,z:number){
    box(x,.12,z, .7,.20,.7,stone);box(x,.235,z,.58,.035,.58,grass);
    const t=new THREE.Mesh(trunk,bark);t.position.set(x,1.22,z);group.add(t);
    for(let n=0;n<5;n++){const m=new THREE.Mesh(crown,leaves);m.position.set(x+Math.sin(n*2.4)*.55,2.65+(n%2)*.45,z+Math.cos(n*2.4)*.5);m.scale.set(.83,.9,.75);m.castShadow=true;group.add(m);}
    colliders.push({source:'street-tree',min:new THREE.Vector3(x-.35,0,z-.35),max:new THREE.Vector3(x+.35,.9,z+.35)});
  }
  for(const x of [-7,7])for(const z of [38.5,51.5])tree(x,z);
  for(const r of CITY_ROADS.northSouth)for(let z=-61;z<78;z+=12)if(CITY_ROADS.eastWest.every(c=>Math.abs(z-c.z)>7))for(const s of [-1,1])tree(r.x+s*4.5,z);
  for(const x of [-5,5])for(const z of [42,49]){box(x,.47,z,2,.09,.5,wood);box(x,.85,z+.22,2,.65,.06,wood);for(const dx of [-.7,.7])box(x+dx,.24,z,.08,.48,.42,iron);colliders.push({source:'park-bench',min:new THREE.Vector3(x-1,0,z-.25),max:new THREE.Vector3(x+1,1.18,z+.26)});}
  const lantern=new THREE.MeshStandardMaterial({color:'#ffe2b1',emissive:'#ffd299',emissiveIntensity:1.3});materials.push(lantern);
  for(const road of CITY_ROADS.northSouth)for(let z=-61;z<79;z+=18){
    if(CITY_ROADS.eastWest.some(r=>Math.abs(z-r.z)<7))continue;
    for(const side of [-1,1]){const x=road.x+side*4.4;box(x,2.1,z,.085,4.2,.085,iron);box(x-side*.4,4.16,z,.88,.08,.10,iron);box(x-side*.78,4.07,z,.32,.09,.18,lantern);}
  }
  // Low bollards define park entries while leaving the central path clear.
  for(const z of [34.1,56.9])for(const x of [-1.1,1.1])box(x,.38,z,.12,.76,.12,iron);
  group.userData.plan={roads:CITY_ROADS,lots:CITY_LOTS,surfaces};
  mergeByMaterial(group);
  return{asphalt,paving,stone,colliders,
    setWet(wet:boolean){asphalt.roughness=wet?.48:.94;paving.roughness=wet?.63:.9;},
    dispose(){textures.forEach(t=>t.dispose());materials.forEach(m=>m.dispose());}
  };
}
