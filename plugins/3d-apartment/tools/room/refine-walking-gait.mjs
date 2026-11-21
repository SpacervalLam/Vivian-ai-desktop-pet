import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/fpsControls.ts';let s=fs.readFileSync(p,'utf8');s=s.replace('walkSpeed = 1.65','walkSpeed = 1.45').replace('sprintSpeed = 3.4','sprintSpeed = 3.1').replace('crouchSpeed = 1.2','crouchSpeed = 0.85').replace('groundAccel = 15','groundAccel = 9').replace('groundFriction = 6','groundFriction = 10');
const a=s.indexOf('  // ---- 头部晃动'),b=s.indexOf('  // ---- 内部状态',a);s=s.slice(0,a)+`  // Visual gait is removed before physics and reapplied after collision resolution.
  private bobPhase = 0;
  private gaitOffset = new THREE.Vector3();
  private gaitVertical = 0;
  private gaitLateral = 0;
  private gaitRoll = 0;
  private gaitPitch = 0;
  private clearGait(): void {
    this.camera.position.sub(this.gaitOffset);this.gaitOffset.set(0,0,0);
    this.bobPhase=this.gaitVertical=this.gaitLateral=this.gaitRoll=this.gaitPitch=0;
    this.updateOrientation();
  }

`+s.slice(b);
s=s.replace('    this.camera.position.set(x, y, z);','    this.clearGait();\n    this.camera.position.set(x, y, z);');s=s.replace('  stopMotion(): void {','  stopMotion(): void {\n    this.clearGait();');s=s.replace('return { x: this.camera.position.x, y: this.camera.position.y, z: this.camera.position.z };','return { x: this.camera.position.x-this.gaitOffset.x, y: this.camera.position.y-this.gaitOffset.y, z: this.camera.position.z-this.gaitOffset.z };');
s=s.replace('    const playerRadius = 0.15;', '    this.camera.position.sub(this.gaitOffset);this.gaitOffset.set(0,0,0);\n    const startX=this.camera.position.x,startZ=this.camera.position.z;\n    const playerRadius = 0.15;');
s=s.replace('const drop = control * this.groundFriction * delta;', 'const drop = control * (1-Math.exp(-this.groundFriction * delta));');
s=s.replace('    const wasMoving = moved || horizSpeed > 0.1;','    const distanceMoved = Math.hypot(this.camera.position.x-startX,this.camera.position.z-startZ);');
const c=s.indexOf('    // ---- 6. Head Bob'),d=s.indexOf('    // 调试遥测',c);s=s.slice(0,c)+`    // Distance-driven footsteps: one vertical pulse per step, alternating lateral sway.
    const actualSpeed=distanceMoved/Math.max(delta,.0001);
    const runBlend=THREE.MathUtils.smoothstep(actualSpeed,this.walkSpeed,this.sprintSpeed);
    const active=this.isGrounded&&actualSpeed>.025;
    const strength=active?Math.min(1,actualSpeed/this.walkSpeed)*(this.crouch?.38:1):0;
    const stepLength=THREE.MathUtils.lerp(.76,1.20,runBlend)*(this.crouch?.8:1);
    if(active)this.bobPhase=(this.bobPhase+distanceMoved/stepLength*Math.PI*2)%(Math.PI*4);
    const blend=1-Math.exp(-12*delta);
    const vertical=Math.sin(this.bobPhase)*THREE.MathUtils.lerp(.018,.033,runBlend)*strength;
    const lateral=Math.sin(this.bobPhase*.5)*THREE.MathUtils.lerp(.009,.017,runBlend)*strength;
    this.gaitVertical+=(vertical-this.gaitVertical)*blend;
    this.gaitLateral+=(lateral-this.gaitLateral)*blend;
    this.gaitRoll+=(Math.sin(this.bobPhase*.5)*.0025*strength-this.gaitRoll)*blend;
    this.gaitPitch+=(Math.cos(this.bobPhase)*.0018*strength-this.gaitPitch)*blend;
    this.gaitOffset.set(Math.cos(this.yaw)*this.gaitLateral,this.gaitVertical,-Math.sin(this.yaw)*this.gaitLateral);
    this.camera.position.add(this.gaitOffset);
    this.updateOrientation();

`+s.slice(d);
s=s.replace('      this.onUnlock?.();','      this.stopMotion();\n      this.onUnlock?.();');s=s.replace('    this.isLocked = false;\n    this.onUnlock?.();','    this.isLocked = false;\n    this.stopMotion();\n    this.onUnlock?.();');s=s.replace('    this.camera.rotateX(this.pitch);','    this.camera.rotateX(this.pitch + this.gaitPitch);\n    this.camera.rotateZ(this.gaitRoll);');fs.writeFileSync(p,s);
