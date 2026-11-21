import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {resolve} from 'node:path';
import {chromium} from 'playwright';
const images=[];const files=new Map();
for(const id of ['deepseek','claude','chatgpt']){
 const root=resolve(process.argv.includes('--release')?'tmp/character-release':'character-packs',id),m=JSON.parse(await readFile(root+'/character.json','utf8'));
 for(const [name,hash] of Object.entries(m.files)){
  const bytes=await readFile(root+'/'+name);
  assert.equal(createHash('sha256').update(bytes).digest('hex'),hash,`${id}/${name}: declared hash`);
  if(name.startsWith('assets/'))assert(new RegExp(`^${id}[-_.]`).test(name.split('/').at(-1)),`${id}: assets owned exclusively`);
  if(/\.(webp|png|jpg|svg)$/.test(name)){const url=`http://character-art.test/${id}/${name}`;files.set(url,bytes);images.push(url);}
 }
}
const browser=await chromium.launch({channel:'chrome',headless:true});
try{
 const page=await browser.newPage();
 await page.route('http://character-art.test/**',async route=>{
  const url=route.request().url();
  if(files.has(url))return route.fulfill({contentType:url.endsWith('.svg')?'image/svg+xml':url.endsWith('.png')?'image/png':'image/webp',body:files.get(url)});
  return route.fulfill({contentType:'text/html',body:'<!doctype html><title>Asset decode test</title>'});
 });
 await page.goto('http://character-art.test/');
 const bad=await page.evaluate(async urls=>{
  const failures=[];
  // Decode sequentially to bound image memory use; no model or application data.
  for(const url of urls){const img=new Image();img.src=url;try{await img.decode();if(!img.naturalWidth||!img.naturalHeight)throw Error('empty image');}catch(e){failures.push(url+': '+String(e));}}
  return failures;
 },images);
 assert.deepEqual(bad,[]);console.log(`${images.length} packaged images: real Chromium decoding, ownership and all file hashes verified.`);
}finally{await browser.close();}
