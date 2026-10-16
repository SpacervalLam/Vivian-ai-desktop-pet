import assert from 'node:assert/strict';
import fs from 'node:fs';
import { comicMarkup, comicTitles } from '../src/components/mind-inspector/pages/comicMarkup.js';
import { selectComicDialogue } from '../src/components/mind-inspector/pages/comicDialogue.js';

const character = name => ({ name, primaryEmotion: '悲伤', presence: '在线 · 3秒', thought: '<img src=x onerror=alert(1)>', goal: '<script>bad()</script>', mood: '<b>真实心情</b>', valence: -.27, arousal: .13, emotions: ['喜悦','悲伤','愤怒','恐惧','亲近','孤独','好奇'].map(k => [k,k === '悲伤' ? 83 : 0]), needs: ['归属','自主','安全','新奇','表达'].map(k => [k,0]), relations: ['信任','亲密','尊重','依赖','熟悉'].map(k => [k,'—']) });
const data = { vivian: character('Vivian'), nana: character('Nana') };
// User-facing reading order: a later panel waits for the previous entrance.
const orderedShots = (page, ids) => {
  const html = comicMarkup(data, page, '/mind-comic/', 0, true);
  const starts = ids.map(id => Number(html.match(new RegExp(`id="${id}"[^>]*--delay:([\\d.]+)ms`))?.[1]));
  assert(starts.every(Number.isFinite));
  for (let index = 1; index < starts.length; index++) {
    assert(starts[index] >= starts[index - 1] + 1150, `${ids[index]} must wait for ${ids[index - 1]} to finish`);
  }
};
orderedShots(9, ['morning-curtains', 'morning-alarm', 'morning-waking']);
orderedShots(6, ['line-v', 'line-phone']);
orderedShots(8, ['daily-arrival', 'daily-straps', 'daily-tickets', 'daily-pole']);
orderedShots(1, ['platform-light', 'rain-soaked-detail', 'waiting-n']);
orderedShots(4, ['together', 'hands', 'door-smile']);
assert.doesNotMatch(comicMarkup(data, 6, '/mind-comic/', 0, true), /comic-source-stamp|此刻状态/);
const assets = [];
for (let page = 0; page < comicTitles.length; page++) {
  const html = comicMarkup(data,page,'/mind-comic/');
  assert(!html.includes('<script>') && !html.includes('<img src=x') && !html.includes('<b>真实心情</b>'));
  assert(!html.includes('undefined'));
  for (const bubble of html.matchAll(/<div class="voice [^>]*>(.*?)<\/div>/gs)) assert(!bubble[1].includes('<small>'), 'speech bubbles contain only speech');
  assets.push(...[...html.matchAll(/src="\/mind-comic\/([^" ]+)"/g)].map(m => m[1]));
}
assert.equal(assets.length-new Set(assets).size,1);
assert.equal(assets.filter(x=>x==='vivian-nana-day/page-04-nana-rain-soaked-v02.webp').length,1,'the reunion page uses the supplied wet-skirt detail once');
assert.match(comicMarkup(data,1,'/mind-comic/'), /class="shot mirror" id="waiting-n"[^]*?nana-umbrella-v02\.webp/, 'Nana faces away from Vivian in the mirrored right panel');
assert.equal(assets.filter(x=>x==='vivian-skirt-hand-v25.webp').length,2);
const detail=comicMarkup(data,7,'/mind-comic/');
assert(detail.includes('id="skirt-full"')&&detail.includes('id="skirt-hand"')&&detail.includes('full-drawing'));
assert.match(detail, /id="hold-n"[^>]*--delay:0ms/, 'page 09 starts with the rightmost panel');
assert.match(detail, /id="skirt-full"[^>]*--delay:1150ms/, 'page 09 then introduces the middle panel');
for (const file of assets) assert(fs.existsSync(`public/mind-comic/${file}`));
const mood = comicMarkup(data,1,'/mind-comic/');
assert(!mood.includes('悲伤 83') && !mood.includes('-0.27') && !mood.includes('0.13'));
for(let page=1;page<=4;page++){
 const html=comicMarkup(data,page,'/mind-comic/');
 assert(!html.includes('0–100')&&!html.includes('class="metrics"')&&!html.includes('mood-link'));
}
const missing = structuredClone(data);
missing.nana.valence = null;
missing.nana.arousal = null;
assert(selectComicDialogue('nana', 'mood', missing.nana).includes('心绪暂时未明'));
const train = comicMarkup(data, 8, '/mind-comic/');
assert.doesNotMatch(train, /把平常的日子/);
for (const [id, delay] of [['daily-arrival', 0], ['daily-straps', 1600], ['daily-pole', 3900], ['daily-tickets', 2750]]) {
  const match = train.match(new RegExp(`id="${id}"[^>]*--delay:([\\d.]+)ms`));
  assert.ok(match, `${id} has an entrance delay`);
  assert.ok(Math.abs(Number(match[1]) - delay) < .001, `${id} follows the preceding entrance`);
}
assert.match(train, /--delay:1150ms[^>]*><span[^>]*><span data-i18n="comic.trainSound"[^>]*>咔 哒/);
assert.match(train, /--delay:5050ms[^>]*>把约定带在身边。/);
const cafe = comicMarkup(data, 3, '/mind-comic/', 0, true);
assert.match(cafe, /cafe-rain-exterior-v02\.webp/);
assert.match(cafe, /duo-window-handdrawn-v02\.webp/);
assert.doesNotMatch(cafe, /diary-window|nana-diary-writing|chapter-title|class="voice/);
for (const [id, delay] of [['cafe-exterior', 0], ['cafe-interior', 1150]]) {
  const match = cafe.match(new RegExp(`id="${id}"[^>]*--delay:([\\d.]+)ms`));
  assert.ok(match, `${id} has an entrance delay`);
  assert.ok(Math.abs(Number(match[1]) - delay) < .001, `${id} follows the preceding café scene`);
}
console.log('Comic overview: dialogue instead of raw numbers, unavailable data, safe text and packaged WebP assets passed');
