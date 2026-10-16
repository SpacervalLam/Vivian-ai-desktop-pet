import assert from 'node:assert/strict';
import { build } from 'esbuild';
const bundle=await build({entryPoints:['src/utils/screenFreezeResize.ts'],bundle:true,write:false,platform:'node',format:'esm'});
const {edgeResizeScale,FREEZE_EDGES}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
for (const edge of FREEZE_EDGES) {
  const dx=edge.includes('w')?-80:edge.includes('e')?80:0;
  const dy=edge.includes('n')?-60:edge.includes('s')?60:0;
  assert.equal(edgeResizeScale(edge,640,480,1,dx,dy),1.125,`${edge} enlarges equally on either axis`);
  assert.equal(edgeResizeScale(edge,640,480,1,-dx,-dy),0.875,`${edge} shrinks equally on either axis`);
}
assert.equal(edgeResizeScale('e',640,480,1,64000,0),8);
assert.equal(edgeResizeScale('n',640,480,1,0,48000),0.1);
assert.equal(edgeResizeScale('nw',640,480,2,-160,0),2.5,'corners work even when only one axis moves');
console.log('Snapshot edge resizing: eight directions, corners, continuing zoom and bounds passed');
