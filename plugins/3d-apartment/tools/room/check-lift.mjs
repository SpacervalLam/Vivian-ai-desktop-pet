import{createRequire}from'node:module';const require=createRequire(import.meta.url);const{chromium}=require(process.env.PLAYWRIGHT_PATH);const b=await chromium.launch({channel:'chrome',headless:true});const p=await b.newPage();await p.goto('http://127.0.0.1:1420/plugins/3d-apartment/tools/room/export-base.html');const result=await p.evaluate(async()=>{
 const THREE=await import('/node_modules/three/build/three.module.js');const{buildApartmentShell,APT_WALL_BOXES}=await import('/plugins/3d-apartment/src/anime/exterior.ts');const{buildBoxColliders}=await import('/plugins/3d-apartment/src/anime/collider.ts');const{FPSControls}=await import('/plugins/3d-apartment/src/anime/fpsControls.ts');const{createApartmentLift,liftTravelMs}=await import('/plugins/3d-apartment/src/anime/apartmentLift.ts');
 const camera=new THREE.PerspectiveCamera();const fps=new FPSControls(camera,document.body);const lift=createApartmentLift(document.body,camera,fps);const {group,boxes}=buildApartmentShell();const cs=[...buildBoxColliders([...APT_WALL_BOXES,...boxes]),...lift.colliders];for(const y of [0,3.4,6.2,9])cs.push({min:new THREE.Vector3(-31,y-.1,-7.15),max:new THREE.Vector3(31,y,6.3),kind:'floor'});
 fps.isLocked=true;fps.setPosition(0,1.662,6.4);const route=[];function walk(x,z){fps.stopMotion();for(let n=0;n<2600;n++){const p=fps.getPosition(),dx=x-p.x,dz=z-p.z;if(Math.hypot(dx,dz)<.055){fps.stopMotion();route.push(p);return;}fps.setRotation(Math.atan2(-dx,-dz),0);fps.moveForward=true;fps.update(1/60,cs);}throw Error('Blocked toward '+JSON.stringify({x,z,actual:fps.getPosition()}));}
 walk(0,0);walk(28,0);walk(-32.2,0);walk(-34.6,0);walk(-34.6,-2.4);
 lift.update(1000,true);if(!lift.selectFloor(3,1000))throw Error('Selection rejected');const before=fps.getPosition();if(lift.selectFloor(1,1200))throw Error('Duplicate accepted');lift.update(1000+liftTravelMs(0,3)-1,true);if(fps.getPosition().y!==before.y)throw Error('Early teleport');lift.update(1000+liftTravelMs(0,3),true);const arrived=fps.getPosition();if(Math.abs(arrived.y-10.662)>.001)throw Error('Wrong landing');
 walk(-34.6,0);walk(-32.2,0);walk(-32.2,-6.55);walk(-29.6,-6.55);walk(4.9,-6.55);
 fps.setPosition(-34.6,10.662,-2.4);lift.update(20000,true);lift.selectFloor(0,20000);lift.update(20000+liftTravelMs(3,0),true);const returned=fps.getPosition();
 lift.selectFloor(1,30000);lift.update(30100,false);lift.update(40000,true);if(lift.getState().busy||fps.getPosition().y!==returned.y)throw Error('Cancel failed');const pairs=[];let stamp=50000;
 for(let from=0;from<4;from++)for(let to=0;to<4;to++){
   fps.setPosition(-34.6,[0,3.4,6.2,9][from]+1.662,-2.4);lift.update(stamp,true);
   const accepted=lift.selectFloor(to,stamp);if(accepted!==(from!==to))throw Error('Invalid same-floor selection');
   if(accepted){lift.update(stamp+liftTravelMs(from,to),true);if(Math.abs(fps.getPosition().y-([0,3.4,6.2,9][to]+1.662))>.001)throw Error('Wrong floor pair');pairs.push([from+1,to+1]);}
   stamp+=10000;
 }
 fps.setPosition(0,1.662,0);lift.update(stamp,true);if(lift.selectFloor(2,stamp))throw Error('Outside cabin accepted');
 lift.dispose();
 return{route,arrived,returned,pairs,durations:[liftTravelMs(0,1),liftTravelMs(0,3)],oldCoreCount:group.children.filter(c=>c.name==='apt-ground').length};
});console.log(JSON.stringify(result,null,2));await b.close();
