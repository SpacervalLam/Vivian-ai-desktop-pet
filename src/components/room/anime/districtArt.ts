import { buildUrbanStreets, CITY_LOTS, CITY_ROADS } from './urbanStreets';
import { asphaltTexture, plazaStoneTexture, sidewalkTexture, curbTexture } from './toon';
import type { Collider } from './collider';
import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { mergeByMaterial } from './merge';

/** District art is independent of navigation: all authored additions are visual only. */
export function createDistrictArt(scene: THREE.Scene) {
  const streets = buildUrbanStreets(scene);
  const colliders: Collider[] = [...streets.colliders];
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
  const trim = new THREE.MeshStandardMaterial({color:'#3b525b',roughness:.7,metalness:.25});
  const roof = new THREE.MeshStandardMaterial({color:'#556765',roughness:.95});
  /**
   * 描边分级（见 toon.ts 的 outlineWeightOf）。
   *
   * 只有**体量**（石材/线脚/屋顶板）描边；玻璃和发光件一律置 0：
   *  - 窗格只有 0.025m 厚，外扩壳比它本身还厚，描了会糊成一坨；
   *  - 发光体描边会把 bloom 的光晕闷死在黑壳里。
   * 注意：街区主体（district）根本不会被 outlineProp 碰到，这些标记只对
   * 近景那一排（frontage）生效——远景保持无线稿，否则雾里的剪影会变成网格。
   */
  for(const m of [...stone,trim,roof])m.userData.outlineWeight=2;
  darkGlass.userData.outlineWeight=0;
  glow.userData.outlineWeight=0;
  const district = new THREE.Group(); district.name='authored-city-district'; district.userData.sceneCollideSkip=true;
  scene.add(district);
  /**
   * 近景一排（公寓正对面那条街，id 以 -19.6 结尾）单独成组。
   *
   * 理由：街区整体是刻意不描边的（性能 + 写实），但这样一来"隔着一条街的楼
   * 是纯色块、身后的公寓有清晰线稿"，室内外质感是断的。只给最近这一排挂描边
   * 就能把断点接上，代价是 4 栋楼的描边壳而不是 48 栋。
   */
  const frontage = new THREE.Group(); frontage.name='authored-city-frontage'; frontage.userData.sceneCollideSkip=true;
  scene.add(frontage);
  let seed=2167; const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  /** 确定性散列，取乘法散列的高位（低位没混匀，直接取模会退化成等差数列）。 */
  const hash=(i:number,salt:number)=>(Math.imul(i+salt,2654435761)>>>8);
  const cube=new THREE.BoxGeometry(1,1,1);
  function box(parent:THREE.Group,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material){
    const mesh=new THREE.Mesh(cube,m);mesh.position.set(x,y,z);mesh.scale.set(w,h,d);mesh.receiveShadow=true;parent.add(mesh);return mesh;
  }
  // Each building has a recessed base, cornices, a setback crown and rooftop plant.
  function building(parent:THREE.Group,x:number,z:number,w:number,d:number,floors:number,index:number){
    const g=new THREE.Group();g.name=`city-block-${index}`;parent.add(g);
    const h=floors*2.8;const m=stone[hash(index,1)%stone.length];
    box(g,x,h/2,z,w,h,d,m);box(g,x,.65,z,w+.15,1.3,d+.15,trim);
    for(let f=1;f<=floors;f++) box(g,x,f*2.8-.06,z,w+.22,.14,d+.22,m);
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
    for(let f=0;f<rows;f++)for(let c=0;c<cols;c++){
      const px=x-w/2+(c+.5)*w/cols,py=f*2.8+1.65;
      for(const side of [-1,1]){
        const pz=z+side*(d/2+.025),lit=random()>.58;
        box(g,px,py,pz,1.05,1.5,.07,trim);
        box(g,px,py,pz+side*.046,.86,1.27,.025,lit?glow:darkGlass);
        box(g,px,py,pz+side*.065,.045,1.3,.025,trim);
        box(g,px,py-.82,pz+side*.12,1.2,.09,.26,m);
      }
    }
    const street=[...CITY_ROADS.eastWest].sort((a,b)=>Math.abs(a.z-z)-Math.abs(b.z-z))[0];
    const side=Math.sign(street.z-z),front=z+side*d/2;
    box(g,x,1.08,front+side*.12,1.28,2.12,.08,darkGlass);
    box(g,x,2.5,front+side*.45,2.0,.10,.90,trim);
    box(g,x,2.435,front+side*.45,1.5,.02,.45,glow);
    box(g,x+.46,1.02,front+side*.18,.035,.44,.035,stone[2]);
    // Side windows prevent blank silhouettes from oblique apartment views.
    for(let f=0;f<rows;f++)for(let c=0;c<Math.floor(d/2);c++)for(const side of [-1,1])
      box(g,x+side*(w/2+.03),f*2.8+1.65,z-d/2+1+c*2,.045,1.25,.9,random()>.66?glow:darkGlass);
  }
  CITY_LOTS.forEach((lot,index)=>{
    // 近景一排（行 3 中央，id 以 -19.6 结尾）单独进 frontage：只有它挂描边。
    const near=lot.id.endsWith('-19.6');
    building(near?frontage:district,lot.x,lot.z,lot.w,lot.d,lot.floors,index);
    colliders.push({source:lot.id,min:new THREE.Vector3(lot.x-lot.w/2-.12,0,lot.z-lot.d/2-.12),max:new THREE.Vector3(lot.x+lot.w/2+.12,lot.floors*2.8,lot.z+lot.d/2+.12)});
  });
  mergeByMaterial(district);
  mergeByMaterial(frontage);
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
  const foliage=new THREE.SphereGeometry(1,8,6);
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
  return {prepare,colliders,frontage,
    setEnvironment(period:string,weather:string){
      const night=period==='night',dusk=period==='dusk';const overcast=weather==='storm';
      streets.setWet(weather==='drizzle'||overcast);
      skyMaterial.uniforms.top.value.set(overcast?'#182b3c':night?'#09172e':dusk?'#424d79':'#468ab8');
      skyMaterial.uniforms.horizon.value.set(overcast?'#4b626f':night?'#334b60':dusk?'#e6ae94':'#c1d9dc');
      skyMaterial.uniforms.night.value=night?1:0;skyMaterial.uniforms.cloud.value=weather==='clear'?.22:overcast?1:.7;
      glow.emissiveIntensity=night?1.5:dusk?.9:0;
      scene.fog=new THREE.Fog(skyMaterial.uniforms.horizon.value,overcast?32:night?55:85,overcast?140:night?230:300);
    },
    update(camera:THREE.Camera){sky.position.copy(camera.position);},
    dispose(){streets.dispose();for(const m of converted.values())m.dispose();for(const g of bevels.values())g.dispose();}
  };
}
