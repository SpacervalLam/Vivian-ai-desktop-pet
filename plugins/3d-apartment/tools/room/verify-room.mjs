import { createRequire } from 'node:module';
import { mkdir, writeFile } from 'node:fs/promises';
import { envSrc } from './env-source.mjs';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_PATH||'playwright');
const out=new URL('../../art/room/verification/',import.meta.url);
await mkdir(out,{recursive:true});
const path=name=>new URL(name,out).pathname.replace(/^\/(\w:)/,'$1');
const browser=await chromium.launch({channel:'chrome',headless:true});
const results=[];
try {
 for(const mode of ['procedural','blender']) {
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  const errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('console',msg=>{if(msg.type()==='error') errors.push(msg.text());});
  if(mode==='procedural') await page.route('**/vivian-furniture.glb',r=>r.abort());
  await page.goto('http://127.0.0.1:1420/plugins/3d-apartment/preview.html');
  await page.waitForFunction(()=>window.__ROOM__?.setEnvironmentSource);
  if(mode==='blender') await page.waitForFunction(()=>{
   let n=0;window.__ROOM__.scene.traverse(o=>{if(o.userData.blenderFurniture)n++;});return n===34;
  },null,{timeout:60000});
  for(const [name,pos,target] of [
   ['overview',[9,20,13],[0,4.2,0]],
   ['living',[1.25,5.65,3.8],[-0.9,4.05,0.6]],
   ['bedroom',[-2.95,5.65,3.8],[-4.6,4.05,0.7]],
   ['desk',[-3.6,5.15,3.15],[-4.7,4.25,4.42]],
  ]) {
   await page.evaluate(({pos,target,src})=>{
    const {camera,controls,setEnvironmentSource}=window.__ROOM__;
    controls.minDistance=0.2;controls.enableDamping=false;
    camera.position.fromArray(pos);controls.target.fromArray(target);controls.update();
    setEnvironmentSource(src);
   },{pos,target,src:envSrc('dusk','clear')});
   await page.waitForTimeout(1800);
   await page.screenshot({path:path(`${mode}-${name}.png`)});
  }
  const stats=await page.evaluate(()=>{
   const {scene,renderer}=window.__ROOM__;
   let upgraded=0,triangles=0,meshes=0,lights=0;const bounds=[];
   scene.traverse(o=>{
    if(o.isLight) lights++;
    if(o.userData.roomFurnitureId){
      const box=new window.__ROOM__.THREE.Box3().setFromObject(o);
      bounds.push({id:o.userData.roomFurnitureId,min:box.min.toArray(),max:box.max.toArray()});
    }
    if(!o.userData.blenderFurniture)return;
    upgraded++;
    o.traverse(m=>{if(m.isMesh){meshes++;triangles+=(m.geometry.index?.count||m.geometry.attributes.position.count)/3;}});

   });
   return {upgraded,triangles,meshes,lights,render:{...renderer.info.render},memory:{...renderer.info.memory},bounds};
  });
  results.push({mode,errors,stats});
  if(mode==='blender' && errors.length) throw new Error(errors.join('\n'));
  // Exercise time/weather switching with the new PBR furniture.
  if(mode==='blender') {
   for(const period of ['morning','noon','night']){
    await page.evaluate((src)=>window.__ROOM__.setEnvironmentSource(src),envSrc(p,'storm'));
    await page.waitForTimeout(250);
   }
   await page.screenshot({path:path('blender-night.png')});
  }
  await page.close();
 }
 if(results[0].stats.lights!==results[1].stats.lights) throw new Error('Furniture lights lost');
 for(const after of results[1].stats.bounds){
   const before=results[0].stats.bounds.find(b=>b.id===after.id);
   if(!before)continue;
   for(const axis of [0,2])for(const edge of ['min','max']){
     if(Math.abs(after[edge][axis]-before[edge][axis])>0.065) throw new Error(`Footprint changed: ${after.id}`);
   }
 }
 await writeFile(new URL('report.json',out),JSON.stringify(results,null,2));
 console.log(JSON.stringify(results.map(({mode,errors,stats})=>({mode,errors,...stats,bounds:undefined})),null,2));
} finally {await browser.close();}
