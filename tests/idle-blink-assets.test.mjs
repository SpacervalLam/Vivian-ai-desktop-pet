import assert from 'node:assert/strict';
import sharp from 'sharp';
import { assertSpriteSeam } from './helpers/character-assets.mjs';
for(const character of ['vivian','nana']){
 const root=`public/chibi/motion/${character}/${character}`;
 const first=await sharp(`${root}-look-sheet.webp`).extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer();
 const idle=await sharp(`${root}-idle-sheet.webp`).ensureAlpha().raw().toBuffer();
 assertSpriteSeam(idle,first,`${character}: look begins at idle`);
 const blink=sharp(`${root}-blink-sheet.webp`);
 const metadata=await blink.metadata();assert.deepEqual([metadata.width,metadata.height],[2048,1536]);
}
console.log('Idle assets: look starts at idle and the 12-frame blink sheets are available.');
