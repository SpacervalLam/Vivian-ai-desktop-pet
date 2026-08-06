import type { Live2DModel } from 'pixi-live2d-display/cubism4';

export type ParameterLayer = 'idle' | 'emotion' | 'blink' | 'instant' | 'speech' | 'manual';

const LAYER_PRIORITY: Record<ParameterLayer, number> = {
  idle: 0,
  emotion: 1,
  blink: 1.2,
  instant: 1.5,
  speech: 3,
  manual: 4,
};

const MOUTH_OPEN_PARAMS = new Set(['ParamMouthOpenY', 'JawOpen', 'Jawopen']);

interface CoreModel {
  setParameterValueById?: (id: string, value: number, weight?: number) => void;
  setParameterValue?: (id: string, value: number, weight?: number) => void;
}

interface InternalModel {
  update?: (dt: number, now: number) => void;
  coreModel?: CoreModel;
}

const MIXER_KEY = '__vivianMixer';

export function getMixer(model: Live2DModel | null): LayeredParameterMixer | null {
  if (!model) return null;
  return (model as unknown as Record<string, unknown>)[MIXER_KEY] as LayeredParameterMixer | null;
}

export function setMixer(model: Live2DModel | null, mixer: LayeredParameterMixer | null): void {
  if (!model) return;
  (model as unknown as Record<string, unknown>)[MIXER_KEY] = mixer;
}

export function createLayeredSetter(
  model: Live2DModel | null,
  layer: ParameterLayer,
): (id: string, value: number) => void {
  return (id: string, value: number) => {
    const mixer = getMixer(model);
    if (mixer) {
      mixer.setParameter(layer, id, value);
    } else {
      directSetParam(model, id, value);
    }
  };
}

export function directSetParam(
  model: Live2DModel | null,
  id: string,
  value: number,
  weight = 1.0,
): void {
  if (!model) return;
  try {
    const internal = (model as unknown as { internalModel?: InternalModel }).internalModel;
    const core = internal?.coreModel;
    if (!core) return;
    if (typeof core.setParameterValueById === 'function') {
      core.setParameterValueById(id, value, weight);
    } else if (typeof core.setParameterValue === 'function') {
      core.setParameterValue(id, value, weight);
    }
  } catch {
    /* ignore */
  }
}

export class LayeredParameterMixer {
  private layers: Map<ParameterLayer, Map<string, number>> = new Map();
  private model: Live2DModel | null = null;
  private originalUpdate: ((dt: number, now: number) => void) | null = null;
  private installed = false;

  constructor(model: Live2DModel | null) {
    this.model = model;
    for (const layer of Object.keys(LAYER_PRIORITY) as ParameterLayer[]) {
      this.layers.set(layer, new Map());
    }
    this.installHook();
  }

  setParameter(layer: ParameterLayer, id: string, value: number): void {
    this.layers.get(layer)?.set(id, value);
  }

  clearLayer(layer: ParameterLayer): void {
    this.layers.get(layer)?.clear();
  }

  clearLayerParam(layer: ParameterLayer, id: string): void {
    this.layers.get(layer)?.delete(id);
  }

  private installHook(): void {
    if (!this.model || this.installed) return;
    const internal = (this.model as unknown as { internalModel?: InternalModel }).internalModel;
    if (!internal || typeof internal.update !== 'function') return;

    this.originalUpdate = internal.update.bind(internal);
    const self = this;
    internal.update = function (dt: number, now: number) {
      if (self.originalUpdate) {
        self.originalUpdate(dt, now);
      }
      self.applyToModel();
    };
    this.installed = true;
  }

  private applyToModel(): void {
    if (!this.model) return;
    const internal = (this.model as unknown as { internalModel?: InternalModel }).internalModel;
    const core = internal?.coreModel;
    if (!core) return;

    const allParams = new Set<string>();
    for (const layer of this.layers.values()) {
      for (const key of layer.keys()) {
        allParams.add(key);
      }
    }

    for (const id of allParams) {
      const finalValue = this.mixParameter(id);
      if (finalValue !== null) {
        this.writeToCore(core, id, finalValue);
      }
    }
  }

  private mixParameter(id: string): number | null {
    const isMouthOpen = MOUTH_OPEN_PARAMS.has(id);

    if (isMouthOpen) {
      const speechVal = this.layers.get('speech')?.get(id);
      let otherMax = -Infinity;
      let hasOther = false;
      for (const [layer, params] of this.layers) {
        if (layer === 'speech') continue;
        const v = params.get(id);
        if (v !== undefined) {
          if (v > otherMax) {
            otherMax = v;
            hasOther = true;
          }
        }
      }
      if (speechVal !== undefined && hasOther) {
        return Math.max(speechVal, otherMax);
      }
      if (speechVal !== undefined) return speechVal;
      if (hasOther) return otherMax;
      return null;
    }

    let bestValue: number | null = null;
    let bestPriority = -1;
    for (const [layer, params] of this.layers) {
      const v = params.get(id);
      if (v !== undefined) {
        const priority = LAYER_PRIORITY[layer];
        if (priority > bestPriority) {
          bestPriority = priority;
          bestValue = v;
        }
      }
    }
    return bestValue;
  }

  private writeToCore(core: CoreModel, id: string, value: number): void {
    try {
      if (typeof core.setParameterValueById === 'function') {
        core.setParameterValueById(id, value, 1.0);
      } else if (typeof core.setParameterValue === 'function') {
        core.setParameterValue(id, value, 1.0);
      }
    } catch {
      /* ignore */
    }
  }

  destroy(): void {
    if (this.model && this.originalUpdate && this.installed) {
      const internal = (this.model as unknown as { internalModel?: InternalModel }).internalModel;
      if (internal) {
        internal.update = this.originalUpdate;
      }
    }
    this.model = null;
    this.originalUpdate = null;
    this.installed = false;
    this.layers.clear();
  }
}
