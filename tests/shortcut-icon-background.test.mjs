import assert from 'node:assert/strict';
import { shortcutIconBackground } from '../src/utils/shortcutIconBackground.ts';
const image = (width, height, pixel) => {
  const bytes = new Uint8ClampedArray(width * height * 4);
  for (let y=0; y<height; y++) for(let x=0; x<width; x++) bytes.set(pixel(x,y), (y*width+x)*4);
  return bytes;
};
assert.equal(shortcutIconBackground(image(12,12,()=>[240,220,40,255]),12,12),'rgb(240, 220, 40)');
const corners=image(12,12,(x,y)=> (x===0||x===11)&&(y===0||y===11) ? [20,60,180,255] : [x*20,y*20,30,255]);
assert.equal(shortcutIconBackground(corners,12,12),'rgb(20, 60, 180)', 'corners outweigh scattered edge shades');
const cutout=color=>image(12,12,(x,y)=>x>1&&x<10&&y>1&&y<10 ? [...color,255] : [0,0,0,0]);
assert.equal(shortcutIconBackground(cutout([0,0,0]),12,12),'rgb(255, 255, 255)');
assert.equal(shortcutIconBackground(cutout([255,255,255]),12,12),'rgb(0, 0, 0)');
assert.equal(shortcutIconBackground(cutout([0,255,0]),12,12),'rgb(255, 0, 255)');
assert.equal(shortcutIconBackground(image(2,2,()=>[0,0,0,0]),2,2),'#f2f2f7');
console.log('Icon backgrounds: opaque edge dominance, corner weights, transparent contrast and empty fallback passed.');
