import fs from 'node:fs';
const path='plugins/3d-apartment/src/anime/props.ts';let s=fs.readFileSync(path,'utf8');const a=s.indexOf('export function buildWindow('),b=s.indexOf('\n/* ===',a);
s=s.slice(0,a)+`export function buildWindow(
  spec: { pos: [number, number, number]; size: [number, number, number] }
): THREE.Group {
  const g = new THREE.Group();
  const wallSurface = new THREE.Group(); wallSurface.name = 'window-wall';
  const roomSide = new THREE.Group(); roomSide.name = 'room-side';
  g.add(wallSurface, roomSide);
  const [w, y0, y1] = spec.size, h = y1-y0, cy = (y0+y1)/2;
  // Local +Z faces indoors. The trim starts 12 mm ahead of the wall;
  // glass, sash and fabric occupy separate depths, including their folds.
  const frame = new THREE.MeshStandardMaterial({color:'#8b897e',roughness:.42,metalness:.3});
  const seal = new THREE.MeshStandardMaterial({color:'#494d48',roughness:.85});
  const sill = new THREE.MeshStandardMaterial({color:'#c9b596',roughness:.7});
  const cloth = new THREE.MeshStandardMaterial({color:'#faf5e9',map:curtainTexture(),roughness:1,side:THREE.DoubleSide});
  const glass = new THREE.MeshStandardMaterial({color:'#b9d7d8',roughness:.14,metalness:.12,transparent:true,opacity:.085,depthWrite:false,side:THREE.DoubleSide});
  for (const material of [frame,seal,sill,cloth,glass]) { material.name='203-window'; material.userData.outlineWeight=0; }
  const bar=(ww:number,hh:number,dd:number,x:number,y:number,z:number,mat:THREE.Material=frame,parent=wallSurface)=>{
    const mesh=m(box(ww,hh,dd),mat,[x,y,z],'both');mesh.userData.noOutline=true;parent.add(mesh);return mesh;
  };
  // Slim perimeter: horizontal pieces meet verticals without intersecting.
  const fw=.036;
  for(const sign of [-1,1]) {
    bar(w+fw*2,fw,.065,0,sign<0?y0-fw/2:y1+fw/2,.0445);
    bar(fw,h,.065,sign*(w/2+fw/2),cy,.0445);
    bar(.012,h-.024,.023,sign*(w/2-.006),cy,.017,seal);
  }
  for(const yy of [y0+.006,y1-.006])bar(w,.012,.023,0,yy,.017,seal);
  const split = w*.12;
  bar(.025,h-.024,.043,split,cy,.048);
  // A fixed picture pane and a narrower operable sash; clear view at eye level.
  for(const [left,right] of [[-w/2+.012,split-.014],[split+.014,w/2-.012]]){
    const pane=m(new THREE.PlaneGeometry(right-left,h-.026),glass,[(left+right)/2,cy,-.009],'none');
    pane.userData.noOutline=true;wallSurface.add(pane);
  }
  bar(.009,.105,.025,split+.035,cy-.1,.091,seal);
  bar(w+.14,.038,.225,0,y0-.06,.082,sill);
  const top=y1+.145;
  const roller=y0>=.8;
  if(roller){
    bar(w+.10,.047,.055,0,top,.165,sill,roomSide);
    const drop=y0>1.4?.21:.14;
    bar(w-.02,drop,.009,0,top-.035-drop/2,.169,cloth,roomSide);
    bar(w-.012,.015,.017,0,top-.043-drop,.17,frame,roomSide);
  }else{
    bar(w+.46,.035,.055,0,top+.018,.235,sill,roomSide);
    const bottom=Math.max(.17,y0-.16),height=top-bottom;
    for(const sign of [-1,1]){
      const width=.29;
      const geo=new THREE.PlaneGeometry(width,height,24,16);
      const p=geo.getAttribute('position') as THREE.BufferAttribute;
      for(let i=0;i<p.count;i++){
        const u=p.getX(i)/width+.5,v=(p.getY(i)+height/2)/height;
        // Soft tapered pleats, with a gently relaxed hem.
        p.setX(i,p.getX(i)*(1+.12*(1-v)));
        p.setZ(i,Math.sin(u*Math.PI*8)*(.018+.009*(1-v)));
        p.setY(i,p.getY(i)+Math.cos(u*Math.PI*8)*.007*(1-v));
      }
      geo.computeVertexNormals();
      const drape=m(geo,cloth,[sign*(w/2+.035),(top+bottom)/2,.23],'both');
      drape.userData.noOutline=true;roomSide.add(drape);
    }
  }
  return g;
}
`+s.slice(b);fs.writeFileSync(path,s);
const tp='plugins/3d-apartment/src/anime/toon.ts';s=fs.readFileSync(tp,'utf8');const ta=s.indexOf('export function curtainTexture()'),tb=s.indexOf('\n/* ---',ta);s=s.slice(0,ta)+`export function curtainTexture(): THREE.Texture {
  if (_curtain) return _curtain;
  const { canvas, ctx } = makeCanvas(256, 256);
  ctx.fillStyle='#eee9df';ctx.fillRect(0,0,256,256);
  // Fine low-contrast linen only: geometric folds supply the shading.
  const rnd=makeRng(77);
  for(let i=0;i<256;i+=2){
    ctx.strokeStyle='rgba(135,124,106,'+(.025+rnd()*.04)+')';ctx.lineWidth=.5;
    ctx.beginPath();ctx.moveTo(i,0);ctx.lineTo(i,256);ctx.moveTo(0,i);ctx.lineTo(256,i);ctx.stroke();
  }
  _curtain=toTexture(canvas,[1,1]);return _curtain;
}
`+s.slice(tb);fs.writeFileSync(tp,s);
const ip='plugins/3d-apartment/src/anime/interiorDesign.ts';s=fs.readFileSync(ip,'utf8').replace('m.roughness=.78;m.userData={outlineWeight:0};','m.roughness=old.name===\'203-window\' && old instanceof THREE.MeshStandardMaterial ? old.roughness : .78;m.userData={outlineWeight:0};');fs.writeFileSync(ip,s);
