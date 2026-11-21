import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';
import { buildCityRoutes } from './cityRoutes';

/** Reusable, original street assets: Mediterranean shop houses, tea market and food-truck court. */
export function createWorldStreetLife(scene:THREE.Scene){
  const root=new THREE.Group();root.name='world-street-life';root.userData.sceneCollideSkip=true;scene.add(root);
  const colliders:Collider[]=[],materials:THREE.Material[]=[],textures:THREE.Texture[]=[];const geometries=new Set<THREE.BufferGeometry>();
  const mat=(color:string,light=false)=>{const m=new THREE.MeshStandardMaterial({color,roughness:.87,...(light?{emissive:color,emissiveIntensity:.7}:{})});m.userData.outlineWeight=0;materials.push(m);return m;};
  const M={cream:mat('#e2d1ac'),salmon:mat('#c98b70'),sage:mat('#9fac93'),green:mat('#315b50'),red:mat('#a84e40'),stone:mat('#b4b2a2'),wood:mat('#886044'),iron:mat('#384747'),glass:mat('#8eaaa7'),paper:mat('#f5e5c6'),blue:mat('#7997ac'),yellow:mat('#debc65'),leaf:mat('#648052'),clay:mat('#b36e50'),lamp:mat('#ffdda0',true)};
  const cube=new THREE.BoxGeometry(1,1,1),cyl=new THREE.CylinderGeometry(1,1,1,12),sphere=new THREE.SphereGeometry(1,10,8),ring=new THREE.TorusGeometry(1,.07,6,24);[cube,cyl,sphere,ring].forEach(g=>geometries.add(g));
  function mesh(g:THREE.BufferGeometry,m:THREE.Material,x:number,y:number,z:number,a=1,b=1,c=1){const o=new THREE.Mesh(g,m);o.position.set(x,y,z);o.scale.set(a,b,c);o.castShadow=true;o.receiveShadow=true;root.add(o);return o;}
  const box=(x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material)=>mesh(cube,m,x,y,z,w,h,d);
  function block(name:string,x:number,y:number,z:number,w:number,h:number,d:number,m:THREE.Material,kind:Collider['kind']='wall'){box(x,y,z,w,h,d,m);colliders.push({source:`world-${name}`,kind,min:new THREE.Vector3(x-w/2,y-h/2,z-d/2),max:new THREE.Vector3(x+w/2,y+h/2,z+d/2)});}
  function rod(a:number[],b:number[],r:number,m:THREE.Material){const v=new THREE.Vector3(...b).sub(new THREE.Vector3(...a));const o=mesh(cyl,m,a[0]+v.x/2,a[1]+v.y/2,a[2]+v.z/2,r,v.length(),r);o.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),v.normalize());}
  function sign(title:string,sub:string,x:number,y:number,z:number,w:number,h:number,bg='#315b50'){
    const cv=document.createElement('canvas');cv.width=768;cv.height=Math.max(128,Math.round(768*h/w));const c=cv.getContext('2d')!;c.fillStyle=bg;c.fillRect(0,0,768,cv.height);c.strokeStyle='#e8d2a5';c.lineWidth=3;c.strokeRect(10,10,748,cv.height-20);c.fillStyle='#f6ead1';c.textAlign='center';c.font=`600 ${Math.min(80,cv.height*.42)}px Georgia,"Microsoft YaHei",serif`;c.fillText(title,384,cv.height*.46,710);c.font=`${Math.min(29,cv.height*.17)}px sans-serif`;c.fillText(sub,384,cv.height*.8,700);
    const t=new THREE.CanvasTexture(cv);t.colorSpace=THREE.SRGBColorSpace;textures.push(t);const m=new THREE.MeshStandardMaterial({map:t,roughness:.9});m.userData.outlineWeight=0;materials.push(m);const g=new THREE.PlaneGeometry(w,h);geometries.add(g);mesh(g,m,x,y,z).rotation.y=Math.PI;
  }
  function pot(x:number,y:number,z:number,r=.2){mesh(cyl,M.clay,x,y+r*.65,z,r,r*1.3,r);mesh(cyl,M.wood,x,y+r*1.32,z,r*.85,.025,r*.85);for(let i=0;i<5;i++){const a=i*1.257;const px=x+Math.cos(a)*r*.75,pz=z+Math.sin(a)*r*.75;rod([x,y+r,z],[px,y+r*2.7,pz],.014,M.leaf);mesh(sphere,i%2?M.paper:M.salmon,px,y+r*2.7,pz,r*.28,r*.25,r*.28);}}
  function table(x:number,z:number){rod([x,.06,z],[x,.78,z],.05,M.iron);mesh(cyl,M.wood,x,.8,z,.48,.06,.48);for(const dx of [-.8,.8]){block('chair',x+dx,.45,z,.42,.85,.45,M.green);box(x+dx,.91,z+.17,.42,.38,.045,M.green);}mesh(cyl,M.paper,x,.88,z,.08,.14,.08);pot(x+.2,.84,z,.06);}
  for(const g of buildCityRoutes(root,colliders,[
    {name:'cafe-court-lane',points:[[-45,85],[-35,84],[-25,84],[-13,85],[-2,86]],width:3.2,walk:true},
    {name:'tea-market-connection',points:[[12,85],[19,87],[25,87],[30,89],[35,92]],width:2.5,walk:true},
    {name:'food-court-connection',points:[[43,83],[53,88],[55,99],[55,108],[48,112]],width:2.6,walk:true},
  ],{road:M.stone,paving:M.stone,paint:M.paper},.06))geometries.add(g);
  // Mediterranean shopfronts with true open arched doorways, rusticated corners and shutters.
  for(const [i,x] of [-30,-24,-18].entries()){
    const z=90.8,h=9.2+i*.4,wall=[M.cream,M.salmon,M.sage][i];
    block('cafe-floor',x,.065,z,5.7,.13,6.2,M.stone,'floor');
    block('house-upper',x,(3.2+h)/2,z,5.7,h-3.2,6.2,wall);
    for(const dx of [-2.75,2.75])block('house-side',x+dx,1.6,z,.2,3.2,6.2,wall);
    block('house-back',x,1.6,z+3,5.7,3.2,.2,wall);
    // Two front bays. The right-hand door remains walkable.
    block('facade-pier',x-.35,1.3,z-3,.28,2.6,.4,wall);block('facade-left',x-2.57,1.3,z-3,.35,2.6,.4,wall);
    box(x-1.45,1.3,z-3.05,1.78,2.3,.06,M.glass);
    for(let bay=0;bay<2;bay++){
      const cx=x-1.45+bay*2.7;
      for(let n=0;n<12;n++){const a=Math.PI*n/11;const o=box(cx+Math.cos(a)*1.04,2.12+Math.sin(a)*1.04,z-3.13,.28,.27,.3,M.cream);o.rotation.z=a;}
    }
    for(let y=3.9;y<h-.6;y+=2.55)for(const dx of [-1.4,1.4]){
      const px=x+dx;box(px,y,z-3.14,1.14,1.75,.11,M.wood);box(px,y,z-3.22,.96,1.58,.03,M.glass);
      for(const side of [-1,1]){box(px+side*.78,y,z-3.15,.42,1.76,.1,M.green);for(let slat=0;slat<12;slat++)box(px+side*.78,y-.77+slat*.14,z-3.23,.39,.035,.06,M.green);}
      box(px,y-1,z-3.5,1.9,.16,1.05,M.cream);for(let r=0;r<9;r++)rod([px-.85+r*.21,y-.95,z-4],[px-.85+r*.21,y-.15,z-4],.015,M.iron);rod([px-.92,y-.1,z-4],[px+.92,y-.1,z-4],.024,M.iron);pot(px+.58,y-.91,z-3.73,.12);
    }
    for(let y=.35;y<h;y+=.5)for(const dx of [-2.72,2.72])box(x+dx,y,z-3.16,.32,.23,.17,M.cream);
    box(x,h+.12,z,6.05,.24,6.55,M.cream);box(x,h+.28,z,5.8,.12,6.3,M.red);
    sign(['CAFFÈ LUCIA','FORNO & PIZZA','ATELIER VERDE'][i],['ESPRESSO · DOLCI','FORNO A LEGNA','FLOWERS · HANDMADE'][i],x,3.35,z-3.32,5.1,.65,i===1?'#a84e40':'#315b50');
    for(let n=0;n<14;n++){box(x-2.6+n*.4,2.8,z-3.72,.39,.07,1.15,n%2?M.paper:i===1?M.red:M.green);box(x-2.6+n*.4,2.64,z-4.28,.39,.28,.045,n%2?M.paper:i===1?M.red:M.green);}
    block('counter',x-.9,.55,z+1.7,2.5,1.1,.65,M.wood);for(let n=0;n<7;n++)mesh(sphere,M.yellow,x-1.9+n*.32,1.18,z+1.7,.12,.07,.12);
    for(const dx of [-2,2])pot(x+dx,.07,z-4.3,.22);
    table(x-1.2,z-5.45);
    sign('MENU','COFFEE  /  PIZZA  /  DAILY SPECIAL',x-2.15,.8,z-4.7,.55,.85,'#344640');
  }
  // Overhead laundry and festoon lights create a lived-in roofline above the walking route.
  for(const z of [84,86]){
    rod([-33,6,z],[-14,6,z],.018,M.iron);
    for(let i=0;i<9;i++){const x=-32+i*2;box(x,5.53,z,.65,.92,.025,[M.paper,M.blue,M.salmon,M.yellow][i%4]);box(x-.21,5.97,z,.035,.09,.04,M.wood);box(x+.21,5.97,z,.035,.09,.04,M.wood);}
    for(const x of [-33,-14])rod([x,0,z],[x,6.2,z],.045,M.iron);
  }
  // Chinese tea market: tiled eaves, hanging lanterns, bamboo trays and fruit crates.
  for(const [i,x] of [21,26].entries()){
    const z=91;block('market-pad',x,.07,z,4.4,.14,4,M.stone,'floor');
    for(const dx of [-1.9,1.9])for(const dz of [-1.4,1.4])block('stall-post',x+dx,1.45,z+dz,.12,2.9,.12,M.wood);
    for(const side of [-1,1]){const o=box(x,3,z+side*.9,4.8,.13,2.1,M.iron);o.rotation.x=side*.22;for(let tile=0;tile<24;tile++)box(x-2.3+tile*.2,3.05,z+side*.9,.055,.04,2.05,M.iron);}
    sign(i?'四季果铺':'春和茶铺',i?'SEASONAL FRUIT':'TEA · DIM SUM',x,2.53,z-1.58,3.5,.47,'#785343');
    block('market-counter',x,.58,z-1,3.6,1.16,.75,M.wood);
    for(let n=0;n<12;n++){const px=x-1.55+(n%6)*.61,pz=z-1.17+Math.floor(n/6)*.3;mesh(cyl,M.clay,px,1.2,pz,.23,.07,.23);mesh(sphere,i?(n%2?M.red:M.yellow):M.paper,px,1.33,pz,.16,.13,.16);}
    for(const dx of [-1.65,1.65]){rod([x+dx,2.85,z-1.55],[x+dx,2.43,z-1.55],.012,M.iron);mesh(sphere,M.red,x+dx,2.19,z-1.55,.22,.3,.22);box(x+dx,1.84,z-1.55,.055,.18,.05,M.yellow);}
  }
  // American-style silver food trailer and a street newsstand beside the mall approach.
  block('food-trailer',59,1.35,98,3.2,2.2,6,M.paper);box(59,2.55,98,3.4,.2,6.2,M.stone);
  for(const z of [96.2,99.8])for(const x of [57.3,60.7]){const o=mesh(cyl,M.iron,x,.44,z,.4,.2,.4);o.rotation.z=Math.PI/2;}
  box(59,1.7,94.95,2.5,.85,.055,M.iron);box(59,1.19,94.65,2.9,.1,.7,M.wood);
  sign('PARKSIDE DINER','BURGERS  ·  COFFEE  ·  GOOD TIMES',59,2.42,94.83,3.15,.48,'#a84e40');
  for(let i=0;i<10;i++)box(57.5+i*.33,2.23,94.45,.32,.08,1,i%2?M.paper:M.red);
  for(const x of [58,61])table(x,92.8);
  block('news-kiosk',64,1.25,104,2.7,2.5,2.4,M.green);box(64,2.65,104,3.2,.25,2.9,M.iron);sign('CITY NEWS','PAPERS · MAPS · MAGAZINES',64,2.23,102.72,2.6,.38);
  for(let row=0;row<3;row++)for(let col=0;col<5;col++){const x=63+col*.5,y=.5+row*.48;box(x,y,102.74,.42,.41,.04,[M.paper,M.salmon,M.blue][(row+col)%3]);box(x,y+.06,102.7,.28,.055,.015,M.iron);}
  // A small scooter with wheels, fork, headlight and rear rack.
  for(const [x,z] of [[-12.5,88],[54,101]]){
    block('scooter',x,.55,z,.55,1.1,1.5,M.sage);for(const dz of [-.65,.65]){const o=mesh(cyl,M.iron,x,.3,z+dz,.28,.14,.28);o.rotation.z=Math.PI/2;}box(x,.86,z,.46,.1,.6,M.wood);rod([x,.45,z-.6],[x,1.2,z-.55],.04,M.iron);rod([x-.3,1.2,z-.55],[x+.3,1.2,z-.55],.025,M.iron);mesh(sphere,M.lamp,x,1.1,z-.62,.13,.13,.08);
  }
  mergeByMaterial(root);
  return {colliders,dispose(){root.removeFromParent();root.traverse(o=>{if(o instanceof THREE.Mesh)geometries.add(o.geometry);});for(const g of geometries)g.dispose();materials.forEach(m=>m.dispose());textures.forEach(t=>t.dispose());}};
}
