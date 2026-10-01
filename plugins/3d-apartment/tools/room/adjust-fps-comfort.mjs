import fs from 'node:fs';
const p='plugins/3d-apartment/src/RoomScene.tsx';let s=fs.readFileSync(p,'utf8');s=s.replace(/const FPS_FOV = 52;[^\r\n]*/, 'const FPS_FOV = 60;        // 稳定纵向视野，室内兼顾周边视野与透视比例');s=s.replace('const aspect = w / Math.max(1, h);','const aspect = Math.max(1, w) / Math.max(1, h);');s=s.replace('const refFov = isFirstPerson ? FPS_FOV : BASE_FOV;\n      const halfH = Math.tan(THREE.MathUtils.degToRad(refFov) / 2) * Math.max(1, BASE_ASPECT / aspect);','const halfH = isFirstPerson\n        // 第一人称不套用全景构图补偿；超宽屏水平视野封顶 100°。\n        ? Math.min(Math.tan(THREE.MathUtils.degToRad(FPS_FOV) / 2), Math.tan(THREE.MathUtils.degToRad(100) / 2) / aspect)\n        : Math.tan(THREE.MathUtils.degToRad(BASE_FOV) / 2) * Math.max(1, BASE_ASPECT / aspect);');
// Accommodate CRLF source while keeping the replacement easy to audit.
if(s.includes('const refFov =')){s=s.replace(/const refFov = isFirstPerson \? FPS_FOV : BASE_FOV;\r?\n      const halfH = [^\r\n]+/,`const halfH = isFirstPerson
        // 第一人称不套用全景构图补偿；超宽屏水平视野封顶 100°。
        ? Math.min(Math.tan(THREE.MathUtils.degToRad(FPS_FOV) / 2), Math.tan(THREE.MathUtils.degToRad(100) / 2) / aspect)
        : Math.tan(THREE.MathUtils.degToRad(BASE_FOV) / 2) * Math.max(1, BASE_ASPECT / aspect);`);}
fs.writeFileSync(p,s);
const f='plugins/3d-apartment/src/anime/fpsControls.ts';s=fs.readFileSync(f,'utf8').replace('walkSpeed = 2.2','walkSpeed = 1.65').replace('sprintSpeed = 4.5','sprintSpeed = 3.4').replace('mouseSensitivity = 0.002','mouseSensitivity = 0.0015').replace('bobAmount = 0.018','bobAmount = 0').replace('sprintBobAmount = 0.03','sprintBobAmount = 0').replace('crouchBobAmount = 0.008','crouchBobAmount = 0').replace('// ---- 头部晃动（Head Bob）----','// ---- 头部晃动（默认关闭，保持探索时地平线稳定）----');fs.writeFileSync(f,s);
