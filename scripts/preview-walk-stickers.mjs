import sharp from 'sharp';
import { chromium } from 'playwright';
import { build } from 'esbuild';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

const output=resolve('output/directional-walk-workflow');await mkdir(output,{recursive:true});
const ids=process.argv.slice(2).length?process.argv.slice(2):['vivian','nana','deepseek','claude','chatgpt'];
const width=ids.length*240;
const outputName=ids.length===5?'all-five-turn-walk':ids.every(id=>['vivian','nana'].includes(id))?'originals-turn-walk':'imported-turn-walk';
const vocab=JSON.parse(await readFile('src/chibi/animations.json','utf8'));
const definitions=Object.fromEntries(vocab.animations.filter(s=>['walk','turn'].includes(s.name)).map(s=>[s.name,s]));
const code=await build({entryPoints:['src/chibi/movementPlayback.ts'],bundle:true,write:false,format:'esm',platform:'node'});
const {quickTurnPlayback}=await import('data:text/javascript;base64,'+Buffer.from(code.outputFiles[0].text).toString('base64'));
const turn=quickTurnPlayback(definitions.turn.frames);
const coreAnchors=JSON.parse(await readFile('src/chibi/frameAnchors.json','utf8'));
const coreScales=JSON.parse(await readFile('src/chibi/frameScales.json','utf8'));
const coreSize=JSON.parse(await readFile('src/chibi/defaultSize.json','utf8'));
const characters=await Promise.all(ids.map(async id=>{
  const builtin=['vivian','nana'].includes(id),root=resolve(builtin?'public':`character-packs/${id}/assets`);
  const manifest=builtin?{name:id==='vivian'?'Vivian':'Nana',anchors:coreAnchors,scales:coreScales,presentation:{defaultSize:coreSize.characters[id]}}:JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8'));
  const images={};
  for(const [name,url]of [['idle',`/chibi/${id}-atlas.webp`],...Object.entries(definitions).flatMap(([name,s])=>['left','right'].map(direction=>[name+':'+direction,s.sheet.replaceAll('{character}',id).replaceAll('{direction}',direction)]))]){
    images[name]={source:'data:image/webp;base64,'+(await readFile(root+url)).toString('base64'),url};
  }
  return {id,manifest,images};
}));
const timeline=[];
for(const direction of ['left','right']){
  timeline.push({name:'idle',frame:0,duration:350,direction});
  for(let i=0;i<turn.frames.length;i++)timeline.push({name:'turn',frame:turn.frames[i],duration:turn.durations[i],direction});
  for(let cycle=0;cycle<2;cycle++)for(let i=0;i<definitions.walk.frames;i++)timeline.push({name:'walk',frame:i,duration:Math.round((i+1)*78/10)*10-Math.round(i*78/10)*10,direction});
  for(let i=turn.frames.length-1;i>=0;i--)timeline.push({name:'turn',frame:turn.frames[i],duration:turn.durations[i],direction});
  timeline.push({name:'idle',frame:0,duration:500,direction});
}
const browser=await chromium.launch({channel:'chrome',headless:true});
try{
 const page=await browser.newPage();
 await page.setContent(`<canvas width="${width}" height="300"></canvas>`);
 await page.evaluate(async({characters,definitions})=>{
  window.model={characters,definitions};
  for(const character of characters)for(const image of Object.values(character.images)){
   const element=new Image();element.src=image.source;await element.decode();image.element=element;
  }
  window.drawFrame=step=>{
   const canvas=document.querySelector('canvas'),ctx=canvas.getContext('2d');
   ctx.fillStyle='#efedf4';ctx.fillRect(0,0,canvas.width,canvas.height);
   characters.forEach(({manifest,images},n)=>{
    const {url,element}=images[step.name==='idle'?'idle':step.name+':'+step.direction],s=definitions[step.name]??{cols:3,rows:2};
    const [scale,origin]=manifest.scales[url]?.[step.frame]??[1,90], [tx,ty]=manifest.anchors[url]?.[step.frame]??[0,0];
    const defaults=manifest.presentation.defaultSize;
    ctx.save();ctx.translate(n*240,32);ctx.scale(240/512,240/512);
    ctx.translate(256,460.8);ctx.translate(0,defaults.translateY*5.12);ctx.scale(defaults.scale,defaults.scale);ctx.translate(-256,-460.8);
    ctx.translate(tx*5.12,ty*5.12);ctx.translate(256,origin*5.12);ctx.scale(scale,scale);ctx.translate(-256,-origin*5.12);
    const w=element.naturalWidth/s.cols,h=element.naturalHeight/s.rows;
    ctx.drawImage(element,step.frame%s.cols*w,Math.floor(step.frame/s.cols)*h,w,h,0,0,512,512);ctx.restore();
    ctx.font='16px Segoe UI';ctx.textAlign='center';ctx.fillStyle='#524862';ctx.fillText(manifest.name,n*240+120,22);
   });
   ctx.font='13px Segoe UI';ctx.fillStyle='#70667d';ctx.fillText(`${step.direction} · ${step.name}`,canvas.width/2,288);
   return canvas.toDataURL('image/png').split(',')[1];
  };
 },{characters,definitions});
 const frames=[];
 for(const step of timeline){const png=await page.evaluate(step=>window.drawFrame(step),step);frames.push(await sharp(Buffer.from(png,'base64')).ensureAlpha().raw().toBuffer());}
 await sharp(Buffer.concat(frames),{raw:{width,height:300*frames.length,channels:4,pageHeight:300}})
  .gif({loop:0,delay:timeline.map(s=>s.duration),colours:256,dither:0.2,effort:4}).toFile(output+'/'+outputName+'.gif');
 await writeFile(output+'/preview-timing.json',JSON.stringify(timeline,null,2)+'\n');
 console.log(`Saved ${timeline.length} chronological frames: ${output}/${outputName}.gif`);
}finally{await browser.close();}
