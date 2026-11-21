import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/apartmentLift.ts';let s=fs.readFileSync(p,'utf8');const mark="  const leaves:Array<[THREE.Mesh,THREE.Mesh]>=[];";if(!s.includes(mark))throw Error('marker');s=s.replace(mark,`  // Close the construction joint without covering either existing wall face.
  box('south-joint',-31.06,6.075,4.73,.20,12.11,.10,stone,true);
  box('north-joint',-31.06,6.075,-7.20,.20,12.11,.10,stone,true);
  // A flush portal joins the taller cafe ceiling to the lower lift-hall ceiling.
  box('lobby-portal-head',-31.045,2.975,-.60,.29,.47,10.58,stone,true);
  box('lobby-portal-north',-31.045,1.39,-5.80,.29,2.70,.20,stone,true);
  box('lobby-portal-south',-31.045,1.39,4.59,.29,2.70,.20,stone,true);
  box('lobby-floor-joint',-31.01,.032,-.55,.02,.06,10.54,stone);
  // Ground floor only: fixed glazing prevents entering from the rear pedestrian path.
  // Upper floors retain their open connection to the north access galleries.
  const glazing=new THREE.MeshStandardMaterial({color:'#9ebeb7',roughness:.18,metalness:.12,transparent:true,opacity:.27,depthWrite:false});
  glazing.userData.outlineWeight=0;
  box('ground-rear-glass',-31.025,1.38,-6.54,.018,2.53,1.10,glazing,true).castShadow=false;
  for(const z of [-7.125,-6.54,-5.955])box('ground-rear-mullion',-31.025,1.38,z,.072,2.60,.04,metal,true);
  for(const y of [.062,2.698])box('ground-rear-transom',-31.025,y,-6.54,.072,.036,1.21,metal,true);
  box('ground-rear-spandrel',-31.025,3.015,-6.54,.09,.58,1.21,stone,true);
`+mark);fs.writeFileSync(p,s);
