import { assertSpriteSeam, characterAssetPath } from './helpers/character-assets.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import sharp from 'sharp';
import {bounds,faceBounds} from '../scripts/chibi/frame-sheets/registration.mjs';

const json=async path=>JSON.parse(await readFile(path,'utf8'));
const vocab=await json('src/chibi/animations.json'),entries=await json('src/chibi/entryStates.json');
const anchors=await json('src/chibi/frameAnchors.json'),scales=await json('src/chibi/frameScales.json');
const packages=await Promise.all(['deepseek','claude','chatgpt'].map(id=>json(`character-packs/${id}/character.json`)));
for(const p of packages){Object.assign(anchors,p.anchors);Object.assign(scales,p.scales);vocab.characters.push(p.id);}
async function frame(s,i){
 const pixels=await sharp(characterAssetPath('public'+s.url)).extract({left:i%s.cols*512,top:Math.floor(i/s.cols)*512,width:512,height:512}).ensureAlpha().raw().toBuffer();
 return {pixels,anchor:anchors[s.url]?.[i]??[0,0],scale:scales[s.url]?.[i]??[1,90]};
}
function same(a,b,label){
 assertSpriteSeam(a.pixels,b.pixels,label);
 assert.deepEqual(a.anchor,b.anchor,`${label}: same displayed position`);
 assert.equal(a.scale[0],b.scale[0],`${label}: same displayed size`);
 if(a.scale[0]!==1)assert.equal(a.scale[1],b.scale[1],`${label}: same scale origin`);
}
function landmarks(f){
 const box=bounds(f.pixels,512,512),face=faceBounds(f.pixels,512,512,box);
 const x=value=>(value-256)*f.scale[0]+256+f.anchor[0]*5.12;
 const y=value=>(value-f.scale[1]*5.12)*f.scale[0]+f.scale[1]*5.12+f.anchor[1]*5.12;
 return {x:x((face.x0+face.x1)/2),foot:y(box.y1)};
}
let checked=0;
const selectedCharacter=process.argv.find(arg=>arg.startsWith('--character='))?.slice('--character='.length);
for(const id of vocab.characters.filter(id=>!selectedCharacter||id===selectedCharacter)){
 const pack=packages.find(p=>p.id===id);
 const available=pack?Object.values(pack.animations):vocab.animations.map(s=>({...s,...s.characterOverrides?.[id]})).filter(s=>!s.characters||s.characters.includes(id));
 const spec=(s,d='left')=>({...s,url:s.sheet.replaceAll('{character}',id).replaceAll('{direction}',d)});
 const idle=await frame(id==='nana'?{url:'/chibi/nana-atlas.webp',cols:3}:{url:`/chibi/motion/${id}/${id}-idle-sheet.webp`,cols:1},0);
 same(idle,await frame({url:`/chibi/${id}-atlas.webp`,cols:3},0),`${id}: fallback idle`);
 assert.deepEqual(available.map(s=>s.name).filter(name=>!entries.idle.includes(name)&&!entries.chains.some(c=>c.to===name)&&!entries.props.includes(name)),[],`${id}: every animation has an audited entry state`);
 for(const s of available.filter(s=>entries.idle.includes(s.name)))for(const d of s.directions??['left']){
  same(idle,await frame(spec(s,d),0),`${id}/${s.name}/${d}: from default idle`);checked++;
 }
 for(const chain of entries.chains){
  const from=available.find(s=>s.name===chain.from),to=available.find(s=>s.name===chain.to);
  if(!from||!to)continue;
  for(const d of to.directions??['left']){
   same(await frame(spec(from,d),from.frames-1),await frame(spec(to,d),0),`${id}: ${from.name} → ${to.name}/${d}`);checked++;
   if(chain.reverseSourceFor?.includes(id))for(let i=0;i<to.frames;i++)same(await frame(spec(from,d),Math.round((1-i/(to.frames-1))*(from.frames-1))),await frame(spec(to,d),i),`${id}/${to.name}/${i}: consistent body and bedding`);
   if(chain.returnToIdle)same(idle,await frame(spec(to,d),to.frames-1),`${id}/${to.name}: return to default idle`);
  }
 }
 const rest=landmarks(idle);
 // The umbrella's proportional fit and entry baseline are covered across the full loop in chibi-umbrella-registration.
 for(const s of available.filter(s=>entries.props.includes(s.name)&&s.name!=='umbrella')){
  const entry=landmarks(await frame(spec(s),0));
  // The clipboard can cover part of the chin and cheeks. The unoccluded floor
  // and head center remain reliable registration checks for the opening frame.
  assert(Math.abs(entry.foot-rest.foot)<=1,`${id}/${s.name}: stable foot position`);
  assert(Math.abs(entry.x-rest.x)<=4,`${id}/${s.name}: stable head center`);
  checked++;
 }
}
console.log(`Original and packaged characters: ${checked} animation entries, contextual chains, prop body anchors and default fallbacks verified.`);
