import fs from 'node:fs';const fp='plugins/3d-apartment/src/anime/fpsControls.ts';let s=fs.readFileSync(fp,'utf8');const anchor='  /** 当前世界坐标';const i=s.indexOf(anchor);if(i<0)throw Error('fps marker');s=s.slice(0,i)+`  /** Clear held input and momentum before/after a lift transfer. */
  stopMotion(): void {
    this.velocity.set(0,0,0);this.vy=0;
    this.moveForward=this.moveBackward=this.moveLeft=this.moveRight=false;
    this.sprint=this.jump=this.crouch=this.mouseRightDown=false;
    this.bobPhase=0;this.currentEyeY=this.targetEyeY=this.standHeight;
  }

`+s.slice(i);fs.writeFileSync(fp,s);
const p='plugins/3d-apartment/src/RoomScene.tsx';s=fs.readFileSync(p,'utf8');s="import { createApartmentLift } from './anime/apartmentLift';\n"+s;
const marker='    freezeStatic(apartmentShell.group);';if(!s.includes(marker))throw Error('shell marker');s=s.replace(marker,marker+`
    const apartmentLift=createApartmentLift(container,camera,fps);
    fpsColliders=fpsColliders.concat(apartmentLift.colliders);
    districtArt.prepare(apartmentLift.group);
    mergeByMaterial(apartmentLift.group);
    scene.add(apartmentLift.group,apartmentLift.dynamic);
    freezeStatic(apartmentLift.group);
    (window as any).__ROOM__.lift=apartmentLift;
    (window as any).__ROOM__.fps=fps;
`);
s=s.replace('        fps.update(elapsed, fpsColliderBuf);','        if (!apartmentLift.update(now, true)) fps.update(elapsed, fpsColliderBuf);');
s=s.replace('        // 观察者模式：WASD/方向键 水平飞行', '        apartmentLift.update(now, false);\n        // 观察者模式：WASD/方向键 水平飞行');
s=s.replace('      store.dispose();','      apartmentLift.dispose();\n      store.dispose();');
// A lobby/cabin is roofed even though it is outside unit 203.
s=s.replace('      rainIndoors = (x, y, z) =>\n        y > yLo && y < yHi &&\n        roofed.some((r) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1);',`      rainIndoors = (x, y, z) =>
        (x > -31 && x < 31 && z > -5.9 && z < 4.7 && y > 0 && y < 3.3) ||
        (x > -36.8 && x < -31 && z > -7.2 && z < 4.7 && y > 0 && y < 12.2) ||
        (y > yLo && y < yHi && roofed.some((r) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1));`);
// CRLF-safe alternate.
s=s.replace(/rainIndoors = \(x, y, z\) =>\r?\n        y > yLo && y < yHi &&\r?\n        roofed.some\(\(r\) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1\);/,`rainIndoors = (x, y, z) =>
        (x > -31 && x < 31 && z > -5.9 && z < 4.7 && y > 0 && y < 3.3) ||
        (x > -36.8 && x < -31 && z > -7.2 && z < 4.7 && y > 0 && y < 12.2) ||
        (y > yLo && y < yHi && roofed.some((r) => x > r.x0 && x < r.x1 && z > r.z0 && z < r.z1));`);
fs.writeFileSync(p,s);
