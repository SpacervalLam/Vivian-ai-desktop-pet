import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';
const bundle = await build({entryPoints:['src/utils/companionGames.ts'],bundle:true,write:false,platform:'node',format:'esm'});
const game = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const old = Object.getOwnPropertyDescriptor(globalThis,'crypto');
Object.defineProperty(globalThis,'crypto',{configurable:true,value:webcrypto});
const hands = ['rock','paper','scissors'];
for (const player of hands) for(const companion of hands) {
  const result = game.rpsOutcome(player,companion);
  assert.equal(result === 'draw', player === companion);
  if(player !== companion) assert.notEqual(result,game.rpsOutcome(companion,player));
}
assert.equal(game.rpsOutcome('rock','scissors'),'win');
assert.equal(game.rpsOutcome('paper','rock'),'win');
assert.equal(game.rpsOutcome('scissors','paper'),'win');
for(let index=0;index<1000;index++) {
  const dice=game.rollDice();assert.ok(dice.player>=1&&dice.player<=100);assert.ok(dice.companion>=1&&dice.companion<=100);
  assert.equal(dice.outcome,dice.player===dice.companion?'draw':dice.player>dice.companion?'win':'lose');
}
let calls=0;
Object.defineProperty(globalThis,'crypto',{configurable:true,value:{getRandomValues(array){array[0]=calls++===0?4294967295:2;return array;}}});
assert.equal(game.randomInt(3),2);assert.equal(calls,2,'biased tail must be rejected');
assert.throws(()=>game.randomInt(0));
if(old) Object.defineProperty(globalThis,'crypto',old);else delete globalThis.crypto;
console.log('Companion games: all outcomes, dice bounds, unbiased randomness passed');
