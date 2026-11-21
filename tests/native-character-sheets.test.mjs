import { characterAssetPath } from './helpers/character-assets.mjs';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {stat,readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import sharp from 'sharp';
const vocab=JSON.parse(await readFile('src/chibi/animations.json','utf8'));
const packages=await Promise.all(['deepseek','claude','chatgpt'].map(async id=>({manifest:JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8')),root:resolve('character-packs',id)})));
const bundled=await build({stdin:{contents:"import {initializeCharacterPackages} from './src/characterPackages';await initializeCharacterPackages();export * from './src/chibi/motionRegistry';",resolveDir:process.cwd()},plugins:[{name:'installed',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'native',namespace:'native'}));b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:'export const invoke=async()=>('+JSON.stringify({active:packages,installed:packages.map(p=>p.manifest)})+');export const convertFileSrc=p=>p;'}));}}],bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent'});
const registry=await import('data:text/javascript;base64,'+Buffer.from(bundled.outputFiles[0].text).toString('base64'));
const selectedCharacter=process.argv.find(arg=>arg.startsWith('--character='))?.slice('--character='.length);
for(const character of ['deepseek','claude','chatgpt'].filter(id=>!selectedCharacter||id===selectedCharacter)){
 for(const {name} of vocab.animations.filter(s=>packages.find(p=>p.manifest.id===character).manifest.files['assets'+s.sheet.replaceAll('{character}',character).replaceAll('{direction}','left')])){
  const spec=registry.resolveMotion(name,character);
  assert.equal(spec.kind,'animation',`${character}/${name} requires a real animation sheet`);
  assert(spec.frames>=1);
  for(const direction of spec.directions??['left']){
   const expected=spec.sheet.replaceAll('{character}',character).replaceAll('{direction}',direction);
   assert.equal(registry.sheetUrl(spec,character,direction),resolve('character-packs',character)+'/assets'+expected,`${character}/${name}: no condensed-pose fallback`);
   assert((await stat(characterAssetPath('public'+expected))).size>0);
   const first=registry.frameStyle(spec,0,character,direction),next=registry.frameStyle(spec,1,character,direction);
   assert.equal(first.backgroundSize,`${spec.cols*100}% ${spec.rows*100}%`);
   if(spec.frames>1)assert.notEqual(first.backgroundPosition,next.backgroundPosition,`${character}/${name}: consecutive frame addressing`);
   const file=characterAssetPath('public'+expected);
   const meta=await sharp(file).metadata();
   assert.deepEqual([meta.width,meta.height,meta.hasAlpha],[spec.cols*512,spec.rows*512,true],`${character}/${name}: native runtime grid`);
   const poses=new Set();let openingHash,closingHash;
   for(let i=0;i<spec.frames;i++){
    const pixels=await sharp(file).extract({left:i%spec.cols*512,top:Math.floor(i/spec.cols)*512,width:512,height:512}).raw().toBuffer();
    let visible=0;
    for(let y=0;y<512;y++)for(let x=0;x<512;x++){
     const alpha=pixels[(y*512+x)*4+3];
     if(alpha>128)visible++;
     if(x<1||x>=511||y<1||y>=511)assert.equal(alpha,0,`${character}/${name}/${i}: transparent safety margin`);
    }
    assert(visible>3000,`${character}/${name}/${i}: populated authored pose`);
    assert(visible<512*512*.8,`${character}/${name}/${i}: transparent backdrop`);
    if(name==='sleep'){
     const occupiedRows=[];
     for(let y=0;y<512;y++){
      let occupied=false;
      for(let x=0;x<512;x++)if(pixels[(y*512+x)*4+3]>200){occupied=true;break;}
      if(occupied)occupiedRows.push(y);
     }
     assert.equal(occupiedRows.at(-1)-occupiedRows[0]+1,occupiedRows.length,`${character}/${name}/${i}: intact silhouette without horizontal crop gaps`);
    }
    const hash=createHash('sha256').update(pixels).digest('hex');
    if(i===0)openingHash=hash;if(i===spec.frames-1)closingHash=hash;
    poses.add(hash);
   }
   // Rigid in-betweens may hold a drawing when the authored body is stationary.
   // They must still retain the original distinct key poses, including blink closure.
   const keyframes=vocab.animations.find(s=>s.name===name)?.keyframes;
   const closesOnOpening=spec.frames>1&&openingHash===closingHash;
   const uniqueFrames=Math.min(keyframes?.length??spec.frames,spec.frames)-(closesOnOpening?1:0);
   if(keyframes)assert.ok(poses.size>=uniqueFrames,`${character}/${name}: original key-pose variety retained`);
   else assert.equal(poses.size,uniqueFrames,`${character}/${name}: distinct poses apart from an intentional shared entry/exit`);
   for(let i=spec.frames;i<spec.cols*spec.rows;i++){
    const alpha=await sharp(file).extract({left:i%spec.cols*512,top:Math.floor(i/spec.cols)*512,width:512,height:512}).extractChannel('alpha').raw().toBuffer();
    assert(alpha.every(value=>value===0),`${character}/${name}/${i}: unused cells stay empty`);
   }
  }
 }
}
console.log('New characters: native per-action animation sheets and consecutive frame addressing verified.');
