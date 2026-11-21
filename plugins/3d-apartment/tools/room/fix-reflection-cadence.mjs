import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/storeDetails.ts';let s=fs.readFileSync(p,'utf8');s=s.replace('// A small, throttled planar reflection, not a second full-resolution city render per frame.','// Low-resolution planar reflection follows the camera every visible frame.');const a=s.indexOf('  let nextReflection = 0;'),b=s.indexOf('  reflection.onBeforeRender =',a);s=s.slice(0,a)+`  // Camera motion changes the entire reflected view, even for static buildings.
  // A time throttle freezes both the capture and its projection, causing visible judder.
  // Keep the 512 x 256 target, distance fade and Reflector's back-face/frustum culling
  // to bound cost without decoupling the reflection from the displayed frame.
`+s.slice(b);s=s.replace(/    const now = performance.now\(\);\r?\n    if \(now < nextReflection\) return;\r?\n    nextReflection = now \+ REFLECTION_INTERVAL;\r?\n/,'');fs.writeFileSync(p,s);
