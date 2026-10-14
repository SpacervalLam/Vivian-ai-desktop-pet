import assert from 'node:assert/strict';
import { bookSpreads, bookDestination, bookSlots, bookFrame, adjacentBookSpread } from '../src/components/mind-inspector/pages/comicBookStructure.js';
import { comicMarkup, comicTitles } from '../src/components/mind-inspector/pages/comicMarkup.js';
import { existsSync } from 'node:fs';

assert.equal(bookSpreads.length, comicTitles.length - 1, 'the retired opening is excluded from the book');
for (let index = 0; index < bookSpreads.length; index++) {
  assert.equal(adjacentBookSpread(index, 1), Math.min(index + 1, bookSpreads.length - 1));
  assert.equal(adjacentBookSpread(index, -1), Math.max(index - 1, 0));
}
assert.equal(adjacentBookSpread(8, -1), 7, 'previous after a directory jump follows page order');
assert.deepEqual(bookSpreads.map(s => s.mind), [9, 6, 8, 1, 4, 2, 3, 7, 5], 'pages 08 and 09 exchange places in reading and directory order');
assert.equal(bookSpreads.length,9,'the book has nine pages after deleting page 10');
assert.ok(!comicTitles.includes('想说给你听'), 'the deleted page has no title or directory entry');
assert.ok(!bookSpreads.some(spread => spread.mind === 0), 'the old cover cannot be reached through book navigation');
const mixed = bookSpreads.filter(s => s.sources.length);
assert.equal(mixed.length, 4);
assert.deepEqual([...new Set(mixed.flatMap(s => s.sources))].sort(), ['diary', 'memory', 'notebook', 'planner']);
for (const slot of Object.values(bookSlots)) for (const [x,y,w,h] of [slot.r, slot.m]) assert.ok(x >= 0 && y >= 0 && x+w <= 100 && y+h <= 100);
const points = source => bookFrame(source).split(' ').map(p => p.split(',').map(Number));
const slope = (source, edge) => { const [a,b] = edge.map(i => points(source)[i]); const [, , w,h] = bookSlots[source].r; return ((b[0]-a[0])*w)/((b[1]-a[1])*h); };
assert.equal(slope('diary',[0,3]), 0, 'the full-height diary shares straight edges with the stacked café panels');
assert.ok(Math.abs(slope('planner',[1,2]) - (-33*7.39/100/61)) < .0001, 'planner and still life share a slope in stage coordinates');
assert.equal(slope('notebook',[1,2]), 0, 'notebook edge stays parallel to Vivian');
assert.equal(bookSlots.notebook.r[3], 100, 'the notebook fills the desktop canvas height');
assert.equal(points('notebook').length, 6, 'the notebook has a bottom-right inset notch');
const char = { name: '角色', primaryEmotion: '喜悦', thought: '此刻心声', goal: '真实目标', mood: '平静', valence: .5, arousal: .3, emotions: [], needs: [], relations: [], presence: '在线' };
const data = { vivian: char, nana: char };
const html = comicTitles.map((_,i) => comicMarkup(data,i,'/mind-comic/',0,true));
const panel09 = id => {
  const match = html[7].match(new RegExp(`class="panel-shadow" data-r="([^"]+)"[^>]*><div class="shot [^"]*" id="${id}" data-points='([^']+)'`));
  assert.ok(match, `${id} has readable frame geometry`);
  return { bounds: match[1].split(',').map(Number), points: JSON.parse(match[2]) };
};
const middle09 = panel09('skirt-full'), right09 = panel09('hold-n');
const edgeX = (panel, index) => panel.bounds[0] + panel.points[index][0] / 100 * panel.bounds[2];
assert.ok(Math.abs((edgeX(middle09, 2) - edgeX(middle09, 1)) - (edgeX(right09, 3) - edgeX(right09, 0))) < 1e-9,
  'page 09 adjacent full-height edges have equal slopes in canvas coordinates');
assert.ok(Math.abs((edgeX(right09, 0) - edgeX(middle09, 1)) - (edgeX(right09, 3) - edgeX(middle09, 2))) < 1e-9,
  'page 09 gutter has the same width at the top and bottom');
assert.ok(!html[6].includes('class="line-chat"'), 'decorative chat is replaced by real dialogue');
assert.ok(html[6].includes('line-phone-off-v33.webp') && html[6].includes('phone-screen-on') && html[6].includes('line-phone-on-v33.webp'), 'new phone artwork preloads both matching screen states');
assert.ok(!html[6].includes('bedside-phone-v14.webp'), 'old phone no longer appears in the spread');
assert.ok(!html[6].includes('DEAR YOU')&&!html[6].includes('字里行间'),'the conversation spread has no external title');
assert.ok(html[6].includes('time-caption')&&html[6].includes('早晨'),'a morning caption belongs inside the drawing');
assert.ok(html[6].includes('thought:vivian') && !html[6].includes('dialogue:vivian:needs'), 'page 03 shows the current thought rather than a canned need dialogue');
assert.ok(html[6].includes('compact cloud tail-left current-thought') && html[6].includes('data-i18n="comic_vivian_current_thought"'), 'current thought has a cloud bubble and translated label');
assert.ok(!html[7].includes('不急着说出口'), 'decorative margin is replaced by the note');
assert.ok(html[7].includes('id="skirt-hand"') && html[7].includes('hand-frames'), 'existing hand animation remains');
assert.equal(html[0], '', 'the retired cover produces no markup or image requests');
assert.ok(html[6].includes('thought:vivian'), 'the active conversation spread retains the current thought');
assert.ok(html[1].includes('雨声漫过站台') && html[1].includes('thought:nana') && html[1].includes('comic_nana_current_thought') && !html[1].includes('chapter-title') && !html[1].includes('你怎么淋湿了'), 'page 04 uses poetic scenery and Nana current thought without a promotional title or dialogue');
assert.ok(!html.join('').includes('TRAIN DATE') && !html.join('').includes('电车约会'), 'the book no longer depends on the train date theme');
// Live bindings follow the retained scenes; plans now occupy the planner panel.
const liveBindings = ['thought:vivian', 'thought:nana', 'dialogue:vivian:emotions', 'dialogue:nana:emotions', 'dialogue:nana:needs', 'presence:vivian'];
for (const key of liveBindings) {
  assert.ok(html.some(markup => markup.includes(`data-bind="${key}"`)), `live ${key} remains in the book`);
}
assert.doesNotMatch(html[5], /data-bind="goal:/, 'the planner panel replaces the decorative goal bubbles');
assert.ok(bookSpreads.find(spread => spread.mind === 5).sources.includes('planner'));
assert.equal((html[9].match(/class="panel-shadow"/g) || []).length, 3, 'morning uses three independent panels');
assert.ok(html[9].includes('page-01-vivian-waking-v01.webp') && html[9].includes('page-01-alarm-clock-v02.webp') && html[9].includes('page-01-morning-curtains-v02.webp'));
assert.doesNotMatch(html.join(''), /daily-seat|daily-message|daily-announcement|听见下一站。/, 'deleted page content is absent');
for(const asset of ['vivian-train-pole-v23','train-arrival-v20','train-straps-v20','train-tickets-v20']) {
  assert.equal(html.slice(8).join('').split(`${asset}.webp`).length-1,1,`reserved ${asset} appears once`);
}
assert.ok(!html[8].includes('goal:vivian'),'the trip has no goal text box');
assert.ok(html[4].includes('shared-handle-edit-v14a.webp') && html[4].includes('shared-handle-corrected-v46.webp'), 'the fifth page keeps the original upper panel and corrected lower hand panel');
assert.doesNotMatch(html[4], /class="(?:voice|narration|comic-sfx)|data-bind=/, 'the fifth page contains no text overlays');
for (const markup of html) for (const match of markup.matchAll(/src="\/mind-comic\/([^" ]+)"/g)) assert.ok(existsSync(new URL(`../public/mind-comic/${match[1]}`, import.meta.url)));
for (const [page, source] of Object.entries({ graph: 'memory', profile: 'memory', diary: 'diary', journal: 'diary', notebook: 'notebook', planner: 'planner', todo: 'planner', scheduler: 'planner' })) {
  const result = bookDestination(page);
  assert.ok(bookSpreads[result.spread].sources.includes(source), `${page} opens a spread containing its original content`);
}
assert.equal(bookDestination('diary', { diaryId: 'entry' }).focus, 'diary');
assert.equal(bookDestination('notebook', { notebookId: 'note' }).focus, 'notebook');
assert.equal(bookDestination('graph', { sub: 'profile' }).focus, 'memory');
assert.equal(bookDestination('mind').spread, 0);
assert.equal(bookDestination('diary').focus, null, 'ordinary navigation keeps a mixed spread');
console.log('Comic book: original page positions, seven reserved assets, live content, functional coverage and legacy navigation passed.');
