import assert from 'node:assert/strict';
import sharp from 'sharp';
const visiblePixels=buffer=>{
 for(let i=0;i<buffer.length;i+=4)if(buffer[i+3]===0)buffer.fill(0,i,i+3);
 return buffer;
};
for(const character of ['vivian','nana']){
 const root=`public/chibi/motion/${character}/${character}`;
 const first=visiblePixels(await sharp(`${root}-look-sheet.webp`).extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer());
 const idle=visiblePixels(await sharp(`${root}-idle-sheet.webp`).ensureAlpha().raw().toBuffer());
 assert.ok(idle.equals(first),`${character}: idle is pixel-identical to look frame one`);
 const blink=sharp(`${root}-blink-sheet.webp`);
 const metadata=await blink.metadata();assert.deepEqual([metadata.width,metadata.height],[1536,1024]);
}
console.log('Idle assets: exact look opening frame retained; original blink sheets available.');
