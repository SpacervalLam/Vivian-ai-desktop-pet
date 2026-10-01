import * as THREE from 'three';
import type { ShellResult } from './props';
import { jpWoodGrainTexture, jpFabricTexture, jpFutonTexture } from './toon';

/** Instance-owned materials: the city and cached procedural materials remain independent. */
export function createInteriorDesign() {
  const owned=new Set<THREE.Material>(), textures=new Set<THREE.Texture>();
  let seed=203;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
  function texture(kind:'oak'|'stone'|'plaster'|'linen'){
    const canvas=document.createElement('canvas');canvas.width=canvas.height=512;const ctx=canvas.getContext('2d')!;
    ctx.fillStyle=kind==='oak'?'#c8ad89':kind==='stone'?'#d6d2c6':kind==='linen'?'#eee9dd':'#eeeae1';ctx.fillRect(0,0,512,512);
    if(kind==='oak'){
      for(let plank=0;plank<8;plank++){
        ctx.fillStyle=`rgba(88,61,35,${.025+random()*.075})`;ctx.fillRect(plank*64,0,63,512);
        ctx.fillStyle='rgba(65,46,29,.20)';ctx.fillRect(plank*64,0,1,512);
        const end=(plank%3)*170+45;ctx.fillRect(plank*64,end,64,1);
        for(let i=0;i<25;i++){ctx.strokeStyle=`rgba(93,63,32,${.015+random()*.055})`;ctx.lineWidth=.5;ctx.beginPath();const x=plank*64+random()*63;ctx.moveTo(x,0);ctx.bezierCurveTo(x+5,170,x-4,350,x,512);ctx.stroke();}
      }
    }else{
      for(let i=0;i<18000;i++){const shade=kind==='stone'?110:150;ctx.fillStyle=`rgba(${shade},${shade-7},${shade-12},${random()*.12})`;const r=kind==='stone'?.6+random()*1.8:.5;ctx.fillRect(random()*512,random()*512,r,r);}
      if(kind==='linen'){ctx.strokeStyle='rgba(130,116,92,.12)';ctx.lineWidth=.5;for(let i=0;i<512;i+=3){ctx.beginPath();ctx.moveTo(i,0);ctx.lineTo(i,512);ctx.moveTo(0,i);ctx.lineTo(512,i);ctx.stroke();}}
    }
    const map=new THREE.CanvasTexture(canvas);map.colorSpace=THREE.SRGBColorSpace;map.wrapS=map.wrapT=THREE.RepeatWrapping;map.anisotropy=4;textures.add(map);return map;
  }
  const oak=texture('oak'),stone=texture('stone'),plaster=texture('plaster'),linen=texture('linen');
  function material(name:string,color:string,map:THREE.Texture|null,roughness=.85){const m=new THREE.MeshStandardMaterial({color,map,roughness});m.name=name;m.userData.outlineWeight=0;owned.add(m);return m;}
  const floor=material('203 / pale oak','#ffffff',oak,.72),tile=material('203 / honed limestone','#ffffff',stone,.82);
  const wall=material('203 / warm lime plaster','#ffffff',plaster,.96),sage=material('203 / sage plaster','#a2b1a3',plaster,.95);
  const ceiling=material('203 / chalk ceiling','#f1eee5',null,.98),trim=material('203 / oak trim','#bfa98a',null,.77);
  const rugEdge=material('203 / rug binding','#958b75',null,.99),rug=material('203 / woven wool','#ddd1b9',linen,.99);
  wall.side=sage.side=ceiling.side=THREE.DoubleSide;wall.shadowSide=sage.shadowSide=ceiling.shadowSide=THREE.DoubleSide;
  const replacements=new Map<string,THREE.Material>();
  const replace=(old:THREE.Material,id:string)=>{
    if(!(old instanceof THREE.MeshStandardMaterial)&&!(old instanceof THREE.MeshToonMaterial))return old;
    const key=old.uuid+id;const cached=replacements.get(key);if(cached)return cached;
    const m=old instanceof THREE.MeshStandardMaterial?old.clone():new THREE.MeshStandardMaterial({color:old.color,map:old.map,transparent:old.transparent,opacity:old.opacity,side:old.side,depthWrite:old.depthWrite,alphaTest:old.alphaTest,emissive:old.emissive,emissiveMap:old.emissiveMap,emissiveIntensity:old.emissiveIntensity});
    m.roughness=old.name==='203-window' && old instanceof THREE.MeshStandardMaterial ? old.roughness : .78;m.userData={outlineWeight:0};
    const fabric=old.name.startsWith('MAT_Fabric')||old.map===jpFabricTexture()||old.map===jpFutonTexture();
    const wood=old.map===jpWoodGrainTexture()||old.name==='MAT_Surface_1';
    if(fabric){m.map=linen;m.color.set(id.startsWith('nana')?'#bda895':id.startsWith('master')?'#a8b4a3':'#8ba69a');m.roughness=.97;m.metalness=0;if(old.name==='MAT_Fabric_8')m.color.set('#f0e7d5');if(old.name==='MAT_Fabric_5')m.color.set('#c2b6a2');}
    if(wood){m.map=oak;m.color.set('#e1d1b9');m.roughness=.74;m.metalness=0;}
    if(id==='jp-counter'&&(wood||old.name==='MAT_Surface_9')){m.map=null;m.color.set('#819b8b');m.roughness=.65;}
    if(id==='jp-counter'&&(old.name==='MAT_Surface_16'||old.color.getHexString()==='e7dfcf')){m.map=stone;m.color.set('#fffaf0');m.roughness=.45;}
    if(old.name.startsWith('MAT_Metal')){m.roughness=.42;m.metalness=.48;}
    replacements.set(key,m);owned.add(m);return m;
  };
  function prepareFurniture(root:THREE.Object3D,id:string){root.traverse(o=>{if(o instanceof THREE.Mesh)o.material=Array.isArray(o.material)?o.material.map(m=>replace(m,id)):replace(o.material,id);});}
  function prepareShell(shell:ShellResult){
    const oldWalls=new Set(shell.wallMeshes.flatMap(o=>Array.isArray(o.material)?o.material:[o.material]));
    shell.group.traverse(o=>{if(!(o instanceof THREE.Mesh))return;
      if(!o.userData.isWallPiece && !Array.isArray(o.material) && oldWalls.has(o.material)){
        o.material=wall;
        o.geometry.computeBoundingBox();
        const height=o.geometry.boundingBox!.max.y-o.geometry.boundingBox!.min.y;
        if(height>2.7)o.scale.y=(height-.02)/height;
      }
      if(o.userData.floorType&&o.userData.floorType!=='jpdeck')o.material=o.userData.floorType==='jptile'?tile:floor;
      else if(o.userData.isCeiling)o.material=ceiling;
      else if(o.userData.isWallPiece){
        const accent=(o.userData.wallAxis==='x'&&Math.abs(o.userData.wallAt+1.86)<.01)||(o.userData.wallAxis==='z'&&Math.abs(o.userData.wallAt+1.34)<.01);
        o.material=accent?sage:wall;
      }
    });
    // Trim is attached to its wall panel so observer visibility and transforms stay consistent.
    for(const panel of shell.wallMeshes){
      const geo=panel.geometry as THREE.PlaneGeometry;const {width,height}=geo.parameters;
      if(panel.position.y-height/2>.005||width<.08)continue;
      const skirting=new THREE.Mesh(new THREE.BoxGeometry(width-.024,.09,.024),trim);
      skirting.name='203-skirting';skirting.position.set(0,-height/2+.065,.021);panel.add(skirting);
    }
  }
  function buildRug(size:number[]){
    const g=new THREE.Group();g.name='203-tailored-rug';
    // One closed volume replaces the two nearly coplanar planes. The top is 24 mm above the floor.
    const mesh=new THREE.Mesh(new THREE.BoxGeometry(size[0],.024,size[2]),[rugEdge,rugEdge,rug,rugEdge,rugEdge,rugEdge]);
    mesh.position.y=.012;mesh.receiveShadow=true;g.add(mesh);return g;
  }
  return {prepareShell,prepareFurniture,buildRug,dispose(){owned.forEach(m=>m.dispose());textures.forEach(t=>t.dispose());}};
}
