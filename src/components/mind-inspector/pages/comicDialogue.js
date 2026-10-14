import { dialogueBank } from './comicDialogueBank.js';
import { localizedDialogueBanks } from './comicDialogueTranslations.js';
import { comicLocale, comicText } from '../../../i18n/comic.js';
const finite = value => typeof value === 'number' && Number.isFinite(value);
const hash = text => [...String(text)].reduce((n,c)=>(n*31+c.charCodeAt(0))>>>0,0);
const moodKey = (v,a) => v>=.55 ? (a>=.55?'joyful':'tender') : v>=.25 ? (a>=.55?'excited':'content') : v<=-.25 ? (a>=.4?'uneasy':'tired') : a>=.55?'restless':'quiet';
export function createDialogueSelector() {
 const cache=new Map();
 return (role,category,character,visit=0,language='zh-CN')=>{
  const suffix=role==='vivian'?'Vivian':'Nana';
  if(character.status==='loading')return comicText(`loading${suffix}`,language);
  if(character.error)return comicText(`error${suffix}`,language);
  const rows=(localizedDialogueBanks[comicLocale(language)]??dialogueBank)[role][category],cacheKey=role+':'+category,previous=cache.get(cacheKey);
  if(category==='mood'){
   const v=character.valence,a=character.arousal;
   if(!finite(v)||!finite(a))return comicText(`moodMissing${suffix}`,language);
   let key=moodKey(v,a);
   if(previous&&key!==previous.key&&Math.abs(v-previous.v)<.08&&Math.abs(a-previous.a)<.08)key=previous.key;
   const variant=hash(role+':'+key+':'+visit)%4;
   if(!previous||previous.key!==key)cache.set(cacheKey,{key,v,a});
   return rows[key][variant];
  }
  const available=(character[category]??[]).filter(([key,value])=>rows[key]&&finite(value)).map(([key,value])=>[key,Math.max(0,Math.min(100,value))]);
  if(!available.length)return comicText(`missing${suffix}`,language);
  available.sort((a,b)=>b[1]-a[1]);let [key,value]=available[0];
  if(category==='emotions'&&available.some(([k])=>k===character.primaryEmotion))[key,value]=available.find(([k])=>k===character.primaryEmotion);
  if(previous&&key!==previous.key){const prior=available.find(([k])=>k===previous.key);if(prior&&value-prior[1]<8)[key,value]=prior;}
  let tier=Math.min(3,Math.floor(value/25));
  if(previous&&previous.key===key){const low=previous.tier*25-4,high=(previous.tier+1)*25+4;if(value>=low&&value<high)tier=previous.tier;}
  cache.set(cacheKey,{key,tier});return rows[key][tier];
 };
}
export const selectComicDialogue=createDialogueSelector();
