import { useEffect, useRef } from 'react';
import type { MutableRefObject, RefObject } from 'react';
import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Live2DLipsync } from '../utils/Live2DLipsync';
import { getCharacterId } from '../characterContext';
import { useAppStore } from '../stores/useAppStore';
import { getMixer } from '../utils/LayeredParameterMixer';
import {
  type EmotionState,
  type FacsChannels,
  emotionToFacs,
  facsToCubismParams,
  FacsSmoother,
  applyEmotionParamsToModel,
} from '../utils/EmotionFacs';

interface PsychologyStatePayload {
  character_id?: string;
  snapshot?: {
    emotion?: EmotionState;
  };
}

interface UseEmotionFacsOptions {
  lipsyncRef: RefObject<Live2DLipsync | null>;
}

const SPEAKING_WEIGHT = 0.42;
const IDLE_WEIGHT = 1.0;

function getWeight(isSpeaking: boolean): number {
  if (isSpeaking) return SPEAKING_WEIGHT;
  return IDLE_WEIGHT;
}

export function useEmotionFacs(
  modelRef: RefObject<Live2DModel | null>,
  options: UseEmotionFacsOptions,
): MutableRefObject<() => void> {
  const emotionRef = useRef<EmotionState | null>(null);
  const smootherRef = useRef<FacsSmoother>(new FacsSmoother(14));
  const lastTimeRef = useRef<number>(0);
  const tickRef = useRef<() => void>(() => {});
  const presenceState = useAppStore((s) => s.presenceState);
  const presenceRef = useRef(presenceState);
  presenceRef.current = presenceState;

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let mounted = true;

    const setup = async () => {
      const charId = getCharacterId();
      const windowLabel = getCurrentWindow().label;

      unlisten = await listen<PsychologyStatePayload>('psychology:state', (event) => {
        if (!mounted) return;
        const payload = event.payload;
        if (payload.character_id && charId && payload.character_id !== charId) return;
        if (!payload.character_id && windowLabel !== 'main' && windowLabel !== payload.character_id) return;

        const emotion = payload.snapshot?.emotion;
        if (emotion) {
          emotionRef.current = emotion;
        }
      });
    };

    setup();

    return () => {
      mounted = false;
      if (unlisten) {
        try {
          unlisten();
        } catch {
          /* ignore */
        }
      }
    };
  }, []);

  useEffect(() => {
    const tick = () => {
      if (document.hidden) return;
      const model = modelRef.current;
      if (!model) return;
      const emotion = emotionRef.current;
      if (!emotion) return;

      // rest/offline 态下跳过情绪 FACS 写入，避免 ParamAngleZ 持续变化驱动尾巴摆动
      // 同时清理 emotion 层 ParamAngleZ/Y 残留，让尾巴在休息态保持静止
      const ps = presenceRef.current;
      if (ps === 'rest' || ps === 'offline') {
        const mixer = getMixer(model);
        if (mixer) {
          mixer.clearLayerParam('emotion', 'ParamAngleZ');
          mixer.clearLayerParam('emotion', 'ParamAngleY');
        }
        lastTimeRef.current = 0;
        return;
      }

      const now = performance.now();
      if (lastTimeRef.current === 0) lastTimeRef.current = now;
      const delta = Math.min(0.1, (now - lastTimeRef.current) / 1000);
      lastTimeRef.current = now;

      const lipsync = options.lipsyncRef.current;
      const isSpeaking = lipsync?.getState() === 'speaking';
      const weight = getWeight(isSpeaking);

      const facs: FacsChannels = emotionToFacs(emotion);
      const targetParams = facsToCubismParams(facs);
      const smoothed = smootherRef.current.smooth(targetParams, delta);
      applyEmotionParamsToModel(model, smoothed, weight);
    };

    tickRef.current = tick;

    return () => {
      tickRef.current = () => {};
      smootherRef.current.reset();
      lastTimeRef.current = 0;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return tickRef;
}
