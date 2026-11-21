import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundled = await build({ stdin: { contents: `export * from './src/chibi/movementPlayback';export * from './src/chibi/walkPlan';export * from './src/chibi/slideTrack';`, resolveDir:process.cwd() }, bundle:true, write:false, platform:'node', format:'esm' });
const { quickTurnPlayback, playWalkingFrames, planSmartMove, planAmbientWalk, runSlide, walkCycleTravelPx } = await import('data:text/javascript;base64,'+Buffer.from(bundled.outputFiles[0].text).toString('base64'));
const turn = quickTurnPlayback(8);
assert.deepEqual(turn.frames,[0,1,2,3,4,5,6,7]);
assert.equal(turn.durations.reduce((a,b)=>a+b,0),160);
assert(turn.durations.every(ms=>ms>=20),'GIF players must not clamp short turn frames to 100ms');
for (const distance of [40,80,140,300,540,850,2000]) for (const plan of [planSmartMove(distance,10),planAmbientWalk(distance,1)]) {
  assert.equal(plan.frames%14,0,'movement ends on the shared side stance');
  assert.equal(plan.durationMs,plan.frames*plan.frameDelayMs,'feet and window share the adjusted duration');
  assert(plan.frameDelayMs>=58&&plan.frameDelayMs<=105);
}
assert.equal(planSmartMove(5,400).walking,false);
const stroll = planAmbientWalk(560, 1);
assert.equal(stroll.frames, 56, 'four short two-step cycles cover 560px without long gliding strides');
assert(Math.abs(560 / stroll.durationMs - 140 / 1090) < 0.002, 'ambient travel matches the authored cadence');
const smallStride=walkCycleTravelPx('nana','right');
assert.equal(smallStride,100);
assert.equal(walkCycleTravelPx('nana','left'),140);
assert.equal(walkCycleTravelPx('vivian','right'),140);
const smallWalk=planAmbientWalk(400,1,smallStride);
assert.equal(smallWalk.frames,56,'Nana takes four measured gait cycles over 400px');
assert.equal(planSmartMove(400,0,smallStride).frames,56,'avoidance uses the same stride profile');
for(const distance of [300,560,850,2000]) {
  const avoiding=planSmartMove(distance,10);
  const cycleTravel=distance/(avoiding.frames/14);
  assert(cycleTravel>=100&&cycleTravel<=180,'long avoidance moves add steps instead of sliding one gait across the screen');
}
let now=0;const shown=[];
const clock={now:()=>now,wait:async ms=>{now+=ms;}};
assert.equal(await playWalkingFrames({frames:28,frameDelayMs:80,cycleFrames:14,show:frame=>shown.push(frame),interrupted:()=>false},clock),true);
assert.deepEqual(shown,[...Array.from({length:28},(_,i)=>i%14),0]);
assert.equal(now,2240);
now=0;const late=[];let waits=0;
await playWalkingFrames({frames:14,frameDelayMs:80,cycleFrames:14,show:frame=>late.push(frame),interrupted:()=>false},{now:()=>now,wait:async ms=>{now+=ms+(waits++===2?150:0);}});
assert.equal(now,1120,'late callbacks skip missed poses instead of extending the walk');
assert.equal(late.at(-1),0);
assert(late.length<15);
now=0;const cancelled=[];
assert.equal(await playWalkingFrames({frames:14,frameDelayMs:80,cycleFrames:14,show:frame=>cancelled.push(frame),interrupted:()=>now>=240},clock),false);
assert.deepEqual(cancelled,[0,1,2],'interruption cannot paint another frame');
const savedWindow=globalThis.window,savedPerformance=globalThis.performance;
try {
  now=0;
  globalThis.performance={now:()=>now};
  globalThis.window={setTimeout:(callback,ms)=>{now+=ms;queueMicrotask(callback);}};
  for(const direction of [-1,1]) {
    now=0;const positions=[];
    await runSlide({fromX:0,fromY:0,toX:direction*560,toY:0,durationMs:stroll.durationMs,linear:true,
      apply:(x,y)=>positions.push({x,y,at:now})});
    for(const {x,y,at} of positions) {
      assert(Math.abs(x-direction*560*at/stroll.durationMs)<=0.5,
        'constant foot cadence needs constant travel, including the first and last step');
      assert.equal(y,0);
    }
    assert.equal(positions.at(-1).x,direction*560);
    assert.equal(now,stroll.durationMs);
  }
} finally {
  if(savedWindow===undefined)delete globalThis.window;else globalThis.window=savedWindow;
  globalThis.performance=savedPerformance;
}
console.log('Walking: 160ms GIF-safe turns, complete gait cycles, shared timing, late callbacks and interruption passed.');
