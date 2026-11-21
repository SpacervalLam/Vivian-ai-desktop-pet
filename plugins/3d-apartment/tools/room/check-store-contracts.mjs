import {createRequire} from 'node:module';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_PATH||'playwright');
const browser=await chromium.launch({channel:'chrome',headless:true});
try {
 const page=await browser.newPage();
 await page.goto('http://127.0.0.1:1420/plugins/3d-apartment/tools/room/export-base.html');
 const result=await page.evaluate(async()=>{
  const {buildConvenienceStore}=await import('/plugins/3d-apartment/src/anime/exterior.ts');
  const {buildSceneColliders}=await import('/plugins/3d-apartment/src/anime/collider.ts');
  const store=buildConvenienceStore();
  const details=store.group.getObjectByName('store-authored-details');
  if(buildSceneColliders(details).length)throw Error('Decorations added colliders');
  for(let row=0;row<2;row++)for(let segment=0;segment<3;segment++){
   const shelf=store.group.getObjectByName(`shelf-r${row}s${segment}`);
   const boxes=buildSceneColliders(shelf);
   if(boxes.length!==1)throw Error('Original shelf collider lost');
   const box=boxes[0];
   if(Math.abs(box.max.x-box.min.x-2.18)>.001 || Math.abs(box.max.z-box.min.z-.58)>.001)throw Error('Shelf footprint changed');
  }
  const leaves=[];store.dynamic.traverse(o=>{if(o.name==='store-door-leaf')leaves.push(o);});
  store.update(0,false);const closed=leaves.map(o=>o.position.x);
  if(store.dynamic.getObjectByName('store-wet-reflection').visible)throw Error('Dry reflection visible');
  store.update(2,true);const open=leaves.map(o=>o.position.x);
  if(leaves.length!==2||closed.some((x,i)=>Math.abs(x-open[i])<.73))throw Error('Door motion broken');
  if(!store.dynamic.getObjectByName('store-wet-reflection').visible)throw Error('Rain reflection hidden');
  store.dispose();
  return {shelfColliders:6,decorationColliders:0,closed,open,weatherToggle:true};
 });
 console.log(JSON.stringify(result,null,2));
} finally {await browser.close();}
