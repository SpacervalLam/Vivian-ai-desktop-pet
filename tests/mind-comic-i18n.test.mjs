import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { build } from 'esbuild';
import { comicTranslations, comicText } from '../src/i18n/comic.js';
import { comicMarkup, comicTitles } from '../src/components/mind-inspector/pages/comicMarkup.js';
import { bookSpreads } from '../src/components/mind-inspector/pages/comicBookStructure.js';
import { dialogueBank } from '../src/components/mind-inspector/pages/comicDialogueBank.js';
import { localizedDialogueBanks } from '../src/components/mind-inspector/pages/comicDialogueTranslations.js';
import { createDialogueSelector } from '../src/components/mind-inspector/pages/comicDialogue.js';

const character = { name: 'Vivian', thought: 'Original thought', goal: 'Original goal', presence: 'Online', primaryEmotion: '喜悦',
  valence: .7, arousal: .6, emotions: [['喜悦', 65]], needs: [['归属', 70]], relations: [['信任', 55]] };
const data = { vivian: character, nana: { ...character, name: 'Nana' } };
for (const language of ['zh-CN','en','ja']) {
  const dictionary = comicTranslations[language];
  assert.deepEqual(Object.keys(dictionary), Object.keys(comicTranslations['zh-CN']));
  assert.equal(dictionary.contents, 'CONTENT');
  for (const page of bookSpreads) assert(dictionary[`title${page.mind}`]);
  for (let page = 0; page < comicTitles.length; page++) {
    const html = comicMarkup(data, page, '/mind-comic/', 0, true, language);
    assert(!html.includes('undefined'));
    for (const match of html.matchAll(/data-i18n(?:-alt)?="comic\.([^"]+)"/g)) assert(dictionary[match[1]], `${language}: ${match[1]}`);
    if (language === 'en') assert.doesNotMatch(html.replace(/<[^>]+>/g, ''), /\p{Script=Han}/u, `English panel ${page} has no Chinese UI copy`);
  }
}
assert.deepEqual([0,2,7,8].map(index => comicText(`title${bookSpreads[index].mind}`)), ['清晨','电车里','留下的足迹','约定']);
for (const language of ['en','ja']) {
  const selector = createDialogueSelector();
  for (const role of ['vivian','nana']) for (const [category,states] of Object.entries(dialogueBank[role])) {
    const translated = localizedDialogueBanks[language][role][category];
    assert.deepEqual(Object.keys(translated), Object.keys(states));
    for (const [state,variants] of Object.entries(translated)) {
      assert.equal(variants.length, states[state].length);
      assert(variants.every(value => typeof value === 'string' && value.trim()));
      if (language === 'en') variants.forEach(value => assert.doesNotMatch(value, /\p{Script=Han}/u));
      if (category !== 'mood') for (const [tier,value] of [10,35,60,90].entries()) {
        const current = { ...character, primaryEmotion: state, [category]: [[state,value]] };
        assert.equal(selector(role, category, current, 0, language), variants[tier]);
      }
    }
    const suffix = role === 'vivian' ? 'Vivian' : 'Nana';
    assert.equal(selector(role, category, { ...character, status: 'loading' }, 0, language), comicText(`loading${suffix}`, language));
    assert.equal(selector(role, category, { ...character, error: 'failed' }, 0, language), comicText(`error${suffix}`, language));
  }
}

// Switch languages against the same mounted text/image nodes, never replacing markup.
const bundle = await build({ entryPoints: ['src/components/mind-inspector/pages/comicBind.ts'], bundle: true, write: false, format: 'esm' });
const { translateComicLabels, patchComicData, fitComicText } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const narration = { style: { fontSize: '24px', removeProperty() {} }, classList: { contains: () => false },
  parentElement: { clientHeight: 30, children: [], classList: { contains: () => false } },
  clientWidth: 50, scrollWidth: 200, scrollHeight: 80 };
globalThis.getComputedStyle = () => ({ fontSize: '24px', paddingTop: '0', paddingBottom: '0', marginTop: '0', marginBottom: '0' });
const narrationRoot = { querySelectorAll: selector => selector.includes('.narration') ? [narration] : [] };
fitComicText(narrationRoot);
assert.equal(narration.style.fontSize, '24px', 'fixed narration must retain its readable authored size');
narration.children = Array.from({ length: 30 }, () => ({ className: 'rain-type-character' }));
fitComicText(narrationRoot);
assert.equal(narration.style.fontSize, '24px', 'splitting narration into animated glyphs must not shrink its font');
const title = { dataset: { i18n: 'comic.morning' }, textContent: '' };
const image = { dataset: { i18nAlt: 'comic.alt_line-phone' }, alt: '' };
const thought = { textContent: '' }, dialogue = { textContent: '' };
const root = { querySelectorAll: selector => selector === '[data-i18n]' ? [title] : selector === '[data-i18n-alt]' ? [image]
  : selector === '[data-bind="thought:vivian"]' ? [thought] : selector === '[data-bind="dialogue:nana:needs"]' ? [dialogue] : [],
  set innerHTML(_) { throw new Error('Language changes must not replace comic markup'); } };
for (const language of ['zh-CN','en','ja']) {
  translateComicLabels(root, key => comicText(key.replace(/^comic\./, ''), language));
  patchComicData(root, { ...data, vivian: { ...character, thought: '用户写下的原始心声' } }, 0, language);
  assert.equal(title.textContent, comicText('morning', language));
  assert.equal(image.alt, comicText('alt_line-phone', language));
  assert.equal(thought.textContent, '用户写下的原始心声');
  assert.equal(root.querySelectorAll('[data-i18n]')[0], title);
}
for (const file of ['UnifiedComicBook.tsx','MemoryPage.tsx','ComicConversation.tsx']) {
  const source = readFileSync(`src/components/mind-inspector/pages/${file}`, 'utf8');
  for (const key of source.matchAll(/t\(['"]comic\.([^'"]+)['"]/g)) assert(comicTranslations.en[key[1]], `${file}: missing ${key[1]}`);
}
console.log('Comic i18n: all labels, captions, image descriptions, 200 dialogue variants, state messages and in-place language switching passed.');
