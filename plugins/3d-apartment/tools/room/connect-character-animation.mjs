import fs from 'node:fs';const p='plugins/3d-apartment/src/RoomScene.tsx';let s=fs.readFileSync(p,'utf8');s="import { createCharacterAnimation } from './characterAnimation';\n"+s;s=s.replace('    const bodies: THREE.Object3D[] = [];','    const bodies: THREE.Object3D[] = [];\n    const characterAnimations: Array<ReturnType<typeof createCharacterAnimation>> = [];');s=s.replace('          body.add(model);','          body.add(model);\n          const animation=createCharacterAnimation(model,gltf.animations);\n          characterAnimations[idx]=animation;\n          if(animation)agents[idx].sitDuration=animation.sitDuration;');
const a=s.indexOf('        // 走路：踏步幅度更大'),b=s.indexOf('\n      }',a);if(a<0||b<0)throw Error('animation block');s=s.slice(0,a)+`        const animation=characterAnimations[i];
        if(animation){
          body.position.y=0;body.rotation.z=0;
          animation.update(elapsed,a.state,a.currentHotspotId,a.walkSpeed);
        }else{
          // Models without embedded clips retain the existing procedural fallback.
          const walking=a.state==='walk';
          const phase=clockT*(walking?9:1.6)+i*1.7;
          body.position.y=Math.abs(Math.sin(phase))*(walking?.032:.011);
          body.rotation.z=walking?Math.sin(phase)*.045:0;
        }`+s.slice(b);s=s.replace('      disposeBlenderFurniture();','      characterAnimations.forEach(animation=>animation?.dispose());\n      disposeBlenderFurniture();');fs.writeFileSync(p,s);
const hp='plugins/3d-apartment/src/agents/hotspots.ts';s=fs.readFileSync(hp,'utf8').replace('  id: string;','  id: string;\n  animation?: \'sit\';');fs.writeFileSync(hp,s);
const lp='plugins/3d-apartment/src/dormLayout.json';s=fs.readFileSync(lp,'utf8').replace('"id": "living-sofa", "pos"','"id": "living-sofa", "animation": "sit", "pos"');fs.writeFileSync(lp,s);
const ap='plugins/3d-apartment/src/agents/usePetAgent.ts';s=fs.readFileSync(ap,'utf8').replace('  walkSpeed = 0.8;','  walkSpeed = 0.8;\n  /** Loaded sitting clip duration; zero for models without skeletal animation. */\n  sitDuration = 0;');s=s.replace('            if (h) this.facing = h.facing;',"            if (h) {\n              this.facing = h.facing;\n              if(h.animation==='sit'&&this.sitDuration>0)this.stayTimer=Math.max(this.stayTimer,this.sitDuration+1);\n            }");fs.writeFileSync(ap,s);
