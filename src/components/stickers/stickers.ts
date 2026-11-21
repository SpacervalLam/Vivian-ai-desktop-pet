import { isCharacterId } from '../../characters';
import {invoke} from '@tauri-apps/api/core';
import type {StickerRef} from '../../types';
import i18n from '../../i18n';
export function stickerCategory(sticker:StickerRef):string {
 return sticker.category || /^(?:vivian|nana|deepseek|claude)(?:_ds)?_([a-z]+)_01$/.exec(sticker.id)?.[1] || 'custom';
}
export function stickerTypeLabel(category:string,fallback=category):string {
 return String(i18n.t(`stickers.${category}`,{defaultValue:fallback}));
}
export function stickerLabel(sticker:StickerRef):string {
 // Imported replacements retain their user-defined labels; builtins use reaction names.
 if(!['1','2'].includes(sticker.version))return sticker.label;
 return stickerTypeLabel(stickerCategory(sticker),sticker.label.split(' · ').pop());
}
export function parseSticker(value:unknown):StickerRef|undefined {
 if(!value||typeof value!=='object')return;
 const s=value as Record<string,unknown>;
 if(['id','character_id','version','label','meaning'].some(k=>typeof s[k]!=='string'))return;
 if(s.category!==undefined&&(typeof s.category!=='string'||!/^[A-Za-z0-9_-]{1,100}$/.test(s.category)))return;
 if(!isCharacterId(s.character_id)||!String(s.id).startsWith(`${s.character_id}_`)||![s.id,s.version].every(v=>/^[A-Za-z0-9_-]{1,100}$/.test(String(v))))return;
 return s as unknown as StickerRef;
}
const cache=new Map<string,Promise<string>>();
export function stickerSource(sticker:StickerRef):Promise<string>{
 const key=JSON.stringify([sticker.character_id,sticker.id,sticker.version]);
 let pending=cache.get(key);
 if(!pending){pending=invoke<string>('get_sticker_data_url',{sticker}).catch(e=>{cache.delete(key);throw e;});cache.set(key,pending);if(cache.size>60)cache.delete(cache.keys().next().value!);}
 return pending;
}
