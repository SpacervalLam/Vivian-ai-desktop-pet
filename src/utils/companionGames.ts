export type Hand = 'rock' | 'paper' | 'scissors';
export type Outcome = 'win' | 'lose' | 'draw';
/** Rejection sampling keeps all outcomes equiprobable. */
export function randomInt(max: number): number {
  if (!Number.isSafeInteger(max) || max < 1 || max > 0x100000000) throw new Error('Invalid random range');
  const limit = Math.floor(0x100000000 / max) * max;
  const values = new Uint32Array(1);
  do { crypto.getRandomValues(values); } while (values[0] >= limit);
  return values[0] % max;
}
export function rpsOutcome(player: Hand, companion: Hand): Outcome {
  if (player === companion) return 'draw';
  return ({ rock: 'scissors', paper: 'rock', scissors: 'paper' })[player] === companion ? 'win' : 'lose';
}
export function playRps(player: Hand) {
  const companion: Hand = (['rock', 'paper', 'scissors'] as const)[randomInt(3)];
  return { player, companion, outcome: rpsOutcome(player, companion) };
}
export function rollDice() {
  const player = randomInt(100) + 1, companion = randomInt(100) + 1;
  return { player, companion, outcome: (player === companion ? 'draw' : player > companion ? 'win' : 'lose') as Outcome };
}
