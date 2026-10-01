import * as THREE from 'three';
import type { Collider } from './collider';
import { CITY_ROUTES, SOUTH_ROUTES, buildCityRoutes } from './cityRoutes';
import { mergeByMaterial } from './merge';
import { createLeafField } from './foliage';

type Plot={x:number;z:number;w:number;d:number;angle:number;front:THREE.Vector3;bounds:THREE.Box2};
/** Street-oriented infill selected against existing authored collision footprints and all road corridors. */
export function createUrbanInfill(scene:THREE.Scene,existing:Collider[]){
  const root=new THREE.Group();root.name='inhabited-city-infill';root.userData.sceneCollideSkip=true;scene.add(root);
  const colliders:Collider[]=[],materials:THREE.Material[]=[],geometries=new Set<THREE.BufferGeometry>();
  const mat=(c:string)=>{const m=new THREE.MeshStandardMaterial({color:c,roughness:.88});m.userData.outlineWeight=0;materials.push(m);return m;};
  const walls=['#b9ac92','#c0937b','#98aaa0','#d4c7ac','#a3a9b2','#baaa9c'].map(mat),trim=mat('#e2d7bd'),iron=mat('#3e5059'),glass=mat('#829fa7'),wood=mat('#80604c'),green=mat('#627d5a'),clay=mat('#ad795d'),roof=mat('#637279'),paving=mat('#b2b2a6');
  const cube=new THREE.BoxGeometry(1,1,1),sphere=new THREE.IcosahedronGeometry(1,1);geometries.add(cube);geometries.add(sphere);
  const reserved=existing.filter(c=>c.kind!=='floor'&&c.kind!=='ramp'&&c.max.y>.55&&c.min.y<4).map(c=>new THREE.Box2(new THREE.Vector2(c.min.x-.65,c.min.z-.65),new THREE.Vector2(c.max.x+.65,c.max.z+.65)));
  for(const [a,b,c,d] of [[-40,40,-10,9],[-40,40,-39,-22],[-31,39,12,28],[-11,11,34,57],[-64,-6,110,155],[16,64,109,146],[78,103,-78,165],[-35,-13,86,96]])reserved.push(new THREE.Box2(new THREE.Vector2(a,c),new THREE.Vector2(b,d)));
  const routes=[...CITY_ROUTES,...SOUTH_ROUTES];
  const samples=routes.map(r=>({r,curve:new THREE.CatmullRomCurve3(r.points.map(([x,z])=>new THREE.Vector3(x,0,z)),false,'centripetal')}));
  const roadPoints=samples.flatMap(({r,curve})=>curve.getSpacedPoints(Math.ceil(curve.getLength())).map(p=>({p,clear:r.width/2+(r.walk?.75:2.2)})));
  const plots:Plot[]=[];
  function propose(x:number,z:number,w:number,d:number,angle:number,front:THREE.Vector3){
    const hx=Math.abs(Math.cos(angle))*w/2+Math.abs(Math.sin(angle))*d/2+.55,hz=Math.abs(Math.sin(angle))*w/2+Math.abs(Math.cos(angle))*d/2+.55;
    const b=new THREE.Box2(new THREE.Vector2(x-hx,z-hz),new THREE.Vector2(x+hx,z+hz));
    if(b.min.x< -77||b.max.x>74||b.min.y< -76||b.max.y>108||reserved.some(r=>r.intersectsBox(b)))return;
    if(roadPoints.some(({p,clear})=>p.x>b.min.x-clear&&p.x<b.max.x+clear&&p.z>b.min.y-clear&&p.z<b.max.y+clear))return;
    plots.push({x,z,w,d,angle,front,bounds:b});reserved.push(b.clone().expandByScalar(.35));
  }
  for(const {r,curve} of samples){
    const length=curve.getLength();
    for(let at=5;at<length-4;at+=6.2)for(const side of [-1,1]){
      const t=at/length,p=curve.getPointAt(t),tangent=curve.getTangentAt(t),normal=new THREE.Vector3(-tangent.z,0,tangent.x).multiplyScalar(side);
      const w=4.8,d=6.2,offset=r.width/2+(r.walk?2.7:4)+d/2;
      const centre=p.clone().addScaledVector(normal,offset);const front=p.clone().addScaledVector(normal,r.width/2+(r.walk?.4:1.6));
      propose(centre.x,centre.z,w,d,Math.atan2(normal.x,normal.z),front);
    }
  }
  function box(parent:THREE.Group,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material){const o=new THREE.Mesh(cube,m);o.position.set(x,y,z);o.scale.set(w,h,d);o.castShadow=true;o.receiveShadow=true;parent.add(o);return o;}
  // One atlas for all storefront signs instead of one high-resolution texture per building.
  const canvas=document.createElement('canvas');canvas.width=1024;canvas.height=1024;const ctx=canvas.getContext('2d')!;
  const names=['CORNER BAKERY','街角书房','DAILY MARKET','CAFÉ TERRACE','青禾食堂','FLOWER STUDIO','VINTAGE & VINYL','邻里药房'];
  names.forEach((name,i)=>{const y=i*128;ctx.fillStyle=['#3e6456','#925f51','#485c79'][i%3];ctx.fillRect(0,y,1024,128);ctx.fillStyle='#f3e5cb';ctx.textAlign='center';ctx.font='bold 48px Georgia,"Microsoft YaHei",sans-serif';ctx.fillText(name,512,y+58);ctx.font='21px sans-serif';ctx.fillText('NEIGHBOURHOOD  ·  OPEN DAILY',512,y+104);});
  const atlas=new THREE.CanvasTexture(canvas);atlas.colorSpace=THREE.SRGBColorSpace;const signMat=new THREE.MeshStandardMaterial({map:atlas,roughness:.85});signMat.userData.outlineWeight=0;materials.push(signMat);
  const signs=names.map((_,i)=>{const geo=new THREE.PlaneGeometry(4.35,.6);const uv=geo.attributes.uv;for(let n=0;n<uv.count;n++)uv.setY(n,1-(i+1)/8+uv.getY(n)/8);geometries.add(geo);return geo;});
  plots.forEach((p,i)=>{
    const g=new THREE.Group();g.name=`infill-house-${i}`;g.position.set(p.x,0,p.z);g.rotation.y=p.angle;root.add(g);
    const h=6.5+(i%3)*2.8,wall=walls[i%walls.length];
    box(g,0,h/2,0,p.w,h,p.d,wall);
    for(let y=3.4;y<h;y+=2.8)box(g,0,y,0,p.w+.12,.12,p.d+.12,trim);
    // All four elevations are furnished: the overview must not reveal blank back walls.
    for(const side of [-1,1])for(let f=0;f<(h-1)/2.8;f++)for(const x of [-1.3,1.3]){
      const y=1.7+f*2.8,z=side*(p.d/2+.035);
      box(g,x,y,z,1.25,1.7,.08,iron);box(g,x,y,z+side*.055,1.08,1.53,.025,glass);box(g,x,y,z+side*.079,.035,1.54,.025,trim);box(g,x,y-.9,z+side*.1,1.4,.1,.3,trim);
      if(f>0&&(i+f)%2===0){box(g,x,y-.92,z+side*.4,1.55,.12,.85,trim);box(g,x,y-.3,z+side*.8,1.55,.045,.045,iron);for(let n=0;n<7;n++)box(g,x-.7+n*.23,y-.58,z+side*.8,.025,.58,.03,iron);box(g,x+.35,y-.73,z+side*.42,.45,.24,.24,clay);box(g,x+.35,y-.52,z+side*.42,.46,.25,.27,green);}
    }
    for(const side of [-1,1])for(let f=0;f<(h-1)/2.8;f++)for(const z of [-1.8,0,1.8]){box(g,side*(p.w/2+.035),1.75+f*2.8,z,.065,1.45,1.04,iron);box(g,side*(p.w/2+.075),1.75+f*2.8,z,.025,1.28,.88,glass);}
    box(g,0,h+.15,0,p.w+.35,.3,p.d+.35,trim);box(g,0,h+.34,0,p.w-.22,.1,p.d-.22,roof);
    box(g,.8,h+.65,1.1,1.3,.5,1.05,iron);for(let n=0;n<7;n++)box(g,.28+n*.17,h+.92,1.1,.06,.035,.85,trim);
    box(g,-2.3,h/2,-3.2,.06,h,.06,iron);
    const sign=new THREE.Mesh(signs[i%8],signMat);sign.rotation.y=Math.PI;sign.position.set(0,2.92,-3.18);g.add(sign);
    for(let n=0;n<12;n++){box(g,-2.2+n*.4,2.5,-3.55,.39,.065,.8,n%2?trim:green);box(g,-2.2+n*.4,2.37,-3.93,.39,.21,.04,n%2?trim:green);}
    box(g,.25,1.25,-3.18,.85,2.5,.08,wood);box(g,.5,1.2,-3.25,.04,.26,.04,trim);
    // Explicit footprint includes projecting balconies, never an entire merged block.
    colliders.push({source:g.name,kind:'wall',min:new THREE.Vector3(p.bounds.min.x,0,p.bounds.min.y),max:new THREE.Vector3(p.bounds.max.x,h,p.bounds.max.y)});
    const entrance=new THREE.Vector3(0,0,-p.d/2-.75).applyAxisAngle(new THREE.Vector3(0,1,0),p.angle).add(g.position);
    for(const geo of buildCityRoutes(root,colliders,[{name:`infill-entry-${i}`,points:[[p.front.x,p.front.z],[entrance.x,entrance.z]],width:1.4,walk:true}],{road:paving,paving,paint:trim},.04))geometries.add(geo);
    // Planted backyard edge and a small paved forecourt occupy the leftover strip.
    box(g,0,.025,3.85,p.w,.05,1.4,paving);for(const x of [-1.8,1.8]){box(g,x,.23,3.8,.65,.46,.65,clay);const bush=new THREE.Mesh(sphere,green);bush.position.set(x,.68,3.8);bush.scale.set(.46,.45,.46);g.add(bush);}
  });
  // Small courtyard gardens occupy residual land between streets and buildings.
  let seed=734;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  const foliage=createLeafField(random,2400);let gardens=0;
  for(let z=39;z<108;z+=12)for(let x=-71;x<74;x+=12){
    if(gardens>=20)continue;
    const b=new THREE.Box2(new THREE.Vector2(x-2.6,z-2.6),new THREE.Vector2(x+2.6,z+2.6));
    if(reserved.some(r=>r.intersectsBox(b))||roadPoints.some(({p,clear})=>p.x>b.min.x-clear&&p.x<b.max.x+clear&&p.z>b.min.y-clear&&p.z<b.max.y+clear))continue;
    const nearest=roadPoints.reduce((a,c)=>Math.hypot(a.p.x-x,a.p.z-z)<Math.hypot(c.p.x-x,c.p.z-z)?a:c);
    if(Math.hypot(nearest.p.x-x,nearest.p.z-z)>15)continue;
    gardens++;reserved.push(b);
    box(root,x,.04,z,5,.08,5,paving);
    colliders.push({source:'infill-courtyard-floor',kind:'floor',min:new THREE.Vector3(x-2.5,0,z-2.5),max:new THREE.Vector3(x+2.5,.08,z+2.5)});
    box(root,x+1.3,.32,z+1,1,.64,1,clay);box(root,x+1.3,1.8,z+1,.15,2.5,.15,wood);
    for(let n=0;n<100;n++)foliage.spray(x+1.3+(random()-.5)*2.5,2.5+random()*1.6,z+1+(random()-.5)*2.5,.16+random()*.22);
    for(let n=0;n<4;n++)box(root,x-.9,.5,z-.65+n*.16,1.8,.08,.12,wood);
    for(const dx of [-.65,.65])box(root,x-.9+dx,.26,z-.4,.09,.5,.6,iron);
    box(root,x-.9,.87,z-.12,1.8,.55,.065,wood);
    colliders.push({source:'infill-courtyard-furniture',kind:'wall',min:new THREE.Vector3(x-1.9,0,z-.8),max:new THREE.Vector3(x+.05,1.2,z)});
    colliders.push({source:'infill-courtyard-tree',kind:'wall',min:new THREE.Vector3(x+.8,0,z+.5),max:new THREE.Vector3(x+1.8,3,z+1.5)});
  }
  // Geographical batches preserve culling when a viewer is inside a street.
  const chunks=new Map<string,THREE.Group>();
  for(const child of [...root.children]){const k=`${Math.floor(child.position.x/28)}:${Math.floor(child.position.z/28)}`;let c=chunks.get(k);if(!c){c=new THREE.Group();c.name=`infill-block-${k}`;chunks.set(k,c);}c.add(child);}
  for(const chunk of chunks.values()){root.add(chunk);mergeByMaterial(chunk);}
  root.add(foliage.build('infill-courtyard-leaves'));
  root.userData.plan={buildings:plots.length,gardens,plots:plots.map(p=>({x:p.x,z:p.z,w:p.w,d:p.d,angle:p.angle}))};
  return {colliders,dispose(){root.removeFromParent();root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);if(o instanceof THREE.InstancedMesh)o.dispose();});geometries.forEach(g=>g.dispose());materials.forEach(m=>m.dispose());atlas.dispose();foliage.material.dispose();foliage.texture.dispose();}};
}
