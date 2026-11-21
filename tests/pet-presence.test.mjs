import assert from 'node:assert/strict';
import { readFile, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import sharp from 'sharp';
const source = await readFile('src-tauri/src/presence/mod.rs','utf8');
const start = source.indexOf('pub enum PresenceState');
const end = source.indexOf('impl Default for PresenceState',start);
const dir = await mkdtemp(path.join(tmpdir(),'pet-presence-'));
try {
 await writeFile(path.join(dir,'main.rs'), '#[derive(Debug,Clone,Copy,PartialEq)]\n'+source.slice(start,end).replace(/#\[serde\([^\n]+\)\]/g,'')+`fn main(){
  let edge=PresenceState::from_str("edge");assert_eq!(edge,PresenceState::Busy);assert_eq!(edge.as_str(),"edge");
  assert!(edge.is_in_presence());assert!(edge.can_direct());
  assert!(!PresenceState::Rest.is_in_presence());assert!(!PresenceState::Rest.can_direct());
  assert_eq!(PresenceState::Rest.message_channel("direct"),"wechat");
  assert_eq!(PresenceState::from_str("busy"),edge);
 }`);
 const exe=path.join(dir,'presence.exe');
 const compiled=spawnSync('rustc',['--edition=2021',path.join(dir,'main.rs'),'-o',exe],{encoding:'utf8'});
 assert.equal(compiled.status,0,compiled.stderr);
 assert.equal(spawnSync(exe).status,0);
 for(const id of ['vivian','nana','deepseek','claude'])for(const side of ['peek','bottom']){
  const file=`public/chibi/motion/${id}/${id}-edge-${side}-sheet.webp`,meta=await sharp(file).metadata();
  assert.deepEqual([meta.width,meta.height,meta.hasAlpha],[2048,side==='peek'?1024:512,true]);
  const first=await sharp(file).extract({left:0,top:0,width:512,height:512}).raw().toBuffer();
  const idle=await sharp(id==='nana'?'public/chibi/nana-atlas.webp':`public/chibi/motion/${id}/${id}-idle-sheet.webp`).extract({left:0,top:0,width:512,height:512}).raw().toBuffer();
  for(let i=0;i<first.length;i+=4){assert.equal(first[i+3],idle[i+3],`${id}: entry silhouette`);if(idle[i+3]>200)for(let c=0;c<3;c++)assert(Math.abs(first[i+c]-idle[i+c])<=2,`${id}: entry color`);}
 }
 console.log('Presence: resting cannot hear; edge can hear; legacy busy migration; all eight horizontal/bottom native edge sheets and continuous entry passed.');
}finally{await rm(dir,{recursive:true,force:true});}
