import sharp from 'sharp';
import { readFile, writeFile, rename, access } from 'node:fs/promises';
import { resolve } from 'node:path';
import { bounds, faceBounds, headCenter, cleanMatte } from './chibi/frame-sheets/registration.mjs';

const ids = process.argv.slice(2);
if (!ids.length) throw new Error('Supply character IDs');
const json = async p => JSON.parse(await readFile(p, 'utf8'));
const vocab = await json('src/chibi/animations.json');
const entries = await json('src/chibi/entryStates.json');
const anchors = await json('src/chibi/frameAnchors.json');
const scales = await json('src/chibi/frameScales.json');
const size = await json('src/chibi/defaultSize.json');
const dock = await json('src/chibi/bottomDock.json');
async function save(path, bytes) { await writeFile(path + '.character-tmp', bytes); await rename(path + '.character-tmp', path); }
const median = list => [...list].sort((a,b)=>a-b)[Math.floor(list.length/2)];
const round = n => +n.toFixed(6);
const standing = new Set(['blink','cast','happy','angry','think','thinking','smug','busy-in','busy-loop','remind','tired','music','look']);
for (const id of ids) {
  const sheets = new Map();
  const definitionPath=resolve('character-packs',id,'character.json');
  let definition=null;try{definition=await json(definitionPath);}catch{}
  const pathFor = url => definition ? resolve('character-packs',id,'assets'+url) : resolve('public' + url);
  async function sheet(url, cols, rows, frames) {
    if (sheets.has(url)) return sheets.get(url);
    const { data, info } = await sharp(await readFile(pathFor(url))).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    if (info.width !== cols*512 || info.height !== rows*512) throw new Error(`Wrong dimensions: ${url}`);
    cleanMatte(data, info.width, info.height);
    const cells = Array.from({length:cols*rows},(_,i)=>{
      const cell = Buffer.alloc(512*512*4);
      if (i<frames) for(let y=0;y<512;y++) {const at=((Math.floor(i/cols)*512+y)*info.width+i%cols*512)*4;data.copy(cell,y*2048,at,at+2048);}
      return cell;
    });
    // Format a two-pixel gutter when soft antialiasing reaches a cell boundary.
    // Resize the whole drawing together, preserving its proportions and alpha.
    for(let i=0;i<frames;i++) {
      const cell=cells[i];let touches=false;
      for(let n=0;n<512;n++)if(cell[n*4+3]||cell[(511*512+n)*4+3]||cell[(n*512)*4+3]||cell[(n*512+511)*4+3]){touches=true;break;}
      if(touches)cells[i]=await sharp(cell,{raw:{width:512,height:512,channels:4}}).resize(508,508).extend({top:2,bottom:2,left:2,right:2,background:{r:0,g:0,b:0,alpha:0}}).raw().toBuffer();
    }
    const s={url,cols,rows,frames,cells}; sheets.set(url,s);
    anchors[url]=Array.from({length:frames},()=>[0,0]);scales[url]=Array.from({length:frames},()=>[1,90]);
    return s;
  }
  const atlas=await sheet(`/chibi/${id}-atlas.webp`,3,2,6);
  const idle=await sheet(`/chibi/motion/${id}/${id}-idle-sheet.webp`,1,1,1);
  idle.cells[0]=Buffer.from(atlas.cells[0]);
  function measure(cell) {const box=bounds(cell,512,512),face=faceBounds(cell,512,512,box);return {top:box.y0,foot:box.y1,bodyX:(box.x0+box.x1)/2,cx:(face.x0+face.x1)/2,head:headCenter(cell,512,512,face,box),chin:face.y1,w:face.width};}
  const rest=measure(idle.cells[0]);
  const specs=[];const seen=new Set();
  for(const spec of definition ? Object.values(definition.animations) : vocab.animations.map(s=>({...s,...s.characterOverrides?.[id]}))) {
    const url=spec.sheet.replaceAll('{character}',id).replaceAll('{direction}','left');
    if(seen.has(url))continue;
    try {await access(pathFor(url));} catch {continue;}
    seen.add(url);const s=await sheet(url,spec.cols,spec.rows,spec.frames);specs.push({...spec,s});
    if(standing.has(spec.name)||['float','turn','walk','umbrella','clipboard'].includes(spec.name)) {
      const measurements=s.cells.slice(0,s.frames).map(cell=>{try{return measure(cell);}catch{return null;}});
      const valid=measurements.filter(Boolean);if(!valid.length)continue;
      let factor=median(valid.map(m=>rest.w/m.w));
      if(spec.name==='blink')factor=median(valid.map(m=>(rest.foot-rest.top)/(m.foot-m.top)));
      if(entries.props.includes(spec.name)) factor=(rest.foot-rest.chin)/(measurements[0].foot-measurements[0].chin);
      factor=Math.max(.65,Math.min(1.4,factor));
      const dx=rest.cx-((median(valid.map(m=>m.cx))-256)*factor+256);
      for(let i=0;i<s.frames;i++) {const m=measurements[i];if(!m)continue;
        // A side-facing cheek is narrower than a frontal face. Never use that
        // width to magnify a turn or walk; register the complete silhouette.
        const silhouette=['turn','walk'].includes(spec.name)||(['claude','deepseek'].includes(id)&&spec.name==='happy');
        const frameFactor=silhouette?(rest.foot-rest.top)/(m.foot-m.top):factor;
        scales[url][i]=[round(frameFactor),90];
        const foot=standing.has(spec.name)||spec.name==='walk'||entries.props.includes(spec.name)?m.foot:median(valid.map(m=>m.foot));
        anchors[url][i]=[round(dx/5.12),round((rest.foot-((foot-460.8)*factor+460.8))/5.12)];
        if(silhouette)anchors[url][i]=[round((rest.bodyX-((m.bodyX-256)*frameFactor+256))/5.12),round((rest.foot-((m.foot-460.8)*frameFactor+460.8))/5.12)];
        if(spec.name==='blink')anchors[url][i][0]=round((rest.head-((m.head-256)*factor+256))/5.12);
      }
      if(entries.props.includes(spec.name)) for(let i=0;i<s.frames;i++) {const m=measurements[i];if(m)anchors[url][i][0]=round((rest.cx-((m.cx-256)*factor+256))/5.12);}
    }
  }
  function copy(target, ti, source, si) {
    target.cells[ti]=Buffer.from(source.cells[si]);
    anchors[target.url][ti]=[...anchors[source.url][si]];
    scales[target.url][ti]=[...scales[source.url][si]];
  }
  for(const spec of specs)if(entries.idle.includes(spec.name))copy(spec.s,0,idle,0);
  for(const chain of entries.chains) {
    const from=specs.find(s=>s.name===chain.from),to=specs.find(s=>s.name===chain.to);if(!from||!to)continue;
    if(chain.from==='sleep')for(let i=0;i<to.frames;i++)copy(to.s,i,from.s,Math.round((1-i/(to.frames-1))*(from.frames-1)));
    // Keep the authored gait's opening pose in its loop. Adapt the turn's final
    // pose to it, rather than inserting a differently drawn turn into every lap.
    if(chain.from==='turn'&&chain.to==='walk')copy(from.s,from.frames-1,to.s,0);
    else copy(to.s,0,from.s,from.frames-1);
    if(chain.returnToIdle)copy(to.s,to.frames-1,idle,0);
  }
  const blink=specs.find(s=>s.name==='blink');if(blink)copy(blink.s,blink.frames-1,idle,0);
  const happy=specs.find(s=>s.name==='happy');if(happy&&['deepseek','claude'].includes(id))copy(happy.s,happy.frames-1,idle,0);
  for(const s of sheets.values()) {
    const data=Buffer.alloc(s.cols*512*s.rows*512*4),width=s.cols*512;
    for(let i=0;i<s.cells.length;i++)for(let y=0;y<512;y++)s.cells[i].copy(data,((Math.floor(i/s.cols)*512+y)*width+i%s.cols*512)*4,y*2048,(y+1)*2048);
    await save(pathFor(s.url),await sharp(data,{raw:{width,height:s.rows*512,channels:4}}).webp({lossless:true}).toBuffer());
  }
  const factor=size.target.height/(rest.foot-rest.top);
  size.characters[id]={scale:round(factor),translateY:round((size.target.foot-460.8-factor*(rest.foot-460.8))/5.12)};
  const edge=specs.find(s=>s.name==='edge-bottom');
  if(edge) {const cell=edge.s.cells[edge.frames-1],box=bounds(cell,512,512);const k=dock.target.width/(size.characters[id].scale*box.width);const y=460.8+size.characters[id].translateY*5.12+size.characters[id].scale*(box.y1-460.8);dock.characters[id]={scale:round(k),translateY:round((dock.target.bottom-460.8-k*(y-460.8))/5.12)};}
  if(definition) {
    const owned=path=>path.startsWith(`/chibi/${id}-`)||path.includes(`/chibi/motion/${id}/`)||path.includes(`/chibi/walk/${id}/`);
    definition.anchors=Object.fromEntries(Object.entries(anchors).filter(([path])=>owned(path)));
    definition.scales=Object.fromEntries(Object.entries(scales).filter(([path])=>owned(path)));
    definition.presentation={...definition.presentation,defaultSize:size.characters[id],bottomDock:dock.characters[id]};
    await save(definitionPath,JSON.stringify(definition,null,2)+'\n');
    for(const table of [anchors,scales])for(const path of Object.keys(table))if(owned(path))delete table[path];
    delete size.characters[id];delete dock.characters[id];
  }
  console.log(`${id}: finalized ${sheets.size} sheets, rest face=${rest.w}, foot=${rest.foot}`);
}
for(const [file,value] of [['frameAnchors',anchors],['frameScales',scales],['defaultSize',size],['bottomDock',dock]])await save(`src/chibi/${file}.json`,JSON.stringify(value,null,2)+'\n');
