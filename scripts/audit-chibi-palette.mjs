// Read-only diagnostic: light lavender/white regions, excluding transparency and skin.
import sharp from 'sharp';
import {readFile,access,mkdir,writeFile} from 'node:fs/promises';
export async function lightPalette(file,cols,rows,frames){
 const {data,info}=await sharp(file).ensureAlpha().raw().toBuffer({resolveWithObject:true});
 const w=info.width/cols,h=info.height/rows;
 if(!Number.isInteger(w)||!Number.isInteger(h))throw new Error(`Non-integral grid: ${file}`);
 const result=[];
 for(let i=0;i<frames;i++){
  const sums=[0,0,0];let count=0;
  for(let y=0;y<h;y++)for(let x=0;x<w;x++){
   const k=((Math.floor(i/cols)*h+y)*info.width+i%cols*w+x)*4;
   const [r,g,b,a]=data.subarray(k,k+4);
   if(a>230&&r>140&&g>125&&b>=r&&b-g<65){count++;sums[0]+=r;sums[1]+=g;sums[2]+=b;}
  }
  result.push({frame:i,count,rgb:sums.map(v=>count?+(v/count).toFixed(2):0)});
 }
 return result;
}
if(process.argv[1]?.endsWith('audit-chibi-palette.mjs')){
 const vocab=JSON.parse(await readFile('src/chibi/animations.json','utf8')),report=[];
 for(const character of ['nana']){
  report.push({character,name:'default',file:`public/chibi/${character}-atlas.webp`,frames:await lightPalette(`public/chibi/${character}-atlas.webp`,3,2,6)});
  for(const spec of vocab.animations){
   if(spec.characters&&!spec.characters.includes(character))continue;
   for(const direction of spec.directions??['left']){
    const file='public'+spec.sheet.replaceAll('{character}',character).replaceAll('{direction}',direction);
    try{await access(file);}catch{continue;}
    const frames=await lightPalette(file,spec.cols,spec.rows,spec.frames);
    const lum=frames.filter(f=>f.count>1000).map(f=>f.rgb[0]*.2126+f.rgb[1]*.7152+f.rgb[2]*.0722);
    report.push({character,name:spec.name,direction,file,range:lum.length?+(Math.max(...lum)-Math.min(...lum)).toFixed(2):0,frames});
   }
  }
 }
 await mkdir('tmp/chibi-palette',{recursive:true});await writeFile('tmp/chibi-palette/audit.json',JSON.stringify(report,null,2)+'\n');
 for(const r of report)console.log(r.name,r.direction??'',r.range??'',r.frames.map(f=>f.rgb.map(Math.round).join('/')).join(' '));
}
