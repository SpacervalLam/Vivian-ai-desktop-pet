import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import { getMixer, directSetParam } from './LayeredParameterMixer';

export interface EmotionState {
  joy: number;
  sadness: number;
  anger: number;
  fear: number;
  closeness: number;
  loneliness: number;
  curiosity: number;
}

export interface FacsChannels {
  browInnerUp: number;
  browDown: number;
  eyeSmile: number;
  eyeSquint: number;
  eyeOpen: number;
  mouthSmile: number;
  mouthFrown: number;
  mouthOpen: number;
  cheekPuff: number;
  blush: number;
  headZ: number;
  headY: number;
}

export const DEFAULT_FACS: FacsChannels = {
  browInnerUp: 0,
  browDown: 0,
  eyeSmile: 0,
  eyeSquint: 0,
  eyeOpen: 1,
  mouthSmile: 0.04,
  mouthFrown: 0,
  mouthOpen: 0,
  cheekPuff: 0,
  blush: 0,
  headZ: 0,
  headY: 0,
};

export function emotionToFacs(e: EmotionState): FacsChannels {
  const joy = clamp01(e.joy);
  const sad = clamp01(e.sadness);
  const anger = clamp01(e.anger);
  const fear = clamp01(e.fear);
  const close = clamp01(e.closeness);
  const lonely = clamp01(e.loneliness);
  const curious = clamp01(e.curiosity);

  const mouthSmile = joy * 0.42 + close * 0.22 + curious * 0.06;
  const mouthFrown = sad * 0.36 + anger * 0.20 + lonely * 0.18;
  const eyeSmile = joy * 0.38 + close * 0.24 + curious * 0.04;
  const eyeSquint = anger * 0.30 + sad * 0.12;
  const eyeOpen = clamp(1 + fear * 0.20 + curious * 0.10 - eyeSquint * 0.30, 0.70, 1.0);
  const browInnerUp = sad * 0.26 + fear * 0.22 + curious * 0.10;
  const browDown = anger * 0.34 + sad * 0.08;
  const cheekPuff = joy * 0.18 + anger * 0.14;
  const blush = close * 0.32 + joy * 0.12;
  const headZ = close * 0.06 - anger * 0.05 + curious * 0.03;
  const headY = sad * 0.12 + lonely * 0.10 - joy * 0.04;

  return {
    browInnerUp,
    browDown,
    eyeSmile,
    eyeSquint,
    eyeOpen,
    mouthSmile,
    mouthFrown,
    mouthOpen: 0,
    cheekPuff,
    blush,
    headZ,
    headY,
  };
}

export function facsToCubismParams(facs: FacsChannels): Record<string, number> {
  const params: Record<string, number> = {};

  const browL = clamp(facs.browInnerUp * 0.8 - facs.browDown * 0.6, -1, 1);
  const browR = browL;
  params['ParamBrowLY'] = browL;
  params['ParamBrowRY'] = browR;
  if (facs.browDown > 0.1) {
    params['Brows'] = facs.browDown * 0.7;
    params['Brow'] = facs.browDown * 0.7;
  }

  params['ParamEyeLSmile'] = clamp(facs.eyeSmile, 0, 1);
  params['ParamEyeRSmile'] = clamp(facs.eyeSmile, 0, 1);

  params['ParamEyeLOpen'] = clamp(facs.eyeOpen - facs.eyeSquint * 0.4, 0, 1.0);
  params['ParamEyeROpen'] = params['ParamEyeLOpen'];

  const mouthForm = clamp(facs.mouthSmile - facs.mouthFrown * 0.8, -1, 1);
  params['ParamMouthForm'] = mouthForm;

  if (facs.mouthOpen > 0.01) {
    params['ParamMouthOpenY'] = clamp(facs.mouthOpen, 0, 1);
    params['JawOpen'] = params['ParamMouthOpenY'];
    params['Jawopen'] = params['ParamMouthOpenY'];
  }

  const cheek = clamp(facs.cheekPuff, 0, 1);
  if (cheek > 0.01) {
    params['CheekPuff'] = cheek;
    params['CheeckPuff'] = cheek;
  }

  const blush = clamp(facs.blush, 0, 1);
  if (blush > 0.01) {
    params['ParamCheek'] = blush;
  }

  // 持续写入 ParamAngleZ/Y，避免 |headZ|/|headY| 在 0.001 阈值附近抖动时
  // targetParams 时有时无，导致 smoother 状态与 emotion 层不一致引起尾巴抽搐
  params['ParamAngleZ'] = clamp(facs.headZ * 30, -30, 30);
  params['ParamAngleY'] = clamp(facs.headY * 10, -10, 10);

  return params;
}

export class FacsSmoother {
  private current: Record<string, number> = {};
  private readonly speed: number;

  constructor(speed = 14) {
    this.speed = speed;
  }

  smooth(target: Record<string, number>, deltaSeconds: number): Record<string, number> {
    const factor = 1 - Math.exp(-this.speed * Math.max(0, deltaSeconds));
    const result: Record<string, number> = {};
    const keys = new Set([...Object.keys(target), ...Object.keys(this.current)]);
    for (const key of keys) {
      const t = target[key] ?? 0;
      const c = this.current[key] ?? 0;
      const next = c + (t - c) * factor;
      this.current[key] = next;
      result[key] = next;
    }
    return result;
  }

  reset(): void {
    this.current = {};
  }
}

export function applyEmotionParamsToModel(
  model: Live2DModel | null,
  params: Record<string, number>,
  weight = 1.0,
): void {
  if (!model) return;
  const mixer = getMixer(model);
  for (const [id, value] of Object.entries(params)) {
    const weighted = id === 'ParamAngleY' || id === 'ParamAngleZ'
      ? value * weight
      : value;
    if (mixer) {
      mixer.setParameter('emotion', id, weighted);
    } else {
      directSetParam(model, id, weighted);
    }
  }
}

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}
