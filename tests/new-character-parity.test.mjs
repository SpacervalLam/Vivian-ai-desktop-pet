import { characterAssetPath } from './helpers/character-assets.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
const json=async p=>JSON.parse(await readFile(characterAssetPath(p),'utf8'));
const stickers=await json('public/stickers/catalog.json');
const packs=await json('src/optional-resources.json');

// 12 个种子源必须同构：语言只影响 content/description，不影响 id、时间、metadata。
// 运行时 seed.rs 只认 JSON；旧版 --- markdown 会让角色直接启动失败。
for(const id of ['vivian','nana','deepseek','claude']){
 const base='src-tauri/prompts/characters/'+id;
 const zh=await json(base+'/seed_memories.md');
 assert(zh.history.length>=48 && zh.memories.length>=16,`${id}: seed size`);
 for(const lang of ['en','ja']){
  const loc=await json(`${base}/seed_memories.${lang}.md`);
  assert.equal(loc.character_id,id,`${id}/${lang}: character_id`);
  assert.equal(loc.seed_revision,zh.seed_revision,`${id}/${lang}: revision`);
  assert.equal(loc.timeline_anchor,zh.timeline_anchor,`${id}/${lang}: timeline anchor`);
  assert.deepEqual(loc.history.map(h=>h.id),zh.history.map(h=>h.id),`${id}/${lang}: history ids`);
  assert.deepEqual(loc.memories.map(m=>m.id),zh.memories.map(m=>m.id),`${id}/${lang}: memory ids`);
  assert.deepEqual(loc.history.map(h=>h.timestamp),zh.history.map(h=>h.timestamp),`${id}/${lang}: history times`);
  assert.deepEqual(loc.history.map(h=>h.metadata),zh.history.map(h=>h.metadata),`${id}/${lang}: history metadata`);
  assert.deepEqual(loc.memories.map(m=>m.created_at),zh.memories.map(m=>m.created_at),`${id}/${lang}: memory times`);
  assert.deepEqual(loc.memories.map(m=>m.metadata),zh.memories.map(m=>m.metadata),`${id}/${lang}: memory metadata`);
  const translated=loc.history.some((h,i)=>h.content!==zh.history[i].content);
  if(loc.translation_status?.startsWith('placeholder'))assert(!translated,`${id}/${lang}: placeholder 必须回退中文`);
  else assert(translated,`${id}/${lang}: 应有真实译文`);
 }
}

for(const [id,name]of [['deepseek','DeepSeek'],['claude','Claude']]){
 const base='src-tauri/prompts/characters/'+id;
 for(const lang of ['en','ja'])for(const section of ['identity','personality','background','interests','appearance','speech','canon_quotes','relationships','examples','persona_config','environment_presets','scenes','seed_memories'])assert((await readFile(characterAssetPath(`${base}/${section}.${lang}.md`),'utf8')).trim().length>30,`${id}: localized ${section}`);
 const voices=await json(base+'/voice_fragments.json');assert(voices.length>=40);assert(new Set(voices.map(v=>v.scope)).size>=10);
 for(const v of voices)for(const lang of ['zh','en','ja'])assert(v.response.text[lang]?.trim());
 const seed=await json(base+'/seed_memories.md');assert(seed.history.length>=48 && seed.memories.length>=16);
 for(const h of seed.history){assert.equal(h.metadata.fictional_backstory,true);assert.equal(h.metadata.memory_owner,id);assert.notEqual(h.metadata.speaker,'user');assert.notEqual(h.metadata.listener,'user');}
 const own=await json(`character-packs/${id}/prompts/stickers.json`);
 // Claude 不随包提供内置贴纸（用户明确要求移除），其余新角色仍与初版保持同量。
 if(id==='claude')assert.equal(own.length,0,'claude must not ship builtin stickers');
 else assert.equal(own.length,10);
 for(const s of own)assert(!packs.find(p=>p.id==='stickers').files.includes('stickers/'+s.filename));
}
console.log('New-character parity: three-language personas, authored histories, dialogue diversity, stickers and packaging verified.');
