import sharp from 'sharp';
import { readFile, writeFile, copyFile, mkdir, access } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { bounds, faceBounds, headCenter, cleanMatte } from './chibi/frame-sheets/registration.mjs';

// Assemble imagegen's temporal drawings without synthesizing or morphing poses.
const args = process.argv.slice(2);
const right = args.includes('--right');
const direction = right ? 'right' : 'left';
const walkCols = Number(args.find(arg=>arg.startsWith('--walk-cols='))?.split('=')[1] ?? 4);
const walkRows = Number(args.find(arg=>arg.startsWith('--walk-rows='))?.split('=')[1] ?? 4);
if (![walkCols,walkRows].every(n=>Number.isInteger(n)&&n>0) || walkCols*walkRows<14) throw Error('Walk source grid must contain at least 14 cells');
const directory = resolve(right ? 'output/directional-walk-workflow' : 'output/walk-sticker-workflow');
const ids = args.filter(arg => !arg.startsWith('--'));
if (!ids.length) ids.push(...(right ? ['vivian','nana','deepseek','claude','chatgpt'] : ['deepseek','claude','chatgpt']));
const coreAnchors = JSON.parse(await readFile('src/chibi/frameAnchors.json','utf8'));
const coreScales = JSON.parse(await readFile('src/chibi/frameScales.json','utf8'));
const median = values => [...values].sort((a,b)=>a-b)[Math.floor(values.length/2)];
const rounded = n => +n.toFixed(6);
const measure = cell => {
  const box = bounds(cell,512,512), face = faceBounds(cell,512,512,box);
  const portrait = bounds(cell,512,face.y0);
  return {...box, head:headCenter(cell,512,512,face,box), portraitHead:(portrait.x0+portrait.x1)/2};
};
async function cells(file, cols, rows, count) {
  const metadata = await sharp(file).metadata();
  return Promise.all(Array.from({length:count},async(_,i)=>{
    const left = Math.round(i%cols*metadata.width/cols), top = Math.round(Math.floor(i/cols)*metadata.height/rows);
    const right = Math.round((i%cols+1)*metadata.width/cols), bottom = Math.round((Math.floor(i/cols)+1)*metadata.height/rows);
    const cell = await sharp(file).extract({left,top,width:right-left,height:bottom-top})
      .resize(480,480,{fit:'contain',background:'#00000000'}).extend({left:16,right:16,top:16,bottom:16,background:'#00000000'})
      .ensureAlpha().raw().toBuffer();
    cleanMatte(cell,512,512);
    let opaque=0;for(let k=3;k<cell.length;k+=4)if(cell[k]>200)opaque++;
    if(opaque>512*512*.8)throw Error(`${file}/${i}: opaque backdrop must be removed by imagegen before assembly`);
    for(let k=0;k<cell.length;k+=4)if(!cell[k+3])cell.fill(0,k,k+4);
    return cell;
  }));
}
async function backup(source,target) {
  try { await access(source); } catch { return; }
  try { await access(target); } catch { await mkdir(resolve(target,'..'),{recursive:true});await copyFile(source,target); }
}
async function sheet(file,frames) {
  // Direct RGBA packing keeps the shared idle/turn/walk endpoints exact,
  // including low-alpha edge colours that compositing would round differently.
  const height=Math.ceil(frames.length/4)*512;
  const pixels=Buffer.alloc(2048*height*4);
  for(let i=0;i<frames.length;i++)for(let y=0;y<512;y++)frames[i].copy(pixels,((Math.floor(i/4)*512+y)*2048+i%4*512)*4,y*512*4,(y+1)*512*4);
  await sharp(pixels,{raw:{width:2048,height,channels:4}}).webp({lossless:true}).toFile(file+'.new');
  const { rename } = await import('node:fs/promises');await rename(file+'.new',file);
}
for(const id of ids) {
  const builtin = ['vivian','nana'].includes(id);
  const root = resolve(builtin ? 'public' : `character-packs/${id}/assets`);
  const manifestPath = resolve('character-packs',id,'character.json');
  const manifest = builtin ? {anchors:coreAnchors,scales:coreScales,files:{}} : JSON.parse(await readFile(manifestPath,'utf8'));
  const walkPath = `/chibi/walk/${id}/${id}-walk-${direction}-sheet.webp`, turnPath = `/chibi/motion/${id}/${id}-turn-${direction}-sheet.webp`;
  if (!builtin) await backup(manifestPath,resolve(directory,'backup',id,'character.json'));
  else for(const name of ['frameAnchors','frameScales']) await backup(`src/chibi/${name}.json`,resolve(directory,'backup',name+'.json'));
  for(const url of [walkPath,turnPath])await backup(root+url,resolve(directory,'backup',id,'assets'+url));
  const suffix = right ? '-right' : '';
  const walk = await cells(resolve(directory,id+'-walk'+suffix+'-generated.png'),walkCols,walkRows,14);
  const authoredTurn = await cells(resolve(directory,id+'-turn'+suffix+'-generated.png'),4,2,8);
  const idle = await sharp(root+`/chibi/${id}-atlas.webp`).extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer();
  const rest = measure(idle);
  const all = [...walk,...authoredTurn];const measurements = all.map(measure);
  // One scale for the entire motion preserves head size and the authored gentle bob.
  const scale = Math.min((rest.height-1)/median(measurements.map(m=>m.height)),(rest.y1-3)/Math.max(...measurements.map(m=>m.height)));
  const minHead = Math.max(...measurements.map(m=>3+scale*(m.head-m.x0)));
  const maxHead = Math.min(...measurements.map(m=>508-scale*(m.x1-m.head)));
  if(minHead>maxHead)throw Error(`${id}: motion exceeds the common cell`);
  const head = Math.max(minHead,Math.min(maxHead,rest.head));
  const anchor = cell => {const m=measure(cell);return [rounded((head-((m.head-256)*scale+256))/5.12),rounded((rest.y1-((m.y1-460.8)*scale+460.8))/5.12)];};
  manifest.anchors[walkPath]=walk.map(anchor);manifest.scales[walkPath]=walk.map(()=>[rounded(scale),90]);
  if (id === 'nana' && right) {
    // Keep tail swing out of the portrait measurement. The grounded gait has
    // a supporting shoe in every pose, so that shoe defines the floor.
    const gait = measurements.slice(0,walk.length);
    const portraitX = head + scale*median(gait.map(m=>m.portraitHead-m.head));
    manifest.anchors[walkPath]=gait.map(m=>[
      rounded((portraitX-((m.portraitHead-256)*scale+256))/5.12),
      rounded((rest.y1-((m.y1-460.8)*scale+460.8))/5.12),
    ]);
  }
  // Store the eight authored poses directly, without duplicated padding frames.
  const turn = authoredTurn.map(cell=>Buffer.from(cell));
  manifest.anchors[turnPath]=turn.map(anchor);manifest.scales[turnPath]=turn.map(()=>[rounded(scale),90]);
  turn[0]=Buffer.from(idle);manifest.anchors[turnPath][0]=[0,0];manifest.scales[turnPath][0]=[1,90];
  turn[7]=Buffer.from(walk[0]);manifest.anchors[turnPath][7]=[...manifest.anchors[walkPath][0]];manifest.scales[turnPath][7]=[...manifest.scales[walkPath][0]];
  for(const [url,frames] of [[walkPath,walk],[turnPath,turn]]){
    const file=root+url;await sheet(file,frames);
    manifest.files['assets'+url]=createHash('sha256').update(await readFile(file)).digest('hex');
  }
  if (!builtin) {
    if(right)manifest.version='1.0.2';
    if(right)manifest.independent_directions=true;
    await writeFile(manifestPath,JSON.stringify(manifest,null,2)+'\n');
  }
  console.log(`${id}/${direction}: authored drawings, shared endpoints, fixed scale ${scale.toFixed(4)}`);
}
if(ids.some(id=>['vivian','nana'].includes(id))){
  await writeFile('src/chibi/frameAnchors.json',JSON.stringify(coreAnchors,null,2)+'\n');
  await writeFile('src/chibi/frameScales.json',JSON.stringify(coreScales,null,2)+'\n');
}
