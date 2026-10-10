import assert from 'node:assert/strict';
import { build } from 'esbuild';
const result = await build({entryPoints:['src/chibi/desktopPhysics.ts'],bundle:true,write:false,platform:'node',format:'esm'});
const {DesktopPhysics:Physics,petFootprint} = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const floor={id:'floor',x:0,y:0,width:1000,height:600};
const roof={id:'roof',x:100,y:250,width:300,height:250};
const create=(body={id:'pet',x:160,y:0,width:40,height:60},windows=[],scale=1)=>{
 const pet=new Physics(body,scale);pet.updateWorld({windows,floors:[floor]},.1);return pet;
};
const advance=(pet,seconds=4)=>{let impacts=[];for(let i=0;i<seconds/.016;i++){const step=pet.step(.016);if(step.impact>180*pet.scale)impacts.push(step.impact);}return impacts;};
let pet=create();assert.ok(advance(pet).length>0,'gravity lands with elastic impact');assert.equal(pet.support,'floor');assert.equal(pet.body.y,540);
pet=create(undefined,[roof]);advance(pet);assert.equal(pet.support,'roof');assert.equal(pet.body.y,190);
pet.updateWorld({windows:[{...roof,x:130,y:280}],floors:[floor]},.1);assert.equal(pet.body.x,190);assert.equal(pet.body.y,220,'moving platforms carry the supported body');
pet.updateWorld({windows:[],floors:[floor]},.1);advance(pet);assert.equal(pet.support,'floor','closed/minimized platform releases support');
pet=create({id:'pet',x:160,y:300,width:40,height:60},[roof]);pet.release(0,0);assert.equal(pet.enclosure,'roof');advance(pet);assert.equal(pet.body.y,436);assert.equal(pet.support,'roof');
pet.updateWorld({windows:[{...roof,x:260}],floors:[floor]},.1);pet.step(.016);assert.ok(pet.body.x>=264);assert.ok(pet.vx>0,'moving enclosure wall transfers momentum');
for(let i=0;i<100;i++){pet.step(.016);assert.ok(pet.body.x>=264&&pet.body.x+40<=556);assert.ok(pet.body.y>=254&&pet.body.y+60<=496);}
pet.updateWorld({windows:[{...roof,width:20,height:20}],floors:[floor]},.1);assert.equal(pet.enclosure,null,'a too-small resized box releases the pet');
pet=create({id:'pet',x:150,y:220,width:40,height:60},[{id:'side',x:0,y:200,width:100,height:200}]);pet.updateWorld({windows:[{id:'side',x:200,y:200,width:100,height:200}],floors:[floor]},.1);assert.ok(pet.vx>0&&pet.vy<0,'right edge sweep knocks the pet airborne');
pet=create({id:'pet',x:320,y:220,width:40,height:60},[{id:'side',x:400,y:200,width:100,height:200}]);pet.updateWorld({windows:[{id:'side',x:250,y:200,width:100,height:200}],floors:[floor]},.1);assert.ok(pet.vx<0&&pet.vy<0,'left edge sweep knocks left');
pet=create({id:'pet',x:130,y:220,width:40,height:60},[{id:'side',x:50,y:200,width:50,height:200}]);pet.updateWorld({windows:[{id:'side',x:50,y:200,width:200,height:200}],floors:[floor]},.1);assert.ok(pet.vx>0,'resizing the right wall also collides');
pet=create({id:'pet',x:150,y:140,width:40,height:60},[{id:'side',x:0,y:200,width:100,height:200}]);pet.updateWorld({windows:[{id:'side',x:200,y:200,width:100,height:200}],floors:[floor]},.1);assert.equal(pet.vx,0,'standing on top cannot become a side hit');
pet=create({id:'pet',x:160,y:350,width:40,height:60});pet.release(0,-500);pet.step(.05);assert.ok(pet.body.y<350,'upward throw rises');advance(pet);assert.equal(pet.support,'floor');
pet=create({id:'pet',x:160,y:350,width:40,height:60});pet.release(500,-500);for(let i=0;i<10;i++)pet.step(.016);assert.ok(pet.body.x>160&&pet.body.y<350,'diagonal throw has both components');
pet=create({id:'pet',x:160,y:350,width:40,height:60});pet.release(500,0);pet.step(.05);assert.ok(pet.body.x>160&&pet.body.y>350,'horizontal throw still falls under gravity');
pet=create();pet.step(10);assert.ok(pet.body.y<10,'sleep/resume does not integrate a huge stale frame');
const front={id:'front',x:80,y:180,width:360,height:400};pet=create(undefined,[front,roof]);advance(pet);assert.equal(pet.support,'front','a covered roof is not a landing surface');
pet=create({id:'pet',x:160,y:130,width:40,height:60},[{...roof,y:240},{...roof,id:'upper',y:210}]);pet.release(0,1000);pet.step(.05);assert.equal(pet.body.y,150,'when a frame crosses two exposed roofs, land on the first crossed roof');
for(const scale of [1,1.25,1.5,2]){
 const scaled=r=>({...r,x:r.x*scale,y:r.y*scale,width:r.width*scale,height:r.height*scale});
 pet=new Physics(scaled({id:'pet',x:-800,y:0,width:40,height:60}),scale);pet.updateWorld({windows:[scaled({...roof,x:-900})],floors:[scaled({...floor,x:-1000})]},.1);advance(pet);assert.equal(pet.support,'roof');assert.ok(Math.abs(pet.body.y/scale-190)<1e-6);
}
for(const [width,height] of [[300,300],[400,300],[200,400]]){const f=petFootprint(width,height);assert.ok(f.left>=0&&f.top>=0&&f.left+f.width<=width&&f.top+f.height<=height);}
console.log('Desktop physics: floor/roof landings, platform motion/removal, container shake/shrink, both moving sides/resize, throw directions, occlusion, resume bounds, negative monitors and DPI passed');
