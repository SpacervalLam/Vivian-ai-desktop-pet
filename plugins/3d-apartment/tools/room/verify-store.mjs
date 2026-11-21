import {createRequire} from 'node:module';
import {mkdir,writeFile} from 'node:fs/promises';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_PATH||'playwright');
import {envSrc} from './env-source.mjs';
const out=new URL('../../art/room/store-verification/',import.meta.url);
await mkdir(out,{recursive:true});
const label=process.argv[2]||'after';
const browser=await chromium.launch({channel:'chrome',headless:true});
try {
 const page=await browser.newPage({viewport:{width:1440,height:1000}});
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 await page.goto('http://127.0.0.1:1420/plugins/3d-apartment/preview.html');
 await page.waitForFunction(()=>window.__ROOM__?.setEnvironmentSource);
 await page.waitForTimeout(3500);
 const views=[['front',[1.1,3.3,7.5],[1.1,1.7,14.5]],['corner',[12,8,6],[0.7,1.8,15]],['interior',[-0.5,2.2,12.8],[-1.0,1.35,15.6]]];
 for(const [name,pos,target] of views){
  await page.evaluate(({pos,target,src})=>{const r=window.__ROOM__;r.controls.minDistance=.2;r.controls.enableDamping=false;r.camera.position.fromArray(pos);r.controls.target.fromArray(target);r.controls.update();r.setEnvironmentSource(src);},{pos,target,src:envSrc('night','drizzle')});
  await page.waitForTimeout(1500);
  await page.screenshot({path:new URL(`${label}-${name}.png`,out).pathname.replace(/^\/(\w:)/,'$1')});
 }
 const stats=await page.evaluate(()=>{
  const {scene,renderer}=window.__ROOM__;const store=scene.getObjectByName('convenience-store');
  let triangles=0,meshes=0;store.traverse(o=>{if(o.isMesh){meshes++;triangles+=(o.geometry.index?.count||o.geometry.attributes.position.count)/3;}});
  return {triangles,meshes,memory:renderer.info.memory,render:renderer.info.render};
 });
 if(label!=='before'){
  const state=await page.evaluate((src)=>{const r=window.__ROOM__;r.setEnvironmentSource(src);return r.scene.getObjectByName('store-wet-reflection')?.visible;},envSrc('noon','clear'));
  await page.waitForTimeout(200);
  const clearVisible=await page.evaluate(()=>window.__ROOM__.scene.getObjectByName('store-wet-reflection')?.visible);
  if(clearVisible!==false) errors.push('Wet reflection remained active in clear weather');
 }
 await writeFile(new URL(`${label}.json`,out),JSON.stringify({errors,stats},null,2));
 console.log(JSON.stringify({errors,stats},null,2));
 if(errors.length)throw Error('Browser verification failed');
}finally{await browser.close();}
