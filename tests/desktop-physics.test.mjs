import assert from 'node:assert/strict';
import { build } from 'esbuild';
const result = await build({entryPoints:['src/chibi/desktopPhysics.ts'],bundle:true,write:false,platform:'node',format:'esm'});
const {DesktopPhysics:Physics,petFootprint,petBody,ceilingDizzyMs} = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
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
pet=create({id:'pet',x:200,y:10,width:40,height:60});pet.release(0,-900);
let hit=pet.step(.016);assert.ok(hit.ceilingImpact>600);assert.ok(pet.vy>0,'top collision still bounces');assert.ok(ceilingDizzyMs(hit.ceilingImpact,1)>=1400);
pet=create({id:'pet',x:200,y:1,width:40,height:60});pet.release(0,-100);hit=pet.step(.016);
assert.ok(hit.ceilingImpact>0);assert.equal(ceilingDizzyMs(hit.ceilingImpact,1),null,'gentle top collision is not dizzy');
for(const [body,vx,vy] of [[{x:1,y:200},-1500,0],[{x:950,y:200},1500,0],[{x:200,y:530},0,1500],[{x:200,y:-10},0,-1500]]){
 pet=create({id:'pet',...body,width:40,height:60});pet.release(vx,vy);hit=pet.step(.016);
 assert.equal(hit.ceilingImpact,0,'side/bottom collision or out-of-bounds correction cannot trigger dizzy');
}
pet=create({id:'pet',x:200,y:260,width:40,height:60},[roof]);pet.release(0,-900);hit=pet.step(.016);
assert.ok(hit.ceilingImpact>600,'an upward collision with an enclosing window ceiling also counts');
for(const scale of [1,1.25,1.5,2]){
 assert.equal(ceilingDizzyMs(599*scale,scale),null);assert.equal(ceilingDizzyMs(600*scale,scale),1400);
 assert.equal(ceilingDizzyMs(1600*scale,scale),2400);
}
for(const bad of [NaN,Infinity,-1])assert.equal(ceilingDizzyMs(bad,1),null);
assert.equal(ceilingDizzyMs(1000,0),null);
// A body fits on these roofs, but its window extends above the screen on the upper one.
for(const scale of [1,1.25,1.5,2]) {
 const scaled=r=>({...r,x:r.x*scale,y:r.y*scale,width:r.width*scale,height:r.height*scale});
 const area=scaled({id:'screen',x:-1000,y:-300,width:1000,height:900});
 const high=scaled({id:'too-high',x:-900,y:-220,width:300,height:30});
 const lower=scaled({id:'lower',x:-900,y:-120,width:300,height:100});
 pet=new Physics(scaled({id:'pet',x:-850,y:-270,width:40,height:60}),scale,30*scale);
 pet.updateWorld({windows:[high,lower],floors:[area]},.1);advance(pet);
 assert.equal(pet.support,'lower','skip the roof that would hide the top of the pet window');
 assert.ok(pet.body.y-pet.windowTopOffset>=area.y,'full window remains on-screen');
 pet.updateWorld({windows:[high,{...lower,y:area.y+70*scale}],floors:[area]},.1);advance(pet);
 assert.equal(pet.support,'screen','a supported window moved too high releases the pet to the next safe floor');
}
pet=new Physics({id:'pet',x:160,y:30,width:40,height:60},1,30);
pet.updateWorld({windows:[{id:'exact',x:100,y:90,width:300,height:100}],floors:[floor]},.1);advance(pet);
assert.equal(pet.support,'exact');assert.equal(pet.body.y-pet.windowTopOffset,0,'exactly flush with the screen top remains a valid roof');
for(const scale of [1,1.25,1.5,2]) {
 const area={id:'screen',x:-1000*scale,y:-300*scale,width:1000*scale,height:900*scale};
 pet=new Physics({id:'pet',x:-850*scale,y:-200*scale,width:40*scale,height:60*scale},scale,30*scale);
 const lowRoof={id:'low-roof',x:-900*scale,y:590*scale,width:300*scale,height:100*scale};
 pet.updateWorld({windows:[lowRoof],floors:[area]},.1);advance(pet);
 assert.equal(pet.support,'low-roof','a roof just above the screen bottom can support the feet');
 assert.equal(pet.body.y+pet.body.height,590*scale);
 pet.updateWorld({windows:[{...lowRoof,id:'offscreen-roof',y:610*scale}],floors:[area]},.1);advance(pet);
 assert.equal(pet.support,'screen');assert.equal(pet.body.y+pet.body.height,600*scale);
 assert.equal(pet.lane.groundY,600*scale,'walking uses the same screen-bottom landing baseline');
}
pet=new Physics({id:'pet',x:160,y:300,width:40,height:60},1,30);
pet.updateWorld({windows:[{id:'container',x:100,y:250,width:300,height:500}],floors:[floor]},.1);pet.release(0,0);advance(pet);
assert.equal(pet.enclosure,'container');
assert.equal(pet.body.y+pet.body.height,600,'the screen bottom limits the feet inside an off-screen container');
assert.equal(pet.lane.groundY,600);
const partial={id:'partial-cover',x:195,y:220,width:100,height:180};
pet=create({id:'pet',x:160,y:0,width:40,height:60},[partial,roof]);advance(pet);
assert.equal(pet.support,'floor','an uncovered center point cannot support feet that overlap a hidden edge');
pet=create({id:'pet',x:120,y:0,width:40,height:60},[partial,roof]);advance(pet);
assert.equal(pet.support,'roof');assert.equal(pet.lane.minX,100);assert.equal(pet.lane.maxX,155,'walking stops before the occluded segment');
const supportedX=pet.body.x;
pet.updateWorld({windows:[roof,partial],floors:[floor]},.1);
assert.equal(pet.body.x,supportedX);assert.equal(pet.lane.maxX,360,'a window behind the support does not occlude its edge');
const cover={id:'cover',x:100,y:220,width:300,height:180};
const lowerEdge={id:'lower-edge',x:100,y:520,width:300,height:70};
pet.updateWorld({windows:[cover,roof,lowerEdge],floors:[floor]},.1);
assert.equal(pet.support,null,'covering a stationary platform releases its support immediately');assert.equal(pet.lane,null);
advance(pet);assert.equal(pet.support,'lower-edge','fall through the hidden roof to the next visible edge');
pet=create({id:'pet',x:120,y:0,width:40,height:60},[roof]);advance(pet);
assert.equal(pet.hasVisibleSupport({windows:[cover,roof],floors:[floor]}),false,'walking detects Z-order occlusion even when the platform geometry is unchanged');
pet=create({id:'pet',x:160,y:0,width:40,height:60},[{...cover,landable:false},roof]);advance(pet);
assert.equal(pet.support,'floor','borderless foreground windows obscure roofs without creating landing platforms');
// 桌宠之间是实体：落到另一只桌宠身上时被水平推开，不会叠在一起，也不会被顶到头上。
const nana={id:'pet-nana',x:120,y:480,width:80,height:120,landable:false,pet:'nana'};
const apart=(a,b)=>a.x+a.width<=b.x||a.x>=b.x+b.width;
pet=create(undefined,[nana]);advance(pet);
assert.ok(apart(pet.body,nana),'a falling pet is pushed beside its neighbour instead of overlapping it');
assert.equal(pet.support,'floor','separation keeps the pet on the ground');
// 走向另一只桌宠时被挡住并反弹，任何一帧都不允许重叠。
pet=create({id:'pet',x:160,y:540,width:40,height:60},[nana]);pet.vx=400;
for(let i=0;i<40;i++){pet.step(.016);assert.ok(apart(pet.body,nana),'walking pets never interpenetrate');}
// 不同高度（未重叠）的桌宠不互相推挤。
const high={id:'pet-vivian',x:160,y:0,width:80,height:120,landable:false,pet:'vivian'};
pet=create({id:'pet',x:160,y:540,width:40,height:60},[high]);pet.step(.016);
assert.equal(pet.body.x,160,'vertically separated pets are left alone');
// 整窗矩形换算成身体矩形，避免按透明留白碰撞。
const windowRect={id:'pet-nana',x:100,y:200,width:213.2,height:246.8,landable:false,pet:'nana'};
const body=petBody(windowRect);assert.ok(body.x>100&&body.y>200&&body.width<213.2&&body.height<246.8&&body.pet==='nana');
// 中心完全重合的两只桌宠按 label 分向，各自朝反方向让开而不是同步滑向一侧。
const vivianBody={id:'vivian',x:160,y:540,width:40,height:60};
const nanaBody={id:'nana',x:140,y:520,width:80,height:80};
const vivian=create(vivianBody,[{...nanaBody,landable:false,pet:'nana'}]);vivian.step(.016);
const nanaPet=create(nanaBody,[{...vivianBody,landable:false,pet:'vivian'}]);nanaPet.step(.016);
assert.ok(vivian.body.x>160&&nanaPet.body.x<140,'co-centred pets separate in opposite directions');
// 等质量碰撞要传递动量：撞方明显减速但继续向前，不是被弹飞。
const nanaBodyStand={id:'pet-nana',x:240,y:540,width:80,height:60,landable:false,pet:'nana'};
const striker=create({id:'pet-vivian',x:170,y:540,width:80,height:60},[nanaBodyStand]);
striker.vx=400;
striker.updateWorld({windows:[nanaBodyStand],floors:[floor]},.1);
striker.step(.016);
assert.ok(striker.vx>0&&striker.vx<400,'the striker slows down but keeps moving forward instead of bouncing back');
// 被撞的一方按动量获得速度（对方速度来自两次地形采样的位移差分，不是额外通道）。
const vivianBodyStand={id:'pet-vivian',x:170,y:540,width:80,height:60,landable:false,pet:'vivian'};
const pushed=create({id:'pet-nana',x:240,y:540,width:80,height:60},[{...vivianBodyStand,x:170}]);
pushed.updateWorld({windows:[{...vivianBodyStand,x:170}],floors:[floor]},.1);
pushed.updateWorld({windows:[{...vivianBodyStand,x:210}],floors:[floor]},.1); // 40px / 0.1s = 400px/s
pushed.step(.016);
assert.ok(pushed.vx>100,'the pushed pet gains forward speed instead of staying put');
// 实机上撞方每帧会把自己复位到「刚好接触再留 1px 间隙」，这时被撞方仍必须拿到速度：
// 只认严格重叠的话，速度交换那一支永远进不来，两只只会互相顶住、碰到就一起静止。
const neat={id:'pet-vivian',x:90,y:540,width:80,height:60,landable:false,pet:'vivian'};
const meter=create({id:'pet-nana',x:251,y:540,width:80,height:60},[{...neat,x:90}]);
meter.updateWorld({windows:[{...neat,x:90}],floors:[floor]},.1);
meter.updateWorld({windows:[{...neat,x:170}],floors:[floor]},.1); // 800px/s，落点右边缘 250 与 251 差 1px
const beforeContact=meter.body.x;
meter.step(.016);
assert.ok(meter.vx>100,'a one-pixel gap still counts as contact');
assert.ok(meter.body.x>=beforeContact,'contact must never pull the pushed pet back');
// 已经迈开的一只不能被「分离约束」重新吸附回接触位置。
const chaser={id:'pet-vivian',x:170,y:540,width:80,height:60,landable:false,pet:'vivian'};
const escaping=create({id:'pet-nana',x:249,y:540,width:80,height:60},[chaser]);
escaping.vx=300;
escaping.step(.016);
assert.ok(escaping.body.x>249,'a separating pet keeps its own position');
// 对撞：两边速度都已知时，等质量 e=0.3 会把各自弹回，而不是继续往原方向推。
const opposing={id:'pet-nana',x:280,y:540,width:80,height:60,landable:false,pet:'nana'};
const headOn=create({id:'pet-vivian',x:200,y:540,width:80,height:60},[{...opposing,x:320}]);
headOn.updateWorld({windows:[{...opposing,x:280}],floors:[floor]},.1); // 对方 -40px / 0.1s = -400px/s
headOn.vx=400;
headOn.step(.016);
assert.ok(headOn.vx<0,'head-on impact must push the left pet back');
// 垂直只重叠几像素、脚底却差着一大截（从上方擦过）不算并排接触，不能被当成墙。
const overhead={id:'pet-vivian',x:240,y:540,width:80,height:60,landable:false,pet:'vivian'};
const passer=create({id:'pet-nana',x:200,y:484,width:80,height:60},[overhead]);
passer.vx=800;
passer.step(.016);
assert.ok(passer.body.x>200,'a pet brushing overhead is not blocked like a wall');
// 贴墙的一只吃到朝墙的速度时必须被墙吸收，而不是越界一帧再弹回来。
const rammer={id:'pet-vivian',pet:'vivian',x:840,y:540,width:80,height:60,landable:false};
const pinned=create({id:'pet-nana',x:920,y:540,width:80,height:60},[{...rammer,x:840}]);
pinned.updateWorld({windows:[{...rammer,x:800}],floors:[floor]},.1);
pinned.updateWorld({windows:[{...rammer,x:840}],floors:[floor]},.1); // +400px/s 逼近
pinned.step(.016);
assert.equal(pinned.vx,0,'a pet pinned against the wall absorbs the impact');
assert.ok(pinned.body.x+80<=1000,'a pinned pet never writes an off-screen position');
// 撞向贴墙的桌宠：它无处可退，冲量从墙传回撞方，撞方应该被弹回而不是停住。
const wallTarget={id:'pet-nana',x:920,y:540,width:80,height:60,landable:false,pet:'nana'};
const bouncer=create({id:'pet-vivian',x:839,y:540,width:80,height:60},[wallTarget]);
bouncer.vx=400;
bouncer.step(.016);
assert.ok(bouncer.vx<0,'ramming a wall-pinned pet bounces the rammer back');
// 贴左墙同理，方向镜像。
const leftTarget={id:'pet-nana',x:0,y:540,width:80,height:60,landable:false,pet:'nana'};
const bouncer2=create({id:'pet-vivian',x:81,y:540,width:80,height:60},[leftTarget]);
bouncer2.vx=-400;
bouncer2.step(.016);
assert.ok(bouncer2.vx>0,'the mirrored wall bounce works the same way');
console.log('Desktop physics: floor/roof landings, platform motion/removal, container shake/shrink, both moving sides/resize, throw directions, occlusion, resume bounds, pet-to-pet separation and momentum transfer, negative monitors and DPI passed');
