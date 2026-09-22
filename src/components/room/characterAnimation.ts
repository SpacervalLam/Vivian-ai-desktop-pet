import * as THREE from 'three';
import type { AgentState } from './agents/usePetAgent';
import { findHotspot } from './agents/hotspots';

/** Animation only owns bones; navigation and model normalization stay on parent groups. */
export function createCharacterAnimation(model:THREE.Object3D,clips:THREE.AnimationClip[]) {
  const find=(name:string)=>clips.find(c=>c.name.toLowerCase().replace(/\.\d+$/,'')===name);
  const idle=find('look_around')??find('standing_relax'),walk=find('walk'),sit=find('sit');
  if(!idle&&!walk&&!sit)return null;
  const mixer=new THREE.AnimationMixer(model);
  const actions=new Map<THREE.AnimationClip,THREE.AnimationAction>();
  for(const clip of [idle,walk,sit])if(clip){const action=mixer.clipAction(clip);action.setLoop(clip===sit?THREE.LoopOnce:THREE.LoopRepeat,clip===sit?1:Infinity);action.clampWhenFinished=clip===sit;actions.set(clip,action);}
  let current:THREE.AnimationAction|null=null;
  return {
    sitDuration:sit?.duration??0,
    update(dt:number,state:AgentState,hotspot:string|null,speed:number){
      // mount 阶段就开始播坐下：落座那 0.5~1.2s 里角色正在往坐面上挪，
      // 等坐稳了才切 sit 的话会出现"站着滑过去、坐好才突然矮下去"的两段式动作。
      const sitting=(state==='stay'||state==='mount')&&hotspot!==null&&findHotspot(hotspot)?.animation==='sit';
      const clip=(state==='walk'?walk:sitting?sit:idle)??idle??walk??sit!;
      const next=actions.get(clip)!;
      if(next!==current){
        next.reset().setEffectiveTimeScale(1).setEffectiveWeight(1).play();
        if(current){current.fadeOut(.30);next.fadeIn(.30);}else next.fadeIn(.18);
        current=next;model.userData.activeAnimation=clip.name;
      }
      if(clip===walk)next.setEffectiveTimeScale(THREE.MathUtils.clamp(speed/.8,.5,1.8));
      mixer.update(dt);
    },
    dispose(){mixer.stopAllAction();mixer.uncacheRoot(model);delete model.userData.activeAnimation;}
  };
}
