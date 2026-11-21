import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/apartmentPodium.ts';let s=fs.readFileSync(p,'utf8');s=s.replace("x,.79,z-.31,width,.48,.16","x,.82,z-.31,width-.06,.48,.16").replace("x,1.63,-4.865","x,1.63,-4.955").replace("x,.037,-5.39,12.28","x,.037,-5.39,12.22").replace("seat(x,-5.1,2.9,sage)","seat(x,-5.49,2.9,sage)").replace("x,1.6,-4.92","x,1.6,-5.015").replace(".612+row*.73",".642+row*.73");s=s.replace("12.28,.05,4.43,stone);","12.28,.05,4.43,stone,true);").replace("11.4,.035,1.50,stone);","11.4,.035,1.50,stone,true);");
const marker='  // Rear amenities replace';const idx=s.indexOf(marker);s=s.slice(0,idx)+`  // Two small multi-stem garden trees sit within the entrance planting pockets.
  const branchGeo=new THREE.CylinderGeometry(.022,.035,1,7);
  for(const x of [-8,8]){
    for(let i=0;i<3;i++){
      const a=new THREE.Vector3(x,.52,5.44),b=new THREE.Vector3(x+(i-1)*.30,1.80+i*.13,5.44+(i%2-.5)*.28);
      const stem=new THREE.Mesh(branchGeo,wood);stem.position.copy(a).add(b).multiplyScalar(.5);stem.scale.y=a.distanceTo(b);stem.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),b.clone().sub(a).normalize());stem.name='podium-garden-stem';group.add(stem);
      for(let k=0;k<4;k++){
        const crown=new THREE.Mesh(leafGeo,leaves);crown.position.set(b.x+Math.cos(k*2.4)*.26,b.y+.13+Math.sin(k*2.4)*.16,b.z+Math.sin(k*2.4)*.23);crown.scale.set(.34,.18,.30);crown.castShadow=true;crown.name='podium-garden-canopy';group.add(crown);
      }
    }
  }
`+s.slice(idx);fs.writeFileSync(p,s);
