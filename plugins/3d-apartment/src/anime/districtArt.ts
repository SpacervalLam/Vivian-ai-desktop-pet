import { buildUrbanStreets, CITY_LOTS, CITY_ROADS, CITY_INBLOCK_TOWER_LOTS } from './urbanStreets';
import { asphaltTexture, plazaStoneTexture, sidewalkTexture, curbTexture } from './toon';
import type { Collider } from './collider';
import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { mergeByMaterial } from './merge';
import { createSakuraTown } from './sakuraTown';
import { createSakuraStation } from './sakuraStation';
import { createCivicQuarter } from './civicQuarter';
import { loadBlenderVehicles } from './blenderVehicles';
import { loadBlenderTrunks } from './blenderTrunks';
import { createGlobalSquare } from './globalSquare';
import { createWorldStreetLife } from './worldStreetLife';
import { createUrbanInfill } from './urbanInfill';
import { createCityDressing } from './cityDressing';
import { createStreetTraffic } from './streetTraffic';
/* 公寓的层标高（单点真相，见 exterior.ts 里那个 export 的注释）。 */
import { FLOORS } from './exterior';
/* 北侧小河的 z 范围，远景得让开它。 */
import { RIVER_Z_N, RIVER_Z_S } from './river';

/**
 * 低模楼的层高：**取公寓那一套**，不再自己写一个 2.8。
 *
 * 原版这里是 `floors*2.8` 的一刀切——首层也只有 2.8。而公寓是
 * 「3.4m 首层 + 2.8m 标准层」（exterior.ts 的 F2 / LVL，FLOORS = [3.4, 6.2, 9.0]）。
 * 于是同一条街上两种楼的"一层"对不上：低模楼的首层比公寓矮 0.6m（21%），
 * 窗洞又只有 1.5m（公寓次窗 1.76m、主窗 2.31m）——低模楼的每一层整体
 * 都比公寓矮一截、密一截，远看就是两套层高拼在一起。
 *
 * 这里按 FLOORS 派生而不是各写一份常量：FLOORS 的注释写明它是"单点真相"，
 * 就是为了避免这种标高漂移。派生之后两栋楼不可能再各自跑偏。
 */
const LEVEL0 = FLOORS[0];              // 3.4  首层层高（= 公寓 F2）
const LEVEL = FLOORS[1] - FLOORS[0];   // 2.8  标准层层高（= 公寓 LVL）
/** 第 f 层（f=0 为首层）的楼面标高。 */
const levelY = (f: number) => f === 0 ? 0 : LEVEL0 + (f - 1) * LEVEL;
/** 第 f 层（f=0 为首层）的层高。 */
const levelH = (f: number) => f === 0 ? LEVEL0 : LEVEL;
/**
 * 窗洞尺寸（米）。
 *
 * 原版只有一个 1.5m 高的方洞，占 2.8m 层高的 54%；公寓的次窗 1.76m（63%）、
 * 主窗 2.31m（82%）。在低模体量上，**窗高是"一层多高"最强的视觉代理**：
 * 窗小，层就显矮。这里照公寓的次窗取值——低模楼本来就该比公寓简化一档，
 * 取次窗而不是主窗（落地窗），读起来才还是"普通的楼"而不是住宅精装立面。
 *
 * WIN_HEAD 是窗顶离本层顶面的距离。公寓的窗顶也在这一带（次窗 2.23、
 * 主窗 2.38，都在层顶下 0.4~0.6），所以窗顶对齐层顶而不是对齐楼面——
 * 层高变了（首层 3.4）窗才跟着变，而不是被拉长。
 */
const WIN_H = 1.76, WIN_HEAD = 0.40;
/** 侧墙窗：比正立面窗矮一档，保持原版 1.25/1.5 的比例。 */
const SIDE_WIN_H = 1.46;

/**
 * 铁路走廊的退界（世界 x）。
 *
 * 从函数里提出来是因为**用它的不止远景墙**：riverbank 铺堤岸时也要在这 62m 里让开
 * （那段是桥和路堤的地盘）。取值为什么是 [74,106] 而不是绿化带的 [80,100]，理由写
 * 在 createDistrictArt 里 ringFits 上方那段长注释——"几何不相交"不等于"看起来不在
 * 轨道上"。改这里必须同步改那边。
 */
export const TRACK_GAP = [74, 106] as const;

/** District rendering and authored collision geometry share the same street plan. */
export function createDistrictArt(scene: THREE.Scene) {
  const streets = buildUrbanStreets(scene);
  const sakuraTown = createSakuraTown(scene);
  const sakuraStation = createSakuraStation(scene);
  const civicQuarter = createCivicQuarter(scene);
  /* 南区东：欧式广场 + 中华街（稀释日本元素占比，见 globalSquare.ts 文件头）。
   *
   * **必须建在这里**（在 `colliders` 组装之前、`cityDressing` 之前）：它的墙盒
   * 要进那份表，`cityDressing` 的落位过滤器才认它是禁区，"扫街绿化"那一段才
   * 不会往广场中间、喷泉上、骑楼券廊里种树。 */
  const globalSquare = createGlobalSquare(scene);
  const worldStreetLife = createWorldStreetLife(scene);
  const colliders: Collider[] = [...streets.colliders, ...sakuraTown.colliders, ...sakuraStation.colliders, ...civicQuarter.colliders, ...globalSquare.colliders, ...worldStreetLife.colliders];
  const converted = new Map<THREE.Material, THREE.Material>();
  const bevels = new Map<string, THREE.BufferGeometry>();
  const glow = new THREE.MeshStandardMaterial({color:'#ffe1b0',emissive:'#ffbe72',emissiveIntensity:1.6,roughness:.65});
  const darkGlass = new THREE.MeshStandardMaterial({color:'#263d4b',metalness:.3,roughness:.3});
  /**
   * 石材色阶。
   *
   * 旧版是 5 色 + `stone[index % 5]`，而每行恰好 8 列 —— 周期 5 在 8 列上必然
   * 回卷，于是第 1&6、2&7、3&8 列**永远同色**，且每一行都是同一序列的循环移位。
   * 这不是"随机不好看"，是结构性的：相邻 block 隔街对望的两栋永远撞色。
   * 现在扩到 10 阶，并按乘法散列取色（见 hash 的注释）。
   */
  const stone = ['#88999d','#71878d','#a2aaa6','#667c88','#998f87',
                 '#7d8f96','#93a09b','#6b8189','#a4aaa4','#5f7681']
    .map(color=>new THREE.MeshStandardMaterial({color,roughness:.91}));
  /**
   * 最外一圈（只留长方体的那批）的石材。
   *
   * 为什么不直接用上面的 `stone`：那批的明暗是靠**立面细节**撑起来的——窗洞、
   * 腰线、女儿墙各自吃光不同，才有体积感。长方体什么细节都没有，同一个材质贴上去
   * 只会读成一块平板。所以这里单独取一组，色相仍是内圈那套冷灰蓝（保持是同一座
   * 城市），但**带一点自发光**：夜里内圈靠窗灯亮起来，外圈没有窗，不给自发光就会
   * 整片塌成黑剪影，远景反而比白天更空。
   *
   * 强度取 0.10：自发光是**常量**，白天太阳的漫反射比它大一个量级（±0.01 线性值，
   * 看不出来），夜里场景一暗它就顶上来，外圈于是读成"远处那片还亮着的城"，
   * 而不是一圈黑剪影。
   *
   * 0.05 是第一版，实测偏弱：夜帧（tmp/skyline2-shots/S11-night-north）里外圈只剩
   * 一条比天空略亮的窄带，几乎读不出体量，所以翻倍。
   */
  const outerStone = ['#7d8f96','#6b8189','#8a9aa0','#5f7681','#93a09b','#74858c']
    .map(color=>new THREE.MeshStandardMaterial({color,roughness:.95,emissive:color,emissiveIntensity:.10}));
  const trim = new THREE.MeshStandardMaterial({color:'#3b525b',roughness:.7,metalness:.25});
  const roof = new THREE.MeshStandardMaterial({color:'#556765',roughness:.95});
  const balconyMetal = new THREE.MeshStandardMaterial({color:'#263d43',roughness:.62,metalness:.28});
  const cedarWood = new THREE.MeshStandardMaterial({color:'#72523e',roughness:.9});
  const planterGreen = new THREE.MeshStandardMaterial({color:'#64866a',roughness:.94});


  /**
   * 描边分级（见 toon.ts 的 outlineWeightOf）。
   *
   * 只有**体量**（石材/线脚/屋顶板）描边；玻璃和发光件一律置 0：
   *  - 窗格只有 0.025m 厚，外扩壳比它本身还厚，描了会糊成一坨；
   *  - 发光体描边会把 bloom 的光晕闷死在黑壳里。
   * 远景保持无线稿；近景商店街由 sakuraTown 独立创建并合批。
   */
  for(const m of [...stone,trim,roof,balconyMetal,cedarWood])m.userData.outlineWeight=2;
  planterGreen.userData.outlineWeight=0;
  darkGlass.userData.outlineWeight=0;
  glow.userData.outlineWeight=0;
  const district = new THREE.Group(); district.name='authored-city-district'; district.userData.sceneCollideSkip=true;
  scene.add(district);
  let seed=2167; const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  /** 确定性散列，取乘法散列的高位（低位没混匀，直接取模会退化成等差数列）。 */
  const hash=(i:number,salt:number)=>(Math.imul(i+salt,2654435761)>>>8);
  const cube=new THREE.BoxGeometry(1,1,1);
  const foliage=new THREE.SphereGeometry(1,8,6);
  const detailMaterials:THREE.Material[]=[balconyMetal,cedarWood,planterGreen];
  function box(parent:THREE.Group,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material){
    const mesh=new THREE.Mesh(cube,m);mesh.position.set(x,y,z);mesh.scale.set(w,h,d);mesh.receiveShadow=true;parent.add(mesh);return mesh;
  }
  /**
   * 一栋低模楼。
   *
   * `opts` 只有远景围合墙用得上：
   *
   *  - `rotY`：整栋楼绕自己转多少弧度。东西两面墙的长立面是 ±x，而这里按 ±z 建
   *    正立面（窗 + 阳台），不转的话朝着街区的那一面恰好是最简的侧墙。
   *  - `front`：正立面在 ±z 的哪一侧。原本按最近的東西向马路推（`sign(street.z-z)`），
   *    远景那批离马路几百米，这个推导对它们没有意义，会让同一面墙上的一半楼朝里、
   *    一半朝外。所以让调用方直接指定。
   *  - `far`：远景简化（判据见下面窗循环里的注释）。
   *
   * 旋转不是重写坐标：整栋楼照旧按**绝对坐标**建，只是装进一个
   * 「先平移回原点 → 旋转 → 再平移回 (x,z)」的三步复合，旋转中心落在楼自己身上。
   * 合批会把 matrixWorld 烘进顶点（见 merge.ts），这层组不留运行时代价；
   * 名字挂在 pivot 上——merge.ts 的 mergedLabel 靠"最近的具名祖先"分类。
   */
  function building(parent:THREE.Group,x:number,z:number,w:number,d:number,floors:number,index:number,opts?:{rotY?:number;front?:number;far?:boolean}){
    const pivot=new THREE.Group();pivot.name=`city-block-${index}`;parent.add(pivot);
    pivot.position.set(x,0,z);pivot.rotation.y=opts?.rotY??0;
    const g=new THREE.Group();g.position.set(-x,0,-z);pivot.add(g);
    const far=opts?.far===true;
    const h=levelY(floors);const m=stone[hash(index,1)%stone.length];
    /* 基座外挑从 +0.15 提到 +0.18：窗玻璃的外表面在墙外 0.0835（pz + 0.046 + 0.0125），
     * 而 +0.15 的基座只挑出 0.075 —— 首层窗的下沿会从基座面里戳出来一条 8.5mm 的亮边。
     * 提到 +0.18（挑出 0.09）刚好把玻璃和窗框都罩住，仍在线脚 +0.22 之内。 */
    box(g,x,h/2,z,w,h,d,m);box(g,x,.65,z,w+.18,1.3,d+.18,trim);
    /* 各层腰线压在**每层楼面**上（不是等距排）：首层 3.4m，上面每层 2.8m，
     * 所以头两条腰线之间比后面几条宽 0.6m —— 公寓的立面就是这个节奏。 */
    for(let f=1;f<=floors;f++) box(g,x,levelY(f)-.06,z,w+.22,.14,d+.22,m);
    box(g,x,h+.18,z,w+.45,.36,d+.45,trim);
    box(g,x,h+.39,z,w-.4,.15,d-.4,roof);
    const crownH=2.2+random()*2;
    box(g,x+w*.14,h+crownH/2,z-d*.12,w*.46,crownH,d*.48,m);
    box(g,x+w*.14,h+crownH+.12,z-d*.12,w*.49,.24,d*.51,trim);
    for(let k=0;k<3;k++){
      box(g,x-w*.28+k*.9,h+.65,z+d*.25,.65,.5,.8,stone[2]);
      box(g,x-w*.28+k*.9,h+.66,z+d*.25+.405,.45,.25,.025,trim);
    }
    const cols=Math.max(2,Math.floor(w/1.65)),rows=floors;
    const street=[...CITY_ROADS.eastWest].sort((a,b)=>Math.abs(a.z-z)-Math.abs(b.z-z))[0];
    const side=opts?.front??Math.sign(street.z-z),front=z+side*d/2;
    for(let f=0;f<rows;f++)for(let c=0;c<cols;c++){
      /* 本层楼面 base / 层高 H。
       *  - 窗（框/玻璃/中挺）按"窗顶 = 层顶 - WIN_HEAD"定位，跟着层高走；
       *  - 阳台板、栏杆、花箱、窗下外机一律按 `base + 常量` 定位——它们贴的是
       *    楼面，不是窗。原版这些都写成 `py - 常量`（因为 py 恰好等于 base+1.65），
       *    窗一加高、py 一降，阳台板就会整体跟着掉进窗洞里，所以必须拆开。 */
      const base=levelY(f),py=base+levelH(f)-WIN_HEAD-WIN_H/2;
      /* 窗框下沿（= 整扇窗的下沿）。
       * 窗台和窗下外机都必须挂在这个标高上，**不能挂 `py`（窗中心）**，
       * 也不能挂 `base + 常量`：前者会跑到窗子正中，后者跟不动层高——
       * 首层 3.4m、标准层 2.8m，洞口下沿到楼面的距离差 0.6m，常量是跟不动的。 */
      const frameBot=py-WIN_H/2;
      const px=x-w/2+(c+.5)*w/cols;
      for(const side of [-1,1]){
        const pz=z+side*(d/2+.025),lit=random()>.58;
        box(g,px,py,pz,1.05,WIN_H,.07,trim);
        box(g,px,py,pz+side*.046,.86,WIN_H-.23,.025,lit?glow:darkGlass);
        /* ── 远景简化 ──────────────────────────────────────────────────────
         * 下面这几处（中挺、外挑窗台、阳台竖栏、花箱叶簇、侧墙外机百叶 + 落水管）
         * 只给**街区里**的楼。判据是**屏幕尺寸**，不是距离：最近的一面远景墙离可活动
         * 范围也有 34m，0.035~0.055m 的零件在那里只占 1~2 像素——留着是往锯齿里加噪，
         * 却要多花近 2/3 的顶点（实测一面墙 444/721 个盒）。窗框、玻璃、窗下外机、
         * 阳台板 + 扶手全部保留：楼在远处仍然读得出是"带阳台的住宅楼"而不是一块光板。
         * ──────────────────────────────────────────────────────────────── */
        if(!far)box(g,px,py,pz+side*.065,.045,WIN_H-.20,.025,trim);
        /* 窗下冷凝机：顶面顶在**窗台底面**下 5mm。
         * 改前写的是 `base + .52`：标准层恰好凑上（间隙 0.075），首层就悬空了
         * （间隙 0.675）——因为首层层高 3.4 而标准层 2.8，常量跟不动洞口下沿。 */
        box(g,px,frameBot-.085,pz+side*.12,1.2,.09,.26,m);
        /* 外挑窗台：**顶面与窗框下沿齐平**。
         * 这是公寓 windowUnit 的规则（exterior.ts:1697「台面中心 = 洞口下沿 - 框宽 + 0.01」，
         * 即台面顶 = 洞口下沿；低模楼的窗框是一块实心板，它的下沿就是整扇窗的下沿）。
         * 改前这里写的是 `py`（窗中心）——一条 0.035 的横带就横在 1.76m 窗子的正中，
         * 远看是一道横梃，不是窗台。 */
        if(!far)box(g,px,frameBot-.0175,pz+side*.083,.88,.035,.03,trim);
        // Small balcony: slab + handrail, with vertical rails and a planter up close.
        if(Math.abs(pz-front)<.04 && f>0){
          box(g,px,base+1.01,pz+side*.22,1.13,.07,.32,balconyMetal);
          box(g,px,base+1.37,pz+side*.36,1.13,.055,.055,balconyMetal);
          if(!far)for(let rail=-4;rail<=4;rail++)box(g,px+rail*.125,base+1.19,pz+side*.36,.035,.36,.04,balconyMetal);
          if((c+f+index)%3===0){
            box(g,px,base+1.11,pz+side*.25,.54,.12,.28,cedarWood);
            if(!far)for(let leaf=0;leaf<3;leaf++)box(g,px-.17+leaf*.17,base+1.26,pz+side*.25,.11,.2,.12,planterGreen);
          }
        }
      }
    }
    // Boxy condenser units, drains and service conduits break up blank side walls.
    for(let f=0;f<floors;f++){
      const ay=levelY(f)+.48;
      for(const edge of [-1,1]){
        const ax=x+edge*(w/2+.12),az=z+side*.35;
        if((index+f+edge)%2===0){
          box(g,ax,ay,az,.34,.28,.44,trim);
          if(!far){
            box(g,ax,ay-.13,az+side*.24,.22,.025,.025,balconyMetal);
            for(let vent=0;vent<3;vent++)box(g,ax-.09+vent*.09,ay,az+side*.23,.025,.16,.02,balconyMetal);
          }
        }
      }
      if(!far)box(g,x-w/2-.07,ay,z+side*(d/2+.07),.055,.055,.055,cedarWood);
    }
    /* 首层店面：门 / 雨篷 / 灯箱 / 立牌。
     * 原版的 y（1.08 / 2.5 / 2.435 / 1.02）是照 2.8m 首层写死的，首层加高到 3.4
     * 之后不跟着走，雨篷就会掉在门和窗中间。定位规则与公寓同一套：
     *   雨篷压在窗顶之上（窗顶 = 层顶 - WIN_HEAD），门顶离雨篷底 0.36。
     * 代入 3.4m 首层：窗顶 3.00、雨篷 3.10、门高 2.74；代入 2.8m 时
     * 正好回到原版的 2.50 / 2.12 —— 也就是这组式子在旧层高下是恒等的。 */
    const canopyY=LEVEL0-WIN_HEAD+.10,doorH=canopyY-.36;
    box(g,x,doorH/2+.02,front+side*.12,1.28,doorH,.08,darkGlass);
    box(g,x,canopyY,front+side*.45,2.0,.10,.90,trim);
    box(g,x,canopyY-.065,front+side*.45,1.5,.02,.45,glow);
    box(g,x+.46,1.24,front+side*.18,.035,.44,.035,stone[2]);
    // Side windows prevent blank silhouettes from oblique apartment views.
    for(let f=0;f<rows;f++)for(let c=0;c<Math.floor(d/2);c++)for(const side of [-1,1])
      box(g,x+side*(w/2+.03),levelY(f)+levelH(f)-WIN_HEAD-SIDE_WIN_H/2,z-d/2+1+c*2,.045,SIDE_WIN_H,.9,random()>.66?glow:darkGlass);
  }
  /**
   * 玩家的可活动范围（米）。
   *
   * 街区边界环、远景围合墙、navWorld 的 `street` 层都以它为基准，所以先在这里声明。
   * 取值与 navWorld 的 street 层 clip **完全一致**（东扩的车站地坪 x[80,150]
   * z[-45,95] 也在里面）——两边同源，「NPC 走得到的地方玩家也走得到」这条不变量
   * 才成立，不会出现宠物能去、玩家却撞上空气墙的裂缝。
   */
  const WALK_BOUNDS={x0:-80,x1:150,z0:-78,z1:165};
  /**
   * 塔楼 footprint 的内缩量（米）。
   *
   * 地块表里同一排的地块只隔 9m，而地块面宽就是 9m——照地块尺寸直接盖，
   * 相邻两栋的主体墙面**恰好共面**（z-fighting），外挑线脚（+0.45）还会互相插进去。
   * 内缩 0.8m 后相邻面之间留出 0.8m，线脚外挑完还剩 0.35m，读起来就是日式街区里
   * 那种窄巷，两个面也不会打架。远景那批本来 18m 间距，不需要缩。
   */
  const TOWER_INSET=.8;
  /**
   * 街区内高楼的落位记录（与下面的 ringPlan 同一套路）。
   *
   * 合批之后单栋楼不可寻址——mergeByMaterial 会把整组按材质压平，一个网格里装着
   * 几十栋楼的几何。要对着"某一栋的正立面"取证（窗台高度、层高节奏这类），
   * 就只能把落位单独发出来。只读，不参与渲染。
   */
  const towerPlan:{id:string;x:number;z:number;w:number;d:number;floors:number;front:number}[]=[];
  scene.userData.towerPlan=towerPlan;
  CITY_LOTS.forEach((lot,index)=>{
    // Near lots have real shop interiors and explicit wall/doorway colliders.
    if(lot.id.endsWith('-19.6'))return; // Replaced by four furnished, walk-in shops in sakuraTown.
    /**
     * 街区内的高楼：**就地**盖在该地块上（走位/朝向全交给 building() 自己按最近的
     * 东西向马路算 side/front）。哪些地块盖高楼由 urbanStreets 的
     * CITY_INBLOCK_TOWER_LOTS 决定——sakuraTown 读同一份集合、跳过这些地块，
     * 一块地只会有一个主人。
     */
    if(!CITY_INBLOCK_TOWER_LOTS.has(lot.id))return;
    const x=lot.x,z=lot.z,w=lot.w-TOWER_INSET,d=lot.d-TOWER_INSET;
    building(district,x,z,w,d,lot.floors,index);
    const street=[...CITY_ROADS.eastWest].sort((a,b)=>Math.abs(a.z-z)-Math.abs(b.z-z))[0];
    towerPlan.push({id:lot.id,x,z,w,d,floors:lot.floors,front:Math.sign(street.z-z)});
    /* 碰撞盒顶面必须跟主体同高。原来写的是 lot.floors*2.8（= 旧的一刀切层高），
     * 层高改成「3.4 + 2.8×(n-1)」后不改这里，盒顶会比楼顶低 0.6m——
     * 站在楼边抬头，头顶最后 0.6m 是能穿过去的。 */
    colliders.push({source:lot.id,min:new THREE.Vector3(x-w/2-.12,0,z-d/2-.12),max:new THREE.Vector3(x+w/2+.12,levelY(lot.floors),z+d/2+.12)});
  });
  mergeByMaterial(district);
  const urbanInfill=createUrbanInfill(scene,colliders);
  colliders.push(...urbanInfill.colliders);
  /* 站前东区 / 沿街陈设 / 西缘服务带。
   *
   * 放在 urbanInfill 之后是有意的：陈设的落位过滤器吃的是"到目前为止的全部碰撞盒"，
   * 排在最后才能把 infill 那批房子也当成禁区，不会把垃圾桶插进人家院里。 */
  const cityDressing=createCityDressing(scene,colliders);
  colliders.push(...cityDressing.colliders);

  /* 动态车流。**放在最后建**：车身组每帧都在动，一旦被 `mergeByMaterial` 扫到
   * 就会被把变换烘进顶点、再也挪不动（第 298 行那次合批的对象是 `district`，
   * 而车流挂在 scene 上，所以现在是安全的 —— 但别把它挪到 298 之前去）。
   * 它不落位、不吃 `free()` 判据，所以 `colliders` 也不进（恒为空数组）。 */
  const streetTraffic=createStreetTraffic(scene);
  const disposeBlenderVehicles=loadBlenderVehicles(scene);
  /* 树干几何同理：Blender 资产异步到位，载入前各场用程序化回退顶着。
   * **必须排在所有建树模块之后** —— 各场是在自己模块里 build 的，这里只是把
   * GLB 几何派发下去（见 blenderTrunks 文件头）。 */
  const disposeBlenderTrunks=loadBlenderTrunks();

  /**
   * 远景低模楼：**四面围合**。
   *
   * 原版只铺北面一行公式：`x = -110+(index%16)*18, z = -112-floor(index/16)*22`。
   * 于是从街区里往北看是三层天际线，往南 / 东 / 西看都是一张空沥青一直铺到地平线，
   * 夜里那三个方向连一盏窗灯都没有（取证见 tmp/ring-shots/R1..R4）。
   *
   * 落位改成「到 WALK_BOUNDS 外扩多少米」，不写死坐标：
   *   行距 34 / 56 / 78m（间隔 22），柱距 18m，每面墙 3 排 × 16 列。
   * 北面代入 WALK_BOUNDS 得 z = -112 / -134 / -156、x = -110 + i·18 —— 与旧坐标
   * **逐位相同**，那一面是验收过的样子，落位一个点都不动。
   *
   * 每面墙**单独成组、单独合批**，两个理由：
   *  1. 四面的包围球互相独立 → 看向北面时南墙整面被视锥剔掉，远景的顶点开销因此
   *     只有"全部铺开"的 1/4 左右（远景本来就大半在雾里）；
   *  2. 可以逐面 `visible=false` 单独核账（见 tmp/_probe-ring.mjs）。
   *
   * 四面的 `front` 一律指向街区：北墙 +z（原样）、南墙 -z、西墙 +x、东墙 -x。
   * 东西两墙的 rotY 就是把"正立面在 +z"转到 ±x（推导见 building 的注释）。
   */
  const RING_ROWS=3,RING_COLS=16,RING_ROW_STEP=22,RING_COL_STEP=18;
  const RING_X0=WALK_BOUNDS.x0-30;   // -110：与旧公式同源
  const RING_Z0=WALK_BOUNDS.z0-34;   // -112
  const ringWalls:[string,number,number,(k:number,i:number)=>[number,number]][]=[
    ['skyline-n',0,1,(k,i)=>[RING_X0+i*RING_COL_STEP,RING_Z0-k*RING_ROW_STEP]],
    ['skyline-s',0,-1,(k,i)=>[RING_X0+i*RING_COL_STEP,WALK_BOUNDS.z1+34+k*RING_ROW_STEP]],
    ['skyline-w',Math.PI/2,1,(k,i)=>[WALK_BOUNDS.x0-34-k*RING_ROW_STEP,RING_Z0+RING_COL_STEP+i*RING_COL_STEP]],
    ['skyline-e',-Math.PI/2,1,(k,i)=>[WALK_BOUNDS.x1+34+k*RING_ROW_STEP,RING_Z0+RING_COL_STEP+i*RING_COL_STEP]],
  ];
  /**
   * 已经落位的远景 footprint（含线脚外挑 0.5m 的余量），用来挡掉四面墙在**转角**处的
   * 互穿。南北墙的列距是 18、东西墙的排距也是 18，但两组网格相位差 4m
   * （x = -110+18i 对 x = -114-22k），角上必然有几栋会插在一起。
   * 与其手工挪坐标，不如按"先到先得"落位——远景只在雾里露个剪影，
   * 角上少一栋，好过两栋互穿。
   */
  /**
   * 铁路走廊：远景围合墙**必须在这里让开**。
   *
   * 轨道从 z=-440 一直铺到 +440（sakuraStation 的 TRACK_HALF），也就是说它从北面
   * 和南面那两圈低模楼里**穿过去**。围合墙是按 18m 柱距排的，格点 x = -110+18i
   * 里 i=11 落在 x=88 —— 楼面宽 9m、footprint 恰好 [83,93]，正骑在两条钢轨
   * （x 85.26~91.74）上，等于在铁路上盖了一栋八层楼。
   *
   * 修法是"让开"，不是"挪一挪"：18m 的柱距配 20m 宽的走廊，无论相位怎么平移都
   * 躲不掉（平移 9m 就换成 x=97 落在走廊里）。
   *
   * **宽度是加过一轮的，这一轮才是对的。**
   * 第一版取 [80,100]，也就是绿化带本身的宽度（sakuraStation 里
   * `box(90,-.045,cz,20,.1,cd,M.grass)` 的 x∈[80,100]）。那版**几何上是干净的**
   * ——四道核账全过（算术交集 0、ringPlan 交集 0、钢轨射线全中 0.51、逐顶点扫
   * 无实体），但**视觉上不干净**：i=12 落在 x=106、footprint [101.5,110.5]，
   * 离走廊边缘只剩 1.5m。从道口贴着轨面朝北看（tmp/track-shots/T3），那栋楼的
   * 剪影直接压进钢轨的透视汇聚线里，读起来就是"楼盖在轨道上"。
   *
   * 教训：**"几何不相交"不等于"看起来不在轨道上"**。20m 的走廊在 100m 外的
   * 透视压缩下只剩十几个像素，贴边的楼一定会和轨道糊在一起。判据得按
   * **铁路用地**来留退界，不能按绿化带的宽度。
   *
   * 所以两侧各退 6m，取 [74,106]。代价是 i=10（x=70，footprint [65.5,74.5]）、
   * i=11（x=88，正骑钢轨）、i=12（x=106）三列全被拿掉，南北两面墙各少 6 栋
   * （原来只少 2 栋），铁路处开一个 62m 的口子；这个口子由更外一圈的
   * `skyline-*2`（见下）在后面补上，正好读成"铁道穿过城市、更远处还有一片市中心"。
   *
   * 核账：tmp/_ring-track-overlap.mjs（纯算术，逐栋算交集；实跑输出 内圈挡掉 12 栋、
   * 剩余压轨 0 栋、豁口 x∈[57.00,119.00] 宽 62.00m）+ tmp/_probe-trackclear.mjs
   * （逐顶点扫走廊，报 y > 8 的实体）+ tmp/_probe-rayid.mjs（从道口反打射线，
   * 报第一个非天空命中落点的 x）。
   *
   * 常量本身提到了模块顶层（见上面的 export），riverbank 也要用它让开河堤。
   */
  /**
   * 让开北侧小河的判据（footprint 是否已探进水面）。
   *
   * `CLEAR = 3m` 是跟着 TRACK_GAP 那条教训来的：**"几何不相交"不等于"看起来不在
   * 水里"**。河道只有 14m 宽，从街区看过去整个夹在远景地带里，透视压缩得非常厉害；
   * 一栋贴着水边 1m 立的楼，从这个距离看上去就是"楼泡在河里"。退 3m 才读得干净。
   */
  const RIVER_CLEAR=3;
  const hitsRiver=(f:{z0:number;z1:number})=>f.z1>RIVER_Z_N-RIVER_CLEAR&&f.z0<RIVER_Z_S+RIVER_CLEAR;
  const ringPlaced:{x0:number;x1:number;z0:number;z1:number}[]=[];
  const ringFits=(x:number,z:number,w:number,d:number,rotY:number)=>{
    const axis=Math.abs(Math.cos(rotY))>.5;   // 未旋转 → 面宽 w 沿 x
    const hw=(axis?w:d)/2+.5,hd=(axis?d:w)/2+.5;
    const f={x0:x-hw,x1:x+hw,z0:z-hd,z1:z+hd};
    /* 让开铁路走廊（见 TRACK_GAP）：轨道从这圈楼里穿过去，楼不能压在上面。 */
    if(f.x1>TRACK_GAP[0]&&f.x0<TRACK_GAP[1])return false;
    /* 让开北侧小河：河的东西两端和世界地面一样铺到雾里，所以这是一条**只看 z** 的
     * 带状判据，四面墙都可能压上去。实际被排掉的是东西两墙 i=1 那一列（z=-94）。 */
    if(hitsRiver(f))return false;
    for(const p of ringPlaced)if(f.x0<p.x1&&f.x1>p.x0&&f.z0<p.z1&&f.z1>p.z0)return false;
    ringPlaced.push(f);return true;
  };
  const skyline:THREE.Group[]=[];
  /* 远景合批之后就再也拆不出一栋栋了，落位记录在这里留一份底，给核账脚本
   * （tmp/_probe-ring.mjs）验"四面墙互相不重叠"。挂 scene.userData 而不是返回值，
   * 是为了不经过 RoomScene 再转一手。 */
  scene.userData.ringPlan=ringPlaced;
  for(const [name,rotY,front,at] of ringWalls){
    const wall=new THREE.Group();wall.name=name;wall.userData.sceneCollideSkip=true;scene.add(wall);
    skyline.push(wall);
    CITY_LOTS.forEach((lot,index)=>{
      if(lot.id.endsWith('-19.6'))return;
      const k=Math.floor(index/RING_COLS),i=index%RING_COLS;
      if(k>=RING_ROWS)return;
      const [x,z]=at(k,i);
      if(!ringFits(x,z,lot.w,lot.d,rotY))return;
      /* 远景不登记碰撞盒：玩家的活动范围被边界环限制在街区里，走过去也碰不到，
       * 留着只是永远命中不了的盒，白白占掉第一人称每帧的遍历和 navWorld 的栅格化。 */
      building(wall,x,z,lot.w,lot.d,lot.floors,index,{rotY,front,far:true});
    });
    mergeByMaterial(wall);
  }

  /**
   * 更外一圈：**只留长方体**的都市剪影。
   *
   * 内圈那三排（离街区 34/56/78m）还是"楼"——有线脚、有窗、有女儿墙，因为它们是
   * 天际线的主体、还在能看清立面的距离上。再往外一排就只剩一个体量了：这个距离上
   * 一扇窗已经小于一个像素，画窗纯属浪费。所以"都市感"只能靠**体量的排布和尺度差**
   * 来给，不能靠立面细节。
   *
   * 三条设计线：
   *
   *  1. **高度差要狠。** 基底 14~28 层，再叠 4 个"市中心核"（按距离线性衰减的加成，
   *     实际最多 +22 层），叠加后封顶 50 层；另有约 1/17 的概率直接起一栋 52~64 层的
   *     地标（146~180m）。天际线的高低差是"像不像城市"的第一因素——等高的盒子排一圈
   *     只会读成货架。**基底为什么是 14 而不是 5，见 outFloors 的注释（取证逼出来的）。**
   *  2. **体量要换形。** 6 种 footprint 按散列取，长宽比在 0.58~1.71 之间；
   *     远近叠在一起，疏密才不匀质。
   *  3. **要留空。** 1/5 的格点直接跳过（当街口 / 公园）。不留空的话四面就是一堵
   *     几十米高的实墙，连"城市的边界"都读不出来。
   *
   * 几何上比内圈简单一档：**不建 pivot、不做 rotY**。长方体没有正立面可言，
   * 东西两面墙只要把面宽/进深对调就行，省掉整层"平移回原点→旋转→平移回去"。
   *
   * 走廊同样让开（见 TRACK_GAP）。轨道铺到 ±440，这几排的 z 全在范围内，
   * 所以这一条对南北两面墙是真的起作用，不是走形式。
   */
  const OUT_ROWS=3,OUT_ROW_STEP=38,OUT_BASE=86,OUT_COL_STEP=30,OUT_COLS=13;
  const OUT_X0=WALK_BOUNDS.x0-30;    // -110，与内圈同源
  const OUT_Z0=WALK_BOUNDS.z0-34;    // -112
  /* 面宽 × 进深候选。上限 24×22，配 30/38 的格距，相邻最小净距 6m —— 网格本身
   * 就保证了不互穿，所以这里**不需要**内圈 ringFits 那套逐对求交。 */
  const OUT_FOOT:readonly[number,number][]=[[22,16],[16,16],[24,14],[14,22],[20,18],[18,12]];
  /* 四个"市中心核"（世界坐标 x,z,半径,层数加成）。只有径向簇的话等高线是同心圆，
   * 从天上看很假；四个偏心核半径不一，天际线才有几个能认出来的高点。
   *
   * 加成值从 28/26/24/28 降到 26/24/22/26 是**算过之后**改的：四个核的圆心
   * （-165,-240 等）并不落在 30/38m 的格点上，最近的格点离圆心 55~69m，
   * 所以实际拿到的是 `boost*(1-d/r)`，名义 28 实际最多只有 22。降下来是为了给
   * 下面的基底腾出空间，不然非地标会被上限削平——一削平就又变成一排等高的盒子。
   * 核实际贡献的加成见 tmp/_out-hist2.mjs 的输出（0,1,2,…,18,20,22）。 */
  const OUT_CORES:readonly[number,number,number,number][]=[
    [-165,-240,125,26],[215,-245,140,24],[-180,250,120,22],[205,240,130,26]];
  /**
   * 外圈层数。
   *
   * **基底从 5~13 抬到 14~28，是被取证逼出来的。** 第一版按"比内圈矮一档"来配，
   * 结果整圈几乎看不见：内圈离街区 34/56/78m、最高 12 层 34m，在最北那条街上
   * 离它只有 43m，34m 就是 38°；而外圈首排 129m，同样 34m 只有 15°——被内圈
   * 挡得严严实实。实测（tmp/skyline-shots/S1-north-street）只有 8 栋地标露得出来，
   * 其余 110 栋白做。换算一下：外圈首排要在 43m 处的内圈屋顶线（约 15~28°）之上
   * 露头，需要 13~25 层。所以基底必须整体抬到 14 层以上。
   *
   * 现在 117 栋里 99% 能露头、89% 明显露头（中位数 26 层 73m）。
   * 核账脚本：tmp/_out-hist2.mjs。
   *
   * 地标单独先判、且**不参与基底/核心叠加**：它们要明显高出一档才有"市中心"的
   * 读感。高度按散列在 52~64 层之间散开——第一版所有地标都写死 50 层，8 栋一模
   * 一样高，从街上看是一排齐头的白柱子，反而不像城市。
   */
  const outFloors=(x:number,z:number,index:number)=>{
    if(hash(index,53)%17===0)return 52+hash(index,29)%13;   // 地标 52~64 层（146~180m）
    let f=14+hash(index,17)%15;                             // 基底 14~28 层（40~79m）
    for(const [cx,cz,r,boost] of OUT_CORES){
      const d=Math.hypot(x-cx,z-cz);
      if(d<r)f+=Math.round(boost*(1-d/r));
    }
    f+=hash(index,23)%5-2;                                  // 逐栋抖动，屋顶线才不会齐平
    return Math.max(6,Math.min(50,f));
  };
  const outerPlaced:{x0:number;x1:number;z0:number;z1:number}[]=[];
  scene.userData.outerRingPlan=outerPlaced;
  /**
   * [组名, 面宽是否沿 x, 散列盐, 取坐标]。东西墙只对调面宽/进深，不旋转。
   *
   * **第三个元素（散列盐）是必须的，不是装饰。** 四面墙的格点公式里 (k,i) 的取值范围
   * 完全一样，如果 index 只由 `k*13+i` 决定，那四面墙的留空格、footprint、地标位置
   * 会**逐格重合**——从天上俯视就是一座旋转对称的城市，而且南北两面会立起同样高的
   * 地标。加一个每面墙不同的常数把散列相位错开，四面的图案就各自独立了。
   */
  const outWalls:[string,boolean,number,(k:number,i:number)=>[number,number]][]=[
    ['skyline-n2',true, 0,   (k,i)=>[OUT_X0+i*OUT_COL_STEP, OUT_Z0-OUT_BASE-k*OUT_ROW_STEP]],
    ['skyline-s2',true, 401, (k,i)=>[OUT_X0+i*OUT_COL_STEP, WALK_BOUNDS.z1+34+OUT_BASE+k*OUT_ROW_STEP]],
    ['skyline-w2',false,823, (k,i)=>[WALK_BOUNDS.x0-34-OUT_BASE-k*OUT_ROW_STEP, OUT_Z0+OUT_COL_STEP+i*OUT_COL_STEP]],
    ['skyline-e2',false,1201,(k,i)=>[WALK_BOUNDS.x1+34+OUT_BASE+k*OUT_ROW_STEP, OUT_Z0+OUT_COL_STEP+i*OUT_COL_STEP]],
  ];
  for(const [name,alongX,wSalt,at] of outWalls){
    const wall=new THREE.Group();wall.name=name;wall.userData.sceneCollideSkip=true;scene.add(wall);
    skyline.push(wall);   // 与内圈一起在卸载时释放几何
    for(let k=0;k<OUT_ROWS;k++)for(let i=0;i<OUT_COLS;i++){
      const [x,z]=at(k,i);
      const index=k*OUT_COLS+i+977+wSalt;         // 977 与内圈错开相位，wSalt 与邻墙错开
      if(hash(index,41)%5===0)continue;           // 留空：街口 / 公园
      const [fw,fd]=OUT_FOOT[hash(index,43)%OUT_FOOT.length];
      const w=alongX?fw:fd,d=alongX?fd:fw;
      const f={x0:x-w/2-.5,x1:x+w/2+.5,z0:z-d/2-.5,z1:z+d/2+.5};
      if(f.x1>TRACK_GAP[0]&&f.x0<TRACK_GAP[1])continue;   // 让开铁路走廊
      if(hitsRiver(f))continue;                           // 让开北侧小河
      outerPlaced.push(f);
      const h=levelY(outFloors(x,z,index));
      box(wall,x,h/2,z,w,h,d,outerStone[hash(index,47)%outerStone.length]);
    }
    mergeByMaterial(wall);
  }

  /**
   * 街区边界：一圈**不可见**的围墙，把第一人称和 NPC 都关在街区里。
   *
   * 为什么必须有它：第一人称没有任何硬性坐标钳位（fpsControls 里只有碰撞盒，
   * 没有 clamp），而可行走的 `street` 地板只有 x[-80,80] z[-78,83]。走出地板之后
   * supportY 返回 null、玩家退回 floorY=0 继续站在世界地面上——也就是说能一路走到
   * 地平线，甚至走进 z≈-110 那圈远景低模楼里。边界环把这条路堵死。
   *
   * 取值就是上面声明的 `WALK_BOUNDS`（x[-80,150] z[-78,95]）——**刻意与 navWorld 的
   * street 层 clip 完全一致**，东扩的车站地坪 x[80,150] z[-45,95] 也在里面。
   * 两边同源的好处是「NPC 走得到的地方玩家也走得到」这条不变量继续成立，不会出现
   * 宠物能去、玩家却撞上空气墙的裂缝。实测这一圈之外只剩远景低模楼，
   * 街区里的手写内容全在里面。
   *
   * 盒子做成 1m 厚、4m 高、四边互相搭接（角上重叠无所谓，碰撞盒重叠无害）：
   *  - **4m 高**：`collidesAt` 只认与「脚底~头顶」区间有交集的盒，而且自身高度
   *    ≤ STEP_UP(0.45) 且顶面不高于脚底+0.45 的盒子会被当成台阶迈过去。4m 两条
   *    都远远躲开，跳也跳不过；
   *  - **1m 厚**：远厚于 `pickOverheadSlabs` 的 maxThickness(0.35)，不会被误认成
   *    「头顶水平板」拿去给街面积水做遮挡剔除（那会误剔掉整条街的反射）。
   *
   * 环本身没有几何体，纯碰撞盒：玩家撞上去只会停住，看不到任何东西。
   */
  const WALL_T=1,WALL_H=4;
  const boundary:Collider[]=[];
  const boundaryWall=(src:string,x0:number,x1:number,z0:number,z1:number)=>{
    boundary.push({source:src,kind:'wall',min:new THREE.Vector3(x0,0,z0),max:new THREE.Vector3(x1,WALL_H,z1)});
  };
  boundaryWall('block-boundary-w',WALK_BOUNDS.x0-WALL_T,WALK_BOUNDS.x0,WALK_BOUNDS.z0-WALL_T,WALK_BOUNDS.z1+WALL_T);
  boundaryWall('block-boundary-e',WALK_BOUNDS.x1,WALK_BOUNDS.x1+WALL_T,WALK_BOUNDS.z0-WALL_T,WALK_BOUNDS.z1+WALL_T);
  boundaryWall('block-boundary-n',WALK_BOUNDS.x0-WALL_T,WALK_BOUNDS.x1+WALL_T,WALK_BOUNDS.z0-WALL_T,WALK_BOUNDS.z0);
  boundaryWall('block-boundary-s',WALK_BOUNDS.x0-WALL_T,WALK_BOUNDS.x1+WALL_T,WALK_BOUNDS.z1,WALK_BOUNDS.z1+WALL_T);

  const foreground=new THREE.Group();foreground.name='authored-roof-and-garden';foreground.userData.sceneCollideSkip=true;scene.add(foreground);
  const cedar=new THREE.MeshStandardMaterial({color:'#796d57',roughness:.84});
  const leaves=['#314f47','#426653','#63806a'].map(color=>new THREE.MeshStandardMaterial({color,roughness:.92}));
  // Convenience-store roof: gravel inset, raised seams, screened plant and a planted edge.
  box(foreground,1.1,3.88,17.6,10.45,.06,6.4,roof);
  for(let x=-3.8;x<6.2;x+=.8)box(foreground,x,3.925,17.6,.025,.025,6.1,trim);
  for(let x=-3.7;x<-.5;x+=1.1){
    box(foreground,x,4.16,19.3,.8,.47,1.1,stone[2]);
    const fan=new THREE.Mesh(new THREE.CylinderGeometry(.27,.27,.045,20),trim);fan.position.set(x,4.42,19.3);foreground.add(fan);
    for(let k=0;k<6;k++)box(foreground,x,4.01+k*.055,18.74,.67,.022,.025,trim);
  }
  for(let x=-4;x<.2;x+=.18)box(foreground,x,4.38,20.5,.075,.95,.09,cedar);
  for(let z=18.3;z<20.6;z+=.18)box(foreground,-4,4.38,z,.09,.95,.075,cedar);
  function planter(x:number,y:number,z:number,w:number){
    box(foreground,x,y+.19,z,w,.38,.58,trim);
    box(foreground,x,y+.4,z,w-.09,.06,.49,roof);
    for(let n=0;n<Math.ceil(w*6);n++){
      const mesh=new THREE.Mesh(foliage,leaves[n%3]);mesh.position.set(x+(random()-.5)*(w-.1),y+.53+random()*.18,z+(random()-.5)*.38);mesh.scale.set(.17+random()*.14,.18+random()*.24,.16+random()*.16);mesh.castShadow=true;foreground.add(mesh);
    }
  }
  for(let x=1.4;x<6;x+=1.3)planter(x,3.92,20.4,1.15);
  mergeByMaterial(foreground);

  const skyMaterial=new THREE.ShaderMaterial({side:THREE.BackSide,depthWrite:false,depthTest:true,
    uniforms:{top:{value:new THREE.Color('#091a30')},horizon:{value:new THREE.Color('#516678')},night:{value:1},cloud:{value:.6}},
    vertexShader:`varying vec3 vDirection;void main(){vDirection=position;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.);gl_Position.z=gl_Position.w;}`,
    fragmentShader:`varying vec3 vDirection;uniform vec3 top;uniform vec3 horizon;uniform float night;uniform float cloud;
    float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
    float noise(vec2 p){vec2 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);return mix(mix(hash(i),hash(i+vec2(1,0)),f.x),mix(hash(i+vec2(0,1)),hash(i+1.),f.x),f.y);}
    void main(){vec3 d=normalize(vDirection);float h=max(d.y,0.);vec3 c=mix(horizon,top,pow(smoothstep(-.08,.85,d.y),.65));
    vec2 uv=d.xz/(abs(d.y)+.22);float n=noise(uv*1.8)*.65+noise(uv*4.6)*.25+noise(uv*11.)*.1;
    c=mix(c,horizon*.85,smoothstep(.47,.75,n)*cloud*smoothstep(0.,.25,h)*.48);
    float moon=dot(d,normalize(vec3(-.48,.55,-.68)));c+=vec3(.48,.62,.8)*pow(max(moon,0.),160.)*night*(1.-cloud*.65);
    c+=vec3(.75,.83,.87)*smoothstep(.9996,.9998,moon)*night*(1.-cloud*.8);
    gl_FragColor=vec4(c,1.);#include <tonemapping_fragment>
    #include <colorspace_fragment>
    }`.replace(';#include',';\n#include')});
  /**
   * 天空球（远景大气）。
   *
   * 这个 shader 每个像素要算 3 个八度的 `fract(sin(dot(...)))` 值噪声 + 月亮高光，
   * 是全屏最贵的一段 ALU。所以绘制顺序必须让它「只画真正露出来的像素」：
   *
   *  - 顶点着色器把 z 顶到 `gl_Position.w`（深度恒等于 1.0），配合默认的
   *    LessEqualDepth：只要该像素已经被任何实体写过深度（深度 < 1.0），
   *    深度测试就把它拒掉——**不进入片元着色器**。
   *  - 因此必须 renderOrder 置大、排在所有不透明实体之后。反过来（排在前面、
   *    或 depthTest 关掉）就没有任何像素能被早剔，室内贴着墙看时整屏噪声白算。
   *  - depthWrite 保持 false：天空不写深度，不会挡住它之后的透明物体（玻璃）。
   *
   * 透明物体（玻璃/水汽）在 three 里一律排在不透明之后，所以天空仍会正确地
   * 出现在它们背后；室外看天时几乎没有遮挡，行为与之前一致。
   */
  const sky=new THREE.Mesh(new THREE.SphereGeometry(140,32,16),skyMaterial);sky.name='district-atmosphere';sky.renderOrder=1000;sky.frustumCulled=false;scene.add(sky);

  // Slow drifting petals above the shrine approach and park path.
  const petalPositions=new Float32Array(96*3);
  for(let i=0;i<96;i++){
    const a=i*2.399;
    petalPositions[i*3]=Math.cos(a)*8.2;
    petalPositions[i*3+1]=.6+((i*37)%71)/10;
    petalPositions[i*3+2]=36+((i*19)%190)/10;
  }
  const petalGeometry=new THREE.BufferGeometry();
  petalGeometry.setAttribute('position',new THREE.BufferAttribute(petalPositions,3));
  const petalMaterial=new THREE.PointsMaterial({color:'#ffd4e1',size:.14,transparent:true,opacity:.88,depthWrite:false,sizeAttenuation:true});
  petalMaterial.userData.outlineWeight=0;
  const petals=new THREE.Points(petalGeometry,petalMaterial);petals.name='sakura-drift';petals.frustumCulled=false;scene.add(petals);

  function prepare(root:THREE.Object3D){
    root.updateMatrixWorld(true);
    root.traverse(o=>{if(!(o instanceof THREE.Mesh))return;
      const convert=(m:THREE.Material)=>{
        if(!(m instanceof THREE.MeshToonMaterial))return m;
        if(converted.has(m))return converted.get(m)!;
        const next=new THREE.MeshStandardMaterial({color:m.color,map:m.map,transparent:m.transparent,opacity:m.opacity,side:m.side,alphaTest:m.alphaTest,depthWrite:m.depthWrite,emissive:m.emissive,emissiveMap:m.emissiveMap,emissiveIntensity:m.emissiveIntensity,roughness:.82});
        next.userData.outlineWeight=0;converted.set(m,next);return next;
      };
      const source=Array.isArray(o.material)?null:o.material as THREE.MeshToonMaterial;
      const road=source?.map===asphaltTexture()||o.name==='world-ground';
      const walk=source?.map===plazaStoneTexture()||source?.map===sidewalkTexture();
      if(road||walk){
        o.material=road?streets.asphalt:streets.paving;
        const geometry=o.geometry.clone(),pos=geometry.attributes.position,uv=geometry.attributes.uv;
        const v=new THREE.Vector3();
        if(uv)for(let i=0;i<pos.count;i++){v.fromBufferAttribute(pos,i).applyMatrix4(o.matrixWorld);uv.setXY(i,v.x/2,v.z/2);}
        o.geometry=geometry;
      }else if(source?.map===curbTexture())o.material=streets.stone;
      else o.material=Array.isArray(o.material)?o.material.map(convert):convert(o.material);
      o.userData.outlineWeight=0;

      const geo=o.geometry;
      if(geo instanceof THREE.BoxGeometry && !(geo instanceof RoundedBoxGeometry)){
        const {width:w,height:h,depth:d}=geo.parameters;
        if(Math.min(w,h,d)>.075&&Math.max(w,h,d)<8){
          const key=`${w},${h},${d}`;let rounded=bevels.get(key);
          if(!rounded){rounded=new RoundedBoxGeometry(w,h,d,1,Math.min(.045,Math.min(w,h,d)*.12));bevels.set(key,rounded);}
          o.geometry=rounded;
        }
      }
    });
  }
  let elapsed=0;
  /* 动态碰撞盒：电车与车流**各自**维护一份，这里每帧合成一个数组交给 RoomScene。
   * 必须是同一个数组对象、原地改内容 —— RoomScene 每帧读
   * `for(const c of districtArt.dynamicColliders) blockerBuf.push(c)`，
   * 换成每帧新建数组会让它读到上一帧的旧引用。 */
  const dynamicColliders:Collider[]=[];
  return {prepare,colliders,boundary,dynamicColliders,
    setEnvironment(period:string,weather:string){
      sakuraTown.setEnvironment(period,weather);
      /* LUMINA 的内透（幕墙/中庭穹顶/停车楼灯带）。只吃 period，不看天气 ——
       * 商场夜里开灯与下不下雨无关。 */
      civicQuarter.setEnvironment(period);
      /* 世界广场的灯笼 / 欧式窗 / 街灯。同样只吃 period。 */
      globalSquare.setEnvironment(period);
      const night=period==='night',dusk=period==='dusk';const overcast=weather==='storm';
      streets.setWet(weather==='drizzle'||overcast);
      skyMaterial.uniforms.top.value.set(overcast?'#182b3c':night?'#09172e':dusk?'#424d79':'#468ab8');
      skyMaterial.uniforms.horizon.value.set(overcast?'#4b626f':night?'#334b60':dusk?'#e6ae94':'#c1d9dc');
      skyMaterial.uniforms.night.value=night?1:0;skyMaterial.uniforms.cloud.value=weather==='clear'?.22:overcast?1:.7;
      glow.emissiveIntensity=night?1.5:dusk?.9:0;
      scene.fog=new THREE.Fog(skyMaterial.uniforms.horizon.value,overcast?32:night?55:85,overcast?140:night?230:300);
    },
    update(camera:THREE.Camera,dt:number){
      sky.position.copy(camera.position);
      elapsed+=dt;
      sakuraTown.update(elapsed);civicQuarter.update(elapsed);sakuraStation.update(dt,camera);streetTraffic.update(dt,camera);
      dynamicColliders.length=0;
      for(const c of sakuraStation.dynamicColliders)dynamicColliders.push(c);
      for(const c of streetTraffic.dynamicColliders)dynamicColliders.push(c);
      const positions=petalGeometry.attributes.position as THREE.BufferAttribute;
      for(let i=0;i<positions.count;i++){
        const baseY=.6+((i*37)%71)/10;
        const phase=elapsed*.45+i*.71;
        const fall=((baseY-.6-elapsed*(.34+(i%4)*.04))%8.2+8.2)%8.2;
        positions.setY(i,.6+fall);
        positions.setX(i,Math.cos(i*2.399+phase*.13)*8.2+Math.sin(phase)*.48);
        positions.setZ(i,36+((i*19)%190)/10+Math.sin(phase*.7)*.38);
      }
      positions.needsUpdate=true;
    },
    dispose(){disposeBlenderVehicles();disposeBlenderTrunks();urbanInfill.dispose();cityDressing.dispose();streetTraffic.dispose();worldStreetLife.dispose();globalSquare.dispose();civicQuarter.dispose();sakuraStation.dispose();sakuraTown.dispose();streets.dispose();petalGeometry.dispose();petalMaterial.dispose();for(const m of detailMaterials)m.dispose();for(const m of converted.values())m.dispose();for(const g of bevels.values())g.dispose();
      /* 远景四面墙的合批几何是自己 merge 出来的，不在上面任何一处缓存里，要单独放。
       * 材质是共享的（stone/trim/roof…），不能在这里 dispose。 */
      for(const wall of skyline)wall.traverse(o=>{const mesh=o as THREE.Mesh;if(mesh.isMesh)mesh.geometry.dispose();});}
  };
}
