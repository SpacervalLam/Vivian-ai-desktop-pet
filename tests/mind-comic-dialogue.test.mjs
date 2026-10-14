import assert from 'node:assert/strict';
import { dialogueBank } from '../src/components/mind-inspector/pages/comicDialogueBank.js';
import { createDialogueSelector } from '../src/components/mind-inspector/pages/comicDialogue.js';

for (const role of ['vivian','nana']) {
  const lines=Object.values(dialogueBank[role]).flatMap(group=>Object.values(group).flat());
  assert.equal(lines.length,100);
  assert.equal(new Set(lines).size,100);
  for (const category of ['emotions','needs','relations']) {
    for (const [key,variants] of Object.entries(dialogueBank[role][category])) {
      for (const [tier,value] of [10,35,60,90].entries()) {
        const select=createDialogueSelector();
        assert.equal(select(role,category,{[category]:[[key,value]],primaryEmotion:key}),variants[tier]);
      }
    }
  }
}
const select=createDialogueSelector();
const needs=value=>({needs:[['归属',value]]});
const initial=select('vivian','needs',needs(49));
assert.equal(select('vivian','needs',needs(51)),initial);
assert.notEqual(select('vivian','needs',needs(60)),initial);
assert.equal(createDialogueSelector()('nana','needs',{needs:[['安全',0]]}),dialogueBank.nana.needs.安全[0]);
assert(createDialogueSelector()('nana','needs',{needs:[]}).includes('暂时未明'));
assert(createDialogueSelector()('vivian','mood',{error:'offline'}).includes('没接上'));
for(const role of ['vivian','nana']) {
 const c={valence:.7,arousal:.8};
 assert.equal(select(role,'mood',c,5),select(role,'mood',c,5));
 assert(dialogueBank[role].mood.joyful.includes(select(role,'mood',c,5)));
}
console.log('Comic dialogue: 100 unique lines per character, every metric tier, hysteresis, real zero, missing/error states and stable mood variants passed');
