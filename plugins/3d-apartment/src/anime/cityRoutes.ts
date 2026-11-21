import * as THREE from 'three';
import type { Collider } from './collider';

export type CityRoute={name:string;points:[number,number][];width:number;walk?:boolean};
/** Anchors retain the apartment entrances and level crossing; local streets end in T-junctions. */
export const CITY_ROUTES:CityRoute[]=[
  {name:'station-high-street',points:[[-80,10.1],[-46,10.1],[0,10.1],[43,10.1],[80,10.1]],width:5},
  {name:'apartment-north-street',points:[[-80,-19],[-46,-19],[0,-19],[43,-19],[80,-19]],width:6},
  {name:'west-neighbourhood-avenue',points:[[-46,-78],[-46,-44],[-46,-19],[-46,10],[-47,30],[-44,47],[-46,65],[-46,83]],width:6},
  {name:'east-station-avenue',points:[[43,-78],[43,-44],[43,-19],[43,10],[43,30],[45,46],[42,63],[43,83]],width:6},
  {name:'north-local-west',points:[[-80,-44],[-66,-45],[-54,-44],[-46,-44]],width:4.6},
  {name:'north-local-east',points:[[-46,-44],[-20,-44],[9,-47],[29,-46],[43,-44]],width:4.6},
  {name:'riverside-lane',points:[[-46,-69],[-27,-72],[0,-70],[24,-67],[43,-69]],width:4.4},
  {name:'west-market-lane',points:[[-80,30],[-65,32],[-54,32],[-47,30]],width:4.6},
  {name:'shrine-garden-promenade',points:[[-47,30],[-24,30],[-8,32],[12,32],[30,30],[43,30]],width:4,walk:true},
  {name:'station-garden-link',points:[[43,30],[59,32],[73,30],[78,32]],width:3.6,walk:true},
  {name:'south-residential-lane',points:[[-80,61],[-66,59],[-53,60],[-46,65],[-29,64],[-16,61],[0,60],[19,62]],width:4.6},
  {name:'south-east-promenade',points:[[19,62],[29,60],[42,63],[59,61],[76,64]],width:3.6,walk:true},
];
export const SOUTH_ROUTES:CityRoute[]=[
  {name:'campus-approach',points:[[-46,83],[-43,91],[-38,99],[-34,106]],width:5},
  {name:'campus-gate-walk',points:[[-34,106],[-34,112],[-34,118.8]],width:4,walk:true},
  {name:'mall-boulevard',points:[[43,83],[38,91],[29,96],[24,103],[29,108],[40,109]],width:6},
  {name:'civic-shared-street',points:[[-70,104],[-52,104],[-34,106],[-15,103],[3,98],[15,99],[24,103]],width:4.5},
  {name:'park-campus-greenway',points:[[0,55],[4,63],[3,77],[-1,87],[-15,97],[-27,108],[-34,114]],width:3.2,walk:true},
  {name:'garden-mall-walk',points:[[3,77],[12,85],[16,96],[12,107],[22,113],[40,115]],width:3.4,walk:true},
];

/** Continuous sampled ribbons, with matching small floor patches rather than a city-wide bounding box. */
export function buildCityRoutes(root:THREE.Group,colliders:Collider[],routes:CityRoute[],materials:{road:THREE.Material;paving:THREE.Material;paint:THREE.Material},baseY=.008){
  const geometries:THREE.BufferGeometry[]=[];
  for(const route of routes){
    const curve=new THREE.CatmullRomCurve3(route.points.map(([x,z])=>new THREE.Vector3(x,0,z)),false,'centripetal');
    const length=curve.getLength(),count=Math.max(12,Math.ceil(length/.9));
    const samples=curve.getSpacedPoints(count),normals=samples.map((_p,i)=>{const v=samples[Math.min(i+1,count)].clone().sub(samples[Math.max(i-1,0)]).normalize();return new THREE.Vector3(-v.z,0,v.x);});
    function ribbon(name:string,left:number,right:number,y:number,mat:THREE.Material){
      const positions:number[]=[],uv:number[]=[],indices:number[]=[];
      for(let i=0;i<=count;i++){for(const offset of [left,right]){const p=samples[i].clone().addScaledVector(normals[i],offset);positions.push(p.x,y,p.z);uv.push(p.x/2,p.z/2);}if(i<count){const a=i*2;indices.push(a,a+1,a+2,a+1,a+3,a+2);}}
      const geo=new THREE.BufferGeometry();geo.setAttribute('position',new THREE.Float32BufferAttribute(positions,3));geo.setAttribute('uv',new THREE.Float32BufferAttribute(uv,2));geo.setIndex(indices);geo.computeVertexNormals();geometries.push(geo);
      const mesh=new THREE.Mesh(geo,mat);mesh.name=name;mesh.receiveShadow=true;root.add(mesh);
    }
    const half=route.width/2,outer=half+(route.walk?.35:1.8);
    ribbon(route.name+'-walk',-outer,outer,baseY,materials.paving);
    if(!route.walk){ribbon(route.name,-half,half,baseY+.004,materials.road);for(const side of [-1,1])ribbon(route.name+'-edge',side*half-.035,side*half+.035,baseY+.007,materials.paint);}
    for(let i=0;i<count;i++){
      const corners=[samples[i].clone().addScaledVector(normals[i],outer),samples[i].clone().addScaledVector(normals[i],-outer),samples[i+1].clone().addScaledVector(normals[i+1],outer),samples[i+1].clone().addScaledVector(normals[i+1],-outer)];
      colliders.push({source:route.name,kind:'floor',min:new THREE.Vector3(Math.min(...corners.map(p=>p.x)),baseY-.02,Math.min(...corners.map(p=>p.z))),max:new THREE.Vector3(Math.max(...corners.map(p=>p.x)),baseY+.008,Math.max(...corners.map(p=>p.z)))});
    }
  }
  return geometries;
}
